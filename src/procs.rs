//! Running processes, used to keep poo away from builds in flight.
//!
//! Every process carries its name and working directory. Build tools (and interpreters that
//! may be running one: node, python, ruby, java, shells) also carry what their arguments say:
//! the tool really running (`node .../vite.js` is vite), the folders it works on (cwd, `-C`,
//! `--manifest-path`...), every path it names, and whether it names where it writes.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// What a build's arguments say about where it writes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Pin {
    /// Unknown: it may write anywhere near where it works.
    #[default]
    None,
    /// A compiler or linker named every output (`-o`, `-output-file-map`).
    All,
    /// An orchestrator named where items of these rules go (`-derivedDataPath`,
    /// `--target-dir`). Other outputs, set by the project itself, are still unknown.
    Rules(&'static [&'static str]),
}

#[derive(Debug, Clone, Default)]
pub struct Proc {
    pub pid: i32,
    /// The executable's name, as macOS or /proc reports it.
    pub name: String,
    pub cwd: PathBuf,
    /// The tool this process runs: its name, or for an interpreter the script or main class
    /// it runs (`node .../npm-cli.js` is npm, `java ...GradleDaemon` is gradle).
    pub tool: String,
    /// Folders the tool works on: cwd plus the ones its flags point at (`make -C x`,
    /// `cargo --manifest-path x/Cargo.toml`, `xcodebuild -project x/A.xcodeproj`).
    pub dirs: Vec<PathBuf>,
    /// Paths named in the arguments (and response files), resolved against cwd.
    pub paths: BTreeSet<PathBuf>,
    /// Where the tool said it writes.
    pub pin: Pin,
    /// The arguments (and any response files) were read. Without them poo knows only the
    /// name and cwd, and treats the build as using its whole checkout.
    pub args_known: bool,
}

impl Proc {
    /// A process with nothing but a name and cwd, as when its arguments cannot be read.
    pub fn bare(pid: i32, name: &str, cwd: &Path) -> Proc {
        Proc { pid, name: name.into(), cwd: cwd.into(), tool: resolve_tool(name, &[]), dirs: vec![cwd.into()], ..Default::default() }
    }

    /// Build a process from its arguments (argv[0] included) and the env vars that matter.
    pub fn with_args(pid: i32, name: &str, cwd: &Path, args: &[String], env: &[(String, String)]) -> Proc {
        let tool = resolve_tool(name, args);
        let mut p = Proc::bare(pid, name, cwd);
        let a = parse_args(&tool, args, cwd);
        p.paths = a.paths;
        p.pin = a.pin;
        p.args_known = !args.is_empty() && a.complete;
        for d in a.dirs {
            if !p.dirs.contains(&d) {
                p.dirs.push(d);
            }
        }
        if matches!(tool.as_str(), "cargo" | "cargo-nextest") {
            for (k, v) in env {
                if (k == "CARGO_TARGET_DIR" || k == "CARGO_BUILD_TARGET_DIR") && !v.is_empty() {
                    add_path(&mut p.paths, v, cwd, true);
                    p.pin = Pin::Rules(CARGO_PIN);
                }
            }
        }
        p.tool = tool;
        p
    }

    /// Whether any folder this process works on is inside `path`.
    pub fn works_in(&self, path: &Path) -> bool {
        self.dirs.iter().any(|d| d.starts_with(path))
    }

    /// Whether the arguments name a path inside `path`.
    pub fn names(&self, path: &Path) -> bool {
        self.paths.range(path.to_path_buf()..).next().map_or(false, |x| x.starts_with(path))
    }

    /// Whether the tool said where items of `rule` go, so one it did not name is not its own.
    pub fn pins(&self, rule: &str) -> bool {
        match self.pin {
            Pin::None => false,
            Pin::All => true,
            Pin::Rules(r) => r.contains(&rule),
        }
    }
}

/// Build tools: a project where one of these runs is hot, and cleaning it would break a build.
/// Stacks in rules can name more (`[[stack]] tools`).
pub const BUILD_TOOLS: &[&str] = &[
    "xcodebuild", "swift-build", "swift-frontend", "swift-test", "swift-driver", "swiftc",
    "cargo", "rustc", "clippy-driver", "cargo-nextest", "clang", "clang++", "cc", "c++", "ld",
    "ld64", "lld", "gradlew", "mvn", "make", "gmake", "ninja", "cmake", "bazel", "buck2", "zig",
    "tsc", "esbuild", "webpack", "rollup", "vite", "turbo", "nx", "next-server", "pod",
    "carthage", "flutter", "dart", "dotnet", "msbuild", "mix", "pip", "uv", "poetry", "go",
    "xcrun", "ibtool", "actool", "codesign", "lipo", "docker-buildx", "pnpm", "yarn", "npm",
    // Usually run by an interpreter; poo finds them in its arguments.
    "gradle", "sbt", "fastlane", "maturin", "meson", "bun-install", "bun-build",
    "gcc", "g++", "ar", "dsymutil", "libtool", "swift-run", "swift-package",
];

/// Build tools that run anything (make targets, task graphs, lanes): they keep their whole
/// checkout in use. Code, not rules, so no mod can scope them.
pub const ALWAYS_WIDE: &[&str] = &[
    "make", "gmake", "ninja", "bazel", "buck2", "turbo", "nx", "docker-buildx", "fastlane",
    "maturin", "meson", "ar",
];

