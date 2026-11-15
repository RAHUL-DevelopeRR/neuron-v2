//! Word documents (`.docx`): headings, paragraphs, lists and tables.
//!
//! A `.docx` holds no fixed pages; pages here follow the page breaks Word saved
//! with the file (`lastRenderedPageBreak`) plus section breaks that start a new
//! page, else explicit page breaks, else about [`ESTIMATED_PAGE_CHARS`]
//! characters each.
//!
//! Footnotes and endnotes are marked where they are referenced (`[^1]`,
//! endnotes `[^i]`) and their text follows at the end of the page that
//! references them (on estimated pages, right after the paragraph or table
//! that references them). `SmartArt` diagrams are read from their data parts.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use super::xml::{
    attr, diagram_text, part_dir, relationships, rels_path, render_diagram, resolve_target, walk,
    Node, Package, MAX_MAIN_PART_BYTES,
};
use super::{Budget, Document, Selection, TableBuilder, TablePart, Unit};

const KIND: &str = "Word document";

/// Characters per page when a document carries no page breaks at all.
pub(super) const ESTIMATED_PAGE_CHARS: usize = 3_000;

/// List levels Word supports (`w:ilvl` 0 to 8).
const LEVELS: usize = 9;
/// Characters kept of a list level's label template (`w:lvlText`); real
/// ones look like `"%1.%2."`.
const MAX_LEVEL_TEXT_CHARS: usize = 64;
/// Characters of one list label.
const MAX_LABEL_CHARS: usize = 64;
/// Tables nested inside table cells that are kept apart; deeper tables add
/// their text to the deepest kept cell.
const MAX_TABLE_NESTING: usize = 4;
/// `SmartArt` diagrams read from one document.
const MAX_DIAGRAMS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StyleKind {
    Heading(usize),
    Bullet,
    Numbered,
}

/// What `word/styles.xml` says about each paragraph style.
#[derive(Debug, Default)]
struct Styles {
    kinds: HashMap<String, StyleKind>,
    /// Numbering a style carries itself (`w:pPr/w:numPr`): list id and level.
    numbering: HashMap<String, (String, Option<String>)>,
}

pub(super) fn read(
    path: &Path,
    selection: &Selection,
    budget: &mut Budget,
) -> Result<Document, String> {
    let mut package = Package::open(path, KIND)?;
    let body = package
        .read_part_up_to("word/document.xml", MAX_MAIN_PART_BYTES)?
        .ok_or_else(|| format!("this is not a readable {KIND}: word/document.xml is missing"))?;
    let styles = match package.read_part("word/styles.xml")? {
        Some(xml) => parse_styles(&xml)?,
        None => Styles::default(),
    };
    let numbering = match package.read_part("word/numbering.xml")? {
        Some(xml) => parse_numbering(&xml)?,
        None => Numbering::default(),
    };
    // Word's own page count from its last save, a check on ours.
    let saved_pages = package
        .read_part("docProps/app.xml")
        .ok()
        .flatten()
        .and_then(|xml| saved_page_count(&xml));
    let mut notes = Vec::new();
    let diagrams = read_diagrams(&mut package, &mut notes);
    let mut parsed = parse_body(&body, &styles, &numbering, &diagrams)?;
    if !parsed.note_refs.is_empty() {
        add_note_text(&mut package, &mut parsed, &mut notes);
    }
    let mut document = paginate(parsed, selection, saved_pages, budget);
    document.notes.extend(notes);
    Ok(document)
}

/// The text of each `SmartArt` diagram the document body links to, by the
/// relationship id its `dgm:relIds r:dm` names. A diagram that cannot be read
/// adds a note instead of failing the whole document.
fn read_diagrams(
    package: &mut Package,
    notes: &mut Vec<String>,
) -> HashMap<String, Vec<(usize, String)>> {
    const PART: &str = "word/document.xml";
    let mut diagrams = HashMap::new();
    let Ok(Some(rels)) = package.read_part(&rels_path(PART)) else {
        return diagrams;
    };
    let Ok(rels) = relationships(&rels) else {
        return diagrams;
    };
    for (id, _, target) in rels
        .into_iter()
        .filter(|(_, kind, _)| kind.ends_with("/diagramData"))
        .take(MAX_DIAGRAMS)
    {
        let part = resolve_target(part_dir(PART), &target);
        match package
            .read_part(&part)
            .and_then(|xml| xml.map_or_else(|| Ok(Vec::new()), |xml| diagram_text(&xml)))
        {
            Ok(lines) => {
                diagrams.insert(id, lines);
            }
            Err(error) => notes.push(format!("a SmartArt diagram could not be read: {error}")),
        }
    }
    diagrams
}

