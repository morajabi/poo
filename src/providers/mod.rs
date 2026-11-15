//! Providers turn tools and well-known layouts into items. Rules pick which
//! provider runs, with which parameters, and in which bucket results land.

pub mod git;
pub mod snapshots;

use crate::catalog::Rule;
use crate::fsutil::Sized;
use crate::model::{ActionKind, Child, Cost, Tier};
use crate::util::{expand, home, run_capture, which};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// An item before classification. Saved in the scan index for incremental rescans.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Raw {
    pub path: PathBuf,
    pub name: String,
    pub rule: String,
    pub sized: Option<(Sized, Vec<Child>)>,
    pub last_used: Option<i64>,
    pub project: Option<PathBuf>,
    pub repo: Option<PathBuf>,
    pub agent: Option<String>,
    pub tier: Option<Tier>,
    pub cost: Option<Cost>,
    pub reasons: Vec<String>,
    pub orphan: bool,
    pub action: Option<ActionKind>,
    pub command: Option<Vec<String>>,
    pub meta: BTreeMap<String, Value>,
    /// Not a filesystem path (docker://, simruntime://).
    pub is_virtual: bool,
    /// Bytes counted elsewhere but freed together with this item.
    pub nested_bytes: u64,
    #[serde(skip)]
    pub exclude: HashSet<PathBuf>,
    /// Where it came from: walk, ignored, global, provider, slow, worktree.
    #[serde(default)]
    pub origin: String,
    /// Sized without its claimed descendants, so nested items are not double counted.
    pub disjoint: bool,
    pub risks: Vec<crate::model::Risk>,
    /// Shown and sized, never cleaned by poo (its owner manages it).
    #[serde(default)]
    pub report_only: bool,
}

