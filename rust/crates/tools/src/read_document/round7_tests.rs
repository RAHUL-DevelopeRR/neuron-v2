//! Regressions from the seventh review: Word list restarts, list styles,
//! tracked formatting changes, footnotes and `SmartArt`; PDFs saved
//! incrementally, opened without a password, or using Unicode `CMap`s,
//! two-byte `ToUnicode` maps on simple fonts and rotated text; formula
//! cells without results, Excel's number display and `OpenDocument` times.
#![allow(clippy::format_collect)]

use std::fmt::Write as _;

use super::hostile_tests::ods;
use super::pdf::fixtures;
use super::pdf_crypt::{self, Method, Settings};
use super::round5_tests::{read, read_pdf, workbook};
use super::tests::{package, rels, temp_dir, write, W};
use super::Options;

fn read_bytes(name: &str, bytes: &[u8]) -> Result<String, String> {
    let dir = temp_dir("round7");
    let path = write(&dir, name, bytes);
    let out = read(
        &path,
        &Options {
            max_chars: Some(30_000),
            ..Options::default()
        },
    );
    std::fs::remove_dir_all(dir).ok();
    out
}

/// A Word document from `body` with `numbering` as its list definitions
/// and `extra` parts.
fn word(body: &str, numbering: &str, extra: &[(&str, String)]) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W} xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>
<w:style w:type="numbering" w:styleId="ClauseList"><w:name w:val="Clause List"/><w:pPr><w:numPr><w:numId w:val="7"/></w:numPr></w:pPr></w:style>
</w:styles>"#
    );
    let numbering = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W}>{numbering}</w:numbering>"#
    );
    let mut parts = vec![
        ("word/document.xml", document),
        ("word/styles.xml", styles),
        ("word/numbering.xml", numbering),
    ];
    parts.extend(extra.iter().map(|(name, xml)| (*name, xml.clone())));
    package(&parts)
}

fn item(num_id: u32, level: u32, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
    )
}

fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

const DECIMAL: &str = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2"/></w:lvl></w:abstractNum>"#;

#[test]
fn a_word_list_restarted_at_one_counts_from_one_again() {
    let numbering = format!(
        "{DECIMAL}<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>\
<w:num w:numId=\"2\"><w:abstractNumId w:val=\"0\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride></w:num>\
<w:num w:numId=\"3\"><w:abstractNumId w:val=\"0\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"10\"/></w:lvlOverride></w:num>"
    );
    let body = [
        item(1, 0, "A"),
        item(1, 0, "B"),
        item(1, 0, "C"),
        para("Between the lists."),
        item(2, 0, "D"),
        item(2, 1, "D sub"),
        item(2, 0, "E"),
        para("Set numbering value."),
        item(3, 0, "J"),
        item(3, 0, "K"),
    ]
    .concat();
    let out = read_bytes("lists.docx", &word(&body, &numbering, &[])).expect("reads");
    for line in [
        "1. A",
        "3. C",
        "1. D",
        "  1.1 D sub",
        "2. E",
        "10. J",
        "11. K",
    ] {
        assert!(out.contains(line), "{line}:\n{out}");
    }
    // A list that continues without a restart keeps counting.
    let numbering = format!(
        "{DECIMAL}<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"0\"/></w:num>"
    );
    let body = [item(1, 0, "A"), para("x"), item(2, 0, "B")].concat();
    let out = read_bytes("continued.docx", &word(&body, &numbering, &[])).expect("reads");
    assert!(out.contains("2. B"), "{out}");
}

