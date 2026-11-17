//! Regressions from the ninth review: tracked moves and deletions in Word,
//! list levels shown without a number, notes referenced from long tables
//! on estimated pages, AES streams whose indirect `/Length` is packed in an
//! object stream, and key lengths given only in a crypt filter.
#![allow(clippy::format_collect)]

use std::fmt::Write as _;

use super::pdf::fixtures;
use super::pdf_crypt::{self, Method};
use super::round5_tests::{read, read_pdf};
use super::round7_tests::{item, read_bytes, word};
use super::round8_tests::{assert_three_pages, encrypt, unlocked, Lock, AES_128};
use super::tests::{package, temp_dir, write, W};
use super::Options;

const LIST: &str = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="none"/><w:lvlText w:val=""/></w:lvl><w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val=""/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;

const CHANGE: &str = r#"w:author="Reviewer" w:date="2026-11-01T10:00:00Z""#;

#[test]
fn tracked_moves_and_deletions_show_only_the_current_text() {
    let moved = "Payment is due in 30 days.";
    let body = [
        item(1, 0, "First clause."),
        // Where the paragraph was: its mark and its text moved away.
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr><w:rPr><w:moveFrom w:id=\"1\" {CHANGE}/></w:rPr></w:pPr>\
<w:moveFromRangeStart w:id=\"2\" w:name=\"move1\" {CHANGE}/><w:moveFrom w:id=\"3\" {CHANGE}><w:r><w:t>{moved}</w:t></w:r></w:moveFrom><w:moveFromRangeEnd w:id=\"2\"/></w:p>"
        ),
        item(1, 0, "Second clause."),
        // Where it went.
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr><w:rPr><w:moveTo w:id=\"4\" {CHANGE}/></w:rPr></w:pPr>\
<w:moveToRangeStart w:id=\"5\" w:name=\"move1\" {CHANGE}/><w:moveTo w:id=\"6\" {CHANGE}><w:r><w:t>{moved}</w:t></w:r></w:moveTo><w:moveToRangeEnd w:id=\"5\"/></w:p>"
        ),
        // A deleted run's footnote mark and line break are gone too.
        format!(
            "<w:p><w:r><w:t>Kept text</w:t></w:r><w:del w:id=\"7\" {CHANGE}><w:r><w:delText xml:space=\"preserve\"> removed words</w:delText></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r><w:r><w:br/><w:tab/></w:r></w:del><w:r><w:t xml:space=\"preserve\"> continues.</w:t></w:r></w:p>"
        ),
        "<w:p><w:r><w:t>Sourced claim</w:t></w:r><w:r><w:footnoteReference w:id=\"3\"/></w:r></w:p>"
            .to_string(),
    ]
    .concat();
    let footnotes = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {W}>
<w:footnote w:id="2"><w:p><w:r><w:t>Deleted note.</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="3"><w:p><w:r><w:footnoteRef/></w:r><w:moveFrom w:id="8" {CHANGE}><w:r><w:t xml:space="preserve"> Old place.</w:t></w:r></w:moveFrom><w:r><w:t xml:space="preserve"> The real source.</w:t></w:r><w:del w:id="9" {CHANGE}><w:r><w:delText> Struck.</w:delText></w:r></w:del></w:p></w:footnote>
</w:footnotes>"#
    );
    let out = read_bytes(
        "redline.docx",
        &word(&body, LIST, &[("word/footnotes.xml", footnotes)]),
    )
    .expect("reads");

    assert_eq!(out.matches(moved).count(), 1, "{out}");
    assert!(out.contains("1. First clause."), "{out}");
    // The moved-away copy takes no number.
    assert!(out.contains("2. Second clause."), "{out}");
    assert!(out.contains(&format!("3. {moved}")), "{out}");
    assert!(out.contains("Kept text continues."), "{out}");
    assert!(!out.contains("removed words"), "{out}");
    assert!(!out.contains("Deleted note."), "{out}");
    assert!(out.contains("Sourced claim[^1]"), "{out}");
    assert!(out.contains("[^1]: The real source."), "{out}");
    assert!(!out.contains("Old place."), "{out}");
    assert!(!out.contains("Struck."), "{out}");
    assert!(!out.contains("no note text"), "{out}");
}

#[test]
fn a_list_level_without_a_number_is_only_indented() {
    let body = [
        item(1, 0, "Scope."),
        item(1, 1, "Definitions follow."),
        item(1, 2, "Empty template."),
        item(1, 0, "Terms."),
    ]
    .concat();
    let out = read_bytes("levels.docx", &word(&body, LIST, &[])).expect("reads");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"1. Scope."), "{out}");
    assert!(lines.contains(&"  Definitions follow."), "{out}");
    assert!(lines.contains(&"    Empty template."), "{out}");
    assert!(lines.contains(&"2. Terms."), "{out}");
    assert!(!out.contains("1. Definitions"), "{out}");
    assert!(!out.contains("1. Empty"), "{out}");
}