pub fn parse_iso(s: &str) -> Option<i64> {
    // 2026-11-15T13:55:13Z or with fractional seconds / offset (treated as UTC).
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let se: i64 = s.get(17..19)?.parse().ok()?;
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

/// Parses docker-style sizes: `1.2GB`, `512MB`, `13.5kB`, `0B`.
pub fn parse_docker_size(s: &str) -> u64 {
    let s = s.split_whitespace().next().unwrap_or("").trim();
    let idx = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let n: f64 = s[..idx].parse().unwrap_or(0.0);
    let mult = match s[idx..].to_ascii_uppercase().as_str() {
        "B" | "" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KIB" => 1024.0,
        "MIB" => 1024.0 * 1024.0,
        "GIB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (n * mult) as u64
}

// ---------------------------------------------------------------------------
// Xcode DerivedData
// ---------------------------------------------------------------------------

pub fn xcode_derived_data(rule: &Rule) -> Vec<Raw> {
    let root = expand(&rule.param_str("path").unwrap_or_else(|| "~/Library/Developer/Xcode/DerivedData".into()));
    let skip: Vec<String> = rule.param_list("skip");
    let Ok(rd) = std::fs::read_dir(&root) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if skip.iter().any(|s| s == &name) || name.starts_with('.') || !e.path().is_dir() {
            continue;
        }
        let mut raw = Raw {
            path: e.path(),
            rule: rule.id.clone(),
            name: name.split('-').next().unwrap_or(&name).to_string(),
            ..Default::default()
        };
        let info = e.path().join("info.plist");
        if let Ok(v) = plist::Value::from_file(&info) {
            let dict = v.as_dictionary();
            if let Some(ws) = dict.and_then(|d| d.get("WorkspacePath")).and_then(|v| v.as_string()) {
                let ws_path = PathBuf::from(ws);
                raw.meta.insert("workspace".into(), Value::String(ws.to_string()));
                if !ws_path.exists() {
                    raw.orphan = true;
                    raw.reasons.push("its Xcode project no longer exists".into());
                }
                raw.project = Some(ws_path.parent().map(|p| p.to_path_buf()).unwrap_or(ws_path.clone()));
                let file = ws_path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                raw.name = format!("{} · {}", file, crate::util::tilde(ws_path.parent().unwrap_or(&ws_path)));
            }
            if let Some(d) = dict.and_then(|d| d.get("LastAccessedDate")).and_then(|v| v.as_date()) {
                let t: std::time::SystemTime = d.into();
                raw.last_used = Some(crate::util::system_time_secs(t));
            }
        } else {
            raw.meta.insert("stub".into(), Value::Bool(true));
        }
        out.push(raw);
    }
    out
}

// ---------------------------------------------------------------------------
// Versioned installs: keep the active one and the newest N
// ---------------------------------------------------------------------------

/// Old versions in one or more folders. `params.path` is a folder or a list of folders (globs
/// allowed); each folder keeps its own newest `keep` entries plus any version an `active`
/// symlink or `active_command` points into. `params.sort` orders entries: `mtime` (default)
/// or `version`, which compares the numbers in the name ("17.5.1 (21F90)", "1.82.0") and
/// keeps everything in a folder where any name has no number.
pub fn versions(rule: &Rule) -> Vec<Raw> {
    let mut dirs: Vec<String> = rule.param_list("path");
    if let Some(p) = rule.param_str("path") {
        dirs.push(p);
    }
    let mut out = vec![];
    for d in dirs {
        let pattern = expand(&d);
        let matched = if crate::fsutil::has_glob(&d) { crate::fsutil::expand_glob(&pattern) } else { vec![pattern] };
        for dir in matched {
            out.extend(versions_in(rule, &dir));
        }
    }
    out
}

/// Numbers in a version-like name, for ordering: "iPhone15,2 17.5.1 (21F90)" sorts by
/// 17.5.1 and then the build 21F90; "1.82.0-aarch64" by 1.82.0.
pub fn version_key(name: &str) -> Option<(Vec<u64>, String)> {
    // Skip a leading device model ("iPhone15,2 ") so the OS version decides.
    let rest = match name.split_once(' ') {
        Some((first, r)) if first.contains(',') => r,
        _ => name,
    };
    let start = rest.find(|c: char| c.is_ascii_digit())?;
    let tail = &rest[start..];
    let end = tail.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(tail.len());
    let nums: Vec<u64> = tail[..end].split('.').filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    if nums.is_empty() {
        return None;
    }
    let build = rest.split_once('(').and_then(|(_, b)| b.split(')').next()).unwrap_or("").to_string();
    Some((nums, build))
}

fn versions_in(rule: &Rule, dir: &Path) -> Vec<Raw> {
    let keep = rule.param_u64("keep").unwrap_or(1) as usize;
    let by_version = rule.param_str("sort").as_deref() == Some("version");
    let pattern = rule.param_str("pattern").and_then(|p| globset::Glob::new(&p).ok()).map(|g| g.compile_matcher());
    // The active version: entries any `active` symlink points into, or the path printed by
    // `active_command` (for example `xcode-select -p`).
    let mut targets: Vec<PathBuf> = rule
        .param_list("active")
        .iter()
        .filter_map(|l| std::fs::canonicalize(expand(l)).ok())
        .collect();
    let cmd = rule.param_list("active_command");
    if !cmd.is_empty() {
        let refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
        if let Some(out) = run_capture(&refs, 10) {
            if let Ok(p) = std::fs::canonicalize(out.trim()) {
                targets.push(p);
            }
        }
    }
    let wants_active = !rule.param_list("active").is_empty() || !cmd.is_empty();
    let mut entries: Vec<(PathBuf, i64)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter(|e| pattern.as_ref().map_or(true, |m| m.is_match(e.file_name().to_string_lossy().as_ref())))
                .map(|e| (e.path(), crate::fsutil::mtime(&e.path()).unwrap_or(0)))
                .collect()
        })
        .unwrap_or_default();
    let active: HashSet<PathBuf> = entries
        .iter()
        .filter(|(p, _)| {
            let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            targets.iter().any(|t| t.starts_with(&c))
        })
        .map(|(p, _)| p.clone())
        .collect();
    if wants_active && active.is_empty() {
        // Could not tell which version is in use: keep everything.
        return vec![];
    }
    if by_version {
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if entries.iter().any(|(p, _)| version_key(&name(p)).is_none()) {
            // A name we cannot order: keep the whole folder.
            return vec![];
        }
        entries.sort_by(|a, b| version_key(&name(&b.0)).cmp(&version_key(&name(&a.0))).then(b.1.cmp(&a.1)));
    } else {
        entries.sort_by(|a, b| b.1.cmp(&a.1));
    }
    let mut kept = 0;
    let mut out = vec![];
    for (p, t) in entries {
        if active.contains(&p) {
            continue;
        }
        if kept < keep {
            kept += 1;
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut raw = Raw { path: p, name, rule: rule.id.clone(), last_used: Some(t), ..Default::default() };
        raw.reasons.push(if by_version { "older version, newer ones kept".into() } else { "superseded version".into() });
        out.push(raw);
    }
    out
}

