//! Agent onboarding: the skill, MCP registration for coding agents, and the
//! setup brief an agent follows to configure poo for its user.

use crate::catalog::Catalog;
use crate::config::Config;
use crate::model::{Report, RuleKind, Tier};
use crate::util::{bytes, home, tilde, which};
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const SKILL: &str = include_str!("skill.md");
pub const MODS_DOCS: &str = include_str!("../docs/mods.md");
/// Bump when the installed files change shape.
pub const INTEGRATION_VERSION: u32 = 1;

pub fn skill_stub() -> String {
    format!(
        "---\nname: poo\ndescription: Inspect and safely free disk space on a developer Mac with the poo CLI. Use when the user is low on disk space, asks what takes space, wants to clean builds, caches, DerivedData, simulators, Docker or agent worktrees, or asks to configure poo.\n---\n\n<!-- managed by poo; `poo integration install` overwrites this file. POO_INTEGRATION_VERSION={} -->\n\n# poo\n\nBefore using poo, run `poo --skill` and follow it. The installed binary is the\nauthority for commands and flags.\n\nNever delete files yourself; clean through `poo clean` or `poo free`, which\nre-check every item and refuse uncommitted worktrees and running builds.\n",
        INTEGRATION_VERSION
    )
}

fn poo_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "poo".into())
}

fn env_dir(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home().join(default))
}

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub id: &'static str,
    pub name: &'static str,
    pub detected: bool,
    pub skill_path: Option<String>,
    pub skill_installed: bool,
    pub skill_current: bool,
    pub mcp_config: Option<String>,
    pub mcp_registered: bool,
}

struct Spec {
    id: &'static str,
    name: &'static str,
    bin: &'static str,
    dir: PathBuf,
    skill_dir: Option<PathBuf>,
    mcp: McpKind,
}

enum McpKind {
    ClaudeCli,
    CodexToml(PathBuf),
    JsonServers(PathBuf),
    OpenCode(PathBuf),
    None,
}

fn specs() -> Vec<Spec> {
    let claude = env_dir("CLAUDE_CONFIG_DIR", ".claude");
    let codex = env_dir("CODEX_HOME", ".codex");
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    vec![
        Spec { id: "claude", name: "Claude Code", bin: "claude", dir: claude.clone(), skill_dir: Some(claude.join("skills/poo")), mcp: McpKind::ClaudeCli },
        Spec { id: "codex", name: "Codex", bin: "codex", dir: codex.clone(), skill_dir: Some(codex.join("skills/poo")), mcp: McpKind::CodexToml(codex.join("config.toml")) },
        Spec { id: "cursor", name: "Cursor", bin: "cursor-agent", dir: home().join(".cursor"), skill_dir: Some(home().join(".cursor/skills/poo")), mcp: McpKind::JsonServers(home().join(".cursor/mcp.json")) },
        Spec { id: "opencode", name: "OpenCode", bin: "opencode", dir: xdg.join("opencode"), skill_dir: Some(xdg.join("opencode/skills/poo")), mcp: McpKind::OpenCode(xdg.join("opencode/opencode.json")) },
        Spec { id: "gemini", name: "Gemini CLI", bin: "gemini", dir: home().join(".gemini"), skill_dir: None, mcp: McpKind::JsonServers(home().join(".gemini/settings.json")) },
        Spec { id: "windsurf", name: "Windsurf", bin: "windsurf", dir: home().join(".codeium/windsurf"), skill_dir: None, mcp: McpKind::JsonServers(home().join(".codeium/windsurf/mcp_config.json")) },
        Spec { id: "claude-desktop", name: "Claude desktop app", bin: "", dir: home().join("Library/Application Support/Claude"), skill_dir: None, mcp: McpKind::JsonServers(home().join("Library/Application Support/Claude/claude_desktop_config.json")) },
        Spec { id: "agents", name: "Shared skills folder (~/.agents)", bin: "", dir: home().join(".agents"), skill_dir: Some(home().join(".agents/skills/poo")), mcp: McpKind::None },
    ]
}

