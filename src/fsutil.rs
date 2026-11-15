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
}

pub fn read_entries(dir: &Path) -> Vec<Entry> {
    if let Some(v) = crate::bulk::read_entries_bulk(dir) {
        return v;
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

/// Shared state for one scan: hardlink dedupe, progress counters and cancellation.
#[derive(Default)]
pub struct SizeCtx {
    seen: Mutex<HashSet<(u64, u64)>>,
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
}

impl Sized {
    fn add(mut self, o: Sized) -> Sized {
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
            let mut seen = self.seen.lock().unwrap();
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
        Some((s, children))
    }

    /// Sizes a path that may be a file or a directory.
    pub fn size_path(&self, path: &Path, exclude: &HashSet<PathBuf>) -> Option<(Sized, Vec<Child>)> {
        let md = std::fs::symlink_metadata(path).ok()?;
        if md.file_type().is_symlink() {
            return Some((Sized { bytes: 0, files: 0, newest: md.mtime() }, vec![]));
        }
        if md.is_dir() {
            let entries = read_entries(path);
            let (mut s, children) = self.size_entries(&entries, md.dev(), exclude);
            s.bytes += md.blocks() * 512;
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
            };
            let b = self.file_bytes(&e);
            self.bytes.fetch_add(b, Ordering::Relaxed);
            Some((Sized { bytes: b, files: 1, newest: md.mtime() }, vec![]))
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
            for base in &current {
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