// ---------------------------------------------------------------------------
// Leftovers of uninstalled apps
// ---------------------------------------------------------------------------

/// Bundle ids of installed apps, including helpers and extensions inside them.
pub fn installed_bundle_ids() -> HashSet<String> {
    let mut ids = HashSet::new();
    let roots = [
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
        PathBuf::from("/Applications/Setapp"),
        home().join("Applications"),
        home().join("Applications/Chrome Apps.localized"),
    ];
    fn add_app(app: &Path, ids: &mut HashSet<String>, depth: usize) {
        if let Ok(v) = plist::Value::from_file(app.join("Contents/Info.plist")) {
            if let Some(id) = v.as_dictionary().and_then(|d| d.get("CFBundleIdentifier")).and_then(|v| v.as_string()) {
                ids.insert(id.to_lowercase());
            }
        }
        if depth == 0 {
            return;
        }
        for sub in ["Contents/Library/LoginItems", "Contents/Library/LaunchServices", "Contents/PlugIns", "Contents/Frameworks", "Contents/Helpers", "Contents/Library/SystemExtensions", "Contents/XPCServices"] {
            if let Ok(rd) = std::fs::read_dir(app.join(sub)) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.ends_with(".app") || n.ends_with(".appex") || n.ends_with(".xpc") || n.ends_with(".systemextension") {
                        add_app(&e.path(), ids, depth - 1);
                    }
                }
            }
        }
    }
    for r in roots {
        if let Ok(rd) = std::fs::read_dir(&r) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".app") {
                    add_app(&e.path(), &mut ids, 2);
                    // iPad and iPhone apps on Apple silicon wrap the real bundle.
                    if let Ok(w) = std::fs::read_dir(e.path().join("Wrapper")) {
                        for inner in w.flatten() {
                            if inner.file_name().to_string_lossy().ends_with(".app") {
                                if let Ok(v) = plist::Value::from_file(inner.path().join("Info.plist")) {
                                    if let Some(id) = v.as_dictionary().and_then(|d| d.get("CFBundleIdentifier")).and_then(|v| v.as_string()) {
                                        ids.insert(id.to_lowercase());
                                    }
                                }
                            }
                        }
                    }
                } else if e.path().is_dir() && !n.starts_with('.') {
                    // Vendor folders (/Applications/Adobe ..., Setapp) hold apps one level down.
                    if let Ok(sub) = std::fs::read_dir(e.path()) {
                        for s in sub.flatten() {
                            if s.file_name().to_string_lossy().ends_with(".app") {
                                add_app(&s.path(), &mut ids, 2);
                            }
                        }
                    }
                }
            }
        }
    }
    // Homebrew cask apps sometimes live elsewhere; their caches use the cask token.
    if let Ok(rd) = std::fs::read_dir("/opt/homebrew/Caskroom") {
        for e in rd.flatten() {
            ids.insert(e.file_name().to_string_lossy().to_lowercase());
        }
    }
    ids
}

fn looks_like_bundle_id(name: &str) -> bool {
    let n = name.to_lowercase();
    let prefixes = ["com.", "org.", "net.", "io.", "app.", "dev.", "co.", "me.", "ai.", "so.", "us.", "tv.", "de.", "fr.", "jp.", "cn."];
    n.matches('.').count() >= 2 && prefixes.iter().any(|p| n.starts_with(p)) && !n.starts_with("com.apple.")
}

