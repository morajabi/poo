//! Drawing. Everything here is a pure function of `App` (plus hit regions it records).

use super::app::*;
use super::fmt::{self, cells, fit, fit_mid, pad, rpad};
use super::theme::Theme;
use crate::model::*;
use crate::util::{age, bytes, tilde};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::path::Path;

pub fn draw(f: &mut Frame, app: &mut App, th: &Theme) {
    app.hits = Hits::default();
    let area = f.area();
    let [head, _gap, body, status] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    header(f, head, app, th);
    let body = inset(body, 1, 0);
    if app.report.is_none() {
        loading(f, body, app, th);
    } else {
        match app.tab {
            Tab::Overview => overview(f, body, app, th),
            Tab::Browse => browse(f, body, app, th),
            Tab::Projects => projects(f, body, app, th),
            Tab::Worktrees => worktrees(f, body, app, th),
            Tab::Plans => plans(f, body, app, th),
        }
    }
    status_line(f, status, app, th);
    if !matches!(app.modal, Modal::None) {
        // Push the screen behind a popup back so the popup reads first.
        f.buffer_mut().set_style(Rect { height: area.height.saturating_sub(1), ..area }, Style::default().add_modifier(Modifier::DIM));
    }
    match &app.modal {
        Modal::None => {}
        Modal::Help => help(f, area, app, th),
        Modal::Detail(i) => {
            let i = *i;
            detail(f, area, app, i, th)
        }
        Modal::Review(_) => review(f, area, app, th),
        Modal::Cleaning(_) => cleaning(f, area, app, th),
        Modal::Summary(_) => summary(f, area, app, th),
    }
}

fn inset(r: Rect, x: u16, y: u16) -> Rect {
    Rect { x: r.x + x.min(r.width / 2), y: r.y + y.min(r.height / 2), width: r.width.saturating_sub(2 * x), height: r.height.saturating_sub(2 * y) }
}

fn panel<'a>(title: &'a str, th: &Theme, focus: bool) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focus { th.border_focus() } else { th.border() })
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {} ", title), if focus { th.bold() } else { th.dim() }))
}

fn kv<'a>(k: &str, v: impl Into<Span<'a>>, th: &Theme) -> Line<'a> {
    Line::from(vec![Span::styled(format!("{:<10}", k), th.dim()), v.into()])
}

/// A bar as spans: colored fill, dim track.
fn bar_spans(frac: f64, w: usize, fill: Style, th: &Theme) -> Vec<Span<'static>> {
    let (s, rest) = fmt::bar(frac, w);
    vec![Span::styled(s, fill), Span::styled("·".repeat(rest), th.track())]
}

fn spinner(app: &App) -> &'static str {
    if app.animate {
        fmt::SPINNER[(app.tick as usize) % fmt::SPINNER.len()]
    } else {
        "·"
    }
}

// -------------------------------------------------------------------- chrome

fn header(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let mut spans = vec![Span::styled(" poo ", th.accent().add_modifier(Modifier::BOLD)), Span::raw(" ")];
    let mut x = area.x + 6;
    let compact = area.width < 70;
    for (i, t) in Tab::ALL.iter().enumerate() {
        // Narrow: only the current view keeps its name; the others are their number keys.
        let label = if compact && app.tab != *t { String::new() } else { t.title().to_string() };
        let key = format!("{}", i + 1);
        let w = (cells(&key) + if label.is_empty() { 0 } else { 1 + cells(&label) } + 2) as u16;
        app.hits.tabs.push((Rect { x, y: area.y, width: w, height: 1 }, *t));
        x += w;
        let on = app.tab == *t;
        spans.push(Span::styled(key, if label.is_empty() { th.text() } else { th.dim() }));
        if !label.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(label, if on { th.accent().add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { th.text() }));
        spans.push(Span::raw("  "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    // Right side: free space and freshness.
    let mut right: Vec<Span> = vec![];
    if let Some(s) = &app.scanning {
        right.push(Span::styled(format!("{} {} ", spinner(app), if s.full { "scanning" } else { "updating" }), th.dim()));
    }
    if let Some(r) = &app.report {
        right.push(Span::styled(bytes(r.disk.free), th.bold()));
        right.push(Span::styled(" free", th.dim()));
        if area.width >= 90 {
            right.push(Span::styled(format!("  ·  updated {}", age(r.scanned_at)), th.dim()));
            if app.live {
                right.push(Span::styled(" ●", th.good()));
            }
        }
    }
    let w: usize = right.iter().map(|s| cells(&s.content)).sum::<usize>() + 1;
    if (w as u16) < area.width / 2 {
        let r = Rect { x: area.x + area.width - w as u16, y: area.y, width: w as u16, height: 1 };
        f.render_widget(Paragraph::new(Line::from(right)), r);
    }
}

pub fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    if app.editing {
        return vec![("enter", "keep"), ("esc", "clear"), ("↑↓", "move")];
    }
    match &app.modal {
        Modal::Review(rv) if rv.confirming => vec![("y", "clean"), ("n", "back")],
        Modal::Review(_) => vec![("enter", "clean"), ("f", "in-use mode"), ("t", "trash"), ("p", "preview"), ("esc", "back")],
        Modal::Cleaning(_) => vec![("esc", "cancel after this item")],
        Modal::Summary(_) => vec![("enter", "done")],
        Modal::Help => vec![("any key", "close")],
        Modal::Detail(_) => vec![("space", "select"), ("c", "clean"), ("o", "show in Finder"), ("esc", "close")],
        Modal::None => match app.tab {
            Tab::Overview => vec![("enter", "browse"), ("1-5", "views"), ("r", "refresh"), ("?", "help"), ("q", "quit")],
            Tab::Plans => vec![("↑↓", "move"), ("enter", "review plan"), ("space", "select"), ("c", "clean"), ("?", "help")],
            _ => vec![("↑↓", "move"), ("enter", "open"), ("esc", "back"), ("space", "select"), ("/", "filter"), ("s", "sort"), ("c", "clean"), ("?", "help")],
        },
    }
}

fn status_line(f: &mut Frame, area: Rect, app: &App, th: &Theme) {
    let mut left: Vec<Span> = vec![Span::raw(" ")];
    if app.editing || (!app.query.is_empty() && matches!(app.modal, Modal::None)) {
        left.push(Span::styled("/ ", th.accent()));
        left.push(Span::styled(app.query.clone(), th.bold()));
        if app.editing {
            left.push(Span::styled("▏", th.accent()));
        }
        left.push(Span::raw("   "));
    }
    let mut used: usize = left.iter().map(|s| cells(&s.content)).sum();
    // Right: toast, or the selection.
    let mut right: Vec<Span> = vec![];
    if let Some((t, at)) = &app.toast {
        if at.elapsed().as_secs() < 4 {
            right.push(Span::styled(t.clone(), th.note()));
        }
    }
    if right.is_empty() && !app.selected.is_empty() {
        right.push(Span::styled(format!("{} selected", app.selected.len()), th.text()));
        right.push(Span::styled(" · ", th.dim()));
        right.push(Span::styled(bytes(app.selection_bytes()), th.good().add_modifier(Modifier::BOLD)));
        right.push(Span::styled("  c", th.accent()));
        right.push(Span::styled(" clean  ", th.dim()));
        right.push(Span::styled("u", th.accent()));
        right.push(Span::styled(" clear", th.dim()));
    }
    let rw: usize = right.iter().map(|s| cells(&s.content)).sum::<usize>() + 1;
    let budget = (area.width as usize).saturating_sub(rw + 2);
    let selection_shown = right.len() > 1;
    for (k, label) in hints(app) {
        // The selection on the right already offers c and u.
        if selection_shown && matches!(k, "c" | "u") {
            continue;
        }
        let w = cells(k) + 1 + cells(label) + 3;
        if used + w > budget {
            break;
        }
        left.push(Span::styled(k, th.accent()));
        left.push(Span::raw(" "));
        left.push(Span::styled(label, th.dim()));
        left.push(Span::raw("   "));
        used += w;
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);
    if rw > 1 && rw as u16 <= area.width {
        let r = Rect { x: area.x + area.width - rw as u16, y: area.y, width: rw as u16, height: 1 };
        f.render_widget(Paragraph::new(Line::from(right)), r);
    }
}

fn loading(f: &mut Frame, area: Rect, app: &App, th: &Theme) {
    let mut lines = vec![Line::raw(""), Line::raw("")];
    match &app.scanning {
        Some(s) => {
            lines.push(Line::from(vec![Span::styled(format!("{}  ", spinner(app)), th.accent()), Span::styled("Looking around your disk", th.bold())]));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(format!("{}  ·  {} in {} files", s.phase, bytes(s.bytes), thousands(s.files)), th.dim())));
            lines.push(Line::from(Span::styled(fit_mid(&s.detail, area.width.saturating_sub(4) as usize), th.dim())));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("The first scan takes a minute; after that poo updates from file changes.", th.dim())));
        }
        None => lines.push(Line::from(Span::styled("Loading…", th.dim()))),
    }
    f.render_widget(Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center), area);
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------ overview

