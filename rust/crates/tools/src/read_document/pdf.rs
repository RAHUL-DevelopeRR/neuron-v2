//! PDF text, page by page.
//!
//! A small reader of its own: it indexes the file's objects (including those
//! packed in object streams), walks the page tree, and runs each page's
//! content streams, decoding text through the fonts' `ToUnicode` maps and
//! following form `XObject`s. Encrypted files are refused; scanned pages without
//! a text layer come back empty, since no OCR runs.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;

use super::{Document, Selection, Unit};

const KIND: &str = "PDF document";
/// The largest stream inflated, guarding against compression bombs.
const MAX_STREAM_BYTES: u64 = 64 * 1024 * 1024;
/// How deep page trees and nested form `XObject`s are followed.
const MAX_DEPTH: usize = 32;
const MAX_FORM_DEPTH: usize = 6;

pub(super) fn read(path: &Path, selection: &Selection) -> Result<Document, String> {
    let data = std::fs::read(path).map_err(|error| format!("cannot open the file: {error}"))?;
    read_bytes(&data, selection)
}

pub(super) fn read_bytes(data: &[u8], selection: &Selection) -> Result<Document, String> {
    let head = &data[..data.len().min(1024)];
    if find(head, b"%PDF-").is_none() {
        return Err("this is not a readable PDF document: the %PDF header is missing".to_string());
    }
    let pdf = Pdf::parse(data);
    if pdf.encrypted {
        return Err(
            "this PDF is encrypted; its text cannot be read without decrypting it first"
                .to_string(),
        );
    }
    let pages = pdf.pages();
    let mut notes = Vec::new();
    if pages.is_empty() {
        // No page tree found: fall back to every text stream as one page.
        let text = crate::pdf_extract::extract_text_from_bytes(data);
        notes.push(
            "the page structure could not be read; all text is shown as one page".to_string(),
        );
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
        });
    }

    let mut units = Vec::new();
    let mut undecodable = false;
    let mut any_text = false;
    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, None) {
            continue;
        }
        let mut sink = TextSink::default();
        let content = page
            .contents
            .iter()
            .filter_map(|reference| pdf.stream_data(reference))
            .collect::<Vec<_>>()
            .join(&b"\n"[..]);
        pdf.run_content(&content, page.resources, &mut sink, 0);
        undecodable |= sink.undecodable;
        let text = sink.finish();
        any_text |= !text.is_empty();
        units.push(Unit {
            number,
            markdown: page_markdown(number, &text),
        });
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
    Ok(Document {
        kind: KIND,
        unit: "page",
        total: pages.len(),
        units,
        notes,
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
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
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
        self.object_from(token)
    }

    fn object_from(&mut self, token: Token) -> Option<Obj> {
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
                    match self.next_token()? {
                        Token::DictClose => break,
                        Token::Name(key) => {
                            let next = self.next_token()?;
                            if next == Token::DictClose {
                                break;
                            }
                            if let Some(value) = self.object_from(next) {
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
                    match self.next_token()? {
                        Token::ArrayClose => break,
                        other => {
                            if let Some(item) = self.object_from(other) {
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

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

// ---------------------------------------------------------------------------
// The document

struct Pdf {
    objects: HashMap<u32, Obj>,
    trailer: Dict,
    encrypted: bool,
}

struct Page<'a> {
    contents: Vec<&'a Obj>,
    resources: Option<&'a Dict>,
}

impl Pdf {
    #[allow(clippy::too_many_lines)]
    fn parse(data: &[u8]) -> Self {
        let mut objects = HashMap::new();
        let mut cursor = 0;
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
            let Some(number) = object_number_before(data, at) else {
                continue;
            };
            let mut lexer = Lexer::new(data, at + 3);
            let Some(object) = lexer.object() else {
                continue;
            };
            if matches!(lexer.next_token(), Some(Token::Keyword(ref word)) if word == "stream") {
                let start = skip_eol(data, lexer.pos);
                let declared = object
                    .as_dict()
                    .and_then(|dict| dict.get("Length"))
                    .and_then(Obj::as_num)
                    .map(to_usize);
                let end = declared
                    .filter(|length| {
                        start + length <= data.len()
                            && data[start + length..]
                                .iter()
                                .position(|byte| !is_whitespace(*byte))
                                .is_some_and(|offset| {
                                    data[start + length + offset..].starts_with(b"endstream")
                                })
                    })
                    .map(|length| start + length)
                    .or_else(|| {
                        find(&data[start..], b"endstream").map(|offset| {
                            let mut end = start + offset;
                            while end > start && matches!(data[end - 1], b'\r' | b'\n') {
                                end -= 1;
                            }
                            end
                        })
                    });
                let Some(end) = end else {
                    continue;
                };
                let dict = match object {
                    Obj::Dict(dict) => dict,
                    _ => Dict::new(),
                };
                objects.insert(number, Obj::Stream(dict, data[start..end].to_vec()));
                cursor = end;
            } else {
                objects.insert(number, object);
            }
        }

        // Objects packed into object streams (PDF 1.5 and later).
        let packed = objects
            .values()
            .filter_map(|object| match object {
                Obj::Stream(dict, _)
                    if dict.get("Type").and_then(Obj::as_name) == Some("ObjStm") =>
                {
                    Some(object.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for stream in packed {
            let Some(decoded) = decode_stream(&stream) else {
                continue;
            };
            let Some(dict) = stream.as_dict() else {
                continue;
            };
            let count = dict.get("N").and_then(Obj::as_num).map_or(0, to_usize);
            let first = dict.get("First").and_then(Obj::as_num).map_or(0, to_usize);
            if first > decoded.len() {
                continue;
            }
            let mut header = Lexer::new(&decoded[..first], 0);
            for _ in 0..count {
                let (Some(Token::Num(number)), Some(Token::Num(offset))) =
                    (header.next_token(), header.next_token())
                else {
                    break;
                };
                let number = to_u32(number);
                if objects.contains_key(&number) {
                    continue;
                }
                let mut lexer = Lexer::new(&decoded, first + to_usize(offset));
                if let Some(object) = lexer.object() {
                    objects.insert(number, object);
                }
            }
        }

        // The newest trailer: a classic `trailer` dictionary or an xref stream.
        let mut trailer = rfind(data, b"trailer")
            .and_then(|at| Lexer::new(data, at + b"trailer".len()).object())
            .and_then(|object| match object {
                Obj::Dict(dict) => Some(dict),
                _ => None,
            })
            .unwrap_or_default();
        let mut encrypted = trailer.contains_key("Encrypt");
        for object in objects.values() {
            if let Obj::Stream(dict, _) = object {
                if dict.get("Type").and_then(Obj::as_name) == Some("XRef") {
                    encrypted |= dict.contains_key("Encrypt");
                    if !trailer.contains_key("Root") {
                        if let Some(root) = dict.get("Root") {
                            trailer.insert("Root".to_string(), root.clone());
                        }
                    }
                }
            }
        }
        Self {
            objects,
            trailer,
            encrypted,
        }
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

    fn collect_pages<'a>(
        &'a self,
        node: &'a Obj,
        inherited: Option<&'a Dict>,
        pages: &mut Vec<Page<'a>>,
        seen: &mut HashSet<u32>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Obj::Ref(number) = node {
            if !seen.insert(*number) {
                return;
            }
        }
        let Some(dict) = self.resolve(node).as_dict() else {
            return;
        };
        let resources = self
            .get(dict, "Resources")
            .and_then(Obj::as_dict)
            .or(inherited);
        let kind = dict.get("Type").and_then(Obj::as_name);
        if let (Some(Obj::Array(kids)), false) = (self.get(dict, "Kids"), kind == Some("Page")) {
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
        decode_stream(self.resolve(object))
    }

    fn fonts(&self, resources: Option<&Dict>) -> HashMap<String, Font> {
        let mut fonts = HashMap::new();
        let Some(font_dict) = resources
            .and_then(|resources| self.get(resources, "Font"))
            .and_then(Obj::as_dict)
        else {
            return fonts;
        };
        for (name, font) in font_dict {
            if let Some(font) = self.resolve(font).as_dict() {
                fonts.insert(name.clone(), self.font(font));
            }
        }
        fonts
    }

    fn font(&self, dict: &Dict) -> Font {
        let composite = dict.get("Subtype").and_then(Obj::as_name) == Some("Type0");
        let cmap = self
            .get(dict, "ToUnicode")
            .and_then(decode_stream)
            .map(|data| CMap::parse(&data))
            .filter(|cmap| !cmap.map.is_empty());
        let mut widths = HashMap::new();
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
                for (offset, width) in entries.iter().enumerate() {
                    if let Some(width) = self.resolve(width).as_num() {
                        widths.insert(first + u32::try_from(offset).unwrap_or(0), width);
                    }
                }
            }
        }
        Font {
            two_byte: composite,
            cmap,
            widths,
            default_width,
        }
    }

    /// The `W` array of a CID font: `c [w1 w2 ...]` and `c_first c_last w`.
    fn cid_widths(&self, entries: &[Obj], widths: &mut HashMap<u32, f64>) {
        let mut index = 0;
        while index < entries.len() {
            let Some(first) = self.resolve(&entries[index]).as_num().map(to_u32) else {
                break;
            };
            match entries.get(index + 1).map(|entry| self.resolve(entry)) {
                Some(Obj::Array(list)) => {
                    for (offset, width) in list.iter().enumerate() {
                        if let Some(width) = self.resolve(width).as_num() {
                            widths.insert(first + u32::try_from(offset).unwrap_or(0), width);
                        }
                    }
                    index += 2;
                }
                Some(Obj::Num(last)) => {
                    let last = to_u32(*last).min(first.saturating_add(65_535));
                    if let Some(width) = entries
                        .get(index + 2)
                        .and_then(|width| self.resolve(width).as_num())
                    {
                        for code in first..=last {
                            widths.insert(code, width);
                        }
                    }
                    index += 3;
                }
                _ => break,
            }
        }
    }

    /// Runs a content stream, writing its text to `sink`.
    #[allow(clippy::too_many_lines)]
    fn run_content(
        &self,
        data: &[u8],
        resources: Option<&Dict>,
        sink: &mut TextSink,
        depth: usize,
    ) {
        let fonts = self.fonts(resources);
        let mut state = TextState::default();
        let mut lexer = Lexer::new(data, 0);
        let mut operands: Vec<Obj> = Vec::new();
        while let Some(token) = lexer.next_token() {
            let Token::Keyword(operator) = token else {
                if let Some(operand) = lexer.object_from(token) {
                    operands.push(operand);
                }
                continue;
            };
            let number = |index: usize| operands.get(index).and_then(Obj::as_num).unwrap_or(0.0);
            match operator.as_str() {
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
                        state.show(
                            bytes,
                            fonts.get(state.font.as_deref().unwrap_or_default()),
                            sink,
                        );
                    }
                }
                "'" | "\"" => {
                    state.next_line();
                    if let Some(Obj::Str(bytes)) = operands.last() {
                        state.show(
                            bytes,
                            fonts.get(state.font.as_deref().unwrap_or_default()),
                            sink,
                        );
                    }
                }
                "TJ" => {
                    if let Some(Obj::Array(items)) = operands.last() {
                        let font = fonts.get(state.font.as_deref().unwrap_or_default());
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
                    let form = operands.first().and_then(Obj::as_name).and_then(|name| {
                        resources
                            .and_then(|resources| self.get(resources, "XObject"))
                            .and_then(Obj::as_dict)
                            .and_then(|xobjects| self.get(xobjects, name))
                    });
                    if let Some(form @ Obj::Stream(dict, _)) = form {
                        if dict.get("Subtype").and_then(Obj::as_name) == Some("Form") {
                            if let Some(content) = decode_stream(form) {
                                let inner = self
                                    .get(dict, "Resources")
                                    .and_then(Obj::as_dict)
                                    .or(resources);
                                sink.newline();
                                self.run_content(&content, inner, sink, depth + 1);
                                sink.newline();
                            }
                        }
                    }
                }
                "BI" => skip_inline_image(&mut lexer),
                _ => {}
            }
            operands.clear();
        }
    }
}

fn object_number_before(data: &[u8], at: usize) -> Option<u32> {
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
    digits(&mut index)?;
    if !skip_space(&mut index) {
        return None;
    }
    let number = digits(&mut index)?;
    if index > 0 && !is_whitespace(data[index - 1]) && !is_delimiter(data[index - 1]) {
        return None;
    }
    std::str::from_utf8(&data[number]).ok()?.parse().ok()
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

/// The decoded bytes of a stream object, or `None` for filters this reader
/// does not handle (images mostly).
fn decode_stream(object: &Obj) -> Option<Vec<u8>> {
    let Obj::Stream(dict, raw) = object else {
        return None;
    };
    let filters = match dict.get("Filter") {
        None => Vec::new(),
        Some(Obj::Name(name)) => vec![name.as_str()],
        Some(Obj::Array(names)) => names.iter().filter_map(Obj::as_name).collect(),
        Some(_) => return None,
    };
    let mut data = raw.clone();
    for filter in filters {
        data = match filter {
            "FlateDecode" | "Fl" => inflate(&data)?,
            "ASCIIHexDecode" | "AHx" => {
                let end = data
                    .iter()
                    .position(|byte| *byte == b'>')
                    .unwrap_or(data.len());
                hex_decode(&data[..end])
            }
            "ASCII85Decode" | "A85" => ascii85(&data),
            _ => return None,
        };
    }
    Some(data)
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let zlib = flate2::read::ZlibDecoder::new(data)
        .take(MAX_STREAM_BYTES)
        .read_to_end(&mut out);
    if zlib.is_ok() || !out.is_empty() {
        return Some(out);
    }
    out.clear();
    flate2::read::DeflateDecoder::new(data)
        .take(MAX_STREAM_BYTES)
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

/// A `ToUnicode` map: character code to text.
#[derive(Debug, Default)]
struct CMap {
    code_bytes: usize,
    map: HashMap<u32, String>,
}

impl CMap {
    #[allow(clippy::too_many_lines)]
    fn parse(data: &[u8]) -> Self {
        let mut cmap = Self::default();
        let mut lexer = Lexer::new(data, 0);
        let mut pending: Vec<Obj> = Vec::new();
        while let Some(token) = lexer.next_token() {
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
                                    cmap.map.insert(code_value(code), utf16(text));
                                }
                            }
                        }
                        "endbfrange" => {
                            for triple in pending.chunks(3) {
                                if let [Obj::Str(low), Obj::Str(high), target] = triple {
                                    cmap.note_width(low.len());
                                    cmap.insert_range(code_value(low), code_value(high), target);
                                }
                            }
                        }
                        _ => {}
                    }
                    pending.clear();
                }
                other => {
                    if let Some(object) = lexer.object_from(other) {
                        pending.push(object);
                    }
                }
            }
        }
        if cmap.code_bytes == 0 {
            cmap.code_bytes = 1;
        }
        cmap
    }

    fn note_width(&mut self, width: usize) {
        if self.code_bytes == 0 {
            self.code_bytes = width.clamp(1, 4);
        }
    }

    fn insert_range(&mut self, low: u32, high: u32, target: &Obj) {
        let high = high.min(low.saturating_add(65_535));
        match target {
            Obj::Str(start) => {
                let mut units = start
                    .chunks(2)
                    .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
                    .collect::<Vec<_>>();
                for code in low..=high {
                    self.map.insert(code, String::from_utf16_lossy(&units));
                    if let Some(last) = units.last_mut() {
                        *last = last.wrapping_add(1);
                    }
                }
            }
            Obj::Array(texts) => {
                for (code, text) in (low..=high).zip(texts) {
                    if let Obj::Str(text) = text {
                        self.map.insert(code, utf16(text));
                    }
                }
            }
            _ => {}
        }
    }
}

fn code_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |acc, byte| (acc << 8) | u32::from(*byte))
}

fn utf16(bytes: &[u8]) -> String {
    let units = bytes
        .chunks(2)
        .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

#[derive(Debug, Default)]
struct Font {
    two_byte: bool,
    cmap: Option<CMap>,
    widths: HashMap<u32, f64>,
    default_width: f64,
}

impl Font {
    /// The text of `bytes` and its width in thousandths of the font size.
    /// `None` text means the codes have no Unicode meaning to recover.
    fn decode(&self, bytes: &[u8]) -> (Option<String>, f64) {
        let code_bytes = self
            .cmap
            .as_ref()
            .map_or(if self.two_byte { 2 } else { 1 }, |cmap| cmap.code_bytes);
        let mut text = String::new();
        let mut width = 0.0;
        let mut decodable = true;
        for chunk in bytes.chunks(code_bytes.max(1)) {
            let code = code_value(chunk);
            width += self
                .widths
                .get(&code)
                .copied()
                .unwrap_or(self.default_width);
            match &self.cmap {
                Some(cmap) => match cmap.map.get(&code) {
                    Some(mapped) => text.push_str(mapped),
                    None if !self.two_byte => {
                        text.push(win_ansi(u8::try_from(code).unwrap_or(b'?')));
                    }
                    None => {}
                },
                None if self.two_byte => decodable = false,
                None => text.extend(chunk.iter().map(|byte| win_ansi(*byte))),
            }
        }
        (decodable.then_some(text), width)
    }
}

/// A single-byte code read as Windows-1252, the usual simple-font encoding.
fn win_ansi(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}',
        '\u{017d}', '\u{fffd}', '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}',
        '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// Where text goes on the page, enough to tell line breaks and word gaps.
#[derive(Debug)]
struct TextState {
    font: Option<String>,
    font_size: f64,
    leading: f64,
    /// Text line matrix: scale and origin.
    line: [f64; 6],
    /// Current x in user space after the last shown text.
    cursor_x: f64,
    last_y: Option<f64>,
    last_end_x: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            font_size: 12.0,
            leading: 0.0,
            line: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            cursor_x: 0.0,
            last_y: None,
            last_end_x: 0.0,
        }
    }
}

impl TextState {
    fn begin_text(&mut self) {
        self.line = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        self.cursor_x = 0.0;
    }

    fn scale(&self) -> f64 {
        let scale = self.line[0].hypot(self.line[1]);
        if scale > 0.0 {
            scale
        } else {
            1.0
        }
    }

    fn y(&self) -> f64 {
        self.line[5]
    }

    fn move_line(&mut self, tx: f64, ty: f64) {
        let [scale_x, skew_y, skew_x, scale_y, origin_x, origin_y] = self.line;
        self.line[4] = origin_x + tx * scale_x + ty * skew_x;
        self.line[5] = origin_y + tx * skew_y + ty * scale_y;
        self.cursor_x = self.line[4];
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
        self.cursor_x = matrix[4];
    }

    fn em(&self) -> f64 {
        self.font_size.abs().max(0.1) * self.scale()
    }

    fn show(&mut self, bytes: &[u8], font: Option<&Font>, sink: &mut TextSink) {
        let fallback = Font {
            default_width: 500.0,
            ..Font::default()
        };
        let font = font.unwrap_or(&fallback);
        let (text, width) = font.decode(bytes);
        let em = self.em();
        if let Some(last_y) = self.last_y {
            if (self.y() - last_y).abs() > em * 0.5 {
                sink.newline();
            } else if self.cursor_x - self.last_end_x > em * 0.15 {
                sink.space();
            }
        }
        match text {
            Some(text) => sink.push(&text),
            None => sink.undecodable = true,
        }
        self.cursor_x += width / 1000.0 * em;
        self.last_end_x = self.cursor_x;
        self.last_y = Some(self.y());
    }

    /// A `TJ` number: moves left by `adjust` thousandths of an em.
    fn adjust(&mut self, adjust: f64, sink: &mut TextSink) {
        self.cursor_x -= adjust / 1000.0 * self.em();
        if adjust < -180.0 {
            sink.space();
            self.last_end_x = self.cursor_x;
        }
    }
}

#[derive(Debug, Default)]
struct TextSink {
    text: String,
    undecodable: bool,
}

impl TextSink {
    fn push(&mut self, text: &str) {
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

    pub(crate) fn stream(dict: &str, data: &[u8], compress: bool) -> Vec<u8> {
        let (data, filter) = if compress {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).expect("compress");
            (encoder.finish().expect("finish"), " /Filter /FlateDecode")
        } else {
            (data.to_vec(), "")
        };
        let mut out = format!("<< {dict} /Length {}{filter} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(&data);
        out.extend_from_slice(b"\nendstream");
        out
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