pub fn app_leftovers(rule: &Rule, installed: &HashSet<String>) -> Vec<Raw> {
    let mut out = vec![];
    let running: Vec<String> = crate::procs::list().into_iter().map(|p| p.name.to_lowercase()).collect();
    for loc in rule.param_list("locations") {
        let dir = expand(&loc);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let raw_name = e.file_name().to_string_lossy().to_string();
            let name = raw_name.trim_end_matches(".savedState").trim_end_matches(".plist").to_string();
            if !looks_like_bundle_id(&name) || rule.exclude.matches(&raw_name) {
                continue;
            }
            let lname = name.to_lowercase();
            // Installed if any installed id equals it, extends it, or it extends an installed id,
            // or another app from the same vendor (com.vendor.*) is installed: vendors share
            // helper and cache ids between their apps.
            let vendor: String = lname.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
            let present = installed.iter().any(|id| {
                lname == *id
                    || lname.starts_with(&format!("{}.", id))
                    || id.starts_with(&format!("{}.", lname))
                    || id.starts_with(&format!("{}.", vendor))
            });
            if present {
                continue;
            }
            // Last check: Spotlight knows apps anywhere (including ones you build in Xcode).
            if let Some(out) = run_capture(&["mdfind", &format!("kMDItemCFBundleIdentifier == '{}'", name)], 5) {
                if !out.trim().is_empty() {
                    continue;
                }
            }
            // A process whose name matches the id's last part means the app is around.
            let last = lname.rsplit('.').next().unwrap_or("");
            if last.len() > 3 && running.iter().any(|r| r == last) {
                continue;
            }
            let mut raw = Raw {
                path: e.path(),
                name: format!("{} ({})", name, dir.file_name().map(|d| d.to_string_lossy().to_string()).unwrap_or_default()),
                rule: rule.id.clone(),
                ..Default::default()
            };
            raw.reasons.push("no installed app has this bundle id".into());
            out.push(raw);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Simulators
// ---------------------------------------------------------------------------

fn runtime_label(key: &str) -> String {
    let tail = key.rsplit('.').next().unwrap_or(key);
    let mut parts = tail.splitn(2, '-');
    let os = parts.next().unwrap_or("");
    let ver = parts.next().unwrap_or("").replace('-', ".");
    format!("{} {}", os, ver).trim().to_string()
}

pub fn simctl_devices() -> Option<Value> {
    if which("xcrun").is_none() {
        return None;
    }
    let out = run_capture(&["xcrun", "simctl", "list", "devices", "-j"], 30)?;
    serde_json::from_str(&out).ok()
}

pub fn simulator_devices(rule: &Rule, devices: Option<&Value>) -> Vec<Raw> {
    let Some(map) = devices.and_then(|v| v.get("devices")).and_then(|v| v.as_object()) else { return vec![] };
    let mut out = vec![];
    for (runtime, list) in map {
        let Some(list) = list.as_array() else { continue };
        for d in list {
            let udid = d.get("udid").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let data = d.get("dataPath").and_then(|v| v.as_str()).unwrap_or_default();
            if udid.is_empty() || data.is_empty() {
                continue;
            }
            let dir = PathBuf::from(data).parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("Simulator");
            let available = d.get("isAvailable").and_then(|v| v.as_bool()).unwrap_or(true);
            let state = d.get("state").and_then(|v| v.as_str()).unwrap_or("Shutdown");
            let last = d.get("lastUsedAt").or_else(|| d.get("lastBootedAt")).and_then(|v| v.as_str()).and_then(parse_iso);
            let mut raw = Raw {
                path: dir.clone(),
                name: format!("{} ({})", name, runtime_label(runtime)),
                rule: rule.id.clone(),
                last_used: last,
                command: Some(vec!["xcrun".into(), "simctl".into(), "delete".into(), udid.clone()]),
                action: Some(ActionKind::Command),
                ..Default::default()
            };
            if let Some(size) = d.get("dataPathSize").and_then(|v| v.as_u64()) {
                let logs = d.get("logPathSize").and_then(|v| v.as_u64()).unwrap_or(0);
                raw.sized = Some((Sized::of(size + logs, 0, last.unwrap_or(0)), vec![]));
            }
            raw.meta.insert("udid".into(), Value::String(udid));
            raw.meta.insert("runtime".into(), Value::String(runtime.clone()));
            raw.meta.insert("state".into(), Value::String(state.into()));
            if !available {
                raw.tier = Some(Tier::Junk);
                raw.cost = Some(Cost::None);
                raw.reasons.push("its runtime is no longer installed".into());
            } else if state == "Booted" {
                raw.tier = Some(Tier::Hot);
                raw.reasons.push("simulator is running".into());
            } else if last.is_none() {
                raw.reasons.push("never booted".into());
                raw.orphan = true;
            }
            out.push(raw);
        }
    }
    out
}

pub fn simulator_runtimes(rule: &Rule, devices: Option<&Value>) -> Vec<Raw> {
    if which("xcrun").is_none() {
        return vec![];
    }
    let Some(out) = run_capture(&["xcrun", "simctl", "runtime", "list", "-j"], 30) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&out) else { return vec![] };
    let used: HashSet<String> = devices
        .and_then(|v| v.get("devices"))
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter(|(_, l)| l.as_array().map_or(false, |a| !a.is_empty()))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut list = vec![];
    for (id, r) in v.as_object().into_iter().flatten() {
        let rid = r.get("runtimeIdentifier").and_then(|v| v.as_str()).unwrap_or_default();
        let version = r.get("version").and_then(|v| v.as_str()).unwrap_or_default();
        let bytes = r.get("sizeBytes").and_then(|v| v.as_u64()).unwrap_or(0);
        let deletable = r.get("deletable").and_then(|v| v.as_bool()).unwrap_or(false);
        let platform = r.get("platformIdentifier").and_then(|v| v.as_str()).unwrap_or_default();
        let os = if platform.contains("iphone") {
            "iOS"
        } else if platform.contains("watch") {
            "watchOS"
        } else if platform.contains("appletv") {
            "tvOS"
        } else if platform.contains("xr") {
            "visionOS"
        } else {
            "Simulator"
        };
        let mut raw = Raw {
            path: PathBuf::from(format!("simruntime://{}", id)),
            name: format!("{} {} runtime ({})", os, version, r.get("build").and_then(|v| v.as_str()).unwrap_or("")),
            rule: rule.id.clone(),
            is_virtual: true,
            last_used: r.get("lastUsedAt").and_then(|v| v.as_str()).and_then(parse_iso),
            sized: Some((Sized::of(bytes, 0, 0), vec![])),
            command: Some(vec!["xcrun".into(), "simctl".into(), "runtime".into(), "delete".into(), id.clone()]),
            action: Some(ActionKind::Command),
            ..Default::default()
        };
        raw.meta.insert("runtime".into(), Value::String(rid.to_string()));
        if !deletable {
            raw.tier = Some(Tier::Protected);
            raw.reasons.push("macOS reports this runtime as not deletable".into());
        } else if !used.contains(rid) {
            raw.orphan = true;
            raw.reasons.push("no simulator uses this runtime".into());
        }
        list.push(raw);
    }
    list
}

// ---------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------

pub fn docker(rule: &Rule) -> Vec<Raw> {
    if which("docker").is_none() {
        return vec![];
    }
    let Some(out) = run_capture(&["docker", "system", "df", "--format", "{{json .}}"], 15) else { return vec![] };
    let mut list = vec![];
    for line in out.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let ty = v.get("Type").and_then(|v| v.as_str()).unwrap_or_default();
        let reclaim = parse_docker_size(v.get("Reclaimable").and_then(|v| v.as_str()).unwrap_or("0B"));
        let total = parse_docker_size(v.get("Size").and_then(|v| v.as_str()).unwrap_or("0B"));
        let (name, tier, cost, cmd): (&str, Tier, Cost, Vec<&str>) = match ty {
            "Build Cache" => ("Docker build cache", Tier::Cold, Cost::Rebuild, vec!["docker", "builder", "prune", "-f", "--filter", "until=168h"]),
            "Images" => ("Unused Docker images", Tier::Warm, Cost::Redownload, vec!["docker", "image", "prune", "-a", "-f"]),
            "Containers" => ("Stopped Docker containers", Tier::Review, Cost::Data, vec!["docker", "container", "prune", "-f"]),
            "Local Volumes" => ("Unused Docker volumes", Tier::Protected, Cost::Data, vec![]),
            _ => continue,
        };
        if reclaim == 0 {
            continue;
        }
        let mut raw = Raw {
            path: PathBuf::from(format!("docker://{}", ty.to_lowercase().replace(' ', "-"))),
            name: name.into(),
            rule: rule.id.clone(),
            is_virtual: true,
            sized: Some((Sized::of(reclaim, 0, 0), vec![])),
            tier: Some(tier),
            cost: Some(cost),
            ..Default::default()
        };
        raw.meta.insert("docker_total".into(), Value::from(total));
        if ty == "Build Cache" {
            raw.reasons.push("keeps build cache used in the last week".into());
        }
        if cmd.is_empty() {
            raw.action = Some(ActionKind::None);
            raw.reasons.push("volumes hold data; remove them yourself with docker volume rm".into());
        } else {
            raw.action = Some(ActionKind::Command);
            raw.command = Some(cmd.into_iter().map(String::from).collect());
        }
        list.push(raw);
    }
    list
}

