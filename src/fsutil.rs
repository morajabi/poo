use crate::model::{Child, DiskInfo};
use crate::util::system_time_secs;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub alloc: u64,
    pub mtime: i64,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    /// Bytes deleting this entry alone frees (APFS: blocks shared with clones excluded).
    pub private: u64,
}

/// Folders macOS guards with a consent prompt. Until someone answers, the first read of
/// one blocks: forever, if nobody is at the Mac. Each is probed once, off-thread, with a
/// timeout; a folder that does not answer is skipped for the rest of the run.
/// Folders whose children are probed one by one: macOS can guard any of them with a
/// consent prompt (another app's data, media, iCloud), and which ones differs per Mac.
/// Longest first, so the most specific parent decides.
fn guarded_parents() -> &'static Vec<PathBuf> {
    static ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let h = crate::util::home();
        let mut v: Vec<PathBuf> = [
            "", "Library", "Library/Application Support", "Library/Caches", "Library/Containers",
            "Library/Group Containers", "Library/Daemon Containers", "Pictures", "Music", "Movies",
        ]
        .iter()
        .map(|p| if p.is_empty() { h.clone() } else { h.join(p) })
        .chain(std::iter::once(PathBuf::from("/Volumes")))
        .collect();
        v.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
        v
    })
}

static BLOCKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static HELD_BACK: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether a first read inside `point` (a child of a guarded parent) can show a macOS
/// privacy prompt: Files & Folders, other apps' data, Contacts, Calendars, iCloud Drive,
/// media libraries, removable and network volumes.
fn prompts(point: &Path) -> bool {
    let (Some(parent), Some(name)) = (point.parent(), point.file_name().and_then(|n| n.to_str())) else { return false };
    if parent == Path::new("/Volumes") {
        return true;
    }
    let h = crate::util::home();
    let Ok(rel) = parent.strip_prefix(&h) else { return false };
    match rel.to_str().unwrap_or("") {
        "" => matches!(name, "Desktop" | "Documents" | "Downloads" | "Pictures" | "Music" | "Movies"),
        "Pictures" | "Music" | "Movies" => true,
        "Library/Containers" | "Library/Group Containers" | "Library/Daemon Containers" => true,
        "Library" => matches!(
            name,
            "Mobile Documents" | "CloudStorage" | "Calendars" | "Reminders" | "Mail" | "Messages" | "Safari" | "HomeKit"
                | "Photos" | "Accounts" | "Cookies" | "Suggestions" | "IdentityServices" | "Sharing" | "Biome"
        ),
        "Library/Application Support" => matches!(
            name,
            "AddressBook" | "CallHistoryDB" | "CallHistoryTransactions" | "FaceTime" | "Knowledge" | "MobileSync"
                | "com.apple.TCC" | "CloudDocs"
        ),
        "Library/Caches" => matches!(
            name,
            "CloudKit" | "com.apple.Safari" | "com.apple.Music" | "com.apple.HomeKit" | "com.apple.findmy.fmipcore"
                | "FamilyCircle" | "com.apple.homed" | "com.apple.containermanagerd" | "com.apple.ap.adprivacyd"
        ),
        _ => false,
    }
}

fn full_disk_access() -> bool {
    static FDA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FDA.get_or_init(crate::util::has_full_disk_access)
}

/// Folders the user added as roots: reading inside them is what they asked for.
fn chosen_roots() -> &'static Vec<PathBuf> {
    static ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        crate::config::Config::load()
            .map(|c| c.roots.iter().map(|r| crate::util::expand(r)).collect())
            .unwrap_or_default()
    })
}

/// Poo never asks macOS for a folder on its own. Without Full Disk Access, a folder that
/// would show a privacy prompt is read only when it answered before (allowed or denied, so
/// no prompt now) or holds a folder the user chose to scan. The rest is skipped and
/// reported once; Full Disk Access covers all of it with one switch.
pub fn held_back(dir: &Path) -> bool {
    if full_disk_access() {
        return false;
    }
    let Some(point) = guard_point(dir).filter(|p| prompts(p)) else { return false };
    if answered().lock().unwrap().contains(&point) {
        return false;
    }
    !chosen_roots().iter().any(|r| r.starts_with(&point) || point.starts_with(r))
}

/// Prompt-guarded folders skipped this run because Poo lacks Full Disk Access.
pub fn held_back_folders() -> Vec<PathBuf> {
    let mut v = HELD_BACK.lock().unwrap().clone();
    v.sort();
    v.dedup();
    v
}

