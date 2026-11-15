//! Routine cleanups. One launchd agent wakes poo every 15 minutes; poo runs whichever
//! `[[schedule]]` entries are due. Scheduled runs rescan first and only ever clean in
//! standard mode: no forced or dangerous items without a person.

use crate::catalog::Catalog;
use crate::clean::{self, CleanOptions};
use crate::config::{state_dir, Config, Schedule};
use crate::model::RiskLevel;
use crate::ops::Ctx;
use crate::plan::Filter;
use crate::scan::ScanOptions;
use crate::util::{bytes, now, parse_bytes};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

pub const LABEL: &str = "com.morajabi.poo.scheduler";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunRecord {
    pub at: i64,
    pub status: String,
    pub freed_bytes: u64,
    pub cleaned: usize,
    pub skipped: usize,
    pub failed: usize,
    pub free_after: u64,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleState {
    #[serde(default)]
    pub last_run: BTreeMap<String, RunRecord>,
    #[serde(default)]
    pub last_tick: i64,
}

fn state_path() -> PathBuf {
    state_dir().join("schedules.json")
}

pub fn load_state() -> ScheduleState {
    std::fs::read(state_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn save_state(s: &ScheduleState) {
    let p = state_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(p, serde_json::to_vec_pretty(s).unwrap_or_default());
}

pub fn parse_every(s: &str) -> Result<i64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).ok_or_else(|| anyhow!("`{}`: use 30m, 6h, 1d or 7d", s))?);
    let n: i64 = num.trim().parse().map_err(|_| anyhow!("`{}`: use 30m, 6h, 1d or 7d", s))?;
    let secs = match unit {
        "m" | "min" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        "w" => n * 7 * 86400,
        _ => bail!("`{}`: use 30m, 6h, 1d or 7d", s),
    };
    if secs < 15 * 60 {
        bail!("schedules run at most every 15 minutes");
    }
    Ok(secs)
}

/// "50G" or "15%" of the disk, in bytes.
pub fn parse_amount(s: &str, disk_total: u64) -> Result<u64> {
    let t = s.trim();
    if let Some(p) = t.strip_suffix('%') {
        let pct: f64 = p.trim().parse().map_err(|_| anyhow!("bad percentage `{}`", s))?;
        return Ok((disk_total as f64 * pct / 100.0) as u64);
    }
    parse_bytes(t).ok_or_else(|| anyhow!("bad size `{}`", s))
}

fn parse_at(s: &str) -> Result<(i64, i64)> {
    let (h, m) = s.split_once(':').ok_or_else(|| anyhow!("`{}`: use HH:MM", s))?;
    let h: i64 = h.parse().map_err(|_| anyhow!("`{}`: use HH:MM", s))?;
    let m: i64 = m.parse().map_err(|_| anyhow!("`{}`: use HH:MM", s))?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        bail!("`{}`: use HH:MM", s);
    }
    Ok((h, m))
}

/// Unix time of today's HH:MM in local time.
fn local_today_at(h: i64, m: i64) -> i64 {
    let t = now() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm.tm_hour = h as i32;
    tm.tm_min = m as i32;
    tm.tm_sec = 0;
    unsafe { libc::mktime(&mut tm) as i64 }
}

pub fn validate(s: &Schedule, cat: &Catalog) -> Result<()> {
    if s.id.is_empty() || !s.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("schedule ids use letters, digits, - and _");
    }
    if cat.plan(&s.plan).is_none() {
        bail!("unknown plan `{}`; plans: {}", s.plan, cat.plans.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", "));
    }
    parse_every(&s.every)?;
    if let Some(a) = &s.at {
        parse_at(a)?;
    }
    for v in [&s.when_free_below, &s.target_free, &s.max_bytes].into_iter().flatten() {
        parse_amount(v, 1_000_000_000_000)?;
    }
    Ok(())
}

/// When a schedule should run next (unix time).
pub fn next_run(s: &Schedule, last: Option<i64>) -> i64 {
    let every = parse_every(&s.every).unwrap_or(86400);
    let base = last.map(|l| l + every).unwrap_or_else(now);
    if let Some((h, m)) = s.at.as_deref().and_then(|a| parse_at(a).ok()) {
        let mut t = local_today_at(h, m);
        let earliest = last.map(|l| l + every - 3600).unwrap_or(0);
        while t < earliest.max(now() - 60) {
            t += 86400;
        }
        return t;
    }
    base
}

