//! Anonymous error reports, so failed cleans and crashes can be fixed without anyone filing
//! an issue. On by default; `poo config set telemetry.enabled false`, `POO_NO_TELEMETRY=1`
//! or `DO_NOT_TRACK=1` turn it off, and debug builds, test harnesses and sandboxed runs
//! (POO_HOME, POO_READ_ONLY, POO_CLEAN_ONLY_UNDER) never send.
//!
//! What a report holds: what failed (a clean of one rule, a scheduled run, a crash), the
//! error text with every path, the home folder and the user name removed, poo's version,
//! macOS version, CPU architecture, how poo was run (cli, app, mcp, schedule, hook, tui)
//! and a random install id (the file `telemetry/id` in the state folder; delete it for a
//! new one). Never paths, project or file names, file contents, hostnames or accounts.
//!
//! Reports are queued as files in the state folder and sent by a detached `poo` after the
//! command returns, so nothing waits on the network. One report per problem per day per
//! machine; the queue holds at most a few dozen and drops anything a week old.

use crate::clean::CleanOptions;
use crate::config::state_dir;
use crate::model::Item;
use crate::util::{home, now};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const KEY: &str = "791394aae4d3788618ccb24afb405648";
const HOST: &str = "o124360.ingest.us.sentry.io";
const PROJECT: &str = "4512476368928768";
/// Hidden argument the detached sender runs with.
pub const FLUSH_ARG: &str = "__telemetry-flush";
const MAX_QUEUE: usize = 40;
const MAX_AGE_SECS: i64 = 7 * 86_400;
/// The same problem is reported once a day per machine.
const DEDUPE_SECS: i64 = 86_400;

fn env_on(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

static ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether this process may queue and send reports.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        if env_on("POO_NO_TELEMETRY") || env_on("DO_NOT_TRACK") {
            return false;
        }
        // POO_TELEMETRY=1 sends from anywhere (to test reporting itself); config still wins.
        if !env_on("POO_TELEMETRY") {
            // Sandboxes and test aids are not real use.
            if ["POO_HOME", "POO_READ_ONLY", "POO_CLEAN_ONLY_UNDER"].iter().any(|k| std::env::var_os(k).is_some()) {
                return false;
            }
            if cfg!(debug_assertions) {
                return false;
            }
            // Only the real binary, never a test harness (poo-<hash>).
            if !std::env::current_exe().ok().and_then(|e| e.file_stem().map(|n| n == "poo")).unwrap_or(false) {
                return false;
            }
        }
        crate::config::Config::load().map_or(true, |c| c.telemetry.enabled)
    })
}

/// How poo was started: "app" when the Poo app runs it (it sets POO_CLIENT=app), else "cli".
pub fn client() -> &'static str {
    match std::env::var("POO_CLIENT").as_deref() {
        Ok("app") => "app",
        _ => "cli",
    }
}

fn dir() -> PathBuf {
    state_dir().join("telemetry")
}

fn queue_dir() -> PathBuf {
    dir().join("queue")
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        let seed = format!("{}-{}-{:?}", now(), std::process::id(), std::time::Instant::now());
        return crate::util::sha256_hex(seed.as_bytes())[..n * 2].to_string();
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A random id for this install, made on first use. Not derived from the machine or user.
fn install_id() -> String {
    let p = dir().join("id");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let s = s.trim();
        if s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return s.to_string();
        }
    }
    let id = random_hex(16);
    let _ = std::fs::create_dir_all(dir());
    let _ = std::fs::write(&p, &id);
    id
}

fn os_version() -> String {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| crate::util::run_capture(&["sw_vers", "-productVersion"], 3).map(|s| s.trim().to_string()).unwrap_or_default()).clone()
}

