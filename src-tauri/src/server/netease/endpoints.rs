//! 网易云业务端点，复刻 server.js 的响应包装格式（前端依赖这些形状）。

use serde_json::{json, Value};

use super::{
    client::{eapi_request, request_eapi, weapi_request},
    cookie_store, qr,
};

/// 音质候选（对应 server.js NETEASE_QUALITY_CANDIDATES）。
const QUALITY_CANDIDATES: &[(&str, i64, &str, bool)] = &[
    ("jymaster", 1999000, "超清母带", true),
    ("hires", 1999000, "高清臻音", false),
    ("lossless", 1411000, "无损", false),
    ("exhigh", 999000, "极高", false),
    ("standard", 128000, "标准", false),
];

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn map_artists(raw: Option<&Value>) -> Vec<Value> {
    raw.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = s(a, "name");
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
fn map_song_record(song: &Value) -> Value {
    let artists = map_artists(song.get("ar").or_else(|| song.get("artists")));
    let album = song.get("al").or_else(|| song.get("album")).cloned().unwrap_or_else(|| json!({}));
    let artist_names = artists
        .iter()
        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
        .collect::<Vec<_>>()
        .join(" / ");
    json!({
        "provider": "netease",
        "source": "netease",
        "type": "song",
        "id": song.get("id").cloned().unwrap_or(Value::Null),
        "name": song.get("name").cloned().unwrap_or(Value::Null),
        "artist": artist_names,
        "artistId": artists.first().and_then(|a| a.get("id")).cloned().unwrap_or(Value::Null),
        "artists": artists,
        "album": s(&album, "name"),
        "cover": album.get("picUrl").or_else(|| album.get("coverUrl")).and_then(|n| n.as_str()).unwrap_or(""),
        "duration": song.get("dt").or_else(|| song.get("duration")).cloned().unwrap_or(json!(0)),
        "fee": song.get("fee").cloned().unwrap_or(Value::Null),
    })
}

// ---------------- 搜索 / 歌词 / 歌曲URL ----------------

/// /api/search → { songs: [...] }
pub async fn search(client: &reqwest::Client, keywords: &str, limit: i64) -> Result<Value, String> {
    let data = json!({ "s": keywords, "type": 1, "limit": limit, "offset": 0, "total": true });
    let body = eapi_request(client, "/api/cloudsearch/pc", data).await?;
    let songs = body
        .get("result")
        .and_then(|r| r.get("songs"))
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().map(map_song_record).collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(json!({ "songs": songs }))
}

/// /api/lyric → { lyric, tlyric, yrc, source }
pub async fn lyric(client: &reqwest::Client, id: &str) -> Result<Value, String> {
    let data = json!({
        "id": id, "cp": false, "tv": 0, "lv": 0, "rv": 0, "kv": 0, "yv": 0, "ytv": 0, "yrv": 0
    });
    let mut source = "lyric_new";
    let mut body = eapi_request(client, "/api/song/lyric/v1", data).await.unwrap_or_else(|_| json!({}));

    let has_lyric = body
        .get("lrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|x| x.as_str())
        .map(|x| !x.is_empty())
        .unwrap_or(false);
    if !has_lyric {
        let data = json!({ "id": id, "tv": -1, "lv": -1, "rv": -1, "kv": -1, "_nmclfl": 1 });
        body = eapi_request(client, "/api/song/lyric", data).await?;
        source = "lyric";
    }

    let pick = |key: &str| {
        body.get(key).and_then(|o| o.get("lyric")).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    Ok(json!({ "lyric": pick("lrc"), "tlyric": pick("tlyric"), "yrc": pick("yrc"), "source": source }))
}

/// /api/song/url → 播放信息（音质回退 + 试听兜底 + 登录态 vip 字段）。
pub async fn song_url(client: &reqwest::Client, id: &str, quality: &str) -> Result<Value, String> {
    let info = login_info(client).await;
    let svip_ready = info.get("isSvip").and_then(|v| v.as_bool()).unwrap_or(false);
    let requested = normalize_quality(quality);

    let mut trial_fallback: Option<Value> = None;
    let mut last_code = Value::Null;
    let mut last_fee = Value::Null;

    for (level, _br, label, svip) in QUALITY_CANDIDATES {
        if *svip && !svip_ready {
            continue;
        }
        let data = json!({ "ids": format!("[{id}]"), "level": level, "encodeType": "flac" });
        let resp = match eapi_request(client, "/api/song/enhance/player/url/v1", data).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(d) = resp.get("data").and_then(|a| a.as_array()).and_then(|a| a.first()) else {
            continue;
        };
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
            }), &info));
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
        return Ok(merge_login(t, &info));
    }
    Ok(merge_login(json!({
        "url": Value::Null, "trial": false, "playable": false,
        "reason": "unavailable", "lastCode": last_code, "fee": last_fee, "requestedQuality": requested,
    }), &info))
}

