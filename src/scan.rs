//! The scanner: expands path rules, runs providers, walks dev roots and agent
//! worktree homes for artifacts and projects, then classifies everything.

use crate::catalog::{Catalog, Rule};
use crate::config::{state_dir, Config};
use crate::fsutil::{expand_glob, has_glob, names, read_entries, Entry, SizeCtx, Sized};
use crate::model::*;
use crate::procs::{self, Proc};
use crate::providers::{self, git, Raw};
use crate::util::{expand, home, now, tilde};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub phase: String,
    pub detail: String,
    pub files: u64,
    pub bytes: u64,
    pub items: usize,
    pub elapsed_ms: u64,
}

#[derive(Default)]
pub struct ScanOptions {
    /// Walk only these roots instead of the configured ones.
    pub roots: Option<Vec<PathBuf>>,
    /// Skip providers (simulators, docker, brew...) for a faster scan.
    pub no_providers: bool,
    /// Skip global path rules.
    pub roots_only: bool,
    pub on_progress: Option<Box<dyn Fn(&Progress) + Send + Sync>>,
}

#[derive(Debug, Clone)]
pub struct FoundProject {
    pub path: PathBuf,
    pub kind: &'static str,
    pub repo: Option<PathBuf>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub stacks: Vec<String>,
    pub last_activity: Option<i64>,
    pub agent: Option<String>,
    pub worktrees: Vec<git::WorktreeInfo>,
}

struct ProjCtx {
    path: PathBuf,
    ignored: HashSet<PathBuf>,
}

struct Walker<'a> {
    cat: &'a Catalog,
    rules: Vec<&'a Rule>,
    ignored_rule: Option<&'a Rule>,
    ctx: &'a SizeCtx,
    exclude: HashSet<PathBuf>,
    exclude_globs: globset::GlobSet,
    max_depth: usize,
    items: Mutex<Vec<Raw>>,
    projects: Mutex<Vec<FoundProject>>,
    known_worktrees: Mutex<HashSet<PathBuf>>,
    homes: Vec<(PathBuf, String)>,
    dirs: AtomicUsize,
    current: &'a Mutex<String>,
}

const DATA_NAMES: &[&str] = &[
    "data", "db", "database", "databases", "storage", "uploads", "backups", "backup", "postgres",
    "pgdata", "mysql", "redis", "minio", "volumes", ".data", "secrets", "certs", "keys", "models",
];

impl<'a> Walker<'a> {
    fn excluded(&self, p: &Path) -> bool {
        self.exclude.contains(p) || self.exclude_globs.is_match(p)
    }

    fn discover_project(&self, dir: &Path, entries: &[Entry], names: &[String]) -> Option<Arc<ProjCtx>> {
        let dotgit = entries.iter().find(|e| e.name == ".git")?;
        let mut proj = FoundProject {
            path: dir.to_path_buf(),
            kind: "repo",
            repo: None,
            branch: None,
            head: None,
            stacks: self.cat.stacks_for(names),
            last_activity: git::last_activity(dir),
            agent: git::agent_for_path(dir, &self.homes),
            worktrees: vec![],
        };
        if dotgit.is_dir {
            proj.worktrees = git::worktrees(dir);
            if let Some(main) = proj.worktrees.first() {
                proj.branch = main.branch.clone();
                proj.head = main.head.clone();
            }
            let mut known = self.known_worktrees.lock().unwrap();
            for w in proj.worktrees.iter().skip(1) {
                known.insert(w.path.clone());
            }
        } else {
            match git::git_dir(dir) {
                Some(gd) if gd.exists() => {
                    if gd.to_string_lossy().contains("/modules/") {
                        proj.kind = "submodule";
                    } else {
                        proj.kind = "worktree";
                        let common = std::fs::read_to_string(gd.join("commondir"))
                            .ok()
                            .map(|c| {
                                let c = PathBuf::from(c.trim());
                                if c.is_absolute() { c } else { gd.join(c) }
                            });
                        proj.repo = common
                            .and_then(|c| std::fs::canonicalize(c).ok())
                            .and_then(|c| c.parent().map(|p| p.to_path_buf()));
                    }
                    if let Ok(head) = std::fs::read_to_string(gd.join("HEAD")) {
                        let head = head.trim();
                        if let Some(b) = head.strip_prefix("ref: refs/heads/") {
                            proj.branch = Some(b.to_string());
                        } else {
                            proj.head = Some(head.to_string());
                        }
                    }
                }
                _ => proj.kind = "orphan",
            }
        }
        let ignored = if self.ignored_rule.is_some() && proj.kind != "orphan" {
            git::ignored_dirs(dir).into_iter().collect()
        } else {
            HashSet::new()
        };
        self.projects.lock().unwrap().push(proj);
        Some(Arc::new(ProjCtx { path: dir.to_path_buf(), ignored }))
    }

