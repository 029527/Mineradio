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

    let podcasts = podcast_hot(client, 6, 0)
        .await
        .ok()
        .and_then(|b| b.get("podcasts").cloned())
        .unwrap_or_else(|| json!([]));

    json!({
        "loggedIn": true,
        "user": info,
        "dailySongs": daily_songs,
        "playlists": playlists,
        "podcasts": podcasts,
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

// ---------------- 收藏 / 歌手 / 评论 / 歌单操作 ----------------

/// /api/song/like/check?ids= → { loggedIn, ids, liked: {id: bool} }
pub async fn song_like_check(client: &reqwest::Client, ids: Vec<i64>) -> Value {
    let info = login_info(client).await;
    let logged_in = info.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
    let user_id = info.get("userId").cloned().unwrap_or(Value::Null);
    if !logged_in {
        return json!({ "loggedIn": false, "ids": ids, "liked": {} });
    }
    // 取用户全部红心 id 做成员判断（likelist，稳定）。
    let liked_ids: Vec<i64> = weapi_request(client, "/api/song/like/get", json!({ "uid": user_id }))
        .await
        .ok()
        .and_then(|b| b.get("ids").and_then(|x| x.as_array()).cloned())
        .map(|arr| arr.iter().filter_map(|i| i.as_i64()).collect())
        .unwrap_or_default();
    let set: std::collections::HashSet<i64> = liked_ids.into_iter().collect();
    let mut liked = serde_json::Map::new();
    for id in &ids {
        liked.insert(id.to_string(), json!(set.contains(id)));
    }
    json!({ "loggedIn": true, "ids": ids, "liked": liked })
}

/// /api/song/like?id=&like= → 红心/取消
pub async fn song_like(client: &reqwest::Client, id: &str, like: bool) -> Value {
    if !cookie_store::is_logged_in() {
        return json!({ "loggedIn": false, "error": "LOGIN_REQUIRED" });
    }
    let data = json!({ "alg": "itembased", "trackId": id, "like": like.to_string(), "time": "3" });
    match weapi_request(client, "/api/radio/like", data).await {
        Ok(body) => {
            let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(200);
            json!({ "loggedIn": true, "id": id, "liked": like, "code": code, "body": body })
        }
        Err(e) => json!({ "loggedIn": true, "id": id, "error": e }),
    }
}

/// /api/song/comments?id= → { id, total, comments, hot }
pub async fn song_comments(client: &reqwest::Client, id: &str, limit: i64, offset: i64) -> Result<Value, String> {
    let uri = format!("/api/v1/resource/comments/R_SO_4_{id}");
    let data = json!({ "rid": id, "limit": limit, "offset": offset, "beforeTime": 0 });
    let body = weapi_request(client, &uri, data).await?;
    let hot = body.get("hotComments").and_then(|x| x.as_array()).is_some() && offset == 0;
    let raw = if hot {
        body.get("hotComments")
    } else {
        body.get("comments")
    };
    let comments = raw
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let content = s(c, "content");
                    if content.is_empty() {
                        return None;
                    }
                    Some(json!({
                        "id": c.get("commentId").cloned().unwrap_or(Value::Null),
                        "content": content,
                        "likedCount": c.get("likedCount").cloned().unwrap_or(json!(0)),
                        "time": c.get("time").cloned().unwrap_or(json!(0)),
                        "user": c.get("user").map(|u| json!({
                            "id": u.get("userId").cloned().unwrap_or(Value::Null),
                            "nickname": s(u, "nickname"),
                            "avatar": s(u, "avatarUrl"),
                        })),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(json!({ "id": id, "total": body.get("total").cloned().unwrap_or(json!(0)), "comments": comments, "hot": hot }))
}

/// /api/artist/detail?id= → { id, artist, songs }
pub async fn artist_detail(client: &reqwest::Client, id: &str, limit: i64) -> Result<Value, String> {
    let detail = eapi_request(client, "/api/artist/head/info/get", json!({ "id": id }))
        .await
        .unwrap_or_else(|_| json!({}));

    let mut raw_songs = eapi_request(
        client,
        "/api/v1/artist/songs",
        json!({ "id": id, "private_cloud": "true", "work_type": 1, "order": "hot", "offset": 0, "limit": limit }),
    )
    .await
    .ok()
    .and_then(|b| b.get("songs").and_then(|x| x.as_array()).cloned())
    .unwrap_or_default();

    if raw_songs.is_empty() {
        raw_songs = weapi_request(client, "/api/artist/top/song", json!({ "id": id }))
            .await
            .ok()
            .and_then(|b| b.get("songs").and_then(|x| x.as_array()).cloned())
            .unwrap_or_default();
    }

    let songs: Vec<Value> = raw_songs.iter().map(map_song_record).take(limit as usize).collect();
    let a = detail
        .get("data")
        .and_then(|d| d.get("artist"))
        .or_else(|| detail.get("artist"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "id": id,
        "artist": {
            "id": a.get("id").cloned().unwrap_or_else(|| json!(id)),
            "name": a.get("name").and_then(|x| x.as_str()).or_else(|| a.get("artistName").and_then(|x| x.as_str())).unwrap_or(""),
            "avatar": a.get("avatar").or_else(|| a.get("cover")).or_else(|| a.get("picUrl")).or_else(|| a.get("img1v1Url")).and_then(|x| x.as_str()).unwrap_or(""),
            "brief": a.get("briefDesc").or_else(|| a.get("description")).and_then(|x| x.as_str()).unwrap_or(""),
            "musicSize": a.get("musicSize").or_else(|| a.get("songSize")).cloned().unwrap_or(json!(0)),
            "albumSize": a.get("albumSize").cloned().unwrap_or(json!(0)),
        },
        "songs": songs,
    }))
}

/// /api/playlist/create?name= → 新建歌单
pub async fn playlist_create(client: &reqwest::Client, name: &str, privacy: &str) -> Value {
    if !cookie_store::is_logged_in() {
        return json!({ "loggedIn": false, "error": "LOGIN_REQUIRED" });
    }
    let data = json!({ "name": name, "privacy": privacy, "type": "NORMAL" });
    match weapi_request(client, "/api/playlist/create", data).await {
        Ok(body) => json!({ "loggedIn": true, "playlist": body.get("playlist").or_else(|| body.get("id")).cloned().unwrap_or(Value::Null), "body": body }),
        Err(e) => json!({ "loggedIn": true, "error": e }),
    }
}

/// /api/playlist/add-song (POST {pid,id}) → 收藏歌曲到歌单
pub async fn playlist_add_song(client: &reqwest::Client, pid: &str, id: &str) -> Value {
    if !cookie_store::is_logged_in() {
        return json!({ "loggedIn": false, "error": "LOGIN_REQUIRED" });
    }
    let track_ids = format!("[\"{id}\"]");
    // 优先 manipulate/tracks(eapi)，失败回退 track/add(weapi)
    let primary = eapi_request(
        client,
        "/api/playlist/manipulate/tracks",
        json!({ "op": "add", "pid": pid, "trackIds": track_ids, "imme": "true" }),
    )
    .await;
    let ok = primary
        .as_ref()
        .ok()
        .and_then(|b| b.get("code").and_then(|c| c.as_i64()))
        .map(|c| c == 200)
        .unwrap_or(false);
    if ok {
        return json!({ "loggedIn": true, "pid": pid, "id": id, "success": true, "code": 200 });
    }
    let fallback = weapi_request(client, "/api/playlist/track/add", json!({ "pid": pid, "ids": id })).await;
    let code = fallback.as_ref().ok().and_then(|b| b.get("code").and_then(|c| c.as_i64())).unwrap_or(0);
    json!({
        "loggedIn": true, "pid": pid, "id": id,
        "success": code == 200, "code": code,
        "body": fallback.unwrap_or(Value::Null),
    })
}

// ---------------- 播客 DJ ----------------

fn first_str<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return s;
            }
        }
    }
    ""
}

