//! Opt-in agent hooks: when a coding agent finishes in a linked worktree, clean that
//! worktree's build output (and optionally the worktree once its work landed).

use crate::clean::{self, CleanOptions};
use crate::config::state_dir;
use crate::model::{ActionKind, RiskLevel};
use crate::ops::Ctx;
use crate::scan::ScanOptions;
use crate::util::{bytes, home, now, tilde, which};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MARK: &str = " hook agent-done";

#[derive(Debug, Clone, Serialize)]
pub struct HookTarget {
    pub id: &'static str,
    pub name: &'static str,
    pub detected: bool,
    pub installed: bool,
    pub event: &'static str,
    pub file: String,
}

fn claude_settings() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude")).join("settings.json")
}

fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

fn read_json(p: &Path) -> Result<Value> {
    match std::fs::read_to_string(p) {
        Ok(t) if !t.trim().is_empty() => serde_json::from_str(&t).with_context(|| format!("{} is not valid JSON", p.display())),
        _ => Ok(json!({})),
    }
}

fn write_atomic(p: &Path, text: &str) -> Result<()> {
    let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    std::fs::create_dir_all(real.parent().unwrap())?;
    let tmp = real.with_file_name(format!(".poo-hooks-{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Ok(md) = std::fs::metadata(&real) {
        let _ = std::fs::set_permissions(&tmp, md.permissions());
    }
    std::fs::rename(tmp, real)?;
    Ok(())
}

fn has_ours(v: &Value, event: &str) -> bool {
    v.get("hooks").and_then(|h| h.get(event)).and_then(|e| e.as_array()).map_or(false, |entries| {
        entries.iter().any(|e| {
            e.get("hooks").and_then(|h| h.as_array()).map_or(false, |hs| {
                hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains(MARK)))
            })
        })
    })
}

fn remove_ours(v: &mut Value, event: &str) {
    if let Some(entries) = v.get_mut("hooks").and_then(|h| h.get_mut(event)).and_then(|e| e.as_array_mut()) {
        for e in entries.iter_mut() {
            if let Some(hs) = e.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                hs.retain(|h| !h.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains(MARK)));
            }
        }
        entries.retain(|e| e.get("hooks").and_then(|h| h.as_array()).map_or(true, |hs| !hs.is_empty()));
    }
    // Leave no empty event arrays behind.
    if let Some(h) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        if h.get(event).and_then(|e| e.as_array()).map_or(false, |a| a.is_empty()) {
            h.remove(event);
        }
    }
}

fn add_ours(v: &mut Value, event: &str, command: String) {
    remove_ours(v, event);
    if !v.get("hooks").map_or(false, |h| h.is_object()) {
        v["hooks"] = json!({});
    }
    let hooks = v["hooks"].as_object_mut().unwrap();
    let entries = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    if let Some(arr) = entries.as_array_mut() {
        arr.push(json!({"hooks": [{"type": "command", "command": command, "timeout": 10}]}));
    }
}

fn command(agent: &str) -> String {
    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "poo".into());
    format!("\"{}\"{} --agent {}", bin, MARK, agent)
}

pub fn status() -> Vec<HookTarget> {
    let cs = claude_settings();
    let ch = codex_dir().join("hooks.json");
    vec![
        HookTarget {
            id: "claude",
            name: "Claude Code",
            detected: cs.parent().map_or(false, |p| p.is_dir()) || which("claude").is_some(),
            installed: read_json(&cs).map_or(false, |v| has_ours(&v, "SessionEnd")),
            event: "SessionEnd",
            file: tilde(&cs),
        },
        HookTarget {
            id: "codex",
            name: "Codex",
            detected: codex_dir().is_dir() || which("codex").is_some(),
            installed: read_json(&ch).map_or(false, |v| has_ours(&v, "Stop")),
            event: "Stop",
            file: tilde(&ch),
        },
    ]
}

pub fn install(id: &str) -> Result<String> {
    if crate::clean::read_only() {
        return Ok(format!("POO_READ_ONLY set: would install the {} hook; nothing was written", id));
    }
    match id {
        "claude" => {
            let p = claude_settings();
            let mut v = read_json(&p)?;
            add_ours(&mut v, "SessionEnd", command("claude"));
            write_atomic(&p, &(serde_json::to_string_pretty(&v)? + "\n"))?;
            Ok(format!("Claude Code: SessionEnd hook in {}", tilde(&p)))
        }
        "codex" => {
            let p = codex_dir().join("hooks.json");
            let mut v = read_json(&p)?;
            add_ours(&mut v, "Stop", command("codex"));
            write_atomic(&p, &(serde_json::to_string_pretty(&v)? + "\n"))?;
            // Codex runs hooks only with the feature on.
            let cfgp = codex_dir().join("config.toml");
            let text = std::fs::read_to_string(&cfgp).unwrap_or_default();
            if !text.lines().any(|l| l.trim_start().starts_with("hooks") && l.contains("true")) {
                let new = if let Some(i) = text.find("[features]") {
                    let insert_at = i + "[features]".len();
                    format!("{}\nhooks = true{}", &text[..insert_at], &text[insert_at..])
                } else {
                    format!("{}{}[features]\nhooks = true\n", text, if text.is_empty() || text.ends_with('\n') { "" } else { "\n" })
                };
                write_atomic(&cfgp, &new)?;
            }
            Ok(format!("Codex: Stop hook in {} (features.hooks on in config.toml)", tilde(&p)))
        }
        _ => Err(anyhow!("unknown agent `{}` (claude, codex)", id)),
    }
}

