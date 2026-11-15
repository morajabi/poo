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
    assert!(admit(&it, &cat, &opts(RiskLevel::Standard), &[]).unwrap_err().contains("--include-in-use"));
    let warnings = admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap();
    assert!(warnings.iter().any(|w| w.contains("recent")));
    it.risks.push(Risk::new("data", RiskLevel::Danger, "real data"));
    assert!(admit(&it, &cat, &opts(RiskLevel::Force), &[]).unwrap_err().contains("--dangerously-include-protected"));
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
    assert!(m("elixir.deps", "deps", &["mix.exs"], &["phoenix", "jason"]));
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
