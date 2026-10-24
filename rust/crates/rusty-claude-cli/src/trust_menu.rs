//! The line REPL's folder trust question, "Before Neuron works here": the folder, what
//! trusting it allows and what its own settings files would turn on, then two choices,
//! Allow or Quit.
//!
//! The words are the full-screen UI's (`ui/neuron-client/src/copy/trust.ts`: `trustCopy`,
//! `trustPreviewLines` and `trustRootNote`), and a test fails when the two drift apart.
//!
//! When stdin and the stream the question is shown on are terminals, it is a menu read
//! in raw mode: 1 or 2 picks a choice, ↑ and ↓ move the marker (stopping at the first and
//! the last choice) and Enter confirms it, Esc and Ctrl-C quit. Otherwise (stdin piped or
//! redirected) the same text without the key hint is printed and a line is read: `1`
//! allows, anything else or nothing does not.
//!
//! Showing the screen reads the folder's settings files and nothing more: no hook runs
//! and no MCP server starts before the user allows the folder.
//!
//! The text goes through `console.rs`'s filter for the stream, like the rest of the
//! CLI's output. A classic Windows console's fonts (Consolas, Lucida Console) have
//! › • – ·, so it leaves them as they are. In the console character set the key hint
//! says `Up/Down` for `↑↓`, as the full-screen UI's does: a classic console read back
//! `??` for them there.

use std::borrow::Cow;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use runtime::ui_hooks::PromptStream;

use crate::console::GlyphMode;

// `trustCopy` in `ui/neuron-client/src/copy/trust.ts`, word for word.
const TITLE: &str = "Before Neuron works here";
const ACCESS: &str = "Neuron will be able to open, change and run files in this folder.";
const TURNS_ON_HEADING: &str = "This project also wants to turn on:";
const TURNS_ON_NOTHING: &str =
    "This project does not turn on any hooks, MCP servers or permission rules.";
const CAUTION: &str = "Continue only if you created this project or trust where it came from.";
const WHY_LABEL: &str = "Why this matters:";
const WHY_LINK: &str = "zero-x.live/docs/trust";
const SESSION_ONLY: &str =
    "This is your home folder, so Neuron trusts it for this session only and asks again next time.";
const ALLOW: &str = "Allow Neuron in this folder";
const QUIT: &str = "Quit";
const TERMINAL_KEYS: &str = "↑↓ or 1–2 to choose · Enter to confirm · Esc to quit";

// The words of `trustPreviewLines` and `trustRootNote` in the same file.
const HOOK_THAT: &str = "hook that";
const HOOKS_THAT: &str = "hooks that";
const RUNS: &str = "runs";
const RUN: &str = "run";
const AROUND_TOOL_CALLS: &str = "commands around tool calls";
const MCP_SERVER: &str = "MCP server";
const MCP_SERVERS: &str = "MCP servers";
const PERMISSION_RULE: &str = "permission rule";
const PERMISSION_RULES: &str = "permission rules";
const FROM: &str = "from";
const SETTINGS_FILES: &str = "settings files";
const OTHER_SETTINGS: &str = "Other settings:";
const GIT_ROOT_THIS_FOLDER: &str = "(git root: this folder)";
const GIT_ROOT: &str = "git root:";

/// Before every line of the screen.
const INDENT: &str = "  ";
/// Between the folder and the note on where trust is recorded.
const ROOT_NOTE_GAP: &str = "        ";
/// The selected choice.
const POINTER: char = '\u{203a}'; // ›
/// Before each thing the project would turn on.
const BULLET: char = '\u{2022}'; // •
/// The arrow keys in [`TERMINAL_KEYS`], and how the console character set says them.
const ARROW_KEYS: &str = "\u{2191}\u{2193}"; // ↑↓
const CONSOLE_ARROW_KEYS: &str = "Up/Down";
/// The width the menu assumes when the terminal's cannot be read.
const DEFAULT_COLUMNS: u16 = 80;

/// Text as the stream's terminal can show it (`console::stdout_text` or `stderr_text`).
type Fit = for<'a> fn(&'a str) -> Cow<'a, str>;

/// One of the two choices, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    Allow,
    Quit,
}

