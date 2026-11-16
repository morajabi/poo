//! MCP server over stdio (JSON-RPC 2.0, one message per line).

use crate::clean::{self, CleanOptions};
use crate::model::{ProjectPolicy, RiskLevel, Tier};
use crate::ops::{self, BucketEdit, Ctx};
use crate::plan::{Admission, Filter};
use crate::scan::ScanOptions;
use crate::util::parse_bytes;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const INSTRUCTIONS: &str = "poo inspects and safely frees disk space on a developer machine. \
Start with `overview`. Use `list_items` and `inspect` to dig in, `plan` to preview a cleanup, and \
`clean` to run it (dry run by default). Never delete files with shell commands; poo re-checks every \
item and refuses uncommitted worktrees and running builds. Items above the configured autonomy tier \
need `approved_by_user: true`, which you may only pass after the user approved those exact items. \
To configure poo for the user, call `setup_brief` and follow it.";

fn tools() -> Value {
    let filter_props = json!({
        "bucket": {"type": "string", "description": "Bucket id, e.g. build, xcode, worktrees, deps, caches, ai, containers, system, junk, other"},
        "group": {"type": "string"},
        "tier": {"type": "string", "enum": ["junk", "cold", "idle", "warm", "hot", "review", "protected"]},
        "project": {"type": "string", "description": "Project, repo or path prefix"},
        "min_size": {"type": "string", "description": "Minimum size like 500M or 2G"},
        "query": {"type": "string", "description": "Text to match in name, path, rule or group"}
    });
    let mut list_props = filter_props.clone();
    list_props["sort"] = json!({"type": "string", "enum": ["size", "age", "tier", "name"]});
    list_props["limit"] = json!({"type": "integer", "default": 50});
    let mut plan_props = filter_props.clone();
    plan_props["plan"] = json!({"type": "string", "description": "quick, stale, deep or emergency (plans are configurable)"});
    plan_props["free"] = json!({"type": "string", "description": "Target to free, e.g. 30G. Makes the plan pick least-regret items until reached."});
    plan_props["mode"] = json!({"type": "string", "enum": ["safe", "in-use", "dangerous"], "default": "safe", "description": "Preview what clean would take in this mode; items it would refuse are left out."});
    let tools = json!([
        {"name": "overview", "description": "Disk usage, buckets with tier breakdown, plan sizes and the biggest items. Uses the cached scan when fresh.",
         "inputSchema": {"type": "object", "properties": {"rescan": {"type": "boolean", "description": "Update the report now (from the file-change journal, usually under a second; a full walk when the journal cannot help)"}, "full": {"type": "boolean", "description": "Walk everything (10-60s)"}}}},
        {"name": "list_items", "description": "Items (folders, simulators, images...) with size, tier, reasons. Filter by bucket, tier, project, size or text.",
         "inputSchema": {"type": "object", "properties": list_props}},
        {"name": "inspect", "description": "Full detail for one item by id or path: reasons, rebuild cost, rule, what is inside.",
         "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "description": "Item id or absolute path"}}, "required": ["id"]}},
        {"name": "plan", "description": "Preview a cleaning plan: which items it would clean and how much it frees.",
         "inputSchema": {"type": "object", "properties": plan_props, "required": ["plan"]}},
        {"name": "clean", "description": "Clean items by id, or a plan. Dry run unless dry_run is false. Items above the user's autonomy tier need approved_by_user after the user approved them.",
         "inputSchema": {"type": "object", "properties": {
            "ids": {"type": "array", "items": {"type": "string"}},
            "plan": {"type": "string"},
            "free": {"type": "string", "description": "With plan: target size"},
            "dry_run": {"type": "boolean", "default": true},
            "approved_by_user": {"type": "boolean", "default": false, "description": "Only after the user explicitly approved these items"},
            "mode": {"type": "string", "enum": ["safe", "in-use", "dangerous"], "default": "safe", "description": "in-use: also things in use (recent, agent open, uncommitted work which poo stashes first, check-first items). dangerous: also protected things (running builds, data, system paths, detached commits). Both need approved_by_user and the user's explicit OK for that mode."}
         }}},
        {"name": "projects", "description": "Projects and worktrees with size, activity status and policy; plus repos grouped with their worktrees.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "catalog", "description": "What poo knows. Without arguments: a summary (buckets with rule counts, plans, providers, stack ids, worktree homes, guards, mods). With bucket, tag, stack, id or query: the matching rules in full, with descriptions, recommendations and matchers.",
         "inputSchema": {"type": "object", "properties": {
            "bucket": {"type": "string", "description": "Rules in this bucket"},
            "tag": {"type": "string", "description": "Rules with this tag"},
            "stack": {"type": "string", "description": "Rules for this stack id"},
            "id": {"type": "string", "description": "One rule by id"},
            "query": {"type": "string", "description": "Text to match in rule id, name or description"}
         }}},
        {"name": "explain_path", "description": "Which rules match a path, whether git ignores it, and how it appears in the report.",
         "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}},
        {"name": "configure", "description": "Change poo's configuration. Actions: add_root, remove_root, set_project_policy (path, policy: active|stale|protected|auto), disable_rule / enable_rule (id, or tag/group/bucket/stack for many), set_rule (id, values: [\"warm_days=7\", \"bucket=work\"]), add_bucket / set_bucket (id, name, order, color, symbol, description, hidden), remove_bucket (id, move_to), set (key, value) for config keys like agents.auto_clean_max_tier (raising it needs approved_by_user), setup_done (by, notes). Propose changes to the user before applying them.",
         "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string"}, "path": {"type": "string"}, "policy": {"type": "string"},
            "id": {"type": "string"}, "values": {"type": "array", "items": {"type": "string"}},
            "name": {"type": "string"}, "order": {"type": "integer"}, "color": {"type": "string"},
            "symbol": {"type": "string"}, "description": {"type": "string"}, "hidden": {"type": "boolean"},
            "move_to": {"type": "string"}, "key": {"type": "string"}, "value": {"type": "string"},
            "tag": {"type": "string"}, "group": {"type": "string"}, "bucket": {"type": "string"}, "stack": {"type": "string"},
            "by": {"type": "string"}, "notes": {"type": "string"},
            "approved_by_user": {"type": "boolean", "default": false, "description": "Required to raise agents.auto_clean_max_tier; only after the user said yes"}
         }, "required": ["action"]}},
        {"name": "setup_brief", "description": "A brief for configuring poo to this user's workflows: machine, space, projects, duplication, everything poo knows, questions to ask and commands to apply.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "schedules", "description": "Routine cleanups run by a launchd agent (standard mode only, rescan first). Actions: list, presets, add (preset, or id with plan+every; optional at HH:MM, when_free_below, target_free, max_bytes, buckets, exclude_buckets, tags, exclude_tags), remove, enable, disable, run (id, dry_run). add and enable return a preview of what the schedule would clean until you pass approved_by_user: true, which you may only do after showing that preview to the user and getting a yes, even when they asked for the schedule.",
         "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["list", "presets", "add", "remove", "enable", "disable", "run"]},
            "id": {"type": "string"}, "preset": {"type": "string"}, "plan": {"type": "string"},
            "every": {"type": "string"}, "at": {"type": "string"}, "when_free_below": {"type": "string"},
            "target_free": {"type": "string"}, "max_bytes": {"type": "string"},
            "buckets": {"type": "array", "items": {"type": "string"}}, "exclude_buckets": {"type": "array", "items": {"type": "string"}},
            "tags": {"type": "array", "items": {"type": "string"}}, "exclude_tags": {"type": "array", "items": {"type": "string"}},
            "dry_run": {"type": "boolean", "default": true},
            "approved_by_user": {"type": "boolean", "default": false, "description": "For add and enable: only after the user saw the preview and said yes"}
         }, "required": ["action"]}},
        {"name": "backups", "description": "What poo saved before cleaning (copies of small ignored files, stashes of uncommitted work, rescue branches). Actions: list, prune (ids or older_than_days; permanent, ask the user first), restore (id; copies files back without overwriting).",
         "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["list", "prune", "restore"]},
            "ids": {"type": "array", "items": {"type": "string"}}, "older_than_days": {"type": "integer"},
            "id": {"type": "string"}, "dry_run": {"type": "boolean", "default": true}
         }, "required": ["action"]}},
        {"name": "growth", "description": "What grew since a point in time (per bucket and group), unusual growth such as a cache that grew many GB today, and a free-space forecast (days until full). Read-only.",
         "inputSchema": {"type": "object", "properties": {"since": {"type": "string", "description": "24h, 7d (default), 2w"}}}},
        {"name": "history", "description": "What poo cleaned before, newest first.",
         "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "default": 50}}}}
    ]);
    annotate(tools)
}

