//! Fast directory reading with `getattrlistbulk(2)` on macOS.
//!
//! One syscall returns the names and attributes of many entries, instead of a
//! `readdir` plus an `lstat` per entry. Fields match `fsutil::read_entries`:
//!
//! - `alloc`: files (and symlinks, sockets, ...) use `ATTR_FILE_ALLOCSIZE`, which
//!   equals `st_blocks * 512`. Directories use `ATTR_DIR_ALLOCSIZE` when the
//!   volume returns it; on APFS both it and `st_blocks` are 0 for directories,
//!   so a missing value is treated as 0.
//! - `nlink`: `ATTR_FILE_LINKCOUNT` for files. Directories always get 1: APFS
//!   does not vend `ATTR_DIR_LINKCOUNT` through the bulk call, and callers only
//!   use `nlink` for hardlink dedupe of non-directories.
//! - `ino`: `ATTR_CMN_FILEID`, which is shared by all hard links like `st_ino`.
//!
//! Entries whose attributes are incomplete or carry `ATTR_CMN_ERROR`, and
//! mount points (whose bulk attributes describe the covered directory, not
//! the mounted root that `lstat` reports), fall back to `lstat` one by one.
//! Any directory-level error returns `None` so the caller can use `std::fs`.

use crate::fsutil::Entry;
use std::path::Path;

/// `EF_MAY_SHARE_BLOCKS` from <sys/stat.h>: the file may share blocks with another (an APFS
/// clone). Without it, a file's private size is its allocated size.
pub const EF_MAY_SHARE_BLOCKS: u64 = 0x1;

/// What to ask for besides names, types, sizes, times and ids.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    /// `ATTR_CMNEXT_PRIVATESIZE`: exact, but APFS looks up every file's extents for it.
    pub private: bool,
    /// `ATTR_CMNEXT_EXT_FLAGS`: cheap; says which files may be clones.
    pub flags: bool,
}

#[cfg(not(target_os = "macos"))]
pub fn read_entries_bulk(_dir: &Path) -> Option<Vec<Entry>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn read_entries_bulk_with(_dir: &Path, _want: Want) -> Option<Vec<(Entry, u64)>> {
    None
}

#[cfg(target_os = "macos")]
pub fn read_entries_bulk(dir: &Path) -> Option<Vec<Entry>> {
    Some(imp::read(dir, Want { private: true, flags: false })?.into_iter().map(|(e, _)| e).collect())
}

