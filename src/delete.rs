//! Deleting fast without giving up safety.
//!
//! Once `clean` has re-checked an item, its folder is renamed into the graveyard: one
//! `rename(2)` on the same volume, so the item is gone from where it was at once. A detached
//! reaper (`poo gc`) then erases the graveyard in parallel, survives the clean returning, and
//! picks up whatever an earlier reaper left when it was interrupted.
//!
//! Erasing works relative to open folder descriptors (`openat`, `unlinkat`): no path is looked
//! up twice, no symlink is ever followed (a symlink is removed, never what it points to), and
//! it never crosses into another volume mounted inside the tree.

use crate::config::state_dir;
use rayon::prelude::*;
use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Where deleted folders wait to be erased.
pub fn graveyard() -> PathBuf {
    state_dir().join("graveyard")
}

/// What erasing a tree removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Erased {
    pub files: u64,
    pub dirs: u64,
    /// Allocated bytes of the files, when asked to measure.
    pub bytes: u64,
}

/// An open folder, closed on drop.
struct Dir(libc::c_int);

impl Drop for Dir {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn strerror(e: i32) -> String {
    io::Error::from_raw_os_error(e).to_string()
}

#[derive(Default)]
struct Tally {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    first_error: Mutex<Option<String>>,
}

impl Tally {
    fn fail(&self, name: &CStr, what: String) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        let mut first = self.first_error.lock().unwrap_or_else(|e| e.into_inner());
        if first.is_none() {
            *first = Some(format!("{}: {}", name.to_string_lossy(), what));
        }
    }
}

const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

fn fstat(fd: libc::c_int) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st)
}

fn lstat_at(dir: libc::c_int, name: &CStr) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0).then_some(st)
}

/// Opens a folder inside `parent` without following a symlink. A folder without read or
/// search permission (the Go module cache makes these) gets rwx for its owner, set on the
/// entry itself and never through a symlink, and is opened again.
fn open_dir_at(parent: libc::c_int, name: &CStr) -> Result<Dir, i32> {
    let mut fixed = false;
    loop {
        let fd = unsafe { libc::openat(parent, name.as_ptr(), DIR_FLAGS) };
        if fd >= 0 {
            return Ok(Dir(fd));
        }
        match errno() {
            libc::EINTR => continue,
            libc::EACCES if !fixed => {
                fixed = true;
                unsafe { libc::fchmodat(parent, name.as_ptr(), libc::S_IRWXU as libc::mode_t, libc::AT_SYMLINK_NOFOLLOW) };
            }
            e => return Err(e),
        }
    }
}

/// Removes one entry of an open folder. A read-only folder is made writable for its owner
/// (through its descriptor, so no path is looked up) and the removal tried again.
fn unlink_at(dir: &Dir, name: &CStr, flags: libc::c_int) -> Result<(), i32> {
    let mut fixed = false;
    loop {
        if unsafe { libc::unlinkat(dir.0, name.as_ptr(), flags) } == 0 {
            return Ok(());
        }
        match errno() {
            // Already gone: another reaper got there first.
            libc::ENOENT => return Ok(()),
            libc::EINTR => continue,
            libc::EACCES | libc::EPERM if !fixed => {
                fixed = true;
                let Some(st) = fstat(dir.0) else { return Err(libc::EACCES) };
                let want = (st.st_mode as u32 & 0o7777) | libc::S_IRWXU as u32;
                if want == st.st_mode as u32 & 0o7777 {
                    return Err(errno_or(libc::EPERM));
                }
                unsafe { libc::fchmod(dir.0, want as libc::mode_t) };
            }
            e => return Err(e),
        }
    }
}

fn errno_or(e: i32) -> i32 {
    match errno() {
        0 => e,
        x => x,
    }
}

