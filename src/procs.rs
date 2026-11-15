//! Working directories of running processes, used to keep poo away from builds in flight.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: i32,
    pub name: String,
    pub cwd: PathBuf,
}

/// Build tools: a project where one of these runs is hot, and cleaning it would break a build.
pub const BUILD_TOOLS: &[&str] = &[
    "xcodebuild", "swift-build", "swift-frontend", "swift-test", "swift-driver", "swiftc",
    "cargo", "rustc", "clippy-driver", "cargo-nextest", "clang", "clang++", "cc", "c++", "ld",
    "ld64", "lld", "gradlew", "mvn", "make", "gmake", "ninja", "cmake", "bazel", "buck2", "zig",
    "tsc", "esbuild", "webpack", "rollup", "vite", "turbo", "nx", "next-server", "pod",
    "carthage", "flutter", "dart", "dotnet", "msbuild", "mix", "pip", "uv", "poetry", "go",
    "xcrun", "ibtool", "actool", "codesign", "lipo", "docker-buildx", "pnpm", "yarn", "npm",
];

/// Agents, editors, shells and dev servers: a project where one runs is in use (warm), but
/// nothing is being built, so a forced clean only costs a rebuild.
pub const PRESENCE: &[&str] = &[
    "claude", "codex", "opencode", "cursor-agent", "gemini", "aider", "amp", "goose", "copilot",
    "node", "bun", "deno", "python", "python3", "ruby", "java", "zsh", "bash", "fish", "sh", "nu",
    "nvim", "vim", "emacs", "hx", "tmux", "rust-analyzer", "sourcekit-lsp", "gopls",
];

#[cfg(target_os = "macos")]
pub fn list() -> Vec<Proc> {
    use std::ffi::CStr;
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    const VNODE_INFO_SIZE: usize = 152;
    const PATH_MAX: usize = 1024;
    let mut pids = vec![0i32; 8192];
    let n = unsafe {
        libc::proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, (pids.len() * 4) as i32)
    };
    if n <= 0 {
        return vec![];
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
        let cwd = cwd.to_string_lossy().to_string();
        if cwd.is_empty() || cwd == "/" {
            continue;
        }
        let mut name_buf = [0u8; 256];
        let len = unsafe { libc::proc_name(pid, name_buf.as_mut_ptr() as *mut libc::c_void, 256) };
        let name = if len > 0 {
            String::from_utf8_lossy(&name_buf[..len as usize]).to_string()
        } else {
            String::new()
        };
        out.push(Proc { pid, name, cwd: PathBuf::from(cwd) });
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn list() -> Vec<Proc> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(cwd) = std::fs::read_link(e.path().join("cwd")) else { continue };
        let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string();
        out.push(Proc { pid, name, cwd });
    }
    out
}

pub fn is_build_tool(name: &str, extra: &[String]) -> bool {
    BUILD_TOOLS.iter().any(|t| *t == name) || extra.iter().any(|t| t == name)
}

pub fn is_presence(name: &str) -> bool {
    PRESENCE.iter().any(|t| *t == name)
}

/// Build processes whose working directory is inside `path`.
pub fn builds_inside<'a>(procs: &'a [Proc], path: &std::path::Path, extra: &[String]) -> Vec<&'a Proc> {
    procs
        .iter()
        .filter(|p| p.cwd.starts_with(path) && is_build_tool(&p.name, extra))
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
