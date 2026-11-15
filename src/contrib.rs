//! Sharing rules back: the safety lint for mods and core, matcher fixtures, `poo mods submit`
//! (scrub personal details, preview, then a pull request through `gh` or a prefilled URL)
//! and `poo suggest` (big folders no rule explains, with draft rules).
//!
//! Nothing here sends anything anywhere on its own: `submit` only talks to GitHub after the
//! person confirmed the exact files and text, and `suggest` only reads.

use crate::catalog::{Catalog, Rule};
use crate::config::{mods_dir, state_dir, Config};
use crate::model::*;
use crate::util::{bytes, home, run_capture, tilde, which};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

pub const UPSTREAM: &str = crate::REPO;

// ---------------------------------------------------------------------------
// Lint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub level: Level,
    pub rule: String,
    pub message: String,
}

fn f(level: Level, rule: &str, message: impl Into<String>) -> Finding {
    Finding { level, rule: rule.to_string(), message: message.into() }
}

/// Folder names that mean nothing on their own. An artifact rule matching one of them needs a
/// marker next to it (parent or path) and proof inside it (children, git ignore, or two
/// parent markers).
pub const GENERIC_NAMES: &[&str] = &[
    "*", "build", "builds", "Build", "Builds", "dist", "out", "output", "outputs", "lib", "libs", "bin", "obj",
    "target", "cache", ".cache", "caches", "Cache", "tmp", "temp", ".tmp", ".temp", "Temp", "vendor", "deps",
    "packages", "public", "site", "release", "releases", "env", "data", "logs", "log", "Logs", "assets", "static",
    "generated", "gen", "artifacts", "coverage", "docs", "Library", "ios", "android", "app", "src", "local",
    "debug", "Debug", "test", "tests", "storage", "var", "views", "book", "pkg", "modules", "compiled", "project",
    "views", "results", "reports", "node", "web", "server", "client",
];

/// Path components the safety floor never enters (kept in sync with `clean::NEVER`).
fn never_components() -> &'static [&'static str] {
    crate::clean::NEVER
}

/// Folders macOS guards with privacy permissions; rules there need `needs-permission`.
const PROTECTED_HINTS: &[&str] = &[
    "/Library/Containers/", "/Library/Group Containers/", "/Library/Mail", "/Library/Messages", "/Library/Safari",
    "/Library/Caches/com.apple.Safari", "/Application Support/MobileSync", "/Application Support/Google/Chrome",
    "/Application Support/AddressBook", "/Library/Calendars", "/Library/Photos",
];

fn todo(s: &Option<String>) -> bool {
    s.as_deref().map_or(true, |t| t.trim().is_empty() || t.contains("TODO"))
}

/// Lints one rule as written (only the fields it sets). `new_rule` is false for overrides of a
/// rule that already exists, which only have to keep the fields they change safe.
/// `contrib` adds the checks only shared rules need (rebuild hints, no exec scripts, system paths).
pub fn lint_spec(spec: &RuleSpec, cat: &Catalog, new_rule: bool, contrib: bool) -> Vec<Finding> {
    use Level::*;
    let id = spec.id.as_str();
    let mut out = vec![];
    if new_rule {
        let valid_id = id.split_once('.').map_or(false, |(a, b)| {
            !a.is_empty() && !b.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._".contains(c))
        });
        if !valid_id {
            out.push(f(Warn, id, "ids look like `tool.thing` in lowercase (cargo.registry, node.next)"));
        }
        for (field, missing) in [
            ("name", spec.name.as_deref().map_or(true, |s| s.trim().is_empty())),
            ("bucket", spec.bucket.is_none()),
            ("group", spec.group.is_none()),
            ("kind", spec.kind.is_none()),
            ("cost", spec.cost.is_none()),
            ("stacks", spec.stacks.as_ref().map_or(true, |s| s.is_empty())),
        ] {
            if missing {
                out.push(f(Error, id, format!("`{}` is required", field)));
            }
        }
        if todo(&spec.description) {
            out.push(f(Error, id, "`description` is required: one line saying what the folder is and why it is safe to remove"));
        }
        if contrib && spec.rebuild.is_none() && matches!(spec.cost, Some(Cost::Rebuild) | Some(Cost::Redownload)) {
            out.push(f(Warn, id, "`rebuild` tells people how the folder comes back (`cargo build`, `npm install`)"));
        }
    }
    if contrib {
        let text = toml::to_string(spec).unwrap_or_default();
        if text.contains("TODO") || text.contains("<the ") {
            out.push(f(Error, id, "fill in every TODO (and placeholders like <the cache folder>) before sharing"));
        }
    }
    if let Some(b) = &spec.bucket {
        if cat.bucket(b).is_none() {
            out.push(f(Error, id, format!("unknown bucket `{}`", b)));
        }
    }
    let kind = spec.kind.or_else(|| cat.rule(id).map(|r| r.kind));
    if contrib && kind == Some(RuleKind::Provider) && spec.provider.as_deref() == Some("exec") {
        out.push(f(Error, id, "exec providers run a script; describe the folder with a path or artifact rule to share it"));
    }
    if kind == Some(RuleKind::Artifact) {
        lint_artifact(spec, cat, &mut out);
    }
    let system = spec.system.unwrap_or(false);
    for p in spec.paths.iter().flatten() {
        let data = spec.cost.or_else(|| cat.rule(id).map(|r| r.cost)) == Some(Cost::Data);
        lint_path(id, p, spec.each.unwrap_or(false), system, data, contrib, spec.tags.as_deref().unwrap_or(&[]), &mut out);
    }
    out
}

