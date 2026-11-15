//! MCP server over stdio (JSON-RPC 2.0, one message per line).

use crate::clean::{self, CleanOptions};
use crate::model::{ProjectPolicy, Tier};
use crate::ops::{self, BucketEdit, Ctx};
use crate::plan::Filter;
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
        "tier": {"type": "string", "enum": ["junk", "cold", "warm", "hot", "review", "protected"]},
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
    json!([
        {"name": "overview", "description": "Disk usage, buckets with tier breakdown, plan sizes and the biggest items. Uses the cached scan when fresh.",
         "inputSchema": {"type": "object", "properties": {"rescan": {"type": "boolean", "description": "Force a new scan (takes 10-60s)"}}}},
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
            "force": {"type": "boolean", "description": "Allow hot or review items the user explicitly chose"}
         }}},
        {"name": "projects", "description": "Projects and worktrees with size, activity status and policy; plus repos grouped with their worktrees.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "catalog", "description": "Everything poo knows: buckets, plans, rules with descriptions and recommendations, providers, stacks, worktree homes, guards, mods.",
         "inputSchema": {"type": "object", "properties": {"bucket": {"type": "string"}}}},
        {"name": "explain_path", "description": "Which rules match a path, whether git ignores it, and how it appears in the report.",
         "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}},
        {"name": "configure", "description": "Change poo's configuration. Actions: add_root, remove_root, set_project_policy (path, policy: active|stale|protected|auto), disable_rule, enable_rule, set_rule (id, values: [\"warm_days=7\", \"bucket=work\"]), add_bucket / set_bucket (id, name, order, color, symbol, description, hidden), remove_bucket (id, move_to), set (key, value) for config keys like agents.auto_clean_max_tier, setup_done (by, notes).",
         "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string"}, "path": {"type": "string"}, "policy": {"type": "string"},
            "id": {"type": "string"}, "values": {"type": "array", "items": {"type": "string"}},
            "name": {"type": "string"}, "order": {"type": "integer"}, "color": {"type": "string"},
            "symbol": {"type": "string"}, "description": {"type": "string"}, "hidden": {"type": "boolean"},
            "move_to": {"type": "string"}, "key": {"type": "string"}, "value": {"type": "string"},
            "by": {"type": "string"}, "notes": {"type": "string"}
         }, "required": ["action"]}},
        {"name": "setup_brief", "description": "A brief for configuring poo to this user's workflows: machine, space, projects, duplication, everything poo knows, questions to ask and commands to apply.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "history", "description": "What poo cleaned before, newest first.",
         "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "default": 50}}}}
    ])
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
    }
}

fn call(ctx: &mut Ctx, name: &str, a: &Value) -> Result<Value> {
    let rescan = a.get("rescan").and_then(|v| v.as_bool()).unwrap_or(false);
    match name {
        "overview" => {
            let r = ops::report(ctx, rescan, ScanOptions::default())?;
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
            let p = ops::make_plan(ctx, &r, &id, target, &filter_from(a))?;
            Ok(json!({"plan": p.plan, "name": p.name, "bytes": p.bytes, "count": p.count, "target_bytes": p.target_bytes,
                      "items": p.items.iter().map(ops::compact).collect::<Vec<_>>()}))
        }
        "clean" => {
            let mut r = ops::report(ctx, false, ScanOptions::default())?;
            let dry = a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
            let approved = a.get("approved_by_user").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = a.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let items = if let Some(ids) = a.get("ids").and_then(|v| v.as_array()) {
                let keys: Vec<String> = ids.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                ops::resolve_ids(&r, &keys)?
            } else if let Some(p) = s(a, "plan") {
                let target = s(a, "free").and_then(|f| parse_bytes(&f));
                ops::make_plan(ctx, &r, &p, target, &Filter::default())?.items
            } else {
                return Err(anyhow!("pass ids or plan"));
            };
            let limit = ctx.cfg.agents.auto_clean_max_tier;
            let needs: Vec<&crate::model::Item> = items.iter().filter(|i| i.tier.rank() > limit.rank()).collect();
            if !dry && !needs.is_empty() && !approved {
                return Ok(json!({
                    "status": "needs_approval",
                    "message": format!("{} item(s) are above the autonomy tier `{}`. Show them to the user; call again with approved_by_user: true only after they approve.", needs.len(), limit.as_str()),
                    "items": needs.iter().map(|i| ops::compact(i)).collect::<Vec<_>>(),
                }));
            }
            let opts = CleanOptions {
                dry_run: dry,
                trash: false,
                max_tier: Some(if approved { Tier::Warm } else { limit }),
                force: force && approved,
                actor: "mcp".into(),
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
        "catalog" => {
            let mut v = ops::catalog_json(&ctx.cat);
            if let Some(b) = s(a, "bucket") {
                if let Some(rules) = v["rules"].as_array() {
                    v["rules"] = json!(rules.iter().filter(|r| r["bucket"] == b.as_str()).cloned().collect::<Vec<_>>());
                }
            }
            Ok(v)
        }
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
                    let id = need("id")?;
                    if ctx.cat.rule(&id).is_none() {
                        return Err(anyhow!("unknown rule `{}`", id));
                    }
                    let mut c = ctx.cfg.clone();
                    c.disabled_rules.retain(|r| r != &id);
                    if action == "disable_rule" {
                        c.disabled_rules.push(id.clone());
                    } else if let Some(o) = c.rule_overrides.get_mut(&id) {
                        o.enabled = Some(true);
                    } else {
                        c.rule_overrides.entry(id).or_default().enabled = Some(true);
                    }
                    c
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
                "set" => ops::set_config_key(&ctx.cfg, &need("key")?, &need("value")?)?,
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
        "history" => {
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            Ok(json!({"history": clean::history(limit)}))
        }
        _ => Err(anyhow!("unknown tool `{}`", name)),
    }
}

pub fn serve() -> Result<()> {
    let mut ctx = Ctx::load()?;
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
                let r = match call(&mut ctx, &name, &args) {
                    Ok(v) => json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}], "structuredContent": v, "isError": false}),
                    Err(e) => json!({"content": [{"type": "text", "text": format!("error: {}", e)}], "isError": true}),
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
