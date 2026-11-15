//! Local APFS snapshots and purgeable space. Report first: poo explains where the space
//! is and who owns it. The only thing it can remove is a Time Machine local snapshot older
//! than a day (`tmutil deletelocalsnapshots <date>`), and only in in-use mode.

use super::Raw;
use crate::catalog::Rule;
use crate::fsutil::Sized;
use crate::model::{ActionKind, Cost, Tier};
use crate::util::{home, now, run_capture};
use serde_json::Value;
use std::path::PathBuf;

/// One APFS snapshot as `diskutil apfs listSnapshots -plist` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub uuid: String,
    pub purgeable: Option<bool>,
}

/// Snapshot names from `tmutil listlocalsnapshots /`.
pub fn parse_tmutil(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| l.starts_with("com.apple.TimeMachine.")).map(String::from).collect()
}

/// Snapshots from `diskutil apfs listSnapshots -plist <volume>`.
pub fn parse_diskutil_plist(data: &[u8]) -> Vec<Snapshot> {
    let Ok(v) = plist::Value::from_reader(std::io::Cursor::new(data)) else { return vec![] };
    let Some(list) = v.as_dictionary().and_then(|d| d.get("Snapshots")).and_then(|s| s.as_array()) else { return vec![] };
    list.iter()
        .filter_map(|s| s.as_dictionary())
        .filter_map(|d| {
            Some(Snapshot {
                name: d.get("SnapshotName")?.as_string()?.to_string(),
                uuid: d.get("SnapshotUUID").and_then(|u| u.as_string()).unwrap_or_default().to_string(),
                purgeable: d.get("Purgeable").and_then(|p| p.as_boolean()),
            })
        })
        .collect()
}

/// `2026-11-14-120000` (or a full `com.apple.TimeMachine.<date>.local` name), local time.
pub fn parse_snapshot_date(s: &str) -> Option<i64> {
    let d = s.strip_prefix("com.apple.TimeMachine.").unwrap_or(s);
    let d = d.strip_suffix(".local").unwrap_or(d);
    let b = d.as_bytes();
    if b.len() != 17 || b[4] != b'-' || b[7] != b'-' || b[10] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| d.get(r)?.parse::<i32>().ok();
    let (y, mo, da, h, mi, se) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(13..15)?, num(15..17)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900;
    tm.tm_mon = mo - 1;
    tm.tm_mday = da;
    tm.tm_hour = h;
    tm.tm_min = mi;
    tm.tm_sec = se;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t as i64)
}

/// The date part tmutil wants for `deletelocalsnapshots`.
pub fn snapshot_date(name: &str) -> Option<&str> {
    let d = name.strip_prefix("com.apple.TimeMachine.")?;
    Some(d.strip_suffix(".local").unwrap_or(d))
}

/// Which app made a snapshot, from its name. None for Time Machine and macOS updates.
pub fn snapshot_owner(name: &str) -> Option<&'static str> {
    const OWNERS: &[(&str, &str)] = &[
        ("com.bombich.ccc", "Carbon Copy Cloner"),
        ("com.backblaze", "Backblaze"),
        ("com.arqbackup", "Arq"),
        ("com.haystacksoftware.arq", "Arq"),
        ("com.shirtpocket", "SuperDuper!"),
        ("com.econtechnologies.chronosync", "ChronoSync"),
        ("com.apple.TimeMachine", ""),
        ("com.apple.os.update", ""),
    ];
    match OWNERS.iter().find(|(p, _)| name.starts_with(p)) {
        Some((_, "")) => None,
        Some((_, o)) => Some(o),
        None => Some("another app"),
    }
}

/// Space macOS counts as available for important work beyond what is free right now:
/// local snapshots, iCloud-optimized files and caches it can reclaim on its own.
pub fn purgeable_bytes() -> Option<u64> {
    let important = important_available(&home())?;
    let free = crate::fsutil::disk_info(&home()).free;
    Some(important.saturating_sub(free))
}