fn lint_artifact(spec: &RuleSpec, cat: &Catalog, out: &mut Vec<Finding>) {
    use Level::*;
    let id = spec.id.as_str();
    // Overrides inherit matchers from the rule they change.
    let base = cat.rule(id);
    let raw = |own: &Option<Vec<String>>, base_set: Option<&Vec<String>>| -> Vec<String> {
        own.clone().or_else(|| base_set.cloned()).unwrap_or_default()
    };
    let names = raw(&spec.names, base.map(|r| &r.names.raw));
    if names.is_empty() {
        out.push(f(Error, id, "artifact rules need `names`"));
        return;
    }
    let parent = raw(&spec.parent_any, base.map(|r| &r.parent_any.raw));
    let parent_all = raw(&spec.parent_all, base.map(|r| &r.parent_all.raw));
    let children = [
        raw(&spec.children_any, base.map(|r| &r.children_any.raw)),
        raw(&spec.children_all, base.map(|r| &r.children_all.raw)),
    ]
    .concat();
    let children_all = raw(&spec.children_all, base.map(|r| &r.children_all.raw));
    let children_none = raw(&spec.children_none, base.map(|r| &r.children_none.raw));
    let path_contains = spec.path_contains.clone().or_else(|| base.map(|r| r.path_contains.clone())).unwrap_or_default();
    let ignored = spec.ignored.or_else(|| base.map(|r| r.ignored)).unwrap_or(false);
    let generic: Vec<&String> = names.iter().filter(|n| GENERIC_NAMES.contains(&n.as_str())).collect();
    if generic.is_empty() {
        return;
    }
    if names.iter().any(|n| n == "*") {
        if children_all.is_empty() {
            out.push(f(Error, id, "names = [\"*\"] matches every folder: add children_all with files only this kind of folder has"));
        }
        if children_none.is_empty() {
            out.push(f(Warn, id, "names = [\"*\"]: add children_none with source files (CMakeLists.txt, .git) so a source folder never matches"));
        }
        return;
    }
    // children_all is a fingerprint (pyvenv.cfg, CMakeCache.txt) and counts as a marker too.
    let marker = !parent.is_empty() || !parent_all.is_empty() || !path_contains.is_empty() || !children_all.is_empty();
    let proof = !children.is_empty() || ignored || parent_all.len() >= 2 || !path_contains.is_empty();
    let shown = generic.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    if !marker {
        out.push(f(Error, id, format!("`{}` is a generic name: add parent_any (a file next to it, like Cargo.toml) or path_contains", shown)));
    }
    if !proof {
        out.push(f(Error, id, format!("`{}` is a generic name: add children_any (what is inside) or ignored = true", shown)));
    }
}

#[allow(clippy::too_many_arguments)]
fn lint_path(id: &str, p: &str, each: bool, system: bool, data: bool, contrib: bool, tags: &[String], out: &mut Vec<Finding>) {
    use Level::*;
    let rest = if let Some(r) = p.strip_prefix("~/") {
        Some(r)
    } else if let Some(r) = p.strip_prefix("$TMPDIR") {
        if r.trim_matches('/').is_empty() && !each {
            out.push(f(Error, id, "the whole temp folder is too broad; name what inside it"));
        }
        None
    } else if p.starts_with("$DARWIN_USER_CACHE_DIR") {
        None
    } else {
        if system {
            if contrib {
                out.push(f(Warn, id, format!("`{}` is outside your home folder; maintainers review system = true rules by hand", p)));
            }
        } else {
            out.push(f(Error, id, format!("`{}` must start with ~/ or $TMPDIR (or set system = true for a system cache)", p)));
        }
        None
    };
    if let Some(r) = rest {
        let parts: Vec<&str> = r.split('/').filter(|s| !s.is_empty()).collect();
        let depth = parts.len() + if each { 1 } else { 0 };
        if parts.first().map_or(true, |c| c.starts_with('*') || c.starts_with('{')) {
            out.push(f(Error, id, format!("`{}` matches folders right in your home folder", p)));
        } else if depth < 2 {
            // The floor refuses anything directly in home, so such a rule can never clean.
            // Data rules only show sizes, which is fine.
            if !data {
                out.push(f(Error, id, format!("`{}` sits directly in your home folder, which poo never deletes: point at the folders inside (`{}/*` or `each = true`)", p, p)));
            }
        }
    }
    if p.starts_with("/Users/") || p.contains(&*home().to_string_lossy()) {
        out.push(f(Error, id, format!("`{}` names a home folder; write it as ~/...", p)));
    }
    for c in p.split('/') {
        if never_components().iter().any(|n| *n == c) {
            out.push(f(Error, id, format!("`{}` goes through `{}`, which poo never touches", p, c)));
        }
        if c.starts_with(".env") {
            out.push(f(Error, id, format!("`{}` looks like an env file", p)));
        }
    }
    if PROTECTED_HINTS.iter().any(|h| p.contains(h)) && !tags.iter().any(|t| t == "needs-permission") {
        out.push(f(Warn, id, format!("`{}` is a location macOS protects: add tags = [\"needs-permission\"]", p)));
    }
}

// ---------------------------------------------------------------------------
// Matcher fixtures: TOML files of folders that must (or must not) match a rule.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct Fixture {
    pub rule: String,
    /// The folder's own name.
    pub name: String,
    /// Entries next to the folder (its parent's entries).
    #[serde(default)]
    pub parent: Vec<String>,
    /// Entries inside the folder.
    #[serde(default)]
    pub children: Vec<String>,
    /// Full path, for rules with path_contains. Defaults to /x/project/<name>.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether the rule must match.
    pub matches: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize, serde::Deserialize)]
pub struct FixtureFile {
    #[serde(default)]
    pub fixture: Vec<Fixture>,
}

pub fn read_fixtures(dir: &Path) -> Result<Vec<(String, Fixture)>> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(out) };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |e| e == "toml")).collect();
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p)?;
        let file: FixtureFile = toml::from_str(&text).with_context(|| format!("{}", p.display()))?;
        let label = p.file_name().unwrap().to_string_lossy().to_string();
        out.extend(file.fixture.into_iter().map(|x| (label.clone(), x)));
    }
    Ok(out)
}

pub fn fixture_matches(rule: &Rule, fx: &Fixture) -> bool {
    let path = fx.path.clone().unwrap_or_else(|| format!("/x/project/{}", fx.name));
    rule.matches_artifact(&fx.name, Path::new(&path), &fx.parent, &fx.children)
}

/// Every fixture must agree with its rule.
pub fn check_fixtures(cat: &Catalog, dir: &Path) -> Vec<Finding> {
    let mut out = vec![];
    match read_fixtures(dir) {
        Err(e) => out.push(f(Level::Error, "fixtures", format!("{:#}", e))),
        Ok(list) => {
            for (file, fx) in list {
                match cat.rule(&fx.rule) {
                    None => out.push(f(Level::Error, &fx.rule, format!("{}: fixture for an unknown rule", file))),
                    Some(r) if fixture_matches(r, &fx) != fx.matches => out.push(f(
                        Level::Error,
                        &fx.rule,
                        format!(
                            "{}: `{}` (next to [{}], holding [{}]) should {}match",
                            file,
                            fx.name,
                            fx.parent.join(", "),
                            fx.children.join(", "),
                            if fx.matches { "" } else { "not " }
                        ),
                    )),
                    _ => {}
                }
            }
        }
    }
    out
}

