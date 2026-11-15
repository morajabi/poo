//! A small history of bucket sizes and free space, kept in the state folder. From it poo
//! tells what grew (per bucket and per group), points out unusual growth, and estimates
//! how long the free space lasts at the current pace. Read-only: it never cleans anything.

use crate::config::state_dir;
use crate::model::Report;
use crate::util::{bytes, now};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DAY: i64 = 86400;
/// Samples older than this are dropped.
const KEEP_DAYS: i64 = 120;
/// Groups smaller than this are not stored (keeps the file small).
const MIN_GROUP_BYTES: u64 = 100_000_000;
/// A group growing at least this much in a day is worth a mention.
const ANOMALY_BYTES: i64 = 5_000_000_000;
/// ...or at least this much while also doubling.
const ANOMALY_DOUBLED_BYTES: i64 = 2_000_000_000;
/// Free space dropping this much in a day is worth a mention.
const FREE_DROP_BYTES: i64 = 20_000_000_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sample {
    pub at: i64,
    pub free: u64,
    pub total: u64,
    /// Bucket id to bytes (report-only buckets included).
    pub buckets: BTreeMap<String, u64>,
    /// "bucket/group" to bytes, for groups of at least 100 MB (cleanable items only).
    #[serde(default)]
    pub groups: BTreeMap<String, u64>,
}

pub fn path() -> std::path::PathBuf {
    state_dir().join("growth.json")
}