#[test]
fn a_list_style_takes_its_levels_from_the_definition_linked_to_the_style() {
    // Word's "Define New List Style": abstractNum 5 holds the levels and
    // links the style; abstractNum 6 only names it, and the paragraphs'
    // list (numId 8) points at 6.
    let numbering = r#"<w:abstractNum w:abstractNumId="5"><w:styleLink w:val="ClauseList"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%2)"/></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="6"><w:numStyleLink w:val="ClauseList"/></w:abstractNum>
<w:num w:numId="7"><w:abstractNumId w:val="5"/></w:num>
<w:num w:numId="8"><w:abstractNumId w:val="6"/></w:num>"#;
    let body = [
        item(8, 0, "Definitions"),
        item(8, 1, "Agreement"),
        item(8, 1, "Party"),
        item(8, 0, "Term"),
    ]
    .concat();
    let out = read_bytes("clauses.docx", &word(&body, numbering, &[])).expect("reads");
    for line in [
        "1. Definitions",
        "  (a) Agreement",
        "  (b) Party",
        "2. Term",
    ] {
        assert!(out.contains(line), "{line}:\n{out}");
    }
    assert!(!out.contains("- Definitions"), "{out}");

    // Without a styleLink, the list style's own numbering finds the levels.
    let numbering = r#"<w:abstractNum w:abstractNumId="5"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="6"><w:numStyleLink w:val="ClauseList"/></w:abstractNum>
<w:num w:numId="7"><w:abstractNumId w:val="5"/></w:num>
<w:num w:numId="8"><w:abstractNumId w:val="6"/></w:num>"#;
    let out =
        read_bytes("clauses2.docx", &word(&item(8, 0, "Scope"), numbering, &[])).expect("reads");
    assert!(out.contains("1. Scope"), "{out}");
}

#[test]
fn tracked_formatting_changes_keep_the_current_formatting() {
    let numbering = format!(
        "{DECIMAL}<w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum>\
<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num>"
    );
    let change = |old: &str| {
        format!(
            "<w:pPrChange w:id=\"1\" w:author=\"Reviewer\" w:date=\"2026-01-01T00:00:00Z\"><w:pPr>{old}</w:pPr></w:pPrChange>"
        )
    };
    let body = [
        // Heading 2 changed to Heading 1.
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/>{}</w:pPr><w:r><w:t>Overview</w:t></w:r></w:p>",
            change("<w:pStyle w:val=\"Heading2\"/>")
        ),
        item(1, 0, "First"),
        // Indented from level 0 to level 1.
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"1\"/><w:numId w:val=\"1\"/></w:numPr>{}</w:pPr><w:r><w:t>Detail</w:t></w:r></w:p>",
            change("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>")
        ),
        // A bullet removed.
        format!(
            "<w:p><w:pPr>{}</w:pPr><w:r><w:t>Plain now</w:t></w:r></w:p>",
            change("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr>")
        ),
        item(1, 0, "Second"),
    ]
    .concat();
    let out = read_bytes("tracked.docx", &word(&body, &numbering, &[])).expect("reads");
    assert!(out.contains("# Overview"), "{out}");
    assert!(!out.contains("## Overview"), "{out}");
    assert!(out.contains("  1.1 Detail"), "{out}");
    assert!(out.contains("\nPlain now"), "{out}");
    assert!(!out.contains("- Plain now"), "{out}");
    assert!(out.contains("2. Second"), "{out}");
}

#[test]
fn footnotes_and_endnotes_are_marked_and_their_text_follows() {
    let body = [
        "<w:p><w:r><w:t xml:space=\"preserve\">as held in Smith</w:t></w:r><w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"2\"/></w:r><w:r><w:t xml:space=\"preserve\"> and Jones</w:t></w:r><w:r><w:footnoteReference w:id=\"3\"/></w:r></w:p>".to_string(),
        "<w:p><w:r><w:t>See the appendix</w:t></w:r><w:r><w:endnoteReference w:id=\"1\"/></w:r></w:p>".to_string(),
        "<w:p><w:r><w:t>Again Smith</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r></w:p>".to_string(),
    ]
    .concat();
    let footnotes = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {W}>
<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>
<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>
<w:footnote w:id="2"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Smith v. Jones, 123 F.3d 456 (2001).</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="3"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t>Jones, at 12.</w:t></w:r></w:p><w:p><w:r><w:t>Second paragraph.</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#
    );
    let endnotes = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:endnotes {W}>
