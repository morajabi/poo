//! Compares `read_entries_std` and `read_entries_bulk` on a real tree (read-only).
//!
//! cargo run --release --example bench_read -- [DIR ...]

use poo::fsutil::{read_entries_std, Entry};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default, Clone, Copy)]
struct Tot {
    entries: u64,
    bytes: u64,
}

fn walk(dir: &Path, dev: u64, read: &(dyn Fn(&Path) -> Vec<Entry> + Sync), seen: &Mutex<HashSet<(u64, u64)>>, par: bool) -> Tot {
    let es = read(dir);
    let mut t = Tot { entries: es.len() as u64, bytes: 0 };
    let mut dirs = vec![];
    for e in &es {
        if e.is_dir && !e.is_symlink {
            t.bytes += e.alloc;
            if e.dev == dev {
                dirs.push(e.path.clone());
            }
        } else if e.nlink > 1 && !seen.lock().unwrap().insert((e.dev, e.ino)) {
        } else {
            t.bytes += e.alloc;
        }
    }
    let sub = |d: &PathBuf| walk(d, dev, read, seen, par);
    let subs: Vec<Tot> = if par { dirs.par_iter().map(sub).collect() } else { dirs.iter().map(sub).collect() };
    for s in subs {
        t.entries += s.entries;
        t.bytes += s.bytes;
    }
    t
}

fn run(name: &str, root: &Path, read: &(dyn Fn(&Path) -> Vec<Entry> + Sync), par: bool) -> (Tot, f64) {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::symlink_metadata(root).unwrap().dev();
    let seen = Mutex::new(HashSet::new());
    let t0 = Instant::now();
    let t = walk(root, dev, read, &seen, par);
    let s = t0.elapsed().as_secs_f64();
    println!(
        "  {name:<5} {:<6} {:>9} entries  {:>8.2}s  {:>10.0} entries/s  {:>16} bytes ({:.2} GiB)",
        if par { "rayon" } else { "serial" },
        t.entries,
        s,
        t.entries as f64 / s,
        t.bytes,
        t.bytes as f64 / (1u64 << 30) as f64
    );
    (t, s)
}

fn main() {
    let home = std::env::var("HOME").unwrap();
    let mut roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        roots.push(PathBuf::from(home).join("dev/inline-chat/inline"));
    }
    let bulk = |p: &Path| poo::bulk::read_entries_bulk(p).expect("bulk reader failed");
    for root in &roots {
        println!("{}", root.display());
        // Warm the cache once so both readers see the same state.
        run("warm", root, &read_entries_std, true);
        for par in [false, true] {
            let (a, sa) = run("std", root, &read_entries_std, par);
            let (b, sb) = run("bulk", root, &bulk, par);
            let diff = (a.bytes as f64 - b.bytes as f64).abs() / (a.bytes.max(1) as f64);
            println!("  -> speedup {:.2}x, totals differ by {:.4}%", sa / sb, diff * 100.0);
            assert!(diff < 0.001, "totals disagree by more than 0.1%");
        }
    }
}
