//! Human-readable terminal output for non-interactive commands. Dense, aligned and calm:
//! sizes right-aligned, bars for scale, tier labels in soft colors, prose kept short.
//! JSON output never comes through here.

use crate::model::*;
use crate::plan::PlanResult;
use crate::tui::fmt::{self as tf, cells, fit, fit_mid, pad, rpad};
use crate::util::{age, bytes, color_enabled, is_tty, tilde};
use std::path::Path;

pub struct Style {
    on: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self::new()
    }
}

impl Style {
    pub fn new() -> Self {
        Self { on: color_enabled() }
    }
    pub fn plain() -> Self {
        Self { on: false }
    }
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on && !s.is_empty() {
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
    pub fn good(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn accent(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn note(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    /// Calm tier label padded to a column: cleanable green, in-use plain, the rest muted.
    pub fn tier(&self, t: Tier) -> String {
        self.tier_w(t, 11)
    }
    pub fn tier_w(&self, t: Tier, w: usize) -> String {
        let code = match t {
            Tier::Junk | Tier::Cold | Tier::Idle => "32",
            Tier::Warm => "0",
            Tier::Hot => "36",
            Tier::Review => "34",
            Tier::Protected => "2",
        };
        self.wrap(code, &pad(t.label(), w))
    }
    pub fn hex(&self, hex: &str, s: &str) -> String {
        if !self.on {
            return s.to_string();
        }
        match crate::tui::theme::parse_hex(hex) {
            Some((r, g, b)) if truecolor() => format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, s),
            Some((r, g, b)) => format!("\x1b[38;5;{}m{}\x1b[0m", crate::tui::theme::rgb_to_256(r, g, b), s),
            None => self.good(s),
        }
    }
    /// A smooth bar with a dim dotted track.
    pub fn bar(&self, frac: f64, w: usize, color: Option<&str>) -> String {
        let (fill, rest) = tf::bar(frac, w);
        let fill = match color {
            Some(c) => self.hex(c, &fill),
            None => self.good(&fill),
        };
        format!("{}{}", fill, self.dim(&"·".repeat(rest)))
    }
}

fn truecolor() -> bool {
    matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor") | Ok("24bit"))
}

/// Output width: the terminal's when stdout is one, else a roomy default.
pub fn width() -> usize {
    if is_tty(1) {
        if let Ok((w, _)) = crossterm::terminal::size() {
            return (w as usize).clamp(50, 200);
        }
    }
    120
}

/// "free 119 GB of 494 GB ▕██████████····▏ 76% used"
pub fn disk_line(st: &Style, d: &DiskInfo) -> String {
    let used = if d.total > 0 { d.used as f64 / d.total as f64 } else { 0.0 };
    format!(
        "{} {}  {}  {}",
        st.bold(&bytes(d.free)),
        st.dim(&format!("free of {}", bytes(d.total))),
        st.bar(used, 16, Some("#8a8f98")),
        st.dim(&format!("{:.0}% used", used * 100.0))
    )
}

fn header(st: &Style, r: &Report) {
    println!(
        "{}  {}   {}",
        st.accent(&st.bold("poo")),
        disk_line(st, &r.disk),
        st.dim(&format!("updated {}", age(r.scanned_at)))
    );
}

fn section(st: &Style, title: &str) {
    println!("\n{}", st.bold(title));
}

pub fn overview(r: &Report) {
    let st = Style::new();
    let w = width();
    header(&st, r);
    let parts = crate::tui::view::disk_parts(r);
    println!(
        "{}",
        st.dim(&format!(
            "     other {}  ·  dev in use {}  ·  not in use {}",
            bytes(parts[0]),
            bytes(parts[1]),
            bytes(parts[2])
        ))
    );
    if !r.detected.is_empty() {
        println!("{}", st.dim(&fit(&format!("     found: {}", r.detected.join(" · ")), w)));
    }

    section(&st, "Areas");
    let shown: Vec<&BucketSummary> = r.buckets.iter().filter(|b| !b.hidden && !b.report_only && b.count > 0).collect();
    let max = shown.iter().map(|b| b.bytes).max().unwrap_or(1).max(1);
    let name_w = shown.iter().map(|b| cells(&b.name)).max().unwrap_or(10).clamp(10, 26);
    let bar_w = if w >= 100 { 24 } else { 12 };
    println!("{}", st.dim(&format!("  {}  {:>9}  {}  {:>10}  {:>6}", pad("", name_w), "size", " ".repeat(bar_w), "not in use", "items")));
    for b in &shown {
        let cold = crate::tui::app::bucket_not_in_use(b);
        println!(
            "  {}  {:>9}  {}  {}  {}",
            pad(&b.name, name_w),
            bytes(b.bytes),
            st.bar(b.bytes as f64 / max as f64, bar_w, Some(&b.color)),
            if cold > 0 { st.good(&rpad(&bytes(cold), 10)) } else { st.dim(&rpad("–", 10)) },
            st.dim(&rpad(&b.count.to_string(), 6))
        );
    }
    let hidden: u64 = r.buckets.iter().filter(|b| b.hidden).map(|b| b.bytes).sum();
    if hidden > 0 {
        println!("  {}  {}", st.dim(&pad("hidden areas", name_w)), st.dim(&rpad(&bytes(hidden), 9)));
    }
    println!("  {}  {}", pad("", name_w), st.bold(&rpad(&bytes(r.total_bytes), 9)));
    // Report-only areas: shown for context, never counted or cleaned by poo.
    for (name, b, hint) in crate::tui::app::elsewhere(r) {
        let tail = match hint {
            Some(h) => format!("  ·  manage in their apps: {}", h),
            None => "  ·  manage in their apps".into(),
        };
        println!("{}", st.dim(&fit(&format!("  {}  {:>9}{}", pad(&name, name_w), bytes(b), tail), w)));
    }

    section(&st, "Plans");
    let desc_w = w.saturating_sub(2 + 12 + 2 + 9 + 2 + 9 + 2);
    for p in &r.plans {
        let size = if p.bytes > 0 { st.good(&rpad(&bytes(p.bytes), 9)) } else { st.dim(&rpad("–", 9)) };
        println!(
            "  {}  {}  {}  {}",
            pad(&p.id, 12),
            size,
            st.dim(&rpad(&format!("{} items", p.count), 9)),
            st.dim(&fit(&p.description, desc_w))
        );
    }
    println!();
    let hint = [("poo", "interactive"), ("ls [area]", "items"), ("show <id>", "details"), ("plan stale", "preview"), ("clean --plan stale", ""), ("free 20G", "")];
    let mut line = String::new();
    for (k, d) in hint {
        let piece = if d.is_empty() { format!("{}   ", st.accent(k)) } else { format!("{} {}   ", st.accent(k), st.dim(d)) };
        line.push_str(&piece);
    }
    println!("{}", line.trim_end());
    for w in &r.warnings {
        println!("{} {}", st.dim("note"), st.dim(w));
    }
}

/// Item table: size, bar, status, last used, name, group.
pub fn items(list: &[&Item], show_bucket: bool) {
    let st = Style::new();
    let w = width();
    let max = list.iter().map(|i| i.total_bytes).max().unwrap_or(1).max(1);
    let bar_w = if w >= 110 { 10 } else { 0 };
    let id_w = list.iter().map(|i| cells(&i.id)).max().unwrap_or(4).clamp(4, 14);
    // id, size, bar, status, used, then name and group share the rest.
    let fixed = id_w + 2 + 9 + 2 + if bar_w > 0 { bar_w + 2 } else { 0 } + 11 + 2 + 9 + 2;
    let rest = w.saturating_sub(fixed);
    let longest_grp = list.iter().map(|i| if show_bucket { cells(&i.bucket) + 3 + cells(&i.group) } else { cells(&i.group) }).max().unwrap_or(0);
    // The group gets what the longest name leaves over (never less than a third).
    let longest_name = list.iter().map(|i| cells(&i.name)).max().unwrap_or(0);
    let grp_w = longest_grp.min(28).min((rest / 3).max(rest.saturating_sub(longest_name + 2)));
    let name_w = rest.saturating_sub(grp_w + 2).max(16);
    println!(
        "{}",
        st.dim(&format!(
            "{}  {:>9}  {}{}  {:>9}  {}{}",
            pad("id", id_w),
            "size",
            if bar_w > 0 { format!("{}  ", " ".repeat(bar_w)) } else { String::new() },
            pad("status", 11),
            "used",
            pad("name", name_w),
            if grp_w > 4 { format!("  {}", if show_bucket { "area / group" } else { "group" }) } else { String::new() }
        ))
    );
    for i in list {
        let used = i.last_used.map(age).unwrap_or_else(|| "–".into());
        let grp = if show_bucket { format!("{} / {}", i.bucket, i.group) } else { i.group.clone() };
        let size = if i.total_bytes > i.bytes { format!("{}+", bytes(i.total_bytes)) } else { bytes(i.bytes) };
        let bar = if bar_w > 0 { format!("{}  ", st.bar(i.total_bytes as f64 / max as f64, bar_w, None)) } else { String::new() };
        println!(
            "{}  {}  {}{}  {}  {}{}",
            st.dim(&pad(&i.id, id_w)),
            st.bold(&rpad(&size, 9)),
            bar,
            if i.report_only { st.dim(&pad("in its app", 11)) } else { st.tier(i.tier) },
            st.dim(&rpad(&used, 9)),
            pad(&i.name, name_w),
            if grp_w > 4 { format!("  {}", st.dim(&fit(&grp, grp_w))) } else { String::new() }
        );
    }
    let total: u64 = list.iter().map(|i| i.bytes).sum();
    let free: u64 = list.iter().filter(|i| crate::tui::app::cleanable(i)).map(|i| i.bytes).sum();
    println!(
        "{}",
        st.dim(&format!("{} items · {}{}", list.len(), bytes(total), if free > 0 { format!(" · {} not in use", bytes(free)) } else { String::new() }))
    );
}

fn kv(st: &Style, k: &str, v: &str) {
    println!("  {}  {}", st.dim(&pad(k, 9)), v);
}

pub fn item_detail(i: &Item, rule: Option<&crate::catalog::Rule>) {
    let st = Style::new();
    let w = width();
    println!("{}  {}", st.bold(&i.name), st.dim(&i.id));
    println!("  {}", st.dim(&fit_mid(&tilde(Path::new(&i.path)), w.saturating_sub(2))));
    println!();
    let nested = if i.total_bytes > i.bytes { format!("  ({} with nested items)", bytes(i.total_bytes)) } else { String::new() };
    kv(&st, "size", &format!("{}  {}{}", st.bold(&bytes(i.bytes)), st.dim(&format!("{} files", crate::tui::view::thousands(i.files))), st.dim(&nested)));
    if i.report_only {
        kv(&st, "status", &st.dim("report only: poo shows it for context and never cleans it; manage it in its app"));
    } else {
        kv(&st, "status", &format!("{}{}", st.tier_w(i.tier, cells(i.tier.label()) + 2), st.dim(i.tier.describe())));
    }
    if let Some(t) = i.last_used {
        kv(&st, "used", &age(t));
    }
    kv(&st, "costs", i.cost.as_str());
    if let Some(rb) = &i.rebuild {
        kv(&st, "rebuild", rb);
    }
    if !i.reasons.is_empty() {
        kv(&st, "why", &i.reasons.join("; "));
    }
    for r in i.risks.iter().filter(|r| r.level > RiskLevel::Standard) {
        kv(&st, "check", &format!("{} {}", r.message, st.dim(&format!("({})", r.level.flag()))));
    }
    for g in &i.safeguards {
        kv(&st, "safe", &format!("{} {}", st.good("↺"), g));
    }
    kv(&st, "area", &format!("{} / {}", i.bucket, i.group));
    kv(&st, "rule", &st.dim(&i.rule));
    if let Some(p) = &i.project {
        kv(&st, "project", &tilde(Path::new(p)));
    }
    if let Some(r) = &i.repo {
        if Some(r) != i.project.as_ref() {
            kv(&st, "repo", &tilde(Path::new(r)));
        }
    }
    if let Some(a) = &i.agent {
        kv(&st, "agent", a);
    }
    for (k, v) in &i.meta {
        kv(&st, k, &v.to_string().trim_matches('"').to_string());
    }
    let action = match i.action.kind {
        ActionKind::Command => format!("runs `{}`", i.action.command.clone().unwrap_or_default().join(" ")),
        ActionKind::WorktreeRemove => "git worktree remove (refuses uncommitted changes)".into(),
        ActionKind::Delete => "delete".into(),
        ActionKind::Trash => "move to Trash".into(),
        ActionKind::None => "none (manage it yourself)".into(),
    };
    kv(&st, "clean", &action);
    if let Some(r) = rule {
        if !r.description.is_empty() {
            kv(&st, "about", &st.dim(&r.description));
        }
        if let Some(rec) = &r.recommend {
            kv(&st, "tip", &st.dim(rec));
        }
    }
    if !i.children.is_empty() {
        println!();
        println!("  {}", st.dim("inside"));
        let max = i.children.iter().map(|c| c.bytes).max().unwrap_or(1).max(1);
        for c in i.children.iter().take(12) {
            println!("  {:>9}  {}  {}", bytes(c.bytes), st.bar(c.bytes as f64 / max as f64, 10, Some("#8a8f98")), c.name);
        }
    }
}

pub fn plan(p: &PlanResult) {
    let st = Style::new();
    println!("{}  {}  {}", st.bold(&p.name), st.good(&st.bold(&bytes(p.bytes))), st.dim(&format!("{} items", p.count)));
    println!("{}", st.dim(&p.description));
    if let Some(t) = p.target_bytes {
        println!("{}", st.dim(&format!("target: free {}", bytes(t))));
    }
    println!();
    let refs: Vec<&Item> = p.items.iter().collect();
    items(&refs, true);
}

fn project_status(st: &Style, s: &str, w: usize) -> String {
    let p = pad(s, w);
    match s {
        "active" => st.accent(&p),
        "stale" => st.good(&p),
        _ => p,
    }
}

pub fn projects(list: &[Project]) {
    let st = Style::new();
    let w = width();
    let max = list.iter().map(|p| p.artifact_bytes).max().unwrap_or(1).max(1);
    let bar_w = if w >= 110 { 10 } else { 0 };
    let fixed = 9 + 2 + if bar_w > 0 { bar_w + 2 } else { 0 } + 8 + 2 + 9 + 2 + 9 + 2 + 10 + 2;
    let path_w = w.saturating_sub(fixed + 14).max(20);
    println!(
        "{}",
        st.dim(&format!(
            "{:>9}  {}{}  {}  {}  {:>9}  {}  {}",
            "build",
            if bar_w > 0 { format!("{}  ", " ".repeat(bar_w)) } else { String::new() },
            pad("status", 8),
            pad("policy", 9),
            pad("kind", 9),
            "active",
            pad("project", path_w),
            "stacks"
        ))
    );
    for p in list {
        let pol = format!("{:?}", p.policy).to_lowercase();
        let bar = if bar_w > 0 { format!("{}  ", st.bar(p.artifact_bytes as f64 / max as f64, bar_w, None)) } else { String::new() };
        println!(
            "{}  {}{}  {}  {}  {}  {}  {}",
            st.bold(&rpad(&bytes(p.artifact_bytes), 9)),
            bar,
            project_status(&st, &p.status, 8),
            st.dim(&pad(&pol, 9)),
            st.dim(&pad(&p.kind, 9)),
            st.dim(&rpad(&p.last_activity.map(age).unwrap_or_else(|| "–".into()), 9)),
            pad(&fit_mid(&tilde(Path::new(&p.path)), path_w), path_w),
            st.dim(&p.stacks.join(" "))
        );
    }
}

/// `poo worktrees`: repos with their checkouts (the JSON shape from `ops::worktrees`).
pub fn worktrees(v: &serde_json::Value) {
    let st = Style::new();
    let w = width();
    let path_w = w.saturating_sub(2 + 9 + 2 + 8 + 2 + 11 + 2 + 10 + 2 + 9 + 2).clamp(20, 60);
    let repos = v["repos"].as_array().cloned().unwrap_or_default();
    if repos.is_empty() {
        println!("{}", st.dim("No repos with extra worktrees found."));
        return;
    }
    for repo in repos {
        println!(
            "{}  {}",
            st.bold(&tilde(Path::new(repo["repo"].as_str().unwrap_or("")))),
            st.dim(&format!("{} across {} checkouts", bytes(repo["build_bytes"].as_u64().unwrap_or(0)), repo["checkouts"]))
        );
        for wt in repo["worktrees"].as_array().into_iter().flatten() {
            let item = &wt["item"];
            let (state, styled): (&str, fn(&Style, &str) -> String) = match (wt["dirty"].as_bool(), wt["merged"].as_bool()) {
                (Some(true), _) => ("changes", |s, x| s.note(x)),
                (_, Some(true)) => ("merged", |s, x| s.good(x)),
                _ if wt["kind"] == "repo" => ("main", |s, x| s.dim(x)),
                _ => ("clean", |s, x| s.dim(x)),
            };
            let tier = item["tier"].as_str().and_then(Tier::parse).map(|t| st.tier(t)).unwrap_or_else(|| st.dim(&pad("–", 11)));
            let path = std::path::Path::new(wt["path"].as_str().unwrap_or(""));
            println!(
                "  {}  {}  {}  {}  {}  {}",
                st.bold(&rpad(&bytes(item["total_bytes"].as_u64().unwrap_or(wt["build_bytes"].as_u64().unwrap_or(0))), 9)),
                styled(&st, &pad(state, 8)),
                tier,
                st.dim(&pad(wt["agent"].as_str().unwrap_or(""), 10)),
                pad(&fit_mid(&tilde(path), path_w), path_w),
                st.dim(&format!("{}  {}", wt["branch"].as_str().unwrap_or("(detached)"), item["id"].as_str().unwrap_or("")))
            );
        }
    }
}

/// `poo schedule ls`.
pub fn schedules(list: &[crate::config::Schedule], state: &crate::schedule::ScheduleState, agent: &str) {
    let st = Style::new();
    println!("{}", st.dim(&format!("{}  {}  {}  {}  {}", pad("id", 10), pad("", 3), pad("plan", 10), pad("when", 30), "last run")));
    for s in list {
        let last = state.last_run.get(&s.id);
        let mut when = format!("every {}", s.every);
        if let Some(a) = &s.at {
            when.push_str(&format!(" at {}", a));
        }
        if let Some(a) = &s.when_free_below {
            when.push_str(&format!(", free < {}", a));
        }
        if let Some(a) = &s.target_free {
            when.push_str(&format!(", to {} free", a));
        }
        let last = last
            .map(|l| format!("{} {} {}", age(l.at), l.status, if l.freed_bytes > 0 { format!("· freed {}", bytes(l.freed_bytes)) } else { String::new() }))
            .unwrap_or_else(|| "never ran".into());
        println!(
            "{}  {}  {}  {}  {}",
            st.bold(&pad(&s.id, 10)),
            if s.enabled { st.good(&pad("on", 3)) } else { st.dim(&pad("off", 3)) },
            pad(&s.plan, 10),
            pad(&when, 30),
            st.dim(last.trim_end())
        );
    }
    println!("\n{}", st.dim(&format!("scheduler {}", agent)));
}

/// `poo backups ls`.
pub fn backups(list: &[crate::backup::Backup], total: u64, cfg: &crate::config::BackupConfig) {
    let st = Style::new();
    let w = width();
    let where_w = w.saturating_sub(12 + 2 + 7 + 2 + 9 + 2 + 9 + 2 + 26 + 2).max(20);
    println!("{}", st.dim(&format!("{}  {}  {:>9}  {:>9}  {}  {}", pad("id", 12), pad("kind", 7), "size", "saved", pad("label", 26), "where")));
    for b in list {
        let wh = match b.kind.as_str() {
            "files" => b.path.as_deref().map(|p| tilde(Path::new(p))).unwrap_or_default(),
            _ => format!("{} in {}", b.r#ref.clone().unwrap_or_default(), b.repo.as_deref().map(|r| tilde(Path::new(r))).unwrap_or_default()),
        };
        println!(
            "{}  {}  {}  {}  {}  {}",
            st.dim(&pad(&b.id, 12)),
            pad(&b.kind, 7),
            rpad(&if b.kind == "files" { bytes(b.bytes) } else { format!("{} files", b.files) }, 9),
            st.dim(&rpad(&age(b.at), 9)),
            pad(&b.label, 26),
            st.dim(&fit_mid(&wh, where_w))
        );
    }
    let frac = if cfg.max_backup_bytes > 0 { total as f64 / cfg.max_backup_bytes as f64 } else { 0.0 };
    println!(
        "\n{} {}  {}",
        st.bold(&bytes(total)),
        st.dim(&format!("of {} budget", bytes(cfg.max_backup_bytes))),
        st.bar(frac, 12, None)
    );
    println!(
        "{}",
        st.dim(&if cfg.auto_prune_days > 0 { format!("file backups older than {} days are pruned", cfg.auto_prune_days) } else { "kept until you prune".into() })
    );
}

/// One live progress line for `poo clean` on a terminal (no newline).
pub fn clean_progress_line(p: &crate::clean::CleanProgress, tick: usize) -> String {
    let st = Style::new();
    let w = width().min(120);
    let frac = if p.total_bytes > 0 { p.freed_bytes as f64 / p.total_bytes as f64 } else { p.done as f64 / p.total.max(1) as f64 };
    let spin = tf::SPINNER[tick % tf::SPINNER.len()];
    let head = format!(
        "{} {}  {} {}  {}",
        st.accent(spin),
        st.bar(frac, 20, None),
        st.good(&rpad(&bytes(p.freed_bytes), 9)),
        st.dim(&format!("of {}", bytes(p.total_bytes))),
        st.dim(&format!("{}/{}", p.done, p.total))
    );
    let used = 2 + 20 + 2 + 9 + 1 + 3 + bytes(p.total_bytes).len() + 2 + format!("{}/{}", p.done, p.total).len();
    format!("{}  {}", head, fit(&p.current, w.saturating_sub(used + 2)))
}

/// Per-item results after `poo clean` / `poo free`.
pub fn clean_result_rows(res: &crate::clean::CleanResult) {
    let st = Style::new();
    let w = width();
    for r in &res.results {
        let mark = match r.status.as_str() {
            "cleaned" => st.good("✓"),
            "would-clean" => st.good("·"),
            "failed" => st.note("✗"),
            _ => st.dim("–"),
        };
        let reason = r.reason.as_ref().map(|x| st.dim(&format!("  {}", x))).unwrap_or_default();
        println!("{} {}  {}{}", mark, rpad(&bytes(r.bytes), 9), fit(&r.name, w.saturating_sub(14)), reason);
        for n in &r.notes {
            println!("             {} {}", st.good("↺"), st.dim(n));
        }
    }
}

/// "Freed 46.9 GB · free 61.0 GB → 108 GB", or the dry-run equivalent.
pub fn clean_summary(res: &crate::clean::CleanResult) {
    let st = Style::new();
    println!();
    if res.dry_run {
        println!(
            "{} {}  {}",
            st.dim("Would free"),
            st.good(&st.bold(&bytes(res.freed_bytes))),
            st.dim(&format!("· {} items · {} stay · nothing removed (dry run)", res.cleaned, res.skipped))
        );
        return;
    }
    let mut extra = vec![format!("{} cleaned", res.cleaned)];
    if res.skipped > 0 {
        extra.push(format!("{} stayed", res.skipped));
    }
    if res.failed > 0 {
        extra.push(format!("{} failed", res.failed));
    }
    println!(
        "{} {}  {} {} {} {}   {}",
        st.bold("Freed"),
        st.good(&st.bold(&bytes(res.freed_bytes))),
        st.dim("· free"),
        bytes(res.disk_free_before),
        st.dim("→"),
        st.bold(&bytes(res.disk_free_after)),
        st.dim(&extra.join(" · "))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_style_has_no_escapes() {
        let st = Style::plain();
        assert_eq!(st.bold("x"), "x");
        assert_eq!(st.tier(Tier::Cold), pad("unused", 11));
        let b = st.bar(0.5, 10, Some("#ff0000"));
        assert_eq!(cells(&b), 10);
        assert!(!b.contains('\x1b'));
    }

    #[test]
    fn tier_labels_stay_calm() {
        for t in Tier::all() {
            let l = t.label();
            for word in ["danger", "warning", "risk", "delete"] {
                assert!(!l.contains(word), "{} label `{}` sounds alarming", t.as_str(), l);
            }
        }
    }
}
