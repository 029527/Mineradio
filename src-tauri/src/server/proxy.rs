//! 音频/封面代理（替换 server.js 的 /api/audio、/api/cover）。
//! - /api/audio：透传 Range 请求/响应（音频拖动进度必需），按域名注入 Referer。
//! - /api/cover：注入 Referer 绕过防盗链，流式转发图片。

use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use super::AppState;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 按音频 URL 域名选择 Referer（对应 audioProxyHeadersFor）。
fn referer_for(url: &str) -> &'static str {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            let host = host.to_lowercase();
            if host.contains("qq.com") || host.contains("qpic.cn") {
                return "https://y.qq.com/";
            }
        }
    }
    "https://music.163.com/"
}

/// 按扩展名判定音频 Content-Type（对应 audioContentTypeForUrl）。
fn audio_content_type(url: &str, upstream: Option<&str>) -> String {
    let pathname = reqwest::Url::parse(url)
        .ok()
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    if pathname.ends_with(".flac") {
        "audio/flac".into()
    } else if pathname.ends_with(".mp3") {
        "audio/mpeg".into()
    } else if pathname.ends_with(".m4a") || pathname.ends_with(".mp4") {
        "audio/mp4".into()
    } else if pathname.ends_with(".ogg") {
        "audio/ogg".into()
    } else if pathname.ends_with(".wav") {
        "audio/wav".into()
    } else {
        upstream.unwrap_or("audio/mpeg").to_string()
    }
}

/// /api/audio?url=... —— 支持 Range 的音频代理。
pub async fn audio(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(audio_url) = q.get("url").filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "Missing url").into_response();
    };

    let mut req = st
        .client
        .get(audio_url)
        .header(header::USER_AGENT, UA)
        .header(header::REFERER, referer_for(audio_url));
    if let Some(range) = headers.get(header::RANGE) {
        req = req.header(header::RANGE, range);
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[Audio] {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    let status = upstream.status();
    let ct = audio_content_type(
        audio_url,
        upstream
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );
    let content_length = upstream.headers().get(header::CONTENT_LENGTH).cloned();
    let content_range = upstream.headers().get(header::CONTENT_RANGE).cloned();

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ct)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cl) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, cl);
    }
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }

    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "").into_response())
}

#[cfg(test)]
mod live_tests {
    use std::time::Duration;

    const PORT: u16 = 34571;

    // 联网测试：cargo test --lib -- --ignored --nocapture proxy
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn audio_range_and_cover_proxy() {
        tokio::spawn(async move {
            let _ = crate::server::serve(PORT).await;
        });
        let base = format!("http://127.0.0.1:{PORT}");
        let client = reqwest::Client::new();
        // 等待后端绑定
        for _ in 0..50 {
            if client
                .get(format!("{base}/api/app/version"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 取真实音频直链 + 封面
        let search: serde_json::Value = client
            .get(format!("{base}/api/search?keywords=%E5%91%A8%E6%9D%B0%E4%BC%A6&limit=1"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let cover = search["songs"][0]["cover"].as_str().unwrap_or("").to_string();
        assert!(cover.starts_with("http"), "无封面 URL");

        let su: serde_json::Value = client
            .get(format!("{base}/api/song/url?id=210049&quality=standard"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let audio_url = su["url"].as_str().expect("无音频 URL").to_string();

        // 音频 Range 代理
        let resp = client
            .get(format!("{base}/api/audio"))
            .query(&[("url", &audio_url)])
            .header("Range", "bytes=0-1023")
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let content_range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).map(String::from);
        let accept_ranges = resp.headers().get("accept-ranges").and_then(|v| v.to_str().ok()).map(String::from);
        let body = resp.bytes().await.unwrap();
        println!("audio: status={status}, content-range={content_range:?}, accept-ranges={accept_ranges:?}, body={} bytes", body.len());
        assert_eq!(status, 206, "上游未按 Range 返回 206");
        assert!(content_range.is_some(), "缺少 Content-Range");
        assert_eq!(body.len(), 1024, "Range 切片大小应为 1024");

        // 封面代理
        let resp = client
            .get(format!("{base}/api/cover"))
            .query(&[("url", &cover)])
            .send()
            .await
            .unwrap();
        let cover_status = resp.status();
        let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let body = resp.bytes().await.unwrap();
        println!("cover: status={cover_status}, content-type={ct}, body={} bytes", body.len());
        assert!(ct.starts_with("image/"), "封面非图片类型");
        assert!(!body.is_empty(), "封面为空");
    }
}

/// /api/cover?url=... —— 封面图代理（注入 Referer）。
pub async fn cover(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(cover_url) = q.get("url") else {
        return (StatusCode::BAD_REQUEST, "Invalid cover url").into_response();
    };
    if !(cover_url.starts_with("http://") || cover_url.starts_with("https://")) {
        return (StatusCode::BAD_REQUEST, "Invalid cover url").into_response();
    }

    let upstream = match st
        .client
        .get(cover_url)
        .header(header::USER_AGENT, UA)
        .header(header::REFERER, "https://music.163.com/")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[Cover] {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    let status = upstream.status();
    let ct = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let content_length = upstream.headers().get(header::CONTENT_LENGTH).cloned();

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ct)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .header(header::CACHE_CONTROL, "public, max-age=86400");
    if let Some(cl) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, cl);
    }

    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "").into_response())
}
