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
    let mut cmd = vec!["git", "-C", d.as_str()];
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

/// Last time someone worked in the checkout: index writes and HEAD moves.
pub fn last_activity(checkout: &Path) -> Option<i64> {
    let gd = git_dir(checkout)?;
    let mut best = None;
    for f in ["index", "HEAD", "logs/HEAD", "FETCH_HEAD", "ORIG_HEAD"] {
        if let Some(t) = crate::fsutil::mtime(&gd.join(f)) {
            best = Some(best.map_or(t, |b: i64| b.max(t)));
        }
    }
    best
}

pub fn is_dirty(checkout: &Path) -> Option<bool> {
    let out = git(checkout, &["status", "--porcelain", "--ignore-submodules=dirty", "-uno"], 20)?;
    if !out.trim().is_empty() {
        return Some(true);
    }
    // Untracked (not ignored) files are work too. Limit the cost on huge trees.
    let untracked = git(checkout, &["ls-files", "--others", "--exclude-standard", "--directory", "--no-empty-directory"], 20)?;
    Some(!untracked.trim().is_empty())
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

/// Whether `head` is already contained in the default branch (or its remote).
pub fn is_merged(repo: &Path, head: &str, default: &str) -> bool {
    let d = repo.to_string_lossy().to_string();
    let ok = |target: &str| {
        std::process::Command::new("git")
            .args(["-C", &d, "merge-base", "--is-ancestor", head, target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if ok(default) {
        return true;
    }
    let remote = if default.starts_with("origin/") { default.to_string() } else { format!("origin/{}", default) };
    ok(&remote)
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