    fn label(&self, path: &Path, proj: &Option<Arc<ProjCtx>>) -> String {
        if let Some(p) = proj {
            if let Ok(rel) = path.strip_prefix(&p.path) {
                let pname = p.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                return format!("{}/{}", pname, rel.display());
            }
        }
        tilde(path)
    }

    /// Walks a directory. Returns bytes and newest mtime not explained by any item,
    /// so ignored folders can report only what no rule recognised.
    fn visit(&self, dir: &Path, entries: &[Entry], depth: usize, proj: Option<Arc<ProjCtx>>, dev: u64) -> (u64, i64) {
        if self.ctx.cancel.load(Ordering::Relaxed) {
            return (0, 0);
        }
        let n = self.dirs.fetch_add(1, Ordering::Relaxed);
        if n % 512 == 0 {
            *self.current.lock().unwrap() = tilde(dir);
        }
        let entry_names = names(entries);
        let mut proj = proj;
        let mut is_worktree_root = false;
        if entry_names.iter().any(|n| n == ".git") {
            if let Some(p) = self.discover_project(dir, entries, &entry_names) {
                is_worktree_root = entries.iter().any(|e| e.name == ".git" && !e.is_dir);
                proj = Some(p);
            }
        } else if proj.is_none() {
            let stacks = self.cat.stacks_for(&entry_names);
            if !stacks.is_empty() {
                self.projects.lock().unwrap().push(FoundProject {
                    path: dir.to_path_buf(),
                    kind: "folder",
                    repo: None,
                    branch: None,
                    head: None,
                    stacks,
                    last_activity: entries.iter().map(|e| e.mtime).max(),
                    agent: None,
                    worktrees: vec![],
                });
                proj = Some(Arc::new(ProjCtx { path: dir.to_path_buf(), ignored: HashSet::new() }));
            }
        }
        let mut own = 0u64;
        let mut newest = 0i64;
        let mut files = 0u64;
        for e in entries.iter().filter(|e| !e.is_dir || e.is_symlink) {
            own += e.alloc;
            newest = newest.max(e.mtime);
            files += 1;
        }
        self.ctx.bytes.fetch_add(own, Ordering::Relaxed);
        self.ctx.files.fetch_add(files, Ordering::Relaxed);

        let min_ignored = self.ignored_rule.and_then(|r| r.param_u64("min_bytes")).unwrap_or(50_000_000);
        let mut recurse = vec![];
        for e in entries {
            if !e.is_dir || e.is_symlink || e.dev != dev || e.name == ".git" || self.excluded(&e.path) {
                continue;
            }
            let child = read_entries(&e.path);
            let child_names = names(&child);
            let nested_repo = child_names.iter().any(|n| n == ".git");
            let is_ignored = proj.as_ref().map_or(false, |p| p.ignored.contains(&e.path));
            let matched = if nested_repo {
                None
            } else {
                self.rules.iter().find(|r| {
                    (!r.ignored || is_ignored) && r.matches_artifact(&e.name, &e.path, &entry_names, &child_names)
                })
            };
            if let Some(rule) = matched {
                let (mut s, children) = self.ctx.size_entries(&child, dev, &HashSet::new());
                s.bytes += e.alloc;
                s.newest = s.newest.max(e.mtime);
                self.items.lock().unwrap().push(Raw {
                    path: e.path.clone(),
                    name: self.label(&e.path, &proj),
                    rule: rule.id.clone(),
                    sized: Some((s, children)),
                    project: proj.as_ref().map(|p| p.path.clone()),
                    ..Default::default()
                });
                continue;
            }
            if depth < self.max_depth {
                let track = is_ignored && !nested_repo && self.ignored_rule.is_some();
                recurse.push((e, child, track));
            }
        }
        let results: Vec<(u64, i64)> = recurse
            .into_par_iter()
            .map(|(e, ents, track)| {
                let (b, n) = self.visit(&e.path, &ents, depth + 1, proj.clone(), dev);
                let b = b + e.alloc;
                let n = n.max(e.mtime);
                if track {
                    if b >= min_ignored {
                        let rule = self.ignored_rule.unwrap();
                        let mut raw = Raw {
                            path: e.path.clone(),
                            name: self.label(&e.path, &proj),
                            rule: rule.id.clone(),
                            sized: Some((Sized { bytes: b, files: 0, newest: n }, vec![])),
                            project: proj.as_ref().map(|p| p.path.clone()),
                            disjoint: true,
                            ..Default::default()
                        };
                        let lname = e.name.to_lowercase();
                        if DATA_NAMES.iter().any(|d| *d == lname) || lname.starts_with(".env") {
                            raw.tier = Some(Tier::Protected);
                            raw.reasons.push("name suggests it holds data".into());
                        } else {
                            raw.reasons.push("git-ignored, no rule explains it".into());
                        }
                        self.items.lock().unwrap().push(raw);
                    }
                    return (0, n);
                }
                (b, n)
            })
            .collect();
        for (b, n) in results {
            own += b;
            newest = newest.max(n);
        }
        if is_worktree_root {
            // The worktree item accounts for its own leftover bytes.
            return (0, newest);
        }
        (own, newest)
    }