/// Rule ids in rules/core at a git ref, for "what is new in this pull request".
pub fn core_ids_at(repo: &Path, git_ref: &str) -> Result<HashSet<String>> {
    let list = run_capture(&["git", "-C", &repo.to_string_lossy(), "ls-tree", "--name-only", git_ref, "rules/core/"], 20)
        .context("git ls-tree failed (is the base ref fetched?)")?;
    let mut ids = HashSet::new();
    for file in list.lines().filter(|l| l.ends_with(".toml")) {
        let spec = format!("{}:{}", git_ref, file);
        let text = run_capture(&["git", "-C", &repo.to_string_lossy(), "show", &spec], 20).unwrap_or_default();
        if let Ok(m) = toml::from_str::<ModFile>(&text) {
            ids.extend(m.rule.into_iter().map(|r| r.id));
        }
    }
    Ok(ids)
}

#[derive(Debug, Serialize)]
pub struct LintReport {
    pub rules: usize,
    pub new_rules: Vec<String>,
    pub findings: Vec<Finding>,
}

impl LintReport {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|x| x.level == Level::Error).count()
    }
}

/// Lints the core mod: every rule, every fixture, and (with a base ref) fixtures for new
/// artifact rules with generic names.
pub fn lint_core(cat: &Catalog, repo: &Path, base: Option<&str>) -> Result<LintReport> {
    let fixtures_dir = repo.join("tests/rule-fixtures");
    let mut findings = vec![];
    let core: Vec<&Rule> = cat.rules.iter().filter(|r| r.source.split('+').next() == Some("core")).collect();
    for r in &core {
        findings.extend(lint_spec(&r.spec, cat, true, false));
    }
    findings.extend(check_fixtures(cat, &fixtures_dir));
    let mut new_rules = vec![];
    if let Some(b) = base {
        let old = core_ids_at(repo, b)?;
        let fx = read_fixtures(&fixtures_dir).unwrap_or_default();
        for r in &core {
            if old.contains(&r.id) {
                continue;
            }
            new_rules.push(r.id.clone());
            let generic = r.kind == RuleKind::Artifact && r.names.raw.iter().any(|n| GENERIC_NAMES.contains(&n.as_str()));
            if generic {
                let pos = fx.iter().any(|(_, x)| x.rule == r.id && x.matches);
                let neg = fx.iter().any(|(_, x)| x.rule == r.id && !x.matches);
                if !(pos && neg) {
                    findings.push(f(
                        Level::Error,
                        &r.id,
                        "new rule with a generic folder name: add a fixture that matches and one that must not (tests/rule-fixtures/)",
                    ));
                }
            }
        }
    }
    findings.sort_by(|a, b| a.level.cmp(&b.level).then(a.rule.cmp(&b.rule)));
    Ok(LintReport { rules: core.len(), new_rules, findings })
}

// ---------------------------------------------------------------------------
// User mods as written
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UserRule {
    pub mod_id: String,
    pub file: PathBuf,
    pub spec: RuleSpec,
}

/// Rules exactly as the user's mods write them (not merged with core).
pub fn user_rules() -> Vec<UserRule> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(mods_dir()) else { return out };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        if fname.starts_with('.') {
            continue;
        }
        let files: Vec<PathBuf> = if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|x| x.extension().map_or(false, |e| e == "toml")).collect())
                .unwrap_or_default();
            v.sort();
            v
        } else if fname.ends_with(".toml") {
            vec![p.clone()]
        } else {
            continue;
        };
        let mut mod_id = fname.trim_end_matches(".toml").to_string();
        let mut specs = vec![];
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let Ok(m) = toml::from_str::<ModFile>(&text) else { continue };
            if let Some(id) = m.id.clone() {
                if file.file_name().map_or(false, |n| n == "mod.toml") || !p.is_dir() {
                    mod_id = id;
                }
            }
            specs.extend(m.rule.into_iter().map(|s| (file.clone(), s)));
        }
        out.extend(specs.into_iter().map(|(file, spec)| UserRule { mod_id: mod_id.clone(), file, spec }));
    }
    out
}

// ---------------------------------------------------------------------------
// Scrubbing personal details
// ---------------------------------------------------------------------------

/// Words that never count as personal even when a project is called that.
const COMMON: &[&str] = &[
    "app", "apps", "api", "web", "www", "server", "client", "core", "docs", "test", "tests", "src", "build", "dist",
    "cache", "data", "main", "master", "dev", "demo", "example", "examples", "lib", "libs", "tools", "site", "blog",
    "mobile", "backend", "frontend", "infra", "scripts", "config", "packages", "android", "ios", "macos", "node",
    "rust", "python", "swift", "go", "java", "ruby", "home", "user", "users", "project", "projects", "code", "work",
    "temp", "tmp", "public", "assets", "static", "target", "vendor", "deps", "logs", "notes", "admin", "shared",
    "common", "utils", "util", "plugins", "plugin", "release", "landing", "website", "worktrees", "library",
];

