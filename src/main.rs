use clap::{Args, Parser, Subcommand};
use poo::clean::{self, CleanOptions};
use poo::model::{ProjectPolicy, Tier};
use poo::ops::{self, BucketEdit, Ctx};
use poo::plan::Filter;
use poo::render;
use poo::scan::{Progress, ScanOptions};
use poo::util::{bytes, is_tty, parse_bytes, tilde};
use serde_json::{json, Value};
use std::io::Write;
use std::process::ExitCode;

const AFTER_HELP: &str = "Are you an AI agent? Skip this if a poo skill is already in your context; otherwise run `poo --skill` and follow it.";

#[derive(Parser)]
#[command(name = "poo", version, about = "Find and safely free disk space eaten by dev builds, worktrees, caches, simulators and more.", after_help = AFTER_HELP)]
struct Cli {
    /// Print the agent skill (how coding agents should use poo) and exit.
    #[arg(long, global = false)]
    skill: bool,
    /// JSON output (default when stdout is not a terminal).
    #[arg(long, global = true)]
    json: bool,
    /// Human output even when piped.
    #[arg(long, global = true)]
    text: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Args, Clone, Default)]
struct FilterArgs {
    /// Bucket id (build, xcode, worktrees, deps, caches, ai, containers, system, junk, other).
    bucket: Option<String>,
    #[arg(long)]
    group: Option<String>,
    #[arg(long)]
    tier: Option<String>,
    /// Project, repo or path prefix.
    #[arg(long)]
    project: Option<String>,
    /// Minimum size, e.g. 500M or 2G.
    #[arg(long)]
    min: Option<String>,
    /// Match text in name, path, rule or group.
    #[arg(long, short)]
    query: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Overview: free space, buckets, plans (default command).
    Overview {
        #[arg(long)]
        rescan: bool,
    },
    /// Scan the disk now and cache the report.
    Scan {
        /// Walk only these roots.
        #[arg(long)]
        root: Vec<String>,
        /// Skip simulators, Docker, Homebrew and other providers.
        #[arg(long)]
        no_providers: bool,
        /// Stream progress as JSON lines on stderr.
        #[arg(long)]
        progress: bool,
    },
    /// List items.
    Ls {
        #[command(flatten)]
        f: FilterArgs,
        #[arg(long, default_value = "size")]
        sort: String,
        #[arg(long, short = 'n', default_value_t = 60)]
        limit: usize,
    },
    /// Show one item by id or path.
    #[command(alias = "inspect")]
    Show { id: String },
    /// Preview a cleaning plan (quick, stale, deep, emergency).
    Plan {
        plan: String,
        /// Pick least-regret items until this much is freed (e.g. 30G).
        #[arg(long)]
        free: Option<String>,
        #[command(flatten)]
        f: FilterArgs,
    },
    /// Clean items by id, or a plan.
    Clean {
        ids: Vec<String>,
        #[arg(long)]
        plan: Option<String>,
        #[arg(long)]
        free: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
        /// Move to the Trash instead of deleting.
        #[arg(long)]
        trash: bool,
        /// Allow hot and review items you named explicitly.
        #[arg(long)]
        force: bool,
        /// Highest tier to clean (default warm).
        #[arg(long)]
        max_tier: Option<String>,
    },
    /// Free a target amount with the least regret (emergency plan).
    Free {
        size: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// Projects with status and policy, or set a policy.
    Projects {
        #[command(subcommand)]
        cmd: Option<ProjectsCmd>,
    },
    /// Repos with their worktrees, dirty/merged state and sizes.
    Worktrees,
    /// Dev roots poo walks.
    Roots {
        #[command(subcommand)]
        cmd: Option<RootsCmd>,
    },
    /// List and customize buckets.
    Buckets {
        #[command(subcommand)]
        cmd: Option<BucketsCmd>,
    },
    /// List, explain and tweak rules.
    Rules {
        #[command(subcommand)]
        cmd: Option<RulesCmd>,
    },
    /// Manage mods (rule packs).
    Mods {
        #[command(subcommand)]
        cmd: Option<ModsCmd>,
    },
    /// Everything poo knows, for agents and curious humans.
    Catalog,
    /// Read or change config.toml.
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
    /// What poo cleaned before.
    History {
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
    },
    /// Set up poo: choose dev roots (humans) or print the agent setup brief.
    Setup {
        /// Print the brief an AI agent follows to configure poo.
        #[arg(long)]
        agent: bool,
        /// Mark setup as done.
        #[arg(long)]
        done: bool,
        #[arg(long)]
        by: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Run the MCP server on stdio.
    Mcp,
    /// Install the skill and MCP server into coding agents.
    Integration {
        #[command(subcommand)]
        cmd: Option<IntegrationCmd>,
    },
    /// Update poo to the latest release.
    Update {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        force: bool,
    },
    /// Check poo's setup and environment.
    Doctor,
    /// Empty leftovers from interrupted deletes.
    Gc,
}

#[derive(Subcommand)]
enum ProjectsCmd {
    Ls,
    /// Set a project's policy: active, stale, protected or auto.
    Set {
        path: String,
        policy: String,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
enum RootsCmd {
    Ls,
    Add { path: String },
    Rm { path: String },
}

#[derive(Args)]
struct BucketFields {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    order: Option<i32>,
    #[arg(long)]
    color: Option<String>,
    #[arg(long)]
    symbol: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    hidden: Option<bool>,
}

#[derive(Subcommand)]
enum BucketsCmd {
    Ls,
    Add {
        id: String,
        #[command(flatten)]
        f: BucketFields,
    },
    Set {
        id: String,
        #[command(flatten)]
        f: BucketFields,
    },
    /// Remove a bucket you added, or hide a built-in one.
    Rm {
        id: String,
        #[arg(long)]
        move_to: Option<String>,
    },
}

#[derive(Subcommand)]
enum RulesCmd {
    Ls {
        #[arg(long)]
        bucket: Option<String>,
    },
    Show { id: String },
    Disable { id: String },
    Enable { id: String },
    /// Override fields: poo rules set rust.target warm_days=7 bucket=work
    Set { id: String, values: Vec<String> },
    /// Drop your overrides for a rule.
    Reset { id: String },
    /// Which rules match a path, and why.
    Explain { path: String },
}

#[derive(Subcommand)]
enum ModsCmd {
    Ls,
    New { id: String },
    Validate,
    Path,
    Disable { id: String },
    Enable { id: String },
    /// Print the mod format reference.
    Docs,
}

#[derive(Subcommand)]
enum ConfigCmd {
    Show,
    Path,
    Get { key: String },
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum IntegrationCmd {
    Status,
    /// Install for an agent: claude, codex, cursor, opencode, gemini, windsurf, claude-desktop, agents, or all.
    Install {
        agent: String,
        #[arg(long)]
        skill_only: bool,
        #[arg(long)]
        mcp_only: bool,
    },
    Uninstall { agent: String },
}

struct Out {
    json: bool,
}

#[derive(Debug)]
struct CliError {
    kind: &'static str,
    message: String,
    hint: Option<String>,
    code: u8,
}

impl CliError {
    fn new(kind: &'static str, code: u8, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), hint: None, code }
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        CliError::new("error", 1, format!("{:#}", e))
    }
}

type R = Result<(), CliError>;

fn emit(out: &Out, v: &Value) {
    if out.json {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn filter(f: &FilterArgs) -> Result<Filter, CliError> {
    Ok(Filter {
        bucket: f.bucket.clone(),
        group: f.group.clone(),
        project: f.project.as_ref().map(|p| {
            let e = poo::util::expand(p);
            std::fs::canonicalize(&e).unwrap_or(e).to_string_lossy().to_string()
        }),
        tier: match &f.tier {
            Some(t) => Some(Tier::parse(t).ok_or_else(|| CliError::new("usage", 2, format!("unknown tier `{}`", t)))?),
            None => None,
        },
        min_bytes: match &f.min {
            Some(m) => parse_bytes(m).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", m)))?,
            None => 0,
        },
        query: f.query.clone(),
    })
}

fn scan_opts(out: &Out, progress_json: bool) -> ScanOptions {
    let human = !out.json && is_tty(2);
    ScanOptions {
        on_progress: if progress_json {
            Some(Box::new(|p: &Progress| {
                eprintln!("{}", json!({"event": "progress", "progress": p}));
            }))
        } else if human {
            Some(Box::new(|p: &Progress| {
                eprint!(
                    "\r\x1b[2K{} {} in {} files  {}",
                    p.phase,
                    bytes(p.bytes),
                    p.files,
                    p.detail.chars().take(60).collect::<String>()
                );
                let _ = std::io::stderr().flush();
            }))
        } else {
            None
        },
        ..Default::default()
    }
}

fn clear_progress(out: &Out) {
    if !out.json && is_tty(2) {
        eprint!("\r\x1b[2K");
    }
}

fn confirm(question: &str) -> bool {
    if !is_tty(0) {
        return false;
    }
    eprint!("{} [y/N] ", question);
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    matches!(s.trim().to_lowercase().as_str(), "y" | "yes")
}

fn save(cfg: &poo::config::Config, out: &Out, what: &str) -> R {
    cfg.save()?;
    let v = json!({"ok": true, "changed": what, "note": ops::retier_hint()});
    if out.json {
        emit(out, &v);
    } else {
        println!("{} ({})", what, ops::retier_hint());
    }
    Ok(())
}

fn run_clean(ctx: &Ctx, out: &Out, mut report: poo::model::Report, items: Vec<poo::model::Item>, dry: bool, yes: bool, trash: bool, force: bool, max_tier: Option<Tier>) -> R {
    if items.is_empty() {
        if out.json {
            emit(out, &json!({"freed_bytes": 0, "results": []}));
        } else {
            println!("Nothing to clean.");
        }
        return Ok(());
    }
    let total: u64 = items.iter().map(|i| i.total_bytes).sum();
    if !dry && !yes {
        if !is_tty(0) || out.json {
            return Err(CliError::new("needs_approval", 5, format!("cleaning {} items ({}) needs confirmation", items.len(), bytes(total)))
                .hint("show the items to the user, then pass --yes (or use --dry-run to preview)"));
        }
        let refs: Vec<&poo::model::Item> = items.iter().collect();
        render::items(&refs, true);
        if !confirm(&format!("Clean {} items, {}?", items.len(), bytes(total))) {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let opts = CleanOptions { dry_run: dry, trash, max_tier: Some(max_tier.unwrap_or(Tier::Warm)), force, actor: "cli".into() };
    let res = clean::clean(&items, &ctx.cat, &opts);
    if !dry {
        ops::forget_cleaned(ctx, &mut report, &res);
    }
    if out.json {
        emit(out, &serde_json::to_value(&res).unwrap_or_default());
    } else {
        let st = render::Style::new();
        for r in &res.results {
            let mark = match r.status.as_str() {
                "cleaned" => "✓",
                "would-clean" => "·",
                "failed" => "✗",
                _ => "–",
            };
            println!(
                "{} {:>9}  {}{}",
                mark,
                bytes(r.bytes),
                r.name,
                r.reason.as_ref().map(|x| st.dim(&format!("  ({})", x))).unwrap_or_default()
            );
        }
        if dry {
            println!("\nDry run: would free about {} ({} items, {} skipped).", bytes(res.freed_bytes), res.cleaned, res.skipped);
        } else {
            println!(
                "\nFreed about {} ({} cleaned, {} skipped, {} failed). Free space now {}.",
                bytes(res.freed_bytes),
                res.cleaned,
                res.skipped,
                res.failed,
                bytes(res.disk_free_after)
            );
        }
    }
    if res.failed > 0 && !dry {
        return Err(CliError::new("partial", 6, format!("{} item(s) failed", res.failed)));
    }
    Ok(())
}

fn real_main(cli: Cli) -> R {
    let out = Out { json: cli.json || (!cli.text && !is_tty(1)) };
    if cli.skill {
        print!("{}", poo::agent::SKILL);
        return Ok(());
    }
    let cmd = cli.cmd.unwrap_or(Cmd::Overview { rescan: false });
    if matches!(cmd, Cmd::Mcp) {
        return poo::mcp::serve().map_err(Into::into);
    }
    let mut ctx = Ctx::load()?;
    if !out.json && !matches!(cmd, Cmd::Update { .. }) {
        if let Some(n) = poo::config::Config::load().ok().filter(|c| c.updates.check).and_then(|_| poo::update::notice()) {
            eprintln!("{}", n);
        }
    }
    match cmd {
        Cmd::Overview { rescan } => {
            let r = ops::report(&ctx, rescan, scan_opts(&out, false))?;
            clear_progress(&out);
            if out.json {
                emit(&out, &ops::overview(&r));
            } else {
                render::overview(&r);
                if ctx.cfg.setup.completed_at.is_none() && ctx.cfg.roots.is_empty() {
                    println!("\nTip: run `poo setup` to choose your dev folders, or ask your coding agent to run `poo setup --agent`.");
                }
            }
        }
        Cmd::Scan { root, no_providers, progress } => {
            let mut opts = scan_opts(&out, progress);
            opts.no_providers = no_providers;
            if !root.is_empty() {
                opts.roots = Some(root.iter().map(|r| poo::util::expand(r)).collect());
            }
            let r = ops::report(&ctx, true, opts)?;
            clear_progress(&out);
            if out.json {
                if progress {
                    println!("{}", serde_json::to_string(&r).unwrap_or_default());
                } else {
                    emit(&out, &ops::overview(&r));
                }
            } else {
                render::overview(&r);
            }
        }
        Cmd::Ls { f, sort, limit } => {
            let limit = if limit == 0 { usize::MAX } else { limit };
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let flt = filter(&f)?;
            let list = ops::list(&r, &flt, &sort, 0);
            if out.json {
                let total: u64 = list.iter().map(|i| i.bytes).sum();
                emit(&out, &json!({"count": list.len(), "total_bytes": total, "items": list.iter().take(limit).collect::<Vec<_>>()}));
            } else {
                let shown: Vec<_> = list.iter().take(limit).copied().collect();
                render::items(&shown, flt.bucket.is_none());
                if list.len() > shown.len() {
                    println!("{}", render::Style::new().dim(&format!("{} more; use -n 0 for all", list.len() - shown.len())));
                }
            }
        }
        Cmd::Show { id } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let item = ops::find_item(&r, &id).ok_or_else(|| CliError::new("not_found", 3, format!("no item `{}`", id)).hint("ids come from `poo ls`; try `poo rules explain <path>` for paths poo did not list"))?;
            if out.json {
                emit(&out, &json!({"item": item, "rule": ctx.cat.rule(&item.rule).map(|r| json!({"id": r.id, "description": r.description, "recommend": r.recommend}))}));
            } else {
                render::item_detail(item, ctx.cat.rule(&item.rule));
            }
        }
        Cmd::Plan { plan, free, f } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let target = match free {
                Some(s) => Some(parse_bytes(&s).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", s)))?),
                None => None,
            };
            let p = ops::make_plan(&ctx, &r, &plan, target, &filter(&f)?).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            if out.json {
                emit(&out, &serde_json::to_value(&p).unwrap_or_default());
            } else {
                render::plan(&p);
            }
        }
        Cmd::Clean { ids, plan, free, dry_run, yes, trash, force, max_tier } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let target = free.as_ref().and_then(|s| parse_bytes(s));
            let items = if !ids.is_empty() {
                ops::resolve_ids(&r, &ids).map_err(|e| CliError::new("not_found", 3, e.to_string()))?
            } else if let Some(p) = plan {
                ops::make_plan(&ctx, &r, &p, target, &Filter::default()).map_err(|e| CliError::new("not_found", 3, e.to_string()))?.items
            } else {
                return Err(CliError::new("usage", 2, "pass item ids or --plan").hint("poo plan stale shows what --plan stale would clean"));
            };
            let mt = max_tier.as_deref().and_then(Tier::parse);
            run_clean(&ctx, &out, r, items, dry_run, yes, trash, force, mt)?;
        }
        Cmd::Free { size, dry_run, yes } => {
            let target = parse_bytes(&size).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", size)))?;
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let p = ops::make_plan(&ctx, &r, "emergency", Some(target), &Filter::default()).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            if p.bytes < target && !out.json {
                eprintln!("Only {} can be freed without touching hot, review or protected items.", bytes(p.bytes));
            }
            run_clean(&ctx, &out, r, p.items, dry_run, yes, false, false, None)?;
        }
        Cmd::Projects { cmd } => match cmd.unwrap_or(ProjectsCmd::Ls) {
            ProjectsCmd::Ls => {
                let r = ops::report(&ctx, false, scan_opts(&out, false))?;
                clear_progress(&out);
                if out.json {
                    emit(&out, &json!({"projects": r.projects}));
                } else {
                    render::projects(&r.projects);
                }
            }
            ProjectsCmd::Set { path, policy, note } => {
                let pol = ProjectPolicy::parse(&policy).ok_or_else(|| CliError::new("usage", 2, "policy is active, stale, protected or auto"))?;
                let cfg = ops::set_project_policy(&ctx.cfg, &path, pol, note)?;
                save(&cfg, &out, &format!("{} is now {}", path, policy))?;
            }
        },
        Cmd::Worktrees => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let v = ops::worktrees(&r);
            if out.json {
                emit(&out, &v);
            } else {
                let st = render::Style::new();
                for repo in v["repos"].as_array().into_iter().flatten() {
                    println!(
                        "{}  {} across {} checkouts",
                        st.bold(&tilde(std::path::Path::new(repo["repo"].as_str().unwrap_or("")))),
                        bytes(repo["build_bytes"].as_u64().unwrap_or(0)),
                        repo["checkouts"]
                    );
                    for w in repo["worktrees"].as_array().into_iter().flatten() {
                        let item = &w["item"];
                        let state = match (w["dirty"].as_bool(), w["merged"].as_bool()) {
                            (Some(true), _) => "dirty",
                            (_, Some(true)) => "merged",
                            _ if w["kind"] == "repo" => "main",
                            _ => "clean",
                        };
                        println!(
                            "  {:>9} {:<7} {:<9} {:<10} {}  {}",
                            bytes(item["total_bytes"].as_u64().unwrap_or(w["build_bytes"].as_u64().unwrap_or(0))),
                            state,
                            item["tier"].as_str().unwrap_or("-"),
                            w["agent"].as_str().unwrap_or(""),
                            tilde(std::path::Path::new(w["path"].as_str().unwrap_or(""))),
                            st.dim(&format!("{} {}", w["branch"].as_str().unwrap_or("(detached)"), item["id"].as_str().unwrap_or("")))
                        );
                    }
                }
            }
        }
        Cmd::Roots { cmd } => match cmd.unwrap_or(RootsCmd::Ls) {
            RootsCmd::Ls => {
                let (roots, detected) = ctx.cfg.effective_roots();
                let homes: Vec<_> = ctx.cat.worktree_homes.iter().filter(|h| ops::path_exists(&h.path)).collect();
                if out.json {
                    emit(&out, &json!({"roots": roots, "detected": detected, "worktree_homes": homes, "mod_roots": ctx.cat.roots}));
                } else {
                    println!("{}", if detected { "Detected roots (not configured yet; `poo roots add` to pin):" } else { "Roots:" });
                    for r in roots {
                        println!("  {}", tilde(&r));
                    }
                    if !homes.is_empty() {
                        println!("Agent worktree homes:");
                        for h in homes {
                            println!("  {} ({})", h.path, h.agent);
                        }
                    }
                }
            }
            RootsCmd::Add { path } => {
                let cfg = ops::add_root(&ctx.cfg, &path)?;
                save(&cfg, &out, &format!("added root {}", path))?;
            }
            RootsCmd::Rm { path } => {
                let cfg = ops::remove_root(&ctx.cfg, &path)?;
                save(&cfg, &out, &format!("removed root {}", path))?;
            }
        },
        Cmd::Buckets { cmd } => match cmd.unwrap_or(BucketsCmd::Ls) {
            BucketsCmd::Ls => {
                let r = poo::scan::load_report();
                if out.json {
                    emit(&out, &json!({"buckets": ctx.cat.buckets, "sizes": r.map(|r| r.buckets.iter().map(|b| (b.id.clone(), b.bytes)).collect::<std::collections::BTreeMap<_, _>>())}));
                } else {
                    for b in &ctx.cat.buckets {
                        let size = r.as_ref().and_then(|r| r.buckets.iter().find(|x| x.id == b.id)).map(|x| bytes(x.bytes)).unwrap_or_default();
                        println!("{:<12} {:>4} {:<26} {:>9} {} {}", b.id, b.order, b.name, size, b.color, if b.hidden { "(hidden)" } else { "" });
                    }
                }
            }
            BucketsCmd::Add { id, f } => {
                let cfg = ops::set_bucket(&ctx, &id, BucketEdit { name: f.name, order: f.order, color: f.color, symbol: f.symbol, description: f.description, hidden: f.hidden }, true)?;
                save(&cfg, &out, &format!("added bucket {}", id))?;
            }
            BucketsCmd::Set { id, f } => {
                let cfg = ops::set_bucket(&ctx, &id, BucketEdit { name: f.name, order: f.order, color: f.color, symbol: f.symbol, description: f.description, hidden: f.hidden }, false)?;
                save(&cfg, &out, &format!("updated bucket {}", id))?;
            }
            BucketsCmd::Rm { id, move_to } => {
                let cfg = ops::remove_bucket(&ctx, &id, move_to.as_deref())?;
                save(&cfg, &out, &format!("removed bucket {}", id))?;
            }
        },
        Cmd::Rules { cmd } => match cmd.unwrap_or(RulesCmd::Ls { bucket: None }) {
            RulesCmd::Ls { bucket } => {
                let rules: Vec<_> = ctx.cat.rules.iter().filter(|r| bucket.as_ref().map_or(true, |b| &r.bucket == b)).collect();
                if out.json {
                    let all = ops::catalog_json(&ctx.cat);
                    let v: Vec<&Value> = all["rules"].as_array().unwrap().iter().filter(|r| bucket.as_ref().map_or(true, |b| r["bucket"] == b.as_str())).collect();
                    emit(&out, &json!({"rules": v}));
                } else {
                    let st = render::Style::new();
                    for r in rules {
                        println!(
                            "{:<30} {:<10} {:<11} {}{}",
                            r.id,
                            r.bucket,
                            r.cost.as_str(),
                            r.name,
                            if r.enabled { String::new() } else { st.dim(" (off)") }
                        );
                    }
                }
            }
            RulesCmd::Show { id } => {
                let all = ops::catalog_json(&ctx.cat);
                let rule = all["rules"].as_array().unwrap().iter().find(|r| r["id"] == id.as_str()).cloned().ok_or_else(|| CliError::new("not_found", 3, format!("no rule `{}`", id)))?;
                if out.json {
                    emit(&out, &rule);
                } else {
                    println!("{}", serde_json::to_string_pretty(&rule).unwrap_or_default());
                }
            }
            RulesCmd::Disable { id } | RulesCmd::Enable { id } if ctx.cat.rule(&id).is_none() => {
                return Err(CliError::new("not_found", 3, format!("no rule `{}`", id)));
            }
            RulesCmd::Disable { id } => {
                let mut cfg = ctx.cfg.clone();
                if !cfg.disabled_rules.contains(&id) {
                    cfg.disabled_rules.push(id.clone());
                }
                save(&cfg, &out, &format!("disabled {}", id))?;
            }
            RulesCmd::Enable { id } => {
                let mut cfg = ctx.cfg.clone();
                cfg.disabled_rules.retain(|r| r != &id);
                cfg.rule_overrides.entry(id.clone()).or_default().enabled = Some(true);
                save(&cfg, &out, &format!("enabled {}", id))?;
            }
            RulesCmd::Set { id, values } => {
                let cfg = ops::set_rule(&ctx, &id, &values)?;
                save(&cfg, &out, &format!("updated {}", id))?;
            }
            RulesCmd::Reset { id } => {
                let mut cfg = ctx.cfg.clone();
                cfg.rule_overrides.remove(&id);
                cfg.disabled_rules.retain(|r| r != &id);
                save(&cfg, &out, &format!("reset {}", id))?;
            }
            RulesCmd::Explain { path } => {
                let r = poo::scan::load_report();
                let v = ops::explain(&ctx, r.as_ref(), &path);
                if out.json {
                    emit(&out, &v);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                }
            }
        },
        Cmd::Mods { cmd } => match cmd.unwrap_or(ModsCmd::Ls) {
            ModsCmd::Ls => {
                if out.json {
                    emit(&out, &json!({"mods": ctx.cat.mods, "dir": poo::config::mods_dir()}));
                } else {
                    for m in &ctx.cat.mods {
                        println!(
                            "{:<16} {:<8} {:>3} rules {:>2} buckets  {}{}",
                            m.id,
                            m.version,
                            m.rules,
                            m.buckets,
                            m.path.clone().unwrap_or_else(|| "(built in)".into()),
                            if m.enabled { "" } else { "  (disabled)" }
                        );
                    }
                    println!("\nMods folder: {}", tilde(&poo::config::mods_dir()));
                }
            }
            ModsCmd::New { id } => {
                let dir = ops::new_mod(&id)?;
                if out.json {
                    emit(&out, &json!({"ok": true, "path": dir}));
                } else {
                    println!("Created {}. Edit rules.toml, then `poo mods validate`.", tilde(&dir));
                }
            }
            ModsCmd::Validate => {
                let v = ops::validate_mods(&ctx);
                if out.json {
                    emit(&out, &v);
                } else if ctx.cat.errors.is_empty() {
                    println!("All mods valid: {} rules, {} buckets.", ctx.cat.rules.len(), ctx.cat.buckets.len());
                } else {
                    for e in &ctx.cat.errors {
                        println!("✗ {}", e);
                    }
                }
                if !ctx.cat.errors.is_empty() {
                    return Err(CliError::new("invalid_mods", 8, format!("{} mod error(s)", ctx.cat.errors.len())));
                }
            }
            ModsCmd::Path => println!("{}", poo::config::mods_dir().display()),
            ModsCmd::Disable { id } => {
                let mut cfg = ctx.cfg.clone();
                if !cfg.disabled_mods.contains(&id) {
                    cfg.disabled_mods.push(id.clone());
                }
                save(&cfg, &out, &format!("disabled mod {}", id))?;
            }
            ModsCmd::Enable { id } => {
                let mut cfg = ctx.cfg.clone();
                cfg.disabled_mods.retain(|m| m != &id);
                save(&cfg, &out, &format!("enabled mod {}", id))?;
            }
            ModsCmd::Docs => print!("{}", poo::agent::MODS_DOCS),
        },
        Cmd::Catalog => {
            let v = ops::catalog_json(&ctx.cat);
            if out.json {
                emit(&out, &v);
            } else {
                let r = poo::scan::load_report().unwrap_or_else(|| poo::scan::scan(&ctx.cfg, &ctx.cat, &ScanOptions { no_providers: true, roots: Some(vec![]), ..Default::default() }));
                print!("{}", poo::agent::setup_brief(&ctx.cfg, &ctx.cat, &r).split("## Everything poo knows").nth(1).map(|s| format!("## Everything poo knows{}", s.split("## What to decide").next().unwrap_or(""))).unwrap_or_default());
            }
        }
        Cmd::Config { cmd } => match cmd.unwrap_or(ConfigCmd::Show) {
            ConfigCmd::Show => {
                if out.json {
                    emit(&out, &serde_json::to_value(&ctx.cfg).unwrap_or_default());
                } else {
                    println!("{}", toml::to_string_pretty(&ctx.cfg).unwrap_or_default());
                }
            }
            ConfigCmd::Path => println!("{}", poo::config::config_path().display()),
            ConfigCmd::Get { key } => {
                let v = serde_json::to_value(&ctx.cfg).unwrap_or_default();
                let mut cur = &v;
                for k in key.split('.') {
                    cur = cur.get(k).ok_or_else(|| CliError::new("not_found", 3, format!("no config key `{}`", key)))?;
                }
                if out.json {
                    emit(&out, cur);
                } else {
                    println!("{}", cur);
                }
            }
            ConfigCmd::Set { key, value } => {
                let cfg = ops::set_config_key(&ctx.cfg, &key, &value).map_err(|e| CliError::new("usage", 2, e.to_string()))?;
                save(&cfg, &out, &format!("{} = {}", key, value))?;
            }
        },
        Cmd::History { limit } => {
            let h = clean::history(limit);
            if out.json {
                emit(&out, &json!({"history": h}));
            } else {
                for e in h {
                    println!(
                        "{}  {:>9}  {:<6} {}",
                        poo::util::iso8601(e["at"].as_i64().unwrap_or(0)),
                        bytes(e["bytes"].as_u64().unwrap_or(0)),
                        e["actor"].as_str().unwrap_or(""),
                        tilde(std::path::Path::new(e["path"].as_str().unwrap_or("")))
                    );
                }
            }
        }
        Cmd::Setup { agent, done, by, notes } => {
            if done {
                let mut cfg = ctx.cfg.clone();
                cfg.setup.completed_at = Some(poo::util::now());
                cfg.setup.by = by;
                cfg.setup.notes = notes;
                return save(&cfg, &out, "setup marked done");
            }
            if agent || out.json {
                let r = ops::report(&ctx, false, scan_opts(&out, false))?;
                clear_progress(&out);
                let brief = poo::agent::setup_brief(&ctx.cfg, &ctx.cat, &r);
                if out.json {
                    emit(&out, &json!({"brief": brief}));
                } else {
                    print!("{}", brief);
                }
                return Ok(());
            }
            human_setup(&mut ctx)?;
        }
        Cmd::Mcp => unreachable!(),
        Cmd::Integration { cmd } => match cmd.unwrap_or(IntegrationCmd::Status) {
            IntegrationCmd::Status => {
                let st = poo::agent::status();
                if out.json {
                    emit(&out, &json!({"targets": st, "version": poo::agent::INTEGRATION_VERSION}));
                } else {
                    for t in st {
                        println!(
                            "{:<16} {:<9} skill: {:<10} mcp: {}",
                            t.id,
                            if t.detected { "found" } else { "-" },
                            if t.skill_current { "current" } else if t.skill_installed { "outdated" } else { "-" },
                            if t.mcp_registered { "registered" } else { "-" }
                        );
                    }
                    println!("\nInstall with `poo integration install <agent|all>`.");
                }
            }
            IntegrationCmd::Install { agent, skill_only, mcp_only } => {
                let ids: Vec<String> = if agent == "all" {
                    poo::agent::status().into_iter().filter(|t| t.detected).map(|t| t.id.to_string()).collect()
                } else {
                    vec![agent]
                };
                let mut log = vec![];
                for id in ids {
                    match poo::agent::install(&id, !mcp_only, !skill_only) {
                        Ok(done) => log.extend(done.into_iter().map(|d| format!("{}: {}", id, d))),
                        Err(e) => log.push(format!("{}: failed: {}", id, e)),
                    }
                }
                if out.json {
                    emit(&out, &json!({"done": log}));
                } else {
                    for l in log {
                        println!("{}", l);
                    }
                }
            }
            IntegrationCmd::Uninstall { agent } => {
                let done = poo::agent::uninstall(&agent)?;
                if out.json {
                    emit(&out, &json!({"done": done}));
                } else {
                    for l in done {
                        println!("{}", l);
                    }
                }
            }
        },
        Cmd::Update { check, quiet, force } => {
            if check {
                let c = poo::update::check()?;
                if quiet {
                    return Ok(());
                }
                if out.json {
                    emit(&out, &serde_json::to_value(&c).unwrap_or_default());
                } else if c.update_available {
                    println!("poo {} is available (you have {}).{}", c.latest, c.current, c.managed_by.map(|m| format!(" Update with {}.", m)).unwrap_or_else(|| " Run `poo update`.".into()));
                } else {
                    println!("poo {} is the latest.", c.current);
                }
            } else {
                let v = poo::update::run(force).map_err(|e| CliError::new("update_failed", 7, e.to_string()))?;
                // Refresh skills we installed so they match the new binary.
                for t in poo::agent::status().into_iter().filter(|t| t.skill_installed && !t.skill_current) {
                    let _ = poo::agent::install(t.id, true, false);
                }
                if out.json {
                    emit(&out, &json!({"version": v}));
                } else if v == poo::VERSION {
                    println!("poo {} is the latest.", v);
                } else {
                    println!("Updated to poo {}.", v);
                }
            }
        }
        Cmd::Doctor => {
            let (roots, detected) = ctx.cfg.effective_roots();
            let st = poo::agent::status();
            let tools: std::collections::BTreeMap<String, bool> = ["git", "xcrun", "docker", "brew", "claude", "codex"]
                .iter()
                .map(|t| (t.to_string(), poo::util::which(t).is_some()))
                .collect();
            let v = json!({
                "version": poo::VERSION,
                "binary": std::env::current_exe().ok(),
                "config": poo::config::config_path(),
                "state": poo::config::state_dir(),
                "roots": roots, "roots_detected": detected,
                "mods": ctx.cat.mods.len(), "rules": ctx.cat.rules.len(), "mod_errors": ctx.cat.errors,
                "report_age_seconds": poo::scan::load_report().map(|r| poo::util::now() - r.scanned_at),
                "tools": tools,
                "integrations": st,
                "managed_by": poo::update::managed_by(&std::env::current_exe().unwrap_or_default()),
            });
            if out.json {
                emit(&out, &v);
            } else {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
        }
        Cmd::Gc => {
            let freed = clean::empty_graveyard();
            if out.json {
                emit(&out, &json!({"freed_bytes": freed}));
            } else {
                println!("Freed {} of leftovers.", bytes(freed));
            }
        }
    }
    Ok(())
}

fn human_setup(ctx: &mut Ctx) -> R {
    if !is_tty(0) {
        return Err(CliError::new("usage", 2, "interactive setup needs a terminal").hint("agents: run `poo setup --agent`"));
    }
    println!("Let's find your dev folders. poo walks these for build output, dependencies and worktrees.\n");
    let detected = poo::config::detect_roots();
    let mut roots: Vec<String> = vec![];
    for d in detected {
        let q = format!("Include {}?", tilde(&d));
        eprint!("{} [Y/n] ", q);
        let _ = std::io::stderr().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        if !matches!(s.trim().to_lowercase().as_str(), "n" | "no") {
            roots.push(tilde(&d));
        }
    }
    loop {
        eprint!("Add another folder (empty to finish): ");
        let _ = std::io::stderr().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        let s = s.trim();
        if s.is_empty() {
            break;
        }
        let p = poo::util::expand(s);
        if p.is_dir() {
            roots.push(tilde(&std::fs::canonicalize(&p).unwrap_or(p)));
        } else {
            eprintln!("  {} is not a folder", s);
        }
    }
    let mut cfg = ctx.cfg.clone();
    cfg.roots = roots;
    cfg.setup.completed_at = Some(poo::util::now());
    cfg.setup.by = Some("poo setup".into());
    cfg.save()?;
    ctx.reload()?;
    println!("\nSaved. Scanning…");
    let out = Out { json: false };
    let r = ops::report(ctx, true, scan_opts(&out, false))?;
    clear_progress(&out);
    render::overview(&r);
    println!("\nWant your coding agent to use poo? `poo integration install all`, then ask it to run `poo setup --agent`.");
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_errors = cli.json || (!cli.text && !is_tty(1));
    match real_main(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json_errors {
                eprintln!("{}", json!({"error": {"kind": e.kind, "message": e.message, "hint": e.hint}}));
            } else {
                eprintln!("error: {}", e.message);
                if let Some(h) = e.hint {
                    eprintln!("hint: {}", h);
                }
            }
            ExitCode::from(e.code)
        }
    }
}