/// Disk parts: other files, found by poo and in use, found and not in use, free.
pub fn disk_parts(r: &Report) -> [u64; 4] {
    let not_used: u64 = r.buckets.iter().map(bucket_not_in_use).sum();
    let found = r.total_bytes.min(r.disk.used);
    let in_use = found.saturating_sub(not_used);
    let other = r.disk.used.saturating_sub(found);
    [other, in_use, not_used.min(found), r.disk.free]
}

fn disk_block(r: &Report, w: usize, th: &Theme) -> Vec<Line<'static>> {
    let parts = disk_parts(r);
    let labels = ["other", "dev, in use", "not in use", "free"];
    let bw = w.saturating_sub(2).max(10);
    let cellsv = fmt::alloc(&parts, bw);
    let glyph = ["█", "█", "█", "░"];
    let mut bar: Vec<Span> = vec![];
    for i in 0..4 {
        bar.push(Span::styled(glyph[i].repeat(cellsv[i]), th.disk_part(i)));
    }
    let full: usize = (0..4).map(|i| 2 + labels[i].len() + 1 + bytes(parts[i]).len() + 4).sum();
    let (labels, gap) = if full <= w { (labels, "    ") } else { (["other", "in use", "not in use", "free"], "  ") };
    let mut legend: Vec<Span> = vec![];
    for i in 0..4 {
        legend.push(Span::styled(if i == 3 { "○ " } else { "● " }, th.disk_part(i)));
        legend.push(Span::styled(format!("{} ", labels[i]), th.dim()));
        legend.push(Span::styled(bytes(parts[i]), if i == 2 { th.good().add_modifier(Modifier::BOLD) } else { th.text() }));
        legend.push(Span::raw(gap));
    }
    let used_pct = if r.disk.total > 0 { r.disk.used as f64 * 100.0 / r.disk.total as f64 } else { 0.0 };
    vec![
        Line::from(vec![
            Span::styled(bytes(r.disk.free), th.bold()),
            Span::styled(format!(" free of {}", bytes(r.disk.total)), th.dim()),
            Span::styled(format!("   {:.0}% used", used_pct), th.dim()),
        ]),
        Line::from(bar),
        Line::from(legend),
    ]
}

fn overview(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let r = app.report.as_ref().unwrap();
    let [disk, _g, rest, fresh] = Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Min(4), Constraint::Length(2)]).areas(area);
    f.render_widget(Paragraph::new(disk_block(r, disk.width as usize, th)), disk);

    let mut bs: Vec<&BucketSummary> = r.buckets.iter().filter(|b| !b.hidden && bucket_not_in_use(b) > 0).collect();
    bs.sort_by_key(|b| std::cmp::Reverse(bucket_not_in_use(b)));
    let away = elsewhere(r);
    let area_h = (bs.len().max(1) + 2 + if away.is_empty() { 0 } else { away.len() + 1 }) as u16;
    let plans_h = (r.plans.len() * 2 + 2) as u16;
    if rest.width >= 100 {
        let [left, _, right] = Layout::horizontal([Constraint::Percentage(55), Constraint::Length(2), Constraint::Percentage(45)]).areas(rest);
        let [a, b] = Layout::vertical([Constraint::Length(area_h.min(left.height / 2)), Constraint::Min(3)]).areas(left);
        let [c, d] = Layout::vertical([Constraint::Length(plans_h.min(right.height / 2)), Constraint::Min(3)]).areas(right);
        areas_panel(f, a, &bs, &away, th);
        largest_panel(f, b, r, th);
        plans_panel(f, c, r, th);
        projects_panel(f, d, r, th);
    } else {
        let [a, c, b] = Layout::vertical([Constraint::Length(area_h.min(rest.height / 3)), Constraint::Length((r.plans.len() as u16 + 2).min(rest.height / 2)), Constraint::Min(3)]).areas(rest);
        areas_panel(f, a, &bs, &away, th);
        plans_panel(f, c, r, th);
        largest_panel(f, b, r, th);
    }

    // Freshness.
    let mut l = vec![
        Span::styled(format!("Updated {}", age(r.scanned_at)), th.dim()),
        Span::styled(format!("  ·  {} item{} in {}", thousands(r.items.len() as u64), if r.items.len() == 1 { "" } else { "s" }, crate::util::plural(r.projects.len() as u64, "project")), th.dim()),
    ];
    if app.live {
        l.push(Span::styled("  ·  following file changes", th.dim()));
    }
    let mut lines = vec![Line::from(l)];
    if let Some(w) = r.warnings.first() {
        lines.push(Line::from(Span::styled(fit(&format!("note: {}", w), fresh.width as usize), th.dim())));
    }
    f.render_widget(Paragraph::new(lines), fresh);
}