const CHOICES: [Choice; 2] = [Choice::Allow, Choice::Quit];

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Self::Allow => ALLOW,
            Self::Quit => QUIT,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Allow => 0,
            Self::Quit => 1,
        }
    }
}

/// How the trust question was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Allow: in the menu, or `1` on a line.
    Allow,
    /// Quit in the menu (2, Esc or Ctrl-C).
    Quit,
    /// A line other than `1`.
    Declined,
    /// Nothing to read (end of input), or reading failed.
    NoInput,
}

/// What the trust screen shows about a folder: what the engine's `trust_preview` event
/// gives the full-screen UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrustScreen {
    /// The folder, as shown.
    pub(crate) cwd: String,
    /// Where trust is recorded: the git root, or the folder itself outside git.
    pub(crate) root: String,
    pub(crate) is_git_root: bool,
    /// The home folder or a folder above it: trusted for this run only.
    pub(crate) session_only: bool,
    /// Hook commands the folder's settings would run around tool calls.
    pub(crate) hooks: usize,
    /// The MCP servers the folder's settings would start, by name.
    pub(crate) mcp_servers: Vec<String>,
    /// The settings file each allow or ask rule comes from, one entry per rule.
    pub(crate) permission_rule_sources: Vec<String>,
    /// Every other key trusting would apply.
    pub(crate) other_settings: Vec<String>,
}

impl TrustScreen {
    /// The screen for `cwd`, read from its settings files (nothing is run or started).
    pub(crate) fn for_folder(cwd: &Path) -> Self {
        let root = runtime::workspace_trust::trust_root(cwd);
        let preview = runtime::ConfigLoader::default_for(cwd).workspace_trust_preview();
        let shown = |path: &Path| path.display().to_string();
        Self {
            cwd: shown(cwd),
            root: shown(&root.root),
            is_git_root: root.is_git_root,
            session_only: root.session_only,
            hooks: preview.hooks.len(),
            mcp_servers: preview
                .mcp_servers
                .iter()
                .map(|server| server.name.clone())
                .collect(),
            permission_rule_sources: preview
                .permission_rules
                .iter()
                .map(|rule| shown(&rule.source))
                .collect(),
            other_settings: preview.other_settings,
        }
    }

    /// The note beside the folder (`trustRootNote`): which folder trust is recorded for.
    /// Empty outside git, where it is the folder itself.
    pub(crate) fn root_note(&self) -> String {
        if !self.is_git_root {
            return String::new();
        }
        if same_path(&self.root, &self.cwd) {
            GIT_ROOT_THIS_FOLDER.to_string()
        } else {
            format!("({GIT_ROOT} {})", self.root)
        }
    }

    /// What trusting the folder would turn on, one line each (`trustPreviewLines`).
    /// Empty when the project adds nothing.
    pub(crate) fn preview_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.hooks > 0 {
            let verb = if self.hooks == 1 { RUNS } else { RUN };
            lines.push(format!(
                "{} {verb} {AROUND_TOOL_CALLS}",
                plural(self.hooks, HOOK_THAT, HOOKS_THAT)
            ));
        }
        if !self.mcp_servers.is_empty() {
            lines.push(format!(
                "{}: {}",
                plural(self.mcp_servers.len(), MCP_SERVER, MCP_SERVERS),
                self.mcp_servers.join(", ")
            ));
        }
        if !self.permission_rule_sources.is_empty() {
            lines.push(format!(
                "{} {}",
                plural(
                    self.permission_rule_sources.len(),
                    PERMISSION_RULE,
                    PERMISSION_RULES
                ),
                self.rule_files()
            ));
        }
        if !self.other_settings.is_empty() {
            lines.push(format!(
                "{OTHER_SETTINGS} {}",
                self.other_settings.join(", ")
            ));
        }
        lines
    }

    /// "from .neuron/settings.json", or "from 2 settings files".
    fn rule_files(&self) -> String {
        let mut files: Vec<&str> = Vec::new();
        for source in &self.permission_rule_sources {
            if !files.contains(&source.as_str()) {
                files.push(source);
            }
        }
        match files.as_slice() {
            [file] => format!("{FROM} {}", relative_to(&self.cwd, file)),
            _ => format!("{FROM} {} {SETTINGS_FILES}", files.len()),
        }
    }

    /// Everything above the choices, a blank line first.
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = vec![String::new(), format!("{INDENT}{TITLE}"), String::new()];
        let note = self.root_note();
        lines.push(if note.is_empty() {
            format!("{INDENT}{}", self.cwd)
        } else {
            format!("{INDENT}{}{ROOT_NOTE_GAP}{note}", self.cwd)
        });
        if self.session_only {
            lines.push(format!("{INDENT}{SESSION_ONLY}"));
        }
        lines.push(String::new());
        lines.push(format!("{INDENT}{ACCESS}"));
        let preview = self.preview_lines();
        if preview.is_empty() {
            lines.push(format!("{INDENT}{TURNS_ON_NOTHING}"));
        } else {
            lines.push(format!("{INDENT}{TURNS_ON_HEADING}"));
            lines.extend(
                preview
                    .iter()
                    .map(|line| format!("{INDENT}{INDENT}{BULLET} {line}")),
            );
        }
        lines.push(format!("{INDENT}{CAUTION}"));
        lines.push(format!("{INDENT}{WHY_LABEL} {WHY_LINK}"));
        lines.push(String::new());
        lines
    }
}

