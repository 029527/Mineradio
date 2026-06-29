//! 内嵌 HTTP 后端（替换 server.js）。
//!
//! 在本地 127.0.0.1 上提供 `/api/*` 与前端静态资源。开发期固定监听
//! `MINERADIO_DEV_API_PORT`（默认 3000，rsbuild 代理 /api 到此）；生产期
//! 由调用方探测空闲端口并把主窗口指向本服务。

pub mod dj_analyzer;
pub mod netease;
pub mod proxy;
pub mod qq;
pub mod update;
pub mod weather;

use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener};

use axum::{
    extract::{Query, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use netease::endpoints;

/// 前端静态资源：debug 期从磁盘 `../frontend/dist` 读取（改完前端 `bun run build` 即可刷新），
/// release 期编入二进制（打包后无需外部目录）。
#[derive(rust_embed::RustEmbed)]
#[folder = "../frontend/dist"]
struct FrontendAssets;

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
}

/// 绑定一个随机空闲高位端口的监听器（127.0.0.1）。绑定即处于 LISTEN 状态，
/// 故可在启动 axum 前先建窗口而不丢连接。
pub fn bind_listener() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

fn build_router() -> Router {
    let state = AppState {
        client: reqwest::Client::builder()
            .gzip(true)
            // 关键：禁用系统代理。reqwest 默认读取 HTTP(S)_PROXY/ALL_PROXY 环境变量，
            // 会把网易云/QQ 请求经 Clash/Surge 等代理路由到境外被拒(error sending request)。
            // 原版 Node http/https 默认直连，这里对齐为直连。
            .no_proxy()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("构建 reqwest client 失败"),
    };

    let api = Router::new()
        .route("/api/app/version", get(app_version))
        .route("/api/update/latest", get(update_latest))
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
        .route("/api/playlist/tracks", get(playlist_tracks))
        .route("/api/discover/home", get(discover_home))
        .route("/api/song/like/check", get(song_like_check))
        .route("/api/song/like", get(song_like))
        .route("/api/song/comments", get(song_comments))
        .route("/api/artist/detail", get(artist_detail))
        .route("/api/playlist/create", get(playlist_create))
        .route("/api/playlist/add-song", axum::routing::post(playlist_add_song))
        .route("/api/weather/radio", get(weather_radio))
        .route("/api/weather/ip-location", get(weather_ip_location))
        .route("/api/podcast/search", get(podcast_search))
        .route("/api/podcast/hot", get(podcast_hot))
        .route("/api/podcast/detail", get(podcast_detail))
        .route("/api/podcast/programs", get(podcast_programs))
        .route("/api/podcast/my", get(podcast_my))
        .route("/api/podcast/dj-beatmap", get(podcast_dj_beatmap))
        .route("/api/beatmap/cache/status", get(beatmap_cache_status))
        .route("/api/beatmap/cache", get(beatmap_cache_get).post(beatmap_cache_post))
        .route("/api/qq/search", get(qq_search))
        .route("/api/qq/song/url", get(qq_song_url))
        .route("/api/qq/lyric", get(qq_lyric))
        .route("/api/qq/login/status", get(qq_login_status))
        .route("/api/qq/login/cookie", axum::routing::post(qq_login_cookie))
        .route("/api/qq/logout", get(qq_logout))
        // 未实现的 /api/* 返回 JSON 404（避免落到静态回退被当成 HTML 解析）。
        .route("/api/*rest", get(api_not_found).post(api_not_found))
        .with_state(state);

    // 非 /api 路径 → 嵌入式前端静态资源
    api.fallback(static_asset)
}

/// 从嵌入资源返回前端文件（`/` → index.html）。
async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match FrontendAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// 用已绑定的监听器启动后端（阻塞于 serve）。供 tokio 任务调用。
pub async fn serve(listener: TcpListener) -> std::io::Result<()> {
    let app = build_router();
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    tracing::info!("Mineradio 后端监听 http://{}", listener.local_addr()?);
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

async fn api_not_found(uri: axum::http::Uri) -> Response {
    json_err(
        axum::http::StatusCode::NOT_FOUND,
        json!({ "error": "NOT_IMPLEMENTED", "path": uri.path() }),
    )
}

async fn update_latest(State(st): State<AppState>) -> Response {
    json_ok(update::latest(&st.client).await)
}

async fn app_version() -> Response {
    json_ok(json!({
        "name": "mineradio",
        "productName": "Mineradio",
        "version": env!("CARGO_PKG_VERSION"),
        "update": {
            "provider": "github",
            "configured": true,
            "owner": "029527",
            "repo": "Mineradio",
            "preview": false,
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

async fn weather_radio(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let city = q.get("city").or_else(|| q.get("q")).cloned().unwrap_or_default();
    let lat = q.get("lat").and_then(|v| v.parse::<f64>().ok());
    let lon = q.get("lon").and_then(|v| v.parse::<f64>().ok());
    let tz = q.get("timezone").cloned().unwrap_or_default();
    json_ok(weather::weather_radio(&st.client, &city, lat, lon, &tz).await)
}

async fn weather_ip_location(State(st): State<AppState>) -> Response {
    json_ok(weather::ip_location(&st.client).await)
}

async fn podcast_search(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let kw = q.get("keywords").cloned().unwrap_or_default();
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(18).clamp(6, 30);
    match endpoints::podcast_search(&st.client, &kw, limit).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "podcasts": [] })),
    }
}

async fn podcast_hot(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(18).clamp(6, 30);
    let offset = q.get("offset").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0);
    match endpoints::podcast_hot(&st.client, limit, offset).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "podcasts": [] })),
    }
}

