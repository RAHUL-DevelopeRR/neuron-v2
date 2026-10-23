//! The terminal the CLI prints to: whether it draws escape sequences, and which
//! characters its font has.
//!
//! A classic Windows console window (conhost: Windows PowerShell 5.1 and cmd started
//! from the Start menu) shows escape sequences as text (`←[38;2;65;105;195m`) until
//! virtual-terminal processing is switched on for it, and its fonts (Consolas, Lucida
//! Console) have none of ✓ ✗ ⚠ ╭ ▸ ❯ and most of the other marks the CLI draws: they
//! show as `?`. [`init`], first thing in `run()`, switches VT processing on and picks
//! the character set:
//!
//! - VT processing that cannot be switched on (a console in legacy mode, Windows
//!   before 10) means plain text: escape sequences are left out.
//! - In a classic console window the marks become ones its fonts have, from the old
//!   console character set (WGL4/CP437: √ × ► ▼ ┌ ...) or ASCII, the same rule the
//!   full-screen UI follows (`ui/neuron-tui/src/utils/glyphs.ts`). Windows Terminal,
//!   VS Code, the other ConPTY terminals and every terminal elsewhere keep Unicode.
//!   `NEURON_TUI_ASCII=1` forces the console set, `NEURON_TUI_ASCII=0` Unicode.
//!
//! Without FFI (the workspace forbids `unsafe`) the console window's class cannot be
//! asked, so the terminal is told apart by the variables other terminals set.
//!
//! Only streams that are terminals change: a file or a pipe gets exactly what was
//! written. The `println!` family is shadowed for the whole crate (`#[macro_use] mod
//! console` comes first in `main.rs`): text printed with them goes through
//! [`stdout_text`] or [`stderr_text`]. Writers that bypass them (the turn's sinks, the
//! spinner) call those themselves.

use std::borrow::Cow;
use std::fmt;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

/// The characters a terminal can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlyphMode {
    /// Every mark the CLI uses.
    Unicode,
    /// A classic Windows console window's fonts: WGL4/CP437 and ASCII.
    Console,
}

/// `NEURON_TUI_ASCII`: `1` the console set, `0` Unicode (shared with the full-screen UI).
pub(crate) const GLYPH_MODE_ENV: &str = "NEURON_TUI_ASCII";

/// Variables only a terminal other than a classic console window sets for the shells
/// it runs. A console window opened from one of them (`start cmd`) inherits them, so
/// this check can take such a window for a modern terminal (and keeps Unicode there).
const MODERN_TERMINAL_VARS: &[&str] = &[
    "WT_SESSION",   // Windows Terminal
    "TERM_PROGRAM", // VS Code, WezTerm, Hyper, Tabby, mintty, Warp...
    "ConEmuPID",    // ConEmu, Cmder
    "ConEmuANSI",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_LOG",
    "KITTY_WINDOW_ID",
    "TERMINAL_EMULATOR", // JetBrains IDEs
    "SSH_CONNECTION",    // the client's terminal draws
    "SSH_TTY",
];

#[derive(Debug, Clone, Copy)]
struct ConsoleState {
    /// The terminal draws escape sequences.
    ansi: bool,
    glyphs: GlyphMode,
    /// stdout and stderr are terminals whose text needs [`console_text`].
    filter_stdout: bool,
    filter_stderr: bool,
}

static STATE: OnceLock<ConsoleState> = OnceLock::new();

/// Switches VT processing on (Windows) and picks the character set. Call it before
/// anything is printed; later calls change nothing.
pub(crate) fn init() {
    STATE.get_or_init(|| {
        let ansi = enable_escape_sequences();
        let glyphs = glyph_mode_from(|name| std::env::var(name).ok(), cfg!(windows));
        let needs_filter = !ansi || glyphs == GlyphMode::Console;
        ConsoleState {
            ansi,
            glyphs,
            filter_stdout: needs_filter && std::io::stdout().is_terminal(),
            filter_stderr: needs_filter && std::io::stderr().is_terminal(),
        }
    });
}

