//! 播客 DJ 节拍图（替换 dj-analyzer.js）。
//! symphonia 解码 MP3 → 低频带 biquad 能量包络 → 节拍栅格 → 相机/脉冲节拍图。
//! 忠实移植 buildBeatMapFromLowEnergy / analyzePodcastDjStreamFull / Intro。

use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---- 基础数学（对应 clamp01/clampRange/percentile/median）----
fn clamp01(v: f64) -> f64 {
    v.max(0.0).min(1.0)
}
fn clamp_range(v: f64, min: f64, max: f64) -> f64 {
    v.max(min).min(max)
}
fn percentile(arr: &[f64], p: f64) -> f64 {
    let len = arr.len();
    if len == 0 {
        return 0.001;
    }
    const MAX: usize = 16000;
    let mut sample: Vec<f64> = if len <= MAX {
        arr.to_vec()
    } else {
        let step = (len - 1) as f64 / (MAX - 1) as f64;
        (0..MAX).map(|i| arr[((i as f64 * step).floor() as usize).min(len - 1)]).collect()
    };
    sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sample.len() as f64 * p).floor() as usize).min(sample.len() - 1);
    let v = sample[idx];
    if v == 0.0 { 0.001 } else { v }
}
fn median(vals: &[f64]) -> f64 {
    let mut v: Vec<f64> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

// ---- biquad ----
#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}
impl Biquad {
    fn new(highpass: bool, freq: f64, q: f64, sr: f64) -> Self {
        let freq = freq.max(8.0).min(sr * 0.45);
        let w0 = 2.0 * std::f64::consts::PI * freq / sr;
        let cos = w0.cos();
        let sin = w0.sin();
        let alpha = sin / (2.0 * if q != 0.0 { q } else { 0.707 });
        let (b0, b1, b2) = if highpass {
            ((1.0 + cos) * 0.5, -(1.0 + cos), (1.0 + cos) * 0.5)
        } else {
            ((1.0 - cos) * 0.5, 1.0 - cos, (1.0 - cos) * 0.5)
        };
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos;
        let a2 = 1.0 - alpha;
        let inv = 1.0 / a0;
        Biquad { b0: b0 * inv, b1: b1 * inv, b2: b2 * inv, a1: a1 * inv, a2: a2 * inv, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

struct Energy {
    low: Vec<f64>,
    hit: Vec<f64>,
    hop_sec: f64,
    duration: f64,
    sample_rate: u32,
    effective_sr: f64,
}

/// 从 MP3 字节解码出低频能量包络（对应 decodePodcastDjEnergyRange / StreamFull 的解码部分）。
/// limit_sec>0 时解码到该时长即停（intro 用）。
fn decode_energy(bytes: Vec<u8>, hop_sec: f64, limit_sec: f64) -> Result<Energy, String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("probe: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("make decoder: {e}"))?;

    let mut hp: Option<Biquad> = None;
    let mut lp: Option<Biquad> = None;
    let mut sample_step = 1usize;
    let mut hop_size = 0usize;
    let mut effective_sr = 0.0f64;
    let mut sample_rate = 0u32;
    let mut frame_sum = 0.0f64;
    let mut frame_peak = 0.0f64;
    let mut frame_count = 0usize;
    let mut effective_samples = 0usize;
    let mut low: Vec<f64> = Vec::new();
    let mut hit: Vec<f64> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break, // EOF 或读尽
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio_buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *audio_buf.spec();
        if effective_sr == 0.0 {
            let sr = spec.rate as f64;
            sample_rate = spec.rate;
            sample_step = if sr >= 44100.0 { 4 } else if sr >= 32000.0 { 3 } else { 2 };
            effective_sr = sr / sample_step as f64;
            hop_size = ((effective_sr * hop_sec).floor() as usize).max(80);
            hp = Some(Biquad::new(true, 32.0, 0.72, effective_sr));
            lp = Some(Biquad::new(false, 178.0, 0.82, effective_sr));
        }
        let ch = spec.channels.count().max(1);
        let mut sb = SampleBuffer::<f32>::new(audio_buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(audio_buf);
        let samples = sb.samples();
        let frames = samples.len() / ch;
        let (hp, lp) = (hp.as_mut().unwrap(), lp.as_mut().unwrap());
        let mut i = 0usize;
        while i < frames {
            let x = if ch >= 2 {
                (samples[i * ch] as f64 + samples[i * ch + 1] as f64) * 0.5
            } else {
                samples[i * ch] as f64
            };
            let y = lp.run(hp.run(x));
            let ay = y.abs();
            frame_sum += y * y;
            if ay > frame_peak {
                frame_peak = ay;
            }
            frame_count += 1;
            effective_samples += 1;
            if frame_count >= hop_size {
                let c = frame_count.max(1) as f64;
                low.push((frame_sum / c).sqrt());
                hit.push(frame_peak);
                frame_sum = 0.0;
                frame_peak = 0.0;
                frame_count = 0;
            }
            i += sample_step;
        }
        if limit_sec > 0.0 && effective_sr > 0.0 && effective_samples as f64 / effective_sr >= limit_sec {
            break;
        }
    }
    if frame_count > 0 {
        let c = frame_count.max(1) as f64;
        low.push((frame_sum / c).sqrt());
        hit.push(frame_peak);
    }

    let duration = if effective_sr > 0.0 { effective_samples as f64 / effective_sr } else { 0.0 };
    Ok(Energy { low, hit, hop_sec, duration, sample_rate, effective_sr })
}

// ---- 节拍图构建（忠实移植 buildBeatMapFromLowEnergy）----

struct Cand {
    frame: usize,
    time: f64,
    score: f64,
    low_tone: f64,
    hit_tone: f64,
    low_rel: f64,
    power: f64,
}

fn empty_map(duration: f64) -> Value {
    json!({
        "kicks": [], "beats": [], "pulseBeats": [], "cameraBeats": [],
        "duration": duration, "visualBeatCount": 0,
        "tempoSource": "podcast-dj-server-empty", "analyzedAt": now_ms(),
    })
}

pub fn build_beatmap(low: &[f64], hit: &[f64], hop_sec: f64, duration_sec: f64) -> Value {
    let n = low.len().min(hit.len());
    if n < 20 {
        return empty_map(if duration_sec > 0.0 { duration_sec } else { 0.0 });
    }
    let low = &low[..n];
    let hit = &hit[..n];

    let band_at = |arr: &[f64], idx: i64| -> f64 {
        let idx = idx.max(0).min(n as i64 - 1) as usize;
        let a = if idx >= 1 { arr[idx - 1] } else { arr[0] };
        let b = arr[idx];
        let c = arr[(idx + 1).min(n - 1)];
        (a + b * 2.0 + c) * 0.25
    };

    let low_floor = 0.0004f64.max(percentile(low, 0.22));
    let low_mid = (low_floor + 0.0002).max(percentile(low, 0.58));
    let low_ref = (low_mid + 0.0002).max(percentile(low, 0.86));
    let low_ceil = (low_ref + 0.0004).max(percentile(low, 0.96));
    let hit_ref = 0.0004f64.max(percentile(hit, 0.86));

    let mut onset = vec![0.0f64; n];
    for i in 4..n {
        let prev = low[i - 1] * 0.62 + low[i - 2] * 0.28 + low[i - 3] * 0.10;
        let low_rise = (low[i] - prev).max(0.0);
        let wide_rise = ((low[i] + low[i - 1]) * 0.5 - (low[i - 3] + low[i - 4]) * 0.5).max(0.0);
        let peak_rise = (hit[i] - hit[i - 2] * 0.84).max(0.0);
        onset[i] = low_rise * 1.72 + wide_rise * 0.86 + peak_rise * 0.10;
    }

    let win_n = (52).max((0.82 / hop_sec).round() as usize);
    let min_frame_gap = (18).max((0.215 / hop_sec).round() as usize);
    let mut candidates: Vec<Cand> = Vec::new();
    let mut sum_o = 0.0f64;
    let mut sq_o = 0.0f64;
    for i in 0..win_n {
        let o = onset.get(i).copied().unwrap_or(0.0);
        sum_o += o;
        sq_o += o * o;
    }
    let mut f = win_n + 4;
    while f + 4 < n {
        let mean = sum_o / win_n as f64;
        let std = (sq_o / win_n as f64 - mean * mean).max(0.0).sqrt();
        let th = mean + std * 1.66 + low_ref * 0.0038;
        let o = onset[f];
        if o > th && o >= onset[f - 1] && o > onset[f + 1] {
            let mut peak_f = f;
            let mut peak_score = o + low[f] * 0.10;
            for pf in (f - 2)..=(f + 3) {
                let ps = onset.get(pf).copied().unwrap_or(0.0) + low.get(pf).copied().unwrap_or(0.0) * 0.10;
                if ps > peak_score {
                    peak_score = ps;
                    peak_f = pf;
                }
            }
            let low_tone = (band_at(low, peak_f as i64) / low_ref).min(2.6);
            let hit_tone = (band_at(hit, peak_f as i64) / hit_ref).min(2.6);
            let low_rel = clamp01((band_at(low, peak_f as i64) - low_floor) / (low_ceil - low_floor).max(0.0001));
            let score = (o - th) / (std + mean * 0.38 + low_ref * 0.012).max(0.0006);
            if score > 0.16 && (low_tone > 0.32 || low_rel > 0.22 || hit_tone > 0.52) {
                let power = score * 0.56
                    + clamp01((low_tone - 0.22) / 1.42).powf(0.82) * 0.34
                    + hit_tone.min(1.5) * 0.08
                    + low_rel * 0.10;
                let cand = Cand { frame: peak_f, time: peak_f as f64 * hop_sec, score, low_tone, hit_tone, low_rel, power };
                if let Some(last) = candidates.last() {
                    // 对齐 Node：间隔(可为负)< minFrameGap 视为同一拍，取功率更高者。
                    if (cand.frame as i64 - last.frame as i64) < min_frame_gap as i64 {
                        if cand.power > last.power {
                            *candidates.last_mut().unwrap() = cand;
                        }
                    } else {
                        candidates.push(cand);
                    }
                } else {
                    candidates.push(cand);
                }
            }
        }
        let old = onset.get(f - win_n).copied().unwrap_or(0.0);
        let next = onset[f];
        sum_o += next - old;
        sq_o += next * next - old * old;
        f += 1;
    }

    if candidates.is_empty() {
        return empty_map(if duration_sec > 0.0 { duration_sec } else { n as f64 * hop_sec });
    }

    let powers: Vec<f64> = candidates.iter().map(|c| c.power).collect();
    let p30 = percentile(&powers, 0.30);
    let p50 = percentile(&powers, 0.50);
    let p90 = (p50 + 0.001).max(percentile(&powers, 0.90));
    let p96 = (p90 + 0.001).max(percentile(&powers, 0.965));

    let strong_idx: Vec<usize> = candidates.iter().enumerate().filter(|(_, c)| c.power >= p50 && c.low_tone > 0.34).map(|(i, _)| i).collect();
    let strong: Vec<usize> = if strong_idx.len() < 16 { (0..candidates.len()).collect() } else { strong_idx };

    let estimate_step = |list: &[usize]| -> f64 {
        if list.len() < 3 {
            return 0.0;
        }
        let bin = 0.006f64;
        let mut hist: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
        let mut med_gaps: Vec<f64> = Vec::new();
        for ai in 0..list.len() {
            let mut bi = ai + 1;
            while bi < list.len() && bi < ai + 10 {
                let raw_gap = candidates[list[bi]].time - candidates[list[ai]].time;
                if raw_gap < 0.24 {
                    bi += 1;
                    continue;
                }
                if raw_gap > 2.55 {
                    break;
                }
                for div in 1..=6 {
                    let g = raw_gap / div as f64;
                    if g < 0.31 {
                        break;
                    }
                    if g > 0.86 {
                        continue;
                    }
                    let weight = (candidates[list[ai]].power * candidates[list[bi]].power).max(0.001).sqrt()
                        / (((bi - ai) * div) as f64).sqrt();
                    let key = (g / bin).round() as i64;
                    *hist.entry(key).or_insert(0.0) += weight;
                    med_gaps.push(g);
                }
                bi += 1;
            }
        }
        let mut best_key: Option<i64> = None;
        let mut best_score = 0.0f64;
        for (&key, _) in hist.iter() {
            let score = hist.get(&key).copied().unwrap_or(0.0)
                + hist.get(&(key - 1)).copied().unwrap_or(0.0) * 0.72
                + hist.get(&(key + 1)).copied().unwrap_or(0.0) * 0.72;
            if score > best_score {
                best_score = score;
                best_key = Some(key);
            }
        }
        if let Some(k) = best_key {
            k as f64 * bin
        } else {
            median(&med_gaps)
        }
    };

    let mut global_step = {
        let s = estimate_step(&strong);
        let s = if s != 0.0 { s } else { estimate_step(&(0..candidates.len()).collect::<Vec<_>>()) };
        if s != 0.0 { s } else { 0.50 }
    };
    global_step = clamp_range(global_step, 0.32, 0.86);

    let dur_default = if duration_sec > 0.0 { duration_sec } else { n as f64 * hop_sec };

    let nearest_candidate = |center: f64, window: f64| -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_score = f64::NEG_INFINITY;
        for ni in 0..candidates.len() {
            if candidates[ni].time < center - window {
                continue;
            }
            if candidates[ni].time > center + window {
                break;
            }
            let dist = (candidates[ni].time - center).abs();
            let score = candidates[ni].power * (1.0 - dist / window.max(0.001) * 0.42);
            if score > best_score {
                best = Some(ni);
                best_score = score;
            }
        }
        best
    };

    let score_phase = |anchor_time: f64, step: f64| -> f64 {
        let mut start = anchor_time;
        while start - step > 0.05 {
            start -= step;
        }
        let end = dur_default.min(180.0);
        let win = (step * 0.18).max(0.055).min(0.125);
        let mut score = 0.0;
        let mut count = 0;
        let mut cursor = 0usize;
        let mut gt = start;
        while gt < end {
            while cursor < candidates.len() && candidates[cursor].time < gt - win {
                cursor += 1;
            }
            let mut best_score = 0.0f64;
            let mut pi = cursor;
            while pi < candidates.len() && candidates[pi].time <= gt + win {
                let dist = (candidates[pi].time - gt).abs();
                let s = candidates[pi].power * (1.0 - dist / win * 0.44);
                if s > best_score {
                    best_score = s;
                }
                pi += 1;
            }
            score += if best_score != 0.0 { best_score } else { -p30 * 0.08 };
            count += 1;
            gt += step;
        }
        if count > 0 { score / count as f64 } else { f64::NEG_INFINITY }
    };

    let mut phase_source: Vec<usize> = strong.iter().copied().filter(|&i| candidates[i].time < dur_default.min(180.0)).take(72).collect();
    if phase_source.is_empty() {
        phase_source = strong.iter().copied().take(1).collect();
    }
    let mut best_anchor = phase_source.first().map(|&i| candidates[i].time).unwrap_or(0.0);
    let mut best_anchor_score = f64::NEG_INFINITY;
    for &i in &phase_source {
        let score = score_phase(candidates[i].time, global_step);
        if score > best_anchor_score {
            best_anchor_score = score;
            best_anchor = candidates[i].time;
        }
    }
    let half_step = global_step * 0.5;
    if half_step >= 0.31 {
        let half_score = score_phase(best_anchor, half_step);
        if half_score > best_anchor_score * 1.04 {
            global_step = half_step;
        }
    }
    let mut anchor = best_anchor;
    while anchor - global_step > 0.05 {
        anchor -= global_step;
    }

    let duration = dur_default;
    let section_len = if duration > 3600.0 { 96.0 } else { 72.0 };
    let section_count = ((duration / section_len).ceil() as usize).max(1);
    let mut section_steps: Vec<f64> = Vec::new();
    for si in 0..section_count {
        let t0 = si as f64 * section_len;
        let t1 = duration.min(t0 + section_len);
        let seg: Vec<usize> = strong.iter().copied().filter(|&i| candidates[i].time >= t0 && candidates[i].time < t1).collect();
        let prev_step = section_steps.last().copied().unwrap_or(global_step);
        let mut local_step = {
            let s = estimate_step(&seg);
            if s != 0.0 { s } else if prev_step != 0.0 { prev_step } else { global_step }
        };
        if prev_step != 0.0 {
            local_step = clamp_range(local_step, prev_step * 0.94, prev_step * 1.06);
        }
        if global_step != 0.0 {
            local_step = clamp_range(local_step, global_step * 0.86, global_step * 1.14);
        }
        section_steps.push(if prev_step != 0.0 { local_step * 0.30 + prev_step * 0.70 } else { local_step });
    }
    let step_at = |time: f64| -> f64 {
        let idx = ((time / section_len).floor() as i64).max(0).min(section_steps.len() as i64 - 1) as usize;
        let s = section_steps.get(idx).copied().unwrap_or(global_step);
        if s != 0.0 { s } else if global_step != 0.0 { global_step } else { 0.50 }
    };

    let mut beats: Vec<Value> = Vec::new();
    let mut camera_beats: Vec<Value> = Vec::new();
    let mut pulse_beats: Vec<Value> = Vec::new();
    let mut kicks: Vec<f64> = Vec::new();
    let mut grid_index = 0usize;
    let mut grid_t = anchor;
    while grid_t < duration - 0.04 {
        let local_step = step_at(grid_t);
        let win_sec = (local_step * 0.20).max(0.060).min(0.135);
        let best_cand = nearest_candidate(grid_t, win_sec);
        let gf = ((grid_t / hop_sec).round() as i64).max(0).min(n as i64 - 1);
        let grid_low = band_at(low, gf);
        let grid_hit = band_at(hit, gf);
        let grid_low_tone = (grid_low / low_ref).min(2.6);
        let grid_hit_tone = (grid_hit / hit_ref).min(2.6);
        let (bc_low_tone, bc_hit_tone, bc_time, bc_power) = match best_cand {
            Some(ci) => (candidates[ci].low_tone, candidates[ci].hit_tone, candidates[ci].time, candidates[ci].power),
            None => (0.0, 0.0, 0.0, 0.0),
        };
        let low_tone = if best_cand.is_some() { (grid_low_tone * 0.62).max(bc_low_tone) } else { grid_low_tone };
        let hit_tone = if best_cand.is_some() { (grid_hit_tone * 0.62).max(bc_hit_tone) } else { grid_hit_tone };
        let dist_penalty = if best_cand.is_some() {
            1.0 - ((bc_time - grid_t).abs() / win_sec).min(1.0) * 0.26
        } else {
            0.54
        };
        let base_power = if best_cand.is_some() { bc_power * dist_penalty } else { grid_low_tone * 0.25 + grid_hit_tone * 0.06 };
        let power_rel = clamp01((base_power - p30 * 0.78) / (p96 - p30 * 0.78).max(0.001));
        let low_rel = clamp01((grid_low - low_floor) / (low_ceil - low_floor).max(0.0001));
        let kick_rel = clamp01(power_rel * 0.74 + low_rel * 0.22 + clamp01((hit_tone - 0.26) / 1.70) * 0.04);
        let soft_grid = (best_cand.is_none() && low_rel < 0.20) || kick_rel < 0.16;
        let slot = grid_index % 4;
        let mut combo = match slot {
            0 => "downbeat",
            1 => "push",
            2 => "drop",
            _ => "rebound",
        };
        if kick_rel > 0.84 && combo != "downbeat" {
            combo = "accent";
        }
        let visual_rel = if kick_rel > 0.76 { 0.76 + (kick_rel - 0.76) * 0.52 } else { kick_rel };
        let down_lift = if combo == "downbeat" {
            if visual_rel > 0.18 { 0.016 + visual_rel * 0.036 } else { visual_rel * 0.028 }
        } else {
            0.0
        };
        let section_gate = clamp01((kick_rel - 0.10) / 0.58);
        let mut impact = (0.022 + visual_rel.powf(1.62) * 0.86 + down_lift).max(0.020).min(0.88);
        let mut strength = (0.13 + visual_rel.powf(1.12) * 0.68 + down_lift * 0.70).max(0.12).min(0.93);
        if soft_grid {
            let soft_mul = if combo == "downbeat" { 0.48 } else { 0.30 };
            impact *= soft_mul;
            strength *= 0.58 + section_gate * 0.22;
        }
        let timing_pull = if best_cand.is_some() { 0.24 + clamp01((kick_rel - 0.25) / 0.65) * 0.46 } else { 0.0 };
        let source_time = if best_cand.is_some() { grid_t * (1.0 - timing_pull) + bc_time * timing_pull } else { grid_t };
        // 忠实复刻 Node 的 JS 真值语义：bestCand 为 null 时 `bestCand && ...` 得 null，
        // 整个 || 链返回 null；cameraBeats 过滤 `camera !== false` → null 也算相机节拍。
        // 三态：Some(true)/Some(false)/None(=JS null)。
        let camera_active: Option<bool> = if impact >= 0.13 {
            Some(true)
        } else if combo == "downbeat" && kick_rel >= 0.14 {
            Some(true)
        } else if best_cand.is_some() {
            Some(kick_rel >= 0.18)
        } else {
            None
        };
        let camera_json = match camera_active {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        };
        let low_mix = (0.52 + visual_rel * 0.32 + low_tone * 0.035 - if combo == "accent" { 0.10 } else { 0.0 }).max(0.42).min(0.90);
        let body_mix = (0.060 + visual_rel * 0.12 + if combo == "push" { 0.18 } else { 0.0 } + if combo == "drop" { 0.24 } else { 0.0 }).max(0.035).min(0.54);
        let snap_mix = (0.026 + if combo == "accent" { 0.40 } else { 0.0 } + if combo == "rebound" { 0.08 } else { 0.0 } + visual_rel * 0.038).max(0.015).min(0.62);
        let confidence = (0.46 + kick_rel * 0.43 + if best_cand.is_some() { 0.08 } else { -0.03 }).max(0.44).min(0.99);
        let mass = (low_mix * 0.72 + visual_rel.powf(1.22) * 0.24).max(0.36).min(0.94);
        let sharpness = (snap_mix * 1.18).max(0.03).min(0.28);
        let pulse = impact > 0.16 || (combo == "downbeat" && kick_rel >= 0.18);

        let beat = json!({
            "time": source_time, "strength": strength, "confidence": confidence, "impact": impact,
            "primary": camera_json.clone(), "camera": camera_json.clone(), "pulse": pulse, "tone": "podcast-dj-server-low-grid",
            "low": low_mix, "body": body_mix, "snap": snap_mix, "mass": mass, "sharpness": sharpness,
            "combo": combo, "step": local_step, "index": beats.len(), "dj": true, "grid": true, "kickOnly": true, "server": true,
        });
        kicks.push(source_time);
        // cameraBeats = beats where camera !== false（Some(true) 和 None 都算）。
        if camera_active != Some(false) {
            camera_beats.push(beat.clone());
        }
        if pulse && (impact >= 0.16 || combo == "downbeat") {
            pulse_beats.push(json!({ "time": source_time, "strength": strength, "impact": impact, "combo": combo, "low": low_mix, "body": body_mix, "snap": snap_mix, "dj": true }));
        }
        beats.push(beat);
        grid_index += 1;
        grid_t += local_step;
    }

    json!({
        "kicks": kicks,
        "beats": beats,
        "pulseBeats": pulse_beats,
        "cameraBeats": camera_beats,
        "gridStep": global_step,
        "sectionSteps": section_steps,
        "tempoSource": "podcast-dj-server-low-offline",
        "duration": duration,
        "visualBeatCount": camera_beats.len(),
        "analyzedAt": now_ms(),
        "debug": { "candidates": candidates.len(), "hopSec": hop_sec, "lowRef": low_ref, "step": global_step },
    })
}

// ---- 抓取 + 编排 ----

async fn fetch_audio(client: &reqwest::Client, url: &str, range: Option<&str>) -> Result<Vec<u8>, String> {
    let mut req = client
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .header(reqwest::header::REFERER, "https://music.163.com/");
    if let Some(r) = range {
        req = req.header(reqwest::header::RANGE, r);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() && resp.status().as_u16() != 206 {
        return Err(format!("Audio fetch failed: {}", resp.status()));
    }
    Ok(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

fn decode_meta(e: &Energy, requested: f64, intro: bool) -> Value {
    json!({
        "sampleRate": e.sample_rate,
        "effectiveSampleRate": e.effective_sr,
        "frames": e.low.len(),
        "requestedDurationSec": requested,
        "effectiveDurationSec": e.duration,
        "intro": intro,
    })
}

/// /api/podcast/dj-beatmap（无 intro）：整段解码 + 构建。
pub async fn analyze_stream(client: &reqwest::Client, url: &str, duration_sec: f64) -> Result<Value, String> {
    let bytes = fetch_audio(client, url, None).await?;
    let hop_sec = if duration_sec > 9000.0 { 0.0125 } else { 0.010 };
    let map = tokio::task::spawn_blocking(move || {
        let e = decode_energy(bytes, hop_sec, 0.0)?;
        let dur = if e.duration > 0.0 { e.duration } else { duration_sec };
        let mut map = build_beatmap(&e.low, &e.hit, e.hop_sec, dur);
        if let Some(o) = map.as_object_mut() {
            o.insert("decode".into(), decode_meta(&e, duration_sec, false));
        }
        Ok::<Value, String>(map)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(map)
}

/// /api/podcast/dj-beatmap（intro=N）：只解码前 introSec，标记 partial。
pub async fn analyze_intro(client: &reqwest::Client, url: &str, duration_sec: f64, intro_sec: f64) -> Result<Value, String> {
    let intro_sec = clamp_range(if intro_sec > 0.0 { intro_sec } else { 180.0 }, 90.0, 240.0);
    // 用 Range 取大致前 intro 段（~6MB，覆盖 ~180-300s 常见码率）。
    let bytes = fetch_audio(client, url, Some("bytes=0-6291456")).await?;
    let map = tokio::task::spawn_blocking(move || {
        let e = decode_energy(bytes, 0.010, intro_sec + 8.0)?;
        let frame_limit = (((intro_sec + 2.0) / e.hop_sec.max(0.001)).ceil() as usize).min(e.low.len()).max(1);
        let low = &e.low[..frame_limit.min(e.low.len())];
        let hit = &e.hit[..frame_limit.min(e.hit.len())];
        let map_duration = intro_sec.min(low.len() as f64 * e.hop_sec);
        let mut map = build_beatmap(low, hit, e.hop_sec, map_duration);
        if let Some(o) = map.as_object_mut() {
            o.insert("partial".into(), json!(true));
            o.insert("partialUntilSec".into(), json!(map_duration));
            o.insert("fullDuration".into(), json!(duration_sec));
            o.insert("tempoSource".into(), json!("podcast-dj-server-intro-offline"));
            o.insert("decode".into(), decode_meta(&e, duration_sec, true));
        }
        Ok::<Value, String>(map)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(map)
}
