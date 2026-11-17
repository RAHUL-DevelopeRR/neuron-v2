//! Regressions from the sixth review: text `calamine` reserves or writes
//! out for an `.xls` file that its first size check missed, glyph names that
//! stood for unbounded text, and padded sheet cells marked as cut.

use std::fmt::Write as _;

use super::pdf::fixtures;
use super::round5_tests::{biff, compound_file, read, read_pdf, workbook, workbook_stream};
use super::tests::{temp_dir, write};
use super::Options;

/// A STRING record (a formula's text result) claiming `chars` characters
/// and holding none.
fn empty_string(chars: u16) -> Vec<u8> {
    let mut data = chars.to_le_bytes().to_vec();
    data.push(0);
    biff(0x0207, &data)
}

/// A LABEL record at `row` claiming `chars` characters and holding none.
fn empty_label(row: u16, chars: u16) -> Vec<u8> {
    let mut data = row.to_le_bytes().to_vec();
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&chars.to_le_bytes());
    data.push(0);
    biff(0x0204, &data)
}

/// A defined name (Lbl) record naming `name` (written as UTF-16) with no
/// formula of its own.
fn defined_name(name: &str) -> Vec<u8> {
    let units = name.encode_utf16().collect::<Vec<_>>();
    let mut data = vec![0u8; 14];
    data[3] = u8::try_from(units.len()).expect("name");
    data.push(0x01);
    for unit in units {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    biff(0x0018, &data)
}

/// A FORMULA record at `row`, column A, whose parsed expression is `tokens`.
fn formula(row: u16, tokens: &[u8]) -> Vec<u8> {
    let mut data = row.to_le_bytes().to_vec();
    data.resize(20, 0);
    data.extend_from_slice(&u16::try_from(tokens.len()).expect("cce").to_le_bytes());
    data.extend_from_slice(tokens);
    biff(0x0006, &data)
}

/// `PtgName`: defined name `index` (numbered from 1).
fn name_token(index: u32) -> Vec<u8> {
    let mut token = vec![0x23];
    token.extend_from_slice(&index.to_le_bytes());
    token
}

#[test]
fn xls_string_records_count_the_length_they_claim_not_the_text_they_hold() {
    let max_cells = super::sheet::MAX_GRID_CELLS;
    // calamine reserves 65,535 bytes for each before decoding nothing, and
    // keeps every one, even all at A1 with no formula before them.
    let records = (0..10).map(|_| empty_string(u16::MAX)).collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&[], &records));
    let error = super::xls::check_with(&file, max_cells, 1_000_000).expect_err("refused");
    assert!(error.contains("copies more than"), "{error}");
    assert!(super::xls::check_with(&file, max_cells, 4_000_000).is_ok());
    // Labels the same.
    let labels = (0..10u16)
        .map(|row| empty_label(row, u16::MAX))
        .collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&[], &labels));
    let error = super::xls::check_with(&file, max_cells, 1_000_000).expect_err("refused");
    assert!(error.contains("copies more than"), "{error}");
    // Short claims stay small.
    let short = (0..10).map(|_| empty_string(3)).collect::<Vec<_>>();
    let file = compound_file("Workbook", &workbook_stream(&[], &short));
    assert!(super::xls::check_with(&file, max_cells, 1_000).is_ok());

    // 1,400 such records, 7 bytes each, would reserve about 92 GB.
    let dir = temp_dir("xls-empty-strings");
    let records = (0..1_400)
        .map(|_| empty_string(u16::MAX))
        .collect::<Vec<_>>();
    let bytes = compound_file("Workbook", &workbook_stream(&[], &records));
    assert!(bytes.len() < 20_000, "{}", bytes.len());
    let path = write(&dir, "empty-strings.xls", &bytes);
    let error = read(&path, &Options::default()).expect_err("refused");
    assert!(error.contains("copies more than 256 MB of text"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn xls_formula_text_counts_the_defined_names_it_writes_out() {
    let max_cells = super::sheet::MAX_GRID_CELLS;
    // A name of 255 CJK characters is 765 bytes of text each time a
    // five-byte token names it.
    let name = defined_name(&"\u{4e00}".repeat(255));
    let one = [formula(0, &name_token(1))];
    let file = compound_file("Workbook", &workbook_stream(&name, &one));
    let error = super::xls::check_with(&file, max_cells, 1_000).expect_err("refused");
    assert!(error.contains("copies more than"), "{error}");
    assert!(super::xls::check_with(&file, max_cells, 2_000).is_ok());
    // A token naming no defined name writes `#REF!`.
    let missing = [formula(0, &name_token(2)), formula(1, &name_token(0))];
    let file = compound_file("Workbook", &workbook_stream(&name, &missing));
    assert!(super::xls::check_with(&file, max_cells, 1_000).is_ok());
    // An ordinary formula, `=1+2`, stays small.
    let sum = [formula(0, &[0x1E, 1, 0, 0x1E, 2, 0, 0x03])];
    let file = compound_file("Workbook", &workbook_stream(&[], &sum));
    assert!(super::xls::check_with(&file, max_cells, 100).is_ok());

    // Twenty records at one cell, each naming the name 10,801 times joined
    // by `+`: about 330 MB of text from a 1.3 MB file.
    let mut tokens = name_token(1);
    for _ in 0..10_800 {
        tokens.extend(name_token(1));
        tokens.push(0x03);
    }
    let records = (0..20).map(|_| formula(0, &tokens)).collect::<Vec<_>>();
    let bytes = compound_file("Workbook", &workbook_stream(&name, &records));
    assert!(bytes.len() < 1_500_000, "{}", bytes.len());
    let dir = temp_dir("xls-formula-names");
    let path = write(&dir, "names.xls", &bytes);
    let error = read(&path, &Options::default()).expect_err("refused");
    assert!(error.contains("copies more than 256 MB of text"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn glyph_names_standing_for_long_text_read_as_unknown() {
    // One name of 250,000 characters named by all 256 codes of a font's
    // `/Differences`, and the encoding shared by two fonts.
    let huge = format!("/uni{}", "4E00".repeat(250_000));
    let mut differences = "0".to_string();
    for _ in 0..256 {
        differences.push_str(" 8 0 R");
    }
    differences.push_str(" 66 /fi");
    let content = b"BT /F1 10 Tf 72 700 Td (AB) Tj ET\nBT /F2 10 Tf 72 680 Td (BA) Tj ET";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", content, true),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /One /Encoding 7 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Two /Encoding 7 0 R >>".to_vec(),
        format!("<< /Type /Encoding /Differences [{differences}] >>").into_bytes(),
        huge.into_bytes(),
    ];
    let text = read_pdf(&fixtures::assemble(&objects, ""));
    // The long name is unknown, so `A` keeps its own meaning; `B` is `fi`.
    assert!(!text.contains('\u{4e00}'), "{}", text.len());
    assert!(text.contains("Afi"), "{text}");
    assert!(text.contains("fiA"), "{text}");
}

#[test]
fn cells_padded_with_spaces_are_not_cut_for_their_padding() {
    let dir = temp_dir("padded-cells");
    // Forty columns leave each cell a few hundred characters; column A
    // holds `abc` and 5,000 spaces, as fixed-width exports write it.
    let mut rows = String::new();
    for row in 1..=3 {
        let _ = write!(
            rows,
            "<row r=\"{row}\"><c r=\"A{row}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">abc{}</t></is></c>",
            " ".repeat(5_000)
        );
        for column in 1..40u32 {
            let _ = write!(
                rows,
                "<c r=\"{}{row}\"><v>{column}</v></c>",
                super::sheet::column_name(column)
            );
        }
        rows.push_str("</row>");
    }
    let path = write(&dir, "padded.xlsx", &workbook(&rows, &[]));
    let out = read(
        &path,
        &Options {
            max_chars: Some(36_000),
            ..Options::default()
        },
    )
    .expect("reads");
    assert!(out.contains("| 1 | abc | 1 | 2 |"), "{out}");
    assert!(!out.contains("[...]"), "{out}");
    assert!(!out.contains("characters are cut"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}