<w:endnote w:type="separator" w:id="-1"><w:p/></w:endnote>
<w:endnote w:id="1"><w:p><w:r><w:endnoteRef/></w:r><w:r><w:t>Appendix A lists the sources.</w:t></w:r></w:p></w:endnote>
</w:endnotes>"#
    );
    let bytes = word(
        &body,
        "",
        &[
            ("word/footnotes.xml", footnotes),
            ("word/endnotes.xml", endnotes),
        ],
    );
    let out = read_bytes("memo.docx", &bytes).expect("reads");
    assert!(out.contains("as held in Smith[^1] and Jones[^2]"), "{out}");
    assert!(out.contains("See the appendix[^i]"), "{out}");
    assert!(out.contains("Again Smith[^1]"), "{out}");
    assert!(
        out.contains("[^1]: Smith v. Jones, 123 F.3d 456 (2001)."),
        "{out}"
    );
    assert!(
        out.contains("[^2]: Jones, at 12. Second paragraph."),
        "{out}"
    );
    assert!(out.contains("[^i]: Appendix A lists the sources."), "{out}");
    assert_eq!(out.matches("[^1]:").count(), 1, "{out}");

    // References whose notes are missing say so.
    let out = read_bytes("lost.docx", &word(&body, "", &[])).expect("reads");
    assert!(out.contains("have no note text"), "{out}");
}

/// A `SmartArt` data part: `points` are (id, text, parent id).
fn diagram_data(points: &[(&str, &str, &str)]) -> String {
    let mut pts = String::from(
        r#"<dgm:pt modelId="root" type="doc"><dgm:prSet/><dgm:spPr/><dgm:t><a:bodyPr/><a:p><a:endParaRPr/></a:p></dgm:t></dgm:pt>"#,
    );
    let mut cxns = String::new();
    for (index, (id, text, parent)) in points.iter().enumerate() {
        let _ = write!(
            pts,
            r#"<dgm:pt modelId="{id}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{text}</a:t></a:r></a:p></dgm:t></dgm:pt><dgm:pt modelId="pres{index}" type="pres"><dgm:prSet presName="node"/><dgm:spPr/></dgm:pt><dgm:pt modelId="par{index}" type="parTrans" cxnId="c{index}"><dgm:prSet/><dgm:spPr/><dgm:t><a:bodyPr/><a:p><a:endParaRPr/></a:p></dgm:t></dgm:pt>"#
        );
        let _ = write!(
            cxns,
            r#"<dgm:cxn modelId="c{index}" srcId="{parent}" destId="{id}" srcOrd="{index}" destOrd="0" parTransId="par{index}"/><dgm:cxn modelId="p{index}" type="presOf" srcId="{id}" destId="pres{index}" srcOrd="0" destOrd="0"/>"#
        );
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst><dgm:bg/><dgm:whole/></dgm:dataModel>"#
    )
}

const PROCESS: &[(&str, &str, &str)] = &[
    ("p1", "Plan", "root"),
    ("p1a", "Scope the work", "p1"),
    ("p2", "Build", "root"),
    ("p3", "Ship", "root"),
];

const RELIDS: &str = r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId8" r:lo="rId9" r:qs="rId10" r:cs="rId11"/></a:graphicData></a:graphic>"#;

