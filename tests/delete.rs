//! Fast deletion: parallel erase without following symlinks, burial in the graveyard with a
//! background reaper (resumed after an interruption), the in-place fallback when the graveyard
//! cannot be used, and worktree removal that moves ignored folders aside (and back when git
//! refuses). Everything runs in a throwaway HOME and POO_HOME. One test: env vars are
//! process-wide.
#![cfg(unix)]

use poo::catalog;
use poo::clean::{self, CleanOptions};
use poo::config::Config;
use poo::delete;
use poo::model::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn item(path: &Path, rule: &str, kind: ActionKind, bytes: u64) -> Item {
    serde_json::from_value(serde_json::json!({
        "id": poo::util::short_hash(&path.to_string_lossy()), "path": path, "name": path.file_name().unwrap().to_string_lossy(),
        "rule": rule, "bucket": "caches", "group": "G", "bytes": bytes, "total_bytes": bytes, "files": 1, "tier": "cold",
        "cost": "rebuild", "reasons": [], "action": {"kind": kind},
    }))
    .unwrap()
}

/// A tree shaped like node_modules: nested packages, a read-only folder, a folder with no
/// permissions at all, and symlinks pointing out of the tree.
fn tree(root: &Path, outside: &Path, packages: usize) -> u64 {
    let mut files = 0;
    for p in 0..packages {
        let d = root.join(format!("pkg{}/lib/deep/er", p));
        std::fs::create_dir_all(&d).unwrap();
        for f in 0..20 {
            std::fs::write(d.join(format!("f{}.js", f)), vec![b'x'; 300]).unwrap();
            files += 1;
        }
        std::fs::write(root.join(format!("pkg{}/package.json", p)), "{}").unwrap();
        files += 1;
    }
    let ro = root.join("pkg0/readonly");
    std::fs::create_dir_all(ro.join("inner")).unwrap();
    std::fs::write(ro.join("inner/a"), "a").unwrap();
    std::fs::set_permissions(ro.join("inner"), std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let locked = root.join("pkg1/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("b"), "b").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::os::unix::fs::symlink(outside, root.join("pkg2/link-to-dir")).unwrap();
    std::os::unix::fs::symlink(outside.join("keep.txt"), root.join("pkg2/link-to-file")).unwrap();
    files + 4
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "protocol.file.allow=always", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

fn assert_outside_intact(outside: &Path) {
    assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "keep", "a symlink was followed");
    assert!(outside.join("sub/also.txt").exists(), "a symlink was followed");
}

#[test]
fn fast_delete_in_a_sandbox() {
    let base = std::env::temp_dir().join(format!("poo-delete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let home = base.join("home");
    let outside = base.join("outside");
    for d in [&home, &outside.join("sub"), &base.join("poo/config/mods"), &base.join("root")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(outside.join("keep.txt"), "keep").unwrap();
    std::fs::write(outside.join("sub/also.txt"), "also").unwrap();
    std::fs::write(base.join("poo/config/config.toml"), "[scan]\nmin_item_bytes = 1\nwarm_days = 1\nidle_days = 1\n").unwrap();
    std::fs::write(
        base.join("poo/config/mods/fake.toml"),
        r#"
[[rule]]
id = "fake.big"
name = "Fake big cache"
bucket = "caches"
kind = "path"
paths = ["~/tools/big"]
cost = "none"

[[rule]]
id = "fake.flat"
name = "Fake flat cache"
bucket = "caches"
kind = "path"
paths = ["~/tools/flat"]
contents = true
cost = "none"

[[rule]]
id = "fake.other"
name = "Fake cache for the fallback"
bucket = "caches"
kind = "path"
paths = ["~/tools/other"]
cost = "none"
"#,
    )
    .unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("POO_HOME", base.join("poo"));
    std::env::set_var("POO_CLEAN_ONLY_UNDER", &home);
    std::env::remove_var("POO_STATE_DIR");
    std::env::remove_var("POO_READ_ONLY");

    // 1. erase: the whole tree goes, read-only and permission-less folders included, and
    // nothing a symlink points at is touched.
    let t = base.join("scratch/node_modules");
    let files = tree(&t, &outside, 30);
    let e = delete::erase(&t, true).unwrap();
    assert!(!t.exists() && base.join("scratch").exists());
    assert_eq!(e.files, files, "{:?}", e);
    assert!(e.bytes > 0);
    assert_outside_intact(&outside);
    // The last component is never followed either: a symlink there is removed itself.
    std::os::unix::fs::symlink(&outside, base.join("scratch/link")).unwrap();
    delete::erase(&base.join("scratch/link"), false).unwrap();
    assert!(std::fs::symlink_metadata(base.join("scratch/link")).is_err());
    assert_outside_intact(&outside);
    // Already gone is fine.
    assert_eq!(delete::erase(&base.join("scratch/nothing-here"), false), Ok(Default::default()));

    // 2. A background clean buries folders (gone from where they were, counted as erasing)
    // and the reaper erases them.
    let big = home.join("tools/big");
    tree(&big, &outside, 40);
    let flat = home.join("tools/flat");
    std::fs::create_dir_all(flat.join("sub")).unwrap();
    for i in 0..300 {
        std::fs::write(flat.join(format!("entry{}", i)), "x").unwrap();
    }
    std::fs::write(flat.join("sub/inner"), "x").unwrap();
    std::fs::create_dir_all(home.join("tools/other/x")).unwrap();
    std::fs::write(home.join("tools/other/x/f"), vec![0u8; 5000]).unwrap();

    let cfg = Config::load().unwrap();
    let cat = catalog::load(&cfg);
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    let mut flat_item = item(&flat, "fake.flat", ActionKind::Delete, 301 * 4096);
    flat_item.meta.insert("contents".into(), true.into());
    let bg = CleanOptions { background_delete: true, mode: RiskLevel::Force, ..Default::default() };
    let items = [item(&big, "fake.big", ActionKind::Delete, 3_000_000), flat_item];
    let res = clean::clean(&items, &cat, &bg);
    for r in &res.results {
        assert_eq!(r.status, "cleaned", "{:?}", r);
    }
    assert!(!big.exists(), "buried at once");
    assert!(flat.exists() && std::fs::read_dir(&flat).unwrap().next().is_none(), "contents moved out, folder kept");
    assert!(res.background_delete);
    assert_eq!(res.erasing_bytes, res.freed_bytes);
    assert_eq!(res.erasing_bytes, items.iter().map(|i| i.total_bytes).sum::<u64>());
    assert_eq!(delete::pending().len(), 2, "one graveyard entry per item");
    let freed = clean::empty_graveyard();
    assert_eq!(freed, res.erasing_bytes, "the reaper reports what was buried, without measuring again");
    assert!(delete::pending().is_empty());
    assert_outside_intact(&outside);
    let history = clean::history(10);
    assert!(history.iter().any(|h| h["rule"] == "fake.big") && history.iter().any(|h| h["rule"] == "fake.flat"));

    // 3. An interrupted reaper: half-erased entries are finished by the next one, staging
    // folders (moves that may still be undone) are left alone, and two reapers at once take
    // turns instead of fighting.
    let half = base.join("scratch/half");
    tree(&half, &outside, 10);
    let grave = delete::bury(&half, Some(12345)).unwrap();
    assert!(!half.exists() && grave.starts_with(delete::graveyard()));
    std::fs::remove_dir_all(grave.join("pkg5")).unwrap();
    let measured = base.join("scratch/measured");
    tree(&measured, &outside, 5);
    delete::bury(&measured, None).unwrap();
    let staging = delete::graveyard().join(".staging-test");
    std::fs::create_dir_all(staging.join("kept")).unwrap();
    let a = std::thread::spawn(delete::reap);
    let b = std::thread::spawn(delete::reap);
    let (a, b) = (a.join().unwrap(), b.join().unwrap());
    assert_eq!(a.entries + b.entries, 2, "{:?} {:?}", a, b);
    assert_eq!(a.failed + b.failed, 0);
    assert!(a.freed + b.freed > 12345, "the unsized entry is measured while erasing");
    assert!(delete::pending().is_empty());
    assert!(staging.join("kept").exists(), "the reaper never erases staging folders");
    std::fs::remove_dir_all(&staging).unwrap();
    assert_outside_intact(&outside);

    // 4. When the graveyard cannot be used (another volume, a broken state folder), the item
    // is erased in place, in the foreground.
    std::fs::write(base.join("not-a-folder"), "").unwrap();
    std::env::set_var("POO_STATE_DIR", base.join("not-a-folder/state"));
    std::fs::create_dir_all(flat.join("again")).unwrap();
    std::fs::write(flat.join("again/f"), "x").unwrap();
    let mut flat_again = item(&flat, "fake.flat", ActionKind::Delete, 4096);
    flat_again.meta.insert("contents".into(), true.into());
    let res = clean::clean(&[item(&home.join("tools/other"), "fake.other", ActionKind::Delete, 5000), flat_again], &cat, &bg);
    std::env::remove_var("POO_STATE_DIR");
    let again = res.results.iter().find(|r| r.path.ends_with("tools/flat")).unwrap();
    assert_eq!(again.status, "cleaned", "{:?}", again);
    assert!(flat.exists() && std::fs::read_dir(&flat).unwrap().next().is_none());
    let other = res.results.iter().find(|r| r.path.ends_with("tools/other")).unwrap();
    assert_eq!(other.status, "cleaned", "{:?}", other);
    assert!(!home.join("tools/other").exists());
    assert_eq!(res.erasing_bytes, 0);
    assert!(!res.background_delete);

    // 5. Worktrees: ignored folders move aside so git only deletes checked-in files.
    let dev = home.join("dev");
    let repo = dev.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), "node_modules/\nbuild/\n").unwrap();
    std::fs::write(repo.join("index.js"), "1").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    let wt = dev.join("repo-wt");
    git(&repo, &["worktree", "add", "-q", &wt.to_string_lossy()]);
    tree(&wt.join("node_modules"), &outside, 20);
    std::fs::create_dir_all(wt.join("packages/a/build")).unwrap();
    std::fs::write(wt.join("packages/a/build/out.o"), vec![0u8; 4000]).unwrap();

    // A second worktree git will refuse to remove: it gets locked after the scan.
    let wt2 = dev.join("repo-wt2");
    git(&repo, &["worktree", "add", "-q", &wt2.to_string_lossy()]);
    tree(&wt2.join("node_modules"), &outside, 5);

    let worktree = |p: &Path| {
        let mut i = item(p, "git.worktrees", ActionKind::WorktreeRemove, 500_000);
        i.repo = Some(repo.to_string_lossy().to_string());
        i
    };
    let res = clean::clean(&[worktree(&wt)], &cat, &bg);
    assert_eq!(res.results[0].status, "cleaned", "{:?}", res.results[0]);
    assert!(!wt.exists());
    assert!(res.erasing_bytes > 0, "the ignored folders are erased in the background");
    let list = Command::new("git").args(["worktree", "list"]).current_dir(&repo).output().unwrap();
    assert!(!String::from_utf8_lossy(&list.stdout).contains("repo-wt "), "git forgot the worktree");
    assert!(clean::empty_graveyard() > 0);
    assert!(delete::pending().is_empty());
    assert_outside_intact(&outside);

    git(&repo, &["worktree", "lock", "--reason", "test", &wt2.to_string_lossy()]);
    let res = clean::clean(&[worktree(&wt2)], &cat, &bg);
    assert_eq!(res.results[0].status, "failed", "{:?}", res.results[0]);
    assert!(wt2.join("node_modules/pkg3/package.json").exists(), "ignored folders went back after git refused");
    assert!(wt2.join("index.js").exists());
    assert!(delete::pending().is_empty());
    let staged = std::fs::read_dir(delete::graveyard()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".staging")).count();
    assert_eq!(staged, 0, "the staging folder is cleaned up after an undo");
    assert_outside_intact(&outside);

    // Leave nothing behind: make everything writable again and remove the sandbox.
    let _ = Command::new("chmod").args(["-R", "u+rwx", &base.to_string_lossy()]).status();
    let _ = std::fs::remove_dir_all(&base);
}