    fn walk_root(&self, root: &Path) {
        let Some(dev) = crate::fsutil::dev_of(root) else { return };
        let entries = read_entries(root);
        self.visit(root, &entries, 0, None, dev);
    }
}

fn dedupe_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    let mut out: Vec<PathBuf> = vec![];
    for r in roots {
        if !out.iter().any(|o| r.starts_with(o)) {
            out.push(r);
        }
    }
    out
}

/// Expands every enabled path rule into candidate paths. Exact paths beat `each` children.
fn path_candidates(cat: &Catalog) -> Vec<(PathBuf, &Rule, bool)> {
    let mut found: Vec<(PathBuf, &Rule, bool)> = vec![];
    for rule in cat.enabled_rules().filter(|r| r.kind == RuleKind::Path) {
        for p in &rule.paths {
            let pattern = expand(p);
            let matches = if has_glob(&pattern.to_string_lossy()) {
                expand_glob(&pattern)
            } else if std::fs::symlink_metadata(&pattern).is_ok() {
                vec![pattern]
            } else {
                vec![]
            };
            for m in matches {
                if rule.each {
                    for e in read_entries(&m) {
                        if e.is_symlink || rule.exclude.matches(&e.name) || e.name == ".DS_Store" {
                            continue;
                        }
                        if !e.is_dir {
                            if !rule.files {
                                continue;
                            }
                            if !rule.extensions.is_empty() {
                                let ext = Path::new(&e.name)
                                    .extension()
                                    .map(|x| x.to_string_lossy().to_lowercase())
                                    .unwrap_or_default();
                                if !rule.extensions.iter().any(|x| x.eq_ignore_ascii_case(&ext)) {
                                    continue;
                                }
                            }
                        }
                        found.push((e.path, rule, true));
                    }
                } else {
                    let name = m.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if rule.exclude.matches(&name) {
                        continue;
                    }
                    found.push((m, rule, false));
                }
            }
        }
    }
    // Same path claimed twice: an exact rule wins over a generic `each` expansion.
    let mut best: HashMap<PathBuf, usize> = HashMap::new();
    for (i, (p, _, each)) in found.iter().enumerate() {
        match best.get(p) {
            Some(&j) if !(found[j].2 && !each) => {}
            _ => {
                best.insert(p.clone(), i);
            }
        }
    }
    let keep: HashSet<usize> = best.values().copied().collect();
    found.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, x)| x).collect()
}

fn rule_with_provider<'a>(cat: &'a Catalog, provider: &str) -> Vec<&'a Rule> {
    cat.enabled_rules()
        .filter(|r| r.kind == RuleKind::Provider && r.provider.as_deref() == Some(provider))
        .collect()
}

