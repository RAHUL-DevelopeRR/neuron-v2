//! Spreadsheets (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`) through `calamine`:
//! each sheet as a Markdown table with row numbers and column letters.
//!
//! Workbooks are untrusted. `calamine` builds a sheet as one dense grid from
//! its first to its last cell, so two cells far apart would ask for hundreds
//! of gigabytes, and a failed allocation aborts the program. So:
//! - `.xlsx`, `.xlsm` and `.xlsb` sheets are streamed cell by cell and never
//!   built as a grid; only the rows and columns shown are kept;
//! - `.xls` and `.ods` workbooks, which `calamine` lays out whole while it
//!   opens them, are measured first ([`super::xls`], [`check_ods`]) and
//!   refused when their grids would be too large;
//! - every zip part is measured inflated before `calamine` reads it.
//!
//! The file is read once, checked, and parsed from those same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Cursor, Read as _};
use std::path::Path;

use calamine::{Data, Ods, Reader, Xls, Xlsb, Xlsx};

use super::xls;
use super::xml::{attr, walk, Node};
use super::{markdown_table, Budget, Document, Options, Selection, Unit};

const KIND: &str = "spreadsheet";

/// Non-empty rows shown per sheet.
pub(super) const MAX_SHEET_ROWS: usize = 500;
/// Columns shown per sheet.
pub(super) const MAX_SHEET_COLUMNS: usize = 40;
/// Grid cells `calamine` may lay out for a whole `.xls` or `.ods` workbook.
pub(super) const MAX_GRID_CELLS: u64 = 8_000_000;
/// Row numbers past this are ignored (Excel's own limit is 1,048,576).
const MAX_ROW: u32 = 1 << 24;
/// Inflated size limits for the parts of a zipped workbook: worksheets
/// (streamed), any other part (`calamine` holds these whole, such as the
/// shared strings), and all parts together.
const MAX_SHEET_PART_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OTHER_PART_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WORKBOOK_BYTES: u64 = 512 * 1024 * 1024;
/// The column limit of `.ods` rows, as `calamine` applies it.
const ODS_MAX_COLUMNS: u64 = 16_384;
const ODS_MAX_ROWS: u64 = 1_048_576;

type Bytes = Cursor<Vec<u8>>;

enum Book {
    Xlsx(Xlsx<Bytes>),
    Xlsb(Xlsb<Bytes>),
    Xls(Xls<Bytes>),
    Ods(Ods<Bytes>),
}

fn unreadable(detail: impl std::fmt::Display) -> String {
    format!("this is not a readable {KIND}: the file is damaged, password protected or not a workbook ({detail})")
}

