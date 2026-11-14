//! Word documents (`.docx`): headings, paragraphs, lists and tables.
//!
//! A `.docx` holds no fixed pages; pages here follow the page breaks Word saved
//! with the file (`lastRenderedPageBreak`) plus section breaks that start a new
//! page, else explicit page breaks, else about [`ESTIMATED_PAGE_CHARS`]
//! characters each.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use super::xml::{attr, walk, Node, Package};
use super::{markdown_table, Budget, Document, Selection, Unit};

const KIND: &str = "Word document";

/// Characters per page when a document carries no page breaks at all.
pub(super) const ESTIMATED_PAGE_CHARS: usize = 3_000;

/// List levels Word supports (`w:ilvl` 0 to 8).
const LEVELS: usize = 9;

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
        .read_part("word/document.xml")?
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
    let parsed = parse_body(&body, &styles, &numbering)?;
    Ok(paginate(parsed, selection, saved_pages, budget))
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
    let ParsedBody { blocks, pages } = parsed;
    let mut notes = Vec::new();
    let mut pages_of_blocks: Vec<Vec<String>> = Vec::new();
    if pages > 1 {
        pages_of_blocks.resize(pages, Vec::new());
        for block in blocks {
            pages_of_blocks[block.page.clamp(1, pages) - 1].push(block.markdown);
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
        for block in blocks {
            if size >= ESTIMATED_PAGE_CHARS && !current.is_empty() {
                pages_of_blocks.push(std::mem::take(&mut current));
                size = 0;
            }
            size += block.markdown.chars().count();
            current.push(block.markdown);
        }
        if !current.is_empty() || pages_of_blocks.is_empty() {
            pages_of_blocks.push(current);
        }
        if pages_of_blocks.len() > 1 {
            let mut note = format!(
                "the file has no saved page breaks; pages are estimated at about {ESTIMATED_PAGE_CHARS} characters each"
            );
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
}

impl Numbering {
    fn level(&self, abstract_id: &str, level: usize) -> Option<&Level> {
        self.levels.get(&(abstract_id.to_string(), level))
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
    walk(xml, |node| match node {
        Node::Open {
            name: "abstractNum",
            attrs,
        } => abstract_id = attr(attrs, "w:abstractNumId").map(str::to_string),
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
                "lvlText" => entry.text = Some(value.to_string()),
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
}

impl Counters {
    /// Counts one paragraph at `level` of list `abstract_id` and returns its
    /// label, such as `"4.3.1"` or `"b)"`; `None` for bullets.
    fn next(&mut self, numbering: &Numbering, abstract_id: &str, level: usize) -> Option<String> {
        let level = level.min(LEVELS - 1);
        let counts = self
            .values
            .entry(abstract_id.to_string())
            .or_insert([None; LEVELS]);
        let start = |level: usize| numbering.level(abstract_id, level).map_or(1, |l| l.start);
        counts[level] = Some(counts[level].map_or(start(level), |count| count.saturating_add(1)));
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
        let label = label.trim().to_string();
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
}

#[derive(Debug, Default)]
struct Table {
    page: usize,
    rows: Vec<Vec<String>>,
}

#[derive(Debug)]
struct Block {
    page: usize,
    markdown: String,
}

struct ParsedBody {
    blocks: Vec<Block>,
    pages: usize,
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
fn parse_body(xml: &str, styles: &Styles, numbering: &Numbering) -> Result<ParsedBody, String> {
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
    let mut tables: Vec<Table> = Vec::new();
    let mut in_properties = false;
    let mut in_section = false;
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
            // Older copies of drawings and text boxes; the current copy is read.
            Node::Open {
                name: "Fallback", ..
            } => skip_depth = 1,
            Node::Open { name: "p", .. } => paragraphs.push(Paragraph {
                page: pager.page,
                ..Paragraph::default()
            }),
            Node::Open { name: "pPr", .. } => in_properties = true,
            Node::Close { name: "pPr" } => in_properties = false,
            Node::Close { name: "sectPr" } => in_section = false,
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
            } if pager.rendered_breaks => pager.rendered_break(&mut paragraphs),
            Node::Close { name: "p" } => {
                let Some(paragraph) = paragraphs.pop() else {
                    return;
                };
                let numbered = numbered(&paragraph, styles, numbering);
                let label = numbered
                    .as_ref()
                    .and_then(|(abstract_id, level)| counters.next(numbering, abstract_id, *level));
                let text = paragraph.text.trim();
                if !text.is_empty() {
                    if let Some(table) = tables.last_mut() {
                        if let Some(cell) = table.rows.last_mut().and_then(|row| row.last_mut()) {
                            if !cell.is_empty() {
                                cell.push('\n');
                            }
                            cell.push_str(text);
                        }
                    } else {
                        pager.section_page_empty = false;
                        blocks.push(Block {
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
                if paragraph.section_break && tables.is_empty() {
                    pager.section_break();
                }
            }
            Node::Open { name: "tbl", .. } => tables.push(Table {
                page: pager.page,
                rows: Vec::new(),
            }),
            Node::Open { name: "tr", .. } => {
                if let Some(table) = tables.last_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Node::Open { name: "tc", .. } => {
                if let Some(row) = tables.last_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(String::new());
                }
            }
            Node::Close { name: "tbl" } => {
                let Some(table) = tables.pop() else {
                    return;
                };
                if let Some(parent) = tables.last_mut() {
                    // A table inside a cell joins the cell as plain text.
                    let flat = table
                        .rows
                        .iter()
                        .map(|row| row.join(" / "))
                        .collect::<Vec<_>>()
                        .join("; ");
                    if let Some(cell) = parent.rows.last_mut().and_then(|row| row.last_mut()) {
                        if !cell.is_empty() {
                            cell.push('\n');
                        }
                        cell.push_str(&flat);
                    }
                    return;
                }
                let markdown = markdown_table(&table.rows);
                if !markdown.is_empty() {
                    pager.section_page_empty = false;
                    blocks.push(Block {
                        page: table.page,
                        markdown,
                    });
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
    Ok(ParsedBody {
        blocks,
        pages: page_count,
    })
}

/// The list a paragraph belongs to, its own or its style's: abstract list id
/// and level. `None` when it is not in a list.
fn numbered(
    paragraph: &Paragraph,
    styles: &Styles,
    numbering: &Numbering,
) -> Option<(String, usize)> {
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
    let abstract_id = numbering
        .abstract_of
        .get(num_id)
        .cloned()
        .unwrap_or_else(|| format!("num:{num_id}"));
    Some((abstract_id, level))
}

fn render_paragraph(
    paragraph: &Paragraph,
    text: &str,
    styles: &Styles,
    numbering: &Numbering,
    numbered: Option<&(String, usize)>,
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
        |(_, level)| *level,
    );
    let indent = "  ".repeat(level);
    if let Some((abstract_id, level)) = numbered {
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
