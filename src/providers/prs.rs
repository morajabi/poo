//! Merged pull requests per branch, asked through the GitHub CLI and cached on disk so a
//! scan asks at most once per branch per `RECHECK` seconds. A merged answer never expires.

use super::git;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How long a "no merged PR yet" answer is reused.
const RECHECK: i64 = 20 * 60;
/// After gh fails (offline, signed out), how long before asking again.
const FAILED_RETRY: i64 = 2 * 3600;
/// Entries older than this are dropped when the cache is saved.
const FORGET: i64 = 60 * 86400;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    checked_at: i64,
    /// (number, merged head sha)
    merged: Option<(u64, String)>,
    #[serde(default)]
    failed: bool,
}

pub struct PrCache {
    path: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
    dirty: Mutex<bool>,
    gh: bool,
}

fn key(repo: &Path, branch: &str) -> String {
    format!("{}\t{}", repo.to_string_lossy(), branch)
}

impl PrCache {
    pub fn load(enabled: bool) -> Self {
        let path = crate::config::state_dir().join("prs.json");
        let entries = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let gh = enabled && std::env::var_os("POO_NO_GH").is_none() && crate::util::run_capture(&["gh", "--version"], 5).is_some();
        PrCache { path, entries: Mutex::new(entries), dirty: Mutex::new(false), gh }
    }

    /// The merged PR for `branch`, from the cache or from gh when the cached answer is old.
    pub fn merged(&self, repo: &Path, branch: &str) -> Option<(u64, String)> {
        let k = key(repo, branch);
        let now = crate::util::now();
        if let Some(e) = self.entries.lock().unwrap().get(&k) {
            let ttl = if e.failed { FAILED_RETRY } else { RECHECK };
            if e.merged.is_some() || now - e.checked_at < ttl || !self.gh {
                return e.merged.clone();
            }
        }
        if !self.gh {
            return None;
        }
        let answer = git::merged_pr(repo, branch);
        let entry = Entry { checked_at: now, merged: answer.clone().flatten(), failed: answer.is_none() };
        let merged = entry.merged.clone();
        self.entries.lock().unwrap().insert(k, entry);
        *self.dirty.lock().unwrap() = true;
        merged
    }

    pub fn save(&self) {
        if !*self.dirty.lock().unwrap() {
            return;
        }
        let now = crate::util::now();
        let mut entries = self.entries.lock().unwrap().clone();
        entries.retain(|_, e| now - e.checked_at < FORGET);
        if let Ok(text) = serde_json::to_string(&entries) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new("."))).is_ok() && std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}
