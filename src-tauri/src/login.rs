//! webview 扫码登录窗（替换 main.js 的 openNeteaseMusicLoginWindow / openQQMusicLoginWindow）。
//!
//! 打开官方登录页 → 异步轮询该 webview 的 cookie 直到出现关键登录票据 → 关窗返回完整
//! cookie 串。前端拿到后 POST 到 /api/*/login/cookie 保存。
//! 注意：Tauri 文档指出 Windows 上 cookies() 在同步 command 会死锁，故全程 async 轮询。

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

struct ProviderCfg {
    label: &'static str,
    title: &'static str,
    login_url: &'static str,
    cookie_urls: &'static [&'static str],
}

/// 网易云 webview 登录（主路径仍是扫码 /api/login/qr/*，此为网页登录补充）。
pub async fn open_netease(app: AppHandle) -> Value {
    open_login(
        app,
        ProviderCfg {
            label: "netease-login",
            title: "网易云音乐登录",
            login_url: "https://music.163.com/#/login",
            cookie_urls: &["https://music.163.com"],
        },
        |c| c.get("MUSIC_U").map(|v| !v.is_empty()).unwrap_or(false),
    )
    .await
}

/// QQ 音乐 webview 登录（QQ 无服务端扫码接口，必须靠此抓 cookie）。
pub async fn open_qq(app: AppHandle) -> Value {
    open_login(
        app,
        ProviderCfg {
            label: "qq-login",
            title: "QQ 音乐登录",
            login_url: "https://y.qq.com/n/ryqq/profile",
            cookie_urls: &["https://y.qq.com", "https://c.y.qq.com"],
        },
        |c| qq_has_uin(c) && qq_has_key(c),
    )
    .await
}

fn qq_has_uin(c: &BTreeMap<String, String>) -> bool {
    ["uin", "qqmusic_uin", "wxuin", "p_uin"]
        .iter()
        .any(|k| c.get(*k).map(|v| v.chars().any(|ch| ch.is_ascii_digit())).unwrap_or(false))
}

fn qq_has_key(c: &BTreeMap<String, String>) -> bool {
    ["qm_keyst", "qqmusic_key", "music_key", "p_skey", "skey", "psrf_qqaccess_token", "psrf_qqrefresh_token", "wxrefresh_token", "wxskey"]
        .iter()
        .any(|k| c.get(*k).map(|v| !v.is_empty()).unwrap_or(false))
}

async fn open_login(
    app: AppHandle,
    cfg: ProviderCfg,
    ready: impl Fn(&BTreeMap<String, String>) -> bool,
) -> Value {
    // 关掉可能存在的旧登录窗
    if let Some(w) = app.get_webview_window(cfg.label) {
        let _ = w.close();
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let url: tauri::Url = match cfg.login_url.parse() {
        Ok(u) => u,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let win = match WebviewWindowBuilder::new(&app, cfg.label, WebviewUrl::External(url))
        .title(cfg.title)
        .inner_size(900.0, 720.0)
        .min_inner_size(760.0, 560.0)
        .center()
        .build()
    {
        Ok(w) => w,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let _ = win.set_focus();

    // 轮询 cookie，最长约 4 分钟
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(w) = app.get_webview_window(cfg.label) else {
            // 用户手动关闭了登录窗
            return json!({ "ok": false, "canceled": true });
        };
        let map = collect_cookies(&w, cfg.cookie_urls);
        if ready(&map) {
            let cookie = render_cookie(&map);
            let _ = w.close();
            return json!({ "ok": true, "cookie": cookie });
        }
    }

    // 超时：返回已有 cookie（可能不完整）
    if let Some(w) = app.get_webview_window(cfg.label) {
        let map = collect_cookies(&w, cfg.cookie_urls);
        let _ = w.close();
        return json!({ "ok": false, "error": "LOGIN_TIMEOUT", "cookie": render_cookie(&map), "partial": true });
    }
    json!({ "ok": false, "error": "LOGIN_TIMEOUT" })
}

fn collect_cookies(win: &tauri::WebviewWindow, urls: &[&str]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for u in urls {
        if let Ok(url) = u.parse::<tauri::Url>() {
            if let Ok(cookies) = win.cookies_for_url(url) {
                for c in cookies {
                    let name = c.name().to_string();
                    let value = c.value().to_string();
                    if !name.is_empty() && !value.is_empty() {
                        map.entry(name).or_insert(value);
                    }
                }
            }
        }
    }
    map
}

fn render_cookie(map: &BTreeMap<String, String>) -> String {
    map.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
}
