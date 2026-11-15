//! CLI side of sharing rules: `poo mods lint`, `poo mods submit`, `poo suggest`.

use super::{emit, CliError, Out};
use poo::contrib::{self, Level, SubmitOptions};
use poo::ops::Ctx;
use poo::util::{bytes, is_tty, parse_bytes};
use serde_json::json;
use std::io::Write;

fn print_findings(findings: &[contrib::Finding]) {
    for x in findings {
        let mark = if x.level == Level::Error { "✗" } else { "!" };
        println!("  {} {:<34} {}", mark, x.rule, x.message);
    }
}

pub fn lint(ctx: &Ctx, out: &Out, core: bool, base: Option<String>, repo: Option<String>) -> Result<(), CliError> {
    let (rules, new_rules, findings) = if core {
        let repo = std::path::PathBuf::from(repo.unwrap_or_else(|| ".".into()));
        let r = contrib::lint_core(&ctx.cat, &repo, base.as_deref())?;
        (r.rules, r.new_rules, r.findings)
    } else {
        let mut findings = vec![];
        let user = contrib::user_rules();
        for u in &user {
            let in_core = ctx.cat.rule(&u.spec.id).map_or(false, |r| r.source.split('+').next() == Some("core"));
            findings.extend(contrib::lint_spec(&u.spec, &ctx.cat, !in_core, true));
        }
        (user.len(), vec![], findings)
    };
    let mut findings = findings;
    findings.extend(ctx.cat.errors.iter().map(|e| contrib::Finding { level: Level::Error, rule: "validate".into(), message: e.clone() }));
    let errors = findings.iter().filter(|x| x.level == Level::Error).count();
    if out.json {
        emit(out, &json!({"rules": rules, "new_rules": new_rules, "errors": errors, "findings": findings}));
    } else {
        println!("Linted {} {}rules{}.", rules, if core { "core " } else { "mod " }, if new_rules.is_empty() { String::new() } else { format!(", {} new", new_rules.len()) });
        print_findings(&findings);
        if findings.is_empty() {
            println!("  ✓ no findings");
        }
    }
    if errors > 0 {
        return Err(CliError::new("lint_failed", 8, format!("{} lint error(s)", errors)));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn submit(
    ctx: &Ctx,
    out: &Out,
    ids: &[String],
    yes: bool,
    dry_run: bool,
    evidence: bool,
    tools: Vec<String>,
    note: Option<String>,
    no_gh: bool,
) -> Result<(), CliError> {
    let report = poo::scan::load_report();
    let opts = SubmitOptions { evidence, tools, note };
    let sub = contrib::prepare(&ctx.cfg, &ctx.cat, report.as_ref(), ids, &opts)?;
    let use_gh = !no_gh && poo::util::which("gh").is_some();
    let interactive = !out.json && is_tty(0) && is_tty(1);

    if out.json {
        let links = if use_gh { vec![] } else { contrib::fallback_urls(&sub) };
        if sub.blocked() || dry_run || !yes {
            emit(
                out,
                &json!({
                    "submission": sub,
                    "sent": false,
                    "would_use": if use_gh { "gh" } else { "links" },
                    "links": links.iter().map(|(l, u)| json!({"label": l, "url": u})).collect::<Vec<_>>(),
                    "next": if sub.blocked() { "fix the lint errors, then run again" } else { "show the person the files and PR text; with their yes, run again with --yes" },
                }),
            );
            return if sub.blocked() { Err(CliError::new("lint_failed", 8, "lint errors block the submission")) } else { Ok(()) };
        }
    } else {
        println!("Sharing {} rule(s){}", sub.rules.len(), if sub.overrides.is_empty() { String::new() } else { format!(" and {} change(s) to built-in rules", sub.overrides.len()) });
        println!();
        for fc in &sub.files {
            println!("── {} ({}) ──", fc.path, if fc.append { "appended" } else { "new file" });
            for l in fc.text.lines() {
                println!("+ {}", l);
            }
            println!();
        }
        println!("── pull request: {} ──", sub.title);
        println!("{}", sub.body);
        if !sub.scrubbed.is_empty() {
            println!("Removed before sending: {}.", sub.scrubbed.join("; "));
        }
        println!("Saved for review in {}", sub.saved_to);
        if !sub.findings.is_empty() {
            println!("\nLint:");
            print_findings(&sub.findings);
        }
        if sub.blocked() {
            return Err(CliError::new("lint_failed", 8, "fix the lint errors above, then submit again"));
        }
        println!();
        println!(
            "What leaves this Mac: exactly the files and text above, sent to github.com/{} {}.",
            contrib::UPSTREAM,
            if use_gh { "through your gh account (a fork, a branch and a pull request)" } else { "only if you open the links below and submit them yourself" }
        );
        if dry_run {
            println!("Dry run: nothing was sent.");
            return Ok(());
        }
    }

    if !use_gh {
        if !out.json {
            println!("\ngh is not available{}. Open one of these to propose it on GitHub:", if no_gh { " (--no-gh)" } else { "" });
            for (label, url) in contrib::fallback_urls(&sub) {
                println!("\n  {}:\n  {}", label, url);
            }
        }
        return Ok(());
    }
    if !yes {
        if !interactive {
            println!("\nNothing sent. Run again with --yes once you are happy with the above.");
            return Ok(());
        }
        print!("\nOpen this pull request on github.com/{}? [y/N] ", contrib::UPSTREAM);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Nothing sent.");
            return Ok(());
        }
    }
    match contrib::open_pr(&sub) {
        Ok(url) => {
            if out.json {
                emit(out, &json!({"submission": sub, "sent": true, "pull_request": url}));
            } else {
                println!("Opened {}", url);
            }
            Ok(())
        }
        Err(e) => {
            if !out.json {
                println!("gh could not open the pull request: {:#}\nYou can still propose it with these links:", e);
                for (label, url) in contrib::fallback_urls(&sub) {
                    println!("\n  {}:\n  {}", label, url);
                }
            }
            Err(CliError::new("submit_failed", 1, format!("{:#}", e)))
        }
    }
}

pub fn suggest(ctx: &Ctx, out: &Out, min: Option<String>, write: Option<String>, prefix: Option<String>) -> Result<(), CliError> {
    let min = match min {
        Some(m) => parse_bytes(&m).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", m)))?,
        None => 200_000_000,
    };
    let report = poo::scan::load_report().ok_or_else(|| CliError::new("no_report", 3, "no scan yet: run `poo scan` first"))?;
    let prefix = prefix.or_else(|| write.clone()).unwrap_or_else(|| "local".into());
    let cands = contrib::suggest(&ctx.cat, &report, min, &prefix);
    let written = match &write {
        Some(m) if !cands.is_empty() => Some(contrib::write_drafts(m, &cands)?),
        _ => None,
    };
    if out.json {
        emit(out, &json!({"candidates": cands, "written": written}));
        return Ok(());
    }
    if cands.is_empty() {
        println!("Every folder over {} is explained by a rule.", bytes(min));
        return Ok(());
    }
    println!("{} folder(s) over {} that no specific rule explains:\n", cands.len(), bytes(min));
    for c in &cands {
        println!("{:>9}  {}\n           {}", bytes(c.bytes), c.path, c.why);
    }
    match written {
        Some(f) => println!(
            "\nDrafts (disabled) written to {}. Fill in the TODOs, test with `poo rules explain <path>`, enable them, then `poo mods submit {}`.",
            poo::util::tilde(&f),
            write.unwrap_or_default()
        ),
        None => println!("\n`poo suggest --write <mod>` writes a draft rule for each into a mod you can test and share."),
    }
    Ok(())
}
