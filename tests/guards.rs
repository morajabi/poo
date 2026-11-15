//! Env-only testing guards. One test per binary: env vars are process-wide.
use poo::catalog;
use poo::clean::{self, floor_check, CleanOptions};
use poo::config::Config;
use poo::model::*;
use std::collections::BTreeMap;

fn item(path: &str, kind: ActionKind) -> Item {
    Item {
        id: "x".into(), path: path.into(), name: "x".into(), rule: "missing.rule".into(), bucket: "build".into(),
        group: "Rust".into(), tags: vec![], bytes: 1, total_bytes: 1, files: 1, last_used: None, tier: Tier::Cold,
        cost: Cost::Rebuild, reasons: vec![], rebuild: None, project: None, repo: None, agent: None,
        action: Action { kind, command: None }, risks: vec![], safeguards: vec![], children: vec![], meta: BTreeMap::new(),
        report_only: false, recommend: None,
    }
}

#[test]
fn env_guards_only_add_safety() {
    let base = std::env::temp_dir().join(format!("poo-guards-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("fixture/a")).unwrap();
    std::fs::create_dir_all(base.join("outside/b")).unwrap();
    let inside = base.join("fixture/a").to_string_lossy().to_string();
    let outside = base.join("outside/b").to_string_lossy().to_string();
    let check = |p: &str, k| floor_check(&item(p, k), Some(RuleKind::Path), false, RiskLevel::Danger);

    assert!(check(&outside, ActionKind::Delete).is_ok());
    std::env::set_var("POO_CLEAN_ONLY_UNDER", base.join("fixture"));
    assert!(check(&inside, ActionKind::Delete).is_ok());
    assert!(check(&outside, ActionKind::Delete).unwrap_err().contains("POO_CLEAN_ONLY_UNDER"));
    assert!(check(&base.join("fixture").to_string_lossy(), ActionKind::Delete).is_err(), "the root itself");
    assert!(check("docker://images", ActionKind::Command).is_err(), "virtual items");
    std::env::set_var("POO_CLEAN_ONLY_UNDER", base.join("nope"));
    assert!(check(&inside, ActionKind::Delete).is_err(), "missing dir refuses all");
    std::env::remove_var("POO_CLEAN_ONLY_UNDER");

    // Read-only: a real clean becomes a dry run and says so; the folder stays.
    let cat = catalog::load(&Config::default());
    std::env::set_var("POO_READ_ONLY", "1");
    let opts = CleanOptions { dry_run: false, mode: RiskLevel::Danger, ..Default::default() };
    let res = clean::clean(&[item(&inside, ActionKind::Delete)], &cat, &opts);
    assert!(res.dry_run && res.read_only.is_some());
    assert!(std::path::Path::new(&inside).exists());
    assert!(poo::hooks::install("claude").unwrap().contains("POO_READ_ONLY"));
    assert!(poo::agent::install("claude", true, true).unwrap()[0].contains("POO_READ_ONLY"));
    assert!(poo::schedule::install_agent().is_err());
    std::env::remove_var("POO_READ_ONLY");
    std::fs::remove_dir_all(&base).unwrap();
}
