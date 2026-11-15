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
    /// Walk everything even when the index could update incrementally.
    pub full: bool,
    pub on_progress: Option<Box<dyn Fn(&Progress) + Send + Sync>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FoundProject {
    pub path: PathBuf,
    pub kind: String,
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
    /// Projects already in the index (incremental scans).
    known_projects: HashSet<PathBuf>,
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
            kind: "repo".into(),
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
                        proj.kind = "submodule".into();
                    } else {
                        proj.kind = "worktree".into();
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
                _ => proj.kind = "orphan".into(),
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
                    kind: "folder".into(),
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
                s.frees += e.alloc;
                self.items.lock().unwrap().push(Raw {
                    path: e.path.clone(),
                    name: self.label(&e.path, &proj),
                    rule: rule.id.clone(),
                    sized: Some((s, children)),
                    project: proj.as_ref().map(|p| p.path.clone()),
                    origin: "walk".into(),
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
                            sized: Some((Sized::of(b, 0, n), vec![])),
                            project: proj.as_ref().map(|p| p.path.clone()),
                            disjoint: true,
                            origin: "ignored".into(),
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
        if entry_names.iter().any(|n| n == ".git") {
            // Newest edit outside build output = when someone last worked here.
            if let Some(fp) = self.projects.lock().unwrap().iter_mut().rev().find(|p| p.path == dir) {
                fp.last_activity = Some(fp.last_activity.unwrap_or(0).max(newest));
            }
        }
        if is_worktree_root {
            // The worktree item accounts for its own bytes, and its edits are its own activity.
            return (0, 0);
        }
        (own, newest)
    }

    /// Incremental: look at one changed folder for new build folders or new checkouts,
    /// without descending into unchanged subtrees.
    fn visit_shallow(&self, dir: &Path, known: &HashSet<PathBuf>, proj: Option<Arc<ProjCtx>>) {
        let Some(dev) = crate::fsutil::dev_of(dir) else { return };
        let entries = read_entries(dir);
        let entry_names = names(&entries);
        if entry_names.iter().any(|n| n == ".git") && !self.known_projects.contains(dir) {
            self.visit(dir, &entries, 0, None, dev);
            return;
        }
        for e in &entries {
            if !e.is_dir || e.is_symlink || e.dev != dev || e.name == ".git" || self.excluded(&e.path) || known.contains(&e.path) {
                continue;
            }
            let child = read_entries(&e.path);
            let child_names = names(&child);
            if child_names.iter().any(|n| n == ".git") {
                if !self.known_projects.contains(&e.path) {
                    self.visit(&e.path, &child, 0, None, dev);
                }
                continue;
            }
            let is_ignored = proj.as_ref().map_or(false, |p| p.ignored.contains(&e.path));
            if let Some(rule) = self.rules.iter().find(|r| (!r.ignored || is_ignored) && r.matches_artifact(&e.name, &e.path, &entry_names, &child_names)) {
                let (mut s, children) = self.ctx.size_entries(&child, dev, &HashSet::new());
                s.bytes += e.alloc;
                s.frees += e.alloc;
                self.items.lock().unwrap().push(Raw {
                    path: e.path.clone(),
                    name: self.label(&e.path, &proj),
                    rule: rule.id.clone(),
                    sized: Some((s, children)),
                    project: proj.as_ref().map(|p| p.path.clone()),
                    origin: "walk".into(),
                    ..Default::default()
                });
            }
        }
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
    let rules: Vec<&Rule> = cat.enabled_rules().filter(|r| r.kind == RuleKind::Path).collect();
    // Rules expand in parallel; collecting keeps catalog order, which the dedupe below relies on.
    let per_rule: Vec<Vec<(PathBuf, &Rule, bool)>> = rules.par_iter().map(|rule| rule_candidates(rule)).collect();
    let found: Vec<(PathBuf, &Rule, bool)> = per_rule.into_iter().flatten().collect();
    dedupe_candidates(found)
}

fn rule_candidates(rule: &Rule) -> Vec<(PathBuf, &Rule, bool)> {
    let mut found: Vec<(PathBuf, &Rule, bool)> = vec![];
    {
        for p in &rule.paths {
            let pattern = expand(p);
            // The deepest fixed folder of the pattern must answer before poo reads below it.
            let fixed: PathBuf = pattern.components().take_while(|c| !has_glob(&c.as_os_str().to_string_lossy())).collect();
            let base = if has_glob(&pattern.to_string_lossy()) { fixed } else { fixed.parent().map(|p| p.to_path_buf()).unwrap_or(fixed) };
            let probe_at = base.ancestors().find(|a| a.is_dir()).map(|a| a.to_path_buf());
            if let Some(b) = probe_at {
                if b != home() && !crate::fsutil::answers(&b) {
                    continue;
                }
                if !has_glob(&pattern.to_string_lossy()) && pattern.is_dir() && !crate::fsutil::answers(&pattern) {
                    continue;
                }
            }
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
    found
}

fn dedupe_candidates<'a>(found: Vec<(PathBuf, &'a Rule, bool)>) -> Vec<(PathBuf, &'a Rule, bool)> {
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
    let ctx = SizeCtx::recording(HashMap::new());
    let phase = Mutex::new(String::from("starting"));
    let current = Mutex::new(String::new());
    let item_count = AtomicUsize::new(0);
    let mut warnings: Vec<String> = cat.errors.iter().map(|e| format!("mod error: {}", e)).collect();
    warnings.extend(access_warnings(cat));

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
    // Journal position before walking: changes during the walk replay next time.
    let event_id = crate::fsevents::current_id();

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
            provider_items.extend(cheap_providers(cat, devices_json.as_ref()));
        }
        if !opts.no_providers {
            provider_items.extend(app_leftover_items(cat));
        }
        // A provider item is more specific than a generic path rule for the same folder.
        let provider_paths: HashSet<PathBuf> = provider_items.iter().filter(|r| !r.is_virtual).map(|r| r.path.clone()).collect();
        let candidates: Vec<_> = candidates.into_iter().filter(|(p, _, _)| !provider_paths.contains(p)).collect();
        let mut claimed: HashSet<PathBuf> = candidates.iter().map(|(p, _, _)| p.clone()).collect();
        claimed.extend(provider_paths.iter().cloned());

        // Exclusions for root walks.
        let mut exclude: HashSet<PathBuf> = claimed.clone();
        exclude.insert(home().join("Library"));
        exclude.insert(home().join(".Trash"));
        // OrbStack mounts container and VM filesystems here: slow to walk, and their space is
        // already counted in its disk image.
        exclude.insert(home().join("OrbStack"));
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
            known_projects: HashSet::new(),
        };

        *phase.lock().unwrap() = "scanning".into();
        let dj = devices_json.clone();
        let no_providers = opts.no_providers;
        let slow_providers = scope.spawn(move || {
            let devices_json = dj;
            if no_providers {
                return (vec![], vec![]);
            }
            slow_providers(cat, devices_json.as_ref())
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
                            origin: "global".into(),
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
            let wts = worktree_items(rule, &found_projects, &raws, &ctx, cfg);
            raws.extend(wts);
        }
        // Save what this scan saw, so the next one only re-measures what changed.
        if opts.roots.is_none() && !opts.no_providers && !opts.roots_only {
            save_index(&Index {
                version: INDEX_VERSION,
                fingerprint: fingerprint(cfg, cat, &roots),
                event_id,
                full_at: now(),
                updated_at: now(),
                roots: roots.clone(),
                raws: raws.clone(),
                projects: found_projects.clone(),
                slow_at: now(),
                dirs: std::mem::take(&mut *ctx.big.lock().unwrap()),
            });
        }
        done.store(true, Ordering::Relaxed);
    });

    *phase.lock().unwrap() = "classifying".into();
    warnings.extend(blocked_warnings());
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