#[test]
fn smartart_text_is_read_in_word_documents_and_presentations() {
    let body = format!(
        "{}<w:p><w:r><w:drawing><wp:inline xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\">{RELIDS}</wp:inline></w:drawing></w:r></w:p>{}",
        para("Our process:"),
        para("After the diagram.")
    );
    let bytes = word(
        &body,
        "",
        &[
            (
                "word/_rels/document.xml.rels",
                rels(&[("rId8", "diagramData", "diagrams/data1.xml")]),
            ),
            ("word/diagrams/data1.xml", diagram_data(PROCESS)),
        ],
    );
    let out = read_bytes("process.docx", &bytes).expect("reads");
    assert!(
        out.contains(
            "Our process:\n\n- Plan\n  - Scope the work\n- Build\n- Ship\n\nAfter the diagram."
        ),
        "{out}"
    );

    let p = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    let slide = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><p:sld {p}><p:cSld><p:spTree>{}<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Diagram 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>{RELIDS}</p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
        super::tests::shape(Some("title"), &[(0, "How we work")])
    );
    let presentation = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation {p}><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#
    );
    let bytes = package(&[
        ("ppt/presentation.xml", presentation),
        (
            "ppt/_rels/presentation.xml.rels",
            rels(&[("rId2", "slide", "slides/slide1.xml")]),
        ),
        ("ppt/slides/slide1.xml", slide),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            rels(&[("rId8", "diagramData", "../diagrams/data1.xml")]),
        ),
        ("ppt/diagrams/data1.xml", diagram_data(PROCESS)),
    ]);
    let out = read_bytes("deck.pptx", &bytes).expect("reads");
    assert!(
        out.contains("## Slide 1: How we work\n\n- Plan\n  - Scope the work\n- Build\n- Ship"),
        "{out}"
    );
}

/// A PDF of `objects` (number, body) written in this order, so one number
/// can be defined more than once, as incremental saves write them.
fn incremental(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    for (number, body) in objects {
        pdf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    pdf.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    pdf
}

/// An object stream holding `objects` (number, body).
fn object_stream(objects: &[(u32, &str)]) -> Vec<u8> {
    let mut header = String::new();
    let mut body = String::new();
    for (number, object) in objects {
        let _ = write!(header, "{number} {} ", body.len());
        body.push_str(object);
        body.push('\n');
    }
    let data = format!("{header}{body}");
    fixtures::stream(
        &format!("/Type /ObjStm /N {} /First {}", objects.len(), header.len()),
        data.as_bytes(),
        true,
    )
}

fn page_objects() -> Vec<(u32, Vec<u8>)> {
    vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        ),
        (
            4,
            fixtures::stream("", b"BT /F1 12 Tf 72 720 Td (Old first page) Tj ET", false),
        ),
        (
            5,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ),
        (
            6,
            b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        ),
        (
            7,
            fixtures::stream("", b"BT /F1 12 Tf 72 720 Td (Added page) Tj ET", false),
        ),
    ]
}

const ONE_PAGE: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const TWO_PAGES: &str = "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>";

#[test]
fn incremental_saves_read_the_newest_object_whatever_holds_it() {
    let cases = [
        // An older object stream, then a newer one in an appended update.
        vec![
            (8, object_stream(&[(2, ONE_PAGE)])),
            (9, object_stream(&[(2, TWO_PAGES)])),
        ],
        // A direct object, then an object stream appended after it.
        vec![
            (2, ONE_PAGE.as_bytes().to_vec()),
            (9, object_stream(&[(2, TWO_PAGES)])),
        ],
        // An object stream, then a direct object appended after it.
        vec![
            (8, object_stream(&[(2, ONE_PAGE)])),
            (2, TWO_PAGES.as_bytes().to_vec()),
        ],
    ];
    for (index, update) in cases.into_iter().enumerate() {
        let mut objects = page_objects();
        objects.extend(update);
        let pdf = incremental(&objects);
        // Object order in memory is random per map: the answer must not be.
        for _ in 0..16 {
            let out = read_pdf(&pdf);
            assert!(out.contains("## Page 2"), "case {index}: {out}");
            assert!(out.contains("Added page"), "case {index}: {out}");
        }
    }
}

