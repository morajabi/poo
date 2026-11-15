//! Everything poo saves before cleaning: copies of small ignored files, stashes of
//! uncommitted work and rescue branches. Each is recorded so people can list, restore
//! and prune them later.

use crate::config::state_dir;
use crate::util::{now, short_hash, tilde};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub at: i64,
    /// files | stash | branch
    pub kind: String,
    pub label: String,
    /// The folder the backup came from.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Where copied files live (files kind).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Repository holding the stash or branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Branch name, or the stash message used to find the stash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
    /// Filled in when listing.
    #[serde(default)]
    pub exists: bool,
}

pub fn dir() -> PathBuf {
    state_dir().join("backups")
}

/// A new dated folder for one backup: backups/2026-11-15/194407-label.
pub fn new_folder(label: &str) -> PathBuf {
    let stamp = crate::util::iso8601(now());
    let (date, time) = stamp.split_once('T').unwrap_or((&stamp, ""));
    let time: String = time.chars().filter(|c| c.is_ascii_digit()).collect();
    let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    dir().join(date).join(format!("{}-{}", time, safe))
}

/// Applies the age limit and the size budget from config. Stashes and branches are only
/// pruned by age, and only when `auto_prune_git` is on.
pub fn enforce(cfg: &crate::config::BackupConfig) -> PruneResult {
    let mut res = PruneResult { pruned: vec![], failed: vec![], freed_bytes: 0, dry_run: false };
    if cfg.auto_prune_days > 0 {
        let r = prune(&[], Some(cfg.auto_prune_days), !cfg.auto_prune_git, false);
        res.pruned.extend(r.pruned);
        res.failed.extend(r.failed);
        res.freed_bytes += r.freed_bytes;
    }
    if cfg.max_backup_bytes > 0 {
        let mut files: Vec<Backup> = read_all().into_iter().filter(|b| b.kind == "files").collect();
        files.sort_by_key(|b| b.at);
        let mut total: u64 = files.iter().map(|b| b.bytes).sum();
        let mut drop = vec![];
        for b in &files {
            if total <= cfg.max_backup_bytes {
                break;
            }
            total -= b.bytes;
            drop.push(b.id.clone());
        }
        if !drop.is_empty() {
            let r = prune(&drop, None, true, false);
            res.pruned.extend(r.pruned);
            res.failed.extend(r.failed);
            res.freed_bytes += r.freed_bytes;
        }
    }
    res
}

pub fn total_bytes(list: &[Backup]) -> u64 {
    list.iter().map(|b| b.bytes).sum()
}

fn index_path() -> PathBuf {
    dir().join("index.jsonl")
}