/// MCP tool annotations: what each tool may change. Everything stays on this Mac.
fn annotate(mut tools: Value) -> Value {
    for t in tools.as_array_mut().into_iter().flatten() {
        let (title, read_only) = match t["name"].as_str().unwrap_or("") {
            "overview" => ("Disk overview", true),
            "list_items" => ("List items", true),
            "inspect" => ("Inspect an item", true),
            "plan" => ("Preview a cleaning plan", true),
            "projects" => ("Projects and worktrees", true),
            "catalog" => ("Rules and buckets", true),
            "explain_path" => ("Explain a path", true),
            "setup_brief" => ("Setup brief", true),
            "growth" => ("Growth and forecast", true),
            "history" => ("Cleaning history", true),
            "clean" => ("Clean items", false),
            "configure" => ("Change configuration", false),
            "schedules" => ("Scheduled cleanups", false),
            "backups" => ("Backups", false),
            _ => ("", true),
        };
        let mut a = json!({"title": title, "readOnlyHint": read_only, "openWorldHint": false});
        if !read_only {
            a["destructiveHint"] = json!(true);
            a["idempotentHint"] = json!(false);
        }
        t["annotations"] = a;
    }
    tools
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from)
}

fn filter_from(a: &Value) -> Filter {
    Filter {
        bucket: s(a, "bucket"),
        group: s(a, "group"),
        project: s(a, "project").map(|p| crate::util::expand(&p).to_string_lossy().to_string()),
        tier: s(a, "tier").and_then(|t| Tier::parse(&t)),
        min_bytes: s(a, "min_size").and_then(|m| parse_bytes(&m)).unwrap_or(0),
        query: s(a, "query"),
        ..Default::default()
    }
}