pub fn always_wide(tool: &str) -> bool {
    ALWAYS_WIDE.contains(&tool)
}

/// Whether a stack may name `tool`: never an interpreter, shell, agent or editor (that would
/// count every node or zsh process as a build), nor a tool that must stay checkout-wide.
pub fn check_stack_tool(tool: &str) -> Result<(), String> {
    if tool.is_empty() || is_interpreter(tool) || is_presence(tool) {
        return Err(format!("`{}` is an interpreter, shell or agent, not a build tool", tool));
    }
    if always_wide(tool) {
        return Err(format!("`{}` runs any task, so it always keeps its whole checkout", tool));
    }
    Ok(())
}

/// Agents, editors, shells and dev servers: a project where one runs is in use (warm), but
/// nothing is being built, so a forced clean only costs a rebuild.
pub const PRESENCE: &[&str] = &[
    "claude", "codex", "opencode", "cursor-agent", "gemini", "aider", "amp", "goose", "copilot",
    "node", "bun", "deno", "python", "python3", "ruby", "java", "zsh", "bash", "fish", "sh", "nu",
    "nvim", "vim", "emacs", "hx", "tmux", "rust-analyzer", "sourcekit-lsp", "gopls",
];

/// Interpreters and launchers whose arguments say which tool they run.
const INTERPRETERS: &[&str] = &[
    "node", "bun", "deno", "ruby", "java", "php", "perl", "beam.smp", "sh", "bash", "zsh", "dash",
];

fn is_interpreter(name: &str) -> bool {
    INTERPRETERS.contains(&name) || name.to_ascii_lowercase().starts_with("python")
}

/// Compilers and linkers: they read only what their arguments name and run no scripts, so
/// they never touch a dependency folder they do not name.
const LEAF_TOOLS: &[&str] = &[
    "swift-frontend", "swiftc", "swift-driver", "clang", "clang++", "cc", "c++", "gcc", "g++",
    "ld", "ld64", "lld", "ar", "dsymutil", "libtool", "rustc", "clippy-driver", "ibtool",
    "actool", "codesign", "lipo",
];

pub fn is_leaf(tool: &str) -> bool {
    LEAF_TOOLS.contains(&tool)
}

const CARGO_PIN: &[&str] = &["rust.target", "rust.target-custom"];
const DERIVED_DATA_PIN: &[&str] = &["xcode.custom-derived-data", "xcode.derived-data"];
const SWIFTPM_PIN: &[&str] = &["swift.build"];

/// Per tool, the flags that say where it writes, and what that covers. `-o` names every
/// output of a compiler but means something else to make or uv; `-derivedDataPath` says
/// where DerivedData goes, not where a project's own SYMROOT points.
const PIN_FLAGS: &[(&[&str], &[&str], Pin)] = &[
    (&["swift-frontend", "swiftc", "swift-driver"], &["-o", "-output-file-map", "-emit-module-path"], Pin::All),
    (&["clang", "clang++", "cc", "c++", "gcc", "g++", "ld", "ld64", "lld", "dsymutil", "libtool"], &["-o"], Pin::All),
    (&["rustc", "clippy-driver"], &["--out-dir", "-o"], Pin::All),
    (&["xcodebuild"], &["-derivedDataPath"], Pin::Rules(DERIVED_DATA_PIN)),
    (&["cargo", "cargo-nextest"], &["--target-dir"], Pin::Rules(CARGO_PIN)),
    (&["swift-build", "swift-test", "swift-run", "swift-package"], &["--scratch-path", "--build-path"], Pin::Rules(SWIFTPM_PIN)),
];

/// Per tool, flags whose value is a folder it works on (`Dir`) or a file in one (`Parent`).
#[derive(Clone, Copy, PartialEq)]
enum DirKind {
    Dir,
    Parent,
}
const DIR_FLAGS: &[(&[&str], &str, DirKind)] = &[
    (&["make", "gmake", "ninja", "cargo", "meson", "go", "pnpm"], "-C", DirKind::Dir),
    (&["make", "gmake"], "--directory", DirKind::Dir),
    (&["cargo", "cargo-nextest"], "--manifest-path", DirKind::Parent),
    (&["swift-build", "swift-test", "swift-run", "swift-package"], "--package-path", DirKind::Dir),
    (&["xcodebuild"], "-project", DirKind::Parent),
    (&["xcodebuild"], "-workspace", DirKind::Parent),
    (&["gradle", "gradlew"], "-p", DirKind::Dir),
    (&["gradle", "gradlew"], "--project-dir", DirKind::Dir),
    (&["mvn"], "-f", DirKind::Parent),
    (&["mvn"], "--file", DirKind::Parent),
    (&["npm"], "--prefix", DirKind::Dir),
    (&["pnpm"], "--dir", DirKind::Dir),
    (&["yarn", "bun-install", "bun-build"], "--cwd", DirKind::Dir),
    (&["tsc"], "-p", DirKind::Parent),
    (&["tsc"], "--project", DirKind::Parent),
    (&["bazel"], "--workspace_directory", DirKind::Dir),
];

/// Flags whose value is a path even without a slash in it (`-derivedDataPath .dd`).
const PATH_FLAGS: &[&str] = &[
    "-o", "-output-file-map", "-emit-module-path", "-derivedDataPath", "--target-dir",
    "--scratch-path", "--build-path", "--out-dir", "--outDir", "--outdir", "-module-cache-path",
    "-I", "-F", "-L", "-archivePath", "-exportPath", "-resultBundlePath",
];

