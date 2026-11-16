//! Operations shared by the CLI, MCP and the app (through the CLI's JSON).

use crate::catalog::{self, Catalog};
use crate::config::Config;
use crate::fsutil::{names, read_entries};
use crate::model::*;
use crate::plan::{self, Filter, PlanResult};
use crate::scan::{self, ScanOptions};
use crate::util::{expand, now, tilde};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Ctx {
    pub cfg: Config,
    pub cat: Catalog,
}

impl Ctx {
    pub fn load() -> Result<Ctx> {
        let cfg = Config::load()?;
        let cat = catalog::load(&cfg);
        Ok(Ctx { cfg, cat })
    }
    pub fn reload(&mut self) -> Result<()> {
        self.cfg = Config::load()?;
        self.cat = catalog::load(&self.cfg);
        Ok(())
    }
}

/// The current report. Reads reuse a report from the last few seconds, otherwise update
/// it from the file-change journal (only what changed is measured again). A full walk
/// happens on `rescan`, when the journal cannot help, or when the report is too old.
pub fn report(ctx: &Ctx, rescan: bool, opts: ScanOptions) -> Result<Report> {
    let cached = if rescan { None } else { scan::load_report().filter(|r| r.version == crate::VERSION) };
    if let Some(r) = &cached {
        if now() - r.scanned_at < ctx.cfg.scan.fresh_seconds as i64 {
            return Ok(cached.unwrap());
        }
    }
    let r = if opts.full {
        scan::scan(&ctx.cfg, &ctx.cat, &opts)
    } else if let Some(r) = scan::scan_incremental(&ctx.cfg, &ctx.cat, &opts) {
        r
    } else {
        match cached {
            Some(r) if now() - r.scanned_at < ctx.cfg.scan.cache_minutes as i64 * 60 => return Ok(r),
            _ => scan::scan(&ctx.cfg, &ctx.cat, &opts),
        }
    };
    scan::save_report(&r)?;
    Ok(r)
}

/// Re-applies classification-independent changes (plans) to a cached report after config edits.
pub fn refresh_plans(ctx: &Ctx, r: &mut Report) {
    r.plans = plan::summaries(&ctx.cat, r);
}

pub fn overview(r: &Report) -> Value {
    json!({
        "disk": r.disk,
        "scanned_at": r.scanned_at,
        "scan_age_seconds": now() - r.scanned_at,
        "total_bytes": r.total_bytes,
        "report_only_bytes": r.report_only_bytes,
        "growth": r.growth.clone().or_else(|| crate::growth::summary(r)),
        "buckets": r.buckets.iter().map(|b| json!({
            "id": b.id, "name": b.name, "bytes": b.bytes, "count": b.count, "hidden": b.hidden, "report_only": b.report_only,
            "color": b.color, "symbol": b.symbol, "tiers": b.tiers,
            "groups": b.groups.iter().take(8).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "plans": r.plans,
        "top_items": r.items.iter().take(10).map(compact).collect::<Vec<_>>(),
        "warnings": r.warnings,
        "roots": r.roots,
    })
}

pub fn compact(i: &Item) -> Value {
    json!({
        "id": i.id, "name": i.name, "path": i.path, "bytes": i.bytes, "total_bytes": i.total_bytes,
        "tier": i.tier, "status": i.tier.label(), "risks": i.risks, "cost": i.cost, "bucket": i.bucket, "group": i.group,
        "last_used": i.last_used, "reasons": i.reasons, "rule": i.rule,
        "project": i.project, "agent": i.agent,
        "report_only": i.report_only, "recommend": i.recommend,
    })
}

pub fn list<'a>(r: &'a Report, f: &Filter, sort: &str, limit: usize) -> Vec<&'a Item> {
    let mut v: Vec<&Item> = r.items.iter().filter(|i| f.matches(i)).collect();
    match sort {
        "age" => v.sort_by_key(|i| i.last_used.unwrap_or(0)),
        "name" => v.sort_by(|a, b| a.name.cmp(&b.name)),
        "tier" => v.sort_by_key(|i| (i.tier.rank(), std::cmp::Reverse(i.bytes))),
        _ => v.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes)),
    }
    if limit > 0 {
        v.truncate(limit);
    }
    v
}