fn areas_panel(f: &mut Frame, area: Rect, bs: &[&BucketSummary], away: &[(String, u64, Option<String>)], th: &Theme) {
    let block = panel("Not in use, by area", th, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let max = bs.first().map(|b| bucket_not_in_use(b)).unwrap_or(1).max(1);
    let w = inner.width as usize;
    let name_w = (w / 3).clamp(10, 24);
    let bar_w = w.saturating_sub(name_w + 10 + 12 + 3).min(30);
    let mut lines = vec![];
    for b in bs {
        let n = bucket_not_in_use(b);
        let mut l = vec![Span::raw(pad(&b.name, name_w)), Span::raw(" ")];
        if bar_w >= 4 {
            l.extend(bar_spans(n as f64 / max as f64, bar_w, th.hex(&b.color), th));
            l.push(Span::raw(" "));
        }
        l.push(Span::styled(rpad(&bytes(n), 9), th.good()));
        if w > name_w + bar_w + 24 {
            l.push(Span::styled(format!("  of {}", bytes(b.bytes)), th.dim()));
        }
        lines.push(Line::from(l));
    }
    if bs.is_empty() {
        lines.push(Line::from(Span::styled("Nothing idle right now.", th.dim())));
    }
    // Report-only areas: context, never counted as cleanable.
    if !away.is_empty() && (lines.len() + 1) < inner.height as usize {
        lines.push(Line::raw(""));
    }
    for (name, b, _) in away {
        lines.push(Line::from(vec![
            Span::styled(pad(name, name_w), th.dim()),
            Span::styled(fit(&format!(" {} · manage in their apps", bytes(*b)), w.saturating_sub(name_w)), th.dim()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn largest_panel(f: &mut Frame, area: Rect, r: &Report, th: &Theme) {
    let block = panel("Largest, not in use", th, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut items: Vec<&Item> = r.items.iter().filter(|i| cleanable(i)).collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.total_bytes));
    let w = inner.width as usize;
    let mut lines = vec![];
    for i in items.iter().take(inner.height as usize) {
        let used = i.last_used.map(age).unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(rpad(&bytes(i.total_bytes), 9), th.bold()),
            Span::raw("  "),
            tier_span(i.tier, 8, th),
            Span::raw(pad(&i.name, w.saturating_sub(9 + 2 + 8 + 10))),
            Span::styled(rpad(&used, 10), th.dim()),
        ]));
    }
    if items.is_empty() {
        lines.push(Line::from(Span::styled("Nothing idle right now. Nice and tidy.", th.dim())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn plans_panel(f: &mut Frame, area: Rect, r: &Report, th: &Theme) {
    let block = panel("Plans", th, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let mut lines = vec![];
    let room = inner.height as usize >= r.plans.len() * 2;
    for p in &r.plans {
        lines.push(Line::from(vec![
            Span::styled(pad(&p.name, 14), th.bold()),
            Span::styled(rpad(&bytes(p.bytes), 9), if p.bytes > 0 { th.good() } else { th.dim() }),
            Span::styled(format!("  {:>4} items", p.count), th.dim()),
        ]));
        if room && w > 20 {
            lines.push(Line::from(Span::styled(fit(&p.description, w), th.dim())));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn projects_panel(f: &mut Frame, area: Rect, r: &Report, th: &Theme) {
    let block = panel("Projects by build size", th, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut ps: Vec<&Project> = r.projects.iter().collect();
    ps.sort_by_key(|p| std::cmp::Reverse(p.artifact_bytes));
    let w = inner.width as usize;
    let mut lines = vec![];
    for p in ps.iter().take(inner.height as usize) {
        lines.push(Line::from(vec![
            Span::styled(rpad(&bytes(p.artifact_bytes), 9), th.bold()),
            Span::raw("  "),
            Span::styled(pad(&p.status, 8), status_style(&p.status, th)),
            Span::raw(pad(&p.name, w.saturating_sub(9 + 2 + 8 + 10))),
            Span::styled(rpad(&p.last_activity.map(age).unwrap_or_default(), 10), th.dim()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// -------------------------------------------------------------------- lists

/// Draws rows with a highlighted cursor row and records the hit area.
fn list(f: &mut Frame, area: Rect, header: Option<Line<'static>>, rows: Vec<Line<'static>>, app: &mut App, th: &Theme) {
    let mut area = area;
    if let Some(h) = header {
        f.render_widget(Paragraph::new(h), Rect { height: 1, ..area });
        area.y += 1;
        area.height = area.height.saturating_sub(1);
    }
    let (c, o) = app.cursor();
    let o = fmt::scroll(o, c, area.height as usize, rows.len());
    app.set_cursor(c, o);
    app.hits.list = Some((area, o));
    for (i, line) in rows.into_iter().enumerate().skip(o).take(area.height as usize) {
        let y = area.y + (i - o) as u16;
        let r = Rect { y, height: 1, ..area };
        if i == c {
            f.render_widget(Block::default().style(th.selected_row()), r);
        }
        f.render_widget(Paragraph::new(line), r);
    }
    if rows_empty_hint(app) && area.height > 0 {
        let msg = if app.query.is_empty() { "Nothing here." } else { "No matches. esc clears the filter." };
        f.render_widget(Paragraph::new(Span::styled(msg, th.dim())), Rect { height: 1, ..area });
    }
}

fn rows_empty_hint(app: &App) -> bool {
    app.list_len() == 0
}

fn mark(sel: bool, partial: bool, th: &Theme) -> Span<'static> {
    if sel {
        Span::styled("● ", th.good())
    } else if partial {
        Span::styled("◐ ", th.good())
    } else {
        Span::styled("  ", th.dim())
    }
}

fn tier_span(t: Tier, w: usize, th: &Theme) -> Span<'static> {
    Span::styled(pad(t.label(), w), th.tier(t))
}

fn split_detail(area: Rect) -> (Rect, Option<Rect>) {
    if area.width >= 96 {
        let [a, _, b] = Layout::horizontal([Constraint::Percentage(60), Constraint::Length(1), Constraint::Percentage(40)]).areas(area);
        (a, Some(b))
    } else {
        (area, None)
    }
}

fn crumbs(app: &App) -> String {
    let mut parts = vec!["All".to_string()];
    for n in app.nav.iter().skip(1) {
        match &n.level {
            Level::Groups { name, .. } => parts.push(name.clone()),
            Level::Items { title, .. } => parts.push(title.clone()),
            Level::Buckets => {}
        }
    }
    parts.join("  ›  ")
}

fn browse(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(2)]).areas(area);
    let rows = app.browse_rows();
    let total: u64 = rows.iter().map(|r| r.bytes).sum();
    let mut l = vec![Span::styled(crumbs(app), th.bold()), Span::styled(format!("   {} · {}", rows.len(), bytes(total)), th.dim())];
    l.push(Span::styled(format!("   sorted by {}", app.sort.label()), th.dim()));
    f.render_widget(Paragraph::new(Line::from(l)), top);
    let (lst, detail) = split_detail(body);
    let w = lst.width as usize;
    let show_bar = w >= 60;
    let show_age = w >= 50;
    let bar_w = 10;
    let fixed = 2 + 1 + 9 + 2 + 11 + if show_age { 9 } else { 0 } + if show_bar { bar_w + 1 } else { 0 };
    let name_w = w.saturating_sub(fixed).max(8);
    let max = rows.iter().map(|r| r.bytes).max().unwrap_or(1).max(1);
    let is_items = rows.first().map_or(false, |r| matches!(r.kind, BKind::Item(_)));
    let header = {
        let mut h = format!("  {}", pad("NAME", name_w));
        if show_bar {
            h.push_str(&" ".repeat(bar_w + 1));
        }
        h.push_str(&format!(" {:>9}  {}", "SIZE", pad(if is_items { "STATUS" } else { "NOT IN USE" }, 11)));
        if show_age {
            h.push_str(&format!("{:>9}", if is_items { "USED" } else { "ITEMS" }));
        }
        Line::from(Span::styled(h, th.dim()))
    };
    let mut lines = vec![];
    for r in &rows {
        let (sel, partial) = match &r.kind {
            BKind::Item(i) => (app.item(*i).map_or(false, |x| app.selected.contains(&x.id)), false),
            BKind::Bucket(b) => partial_sel(app, |i| &i.bucket == b),
            BKind::Group { bucket, group } => partial_sel(app, |i| &i.bucket == bucket && &i.group == group),
        };
        let mut l = vec![mark(sel, partial, th), Span::raw(pad(&r.name, name_w))];
        if show_bar {
            l.push(Span::raw(" "));
            let fill = match (r.color.as_deref(), r.tier) {
                (Some(c), _) => th.hex(c),
                (None, Some(_)) if r.report_only => th.dim(),
                (None, Some(t)) if cleanable_tier(t) => th.good(),
                (None, Some(_)) => th.dim(),
                (None, None) => th.good(),
            };
            l.extend(bar_spans(r.bytes as f64 / max as f64, bar_w, fill, th));
        }
        l.push(Span::raw(" "));
        l.push(Span::styled(rpad(&bytes(r.bytes), 9), th.bold()));
        l.push(Span::raw("  "));
        match r.tier {
            _ if r.report_only => l.push(Span::styled(pad("in its app", 11), th.dim())),
            Some(t) => l.push(tier_span(t, 11, th)),
            None => l.push(Span::styled(pad(&if r.free_bytes > 0 { bytes(r.free_bytes) } else { "–".into() }, 11), if r.free_bytes > 0 { th.good() } else { th.dim() })),
        }
        if show_age {
            let a = if is_items { r.last_used.map(age).unwrap_or_else(|| "–".into()) } else { r.count.to_string() };
            l.push(Span::styled(rpad(&a, 9), th.dim()));
        }
        lines.push(Line::from(l));
    }
    list(f, lst, Some(header), lines, app, th);
    app.hits.detail_pane = detail.is_some();
    if let Some(d) = detail {
        let (c, _) = app.cursor();
        let row = rows.get(c).cloned();
        browse_detail(f, d, app, row, th);
    }
}

fn partial_sel(app: &App, pred: impl Fn(&Item) -> bool) -> (bool, bool) {
    let Some(r) = &app.report else { return (false, false) };
    if app.selected.is_empty() {
        return (false, false);
    }
    let mut any = false;
    let mut all = true;
    let mut n = 0;
    for i in r.items.iter().filter(|i| pred(i) && cleanable(i)) {
        n += 1;
        if app.selected.contains(&i.id) {
            any = true;
        } else {
            all = false;
        }
    }
    (n > 0 && all, any && !all)
}

fn browse_detail(f: &mut Frame, area: Rect, app: &App, row: Option<BRow>, th: &Theme) {
    let block = panel("Details", th, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(row) = row else { return };
    let w = inner.width as usize;
    let r = app.report.as_ref().unwrap();
    let mut lines: Vec<Line> = vec![];
    match &row.kind {
        BKind::Item(idx) => {
            let Some(i) = app.item(*idx) else { return };
            lines = item_lines(i, app, w, th);
        }
        BKind::Bucket(id) => {
            let Some(b) = r.buckets.iter().find(|b| &b.id == id) else { return };
            lines.push(Line::from(Span::styled(b.name.clone(), th.bold())));
            if !b.description.is_empty() {
                lines.push(Line::from(Span::styled(b.description.clone(), th.dim())));
            }
            lines.push(Line::raw(""));
            lines.push(kv("size", Span::raw(format!("{} in {}", bytes(b.bytes), crate::util::plural(b.count as u64, "item"))), th));
            lines.push(Line::raw(""));
            lines.extend(tier_breakdown(&b.tiers, w, th));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("Largest groups", th.dim())));
            let mut gs = b.groups.clone();
            gs.sort_by_key(|g| std::cmp::Reverse(g.bytes));
            for g in gs.iter().take(8) {
                lines.push(Line::from(vec![Span::styled(rpad(&bytes(g.bytes), 9), th.text()), Span::raw("  "), Span::raw(fit(&g.name, w.saturating_sub(11)))]));
            }
        }
        BKind::Group { bucket, group } => {
            let items: Vec<&Item> = r.items.iter().filter(|i| &i.bucket == bucket && &i.group == group).collect();
            lines.push(Line::from(Span::styled(group.clone(), th.bold())));
            lines.push(Line::raw(""));
            lines.push(kv("size", Span::raw(format!("{} in {}", bytes(row.bytes), crate::util::plural(items.len() as u64, "item"))), th));
            if let Some(rule) = items.first().and_then(|i| app.ctx.cat.rule(&i.rule)) {
                if !rule.description.is_empty() {
                    lines.push(Line::raw(""));
                    lines.push(Line::from(Span::styled(rule.description.clone(), th.dim())));
                }
            }
            let mut tiers = std::collections::BTreeMap::new();
            for i in &items {
                *tiers.entry(i.tier.as_str().to_string()).or_insert(0u64) += i.bytes;
            }
            lines.push(Line::raw(""));
            lines.extend(tier_breakdown(&tiers, w, th));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn tier_breakdown(tiers: &std::collections::BTreeMap<String, u64>, w: usize, th: &Theme) -> Vec<Line<'static>> {
    let total: u64 = tiers.values().sum::<u64>().max(1);
    let bw = w.saturating_sub(24).min(20);
    let mut out = vec![];
    for t in Tier::all() {
        let Some(&b) = tiers.get(t.as_str()) else { continue };
        if b == 0 {
            continue;
        }
        let mut l = vec![tier_span(t, 12, th), Span::styled(rpad(&bytes(b), 9), th.text()), Span::raw("  ")];
        if bw >= 4 {
            l.extend(bar_spans(b as f64 / total as f64, bw, th.tier(t), th));
        }
        out.push(Line::from(l));
    }
    out
}

pub fn item_lines(i: &Item, app: &App, w: usize, th: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(i.name.clone(), th.bold())),
        Line::from(Span::styled(fit_mid(&tilde(Path::new(&i.path)), w), th.dim())),
        Line::raw(""),
    ];
    let size = if i.total_bytes > i.bytes { format!("{}  ({} with nested)", bytes(i.bytes), bytes(i.total_bytes)) } else { bytes(i.bytes) };
    lines.push(kv("size", Span::raw(format!("{}  ·  {} files", size, thousands(i.files))), th));
    if i.report_only {
        lines.push(Line::from(vec![Span::styled(format!("{:<10}", "status"), th.dim()), Span::styled("report only  shown for context; manage it in its app".to_string(), th.dim())]));
    } else {
        lines.push(Line::from(vec![Span::styled(format!("{:<10}", "status"), th.dim()), Span::styled(i.tier.label().to_string(), th.tier(i.tier)), Span::styled(format!("  {}", i.tier.describe()), th.dim())]));
    }
    if let Some(t) = i.last_used {
        lines.push(kv("used", Span::raw(age(t)), th));
    }
    let cost = match i.cost {
        Cost::None => "nothing to rebuild".to_string(),
        Cost::Rebuild => "a rebuild".to_string(),
        Cost::Redownload => "a redownload".to_string(),
        Cost::Data => "your data, not recoverable".to_string(),
    };
    lines.push(kv("costs", Span::raw(cost), th));
    if let Some(rb) = &i.rebuild {
        lines.push(kv("rebuild", Span::raw(rb.clone()), th));
    }
    if !i.reasons.is_empty() {
        lines.push(kv("why", Span::raw(i.reasons.join("; ")), th));
    }
    if let Some(p) = &i.project {
        lines.push(kv("project", Span::raw(fit_mid(&tilde(Path::new(p)), w.saturating_sub(11))), th));
    }
    if let Some(a) = &i.agent {
        lines.push(kv("agent", Span::raw(a.clone()), th));
    }
    let risks: Vec<&Risk> = i.risks.iter().filter(|r| r.level > RiskLevel::Standard).collect();
    if !risks.is_empty() || !i.safeguards.is_empty() {
        lines.push(Line::raw(""));
    }
    for r in risks {
        lines.push(Line::from(vec![Span::styled("⚑ ", th.note()), Span::raw(r.message.clone()), Span::styled(format!("  ({} mode)", r.level.as_str()), th.dim())]));
    }
    for g in &i.safeguards {
        lines.push(Line::from(vec![Span::styled("↺ ", th.good()), Span::styled(g.clone(), th.dim())]));
    }
    let action = if i.report_only { "none; manage it in its app".into() } else { match i.action.kind {
        ActionKind::Command => format!("runs `{}`", i.action.command.clone().unwrap_or_default().join(" ")),
        ActionKind::WorktreeRemove => "git worktree remove".into(),
        ActionKind::Delete => "delete".into(),
        ActionKind::Trash => "move to Trash".into(),
        ActionKind::None => "manage it yourself".into(),
    } };
    lines.push(Line::raw(""));
    lines.push(kv("action", Span::raw(action), th));
    if let Some(rule) = app.ctx.cat.rule(&i.rule) {
        // Report-only items already say this in their safeguard line.
        if let Some(rec) = rule.recommend.as_ref().filter(|rec| !i.safeguards.iter().any(|g| g.contains(rec.as_str()))) {
            lines.push(kv("tip", Span::styled(rec.clone(), th.dim()), th));
        }
    }
    if !i.children.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("Inside", th.dim())));
        let max = i.children.iter().map(|c| c.bytes).max().unwrap_or(1).max(1);
        let bw = 8;
        for c in i.children.iter().take(10) {
            let mut l = vec![Span::raw(rpad(&bytes(c.bytes), 9)), Span::raw(" ")];
            l.extend(bar_spans(c.bytes as f64 / max as f64, bw, th.dim(), th));
            l.push(Span::raw(" "));
            l.push(Span::raw(fit(&c.name, w.saturating_sub(20))));
            lines.push(Line::from(l));
        }
    }
    lines
}

// ------------------------------------------------------------ projects/trees

fn status_style(s: &str, th: &Theme) -> Style {
    match s {
        "active" => th.accent(),
        "stale" => th.good(),
        _ => th.text(),
    }
}

fn projects(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let rows = app.project_rows();
    let total: u64 = rows.iter().map(|p| p.artifact_bytes).sum();
    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(2)]).areas(area);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("Projects", th.bold()), Span::styled(format!("   {} · {} of build output   sorted by {}", rows.len(), bytes(total), app.sort.label()), th.dim())])),
        top,
    );
    let (lst, detail) = split_detail(body);
    let w = lst.width as usize;
    let show_stacks = w >= 80;
    let fixed = 2 + 9 + 2 + 8 + 2 + 9 + if show_stacks { 18 } else { 0 };
    let name_w = w.saturating_sub(fixed).max(10);
    let header = Line::from(Span::styled(
        format!("  {}{:>9}  {}{:>11}{}", pad("PROJECT", name_w), "BUILD", pad("STATUS", 8), "ACTIVE", if show_stacks { "  STACKS" } else { "" }),
        th.dim(),
    ));
    let mut lines = vec![];
    let cur = rows.get(app.projects_cursor.0).map(|p| (*p).clone());
    for p in &rows {
        let (sel, partial) = {
            let f = crate::plan::Filter { project: Some(p.path.clone()), ..Default::default() };
            partial_sel(app, |i| f.matches(i))
        };
        let name = if p.kind == "worktree" { format!("{} ↳", p.name) } else { p.name.clone() };
        let mut l = vec![
            mark(sel, partial, th),
            Span::raw(pad(&name, name_w)),
            Span::styled(rpad(&bytes(p.artifact_bytes), 9), th.bold()),
            Span::raw("  "),
            Span::styled(pad(&p.status, 8), status_style(&p.status, th)),
            Span::styled(rpad(&p.last_activity.map(age).unwrap_or_else(|| "–".into()), 11), th.dim()),
        ];
        if show_stacks {
            l.push(Span::styled(format!("  {}", fit(&p.stacks.join(" "), 16)), th.dim()));
        }
        lines.push(Line::from(l));
    }
    list(f, lst, Some(header), lines, app, th);
    if let (Some(d), Some(p)) = (detail, cur) {
        let block = panel("Details", th, false);
        let inner = block.inner(d);
        f.render_widget(block, d);
        let w = inner.width as usize;
        let mut lines = vec![
            Line::from(Span::styled(p.name.clone(), th.bold())),
            Line::from(Span::styled(fit_mid(&tilde(Path::new(&p.path)), w), th.dim())),
            Line::raw(""),
            kv("kind", Span::raw(p.kind.clone()), th),
            kv("status", Span::styled(p.status.clone(), status_style(&p.status, th)), th),
            kv("policy", Span::raw(format!("{:?}", p.policy).to_lowercase()), th),
            kv("build", Span::raw(format!("{} in {}", bytes(p.artifact_bytes), crate::util::plural(p.item_count as u64, "item"))), th),
        ];
        if let Some(b) = &p.branch {
            lines.push(kv("branch", Span::raw(b.clone()), th));
        }
        if let Some(a) = &p.agent {
            lines.push(kv("agent", Span::raw(a.clone()), th));
        }
        if p.dirty == Some(true) {
            lines.push(kv("git", Span::styled("uncommitted changes", th.note()), th));
        } else if p.merged == Some(true) {
            lines.push(kv("git", Span::styled("merged", th.good()), th));
        }
        if !p.active.is_empty() {
            lines.push(kv("open in", Span::styled(p.active.join(", "), th.accent()), th));
        }
        if let Some(t) = p.last_activity {
            lines.push(kv("active", Span::raw(age(t)), th));
        }
        if !p.stacks.is_empty() {
            lines.push(kv("stacks", Span::raw(p.stacks.join(", ")), th));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled("enter", th.accent()), Span::styled(" browse its items", th.dim())]));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

fn worktrees(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let rows = app.worktree_rows();
    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(2)]).areas(area);
    let repos = rows.iter().filter(|r| r.repo).count();
    let trees = rows.len() - repos;
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled("Worktrees", th.bold()), Span::styled(format!("   {} checkouts across {} repos", trees, repos), th.dim())])), top);
    let w = body.width as usize;
    let show_agent = w >= 90;
    let fixed = 2 + 2 + 9 + 2 + 8 + 2 + 11 + if show_agent { 12 } else { 0 } + 10;
    let name_w = w.saturating_sub(fixed).max(12);
    let header = Line::from(Span::styled(
        format!("    {}{:>9}  {}  {}{}{:>10}", pad("CHECKOUT", name_w), "SIZE", pad("GIT", 8), pad("STATUS", 11), if show_agent { pad("AGENT", 12) } else { String::new() }, "ACTIVE"),
        th.dim(),
    ));
    let mut lines = vec![];
    for r in &rows {
        if r.repo {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(pad(&tilde(Path::new(&r.path)), name_w + 2), th.bold()),
                Span::styled(rpad(&bytes(r.bytes), 9), th.bold()),
                Span::styled(format!("  {} checkouts", r.checkouts), th.dim()),
            ]));
            continue;
        }
        let sel = r.item_id.as_ref().map_or(false, |id| app.selected.contains(id));
        let name = format!("{}  {}", std::path::Path::new(&r.path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), r.branch);
        let git_style = match r.state.as_str() {
            "changes" => th.note(),
            "merged" => th.good(),
            _ => th.dim(),
        };
        let mut l = vec![
            mark(sel, false, th),
            Span::styled("  ", th.dim()),
            Span::raw(pad(&name, name_w)),
            Span::styled(rpad(&bytes(r.bytes), 9), th.text()),
            Span::raw("  "),
            Span::styled(pad(&r.state, 8), git_style),
            Span::raw("  "),
            r.tier.map(|t| tier_span(t, 11, th)).unwrap_or_else(|| Span::styled(pad("–", 11), th.dim())),
        ];
        if show_agent {
            l.push(Span::styled(pad(&r.agent, 12), th.dim()));
        }
        l.push(Span::styled(rpad(&r.last_activity.map(age).unwrap_or_else(|| "–".into()), 10), th.dim()));
        lines.push(Line::from(l));
    }
    list(f, body, Some(header), lines, app, th);
}

