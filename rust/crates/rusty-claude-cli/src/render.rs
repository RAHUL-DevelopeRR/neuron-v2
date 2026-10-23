use std::borrow::Cow;
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTheme {
    heading: Color,
    emphasis: Color,
    strong: Color,
    inline_code: Color,
    link: Color,
    quote: Color,
    table_border: Color,
    code_block_border: Color,
    spinner_active: Color,
    spinner_done: Color,
    spinner_failed: Color,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            heading: Color::Cyan,
            emphasis: Color::Magenta,
            strong: Color::Yellow,
            inline_code: Color::Green,
            link: Color::Blue,
            quote: Color::DarkGrey,
            table_border: Color::DarkCyan,
            code_block_border: Color::DarkGrey,
            spinner_active: Color::Blue,
            spinner_done: Color::Green,
            spinner_failed: Color::Red,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Spinner {
    frame_index: usize,
    /// For output that is not a terminal: no animation, and the end lines as text.
    plain: bool,
}

impl Spinner {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// For a file or a pipe: [`Self::tick`] draws nothing, and the end lines carry no
    /// colours or cursor moves.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            plain: true,
            ..Self::default()
        }
    }

    pub fn tick(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        if self.plain {
            return Ok(());
        }
        let frame = Self::FRAMES[self.frame_index % Self::FRAMES.len()];
        self.frame_index += 1;
        queue!(
            out,
            SavePosition,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_active),
            Print(format!("{frame} {label}")),
            ResetColor,
            RestorePosition
        )?;
        out.flush()
    }

    pub fn finish(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        if self.plain {
            writeln!(out, "\u{2714} {label}")?;
            return out.flush();
        }
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_done),
            Print(format!("✔ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }

    pub fn fail(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        if self.plain {
            writeln!(out, "\u{2718} {label}")?;
            return out.flush();
        }
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_failed),
            Print(format!("✘ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered { next_index: u64 },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl TableState {
    fn push_cell(&mut self) {
        let cell = self.current_cell.trim().to_string();
        self.current_row.push(cell);
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if self.current_row.is_empty() {
            return;
        }
        let row = std::mem::take(&mut self.current_row);
        if self.in_head {
            self.headers = row;
        } else {
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RenderState {
    emphasis: usize,
    strong: usize,
    heading_level: Option<u8>,
    quote: usize,
    list_stack: Vec<ListKind>,
    link_stack: Vec<LinkState>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkState {
    destination: String,
    text: String,
}

impl RenderState {
    fn style_text(&self, text: &str, theme: &ColorTheme) -> String {
        let mut style = text.stylize();

        if matches!(self.heading_level, Some(1 | 2)) || self.strong > 0 {
            style = style.bold();
        }
        if self.emphasis > 0 {
            style = style.italic();
        }

        if let Some(level) = self.heading_level {
            style = match level {
                1 => style.with(theme.heading),
                2 => style.white(),
                3 => style.with(Color::Blue),
                _ => style.with(Color::Grey),
            };
        } else if self.strong > 0 {
            style = style.with(theme.strong);
        } else if self.emphasis > 0 {
            style = style.with(theme.emphasis);
        }

        if self.quote > 0 {
            style = style.with(theme.quote);
        }

        format!("{style}")
    }

    fn append_raw(&mut self, output: &mut String, text: &str) {
        if let Some(link) = self.link_stack.last_mut() {
            link.text.push_str(text);
        } else if let Some(table) = self.table.as_mut() {
            table.current_cell.push_str(text);
        } else {
            output.push_str(text);
        }
    }

    fn append_styled(&mut self, output: &mut String, text: &str, theme: &ColorTheme) {
        let styled = self.style_text(text, theme);
        self.append_raw(output, &styled);
    }
}

#[derive(Debug)]
pub struct TerminalRenderer {
    syntax_set: SyntaxSet,
    syntax_theme: Theme,
    color_theme: ColorTheme,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        Self {
            syntax_set,
            syntax_theme,
            color_theme: ColorTheme::default(),
        }
    }
}

impl TerminalRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color_theme(&self) -> &ColorTheme {
        &self.color_theme
    }

    /// `markdown` as ANSI text, without trailing whitespace: a caller that prints it
    /// ends the line itself (see [`MarkdownStreamState`]).
    #[must_use]
    pub fn render_markdown(&self, markdown: &str) -> String {
        let normalized = normalize_nested_fences(markdown);
        let normalized = keep_path_backslashes(&normalized);
        let mut output = String::new();
        let mut state = RenderState::default();
        let mut code_language = String::new();
        let mut code_buffer = String::new();
        let mut in_code_block = false;

        for event in Parser::new_ext(&normalized, markdown_options()) {
            self.render_event(
                event,
                &mut state,
                &mut output,
                &mut code_buffer,
                &mut code_language,
                &mut in_code_block,
            );
        }

        break_inline_bullets(output.trim_end())
    }

    #[must_use]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        self.render_markdown(markdown)
    }

    #[allow(clippy::too_many_lines)]
    fn render_event(
        &self,
        event: Event<'_>,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        code_language: &mut String,
        in_code_block: &mut bool,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                Self::start_heading(state, level as u8, output);
            }
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(..)) => self.start_quote(state, output),
            Event::End(TagEnd::BlockQuote(..)) => {
                state.quote = state.quote.saturating_sub(1);
                output.push('\n');
            }
            Event::End(TagEnd::Heading(..)) => {
                state.heading_level = None;
                output.push_str("\n\n");
            }
            // After a nested list the item's line has ended already.
            Event::End(TagEnd::Item) if output.ends_with('\n') => {}
            Event::End(TagEnd::Item) | Event::SoftBreak | Event::HardBreak => {
                state.append_raw(output, "\n");
            }
            Event::Start(Tag::List(first_item)) => {
                let kind = match first_item {
                    Some(index) => ListKind::Ordered { next_index: index },
                    None => ListKind::Unordered,
                };
                state.list_stack.push(kind);
            }
            Event::End(TagEnd::List(..)) => {
                state.list_stack.pop();
                // A nested list ends inside its parent item: no blank line.
                if state.list_stack.is_empty() {
                    output.push('\n');
                }
            }
            Event::Start(Tag::Item) => Self::start_item(state, output),
            Event::Start(Tag::CodeBlock(kind)) => {
                *in_code_block = true;
                *code_language = match kind {
                    CodeBlockKind::Indented => String::from("text"),
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                };
                code_buffer.clear();
                self.start_code_block(code_language, output);
            }
            Event::End(TagEnd::CodeBlock) => {
                self.finish_code_block(code_buffer, code_language, output);
                *in_code_block = false;
                code_language.clear();
                code_buffer.clear();
            }
            Event::Start(Tag::Emphasis) => state.emphasis += 1,
            Event::End(TagEnd::Emphasis) => state.emphasis = state.emphasis.saturating_sub(1),
            Event::Start(Tag::Strong) => state.strong += 1,
            Event::End(TagEnd::Strong) => state.strong = state.strong.saturating_sub(1),
            Event::Code(code) => {
                let rendered =
                    format!("{}", format!("`{code}`").with(self.color_theme.inline_code));
                state.append_raw(output, &rendered);
            }
            Event::Rule => output.push_str("---\n"),
            Event::Text(text) => {
                self.push_text(text.as_ref(), state, output, code_buffer, *in_code_block);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                state.append_raw(output, &html);
            }
            Event::FootnoteReference(reference) => {
                state.append_raw(output, &format!("[{reference}]"));
            }
            Event::TaskListMarker(done) => {
                state.append_raw(output, if done { "[x] " } else { "[ ] " });
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                state.append_raw(output, &math);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                state.link_stack.push(LinkState {
                    destination: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = state.link_stack.pop() {
                    let label = if link.text.is_empty() {
                        link.destination.clone()
                    } else {
                        link.text
                    };
                    let rendered = format!(
                        "{}",
                        format!("[{label}]({})", link.destination)
                            .underlined()
                            .with(self.color_theme.link)
                    );
                    state.append_raw(output, &rendered);
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let rendered = format!(
                    "{}",
                    format!("[image:{dest_url}]").with(self.color_theme.link)
                );
                state.append_raw(output, &rendered);
            }
            Event::Start(Tag::Table(..)) => state.table = Some(TableState::default()),
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    output.push_str(&self.render_table(&table));
                    output.push_str("\n\n");
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                    table.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_row.clear();
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.push_cell();
                }
            }
            Event::Start(Tag::Paragraph | Tag::MetadataBlock(..) | _)
            | Event::End(TagEnd::Image | TagEnd::MetadataBlock(..) | _) => {}
        }
    }

    fn start_heading(state: &mut RenderState, level: u8, output: &mut String) {
        state.heading_level = Some(level);
        if !output.is_empty() {
            output.push('\n');
        }
    }

    fn start_quote(&self, state: &mut RenderState, output: &mut String) {
        state.quote += 1;
        let _ = write!(output, "{}", "│ ".with(self.color_theme.quote));
    }

    fn start_item(state: &mut RenderState, output: &mut String) {
        let depth = state.list_stack.len().saturating_sub(1);
        // A nested list starts right after its parent item's text: on a line of its own.
        if depth > 0 && !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&"  ".repeat(depth));

        let marker = match state.list_stack.last_mut() {
            Some(ListKind::Ordered { next_index }) => {
                let value = *next_index;
                *next_index += 1;
                format!("{value}. ")
            }
            _ => "• ".to_string(),
        };
        output.push_str(&marker);
    }

    fn start_code_block(&self, code_language: &str, output: &mut String) {
        let label = if code_language.is_empty() {
            "code".to_string()
        } else {
            code_language.to_string()
        };
        let _ = writeln!(
            output,
            "{}",
            format!("╭─ {label}")
                .bold()
                .with(self.color_theme.code_block_border)
        );
    }

    fn finish_code_block(&self, code_buffer: &str, code_language: &str, output: &mut String) {
        output.push_str(&self.highlight_code(code_buffer, code_language));
        let _ = write!(
            output,
            "{}",
            "╰─".bold().with(self.color_theme.code_block_border)
        );
        output.push_str("\n\n");
    }

    fn push_text(
        &self,
        text: &str,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        in_code_block: bool,
    ) {
        if in_code_block {
            code_buffer.push_str(text);
        } else {
            state.append_styled(output, text, &self.color_theme);
        }
    }

    fn render_table(&self, table: &TableState) -> String {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.iter().cloned());

        if rows.is_empty() {
            return String::new();
        }

        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| visible_width(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();

        let border = format!("{}", "│".with(self.color_theme.table_border));
        let separator = widths
            .iter()
            .map(|width| "─".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join(&format!("{}", "┼".with(self.color_theme.table_border)));
        let separator = format!("{border}{separator}{border}");

        let mut output = String::new();
        if !table.headers.is_empty() {
            output.push_str(&self.render_table_row(&table.headers, &widths, true));
            output.push('\n');
            output.push_str(&separator);
            if !table.rows.is_empty() {
                output.push('\n');
            }
        }

        for (index, row) in table.rows.iter().enumerate() {
            output.push_str(&self.render_table_row(row, &widths, false));
            if index + 1 < table.rows.len() {
                output.push('\n');
            }
        }

        output
    }

    fn render_table_row(&self, row: &[String], widths: &[usize], is_header: bool) -> String {
        let border = format!("{}", "│".with(self.color_theme.table_border));
        let mut line = String::new();
        line.push_str(&border);

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            line.push(' ');
            if is_header {
                let _ = write!(line, "{}", cell.bold().with(self.color_theme.heading));
            } else {
                line.push_str(cell);
            }
            let padding = width.saturating_sub(visible_width(cell));
            line.push_str(&" ".repeat(padding + 1));
            line.push_str(&border);
        }

        line
    }

    #[must_use]
    pub fn highlight_code(&self, code: &str, language: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut syntax_highlighter = HighlightLines::new(syntax, &self.syntax_theme);
        let mut colored_output = String::new();

        for line in LinesWithEndings::from(code) {
            match syntax_highlighter.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    colored_output.push_str(&apply_code_block_background(&escaped));
                }
                Err(_) => colored_output.push_str(&apply_code_block_background(line)),
            }
        }

        colored_output
    }
}