/// Switches on `ENABLE_VIRTUAL_TERMINAL_PROCESSING` for the console (one mode for
/// stdout and stderr: the console's screen buffer). False when it cannot be, unless
/// `TERM` says the terminal draws them itself (mintty, MSYS2).
#[cfg(windows)]
fn enable_escape_sequences() -> bool {
    crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
fn enable_escape_sequences() -> bool {
    true
}

/// `NEURON_TUI_ASCII` read as a choice, or `None` when it does not decide.
fn glyph_mode_override(value: Option<&str>) -> Option<GlyphMode> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(GlyphMode::Console),
        "0" | "false" | "no" | "off" => Some(GlyphMode::Unicode),
        _ => None,
    }
}

/// The character set for a terminal with environment `var`: the override first; on
/// Windows a classic console window unless a modern terminal's variable (or `TERM`:
/// mintty, Cygwin and MSYS2) is set; elsewhere Unicode, except the Linux virtual
/// console, whose fonts lack the same marks.
pub(crate) fn glyph_mode_from(var: impl Fn(&str) -> Option<String>, windows: bool) -> GlyphMode {
    if let Some(mode) = glyph_mode_override(var(GLYPH_MODE_ENV).as_deref()) {
        return mode;
    }
    let set = |name: &str| var(name).is_some_and(|value| !value.trim().is_empty());
    if !windows {
        return if var("TERM").as_deref() == Some("linux") {
            GlyphMode::Console
        } else {
            GlyphMode::Unicode
        };
    }
    if MODERN_TERMINAL_VARS.iter().any(|name| set(name)) || set("TERM") {
        GlyphMode::Unicode
    } else {
        GlyphMode::Console
    }
}

/// Whether the console character set is in use (no effect before [`init`]).
pub(crate) fn console_glyphs() -> bool {
    STATE
        .get()
        .is_some_and(|state| state.glyphs == GlyphMode::Console)
}

/// Whether stdout's text goes through [`stdout_text`] (used by `println!`).
pub(crate) fn filters_stdout() -> bool {
    STATE.get().is_some_and(|state| state.filter_stdout)
}

/// Whether stderr's text goes through [`stderr_text`] (used by `eprintln!`).
pub(crate) fn filters_stderr() -> bool {
    STATE.get().is_some_and(|state| state.filter_stderr)
}

/// `text` as stdout's terminal can show it (unchanged for a file or a pipe).
pub(crate) fn stdout_text(text: &str) -> Cow<'_, str> {
    match STATE.get() {
        Some(state) if state.filter_stdout => terminal_text(text, state.ansi, state.glyphs),
        _ => Cow::Borrowed(text),
    }
}

/// `text` as stderr's terminal can show it (unchanged for a file or a pipe).
pub(crate) fn stderr_text(text: &str) -> Cow<'_, str> {
    match STATE.get() {
        Some(state) if state.filter_stderr => terminal_text(text, state.ansi, state.glyphs),
        _ => Cow::Borrowed(text),
    }
}

/// `text` for a terminal that draws escape sequences (`ansi`) or not, in `glyphs`.
pub(crate) fn terminal_text(text: &str, ansi: bool, glyphs: GlyphMode) -> Cow<'_, str> {
    let text = if ansi {
        Cow::Borrowed(text)
    } else {
        strip_escapes(text)
    };
    match glyphs {
        GlyphMode::Unicode => text,
        GlyphMode::Console => match console_safe(&text) {
            Cow::Borrowed(_) => text,
            Cow::Owned(safe) => Cow::Owned(safe),
        },
    }
}

/// `text` without terminal escape sequences (colours, cursor moves, titles).
pub(crate) fn strip_escapes(text: &str) -> Cow<'_, str> {
    if !text.contains('\x1b') {
        return Cow::Borrowed(text);
    }
    let mut plain = crate::turn_sink::StripAnsi::new(Vec::with_capacity(text.len()));
    // Writing to a Vec cannot fail, and only whole sequences are removed, so the
    // rest stays valid UTF-8.
    let _ = plain.write_all(text.as_bytes());
    Cow::Owned(String::from_utf8_lossy(&plain.into_inner()).into_owned())
}

/// `text` in characters a classic console window's fonts have: the marks the CLI
/// draws become WGL4/CP437 ones or ASCII, one column for one column where the font has
/// a match. Other text is unchanged.
pub(crate) fn console_safe(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|ch| console_replacement(ch).is_some()) {
        return Cow::Borrowed(text);
    }
    let mut safe = String::with_capacity(text.len());
    for ch in text.chars() {
        match console_replacement(ch) {
            Some(replacement) => safe.push_str(replacement),
            None => safe.push(ch),
        }
    }
    Cow::Owned(safe)
}

