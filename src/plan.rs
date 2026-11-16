//! Recommended cleaning plans. Plans are mod data (`[[plan]]`), this file only
//! evaluates them against a report.

use crate::catalog::{Catalog, Plan};
use crate::model::{ActionKind, Item, PlanSummary, Report, RiskLevel, Tier};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub buckets: Vec<String>,
    pub exclude_buckets: Vec<String>,
    pub tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub bucket: Option<String>,
    pub group: Option<String>,
    pub project: Option<String>,
    pub tier: Option<Tier>,
    pub min_bytes: u64,
    pub query: Option<String>,
}

impl Filter {
    pub fn matches(&self, i: &Item) -> bool {
        if !self.buckets.is_empty() && !self.buckets.contains(&i.bucket) {
            return false;
        }
        if self.exclude_buckets.contains(&i.bucket) {
            return false;
        }
        if !self.tags.is_empty() && !i.tags.iter().any(|t| self.tags.contains(t)) {
            return false;
        }
        if i.tags.iter().any(|t| self.exclude_tags.contains(t)) {
            return false;
        }
        if let Some(b) = &self.bucket {
            if &i.bucket != b {
                return false;
            }
        }
        if let Some(g) = &self.group {
            if !i.group.eq_ignore_ascii_case(g) {
                return false;
            }
        }
        if let Some(p) = &self.project {
            let hit = i.project.as_deref().map_or(false, |x| x.starts_with(p.as_str()))
                || i.repo.as_deref() == Some(p.as_str())
                || i.path.starts_with(p.as_str());
            if !hit {
                return false;
            }
        }
        if let Some(t) = self.tier {
            if i.tier != t {
                return false;
            }
        }
        if i.bytes < self.min_bytes {
            return false;
        }
        if let Some(q) = &self.query {
            let q = q.to_lowercase();
            let hay = format!("{} {} {} {} {}", i.name, i.path, i.rule, i.group, i.bucket).to_lowercase();
            if !hay.contains(&q) {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanResult {
    pub plan: String,
    pub name: String,
    pub description: String,
    pub bytes: u64,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_bytes: Option<u64>,
    pub items: Vec<Item>,
}

/// What the clean that follows a plan accepts: its mode and tier limit. A plan keeps only
/// items `clean` would admit with the same flags, so its total is what that clean frees.
#[derive(Debug, Clone, Copy)]
pub struct Admission {
    pub mode: RiskLevel,
    pub max_tier: Option<Tier>,
}

impl Default for Admission {
    /// `poo clean --plan` without flags: risk-free items up to `warm`.
    fn default() -> Self {
        Self { mode: RiskLevel::Standard, max_tier: Some(Tier::Warm) }
    }
}

impl Admission {
    pub fn mode(mode: RiskLevel) -> Self {
        Self { mode, ..Default::default() }
    }
    pub fn admits(&self, cat: &Catalog, i: &Item) -> bool {
        crate::clean::admit_static(i, cat, self.mode, self.max_tier).is_ok()
    }
}

pub fn eligible(plan: &Plan, i: &Item) -> bool {
    plan.tiers.contains(&i.tier)
        && i.cost.rank() <= plan.max_cost.rank()
        && !plan.exclude_buckets.contains(&i.bucket)
        && i.action.kind != ActionKind::None
        && !i.report_only
}

/// Bytes a greedy plan should free to reach its free-space target.
pub fn default_target(plan: &Plan, report: &Report) -> Option<u64> {
    let pct = plan.target_free_percent?;
    let want = report.disk.total / 100 * pct as u64;
    Some(want.saturating_sub(report.disk.free))
}

pub fn evaluate(plan: &Plan, report: &Report, filter: &Filter, target: Option<u64>, cat: &Catalog, adm: Admission) -> PlanResult {
    let mut items: Vec<Item> = report
        .items
        .iter()
        .filter(|i| eligible(plan, i) && filter.matches(i) && adm.admits(cat, i))
        .cloned()
        .collect();
    let mut target_bytes = None;
    if plan.greedy {
        let t = target.or_else(|| default_target(plan, report)).unwrap_or(0);
        target_bytes = Some(t);
        // Least regret first: safest tier, cheapest to recreate, longest unused.
        items.sort_by(|a, b| {
            (a.tier.rank(), a.cost.rank(), a.last_used.unwrap_or(0))
                .cmp(&(b.tier.rank(), b.cost.rank(), b.last_used.unwrap_or(0)))
        });
        let items_nested = drop_nested(items);
        let mut sum = 0u64;
        let mut picked: Vec<Item> = vec![];
        for i in items_nested {
            if sum >= t {
                break;
            }
            sum += i.total_bytes;
            picked.push(i);
        }
        // Trim overshoot: drop the most regrettable picks that are not needed to reach the target.
        if sum > t {
            let mut idx: Vec<usize> = (0..picked.len()).collect();
            idx.sort_by_key(|&k| std::cmp::Reverse((picked[k].tier.rank(), picked[k].cost.rank(), picked[k].last_used.unwrap_or(0))));
            let mut drop = std::collections::HashSet::new();
            for k in idx {
                // Junk costs nothing to lose, so it always stays in.
                if picked[k].tier != Tier::Junk && sum - picked[k].total_bytes >= t {
                    sum -= picked[k].total_bytes;
                    drop.insert(k);
                }
            }
            picked = picked.into_iter().enumerate().filter(|(k, _)| !drop.contains(k)).map(|(_, i)| i).collect();
        }
        items = picked;
    } else {
        items.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
    }
    let items = drop_nested(items);
    PlanResult {
        plan: plan.id.clone(),
        name: plan.name.clone(),
        description: plan.description.clone(),
        bytes: items.iter().map(|i| i.total_bytes).sum(),
        count: items.len(),
        target_bytes,
        items,
    }
}

/// What cleaning all of these frees, counting nested items once: an item inside another
/// listed item (a build folder in a listed worktree) is already in that item's total_bytes.
pub fn freed_total(items: &[&Item]) -> u64 {
    let paths: std::collections::HashSet<&str> = items.iter().map(|i| i.path.as_str()).collect();
    items
        .iter()
        .filter(|i| !std::path::Path::new(&i.path).ancestors().skip(1).any(|a| a.to_str().map_or(false, |a| paths.contains(a))))
        .map(|i| i.total_bytes)
        .sum()
}

/// When a worktree is removed, its nested artifacts go with it: count them once.
pub fn drop_nested(items: Vec<Item>) -> Vec<Item> {
    let wts: Vec<String> = items
        .iter()
        .filter(|i| i.action.kind == ActionKind::WorktreeRemove)
        .map(|i| format!("{}/", i.path))
        .collect();
    if wts.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|i| i.action.kind == ActionKind::WorktreeRemove || !wts.iter().any(|w| i.path.starts_with(w)))
        .collect()
}

pub fn summaries(cat: &Catalog, report: &Report) -> Vec<PlanSummary> {
    cat.plans
        .iter()
        .map(|p| {
            let r = evaluate(p, report, &Filter::default(), None, cat, Admission::default());
            PlanSummary {
                id: p.id.clone(),
                name: p.name.clone(),
                description: p.description.clone(),
                bytes: r.bytes,
                count: r.count,
                target_bytes: r.target_bytes,
            }
        })
        .collect()
}
