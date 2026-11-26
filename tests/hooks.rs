//! The agent hook when another clean is running: agents often finish together, and nothing
//! retries a hook, so it waits for the clean ahead of it instead of leaving its worktree's
//! build output behind. Builds a throwaway home in $TMPDIR. One test: env vars are process-wide.
#![cfg(unix)]

use poo::schedule::RunLock;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

#[test]
fn hook_waits_for_a_running_clean() {
    let home = std::env::temp_dir().join(format!("poo-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let repo = home.join("dev/app");
    let wt = home.join("dev/app-wt");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(home.join(".poo/config")).unwrap();
    std::fs::write(home.join(".poo/config/config.toml"), "[hooks]\nnotify = false\n").unwrap();
    std::fs::write(repo.join("package.json"), "{\"name\":\"app\"}\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    // A worktree whose work landed (nothing past main), with build output in it.
    git(&repo, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
    let deps = wt.join("node_modules/left-pad");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("index.js"), vec![b'x'; 1 << 20]).unwrap();

    // Another clean holds the run lock and finishes a moment after the hook starts.
    std::env::set_var("POO_HOME", home.join(".poo"));
    let held = RunLock::try_acquire("schedule:guard").expect("lock");
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        drop(held);
    });
    let t = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_poo"))
        .args(["hook", "agent-done", "--agent", "claude", "--cwd", wt.to_str().unwrap(), "--detached"])
        .env("HOME", &home)
        .env("POO_HOME", home.join(".poo"))
        .env("POO_NO_UPDATE_CHECK", "1")
        .env("POO_NO_TELEMETRY", "1")
        .output()
        .unwrap();
    releaser.join().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = std::fs::read_to_string(home.join(".poo/state/hooks.log")).unwrap_or_default();
    assert!(t.elapsed() >= Duration::from_millis(1500), "the hook did not wait for the clean ahead of it: {}", log);
    assert!(log.contains("(1 cleaned, 0 skipped)"), "{}", log);
    assert!(!wt.join("node_modules").exists(), "the worktree's build output is still there: {}", log);
    let _ = std::fs::remove_dir_all(&home);
}
