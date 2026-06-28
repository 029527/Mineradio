//! 天气电台（替换 server.js 的 buildWeatherRadio / fetchIpWeatherLocation）。
//! open-meteo 天气 + 心情映射 → 种子查询 → 搜索歌曲组队。

use serde_json::{json, Value};

use super::netease::endpoints;

const OPEN_METEO_FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const OPEN_METEO_GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const WEATHER_IP_LOCATION_URL: &str = "http://ip-api.com/json/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

struct Loc {
    name: String,
    country: String,
    admin1: String,
    latitude: f64,
    longitude: f64,
    timezone: String,
    fallback: bool,
}

fn default_location() -> Loc {
    Loc {
        name: "上海".into(),
        country: "China".into(),
        admin1: String::new(),
        latitude: 31.2304,
        longitude: 121.4737,
        timezone: "Asia/Shanghai".into(),
        fallback: false,
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = client.get(url).header(reqwest::header::USER_AGENT, UA).send().await.map_err(|e| e.to_string())?;
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

/// /api/weather/ip-location → { ok, location }
pub async fn ip_location(client: &reqwest::Client) -> Value {
    let url = format!("{WEATHER_IP_LOCATION_URL}?fields=status,message,country,regionName,city,lat,lon,timezone,query&lang=zh-CN");
    match get_json(client, &url).await {
        Ok(b) if b.get("status").and_then(|s| s.as_str()) == Some("success") => json!({
            "ok": true,
            "location": {
                "provider": "ip-api",
                "city": b.get("city").and_then(|x| x.as_str()).unwrap_or("上海"),
                "region": b.get("regionName").and_then(|x| x.as_str()).unwrap_or(""),
                "country": b.get("country").and_then(|x| x.as_str()).unwrap_or(""),
                "latitude": b.get("lat").cloned().unwrap_or(Value::Null),
                "longitude": b.get("lon").cloned().unwrap_or(Value::Null),
                "timezone": b.get("timezone").and_then(|x| x.as_str()).unwrap_or(""),
            }
        }),
        Ok(b) => json!({ "ok": false, "error": b.get("message").and_then(|x| x.as_str()).unwrap_or("IP_LOCATION_FAILED"), "location": Value::Null }),
        Err(e) => json!({ "ok": false, "error": e, "location": Value::Null }),
    }
}

async fn resolve_location(client: &reqwest::Client, query: &str) -> Loc {
    let raw = query.trim();
    if raw.is_empty() {
        return default_location();
    }
    let url = format!("{OPEN_METEO_GEOCODE_URL}?name={}&count=1&language=zh&format=json", urlencoding(raw));
    match get_json(client, &url).await {
        Ok(b) => {
            if let Some(first) = b.get("results").and_then(|r| r.as_array()).and_then(|a| a.first()) {
                return Loc {
                    name: first.get("name").and_then(|x| x.as_str()).unwrap_or(raw).into(),
                    country: first.get("country").and_then(|x| x.as_str()).unwrap_or("").into(),
                    admin1: first.get("admin1").and_then(|x| x.as_str()).unwrap_or("").into(),
                    latitude: first.get("latitude").and_then(|x| x.as_f64()).unwrap_or(0.0),
                    longitude: first.get("longitude").and_then(|x| x.as_f64()).unwrap_or(0.0),
                    timezone: first.get("timezone").and_then(|x| x.as_str()).unwrap_or("auto").into(),
                    fallback: false,
                };
            }
            let mut d = default_location();
            d.fallback = true;
            d
        }
        Err(_) => {
            let mut d = default_location();
            d.fallback = true;
            d
        }
    }
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

fn weather_label(code: i64) -> &'static str {
    match code {
        0 => "晴",
        1 | 2 => "少云",
        3 => "阴",
        45 | 48 => "雾",
        51 | 53 | 55 => "毛毛雨",
        56 | 57 | 66 | 67 => "冻雨",
        61 | 63 | 65 => "雨",
        71 | 73 | 75 | 77 => "雪",
        80 | 81 | 82 => "阵雨",
        85 | 86 => "阵雪",
        95 | 96 | 99 => "雷雨",
        _ => "天气",
    }
}

fn parse_hour(time: Option<&str>) -> i64 {
    time.and_then(|t| t.split('T').nth(1))
        .and_then(|hm| hm.split(':').next())
        .and_then(|h| h.parse::<i64>().ok())
        .unwrap_or(12)
}

fn kw(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// 心情映射（对应 buildWeatherMood）。返回 (key, title, tagline, keywords)。
fn build_mood(weather: &Value) -> Value {
    let code = weather.get("weatherCode").and_then(|x| x.as_i64()).unwrap_or(-1);
    let temp = weather.get("temperature").and_then(|x| x.as_f64());
    let apparent = weather.get("apparentTemperature").and_then(|x| x.as_f64());
    let feels = apparent.or(temp).unwrap_or(20.0);
    let rain = weather.get("precipitation").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let humidity = weather.get("humidity").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let wind = weather.get("windSpeed").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let is_day = weather.get("isDay").and_then(|x| x.as_i64());
    let hour = parse_hour(weather.get("time").and_then(|x| x.as_str()));

    let is_night = is_day == Some(0) || hour < 6 || hour >= 20;
    let is_morning = (5..11).contains(&hour);
    let is_dusk = (17..20).contains(&hour);
    let is_rain = rain > 0.0 || [51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 80, 81, 82, 95, 96, 99].contains(&code);
    let is_snow = [71, 73, 75, 77, 85, 86].contains(&code);
    let is_cloud = [2, 3, 45, 48].contains(&code);
    let is_storm = [95, 96, 99].contains(&code);

    let (mut key, mut title, mut tagline, mut keywords) = if is_storm {
        ("storm".to_string(), "雷雨电台".to_string(), "低频更厚，适合把世界关小一点".to_string(),
         kw(&["暗色 R&B", "trip hop", "夜晚 电子", "氛围 摇滚", "雨夜 歌单"]))
    } else if is_rain {
        ("rain".to_string(), "雨天电台".to_string(), "留一点潮湿的空间给旋律".to_string(),
         kw(&["雨天 R&B", "lofi rainy", "华语 慢歌", "dream pop", "雨夜 歌单"]))
    } else if is_snow || feels <= 3.0 {
        ("snow".to_string(), "冷空气电台".to_string(), "干净、慢速、带一点冬天的颗粒感".to_string(),
         kw(&["冬天 民谣", "ambient piano", "日系 冬天", "indie folk", "安静 歌单"]))
    } else if feels >= 31.0 || humidity >= 78.0 {
        ("humid".to_string(), "闷热电台".to_string(), "降低密度，留出一点呼吸".to_string(),
         kw(&["夏日 chill", "bossa nova", "city pop 夏天", "轻电子", "海边 歌单"]))
    } else if is_cloud {
        ("cloudy".to_string(), "阴天电台".to_string(), "不急着明亮，先让声音变软".to_string(),
         kw(&["阴天 华语", "indie rock mellow", "neo soul", "chillhop", "独立 民谣"]))
    } else {
        ("clear".to_string(), "晴朗电台".to_string(), "让节奏亮一点，像窗边的光".to_string(),
         kw(&["轻快 华语", "city pop", "indie pop", "chill pop", "阳光 歌单"]))
    };

    if is_night {
        let clear = key.starts_with("clear");
        key.push_str("-night");
        title = if clear { "夜色电台".to_string() } else { title.replace("电台", "夜听") };
        tagline = "音量放低一点，让夜色参与编曲".to_string();
        let mut k = kw(&["夜晚 R&B", "late night jazz", "ambient", "lofi sleep", "夜跑 歌单"]);
        k.extend(keywords.iter().take(3).cloned());
        keywords = k;
    } else if is_morning {
        title = if key.starts_with("rain") { "雨晨电台".to_string() } else { "早晨电台".to_string() };
        let mut k = kw(&["早晨 通勤", "morning acoustic", "清晨 indie", "轻快 华语"]);
        k.extend(keywords.iter().take(3).cloned());
        keywords = k;
    } else if is_dusk {
        title = if key.starts_with("rain") { "黄昏雨声".to_string() } else { "黄昏电台".to_string() };
        let mut k = kw(&["黄昏 city pop", "日落 歌单", "落日飞车", "soul pop"]);
        k.extend(keywords.iter().take(3).cloned());
        keywords = k;
    }

    if wind >= 28.0 {
        let mut k = kw(&["公路 摇滚", "windy day playlist"]);
        k.extend(keywords.iter().take(4).cloned());
        keywords = k;
    }

    // 去重 + 截断 7
    let mut seen = std::collections::HashSet::new();
    keywords.retain(|x| seen.insert(x.clone()));
    keywords.truncate(7);

    json!({ "key": key, "title": title, "tagline": tagline, "keywords": keywords })
}

/// 种子查询（对应 weatherRadioSeedQueries）。
fn seed_queries(mood_key: &str) -> Vec<&'static str> {
    if mood_key.contains("rain") || mood_key.contains("storm") {
        vec!["陈奕迅 阴天快乐", "周杰伦 雨下一整晚", "孙燕姿 遇见", "林宥嘉 说谎", "毛不易 消愁"]
    } else if mood_key.contains("snow") || mood_key.contains("cloudy") {
        vec!["陈奕迅 好久不见", "莫文蔚 阴天", "李健 贝加尔湖畔", "朴树 平凡之路", "蔡健雅 达尔文"]
    } else if mood_key.contains("humid") {
        vec!["落日飞车 My Jinji", "告五人 爱人错过", "夏日入侵企画 想去海边", "陈绮贞 旅行的意义", "王若琳 Lost in Paradise"]
    } else if mood_key.contains("night") {
        vec!["方大同 特别的人", "陶喆 爱很简单", "Frank Ocean Pink + White", "林忆莲 夜太黑", "Norah Jones Don't Know Why"]
    } else {
        vec!["孙燕姿 天黑黑", "周杰伦 晴天", "五月天 温柔", "陈奕迅 稳稳的幸福", "王菲"]
    }
}

async fn fetch_weather(client: &reqwest::Client, loc: &Loc) -> Result<Value, String> {
    let url = format!(
        "{OPEN_METEO_FORECAST_URL}?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,rain,showers,snowfall,weather_code,cloud_cover,wind_speed_10m,wind_gusts_10m&forecast_days=1&timezone={}",
        loc.latitude, loc.longitude, if loc.timezone.is_empty() { "auto" } else { &loc.timezone }
    );
    let body = get_json(client, &url).await?;
    let cur = body.get("current").cloned().unwrap_or_else(|| json!({}));
    let num = |k: &str| cur.get(k).cloned().unwrap_or(Value::Null);
    let code = cur.get("weather_code").and_then(|x| x.as_i64()).unwrap_or(-1);
    Ok(json!({
        "provider": "open-meteo",
        "location": {
            "name": loc.name, "country": loc.country, "admin1": loc.admin1,
            "latitude": loc.latitude, "longitude": loc.longitude,
            "timezone": body.get("timezone").and_then(|x| x.as_str()).unwrap_or(&loc.timezone),
            "fallback": loc.fallback,
        },
        "label": weather_label(code),
        "weatherCode": code,
        "temperature": num("temperature_2m"),
        "apparentTemperature": num("apparent_temperature"),
        "humidity": num("relative_humidity_2m"),
        "precipitation": cur.get("precipitation").or_else(|| cur.get("rain")).cloned().unwrap_or(json!(0)),
        "cloudCover": num("cloud_cover"),
        "windSpeed": num("wind_speed_10m"),
        "windGusts": num("wind_gusts_10m"),
        "isDay": num("is_day"),
        "time": num("time"),
    }))
}

/// /api/weather/radio
pub async fn weather_radio(
    client: &reqwest::Client,
    city: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    timezone: &str,
) -> Value {
    let loc = if let (Some(la), Some(lo)) = (lat, lon) {
        Loc {
            name: if city.is_empty() { "当前位置".into() } else { city.into() },
            country: String::new(),
            admin1: String::new(),
            latitude: la,
            longitude: lo,
            timezone: if timezone.is_empty() { "auto".into() } else { timezone.into() },
            fallback: false,
        }
    } else {
        resolve_location(client, city).await
    };

    let weather = match fetch_weather(client, &loc).await {
        Ok(mut w) => {
            w["mood"] = build_mood(&w);
            w
        }
        Err(e) => {
            return json!({
                "ok": false, "error": e, "weather": Value::Null,
                "radio": { "title": "天气电台", "subtitle": "天气暂时没有回来，可以先听今日推荐。", "seedQueries": [], "songs": [] }
            });
        }
    };

    let mood = weather.get("mood").cloned().unwrap_or_else(|| json!({}));
    let mood_key = mood.get("key").and_then(|x| x.as_str()).unwrap_or("clear");
    let queries = seed_queries(mood_key);

    // 并发搜索种子查询，组队去重
    let mut songs: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for q in queries.iter().take(4) {
        if let Ok(res) = endpoints::search(client, q, 6).await {
            if let Some(arr) = res.get("songs").and_then(|x| x.as_array()) {
                for s in arr {
                    let id = s.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
                    if id != 0 && seen.insert(id) && s.get("name").and_then(|n| n.as_str()).map(|n| !n.is_empty()).unwrap_or(false) {
                        songs.push(s.clone());
                    }
                }
            }
        }
    }
    songs.truncate(18);

    json!({
        "ok": true,
        "weather": weather,
        "radio": {
            "title": mood.get("title").cloned().unwrap_or(json!("天气电台")),
            "subtitle": mood.get("tagline").cloned().unwrap_or(json!("")),
            "seedQueries": queries.iter().take(4).collect::<Vec<_>>(),
            "songs": songs,
        }
    })
}
