//! Regressions from the eighth review: PDFs whose `/Encrypt` sits in an
//! older trailer (linearized files), streams with an indirect `/Length`
//! under AES, encryption dictionaries without `/Length`, and Word notes on
//! estimated pages.
#![allow(clippy::format_collect)]

use std::fmt::Write as _;

use super::pdf::fixtures;
use super::pdf_crypt::{self, Method, Settings};
use super::round5_tests::{read, read_pdf};
use super::tests::{package, temp_dir, write, W};
use super::Options;

/// How a test file is encrypted.
#[derive(Clone, Copy)]
struct Lock {
    version: u32,
    revision: u32,
    method: Method,
    key_bits: u32,
    /// Whether the `/Encrypt` dictionary carries `/Length`.
    write_length: bool,
}

const RC4_128: Lock = Lock {
    version: 2,
    revision: 3,
    method: Method::Rc4,
    key_bits: 128,
    write_length: true,
};

const AES_128: Lock = Lock {
    version: 4,
    revision: 4,
    method: Method::Aes128,
    key_bits: 128,
    write_length: true,
};

const ID: &[u8] = b"0123456789abcdef";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// The settings of `lock` with an empty user password, and its file key.
fn unlocked(lock: Lock) -> (Settings, Vec<u8>) {
    let mut settings = Settings {
        version: lock.version,
        revision: lock.revision,
        length_bits: Some(lock.key_bits),
        owner: vec![0x5a; 32],
        user: Vec::new(),
        user_key: Vec::new(),
        permissions: -1084,
        encrypt_metadata: true,
        id: ID.to_vec(),
        strings: lock.method,
        streams: lock.method,
    };
    let key = pdf_crypt::fixtures::file_key(&settings);
    settings.user = if lock.revision == 2 {
        pdf_crypt::rc4(&key, &pdf_crypt::PADDING)
    } else {
        pdf_crypt::fixtures::legacy_user(&settings, &key)
    };
    (settings, key)
}

/// Encrypts the streams of `objects` (numbered from 1) with `lock`, setting
/// each stream's `/Length` (direct, or the number object an indirect one
/// names), and adds the `/Encrypt` dictionary. Returns the trailer entries.
fn encrypt(objects: &mut Vec<Vec<u8>>, lock: Lock) -> String {
    let (settings, key) = unlocked(lock);
    let mut lengths = Vec::new();
    for (index, object) in objects.iter_mut().enumerate() {
        let number = u32::try_from(index + 1).expect("number");
        let Some(start) = object.windows(7).position(|window| window == b"stream\n") else {
            continue;
        };
        let end = object.len() - b"\nendstream".len();
        let data = pdf_crypt::fixtures::encrypt(&key, lock.method, number, &object[start + 7..end]);
        let head = String::from_utf8_lossy(&object[..start]).to_string();
        let at = head.find("/Length ").expect("length") + "/Length ".len();
        let digits = head[at..]
            .find(|ch: char| !ch.is_ascii_digit())
            .map_or(head.len(), |offset| at + offset);
        let head = if head[digits..].starts_with(" 0 R") {
            lengths.push((head[at..digits].parse::<usize>().expect("ref"), data.len()));
            head
        } else {
            format!("{}{}{}", &head[..at], data.len(), &head[digits..])
        };
        let mut rebuilt = head.into_bytes();
        rebuilt.extend_from_slice(b"stream\n");
        rebuilt.extend_from_slice(&data);
        rebuilt.extend_from_slice(b"\nendstream");
        *object = rebuilt;
    }
    for (number, length) in lengths {
        objects[number - 1] = length.to_string().into_bytes();
    }
    let filters = if lock.method == Method::Aes128 {
        " /CF << /StdCF << /CFM /AESV2 /Length 16 >> >> /StmF /StdCF /StrF /StdCF"
    } else {
        ""
    };
    let length = if lock.write_length {
        format!(" /Length {}", lock.key_bits)
    } else {
        String::new()
    };
    objects.push(
        format!(
            "<< /Filter /Standard /V {} /R {}{length} /O <{}> /U <{}> /P {}{filters} >>",
            lock.version,
            lock.revision,
            hex(&settings.owner),
            hex(&settings.user),
            settings.permissions
        )
        .into_bytes(),
    );
    format!(
        "/Encrypt {} 0 R /ID [<{}> <{}>]",
        objects.len(),
        hex(ID),
        hex(ID)
    )
}

