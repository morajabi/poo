//! Cleaning. Every item carries risks; each risk names the mode (standard, force, danger)
//! that may clean it anyway. Under all of that sits a floor no mode, mod or config lifts.

use crate::catalog::{Catalog, Guard};
use crate::config::state_dir;
use crate::fsutil::{names, read_entries};
use crate::model::{ActionKind, Item, RiskLevel, RuleKind, Tier};
use crate::procs::{self, Proc};
use crate::providers::git;
use crate::util::{expand, home, iso8601, now, run_capture, tilde};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub dry_run: bool,
    /// Move to the Trash instead of deleting (filesystem items only).
    pub trash: bool,
    /// Highest tier allowed (plans and agent autonomy). Risk mode still applies.
    pub max_tier: Option<Tier>,
    /// safe: only risk-free items. in-use: also recent, agent working, dirty worktrees
    /// (stashed first), check-first. dangerous: also running builds, data, protected,
    /// system paths, unreachable commits.
    pub mode: RiskLevel,
    /// Who asked: cli, app, mcp, schedule:<id>.
    pub actor: String,
    pub backup: crate::config::BackupConfig,
    /// Erase moved folders in a detached `poo gc` so the clean returns quickly.
    pub background_delete: bool,
}

impl Default for CleanOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            trash: false,
            max_tier: None,
            mode: RiskLevel::Standard,
            actor: "cli".into(),
            backup: Default::default(),
            background_delete: false,
        }
    }
}