fn worktree_items(rule: &Rule, projects: &[FoundProject], raws: &[Raw], ctx: &SizeCtx, cfg: &Config) -> Vec<Raw> {
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
                origin: "worktree".into(),
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
            let Some(repo) = p.repo.clone() else {
                raw.tier = Some(Tier::Review);
                raw.reasons.push("could not find its repository".into());
                return Some(raw);
            };
            let default = {
                let mut d = defaults.lock().unwrap();
                d.entry(repo.clone()).or_insert_with(|| git::default_branch(&repo)).clone()
            };
            let head_ref = p.branch.clone().or_else(|| p.head.clone()).unwrap_or_else(|| "HEAD".into());
            let merged = default.as_ref().map_or(false, |d| git::is_merged(&repo, &head_ref, d));
            raw.meta.insert("merged".into(), Value::Bool(merged));
            if let Some(d) = &default {
                raw.meta.insert("default_branch".into(), Value::String(d.trim_start_matches("origin/").to_string()));
            }
            if wt_info.as_ref().map_or(false, |w| w.locked) {
                raw.tier = Some(Tier::Protected);
                raw.reasons.push("worktree is locked".into());
                raw.risks.push(Risk::new("locked", RiskLevel::Danger, "git worktree is locked; poo unlocks and removes it"));
            }
            let artifacts: Vec<PathBuf> = nested.iter().map(|r| r.path.clone()).collect();
            if cfg.backup.config_files {
                let n = git::small_ignored_files(&p.path, cfg.backup.max_file_bytes).len();
                raw.meta.insert("config_files".into(), Value::from(n));
            }
            raw.meta.insert(
                "artifacts".into(),
                Value::Array(artifacts.iter().map(|a| Value::String(a.to_string_lossy().to_string())).collect()),
            );
            match git::dirty_state(&p.path, &artifacts) {
                Some(d) if d.is_dirty() => {
                    raw.tier.get_or_insert(Tier::Review);
                    raw.reasons.push(format!("uncommitted work: {}", d.summary()));
                    raw.meta.insert("dirty".into(), Value::Bool(true));
                    raw.risks.push(Risk::new("dirty", RiskLevel::Force, format!("{}; poo stashes it (git stash -u) before removing, so it stays recoverable from `git stash list`", d.summary())));
                }
                None => {
                    raw.tier = Some(Tier::Review);
                    raw.reasons.push("could not read git status".into());
                    raw.risks.push(Risk::new("unknown-status", RiskLevel::Danger, "git status failed for this worktree"));
                }
                _ => {
                    raw.meta.insert("dirty".into(), Value::Bool(false));
                }
            }
            if merged {
                raw.reasons.push(format!("its commits are already in {}", default.clone().unwrap_or_default().trim_start_matches("origin/")));
            } else if p.branch.is_some() {
                raw.reasons.push("its branch stays after removal, so no commits are lost".into());
                raw.risks.push(Risk::new("unmerged", RiskLevel::Standard, "branch not merged yet; it is kept"));
            } else if git::reachable(&repo, &head_ref) {
                raw.reasons.push("detached at a commit a branch already has".into());
            } else {
                raw.tier = Some(Tier::Review);
                raw.reasons.push("detached commits are not on any branch".into());
                raw.risks.push(Risk::new("unreachable-commits", RiskLevel::Danger, "detached commits on no branch would only survive in the reflog; poo tags them poo/<name> first"));
            }
            Some(raw)
        })
        .collect()
}

/// What poo knows about the checkout an item belongs to.
#[derive(Default)]
struct CheckoutInfo {
    last_activity: Option<i64>,
    policy: ProjectPolicy,
    /// 0 = most recently used checkout of its repo.
    rank: Option<usize>,
    siblings: usize,
    repo_name: String,
    /// Default branch the checkout's commits landed in, and whether leftover edits remain.
    landed: Option<(String, bool)>,
    presence: Vec<String>,
    builds: Vec<String>,
}

impl Default for ProjectPolicy {
    fn default() -> Self {
        ProjectPolicy::Auto
    }
}

struct Verdict {
    tier: Tier,
    reasons: Vec<String>,
    risks: Vec<Risk>,
}

fn tier_risks(tier: Tier, risks: &mut Vec<Risk>) {
    let has = |risks: &Vec<Risk>, lvl: RiskLevel| risks.iter().any(|r| r.level >= lvl);
    match tier {
        Tier::Hot if !has(risks, RiskLevel::Force) => risks.push(Risk::new("in-use", RiskLevel::Force, "in use right now")),
        Tier::Review if !has(risks, RiskLevel::Force) => risks.push(Risk::new("review", RiskLevel::Force, "needs a human look first")),
        Tier::Protected if !has(risks, RiskLevel::Danger) => risks.push(Risk::new("protected", RiskLevel::Danger, "kept; only --dangerously-include-protected cleans it")),
        _ => {}
    }
}

