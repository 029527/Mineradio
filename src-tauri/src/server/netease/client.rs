//! 网易云请求组装（eapi 路径）。复刻 request.js 的 eapi 分支：
//! 构造 header → eapi 加密 → POST `/eapi/<uri.substr(5)>`，body = `params=<HEX>`。

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::crypto;

const API_DOMAIN: &str = "https://interface.music.163.com";
// request.js: chooseUserAgent('api','iphone')
const API_UA: &str = "NeteaseMusic 9.0.90/5038 (iPhone; iOS 16.2; zh_CN)";

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// header 对象转 Cookie 字符串（对应 createHeaderCookie：encodeURIComponent 后用 "; " 连接）。
fn header_to_cookie(header: &Value) -> String {
    let obj = header.as_object().unwrap();
    obj.iter()
        .map(|(k, v)| {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", urlencode(k), urlencode(&val))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 最小 encodeURIComponent（仅转义 cookie 中可能出问题的字符）。
fn urlencode(s: &str) -> String {
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

/// 发起一个 eapi 请求。`uri` 为原始 `/api/...` 路径，`data` 为业务参数对象。
/// 可选 `music_u`（登录态 MUSIC_U cookie）。返回解析后的 JSON。
pub async fn eapi_request(
    client: &reqwest::Client,
    uri: &str,
    mut data: Value,
    music_u: Option<&str>,
) -> Result<Value, String> {
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
        "__csrf": "",
        "channel": "netease",
        "requestId": request_id,
    });
    if let Some(mu) = music_u {
        header["MUSIC_U"] = json!(mu);
    }

    let cookie = header_to_cookie(&header);

    // request.js: data.header = header; data.e_r = false
    data["header"] = header;
    data["e_r"] = json!(false);

    let json_text = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    let params = crypto::eapi(uri, &json_text);
    let url = format!("{}/eapi/{}", API_DOMAIN, &uri[5..]);

    let resp = client
        .post(&url)
        .header("User-Agent", API_UA)
        .header("Referer", "https://music.163.com")
        .header("Cookie", cookie)
        .form(&[("params", params)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let body = resp.text().await.map_err(|e| e.to_string())?;
    serde_json::from_str::<Value>(&body)
        .map_err(|e| format!("响应非 JSON: {e} — 前 200 字符: {}", &body.chars().take(200).collect::<String>()))
}

#[cfg(test)]
mod live_tests {
    use super::*;

    // 联网测试，默认忽略；运行： cargo test --lib -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn live_search_zhoujielun() {
        let client = reqwest::Client::new();
        let data = json!({ "s": "周杰伦", "type": 1, "limit": 3, "offset": 0 });
        let res = eapi_request(&client, "/api/search/get", data, None)
            .await
            .expect("请求失败");
        let code = res.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        println!("code = {code}");
        let songs = res
            .get("result")
            .and_then(|r| r.get("songs"))
            .and_then(|s| s.as_array());
        if let Some(songs) = songs {
            println!("命中 {} 首：", songs.len());
            for s in songs {
                println!(
                    "  - {} / {}",
                    s.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                    s.get("id").and_then(|i| i.as_i64()).unwrap_or(0)
                );
            }
        }
        assert_eq!(code, 200, "网易云返回非 200: {res}");
        assert!(songs.map(|s| !s.is_empty()).unwrap_or(false), "无搜索结果");
    }

    #[ignore]
    #[tokio::test]
    async fn live_lyric() {
        let client = reqwest::Client::new();
        // 布拉格广场 210049
        let data = json!({ "id": 210049, "tv": -1, "lv": -1, "rv": -1, "kv": -1, "_nmclfl": 1 });
        let res = eapi_request(&client, "/api/song/lyric", data, None)
            .await
            .expect("请求失败");
        let lrc = res
            .get("lrc")
            .and_then(|l| l.get("lyric"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        println!("歌词前 80 字符: {}", lrc.chars().take(80).collect::<String>());
        assert_eq!(res.get("code").and_then(|c| c.as_i64()).unwrap_or(-1), 200);
        assert!(!lrc.is_empty(), "歌词为空");
    }

    #[ignore]
    #[tokio::test]
    async fn live_song_url_v1() {
        let client = reqwest::Client::new();
        let data = json!({ "ids": "[210049]", "level": "standard", "encodeType": "flac" });
        let res = eapi_request(&client, "/api/song/enhance/player/url/v1", data, None)
            .await
            .expect("请求失败");
        let code = res.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let url = res
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("url"))
            .and_then(|u| u.as_str());
        println!("code={code}, url={url:?}");
        assert_eq!(code, 200, "返回非 200: {res}");
        // 匿名态部分歌曲可能返回 null（需登录），故仅校验 code，不强制 url 存在。
    }
}