impl CleanOptions {
    /// Options for a caller, with the user's backup settings applied.
    pub fn from_config(cfg: &crate::config::Config, mode: RiskLevel, dry_run: bool, actor: &str) -> Self {
        Self {
            dry_run,
            trash: cfg.backup.use_trash,
            max_tier: Some(Tier::Warm),
            mode,
            actor: actor.into(),
            backup: cfg.backup.clone(),
            background_delete: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemResult {
    pub id: String,
    pub path: String,
    pub name: String,
    /// cleaned | skipped | failed | would-clean
    pub status: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Risks accepted for this item (force or danger mode).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// What poo did to keep things recoverable (stash refs, backups, rescue branches).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Set by SIGTERM/SIGINT: finish the item in flight, then stop.
pub static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Lets a running clean stop between items when the app presses Cancel or you hit ^C.
pub fn install_cancel_handler() {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanProgress {
    pub event: &'static str,
    pub done: usize,
    pub total: usize,
    pub freed_bytes: u64,
    pub total_bytes: u64,
    pub current: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanResult {
    pub cancelled: bool,
    /// Deleted folders were moved aside; a background `poo gc` erases them.
    pub background_delete: bool,
    pub dry_run: bool,
    pub mode: RiskLevel,
    pub freed_bytes: u64,
    pub cleaned: usize,
    pub skipped: usize,
    pub failed: usize,
    pub disk_free_before: u64,
    pub disk_free_after: u64,
    pub results: Vec<ItemResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_note: Option<String>,
    /// Set when POO_READ_ONLY forced this run to a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

/// Testing and demo aid: with `POO_READ_ONLY=1` every clean, free, schedule run and hook
/// action is a dry run. It only adds safety; nothing turns it off from config or a mod.
pub fn read_only() -> bool {
    std::env::var("POO_READ_ONLY").map_or(false, |v| !v.is_empty() && v != "0")
}

pub const READ_ONLY_NOTE: &str = "POO_READ_ONLY is set: this was a dry run, nothing was deleted or changed";

/// Testing and demo aid: with `POO_CLEAN_ONLY_UNDER=<dir>` the floor refuses every path
/// outside that folder (and every virtual item). A folder that does not resolve refuses all.
pub fn clean_only_under() -> Option<Result<PathBuf, String>> {
    let v = std::env::var_os("POO_CLEAN_ONLY_UNDER").filter(|v| !v.is_empty())?;
    Some(std::fs::canonicalize(&v).map_err(|_| format!("POO_CLEAN_ONLY_UNDER `{}` does not exist; refusing everything", PathBuf::from(&v).display())))
}

/// Never touched, in any mode.
pub const NEVER: &[&str] = &[
    ".ssh", ".gnupg", "Keychains", "Mobile Documents", ".password-store", ".aws", ".kube",
    "1Password", "com.apple.TCC", "Photos Library.photoslibrary", "Mail", "CloudStorage",
    // A connected device's filesystem, mounted under ~/Library/Developer/CoreDevice.
    "DeviceFS",
];

/// System paths danger mode still refuses.
const CRITICAL: &[&str] = &[
    "/", "/System", "/bin", "/sbin", "/usr", "/etc", "/private/etc", "/var", "/private/var",
    "/Library", "/Applications", "/Users", "/Volumes", "/opt", "/opt/homebrew", "/cores", "/dev",
    "/private", "/private/var/db", "/Library/Apple", "/Library/Keychains", "/nix", "/usr/local",
];

/// Whole trees danger mode never enters.
const CRITICAL_TREES: &[&str] = &[
    "/System", "/bin", "/sbin", "/etc", "/private/etc", "/dev", "/Library/Apple", "/Library/Keychains",
    "/private/var/db", "/private/var/root", "/Library/Extensions", "/Library/Frameworks", "/Library/LaunchDaemons",
    "/Library/LaunchAgents", "/Library/PrivilegedHelperTools", "/Applications/Utilities",
    // Installed software; /usr/local stays open below these for caches and the like.
    "/usr/local/bin", "/usr/local/sbin", "/usr/local/lib", "/usr/local/include", "/usr/local/etc",
    "/usr/local/share",
];

/// Trees under your home that poo never enters or deletes from, in any mode. OrbStack mounts
/// container and VM filesystems at ~/OrbStack; their space lives in its disk image.
const NEVER_HOME_TREES: &[&str] = &["OrbStack"];

/// Symlinks macOS itself puts at the top of the disk; an ancestor through one of these is fine.
const SYSTEM_LINKS: &[&str] = &["/var", "/tmp", "/etc"];

/// The first folder above `p` that is a symlink, other than macOS's own /var, /tmp and /etc.
pub fn symlinked_ancestor(p: &Path) -> Option<PathBuf> {
    let mut cur = PathBuf::new();
    for c in p.parent()?.components() {
        cur.push(c);
        if SYSTEM_LINKS.iter().any(|s| Path::new(s) == cur) {
            continue;
        }
        if std::fs::symlink_metadata(&cur).map_or(false, |m| m.file_type().is_symlink()) {
            return Some(cur);
        }
    }
    None
}

/// The app bundle `p` sits inside (not `p` itself when it is the bundle).
pub fn enclosing_app_bundle(p: &Path) -> Option<PathBuf> {
    let mut cur = PathBuf::new();
    let parent = p.parent()?;
    for c in parent.components() {
        cur.push(c);
        if c.as_os_str().to_string_lossy().to_lowercase().ends_with(".app") {
            return Some(cur);
        }
    }
    None
}

/// The one way into an app bundle: a `versions` rule whose folder of versions lives inside
/// one (browser framework versions), deleting a direct child of that folder. The provider
/// itself always keeps the current version.
fn bundle_exception(item: &Item, rule: Option<&crate::catalog::Rule>) -> bool {
    let Some(rule) = rule else { return false };
    if rule.kind != RuleKind::Provider || rule.provider.as_deref() != Some("versions") {
        return false;
    }
    let Some(dir) = rule.param_str("path").map(|d| expand(&d)) else { return false };
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let p = Path::new(&item.path);
    let parent = p.parent().and_then(|d| std::fs::canonicalize(d).ok());
    parent.as_deref() == Some(dir.as_path()) && p.file_name().map_or(false, |n| n != "Current")
}

/// Local snapshots are someone's backups: the only snapshot command poo runs is
/// `tmutil deletelocalsnapshots <date>` for one Time Machine snapshot older than a day.
fn snapshot_command_check(item: &Item) -> Result<(), String> {
    let Some(cmd) = &item.action.command else { return Ok(()) };
    let bin = cmd.first().map(|b| Path::new(b).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let touches = |words: &[&str]| cmd.iter().any(|a| words.contains(&a.as_str()));
    match bin.as_deref() {
        Some("tmutil") => {
            if !touches(&["deletelocalsnapshots"]) {
                return if touches(&["thinlocalsnapshots", "delete", "disable", "disablelocal", "removedestination"]) {
                    Err("poo only removes single Time Machine snapshots older than a day".into())
                } else {
                    Ok(())
                };
            }
            let date = match cmd.as_slice() {
                [_, sub, date] if sub == "deletelocalsnapshots" => date,
                _ => return Err("poo removes one Time Machine snapshot at a time, by date".into()),
            };
            let at = crate::providers::snapshots::parse_snapshot_date(date)
                .ok_or_else(|| format!("`{}` is not a snapshot date", date))?;
            if now() - at < 24 * 3600 {
                return Err("this Time Machine snapshot is less than a day old; poo leaves it".into());
            }
            Ok(())
        }
        Some("diskutil") if touches(&["deleteSnapshot", "deletesnapshot"]) => {
            Err("poo does not delete APFS snapshots made by other apps; use the app that made them".into())
        }
        _ => Ok(()),
    }
}

/// Why poo will not clean a report-only item, and where to manage it instead.
pub fn report_only_message(item: &Item) -> String {
    match &item.recommend {
        Some(r) => format!("report only: poo shows this but does not clean it. {}", r),
        None => "report only: poo shows this but does not clean it".to_string(),
    }
}

/// The floor. In danger mode a few checks loosen; the rest always hold.
pub fn floor_check(item: &Item, rule_kind: Option<RuleKind>, rule_system: bool, mode: RiskLevel) -> Result<Vec<String>, String> {
    floor_check_inner(item, rule_kind, rule_system, false, mode)
}

/// The floor for an item and the rule that found it (what `clean` uses).
pub fn floor_check_rule(item: &Item, rule: Option<&crate::catalog::Rule>, mode: RiskLevel) -> Result<Vec<String>, String> {
    floor_check_inner(item, rule.map(|r| r.kind), rule.map_or(false, |r| r.system), bundle_exception(item, rule), mode)
}

fn floor_check_inner(item: &Item, rule_kind: Option<RuleKind>, rule_system: bool, in_bundle_ok: bool, mode: RiskLevel) -> Result<Vec<String>, String> {
    let mut warnings = vec![];
    let only_under = clean_only_under().transpose()?;
    if only_under.is_some() && item.path.contains("://") {
        return Err("POO_CLEAN_ONLY_UNDER is set: virtual items are refused".into());
    }
    if item.action.kind == ActionKind::Command {
        snapshot_command_check(item)?;
    }
    if item.path.contains("://") {
        return match item.action.kind {
            ActionKind::Command => Ok(warnings),
            _ => Err("virtual items can only be cleaned by a command".into()),
        };
    }
    let raw = PathBuf::from(&item.path);
    if !raw.is_absolute() {
        return Err("path is not absolute".into());
    }
    if raw.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("path contains `..`".into());
    }
    // A symlinked folder above the item could point anywhere; only macOS's own links pass.
    if let Some(link) = symlinked_ancestor(&raw) {
        return Err(format!("a folder above it is a symlink ({})", link.display()));
    }
    // Resolve symlinked parents (/var -> /private/var) but never the item itself.
    let p = match (raw.parent().and_then(|d| std::fs::canonicalize(d).ok()), raw.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => raw.clone(),
    };
    if let Some(root) = &only_under {
        if !p.starts_with(root) || &p == root {
            return Err(format!("outside POO_CLEAN_ONLY_UNDER ({})", root.display()));
        }
    }
    let h = home();
    let tmp = std::env::var("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    if p == h || h.starts_with(&p) {
        return Err("that is your home folder or one of its parents".into());
    }
    let inside_home = p.starts_with(&h);
    let user_cache = crate::util::darwin_user_cache_dir().and_then(|c| std::fs::canonicalize(c).ok());
    let inside_tmp = (p.starts_with(&tmp) && p != tmp) || user_cache.as_ref().map_or(false, |c| p.starts_with(c) && &p != c);
    if !(inside_home || inside_tmp || rule_system) {
        if mode < RiskLevel::Danger {
            return Err("outside your home and temp folders (needs --dangerously-include-protected)".into());
        }
        let ps = p.to_string_lossy();
        let under_usr = p.starts_with("/usr") && !p.starts_with("/usr/local");
        if CRITICAL.iter().any(|c| ps == *c)
            || CRITICAL_TREES.iter().any(|c| p.starts_with(c))
            || under_usr
            || p.components().count() < 3
        {
            return Err("a system folder poo never deletes".into());
        }
        warnings.push(format!("outside your home folder: {}", ps));
    }
    if inside_home {
        let depth = p.strip_prefix(&h).map(|r| r.components().count()).unwrap_or(0);
        if depth < 2 {
            return Err("too close to your home folder".into());
        }
    }
    for t in NEVER_HOME_TREES {
        let tree = h.join(t).to_string_lossy().to_lowercase();
        let ps = p.to_string_lossy().to_lowercase();
        if ps == tree || ps.starts_with(&format!("{}/", tree)) {
            return Err(format!("inside ~/{}, which poo never touches", t));
        }
    }
    if let Some(app) = enclosing_app_bundle(&p) {
        if !in_bundle_ok {
            return Err(format!("inside the app bundle {}; poo never changes app bundles", app.display()));
        }
    }
    for c in p.components() {
        let s = c.as_os_str().to_string_lossy();
        if NEVER.iter().any(|n| *n == s) {
            return Err(format!("path goes through protected `{}`", s));
        }
        if s.starts_with(".env") {
            return Err("looks like an env file".into());
        }
    }
    let md = std::fs::symlink_metadata(&p).map_err(|_| "no longer exists".to_string())?;
    if md.file_type().is_symlink() {
        return Err("is a symlink".into());
    }
    if rule_kind == Some(RuleKind::Artifact) && md.is_dir() && p.join(".git").exists() {
        if mode < RiskLevel::Danger {
            return Err("contains a .git entry (needs --dangerously-include-protected)".into());
        }
        warnings.push("contains a .git entry".into());
    }
    if item.action.kind == ActionKind::WorktreeRemove && !p.join(".git").is_file() {
        return Err("is not a linked git worktree".into());
    }
    Ok(warnings)
}

/// Re-checks that an artifact still looks like what its rule describes.
fn still_matches(item: &Item, cat: &Catalog) -> Result<(), String> {
    let Some(rule) = cat.rule(&item.rule) else {
        return Err(format!("rule `{}` no longer exists", item.rule));
    };
    if !rule.enabled {
        return Err(format!("rule `{}` is disabled", item.rule));
    }
    if rule.kind == RuleKind::Artifact {
        let p = Path::new(&item.path);
        let parent = p.parent().ok_or("no parent")?;
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parent_names = names(&read_entries(parent));
        let child_names = names(&read_entries(p));
        if !rule.matches_artifact(&name, p, &parent_names, &child_names) {
            return Err("no longer matches its rule".into());
        }
        if rule.ignored {
            let ok = std::process::Command::new("git")
                .args(["--no-optional-locks", "-C", &parent.to_string_lossy(), "check-ignore", "-q", &item.path])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return Err("git no longer ignores it".into());
            }
        }
    }
    Ok(())
}

/// An exec guard that has not answered by then counts as "could not tell".
const GUARD_TIMEOUT_SECS: u64 = 30;

/// Guards veto cleaning. In danger mode they warn instead.
fn run_guards(item: &Item, guards: &[Guard], procs_list: &[Proc]) -> Result<(), String> {
    let project = item.project.clone().unwrap_or_else(|| item.path.clone());
    for g in guards {
        match g.kind.as_str() {
            "process" => {
                let scope = Path::new(&project);
                let hit = procs_list.iter().find(|p| {
                    p.cwd.starts_with(scope) && (procs::is_build_tool(&p.name, &[]) || g.names.contains(&p.name))
                });
                if let Some(p) = hit {
                    return Err(format!("{} (pid {}) is building in {}", p.name, p.pid, tilde(scope)));
                }
            }
            "file-nonempty" => {
                let Some(f) = &g.file else { continue };
                let path = expand(&f.replace("{project}", &project).replace("{path}", &item.path));
                if std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
                    return Err(format!("guard `{}`: {} is not empty", g.id, tilde(&path)));
                }
            }
            "exec" => {
                let Some(cmd) = &g.command else { continue };
                let mut argv = cmd.clone();
                if let Some(d) = &g.mod_dir {
                    let f = d.join(&argv[0]);
                    if f.exists() {
                        argv[0] = f.to_string_lossy().to_string();
                    }
                }
                let json = serde_json::to_string(item).unwrap_or_default();
                // Three answers: yes cleans, no blocks, and "could not tell" blocks too.
                match crate::util::run_probe(&argv, Some(json.as_bytes()), GUARD_TIMEOUT_SECS) {
                    crate::util::Probe::Yes => {}
                    crate::util::Probe::No(msg) => {
                        return Err(format!("guard `{}`: {}", g.id, if msg.is_empty() { "blocked".into() } else { msg }));
                    }
                    crate::util::Probe::Unknown(why) => {
                        return Err(format!("guard `{}` could not tell ({}); skipped to be safe", g.id, why));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn graveyard() -> PathBuf {
    state_dir().join("graveyard")
}


/// Renames into the graveyard (instant, same volume); the graveyard is erased right away or,
/// with `background`, by a detached `poo gc` after the clean returns.
fn delete_path(p: &Path, contents_only: bool, background: bool) -> Result<(), String> {
    if contents_only {
        for e in read_entries(p) {
            delete_path(&e.path, false, background)?;
        }
        return Ok(());
    }
    let md = std::fs::symlink_metadata(p).map_err(|e| e.to_string())?;
    if !md.is_dir() {
        return std::fs::remove_file(p).map_err(|e| e.to_string());
    }
    let grave = graveyard();
    let _ = std::fs::create_dir_all(&grave);
    let target = grave.join(format!("{}-{}", now(), crate::util::short_hash(&p.to_string_lossy())));
    match std::fs::rename(p, &target) {
        Ok(()) if background => Ok(()),
        Ok(()) => remove_tree(&target).map_err(|e| format!("moved to graveyard but delete failed: {}", e)),
        Err(_) => remove_tree(p),
    }
}

/// remove_dir_all, making read-only folders writable first (Go module cache, some SwiftPM checkouts).
fn remove_tree(p: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let _ = std::process::Command::new("chmod").args(["-R", "u+w", &p.to_string_lossy()]).status();
            std::fs::remove_dir_all(p).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn trash_path(p: &Path) -> Result<(), String> {
    let trash = home().join(".Trash");
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "item".into());
    let mut target = trash.join(&name);
    let mut n = 1;
    while target.exists() {
        target = trash.join(format!("{} {}", name, n));
        n += 1;
    }
    std::fs::rename(p, &target).map_err(|e| format!("could not move to Trash: {}", e))
}

/// Empties leftovers from interrupted deletes.
pub fn empty_graveyard() -> u64 {
    if read_only() {
        return 0;
    }
    let mut freed = 0;
    for e in read_entries(&graveyard()) {
        freed += crate::fsutil::SizeCtx::default()
            .size_path(&e.path, &Default::default())
            .map_or(0, |s| s.0.bytes);
        let _ = if e.is_dir { remove_tree(&e.path) } else { std::fs::remove_file(&e.path).map_err(|e| e.to_string()) };
    }
    freed
}

fn git_out(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Removes a linked worktree while keeping everything recoverable.
fn remove_worktree(item: &Item, opts: &CleanOptions, notes: &mut Vec<String>) -> Result<(), String> {
    let mode = opts.mode;
    let cfgb = &opts.backup;
    let repo = item.repo.clone().ok_or("worktree has no repo")?;
    let wt = Path::new(&item.path);
    let label = wt.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "worktree".into());
    let label = if label == Path::new(&repo).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() {
        // ~/.codex/worktrees/<task>/<repo>: the task folder names it.
        wt.parent().and_then(|p| p.file_name()).map(|n| format!("{}-{}", n.to_string_lossy(), label)).unwrap_or(label)
    } else {
        label
    };
    // 1. Copy small ignored files (.env, Local.xcconfig...) that removal would delete.
    if cfgb.config_files {
        let small = git::small_ignored_files(wt, cfgb.max_file_bytes);
        if !small.is_empty() {
            let dest = crate::backup::new_folder(&label);
            let (mut copied, mut total) = (0u64, 0u64);
            for f in small.iter() {
                let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                if total + size > cfgb.max_total_bytes {
                    break;
                }
                if let Ok(rel) = f.strip_prefix(wt) {
                    let to = dest.join(rel);
                    if std::fs::create_dir_all(to.parent().unwrap()).is_ok() && std::fs::copy(f, &to).is_ok() {
                        copied += 1;
                        total += size;
                    }
                }
            }
            if copied > 0 {
                let b = crate::backup::record(crate::backup::Backup {
                    id: String::new(),
                    at: now(),
                    kind: "files".into(),
                    label: label.clone(),
                    source: item.path.clone(),
                    item: Some(item.id.clone()),
                    path: Some(dest.to_string_lossy().to_string()),
                    repo: Some(repo.clone()),
                    r#ref: None,
                    files: copied,
                    bytes: total,
                    exists: true,
                });
                notes.push(format!("backed up {} ignored config file(s) to {} (backup {})", copied, tilde(&dest), b.id));
            }
        }
    }
    let has = |tag: &str| item.risks.iter().any(|r| r.tag == tag);
    // 2. Re-check uncommitted work now. Build output poo knows about does not count.
    let artifacts: Vec<PathBuf> = item
        .meta
        .get("artifacts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(PathBuf::from)).collect())
        .unwrap_or_default();
    let dirty = git::dirty_state(wt, &artifacts).ok_or("could not read git status")?;
    if dirty.is_dirty() {
        if mode < RiskLevel::Force {
            return Err(format!("has uncommitted work ({}); needs --include-in-use, which stashes it first", dirty.summary()));
        }
        if cfgb.stash_dirty {
            // Stash only the real work, never untracked build output, into the stash every
            // checkout of the repo shares.
            let msg = format!("poo: {} {}", label, iso8601(now()));
            let mut args: Vec<String> = vec!["stash".into(), "push".into(), "--include-untracked".into(), "-m".into(), msg.clone(), "--".into()];
            args.extend(dirty.modified.iter().cloned());
            args.extend(dirty.untracked.iter().cloned());
            let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            git_out(&item.path, &refs)?;
            let again = git::dirty_state(wt, &artifacts).ok_or("could not read git status after stashing")?;
            if again.is_dirty() {
                return Err(format!("stashed, but work remains ({}); left the worktree in place", again.summary()));
            }
            let b = crate::backup::record(crate::backup::Backup {
                id: String::new(),
                at: now(),
                kind: "stash".into(),
                label: label.clone(),
                source: item.path.clone(),
                item: Some(item.id.clone()),
                path: None,
                repo: Some(repo.clone()),
                r#ref: Some(msg.clone()),
                files: (dirty.modified.len() + dirty.untracked.len()) as u64,
                bytes: 0,
                exists: true,
            });
            notes.push(format!("stashed uncommitted work as \"{}\" in {} (backup {})", msg, tilde(Path::new(&repo)), b.id));
        } else if mode < RiskLevel::Danger {
            return Err("has uncommitted work and backup.stash_dirty is off; needs --dangerously-include-protected, which discards it".into());
        } else {
            notes.push(format!("discarded uncommitted work ({}) because backup.stash_dirty is off", dirty.summary()));
        }
    }
    // 3. Detached commits on no branch get a rescue branch.
    if has("unreachable-commits") {
        if cfgb.rescue_branches {
            let head = git_out(&item.path, &["rev-parse", "HEAD"])?;
            let branch = format!("poo/{}-{}", label, now());
            git_out(&repo, &["branch", &branch, &head])?;
            let b = crate::backup::record(crate::backup::Backup {
                id: String::new(),
                at: now(),
                kind: "branch".into(),
                label: label.clone(),
                source: item.path.clone(),
                item: Some(item.id.clone()),
                path: None,
                repo: Some(repo.clone()),
                r#ref: Some(branch.clone()),
                files: 0,
                bytes: 0,
                exists: true,
            });
            notes.push(format!("kept detached commits on branch {} (backup {})", branch, b.id));
        } else {
            notes.push("detached commits left to the reflog because backup.rescue_branches is off".into());
        }
    }
    if has("locked") {
        let _ = git_out(&repo, &["worktree", "unlock", &item.path]);
    }
    // 4. Remove. What is left is committed, stashed, ignored or known build output, so
    // --force only skips git's refusal over untracked build folders.
    git_out(&repo, &["worktree", "remove", "--force", &item.path]).map(|_| ())
}

/// Cleans one item. Returns the bytes measured as freed when the item says how to measure
/// (owner commands re-measure their folder, snapshots the free space), otherwise None.
fn perform(item: &Item, cat: &Catalog, opts: &CleanOptions, notes: &mut Vec<String>) -> Result<Option<u64>, String> {
    let p = Path::new(&item.path);
    match item.action.kind {
        ActionKind::None => Err("this item has no clean action".into()),
        ActionKind::Delete if opts.trash => trash_path(p).map(|_| None),
        ActionKind::Trash => trash_path(p).map(|_| None),
        ActionKind::Delete => {
            let contents = item.meta.get("contents").and_then(|v| v.as_bool()).unwrap_or(false);
            delete_path(p, contents, opts.background_delete).map(|_| None)
        }
        ActionKind::Command => {
            let argv = command_argv(item).ok_or("no command")?;
            let timeout = cat.rule(&item.rule).map_or(600, |r| r.timeout_secs);
            let before = measure(item);
            match crate::util::run_probe(&argv, None, timeout) {
                crate::util::Probe::Yes => {}
                crate::util::Probe::No(msg) if msg.is_empty() => return Err(format!("`{}` failed", shown(&argv))),
                crate::util::Probe::No(msg) => return Err(format!("`{}` failed: {}", shown(&argv), msg)),
                crate::util::Probe::Unknown(why) => return Err(why),
            }
            let after = measure(item);
            let freed = match (item.meta.get("measure").and_then(|m| m.as_str()), before, after) {
                (Some("purgeable"), Some(b), Some(a)) => Some(a.saturating_sub(b)),
                (_, Some(b), Some(a)) => Some(b.saturating_sub(a)),
                _ => None,
            };
            if let Some(f) = freed {
                notes.push(format!("measured {} freed", crate::util::bytes(f)));
            }
            Ok(freed)
        }
        ActionKind::WorktreeRemove => remove_worktree(item, opts, notes).map(|_| None),
    }
}

/// The command an item runs, with {path}, {id} and {name} filled in. Never through a shell.
pub fn command_argv(item: &Item) -> Option<Vec<String>> {
    let cmd = item.action.command.clone()?;
    Some(
        cmd.iter()
            .map(|a| {
                a.replace("{path}", &item.path)
                    .replace("{id}", item.meta.get("udid").and_then(|v| v.as_str()).unwrap_or(&item.id))
                    .replace("{name}", &item.name)
            })
            .collect(),
    )
}

/// A command for messages: the tool's name, not its full path.
fn shown(argv: &[String]) -> String {
    let mut v = argv.to_vec();
    if let Some(first) = v.first_mut() {
        if let Some(n) = Path::new(first.as_str()).file_name() {
            *first = n.to_string_lossy().to_string();
        }
    }
    v.join(" ")
}

/// What an item's `measure` hint says to look at: its folder's size, or free disk space.
fn measure(item: &Item) -> Option<u64> {
    match item.meta.get("measure").and_then(|m| m.as_str())? {
        "path" => Some(
            crate::fsutil::SizeCtx::default()
                .size_path(Path::new(&item.path), &Default::default())
                .map_or(0, |s| s.0.bytes),
        ),
        "purgeable" => Some(crate::fsutil::disk_info(&home()).free),
        _ => None,
    }
}

/// The rule's probe must say yes before its command runs. Three answers: yes runs, no and
/// "could not tell" (error, timeout, tool missing) both skip, with the reason.
pub fn probe_ok(item: &Item, cat: &Catalog) -> Result<(), String> {
    if item.action.kind != ActionKind::Command {
        return Ok(());
    }
    let Some(probe) = cat.rule(&item.rule).and_then(|r| r.probe.clone()).filter(|p| !p.is_empty()) else { return Ok(()) };
    let mut argv = probe.clone();
    if let Some(bin) = crate::util::which(&argv[0]) {
        argv[0] = bin.to_string_lossy().to_string();
    }
    match crate::util::run_probe(&argv, None, PROBE_TIMEOUT_SECS) {
        crate::util::Probe::Yes => Ok(()),
        crate::util::Probe::No(msg) => Err(format!(
            "`{}` said no{}; skipped",
            probe.join(" "),
            if msg.is_empty() { String::new() } else { format!(" ({})", msg) }
        )),
        crate::util::Probe::Unknown(why) => Err(format!("could not tell whether `{}` is ready ({}); skipped to be safe", probe.join(" "), why)),
    }
}

/// How long a probe may take before poo treats it as "could not tell".
const PROBE_TIMEOUT_SECS: u64 = 30;

/// Why an item cannot be cleaned in this mode, or the warnings accepted to clean it.
pub fn admit(item: &Item, cat: &Catalog, opts: &CleanOptions, procs_list: &[Proc]) -> Result<Vec<String>, String> {
    let mut warnings = vec![];
    // Report-only items are never cleaned, in any mode.
    if item.report_only || cat.rule(&item.rule).map_or(false, |r| r.report_only) {
        return Err(report_only_message(item));
    }
    // Snapshots are backups: never removed by a schedule or a hook, only when someone asks.
    let snapshot_cmd = item.action.command.as_ref().map_or(false, |c| c.iter().any(|a| a == "deletelocalsnapshots"));
    if snapshot_cmd && (opts.actor.starts_with("schedule") || opts.actor.starts_with("hook")) {
        return Err("Time Machine snapshots are only removed when you ask, never on a schedule".into());
    }
    if let Some(limit) = opts.max_tier {
        if item.tier.rank() > limit.rank() && opts.mode == RiskLevel::Standard {
            return Err(format!("{} item; plans and schedules stop at `{}` (pick it explicitly with --include-in-use)", item.tier.label(), limit.label()));
        }
    }
    for r in &item.risks {
        if r.level > opts.mode {
            return Err(format!("{} (needs {})", r.message, r.level.flag()));
        }
        if r.level > RiskLevel::Standard {
            warnings.push(format!("{}: {}", r.tag, r.message));
        }
    }
    let rule = cat.rule(&item.rule);
    warnings.extend(floor_check_rule(item, rule, opts.mode)?);
    if !item.path.contains("://") {
        if let Err(e) = still_matches(item, cat) {
            if opts.mode < RiskLevel::Danger || e == "no longer exists" {
                return Err(e);
            }
            warnings.push(e);
        }
    }
    if let Err(e) = run_guards(item, &cat.guards, procs_list) {
        if opts.mode < RiskLevel::Danger {
            return Err(format!("{} (needs --dangerously-include-protected)", e));
        }
        warnings.push(e);
    }
    Ok(warnings)
}

pub fn clean(items: &[Item], cat: &Catalog, opts: &CleanOptions) -> CleanResult {
    clean_with(items, cat, opts, None)
}

pub fn clean_with(items: &[Item], cat: &Catalog, opts: &CleanOptions, progress: Option<&dyn Fn(&CleanProgress)>) -> CleanResult {
    let forced;
    let opts = if read_only() && !opts.dry_run {
        forced = CleanOptions { dry_run: true, ..opts.clone() };
        &forced
    } else {
        opts
    };
    let total_count = items.len();
    let total_bytes: u64 = items.iter().map(|i| i.total_bytes).sum();
    let mut cancelled = false;
    if !opts.dry_run {
        crate::backup::enforce(&opts.backup);
    }
    let before = crate::fsutil::disk_info(&home()).free;
    // Unknown means skip: without the process list poo cannot tell whether a build or an
    // app is using an item.
    let procs_known = procs::try_list();
    let procs_unknown = procs_known.is_none();
    let procs_list = procs_known.unwrap_or_default();
    let mut results = vec![];
    let mut freed = 0u64;
    // Bytes of batch items nested in a worktree are counted on their own, not twice.
    let nested_in = |wt: &Item| -> u64 {
        let prefix = format!("{}/", wt.path);
        items.iter().filter(|o| o.path.starts_with(&prefix)).map(|o| o.bytes).sum()
    };
    // Nested artifacts first, worktrees last, so git worktree remove sees fewer files.
    let mut ordered: Vec<&Item> = items.iter().collect();
    ordered.sort_by_key(|i| (i.action.kind == ActionKind::WorktreeRemove, std::cmp::Reverse(i.path.len())));
    for item in ordered {
        if CANCEL.load(std::sync::atomic::Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        if let Some(cb) = progress {
            cb(&CleanProgress { event: "clean-progress", done: results.len(), total: total_count, freed_bytes: freed, total_bytes, current: item.name.clone(), status: "working".into() });
        }
        let mut res = ItemResult {
            id: item.id.clone(),
            path: item.path.clone(),
            name: item.name.clone(),
            status: "skipped".into(),
            bytes: if item.action.kind == ActionKind::WorktreeRemove { item.total_bytes.saturating_sub(nested_in(item)) } else { item.total_bytes },
            reason: None,
            warnings: vec![],
            notes: vec![],
        };
        let admitted = if procs_unknown {
            Err("could not read the list of running processes, so poo cannot tell whether something is using this; skipped to be safe".to_string())
        } else {
            admit(item, cat, opts, &procs_list)
        };
        match admitted {
            Err(e) => {
                if !opts.dry_run && (e.contains("could not tell") || procs_unknown) {
                    log_skip(item, &opts.actor, opts.mode, &e);
                }
                res.reason = Some(e);
                results.push(res);
                continue;
            }
            Ok(w) => res.warnings = w,
        }
        if let Err(e) = probe_ok(item, cat) {
            if !opts.dry_run && e.contains("could not tell") {
                log_skip(item, &opts.actor, opts.mode, &e);
            }
            res.reason = Some(e);
            results.push(res);
            continue;
        }
        if opts.dry_run {
            res.status = "would-clean".into();
            freed += res.bytes;
            results.push(res);
            continue;
        }
        let mut notes = vec![];
        match perform(item, cat, opts, &mut notes) {
            Ok(measured) => {
                if let Some(m) = measured {
                    res.bytes = m;
                }
                res.status = "cleaned".into();
                freed += res.bytes;
                log_history(item, &opts.actor, opts.mode, &notes);
            }
            Err(e) => {
                res.status = "failed".into();
                res.reason = Some(e);
            }
        }
        res.notes = notes;
        results.push(res);
    }
    if let Some(cb) = progress {
        let status = if cancelled { "cancelled" } else { "done" };
        cb(&CleanProgress { event: "clean-progress", done: results.len(), total: total_count, freed_bytes: freed, total_bytes, current: String::new(), status: status.into() });
    }
    if !opts.dry_run {
        // New backups may push past the budget.
        crate::backup::enforce(&opts.backup);
    }
    let graveyard_busy = !opts.dry_run && opts.background_delete && !read_entries(&graveyard()).is_empty();
    if graveyard_busy {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(["gc", "--json"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    let after = crate::fsutil::disk_info(&home()).free;
    let cleaned_bytes: u64 = results.iter().filter(|r| r.status == "cleaned").map(|r| r.bytes).sum();
    let snapshot_note = (!opts.dry_run && cleaned_bytes > 2_000_000_000 && after < before + cleaned_bytes / 4)
        .then(snapshot_hint)
        .flatten();
    CleanResult {
        cancelled,
        background_delete: graveyard_busy,
        dry_run: opts.dry_run,
        mode: opts.mode,
        freed_bytes: freed,
        cleaned: results.iter().filter(|r| r.status == "cleaned" || r.status == "would-clean").count(),
        skipped: results.iter().filter(|r| r.status == "skipped").count(),
        failed: results.iter().filter(|r| r.status == "failed").count(),
        disk_free_before: before,
        disk_free_after: after,
        results,
        snapshot_note,
        read_only: (read_only() && opts.dry_run).then(|| READ_ONLY_NOTE.to_string()),
    }
}

/// Local Time Machine snapshots keep deleted files on disk until macOS thins them.
pub fn snapshot_hint() -> Option<String> {
    let out = run_capture(&["tmutil", "listlocalsnapshots", "/"], 10)?;
    let n = out.lines().filter(|l| l.contains("com.apple")).count();
    (n > 0).then(|| {
        format!(
            "{} local Time Machine snapshot(s) still reference deleted files, so free space grows only after macOS thins them. To reclaim it now: tmutil thinlocalsnapshots / 999999999999 4",
            n
        )
    })
}

/// Items skipped because a probe or guard could not answer are kept in the history with
/// zero bytes, so "why was this left?" has an answer later.
fn log_skip(item: &Item, actor: &str, mode: RiskLevel, reason: &str) {
    append_history(serde_json::json!({
        "at": now(),
        "actor": actor,
        "mode": mode,
        "status": "skipped",
        "reason": reason,
        "id": item.id,
        "path": item.path,
        "name": item.name,
        "rule": item.rule,
        "bucket": item.bucket,
        "tier": item.tier,
        "bytes": 0,
    }));
}

fn append_history(line: serde_json::Value) {
    let p = state_dir().join("history.jsonl");
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{}", line);
    }
}

fn log_history(item: &Item, actor: &str, mode: RiskLevel, notes: &[String]) {
    let line = serde_json::json!({
        "at": now(),
        "actor": actor,
        "mode": mode,
        "id": item.id,
        "path": item.path,
        "name": item.name,
        "rule": item.rule,
        "bucket": item.bucket,
        "tier": item.tier,
        "bytes": item.total_bytes,
        "notes": notes,
    });
    let p = state_dir().join("history.jsonl");
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{}", line);
    }
}

pub fn history(limit: usize) -> Vec<serde_json::Value> {
    let p = state_dir().join("history.jsonl");
    let text = std::fs::read_to_string(p).unwrap_or_default();
    let mut v: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(limit);
    v
}