fn classify(raw: &Raw, rule: &Rule, cfg: &Config, info: Option<&CheckoutInfo>, procs_list: &[Proc], extra_tools: &[String]) -> Verdict {
    let mut v = Verdict { tier: Tier::Warm, reasons: raw.reasons.clone(), risks: raw.risks.clone() };
    let cost = raw.cost.unwrap_or(rule.cost);
    let finish = |mut v: Verdict, t: Tier| {
        v.tier = t;
        tier_risks(t, &mut v.risks);
        v
    };
    // Running apps and builds make anything hot, even provider-tiered items.
    if let Some(app) = rule.running_apps.iter().find(|a| procs_list.iter().any(|p| &p.name == *a)) {
        v.reasons.push(format!("{} is running", app));
        v.risks.push(Risk::new("app-running", RiskLevel::Danger, format!("{} is open and may rewrite or crash on a missing cache", app)));
        return finish(v, Tier::Hot);
    }
    let builds: Vec<String> = match info {
        Some(i) => i.builds.clone(),
        None if !raw.is_virtual => procs::builds_inside(procs_list, &raw.path, extra_tools).iter().map(|p| p.name.clone()).collect(),
        None => vec![],
    };
    if let Some(b) = builds.first() {
        v.reasons.push(format!("{} is building here right now", b));
        v.risks.push(Risk::new("build-running", RiskLevel::Danger, format!("{} is running in this checkout; cleaning now can break that build", b)));
        return finish(v, Tier::Hot);
    }
    if let Some(t) = raw.tier.or(rule.tier) {
        return finish(v, t);
    }
    if cost == Cost::Data {
        v.reasons.push("holds data, not a cache".into());
        v.risks.push(Risk::new("data", RiskLevel::Danger, "real data that poo cannot recreate"));
        return finish(v, Tier::Protected);
    }
    let policy = info.map(|i| i.policy).unwrap_or(ProjectPolicy::Auto);
    if policy == ProjectPolicy::Protected {
        v.reasons.push("project is protected".into());
        v.risks.push(Risk::new("protected-project", RiskLevel::Danger, "you marked this project protected"));
        return finish(v, Tier::Protected);
    }
    if cost == Cost::None {
        v.reasons.insert(0, "nothing to rebuild".into());
        return finish(v, Tier::Junk);
    }
    if raw.orphan {
        return finish(v, Tier::Cold);
    }
    if policy == ProjectPolicy::Stale {
        v.reasons.push("project marked stale".into());
        return finish(v, Tier::Cold);
    }
    let t = now();
    let item_t = raw.last_used.or(raw.sized.as_ref().map(|s| s.0.newest).filter(|t| *t > 0));
    let item_age = item_t.map(|i| (t - i).max(0));
    let presence: Vec<String> = info.map(|i| i.presence.clone()).unwrap_or_default();
    if item_age.map_or(false, |a| a < 3600) {
        v.reasons.push("used in the last hour".into());
        v.risks.push(Risk::new("recent", RiskLevel::Force, "used in the last hour"));
        return finish(v, Tier::Hot);
    }
    if !presence.is_empty() {
        v.reasons.push(format!("{} working in this checkout", presence.join(", ")));
        v.risks.push(Risk::new("agent-active", RiskLevel::Force, format!("{} is open here; cleaning costs it a rebuild", presence.join(", "))));
    }
    if policy == ProjectPolicy::Active {
        v.reasons.push("project marked active".into());
        return finish(v, Tier::Warm);
    }
    let idle_hours = cfg.scan.idle_hours as i64 * 3600;
    if let (Some(i), Some(age)) = (info, item_age) {
        if let Some((branch, dirty)) = &i.landed {
            if presence.is_empty() && age >= cfg.scan.landed_idle_hours as i64 * 3600 {
                if *dirty {
                    v.reasons.push(format!("this worktree's commits landed in {}; only leftover edits remain; built {}", branch, crate::util::age(item_t.unwrap())));
                    return finish(v, Tier::Idle);
                }
                v.reasons.push(format!("this worktree's commits already landed in {}; built {}", branch, crate::util::age(item_t.unwrap())));
                return finish(v, Tier::Cold);
            }
        }
    }
    let warm_days = if rule.warm_days > 0 { rule.warm_days } else { cfg.scan.warm_days } as i64;
    let idle_days = if rule.idle_days > 0 { rule.idle_days } else { cfg.scan.idle_days } as i64;
    let proj_t = info.and_then(|i| i.last_activity);
    let activity = match (item_t, proj_t) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let Some(activity) = activity else {
        v.reasons.push("no activity recorded".into());
        return finish(v, Tier::Cold);
    };
    let age = (t - activity).max(0);
    let which = if proj_t == Some(activity) && item_t.map_or(true, |i| i < activity) { "checkout used" } else { "last used" };
    if age >= warm_days * 86400 && presence.is_empty() {
        v.reasons.push(format!("{} {} (cold after {}d)", which, crate::util::age(activity), warm_days));
        return finish(v, Tier::Cold);
    }
    if !presence.is_empty() {
        return finish(v, Tier::Warm);
    }
    // Other checkouts of a busy repo: keep the most recent ones warm, the rest go idle.
    if let (Some(i), Some(age)) = (info, item_age) {
        if let Some(rank) = i.rank {
            if i.siblings > 1 && rank >= cfg.scan.keep_recent_checkouts && age >= idle_hours {
                v.reasons.push(format!(
                    "{} other checkouts of {} were used more recently; this one {}",
                    rank,
                    i.repo_name,
                    crate::util::age(item_t.unwrap())
                ));
                return finish(v, Tier::Idle);
            }
        }
    }
    if age >= idle_days * 86400 {
        v.reasons.push(format!("{} {}, nobody working there", which, crate::util::age(activity)));
        return finish(v, Tier::Idle);
    }
    v.reasons.push(format!("{} {}", which, crate::util::age(activity)));
    finish(v, Tier::Warm)
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

    // Normalize each item's checkout to the project root that holds it.
    for raw in kept.iter_mut() {
        let base = raw.project.clone().or_else(|| (!raw.is_virtual).then(|| raw.path.clone()));
        if let Some(fp) = base.as_deref().and_then(find_project) {
            raw.project = Some(fp.path.clone());
        }
    }
    // Checkout activity = git activity or the newest build inside it.
    let mut activity: HashMap<PathBuf, i64> = HashMap::new();
    for fp in &found {
        if let Some(t) = fp.last_activity {
            activity.insert(fp.path.clone(), t);
        }
    }
    for raw in &kept {
        if let Some(p) = &raw.project {
            let t = raw.last_used.unwrap_or(0).max(raw.sized.as_ref().map_or(0, |s| s.0.newest));
            let e = activity.entry(p.clone()).or_insert(0);
            *e = (*e).max(t);
        }
    }
    // Rank checkouts of the same repo by recency.
    let mut by_repo: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for fp in found.iter().filter(|f| f.kind == "repo" || f.kind == "worktree") {
        let key = fp.repo.clone().unwrap_or_else(|| fp.path.clone());
        by_repo.entry(key).or_default().push(fp.path.clone());
    }
    let mut rank: HashMap<PathBuf, (usize, usize, String)> = HashMap::new();
    for (repo, mut list) in by_repo {
        list.sort_by_key(|p| std::cmp::Reverse(activity.get(p).copied().unwrap_or(0)));
        let n = list.len();
        let name = repo.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        for (i, p) in list.into_iter().enumerate() {
            rank.insert(p, (i, n, name.clone()));
        }
    }
    // Worktrees whose commits already landed (flag true when leftover edits remain).
    let landed: HashMap<PathBuf, (String, bool)> = kept
        .iter()
        .filter(|r| r.action == Some(ActionKind::WorktreeRemove) && r.meta.get("merged").and_then(|v| v.as_bool()) == Some(true))
        .map(|r| {
            let branch = r.meta.get("default_branch").and_then(|v| v.as_str()).unwrap_or("main").to_string();
            let dirty = r.meta.get("dirty").and_then(|v| v.as_bool()).unwrap_or(true);
            (r.path.clone(), (branch, dirty))
        })
        .collect();
    // Attribute each process to the innermost checkout it runs in.
    let mut presence: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut builds: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for p in procs_list {
        if let Some(fp) = find_project(&p.cwd) {
            if procs::is_build_tool(&p.name, &extra_tools) {
                builds.entry(fp.path.clone()).or_default().push(p.name.clone());
            } else if procs::is_presence(&p.name) {
                let e = presence.entry(fp.path.clone()).or_default();
                if !e.contains(&p.name) {
                    e.push(p.name.clone());
                }
            }
        }
    }

    let mut items: Vec<Item> = vec![];
    for mut raw in kept {
        let Some(rule) = cat.rule(&raw.rule) else { continue };
        let (sized, children) = raw.sized.take().unwrap_or((Sized::default(), vec![]));
        let size_unknown = raw.meta.get("size_unknown").and_then(|v| v.as_bool()) == Some(true);
        if sized.bytes < cfg.scan.min_item_bytes.max(rule.min_bytes) && raw.action != Some(ActionKind::WorktreeRemove) && !size_unknown {
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
        let info = raw.project.as_ref().map(|p| {
            let key = p.to_string_lossy().to_string();
            let r = rank.get(p);
            CheckoutInfo {
                last_activity: fp.and_then(|f| f.last_activity),
                policy: cfg.policy_for(&key),
                rank: r.map(|x| x.0),
                siblings: r.map_or(1, |x| x.1),
                repo_name: r.map(|x| x.2.clone()).unwrap_or_default(),
                landed: landed.get(p).cloned(),
                presence: presence.get(p).cloned().unwrap_or_default(),
                builds: builds.get(p).cloned().unwrap_or_default(),
            }
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
        let verdict = classify(&raw, rule, cfg, info.as_ref(), procs_list, &extra_tools);
        let (tier, reasons, mut risks) = (verdict.tier, verdict.reasons, verdict.risks);
        // Removing a whole checkout is more disruptive than deleting its build output:
        // only cold (landed or long unused) worktrees go without --force.
        if raw.action == Some(ActionKind::WorktreeRemove) && !matches!(tier, Tier::Junk | Tier::Cold) {
            risks.push(Risk::new(
                "whole-checkout",
                RiskLevel::Force,
                "removes the whole checkout (its branch stays); agents or editors using this folder lose it",
            ));
        }
        let cost = raw.cost.unwrap_or(rule.cost);
        let mut action_kind = raw.action.unwrap_or(rule.action);
        let mut command = raw.command.clone().or_else(|| rule.command.clone());
        // Owner-command cleaning: the tool's own command instead of deleting the folder.
        if !raw.is_virtual && matches!(action_kind, ActionKind::Delete | ActionKind::Trash) {
            if let Some(cmd) = rule.owner_command() {
                action_kind = ActionKind::Command;
                command = Some(cmd);
                raw.meta.insert("owner_cleanup".into(), Value::Bool(true));
            }
        }
        // A command that cleans a folder is judged by re-measuring that folder afterwards.
        if !raw.is_virtual && action_kind == ActionKind::Command {
            raw.meta.insert("measure".into(), Value::from("path"));
        }
        let report_only = rule.report_only || raw.report_only;
        let recommend = raw.meta.remove("recommend").and_then(|v| v.as_str().map(String::from)).or_else(|| rule.recommend.clone());
        let path_s = raw.path.to_string_lossy().to_string();
        let safeguards = if report_only {
            vec![match &recommend {
                Some(r) => format!("poo shows it but never cleans it: {}", r),
                None => "poo shows it but never cleans it".to_string(),
            }]
        } else {
            safeguards_for(cfg, rule, action_kind, command.as_deref(), &raw, &risks)
        };
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
            tags: rule.tags.clone(),
            bytes: sized.bytes,
            total_bytes: sized.bytes + raw.nested_bytes,
            // Sizes a tool reported (no files counted) free what they use.
            frees: if sized.files == 0 { sized.bytes } else { sized.frees.min(sized.bytes) },
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
            risks,
            safeguards,
            children: children.into_iter().take(16).collect(),
            meta: raw.meta,
            report_only,
            recommend,
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
            report_only: b.report_only,
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
                kind: fp.kind.clone(),
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
                recency_rank: rank.get(&fp.path).map(|r| r.0),
                active: presence.get(&fp.path).cloned().unwrap_or_default(),
            }
        })
        .collect();
    projects.sort_by(|a, b| b.artifact_bytes.cmp(&a.artifact_bytes));

    let mut detected: Vec<String> = found.iter().flat_map(|p| p.stacks.clone()).collect();
    for (tool, stack) in [
        ("xcrun", "xcode"), ("docker", "docker"), ("brew", "homebrew"), ("cargo", "rust"), ("node", "node"),
        ("bun", "bun"), ("python3", "python"), ("ruby", "ruby"), ("go", "go"), ("java", "jvm"),
        ("flutter", "flutter"), ("dotnet", "dotnet"), ("claude", "claude"), ("codex", "codex"), ("ollama", "ollama"),
    ] {
        // macOS ships stubs in /usr/bin (ruby, java, python3) that say nothing about the user.
        if crate::util::which(tool).map_or(false, |p| !p.starts_with("/usr/bin") && !p.starts_with("/bin")) {
            detected.push(stack.to_string());
        }
    }
    if std::path::Path::new("/Applications/Xcode.app").exists() || home().join("Library/Developer/Xcode").exists() {
        detected.push("xcode".into());
    }
    if home().join("Library/Android/sdk").exists() || home().join(".android").exists() {
        detected.push("android".into());
    }
    detected.sort();
    detected.dedup();
    let mut rule_hits: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for it in &items {
        let e = rule_hits.entry(it.rule.clone()).or_default();
        e.0 += 1;
        e.1 += it.bytes;
    }
    let disk = crate::fsutil::disk_info(&home());
    let items_report_only_bytes: u64 = items.iter().filter(|i| i.report_only).map(|i| i.bytes).sum();
    let mut report = Report {
        schema: SCHEMA,
        version: env!("CARGO_PKG_VERSION").into(),
        scanned_at: now(),
        duration_ms: started.elapsed().as_millis() as u64,
        disk,
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        total_bytes: items.iter().filter(|i| !i.report_only).map(|i| i.bytes).sum(),
        buckets,
        plans: vec![],
        projects,
        items,
        warnings,
        detected,
        rule_hits,
        report_only_bytes: items_report_only_bytes,
        growth: None,
    };
    report.plans = crate::plan::summaries(cat, &report);
    report
}

/// Plain sentences about what poo does before and while cleaning an item.
fn safeguards_for(cfg: &Config, rule: &Rule, action: ActionKind, command: Option<&[String]>, raw: &Raw, risks: &[Risk]) -> Vec<String> {
    let b = &cfg.backup;
    let has = |t: &str| risks.iter().any(|r| r.tag == t);
    let mut v = vec![];
    match action {
        ActionKind::WorktreeRemove => {
            let n = raw.meta.get("config_files").and_then(|x| x.as_u64()).unwrap_or(0);
            if b.config_files && n > 0 {
                v.push(format!("Backs up {} ignored config file{} (.env, Local.xcconfig...) to Backups first", n, if n == 1 { "" } else { "s" }));
            }
            if has("dirty") {
                v.push(if b.stash_dirty {
                    "Stashes uncommitted work first; restore it with git stash apply".to_string()
                } else {
                    "Discards uncommitted work (backup.stash_dirty is off)".to_string()
                });
            }
            if has("unreachable-commits") {
                v.push(if b.rescue_branches {
                    "Saves detached commits to a poo/ branch first".to_string()
                } else {
                    "Leaves detached commits to the reflog (backup.rescue_branches is off)".to_string()
                });
            }
            if raw.meta.get("merged").and_then(|x| x.as_bool()) == Some(true) {
                v.push("Its commits are already in the main branch".into());
            } else if raw.meta.contains_key("branch") {
                v.push("Its branch and commits stay; only the checkout folder goes".into());
            }
            v.push("Removed with git worktree remove; recreate with git worktree add".into());
        }
        ActionKind::Delete | ActionKind::Trash if b.use_trash || action == ActionKind::Trash => {
            v.push("Moves to the Trash".into());
        }
        ActionKind::Delete => {
            let contents = rule.contents;
            let what = if contents { "Empties the folder (the folder itself stays)" } else { "Deleted permanently" };
            match (&rule.rebuild, raw.cost.unwrap_or(rule.cost)) {
                (_, Cost::None) => v.push(format!("{}; nothing to rebuild", what)),
                (Some(r), _) => v.push(format!("{}; comes back with: {}", what, r)),
                (None, Cost::Redownload) => v.push(format!("{}; downloaded again when needed", what)),
                (None, _) => v.push(format!("{}; regenerated when needed", what)),
            }
        }
        ActionKind::Command => {
            if let Some(c) = command {
                let mut shown = c.to_vec();
                if let Some(first) = shown.first_mut() {
                    *first = Path::new(first.as_str()).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                }
                if raw.meta.get("owner_cleanup").and_then(|m| m.as_bool()) == Some(true) {
                    v.push(format!("Runs `{}`, {}'s own cleanup, instead of deleting the folder; frees what it no longer needs", shown.join(" "), shown[0]));
                } else {
                    v.push(format!("Runs `{}`", shown.join(" ")));
                }
                if let Some(p) = &rule.probe {
                    v.push(format!("Only when `{}` answers; otherwise it is skipped", p.join(" ")));
                }
            }
        }
        ActionKind::None => v.push("Poo shows it but leaves cleaning to you".into()),
        _ => {}
    }
    v
}

pub fn report_path() -> PathBuf {
    state_dir().join("report.json")
}

pub fn save_report(r: &Report) -> anyhow::Result<()> {
    let p = report_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    // Growth history: one sample per save (thinned), and the summary rides along in the file.
    crate::growth::record(r);
    let mut with_growth;
    let r = match crate::growth::summary(r) {
        Some(g) => {
            with_growth = r.clone();
            with_growth.growth = Some(g);
            &with_growth
        }
        None => r,
    };
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

// ---------------------------------------------------------------------------
// Providers, split by cost so incremental scans can reuse the slow ones.
// ---------------------------------------------------------------------------

fn cheap_providers(cat: &Catalog, devices_json: Option<&Value>) -> Vec<Raw> {
    let mut out = cheap_providers_except_simulators(cat);
    for r in rule_with_provider(cat, "simulator-devices") {
        out.extend(providers::simulator_devices(r, devices_json));
    }
    for r in out.iter_mut() {
        r.origin = "provider".into();
    }
    out
}

fn cheap_providers_except_simulators(cat: &Catalog) -> Vec<Raw> {
    let mut out = vec![];
    for r in rule_with_provider(cat, "xcode-derived-data") {
        out.extend(providers::xcode_derived_data(r));
    }
    for r in rule_with_provider(cat, "versions") {
        out.extend(providers::versions(r));
    }
    for r in out.iter_mut() {
        r.origin = "provider".into();
    }
    out
}

/// Leftovers of removed apps. Slow to list, but they claim their paths before path rules do.
fn app_leftover_items(cat: &Catalog) -> Vec<Raw> {
    let mut out = vec![];
    let leftovers = rule_with_provider(cat, "app-leftovers");
    if !leftovers.is_empty() {
        let installed = providers::installed_bundle_ids();
        for r in leftovers {
            out.extend(providers::app_leftovers(r, &installed));
        }
    }
    for r in out.iter_mut() {
        r.origin = "slow".into();
    }
    out
}

/// Docker, Homebrew, simulator runtimes and exec rules: shell out, so they are reused for a while.
fn slow_providers(cat: &Catalog, devices_json: Option<&Value>) -> (Vec<Raw>, Vec<String>) {
    let mut out: Vec<Raw> = vec![];
    let mut warn = vec![];
    for r in rule_with_provider(cat, "simulator-runtimes") {
        out.extend(providers::simulator_runtimes(r, devices_json));
    }
    for r in rule_with_provider(cat, "docker") {
        out.extend(providers::docker(r));
    }
    for r in rule_with_provider(cat, "homebrew") {
        out.extend(providers::homebrew(r));
    }
    for r in rule_with_provider(cat, "snapshots") {
        out.extend(providers::snapshots::snapshots(r));
    }
    for r in rule_with_provider(cat, "exec") {
        match providers::exec(r) {
            Ok(items) => out.extend(items),
            Err(e) => warn.push(format!("rule `{}`: {}", r.id, e)),
        }
    }
    for r in out.iter_mut() {
        r.origin = "slow".into();
    }
    (out, warn)
}

// ---------------------------------------------------------------------------
// Scan index and incremental rescans
// ---------------------------------------------------------------------------

const INDEX_VERSION: u32 = 3;
/// Slow providers (Docker, Homebrew, app leftovers, runtimes) are reused this long.
const SLOW_TTL: i64 = 15 * 60;
/// A full walk at least this often catches anything the journal cannot tell.
const FULL_EVERY: i64 = 24 * 3600;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Index {
    pub version: u32,
    pub fingerprint: String,
    pub event_id: u64,
    pub full_at: i64,
    pub updated_at: i64,
    pub roots: Vec<PathBuf>,
    pub raws: Vec<Raw>,
    pub projects: Vec<FoundProject>,
    pub slow_at: i64,
    /// Sizes of big folders, so rescans re-measure only changed branches.
    #[serde(default)]
    pub dirs: Vec<(PathBuf, Sized)>,
}

fn index_path() -> PathBuf {
    state_dir().join("index.json")
}

pub fn save_index(idx: &Index) {
    let p = index_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension(format!("json.tmp-{}", std::process::id()));
    if let Ok(data) = serde_json::to_vec(idx) {
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
}

pub fn load_index() -> Option<Index> {
    serde_json::from_slice(&std::fs::read(index_path()).ok()?).ok()
}

/// Drops cleaned paths from the index so the next incremental scan does not resurrect them.
pub fn forget_in_index(paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    if let Some(mut idx) = load_index() {
        let gone = |r: &Raw| {
            let p = r.path.to_string_lossy();
            paths.iter().any(|g| p == g.as_str() || p.starts_with(&format!("{}/", g)))
        };
        if idx.raws.iter().any(|r| r.origin == "slow" && gone(r)) {
            idx.slow_at = 0;
        }
        idx.raws.retain(|r| !gone(r));
        save_index(&idx);
    }
}

/// Changes to rules, roots or scan settings invalidate the index.
fn fingerprint(cfg: &Config, cat: &Catalog, roots: &[PathBuf]) -> String {
    let rules: Vec<(&str, bool, &RuleSpec)> = cat.rules.iter().map(|r| (r.id.as_str(), r.enabled, &r.spec)).collect();
    let data = serde_json::json!({
        "v": crate::VERSION,
        "rules": rules,
        "roots": roots,
        "homes": cat.worktree_homes,
        "depth": cfg.scan.max_depth,
        "exclude": cfg.scan.exclude,
        "backup": cfg.backup.config_files,
    });
    crate::util::short_hash(&data.to_string())
}

fn access_warnings(cat: &Catalog) -> Vec<String> {
    let gated = cat.rules.iter().filter(|r| r.needs_permission && !r.enabled).count();
    if gated > 0 && !crate::util::has_full_disk_access() {
        return vec![format!(
            "{} rules (browser profiles, Mail, Messages, app containers) are off until poo has Full Disk Access: System Settings > Privacy & Security > Full Disk Access",
            gated
        )];
    }
    vec![]
}

/// Where a default scan walks: configured (or detected) roots, mod roots and agent worktree
/// homes, canonical and without nesting. The bool says the roots were detected.
pub fn scan_roots(cfg: &Config, cat: &Catalog) -> (Vec<PathBuf>, Vec<(PathBuf, String)>, bool) {
    let (mut roots, detected) = cfg.effective_roots();
    roots.extend(cat.roots.iter().map(|r| expand(r)).filter(|p| p.is_dir()));
    let homes: Vec<(PathBuf, String)> = cat
        .worktree_homes
        .iter()
        .map(|h| (expand(&h.path), h.agent.clone()))
        .filter(|(p, _)| p.is_dir())
        .collect();
    roots.extend(homes.iter().map(|(p, _)| p.clone()));
    let roots = dedupe_roots(roots.into_iter().filter_map(|r| std::fs::canonicalize(&r).ok()).collect());
    (roots, homes, detected)
}

/// Folders the file-change journal is read for.
pub fn watch_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut w = vec![home()];
    if let Ok(t) = std::env::var("TMPDIR") {
        if let Ok(c) = std::fs::canonicalize(t) {
            w.push(c);
        }
    }
    for r in roots {
        if !r.starts_with(home()) {
            w.push(r.clone());
        }
    }
    w
}

/// FSEvents reports /private/var/...; poo may know the same folder as /var/...
pub fn path_forms(p: &Path) -> Vec<PathBuf> {
    let s = p.to_string_lossy();
    let mut v = vec![p.to_path_buf()];
    for (a, b) in [("/private/var/", "/var/"), ("/private/tmp/", "/tmp/")] {
        if let Some(rest) = s.strip_prefix(a) {
            v.push(PathBuf::from(format!("{}{}", b, rest)));
        }
    }
    v
}

pub fn nearest_known(p: &Path, known: &HashSet<PathBuf>) -> Option<PathBuf> {
    for form in path_forms(p) {
        for a in form.ancestors() {
            if known.contains(a) {
                return Some(a.to_path_buf());
            }
        }
    }
    None
}

/// Rescans using the index and the FSEvents journal. None when a full scan is needed.
pub fn scan_incremental(cfg: &Config, cat: &Catalog, opts: &ScanOptions) -> Option<Report> {
    if opts.roots.is_some() || opts.no_providers || opts.roots_only {
        return None;
    }
    let started = Instant::now();
    let idx = load_index()?;
    if idx.version != INDEX_VERSION || now() - idx.full_at > FULL_EVERY {
        return None;
    }
    // Same roots and rules as when the index was built.
    let (roots, homes, detected) = scan_roots(cfg, cat);
    if fingerprint(cfg, cat, &roots) != idx.fingerprint {
        return None;
    }
    trace(&started, "load");
    let new_id = crate::fsevents::current_id();
    let changes = crate::fsevents::changes_since(&watch_roots(&roots), idx.event_id, std::time::Duration::from_secs(20))?;
    if changes.lost {
        return None;
    }
    trace(&started, "replay");
    let state = state_dir();
    let changed: Vec<PathBuf> = changes.paths.into_iter().filter(|p| !p.starts_with(&state)).collect();
    if changes.rescan_dirs.iter().any(|d| roots.iter().any(|r| d.starts_with(r) || r.starts_with(d))) {
        return None;
    }
    if let Some(cb) = &opts.on_progress {
        cb(&Progress { phase: "updating".into(), detail: format!("{} changed folders", changed.len()), files: 0, bytes: 0, items: idx.raws.len(), elapsed_ms: started.elapsed().as_millis() as u64 });
    }

    let ctx = SizeCtx::recording(idx.dirs.iter().cloned().collect());
    let old_dirs = idx.dirs;
    let mut raws = idx.raws;
    let mut projects = idx.projects;
    // Gone items drop out. A removed folder shows up as a change in one of its ancestors,
    // so only items below a changed folder need a look.
    let changed_set: HashSet<PathBuf> = changed.iter().flat_map(|c| path_forms(c)).collect();
    let maybe_gone: Vec<bool> = raws
        .par_iter()
        .map(|r| !r.is_virtual && r.path.ancestors().any(|a| changed_set.contains(a)) && std::fs::symlink_metadata(&r.path).is_err())
        .collect();
    let mut gone_iter = maybe_gone.into_iter();
    raws.retain(|_| !gone_iter.next().unwrap_or(false));
    trace(&started, "retain gone");
    let known: HashSet<PathBuf> = raws.iter().filter(|r| !r.is_virtual).map(|r| r.path.clone()).collect();
    let project_paths: HashSet<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();

    // Folders the journal could not itemize (outside dev roots): measure what is inside afresh.
    let rescan: Vec<PathBuf> = changes.rescan_dirs.iter().flat_map(|d| path_forms(d)).collect();
    let fresh: HashSet<PathBuf> = known.iter().filter(|k| rescan.iter().any(|d| k.starts_with(d) || d.starts_with(k))).cloned().collect();
    // Changes inside an item make it dirty; others are structural.
    let mut dirty: HashSet<PathBuf> = fresh.clone();
    let mut outside: Vec<PathBuf> = vec![];
    for c in &changed {
        match nearest_known(c, &known) {
            Some(k) => {
                dirty.insert(k);
            }
            None => outside.push(c.clone()),
        }
    }

    if std::env::var_os("POO_TRACE").is_some() {
        for d in &dirty {
            let b = raws.iter().find(|r| &r.path == d).and_then(|r| r.sized.as_ref()).map_or(0, |s| s.0.bytes);
            eprintln!("dirty {} {}", crate::util::bytes(b), d.display());
        }
    }
    // Re-measure dirty items, excluding other items nested inside them.
    let descendants = |p: &Path| -> HashSet<PathBuf> { known.iter().filter(|o| o.as_path() != p && o.starts_with(p)).cloned().collect() };
    trace(&started, "dirty sets");
    let mut changed_in: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for c in &changed {
        if let Some(k) = nearest_known(c, &known) {
            let form = path_forms(c).into_iter().find(|f| f.starts_with(&k)).unwrap_or_else(|| c.clone());
            changed_in.entry(k).or_default().push(form);
        }
    }
    raws.par_iter_mut().for_each(|r| {
        if r.is_virtual || !dirty.contains(&r.path) || r.origin == "worktree" {
            return;
        }
        let below: Vec<&Path> = changed_in.get(&r.path).map(|v| v.iter().map(|p| p.as_path()).collect()).unwrap_or_default();
        let t = Instant::now();
        let sized = if fresh.contains(&r.path) {
            ctx.size_path(&r.path, &descendants(&r.path))
        } else {
            ctx.size_path_delta(&r.path, &descendants(&r.path), &below)
        };
        if std::env::var_os("POO_TRACE").is_some() {
            eprintln!("resized {:>5} ms  {} changed  {}", t.elapsed().as_millis(), below.len(), r.path.display());
        }
        if let Some(s) = sized {
            r.sized = Some(s);
            r.last_used = None;
        }
    });

    trace(&started, "resize");
    // Structural changes in dev roots: new build folders, new checkouts.
    let current = Mutex::new(String::new());
    let mut gb = globset::GlobSetBuilder::new();
    for x in &cfg.scan.exclude {
        if let Ok(g) = globset::Glob::new(&expand(x).to_string_lossy()) {
            gb.add(g);
        }
    }
    let mut exclude: HashSet<PathBuf> = known.clone();
    exclude.insert(home().join("Library"));
    exclude.insert(home().join("OrbStack"));
    exclude.insert(home().join(".Trash"));
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
        known_projects: project_paths.clone(),
    };
    let mut proj_sorted: Vec<PathBuf> = project_paths.iter().cloned().collect();
    proj_sorted.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    let mut ctx_cache: HashMap<PathBuf, Arc<ProjCtx>> = HashMap::new();
    let mut touched_projects: HashMap<PathBuf, (i64, bool)> = HashMap::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    for d in &outside {
        let d = path_forms(d).into_iter().find(|f| roots.iter().any(|r| f.starts_with(r))).unwrap_or_else(|| d.clone());
        if !roots.iter().any(|r| d.starts_with(r)) || !seen_dirs.insert(d.clone()) {
            continue;
        }
        let proj = proj_sorted.iter().find(|p| d.starts_with(p)).cloned();
        if let Some(p) = &proj {
            let in_git = d.strip_prefix(p).map_or(false, |rel| rel.components().next().map_or(false, |c| c.as_os_str() == ".git"));
            // Edits count as activity, deletions do not: newest file here, not the folder's mtime.
            let newest_file = read_entries(&d).iter().filter(|e| !e.is_dir && e.name != ".DS_Store").map(|e| e.mtime).max().unwrap_or(0);
            let e = touched_projects.entry(p.clone()).or_insert((0, false));
            e.0 = e.0.max(newest_file);
            e.1 |= in_git;
            if in_git {
                continue;
            }
        }
        if !d.is_dir() {
            continue;
        }
        let pctx = proj.map(|p| {
            ctx_cache
                .entry(p.clone())
                .or_insert_with(|| {
                    let ignored = if walker.ignored_rule.is_some() { git::ignored_dirs(&p).into_iter().collect() } else { HashSet::new() };
                    Arc::new(ProjCtx { path: p.clone(), ignored })
                })
                .clone()
        });
        walker.visit_shallow(&d, &known, pctx);
    }
    let new_raws = walker.items.into_inner().unwrap();
    let new_projects = walker.projects.into_inner().unwrap();
    // Several changed folders can lead to the same new checkout: keep the first sighting.
    let mut added: HashSet<PathBuf> = HashSet::new();
    for r in new_raws {
        if !known.contains(&r.path) && added.insert(r.path.clone()) {
            raws.push(r);
        }
    }
    let mut added: HashSet<PathBuf> = HashSet::new();
    for p in new_projects {
        if !project_paths.contains(&p.path) && added.insert(p.path.clone()) {
            projects.push(p);
        }
    }
    for p in projects.iter_mut() {
        if let Some((t, git_touched)) = touched_projects.get(&p.path) {
            let git_t = if *git_touched { git::last_activity(&p.path) } else { None };
            p.last_activity = Some(p.last_activity.unwrap_or(0).max(*t).max(git_t.unwrap_or(0)));
        }
    }

    trace(&started, "structural");
    // Slow providers: reuse until their TTL runs out.
    // simctl is slow to ask: only when simulators changed on disk, or with the slow providers.
    let refresh_slow = now() - idx.slow_at > SLOW_TTL;
    let sim_root = home().join("Library/Developer/CoreSimulator");
    let sim_rules: HashSet<String> = rule_with_provider(cat, "simulator-devices").iter().map(|r| r.id.clone()).collect();
    let refresh_sims = !sim_rules.is_empty() && (refresh_slow || changed.iter().any(|c| c.starts_with(&sim_root)));
    let needs_simctl = refresh_sims || (refresh_slow && !rule_with_provider(cat, "simulator-runtimes").is_empty());
    let devices_json = if needs_simctl { providers::simctl_devices() } else { None };
    let mut slow_at = idx.slow_at;
    let mut warnings: Vec<String> = cat.errors.iter().map(|e| format!("mod error: {}", e)).collect();
    if refresh_slow {
        let old_slow: HashMap<(String, PathBuf), Raw> = raws.iter().filter(|r| r.origin == "slow").map(|r| ((r.rule.clone(), r.path.clone()), r.clone())).collect();
        let (mut slow, warn) = slow_providers(cat, devices_json.as_ref());
        slow.extend(app_leftover_items(cat));
        let slow: Vec<Raw> = slow
            .into_par_iter()
            .map(|mut r| {
                if r.sized.is_none() && !r.is_virtual {
                    match old_slow.get(&(r.rule.clone(), r.path.clone())) {
                        Some(old) if !dirty.contains(&r.path) => r.sized = old.sized.clone(),
                        _ => r.sized = ctx.size_path(&r.path, &HashSet::new()),
                    }
                }
                r
            })
            .collect();
        raws.retain(|r| r.origin != "slow");
        raws.extend(slow);
        warnings.extend(warn);
        slow_at = now();
    }

    trace(&started, "slow");
    // Global locations: cheap to list again; reuse sizes of unchanged ones.
    let cheap = if refresh_sims {
        cheap_providers(cat, devices_json.as_ref())
    } else {
        let mut v = cheap_providers_except_simulators(cat);
        v.extend(raws.iter().filter(|r| r.origin == "provider" && sim_rules.contains(&r.rule)).cloned());
        v
    };
    trace(&started, "cheap providers");
    let mut provider_paths: HashSet<PathBuf> = cheap.iter().filter(|r| !r.is_virtual).map(|r| r.path.clone()).collect();
    provider_paths.extend(raws.iter().filter(|r| r.origin == "slow" && !r.is_virtual).map(|r| r.path.clone()));
    let candidates: Vec<_> = path_candidates(cat).into_iter().filter(|(p, _, _)| !provider_paths.contains(p)).collect();
    let mut claimed: HashSet<PathBuf> = candidates.iter().map(|(p, _, _)| p.clone()).collect();
    claimed.extend(provider_paths.iter().cloned());
    let old_global: HashMap<PathBuf, Raw> = raws.iter().filter(|r| r.origin == "global").map(|r| (r.path.clone(), r.clone())).collect();
    let old_provider: HashMap<(String, PathBuf), Raw> = raws.iter().filter(|r| r.origin == "provider").map(|r| ((r.rule.clone(), r.path.clone()), r.clone())).collect();
    raws.retain(|r| r.origin != "global" && r.origin != "provider");
    let all_claimed: Vec<PathBuf> = claimed.iter().cloned().collect();
    trace(&started, "candidates");
    let globals: Vec<Raw> = candidates
        .par_iter()
        .filter_map(|(path, rule, _)| {
            if let Some(old) = old_global.get(path) {
                if old.rule == rule.id {
                    return Some(old.clone());
                }
            }
            let excl: HashSet<PathBuf> = all_claimed.iter().filter(|o| *o != path && o.starts_with(path)).cloned().collect();
            let (s, children) = ctx.size_path(path, &excl)?;
            Some(Raw { path: path.clone(), name: path_label(path, rule), rule: rule.id.clone(), sized: Some((s, children)), disjoint: true, origin: "global".into(), ..Default::default() })
        })
        .collect();
    raws.extend(globals);
    let providers_now: Vec<Raw> = cheap
        .into_par_iter()
        .map(|mut r| {
            if r.sized.is_none() && !r.is_virtual {
                match old_provider.get(&(r.rule.clone(), r.path.clone())) {
                    Some(old) if old.sized.is_some() => r.sized = old.sized.clone(),
                    _ => r.sized = ctx.size_path(&r.path, &HashSet::new()),
                }
            }
            r
        })
        .collect();
    raws.extend(providers_now);


    trace(&started, "globals");
    // Worktrees: recompute the ones with changes inside, refresh nested sizes of the rest.
    let wt_rule = rule_with_provider(cat, "git-worktrees").into_iter().next();
    let existing_wt: HashSet<PathBuf> = raws.iter().filter(|r| r.origin == "worktree").map(|r| r.path.clone()).collect();
    let need: Vec<FoundProject> = projects
        .iter()
        .filter(|p| p.kind == "worktree" || p.kind == "orphan")
        .filter(|p| !existing_wt.contains(&p.path) || dirty.contains(&p.path) || touched_projects.contains_key(&p.path))
        .cloned()
        .collect();
    if let Some(rule) = wt_rule {
        let need_paths: HashSet<PathBuf> = need.iter().map(|p| p.path.clone()).collect();
        raws.retain(|r| !(r.origin == "worktree" && need_paths.contains(&r.path)));
        let non_wt: Vec<Raw> = raws.iter().filter(|r| r.origin != "worktree").cloned().collect();
        let fresh = worktree_items(rule, &need, &non_wt, &ctx, cfg);
        raws.extend(fresh);
        let nested_sizes: Vec<(PathBuf, u64)> = raws.iter().filter(|r| r.origin != "worktree" && !r.is_virtual).map(|r| (r.path.clone(), r.sized.as_ref().map_or(0, |s| s.0.bytes))).collect();
        for r in raws.iter_mut().filter(|r| r.origin == "worktree") {
            r.nested_bytes = nested_sizes.iter().filter(|(p, _)| p.starts_with(&r.path) && p != &r.path).map(|(_, b)| b).sum();
        }
    }
    // Worktrees that are gone.
    raws.retain(|r| r.origin != "worktree" || r.path.exists());
    projects.retain(|p| p.path.exists());

    trace(&started, "worktrees");
    warnings.extend(access_warnings(cat));
    if detected {
        warnings.push("no roots configured; using detected folders (run `poo roots add <path>`)".into());
    }
    let remeasured: HashSet<PathBuf> = dirty.iter().cloned().collect();
    let live: HashSet<PathBuf> = raws.iter().filter(|r| !r.is_virtual).map(|r| r.path.clone()).collect();
    let mut dirs: Vec<(PathBuf, Sized)> = old_dirs
        .into_iter()
        .filter(|(p, _)| {
            let mut under_live = false;
            for a in p.ancestors() {
                if remeasured.contains(a) {
                    return false;
                }
                under_live |= live.contains(a);
            }
            under_live
        })
        .collect();
    dirs.extend(std::mem::take(&mut *ctx.big.lock().unwrap()));
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    dirs.dedup_by(|a, b| a.0 == b.0);
    save_index(&Index {
        version: INDEX_VERSION,
        fingerprint: idx.fingerprint,
        event_id: new_id,
        full_at: idx.full_at,
        updated_at: now(),
        roots: roots.clone(),
        raws: raws.clone(),
        projects: projects.clone(),
        slow_at,
        dirs,
    });
    trace(&started, "index");
    let procs_list = procs::list();
    trace(&started, "procs");
    warnings.extend(blocked_warnings());
    let mut report = build_report(cfg, cat, raws, projects, &procs_list, roots, warnings, started);
    trace(&started, "report");
    report.warnings.retain(|w| !w.is_empty());
    Some(report)
}

/// Incremental when possible, full otherwise.
pub fn scan_auto(cfg: &Config, cat: &Catalog, opts: &ScanOptions) -> Report {
    if !opts.full {
        if let Some(r) = scan_incremental(cfg, cat, opts) {
            return r;
        }
    }
    scan(cfg, cat, opts)
}

/// `POO_TRACE=1` prints how long each step of an incremental scan took.
fn trace(started: &Instant, step: &str) {
    if std::env::var_os("POO_TRACE").is_some() {
        eprintln!("trace {:>6} ms  {}", started.elapsed().as_millis(), step);
    }
}

/// Guarded folders that did not answer: say so instead of hanging on them.
fn blocked_warnings() -> Vec<String> {
    crate::fsutil::blocked_folders()
        .iter()
        .map(|p| format!("skipped {}: macOS is waiting for permission (System Settings > Privacy & Security > Files and Folders, or Full Disk Access)", crate::util::tilde(p)))
        .collect()
}
