//! Zip packages and XML walking shared by the DOCX and PPTX readers.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read as _, Seek as _};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

/// The largest XML part read out of a package, decompressed. Guards against
/// zip bombs: a 10 MB file can claim gigabytes once inflated.
pub(super) const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
/// The largest main part (`word/document.xml`), which holds a whole Word
/// document: a long report made mostly of tables can pass
/// [`MAX_PART_BYTES`] while its file stays small. It still counts against
/// [`MAX_PACKAGE_BYTES`].
pub(super) const MAX_MAIN_PART_BYTES: u64 = MAX_PACKAGE_BYTES;
/// The most bytes inflated across all parts read from one package, so many
/// parts (or many entries sharing the same compressed bytes) cannot add up
/// to gigabytes.
pub(super) const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

pub(super) struct Package {
    archive: zip::ZipArchive<File>,
    kind: &'static str,
    /// Inflated bytes still allowed before [`MAX_PACKAGE_BYTES`] is reached.
    remaining: u64,
}

impl Package {
    /// Opens `path` as a zip package holding a `kind` ("Word document").
    pub(super) fn open(path: &Path, kind: &'static str) -> Result<Self, String> {
        let mut file =
            File::open(path).map_err(|error| format!("cannot open the file: {error}"))?;
        // Text under a document name (an export saved as .docx) goes to
        // read_file, judged by the same test read_file uses.
        let mut head = Vec::new();
        (&mut file)
            .take(8192)
            .read_to_end(&mut head)
            .map_err(|error| format!("cannot open the file: {error}"))?;
        let extension = path.extension().map_or_else(String::new, |extension| {
            extension.to_string_lossy().into_owned()
        });
        if !runtime::has_document_signature(&extension, &head) && super::looks_like_text(&head) {
            return Err(super::text_file_error(path));
        }
        file.rewind()
            .map_err(|error| format!("cannot open the file: {error}"))?;
        let archive = zip::ZipArchive::new(file).map_err(|_| {
            format!("this is not a readable {kind}: the file is damaged, password protected or in an older format")
        })?;
        Ok(Self {
            archive,
            kind,
            remaining: MAX_PACKAGE_BYTES,
        })
    }

    /// The same package with only `bytes` left to inflate.
    #[cfg(test)]
    pub(super) fn with_budget(mut self, bytes: u64) -> Self {
        self.remaining = bytes;
        self
    }

    fn too_big(&self) -> String {
        format!(
            "this {} unpacks to more than {} MB, more than read_document reads",
            self.kind,
            MAX_PACKAGE_BYTES / (1024 * 1024)
        )
    }

    /// The text of part `name`, or `None` when the package has no such part.
    pub(super) fn read_part(&mut self, name: &str) -> Result<Option<String>, String> {
        self.read_part_up_to(name, MAX_PART_BYTES)
    }

    /// The text of part `name`, inflated to at most `max_part_bytes`.
    pub(super) fn read_part_up_to(
        &mut self,
        name: &str,
        max_part_bytes: u64,
    ) -> Result<Option<String>, String> {
        let remaining = self.remaining;
        let too_big = self.too_big();
        let mut entry = match self.archive.by_name(name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "this {} is damaged: cannot read {name} ({error})",
                    self.kind
                ))
            }
        };
        // The central directory's size is only a claim, but one that already
        // passes a limit needs no inflating to refuse.
        let part_too_big = format!(
            "{name} in this {} unpacks to more than {} MB",
            self.kind,
            max_part_bytes / (1024 * 1024)
        );
        if entry.size() > max_part_bytes {
            return Err(part_too_big);
        }
        if entry.size() > remaining {
            return Err(too_big);
        }
        let limit = max_part_bytes.min(remaining);
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                format!(
                    "this {} is damaged: cannot read {name} ({error})",
                    self.kind
                )
            })?;
        let read = bytes.len() as u64;
        if read > limit {
            return Err(if limit < max_part_bytes {
                too_big
            } else {
                part_too_big
            });
        }
        self.remaining -= read;
        // One copy of a large part at a time: valid UTF-8 is kept as read.
        let mut text = String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        if text.starts_with('\u{feff}') {
            text.drain(..'\u{feff}'.len_utf8());
        }
        Ok(Some(text))
    }
}

