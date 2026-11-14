//! Fixtures are built in memory: OOXML packages from zip + XML, workbooks the
//! same way, and PDFs from [`super::pdf::fixtures`].
#![allow(clippy::format_collect)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::pdf::fixtures;
use super::{read_document, Selection, DEFAULT_MAX_CHARS};

pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "neuron-read-document-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A zip package of `parts` (path, contents).
pub(crate) fn package(parts: &[(&str, String)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, contents) in parts {
        writer.start_file(*name, options).expect("start part");
        writer.write_all(contents.as_bytes()).expect("write part");
    }
    writer.finish().expect("finish zip").into_inner()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write fixture");
    path
}

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

fn docx(body: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>
<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style>
<w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>
<w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
</w:styles>"#
    );
    let numbering = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W}>
<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#
    );
    package(&[
        ("word/document.xml", document),
        ("word/styles.xml", styles),
        ("word/numbering.xml", numbering),
    ])
}

fn paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

fn styled(style: &str, text: &str) -> String {
    format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn listed(num_id: u32, level: u32, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
    )
}

fn table_cell(text: &str) -> String {
    format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>")
}

pub(crate) fn sample_docx() -> Vec<u8> {
    let body = [
        styled("Title", "Project Plan"),
        styled("berschrift1", "Goals &amp; scope"),
        "<w:p><w:r><w:t xml:space=\"preserve\">Ship the </w:t></w:r><w:r><w:tab/><w:t>reader.</w:t></w:r></w:p>".to_string(),
        listed(1, 0, "First bullet"),
        listed(1, 1, "Nested bullet"),
        listed(2, 0, "Step one"),
        format!(
            "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
            table_cell("Name"),
            table_cell("Score"),
            table_cell("A|B"),
            table_cell("10")
        ),
        "<w:p><w:r><w:lastRenderedPageBreak/><w:t>Second page text</w:t></w:r></w:p>".to_string(),
        styled("Heading2", "Appendix"),
        "<mc:AlternateContent><mc:Choice Requires=\"wps\"><w:p><w:r><w:t>Choice text</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>Fallback text</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>".to_string(),
        "<w:p><w:r><w:delText>deleted words</w:delText><w:instrText>PAGE</w:instrText><w:t>Kept</w:t></w:r></w:p>".to_string(),
    ]
    .concat();
    docx(&body)
}