#[cfg(target_os = "macos")]
fn important_available(path: &std::path::Path) -> Option<u64> {
    use std::ffi::c_void;
    use std::os::unix::ffi::OsStrExt;
    type CFRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFURLVolumeAvailableCapacityForImportantUsageKey: CFRef;
        fn CFURLCreateFromFileSystemRepresentation(alloc: CFRef, buf: *const u8, len: isize, is_dir: u8) -> CFRef;
        fn CFURLCopyResourcePropertyForKey(url: CFRef, key: CFRef, value: *mut CFRef, error: *mut CFRef) -> u8;
        fn CFNumberGetValue(n: CFRef, the_type: isize, value: *mut c_void) -> u8;
        fn CFRelease(cf: CFRef);
    }
    const K_CF_NUMBER_SINT64: isize = 4;
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 1);
        if url.is_null() {
            return None;
        }
        let mut value: CFRef = std::ptr::null();
        let ok = CFURLCopyResourcePropertyForKey(url, kCFURLVolumeAvailableCapacityForImportantUsageKey, &mut value, std::ptr::null_mut());
        CFRelease(url);
        if ok == 0 || value.is_null() {
            return None;
        }
        let mut n: i64 = 0;
        let got = CFNumberGetValue(value, K_CF_NUMBER_SINT64, &mut n as *mut i64 as *mut c_void);
        CFRelease(value);
        (got != 0 && n > 0).then_some(n as u64)
    }
}

#[cfg(not(target_os = "macos"))]
fn important_available(_: &std::path::Path) -> Option<u64> {
    None
}

/// Lists snapshots of the data volume (and the system volume, for other apps' snapshots).
fn list_apfs() -> Vec<Snapshot> {
    let mut all: Vec<Snapshot> = vec![];
    for vol in ["/System/Volumes/Data", "/"] {
        let Some(out) = run_capture(&["diskutil", "apfs", "listSnapshots", "-plist", vol], 15) else { continue };
        for s in parse_diskutil_plist(out.as_bytes()) {
            if !all.iter().any(|x| x.name == s.name) {
                all.push(s);
            }
        }
    }
    all
}

/// Items for the `snapshots` provider. Sizes of single snapshots are not knowable cheaply,
/// so they carry `size_unknown`; the space they hold is part of the purgeable item.
pub fn snapshots(rule: &Rule) -> Vec<Raw> {
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    let min_age = rule.param_u64("min_age_hours").unwrap_or(24).max(24) as i64 * 3600;
    let tm: Vec<String> = run_capture(&["tmutil", "listlocalsnapshots", "/"], 15).map(|o| parse_tmutil(&o)).unwrap_or_default();
    let apfs = list_apfs();
    let purgeable = if time_machine_only(rule) { None } else { purgeable_bytes() };
    build_items(rule, &tm, &apfs, purgeable, now(), min_age)
}

/// `params.only = "time-machine"`: just the removable Time Machine snapshots (the rule for
/// those sits in a cleanable bucket); otherwise the awareness items.
fn time_machine_only(rule: &Rule) -> bool {
    rule.param_str("only").as_deref() == Some("time-machine")
}

