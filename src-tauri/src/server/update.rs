//! 更新检查（替换 server.js 的 /api/update/latest）。
//! 查 GitHub Releases 比较版本。下载/安装为 Windows 安装包专有流程，
//! 留给打包阶段(Tauri updater)；当前 download/patch 走 404 兜底。

use std::time::Duration;

use serde_json::{json, Value};

const OWNER: &str = "XxHuberrr";
const REPO: &str = "Mineradio";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn normalize_version(v: &str) -> String {
    let v = v.trim().trim_start_matches(['v', 'V']);
    // 去掉 +build / -pre 后缀
    let v = v.split('+').next().unwrap_or(v);
    let v = v.split('-').next().unwrap_or(v);
    v.to_string()
}

/// >0 表示 a 比 b 新。
fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |s: &str| -> Vec<i64> {
        normalize_version(s).split('.').map(|n| n.parse::<i64>().unwrap_or(0)).collect()
    };
    let aa = parse(a);
    let bb = parse(b);
    let len = aa.len().max(bb.len()).max(3);
    for i in 0..len {
        let l = aa.get(i).copied().unwrap_or(0);
        let r = bb.get(i).copied().unwrap_or(0);
        if l > r {
            return 1;
        }
        if l < r {
            return -1;
        }
    }
    0
}

fn pick_release_asset(assets: &[Value]) -> Option<Value> {
    let by_ext = |re: &[&str]| {
        assets.iter().find(|a| {
            let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
            re.iter().any(|ext| name.ends_with(ext))
        })
    };
    let preferred = by_ext(&[".exe", ".msi"]).or_else(|| by_ext(&[".zip", ".7z"])).or_else(|| assets.first())?;
    Some(json!({
        "name": preferred.get("name").cloned().unwrap_or(Value::Null),
        "size": preferred.get("size").cloned().unwrap_or(json!(0)),
        "contentType": preferred.get("content_type").cloned().unwrap_or(Value::Null),
        "downloadUrl": preferred.get("browser_download_url").cloned().unwrap_or(Value::Null),
    }))
}

fn clean_release_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(['-', '*', '#', '>', ' '])
        .trim()
        .to_string()
}

fn extract_release_notes(body: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for line in body.lines() {
        let text = clean_release_line(line);
        if text.is_empty() {
            continue;
        }
        let low = text.to_lowercase();
        if matches!(low.as_str(), "what's changed" | "whats changed" | "changes" | "changelog" | "full changelog" | "更新日志") {
            continue;
        }
        if low.starts_with("http://") || low.starts_with("https://") {
            continue;
        }
        if text.chars().count() > 72 {
            continue;
        }
        notes.push(text);
        if notes.len() >= 4 {
            break;
        }
    }
    notes
}

const FALLBACK_NOTES: &[&str] = &["电影镜头节奏更松", "音源失败自动换源", "右上角更新提示"];

fn local_fallback(reason: &str, configured: bool) -> Value {
    json!({
        "configured": configured,
        "preview": true,
        "updateAvailable": false,
        "currentVersion": APP_VERSION,
        "latestVersion": APP_VERSION,
        "release": {
            "tagName": format!("v{APP_VERSION}"),
            "name": format!("Mineradio v{APP_VERSION}"),
            "version": APP_VERSION,
            "htmlUrl": "",
            "downloadUrl": "",
            "summary": "当前版本，更新检测已就绪。",
            "notes": FALLBACK_NOTES,
        },
        "reason": reason,
    })
}

/// /api/update/latest
pub async fn latest(client: &reqwest::Client) -> Value {
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, format!("Mineradio/{APP_VERSION}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_millis(8500))
        .send()
        .await;

    let data = match resp {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => v,
            Err(e) => return local_fallback(&e.to_string(), true),
        },
        Ok(r) => return local_fallback(&format!("GitHub Releases {}", r.status().as_u16()), true),
        Err(e) => return local_fallback(&e.to_string(), true),
    };

    let tag = data.get("tag_name").or_else(|| data.get("name")).and_then(|x| x.as_str()).unwrap_or(APP_VERSION);
    let latest_version = {
        let n = normalize_version(tag);
        if n.is_empty() { APP_VERSION.to_string() } else { n }
    };
    let assets = data.get("assets").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let asset = pick_release_asset(&assets);
    let body = data.get("body").and_then(|x| x.as_str()).unwrap_or("");
    let notes = {
        let n = extract_release_notes(body);
        if n.is_empty() { FALLBACK_NOTES.iter().map(|s| s.to_string()).collect() } else { n }
    };
    let update_available = compare_versions(&latest_version, APP_VERSION) > 0;
    let summary = notes.first().cloned().unwrap_or_else(|| "发现新版本，建议更新。".to_string());
    let download_url = asset.as_ref().and_then(|a| a.get("downloadUrl").and_then(|x| x.as_str())).unwrap_or("").to_string();

    json!({
        "configured": true,
        "preview": false,
        "updateAvailable": update_available,
        "currentVersion": APP_VERSION,
        "latestVersion": latest_version,
        "release": {
            "tagName": data.get("tag_name").cloned().unwrap_or_else(|| json!(format!("v{latest_version}"))),
            "name": data.get("name").cloned().unwrap_or_else(|| json!(format!("Mineradio v{latest_version}"))),
            "version": latest_version,
            "publishedAt": data.get("published_at").cloned().unwrap_or(json!("")),
            "htmlUrl": data.get("html_url").cloned().unwrap_or(json!("")),
            "downloadUrl": download_url,
            "asset": asset,
            "summary": summary,
            "notes": notes,
        },
    })
}
