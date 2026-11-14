//! Regressions from the fifth review: each test is one file shape that used
//! to ask for far more memory than the file's size, cut text it had room to
//! show, or misread a font.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use super::pdf::fixtures;
use super::tests::{docx, package, rels, temp_dir, write};
use super::{Budget, Options, Selection, DEFAULT_MAX_CHARS};

fn read(path: &Path, options: &Options<'_>) -> Result<String, String> {
    super::read_document(path, options)
}

/// A one-sheet `.xlsx` holding `rows`, with `extra` parts.
fn workbook(rows: &str, extra: &[(&str, String)]) -> Vec<u8> {
    let workbook = r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let sheet = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
    );
    let mut parts = vec![
        (
            "_rels/.rels",
            rels(&[("rId1", "officeDocument", "xl/workbook.xml")]),
        ),
        ("xl/workbook.xml", workbook.to_string()),
        (
            "xl/_rels/workbook.xml.rels",
            rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")]),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    parts.extend(extra.iter().map(|(name, xml)| (*name, xml.clone())));
    package(&parts)
}

#[test]
fn one_long_shared_string_in_thousands_of_cells_is_cut_as_it_is_read() {
    let dir = temp_dir("sst-fanout");
    // One 1 MB string (about 1 KB zipped) named by every cell of a 500 x 40
    // window: copied whole per cell, that is 20 GB.
    let table = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="20000" uniqueCount="1"><si><t>{}</t></si></sst>"#,
        "a".repeat(1_000_000)
    );
    let mut rows = String::new();
    for row in 1..=500 {
        let _ = write!(rows, "<row r=\"{row}\">");
        for column in 0..40u32 {
            let _ = write!(
                rows,
                "<c r=\"{}{row}\" t=\"s\"><v>0</v></c>",
                super::sheet::column_name(column)
            );
        }
        rows.push_str("</row>");
    }
    let bytes = workbook(&rows, &[("xl/sharedStrings.xml", table)]);
    assert!(bytes.len() < 200_000, "{}", bytes.len());
    let path = write(&dir, "fanout.xlsx", &bytes);
    let started = Instant::now();
    let out = read(
        &path,
        &Options {
            max_chars: Some(super::MAX_MAX_CHARS),
            ..Options::default()
        },
    )
    .expect("reads");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    assert!(out.contains("(500 rows x 40 columns)"), "{}", &out[..500]);
    assert!(out.contains("characters are cut"), "{}", &out[..1_000]);
    assert!(out.contains("aaaa [...] |"), "{}", &out[..1_000]);
    assert!(out.len() <= super::MAX_MAX_CHARS * 4, "{}", out.len());

    // One column of the same cells shows each cut at the one-column size.
    let out = read(
        &path,
        &Options {
            columns: Some("A"),
            rows: Some("1-3"),
            max_chars: Some(super::MAX_MAX_CHARS),
            ..Options::default()
        },
    )
    .expect("reads");
    let ceiling = super::max_chars_ceiling();
    let one_column = ceiling * 5 / 6;
    let shown = format!("| 1 | {} [...] |", "a".repeat(one_column));
    assert!(out.contains(&shown), "{}", &out[..300]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cells_kept_before_a_wider_column_appears_are_cut_to_the_final_width() {
    let dir = temp_dir("sst-widening");
    let long = "b".repeat(10_000);
    // Column A first in every row, then one value further right in the last
    // row: the cells kept whole early are cut to what three columns allow.
    let mut rows = String::new();
    for row in 1..=3 {
        let _ = write!(
            rows,
            "<row r=\"{row}\"><c r=\"A{row}\" t=\"inlineStr\"><is><t>{long}</t></is></c></row>"
        );
    }
    rows.push_str(r#"<row r="4"><c r="C4"><v>7</v></c></row>"#);
    let path = write(&dir, "widening.xlsx", &workbook(&rows, &[]));
    let out = read(
        &path,
        &Options {
            max_chars: Some(super::MAX_MAX_CHARS),
            ..Options::default()
        },
    )
    .expect("reads");
    let ceiling = super::max_chars_ceiling();
    let per_cell = (ceiling * 2 / 3).min(super::MAX_ROW_LINE_CHARS) / 3;
    let shown = format!("| 1 | {} [...] |  |  |", "b".repeat(per_cell));
    assert!(out.contains(&shown), "{out}");
    assert!(out.contains("| 4 |  |  | 7 |"), "{out}");
    assert!(
        out.contains(&format!("cells longer than {per_cell} characters are cut")),
        "{out}"
    );
    std::fs::remove_dir_all(dir).ok();
}

/// A compound file (512-byte sectors) holding one stream, with as many
/// allocation sectors as the stream needs.
fn compound_file(name: &str, stream: &[u8]) -> Vec<u8> {
    const FREE: u32 = 0xFFFF_FFFF;
    const END: u32 = 0xFFFF_FFFE;
    let mut stream = stream.to_vec();
    if stream.len() < 4096 {
        let pad = 4096 - stream.len();
        stream.extend_from_slice(&[0x55, 0x00]);
        stream.extend_from_slice(&u16::try_from(pad - 4).expect("pad").to_le_bytes());
        stream.resize(4096, 0);
    }
    let sectors = stream.len().div_ceil(512);
    // Allocation sectors, then the directory, then the stream.
    let fat_sectors = (sectors + 1).div_ceil(127);
    assert!(fat_sectors <= 109, "fixture too large");
    let directory = fat_sectors;
    let first = fat_sectors + 1;
    let id = |index: usize| u32::try_from(index).expect("sector");
    let mut fat = vec![FREE; fat_sectors * 128];
    for slot in fat.iter_mut().take(fat_sectors) {
        *slot = 0xFFFF_FFFD;
    }
    fat[directory] = END;
    for index in 0..sectors {
        fat[first + index] = if index + 1 < sectors {
            id(first + index + 1)
        } else {
            END
        };
    }
    let mut header = vec![0u8; 512];
    header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    header[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    header[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    header[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    header[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    header[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    header[0x2C..0x30].copy_from_slice(&id(fat_sectors).to_le_bytes());
    header[0x30..0x34].copy_from_slice(&id(directory).to_le_bytes());
    header[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
    header[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    header[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for slot in 0..109 {
        let sector = if slot < fat_sectors { id(slot) } else { FREE };
        header[0x4C + slot * 4..0x50 + slot * 4].copy_from_slice(&sector.to_le_bytes());
    }
    let entry = |entry_name: &str, kind: u8, child: u32, start: u32, size: usize| {
        let mut entry = vec![0u8; 128];
        let units = entry_name.encode_utf16().collect::<Vec<_>>();
        for (index, unit) in units.iter().enumerate() {
            entry[index * 2..index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entry[0x40..0x42].copy_from_slice(
            &u16::try_from(units.len() * 2 + 2)
                .expect("name")
                .to_le_bytes(),
        );
        entry[0x42] = kind;
        entry[0x43] = 1;
        entry[0x44..0x48].copy_from_slice(&FREE.to_le_bytes());
        entry[0x48..0x4C].copy_from_slice(&FREE.to_le_bytes());
        entry[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
        entry[0x74..0x78].copy_from_slice(&start.to_le_bytes());
        entry[0x78..0x7C].copy_from_slice(&u32::try_from(size).expect("size").to_le_bytes());
        entry
    };
    let mut file = header;
    file.extend(fat.iter().flat_map(|sector| sector.to_le_bytes()));
    let mut entries = entry("Root Entry", 5, 1, END, 0);
    entries.extend(entry(name, 2, FREE, id(first), stream.len()));
    entries.resize(512, 0);
    file.extend(entries);
    file.extend(&stream);
    file.resize(512 * (first + sectors + 1), 0);
    file
}

fn biff(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut record = kind.to_le_bytes().to_vec();
    record.extend_from_slice(&u16::try_from(data.len()).expect("record").to_le_bytes());
    record.extend_from_slice(data);
    record
}

/// A BIFF8 SST record and its CONTINUE records holding `strings`, each
/// written as UTF-16 (`(text, rich runs)`), split at `part` bytes.
fn sst(strings: &[(String, u16)], part: usize) -> Vec<u8> {
    // Each piece of the table: a string header, or characters.
    let mut first = Vec::new();
    let count = u32::try_from(strings.len()).expect("count");
    first.extend_from_slice(&count.to_le_bytes());
    first.extend_from_slice(&count.to_le_bytes());
    let mut records = Vec::new();
    let mut current = first;
    for (text, runs) in strings {
        let units = text.encode_utf16().collect::<Vec<_>>();
        let mut header = u16::try_from(units.len())
            .expect("cch")
            .to_le_bytes()
            .to_vec();
        header.push(if *runs > 0 { 0x09 } else { 0x01 });
        if *runs > 0 {
            header.extend_from_slice(&runs.to_le_bytes());
        }
        if current.len() + header.len() + 2 > part {
            records.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(&header);
        for unit in units {
            if current.len() + 2 > part {
                records.push(std::mem::take(&mut current));
                // A string running on starts its part with its width flag.
                current.push(0x01);
            }
            current.extend_from_slice(&unit.to_le_bytes());
        }
        for _ in 0..*runs {
            if current.len() + 4 > part {
                records.push(std::mem::take(&mut current));
            }
            current.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    records.push(current);
    let mut out = Vec::new();
    for (index, data) in records.iter().enumerate() {
        out.extend(biff(if index == 0 { 0x00FC } else { 0x003C }, data));
    }
    out
}

/// A BIFF8 workbook stream: `globals` records, then one sheet of
/// `sheet_records`.
fn workbook_stream(globals: &[u8], sheet_records: &[Vec<u8>]) -> Vec<u8> {
    let bof = |kind: u16| {
        let mut data = vec![0u8; 16];
        data[..2].copy_from_slice(&0x0600u16.to_le_bytes());
        data[2..4].copy_from_slice(&kind.to_le_bytes());
        biff(0x0809, &data)
    };
    let name = b"Data";
    let bound_sheet = |offset: u32| {
        let mut data = offset.to_le_bytes().to_vec();
        data.extend_from_slice(&[0, 0, u8::try_from(name.len()).expect("name"), 0]);
        data.extend_from_slice(name);
        biff(0x0085, &data)
    };
    let globals_length =
        bof(0x0005).len() + globals.len() + bound_sheet(0).len() + biff(0x000A, &[]).len();
    let mut stream = bof(0x0005);
    stream.extend_from_slice(globals);
    stream.extend(bound_sheet(u32::try_from(globals_length).expect("offset")));
    stream.extend(biff(0x000A, &[]));
    stream.extend(bof(0x0010));
    for record in sheet_records {
        stream.extend_from_slice(record);
    }
    stream.extend(biff(0x000A, &[]));
    stream
}

fn label_sst(row: u16, column: u16, index: u32) -> Vec<u8> {
    let mut data = row.to_le_bytes().to_vec();
    data.extend_from_slice(&column.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&index.to_le_bytes());
    biff(0x00FD, &data)
}

fn label(row: u16, column: u16, text: &str) -> Vec<u8> {
    let mut data = row.to_le_bytes().to_vec();
    data.extend_from_slice(&column.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&u16::try_from(text.len()).expect("cch").to_le_bytes());
    data.push(0);
    data.extend_from_slice(text.as_bytes());
    biff(0x0204, &data)
}

#[test]
fn xls_cells_copying_one_long_shared_string_are_measured_before_calamine_opens_them() {
    let dir = temp_dir("xls-sst-fanout");
    // One string of 65,535 CJK characters (196,605 bytes as UTF-8), and 1,400
    // cells that each get a copy of it: about 275 MB from a 150 KB file.
    let long = "\u{4e00}".repeat(65_535);
    let table = sst(&[(long.clone(), 0)], 8_224);
    let cells = (0..1_400u16)
        .map(|row| label_sst(row, 0, 0))
        .collect::<Vec<_>>();
    let bytes = compound_file("Workbook", &workbook_stream(&table, &cells));
    assert!(bytes.len() < 200_000, "{}", bytes.len());
    let path = write(&dir, "fanout.xls", &bytes);
    let error = read(&path, &Options::default()).expect_err("refused");
    assert!(error.contains("copies more than 256 MB of text"), "{error}");

    // A hundred such cells read, each cut to what the table shows.
    let cells = (0..100u16)
        .map(|row| label_sst(row, 0, 0))
        .collect::<Vec<_>>();
    let bytes = compound_file("Workbook", &workbook_stream(&table, &cells));
    let path = write(&dir, "some.xls", &bytes);
    let out = read(
        &path,
        &Options {
            max_chars: Some(super::MAX_MAX_CHARS),
            ..Options::default()
        },
    )
    .expect("reads");
    let start = out.chars().take(300).collect::<String>();
    let one_column = super::max_chars_ceiling() * 5 / 6;
    let shown = format!("| 1 | {} [...] |", "\u{4e00}".repeat(one_column));
    assert!(out.contains(&shown), "{start}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn xls_text_is_counted_per_string_and_per_inline_label() {
    let limit = 2_000;
    let max_cells = super::sheet::MAX_GRID_CELLS;
    // A rich string first (its formatting runs are skipped), then a long
    // one: cells naming the short one pass, cells naming the long one do not.
    let table = sst(&[("ab".to_string(), 3), ("x".repeat(400), 0)], 64);
    let short = (0..10u16)
        .map(|row| label_sst(row, 0, 0))
        .collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&table, &short));
    assert!(super::xls::check_with(&file, max_cells, limit).is_ok());
    let long = (0..10u16)
        .map(|row| label_sst(row, 0, 1))
        .collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&table, &long));
    let error = super::xls::check_with(&file, max_cells, limit).expect_err("refused");
    assert!(error.contains("copies more than"), "{error}");
    // A cell naming no string copies nothing.
    let missing = (0..10u16)
        .map(|row| label_sst(row, 0, 7))
        .collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&table, &missing));
    assert!(super::xls::check_with(&file, max_cells, limit).is_ok());

    // Labels holding their own text count too.
    let text = "y".repeat(100);
    let labels = (0..10u16)
        .map(|row| label(row, 0, &text))
        .collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&[], &labels));
    let error = super::xls::check_with(&file, max_cells, limit).expect_err("refused");
    assert!(error.contains("copies more than"), "{error}");
    let file = compound_file("Workbook", &workbook_stream(&[], &labels[..5]));
    assert!(super::xls::check_with(&file, max_cells, limit).is_ok());

    // A table cut short is refused, as calamine would fail on it.
    let mut cut = sst(&[("z".repeat(50), 0)], 8_224);
    cut.truncate(cut.len() - 20);
    let length = u16::try_from(cut.len() - 4).expect("length");
    cut[2..4].copy_from_slice(&length.to_le_bytes());
    let file = compound_file("Workbook", &workbook_stream(&cut, &short));
    assert!(super::xls::check_with(&file, max_cells, limit).is_err());
}

fn word_cell(text: &str, span: usize) -> String {
    format!(
        r#"<w:tc><w:tcPr><w:gridSpan w:val="{span}"/></w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"#
    )
}

#[test]
fn merged_word_cells_keep_the_room_of_the_columns_they_cover() {
    let dir = temp_dir("docx-merged-room");
    // A 16-column form: label/value pairs four columns wide, then a long
    // description twelve columns wide.
    let description = "word ".repeat(600);
    let description = description.trim_end();
    let grid = "<w:gridCol/>".repeat(16);
    let body = format!(
        "<w:tbl><w:tblGrid>{grid}</w:tblGrid><w:tr>{}{}{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
        word_cell("Name", 4),
        word_cell("Ada", 4),
        word_cell("Role", 4),
        word_cell("Engineer", 4),
        word_cell("Description", 4),
        word_cell(description, 12),
    );
    let path = write(&dir, "form.docx", &docx(&body));
    let out = read(
        &path,
        &Options {
            max_chars: Some(super::MAX_MAX_CHARS),
            ..Options::default()
        },
    )
    .expect("reads");
    assert!(out.contains(description), "{out}");
    assert!(!out.contains("[...]"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_row_shares_its_room_with_the_cells_that_hold_text() {
    // Short cells keep their text; the long ones share what is left.
    assert_eq!(super::fair_share(vec![10, 5_000, 5_000], 6_000), 2_995);
    assert_eq!(super::fair_share(vec![10, 20], 6_000), usize::MAX);
    assert_eq!(super::fair_share(Vec::new(), 6_000), usize::MAX);
    assert_eq!(super::fair_share(vec![9_000; 4], 24_000), 6_000);
}

fn read_pdf(bytes: &[u8]) -> String {
    let document = super::pdf::read_bytes(
        bytes,
        &Selection::default(),
        &mut Budget::new(DEFAULT_MAX_CHARS),
    )
    .expect("reads");
    document
        .units
        .iter()
        .map(|unit| unit.markdown.clone())
        .collect()
}

#[test]
fn simple_fonts_read_their_encoding_and_its_differences() {
    let content =
        b"BT /F1 10 Tf 72 700 Td (e\\016cient \\014nance 1990\\1731995 \\134quoted) Tj ET\n\
BT /F2 10 Tf 72 680 Td (caf\\216 \\322ok\\323) Tj ET\n\
BT /F3 10 Tf 72 660 Td (it\\047s \\256ne) Tj ET\n\
BT /F4 10 Tf 72 640 Td (plain caf\\351) Tj ET";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 8 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", content, true),
        // pdfTeX's Computer Modern (OT1) without a ToUnicode map.
        b"<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /Encoding << /Type /Encoding /Differences [11 /ff /fi /fl /ffi 123 /endash 92 /quotedblleft] >> >>".to_vec(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Geneva /Encoding /MacRomanEncoding >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding << /BaseEncoding /StandardEncoding >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let text = read_pdf(&fixtures::assemble(&objects, ""));
    assert!(
        text.contains("efficient finance 1990\u{2013}1995"),
        "{text}"
    );
    assert!(text.contains("\u{201c}quoted"), "{text}");
    assert!(text.contains("caf\u{e9} \u{201c}ok\u{201d}"), "{text}");
    assert!(text.contains("it\u{2019}s fine"), "{text}");
    // A font without an encoding still reads as Windows-1252.
    assert!(text.contains("plain caf\u{e9}"), "{text}");
}
