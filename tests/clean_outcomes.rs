//! What a clean reports when things do not go to plan: an item something else already
//! removed is skipped (not failed), a tool's command that removes nothing is skipped with the
//! reason, and a real failure is kept in the history with its error. One test: env vars are
//! process-wide.
#![cfg(target_os = "macos")]

use poo::catalog;
use poo::clean::{self, CleanOptions};
use poo::config::Config;
use poo::model::*;
use std::path::Path;
use std::process::Command;

fn item(path: &Path, rule: &str, action: serde_json::Value, bytes: u64) -> Item {
    serde_json::from_value(serde_json::json!({
        "id": poo::util::short_hash(&path.to_string_lossy()), "path": path, "name": path.file_name().unwrap().to_string_lossy(),
        "rule": rule, "bucket": "caches", "group": "G", "bytes": bytes, "total_bytes": bytes, "files": 1, "tier": "cold",
        "cost": "rebuild", "reasons": [], "action": action,
    }))
    .unwrap()
}

#[test]
fn gone_nothing_removed_and_failures_are_reported_honestly() {
    let base = std::env::temp_dir().join(format!("poo-outcomes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let home = base.join("home");
    std::fs::create_dir_all(home.join("tools")).unwrap();
    std::fs::create_dir_all(base.join("poo/config/mods")).unwrap();
    std::fs::write(
        base.join("poo/config/mods/fake.toml"),
        r#"
[[rule]]
id = "fake.cache"
name = "Fake cache"
bucket = "caches"
kind = "path"
paths = ["~/tools/cache", "~/tools/gone", "~/tools/stuck/cache"]
cost = "none"
"#,
    )
    .unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("POO_HOME", base.join("poo"));
    std::env::set_var("POO_CLEAN_ONLY_UNDER", &home);
    std::env::remove_var("POO_STATE_DIR");
    std::env::remove_var("POO_READ_ONLY");
    let cat = catalog::load(&Config::load().unwrap());
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    let opts = CleanOptions { mode: RiskLevel::Force, ..Default::default() };

    // The owner's command ran and the folder did not shrink.
    let cache = home.join("tools/cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("keep"), vec![0u8; 20_000]).unwrap();
    let mut cmd = item(&cache, "fake.cache", serde_json::json!({"kind": "command", "command": ["/usr/bin/true"]}), 20_000);
    cmd.meta.insert("measure".into(), "path".into());
    // Removed by someone else after the scan.
    let gone = item(&home.join("tools/gone"), "fake.cache", serde_json::json!({"kind": "delete"}), 5_000);
    // A folder poo may not remove (its parent is locked).
    let stuck = home.join("tools/stuck/cache");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(stuck.join("f"), "x").unwrap();
    assert!(Command::new("chflags").arg("uchg").arg(home.join("tools/stuck")).status().unwrap().success());
    let fail = item(&stuck, "fake.cache", serde_json::json!({"kind": "delete"}), 4_096);

    let res = clean::clean(&[cmd, gone, fail], &cat, &opts);
    let _ = Command::new("chflags").arg("nouchg").arg(home.join("tools/stuck")).status();
    let by = |p: &Path| res.results.iter().find(|r| r.path == p.to_string_lossy()).unwrap().clone();

    let c = by(&cache);
    assert_eq!(c.status, "skipped", "{:?}", c);
    assert!(c.reason.as_deref().unwrap_or("").contains("removed nothing"), "{:?}", c);
    assert_eq!(c.bytes, 0);
    let g = by(&home.join("tools/gone"));
    assert_eq!(g.status, "skipped", "gone is not a failure: {:?}", g);
    let f = by(&stuck);
    assert_eq!(f.status, "failed", "{:?}", f);
    assert_eq!(res.failed, 1);
    assert_eq!(res.freed_bytes, 0, "nothing was freed");

    let history = clean::history(20);
    assert!(history.iter().any(|h| h["status"] == "failed" && h["path"] == stuck.to_string_lossy().as_ref() && h["reason"].is_string()), "{:?}", history);
    assert!(history.iter().all(|h| h["bytes"].as_u64() == Some(0)), "no history line claims freed space: {:?}", history);
    let _ = std::fs::remove_dir_all(&base);
}