/// "1 hook that", "2 hooks that".
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A path with forward slashes and no trailing slash.
fn normal(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

/// `path` relative to `base` when it is inside it (letter case aside), with forward
/// slashes.
fn relative_to(base: &str, path: &str) -> String {
    let prefix = format!("{}/", normal(base));
    let full = normal(path);
    match full.get(..prefix.len()) {
        Some(head) if head.to_lowercase() == prefix.to_lowercase() => {
            full[prefix.len()..].to_string()
        }
        _ => full,
    }
}

fn same_path(left: &str, right: &str) -> bool {
    normal(left).to_lowercase() == normal(right).to_lowercase()
}

/// The menu's key hint (`terminalKeys`) in the terminal's character set: `Up/Down` for
/// `↑↓` in the console set.
pub(crate) fn key_hint(glyphs: GlyphMode) -> Cow<'static, str> {
    match glyphs {
        GlyphMode::Unicode => Cow::Borrowed(TERMINAL_KEYS),
        GlyphMode::Console => Cow::Owned(TERMINAL_KEYS.replace(ARROW_KEYS, CONSOLE_ARROW_KEYS)),
    }
}

/// The two choices with the marker on `selected`, and, for the menu, `key_hint` under
/// them after a blank line.
pub(crate) fn choice_lines(selected: usize, key_hint: Option<&str>) -> Vec<String> {
    let mut lines: Vec<String> = CHOICES
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let marker = if index == selected { POINTER } else { ' ' };
            format!("{INDENT}{marker} {}  {}", index + 1, choice.label())
        })
        .collect();
    if let Some(hint) = key_hint {
        lines.push(String::new());
        lines.push(format!("{INDENT}{hint}"));
    }
    lines
}

/// What a key does in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuStep {
    /// Move the marker to this choice.
    Select(usize),
    Choose(Choice),
    Ignore,
}

/// What `key` does with the marker on `selected`. ↑ and ↓ stop at the first and the
/// last choice rather than wrapping around. Key releases (Windows reports them, among
/// them the release of the Enter that started Neuron) do nothing.
pub(crate) fn menu_step(selected: usize, key: &KeyEvent) -> MenuStep {
    let last = CHOICES.len() - 1;
    let selected = selected.min(last);
    let pressed = key.kind == KeyEventKind::Press;
    let held = pressed || key.kind == KeyEventKind::Repeat;
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up if held => MenuStep::Select(selected.saturating_sub(1)),
        KeyCode::Down if held => MenuStep::Select((selected + 1).min(last)),
        KeyCode::Enter if pressed => MenuStep::Choose(CHOICES[selected]),
        KeyCode::Esc if pressed => MenuStep::Choose(Choice::Quit),
        KeyCode::Char('c' | 'C') if pressed && control => MenuStep::Choose(Choice::Quit),
        KeyCode::Char('1') if pressed && !control => MenuStep::Choose(Choice::Allow),
        KeyCode::Char('2') if pressed && !control => MenuStep::Choose(Choice::Quit),
        _ => MenuStep::Ignore,
    }
}

