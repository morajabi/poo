//! Cleaning with a hard safety floor that no mod or config can disable.

use crate::catalog::{Catalog, Guard};
use crate::config::state_dir;
use crate::fsutil::{names, read_entries};
use crate::model::{ActionKind, Item, RuleKind, Tier};
use crate::procs::{self, Proc};
use crate::util::{expand, home, now, run_capture};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct CleanOptions {
    pub dry_run: bool,
    /// Move to the Trash instead of deleting (filesystem items only).
    pub trash: bool,
    /// Highest tier allowed. Items above it are skipped.
    pub max_tier: Option<Tier>,
    /// Allow hot and review items when named explicitly.
    pub force: bool,
    /// Who asked: cli, app, mcp.
    pub actor: String,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanResult {
    pub dry_run: bool,
    pub freed_bytes: u64,
    pub cleaned: usize,
    pub skipped: usize,
    pub failed: usize,
    pub disk_free_before: u64,
    pub disk_free_after: u64,
    pub results: Vec<ItemResult>,
}

const NEVER: &[&str] = &[
    ".ssh", ".gnupg", "Keychains", "Mobile Documents", ".password-store", ".aws", ".kube",
    "1Password", "com.apple.TCC", "Photos Library.photoslibrary", "Mail",
];

/// The safety floor. Returns why a path must not be touched.
pub fn floor_check(item: &Item, rule_kind: Option<RuleKind>, rule_system: bool) -> Result<(), String> {
    if item.path.contains("://") {
        return match item.action.kind {
            ActionKind::Command => Ok(()),
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
    // Resolve symlinked parents (/var -> /private/var) but never the item itself.
    let p = match (raw.parent().and_then(|d| std::fs::canonicalize(d).ok()), raw.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => raw.clone(),
    };
    let h = home();
    let tmp = std::env::var("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    let inside_home = p.starts_with(&h) && p != h;
    let inside_tmp = p.starts_with(&tmp) && p != tmp;
    if !(inside_home || inside_tmp || rule_system) {
        return Err("outside your home and temp folders".into());
    }
    // At least two levels below home, so ~/Library or ~/dev themselves are never targets.
    if inside_home {
        let depth = p.strip_prefix(&h).map(|r| r.components().count()).unwrap_or(0);
        if depth < 2 {
            return Err("too close to your home folder".into());
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
        return Err("contains a .git entry".into());
    }
    if item.action.kind == ActionKind::WorktreeRemove && !p.join(".git").is_file() {
        return Err("is not a linked git worktree".into());
    }
    Ok(())
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
                .args(["-C", &parent.to_string_lossy(), "check-ignore", "-q", &item.path])
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

fn run_guards(item: &Item, guards: &[Guard], procs_list: &[Proc]) -> Result<(), String> {
    let project = item.project.clone().unwrap_or_else(|| item.path.clone());
    for g in guards {
        match g.kind.as_str() {
            "process" => {
                let scope = Path::new(&project);
                let hits: Vec<&Proc> = procs_list
                    .iter()
                    .filter(|p| p.cwd.starts_with(scope) && (procs::is_build_tool(&p.name, &[]) || g.names.contains(&p.name)))
                    .collect();
                if let Some(p) = hits.first() {
                    return Err(format!("{} (pid {}) is running in {}", p.name, p.pid, crate::util::tilde(scope)));
                }
            }
            "file-nonempty" => {
                let Some(f) = &g.file else { continue };
                let path = expand(&f.replace("{project}", &project).replace("{path}", &item.path));
                if std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
                    return Err(format!("guard `{}`: {} is not empty", g.id, crate::util::tilde(&path)));
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
                let out = std::process::Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .and_then(|mut c| {
                        if let Some(mut s) = c.stdin.take() {
                            let _ = s.write_all(json.as_bytes());
                        }
                        c.wait_with_output()
                    });
                match out {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        let msg = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        return Err(format!("guard `{}`: {}", g.id, if msg.is_empty() { "blocked".into() } else { msg }));
                    }
                    Err(e) => return Err(format!("guard `{}` failed to run: {}", g.id, e)),
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

/// Renames into the graveyard (instant, same volume) then deletes.
fn delete_path(p: &Path, contents_only: bool) -> Result<(), String> {
    if contents_only {
        for e in read_entries(p) {
            delete_path(&e.path, false)?;
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
        Ok(()) => std::fs::remove_dir_all(&target).map_err(|e| format!("moved to graveyard but delete failed: {}", e)),
        Err(_) => std::fs::remove_dir_all(p).map_err(|e| e.to_string()),
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
    let mut freed = 0;
    for e in read_entries(&graveyard()) {
        freed += crate::fsutil::SizeCtx::default()
            .size_path(&e.path, &Default::default())
            .map_or(0, |s| s.0.bytes);
        let _ = if e.is_dir { std::fs::remove_dir_all(&e.path) } else { std::fs::remove_file(&e.path) };
    }
    freed
}

fn perform(item: &Item, opts: &CleanOptions) -> Result<(), String> {
    let p = Path::new(&item.path);
    match item.action.kind {
        ActionKind::None => Err("this item has no clean action".into()),
        ActionKind::Delete if opts.trash => trash_path(p),
        ActionKind::Trash => trash_path(p),
        ActionKind::Delete => {
            let contents = item.meta.get("contents").and_then(|v| v.as_bool()).unwrap_or(false);
            delete_path(p, contents)
        }
        ActionKind::Command => {
            let cmd = item.action.command.clone().ok_or("no command")?;
            let argv: Vec<String> = cmd
                .iter()
                .map(|a| a.replace("{path}", &item.path).replace("{id}", item.meta.get("udid").and_then(|v| v.as_str()).unwrap_or(&item.id)).replace("{name}", &item.name))
                .collect();
            let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
            run_capture(&refs, 600).map(|_| ()).ok_or_else(|| format!("`{}` failed", argv.join(" ")))
        }
        ActionKind::WorktreeRemove => {
            let repo = item.repo.clone().ok_or("worktree has no repo")?;
            let out = std::process::Command::new("git")
                .args(["-C", &repo, "worktree", "remove", &item.path])
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
    }
}

pub fn clean(items: &[Item], cat: &Catalog, opts: &CleanOptions) -> CleanResult {
    let before = crate::fsutil::disk_info(&home()).free;
    let procs_list = procs::list();
    let mut results = vec![];
    let mut freed = 0u64;
    // Nested artifacts first, worktrees last, so git worktree remove sees fewer files.
    // Bytes of batch items nested in a worktree are counted on their own, not twice.
    let nested_in = |wt: &Item| -> u64 {
        let prefix = format!("{}/", wt.path);
        items.iter().filter(|o| o.path.starts_with(&prefix)).map(|o| o.bytes).sum()
    };
    let mut ordered: Vec<&Item> = items.iter().collect();
    ordered.sort_by_key(|i| (i.action.kind == ActionKind::WorktreeRemove, std::cmp::Reverse(i.path.len())));
    for item in ordered {
        let mut res = ItemResult {
            id: item.id.clone(),
            path: item.path.clone(),
            name: item.name.clone(),
            status: "skipped".into(),
            bytes: if item.action.kind == ActionKind::WorktreeRemove { item.total_bytes.saturating_sub(nested_in(item)) } else { item.total_bytes },
            reason: None,
        };
        let rule = cat.rule(&item.rule);
        let check = (|| -> Result<(), String> {
            if item.tier == Tier::Protected {
                return Err(format!("protected: {}", item.reasons.join("; ")));
            }
            let limit = opts.max_tier.unwrap_or(Tier::Warm);
            if item.tier.rank() > limit.rank() && !(opts.force && matches!(item.tier, Tier::Hot | Tier::Review)) {
                return Err(format!("{} item; pass --force to clean hot or review items", item.tier.as_str()));
            }
            floor_check(item, rule.map(|r| r.kind), rule.map_or(false, |r| r.system))?;
            if !item.path.contains("://") {
                still_matches(item, cat)?;
            }
            run_guards(item, &cat.guards, &procs_list)?;
            Ok(())
        })();
        if let Err(e) = check {
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
        match perform(item, opts) {
            Ok(()) => {
                res.status = "cleaned".into();
                freed += res.bytes;
                log_history(item, &opts.actor);
            }
            Err(e) => {
                res.status = "failed".into();
                res.reason = Some(e);
            }
        }
        results.push(res);
    }
    let after = crate::fsutil::disk_info(&home()).free;
    CleanResult {
        dry_run: opts.dry_run,
        freed_bytes: freed,
        cleaned: results.iter().filter(|r| r.status == "cleaned" || r.status == "would-clean").count(),
        skipped: results.iter().filter(|r| r.status == "skipped").count(),
        failed: results.iter().filter(|r| r.status == "failed").count(),
        disk_free_before: before,
        disk_free_after: after,
        results,
    }
}

fn log_history(item: &Item, actor: &str) {
    let line = serde_json::json!({
        "at": now(),
        "actor": actor,
        "id": item.id,
        "path": item.path,
        "rule": item.rule,
        "bucket": item.bucket,
        "tier": item.tier,
        "bytes": item.total_bytes,
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
