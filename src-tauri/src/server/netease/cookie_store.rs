//! 网易云登录态（完整 cookie）的进程内存储 + 文件持久化。
//! 替换 server.js 的 userCookie / saveCookie / .cookie 读写。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

static COOKIES: RwLock<BTreeMap<String, String>> = RwLock::new(BTreeMap::new());
static FILE: OnceLock<PathBuf> = OnceLock::new();

/// 初始化：设定持久化文件路径并加载已有 cookie。
pub fn init(path: PathBuf) {
    if FILE.set(path.clone()).is_err() {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(&path) {
        let parsed = parse_cookie_string(&text);
        if !parsed.is_empty() {
            *COOKIES.write().unwrap() = parsed;
        }
    }
}

/// 解析 "k=v; k=v" 形式为 map（仅取每段第一个等号前后的键值）。
fn parse_cookie_string(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for part in text.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            // 跳过 cookie 属性
            if matches!(
                k.to_ascii_lowercase().as_str(),
                "domain" | "path" | "expires" | "max-age" | "samesite" | "secure" | "httponly"
            ) {
                continue;
            }
            if !k.is_empty() {
                map.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    map
}

fn persist() {
    if let Some(path) = FILE.get() {
        let _ = std::fs::write(path, full());
    }
}

/// 合并多条 Set-Cookie 的 "k=v"（已去属性）到存储并持久化。
pub fn ingest(pairs: Vec<String>) {
    {
        let mut map = COOKIES.write().unwrap();
        for p in pairs {
            if let Some((k, v)) = p.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    persist();
}

/// 用完整 cookie 串覆盖式合并（用于手动粘贴 cookie 登录）。
pub fn set_from_cookie_string(text: &str) {
    let parsed = parse_cookie_string(text);
    {
        let mut map = COOKIES.write().unwrap();
        for (k, v) in parsed {
            map.insert(k, v);
        }
    }
    persist();
}

/// 清空登录态（登出）。
pub fn clear() {
    COOKIES.write().unwrap().clear();
    persist();
}

/// 渲染完整 cookie 头 "k=v; k=v"。
pub fn full() -> String {
    let map = COOKIES.read().unwrap();
    map.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn music_u() -> Option<String> {
    let map = COOKIES.read().unwrap();
    map.get("MUSIC_U").filter(|v| !v.is_empty()).cloned()
}

pub fn csrf() -> String {
    let map = COOKIES.read().unwrap();
    map.get("__csrf").cloned().unwrap_or_default()
}

pub fn is_logged_in() -> bool {
    music_u().is_some()
}
