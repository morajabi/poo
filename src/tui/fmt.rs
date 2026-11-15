//! Pure layout helpers shared by the TUI and the plain renderer. No terminal I/O here,
//! so everything is unit tested.

/// Display width in terminal cells. Wide glyphs (emoji, CJK) count as two.
pub fn cells(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

pub fn char_cells(c: char) -> usize {
    let u = c as u32;
    if u < 0x1100 {
        return 1;
    }
    // Rough East Asian Wide / emoji ranges; enough for names and our own glyphs.
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1FAFF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

/// Truncates to `w` cells with a trailing ellipsis. Never pads.
pub fn fit(s: &str, w: usize) -> String {
    if cells(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = char_cells(c);
        if used + cw > w - 1 {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// Truncates to `w` cells and pads with spaces to exactly `w`.
pub fn pad(s: &str, w: usize) -> String {
    let t = fit(s, w);
    let n = cells(&t);
    format!("{}{}", t, " ".repeat(w.saturating_sub(n)))
}

/// Right-aligns in exactly `w` cells (truncating from the left part if needed).
pub fn rpad(s: &str, w: usize) -> String {
    let t = fit(s, w);
    let n = cells(&t);
    format!("{}{}", " ".repeat(w.saturating_sub(n)), t)
}

/// Middle ellipsis for paths: keeps the start (`~/dev`) and the end (the name).
pub fn fit_mid(s: &str, w: usize) -> String {
    let n = cells(s);
    if n <= w {
        return s.to_string();
    }
    if w < 5 {
        return fit(s, w);
    }
    let chars: Vec<char> = s.chars().collect();
    let tail = (w - 1) * 3 / 5;
    let head = w - 1 - tail;
    let mut out: String = chars.iter().take(head).collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    // Wide chars can push us over; fall back to a plain fit then.
    if cells(&out) > w {
        return fit(s, w);
    }
    out
}

const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// A smooth bar using eighth blocks. Returns (filled, track_cells): the filled part
/// (full blocks plus one partial) and how many cells remain for the track.
pub fn bar(frac: f64, w: usize) -> (String, usize) {
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let units = (frac * w as f64 * 8.0).round() as usize;
    let full = (units / 8).min(w);
    let part = if full < w { units % 8 } else { 0 };
    let mut s = "█".repeat(full);
    let mut used = full;
    if part > 0 {
        s.push(EIGHTHS[part]);
        used += 1;
    }
    (s, w - used)
}

/// Splits `width` cells between parts proportionally (largest remainder). Any part
/// that is non-zero gets at least one cell when there is room, so small slices stay visible.
pub fn alloc(parts: &[u64], width: usize) -> Vec<usize> {
    let total: u128 = parts.iter().map(|&p| p as u128).sum();
    let mut out = vec![0usize; parts.len()];
    if total == 0 || width == 0 {
        return out;
    }
    let mut rems: Vec<(u128, usize)> = vec![];
    let mut used = 0;
    for (i, &p) in parts.iter().enumerate() {
        let exact = p as u128 * width as u128;
        out[i] = (exact / total) as usize;
        used += out[i];
        rems.push((exact % total, i));
    }
    rems.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, i) in rems.iter().take(width - used) {
        out[*i] += 1;
    }
    // Lift tiny non-zero parts to one cell, taking from the largest.
    let nonzero = parts.iter().filter(|&&p| p > 0).count();
    if nonzero <= width {
        for i in 0..parts.len() {
            if parts[i] > 0 && out[i] == 0 {
                if let Some(big) = (0..out.len()).max_by_key(|&j| out[j]) {
                    if out[big] > 1 {
                        out[big] -= 1;
                        out[i] = 1;
                    }
                }
            }
        }
    }
    out
}

/// One frame of the sweeping broom: dust ahead, swept floor behind. Exactly `w` cells.
pub fn broom(tick: u64, w: usize) -> (String, String) {
    if w < 4 {
        return (" ".repeat(w), String::new());
    }
    let span = (w - 2) as u64;
    let p = (tick % (span + 1)) as usize;
    let swept = " ".repeat(p);
    let mut ahead = String::from("🧹");
    for i in (p + 2)..w {
        let d = i - p - 2;
        let c = if d < 2 && tick % 2 == 0 {
            '∴'
        } else if d < 2 {
            '∵'
        } else {
            match (i * 7 + 3) % 11 {
                0 | 5 => '·',
                8 => '.',
                _ => ' ',
            }
        };
        ahead.push(c);
    }
    (swept, ahead)
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// "12s", "4m", "2h", "3d" — compact freshness for the header.
pub fn since(secs: i64) -> String {
    let d = secs.max(0);
    if d < 5 {
        "now".into()
    } else if d < 60 {
        format!("{}s", d)
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86400)
    }
}

/// Keeps `cursor` visible in a window of `height` rows, returning the new offset.
pub fn scroll(offset: usize, cursor: usize, height: usize, len: usize) -> usize {
    if height == 0 || len == 0 {
        return 0;
    }
    let mut o = offset.min(len.saturating_sub(1));
    if cursor < o {
        o = cursor;
    } else if cursor >= o + height {
        o = cursor + 1 - height;
    }
    o.min(len.saturating_sub(height.min(len)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_and_pad() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(cells(&fit("hello world", 6)), 6);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(rpad("ab", 4), "  ab");
        assert_eq!(cells(&pad("a very long name", 5)), 5);
        assert_eq!(fit("x", 0), "");
    }

    #[test]
    fn wide_glyphs() {
        assert_eq!(cells("🧹"), 2);
        assert_eq!(cells("ab🧹"), 4);
        assert!(cells(&fit("🧹🧹🧹🧹", 5)) <= 5);
    }

    #[test]
    fn mid_keeps_ends() {
        let s = "~/dev/some/really/deep/project/target";
        let m = fit_mid(s, 20);
        assert_eq!(cells(&m), 20);
        assert!(m.starts_with("~/dev"));
        assert!(m.ends_with("target"));
        assert_eq!(fit_mid("short", 20), "short");
    }

    #[test]
    fn bar_widths() {
        for w in [0, 1, 7, 20] {
            for f in [0.0, 0.01, 0.33, 0.5, 0.999, 1.0, 2.0, -1.0, f64::NAN] {
                let (s, rest) = bar(f, w);
                assert_eq!(cells(&s) + rest, w, "w={} f={}", w, f);
            }
        }
        assert_eq!(bar(1.0, 4).0, "████");
        assert_eq!(bar(0.0, 4), (String::new(), 4));
        assert_eq!(bar(0.5, 4).0, "██");
    }

    #[test]
    fn alloc_sums_and_keeps_slivers() {
        let a = alloc(&[900, 90, 9, 1], 20);
        assert_eq!(a.iter().sum::<usize>(), 20);
        assert!(a.iter().all(|&x| x >= 1));
        assert_eq!(alloc(&[0, 0], 10), vec![0, 0]);
        assert_eq!(alloc(&[1, 1], 0), vec![0, 0]);
        assert_eq!(alloc(&[5, 0, 5], 10), vec![5, 0, 5]);
        let b = alloc(&[1, 1, 1], 2);
        assert_eq!(b.iter().sum::<usize>(), 2);
    }

    #[test]
    fn broom_is_exact_width() {
        for w in [4, 10, 33] {
            for t in 0..80 {
                let (a, b) = broom(t, w);
                assert_eq!(cells(&a) + cells(&b), w, "w={} t={}", w, t);
            }
        }
    }

    #[test]
    fn scroll_window() {
        assert_eq!(scroll(0, 0, 5, 100), 0);
        assert_eq!(scroll(0, 7, 5, 100), 3);
        assert_eq!(scroll(10, 4, 5, 100), 4);
        assert_eq!(scroll(50, 99, 5, 100), 95);
        assert_eq!(scroll(9, 2, 5, 3), 0);
        assert_eq!(scroll(0, 0, 0, 3), 0);
    }

    #[test]
    fn since_compact() {
        assert_eq!(since(2), "now");
        assert_eq!(since(42), "42s");
        assert_eq!(since(600), "10m");
        assert_eq!(since(7200), "2h");
    }
}
