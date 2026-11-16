//! The `poo` binary as people and scripts meet it: help, errors and exit codes.
//! Nothing here scans the disk.

use std::process::Command;

fn poo(args: &[&str], cols: &str) -> std::process::Output {
    // An empty home keeps the one command that has to scan (`show`) quick and off real files.
    let home = std::env::temp_dir().join(format!("poo-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    Command::new(env!("CARGO_BIN_EXE_poo"))
        .args(args)
        .env("HOME", &home)
        .env("POO_HOME", home.join(".poo"))
        .env("POO_NO_UPDATE_CHECK", "1")
        .env("COLUMNS", cols)
        .env_remove("NO_COLOR")
        .output()
        .unwrap()
}

fn widest(s: &str) -> usize {
    s.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}

#[test]
fn help_is_grouped_and_fits_80_columns() {
    let o = poo(&["--help"], "80");
    assert!(o.status.success());
    let h = String::from_utf8(o.stdout).unwrap();
    for heading in ["Look:", "Clean:", "Keep it tidy:", "Agents:", "Set up and extend:", "Options:"] {
        assert!(h.contains(heading), "--help has no `{}` section:\n{}", heading, h);
    }
    assert!(widest(&h) <= 80, "--help is wider than 80 columns:\n{}", h);
    assert!(!h.contains("\x1b["), "--help to a pipe has escape codes");
}

#[test]
fn piped_errors_are_json_with_hints_and_exit_codes() {
    let o = poo(&["show", "nope-not-an-id"], "80");
    assert_eq!(o.status.code(), Some(3));
    let e: serde_json::Value = serde_json::from_slice(&o.stderr).unwrap();
    assert_eq!(e["error"]["kind"], "not_found");
    assert!(e["error"]["hint"].as_str().is_some_and(|h| h.contains("poo ls")));

    let o = poo(&["ls", "no-such-bucket", "--text"], "80");
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.starts_with("error: unknown bucket"), "{}", err);
    assert!(err.contains("hint: buckets:"), "{}", err);
}

#[test]
fn unknown_command_suggests_the_close_one() {
    let o = poo(&["lss"], "80");
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("'ls'"));
}
