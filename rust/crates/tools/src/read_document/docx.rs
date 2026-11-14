//! Word documents (`.docx`): headings, paragraphs, lists and tables.
//!
//! A `.docx` holds no fixed pages; pages here follow the page breaks Word saved
//! with the file (`lastRenderedPageBreak`), else explicit page breaks, else
//! about [`ESTIMATED_PAGE_CHARS`] characters each.

use std::collections::HashMap;
use std::path::Path;

use super::xml::{attr, walk, Node, Package};
use super::{markdown_table, Document, Selection, Unit};

const KIND: &str = "Word document";

/// Characters per page when a document carries no page breaks at all.
pub(super) const ESTIMATED_PAGE_CHARS: usize = 3_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StyleKind {
    Heading(usize),
    Bullet,
    Numbered,
}

pub(super) fn read(path: &Path, selection: &Selection) -> Result<Document, String> {
    let mut package = Package::open(path, KIND)?;
    let body = package
        .read_part("word/document.xml")?
        .ok_or_else(|| format!("this is not a readable {KIND}: word/document.xml is missing"))?;
    let styles = match package.read_part("word/styles.xml")? {
        Some(xml) => parse_styles(&xml)?,
        None => HashMap::new(),
    };
    let numbering = match package.read_part("word/numbering.xml")? {
        Some(xml) => parse_numbering(&xml)?,
        None => Numbering::default(),
    };
    let parsed = parse_body(&body, &styles, &numbering)?;
    Ok(paginate(parsed, selection))
}

fn paginate(parsed: ParsedBody, selection: &Selection) -> Document {
    let ParsedBody { blocks, pages } = parsed;
    let mut notes = Vec::new();
    let mut pages_of_blocks: Vec<Vec<String>> = Vec::new();
    if pages > 1 {
        pages_of_blocks.resize(pages, Vec::new());
        for block in blocks {
            pages_of_blocks[block.page.clamp(1, pages) - 1].push(block.markdown);
        }
        notes.push("page numbers follow the page breaks saved with the file and may differ slightly from Word".to_string());
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
            notes.push(format!(
                "the file has no saved page breaks; pages are estimated at about {ESTIMATED_PAGE_CHARS} characters each"
            ));
        }
    }
    let total = pages_of_blocks.len();
    let units = pages_of_blocks
        .into_iter()
        .enumerate()
        .filter(|(index, _)| selection.includes(index + 1, None))
        .map(|(index, blocks)| {
            let number = index + 1;
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
            Unit { number, markdown }
        })
        .collect();
    Document {
        kind: KIND,
        unit: "page",
        total,
        units,
        notes,
    }
}

/// Style id to what it means, from `word/styles.xml`. Heading styles are found
/// by name ("heading 2", "Title") or outline level, so localized ids such as
/// `berschrift1` still count.
fn parse_styles(xml: &str) -> Result<HashMap<String, StyleKind>, String> {
    let mut styles = HashMap::new();
    let mut id: Option<String> = None;
    let mut name = String::new();
    let mut outline: Option<usize> = None;
    walk(xml, |node| match node {
        Node::Open {
            name: "style",
            attrs,
        } => {
            id = attr(attrs, "w:styleId").map(str::to_string);
            name.clear();
            outline = None;
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
                    styles.insert(id, kind);
                }
            }
        }
        _ => {}
    })?;
    Ok(styles)
}

/// Which list levels are bulleted, from `word/numbering.xml`.
#[derive(Debug, Default)]
struct Numbering {
    abstract_of: HashMap<String, String>,
    bullet: HashMap<(String, String), bool>,
}

impl Numbering {
    fn is_bullet(&self, num_id: &str, level: &str) -> bool {
        self.abstract_of
            .get(num_id)
            .and_then(|abstract_id| self.bullet.get(&(abstract_id.clone(), level.to_string())))
            .copied()
            .unwrap_or(true)
    }
}