/// The names in an open folder, and whether each is a folder (never through a symlink).
fn list(dir: &Dir) -> Result<Vec<(CString, bool)>, i32> {
    let fd = unsafe { libc::dup(dir.0) };
    if fd < 0 {
        return Err(errno());
    }
    let d = unsafe { libc::fdopendir(fd) };
    if d.is_null() {
        let e = errno();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    unsafe { libc::rewinddir(d) };
    let mut out = Vec::new();
    loop {
        let ent = unsafe { libc::readdir(d) };
        if ent.is_null() {
            break;
        }
        let (name, kind) = unsafe { (CStr::from_ptr((*ent).d_name.as_ptr()), (*ent).d_type) };
        let b = name.to_bytes();
        if b == b"." || b == b".." {
            continue;
        }
        let is_dir = match kind {
            libc::DT_DIR => true,
            libc::DT_UNKNOWN => lstat_at(dir.0, name).is_some_and(|st| st.st_mode & libc::S_IFMT == libc::S_IFDIR),
            _ => false,
        };
        out.push((name.to_owned(), is_dir));
    }
    unsafe { libc::closedir(d) };
    Ok(out)
}

/// Files of one folder are removed in chunks across threads, which helps large flat folders.
const FILE_CHUNK: usize = 256;

/// Empties an open folder: files first, then each subfolder in parallel.
fn clear(dir: &Dir, dev: libc::dev_t, t: &Tally, measure: bool) {
    let entries = match list(dir) {
        Ok(v) => v,
        Err(e) => return t.fail(c".", format!("could not list a folder: {}", strerror(e))),
    };
    let (dirs, files): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(_, d)| *d);
    files.par_chunks(FILE_CHUNK).for_each(|chunk| {
        for (name, _) in chunk {
            let size = if measure { lstat_at(dir.0, name).map_or(0, |st| st.st_blocks as u64 * 512) } else { 0 };
            match unlink_at(dir, name, 0) {
                Ok(()) => {
                    t.files.fetch_add(1, Ordering::Relaxed);
                    t.bytes.fetch_add(size, Ordering::Relaxed);
                }
                // The entry turned into a folder after the listing; erase it as one.
                Err(libc::EPERM) | Err(libc::EISDIR) if lstat_at(dir.0, name).is_some_and(|st| st.st_mode & libc::S_IFMT == libc::S_IFDIR) => {
                    remove_entry(dir, name, dev, t, measure)
                }
                Err(e) => t.fail(name, strerror(e)),
            }
        }
    });
    dirs.par_iter().for_each(|(name, _)| remove_entry(dir, name, dev, t, measure));
}

/// Removes `name` from an open folder: a folder is emptied and removed, anything else
/// (files, symlinks) is unlinked as the entry itself.
fn remove_entry(parent: &Dir, name: &CStr, dev: libc::dev_t, t: &Tally, measure: bool) {
    match open_dir_at(parent.0, name) {
        Ok(child) => {
            // Never cross into another volume mounted inside the tree.
            match fstat(child.0) {
                Some(st) if st.st_dev == dev => {}
                Some(_) => return t.fail(name, "another volume is mounted here; left it".into()),
                None => return t.fail(name, strerror(errno())),
            }
            clear(&child, dev, t, measure);
            drop(child);
            match unlink_at(parent, name, libc::AT_REMOVEDIR) {
                Ok(()) => {
                    t.dirs.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => t.fail(name, strerror(e)),
            }
        }
        // A file or a symlink (O_NOFOLLOW): remove the entry, never what a link points to.
        Err(libc::ENOTDIR) | Err(libc::ELOOP) => {
            let size = if measure { lstat_at(parent.0, name).map_or(0, |st| st.st_blocks as u64 * 512) } else { 0 };
            match unlink_at(parent, name, 0) {
                Ok(()) => {
                    t.files.fetch_add(1, Ordering::Relaxed);
                    t.bytes.fetch_add(size, Ordering::Relaxed);
                }
                Err(e) => t.fail(name, strerror(e)),
            }
        }
        Err(libc::ENOENT) => {}
        Err(e) => t.fail(name, strerror(e)),
    }
}

/// APFS serializes much of unlinking: on a 10-core Mac, 300k files erase in 8s with 4 threads,
/// 18s with `rm -rf`, and slower again past 6 threads as they fight over the volume's locks.
const ERASE_THREADS: usize = 4;

/// Threads with room for deep trees.
fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, ERASE_THREADS);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .stack_size(64 << 20)
            .thread_name(|i| format!("poo-erase-{}", i))
            .build()
            .expect("erase thread pool")
    })
}

