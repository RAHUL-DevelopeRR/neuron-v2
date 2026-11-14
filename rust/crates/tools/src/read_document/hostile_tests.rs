//! Hostile and awkward files: each of these once crashed, hung or misread.
#![allow(clippy::format_collect)]

use super::pdf::fixtures;
use super::tests::{package, row, sample_docx, temp_dir, write, xlsx, W};
use super::{Budget, Options, Selection, DEFAULT_MAX_CHARS};
use std::fmt::Write as _;
use std::path::Path;

fn read_document(
    path: &Path,
    pages: Option<&str>,
    max_chars: Option<usize>,
) -> Result<String, String> {
    super::read_document(
        path,
        &Options {
            pages,
            max_chars,
            ..Options::default()
        },
    )
}

fn read_sheet(
    path: &Path,
    pages: Option<&str>,
    rows: Option<&str>,
    columns: Option<&str>,
) -> Result<String, String> {
    super::read_document(
        path,
        &Options {
            pages,
            rows,
            columns,
            max_chars: Some(100_000),
        },
    )
}

fn read_pdf(bytes: &[u8]) -> Result<super::Document, String> {
    super::pdf::read_bytes(
        bytes,
        &Selection::default(),
        &mut Budget::new(DEFAULT_MAX_CHARS),
    )
}

