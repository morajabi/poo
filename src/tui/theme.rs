//! Colors for the TUI. Named ANSI colors and DIM follow the terminal's own palette, so
//! the same theme reads on dark and light backgrounds. NO_COLOR turns color off.

use crate::model::Tier;
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub light: bool,
    pub truecolor: bool,
}

impl Theme {
    pub fn detect() -> Theme {
        let color = !crate::util::no_color();
        // COLORFGBG="15;0" (fg;bg); a light background is 7 or 15.
        let light = std::env::var("COLORFGBG")
            .ok()
            .and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok()))
            .map(|bg| bg == 7 || bg == 15)
            .unwrap_or(false)
            || std::env::var("POO_THEME").map(|t| t == "light").unwrap_or(false);
        let truecolor = matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor") | Ok("24bit"));
        Theme { color, light, truecolor }
    }

    /// Whether the user said which background they have (COLORFGBG or POO_THEME).
    pub fn background_known() -> bool {
        std::env::var_os("COLORFGBG").is_some() || std::env::var_os("POO_THEME").is_some()
    }

    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    }
    pub fn text(&self) -> Style {
        Style::default()
    }
    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    /// The one accent: tabs, focused borders, key names.
    pub fn accent(&self) -> Style {
        self.fg(Color::Cyan)
    }
    /// Space that is fine to free.
    pub fn good(&self) -> Style {
        self.fg(Color::Green)
    }
    pub fn info(&self) -> Style {
        self.fg(Color::Blue)
    }
    /// Gentle attention (in-use mode, warnings). Never red.
    pub fn note(&self) -> Style {
        self.fg(Color::Yellow)
    }
    pub fn border(&self) -> Style {
        self.dim()
    }
    pub fn border_focus(&self) -> Style {
        if self.color {
            self.accent()
        } else {
            self.bold()
        }
    }
    pub fn selected_row(&self) -> Style {
        if !self.color {
            return Style::default().add_modifier(Modifier::REVERSED);
        }
        let bg = if self.light { Color::Indexed(254) } else { Color::Indexed(236) };
        Style::default().bg(bg)
    }
    pub fn track(&self) -> Style {
        self.dim()
    }
    pub fn tier(&self, t: Tier) -> Style {
        match t {
            Tier::Junk | Tier::Cold | Tier::Idle => self.good(),
            Tier::Warm => self.text(),
            Tier::Hot => self.accent(),
            Tier::Review => self.info(),
            Tier::Protected => self.dim(),
        }
    }
    /// Disk bar parts: other, found in use, not in use, free.
    pub fn disk_part(&self, i: usize) -> Style {
        if !self.color {
            return Style::default();
        }
        match i {
            0 => Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            1 => Style::default().fg(Color::Blue),
            2 => Style::default().fg(Color::Green),
            _ => Style::default().add_modifier(Modifier::DIM),
        }
    }
    /// A bucket's own color (from its mod) for its bar.
    pub fn hex(&self, hex: &str) -> Style {
        if !self.color {
            return Style::default();
        }
        match parse_hex(hex) {
            Some((r, g, b)) if self.truecolor => Style::default().fg(Color::Rgb(r, g, b)),
            Some((r, g, b)) => Style::default().fg(Color::Indexed(rgb_to_256(r, g, b))),
            None => self.good(),
        }
    }
}

/// Asks the terminal for its background color (OSC 11), followed by a device attributes
/// request every terminal answers, so terminals without OSC 11 cost one round trip rather
/// than the timeout. Call in raw mode, before anything else reads stdin. `Some(true)` for a
/// light background.
#[cfg(unix)]
pub fn query_light_background(timeout: std::time::Duration) -> Option<bool> {
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
    out.flush().ok()?;
    let deadline = std::time::Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as libc::c_int) };
        if n <= 0 {
            break;
        }
        let mut chunk = [0u8; 128];
        let r = unsafe { libc::read(0, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if r <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..r as usize]);
        // The device attributes answer (ESC [ ? ... c) comes last.
        if let Some(i) = find(&buf, b"\x1b[?") {
            if buf[i..].contains(&b'c') {
                break;
            }
        }
        if buf.len() > 512 {
            break;
        }
    }
    parse_osc11(&buf)
}

#[cfg(not(unix))]
pub fn query_light_background(_: std::time::Duration) -> Option<bool> {
    None
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` (1 to 4 hex digits per channel) to light or dark.
pub fn parse_osc11(buf: &[u8]) -> Option<bool> {
    let i = find(buf, b"]11;rgb:")? + 8;
    let rest = std::str::from_utf8(&buf[i..]).ok()?;
    let end = rest.find(|c: char| !(c.is_ascii_hexdigit() || c == '/')).unwrap_or(rest.len());
    let mut ch = rest[..end].split('/').map(|h| {
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len().clamp(1, 4))) - 1;
        Some(v as f64 / max as f64)
    });
    let (r, g, b) = (ch.next()??, ch.next()??, ch.next()??);
    Some(0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5)
}

pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

/// Nearest xterm-256 color for terminals without 24-bit color (Terminal.app).
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let (qr, qg, qb) = (q(r), q(g), q(b));
    let cube = 16 + 36 * qr + 6 * qg + qb;
    // Grays are closer on the ramp.
    if r.abs_diff(g) < 10 && g.abs_diff(b) < 10 {
        let avg = (r as u16 + g as u16 + b as u16) / 3;
        if avg < 8 {
            return 16;
        }
        if avg > 238 {
            return 231;
        }
        return 232 + ((avg - 8) / 10) as u8;
    }
    cube
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_256() {
        assert_eq!(parse_hex("#ff8000"), Some((255, 128, 0)));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
        assert_eq!(rgb_to_256(255, 0, 0), 196);
        assert_eq!(rgb_to_256(0, 255, 0), 46);
    }

    #[test]
    fn osc11_background() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;22c"), Some(true));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x07"), Some(false));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:fa/fa/f0\x07"), Some(true));
        assert_eq!(parse_osc11(b"\x1b[?1;2c"), None);
    }
}