/// No smart punctuation: `[...values]` must not turn into `[…values]`.
fn markdown_options() -> Options {
    Options::all().difference(Options::ENABLE_SMART_PUNCTUATION)
}

/// Renders a streamed text block as complete lines arrive. Each chunk is a Markdown
/// document of its own whose rendering drops the line breaks around it, so they are put
/// back here: every printed chunk ends its line, and a blank line in the source stays one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownStreamState {
    pending: String,
    /// Part of the current block was printed.
    started: bool,
    /// The source has a blank line after what was printed. It goes out before the next
    /// chunk, so a block never ends on an empty line.
    blank_line_owed: bool,
}

impl MarkdownStreamState {
    #[must_use]
    pub fn push(&mut self, renderer: &TerminalRenderer, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        let split = find_stream_safe_boundary(&self.pending)?;
        let ready = self.pending.drain(..split).collect::<String>();
        self.render_chunk(renderer, &ready)
    }

    /// Ends the block: renders what is still buffered. Whatever this block printed ends
    /// with a line break.
    #[must_use]
    pub fn flush(&mut self, renderer: &TerminalRenderer) -> Option<String> {
        let pending = std::mem::take(&mut self.pending);
        let rendered = self.render_chunk(renderer, &pending);
        self.started = false;
        self.blank_line_owed = false;
        rendered
    }

