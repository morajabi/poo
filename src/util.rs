use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands `~`, `$HOME` and `$TMPDIR` at the start of a path.
pub fn expand(path: &str) -> PathBuf {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let tmp = tmp.trim_end_matches('/').to_string();
    if path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    if let Some(rest) = path.strip_prefix("$HOME") {
        return PathBuf::from(format!("{}{}", home().display(), rest));
    }
    if let Some(rest) = path.strip_prefix("$TMPDIR") {
        return PathBuf::from(format!("{}{}", tmp, rest));
    }
    PathBuf::from(path)
}

/// Shortens a path for display by replacing the home prefix with `~`.
pub fn tilde(path: &Path) -> String {
    let h = home();
    if let Ok(rest) = path.strip_prefix(&h) {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn system_time_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Decimal units, matching Finder and `df -H`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", n)
    } else if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[i])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, UNITS[i])
    } else {
        format!("{:.2} {}", v, UNITS[i])
    }
}

/// Parses sizes like `20G`, `1.5GB`, `500M`, `2048` (bytes).
pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase().replace(' ', "");
    let (num, unit): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.');
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim_end_matches('B').trim_end_matches('I') {
        "" => 1.0,
        "K" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

pub fn age(secs: i64) -> String {
    let d = (now() - secs).max(0);
    if d < 60 {
        "just now".into()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else if d < 86400 * 60 {
        format!("{}d ago", d / 86400)
    } else if d < 86400 * 730 {
        format!("{}mo ago", d / (86400 * 30))
    } else {
        format!("{}y ago", d / (86400 * 365))
    }
}

pub fn days_since(secs: i64) -> i64 {
    ((now() - secs).max(0)) / 86400
}

/// RFC 3339 UTC timestamp without external crates.
pub fn iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn short_hash(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest[..6].iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && is_tty(1)
}

/// Runs a command with a timeout and returns stdout on success.
pub fn run_capture(cmd: &[&str], timeout_secs: u64) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(cmd.first()?)
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()?;
                return if status.success() {
                    Some(String::from_utf8_lossy(&out).into_owned())
                } else {
                    None
                };
            }
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let p = Path::new(extra).join(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_bytes("20G"), Some(20_000_000_000));
        assert_eq!(parse_bytes("1.5gb"), Some(1_500_000_000));
        assert_eq!(parse_bytes("500 MB"), Some(500_000_000));
        assert_eq!(parse_bytes("12"), Some(12));
        assert_eq!(parse_bytes("x"), None);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_500_000), "1.50 MB");
        assert_eq!(bytes(25_300_000_000), "25.3 GB");
    }

    #[test]
    fn iso() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