/// The text of a `SmartArt` diagram's data part (`dgm:dataModel`): each
/// point's text with its depth below the diagram's root, in saved order.
/// Presentation, transition and root points carry no text of their own.
pub(super) fn diagram_text(xml: &str) -> Result<Vec<(usize, String)>, String> {
    /// Parent links followed to find a point's depth; guards cycles.
    const MAX_DEPTH: usize = 16;
    let mut points: Vec<(String, String, String)> = Vec::new();
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut current: Option<(String, String, String)> = None;
    let mut in_run = false;
    let mut in_text = 0usize;
    walk(xml, |node| match node {
        Node::Open { name: "pt", attrs } => {
            current = Some((
                attr(attrs, "modelId").unwrap_or_default().to_string(),
                attr(attrs, "type").unwrap_or("node").to_string(),
                String::new(),
            ));
        }
        Node::Close { name: "pt" } => {
            if let Some(point) = current.take() {
                points.push(point);
            }
        }
        Node::Open { name: "cxn", attrs } => {
            if matches!(attr(attrs, "type"), None | Some("parOf")) {
                if let (Some(parent), Some(child)) = (attr(attrs, "srcId"), attr(attrs, "destId")) {
                    parents.insert(child.to_string(), parent.to_string());
                }
            }
        }
        Node::Open { name: "p", .. } => {
            if let Some((_, _, text)) = current.as_mut() {
                if !text.is_empty() {
                    text.push(' ');
                }
            }
        }
        Node::Open {
            name: "r" | "fld", ..
        } => in_run = true,
        Node::Close { name: "r" | "fld" } => in_run = false,
        Node::Open { name: "t", .. } => in_text += 1,
        Node::Close { name: "t" } => in_text = in_text.saturating_sub(1),
        Node::Text(chunk) if in_run && in_text > 0 => {
            if let Some((_, _, text)) = current.as_mut() {
                text.push_str(chunk);
            }
        }
        _ => {}
    })?;
    let kinds: HashMap<&str, &str> = points
        .iter()
        .map(|(id, kind, _)| (id.as_str(), kind.as_str()))
        .collect();
    let mut lines = Vec::new();
    for (id, kind, text) in &points {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() || matches!(kind.as_str(), "pres" | "parTrans" | "sibTrans" | "doc") {
            continue;
        }
        let mut depth = 0usize;
        let mut at = id.as_str();
        while let Some(parent) = parents.get(at) {
            if depth >= MAX_DEPTH || kinds.get(parent.as_str()).is_none_or(|kind| *kind == "doc") {
                break;
            }
            depth += 1;
            at = parent;
        }
        lines.push((depth, text));
    }
    Ok(lines)
}

/// A diagram's text as a bulleted list, indented by depth.
pub(super) fn render_diagram(lines: &[(usize, String)]) -> String {
    lines
        .iter()
        .map(|(depth, text)| format!("{}- {text}", "  ".repeat((*depth).min(8))))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One step of an XML walk. Names are local (no namespace prefix); attribute
/// keys keep their prefix (`w:val`).
pub(super) enum Node<'a> {
    Open {
        name: &'a str,
        attrs: &'a [(String, String)],
    },
    Close {
        name: &'a str,
    },
    Text(&'a str),
}

/// Walks `xml`, calling `visit` for each element start, end and text run.
/// Empty elements arrive as an open followed by a close.
pub(super) fn walk(xml: &str, mut visit: impl FnMut(Node<'_>)) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut attrs = Vec::new();
    loop {
        let event = reader.read_event().map_err(|error| {
            format!("damaged XML at byte {}: {error}", reader.buffer_position())
        })?;
        match event {
            Event::Start(start) => {
                attrs.clear();
                for attr in start.attributes().with_checks(false).flatten() {
                    let key = attr.key.into_inner().to_string();
                    let value = quick_xml::escape::unescape(&attr.value)
                        .map_or_else(|_| attr.value.to_string(), std::borrow::Cow::into_owned);
                    attrs.push((key, value));
                }
                let name = start.local_name();
                visit(Node::Open {
                    name: name.into_inner(),
                    attrs: &attrs,
                });
            }
            Event::End(end) => visit(Node::Close {
                name: end.local_name().into_inner(),
            }),
            Event::Text(text) => visit(Node::Text(&text.xml10_content())),
            Event::CData(data) => visit(Node::Text(&data.into_inner())),
            Event::GeneralRef(reference) => {
                if let Ok(Some(ch)) = reference.resolve_char_ref() {
                    visit(Node::Text(ch.encode_utf8(&mut [0; 4])));
                } else if let Some(text) = quick_xml::escape::resolve_predefined_entity(&reference)
                {
                    visit(Node::Text(text));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// The value of attribute `qname` (`"w:val"`): an exact key match, else any
/// prefixed key with the same local name, since prefixes are only conventions.
pub(super) fn attr<'a>(attrs: &'a [(String, String)], qname: &str) -> Option<&'a str> {
    if let Some((_, value)) = attrs.iter().find(|(key, _)| key == qname) {
        return Some(value);
    }
    let (_, local) = qname.split_once(':')?;
    attrs
        .iter()
        .find(|(key, _)| key.split_once(':').is_some_and(|(_, name)| name == local))
        .map(|(_, value)| value.as_str())
}

/// The relationships of a `.rels` part: id, type and target of each.
pub(super) fn relationships(xml: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut found = Vec::new();
    walk(xml, |node| {
        if let Node::Open {
            name: "Relationship",
            attrs,
        } = node
        {
            if let (Some(id), Some(target)) = (attr(attrs, "Id"), attr(attrs, "Target")) {
                found.push((
                    id.to_string(),
                    attr(attrs, "Type").unwrap_or_default().to_string(),
                    target.to_string(),
                ));
            }
        }
    })?;
    Ok(found)
}

/// Resolves a relationship `target` against the folder of the part that owns
/// it (`"ppt/slides"`), as a package path (`"ppt/notesSlides/notesSlide1.xml"`).
pub(super) fn resolve_target(base_dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base_dir
            .split('/')
            .filter(|part| !part.is_empty())
            .collect()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The `_rels` part that belongs to package part `part`.
pub(super) fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// The folder of package part `part`.
pub(super) fn part_dir(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(dir, _)| dir)
}
