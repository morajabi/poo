use poo::bulk::read_entries_bulk;
use poo::fsutil::{read_entries_std, Entry};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn tmpdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("poo-bulk-{}-{}", std::process::id(), line!()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn sorted(mut v: Vec<Entry>) -> Vec<Entry> {
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn assert_same(dir: &Path) -> usize {
    let a = sorted(read_entries_std(dir));
    let Some(b) = read_entries_bulk(dir) else {
        assert!(!cfg!(target_os = "macos"), "bulk reader failed on {}", dir.display());
        return 0;
    };
    let b = sorted(b);
    assert_eq!(a.len(), b.len(), "entry count in {}", dir.display());
    for (x, y) in a.iter().zip(&b) {
        let ctx = format!("{:?}\n vs {:?}", x, y);
        assert_eq!(x.name, y.name, "{ctx}");
        assert_eq!(x.path, y.path, "{ctx}");
        assert_eq!(x.is_dir, y.is_dir, "{ctx}");
        assert_eq!(x.is_symlink, y.is_symlink, "{ctx}");
        assert_eq!(x.alloc, y.alloc, "{ctx}");
        assert_eq!(x.mtime, y.mtime, "{ctx}");
        assert_eq!(x.dev, y.dev, "{ctx}");
        assert_eq!(x.ino, y.ino, "{ctx}");
        // Directory link counts are not vended by getattrlistbulk on APFS; bulk reports 1.
        if !x.is_dir {
            assert_eq!(x.nlink, y.nlink, "{ctx}");
        }
    }
    let mut n = a.len();
    for e in a.iter().filter(|e| e.is_dir && !e.is_symlink) {
        n += assert_same(&e.path);
    }
    n
}

#[test]
fn bulk_matches_std() {
    let root = tmpdir();
    for (name, size) in [("empty", 0usize), ("one", 1), ("4k", 4096), ("4k1", 4097), ("big", 1 << 20)] {
        fs::File::create(root.join(name)).unwrap().write_all(&vec![7u8; size]).unwrap();
    }
    // Sparse file: allocated size well below logical size.
    let f = fs::File::create(root.join("sparse")).unwrap();
    f.set_len(64 << 20).unwrap();
    fs::write(root.join("with space.txt"), b"hello").unwrap();
    fs::write(root.join("ünïcødé-文件-🦀"), b"unicode").unwrap();
    fs::hard_link(root.join("big"), root.join("big-hardlink")).unwrap();
    std::os::unix::fs::symlink("big", root.join("link")).unwrap();
    std::os::unix::fs::symlink("nowhere", root.join("dangling")).unwrap();
    std::os::unix::fs::symlink("a", root.join("dirlink")).unwrap();
    let deep = root.join("a/b/c d/ë");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("leaf"), vec![1u8; 10_000]).unwrap();
    fs::create_dir(root.join("emptydir")).unwrap();
    for i in 0..3000 {
        // Enough entries to need several getattrlistbulk calls.
        fs::write(root.join("a").join(format!("f{i:05}-{}", "x".repeat(i % 200))), b"x").unwrap();
    }

    let n = assert_same(&root);
    fs::remove_dir_all(&root).unwrap();
    if cfg!(target_os = "macos") {
        assert!(n > 3000, "compared {n} entries");
    }
}

#[test]
fn bulk_missing_dir_is_none() {
    assert!(read_entries_bulk(Path::new("/definitely/not/here")).is_none());
}

/// The lite read (no APFS private sizes, for walking structure) sees the same entries.
#[test]
fn lite_read_matches_full_read() {
    let d = std::env::temp_dir().join(format!("poo-bulk-lite-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("sub/deeper")).unwrap();
    fs::write(d.join("a.txt"), vec![7u8; 10_000]).unwrap();
    fs::write(d.join("sub/b.bin"), vec![1u8; 100_000]).unwrap();
    let _ = std::os::unix::fs::symlink("a.txt", d.join("link"));
    for dir in [d.clone(), d.join("sub")] {
        let full = sorted(poo::fsutil::read_entries(&dir));
        let lite = sorted(poo::fsutil::read_entries_lite(&dir));
        assert_eq!(full.len(), lite.len());
        for (x, y) in full.iter().zip(&lite) {
            assert_eq!((&x.name, &x.path, x.is_dir, x.is_symlink, x.alloc, x.mtime, x.dev, x.ino, x.nlink), (&y.name, &y.path, y.is_dir, y.is_symlink, y.alloc, y.mtime, y.dev, y.ino, y.nlink));
            assert_eq!(y.private, y.alloc, "lite reads report private = alloc");
        }
    }
    let _ = fs::remove_dir_all(&d);
}
