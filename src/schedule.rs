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

/// How often launchd wakes the scheduler (the agent's `StartInterval`).
pub const TICK_SECS: i64 = 900;

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
    /// When the scheduler first saw each enabled schedule that has not run yet, so a schedule
    /// with `at` runs at the first HH:MM after it was added, even if that tick came late.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub first_seen: BTreeMap<String, i64>,
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
    let tmp = p.with_extension(format!("json.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
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

/// Unix time of HH:MM local time, `days` after the local day of `t`.
fn local_at(t: i64, days: i64, h: i64, m: i64) -> i64 {
    let t = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm.tm_mday += days as i32;
    tm.tm_hour = h as i32;
    tm.tm_min = m as i32;
    tm.tm_sec = 0;
    // Let mktime work out daylight saving for the new day.
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// The first HH:MM at or after `t`.
fn slot_from(t: i64, h: i64, m: i64) -> i64 {
    let s = local_at(t, 0, h, m);
    if s >= t {
        s
    } else {
        local_at(t, 1, h, m)
    }
}

/// The last HH:MM at or before `t`.
fn slot_before(t: i64, h: i64, m: i64) -> i64 {
    let s = local_at(t, 0, h, m);
    if s <= t {
        s
    } else {
        local_at(t, -1, h, m)
    }
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

/// When a schedule is due (unix time); a tick at or after it runs the schedule. `last` is its
/// last recorded run and `since` when the scheduler first saw it (for one that never ran).
///
/// A schedule with `at` is due at the first HH:MM after its last run's slot plus `every`.
/// The scheduler only wakes every 15 minutes and not at all while the Mac sleeps, so the
/// slot stays due until a tick catches it: 03:30 runs at the 03:33 tick, or on wake.
pub fn due_at(s: &Schedule, last: Option<i64>, since: i64, now: i64) -> i64 {
    let every = parse_every(&s.every).unwrap_or(86400);
    let Some((h, m)) = s.at.as_deref().and_then(|a| parse_at(a).ok()) else {
        return last.map_or(now, |l| l + every);
    };
    let from = match last {
        // Counted from the slot the last run served, so a late run (the Mac slept through
        // 03:30 and ran at 09:00) does not push the next night out. The hour of slack keeps
        // a daylight saving change from skipping a day.
        Some(l) => slot_before(l, h, m) + every - 3600,
        None => since,
    };
    slot_from(from, h, m)
}

/// When a schedule runs next, for display: when it is due, but never before the
/// scheduler's next wake. Overdue schedules run at that wake.
pub fn next_run(s: &Schedule, state: &ScheduleState) -> i64 {
    let t = now();
    let last = state.last_run.get(&s.id).map(|r| r.at);
    let since = state.first_seen.get(&s.id).copied().unwrap_or(t);
    let next_tick = if state.last_tick > 0 { state.last_tick + TICK_SECS } else { t };
    due_at(s, last, since, t).max(next_tick).max(t)
}

/// Whether a run's record replaces the stored one. Busy runs keep the last real run so the
/// schedule tries again on the next tick; a not-needed check does not count as a run, but
/// it does replace an old error, which would otherwise show as the schedule's state forever.
pub fn records(prev: Option<&RunRecord>, rec: &RunRecord) -> bool {
    match rec.status.as_str() {
        "busy" => false,
        "not-needed" => prev.is_none_or(|p| p.status == "error"),
        _ => true,
    }
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

/// One exclusive runner at a time (scheduler, app and CLI cleans). The holder writes who it
/// is into the lock file, so a clean that has to wait can say what it is waiting for.
pub struct RunLock(#[allow(dead_code)] std::fs::File);

/// Who holds the run lock, as its holder wrote it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub actor: String,
    pub since: i64,
}

impl Holder {
    /// "a scheduled clean (guard)", "a clean from an agent"...
    pub fn describe(&self) -> String {
        let (kind, rest) = self.actor.split_once(':').unwrap_or((self.actor.as_str(), ""));
        let what = match kind {
            "schedule" => format!("the scheduled clean `{}`", rest),
            "hook" => format!("the {} agent hook", rest),
            "mcp" => "a clean an agent started".to_string(),
            "tui" => "a clean in poo's terminal UI".to_string(),
            "app" => "a clean in the Poo app".to_string(),
            _ => "another poo clean".to_string(),
        };
        let secs = (now() - self.since).max(0);
        if self.since > 0 && secs >= 5 {
            format!("{} (running for {})", what, crate::util::ago_secs(secs))
        } else {
            what
        }
    }
}

fn run_lock_path() -> PathBuf {
    state_dir().join("run.lock")
}

impl RunLock {
    /// Takes the lock if nobody holds it.
    pub fn try_acquire(actor: &str) -> Option<RunLock> {
        use std::io::{Seek, Write};
        use std::os::unix::io::AsRawFd;
        let _ = std::fs::create_dir_all(state_dir());
        let mut f = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(run_lock_path()).ok()?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return None;
        }
        let h = Holder { pid: std::process::id(), actor: actor.to_string(), since: now() };
        if f.set_len(0).is_ok() && f.rewind().is_ok() {
            let _ = f.write_all(serde_json::to_string(&h).unwrap_or_default().as_bytes());
        }
        Some(RunLock(f))
    }

    /// Waits up to `timeout` for the lock, calling `waiting` with the holder about once a
    /// second. Gives up early when `stop` says so (cancel pressed while waiting).
    pub fn acquire_wait(actor: &str, timeout: std::time::Duration, waiting: &dyn Fn(&Holder), stop: &dyn Fn() -> bool) -> Result<RunLock, Busy> {
        let start = std::time::Instant::now();
        let mut last_note: Option<std::time::Instant> = None;
        loop {
            if let Some(l) = Self::try_acquire(actor) {
                return Ok(l);
            }
            let h = holder().unwrap_or_default();
            if stop() {
                return Err(Busy { holder: h, waited: start.elapsed().as_secs(), cancelled: true });
            }
            if start.elapsed() >= timeout {
                return Err(Busy { holder: h, waited: start.elapsed().as_secs(), cancelled: false });
            }
            if last_note.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
                waiting(&h);
                last_note = Some(std::time::Instant::now());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

/// The current holder of the run lock, if any. Only meaningful while the lock is held.
pub fn holder() -> Option<Holder> {
    let text = std::fs::read_to_string(run_lock_path()).ok()?;
    serde_json::from_str(text.trim()).ok()
}

/// Why a clean could not take the run lock.
#[derive(Debug, Clone)]
pub struct Busy {
    pub holder: Holder,
    pub waited: u64,
    pub cancelled: bool,
}

impl Busy {
    pub fn message(&self) -> String {
        if self.cancelled {
            format!("cancelled while waiting for {} to finish", self.holder.describe())
        } else if self.waited > 0 {
            format!("waited {} for {} to finish; it is still running", crate::util::ago_secs(self.waited as i64), self.holder.describe())
        } else {
            format!("{} is in progress", self.holder.describe())
        }
    }
}

/// The message for a run that found the lock taken and did not wait.
pub fn busy_message() -> String {
    Busy { holder: holder().unwrap_or_default(), waited: 0, cancelled: false }.message()
}

/// Runs one schedule now. `dry_run` previews without deleting.
pub fn run(ctx: &Ctx, s: &Schedule, dry_run: bool) -> Result<RunRecord> {
    let dry_run = dry_run || crate::clean::read_only();
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
    // Not needed is decided without the lock; a busy lock is retried on the next tick.
    let Some(_lock) = RunLock::try_acquire(&format!("schedule:{}", s.id)) else {
        return Ok(RunRecord { at: now(), status: "busy".into(), free_after: disk.free, message: busy_message(), ..Default::default() });
    };
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
    let mut plan = crate::ops::make_plan(ctx, &report, &s.plan, target, &filter, crate::plan::Admission::default())?;
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
    let t = now();
    state.last_tick = t;
    // Only enabled schedules that have not run yet keep a first-seen time.
    let waiting: Vec<&str> = ctx.cfg.schedule.iter().filter(|s| s.enabled && !state.last_run.contains_key(&s.id)).map(|s| s.id.as_str()).collect();
    state.first_seen.retain(|id, _| waiting.contains(&id.as_str()));
    let mut done = vec![];
    for s in ctx.cfg.schedule.iter().filter(|s| s.enabled) {
        let prev = state.last_run.get(&s.id).cloned();
        let last = prev.as_ref().map(|r| r.at);
        let since = if last.is_none() { *state.first_seen.entry(s.id.clone()).or_insert(t) } else { t };
        if t < due_at(s, last, since, t) {
            continue;
        }
        match run(&ctx, s, false) {
            Ok(rec) if rec.status == "busy" => {
                log_line(&format!("{}: {}; trying again next tick", s.id, rec.message));
                done.push((s.id.clone(), rec));
            }
            Ok(rec) => {
                log_line(&format!("{}: {} freed {} ({} cleaned, {} skipped, {} failed)", s.id, rec.status, bytes(rec.freed_bytes), rec.cleaned, rec.skipped, rec.failed));
                if s.notify && rec.status == "ran" && rec.freed_bytes > 0 {
                    notify("poo", &format!("{} freed {}. {} free now.", s.id, bytes(rec.freed_bytes), bytes(rec.free_after)));
                }
                if records(prev.as_ref(), &rec) {
                    state.last_run.insert(s.id.clone(), rec.clone());
                    state.first_seen.remove(&s.id);
                }
                done.push((s.id.clone(), rec));
            }
            Err(e) => {
                log_line(&format!("{}: failed: {:#}", s.id, e));
                crate::telemetry::error("schedule", &format!("schedule run failed: {:#}", e), &[("schedule", s.id.as_str())]);
                state.last_run.insert(s.id.clone(), RunRecord { at: now(), status: "error".into(), message: format!("{:#}", e), ..Default::default() });
                state.first_seen.remove(&s.id);
            }
        }
    }
    save_state(&state);
    Ok(done)
}
