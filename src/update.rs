//! Self-update from GitHub releases through a `latest.json` manifest that the
//! install script reads too. Package-manager installs are left to their manager.

use crate::config::state_dir;
use crate::util::{now, sha256_hex};
use crate::{REPO, VERSION};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub assets: BTreeMap<String, String>,
    #[serde(default)]
    pub sha256: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub managed_by: Option<String>,
    pub notes: String,
}

pub fn manifest_url() -> String {
    std::env::var("POO_RELEASES_URL")
        .unwrap_or_else(|_| format!("https://github.com/{}/releases/latest/download/latest.json", REPO))
}

pub fn target_key() -> String {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        o => o,
    };
    format!("cli-{}-{}", arch, os)
}

fn parse_ver(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(|c| c == '.' || c == '-')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub fn newer(latest: &str, current: &str) -> bool {
    parse_ver(latest) > parse_ver(current)
}

/// Which package manager owns this binary, if any.
pub fn managed_by(exe: &Path) -> Option<String> {
    let s = exe.to_string_lossy();
    if s.contains("/Cellar/") || s.contains("/homebrew/") || s.contains("/linuxbrew/") {
        return Some("brew upgrade poo".into());
    }
    if s.contains("/nix/store/") {
        return Some("your Nix configuration".into());
    }
    if s.contains("/mise/installs/") {
        return Some("mise upgrade poo".into());
    }
    if s.contains(".app/Contents/") {
        return Some("the Poo app (it updates itself and its bundled CLI)".into());
    }
    None
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .user_agent(format!("poo/{}", VERSION))
        .build()
        .new_agent();
    let mut resp = agent.get(url).call().with_context(|| format!("GET {}", url))?;
    let body = resp.body_mut().with_config().limit(200 * 1024 * 1024).read_to_vec()?;
    Ok(body)
}

pub fn fetch_manifest() -> Result<Manifest> {
    let data = fetch(&manifest_url())?;
    serde_json::from_slice(&data).context("release manifest is not valid JSON")
}

pub fn check() -> Result<Check> {
    let m = fetch_manifest()?;
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = record_check(&m.version);
    Ok(Check {
        current: VERSION.into(),
        update_available: newer(&m.version, VERSION),
        latest: m.version,
        managed_by: managed_by(&exe),
        notes: m.notes,
    })
}

/// Downloads, verifies and swaps in the new binary. Returns the new version.
pub fn run(force: bool) -> Result<String> {
    let m = fetch_manifest()?;
    if !force && !newer(&m.version, VERSION) {
        return Ok(VERSION.into());
    }
    let exe = std::fs::canonicalize(std::env::current_exe()?)?;
    if let Some(mgr) = managed_by(&exe) {
        bail!("poo was installed by a package manager; update with: {}", mgr);
    }
    let key = target_key();
    let url = m.assets.get(&key).with_context(|| format!("no release asset for {}", key))?;
    let want = m.sha256.get(&key).with_context(|| format!("no checksum for {}", key))?;
    let data = fetch(url)?;
    let got = sha256_hex(&data);
    if !got.eq_ignore_ascii_case(want) {
        bail!("checksum mismatch for {} (expected {}, got {})", key, want, got);
    }
    let dir = exe.parent().context("binary has no parent folder")?;
    let staging = dir.join(format!(".poo-update-{}", std::process::id()));
    std::fs::create_dir_all(&staging)?;
    let tarball = staging.join("poo.tar.gz");
    std::fs::write(&tarball, &data)?;
    let st = std::process::Command::new("tar")
        .args(["-xzf", &tarball.to_string_lossy(), "-C", &staging.to_string_lossy()])
        .status()?;
    if !st.success() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("could not unpack the update");
    }
    let new_bin = find_binary(&staging).context("update archive has no poo binary")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&new_bin, &exe).context("could not replace the poo binary (permissions?)")?;
    let _ = std::fs::remove_dir_all(&staging);
    let _ = record_check(&m.version);
    Ok(m.version)
}

fn find_binary(dir: &Path) -> Option<PathBuf> {
    for e in walk(dir) {
        if e.file_name().map(|n| n == "poo").unwrap_or(false) && e.is_file() {
            return Some(e);
        }
    }
    None
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[derive(Serialize, Deserialize, Default)]
struct Stamp {
    checked_at: i64,
    latest: String,
}

fn stamp_path() -> PathBuf {
    state_dir().join("update-check.json")
}

fn record_check(latest: &str) -> Result<()> {
    let p = stamp_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, serde_json::to_vec(&Stamp { checked_at: now(), latest: latest.into() })?)?;
    Ok(())
}

/// A one-line notice at most once a day for interactive use. Never blocks for long.
pub fn notice() -> Option<String> {
    let stamp: Stamp = std::fs::read(stamp_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    if now() - stamp.checked_at > 86400 {
        // Refresh in the background for next time.
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(["update", "--check", "--quiet"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    if !stamp.latest.is_empty() && newer(&stamp.latest, VERSION) {
        return Some(format!("poo {} is available (you have {}). Run `poo update`.", stamp.latest, VERSION));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }
}
