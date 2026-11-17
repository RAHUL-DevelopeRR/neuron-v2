//! PDF text, page by page.
//!
//! A small reader of its own: it indexes the file's objects (including those
//! packed in object streams), walks the page tree, and runs each page's
//! content streams, decoding text through the fonts' `ToUnicode` maps and
//! following form `XObject`s. Encrypted files that open without a password
//! (an owner password only) are decrypted; those that need a password are
//! refused. Scanned pages without a text layer come back empty, since no OCR
//! runs.
//!
//! When a file was saved incrementally, the definition of an object written
//! later in the file wins, whether it is written directly or packed in an
//! object stream.
//!
//! The file is treated as hostile: object nesting, the number of parsed
//! objects, the bytes inflated per call, nested forms, font maps and the
//! text kept per page all have limits, and the object scan is linear.

use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;
use std::rc::Rc;

use super::glyphs::{self, win_ansi, CodeTable};
use super::pdf_crypt::{self, Method, Security, Settings};
use super::{Budget, Document, Selection, Unit};

const KIND: &str = "PDF document";
/// The largest single stream inflated.
const MAX_STREAM_BYTES: u64 = 64 * 1024 * 1024;
/// The most bytes decoded (and run) across one call: object streams, page
/// contents, forms each time they are drawn, and font maps.
pub(super) const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
/// How deep page trees and references are followed.
const MAX_DEPTH: usize = 32;
/// How deeply form `XObject`s nest.
const MAX_FORM_DEPTH: usize = 6;
/// How deeply arrays and dictionaries nest before the rest is skipped.
pub(super) const MAX_OBJECT_DEPTH: usize = 64;
/// The most parsed values kept for the whole file.
pub(super) const MAX_NODES: usize = 4_000_000;
/// The most values one object (or one content stream operand) is built
/// from; the rest of it is skipped. Checked while the object is built, so a
/// single huge array cannot fill memory before any limit is looked at.
pub(super) const MAX_OBJECT_NODES: usize = 1_000_000;
/// Page tree nodes visited, pages included.
pub(super) const MAX_PAGE_TREE_NODES: usize = 1_000_000;
/// UTF-16 units one `ToUnicode` entry maps a code to (ligatures need a few).
const MAX_MAPPED_UNITS: usize = 32;
/// Whitespace allowed between a stream's declared end and `endstream`.
const MAX_STREAM_END_GAP: usize = 64;
/// The most objects one object stream may declare.
const MAX_PACKED_OBJECTS: usize = 100_000;
/// The most character codes and width entries one font may define.
pub(super) const MAX_FONT_CODES: usize = 1_000_000;
/// Values kept while waiting for an operator or `CMap` keyword.
const MAX_OPERANDS: usize = 64;
const MAX_PENDING: usize = 100_000;
/// Graphics states saved with `q` at once.
const MAX_SAVED_STATES: usize = 256;
/// Bytes of text kept per page; more than any call returns.
const MAX_PAGE_TEXT: usize = 1_000_000;

pub(super) fn read(
    path: &Path,
    selection: &Selection,
    budget: &mut Budget,
) -> Result<Document, String> {
    let data = std::fs::read(path).map_err(|error| format!("cannot open the file: {error}"))?;
    if !runtime::has_pdf_header(&data) && super::looks_like_text(&data) {
        return Err(super::text_file_error(path));
    }
    read_bytes(&data, selection, budget)
}

#[allow(clippy::too_many_lines)]
pub(super) fn read_bytes(
    data: &[u8],
    selection: &Selection,
    budget: &mut Budget,
) -> Result<Document, String> {
    if !runtime::has_pdf_header(data) {
        return Err("this is not a readable PDF document: the %PDF header is missing".to_string());
    }
    let pdf = Pdf::parse(data)?;
    let mut run = Run::default();
    let pages = pdf.pages();
    let mut notes = Vec::new();
    if pages.is_empty() {
        // No page tree found: run every content-like stream as one page.
        let mut sink = TextSink::default();
        let mut numbers = pdf.objects.keys().copied().collect::<Vec<_>>();
        numbers.sort_unstable();
        for number in numbers {
            let object = &pdf.objects[&number];
            let Obj::Stream(dict, _) = object else {
                continue;
            };
            // Content streams carry no type; fonts, images and the like do,
            // and embedded font programs carry their lengths.
            if ["Type", "Subtype", "Length1", "Length2"]
                .iter()
                .any(|key| dict.contains_key(*key))
            {
                continue;
            }
            if let Some(content) = pdf.decode(object) {
                if find(&content, b"BT").is_some() {
                    pdf.run_content(&content, None, &mut sink, &mut run, 0, IDENTITY);
                    sink.newline();
                }
            }
            if pdf.decoded_all() {
                break;
            }
        }
        notes.push(
            "the page structure could not be read; all text is shown as one page".to_string(),
        );
        let text = sink.finish();
        let units = if selection.includes(1, None) {
            vec![Unit {
                number: 1,
                markdown: page_markdown(1, &text),
            }]
        } else {
            Vec::new()
        };
        return Ok(Document {
            kind: KIND,
            unit: "page",
            total: 1,
            units,
            notes,
            unread: Vec::new(),
            selectors: Vec::new(),
        });
    }

    let mut units = Vec::new();
    let mut unread = Vec::new();
    let mut undecodable = false;
    let mut any_text = false;
    let mut cut_short = false;
    let mut stopped_at = None;
    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, None) {
            continue;
        }
        if budget.is_spent() || stopped_at.is_some() {
            unread.push(number);
            continue;
        }
        let mut sink = TextSink::default();
        // The same stream listed twice draws the same text twice: decode
        // each one once.
        let mut seen = HashSet::new();
        let mut content = Vec::new();
        for reference in &page.contents {
            if let Obj::Ref(object) = reference {
                if !seen.insert(*object) {
                    continue;
                }
            }
            if let Some(data) = pdf.stream_data(reference) {
                content.extend_from_slice(&data);
                content.push(b'\n');
            }
        }
        pdf.run_content(&content, page.resources, &mut sink, &mut run, 0, IDENTITY);
        undecodable |= sink.undecodable;
        cut_short |= sink.cut;
        let text = sink.finish();
        any_text |= !text.is_empty();
        let markdown = page_markdown(number, &text);
        budget.spend(&markdown);
        units.push(Unit { number, markdown });
        if pdf.decoded_all() {
            stopped_at = Some(number);
        }
    }
    if !units.is_empty() && !any_text {
        notes.push(
            "no text found: the pages may be scanned images, which this tool cannot read"
                .to_string(),
        );
    } else if undecodable {
        notes
            .push("some text could not be decoded because its font has no Unicode map".to_string());
    }
    if let Some(page) = stopped_at {
        notes.push(format!(
            "stopped after page {page}: the file's content unpacks to more than {} MB in one call; ask for later pages with pages=",
            MAX_DECODED_BYTES / (1024 * 1024)
        ));
    }
    if cut_short {
        notes.push(
            "some pages hold more drawing instructions or text than the reader follows; their text may be incomplete"
                .to_string(),
        );
    }
    Ok(Document {
        kind: KIND,
        unit: "page",
        total: pages.len(),
        units,
        notes,
        unread,
        selectors: Vec::new(),
    })
}

fn page_markdown(number: usize, text: &str) -> String {
    if text.trim().is_empty() {
        format!("## Page {number}\n\n(no text on this page)")
    } else {
        format!("## Page {number}\n\n{}", text.trim())
    }
}

// ---------------------------------------------------------------------------
// Objects and lexing

#[derive(Debug, Clone, PartialEq)]
enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32),
    Stream(Dict, Vec<u8>),
}

type Dict = HashMap<String, Obj>;

