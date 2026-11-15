use crate::model::{BucketSpec, PlanSpec, ProjectPolicy, RuleSpec, Tier};
use crate::util::{expand, home};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `~/.config/poo` unless `POO_CONFIG_DIR` or `POO_HOME` says otherwise.
pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("POO_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("POO_HOME") {
        return PathBuf::from(p).join("config");
    }
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(p).join("poo");
    }
    home().join(".config/poo")
}

/// `~/.local/state/poo` for the scan cache, history and graveyard.
pub fn state_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("POO_STATE_DIR") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("POO_HOME") {
        return PathBuf::from(p).join("state");
    }
    if let Some(p) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(p).join("poo");
    }
    home().join(".local/state/poo")
}

pub fn mods_dir() -> PathBuf {
    config_dir().join("mods")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub policy: Option<ProjectPolicy>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentsConfig {
    /// Highest tier an agent may clean through MCP without `approved_by_user`.
    pub auto_clean_max_tier: Tier,
    /// Terminal app terminal launchers open (Terminal, iTerm, Ghostty, Warp...).
    pub terminal_app: String,
    /// Launcher `poo ask` uses when none is named.
    pub default_launcher: Option<String>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self { auto_clean_max_tier: Tier::Cold, terminal_app: "Terminal".into(), default_launcher: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScanConfig {
    pub max_depth: usize,
    /// Paths (globs allowed) never walked.
    pub exclude: Vec<String>,
    /// Items smaller than this are dropped from reports.
    pub min_item_bytes: u64,
    /// Default warm window for rules that do not set one.
    pub warm_days: u32,
    /// Reports younger than this are reused as they are; older ones update from the
    /// file-change journal first (usually well under a second).
    pub fresh_seconds: u64,
    /// When the journal cannot help (it needs a full walk), reports younger than this are
    /// still reused by `poo ls` and friends instead of blocking on a full walk.
    pub cache_minutes: u64,
    /// Rebuildable things untouched this many days (and with nobody working there) are idle.
    pub idle_days: u32,
    /// Per repo, this many most recently used checkouts keep their builds warm.
    pub keep_recent_checkouts: usize,
    /// Builds of other checkouts become idle after this many hours without use.
    pub idle_hours: u32,
    /// Builds of a worktree whose commits already landed go cold after this many idle hours.
    pub landed_idle_hours: u32,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            max_depth: 14,
            exclude: vec![],
            min_item_bytes: 1_000_000,
            warm_days: 14,
            fresh_seconds: 20,
            cache_minutes: 60,
            idle_days: 3,
            keep_recent_checkouts: 2,
            idle_hours: 6,
            landed_idle_hours: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdatesConfig {
    pub check: bool,
    pub channel: String,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { check: true, channel: "stable".into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SetupState {
    pub completed_at: Option<i64>,
    pub by: Option<String>,
    pub notes: Option<String>,
}

/// What poo saves before cleaning, and for how long.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BackupConfig {
    /// Copy small ignored files (.env, Local.xcconfig) before removing a worktree.
    pub config_files: bool,
    /// Largest single file copied.
    pub max_file_bytes: u64,
    /// Most copied per worktree.
    pub max_total_bytes: u64,
    /// Stash uncommitted work before a forced worktree removal (off: refuse instead).
    pub stash_dirty: bool,
    /// Save detached commits to a poo/ branch before a danger-mode removal.
    pub rescue_branches: bool,
    /// Move deleted folders to the Trash instead of deleting them.
    pub use_trash: bool,
    /// Delete poo's file backups older than this many days (0 keeps them until you prune).
    pub auto_prune_days: u32,
    /// Also drop poo's stashes and rescue branches after `auto_prune_days`.
    pub auto_prune_git: bool,
    /// Total size budget for file backups; the oldest go first when it is exceeded.
    pub max_backup_bytes: u64,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            config_files: true,
            max_file_bytes: 1_000_000,
            max_total_bytes: 50_000_000,
            stash_dirty: true,
            rescue_branches: true,
            use_trash: false,
            auto_prune_days: 90,
            auto_prune_git: false,
            max_backup_bytes: 2_000_000_000,
        }
    }
}

/// What happens when a coding agent finishes in a worktree (only once `poo hooks install` ran).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HooksConfig {
    /// landed: clean a worktree's build output once its commits are in the main branch.
    /// always: whenever a Claude Code session ends in a worktree. off: do nothing.
    pub on_agent_done: String,
    /// Also remove the worktree itself when its work landed and nothing is uncommitted.
    pub remove_landed_worktrees: bool,
    pub notify: bool,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self { on_agent_done: "landed".into(), remove_landed_worktrees: false, notify: true }
    }
}

/// A routine cleanup the scheduler runs on its own (standard mode only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub id: String,
    /// Plan to run: quick, stale, deep, emergency or one from a mod.
    pub plan: String,
    /// How often: 30m, 6h, 1d, 7d.
    pub every: String,
    /// Local time of day for daily or weekly runs, "03:30".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Only run when free space is below this ("50G" or "10%").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_free_below: Option<String>,
    /// Free least-regret items until this much is free ("80G" or "20%").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_free: Option<String>,
    /// Never clean more than this per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_buckets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_tags: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub notify: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub roots: Vec<String>,
    pub disabled_rules: Vec<String>,
    pub disabled_mods: Vec<String>,
    pub disabled_guards: Vec<String>,
    pub rule_overrides: BTreeMap<String, RuleSpec>,
    pub bucket: Vec<BucketSpec>,
    pub bucket_overrides: BTreeMap<String, BucketSpec>,
    pub plan_overrides: BTreeMap<String, PlanSpec>,
    pub projects: BTreeMap<String, ProjectConfig>,
    pub agents: AgentsConfig,
    pub scan: ScanConfig,
    pub updates: UpdatesConfig,
    pub setup: SetupState,
    pub backup: BackupConfig,
    pub hooks: HooksConfig,
    pub schedule: Vec<Schedule>,
}