/// Script names too generic to name a tool: use the package they live in instead.
const GENERIC_SCRIPTS: &[&str] = &["cli", "index", "main", "bin", "run", "start", "entry", "bootstrap", "launcher"];

/// Cross and versioned compilers are the compiler: `arm-none-eabi-gcc` is gcc, `clang-18`
/// clang.
fn native_tool(name: &str) -> String {
    let base = match name.rsplit_once('-') {
        Some((h, v)) if !h.is_empty() && !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') => h,
        _ => name,
    };
    for t in ["gcc", "g++", "clang", "clang++", "cc", "c++", "ld", "ar"] {
        if base.len() > t.len() && base.ends_with(t) && base[..base.len() - t.len()].ends_with('-') {
            return t.to_string();
        }
    }
    base.to_string()
}

/// The tool a process runs: an interpreter's script or main class from its arguments, or
/// for other processes their own (normalized) name.
pub fn resolve_tool(name: &str, args: &[String]) -> String {
    if !is_interpreter(name) {
        return native_tool(name);
    }
    let rest: &[String] = if args.is_empty() { args } else { &args[1..] };
    let lower = name.to_ascii_lowercase();
    let found = match lower.as_str() {
        "java" => java_tool(rest),
        "beam.smp" => rest.iter().skip_while(|a| *a != "-extra").nth(1).map(|s| script_tool(s)),
        n if n.starts_with("python") => python_tool(rest),
        "bun" => bun_tool(rest),
        _ => {
            // node, deno, ruby, php, perl, shells: the first argument that is not a flag.
            // `sh -c "..."` runs a command line whose tools show up as their own processes.
            if rest.first().map_or(false, |a| a == "-c") {
                None
            } else {
                let skip_value = |a: &str| matches!(a, "-r" | "--require" | "--import" | "--loader" | "-e" | "--eval");
                let mut it = rest.iter();
                let mut script = None;
                let mut prev_flag = false;
                while let Some(a) = it.next() {
                    if skip_value(a) {
                        it.next();
                        prev_flag = false;
                    } else if a.starts_with('-') {
                        prev_flag = !a.contains('=');
                    } else if prev_flag && flag_value(a) {
                        // `--conditions development`, `--inspect-port 9229`
                        prev_flag = false;
                    } else {
                        if lower == "deno" && matches!(a.as_str(), "run" | "task") {
                            continue;
                        }
                        script = Some(a);
                        break;
                    }
                }
                script.map(|s| script_tool(s))
            }
        }
    };
    found.filter(|t| !t.is_empty()).unwrap_or_else(|| name.to_string())
}

/// `.../node_modules/vite/bin/vite.js` is vite, `.../npm/bin/npm-cli.js` npm,
/// `.yarn/releases/yarn-4.1.0.cjs` yarn, `.../@rspack/cli/bin/cli.js` rspack, `./gradlew` gradlew.
fn script_tool(script: &str) -> String {
    let p = Path::new(script);
    let mut stem = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for ext in [".js", ".cjs", ".mjs", ".ts", ".rb", ".py", ".php", ".pl", ".sh", ".snapshot"] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s.to_string();
        }
    }
    // flutter_tools.snapshot is the flutter tool.
    if stem == "flutter_tools" {
        return "flutter".into();
    }
    // yarn-4.1.0, pip3.12
    if let Some(i) = stem.find(|c: char| c.is_ascii_digit()) {
        let head = stem[..i].trim_end_matches(['-', '.', '_']);
        if !head.is_empty() && stem[i..].chars().all(|c| c.is_ascii_digit() || c == '.') {
            stem = head.to_string();
        }
    }
    if GENERIC_SCRIPTS.contains(&stem.as_str()) || stem.is_empty() {
        let comps: Vec<String> = p.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        if let Some(i) = comps.iter().rposition(|c| c == "node_modules") {
            let pkg = comps.get(i + 1).cloned().unwrap_or_default();
            stem = match pkg.strip_prefix('@') {
                Some(scope) => {
                    let name = comps.get(i + 2).cloned().unwrap_or_default();
                    if GENERIC_SCRIPTS.contains(&name.as_str()) { scope.to_string() } else { name }
                }
                None => pkg,
            };
        }
    }
    let stem = stem.strip_suffix("-cli").map(str::to_string).unwrap_or(stem);
    match stem.as_str() {
        "pip3" => "pip".into(),
        "mvnw" => "mvn".into(),
        "bundler" => "bundle".into(),
        _ => stem,
    }
}

/// A word after a flag that names no file (no `/`, no `.`) is that flag's value.
fn flag_value(a: &str) -> bool {
    !a.contains('/') && !a.contains('.')
}

fn python_tool(rest: &[String]) -> Option<String> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "-m" {
            return it.next().map(|m| script_tool(m.split('.').next().unwrap_or(m)));
        }
        if a == "-c" {
            return None;
        }
        if matches!(a.as_str(), "-X" | "-W" | "-Q") {
            it.next();
            continue;
        }
        if !a.starts_with('-') {
            return Some(script_tool(a));
        }
    }
    None
}