#[test]
fn docx_becomes_markdown_with_headings_lists_tables_and_pages() {
    let dir = temp_dir("docx");
    let path = write(&dir, "plan.docx", &sample_docx());
    let out = read_document(&path, None, None).expect("reads");
    assert!(
        out.starts_with("[plan.docx: Word document, 2 pages, showing all]"),
        "{out}"
    );
    for expected in [
        "# Project Plan",
        "# Goals & scope",
        "Ship the  reader.",
        "- First bullet",
        "  - Nested bullet",
        "1. Step one",
        "| Name | Score |\n| --- | --- |\n| A\\|B | 10 |",
        "[page 2]\n\nSecond page text",
        "## Appendix",
        "Choice text",
        "Kept",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in\n{out}");
    }
    for absent in ["Fallback text", "deleted words", "PAGE"] {
        assert!(!out.contains(absent), "{absent:?} leaked into\n{out}");
    }

    let second = read_document(&path, Some("2"), None).expect("page 2");
    assert!(second.contains("showing page 2]"), "{second}");
    assert!(second.contains("Second page text"));
    assert!(!second.contains("Project Plan"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn docx_without_page_breaks_gets_estimated_pages_and_a_truncation_hint() {
    let dir = temp_dir("docx-long");
    let body = (1..=30)
        .map(|index| paragraph(&format!("Paragraph {index} {}", "lorem ipsum ".repeat(25))))
        .collect::<String>();
    let path = write(&dir, "long.docx", &docx(&body));
    let out = read_document(&path, None, Some(4_000)).expect("reads");
    assert!(out.contains("pages are estimated"), "{out}");
    assert!(out.contains("[page 1]"));
    let hint = out.lines().last().expect("hint");
    assert!(
        hint.starts_with("[truncated: ") && hint.contains("more chars, use pages=\""),
        "{hint}"
    );
    // The hint names where to resume, and following it works.
    let resume = hint
        .split("pages=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("range");
    let next = read_document(&path, Some(resume), Some(4_000)).expect("resume");
    let first_page = resume.split('-').next().expect("start");
    assert!(next.contains(&format!("[page {first_page}]")), "{next}");
    assert!(
        out.chars().count() < 4_000 + 400,
        "header and hint stay small"
    );
    std::fs::remove_dir_all(dir).ok();
}

const P: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

fn shape(placeholder: Option<&str>, paragraphs: &[(u32, &str)]) -> String {
    let ph = placeholder.map_or_else(String::new, |kind| format!("<p:ph type=\"{kind}\"/>"));
    let body = paragraphs
        .iter()
        .map(|(level, text)| {
            format!("<a:p><a:pPr lvl=\"{level}\"/><a:r><a:t>{text}</a:t></a:r></a:p>")
        })
        .collect::<String>();
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"s\"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:txBody><a:bodyPr/>{body}</p:txBody></p:sp>"
    )
}

fn slide(attrs: &str, shapes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><p:sld {P} {attrs}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#
    )
}

fn rels(entries: &[(&str, &str, &str)]) -> String {
    let body = entries
        .iter()
        .map(|(id, kind, target)| {
            format!(
                "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}\" Target=\"{target}\"/>"
            )
        })
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#
    )
}

/// A presentation of `slides` (each a slide XML), in order, with notes for
/// the first slide.
fn pptx(slides: &[String], first_notes: Option<&str>) -> Vec<u8> {
    let ids = (0..slides.len())
        .map(|index| {
            format!(
                "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
                256 + index,
                index + 10
            )
        })
        .collect::<String>();
    let presentation = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation {P}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#
    );
    // Reverse the file names so order must come from sldIdLst, not names.
    let names = (0..slides.len())
        .map(|index| format!("slide{}.xml", slides.len() - index))
        .collect::<Vec<_>>();
    let rel_ids = (0..slides.len())
        .map(|index| format!("rId{}", index + 10))
        .collect::<Vec<_>>();
    let targets = names
        .iter()
        .map(|name| format!("slides/{name}"))
        .collect::<Vec<_>>();
    let rel_entries = rel_ids
        .iter()
        .zip(&targets)
        .map(|(id, target)| (id.as_str(), "slide", target.as_str()))
        .collect::<Vec<_>>();
    let mut parts = vec![
        ("ppt/presentation.xml".to_string(), presentation),
        (
            "ppt/_rels/presentation.xml.rels".to_string(),
            rels(&rel_entries),
        ),
    ];
    for (name, xml) in names.iter().zip(slides) {
        parts.push((format!("ppt/slides/{name}"), xml.clone()));
    }
    if let Some(notes) = first_notes {
        parts.push((
            format!("ppt/slides/_rels/{}.rels", names[0]),
            rels(&[("rId1", "notesSlide", "../notesSlides/notesSlide1.xml")]),
        ));
        let notes_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><p:notes {P}><p:cSld><p:spTree>{}{}{}</p:spTree></p:cSld></p:notes>"#,
            shape(Some("sldImg"), &[]),
            shape(Some("body"), &[(0, notes)]),
            shape(Some("sldNum"), &[(0, "1")])
        );
        parts.push(("ppt/notesSlides/notesSlide1.xml".to_string(), notes_xml));
    }
    let borrowed = parts
        .iter()
        .map(|(name, xml)| (name.as_str(), xml.clone()))
        .collect::<Vec<_>>();
    package(&borrowed)
}

fn table_frame(rows: &[&[&str]]) -> String {
    let body = rows
        .iter()
        .map(|row| {
            let cells = row
                .iter()
                .map(|cell| {
                    format!("<a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>{cell}</a:t></a:r></a:p></a:txBody></a:tc>")
                })
                .collect::<String>();
            format!("<a:tr h=\"1\">{cells}</a:tr>")
        })
        .collect::<String>();
    format!(
        "<p:graphicFrame><a:graphic><a:graphicData><a:tbl>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"
    )
}