fn parse_numbering(xml: &str) -> Result<Numbering, String> {
    let mut numbering = Numbering::default();
    let mut abstract_id: Option<String> = None;
    let mut level: Option<String> = None;
    let mut num_id: Option<String> = None;
    walk(xml, |node| match node {
        Node::Open {
            name: "abstractNum",
            attrs,
        } => abstract_id = attr(attrs, "w:abstractNumId").map(str::to_string),
        Node::Close {
            name: "abstractNum",
        } => abstract_id = None,
        Node::Open { name: "lvl", attrs } if abstract_id.is_some() => {
            level = attr(attrs, "w:ilvl").map(str::to_string);
        }
        Node::Open {
            name: "numFmt",
            attrs,
        } => {
            if let (Some(abstract_id), Some(level)) = (&abstract_id, &level) {
                numbering.bullet.insert(
                    (abstract_id.clone(), level.clone()),
                    attr(attrs, "w:val") == Some("bullet"),
                );
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

#[derive(Debug, Default)]
struct Paragraph {
    page: usize,
    style: Option<String>,
    num_id: Option<String>,
    level: Option<String>,
    outline: Option<usize>,
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

#[allow(clippy::too_many_lines)]
fn parse_body(
    xml: &str,
    styles: &HashMap<String, StyleKind>,
    numbering: &Numbering,
) -> Result<ParsedBody, String> {
    // Word records where pages ended when it last laid the document out; use
    // those when present, else the explicit breaks.
    let rendered_breaks = xml.contains("lastRenderedPageBreak");
    let mut page = 1;
    let mut blocks = Vec::new();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    let mut in_properties = false;
    let mut in_text = false;
    let mut skip_depth = 0usize;

    let new_page = |page: &mut usize, paragraphs: &mut Vec<Paragraph>| {
        *page += 1;
        // A break before any text moves the paragraph to the new page.
        if let Some(paragraph) = paragraphs.last_mut() {
            if paragraph.text.trim().is_empty() {
                paragraph.page = *page;
            }
        }
    };

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
                page,
                ..Paragraph::default()
            }),
            Node::Open { name: "pPr", .. } => in_properties = true,
            Node::Close { name: "pPr" } => in_properties = false,
            Node::Open { name, attrs } if in_properties => {
                let value = attr(attrs, "w:val");
                if name == "pageBreakBefore"
                    && !rendered_breaks
                    && !matches!(value, Some("0" | "false"))
                {
                    new_page(&mut page, &mut paragraphs);
                    return;
                }
                let Some(paragraph) = paragraphs.last_mut() else {
                    return;
                };
                match name {
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
                    if !rendered_breaks {
                        new_page(&mut page, &mut paragraphs);
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
            } if rendered_breaks => new_page(&mut page, &mut paragraphs),
            Node::Close { name: "p" } => {
                let Some(paragraph) = paragraphs.pop() else {
                    return;
                };
                let text = paragraph.text.trim();
                if text.is_empty() {
                    return;
                }
                if let Some(table) = tables.last_mut() {
                    if let Some(cell) = table.rows.last_mut().and_then(|row| row.last_mut()) {
                        if !cell.is_empty() {
                            cell.push('\n');
                        }
                        cell.push_str(text);
                    }
                    return;
                }
                blocks.push(Block {
                    page: paragraph.page,
                    markdown: render_paragraph(&paragraph, text, styles, numbering),
                });
            }
            Node::Open { name: "tbl", .. } => tables.push(Table {
                page,
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
                    blocks.push(Block {
                        page: table.page,
                        markdown,
                    });
                }
            }
            _ => {}
        }
    })?;
    Ok(ParsedBody {
        blocks,
        pages: page,
    })
}

fn render_paragraph(
    paragraph: &Paragraph,
    text: &str,
    styles: &HashMap<String, StyleKind>,
    numbering: &Numbering,
) -> String {
    let style = paragraph
        .style
        .as_deref()
        .and_then(|style| styles.get(style).copied().or_else(|| fallback_style(style)));
    let level = paragraph
        .level
        .as_deref()
        .and_then(|level| level.parse::<usize>().ok())
        .unwrap_or(0)
        .min(8);
    let indent = "  ".repeat(level);
    if let Some(num_id) = paragraph.num_id.as_deref().filter(|id| *id != "0") {
        let marker = if numbering.is_bullet(num_id, &level.to_string()) {
            "-"
        } else {
            "1."
        };
        return format!("{indent}{marker} {}", one_line(text));
    }
    match style {
        Some(StyleKind::Heading(level)) => format!("{} {}", "#".repeat(level), one_line(text)),
        Some(StyleKind::Bullet) => format!("{indent}- {}", one_line(text)),
        Some(StyleKind::Numbered) => format!("{indent}1. {}", one_line(text)),
        None => match paragraph.outline.filter(|level| *level < 9) {
            Some(level) => format!("{} {}", "#".repeat((level + 1).min(6)), one_line(text)),
            None => text.to_string(),
        },
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