fn first_num(v: &Value, keys: &[&str]) -> Value {
    for k in keys {
        if let Some(n) = v.get(*k) {
            if n.is_number() {
                return n.clone();
            }
        }
    }
    json!(0)
}

fn map_podcast_radio(r: &Value) -> Value {
    let empty = json!({});
    let dj = r.get("dj").or_else(|| r.get("djSimple")).or_else(|| r.get("djUser")).or_else(|| r.get("creator")).unwrap_or(&empty);
    let id = r.get("id").or_else(|| r.get("rid")).or_else(|| r.get("radioId")).cloned().unwrap_or(Value::Null);
    json!({
        "id": id, "rid": id,
        "name": first_str(r, &["name", "radioName"]),
        "cover": first_str(r, &["picUrl", "picURL", "coverUrl", "coverImgUrl", "avatarUrl"]),
        "desc": first_str(r, &["desc", "description", "rcmdText"]),
        "djName": if !s(dj, "nickname").is_empty() { s(dj, "nickname") } else { first_str(r, &["djName", "nickname"]) },
        "category": first_str(r, &["category", "categoryName"]),
        "programCount": first_num(r, &["programCount", "programNum", "programCnt"]),
        "subCount": first_num(r, &["subCount", "subedCount", "subscriberCount"]),
    })
}

