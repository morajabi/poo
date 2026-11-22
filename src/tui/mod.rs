//! `poo ui`: the interactive terminal UI (ratatui + crossterm).
//!
//! Reads come from the cached report and the incremental engine; a background thread
//! keeps it fresh. Cleaning runs `clean::clean_with` on a worker thread with the same
//! options, admission checks, floor and backups as `poo clean`.

pub mod app;
pub mod fmt;
#[cfg(test)]
mod snapshot;
pub mod theme;
pub mod view;

use crate::clean::{self, CleanProgress, CleanResult};
use crate::model::Report;
use crate::ops::{self, Ctx};
use crate::scan::{self, Progress, ScanOptions};
use app::{App, Cleaning, Effect, Modal, ScanState, Summary};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub struct UiOptions {
    pub animate: bool,
}

impl UiOptions {
    /// Animation is on unless `--no-animation`, POO_NO_ANIMATION, or macOS Reduce Motion.
    pub fn detect(no_animation_flag: bool) -> UiOptions {
        let off = no_animation_flag || std::env::var_os("POO_NO_ANIMATION").is_some() || reduce_motion();
        UiOptions { animate: !off }
    }
}

/// Reads the preference file directly: spawning `defaults` costs ~20 ms before the first frame.
fn reduce_motion() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let p = crate::util::home().join("Library/Preferences/com.apple.universalaccess.plist");
    let Ok(v) = plist::Value::from_file(p) else { return false };
    match v.as_dictionary().and_then(|d| d.get("reduceMotion")) {
        Some(plist::Value::Boolean(b)) => *b,
        Some(plist::Value::Integer(i)) => i.as_signed() == Some(1),
        _ => false,
    }
}

/// Set by SIGTERM, SIGHUP or SIGINT (e.g. `kill`, a closed terminal tab): leave cleanly,
/// restoring the terminal, after any clean in flight stops between items.
static QUIT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_quit_signal(_: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
    clean::CANCEL.store(true, Ordering::SeqCst);
}

fn install_quit_handler() {
    unsafe {
        for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            libc::signal(sig, on_quit_signal as *const () as libc::sighandler_t);
        }
    }
}

enum Msg {
    Report(Box<Report>),
    Scan(Progress, bool),
    ScanDone,
    Clean(CleanProgress),
    CleanDone(Box<CleanResult>, Option<Box<Report>>),
    Error(String),
}

#[derive(Clone, Copy)]
enum Req {
    Refresh,
    Rescan,
}

fn report_mtime() -> Option<SystemTime> {
    std::fs::metadata(scan::report_path()).and_then(|m| m.modified()).ok()
}

/// Keeps the report fresh: follows report.json written by others (`poo watch`, the app)
/// and asks the incremental engine for an update every so often or on request.
fn spawn_refresher(tx: Sender<Msg>, rx: Receiver<Req>, busy: Arc<AtomicBool>, have_report: bool) {
    std::thread::spawn(move || {
        let Ok(ctx) = Ctx::load() else { return };
        let mut seen = report_mtime();
        let mut last_update = Instant::now();
        let mut pending: Option<Req> = if have_report { Some(Req::Refresh) } else { Some(Req::Rescan) };
        loop {
            if let Some(req) = pending.take() {
                if !busy.load(Ordering::SeqCst) {
                    let full = matches!(req, Req::Rescan);
                    let ptx = tx.clone();
                    let opts = ScanOptions { full, on_progress: Some(Box::new(move |p: &Progress| { let _ = ptx.send(Msg::Scan(p.clone(), full)); })), ..Default::default() };
                    match ops::report(&ctx, full, opts) {
                        Ok(r) => {
                            seen = report_mtime();
                            if tx.send(Msg::Report(Box::new(r))).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Error(format!("{:#}", e)));
                        }
                    }
                    let _ = tx.send(Msg::ScanDone);
                    last_update = Instant::now();
                }
            }
            match rx.recv_timeout(Duration::from_millis(1500)) {
                Ok(r) => pending = Some(r),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                Err(_) => {}
            }
            // Someone else updated the report.
            let m = report_mtime();
            if m.is_some() && m != seen && !busy.load(Ordering::SeqCst) {
                seen = m;
                if let Some(r) = scan::load_report() {
                    if tx.send(Msg::Report(Box::new(r))).is_err() {
                        return;
                    }
                }
            }
            if pending.is_none() && last_update.elapsed() > Duration::from_secs(ctx.cfg.scan.fresh_seconds.max(15)) {
                pending = Some(Req::Refresh);
            }
        }
    });
}