pub fn record(mut b: Backup) -> Backup {
    if b.id.is_empty() {
        b.id = short_hash(&format!("{}|{}|{}|{:?}", b.kind, b.source, b.at, b.r#ref));
    }
    let _ = std::fs::create_dir_all(dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(index_path()) {
        let _ = writeln!(f, "{}", serde_json::to_string(&b).unwrap_or_default());
    }
    b
}

fn read_all() -> Vec<Backup> {
    std::fs::read_to_string(index_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn write_all(list: &[Backup]) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = index_path().with_extension("jsonl.tmp");
    let mut text = String::new();
    for b in list {
        text.push_str(&serde_json::to_string(b)?);
        text.push('\n');
    }
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, index_path())?;
    Ok(())
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// `stash@{n}` currently holding the stash with this message.
fn find_stash(repo: &str, message: &str) -> Option<String> {
    let out = git(repo, &["stash", "list", "--format=%gd%x00%gs"])?;
    out.lines().find_map(|l| {
        let (r, subject) = l.split_once('\0')?;
        subject.ends_with(message).then(|| r.to_string())
    })
}

fn exists(b: &Backup) -> bool {
    match b.kind.as_str() {
        "files" => b.path.as_deref().map_or(false, |p| Path::new(p).exists()),
        "stash" => match (&b.repo, &b.r#ref) {
            (Some(repo), Some(msg)) => find_stash(repo, msg).is_some(),
            _ => false,
        },
        "branch" => match (&b.repo, &b.r#ref) {
            (Some(repo), Some(br)) => git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", br)]).is_some(),
            _ => false,
        },
        _ => false,
    }
}

/// All recorded backups that still exist, newest first.
pub fn list() -> Vec<Backup> {
    let mut v: Vec<Backup> = read_all()
        .into_iter()
        .map(|mut b| {
            b.exists = exists(&b);
            b
        })
        .filter(|b| b.exists)
        .collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

fn remove(b: &Backup) -> Result<()> {
    match b.kind.as_str() {
        "files" => {
            let p = b.path.as_deref().ok_or_else(|| anyhow!("backup has no path"))?;
            let p = Path::new(p);
            if !p.starts_with(dir()) {
                bail!("refusing to delete {} outside poo's backups folder", tilde(p));
            }
            std::fs::remove_dir_all(p)?;
            if let Some(parent) = p.parent() {
                let _ = std::fs::remove_dir(parent); // empty date folder
            }
        }
        "stash" => {
            let (repo, msg) = (b.repo.as_deref().unwrap_or(""), b.r#ref.as_deref().unwrap_or(""));
            let r = find_stash(repo, msg).ok_or_else(|| anyhow!("stash not found"))?;
            git(repo, &["stash", "drop", &r]).ok_or_else(|| anyhow!("git stash drop {} failed", r))?;
        }
        "branch" => {
            let (repo, br) = (b.repo.as_deref().unwrap_or(""), b.r#ref.as_deref().unwrap_or(""));
            if !br.starts_with("poo/") {
                bail!("refusing to delete branch {} not created by poo", br);
            }
            git(repo, &["branch", "-D", br]).ok_or_else(|| anyhow!("git branch -D {} failed", br))?;
        }
        k => bail!("unknown backup kind {}", k),
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PruneResult {
    pub pruned: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub freed_bytes: u64,
    pub dry_run: bool,
}

/// Prunes backups by id, or every backup older than `older_than_days` (files kind only when
/// `files_only`, which automatic pruning uses so stashes and branches stay until a person acts).
pub fn prune(ids: &[String], older_than_days: Option<u32>, files_only: bool, dry_run: bool) -> PruneResult {
    let all = read_all();
    let cutoff = older_than_days.map(|d| now() - d as i64 * 86400);
    let selected = |b: &Backup| {
        (ids.contains(&b.id) || cutoff.map_or(false, |c| b.at < c)) && (!files_only || b.kind == "files")
    };
    let mut res = PruneResult { pruned: vec![], failed: vec![], freed_bytes: 0, dry_run };
    let mut keep = vec![];
    for b in all {
        if !selected(&b) {
            keep.push(b);
            continue;
        }
        if dry_run {
            res.pruned.push(b.id.clone());
            res.freed_bytes += b.bytes;
            keep.push(b);
            continue;
        }
        match remove(&b) {
            Ok(()) => {
                res.pruned.push(b.id.clone());
                res.freed_bytes += b.bytes;
            }
            Err(e) if !exists(&b) => {
                // Already gone: drop the record.
                let _ = e;
                res.pruned.push(b.id.clone());
            }
            Err(e) => {
                res.failed.push((b.id.clone(), e.to_string()));
                keep.push(b);
            }
        }
    }
    if !dry_run {
        let _ = write_all(&keep);
    }
    res
}

/// Copies a files backup back into its source folder without overwriting anything.
pub fn restore(id: &str) -> Result<(String, u64)> {
    let b = read_all().into_iter().find(|b| b.id == id).ok_or_else(|| anyhow!("no backup `{}`", id))?;
    match b.kind.as_str() {
        "files" => {
            let from = PathBuf::from(b.path.clone().ok_or_else(|| anyhow!("backup has no path"))?);
            let to = PathBuf::from(&b.source);
            if !to.is_dir() {
                bail!("{} no longer exists; the files are in {}", tilde(&to), tilde(&from));
            }
            let mut n = 0;
            for f in walk(&from) {
                let rel = f.strip_prefix(&from)?;
                let dest = to.join(rel);
                if dest.exists() {
                    continue;
                }
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::copy(&f, &dest)?;
                n += 1;
            }
            Ok((format!("restored {} file(s) into {}", n, tilde(&to)), n))
        }
        "stash" => Ok((
            format!(
                "recreate the worktree, then in it run: git stash apply \"{}\" (find it with git stash list in {})",
                find_stash(b.repo.as_deref().unwrap_or(""), b.r#ref.as_deref().unwrap_or("")).unwrap_or_default(),
                tilde(Path::new(b.repo.as_deref().unwrap_or("")))
            ),
            0,
        )),
        "branch" => Ok((
            format!("the commits are on branch {}: git worktree add <path> {}", b.r#ref.clone().unwrap_or_default(), b.r#ref.clone().unwrap_or_default()),
            0,
        )),
        _ => bail!("unknown backup kind"),
    }
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