#[test]
fn pptx_slides_come_in_order_with_titles_bullets_tables_and_notes() {
    let dir = temp_dir("pptx");
    let slides = [
        slide(
            "",
            &[
                shape(Some("title"), &[(0, "Welcome")]),
                shape(Some("body"), &[(0, "Point A"), (1, "Detail")]),
                shape(None, &[(0, "Free text box")]),
            ]
            .concat(),
        ),
        slide(
            "show=\"0\"",
            &[
                shape(Some("ctrTitle"), &[(0, "Numbers")]),
                table_frame(&[&["Region", "Sales"], &["North", "42"]]),
            ]
            .concat(),
        ),
    ];
    let path = write(&dir, "deck.pptx", &pptx(&slides, Some("Say hello first")));
    let out = read_document(&path, None, None).expect("reads");
    assert!(
        out.starts_with("[deck.pptx: PowerPoint presentation, 2 slides, showing all]"),
        "{out}"
    );
    let first = out.find("## Slide 1: Welcome").expect("slide 1");
    let second = out.find("## Slide 2: Numbers").expect("slide 2");
    assert!(first < second);
    for expected in [
        "- Point A\n  - Detail",
        "Free text box",
        "Notes:\nSay hello first",
        "(hidden slide)",
        "| Region | Sales |\n| --- | --- |\n| North | 42 |",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in\n{out}");
    }
    // The slide number placeholder on the notes page is not a note.
    assert!(!out.contains("Notes:\nSay hello first\n1"), "{out}");

    let only_second = read_document(&path, Some("2"), None).expect("slide 2");
    assert!(only_second.contains("showing slide 2]"));
    assert!(!only_second.contains("Welcome"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn many_slides_truncate_at_a_slide_boundary_and_one_long_slide_says_so() {
    let dir = temp_dir("pptx-long");
    let slides = (1..=10)
        .map(|number| {
            slide(
                "",
                &[
                    shape(Some("title"), &[(0, &format!("Topic {number}"))]),
                    shape(Some("body"), &[(0, &"words ".repeat(100))]),
                ]
                .concat(),
            )
        })
        .collect::<Vec<_>>();
    let path = write(&dir, "long.pptx", &pptx(&slides, None));
    let out = read_document(&path, Some("1-"), Some(2_000)).expect("reads");
    let hint = out.lines().last().expect("hint");
    assert!(hint.starts_with("[truncated: "), "{hint}");
    assert!(hint.contains("use pages=\""), "{hint}");
    assert!(hint.ends_with("-10\"]"), "{hint}");
    let resume = hint
        .split("pages=\"")
        .nth(1)
        .and_then(|rest| rest.split('-').next())
        .and_then(|start| start.parse::<usize>().ok())
        .expect("resume slide");
    assert!(out.contains(&format!("## Slide {}: ", resume - 1)), "{out}");
    assert!(!out.contains(&format!("## Slide {resume}: ")), "{out}");

    let big = slide("", &shape(Some("body"), &[(0, &"x".repeat(3_000))]));
    let path = write(&dir, "big.pptx", &pptx(&[big], None));
    let out = read_document(&path, None, Some(1_000)).expect("reads");
    assert!(
        out.contains("slide 1 alone is longer than max_chars"),
        "{out}"
    );
    std::fs::remove_dir_all(dir).ok();
}

fn xlsx(sheets: &[(&str, String)]) -> Vec<u8> {
    let entries = sheets
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!(
                "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                index + 1,
                index + 1
            )
        })
        .collect::<String>();
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{entries}</sheets></workbook>"#
    );
    let targets = (0..sheets.len())
        .map(|index| {
            (
                format!("rId{}", index + 1),
                format!("worksheets/sheet{}.xml", index + 1),
            )
        })
        .collect::<Vec<_>>();
    let rel_entries = targets
        .iter()
        .map(|(id, target)| (id.as_str(), "worksheet", target.as_str()))
        .collect::<Vec<_>>();
    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    let mut parts = vec![
        ("[Content_Types].xml".to_string(), content_types.to_string()),
        (
            "_rels/.rels".to_string(),
            rels(&[("rId1", "officeDocument", "xl/workbook.xml")]),
        ),
        ("xl/workbook.xml".to_string(), workbook),
        ("xl/_rels/workbook.xml.rels".to_string(), rels(&rel_entries)),
    ];
    for (index, (_, rows)) in sheets.iter().enumerate() {
        parts.push((
            format!("xl/worksheets/sheet{}.xml", index + 1),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
            ),
        ));
    }
    let borrowed = parts
        .iter()
        .map(|(name, xml)| (name.as_str(), xml.clone()))
        .collect::<Vec<_>>();
    package(&borrowed)
}