fn bun_tool(rest: &[String]) -> Option<String> {
    let first = rest.iter().find(|a| !a.starts_with('-'))?;
    Some(match first.as_str() {
        "install" | "i" | "add" | "a" | "remove" | "rm" | "update" | "upgrade" | "link" | "pm" => "bun-install".into(),
        "build" => "bun-build".into(),
        "run" | "x" | "exec" => rest.iter().filter(|a| !a.starts_with('-')).nth(1).map(|s| script_tool(s))?,
        s => script_tool(s),
    })
}

fn java_tool(rest: &[String]) -> Option<String> {
    let has = |needle: &str| rest.iter().any(|a| a.contains(needle));
    if has("org.gradle.") || has("gradle-wrapper.jar") || has("gradle-launcher") {
        Some("gradle".into())
    } else if has("org.codehaus.plexus.classworlds") || has("org.apache.maven.") {
        Some("mvn".into())
    } else if has("xsbt.boot.Boot") || has("sbt-launch") {
        Some("sbt".into())
    } else if has("com.google.devtools.build.lib.bazel") {
        Some("bazel".into())
    } else {
        None
    }
}

/// What a tool's arguments say.
#[derive(Default)]
pub struct Args {
    pub paths: BTreeSet<PathBuf>,
    pub dirs: Vec<PathBuf>,
    pub pin: Pin,
    /// Every response file could be read.
    pub complete: bool,
}

/// `a/../b` and `./` removed without touching the disk.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

fn resolve(v: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(v);
    normalize(&if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) })
}

/// Split a response file the way a shell would: whitespace separates, quotes group,
/// backslash escapes (`"/r/My App/Pods"` is one path).
fn split_response(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                started = true;
            }
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Paths, folders and pinning from `tool`'s arguments (argv[0] first). Compilers' `@file`
/// response files are read too; every other argument of a compiler counts as a path, since
/// it reads only what it is given (`-isystem vendor`, `main.c`).
pub fn parse_args(tool: &str, args: &[String], cwd: &Path) -> Args {
    let leaf = is_leaf(tool);
    let mut out = Args { complete: true, ..Default::default() };
    let mut tokens: Vec<String> = vec![];
    for a in args.iter().skip(1) {
        // `@executable_path/..` in a linker's -rpath is a path, not a response file.
        let dyld = ["@executable_path", "@loader_path", "@rpath"].iter().any(|p| a.starts_with(p));
        if let Some(f) = a.strip_prefix('@').filter(|_| leaf && !dyld) {
            let f = cwd.join(f);
            match std::fs::metadata(&f).ok().filter(|m| m.is_file() && m.len() < 8 << 20).and_then(|_| std::fs::read_to_string(&f).ok()) {
                Some(s) => {
                    let inner = split_response(&s);
                    // Nested response files are not followed: what they name stays unknown.
                    if inner.iter().any(|t| t.starts_with('@') && !["@executable_path", "@loader_path", "@rpath"].iter().any(|p| t.starts_with(p))) {
                        out.complete = false;
                    }
                    tokens.extend(inner);
                }
                None => out.complete = false,
            }
            continue;
        }
        tokens.push(a.clone());
    }
    let pin = PIN_FLAGS.iter().find(|(t, _, _)| t.contains(&tool));
    let pins = |f: &str| pin.map_or(false, |(_, flags, _)| flags.contains(&f));
    let dir_flag = |f: &str| DIR_FLAGS.iter().find(|(t, fl, _)| *fl == f && t.contains(&tool)).map(|x| x.2);
    let add_dir = |out: &mut Args, v: &str, kind: DirKind| {
        let p = resolve(v, cwd);
        let looks_file = p.extension().is_some() && !v.ends_with('/');
        let d = if kind == DirKind::Parent && looks_file { p.parent().map(Path::to_path_buf).unwrap_or(p.clone()) } else { p.clone() };
        add_path(&mut out.paths, v, cwd, true);
        if !out.dirs.contains(&d) {
            out.dirs.push(d);
        }
    };
    let mut pinned = false;
    let mut pending: Option<String> = None;
    for t in &tokens {
        if let Some(f) = pending.take() {
            pinned |= pins(&f);
            match dir_flag(&f) {
                Some(kind) => add_dir(&mut out, t, kind),
                None => add_path(&mut out.paths, t, cwd, true),
            }
            continue;
        }
        if t.starts_with('-') {
            if let Some((k, v)) = t.split_once('=') {
                pinned |= pins(k);
                match dir_flag(k) {
                    Some(kind) => add_dir(&mut out, v, kind),
                    None => add_path(&mut out.paths, v, cwd, leaf || PATH_FLAGS.contains(&k)),
                }
            } else if PATH_FLAGS.contains(&t.as_str()) || pins(t) || dir_flag(t).is_some() {
                pending = Some(t.clone());
            } else if let Some(v) = ["-I", "-F", "-L"].iter().find_map(|f| t.strip_prefix(f)).filter(|v| !v.is_empty()) {
                add_path(&mut out.paths, v, cwd, true);
            }
            continue;
        }
        // Build settings: `SYMROOT=build`, `CONFIGURATION_BUILD_DIR=out`.
        if let Some((k, v)) = t.split_once('=') {
            let setting = !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if setting && ["ROOT", "DIR", "PATH", "LOCATION"].iter().any(|e| k.ends_with(e)) {
                add_path(&mut out.paths, v, cwd, true);
                continue;
            }
        }
        add_path(&mut out.paths, t, cwd, leaf);
    }
    if pinned {
        out.pin = pin.map_or(Pin::None, |p| p.2);
    }
    out
}