// ---------------------------------------------------------------------------
// Homebrew
// ---------------------------------------------------------------------------

pub fn homebrew(rule: &Rule) -> Vec<Raw> {
    if which("brew").is_none() {
        return vec![];
    }
    let Some(out) = run_capture(&["brew", "cleanup", "-n", "--prune=all"], 90) else { return vec![] };
    let cache = home().join("Library/Caches/Homebrew");
    let mut bytes = 0u64;
    let mut count = 0;
    for line in out.lines() {
        let Some(rest) = line.strip_prefix("Would remove: ") else { continue };
        let path = rest.split(" (").next().unwrap_or_default();
        if Path::new(path).starts_with(&cache) {
            continue;
        }
        if let Some(sz) = rest.rsplit(", ").next().map(|s| s.trim_end_matches(')')) {
            bytes += parse_docker_size(sz);
            count += 1;
        }
    }
    if bytes == 0 {
        return vec![];
    }
    let mut raw = Raw {
        path: PathBuf::from("brew://cleanup"),
        name: format!("Old Homebrew versions ({} entries)", count),
        rule: rule.id.clone(),
        is_virtual: true,
        sized: Some((Sized::of(bytes, count, 0), vec![])),
        action: Some(ActionKind::Command),
        command: Some(vec!["brew".into(), "cleanup".into(), "--prune=all".into()]),
        ..Default::default()
    };
    raw.reasons.push("superseded formula and cask versions".into());
    vec![raw]
}