pub fn scan(cfg: &Config, cat: &Catalog, opts: &ScanOptions) -> Report {
    let started = Instant::now();
    let ctx = SizeCtx::default();
    let phase = Mutex::new(String::from("starting"));
    let current = Mutex::new(String::new());
    let item_count = AtomicUsize::new(0);
    let mut warnings: Vec<String> = cat.errors.iter().map(|e| format!("mod error: {}", e)).collect();

    let (cfg_roots, detected) = cfg.effective_roots();
    let mut roots: Vec<PathBuf> = opts.roots.clone().unwrap_or(cfg_roots);
    if opts.roots.is_none() {
        if detected {
            warnings.push("no roots configured; using detected folders (run `poo roots add <path>`)".into());
        }
        roots.extend(cat.roots.iter().map(|r| expand(r)).filter(|p| p.is_dir()));
    }
    let homes: Vec<(PathBuf, String)> = cat
        .worktree_homes
        .iter()
        .map(|h| (expand(&h.path), h.agent.clone()))
        .filter(|(p, _)| p.is_dir())
        .collect();
    if opts.roots.is_none() {
        roots.extend(homes.iter().map(|(p, _)| p.clone()));
    }
    let roots: Vec<PathBuf> = dedupe_roots(
        roots.into_iter().filter_map(|r| std::fs::canonicalize(&r).ok()).collect(),
    );

    let done = std::sync::atomic::AtomicBool::new(false);
    let procs_list: Vec<Proc> = procs::list();

    let report_progress = |final_tick: bool| {
        if let Some(cb) = &opts.on_progress {
            cb(&Progress {
                phase: phase.lock().unwrap().clone(),
                detail: current.lock().unwrap().clone(),
                files: ctx.files.load(Ordering::Relaxed),
                bytes: ctx.bytes.load(Ordering::Relaxed),
                items: item_count.load(Ordering::Relaxed),
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        let _ = final_tick;
    };

    let mut raws: Vec<Raw> = vec![];
    let mut found_projects: Vec<FoundProject> = vec![];

    std::thread::scope(|scope| {
        // Progress ticker.
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                report_progress(false);
            }
        });

        // Phase A: cheap discovery so roots never double count global items.
        *phase.lock().unwrap() = "discovering".into();
        let candidates = if opts.roots_only { vec![] } else { path_candidates(cat) };
        let devices_json = if opts.no_providers || rule_with_provider(cat, "simulator-devices").is_empty() {
            None
        } else {
            providers::simctl_devices()
        };
        let mut provider_items: Vec<Raw> = vec![];
        if !opts.no_providers {
            for r in rule_with_provider(cat, "xcode-derived-data") {
                provider_items.extend(providers::xcode_derived_data(r));
            }
            for r in rule_with_provider(cat, "versions") {
                provider_items.extend(providers::versions(r));
            }
            for r in rule_with_provider(cat, "simulator-devices") {
                provider_items.extend(providers::simulator_devices(r, devices_json.as_ref()));
            }
        }
        let mut claimed: HashSet<PathBuf> = candidates.iter().map(|(p, _, _)| p.clone()).collect();
        claimed.extend(provider_items.iter().filter(|r| !r.is_virtual).map(|r| r.path.clone()));

        // Exclusions for root walks.
        let mut exclude: HashSet<PathBuf> = claimed.clone();
        exclude.insert(home().join("Library"));
        exclude.insert(home().join(".Trash"));
        let mut gb = globset::GlobSetBuilder::new();
        for x in &cfg.scan.exclude {
            if let Ok(g) = globset::Glob::new(&expand(x).to_string_lossy()) {
                gb.add(g);
            }
        }
        let walker = Walker {
            cat,
            rules: cat.enabled_rules().filter(|r| r.kind == RuleKind::Artifact).collect(),
            ignored_rule: rule_with_provider(cat, "git-ignored").into_iter().next(),
            ctx: &ctx,
            exclude,
            exclude_globs: gb.build().unwrap_or_else(|_| globset::GlobSet::empty()),
            max_depth: cfg.scan.max_depth,
            items: Mutex::new(vec![]),
            projects: Mutex::new(vec![]),
            known_worktrees: Mutex::new(HashSet::new()),
            homes: homes.clone(),
            dirs: AtomicUsize::new(0),
            current: &current,
        };

        *phase.lock().unwrap() = "scanning".into();
        let dj = devices_json.clone();
        let no_providers = opts.no_providers;
        let slow_providers = scope.spawn(move || {
            let devices_json = dj;
            let mut out: Vec<Raw> = vec![];
            let mut warn = vec![];
            if no_providers {
                return (out, warn);
            }
            for r in rule_with_provider(cat, "simulator-runtimes") {
                out.extend(providers::simulator_runtimes(r, devices_json.as_ref()));
            }
            for r in rule_with_provider(cat, "docker") {
                out.extend(providers::docker(r));
            }
            for r in rule_with_provider(cat, "homebrew") {
                out.extend(providers::homebrew(r));
            }
            for r in rule_with_provider(cat, "exec") {
                match providers::exec(r) {
                    Ok(items) => out.extend(items),
                    Err(e) => warn.push(format!("rule `{}`: {}", r.id, e)),
                }
            }
            (out, warn)
        });

        // Size global candidates and provider dirs while roots are walked.
        let (_, sized_globals) = rayon::join(
            || roots.par_iter().for_each(|r| walker.walk_root(r)),
            || {
                let all_paths: Vec<PathBuf> = claimed.iter().cloned().collect();
                let mut globals: Vec<Raw> = candidates
                    .par_iter()
                    .filter_map(|(path, rule, _)| {
                        let excl: HashSet<PathBuf> = all_paths
                            .iter()
                            .filter(|o| *o != path && o.starts_with(path))
                            .cloned()
                            .collect();
                        let (s, children) = ctx.size_path(path, &excl)?;
                        item_count.fetch_add(1, Ordering::Relaxed);
                        Some(Raw {
                            path: path.clone(),
                            name: path_label(path, rule),
                            rule: rule.id.clone(),
                            sized: Some((s, children)),
                            disjoint: true,
                            ..Default::default()
                        })
                    })
                    .collect();
                let sized_providers: Vec<Raw> = provider_items
                    .into_par_iter()
                    .map(|mut r| {
                        if r.sized.is_none() && !r.is_virtual {
                            r.sized = ctx.size_path(&r.path, &HashSet::new());
                        }
                        r
                    })
                    .collect();
                globals.extend(sized_providers);
                globals
            },
        );

        // Worktrees outside every root (agents put them anywhere): walk them too.
        let walked: Vec<PathBuf> = roots.clone();
        let extra: Vec<PathBuf> = walker
            .known_worktrees
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.is_dir() && !walked.iter().any(|r| w.starts_with(r)))
            .cloned()
            .collect();
        if !extra.is_empty() {
            *phase.lock().unwrap() = "scanning worktrees".into();
            dedupe_roots(extra).par_iter().for_each(|r| walker.walk_root(r));
        }

        let (slow, warn) = slow_providers.join().unwrap_or_default();
        warnings.extend(warn);
        raws = walker.items.into_inner().unwrap();
        raws.extend(sized_globals);
        raws.extend(slow);
        found_projects = walker.projects.into_inner().unwrap();

        // Whole worktrees.
        *phase.lock().unwrap() = "checking worktrees".into();
        if let Some(rule) = rule_with_provider(cat, "git-worktrees").into_iter().next() {
            let wts = worktree_items(rule, &found_projects, &raws, &ctx);
            raws.extend(wts);
        }
        done.store(true, Ordering::Relaxed);
    });

    *phase.lock().unwrap() = "classifying".into();
    let report = build_report(cfg, cat, raws, found_projects, &procs_list, roots, warnings, started);
    report_progress(true);
    report
}