fn add_path(paths: &mut BTreeSet<PathBuf>, v: &str, cwd: &Path, is_path: bool) {
    if v.is_empty() || !(is_path || v.contains('/')) || v.contains("://") {
        return;
    }
    // rustc's `-L dependency=/abs/path`, xcodebuild's `SYMROOT=build`.
    if let Some((k, rest)) = v.split_once('=') {
        if !k.is_empty() && !k.contains('/') && !rest.is_empty() {
            paths.insert(resolve(rest, cwd));
            return;
        }
    }
    paths.insert(resolve(v, cwd));
}

/// What the built-in rules say about an item's rule, for scoping builds near it. Only rules
/// and stacks from the core mod count; a mod or config can never narrow it.
pub struct Scope<'a> {
    pub rule: &'a str,
    /// Tools of the rule's stacks: the toolchain that writes this output.
    pub tools: Vec<&'a str>,
    /// Packages of these stacks never read each other's output (SwiftPM).
    pub isolated: bool,
}

/// Whether build process `p` may be writing or reading `item`. When unsure, it is.
///
/// 1. It works inside the item, or names a path inside it: yes.
/// 2. The item is not in a checkout `p` works in (`checkout` None): no.
/// 3. Everything else in that checkout stays in use unless poo knows better. It knows better
///    only for build output (`scope` is Some) of the tool's own toolchain, when it read the
///    tool's arguments, and the tool is not one that runs anything (make, turbo):
///    - a tool that said where it writes this kind of output (`-derivedDataPath`,
///      `--target-dir`, a compiler's `-o`) does not use another copy of it;
///    - in stacks whose packages stay apart (SwiftPM), a build uses only items at or
///      above its parent folder: `swift build` in one package leaves its sibling's `.build`.
///
/// Another toolchain's output stays in use (Xcode links a Rust staticlib from target/,
/// `go:embed` reads web/dist), and so do dependencies and caches (Xcode run-script phases
/// and Gradle read node_modules in React Native apps).
pub fn build_touches(p: &Proc, item: &Path, checkout: Option<&Path>, scope: Option<&Scope>) -> bool {
    if p.works_in(item) || p.names(item) {
        return true;
    }
    let Some(k) = checkout else { return false };
    let Some(s) = scope else { return true };
    if !p.args_known || always_wide(&p.tool) || !s.tools.contains(&p.tool.as_str()) {
        return true;
    }
    if p.pins(s.rule) {
        return false;
    }
    if s.isolated {
        return near(p, item, k);
    }
    true
}

fn near(p: &Proc, item: &Path, checkout: &Path) -> bool {
    if !item.starts_with(checkout) {
        // Outside the checkout but attributed to it: no folders to compare.
        return true;
    }
    let parent = item.parent().unwrap_or(item);
    p.dirs.iter().filter(|d| d.starts_with(checkout)).any(|d| d.starts_with(parent) || item.starts_with(d))
}

/// Running processes, or nothing when they cannot be listed. Cleaning uses `try_list`.
pub fn list() -> Vec<Proc> {
    try_list().unwrap_or_default()
}

/// Whether to read a process's arguments: build tools and interpreters that may run one.
fn wants_args(name: &str) -> bool {
    is_build_tool(&native_tool(name), &[]) || is_interpreter(name)
}

/// Running processes, or None when the list could not be read (then poo cannot tell
/// whether a build is running, and skips).
#[cfg(target_os = "macos")]
pub fn try_list() -> Option<Vec<Proc>> {
    use std::ffi::CStr;
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    const VNODE_INFO_SIZE: usize = 152;
    const PATH_MAX: usize = 1024;
    let mut pids = vec![0i32; 8192];
    let n = unsafe {
        libc::proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, (pids.len() * 4) as i32)
    };
    if n <= 0 {
        return None;
    }
    pids.truncate(n as usize);
    let mut out = vec![];
    let mut buf = vec![0u8; (VNODE_INFO_SIZE + PATH_MAX) * 2];
    for pid in pids {
        if pid <= 0 {
            continue;
        }
        let r = unsafe {
            libc::proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as i32)
        };
        if r <= 0 {
            continue;
        }
        let path_bytes = &buf[VNODE_INFO_SIZE..VNODE_INFO_SIZE + PATH_MAX];
        let Ok(cwd) = CStr::from_bytes_until_nul(path_bytes) else { continue };
        // GUI apps run with cwd "/": keep them, running_apps checks need their names.
        let cwd = cwd.to_string_lossy().to_string();
        if cwd.is_empty() {
            continue;
        }
        let mut name_buf = [0u8; 256];
        let len = unsafe { libc::proc_name(pid, name_buf.as_mut_ptr() as *mut libc::c_void, 256) };
        let name = if len > 0 {
            String::from_utf8_lossy(&name_buf[..len as usize]).to_string()
        } else {
            String::new()
        };
        let cwd = PathBuf::from(cwd);
        let p = match wants_args(&name).then(|| args_of(pid)).flatten() {
            Some((args, env)) => Proc::with_args(pid, &name, &cwd, &args, &env),
            None => Proc::bare(pid, &name, &cwd),
        };
        out.push(p);
    }
    Some(out)
}