/// Removes anything that could say who or where: the home folder, the user name, and every
/// token that looks like a path. Keeps the shape of the error ("could not erase <path>:
/// Permission denied").
pub fn scrub(s: &str) -> String {
    let h = home();
    let mut s = s.replace(&*h.to_string_lossy(), "~");
    if let Some(user) = h.file_name().map(|u| u.to_string_lossy().to_string()).filter(|u| u.len() >= 3) {
        s = s.replace(&user, "<user>");
    }
    // A quoted path may hold spaces ('/Users/x/My App'): the whole quote goes.
    for q in ['\'', '"'] {
        let parts: Vec<&str> = s.split(q).collect();
        if parts.len() >= 3 {
            s = parts
                .iter()
                .enumerate()
                .map(|(i, p)| if i % 2 == 1 && i + 1 < parts.len() && (p.contains('/') || p.starts_with('~')) { "<path>" } else { p })
                .collect::<Vec<_>>()
                .join(&q.to_string());
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut tok = String::new();
    let flush = |tok: &mut String, out: &mut String| {
        if tok.contains('/') || tok.starts_with('~') {
            let tail: String = tok.chars().rev().take_while(|c| matches!(c, ':' | '.' | ',' | ';')).collect();
            out.push_str("<path>");
            out.extend(tail.chars().rev());
        } else {
            out.push_str(tok);
        }
        tok.clear();
    };
    for c in s.chars() {
        if c.is_whitespace() || "'\"`()[]{}<>=".contains(c) {
            flush(&mut tok, &mut out);
            out.push(c);
        } else {
            tok.push(c);
        }
    }
    flush(&mut tok, &mut out);
    // A path token next to its brackets reads "<<path>>"; keep it simple.
    let out = out.replace("<<path>>", "<path>");
    out.chars().take(400).collect()
}

/// What groups reports: the scrubbed text with numbers folded, so sizes, pids and counts
/// do not split one problem into many.
fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut in_num = false;
    for c in scrub(s).chars() {
        if c.is_ascii_digit() {
            if !in_num {
                out.push('N');
            }
            in_num = true;
        } else {
            in_num = false;
            out.push(c);
        }
    }
    out.chars().take(160).collect()
}

/// Problems already queued by this process, so 50 items failing the same way send one.
static SEEN: Mutex<Option<BTreeMap<String, u64>>> = Mutex::new(None);

/// Drops a problem reported in the last day; otherwise remembers it.
fn fresh(fp: &str) -> bool {
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let m = seen.get_or_insert_with(BTreeMap::new);
        if let Some(n) = m.get_mut(fp) {
            *n += 1;
            return false;
        }
        m.insert(fp.to_string(), 1);
    }
    let p = dir().join("sent.json");
    let mut sent: BTreeMap<String, i64> = std::fs::read(&p).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    let key = crate::util::short_hash(fp);
    let t = now();
    sent.retain(|_, at| t - *at < DEDUPE_SECS * 2);
    if sent.get(&key).is_some_and(|at| t - at < DEDUPE_SECS) {
        return false;
    }
    sent.insert(key, t);
    let _ = std::fs::create_dir_all(dir());
    let tmp = p.with_extension(format!("json.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, serde_json::to_vec(&sent).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, &p);
    }
    true
}

/// Queues one report. `message` is scrubbed here; tags must already be free of paths.
pub fn record(level: &str, area: &str, message: &str, fingerprint: &[&str], tags: &[(&str, &str)]) {
    if !enabled() {
        return;
    }
    let msg = scrub(message);
    let mut fp: Vec<String> = vec![area.to_string()];
    fp.extend(fingerprint.iter().map(|s| s.to_string()));
    fp.push(normalize(message));
    if !fresh(&fp.join("|")) {
        return;
    }
    let _ = std::fs::create_dir_all(queue_dir());
    if std::fs::read_dir(queue_dir()).map_or(0, |d| d.count()) >= MAX_QUEUE {
        return;
    }
    let mut t: serde_json::Map<String, Value> = tags.iter().map(|(k, v)| (k.to_string(), Value::from(scrub(v)))).collect();
    t.insert("area".into(), area.into());
    t.entry("client").or_insert_with(|| client().into());
    t.insert("arch".into(), std::env::consts::ARCH.into());
    let id = random_hex(16);
    let event = json!({
        "event_id": id,
        "timestamp": now(),
        "platform": "other",
        "level": level,
        "logger": "poo",
        "release": format!("poo@{}", crate::VERSION),
        "environment": "production",
        "message": {"formatted": msg},
        "fingerprint": fp,
        "tags": t,
        // An explicit placeholder IP keeps Sentry from locating the sender from the connection.
        "user": {"id": install_id(), "ip_address": "0.0.0.0"},
        "contexts": {"os": {"name": "macOS", "version": os_version()}},
        "sdk": {"name": "poo.telemetry", "version": crate::VERSION},
    });
    let p = queue_dir().join(format!("{}.json", id));
    let tmp = queue_dir().join(format!(".{}.tmp", id));
    if std::fs::write(&tmp, event.to_string()).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}

pub fn error(area: &str, message: &str, tags: &[(&str, &str)]) {
    record("error", area, message, &[], tags);
}

pub fn warning(area: &str, message: &str, tags: &[(&str, &str)]) {
    record("warning", area, message, &[], tags);
}

fn actor_kind(actor: &str) -> &str {
    actor.split(':').next().unwrap_or(actor)
}

fn item_tags<'a>(item: &'a Item, opts: &'a CleanOptions, action: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("rule", item.rule.as_str()),
        ("bucket", item.bucket.as_str()),
        ("tier", item.tier.as_str()),
        ("action", action),
        ("client", actor_kind(&opts.actor)),
        ("mode", opts.mode.as_str()),
        ("trash", if opts.trash { "yes" } else { "no" }),
    ]
}