/// Guarded folders that did not answer in time during this run.
pub fn blocked_folders() -> Vec<PathBuf> {
    let mut v = BLOCKED.lock().unwrap().clone();
    v.sort();
    v.dedup();
    v
}

/// The folder whose answer decides whether reading `path` might block, if any: the child
/// of the most specific guarded parent that `path` is in.
fn guard_point(path: &Path) -> Option<PathBuf> {
    let parent = guarded_parents().iter().find(|r| path.starts_with(r) && path != r.as_path())?;
    let child = path.strip_prefix(parent).ok()?.components().next()?;
    Some(parent.join(child))
}

/// False when `path` is inside a guarded folder that does not answer (a pending privacy
/// prompt, a hung network volume or cloud provider).
pub fn readable_without_blocking(path: &Path) -> bool {
    thread_local! {
        static LAST: std::cell::RefCell<Option<(PathBuf, bool)>> = const { std::cell::RefCell::new(None) };
    }
    let Some(g) = guard_point(path) else { return true };
    if let Some(ok) = LAST.with(|l| l.borrow().as_ref().filter(|(p, _)| *p == g).map(|(_, ok)| *ok)) {
        return ok;
    }
    let ok = answers(&g);
    LAST.with(|l| *l.borrow_mut() = Some((g, ok)));
    ok
}

/// Probes the guard points of many paths at once (one shared 3s wait, not one per folder)
/// and returns which paths are safe to read.
pub fn readable_many(paths: &[PathBuf]) -> Vec<bool> {
    let mut points: Vec<PathBuf> = paths.iter().filter_map(|p| guard_point(p)).collect();
    points.sort();
    points.dedup();
    probe_many(&points);
    paths.iter().map(|p| readable_without_blocking(p)).collect()
}

/// Whether listing `dir` returns within a few seconds. macOS blocks the first read of
/// privacy-guarded folders (another app's container, media libraries, Downloads) until
/// someone answers a prompt. Probed once per folder per run, off-thread; a folder that
/// does not answer is skipped and reported, never waited on.
pub fn answers(dir: &Path) -> bool {
    probe_many(std::slice::from_ref(&dir.to_path_buf()));
    probe_cache().lock().unwrap().get(dir).copied().unwrap_or(true)
}

fn probe_cache() -> &'static Mutex<std::collections::HashMap<PathBuf, bool>> {
    static CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<PathBuf, bool>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Probes folders not yet known, all at once, within one shared deadline.
fn probe_many(dirs: &[PathBuf]) {
    let cache = probe_cache();
    let silent = load_silent();
    let now = crate::util::now();
    let mut todo = vec![];
    {
        let mut c = cache.lock().unwrap();
        for d in dirs {
            if c.contains_key(d) {
                continue;
            }
            if held_back(d) {
                c.insert(d.clone(), false);
                HELD_BACK.lock().unwrap().extend(guard_point(d));
                continue;
            }
            // Silence is remembered across runs for a while, so later commands skip it at once.
            if silent.get(d).map_or(false, |t| now - t < SILENT_FOR) {
                c.insert(d.clone(), false);
                BLOCKED.lock().unwrap().push(d.clone());
                continue;
            }
            todo.push(d.clone());
        }
    }
    if todo.is_empty() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    for (i, d) in todo.iter().enumerate() {
        let tx = tx.clone();
        let d = d.clone();
        std::thread::spawn(move || {
            // An error (missing, no permission) answers too: only silence is a problem.
            let _ = std::fs::read_dir(&d).map(|mut it| it.next());
            let _ = tx.send(i);
        });
    }
    drop(tx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut answered = vec![false; todo.len()];
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(i) => answered[i] = true,
            Err(_) => break,
        }
        if answered.iter().all(|a| *a) {
            break;
        }
    }
    let mut silent_now = vec![];
    let mut newly_answered = vec![];
    let mut c = cache.lock().unwrap();
    for (d, ok) in todo.into_iter().zip(answered) {
        if !ok {
            BLOCKED.lock().unwrap().push(d.clone());
            silent_now.push(d.clone());
        } else if let Some(p) = guard_point(&d).filter(|p| prompts(p)) {
            newly_answered.push(p);
        }
        c.insert(d, ok);
    }
    drop(c);
    if !silent_now.is_empty() {
        remember_silent(&silent_now);
    }
    if !newly_answered.is_empty() {
        remember_answered(newly_answered);
    }
}

pub fn read_entries(dir: &Path) -> Vec<Entry> {
    if !readable_without_blocking(dir) {
        return vec![];
    }
    if let Some(v) = crate::bulk::read_entries_bulk(dir) {
        return v;
    }
    read_entries_std(dir)
}

