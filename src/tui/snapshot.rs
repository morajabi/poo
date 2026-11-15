//! Renders every screen into a test buffer at several sizes. With POO_TUI_SNAP_DIR set,
//! also writes each screen as ANSI text and a cell dump (for PNG screenshots), using the
//! report at POO_TUI_SNAP_REPORT when given. Nothing here touches the disk otherwise.

use super::app::*;
use super::theme::Theme;
use super::view;
use crate::clean::{CleanProgress, ItemResult};
use crate::model::*;
use crate::ops::Ctx;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use serde_json::json;
use std::time::Instant;

fn fixture() -> Report {
    let now = crate::util::now();
    let item = |id: &str, name: &str, bucket: &str, group: &str, b: u64, tier: &str, days: i64, project: &str| {
        json!({
            "id": id, "path": format!("/Users/me/dev/{}/{}", project, name), "name": name, "rule": "rust.target",
            "bucket": bucket, "group": group, "bytes": b, "total_bytes": b, "files": b / 40000,
            "last_used": now - days * 86400, "tier": tier, "cost": "rebuild",
            "reasons": [format!("untouched for {} days", days)], "rebuild": "cargo build",
            "project": format!("/Users/me/dev/{}", project), "action": {"kind": "delete"},
            "children": [{"name": "debug", "bytes": b * 2 / 3}, {"name": "release", "bytes": b / 3}],
        })
    };
    let items = vec![
        item("a1", "target", "build", "Rust target", 14_200_000_000, "cold", 21, "engine"),
        item("a2", "target", "build", "Rust target", 6_100_000_000, "warm", 0, "poo"),
        item("a3", "node_modules", "deps", "node_modules", 2_300_000_000, "idle", 9, "site"),
        item("a4", "DerivedData", "xcode", "DerivedData", 31_000_000_000, "cold", 40, "app"),
        item("a5", "iOS 17.2 Simulator", "xcode", "Simulators", 8_000_000_000, "review", 200, "sim"),
        item("a6", ".next", "build", "Next.js", 900_000_000, "hot", 0, "site"),
    ];
    let mut r: serde_json::Value = json!({
        "schema": SCHEMA, "version": crate::VERSION, "scanned_at": now - 40, "duration_ms": 900,
        "disk": {"total": 994_000_000_000u64, "free": 61_000_000_000u64, "used": 933_000_000_000u64},
        "roots": ["/Users/me/dev"], "total_bytes": 62_500_000_000u64, "buckets": [], "plans": [], "projects": [
            {"path": "/Users/me/dev/engine", "name": "engine", "kind": "repo", "stacks": ["rust"], "last_activity": now - 21 * 86400, "artifact_bytes": 14_200_000_000u64, "item_count": 1, "policy": "auto", "status": "stale"},
            {"path": "/Users/me/dev/poo", "name": "poo", "kind": "repo", "stacks": ["rust", "swift"], "last_activity": now - 300, "artifact_bytes": 6_100_000_000u64, "item_count": 1, "policy": "auto", "status": "active", "active": ["claude"]},
            {"path": "/Users/me/dev/poo-wt", "name": "poo-wt", "kind": "worktree", "repo": "/Users/me/dev/poo", "branch": "feature/tui", "stacks": ["rust"], "last_activity": now - 3 * 86400, "artifact_bytes": 2_000_000_000u64, "item_count": 1, "policy": "auto", "status": "idle", "merged": true},
        ],
        "items": items, "warnings": [],
    });
    let buckets = [("build", "Build output", "#4f9d69"), ("deps", "Dependencies", "#d08c3f"), ("xcode", "Xcode", "#4a7fd0")];
    let its: Vec<Item> = serde_json::from_value(r["items"].clone()).unwrap();
    let mut bs = vec![];
    for (i, (id, name, color)) in buckets.iter().enumerate() {
        let mine: Vec<&Item> = its.iter().filter(|x| x.bucket == *id).collect();
        let mut tiers = std::collections::BTreeMap::new();
        for x in &mine {
            *tiers.entry(x.tier.as_str().to_string()).or_insert(0u64) += x.bytes;
        }
        bs.push(json!({"id": id, "name": name, "order": i, "color": color, "symbol": "", "description": format!("{} found in your projects", name),
            "hidden": false, "bytes": mine.iter().map(|x| x.bytes).sum::<u64>(), "count": mine.len(), "tiers": tiers, "groups": []}));
    }
    r["buckets"] = json!(bs);
    let mut rep: Report = serde_json::from_value(r).unwrap();
    rep.plans = vec![
        PlanSummary { id: "quick".into(), name: "Quick".into(), description: "Junk and caches, nothing to rebuild".into(), bytes: 2_300_000_000, count: 1, target_bytes: None },
        PlanSummary { id: "stale".into(), name: "Stale".into(), description: "Builds and deps nobody used for a while".into(), bytes: 45_200_000_000, count: 2, target_bytes: None },
    ];
    rep
}

