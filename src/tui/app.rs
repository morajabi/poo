//! TUI state and input handling. Drawing lives in `view.rs`, threads in `mod.rs`.

use crate::clean::{self, CleanOptions, CleanProgress, CleanResult};
use crate::model::*;
use crate::ops::{self, Ctx};
use crate::plan::{Filter, PlanResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::BTreeSet;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Overview,
    Browse,
    Projects,
    Worktrees,
    Plans,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Browse, Tab::Projects, Tab::Worktrees, Tab::Plans];
    pub fn title(&self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Browse => "Browse",
            Tab::Projects => "Projects",
            Tab::Worktrees => "Worktrees",
            Tab::Plans => "Plans",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Size,
    Age,
    Name,
}

impl Sort {
    pub fn label(&self) -> &'static str {
        match self {
            Sort::Size => "size",
            Sort::Age => "age",
            Sort::Name => "name",
        }
    }
    fn next(self) -> Sort {
        match self {
            Sort::Size => Sort::Age,
            Sort::Age => Sort::Name,
            Sort::Name => Sort::Size,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Buckets,
    Groups { bucket: String, name: String },
    Items { bucket: Option<String>, group: Option<String>, project: Option<String>, title: String },
}

#[derive(Clone, Debug)]
pub struct Nav {
    pub level: Level,
    pub cursor: usize,
    pub offset: usize,
}

impl Nav {
    fn new(level: Level) -> Nav {
        Nav { level, cursor: 0, offset: 0 }
    }
}

/// One row of the Browse list.
#[derive(Clone, Debug)]
pub struct BRow {
    pub name: String,
    pub bytes: u64,
    pub count: usize,
    /// Bytes not in use (junk, unused, idle) for buckets and groups.
    pub free_bytes: u64,
    /// Shown for context only; managed in its own app, never cleaned by poo.
    pub report_only: bool,
    pub tier: Option<Tier>,
    pub last_used: Option<i64>,
    pub color: Option<String>,
    pub kind: BKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BKind {
    Bucket(String),
    Group { bucket: String, group: String },
    Item(usize),
}

#[derive(Clone, Debug)]
pub struct WRow {
    pub repo: bool,
    pub path: String,
    pub bytes: u64,
    pub state: String,
    pub tier: Option<Tier>,
    pub agent: String,
    pub branch: String,
    pub item_id: Option<String>,
    pub checkouts: u64,
    pub last_activity: Option<i64>,
}

pub struct Review {
    pub items: Vec<Item>,
    pub title: String,
    pub mode: RiskLevel,
    pub trash: bool,
    pub dry: bool,
    /// Per item: Ok(warnings accepted) or Err(why it stays).
    pub admits: Vec<Result<Vec<String>, String>>,
    pub offset: usize,
    pub confirming: bool,
    pub error: Option<String>,
}

impl Review {
    pub fn cleanable(&self) -> (usize, u64) {
        let mut n = 0;
        let mut b = 0;
        for (i, a) in self.items.iter().zip(&self.admits) {
            if a.is_ok() {
                n += 1;
                b += i.total_bytes;
            }
        }
        (n, b)
    }
}

pub struct Cleaning {
    pub total: usize,
    pub total_bytes: u64,
    pub progress: Option<CleanProgress>,
    pub started: Instant,
    pub cancel_requested: bool,
    pub dry: bool,
}

pub struct Summary {
    pub res: CleanResult,
    pub offset: usize,
}

pub enum Modal {
    None,
    Help,
    Review(Review),
    Cleaning(Cleaning),
    Summary(Summary),
}

/// What the event loop should do after a key or mouse event.
pub enum Effect {
    None,
    Quit,
    Refresh,
    Rescan,
    StartClean { items: Vec<Item>, mode: RiskLevel, trash: bool, dry: bool },
    Reveal(String),
}

#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, Tab)>,
    /// List body area (rows only) and its scroll offset.
    pub list: Option<(Rect, usize)>,
    pub modal: Option<Rect>,
}

pub struct ScanState {
    pub phase: String,
    pub detail: String,
    pub bytes: u64,
    pub files: u64,
    pub full: bool,
}

pub struct App {
    pub ctx: Ctx,
    pub report: Option<Report>,
    pub plans: Vec<PlanResult>,
    pub tab: Tab,
    pub nav: Vec<Nav>,
    pub projects_cursor: (usize, usize),
    pub worktrees_cursor: (usize, usize),
    pub plans_cursor: (usize, usize),
    pub sort: Sort,
    pub query: String,
    pub editing: bool,
    pub selected: BTreeSet<String>,
    pub modal: Modal,
    pub scanning: Option<ScanState>,
    pub toast: Option<(String, Instant)>,
    pub animate: bool,
    pub tick: u64,
    pub hits: Hits,
    pub last_click: Option<(u16, u16, Instant)>,
    pub live: bool,
}

impl App {
    pub fn new(ctx: Ctx, report: Option<Report>, animate: bool) -> App {
        let mut a = App {
            ctx,
            report: None,
            plans: vec![],
            tab: Tab::Overview,
            nav: vec![Nav::new(Level::Buckets)],
            projects_cursor: (0, 0),
            worktrees_cursor: (0, 0),
            plans_cursor: (0, 0),
            sort: Sort::Size,
            query: String::new(),
            editing: false,
            selected: BTreeSet::new(),
            modal: Modal::None,
            scanning: None,
            toast: None,
            animate,
            tick: 0,
            hits: Hits::default(),
            last_click: None,
            live: false,
        };
        if let Some(r) = report {
            a.set_report(r);
        }
        a
    }

    pub fn set_report(&mut self, r: Report) {
        self.plans = self
            .ctx
            .cat
            .plans
            .iter()
            .filter_map(|p| ops::make_plan(&self.ctx, &r, &p.id, None, &Filter::default(), crate::plan::Admission::default()).ok())
            .collect();
        // Drop selections that no longer exist.
        let ids: BTreeSet<&str> = r.items.iter().map(|i| i.id.as_str()).collect();
        self.selected.retain(|s| ids.contains(s.as_str()));
        self.report = Some(r);
        self.clamp_cursors();
    }

    pub fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    // ---------------------------------------------------------------- rows

    fn item_filter(&self, level: &Level) -> Filter {
        let mut f = Filter::default();
        match level {
            Level::Buckets => {}
            Level::Groups { bucket, .. } => f.bucket = Some(bucket.clone()),
            Level::Items { bucket, group, project, .. } => {
                f.bucket = bucket.clone();
                f.group = group.clone();
                f.project = project.clone();
            }
        }
        if !self.query.is_empty() {
            f.query = Some(self.query.clone());
        }
        f
    }

    fn sort_rows(&self, rows: &mut [BRow]) {
        match self.sort {
            Sort::Size => rows.sort_by(|a, b| b.bytes.cmp(&a.bytes)),
            Sort::Age => rows.sort_by_key(|r| r.last_used.unwrap_or(i64::MAX)),
            Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
        }
    }

    pub fn browse_rows(&self) -> Vec<BRow> {
        let Some(r) = &self.report else { return vec![] };
        let level = &self.nav.last().unwrap().level;
        let searching = !self.query.is_empty() && !matches!(level, Level::Items { .. });
        let mut rows: Vec<BRow> = match level {
            Level::Buckets if !searching => r
                .buckets
                .iter()
                .filter(|b| !b.hidden && b.count > 0)
                .map(|b| BRow {
                    name: b.name.clone(),
                    bytes: b.bytes,
                    count: b.count,
                    free_bytes: bucket_not_in_use(b),
                    report_only: b.report_only,
                    tier: None,
                    last_used: None,
                    color: Some(b.color.clone()),
                    kind: BKind::Bucket(b.id.clone()),
                })
                .collect(),
            Level::Groups { bucket, .. } if !searching => {
                let mut groups: std::collections::BTreeMap<String, BRow> = Default::default();
                for (idx, i) in r.items.iter().enumerate().filter(|(_, i)| &i.bucket == bucket) {
                    let _ = idx;
                    let g = groups.entry(i.group.clone()).or_insert_with(|| BRow {
                        name: i.group.clone(),
                        bytes: 0,
                        count: 0,
                        free_bytes: 0,
                        report_only: i.report_only,
                        tier: None,
                        last_used: None,
                        color: None,
                        kind: BKind::Group { bucket: bucket.clone(), group: i.group.clone() },
                    });
                    g.bytes += i.bytes;
                    g.count += 1;
                    if cleanable(i) {
                        g.free_bytes += i.bytes;
                    }
                    g.last_used = match (g.last_used, i.last_used) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                }
                groups.into_values().collect()
            }
            _ => {
                let f = self.item_filter(level);
                r.items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| f.matches(i))
                    .map(|(idx, i)| BRow {
                        name: i.name.clone(),
                        bytes: i.total_bytes,
                        count: 1,
                        free_bytes: 0,
                        report_only: i.report_only,
                        tier: Some(i.tier),
                        last_used: i.last_used,
                        color: None,
                        kind: BKind::Item(idx),
                    })
                    .collect()
            }
        };
        if !self.query.is_empty() && matches!(level, Level::Buckets | Level::Groups { .. }) && !searching {
            let q = self.query.to_lowercase();
            rows.retain(|x| x.name.to_lowercase().contains(&q));
        }
        if matches!(level, Level::Buckets) && !searching && self.sort == Sort::Size {
            // Keep the mod's bucket order when sorting by size? No: biggest first reads best.
        }
        self.sort_rows(&mut rows);
        rows
    }

    pub fn project_rows(&self) -> Vec<&Project> {
        let Some(r) = &self.report else { return vec![] };
        let q = self.query.to_lowercase();
        let mut v: Vec<&Project> = r
            .projects
            .iter()
            .filter(|p| q.is_empty() || p.path.to_lowercase().contains(&q) || p.stacks.iter().any(|s| s.contains(&q)))
            .collect();
        match self.sort {
            Sort::Size => v.sort_by(|a, b| b.artifact_bytes.cmp(&a.artifact_bytes)),
            Sort::Age => v.sort_by_key(|p| p.last_activity.unwrap_or(i64::MAX)),
            Sort::Name => v.sort_by_key(|p| p.name.to_lowercase()),
        }
        v
    }

    pub fn worktree_rows(&self) -> Vec<WRow> {
        let Some(r) = &self.report else { return vec![] };
        let v = ops::worktrees(r);
        let q = self.query.to_lowercase();
        let mut out = vec![];
        for repo in v["repos"].as_array().into_iter().flatten() {
            let path = repo["repo"].as_str().unwrap_or("").to_string();
            let wts: Vec<&serde_json::Value> = repo["worktrees"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| q.is_empty() || w["path"].as_str().unwrap_or("").to_lowercase().contains(&q) || w["branch"].as_str().unwrap_or("").to_lowercase().contains(&q))
                .collect();
            if wts.is_empty() {
                continue;
            }
            out.push(WRow {
                repo: true,
                path,
                bytes: repo["build_bytes"].as_u64().unwrap_or(0),
                state: String::new(),
                tier: None,
                agent: String::new(),
                branch: String::new(),
                item_id: None,
                checkouts: repo["checkouts"].as_u64().unwrap_or(0),
                last_activity: None,
            });
            for w in wts {
                let item = &w["item"];
                let state = match (w["dirty"].as_bool(), w["merged"].as_bool()) {
                    (Some(true), _) => "changes",
                    (_, Some(true)) => "merged",
                    _ if w["kind"] == "repo" => "main",
                    _ => "clean",
                };
                out.push(WRow {
                    repo: false,
                    path: w["path"].as_str().unwrap_or("").to_string(),
                    bytes: item["total_bytes"].as_u64().unwrap_or(w["build_bytes"].as_u64().unwrap_or(0)),
                    state: state.into(),
                    tier: item["tier"].as_str().and_then(Tier::parse),
                    agent: w["agent"].as_str().unwrap_or("").to_string(),
                    branch: w["branch"].as_str().unwrap_or("(detached)").to_string(),
                    item_id: item["id"].as_str().map(String::from),
                    checkouts: 0,
                    last_activity: w["last_activity"].as_i64(),
                });
            }
        }
        out
    }

    pub fn list_len(&self) -> usize {
        match self.tab {
            Tab::Overview => 0,
            Tab::Browse => self.browse_rows().len(),
            Tab::Projects => self.project_rows().len(),
            Tab::Worktrees => self.worktree_rows().len(),
            Tab::Plans => self.plans.len(),
        }
    }

    pub fn cursor(&self) -> (usize, usize) {
        match self.tab {
            Tab::Browse => {
                let n = self.nav.last().unwrap();
                (n.cursor, n.offset)
            }
            Tab::Projects => self.projects_cursor,
            Tab::Worktrees => self.worktrees_cursor,
            Tab::Plans => self.plans_cursor,
            Tab::Overview => (0, 0),
        }
    }

    pub fn set_cursor(&mut self, c: usize, o: usize) {
        match self.tab {
            Tab::Browse => {
                let n = self.nav.last_mut().unwrap();
                n.cursor = c;
                n.offset = o;
            }
            Tab::Projects => self.projects_cursor = (c, o),
            Tab::Worktrees => self.worktrees_cursor = (c, o),
            Tab::Plans => self.plans_cursor = (c, o),
            Tab::Overview => {}
        }
    }

    fn clamp_cursors(&mut self) {
        let tab = self.tab;
        for t in [Tab::Browse, Tab::Projects, Tab::Worktrees, Tab::Plans] {
            self.tab = t;
            let len = self.list_len();
            let (c, o) = self.cursor();
            self.set_cursor(c.min(len.saturating_sub(1)), o);
        }
        self.tab = tab;
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let (c, o) = self.cursor();
        let n = (c as isize + delta).clamp(0, len as isize - 1) as usize;
        self.set_cursor(n, o);
    }

    pub fn item(&self, idx: usize) -> Option<&Item> {
        self.report.as_ref().and_then(|r| r.items.get(idx))
    }

    pub fn find(&self, id: &str) -> Option<&Item> {
        self.report.as_ref().and_then(|r| r.items.iter().find(|i| i.id == id))
    }

    pub fn selection_bytes(&self) -> u64 {
        let Some(r) = &self.report else { return 0 };
        r.items.iter().filter(|i| self.selected.contains(&i.id)).map(|i| i.total_bytes).sum()
    }

    /// Items under the cursor row (an item, or all items in a bucket/group/plan/worktree).
    fn cursor_items(&self) -> Vec<String> {
        let (c, _) = self.cursor();
        match self.tab {
            Tab::Browse => match self.browse_rows().get(c).map(|r| r.kind.clone()) {
                Some(BKind::Item(i)) => self.item(i).filter(|x| !x.report_only).map(|x| vec![x.id.clone()]).unwrap_or_default(),
                Some(BKind::Bucket(b)) => self.report.iter().flat_map(|r| r.items.iter()).filter(|i| i.bucket == b && cleanable(i)).map(|i| i.id.clone()).collect(),
                Some(BKind::Group { bucket, group }) => self
                    .report
                    .iter()
                    .flat_map(|r| r.items.iter())
                    .filter(|i| i.bucket == bucket && i.group == group && cleanable(i))
                    .map(|i| i.id.clone())
                    .collect(),
                None => vec![],
            },
            Tab::Projects => {
                let rows = self.project_rows();
                let Some(p) = rows.get(c) else { return vec![] };
                let f = Filter { project: Some(p.path.clone()), ..Default::default() };
                self.report.iter().flat_map(|r| r.items.iter()).filter(|i| f.matches(i) && cleanable(i)).map(|i| i.id.clone()).collect()
            }
            Tab::Worktrees => self.worktree_rows().get(c).and_then(|w| w.item_id.clone()).into_iter().collect(),
            Tab::Plans => self.plans.get(c).map(|p| p.items.iter().map(|i| i.id.clone()).collect()).unwrap_or_default(),
            Tab::Overview => vec![],
        }
    }

    fn toggle_select(&mut self) {
        let ids = self.cursor_items();
        if ids.is_empty() {
            return;
        }
        let all_in = ids.iter().all(|i| self.selected.contains(i));
        for id in ids {
            if all_in {
                self.selected.remove(&id);
            } else {
                self.selected.insert(id);
            }
        }
    }

    fn select_visible(&mut self) {
        if self.tab != Tab::Browse {
            return self.toggle_select();
        }
        let rows = self.browse_rows();
        for r in rows {
            if let BKind::Item(i) = r.kind {
                if let Some(it) = self.item(i) {
                    if cleanable(it) {
                        let id = it.id.clone();
                        self.selected.insert(id);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- review

    pub fn open_review(&mut self, items: Vec<Item>, title: String) {
        if items.is_empty() {
            self.toast("Nothing to clean here");
            return;
        }
        let mut rv = Review { items, title, mode: RiskLevel::Standard, trash: self.ctx.cfg.backup.use_trash, dry: false, admits: vec![], offset: 0, confirming: false, error: None };
        // Cleanable first, biggest first.
        self.readmit(&mut rv);
        let mut pairs: Vec<(Item, Result<Vec<String>, String>)> = rv.items.drain(..).zip(rv.admits.drain(..)).collect();
        pairs.sort_by(|a, b| a.1.is_err().cmp(&b.1.is_err()).then(b.0.total_bytes.cmp(&a.0.total_bytes)));
        let (i, a): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        rv.items = i;
        rv.admits = a;
        self.modal = Modal::Review(rv);
    }

    /// The same admission check `clean_with` runs, so the review shows what will happen.
    fn readmit(&self, rv: &mut Review) {
        let opts = self.clean_options(rv.mode, rv.trash, rv.dry);
        let procs_list = crate::procs::list();
        rv.admits = rv.items.iter().map(|i| clean::admit(i, &self.ctx.cat, &opts, &procs_list)).collect();
    }

    pub fn clean_options(&self, mode: RiskLevel, trash: bool, dry: bool) -> CleanOptions {
        let mut o = CleanOptions::from_config(&self.ctx.cfg, mode, dry, "tui");
        o.trash = trash || self.ctx.cfg.backup.use_trash;
        o.max_tier = Some(Tier::Warm);
        o
    }

    fn review_selected(&mut self) {
        let ids: Vec<String> = if self.selected.is_empty() { self.cursor_items() } else { self.selected.iter().cloned().collect() };
        let items: Vec<Item> = ids.iter().filter_map(|id| self.find(id).cloned()).collect();
        let title = if self.selected.is_empty() { "Clean".to_string() } else { format!("Clean {} selected", self.selected.len()) };
        self.open_review(items, title);
    }

    // ---------------------------------------------------------------- input

    pub fn on_key(&mut self, k: KeyEvent) -> Effect {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Modals first.
        match &mut self.modal {
            Modal::Help => {
                self.modal = Modal::None;
                return Effect::None;
            }
            Modal::Cleaning(c) => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q')) || ctrl {
                    c.cancel_requested = true;
                    clean::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                return Effect::None;
            }
            Modal::Summary(s) => {
                match k.code {
                    KeyCode::Char('j') | KeyCode::Down => s.offset += 1,
                    KeyCode::Char('k') | KeyCode::Up => s.offset = s.offset.saturating_sub(1),
                    _ => {
                        self.modal = Modal::None;
                        self.selected.clear();
                    }
                }
                return Effect::None;
            }
            Modal::Review(_) => return self.on_review_key(k),
            Modal::None => {}
        }
        if self.editing {
            match k.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.editing = false;
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char('u') if ctrl => self.query.clear(),
                KeyCode::Char(c) if !ctrl => self.query.push(c),
                KeyCode::Down => self.move_cursor(1),
                KeyCode::Up => self.move_cursor(-1),
                _ => {}
            }
            let (_, o) = self.cursor();
            self.set_cursor(0, o);
            return Effect::None;
        }
        let page = 10;
        match k.code {
            KeyCode::Char('c') if ctrl => return Effect::Quit,
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('1') => self.tab = Tab::Overview,
            KeyCode::Char('2') => self.tab = Tab::Browse,
            KeyCode::Char('3') => self.tab = Tab::Projects,
            KeyCode::Char('4') => self.tab = Tab::Worktrees,
            KeyCode::Char('5') => self.tab = Tab::Plans,
            KeyCode::Tab => self.cycle_tab(1),
            KeyCode::BackTab => self.cycle_tab(-1),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('d') if ctrl => self.move_cursor(page),
            KeyCode::Char('u') if ctrl => self.move_cursor(-page),
            KeyCode::PageDown => self.move_cursor(page),
            KeyCode::PageUp => self.move_cursor(-page),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(-(1 << 30)),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(1 << 30),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.enter(),
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.back(),
            KeyCode::Char(' ') => {
                self.toggle_select();
                self.move_cursor(1);
            }
            KeyCode::Char('a') => self.select_visible(),
            KeyCode::Char('u') => self.selected.clear(),
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('/') => {
                if self.tab == Tab::Overview {
                    self.tab = Tab::Browse;
                }
                self.editing = true;
            }
            KeyCode::Char('c') => self.review_selected(),
            KeyCode::Char('r') => return Effect::Refresh,
            KeyCode::Char('R') => return Effect::Rescan,
            KeyCode::Char('o') => {
                if let Some(p) = self.cursor_path() {
                    return Effect::Reveal(p);
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn cycle_tab(&mut self, d: isize) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        self.tab = Tab::ALL[((i + d).rem_euclid(5)) as usize];
    }

    fn cursor_path(&self) -> Option<String> {
        let (c, _) = self.cursor();
        match self.tab {
            Tab::Browse => match self.browse_rows().get(c)?.kind {
                BKind::Item(i) => self.item(i).map(|x| x.path.clone()),
                _ => None,
            },
            Tab::Projects => self.project_rows().get(c).map(|p| p.path.clone()),
            Tab::Worktrees => self.worktree_rows().get(c).map(|w| w.path.clone()),
            _ => None,
        }
    }

    fn enter(&mut self) {
        let (c, _) = self.cursor();
        match self.tab {
            Tab::Overview => self.tab = Tab::Browse,
            Tab::Browse => {
                let Some(row) = self.browse_rows().get(c).cloned() else { return };
                let next = match row.kind {
                    BKind::Bucket(b) => Level::Groups { bucket: b, name: row.name },
                    BKind::Group { bucket, group } => Level::Items { bucket: Some(bucket), group: Some(group.clone()), project: None, title: group },
                    BKind::Item(_) => return,
                };
                self.query.clear();
                self.nav.push(Nav::new(next));
            }
            Tab::Projects => {
                let Some(p) = self.project_rows().get(c).map(|p| (p.path.clone(), p.name.clone())) else { return };
                self.query.clear();
                self.nav = vec![Nav::new(Level::Buckets), Nav::new(Level::Items { bucket: None, group: None, project: Some(p.0), title: p.1 })];
                self.tab = Tab::Browse;
            }
            Tab::Worktrees => {
                let Some(w) = self.worktree_rows().get(c).cloned() else { return };
                let name = std::path::Path::new(&w.path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.query.clear();
                self.nav = vec![Nav::new(Level::Buckets), Nav::new(Level::Items { bucket: None, group: None, project: Some(w.path), title: name })];
                self.tab = Tab::Browse;
            }
            Tab::Plans => {
                if let Some(p) = self.plans.get(c) {
                    let (items, title) = (p.items.clone(), format!("Plan · {}", p.name));
                    self.open_review(items, title);
                }
            }
        }
    }

    fn back(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            return;
        }
        if self.tab == Tab::Browse && self.nav.len() > 1 {
            self.nav.pop();
        } else if self.tab != Tab::Overview && !self.selected.is_empty() {
            // Esc with nothing to go back to keeps the selection; `u` clears it.
        }
    }

    fn on_review_key(&mut self, k: KeyEvent) -> Effect {
        let Modal::Review(mut rv) = std::mem::replace(&mut self.modal, Modal::None) else { return Effect::None };
        if rv.confirming {
            match k.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let items: Vec<Item> = rv.items.iter().zip(&rv.admits).filter(|(_, a)| a.is_ok()).map(|(i, _)| i.clone()).collect();
                    let eff = Effect::StartClean { items, mode: rv.mode, trash: rv.trash, dry: rv.dry };
                    rv.confirming = false;
                    self.modal = Modal::Review(rv);
                    return eff;
                }
                _ => rv.confirming = false,
            }
            self.modal = Modal::Review(rv);
            return Effect::None;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left => return Effect::None,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Effect::None,
            KeyCode::Enter | KeyCode::Char('c') => {
                if rv.cleanable().0 > 0 {
                    rv.confirming = true;
                    rv.error = None;
                } else {
                    rv.error = Some("Nothing here can be cleaned in this mode".into());
                }
            }
            KeyCode::Char('f') => {
                rv.mode = if rv.mode == RiskLevel::Standard { RiskLevel::Force } else { RiskLevel::Standard };
                self.readmit(&mut rv);
            }
            KeyCode::Char('t') => {
                rv.trash = !rv.trash;
                self.readmit(&mut rv);
            }
            KeyCode::Char('p') => {
                rv.dry = !rv.dry;
                self.readmit(&mut rv);
            }
            KeyCode::Char('j') | KeyCode::Down => rv.offset = (rv.offset + 1).min(rv.items.len().saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => rv.offset = rv.offset.saturating_sub(1),
            KeyCode::PageDown => rv.offset = (rv.offset + 10).min(rv.items.len().saturating_sub(1)),
            KeyCode::PageUp => rv.offset = rv.offset.saturating_sub(10),
            _ => {}
        }
        self.modal = Modal::Review(rv);
        Effect::None
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Effect {
        match m.kind {
            MouseEventKind::ScrollDown => match &mut self.modal {
                Modal::Review(rv) => rv.offset = (rv.offset + 1).min(rv.items.len().saturating_sub(1)),
                Modal::Summary(s) => s.offset += 1,
                Modal::None => self.move_cursor(1),
                _ => {}
            },
            MouseEventKind::ScrollUp => match &mut self.modal {
                Modal::Review(rv) => rv.offset = rv.offset.saturating_sub(1),
                Modal::Summary(s) => s.offset = s.offset.saturating_sub(1),
                Modal::None => self.move_cursor(-1),
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Left) => {
                if !matches!(self.modal, Modal::None) {
                    // Clicking outside a help or summary popup closes it.
                    if matches!(self.modal, Modal::Help | Modal::Summary(_)) && !self.hits.modal.map_or(false, |r| contains(r, m.column, m.row)) {
                        self.modal = Modal::None;
                    }
                    return Effect::None;
                }
                for (r, t) in &self.hits.tabs {
                    if contains(*r, m.column, m.row) {
                        self.tab = *t;
                        return Effect::None;
                    }
                }
                if let Some((area, offset)) = self.hits.list {
                    if contains(area, m.column, m.row) {
                        let row = offset + (m.row - area.y) as usize;
                        if row < self.list_len() {
                            let (c, o) = self.cursor();
                            let double = self.last_click.map_or(false, |(x, y, t)| x == m.column && y == m.row && t.elapsed().as_millis() < 400);
                            self.set_cursor(row, o);
                            self.last_click = Some((m.column, m.row, Instant::now()));
                            if row == c && double {
                                self.enter();
                            }
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if matches!(self.modal, Modal::None) {
                    self.back();
                }
            }
            _ => {}
        }
        Effect::None
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

pub fn cleanable_tier(t: Tier) -> bool {
    matches!(t, Tier::Junk | Tier::Cold | Tier::Idle)
}

pub fn not_in_use(tiers: &std::collections::BTreeMap<String, u64>) -> u64 {
    ["junk", "cold", "idle"].iter().filter_map(|t| tiers.get(*t)).sum()
}

/// Whether poo would offer this item for cleaning: not in use, and not report-only
/// (those are managed in their own apps and never cleaned by poo).
pub fn cleanable(i: &Item) -> bool {
    !i.report_only && cleanable_tier(i.tier)
}

/// Bytes not in use in an area; report-only areas never count.
pub fn bucket_not_in_use(b: &crate::model::BucketSummary) -> u64 {
    if b.report_only {
        0
    } else {
        not_in_use(&b.tiers)
    }
}

/// Report-only areas with something in them: (name, bytes, owner hint from the largest item).
pub fn elsewhere(r: &crate::model::Report) -> Vec<(String, u64, Option<String>)> {
    r.buckets
        .iter()
        .filter(|b| b.report_only && b.bytes > 0)
        .map(|b| {
            let hint = r.items.iter().filter(|i| i.bucket == b.id && i.recommend.is_some()).max_by_key(|i| i.bytes).and_then(|i| i.recommend.clone());
            (b.name.clone(), b.bytes, hint)
        })
        .collect()
}