/// `1` allows; another line declines; no line (end of input) or a failed read is no
/// input.
pub(crate) fn line_answer(read: &io::Result<usize>, line: &str) -> Answer {
    match read {
        Ok(0) | Err(_) => Answer::NoInput,
        Ok(_) if line.trim() == "1" => Answer::Allow,
        Ok(_) => Answer::Declined,
    }
}

/// Asks without the menu: writes the screen and the choices (no key hint) to `out`, then
/// reads one line from `input`.
pub(crate) fn ask_line(
    out: &mut dyn Write,
    input: &mut dyn BufRead,
    screen: &TrustScreen,
    fit: Fit,
) -> Answer {
    let mut text = String::new();
    for line in screen.lines().iter().chain(&choice_lines(0, None)) {
        text.push_str(line);
        text.push('\n');
    }
    let _ = out.write_all(fit(&text).as_bytes());
    let _ = out.flush();
    let mut line = String::new();
    let read = input.read_line(&mut line);
    line_answer(&read, &line)
}

/// Asks the trust question about `screen`'s folder on `stream`: the menu when stdin and
/// `stream` are terminals, else (or when raw mode cannot be turned on) a line.
pub(crate) fn ask(stream: PromptStream, screen: &TrustScreen) -> Answer {
    let fit: Fit = match stream {
        PromptStream::Stdout => crate::console::stdout_text,
        PromptStream::Stderr => crate::console::stderr_text,
    };
    let stream_is_terminal = match stream {
        PromptStream::Stdout => io::stdout().is_terminal(),
        PromptStream::Stderr => io::stderr().is_terminal(),
    };
    if runtime::ui_hooks::stdin_is_terminal() && stream_is_terminal {
        if let Ok(raw_mode) = RawMode::enable(stream) {
            let chosen = run_menu(&mut stream.writer(), screen, fit);
            drop(raw_mode);
            return match chosen {
                Ok(Choice::Allow) => Answer::Allow,
                Ok(Choice::Quit) => Answer::Quit,
                Err(_) => Answer::NoInput,
            };
        }
    }
    ask_line(&mut stream.writer(), &mut io::stdin().lock(), screen, fit)
}

/// Draws the menu and reads keys until a choice is made (in raw mode: lines end in
/// `\r\n`). The choices stay on the screen with the marker on the one made.
fn run_menu(out: &mut dyn Write, screen: &TrustScreen, fit: Fit) -> io::Result<Choice> {
    let hint = key_hint(if crate::console::console_glyphs() {
        GlyphMode::Console
    } else {
        GlyphMode::Unicode
    });
    out.queue(cursor::Hide)?;
    for line in screen.lines() {
        out.queue(Print(fit(&line)))?.queue(Print("\r\n"))?;
    }
    let mut selected = 0;
    let mut rows_up = draw(out, &choice_lines(selected, Some(&hint)), 0, fit)?;
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match menu_step(selected, &key) {
            MenuStep::Ignore => {}
            MenuStep::Select(next) if next == selected => {}
            MenuStep::Select(next) => {
                selected = next;
                rows_up = draw(out, &choice_lines(selected, Some(&hint)), rows_up, fit)?;
            }
            MenuStep::Choose(choice) => {
                draw(out, &choice_lines(choice.index(), None), rows_up, fit)?;
                out.queue(Print("\r\n"))?;
                out.flush()?;
                return Ok(choice);
            }
        }
    }
}

/// Draws `lines` over the ones drawn before, which start `rows_up` rows above the
/// cursor, and leaves the cursor at the end of the last one. Returns how many rows
/// above the cursor they start now.
fn draw(out: &mut dyn Write, lines: &[String], rows_up: usize, fit: Fit) -> io::Result<usize> {
    // Never `MoveUp(0)`: terminals read a count of 0 as 1.
    if rows_up > 0 {
        out.queue(cursor::MoveUp(u16::try_from(rows_up).unwrap_or(u16::MAX)))?;
    }
    out.queue(cursor::MoveToColumn(0))?
        .queue(Clear(ClearType::FromCursorDown))?;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.queue(Print("\r\n"))?;
        }
        out.queue(Print(fit(line)))?;
    }
    out.flush()?;
    let columns = terminal::size().map_or(DEFAULT_COLUMNS, |(columns, _)| columns);
    Ok(rows_above_last(lines, columns))
}