/// Like `read_entries` without APFS private sizes (`private` is `alloc`): for walking
/// structure and matching names, never for sizing. APFS looks up every file's extents to
/// answer the private size, which costs about as much as the rest of the read.
pub fn read_entries_lite(dir: &Path) -> Vec<Entry> {
    if !readable_without_blocking(dir) {
        return vec![];
    }
    let want = crate::bulk::Want { private: false, flags: false };
    if let Some(v) = crate::bulk::read_entries_bulk_with(dir, want) {
        return v.into_iter().map(|(e, _)| e).collect();
    }
    read_entries_std(dir)
}

/// Portable reader: `read_dir` plus an `lstat` per entry. Fallback for `read_entries`.
pub fn read_entries_std(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        let ft = md.file_type();
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
            alloc: md.blocks() * 512,
            mtime: md.mtime(),
            dev: md.dev(),
            ino: md.ino(),
            nlink: md.nlink(),
            private: md.blocks() * 512,
        });
    }
    out
}

pub fn names(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|e| e.name.clone()).collect()
}

/// Folders at least this big are remembered between scans, so a rescan re-measures only the
/// branches that changed.
pub const MEMO_MIN_BYTES: u64 = 32_000_000;

// Ages (`Sized::newest`) come from files only. A folder's own mtime changes when anything
// inside is added or removed, including by a cleanup, which is not use.

const SEEN_SHARDS: usize = 32;

/// Shared state for one scan: hardlink dedupe, progress counters and cancellation.
#[derive(Default)]
pub struct SizeCtx {
    /// Hard links already counted, sharded by inode so parallel walks of pnpm-style trees
    /// (every file a hard link) do not queue on one lock.
    seen: [Mutex<HashSet<(u64, u64)>>; SEEN_SHARDS],
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
    /// Record big folders into `big` while sizing.
    pub record: bool,
    pub big: Mutex<Vec<(PathBuf, Sized)>>,
    /// Sizes of big folders from the last scan (incremental scans).
    pub memo: HashMap<PathBuf, Sized>,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Sized {
    pub bytes: u64,
    pub files: u64,
    pub newest: i64,
    /// What deleting it frees: like `bytes`, minus blocks shared with APFS clones elsewhere
    /// (pnpm stores, cloned checkouts, copied Xcode builds).
    #[serde(default)]
    pub frees: u64,
}

impl Sized {
    /// A size measured some other way (a tool's report): it frees what it uses.
    pub fn of(bytes: u64, files: u64, newest: i64) -> Sized {
        Sized { bytes, files, newest, frees: bytes }
    }

    fn add(mut self, o: Sized) -> Sized {
        self.frees += o.frees;
        self.bytes += o.bytes;
        self.files += o.files;
        self.newest = self.newest.max(o.newest);
        self
    }
}

impl SizeCtx {
    /// A context that records big folders and reuses `memo` for untouched branches.
    pub fn recording(memo: HashMap<PathBuf, Sized>) -> Self {
        SizeCtx { record: true, memo, ..Default::default() }
    }

    fn file_bytes(&self, e: &Entry) -> u64 {
        if e.nlink > 1 && !e.is_dir {
            let mut seen = self.seen[(e.ino as usize) % SEEN_SHARDS].lock().unwrap();
            if !seen.insert((e.dev, e.ino)) {
                return 0;
            }
        }
        e.alloc
    }

    /// Sizes a directory tree without following symlinks or crossing devices.
    pub fn size_dir(&self, path: &Path, dev: u64, exclude: &HashSet<PathBuf>) -> Sized {
        if self.cancel.load(Ordering::Relaxed) {
            return Sized::default();
        }
        let entries = read_entries(path);
        self.size_entries(&entries, dev, exclude).0
    }

    /// Sizes already-read entries and returns per-child sizes for breakdowns.
    pub fn size_entries(&self, entries: &[Entry], dev: u64, exclude: &HashSet<PathBuf>) -> (Sized, Vec<Child>) {
        let mut total = Sized::default();
        let mut children = Vec::with_capacity(entries.len());
        let mut dirs = vec![];
        for e in entries {
            if !exclude.is_empty() && exclude.contains(&e.path) {
                continue;
            }
            if e.is_dir && !e.is_symlink {
                if e.dev != dev {
                    continue;
                }
                dirs.push(e);
                continue;
            }
            let b = self.file_bytes(e);
            total.bytes += b;
            total.frees += if b == 0 { 0 } else { e.private.min(b) };
            total.files += 1;
            total.newest = total.newest.max(e.mtime);
            children.push(Child { name: e.name.clone(), bytes: b });
        }
        self.files.fetch_add(total.files, Ordering::Relaxed);
        self.bytes.fetch_add(total.bytes, Ordering::Relaxed);
        let sub: Vec<(String, Sized)> = dirs
            .par_iter()
            .map(|d| {
                let mut s = self.size_dir(&d.path, dev, exclude);
                s.bytes += d.alloc;
                s.frees += d.alloc;
                self.remember(&d.path, s);
                (d.name.clone(), s)
            })
            .collect();
        for (name, s) in sub {
            children.push(Child { name, bytes: s.bytes });
            total = total.add(s);
        }
        children.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        (total, children)
    }