/// Env vars worth reading from a build process.
const ENV_KEYS: &[&str] = &["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"];

/// A process's arguments (argv[0] first) and the env vars in ENV_KEYS, or None when they
/// cannot be read.
#[cfg(target_os = "macos")]
fn args_of(pid: i32) -> Option<(Vec<String>, Vec<(String, String)>)> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 || size < 4 {
        return None;
    }
    buf.truncate(size);
    // Layout: argc, the exec path, NUL padding, argc NUL-terminated arguments, then env.
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let rest = &buf[4..];
    let end = rest.iter().position(|b| *b == 0)?;
    let rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0)?;
    let mut parts = rest[start..].split(|b| *b == 0);
    let args: Vec<String> = parts.by_ref().take(argc).map(|s| String::from_utf8_lossy(s).to_string()).collect();
    // A short list (truncated buffer, an empty argv[0]) is not the real one.
    if args.len() < argc {
        return None;
    }
    let env = parts.take_while(|s| !s.is_empty()).filter_map(|s| env_pair(&String::from_utf8_lossy(s))).collect();
    Some((args, env))
}

fn env_pair(kv: &str) -> Option<(String, String)> {
    let (k, v) = kv.split_once('=')?;
    ENV_KEYS.contains(&k).then(|| (k.to_string(), v.to_string()))
}

#[cfg(not(target_os = "macos"))]
pub fn try_list() -> Option<Vec<Proc>> {
    let mut out = vec![];
    let rd = std::fs::read_dir("/proc").ok()?;
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(cwd) = std::fs::read_link(e.path().join("cwd")) else { continue };
        let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string();
        let split = |raw: Vec<u8>| -> Vec<String> {
            raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).to_string()).collect()
        };
        let p = match std::fs::read(e.path().join("cmdline")).ok().filter(|_| wants_args(&name)) {
            Some(raw) => {
                let args = split(raw);
                let env: Vec<(String, String)> = std::fs::read(e.path().join("environ"))
                    .map(|raw| split(raw).iter().filter_map(|kv| env_pair(kv)).collect())
                    .unwrap_or_default();
                Proc::with_args(pid, &name, &cwd, &args, &env)
            }
            None => Proc::bare(pid, &name, &cwd),
        };
        out.push(p);
    }
    Some(out)
}

pub fn is_build_tool(name: &str, extra: &[String]) -> bool {
    BUILD_TOOLS.iter().any(|t| *t == name) || extra.iter().any(|t| t == name)
}

pub fn is_presence(name: &str) -> bool {
    PRESENCE.iter().any(|t| *t == name)
}

/// Build processes working inside `path` or naming a path inside it.
pub fn builds_inside<'a>(procs: &'a [Proc], path: &std::path::Path, extra: &[String]) -> Vec<&'a Proc> {
    procs
        .iter()
        .filter(|p| (p.works_in(path) || p.names(path)) && is_build_tool(&p.tool, extra))
        .collect()
}