#[test]
fn pdfs_with_only_an_owner_password_are_decrypted() {
    // Written by pypdf (RC4 40 and 128 bit, AES 128 and 256 bit, revision
    // 5 and 6) and by MuPDF with object streams.
    let files: [(&str, &[u8]); 8] = [
        ("rc4-40", include_bytes!("testdata/encrypted-rc4-40.pdf")),
        ("rc4-128", include_bytes!("testdata/encrypted-rc4-128.pdf")),
        ("aes-128", include_bytes!("testdata/encrypted-aes-128.pdf")),
        (
            "aes-256-r5",
            include_bytes!("testdata/encrypted-aes-256-r5.pdf"),
        ),
        ("aes-256", include_bytes!("testdata/encrypted-aes-256.pdf")),
        (
            "objstm-rc4",
            include_bytes!("testdata/encrypted-objstm-rc4.pdf"),
        ),
        (
            "objstm-aes128",
            include_bytes!("testdata/encrypted-objstm-aes128.pdf"),
        ),
        (
            "objstm-aes256",
            include_bytes!("testdata/encrypted-objstm-aes256.pdf"),
        ),
    ];
    for (name, bytes) in files {
        let out = read_bytes(&format!("{name}.pdf"), bytes).unwrap_or_else(|error| {
            panic!("{name}: {error}");
        });
        assert!(
            out.contains("Encrypted hello from page one"),
            "{name}: {out}"
        );
        assert!(
            out.contains("Second page (with parens) text"),
            "{name}: {out}"
        );
    }
    let error = read_bytes(
        "locked.pdf",
        include_bytes!("testdata/encrypted-needs-password.pdf"),
    )
    .expect_err("needs a password");
    assert!(
        error.contains("encrypted") && error.contains("password"),
        "{error}"
    );
}

/// The three-page fixture encrypted with `method` at `revision`, by the
/// test's own encryptor.
fn encrypted_fixture(revision: u32, method: Method) -> Vec<u8> {
    let id = b"0123456789abcdef".to_vec();
    let mut settings = Settings {
        revision,
        length_bits: Some(if revision == 2 { 40 } else { 128 }),
        owner: vec![0x5a; 32],
        user: Vec::new(),
        user_key: Vec::new(),
        permissions: -1084,
        encrypt_metadata: true,
        id: id.clone(),
        strings: method,
        streams: method,
    };
    let key = pdf_crypt::fixtures::file_key(&settings);
    settings.user = if revision == 2 {
        pdf_crypt::rc4(&key, &pdf_crypt::PADDING)
    } else {
        pdf_crypt::fixtures::legacy_user(&settings, &key)
    };
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
    };
    let mut objects = fixtures::three_page_objects();
    for (index, object) in objects.iter_mut().enumerate() {
        let number = u32::try_from(index + 1).expect("number");
        let Some(start) = object.windows(7).position(|window| window == b"stream\n") else {
            continue;
        };
        let end = object.len() - b"\nendstream".len();
        let data = pdf_crypt::fixtures::encrypt(&key, method, number, &object[start + 7..end]);
        let head = String::from_utf8_lossy(&object[..start]).to_string();
        let head = with_length(&head, data.len());
        let mut rebuilt = head.into_bytes();
        rebuilt.extend_from_slice(b"stream\n");
        rebuilt.extend_from_slice(&data);
        rebuilt.extend_from_slice(b"\nendstream");
        *object = rebuilt;
    }
    let (version, filters) = match method {
        Method::Aes128 => (
            4,
            " /CF << /StdCF << /CFM /AESV2 /Length 16 >> >> /StmF /StdCF /StrF /StdCF",
        ),
        _ if revision == 2 => (1, ""),
        _ => (2, ""),
    };
    objects.push(
        format!(
            "<< /Filter /Standard /V {version} /R {revision} /Length {} /O <{}> /U <{}> /P {}{filters} >>",
            settings.length_bits.unwrap_or(40),
            hex(&settings.owner),
            hex(&settings.user),
            settings.permissions
        )
        .into_bytes(),
    );
    let encrypt = objects.len();
    fixtures::assemble(
        &objects,
        &format!("/Encrypt {encrypt} 0 R /ID [<{}> <{}>]", hex(&id), hex(&id)),
    )
}

/// `head` (a stream dictionary) with its `/Length` set to `length`.
fn with_length(head: &str, length: usize) -> String {
    let at = head.find("/Length ").expect("length") + "/Length ".len();
    let end = head[at..]
        .find(|ch: char| !ch.is_ascii_digit())
        .map_or(head.len(), |offset| at + offset);
    format!("{}{length}{}", &head[..at], &head[end..])
}