/// Adds the text of every footnote and endnote the body references, as
/// `[^1]: text` blocks placed by [`paginate`] on the page that references
/// each.
fn add_note_text(package: &mut Package, parsed: &mut ParsedBody, notes: &mut Vec<String>) {
    let mut texts: HashMap<(NoteKind, String), String> = HashMap::new();
    for (kind, part) in [
        (NoteKind::Foot, "word/footnotes.xml"),
        (NoteKind::End, "word/endnotes.xml"),
    ] {
        if !parsed.note_refs.iter().any(|note| note.kind == kind) {
            continue;
        }
        match package
            .read_part(part)
            .and_then(|xml| xml.map(|xml| parse_notes(&xml)).transpose())
        {
            Ok(Some(found)) => {
                texts.extend(found.into_iter().map(|(id, text)| ((kind, id), text)));
            }
            Ok(None) => {}
            Err(error) => notes.push(format!("the {} could not be read: {error}", kind.plural())),
        }
    }
    let mut missing = 0usize;
    for note in std::mem::take(&mut parsed.note_refs) {
        match texts.get(&(note.kind, note.id)) {
            Some(text) => parsed.note_blocks.push((
                note.anchor,
                Block::Text {
                    page: note.page,
                    markdown: format!("[^{}]: {}", note.label, one_line(text)),
                },
            )),
            None => missing += 1,
        }
    }
    if missing > 0 {
        notes.push(format!(
            "{missing} footnote or endnote reference(s) have no note text in the file"
        ));
    }
}

/// Note id to its text, from `word/footnotes.xml` or `word/endnotes.xml`.
/// The separator entries Word keeps there (`w:type` set) are skipped.
fn parse_notes(xml: &str) -> Result<HashMap<String, String>, String> {
    let mut found = HashMap::new();
    let mut current: Option<(String, String)> = None;
    let mut in_text = false;
    let mut skip_depth = 0usize;
    walk(xml, |node| {
        if skip_depth > 0 {
            match node {
                Node::Open { .. } => skip_depth += 1,
                Node::Close { .. } => skip_depth -= 1,
                Node::Text(_) => {}
            }
            return;
        }
        match node {
            Node::Open {
                name: "Fallback", ..
            } => skip_depth = 1,
            Node::Open {
                name: "footnote" | "endnote",
                attrs,
            } => {
                current = match (attr(attrs, "w:id"), attr(attrs, "w:type")) {
                    (Some(id), None | Some("normal")) => Some((id.to_string(), String::new())),
                    _ => None,
                };
            }
            Node::Close {
                name: "footnote" | "endnote",
            } => {
                if let Some((id, text)) = current.take() {
                    let text = text.trim();
                    if !text.is_empty() {
                        found.insert(id, text.to_string());
                    }
                }
            }
            Node::Open { name: "p", .. } => {
                if let Some((_, text)) = current.as_mut() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                }
            }
            Node::Open { name: "t", .. } => in_text = true,
            Node::Close { name: "t" } => in_text = false,
            Node::Text(chunk) if in_text => {
                if let Some((_, text)) = current.as_mut() {
                    text.push_str(chunk);
                }
            }
            Node::Open {
                name: "tab" | "br" | "cr",
                ..
            } => {
                if let Some((_, text)) = current.as_mut() {
                    text.push(' ');
                }
            }
            _ => {}
        }
    })?;
    Ok(found)
}

/// The first of the `pages` saved pages whose blocks, with the page's
/// `[page N]` label, render to more than `ceiling` characters, if any.
fn overlong_saved_page<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    pages: usize,
    ceiling: usize,
) -> Option<usize> {
    /// Room for the `[page N]` label and the blank line after it.
    const PAGE_LABEL_CHARS: usize = 32;
    let ceiling = ceiling.saturating_sub(PAGE_LABEL_CHARS);
    let mut sizes = vec![0usize; pages];
    for block in blocks {
        let size = &mut sizes[block.page().clamp(1, pages) - 1];
        if *size > 0 {
            *size += 2;
        }
        *size += block.chars();
    }
    sizes
        .iter()
        .position(|size| *size > ceiling)
        .map(|index| index + 1)
}

/// The body's `blocks` with the note blocks placed, and the first of the
/// `pages` saved pages too long for one call, if any.
///
/// A saved page longer than one call returns (a long table with no break
/// saved inside it, as generated files often have) could not be read to its
/// end; such a file is paginated by size instead. On saved pages note text
/// goes at the end of the page that references it. Estimated pages are cut
/// by size, so each note block (anchored to the index of the block that
/// references it; anchors rise in reading order) follows that block and
/// lands on the same page as its mark.
fn place_notes(
    blocks: Vec<Block>,
    note_blocks: Vec<(usize, Block)>,
    pages: usize,
) -> (Vec<Block>, Option<usize>) {
    let overlong = if pages > 1 {
        overlong_saved_page(
            blocks
                .iter()
                .chain(note_blocks.iter().map(|(_, block)| block)),
            pages,
            super::max_chars_ceiling(),
        )
    } else {
        None
    };
    if pages > 1 && overlong.is_none() {
        let blocks = blocks
            .into_iter()
            .chain(note_blocks.into_iter().map(|(_, block)| block))
            .collect();
        return (blocks, None);
    }
    let mut placed = Vec::with_capacity(blocks.len() + note_blocks.len());
    let mut note_blocks = note_blocks.into_iter().peekable();
    for (index, block) in blocks.into_iter().enumerate() {
        placed.push(block);
        while let Some((_, note)) = note_blocks.next_if(|(anchor, _)| *anchor <= index) {
            placed.push(note);
        }
    }
    placed.extend(note_blocks.map(|(_, note)| note));
    (placed, overlong)
}

/// `<Pages>46</Pages>` from `docProps/app.xml`.
fn saved_page_count(xml: &str) -> Option<usize> {
    let mut in_pages = false;
    let mut pages = None;
    walk(xml, |node| match node {
        Node::Open { name: "Pages", .. } => in_pages = true,
        Node::Close { name: "Pages" } => in_pages = false,
        Node::Text(text) if in_pages => pages = text.trim().parse().ok(),
        _ => {}
    })
    .ok()?;
    pages.filter(|pages| *pages > 0)
}

