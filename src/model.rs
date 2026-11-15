//! Data shared by the engine, the CLI, MCP and the macOS app (as JSON).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Junk,
    Cold,
    Warm,
    Hot,
    Review,
    Protected,
}

impl Tier {
    pub fn all() -> [Tier; 6] {
        [Tier::Junk, Tier::Cold, Tier::Warm, Tier::Hot, Tier::Review, Tier::Protected]
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Junk => "junk",
            Tier::Cold => "cold",
            Tier::Warm => "warm",
            Tier::Hot => "hot",
            Tier::Review => "review",
            Tier::Protected => "protected",
        }
    }
    pub fn parse(s: &str) -> Option<Tier> {
        Tier::all().into_iter().find(|t| t.as_str() == s)
    }
    /// How willing poo is to clean this tier; lower is safer.
    pub fn rank(&self) -> u8 {
        match self {
            Tier::Junk => 0,
            Tier::Cold => 1,
            Tier::Warm => 2,
            Tier::Hot => 3,
            Tier::Review => 4,
            Tier::Protected => 5,
        }
    }
    pub fn describe(&self) -> &'static str {
        match self {
            Tier::Junk => "junk, nothing to rebuild",
            Tier::Cold => "unused for a while or orphaned",
            Tier::Warm => "used recently, will be rebuilt or redownloaded",
            Tier::Hot => "in use right now",
            Tier::Review => "needs a human look",
            Tier::Protected => "never cleaned automatically",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cost {
    None,
    Rebuild,
    Redownload,
    Data,
}

impl Cost {
    pub fn as_str(&self) -> &'static str {
        match self {
            Cost::None => "none",
            Cost::Rebuild => "rebuild",
            Cost::Redownload => "redownload",
            Cost::Data => "data",
        }
    }
    pub fn rank(&self) -> u8 {
        match self {
            Cost::None => 0,
            Cost::Rebuild => 1,
            Cost::Redownload => 2,
            Cost::Data => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleKind {
    Artifact,
    Path,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    Delete,
    Trash,
    Command,
    WorktreeRemove,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectPolicy {
    Auto,
    Active,
    Stale,
    Protected,
}

impl ProjectPolicy {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "active" => Some(Self::Active),
            "stale" => Some(Self::Stale),
            "protected" => Some(Self::Protected),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Mod file specs. Every field is optional so a later mod can override only
// what it sets.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub bucket: Option<String>,
    pub group: Option<String>,
    pub kind: Option<RuleKind>,
    pub description: Option<String>,
    pub recommend: Option<String>,
    pub tags: Option<Vec<String>>,
    pub enabled: Option<bool>,
    // artifact matching
    pub names: Option<Vec<String>>,
    pub parent_any: Option<Vec<String>>,
    pub parent_all: Option<Vec<String>>,
    pub parent_none: Option<Vec<String>>,
    pub children_any: Option<Vec<String>>,
    pub children_all: Option<Vec<String>>,
    pub children_none: Option<Vec<String>>,
    pub path_contains: Option<Vec<String>>,
    pub path_excludes: Option<Vec<String>>,
    /// Only match when git ignores the folder (for generic names like dist or build).
    pub ignored: Option<bool>,
    // path matching
    pub paths: Option<Vec<String>>,
    pub each: Option<bool>,
    pub files: Option<bool>,
    pub extensions: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub min_age_days: Option<u32>,
    pub min_bytes: Option<u64>,
    pub system: Option<bool>,
    /// Clean the folder's contents but keep the folder itself.
    pub contents: Option<bool>,
    /// Process names; when any runs, matching items are hot (apps rewrite their caches live).
    pub running_apps: Option<Vec<String>>,
    // provider
    pub provider: Option<String>,
    pub params: Option<toml::Table>,
    // classification and action
    pub cost: Option<Cost>,
    pub tier: Option<Tier>,
    pub rebuild: Option<String>,
    pub warm_days: Option<u32>,
    pub action: Option<ActionKind>,
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BucketSpec {
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub order: Option<i32>,
    pub color: Option<String>,
    pub symbol: Option<String>,
    pub description: Option<String>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardSpec {
    pub id: String,
    pub kind: String,
    pub description: Option<String>,
    pub file: Option<String>,
    pub names: Option<Vec<String>>,
    pub command: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootSpec {
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeHomeSpec {
    pub path: String,
    pub agent: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModFile {
    // manifest fields, only meaningful in mod.toml
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
    #[serde(default)]
    pub rule: Vec<RuleSpec>,
    #[serde(default)]
    pub bucket: Vec<BucketSpec>,
    #[serde(default)]
    pub guard: Vec<GuardSpec>,
    #[serde(default)]
    pub root: Vec<RootSpec>,
    #[serde(default)]
    pub worktree_home: Vec<WorktreeHomeSpec>,
    #[serde(default)]
    pub plan: Vec<PlanSpec>,
    #[serde(default)]
    pub stack: Vec<StackSpec>,
}

/// Marker files that identify a project's stack (rust, swift, node, ...).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackSpec {
    pub id: String,
    pub name: Option<String>,
    pub markers: Vec<String>,
}

/// Recommended cleaning plans are data too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSpec {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub order: Option<i32>,
    /// Tiers this plan includes.
    pub tiers: Option<Vec<Tier>>,
    /// Highest cost this plan accepts.
    pub max_cost: Option<Cost>,
    /// Buckets to leave out of this plan.
    pub exclude_buckets: Option<Vec<String>>,
    /// Greedy plans pick the least regrettable items until the target is met.
    pub greedy: Option<bool>,
    /// Default target in bytes for greedy plans, or a percentage of the disk with `target_free_percent`.
    pub target_free_percent: Option<u32>,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Child {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub path: String,
    pub name: String,
    pub rule: String,
    pub bucket: String,
    pub group: String,
    /// Bytes counted in totals. Disjoint from every other item.
    pub bytes: u64,
    /// Bytes freed when this item is cleaned, including nested items (worktrees).
    pub total_bytes: u64,
    pub files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
    pub tier: Tier,
    pub cost: Cost,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub action: Action,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<Child>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub meta: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    pub name: String,
    pub bytes: u64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSummary {
    pub id: String,
    pub name: String,
    pub order: i32,
    pub color: String,
    pub symbol: String,
    pub description: String,
    pub hidden: bool,
    pub bytes: u64,
    pub count: usize,
    pub tiers: BTreeMap<String, u64>,
    pub groups: Vec<GroupSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub path: String,
    pub name: String,
    /// repo | worktree | folder
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub stacks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<i64>,
    pub artifact_bytes: u64,
    pub item_count: usize,
    pub policy: ProjectPolicy,
    /// active | idle | stale
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub bytes: u64,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskInfo {
    pub total: u64,
    pub free: u64,
    pub used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub version: String,
    pub scanned_at: i64,
    pub duration_ms: u64,
    pub disk: DiskInfo,
    pub roots: Vec<String>,
    pub total_bytes: u64,
    pub buckets: Vec<BucketSummary>,
    pub plans: Vec<PlanSummary>,
    pub projects: Vec<Project>,
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
}
