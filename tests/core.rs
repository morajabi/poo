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
        safeguards: vec![],
        children: vec![],
        meta: BTreeMap::new(),
        frees: 0,
        report_only: false,
        recommend: None,
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
fn scan_pace_scales_windows_and_is_settable() {
    let cfg = Config::default();
    assert_eq!(cfg.scan.window(3600), 3600);
    let cfg = poo::ops::set_config_key(&cfg, "scan.pace", "eager").unwrap();
    assert_eq!(cfg.scan.window(3600), 1800);
    let cfg = poo::ops::set_config_key(&cfg, "scan.pace", "relaxed").unwrap();
    assert_eq!(cfg.scan.window(3600), 7200);
    assert!(poo::ops::set_config_key(&cfg, "scan.pace", "sometimes").is_err());
    let cfg = poo::ops::set_config_key(&cfg, "scan.worktree_idle_hours", "8").unwrap();
    assert_eq!(cfg.scan.worktree_idle_hours, 8);
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
    assert!(admit(&it, &cat, &opts(RiskLevel::Standard), &[]).unwrap_err().contains("--include-in-use"));
    let warnings = admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap();
    assert!(warnings.iter().any(|w| w.contains("recent")));
    it.risks.push(Risk::new("data", RiskLevel::Danger, "real data"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap_err().contains("--dangerously-include-protected"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Danger), &[]).is_ok());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn running_builds_veto_cleaning_even_with_the_guard_off() {
    use poo::clean::{admit, CleanOptions};
    use poo::procs::Proc;
    let mut cfg = Config::default();
    cfg.disabled_guards.push("core.build-processes".into());
    let cat = catalog::load(&cfg);
    let base = std::env::temp_dir().join(format!("poo-veto-{}", std::process::id()));
    std::fs::create_dir_all(base.join("app/target/debug")).unwrap();
    std::fs::write(base.join("app/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(base.join("app/target/.rustc_info.json"), "{}").unwrap();
    let app = base.join("app");
    let mut it = item(&app.join("target").to_string_lossy(), ActionKind::Delete);
    it.project = Some(app.to_string_lossy().to_string());
    let opts = CleanOptions { mode: RiskLevel::Force, max_tier: Some(Tier::Warm), ..Default::default() };
    let p = |name: &str, cwd: &std::path::Path, args: &[&str]| {
        Proc::with_args(1, name, cwd, &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &[])
    };
    // cargo in the project, and Xcode in it (it may link a Rust staticlib), both veto.
    let cargo = p("cargo", &app, &["cargo", "build"]);
    assert!(admit(&it, &cat, &opts, &[cargo]).unwrap_err().contains("cargo"));
    let xb = p("xcodebuild", &app.join("apple"), &["xcodebuild", "-derivedDataPath", "/tmp/dd"]);
    assert!(admit(&it, &cat, &opts, &[xb]).unwrap_err().contains("xcodebuild"));
    // A build in another checkout does not.
    let other = p("cargo", &base.join("other"), &["cargo", "build"]);
    assert!(admit(&it, &cat, &opts, &[other]).is_ok());
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

#[test]
fn risky_artifact_matchers() {
    let cat = catalog::load(&Config::default());
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let p = std::path::Path::new("/x/proj/thing");
    let m = |id: &str, name: &str, parent: &[&str], children: &[&str]| {
        cat.rule(id).unwrap_or_else(|| panic!("missing {}", id)).matches_artifact(name, p, &s(parent), &s(children))
    };

    // CMake: a build tree yes, an in-source build (the source folder itself) never.
    assert!(m("cmake.build", "build", &["CMakeLists.txt"], &["CMakeCache.txt", "CMakeFiles", "Makefile"]));
    assert!(!m("cmake.build", "lib", &["CMakeLists.txt"], &["CMakeCache.txt", "CMakeLists.txt", "src"]));
    assert!(!m("meson.build", "proj", &[], &["build.ninja", "meson-private", "meson.build"]));

    // Conda: an env inside a project yes, a whole Miniconda install no.
    assert!(m("python.conda-env", "env", &["environment.yml"], &["conda-meta", "bin", "lib"]));
    assert!(!m("python.conda-env", "miniconda3", &[], &["conda-meta", "condabin", "envs", "pkgs", "bin"]));

    // Generic names need git to ignore them.
    for id in ["php.vendor", "node.electron-builds", "rn.expo-prebuild", "docs.mkdocs-site", "docs.sphinx-build", "docs.mdbook", "jvm.intellij-out", "node.framework-caches"] {
        assert!(cat.rule(id).unwrap().ignored, "{} must only match git-ignored folders", id);
    }
    // Jekyll no longer claims Hugo's resources/ or any folder next to a config.toml.
    assert!(!m("jekyll.site", "resources", &["config.toml"], &["_gen"]));
    assert!(!m("jekyll.site", "_site", &["config.toml", "pyproject.toml"], &["index.html"]));
    assert!(m("jekyll.site", "_site", &["_config.yml", "Gemfile"], &["index.html"]));

    // Expo prebuild only with an Expo config and native project inside.
    assert!(m("rn.expo-prebuild", "ios", &["app.json", "package.json"], &["Podfile", "App.xcworkspace"]));
    assert!(!m("rn.expo-prebuild", "ios", &["package.json"], &["Podfile"]));
    assert!(!m("rn.expo-prebuild", "android", &["app.json", "package.json"], &["README.md"]));

    // Electron packaging output, not a plain dist of JS files.
    assert!(m("node.electron-builds", "dist", &["package.json"], &["mac-arm64", "MyApp-1.0.0-arm64.dmg", "latest-mac.yml"]));
    assert!(!m("node.electron-builds", "dist", &["package.json"], &["index.js", "index.d.ts"]));

    // Lean, docs, sbt, Racket, Elixir deps.
    assert!(m("lean.lake", ".lake", &["lakefile.lean", "lean-toolchain"], &["build", "packages"]));
    assert!(m("docs.sphinx-build", "_build", &["conf.py", "index.rst"], &["html", "doctrees"]));
    assert!(!m("docs.sphinx-build", "_build", &["mix.exs"], &["dev", "test"]));
    assert!(m("scala.sbt-project-target", "target", &["build.properties", "plugins.sbt"], &["scala-2.12", "streams"]));
    assert!(!m("scala.sbt-project-target", "project", &["build.sbt"], &["build.properties", "plugins.sbt"]));
    assert!(m("racket.compiled", "compiled", &["main.rkt"], &["main_rkt.zo", "main_rkt.dep"]));
    assert!(!m("racket.compiled", "compiled", &["main.rkt"], &["main_rkt.zo", "keep.rkt"]));
    assert!(m("elixir.deps", "deps", &["mix.exs", "mix.lock"], &["phoenix", "jason"]));
    assert_eq!(cat.rule("elixir.deps").unwrap().cost, Cost::Redownload);
    assert!(!m("elixir.build", "deps", &["mix.exs"], &["phoenix"]), "deps moved to elixir.deps");
    assert!(m("node.cdk-out", "cdk.out", &["cdk.json"], &["manifest.json", "tree.json"]));

    // One Terragrunt rule, and it keeps the local-state review.
    assert!(cat.rule("terragrunt.cache").is_none());
    assert_eq!(cat.rule("terraform.terragrunt-cache").unwrap().tier, Some(Tier::Review));
}

#[test]
fn browser_caches_split_by_permission() {
    let cat = catalog::load(&Config::default());
    for b in ["chrome", "brave", "edge", "arc", "dia", "vivaldi", "opera", "helium", "comet", "chromium", "firefox", "zen", "firefox-forks"] {
        let open = cat.rule(&format!("browser.{}-cache", b)).unwrap();
        let gated = cat.rule(&format!("browser.{}-profile-caches", b)).unwrap();
        assert!(!open.tags.iter().any(|t| t == "needs-permission"), "{} cache must not need permission", b);
        assert!(gated.tags.iter().any(|t| t == "needs-permission"), "{} profile caches must need permission", b);
        assert!(!open.running_apps.is_empty() && open.running_apps == gated.running_apps, "{} running_apps", b);
    }
    for id in ["browser.safari-cache", "xcode.ios-backups", "app.mail-cache"] {
        assert!(cat.rule(id).unwrap().tags.iter().any(|t| t == "needs-permission"), "{}", id);
    }
}

#[test]
fn rule_fixtures_and_core_lint() {
    let cat = catalog::load(&Config::default());
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let report = poo::contrib::lint_core(&cat, repo, None).unwrap();
    let errors: Vec<_> = report.findings.iter().filter(|f| f.level == poo::contrib::Level::Error).collect();
    assert!(errors.is_empty(), "{:#?}", errors);
    assert!(poo::contrib::read_fixtures(&repo.join("tests/rule-fixtures")).unwrap().len() >= 20);
}

#[test]
fn contrib_lint_and_scrub() {
    use poo::contrib::{lint_spec, replace_word, Level};
    let cat = catalog::load(&Config::default());
    let errors = |spec: RuleSpec| lint_spec(&spec, &cat, true, true).into_iter().filter(|f| f.level == Level::Error).map(|f| f.message).collect::<Vec<_>>();
    let base = || RuleSpec {
        id: "acme.cache".into(),
        name: Some("Acme cache".into()),
        bucket: Some("caches".into()),
        group: Some("Acme".into()),
        kind: Some(RuleKind::Path),
        stacks: Some(vec!["node".into()]),
        cost: Some(Cost::Rebuild),
        rebuild: Some("acme build".into()),
        description: Some("Acme's build cache.".into()),
        ..Default::default()
    };
    assert!(errors(RuleSpec { paths: Some(vec!["~/Library/Caches/acme".into()]), ..base() }).is_empty());
    for bad in ["/Users/someone/Library/Caches/acme", "~/.ssh/acme", "~/*", "~/.acme", "/opt/acme", "~/dev/app/.env.local"] {
        assert!(!errors(RuleSpec { paths: Some(vec![bad.into()]), ..base() }).is_empty(), "{} should fail", bad);
    }
    assert!(!errors(RuleSpec { description: None, paths: Some(vec!["~/.acme/cache".into()]), ..base() }).is_empty());
    let art = |names: &[&str], parent: Option<&[&str]>, children: Option<&[&str]>, ignored: Option<bool>| RuleSpec {
        kind: Some(RuleKind::Artifact),
        bucket: Some("build".into()),
        names: Some(names.iter().map(|s| s.to_string()).collect()),
        parent_any: parent.map(|v| v.iter().map(|s| s.to_string()).collect()),
        children_any: children.map(|v| v.iter().map(|s| s.to_string()).collect()),
        ignored,
        ..base()
    };
    assert!(!errors(art(&["build"], None, None, None)).is_empty(), "bare generic name");
    assert!(!errors(art(&["build"], Some(&["acme.json"]), None, None)).is_empty(), "marker but no proof");
    assert!(errors(art(&["build"], Some(&["acme.json"]), None, Some(true))).is_empty());
    assert!(errors(art(&[".acme-cache"], None, None, None)).is_empty(), "specific names need nothing else");
    assert_eq!(replace_word("Built by mo for inline-chat", "inline-chat", "<project>"), "Built by mo for <project>");
    assert_eq!(replace_word("inlined code", "inline", "<project>"), "inlined code");
}

#[test]
fn versions_keep_newest_per_folder_by_os_version() {
    use poo::providers::version_key;
    // Device Support names: OS version first (device model prefix ignored), then the build.
    assert!(version_key("17.5.1 (21F90)") > version_key("17.5 (21F79)"));
    assert!(version_key("iPhone16,1 18.0 (22A3354)") > version_key("17.5.1 (21F90)"));
    assert!(version_key("10.0 (14A345)") > version_key("9.3.5 (13G36)"), "numeric, not string, order");
    assert!(version_key("Current").is_none());

    let base = std::env::temp_dir().join(format!("poo-versions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for (dir, names) in [
        ("iOS DeviceSupport", &["16.4 (20E247)", "17.5 (21F79)", "18.1 (22B83)", "9.3 (13E233)"][..]),
        ("watchOS DeviceSupport", &["Watch6,1 10.5 (21T576)"][..]),
    ] {
        for n in names {
            std::fs::create_dir_all(base.join(dir).join(n)).unwrap();
        }
    }
    let cat = catalog::load(&Config::default());
    let mut rule = cat.rule("xcode.device-support").unwrap().clone();
    rule.params.insert("path".into(), toml::Value::Array(vec![toml::Value::String(format!("{}/*DeviceSupport", base.display()))]));
    let names: Vec<String> = poo::providers::versions(&rule).into_iter().map(|r| r.name).collect();
    assert_eq!(names.len(), 2, "{:?}", names);
    assert!(names.contains(&"16.4 (20E247)".to_string()) && names.contains(&"9.3 (13E233)".to_string()), "{:?}", names);
    // A folder with a name it cannot order keeps everything.
    std::fs::create_dir_all(base.join("iOS DeviceSupport/Unknown")).unwrap();
    assert!(poo::providers::versions(&rule).is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn offline_music_and_running_dlx_are_protected() {
    let cat = catalog::load(&Config::default());
    let sp = cat.rule("app.spotify-cache").unwrap();
    assert!(sp.exclude.matches("Data"), "Spotify's offline storage is never in the cache rule");
    assert!(sp.paths.iter().all(|p| !p.contains("PersistentCache")));
    assert_eq!(cat.rule("app.spotify-offline").unwrap().tier, Some(Tier::Review));
    assert!(cat.rule("pnpm.metadata-cache").unwrap().exclude.matches("dlx"));
    assert!(cat.rule("pnpm.dlx-cache").unwrap().running_apps.iter().any(|a| a == "pnpm"));
    // Catch-all rules sort after every specific rule.
    let first_fallback = cat.rules.iter().position(|r| r.tags.iter().any(|t| t == "fallback")).unwrap();
    assert!(cat.rules[first_fallback..].iter().all(|r| r.tags.iter().any(|t| t == "fallback")));
}

#[test]
fn elsewhere_rules_are_report_only() {
    let cat = catalog::load(&Config::default());
    let aware: Vec<_> = cat.rules.iter().filter(|r| r.id.starts_with("aware.")).collect();
    assert!(aware.len() >= 7);
    for r in aware {
        assert!(r.report_only, "{} must be report-only", r.id);
        assert!(r.recommend.is_some(), "{} must say where to manage it", r.id);
    }
    // Partial Steam downloads are the one cleanable piece, and only while Steam is closed.
    let steam = cat.rule("app.steam-partial-downloads").unwrap();
    assert!(!steam.report_only && !steam.running_apps.is_empty());
}

#[test]
fn plans_keep_only_what_clean_admits() {
    use poo::plan::{evaluate, freed_total, Admission, Filter};
    let cat = catalog::load(&Config::default());
    let base = std::env::temp_dir().join(format!("poo-plan-admit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("app/target")).unwrap();
    std::fs::create_dir_all(base.join("wt/target")).unwrap();
    std::fs::write(base.join("wt/.git"), "gitdir: /nowhere\n").unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let p = |rel: &str| base.join(rel).to_string_lossy().to_string();
    let mut app = item(&p("app/target"), ActionKind::Delete);
    app.bytes = 5;
    app.total_bytes = 5;
    let mut wt_target = item(&p("wt/target"), ActionKind::Delete);
    wt_target.bytes = 100;
    wt_target.total_bytes = 100;
    let mut wt = item(&p("wt"), ActionKind::WorktreeRemove);
    wt.bucket = "worktrees".into();
    wt.rule = "git.worktrees".into();
    wt.bytes = 10;
    wt.total_bytes = 110;
    wt.risks.push(Risk::new("dirty", RiskLevel::Force, "uncommitted work"));
    // Gone since the scan: clean would refuse it, so the plan must too.
    let mut gone = item(&p("gone/target"), ActionKind::Delete);
    gone.bytes = 1000;
    gone.total_bytes = 1000;
    let report: Report = serde_json::from_value(serde_json::json!({
        "schema": 1, "version": "test", "scanned_at": 0, "duration_ms": 0, "disk": {"total": 0, "free": 0, "used": 0},
        "roots": [], "total_bytes": 0, "buckets": [], "plans": [], "projects": [], "items": [], "warnings": [],
    }))
    .unwrap();
    let report = Report { items: vec![app, wt_target, wt, gone], ..report };
    let stale = cat.plan("stale").unwrap();

    // Standard: the dirty worktree needs --include-in-use, so its build folder is cleaned alone.
    let r = evaluate(stale, &report, &Filter::default(), None, &cat, Admission::default());
    let mut paths: Vec<&str> = r.items.iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec![p("app/target"), p("wt/target")]);
    assert_eq!(r.bytes, 105);

    // In-use: the whole worktree goes, and its build folder is counted once inside it.
    let r = evaluate(stale, &report, &Filter::default(), None, &cat, Admission::mode(RiskLevel::Force));
    let mut paths: Vec<&str> = r.items.iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec![p("app/target"), p("wt")]);
    assert_eq!(r.bytes, 115);

    // Totals of a list count nested items once.
    let all: Vec<&Item> = report.items.iter().take(3).collect();
    assert_eq!(freed_total(&all), 115);
    std::fs::remove_dir_all(&base).unwrap();
}
