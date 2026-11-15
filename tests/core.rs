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
        tags: vec![],
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
        risks: vec![],
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
        "/System/Library".to_string(),
        "/usr".to_string(),
    ] {
        assert!(floor_check(&item(&bad, ActionKind::Delete), Some(RuleKind::Artifact), false, RiskLevel::Danger).is_err(), "allowed {}", bad);
    }
    // Virtual items only through commands.
    assert!(floor_check(&item("docker://images", ActionKind::Delete), None, false, RiskLevel::Standard).is_err());
    assert!(floor_check(&item("docker://images", ActionKind::Command), None, false, RiskLevel::Standard).is_ok());
}

#[test]
fn safety_floor_refuses_symlinks_and_repos() {
    let base = std::env::temp_dir().join(format!("poo-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("proj/target/.git")).unwrap();
    std::os::unix::fs::symlink(base.join("proj"), base.join("link")).unwrap();
    let t = base.join("proj/target").to_string_lossy().to_string();
    assert!(floor_check(&item(&t, ActionKind::Delete), Some(RuleKind::Artifact), false, RiskLevel::Force).unwrap_err().contains(".git"));
    let l = base.join("link").to_string_lossy().to_string();
    assert!(floor_check(&item(&l, ActionKind::Delete), Some(RuleKind::Path), false, RiskLevel::Danger).unwrap_err().contains("symlink"));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn schedule_parsing() {
    use poo::schedule::{parse_amount, parse_every};
    assert_eq!(parse_every("30m").unwrap(), 1800);
    assert_eq!(parse_every("1d").unwrap(), 86400);
    assert!(parse_every("5m").is_err(), "faster than the 15 minute tick");
    assert!(parse_every("soon").is_err());
    assert_eq!(parse_amount("50G", 0).unwrap(), 50_000_000_000);
    assert_eq!(parse_amount("10%", 1_000).unwrap(), 100);
    let cat = catalog::load(&Config::default());
    for (id, _, s) in poo::schedule::presets() {
        poo::schedule::validate(&s, &cat).unwrap_or_else(|e| panic!("preset {} invalid: {}", id, e));
    }
}

#[test]
fn risks_gate_cleaning_modes() {
    use poo::clean::{admit, CleanOptions};
    let cat = catalog::load(&Config::default());
    let base = std::env::temp_dir().join(format!("poo-risk-{}", std::process::id()));
    std::fs::create_dir_all(base.join("app/target/debug")).unwrap();
    std::fs::write(base.join("app/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(base.join("app/target/.rustc_info.json"), "{}").unwrap();
    let mut it = item(&base.join("app/target").to_string_lossy(), ActionKind::Delete);
    let opts = |mode| CleanOptions { mode, max_tier: Some(Tier::Warm), ..Default::default() };
    assert!(admit(&it, &cat, &opts(RiskLevel::Standard), &[]).is_ok());
    it.risks.push(Risk::new("recent", RiskLevel::Force, "used in the last hour"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Standard), &[]).unwrap_err().contains("--force"));
    let warnings = admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap();
    assert!(warnings.iter().any(|w| w.contains("recent")));
    it.risks.push(Risk::new("data", RiskLevel::Danger, "real data"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap_err().contains("--danger"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Danger), &[]).is_ok());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn plan_filters_by_bucket_and_tag() {
    use poo::plan::Filter;
    let mut it = item("/x/app/target", ActionKind::Delete);
    it.tags = vec!["rust".into(), "build-output".into()];
    assert!(Filter { buckets: vec!["build".into()], ..Default::default() }.matches(&it));
    assert!(!Filter { exclude_buckets: vec!["build".into()], ..Default::default() }.matches(&it));
    assert!(Filter { tags: vec!["rust".into()], ..Default::default() }.matches(&it));
    assert!(!Filter { exclude_tags: vec!["build-output".into()], ..Default::default() }.matches(&it));
}