fn mcp_registered(spec: &Spec) -> bool {
    match &spec.mcp {
        McpKind::ClaudeCli => {
            let f = home().join(".claude.json");
            std::fs::read_to_string(f).map(|t| t.contains("\"poo\"")).unwrap_or(false)
        }
        McpKind::CodexToml(p) => std::fs::read_to_string(p).map(|t| t.contains("[mcp_servers.poo]")).unwrap_or(false),
        McpKind::JsonServers(p) | McpKind::OpenCode(p) => std::fs::read_to_string(p).map(|t| t.contains("\"poo\"")).unwrap_or(false),
        McpKind::None => false,
    }
}

fn mcp_path(spec: &Spec) -> Option<String> {
    match &spec.mcp {
        McpKind::ClaudeCli => Some("~/.claude.json (via `claude mcp add`)".into()),
        McpKind::CodexToml(p) | McpKind::JsonServers(p) | McpKind::OpenCode(p) => Some(tilde(p)),
        McpKind::None => None,
    }
}

pub fn status() -> Vec<Target> {
    specs()
        .into_iter()
        .map(|s| {
            let skill_file = s.skill_dir.as_ref().map(|d| d.join("SKILL.md"));
            let text = skill_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
            Target {
                id: s.id,
                name: s.name,
                detected: s.dir.is_dir() || (!s.bin.is_empty() && which(s.bin).is_some()),
                skill_path: skill_file.as_ref().map(|f| tilde(f)),
                skill_installed: text.is_some(),
                skill_current: text
                    .map(|t| t.contains(&format!("POO_INTEGRATION_VERSION={}", INTEGRATION_VERSION)))
                    .unwrap_or(false),
                mcp_config: mcp_path(&s),
                mcp_registered: mcp_registered(&s),
            }
        })
        .collect()
}

/// Atomic write that refuses hard-linked files and keeps permissions.
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Ok(md) = std::fs::metadata(path) {
        use std::os::unix::fs::MetadataExt;
        if md.nlink() > 1 {
            anyhow::bail!("{} is hard-linked; not touching it", path.display());
        }
        if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
            return Ok(());
        }
    }
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    std::fs::create_dir_all(real.parent().unwrap())?;
    let tmp = real.with_file_name(format!(".poo-config-{}.tmp", std::process::id()));
    std::fs::write(&tmp, content)?;
    if let Ok(md) = std::fs::metadata(&real) {
        let _ = std::fs::set_permissions(&tmp, md.permissions());
    }
    std::fs::rename(&tmp, &real)?;
    Ok(())
}

fn merge_json(path: &Path, f: impl FnOnce(&mut serde_json::Value)) -> Result<()> {
    let mut v: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(t) if !t.trim().is_empty() => serde_json::from_str(&t).with_context(|| format!("{} is not valid JSON", path.display()))?,
        _ => serde_json::json!({}),
    };
    f(&mut v);
    write_atomic(path, &(serde_json::to_string_pretty(&v)? + "\n"))
}