impl Obj {
    fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(dict) | Self::Stream(dict, _) => Some(dict),
            _ => None,
        }
    }

    fn as_name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    DictOpen,
    DictClose,
    ArrayOpen,
    ArrayClose,
    Keyword(String),
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
    /// Values built so far, for the file-wide [`MAX_NODES`] limit.
    nodes: usize,
    /// Values built for the object being read, for [`MAX_OBJECT_NODES`].
    object_nodes: usize,
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
            nodes: 0,
            object_nodes: 0,
        }
    }

    /// Whether the object being read already has [`MAX_OBJECT_NODES`] values.
    fn object_full(&self) -> bool {
        self.object_nodes >= MAX_OBJECT_NODES
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.data.len() {
            let byte = self.data[self.pos];
            if is_whitespace(byte) {
                self.pos += 1;
            } else if byte == b'%' {
                while self.pos < self.data.len() && !matches!(self.data[self.pos], b'\r' | b'\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Option<Token> {
        self.skip_whitespace();
        let byte = *self.data.get(self.pos)?;
        match byte {
            b'(' => Some(Token::Str(self.literal_string())),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                Some(Token::DictOpen)
            }
            b'<' => Some(Token::Str(self.hex_string())),
            b'>' if self.data.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                Some(Token::DictClose)
            }
            b'[' => {
                self.pos += 1;
                Some(Token::ArrayOpen)
            }
            b']' => {
                self.pos += 1;
                Some(Token::ArrayClose)
            }
            b'/' => {
                self.pos += 1;
                let word = self.regular_run();
                Some(Token::Name(decode_name(word)))
            }
            b'{' | b'}' | b')' | b'>' => {
                self.pos += 1;
                Some(Token::Keyword(char::from(byte).to_string()))
            }
            _ => {
                let word = self.regular_run();
                if word.is_empty() {
                    self.pos += 1;
                    return Some(Token::Keyword(String::new()));
                }
                let text = String::from_utf8_lossy(word);
                if word[0].is_ascii_digit() || matches!(word[0], b'+' | b'-' | b'.') {
                    if let Ok(value) = text.parse::<f64>() {
                        return Some(Token::Num(value));
                    }
                }
                Some(Token::Keyword(text.into_owned()))
            }
        }
    }

    fn regular_run(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.pos < self.data.len()
            && !is_whitespace(self.data[self.pos])
            && !is_delimiter(self.data[self.pos])
        {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn literal_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut depth = 1usize;
        let mut out = Vec::new();
        while self.pos < self.data.len() {
            let byte = self.data[self.pos];
            self.pos += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    out.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(byte);
                }
                b'\\' => {
                    let Some(&next) = self.data.get(self.pos) else {
                        break;
                    };
                    self.pos += 1;
                    match next {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut value = u32::from(next - b'0');
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&digit @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(digit - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(u8::try_from(value & 0xff).unwrap_or(0));
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(byte),
            }
        }
        out
    }

    fn hex_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|byte| *byte == b'>')
            .unwrap_or(rest.len());
        self.pos += (end + 1).min(rest.len());
        hex_decode(&rest[..end])
    }

    /// The object starting at the next token.
    fn object(&mut self) -> Option<Obj> {
        let token = self.next_token()?;
        self.object_from(token, 0)
    }

    /// The object that starts with `token`, nested `depth` containers deep.
    /// Past [`MAX_OBJECT_DEPTH`] a container is skipped without recursing
    /// and stands as `null`, so no input can exhaust the stack; past
    /// [`MAX_OBJECT_NODES`] values the rest of the object is skipped.
    fn object_from(&mut self, token: Token, depth: usize) -> Option<Obj> {
        if depth == 0 {
            self.object_nodes = 0;
        }
        self.nodes += 1;
        self.object_nodes += 1;
        if matches!(token, Token::DictOpen | Token::ArrayOpen)
            && (depth >= MAX_OBJECT_DEPTH || self.object_full())
        {
            self.skip_container();
            return Some(Obj::Null);
        }
        Some(match token {
            Token::Num(value) => {
                // `12 0 R` is a reference.
                let saved = self.pos;
                if value >= 0.0 && value.fract() == 0.0 {
                    if let Some(Token::Num(generation)) = self.next_token() {
                        if generation.fract() == 0.0
                            && matches!(self.next_token(), Some(Token::Keyword(ref word)) if word == "R")
                        {
                            return Some(Obj::Ref(to_u32(value)));
                        }
                    }
                }
                self.pos = saved;
                Obj::Num(value)
            }
            Token::Str(bytes) => Obj::Str(bytes),
            Token::Name(name) => Obj::Name(name),
            Token::DictOpen => {
                let mut dict = Dict::new();
                loop {
                    if self.object_full() {
                        self.skip_container();
                        break;
                    }
                    match self.next_token()? {
                        Token::DictClose => break,
                        Token::Name(key) => {
                            let next = self.next_token()?;
                            if next == Token::DictClose {
                                break;
                            }
                            if let Some(value) = self.object_from(next, depth + 1) {
                                dict.insert(key, value);
                            }
                        }
                        _ => {}
                    }
                }
                Obj::Dict(dict)
            }
            Token::ArrayOpen => {
                let mut items = Vec::new();
                loop {
                    if self.object_full() {
                        self.skip_container();
                        break;
                    }
                    match self.next_token()? {
                        Token::ArrayClose => break,
                        other => {
                            if let Some(item) = self.object_from(other, depth + 1) {
                                items.push(item);
                            }
                        }
                    }
                }
                Obj::Array(items)
            }
            Token::Keyword(word) => match word.as_str() {
                "true" => Obj::Bool(true),
                "false" => Obj::Bool(false),
                _ => Obj::Null,
            },
            Token::DictClose | Token::ArrayClose => Obj::Null,
        })
    }

    /// Skips to the end of the container just opened, counting brackets
    /// instead of recursing.
    fn skip_container(&mut self) {
        let mut open = 1usize;
        while let Some(token) = self.next_token() {
            match token {
                Token::ArrayOpen | Token::DictOpen => open += 1,
                Token::ArrayClose | Token::DictClose => {
                    open -= 1;
                    if open == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_u32(value: f64) -> u32 {
    value.clamp(0.0, f64::from(u32::MAX)) as u32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_usize(value: f64) -> usize {
    if value.is_finite() && value > 0.0 {
        value as usize
    } else {
        0
    }
}

/// Hex digits as bytes, skipping anything else; an odd last digit is padded.
fn hex_decode(data: &[u8]) -> Vec<u8> {
    let mut digits = data
        .iter()
        .filter_map(|byte| char::from(*byte).to_digit(16))
        .map(|value| u8::try_from(value).unwrap_or(0))
        .collect::<Vec<_>>();
    if digits.len() % 2 == 1 {
        digits.push(0);
    }
    digits
        .chunks(2)
        .map(|pair| pair[0] * 16 + pair[1])
        .collect()
}

/// `/A#20B` as `A B`.
fn decode_name(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'#' {
            let hex = raw.get(index + 1..index + 3).and_then(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            });
            if let Some(byte) = hex {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Finds the next `needle` at or after positions that only move forward,
/// remembering the last hit and where a search found nothing, so a scan of
/// the whole file stays linear however often it asks.
struct Finder {
    needle: &'static [u8],
    found: Option<usize>,
    none_from: usize,
}

impl Finder {
    fn new(needle: &'static [u8]) -> Self {
        Self {
            needle,
            found: None,
            none_from: usize::MAX,
        }
    }

    fn next(&mut self, data: &[u8], from: usize) -> Option<usize> {
        if let Some(found) = self.found.filter(|found| *found >= from) {
            return Some(found);
        }
        if from >= self.none_from || from > data.len() {
            return None;
        }
        if let Some(offset) = find(&data[from..], self.needle) {
            self.found = Some(from + offset);
            self.found
        } else {
            self.none_from = from;
            None
        }
    }
}

// ---------------------------------------------------------------------------
// The document

struct Pdf {
    objects: HashMap<u32, Obj>,
    trailer: Dict,
    /// Bytes still allowed to be decoded in this call.
    decode_left: Cell<u64>,
}

struct Page<'a> {
    contents: Vec<&'a Obj>,
    resources: Option<&'a Dict>,
}

/// What one call caches and tracks while running content streams.
#[derive(Default)]
struct Run {
    /// Fonts by the address of their dictionary, built once per call.
    fonts: HashMap<usize, Rc<Font>>,
    /// The fonts of each resource dictionary, by its address.
    resources: HashMap<usize, Rc<HashMap<String, Rc<Font>>>>,
    /// Encoding tables of simple fonts, by the address of their `/Encoding`
    /// object (or the place of a base encoding's name in [`glyphs::NAMED`]).
    encodings: HashMap<usize, Rc<CodeTable>>,
    /// Form `XObject`s being drawn, to skip one that draws itself.
    forms: Vec<u32>,
}

fn too_complex() -> String {
    "this PDF is too complex to read safely: it holds more objects than read_document follows"
        .to_string()
}

impl Pdf {
    #[allow(clippy::too_many_lines)]
    fn parse(data: &[u8]) -> Result<Self, String> {
        let mut objects = HashMap::new();
        // Where each object kept in `objects` is defined, and its generation.
        let mut defined: HashMap<u32, (usize, u16)> = HashMap::new();
        let mut cursor = 0;
        let mut nodes = 0usize;
        // Bytes lexed while looking for objects; a damaged file can make the
        // same bytes be read again from many starting points, so stop well
        // before that turns quadratic.
        let mut scanned = 0usize;
        let lex_limit = data.len().saturating_mul(8).saturating_add(1 << 20);
        let mut endstreams = Finder::new(b"endstream");
        // Where a stream of `length` bytes from `start` ends, when `endstream`
        // follows it: only an end of line belongs there, and a long run of
        // whitespace is not scanned again per header.
        let stream_end = |start: usize, length: usize| -> Option<usize> {
            let end = start.checked_add(length).filter(|end| *end <= data.len())?;
            let offset = data[end..]
                .iter()
                .take(MAX_STREAM_END_GAP)
                .position(|byte| !is_whitespace(*byte))?;
            data[end + offset..]
                .starts_with(b"endstream")
                .then_some(end)
        };
        // The data of every stream, in file order, so a `trailer` keyword
        // inside one (an embedded file) is not taken for the file's own.
        let mut stream_spans: Vec<(usize, usize)> = Vec::new();
        // Streams whose `/Length` is an indirect object, read once every
        // object is known: (stream number, where it is defined, data start,
        // length object number).
        let mut indirect_lengths: Vec<(u32, usize, usize, u32)> = Vec::new();
        // Streams whose end came from `endstream`, with line ends trimmed
        // before it: (where the stream is defined, data start, trimmed end,
        // `endstream`), by stream number.
        let mut loose_ends: HashMap<u32, (usize, usize, usize, usize)> = HashMap::new();
        while let Some(found) = find(&data[cursor..], b"obj") {
            let at = cursor + found;
            cursor = at + 3;
            if at >= 3 && &data[at - 3..at] == b"end" {
                continue;
            }
            if data
                .get(at + 3)
                .is_some_and(|byte| !is_whitespace(*byte) && !is_delimiter(*byte))
            {
                continue;
            }
            let Some((number, generation)) = object_number_before(data, at) else {
                continue;
            };
            if scanned > lex_limit {
                break;
            }
            let mut lexer = Lexer::new(data, at + 3);
            let object = lexer.object();
            scanned = scanned.saturating_add(lexer.pos - (at + 3));
            nodes = nodes.saturating_add(lexer.nodes);
            if nodes > MAX_NODES {
                return Err(too_complex());
            }
            let Some(object) = object else {
                continue;
            };
            match lexer.next_token() {
                Some(Token::Keyword(ref word)) if word == "stream" => {
                    let start = skip_eol(data, lexer.pos);
                    let length = object.as_dict().and_then(|dict| dict.get("Length"));
                    let declared = length.and_then(Obj::as_num).map(to_usize);
                    let end = match declared.and_then(|length| stream_end(start, length)) {
                        Some(end) => Some(end),
                        None => endstreams.next(data, start).map(|found| {
                            let mut end = found;
                            while end > start && matches!(data[end - 1], b'\r' | b'\n') {
                                end -= 1;
                            }
                            if end < found {
                                loose_ends.insert(number, (at, start, end, found));
                            }
                            end
                        }),
                    };
                    let Some(end) = end else {
                        continue;
                    };
                    // The end found by `endstream` drops trailing line ends,
                    // which may be data (encrypted bytes): an indirect
                    // `/Length` gives the exact end once it is read.
                    if let Some(Obj::Ref(length)) = length {
                        indirect_lengths.push((number, at, start, *length));
                    }
                    stream_spans.push((start, end));
                    let dict = match object {
                        Obj::Dict(dict) => dict,
                        _ => Dict::new(),
                    };
                    objects.insert(number, Obj::Stream(dict, data[start..end].to_vec()));
                    defined.insert(number, (at, generation));
                    cursor = end;
                }
                Some(Token::Keyword(ref word)) if word == "endobj" => {
                    objects.insert(number, object);
                    defined.insert(number, (at, generation));
                    // A well-formed object: anything inside it is not another.
                    cursor = lexer.pos;
                }
                _ => {
                    objects.insert(number, object);
                    defined.insert(number, (at, generation));
                }
            }
        }

        for (number, at, start, length) in indirect_lengths {
            if defined.get(&number).map(|(defined_at, _)| *defined_at) != Some(at) {
                continue;
            }
            let Some(end) = objects
                .get(&length)
                .and_then(Obj::as_num)
                .and_then(|length| stream_end(start, to_usize(length)))
            else {
                continue;
            };
            if let Some(Obj::Stream(_, bytes)) = objects.get_mut(&number) {
                *bytes = data[start..end].to_vec();
                loose_ends.remove(&number);
            }
        }

        let mut pdf = Self {
            objects,
            trailer: Dict::new(),
            decode_left: Cell::new(MAX_DECODED_BYTES),
        };

        // Every trailer in the file: classic `trailer` dictionaries (outside
        // stream data) and cross-reference streams, by where they are.
        let mut sections: Vec<(usize, Dict)> = Vec::new();
        let mut from = 0;
        while let Some(found) = find(&data[from..], b"trailer") {
            let at = from + found;
            from = at + b"trailer".len();
            let span = stream_spans.partition_point(|(start, _)| *start <= at);
            if span > 0 && stream_spans[span - 1].1 > at {
                continue;
            }
            let after = data[from..]
                .iter()
                .position(|byte| !is_whitespace(*byte))
                .map_or(data.len(), |offset| from + offset);
            if data.get(after) != Some(&b'<') || scanned > lex_limit {
                continue;
            }
            let mut lexer = Lexer::new(data, from);
            let object = lexer.object();
            scanned = scanned.saturating_add(lexer.pos.saturating_sub(from));
            nodes = nodes.saturating_add(lexer.nodes);
            if nodes > MAX_NODES {
                return Err(too_complex());
            }
            if let Some(Obj::Dict(dict)) = object {
                sections.push((at, dict));
            }
        }
        for (number, object) in &pdf.objects {
            if let Obj::Stream(dict, _) = object {
                if dict.get("Type").and_then(Obj::as_name) == Some("XRef") {
                    if let Some((at, _)) = defined.get(number) {
                        sections.push((*at, dict.clone()));
                    }
                }
            }
        }
        // The newest (last in the file) is the trailer; older ones fill in
        // what it leaves out. A linearized file's last trailer is often just
        // `<< /Size N >>`, with `/Root`, `/Encrypt` and `/ID` in the
        // first-page trailer near the start of the file.
        sections.sort_unstable_by_key(|(at, _)| Reverse(*at));
        let mut sections = sections.into_iter();
        let mut trailer = sections.next().map(|(_, dict)| dict).unwrap_or_default();
        for (_, older) in sections {
            for key in ["Root", "Encrypt", "ID", "Info"] {
                if !trailer.contains_key(key) {
                    if let Some(value) = older.get(key) {
                        trailer.insert(key.to_string(), value.clone());
                    }
                }
            }
        }

        if let Some(encrypt) = trailer.get("Encrypt") {
            let encrypt_number = match encrypt {
                Obj::Ref(number) => Some(*number),
                _ => None,
            };
            let security = pdf.security(encrypt, &trailer)?;
            if security.streams_use_aes() {
                pdf.restore_aes_tails(data, &loose_ends, &defined);
            }
            pdf.decrypt_all(&security, &defined, encrypt_number);
        }

        // Objects packed into object streams (PDF 1.5 and later), oldest
        // stream first: a stream written later (an incremental save) holds
        // newer definitions, and a packed object replaces a direct one only
        // when its stream comes after it in the file.
        let mut containers = pdf
            .objects
            .iter()
            .filter(|(_, object)| {
                matches!(object, Obj::Stream(dict, _)
                    if dict.get("Type").and_then(Obj::as_name) == Some("ObjStm"))
            })
            .map(|(number, _)| (defined.get(number).map_or(0, |(at, _)| *at), *number))
            .collect::<Vec<_>>();
        containers.sort_unstable();
        let mut unpacked = Vec::new();
        for (container_at, container) in containers {
            let Some(stream) = pdf.objects.get(&container).cloned() else {
                continue;
            };
            let Some(decoded) = pdf.decode(&stream) else {
                continue;
            };
            let Some(dict) = stream.as_dict() else {
                continue;
            };
            let count = dict
                .get("N")
                .and_then(Obj::as_num)
                .map_or(0, to_usize)
                .min(MAX_PACKED_OBJECTS);
            let first = dict.get("First").and_then(Obj::as_num).map_or(0, to_usize);
            if first > decoded.len() {
                continue;
            }
            // The header pairs object numbers with offsets. Offsets must rise,
            // and each object is read only up to the next one, so no byte of
            // the stream is parsed twice.
            let mut header = Lexer::new(&decoded[..first], 0);
            let mut entries: Vec<(u32, usize)> = Vec::new();
            for _ in 0..count {
                let (Some(Token::Num(number)), Some(Token::Num(offset))) =
                    (header.next_token(), header.next_token())
                else {
                    break;
                };
                let Some(start) = first.checked_add(to_usize(offset)) else {
                    break;
                };
                if start > decoded.len() || entries.last().is_some_and(|(_, last)| start <= *last) {
                    break;
                }
                entries.push((to_u32(number), start));
            }
            for (index, (number, start)) in entries.iter().enumerate() {
                let end = entries
                    .get(index + 1)
                    .map_or(decoded.len(), |(_, next)| *next);
                // A direct definition later in the file is newer.
                if *number == container
                    || defined
                        .get(number)
                        .is_some_and(|(direct_at, _)| *direct_at > container_at)
                {
                    continue;
                }
                let mut lexer = Lexer::new(&decoded[..end], *start);
                let object = lexer.object();
                nodes = nodes.saturating_add(lexer.nodes);
                if nodes > MAX_NODES {
                    return Err(too_complex());
                }
                if let Some(object) = object {
                    unpacked.push((*number, object));
                }
            }
        }
        // In file order, so the newest definition is the one kept.
        for (number, object) in unpacked {
            pdf.objects.insert(number, object);
        }
        if pdf.decoded_all() {
            return Err(format!(
                "this PDF unpacks to more than {} MB, more than read_document reads",
                MAX_DECODED_BYTES / (1024 * 1024)
            ));
        }
        pdf.trailer = trailer;
        Ok(pdf)
    }

    /// The security handler of an `/Encrypt` dictionary, unlocked with the
    /// empty user password; an error when the file needs a password or
    /// uses a handler this reader does not support.
    fn security(&self, encrypt: &Obj, trailer: &Dict) -> Result<Security, String> {
        let Some(dict) = self.resolve(encrypt).as_dict() else {
            return Err(pdf_crypt::unsupported());
        };
        if dict.get("Filter").and_then(Obj::as_name) != Some("Standard") {
            return Err(pdf_crypt::unsupported());
        }
        let number = |key: &str| self.get(dict, key).and_then(Obj::as_num);
        let bytes = |key: &str| match self.get(dict, key) {
            Some(Obj::Str(bytes)) => bytes.clone(),
            _ => Vec::new(),
        };
        let version = number("V").map_or(0, to_u32);
        let revision = number("R").map_or(0, to_u32);
        let id = match self.get(trailer, "ID") {
            Some(Obj::Array(ids)) => match ids.first().map(|first| self.resolve(first)) {
                Some(Obj::Str(id)) => id.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        // Version 4 and 5 name a crypt filter for strings and for streams.
        let filters = self.get(dict, "CF").and_then(Obj::as_dict);
        let method = |key: &str| -> Result<Method, String> {
            let name = self
                .get(dict, key)
                .and_then(Obj::as_name)
                .unwrap_or("Identity");
            if name == "Identity" {
                return Ok(Method::Identity);
            }
            let cfm = filters
                .and_then(|filters| self.get(filters, name))
                .and_then(Obj::as_dict)
                .and_then(|filter| self.get(filter, "CFM"))
                .and_then(Obj::as_name);
            Method::from_cfm(cfm).ok_or_else(pdf_crypt::unsupported)
        };
        let (strings, streams) = match version {
            1 | 2 => (Method::Rc4, Method::Rc4),
            4 | 5 => (method("StrF")?, method("StmF")?),
            _ => return Err(pdf_crypt::unsupported()),
        };
        #[allow(clippy::cast_possible_truncation)]
        let permissions = number("P").map_or(-1, |value| value as i64 as i32);
        // Version 4 may give the key length only in its crypt filter, in
        // bytes or bits (pdf.js reads values under 40 as bytes).
        let filter_length = || {
            let name = self.get(dict, "StmF").and_then(Obj::as_name)?;
            let length = filters
                .and_then(|filters| self.get(filters, name))
                .and_then(Obj::as_dict)
                .and_then(|filter| self.get(filter, "Length"))
                .and_then(Obj::as_num)
                .map(to_u32)?;
            Some(if length < 40 { length * 8 } else { length })
        };
        let length_bits =
            number("Length")
                .map(to_u32)
                .or_else(|| if version == 4 { filter_length() } else { None });
        let settings = Settings {
            version,
            revision,
            length_bits,
            owner: bytes("O"),
            user: bytes("U"),
            user_key: bytes("UE"),
            permissions,
            encrypt_metadata: !matches!(self.get(dict, "EncryptMetadata"), Some(Obj::Bool(false))),
            id,
            strings,
            streams,
        };
        Security::open(&settings)
    }

    /// Decrypts the strings and streams of every object written directly in
    /// the file, before object streams are unpacked (objects packed in a
    /// stream are not encrypted again). The `/Encrypt` dictionary and
    /// cross-reference streams are never encrypted.
    fn decrypt_all(
        &mut self,
        security: &Security,
        defined: &HashMap<u32, (usize, u16)>,
        encrypt_number: Option<u32>,
    ) {
        for (number, object) in &mut self.objects {
            if Some(*number) == encrypt_number {
                continue;
            }
            if let Obj::Stream(dict, _) = object {
                if dict.get("Type").and_then(Obj::as_name) == Some("XRef") {
                    continue;
                }
            }
            let generation = defined.get(number).map_or(0, |(_, generation)| *generation);
            decrypt_object(security, *number, generation, object, 0);
        }
    }

    /// Gives back the line-end bytes trimmed from the end of a stream whose
    /// length was not known (none given, or an indirect `/Length` packed in
    /// an object stream, which is unpacked only after decryption) when they
    /// complete its last AES block: AES data is always whole 16-byte blocks,
    /// and without them the last block, and the end of the text, is lost.
    fn restore_aes_tails(
        &mut self,
        data: &[u8],
        loose_ends: &HashMap<u32, (usize, usize, usize, usize)>,
        defined: &HashMap<u32, (usize, u16)>,
    ) {
        for (number, (at, start, end, endstream)) in loose_ends {
            if defined.get(number).map(|(defined_at, _)| defined_at) != Some(at) {
                continue;
            }
            let missing = (16 - (end - start) % 16) % 16;
            if missing == 0 || end + missing > *endstream {
                continue;
            }
            if let Some(Obj::Stream(_, bytes)) = self.objects.get_mut(number) {
                *bytes = data[*start..end + missing].to_vec();
            }
        }
    }

    /// Whether the decode budget for this call is used up.
    fn decoded_all(&self) -> bool {
        self.decode_left.get() == 0
    }

    /// The decoded bytes of a stream object, charged to this call's budget;
    /// `None` for filters this reader does not handle (images mostly) or once
    /// the budget is spent.
    fn decode(&self, object: &Obj) -> Option<Vec<u8>> {
        let Obj::Stream(dict, raw) = object else {
            return None;
        };
        let filters = match dict.get("Filter") {
            None => Vec::new(),
            Some(Obj::Name(name)) => vec![name.as_str()],
            Some(Obj::Array(names)) => names.iter().filter_map(Obj::as_name).collect(),
            Some(_) => return None,
        };
        let mut data = self.charge(raw.clone())?;
        for filter in filters {
            let decoded = match filter {
                "FlateDecode" | "Fl" => {
                    inflate(&data, MAX_STREAM_BYTES.min(self.decode_left.get()))?
                }
                "ASCIIHexDecode" | "AHx" => {
                    let end = data
                        .iter()
                        .position(|byte| *byte == b'>')
                        .unwrap_or(data.len());
                    hex_decode(&data[..end])
                }
                "ASCII85Decode" | "A85" => ascii85(&data),
                // The file's own decryption already applied.
                "Crypt" => continue,
                _ => return None,
            };
            data = self.charge(decoded)?;
        }
        Some(data)
    }

    /// Takes `data`'s length off the budget, cutting it to what is left.
    fn charge(&self, mut data: Vec<u8>) -> Option<Vec<u8>> {
        let left = self.decode_left.get();
        if left == 0 {
            return None;
        }
        let allowed = usize::try_from(left).unwrap_or(usize::MAX);
        if data.len() > allowed {
            data.truncate(allowed);
        }
        self.decode_left.set(left - data.len() as u64);
        Some(data)
    }

    fn resolve<'a>(&'a self, mut object: &'a Obj) -> &'a Obj {
        for _ in 0..MAX_DEPTH {
            match object {
                Obj::Ref(number) => match self.objects.get(number) {
                    Some(target) => object = target,
                    None => return &Obj::Null,
                },
                _ => return object,
            }
        }
        &Obj::Null
    }

    fn get<'a>(&'a self, dict: &'a Dict, key: &str) -> Option<&'a Obj> {
        dict.get(key).map(|object| self.resolve(object))
    }

    fn catalog(&self) -> Option<&Dict> {
        if let Some(root) = self.trailer.get("Root") {
            if let Some(dict) = self.resolve(root).as_dict() {
                return Some(dict);
            }
        }
        self.objects.values().find_map(|object| {
            object
                .as_dict()
                .filter(|dict| dict.get("Type").and_then(Obj::as_name) == Some("Catalog"))
        })
    }

    fn pages(&self) -> Vec<Page<'_>> {
        let mut pages = Vec::new();
        let Some(root) = self.catalog().and_then(|catalog| catalog.get("Pages")) else {
            return pages;
        };
        let mut seen = HashSet::new();
        self.collect_pages(root, None, &mut pages, &mut seen, 0);
        pages
    }

    /// Walks the page tree. Every node and every `Kids` array is visited
    /// once, known by its address (whether it is referenced or written
    /// inline), and at most [`MAX_PAGE_TREE_NODES`] nodes are visited, so a
    /// tree whose nodes share their kids cannot fan out exponentially.
    fn collect_pages<'a>(
        &'a self,
        node: &'a Obj,
        inherited: Option<&'a Dict>,
        pages: &mut Vec<Page<'a>>,
        seen: &mut HashSet<usize>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH || seen.len() >= MAX_PAGE_TREE_NODES {
            return;
        }
        let Some(dict) = self.resolve(node).as_dict() else {
            return;
        };
        if !seen.insert(std::ptr::from_ref(dict) as usize) {
            return;
        }
        let resources = self
            .get(dict, "Resources")
            .and_then(Obj::as_dict)
            .or(inherited);
        let kind = dict.get("Type").and_then(Obj::as_name);
        if let (Some(Obj::Array(kids)), false) = (self.get(dict, "Kids"), kind == Some("Page")) {
            if !seen.insert(std::ptr::from_ref(kids) as usize) {
                return;
            }
            for kid in kids {
                self.collect_pages(kid, resources, pages, seen, depth + 1);
            }
            return;
        }
        let contents = match dict.get("Contents") {
            Some(contents) => match self.resolve(contents) {
                Obj::Array(items) => items.iter().collect(),
                _ => vec![contents],
            },
            None => Vec::new(),
        };
        pages.push(Page {
            contents,
            resources,
        });
    }

    fn stream_data(&self, object: &Obj) -> Option<Vec<u8>> {
        self.decode(self.resolve(object))
    }

    /// The fonts of a resource dictionary, built once per dictionary and
    /// once per font for the whole call.
    fn fonts(&self, resources: Option<&Dict>, run: &mut Run) -> Rc<HashMap<String, Rc<Font>>> {
        let key = resources.map_or(0, |resources| std::ptr::from_ref(resources) as usize);
        if let Some(fonts) = run.resources.get(&key) {
            return Rc::clone(fonts);
        }
        let mut fonts = HashMap::new();
        if let Some(font_dict) = resources
            .and_then(|resources| self.get(resources, "Font"))
            .and_then(Obj::as_dict)
        {
            for (name, font) in font_dict {
                if let Some(font) = self.resolve(font).as_dict() {
                    let font_key = std::ptr::from_ref(font) as usize;
                    let built = if let Some(built) = run.fonts.get(&font_key) {
                        Rc::clone(built)
                    } else {
                        let built = Rc::new(self.font(font, &mut run.encodings));
                        run.fonts.insert(font_key, Rc::clone(&built));
                        built
                    };
                    fonts.insert(name.clone(), built);
                }
            }
        }
        let fonts = Rc::new(fonts);
        run.resources.insert(key, Rc::clone(&fonts));
        fonts
    }

    /// The font `dict` describes; `encodings` holds the encoding tables
    /// built so far, by the address of their `/Encoding` object.
    fn font(&self, dict: &Dict, encodings: &mut HashMap<usize, Rc<CodeTable>>) -> Font {
        let composite = dict.get("Subtype").and_then(Obj::as_name) == Some("Type0");
        let cmap = self
            .get(dict, "ToUnicode")
            .and_then(|object| self.decode(object))
            .map(|data| CMap::parse(&data))
            .filter(|cmap| !cmap.is_empty());
        let mut widths = Widths::default();
        let mut default_width = if composite { 1000.0 } else { 500.0 };
        if composite {
            if let Some(Obj::Array(descendants)) = self.get(dict, "DescendantFonts") {
                if let Some(descendant) = descendants
                    .first()
                    .and_then(|first| self.resolve(first).as_dict())
                {
                    if let Some(width) = self.get(descendant, "DW").and_then(Obj::as_num) {
                        default_width = width;
                    }
                    if let Some(Obj::Array(entries)) = self.get(descendant, "W") {
                        self.cid_widths(entries, &mut widths);
                    }
                }
            }
        } else {
            let first = self
                .get(dict, "FirstChar")
                .and_then(Obj::as_num)
                .map_or(0, to_u32);
            if let Some(Obj::Array(entries)) = self.get(dict, "Widths") {
                for (offset, width) in entries.iter().take(MAX_FONT_CODES).enumerate() {
                    if let Some(width) = self.resolve(width).as_num() {
                        widths.codes.insert(
                            first.saturating_add(u32::try_from(offset).unwrap_or(0)),
                            width,
                        );
                    }
                }
            }
        }
        widths.ranges.sort_by_key(|range| range.0);
        // A predefined Unicode CMap (`UniGB-UCS2-H`, `UniJIS-UTF16-H`, ...)
        // makes each code its own UTF-16 text; such fonts (iText's standard
        // CJK fonts) often come without a ToUnicode map.
        let unicode_codes = composite
            && cmap.is_none()
            && self
                .get(dict, "Encoding")
                .and_then(Obj::as_name)
                .is_some_and(|name| {
                    name.starts_with("Uni") && (name.contains("-UCS2-") || name.contains("-UTF16-"))
                });
        Font {
            two_byte: composite,
            unicode_codes,
            cmap,
            encoding: if composite {
                None
            } else {
                self.simple_encoding(dict, encodings)
            },
            widths,
            default_width,
        }
    }

    /// What the codes of a simple font mean by its `/Encoding`: a named base
    /// encoding, or a dictionary of one and its `/Differences`. `None` reads
    /// codes as Windows-1252. Fonts sharing one `/Encoding` object, or
    /// naming the same base encoding, share one table.
    fn simple_encoding(
        &self,
        dict: &Dict,
        encodings: &mut HashMap<usize, Rc<CodeTable>>,
    ) -> Option<Rc<CodeTable>> {
        let encoding = self.get(dict, "Encoding")?;
        let key = match encoding.as_name() {
            // The few named tables, by their place in this list.
            Some(name) => glyphs::NAMED.iter().position(|known| *known == name)?,
            None => std::ptr::from_ref(encoding) as usize,
        };
        if let Some(table) = encodings.get(&key) {
            return Some(Rc::clone(table));
        }
        let table = Rc::new(self.encoding_table(encoding)?);
        encodings.insert(key, Rc::clone(&table));
        Some(table)
    }

    /// The table of one `/Encoding` object.
    fn encoding_table(&self, encoding: &Obj) -> Option<CodeTable> {
        if let Some(name) = encoding.as_name() {
            return glyphs::named(name);
        }
        let encoding = encoding.as_dict()?;
        let mut table = self
            .get(encoding, "BaseEncoding")
            .and_then(Obj::as_name)
            .and_then(glyphs::named)
            .unwrap_or_else(glyphs::default_table);
        if let Some(Obj::Array(differences)) = self.get(encoding, "Differences") {
            // `code /name1 /name2 ...`: names for consecutive codes.
            let mut code: Option<usize> = None;
            // 256 names and their codes fit many times over.
            for entry in differences.iter().take(4_096) {
                match self.resolve(entry) {
                    Obj::Num(number) => code = Some(to_usize(*number)),
                    Obj::Name(glyph) => {
                        if let Some(at) = code {
                            // Names past the last code are not read at all.
                            if let Some(slot) = table.get_mut(at) {
                                if let Some(text) = glyphs::glyph_text(glyph) {
                                    *slot = text;
                                }
                            }
                            code = Some(at.saturating_add(1));
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(table)
    }

    /// The `W` array of a CID font: `c [w1 w2 ...]` and `c_first c_last w`.
    /// Ranges are kept as ranges, not one entry per code.
    fn cid_widths(&self, entries: &[Obj], widths: &mut Widths) {
        let mut index = 0;
        let mut defined = 0usize;
        while index < entries.len() && defined < MAX_FONT_CODES {
            let Some(first) = self.resolve(&entries[index]).as_num().map(to_u32) else {
                break;
            };
            match entries.get(index + 1).map(|entry| self.resolve(entry)) {
                Some(Obj::Array(list)) => {
                    for (offset, width) in list.iter().enumerate() {
                        if let Some(width) = self.resolve(width).as_num() {
                            widths.codes.insert(
                                first.saturating_add(u32::try_from(offset).unwrap_or(0)),
                                width,
                            );
                        }
                    }
                    defined += list.len().max(1);
                    index += 2;
                }
                Some(Obj::Num(last)) => {
                    if let Some(width) = entries
                        .get(index + 2)
                        .and_then(|width| self.resolve(width).as_num())
                    {
                        widths.ranges.push((first, to_u32(*last).max(first), width));
                    }
                    defined += 1;
                    index += 3;
                }
                _ => break,
            }
        }
    }

    /// Runs a content stream, writing its text to `sink`. `ctm` is the
    /// transformation in force when the stream starts.
    #[allow(clippy::too_many_lines)]
    fn run_content(
        &self,
        data: &[u8],
        resources: Option<&Dict>,
        sink: &mut TextSink,
        run: &mut Run,
        depth: usize,
        ctm: Matrix,
    ) {
        let fonts = self.fonts(resources, run);
        let mut state = TextState {
            ctm,
            ..TextState::default()
        };
        let mut saved: Vec<Matrix> = Vec::new();
        let mut lexer = Lexer::new(data, 0);
        let mut operands: Vec<Obj> = Vec::new();
        while let Some(token) = lexer.next_token() {
            if sink.cut {
                return;
            }
            let Token::Keyword(operator) = token else {
                if operands.len() >= MAX_OPERANDS {
                    // No operator takes this many; keep only the latest.
                    operands.drain(..MAX_OPERANDS / 2);
                }
                if let Some(operand) = lexer.object_from(token, 0) {
                    operands.push(operand);
                }
                continue;
            };
            let number = |index: usize| operands.get(index).and_then(Obj::as_num).unwrap_or(0.0);
            match operator.as_str() {
                "q" => {
                    if saved.len() < MAX_SAVED_STATES {
                        saved.push(state.ctm);
                    }
                }
                "Q" => {
                    if let Some(ctm) = saved.pop() {
                        state.ctm = ctm;
                    }
                }
                "cm" => {
                    let matrix = [
                        number(0),
                        number(1),
                        number(2),
                        number(3),
                        number(4),
                        number(5),
                    ];
                    state.ctm = multiply(&matrix, &state.ctm);
                }
                "BT" => state.begin_text(),
                "Tf" => {
                    state.font = operands.first().and_then(Obj::as_name).map(str::to_string);
                    state.font_size = number(1);
                }
                "TL" => state.leading = number(0),
                "Td" => state.move_line(number(0), number(1)),
                "TD" => {
                    state.leading = -number(1);
                    state.move_line(number(0), number(1));
                }
                "T*" => state.next_line(),
                "Tm" => state.set_matrix([
                    number(0),
                    number(1),
                    number(2),
                    number(3),
                    number(4),
                    number(5),
                ]),
                "Tj" => {
                    if let Some(Obj::Str(bytes)) = operands.last() {
                        let font = fonts.get(state.font.as_deref().unwrap_or_default());
                        state.show(bytes, font.map(AsRef::as_ref), sink);
                    }
                }
                "'" | "\"" => {
                    state.next_line();
                    if let Some(Obj::Str(bytes)) = operands.last() {
                        let font = fonts.get(state.font.as_deref().unwrap_or_default());
                        state.show(bytes, font.map(AsRef::as_ref), sink);
                    }
                }
                "TJ" => {
                    if let Some(Obj::Array(items)) = operands.last() {
                        let font = fonts
                            .get(state.font.as_deref().unwrap_or_default())
                            .map(AsRef::as_ref);
                        for item in items {
                            match item {
                                Obj::Str(bytes) => state.show(bytes, font, sink),
                                Obj::Num(adjust) => state.adjust(*adjust, sink),
                                _ => {}
                            }
                        }
                    }
                }
                "Do" if depth < MAX_FORM_DEPTH => {
                    self.draw_form(&operands, resources, sink, run, depth, state.ctm);
                }
                "BI" => skip_inline_image(&mut lexer),
                _ => {}
            }
            operands.clear();
        }
    }

    /// `/Name Do` for a form `XObject`: runs its content with its own
    /// resources and matrix. A form already being drawn is skipped, so a
    /// form that draws itself ends at once; the decode budget bounds the
    /// rest.
    fn draw_form(
        &self,
        operands: &[Obj],
        resources: Option<&Dict>,
        sink: &mut TextSink,
        run: &mut Run,
        depth: usize,
        ctm: Matrix,
    ) {
        let Some(entry) = operands.first().and_then(Obj::as_name).and_then(|name| {
            resources
                .and_then(|resources| self.get(resources, "XObject"))
                .and_then(Obj::as_dict)
                .and_then(|xobjects| xobjects.get(name))
        }) else {
            return;
        };
        let number = match entry {
            Obj::Ref(number) => Some(*number),
            _ => None,
        };
        if number.is_some_and(|number| run.forms.contains(&number)) {
            return;
        }
        let form = self.resolve(entry);
        let Obj::Stream(dict, _) = form else {
            return;
        };
        if dict.get("Subtype").and_then(Obj::as_name) != Some("Form") {
            return;
        }
        let Some(content) = self.decode(form) else {
            return;
        };
        let inner = self
            .get(dict, "Resources")
            .and_then(Obj::as_dict)
            .or(resources);
        let matrix = match self.get(dict, "Matrix") {
            Some(Obj::Array(values)) if values.len() == 6 => {
                let mut matrix = IDENTITY;
                for (slot, value) in matrix.iter_mut().zip(values) {
                    *slot = self.resolve(value).as_num().unwrap_or(0.0);
                }
                matrix
            }
            _ => IDENTITY,
        };
        if let Some(number) = number {
            run.forms.push(number);
        }
        sink.newline();
        self.run_content(
            &content,
            inner,
            sink,
            run,
            depth + 1,
            multiply(&matrix, &ctm),
        );
        sink.newline();
        if number.is_some() {
            run.forms.pop();
        }
    }
}

/// Decrypts every string inside `object`, and a stream's data.
fn decrypt_object(
    security: &Security,
    number: u32,
    generation: u16,
    object: &mut Obj,
    depth: usize,
) {
    if depth > MAX_OBJECT_DEPTH {
        return;
    }
    match object {
        Obj::Str(bytes) => *bytes = security.decrypt(number, generation, bytes, false),
        Obj::Array(items) => {
            for item in items {
                decrypt_object(security, number, generation, item, depth + 1);
            }
        }
        Obj::Dict(dict) => {
            for value in dict.values_mut() {
                decrypt_object(security, number, generation, value, depth + 1);
            }
        }
        Obj::Stream(dict, data) => {
            for value in dict.values_mut() {
                decrypt_object(security, number, generation, value, depth + 1);
            }
            *data = security.decrypt(number, generation, data, true);
        }
        _ => {}
    }
}

/// The object number and generation written before `obj` at `at`.
fn object_number_before(data: &[u8], at: usize) -> Option<(u32, u16)> {
    let mut index = at;
    let skip_space = |index: &mut usize| {
        let start = *index;
        while *index > 0 && is_whitespace(data[*index - 1]) {
            *index -= 1;
        }
        *index < start
    };
    let digits = |index: &mut usize| {
        let end = *index;
        while *index > 0 && data[*index - 1].is_ascii_digit() {
            *index -= 1;
        }
        (*index < end).then_some(*index..end)
    };
    if !skip_space(&mut index) {
        return None;
    }
    let generation = digits(&mut index)?;
    if !skip_space(&mut index) {
        return None;
    }
    let number = digits(&mut index)?;
    if index > 0 && !is_whitespace(data[index - 1]) && !is_delimiter(data[index - 1]) {
        return None;
    }
    let generation = std::str::from_utf8(&data[generation])
        .ok()?
        .parse()
        .unwrap_or(0);
    Some((
        std::str::from_utf8(&data[number]).ok()?.parse().ok()?,
        generation,
    ))
}

fn skip_eol(data: &[u8], mut pos: usize) -> usize {
    while pos < data.len() && matches!(data[pos], b' ' | b'\t') {
        pos += 1;
    }
    if data.get(pos) == Some(&b'\r') {
        pos += 1;
    }
    if data.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    pos
}

/// Skips inline image data, from after `BI` to after its `EI`.
fn skip_inline_image(lexer: &mut Lexer<'_>) {
    let data = lexer.data;
    let Some(id) = find(&data[lexer.pos..], b"ID") else {
        lexer.pos = data.len();
        return;
    };
    let mut pos = lexer.pos + id + 2;
    while let Some(offset) = find(&data[pos..], b"EI") {
        let at = pos + offset;
        let before = at == 0 || is_whitespace(data[at - 1]);
        let after = data.get(at + 2).is_none_or(|byte| is_whitespace(*byte));
        if before && after {
            lexer.pos = at + 2;
            return;
        }
        pos = at + 2;
    }
    lexer.pos = data.len();
}

/// Inflates zlib (or raw deflate) data, up to `limit` bytes.
fn inflate(data: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let zlib = flate2::read::ZlibDecoder::new(data)
        .take(limit)
        .read_to_end(&mut out);
    if zlib.is_ok() || !out.is_empty() {
        return Some(out);
    }
    out.clear();
    flate2::read::DeflateDecoder::new(data)
        .take(limit)
        .read_to_end(&mut out)
        .ok()
        .map(|_| out)
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = Vec::with_capacity(5);
    for &byte in data {
        match byte {
            b'~' => break,
            b'z' if group.is_empty() => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group.push(u32::from(byte - b'!'));
                if group.len() == 5 {
                    let value = group
                        .iter()
                        .fold(0u32, |acc, digit| acc.wrapping_mul(85).wrapping_add(*digit));
                    out.extend_from_slice(&value.to_be_bytes());
                    group.clear();
                }
            }
            _ => {}
        }
    }
    if group.len() > 1 {
        let kept = group.len() - 1;
        group.resize(5, 84);
        let value = group
            .iter()
            .fold(0u32, |acc, digit| acc.wrapping_mul(85).wrapping_add(*digit));
        out.extend_from_slice(&value.to_be_bytes()[..kept]);
    }
    out
}

// ---------------------------------------------------------------------------
// Fonts and text

/// A `ToUnicode` map: character code to text. `bfrange` entries that count
/// up from a start text stay ranges and are decoded on lookup, so a few
/// lines cannot expand into billions of entries.
#[derive(Debug, Default)]
struct CMap {
    code_bytes: usize,
    map: HashMap<u32, String>,
    /// `(low, high, UTF-16 text of low)`, sorted by `low`.
    ranges: Vec<(u32, u32, Vec<u16>)>,
}

impl CMap {
    fn parse(data: &[u8]) -> Self {
        let mut cmap = Self::default();
        let mut lexer = Lexer::new(data, 0);
        let mut pending: Vec<Obj> = Vec::new();
        let mut defined = 0usize;
        while let Some(token) = lexer.next_token() {
            if defined >= MAX_FONT_CODES {
                break;
            }
            match token {
                Token::Keyword(word) => {
                    match word.as_str() {
                        "endcodespacerange" => {
                            if let Some(Obj::Str(low)) = pending.first() {
                                cmap.code_bytes = low.len();
                            }
                        }
                        "endbfchar" => {
                            for pair in pending.chunks(2) {
                                if let [Obj::Str(code), Obj::Str(text)] = pair {
                                    cmap.note_width(code.len());
                                    cmap.map.insert(code_value(code), mapped_text(text));
                                    defined += 1;
                                }
                            }
                        }
                        "endbfrange" => {
                            for triple in pending.chunks(3) {
                                if let [Obj::Str(low), Obj::Str(high), target] = triple {
                                    cmap.note_width(low.len());
                                    defined += cmap.insert_range(
                                        code_value(low),
                                        code_value(high),
                                        target,
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                    pending.clear();
                }
                other => {
                    if let Some(object) = lexer.object_from(other, 0) {
                        if pending.len() < MAX_PENDING {
                            pending.push(object);
                        }
                    }
                }
            }
        }
        cmap.ranges.sort_by_key(|range| range.0);
        if cmap.code_bytes == 0 {
            cmap.code_bytes = 1;
        }
        cmap
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty() && self.ranges.is_empty()
    }

    fn note_width(&mut self, width: usize) {
        if self.code_bytes == 0 {
            self.code_bytes = width.clamp(1, 4);
        }
    }

    /// Adds one `bfrange` entry; returns how many codes it defined, counted
    /// against [`MAX_FONT_CODES`].
    fn insert_range(&mut self, low: u32, high: u32, target: &Obj) -> usize {
        if high < low {
            return 0;
        }
        match target {
            Obj::Str(start) => {
                let units = start
                    .chunks(2)
                    .take(MAX_MAPPED_UNITS)
                    .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
                    .collect::<Vec<_>>();
                self.ranges.push((low, high, units));
                1
            }
            Obj::Array(texts) => {
                let mut count = 0;
                for (code, text) in (low..=high).zip(texts) {
                    if let Obj::Str(text) = text {
                        self.map.insert(code, mapped_text(text));
                        count += 1;
                    }
                }
                count
            }
            _ => 0,
        }
    }

    fn get(&self, code: u32) -> Option<String> {
        if let Some(text) = self.map.get(&code) {
            return Some(text.clone());
        }
        let index = self.ranges.partition_point(|range| range.0 <= code);
        let (low, high, start) = self.ranges.get(index.checked_sub(1)?)?;
        if code > *high {
            return None;
        }
        let mut units = start.clone();
        if let Some(last) = units.last_mut() {
            // The offset fits: a range spans at most u32::MAX codes, and
            // only the low 16 bits move the last unit.
            *last = last.wrapping_add(u16::try_from((code - low) & 0xffff).unwrap_or(0));
        }
        Some(String::from_utf16_lossy(&units))
    }
}

fn code_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |acc, byte| (acc << 8) | u32::from(*byte))
}

/// The text a `ToUnicode` entry maps to (UTF-16BE), at most
/// [`MAX_MAPPED_UNITS`] units: one code never stands for more.
fn mapped_text(bytes: &[u8]) -> String {
    let units = bytes
        .chunks(2)
        .take(MAX_MAPPED_UNITS)
        .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

/// Glyph widths: single codes, plus CID ranges sorted by their first code.
#[derive(Debug, Default)]
struct Widths {
    codes: HashMap<u32, f64>,
    ranges: Vec<(u32, u32, f64)>,
}

impl Widths {
    fn get(&self, code: u32) -> Option<f64> {
        if let Some(width) = self.codes.get(&code) {
            return Some(*width);
        }
        let index = self.ranges.partition_point(|range| range.0 <= code);
        let (_, high, width) = self.ranges.get(index.checked_sub(1)?)?;
        (code <= *high).then_some(*width)
    }
}

#[derive(Debug, Default)]
struct Font {
    two_byte: bool,
    /// A composite font whose codes are UTF-16 text (a predefined Unicode
    /// `CMap`), read when it has no `ToUnicode` map.
    unicode_codes: bool,
    cmap: Option<CMap>,
    /// What a simple font's codes mean when `cmap` does not say: its
    /// `/Encoding`, or Windows-1252 when `None`.
    encoding: Option<Rc<CodeTable>>,
    widths: Widths,
    default_width: f64,
}

impl Font {
    /// The text of `bytes` and its width in thousandths of the font size.
    /// `None` text means the codes have no Unicode meaning to recover. Text
    /// stops growing past `limit` bytes; the flag says it was cut.
    fn decode(&self, bytes: &[u8], limit: usize) -> (Option<String>, f64, bool) {
        // A simple font's codes are one byte each, whatever codespace its
        // ToUnicode map declares (some writers give it <0000> <FFFF>).
        let code_bytes = if self.two_byte {
            self.cmap.as_ref().map_or(2, |cmap| cmap.code_bytes)
        } else {
            1
        };
        let mut text = String::new();
        let mut width = 0.0;
        let mut decodable = true;
        let mut cut = false;
        // The first half of a UTF-16 surrogate pair, waiting for the second.
        let mut high_surrogate: Option<u16> = None;
        for chunk in bytes.chunks(code_bytes.max(1)) {
            let code = code_value(chunk);
            width += self.widths.get(code).unwrap_or(self.default_width);
            if text.len() > limit {
                cut = true;
                continue;
            }
            match &self.cmap {
                Some(cmap) => match cmap.get(code) {
                    Some(mapped) => text.push_str(&mapped),
                    None if !self.two_byte => {
                        self.push_simple(&mut text, u8::try_from(code).unwrap_or(b'?'));
                    }
                    None => {}
                },
                None if self.unicode_codes => {
                    let unit = u16::try_from(code & 0xffff).unwrap_or(0);
                    let pair = match high_surrogate.take() {
                        Some(high) => vec![high, unit],
                        None if (0xD800..0xDC00).contains(&unit) => {
                            high_surrogate = Some(unit);
                            continue;
                        }
                        None => vec![unit],
                    };
                    text.extend(
                        char::decode_utf16(pair)
                            .map(|ch| ch.unwrap_or(char::REPLACEMENT_CHARACTER)),
                    );
                }
                None if self.two_byte => decodable = false,
                None => {
                    for byte in chunk {
                        self.push_simple(&mut text, *byte);
                    }
                }
            }
        }
        (decodable.then_some(text), width, cut)
    }

    /// The text of one code of a simple font, by its encoding.
    fn push_simple(&self, text: &mut String, byte: u8) {
        match self
            .encoding
            .as_ref()
            .and_then(|table| table.get(usize::from(byte)))
        {
            Some(mapped) => text.push_str(mapped),
            None => text.push(win_ansi(byte)),
        }
    }
}

/// An affine matrix `[a b c d e f]` as PDF writes them.
type Matrix = [f64; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `first` then `second`: the matrix PDF's `cm` makes from `first` (the
/// operand) and `second` (the current transformation).
fn multiply(first: &Matrix, second: &Matrix) -> Matrix {
    let [a1, b1, c1, d1, e1, f1] = *first;
    let [a2, b2, c2, d2, e2, f2] = *second;
    [
        a1 * a2 + b1 * c2,
        a1 * b2 + b1 * d2,
        c1 * a2 + d1 * c2,
        c1 * b2 + d1 * d2,
        e1 * a2 + f1 * c2 + e2,
        e1 * b2 + f1 * d2 + f2,
    ]
}

fn apply(matrix: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (
        x * matrix[0] + y * matrix[2] + matrix[4],
        x * matrix[1] + y * matrix[3] + matrix[5],
    )
}

/// Where text goes on the page, enough to tell line breaks and word gaps.
/// Positions are compared on the page (after the current transformation),
/// so text placed with `cm` (a common way to lay out table cells) still
/// gets its spaces and line breaks.
#[derive(Debug)]
struct TextState {
    font: Option<String>,
    font_size: f64,
    leading: f64,
    /// Text line matrix: scale, rotation and origin.
    line: [f64; 6],
    /// How far text has advanced along the line since its start, in text
    /// space (the text matrix is the line matrix moved this far along x).
    advance: f64,
    /// The current transformation matrix.
    ctm: Matrix,
    /// The page point where the last text ended.
    last: Option<(f64, f64)>,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            font_size: 12.0,
            leading: 0.0,
            line: IDENTITY,
            advance: 0.0,
            ctm: IDENTITY,
            last: None,
        }
    }
}

impl TextState {
    fn begin_text(&mut self) {
        self.line = IDENTITY;
        self.advance = 0.0;
    }

    fn scale(matrix: &Matrix) -> f64 {
        let scale = matrix[0].hypot(matrix[1]);
        if scale > 0.0 && scale.is_finite() {
            scale
        } else {
            1.0
        }
    }

    fn move_line(&mut self, tx: f64, ty: f64) {
        let [scale_x, skew_y, skew_x, scale_y, origin_x, origin_y] = self.line;
        self.line[4] = origin_x + tx * scale_x + ty * skew_x;
        self.line[5] = origin_y + tx * skew_y + ty * scale_y;
        self.advance = 0.0;
    }

    fn next_line(&mut self) {
        let leading = if self.leading == 0.0 {
            self.font_size.abs().max(1.0) * 1.2
        } else {
            self.leading
        };
        self.move_line(0.0, -leading);
    }

    fn set_matrix(&mut self, matrix: [f64; 6]) {
        self.line = matrix;
        self.advance = 0.0;
    }

    /// One em in user space.
    fn em(&self) -> f64 {
        self.font_size.abs().max(0.1) * Self::scale(&self.line)
    }

    /// The current text position on the page.
    fn page_point(&self) -> (f64, f64) {
        let (x, y) = apply(&self.line, self.advance, 0.0);
        apply(&self.ctm, x, y)
    }

    /// The unit direction text runs in on the page, which a rotated text
    /// matrix or transformation turns.
    fn direction(&self) -> (f64, f64) {
        let (start_x, start_y) = apply(&self.ctm, self.line[4], self.line[5]);
        let (end_x, end_y) = {
            let (x, y) = apply(&self.line, 1.0, 0.0);
            apply(&self.ctm, x, y)
        };
        let (dx, dy) = (end_x - start_x, end_y - start_y);
        let length = dx.hypot(dy);
        if length > 0.0 && length.is_finite() {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        }
    }

    fn show(&mut self, bytes: &[u8], font: Option<&Font>, sink: &mut TextSink) {
        let fallback = Font {
            default_width: 500.0,
            ..Font::default()
        };
        let font = font.unwrap_or(&fallback);
        let room = MAX_PAGE_TEXT.saturating_sub(sink.text.len());
        let (text, width, cut) = font.decode(bytes, room);
        sink.cut |= cut;
        let page_em = self.em() * Self::scale(&self.ctm);
        let (x, y) = self.page_point();
        if let Some((last_x, last_y)) = self.last {
            // Measured along the text's direction and across it, so text
            // set in a rotated matrix breaks lines and words as it reads.
            let (along_x, along_y) = self.direction();
            let (dx, dy) = (x - last_x, y - last_y);
            let along = dx * along_x + dy * along_y;
            let across = dy * along_x - dx * along_y;
            if across.abs() > page_em * 0.5 {
                sink.newline();
            } else if along > page_em * 0.15 {
                sink.space();
            }
        }
        match text {
            Some(text) => sink.push(&text),
            None => sink.undecodable = true,
        }
        self.advance += width / 1000.0 * self.font_size.abs().max(0.1);
        self.last = Some(self.page_point());
    }

    /// A `TJ` number: moves left by `adjust` thousandths of an em.
    fn adjust(&mut self, adjust: f64, sink: &mut TextSink) {
        self.advance -= adjust / 1000.0 * self.font_size.abs().max(0.1);
        if adjust < -180.0 {
            sink.space();
            let point = self.page_point();
            if let Some(last) = self.last.as_mut() {
                *last = point;
            }
        }
    }
}

#[derive(Debug, Default)]
struct TextSink {
    text: String,
    undecodable: bool,
    /// The page held more text than [`MAX_PAGE_TEXT`]; the rest is dropped.
    cut: bool,
}

impl TextSink {
    fn push(&mut self, text: &str) {
        if self.text.len() + text.len() > MAX_PAGE_TEXT {
            self.cut = true;
            return;
        }
        self.text.push_str(text);
    }

    fn space(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with(char::is_whitespace) {
            self.text.push(' ');
        }
    }

    fn newline(&mut self) {
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn finish(self) -> String {
        let mut out = String::with_capacity(self.text.len());
        let mut blank_run = 0;
        for line in self.text.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run > 1 {
                    continue;
                }
            } else {
                blank_run = 0;
            }
            out.push_str(line);
            out.push('\n');
        }
        out.trim().to_string()
    }
}

#[cfg(test)]
pub(super) mod fixtures {
    //! Small PDFs built in memory for the tests.

    use std::fmt::Write as _;
    use std::io::Write as _;

    /// A PDF from `objects` (numbered from 1; object 1 is the catalog) with a
    /// classic cross-reference table and `trailer_extra` added to the trailer.
    pub(crate) fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in offsets {
            let _ = writeln!(table, "{offset:010} 00000 n ");
        }
        let _ = write!(
            table,
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        );
        pdf.extend_from_slice(table.as_bytes());
        pdf
    }

    pub(crate) fn compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(data).expect("compress");
        encoder.finish().expect("finish")
    }

    pub(crate) fn stream(dict: &str, data: &[u8], compress: bool) -> Vec<u8> {
        let (data, filter) = if compress {
            (self::compress(data), " /Filter /FlateDecode")
        } else {
            (data.to_vec(), "")
        };
        let mut out = format!("<< {dict} /Length {}{filter} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(&data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    /// One page drawing `content` with Helvetica as `/F1` and `xobjects` as
    /// the page's `/XObject` dictionary entries (objects from 5 on are
    /// `extra`).
    pub(crate) fn one_page(content: &[u8], xobjects: &str, extra: Vec<Vec<u8>>) -> Vec<u8> {
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << {xobjects} >> >> >>"
            )
            .into_bytes(),
            stream("", content, true),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ];
        objects.extend(extra);
        assemble(&objects, "")
    }

    /// Three pages: plain text in Helvetica; a two-byte font decoded through a
    /// `ToUnicode` map, with a compressed content stream; and text drawn by a
    /// form `XObject`.
    pub(crate) fn three_page_pdf() -> Vec<u8> {
        assemble(&three_page_objects(), "")
    }

    pub(crate) fn three_page_objects() -> Vec<Vec<u8>> {
        let cmap = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
2 beginbfchar\n<0001> <0048>\n<0002> <0069>\nendbfchar\n\
1 beginbfrange\n<0010> <0012> <0041>\nendbfrange\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";
        vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R 11 0 R] /Count 3 /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents [8 0 R 9 0 R] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Embedded /Encoding /Identity-H /ToUnicode 10 0 R >>".to_vec(),
            stream(
                "",
                b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj 0 -14 Td (Revenue grew \\(a lot\\)) Tj ET",
                false,
            ),
            stream("", b"BT /F2 12 Tf 72 720 Td <00010002> Tj ET", true),
            stream("", b"BT /F2 12 Tf 72 700 Td [<0010> -300 <00110012>] TJ ET", true),
            stream("", cmap, false),
            b"<< /Type /Page /Parent 2 0 R /Contents 12 0 R /Resources << /XObject << /X1 13 0 R >> /Font << /F1 5 0 R >> >> >>".to_vec(),
            stream("", b"q /X1 Do Q", false),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 600 800]",
                b"BT /F1 10 Tf 72 500 Td (Inside a form) Tj ET",
                false,
            ),
        ]
    }
}