/// `objects` laid out as a linearized file is: the full trailer (`/Root`,
/// `trailer_extra`) in a first-page section at the start, and a last
/// section holding only `/Size`, as classic trailers or as
/// cross-reference streams.
fn linearized(objects: &[Vec<u8>], trailer_extra: &str, xref_streams: bool) -> Vec<u8> {
    let size = objects.len() + 1;
    let full = format!("/Size {size} /Root 1 0 R {trailer_extra}");
    let mut pdf = b"%PDF-1.7\n".to_vec();
    if xref_streams {
        pdf.extend_from_slice(
            format!(
                "90 0 obj\n<< /Type /XRef {full} /W [1 2 1] /Length 0 >>\nstream\n\nendstream\nendobj\n"
            )
            .as_bytes(),
        );
    } else {
        pdf.extend_from_slice(
            format!("xref\n0 0\ntrailer\n<< {full} /Prev 1000 >>\nstartxref\n0\n%%EOF\n")
                .as_bytes(),
        );
    }
    for (index, body) in objects.iter().enumerate() {
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    if xref_streams {
        pdf.extend_from_slice(
            format!(
                "91 0 obj\n<< /Type /XRef /Size {size} /W [1 2 1] /Length 0 >>\nstream\n\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
            )
            .as_bytes(),
        );
    } else {
        pdf.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size {size} >>\nstartxref\n9\n%%EOF\n"
            )
            .as_bytes(),
        );
    }
    pdf
}

fn assert_three_pages(out: &str, label: &str) {
    assert!(out.contains("Quarterly report"), "{label}: {out}");
    assert!(out.contains("Revenue grew (a lot)"), "{label}: {out}");
    assert!(out.contains("Hi"), "{label}: {out}");
    assert!(out.contains("Inside a form"), "{label}: {out}");
}

#[test]
fn linearized_pdfs_find_encrypt_in_the_first_page_trailer() {
    for lock in [None, Some(RC4_128), Some(AES_128)] {
        for xref_streams in [false, true] {
            let mut objects = fixtures::three_page_objects();
            let extra = lock.map_or_else(String::new, |lock| encrypt(&mut objects, lock));
            let pdf = linearized(&objects, &extra, xref_streams);
            assert_three_pages(
                &read_pdf(&pdf),
                &format!("encrypted {}, xref streams {xref_streams}", lock.is_some()),
            );
        }
    }
}

#[test]
fn a_trailer_inside_stream_data_is_not_the_files_own() {
    // An embedded file (stored uncompressed) holding another PDF's trailer
    // names an /Encrypt dictionary; the outer file is not encrypted.
    let mut objects = fixtures::three_page_objects();
    objects.push(fixtures::stream(
        "/Type /EmbeddedFile",
        b"xref\n0 1\ntrailer\n<< /Size 3 /Root 1 0 R /Encrypt 99 0 R /ID [<00> <00>] >>\nstartxref\n0\n%%EOF",
        false,
    ));
    // The embedded trailer comes before the file's own, where an older
    // section's would be, so it must not fill in /Encrypt.
    let pdf = fixtures::assemble(&objects, "");
    assert_three_pages(&read_pdf(&pdf), "embedded trailer");
}