/// How many rows above the last of `lines` the first one starts on a terminal
/// `columns` wide, where a longer line wraps.
pub(crate) fn rows_above_last(lines: &[String], columns: u16) -> usize {
    let columns = usize::from(columns.max(1));
    let rows: usize = lines
        .iter()
        .map(|line| line.chars().count().max(1).div_ceil(columns))
        .sum();
    rows.saturating_sub(1)
}

/// Raw mode for the menu. Dropping it shows the cursor again and puts the terminal back
/// as it was, also when drawing or reading keys failed.
struct RawMode {
    stream: PromptStream,
    /// The console's input mode as found (Windows), put back exactly.
    #[cfg(windows)]
    input_mode: Option<u32>,
}

impl RawMode {
    fn enable(stream: PromptStream) -> io::Result<Self> {
        #[cfg(windows)]
        let input_mode = console_input().and_then(|console| console.mode().ok());
        terminal::enable_raw_mode()?;
        Ok(Self {
            stream,
            #[cfg(windows)]
            input_mode,
        })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let mut out = self.stream.writer();
        let _ = out.queue(cursor::Show).and_then(Write::flush);
        let _ = terminal::disable_raw_mode();
        #[cfg(windows)]
        {
            if let (Some(mode), Some(console)) = (self.input_mode, console_input()) {
                let _ = console.set_mode(mode);
            }
        }
    }
}