    /// A complete block: what is still buffered and `markdown`, as one document.
    #[must_use]
    pub fn render_block(&mut self, renderer: &TerminalRenderer, markdown: &str) -> Option<String> {
        self.pending.push_str(markdown);
        self.flush(renderer)
    }

    fn render_chunk(&mut self, renderer: &TerminalRenderer, source: &str) -> Option<String> {
        let rendered = renderer.markdown_to_ansi(source);
        let body = rendered.trim_start_matches('\n');
        if body.is_empty() {
            // Blank lines: a paragraph break, once more of the block follows.
            self.blank_line_owed |= self.started && ends_with_blank_line(source);
            return None;
        }
        let mut chunk = String::with_capacity(body.len() + 2);
        if self.started && (self.blank_line_owed || starts_with_blank_line(source)) {
            chunk.push('\n');
        }
        chunk.push_str(body);
        chunk.push('\n');
        self.started = true;
        self.blank_line_owed = ends_with_blank_line(source);
        Some(chunk)
    }
}

/// The first line of `source` is complete and blank.
fn starts_with_blank_line(source: &str) -> bool {
    source
        .split_inclusive('\n')
        .next()
        .is_some_and(|line| line.ends_with('\n') && line.trim().is_empty())
}

/// The last line of `source` is complete and blank.
fn ends_with_blank_line(source: &str) -> bool {
    source
        .strip_suffix('\n')
        .is_some_and(|text| text.rsplit('\n').next().unwrap_or(text).trim().is_empty())
}