#[test]
fn an_indirect_stream_length_keeps_aes_data_that_ends_in_a_line_feed() {
    // Content whose last encrypted byte is a line feed and whose last block
    // holds the end of the text: trimming it as an end of line drops that
    // block, and with it the text.
    let (_, key) = unlocked(AES_128);
    let content = (0..20_000)
        .map(|pad| {
            format!(
                "{}BT /F1 12 Tf 72 720 Td (Tail words here) Tj ET",
                " ".repeat(pad)
            )
        })
        .find(|content| {
            let data = pdf_crypt::fixtures::encrypt(&key, Method::Aes128, 4, content.as_bytes());
            data.last() == Some(&b'\n') && content.len() % 16 >= 11
        })
        .expect("some padding ends the data in a line feed");
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        {
            let mut stream = b"<< /Length 6 0 R >>\nstream\n".to_vec();
            stream.extend_from_slice(content.as_bytes());
            stream.extend_from_slice(b"\nendstream");
            stream
        },
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"0".to_vec(),
    ];
    let extra = encrypt(&mut objects, AES_128);
    let pdf = fixtures::assemble(&objects, &extra);
    let out = read_pdf(&pdf);
    assert!(out.contains("Tail words here"), "{out}");
}

#[test]
fn encryption_without_a_length_uses_the_default_for_its_version() {
    let cases = [
        // ISO 32000: no /Length at versions 1 to 3 means 40 bits.
        Lock {
            version: 2,
            revision: 3,
            method: Method::Rc4,
            key_bits: 40,
            write_length: false,
        },
        // A writer that left /Length out of a 128-bit file still opens.
        Lock {
            version: 2,
            revision: 3,
            method: Method::Rc4,
            key_bits: 128,
            write_length: false,
        },
        // Version 4 gives the length in its crypt filter.
        Lock {
            write_length: false,
            ..AES_128
        },
    ];
    for (index, lock) in cases.into_iter().enumerate() {
        let mut objects = fixtures::three_page_objects();
        let extra = encrypt(&mut objects, lock);
        let pdf = fixtures::assemble(&objects, &extra);
        assert_three_pages(&read_pdf(&pdf), &format!("case {index}"));
    }
}

#[test]
fn notes_on_estimated_pages_follow_their_reference() {
    // A Google Docs style export: no saved page breaks, so pages are cut by
    // size, with a footnote referenced in the first paragraph.
    let filler = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(45);
    let mut body = String::from(
        "<w:p><w:r><w:t>Opening claim</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r></w:p>",
    );
    for index in 0..8 {
        let _ = write!(
            body,
            "<w:p><w:r><w:t xml:space=\"preserve\">Section {index}. {filler}</w:t></w:r></w:p>"
        );
    }
    body.push_str(
        "<w:p><w:r><w:t>Closing claim</w:t></w:r><w:r><w:footnoteReference w:id=\"3\"/></w:r></w:p>",
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let footnotes = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {W}>
<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>
<w:footnote w:id="2"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> The opening source.</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="3"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> The closing source.</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#
    );
    let bytes = package(&[
        ("word/document.xml", document),
        ("word/footnotes.xml", footnotes),
    ]);
    let dir = temp_dir("round8");
    let path = write(&dir, "export.docx", &bytes);
    let first = read(
        &path,
        &Options {
            pages: Some("1"),
            max_chars: Some(30_000),
            ..Options::default()
        },
    )
    .expect("reads");
    let whole = read(
        &path,
        &Options {
            max_chars: Some(60_000),
            ..Options::default()
        },
    )
    .expect("reads");
    std::fs::remove_dir_all(dir).ok();

    assert!(first.contains("pages are estimated"), "{first}");
    assert!(first.contains("Opening claim[^1]"), "{first}");
    assert!(first.contains("[^1]: The opening source."), "{first}");
    assert!(!first.contains("[^2]: The closing source."), "{first}");
    assert_eq!(whole.matches("[^1]:").count(), 1, "{whole}");
    let closing = whole.find("Closing claim[^2]").expect("closing");
    let note = whole.find("[^2]: The closing source.").expect("note");
    assert!(note > closing, "{whole}");
}