    fn remember(&self, path: &Path, s: Sized) {
        if self.record && s.bytes >= MEMO_MIN_BYTES {
            self.big.lock().unwrap().push((path.to_path_buf(), s));
        }
    }

    /// Like `size_entries`, but subfolders with no change below them reuse their size from
    /// `memo`. `changed` lists folders whose contents changed, all inside the folder.
    fn size_entries_delta(&self, entries: &[Entry], dev: u64, exclude: &HashSet<PathBuf>, changed: &[&Path]) -> (Sized, Vec<Child>) {
        let mut total = Sized::default();
        let mut children = Vec::with_capacity(entries.len());
        let mut dirs = vec![];
        for e in entries {
            if !exclude.is_empty() && exclude.contains(&e.path) {
                continue;
            }
            if e.is_dir && !e.is_symlink {
                if e.dev == dev {
                    dirs.push(e);
                }
                continue;
            }
            let b = self.file_bytes(e);
            total.bytes += b;
            total.frees += if b == 0 { 0 } else { e.private.min(b) };
            total.files += 1;
            total.newest = total.newest.max(e.mtime);
            children.push(Child { name: e.name.clone(), bytes: b });
        }
        let sub: Vec<(String, Sized)> = dirs
            .par_iter()
            .map(|d| {
                let below: Vec<&Path> = changed.iter().copied().filter(|c| c.starts_with(&d.path)).collect();
                let s = match self.memo.get(&d.path) {
                    Some(m) if below.is_empty() => *m,
                    _ => {
                        let mut s = if below.is_empty() {
                            self.size_dir(&d.path, dev, exclude)
                        } else {
                            self.size_entries_delta(&read_entries(&d.path), dev, exclude, &below).0
                        };
                        s.bytes += d.alloc;
                        s.frees += d.alloc;
                        s
                    }
                };
                self.remember(&d.path, s);
                (d.name.clone(), s)
            })
            .collect();
        for (name, s) in sub {
            children.push(Child { name, bytes: s.bytes });
            total = total.add(s);
        }
        children.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        (total, children)
    }

    /// Re-measures a folder after `changed` folders inside it changed, reusing remembered
    /// sizes of untouched branches.
    pub fn size_path_delta(&self, path: &Path, exclude: &HashSet<PathBuf>, changed: &[&Path]) -> Option<(Sized, Vec<Child>)> {
        let md = std::fs::symlink_metadata(path).ok()?;
        if !md.is_dir() || md.file_type().is_symlink() {
            return self.size_path(path, exclude);
        }
        let entries = read_entries(path);
        let (mut s, children) = self.size_entries_delta(&entries, md.dev(), exclude, changed);
        s.bytes += md.blocks() * 512;
        s.frees += md.blocks() * 512;
        Some((s, children))
    }