/// Entries with their extended flags (0 unless `want.flags`). Without `want.private`,
/// `private` is the allocated size.
#[cfg(target_os = "macos")]
pub fn read_entries_bulk_with(dir: &Path, want: Want) -> Option<Vec<(Entry, u64)>> {
    imp::read(dir, want)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Entry, Want};
    use libc::{attrlist, attrreference_t, attribute_set_t, c_void};
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    /// `ATTR_CMN_ERROR` from <sys/attr.h>; not exported by libc 0.2.
    const ATTR_CMN_ERROR: u32 = 0x2000_0000;
    /// `DIR_MNTSTATUS_MNTPOINT` from <sys/attr.h>; not exported by libc 0.2.
    const DIR_MNTSTATUS_MNTPOINT: u32 = 0x0000_0001;
    /// `ATTR_CMNEXT_PRIVATESIZE` (bytes freed if this file alone were deleted: shared
    /// clone blocks excluded) and `FSOPT_ATTR_CMN_EXTENDED` from <sys/attr.h>.
    const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
    const FSOPT_ATTR_CMN_EXTENDED: u64 = 0x0000_0020;
    const ATTR_CMNEXT_EXT_FLAGS: u32 = 0x0000_0200;
    /// `enum vtype` values from <sys/vnode.h>.
    const VDIR: u32 = 2;
    const VLNK: u32 = 5;

    const CMN: u32 = libc::ATTR_CMN_RETURNED_ATTRS
        | libc::ATTR_CMN_NAME
        | libc::ATTR_CMN_DEVID
        | libc::ATTR_CMN_OBJTYPE
        | libc::ATTR_CMN_MODTIME
        | libc::ATTR_CMN_FILEID
        | ATTR_CMN_ERROR;
    const DIR: u32 = libc::ATTR_DIR_MOUNTSTATUS | libc::ATTR_DIR_ALLOCSIZE;
    const FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE;

    /// 256 KB, 8-byte aligned (each entry group starts 8-byte aligned).
    const BUF_WORDS: usize = 32 * 1024;

    struct Fd(i32);
    impl Drop for Fd {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }

    /// Bounds-checked cursor over one entry's attribute group.
    struct Cur<'a> {
        b: &'a [u8],
        off: usize,
    }
    impl Cur<'_> {
        fn take<T: Copy>(&mut self) -> Option<T> {
            let n = std::mem::size_of::<T>();
            let s = self.b.get(self.off..self.off + n)?;
            self.off += n;
            Some(unsafe { std::ptr::read_unaligned(s.as_ptr() as *const T) })
        }
    }

    pub fn read(dir: &Path, want: Want) -> Option<Vec<(Entry, u64)>> {
        let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if raw < 0 {
            return None;
        }
        let fd = Fd(raw);
        let mut al = attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: CMN,
            volattr: 0,
            dirattr: DIR,
            fileattr: FILE,
            forkattr: if want.private { ATTR_CMNEXT_PRIVATESIZE } else { 0 } | if want.flags { ATTR_CMNEXT_EXT_FLAGS } else { 0 },
        };
        // One buffer per thread: a fresh zeroed 256 KB per folder cost more than the read.
        thread_local! {
            static BUF: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(vec![0u64; BUF_WORDS]);
        }
        BUF.with(|b| match b.try_borrow_mut() {
            Ok(mut buf) => read_with(dir, fd.0, &mut al, &mut buf),
            Err(_) => read_with(dir, fd.0, &mut al, &mut vec![0u64; BUF_WORDS]),
        })
    }

    fn read_with(dir: &Path, fd: i32, al: &mut attrlist, buf: &mut [u64]) -> Option<Vec<(Entry, u64)>> {
        let size = buf.len() * 8;
        let mut out = Vec::new();
        loop {
            let n = unsafe {
                libc::getattrlistbulk(
                    fd,
                    al as *mut attrlist as *mut c_void,
                    buf.as_mut_ptr() as *mut c_void,
                    size,
                    FSOPT_ATTR_CMN_EXTENDED,
                )
            };
            if n < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return None;
            }
            if n == 0 {
                break;
            }
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, size) };
            out.reserve(n as usize);
            let mut pos = 0usize;
            for _ in 0..n {
                let len = u32::from_ne_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
                if len < 4 || pos + len > size {
                    return None;
                }
                let group = &bytes[pos..pos + len];
                pos += len;
                if let Some(e) = parse(dir, group)? {
                    out.push(e);
                }
            }
        }
        Some(out)
    }

    /// `None` = malformed buffer (abort), `Some(None)` = entry vanished (skip).
    fn parse(dir: &Path, g: &[u8]) -> Option<Option<(Entry, u64)>> {
        let mut c = Cur { b: g, off: 4 };
        let ret: attribute_set_t = c.take()?;
        let has = |set: u32, bit: u32| set & bit != 0;
        let mut err = 0u32;
        if has(ret.commonattr, ATTR_CMN_ERROR) {
            err = c.take()?;
        }
        // Without a name we cannot even fall back, so treat it as a broken read.
        if !has(ret.commonattr, libc::ATTR_CMN_NAME) {
            return None;
        }
        let name_at = c.off;
        let r: attrreference_t = c.take()?;
        let start = name_at.checked_add_signed(r.attr_dataoffset as isize)?;
        let raw = g.get(start..start + r.attr_length as usize)?;
        let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
        let os = OsStr::from_bytes(raw);
        let path = dir.join(os);
        let name = os.to_string_lossy().into_owned();

        let dev: Option<i32> = if has(ret.commonattr, libc::ATTR_CMN_DEVID) { Some(c.take()?) } else { None };
        let objtype: Option<u32> = if has(ret.commonattr, libc::ATTR_CMN_OBJTYPE) { Some(c.take()?) } else { None };
        let mtime: Option<libc::timespec> =
            if has(ret.commonattr, libc::ATTR_CMN_MODTIME) { Some(c.take()?) } else { None };
        let ino: Option<u64> = if has(ret.commonattr, libc::ATTR_CMN_FILEID) { Some(c.take()?) } else { None };

        let (Some(dev), Some(objtype), Some(mtime), Some(ino)) = (dev, objtype, mtime, ino) else {
            return Some(lstat_entry(name, path).map(|e| (e, 0)));
        };
        if err != 0 {
            return Some(lstat_entry(name, path).map(|e| (e, 0)));
        }

        let is_dir = objtype == VDIR;
        let (alloc, nlink, is_file) = if is_dir {
            let mnt: u32 = if has(ret.dirattr, libc::ATTR_DIR_MOUNTSTATUS) { c.take()? } else { 0 };
            if mnt & DIR_MNTSTATUS_MNTPOINT != 0 {
                return Some(lstat_entry(name, path).map(|e| (e, 0)));
            }
            let alloc: i64 = if has(ret.dirattr, libc::ATTR_DIR_ALLOCSIZE) { c.take()? } else { 0 };
            (alloc.max(0) as u64, 1, None)
        } else {
            if !has(ret.fileattr, libc::ATTR_FILE_LINKCOUNT) || !has(ret.fileattr, libc::ATTR_FILE_ALLOCSIZE) {
                return Some(lstat_entry(name, path).map(|e| (e, 0)));
            }
            let nlink: u32 = c.take()?;
            let alloc: i64 = c.take()?;
            (alloc.max(0) as u64, nlink as u64, Some(()))
        };
        // Extended common attributes come last.
        let private: Option<i64> = if has(ret.forkattr, ATTR_CMNEXT_PRIVATESIZE) { Some(c.take()?) } else { None };
        let flags: u64 = if has(ret.forkattr, ATTR_CMNEXT_EXT_FLAGS) { c.take()? } else { 0 };
        let private = match (is_file, private) {
            (Some(()), Some(p)) => (p.max(0) as u64).min(alloc),
            _ => alloc,
        };
        Some(Some((Entry {
            name,
            path,
            is_dir,
            is_symlink: objtype == VLNK,
            alloc,
            mtime: mtime.tv_sec,
            // Same sign extension as std's `MetadataExt::dev` (`st_dev as u64`).
            dev: dev as u64,
            ino,
            nlink,
            private,
        }, flags)))
    }

    fn lstat_entry(name: String, path: std::path::PathBuf) -> Option<Entry> {
        let md = std::fs::symlink_metadata(&path).ok()?;
        let ft = md.file_type();
        Some(Entry {
            name,
            path,
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
            alloc: md.blocks() * 512,
            mtime: md.mtime(),
            dev: md.dev(),
            ino: md.ino(),
            nlink: md.nlink(),
            private: md.blocks() * 512,
        })
    }
}
