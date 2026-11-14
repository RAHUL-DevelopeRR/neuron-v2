//! `read_document`: Word, `PowerPoint`, spreadsheet and PDF files as Markdown.
//!
//! Pure Rust, read-only: OOXML packages are unzipped (`zip`) and their XML
//! walked (`quick-xml`); spreadsheets go through `calamine`; PDFs through a
//! small parser of their own ([`pdf`]). No scripts or external programs run.
//!
//! Every reader treats the file as hostile: inflated sizes, object counts,
//! nesting depth and the work spent per call are all capped, and a panic in
//! a parser ends the call with an error instead of the program. In the CLI
//! the parse also runs in a child process ([`worker`]) with a time limit (and
//! on Windows a memory limit), so a file that gets past those caps still
//! cannot crash or hang the agent.

mod docx;
mod pdf;
mod pptx;
mod sheet;
pub(crate) mod worker;
mod xls;
mod xml;

#[cfg(test)]
mod hostile_tests;
#[cfg(test)]
mod regression_tests;
#[cfg(test)]
pub(crate) mod tests;

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// Characters returned when the caller gives no `max_chars`: sized for small
/// context windows.
pub(crate) const DEFAULT_MAX_CHARS: usize = 12_000;
/// The most characters one call returns. The runtime cuts every tool result
/// to its own limit (40,000 characters by default), so this stays under that
/// with room for the header and the hint; see [`max_chars_ceiling`].
pub(crate) const MAX_MAX_CHARS: usize = 36_000;
/// Room kept under the runtime's tool result limit for the header and hint.
const HEADER_ROOM: usize = 4_000;
/// The fewest characters one call returns, whatever `max_chars` says.
const MIN_MAX_CHARS: usize = 500;
/// Columns a Markdown table keeps; further cells are dropped.
pub(crate) const MAX_TABLE_COLUMNS: usize = 64;
/// Rows one table read from a document keeps; further rows are dropped.
pub(crate) const MAX_TABLE_ROWS: usize = 50_000;
/// Characters one rendered table may take; more than any call returns.
const MAX_TABLE_CHARS: usize = 200_000;
/// Sheet names listed in a note or error; the rest are counted.
const MAX_LISTED_SHEETS: usize = 50;
/// Characters of one sheet name in such a listing.
const MAX_LISTED_NAME_CHARS: usize = 64;

/// The largest `max_chars` a call honours: [`MAX_MAX_CHARS`], or less when
/// the runtime's tool result limit (`NEURON_MAX_TOOL_RESULT_CHARS`) is lower,
/// so what is returned is never cut again in the middle.
pub(crate) fn max_chars_ceiling() -> usize {
    let runtime_limit = runtime::context_manager::ContextPolicy::from_env().max_tool_result_chars;
    runtime_limit
        .saturating_sub(HEADER_ROOM)
        .clamp(MIN_MAX_CHARS, MAX_MAX_CHARS)
}

/// A document as pages, slides or sheets, of which only the selected ones are
/// filled in.
#[derive(Debug, Default)]
pub(crate) struct Document {
    /// What the file is, as the header names it ("Word document").
    pub(crate) kind: &'static str,
    /// What one unit is: "page", "slide" or "sheet".
    pub(crate) unit: &'static str,
    /// How many units the file has.
    pub(crate) total: usize,
    /// The selected units, in order.
    pub(crate) units: Vec<Unit>,
    /// Caveats about the whole file, shown under the header.
    pub(crate) notes: Vec<String>,
    /// Selected units left unbuilt because earlier ones already filled the
    /// character budget; the truncation hint names them.
    pub(crate) unread: Vec<usize>,
    /// Units that hints must name by something other than their number: a
    /// sheet whose position is also another sheet's name is named by its own
    /// name, since `pages` matches names first.
    pub(crate) selectors: Vec<(usize, String)>,
}

impl Document {
    /// How `pages=` names unit `number`.
    fn selector(&self, number: usize) -> String {
        self.selectors
            .iter()
            .find(|(unit, _)| *unit == number)
            .map_or_else(|| number.to_string(), |(_, name)| name.clone())
    }

