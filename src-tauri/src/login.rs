//! webview 扫码登录窗（替换 main.js 的 openNeteaseMusicLoginWindow / openQQMusicLoginWindow）。
//!
//! 打开官方登录页 WebviewWindow → 异步轮询该 webview 的 cookie 直到出现关键登录票据
//! → 关窗返回完整 cookie 串。前端拿到后 POST 到 /api/*/login/cookie 保存。
//! QQ 的播放票据 qm_keyst 需登录后再访问一次播放页才下发，故有"预热"跳转。
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
    warmup_url: Option<&'static str>,
}

/// 网易云 webview 登录（主路径仍是扫码 /api/login/qr/*，此为网页登录补充）。
pub async fn open_netease(app: AppHandle) -> Value {
    open_login(
        app,
        ProviderCfg {
            label: "netease-login",
            title: "网易云音乐登录",
            login_url: "https://music.163.com/#/login",
            cookie_urls: &["https://music.163.com", "https://music.163.com/", "https://163.com"],
            warmup_url: None,
        },
        |c| c.get("MUSIC_U").map(|v| !v.is_empty()).unwrap_or(false),
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
            cookie_urls: &[
                "https://y.qq.com",
                "https://c.y.qq.com",
                "https://i.y.qq.com",
                "https://qq.com",
                "https://graph.qq.com",
                "https://u.y.qq.com",
            ],
            warmup_url: Some("https://y.qq.com/n/ryqq/player"),
        },
        // 完整：有 uin 且有播放票据
        |c| qq_has_uin(c) && qq_has_key(c),
        // 部分：已登录（有 uin），可触发预热取 qm_keyst
        qq_has_uin,
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
    partial: impl Fn(&BTreeMap<String, String>) -> bool,
) -> Value {
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

    let mut warmed = false;
    let mut warmup_at: Option<usize> = None;

    for i in 0..240 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(w) = app.get_webview_window(cfg.label) else {
            return json!({ "ok": false, "canceled": true });
        };
        let map = collect_cookies(&w, cfg.cookie_urls);
        // 诊断日志：看每轮读到哪些 cookie 名
        tracing::warn!("[LOGIN {} #{i}] cookies: {:?}", cfg.label, map.keys().collect::<Vec<_>>());

        if ready(&map) {
            let cookie = render_cookie(&map);
            let _ = w.close();
            return json!({ "ok": true, "cookie": cookie });
        }

        // 已登录但还差播放票据 → 预热跳转一次（QQ）
        if !warmed && partial(&map) {
            if let Some(wu) = cfg.warmup_url {
                warmed = true;
                warmup_at = Some(i);
                tracing::warn!("[LOGIN {}] warmup → {wu}", cfg.label);
                let _ = w.eval(&format!("window.location.assign('{wu}')"));
            }
        }

        // 预热后再等 ~12s 仍只有 uin、无票据：返回 partial 让前端走兜底
        if let Some(wat) = warmup_at {
            if i - wat >= 12 && partial(&map) && !ready(&map) {
                let cookie = render_cookie(&map);
                let _ = w.close();
                return json!({ "ok": true, "cookie": cookie, "partial": true });
            }
        }
    }

    if let Some(w) = app.get_webview_window(cfg.label) {
        let map = collect_cookies(&w, cfg.cookie_urls);
        let _ = w.close();
        return json!({ "ok": false, "error": "LOGIN_TIMEOUT", "cookie": render_cookie(&map), "partial": true });
    }
    json!({ "ok": false, "error": "LOGIN_TIMEOUT" })
}

fn collect_cookies(win: &tauri::WebviewWindow, urls: &[&str]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    // 先尝试 cookies()（全量），再按 url 兜底
    if let Ok(all) = win.cookies() {
        for c in all {
            let (n, v) = (c.name().to_string(), c.value().to_string());
            if !n.is_empty() && !v.is_empty() {
                map.entry(n).or_insert(v);
            }
        }
    }
    for u in urls {
        if let Ok(url) = u.parse::<tauri::Url>() {
            if let Ok(cookies) = win.cookies_for_url(url) {
                for c in cookies {
                    let (n, v) = (c.name().to_string(), c.value().to_string());
                    if !n.is_empty() && !v.is_empty() {
                        map.entry(n).or_insert(v);
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