fn path_label(path: &Path, rule: &Rule) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if rule.each {
        name
    } else {
        tilde(path)
    }
}

fn worktree_items(rule: &Rule, projects: &[FoundProject], raws: &[Raw], ctx: &SizeCtx) -> Vec<Raw> {
    let repos: HashMap<PathBuf, &FoundProject> = projects
        .iter()
        .filter(|p| p.kind == "repo")
        .map(|p| (p.path.clone(), p))
        .collect();
    let defaults: Mutex<HashMap<PathBuf, Option<String>>> = Mutex::new(HashMap::new());
    projects
        .par_iter()
        .filter(|p| p.kind == "worktree" || p.kind == "orphan")
        .filter_map(|p| {
            let nested: Vec<&Raw> = raws.iter().filter(|r| !r.is_virtual && r.path.starts_with(&p.path)).collect();
            let exclude: HashSet<PathBuf> = nested.iter().map(|r| r.path.clone()).collect();
            let nested_bytes: u64 = nested.iter().map(|r| r.sized.as_ref().map_or(0, |s| s.0.bytes)).sum();
            let (s, children) = ctx.size_path(&p.path, &exclude)?;
            let pname = p.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let repo_name = p.repo.as_ref().and_then(|r| r.file_name()).map(|s| s.to_string_lossy().to_string());
            // ~/.codex/worktrees/<task>/<repo>: the task folder is the useful name.
            let label = if repo_name.as_deref() == Some(pname.as_str()) {
                let parent = p.path.parent().and_then(|x| x.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                format!("{}/{}", parent, pname)
            } else {
                pname.clone()
            };
            let mut raw = Raw {
                path: p.path.clone(),
                name: label,
                rule: rule.id.clone(),
                sized: Some((s, children)),
                last_used: p.last_activity,
                project: Some(p.path.clone()),
                repo: p.repo.clone(),
                agent: p.agent.clone(),
                nested_bytes,
                action: Some(ActionKind::WorktreeRemove),
                ..Default::default()
            };
            if let Some(b) = &p.branch {
                raw.meta.insert("branch".into(), Value::String(b.clone()));
            }
            if p.kind == "orphan" {
                raw.tier = Some(Tier::Review);
                raw.action = Some(ActionKind::Delete);
                raw.reasons.push("its repository is gone (broken .git link); check for work before deleting".into());
                return Some(raw);
            }
            let wt_info = p.repo.as_ref().and_then(|r| repos.get(r)).and_then(|repo| {
                repo.worktrees.iter().find(|w| w.path == p.path).cloned()
            });
            if wt_info.as_ref().map_or(false, |w| w.locked) {
                raw.tier = Some(Tier::Protected);
                raw.reasons.push("worktree is locked".into());
                return Some(raw);
            }
            match git::is_dirty(&p.path) {
                Some(true) => {
                    raw.tier = Some(Tier::Protected);
                    raw.reasons.push("has uncommitted or untracked changes".into());
                    raw.meta.insert("dirty".into(), Value::Bool(true));
                    return Some(raw);
                }
                None => {
                    raw.tier = Some(Tier::Review);
                    raw.reasons.push("could not read git status".into());
                    return Some(raw);
                }
                _ => {
                    raw.meta.insert("dirty".into(), Value::Bool(false));
                }
            }
            let repo = p.repo.clone()?;
            let default = {
                let mut d = defaults.lock().unwrap();
                d.entry(repo.clone()).or_insert_with(|| git::default_branch(&repo)).clone()
            };
            let head_ref = p.branch.clone().or_else(|| p.head.clone()).unwrap_or_else(|| "HEAD".into());
            let merged = default.as_ref().map_or(false, |d| git::is_merged(&repo, &head_ref, d));
            raw.meta.insert("merged".into(), Value::Bool(merged));
            if let Some(d) = &default {
                raw.meta.insert("default_branch".into(), Value::String(d.clone()));
            }
            if merged {
                raw.reasons.push(format!("clean, and already in {}", default.unwrap_or_default()));
            } else if p.branch.is_some() {
                raw.reasons.push("clean; its branch stays after removal, so no commits are lost".into());
            } else {
                // Detached commits not reachable from any ref would only live in the reflog.
                let contained = git::git(&repo, &["for-each-ref", "--contains", &head_ref, "--count=1", "refs/heads", "refs/remotes"], 10)
                    .map_or(false, |o| !o.trim().is_empty());
                if contained {
                    raw.reasons.push("clean, detached at a commit that a branch already has".into());
                } else {
                    raw.tier = Some(Tier::Review);
                    raw.reasons.push("detached commits are not on any branch".into());
                }
            }
            Some(raw)
        })
        .collect()
}

struct ProjInfo {
    last_activity: Option<i64>,
    policy: ProjectPolicy,
}

fn classify(raw: &Raw, rule: &Rule, cfg: &Config, proj: Option<&ProjInfo>, procs_list: &[Proc], extra_tools: &[String]) -> (Tier, Vec<String>) {
    let mut reasons = raw.reasons.clone();
    let cost = raw.cost.unwrap_or(rule.cost);
    if let Some(t) = raw.tier {
        return (t, reasons);
    }
    if let Some(t) = rule.tier {
        return (t, reasons);
    }
    if cost == Cost::Data {
        reasons.push("holds data, not a cache".into());
        return (Tier::Protected, reasons);
    }
    let policy = proj.map(|p| p.policy).unwrap_or(ProjectPolicy::Auto);
    if policy == ProjectPolicy::Protected {
        reasons.push("project is protected".into());
        return (Tier::Protected, reasons);
    }
    if let Some(app) = rule.running_apps.iter().find(|a| procs_list.iter().any(|p| &p.name == *a)) {
        reasons.push(format!("{} is running", app));
        return (Tier::Hot, reasons);
    }
    if !raw.is_virtual {
        let scope = raw.project.as_deref().unwrap_or(&raw.path);
        let builds = procs::builds_inside(procs_list, scope, extra_tools);
        if let Some(b) = builds.first() {
            reasons.push(format!("{} is running here", b.name));
            return (Tier::Hot, reasons);
        }
    }
    if cost == Cost::None {
        reasons.insert(0, "nothing to rebuild".into());
        return (Tier::Junk, reasons);
    }
    if raw.orphan {
        return (Tier::Cold, reasons);
    }
    if policy == ProjectPolicy::Stale {
        reasons.push("project marked stale".into());
        return (Tier::Cold, reasons);
    }
    let warm_days = if rule.warm_days > 0 { rule.warm_days } else { cfg.scan.warm_days } as i64;
    let item_t = raw.last_used.or(raw.sized.as_ref().map(|s| s.0.newest).filter(|t| *t > 0));
    let proj_t = proj.and_then(|p| p.last_activity);
    let activity = match (item_t, proj_t) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let t = now();
    if policy == ProjectPolicy::Active {
        reasons.push("project marked active".into());
        if activity.map_or(false, |a| t - a < 3600) {
            return (Tier::Hot, reasons);
        }
        return (Tier::Warm, reasons);
    }
    match activity {
        None => {
            reasons.push("no activity recorded".into());
            (Tier::Cold, reasons)
        }
        Some(a) => {
            let age = (t - a).max(0);
            let which = if proj_t.map_or(false, |p| Some(p) == activity && item_t.map_or(true, |i| i < p)) {
                "project"
            } else {
                "last used"
            };
            if age < 3600 {
                reasons.push(if which == "project" { "project touched in the last hour".into() } else { "used in the last hour".to_string() });
                (Tier::Hot, reasons)
            } else if age < warm_days * 86400 {
                reasons.push(format!("{} {}", which, crate::util::age(a)));
                (Tier::Warm, reasons)
            } else {
                reasons.push(format!("{} {} (warm window {}d)", which, crate::util::age(a), warm_days));
                (Tier::Cold, reasons)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    cfg: &Config,
    cat: &Catalog,
    raws: Vec<Raw>,
    found: Vec<FoundProject>,
    procs_list: &[Proc],
    roots: Vec<PathBuf>,
    warnings: Vec<String>,
    started: Instant,
) -> Report {
    // Projects by path, for activity lookups (longest prefix wins).
    let mut proj_paths: Vec<&FoundProject> = found.iter().collect();
    proj_paths.sort_by_key(|p| std::cmp::Reverse(p.path.as_os_str().len()));
    let find_project = |p: &Path| proj_paths.iter().find(|fp| p.starts_with(&fp.path)).copied();
    let extra_tools: Vec<String> = cat
        .guards
        .iter()
        .filter(|g| g.kind == "process")
        .flat_map(|g| g.names.clone())
        .collect();

    // Drop raws nested inside another non-worktree item (already counted there).
    let mut raws = raws;
    raws.sort_by(|a, b| a.path.cmp(&b.path));
    let mut kept: Vec<Raw> = Vec::with_capacity(raws.len());
    for r in raws {
        if !r.is_virtual {
            if let Some(last) = kept.iter().rev().find(|k| !k.is_virtual && k.action != Some(ActionKind::WorktreeRemove)) {
                if r.path == last.path || (!last.disjoint && r.path.starts_with(&last.path)) {
                    continue;
                }
            }
        }
        kept.push(r);
    }

    let mut items: Vec<Item> = vec![];
    for mut raw in kept {
        let Some(rule) = cat.rule(&raw.rule) else { continue };
        let (sized, children) = raw.sized.take().unwrap_or((Sized::default(), vec![]));
        if sized.bytes < cfg.scan.min_item_bytes.max(rule.min_bytes) && raw.action != Some(ActionKind::WorktreeRemove) {
            continue;
        }
        let fp = raw.project.as_deref().and_then(find_project);
        if raw.project.is_none() {
            if let Some(fp) = (!raw.is_virtual).then(|| find_project(&raw.path)).flatten() {
                raw.project = Some(fp.path.clone());
            }
        }
        if let Some(fp) = fp {
            if raw.repo.is_none() {
                raw.repo = fp.repo.clone().or_else(|| (fp.kind == "repo").then(|| fp.path.clone()));
            }
            if raw.agent.is_none() {
                raw.agent = fp.agent.clone();
            }
        }
        let proj_key = raw.project.as_ref().map(|p| p.to_string_lossy().to_string());
        let info = proj_key.as_ref().map(|k| ProjInfo {
            last_activity: fp.and_then(|f| f.last_activity),
            policy: cfg.policy_for(k),
        });
        if raw.last_used.is_none() && sized.newest > 0 {
            raw.last_used = Some(sized.newest);
        } else if let (Some(l), true) = (raw.last_used, sized.newest > 0) {
            raw.last_used = Some(l.max(sized.newest));
        }
        if rule.min_age_days > 0 {
            if let Some(l) = raw.last_used {
                if crate::util::days_since(l) < rule.min_age_days as i64 {
                    continue;
                }
            }
        }
        let (tier, reasons) = classify(&raw, rule, cfg, info.as_ref(), procs_list, &extra_tools);
        let cost = raw.cost.unwrap_or(rule.cost);
        let action_kind = raw.action.unwrap_or(rule.action);
        let command = raw.command.clone().or_else(|| rule.command.clone());
        let path_s = raw.path.to_string_lossy().to_string();
        if rule.contents {
            raw.meta.insert("contents".into(), Value::Bool(true));
        }
        items.push(Item {
            id: crate::util::short_hash(&format!("{}|{}", rule.id, path_s)),
            path: path_s,
            name: raw.name.clone(),
            rule: rule.id.clone(),
            bucket: rule.bucket.clone(),
            group: rule.group.clone(),
            bytes: sized.bytes,
            total_bytes: sized.bytes + raw.nested_bytes,
            files: sized.files,
            last_used: raw.last_used,
            tier,
            cost,
            reasons,
            rebuild: rule.rebuild.clone(),
            project: proj_key,
            repo: raw.repo.as_ref().map(|r| r.to_string_lossy().to_string()),
            agent: raw.agent.clone(),
            action: Action { kind: action_kind, command },
            children: children.into_iter().take(16).collect(),
            meta: raw.meta,
        });
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));

    // Buckets.
    let mut buckets: Vec<BucketSummary> = cat
        .buckets
        .iter()
        .map(|b| BucketSummary {
            id: b.id.clone(),
            name: b.name.clone(),
            order: b.order,
            color: b.color.clone(),
            symbol: b.symbol.clone(),
            description: b.description.clone(),
            hidden: b.hidden,
            bytes: 0,
            count: 0,
            tiers: BTreeMap::new(),
            groups: vec![],
        })
        .collect();
    for it in &items {
        if let Some(b) = buckets.iter_mut().find(|b| b.id == it.bucket) {
            b.bytes += it.bytes;
            b.count += 1;
            *b.tiers.entry(it.tier.as_str().to_string()).or_default() += it.bytes;
            match b.groups.iter_mut().find(|g| g.name == it.group) {
                Some(g) => {
                    g.bytes += it.bytes;
                    g.count += 1;
                }
                None => b.groups.push(GroupSummary { name: it.group.clone(), bytes: it.bytes, count: 1 }),
            }
        }
    }
    for b in &mut buckets {
        b.groups.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    }

    // Projects.
    let mut projects: Vec<Project> = found
        .iter()
        .map(|fp| {
            let key = fp.path.to_string_lossy().to_string();
            let mine: Vec<&Item> = items.iter().filter(|i| i.project.as_deref() == Some(key.as_str())).collect();
            let last = fp.last_activity;
            let status = match last.map(crate::util::days_since) {
                Some(d) if d < 14 => "active",
                Some(d) if d < 60 => "idle",
                _ => "stale",
            };
            let wt_item = mine.iter().find(|i| i.action.kind == ActionKind::WorktreeRemove);
            Project {
                path: key.clone(),
                name: fp.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                kind: fp.kind.to_string(),
                repo: fp.repo.as_ref().map(|r| r.to_string_lossy().to_string()),
                agent: fp.agent.clone(),
                branch: fp.branch.clone(),
                stacks: fp.stacks.clone(),
                last_activity: last,
                artifact_bytes: mine.iter().map(|i| i.bytes).sum(),
                item_count: mine.len(),
                policy: cfg.policy_for(&key),
                status: status.into(),
                dirty: wt_item.and_then(|i| i.meta.get("dirty")).and_then(|v| v.as_bool()),
                merged: wt_item.and_then(|i| i.meta.get("merged")).and_then(|v| v.as_bool()),
            }
        })
        .collect();
    projects.sort_by(|a, b| b.artifact_bytes.cmp(&a.artifact_bytes));

    let disk = crate::fsutil::disk_info(&home());
    let mut report = Report {
        schema: SCHEMA,
        version: env!("CARGO_PKG_VERSION").into(),
        scanned_at: now(),
        duration_ms: started.elapsed().as_millis() as u64,
        disk,
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        total_bytes: items.iter().map(|i| i.bytes).sum(),
        buckets,
        plans: vec![],
        projects,
        items,
        warnings,
    };
    report.plans = crate::plan::summaries(cat, &report);
    report
}

pub fn report_path() -> PathBuf {
    state_dir().join("report.json")
}

pub fn save_report(r: &Report) -> anyhow::Result<()> {
    let p = report_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(r)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

pub fn load_report() -> Option<Report> {
    let data = std::fs::read(report_path()).ok()?;
    let r: Report = serde_json::from_slice(&data).ok()?;
    (r.schema == SCHEMA).then_some(r)
}