pub fn find_item<'a>(r: &'a Report, key: &str) -> Option<&'a Item> {
    if let Some(i) = r.items.iter().find(|i| i.id == key) {
        return Some(i);
    }
    let p = expand(key);
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    let ps = p.to_string_lossy();
    r.items.iter().find(|i| i.path == ps)
}

/// Asked to clean something poo only reports on (snapshots, purgeable space, media...).
#[derive(Debug)]
pub struct ReportOnlyError(pub String);

impl std::fmt::Display for ReportOnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReportOnlyError {}

/// The items to clean for these ids or paths. Report-only items are refused here, so the
/// CLI, the app and MCP all say the same thing.
pub fn resolve_ids(r: &Report, keys: &[String]) -> Result<Vec<Item>> {
    let mut out = vec![];
    for k in keys {
        match find_item(r, k) {
            Some(i) if i.report_only => {
                return Err(ReportOnlyError(format!("{} is report-only: {}", i.name, crate::clean::report_only_message(i))).into())
            }
            Some(i) => out.push(i.clone()),
            None => bail!("no item `{}` in the last scan (run `poo scan` or check `poo ls`)", k),
        }
    }
    Ok(out)
}

/// A plan as `clean` would run it with `adm` (mode and tier limit): items it would refuse
/// are left out, so the plan's total is what that clean frees.
pub fn make_plan(ctx: &Ctx, r: &Report, id: &str, target: Option<u64>, f: &Filter, adm: plan::Admission) -> Result<PlanResult> {
    let p = ctx.cat.plan(id).ok_or_else(|| {
        anyhow!("unknown plan `{}`; plans: {}", id, ctx.cat.plans.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", "))
    })?;
    let mut p = p.clone();
    if target.is_some() {
        p.greedy = true;
    }
    Ok(plan::evaluate(&p, r, f, target, &ctx.cat, adm))
}

/// Removes cleaned items from the cached report so follow-up commands stay accurate.
pub fn forget_cleaned(ctx: &Ctx, r: &mut Report, res: &crate::clean::CleanResult) {
    let gone: std::collections::HashSet<&str> =
        res.results.iter().filter(|x| x.status == "cleaned").map(|x| x.path.as_str()).collect();
    if gone.is_empty() {
        return;
    }
    r.items.retain(|i| {
        !gone.contains(i.path.as_str()) && !gone.iter().any(|g| i.path.starts_with(&format!("{}/", g)))
    });
    r.total_bytes = r.items.iter().map(|i| i.bytes).sum();
    for b in &mut r.buckets {
        let mine: Vec<&Item> = r.items.iter().filter(|i| i.bucket == b.id).collect();
        b.bytes = mine.iter().map(|i| i.bytes).sum();
        b.count = mine.len();
        b.tiers.clear();
        for i in &mine {
            *b.tiers.entry(i.tier.as_str().to_string()).or_default() += i.bytes;
        }
        for g in &mut b.groups {
            let gi: Vec<&&Item> = mine.iter().filter(|i| i.group == g.name).collect();
            g.bytes = gi.iter().map(|i| i.bytes).sum();
            g.count = gi.len();
        }
        b.groups.retain(|g| g.count > 0);
    }
    r.disk = crate::fsutil::disk_info(&crate::util::home());
    refresh_plans(ctx, r);
    let _ = scan::save_report(r);
    scan::forget_in_index(&gone.iter().map(|g| g.to_string()).collect::<Vec<_>>());
}

