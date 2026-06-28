//! QQ 音乐（替换 server.js 自实现的 QQ 直连）。
//! musicu.fcg + smartbox 搜索 + vkey 取播放地址 + 歌词(base64) + cookie 登录。

use std::sync::{OnceLock, RwLock};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

const MUSICU_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const SMARTBOX_URL: &str = "https://c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg";
const REFERER: &str = "https://y.qq.com/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// 音质候选模板（对应 QQ_QUALITY_CANDIDATE_TEMPLATES）。
const QQ_QUALITY: &[(&str, &str, &str, &str)] = &[
    ("RS01", ".flac", "hires", "Hi-Res FLAC"),
    ("F000", ".flac", "lossless", "无损 FLAC"),
    ("M800", ".mp3", "exhigh", "320k MP3"),
    ("M500", ".mp3", "standard", "128k MP3"),
    ("C400", ".m4a", "aac", "AAC/M4A"),
];

// ---- QQ cookie 存储（独立于网易云）----
static QQ_COOKIE: RwLock<String> = RwLock::new(String::new());
static FILE: OnceLock<std::path::PathBuf> = OnceLock::new();

pub fn init(path: std::path::PathBuf) {
    if FILE.set(path.clone()).is_err() {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(&path) {
        *QQ_COOKIE.write().unwrap() = text.trim().to_string();
    }
}

fn save_cookie(c: &str) {
    *QQ_COOKIE.write().unwrap() = c.trim().to_string();
    if let Some(p) = FILE.get() {
        let _ = std::fs::write(p, c.trim());
    }
}

fn cookie() -> String {
    QQ_COOKIE.read().unwrap().clone()
}

fn cookie_val(key: &str) -> String {
    let c = cookie();
    for part in c.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            if k.trim() == key {
                return v.trim().to_string();
            }
        }
    }
    String::new()
}

fn uin() -> String {
    let raw = if cookie_val("login_type") == "2" {
        first_nonempty(&["wxuin", "uin", "p_uin"])
    } else {
        first_nonempty(&["uin", "qqmusic_uin", "wxuin", "p_uin"])
    };
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.trim_start_matches('0').to_string()
}

fn music_key() -> String {
    first_nonempty(&[
        "qm_keyst", "qqmusic_key", "music_key", "p_skey", "skey",
        "psrf_qqaccess_token", "psrf_qqrefresh_token", "wxrefresh_token", "wxskey",
    ])
}

fn first_nonempty(keys: &[&str]) -> String {
    for k in keys {
        let v = cookie_val(k);
        if !v.is_empty() {
            return v;
        }
    }
    String::new()
}

// ---- 请求 ----
fn parse_callback(text: &str) -> Value {
    let raw = text.trim();
    // 去掉 callback(...) 包裹
    let inner = raw
        .strip_prefix("callback(")
        .and_then(|s| s.strip_suffix(");").or_else(|| s.strip_suffix(")")))
        .unwrap_or(raw);
    serde_json::from_str(inner).unwrap_or(Value::Null)
}

async fn musicu(client: &reqwest::Client, payload: Value, with_cookie: bool) -> Result<Value, String> {
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let mut req = client
        .post(MUSICU_URL)
        .header(reqwest::header::REFERER, REFERER)
        .header(reqwest::header::USER_AGENT, UA)
        .header(reqwest::header::CONTENT_TYPE, "application/json;charset=UTF-8")
        .body(body);
    if with_cookie {
        let c = cookie();
        if !c.is_empty() {
            req = req.header(reqwest::header::COOKIE, c);
        }
    }
    let text = req.send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    Ok(parse_callback(&text))
}

async fn fcg_get(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let text = client
        .get(url)
        .header(reqwest::header::REFERER, REFERER)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    Ok(parse_callback(&text))
}

// ---- 映射 ----
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn album_cover(album_mid: &str) -> String {
    if album_mid.is_empty() {
        String::new()
    } else {
        format!("https://y.qq.com/music/photo_new/T002R300x300M000{album_mid}.jpg?max_age=2592000")
    }
}

