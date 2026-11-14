//! `read_document`: Word, `PowerPoint`, spreadsheet and PDF files as Markdown.
//!
//! Pure Rust, read-only: OOXML packages are unzipped (`zip`) and their XML
//! walked (`quick-xml`); spreadsheets go through `calamine`; PDFs through a
//! small parser of their own ([`pdf`]). No scripts or external programs run.

mod docx;
mod pdf;
mod pptx;
mod sheet;
mod xml;

#[cfg(test)]
pub(crate) mod tests;

use std::path::Path;

/// Characters returned when the caller gives no `max_chars`: sized for small
/// context windows.
pub(crate) const DEFAULT_MAX_CHARS: usize = 12_000;
/// The most characters one call returns.
pub(crate) const MAX_MAX_CHARS: usize = 100_000;
/// The fewest characters one call returns, whatever `max_chars` says.
const MIN_MAX_CHARS: usize = 500;

/// A document as pages, slides or sheets, of which only the selected ones are
/// filled in.
#[derive(Debug)]
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
}

#[derive(Debug)]
pub(crate) struct Unit {
    /// 1-based position in the file.
    pub(crate) number: usize,
    pub(crate) markdown: String,
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
        let mut selection = Self::default();
        for part in text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match parse_range(part) {
                Some(Ok(range)) => selection.ranges.push(range),
                Some(Err(error)) => return Err(error),
                None => selection
                    .names
                    .push(part.trim_matches(|ch| ch == '"' || ch == '\'').to_string()),
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
pub(crate) fn read_document(
    path: &Path,
    pages: Option<&str>,
    max_chars: Option<usize>,
) -> Result<String, String> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let selection = match pages.map(str::trim).filter(|pages| !pages.is_empty()) {
        Some(pages) => Selection::parse(pages)?,
        None => Selection::default(),
    };
    let spreadsheet = matches!(extension.as_str(), "xlsx" | "xlsm" | "xlsb" | "xls" | "ods");
    if selection.has_names() && !spreadsheet {
        return Err(format!(
            "pages=\"{}\" is not a page range; use numbers such as \"3\" or \"2-5\"",
            pages.unwrap_or_default()
        ));
    }
    let document = match extension.as_str() {
        "docx" => docx::read(path, &selection)?,
        "pptx" => pptx::read(path, &selection)?,
        "pdf" => pdf::read(path, &selection)?,
        _ if spreadsheet => sheet::read(path, &selection)?,
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
    let max_chars = max_chars
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(MIN_MAX_CHARS, MAX_MAX_CHARS);
    render(path, &document, &selection, max_chars)
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

fn render(
    path: &Path,
    document: &Document,
    selection: &Selection,
    max_chars: usize,
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
    let showing = if selection.is_all() {
        "showing all".to_string()
    } else {
        format!("showing {} {}", document.unit, compact_ranges(&numbers))
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

    let mut body = String::new();
    let mut starts = Vec::with_capacity(document.units.len());
    for unit in &document.units {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        starts.push(body.chars().count());
        body.push_str(unit.markdown.trim_end());
    }
    let total_chars = body.chars().count();
    if total_chars <= max_chars {
        return Ok(format!("{header}\n\n{body}"));
    }

    // Cut at a unit boundary when one fits, so the hint can name where to resume.
    let cut_unit = starts
        .iter()
        .rposition(|start| *start < max_chars)
        .unwrap_or(0);
    let (shown, hint) = if cut_unit > 0 {
        let shown = take_chars(&body, starts[cut_unit]).trim_end().to_string();
        let rest = compact_ranges(&numbers[cut_unit..]);
        let remaining = total_chars - starts[cut_unit];
        (
            shown,
            format!("[truncated: {remaining} more chars, use pages=\"{rest}\"]"),
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
        let hint = if max_chars < MAX_MAX_CHARS {
            let next = numbers.get(1).map_or_else(String::new, |_| {
                format!(
                    ", or pages=\"{}\" for what follows",
                    compact_ranges(&numbers[1..])
                )
            });
            format!(
                "[truncated: {remaining} more chars; {unit_name} alone is longer than max_chars, call again with a larger max_chars (up to {MAX_MAX_CHARS}){next}]"
            )
        } else {
            format!("[truncated: {remaining} more chars; the rest of {unit_name} does not fit in one call]")
        };
        (shown.trim_end().to_string(), hint)
    };
    Ok(format!("{header}\n\n{shown}\n\n{hint}"))
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
/// padded; an empty table gives an empty string.
pub(crate) fn markdown_table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return String::new();
    }
    let line = |row: &[String]| {
        let mut cells = row.iter().map(|cell| table_cell(cell)).collect::<Vec<_>>();
        cells.resize(columns, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut lines = vec![line(&rows[0]), format!("|{}", " --- |".repeat(columns))];
    lines.extend(rows[1..].iter().map(|row| line(row)));
    lines.join("\n")
}
