//! Running builds against the core rules: a build keeps what it may be using, and nothing
//! else. Processes are built from arguments; nothing real runs.

use poo::catalog::{self, Catalog};
use poo::config::Config;
use poo::procs::{self, Proc};
use std::path::Path;

fn proc_(name: &str, cwd: &str, args: &[&str]) -> Proc {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    Proc::with_args(1, name, Path::new(cwd), &args, &[])
}

fn touches(cat: &Catalog, p: &Proc, path: &str, rule: &str) -> bool {
    let r = cat.rule(rule).unwrap_or_else(|| panic!("no rule {}", rule));
    cat.build_touches(p, Path::new(path), Some(r), Some(Path::new("/r")))
}

#[test]
fn core_stack_tools_are_safe() {
    let cat = catalog::load(&Config::default());
    for s in &cat.stacks {
        for t in &s.tools {
            assert!(procs::check_stack_tool(t).is_ok(), "stack {} names {}", s.id, t);
        }
    }
    // Every built-in build tool is either scoped by a core stack or keeps its whole checkout.
    let claimed = cat.build_tool_names();
    for t in procs::BUILD_TOOLS {
        assert!(claimed.iter().any(|c| c == t) || procs::always_wide(t), "{} is neither claimed nor wide", t);
    }
    for t in procs::ALWAYS_WIDE {
        assert!(!cat.stacks.iter().any(|s| s.tools.iter().any(|x| x == t)), "a core stack claims wide tool {}", t);
    }
}

#[test]
fn builds_keep_only_what_they_use() {
    let cat = catalog::load(&Config::default());
    let xb = proc_("xcodebuild", "/r/apple", &["xcodebuild", "-scheme", "App"]);
    let xb_dd = proc_("xcodebuild", "/r/apple", &["xcodebuild", "-scheme", "App", "-derivedDataPath", "../.dd"]);
    let cargo = proc_("cargo", "/r", &["cargo", "build"]);
    let npm = proc_("node", "/r", &["node", "/usr/local/lib/node_modules/npm/bin/npm-cli.js", "install"]);
    let pod = proc_("ruby", "/r/ios", &["ruby", "/usr/local/bin/pod", "install"]);
    let make = proc_("make", "/r/native", &["make", "all"]);
    let swift_kit = proc_("swift-build", "/r/apple/Kit", &["swift-build"]);
    let sf = proc_("swift-frontend", "/r/apple", &["swift-frontend", "-c", "a.swift", "-o", "/dd/a.o"]);

    // A pinned build keeps only the DerivedData it names; an unpinned one keeps them all.
    assert!(touches(&cat, &xb_dd, "/r/.dd", "xcode.custom-derived-data"));
    assert!(!touches(&cat, &xb_dd, "/r/.dd-other", "xcode.custom-derived-data"));
    assert!(touches(&cat, &xb, "/r/.dd-other", "xcode.custom-derived-data"));
    // ...and nothing about where the project's own SYMROOT goes.
    assert!(touches(&cat, &xb_dd, "/r/apple/build", "xcode.legacy-build"));
    // Compilers name every output: a Swift compile into DerivedData leaves .build alone.
    assert!(!touches(&cat, &sf, "/r/apple/Kit/.build", "swift.build"));
    // SwiftPM packages are apart; Cargo keeps the workspace target.
    assert!(touches(&cat, &swift_kit, "/r/apple/Kit/.build", "swift.build"));
    assert!(!touches(&cat, &swift_kit, "/r/apple/UI/.build", "swift.build"));
    assert!(touches(&cat, &cargo, "/r/target", "rust.target"));

    // Another toolchain's output stays in use: Xcode may link a Rust staticlib.
    assert!(touches(&cat, &xb_dd, "/r/target", "rust.target"));
    assert!(touches(&cat, &sf, "/r/target", "rust.target"));
    assert!(touches(&cat, &npm, "/r/target", "rust.target"));
    // Dependencies stay in use for every build: Xcode run-script phases read node_modules.
    assert!(touches(&cat, &xb_dd, "/r/node_modules", "node.modules"));
    assert!(touches(&cat, &sf, "/r/node_modules", "node.modules"));
    assert!(touches(&cat, &pod, "/r/ios/Pods", "cocoapods.pods"));
    assert!(touches(&cat, &cargo, "/r/.venv", "python.venv"));
    // Orchestrators keep everything, wherever in the checkout they run.
    assert!(touches(&cat, &make, "/r/target", "rust.target"));
    assert!(touches(&cat, &make, "/r/apple/UI/.build", "swift.build"));
    // Rules without stacks (git-ignored folders) stay in use.
    assert!(touches(&cat, &xb_dd, "/r/.traces", "git.ignored"));
}

