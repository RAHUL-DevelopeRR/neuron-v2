//! Regressions from the fourth review: each test is one file shape that used
//! to slip past a check, misread, or leave part of a file out of reach.

use std::fmt::Write as _;
use std::path::Path;

use super::tests::{package, rels, row, temp_dir, write, xlsx, W};
use super::Options;

fn read(path: &Path, options: &Options<'_>) -> Result<String, String> {
    super::read_document(path, options)
}

fn read_default(path: &Path) -> Result<String, String> {
    read(path, &Options::default())
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
fn a_shared_string_claim_behind_markup_only_a_strict_reader_rejects_is_still_measured() {
    let dir = temp_dir("sst-lenient");
    let sheet = row(1, &["1"]);
    // A mismatched end tag before the table: calamine reads past it (it does
    // not match end tags) and would reserve room for the claimed count.
    for (name, sst) in [
        (
            "mismatched.xlsx",
            r#"<a></b><sst uniqueCount="300000000000000000"/>"#,
        ),
        (
            "mismatched-open.xlsx",
            r#"<?xml version="1.0"?><x><y></x></y><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" uniqueCount="300000000000000000"></sst>"#,
        ),
    ] {
        let bytes = workbook(&sheet, &[("xl/sharedStrings.xml", sst.to_string())]);
        let path = write(&dir, name, &bytes);
        let error = read_default(&path).expect_err("refused before calamine reserves");
        assert!(
            error.contains("claims more strings than it holds"),
            "{name}: {error}"
        );
    }
    // Markup no reader gets past before the table is refused as damaged,
    // not waved through as a table claiming nothing.
    let bytes = workbook(
        &sheet,
        &[(
            "xl/sharedStrings.xml",
            r#"<!-- never closed <sst uniqueCount="300000000000000000"/>"#.to_string(),
        )],
    );
    let path = write(&dir, "unclosed.xlsx", &bytes);
    let error = read_default(&path).expect_err("refused");
    assert!(error.contains("shared string table is damaged"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn text_mentioning_a_pdf_header_under_another_document_name_goes_to_read_file() {
    let dir = temp_dir("pdf-mention");
    for name in [
        "report.xls",
        "report.xlsx",
        "report.ods",
        "report.docx",
        "report.pptx",
    ] {
        let path = write(&dir, name, b"Name,Note\nA,100%PDF-ready\n");
        let error = read_default(&path).expect_err("text, not a document");
        assert!(error.contains("read it with read_file"), "{name}: {error}");
        // And read_file reads it: the two tools never send it back and forth.
        let text = runtime::read_file(path.to_string_lossy().as_ref(), None, None)
            .unwrap_or_else(|error| panic!("{name}: read_file reads it: {error}"))
            .file
            .content;
        assert!(text.contains("100%PDF-ready"), "{name}: {text}");
    }
    // A CSV whose first column is "PK" is text, not a broken zip package.
    let path = write(&dir, "keys.xls", b"PK,Name\n1,a\n");
    let error = read_default(&path).expect_err("text");
    assert!(error.contains("read it with read_file"), "{error}");
    assert!(runtime::read_file(path.to_string_lossy().as_ref(), None, None).is_ok());
    std::fs::remove_dir_all(dir).ok();
}

fn docx(body: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    package(&[("word/document.xml", document)])
}

fn cell(text: &str, properties: &str) -> String {
    format!("<w:tc><w:tcPr>{properties}</w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>")
}

#[test]
fn merged_word_table_cells_keep_later_cells_under_their_headers() {
    let dir = temp_dir("docx-merged");
    let plain = |texts: &[&str]| texts.iter().map(|text| cell(text, "")).collect::<String>();
    let span = |count: usize| format!(r#"<w:gridSpan w:val="{count}"/>"#);
    let body = format!(
        "<w:tbl><w:tblGrid><w:gridCol/><w:gridCol/><w:gridCol/><w:gridCol/></w:tblGrid>\
<w:tr>{}</w:tr>\
<w:tr>{}{}</w:tr>\
<w:tr><w:trPr><w:gridBefore w:val=\"1\"/></w:trPr>{}</w:tr>\
<w:tr><w:trPr><w:gridAfter w:val=\"1\"/></w:trPr>{}{}</w:tr>\
<w:tr>{}{}</w:tr>\
</w:tbl>",
        plain(&["R1C1", "R1C2", "R1C3", "R1C4"]),
        cell("MergedAB", &span(2)),
        plain(&["R2C3", "R2C4"]),
        plain(&["R3C2", "R3C3", "R3C4"]),
        cell("R4AB", &span(2)),
        plain(&["R4C3"]),
        // A tracked change's copy of the old properties is not the layout.
        cell(
            "R5C1",
            &format!(
                r#"<w:tcPrChange w:id="1"><w:tcPr>{}</w:tcPr></w:tcPrChange>"#,
                span(3)
            ),
        ),
        plain(&["R5C2", "R5C3", "R5C4"]),
    );
    let path = write(&dir, "merged.docx", &docx(&body));
    let out = read_default(&path).expect("reads");
    for line in [
        "| R1C1 | R1C2 | R1C3 | R1C4 |",
        "| MergedAB |  | R2C3 | R2C4 |",
        "|  | R3C2 | R3C3 | R3C4 |",
        "| R4AB |  | R4C3 |  |",
        "| R5C1 | R5C2 | R5C3 | R5C4 |",
    ] {
        assert!(out.lines().any(|shown| shown == line), "{line}\n{out}");
    }
    // A hostile span adds no more than a table's columns.
    let body = format!(
        "<w:tbl><w:tr>{}{}</w:tr></w:tbl>",
        cell("Wide", &span(usize::MAX)),
        plain(&["After"])
    );
    let path = write(&dir, "wide.docx", &docx(&body));
    let out = read_default(&path).expect("reads");
    assert!(out.contains("| Wide |"), "{out}");
    assert!(out.len() < 2_000, "{}", out.len());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn times_of_day_and_durations_read_as_excel_shows_them() {
    let dir = temp_dir("times");
    let styles = r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="[h]:mm"/></numFmts><cellXfs count="5"><xf numFmtId="0"/><xf numFmtId="20" applyNumberFormat="1"/><xf numFmtId="18" applyNumberFormat="1"/><xf numFmtId="164" applyNumberFormat="1"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>"#;
    let cells = [
        ("A", 4, "45353"),
        ("B", 1, "0.375"),
        ("C", 2, "0.70833"),
        ("D", 3, "0.33333333333"),
        ("E", 3, "1.5"),
        ("F", 1, "45353.5"),
        ("G", 1, "0.5104166667"),
    ];
    let mut rows = String::from("<row r=\"1\">");
    for (column, style, value) in cells {
        let _ = write!(rows, "<c r=\"{column}1\" s=\"{style}\"><v>{value}</v></c>");
    }
    rows.push_str("</row>");
    let path = write(
        &dir,
        "times.xlsx",
        &workbook(&rows, &[("xl/styles.xml", styles.to_string())]),
    );
    let out = read_default(&path).expect("reads");
    assert!(
        out.contains(
            "| 1 | 2024-03-02 | 09:00 | 17:00 | 8:00:00 | 36:00:00 | 2024-03-02 12:00:00 | 12:15 |"
        ),
        "{out}"
    );
    assert!(!out.contains("1899"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_sheet_row_longer_than_one_call_is_cut_per_cell_so_every_row_is_reachable() {
    let dir = temp_dir("long-cells");
    let long = "x".repeat(3_600);
    let cells = vec![long.as_str(); 12];
    let mut rows = row(
        1,
        &[
            "Q1", "Q2", "Q3", "Q4", "Q5", "Q6", "Q7", "Q8", "Q9", "Q10", "Q11", "Q12",
        ],
    );
    for number in 2..=6 {
        rows.push_str(&row(number, &cells));
    }
    let path = write(&dir, "survey.xlsx", &xlsx(&[("Answers", rows)]));
    let ceiling = super::MAX_MAX_CHARS;

    // Each call at the ceiling ends on a whole row and names the next one.
    let mut next = 2;
    let mut attempts = 0;
    while next <= 6 {
        attempts += 1;
        assert!(attempts < 10, "no progress past row {next}");
        let from = format!("{next}-");
        let out = read(
            &path,
            &Options {
                rows: Some(&from),
                max_chars: Some(ceiling),
                ..Options::default()
            },
        )
        .expect("reads");
        assert!(out.contains("[cells longer than"), "{out}");
        assert!(out.contains("columns=\"A\""), "{out}");
        let whole = out
            .lines()
            .filter(|line| line.starts_with("| ") && line.ends_with(" |"))
            .filter_map(|line| line[2..].split(' ').next()?.parse::<usize>().ok())
            .collect::<Vec<_>>();
        assert!(whole.contains(&next), "row {next} is shown whole:\n{out}");
        let last = *whole.iter().max().expect("a row");
        if last == 6 {
            break;
        }
        let hint = out.lines().last().expect("hint");
        assert!(hint.contains(&format!("rows=\"{}-\"", last + 1)), "{hint}");
        next = last + 1;
    }

    // One column shows a cut cell in full.
    let out = read(
        &path,
        &Options {
            rows: Some("2"),
            columns: Some("A"),
            max_chars: Some(ceiling),
            ..Options::default()
        },
    )
    .expect("reads");
    assert!(out.contains(&format!("| 2 | {long} |")), "{out}");
    assert!(!out.contains("[cells longer than"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}
