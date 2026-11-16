//! Times parallel walks of real trees with different bulk attribute sets, and checks that a
//! file without `EF_MAY_SHARE_BLOCKS` always has private size == allocated size (read-only).
//!
//! cargo run --release --example walk_time -- DIR...
use poo::bulk::{read_entries_bulk_with, Want, EF_MAY_SHARE_BLOCKS};
use poo::fsutil::Entry;
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Default)]
struct C {
    entries: AtomicU64,
    files: AtomicU64,
    flagged: AtomicU64,
    flagged_dirs: AtomicU64,
    dirs: AtomicU64,
    bad: AtomicU64,
    shared_bytes: AtomicU64,
}

#[derive(Clone, Copy)]
enum Mode {
    Fixed(Want),
    /// Flags first; folders holding possible clones are read again with private sizes.
    Hybrid,
}

fn read(d: &Path, m: Mode) -> Option<Vec<(Entry, u64)>> {
    match m {
        Mode::Fixed(w) => read_entries_bulk_with(d, w),
        Mode::Hybrid => {
            let es = read_entries_bulk_with(d, Want { private: false, flags: true })?;
            if es.iter().any(|(e, f)| !e.is_dir && f & EF_MAY_SHARE_BLOCKS != 0) {
                return read_entries_bulk_with(d, Want { private: true, flags: true });
            }
            Some(es)
        }
    }
}

fn walk(d: &Path, m: Mode, check: bool, c: &C) {
    let Some(es) = read(d, m) else { return };
    c.entries.fetch_add(es.len() as u64, Relaxed);
    c.dirs.fetch_add(1, Relaxed);
    let mut any = false;
    for (e, f) in &es {
        if e.is_dir {
            continue;
        }
        c.files.fetch_add(1, Relaxed);
        if f & EF_MAY_SHARE_BLOCKS != 0 {
            c.flagged.fetch_add(1, Relaxed);
            any = true;
        } else if check && e.private != e.alloc {
            c.bad.fetch_add(1, Relaxed);
            eprintln!("unflagged but private {} != alloc {}: {}", e.private, e.alloc, e.path.display());
        }
        c.shared_bytes.fetch_add(e.alloc - e.private, Relaxed);
    }
    if any {
        c.flagged_dirs.fetch_add(1, Relaxed);
    }
    es.par_iter().filter(|(e, _)| e.is_dir && !e.is_symlink).for_each(|(e, _)| walk(&e.path, m, check, c));
}

fn main() {
    for a in std::env::args().skip(1) {
        for (label, m, check) in [
            ("private+flags", Mode::Fixed(Want { private: true, flags: true }), true),
            ("private", Mode::Fixed(Want { private: true, flags: false }), false),
            ("hybrid", Mode::Hybrid, false),
            ("flags", Mode::Fixed(Want { private: false, flags: true }), false),
            ("neither", Mode::Fixed(Want { private: false, flags: false }), false),
        ] {
            let c = C::default();
            let t = std::time::Instant::now();
            walk(Path::new(&a), m, check, &c);
            println!(
                "{:<14} {:>9} entries {:>8.3}s  files {} flagged {} in {}/{} dirs, unflagged mismatches {}, shared {} MB  {}",
                label,
                c.entries.load(Relaxed),
                t.elapsed().as_secs_f64(),
                c.files.load(Relaxed),
                c.flagged.load(Relaxed),
                c.flagged_dirs.load(Relaxed),
                c.dirs.load(Relaxed),
                c.bad.load(Relaxed),
                c.shared_bytes.load(Relaxed) / 1_000_000,
                a
            );
        }
    }
}
