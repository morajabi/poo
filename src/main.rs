use clap::{Args, Parser, Subcommand};
use poo::clean::{self, CleanOptions};
use poo::model::{ProjectPolicy, RiskLevel, Tier};
use poo::ops::{self, BucketEdit, Ctx};
use poo::plan::{Admission, Filter};
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
    /// Interactive UI without motion (also POO_NO_ANIMATION or macOS Reduce Motion).
    #[arg(long, global = true)]
    no_animation: bool,
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
    /// Interactive terminal UI (default with no command in a terminal).
    Ui,
    /// Overview: free space, buckets, plans (default command when piped or with --text).
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
        /// Walk everything instead of updating from the file-change journal.
        #[arg(long)]
        full: bool,
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
        /// Preview `clean --plan <PLAN> --include-in-use`: also items in use.
        #[arg(long = "include-in-use", alias = "force")]
        force: bool,
        /// Preview `clean --plan <PLAN> --dangerously-include-protected`: also protected items.
        #[arg(long = "dangerously-include-protected", alias = "danger")]
        danger: bool,
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
        /// Also clean things in use: used in the last hour, a checkout with an agent or editor
        /// open, worktrees with uncommitted work (stashed first), check-first items.
        #[arg(long = "include-in-use", alias = "force")]
        force: bool,
        /// Also protected things: running builds, data, kept projects, system paths, detached
        /// commits (saved to a poo/ branch first). Asks you to type "dangerous".
        #[arg(long = "dangerously-include-protected", alias = "danger")]
        danger: bool,
        /// Highest tier to clean in standard mode (default warm). `auto`: the agent autonomy tier
        /// (agents.auto_clean_max_tier); agents cleaning without asking pass it.
        #[arg(long)]
        max_tier: Option<String>,
        /// Stream progress as JSON lines on stderr.
        #[arg(long)]
        progress: bool,
        /// With --plan: only these buckets (repeatable).
        #[arg(long)]
        bucket: Vec<String>,
        /// With --plan: only items with these tags (repeatable).
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Free a target amount with the least regret (emergency plan).
    Free {
        size: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short)]
        yes: bool,
        /// Let the emergency plan include things in use too.
        #[arg(long = "include-in-use", alias = "force")]
        force: bool,
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
    /// Big folders no rule explains yet, with draft rules to test and share.
    Suggest {
        /// Smallest folder worth a rule (default 200M).
        #[arg(long)]
        min: Option<String>,
        /// Write the drafts (disabled) into this mod, e.g. `poo suggest --write mine`.
        #[arg(long)]
        write: Option<String>,
        /// Id prefix for drafts (default: the --write mod id, or `local`).
        #[arg(long)]
        prefix: Option<String>,
    },
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
    /// What grew: per bucket and group since a point in time, unusual growth, and how long
    /// the free space lasts at this pace.
    Growth {
        /// How far back to compare: 24h, 7d, 2w...
        #[arg(long, default_value = "7d")]
        since: String,
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
        /// Only report whether a newer version exists.
        #[arg(long)]
        check: bool,
        #[arg(long)]
        quiet: bool,
        /// Reinstall even when already on the latest version.
        #[arg(long)]
        force: bool,
        /// The daily background check (spawned by poo itself).
        #[arg(long, hide = true)]
        background: bool,
    },
    /// Routine cleanups that run on their own (launchd).
    Schedule {
        #[command(subcommand)]
        cmd: Option<ScheduleCmd>,
    },
    /// Write a brief about an item, project (path), bucket:<id> or overview for an AI agent.
    Prompt {
        #[arg(default_value = "overview")]
        target: String,
    },
    /// Open an agent (Claude Code, Codex, Cursor...) with a brief about a target.
    Ask {
        #[arg(default_value = "overview")]
        target: String,
        /// Launcher id (see `poo ask --list`); default: agents.default_launcher or the first available.
        #[arg(long = "with")]
        with: Option<String>,
        /// List launchers and whether they are installed.
        #[arg(long)]
        list: bool,
    },
    /// What poo saved before cleaning: config file copies, stashes, rescue branches.
    Backups {
        #[command(subcommand)]
        cmd: Option<BackupsCmd>,
    },
    /// Opt-in agent hooks: clean a worktree's build output when an agent finishes there.
    Hooks {
        #[command(subcommand)]
        cmd: Option<HooksCmd>,
    },
    /// Entry point the agent hooks call.
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        cmd: HookCmd,
    },
    /// Keep the report current: watch for file changes and update as they happen.
    Watch {
        /// Seconds of quiet after a change before updating.
        #[arg(long, default_value_t = 2.0)]
        debounce: f64,
        /// Update at least this often (seconds), even without changes.
        #[arg(long, default_value_t = 600)]
        every: u64,
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

#[derive(Args, Clone, Default)]
struct RuleSelect {
    /// Rule id (or use --tag, --group, --bucket, --stack to select many).
    id: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    group: Option<String>,
    #[arg(long)]
    bucket: Option<String>,
    #[arg(long)]
    stack: Option<String>,
}

#[derive(Subcommand)]
enum RulesCmd {
    Ls {
        #[arg(long)]
        bucket: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        stack: Option<String>,
        /// Only rules that matched something in the last scan.
        #[arg(long)]
        hits: bool,
    },
    Show { id: String },
    /// Disable one rule, or every rule with a tag, group, bucket or stack.
    Disable {
        #[command(flatten)]
        sel: RuleSelect,
    },
    Enable {
        #[command(flatten)]
        sel: RuleSelect,
    },
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
    /// Safety lint for your mods, or for core with --core (CI uses this).
    Lint {
        /// Lint rules/core of this checkout and its matcher fixtures.
        #[arg(long)]
        core: bool,
        /// With --core: rules missing at this git ref are new and need fixtures.
        #[arg(long)]
        base: Option<String>,
        /// Repository root for --core (default: current folder).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Share mods or rules with poo as a pull request: lint, remove personal details,
    /// show everything, and only then open the PR (gh) or print prefilled links.
    Submit {
        /// Mod ids or rule ids from your mods.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Open the pull request without asking (only after a person approved the preview).
        #[arg(long)]
        yes: bool,
        /// Only show what would be sent.
        #[arg(long)]
        dry_run: bool,
        /// Leave out sizes, tool versions and matcher fixtures from this Mac.
        #[arg(long)]
        no_evidence: bool,
        /// Also report `<tool> --version` (repeatable).
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// A sentence for reviewers (scrubbed like everything else).
        #[arg(long)]
        note: Option<String>,
        /// Print prefilled GitHub links instead of using gh.
        #[arg(long)]
        no_gh: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    Show,
    Path,
    /// Every setting with its default and what it does.
    Keys,
    Get { key: String },
    Set {
        key: String,
        value: String,
        /// Confirm a change asked for by a coding agent (agents must ask the user first).
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum BackupsCmd {
    Ls,
    /// Delete backups by id, or all older than an age (30d).
    Prune {
        ids: Vec<String>,
        #[arg(long)]
        older_than: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// Copy a file backup back into its folder (never overwrites).
    Restore {
        id: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Apply the age limit and size budget from config now.
    Enforce,
    /// Print the backups folder.
    Path,
}

#[derive(Subcommand)]
enum HooksCmd {
    Status,
    /// claude, codex or all
    Install {
        agent: String,
        /// Confirm a change asked for by a coding agent (agents must ask the user first).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    Uninstall { agent: String },
    /// Recent hook activity.
    Log {
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    AgentDone {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        detached: bool,
    },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    Ls,
    /// Add a schedule. With only an id that names a preset (nightly, quick, guard, weekly), uses it.
    Add {
        id: String,
        /// Start from a preset: nightly, quick, guard, weekly.
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        plan: Option<String>,
        /// 30m, 6h, 1d, 7d
        #[arg(long)]
        every: Option<String>,
        /// Local time for daily runs, e.g. 03:30
        #[arg(long)]
        at: Option<String>,
        /// Only run when free space is below this (50G or 10%).
        #[arg(long)]
        when_free_below: Option<String>,
        /// Free least-regret items until this much is free (80G or 20%).
        #[arg(long)]
        target_free: Option<String>,
        /// Clean at most this much per run.
        #[arg(long)]
        max: Option<String>,
        #[arg(long)]
        bucket: Vec<String>,
        #[arg(long)]
        exclude_bucket: Vec<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        exclude_tag: Vec<String>,
        #[arg(long)]
        no_notify: bool,
        /// Confirm a change asked for by a coding agent (agents must ask the user first).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    Rm { id: String },
    Enable {
        id: String,
        /// Confirm a change asked for by a coding agent (agents must ask the user first).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    Disable { id: String },
    /// Run a schedule now.
    Run {
        id: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// List presets.
    Presets,
    /// Called by launchd every 15 minutes.
    #[command(hide = true)]
    Tick,
    /// Reinstall the launchd agent.
    Install,
    /// Remove the launchd agent (schedules stay in config).
    Uninstall,
    /// Recent scheduler activity.
    Log {
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
    },
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
        /// Confirm a change asked for by a coding agent (agents must ask the user first).
        #[arg(long, short = 'y')]
        yes: bool,
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

/// A coding agent asking for something that keeps acting after its session (schedules,
/// hooks, integrations, more autonomy) must show the user first and pass `--yes`.
fn agent_gate(yes: bool, what: String) -> R {
    match poo::agent::agent_context() {
        Some(agent) if !yes => Err(CliError::new("needs_approval", 5, format!("{} (asked from {}; nothing was changed)", what, agent))
            .hint("show the user exactly this, ask, and rerun with --yes only after they agree")),
        _ => Ok(()),
    }
}

/// A config.toml that does not load: a typed error agents can act on.
fn config_error(e: anyhow::Error) -> CliError {
    CliError::new("invalid_config", 8, format!("{:#}", e)).hint(format!(
        "fix the TOML in {} (show the user the error first), or move the file aside to start from defaults",
        poo::config::config_path().display()
    ))
}

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
        ..Default::default()
    })
}

/// `poo growth` for people: calm, a few lines.
fn print_growth(g: &Value) {
    let signed = |v: &Value| -> Option<String> {
        let d = v.as_i64()?;
        Some(format!("{}{}", if d < 0 { "-" } else { "+" }, bytes(d.unsigned_abs())))
    };
    if g["from"].is_null() {
        println!("Not enough history for that range yet; poo keeps a sample each time it scans.");
    } else {
        let days = (g["to"].as_i64().unwrap_or(0) - g["from"].as_i64().unwrap_or(0)) as f64 / 86400.0;
        println!("Since {} ({:.0} days ago)", poo::util::iso8601(g["from"].as_i64().unwrap_or(0)).get(..10).unwrap_or(""), days);
        if let Some(f) = signed(&g["free_delta"]) {
            println!("  free space   {}", f);
        }
        for b in g["buckets"].as_array().into_iter().flatten() {
            if let Some(d) = signed(&b["delta"]).filter(|_| b["delta"].as_i64().unwrap_or(0).unsigned_abs() >= 50_000_000) {
                println!("  {:<28} {:>10}  (now {})", b["name"].as_str().unwrap_or(""), d, bytes(b["bytes"].as_u64().unwrap_or(0)));
            }
        }
        let groups = g["groups"].as_array().cloned().unwrap_or_default();
        if !groups.is_empty() {
            println!("Grew the most");
            for x in groups.iter().take(5) {
                println!("  {:<28} {:>10}", x["group"].as_str().unwrap_or(""), signed(&x["delta"]).unwrap_or_default());
            }
        }
    }
    for a in g["anomalies"].as_array().into_iter().flatten() {
        println!("Note: {}", a["message"].as_str().unwrap_or(""));
    }
    if let Some(m) = g["forecast"]["message"].as_str() {
        println!("{}", m);
    }
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

#[allow(clippy::too_many_arguments)]
fn run_clean(ctx: &Ctx, out: &Out, mut report: poo::model::Report, items: Vec<poo::model::Item>, dry: bool, yes: bool, trash: bool, mode: RiskLevel, max_tier: Option<Tier>) -> R {
    run_clean_p(ctx, out, &mut report, items, dry, yes, trash, mode, max_tier, false)
}

#[allow(clippy::too_many_arguments)]
fn run_clean_p(ctx: &Ctx, out: &Out, report: &mut poo::model::Report, items: Vec<poo::model::Item>, dry: bool, yes: bool, trash: bool, mode: RiskLevel, max_tier: Option<Tier>, progress: bool) -> R {
    if items.is_empty() {
        if out.json {
            emit(out, &json!({"freed_bytes": 0, "results": []}));
        } else {
            println!("Nothing to clean.");
        }
        return Ok(());
    }
    let total: u64 = items.iter().map(|i| i.total_bytes).sum();
    let mut opts = CleanOptions::from_config(&ctx.cfg, mode, dry, "cli");
    opts.trash = trash || ctx.cfg.backup.use_trash;
    opts.max_tier = Some(max_tier.unwrap_or(Tier::Warm));
    let _lock = if dry { None } else {
        Some(poo::schedule::RunLock::try_acquire().ok_or_else(|| CliError::new("busy", 9, "another poo clean or scheduled run is in progress"))?)
    };
    if !dry && !yes {
        if !is_tty(0) || out.json {
            return Err(CliError::new("needs_approval", 5, format!("cleaning {} items ({}) needs confirmation", items.len(), bytes(total)))
                .hint("show the items to the user, then pass --yes (or use --dry-run to preview)"));
        }
        let refs: Vec<&poo::model::Item> = items.iter().collect();
        render::items(&refs, true);
        // Show every risk this mode accepts before asking.
        let procs_list = poo::procs::list();
        let st = render::Style::new();
        let mut risky = 0;
        for i in &items {
            match clean::admit(i, &ctx.cat, &opts, &procs_list) {
                Ok(w) if !w.is_empty() => {
                    risky += 1;
                    eprintln!("{} {}", st.bold("⚠"), i.name);
                    for x in w {
                        eprintln!("    {}", x);
                    }
                }
                Err(e) => eprintln!("{} {}  {}", st.dim("–"), i.name, st.dim(&format!("skipped: {}", e))),
                _ => {}
            }
        }
        if mode == RiskLevel::Danger {
            eprint!("Including protected items: {} item(s) carry warnings above. Type \"dangerous\" to clean {}: ", risky, bytes(total));
            let _ = std::io::stderr().flush();
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            if s.trim() != "dangerous" {
                println!("Cancelled.");
                return Ok(());
            }
        } else if !confirm(&format!("Clean {} items, {}{}?", items.len(), bytes(total), if risky > 0 { format!(" ({} with warnings)", risky) } else { String::new() })) {
            println!("Cancelled.");
            return Ok(());
        }
    }
    clean::install_cancel_handler();
    let human_progress = !out.json && is_tty(2) && !dry;
    let tick = std::cell::Cell::new(0usize);
    let cb = |p: &clean::CleanProgress| {
        if progress {
            eprintln!("{}", serde_json::to_string(p).unwrap_or_default());
        } else if human_progress {
            eprint!("\r\x1b[2K{}", render::clean_progress_line(p, tick.get()));
            tick.set(tick.get() + 1);
            let _ = std::io::stderr().flush();
        }
    };
    let res = clean::clean_with(&items, &ctx.cat, &opts, Some(&cb));
    if human_progress {
        eprint!("\r\x1b[2K");
    }
    if !dry {
        ops::forget_cleaned(ctx, report, &res);
    }
    if out.json {
        emit(out, &serde_json::to_value(&res).unwrap_or_default());
    } else {
        render::clean_result_rows(&res);
        if let Some(n) = &res.snapshot_note {
            println!("\nnote: {}", n);
        }
        if res.cancelled {
            println!("\nCancelled after the item in flight.");
        }
        if res.background_delete {
            println!("\nErasing in the background; free space grows over the next minutes.");
        }
        if let Some(n) = &res.read_only {
            println!("\n{}", n);
        }
        if dry {
            render::clean_summary(&res);
        } else {
            render::clean_summary(&res);
        }
    }
    if res.failed > 0 && !dry {
        return Err(CliError::new("partial", 6, format!("{} item(s) failed", res.failed)));
    }
    Ok(())
}

fn real_main(cli: Cli) -> R {
    let out = Out { json: cli.json || (!cli.text && !is_tty(1)) };
    let explicit_json = cli.json;
    if cli.skill {
        print!("{}", poo::agent::SKILL);
        return Ok(());
    }
    let interactive = !cli.json && !cli.text && is_tty(0) && is_tty(1) && std::env::var("TERM").map_or(true, |t| t != "dumb");
    let cmd = cli.cmd.unwrap_or(if interactive { Cmd::Ui } else { Cmd::Overview { rescan: false } });
    if matches!(cmd, Cmd::Ui) {
        if !is_tty(0) || !is_tty(1) {
            return Err(CliError::new("usage", 2, "poo ui needs an interactive terminal").hint("use `poo --text` or `poo --json` when piping"));
        }
        // The daily update check still runs; its hint would be wiped by the full-screen UI.
        if let Ok(cfg) = poo::config::Config::load() {
            if cfg.updates.check && poo::update::background_allowed() {
                let _ = poo::update::background(cfg.updates.auto);
            }
        }
        return poo::tui::run(poo::tui::UiOptions::detect(cli.no_animation)).map_err(Into::into);
    }
    if matches!(cmd, Cmd::Mcp) {
        // Agents may be the only ones running poo: keep it current from here too (silently).
        if let Ok(cfg) = poo::config::Config::load() {
            if cfg.updates.check && poo::update::background_allowed() {
                let _ = poo::update::background(cfg.updates.auto);
            }
        }
        return poo::mcp::serve().map_err(Into::into);
    }
    let mut ctx = Ctx::load().map_err(config_error)?;
    if ctx.cfg.updates.check && !matches!(cmd, Cmd::Update { .. } | Cmd::Watch { .. }) && poo::update::background_allowed() {
        if let Some(n) = poo::update::background(ctx.cfg.updates.auto) {
            if !out.json && is_tty(2) {
                eprintln!("{}", n);
            }
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
        Cmd::Scan { root, no_providers, progress, full } => {
            let mut opts = scan_opts(&out, progress);
            opts.no_providers = no_providers;
            opts.full = full;
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
        Cmd::Plan { plan, free, f, force, danger } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let target = match free {
                Some(s) => Some(parse_bytes(&s).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", s)))?),
                None => None,
            };
            let mode = if danger { RiskLevel::Danger } else if force { RiskLevel::Force } else { RiskLevel::Standard };
            let p = ops::make_plan(&ctx, &r, &plan, target, &filter(&f)?, Admission::mode(mode)).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            if out.json {
                emit(&out, &serde_json::to_value(&p).unwrap_or_default());
            } else {
                render::plan(&p);
            }
        }
        Cmd::Clean { ids, plan, free, dry_run, yes, trash, force, danger, max_tier, progress, bucket, tag } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let target = free.as_ref().and_then(|s| parse_bytes(s));
            // `auto` is the agent autonomy tier: what an agent may clean without asking.
            let mt = match max_tier.as_deref() {
                None => None,
                Some("auto") => Some(ctx.cfg.agents.auto_clean_max_tier),
                Some(t) => Some(Tier::parse(t).or_else(|| Tier::all().into_iter().find(|x| x.label() == t)).ok_or_else(|| {
                    CliError::new("usage", 2, format!("unknown tier `{}`", t)).hint("junk, cold, idle, warm, or auto (agents.auto_clean_max_tier)")
                })?),
            };
            let mode = if danger { RiskLevel::Danger } else if force { RiskLevel::Force } else { RiskLevel::Standard };
            let items = if !ids.is_empty() {
                ops::resolve_ids(&r, &ids).map_err(|e| match e.downcast_ref::<ops::ReportOnlyError>() {
                    Some(ro) => CliError::new("report_only", 2, ro.0.clone()).hint("poo never cleans report-only items; manage them where the message says"),
                    None => CliError::new("not_found", 3, e.to_string()),
                })?
            } else if let Some(p) = plan {
                let f = Filter { buckets: bucket, tags: tag, ..Default::default() };
                let adm = Admission { mode, max_tier: Some(mt.unwrap_or(Tier::Warm)) };
                ops::make_plan(&ctx, &r, &p, target, &f, adm).map_err(|e| CliError::new("not_found", 3, e.to_string()))?.items
            } else {
                return Err(CliError::new("usage", 2, "pass item ids or --plan").hint("poo plan stale shows what --plan stale would clean"));
            };
            let mut r = r;
            run_clean_p(&ctx, &out, &mut r, items, dry_run, yes, trash, mode, mt, progress)?;
        }
        Cmd::Free { size, dry_run, yes, force } => {
            let target = parse_bytes(&size).ok_or_else(|| CliError::new("usage", 2, format!("bad size `{}`", size)))?;
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let mode = if force { RiskLevel::Force } else { RiskLevel::Standard };
            let p = ops::make_plan(&ctx, &r, "emergency", Some(target), &Filter::default(), Admission::mode(mode)).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            if p.bytes < target && !out.json {
                eprintln!("Only {} can be freed without touching hot, review or protected items.", bytes(p.bytes));
            }
            run_clean(&ctx, &out, r, p.items, dry_run, yes, false, mode, None)?;
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
                render::worktrees(&v);
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
        Cmd::Rules { cmd } => match cmd.unwrap_or(RulesCmd::Ls { bucket: None, tag: None, stack: None, hits: false }) {
            RulesCmd::Ls { bucket, tag, stack, hits } => {
                let report = poo::scan::load_report();
                let hit = |id: &str| report.as_ref().and_then(|r| r.rule_hits.get(id).copied());
                let keep = |r: &&poo::catalog::Rule| {
                    bucket.as_ref().map_or(true, |b| &r.bucket == b)
                        && tag.as_ref().map_or(true, |t| r.tags.contains(t))
                        && stack.as_ref().map_or(true, |s| r.stacks.contains(s))
                        && (!hits || hit(&r.id).is_some())
                };
                let rules: Vec<_> = ctx.cat.rules.iter().filter(keep).collect();
                if out.json {
                    let all = ops::catalog_json(&ctx.cat);
                    let ids: std::collections::HashSet<&str> = rules.iter().map(|r| r.id.as_str()).collect();
                    let v: Vec<Value> = all["rules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|r| ids.contains(r["id"].as_str().unwrap_or("")))
                        .map(|r| {
                            let mut r = r.clone();
                            if let Some((n, b)) = hit(r["id"].as_str().unwrap_or("")) {
                                r["hits"] = json!({"items": n, "bytes": b});
                            }
                            r
                        })
                        .collect();
                    emit(&out, &json!({"rules": v}));
                } else {
                    let st = render::Style::new();
                    for r in rules {
                        let h = hit(&r.id).map(|(n, b)| format!("{} in {}", bytes(b), n)).unwrap_or_default();
                        println!(
                            "{:<32} {:<10} {:<10} {:>14}  {}{}",
                            r.id,
                            r.bucket,
                            r.cost.as_str(),
                            h,
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
            RulesCmd::Disable { sel } | RulesCmd::Enable { sel } if select_rules(&ctx, &sel).is_empty() => {
                return Err(CliError::new("not_found", 3, "no rule matches that selection").hint("poo rules ls --tag <tag> shows what a tag selects"));
            }
            RulesCmd::Disable { sel } => {
                let ids = select_rules(&ctx, &sel);
                let cfg = ops::toggle_rules(&ctx.cfg, &ids, false);
                save(&cfg, &out, &format!("disabled {} rule(s): {}", ids.len(), ids.join(", ")))?;
            }
            RulesCmd::Enable { sel } => {
                let ids = select_rules(&ctx, &sel);
                let cfg = ops::toggle_rules(&ctx.cfg, &ids, true);
                save(&cfg, &out, &format!("enabled {} rule(s): {}", ids.len(), ids.join(", ")))?;
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
            ModsCmd::Lint { core, base, repo } => contrib_cmd::lint(&ctx, &out, core, base, repo)?,
            ModsCmd::Submit { ids, yes, dry_run, no_evidence, tools, note, no_gh } => {
                contrib_cmd::submit(&ctx, &out, &ids, yes, dry_run, !no_evidence, tools, note, no_gh)?
            }
        },
        Cmd::Suggest { min, write, prefix } => contrib_cmd::suggest(&ctx, &out, min, write, prefix)?,
        Cmd::Catalog => {
            let v = ops::catalog_json(&ctx.cat);
            if out.json {
                emit(&out, &v);
            } else {
                print!("{}", poo::agent::catalog_section(&ctx.cat, None));
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
            ConfigCmd::Keys => {
                let defaults = serde_json::to_value(poo::config::Config::default()).unwrap_or_default();
                if out.json {
                    emit(&out, &json!({"keys": poo::config::KEYS.iter().map(|(k, d)| json!({"key": k, "description": d})).collect::<Vec<_>>(), "defaults": defaults}));
                } else {
                    for (k, d) in poo::config::KEYS {
                        println!("{}\n    {}", k, d);
                    }
                    println!("\nCurrent values: poo config show. Change: poo config set <key> <value>.");
                }
            }
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
            ConfigCmd::Set { key, value, yes } => {
                if key == "agents.auto_clean_max_tier" && Tier::parse(&value).map_or(false, |t| t.rank() > ctx.cfg.agents.auto_clean_max_tier.rank()) {
                    agent_gate(yes, format!("raising agents.auto_clean_max_tier from {} to {} lets agents clean more without asking", ctx.cfg.agents.auto_clean_max_tier.as_str(), value))?;
                }
                let cfg = ops::set_config_key(&ctx.cfg, &key, &value).map_err(|e| CliError::new("usage", 2, e.to_string()))?;
                save(&cfg, &out, &format!("{} = {}", key, value))?;
            }
        },
        Cmd::Growth { since } => {
            let secs = poo::growth::parse_since(&since).ok_or_else(|| CliError::new("usage", 2, format!("bad duration `{}`", since)).hint("use 24h, 7d or 2w"))?;
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let g = poo::growth::report(&r, secs);
            if out.json {
                emit(&out, &g);
            } else {
                print_growth(&g);
            }
        }
        Cmd::History { limit } => {
            let h = clean::history(limit);
            if out.json {
                emit(&out, &json!({"history": h}));
            } else {
                for e in h {
                    if e["status"].as_str() == Some("skipped") {
                        println!(
                            "{}  {:>9}  {:<6} {}  ({})",
                            poo::util::iso8601(e["at"].as_i64().unwrap_or(0)),
                            "skipped",
                            e["actor"].as_str().unwrap_or(""),
                            tilde(std::path::Path::new(e["path"].as_str().unwrap_or(""))),
                            e["reason"].as_str().unwrap_or("")
                        );
                        continue;
                    }
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
                // The brief is markdown for agents: keep it readable when piped.
                if out.json && (explicit_json || !agent) {
                    emit(&out, &json!({"brief": brief}));
                } else {
                    print!("{}", brief);
                }
                return Ok(());
            }
            human_setup(&mut ctx)?;
        }
        Cmd::Mcp | Cmd::Ui => unreachable!(),
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
            IntegrationCmd::Install { agent, skill_only, mcp_only, yes } => {
                agent_gate(yes, format!("installing the poo integration for {} edits that agent's own config files", agent))?;
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
        Cmd::Update { background: true, .. } => {
            if let Ok(Some(_)) = poo::update::background_run(ctx.cfg.updates.auto) {
                refresh_skills();
            }
        }
        Cmd::Update { check, quiet, force, .. } => {
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
                refresh_skills();
                if out.json {
                    emit(&out, &json!({"version": v}));
                } else if v == poo::VERSION {
                    println!("poo {} is the latest.", v);
                } else {
                    println!("Updated to poo {}.", v);
                }
            }
        }
        Cmd::Schedule { cmd } => schedule_cmd(&mut ctx, &out, cmd.unwrap_or(ScheduleCmd::Ls))?,
        Cmd::Backups { cmd } => backups_cmd(&ctx, &out, cmd.unwrap_or(BackupsCmd::Ls))?,
        Cmd::Prompt { target } => {
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let b = poo::ask::brief(&r, &ctx.cat, &target).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            if out.json {
                emit(&out, &serde_json::to_value(&b).unwrap_or_default());
            } else {
                println!("{}", b.prompt);
            }
        }
        Cmd::Ask { target, with, list } => {
            let launchers = poo::ask::launchers(&ctx.cat);
            if list {
                if out.json {
                    emit(&out, &json!({"launchers": launchers.iter().map(|(l, a)| json!({"id": l.id, "name": l.name, "kind": l.kind, "available": a})).collect::<Vec<_>>(), "default": ctx.cfg.agents.default_launcher, "skill_installed": poo::agent::status().iter().any(|t| t.skill_installed)}));
                } else {
                    for (l, a) in &launchers {
                        println!("{:<12} {:<14} {:<9} {}", l.id, l.name, l.kind, if *a { "installed" } else { "-" });
                    }
                }
                return Ok(());
            }
            let r = ops::report(&ctx, false, scan_opts(&out, false))?;
            clear_progress(&out);
            let b = poo::ask::brief(&r, &ctx.cat, &target).map_err(|e| CliError::new("not_found", 3, e.to_string()))?;
            let pick = with
                .or_else(|| ctx.cfg.agents.default_launcher.clone())
                .or_else(|| launchers.iter().find(|(_, a)| *a).map(|(l, _)| l.id.clone()))
                .ok_or_else(|| CliError::new("not_found", 3, "no agent launcher is installed").hint("copy the brief with `poo prompt <target>`"))?;
            let l = launchers.iter().find(|(l, _)| l.id == pick).map(|(l, _)| l.clone()).ok_or_else(|| CliError::new("not_found", 3, format!("no launcher `{}`", pick)))?;
            let msg = poo::ask::launch(&ctx.cfg, &l, &b)?;
            if out.json {
                emit(&out, &json!({"ok": true, "message": msg, "launcher": l.id}));
            } else {
                println!("{}", msg);
            }
        }
        Cmd::Hooks { cmd } => match cmd.unwrap_or(HooksCmd::Status) {
            HooksCmd::Status => {
                let st = poo::hooks::status();
                if out.json {
                    emit(&out, &json!({"targets": st, "on_agent_done": ctx.cfg.hooks.on_agent_done, "remove_landed_worktrees": ctx.cfg.hooks.remove_landed_worktrees}));
                } else {
                    for t in &st {
                        println!("{:<8} {:<9} {:<13} {}", t.id, if t.detected { "found" } else { "-" }, if t.installed { "installed" } else { "not installed" }, t.file);
                    }
                    println!("\nWhen an agent finishes in a worktree: {} (hooks.on_agent_done: landed | always | off)", ctx.cfg.hooks.on_agent_done);
                    println!("Hooks are off until you run `poo hooks install claude|codex|all`.");
                }
            }
            HooksCmd::Install { agent, yes } => {
                agent_gate(yes, format!("installing the {} hook makes poo clean a worktree's build output whenever that agent finishes there", agent))?;
                let ids: Vec<&str> = if agent == "all" { poo::hooks::status().iter().filter(|t| t.detected).map(|t| t.id).collect() } else { vec![agent.as_str()] };
                let mut done = vec![];
                for id in ids {
                    done.push(poo::hooks::install(id).map_err(|e| CliError::new("hooks", 1, format!("{:#}", e)))?);
                }
                if out.json { emit(&out, &json!({"done": done})); } else { for d in done { println!("{}", d); } }
            }
            HooksCmd::Uninstall { agent } => {
                let ids: Vec<&str> = if agent == "all" { vec!["claude", "codex"] } else { vec![agent.as_str()] };
                let mut done = vec![];
                for id in ids {
                    done.push(poo::hooks::uninstall(id).map_err(|e| CliError::new("hooks", 1, format!("{:#}", e)))?);
                }
                if out.json { emit(&out, &json!({"done": done})); } else { for d in done { println!("{}", d); } }
            }
            HooksCmd::Log { limit } => {
                let text = std::fs::read_to_string(poo::config::state_dir().join("hooks.log")).unwrap_or_default();
                let lines: Vec<&str> = text.lines().rev().take(limit).collect();
                if out.json { emit(&out, &json!({"log": lines})); } else { for l in lines.into_iter().rev() { println!("{}", l); } }
            }
        },
        Cmd::Hook { cmd: HookCmd::AgentDone { agent, cwd, detached } } => {
            // Hooks must never fail the agent: log and exit 0.
            if let Err(e) = poo::hooks::agent_done(&agent, cwd, detached) {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(poo::config::state_dir().join("hooks.log"))
                    .map(|mut f| writeln!(f, "{} {} error: {:#}", poo::util::iso8601(poo::util::now()), agent, e));
            }
        }
        Cmd::Watch { debounce, every } => {
            poo::watch::run(
                &mut ctx,
                &poo::watch::WatchOptions {
                    debounce: std::time::Duration::from_secs_f64(debounce.max(0.2)),
                    every: std::time::Duration::from_secs(every.max(30)),
                    json: out.json,
                },
            )?;
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

/// What a schedule does and would clean right now, for the user to approve.
fn schedule_summary(ctx: &mut Ctx, s: &poo::config::Schedule) -> String {
    let when = format!("every {}{}{}", s.every, s.at.as_ref().map(|a| format!(" at {}", a)).unwrap_or_default(), s.when_free_below.as_ref().map(|b| format!(" when free space is below {}", b)).unwrap_or_default());
    match poo::mcp::schedule_preview(ctx, s) {
        Ok(v) => {
            let w = &v["would_clean_now"];
            let names: Vec<&str> = w["items"].as_array().map(|a| a.iter().take(8).filter_map(|i| i["name"].as_str()).collect()).unwrap_or_default();
            format!("schedule `{}` runs plan `{}` {}; right now it would clean {} in {} items ({}). Scheduled runs skip items with risks", s.id, s.plan, when, bytes(w["bytes"].as_u64().unwrap_or(0)), w["count"], names.join(", "))
        }
        Err(_) => format!("schedule `{}` runs plan `{}` {}", s.id, s.plan, when),
    }
}

fn schedule_cmd(ctx: &mut Ctx, out: &Out, cmd: ScheduleCmd) -> R {
    use poo::schedule as sch;
    let save_sync = |cfg: &poo::config::Config, what: String| -> R {
        cfg.save()?;
        let note = sch::sync_agent(cfg).map_err(|e| CliError::new("schedule", 1, format!("{:#}", e)))?;
        if out.json {
            emit(out, &json!({"ok": true, "changed": what, "agent": note}));
        } else {
            println!("{}", what);
            if let Some(n) = note {
                println!("{}", n);
            }
        }
        Ok(())
    };
    match cmd {
        ScheduleCmd::Ls => {
            let state = sch::load_state();
            let list: Vec<Value> = ctx
                .cfg
                .schedule
                .iter()
                .map(|s| {
                    let last = state.last_run.get(&s.id);
                    json!({"schedule": s, "last_run": last, "next_run": s.enabled.then(|| sch::next_run(s, last.map(|l| l.at)))})
                })
                .collect();
            if out.json {
                emit(out, &json!({"schedules": list, "agent_installed": sch::agent_installed(), "agent_loaded": sch::agent_loaded(), "last_tick": state.last_tick}));
            } else if ctx.cfg.schedule.is_empty() {
                println!("No schedules. Try `poo schedule add nightly` (see `poo schedule presets`).");
            } else {
                let agent = if sch::agent_loaded() { "running" } else if sch::agent_installed() { "installed, not loaded" } else { "not installed" };
                render::schedules(&ctx.cfg.schedule, &state, agent);
            }
            Ok(())
        }
        ScheduleCmd::Presets => {
            let p: Vec<Value> = sch::presets().into_iter().map(|(id, d, s)| json!({"id": id, "description": d, "schedule": s})).collect();
            if out.json {
                emit(out, &json!({"presets": p}));
            } else {
                for (id, d, _) in sch::presets() {
                    println!("{:<8} {}", id, d);
                }
            }
            Ok(())
        }
        ScheduleCmd::Add { id, preset, plan, every, at, when_free_below, target_free, max, bucket, exclude_bucket, tag, exclude_tag, no_notify, yes } => {
            let base = preset
                .as_deref()
                .or(if plan.is_none() && every.is_none() { Some(id.as_str()) } else { None })
                .and_then(|p| sch::presets().into_iter().find(|(pid, _, _)| *pid == p).map(|(_, _, s)| s));
            let mut s = base.unwrap_or(poo::config::Schedule {
                id: id.clone(),
                plan: "stale".into(),
                every: "1d".into(),
                at: None,
                when_free_below: None,
                target_free: None,
                max_bytes: None,
                buckets: vec![],
                exclude_buckets: vec![],
                tags: vec![],
                exclude_tags: vec![],
                enabled: true,
                notify: true,
            });
            s.id = id.clone();
            if let Some(v) = plan { s.plan = v; }
            if let Some(v) = every { s.every = v; }
            if at.is_some() { s.at = at; }
            if when_free_below.is_some() { s.when_free_below = when_free_below; }
            if target_free.is_some() { s.target_free = target_free; }
            if max.is_some() { s.max_bytes = max; }
            if !bucket.is_empty() { s.buckets = bucket; }
            if !exclude_bucket.is_empty() { s.exclude_buckets = exclude_bucket; }
            if !tag.is_empty() { s.tags = tag; }
            if !exclude_tag.is_empty() { s.exclude_tags = exclude_tag; }
            if no_notify { s.notify = false; }
            sch::validate(&s, &ctx.cat).map_err(|e| CliError::new("usage", 2, e.to_string()))?;
            if !yes && poo::agent::agent_context().is_some() {
                agent_gate(false, schedule_summary(ctx, &s))?;
            }
            let mut cfg = ctx.cfg.clone();
            cfg.schedule.retain(|x| x.id != s.id);
            cfg.schedule.push(s.clone());
            save_sync(&cfg, format!("schedule `{}`: plan {} every {}{}", s.id, s.plan, s.every, s.at.map(|a| format!(" at {}", a)).unwrap_or_default()))
        }
        ScheduleCmd::Rm { id } | ScheduleCmd::Enable { id, .. } | ScheduleCmd::Disable { id } if !ctx.cfg.schedule.iter().any(|s| s.id == id) => {
            Err(CliError::new("not_found", 3, format!("no schedule `{}`", id)))
        }
        ScheduleCmd::Rm { id } => {
            let mut cfg = ctx.cfg.clone();
            cfg.schedule.retain(|s| s.id != id);
            save_sync(&cfg, format!("removed schedule `{}`", id))
        }
        ScheduleCmd::Enable { id, yes } => {
            if !yes && poo::agent::agent_context().is_some() {
                let s = ctx.cfg.schedule.iter().find(|s| s.id == id).cloned().unwrap();
                agent_gate(false, schedule_summary(ctx, &s))?;
            }
            let mut cfg = ctx.cfg.clone();
            cfg.schedule.iter_mut().filter(|s| s.id == id).for_each(|s| s.enabled = true);
            save_sync(&cfg, format!("enabled schedule `{}`", id))
        }
        ScheduleCmd::Disable { id } => {
            let mut cfg = ctx.cfg.clone();
            cfg.schedule.iter_mut().filter(|s| s.id == id).for_each(|s| s.enabled = false);
            save_sync(&cfg, format!("disabled schedule `{}`", id))
        }
        ScheduleCmd::Run { id, dry_run } => {
            let s = ctx.cfg.schedule.iter().find(|s| s.id == id).cloned().ok_or_else(|| CliError::new("not_found", 3, format!("no schedule `{}`", id)))?;
            let rec = sch::run(ctx, &s, dry_run)?;
            if out.json {
                emit(out, &serde_json::to_value(&rec).unwrap_or_default());
            } else {
                println!("{}: {} {} ({} cleaned, {} skipped, {} failed). {} free.", id, rec.status, bytes(rec.freed_bytes), rec.cleaned, rec.skipped, rec.failed, bytes(rec.free_after));
                if !rec.message.is_empty() {
                    println!("{}", rec.message);
                }
            }
            Ok(())
        }
        ScheduleCmd::Tick => {
            let done = sch::tick()?;
            if out.json {
                emit(out, &json!({"ran": done.iter().map(|(id, r)| json!({"id": id, "record": r})).collect::<Vec<_>>()}));
            }
            Ok(())
        }
        ScheduleCmd::Install => {
            let p = sch::install_agent()?;
            save_sync(&ctx.cfg, format!("installed {}", tilde(&p)))
        }
        ScheduleCmd::Uninstall => {
            sch::uninstall_agent()?;
            if !out.json {
                println!("scheduler agent removed; schedules stay in config.toml");
            }
            Ok(())
        }
        ScheduleCmd::Log { limit } => {
            let text = std::fs::read_to_string(poo::config::state_dir().join("scheduler.log")).unwrap_or_default();
            let lines: Vec<&str> = text.lines().rev().take(limit).collect();
            if out.json {
                emit(out, &json!({"log": lines}));
            } else {
                for l in lines.into_iter().rev() {
                    println!("{}", l);
                }
            }
            Ok(())
        }
    }
}

fn backups_cmd(ctx: &Ctx, out: &Out, cmd: BackupsCmd) -> R {
    use poo::backup;
    match cmd {
        BackupsCmd::Ls => {
            let list = backup::list();
            let total = backup::total_bytes(&list);
            if out.json {
                emit(out, &json!({"backups": list, "total_bytes": total, "dir": backup::dir(), "budget_bytes": ctx.cfg.backup.max_backup_bytes, "auto_prune_days": ctx.cfg.backup.auto_prune_days}));
            } else if list.is_empty() {
                println!("No backups. poo saves small ignored files, stashes and rescue branches here before removing worktrees.");
            } else {
                render::backups(&list, total, &ctx.cfg.backup);
            }
            Ok(())
        }
        BackupsCmd::Prune { ids, older_than, all, dry_run, yes } => {
            let days = match older_than {
                Some(s) => Some(s.trim_end_matches('d').parse::<u32>().map_err(|_| CliError::new("usage", 2, "use --older-than 30d"))?),
                None if all => Some(0),
                None => None,
            };
            if ids.is_empty() && days.is_none() {
                return Err(CliError::new("usage", 2, "pass backup ids, --older-than 30d or --all"));
            }
            if !dry_run && !yes && (!is_tty(0) || out.json) {
                return Err(CliError::new("needs_approval", 5, "pruning backups is permanent").hint("pass --yes"));
            }
            if !dry_run && !yes && !confirm("Prune these backups permanently (stashes are dropped, rescue branches deleted)?") {
                println!("Cancelled.");
                return Ok(());
            }
            let r = backup::prune(&ids, days, false, dry_run);
            if out.json {
                emit(out, &serde_json::to_value(&r).unwrap_or_default());
            } else {
                println!("{} {} backup(s), {}.", if dry_run { "Would prune" } else { "Pruned" }, r.pruned.len(), bytes(r.freed_bytes));
                for (id, e) in &r.failed {
                    println!("✗ {}: {}", id, e);
                }
            }
            Ok(())
        }
        BackupsCmd::Restore { id, yes } => {
            if !yes && !is_tty(0) {
                return Err(CliError::new("needs_approval", 5, "restoring writes files into the original folder").hint("pass --yes"));
            }
            let (msg, n) = backup::restore(&id)?;
            if out.json {
                emit(out, &json!({"ok": true, "message": msg, "files": n}));
            } else {
                println!("{}", msg);
            }
            Ok(())
        }
        BackupsCmd::Enforce => {
            let r = backup::enforce(&ctx.cfg.backup);
            if out.json {
                emit(out, &serde_json::to_value(&r).unwrap_or_default());
            } else {
                println!("Pruned {} backup(s), {}.", r.pruned.len(), bytes(r.freed_bytes));
            }
            Ok(())
        }
        BackupsCmd::Path => {
            println!("{}", backup::dir().display());
            Ok(())
        }
    }
}

fn select_rules(ctx: &Ctx, sel: &RuleSelect) -> Vec<String> {
    ops::select_rules(&ctx.cat, sel.id.as_deref(), sel.tag.as_deref(), sel.group.as_deref(), sel.bucket.as_deref(), sel.stack.as_deref())
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

mod contrib_cmd;

/// Refreshes skills poo installed so they match the new binary.
fn refresh_skills() {
    for t in poo::agent::status().into_iter().filter(|t| t.skill_installed && !t.skill_current) {
        let _ = poo::agent::install(t.id, true, false);
    }
}