impl Book {
    fn open(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|error| format!("cannot open the file: {error}"))?;
        if xls::is_compound_file(&data) {
            xls::check(&data, MAX_GRID_CELLS)?;
            return Xls::new(Cursor::new(data))
                .map(Self::Xls)
                .map_err(unreadable);
        }
        if !data.starts_with(b"PK") {
            return Err(unreadable("not a zip package or an Excel 97-2003 file"));
        }
        let kind = {
            let mut archive = zip::ZipArchive::new(Cursor::new(&data[..])).map_err(unreadable)?;
            let has = |archive: &zip::ZipArchive<Cursor<&[u8]>>, wanted: &str| {
                archive
                    .file_names()
                    .any(|name| name.is_ok_and(|name| name.eq_ignore_ascii_case(wanted)))
            };
            let kind = if has(&archive, "xl/workbook.bin") {
                "xlsb"
            } else if has(&archive, "xl/workbook.xml") {
                "xlsx"
            } else if has(&archive, "content.xml") {
                "ods"
            } else {
                return Err(unreadable("no workbook part"));
            };
            check_zip(&mut archive)?;
            if kind == "ods" {
                check_ods(&mut archive)?;
            }
            kind
        };
        let bytes = Cursor::new(data);
        match kind {
            "xlsb" => Xlsb::new(bytes).map(Self::Xlsb).map_err(unreadable),
            "xlsx" => Xlsx::new(bytes).map(Self::Xlsx).map_err(unreadable),
            _ => Ods::new(bytes).map(Self::Ods).map_err(unreadable),
        }
    }

    fn sheet_names(&self) -> Vec<String> {
        match self {
            Self::Xlsx(book) => book.sheet_names(),
            Self::Xlsb(book) => book.sheet_names(),
            Self::Xls(book) => book.sheet_names(),
            Self::Ods(book) => book.sheet_names(),
        }
    }

    /// The whole grid of sheet `name`, for the formats `calamine` already
    /// holds as grids (`.xls`, `.ods`); `None` for streamed formats.
    fn grid(&mut self, name: &str) -> Result<Option<calamine::Range<Data>>, String> {
        match self {
            Self::Xls(book) => book.worksheet_range(name).map(Some).map_err(cannot_read),
            Self::Ods(book) => book.worksheet_range(name).map(Some).map_err(cannot_read),
            Self::Xlsx(_) | Self::Xlsb(_) => Ok(None),
        }
    }

    /// Calls `visit` with each non-blank cell of sheet `name`: row, column
    /// (both 0-based) and value. Streamed formats are read again each call.
    fn scan(
        &mut self,
        name: &str,
        grid: Option<&calamine::Range<Data>>,
        visit: &mut dyn FnMut(u32, u32, &Data),
    ) -> Result<(), String> {
        let mut emit = |row: u32, column: u32, value: Data| {
            if row < MAX_ROW && !is_blank(&value) {
                visit(row, column, &value);
            }
        };
        match self {
            Self::Xlsx(book) => {
                let mut cells = book.worksheet_cells_reader(name).map_err(cannot_read)?;
                while let Some(cell) = cells.next_cell().map_err(cannot_read)? {
                    let (row, column) = cell.get_position();
                    emit(row, column, cell.get_value().clone().into());
                }
            }
            Self::Xlsb(book) => {
                let mut cells = book.worksheet_cells_reader(name).map_err(cannot_read)?;
                while let Some(cell) = cells.next_cell().map_err(cannot_read)? {
                    let (row, column) = cell.get_position();
                    emit(row, column, cell.get_value().clone().into());
                }
            }
            Self::Xls(_) | Self::Ods(_) => {
                if let Some(grid) = grid {
                    let (top, left) = grid.start().unwrap_or((0, 0));
                    for (row, column, value) in grid.used_cells() {
                        let row = u32::try_from(row).unwrap_or(u32::MAX).saturating_add(top);
                        let column = u32::try_from(column)
                            .unwrap_or(u32::MAX)
                            .saturating_add(left);
                        emit(row, column, value.clone());
                    }
                }
            }
        }
        Ok(())
    }
}

fn cannot_read(error: impl std::fmt::Display) -> String {
    format!("this sheet cannot be read: {error}")
}

