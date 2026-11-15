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
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self { auto_clean_max_tier: Tier::Cold }
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
    /// Scan results younger than this are reused by `poo ls` and friends.
    pub cache_minutes: u64,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            max_depth: 14,
            exclude: vec![],
            min_item_bytes: 1_000_000,
            warm_days: 14,
            cache_minutes: 60,
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
}

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