pub fn load() -> Vec<Sample> {
    std::fs::read(path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn save(samples: &[Sample]) {
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension(format!("json.tmp-{}", std::process::id()));
    if let Ok(data) = serde_json::to_vec(samples) {
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
}

pub fn sample_of(r: &Report) -> Sample {
    let mut groups: BTreeMap<String, u64> = BTreeMap::new();
    for i in r.items.iter().filter(|i| !i.report_only) {
        *groups.entry(format!("{}/{}", i.bucket, i.group)).or_default() += i.bytes;
    }
    groups.retain(|_, b| *b >= MIN_GROUP_BYTES);
    Sample {
        at: r.scanned_at,
        free: r.disk.free,
        total: r.disk.total,
        buckets: r.buckets.iter().map(|b| (b.id.clone(), b.bytes)).collect(),
        groups,
    }
}

/// Adds this report to the history and thins it: every sample from the last two days at
/// most one per hour, older ones one per day, nothing past 120 days.
pub fn record(r: &Report) {
    if r.disk.total == 0 {
        return;
    }
    let mut samples = load();
    samples.push(sample_of(r));
    save(&compact(samples, now()));
}

pub fn compact(mut samples: Vec<Sample>, at: i64) -> Vec<Sample> {
    samples.sort_by_key(|s| s.at);
    let mut out: Vec<Sample> = vec![];
    for s in samples {
        if at - s.at > KEEP_DAYS * DAY {
            continue;
        }
        let slot = |x: &Sample| if at - x.at <= 2 * DAY { x.at / 3600 } else { x.at / DAY * 24 + 100_000_000 };
        // The latest sample of each slot wins.
        if out.last().map_or(false, |l| slot(l) == slot(&s)) {
            out.pop();
        }
        out.push(s);
    }
    out
}

/// The sample closest to `at - ago`, if one is at least `min_age` old.
fn baseline(samples: &[Sample], at: i64, ago: i64, min_age: i64) -> Option<&Sample> {
    samples
        .iter()
        .filter(|s| at - s.at >= min_age)
        .min_by_key(|s| ((at - s.at) - ago).abs())
}

fn delta(now: u64, then: Option<u64>) -> Option<i64> {
    then.map(|t| now as i64 - t as i64)
}

/// Growth since a point in the past, per bucket and group, plus anomalies and a forecast.
pub fn compute(samples: &[Sample], current: &Sample, names: &BTreeMap<String, String>, since: i64) -> Value {
    let at = current.at;
    let day = baseline(samples, at, DAY, DAY * 5 / 6);
    let week = baseline(samples, at, 7 * DAY, 6 * DAY);
    let base = baseline(samples, at, since, since * 5 / 6);
    let buckets: Vec<Value> = current
        .buckets
        .iter()
        .map(|(id, b)| {
            json!({
                "id": id,
                "name": names.get(id).cloned().unwrap_or_else(|| id.clone()),
                "bytes": b,
                "delta_24h": delta(*b, day.map(|d| d.buckets.get(id).copied().unwrap_or(0))),
                "delta_7d": delta(*b, week.map(|d| d.buckets.get(id).copied().unwrap_or(0))),
                "delta": delta(*b, base.map(|d| d.buckets.get(id).copied().unwrap_or(0))),
            })
        })
        .collect();
    let mut growers: Vec<(String, u64, i64)> = match base {
        Some(b) => current
            .groups
            .iter()
            .map(|(k, v)| (k.clone(), *v, *v as i64 - b.groups.get(k).copied().unwrap_or(0) as i64))
            .filter(|(_, _, d)| *d > 0)
            .collect(),
        None => vec![],
    };
    growers.sort_by_key(|g| std::cmp::Reverse(g.2));
    let groups: Vec<Value> = growers
        .iter()
        .take(10)
        .map(|(k, v, d)| {
            let (bucket, group) = k.split_once('/').unwrap_or(("", k));
            json!({"bucket": bucket, "group": group, "bytes": v, "delta": d})
        })
        .collect();
    json!({
        "since_seconds": since,
        "from": base.map(|b| b.at),
        "to": at,
        "samples": samples.len(),
        "free": current.free,
        "free_delta": base.map(|b| current.free as i64 - b.free as i64),
        "buckets": buckets,
        "groups": groups,
        "anomalies": anomalies(day, current),
        "forecast": forecast(samples, current),
    })
}

/// Unusual growth in the last day, in plain words.
pub fn anomalies(day: Option<&Sample>, current: &Sample) -> Vec<Value> {
    let Some(d) = day else { return vec![] };
    let mut out = vec![];
    let mut grew: Vec<(&String, i64, u64)> = current
        .groups
        .iter()
        .filter_map(|(k, v)| {
            let before = d.groups.get(k).copied().unwrap_or(0);
            let g = *v as i64 - before as i64;
            let doubled = before > 0 && *v >= before * 2;
            (g >= ANOMALY_BYTES || (g >= ANOMALY_DOUBLED_BYTES && (doubled || before == 0))).then_some((k, g, *v))
        })
        .collect();
    grew.sort_by_key(|g| std::cmp::Reverse(g.1));
    for (k, g, v) in grew.into_iter().take(5) {
        let (bucket, group) = k.split_once('/').unwrap_or(("", k));
        out.push(json!({
            "kind": "group-growth",
            "bucket": bucket,
            "group": group,
            "bytes": v,
            "delta": g,
            "message": format!("{} grew {} since yesterday (now {})", group, bytes(g as u64), bytes(v)),
        }));
    }
    let drop = d.free as i64 - current.free as i64;
    if drop >= FREE_DROP_BYTES {
        out.push(json!({
            "kind": "free-drop",
            "delta": -drop,
            "message": format!("Free space went down {} since yesterday", bytes(drop as u64)),
        }));
    }
    out
}

/// A straight-line fit of free space over the last two weeks (one point per day).
pub fn forecast(samples: &[Sample], current: &Sample) -> Value {
    let at = current.at;
    let mut daily: BTreeMap<i64, (i64, f64)> = BTreeMap::new();
    for s in samples.iter().filter(|s| at - s.at <= 14 * DAY && s.at <= at) {
        daily.insert(s.at / DAY, (s.at, s.free as f64));
    }
    daily.insert(at / DAY, (at, current.free as f64));
    let pts: Vec<(f64, f64)> = daily.values().map(|(t, f)| (*t as f64 / DAY as f64, *f)).collect();
    let span = pts.last().map_or(0.0, |l| l.0) - pts.first().map_or(0.0, |f| f.0);
    if pts.len() < 3 || span < 2.0 {
        return json!({"free": current.free, "days_until_full": null, "based_on_days": span.round() as i64,
            "message": "Not enough history yet; poo needs a few days of scans to estimate."});
    }
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let per_day = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let days = (per_day < -100_000_000.0).then(|| (current.free as f64 / -per_day).max(0.0));
    let message = match days {
        Some(d) if d < 1.0 => "At this pace the disk fills within a day".to_string(),
        Some(d) => format!("At this pace the disk fills in about {} days", d.round() as i64),
        None if per_day > 100_000_000.0 => "Free space has been growing".to_string(),
        None => "Free space is steady".to_string(),
    };
    json!({
        "free": current.free,
        "change_per_day": per_day.round() as i64,
        "days_until_full": days.map(|d| (d * 10.0).round() / 10.0),
        "based_on_days": span.round() as i64,
        "message": message,
    })
}

fn names(r: &Report) -> BTreeMap<String, String> {
    r.buckets.iter().map(|b| (b.id.clone(), b.name.clone())).collect()
}

/// Growth since `since` seconds ago, for `poo growth` and MCP.
pub fn report(r: &Report, since: i64) -> Value {
    let samples = load();
    compute(&samples, &sample_of(r), &names(r), since)
}

/// The compact form in the overview and report.json: per bucket delta_24h and delta_7d,
/// anomalies and the forecast. None until there is at least one earlier sample.
pub fn summary(r: &Report) -> Option<Value> {
    let samples = load();
    if samples.iter().all(|s| s.at >= r.scanned_at) {
        return None;
    }
    let full = compute(&samples, &sample_of(r), &names(r), DAY);
    Some(json!({
        "buckets": full["buckets"].as_array().map(|a| a.iter().map(|b| json!({
            "id": b["id"], "delta_24h": b["delta_24h"], "delta_7d": b["delta_7d"],
        })).collect::<Vec<_>>()),
        "anomalies": full["anomalies"],
        "forecast": full["forecast"],
    }))
}

/// Parses `7d`, `24h`, `2w` or plain seconds.
pub fn parse_since(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num.parse().ok()?;
    let mult = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => DAY,
        "w" => 7 * DAY,
        _ => return None,
    };
    (n > 0).then_some(n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(at: i64, free: u64, dd: u64) -> Sample {
        let mut x = Sample { at, free, total: 1_000_000_000_000, ..Default::default() };
        x.buckets.insert("xcode".into(), dd);
        x.groups.insert("xcode/DerivedData".into(), dd);
        x
    }

    #[test]
    fn growth_anomaly_and_forecast() {
        let t = 100 * DAY;
        let gb = 1_000_000_000u64;
        // 10 GB less free every day; DerivedData jumps 18 GB in the last day.
        let hist: Vec<Sample> = (1..=6).rev().map(|d| s(t - d * DAY, 100 * gb + 10 * gb * d as u64, 2 * gb)).collect();
        let cur = s(t, 100 * gb, 20 * gb);
        let v = compute(&hist, &cur, &BTreeMap::new(), 7 * DAY);
        let a = &v["anomalies"][0];
        assert_eq!(a["group"], "DerivedData");
        assert!(a["message"].as_str().unwrap().contains("grew 18"));
        let days = v["forecast"]["days_until_full"].as_f64().unwrap();
        assert!((9.0..=11.0).contains(&days), "{}", days);
        assert_eq!(v["buckets"][0]["delta_24h"], json!(18 * gb as i64));
        assert_eq!(parse_since("7d"), Some(7 * DAY));
        assert_eq!(parse_since("x"), None);
    }

    #[test]
    fn thins_old_samples() {
        let t = 200 * DAY;
        let many: Vec<Sample> = (0..(10 * 24)).map(|h| s(t - h * 3600, 1, 1)).chain([s(t - 200 * DAY, 1, 1)]).collect();
        let c = compact(many, t);
        // Two days hourly (49 slots incl. now) plus one per day for the older eight.
        assert!(c.len() <= 49 + 9, "{}", c.len());
        assert!(c.iter().all(|x| t - x.at <= KEEP_DAYS * DAY));
    }
}