#[test]
fn encrypted_streams_decrypt_with_each_legacy_method() {
    for (revision, method) in [(2, Method::Rc4), (3, Method::Rc4), (4, Method::Aes128)] {
        let out = read_pdf(&encrypted_fixture(revision, method));
        assert!(out.contains("Quarterly report"), "{revision}: {out}");
        assert!(out.contains("Revenue grew (a lot)"), "{revision}: {out}");
        assert!(out.contains("Hi"), "{revision}: {out}");
        assert!(out.contains("Inside a form"), "{revision}: {out}");
    }
    // An unknown handler or a missing dictionary is still refused.
    let unknown = fixtures::assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(),
            b"<< /Filter /FancyHandler /V 2 /R 3 >>".to_vec(),
        ],
        "/Encrypt 3 0 R",
    );
    let error = super::pdf::read_bytes(
        &unknown,
        &super::Selection::default(),
        &mut super::Budget::new(super::DEFAULT_MAX_CHARS),
    )
    .expect_err("unsupported");
    assert!(error.contains("encrypted"), "{error}");
}

#[test]
fn security_handler_primitives_match_known_answers() {
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        hex(&pdf_crypt::md5(b"")),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    assert_eq!(
        hex(&pdf_crypt::md5(b"abc")),
        "900150983cd24fb0d6963f7d28e17f72"
    );
    assert_eq!(
        hex(&pdf_crypt::md5(
            b"The quick brown fox jumps over the lazy dog"
        )),
        "9e107d9d372bb6826bd81d3542a419d6"
    );
    assert_eq!(
        hex(&pdf_crypt::md5(&[b'a'; 1000])),
        "cabe45dcc9ae5b66ba86600cca6b8ba8"
    );
    assert_eq!(
        hex(&pdf_crypt::rc4(b"Key", b"Plaintext")),
        "bbf316e8d940af0ad3"
    );
    assert_eq!(
        hex(&pdf_crypt::rc4(b"Secret", b"Attack at dawn")),
        "45a01f645fc35b383552544b9bf5"
    );
    // Algorithm 2.B for an empty password, as pypdf computes it.
    assert_eq!(
        hex(&pdf_crypt::fixtures::hash_2b(&(0..8).collect::<Vec<u8>>())),
        "1403c04eb647d2e60452dfc4eb0a5e0cf322e8a83a759eabbd17d498a93ba041"
    );
    assert_eq!(
        hex(&pdf_crypt::fixtures::hash_2b(b"saltsalt")),
        "f383e1cdcd8e7e67f45a599473d3f5a7bd181c381364391074cafe0713e3331d"
    );
}

/// One page drawing `content` with font `/F1` defined by `font` (object 5)
/// and `extra` objects from 6 on.
fn font_page(content: &[u8], font: &str, extra: Vec<Vec<u8>>) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        fixtures::stream("", content, false),
        font.as_bytes().to_vec(),
    ];
    objects.extend(extra);
    fixtures::assemble(&objects, "")
}

