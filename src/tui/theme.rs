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
        let color = std::env::var_os("NO_COLOR").is_none();
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
}