fn merge_login(mut payload: Value, info: &Value) -> Value {
    let obj = payload.as_object_mut().unwrap();
    obj.insert("loggedIn".into(), info.get("loggedIn").cloned().unwrap_or(json!(false)));
    obj.insert("vipType".into(), info.get("vipType").cloned().unwrap_or(json!(0)));
    obj.insert("vipLevel".into(), info.get("vipLevel").cloned().unwrap_or(json!("none")));
    obj.insert("isVip".into(), info.get("isVip").cloned().unwrap_or(json!(false)));
    obj.insert("isSvip".into(), info.get("isSvip").cloned().unwrap_or(json!(false)));
    obj.insert("vipLabel".into(), info.get("vipLabel").cloned().unwrap_or(json!("无VIP")));
    payload
}

fn normalize_quality(value: &str) -> String {
    match value.to_lowercase().trim() {
        "jymaster" | "master" | "studio" | "svip" => "jymaster",
        "hires" | "hi-res" | "highres" | "zhenyin" | "spatial" => "hires",
        "lossless" | "flac" | "sq" => "lossless",
        "exhigh" | "high" | "hq" => "exhigh",
        _ => "standard",
    }
    .to_string()
}

// ---------------- 登录 ----------------

/// /api/login/qr/key → { key }
pub async fn login_qr_key(client: &reqwest::Client) -> Result<Value, String> {
    let body = eapi_request(client, "/api/login/qrcode/unikey", json!({ "type": 3 })).await?;
    Ok(json!({ "key": body.get("unikey").cloned().unwrap_or(Value::Null) }))
}

/// /api/login/qr/create?key= → { img, url }（本地生成二维码）
pub fn login_qr_create(key: &str) -> Value {
    let url = format!("https://music.163.com/login?codekey={key}");
    json!({ "img": qr::data_url(&url).unwrap_or_default(), "url": url })
}

/// /api/login/qr/check?key= → { code, message, ...info }
pub async fn login_qr_check(client: &reqwest::Client, key: &str) -> Result<Value, String> {
    let resp = request_eapi(client, "/api/login/qrcode/client/login", json!({ "key": key, "type": 3 })).await?;
    let code = resp.body.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    let message = s(&resp.body, "message").to_string();
    // 803 = 授权成功
    if code == 803 {
        cookie_store::ingest(resp.cookies);
        let info = login_info(client).await;
        let mut out = json!({ "code": code, "message": message, "hasCookie": true });
        if let (Some(o), Some(i)) = (out.as_object_mut(), info.as_object()) {
            for (k, v) in i {
                o.insert(k.clone(), v.clone());
            }
        }
        return Ok(out);
    }
    Ok(json!({ "code": code, "message": message }))
}

/// /api/login/status → 登录态信息
pub async fn login_status(client: &reqwest::Client) -> Value {
    login_info(client).await
}

/// /api/logout → { ok: true }
pub async fn logout(client: &reqwest::Client) -> Value {
    let _ = eapi_request(client, "/api/logout", json!({})).await;
    cookie_store::clear();
    json!({ "ok": true })
}

/// /api/login/cookie （手动粘贴 cookie 登录）
pub async fn login_cookie(client: &reqwest::Client, raw: &str) -> Value {
    cookie_store::set_from_cookie_string(raw);
    if cookie_store::music_u().is_none() {
        return json!({ "loggedIn": false, "error": "INVALID_NETEASE_COOKIE", "message": "网易云 cookie 缺少 MUSIC_U" });
    }
    let mut info = login_info(client).await;
    if let Some(o) = info.as_object_mut() {
        o.insert("saved".into(), json!(true));
        o.insert("hasCookie".into(), json!(true));
    }
    info
}