    /// `numbers` as a `pages=` value: ranges, with a unit that has a
    /// selector of its own named by it.
    fn pages_value(&self, numbers: &[usize]) -> String {
        if self.selectors.is_empty() {
            return compact_ranges(numbers);
        }
        let (named, plain): (Vec<usize>, Vec<usize>) = numbers
            .iter()
            .partition(|number| self.selectors.iter().any(|(unit, _)| unit == *number));
        let mut parts = Vec::new();
        if !plain.is_empty() {
            parts.push(compact_ranges(&plain));
        }
        parts.extend(named.into_iter().map(|number| self.selector(number)));
        parts.join(",")
    }
}

#[derive(Debug)]
pub(crate) struct Unit {
    /// 1-based position in the file.
    pub(crate) number: usize,
    pub(crate) markdown: String,
}

/// What one `read_document` call asks for.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Options<'a> {
    pub(crate) pages: Option<&'a str>,
    /// Spreadsheet rows, 1-based (`"501-1000"`).
    pub(crate) rows: Option<&'a str>,
    /// Spreadsheet columns (`"AO-BZ"`).
    pub(crate) columns: Option<&'a str>,
    pub(crate) max_chars: Option<usize>,
}

/// Characters built so far against `max_chars`. Readers stop building units
/// once it is spent, so a huge file costs no more than its first pages.
#[derive(Debug)]
pub(crate) struct Budget {
    max_chars: usize,
    used: usize,
}

impl Budget {
    pub(crate) fn new(max_chars: usize) -> Self {
        Self { max_chars, used: 0 }
    }

    /// Whether the units built so far already pass `max_chars`.
    pub(crate) fn is_spent(&self) -> bool {
        self.used > self.max_chars
    }

    pub(crate) fn spend(&mut self, markdown: &str) {
        self.used = self
            .used
            .saturating_add(markdown.trim_end().chars().count() + 2);
    }
}

/// Which pages, slides or sheets to return: 1-based ranges, and sheet names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    ranges: Vec<(usize, usize)>,
    names: Vec<String>,
}

impl Selection {
    /// Parses `"2"`, `"3-5"`, `"7-"` and lists of them (`"1,4-6"`). Other
    /// entries are kept as names, which only spreadsheets accept.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        Self::parse_with_names(text, &[])
    }

    /// Like [`Self::parse`], but an entry that is exactly one of `known`
    /// (sheet names such as `"2026-27"` or `"2024"`) is taken as that name
    /// before it is read as a range.
    pub(crate) fn parse_with_names(text: &str, known: &[String]) -> Result<Self, String> {
        let mut selection = Self::default();
        for part in text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let unquoted = part.trim_matches(|ch| ch == '"' || ch == '\'');
            if known
                .iter()
                .any(|name| name.trim().eq_ignore_ascii_case(unquoted))
            {
                selection.names.push(unquoted.to_string());
                continue;
            }
            match parse_range(part) {
                Some(Ok(range)) => selection.ranges.push(range),
                Some(Err(error)) => return Err(error),
                None => selection.names.push(unquoted.to_string()),
            }
        }
        Ok(selection)
    }

    pub(crate) fn is_all(&self) -> bool {
        self.ranges.is_empty() && self.names.is_empty()
    }

    pub(crate) fn has_names(&self) -> bool {
        !self.names.is_empty()
    }

    /// Whether unit `number` (named `name`, for sheets) is selected.
    pub(crate) fn includes(&self, number: usize, name: Option<&str>) -> bool {
        self.is_all()
            || self
                .ranges
                .iter()
                .any(|(start, end)| (*start..=*end).contains(&number))
            || name.is_some_and(|name| {
                self.names
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(name.trim()))
            })
    }
}

/// `Some(Ok(..))` for a number or range, `Some(Err(..))` for a malformed one,
/// `None` for text that is not numeric at all.
fn parse_range(part: &str) -> Option<Result<(usize, usize), String>> {
    let numeric = |text: &str| !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit());
    let (start, end) = match part.split_once('-') {
        Some((start, end)) => (start.trim(), Some(end.trim())),
        None => (part, None),
    };
    if !numeric(start) || end.is_some_and(|end| !end.is_empty() && !numeric(end)) {
        return None;
    }
    let start = start.parse::<usize>().ok()?;
    let end = match end {
        None => start,
        Some("") => usize::MAX,
        Some(end) => end.parse::<usize>().ok()?,
    };
    if start == 0 || end < start {
        return Some(Err(format!(
            "pages=\"{part}\" is not a valid range: numbers start at 1 and ranges run low to high, for example \"2-4\""
        )));
    }
    Some(Ok((start, end)))
}