fn paginate(
    parsed: ParsedBody,
    selection: &Selection,
    saved_pages: Option<usize>,
    budget: &mut Budget,
) -> Document {
    let ParsedBody {
        blocks,
        pages,
        note_blocks,
        ..
    } = parsed;
    let mut notes = Vec::new();
    let mut pages_of_blocks: Vec<Vec<String>> = Vec::new();
    let (blocks, overlong) = place_notes(blocks, note_blocks, pages);
    if pages > 1 && overlong.is_none() {
        pages_of_blocks.resize(pages, Vec::new());
        for block in blocks {
            let page = block.page().clamp(1, pages);
            pages_of_blocks[page - 1].push(block.into_markdown());
        }
        let mut note = "page numbers follow the page breaks saved with the file".to_string();
        match saved_pages {
            Some(saved) if saved != pages => {
                let _ = write!(
                    note,
                    " and may differ from Word, which last counted {saved} pages"
                );
            }
            Some(_) => {}
            None => note.push_str(" and may differ slightly from Word"),
        }
        notes.push(note);
    } else {
        let mut current: Vec<String> = Vec::new();
        let mut size = 0;
        // A long table is split into parts of about a page, so every row
        // lands on a page one call can return.
        let markdown = blocks.into_iter().flat_map(|block| match block {
            Block::Text { markdown, .. } => vec![markdown],
            Block::Table(part) => part
                .split(ESTIMATED_PAGE_CHARS)
                .iter()
                .map(TablePart::render)
                .collect(),
        });
        for markdown in markdown {
            if size >= ESTIMATED_PAGE_CHARS && !current.is_empty() {
                pages_of_blocks.push(std::mem::take(&mut current));
                size = 0;
            }
            size += markdown.chars().count();
            current.push(markdown);
        }
        if !current.is_empty() || pages_of_blocks.is_empty() {
            pages_of_blocks.push(current);
        }
        if pages_of_blocks.len() > 1 {
            let mut note = match overlong {
                Some(page) => format!(
                    "page {page} of the saved page breaks holds more than one call returns, so pages are estimated at about {ESTIMATED_PAGE_CHARS} characters each instead"
                ),
                None => format!(
                    "the file has no saved page breaks; pages are estimated at about {ESTIMATED_PAGE_CHARS} characters each"
                ),
            };
            if let Some(saved) = saved_pages {
                let _ = write!(note, " (Word last counted {saved} pages)");
            }
            notes.push(note);
        }
    }
    let total = pages_of_blocks.len();
    let mut units = Vec::new();
    let mut unread = Vec::new();
    for (index, blocks) in pages_of_blocks.into_iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, None) {
            continue;
        }
        if budget.is_spent() {
            unread.push(number);
            continue;
        }
        let text = if blocks.is_empty() {
            "(no text on this page)".to_string()
        } else {
            blocks.join("\n\n")
        };
        let markdown = if total > 1 {
            format!("[page {number}]\n\n{text}")
        } else {
            text
        };
        budget.spend(&markdown);
        units.push(Unit { number, markdown });
    }
    Document {
        kind: KIND,
        unit: "page",
        total,
        units,
        notes,
        unread,
        selectors: Vec::new(),
    }
}

/// Style id to what it means, from `word/styles.xml`. Heading styles are found
/// by name ("heading 2", "Title") or outline level, so localized ids such as
/// `berschrift1` still count.
fn parse_styles(xml: &str) -> Result<Styles, String> {
    let mut styles = Styles::default();
    let mut id: Option<String> = None;
    let mut name = String::new();
    let mut outline: Option<usize> = None;
    let mut num_id: Option<String> = None;
    let mut num_level: Option<String> = None;
    walk(xml, |node| match node {
        Node::Open {
            name: "style",
            attrs,
        } => {
            id = attr(attrs, "w:styleId").map(str::to_string);
            name.clear();
            outline = None;
            num_id = None;
            num_level = None;
        }
        Node::Open {
            name: "name",
            attrs,
        } if id.is_some() => {
            name = attr(attrs, "w:val")
                .unwrap_or_default()
                .to_ascii_lowercase();
        }
        Node::Open {
            name: "outlineLvl",
            attrs,
        } if id.is_some() => {
            outline = attr(attrs, "w:val").and_then(|value| value.parse().ok());
        }
        Node::Open {
            name: "numId",
            attrs,
        } if id.is_some() => num_id = attr(attrs, "w:val").map(str::to_string),
        Node::Open {
            name: "ilvl",
            attrs,
        } if id.is_some() => num_level = attr(attrs, "w:val").map(str::to_string),
        Node::Close { name: "style" } => {
            if let Some(id) = id.take() {
                let kind = if name == "title" {
                    Some(StyleKind::Heading(1))
                } else if let Some(level) = name
                    .strip_prefix("heading ")
                    .and_then(|level| level.trim().parse::<usize>().ok())
                {
                    Some(StyleKind::Heading(level.clamp(1, 6)))
                } else if name.starts_with("list bullet") {
                    Some(StyleKind::Bullet)
                } else if name.starts_with("list number") {
                    Some(StyleKind::Numbered)
                } else {
                    outline
                        .filter(|level| *level < 9)
                        .map(|level| StyleKind::Heading((level + 1).min(6)))
                };
                if let Some(kind) = kind {
                    styles.kinds.insert(id.clone(), kind);
                }
                if let Some(num_id) = num_id.take().filter(|num_id| num_id != "0") {
                    styles.numbering.insert(id, (num_id, num_level.take()));
                }
            }
        }
        _ => {}
    })?;
    Ok(styles)
}