pub fn presets() -> Vec<(&'static str, &'static str, Schedule)> {
    let mk = |id: &str, plan: &str, every: &str, at: Option<&str>, below: Option<&str>, target: Option<&str>| Schedule {
        id: id.into(),
        plan: plan.into(),
        every: every.into(),
        at: at.map(String::from),
        when_free_below: below.map(String::from),
        target_free: target.map(String::from),
        max_bytes: None,
        buckets: vec![],
        exclude_buckets: vec![],
        tags: vec![],
        exclude_tags: vec![],
        enabled: true,
        notify: true,
    };
    vec![
        ("nightly", "Every night at 03:30, clean everything nothing is using (plan `stale`).", mk("nightly", "stale", "1d", Some("03:30"), None, None)),
        ("quick", "Every 6 hours, clean junk only: logs, temp, crash reports.", mk("quick", "quick", "6h", None, None, None)),
        ("guard", "Low-disk guard: every 15 minutes, if free space drops below 20 GB, clean what is not in use until 50 GB are free.", mk("guard", "emergency", "15m", None, Some("20G"), Some("50G"))),
        ("weekly", "Every Sunday-ish, a weekly `stale` run for machines that sleep at night.", mk("weekly", "stale", "7d", Some("12:00"), None, None)),
    ]
}

fn plist_path() -> PathBuf {
    crate::util::home().join("Library/LaunchAgents").join(format!("{}.plist", LABEL))
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn agent_installed() -> bool {
    plist_path().exists()
}

pub fn agent_loaded() -> bool {
    crate::util::run_capture(&["launchctl", "print", &format!("gui/{}/{}", uid(), LABEL)], 5).is_some()
}

/// Installs (or refreshes) the launchd agent that runs `poo schedule tick`.
pub fn install_agent() -> Result<PathBuf> {
    if crate::clean::read_only() {
        bail!("POO_READ_ONLY set: the launchd agent was not installed");
    }
    let bin = std::fs::canonicalize(std::env::current_exe()?)?;
    let log = state_dir().join("scheduler.log");
    std::fs::create_dir_all(state_dir())?;
    let path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin", path);
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut env = format!("<key>PATH</key><string>{}</string><key>HOME</key><string>{}</string>", esc(&path), esc(&crate::util::home().to_string_lossy()));
    for var in ["POO_HOME", "POO_CONFIG_DIR", "POO_STATE_DIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME"] {
        if let Ok(v) = std::env::var(var) {
            env.push_str(&format!("<key>{}</key><string>{}</string>", var, esc(&v)));
        }
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array><string>{bin}</string><string>schedule</string><string>tick</string></array>
  <key>StartInterval</key><integer>900</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>EnvironmentVariables</key><dict>{env}</dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        label = LABEL,
        bin = esc(&bin.to_string_lossy()),
        env = env,
        log = esc(&log.to_string_lossy()),
    );
    let p = plist_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, plist)?;
    let domain = format!("gui/{}", uid());
    let _ = std::process::Command::new("launchctl").args(["bootout", &format!("{}/{}", domain, LABEL)]).output();
    let out = std::process::Command::new("launchctl").args(["bootstrap", &domain, &p.to_string_lossy()]).output()?;
    if !out.status.success() {
        bail!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(p)
}

pub fn uninstall_agent() -> Result<()> {
    if crate::clean::read_only() {
        bail!("POO_READ_ONLY set: the launchd agent was not touched");
    }
    let _ = std::process::Command::new("launchctl").args(["bootout", &format!("gui/{}/{}", uid(), LABEL)]).output();
    let p = plist_path();
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// Keeps the launchd agent in step with config: installed while any schedule is enabled.
pub fn sync_agent(cfg: &Config) -> Result<Option<String>> {
    if crate::clean::read_only() {
        return Ok(Some("POO_READ_ONLY set; launchd agent not touched (schedules saved to config only)".into()));
    }
    if std::env::var_os("POO_NO_LAUNCHD").is_some() {
        return Ok(Some("POO_NO_LAUNCHD set; launchd agent not touched".into()));
    }
    let any = cfg.schedule.iter().any(|s| s.enabled);
    if any {
        let p = install_agent()?;
        Ok(Some(format!("scheduler agent active ({})", crate::util::tilde(&p))))
    } else if agent_installed() {
        uninstall_agent()?;
        Ok(Some("no enabled schedules; scheduler agent removed".into()))
    } else {
        Ok(None)
    }
}

pub fn notify(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('\\', "").replace('"', "'"),
        title.replace('"', "'")
    );
    let _ = std::process::Command::new("osascript").args(["-e", &script]).output();
}

fn log_line(line: &str) {
    let p = state_dir().join("scheduler.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{} {}", crate::util::iso8601(now()), line);
    }
}

