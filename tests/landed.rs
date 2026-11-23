//! How poo tells that a branch's work landed: fast-forward, squash, rebase, a merged PR
//! with commits after it, and a deleted upstream. Builds throwaway repos in $TMPDIR.

use poo::providers::git::{self, Landed};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

struct Sandbox {
    root: PathBuf,
    repo: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("poo-landed-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "--bare", "origin.git"]);
        git(&root, &["clone", "-q", "origin.git", "repo"]);
        let repo = root.join("repo");
        commit(&repo, "a.txt", "a\n", "base");
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        git(&repo, &["remote", "set-head", "origin", "main"]);
        Sandbox { root, repo }
    }

    fn branch(&self, name: &str) {
        git(&self.repo, &["checkout", "-q", "-b", name, "main"]);
    }

    fn main(&self) {
        git(&self.repo, &["checkout", "-q", "main"]);
    }

    fn push_main(&self) {
        git(&self.repo, &["push", "-q", "origin", "main"]);
        git(&self.repo, &["fetch", "-q", "origin"]);
    }

    fn landed(&self, branch: &str) -> Option<Landed> {
        git::landed(&self.repo, branch, "origin/main", Some(branch))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fast_forward_and_unmerged() {
    let s = Sandbox::new("ff");
    s.branch("ff");
    commit(&s.repo, "b.txt", "b\n", "ff work");
    s.branch("open");
    commit(&s.repo, "c.txt", "c\n", "open work");
    s.main();
    git(&s.repo, &["merge", "-q", "--ff-only", "ff"]);
    s.push_main();
    assert_eq!(s.landed("ff"), Some(Landed::Ancestor));
    assert_eq!(s.landed("open"), None);
    assert!(git::is_merged(&s.repo, "ff", "origin/main"));
    assert!(!git::is_merged(&s.repo, "open", "origin/main"));
}

#[test]
fn squash_merge_counts_even_after_main_moves_on() {
    let s = Sandbox::new("squash");
    s.branch("feat");
    commit(&s.repo, "b.txt", "one\n", "feat 1");
    commit(&s.repo, "b.txt", "one\ntwo\n", "feat 2");
    s.main();
    git(&s.repo, &["merge", "-q", "--squash", "feat"]);
    git(&s.repo, &["commit", "-q", "-m", "feat (#1)"]);
    commit(&s.repo, "z.txt", "later\n", "unrelated later work");
    s.push_main();
    assert_eq!(s.landed("feat"), Some(Landed::Tree));
    assert!(git::is_merged(&s.repo, "feat", "origin/main"), "the hook's check sees squash merges too");
}

#[test]
fn rebase_merge_counts_when_main_changed_the_same_file_since() {
    let s = Sandbox::new("rebase");
    s.branch("feat");
    commit(&s.repo, "c.txt", "v1\n", "add c");
    let picked = git(&s.repo, &["rev-parse", "HEAD"]);
    s.main();
    commit(&s.repo, "d.txt", "d\n", "main moved");
    git(&s.repo, &["cherry-pick", picked.as_str()]);
    commit(&s.repo, "c.txt", "v2\n", "c changed again");
    s.push_main();
    // Merging feat now conflicts with main's newer c.txt, but its patch is in main.
    assert_eq!(s.landed("feat"), Some(Landed::Patch));
}

#[test]
fn commits_after_a_merged_head_are_counted() {
    let s = Sandbox::new("after");
    s.branch("feat");
    commit(&s.repo, "b.txt", "b\n", "feat");
    let merged_head = git(&s.repo, &["rev-parse", "HEAD"]);
    commit(&s.repo, "b.txt", "b\nmore\n", "after the merge");
    s.main();
    git(&s.repo, &["merge", "-q", "--squash", &merged_head]);
    git(&s.repo, &["commit", "-q", "-m", "feat (#2)"]);
    s.push_main();
    // The extra commit never reached main: no content check calls this landed.
    assert_eq!(s.landed("feat"), None);
    assert_eq!(git::commits_after(&s.repo, &merged_head, "feat"), Some(1));
    assert_eq!(git::commits_after(&s.repo, "0123456789012345678901234567890123456789", "feat"), None);
    let pr = Landed::Pr { number: 2, extra: Some(1) };
    assert!(!pr.complete());
    assert!(Landed::Pr { number: 2, extra: Some(0) }.complete());
    assert!(pr.describe("main").contains("1 local commit made after it"));
}

#[test]
fn deleted_upstream_is_a_hint() {
    let s = Sandbox::new("gone");
    s.branch("feat");
    commit(&s.repo, "b.txt", "b\n", "feat");
    git(&s.repo, &["push", "-q", "-u", "origin", "feat"]);
    git(&s.repo, &["push", "-q", "origin", "--delete", "feat"]);
    git(&s.repo, &["fetch", "-q", "--prune", "origin"]);
    s.main();
    let l = s.landed("feat");
    assert_eq!(l, Some(Landed::Gone));
    assert!(!l.unwrap().complete(), "a deleted remote branch alone never makes a worktree removable");
}

#[test]
fn github_slugs() {
    let s = Sandbox::new("slug");
    for (url, want) in [
        ("git@github.com:usenoor/poo.git", Some("usenoor/poo")),
        ("https://github.com/usenoor/poo", Some("usenoor/poo")),
        ("ssh://git@github.com/usenoor/poo.git", Some("usenoor/poo")),
        ("https://gitlab.com/usenoor/poo.git", None),
    ] {
        git(&s.repo, &["remote", "set-url", "origin", url]);
        assert_eq!(git::github_slug(&s.repo).as_deref(), want, "{}", url);
    }
}