/// The console's input buffer (whatever stdin is); `None` without a console.
#[cfg(windows)]
fn console_input() -> Option<crossterm_winapi::ConsoleMode> {
    crossterm_winapi::Handle::current_in_handle()
        .ok()
        .map(crossterm_winapi::ConsoleMode::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unchanged(text: &str) -> Cow<'_, str> {
        Cow::Borrowed(text)
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    /// The fake engine's `trust_preview` sample, the one the UI's copy test reads.
    fn sample() -> TrustScreen {
        let fixture =
            workspace_root().join("rust/crates/neuron-tui/fixtures/protocol-v2-events.jsonl");
        let text = std::fs::read_to_string(&fixture).expect("protocol fixture");
        let event: serde_json::Value = text
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|event| event["type"] == "trust_preview")
            .expect("a trust_preview sample");
        let texts = |key: &str, field: &str| -> Vec<String> {
            event[key]
                .as_array()
                .expect(key)
                .iter()
                .map(|item| {
                    let value = if field.is_empty() { item } else { &item[field] };
                    value.as_str().expect(field).to_string()
                })
                .collect()
        };
        TrustScreen {
            cwd: event["cwd"].as_str().expect("cwd").to_string(),
            root: event["root"].as_str().expect("root").to_string(),
            is_git_root: event["is_git_root"].as_bool().expect("is_git_root"),
            session_only: event["session_only"].as_bool().expect("session_only"),
            hooks: event["hooks"].as_array().expect("hooks").len(),
            mcp_servers: texts("mcp_servers", "name"),
            permission_rule_sources: texts("permission_rules", "source"),
            other_settings: texts("other_settings", ""),
        }
    }

    #[test]
    fn lists_what_the_sample_project_would_turn_on() {
        let screen = sample();
        assert_eq!(
            screen.preview_lines(),
            strings(&[
                "2 hooks that run commands around tool calls",
                "1 MCP server: postgres-tools",
                "3 permission rules from .neuron/settings.json",
                "Other settings: env",
            ])
        );
        assert_eq!(screen.root_note(), "(git root: this folder)");
    }

    #[test]
    fn counts_one_of_each_in_the_singular_and_several_files_as_a_number() {
        let screen = TrustScreen {
            cwd: r"C:\work\app\web".into(),
            root: r"C:\work\app".into(),
            is_git_root: true,
            hooks: 1,
            mcp_servers: strings(&["docs", "search"]),
            permission_rule_sources: strings(&[
                r"C:\work\app\web\.neuron\settings.json",
                r"C:\work\app\web\.neuron.json",
            ]),
            ..TrustScreen::default()
        };
        assert_eq!(
            screen.preview_lines(),
            strings(&[
                "1 hook that runs commands around tool calls",
                "2 MCP servers: docs, search",
                "2 permission rules from 2 settings files",
            ])
        );
        assert_eq!(screen.root_note(), r"(git root: C:\work\app)");
        let outside_git = TrustScreen {
            is_git_root: false,
            ..screen.clone()
        };
        assert_eq!(outside_git.root_note(), "");

        // One rule, and several rules from one file: that file, relative to the folder
        // whatever its letter case, with forward slashes.
        let one = TrustScreen {
            hooks: 0,
            mcp_servers: strings(&["docs"]),
            permission_rule_sources: strings(&[r"c:\Work\App\web\.neuron\settings.local.json"]),
            ..screen.clone()
        };
        assert_eq!(
            one.preview_lines(),
            strings(&[
                "1 MCP server: docs",
                "1 permission rule from .neuron/settings.local.json",
            ])
        );
        let outside = TrustScreen {
            permission_rule_sources: strings(&[r"D:\shared\rules.json", r"D:\shared\rules.json"]),
            ..one
        };
        assert_eq!(
            outside.preview_lines()[1],
            "2 permission rules from D:/shared/rules.json"
        );
        // The git root is the folder whatever the slashes and letter case.
        let at_root = TrustScreen {
            cwd: "c:/work/app/".into(),
            ..screen
        };
        assert_eq!(at_root.root_note(), "(git root: this folder)");
    }

    #[test]
    fn the_menu_reads_as_the_spec_with_the_marker_on_allow() {
        let screen = TrustScreen {
            cwd: r"C:\RAHUL\PROJECTS _OF_Rahul\zero-x.live".into(),
            root: r"C:\RAHUL\PROJECTS _OF_Rahul\zero-x.live".into(),
            is_git_root: true,
            session_only: false,
            hooks: 2,
            mcp_servers: strings(&["postgres-tools"]),
            permission_rule_sources: vec![
                r"C:\RAHUL\PROJECTS _OF_Rahul\zero-x.live\.neuron\settings.json".into();
                3
            ],
            other_settings: Vec::new(),
        };
        let mut lines = screen.lines();
        lines.extend(choice_lines(0, Some(&key_hint(GlyphMode::Unicode))));
        let expected = "
  Before Neuron works here

  C:\\RAHUL\\PROJECTS _OF_Rahul\\zero-x.live        (git root: this folder)

  Neuron will be able to open, change and run files in this folder.
  This project also wants to turn on:
    \u{2022} 2 hooks that run commands around tool calls
    \u{2022} 1 MCP server: postgres-tools
    \u{2022} 3 permission rules from .neuron/settings.json
  Continue only if you created this project or trust where it came from.
  Why this matters: zero-x.live/docs/trust

  \u{203a} 1  Allow Neuron in this folder
    2  Quit

  \u{2191}\u{2193} or 1\u{2013}2 to choose \u{b7} Enter to confirm \u{b7} Esc to quit";
        assert_eq!(lines.join("\n"), expected);
    }

    #[test]
    fn a_project_that_adds_nothing_says_so_instead_of_the_list() {
        let screen = TrustScreen {
            cwd: "/home/dev/notes".into(),
            root: "/home/dev/notes".into(),
            ..TrustScreen::default()
        };
        assert!(screen.preview_lines().is_empty());
        let lines = screen.lines();
        assert!(lines.contains(&format!("  {TURNS_ON_NOTHING}")));
        assert!(!lines.iter().any(|line| line.contains(TURNS_ON_HEADING)));
        assert!(!lines.iter().any(|line| line.contains(BULLET)));
        // Outside git there is no note beside the folder.
        assert_eq!(lines[3], "  /home/dev/notes");
    }

    #[test]
    fn the_home_folder_says_it_is_trusted_for_this_session_only() {
        let screen = TrustScreen {
            cwd: r"C:\Users\dev".into(),
            root: r"C:\Users\dev".into(),
            session_only: true,
            ..TrustScreen::default()
        };
        let lines = screen.lines();
        assert_eq!(lines[3], r"  C:\Users\dev");
        assert_eq!(lines[4], format!("  {SESSION_ONLY}"));
        let elsewhere = TrustScreen {
            session_only: false,
            ..screen
        };
        assert!(!elsewhere
            .lines()
            .iter()
            .any(|line| line.contains(SESSION_ONLY)));
    }

    /// A classic Windows console read `↑↓` back as `??`: its character set says Up/Down.
    #[test]
    fn the_console_set_says_up_down_for_the_arrows() {
        assert_eq!(
            key_hint(GlyphMode::Console),
            "Up/Down or 1\u{2013}2 to choose \u{b7} Enter to confirm \u{b7} Esc to quit"
        );
        assert_eq!(key_hint(GlyphMode::Unicode), TERMINAL_KEYS);
        let lines = choice_lines(0, Some(&key_hint(GlyphMode::Console)));
        assert_eq!(
            lines,
            strings(&[
                "  \u{203a} 1  Allow Neuron in this folder",
                "    2  Quit",
                "",
                "  Up/Down or 1\u{2013}2 to choose \u{b7} Enter to confirm \u{b7} Esc to quit",
            ])
        );
        assert!(!lines.concat().contains(['\u{2191}', '\u{2193}']));
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn up_and_down_move_the_marker_and_stop_at_either_end() {
        assert_eq!(menu_step(0, &press(KeyCode::Down)), MenuStep::Select(1));
        assert_eq!(
            menu_step(1, &press(KeyCode::Down)),
            MenuStep::Select(1),
            "no wrap past Quit"
        );
        assert_eq!(menu_step(1, &press(KeyCode::Up)), MenuStep::Select(0));
        assert_eq!(
            menu_step(0, &press(KeyCode::Up)),
            MenuStep::Select(0),
            "no wrap past Allow"
        );
        let held = KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Repeat);
        assert_eq!(menu_step(0, &held), MenuStep::Select(1));
        assert_eq!(
            choice_lines(1, None)[0],
            "    1  Allow Neuron in this folder"
        );
        assert_eq!(choice_lines(1, None)[1], "  \u{203a} 2  Quit");
    }

    #[test]
    fn keys_choose_allow_or_quit() {
        use Choice::{Allow, Quit};
        assert_eq!(
            menu_step(1, &press(KeyCode::Char('1'))),
            MenuStep::Choose(Allow)
        );
        assert_eq!(
            menu_step(0, &press(KeyCode::Char('2'))),
            MenuStep::Choose(Quit)
        );
        assert_eq!(
            menu_step(0, &press(KeyCode::Enter)),
            MenuStep::Choose(Allow)
        );
        assert_eq!(menu_step(1, &press(KeyCode::Enter)), MenuStep::Choose(Quit));
        assert_eq!(menu_step(0, &press(KeyCode::Esc)), MenuStep::Choose(Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(menu_step(0, &ctrl_c), MenuStep::Choose(Quit));
        // A plain c, other keys, and key releases do nothing.
        for key in [
            press(KeyCode::Char('c')),
            press(KeyCode::Char('3')),
            press(KeyCode::Char('y')),
            press(KeyCode::Tab),
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release),
            KeyEvent::new_with_kind(
                KeyCode::Char('1'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ),
        ] {
            assert_eq!(menu_step(0, &key), MenuStep::Ignore, "{key:?}");
        }
    }

    #[test]
    fn redrawing_the_choices_counts_wrapped_rows() {
        let lines = choice_lines(0, Some(&key_hint(GlyphMode::Unicode)));
        assert_eq!(rows_above_last(&lines, 120), 3);
        // The key hint (54 columns) takes two rows at 40 columns, and one at 54.
        assert_eq!(lines[3].chars().count(), 54);
        assert_eq!(rows_above_last(&lines, 40), 4);
        assert_eq!(rows_above_last(&lines, 54), 3);
        assert_eq!(rows_above_last(&choice_lines(0, None), 80), 1);
        assert_eq!(rows_above_last(&[], 80), 0);
        // The console set's hint is 59 columns.
        let console = choice_lines(0, Some(&key_hint(GlyphMode::Console)));
        assert_eq!(console[3].chars().count(), 59);
        assert_eq!(rows_above_last(&console, 58), 4);
        assert_eq!(
            rows_above_last(&lines, 0),
            rows_above_last(&lines, 1),
            "a width of 0 counts as 1"
        );
    }

    #[test]
    fn a_piped_answer_is_read_from_one_line() {
        let screen = sample();
        let ask = |input: &str| {
            let mut out = Vec::new();
            let answer = ask_line(&mut out, &mut input.as_bytes(), &screen, unchanged);
            (answer, String::from_utf8(out).expect("utf-8"))
        };
        let (answer, shown) = ask("1\n");
        assert_eq!(answer, Answer::Allow);
        assert!(shown.contains(TITLE));
        assert!(shown.contains("\u{2022} 1 MCP server: postgres-tools"));
        assert!(shown.ends_with("  \u{203a} 1  Allow Neuron in this folder\n    2  Quit\n"));
        assert!(
            !shown.contains("Esc to quit"),
            "no key hint without the menu"
        );
        assert_eq!(ask(" 1 \r\n").0, Answer::Allow);
        assert_eq!(ask("2\n").0, Answer::Declined);
        assert_eq!(ask("yes\n").0, Answer::Declined);
        assert_eq!(ask("\n").0, Answer::Declined);
        assert_eq!(ask("").0, Answer::NoInput, "end of input");
        assert_eq!(
            line_answer(&Err(io::Error::other("closed")), ""),
            Answer::NoInput
        );
    }

    /// The trust screen's words live in `ui/neuron-client/src/copy/trust.ts` for both
    /// screens; this fails when a string here is not there word for word.
    #[test]
    fn the_copy_matches_the_ui_word_for_word() {
        let path = workspace_root().join("ui/neuron-client/src/copy/trust.ts");
        let copy = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        for (key, value) in [
            ("title", TITLE),
            ("access", ACCESS),
            ("turnsOnHeading", TURNS_ON_HEADING),
            ("turnsOnNothing", TURNS_ON_NOTHING),
            ("caution", CAUTION),
            ("whyLabel", WHY_LABEL),
            ("whyLink", WHY_LINK),
            ("sessionOnly", SESSION_ONLY),
            ("allow", ALLOW),
            ("quit", QUIT),
            ("terminalKeys", TERMINAL_KEYS),
        ] {
            let entry = format!("{key}: '{value}'");
            assert!(copy.contains(&entry), "trust.ts has no `{entry}`");
        }
        // The key hint as each character set shows it: `terminalKeys`, and in the
        // console set the same with Up/Down for the arrows.
        let terminal_keys = copy
            .split_once("terminalKeys: '")
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(value, _)| value)
            .expect("terminalKeys in trust.ts");
        assert_eq!(key_hint(GlyphMode::Unicode), terminal_keys);
        assert_eq!(
            key_hint(GlyphMode::Console),
            terminal_keys.replace("\u{2191}\u{2193}", "Up/Down")
        );
        for quoted in [
            HOOK_THAT,
            HOOKS_THAT,
            RUNS,
            RUN,
            MCP_SERVER,
            MCP_SERVERS,
            PERMISSION_RULE,
            PERMISSION_RULES,
            GIT_ROOT_THIS_FOLDER,
        ] {
            let entry = format!("'{quoted}'");
            assert!(copy.contains(&entry), "trust.ts has no {entry}");
        }
        for template in [
            format!("}} {AROUND_TOOL_CALLS}`"),
            format!("`{FROM} ${{"),
            format!("}} {SETTINGS_FILES}`"),
            format!("`{OTHER_SETTINGS} ${{"),
            format!("`({GIT_ROOT} ${{"),
            "join(', ')".to_string(),
        ] {
            assert!(copy.contains(&template), "trust.ts has no {template}");
        }
    }

    #[test]
    fn the_copy_names_no_other_product() {
        for text in [
            TITLE,
            ACCESS,
            TURNS_ON_HEADING,
            TURNS_ON_NOTHING,
            CAUTION,
            WHY_LABEL,
            WHY_LINK,
            SESSION_ONLY,
            ALLOW,
            QUIT,
            TERMINAL_KEYS,
        ] {
            let lower = text.to_lowercase();
            assert!(
                !lower.contains("claude") && !lower.contains("anthropic"),
                "{text}"
            );
        }
    }
}