    /// Sizes a path that may be a file or a directory.
    pub fn size_path(&self, path: &Path, exclude: &HashSet<PathBuf>) -> Option<(Sized, Vec<Child>)> {
        let md = std::fs::symlink_metadata(path).ok()?;
        if md.file_type().is_symlink() {
            return Some((Sized::of(0, 0, md.mtime()), vec![]));
        }
        if md.is_dir() {
            let entries = read_entries(path);
            let (mut s, children) = self.size_entries(&entries, md.dev(), exclude);
            s.bytes += md.blocks() * 512;
            s.frees += md.blocks() * 512;
            Some((s, children))
        } else {
            let e = Entry {
                name: String::new(),
                path: path.to_path_buf(),
                is_dir: false,
                is_symlink: false,
                alloc: md.blocks() * 512,
                mtime: md.mtime(),
                dev: md.dev(),
                ino: md.ino(),
                nlink: md.nlink(),
                private: md.blocks() * 512,
            };
            let b = self.file_bytes(&e);
            self.bytes.fetch_add(b, Ordering::Relaxed);
            Some((Sized::of(b, 1, md.mtime()), vec![]))
        }
    }
}

pub fn has_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[') || s.contains('{')
}

/// Expands a path pattern component by component. Only matching components read directories.
pub fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
    let mut current: Vec<PathBuf> = vec![PathBuf::from("/")];
    for comp in pattern.components() {
        let c = comp.as_os_str().to_string_lossy();
        if c == "/" {
            continue;
        }
        let mut next = vec![];
        if has_glob(&c) {
            let Ok(g) = globset::Glob::new(&c) else { return vec![] };
            let m = g.compile_matcher();
            let ok = readable_many(&current);
            for (base, ok) in current.iter().zip(ok) {
                if !ok {
                    continue;
                }
                let Ok(rd) = std::fs::read_dir(base) else { continue };
                for e in rd.flatten() {
                    let n = e.file_name();
                    if m.is_match(n.to_string_lossy().as_ref()) {
                        next.push(base.join(n));
                    }
                }
            }
        } else {
            for base in &current {
                let p = base.join(c.as_ref());
                if std::fs::symlink_metadata(&p).is_ok() {
                    next.push(p);
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    // Whatever is read next (sized, listed) must answer: probe all matches at once.
    let ok = readable_many(&current);
    let mut current: Vec<PathBuf> = current.into_iter().zip(ok).filter(|(_, ok)| *ok).map(|(p, _)| p).collect();
    current.sort();
    current
}

pub fn disk_info(path: &Path) -> DiskInfo {
    use std::ffi::CString;
    let Ok(c) = CString::new(path.as_os_str().to_string_lossy().as_bytes()) else {
        return DiskInfo::default();
    };
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return DiskInfo::default();
    }
    let bsize = s.f_bsize as u64;
    let total = s.f_blocks as u64 * bsize;
    let free = s.f_bavail as u64 * bsize;
    DiskInfo { total, free, used: total.saturating_sub(free) }
}

pub fn mtime(path: &Path) -> Option<i64> {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(system_time_secs)
}

pub fn dev_of(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|m| m.dev())
}

/// A folder that stayed silent is most likely showing a privacy prompt nobody answered:
/// leave it alone for a day rather than asking again.
const SILENT_FOR: i64 = 24 * 60 * 60;

fn answered_file() -> PathBuf {
    crate::config::state_dir().join("answered-folders.json")
}

/// Prompt-guarded folders that answered once (allowed or denied): reading them again never
/// shows a prompt, so they stay readable without Full Disk Access.
fn answered() -> &'static Mutex<HashSet<PathBuf>> {
    static SET: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
    SET.get_or_init(|| {
        let v: Vec<PathBuf> = std::fs::read(answered_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Mutex::new(v.into_iter().collect())
    })
}

fn remember_answered(points: Vec<PathBuf>) {
    let mut set = answered().lock().unwrap();
    let before = set.len();
    set.extend(points);
    if set.len() == before {
        return;
    }
    // Merge with what other runs wrote meanwhile.
    let disk: Vec<PathBuf> = std::fs::read(answered_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    set.extend(disk);
    let mut all: Vec<&PathBuf> = set.iter().collect();
    all.sort();
    let f = answered_file();
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    if let Ok(data) = serde_json::to_vec(&all) {
        let tmp = f.with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, f);
        }
    }
}

fn silent_file() -> PathBuf {
    crate::config::state_dir().join("silent-folders.json")
}

fn load_silent() -> std::collections::HashMap<PathBuf, i64> {
    std::fs::read(silent_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn remember_silent(dirs: &[PathBuf]) {
    let mut m = load_silent();
    let now = crate::util::now();
    m.retain(|_, t| now - *t < SILENT_FOR);
    for d in dirs {
        m.insert(d.clone(), now);
    }
    let f = silent_file();
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    if let Ok(data) = serde_json::to_vec(&m) {
        let tmp = f.with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, f);
        }
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn folders_macos_guards_with_a_prompt() {
        let h = crate::util::home();
        for p in ["Downloads", "Documents", "Music/Music", "Library/Containers/com.docker.docker", "Library/Mobile Documents", "Library/Application Support/AddressBook"] {
            assert!(prompts(&guard_point(&h.join(p).join("x")).unwrap()), "{p}");
        }
        for p in ["dev", ".cargo", "Library/Caches/com.apple.dt.Xcode", "Library/Developer", "Library/Application Support/Code"] {
            assert!(guard_point(&h.join(p).join("x")).map_or(true, |g| !prompts(&g)), "{p}");
        }
        assert!(prompts(Path::new("/Volumes/External")));
    }
}
