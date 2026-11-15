//! Data shared by the engine, the CLI, MCP and the macOS app (as JSON).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Junk,
    Cold,
    Idle,
    Warm,
    Hot,
    Review,
    Protected,
}

impl Tier {
    pub fn all() -> [Tier; 7] {
        [Tier::Junk, Tier::Cold, Tier::Idle, Tier::Warm, Tier::Hot, Tier::Review, Tier::Protected]
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Junk => "junk",
            Tier::Cold => "cold",
            Tier::Idle => "idle",
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
            Tier::Idle => 2,
            Tier::Warm => 3,
            Tier::Hot => 4,
            Tier::Review => 5,
            Tier::Protected => 6,
        }
    }
    pub fn describe(&self) -> &'static str {
        match self {
            Tier::Junk => "junk, nothing to rebuild",
            Tier::Cold => "unused for days, orphaned, or its work already landed",
            Tier::Idle => "nothing is using it right now; costs a rebuild if you return",
            Tier::Warm => "in active use, will be rebuilt or redownloaded soon",
            Tier::Hot => "in use right now",
            Tier::Review => "needs a human look",
            Tier::Protected => "kept; only --dangerously-include-protected cleans it",
        }
    }
}

/// How much risk a clean may take. Items carry risks; each needs a mode to be allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Only items without risks.
    #[serde(rename = "safe", alias = "standard")]
    Standard,
    /// Also items in use: recent, agent open, uncommitted work (stashed first), check-first.
    #[serde(rename = "in-use", alias = "force")]
    Force,
    /// Also protected things: running builds, data, system paths, detached commits.
    #[serde(rename = "dangerous", alias = "danger")]
    Danger,
}

impl RiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Standard => "safe",
            RiskLevel::Force => "in-use",
            RiskLevel::Danger => "dangerous",
        }
    }
    /// The CLI flag that allows this level.
    pub fn flag(&self) -> &'static str {
        match self {
            RiskLevel::Standard => "",
            RiskLevel::Force => "--include-in-use",
            RiskLevel::Danger => "--dangerously-include-protected",
        }
    }
}

impl Tier {
    /// Calm, human label. Ids stay stable in JSON.
    pub fn label(&self) -> &'static str {
        match self {
            Tier::Junk => "junk",
            Tier::Cold => "unused",
            Tier::Idle => "idle",
            Tier::Warm => "in use",
            Tier::Hot => "active now",
            Tier::Review => "check first",
            Tier::Protected => "kept",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    /// Stable tag: recent, agent-active, review, dirty, unmerged, build-running, app-running, data, system-path, unreachable-commits.
    pub tag: String,
    /// Mode needed to clean despite this risk.
    pub level: RiskLevel,
    pub message: String,
}

impl Risk {
    pub fn new(tag: &str, level: RiskLevel, message: impl Into<String>) -> Risk {
        Risk { tag: tag.into(), level, message: message.into() }
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
    /// Stacks or tools this rule belongs to (rust, swift, xcode, node, python, ruby, docker,
    /// android, macos...). Used to show people what is relevant to their machine.
    pub stacks: Option<Vec<String>>,
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
    /// Files inside the folder that must start with this text (`{ "CACHEDIR.TAG" = "Signature: 8a47..." }`).
    /// Read only after every name matcher passed.
    pub contains: Option<BTreeMap<String, String>>,
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
    /// Untouched this many days with nobody working there counts as idle.
    pub idle_days: Option<u32>,
    pub action: Option<ActionKind>,
    pub command: Option<Vec<String>>,
    /// Shown and sized but never cleaned by poo: never in plans, refused by clean. Point
    /// people at the owner's own UI with `recommend`.
    pub report_only: Option<bool>,
    /// Clean by running the owning tool's command (argv, no shell) instead of deleting,
    /// when that tool is installed. poo measures the path before and after.
    pub clean_command: Option<Vec<String>>,
    /// A command that must exit 0 before `clean_command` (or a provider's command) runs,
    /// for example `["docker", "info"]`. A failure or timeout skips the item.
    pub probe: Option<Vec<String>>,
    /// Time limit for `clean_command` or a provider's command (default 600 seconds).
    pub timeout_secs: Option<u64>,
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
    /// A bucket for awareness only ("Elsewhere on this Mac"): its items are report-only.
    pub report_only: Option<bool>,
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
    #[serde(default)]
    pub launcher: Vec<LauncherSpec>,
}

/// How to hand a prompt to a coding agent or AI app.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherSpec {
    pub id: String,
    pub name: Option<String>,
    /// terminal: run `command` in a new terminal window in the target folder.
    /// url: open `url` (deep link) with the prompt filled in.
    pub kind: Option<String>,
    pub command: Option<Vec<String>>,
    pub url: Option<String>,
    /// Binaries on PATH that mean it is installed.
    pub detect: Option<Vec<String>>,
    /// App bundles (in /Applications or ~/Applications) that mean it is installed.
    pub detect_app: Option<Vec<String>>,
    pub order: Option<i32>,
    pub enabled: Option<bool>,
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
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
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
    /// What could go wrong cleaning this, and which mode allows it.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub risks: Vec<Risk>,
    /// What poo does before and while cleaning this: backups, stashes, kept branches.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub safeguards: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<Child>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub meta: BTreeMap<String, serde_json::Value>,
    /// Shown and sized, never in plans and never cleaned by poo; `recommend` says where to
    /// manage it instead.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub report_only: bool,
    /// What to do about it, often the owner's own UI.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub recommend: Option<String>,
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
    /// Awareness only: nothing in it is cleanable by poo.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub report_only: bool,
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
    /// Rank among checkouts of the same repo by recent build activity (0 = most recent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recency_rank: Option<usize>,
    /// Coding agents, editors or shells currently working in this checkout.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub active: Vec<String>,
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
    /// Stacks found in your projects plus tools found on this machine.
    #[serde(default)]
    pub detected: Vec<String>,
    /// Rules that matched something, with item count and bytes.
    #[serde(default)]
    pub rule_hits: BTreeMap<String, (usize, u64)>,
    /// Bytes of report-only items (shown, not counted in `total_bytes`, never cleaned).
    #[serde(default)]
    pub report_only_bytes: u64,
    /// Growth per bucket, anomalies and a free-space forecast from the daily history.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub growth: Option<serde_json::Value>,
}
