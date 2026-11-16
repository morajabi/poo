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
    // Before Homebrew: the cask links the app's bundled CLI into the brew prefix.
    if s.contains(".app/Contents/") {
        return Some("the Poo app: Sparkle updates the app and its bundled CLI together (Poo > Check for Updates…)".into());
    }
    if s.contains("/Cellar/") || s.contains("/Caskroom/") || s.contains("/homebrew/") || s.contains("/linuxbrew/") {
        return Some("brew upgrade poo".into());
    }
    if s.contains("/nix/store/") {
        return Some("your Nix configuration".into());
    }
    if s.contains("/mise/installs/") {
        return Some("mise upgrade poo".into());
    }
    None
}

fn current_exe() -> PathBuf {
    std::env::current_exe().map(|p| std::fs::canonicalize(&p).unwrap_or(p)).unwrap_or_default()
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
    let exe = current_exe();
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
    install(&m, force)
}

fn install(m: &Manifest, force: bool) -> Result<String> {
    if !force && !newer(&m.version, VERSION) {
        return Ok(VERSION.into());
    }
    let exe = std::fs::canonicalize(std::env::current_exe()?)?;
    if let Some(mgr) = managed_by(&exe) {
        bail!("poo is not updated in place here; it is managed by {}", mgr);
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
    // Staged next to the binary so the final rename is atomic; removed on every path.
    let res = (|| -> Result<()> {
        std::fs::create_dir_all(&staging).context("could not write next to the poo binary (try `sudo poo update`)")?;
        let tarball = staging.join("poo.tar.gz");
        std::fs::write(&tarball, &data)?;
        let st = std::process::Command::new("tar")
            .args(["-xzf", &tarball.to_string_lossy(), "-C", &staging.to_string_lossy()])
            .status()?;
        if !st.success() {
            bail!("could not unpack the update");
        }
        let new_bin = find_binary(&staging).context("update archive has no poo binary")?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&new_bin, &exe).context("could not replace the poo binary (permissions?)")?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    res?;
    let _ = record_check(&m.version);
    Ok(m.version.clone())
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

const DAY: i64 = 86400;

/// The last update check on record (no network): when it ran and the latest version it saw.
pub fn last_check() -> Option<(i64, String)> {
    let st = read_stamp();
    (st.checked_at > 0 && !st.latest.is_empty()).then_some((st.checked_at, st.latest))
}

fn stamp_path() -> PathBuf {
    state_dir().join("update-check.json")
}

fn read_stamp() -> Stamp {
    std::fs::read(stamp_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn write_stamp(st: &Stamp) -> Result<()> {
    let p = stamp_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(st)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

fn record_check(latest: &str) -> Result<()> {
    write_stamp(&Stamp { checked_at: now(), latest: latest.into() })
}

/// Whether this run may look for updates on its own: a person at a terminal, not CI,
/// not a test, not the copy inside Poo.app (Sparkle owns that one).
/// Agents and scripts count too: a CLI only an agent runs still updates itself.
pub fn background_allowed() -> bool {
    if ["CI", "POO_NO_UPDATE_CHECK"].iter().any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty())) {
        return false;
    }
    !current_exe().to_string_lossy().contains(".app/Contents/")
}

/// Called at the start of a command. Costs one small file read: when the last check is
/// a day old it spawns `poo update --background` detached and returns at once. Returns a
/// one-line hint when a newer version is known and will not be installed on its own.
pub fn background(auto: bool) -> Option<String> {
    let mut st = read_stamp();
    if now() - st.checked_at > DAY {
        // Claim today's check first so parallel commands do not all spawn one.
        st.checked_at = now();
        if write_stamp(&st).is_ok() {
            spawn_background();
        }
    }
    if st.latest.is_empty() || !newer(&st.latest, VERSION) {
        return None;
    }
    let exe = current_exe();
    match managed_by(&exe) {
        Some(mgr) => Some(format!("poo {} is available (you have {}). Update with {}.", st.latest, VERSION, mgr)),
        // The background run installs it, unless it can't write next to the binary.
        None if auto && exe.parent().is_some_and(writable) => None,
        None if auto => Some(format!("poo {} is available (you have {}). Run `sudo poo update`.", st.latest, VERSION)),
        None => Some(format!("poo {} is available (you have {}). Run `poo update`.", st.latest, VERSION)),
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".poo-write-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn spawn_background() {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::process::Command::new(exe)
        .args(["update", "--background"])
        .env("POO_NO_UPDATE_CHECK", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Its own process group: Ctrl-C in the terminal does not reach it.
        .process_group(0)
        .spawn();
}

/// What `poo update --background` does: record the latest version and, when allowed,
/// install it. Never prints; failures wait for tomorrow's check.
pub fn background_run(auto: bool) -> Result<Option<String>> {
    let m = fetch_manifest()?;
    record_check(&m.version)?;
    if !auto || !newer(&m.version, VERSION) || managed_by(&current_exe()).is_some() {
        return Ok(None);
    }
    install(&m, false).map(Some)
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
        assert!(newer("0.1.10", "0.1.9"));
    }

    #[test]
    fn managed() {
        assert!(managed_by(Path::new("/Applications/Poo.app/Contents/Resources/bin/poo")).unwrap().contains("Sparkle"));
        assert!(managed_by(Path::new("/opt/homebrew/Cellar/poo/0.1.3/bin/poo")).unwrap().contains("brew"));
        assert!(managed_by(Path::new("/Users/me/.local/bin/poo")).is_none());
    }
}