fn mode_from(a: &Value) -> RiskLevel {
    match s(a, "mode").as_deref() {
        Some("in-use") | Some("force") => RiskLevel::Force,
        Some("dangerous") | Some("danger") => RiskLevel::Danger,
        _ => RiskLevel::Standard,
    }
}

fn call(ctx: &mut Ctx, name: &str, a: &Value) -> Result<Value> {
    let full = a.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
    let rescan = full || a.get("rescan").and_then(|v| v.as_bool()).unwrap_or(false);
    match name {
        "growth" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            let since = s(a, "since").unwrap_or_else(|| "7d".into());
            let secs = crate::growth::parse_since(&since).ok_or_else(|| anyhow!("since is like 24h, 7d or 2w"))?;
            Ok(crate::growth::report(&r, secs))
        }
        "overview" => {
            let r = ops::report(ctx, rescan, ScanOptions { full, ..Default::default() })?;
            let mut v = ops::overview(&r);
            v["setup_completed"] = json!(ctx.cfg.setup.completed_at.is_some());
            v["autonomy_max_tier"] = json!(ctx.cfg.agents.auto_clean_max_tier);
            Ok(v)
        }
        "list_items" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let sort = s(a, "sort").unwrap_or_else(|| "size".into());
            let f = filter_from(a);
            let all = ops::list(&r, &f, &sort, 0);
            let total: u64 = all.iter().map(|i| i.bytes).sum();
            Ok(json!({"count": all.len(), "total_bytes": total, "items": all.iter().take(limit).map(|i| ops::compact(i)).collect::<Vec<_>>()}))
        }
        "inspect" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            let key = s(a, "id").ok_or_else(|| anyhow!("id is required"))?;
            let item = ops::find_item(&r, &key).ok_or_else(|| anyhow!("no item `{}`", key))?;
            let rule = ctx.cat.rule(&item.rule);
            Ok(json!({"item": item, "rule": rule.map(|r| json!({"id": r.id, "name": r.name, "description": r.description, "recommend": r.recommend, "rebuild": r.rebuild, "source": r.source}))}))
        }
        "plan" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            let id = s(a, "plan").ok_or_else(|| anyhow!("plan is required"))?;
            let target = s(a, "free").and_then(|f| parse_bytes(&f));
            let p = ops::make_plan(ctx, &r, &id, target, &filter_from(a), Admission::mode(mode_from(a)))?;
            Ok(json!({"plan": p.plan, "name": p.name, "bytes": p.bytes, "count": p.count, "target_bytes": p.target_bytes,
                      "items": p.items.iter().map(ops::compact).collect::<Vec<_>>()}))
        }
        "clean" => {
            let mut r = ops::report(ctx, false, ScanOptions::default())?;
            let dry = a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
            let approved = a.get("approved_by_user").and_then(|v| v.as_bool()).unwrap_or(false);
            let mode = mode_from(a);
            if mode > RiskLevel::Standard && !approved && !dry {
                return Ok(json!({"status": "needs_approval", "message": format!("mode `{}` needs approved_by_user: true after the user explicitly agreed to it", mode.as_str())}));
            }
            let items = if let Some(ids) = a.get("ids").and_then(|v| v.as_array()) {
                let keys: Vec<String> = ids.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                ops::resolve_ids(&r, &keys)?
            } else if let Some(p) = s(a, "plan") {
                let target = s(a, "free").and_then(|f| parse_bytes(&f));
                // Items above the autonomy tier stay in, so they can be shown for approval.
                ops::make_plan(ctx, &r, &p, target, &Filter::default(), Admission::mode(mode))?.items
            } else {
                return Err(anyhow!("pass ids or plan"));
            };
            let limit = ctx.cfg.agents.auto_clean_max_tier;
            let needs: Vec<&crate::model::Item> = items.iter().filter(|i| i.tier.rank() > limit.rank()).collect();
            if !dry && !needs.is_empty() && !approved {
                return Ok(json!({
                    "status": "needs_approval",
                    "message": format!("{} above the autonomy tier `{}`. Show them to the user; call again with approved_by_user: true only after they approve.", if needs.len() == 1 { "1 item is".to_string() } else { format!("{} items are", needs.len()) }, limit.as_str()),
                    "items": needs.iter().map(|i| ops::compact(i)).collect::<Vec<_>>(),
                }));
            }
            let mut opts = CleanOptions::from_config(&ctx.cfg, mode, dry, "mcp");
            opts.max_tier = Some(if approved { Tier::Warm } else { limit });
            let _lock = if dry { None } else {
                Some(crate::schedule::RunLock::try_acquire().ok_or_else(|| anyhow!("another poo clean or scheduled run is in progress"))?)
            };
            let res = clean::clean(&items, &ctx.cat, &opts);
            if !dry {
                ops::forget_cleaned(ctx, &mut r, &res);
            }
            Ok(serde_json::to_value(res)?)
        }
        "projects" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            Ok(json!({"projects": r.projects, "worktrees": ops::worktrees(&r)["repos"]}))
        }
        "catalog" => Ok(catalog(ctx, a)),
        "explain_path" => {
            let p = s(a, "path").ok_or_else(|| anyhow!("path is required"))?;
            let r = crate::scan::load_report();
            Ok(ops::explain(ctx, r.as_ref(), &p))
        }
        "configure" => {
            let action = s(a, "action").unwrap_or_default();
            let need = |k: &str| s(a, k).ok_or_else(|| anyhow!("`{}` is required for {}", k, action));
            let cfg = match action.as_str() {
                "add_root" => ops::add_root(&ctx.cfg, &need("path")?)?,
                "remove_root" => ops::remove_root(&ctx.cfg, &need("path")?)?,
                "set_project_policy" => {
                    let pol = ProjectPolicy::parse(&need("policy")?).ok_or_else(|| anyhow!("policy is active, stale, protected or auto"))?;
                    ops::set_project_policy(&ctx.cfg, &need("path")?, pol, s(a, "notes"))?
                }
                "disable_rule" | "enable_rule" => {
                    let ids = ops::select_rules(&ctx.cat, s(a, "id").as_deref(), s(a, "tag").as_deref(), s(a, "group").as_deref(), s(a, "bucket").as_deref(), s(a, "stack").as_deref());
                    if ids.is_empty() {
                        return Err(anyhow!("no rule matches; pass id, tag, group, bucket or stack"));
                    }
                    ops::toggle_rules(&ctx.cfg, &ids, action == "enable_rule")
                }
                "set_rule" => {
                    let vals: Vec<String> = a.get("values").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    ops::set_rule(ctx, &need("id")?, &vals)?
                }
                "add_bucket" | "set_bucket" => {
                    let e = BucketEdit {
                        name: s(a, "name"),
                        order: a.get("order").and_then(|v| v.as_i64()).map(|v| v as i32),
                        color: s(a, "color"),
                        symbol: s(a, "symbol"),
                        description: s(a, "description"),
                        hidden: a.get("hidden").and_then(|v| v.as_bool()),
                    };
                    ops::set_bucket(ctx, &need("id")?, e, action == "add_bucket")?
                }
                "remove_bucket" => ops::remove_bucket(ctx, &need("id")?, s(a, "move_to").as_deref())?,
                "set" => {
                    let (k, v) = (need("key")?, need("value")?);
                    let raises = k == "agents.auto_clean_max_tier" && Tier::parse(&v).map_or(false, |t| t.rank() > ctx.cfg.agents.auto_clean_max_tier.rank());
                    if raises && !a.get("approved_by_user").and_then(|x| x.as_bool()).unwrap_or(false) {
                        return Ok(json!({"status": "needs_approval", "message": format!("Not changed. Raising agents.auto_clean_max_tier from {} to {} lets agents clean more without asking; call again with approved_by_user: true after the user agrees.", ctx.cfg.agents.auto_clean_max_tier.as_str(), v)}));
                    }
                    ops::set_config_key(&ctx.cfg, &k, &v)?
                }
                "setup_done" => {
                    let mut c = ctx.cfg.clone();
                    c.setup.completed_at = Some(crate::util::now());
                    c.setup.by = s(a, "by");
                    c.setup.notes = s(a, "notes");
                    c
                }
                _ => return Err(anyhow!("unknown action `{}`", action)),
            };
            cfg.save()?;
            ctx.reload()?;
            Ok(json!({"ok": true, "note": ops::retier_hint(), "errors": ctx.cat.errors}))
        }
        "setup_brief" => {
            let r = ops::report(ctx, false, ScanOptions::default())?;
            Ok(json!({"brief": crate::agent::setup_brief(&ctx.cfg, &ctx.cat, &r)}))
        }
        "schedules" => schedules_tool(ctx, a),
        "backups" => {
            let action = s(a, "action").unwrap_or_else(|| "list".into());
            match action.as_str() {
                "list" => {
                    let l = crate::backup::list();
                    Ok(json!({"backups": l, "total_bytes": crate::backup::total_bytes(&l), "budget_bytes": ctx.cfg.backup.max_backup_bytes}))
                }
                "prune" => {
                    let ids: Vec<String> = a.get("ids").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    let days = a.get("older_than_days").and_then(|v| v.as_u64()).map(|d| d as u32);
                    let dry = a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
                    Ok(serde_json::to_value(crate::backup::prune(&ids, days, false, dry))?)
                }
                "restore" => {
                    let id = s(a, "id").ok_or_else(|| anyhow!("id is required"))?;
                    let (m, n) = crate::backup::restore(&id)?;
                    Ok(json!({"ok": true, "message": m, "files": n}))
                }
                _ => Err(anyhow!("unknown action `{}`", action)),
            }
        }
        "history" => {
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            Ok(json!({"history": clean::history(limit)}))
        }
        _ => Err(anyhow!("unknown tool `{}`", name)),
    }
}