/// Each folder being erased holds a descriptor; macOS starts processes at 256.
fn raise_fd_limit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut rl: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) == 0 {
            let want = rl.rlim_max.min(8192);
            if rl.rlim_cur < want {
                rl.rlim_cur = want;
                libc::setrlimit(libc::RLIMIT_NOFILE, &rl);
            }
        }
    });
}

/// Erases `path` (a folder tree or a single entry) in parallel. The last component is never
/// followed: a symlink there is removed itself. Folders above it are trusted, so callers pass
/// paths they have checked (the clean floor) or own (the graveyard). Errors do not stop the
/// rest; the first one is reported with a count.
pub fn erase(path: &Path, measure: bool) -> Result<Erased, String> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("cannot erase {}", path.display()));
    };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let cparent = CString::new(parent.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let cname = CString::new(name.as_bytes()).map_err(|e| e.to_string())?;
    raise_fd_limit();
    let pfd = unsafe { libc::open(cparent.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if pfd < 0 {
        let e = errno();
        return if e == libc::ENOENT { Ok(Erased::default()) } else { Err(format!("{}: {}", parent.display(), strerror(e))) };
    }
    let pdir = Dir(pfd);
    let dev = fstat(pdir.0).map(|st| st.st_dev).ok_or_else(|| strerror(errno()))?;
    let t = Tally::default();
    pool().install(|| remove_entry(&pdir, &cname, dev, &t, measure));
    let done = Erased {
        files: t.files.load(Ordering::Relaxed),
        dirs: t.dirs.load(Ordering::Relaxed),
        bytes: t.bytes.load(Ordering::Relaxed),
    };
    match t.errors.load(Ordering::Relaxed) {
        0 => Ok(done),
        n => {
            let first = t.first_error.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
            Err(if n == 1 { format!("could not erase {}", first) } else { format!("could not erase {} entries (first: {})", n, first) })
        }
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh graveyard name; it carries the item's size so the reaper can report it freed
/// without measuring the tree again.
fn grave_name(p: &Path, bytes: Option<u64>) -> String {
    let base = format!(
        "{}-{}-{}-{}",
        crate::util::now(),
        crate::util::short_hash(&p.to_string_lossy()),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    match bytes {
        Some(b) => format!("{}-b{}", base, b),
        None => base,
    }
}

/// The size a graveyard entry was buried with.
fn grave_bytes(name: &str) -> Option<u64> {
    name.rsplit_once("-b").and_then(|(_, b)| b.parse().ok())
}

/// Renames without ever replacing what is already at `to`.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let f = CString::new(from.as_os_str().as_bytes())?;
        let t = CString::new(to.as_os_str().as_bytes())?;
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(not(target_os = "macos"))]
    {
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        std::fs::rename(from, to)
    }
}

/// Moves `p` into the graveyard in one rename. Fails without touching `p` when that cannot
/// be done, for example when `p` is on another volume.
pub fn bury(p: &Path, bytes: Option<u64>) -> io::Result<PathBuf> {
    let g = graveyard();
    std::fs::create_dir_all(&g)?;
    let to = g.join(grave_name(p, bytes));
    rename_new(p, &to)?;
    Ok(to)
}

/// A folder in the graveyard the reaper leaves alone (its name starts with a dot) until it is
/// committed, for moves that may have to be undone.
pub struct Staging {
    dir: PathBuf,
    moved: Vec<(PathBuf, PathBuf)>,
    reapable: bool,
}

impl Staging {
    /// A staging folder the reaper leaves alone until `commit`.
    pub fn new(label: &Path) -> io::Result<Self> {
        let g = graveyard();
        std::fs::create_dir_all(&g)?;
        let dir = g.join(format!(".staging-{}", grave_name(label, None)));
        std::fs::create_dir(&dir)?;
        Ok(Self { dir, moved: vec![], reapable: false })
    }

    /// A graveyard entry that is filled in place: a running reaper may start on it at once.
    pub fn grave(label: &Path, bytes: Option<u64>) -> io::Result<Self> {
        let g = graveyard();
        std::fs::create_dir_all(&g)?;
        let dir = g.join(grave_name(label, bytes));
        std::fs::create_dir(&dir)?;
        Ok(Self { dir, moved: vec![], reapable: true })
    }

    /// Moves `p` in, or fails without touching it.
    pub fn take(&mut self, p: &Path) -> io::Result<()> {
        let to = self.dir.join(self.moved.len().to_string());
        rename_new(p, &to)?;
        self.moved.push((p.to_path_buf(), to));
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.moved.is_empty()
    }

    /// Hands everything to the reaper.
    pub fn commit(self, bytes: Option<u64>) -> io::Result<PathBuf> {
        if self.reapable {
            return Ok(self.dir);
        }
        let to = graveyard().join(grave_name(&self.dir, bytes));
        rename_new(&self.dir, &to)?;
        Ok(to)
    }

    /// Puts everything back. What cannot go back stays in the staging folder, which the
    /// reaper never erases; the error says where.
    pub fn undo(self) -> Result<(), String> {
        let mut stuck = 0;
        for (from, to) in self.moved.iter().rev() {
            if rename_new(to, from).is_err() {
                stuck += 1;
            }
        }
        if stuck == 0 {
            let _ = std::fs::remove_dir(&self.dir);
            Ok(())
        } else {
            Err(format!("{} could not be put back and wait in {}", crate::util::plural(stuck, "folder"), self.dir.display()))
        }
    }
}

/// What a reaper pass erased.
#[derive(Debug, Default, Clone, Copy)]
pub struct Reaped {
    pub freed: u64,
    pub entries: usize,
    pub failed: usize,
}

/// Graveyard entries waiting to be erased (staging folders and the lock are not).
pub fn pending() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(graveyard()) else { return vec![] };
    rd.flatten().filter(|e| !e.file_name().as_bytes().starts_with(b".")).map(|e| e.path()).collect()
}

/// Holds the reaper lock until dropped. One reaper erases at a time; another waits its turn.
struct ReaperLock(libc::c_int);

impl Drop for ReaperLock {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn reaper_lock() -> Option<ReaperLock> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let p = CString::new(dir.join("graveyard.lock").as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::open(p.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW, 0o600) };
    if fd < 0 {
        return None;
    }
    loop {
        if unsafe { libc::flock(fd, libc::LOCK_EX) } == 0 {
            return Some(ReaperLock(fd));
        }
        if errno() != libc::EINTR {
            unsafe { libc::close(fd) };
            return None;
        }
    }
}

/// Erases everything in the graveyard, entries in parallel, until it stays empty. Entries that
/// fail are left for a later run.
pub fn reap() -> Reaped {
    let mut out = Reaped::default();
    let Some(_lock) = reaper_lock() else { return out };
    let mut failed: std::collections::HashSet<PathBuf> = Default::default();
    loop {
        let todo: Vec<PathBuf> = pending().into_iter().filter(|p| !failed.contains(p)).collect();
        if todo.is_empty() {
            break;
        }
        let done: Vec<(PathBuf, Result<u64, String>)> = pool().install(|| {
            todo.par_iter()
                .map(|p| {
                    let known = p.file_name().and_then(|n| grave_bytes(&n.to_string_lossy()));
                    (p.clone(), erase(p, known.is_none()).map(|e| known.unwrap_or(e.bytes)))
                })
                .collect()
        });
        for (p, r) in done {
            match r {
                Ok(b) => {
                    out.freed += b;
                    out.entries += 1;
                }
                Err(_) => {
                    out.failed += 1;
                    failed.insert(p);
                }
            }
        }
    }
    out
}

/// Starts a detached `poo gc` when the graveyard has work. It runs in its own session, so
/// the clean returning, ^C in the terminal or the app quitting does not stop it. Only a
/// binary named `poo` spawns one (never a test harness).
pub fn spawn_reaper() -> bool {
    use std::os::unix::process::CommandExt;
    if pending().is_empty() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    if exe.file_stem().is_none_or(|n| !n.as_encoded_bytes().starts_with(b"poo")) {
        return false;
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["gc", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grave_names_carry_sizes() {
        let n = grave_name(Path::new("/a/b"), Some(1234));
        assert_eq!(grave_bytes(&n), Some(1234));
        assert_eq!(grave_bytes(&grave_name(Path::new("/a/b-b9"), None)), None);
    }
}
