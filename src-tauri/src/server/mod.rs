//! 内嵌 HTTP 后端（替换 server.js）。
//!
//! 在本地 127.0.0.1 上提供 `/api/*` 与前端静态资源。开发期固定监听
//! `MINERADIO_DEV_API_PORT`（默认 3000，rsbuild 代理 /api 到此）；生产期
//! 由调用方探测空闲端口并把主窗口指向本服务。

pub mod netease;
pub mod proxy;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::services::ServeDir;

use netease::endpoints;

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
}

/// 探测一个空闲 TCP 端口。
pub fn find_free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

/// 前端静态资源目录（生产期由打包阶段最终确定；开发期回退到源码树）。
fn static_dir() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("../frontend/dist"),
        PathBuf::from("frontend/dist"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

fn build_router() -> Router {
    let state = AppState {
        client: reqwest::Client::builder()
            .gzip(true)
            .build()
            .expect("构建 reqwest client 失败"),
    };

    let api = Router::new()
        .route("/api/app/version", get(app_version))
        .route("/api/search", get(search))
        .route("/api/song/url", get(song_url))
        .route("/api/lyric", get(lyric))
        .route("/api/cover", get(proxy::cover))
        .route("/api/audio", get(proxy::audio))
        .route("/api/login/qr/key", get(login_qr_key))
        .route("/api/login/qr/create", get(login_qr_create))
        .route("/api/login/qr/check", get(login_qr_check))
        .route("/api/login/status", get(login_status))
        .route("/api/logout", get(logout))
        .route("/api/login/cookie", axum::routing::post(login_cookie))
        .route("/api/user/playlists", get(user_playlists))
        .route("/api/discover/home", get(discover_home))
        .with_state(state);

    match static_dir() {
        Some(dir) => {
            tracing::info!("静态资源目录: {}", dir.display());
            api.fallback_service(ServeDir::new(dir))
        }
        None => api,
    }
}

/// 在指定端口启动后端（阻塞于 serve）。供 tokio 任务调用。
pub async fn serve(port: u16) -> std::io::Result<()> {
    let app = build_router();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Mineradio 后端监听 http://{addr}");
    axum::serve(listener, app).await
}

// ---- 统一 JSON 响应（带 CORS 与禁缓存头，对应 server.js sendJSON）----
fn json_ok(value: Value) -> Response {
    (
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Json(value),
    )
        .into_response()
}

fn json_err(status: axum::http::StatusCode, value: Value) -> Response {
    (status, [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], Json(value)).into_response()
}

// ---- 路由处理 ----

async fn app_version() -> Response {
    json_ok(json!({
        "name": "mineradio",
        "productName": "Mineradio",
        "version": env!("CARGO_PKG_VERSION"),
        "update": {
            "provider": "github",
            "configured": true,
            "owner": "XxHuberrr",
            "repo": "Mineradio",
            "preview": true,
            "manifestOverride": false,
        },
    }))
}

async fn search(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let keywords = q.get("keywords").cloned().unwrap_or_default();
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(20);
    match endpoints::search(&st.client, &keywords, limit).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": e, "songs": [] }),
        ),
    }
}

async fn song_url(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let id = q.get("id").cloned().unwrap_or_default();
    let quality = q.get("quality").cloned().unwrap_or_default();
    match endpoints::song_url(&st.client, &id, &quality).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": e }),
        ),
    }
}

async fn lyric(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(id) = q.get("id").filter(|s| !s.is_empty()) else {
        return json_err(
            axum::http::StatusCode::BAD_REQUEST,
            json!({ "error": "Missing song id", "lyric": "" }),
        );
    };
    match endpoints::lyric(&st.client, id).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": e, "lyric": "" }),
        ),
    }
}

// ---- 登录 / 用户 ----

async fn login_qr_key(State(st): State<AppState>) -> Response {
    match endpoints::login_qr_key(&st.client).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e })),
    }
}

async fn login_qr_create(Query(q): Query<HashMap<String, String>>) -> Response {
    let key = q.get("key").map(|s| s.as_str()).unwrap_or("");
    json_ok(endpoints::login_qr_create(key))
}

async fn login_qr_check(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let key = q.get("key").map(|s| s.as_str()).unwrap_or("");
    match endpoints::login_qr_check(&st.client, key).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e })),
    }
}

async fn login_status(State(st): State<AppState>) -> Response {
    json_ok(endpoints::login_status(&st.client).await)
}

async fn logout(State(st): State<AppState>) -> Response {
    json_ok(endpoints::logout(&st.client).await)
}

async fn login_cookie(State(st): State<AppState>, body: String) -> Response {
    // 接受 JSON {cookie|data|text} 或原始 cookie 串。
    let raw = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("cookie")
                .or_else(|| v.get("data"))
                .or_else(|| v.get("text"))
                .and_then(|x| x.as_str())
                .map(String::from)
        })
        .unwrap_or(body);
    json_ok(endpoints::login_cookie(&st.client, &raw).await)
}

async fn user_playlists(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(60).clamp(12, 100);
    json_ok(endpoints::user_playlists(&st.client, limit).await)
}

async fn discover_home(State(st): State<AppState>) -> Response {
    json_ok(endpoints::discover_home(&st.client).await)
}

#[cfg(test)]
mod login_tests {
    use std::time::Duration;

    const PORT: u16 = 34572;

    // 联网测试：cargo test --lib -- --ignored --nocapture login_qr_flow
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_qr_flow_anonymous() {
        tokio::spawn(async move {
            let _ = super::serve(PORT).await;
        });
        let base = format!("http://127.0.0.1:{PORT}");
        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client.get(format!("{base}/api/app/version")).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let key_resp: serde_json::Value = client.get(format!("{base}/api/login/qr/key")).send().await.unwrap().json().await.unwrap();
        let key = key_resp["key"].as_str().expect("无 unikey");
        println!("unikey = {key}");
        assert!(!key.is_empty());

        let create: serde_json::Value = client.get(format!("{base}/api/login/qr/create")).query(&[("key", key)]).send().await.unwrap().json().await.unwrap();
        let img = create["img"].as_str().unwrap_or("");
        println!("qr img prefix = {}", &img.chars().take(30).collect::<String>());
        assert!(img.starts_with("data:image/png;base64,"), "二维码非 PNG data URL");

        let check: serde_json::Value = client.get(format!("{base}/api/login/qr/check")).query(&[("key", key)]).send().await.unwrap().json().await.unwrap();
        let code = check["code"].as_i64().unwrap_or(0);
        println!("qr check code = {code} (801=等待扫码)");
        assert!(code == 801 || code == 800, "未扫码应为 801/800, 实际 {code}: {check}");

        let pl: serde_json::Value = client.get(format!("{base}/api/user/playlists")).send().await.unwrap().json().await.unwrap();
        assert_eq!(pl["loggedIn"].as_bool(), Some(false), "匿名应未登录");

        let home: serde_json::Value = client.get(format!("{base}/api/discover/home")).send().await.unwrap().json().await.unwrap();
        assert_eq!(home["mode"].as_str(), Some("starter"), "匿名首页应为 starter");
        println!("登录链路匿名态全部符合预期");
    }
}
