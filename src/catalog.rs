//! Loads the built-in `core` mod and user mods, merges them by id, applies
//! config overrides and compiles matchers. Everything poo knows passes through here.

use crate::config::{mods_dir, Config};
use crate::model::*;
use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use include_dir::{include_dir, Dir};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

static CORE: Dir = include_dir!("$CARGO_MANIFEST_DIR/rules/core");

pub const PROVIDERS: &[(&str, &str)] = &[
    ("git-worktrees", "Linked git worktrees of every repo found, with dirty, merged and agent status"),
    ("git-ignored", "Large git-ignored folders no other rule explains"),
    ("xcode-derived-data", "Xcode DerivedData folders mapped to their workspace, with orphan detection"),
    ("simulator-devices", "Simulator devices from simctl, with availability and last boot"),
    ("simulator-runtimes", "Installed simulator runtimes and whether any device uses them"),
    ("docker", "Docker build cache, images, containers and volumes from docker system df"),
    ("homebrew", "Old Homebrew versions and downloads that brew cleanup would remove"),
    ("versions", "Old versions in a versions folder, keeping the active one (symlink target) and the newest N"),
    ("app-leftovers", "Caches, logs and support folders of apps that are no longer installed"),
    ("exec", "Any executable in a mod that prints {\"items\": [...]} as JSON"),
];

#[derive(Debug, Clone, Serialize)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub priority: i32,
    pub builtin: bool,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub rules: usize,
    pub buckets: usize,
    pub guards: usize,
}

#[derive(Debug, Clone, Default)]
pub struct NameSet {
    pub raw: Vec<String>,
    globs: Vec<GlobMatcher>,
    set: Option<GlobSet>,
}

impl NameSet {
    fn new(patterns: &[String]) -> Result<NameSet, String> {
        let mut b = GlobSetBuilder::new();
        let mut globs = vec![];
        for p in patterns {
            let g = Glob::new(p).map_err(|e| format!("bad pattern `{}`: {}", p, e))?;
            globs.push(g.compile_matcher());
            b.add(g);
        }
        Ok(NameSet {
            raw: patterns.to_vec(),
            globs,
            set: if patterns.is_empty() { None } else { b.build().ok() },
        })
    }
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
    pub fn matches(&self, name: &str) -> bool {
        self.set.as_ref().map(|s| s.is_match(name)).unwrap_or(false)
    }
    pub fn any(&self, names: &[String]) -> bool {
        names.iter().any(|n| self.matches(n))
    }
    pub fn all(&self, names: &[String]) -> bool {
        self.globs.iter().all(|g| names.iter().any(|n| g.is_match(n)))
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub bucket: String,
    pub group: String,
    pub kind: RuleKind,
    pub description: String,
    pub recommend: Option<String>,
    pub tags: Vec<String>,
    pub stacks: Vec<String>,
    pub enabled: bool,
    pub names: NameSet,
    pub parent_any: NameSet,
    pub parent_all: NameSet,
    pub parent_none: NameSet,
    pub children_any: NameSet,
    pub children_all: NameSet,
    pub children_none: NameSet,
    pub path_contains: Vec<String>,
    pub path_excludes: Vec<String>,
    pub ignored: bool,
    pub paths: Vec<String>,
    pub each: bool,
    pub files: bool,
    pub extensions: Vec<String>,
    pub exclude: NameSet,
    pub min_age_days: u32,
    pub min_bytes: u64,
    pub system: bool,
    pub contents: bool,
    pub running_apps: Vec<String>,
    pub provider: Option<String>,
    pub params: toml::Table,
    pub cost: Cost,
    pub tier: Option<Tier>,
    pub rebuild: Option<String>,
    pub warm_days: u32,
    pub idle_days: u32,
    pub action: ActionKind,
    pub command: Option<Vec<String>>,
    pub source: String,
    pub mod_dir: Option<PathBuf>,
    pub spec: RuleSpec,
    /// Reads other apps' protected data: on only with Full Disk Access or an explicit enable.
    pub needs_permission: bool,
}

impl Rule {
    /// Whether a directory found while walking matches this artifact rule.
    pub fn matches_artifact(&self, name: &str, path: &Path, parent: &[String], children: &[String]) -> bool {
        if self.kind != RuleKind::Artifact || !self.names.matches(name) {
            return false;
        }
        if !self.parent_any.is_empty() && !self.parent_any.any(parent) {
            return false;
        }
        if !self.parent_all.is_empty() && !self.parent_all.all(parent) {
            return false;
        }
        if !self.parent_none.is_empty() && self.parent_none.any(parent) {
            return false;
        }
        if !self.children_any.is_empty() && !self.children_any.any(children) {
            return false;
        }
        if !self.children_all.is_empty() && !self.children_all.all(children) {
            return false;
        }
        if !self.children_none.is_empty() && self.children_none.any(children) {
            return false;
        }
        let p = path.to_string_lossy();
        if !self.path_contains.is_empty() && !self.path_contains.iter().any(|s| p.contains(s.as_str())) {
            return false;
        }
        if self.path_excludes.iter().any(|s| p.contains(s.as_str())) {
            return false;
        }
        true
    }

