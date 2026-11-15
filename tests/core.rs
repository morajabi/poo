use poo::catalog;
use poo::clean::floor_check;
use poo::config::Config;
use poo::model::*;
use std::collections::BTreeMap;

fn item(path: &str, kind: ActionKind) -> Item {
    Item {
        id: "x".into(),
        path: path.into(),
        name: "x".into(),
        rule: "rust.target".into(),
        bucket: "build".into(),
        group: "Rust".into(),
        bytes: 1,
        total_bytes: 1,
        files: 1,
        last_used: None,
        tier: Tier::Cold,
        cost: Cost::Rebuild,
        reasons: vec![],
        rebuild: None,
        project: None,
        repo: None,
        agent: None,
        action: Action { kind, command: None },
        children: vec![],
        meta: BTreeMap::new(),
    }
}

#[test]
fn core_mod_loads_cleanly() {
    let cat = catalog::load(&Config::default());
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    assert!(cat.rules.len() > 150, "only {} rules", cat.rules.len());
    for id in ["quick", "stale", "deep", "emergency"] {
        assert!(cat.plan(id).is_some(), "missing plan {}", id);
    }
    // Every rule points at a real bucket.
    for r in &cat.rules {
        assert!(cat.bucket(&r.bucket).is_some(), "{} -> {}", r.id, r.bucket);
    }
}

#[test]
fn artifact_matchers() {
    let cat = catalog::load(&Config::default());
    let rust = cat.rule("rust.target").unwrap();
    let p = std::path::Path::new("/x/app/target");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(rust.matches_artifact("target", p, &s(&["Cargo.toml", "src"]), &s(&["debug", ".rustc_info.json"])));
    assert!(!rust.matches_artifact("target", p, &s(&["pom.xml"]), &s(&["debug"])));
    let dd = cat.rule("xcode.custom-derived-data").unwrap();
    assert!(dd.matches_artifact(".dd", p, &s(&[]), &s(&["Build", "Logs", "Index.noindex"])));
    assert!(!dd.matches_artifact("docs", p, &s(&[]), &s(&["Build", "Notes"])));
    let dist = cat.rule("node.dist").unwrap();
    assert!(dist.ignored, "dist must require git-ignored folders");
}

#[test]
fn config_overrides_merge_by_field() {
    let mut cfg = Config::default();
    cfg.rule_overrides.insert("rust.target".into(), RuleSpec { warm_days: Some(3), ..Default::default() });
    cfg.disabled_rules.push("system.app-caches".into());
    cfg.bucket_overrides.insert("junk".into(), BucketSpec { name: Some("Junk drawer".into()), ..Default::default() });
    let cat = catalog::load(&cfg);
    let r = cat.rule("rust.target").unwrap();
    assert_eq!(r.warm_days, 3);
    assert!(!r.names.raw.is_empty(), "override must keep other fields");
    assert!(!cat.rule("system.app-caches").unwrap().enabled);
    assert_eq!(cat.bucket("junk").unwrap().name, "Junk drawer");
}

#[test]
fn safety_floor_refuses_dangerous_paths() {
    let home = poo::util::home();
    let h = home.to_string_lossy();
    for bad in [
        "/".to_string(),
        h.to_string(),
        format!("{}/Library", h),
        format!("{}/.ssh/keys", h),
        "/usr/local/lib".to_string(),
        format!("{}/dev/../.ssh", h),
    ] {
        assert!(floor_check(&item(&bad, ActionKind::Delete), Some(RuleKind::Artifact), false).is_err(), "allowed {}", bad);
    }
    // Virtual items only through commands.
    assert!(floor_check(&item("docker://images", ActionKind::Delete), None, false).is_err());
    assert!(floor_check(&item("docker://images", ActionKind::Command), None, false).is_ok());
}

#[test]
fn safety_floor_refuses_symlinks_and_repos() {
    let base = std::env::temp_dir().join(format!("poo-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("proj/target/.git")).unwrap();
    std::os::unix::fs::symlink(base.join("proj"), base.join("link")).unwrap();
    let t = base.join("proj/target").to_string_lossy().to_string();
    assert!(floor_check(&item(&t, ActionKind::Delete), Some(RuleKind::Artifact), false).unwrap_err().contains(".git"));
    let l = base.join("link").to_string_lossy().to_string();
    assert!(floor_check(&item(&l, ActionKind::Delete), Some(RuleKind::Path), false).unwrap_err().contains("symlink"));
    std::fs::remove_dir_all(&base).unwrap();
}