/// Measures every XML and binary part inflated (the sizes the zip claims
/// can lie) before `calamine` reads any of them.
fn check_zip(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<(), String> {
    let mut total = 0u64;
    for position in 0..archive.len() {
        let mut entry = archive.by_index(position).map_err(unreadable)?;
        let name = entry
            .name()
            .map(|name| name.to_ascii_lowercase())
            .unwrap_or_default();
        let extension = Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if entry.is_dir() || !matches!(extension, "xml" | "bin" | "rels") {
            continue;
        }
        let limit = if name.contains("worksheets/") {
            MAX_SHEET_PART_BYTES
        } else {
            MAX_OTHER_PART_BYTES
        };
        let too_big = || {
            format!(
                "this {KIND} unpacks to more than read_document reads ({name} or the file as a whole is too large once unzipped)"
            )
        };
        if entry.size() > limit {
            return Err(too_big());
        }
        let read = std::io::copy(&mut (&mut entry).take(limit + 1), &mut std::io::sink())
            .map_err(unreadable)?;
        total = total.saturating_add(read);
        if read > limit || total > MAX_WORKBOOK_BYTES {
            return Err(too_big());
        }
    }
    Ok(())
}

/// One table of an `.ods` file as `calamine` will build it.
#[derive(Default)]
struct OdsTable {
    /// Each row element: repeat count, cells pushed, whether any holds a value.
    rows: Vec<(u64, u64, bool)>,
    total_rows: u64,
}

impl OdsTable {
    /// Cells `calamine` allocates for this table: the cells it reads, then
    /// the grid it expands them into.
    fn cells(&self) -> u64 {
        let read = self.rows.iter().map(|(_, pushed, _)| *pushed).sum::<u64>();
        let width = self
            .rows
            .iter()
            .filter(|(_, _, filled)| *filled)
            .map(|(_, pushed, _)| *pushed)
            .max()
            .unwrap_or(0);
        let first = self.rows.iter().position(|(_, _, filled)| *filled);
        let last = self.rows.iter().rposition(|(_, _, filled)| *filled);
        let grid = match (first, last) {
            (Some(first), Some(last)) => self.rows[first..=last]
                .iter()
                .map(|(repeats, _, _)| repeats.saturating_mul(width))
                .fold(0u64, u64::saturating_add),
            _ => 0,
        };
        read.saturating_add(grid)
    }
}

/// Refuses an `.ods` file whose tables `calamine` would expand past
/// [`MAX_GRID_CELLS`]: repeated rows and cells let a few bytes describe
/// millions of cells, and `calamine` expands them while opening the file.
fn check_ods(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<(), String> {
    let mut entry = archive.by_name("content.xml").map_err(unreadable)?;
    let mut bytes = Vec::new();
    (&mut entry)
        .take(MAX_OTHER_PART_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    let xml = String::from_utf8_lossy(&bytes);

    let mut total = 0u64;
    let mut table: Option<OdsTable> = None;
    // The row being read: repeats, cells pushed, empty cells waiting, filled.
    let mut row: Option<(u64, u64, u64, bool)> = None;
    // The cell being read: depth inside it, repeats, holds a value.
    let mut cell: Option<(usize, u64, bool)> = None;
    let repeats = |attrs: &[(String, String)], key: &str| {
        attr(attrs, key)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(1)
    };
    walk(&xml, |node| {
        if let Some((depth, _, filled)) = cell.as_mut() {
            match node {
                Node::Open { .. } => {
                    *depth += 1;
                    *filled = true;
                }
                Node::Text(text) => *filled |= !text.trim().is_empty(),
                Node::Close { .. } => {
                    *depth -= 1;
                    if *depth == 0 {
                        let (_, count, filled) = cell.take().unwrap_or_default();
                        if let Some((_, pushed, waiting, row_filled)) = row.as_mut() {
                            // Empty cells are only stored once a later cell
                            // follows them, as calamine does.
                            *pushed += (*waiting).min(ODS_MAX_COLUMNS - *pushed);
                            *waiting = 0;
                            let count = count.min(ODS_MAX_COLUMNS - *pushed);
                            if filled {
                                *pushed += count;
                                *row_filled = true;
                            } else {
                                *waiting = count;
                            }
                        }
                    }
                }
            }
            return;
        }
        match node {
            Node::Open { name: "table", .. } => table = Some(OdsTable::default()),
            Node::Close { name: "table" } => {
                if let Some(done) = table.take() {
                    total = total.saturating_add(done.cells());
                }
            }
            Node::Open {
                name: "table-row",
                attrs,
            } => {
                row = Some((repeats(attrs, "table:number-rows-repeated"), 0, 0, false));
            }
            Node::Close { name: "table-row" } => {
                if let (Some((count, pushed, _, filled)), Some(table)) =
                    (row.take(), table.as_mut())
                {
                    let count = count.min(ODS_MAX_ROWS.saturating_sub(table.total_rows));
                    table.total_rows += count;
                    table.rows.push((count, pushed, filled));
                }
            }
            Node::Open {
                name: "table-cell" | "covered-table-cell",
                attrs,
            } if row.is_some() => {
                let filled = attr(attrs, "office:value-type").is_some()
                    || attr(attrs, "table:formula").is_some();
                cell = Some((1, repeats(attrs, "table:number-columns-repeated"), filled));
            }
            _ => {}
        }
    })
    .map_err(unreadable)?;
    if total > MAX_GRID_CELLS {
        return Err(format!(
            "this .ods workbook lays its sheets out over more than {MAX_GRID_CELLS} cells, more than read_document reads; save it as .xlsx and read that instead"
        ));
    }
    Ok(())
}

/// Rows and columns asked for, 0-based and inclusive.
#[derive(Debug, Clone, Copy, Default)]
struct Window {
    rows: Option<(u32, u32)>,
    columns: Option<(u32, u32)>,
}

impl Window {
    fn parse(options: &Options<'_>) -> Result<Self, String> {
        let rows = options
            .rows
            .map(str::trim)
            .filter(|rows| !rows.is_empty())
            .map(|rows| {
                parse_span(rows, |text| {
                    text.parse::<u32>()
                        .ok()
                        .filter(|row| *row >= 1)
                        .map(|row| row - 1)
                })
                .ok_or_else(|| {
                    format!("rows=\"{rows}\" is not a row range; use sheet row numbers such as \"501-1000\" or \"501-\"")
                })
            })
            .transpose()?;
        let columns = options
            .columns
            .map(str::trim)
            .filter(|columns| !columns.is_empty())
            .map(|columns| {
                parse_span(columns, column_index).ok_or_else(|| {
                    format!("columns=\"{columns}\" is not a column range; use letters such as \"AO-BZ\" or \"C\"")
                })
            })
            .transpose()?;
        Ok(Self { rows, columns })
    }
}

/// `"A-B"`, `"A-"` or `"A"` through `bound` (which gives a 0-based index).
fn parse_span(text: &str, bound: impl Fn(&str) -> Option<u32>) -> Option<(u32, u32)> {
    let (start, end) = match text.split_once('-') {
        Some((start, end)) => (start.trim(), Some(end.trim())),
        None => (text, None),
    };
    let start = bound(start)?;
    let end = match end {
        None => start,
        Some("") => u32::MAX,
        Some(end) => bound(end)?,
    };
    (start <= end).then_some((start, end))
}

/// The 0-based index of column letters (`"A"` is 0, `"AA"` is 26), or of a
/// 1-based column number.
fn column_index(text: &str) -> Option<u32> {
    if let Ok(number) = text.parse::<u32>() {
        return number.checked_sub(1);
    }
    if text.is_empty() || text.len() > 3 || !text.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    let value = text
        .to_ascii_uppercase()
        .bytes()
        .fold(0u32, |value, byte| value * 26 + u32::from(byte - b'A' + 1));
    value.checked_sub(1)
}

pub(super) fn read(
    path: &Path,
    options: &Options<'_>,
    budget: &mut Budget,
) -> Result<(Document, Selection), String> {
    let window = Window::parse(options)?;
    let mut book = Book::open(path)?;
    let names = book.sheet_names();
    let selection = match options
        .pages
        .map(str::trim)
        .filter(|pages| !pages.is_empty())
    {
        Some(pages) => Selection::parse_with_names(pages, &names)?,
        None => Selection::default(),
    };
    let mut units = Vec::new();
    let mut unread = Vec::new();
    let mut matched_names = 0;
    for (index, name) in names.iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, Some(name)) {
            continue;
        }
        matched_names += 1;
        if budget.is_spent() {
            unread.push(number);
            continue;
        }
        let markdown = match render_sheet(&mut book, number, name, window, options) {
            Ok(markdown) => markdown,
            Err(error) => format!("## Sheet {number}: {name}\n\n({error})"),
        };
        budget.spend(&markdown);
        units.push(Unit { number, markdown });
    }
    if matched_names == 0 && selection.has_names() {
        let listing = names
            .iter()
            .enumerate()
            .map(|(index, name)| format!("{} {name}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("no sheet matches pages; the sheets are: {listing}"));
    }
    let mut notes = Vec::new();
    if names.len() > 1 {
        notes.push(format!(
            "sheets: {}",
            names
                .iter()
                .enumerate()
                .map(|(index, name)| format!("{} {name}", index + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok((
        Document {
            kind: KIND,
            unit: "sheet",
            total: names.len(),
            units,
            notes,
            unread,
        },
        selection,
    ))
}

/// Which rows hold a value, as bits.
#[derive(Default)]
struct RowSet {
    bits: Vec<u64>,
    count: usize,
}

impl RowSet {
    fn insert(&mut self, row: u32) {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let (word, bit) = (row / 64, row % 64);
        if word >= self.bits.len() {
            self.bits.resize(word + 1, 0);
        }
        if self.bits[word] & (1 << bit) == 0 {
            self.bits[word] |= 1 << bit;
            self.count += 1;
        }
    }

    /// The rows from `from` to `to` (inclusive), in order.
    fn between(&self, from: u32, to: u32) -> impl Iterator<Item = u32> + '_ {
        let from = usize::try_from(from).unwrap_or(usize::MAX);
        let end = usize::try_from(to)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(self.bits.len().saturating_mul(64));
        (from..end)
            .filter(move |row| self.bits[row / 64] & (1 << (row % 64)) != 0)
            .filter_map(|row| u32::try_from(row).ok())
    }
}

#[allow(clippy::too_many_lines)]
fn render_sheet(
    book: &mut Book,
    number: usize,
    name: &str,
    window: Window,
    options: &Options<'_>,
) -> Result<String, String> {
    let grid = book.grid(name)?;

    // First pass: where the values are.
    let mut used = RowSet::default();
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    book.scan(name, grid.as_ref(), &mut |row, column, _| {
        used.insert(row);
        bounds = Some(match bounds {
            None => (row, row, column, column),
            Some((top, bottom, left, right)) => (
                top.min(row),
                bottom.max(row),
                left.min(column),
                right.max(column),
            ),
        });
    })?;
    let Some((top, bottom, left, right)) = bounds else {
        return Ok(format!("## Sheet {number}: {name}\n\n(empty sheet)"));
    };
    let mut heading = format!(
        "## Sheet {number}: {name} ({} rows x {} columns)",
        bottom - top + 1,
        right - left + 1
    );

    let (row_from, row_to) = window.rows.unwrap_or((top, bottom));
    let (row_from, row_to) = (row_from.max(top), row_to.min(bottom));
    let (column_from, column_to) = window.columns.unwrap_or((left, right));
    let (column_from, column_to) = (column_from.max(left), column_to.min(right));
    if window.rows.is_some() || window.columns.is_some() {
        let mut parts = Vec::new();
        if let Some((from, to)) = window.rows {
            parts.push(format!(
                "rows {}-{}",
                from + 1,
                to.saturating_add(1).min(bottom + 1)
            ));
        }
        if let Some((from, to)) = window.columns {
            parts.push(format!(
                "columns {}-{}",
                column_name(from),
                column_name(to.min(right))
            ));
        }
        let _ = write!(heading, ", {}", parts.join(", "));
    }
    let shown_rows = if row_from <= row_to {
        used.between(row_from, row_to)
            .take(MAX_SHEET_ROWS + 1)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let more_rows = shown_rows.len() > MAX_SHEET_ROWS;
    let shown_rows = &shown_rows[..shown_rows.len().min(MAX_SHEET_ROWS)];
    let (Some(&first_row), Some(&last_row)) = (shown_rows.first(), shown_rows.last()) else {
        return Ok(format!(
            "{heading}\n\n(no values in these rows; the sheet has values in rows {}-{})",
            top + 1,
            bottom + 1
        ));
    };
    let last_column = if column_from <= column_to {
        column_to
            .min(column_from.saturating_add(u32::try_from(MAX_SHEET_COLUMNS).unwrap_or(40) - 1))
    } else {
        column_from
    };

    // Second pass: the values in the window, at most rows x columns shown.
    let mut values: BTreeMap<(u32, u32), String> = BTreeMap::new();
    if column_from <= column_to {
        book.scan(name, grid.as_ref(), &mut |row, column, value| {
            if (first_row..=last_row).contains(&row)
                && (column_from..=last_column).contains(&column)
            {
                values.insert((row, column), cell_text(value));
            }
        })?;
    }
    // Columns empty in every shown row are left off the right edge.
    let Some(widest) = values.keys().map(|(_, column)| *column).max() else {
        return Ok(format!(
            "{heading}\n\n(no values in these columns; the sheet has values in columns {}-{})",
            column_name(left),
            column_name(right)
        ));
    };
    let mut header = vec!["Row".to_string()];
    header.extend((column_from..=widest).map(column_name));
    let mut rows = vec![header];
    for &row in shown_rows {
        let mut cells = vec![(row + 1).to_string()];
        cells.extend(
            (column_from..=widest)
                .map(|column| values.get(&(row, column)).cloned().unwrap_or_default()),
        );
        rows.push(cells);
    }
    let mut text = format!("{heading}\n\n{}", markdown_table(&rows));

    let mut notes = Vec::new();
    let keep_columns = options
        .columns
        .map(str::trim)
        .filter(|columns| !columns.is_empty())
        .map_or_else(String::new, |columns| format!(" columns=\"{columns}\""));
    if more_rows {
        let end = window.rows.map_or_else(String::new, |(_, to)| {
            if to == u32::MAX {
                String::new()
            } else {
                (to + 1).to_string()
            }
        });
        notes.push(format!(
            "showing rows {}-{} ({} of {} non-empty rows); for more use pages=\"{number}\" rows=\"{}-{end}\"{keep_columns}",
            first_row + 1,
            last_row + 1,
            shown_rows.len(),
            used.count,
            last_row + 2
        ));
    }
    if column_to > last_column {
        let keep_rows = options
            .rows
            .map(str::trim)
            .filter(|rows| !rows.is_empty())
            .map_or_else(String::new, |rows| format!(" rows=\"{rows}\""));
        notes.push(format!(
            "showing columns {}-{} of {}-{}; for more use pages=\"{number}\" columns=\"{}-{}\"{keep_rows}",
            column_name(column_from),
            column_name(last_column),
            column_name(left),
            column_name(right),
            column_name(last_column + 1),
            column_name(column_to)
        ));
    }
    for note in notes {
        let _ = write!(text, "\n\n[{note}]");
    }
    Ok(text)
}

/// Column letters for a 0-based column index: 0 is `A`, 26 is `AA`.
pub(super) fn column_name(index: u32) -> String {
    let mut index = usize::try_from(index).unwrap_or(0);
    let mut name = Vec::new();
    loop {
        name.push(b'A' + u8::try_from(index % 26).unwrap_or(0));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// Whether a cell shows nothing: empty, or text that is only whitespace
/// (cleared cells often keep an empty shared string).
fn is_blank(cell: &Data) -> bool {
    match cell {
        Data::Empty => true,
        Data::String(text) | Data::DateTimeIso(text) | Data::DurationIso(text) => {
            text.trim().is_empty()
        }
        _ => false,
    }
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Float(value) => format_number(*value),
        Data::DateTime(value) if value.is_datetime() => {
            let (year, month, day, hour, minute, second, _) = value.to_ymd_hms_milli();
            if (hour, minute, second) == (0, 0, 0) {
                format!("{year:04}-{month:02}-{day:02}")
            } else {
                format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
            }
        }
        Data::DateTime(value) => format_number(value.as_f64()),
        other => other.to_string(),
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}