fn start_clean(app: &mut App, tx: &Sender<Msg>, busy: &Arc<AtomicBool>, items: Vec<crate::model::Item>, mode: crate::model::RiskLevel, trash: bool, dry: bool) {
    let lock = if dry {
        None
    } else {
        match crate::schedule::RunLock::try_acquire("tui") {
            Some(l) => Some(l),
            None => {
                if let Modal::Review(rv) = &mut app.modal {
                    let mut m = crate::schedule::busy_message();
                    if let Some(c) = m.get_mut(0..1) {
                        c.make_ascii_uppercase();
                    }
                    rv.error = Some(format!("{}. Try again when it finishes.", m));
                }
                return;
            }
        }
    };
    let opts = app.clean_options(mode, trash, dry);
    let total_bytes: u64 = items.iter().map(|i| i.total_bytes).sum();
    app.modal = Modal::Cleaning(Cleaning { total: items.len(), total_bytes, progress: None, started: Instant::now(), cancel_requested: false, dry });
    clean::CANCEL.store(false, Ordering::SeqCst);
    busy.store(true, Ordering::SeqCst);
    let tx = tx.clone();
    let busy = busy.clone();
    std::thread::spawn(move || {
        let _lock = lock;
        let Ok(ctx) = Ctx::load() else {
            let _ = tx.send(Msg::Error("could not load config".into()));
            busy.store(false, Ordering::SeqCst);
            return;
        };
        let ptx = tx.clone();
        let cb = move |p: &CleanProgress| {
            let _ = ptx.send(Msg::Clean(p.clone()));
        };
        let res = clean::clean_with(&items, &ctx.cat, &opts, Some(&cb));
        let mut report = None;
        if !dry {
            if let Some(mut r) = scan::load_report() {
                ops::forget_cleaned(&ctx, &mut r, &res);
                report = Some(Box::new(r));
            }
        }
        clean::CANCEL.store(false, Ordering::SeqCst);
        busy.store(false, Ordering::SeqCst);
        let _ = tx.send(Msg::CleanDone(Box::new(res), report));
    });
}

fn reveal(path: &str) {
    let _ = std::process::Command::new("open")
        .args(["-R", path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub fn run(o: UiOptions) -> anyhow::Result<()> {
    let ctx = Ctx::load()?;
    let cached = scan::load_report().filter(|r| r.version == crate::VERSION);
    let have = cached.is_some();
    let mut app = App::new(ctx, cached, o.animate);
    let mut th = theme::Theme::detect();

    let (tx, rx) = channel::<Msg>();
    let (req_tx, req_rx) = channel::<Req>();
    let busy = Arc::new(AtomicBool::new(false));
    spawn_refresher(tx.clone(), req_rx, busy.clone(), have);
    app.live = true;
    if !have {
        app.scanning = Some(ScanState { phase: "starting".into(), detail: String::new(), bytes: 0, files: 0, full: true });
    }

    install_quit_handler();
    let mut terminal = ratatui::init();
    if th.color && !theme::Theme::background_known() {
        if let Some(light) = theme::query_light_background(Duration::from_millis(120)) {
            th.light = light;
        }
    }
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    // ratatui's hook restores raw mode and the screen; mouse reporting must go too, or the
    // shell fills with escape codes on every mouse move after a crash.
    let restore_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        restore_hook(info);
    }));
    let result = (|| -> anyhow::Result<()> {
        let frame = Duration::from_millis(if o.animate { 80 } else { 250 });
        loop {
            if QUIT.load(Ordering::SeqCst) && !busy.load(Ordering::SeqCst) {
                return Ok(());
            }
            terminal.draw(|f| view::draw(f, &mut app, &th))?;
            if event::poll(frame)? {
                let eff = match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => Effect::None,
                };
                match eff {
                    Effect::Quit => {
                        if busy.load(Ordering::SeqCst) {
                            app.toast("Cleaning is still running; esc cancels it first");
                        } else {
                            return Ok(());
                        }
                    }
                    Effect::Refresh => {
                        let _ = req_tx.send(Req::Refresh);
                        app.toast("Refreshing from file changes…");
                    }
                    Effect::Rescan => {
                        let _ = req_tx.send(Req::Rescan);
                        app.scanning = Some(ScanState { phase: "starting".into(), detail: String::new(), bytes: 0, files: 0, full: true });
                    }
                    Effect::StartClean { items, mode, trash, dry } => start_clean(&mut app, &tx, &busy, items, mode, trash, dry),
                    Effect::Reveal(p) => reveal(&p),
                    Effect::None => {}
                }
            }
            app.tick = app.tick.wrapping_add(1);
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Report(r) => app.set_report(*r),
                    Msg::Scan(p, full) => {
                        // Only show a scan indicator for walks that take a moment.
                        if full || p.elapsed_ms > 700 {
                            app.scanning = Some(ScanState { phase: p.phase, detail: crate::util::tilde(std::path::Path::new(&p.detail)), bytes: p.bytes, files: p.files, full });
                        }
                    }
                    Msg::ScanDone => app.scanning = None,
                    Msg::Clean(p) => {
                        if let Modal::Cleaning(c) = &mut app.modal {
                            c.progress = Some(p);
                        }
                    }
                    Msg::CleanDone(res, report) => {
                        if let Some(r) = report {
                            app.set_report(*r);
                        }
                        app.modal = Modal::Summary(Summary { res: *res, offset: 0 });
                    }
                    Msg::Error(e) => app.toast(e),
                }
            }
        }
    })();
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}