    pub fn param_str(&self, key: &str) -> Option<String> {
        self.params.get(key).and_then(|v| v.as_str()).map(String::from)
    }
    pub fn param_u64(&self, key: &str) -> Option<u64> {
        self.params.get(key).and_then(|v| v.as_integer()).map(|v| v.max(0) as u64)
    }
    pub fn param_list(&self, key: &str) -> Vec<String> {
        self.params
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub id: String,
    pub name: String,
    pub order: i32,
    pub color: String,
    pub symbol: String,
    pub description: String,
    pub hidden: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub description: String,
    pub order: i32,
    pub tiers: Vec<Tier>,
    pub max_cost: Cost,
    pub exclude_buckets: Vec<String>,
    pub greedy: bool,
    pub target_free_percent: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Guard {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub file: Option<String>,
    pub names: Vec<String>,
    pub command: Option<Vec<String>>,
    pub source: String,
    #[serde(skip)]
    pub mod_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stack {
    pub id: String,
    pub name: String,
    pub markers: Vec<String>,
    #[serde(skip)]
    pub set: NameSet,
}

#[derive(Debug, Clone, Serialize)]
pub struct Launcher {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub command: Vec<String>,
    pub url: Option<String>,
    pub detect: Vec<String>,
    pub detect_app: Vec<String>,
    pub order: i32,
    pub source: String,
}

impl Launcher {
    pub fn available(&self) -> bool {
        let bin = self.detect.iter().any(|b| crate::util::which(b).is_some());
        let app = self.detect_app.iter().any(|a| {
            std::path::Path::new("/Applications").join(a).exists() || crate::util::home().join("Applications").join(a).exists()
        });
        (self.detect.is_empty() && self.detect_app.is_empty()) || bin || app
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeHome {
    pub path: String,
    pub agent: String,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub mods: Vec<ModInfo>,
    pub rules: Vec<Rule>,
    pub buckets: Vec<Bucket>,
    pub plans: Vec<Plan>,
    pub guards: Vec<Guard>,
    pub roots: Vec<String>,
    pub worktree_homes: Vec<WorktreeHome>,
    pub stacks: Vec<Stack>,
    pub launchers: Vec<Launcher>,
    pub errors: Vec<String>,
}

impl Catalog {
    /// Stack ids whose markers appear among a folder's entries.
    pub fn stacks_for(&self, entries: &[String]) -> Vec<String> {
        self.stacks.iter().filter(|s| s.set.any(entries)).map(|s| s.id.clone()).collect()
    }
    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }
    pub fn bucket(&self, id: &str) -> Option<&Bucket> {
        self.buckets.iter().find(|b| b.id == id)
    }
    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }
    pub fn enabled_rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(|r| r.enabled)
    }
}

struct LoadedMod {
    info: ModInfo,
    dir: Option<PathBuf>,
    files: Vec<(String, ModFile)>,
}

fn parse_file(label: &str, text: &str, errors: &mut Vec<String>) -> Option<ModFile> {
    match toml::from_str::<ModFile>(text) {
        Ok(f) => Some(f),
        Err(e) => {
            errors.push(format!("{}: {}", label, e.to_string().trim()));
            None
        }
    }
}

fn load_core(errors: &mut Vec<String>) -> LoadedMod {
    let mut files = vec![];
    let mut manifest = ModFile::default();
    // POO_CORE_DIR loads the core mod from disk, for developing rules without rebuilding.
    let mut sources: Vec<(String, String)> = match std::env::var_os("POO_CORE_DIR") {
        Some(dir) => std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        std::fs::read_to_string(e.path()).ok().map(|t| (n, t))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => CORE
            .files()
            .map(|f| (f.path().to_string_lossy().to_string(), f.contents_utf8().unwrap_or_default().to_string()))
            .collect(),
    };
    sources.sort();
    for (name, text) in sources {
        if !name.ends_with(".toml") {
            continue;
        }
        let text = text.as_str();
        if let Some(parsed) = parse_file(&format!("core/{}", name), text, errors) {
            if name == "mod.toml" {
                manifest = parsed.clone();
            }
            files.push((name, parsed));
        }
    }
    LoadedMod {
        info: ModInfo {
            id: "core".into(),
            name: manifest.name.unwrap_or_else(|| "poo core".into()),
            version: env!("CARGO_PKG_VERSION").into(),
            description: manifest.description.unwrap_or_default(),
            priority: 0,
            builtin: true,
            enabled: true,
            path: None,
            rules: 0,
            buckets: 0,
            guards: 0,
        },
        dir: None,
        files,
    }
}

fn load_user_mods(errors: &mut Vec<String>) -> Vec<LoadedMod> {
    let dir = mods_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
    let mut mods = vec![];
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        if fname.starts_with('.') {
            continue;
        }
        let mut files = vec![];
        let (manifest_path, mod_dir) = if p.is_dir() {
            (p.join("mod.toml"), Some(p.clone()))
        } else if fname.ends_with(".toml") {
            (p.clone(), None)
        } else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            errors.push(format!("{}: missing mod.toml", p.display()));
            continue;
        };
        let Some(manifest) = parse_file(&manifest_path.display().to_string(), &text, errors) else {
            continue;
        };
        files.push(("mod.toml".to_string(), manifest.clone()));
        if let Some(d) = &mod_dir {
            let mut extra: Vec<PathBuf> = std::fs::read_dir(d)
                .map(|rd| rd.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            extra.sort();
            for f in extra {
                let n = f.file_name().unwrap().to_string_lossy().to_string();
                if n == "mod.toml" || !n.ends_with(".toml") {
                    continue;
                }
                if let Ok(t) = std::fs::read_to_string(&f) {
                    if let Some(parsed) = parse_file(&f.display().to_string(), &t, errors) {
                        files.push((n, parsed));
                    }
                }
            }
        }
        let id = manifest
            .id
            .clone()
            .unwrap_or_else(|| fname.trim_end_matches(".toml").to_string());
        mods.push(LoadedMod {
            info: ModInfo {
                id,
                name: manifest.name.clone().unwrap_or_else(|| fname.clone()),
                version: manifest.version.clone().unwrap_or_else(|| "0.0.0".into()),
                description: manifest.description.clone().unwrap_or_default(),
                priority: manifest.priority.unwrap_or(10),
                builtin: false,
                enabled: true,
                path: Some(p.display().to_string()),
                rules: 0,
                buckets: 0,
                guards: 0,
            },
            dir: mod_dir.or_else(|| p.parent().map(|x| x.to_path_buf())),
            files,
        });
    }
    mods.sort_by(|a, b| (a.info.priority, &a.info.id).cmp(&(b.info.priority, &b.info.id)));
    mods
}

macro_rules! merge_opt {
    ($base:expr, $over:expr, $($f:ident),*) => {
        $( if $over.$f.is_some() { $base.$f = $over.$f.clone(); } )*
    };
}

pub fn merge_rule(base: &mut RuleSpec, o: &RuleSpec) {
    merge_opt!(
        base, o, name, bucket, group, kind, description, recommend, tags, stacks, enabled, names, parent_any,
        parent_all, parent_none, children_any, children_all, children_none, path_contains,
        path_excludes, ignored, paths, each, files, extensions, exclude, min_age_days, min_bytes, system, contents, running_apps,
        provider, params, cost, tier, rebuild, warm_days, idle_days, action, command
    );
}

pub fn merge_bucket(base: &mut BucketSpec, o: &BucketSpec) {
    merge_opt!(base, o, name, order, color, symbol, description, hidden);
}

fn merge_plan(base: &mut PlanSpec, o: &PlanSpec) {
    merge_opt!(base, o, name, description, order, tiers, max_cost, exclude_buckets, greedy, target_free_percent);
}

/// Ordered map keeping first-definition order.
struct Ordered<T> {
    order: Vec<String>,
    map: HashMap<String, (T, String, Option<PathBuf>)>,
}

impl<T> Ordered<T> {
    fn new() -> Self {
        Self { order: vec![], map: HashMap::new() }
    }
}

pub fn load(cfg: &Config) -> Catalog {
    let mut errors = vec![];
    let mut all = vec![load_core(&mut errors)];
    all.extend(load_user_mods(&mut errors));

    let mut rules: Ordered<RuleSpec> = Ordered::new();
    let mut buckets: Ordered<BucketSpec> = Ordered::new();
    let mut plans: Ordered<PlanSpec> = Ordered::new();
    let mut guards: Vec<Guard> = vec![];
    let mut roots = vec![];
    let mut homes: Vec<WorktreeHome> = vec![];
    let mut stacks: Vec<Stack> = vec![];
    let mut launchers: Vec<Launcher> = vec![];
    let mut mods = vec![];

    for m in &mut all {
        let enabled = !cfg.disabled_mods.contains(&m.info.id);
        m.info.enabled = enabled;
        for (_, f) in &m.files {
            m.info.rules += f.rule.len();
            m.info.buckets += f.bucket.len();
            m.info.guards += f.guard.len();
        }
        mods.push(m.info.clone());
        if !enabled {
            continue;
        }
        for (fname, f) in &m.files {
            let label = format!("{}/{}", m.info.id, fname);
            for b in &f.bucket {
                if b.id.is_empty() {
                    errors.push(format!("{}: bucket without id", label));
                    continue;
                }
                match buckets.map.get_mut(&b.id) {
                    Some((base, _, _)) => merge_bucket(base, b),
                    None => {
                        buckets.order.push(b.id.clone());
                        buckets.map.insert(b.id.clone(), (b.clone(), m.info.id.clone(), m.dir.clone()));
                    }
                }
            }
            for r in &f.rule {
                if r.id.is_empty() {
                    errors.push(format!("{}: rule without id", label));
                    continue;
                }
                match rules.map.get_mut(&r.id) {
                    Some((base, src, _)) => {
                        merge_rule(base, r);
                        if !src.contains(&m.info.id) {
                            src.push('+');
                            src.push_str(&m.info.id);
                        }
                    }
                    None => {
                        rules.order.push(r.id.clone());
                        rules.map.insert(r.id.clone(), (r.clone(), m.info.id.clone(), m.dir.clone()));
                    }
                }
            }
            for p in &f.plan {
                match plans.map.get_mut(&p.id) {
                    Some((base, _, _)) => merge_plan(base, p),
                    None => {
                        plans.order.push(p.id.clone());
                        plans.map.insert(p.id.clone(), (p.clone(), m.info.id.clone(), None));
                    }
                }
            }
            for g in &f.guard {
                guards.retain(|x| x.id != g.id);
                if g.enabled == Some(false) || cfg.disabled_guards.contains(&g.id) {
                    continue;
                }
                guards.push(Guard {
                    id: g.id.clone(),
                    kind: g.kind.clone(),
                    description: g.description.clone().unwrap_or_default(),
                    file: g.file.clone(),
                    names: g.names.clone().unwrap_or_default(),
                    command: g.command.clone(),
                    source: m.info.id.clone(),
                    mod_dir: m.dir.clone(),
                });
            }
            for st in &f.stack {
                stacks.retain(|x| x.id != st.id);
                match NameSet::new(&st.markers) {
                    Ok(set) => stacks.push(Stack {
                        id: st.id.clone(),
                        name: st.name.clone().unwrap_or_else(|| st.id.clone()),
                        markers: st.markers.clone(),
                        set,
                    }),
                    Err(e) => errors.push(format!("{}: stack `{}`: {}", label, st.id, e)),
                }
            }
            for l in &f.launcher {
                let prev = launchers.iter().position(|x| x.id == l.id);
                let base = prev.map(|i| launchers.remove(i));
                if l.enabled == Some(false) {
                    continue;
                }
                launchers.push(Launcher {
                    id: l.id.clone(),
                    name: l.name.clone().or_else(|| base.as_ref().map(|b| b.name.clone())).unwrap_or_else(|| l.id.clone()),
                    kind: l.kind.clone().or_else(|| base.as_ref().map(|b| b.kind.clone())).unwrap_or_else(|| "terminal".into()),
                    command: l.command.clone().or_else(|| base.as_ref().map(|b| b.command.clone())).unwrap_or_default(),
                    url: l.url.clone().or_else(|| base.as_ref().and_then(|b| b.url.clone())),
                    detect: l.detect.clone().or_else(|| base.as_ref().map(|b| b.detect.clone())).unwrap_or_default(),
                    detect_app: l.detect_app.clone().or_else(|| base.as_ref().map(|b| b.detect_app.clone())).unwrap_or_default(),
                    order: l.order.or_else(|| base.as_ref().map(|b| b.order)).unwrap_or(100),
                    source: m.info.id.clone(),
                });
            }
            roots.extend(f.root.iter().map(|r| r.path.clone()));
            for h in &f.worktree_home {
                homes.retain(|x| x.path != h.path);
                homes.push(WorktreeHome { path: h.path.clone(), agent: h.agent.clone() });
            }
        }
    }

    // Config-level buckets, overrides and rule tweaks win over every mod.
    for b in &cfg.bucket {
        match buckets.map.get_mut(&b.id) {
            Some((base, _, _)) => merge_bucket(base, b),
            None => {
                buckets.order.push(b.id.clone());
                buckets.map.insert(b.id.clone(), (b.clone(), "config".into(), None));
            }
        }
    }
    for (id, b) in &cfg.bucket_overrides {
        if let Some((base, _, _)) = buckets.map.get_mut(id) {
            merge_bucket(base, b);
        } else {
            errors.push(format!("config: bucket_overrides.{} names an unknown bucket", id));
        }
    }
    for (id, r) in &cfg.rule_overrides {
        if let Some((base, src, _)) = rules.map.get_mut(id) {
            merge_rule(base, r);
            if !src.ends_with("+config") {
                src.push_str("+config");
            }
        } else {
            errors.push(format!("config: rule_overrides.{} names an unknown rule", id));
        }
    }
    for (id, p) in &cfg.plan_overrides {
        if let Some((base, _, _)) = plans.map.get_mut(id) {
            merge_plan(base, p);
        }
    }
    for id in &cfg.disabled_rules {
        if let Some((base, _, _)) = rules.map.get_mut(id) {
            base.enabled = Some(false);
        }
    }

    let mut bucket_list: Vec<Bucket> = buckets
        .order
        .iter()
        .map(|id| {
            let (b, src, _) = &buckets.map[id];
            Bucket {
                id: id.clone(),
                name: b.name.clone().unwrap_or_else(|| id.clone()),
                order: b.order.unwrap_or(100),
                color: b.color.clone().unwrap_or_else(|| "#8E8E93".into()),
                symbol: b.symbol.clone().unwrap_or_else(|| "folder".into()),
                description: b.description.clone().unwrap_or_default(),
                hidden: b.hidden.unwrap_or(false),
                source: src.clone(),
            }
        })
        .collect();
    bucket_list.sort_by_key(|b| b.order);

    let mut plan_list: Vec<Plan> = plans
        .order
        .iter()
        .map(|id| {
            let (p, _, _) = &plans.map[id];
            Plan {
                id: id.clone(),
                name: p.name.clone().unwrap_or_else(|| id.clone()),
                description: p.description.clone().unwrap_or_default(),
                order: p.order.unwrap_or(100),
                tiers: p.tiers.clone().unwrap_or_else(|| vec![Tier::Junk]),
                max_cost: p.max_cost.unwrap_or(Cost::Redownload),
                exclude_buckets: p.exclude_buckets.clone().unwrap_or_default(),
                greedy: p.greedy.unwrap_or(false),
                target_free_percent: p.target_free_percent,
            }
        })
        .collect();
    plan_list.sort_by_key(|p| p.order);

    // Rules that read other apps' protected data run only with Full Disk Access, so a scan
    // never triggers "Data Access Blocked" or permission prompts. A config override that
    // enables one explicitly wins.
    let fda = crate::util::has_full_disk_access();
    let mut compiled = vec![];
    for id in &rules.order {
        let (spec, src, dir) = &rules.map[id];
        match compile_rule(spec, src, dir.clone(), &bucket_list) {
            Ok(mut r) => {
                if r.tags.iter().any(|t| t == "needs-permission") {
                    let explicit = cfg.rule_overrides.get(id).and_then(|o| o.enabled);
                    let disabled = cfg.disabled_rules.contains(id);
                    r.needs_permission = true;
                    r.enabled = !disabled && explicit.unwrap_or(fda);
                }
                compiled.push(r)
            }
            Err(e) => errors.push(format!("rule `{}` ({}): {}", id, src, e)),
        }
    }

    Catalog {
        mods,
        rules: compiled,
        buckets: bucket_list,
        plans: plan_list,
        guards,
        roots,
        worktree_homes: homes,
        stacks,
        launchers: {
            let mut l = launchers;
            l.sort_by_key(|x| x.order);
            l
        },
        errors,
    }
}

fn compile_rule(s: &RuleSpec, src: &str, dir: Option<PathBuf>, buckets: &[Bucket]) -> Result<Rule, String> {
    let kind = s.kind.ok_or("missing `kind` (artifact, path or provider)")?;
    let name = s.name.clone().ok_or("missing `name`")?;
    let bucket = s.bucket.clone().ok_or("missing `bucket`")?;
    if !buckets.iter().any(|b| b.id == bucket) {
        return Err(format!("unknown bucket `{}`", bucket));
    }
    let cost = s.cost.ok_or("missing `cost` (none, rebuild, redownload or data)")?;
    let ns = |v: &Option<Vec<String>>| NameSet::new(v.as_deref().unwrap_or(&[]));
    match kind {
        RuleKind::Artifact if s.names.as_ref().map_or(true, |n| n.is_empty()) => {
            return Err("artifact rules need `names`".into())
        }
        RuleKind::Path if s.paths.as_ref().map_or(true, |n| n.is_empty()) => {
            return Err("path rules need `paths`".into())
        }
        RuleKind::Provider => {
            let p = s.provider.as_deref().ok_or("provider rules need `provider`")?;
            if !PROVIDERS.iter().any(|(id, _)| *id == p) {
                return Err(format!("unknown provider `{}`", p));
            }
        }
        _ => {}
    }
    let action = s.action.unwrap_or(match kind {
        RuleKind::Provider => ActionKind::Command,
        _ => ActionKind::Delete,
    });
    if action == ActionKind::Command && s.command.is_none() && kind != RuleKind::Provider {
        return Err("action `command` needs `command`".into());
    }
    Ok(Rule {
        id: s.id.clone(),
        group: s.group.clone().unwrap_or_else(|| name.clone()),
        name,
        bucket,
        kind,
        description: s.description.clone().unwrap_or_default(),
        recommend: s.recommend.clone(),
        tags: s.tags.clone().unwrap_or_default(),
        stacks: s.stacks.clone().unwrap_or_default(),
        enabled: s.enabled.unwrap_or(true),
        names: ns(&s.names)?,
        parent_any: ns(&s.parent_any)?,
        parent_all: ns(&s.parent_all)?,
        parent_none: ns(&s.parent_none)?,
        children_any: ns(&s.children_any)?,
        children_all: ns(&s.children_all)?,
        children_none: ns(&s.children_none)?,
        path_contains: s.path_contains.clone().unwrap_or_default(),
        path_excludes: s.path_excludes.clone().unwrap_or_default(),
        ignored: s.ignored.unwrap_or(false),
        paths: s.paths.clone().unwrap_or_default(),
        each: s.each.unwrap_or(false),
        files: s.files.unwrap_or(false),
        extensions: s.extensions.clone().unwrap_or_default(),
        exclude: ns(&s.exclude)?,
        min_age_days: s.min_age_days.unwrap_or(0),
        min_bytes: s.min_bytes.unwrap_or(0),
        system: s.system.unwrap_or(false),
        contents: s.contents.unwrap_or(false),
        running_apps: s.running_apps.clone().unwrap_or_default(),
        provider: s.provider.clone(),
        params: s.params.clone().unwrap_or_default(),
        cost,
        tier: s.tier,
        rebuild: s.rebuild.clone(),
        warm_days: s.warm_days.unwrap_or(0),
        idle_days: s.idle_days.unwrap_or(0),
        action,
        command: s.command.clone(),
        source: src.to_string(),
        mod_dir: dir,
        spec: s.clone(),
        needs_permission: false,
    })
}
