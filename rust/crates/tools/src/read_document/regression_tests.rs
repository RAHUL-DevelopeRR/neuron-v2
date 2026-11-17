//! Regressions from the second hostile-file review: each test is one file
//! shape that used to abort, hang or misread.
#![allow(clippy::format_collect)]

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use super::pdf::fixtures;
use super::tests::{pptx, rels, row, slide, temp_dir, write, xlsx, W};
use super::{Budget, Options, Selection, DEFAULT_MAX_CHARS};

fn read(path: &Path, pages: Option<&str>, max_chars: Option<usize>) -> Result<String, String> {
    super::read_document(
        path,
        &Options {
            pages,
            max_chars,
            ..Options::default()
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

/// A one-sheet `.xlsx` with `extra` parts, its sheet at `sheet_target`
/// (relative to `xl/`).
fn workbook_with(sheet_target: &str, sheet_xml: String, extra: Vec<(&str, String)>) -> Vec<u8> {
    package(&workbook_parts(sheet_target, sheet_xml, extra))
}

fn workbook_parts(
    sheet_target: &str,
    sheet_xml: String,
    extra: Vec<(&str, String)>,
) -> Vec<(String, String)> {
    let workbook = r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let mut parts = vec![
        (
            "_rels/.rels".to_string(),
            rels(&[("rId1", "officeDocument", "xl/workbook.xml")]),
        ),
        ("xl/workbook.xml".to_string(), workbook.to_string()),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            rels(&[("rId1", "worksheet", sheet_target)]),
        ),
        (format!("xl/{sheet_target}"), sheet_xml),
    ];
    parts.extend(extra.into_iter().map(|(name, xml)| (name.to_string(), xml)));
    parts
}

fn package(parts: &[(String, String)]) -> Vec<u8> {
    let borrowed = parts
        .iter()
        .map(|(name, xml)| (name.as_str(), xml.clone()))
        .collect::<Vec<_>>();
    super::tests::package(&borrowed)
}

fn sheet_xml(rows: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
    )
}

#[test]
fn a_shared_string_table_claiming_billions_of_strings_is_refused() {
    let dir = temp_dir("sst-count");
    for sst in [
        r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" uniqueCount="100000000000"/>"#,
        r#"<?xml version="1.0"?><!-- padding --><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="5" uniqueCount="+900000000000000"><si><t>a</t></si></sst>"#,
    ] {
        let bytes = workbook_with(
            "worksheets/sheet1.xml",
            sheet_xml(&row(1, &["1"])),
            vec![("xl/sharedStrings.xml", sst.to_string())],
        );
        let path = write(&dir, "strings.xlsx", &bytes);
        let error = read(&path, None, None).expect_err("refused before calamine reserves");
        assert!(
            error.contains("claims more strings than it holds"),
            "{error}"
        );
    }
    // An honest table reads.
    let honest = workbook_with(
        "worksheets/sheet1.xml",
        sheet_xml(r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#),
        vec![(
            "xl/sharedStrings.xml",
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>hello</t></si></sst>"#
                .to_string(),
        )],
    );
    let path = write(&dir, "honest.xlsx", &honest);
    let out = read(&path, None, None).expect("reads");
    assert!(out.contains("| 1 | hello |"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_workbook_repeating_a_few_strings_in_many_cells_reads() {
    // `count` is every string cell in the workbook, `uniqueCount` the table's
    // entries: a sheet of status columns has millions of one and fifty of the
    // other, in a table of a few hundred bytes.
    let dir = temp_dir("sst-repeated");
    let strings = (0..50)
        .map(|index| format!("<si><t>S{index}</t></si>"))
        .collect::<String>();
    let table = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2000000" uniqueCount="50">{strings}</sst>"#
    );
    let rows = (1..=20)
        .map(|row| {
            format!(
                r#"<row r="{row}"><c r="A{row}" t="s"><v>{}</v></c><c r="B{row}" t="s"><v>{}</v></c></row>"#,
                row % 50,
                (row * 7) % 50
            )
        })
        .collect::<String>();
    let bytes = workbook_with(
        "worksheets/sheet1.xml",
        sheet_xml(&rows),
        vec![("xl/sharedStrings.xml", table)],
    );
    let path = write(&dir, "status.xlsx", &bytes);
    let out = read(&path, None, None).expect("an ordinary workbook reads");
    assert!(out.contains("| 1 | S1 | S7 |"), "{out}");
    assert!(out.contains("| 20 | S20 | S40 |"), "{out}");
    // A writer that puts the cell count in uniqueCount as well still reads.
    let sloppy = workbook_with(
        "worksheets/sheet1.xml",
        sheet_xml(r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#),
        vec![(
            "xl/sharedStrings.xml",
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1500000" uniqueCount="1500000"><si><t>hello</t></si></sst>"#
                .to_string(),
        )],
    );
    let path = write(&dir, "sloppy.xlsx", &sloppy);
    let out = read(&path, None, None).expect("a modest wrong uniqueCount still reads");
    assert!(out.contains("| 1 | hello |"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn every_zip_entry_is_measured_whatever_its_extension() {
    let dir = temp_dir("zip-dat");
    // The sheet lives at a path with no XML extension, and unpacks past the
    // limit for parts outside worksheets/.
    let mut huge = String::with_capacity(70 * 1024 * 1024);
    huge.push_str(r#"<worksheet><sheetData><row r="1"><c r="A1"><v>"#);
    huge.push_str(&"1".repeat(65 * 1024 * 1024));
    huge.push_str("</v></c></row></sheetData></worksheet>");
    // Compressed fast: a debug build deflates 65 MB slowly at the default
    // level.
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(1));
    for (name, contents) in workbook_parts("data/sheet.dat", huge, Vec::new()) {
        writer.start_file(name, options).expect("start part");
        std::io::Write::write_all(&mut writer, contents.as_bytes()).expect("write part");
    }
    let bytes = writer.finish().expect("finish zip").into_inner();
    let path = write(&dir, "dat.xlsx", &bytes);
    let error = read(&path, None, None).expect_err("measured and refused");
    assert!(error.contains("too large once unzipped"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

/// A compound file (512-byte sectors) whose directory holds `entries` in
/// order after the root: name, entry type, the name-length field, stream.
fn compound(entries: &[(&str, u8, u16, Vec<u8>)]) -> Vec<u8> {
    const FREE: u32 = 0xFFFF_FFFF;
    const END: u32 = 0xFFFF_FFFE;
    let mut fat = vec![FREE; 128];
    fat[0] = 0xFFFF_FFFD;
    fat[1] = END;
    let mut next_sector = 2usize;
    let mut placed = Vec::new();
    let mut body = Vec::new();
    for (_, _, _, stream) in entries {
        let mut stream = stream.clone();
        if stream.len() < 4096 {
            let pad = 4096 - stream.len();
            stream.extend_from_slice(&[0x55, 0x00]);
            stream.extend_from_slice(&u16::try_from(pad - 4).expect("pad").to_le_bytes());
            stream.resize(4096, 0);
        }
        let sectors = stream.len().div_ceil(512);
        let start = next_sector;
        for index in 0..sectors {
            fat[start + index] = if index + 1 < sectors {
                u32::try_from(start + index + 1).expect("sector")
            } else {
                END
            };
        }
        next_sector += sectors;
        assert!(next_sector <= 128, "fixture too large");
        placed.push((start, stream.len()));
        stream.resize(sectors * 512, 0);
        body.extend(stream);
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
    let entry = |name: &str, kind: u8, length: u16, start: u32, size: usize| {
        let mut entry = vec![0u8; 128];
        for (index, unit) in name.encode_utf16().enumerate() {
            entry[index * 2..index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entry[0x40..0x42].copy_from_slice(&length.to_le_bytes());
        entry[0x42] = kind;
        entry[0x43] = 1;
        entry[0x44..0x48].copy_from_slice(&FREE.to_le_bytes());
        entry[0x48..0x4C].copy_from_slice(&FREE.to_le_bytes());
        entry[0x4C..0x50].copy_from_slice(&FREE.to_le_bytes());
        entry[0x74..0x78].copy_from_slice(&start.to_le_bytes());
        entry[0x78..0x7C].copy_from_slice(&u32::try_from(size).expect("size").to_le_bytes());
        entry
    };
    let mut directory = entry("Root Entry", 5, 22, END, 0);
    for ((name, kind, length, _), (start, size)) in entries.iter().zip(&placed) {
        directory.extend(entry(
            name,
            *kind,
            *length,
            u32::try_from(*start).expect("start"),
            *size,
        ));
    }
    assert!(directory.len() <= 512, "one directory sector");
    directory.resize(512, 0);
    let mut file = header;
    file.extend(fat.iter().flat_map(|id| id.to_le_bytes()));
    file.extend(directory);
    file.extend(body);
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
    let bound_sheet = |offset: u32| {
        let mut data = offset.to_le_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 4, 0]);
        data.extend_from_slice(b"Data");
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
fn a_string_record_before_any_formula_counts_as_a1() {
    let dir = temp_dir("xls-string");
    // One far cell, then a STRING record with no formula before it: calamine
    // puts its cell at A1, spanning 65536 x 256 cells.
    let stream = workbook_stream(&[number_record(65_535, 255, 1.0), biff(0x0207, &[0, 0])]);
    let path = write(
        &dir,
        "string.xls",
        &compound(&[("Workbook", 2, 18, stream)]),
    );
    let error = read(&path, None, None).expect_err("refused");
    assert!(error.contains("more than 8000000 cells"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_workbook_stream_is_found_the_way_calamine_finds_it() {
    let dir = temp_dir("xls-cfb");
    let hostile = workbook_stream(&[number_record(0, 0, 1.0), number_record(65_535, 255, 2.0)]);
    let benign = workbook_stream(&[number_record(0, 0, 42.0)]);
    // calamine takes the first entry named Workbook whatever its type and
    // name-length field; a second, proper entry must not be the one checked.
    for (kind, length) in [(0u8, 18u16), (1, 18), (2, 0)] {
        let file = compound(&[
            ("Workbook", kind, length, hostile.clone()),
            ("Workbook", 2, 18, benign.clone()),
        ]);
        let path = write(&dir, "twin.xls", &file);
        let error = read(&path, None, None).expect_err("the stream calamine reads is checked");
        assert!(error.contains("more than 8000000 cells"), "{error}");
    }
    // A Book stream next to the Workbook stream is checked too: calamine
    // falls back to it when Workbook fails.
    let file = compound(&[
        ("Workbook", 2, 18, benign.clone()),
        ("Book", 2, 10, hostile),
    ]);
    let path = write(&dir, "book.xls", &file);
    assert!(read(&path, None, None).is_err());
    // The proper single stream still reads.
    let path = write(&dir, "fine.xls", &compound(&[("Workbook", 2, 18, benign)]));
    let out = read(&path, None, None).expect("reads");
    assert!(out.contains("| 1 | 42 |"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

fn ods(content_body: &str) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet>{content_body}</office:spreadsheet></office:body></office:document-content>"#
    );
    super::tests::package(&[
        (
            "mimetype",
            "application/vnd.oasis.opendocument.spreadsheet".to_string(),
        ),
        ("content.xml", content),
    ])
}

#[test]
fn ods_cells_with_a_value_but_no_value_type_count_as_filled() {
    let dir = temp_dir("ods-value");
    for cell in [
        r#"<table:table-cell office:value="1" table:number-columns-repeated="16384"/>"#,
        r#"<table:table-cell office:string-value="x" table:number-columns-repeated="16384"/>"#,
        r#"<table:table-cell office:boolean-value="true" table:number-columns-repeated="16384"/>"#,
    ] {
        let table = format!(
            r#"<table:table table:name="S"><table:table-row table:number-rows-repeated="6100">{cell}</table:table-row></table:table>"#
        );
        let path = write(&dir, "value.ods", &ods(&table));
        let error = read(&path, None, None).expect_err("refused");
        assert!(error.contains("more than 8000000 cells"), "{error}");
    }
    // One cell whose text is a hundred billion spaces.
    let spaces = r#"<table:table table:name="S"><table:table-row><table:table-cell office:value-type="string"><text:p><text:s text:c="100000000000"/></text:p></table:table-cell></table:table-row></table:table>"#;
    let path = write(&dir, "spaces.ods", &ods(spaces));
    let error = read(&path, None, None).expect_err("refused");
    assert!(error.contains("more than 8000000 cells or"), "{error}");
    // A long text repeated across a wide row.
    let long = format!(
        r#"<table:table table:name="S"><table:table-row table:number-rows-repeated="100"><table:table-cell office:value-type="string" office:string-value="{}" table:number-columns-repeated="10000"/></table:table-row></table:table>"#,
        "y".repeat(10_000)
    );
    let path = write(&dir, "long.ods", &ods(&long));
    assert!(read(&path, None, None).is_err());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_page_tree_whose_nodes_share_their_kids_is_walked_once() {
    // Each level is an array of two inline nodes that both point at the next
    // array: 2^31 paths to the leaves if nodes were visited per path.
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids 3 0 R >>".to_vec(),
    ];
    for number in 3..33 {
        objects.push(
            format!(
                "[<< /Kids {next} 0 R >> << /Kids {next} 0 R >>]",
                next = number + 1
            )
            .into_bytes(),
        );
    }
    objects.push(b"[<< /Type /Page >> << /Type /Page >>]".to_vec());
    let started = Instant::now();
    let document = read_pdf(&fixtures::assemble(&objects, "")).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(document.total <= 4, "{}", document.total);
}

#[test]
fn a_font_map_with_huge_targets_cannot_multiply_text() {
    // One code maps to 200,000 characters; the page shows it 20,000 times.
    let target = "0041".repeat(200_000);
    let cmap = format!(
        "begincmap\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfchar\n<41> <{target}>\nendbfchar\n1 beginbfrange\n<42> <43> <{target}>\nendbfrange\nendcmap\n"
    );
    let shown = format!(
        "BT /F9 12 Tf 72 700 Td ({}) Tj ({}) Tj ET",
        "A".repeat(20_000),
        "B".repeat(20_000)
    );
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F9 5 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", shown.as_bytes(), true),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".to_vec(),
        fixtures::stream("", cmap.as_bytes(), true),
    ];
    let started = Instant::now();
    let document = read_pdf(&fixtures::assemble(&objects, "")).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(20));
    let text = &document.units[0].markdown;
    assert!(text.len() <= 1_100_000, "{}", text.len());
    assert!(text.contains("AAAA"), "mapped text still shows");
}

#[test]
fn one_huge_operand_or_object_is_cut_while_it_is_built() {
    // 3 million empty arrays in one operand of a content stream.
    let mut content = b"BT /F1 12 Tf 72 700 Td (Kept) Tj ET [".to_vec();
    content.extend(std::iter::repeat_n(b"[]".as_slice(), 3_000_000).flatten());
    content.extend_from_slice(b"] TJ BT /F1 12 Tf 72 680 Td (After) Tj ET");
    let document = read_pdf(&fixtures::one_page(&content, "", Vec::new())).expect("reads the page");
    let text = &document.units[0].markdown;
    assert!(text.contains("Kept") && text.contains("After"), "{text}");

    // The same as a top-level object.
    let mut object = b"%PDF-1.7\n1 0 obj\n[".to_vec();
    object.extend(std::iter::repeat_n(b"0 ".as_slice(), 3_000_000).flatten());
    object.extend_from_slice(b"]\nendobj\n");
    let _ = read_pdf(&object);
}

#[test]
fn stream_lengths_pointing_into_long_whitespace_do_not_rescan_it() {
    // 20,000 stream headers whose /Length all end inside one 2 MB run of
    // spaces, with no endstream anywhere.
    let mut data = b"%PDF-1.7\n".to_vec();
    let headers = 20_000usize;
    let header_len = |index: usize| format!("{index} 0 obj<</Length 0000000000>>stream\n").len();
    let total_headers = (0..headers).map(header_len).sum::<usize>() + data.len();
    for index in 0..headers {
        let start = data.len() + header_len(index);
        let length = total_headers - start;
        data.extend_from_slice(format!("{index} 0 obj<</Length {length:010}>>stream\n").as_bytes());
    }
    assert_eq!(data.len(), total_headers);
    data.extend(std::iter::repeat_n(b' ', 2 * 1024 * 1024));
    let started = Instant::now();
    let _ = read_pdf(&data);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

fn docx_with(document_body: &str, numbering: Option<&str>) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{document_body}<w:sectPr/></w:body></w:document>"#
    );
    let mut parts = vec![("word/document.xml".to_string(), document)];
    if let Some(numbering) = numbering {
        parts.push((
            "word/numbering.xml".to_string(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W}>{numbering}</w:numbering>"#),
        ));
    }
    package(&parts)
}

#[test]
fn wide_and_tall_tables_of_empty_cells_stay_small() {
    let dir = temp_dir("wide-table");
    let wide_row = format!("<w:tr>{}</w:tr>", "<w:tc/>".repeat(100_000));
    let first = "<w:tr><w:tc><w:p><w:r><w:t>Head</w:t></w:r></w:p></w:tc></w:tr>";
    let body = format!(
        "<w:tbl>{first}{wide_row}{}</w:tbl><w:p><w:r><w:t>After</w:t></w:r></w:p>",
        "<w:tr/>".repeat(100_000)
    );
    let path = write(&dir, "wide.docx", &docx_with(&body, None));
    let started = Instant::now();
    let out = read(&path, None, None).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(out.contains("| Head |\n| --- |"), "{out}");
    assert!(out.contains("After"), "{out}");
    assert!(out.len() < 20_000, "{}", out.len());

    // The same table on a slide.
    let cells = format!(
        "<a:tr><a:tc><a:txBody><a:p><a:r><a:t>Head</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr>{}</a:tr>{}",
        "<a:tc/>".repeat(100_000),
        "<a:tr/>".repeat(100_000)
    );
    let frame = format!(
        "<p:graphicFrame><a:graphic><a:graphicData><a:tbl>{cells}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"
    );
    let path = write(&dir, "wide.pptx", &pptx(&[slide("", &frame)], None));
    let out = read(&path, None, None).expect("reads");
    assert!(out.contains("| Head |"), "{out}");
    assert!(out.len() < 20_000, "{}", out.len());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_huge_list_label_template_is_cut() {
    let dir = temp_dir("lvltext");
    let numbering = format!(
        r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="{}%1"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#,
        "A".repeat(1_000_000)
    );
    let paragraph = r#"<w:p><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#;
    let body = paragraph.repeat(20_000);
    let path = write(&dir, "labels.docx", &docx_with(&body, Some(&numbering)));
    let started = Instant::now();
    let out = read(&path, Some("1"), None).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(20));
    let first = out
        .lines()
        .find(|line| line.ends_with(" x"))
        .expect("a list item");
    assert!(first.len() <= 70, "{first}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn deeply_nested_tables_are_flattened_in_linear_time() {
    let dir = temp_dir("nested-tables");
    let depth = 20_000;
    let open = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>level text here</w:t></w:r></w:p>";
    let close = "</w:tc></w:tr></w:tbl>";
    let body = format!("{}{}", open.repeat(depth), close.repeat(depth));
    let path = write(&dir, "nested.docx", &docx_with(&body, None));
    let started = Instant::now();
    let out = read(&path, None, None).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(out.contains("level text here"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_table_over_several_pages_puts_its_rows_on_their_pages() {
    let dir = temp_dir("table-pages");
    let cell = |text: &str| format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>");
    let broken = |text: &str| {
        format!("<w:tc><w:p><w:r><w:lastRenderedPageBreak/><w:t>{text}</w:t></w:r></w:p></w:tc>")
    };
    let body = format!(
        "<w:p><w:r><w:t>Intro</w:t></w:r></w:p><w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl><w:p><w:r><w:t>Outro</w:t></w:r></w:p>",
        cell("Name"),
        cell("Qty"),
        cell("Bolt"),
        cell("4"),
        broken("Nut"),
        cell("9"),
        cell("Washer"),
        cell("2")
    );
    let path = write(&dir, "long-table.docx", &docx_with(&body, None));
    let second = read(&path, Some("2"), None).expect("page 2");
    assert!(second.contains("(table continued)"), "{second}");
    assert!(
        second.contains("| Name | Qty |\n| --- | --- |\n| Nut | 9 |\n| Washer | 2 |"),
        "{second}"
    );
    assert!(!second.contains("Bolt"), "{second}");
    let first = read(&path, Some("1"), None).expect("page 1");
    assert!(first.contains("| Bolt | 4 |"), "{first}");
    assert!(!first.contains("Nut"), "{first}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn slide_ids_are_matched_to_parts_in_linear_time() {
    let dir = temp_dir("pptx-ids");
    // 200,000 unmatched slide ids and 200,000 relationships that match none
    // of them.
    let ids = "<p:sldId id=\"256\" r:id=\"rIdA\"/>".repeat(200_000);
    let presentation = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst>{ids}<p:sldId id="257" r:id="rId10"/></p:sldIdLst></p:presentation>"#
    );
    let mut relationships = String::new();
    for _ in 0..200_000 {
        relationships.push_str(r#"<Relationship Id="rIdB" Type="x" Target="nothing.xml"/>"#);
    }
    let rels_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relationships}<Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#
    );
    let bytes = super::tests::package(&[
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", rels_xml),
        ("ppt/slides/slide1.xml", slide("", "")),
    ]);
    let path = write(&dir, "ids.pptx", &bytes);
    let started = Instant::now();
    let out = read(&path, None, None).expect("reads");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(out.contains("1 slide"), "{out}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn hints_name_a_sheet_by_name_when_its_number_is_another_sheets_name() {
    let dir = temp_dir("sheet-selectors");
    let mut long = String::new();
    for number in 1..=600 {
        long.push_str(&row(number, &[&format!("item {number}")]));
    }
    let path = write(
        &dir,
        "numbers.xlsx",
        &xlsx(&[
            ("Index", row(1, &["contents"])),
            ("1", long),
            ("2", row(1, &["other sheet"])),
        ]),
    );
    // The sheet named "1" is at position 2, and the sheet named "2" exists:
    // its hints must say pages="1", which selects it by name.
    let out = read(&path, Some("1"), Some(30_000)).expect("reads");
    assert!(out.contains("## Sheet 2: 1"), "{out}");
    assert!(
        out.contains("for more use pages=\"1\" rows=\"501-\""),
        "{out}"
    );
    let next = super::read_document(
        &path,
        &Options {
            pages: Some("1"),
            rows: Some("501-"),
            ..Options::default()
        },
    )
    .expect("follows the hint");
    assert!(next.contains("| 501 | item 501 |"), "{next}");
    assert!(!next.contains("other sheet"), "{next}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_row_cut_in_the_middle_is_resumed_not_skipped() {
    let dir = temp_dir("mid-row");
    let wide = "w".repeat(150);
    let cells = vec![wide.as_str(); 40];
    let mut rows = String::new();
    for number in 1..=10 {
        rows.push_str(&row(number, &cells));
    }
    let path = write(&dir, "wide.xlsx", &xlsx(&[("Wide", rows)]));
    for max_chars in (8_000..12_000).step_by(37) {
        let out = read(&path, None, Some(max_chars)).expect("reads");
        let Some(hint) = out.lines().last().filter(|line| line.contains("rows=\"")) else {
            continue;
        };
        let resume = hint
            .split("rows=\"")
            .nth(1)
            .and_then(|rest| rest.split('-').next())
            .and_then(|row| row.parse::<usize>().ok())
            .expect("resume row");
        // Every row before the resume row is shown whole.
        let last_whole = out
            .lines()
            .filter(|line| line.starts_with("| ") && line.trim_end().ends_with('|'))
            .filter(|line| line.matches(" | ").count() >= 40)
            .filter_map(|line| line[2..].split(' ').next()?.parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        assert_eq!(resume, last_whole + 1, "max_chars {max_chars}: {hint}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn many_sheets_are_listed_briefly() {
    let dir = temp_dir("many-sheets");
    let long_name = "n".repeat(200);
    let mut sheets = vec![(long_name.as_str(), row(1, &["first"]))];
    let names = (2..=120)
        .map(|number| format!("S{number}"))
        .collect::<Vec<_>>();
    for name in &names {
        sheets.push((name.as_str(), row(1, &["x"])));
    }
    let path = write(&dir, "many.xlsx", &xlsx(&sheets));
    let out = read(&path, Some("1"), Some(500)).expect("reads");
    let note = out
        .lines()
        .find(|line| line.starts_with("[note: sheets: "))
        .expect("sheet list");
    assert!(note.contains(", and 70 more]"), "{note}");
    assert!(note.len() < 1_500, "{}", note.len());
    let error = read(&path, Some("nope"), None).expect_err("no such sheet");
    assert!(error.contains(", and 70 more"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn text_saved_with_a_document_extension_points_at_read_file() {
    let dir = temp_dir("text-as-xls");
    for (name, text) in [
        (
            "statement.xls",
            "<html><table><tr><td>a</td></tr></table></html>",
        ),
        ("export.xls", "a\tb\n1\t2\n"),
        ("notes.pdf", "just some notes\n"),
    ] {
        let path = write(&dir, name, text.as_bytes());
        let error = read(&path, None, None).expect_err("not a document");
        assert!(error.contains("read it with read_file"), "{error}");
        assert!(error.contains(name), "{error}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn max_chars_stays_under_the_runtimes_tool_result_limit() {
    let dir = temp_dir("ceiling");
    let mut rows = String::new();
    for number in 1..=400 {
        let _ = write!(rows, "{}", row(number, &[&"z".repeat(200)]));
    }
    let path = write(&dir, "big.xlsx", &xlsx(&[("Big", rows)]));
    let out = read(&path, None, Some(100_000)).expect("reads");
    let body = out.split_once("\n\n").map_or("", |(_, body)| body);
    assert!(
        body.chars().count() <= super::MAX_MAX_CHARS + 200,
        "{}",
        body.chars().count()
    );
    assert!(super::max_chars_ceiling() <= 36_000);
    std::fs::remove_dir_all(dir).ok();
}

/// A Word table of `rows` (each a list of cell texts) inside a body.
fn word_table(rows: &[Vec<String>]) -> String {
    let rows = rows
        .iter()
        .map(|cells| {
            let cells = cells
                .iter()
                .map(|text| format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"))
                .collect::<String>();
            format!("<w:tr>{cells}</w:tr>")
        })
        .collect::<String>();
    format!("<w:tbl>{rows}</w:tbl>")
}

#[test]
fn a_huge_header_over_many_page_broken_rows_is_not_copied_per_page() {
    let dir = temp_dir("table-header-pages");
    // 64 header cells of 16 KB each, then 50,000 rows that each start a page
    // and hold some text. The header used to be copied and rendered once per
    // page, about 100 GB.
    let mut body = String::from("<w:tbl><w:tr>");
    for column in 0..64 {
        let _ = write!(
            body,
            "<w:tc><w:p><w:r><w:t>h{column}{}</w:t></w:r></w:p></w:tc>",
            "x".repeat(16_000)
        );
    }
    body.push_str("</w:tr>");
    for number in 0..50_000 {
        let _ = write!(
            body,
            r#"<w:tr><w:tc><w:p><w:r><w:br w:type="page"/><w:t>row{number}</w:t></w:r></w:p></w:tc></w:tr>"#
        );
    }
    body.push_str("</w:tbl>");
    let path = write(&dir, "pages.docx", &super::tests::docx(&body));
    let started = Instant::now();
    let first = read(&path, Some("1"), Some(36_000)).expect("reads");
    // The first page holds the header, its cells cut short, once.
    assert_eq!(first.matches("| h0").count(), 1, "{first:.500}");
    assert!(first.contains(" [...]"), "{first:.500}");
    assert!(!first.contains("(table continued)"));
    // A later page continues the table under a shorter copy of the header.
    let second = read(&path, Some("2"), Some(36_000)).expect("reads");
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    assert!(second.contains("(table continued)"), "{second:.500}");
    assert!(second.contains("| row0 |"), "{second:.500}");
    let repeated = second
        .lines()
        .find(|line| line.starts_with("| h0"))
        .expect("the header is repeated");
    assert!(repeated.chars().count() < 4_000, "{}", repeated.len());

    // Every part together stays within the table cap, repeated headers
    // included, and says that the rest is left out.
    let mut table = super::TableBuilder::default();
    table.start_row(1);
    for column in 0..64 {
        table.start_cell();
        table.push_line(&format!("h{column}{}", "x".repeat(16_000)));
    }
    for number in 0..50_000 {
        table.start_row(number + 2);
        table.start_cell();
        table.push_line(&format!("row{number}"));
    }
    let parts = table.into_parts();
    assert!(parts.len() > 100, "{}", parts.len());
    assert!(parts[1].render().starts_with("(table continued)"));
    let total = parts
        .iter()
        .map(super::TablePart::rendered_chars)
        .sum::<usize>();
    // The cap holds the header, the rows and the repeated headers; only the
    // closing note about the rows left out comes on top.
    let note = parts
        .last()
        .and_then(|part| part.note.as_ref())
        .map_or(0, |note| note.chars().count() + 2);
    assert!(note > 0 && note < 100, "{note}");
    assert!(total <= super::MAX_DOCUMENT_TABLE_CHARS + note, "{total}");
    assert_eq!(
        parts
            .iter()
            .map(|part| part.render().chars().count())
            .sum::<usize>(),
        total
    );
    let last = parts.last().expect("a part").render();
    assert!(
        last.ends_with("more rows of this table are longer than read_document returns]"),
        "{}",
        &last[last.len().saturating_sub(200)..]
    );
    std::fs::remove_dir_all(dir).ok();
}

/// Reads every page of `path` by following the hints, `max_chars` at a time,
/// and returns all the text; fails when a hint says something cannot be
/// reached.
fn read_all_pages(path: &Path, max_chars: usize) -> String {
    let mut all = String::new();
    let mut next = "1-".to_string();
    for _ in 0..1_000 {
        let out = read(path, Some(&next), Some(max_chars)).expect("reads");
        assert!(!out.contains("does not fit in one call"), "{out:.500}");
        assert!(!out.contains("longer than max_chars"), "{out:.500}");
        all.push_str(&out);
        let Some((_, hint)) = out.rsplit_once("use pages=\"") else {
            return all;
        };
        let start = hint
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .unwrap_or_default();
        next = format!("{start}-");
    }
    panic!("the hints never reached the end");
}

#[test]
fn a_long_table_after_a_saved_page_break_can_be_read_to_its_end() {
    let dir = temp_dir("table-after-break");
    let mut rows = vec![vec![
        "Code".to_string(),
        "Name".to_string(),
        "Region".to_string(),
        "Amount".to_string(),
    ]];
    for number in 1..=1_000 {
        rows.push(
            (0..4)
                .map(|column| format!("r{number:04}c{column}-{}", "q".repeat(52)))
                .collect(),
        );
    }
    // An explicit page break, as generated files write between sections,
    // and no break saved inside the table: the table is all of page 2.
    let body = format!(
        r#"<w:p><w:r><w:t>Intro</w:t><w:br w:type="page"/></w:r></w:p>{}"#,
        word_table(&rows)
    );
    let path = write(&dir, "sections.docx", &super::tests::docx(&body));
    let first = read(&path, Some("1"), Some(36_000)).expect("reads");
    assert!(first.contains("pages are estimated"), "{first:.500}");
    let all = read_all_pages(&path, 36_000);
    let seen = all
        .lines()
        .filter(|line| line.starts_with("| r"))
        .map(|line| line[3..7].parse::<usize>().expect("row number"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(seen.len(), 1_000, "every row is on some page");
    assert!(seen.contains(&1_000));

    // A short table after the same break keeps the saved page numbers.
    let body = format!(
        r#"<w:p><w:r><w:t>Intro</w:t><w:br w:type="page"/></w:r></w:p>{}"#,
        word_table(&rows[..20])
    );
    let path = write(&dir, "short.docx", &super::tests::docx(&body));
    let second = read(&path, Some("2"), Some(36_000)).expect("reads");
    assert!(
        second.contains("follow the page breaks saved"),
        "{second:.500}"
    );
    assert!(second.contains("| r0019c0-"), "{second:.500}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_long_header_and_a_long_first_row_each_fit_in_one_call() {
    let dir = temp_dir("table-long-start");
    let header = (0..6)
        .map(|column| format!("H{column}{}", "a".repeat(4_100)))
        .collect::<Vec<_>>();
    let first_row = (0..6)
        .map(|column| format!("R{column}{}", "b".repeat(4_100)))
        .collect::<Vec<_>>();
    let body = format!(
        "{}<w:p><w:r><w:t>After the table</w:t></w:r></w:p>",
        word_table(&[header, first_row])
    );
    let path = write(&dir, "long-start.docx", &super::tests::docx(&body));
    let all = read_all_pages(&path, 36_000);
    let row = all
        .lines()
        .find(|line| line.starts_with("| R0"))
        .expect("the first row is shown");
    assert!(row.contains("| R5"), "the whole row: {}", row.len());
    assert!(all.contains("After the table"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn table_cells_and_tables_are_capped_while_they_are_built() {
    let mut table = super::TableBuilder::default();
    table.start_row(1);
    table.start_cell();
    table.push_line("Header");
    for _ in 0..400 {
        table.start_row(1);
        table.start_cell();
        table.push_line(&"y".repeat(3_000));
        // Past the cell's cap: dropped.
        table.push_line(&"z".repeat(3_000));
        table.push_line("never");
    }
    let parts = table.into_parts();
    assert_eq!(parts.len(), 1);
    let markdown = parts[0].render();
    assert!(!markdown.contains("never"));
    let first_row = markdown.lines().nth(2).expect("a body row");
    assert!(first_row.ends_with(" [...] |"), "{first_row:.80}");
    assert!(first_row.chars().count() < super::MAX_CELL_CHARS + 20);
    // All parts together stay within the table cap, with a note.
    assert!(markdown.chars().count() <= 1_000_200, "{}", markdown.len());
    assert!(
        markdown.ends_with("more rows of this table are longer than read_document returns]"),
        "{}",
        &markdown[markdown.len() - 200..]
    );
}

#[test]
fn every_row_of_a_long_table_without_saved_page_breaks_can_be_read() {
    let dir = temp_dir("table-estimated");
    let mut rows = vec![vec![
        "Code".to_string(),
        "Name".to_string(),
        "Region".to_string(),
        "Amount".to_string(),
    ]];
    for number in 1..=1_500 {
        rows.push(
            (0..4)
                .map(|column| format!("r{number:04}c{column}-{}", "q".repeat(22)))
                .collect(),
        );
    }
    let body = format!(
        "<w:p><w:r><w:t>Ledger</w:t></w:r></w:p>{}",
        word_table(&rows)
    );
    let path = write(&dir, "ledger.docx", &super::tests::docx(&body));
    let first = read(&path, Some("1"), Some(36_000)).expect("reads");
    let total = first
        .split(", ")
        .find_map(|part| part.strip_suffix(" pages"))
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("a page count: {first:.300}"));
    assert!(total > 20, "the table spans estimated pages: {total}");
    let mut seen = std::collections::BTreeSet::new();
    for page in 1..=total {
        let out = read(&path, Some(&page.to_string()), Some(36_000)).expect("reads");
        assert!(!out.contains("does not fit in one call"), "page {page}");
        assert!(!out.contains("[truncated"), "page {page}: {out:.300}");
        if page > 1 {
            assert!(out.contains("(table continued)"), "page {page}");
            assert!(
                out.contains("| Code | Name | Region | Amount |"),
                "page {page}"
            );
        }
        for line in out.lines().filter(|line| line.starts_with("| r")) {
            seen.insert(line[3..7].parse::<usize>().expect("row number"));
        }
    }
    assert_eq!(seen.len(), 1_500, "every row is on some page");
    assert!(seen.contains(&1_500));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn utf16_and_legacy_text_named_as_documents_point_at_read_file() {
    let dir = temp_dir("utf16-text");
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "Name\tCity\r\nJos\u{e9}\tM\u{e1}laga\r\n".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    for name in ["unicode.xls", "unicode.pdf"] {
        let path = write(&dir, name, &utf16);
        let error = read(&path, None, None).expect_err("text, not a document");
        assert!(error.contains("read it with read_file"), "{error}");
        // And read_file reads it: the two tools agree.
        let text = runtime::read_file(path.to_string_lossy().as_ref(), None, None)
            .expect("read_file reads it")
            .file
            .content;
        assert!(text.contains("Jos\u{e9}\tM\u{e1}laga"), "{text}");
    }
    let path = write(&dir, "export.xls", b"Name,City\nJos\xe9,M\xe1laga\n");
    let error = read(&path, None, None).expect_err("text, not a document");
    assert!(error.contains("read it with read_file"), "{error}");
    let text = runtime::read_file(path.to_string_lossy().as_ref(), None, None)
        .expect("read_file reads it")
        .file
        .content;
    assert!(text.contains("Jos\u{e9},M\u{e1}laga"), "{text}");
    std::fs::remove_dir_all(dir).ok();
}