#[test]
fn a_note_referenced_in_a_long_table_stays_on_the_page_of_its_mark() {
    // A Google Docs style export (no saved page breaks) with a table of
    // about 15,000 characters, its second row holding a footnote mark.
    let filler = "Revenue line item with a long description of what it covers. ".repeat(4);
    let mut rows = String::from(
        "<w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Detail</w:t></w:r></w:p></w:tc></w:tr>",
    );
    for index in 0..60 {
        let mark = if index == 1 {
            "<w:r><w:footnoteReference w:id=\"2\"/></w:r>"
        } else {
            ""
        };
        let _ = write!(
            rows,
            "<w:tr><w:tc><w:p><w:r><w:t>Row {index}</w:t></w:r>{mark}</w:p></w:tc><w:tc><w:p><w:r><w:t xml:space=\"preserve\">{filler}</w:t></w:r></w:p></w:tc></w:tr>"
        );
    }
    let body = format!(
        "<w:p><w:r><w:t>Opening</w:t></w:r></w:p><w:tbl>{rows}</w:tbl><w:p><w:r><w:t>After the table</w:t></w:r></w:p>"
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let footnotes = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {W}>
<w:footnote w:id="2"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> The table source.</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#
    );
    let bytes = package(&[
        ("word/document.xml", document),
        ("word/footnotes.xml", footnotes),
    ]);
    let dir = temp_dir("round9");
    let path = write(&dir, "table.docx", &bytes);
    let options = |pages| Options {
        pages,
        max_chars: Some(60_000),
        ..Options::default()
    };
    let first = read(&path, &options(Some("1"))).expect("reads");
    let whole = read(&path, &options(None)).expect("reads");
    std::fs::remove_dir_all(dir).ok();

    assert!(first.contains("pages are estimated"), "{first}");
    assert!(first.contains("Row 1[^1]"), "{first}");
    assert!(first.contains("[^1]: The table source."), "{first}");
    assert_eq!(
        whole.matches("[^1]: The table source.").count(),
        1,
        "{whole}"
    );
    let note = whole.find("[^1]: The table source.").expect("note");
    let last_row = whole.find("Row 59").expect("last row");
    assert!(note < last_row, "{whole}");
    assert!(whole.find("Row 1[^1]").expect("mark") < note, "{whole}");
}

/// `object` with its direct `/Length` replaced by a reference to `number`.
fn length_by_reference(object: &[u8], number: u32) -> Vec<u8> {
    let start = object
        .windows(8)
        .position(|window| window == b"/Length ")
        .expect("length")
        + 8;
    let end = start
        + object[start..]
            .iter()
            .position(|byte| !byte.is_ascii_digit())
            .expect("digits end");
    let mut out = object[..start].to_vec();
    out.extend_from_slice(format!("{number} 0 R").as_bytes());
    out.extend_from_slice(&object[end..]);
    out
}

#[test]
fn an_aes_stream_whose_length_is_packed_in_an_object_stream_keeps_its_last_block() {
    // The content stream's /Length is object 9, which only an object stream
    // holds, so its exact end is not known before the stream is decrypted.
    // Its last encrypted byte is a line end, which the `endstream` fallback
    // trims, and its last block holds the end of the text.
    let (_, key) = unlocked(AES_128);
    let encrypted =
        |content: &str| pdf_crypt::fixtures::encrypt(&key, Method::Aes128, 4, content.as_bytes());
    let content = (0..20_000)
        .map(|pad| {
            format!(
                "{}BT /F1 12 Tf 72 720 Td (Final sentence here) Tj ET",
                " ".repeat(pad)
            )
        })
        .find(|content| {
            matches!(encrypted(content).last(), Some(b'\n' | b'\r')) && content.len() % 16 >= 11
        })
        .expect("some padding ends the data in a line end");
    let packed = format!("9 0 {}", encrypted(&content).len());
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", content.as_bytes(), false),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        fixtures::stream("/Type /ObjStm /N 1 /First 4", packed.as_bytes(), false),
    ];
    let extra = encrypt(&mut objects, AES_128);
    objects[3] = length_by_reference(&objects[3], 9);
    let pdf = fixtures::assemble(&objects, &extra);
    let out = read_pdf(&pdf);
    assert!(out.contains("Final sentence here"), "{out}");
}

#[test]
fn a_key_length_given_only_in_the_crypt_filter_is_read_in_bytes_or_bits() {
    // A 96-bit key is neither length tried without one (128 bits, then 40),
    // so these files open only through the crypt filter's /Length: 12 is
    // bytes (under 40), 96 is bits.
    for filter_length in [12, 96] {
        let lock = Lock {
            version: 4,
            revision: 4,
            method: Method::Rc4,
            key_bits: 96,
            write_length: false,
        };
        let mut objects = fixtures::three_page_objects();
        let extra = encrypt(&mut objects, lock);
        let dictionary = objects.last_mut().expect("encrypt dictionary");
        let text = String::from_utf8(dictionary.clone()).expect("ascii");
        let text = text.strip_suffix(" >>").expect("dictionary end");
        *dictionary = format!(
            "{text} /CF << /StdCF << /CFM /V2 /Length {filter_length} >> >> /StmF /StdCF /StrF /StdCF >>"
        )
        .into_bytes();
        let pdf = fixtures::assemble(&objects, &extra);
        assert_three_pages(&read_pdf(&pdf), &format!("filter length {filter_length}"));
    }
}