fn load_report() -> Report {
    std::env::var("POO_TUI_SNAP_REPORT")
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(fixture)
}

fn color(c: Color) -> String {
    match c {
        Color::Reset => "reset".into(),
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        Color::Indexed(i) => format!("i{}", i),
        other => format!("{:?}", other).to_lowercase(),
    }
}

fn ansi_code(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 40 } else { 30 };
    Some(match c {
        Color::Reset => return None,
        Color::Black => format!("{}", base),
        Color::Red => format!("{}", base + 1),
        Color::Green => format!("{}", base + 2),
        Color::Yellow => format!("{}", base + 3),
        Color::Blue => format!("{}", base + 4),
        Color::Magenta => format!("{}", base + 5),
        Color::Cyan => format!("{}", base + 6),
        Color::Gray => format!("{}", base + 7),
        Color::Indexed(i) => format!("{};5;{}", base + 8, i),
        Color::Rgb(r, g, b) => format!("{};2;{};{};{}", base + 8, r, g, b),
        _ => return None,
    })
}

fn dump(buf: &Buffer, dir: &std::path::Path, name: &str) {
    let area = buf.area;
    let mut ansi = String::new();
    let mut text = String::new();
    let mut rows = vec![];
    for y in 0..area.height {
        let mut row = vec![];
        let mut skip = 0;
        for x in 0..area.width {
            let c = &buf[(x, y)];
            let m = c.modifier;
            row.push(json!([c.symbol(), color(c.fg), color(c.bg), m.contains(Modifier::BOLD), m.contains(Modifier::DIM), m.contains(Modifier::UNDERLINED), m.contains(Modifier::REVERSED)]));
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = super::fmt::cells(c.symbol()).saturating_sub(1);
            let mut codes = vec!["0".to_string()];
            if m.contains(Modifier::BOLD) {
                codes.push("1".into());
            }
            if m.contains(Modifier::DIM) {
                codes.push("2".into());
            }
            if m.contains(Modifier::UNDERLINED) {
                codes.push("4".into());
            }
            if m.contains(Modifier::REVERSED) {
                codes.push("7".into());
            }
            codes.extend(ansi_code(c.fg, false));
            codes.extend(ansi_code(c.bg, true));
            ansi.push_str(&format!("\x1b[{}m{}", codes.join(";"), c.symbol()));
            text.push_str(c.symbol());
        }
        ansi.push_str("\x1b[0m\n");
        text.push('\n');
        rows.push(row);
    }
    std::fs::write(dir.join(format!("{}.ansi", name)), ansi).unwrap();
    std::fs::write(dir.join(format!("{}.txt", name)), text).unwrap();
    std::fs::write(dir.join(format!("{}.cells.json", name)), serde_json::to_string(&json!({"width": area.width, "height": area.height, "rows": rows})).unwrap()).unwrap();
}

fn render(app: &mut App, th: &Theme, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| view::draw(f, app, th)).unwrap();
    t.backend().buffer().clone()
}