/// `CommonMark` reads the `\.` in `C:\Users\x\.neuron` as an escaped `.` and drops the
/// backslash. In words that look like paths, every backslash is doubled first so the
/// path prints as written; elsewhere `\*`, `1\.` and the other escapes still work. Code,
/// HTML, math and autolinks are left alone: their backslashes are literal already.
fn keep_path_backslashes(markdown: &str) -> Cow<'_, str> {
    if !markdown.contains('\\') {
        return Cow::Borrowed(markdown);
    }
    let mut literal = Parser::new_ext(markdown, markdown_options())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Start(
                Tag::CodeBlock(_)
                | Tag::Link {
                    link_type: LinkType::Autolink | LinkType::Email,
                    ..
                },
            ) => Some(range),
            _ => None,
        })
        .collect::<Vec<_>>();
    literal.sort_by_key(|range| range.start);

    let mut kept = String::with_capacity(markdown.len() + 8);
    let mut cursor = 0;
    for range in literal {
        let start = range.start.max(cursor);
        let end = range.end.max(start);
        if start == end {
            continue;
        }
        double_path_backslashes(&markdown[cursor..start], &mut kept);
        kept.push_str(&markdown[start..end]);
        cursor = end;
    }
    double_path_backslashes(&markdown[cursor..], &mut kept);
    if kept == markdown {
        Cow::Borrowed(markdown)
    } else {
        Cow::Owned(kept)
    }
}

/// Copies `text` to `out`, doubling the backslashes of its path-like words (not one
/// before a backtick, which would open a code span).
fn double_path_backslashes(text: &str, out: &mut String) {
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        if !is_path_like(word) {
            out.push_str(piece);
            continue;
        }
        let mut chars = word.chars().peekable();
        while let Some(ch) = chars.next() {
            out.push(ch);
            if ch == '\\' && chars.peek() != Some(&'`') {
                out.push('\\');
            }
        }
        out.push_str(&piece[word.len()..]);
    }
}

