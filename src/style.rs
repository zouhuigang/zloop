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

/// 一个字符串在终端里占几列。
///
/// 按 Unicode East_Asian_Width 算（`unicode-width`）：宽字符（汉字、大部分 emoji）2 列，
/// 其余 1 列，ambiguous 按 1 列——和 wcwidth 一致，也就是绝大多数终端的实际行为。
///
/// 这里曾经是一张手写的区间表，把 `0x231A..=0x23FA` 整段当成 2 列，于是 `⏭ ⏱ ⏸ ⏹ ⏺`
/// 这些 EAW=N 的符号（还有 `⚠` `✓`）都被多算一列。以前每行右边没有东西，看不出来；
/// 一旦画表格的右边框，那几行就短一格。而且目标文字 / note 里可能出现任意 emoji，
/// 手写表覆盖不了，所以换成标准实现。
pub fn width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
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

/// 一列怎么对齐。数字右对齐、文字左对齐，看着才像一张表。
#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Right,
}

/// 画一张框线表：表头 + 若干行，列宽按 `width()` 算（中文和 emoji 两列）。
///
/// `budget` 是可用总宽度；哪一列该被压缩由 `flex` 指定（通常是文本那列），
/// 其余列按内容取最大宽。返回逐行的字符串，调用方自己加缩进。
pub fn table(head: &[&str], rows: &[Vec<String>], align: &[Align], flex: usize, budget: usize, c: &Style) -> Vec<String> {
    let n = head.len();
    let mut w: Vec<usize> = (0..n)
        .map(|i| rows.iter().map(|r| width(&r[i])).max().unwrap_or(0).max(width(head[i])))
        .collect();
    // 框线开销：每列 `│ 内容 ` = 宽度 + 3，末尾再一个 `│`
    let fixed: usize = w.iter().enumerate().filter(|(i, _)| *i != flex).map(|(_, x)| *x).sum();
    let room = budget.saturating_sub(fixed + 3 * n + 1);
    w[flex] = w[flex].min(room.max(8));

    let rule = |l: &str, m: &str, r: &str| {
        c.dim(&format!("{l}{}{r}", w.iter().map(|x| "─".repeat(x + 2)).collect::<Vec<_>>().join(m)))
    };
    let bar = c.dim("│");
    let cell = |s: &str, i: usize| {
        let s = truncate(s, w[i]);
        let pad = " ".repeat(w[i].saturating_sub(width(&s)));
        if align.get(i) == Some(&Align::Right) { format!("{pad}{s}") } else { format!("{s}{pad}") }
    };
    let line = |cells: Vec<String>| format!("{bar} {} {bar}", cells.join(&format!(" {bar} ")));

    let mut out = vec![rule("┌", "┬", "┐")];
    out.push(line(head.iter().enumerate().map(|(i, h)| c.dim(&cell(h, i))).collect()));
    out.push(rule("├", "┼", "┤"));
    for r in rows {
        out.push(line(r.iter().enumerate().map(|(i, v)| cell(v, i)).collect()));
    }
    out.push(rule("└", "┴", "┘"));
    out
}