pub fn worktrees(r: &Report) -> Value {
    let mut repos: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for p in r.projects.iter().filter(|p| p.kind == "worktree" || p.kind == "repo" || p.kind == "orphan") {
        let key = p.repo.clone().unwrap_or_else(|| p.path.clone());
        let item = r.items.iter().find(|i| i.path == p.path && i.action.kind == ActionKind::WorktreeRemove);
        repos.entry(key).or_default().push(json!({
            "path": p.path, "kind": p.kind, "branch": p.branch, "agent": p.agent,
            "last_activity": p.last_activity, "status": p.status, "policy": p.policy,
            "build_bytes": p.artifact_bytes, "dirty": p.dirty, "merged": p.merged,
            "item": item.map(|i| json!({"id": i.id, "tier": i.tier, "total_bytes": i.total_bytes, "reasons": i.reasons})),
        }));
    }
    let mut out: Vec<Value> = repos
        .into_iter()
        .filter(|(_, v)| v.len() > 1 || v.iter().any(|w| w["kind"] != "repo"))
        .map(|(repo, wts)| {
            let total: u64 = wts.iter().map(|w| w["build_bytes"].as_u64().unwrap_or(0)).sum();
            json!({"repo": repo, "checkouts": wts.len(), "build_bytes": total, "worktrees": wts})
        })
        .collect();
    out.sort_by(|a, b| b["build_bytes"].as_u64().cmp(&a["build_bytes"].as_u64()));
    json!({ "repos": out })
}

pub fn catalog_json(cat: &Catalog) -> Value {
    json!({
        "full_disk_access": crate::util::has_full_disk_access(),
        "mods": cat.mods,
        "buckets": cat.buckets,
        "plans": cat.plans,
        "providers": catalog::PROVIDERS.iter().map(|(id, d)| json!({"id": id, "description": d})).collect::<Vec<_>>(),
        "stacks": cat.stacks,
        "worktree_homes": cat.worktree_homes,
        "guards": cat.guards,
        "rules": cat.rules.iter().map(|r| json!({
            "id": r.id, "name": r.name, "bucket": r.bucket, "group": r.group, "kind": r.kind,
            "enabled": r.enabled, "cost": r.cost, "tier": r.tier, "warm_days": r.warm_days,
            "tags": r.tags, "stacks": r.stacks, "idle_days": r.idle_days, "running_apps": r.running_apps,
            "exclude": r.exclude.raw, "min_age_days": r.min_age_days, "contents": r.contents, "system": r.system,
            "needs_full_disk_access": r.needs_permission,
            "description": r.description, "recommend": r.recommend, "rebuild": r.rebuild,
            "names": r.names.raw, "parent_any": r.parent_any.raw, "children_any": r.children_any.raw,
            "children_all": r.children_all.raw, "ignored_only": r.ignored,
            "paths": r.paths, "each": r.each, "provider": r.provider, "action": r.action,
            "source": r.source,
        })).collect::<Vec<_>>(),
        "errors": cat.errors,
    })
}