/// Reads `path` (already checked against the workspace and size limits) and
/// returns Markdown of at most `max_chars` characters plus a short header.
pub(crate) fn read_document(path: &Path, options: &Options<'_>) -> Result<String, String> {
    // The parsers handle untrusted bytes; a bug in one (or in a dependency)
    // must end this call with an error, not take the whole program down.
    catch_unwind(AssertUnwindSafe(|| read_unguarded(path, options))).unwrap_or_else(|_| {
        Err(
            "this file could not be read: it is damaged in a way the reader does not handle"
                .to_string(),
        )
    })
}

fn read_unguarded(path: &Path, options: &Options<'_>) -> Result<String, String> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let pages = options
        .pages
        .map(str::trim)
        .filter(|pages| !pages.is_empty());
    let spreadsheet = matches!(extension.as_str(), "xlsx" | "xlsm" | "xlsb" | "xls" | "ods");
    let sheet_only = [options.rows, options.columns]
        .iter()
        .any(|value| value.is_some_and(|value| !value.trim().is_empty()));
    if sheet_only && !spreadsheet {
        return Err("rows and columns only apply to spreadsheets".to_string());
    }
    let ceiling = max_chars_ceiling();
    let max_chars = options
        .max_chars
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(MIN_MAX_CHARS, ceiling);
    let mut budget = Budget::new(max_chars);
    let (document, selection) = if spreadsheet {
        sheet::read(path, options, &mut budget)?
    } else {
        let selection = match pages {
            Some(pages) => Selection::parse(pages)?,
            None => Selection::default(),
        };
        if selection.has_names() {
            return Err(format!(
                "pages=\"{}\" is not a page range; use numbers such as \"3\" or \"2-5\"",
                pages.unwrap_or_default()
            ));
        }
        let document = match extension.as_str() {
            "docx" => docx::read(path, &selection, &mut budget)?,
            "pptx" => pptx::read(path, &selection, &mut budget)?,
            "pdf" => pdf::read(path, &selection, &mut budget)?,
            _ => {
                let supported = runtime::DOCUMENT_KINDS
                    .iter()
                    .map(|(extension, _)| format!(".{extension}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "read_document reads {supported} files; use read_file for text files"
                ));
            }
        };
        (document, selection)
    };
    render(path, &document, &selection, options, max_chars, ceiling)
}

/// The error for a file whose bytes are text although its extension names a
/// document: many "export to Excel" features write HTML or CSV as `.xls`.
pub(crate) fn text_file_error(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    format!(
        "{name} is a text file (such as HTML, CSV or tab-separated values) despite its extension; read it with read_file"
    )
}

/// Whether `data` looks like text: UTF-16 with a byte order mark, or no NUL
/// byte in its first 8 KB. `read_file` reads exactly these files as text
/// (in any of their usual encodings), so the two tools agree.
pub(crate) fn looks_like_text(data: &[u8]) -> bool {
    runtime::document_named_file_is_text(data)
}

/// `names` as `1 A, 2 B, ...`, at most [`MAX_LISTED_SHEETS`] of them, each
/// cut to [`MAX_LISTED_NAME_CHARS`] characters.
pub(crate) fn sheet_listing(names: &[String]) -> String {
    let mut listing = names
        .iter()
        .take(MAX_LISTED_SHEETS)
        .enumerate()
        .map(|(index, name)| {
            let mut shown = name.chars().take(MAX_LISTED_NAME_CHARS).collect::<String>();
            if name.chars().count() > MAX_LISTED_NAME_CHARS {
                shown.push_str("...");
            }
            format!("{} {shown}", index + 1)
        })
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_LISTED_SHEETS {
        let _ = write!(listing, ", and {} more", names.len() - MAX_LISTED_SHEETS);
    }
    listing
}