/// One exclusive runner at a time (scheduler, app and CLI cleans).
pub struct RunLock(#[allow(dead_code)] std::fs::File);

impl RunLock {
    pub fn try_acquire() -> Option<RunLock> {
        use std::os::unix::io::AsRawFd;
        let p = state_dir().join("run.lock");
        let _ = std::fs::create_dir_all(state_dir());
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(p).ok()?;
        let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        (r == 0).then_some(RunLock(f))
    }
}

/// Runs one schedule now. `dry_run` previews without deleting.
pub fn run(ctx: &Ctx, s: &Schedule, dry_run: bool) -> Result<RunRecord> {
    let dry_run = dry_run || crate::clean::read_only();
    let _lock = RunLock::try_acquire().ok_or_else(|| anyhow!("another poo clean or scheduled run is in progress"))?;
    let disk = crate::fsutil::disk_info(&crate::util::home());
    if let Some(b) = &s.when_free_below {
        let threshold = parse_amount(b, disk.total)?;
        if disk.free >= threshold {
            return Ok(RunRecord {
                at: now(),
                status: "not-needed".into(),
                free_after: disk.free,
                message: format!("{} free, above {}", bytes(disk.free), b),
                ..Default::default()
            });
        }
    }
    // Always rescan (from the journal when possible): never act on an old picture of the disk.
    let report = crate::scan::scan_auto(&ctx.cfg, &ctx.cat, &ScanOptions::default());
    crate::scan::save_report(&report)?;
    let target = match &s.target_free {
        Some(t) => Some(parse_amount(t, disk.total)?.saturating_sub(report.disk.free)),
        None => None,
    };
    let filter = Filter {
        buckets: s.buckets.clone(),
        exclude_buckets: s.exclude_buckets.clone(),
        tags: s.tags.clone(),
        exclude_tags: s.exclude_tags.clone(),
        ..Default::default()
    };
    let mut plan = crate::ops::make_plan(ctx, &report, &s.plan, target, &filter)?;
    if let Some(m) = &s.max_bytes {
        let cap = parse_amount(m, disk.total)?;
        let mut sum = 0;
        plan.items.retain(|i| {
            if sum + i.total_bytes > cap {
                return false;
            }
            sum += i.total_bytes;
            true
        });
    }
    let opts = CleanOptions::from_config(&ctx.cfg, RiskLevel::Standard, dry_run, &format!("schedule:{}", s.id));
    let res = clean::clean(&plan.items, &ctx.cat, &opts);
    let mut report = report;
    if !dry_run {
        crate::ops::forget_cleaned(ctx, &mut report, &res);
    }
    let rec = RunRecord {
        at: now(),
        status: if dry_run { "dry-run".into() } else { "ran".into() },
        freed_bytes: res.freed_bytes,
        cleaned: res.cleaned,
        skipped: res.skipped,
        failed: res.failed,
        free_after: res.disk_free_after,
        message: res.read_only.clone().or_else(|| res.snapshot_note.clone()).unwrap_or_default(),
    };
    Ok(rec)
}

/// Called by launchd: runs whatever is due.
pub fn tick() -> Result<Vec<(String, RunRecord)>> {
    let ctx = Ctx::load().context("loading config")?;
    let mut state = load_state();
    state.last_tick = now();
    let mut done = vec![];
    for s in ctx.cfg.schedule.iter().filter(|s| s.enabled) {
        let last = state.last_run.get(&s.id).map(|r| r.at);
        if now() < next_run(s, last) && last.is_some() {
            continue;
        }
        if last.is_none() && s.at.is_some() && now() < next_run(s, None) {
            continue;
        }
        match run(&ctx, s, false) {
            Ok(rec) => {
                log_line(&format!("{}: {} freed {} ({} cleaned, {} skipped, {} failed)", s.id, rec.status, bytes(rec.freed_bytes), rec.cleaned, rec.skipped, rec.failed));
                if s.notify && rec.status == "ran" && rec.freed_bytes > 0 {
                    notify("poo", &format!("{} freed {}. {} free now.", s.id, bytes(rec.freed_bytes), bytes(rec.free_after)));
                }
                if rec.status != "not-needed" || last.is_none() {
                    state.last_run.insert(s.id.clone(), rec.clone());
                }
                done.push((s.id.clone(), rec));
            }
            Err(e) => {
                log_line(&format!("{}: failed: {:#}", s.id, e));
                state.last_run.insert(s.id.clone(), RunRecord { at: now(), status: "error".into(), message: format!("{:#}", e), ..Default::default() });
            }
        }
    }
    save_state(&state);
    Ok(done)
}
