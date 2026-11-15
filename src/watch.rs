//! `poo watch`: keeps the report current while someone is looking (the app, a terminal).
//! File changes stream in from FSEvents; after a quiet moment poo updates the report from
//! the journal, so only what changed is measured again.

use crate::ops::Ctx;
use crate::scan::{self, ScanOptions};
use crate::util::{bytes, now};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

pub struct WatchOptions {
    /// Quiet time after the last change before updating.
    pub debounce: Duration,
    /// Update at least this often anyway (ages move tiers, other apps use the disk).
    pub every: Duration,
    pub json: bool,
}

/// Changes outside dev folders (app caches churn constantly) update at most this often.
const OTHER_GAP: Duration = Duration::from_secs(60);
/// A build that keeps writing still shows up this often.
const MAX_WAIT: Duration = Duration::from_secs(30);

pub fn run(ctx: &mut Ctx, o: &WatchOptions) -> Result<()> {
    let (roots, _, _) = scan::scan_roots(&ctx.cfg, &ctx.cat);
    let watched = scan::watch_roots(&roots);
    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let w = watched.clone();
    std::thread::spawn(move || crate::fsevents::watch(&w, 0.5, tx));
    let parent = unsafe { libc::getppid() };
    let state = crate::config::state_dir();
    let config_file = crate::config::config_dir().join("config.toml");
    let mut config_mtime = crate::fsutil::mtime(&config_file);

    emit(o.json, json!({"event": "ready", "roots": roots, "watching": watched}), &format!("watching {} folders for changes (Ctrl-C to stop)", watched.len()));
    let mut known = update(ctx, o, 0);

    let mut last_update = Instant::now();
    let mut first_dev: Option<Instant> = None;
    let mut last_dev = Instant::now();
    let mut other = false;
    let mut changed = 0usize;
    loop {
        // Started by the app: leave with it.
        if parent != 1 && unsafe { libc::getppid() } == 1 {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(batch) => {
                for p in batch {
                    if p.starts_with(&state) || scan::path_forms(&p).iter().any(|f| f.starts_with(&state)) {
                        continue;
                    }
                    if scan::path_forms(&p).iter().any(|f| roots.iter().any(|r| f.starts_with(r))) {
                        first_dev.get_or_insert_with(Instant::now);
                        last_dev = Instant::now();
                        changed += 1;
                    } else if scan::nearest_known(&p, &known).is_some() {
                        other = true;
                        changed += 1;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                emit(o.json, json!({"event": "error", "message": "the file-change stream stopped"}), "the file-change stream stopped");
                anyhow::bail!("the file-change stream stopped");
            }
        }
        let config_now = crate::fsutil::mtime(&config_file);
        let config_changed = config_now != config_mtime;
        let dev_due = first_dev.map_or(false, |f| last_dev.elapsed() >= o.debounce || f.elapsed() >= MAX_WAIT);
        let other_due = other && last_update.elapsed() >= OTHER_GAP;
        if dev_due || other_due || config_changed || last_update.elapsed() >= o.every {
            if config_changed {
                config_mtime = config_now;
                if let Err(e) = ctx.reload() {
                    emit(o.json, json!({"event": "error", "message": e.to_string()}), &format!("config: {}", e));
                }
            }
            if changed > 0 {
                emit(o.json, json!({"event": "changed", "paths": changed}), &format!("{} folders changed", changed));
            }
            known = update(ctx, o, changed);
            last_update = Instant::now();
            first_dev = None;
            other = false;
            changed = 0;
        }
    }
}

/// Updates and saves the report; returns the item paths outside dev folders worth watching.
fn update(ctx: &Ctx, o: &WatchOptions, _changed: usize) -> HashSet<PathBuf> {
    let t = Instant::now();
    let (r, incremental) = match scan::scan_incremental(&ctx.cfg, &ctx.cat, &ScanOptions::default()) {
        Some(r) => (r, true),
        None => (scan::scan(&ctx.cfg, &ctx.cat, &ScanOptions::default()), false),
    };
    if let Err(e) = scan::save_report(&r) {
        emit(o.json, json!({"event": "error", "message": e.to_string()}), &format!("could not save the report: {}", e));
    }
    let took = t.elapsed().as_millis() as u64;
    emit(
        o.json,
        json!({
            "event": "report",
            "generated_at": r.scanned_at,
            "took_ms": took,
            "incremental": incremental,
            "items": r.items.len(),
            "total_bytes": r.total_bytes,
            "free": r.disk.free,
        }),
        &format!(
            "{} updated in {:.1}s{} · {} tracked · {} free",
            clock(now()),
            took as f64 / 1000.0,
            if incremental { "" } else { " (full walk)" },
            bytes(r.total_bytes),
            bytes(r.disk.free)
        ),
    );
    scan::load_index()
        .map(|i| i.raws.into_iter().filter(|r| !r.is_virtual && r.origin != "walk" && r.origin != "worktree").map(|r| r.path).collect())
        .unwrap_or_default()
}

fn emit(json: bool, v: Value, human: &str) {
    use std::io::Write;
    if json {
        println!("{}", v);
    } else {
        println!("{}", human);
    }
    let _ = std::io::stdout().flush();
}

/// Local wall-clock time as HH:MM:SS.
fn clock(secs: i64) -> String {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