/// A word with a backslash that separates path parts rather than escaping Markdown: one
/// before a letter or digit (`C:\Users`, `\\server`), before a dot file (`x\.neuron`,
/// `.\.git`), or after a drive (`C:\`).
fn is_path_like(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.iter().enumerate().any(|(index, &byte)| {
        if byte != b'\\' {
            return false;
        }
        match bytes.get(index + 1) {
            Some(next) if next.is_ascii_alphanumeric() => true,
            Some(b'.') => index > 0 && bytes.get(index + 2).is_some_and(u8::is_ascii_alphanumeric),
            _ => {
                index >= 2
                    && bytes[index - 1] == b':'
                    && bytes[index - 2].is_ascii_alphabetic()
                    && (index == 2 || !bytes[index - 3].is_ascii_alphanumeric())
            }
        }
    })
}

fn apply_code_block_background(line: &str) -> String {
    let trimmed = line.trim_end_matches('\n');
    let trailing_newline = if trimmed.len() == line.len() {
        ""
    } else {
        "\n"
    };
    let with_background = trimmed.replace("\u{1b}[0m", "\u{1b}[0;48;5;236m");
    format!("\u{1b}[48;5;236m{with_background}\u{1b}[0m{trailing_newline}")
}

/// Pre-process raw markdown so that fenced code blocks whose body contains
/// fence markers of equal or greater length are wrapped with a longer fence.
///
/// LLMs frequently emit triple-backtick code blocks that contain triple-backtick
/// examples.  `CommonMark` (and pulldown-cmark) treats the inner marker as the
/// closing fence, breaking the render.  This function detects the situation and
/// upgrades the outer fence to use enough backticks (or tildes) that the inner
/// markers become ordinary content.
#[allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::manual_repeat_n,
    clippy::manual_str_repeat
)]
fn normalize_nested_fences(markdown: &str) -> String {
    // A fence line is either "labeled" (has an info string ⇒ always an opener)
    // or "bare" (no info string ⇒ could be opener or closer).
    #[derive(Debug, Clone)]
    struct FenceLine {
        char: char,
        len: usize,
        has_info: bool,
        indent: usize,
    }

    fn parse_fence_line(line: &str) -> Option<FenceLine> {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        let indent = trimmed.chars().take_while(|c| *c == ' ').count();
        if indent > 3 {
            return None;
        }
        let rest = &trimmed[indent..];
        let ch = rest.chars().next()?;
        if ch != '`' && ch != '~' {
            return None;
        }
        let len = rest.chars().take_while(|c| *c == ch).count();
        if len < 3 {
            return None;
        }
        let after = &rest[len..];
        if ch == '`' && after.contains('`') {
            return None;
        }
        let has_info = !after.trim().is_empty();
        Some(FenceLine {
            char: ch,
            len,
            has_info,
            indent,
        })
    }

    let lines: Vec<&str> = markdown.split_inclusive('\n').collect();
    // Handle final line that may lack trailing newline.
    // split_inclusive already keeps the original chunks, including a
    // final chunk without '\n' if the input doesn't end with one.

    // First pass: classify every line.
    let fence_info: Vec<Option<FenceLine>> = lines.iter().map(|l| parse_fence_line(l)).collect();

    // Second pass: pair openers with closers using a stack, recording
    // (opener_idx, closer_idx) pairs plus the max fence length found between
    // them.
    struct StackEntry {
        line_idx: usize,
        fence: FenceLine,
    }

    let mut stack: Vec<StackEntry> = Vec::new();
    // Paired blocks: (opener_line, closer_line, max_inner_fence_len)
    let mut pairs: Vec<(usize, usize, usize)> = Vec::new();

    for (i, fi) in fence_info.iter().enumerate() {
        let Some(fl) = fi else { continue };

        if fl.has_info {
            // Labeled fence ⇒ always an opener.
            stack.push(StackEntry {
                line_idx: i,
                fence: fl.clone(),
            });
        } else {
            // Bare fence ⇒ try to close the top of the stack if compatible.
            let closes_top = stack
                .last()
                .is_some_and(|top| top.fence.char == fl.char && fl.len >= top.fence.len);
            if closes_top {
                let opener = stack.pop().unwrap();
                // Find max fence length of any fence line strictly between
                // opener and closer (these are the nested fences).
                let inner_max = fence_info[opener.line_idx + 1..i]
                    .iter()
                    .filter_map(|fi| fi.as_ref().map(|f| f.len))
                    .max()
                    .unwrap_or(0);
                pairs.push((opener.line_idx, i, inner_max));
            } else {
                // Treat as opener.
                stack.push(StackEntry {
                    line_idx: i,
                    fence: fl.clone(),
                });
            }
        }
    }

    // Determine which lines need rewriting.  A pair needs rewriting when
    // its opener length <= max inner fence length.
    struct Rewrite {
        char: char,
        new_len: usize,
        indent: usize,
    }
    let mut rewrites: std::collections::HashMap<usize, Rewrite> = std::collections::HashMap::new();

    for (opener_idx, closer_idx, inner_max) in &pairs {
        let opener_fl = fence_info[*opener_idx].as_ref().unwrap();
        if opener_fl.len <= *inner_max {
            let new_len = inner_max + 1;
            let info_part = {
                let trimmed = lines[*opener_idx]
                    .trim_end_matches('\n')
                    .trim_end_matches('\r');
                let rest = &trimmed[opener_fl.indent..];
                rest[opener_fl.len..].to_string()
            };
            rewrites.insert(
                *opener_idx,
                Rewrite {
                    char: opener_fl.char,
                    new_len,
                    indent: opener_fl.indent,
                },
            );
            let closer_fl = fence_info[*closer_idx].as_ref().unwrap();
            rewrites.insert(
                *closer_idx,
                Rewrite {
                    char: closer_fl.char,
                    new_len,
                    indent: closer_fl.indent,
                },
            );
            // Store info string only in the opener; closer keeps the trailing
            // portion which is already handled through the original line.
            // Actually, we rebuild both lines from scratch below, including
            // the info string for the opener.
            let _ = info_part; // consumed in rebuild
        }
    }

    if rewrites.is_empty() {
        return markdown.to_string();
    }

    // Rebuild.
    let mut out = String::with_capacity(markdown.len() + rewrites.len() * 4);
    for (i, line) in lines.iter().enumerate() {
        if let Some(rw) = rewrites.get(&i) {
            let fence_str: String = std::iter::repeat(rw.char).take(rw.new_len).collect();
            let indent_str: String = std::iter::repeat(' ').take(rw.indent).collect();
            // Recover the original info string (if any) and trailing newline.
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let fi = fence_info[i].as_ref().unwrap();
            let info = &trimmed[fi.indent + fi.len..];
            let trailing = &line[trimmed.len()..];
            out.push_str(&indent_str);
            out.push_str(&fence_str);
            out.push_str(info);
            out.push_str(trailing);
        } else {
            out.push_str(line);
        }
    }
    out
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut open_fence: Option<FenceMarker> = None;
    let mut last_boundary = None;
    // In a list, whether a boundary is safe depends on the next non-blank line: an
    // indented one (a nested item, an item's paragraph or code) belongs to the item
    // before it, and rendered on its own it would lose its nesting. `waiting` is the
    // latest boundary since the list's last line, until that line arrives.
    let mut in_list = false;
    let mut waiting: Option<usize> = None;
    let mut fence_in_list = false;

    for (offset, line) in markdown.split_inclusive('\n').scan(0usize, |cursor, line| {
        let start = *cursor;
        *cursor += line.len();
        Some((start, line))
    }) {
        let line_without_newline = line.trim_end_matches('\n');
        let end = offset + line.len();
        if let Some(opener) = open_fence {
            if line_closes_fence(line_without_newline, opener) {
                open_fence = None;
                if fence_in_list {
                    waiting = Some(end);
                } else {
                    last_boundary = Some(end);
                }
            }
            continue;
        }

        let blank = line_without_newline.trim().is_empty();
        if !blank {
            let indented = line_without_newline.starts_with([' ', '\t']);
            if let Some(boundary) = waiting.take() {
                if !indented {
                    last_boundary = Some(boundary);
                }
            }
            in_list = indented || is_list_item_line(line_without_newline);
        }

        if let Some(opener) = parse_fence_opener(line_without_newline) {
            open_fence = Some(opener);
            fence_in_list = in_list;
            continue;
        }

        // For complete lines, blank or not, a split point: short responses render
        // incrementally instead of being buffered until flush.
        if line.ends_with('\n') {
            if in_list {
                waiting = Some(end);
            } else {
                last_boundary = Some(end);
            }
        }
    }

    last_boundary
}

