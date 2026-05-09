// ── NeuronCLI Brand Color System ─────────────────────────────────────────────
//
// Every piece of terminal output should use these constants for a unified,
// premium feel.  Import `use crate::brand::*;` in any module that renders
// user-facing text.

// ── Raw ANSI escape codes (RGB true-color) ───────────────────────────────────

/// Primary brand blue — headings, borders, connection status  (#4169C3)
pub const BLUE: &str = "\x1b[38;2;65;105;195m";
/// Accent orange — highlights, active items, "what's new"     (#F0A028)
pub const ORANGE: &str = "\x1b[38;2;240;160;40m";
/// Success green — checkmarks, completions, connected         (#2D8C3C)
pub const GREEN: &str = "\x1b[38;2;45;140;60m";
/// Error/warning red — errors, deletions                      (#C83228)
pub const RED: &str = "\x1b[38;2;200;50;40m";
/// Secondary dim — metadata, paths, timestamps                (#888888)
pub const DIM: &str = "\x1b[38;2;136;136;136m";
/// Bright foreground                                          (#DCDCE6)
pub const WHITE: &str = "\x1b[38;2;220;220;230m";
/// Soft white for decorative elements                         (#C8C8DC)
pub const SOFT: &str = "\x1b[38;2;200;200;220m";
/// Cyan for code/commands                                     (#5AC8FA)
pub const CYAN: &str = "\x1b[38;2;90;200;250m";
/// Bold modifier
pub const BOLD: &str = "\x1b[1m";
/// Dim modifier
pub const DIM_ATTR: &str = "\x1b[2m";
/// Italic modifier
pub const ITALIC: &str = "\x1b[3m";
/// Underline modifier
pub const UNDERLINE: &str = "\x1b[4m";
/// Reset all attributes
pub const R: &str = "\x1b[0m";
/// Dark background for code blocks
pub const BG_CODE: &str = "\x1b[48;5;236m";
/// Neuron gradient text: N(blue) e(red) u(orange) r(orange) o(green) n(green)
pub const NEURON_LOGO: &str = "\x1b[1;38;2;65;105;195mN\x1b[38;2;200;50;40me\x1b[38;2;240;160;40mu\x1b[38;2;240;160;40mr\x1b[38;2;45;140;60mo\x1b[38;2;45;140;60mn\x1b[0m";

// ── Block character icons (NO emojis) ────────────────────────────────────────
// These are single-width Unicode characters that render identically
// on every terminal, every OS, every font.  Premium, minimal, consistent.

/// Green checkmark indicator
pub const ICON_OK: &str = "\x1b[38;2;45;140;60m\u{2713}\x1b[0m"; // ✓
/// Red cross indicator
pub const ICON_ERR: &str = "\x1b[38;2;200;50;40m\u{2717}\x1b[0m"; // ✗
/// Blue filled circle (active)
pub const ICON_ACTIVE: &str = "\x1b[38;2;65;105;195m\u{25CF}\x1b[0m"; // ●
/// Dim empty circle (inactive)
pub const ICON_INACTIVE: &str = "\x1b[38;2;136;136;136m\u{25CB}\x1b[0m"; // ○
/// Orange right-pointing triangle (action/arrow)
pub const ICON_ARROW: &str = "\x1b[38;2;240;160;40m\u{25B8}\x1b[0m"; // ▸
/// Blue small diamond
pub const ICON_DIAMOND: &str = "\x1b[38;2;65;105;195m\u{25C6}\x1b[0m"; // ◆
/// Cyan file indicator (replaces 📄)
pub const ICON_FILE: &str = "\x1b[38;2;90;200;250m\u{25A0}\x1b[0m"; // ■
/// Green write indicator (replaces ✏️)
pub const ICON_WRITE: &str = "\x1b[38;2;45;140;60m\u{25B6}\x1b[0m"; // ▶
/// Orange edit indicator (replaces 📝)
pub const ICON_EDIT: &str = "\x1b[38;2;240;160;40m\u{25C8}\x1b[0m"; // ◈
/// Blue search indicator (replaces 🔎)
pub const ICON_SEARCH: &str = "\x1b[38;2;65;105;195m\u{25C9}\x1b[0m"; // ◉
/// Orange web indicator (replaces 🌐)
pub const ICON_WEB: &str = "\x1b[38;2;240;160;40m\u{25CE}\x1b[0m"; // ◎
/// Blue thinking indicator (replaces 🧠)
pub const ICON_THINK: &str = "\x1b[38;2;65;105;195m\u{25E6}\x1b[0m"; // ◦
/// Green done indicator (replaces ✨)
pub const ICON_DONE: &str = "\x1b[38;2;45;140;60m\u{2714}\x1b[0m"; // ✔
/// Orange command/bash indicator (replaces $)
pub const ICON_CMD: &str = "\x1b[38;2;240;160;40m\u{25B8}\x1b[0m"; // ▸

// ── Semantic formatting helpers ──────────────────────────────────────────────

/// Wrap text with green (success context).
#[must_use]
pub fn success(text: &str) -> String {
    format!("{GREEN}{text}{R}")
}