/// Names of agents, editors and shells working inside `path`.
pub fn presence_inside(procs: &[Proc], path: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = procs
        .iter()
        .filter(|p| p.cwd.starts_with(path) && is_presence(&p.name))
        .map(|p| p.name.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_(name: &str, cwd: &str, args: &[&str]) -> Proc {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Proc::with_args(1, name, Path::new(cwd), &args, &[])
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    const K: &str = "/r";
    fn touches(p: &Proc, item: &str, scope: Option<&Scope>) -> bool {
        build_touches(p, Path::new(item), Some(Path::new(K)), scope)
    }
    fn rust() -> Scope<'static> {
        Scope { rule: "rust.target", tools: vec!["cargo", "rustc"], isolated: false }
    }
    fn swift() -> Scope<'static> {
        Scope { rule: "swift.build", tools: vec!["swift-build", "swift-frontend"], isolated: true }
    }
    fn dd() -> Scope<'static> {
        Scope { rule: "xcode.custom-derived-data", tools: vec!["xcodebuild", "swift-frontend"], isolated: false }
    }
    fn legacy() -> Scope<'static> {
        Scope { rule: "xcode.legacy-build", tools: vec!["xcodebuild", "swift-frontend"], isolated: false }
    }
    fn dist() -> Scope<'static> {
        Scope { rule: "node.dist", tools: vec!["npm", "vite", "tsc"], isolated: false }
    }

    #[test]
    fn args_resolve_paths_and_pin_by_tool() {
        let p = proc_("xcodebuild", "/r/apple", &["xcodebuild", "-project", "Inline.xcodeproj", "-derivedDataPath", "../.dd", "build"]);
        assert_eq!(p.pin, Pin::Rules(DERIVED_DATA_PIN));
        assert!(p.paths.contains(Path::new("/r/.dd")));
        assert!(p.args_known);
        let p = proc_("rustc", "/r", &["rustc", "-L", "dependency=/r/target/debug/deps", "--out-dir=/r/target/debug/deps"]);
        assert_eq!(p.pin, Pin::All);
        assert!(p.paths.contains(Path::new("/r/target/debug/deps")));
        let p = proc_("cargo", "/r", &["cargo", "build"]);
        assert_eq!(p.pin, Pin::None);
        assert!(p.paths.is_empty());
        // `-o` pins compilers, not make or uv.
        assert_eq!(proc_("make", "/r", &["make", "-o", "/tmp/x"]).pin, Pin::None);
        assert_eq!(proc_("uv", "/r", &["uv", "pip", "compile", "-o", "req.txt"]).pin, Pin::None);
        assert_eq!(proc_("clang", "/r", &["clang", "-c", "a.c", "-o", "build/a.o"]).pin, Pin::All);
        // Build settings on the command line, relative or absolute.
        let p = proc_("xcodebuild", "/r/apple", &["xcodebuild", "SYMROOT=build", "OBJROOT=/r/obj"]);
        assert!(p.names(Path::new("/r/apple/build")) && p.names(Path::new("/r/obj")));
        // Compilers' arguments are all paths: `-isystem vendor`.
        let p = proc_("clang", "/r", &["clang", "-isystem", "vendor", "-c", "a.c", "-o", "a.o"]);
        assert!(p.names(Path::new("/r/vendor")));
    }

    #[test]
    fn env_target_dir_pins_cargo() {
        let env = vec![("CARGO_TARGET_DIR".to_string(), "/shared/target".to_string())];
        let p = Proc::with_args(1, "cargo", Path::new("/r"), &s(&["cargo", "build"]), &env);
        assert!(p.pins("rust.target") && p.names(Path::new("/shared/target")));
        assert!(!touches(&p, "/r/target", Some(&rust())));
    }

    #[test]
    fn response_files_are_read_like_a_shell() {
        let dir = std::env::temp_dir().join(format!("poo-procs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rsp = dir.join("args.resp");
        std::fs::write(&rsp, "-c \"/r/My App/a.swift\"\n-I '/r/My App/ios/Pods/Headers'\n-output-file-map /dd/Build/x.json\n").unwrap();
        let p = proc_("swift-frontend", "/r", &["swift-frontend", &format!("@{}", rsp.display())]);
        assert_eq!(p.pin, Pin::All);
        assert!(p.args_known);
        assert!(p.names(Path::new("/r/My App/ios/Pods")));
        assert!(p.paths.contains(Path::new("/dd/Build/x.json")));
        // A response file that cannot be read leaves the arguments unknown.
        let p = proc_("swift-frontend", "/r", &["swift-frontend", "@/nonexistent/args.resp"]);
        assert!(!p.args_known);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(split_response(r#"a "b c" 'd e' f\ g"#), s(&["a", "b c", "d e", "f g"]));
        // Linker rpaths are not response files.
        let p = proc_("ld", "/r", &["ld", "-rpath", "@executable_path/../Frameworks", "-o", "/dd/App"]);
        assert!(p.args_known && p.pin == Pin::All);
    }

    #[test]
    fn interpreters_resolve_to_the_tool_they_run() {
        let t = |name: &str, args: &[&str]| resolve_tool(name, &s(args));
        assert_eq!(t("node", &["node", "/usr/local/lib/node_modules/npm/bin/npm-cli.js", "install"]), "npm");
        assert_eq!(t("node", &["node", "--max-old-space-size=4096", "/r/node_modules/vite/bin/vite.js", "build"]), "vite");
        assert_eq!(t("node", &["node", "--conditions", "development", "/r/node_modules/vite/bin/vite.js"]), "vite");
        assert_eq!(t("node", &["node", "--inspect-port", "9229", "/r/node_modules/.bin/tsc"]), "tsc");
        assert_eq!(t("node", &["node", "/r/node_modules/webpack-cli/bin/cli.js"]), "webpack");
        assert_eq!(t("node", &["node", "/r/node_modules/@rspack/cli/bin/cli.js"]), "rspack");
        assert_eq!(t("node", &["node", "/r/.yarn/releases/yarn-4.1.0.cjs", "install"]), "yarn");
        assert_eq!(t("node", &["node", "-r", "ts-node/register", "/r/node_modules/next/dist/bin/next", "build"]), "next");
        assert_eq!(t("ruby", &["ruby", "/usr/local/bin/pod", "install"]), "pod");
        assert_eq!(t("python3", &["python3", "-m", "pip", "install", "-e", "."]), "pip");
        assert_eq!(t("python3", &["python3", "-X", "utf8", "/r/.venv/bin/poetry", "install"]), "poetry");
        assert_eq!(t("Python", &["/usr/bin/python3", "/r/.venv/bin/poetry", "install"]), "poetry");
        assert_eq!(t("java", &["java", "-cp", "/x/gradle-launcher-8.jar", "org.gradle.launcher.daemon.bootstrap.GradleDaemon"]), "gradle");
        assert_eq!(t("java", &["java", "-classpath", "/m/boot/plexus-classworlds.jar", "org.codehaus.plexus.classworlds.launcher.Launcher"]), "mvn");
        assert_eq!(t("bash", &["bash", "./gradlew", "assembleDebug"]), "gradlew");
        assert_eq!(t("sh", &["sh", "./mvnw", "package"]), "mvn");
        assert_eq!(t("bun", &["bun", "install"]), "bun-install");
        // Not a tool poo knows: stays the interpreter, which is presence, not a build.
        assert_eq!(t("zsh", &["-zsh"]), "zsh");
        assert_eq!(t("bash", &["bash", "-c", "cargo build"]), "bash");
        assert_eq!(t("node", &["node"]), "node");
        // Cross and versioned compilers.
        assert_eq!(t("arm-none-eabi-gcc", &[]), "gcc");
        assert_eq!(t("x86_64-linux-gnu-g++", &[]), "g++");
        assert_eq!(t("clang-18", &[]), "clang");
        assert_eq!(t("swift-frontend", &[]), "swift-frontend");
        assert_eq!(t("cargo-nextest", &[]), "cargo-nextest");
        assert_eq!(t("next-server", &[]), "next-server");
    }

    #[test]
    fn flags_point_at_the_folders_a_build_works_on() {
        let p = proc_("cargo", "/home", &["cargo", "build", "--manifest-path", "/r/crates/a/Cargo.toml"]);
        assert!(p.dirs.contains(&PathBuf::from("/r/crates/a")));
        assert!(touches(&p, "/r/target", Some(&rust())));
        let p = proc_("make", "/home", &["make", "-C", "/r/native"]);
        assert!(p.works_in(Path::new("/r/native")));
        let p = proc_("xcodebuild", "/home", &["xcodebuild", "-workspace", "/r/ios/App.xcworkspace", "-scheme", "App"]);
        assert!(build_touches(&p, Path::new("/r/ios"), None, None));
    }

    #[test]
    fn named_paths_always_count() {
        let p = proc_("swift-frontend", "/r/apple", &["swift-frontend", "-o", "/r/apple/Kit/.build/out/a.o"]);
        assert!(build_touches(&p, Path::new("/r/apple/Kit/.build"), None, None));
        assert!(!build_touches(&p, Path::new("/r/apple/UI/.build"), None, None));
        let p = proc_("cargo", "/r/target/x", &["cargo"]);
        assert!(build_touches(&p, Path::new("/r/target"), None, None));
    }

    #[test]
    fn a_pin_frees_only_other_copies_of_what_it_names() {
        let p = proc_("xcodebuild", "/r", &["xcodebuild", "-derivedDataPath", ".dd-mac-agent"]);
        assert!(touches(&p, "/r/.dd-mac-agent", Some(&dd())));
        assert!(!touches(&p, "/r/.dd-ios-main", Some(&dd())));
        // -derivedDataPath says nothing about a SYMROOT the project sets.
        assert!(touches(&p, "/r/apple/build", Some(&legacy())));
        // Compilers name every output.
        let sf = proc_("swift-frontend", "/r/apple", &["swift-frontend", "-o", "/dd/a.o"]);
        assert!(!touches(&sf, "/r/apple/Kit/.build", Some(&swift())));
    }

    #[test]
    fn other_toolchains_output_stays_in_use() {
        // Xcode may link a Rust staticlib from target/; go:embed may read web/dist.
        let xb = proc_("xcodebuild", "/r/apple", &["xcodebuild", "-scheme", "App", "-derivedDataPath", "/dd"]);
        assert!(touches(&xb, "/r/target", Some(&rust())));
        let go = proc_("go", "/r/server", &["go", "build", "./..."]);
        assert!(touches(&go, "/r/web/dist", Some(&dist())));
        // Compilers too: Xcode.app builds run them without xcodebuild.
        let sf = proc_("swift-frontend", "/r/apple", &["swift-frontend", "-o", "/dd/a.o"]);
        assert!(touches(&sf, "/r/target", Some(&rust())));
    }

    #[test]
    fn dependencies_caches_and_unknowns_stay_in_use() {
        let sf = proc_("swift-frontend", "/r/ios", &["swift-frontend", "-o", "/dd/a.o"]);
        assert!(touches(&sf, "/r/node_modules", None));
        let bare = Proc::bare(1, "clang", Path::new("/r/ios"));
        assert!(!bare.args_known);
        assert!(touches(&bare, "/r/ios/Pods", None));
        // Unknown arguments: no scoping even for its own output.
        assert!(touches(&bare, "/r/.dd-x", Some(&dd())));
    }

    #[test]
    fn isolated_stacks_keep_sibling_packages_apart() {
        let b = proc_("swift-build", "/r/apple/Kit", &["swift-build"]);
        assert!(touches(&b, "/r/apple/Kit/.build", Some(&swift())));
        assert!(!touches(&b, "/r/apple/UI/.build", Some(&swift())));
        let root = proc_("swift-build", "/r", &["swift-build"]);
        assert!(touches(&root, "/r/apple/UI/.build", Some(&swift())));
        let cargo = proc_("cargo", "/r/crates/a", &["cargo", "build"]);
        assert!(touches(&cargo, "/r/target", Some(&rust())));
        // Not isolated: node workspaces read their siblings' dist.
        let vite = proc_("node", "/r/apps/web", &["node", "/r/node_modules/vite/bin/vite.js", "build"]);
        assert!(touches(&vite, "/r/packages/ui/dist", Some(&dist())));
    }

    #[test]
    fn tools_that_run_anything_keep_the_whole_checkout() {
        let m = proc_("make", "/r/native", &["make"]);
        assert!(touches(&m, "/r/target", Some(&rust())));
        assert!(touches(&m, "/r/apple/UI/.build", Some(&swift())));
        let turbo = proc_("turbo", "/r/apps/web", &["turbo", "build"]);
        assert!(touches(&turbo, "/r/packages/ui/dist", Some(&dist())));
    }

    #[test]
    fn stack_tools_are_checked() {
        assert!(check_stack_tool("cargo").is_ok());
        for bad in ["node", "zsh", "python3", "claude", "make", "turbo", ""] {
            assert!(check_stack_tool(bad).is_err(), "{}", bad);
        }
    }
}