fn plural(unit: &str, count: usize) -> String {
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// `[3, 4, 5, 9]` as `"3-5,9"`.
fn compact_ranges(numbers: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < numbers.len() {
        let start = numbers[index];
        let mut end = start;
        while index + 1 < numbers.len() && numbers[index + 1] == end + 1 {
            index += 1;
            end = numbers[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    parts.join(",")
}

#[allow(clippy::too_many_lines)]
fn render(
    path: &Path,
    document: &Document,
    selection: &Selection,
    options: &Options<'_>,
    max_chars: usize,
    ceiling: usize,
) -> Result<String, String> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    if document.units.is_empty() {
        return Err(if document.total == 0 {
            format!("{name} has no {}s to read", document.unit)
        } else {
            format!(
                "pages selects nothing: {name} has {}",
                plural(document.unit, document.total)
            )
        });
    }
    let numbers = document
        .units
        .iter()
        .map(|unit| unit.number)
        .collect::<Vec<_>>();
    let unread = &document.unread;
    let showing = if selection.is_all() {
        "showing all".to_string()
    } else {
        let mut selected = numbers.clone();
        selected.extend(unread);
        format!("showing {} {}", document.unit, compact_ranges(&selected))
    };
    let mut header = format!(
        "[{name}: {}, {}, {showing}]",
        document.kind,
        plural(document.unit, document.total)
    );
    for note in &document.notes {
        header.push_str("\n[note: ");
        header.push_str(note);
        header.push(']');
    }

    // Unit start offsets in characters, counted as the body grows: counting
    // the whole body again for every unit would be quadratic.
    let mut body = String::new();
    let mut starts = Vec::with_capacity(document.units.len());
    let mut total_chars = 0;
    for unit in &document.units {
        if !body.is_empty() {
            body.push_str("\n\n");
            total_chars += 2;
        }
        starts.push(total_chars);
        let markdown = unit.markdown.trim_end();
        body.push_str(markdown);
        total_chars += markdown.chars().count();
    }
    if total_chars <= max_chars {
        if unread.is_empty() {
            return Ok(format!("{header}\n\n{body}"));
        }
        return Ok(format!(
            "{header}\n\n{body}\n\n[truncated: more follows, use pages=\"{}\"]",
            document.pages_value(unread)
        ));
    }
    let more = if unread.is_empty() { "" } else { "at least " };

    // Cut at a unit boundary when one fits, so the hint can name where to resume.
    let cut_unit = starts
        .iter()
        .rposition(|start| *start < max_chars)
        .unwrap_or(0);
    let (shown, hint) = if cut_unit > 0 {
        let shown = take_chars(&body, starts[cut_unit]).trim_end().to_string();
        let mut rest = numbers[cut_unit..].to_vec();
        rest.extend(unread);
        let remaining = total_chars - starts[cut_unit];
        (
            shown,
            format!(
                "[truncated: {more}{remaining} more chars, use pages=\"{}\"]",
                document.pages_value(&rest)
            ),
        )
    } else {
        let mut shown = take_chars(&body, max_chars).to_string();
        // End on a line break when one is reasonably close.
        if let Some(line_end) = shown.rfind('\n') {
            if shown[..line_end].chars().count() > max_chars * 3 / 5 {
                shown.truncate(line_end);
            }
        }
        let remaining = total_chars - shown.chars().count();
        let unit_name = format!("{} {}", document.unit, numbers[0]);
        let mut rest = numbers[1..].to_vec();
        rest.extend(unread);
        let next = if rest.is_empty() {
            String::new()
        } else {
            format!(
                ", or pages=\"{}\" for what follows",
                document.pages_value(&rest)
            )
        };
        let resume_row = if document.unit == "sheet" {
            // A line cut in the middle is not a complete row: resume at it.
            let complete = body
                .as_bytes()
                .get(shown.len())
                .is_none_or(|byte| *byte == b'\n');
            let lines = if complete {
                shown.as_str()
            } else {
                shown.rfind('\n').map_or("", |end| &shown[..end])
            };
            last_table_row(lines)
        } else {
            None
        };
        let hint = if let Some(row) = resume_row {
            // A sheet resumes at the row after the last one shown.
            let columns = options
                .columns
                .map(str::trim)
                .filter(|columns| !columns.is_empty())
                .map_or_else(String::new, |columns| format!(" columns=\"{columns}\""));
            format!(
                "[truncated: {more}{remaining} more chars; continue with pages=\"{}\" rows=\"{}-\"{columns}{next}]",
                document.selector(numbers[0]),
                row + 1
            )
        } else if max_chars < ceiling {
            format!(
                "[truncated: {more}{remaining} more chars; {unit_name} alone is longer than max_chars, call again with a larger max_chars (up to {ceiling}){next}]"
            )
        } else {
            format!(
                "[truncated: {more}{remaining} more chars; the rest of {unit_name} does not fit in one call{next}]"
            )
        };
        (shown.trim_end().to_string(), hint)
    };
    Ok(format!("{header}\n\n{shown}\n\n{hint}"))
}

/// The sheet row number of the last complete table line in `text`
/// (`| 512 | ... |`).
fn last_table_row(text: &str) -> Option<usize> {
    text.lines().rev().find_map(|line| {
        let line = line.trim_end();
        if !(line.starts_with("| ") && line.ends_with('|')) {
            return None;
        }
        line[2..].split(' ').next()?.parse().ok()
    })
}

fn take_chars(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// Text as one Markdown table cell: pipes escaped, line breaks as `<br>`.
pub(crate) fn table_cell(text: &str) -> String {
    text.trim()
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

/// Rows as a Markdown table whose first row is the header. Short rows are
/// padded; columns empty in every row are left off the right edge, rows
/// after the header with no text are left out, and a table with no text
/// gives an empty string. At most [`MAX_TABLE_COLUMNS`] columns and about
/// [`MAX_TABLE_CHARS`] characters are built, so a hostile table of empty
/// cells cannot grow into gigabytes.
pub(crate) fn markdown_table(rows: &[Vec<String>]) -> String {
    let filled = |cell: &String| !cell.trim().is_empty();
    let columns = rows
        .iter()
        .filter_map(|row| row.iter().rposition(filled))
        .max()
        .map_or(0, |last| last + 1)
        .min(MAX_TABLE_COLUMNS);
    if columns == 0 {
        return String::new();
    }
    let line = |row: &[String]| {
        let mut cells = row
            .iter()
            .take(columns)
            .map(|cell| table_cell(cell))
            .collect::<Vec<_>>();
        cells.resize(columns, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = format!("{}\n|{}", line(&rows[0]), " --- |".repeat(columns));
    let body = rows[1..]
        .iter()
        .filter(|row| row.iter().take(columns).any(filled))
        .collect::<Vec<_>>();
    for (index, row) in body.iter().enumerate() {
        if out.len() > MAX_TABLE_CHARS {
            let _ = write!(
                out,
                "\n\n[{} more rows of this table are longer than read_document returns]",
                body.len() - index
            );
            break;
        }
        out.push('\n');
        out.push_str(&line(row));
    }
    out
}

/// Characters of text one cell of a document table keeps while the XML is
/// walked; the rest is dropped and the cell ends in [`CUT_MARK`].
pub(crate) const MAX_CELL_CHARS: usize = 4_000;
/// Characters one rendered row of a document table takes at most: the cells
/// of a wide row are cut shorter so the whole line fits.
const MAX_ROW_LINE_CHARS: usize = 24_000;
/// Characters of the header row repeated over each continued part.
const MAX_REPEATED_HEADER_CHARS: usize = 2_000;
/// Characters one document table renders to across all its parts, repeated
/// headers included. Tables are split into parts of about a page, so this
/// is more than one call returns but every part stays reachable.
const MAX_DOCUMENT_TABLE_CHARS: usize = 1_000_000;
/// Marks a cell whose text was cut.
const CUT_MARK: &str = " [...]";
/// Starts each part of a table after the first.
const CONTINUED: &str = "(table continued)\n\n";

/// One cell as Markdown, cut to `max_chars` characters (after escaping)
/// without splitting an escape.
fn cut_cell(text: &str, max_chars: usize) -> String {
    let escaped = table_cell(text);
    if escaped.chars().count() <= max_chars {
        return escaped;
    }
    let mut kept = take_chars(&escaped, max_chars).to_string();
    for partial in ["<br", "<b", "<", "\\"] {
        if kept.ends_with(partial) {
            kept.truncate(kept.len() - partial.len());
            break;
        }
    }
    kept.push_str(CUT_MARK);
    kept
}

/// `row` as one Markdown table line of `columns` cells, each at most
/// `per_cell` characters.
fn row_line(row: &[String], columns: usize, per_cell: usize) -> String {
    let mut cells = row
        .iter()
        .take(columns)
        .map(|cell| cut_cell(cell, per_cell))
        .collect::<Vec<_>>();
    cells.resize(columns, String::new());
    format!("| {} |", cells.join(" | "))
}

/// The rows of a document table that start on one page, rendered: the
/// header lines are shared by every part of the table, not copied.
#[derive(Debug, Clone)]
pub(crate) struct TablePart {
    pub(crate) page: usize,
    /// Header row and separator line, for the first part.
    header: std::sync::Arc<str>,
    /// A shorter copy for the parts after it.
    repeat_header: std::sync::Arc<str>,
    /// The part continues the table: it starts with [`CONTINUED`].
    continued: bool,
    lines: Vec<String>,
    /// A note after the last row ("more rows are left out").
    note: Option<String>,
}

impl TablePart {
    pub(crate) fn render(&self) -> String {
        let header: &str = if self.continued {
            &self.repeat_header
        } else {
            &self.header
        };
        let mut out = String::with_capacity(
            header.len() + self.lines.iter().map(|line| line.len() + 1).sum::<usize>() + 32,
        );
        if self.continued {
            out.push_str(CONTINUED);
        }
        out.push_str(header);
        for line in &self.lines {
            out.push('\n');
            out.push_str(line);
        }
        if let Some(note) = &self.note {
            out.push_str("\n\n");
            out.push_str(note);
        }
        out
    }

    /// The part as parts of about `max_chars` characters of rows each, all
    /// but the first continuing the table, so no one page has to hold a
    /// long table (pages estimated by size).
    pub(crate) fn split(self, max_chars: usize) -> Vec<TablePart> {
        let Self {
            page,
            header,
            repeat_header,
            continued,
            lines,
            note,
        } = self;
        let part = |continued: bool, lines: Vec<String>| TablePart {
            page,
            header: header.clone(),
            repeat_header: repeat_header.clone(),
            continued,
            lines,
            note: None,
        };
        let mut parts = Vec::new();
        let mut current = Vec::new();
        let mut size = 0;
        for line in lines {
            if size >= max_chars && !current.is_empty() {
                parts.push(part(
                    continued || !parts.is_empty(),
                    std::mem::take(&mut current),
                ));
                size = 0;
            }
            size += line.chars().count() + 1;
            current.push(line);
        }
        let mut last = part(continued || !parts.is_empty(), current);
        last.note = note;
        parts.push(last);
        parts
    }
}

/// A table being read out of a document: rows of cells, each row with the
/// page it starts on, kept within [`MAX_TABLE_ROWS`], [`MAX_TABLE_COLUMNS`]
/// and [`MAX_CELL_CHARS`] while the XML is walked.
#[derive(Debug, Default)]
pub(crate) struct TableBuilder {
    pub(crate) rows: Vec<Vec<String>>,
    /// The page each row starts on (documents with pages).
    pages: Vec<usize>,
    /// The current row or cell is past a limit (or the cell is full); its
    /// text is dropped.
    skip_row: bool,
    skip_cell: bool,
    /// Characters in the current cell.
    cell_chars: usize,
    /// Rows were dropped.
    cut: bool,
}

impl TableBuilder {
    pub(crate) fn start_row(&mut self, page: usize) {
        self.skip_cell = true;
        if self.rows.len() >= MAX_TABLE_ROWS {
            self.skip_row = true;
            self.cut = true;
            return;
        }
        self.skip_row = false;
        self.rows.push(Vec::new());
        self.pages.push(page);
    }

    pub(crate) fn start_cell(&mut self) {
        self.skip_cell = true;
        self.cell_chars = 0;
        if self.skip_row {
            return;
        }
        if let Some(row) = self.rows.last_mut() {
            if row.len() < MAX_TABLE_COLUMNS {
                row.push(String::new());
                self.skip_cell = false;
            }
        }
    }

    /// Adds `text` to the current cell, on a line of its own when the cell
    /// already has text, keeping at most [`MAX_CELL_CHARS`] characters.
    pub(crate) fn push_line(&mut self, text: &str) {
        if self.skip_cell || self.skip_row {
            return;
        }
        let Some(cell) = self.rows.last_mut().and_then(|row| row.last_mut()) else {
            return;
        };
        if !cell.is_empty() {
            cell.push('\n');
            self.cell_chars += 1;
        }
        let kept = take_chars(text, MAX_CELL_CHARS.saturating_sub(self.cell_chars));
        cell.push_str(kept);
        self.cell_chars += kept.chars().count();
        if kept.len() < text.len() {
            cell.push_str(CUT_MARK);
            self.skip_cell = true;
        }
    }

    /// A page break inside the table: a row with no text yet starts on
    /// `page`.
    pub(crate) fn page_break(&mut self, page: usize) {
        let empty = self
            .rows
            .last()
            .is_some_and(|row| row.iter().all(|cell| cell.trim().is_empty()));
        if empty {
            if let Some(last) = self.pages.last_mut() {
                *last = page;
            }
        }
    }

    /// The table as plain text, for a table nested in a cell: no more than
    /// the cell it goes into can keep.
    pub(crate) fn flatten(&self) -> String {
        let mut out = String::new();
        'rows: for (index, row) in self.rows.iter().enumerate() {
            if index > 0 {
                out.push_str("; ");
            }
            for (column, cell) in row.iter().enumerate() {
                if column > 0 {
                    out.push_str(" / ");
                }
                out.push_str(cell);
                // Bytes: at least MAX_CELL_CHARS characters.
                if out.len() > MAX_CELL_CHARS * 4 {
                    break 'rows;
                }
            }
        }
        out
    }

    /// The table as rendered parts, one per page its rows start on, each
    /// after the first repeating the header row. Rows with no text are left
    /// out, and all parts together stay within [`MAX_DOCUMENT_TABLE_CHARS`].
    pub(crate) fn into_parts(self) -> Vec<TablePart> {
        let Self {
            rows, pages, cut, ..
        } = self;
        let filled = |cell: &String| !cell.trim().is_empty();
        let columns = rows
            .iter()
            .filter_map(|row| row.iter().rposition(filled))
            .max()
            .map_or(0, |last| last + 1)
            .min(MAX_TABLE_COLUMNS);
        let Some(first) = rows.first() else {
            return Vec::new();
        };
        if columns == 0 {
            return Vec::new();
        }
        let per_cell = (MAX_ROW_LINE_CHARS / columns).min(MAX_CELL_CHARS);
        let separator = " --- |".repeat(columns);
        let header: std::sync::Arc<str> =
            format!("{}\n|{separator}", row_line(first, columns, per_cell)).into();
        let repeat_per_cell = (MAX_REPEATED_HEADER_CHARS / columns).clamp(16, per_cell.max(16));
        let repeat_header: std::sync::Arc<str> = format!(
            "{}\n|{separator}",
            row_line(first, columns, repeat_per_cell)
        )
        .into();
        let repeat_cost = repeat_header.chars().count() + CONTINUED.len();
        let mut total = header.chars().count();
        let mut parts = vec![TablePart {
            page: pages[0],
            header: header.clone(),
            repeat_header: repeat_header.clone(),
            continued: false,
            lines: Vec::new(),
            note: None,
        }];
        let body = rows
            .iter()
            .zip(&pages)
            .skip(1)
            .filter(|(row, _)| row.iter().take(columns).any(filled))
            .collect::<Vec<_>>();
        let mut note = None;
        for (index, (row, page)) in body.iter().enumerate() {
            let line = row_line(row, columns, per_cell);
            let current = parts.last().map_or(0, |part| part.page);
            let new_part = **page > current;
            let cost = line.chars().count() + 1 + if new_part { repeat_cost } else { 0 };
            if total + cost > MAX_DOCUMENT_TABLE_CHARS {
                note = Some(format!(
                    "[{} more rows of this table are longer than read_document returns]",
                    body.len() - index
                ));
                break;
            }
            total += cost;
            if new_part {
                parts.push(TablePart {
                    page: **page,
                    header: header.clone(),
                    repeat_header: repeat_header.clone(),
                    continued: true,
                    lines: Vec::new(),
                    note: None,
                });
            }
            if let Some(part) = parts.last_mut() {
                part.lines.push(line);
            }
        }
        if note.is_none() && cut {
            note = Some(format!(
                "[the table has more than {MAX_TABLE_ROWS} rows; the rest is left out]"
            ));
        }
        if let Some(last) = parts.last_mut() {
            last.note = note;
        }
        parts
    }

    /// The table as Markdown blocks, one per page its rows start on, each
    /// after the first repeating the header row: `(page, markdown)`.
    pub(crate) fn into_blocks(self) -> Vec<(usize, String)> {
        self.into_parts()
            .into_iter()
            .map(|part| (part.page, part.render()))
            .collect()
    }

    /// The table as one Markdown table, for documents without pages.
    pub(crate) fn into_markdown(self) -> String {
        self.into_blocks()
            .into_iter()
            .map(|(_, markdown)| markdown)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
