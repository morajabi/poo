//! "Ask your agent": a brief with everything poo knows about an item, project, bucket or
//! plan, handed to a coding agent (terminal launchers) or an AI app (deep links), so the
//! agent can dig into what poo's static analysis cannot know.

use crate::catalog::{Catalog, Launcher};
use crate::config::{state_dir, Config};
use crate::model::{Item, Report};
use crate::util::{bytes, home, tilde};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub title: String,
    pub prompt: String,
    /// Folder an agent should start in.
    pub cwd: String,
}

const RULES: &str = "Ground rules: don't delete or move anything yourself. Inspect with read-only commands (git status, git log, git branch -vv, gh pr list --head <branch>, ls, du). If cleaning is right, tell me what and why, then use `poo clean <id> --dry-run` and only after I agree `poo clean <id>`; poo re-checks every item and backs up what removal would lose. If the poo skill isn't loaded, run `poo --skill` first.";

fn item_facts(i: &Item, cat: &Catalog) -> String {
    let rule = cat.rule(&i.rule);
    let mut s = String::new();
    s.push_str(&format!("- Path: {}\n", i.path));
    s.push_str(&format!("- What poo thinks it is: {} (rule `{}`){}\n", rule.map(|r| r.name.as_str()).unwrap_or(&i.group), i.rule, rule.map(|r| format!(": {}", r.description)).unwrap_or_default()));
    s.push_str(&format!("- Size: {}{} in {} files\n", bytes(i.bytes), if i.total_bytes > i.bytes { format!(" ({} with build output inside)", bytes(i.total_bytes)) } else { String::new() }, i.files));
    s.push_str(&format!("- Status: {} ({})\n", i.tier.label(), i.tier.describe()));
    if let Some(t) = i.last_used {
        s.push_str(&format!("- Last used: {}\n", crate::util::age(t)));
    }
    if !i.reasons.is_empty() {
        s.push_str(&format!("- Why: {}\n", i.reasons.join("; ")));
    }
    for r in &i.risks {
        s.push_str(&format!("- Note: {}\n", r.message));
    }
    if let Some(p) = &i.project {
        s.push_str(&format!("- Checkout: {}\n", p));
    }
    if let Some(r) = &i.repo {
        s.push_str(&format!("- Repository: {}\n", r));
    }
    if let Some(a) = &i.agent {
        s.push_str(&format!("- Created by agent: {}\n", a));
    }
    for (k, v) in &i.meta {
        if ["branch", "merged", "dirty", "default_branch", "workspace"].contains(&k.as_str()) {
            s.push_str(&format!("- {}: {}\n", k, v));
        }
    }
    for g in &i.safeguards {
        s.push_str(&format!("- If cleaned: {}\n", g));
    }
    s.push_str(&format!("- poo id: {} (poo show {})\n", i.id, i.id));
    s
}