#[test]
fn composite_fonts_on_predefined_unicode_cmaps_read_without_a_unicode_map() {
    // iText's STSong-Light on UniGB-UCS2-H, not embedded, no ToUnicode.
    let pdf = font_page(
        b"BT /F1 12 Tf 72 720 Td <4E2D65875B57> Tj ET",
        "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [6 0 R] >>",
        vec![b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /DW 1000 >>".to_vec()],
    );
    let out = read_pdf(&pdf);
    assert!(out.contains("中文字"), "{out}");
    // UTF-16 CMaps carry characters past the BMP as surrogate pairs.
    let pdf = font_page(
        b"BT /F1 12 Tf 72 720 Td <D840DC0B0041> Tj ET",
        "<< /Type /Font /Subtype /Type0 /BaseFont /KozMinPro-Regular /Encoding /UniJIS-UTF16-H /DescendantFonts [6 0 R] >>",
        vec![b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /KozMinPro-Regular >>".to_vec()],
    );
    let out = read_pdf(&pdf);
    assert!(out.contains("\u{2000B}A"), "{out}");
    // Other composite fonts without a Unicode map still say so.
    let document = super::pdf::read_bytes(
        &font_page(
            b"BT /F1 12 Tf 72 720 Td <00010002> Tj ET",
            "<< /Type /Font /Subtype /Type0 /BaseFont /Embedded /Encoding /Identity-H /DescendantFonts [6 0 R] >>",
            vec![b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Embedded >>".to_vec()],
        ),
        &super::Selection::default(),
        &mut super::Budget::new(super::DEFAULT_MAX_CHARS),
    )
    .expect("reads");
    assert!(
        document
            .notes
            .iter()
            .any(|note| note.contains("no Unicode map") || note.contains("no text found")),
        "{:?}",
        document.notes
    );
}

#[test]
fn simple_fonts_read_one_byte_codes_whatever_their_unicode_map_claims() {
    let cmap = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
1 beginbfrange\n<0020> <007E> <0020>\nendbfrange\n\
1 beginbfchar\n<0041> <0391>\nendbfchar\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";
    let pdf = font_page(
        b"BT /F1 12 Tf 72 720 Td (Hello BETA) Tj ET",
        "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /Encoding /WinAnsiEncoding /ToUnicode 6 0 R >>",
        vec![fixtures::stream("", cmap, false)],
    );
    let out = read_pdf(&pdf);
    assert!(out.contains("Hello BET\u{391}"), "{out}");
}

#[test]
fn text_in_a_rotated_text_matrix_keeps_its_line_breaks_and_spaces() {
    let pdf = fixtures::one_page(
        b"BT /F1 10 Tf 0 1 -1 0 100 100 Tm (word1) Tj 0 -12 Td (word2) Tj [( next) -400 (after)] TJ ET",
        "",
        Vec::new(),
    );
    let out = read_pdf(&pdf);
    assert!(out.contains("word1\nword2 next after"), "{out}");
    // Upright text is unchanged.
    let pdf = fixtures::one_page(
        b"BT /F1 10 Tf 1 0 0 1 72 700 Tm (left) Tj ( right) Tj 0 -14 Td (below) Tj ET",
        "",
        Vec::new(),
    );
    let out = read_pdf(&pdf);
    assert!(out.contains("left right\nbelow"), "{out}");
}

#[test]
fn formula_cells_without_saved_results_show_their_formulas() {
    // openpyxl: formulas with no cached value, empty or missing.
    let rows = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Qty</t></is></c><c r="B1" t="inlineStr"><is><t>Price</t></is></c><c r="C1" t="inlineStr"><is><t>Total</t></is></c></row><row r="2"><c r="A2"><v>2</v></c><c r="B2"><v>3</v></c><c r="C2"><f>A2*B2</f><v></v></c></row><row r="3"><c r="A3"><v>4</v></c><c r="B3"><v>5</v></c><c r="C3"><f>A3*B3</f></c></row><row r="4"><c r="C4"><f>SUM(C2:C3)</f><v>26</v></c></row>"#;
    let out = read_bytes("openpyxl.xlsx", &workbook(rows, &[])).expect("reads");
    assert!(out.contains("| =A2*B2 |"), "{out}");
    assert!(out.contains("| =A3*B3 |"), "{out}");
    assert!(out.contains("| 26 |"), "{out}");
    assert!(out.contains("2 formula cell(s)"), "{out}");

    // xlsxwriter and openpyxl ask for a recalculation on load; their saved
    // results are placeholders.
    let mut bytes = workbook(
        r#"<row r="1"><c r="A1"><v>2</v></c><c r="B1"><f>A1*10</f><v>0</v></c><c r="C1"><f>A1*21</f><v>42</v></c></row>"#,
        &[],
    );
    let recalc = r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><calcPr calcId="124519" fullCalcOnLoad="1"/></workbook>"#;
    bytes = replace_part(&bytes, "xl/workbook.xml", recalc);
    let out = read_bytes("xlsxwriter.xlsx", &bytes).expect("reads");
    assert!(out.contains("| =A1*10 |"), "{out}");
    assert!(!out.contains("| 0 |"), "{out}");
    // A real saved result is kept even then.
    assert!(out.contains("| 42 |") && !out.contains("=A1*21"), "{out}");
}

/// `bytes` (a zip package) with part `name` holding `xml` instead.
fn replace_part(bytes: &[u8], name: &str, xml: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("entry");
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).expect("text");
        let entry_name = entry.name().expect("name").to_string();
        if entry_name == name {
            text = xml.to_string();
        }
        parts.push((entry_name, text));
    }
    let borrowed = parts
        .iter()
        .map(|(name, text)| (name.as_str(), text.clone()))
        .collect::<Vec<_>>();
    package(&borrowed)
}

#[test]
fn numbers_show_at_most_fifteen_significant_digits_as_excel_does() {
    let rows = r#"<row r="1"><c r="A1"><v>1358.0160000000001</v></c><c r="B1"><v>0.30000000000000004</v></c><c r="C1"><v>12345678.9</v></c><c r="D1"><v>-0.1</v></c><c r="E1"><v>1E-20</v></c><c r="F1"><v>1234567890.12345</v></c></row>"#;
    let out = read_bytes("noise.xlsx", &workbook(rows, &[])).expect("reads");
    for cell in [
        "| 1358.016 |",
        "| 0.3 |",
        "| 12345678.9 |",
        "| -0.1 |",
        "| 1234567890.12345 |",
    ] {
        assert!(out.contains(cell), "{cell}:\n{out}");
    }
    assert!(
        !out.contains("1358.0160000000001") && !out.contains("0.30000000000000004"),
        "{out}"
    );
}

#[test]
fn opendocument_times_and_dates_read_like_excel_ones() {
    let cell = |kind: &str, attr: &str, value: &str| {
        format!(
            r#"<table:table-cell office:value-type="{kind}" office:{attr}="{value}"><text:p>x</text:p></table:table-cell>"#
        )
    };
    let body = format!(
        r#"<table:table table:name="Times"><table:table-row>{}{}{}{}{}{}</table:table-row></table:table>"#,
        cell("time", "time-value", "PT09H30M00S"),
        cell("time", "time-value", "PT13H05M30S"),
        cell("time", "time-value", "PT36H30M00S"),
        cell("date", "date-value", "2024-01-15T09:30:00"),
        cell("date", "date-value", "2024-01-15T00:00:00"),
        cell("date", "date-value", "2024-02-01"),
    );
    let out = read_bytes("times.ods", &ods(&body)).expect("reads");
    for value in [
        "| 09:30 |",
        "| 13:05:30 |",
        "| 36:30:00 |",
        "| 2024-01-15 09:30:00 |",
        "| 2024-01-15 |",
        "| 2024-02-01 |",
    ] {
        assert!(out.contains(value), "{value}:\n{out}");
    }
    assert!(!out.contains("PT"), "{out}");
}

#[test]
fn the_main_word_part_may_pass_the_per_part_limit_within_the_package_budget() {
    let dir = temp_dir("main-part");
    let body = para(&"x".repeat(5_000));
    let path = write(&dir, "big.docx", &word(&body, "", &[]));
    let mut package = super::xml::Package::open(&path, "Word document").expect("opens");
    let error = package
        .read_part_up_to("word/document.xml", 1_000)
        .expect_err("over a small part limit");
    assert!(error.contains("unpacks to more than"), "{error}");
    let mut package = super::xml::Package::open(&path, "Word document").expect("opens");
    assert!(package
        .read_part_up_to("word/document.xml", 100_000)
        .expect("reads")
        .is_some());
    let limits = (super::xml::MAX_PART_BYTES, super::xml::MAX_MAIN_PART_BYTES);
    assert!(limits.1 > limits.0, "{limits:?}");
    std::fs::remove_dir_all(dir).ok();
}