#[test]
fn interpreters_are_seen_as_builds() {
    let cat = catalog::load(&Config::default());
    let tools = cat.build_tool_names();
    for (name, args, tool) in [
        ("node", vec!["node", "/r/node_modules/vite/bin/vite.js", "build"], "vite"),
        ("node", vec!["node", "/usr/local/lib/node_modules/npm/bin/npm-cli.js", "ci"], "npm"),
        ("ruby", vec!["ruby", "/usr/local/bin/pod", "install"], "pod"),
        ("java", vec!["java", "org.gradle.wrapper.GradleWrapperMain", "build"], "gradle"),
        ("python3", vec!["python3", "-m", "pip", "install", "."], "pip"),
        ("php", vec!["php", "/usr/local/bin/composer", "install"], "composer"),
        ("arm-none-eabi-gcc", vec!["arm-none-eabi-gcc", "-c", "a.c", "-o", "build/a.o"], "gcc"),
    ] {
        let p = proc_(name, "/r", &args);
        assert_eq!(p.tool, tool);
        assert!(procs::is_build_tool(&p.tool, &tools), "{} is not a build tool", tool);
    }
    // A shell or a bare node is presence, not a build.
    for (name, args) in [("zsh", vec!["-zsh"]), ("node", vec!["node", "/r/server.js"])] {
        let p = proc_(name, "/r", &args);
        assert!(!procs::is_build_tool(&p.tool, &tools), "{:?}", args);
    }
}

#[test]
fn mods_and_config_cannot_weaken_build_checks() {
    let home = std::env::temp_dir().join(format!("poo-builds-{}", std::process::id()));
    let mods = home.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(
        mods.join("loosen.toml"),
        r#"
id = "loosen"

[[stack]]
id = "rust"
markers = ["Cargo.toml"]
tools = ["rust-foo"]
isolated = false

[[stack]]
id = "nothing"
markers = ["Nothing.toml"]
tools = ["nothing-tool"]

[[stack]]
id = "everything"
markers = ["Taskfile.yml"]
tools = ["make", "node", "zsh"]

# Point a core rule at another stack, and make dependencies look like build output.
[[rule]]
id = "rust.target"
stacks = ["nothing"]

[[rule]]
id = "node.modules"
bucket = "build"
stacks = ["nothing"]

[[guard]]
id = "core.build-processes"
kind = "process"
enabled = false
"#,
    )
    .unwrap();
    std::env::set_var("POO_CONFIG_DIR", &home);
    let mut cfg = Config::default();
    cfg.disabled_guards.push("core.build-processes".into());
    let cat = catalog::load(&cfg);
    std::env::remove_var("POO_CONFIG_DIR");
    std::fs::remove_dir_all(&home).ok();

    let rust = cat.stacks.iter().find(|s| s.id == "rust").unwrap();
    assert!(rust.tools.iter().any(|t| t == "cargo"), "override dropped cargo: {:?}", rust.tools);
    let every = cat.stacks.iter().find(|s| s.id == "everything").unwrap();
    assert!(every.tools.is_empty(), "{:?}", every.tools);
    assert_eq!(cat.errors.iter().filter(|e| e.contains("everything")).count(), 3, "{:?}", cat.errors);

    // The changed rules are no longer scoped: every build near them keeps them.
    let cargo = proc_("cargo", "/r", &["cargo", "build"]);
    let xb = proc_("xcodebuild", "/r/ios", &["xcodebuild", "-derivedDataPath", "/dd"]);
    let npm = proc_("node", "/r", &["node", "/usr/local/lib/node_modules/npm/bin/npm-cli.js", "install"]);
    assert!(touches(&cat, &cargo, "/r/target", "rust.target"));
    assert!(touches(&cat, &xb, "/r/target", "rust.target"));
    assert!(touches(&cat, &npm, "/r/node_modules", "node.modules"));
    assert!(touches(&cat, &xb, "/r/node_modules", "node.modules"));
    // The isolated flag stays the core's: SwiftPM siblings, still apart; nothing loosened.
    let make = proc_("make", "/r", &["make"]);
    assert!(touches(&cat, &make, "/r/target", "rust.target"));
}
