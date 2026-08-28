//! Terminal colour, switched off whenever the output is not a terminal.
//!
//! Rules, in order: `--no-color` → `NO_COLOR` (any value) → `CLICOLOR_FORCE=1` → is stdout a tty.
//! Piped output is therefore always plain text, which keeps `zloop status | grep …` and the
//! test-suite assertions working on exactly what a human sees minus the escapes.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    pub fn detect(no_color_flag: bool) -> Style {
        if no_color_flag || std::env::var_os("NO_COLOR").is_some() {
            return Style { color: false };
        }
        if std::env::var("CLICOLOR_FORCE").map(|v| v == "1").unwrap_or(false) {
            return Style { color: true };
        }
        Style { color: std::io::stdout().is_terminal() }
    }

    pub fn plain() -> Style {
        Style { color: false }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color && !s.is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn blue(&self, s: &str) -> String {
        self.wrap("34", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    /// Reverse video + bold: the one line that should be readable from across the room.
    pub fn banner(&self, code: &str, s: &str) -> String {
        self.wrap(&format!("1;{code}"), s)
    }
}

/// Terminal width in columns: `COLUMNS` → `TIOCGWINSZ` → 80.
///
/// Every line `zloop status` prints is truncated to fit this, because a wrapped line loses the
/// left gutter and that is exactly what makes a dashboard look ragged.
pub fn term_width() -> usize {
    if let Some(n) = std::env::var("COLUMNS").ok().and_then(|v| v.parse::<usize>().ok()) {
        if n >= 20 {
            return n;
        }
    }
    #[cfg(unix)]
    {
        if let Some(n) = ioctl_cols() {
            if n >= 20 {
                return n;
            }
        }
    }
    80
}

#[cfg(unix)]
fn ioctl_cols() -> Option<usize> {
    use std::os::raw::{c_int, c_ulong};
    #[repr(C)]
    struct Winsize {
        rows: u16,
        cols: u16,
        xpix: u16,
        ypix: u16,
    }
    extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_vendor = "apple", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
    const TIOCGWINSZ: c_ulong = 0x4008_7468;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_vendor = "apple", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
    const TIOCGWINSZ: c_ulong = 0x5413;
    let mut ws = Winsize { rows: 0, cols: 0, xpix: 0, ypix: 0 };
    // stdout, then stderr, then stdin: `zloop status | less` still knows how wide the screen is.
    for fd in [1, 2, 0] {
        if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize) } == 0 && ws.cols > 0 {
            return Some(ws.cols as usize);
        }
    }
    None
}

/// Display width of a string, counting CJK/emoji as two columns.
pub fn width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let c = c as u32;
            let wide = (0x1100..=0x115F).contains(&c)
                || (0x231A..=0x23FA).contains(&c) // ⌚⌛⏩…⏳⏸⏹⏺ render two columns wide
                || (0x2E80..=0xA4CF).contains(&c)
                || (0xAC00..=0xD7A3).contains(&c)
                || (0xF900..=0xFAFF).contains(&c)
                || (0xFE30..=0xFE6F).contains(&c)
                || (0xFF00..=0xFF60).contains(&c)
                || (0xFFE0..=0xFFE6).contains(&c)
                || (0x1F300..=0x1FAFF).contains(&c)
                || (0x2600..=0x27BF).contains(&c);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Truncate to `max` display columns, adding an ellipsis when it does not fit.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = width(&c.to_string());
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// `████████░░░░` — `done` of `total`, `cells` wide.
pub fn bar(done: usize, total: usize, cells: usize, st: &Style) -> String {
    if total == 0 {
        return st.dim(&"░".repeat(cells));
    }
    let filled = (done * cells) / total.max(1);
    let filled = filled.min(cells);
    let full = "█".repeat(filled);
    let empty = "░".repeat(cells - filled);
    let colored = if done == total { st.green(&full) } else { st.cyan(&full) };
    format!("{colored}{}", st.dim(&empty))
}