pub fn install(id: &str, skill: bool, mcp: bool) -> Result<Vec<String>> {
    let spec = specs().into_iter().find(|s| s.id == id).with_context(|| format!("unknown agent `{}`", id))?;
    let mut done = vec![];
    let bin = poo_bin();
    if skill {
        if let Some(dir) = &spec.skill_dir {
            write_atomic(&dir.join("SKILL.md"), &skill_stub())?;
            done.push(format!("skill → {}", tilde(&dir.join("SKILL.md"))));
        }
    }
    if mcp {
        match &spec.mcp {
            McpKind::ClaudeCli => {
                if which("claude").is_some() {
                    let _ = std::process::Command::new("claude").args(["mcp", "remove", "--scope", "user", "poo"]).output();
                    let out = std::process::Command::new("claude")
                        .args(["mcp", "add", "--scope", "user", "poo", "--", &bin, "mcp"])
                        .output()?;
                    if !out.status.success() {
                        anyhow::bail!("claude mcp add failed: {}", String::from_utf8_lossy(&out.stderr).trim());
                    }
                    done.push("mcp → claude mcp add --scope user poo".into());
                } else {
                    done.push(format!("mcp: `claude` not on PATH; run: claude mcp add --scope user poo -- {} mcp", bin));
                }
            }
            McpKind::CodexToml(p) => {
                let text = std::fs::read_to_string(p).unwrap_or_default();
                if !text.contains("[mcp_servers.poo]") {
                    let add = format!(
                        "{}\n# managed by poo (POO_INTEGRATION_VERSION={})\n[mcp_servers.poo]\ncommand = {:?}\nargs = [\"mcp\"]\n",
                        if text.is_empty() || text.ends_with('\n') { "" } else { "\n" },
                        INTEGRATION_VERSION,
                        bin
                    );
                    write_atomic(p, &(text + &add))?;
                }
                done.push(format!("mcp → {}", tilde(p)));
            }
            McpKind::JsonServers(p) => {
                let b = bin.clone();
                merge_json(p, |v| {
                    v["mcpServers"]["poo"] = serde_json::json!({"command": b, "args": ["mcp"]});
                })?;
                done.push(format!("mcp → {}", tilde(p)));
            }
            McpKind::OpenCode(p) => {
                let b = bin.clone();
                merge_json(p, |v| {
                    v["mcp"]["poo"] = serde_json::json!({"type": "local", "command": [b, "mcp"], "enabled": true});
                })?;
                done.push(format!("mcp → {}", tilde(p)));
            }
            McpKind::None => {}
        }
    }
    Ok(done)
}

pub fn uninstall(id: &str) -> Result<Vec<String>> {
    let spec = specs().into_iter().find(|s| s.id == id).with_context(|| format!("unknown agent `{}`", id))?;
    let mut done = vec![];
    if let Some(dir) = &spec.skill_dir {
        let f = dir.join("SKILL.md");
        if std::fs::read_to_string(&f).map(|t| t.contains("managed by poo")).unwrap_or(false) {
            std::fs::remove_file(&f)?;
            let _ = std::fs::remove_dir(dir);
            done.push(format!("removed {}", tilde(&f)));
        }
    }
    match &spec.mcp {
        McpKind::ClaudeCli if which("claude").is_some() => {
            let _ = std::process::Command::new("claude").args(["mcp", "remove", "--scope", "user", "poo"]).output();
            done.push("claude mcp remove poo".into());
        }
        McpKind::JsonServers(p) if p.exists() => {
            merge_json(p, |v| {
                if let Some(m) = v.get_mut("mcpServers").and_then(|m| m.as_object_mut()) {
                    m.remove("poo");
                }
            })?;
            done.push(format!("removed poo from {}", tilde(p)));
        }
        McpKind::OpenCode(p) if p.exists() => {
            merge_json(p, |v| {
                if let Some(m) = v.get_mut("mcp").and_then(|m| m.as_object_mut()) {
                    m.remove("poo");
                }
            })?;
            done.push(format!("removed poo from {}", tilde(p)));
        }
        McpKind::CodexToml(p) => {
            done.push(format!("edit {} and remove [mcp_servers.poo]", tilde(p)));
        }
        _ => {}
    }
    Ok(done)
}