pub fn uninstall(id: &str) -> Result<String> {
    if crate::clean::read_only() {
        return Ok(format!("POO_READ_ONLY set: would remove the {} hook; nothing was written", id));
    }
    let (p, event) = match id {
        "claude" => (claude_settings(), "SessionEnd"),
        "codex" => (codex_dir().join("hooks.json"), "Stop"),
        _ => return Err(anyhow!("unknown agent `{}`", id)),
    };
    if !p.exists() {
        return Ok(format!("{}: nothing installed", id));
    }
    let mut v = read_json(&p)?;
    remove_ours(&mut v, event);
    write_atomic(&p, &(serde_json::to_string_pretty(&v)? + "\n"))?;
    Ok(format!("{}: removed poo's {} hook from {}", id, event, tilde(&p)))
}

fn log(line: &str) {
    let p = state_dir().join("hooks.log");
    let _ = std::fs::create_dir_all(state_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{} {}", crate::util::iso8601(now()), line);
    }
}

/// How long a hook waits for another clean to finish before it gives up on its worktree.
const HOOK_WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Called by the agent. Returns at once; the work happens in a detached poo.
pub fn agent_done(agent: &str, cwd: Option<String>, detached: bool) -> Result<()> {
    if !detached {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let payload: Value = serde_json::from_str(&input).unwrap_or(json!({}));
        let dir = cwd.or_else(|| payload.get("cwd").and_then(|v| v.as_str()).map(String::from)).unwrap_or_default();
        if dir.is_empty() {
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe)
            .args(["hook", "agent-done", "--agent", agent, "--cwd", &dir, "--detached"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(());
    }
    let ctx = Ctx::load()?;
    let mode = ctx.cfg.hooks.on_agent_done.clone();
    if mode == "off" {
        return Ok(());
    }
    let cwd = PathBuf::from(cwd.unwrap_or_default());
    // Innermost checkout; only linked worktrees (a .git file), never a main checkout.
    let mut wt = None;
    let mut cur = Some(cwd.as_path());
    while let Some(d) = cur {
        let g = d.join(".git");
        if g.is_file() {
            wt = Some(d.to_path_buf());
            break;
        }
        if g.is_dir() {
            break;
        }
        cur = d.parent();
    }
    let Some(wt) = wt else { return Ok(()) };
    let repo = crate::providers::git::git_dir(&wt)
        .and_then(|gd| std::fs::read_to_string(gd.join("commondir")).ok().map(|c| {
            let c = PathBuf::from(c.trim());
            if c.is_absolute() { c } else { gd.join(c) }
        }))
        .and_then(|c| std::fs::canonicalize(c).ok())
        .and_then(|c| c.parent().map(|p| p.to_path_buf()))
        .ok_or_else(|| anyhow!("could not find the repository of {}", wt.display()))?;
    let default = crate::providers::git::default_branch(&repo).unwrap_or_else(|| "main".into());
    let landed = crate::providers::git::is_merged(&repo, "HEAD", &default) || {
        let head = crate::providers::git::git(&wt, &["rev-parse", "HEAD"], 5).unwrap_or_default();
        !head.trim().is_empty() && crate::providers::git::is_merged(&repo, head.trim(), &default)
    };
    // Codex fires Stop after every turn, so only landed work is cleaned for it.
    let allowed = landed || (mode == "always" && agent == "claude");
    if !allowed {
        log(&format!("{} {}: work not landed yet; left alone", agent, tilde(&wt)));
        return Ok(());
    }
    // Agents often finish together (parallel sessions, a batch of worktrees), and only one
    // clean runs at a time. Nothing retries a hook, so wait for the clean ahead of this one:
    // this process is detached, and the agent is not waiting on it.
    let lock = crate::schedule::RunLock::acquire_wait(&format!("hook:{}", agent), HOOK_WAIT, &|_| {}, &|| false);
    let _lock = match lock {
        Ok(l) => l,
        Err(busy) => {
            log(&format!("{} {}: {}; skipped", agent, tilde(&wt), busy.message()));
            return Ok(());
        }
    };
    let wt = std::fs::canonicalize(&wt).unwrap_or(wt);
    let report = crate::scan::scan(
        &ctx.cfg,
        &ctx.cat,
        &ScanOptions { roots: Some(vec![wt.clone()]), no_providers: true, roots_only: true, full: false, on_progress: None },
    );
    let prefix = format!("{}/", wt.to_string_lossy());
    let items: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.path.starts_with(&prefix) || (i.path == wt.to_string_lossy() && i.action.kind == ActionKind::WorktreeRemove))
        .filter(|i| {
            if i.action.kind == ActionKind::WorktreeRemove {
                ctx.cfg.hooks.remove_landed_worktrees && landed && !i.risks.iter().any(|r| r.tag == "dirty")
            } else {
                i.bucket != "other" && i.tier != crate::model::Tier::Protected
            }
        })
        .cloned()
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    // The agent just finished: allow "used recently" and "agent open", never running builds.
    let opts = CleanOptions::from_config(&ctx.cfg, RiskLevel::Force, false, &format!("hook:{}", agent));
    let res = clean::clean(&items, &ctx.cat, &opts);
    let label = wt.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    log(&format!("{} {}: freed {} ({} cleaned, {} skipped)", agent, tilde(&wt), bytes(res.freed_bytes), res.cleaned, res.skipped));
    if let Some(mut cached) = crate::scan::load_report() {
        crate::ops::forget_cleaned(&ctx, &mut cached, &res);
    }
    if ctx.cfg.hooks.notify && res.freed_bytes > 0 {
        let why = if landed { format!("its work is in {}", default.trim_start_matches("origin/")) } else { "the agent session ended".to_string() };
        crate::schedule::notify("poo", &format!("Freed {} of build output in {} ({}).", bytes(res.freed_bytes), label, why));
    }
    Ok(())
}