/// The catalog for agents: a summary by default (all rules are over a megabyte), the
/// matching rules in full when filtered.
fn catalog(ctx: &Ctx, a: &Value) -> Value {
    let mut v = ops::catalog_json(&ctx.cat);
    let rules = v["rules"].as_array().cloned().unwrap_or_default();
    let (bucket, tag, stack, id, query) = (s(a, "bucket"), s(a, "tag"), s(a, "stack"), s(a, "id"), s(a, "query").map(|q| q.to_lowercase()));
    if bucket.is_some() || tag.is_some() || stack.is_some() || id.is_some() || query.is_some() {
        let has = |r: &Value, k: &str, x: &str| r[k].as_array().map_or(false, |l| l.iter().any(|t| t == x));
        let keep: Vec<Value> = rules
            .into_iter()
            .filter(|r| bucket.as_deref().map_or(true, |b| r["bucket"] == b))
            .filter(|r| tag.as_deref().map_or(true, |t| has(r, "tags", t)))
            .filter(|r| stack.as_deref().map_or(true, |x| has(r, "stacks", x)))
            .filter(|r| id.as_deref().map_or(true, |i| r["id"] == i))
            .filter(|r| {
                query.as_deref().map_or(true, |q| {
                    ["id", "name", "description"].iter().any(|k| r[*k].as_str().map_or(false, |t| t.to_lowercase().contains(q)))
                })
            })
            .collect();
        return json!({"count": keep.len(), "rules": keep});
    }
    let mut per_bucket: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &rules {
        *per_bucket.entry(r["bucket"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    if let Some(bs) = v["buckets"].as_array_mut() {
        for b in bs {
            b["rule_count"] = json!(per_bucket.get(b["id"].as_str().unwrap_or("")).copied().unwrap_or(0));
        }
    }
    let obj = v.as_object_mut().unwrap();
    obj.remove("rules");
    obj.insert("rule_count".into(), json!(rules.len()));
    obj.insert("stacks".into(), json!(ctx.cat.stacks.iter().map(|x| x.id.clone()).collect::<Vec<_>>()));
    obj.insert("note".into(), json!("Rules are left out of this summary; call catalog with bucket, tag, stack, id or query for them in full."));
    v
}

fn schedules_tool(ctx: &mut Ctx, a: &Value) -> Result<Value> {
    use crate::schedule as sch;
    let action = s(a, "action").unwrap_or_else(|| "list".into());
    let approved = a.get("approved_by_user").and_then(|v| v.as_bool()).unwrap_or(false);
    let list_of = |k: &str| -> Vec<String> {
        a.get(k).and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let save = |ctx: &mut Ctx, cfg: crate::config::Config| -> Result<Value> {
        cfg.save()?;
        let note = sch::sync_agent(&cfg)?;
        ctx.reload()?;
        Ok(json!({"ok": true, "agent": note, "schedules": ctx.cfg.schedule}))
    };
    match action.as_str() {
        "list" => {
            let state = sch::load_state();
            Ok(json!({"schedules": ctx.cfg.schedule.iter().map(|x| {
                let last = state.last_run.get(&x.id);
                json!({"schedule": x, "last_run": last, "next_run": sch::next_run(x, last.map(|l| l.at))})
            }).collect::<Vec<_>>(), "agent_loaded": sch::agent_loaded()}))
        }
        "presets" => Ok(json!({"presets": sch::presets().into_iter().map(|(id, d, x)| json!({"id": id, "description": d, "schedule": x})).collect::<Vec<_>>()})),
        "add" => {
            let id = s(a, "id").or_else(|| s(a, "preset")).ok_or_else(|| anyhow!("pass a preset (nightly, quick, guard, weekly) or an id with plan and every"))?;
            let preset = s(a, "preset").unwrap_or_else(|| id.clone());
            let mut x = sch::presets().into_iter().find(|(p, _, _)| *p == preset).map(|(_, _, x)| x).unwrap_or(crate::config::Schedule {
                id: id.clone(), plan: "stale".into(), every: "1d".into(), at: None, when_free_below: None, target_free: None,
                max_bytes: None, buckets: vec![], exclude_buckets: vec![], tags: vec![], exclude_tags: vec![], enabled: true, notify: true,
            });
            x.id = id;
            if let Some(v) = s(a, "plan") { x.plan = v; }
            if let Some(v) = s(a, "every") { x.every = v; }
            if let Some(v) = s(a, "at") { x.at = Some(v); }
            if let Some(v) = s(a, "when_free_below") { x.when_free_below = Some(v); }
            if let Some(v) = s(a, "target_free") { x.target_free = Some(v); }
            if let Some(v) = s(a, "max_bytes") { x.max_bytes = Some(v); }
            for (k, f) in [("buckets", &mut x.buckets), ("exclude_buckets", &mut x.exclude_buckets), ("tags", &mut x.tags), ("exclude_tags", &mut x.exclude_tags)] {
                let v = list_of(k);
                if !v.is_empty() { *f = v; }
            }
            sch::validate(&x, &ctx.cat)?;
            if !approved {
                return schedule_preview(ctx, &x);
            }
            let mut cfg = ctx.cfg.clone();
            cfg.schedule.retain(|y| y.id != x.id);
            cfg.schedule.push(x);
            save(ctx, cfg)
        }
        "remove" | "enable" | "disable" => {
            let id = s(a, "id").ok_or_else(|| anyhow!("id is required"))?;
            if !ctx.cfg.schedule.iter().any(|y| y.id == id) {
                return Err(anyhow!("no schedule `{}`", id));
            }
            if action == "enable" && !approved {
                let x = ctx.cfg.schedule.iter().find(|y| y.id == id).cloned().unwrap();
                return schedule_preview(ctx, &x);
            }
            let mut cfg = ctx.cfg.clone();
            match action.as_str() {
                "remove" => cfg.schedule.retain(|y| y.id != id),
                "enable" => cfg.schedule.iter_mut().filter(|y| y.id == id).for_each(|y| y.enabled = true),
                _ => cfg.schedule.iter_mut().filter(|y| y.id == id).for_each(|y| y.enabled = false),
            }
            save(ctx, cfg)
        }
        "run" => {
            let id = s(a, "id").ok_or_else(|| anyhow!("id is required"))?;
            let x = ctx.cfg.schedule.iter().find(|y| y.id == id).cloned().ok_or_else(|| anyhow!("no schedule `{}`", id))?;
            let dry = a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
            Ok(serde_json::to_value(sch::run(ctx, &x, dry)?)?)
        }
        _ => Err(anyhow!("unknown action `{}`", action)),
    }
}

/// What a schedule would clean if it ran now, for the user to approve before it is saved.
pub fn schedule_preview(ctx: &mut Ctx, x: &crate::config::Schedule) -> Result<Value> {
    let r = ops::report(ctx, false, ScanOptions::default())?;
    let filter = Filter {
        buckets: x.buckets.clone(),
        exclude_buckets: x.exclude_buckets.clone(),
        tags: x.tags.clone(),
        exclude_tags: x.exclude_tags.clone(),
        ..Default::default()
    };
    let p = ops::make_plan(ctx, &r, &x.plan, None, &filter, Admission::default())?;
    Ok(json!({
        "status": "needs_approval",
        "message": format!("Not saved. Show the user this schedule and what it would clean right now ({} in {}; scheduled runs skip items with risks), then call again with approved_by_user: true after they say yes.", crate::util::bytes(p.bytes), crate::util::plural(p.count as u64, "item")),
        "schedule": x,
        "would_clean_now": {"plan": p.plan, "bytes": p.bytes, "count": p.count, "items": p.items.iter().take(15).map(ops::compact).collect::<Vec<_>>()},
    }))
}

/// Hints for common tool errors, like the CLI's `{error:{kind,message,hint}}`.
fn error_json(msg: &str) -> Value {
    let (kind, hint) = if msg.starts_with("no item") || msg.starts_with("no items") {
        ("not_found", "ids come from overview, list_items or plan; inspect also takes an absolute path. Rescan with overview {rescan: true} if the disk changed.")
    } else if msg.starts_with("no schedule") {
        ("not_found", "schedules {action: list} shows the ids")
    } else if msg.contains("config.toml") {
        ("invalid_config", "show the user the error, then fix the TOML in config.toml or move it aside; poo retries loading it on every call")
    } else if msg.contains("is required") || msg.starts_with("unknown") || msg.starts_with("pass ") {
        ("usage", "check this tool's inputSchema")
    } else {
        ("error", "")
    };
    json!({"error": {"kind": kind, "message": msg, "hint": if hint.is_empty() { Value::Null } else { json!(hint) }}})
}

pub fn serve() -> Result<()> {
    // A broken config must not kill the server: report it on each call and retry the load.
    let mut ctx: Option<Ctx> = Ctx::load().ok();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}});
                writeln!(out, "{}", err)?;
                out.flush()?;
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result: Option<std::result::Result<Value, (i64, String)>> = match method {
            "initialize" => {
                let proto = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-06-18").to_string();
                Some(Ok(json!({
                    "protocolVersion": proto,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "poo", "version": crate::VERSION},
                    "instructions": INSTRUCTIONS,
                })))
            }
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({"tools": tools()}))),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if ctx.is_none() {
                    ctx = Ctx::load().ok();
                }
                let res = match ctx.as_mut() {
                    Some(c) => call(c, &name, &args),
                    None => Ctx::load().map(|_| Value::Null).map_err(|e| anyhow!("{:#}", e)),
                };
                let r = match res {
                    Ok(v) => {
                        // Compact JSON: the same data in fewer tokens. The setup brief is markdown.
                        let text = v.get("brief").and_then(|b| b.as_str()).map(String::from).unwrap_or_else(|| serde_json::to_string(&v).unwrap_or_default());
                        json!({"content": [{"type": "text", "text": text}], "structuredContent": v, "isError": false})
                    }
                    Err(e) => {
                        let ej = error_json(&format!("{:#}", e));
                        let text = format!("error: {}{}", ej["error"]["message"].as_str().unwrap_or(""), ej["error"]["hint"].as_str().map(|h| format!("\nhint: {}", h)).unwrap_or_default());
                        json!({"content": [{"type": "text", "text": text}], "structuredContent": ej, "isError": true})
                    }
                };
                Some(Ok(r))
            }
            m if m.starts_with("notifications/") => None,
            _ => Some(Err((-32601, format!("method not found: {}", method)))),
        };
        if let (Some(res), Some(id)) = (result, id) {
            let msg = match res {
                Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
                Err((code, m)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}}),
            };
            writeln!(out, "{}", msg)?;
            out.flush()?;
        }
    }
    Ok(())
}