/// The brief an agent follows to configure poo for its user.
pub fn setup_brief(cfg: &Config, cat: &Catalog, r: &Report) -> String {
    let mut s = String::new();
    let push = |s: &mut String, line: String| {
        s.push_str(&line);
        s.push('\n');
    };
    push(&mut s, "# poo setup brief".into());
    push(&mut s, String::new());
    push(&mut s, "You are configuring poo, a disk-space tool for developers, for your user. Your goal: poo's buckets, tiers and plans should match how this user actually works, so they (and you) can free space without regret. Read all of this, combine it with what you already know about the user from memory and context, then propose a configuration, ask only what you cannot infer, and apply it with the commands at the end.".into());
    push(&mut s, String::new());

    push(&mut s, "## Machine".into());
    push(&mut s, format!("- Disk: {} free of {} ({:.0}% used).", bytes(r.disk.free), bytes(r.disk.total), r.disk.used as f64 * 100.0 / r.disk.total.max(1) as f64));
    push(&mut s, format!("- poo found {} in {} items. Last scan {}.", bytes(r.total_bytes), r.items.len(), crate::util::age(r.scanned_at)));
    let (roots, detected) = cfg.effective_roots();
    push(&mut s, format!("- Dev roots{}: {}", if detected { " (auto-detected, not confirmed)" } else { "" }, roots.iter().map(|p| tilde(p)).collect::<Vec<_>>().join(", ")));
    let homes: Vec<String> = cat.worktree_homes.iter().filter(|h| crate::util::expand(&h.path).is_dir()).map(|h| format!("{} ({})", h.path, h.agent)).collect();
    push(&mut s, format!("- Agent worktree homes present: {}", if homes.is_empty() { "none".into() } else { homes.join(", ") }));
    let agents: Vec<String> = status().into_iter().filter(|t| t.detected).map(|t| format!("{}{}", t.name, if t.mcp_registered { " (poo MCP registered)" } else { "" })).collect();
    push(&mut s, format!("- Coding agents installed: {}", agents.join(", ")));
    push(&mut s, format!("- Setup completed before: {}", cfg.setup.completed_at.map(crate::util::iso8601).unwrap_or_else(|| "no".into())));
    push(&mut s, String::new());

    push(&mut s, "## Where the space is".into());
    for b in r.buckets.iter().filter(|b| b.count > 0) {
        let tiers: Vec<String> = Tier::all().iter().filter_map(|t| b.tiers.get(t.as_str()).map(|v| format!("{} {}", t.as_str(), bytes(*v)))).collect();
        push(&mut s, format!("- **{}** `{}`: {} ({}). Top groups: {}", b.name, b.id, bytes(b.bytes), tiers.join(", "), b.groups.iter().take(4).map(|g| format!("{} {}", g.name, bytes(g.bytes))).collect::<Vec<_>>().join(", ")));
    }
    push(&mut s, String::new());
    push(&mut s, "Plans right now:".into());
    for p in &r.plans {
        push(&mut s, format!("- `{}` {}: {} in {} items. {}", p.id, p.name, bytes(p.bytes), p.count, p.description));
    }
    push(&mut s, String::new());

    push(&mut s, "## Projects and duplication".into());
    let mut repos: std::collections::BTreeMap<String, (u64, usize, usize)> = Default::default();
    for p in &r.projects {
        let key = p.repo.clone().unwrap_or_else(|| p.path.clone());
        let e = repos.entry(key).or_default();
        e.0 += p.artifact_bytes;
        e.1 += 1;
        if p.kind == "worktree" {
            e.2 += 1;
        }
    }
    let mut repos: Vec<_> = repos.into_iter().collect();
    repos.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    push(&mut s, "Repos by build output and dependencies across all their checkouts:".into());
    for (repo, (b, n, wt)) in repos.iter().take(12) {
        push(&mut s, format!("- {}: {} across {} checkout(s), {} linked worktree(s)", tilde(Path::new(repo)), bytes(*b), n, wt));
    }
    push(&mut s, String::new());
    push(&mut s, "Projects (status from git activity; policy from config):".into());
    for p in r.projects.iter().take(25) {
        push(&mut s, format!("- {} [{}] {} {}, status {}, policy {:?}, stacks {}", tilde(Path::new(&p.path)), p.kind, bytes(p.artifact_bytes), p.agent.as_deref().map(|a| format!("(agent {})", a)).unwrap_or_default(), p.status, p.policy, p.stacks.join(",")).to_lowercase());
    }
    push(&mut s, String::new());

    let unknown: Vec<_> = r.items.iter().filter(|i| i.bucket == "other").take(15).collect();
    if !unknown.is_empty() {
        push(&mut s, "## Folders poo cannot explain yet (candidates for new rules)".into());
        for i in unknown {
            push(&mut s, format!("- {} {} ({})", bytes(i.bytes), tilde(Path::new(&i.path)), i.id));
        }
        push(&mut s, String::new());
    }

    push(&mut s, "## Everything poo knows (catalog)".into());
    push(&mut s, "Use this to widen your view beyond what you already know about the user's setup. Rules marked (off) are disabled.".into());
    for b in &cat.buckets {
        let rules: Vec<_> = cat.rules.iter().filter(|r| r.bucket == b.id).collect();
        if rules.is_empty() {
            continue;
        }
        push(&mut s, format!("### {} `{}`", b.name, b.id));
        for rule in rules {
            let kind = match rule.kind {
                RuleKind::Artifact => format!("dirs named {}", rule.names.raw.join("/")),
                RuleKind::Path => rule.paths.first().cloned().unwrap_or_default(),
                RuleKind::Provider => format!("provider {}", rule.provider.clone().unwrap_or_default()),
            };
            push(&mut s, format!("- `{}`{} {} [{}{}] {}{}", rule.id, if rule.enabled { "" } else { " (off)" }, rule.name, rule.cost.as_str(), rule.tier.map(|t| format!(", {}", t.as_str())).unwrap_or_default(), kind, rule.recommend.as_ref().map(|r| format!(". Tip: {}", r)).unwrap_or_default()));
        }
    }
    push(&mut s, String::new());
    push(&mut s, format!("Mods loaded: {}", cat.mods.iter().map(|m| format!("{} ({} rules{})", m.id, m.rules, if m.enabled { "" } else { ", disabled" })).collect::<Vec<_>>().join(", ")));
    push(&mut s, String::new());

    push(&mut s, "## What to decide with the user".into());
    for q in [
        "Dev roots: which folders hold their projects? Confirm or fix the detected roots (`poo roots add/rm`).",
        "Active work: which projects and worktrees are in flight right now? Mark them `active` so their builds stay warm (`poo projects set PATH active`).",
        "Dormant work: which projects are finished or parked? Mark them `stale` so their build output and dependencies count as cold.",
        "Precious folders: anything poo must never touch (generated data they cannot recreate, fixtures, large datasets)? Mark the project `protected` or disable the rule.",
        "Worktree habits: do agents create worktrees per task? How soon after merging are those disposable? Tune `git.worktrees` warm_days (default 7).",
        "Rebuild tolerance: how painful is a cold rebuild for their main stacks (Xcode, Rust, Node)? Raise warm_days for expensive stacks, lower it for cheap ones.",
        "Simulators and runtimes: which OS versions do they still test on?",
        "AI history: keep agent transcripts and editor chat history, or allow cleaning old ones?",
        "Autonomy: may you clean junk and cold items without asking each time (`agents.auto_clean_max_tier = cold`), only junk, or nothing?",
        "Buckets: does the default grouping match how they think? Rename, merge, reorder or hide buckets.",
    ] {
        push(&mut s, format!("- {}", q));
    }
    push(&mut s, String::new());
    push(&mut s, "Infer what you can from memory, their repos, CLAUDE.md/AGENTS.md conventions (where builds go, how worktrees are named) and the data above. Ask the rest in one short message. Suggest structural fixes too, for example keeping each worktree's build outputs under one build/ folder, or Rust `[profile.dev] debug = \"line-tables-only\"`.".into());
    push(&mut s, String::new());

    push(&mut s, "## Commands to apply it".into());
    for c in [
        "poo roots add ~/work            poo roots rm ~/old",
        "poo projects set ~/dev/app active|stale|protected|auto",
        "poo rules set rust.target warm_days=7     poo rules set git.worktrees warm_days=3",
        "poo rules disable system.app-caches       poo rules enable system.sandbox-caches",
        "poo buckets set build --name \"Builds\" --order 5     poo buckets add work --name \"Work repos\" --color \"#FF9500\"",
        "poo rules set rust.target bucket=work    (move a rule to another bucket)",
        "poo config set agents.auto_clean_max_tier cold",
        "poo mods new myteam     (then edit ~/.config/poo/mods/myteam/rules.toml; poo mods validate)",
        "poo scan                (rescan to see the effect)",
        "poo setup --done --by <agent name> --notes \"what you configured and why\"",
    ] {
        push(&mut s, format!("    {}", c));
    }
    push(&mut s, String::new());
    push(&mut s, "Finish by summarizing for the user what you configured, how much each plan frees now, and what you recommend cleaning first.".into());
    s
}