async fn podcast_detail(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(rid) = q.get("id").or_else(|| q.get("rid")).filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing podcast id" }));
    };
    match endpoints::podcast_detail(&st.client, rid).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e })),
    }
}

async fn podcast_programs(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(rid) = q.get("id").or_else(|| q.get("rid")).filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing podcast id", "programs": [] }));
    };
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(30).clamp(10, 60);
    let offset = q.get("offset").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0);
    match endpoints::podcast_programs(&st.client, rid, limit, offset).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "programs": [] })),
    }
}

async fn podcast_my(State(st): State<AppState>) -> Response {
    json_ok(endpoints::podcast_my(&st.client).await)
}

async fn podcast_dj_beatmap(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(url) = q.get("url").filter(|u| u.starts_with("http")) else {
        return json_err(StatusCode::BAD_REQUEST, json!({ "error": "Invalid audio url" }));
    };
    let duration = q.get("duration").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).max(0.0);
    let intro = q.get("intro").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).max(0.0);
    let result = if intro > 0.0 {
        dj_analyzer::analyze_intro(&st.client, url, duration, intro).await
    } else {
        dj_analyzer::analyze_stream(&st.client, url, duration).await
    };
    match result {
        Ok(map) => json_ok(json!({ "ok": true, "map": map })),
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, json!({ "ok": false, "error": e })),
    }
}

// 节拍图缓存：内存模式（客户端自行缓存；服务端不持久化）。
async fn beatmap_cache_status() -> Response {
    json_ok(json!({ "enabled": false, "mode": "memory-only", "reason": "SERVER_CACHE_DISABLED" }))
}

async fn beatmap_cache_get(Query(q): Query<HashMap<String, String>>) -> Response {
    json_ok(json!({ "ok": true, "hit": false, "key": q.get("key").cloned().unwrap_or_default() }))
}

async fn beatmap_cache_post() -> Response {
    json_ok(json!({ "ok": true, "enabled": false, "mode": "memory-only" }))
}

// ---- QQ 音乐 ----

async fn qq_search(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let kw = q.get("keywords").cloned().unwrap_or_default();
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(8).clamp(4, 12);
    json_ok(qq::search(&st.client, &kw, limit).await)
}

async fn qq_song_url(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mid = q.get("mid").or_else(|| q.get("id")).cloned().unwrap_or_default();
    let media_mid = q.get("mediaMid").or_else(|| q.get("media_mid")).cloned().unwrap_or_default();
    let quality = q.get("quality").cloned().unwrap_or_default();
    json_ok(qq::song_url(&st.client, &mid, &media_mid, &quality).await)
}