/// 登录态信息：weapi /api/w/nuser/account/get → 规整（对应 getLoginInfo + normalizeLoginInfo）。
pub async fn login_info(client: &reqwest::Client) -> Value {
    if !cookie_store::is_logged_in() {
        return logged_out_info();
    }
    match weapi_request(client, "/api/w/nuser/account/get", json!({})).await {
        Ok(body) => {
            let data = body.get("data").unwrap_or(&body);
            let profile = data.get("profile").or_else(|| body.get("profile"));
            let account = data.get("account").or_else(|| body.get("account"));
            normalize_login_info(profile, account, data)
        }
        Err(_) => logged_out_info(),
    }
}

fn logged_out_info() -> Value {
    json!({
        "loggedIn": false, "vipType": 0, "vipLevel": "none",
        "isVip": false, "isSvip": false, "vipLabel": "无VIP"
    })
}

fn normalize_login_info(profile: Option<&Value>, account: Option<&Value>, extra: &Value) -> Value {
    let p = profile.cloned().unwrap_or_else(|| json!({}));
    let a = account.cloned().unwrap_or_else(|| json!({}));
    let user_id = p
        .get("userId")
        .or_else(|| p.get("id"))
        .or_else(|| a.get("userId"))
        .or_else(|| a.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    if user_id.is_null() {
        return logged_out_info();
    }
    let vip = normalize_vip(&p, &a, extra);
    let mut out = json!({
        "loggedIn": true,
        "userId": user_id,
        "nickname": p.get("nickname").and_then(|x| x.as_str()).unwrap_or("网易云用户"),
        "avatar": p.get("avatarUrl").and_then(|x| x.as_str()).unwrap_or(""),
    });
    if let (Some(o), Some(v)) = (out.as_object_mut(), vip.as_object()) {
        for (k, val) in v {
            o.insert(k.clone(), val.clone());
        }
    }
    out
}

fn normalize_vip(profile: &Value, account: &Value, _extra: &Value) -> Value {
    let num = |obj: &Value, k: &str| obj.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let vip_type = num(account, "vipType").max(num(profile, "vipType"));
    let svip_flag = num(account, "svipType") > 0 || vip_type >= 10;
    let is_svip = svip_flag;
    let is_vip = is_svip || vip_type > 0;
    let vip_level = if is_svip { "svip" } else if is_vip { "vip" } else { "none" };
    let vip_label = if is_svip { "SVIP" } else if is_vip { "VIP" } else { "无VIP" };
    json!({
        "vipType": vip_type, "vipLevel": vip_level,
        "isVip": is_vip, "isSvip": is_svip, "vipLabel": vip_label
    })
}

// ---------------- 用户歌单 / 首页 ----------------

/// /api/user/playlists → { loggedIn, userId, playlists }
pub async fn user_playlists(client: &reqwest::Client, limit: i64) -> Value {
    let info = login_info(client).await;
    let logged_in = info.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
    let user_id = info.get("userId").cloned().unwrap_or(Value::Null);
    if !logged_in || user_id.is_null() {
        return json!({ "loggedIn": false, "playlists": [] });
    }
    let data = json!({ "uid": user_id, "limit": limit, "offset": 0 });
    match weapi_request(client, "/api/user/playlist", data).await {
        Ok(body) => {
            let list = body
                .get("playlist")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|pl| {
                            json!({
                                "id": pl.get("id").cloned().unwrap_or(Value::Null),
                                "name": s(pl, "name"),
                                "cover": s(pl, "coverImgUrl"),
                                "trackCount": pl.get("trackCount").cloned().unwrap_or(json!(0)),
                                "playCount": pl.get("playCount").cloned().unwrap_or(json!(0)),
                                "creator": pl.get("creator").map(|c| s(c, "nickname")).unwrap_or(""),
                                "subscribed": pl.get("subscribed").cloned().unwrap_or(json!(false)),
                                "specialType": pl.get("specialType").cloned().unwrap_or(json!(0)),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({ "loggedIn": true, "userId": user_id, "playlists": list })
        }
        Err(e) => json!({ "error": e, "loggedIn": true, "playlists": [] }),
    }
}

/// /api/discover/home → 首页（首版：登录后给每日推荐+推荐歌单，未登录给 starter）。
pub async fn discover_home(client: &reqwest::Client) -> Value {
    let info = login_info(client).await;
    let logged_in = info.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
    if !logged_in {
        return json!({
            "loggedIn": false, "user": Value::Null,
            "dailySongs": [], "playlists": [], "podcasts": [],
            "mode": "starter"
        });
    }

    let playlists = weapi_request(client, "/api/personalized/playlist", json!({ "limit": 8 }))
        .await
        .ok()
        .and_then(|b| b.get("result").cloned())
        .and_then(|r| r.as_array().cloned())
        .map(|arr| arr.iter().map(map_discover_playlist).collect::<Vec<_>>())
        .unwrap_or_default();

    let daily_songs = weapi_request(client, "/api/v3/discovery/recommend/songs", json!({}))
        .await
        .ok()
        .and_then(|b| b.get("data").and_then(|d| d.get("dailySongs")).cloned())
        .and_then(|x| x.as_array().cloned())
        .map(|arr| arr.iter().map(map_song_record).collect::<Vec<_>>())
        .unwrap_or_default();

    json!({
        "loggedIn": true,
        "user": info,
        "dailySongs": daily_songs,
        "playlists": playlists,
        "podcasts": [],
        "mode": "home"
    })
}

/// /api/playlist/tracks → { playlist: {...}, tracks: [...] }
pub async fn playlist_tracks(client: &reqwest::Client, id: &str) -> Result<Value, String> {
    let detail = eapi_request(client, "/api/v6/playlist/detail", json!({ "id": id, "n": 100000, "s": 8 })).await?;
    let pl = detail.get("playlist").cloned().unwrap_or_else(|| json!({}));
    let meta = json!({
        "id": pl.get("id").cloned().unwrap_or_else(|| json!(id)),
        "name": s(&pl, "name"),
        "cover": s(&pl, "coverImgUrl"),
        "trackCount": pl.get("trackCount").cloned().unwrap_or(json!(0)),
    });

    // 先尝试 trackIds → 批量 song/detail（拿全量、字段更全）
    let track_ids: Vec<i64> = pl
        .get("trackIds")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(|t| t.get("id").and_then(|i| i.as_i64())).take(500).collect())
        .unwrap_or_default();

    let tracks: Vec<Value> = if !track_ids.is_empty() {
        let c = format!(
            "[{}]",
            track_ids.iter().map(|i| format!("{{\"id\":{i}}}")).collect::<Vec<_>>().join(",")
        );
        let sd = eapi_request(client, "/api/v3/song/detail", json!({ "c": c })).await?;
        sd.get("songs")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().map(map_song_record).filter(|t| !t.get("id").map(|i| i.is_null()).unwrap_or(true)).collect())
            .unwrap_or_default()
    } else {
        pl.get("tracks")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().map(map_song_record).collect())
            .unwrap_or_default()
    };

    let mut meta = meta;
    if meta.get("trackCount").and_then(|c| c.as_i64()).unwrap_or(0) == 0 {
        meta["trackCount"] = json!(tracks.len());
    }
    Ok(json!({ "playlist": meta, "tracks": tracks }))
}

fn map_discover_playlist(pl: &Value) -> Value {
    json!({
        "provider": "netease",
        "source": "netease",
        "type": "playlist",
        "id": pl.get("id").or_else(|| pl.get("resourceId")).cloned().unwrap_or(Value::Null),
        "name": pl.get("name").and_then(|x| x.as_str()).or_else(|| pl.get("title").and_then(|x| x.as_str())).unwrap_or(""),
        "cover": pl.get("picUrl").or_else(|| pl.get("coverImgUrl")).and_then(|x| x.as_str()).unwrap_or(""),
        "trackCount": pl.get("trackCount").or_else(|| pl.get("songCount")).cloned().unwrap_or(json!(0)),
        "playCount": pl.get("playCount").or_else(|| pl.get("playcount")).cloned().unwrap_or(json!(0)),
        "creator": pl.get("creator").map(|c| s(c, "nickname")).unwrap_or(""),
        "tag": s(pl, "alg"),
    })
}