fn map_smart_song(item: &Value) -> Value {
    let mid = item.get("mid").or_else(|| item.get("songmid")).or_else(|| item.get("id")).and_then(|x| x.as_str()).unwrap_or("");
    json!({
        "provider": "qq", "source": "qq", "type": "qq",
        "id": mid, "qqId": item.get("id").cloned().unwrap_or(Value::Null), "mid": mid, "songmid": mid,
        "name": if !s(item, "name").is_empty() { s(item, "name") } else { s(item, "title") },
        "artist": s(item, "singer"),
        "artists": if s(item, "singer").is_empty() { json!([]) } else { json!([{ "name": s(item, "singer") }]) },
        "album": "", "cover": "", "duration": 0, "fee": 0, "playable": false,
    })
}

fn map_artists(raw: Option<&Value>) -> Vec<Value> {
    raw.and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = a.get("name").or_else(|| a.get("title")).and_then(|x| x.as_str()).unwrap_or("");
                    if name.is_empty() {
                        return None;
                    }
                    Some(json!({ "id": a.get("id").cloned().unwrap_or(Value::Null), "mid": a.get("mid").cloned().unwrap_or(Value::Null), "name": name }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn map_track(track: &Value, fallback: &Value) -> Value {
    let album = track.get("album").cloned().unwrap_or_else(|| json!({}));
    let artists = map_artists(track.get("singer"));
    let mid = {
        let m = s(track, "mid");
        if m.is_empty() { s(fallback, "mid").to_string() } else { m.to_string() }
    };
    let album_mid = album.get("mid").or_else(|| album.get("pmid")).and_then(|x| x.as_str()).unwrap_or("");
    let artist_names = artists.iter().filter_map(|a| a.get("name").and_then(|n| n.as_str())).collect::<Vec<_>>().join(" / ");
    let name = if !s(track, "name").is_empty() { s(track, "name").to_string() } else { s(fallback, "name").to_string() };
    let artist = if !artist_names.is_empty() { artist_names } else { s(fallback, "artist").to_string() };
    let artists_v = if artists.is_empty() { fallback.get("artists").cloned().unwrap_or(json!([])) } else { json!(artists) };
    let album_name = if !s(&album, "name").is_empty() { s(&album, "name").to_string() } else { s(fallback, "album").to_string() };
    let cover = { let c = album_cover(album_mid); if c.is_empty() { s(fallback, "cover").to_string() } else { c } };
    let duration = track.get("interval").and_then(|x| x.as_i64()).unwrap_or(0) * 1000;
    let fee = if track.get("pay").and_then(|p| p.get("pay_play")).and_then(|x| x.as_i64()).unwrap_or(0) != 0 { 1 } else { 0 };
    json!({
        "provider": "qq", "source": "qq", "type": "qq",
        "id": mid, "mid": mid, "songmid": mid,
        "qqId": track.get("id").or_else(|| fallback.get("qqId")).cloned().unwrap_or(Value::Null),
        "mediaMid": track.get("file").and_then(|f| f.get("media_mid")).cloned().unwrap_or(Value::Null),
        "name": name, "artist": artist, "artists": artists_v,
        "album": album_name, "albumMid": album_mid, "cover": cover,
        "duration": duration, "fee": fee, "playable": false,
    })
}

// ---- 业务 ----

/// /api/qq/search → { provider, songs }
pub async fn search(client: &reqwest::Client, keywords: &str, limit: i64) -> Value {
    if keywords.is_empty() {
        return json!({ "provider": "qq", "songs": [] });
    }
    let url = format!(
        "{SMARTBOX_URL}?format=json&key={}&g_tk=5381&loginUin=0&hostUin=0&inCharset=utf8&outCharset=utf-8&notice=0&platform=yqq.json&needNewCode=0",
        urlencoding(keywords)
    );
    let base: Vec<Value> = fcg_get(client, &url)
        .await
        .ok()
        .and_then(|j| j.get("data").and_then(|d| d.get("song")).and_then(|s| s.get("itemlist")).and_then(|x| x.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .take(limit.clamp(1, 10) as usize)
        .map(map_smart_song)
        .collect();

    // 并发取详情补全封面/专辑/时长
    let detailed = futures_util::future::join_all(base.iter().map(|item| {
        let mid = s(item, "mid").to_string();
        let fb = item.clone();
        async move {
            if mid.is_empty() {
                return fb;
            }
            song_detail(client, &mid, &fb).await
        }
    }))
    .await;

    let mut seen = std::collections::HashSet::new();
    let songs: Vec<Value> = detailed
        .into_iter()
        .filter(|sng| {
            let key = s(sng, "mid").to_string();
            !key.is_empty() && !s(sng, "name").is_empty() && seen.insert(key)
        })
        .collect();
    json!({ "provider": "qq", "songs": songs })
}

async fn song_detail(client: &reqwest::Client, mid: &str, fallback: &Value) -> Value {
    let payload = json!({
        "comm": { "ct": 24, "cv": 0 },
        "songinfo": { "module": "music.pf_song_detail_svr", "method": "get_song_detail_yqq", "param": { "song_mid": mid } }
    });
    match musicu(client, payload, false).await {
        Ok(j) => {
            let track = j.get("songinfo").and_then(|s| s.get("data")).and_then(|d| d.get("track_info"));
            match track {
                Some(t) => map_track(t, fallback),
                None => fallback.clone(),
            }
        }
        Err(_) => fallback.clone(),
    }
}

/// /api/qq/song/url → 播放地址（vkey）
pub async fn song_url(client: &reqwest::Client, mid: &str, media_mid: &str, _quality: &str) -> Value {
    if mid.is_empty() {
        return json!({ "provider": "qq", "url": "", "playable": false, "error": "MISSING_MID" });
    }
    let guid = "1234567890";
    let uin_v = { let u = uin(); if u.is_empty() { "0".to_string() } else { u } };
    let key = music_key();

    let mut media_ids: Vec<String> = Vec::new();
    if !media_mid.is_empty() {
        media_ids.push(media_mid.to_string());
    }
    if !media_ids.contains(&mid.to_string()) {
        media_ids.push(mid.to_string());
    }
    let mut filenames: Vec<String> = Vec::new();
    let mut metas: Vec<(String, &str, &str)> = Vec::new(); // (filename, level, label)
    for media_id in &media_ids {
        for (prefix, ext, level, label) in QQ_QUALITY {
            let fname = format!("{prefix}{media_id}{ext}");
            filenames.push(fname.clone());
            metas.push((fname, level, label));
        }
    }

    let n = filenames.len();
    let mut param = json!({
        "guid": guid,
        "songmid": vec![mid; n],
        "songtype": vec![0; n],
        "uin": uin_v,
        "loginflag": 1,
        "platform": "20",
        "filename": filenames,
    });
    if n == 0 {
        param = json!({ "guid": guid, "songmid": [mid], "songtype": [0], "uin": uin_v, "loginflag": 1, "platform": "20" });
    }
    let mut comm = json!({ "uin": uin_v, "format": "json", "ct": if key.is_empty() { 24 } else { 19 }, "cv": 0 });
    if !key.is_empty() {
        comm["authst"] = json!(key);
    }
    let payload = json!({ "comm": comm, "req_0": { "module": "vkey.GetVkeyServer", "method": "CgiGetVkey", "param": param } });

    let data = match musicu(client, payload, true).await {
        Ok(j) => j.get("req_0").and_then(|r| r.get("data")).cloned().unwrap_or(Value::Null),
        Err(e) => return json!({ "provider": "qq", "url": "", "playable": false, "error": e }),
    };
    let infos = data.get("midurlinfo").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let info = infos.iter().find(|i| i.get("purl").and_then(|p| p.as_str()).map(|p| !p.is_empty()).unwrap_or(false)).or_else(|| infos.first());
    if let Some(info) = info {
        let purl = s(info, "purl");
        if !purl.is_empty() {
            let sip = data.get("sip").and_then(|x| x.as_array()).and_then(|a| a.first()).and_then(|x| x.as_str()).unwrap_or("https://ws.stream.qqmusic.qq.com/");
            let fname = s(info, "filename");
            let meta = metas.iter().find(|(f, _, _)| f == fname);
            return json!({
                "provider": "qq", "url": format!("{sip}{purl}"),
                "trial": false, "playable": true,
                "level": meta.map(|m| m.1).unwrap_or(fname),
                "quality": meta.map(|m| m.2).unwrap_or(fname),
                "filename": fname,
            });
        }
    }
    json!({ "provider": "qq", "url": "", "playable": false, "error": "QQ_URL_UNAVAILABLE", "loggedIn": !uin().is_empty() && !key.is_empty() })
}

fn decode_lyric(text: &str) -> String {
    let raw = text.trim();
    if raw.is_empty() {
        return String::new();
    }
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let looks_b64 = compact.len() >= 8 && compact.len() % 4 == 0 && compact.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=');
    if looks_b64 && !raw.starts_with('[') {
        if let Ok(bytes) = STANDARD.decode(&compact) {
            if let Ok(decoded) = String::from_utf8(bytes) {
                let decoded = decoded.trim_start_matches('\u{feff}');
                if decoded.contains('[') || decoded.chars().any(|c| ('\u{4e00}'..='\u{9fa5}').contains(&c)) {
                    return decoded.replace("\r\n", "\n").trim().to_string();
                }
            }
        }
    }
    raw.replace("\r\n", "\n").trim().to_string()
}

/// /api/qq/lyric → { provider, lyric, tlyric }
pub async fn lyric(client: &reqwest::Client, mid: &str, _id: &str) -> Value {
    let payload = json!({
        "comm": { "ct": 24, "cv": 0 },
        "lyric": { "module": "music.musichallSong.PlayLyricInfo", "method": "GetPlayLyricInfo", "param": { "songMID": mid } }
    });
    let mut lyric_text = String::new();
    let mut trans_text = String::new();
    if let Ok(j) = musicu(client, payload, true).await {
        if let Some(data) = j.get("lyric").and_then(|l| l.get("data")) {
            lyric_text = decode_lyric(s(data, "lyric"));
            trans_text = decode_lyric(s(data, "trans"));
        }
    }
    if lyric_text.is_empty() && !mid.is_empty() {
        let url = format!(
            "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg?songmid={mid}&songtype=0&format=json&nobase64=1&g_tk=5381&loginUin=0&hostUin=0&inCharset=utf8&outCharset=utf-8&notice=0&platform=yqq.json&needNewCode=0"
        );
        if let Ok(b) = fcg_get(client, &url).await {
            lyric_text = decode_lyric(s(&b, "lyric"));
            if trans_text.is_empty() {
                trans_text = decode_lyric(s(&b, "trans"));
            }
        }
    }
    json!({ "provider": "qq", "lyric": lyric_text, "tlyric": trans_text, "source": "qq" })
}

/// /api/qq/login/status
pub fn login_status() -> Value {
    let logged_in = !uin().is_empty() && !music_key().is_empty();
    json!({ "provider": "qq", "loggedIn": logged_in, "uin": uin() })
}

/// /api/qq/login/cookie （手动粘贴 cookie）
pub fn login_cookie(raw: &str) -> Value {
    save_cookie(raw);
    if uin().is_empty() || music_key().is_empty() {
        save_cookie("");
        return json!({ "provider": "qq", "loggedIn": false, "error": "INVALID_QQ_COOKIE", "message": "QQ cookie 缺少 uin 或有效登录票据" });
    }
    let mut info = login_status();
    info["saved"] = json!(true);
    info
}

/// /api/qq/logout
pub fn logout() -> Value {
    save_cookie("");
    json!({ "provider": "qq", "ok": true, "loggedIn": false })
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