/// The item list, separated from the commands so it can be tested with fixtures.
pub fn build_items(rule: &Rule, tm: &[String], apfs: &[Snapshot], purgeable: Option<u64>, at: i64, min_age: i64) -> Vec<Raw> {
    let mut out = vec![];
    let mut tm_names: Vec<String> = tm.to_vec();
    for s in apfs {
        if s.name.starts_with("com.apple.TimeMachine.") && !tm_names.contains(&s.name) {
            tm_names.push(s.name.clone());
        }
    }
    let mut others: std::collections::BTreeMap<&'static str, Vec<String>> = Default::default();
    for s in apfs {
        if let Some(owner) = snapshot_owner(&s.name) {
            others.entry(owner).or_default().push(s.name.clone());
        }
    }
    let tm_recent = tm_names.iter().filter(|n| parse_snapshot_date(n).map_or(true, |t| at - t < min_age)).count();
    let tm_only = time_machine_only(rule);

    if let Some(p) = purgeable.filter(|p| *p > 0 && !tm_only) {
        let mut raw = Raw {
            path: PathBuf::from("purgeable://data"),
            name: "Purgeable space".into(),
            rule: rule.id.clone(),
            is_virtual: true,
            sized: Some((Sized { bytes: p, ..Default::default() }, vec![])),
            tier: Some(Tier::Review),
            cost: Some(Cost::Data),
            action: Some(ActionKind::None),
            report_only: true,
            ..Default::default()
        };
        raw.reasons.push("space macOS reclaims on its own when it needs room: local snapshots, iCloud-optimized files, system caches".into());
        if !tm_names.is_empty() {
            raw.reasons.push(format!("{} local Time Machine snapshot{}", tm_names.len(), if tm_names.len() == 1 { "" } else { "s" }));
        }
        for (owner, list) in &others {
            raw.reasons.push(format!("{} snapshot{} from {}", list.len(), if list.len() == 1 { "" } else { "s" }, owner));
        }
        raw.meta.insert("time_machine_snapshots".into(), Value::from(tm_names.len()));
        raw.meta.insert("time_machine_recent".into(), Value::from(tm_recent));
        raw.meta.insert("recommend".into(), Value::from("Nothing to do: macOS frees this when it needs the room. Finder and About This Mac count it as available."));
        out.push(raw);
    }

    for name in tm_names.iter().filter(|_| tm_only) {
        let (Some(date), Some(t)) = (snapshot_date(name), parse_snapshot_date(name)) else { continue };
        if at - t < min_age {
            continue;
        }
        let mut raw = Raw {
            path: PathBuf::from(format!("tmsnapshot://{}", date)),
            name: format!("Time Machine local snapshot {} {}:{}", &date[..10], &date[11..13], &date[13..15]),
            rule: rule.id.clone(),
            is_virtual: true,
            last_used: Some(t),
            sized: Some((Sized { newest: t, ..Default::default() }, vec![])),
            tier: Some(Tier::Review),
            cost: Some(Cost::Data),
            action: Some(ActionKind::Command),
            command: Some(vec!["tmutil".into(), "deletelocalsnapshots".into(), date.to_string()]),
            ..Default::default()
        };
        raw.reasons.push(format!("local Time Machine snapshot, {}", crate::util::age(t)));
        raw.reasons.push("macOS removes these on its own when space runs low; your backup disk keeps its copy".into());
        raw.meta.insert("snapshot".into(), Value::from(name.clone()));
        raw.meta.insert("size_unknown".into(), Value::Bool(true));
        raw.meta.insert("measure".into(), Value::from("purgeable"));
        out.push(raw);
    }

    for (owner, list) in others.into_iter().filter(|_| !tm_only) {
        let mut raw = Raw {
            path: PathBuf::from(format!("snapshot://{}", owner.to_lowercase().replace(' ', "-"))),
            name: format!("Snapshots made by {} ({})", owner, list.len()),
            rule: rule.id.clone(),
            is_virtual: true,
            sized: Some((Sized::default(), vec![])),
            tier: Some(Tier::Review),
            cost: Some(Cost::Data),
            action: Some(ActionKind::None),
            report_only: true,
            ..Default::default()
        };
        raw.reasons.push(format!("APFS snapshots {} keeps; they count toward purgeable space", owner));
        raw.meta.insert("snapshots".into(), Value::from(list));
        raw.meta.insert("size_unknown".into(), Value::Bool(true));
        raw.meta.insert(
            "recommend".into(),
            Value::from(if owner == "another app" {
                "Manage them in the app that made them".to_string()
            } else {
                format!("Manage them in {}", owner)
            }),
        );
        out.push(raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dates_and_owners() {
        let t = parse_snapshot_date("com.apple.TimeMachine.2026-11-14-120000.local").unwrap();
        assert_eq!(parse_snapshot_date("2026-11-14-120000"), Some(t));
        assert!(parse_snapshot_date("/").is_none());
        assert!(parse_snapshot_date("2026-13-14-120000").is_none());
        assert_eq!(snapshot_date("com.apple.TimeMachine.2026-11-14-120000.local"), Some("2026-11-14-120000"));
        assert_eq!(snapshot_owner("com.bombich.ccc.12345"), Some("Carbon Copy Cloner"));
        assert_eq!(snapshot_owner("com.apple.os.update-ABC"), None);
        assert_eq!(snapshot_owner("com.apple.TimeMachine.2026-11-14-120000.local"), None);
        assert_eq!(parse_tmutil("Snapshots for disk /:\ncom.apple.TimeMachine.2026-11-14-120000.local\n"), vec!["com.apple.TimeMachine.2026-11-14-120000.local"]);
    }
}
