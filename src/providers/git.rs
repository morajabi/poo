//! Git repos, linked worktrees and ignored folders.

use crate::util::run_capture;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
    pub is_main: bool,
}

pub fn git(dir: &Path, args: &[&str], timeout: u64) -> Option<String> {
    let d = dir.to_string_lossy().to_string();
    // --no-optional-locks: never refresh .git/index, which would make poo itself look like activity.
    let mut cmd = vec!["git", "--no-optional-locks", "-C", d.as_str()];
    cmd.extend_from_slice(args);
    run_capture(&cmd, timeout)
}

pub fn worktrees(repo: &Path) -> Vec<WorktreeInfo> {
    let Some(out) = git(repo, &["worktree", "list", "--porcelain"], 10) else { return vec![] };
    let mut list = vec![];
    let mut cur: Option<WorktreeInfo> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            cur = Some(WorktreeInfo {
                path: PathBuf::from(p),
                head: None,
                branch: None,
                detached: false,
                locked: false,
                prunable: false,
                is_main: list.is_empty(),
            });
        } else if let Some(w) = cur.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.trim_start_matches("refs/heads/").to_string());
            } else if line == "detached" {
                w.detached = true;
            } else if line.starts_with("locked") {
                w.locked = true;
            } else if line.starts_with("prunable") {
                w.prunable = true;
            }
        }
    }
    if let Some(w) = cur {
        list.push(w);
    }
    list
}

/// Resolves the git dir of a checkout: `.git` dir, or the target of a `.git` file.
pub fn git_dir(checkout: &Path) -> Option<PathBuf> {
    let dot = checkout.join(".git");
    let md = std::fs::symlink_metadata(&dot).ok()?;
    if md.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let p = text.trim().strip_prefix("gitdir:")?.trim();
    let p = PathBuf::from(p);
    Some(if p.is_absolute() { p } else { checkout.join(p) })
}

/// Last commit, checkout, merge or rebase in the checkout. The index is not used: any tool
/// running `git status` rewrites it, so it says nothing about real work.
pub fn last_activity(checkout: &Path) -> Option<i64> {
    let gd = git_dir(checkout)?;
    let mut best = None;
    for f in ["HEAD", "logs/HEAD", "ORIG_HEAD"] {
        if let Some(t) = crate::fsutil::mtime(&gd.join(f)) {
            best = Some(best.map_or(t, |b: i64| b.max(t)));
        }
    }
    best
}

/// Uncommitted work in a checkout: modified tracked files, and untracked paths that are
/// not build output poo already knows about (agents often leave .dd or build/ unignored).
pub struct DirtyState {
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
}

impl DirtyState {
    pub fn is_dirty(&self) -> bool {
        !self.modified.is_empty() || !self.untracked.is_empty()
    }
    pub fn summary(&self) -> String {
        let mut parts = vec![];
        let m = self.modified.len();
        if m > 0 {
            parts.push(format!("{} modified file{}", m, if m == 1 { "" } else { "s" }));
        }
        if !self.untracked.is_empty() {
            let shown: Vec<&str> = self.untracked.iter().take(3).map(|s| s.as_str()).collect();
            parts.push(format!("untracked {}{}", shown.join(", "), if self.untracked.len() > 3 { ", ..." } else { "" }));
        }
        parts.join("; ")
    }
}

pub fn dirty_state(checkout: &Path, artifacts: &[PathBuf]) -> Option<DirtyState> {
    let out = git(checkout, &["status", "--porcelain", "--ignore-submodules=dirty", "-uno"], 20)?;
    let modified: Vec<String> = out
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let p = &l[3..];
            // Renames print "old -> new"; the new path is what exists.
            p.rsplit(" -> ").next().unwrap_or(p).trim_matches('"').to_string()
        })
        .collect();
    let untracked_out = git(checkout, &["ls-files", "--others", "--exclude-standard", "--directory", "--no-empty-directory"], 20)?;
    let untracked = untracked_out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let abs = checkout.join(l.trim_end_matches('/'));
            !artifacts.iter().any(|a| abs.starts_with(a) || a.starts_with(&abs))
        })
        .map(String::from)
        .collect();
    Some(DirtyState { modified, untracked })
}

pub fn is_dirty(checkout: &Path) -> Option<bool> {
    dirty_state(checkout, &[]).map(|d| d.is_dirty())
}

pub fn default_branch(repo: &Path) -> Option<String> {
    if let Some(out) = git(repo, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], 5) {
        let b = out.trim().to_string();
        if !b.is_empty() {
            return Some(b);
        }
    }
    for b in ["main", "master", "trunk", "develop"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", b], 5).is_some() {
            return Some(b.to_string());
        }
    }
    None
}

/// Whether `head` is already contained in the default branch, locally or on the remote.
pub fn is_merged(repo: &Path, head: &str, default: &str) -> bool {
    let d = repo.to_string_lossy().to_string();
    let ok = |target: &str| {
        std::process::Command::new("git")
            .args(["--no-optional-locks", "-C", &d, "merge-base", "--is-ancestor", head, target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let local = default.trim_start_matches("origin/").to_string();
    let remote = format!("origin/{}", local);
    [default.to_string(), local, remote].iter().any(|t| ok(t))
}

/// Whether a commit is reachable from any branch or remote branch.
pub fn reachable(repo: &Path, head: &str) -> bool {
    git(repo, &["for-each-ref", "--contains", head, "--count=1", "refs/heads", "refs/remotes"], 10)
        .map_or(false, |o| !o.trim().is_empty())
}

/// Ignored files (not folders) small enough to be config: .env, Local.xcconfig, keys.
pub fn small_ignored_files(checkout: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let Some(out) = git(checkout, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"], 30) else {
        return vec![];
    };
    out.split('\0')
        .filter(|s| !s.is_empty() && !s.ends_with('/'))
        .map(|s| checkout.join(s))
        .filter(|p| std::fs::symlink_metadata(p).map(|m| m.is_file() && m.len() <= max_bytes).unwrap_or(false))
        .collect()
}

/// Ignored directories at the top of each ignored subtree, relative to the repo.
pub fn ignored_dirs(repo: &Path) -> Vec<PathBuf> {
    let Some(out) = git(
        repo,
        &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"],
        30,
    ) else {
        return vec![];
    };
    out.split('\0')
        .filter(|s| s.ends_with('/'))
        .map(|s| repo.join(s.trim_end_matches('/')))
        .collect()
}

pub fn agent_for_path(path: &Path, homes: &[(PathBuf, String)]) -> Option<String> {
    for (home, agent) in homes {
        if path.starts_with(home) {
            return Some(agent.clone());
        }
    }
    let s = path.to_string_lossy();
    let guesses = [
        ("/.claude/worktrees/", "claude"),
        ("/.codex/worktrees/", "codex"),
        ("/conductor/", "conductor"),
        ("/.cursor/worktrees/", "cursor"),
        ("/.opencode/", "opencode"),
        ("/.t3/", "t3code"),
        ("/.superset/", "superset"),
        ("/.crystal/", "crystal"),
        ("/.vibe-kanban/", "vibe-kanban"),
        ("/.sculptor/", "sculptor"),
        ("/.terragon/", "terragon"),
    ];
    guesses.iter().find(|(m, _)| s.contains(m)).map(|(_, a)| a.to_string())
}