/// What a classic console window draws instead of `ch`, when its fonts lack it.
fn console_replacement(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{2713}' | '\u{2714}' => "\u{221a}", // ✓ ✔ -> √
        '\u{2717}' | '\u{2718}' | '\u{2715}' | '\u{2716}' => "\u{00d7}", // ✗ ✘ ✕ ✖ -> ×
        '\u{26a0}' => "!",                     // ⚠
        '\u{fe0f}' => "",                      // emoji presentation selector
        '\u{256d}' => "\u{250c}",              // ╭ -> ┌
        '\u{256e}' => "\u{2510}",              // ╮ -> ┐
        '\u{2570}' | '\u{21b3}' | '\u{23bf}' => "\u{2514}", // ╰ ↳ ⎿ -> └
        '\u{256f}' => "\u{2518}",              // ╯ -> ┘
        '\u{2501}' | '\u{2504}' | '\u{2508}' => "\u{2500}", // ━ ┄ ┈ -> ─
        '\u{2503}' | '\u{2506}' | '\u{250a}' => "\u{2502}", // ┃ ┆ ┊ -> │
        '\u{25b8}' | '\u{25b6}' => "\u{25ba}", // ▸ ▶ -> ►
        '\u{25c2}' | '\u{25c0}' => "\u{25c4}", // ◂ ◀ -> ◄
        '\u{25be}' => "\u{25bc}",              // ▾ -> ▼
        '\u{25b4}' => "\u{25b2}",              // ▴ -> ▲
        '\u{25c6}' | '\u{25c8}' => "\u{2666}", // ◆ ◈ -> ♦
        '\u{25c9}' => "\u{25cf}",              // ◉ -> ●
        '\u{25ce}' => "\u{25cb}",              // ◎ -> ○
        '\u{258d}' => "\u{258c}",              // ▍ -> ▌
        '\u{276f}' => ">",                     // ❯
        '\u{21d2}' => "=>",                    // ⇒
        '\u{23f8}' => "||",                    // ⏸
        '\u{2197}' => "->",                    // ↗
        // ✳ ✶ ✻ ✨, and the spinner's braille frames.
        '\u{2733}' | '\u{2736}' | '\u{273b}' | '\u{2728}' | '\u{2800}'..='\u{28ff}' => "*",
        _ => return None,
    })
}

/// `println!` for this crate (see the module docs).
macro_rules! println {
    () => {
        $crate::console::print_stdout(::std::format_args!(""), true)
    };
    ($($arg:tt)*) => {
        $crate::console::print_stdout(::std::format_args!($($arg)*), true)
    };
}

/// `print!` for this crate (see the module docs).
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::print_stdout(::std::format_args!($($arg)*), false)
    };
}

/// `eprintln!` for this crate (see the module docs).
macro_rules! eprintln {
    () => {
        $crate::console::print_stderr(::std::format_args!(""), true)
    };
    ($($arg:tt)*) => {
        $crate::console::print_stderr(::std::format_args!($($arg)*), true)
    };
}

/// `eprint!` for this crate (see the module docs).
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::console::print_stderr(::std::format_args!($($arg)*), false)
    };
}

/// What the crate's `print!`/`println!` call: std's, through [`stdout_text`] when
/// stdout's terminal needs it. Std's macros keep test output capture working.
#[allow(clippy::print_stdout)]
pub(crate) fn print_stdout(args: fmt::Arguments<'_>, newline: bool) {
    if filters_stdout() {
        let text = args.to_string();
        let text = stdout_text(&text);
        if newline {
            ::std::println!("{text}");
        } else {
            ::std::print!("{text}");
        }
    } else if newline {
        ::std::println!("{args}");
    } else {
        ::std::print!("{args}");
    }
}

/// What the crate's `eprint!`/`eprintln!` call: std's, through [`stderr_text`] when
/// stderr's terminal needs it.
#[allow(clippy::print_stderr)]
pub(crate) fn print_stderr(args: fmt::Arguments<'_>, newline: bool) {
    if filters_stderr() {
        let text = args.to_string();
        let text = stderr_text(&text);
        if newline {
            ::std::eprintln!("{text}");
        } else {
            ::std::eprint!("{text}");
        }
    } else if newline {
        ::std::eprintln!("{args}");
    } else {
        ::std::eprint!("{args}");
    }
}

