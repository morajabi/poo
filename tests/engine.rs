//! Owner-command cleaning, probes, report-only items, snapshot limits and floor additions.
//! Everything runs in a throwaway HOME and POO_HOME with fake tools on PATH; nothing real is
//! scanned, cleaned or run. One test: env vars are process-wide.
#![cfg(unix)]

use poo::catalog;
use poo::clean::{self, floor_check, CleanOptions};
use poo::config::Config;
use poo::model::*;
use poo::scan::{self, ScanOptions};
use poo::util::{run_probe, Probe};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn item(path: &str, kind: ActionKind, command: Option<Vec<String>>) -> Item {
    let mut i: Item = serde_json::from_value(serde_json::json!({
        "id": "x", "path": path, "name": "x", "rule": "missing.rule", "bucket": "caches", "group": "G",
        "bytes": 1, "total_bytes": 1, "files": 1, "tier": "cold", "cost": "rebuild", "reasons": [],
        "action": {"kind": "delete"},
    }))
    .unwrap();
    i.action = Action { kind, command };
    i.meta = BTreeMap::new();
    i
}

fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{}\n", body)).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn engine_features_in_a_sandbox() {
    let base = std::env::temp_dir().join(format!("poo-engine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let home = base.join("home");
    let bin = base.join("bin");
    let cache = home.join("tools/fakecache");
    for d in [&home, &bin, &cache, &base.join("poo/config/mods"), &base.join("root")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(cache.join("big1"), vec![1u8; 300_000]).unwrap();
    std::fs::write(cache.join("big2"), vec![2u8; 300_000]).unwrap();
    // A fake owner tool that "prunes" one file, and a probe that answers from a flag file.
    script(&bin, "fakeprune", "rm -f \"$HOME/tools/fakecache/big1\"");
    script(&bin, "fakeprobe", "[ -f \"$HOME/probe-yes\" ]");
    std::fs::write(
        base.join("poo/config/config.toml"),
        "[scan]\nmin_item_bytes = 1\nwarm_days = 1\nidle_days = 1\n",
    )
    .unwrap();
    std::fs::write(
        base.join("poo/config/mods/fake.toml"),
        r#"
[[rule]]
id = "fake.cache"
name = "Fake cache"
bucket = "caches"
kind = "path"
paths = ["~/tools/fakecache"]
cost = "none"
clean_command = ["fakeprune"]
probe = ["fakeprobe"]
timeout_secs = 20

[[rule]]
id = "fake.nocmd"
name = "Fake cache, tool missing"
bucket = "caches"
kind = "path"
paths = ["~/tools/othercache"]
cost = "none"
clean_command = ["poo-tool-that-does-not-exist"]

[[rule]]
id = "fake.elsewhere"
name = "Something elsewhere"
bucket = "elsewhere"
kind = "path"
paths = ["~/tools/media"]
cost = "data"
recommend = "Manage it in Fake.app"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(home.join("tools/othercache")).unwrap();
    std::fs::write(home.join("tools/othercache/f"), vec![0u8; 1000]).unwrap();
    std::fs::create_dir_all(home.join("tools/media")).unwrap();
    std::fs::write(home.join("tools/media/f"), vec![0u8; 5000]).unwrap();

    std::env::set_var("HOME", &home);
    std::env::set_var("POO_HOME", base.join("poo"));
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    std::env::set_var("POO_CLEAN_ONLY_UNDER", &home);

    let cfg = Config::load().unwrap();
    let cat = catalog::load(&cfg);
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    assert!(cat.rule("fake.elsewhere").unwrap().report_only, "rules in a report-only bucket are report-only");
    let opts = ScanOptions { roots: Some(vec![base.join("root")]), no_providers: true, ..Default::default() };
    let report = scan::scan(&cfg, &cat, &opts);
    let get = |rule: &str| report.items.iter().find(|i| i.rule == rule).unwrap_or_else(|| panic!("no item for {}", rule)).clone();

    // C1: the owner command replaces delete when the tool exists, and falls back otherwise.
    let fake = get("fake.cache");
    assert_eq!(fake.action.kind, ActionKind::Command);
    assert!(fake.action.command.as_ref().unwrap()[0].ends_with("/fakeprune"));
    assert!(fake.safeguards.iter().any(|s| s.contains("own cleanup")), "{:?}", fake.safeguards);
    assert_eq!(get("fake.nocmd").action.kind, ActionKind::Delete);

    // C4: report-only items are sized and visible, but not in totals, plans or clean.
    let media = get("fake.elsewhere");
    assert!(media.report_only && media.recommend.as_deref() == Some("Manage it in Fake.app"));
    assert!(report.report_only_bytes >= media.bytes && report.total_bytes < report.items.iter().map(|i| i.bytes).sum::<u64>());
    assert!(report.buckets.iter().any(|b| b.id == "elsewhere" && b.report_only && b.bytes >= media.bytes));
    for p in &cat.plans {
        assert!(!poo::plan::eligible(p, &media), "plan {} took a report-only item", p.id);
    }
    let danger = CleanOptions { mode: RiskLevel::Danger, ..Default::default() };
    let err = clean::admit(&media, &cat, &danger, &[]).unwrap_err();
    assert!(err.contains("report only") && err.contains("Fake.app"), "{}", err);
    let e = poo::ops::resolve_ids(&report, &[media.id.clone()]).unwrap_err();
    assert!(e.downcast_ref::<poo::ops::ReportOnlyError>().is_some());

    // C12: the probe says no -> skipped, nothing ran.
    let real = CleanOptions { dry_run: false, ..Default::default() };
    let res = clean::clean(&[fake.clone()], &cat, &real);
    assert_eq!(res.results[0].status, "skipped", "{:?}", res.results[0]);
    assert!(res.results[0].reason.as_deref().unwrap().contains("said no"));
    assert!(cache.join("big1").exists());

    // Probe yes -> the command runs and freed bytes come from re-measuring the folder.
    std::fs::write(home.join("probe-yes"), "").unwrap();
    let res = clean::clean(&[fake.clone()], &cat, &real);
    let r0 = &res.results[0];
    assert_eq!(r0.status, "cleaned", "{:?}", r0);
    assert!(!cache.join("big1").exists() && cache.join("big2").exists(), "only the tool's own cleanup ran");
    assert!(r0.bytes >= 300_000 && r0.bytes < fake.bytes, "measured {} of {}", r0.bytes, fake.bytes);

    // Unknown answers (timeouts, missing tools) are their own outcome.
    assert!(matches!(run_probe(&["sleep".into(), "5".into()], None, 1), Probe::Unknown(_)));
    assert!(matches!(run_probe(&["poo-tool-that-does-not-exist".into()], None, 5), Probe::Unknown(_)));
    assert!(matches!(run_probe(&["false".into()], None, 5), Probe::No(_)));

    // C13: app bundles, ~/OrbStack and symlinked ancestors are refused in every mode.
    std::env::remove_var("POO_CLEAN_ONLY_UNDER");
    let check = |p: &Path| floor_check(&item(&p.to_string_lossy(), ActionKind::Delete, None), Some(RuleKind::Path), false, RiskLevel::Danger);
    let inside_app = home.join("Applications/Foo.app/Contents/Frameworks/old");
    std::fs::create_dir_all(&inside_app).unwrap();
    assert!(check(&inside_app).unwrap_err().contains("app bundle"));
    let orb = home.join("OrbStack/docker/volumes");
    std::fs::create_dir_all(&orb).unwrap();
    assert!(check(&orb).unwrap_err().contains("OrbStack"));
    std::fs::create_dir_all(home.join("real/sub/cache")).unwrap();
    std::os::unix::fs::symlink(home.join("real"), home.join("linked")).unwrap();
    assert!(check(&home.join("linked/sub/cache")).unwrap_err().contains("symlink"));
    assert!(check(&home.join("real/sub/cache")).is_ok());

    // C3: Time Machine snapshot commands only for one snapshot older than a day, never on a schedule.
    let tm = |date: &str| item("tmsnapshot://x", ActionKind::Command, Some(vec!["tmutil".into(), "deletelocalsnapshots".into(), date.into()]));
    assert!(floor_check(&tm("2020-01-01-000000"), None, false, RiskLevel::Force).is_ok());
    assert!(floor_check(&tm("/"), None, false, RiskLevel::Danger).is_err());
    let today = poo::util::iso8601(poo::util::now()).get(..10).unwrap().to_string() + "-000000";
    let soon = poo::providers::snapshots::parse_snapshot_date(&today).unwrap();
    if poo::util::now() - soon < 24 * 3600 {
        assert!(floor_check(&tm(&today), None, false, RiskLevel::Danger).unwrap_err().contains("less than a day"));
    }
    let thin = item("tmsnapshot://x", ActionKind::Command, Some(vec!["tmutil".into(), "thinlocalsnapshots".into(), "/".into()]));
    assert!(floor_check(&thin, None, false, RiskLevel::Danger).is_err());
    let sched = CleanOptions { mode: RiskLevel::Force, actor: "schedule:nightly".into(), ..Default::default() };
    assert!(clean::admit(&tm("2020-01-01-000000"), &cat, &sched, &[]).unwrap_err().contains("schedule"));

    // Check-first items (simulator runtimes, SDKs, models) are never cleaned by a schedule
    // or a hook, in any mode; a person can still pick them.
    let mut runtime = item("simruntime://abc", ActionKind::Command, Some(vec!["xcrun".into(), "simctl".into(), "runtime".into(), "delete".into(), "abc".into()]));
    runtime.tier = Tier::Review;
    for actor in ["schedule:guard", "hook:claude"] {
        let o = CleanOptions { mode: RiskLevel::Danger, actor: actor.into(), ..Default::default() };
        assert!(clean::admit(&runtime, &cat, &o, &[]).unwrap_err().contains("check first"), "{actor}");
    }
    let rt_rule = cat.rule("simulator.runtimes").unwrap();
    assert_eq!(rt_rule.tier, Some(Tier::Review));

    // C18: every saved report adds a growth sample.
    scan::save_report(&report).unwrap();
    assert_eq!(poo::growth::load().len(), 1);
    let g = poo::growth::report(&report, 7 * 86400);
    assert!(g["from"].is_null() && g["forecast"]["days_until_full"].is_null());

    std::fs::remove_dir_all(&base).unwrap();
}