fn plans(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(2)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled("Plans", th.bold()), Span::styled("   ready-made cleanups; each one is reviewed before anything is removed", th.dim())])), top);
    let wide = body.width >= 80;
    let (lst, detail) = if wide {
        let [a, _, b] = Layout::horizontal([Constraint::Length(40), Constraint::Length(1), Constraint::Min(30)]).areas(body);
        (a, Some(b))
    } else {
        (body, None)
    };
    let mut lines = vec![];
    for p in &app.plans {
        let sel = !p.items.is_empty() && p.items.iter().all(|i| app.selected.contains(&i.id));
        lines.push(Line::from(vec![
            mark(sel, false, th),
            Span::styled(pad(&p.name, 14), th.bold()),
            Span::styled(rpad(&bytes(p.bytes), 9), if p.bytes > 0 { th.good() } else { th.dim() }),
            Span::styled(format!("  {:>4} items", p.count), th.dim()),
        ]));
    }
    let cur = app.plans_cursor.0;
    list(f, lst, None, lines, app, th);
    let (Some(d), Some(p)) = (detail, app.plans.get(cur)) else { return };
    let block = panel(&p.name, th, false);
    let inner = block.inner(d);
    f.render_widget(block, d);
    let w = inner.width as usize;
    let mut lines = vec![Line::from(Span::styled(p.description.clone(), th.dim())), Line::raw("")];
    lines.push(Line::from(vec![Span::styled(bytes(p.bytes), th.good().add_modifier(Modifier::BOLD)), Span::styled(format!(" in {}", crate::util::plural(p.count as u64, "item")), th.dim())]));
    if let Some(t) = p.target_bytes {
        lines.push(Line::from(Span::styled(format!("aims to free {}", bytes(t)), th.dim())));
    }
    lines.push(Line::raw(""));
    let mut items: Vec<&Item> = p.items.iter().collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.total_bytes));
    let room = (inner.height as usize).saturating_sub(lines.len() + 2);
    for i in items.iter().take(room) {
        lines.push(Line::from(vec![
            Span::raw(rpad(&bytes(i.total_bytes), 9)),
            Span::raw("  "),
            tier_span(i.tier, 11, th),
            Span::raw(fit(&i.name, w.saturating_sub(23))),
        ]));
    }
    if items.len() > room {
        lines.push(Line::from(Span::styled(format!("            and {} more", items.len() - room), th.dim())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// -------------------------------------------------------------------- modals

fn popup(area: Rect, w_pct: u16, h_pct: u16, min_w: u16, min_h: u16) -> Rect {
    let w = (area.width * w_pct / 100).max(min_w).min(area.width);
    // Never cover the header row or the status line.
    let room = area.height.saturating_sub(2);
    let h = (area.height * h_pct / 100).max(min_h).min(room);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + 1 + (room - h) / 2, width: w, height: h }
}

fn help(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let r = popup(area, 60, 0, 66, 23);
    app.hits.modal = Some(r);
    f.render_widget(Clear, r);
    let block = panel("Keys", th, true);
    let inner = inset(block.inner(r), 1, 0);
    f.render_widget(block, r);
    let keys: &[(&str, &str)] = &[
        ("1–5  tab", "switch view"),
        ("↑↓  j k", "move"),
        ("enter  l  →", "open"),
        ("esc  h  ←", "back, clear filter"),
        ("g  G", "top, bottom"),
        ("^d  ^u", "page down, up"),
        ("/", "filter"),
        ("s", "sort by size, age, name"),
        ("space", "select (rows, groups, plans)"),
        ("a", "select everything not in use here"),
        ("u", "clear selection"),
        ("c", "review and clean"),
        ("o", "show in Finder"),
        ("r  R", "refresh, full rescan"),
        ("mouse", "click, double-click, scroll, right-click back"),
        ("q", "quit"),
    ];
    let mut lines = vec![];
    for (k, d) in keys {
        lines.push(Line::from(vec![Span::styled(pad(k, 14), th.accent()), Span::raw(d.to_string())]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("Every clean shows a review first. Safe mode skips items in use (f there allows them); protected items are CLI-only.", th.dim())));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// One item's details as a popup, for terminals too narrow for the side pane.
fn detail(f: &mut Frame, area: Rect, app: &mut App, idx: usize, th: &Theme) {
    let r = popup(area, 80, 80, 40, 12);
    app.hits.modal = Some(r);
    f.render_widget(Clear, r);
    let selected = app.item(idx).map_or(false, |i| app.selected.contains(&i.id));
    let block = panel(if selected { "Details · selected" } else { "Details" }, th, true);
    let inner = inset(block.inner(r), 1, 0);
    f.render_widget(block, r);
    let Some(i) = app.item(idx) else { return };
    let lines = item_lines(i, app, inner.width as usize, th);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn review(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let Modal::Review(rv) = &app.modal else { return };
    let r = popup(area, 86, 86, 60, 16);
    let title = rv.title.clone();
    f.render_widget(Clear, r);
    let block = panel(&title, th, true);
    let inner = inset(block.inner(r), 1, 0);
    f.render_widget(block, r);
    let [sum, modes, _g, body, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(inner);
    let (n, b) = rv.cleanable();
    let stays = rv.items.len() - n;
    let mut s = vec![
        Span::styled(format!("{} to clean", n), th.bold()),
        Span::styled("  ·  ", th.dim()),
        Span::styled(bytes(b), th.good().add_modifier(Modifier::BOLD)),
    ];
    if stays > 0 {
        s.push(Span::styled(format!("  ·  {} stay", stays), th.dim()));
    }
    f.render_widget(Paragraph::new(Line::from(s)), sum);
    let toggle = |on: bool, label: &str, key: &str| -> Vec<Span<'static>> {
        vec![Span::styled(key.to_string(), th.accent()), Span::raw(" "), Span::styled(format!("{} {}", if on { "●" } else { "○" }, label), if on { th.bold() } else { th.dim() }), Span::raw("    ")]
    };
    let mut m = vec![Span::styled("mode ", th.dim())];
    m.push(Span::styled(if rv.mode == RiskLevel::Standard { "safe" } else { "in use allowed" }, if rv.mode == RiskLevel::Standard { th.good() } else { th.note() }));
    m.push(Span::raw("    "));
    m.extend(toggle(rv.mode == RiskLevel::Force, "include in use", "f"));
    m.extend(toggle(rv.trash, "to Trash", "t"));
    m.extend(toggle(rv.dry, "preview only", "p"));
    f.render_widget(Paragraph::new(Line::from(m)), modes);

    let w = body.width as usize;
    let name_w = w.saturating_sub(2 + 9 + 2 + 12).max(10);
    let mut lines: Vec<Line> = vec![];
    for (i, a) in rv.items.iter().zip(&rv.admits) {
        match a {
            Ok(warn) => {
                lines.push(Line::from(vec![
                    Span::styled("✓ ", th.good()),
                    Span::styled(rpad(&bytes(i.total_bytes), 9), th.bold()),
                    Span::raw("  "),
                    tier_span(i.tier, 12, th),
                    Span::raw(fit(&i.name, name_w)),
                ]));
                for x in warn {
                    lines.push(Line::from(vec![Span::raw("               "), Span::styled("⚑ ", th.note()), Span::styled(fit(x, w.saturating_sub(17)), th.text())]));
                }
                for g in i.safeguards.iter().filter(|g| is_backup_note(g)) {
                    lines.push(Line::from(vec![Span::raw("               "), Span::styled("↺ ", th.good()), Span::styled(fit(g, w.saturating_sub(17)), th.dim())]));
                }
            }
            Err(why) => {
                lines.push(Line::from(vec![
                    Span::styled("– ", th.dim()),
                    Span::styled(rpad(&bytes(i.total_bytes), 9), th.dim()),
                    Span::raw("  "),
                    Span::styled(pad("stays", 12), th.dim()),
                    Span::styled(fit(&format!("{}  ({})", i.name, why), name_w), th.dim()),
                ]));
            }
        }
    }
    let off = rv.offset.min(lines.len().saturating_sub(1));
    f.render_widget(Paragraph::new(lines.into_iter().skip(off).collect::<Vec<_>>()), body);

    let mut fl = vec![];
    if rv.confirming {
        let verb = if rv.dry { "Preview cleaning" } else if rv.trash { "Move to Trash" } else { "Clean" };
        fl.push(Line::from(vec![
            Span::styled(format!("{} {}, {}?  ", verb, crate::util::plural(n as u64, "item"), bytes(b)), th.bold()),
            Span::styled("y", th.accent()),
            Span::styled(" yes   ", th.dim()),
            Span::styled("n", th.accent()),
            Span::styled(" no", th.dim()),
        ]));
    } else if let Some(e) = &rv.error {
        fl.push(Line::from(Span::styled(e.clone(), th.note())));
    } else {
        let b = crate::backup::dir();
        fl.push(Line::from(Span::styled(fit(&format!("↺ backups, stashes and rescue branches are kept in {} (poo backups)", tilde(&b)), w), th.dim())));
    }
    f.render_widget(Paragraph::new(fl), foot);
}

/// Safeguards that save something (backups, stashes, branches), not plain action notes.
pub fn is_backup_note(s: &str) -> bool {
    const ACTIONS: [&str; 6] = ["Deleted permanently", "Empties the folder", "Moves to the Trash", "Runs `", "Removed with git worktree remove", "Poo shows it"];
    !ACTIONS.iter().any(|a| s.starts_with(a))
}

fn cleaning(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let Modal::Cleaning(c) = &app.modal else { return };
    let r = popup(area, 70, 0, 50, 13);
    f.render_widget(Clear, r);
    let block = panel(if c.dry { "Preview" } else { "Cleaning" }, th, true);
    let inner = inset(block.inner(r), 2, 1);
    f.render_widget(block, r);
    let w = inner.width as usize;
    let (done, freed, cur) = c.progress.as_ref().map(|p| (p.done, p.freed_bytes, p.current.clone())).unwrap_or((0, 0, String::new()));
    let frac = if c.total_bytes > 0 { freed as f64 / c.total_bytes as f64 } else { done as f64 / c.total.max(1) as f64 };
    let mut lines = vec![];
    if app.animate {
        let (swept, ahead) = fmt::broom(app.tick / 2, w);
        lines.push(Line::from(vec![Span::raw(swept), Span::styled(ahead, th.dim())]));
    } else {
        lines.push(Line::from(Span::raw("🧹")));
    }
    lines.push(Line::raw(""));
    let mut b = bar_spans(frac, w.saturating_sub(7), th.good(), th);
    b.push(Span::styled(format!(" {:>4.0}%", frac * 100.0), th.bold()));
    lines.push(Line::from(b));
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", bytes(freed)), th.good().add_modifier(Modifier::BOLD)),
        Span::styled(format!("of {}  ·  {} of {}  ·  {}s", bytes(c.total_bytes), done, crate::util::plural(c.total as u64, "item"), c.started.elapsed().as_secs()), th.dim()),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(fit(&cur, w), th.text())));
    lines.push(Line::raw(""));
    if c.cancel_requested {
        lines.push(Line::from(Span::styled("Stopping after this item…", th.note())));
    } else {
        lines.push(Line::from(vec![Span::styled("esc", th.accent()), Span::styled(" cancel after this item", th.dim())]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn summary(f: &mut Frame, area: Rect, app: &mut App, th: &Theme) {
    let Modal::Summary(s) = &app.modal else { return };
    let rows = s.res.results.len() + s.res.results.iter().map(|x| x.notes.len()).sum::<usize>() + 9;
    let r = popup(area, 74, 0, 54, rows.min(u16::MAX as usize) as u16);
    app.hits.modal = Some(r);
    let Modal::Summary(s2) = &app.modal else { return };
    let res = &s2.res;
    f.render_widget(Clear, r);
    let block = panel(if res.dry_run { "Preview" } else { "Done" }, th, true);
    let inner = inset(block.inner(r), 2, 1);
    f.render_widget(block, r);
    let w = inner.width as usize;
    let mut lines = vec![];
    if res.dry_run {
        lines.push(Line::from(vec![Span::styled("Would free ", th.text()), Span::styled(bytes(res.freed_bytes), th.good().add_modifier(Modifier::BOLD)), Span::styled("  ·  nothing was removed", th.dim())]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Freed ", th.bold()),
            Span::styled(bytes(res.freed_bytes), th.good().add_modifier(Modifier::BOLD)),
            Span::styled("  ·  free ", th.dim()),
            Span::raw(bytes(res.disk_free_before)),
            Span::styled(" → ", th.dim()),
            Span::styled(bytes(res.disk_free_after), th.bold()),
        ]));
    }
    let mut counts = vec![Span::styled(format!("{} cleaned", res.cleaned), th.text())];
    if res.skipped > 0 {
        counts.push(Span::styled(format!("  ·  {} stayed", res.skipped), th.dim()));
    }
    if res.failed > 0 {
        counts.push(Span::styled(format!("  ·  {} failed", res.failed), th.note()));
    }
    if res.cancelled {
        counts.push(Span::styled("  ·  cancelled", th.note()));
    }
    lines.push(Line::from(counts));
    lines.push(Line::raw(""));
    let mut body: Vec<Line> = vec![];
    for x in &res.results {
        let (m, st) = match x.status.as_str() {
            "cleaned" => ("✓ ", th.good()),
            "would-clean" => ("· ", th.good()),
            "failed" => ("✗ ", th.note()),
            _ => ("– ", th.dim()),
        };
        let mut l = vec![Span::styled(m, st), Span::styled(rpad(&bytes(x.bytes), 9), th.text()), Span::raw("  "), Span::raw(fit(&x.name, w.saturating_sub(14)))];
        if let Some(rn) = &x.reason {
            l.push(Span::styled(format!("  {}", rn), th.dim()));
        }
        body.push(Line::from(l));
        for n in &x.notes {
            body.push(Line::from(vec![Span::raw("             "), Span::styled("↺ ", th.good()), Span::styled(n.clone(), th.dim())]));
        }
    }
    if res.background_delete {
        body.push(Line::raw(""));
        body.push(Line::from(Span::styled("Erasing in the background; free space keeps growing for a minute.", th.dim())));
    }
    if let Some(n) = &res.snapshot_note {
        body.push(Line::from(Span::styled(n.clone(), th.dim())));
    }
    let off = s2.offset.min(body.len().saturating_sub(1));
    lines.extend(body.into_iter().skip(off));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_stays_inside_and_off_the_chrome() {
        for (w, h) in [(30u16, 8u16), (80, 24), (200, 60)] {
            let area = Rect::new(0, 0, w, h);
            for (wp, hp, mw, mh) in [(86, 86, 60, 16), (70, 0, 50, 13), (60, 70, 50, 20)] {
                let r = popup(area, wp, hp, mw, mh);
                assert!(r.y >= 1, "covers header at {}x{}", w, h);
                assert!(r.y + r.height <= h - 1, "covers status at {}x{}", w, h);
                assert!(r.x + r.width <= w);
            }
        }
    }

    #[test]
    fn disk_parts_add_up() {
        let mut r: Report = serde_json::from_value(serde_json::json!({
            "schema": SCHEMA, "version": "x", "scanned_at": 0, "duration_ms": 0,
            "disk": {"total": 1000, "free": 300, "used": 700}, "roots": [], "total_bytes": 400,
            "buckets": [], "plans": [], "projects": [], "items": [], "warnings": []
        }))
        .unwrap();
        r.buckets = vec![];
        let p = disk_parts(&r);
        assert_eq!(p.iter().sum::<u64>(), 1000);
        r.total_bytes = 5000; // found more than used (sparse files, clones): never negative
        let p = disk_parts(&r);
        assert_eq!(p[0], 0);
        assert_eq!(p[3], 300);
    }

    #[test]
    fn report_only_is_never_not_in_use() {
        let bucket = |id: &str, report_only: bool| {
            serde_json::json!({"id": id, "name": id, "order": 0, "color": "#888888", "symbol": "", "description": "",
                "hidden": false, "report_only": report_only, "bytes": 100, "count": 1, "tiers": {"cold": 100}, "groups": []})
        };
        let r: Report = serde_json::from_value(serde_json::json!({
            "schema": SCHEMA, "version": "x", "scanned_at": 0, "duration_ms": 0,
            "disk": {"total": 1000, "free": 300, "used": 700}, "roots": [], "total_bytes": 100,
            "buckets": [bucket("build", false), bucket("elsewhere", true)], "plans": [], "projects": [], "items": [], "warnings": []
        }))
        .unwrap();
        assert_eq!(disk_parts(&r)[2], 100);
        assert_eq!(bucket_not_in_use(&r.buckets[1]), 0);
        assert_eq!(elsewhere(&r).len(), 1);
    }

    #[test]
    fn thousands_sep() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