/// `- item`, `* item`, `+ item`, `1. item` or `1) item`.
fn is_list_item_line(line: &str) -> bool {
    let rest = line.trim_start();
    if rest.starts_with(['-', '*', '+']) {
        return rest[1..].is_empty() || rest[1..].starts_with([' ', '\t']);
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    (1..=9).contains(&digits)
        && rest[digits..].starts_with(['.', ')'])
        && (rest[digits + 1..].is_empty() || rest[digits + 1..].starts_with([' ', '\t']))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FenceMarker {
    character: char,
    length: usize,
}

fn parse_fence_opener(line: &str) -> Option<FenceMarker> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let character = rest.chars().next()?;
    if character != '`' && character != '~' {
        return None;
    }
    let length = rest.chars().take_while(|c| *c == character).count();
    if length < 3 {
        return None;
    }
    let info_string = &rest[length..];
    if character == '`' && info_string.contains('`') {
        return None;
    }
    Some(FenceMarker { character, length })
}

fn line_closes_fence(line: &str, opener: FenceMarker) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let length = rest.chars().take_while(|c| *c == opener.character).count();
    if length < opener.length {
        return false;
    }
    rest[length..].chars().all(|c| c == ' ' || c == '\t')
}

fn visible_width(input: &str) -> usize {
    strip_ansi(input).chars().count()
}