/// Wrap text with red (error context).
#[must_use]
pub fn error(text: &str) -> String {
    format!("{RED}{text}{R}")
}

/// Wrap text with blue bold (heading context).
#[must_use]
pub fn heading(text: &str) -> String {
    format!("{BLUE}{BOLD}{text}{R}")
}

/// Wrap text with orange (accent/highlight).
#[must_use]
pub fn accent(text: &str) -> String {
    format!("{ORANGE}{text}{R}")
}

/// Wrap text with dim grey (secondary info).
#[must_use]
pub fn dim(text: &str) -> String {
    format!("{DIM}{text}{R}")
}

/// Wrap text with bold white.
#[must_use]
pub fn bright(text: &str) -> String {
    format!("{WHITE}{BOLD}{text}{R}")
}

// ── Terminal width detection ─────────────────────────────────────────────────

/// Get the current terminal width.  Falls back to 80 if detection fails.
#[must_use]
pub fn term_width() -> usize {
    // Try crossterm first, then fallback to environment variable, then 80
    #[cfg(not(test))]
    {
        if let Ok((cols, _)) = crossterm::terminal::size() {
            return cols as usize;
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(80)
}

/// Clamp a box width to fit the terminal, with a minimum of `min_width`.
#[must_use]
pub fn clamped_width(desired: usize, min_width: usize) -> usize {
    let tw = term_width().saturating_sub(2); // leave 2 chars margin
    desired.min(tw).max(min_width)
}

// ── Box drawing helpers (terminal-width-aware) ──────────────────────────────

/// Draw a blue-bordered box top with a title.
///
/// Example: `╭─── Sessions ───────────────────────╮`
#[must_use]
pub fn box_top(title: &str, width: usize) -> String {
    let w = clamped_width(width, 30);
    let inner = w.saturating_sub(2); // inside ╭ and ╮
    let label = format!(" {} ", title);
    let label_len = strip_ansi_len(&label);
    let left_pad = 3; // "───"
    let right_pad = inner.saturating_sub(left_pad + label_len);
    format!(
        "{BLUE}╭{left}{WHITE}{BOLD}{label}{R}{BLUE}{right}╮{R}",
        left = "─".repeat(left_pad),
        label = label,
        right = "─".repeat(right_pad),
    )
}

/// Draw a blue-bordered box bottom.
#[must_use]
pub fn box_bottom(width: usize) -> String {
    let w = clamped_width(width, 30);
    let inner = w.saturating_sub(2);
    format!("{BLUE}╰{dashes}╯{R}", dashes = "─".repeat(inner))
}

/// Draw a blue-bordered box separator.
#[must_use]
pub fn box_separator(width: usize) -> String {
    let w = clamped_width(width, 30);
    let inner = w.saturating_sub(2);
    format!("{BLUE}├{dashes}┤{R}", dashes = "─".repeat(inner))
}

/// Draw a blue-bordered row with content left-aligned, truncated to fit.
#[must_use]
pub fn box_row(content: &str, width: usize) -> String {
    let w = clamped_width(width, 30);
    let inner = w.saturating_sub(2);
    let visible = strip_ansi_len(content);
    // If content is too wide, truncate it
    let display = if visible > inner.saturating_sub(2) {
        truncate_ansi(content, inner.saturating_sub(4))
    } else {
        content.to_string()
    };
    let display_len = strip_ansi_len(&display);
    let padding = inner.saturating_sub(display_len + 1); // 1 for left space
    format!(
        "{BLUE}│{R} {display}{pad}{BLUE}│{R}",
        pad = " ".repeat(padding),
    )
}

/// Draw a blue-bordered empty row.
#[must_use]
pub fn box_empty(width: usize) -> String {
    let w = clamped_width(width, 30);
    let inner = w.saturating_sub(2);
    format!("{BLUE}│{R}{spaces}{BLUE}│{R}", spaces = " ".repeat(inner))
}

/// Compute the visible length of a string (strip ANSI escape codes).
#[must_use]
pub fn strip_ansi_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }
    len
}

/// Truncate an ANSI-colored string to `max_visible` characters,
/// preserving escape codes and appending `…` if truncated.
#[must_use]
pub fn truncate_ansi(s: &str, max_visible: usize) -> String {
    let mut result = String::new();
    let mut visible = 0;
    let mut in_escape = false;

    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
            result.push(ch);
        } else if in_escape {
            result.push(ch);
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            if visible >= max_visible {
                result.push_str(&format!("{DIM}…{R}"));
                break;
            }
            result.push(ch);
            visible += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_len_plain() {
        assert_eq!(strip_ansi_len("hello"), 5);
    }

    #[test]
    fn strip_ansi_len_colored() {
        let colored = format!("{GREEN}hello{R}");
        assert_eq!(strip_ansi_len(&colored), 5);
    }

    #[test]
    fn truncate_ansi_short() {
        assert_eq!(strip_ansi_len(&truncate_ansi("hello world", 5)), 6); // 5 + …
    }

    #[test]
    fn box_top_format() {
        let top = box_top("Sessions", 40);
        assert!(top.contains("Sessions"));
        assert!(top.contains("╭"));
        assert!(top.contains("╮"));
    }
}