/// One level of a list definition in `word/numbering.xml`.
#[derive(Debug, Clone)]
struct Level {
    /// `w:numFmt`: `bullet`, `decimal`, `lowerLetter`, `upperRoman`, ...
    format: String,
    /// `w:lvlText`, such as `"%1.%2."`.
    text: Option<String>,
    start: u32,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            format: "bullet".to_string(),
            text: None,
            start: 1,
        }
    }
}

/// The list definitions of `word/numbering.xml`.
#[derive(Debug, Default)]
struct Numbering {
    abstract_of: HashMap<String, String>,
    levels: HashMap<(String, usize), Level>,
    /// List style id to the list definition that holds its levels
    /// (`w:abstractNum/w:styleLink`).
    style_links: HashMap<String, String>,
    /// List definition to the list style it uses (`w:numStyleLink`): its
    /// levels are those of the definition with that style link.
    uses_style: HashMap<String, String>,
    /// A list's restart of one level (`w:num/w:lvlOverride/w:startOverride`):
    /// `(numId, level)` to the number it starts again at.
    restarts: HashMap<(String, usize), u32>,
}

impl Numbering {
    fn level(&self, abstract_id: &str, level: usize) -> Option<&Level> {
        self.levels.get(&(abstract_id.to_string(), level))
    }

    /// The list definition that holds the levels of `abstract_id`: itself,
    /// or for one that only names a list style (Word's "Define New List
    /// Style"), the definition linked to that style, found directly or
    /// through the style's own numbering.
    fn resolve(&self, abstract_id: String, styles: &Styles) -> String {
        let Some(style) = self.uses_style.get(&abstract_id) else {
            return abstract_id;
        };
        if let Some(linked) = self.style_links.get(style) {
            return linked.clone();
        }
        styles
            .numbering
            .get(style)
            .and_then(|(num_id, _)| self.abstract_of.get(num_id))
            .filter(|linked| **linked != abstract_id && !self.uses_style.contains_key(*linked))
            .cloned()
            .unwrap_or(abstract_id)
    }

    fn is_bullet(&self, abstract_id: Option<&str>, level: usize) -> bool {
        abstract_id
            .and_then(|abstract_id| self.level(abstract_id, level))
            .is_none_or(|level| level.format == "bullet")
    }
}

fn parse_numbering(xml: &str) -> Result<Numbering, String> {
    let mut numbering = Numbering::default();
    let mut abstract_id: Option<String> = None;
    let mut level: Option<usize> = None;
    let mut num_id: Option<String> = None;
    let mut override_level: Option<usize> = None;
    walk(xml, |node| match node {
        Node::Open {
            name: "abstractNum",
            attrs,
        } => abstract_id = attr(attrs, "w:abstractNumId").map(str::to_string),
        Node::Open {
            name: name @ ("styleLink" | "numStyleLink"),
            attrs,
        } if level.is_none() => {
            if let (Some(abstract_id), Some(style)) = (&abstract_id, attr(attrs, "w:val")) {
                if name == "styleLink" {
                    numbering
                        .style_links
                        .entry(style.to_string())
                        .or_insert_with(|| abstract_id.clone());
                } else {
                    numbering
                        .uses_style
                        .insert(abstract_id.clone(), style.to_string());
                }
            }
        }
        Node::Open {
            name: "lvlOverride",
            attrs,
        } if num_id.is_some() => {
            override_level = attr(attrs, "w:ilvl")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|level| *level < LEVELS);
        }
        Node::Close {
            name: "lvlOverride",
        } => override_level = None,
        Node::Open {
            name: "startOverride",
            attrs,
        } => {
            if let (Some(num_id), Some(level), Some(start)) = (
                &num_id,
                override_level,
                attr(attrs, "w:val").and_then(|value| value.trim().parse::<u32>().ok()),
            ) {
                numbering.restarts.insert((num_id.clone(), level), start);
            }
        }
        Node::Close {
            name: "abstractNum",
        } => {
            abstract_id = None;
            level = None;
        }
        Node::Open { name: "lvl", attrs } if abstract_id.is_some() => {
            level = attr(attrs, "w:ilvl")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|level| *level < LEVELS);
            if let (Some(abstract_id), Some(level)) = (&abstract_id, level) {
                numbering
                    .levels
                    .insert((abstract_id.clone(), level), Level::default());
            }
        }
        Node::Close { name: "lvl" } => level = None,
        Node::Open { name, attrs } if matches!(name, "numFmt" | "lvlText" | "start") => {
            let (Some(abstract_id), Some(level)) = (&abstract_id, level) else {
                return;
            };
            let Some(entry) = numbering.levels.get_mut(&(abstract_id.clone(), level)) else {
                return;
            };
            let value = attr(attrs, "w:val").unwrap_or_default();
            match name {
                "numFmt" => entry.format = value.to_string(),
                "lvlText" => {
                    entry.text = Some(value.chars().take(MAX_LEVEL_TEXT_CHARS).collect());
                }
                _ => entry.start = value.parse().unwrap_or(1),
            }
        }
        Node::Open { name: "num", attrs } => num_id = attr(attrs, "w:numId").map(str::to_string),
        Node::Close { name: "num" } => num_id = None,
        Node::Open {
            name: "abstractNumId",
            attrs,
        } => {
            if let (Some(num_id), Some(value)) = (&num_id, attr(attrs, "w:val")) {
                numbering
                    .abstract_of
                    .insert(num_id.clone(), value.to_string());
            }
        }
        _ => {}
    })?;
    Ok(numbering)
}