#[cfg(test)]
mod tests {
    use super::{console_safe, glyph_mode_from, strip_escapes, terminal_text, GlyphMode};
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        move |name: &str| vars.get(name).cloned()
    }

    #[test]
    fn a_windows_console_without_a_modern_terminal_gets_the_console_set() {
        assert_eq!(glyph_mode_from(env(&[]), true), GlyphMode::Console);
        for var in [
            "WT_SESSION",
            "TERM_PROGRAM",
            "TERM",
            "ConEmuPID",
            "WEZTERM_PANE",
            "ALACRITTY_WINDOW_ID",
            "KITTY_WINDOW_ID",
            "TERMINAL_EMULATOR",
            "SSH_CONNECTION",
        ] {
            assert_eq!(
                glyph_mode_from(env(&[(var, "x")]), true),
                GlyphMode::Unicode,
                "{var}"
            );
        }
        // An empty value is not set.
        assert_eq!(
            glyph_mode_from(env(&[("WT_SESSION", " ")]), true),
            GlyphMode::Console
        );
    }

    #[test]
    fn neuron_tui_ascii_overrides_the_detection() {
        assert_eq!(
            glyph_mode_from(env(&[("NEURON_TUI_ASCII", "1"), ("WT_SESSION", "x")]), true),
            GlyphMode::Console
        );
        assert_eq!(
            glyph_mode_from(env(&[("NEURON_TUI_ASCII", "0")]), true),
            GlyphMode::Unicode
        );
        assert_eq!(
            glyph_mode_from(env(&[("NEURON_TUI_ASCII", "yes")]), false),
            GlyphMode::Console
        );
        // A value that does not decide leaves the detection.
        assert_eq!(
            glyph_mode_from(env(&[("NEURON_TUI_ASCII", "maybe")]), true),
            GlyphMode::Console
        );
    }

    #[test]
    fn other_systems_keep_unicode_except_the_linux_virtual_console() {
        assert_eq!(glyph_mode_from(env(&[]), false), GlyphMode::Unicode);
        assert_eq!(
            glyph_mode_from(env(&[("TERM", "xterm-256color")]), false),
            GlyphMode::Unicode
        );
        assert_eq!(
            glyph_mode_from(env(&[("TERM", "linux")]), false),
            GlyphMode::Console
        );
    }

    #[test]
    fn console_text_uses_marks_the_console_fonts_have() {
        assert_eq!(
            console_safe("\u{2713} Connected \u{2717} failed \u{26a0}\u{fe0f} careful"),
            "\u{221a} Connected \u{00d7} failed ! careful"
        );
        assert_eq!(
            console_safe("\u{256d}\u{2500}\u{256e}\n\u{2570}\u{2500}\u{256f}"),
            "\u{250c}\u{2500}\u{2510}\n\u{2514}\u{2500}\u{2518}"
        );
        assert_eq!(
            console_safe("\u{25b8} next \u{276f} \u{2714} Done"),
            "\u{25ba} next > \u{221a} Done"
        );
        // What the fonts have stays, and plain text is not copied.
        let plain = "a \u{2500}\u{2502} \u{00b7} \u{2026} \u{2192} \u{25cf} text";
        assert!(matches!(console_safe(plain), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn a_terminal_without_escape_sequences_gets_plain_text() {
        let line = "\x1b[38;2;45;140;60m\x1b[1m\u{2713}\x1b[0m \x1b[38;2;136;136;136mConnected:\x1b[0m auto";
        assert_eq!(strip_escapes(line), "\u{2713} Connected: auto");
        assert_eq!(
            terminal_text(line, false, GlyphMode::Console),
            "\u{221a} Connected: auto"
        );
        assert_eq!(
            terminal_text(line, true, GlyphMode::Console),
            "\x1b[38;2;45;140;60m\x1b[1m\u{221a}\x1b[0m \x1b[38;2;136;136;136mConnected:\x1b[0m auto"
        );
        assert_eq!(terminal_text(line, true, GlyphMode::Unicode), line);
    }
}