pub struct Scrubber {
    home: String,
    words: Vec<(String, &'static str)>,
}

impl Scrubber {
    pub fn new(cfg: &Config, report: Option<&Report>) -> Scrubber {
        let home = home().to_string_lossy().to_string();
        let mut words: Vec<(String, &'static str)> = vec![];
        let user = std::env::var("USER").unwrap_or_default();
        if user.len() >= 3 {
            words.push((user, "<user>"));
        }
        if let Some(name) = home.rsplit('/').next() {
            if name.len() >= 3 {
                words.push((name.to_string(), "<user>"));
            }
        }
        if let Some(real) = run_capture(&["id", "-F"], 3) {
            for part in real.split_whitespace().filter(|p| p.len() >= 2) {
                words.push((part.to_string(), "<user>"));
            }
        }
        for key in ["ComputerName", "LocalHostName", "HostName"] {
            if let Some(h) = run_capture(&["scutil", "--get", key], 3) {
                let h = h.trim().to_string();
                if h.len() >= 3 {
                    words.push((h.clone(), "<host>"));
                    words.push((h.replace(' ', "-"), "<host>"));
                }
            }
        }
        if let Some(h) = run_capture(&["hostname"], 3) {
            let h = h.trim().to_string();
            if h.len() >= 3 {
                words.push((h.split('.').next().unwrap_or(&h).to_string(), "<host>"));
                words.push((h, "<host>"));
            }
        }
        let mut projects: BTreeSet<String> = BTreeSet::new();
        if let Some(r) = report {
            for p in &r.projects {
                projects.insert(p.name.clone());
                if let Some(repo) = &p.repo {
                    if let Some(n) = Path::new(repo).file_name() {
                        projects.insert(n.to_string_lossy().to_string());
                    }
                }
            }
        }
        for root in cfg.effective_roots().0 {
            if let Ok(rd) = std::fs::read_dir(&root) {
                for e in rd.flatten().filter(|e| e.path().is_dir()) {
                    projects.insert(e.file_name().to_string_lossy().to_string());
                }
            }
        }
        for p in projects {
            if p.len() >= 4 && !COMMON.contains(&p.to_lowercase().as_str()) && !p.starts_with('.') {
                words.push((p, "<project>"));
            }
        }
        // Longest first, so "inline-chat" goes before "inline".
        words.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        words.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
        Scrubber { home, words }
    }

    /// Returns the scrubbed text and what was replaced (as the kind of detail, never the value).
    pub fn scrub(&self, s: &str, notes: &mut BTreeSet<String>) -> String {
        let mut t = s.replace(&self.home, "~");
        if t != s {
            notes.insert("your home folder path became ~".into());
        }
        // /Users/<anyone>/ → ~/
        while let Some(i) = t.find("/Users/") {
            let rest = &t[i + 7..];
            let end = rest.find('/').unwrap_or(rest.len());
            t = format!("{}~{}", &t[..i], &rest[end..]);
            notes.insert("a /Users/<name> path became ~".into());
        }
        t = scrub_emails(&t, notes);
        for (w, label) in &self.words {
            let replaced = replace_word(&t, w, label);
            if replaced != t {
                notes.insert(format!("{} names replaced with {}", label.trim_matches(|c| c == '<' || c == '>'), label));
                t = replaced;
            }
        }
        t
    }
}

fn scrub_emails(s: &str, notes: &mut BTreeSet<String>) -> String {
    let mut out: Vec<String> = vec![];
    for tok in s.split(' ') {
        let core = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '-' && c != '+');
        if let Some((a, b)) = core.split_once('@') {
            if !a.is_empty() && b.contains('.') && !b.starts_with('.') {
                notes.insert("an email address was removed".into());
                out.push(tok.replace(core, "<email>"));
                continue;
            }
        }
        out.push(tok.to_string());
    }
    out.join(" ")
}

/// Case-insensitive, whole-word replacement (word characters are alphanumerics, `-` and `_`).
pub fn replace_word(s: &str, word: &str, with: &str) -> String {
    if word.is_empty() {
        return s.to_string();
    }
    let lower = s.to_lowercase();
    let w = word.to_lowercase();
    if lower.len() != s.len() {
        return s.to_string();
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut out = String::new();
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&w) {
        let start = i + pos;
        let end = start + w.len();
        let before = s[..start].chars().last().map_or(true, |c| !is_word(c));
        let after = s[end..].chars().next().map_or(true, |c| !is_word(c));
        out.push_str(&s[i..start]);
        if before && after {
            out.push_str(with);
        } else {
            out.push_str(&s[start..end]);
        }
        i = end;
    }
    out.push_str(&s[i..]);
    out
}

fn scrub_spec(spec: &RuleSpec, sc: &Scrubber, notes: &mut BTreeSet<String>) -> RuleSpec {
    let v = toml::Value::try_from(spec).expect("rule serializes");
    let v = scrub_value(v, sc, notes);
    v.try_into().expect("rule deserializes")
}

fn scrub_value(v: toml::Value, sc: &Scrubber, notes: &mut BTreeSet<String>) -> toml::Value {
    match v {
        toml::Value::String(s) => toml::Value::String(sc.scrub(&s, notes)),
        toml::Value::Array(a) => toml::Value::Array(a.into_iter().map(|x| scrub_value(x, sc, notes)).collect()),
        toml::Value::Table(t) => toml::Value::Table(t.into_iter().map(|(k, x)| (k, scrub_value(x, sc, notes))).collect()),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Rendering rules as TOML in core's style
// ---------------------------------------------------------------------------

const KEY_ORDER: &[&str] = &[
    "id", "name", "bucket", "group", "kind", "provider", "tags", "stacks", "enabled", "names", "parent_any", "parent_all",
    "parent_none", "children_any", "children_all", "children_none", "path_contains", "path_excludes", "ignored", "paths",
    "each", "files", "extensions", "exclude", "min_age_days", "min_bytes", "system", "contents", "running_apps", "cost",
    "tier", "rebuild", "warm_days", "idle_days", "action", "command", "description", "recommend",
];

pub fn render_rule(spec: &RuleSpec) -> String {
    let v = toml::Value::try_from(spec).expect("rule serializes");
    let table = v.as_table().cloned().unwrap_or_default();
    let mut s = String::from("[[rule]]\n");
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort_by_key(|k| KEY_ORDER.iter().position(|x| x == k).unwrap_or(usize::MAX));
    let mut params = None;
    for k in keys {
        let val = &table[k];
        if k == "params" {
            params = Some(val.clone());
            continue;
        }
        s.push_str(&format!("{} = {}\n", k, inline(val)));
    }
    if let Some(toml::Value::Table(p)) = params {
        for (k, v) in p {
            s.push_str(&format!("params.{} = {}\n", k, inline(&v)));
        }
    }
    s
}

fn inline(v: &toml::Value) -> String {
    match v {
        toml::Value::Array(a) => format!("[{}]", a.iter().map(inline).collect::<Vec<_>>().join(", ")),
        toml::Value::Table(t) => format!("{{ {} }}", t.iter().map(|(k, v)| format!("{} = {}", k, inline(v))).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Submissions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    /// Path in the repository.
    pub path: String,
    /// Appended to an existing file, or a new file.
    pub append: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub rule: String,
    pub matched: String,
    pub recreated_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Submission {
    pub slug: String,
    pub rules: Vec<String>,
    pub overrides: Vec<String>,
    pub title: String,
    pub body: String,
    pub files: Vec<FileChange>,
    pub findings: Vec<Finding>,
    /// What was removed or generalized while scrubbing (kinds, not values).
    pub scrubbed: Vec<String>,
    pub tool_versions: Vec<String>,
    pub evidence: Vec<Evidence>,
    /// Where the files and text were saved for inspection.
    pub saved_to: String,
}

impl Submission {
    pub fn blocked(&self) -> bool {
        self.findings.iter().any(|x| x.level == Level::Error)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SubmitOptions {
    pub evidence: bool,
    pub tools: Vec<String>,
    pub note: Option<String>,
}

/// Which core file a new rule belongs in. None: a new contrib file.
pub fn target_file(spec: &RuleSpec) -> Option<&'static str> {
    let stacks = spec.stacks.clone().unwrap_or_default();
    let has = |s: &str| stacks.iter().any(|x| x == s);
    let bucket = spec.bucket.as_deref().unwrap_or("");
    let artifact = spec.kind == Some(RuleKind::Artifact);
    Some(match () {
        _ if has("xcode") || has("swift") || has("cocoapods") || has("carthage") || has("tuist") => "apple.toml",
        _ if has("react-native") || has("flutter") || has("android") && artifact => "dev-mobile.toml",
        _ if has("rust") => "rust.toml",
        _ if has("node") || has("bun") || has("deno") || has("pnpm") || has("yarn") => if artifact { "dev-web.toml" } else { "web.toml" },
        _ if has("python") || has("conda") || has("jupyter") => "python.toml",
        _ if has("jvm") || has("gradle") || has("maven") || has("scala") || has("kotlin") || has("clojure") || has("android") => "dev-jvm.toml",
        _ if has("ruby") || has("php") || has("elixir") || has("erlang") || has("haskell") || has("ocaml") || has("dotnet") || has("lean") => "dev-langs.toml",
        _ if has("cpp") || has("cmake") || has("nix") || has("bazel") => "dev-native.toml",
        _ if has("terraform") || has("cloud") || has("kubernetes") => "tools-cloud.toml",
        _ if has("browsers") => "macos-browsers.toml",
        _ if has("docker") || has("vm") || bucket == "containers" => "containers.toml",
        _ if bucket == "ai" => "ai.toml",
        _ if has("macos") && bucket == "system" => "macos-apps.toml",
        _ => return None,
    })
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

/// Versions of the tools behind a rule's stacks (or `extra` commands), first line only.
fn tool_versions(stacks: &BTreeSet<String>, extra: &[String], sc: &Scrubber, notes: &mut BTreeSet<String>) -> Vec<String> {
    let map: &[(&str, &[&str])] = &[
        ("rust", &["cargo", "--version"]), ("node", &["node", "--version"]), ("python", &["python3", "--version"]),
        ("xcode", &["xcodebuild", "-version"]), ("swift", &["swift", "--version"]), ("go", &["go", "version"]),
        ("ruby", &["ruby", "--version"]), ("gradle", &["gradle", "--version"]), ("java", &["java", "-version"]),
        ("bun", &["bun", "--version"]), ("deno", &["deno", "--version"]), ("pnpm", &["pnpm", "--version"]),
        ("yarn", &["yarn", "--version"]), ("flutter", &["flutter", "--version"]), ("dotnet", &["dotnet", "--version"]),
        ("php", &["php", "--version"]), ("elixir", &["elixir", "--version"]), ("zig", &["zig", "version"]),
        ("docker", &["docker", "--version"]), ("terraform", &["terraform", "version"]), ("lean", &["lean", "--version"]),
    ];
    let mut cmds: Vec<Vec<String>> = vec![];
    for s in stacks {
        if let Some((_, c)) = map.iter().find(|(k, _)| k == s) {
            cmds.push(c.iter().map(|x| x.to_string()).collect());
        }
    }
    for t in extra {
        cmds.push(vec![t.clone(), "--version".into()]);
    }
    let mut out = vec![];
    for c in cmds {
        if which(&c[0]).is_none() {
            continue;
        }
        let args: Vec<&str> = c.iter().map(|s| s.as_str()).collect();
        if let Some(v) = run_capture(&args, 5) {
            if let Some(line) = v.lines().find(|l| !l.trim().is_empty()) {
                out.push(format!("{}: {}", c[0], sc.scrub(line.trim(), notes)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Fixtures from folders the rule matched on this Mac: only entries the rule's own matchers
/// mention are kept, so file names from your projects never leave.
fn fixtures_from_report(rule: &Rule, report: Option<&Report>) -> Vec<Fixture> {
    let mut out = vec![];
    if rule.kind != RuleKind::Artifact {
        return out;
    }
    let mentioned = |names: &[String], sets: &[&crate::catalog::NameSet]| -> Vec<String> {
        names.iter().filter(|n| sets.iter().any(|s| s.matches(n))).cloned().collect()
    };
    if let Some(r) = report {
        for it in r.items.iter().filter(|i| i.rule == rule.id).take(3) {
            let p = Path::new(&it.path);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let parent = p.parent().map(|d| crate::fsutil::names(&crate::fsutil::read_entries(d))).unwrap_or_default();
            let children = crate::fsutil::names(&crate::fsutil::read_entries(p));
            let fx = Fixture {
                rule: rule.id.clone(),
                name,
                parent: mentioned(&parent, &[&rule.parent_any, &rule.parent_all]),
                children: mentioned(&children, &[&rule.children_any, &rule.children_all]),
                path: None,
                matches: true,
                note: Some("seen on the submitter's Mac".into()),
            };
            if fixture_matches(rule, &fx) && !out.iter().any(|x: &Fixture| x.name == fx.name && x.parent == fx.parent && x.children == fx.children) {
                out.push(fx);
            }
        }
    }
    if let Some(first) = out.first().cloned() {
        // The same folder without its markers must not match.
        let neg = Fixture { parent: vec![], children: vec![], matches: false, note: Some("no markers: must not match".into()), ..first };
        if !fixture_matches(rule, &neg) {
            out.push(neg);
        }
    }
    out
}

pub fn prepare(cfg: &Config, cat: &Catalog, report: Option<&Report>, ids: &[String], opts: &SubmitOptions) -> Result<Submission> {
    let all = user_rules();
    let mut picked: Vec<UserRule> = vec![];
    for id in ids {
        let by_mod: Vec<&UserRule> = all.iter().filter(|u| &u.mod_id == id).collect();
        if !by_mod.is_empty() {
            picked.extend(by_mod.into_iter().cloned());
        } else if let Some(u) = all.iter().find(|u| &u.spec.id == id) {
            picked.push(u.clone());
        } else if cat.rule(id).map_or(false, |r| r.source == "core") {
            bail!("`{}` is a built-in rule with no changes from your mods; nothing to submit", id);
        } else {
            bail!("no mod or mod rule called `{}` (see `poo mods ls` and `poo rules ls`)", id);
        }
    }
    picked.dedup_by(|a, b| a.spec.id == b.spec.id);
    if picked.is_empty() {
        bail!("nothing to submit");
    }
    let sc = Scrubber::new(cfg, report);
    let mut notes = BTreeSet::new();
    let mut findings = vec![];
    let mut new_specs: Vec<RuleSpec> = vec![];
    let mut overrides: Vec<RuleSpec> = vec![];
    let mut stacks = BTreeSet::new();
    let mut evidence = vec![];
    let mut fixtures = vec![];
    for u in &picked {
        let in_core = cat.rule(&u.spec.id).map_or(false, |r| r.source.split('+').next() == Some("core"));
        let mut spec = scrub_spec(&u.spec, &sc, &mut notes);
        spec.enabled = None;
        findings.extend(lint_spec(&spec, cat, !in_core, true));
        stacks.extend(spec.stacks.clone().unwrap_or_default());
        if let Some(rule) = cat.rule(&u.spec.id) {
            let hits = report.and_then(|r| r.rule_hits.get(&rule.id)).copied();
            evidence.push(Evidence {
                rule: rule.id.clone(),
                matched: match hits {
                    Some((n, b)) if opts.evidence => format!("{} folder{} on the submitter's Mac, {} in total", n, if n == 1 { "" } else { "s" }, bytes(round_bytes(b))),
                    Some(_) => "not shared".into(),
                    None => "no matches in the last scan".into(),
                },
                recreated_by: spec.rebuild.clone().or_else(|| rule.rebuild.clone()),
            });
            if opts.evidence && !in_core {
                fixtures.extend(fixtures_from_report(rule, report));
            }
        }
        if in_core {
            overrides.push(spec);
        } else {
            new_specs.push(spec);
        }
    }
    let first = picked[0].mod_id.clone();
    let slug = slugify(&sc.scrub(if ids.len() == 1 { &ids[0] } else { &first }, &mut BTreeSet::new()));
    let slug = if slug.is_empty() { "rules".to_string() } else { slug };
    // Group new rules by destination file.
    let mut by_file: BTreeMap<String, Vec<&RuleSpec>> = BTreeMap::new();
    for s in &new_specs {
        let file = target_file(s).map(|f| f.to_string()).unwrap_or_else(|| format!("contrib-{}.toml", slug));
        by_file.entry(file).or_default().push(s);
    }
    let mut files = vec![];
    for (file, specs) in by_file {
        let contrib = file.starts_with("contrib-");
        let mut text = if contrib {
            format!("# Contributed with `poo mods submit`. Maintainers may move these rules into a topic file.\n")
        } else {
            String::new()
        };
        for s in specs {
            text.push('\n');
            text.push_str(&render_rule(s));
        }
        files.push(FileChange { path: format!("rules/core/{}", file), append: !contrib, text });
    }
    if !overrides.is_empty() {
        let mut text = String::from("# Changes to built-in rules, contributed with `poo mods submit`.\n# A maintainer folds these fields into the original rule before merging.\n");
        for s in &overrides {
            text.push('\n');
            text.push_str(&render_rule(s));
        }
        files.push(FileChange { path: format!("rules/proposals/{}.toml", slug), append: false, text });
    }
    if !fixtures.is_empty() {
        let text = toml::to_string(&FixtureFile { fixture: fixtures }).unwrap_or_default();
        files.push(FileChange { path: format!("tests/rule-fixtures/{}.toml", slug), append: false, text });
    }
    let tool_versions = if opts.evidence { tool_versions(&stacks, &opts.tools, &sc, &mut notes) } else { vec![] };
    let names: Vec<String> = new_specs.iter().chain(overrides.iter()).map(|s| s.id.clone()).collect();
    let title = if names.len() == 1 {
        format!("rules: {}", new_specs.first().or(overrides.first()).and_then(|s| s.name.clone()).unwrap_or_else(|| names[0].clone()).to_lowercase())
    } else {
        format!("rules: {} rules from the {} mod", names.len(), slug)
    };
    let note = opts.note.as_ref().map(|n| sc.scrub(n, &mut notes));
    let body = pr_body(&new_specs, &overrides, &evidence, &tool_versions, note.as_deref(), &findings);
    let saved = state_dir().join("submissions").join(&slug);
    let _ = std::fs::create_dir_all(&saved);
    for fc in &files {
        let _ = std::fs::write(saved.join(fc.path.replace('/', "__")), &fc.text);
    }
    let _ = std::fs::write(saved.join("PR.md"), format!("# {}\n\n{}", title, body));
    Ok(Submission {
        slug,
        rules: new_specs.iter().map(|s| s.id.clone()).collect(),
        overrides: overrides.iter().map(|s| s.id.clone()).collect(),
        title,
        body,
        files,
        findings,
        scrubbed: notes.into_iter().collect(),
        tool_versions,
        evidence,
        saved_to: tilde(&saved),
    })
}

fn round_bytes(b: u64) -> u64 {
    // Two significant digits: enough to say "big", not enough to fingerprint.
    let mut m = 1u64;
    while b / m >= 100 {
        m *= 10;
    }
    (b / m) * m
}

fn pr_body(new: &[RuleSpec], overrides: &[RuleSpec], ev: &[Evidence], tools: &[String], note: Option<&str>, findings: &[Finding]) -> String {
    let mut s = String::new();
    s.push_str("## What this adds\n\n");
    for r in new {
        s.push_str(&format!(
            "- **{}** `{}`: {}\n",
            r.name.clone().unwrap_or_default(),
            r.id,
            r.description.clone().unwrap_or_default()
        ));
    }
    for r in overrides {
        s.push_str(&format!("- change to built-in `{}` (fields: {})\n", r.id, changed_fields(r).join(", ")));
    }
    if let Some(n) = note {
        s.push_str(&format!("\n{}\n", n));
    }
    s.push_str("\n## Evidence\n\n| rule | seen on my Mac | how it comes back |\n|---|---|---|\n");
    for e in ev {
        s.push_str(&format!("| `{}` | {} | {} |\n", e.rule, e.matched, e.recreated_by.clone().unwrap_or_else(|| "n/a".into())));
    }
    if !tools.is_empty() {
        s.push_str("\nTool versions:\n\n");
        for t in tools {
            s.push_str(&format!("- {}\n", t));
        }
    }
    s.push_str("\n## Safety checklist\n\n");
    s.push_str("- [x] `poo mods validate` and the safety lint pass on my machine\n");
    s.push_str("- [x] paths stay under `~` or `$TMPDIR`; generic folder names have markers\n");
    s.push_str("- [ ] the folder really comes back by itself or with the command above\n");
    s.push_str("- [ ] apps that keep it open are listed in `running_apps`\n");
    let warns: Vec<&Finding> = findings.iter().filter(|x| x.level == Level::Warn).collect();
    if !warns.is_empty() {
        s.push_str("\nLint notes:\n\n");
        for w in warns {
            s.push_str(&format!("- `{}`: {}\n", w.rule, w.message));
        }
    }
    s.push_str("\n---\nSubmitted with `poo mods submit`. Personal details (home path, user, host and project names) were removed before this left the machine.\n");
    s
}

fn changed_fields(spec: &RuleSpec) -> Vec<String> {
    toml::Value::try_from(spec)
        .ok()
        .and_then(|v| v.as_table().map(|t| t.keys().filter(|k| *k != "id").cloned().collect()))
        .unwrap_or_default()
}

/// Opens the pull request with `gh`: fork, branch from upstream main, write the files through
/// the GitHub contents API, open the PR. Returns its URL.
pub fn open_pr(sub: &Submission) -> Result<String> {
    if which("gh").is_none() {
        bail!("gh is not installed");
    }
    let gh = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new("gh").args(args).output().context("running gh")?;
        if !out.status.success() {
            bail!("gh {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    gh(&["auth", "status"]).context("gh is not signed in (run `gh auth login`)")?;
    let login = gh(&["api", "user", "-q", ".login"])?;
    let _ = gh(&["repo", "fork", UPSTREAM, "--clone=false", "--default-branch-only"]);
    let fork = format!("{}/poo", login);
    let _ = gh(&["repo", "sync", &fork, "--branch", "main"]);
    let base = gh(&["api", &format!("repos/{}/git/ref/heads/main", UPSTREAM), "-q", ".object.sha"])?;
    let branch = format!("mods/{}-{}", sub.slug, &crate::util::short_hash(&sub.body)[..6]);
    gh(&["api", "-X", "POST", &format!("repos/{}/git/refs", fork), "-f", &format!("ref=refs/heads/{}", branch), "-f", &format!("sha={}", base)])
        .or_else(|_| {
            let fork_main = gh(&["api", &format!("repos/{}/git/ref/heads/main", fork), "-q", ".object.sha"])?;
            gh(&["api", "-X", "POST", &format!("repos/{}/git/refs", fork), "-f", &format!("ref=refs/heads/{}", branch), "-f", &format!("sha={}", fork_main)])
        })?;
    for fc in &sub.files {
        let existing = gh(&["api", "-H", "Accept: application/vnd.github.raw", &format!("repos/{}/contents/{}?ref={}", fork, fc.path, branch)]).ok();
        let sha = gh(&["api", &format!("repos/{}/contents/{}?ref={}", fork, fc.path, branch), "-q", ".sha"]).ok();
        let content = match (&existing, fc.append) {
            (Some(old), true) => format!("{}\n{}", old.trim_end(), fc.text),
            _ => fc.text.clone(),
        };
        let mut body = serde_json::json!({
            "message": format!("rules: {} (poo mods submit)", fc.path.rsplit('/').next().unwrap_or("")),
            "content": base64(content.as_bytes()),
            "branch": branch,
        });
        if let Some(sha) = sha {
            body["sha"] = serde_json::json!(sha);
        }
        let tmp = std::env::temp_dir().join(format!("poo-submit-{}.json", std::process::id()));
        std::fs::write(&tmp, body.to_string())?;
        let res = gh(&["api", "-X", "PUT", &format!("repos/{}/contents/{}", fork, fc.path), "--input", &tmp.to_string_lossy()]);
        let _ = std::fs::remove_file(&tmp);
        res?;
    }
    let body_file = std::env::temp_dir().join(format!("poo-submit-body-{}.md", std::process::id()));
    std::fs::write(&body_file, &sub.body)?;
    let url = gh(&[
        "pr", "create", "--repo", UPSTREAM, "--head", &format!("{}:{}", login, branch), "--base", "main", "--title", &sub.title,
        "--body-file", &body_file.to_string_lossy(),
    ]);
    let _ = std::fs::remove_file(&body_file);
    url
}

/// Links that need no gh: a prefilled new-file page for each new file (GitHub forks for you),
/// and a prefilled issue with everything.
pub fn fallback_urls(sub: &Submission) -> Vec<(String, String)> {
    let mut out = vec![];
    for fc in sub.files.iter().filter(|f| !f.append) {
        let (dir, name) = fc.path.rsplit_once('/').unwrap_or(("", &fc.path));
        let url = format!("https://github.com/{}/new/main/{}?filename={}&value={}", UPSTREAM, dir, pct(name), pct(&fc.text));
        out.push((format!("create {}", fc.path), url));
    }
    let mut body = sub.body.clone();
    for fc in &sub.files {
        body.push_str(&format!("\n\n`{}`{}:\n\n```toml\n{}\n```\n", fc.path, if fc.append { " (append)" } else { "" }, fc.text.trim()));
    }
    let issue = format!("https://github.com/{}/issues/new?labels=rules&title={}&body={}", UPSTREAM, pct(&sub.title), pct(&body));
    if issue.len() < 8000 {
        out.push(("open an issue with everything".into(), issue));
    } else {
        out.push((
            "open an issue and paste PR.md from the folder above".into(),
            format!("https://github.com/{}/issues/new?labels=rules&title={}", UPSTREAM, pct(&sub.title)),
        ));
    }
    out
}

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{:02X}", b));
        }
    }
    o
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

// ---------------------------------------------------------------------------
// Suggest: big folders no rule explains
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub path: String,
    pub bytes: u64,
    /// ignored-folder | generic-cache | home-folder | app-support
    pub source: String,
    pub why: String,
    pub draft: String,
}

const GENERIC_RULES: &[&str] = &["git.ignored", "system.app-caches", "system.xdg-cache", "system.chromium-app-caches"];

pub fn suggest(cat: &Catalog, report: &Report, min_bytes: u64, prefix: &str) -> Vec<Candidate> {
    let h = home();
    let mut out = vec![];
    // 1. Items only generic rules explain.
    for it in report.items.iter().filter(|i| GENERIC_RULES.contains(&i.rule.as_str()) && i.bytes >= min_bytes) {
        let p = Path::new(&it.path);
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if it.rule == "git.ignored" {
            out.push(Candidate {
                path: tilde(p),
                bytes: it.bytes,
                source: "ignored-folder".into(),
                why: "git ignores it inside a project, but no rule says what it is".into(),
                draft: draft_artifact(cat, p, &name, prefix),
            });
        } else {
            out.push(Candidate {
                path: tilde(p),
                bytes: it.bytes,
                source: "generic-cache".into(),
                why: format!("only the catch-all `{}` rule covers it, without an app guard or rebuild hint", it.rule),
                draft: draft_path(&tilde(p), &name, prefix, "caches"),
            });
        }
    }
    // 2. Big dot folders and Application Support folders whose space no item explains.
    let explained: Vec<(&Path, u64)> = report.items.iter().filter(|i| !i.path.contains("://")).map(|i| (Path::new(&i.path), i.bytes)).collect();
    let mut dirs: Vec<PathBuf> = vec![];
    for parent in [h.clone(), h.join("Library/Application Support"), h.join(".local/share"), h.join(".config")] {
        for e in crate::fsutil::read_entries(&parent) {
            if !e.is_dir || e.is_symlink {
                continue;
            }
            let hidden_only = parent == h;
            if hidden_only && !e.name.starts_with('.') {
                continue;
            }
            if matches!(e.name.as_str(), ".Trash" | ".cache" | ".local" | ".config" | ".git") || never_components().contains(&e.name.as_str()) {
                continue;
            }
            dirs.push(e.path);
        }
    }
    let sizer = crate::fsutil::SizeCtx::default();
    for d in dirs {
        let Some((s, _)) = sizer.size_path(&d, &HashSet::new()) else { continue };
        if s.bytes < min_bytes {
            continue;
        }
        let covered: u64 = explained.iter().filter(|(p, _)| p.starts_with(&d)).map(|(_, b)| *b).sum();
        let under_rule = explained.iter().any(|(p, _)| d.starts_with(p));
        let rest = s.bytes.saturating_sub(covered);
        if under_rule || rest < min_bytes {
            continue;
        }
        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Candidate {
            path: tilde(&d),
            bytes: rest,
            source: if d.starts_with(h.join("Library")) { "app-support".into() } else { "home-folder".into() },
            why: format!("{} here that no rule explains; look inside for the cache or download part", bytes(rest)),
            draft: draft_path(&format!("{}/<the cache folder>", tilde(&d)), &name, prefix, "caches"),
        });
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

fn draft_id(prefix: &str, name: &str) -> String {
    let n = slugify(name.trim_start_matches('.'));
    format!("{}.{}", prefix, if n.is_empty() { "folder".into() } else { n })
}

fn draft_artifact(cat: &Catalog, p: &Path, name: &str, prefix: &str) -> String {
    let parent = p.parent().map(|d| crate::fsutil::names(&crate::fsutil::read_entries(d))).unwrap_or_default();
    let stacks = cat.stacks_for(&parent);
    let markers: Vec<String> = parent
        .iter()
        .filter(|n| cat.stacks.iter().any(|s| s.set.matches(n)))
        .take(3)
        .cloned()
        .collect();
    let mut children: Vec<String> = crate::fsutil::read_entries(p)
        .into_iter()
        .filter(|e| e.is_dir && !e.name.starts_with('.'))
        .map(|e| e.name)
        .take(4)
        .collect();
    children.sort();
    let spec = RuleSpec {
        id: draft_id(prefix, name),
        name: Some(format!("{} folders", name)),
        bucket: Some("build".into()),
        group: Some("TODO group".into()),
        kind: Some(RuleKind::Artifact),
        tags: Some(vec!["build-output".into()]),
        stacks: Some(if stacks.is_empty() { vec!["TODO".into()] } else { stacks }),
        enabled: Some(false),
        names: Some(vec![name.to_string()]),
        parent_any: if markers.is_empty() { None } else { Some(markers) },
        children_any: if children.is_empty() { None } else { Some(children) },
        ignored: Some(true),
        cost: Some(Cost::Rebuild),
        rebuild: Some("TODO: the command that recreates it".into()),
        description: Some("TODO: what writes this folder and why it is safe to remove".into()),
        ..Default::default()
    };
    render_rule(&spec)
}

fn draft_path(path: &str, name: &str, prefix: &str, bucket: &str) -> String {
    let spec = RuleSpec {
        id: draft_id(prefix, name),
        name: Some(format!("{} cache", name.trim_start_matches('.'))),
        bucket: Some(bucket.into()),
        group: Some("TODO group".into()),
        kind: Some(RuleKind::Path),
        tags: Some(vec!["cache".into()]),
        stacks: Some(vec!["TODO".into()]),
        enabled: Some(false),
        paths: Some(vec![path.to_string()]),
        running_apps: Some(vec!["TODO: process name of the app that owns it, or remove".into()]),
        cost: Some(Cost::Rebuild),
        rebuild: Some("TODO: how it comes back".into()),
        description: Some("TODO: what keeps this and why it is safe to remove".into()),
        ..Default::default()
    };
    render_rule(&spec)
}

/// Writes drafts into a user mod (created if needed), disabled until reviewed.
pub fn write_drafts(mod_id: &str, cands: &[Candidate]) -> Result<PathBuf> {
    let dir = mods_dir().join(mod_id);
    if !dir.exists() {
        crate::ops::new_mod(mod_id)?;
    }
    let file = dir.join("suggested.toml");
    let mut text = std::fs::read_to_string(&file).unwrap_or_else(|_| {
        "# Draft rules from `poo suggest`. Each starts disabled: fill in the TODOs, check it with\n# `poo rules explain <path>`, set enabled = true, then share it with `poo mods submit`.\n".into()
    });
    for c in cands {
        if text.contains(&format!("id = \"{}\"", c.draft.lines().nth(1).and_then(|l| l.split('"').nth(1)).unwrap_or(""))) {
            continue;
        }
        text.push_str(&format!("\n# {} ({}): {}\n{}", c.path, bytes(c.bytes), c.why, c.draft));
    }
    std::fs::write(&file, text)?;
    Ok(file)
}