/// List counters as Word keeps them while it lays the document out: one set
/// per list definition, deeper levels restarting when a shallower one moves.
#[derive(Debug, Default)]
struct Counters {
    values: HashMap<String, [Option<u32>; LEVELS]>,
    /// Lists whose restart of a level (`startOverride`) has been applied.
    restarted: HashSet<(String, usize)>,
}

impl Counters {
    /// Counts one paragraph at `level` of list `abstract_id` and returns its
    /// label, such as `"4.3.1"` or `"b)"`; `None` for bullets. A list
    /// (`num_id`) that restarts this level starts it again the first time
    /// one of its paragraphs reaches it.
    fn next(
        &mut self,
        numbering: &Numbering,
        abstract_id: &str,
        num_id: &str,
        level: usize,
    ) -> Option<String> {
        let level = level.min(LEVELS - 1);
        let restart = numbering
            .restarts
            .get(&(num_id.to_string(), level))
            .copied()
            .filter(|_| self.restarted.insert((num_id.to_string(), level)));
        let counts = self
            .values
            .entry(abstract_id.to_string())
            .or_insert([None; LEVELS]);
        let start = |level: usize| numbering.level(abstract_id, level).map_or(1, |l| l.start);
        counts[level] = Some(match restart {
            Some(value) => value,
            None => counts[level].map_or(start(level), |count| count.saturating_add(1)),
        });
        for deeper in counts.iter_mut().skip(level + 1) {
            *deeper = None;
        }
        let definition = numbering.level(abstract_id, level)?;
        if matches!(definition.format.as_str(), "bullet" | "none") {
            return None;
        }
        let template = definition
            .text
            .clone()
            .unwrap_or_else(|| format!("%{}.", level + 1));
        let mut label = String::new();
        let mut chars = template.chars().peekable();
        while let Some(ch) = chars.next() {
            if label.len() > MAX_LABEL_CHARS * 4 {
                break;
            }
            let digit = chars.peek().and_then(|next| next.to_digit(10));
            match (ch, digit) {
                ('%', Some(digit @ 1..=9)) => {
                    chars.next();
                    let index = usize::try_from(digit - 1).unwrap_or(0);
                    let value = counts[index].unwrap_or_else(|| start(index));
                    let format = numbering
                        .level(abstract_id, index)
                        .map_or("decimal", |level| level.format.as_str());
                    label.push_str(&format_number(value, format));
                }
                _ => label.push(ch),
            }
        }
        let label = label
            .trim()
            .chars()
            .take(MAX_LABEL_CHARS)
            .collect::<String>();
        (!label.is_empty()).then_some(label)
    }
}

/// A list counter in one of Word's number formats.
fn format_number(value: u32, format: &str) -> String {
    match format {
        "lowerLetter" => letters(value).to_ascii_lowercase(),
        "upperLetter" => letters(value),
        "lowerRoman" => roman(value).to_ascii_lowercase(),
        "upperRoman" => roman(value),
        "decimalZero" => format!("{value:02}"),
        _ => value.to_string(),
    }
}

/// 1 as `A`, 27 as `AA`: Word repeats the letter.
fn letters(value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let letter = char::from(b'A' + u8::try_from((value - 1) % 26).unwrap_or(0));
    let count = usize::try_from((value - 1) / 26 + 1).unwrap_or(1).min(8);
    letter.to_string().repeat(count)
}

fn roman(mut value: u32) -> String {
    if value == 0 || value >= 4000 {
        return value.to_string();
    }
    let mut out = String::new();
    for (amount, numeral) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= amount {
            out.push_str(numeral);
            value -= amount;
        }
    }
    out
}

#[derive(Debug, Default)]
struct Paragraph {
    page: usize,
    style: Option<String>,
    num_id: Option<String>,
    level: Option<String>,
    outline: Option<usize>,
    /// The paragraph ends a section that starts on a new page.
    section_break: bool,
    text: String,
    /// `SmartArt` diagrams in the paragraph, by relationship id.
    diagrams: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NoteKind {
    Foot,
    End,
}

impl NoteKind {
    fn plural(self) -> &'static str {
        match self {
            Self::Foot => "footnotes",
            Self::End => "endnotes",
        }
    }
}

/// A footnote or endnote reference in the body.
#[derive(Debug)]
struct NoteRef {
    kind: NoteKind,
    id: String,
    /// The mark shown in the text: footnotes `1`, `2`, ...; endnotes `i`,
    /// `ii`, ... as Word numbers them by default.
    label: String,
    page: usize,
    /// The index the block holding the reference takes in the body's
    /// blocks (the paragraph's, or its table's).
    anchor: usize,
}

