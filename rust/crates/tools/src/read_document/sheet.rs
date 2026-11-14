//! Spreadsheets (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`) through `calamine`:
//! each sheet as a Markdown table with row numbers and column letters.

use std::fmt::Write as _;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};

use super::{markdown_table, Document, Selection, Unit};

const KIND: &str = "spreadsheet";

/// Non-empty rows shown per sheet.
pub(super) const MAX_SHEET_ROWS: usize = 500;
/// Columns shown per sheet.
pub(super) const MAX_SHEET_COLUMNS: usize = 40;

pub(super) fn read(path: &Path, selection: &Selection) -> Result<Document, String> {
    let mut workbook = open_workbook_auto(path).map_err(|error| {
        format!("this is not a readable {KIND}: the file is damaged, password protected or not a workbook ({error})")
    })?;
    let names = workbook.sheet_names();
    let mut units = Vec::new();
    let mut matched_names = 0;
    for (index, name) in names.iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, Some(name)) {
            continue;
        }
        matched_names += 1;
        let markdown = match workbook.worksheet_range(name) {
            Ok(range) => render_sheet(number, name, &range),
            Err(error) => {
                format!("## Sheet {number}: {name}\n\n(this sheet cannot be read: {error})")
            }
        };
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
    Ok(Document {
        kind: KIND,
        unit: "sheet",
        total: names.len(),
        units,
        notes,
    })
}

/// Column letters for a 0-based column index: 0 is `A`, 26 is `AA`.
pub(super) fn column_name(mut index: usize) -> String {
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

fn render_sheet(number: usize, name: &str, range: &calamine::Range<Data>) -> String {
    let (height, width) = range.get_size();
    let heading = format!("## Sheet {number}: {name} ({height} rows x {width} columns)");
    if height == 0 || width == 0 {
        return format!("{heading}\n\n(empty sheet)");
    }
    let (first_row, first_column) = range
        .start()
        .map_or((0, 0), |(row, column)| (row as usize, column as usize));
    let shown_columns = width.min(MAX_SHEET_COLUMNS);
    let mut header = vec!["Row".to_string()];
    header.extend((0..shown_columns).map(|offset| column_name(first_column + offset)));
    let mut rows = vec![header];
    let mut non_empty = 0;
    let mut last_shown_row = first_row;
    for (offset, row) in range.rows().enumerate() {
        if row.iter().all(|cell| matches!(cell, Data::Empty)) {
            continue;
        }
        non_empty += 1;
        if non_empty > MAX_SHEET_ROWS {
            continue;
        }
        last_shown_row = first_row + offset;
        let mut cells = vec![(first_row + offset + 1).to_string()];
        cells.extend(row.iter().take(shown_columns).map(cell_text));
        rows.push(cells);
    }
    let mut text = format!("{heading}\n\n{}", markdown_table(&rows));
    let mut cut = Vec::new();
    if non_empty > MAX_SHEET_ROWS {
        cut.push(format!(
            "showing the first {MAX_SHEET_ROWS} of {non_empty} non-empty rows (to row {})",
            last_shown_row + 1
        ));
    }
    if width > MAX_SHEET_COLUMNS {
        cut.push(format!(
            "showing columns {}-{} of {width}",
            column_name(first_column),
            column_name(first_column + shown_columns - 1)
        ));
    }
    if !cut.is_empty() {
        let _ = write!(text, "\n\n[{}]", cut.join("; "));
    }
    text
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