// ---------------------------------------------------------------------------
// Exec providers written by mods
// ---------------------------------------------------------------------------

pub fn exec(rule: &Rule) -> Result<Vec<Raw>, String> {
    let cmd = rule.param_list("command");
    if cmd.is_empty() {
        return Err("exec provider needs params.command".into());
    }
    let mut argv: Vec<String> = cmd.clone();
    if let Some(dir) = &rule.mod_dir {
        let first = dir.join(&argv[0]);
        if first.exists() {
            argv[0] = first.to_string_lossy().to_string();
        }
    }
    let timeout = rule.param_u64("timeout").unwrap_or(60);
    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let out = run_capture(&refs, timeout).ok_or_else(|| format!("`{}` failed or timed out", argv.join(" ")))?;
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("invalid JSON from `{}`: {}", argv[0], e))?;
    let mut list = vec![];
    for it in v.get("items").and_then(|v| v.as_array()).into_iter().flatten() {
        let Some(path) = it.get("path").and_then(|v| v.as_str()) else { continue };
        let mut raw = Raw {
            path: expand(path),
            name: it.get("name").and_then(|v| v.as_str()).unwrap_or(path).to_string(),
            rule: rule.id.clone(),
            last_used: it.get("last_used").and_then(|v| v.as_i64()),
            project: it.get("project").and_then(|v| v.as_str()).map(expand),
            ..Default::default()
        };
        if let Some(b) = it.get("bytes").and_then(|v| v.as_u64()) {
            raw.sized = Some((Sized::of(b, 0, raw.last_used.unwrap_or(0)), vec![]));
        }
        if let Some(c) = it.get("cost").and_then(|v| v.as_str()) {
            raw.cost = serde_json::from_value(Value::String(c.into())).ok();
        }
        if let Some(t) = it.get("tier").and_then(|v| v.as_str()) {
            raw.tier = Tier::parse(t);
        }
        if let Some(r) = it.get("reasons").and_then(|v| v.as_array()) {
            raw.reasons = r.iter().filter_map(|x| x.as_str().map(String::from)).collect();
        }
        if let Some(c) = it.get("command").and_then(|v| v.as_array()) {
            raw.command = Some(c.iter().filter_map(|x| x.as_str().map(String::from)).collect());
            raw.action = Some(ActionKind::Command);
        }
        raw.is_virtual = path.contains("://");
        list.push(raw);
    }
    Ok(list)
}
