//! Colors, padding, and printing.

use owo_colors::OwoColorize;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static COLOR: AtomicBool = AtomicBool::new(false);

/// Decide once, at startup, whether this run uses color.
pub fn init(no_color_flag: bool) {
    let env = std::env::var("NO_COLOR").ok();
    let tty = std::io::stdout().is_terminal();
    let on = should_color(no_color_flag, env.as_deref(), tty) && enable_ansi();
    COLOR.store(on, Ordering::Relaxed);
}

/// Color only on a terminal, without `--no-color`, and with `NO_COLOR` unset
/// or empty (<https://no-color.org>).
pub fn should_color(no_color_flag: bool, no_color_env: Option<&str>, is_tty: bool) -> bool {
    let env_disables = no_color_env.is_some_and(|v| !v.is_empty());
    is_tty && !no_color_flag && !env_disables
}

/// Older Windows consoles need virtual terminal processing switched on before
/// they render ANSI codes. Returns false if the console refuses.
#[cfg(windows)]
fn enable_ansi() -> bool {
    use std::ffi::c_void;
    unsafe extern "system" {
        fn GetStdHandle(std_handle: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    // SAFETY: plain kernel32 calls on our own stdout handle; `mode` is a
    // valid pointer for the duration of the call.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_ansi() -> bool {
    true
}

fn color_on() -> bool {
    COLOR.load(Ordering::Relaxed)
}

/// Apply `style` when color is on, else return the text untouched.
fn paint(s: &str, style: impl Fn(&str) -> String) -> String {
    if color_on() { style(s) } else { s.to_string() }
}

pub fn green(s: &str) -> String {
    paint(s, |s| s.green().to_string())
}

pub fn red(s: &str) -> String {
    paint(s, |s| s.red().to_string())
}

pub fn yellow(s: &str) -> String {
    paint(s, |s| s.yellow().to_string())
}

pub fn cyan(s: &str) -> String {
    paint(s, |s| s.cyan().to_string())
}

pub fn dim(s: &str) -> String {
    paint(s, |s| s.dimmed().to_string())
}

pub fn bold(s: &str) -> String {
    paint(s, |s| s.bold().to_string())
}

/// Print one line to stdout. Unlike `println!`, this doesn't panic when the
/// reader goes away early (`hoppy ports | head -3`).
pub fn say(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// Print without a trailing newline and flush, for prompts.
pub fn ask(prompt: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "{prompt}");
    let _ = out.flush();
}

/// How many character cells a string occupies on screen, skipping ANSI
/// escape codes (`ESC [ ... letter`).
pub fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip to the letter that ends the sequence (the `m` in `ESC[32m`).
            for esc in chars.by_ref() {
                if esc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            width += 1;
        }
    }
    width
}

/// Pad with spaces on the right until the string is `width` cells wide.
pub fn pad(s: &str, width: usize) -> String {
    let missing = width.saturating_sub(visible_width(s));
    format!("{s}{}", " ".repeat(missing))
}

/// Lay rows out as left-aligned columns, two spaces apart, sized to the
/// widest visible cell in each column. Returns one string per row.
pub fn table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|col| {
            rows.iter()
                .filter_map(|row| row.get(col))
                .map(|cell| visible_width(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();

    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, &width)| pad(cell, width))
                .collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_needs_a_tty() {
        assert!(should_color(false, None, true));
        assert!(!should_color(false, None, false));
    }

    #[test]
    fn flag_and_env_disable_color() {
        assert!(!should_color(true, None, true));
        assert!(!should_color(false, Some("1"), true));
        // An empty NO_COLOR counts as unset, per no-color.org.
        assert!(should_color(false, Some(""), true));
    }

    #[test]
    fn visible_width_ignores_ansi_codes() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width("\x1b[32mhello\x1b[0m"), 5);
        assert_eq!(visible_width("\x1b[1;31m●\x1b[0m ok"), 4);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn pad_uses_visible_width() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("\x1b[32mab\x1b[0m", 4), "\x1b[32mab\x1b[0m  ");
        assert_eq!(pad("toolong", 3), "toolong");
    }

    #[test]
    fn table_aligns_columns_with_colored_cells() {
        let rows = vec![
            vec!["\x1b[32m22\x1b[0m".to_string(), "sshd".to_string()],
            vec!["3000".to_string(), "node".to_string()],
        ];
        let lines = table(&rows);
        assert_eq!(lines[0], "\x1b[32m22\x1b[0m    sshd");
        assert_eq!(lines[1], "3000  node");
    }
}