#[derive(Debug)]
enum Block {
    Text {
        page: usize,
        markdown: String,
    },
    /// The rows of a table that start on one page.
    Table(TablePart),
}

impl Block {
    fn page(&self) -> usize {
        match self {
            Self::Text { page, .. } => *page,
            Self::Table(part) => part.page,
        }
    }

    /// Characters of [`Self::into_markdown`], without building it.
    fn chars(&self) -> usize {
        match self {
            Self::Text { markdown, .. } => markdown.chars().count(),
            Self::Table(part) => part.rendered_chars(),
        }
    }

    fn into_markdown(self) -> String {
        match self {
            Self::Text { markdown, .. } => markdown,
            Self::Table(part) => part.render(),
        }
    }
}

struct ParsedBody {
    blocks: Vec<Block>,
    pages: usize,
    /// Footnote and endnote references, in reading order, each note once.
    note_refs: Vec<NoteRef>,
    /// The `[^1]: text` block of each referenced note, with its reference's
    /// anchor (see [`NoteRef::anchor`]).
    note_blocks: Vec<(usize, Block)>,
}

/// Page counting while the body is walked.
struct Pager {
    page: usize,
    /// Whether Word saved `lastRenderedPageBreak` markers.
    rendered_breaks: bool,
    /// A section break just started a page; the rendered break Word usually
    /// writes before the next text marks that same page and is not counted.
    after_section_break: bool,
    /// Nothing has landed on the page a section break started.
    section_page_empty: bool,
}

impl Pager {
    fn new_page(&mut self, paragraphs: &mut [Paragraph]) {
        self.page += 1;
        self.section_page_empty = false;
        // A break before any text moves the paragraph to the new page.
        if let Some(paragraph) = paragraphs.last_mut() {
            if paragraph.text.trim().is_empty() {
                paragraph.page = self.page;
            }
        }
    }

    fn section_break(&mut self) {
        self.page += 1;
        self.after_section_break = true;
        self.section_page_empty = true;
    }

    fn rendered_break(&mut self, paragraphs: &mut [Paragraph]) {
        if self.after_section_break {
            self.after_section_break = false;
        } else {
            self.new_page(paragraphs);
        }
    }
}