async fn qq_lyric(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mid = q.get("mid").or_else(|| q.get("songmid")).cloned().unwrap_or_default();
    let id = q.get("id").or_else(|| q.get("qqId")).cloned().unwrap_or_default();
    if mid.is_empty() && id.is_empty() {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "provider": "qq", "error": "Missing QQ song mid or id", "lyric": "" }));
    }
    json_ok(qq::lyric(&st.client, &mid, &id).await)
}

async fn qq_login_status() -> Response {
    json_ok(qq::login_status())
}

async fn qq_login_cookie(body: String) -> Response {
    let raw = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("cookie").or_else(|| v.get("data")).or_else(|| v.get("text")).and_then(|x| x.as_str()).map(String::from))
        .unwrap_or(body);
    json_ok(qq::login_cookie(&raw))
}

async fn qq_logout() -> Response {
    json_ok(qq::logout())
}

async fn song_like_check(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let ids: Vec<i64> = q
        .get("ids")
        .or_else(|| q.get("id"))
        .map(|s| s.split(',').filter_map(|x| x.trim().parse::<i64>().ok()).collect())
        .unwrap_or_default();
    json_ok(endpoints::song_like_check(&st.client, ids).await)
}

async fn song_like(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let id = q.get("id").cloned().unwrap_or_default();
    let like = q.get("like").map(|v| v != "false").unwrap_or(true);
    if id.is_empty() {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing song id" }));
    }
    json_ok(endpoints::song_like(&st.client, &id, like).await)
}

async fn song_comments(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(id) = q.get("id").filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing song id", "comments": [] }));
    };
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(20).clamp(6, 50);
    let offset = q.get("offset").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0);
    match endpoints::song_comments(&st.client, id, limit, offset).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "comments": [] })),
    }
}

async fn artist_detail(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(id) = q.get("id").filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing artist id", "songs": [] }));
    };
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(30).clamp(10, 80);
    match endpoints::artist_detail(&st.client, id, limit).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "songs": [] })),
    }
}

async fn playlist_create(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(name) = q.get("name").filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing playlist name" }));
    };
    let privacy = q.get("privacy").cloned().unwrap_or_else(|| "0".into());
    json_ok(endpoints::playlist_create(&st.client, name, &privacy).await)
}

async fn playlist_add_song(State(st): State<AppState>, body: String) -> Response {
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let pid = v.get("pid").and_then(|x| x.as_str().map(String::from).or_else(|| x.as_i64().map(|n| n.to_string()))).unwrap_or_default();
    let id = v
        .get("id")
        .or_else(|| v.get("ids"))
        .and_then(|x| x.as_str().map(String::from).or_else(|| x.as_i64().map(|n| n.to_string())))
        .unwrap_or_default();
    if pid.is_empty() || id.is_empty() {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing playlist id or song id" }));
    }
    json_ok(endpoints::playlist_add_song(&st.client, &pid, &id).await)
}