/// Convert inline bullet sequences like "text• item• item" into
/// line-broken bullets for better terminal readability.
fn break_inline_bullets(text: &str) -> String {
    let bullet = '•';
    let mut result = String::with_capacity(text.len() + 16);
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        result.push(ch);
        if ch == bullet {
            if chars.peek() == Some(&' ') {
                // Look back past a nested item's indentation: if the bullet does not
                // start its line, break the line before it.
                let prev = result.chars().rev().skip(1).find(|c| *c != ' ');
                if prev.is_some_and(|c| c != '\n') {
                    let bullet_start = result.len() - bullet.len_utf8();
                    result.insert(bullet_start, '\n');
                }
            }
        }
    }

    result
}

pub(crate) fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{strip_ansi, MarkdownStreamState, Spinner, TerminalRenderer};

    #[test]
    fn renders_markdown_with_styling_and_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("# Heading\n\nThis is **bold** and *italic*.\n\n- item\n\n`code`");

        assert!(markdown_output.contains("Heading"));
        assert!(markdown_output.contains("• item"));
        assert!(markdown_output.contains("code"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn keeps_code_punctuation_literal() {
        let terminal_renderer = TerminalRenderer::new();
        let plain_text = strip_ansi(
            &terminal_renderer
                .render_markdown("const sorted = [...values].sort(); it's \"quoted\" -- here"),
        );

        assert!(plain_text.contains("[...values]"), "{plain_text}");
        assert!(
            plain_text.contains("it's \"quoted\" -- here"),
            "{plain_text}"
        );
    }

    #[test]
    fn renders_links_as_colored_markdown_labels() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("See [Claw](https://example.com/docs) now.");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("[Claw](https://example.com/docs)"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn highlights_fenced_code_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ rust"));
        assert!(plain_text.contains("fn hi"));
        assert!(markdown_output.contains('\u{1b}'));
        assert!(markdown_output.contains("[48;5;236m"));
    }

    #[test]
    fn renders_ordered_and_nested_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("1. first\n2. second\n   - nested\n   - child");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("1. first"));
        assert!(plain_text.contains("2. second"));
        assert!(plain_text.contains("• nested"));
        assert!(plain_text.contains("• child"));
    }

    /// Nested items keep their indentation, rendered whole or streamed line by line or
    /// character by character.
    #[test]
    fn nested_items_under_numbered_ones_keep_their_indentation() {
        let renderer = TerminalRenderer::new();
        let source =
            "Steps:\n\n1. First\n   - one\n   - two\n2. Second\n   - three\n     - deeper\n\nDone.";
        let expected =
            "Steps:\n\n1. First\n  \u{2022} one\n  \u{2022} two\n2. Second\n  \u{2022} three\n    \u{2022} deeper\n\nDone.\n";
        let chunkings: [Vec<String>; 3] = [
            vec![source.to_string()],
            source.split_inclusive('\n').map(str::to_string).collect(),
            source.chars().map(String::from).collect(),
        ];
        for chunks in chunkings {
            let mut state = MarkdownStreamState::default();
            let mut printed = String::new();
            for chunk in &chunks {
                printed.push_str(&state.push(&renderer, chunk).unwrap_or_default());
            }
            printed.push_str(&state.flush(&renderer).unwrap_or_default());
            assert_eq!(strip_ansi(&printed), expected, "{} chunks", chunks.len());
        }
    }

    #[test]
    fn renders_tables_with_alignment() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| alpha | 1 |\n| beta | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name  │ Value │");
        assert_eq!(lines[1], "│───────┼───────│");
        assert_eq!(lines[2], "│ alpha │ 1     │");
        assert_eq!(lines[3], "│ beta  │ 22    │");
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("Heading"));
        assert!(plain_text.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn streaming_state_holds_outer_fence_with_nested_inner_fence() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(
            state.push(&renderer, "````markdown\n```rust\nfn inner() {}\n"),
            None,
            "inner triple backticks must not close the outer four-backtick fence"
        );
        assert_eq!(
            state.push(&renderer, "```\n"),
            None,
            "closing the inner fence must not flush the outer fence"
        );
        let flushed = state
            .push(&renderer, "````\n")
            .expect("closing the outer four-backtick fence flushes the buffered block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("fn inner()"));
        assert!(plain_text.contains("```rust"));
    }

    #[test]
    fn streaming_state_distinguishes_backtick_and_tilde_fences() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "~~~text\n"), None);
        assert_eq!(
            state.push(&renderer, "```\nstill inside tilde fence\n"),
            None,
            "a backtick fence cannot close a tilde-opened fence"
        );
        assert_eq!(state.push(&renderer, "```\n"), None);
        let flushed = state
            .push(&renderer, "~~~\n")
            .expect("matching tilde marker closes the fence");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("still inside tilde fence"));
    }

    #[test]
    fn renders_nested_fenced_code_block_preserves_inner_markers() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("````markdown\n```rust\nfn nested() {}\n```\n````");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ markdown"));
        assert!(plain_text.contains("```rust"));
        assert!(plain_text.contains("fn nested()"));
    }

    #[test]
    fn streamed_chunks_keep_line_breaks_and_paragraphs() {
        let renderer = TerminalRenderer::new();
        let source = "Line one\nLine two\n\n\nPara two\n\n- a\n- b\n\nlast";
        let expected = "Line one\nLine two\n\nPara two\n\n\u{2022} a\n\u{2022} b\n\nlast\n";
        let chunkings: [Vec<String>; 3] = [
            vec![source.to_string()],
            source.split_inclusive('\n').map(str::to_string).collect(),
            source.chars().map(String::from).collect(),
        ];
        for chunks in chunkings {
            let mut state = MarkdownStreamState::default();
            let mut printed = String::new();
            for chunk in &chunks {
                printed.push_str(&state.push(&renderer, chunk).unwrap_or_default());
            }
            printed.push_str(&state.flush(&renderer).unwrap_or_default());
            assert_eq!(strip_ansi(&printed), expected, "{} chunks", chunks.len());
        }

        // A block never starts or ends on a blank line, and the next one starts fresh.
        let mut state = MarkdownStreamState::default();
        assert_eq!(state.push(&renderer, "\n\n"), None);
        assert_eq!(
            strip_ansi(&state.push(&renderer, "hi\n\n").unwrap()),
            "hi\n"
        );
        assert_eq!(state.flush(&renderer), None);
        assert_eq!(
            strip_ansi(&state.render_block(&renderer, "next").unwrap()),
            "next\n"
        );
    }

    #[test]
    fn keeps_backslashes_of_paths_and_code_but_reads_markdown_escapes() {
        let renderer = TerminalRenderer::new();
        let plain = |markdown: &str| strip_ansi(&renderer.render_markdown(markdown));
        for path in [
            r"C:\Users\x\.neuron\a.txt",
            r"Open .\.git or folder\.config",
            r"\\server\share\.x",
            r"match \d+\.\d+ here",
            r"(see C:\)",
        ] {
            assert_eq!(plain(path), path);
        }
        assert_eq!(plain(r"**C:\_build\out**"), r"C:\_build\out");
        // Code is literal already, so nothing is doubled there.
        assert_eq!(plain(r"run `C:\x\.y` now"), r"run `C:\x\.y` now");
        let block = plain("```\nC:\\x\\.y\n```");
        assert!(block.contains("C:\\x\\.y\n"), "{block}");
        assert!(!block.contains(r"\\"), "{block}");
        // Markdown escapes outside paths still work.
        assert_eq!(
            plain(r"\*not italic\* and 1\. not a list"),
            "*not italic* and 1. not a list"
        );
        assert_eq!(
            plain(r"\_\_init\_\_.py and \\ alone"),
            r"__init__.py and \ alone"
        );
    }

    #[test]
    fn spinner_advances_frames() {
        let terminal_renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        let mut out = Vec::new();
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");

        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("Working"));
    }
}