fn text_of(b: &Buffer) -> String {
    let mut s = String::new();
    for y in 0..b.area.height {
        for x in 0..b.area.width {
            s.push_str(b[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

/// Builds every screen state the TUI has, in order, with a name.
fn screens(app: &mut App) -> Vec<(&'static str, Box<dyn Fn(&mut App)>)> {
    let _ = app;
    vec![
        ("overview", Box::new(|a: &mut App| a.tab = Tab::Overview)),
        ("browse", Box::new(|a: &mut App| a.tab = Tab::Browse)),
        (
            "browse-items",
            Box::new(|a: &mut App| {
                a.tab = Tab::Browse;
                // Drill into the biggest bucket, then its biggest group.
                let k = |c| crossterm::event::KeyEvent::new(c, crossterm::event::KeyModifiers::NONE);
                a.nav.truncate(1);
                a.on_key(k(crossterm::event::KeyCode::Enter));
                a.on_key(k(crossterm::event::KeyCode::Enter));
                a.on_key(k(crossterm::event::KeyCode::Char(' ')));
            }),
        ),
        ("projects", Box::new(|a: &mut App| a.tab = Tab::Projects)),
        ("worktrees", Box::new(|a: &mut App| a.tab = Tab::Worktrees)),
        ("plans", Box::new(|a: &mut App| a.tab = Tab::Plans)),
        ("help", Box::new(|a: &mut App| a.modal = Modal::Help)),
        (
            "review",
            Box::new(|a: &mut App| {
                let items: Vec<Item> = a.plans.iter().max_by_key(|p| p.bytes).map(|p| p.items.iter().take(14).cloned().collect()).unwrap_or_default();
                let items = if items.is_empty() { a.report.as_ref().unwrap().items.iter().take(6).cloned().collect() } else { items };
                a.open_review(items, "Plan · Stale".into());
            }),
        ),
        (
            "cleaning",
            Box::new(|a: &mut App| {
                a.modal = Modal::Cleaning(Cleaning {
                    total: 12,
                    total_bytes: 48_300_000_000,
                    progress: Some(CleanProgress { event: "clean-progress", done: 7, total: 12, freed_bytes: 29_100_000_000, total_bytes: 48_300_000_000, current: "DerivedData · App-gqzxdhbnwfle".into(), status: "working".into() }),
                    started: Instant::now(),
                    cancel_requested: false,
                    dry: false,
                });
                a.tick = 37;
            }),
        ),
        (
            "summary",
            Box::new(|a: &mut App| {
                let r = |name: &str, b: u64, st: &str, reason: Option<&str>, notes: Vec<&str>| ItemResult {
                    id: name.into(),
                    path: String::new(),
                    name: name.into(),
                    status: st.into(),
                    bytes: b,
                    reason: reason.map(String::from),
                    warnings: vec![],
                    notes: notes.into_iter().map(String::from).collect(),
                };
                let mut res = crate::clean::clean_with(&[], &a.ctx.cat, &a.clean_options(RiskLevel::Standard, false, true), None);
                res.dry_run = false;
                res.background_delete = true;
                res.freed_bytes = 46_900_000_000;
                res.cleaned = 11;
                res.skipped = 1;
                res.disk_free_before = 61_000_000_000;
                res.disk_free_after = 107_900_000_000;
                res.results = vec![
                    r("DerivedData · App", 31_000_000_000, "cleaned", None, vec![]),
                    r("engine/target", 14_200_000_000, "cleaned", None, vec![]),
                    r("poo-wt (worktree)", 1_700_000_000, "cleaned", None, vec!["saved .env.local to backups/2026-11-16-poo-wt"]),
                    r("site/node_modules", 1_400_000_000, "skipped", Some("a dev server is running there"), vec![]),
                ];
                a.modal = Modal::Summary(Summary { offset: 0, res });
            }),
        ),
    ]
}

#[test]
fn every_screen_renders_at_every_size() {
    let home = std::env::temp_dir().join(format!("poo-tui-snap-{}", std::process::id()));
    std::env::set_var("POO_HOME", &home);
    let ctx = Ctx::load().unwrap();
    let report = load_report();
    let out = std::env::var_os("POO_TUI_SNAP_DIR").map(std::path::PathBuf::from);
    if let Some(d) = &out {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut th = Theme::detect();
    th.color = true;
    th.truecolor = true;
    let mut app = App::new(ctx, Some(report), true);
    let list = screens(&mut app);
    for (name, setup) in &list {
        for (w, h) in [(36u16, 12u16), (80, 24), (132, 38)] {
            app.modal = Modal::None;
            app.nav.truncate(1);
            app.selected.clear();
            setup(&mut app);
            let buf = render(&mut app, &th, w, h);
            let t = text_of(&buf);
            assert!(t.contains("poo"), "{} at {}x{} has no header", name, w, h);
            if let Some(dir) = &out {
                if w == 132 {
                    dump(&buf, dir, name);
                }
                if w == 80 && *name == "overview" {
                    dump(&buf, dir, "overview-80");
                }
            }
        }
    }
    // NO_COLOR still renders, with no colors at all.
    th.color = false;
    app.modal = Modal::None;
    app.tab = Tab::Browse;
    let buf = render(&mut app, &th, 100, 30);
    for y in 0..30 {
        for x in 0..100 {
            let c = &buf[(x, y)];
            assert!(matches!(c.fg, Color::Reset) && matches!(c.bg, Color::Reset), "color at {},{} with NO_COLOR", x, y);
        }
    }
    let _ = std::fs::remove_dir_all(&home);
}