async fn playlist_tracks(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(id) = q.get("id").filter(|s| !s.is_empty()) else {
        return json_err(axum::http::StatusCode::BAD_REQUEST, json!({ "error": "Missing playlist id", "tracks": [] }));
    };
    match endpoints::playlist_tracks(&st.client, id).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e, "tracks": [] })),
    }
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

        // 公开歌单 tracks（匿名可取）：3778678 = 云音乐热歌榜
        let pt: serde_json::Value = client.get(format!("{base}/api/playlist/tracks")).query(&[("id", "3778678")]).send().await.unwrap().json().await.unwrap();
        let tracks = pt["tracks"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("歌单 '{}' 取到 {} 首", pt["playlist"]["name"].as_str().unwrap_or("?"), tracks);
        assert!(tracks > 0, "公开歌单应有歌曲: {pt}");
        assert!(pt["tracks"][0]["name"].as_str().is_some(), "歌曲缺 name");

        // 评论（公开热评，匿名可取）
        let cm: serde_json::Value = client.get(format!("{base}/api/song/comments")).query(&[("id", "210049")]).send().await.unwrap().json().await.unwrap();
        let n = cm["comments"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("评论取到 {n} 条 (hot={})", cm["hot"]);
        assert!(n > 0, "应有热评: {cm}");

        // 歌手页（周杰伦 6452，匿名可取热门歌）
        let ar: serde_json::Value = client.get(format!("{base}/api/artist/detail")).query(&[("id", "6452")]).send().await.unwrap().json().await.unwrap();
        let asongs = ar["songs"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("歌手 '{}' 取到 {} 首热门歌", ar["artist"]["name"].as_str().unwrap_or("?"), asongs);
        assert!(asongs > 0, "歌手应有热门歌: {ar}");

        // 天气电台（上海）
        let wr: serde_json::Value = client.get(format!("{base}/api/weather/radio")).query(&[("city", "上海")]).send().await.unwrap().json().await.unwrap();
        let wsongs = wr["radio"]["songs"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("天气电台: ok={}, {}℃ {} → '{}' 组了 {} 首",
            wr["ok"], wr["weather"]["temperature"], wr["weather"]["label"].as_str().unwrap_or("?"),
            wr["radio"]["title"].as_str().unwrap_or("?"), wsongs);
        assert_eq!(wr["ok"].as_bool(), Some(true), "天气电台应 ok: {wr}");
        assert!(wsongs > 0, "天气电台应有歌曲");

        // 播客热门
        let ph: serde_json::Value = client.get(format!("{base}/api/podcast/hot")).query(&[("limit", "6")]).send().await.unwrap().json().await.unwrap();
        let pn = ph["podcasts"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("播客热门 {pn} 个 (示例: '{}')", ph["podcasts"][0]["name"].as_str().unwrap_or("?"));
        assert!(pn > 0, "应有热门播客: {ph}");

        // 播客节目（取第一个热门播客的节目）
        if let Some(rid) = ph["podcasts"][0]["id"].as_i64() {
            let pp: serde_json::Value = client.get(format!("{base}/api/podcast/programs")).query(&[("id", &rid.to_string())]).send().await.unwrap().json().await.unwrap();
            let progn = pp["programs"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("播客 {} 节目 {} 集", rid, progn);
            assert!(progn > 0, "播客应有节目: {pp}");
        }

        // QQ 音乐搜索
        let qs: serde_json::Value = client.get(format!("{base}/api/qq/search")).query(&[("keywords", "周杰伦"), ("limit", "5")]).send().await.unwrap().json().await.unwrap();
        let qn = qs["songs"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("QQ 搜索 {} 首 (示例: '{}' / '{}', cover={})", qn,
            qs["songs"][0]["name"].as_str().unwrap_or("?"), qs["songs"][0]["artist"].as_str().unwrap_or("?"),
            !qs["songs"][0]["cover"].as_str().unwrap_or("").is_empty());
        assert!(qn > 0, "QQ 应有搜索结果: {qs}");

        // QQ 歌词
        if let Some(mid) = qs["songs"][0]["mid"].as_str() {
            let ql: serde_json::Value = client.get(format!("{base}/api/qq/lyric")).query(&[("mid", mid)]).send().await.unwrap().json().await.unwrap();
            let has = !ql["lyric"].as_str().unwrap_or("").is_empty();
            println!("QQ 歌词 mid={mid}: {}字符", ql["lyric"].as_str().unwrap_or("").chars().count());
            assert!(has, "QQ 歌词应非空: {ql}");
            // QQ song url（匿名通常试听/受限，校验返回结构）
            let qu: serde_json::Value = client.get(format!("{base}/api/qq/song/url")).query(&[("mid", mid)]).send().await.unwrap().json().await.unwrap();
            println!("QQ song/url: provider={}, playable={}, url={}", qu["provider"], qu["playable"], !qu["url"].as_str().unwrap_or("").is_empty());
            assert_eq!(qu["provider"].as_str(), Some("qq"));
        }

        // 更新检查
        let up: serde_json::Value = client.get(format!("{base}/api/update/latest")).send().await.unwrap().json().await.unwrap();
        println!("更新检查: 当前 {} / 最新 {} / 有更新={} / 资产='{}'",
            up["currentVersion"].as_str().unwrap_or("?"), up["latestVersion"].as_str().unwrap_or("?"),
            up["updateAvailable"], up["release"]["asset"]["name"].as_str().unwrap_or("无"));
        assert!(up["latestVersion"].as_str().is_some(), "应有 latestVersion: {up}");
        assert!(up["configured"].as_bool().unwrap_or(false), "更新应已配置");
    }
}
