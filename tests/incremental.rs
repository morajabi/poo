//! Incremental scans (file-change journal + index) must agree with full walks.
//! Runs the real binary against a throwaway HOME, so nothing real is scanned or touched.
#![cfg(target_os = "macos")]

use poo::model::RuleKind;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new() -> Env {
        let dir = std::env::temp_dir().join(format!("poo-incremental-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["home", "tmp", "poo/config", "root"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let dir = std::fs::canonicalize(dir).unwrap();
        // Providers shell out to simctl, Docker and Homebrew on the real machine: keep them off.
        let cat = poo::catalog::load(&poo::config::Config::default());
        let off: Vec<String> = cat
            .rules
            .iter()
            .filter(|r| r.kind == RuleKind::Provider && !r.id.starts_with("git."))
            .map(|r| format!("{:?}", r.id))
            .collect();
        std::fs::write(
            dir.join("poo/config/config.toml"),
            format!(
                "roots = [{:?}]\ndisabled_rules = [{}]\n[scan]\nmin_item_bytes = 1000000\n",
                dir.join("root").to_string_lossy(),
                off.join(", ")
            ),
        )
        .unwrap();
        Env { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    fn poo(&self, args: &[&str]) -> (serde_json::Value, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_poo"))
            .args(args)
            .env("HOME", self.dir.join("home"))
            .env("TMPDIR", self.dir.join("tmp"))
            .env("POO_HOME", self.dir.join("poo"))
            .env("POO_TRACE", "1")
            .env("NO_COLOR", "1")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("POO_CONFIG_DIR")
            .env_remove("POO_STATE_DIR")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "poo {:?} failed: {}", args, stderr);
        (serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null), stderr)
    }

    /// Scans (incrementally unless `full`) and returns item path -> MB under the test root.
    fn items(&self, full: bool) -> (BTreeMap<String, u64>, bool) {
        let (_, stderr) = if full { self.poo(&["scan", "--full", "--json"]) } else { self.poo(&["scan", "--json"]) };
        let incremental = stderr.contains("trace");
        let (v, _) = self.poo(&["ls", "-n", "0", "--json"]);
        let root = self.root().to_string_lossy().to_string();
        let mut m = BTreeMap::new();
        for i in v["items"].as_array().unwrap() {
            let p = i["path"].as_str().unwrap();
            if let Some(rel) = p.strip_prefix(&root) {
                let kind = i["action"]["kind"].as_str().unwrap_or("");
                m.insert(format!("{} {}", rel, kind), i["bytes"].as_u64().unwrap() / 1_000_000);
            }
        }
        (m, incremental)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_mb(p: &Path, mb: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let data: Vec<u8> = (0..mb * 1_000_000).map(|i| (i * 7 + 3) as u8).collect();
    std::fs::write(p, data).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {:?}", args);
}

/// Lets the file-change journal record the last change before poo replays it.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1200));
}

#[test]
fn incremental_scans_match_full_scans() {
    let env = Env::new();
    let root = env.root();
    let p1 = root.join("app");
    std::fs::create_dir_all(&p1).unwrap();
    std::fs::write(p1.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(p1.join("main.rs"), "fn main() {}\n").unwrap();
    git(&p1, &["init", "-q"]);
    git(&p1, &["add", "."]);
    git(&p1, &["commit", "-qm", "init"]);
    write_mb(&p1.join("target/debug/a.bin"), 3);
    std::fs::write(p1.join("package.json"), "{}").unwrap();
    write_mb(&p1.join("node_modules/x/b.bin"), 2);

    let (full, _) = env.items(true);
    assert_eq!(full.get("/app/target delete"), Some(&3), "{:?}", full);

    // Nothing changed.
    let (inc, was_inc) = env.items(false);
    assert!(was_inc, "expected an incremental scan");
    assert_eq!(inc, full);

    // Growth deep inside a build folder, a new project, a deleted folder, a new worktree.
    write_mb(&p1.join("target/debug/deps/c.bin"), 4);
    let p2 = root.join("other");
    std::fs::create_dir_all(&p2).unwrap();
    std::fs::write(p2.join("Cargo.toml"), "[package]\n").unwrap();
    git(&p2, &["init", "-q"]);
    write_mb(&p2.join("target/release/x.bin"), 5);
    std::fs::remove_dir_all(p1.join("node_modules")).unwrap();
    git(&p1, &["worktree", "add", "-q", root.join("app-wt").to_str().unwrap(), "-b", "feat"]);
    write_mb(&root.join("app-wt/target/debug/w.bin"), 6);
    settle();
    let (inc, was_inc) = env.items(false);
    assert!(was_inc, "expected an incremental scan");
    assert_eq!(inc.get("/app/target delete"), Some(&7), "{:?}", inc);
    assert_eq!(inc.get("/other/target delete"), Some(&5), "{:?}", inc);
    assert_eq!(inc.get("/app-wt/target delete"), Some(&6), "{:?}", inc);
    assert!(!inc.keys().any(|k| k.contains("node_modules")), "{:?}", inc);

    // Shrink and remove the worktree.
    std::fs::remove_file(p1.join("target/debug/deps/c.bin")).unwrap();
    git(&p1, &["worktree", "remove", "--force", root.join("app-wt").to_str().unwrap()]);
    settle();
    let (inc, _) = env.items(false);
    let (full, _) = env.items(true);
    assert_eq!(inc, full);
    assert_eq!(full.get("/app/target delete"), Some(&3), "{:?}", full);
}