fn map_podcast_program(p: &Value, fallback_radio: &Value) -> Value {
    let empty = json!({});
    let main_song = p.get("mainSong").or_else(|| p.get("song")).or_else(|| p.get("mainTrack")).unwrap_or(&empty);
    let radio = p.get("radio").unwrap_or(fallback_radio);
    let mapped_radio = map_podcast_radio(radio);
    let artists = map_artists(main_song.get("ar").or_else(|| main_song.get("artists")));
    let album = main_song.get("al").or_else(|| main_song.get("album")).cloned().unwrap_or_else(|| json!({}));
    let playable_id = main_song.get("id").or_else(|| p.get("mainSongId")).or_else(|| p.get("songId")).cloned().unwrap_or(Value::Null);
    let radio_name = mapped_radio.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let dj_name = mapped_radio.get("djName").and_then(|x| x.as_str()).unwrap_or("").to_string();

    let name = {
        let n = s(p, "name");
        if n.is_empty() { s(main_song, "name").to_string() } else { n.to_string() }
    };
    let cover = {
        let c = first_str(p, &["coverUrl", "cover", "blurCoverUrl"]);
        if c.is_empty() { mapped_radio.get("cover").and_then(|x| x.as_str()).unwrap_or("").to_string() } else { c.to_string() }
    };
    let duration = first_num(p, &["duration"]).as_i64().filter(|d| *d > 0).map(|d| json!(d)).unwrap_or_else(|| first_num(main_song, &["dt", "duration"]));
    let album_name = if !radio_name.is_empty() { radio_name.clone() } else { s(&album, "name").to_string() };
    let artist = if !radio_name.is_empty() { radio_name.clone() } else { dj_name.clone() };

    json!({
        "type": "podcast", "source": "podcast",
        "id": playable_id,
        "programId": p.get("id").or_else(|| p.get("programId")).cloned().unwrap_or(Value::Null),
        "radioId": mapped_radio.get("id").cloned().unwrap_or(Value::Null),
        "name": name,
        "artist": artist,
        "artists": artists,
        "album": album_name,
        "cover": cover,
        "duration": duration,
        "fee": main_song.get("fee").cloned().unwrap_or(Value::Null),
        "djName": dj_name,
        "radioName": radio_name,
        "desc": first_str(p, &["description", "desc"]),
        "serialNum": first_num(p, &["serialNum", "serial"]),
    })
}

/// /api/podcast/search → { podcasts, total }
pub async fn podcast_search(client: &reqwest::Client, keywords: &str, limit: i64) -> Result<Value, String> {
    if keywords.is_empty() {
        return Ok(json!({ "podcasts": [] }));
    }
    let body = eapi_request(client, "/api/cloudsearch/pc", json!({ "s": keywords, "type": 1009, "limit": limit, "offset": 0, "total": true })).await?;
    let result = body.get("result").cloned().unwrap_or_else(|| json!({}));
    let podcasts = result.get("djRadios").or_else(|| result.get("djradios")).or_else(|| result.get("radios"))
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().map(map_podcast_radio).collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(json!({ "podcasts": podcasts, "total": result.get("djRadiosCount").cloned().unwrap_or(json!(0)) }))
}

/// /api/podcast/hot → { podcasts, more }
pub async fn podcast_hot(client: &reqwest::Client, limit: i64, offset: i64) -> Result<Value, String> {
    let body = weapi_request(client, "/api/djradio/hot/v1", json!({ "limit": limit, "offset": offset })).await?;
    let raw = body.get("djRadios").or_else(|| body.get("djradios")).or_else(|| body.get("radios"));
    let podcasts = raw.and_then(|x| x.as_array()).map(|arr| arr.iter().map(map_podcast_radio).collect::<Vec<_>>()).unwrap_or_default();
    Ok(json!({ "podcasts": podcasts, "more": body.get("hasMore").cloned().unwrap_or(json!(false)) }))
}

/// /api/podcast/detail → { podcast }
pub async fn podcast_detail(client: &reqwest::Client, rid: &str) -> Result<Value, String> {
    let body = weapi_request(client, "/api/djradio/v2/get", json!({ "id": rid })).await?;
    let radio = body.get("data").or_else(|| body.get("djRadio")).or_else(|| body.get("radio")).unwrap_or(&body);
    Ok(json!({ "podcast": map_podcast_radio(radio) }))
}

/// /api/podcast/programs → { radio, programs, more, total }
pub async fn podcast_programs(client: &reqwest::Client, rid: &str, limit: i64, offset: i64) -> Result<Value, String> {
    let body = weapi_request(client, "/api/dj/program/byradio", json!({ "radioId": rid, "limit": limit, "offset": offset, "asc": false })).await?;
    let raw = body.get("programs").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let radio = raw.first().and_then(|p| p.get("radio")).map(map_podcast_radio).unwrap_or_else(|| json!({ "id": rid, "rid": rid }));
    let programs: Vec<Value> = raw.iter().map(|p| map_podcast_program(p, &radio)).filter(|p| !p.get("id").map(|i| i.is_null()).unwrap_or(true)).collect();
    Ok(json!({ "radio": radio, "programs": programs, "more": body.get("more").cloned().unwrap_or(json!(false)), "total": body.get("count").cloned().unwrap_or(json!(programs.len())) }))
}

/// /api/podcast/my → 登录后的播客收藏（首版：登录态返回空集合，未登录 starter）。
pub async fn podcast_my(client: &reqwest::Client) -> Value {
    let logged_in = login_info(client).await.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
    let collections: Vec<Value> = ["collect", "created", "liked"]
        .iter()
        .map(|k| json!({ "key": k, "items": [] }))
        .collect();
    json!({ "loggedIn": logged_in, "collections": collections })
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