fn action_name(item: &Item) -> String {
    serde_json::to_value(item.action.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

/// A clean of one item failed.
pub fn clean_failed(item: &Item, reason: &str, opts: &CleanOptions) {
    let action = action_name(item);
    record("error", "clean", &format!("{} failed: {}", item.rule, reason), &[&item.rule], &item_tags(item, opts, &action));
}

/// An item was skipped because poo could not tell whether it was safe (a probe or guard
/// gave no answer, the process list was unreadable).
pub fn clean_skipped(item: &Item, reason: &str, opts: &CleanOptions) {
    let action = action_name(item);
    record("warning", "clean-skip", &format!("{} skipped: {}", item.rule, reason), &[&item.rule], &item_tags(item, opts, &action));
}

/// A clean gave up waiting for another one.
pub fn busy(actor: &str, b: &crate::schedule::Busy) {
    if b.cancelled {
        return;
    }
    let holder = actor_kind(&b.holder.actor).to_string();
    let waited = b.waited.to_string();
    let held = if b.holder.since > 0 { (now() - b.holder.since).max(0) / 60 } else { 0 };
    let held = held.to_string();
    record(
        "warning",
        "busy",
        &format!("{} clean gave up waiting for a {} clean", actor_kind(actor), holder),
        &[actor_kind(actor), &holder],
        &[("client", actor_kind(actor)), ("holder", &holder), ("waited_secs", &waited), ("holder_minutes", &held)],
    );
}

/// A command ended with an error worth knowing about (not usage mistakes or refusals).
pub fn cli_error(command: &str, kind: &str, message: &str) {
    if matches!(kind, "usage" | "not_found" | "needs_approval" | "report_only" | "partial" | "busy" | "cancelled" | "invalid_config" | "invalid_mods") {
        return;
    }
    record("error", "cli", message, &[command, kind], &[("command", command), ("kind", kind)]);
}

/// Reports panics (on the next run, from the queue). Keeps the default message.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        // Source locations are poo's own files (scan.rs:120), not the user's.
        let at = info
            .location()
            .map(|l| format!("{}:{}", std::path::Path::new(l.file()).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), l.line()))
            .unwrap_or_default();
        record("fatal", "panic", &format!("panicked at {}: {}", at, msg), &[&at], &[("location", &at)]);
        prev(info);
    }));
}

fn queued() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(queue_dir()) else { return vec![] };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json") && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect()
}

/// Starts a detached sender when reports are waiting. Returns at once.
pub fn flush_in_background() {
    use std::os::unix::process::CommandExt;
    if !enabled() || queued().is_empty() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(FLUSH_ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let _ = cmd.spawn();
}

/// Sends what is queued, one sender at a time. Keeps a report when the network or Sentry
/// is unavailable, drops it once it is a week old or Sentry refuses it.
pub fn flush() -> usize {
    if !enabled() {
        // Turned off after reports were queued: they are never sent.
        let _ = std::fs::remove_dir_all(queue_dir());
        return 0;
    }
    let _ = std::fs::create_dir_all(dir());
    let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir().join("send.lock")) else { return 0 };
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return 0;
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(format!("poo/{}", crate::VERSION))
        .build()
        .new_agent();
    let url = format!("https://{}/api/{}/envelope/", HOST, PROJECT);
    let auth = format!("Sentry sentry_version=7, sentry_key={}, sentry_client=poo/{}", KEY, crate::VERSION);
    let mut sent = 0;
    for p in queued().into_iter().take(MAX_QUEUE) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(&text) else {
            let _ = std::fs::remove_file(&p);
            continue;
        };
        if now() - ev["timestamp"].as_i64().unwrap_or(0) > MAX_AGE_SECS {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        let header = json!({"event_id": ev["event_id"], "sent_at": crate::util::iso8601(now())});
        let body = format!("{}\n{}\n{}\n", header, json!({"type": "event", "length": text.len()}), text);
        match agent.post(&url).header("Content-Type", "application/x-sentry-envelope").header("X-Sentry-Auth", &auth).send(body.as_bytes()) {
            Ok(_) => {
                sent += 1;
                let _ = std::fs::remove_file(&p);
            }
            // Rate limited or Sentry down: try again later. Anything else will never be taken.
            Err(ureq::Error::StatusCode(c)) if c == 429 || c >= 500 => break,
            Err(ureq::Error::StatusCode(_)) => {
                let _ = std::fs::remove_file(&p);
            }
            Err(_) => break,
        }
    }
    sent
}

/// Says once, in an interactive terminal, that reports are on and how to turn them off.
pub fn notice_once() -> Option<String> {
    if !enabled() {
        return None;
    }
    let p = dir().join("notice-shown");
    if p.exists() {
        return None;
    }
    let _ = std::fs::create_dir_all(dir());
    std::fs::write(&p, now().to_string()).ok()?;
    Some("poo sends anonymous error reports when a clean fails (no paths or names). Turn off: poo config set telemetry.enabled false".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_removes_paths_and_user() {
        let h = home().to_string_lossy().to_string();
        let s = scrub(&format!("could not erase {}/dev/secret-app/node_modules/x: Permission denied", h));
        assert_eq!(s, "could not erase <path>: Permission denied");
        assert_eq!(scrub("fatal: '/private/tmp/My App' is locked"), "fatal: '<path>' is locked");
        assert_eq!(scrub("`brew cleanup --prune=14` failed"), "`brew cleanup --prune=14` failed");
        assert_eq!(scrub("could not erase web/.next/cache:"), "could not erase <path>:");
        assert!(!scrub(&format!("in {}", h)).contains(&h));
    }

    #[test]
    fn normalize_folds_numbers() {
        assert_eq!(normalize("could not erase 12 entries (first: x: y)"), normalize("could not erase 3 entries (first: x: y)"));
    }
}