fn row(number: usize, cells: &[&str]) -> String {
    let cells = cells
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let reference = format!("{}{number}", super::sheet::column_name(index));
            if value.parse::<f64>().is_ok() {
                format!("<c r=\"{reference}\"><v>{value}</v></c>")
            } else {
                format!("<c r=\"{reference}\" t=\"inlineStr\"><is><t>{value}</t></is></c>")
            }
        })
        .collect::<String>();
    format!("<row r=\"{number}\">{cells}</row>")
}

#[test]
fn spreadsheets_show_each_sheet_as_a_table_with_caps_and_name_selection() {
    let dir = temp_dir("xlsx");
    let sales = [
        row(1, &["Region", "Units", "Price"]),
        row(2, &["North", "10", "2.5"]),
        row(4, &["South|East", "7", "3"]),
    ]
    .concat();
    let mut log_rows = String::new();
    for number in 1..=520 {
        log_rows.push_str(&row(number, &[&format!("item {number}"), "1"]));
    }
    let wide_values = (1..=45).map(|value| value.to_string()).collect::<Vec<_>>();
    let wide = row(
        1,
        &wide_values.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let path = write(
        &dir,
        "book.xlsx",
        &xlsx(&[("Sales", sales), ("Log", log_rows), ("Wide", wide)]),
    );

    let out = read_document(&path, Some("1"), None).expect("reads");
    assert!(
        out.starts_with("[book.xlsx: spreadsheet, 3 sheets, showing sheet 1]"),
        "{out}"
    );
    assert!(
        out.contains("[note: sheets: 1 Sales, 2 Log, 3 Wide]"),
        "{out}"
    );
    assert!(
        out.contains("## Sheet 1: Sales (4 rows x 3 columns)"),
        "{out}"
    );
    assert!(
        out.contains(
            "| Row | A | B | C |\n| --- | --- | --- | --- |\n| 1 | Region | Units | Price |\n| 2 | North | 10 | 2.5 |\n| 4 | South\\|East | 7 | 3 |"
        ),
        "{out}"
    );

    let log = read_document(&path, Some("Log"), Some(100_000)).expect("by name");
    assert!(log.contains("## Sheet 2: Log"), "{log}");
    assert!(log.contains("| 500 | item 500 | 1 |"));
    assert!(!log.contains("| 501 | item 501"));
    assert!(log.contains("[showing the first 500 of 520 non-empty rows (to row 500)]"));

    let wide = read_document(&path, Some("3"), None).expect("wide");
    assert!(wide.contains("[showing columns A-AN of 45]"), "{wide}");

    let error = read_document(&path, Some("Missing"), None).expect_err("no such sheet");
    assert!(error.contains("1 Sales, 2 Log, 3 Wide"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pdf_text_comes_page_by_page_through_unicode_maps_and_forms() {
    let dir = temp_dir("pdf");
    let path = write(&dir, "report.pdf", &fixtures::three_page_pdf());
    let out = read_document(&path, None, None).expect("reads");
    assert!(
        out.starts_with("[report.pdf: PDF document, 3 pages, showing all]"),
        "{out}"
    );
    assert!(
        out.contains("## Page 1\n\nQuarterly report\nRevenue grew (a lot)"),
        "{out}"
    );
    assert!(out.contains("## Page 2\n\nHi\nA BC"), "{out}");
    assert!(out.contains("## Page 3\n\nInside a form"), "{out}");

    let range = read_document(&path, Some("2-3"), None).expect("range");
    assert!(range.contains("showing page 2-3]"), "{range}");
    assert!(!range.contains("Quarterly"));
    assert!(range.contains("Inside a form"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pdf_pages_inside_object_streams_are_found() {
    let header_and_objects = |pages_offset: usize| format!("5 0 6 {pages_offset} ");
    let pages = "<< /Type /Pages /Kids [6 0 R] /Count 1 /Resources << /Font << /F1 3 0 R >> >> >>";
    let page = "<< /Type /Page /Parent 5 0 R /Contents 4 0 R >>";
    let mut header = header_and_objects(pages.len() + 1);
    while header.len() < 20 {
        header.push(' ');
    }
    let packed = format!("{header}{pages}\n{page}");
    let objects = vec![
        b"<< /Type /Catalog /Pages 5 0 R >>".to_vec(),
        fixtures::stream(
            &format!("/Type /ObjStm /N 2 /First {}", header.len()),
            packed.as_bytes(),
            true,
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        fixtures::stream("", b"BT /F1 12 Tf 72 720 Td (Packed page) Tj ET", true),
    ];
    let document = super::pdf::read_bytes(&fixtures::assemble(&objects, ""), &Selection::default())
        .expect("reads");
    assert_eq!(document.total, 1);
    assert!(
        document.units[0].markdown.contains("Packed page"),
        "{:?}",
        document.units
    );
}

#[test]
fn bad_input_gets_short_clear_errors() {
    let dir = temp_dir("errors");
    let encrypted = write(
        &dir,
        "locked.pdf",
        &fixtures::assemble(&fixtures::three_page_objects(), "/Encrypt 99 0 R"),
    );
    let error = read_document(&encrypted, None, None).expect_err("encrypted");
    assert!(error.contains("encrypted"), "{error}");

    let fake_pdf = write(&dir, "fake.pdf", b"hello there");
    let error = read_document(&fake_pdf, None, None).expect_err("not a pdf");
    assert!(error.contains("%PDF header is missing"), "{error}");

    let not_zip = write(&dir, "broken.docx", b"definitely not a zip");
    let error = read_document(&not_zip, None, None).expect_err("not a zip");
    assert!(error.contains("not a readable Word document"), "{error}");

    let no_body = write(
        &dir,
        "empty.docx",
        &package(&[("word/styles.xml", "<x/>".to_string())]),
    );
    let error = read_document(&no_body, None, None).expect_err("no body");
    assert!(error.contains("word/document.xml is missing"), "{error}");

    let bad_xml = write(
        &dir,
        "badxml.docx",
        &package(&[(
            "word/document.xml",
            "<w:document><w:body></w:p></w:document>".to_string(),
        )]),
    );
    let error = read_document(&bad_xml, None, None).expect_err("damaged xml");
    assert!(error.contains("damaged XML"), "{error}");

    let not_book = write(&dir, "broken.xlsx", b"nope");
    let error = read_document(&not_book, None, None).expect_err("not a workbook");
    assert!(error.contains("not a readable spreadsheet"), "{error}");

    let text = write(&dir, "notes.txt", b"plain");
    let error = read_document(&text, None, None).expect_err("unsupported");
    assert!(error.contains("use read_file"), "{error}");

    let pdf = write(&dir, "report.pdf", &fixtures::three_page_pdf());
    let error = read_document(&pdf, Some("9"), None).expect_err("past the end");
    assert_eq!(error, "pages selects nothing: report.pdf has 3 pages");
    let error = read_document(&pdf, Some("0"), None).expect_err("zero");
    assert!(error.contains("not a valid range"), "{error}");
    let error = read_document(&pdf, Some("intro"), None).expect_err("name on a pdf");
    assert!(error.contains("not a page range"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn selections_parse_ranges_lists_and_names() {
    let selection = Selection::parse("1, 3-4 ,7-").expect("parses");
    assert!(selection.includes(1, None));
    assert!(!selection.includes(2, None));
    assert!(selection.includes(4, None));
    assert!(selection.includes(70, None));
    let names = Selection::parse("Totals").expect("name");
    assert!(names.has_names());
    assert!(names.includes(9, Some("totals")));
    assert!(!names.includes(1, Some("Other")));
    assert!(Selection::parse("5-2").is_err());
    assert!(Selection::default().is_all());
    assert_eq!(super::compact_ranges(&[1, 2, 3, 5, 7, 8]), "1-3,5,7-8");
}

#[test]
fn the_default_budget_suits_small_models() {
    assert_eq!(DEFAULT_MAX_CHARS, 12_000);
}
