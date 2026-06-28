//! 网易云请求组装：eapi 与 weapi 两种路径，注入登录 cookie 并捕获 Set-Cookie。
//! 复刻 request.js 的 eapi / weapi 分支。

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::SET_COOKIE;
use serde_json::{json, Value};

use super::{cookie_store, crypto};

const API_DOMAIN: &str = "https://interface.music.163.com";
const WEAPI_DOMAIN: &str = "https://music.163.com";
// request.js: chooseUserAgent('api','iphone')
const API_UA: &str = "NeteaseMusic 9.0.90/5038 (iPhone; iOS 16.2; zh_CN)";
// request.js: chooseUserAgent('weapi')
const WEAPI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0";

/// 一次请求的结果：解析后的 JSON + 响应携带的 Set-Cookie（已去属性的 k=v）。
pub struct ApiResponse {
    pub body: Value,
    pub cookies: Vec<String>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

fn collect_set_cookies(resp: &reqwest::Response) -> Vec<String> {
    resp.headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .map(|s| s.trim().to_string())
        .filter(|s| s.contains('='))
        .collect()
}

async fn parse_body(resp: reqwest::Response) -> Result<Value, String> {
    let text = resp.text().await.map_err(|e| e.to_string())?;
    serde_json::from_str::<Value>(&text).map_err(|e| {
        format!("响应非 JSON: {e} — 前 200 字符: {}", text.chars().take(200).collect::<String>())
    })
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// eapi 请求。`uri` 为原始 `/api/...`，`data` 为业务参数对象。
pub async fn request_eapi(
    client: &reqwest::Client,
    uri: &str,
    mut data: Value,
) -> Result<ApiResponse, String> {
    let buildver = (now_ms() / 1000).to_string();
    let request_id = format!("{}_{:04}", now_ms(), now_ms() % 1000);

    let mut header = json!({
        "osver": "Microsoft-Windows-10-Professional-build-19045-64bit",
        "deviceId": "",
        "os": "pc",
        "appver": "3.1.17.204416",
        "versioncode": "140",
        "mobilename": "",
        "buildver": buildver,
        "resolution": "1920x1080",
        "__csrf": cookie_store::csrf(),
        "channel": "netease",
        "requestId": request_id,
    });
    if let Some(mu) = cookie_store::music_u() {
        header["MUSIC_U"] = json!(mu);
    }
    let cookie = header_to_cookie(&header);

    data["header"] = header;
    data["e_r"] = json!(false);

    let json_text = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    let params = crypto::eapi(uri, &json_text);
    let url = format!("{}/eapi/{}", API_DOMAIN, &uri[5..]);

    let resp = client
        .post(&url)
        .header(reqwest::header::USER_AGENT, API_UA)
        .header(reqwest::header::REFERER, "https://music.163.com")
        .header(reqwest::header::COOKIE, cookie)
        .form(&[("params", params)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let cookies = collect_set_cookies(&resp);
    let body = parse_body(resp).await?;
    Ok(ApiResponse { body, cookies })
}

/// weapi 请求。受保护接口（登录态/账户）走此路径。
pub async fn request_weapi(
    client: &reqwest::Client,
    uri: &str,
    mut data: Value,
) -> Result<ApiResponse, String> {
    let csrf = cookie_store::csrf();
    data["csrf_token"] = json!(csrf);
    let json_text = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    let (params, enc_sec_key) = crypto::weapi(&json_text);
    let url = format!("{}/weapi/{}?csrf_token={}", WEAPI_DOMAIN, &uri[5..], csrf);

    // weapi 需要登录 cookie；匿名时附带最小设备 cookie。
    let mut cookie = cookie_store::full();
    if cookie.is_empty() {
        cookie = "os=pc; appver=3.1.17.204416".to_string();
    } else {
        cookie.push_str("; os=pc");
    }

    let resp = client
        .post(&url)
        .header(reqwest::header::USER_AGENT, WEAPI_UA)
        .header(reqwest::header::REFERER, "https://music.163.com")
        .header(reqwest::header::COOKIE, cookie)
        .form(&[("params", params), ("encSecKey", enc_sec_key)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let cookies = collect_set_cookies(&resp);
    let body = parse_body(resp).await?;
    Ok(ApiResponse { body, cookies })
}

/// 便捷：仅取 eapi 响应 body（多数业务端点用）。
pub async fn eapi_request(client: &reqwest::Client, uri: &str, data: Value) -> Result<Value, String> {
    request_eapi(client, uri, data).await.map(|r| r.body)
}

/// 便捷：仅取 weapi 响应 body。
pub async fn weapi_request(client: &reqwest::Client, uri: &str, data: Value) -> Result<Value, String> {
    request_weapi(client, uri, data).await.map(|r| r.body)
}

/// header 对象转 Cookie 字符串（对应 createHeaderCookie）。
fn header_to_cookie(header: &Value) -> String {
    let obj = header.as_object().unwrap();
    obj.iter()
        .map(|(k, v)| {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", url_encode(k), url_encode(&val))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[ignore]
    #[tokio::test]
    async fn live_search_zhoujielun() {
        let client = reqwest::Client::new();
        let data = json!({ "s": "周杰伦", "type": 1, "limit": 3, "offset": 0 });
        let res = eapi_request(&client, "/api/search/get", data).await.expect("请求失败");
        let code = res.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let songs = res
            .get("result")
            .and_then(|r| r.get("songs"))
            .and_then(|s| s.as_array());
        assert_eq!(code, 200, "网易云返回非 200: {res}");
        assert!(songs.map(|s| !s.is_empty()).unwrap_or(false), "无搜索结果");
    }

    #[ignore]
    #[tokio::test]
    async fn live_lyric() {
        let client = reqwest::Client::new();
        let data = json!({ "id": 210049, "tv": -1, "lv": -1, "rv": -1, "kv": -1, "_nmclfl": 1 });
        let res = eapi_request(&client, "/api/song/lyric", data).await.expect("请求失败");
        assert_eq!(res.get("code").and_then(|c| c.as_i64()).unwrap_or(-1), 200);
    }
}