#[allow(clippy::too_many_lines)]
fn parse_body(
    xml: &str,
    styles: &Styles,
    numbering: &Numbering,
    diagrams: &HashMap<String, Vec<(usize, String)>>,
) -> Result<ParsedBody, String> {
    // Word records where pages ended when it last laid the document out; use
    // those when present, else the explicit breaks.
    let mut pager = Pager {
        page: 1,
        rendered_breaks: xml.contains("lastRenderedPageBreak"),
        after_section_break: false,
        section_page_empty: false,
    };
    let mut counters = Counters::default();
    let mut blocks = Vec::new();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut tables: Vec<TableBuilder> = Vec::new();
    // Tables nested past MAX_TABLE_NESTING, still open.
    let mut ignored_tables = 0usize;
    let mut in_properties = false;
    let mut in_cell_properties = false;
    let mut in_row_properties = false;
    // Tracked changes' copies of old cell or row properties, still open.
    let mut property_changes = 0usize;
    // Tracked changes' copies of old paragraph, run or section properties,
    // still open: what the paragraph looked like before, not what it is.
    let mut paragraph_changes = 0usize;
    let mut in_section = false;
    let mut in_text = false;
    let mut skip_depth = 0usize;
    let mut note_refs: Vec<NoteRef> = Vec::new();
    let mut note_labels: HashMap<(NoteKind, String), String> = HashMap::new();
    let mut note_counts = [0u32; 2];

    walk(xml, |node| {
        if skip_depth > 0 {
            match node {
                Node::Open { .. } => skip_depth += 1,
                Node::Close { .. } => skip_depth -= 1,
                Node::Text(_) => {}
            }
            return;
        }
        match node {
            // Older copies of drawings and text boxes; the current copy is read.
            Node::Open {
                name: "Fallback", ..
            } => skip_depth = 1,
            Node::Open {
                name: "pPrChange" | "rPrChange" | "sectPrChange" | "numberingChange",
                ..
            } => paragraph_changes += 1,
            Node::Close {
                name: "pPrChange" | "rPrChange" | "sectPrChange" | "numberingChange",
            } => paragraph_changes = paragraph_changes.saturating_sub(1),
            // Inside a tracked change only the old properties are written.
            _ if paragraph_changes > 0 => {}
            Node::Open { name: "p", .. } => paragraphs.push(Paragraph {
                page: pager.page,
                ..Paragraph::default()
            }),
            Node::Open { name: "pPr", .. } => in_properties = true,
            Node::Close { name: "pPr" } => in_properties = false,
            Node::Close { name: "sectPr" } => in_section = false,
            Node::Open {
                name: name @ ("footnoteReference" | "endnoteReference"),
                attrs,
            } => {
                let Some(id) = attr(attrs, "w:id") else {
                    return;
                };
                let kind = if name == "footnoteReference" {
                    NoteKind::Foot
                } else {
                    NoteKind::End
                };
                let label = note_labels
                    .entry((kind, id.to_string()))
                    .or_insert_with(|| {
                        let count = &mut note_counts[usize::from(kind == NoteKind::End)];
                        *count = count.saturating_add(1);
                        let label = match kind {
                            NoteKind::Foot => count.to_string(),
                            NoteKind::End => roman(*count).to_ascii_lowercase(),
                        };
                        note_refs.push(NoteRef {
                            kind,
                            id: id.to_string(),
                            label: label.clone(),
                            page: pager.page,
                            anchor: blocks.len(),
                        });
                        label
                    });
                if let Some(paragraph) = paragraphs.last_mut() {
                    let _ = write!(paragraph.text, "[^{label}]");
                }
            }
            Node::Open {
                name: "relIds",
                attrs,
            } => {
                if let (Some(paragraph), Some(id)) = (paragraphs.last_mut(), attr(attrs, "r:dm")) {
                    paragraph.diagrams.push(id.to_string());
                }
            }
            Node::Open { name, attrs } if in_properties => {
                let value = attr(attrs, "w:val");
                if name == "pageBreakBefore"
                    && !pager.rendered_breaks
                    && !matches!(value, Some("0" | "false"))
                {
                    pager.new_page(&mut paragraphs);
                    return;
                }
                let Some(paragraph) = paragraphs.last_mut() else {
                    return;
                };
                match name {
                    // A section break in a paragraph's properties ends that
                    // section; unless it is continuous, the next starts a page.
                    "sectPr" => {
                        in_section = true;
                        paragraph.section_break = true;
                    }
                    "type" if in_section => {
                        paragraph.section_break = value != Some("continuous");
                    }
                    _ if in_section => {}
                    "pStyle" => paragraph.style = value.map(str::to_string),
                    "numId" => paragraph.num_id = value.map(str::to_string),
                    "ilvl" => paragraph.level = value.map(str::to_string),
                    "outlineLvl" => paragraph.outline = value.and_then(|value| value.parse().ok()),
                    _ => {}
                }
            }
            Node::Open { name: "t", .. } => in_text = true,
            Node::Close { name: "t" } => in_text = false,
            Node::Text(text) if in_text => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.text.push_str(text);
                    if !text.trim().is_empty() {
                        pager.after_section_break = false;
                    }
                }
            }
            Node::Open { name: "tab", .. } => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.text.push(' ');
                }
            }
            Node::Open {
                name: "br" | "cr",
                attrs,
            } => match attr(attrs, "w:type") {
                Some("page") => {
                    if !pager.rendered_breaks {
                        pager.new_page(&mut paragraphs);
                        if let Some(table) = tables.first_mut() {
                            table.page_break(pager.page);
                        }
                    }
                }
                Some("column") => {}
                _ => {
                    if let Some(paragraph) = paragraphs.last_mut() {
                        paragraph.text.push('\n');
                    }
                }
            },
            Node::Open {
                name: "noBreakHyphen",
                ..
            } => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.text.push('-');
                }
            }
            Node::Open {
                name: "lastRenderedPageBreak",
                ..
            } if pager.rendered_breaks => {
                pager.rendered_break(&mut paragraphs);
                // A row that starts on the new page belongs to it.
                if let Some(table) = tables.first_mut() {
                    table.page_break(pager.page);
                }
            }
            Node::Close { name: "p" } => {
                let Some(paragraph) = paragraphs.pop() else {
                    return;
                };
                let numbered = numbered(&paragraph, styles, numbering);
                let label = numbered.as_ref().and_then(|(abstract_id, level, num_id)| {
                    counters.next(numbering, abstract_id, num_id, *level)
                });
                let text = paragraph.text.trim();
                if !text.is_empty() {
                    if let Some(table) = tables.last_mut() {
                        table.push_line(text);
                    } else {
                        pager.section_page_empty = false;
                        blocks.push(Block::Text {
                            page: paragraph.page,
                            markdown: render_paragraph(
                                &paragraph,
                                text,
                                styles,
                                numbering,
                                numbered.as_ref(),
                                label.as_deref(),
                            ),
                        });
                    }
                }
                for id in &paragraph.diagrams {
                    let Some(lines) = diagrams.get(id).filter(|lines| !lines.is_empty()) else {
                        continue;
                    };
                    let markdown = render_diagram(lines);
                    if let Some(table) = tables.last_mut() {
                        table.push_line(&markdown);
                    } else {
                        pager.section_page_empty = false;
                        blocks.push(Block::Text {
                            page: paragraph.page,
                            markdown,
                        });
                    }
                }
                if paragraph.section_break && tables.is_empty() {
                    pager.section_break();
                }
            }
            Node::Open { name: "tbl", .. } => {
                if tables.len() >= MAX_TABLE_NESTING {
                    ignored_tables += 1;
                } else {
                    tables.push(TableBuilder::default());
                }
            }
            Node::Open { name: "tr", .. } if ignored_tables == 0 => {
                if let Some(table) = tables.last_mut() {
                    table.start_row(pager.page);
                }
            }
            Node::Open { name: "tc", .. } if ignored_tables == 0 => {
                if let Some(table) = tables.last_mut() {
                    table.start_cell();
                }
            }
            // Cell and row properties: where a row's cells sit on the
            // table's grid. A tracked change's copy of the old properties
            // is not the current layout.
            Node::Open { name: "tcPr", .. } => in_cell_properties = true,
            Node::Close { name: "tcPr" } => in_cell_properties = false,
            Node::Open { name: "trPr", .. } => in_row_properties = true,
            Node::Close { name: "trPr" } => in_row_properties = false,
            Node::Open {
                name: "tcPrChange" | "trPrChange",
                ..
            } => property_changes += 1,
            Node::Close {
                name: "tcPrChange" | "trPrChange",
            } => property_changes = property_changes.saturating_sub(1),
            Node::Open {
                name: name @ ("gridSpan" | "gridBefore" | "gridAfter"),
                attrs,
            } if ignored_tables == 0 && property_changes == 0 => {
                let count = attr(attrs, "w:val")
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if let Some(table) = tables.last_mut() {
                    match name {
                        "gridSpan" if in_cell_properties => table.set_cell_span(count),
                        "gridBefore" if in_row_properties => table.skip_columns(count),
                        "gridAfter" if in_row_properties => table.set_row_grid_after(count),
                        _ => {}
                    }
                }
            }
            Node::Close { name: "tc" } if ignored_tables == 0 => {
                if let Some(table) = tables.last_mut() {
                    table.end_cell();
                }
            }
            Node::Close { name: "tr" } if ignored_tables == 0 => {
                if let Some(table) = tables.last_mut() {
                    table.end_row();
                }
            }
            Node::Close { name: "tbl" } => {
                if ignored_tables > 0 {
                    ignored_tables -= 1;
                    return;
                }
                let Some(table) = tables.pop() else {
                    return;
                };
                if let Some(parent) = tables.last_mut() {
                    // A table inside a cell joins the cell as plain text.
                    parent.push_line(&table.flatten());
                    return;
                }
                // One block per page the table's rows start on, so a long
                // table's later rows are on the pages Word put them.
                for part in table.into_parts() {
                    pager.section_page_empty = false;
                    blocks.push(Block::Table(part));
                }
            }
            _ => {}
        }
    })?;
    // A section break in the last paragraph starts no real page.
    let page_count = if pager.section_page_empty && pager.page > 1 {
        pager.page - 1
    } else {
        pager.page
    };
    // A note referenced on a page that a trailing section break opened
    // belongs to the last real page.
    for note in &mut note_refs {
        note.page = note.page.min(page_count);
    }
    Ok(ParsedBody {
        blocks,
        pages: page_count,
        note_refs,
        note_blocks: Vec::new(),
    })
}