/// Every setting with its default and a plain explanation, for `poo config keys`.
pub const KEYS: &[(&str, &str)] = &[
    ("roots", "Folders poo walks for projects. Empty: auto-detected (~/dev, ~/Developer, ~/code...)."),
    ("disabled_rules / disabled_mods / disabled_guards", "Turned-off rules, mods and guards. Prefer `poo rules disable --tag <pack>`."),
    ("rule_overrides.<id>.<field>", "Change any field of a built-in rule: warm_days, idle_days, bucket, enabled, tier..."),
    ("bucket / bucket_overrides.<id>", "Add buckets or rename, recolor, reorder or hide built-in ones."),
    ("plan_overrides.<id>", "Change a plan's tiers, max_cost, exclude_buckets or target."),
    ("projects.<path>.policy", "auto | active (keep warm) | stale (treat as unused) | protected (never touch)."),
    ("agents.auto_clean_max_tier", "Highest tier an agent may clean through MCP without asking (default cold)."),
    ("scan.warm_days", "Unused this many days counts as unused (default 14, rules can override)."),
    ("scan.idle_days", "Untouched this many days with nobody working there counts as idle (default 3)."),
    ("scan.keep_recent_checkouts", "Per repo, this many most recent checkouts keep their builds in use (default 2)."),
    ("scan.idle_hours", "Builds of other checkouts go idle after this many hours (default 6)."),
    ("scan.landed_idle_hours", "Builds of worktrees whose work landed go unused after this many hours (default 2)."),
    ("scan.max_depth / scan.exclude / scan.min_item_bytes / scan.fresh_seconds / scan.cache_minutes", "How deep and where poo walks, the smallest item shown, how long a report is reused before it updates from the file-change journal, and how long it is reused when a full walk would be needed."),
    ("backup.config_files", "Copy small ignored files (.env, Local.xcconfig) before removing a worktree (default on)."),
    ("backup.max_file_bytes / backup.max_total_bytes", "Largest file and most per worktree copied (1 MB / 50 MB)."),
    ("backup.stash_dirty", "Stash uncommitted work before an --include-in-use worktree removal (default on; off refuses)."),
    ("backup.rescue_branches", "Save detached commits to a poo/ branch before removing (default on)."),
    ("backup.use_trash", "Move deleted folders to the Trash instead of deleting (default off)."),
    ("backup.max_backup_bytes", "Budget for file backups; the oldest go first (default 2 GB)."),
    ("backup.auto_prune_days", "Drop file backups older than this (default 90; 0 keeps them until you prune)."),
    ("backup.auto_prune_git", "Also drop poo's stashes and rescue branches after auto_prune_days (default off)."),
    ("hooks.on_agent_done", "After `poo hooks install`: landed (default) | always | off."),
    ("hooks.remove_landed_worktrees", "Also remove a landed, clean worktree from the hook (default off)."),
    ("hooks.notify", "Show a notification after a hook cleans something (default on)."),
    ("schedule", "Routine cleanups; manage with `poo schedule` (presets nightly, quick, guard, weekly)."),
    ("updates.check", "Check for new versions once a day (default on)."),
];

/// Common places developers keep projects. Used when no roots are configured.
pub const CANDIDATE_ROOTS: &[&str] = &[
    "~/dev",
    "~/Developer",
    "~/code",
    "~/Code",
    "~/src",
    "~/projects",
    "~/Projects",
    "~/work",
    "~/repos",
    "~/git",
    "~/github",
    "~/GitHub",
    "~/workspace",
    "~/Workspace",
    "~/Sites",
    "~/Documents/GitHub",
    "~/Documents/Projects",
    "~/Documents/dev",
    "~/Documents/Code",
];

impl Config {
    pub fn load() -> Result<Config> {
        let path = config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        let text = format!(
            "# poo configuration. Edit freely, or use `poo roots`, `poo rules`, `poo buckets`,\n# `poo projects` and `poo config`. Docs: https://github.com/morajabi/poo/blob/main/docs/mods.md\n\n{}",
            toml::to_string_pretty(self)?
        );
        let tmp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Configured roots, or detected candidates when none are configured.
    pub fn effective_roots(&self) -> (Vec<PathBuf>, bool) {
        if !self.roots.is_empty() {
            let roots = self
                .roots
                .iter()
                .map(|r| expand(r))
                .filter(|p| p.is_dir())
                .collect();
            return (roots, false);
        }
        (detect_roots(), true)
    }

    pub fn policy_for(&self, project: &str) -> ProjectPolicy {
        self.projects
            .get(project)
            .and_then(|p| p.policy)
            .unwrap_or(ProjectPolicy::Auto)
    }
}

pub fn detect_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for c in CANDIDATE_ROOTS {
        let p = expand(c);
        let Ok(canon) = std::fs::canonicalize(&p) else { continue };
        // macOS volumes are usually case-insensitive: ~/Code and ~/code are one folder.
        let lower = PathBuf::from(canon.to_string_lossy().to_lowercase());
        let overlaps = out.iter().any(|o| {
            let o = PathBuf::from(o.to_string_lossy().to_lowercase());
            lower.starts_with(&o) || o.starts_with(&lower)
        });
        if !canon.is_dir() || overlaps {
            continue;
        }
        out.push(canon);
    }
    out
}
