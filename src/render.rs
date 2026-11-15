//! Human-readable terminal output.

use crate::model::*;
use crate::plan::PlanResult;
use crate::util::{age, bytes, color_enabled, tilde};
use std::path::Path;

pub struct Style {
    on: bool,
}

impl Style {
    pub fn new() -> Self {
        Self { on: color_enabled() }
    }
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn tier(&self, t: Tier) -> String {
        let code = match t {
            Tier::Junk => "33",
            Tier::Cold => "36",
            Tier::Warm => "35",
            Tier::Hot => "31",
            Tier::Review => "34",
            Tier::Protected => "32",
        };
        self.wrap(code, &format!("{:<9}", t.as_str()))
    }
    pub fn hex(&self, hex: &str, s: &str) -> String {
        if !self.on {
            return s.to_string();
        }
        let h = hex.trim_start_matches('#');
        if h.len() != 6 {
            return s.to_string();
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(128);
        format!("\x1b[38;2;{};{};{}m{}\x1b[0m", p(0), p(2), p(4), s)
    }
}

fn bar(frac: f64, width: usize) -> String {
    let filled = ((frac * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        format!("{:<width$}", s, width = n)
    } else {
        let t: String = s.chars().take(n - 1).collect();
        format!("{}…", t)
    }
}

pub fn overview(r: &Report) {
    let st = Style::new();
    let used_pct = if r.disk.total > 0 { r.disk.used as f64 / r.disk.total as f64 * 100.0 } else { 0.0 };
    println!(
        "{}  {} free of {} ({:.0}% used)   {}",
        st.bold("poo"),
        st.bold(&bytes(r.disk.free)),
        bytes(r.disk.total),
        used_pct,
        st.dim(&format!("scanned {} in {:.1}s", age(r.scanned_at), r.duration_ms as f64 / 1000.0))
    );
    println!();
    let max = r.buckets.iter().map(|b| b.bytes).max().unwrap_or(1).max(1);
    for b in r.buckets.iter().filter(|b| !b.hidden && b.count > 0) {
        let cold: u64 = ["junk", "cold"].iter().filter_map(|t| b.tiers.get(*t)).sum();
        println!(
            "  {} {:>9}  {}  {}",
            trunc(&b.name, 24),
            bytes(b.bytes),
            st.hex(&b.color, &bar(b.bytes as f64 / max as f64, 22)),
            st.dim(&format!("{} items, {} stale", b.count, bytes(cold)))
        );
    }
    let hidden: u64 = r.buckets.iter().filter(|b| b.hidden).map(|b| b.bytes).sum();
    if hidden > 0 {
        println!("  {} {:>9}", trunc("Hidden buckets", 24), bytes(hidden));
    }
    println!("  {} {:>9}", st.bold(&trunc("Total", 24)), st.bold(&bytes(r.total_bytes)));
    println!();
    println!("{}", st.bold("Plans"));
    for p in &r.plans {
        let extra = p
            .target_bytes
            .map(|t| format!(" (target {})", bytes(t)))
            .unwrap_or_default();
        println!(
            "  {:<10} {:>9}  {:>4} items  {}{}",
            p.id,
            bytes(p.bytes),
            p.count,
            st.dim(&p.description),
            st.dim(&extra)
        );
    }
    println!();
    println!("{}", st.dim("poo ls [bucket]   poo show <id>   poo plan stale   poo clean --plan stale   poo free 20G"));
    for w in &r.warnings {
        println!("{} {}", st.dim("note:"), w);
    }
}

pub fn items(list: &[&Item], show_bucket: bool) {
    let st = Style::new();
    println!(
        "{}",
        st.dim(&format!(
            "{:<12} {:>9} {:<9} {:>8}  {:<52} {}",
            "ID", "SIZE", "TIER", "USED", "NAME", if show_bucket { "BUCKET / GROUP" } else { "GROUP" }
        ))
    );
    for i in list {
        let used = i.last_used.map(age).unwrap_or_else(|| "-".into());
        let grp = if show_bucket { format!("{} / {}", i.bucket, i.group) } else { i.group.clone() };
        let size = if i.total_bytes > i.bytes { format!("{}+", bytes(i.total_bytes)) } else { bytes(i.bytes) };
        println!(
            "{:<12} {:>9} {} {:>8}  {} {}",
            i.id,
            size,
            st.tier(i.tier),
            used,
            trunc(&i.name, 52),
            st.dim(&grp)
        );
    }
    let total: u64 = list.iter().map(|i| i.bytes).sum();
    println!("{}", st.dim(&format!("{} items, {}", list.len(), bytes(total))));
}

pub fn item_detail(i: &Item, rule: Option<&crate::catalog::Rule>) {
    let st = Style::new();
    println!("{}  {}", st.bold(&i.name), st.dim(&i.id));
    println!("  path      {}", tilde(Path::new(&i.path)));
    println!("  size      {}  ({} files){}", bytes(i.bytes), i.files, if i.total_bytes > i.bytes { format!(", {} with nested items", bytes(i.total_bytes)) } else { String::new() });
    println!("  tier      {} {}", st.tier(i.tier), st.dim(i.tier.describe()));
    println!("  cost      {}", i.cost.as_str());
    if let Some(t) = i.last_used {
        println!("  used      {}", age(t));
    }
    println!("  bucket    {} / {}", i.bucket, i.group);
    println!("  rule      {}", i.rule);
    if let Some(p) = &i.project {
        println!("  project   {}", tilde(Path::new(p)));
    }
    if let Some(r) = &i.repo {
        if Some(r) != i.project.as_ref() {
            println!("  repo      {}", tilde(Path::new(r)));
        }
    }
    if let Some(a) = &i.agent {
        println!("  agent     {}", a);
    }
    for (k, v) in &i.meta {
        println!("  {:<9} {}", k, v);
    }
    if !i.reasons.is_empty() {
        println!("  why       {}", i.reasons.join("; "));
    }
    if let Some(rb) = &i.rebuild {
        println!("  rebuild   {}", rb);
    }
    let action = match i.action.kind {
        ActionKind::Command => format!("runs `{}`", i.action.command.clone().unwrap_or_default().join(" ")),
        ActionKind::WorktreeRemove => "git worktree remove (refuses uncommitted changes)".into(),
        ActionKind::Delete => "delete".into(),
        ActionKind::Trash => "move to Trash".into(),
        ActionKind::None => "none (manage it yourself)".into(),
    };
    println!("  clean     {}", action);
    if let Some(r) = rule {
        if !r.description.is_empty() {
            println!("  about     {}", r.description);
        }
        if let Some(rec) = &r.recommend {
            println!("  tip       {}", rec);
        }
    }
    if !i.children.is_empty() {
        println!("  inside");
        for c in i.children.iter().take(12) {
            println!("    {:>9}  {}", bytes(c.bytes), c.name);
        }
    }
}

pub fn plan(p: &PlanResult) {
    let st = Style::new();
    println!("{}  {}  {} items", st.bold(&p.name), st.bold(&bytes(p.bytes)), p.count);
    println!("{}", st.dim(&p.description));
    if let Some(t) = p.target_bytes {
        println!("{}", st.dim(&format!("target: free {}", bytes(t))));
    }
    println!();
    let refs: Vec<&Item> = p.items.iter().collect();
    items(&refs, true);
}

pub fn projects(list: &[Project]) {
    let st = Style::new();
    println!(
        "{}",
        st.dim(&format!("{:>9} {:<8} {:<9} {:<10} {:>8}  {:<40} {}", "SIZE", "STATUS", "POLICY", "KIND", "ACTIVE", "PROJECT", "STACKS"))
    );
    for p in list {
        let pol = format!("{:?}", p.policy).to_lowercase();
        println!(
            "{:>9} {:<8} {:<9} {:<10} {:>8}  {} {}",
            bytes(p.artifact_bytes),
            p.status,
            pol,
            p.kind,
            p.last_activity.map(age).unwrap_or_else(|| "-".into()),
            trunc(&tilde(Path::new(&p.path)), 40),
            st.dim(&p.stacks.join(","))
        );
    }
}