pub fn for_item(i: &Item, cat: &Catalog) -> Brief {
    let what = if i.action.kind == crate::model::ActionKind::WorktreeRemove { "git worktree" } else { "folder" };
    let prompt = format!(
        "poo, my disk-space tool, found this {what} and I'm not sure what it is or whether I still need it. Help me understand it and decide.\n\nWhat poo found:\n{facts}\nPlease:\n1. Work out what this {what} is for: which project, branch or task, who created it, and when it was last really used.\n2. Check for anything unique in it that a rebuild or a fresh checkout would not bring back (uncommitted or unpushed work, untracked files, local data, config).\n3. Tell me plainly whether to keep it, clean its build output, or remove it, and what I'd lose.\n\n{RULES}",
        facts = item_facts(i, cat),
    );
    let cwd = if i.path.contains("://") {
        home().to_string_lossy().to_string()
    } else if Path::new(&i.path).is_dir() {
        i.project.clone().unwrap_or_else(|| i.path.clone())
    } else {
        Path::new(&i.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    };
    Brief { title: format!("Ask about {}", i.name), prompt, cwd }
}

pub fn for_project(r: &Report, path: &str, cat: &Catalog) -> Option<Brief> {
    let p = r.projects.iter().find(|p| p.path == path)?;
    let items: Vec<&Item> = r.items.iter().filter(|i| i.project.as_deref() == Some(path)).collect();
    let mut facts = format!(
        "- Path: {}\n- Kind: {}{}\n- Stacks: {}\n- Last activity: {}\n- Build output and dependencies: {} in {} items\n- Status: {}, policy {:?}\n",
        p.path,
        p.kind,
        p.branch.as_ref().map(|b| format!(", branch {}", b)).unwrap_or_default(),
        p.stacks.join(", "),
        p.last_activity.map(crate::util::age).unwrap_or_else(|| "unknown".into()),
        bytes(p.artifact_bytes),
        p.item_count,
        p.status,
        p.policy
    );
    if let Some(repo) = &p.repo {
        facts.push_str(&format!("- Repository: {}\n", repo));
    }
    if !p.active.is_empty() {
        facts.push_str(&format!("- Running there now: {}\n", p.active.join(", ")));
    }
    facts.push_str("\nItems poo found inside:\n");
    for i in items.iter().take(15) {
        facts.push_str(&format!("- {} {} [{}] {} (poo id {})\n", bytes(i.total_bytes), i.name, i.tier.label(), i.reasons.join("; "), i.id));
    }
    let _ = cat;
    let prompt = format!(
        "poo, my disk-space tool, summarized this project. Help me decide how to handle its disk use.\n\n{facts}\nPlease:\n1. Tell me what this project or checkout is and whether I'm still working on it (recent commits, open PRs, uncommitted work).\n2. Suggest which of these items are safe to clean now, and whether I should mark the project active, stale or protected in poo (`poo projects set <path> active|stale|protected`).\n\n{RULES}"
    );
    Some(Brief { title: format!("Ask about {}", p.name), prompt, cwd: p.path.clone() })
}

pub fn for_bucket(r: &Report, bucket: &str) -> Option<Brief> {
    let b = r.buckets.iter().find(|b| b.id == bucket)?;
    let mut facts = format!("Bucket \"{}\": {} in {} items. By status: {}\n\nBiggest items:\n", b.name, bytes(b.bytes), b.count, b.tiers.iter().map(|(k, v)| format!("{} {}", k, bytes(*v))).collect::<Vec<_>>().join(", "));
    for i in r.items.iter().filter(|i| i.bucket == bucket).take(20) {
        facts.push_str(&format!("- {} {} [{}] {} (poo id {})\n", bytes(i.total_bytes), i.name, i.tier.label(), i.reasons.join("; "), i.id));
    }
    let prompt = format!(
        "poo, my disk-space tool, grouped these under \"{name}\". Help me review them.\n\n{facts}\nPlease look at the biggest ones, tell me what each really is and which I can clean, and point out anything poo may have misjudged.\n\n{RULES}",
        name = b.name
    );
    Some(Brief { title: format!("Ask about {}", b.name), prompt, cwd: home().to_string_lossy().to_string() })
}

pub fn for_overview(r: &Report) -> Brief {
    let mut facts = format!("Disk: {} free of {}. poo found {} of dev-related data.\nPlans: {}\n\nBiggest items:\n", bytes(r.disk.free), bytes(r.disk.total), bytes(r.total_bytes), r.plans.iter().map(|p| format!("{} {}", p.id, bytes(p.bytes))).collect::<Vec<_>>().join(", "));
    for i in r.items.iter().take(25) {
        facts.push_str(&format!("- {} {} [{}] {} (poo id {})\n", bytes(i.total_bytes), i.name, i.tier.label(), i.reasons.join("; "), i.id));
    }
    let prompt = format!(
        "Help me get my disk under control with poo, my disk-space tool.\n\n{facts}\nPlease review this with what you know about my projects and workflows, run `poo` and `poo worktrees` for more, and propose what to clean now, what to keep, and any poo settings or schedules that would keep my disk healthy. Ask me what you can't infer.\n\n{RULES}"
    );
    Brief { title: "Ask about my disk".into(), prompt, cwd: home().to_string_lossy().to_string() }
}

fn item_line(i: &Item) -> String {
    format!(
        "- {} {} [{}] {}{} (poo id {})\n",
        bytes(i.total_bytes),
        i.path,
        i.tier.label(),
        i.reasons.join("; "),
        i.risks.iter().filter(|r| r.level > crate::model::RiskLevel::Standard).map(|r| format!("; note: {}", r.message)).collect::<String>(),
        i.id
    )
}

/// A group of items the user is looking at or selected.
pub fn for_items(r: &Report, ids: &[&str]) -> Option<Brief> {
    let items: Vec<&Item> = ids.iter().filter_map(|id| r.items.iter().find(|i| i.id == *id)).collect();
    if items.is_empty() {
        return None;
    }
    let total: u64 = items.iter().map(|i| i.total_bytes).sum();
    let mut facts = format!("{} items, {} in total:\n", items.len(), bytes(total));
    for i in &items {
        facts.push_str(&item_line(i));
    }
    let prompt = format!(
        "poo, my disk-space tool, listed these and I want a second opinion before cleaning. Go through all of them together, not one question at a time.\n\n{facts}\nPlease:\n1. Group them by what they really are (which project, worktree, tool or task) and check the ones poo can't judge from the filesystem: git state, open PRs, whether I still work there, anything unique inside.\n2. Give me one table: item, what it is, keep / clean build output / remove, and why.\n3. Point out anything poo may have misjudged and suggest poo settings (project policies, rules) that would get it right next time.\n\n{RULES}"
    );
    Some(Brief { title: format!("Ask about {} items", items.len()), prompt, cwd: home().to_string_lossy().to_string() })
}

pub fn for_worktrees(r: &Report) -> Brief {
    let mut facts = String::from("Linked git worktrees poo found (size includes build output inside):\n");
    for i in r.items.iter().filter(|i| i.action.kind == crate::model::ActionKind::WorktreeRemove) {
        let branch = i.meta.get("branch").and_then(|v| v.as_str()).unwrap_or("detached");
        facts.push_str(&format!(
            "- {} {} (repo {}, branch {}, agent {}) [{}] {} (poo id {})\n",
            bytes(i.total_bytes),
            i.path,
            i.repo.as_deref().unwrap_or("?"),
            branch,
            i.agent.as_deref().unwrap_or("-"),
            i.tier.label(),
            i.reasons.join("; "),
            i.id
        ));
    }
    let prompt = format!(
        "I lose track of my git worktrees; coding agents create many of them. poo, my disk-space tool, found these. Help me sort them out in one pass.\n\n{facts}\nPlease check each one's branch, recent commits, uncommitted work and PR status (gh pr list --head <branch> --state all), work out which task it was for, and give me one table: worktree, what it was for, status (merged / open PR / abandoned / active), and keep, clean build output, or remove. Run `poo worktrees` for more detail.\n\n{RULES}"
    );
    Brief { title: "Ask about my worktrees".into(), prompt, cwd: home().to_string_lossy().to_string() }
}

pub fn for_projects(r: &Report) -> Brief {
    let mut facts = String::from("Projects and checkouts poo found, by build output and dependency size:\n");
    for p in r.projects.iter().filter(|p| p.artifact_bytes > 0).take(40) {
        facts.push_str(&format!(
            "- {} {} [{}{}] stacks {}, last activity {}, status {}, policy {:?}\n",
            bytes(p.artifact_bytes),
            p.path,
            p.kind,
            p.branch.as_ref().map(|b| format!(", branch {}", b)).unwrap_or_default(),
            p.stacks.join(","),
            p.last_activity.map(crate::util::age).unwrap_or_else(|| "?".into()),
            p.status,
            p.policy
        ));
    }
    let prompt = format!(
        "poo, my disk-space tool, lists these projects. Help me tell poo which ones I actively work on, which are parked, and which hold anything precious, so its cleanups match how I work.\n\n{facts}\nUse what you know about me and the repos (recent commits, open PRs). Propose `poo projects set <path> active|stale|protected` commands for each, and ask me only about the unclear ones.\n\n{RULES}"
    );
    Brief { title: "Ask about my projects".into(), prompt, cwd: home().to_string_lossy().to_string() }
}

/// Resolves `overview`, `worktrees`, `projects`, `items:<ids>`, `bucket:<id>`,
/// `project:<path>`, an item id or a path.
pub fn brief(r: &Report, cat: &Catalog, target: &str) -> Result<Brief> {
    if target == "overview" || target.is_empty() {
        return Ok(for_overview(r));
    }
    if target == "worktrees" {
        return Ok(for_worktrees(r));
    }
    if target == "projects" {
        return Ok(for_projects(r));
    }
    if let Some(list) = target.strip_prefix("items:") {
        let ids: Vec<&str> = list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        return for_items(r, &ids).ok_or_else(|| anyhow!("none of those ids are in the last scan"));
    }
    if let Some(b) = target.strip_prefix("bucket:") {
        return for_bucket(r, b).ok_or_else(|| anyhow!("no bucket `{}` in the last scan", b));
    }
    let raw = target.strip_prefix("project:").unwrap_or(target);
    let expanded = crate::util::expand(raw);
    let canon = std::fs::canonicalize(&expanded).unwrap_or(expanded).to_string_lossy().to_string();
    if let Some(i) = crate::ops::find_item(r, raw) {
        return Ok(for_item(i, cat));
    }
    if let Some(b) = for_project(r, &canon, cat) {
        return Ok(b);
    }
    bail!("`{}` is not an item id, project path, bucket:<id> or overview", target)
}

pub fn launchers(cat: &Catalog) -> Vec<(Launcher, bool)> {
    cat.launchers.iter().map(|l| (l.clone(), l.available())).collect()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Opens the brief with a launcher. Terminal launchers get a .command file so no
/// automation permission is needed; URL launchers get a deep link.
pub fn launch(cfg: &Config, l: &Launcher, b: &Brief) -> Result<String> {
    match l.kind.as_str() {
        "terminal" => {
            let dir = state_dir().join("launch");
            std::fs::create_dir_all(&dir)?;
            // Keep only recent launch scripts.
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    if crate::fsutil::mtime(&e.path()).map_or(false, |t| crate::util::now() - t > 86400) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let script: PathBuf = dir.join(format!("{}-{}.command", l.id, crate::util::now()));
            let args: Vec<String> = l.command.iter().map(|a| shell_quote(&a.replace("{prompt}", &b.prompt).replace("{cwd}", &b.cwd))).collect();
            let text = format!(
                "#!/bin/zsh -il\n# Started by poo: {}\ncd {} || exit 1\nclear\nexec {}\n",
                b.title.replace('\n', " "),
                shell_quote(&b.cwd),
                args.join(" ")
            );
            std::fs::write(&script, text)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
            let status = std::process::Command::new("open").args(["-a", &cfg.agents.terminal_app, &script.to_string_lossy()]).status()?;
            if !status.success() {
                bail!("could not open {} (set agents.terminal_app)", cfg.agents.terminal_app);
            }
            Ok(format!("opened {} in {} at {}", l.name, cfg.agents.terminal_app, tilde(Path::new(&b.cwd))))
        }
        "url" => {
            let tpl = l.url.clone().ok_or_else(|| anyhow!("launcher {} has no url", l.id))?;
            let url = tpl.replace("{prompt}", &percent_encode(&b.prompt)).replace("{cwd}", &percent_encode(&b.cwd));
            let status = std::process::Command::new("open").arg(&url).status()?;
            if !status.success() {
                bail!("could not open {}", l.name);
            }
            Ok(format!("opened {}", l.name))
        }
        k => bail!("unknown launcher kind `{}`", k),
    }
}
