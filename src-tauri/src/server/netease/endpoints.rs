//! 网易云业务端点，复刻 server.js 的响应包装格式（前端依赖这些形状）。

use serde_json::{json, Value};

use super::{client::eapi_request, cookie_store};

/// 音质候选（对应 server.js NETEASE_QUALITY_CANDIDATES）。
const QUALITY_CANDIDATES: &[(&str, i64, &str, bool)] = &[
    ("jymaster", 1999000, "超清母带", true),
    ("hires", 1999000, "高清臻音", false),
    ("lossless", 1411000, "无损", false),
    ("exhigh", 999000, "极高", false),
    ("standard", 128000, "标准", false),
];

fn map_artists(raw: Option<&Value>) -> Vec<Value> {
    raw.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        return None;
                    }
                    Some(json!({ "id": a.get("id").cloned().unwrap_or(Value::Null), "name": name }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 复刻 server.js mapSongRecord。
fn map_song_record(s: &Value) -> Value {
    let artists = map_artists(s.get("ar").or_else(|| s.get("artists")));
    let album = s
        .get("al")
        .or_else(|| s.get("album"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let artist_names = artists
        .iter()
        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
        .collect::<Vec<_>>()
        .join(" / ");
    json!({
        "provider": "netease",
        "source": "netease",
        "type": "song",
        "id": s.get("id").cloned().unwrap_or(Value::Null),
        "name": s.get("name").cloned().unwrap_or(Value::Null),
        "artist": artist_names,
        "artistId": artists.first().and_then(|a| a.get("id")).cloned().unwrap_or(Value::Null),
        "artists": artists,
        "album": album.get("name").and_then(|n| n.as_str()).unwrap_or(""),
        "cover": album.get("picUrl").or_else(|| album.get("coverUrl")).and_then(|n| n.as_str()).unwrap_or(""),
        "duration": s.get("dt").or_else(|| s.get("duration")).cloned().unwrap_or(json!(0)),
        "fee": s.get("fee").cloned().unwrap_or(Value::Null),
    })
}

/// /api/search → { songs: [...] }
pub async fn search(client: &reqwest::Client, keywords: &str, limit: i64) -> Result<Value, String> {
    let data = json!({ "s": keywords, "type": 1, "limit": limit, "offset": 0, "total": true });
    let body = eapi_request(client, "/api/cloudsearch/pc", data, cookie_store::music_u().as_deref()).await?;
    let songs = body
        .get("result")
        .and_then(|r| r.get("songs"))
        .and_then(|s| s.as_array())
        .map(|arr| arr.iter().map(map_song_record).collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(json!({ "songs": songs }))
}

/// /api/lyric → { lyric, tlyric, yrc, source }
pub async fn lyric(client: &reqwest::Client, id: &str) -> Result<Value, String> {
    let music_u = cookie_store::music_u();
    // 优先 lyric_new（含逐字 yrc）
    let data = json!({
        "id": id, "cp": false, "tv": 0, "lv": 0, "rv": 0, "kv": 0, "yv": 0, "ytv": 0, "yrv": 0
    });
    let mut source = "lyric_new";
    let mut body = eapi_request(client, "/api/song/lyric/v1", data, music_u.as_deref())
        .await
        .unwrap_or_else(|_| json!({}));

    let has_lyric = body
        .get("lrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|s| s.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !has_lyric {
        let data = json!({ "id": id, "tv": -1, "lv": -1, "rv": -1, "kv": -1, "_nmclfl": 1 });
        body = eapi_request(client, "/api/song/lyric", data, music_u.as_deref()).await?;
        source = "lyric";
    }

    let pick = |key: &str| {
        body.get(key)
            .and_then(|o| o.get("lyric"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string()
    };
    Ok(json!({
        "lyric": pick("lrc"),
        "tlyric": pick("tlyric"),
        "yrc": pick("yrc"),
        "source": source,
    }))
}

/// /api/song/url → 播放信息（含音质回退与试听兜底）。
pub async fn song_url(client: &reqwest::Client, id: &str, quality: &str) -> Result<Value, String> {
    let music_u = cookie_store::music_u();
    let svip_ready = false; // 匿名/普通态；SVIP 判定将在登录态接入
    let requested = normalize_quality(quality);

    let mut trial_fallback: Option<Value> = None;
    let mut last_code = Value::Null;
    let mut last_fee = Value::Null;

    for (level, _br, label, svip) in QUALITY_CANDIDATES {
        if *svip && !svip_ready {
            continue;
        }
        let data = json!({ "ids": format!("[{id}]"), "level": level, "encodeType": "flac" });
        let resp = match eapi_request(client, "/api/song/enhance/player/url/v1", data, music_u.as_deref()).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let d = resp.get("data").and_then(|a| a.as_array()).and_then(|a| a.first());
        let Some(d) = d else { continue };
        last_code = d.get("code").cloned().unwrap_or(Value::Null);
        last_fee = d.get("fee").cloned().unwrap_or(Value::Null);
        let url = d.get("url").and_then(|u| u.as_str()).filter(|u| !u.is_empty());
        let free_trial = d.get("freeTrialInfo").filter(|v| !v.is_null());
        let Some(url) = url else { continue };
        let br = d.get("br").cloned().unwrap_or(Value::Null);
        if free_trial.is_none() {
            return Ok(merge_login(json!({
                "url": url, "trial": false, "playable": true,
                "level": level, "quality": label, "br": br, "requestedQuality": requested,
            })));
        }
        if trial_fallback.is_none() {
            trial_fallback = Some(json!({
                "url": url, "trial": true, "playable": true,
                "level": level, "quality": label, "br": br, "requestedQuality": requested,
                "trialInfo": free_trial.cloned().unwrap_or(Value::Null),
            }));
        }
    }

    if let Some(t) = trial_fallback {
        return Ok(merge_login(t));
    }
    Ok(merge_login(json!({
        "url": Value::Null, "trial": false, "playable": false,
        "reason": "unavailable", "lastCode": last_code, "fee": last_fee,
        "requestedQuality": requested,
    })))
}

/// 把登录态字段合并进 song/url 响应（对应路由里附加的 vip 字段）。
fn merge_login(mut info: Value) -> Value {
    let logged_in = cookie_store::is_logged_in();
    let obj = info.as_object_mut().unwrap();
    obj.insert("loggedIn".into(), json!(logged_in));
    obj.insert("vipType".into(), json!(0));
    obj.insert("vipLevel".into(), json!("none"));
    obj.insert("isVip".into(), json!(false));
    obj.insert("isSvip".into(), json!(false));
    obj.insert("vipLabel".into(), json!("无VIP"));
    info
}

fn normalize_quality(value: &str) -> String {
    let raw = value.to_lowercase();
    let raw = raw.trim();
    match raw {
        "jymaster" | "master" | "studio" | "svip" => "jymaster",
        "hires" | "hi-res" | "highres" | "zhenyin" | "spatial" => "hires",
        "lossless" | "flac" | "sq" => "lossless",
        "exhigh" | "high" | "hq" => "exhigh",
        "standard" | "low" | "" => "standard",
        _ => "standard",
    }
    .to_string()
}