/// The list a paragraph belongs to, its own or its style's: abstract list id
/// (the one holding its levels), level and list id. `None` when it is not in
/// a list.
fn numbered(
    paragraph: &Paragraph,
    styles: &Styles,
    numbering: &Numbering,
) -> Option<(String, usize, String)> {
    let from_style = paragraph
        .style
        .as_deref()
        .and_then(|style| styles.numbering.get(style));
    let num_id = paragraph
        .num_id
        .as_deref()
        .or_else(|| from_style.map(|(num_id, _)| num_id.as_str()))
        .filter(|num_id| *num_id != "0")?;
    let level = paragraph
        .level
        .as_deref()
        .or_else(|| from_style.and_then(|(_, level)| level.as_deref()))
        .and_then(|level| level.parse::<usize>().ok())
        .unwrap_or(0)
        .min(LEVELS - 1);
    let abstract_id = numbering.abstract_of.get(num_id).cloned().map_or_else(
        || format!("num:{num_id}"),
        |abstract_id| numbering.resolve(abstract_id, styles),
    );
    Some((abstract_id, level, num_id.to_string()))
}

fn render_paragraph(
    paragraph: &Paragraph,
    text: &str,
    styles: &Styles,
    numbering: &Numbering,
    numbered: Option<&(String, usize, String)>,
    label: Option<&str>,
) -> String {
    let style = paragraph.style.as_deref().and_then(|style| {
        styles
            .kinds
            .get(style)
            .copied()
            .or_else(|| fallback_style(style))
    });
    let heading = match style {
        Some(StyleKind::Heading(level)) => Some(level),
        Some(_) => None,
        None => paragraph
            .outline
            .filter(|level| *level < 9)
            .map(|level| (level + 1).min(6)),
    };
    // A heading stays a heading even when it is numbered (4.3.1 ...).
    if let Some(level) = heading {
        let prefix = label.map_or_else(String::new, |label| format!("{label} "));
        return format!("{} {prefix}{}", "#".repeat(level), one_line(text));
    }
    let level = numbered.map_or_else(
        || {
            paragraph
                .level
                .as_deref()
                .and_then(|level| level.parse::<usize>().ok())
                .unwrap_or(0)
                .min(LEVELS - 1)
        },
        |(_, level, _)| *level,
    );
    let indent = "  ".repeat(level);
    if let Some((abstract_id, level, _)) = numbered {
        let marker = if numbering.is_bullet(Some(abstract_id), *level) {
            "-"
        } else {
            label.unwrap_or("1.")
        };
        return format!("{indent}{marker} {}", one_line(text));
    }
    match style {
        Some(StyleKind::Bullet) => format!("{indent}- {}", one_line(text)),
        Some(StyleKind::Numbered) => format!("{indent}1. {}", one_line(text)),
        _ => text.to_string(),
    }
}

/// Built-in style ids, for files that ship without `styles.xml`.
fn fallback_style(style: &str) -> Option<StyleKind> {
    let lower = style.to_ascii_lowercase();
    if lower == "title" {
        return Some(StyleKind::Heading(1));
    }
    lower
        .strip_prefix("heading")
        .and_then(|level| level.parse::<usize>().ok())
        .map(|level| StyleKind::Heading(level.clamp(1, 6)))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