pub fn explain(ctx: &Ctx, r: Option<&Report>, raw: &str) -> Value {
    let p = expand(raw);
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    let exists = p.exists();
    let mut matches = vec![];
    if exists && p.is_dir() {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parent = p.parent().map(read_entries).map(|e| names(&e)).unwrap_or_default();
        let children = names(&read_entries(&p));
        for rule in ctx.cat.rules.iter().filter(|r| r.kind == RuleKind::Artifact) {
            if rule.matches_artifact(&name, &p, &parent, &children) {
                matches.push(json!({"rule": rule.id, "kind": "artifact", "enabled": rule.enabled, "ignored_required": rule.ignored, "bucket": rule.bucket, "cost": rule.cost}));
            }
        }
    }
    for rule in ctx.cat.rules.iter().filter(|r| r.kind == RuleKind::Path) {
        for pat in &rule.paths {
            let ep = expand(pat);
            let hits = crate::fsutil::expand_glob(&ep);
            let direct = hits.iter().any(|h| h == &p);
            let child = rule.each && hits.iter().any(|h| p.parent() == Some(h.as_path()));
            if direct || child {
                matches.push(json!({"rule": rule.id, "kind": "path", "pattern": pat, "each": rule.each, "enabled": rule.enabled, "bucket": rule.bucket, "cost": rule.cost}));
            }
        }
    }
    let item = r.and_then(|r| {
        let ps = p.to_string_lossy().to_string();
        r.items
            .iter()
            .filter(|i| ps == i.path || ps.starts_with(&format!("{}/", i.path)))
            .max_by_key(|i| i.path.len())
            .map(compact)
    });
    let ignored = p.parent().map(|parent| {
        std::process::Command::new("git")
            .args(["-C", &parent.to_string_lossy(), "check-ignore", "-q", &p.to_string_lossy()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    json!({
        "path": p, "exists": exists, "git_ignored": ignored,
        "matching_rules": matches,
        "first_match_wins": "artifact rules are tried in catalog order; for path rules an exact path beats an `each` expansion",
        "in_report_as": item,
    })
}

fn parse_toml_value(v: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("v = {}", v))
        .ok()
        .and_then(|t| t.get("v").cloned())
        .unwrap_or_else(|| toml::Value::String(v.to_string()))
}

/// Changes one dotted key in config.toml, validating by round-tripping.
pub fn set_config_key(cfg: &Config, key: &str, value: &str) -> Result<Config> {
    let mut root = toml::Value::try_from(cfg)?;
    let parts: Vec<&str> = key.split('.').collect();
    let mut cur = &mut root;
    for (i, part) in parts.iter().enumerate() {
        let table = cur.as_table_mut().ok_or_else(|| anyhow!("`{}` is not a table", parts[..i].join(".")))?;
        if i == parts.len() - 1 {
            table.insert(part.to_string(), parse_toml_value(value));
            break;
        }
        cur = table.entry(part.to_string()).or_insert(toml::Value::Table(Default::default()));
    }
    let new: Config = root.try_into().map_err(|e| anyhow!("invalid value for {}: {}", key, e))?;
    Ok(new)
}

pub fn set_rule(ctx: &Ctx, id: &str, assignments: &[String]) -> Result<Config> {
    if ctx.cat.rule(id).is_none() {
        bail!("unknown rule `{}` (see `poo rules`)", id);
    }
    let mut cfg = ctx.cfg.clone();
    let entry = cfg.rule_overrides.entry(id.to_string()).or_default();
    for a in assignments {
        let (k, v) = a.split_once('=').ok_or_else(|| anyhow!("expected key=value, got `{}`", a))?;
        let doc = format!("{} = {}", k.trim(), parse_toml_value(v.trim()));
        let spec: RuleSpec = toml::from_str(&doc).map_err(|e| anyhow!("{}: {}", a, e.to_string().trim()))?;
        crate::catalog::merge_rule(entry, &spec);
    }
    let check = crate::catalog::load(&cfg);
    if let Some(e) = check.errors.iter().find(|e| e.contains(id)) {
        bail!("that override breaks the rule: {}", e);
    }
    Ok(cfg)
}

/// Rule ids selected by id, tag, group, bucket or stack (all given filters must match).
pub fn select_rules(cat: &Catalog, id: Option<&str>, tag: Option<&str>, group: Option<&str>, bucket: Option<&str>, stack: Option<&str>) -> Vec<String> {
    if id.is_none() && tag.is_none() && group.is_none() && bucket.is_none() && stack.is_none() {
        return vec![];
    }
    cat.rules
        .iter()
        .filter(|r| id.map_or(true, |i| r.id == i))
        .filter(|r| tag.map_or(true, |t| r.tags.iter().any(|x| x == t)))
        .filter(|r| group.map_or(true, |g| r.group.eq_ignore_ascii_case(g)))
        .filter(|r| bucket.map_or(true, |b| r.bucket == b))
        .filter(|r| stack.map_or(true, |s| r.stacks.iter().any(|x| x == s)))
        .map(|r| r.id.clone())
        .collect()
}

pub fn toggle_rules(cfg: &Config, ids: &[String], enable: bool) -> Config {
    let mut c = cfg.clone();
    for id in ids {
        c.disabled_rules.retain(|r| r != id);
        if enable {
            c.rule_overrides.entry(id.clone()).or_default().enabled = Some(true);
        } else {
            c.disabled_rules.push(id.clone());
            if let Some(o) = c.rule_overrides.get_mut(id) {
                o.enabled = None;
            }
        }
    }
    c
}

pub struct BucketEdit {
    pub name: Option<String>,
    pub order: Option<i32>,
    pub color: Option<String>,
    pub symbol: Option<String>,
    pub description: Option<String>,
    pub hidden: Option<bool>,
}

pub fn set_bucket(ctx: &Ctx, id: &str, e: BucketEdit, create: bool) -> Result<Config> {
    let mut cfg = ctx.cfg.clone();
    let spec = BucketSpec {
        id: id.to_string(),
        name: e.name,
        order: e.order,
        color: e.color,
        symbol: e.symbol,
        description: e.description,
        hidden: e.hidden,
        ..Default::default()
    };
    let exists = ctx.cat.bucket(id).is_some();
    if create {
        if exists {
            bail!("bucket `{}` already exists; use `poo buckets set`", id);
        }
        let mut s = spec;
        if s.name.is_none() {
            s.name = Some(id.to_string());
        }
        cfg.bucket.push(s);
    } else if let Some(b) = cfg.bucket.iter_mut().find(|b| b.id == id) {
        crate::catalog::merge_bucket(b, &spec);
    } else if exists {
        let entry = cfg.bucket_overrides.entry(id.to_string()).or_default();
        crate::catalog::merge_bucket(entry, &spec);
    } else {
        bail!("unknown bucket `{}`; create it with `poo buckets add {}`", id, id);
    }
    Ok(cfg)
}

pub fn remove_bucket(ctx: &Ctx, id: &str, move_to: Option<&str>) -> Result<Config> {
    let mut cfg = ctx.cfg.clone();
    let using: Vec<&str> = ctx.cat.rules.iter().filter(|r| r.bucket == id).map(|r| r.id.as_str()).collect();
    if cfg.bucket.iter().any(|b| b.id == id) {
        if !using.is_empty() && move_to.is_none() {
            bail!("rules still use `{}` ({}); pass --move-to <bucket>", id, using.join(", "));
        }
        cfg.bucket.retain(|b| b.id != id);
    } else if ctx.cat.bucket(id).is_some() {
        // Built-in bucket: hide it, optionally moving its rules.
        if move_to.is_none() {
            cfg.bucket_overrides.entry(id.to_string()).or_default().hidden = Some(true);
        }
    } else {
        bail!("unknown bucket `{}`", id);
    }
    if let Some(dest) = move_to {
        if ctx.cat.bucket(dest).is_none() && !cfg.bucket.iter().any(|b| b.id == dest) {
            bail!("unknown bucket `{}`", dest);
        }
        for r in using {
            cfg.rule_overrides.entry(r.to_string()).or_default().bucket = Some(dest.to_string());
        }
        if ctx.cat.bucket(id).map_or(false, |b| b.source != "config") {
            cfg.bucket_overrides.entry(id.to_string()).or_default().hidden = Some(true);
        }
    }
    Ok(cfg)
}

pub fn add_root(cfg: &Config, path: &str) -> Result<Config> {
    let p = expand(path);
    let p = std::fs::canonicalize(&p).map_err(|_| anyhow!("{} does not exist", path))?;
    let mut c = cfg.clone();
    if c.roots.is_empty() {
        // First explicit root: keep the detected ones the user was already using.
        c.roots = crate::config::detect_roots().iter().map(|r| tilde(r)).collect();
    }
    let t = tilde(&p);
    if !c.roots.contains(&t) {
        c.roots.push(t);
    }
    Ok(c)
}

pub fn remove_root(cfg: &Config, path: &str) -> Result<Config> {
    let mut c = cfg.clone();
    if c.roots.is_empty() {
        c.roots = crate::config::detect_roots().iter().map(|r| tilde(r)).collect();
    }
    let target = expand(path);
    let target = std::fs::canonicalize(&target).unwrap_or(target);
    let before = c.roots.len();
    c.roots.retain(|r| {
        let e = expand(r);
        std::fs::canonicalize(&e).unwrap_or(e) != target
    });
    if c.roots.len() == before {
        bail!("{} is not a root", path);
    }
    Ok(c)
}

pub fn set_project_policy(cfg: &Config, path: &str, policy: ProjectPolicy, note: Option<String>) -> Result<Config> {
    let p = expand(path);
    let p: PathBuf = std::fs::canonicalize(&p).map_err(|_| anyhow!("{} does not exist", path))?;
    let mut c = cfg.clone();
    let key = p.to_string_lossy().to_string();
    if policy == ProjectPolicy::Auto && note.is_none() {
        c.projects.remove(&key);
    } else {
        let e = c.projects.entry(key).or_default();
        e.policy = Some(policy);
        if note.is_some() {
            e.note = note;
        }
    }
    Ok(c)
}

/// Re-tiers a cached report after policy or rule changes without rescanning the disk.
pub fn retier_hint() -> &'static str {
    "config saved; run `poo scan` to re-tier items with the new settings"
}

pub fn new_mod(id: &str) -> Result<PathBuf> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("mod ids use letters, digits, - and _");
    }
    let dir = crate::config::mods_dir().join(id);
    if dir.exists() {
        bail!("{} already exists", tilde(&dir));
    }
    std::fs::create_dir_all(dir.join("bin"))?;
    std::fs::write(
        dir.join("mod.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\ndescription = \"\"\npriority = 10\n"),
    )?;
    std::fs::write(
        dir.join("rules.toml"),
        r##"# Rules in this file override built-in rules with the same id (only the fields you set)
# and add new ones. Docs: `poo mods docs`. Check with `poo mods validate` and `poo rules explain <path>`.

# A folder your projects produce that poo does not know yet:
# [[rule]]
# id = "team.fixtures"
# name = "Downloaded test fixtures"
# bucket = "build"
# group = "Fixtures"
# kind = "artifact"
# names = ["fixtures-cache"]
# parent_any = ["package.json"]
# cost = "redownload"
# rebuild = "npm run fixtures"
# description = "Large fixtures fetched by the test setup."

# A fixed location:
# [[rule]]
# id = "team.recordings"
# name = "Screen recordings"
# bucket = "junk"
# kind = "path"
# paths = ["~/Movies/Recordings"]
# each = true
# files = true
# cost = "none"
# min_age_days = 30

# Tweak a built-in rule:
# [[rule]]
# id = "rust.target"
# warm_days = 3

# A custom provider in any language (prints {"items": [...]}):
# [[rule]]
# id = "team.custom"
# name = "Custom"
# bucket = "other"
# kind = "provider"
# provider = "exec"
# params.command = ["bin/list.sh"]
# cost = "rebuild"

# A bucket of your own:
# [[bucket]]
# id = "team"
# name = "Team projects"
# order = 5
# color = "#FF9500"
# symbol = "person.3"

# A guard that vetoes cleaning:
# [[guard]]
# id = "team.running-file"
# kind = "file-nonempty"
# file = "{project}/.running"
# description = "Skip projects with an active build listed in .running"
"##,
    )?;
    Ok(dir)
}

pub fn validate_mods(ctx: &Ctx) -> Value {
    json!({
        "ok": ctx.cat.errors.is_empty(),
        "errors": ctx.cat.errors,
        "mods": ctx.cat.mods,
        "rules": ctx.cat.rules.len(),
        "buckets": ctx.cat.buckets.len(),
    })
}

pub fn path_exists(p: &str) -> bool {
    Path::new(&expand(p)).exists()
}