#[test]
fn a_two_cell_sheet_spanning_the_whole_grid_is_streamed_not_allocated() {
    // A1 and XFD1048576: as one dense grid this is 17 billion cells.
    let dir = temp_dir("xlsx-span");
    let cells = "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row><row r=\"1048576\"><c r=\"XFD1048576\"><v>2</v></c></row>";
    let path = write(&dir, "span.xlsx", &xlsx(&[("Span", cells.to_string())]));
    let out = read_document(&path, None, None).expect("reads without building the grid");
    assert!(
        out.contains("## Sheet 1: Span (1048576 rows x 16384 columns)"),
        "{out}"
    );
    assert!(
        out.contains("| Row | A |\n| --- | --- |\n| 1 | 1 |\n| 1048576 |  |"),
        "{out}"
    );
    assert!(
        out.contains("for more use pages=\"1\" columns=\"AO-XFD\""),
        "{out}"
    );
    let far = read_sheet(&path, None, Some("1048576"), Some("XFD")).expect("far corner");
    assert!(
        far.contains("| Row | XFD |\n| --- | --- |\n| 1048576 | 2 |"),
        "{far}"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn blank_text_cells_do_not_count_as_rows_or_columns() {
    let dir = temp_dir("xlsx-blank");
    let rows = [
        row(1, &["Item", "Qty"]),
        // Cleared cells that kept an empty or blank string.
        row(2, &["", " ", "", ""]),
        row(3, &["Bolt", "4", ""]),
    ]
    .concat();
    let path = write(&dir, "blank.xlsx", &xlsx(&[("Balance", rows)]));
    let out = read_document(&path, None, None).expect("reads");
    assert!(out.contains("(3 rows x 2 columns)"), "{out}");
    assert!(
        out.contains("| Row | A | B |\n| --- | --- | --- |\n| 1 | Item | Qty |\n| 3 | Bolt | 4 |"),
        "{out}"
    );
    assert!(!out.contains("| 2 |"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn sheet_names_that_look_like_ranges_select_their_sheet() {
    let dir = temp_dir("xlsx-names");
    let path = write(
        &dir,
        "years.xlsx",
        &xlsx(&[
            ("Summary", row(1, &["all"])),
            ("2026-27", row(1, &["this year"])),
            ("2024", row(1, &["old"])),
        ]),
    );
    let fiscal = read_document(&path, Some("2026-27"), None).expect("fiscal year sheet");
    assert!(fiscal.contains("## Sheet 2: 2026-27"), "{fiscal}");
    assert!(fiscal.contains("this year"), "{fiscal}");
    let year = read_document(&path, Some("2024"), None).expect("year sheet");
    assert!(year.contains("## Sheet 3: 2024"), "{year}");
    assert!(!year.contains("this year"), "{year}");
    // Plain numbers that match no sheet name are still positions.
    let first = read_document(&path, Some("1"), None).expect("by number");
    assert!(first.contains("## Sheet 1: Summary"), "{first}");
    std::fs::remove_dir_all(dir).ok();
}

fn ods(content_body: &str) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet>{content_body}</office:spreadsheet></office:body></office:document-content>"#
    );
    package(&[
        (
            "mimetype",
            "application/vnd.oasis.opendocument.spreadsheet".to_string(),
        ),
        ("content.xml", content),
        (
            "META-INF/manifest.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#
                .to_string(),
        ),
    ])
}

#[test]
fn ods_files_whose_repeats_expand_to_huge_grids_are_refused() {
    let dir = temp_dir("ods");
    let huge = ods(
        r#"<table:table table:name="Big"><table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="100" office:value-type="float" office:value="1"/></table:table-row></table:table>"#,
    );
    let path = write(&dir, "big.ods", &huge);
    let error = read_document(&path, None, None).expect_err("refused");
    assert!(error.contains("more than 8000000 cells"), "{error}");

    // LibreOffice pads every sheet with a million empty repeated rows; those
    // are never expanded and must not be refused.
    let normal = ods(
        r#"<table:table table:name="Stock"><table:table-row><table:table-cell office:value-type="string"><text:p>Bolt</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="4"><text:p>4</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1022"/></table:table-row><table:table-row table:number-rows-repeated="1048575"><table:table-cell table:number-columns-repeated="1024"/></table:table-row></table:table>"#,
    );
    let path = write(&dir, "stock.ods", &normal);
    let out = read_document(&path, None, None).expect("reads");
    assert!(out.contains("| 1 | Bolt | 4 |"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

/// A compound file (512-byte sectors) holding one stream: what `.xls`
/// files are.
fn compound_file(name: &str, stream: &[u8]) -> Vec<u8> {
    const FREE: u32 = 0xFFFF_FFFF;
    const END: u32 = 0xFFFF_FFFE;
    let mut stream = stream.to_vec();
    if stream.len() < 4096 {
        // Pad past the mini stream cutoff with an ignored record.
        let pad = 4096 - stream.len();
        stream.extend_from_slice(&[0x55, 0x00]);
        stream.extend_from_slice(&u16::try_from(pad - 4).expect("pad").to_le_bytes());
        stream.resize(4096, 0);
    }
    let sectors = stream.len().div_ceil(512);
    assert!(sectors + 2 <= 128, "fixture too large");
    let mut fat = vec![FREE; 128];
    fat[0] = 0xFFFF_FFFD;
    fat[1] = END;
    for index in 0..sectors {
        fat[2 + index] = if index + 1 < sectors {
            u32::try_from(3 + index).expect("sector")
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
    header[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    header[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    header[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
    header[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    header[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for slot in 0..109 {
        let id = if slot == 0 { 0 } else { FREE };
        header[0x4C + slot * 4..0x50 + slot * 4].copy_from_slice(&id.to_le_bytes());
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
    let mut directory = entry("Root Entry", 5, 1, END, 0);
    directory.extend(entry(name, 2, FREE, 2, stream.len()));
    directory.resize(512, 0);
    let mut file = header;
    file.extend(fat.iter().flat_map(|id| id.to_le_bytes()));
    file.extend(directory);
    file.extend(&stream);
    file.resize(512 * (sectors + 3), 0);
    file
}

fn biff(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut record = kind.to_le_bytes().to_vec();
    record.extend_from_slice(&u16::try_from(data.len()).expect("record").to_le_bytes());
    record.extend_from_slice(data);
    record
}

/// A BIFF8 workbook stream with one sheet holding `sheet_records`.
fn workbook_stream(sheet_records: &[Vec<u8>]) -> Vec<u8> {
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
    let globals_length = bof(0x0005).len() + bound_sheet(0).len() + biff(0x000A, &[]).len();
    let mut stream = bof(0x0005);
    stream.extend(bound_sheet(u32::try_from(globals_length).expect("offset")));
    stream.extend(biff(0x000A, &[]));
    stream.extend(bof(0x0010));
    for record in sheet_records {
        stream.extend_from_slice(record);
    }
    stream.extend(biff(0x000A, &[]));
    stream
}

fn number_record(row: u16, column: u16, value: f64) -> Vec<u8> {
    let mut data = row.to_le_bytes().to_vec();
    data.extend_from_slice(&column.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&value.to_le_bytes());
    biff(0x0203, &data)
}

#[test]
fn xls_workbooks_are_measured_before_calamine_lays_them_out() {
    let dir = temp_dir("xls");
    // Two cells at opposite corners: a 16.7 million cell grid.
    let spread = compound_file(
        "Workbook",
        &workbook_stream(&[number_record(0, 0, 1.0), number_record(65_535, 255, 2.0)]),
    );
    let path = write(&dir, "spread.xls", &spread);
    let error = read_document(&path, None, None).expect_err("refused");
    assert!(error.contains("more than 8000000 cells"), "{error}");

    // A size record whose last row comes before its first would wrap
    // calamine's arithmetic into a huge reservation.
    let mut dimensions = 10u32.to_le_bytes().to_vec();
    dimensions.extend_from_slice(&2u32.to_le_bytes());
    dimensions.extend_from_slice(&0u16.to_le_bytes());
    dimensions.extend_from_slice(&3u16.to_le_bytes());
    dimensions.extend_from_slice(&0u16.to_le_bytes());
    let wrapped = compound_file(
        "Workbook",
        &workbook_stream(&[biff(0x0200, &dimensions), number_record(0, 0, 1.0)]),
    );
    let path = write(&dir, "wrapped.xls", &wrapped);
    let error = read_document(&path, None, None).expect_err("refused");
    assert!(error.contains("damaged"), "{error}");

    // A small sheet passes the check and reads.
    let small = compound_file(
        "Workbook",
        &workbook_stream(&[number_record(0, 0, 42.0), number_record(1, 1, 7.0)]),
    );
    let path = write(&dir, "small.xls", &small);
    let out = read_document(&path, None, None).expect("reads");
    assert!(out.contains("| 1 | 42 |  |"), "{out}");
    assert!(out.contains("| 2 |  | 7 |"), "{out}");

    // A compound file without a workbook (an encrypted .xlsx looks like this).
    let other = compound_file("EncryptedPackage", b"secret");
    let path = write(&dir, "locked.xlsx", &other);
    let error = read_document(&path, None, None).expect_err("refused");
    assert!(error.contains("password protected"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn deeply_nested_pdf_objects_end_in_a_result_not_a_stack_overflow() {
    for opener in ["[", "<<"] {
        let nested = opener.repeat(200_000);
        let mut top_level = b"%PDF-1.7\n1 0 obj\n".to_vec();
        top_level.extend_from_slice(nested.as_bytes());
        top_level.extend_from_slice(b"\nendobj\n");
        let _ = read_pdf(&top_level);

        // The same inside a (compressed) content stream.
        let content = format!("BT /F1 12 Tf 72 700 Td (Before) Tj {nested} ET");
        let document = read_pdf(&fixtures::one_page(content.as_bytes(), "", Vec::new()))
            .expect("reads the page");
        assert!(
            document.units[0].markdown.contains("Before"),
            "{document:?}"
        );
    }
}

#[test]
fn a_form_that_draws_itself_or_fans_out_ends_quickly() {
    // Form 6 draws itself 50 times.
    let looping = fixtures::stream(
        "/Type /XObject /Subtype /Form /Resources << /XObject << /X 6 0 R >> /Font << /F1 5 0 R >> >>",
        format!("BT /F1 12 Tf 72 700 Td (Loop) Tj ET {}", "/X Do ".repeat(50)).as_bytes(),
        false,
    );
    let document =
        read_pdf(&fixtures::one_page(b"/X Do", "/X 6 0 R", vec![looping])).expect("reads");
    assert_eq!(
        document.units[0].markdown.matches("Loop").count(),
        1,
        "{document:?}"
    );

    // Six forms, each drawing the next 50 times: 50^5 leaf draws if nothing
    // stopped it. The page text limit and the decode budget end it.
    let mut forms = Vec::new();
    for level in 0..6 {
        let number = 6 + level;
        let content = if level == 5 {
            b"BT /F1 12 Tf 72 700 Td (x) Tj ET".to_vec()
        } else {
            "/N Do ".repeat(50).into_bytes()
        };
        forms.push(fixtures::stream(
            &format!(
                "/Type /XObject /Subtype /Form /Resources << /XObject << /N {} 0 R >> /Font << /F1 5 0 R >> >>",
                number + 1
            ),
            &content,
            false,
        ));
    }
    let document = read_pdf(&fixtures::one_page(b"/N Do", "/N 6 0 R", forms)).expect("reads");
    assert!(document.units[0].markdown.contains('x'));
    assert!(
        document
            .notes
            .iter()
            .any(|note| note.contains("incomplete") || note.contains("stopped after")),
        "{:?}",
        document.notes
    );
}

#[test]
fn font_maps_keep_ranges_as_ranges() {
    // 2,000 four-byte ranges of 65,536 codes each: 131 million entries if
    // every code were stored.
    let mut cmap = String::from(
        "begincmap\n1 begincodespacerange\n<00000000> <FFFFFFFF>\nendcodespacerange\n100 beginbfrange\n",
    );
    for high in 0..2_000u32 {
        let _ = writeln!(cmap, "<{high:04X}0000> <{high:04X}FFFF> <0041>");
    }
    cmap.push_str("endbfrange\nendcmap\n");
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F9 6 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", b"BT /F9 12 Tf 72 700 Td <00070002> Tj ET", false),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Big /Encoding /Identity-H /ToUnicode 7 0 R /DescendantFonts [8 0 R] >>".to_vec(),
        fixtures::stream("", cmap.as_bytes(), true),
        b"<< /Type /Font /Subtype /CIDFontType2 /W [0 4000000000 500 1 [600 700]] >>".to_vec(),
    ];
    let document = read_pdf(&fixtures::assemble(&objects, "")).expect("reads");
    // Code 0x00070002 is two past the start of the range at 0x00070000.
    assert!(
        document.units[0].markdown.contains("## Page 1\n\nC"),
        "{document:?}"
    );
}

#[test]
fn repeated_and_overlapping_references_are_read_once() {
    // One stream listed 100,000 times in a page's contents.
    let references = "4 0 R ".repeat(100_000);
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /Contents [{references}] /Resources << /Font << /F1 5 0 R >> >> >>"
        )
        .into_bytes(),
        fixtures::stream("", b"BT /F1 12 Tf 72 700 Td (Once) Tj ET", true),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let document = read_pdf(&fixtures::assemble(&objects, "")).expect("reads");
    assert_eq!(document.units[0].markdown.matches("Once").count(), 1);

    // An object stream whose header points 2,000 objects at one offset:
    // only the first is read, the rest would parse the same bytes again.
    let pages = "<< /Type /Pages /Kids [6 0 R] /Count 1 /Resources << /Font << /F1 3 0 R >> >> >>";
    let mut header = "7 0 ".to_string();
    for number in 20..2_020 {
        let _ = write!(header, "{number} 0 ");
    }
    let packed = format!("{header}{pages}");
    let objects = vec![
        b"<< /Type /Catalog /Pages 7 0 R >>".to_vec(),
        fixtures::stream(
            &format!("/Type /ObjStm /N 2001 /First {}", header.len()),
            packed.as_bytes(),
            true,
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        fixtures::stream("", b"BT /F1 12 Tf 72 720 Td (Packed page) Tj ET", true),
        b"<< >>".to_vec(),
        b"<< /Type /Page /Parent 7 0 R /Contents 4 0 R >>".to_vec(),
    ];
    let document = read_pdf(&fixtures::assemble(&objects, "")).expect("reads");
    assert!(
        document.units[0].markdown.contains("Packed page"),
        "{document:?}"
    );
}

#[test]
fn a_huge_stream_length_and_streams_without_an_end_do_not_panic_or_stall() {
    let overflow =
        b"%PDF-1.7\n1 0 obj <</Length 99999999999999999999>> stream\nendstream endobj\n".to_vec();
    let _ = read_pdf(&overflow);
    let near_max = b"%PDF-1.7\n1 0 obj <</Length 18446744073709551615>> stream\nendstream endobj\n";
    let _ = read_pdf(near_max);

    // 100,000 streams with no endstream anywhere: each used to search the
    // rest of the file.
    let mut endless = b"%PDF-1.7\n".to_vec();
    for _ in 0..100_000 {
        endless.extend_from_slice(b"1 0 obj<<>>stream\n");
    }
    let _ = read_pdf(&endless);
}

#[test]
fn table_cells_placed_with_cm_keep_their_spaces_and_rows() {
    // How ReportLab and others draw tables: each cell moved into place
    // with cm, then drawn at the same text position.
    let content = b"q 1 0 0 1 72 700 cm BT /F1 12 Tf 1 0 0 1 0 2.9 Tm (Symbol) Tj ET Q \
q 1 0 0 1 200 700 cm BT /F1 12 Tf 1 0 0 1 0 2.9 Tm (Value) Tj ET Q \
q 1 0 0 1 72 680 cm BT /F1 12 Tf 1 0 0 1 0 2.9 Tm (r) Tj ET Q \
q 1 0 0 1 200 680 cm BT /F1 12 Tf 1 0 0 1 0 2.9 Tm (4) Tj ET Q";
    let document = read_pdf(&fixtures::one_page(content, "", Vec::new())).expect("reads");
    assert!(
        document.units[0].markdown.ends_with("Symbol Value\nr 4"),
        "{:?}",
        document.units[0].markdown
    );

    // A form's own matrix moves its text too.
    let form = fixtures::stream(
        "/Type /XObject /Subtype /Form /Matrix [1 0 0 1 300 0] /Resources << /Font << /F1 5 0 R >> >>",
        b"BT /F1 12 Tf 0 0 Td (Right) Tj ET",
        false,
    );
    let content =
        b"q 1 0 0 1 72 700 cm BT /F1 12 Tf 0 0 Td (Left) Tj ET Q q 1 0 0 1 0 650 cm /X Do Q";
    let document = read_pdf(&fixtures::one_page(content, "/X 6 0 R", vec![form])).expect("reads");
    assert!(
        document.units[0].markdown.contains("Left\nRight"),
        "{:?}",
        document.units[0].markdown
    );
}

#[test]
fn numbered_headings_stay_headings_with_their_numbers() {
    let dir = temp_dir("docx-numbered");
    let heading = |style: &str, level: u32, text: &str| {
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/><w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"3\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
        )
    };
    let listed = |text: &str| {
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
        )
    };
    let body = [
        heading("berschrift1", 0, "PROPOSED SYSTEM"),
        heading("Heading2", 1, "MODULES"),
        heading("Heading3", 2, "HUMAN MOVEMENT DETECTION"),
        heading("Heading3", 2, "TECHNICAL FEASIBILITY"),
        heading("Heading2", 1, "RESULTS"),
        listed("A numbered step"),
        listed("The next step"),
    ]
    .concat();
    let path = write(&dir, "report.docx", &super::tests::docx(&body));
    let out = read_document(&path, None, None).expect("reads");
    for expected in [
        "# 4 PROPOSED SYSTEM",
        "## 4.1 MODULES",
        "### 4.1.1 HUMAN MOVEMENT DETECTION",
        "### 4.1.2 TECHNICAL FEASIBILITY",
        "## 4.2 RESULTS",
        "1. A numbered step",
        "2. The next step",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in\n{out}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn section_breaks_start_pages_and_word_page_count_is_noted() {
    let dir = temp_dir("docx-sections");
    let paragraph = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let section_end = |text: &str, kind: Option<&str>| {
        let kind = kind.map_or_else(String::new, |kind| format!("<w:type w:val=\"{kind}\"/>"));
        format!(
            "<w:p><w:pPr><w:sectPr>{kind}<w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
        )
    };
    let rendered =
        |text: &str| format!("<w:p><w:r><w:lastRenderedPageBreak/><w:t>{text}</w:t></w:r></w:p>");
    let body = [
        paragraph("Page one"),
        section_end("End of section one", Some("nextPage")),
        // Word usually marks the new page too; that is the same page.
        "<w:p/>".to_string(),
        rendered("Page two"),
        section_end("End of section two", None),
        // ...but not always.
        paragraph("Page three"),
        section_end("Continuous section", Some("continuous")),
        paragraph("Still page three"),
        rendered("Page four"),
        section_end("Last paragraph ends a section", Some("nextPage")),
    ]
    .concat();
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let app = r#"<?xml version="1.0" encoding="UTF-8"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Pages>5</Pages></Properties>"#;
    let path = write(
        &dir,
        "sections.docx",
        &package(&[
            ("word/document.xml", document),
            ("docProps/app.xml", app.to_string()),
        ]),
    );
    let out = read_document(&path, None, None).expect("reads");
    assert!(out.contains("Word document, 4 pages"), "{out}");
    assert!(out.contains("Word, which last counted 5 pages"), "{out}");
    for (page, text) in [
        (1, "Page one"),
        (2, "Page two"),
        (3, "Page three"),
        (4, "Page four"),
    ] {
        let at = out.find(&format!("[page {page}]")).expect("page marker");
        let next = out[at..]
            .find(&format!("[page {}]", page + 1))
            .map_or(out.len(), |offset| at + offset);
        assert!(
            out[at..next].contains(text),
            "{text} not on page {page}:\n{out}"
        );
    }
    assert!(out.contains("Still page three"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn many_pages_render_in_linear_time_and_stop_building_past_the_budget() {
    let dir = temp_dir("docx-pages");
    let body = (0..50_000)
        .map(|index| format!("<w:p><w:r><w:lastRenderedPageBreak/><w:t>p{index}</w:t></w:r></w:p>"))
        .collect::<String>();
    let path = write(&dir, "pages.docx", &super::tests::docx(&body));
    let out = read_document(&path, None, None).expect("reads");
    assert!(out.contains("Word document, 50001 pages"), "{out:.300}");
    let hint = out.lines().last().expect("hint");
    assert!(
        hint.starts_with("[truncated: at least ") && hint.ends_with("-50001\"]"),
        "{hint}"
    );
    assert!(out.chars().count() < DEFAULT_MAX_CHARS + 500);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn packages_have_a_total_inflate_budget() {
    let dir = temp_dir("package-budget");
    let path = write(&dir, "plan.docx", &sample_docx());
    let mut package = super::xml::Package::open(&path, "Word document")
        .expect("opens")
        .with_budget(100);
    let error = package
        .read_part("word/document.xml")
        .expect_err("over the budget");
    assert!(error.contains("unpacks to more than"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn rows_and_columns_apply_only_to_spreadsheets() {
    let dir = temp_dir("rows-elsewhere");
    let path = write(&dir, "plan.docx", &sample_docx());
    let error = super::read_document(
        &path,
        &Options {
            rows: Some("2-3"),
            ..Options::default()
        },
    )
    .expect_err("not a spreadsheet");
    assert!(error.contains("only apply to spreadsheets"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}
