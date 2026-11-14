//! `PowerPoint` presentations (`.pptx`): each slide with its title, text,
//! tables and speaker notes, in presentation order.

use std::path::Path;

use super::xml::{attr, part_dir, relationships, rels_path, resolve_target, walk, Node, Package};
use super::{markdown_table, Budget, Document, Selection, Unit};

const KIND: &str = "PowerPoint presentation";

pub(super) fn read(
    path: &Path,
    selection: &Selection,
    budget: &mut Budget,
) -> Result<Document, String> {
    let mut package = Package::open(path, KIND)?;
    let presentation = package
        .read_part("ppt/presentation.xml")?
        .ok_or_else(|| format!("this is not a readable {KIND}: ppt/presentation.xml is missing"))?;
    let rels = package
        .read_part("ppt/_rels/presentation.xml.rels")?
        .map(|xml| relationships(&xml))
        .transpose()?
        .unwrap_or_default();

    let mut slide_ids = Vec::new();
    walk(&presentation, |node| {
        if let Node::Open {
            name: "sldId",
            attrs,
        } = node
        {
            if let Some(id) = attr(attrs, "r:id") {
                slide_ids.push(id.to_string());
            }
        }
    })?;
    let slide_parts = slide_ids
        .iter()
        .filter_map(|id| {
            rels.iter()
                .find(|(rel_id, _, _)| rel_id == id)
                .map(|(_, _, target)| resolve_target("ppt", target))
        })
        .collect::<Vec<_>>();

    let mut units = Vec::new();
    let mut unread = Vec::new();
    for (index, part) in slide_parts.iter().enumerate() {
        let number = index + 1;
        if !selection.includes(number, None) {
            continue;
        }
        if budget.is_spent() {
            // Enough text for this call: name the rest without reading it.
            unread.push(number);
            continue;
        }
        let markdown = match package.read_part(part)? {
            Some(xml) => {
                let slide = parse_slide(&xml)?;
                let notes = read_notes(&mut package, part)?;
                render_slide(number, &slide, notes.as_deref())
            }
            None => format!("## Slide {number}\n\n(the slide is missing from the file)"),
        };
        budget.spend(&markdown);
        units.push(Unit { number, markdown });
    }
    Ok(Document {
        kind: KIND,
        unit: "slide",
        total: slide_parts.len(),
        units,
        notes: Vec::new(),
        unread,
    })
}

fn read_notes(package: &mut Package, slide_part: &str) -> Result<Option<String>, String> {
    let Some(rels) = package.read_part(&rels_path(slide_part))? else {
        return Ok(None);
    };
    let Some(target) = relationships(&rels)?
        .into_iter()
        .find(|(_, kind, _)| kind.ends_with("/notesSlide"))
        .map(|(_, _, target)| resolve_target(part_dir(slide_part), &target))
    else {
        return Ok(None);
    };
    let Some(xml) = package.read_part(&target)? else {
        return Ok(None);
    };
    let notes = parse_slide(&xml)?;
    let text = notes
        .shapes
        .iter()
        .filter(|shape| shape.placeholder.as_deref() == Some("body"))
        .flat_map(|shape| shape.paragraphs.iter().map(|(_, text)| text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok((!text.trim().is_empty()).then_some(text))
}

#[derive(Debug, Default)]
struct Shape {
    /// The placeholder type (`title`, `body`, ...); `None` for a free text box.
    placeholder: Option<String>,
    paragraphs: Vec<(usize, String)>,
}

#[derive(Debug, Default)]
struct Slide {
    hidden: bool,
    shapes: Vec<Shape>,
    /// Shapes and tables in reading order: an index into `shapes`, or a table.
    order: Vec<Content>,
}

#[derive(Debug)]
enum Content {
    Shape(usize),
    Table(Vec<Vec<String>>),
}

fn parse_slide(xml: &str) -> Result<Slide, String> {
    let mut slide = Slide::default();
    let mut shapes: Vec<Shape> = Vec::new();
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut paragraph: Option<(usize, String)> = None;
    let mut in_text = false;
    walk(xml, |node| match node {
        Node::Open { name: "sld", attrs } => slide.hidden = attr(attrs, "show") == Some("0"),
        Node::Open { name: "sp", .. } => shapes.push(Shape::default()),
        Node::Open { name: "ph", attrs } => {
            if let Some(shape) = shapes.last_mut() {
                shape.placeholder = Some(attr(attrs, "type").unwrap_or("body").to_string());
            }
        }
        Node::Close { name: "sp" } => {
            if let Some(shape) = shapes.pop() {
                slide.order.push(Content::Shape(slide.shapes.len()));
                slide.shapes.push(shape);
            }
        }
        Node::Open { name: "p", .. } => paragraph = Some((0, String::new())),
        Node::Open { name: "pPr", attrs } => {
            if let Some((level, _)) = paragraph.as_mut() {
                *level = attr(attrs, "lvl")
                    .and_then(|level| level.parse().ok())
                    .unwrap_or(0);
            }
        }
        Node::Open { name: "t", .. } => in_text = true,
        Node::Close { name: "t" } => in_text = false,
        Node::Text(text) if in_text => {
            if let Some((_, buffer)) = paragraph.as_mut() {
                buffer.push_str(text);
            }
        }
        Node::Open { name: "br", .. } => {
            if let Some((_, buffer)) = paragraph.as_mut() {
                buffer.push('\n');
            }
        }
        Node::Close { name: "p" } => {
            let Some((level, text)) = paragraph.take() else {
                return;
            };
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            if let Some(cell) = tables
                .last_mut()
                .and_then(|table| table.last_mut())
                .and_then(|row| row.last_mut())
            {
                if !cell.is_empty() {
                    cell.push('\n');
                }
                cell.push_str(&text);
            } else if let Some(shape) = shapes.last_mut() {
                shape.paragraphs.push((level, text));
            }
        }
        Node::Open { name: "tbl", .. } => tables.push(Vec::new()),
        Node::Open { name: "tr", .. } => {
            if let Some(table) = tables.last_mut() {
                table.push(Vec::new());
            }
        }
        Node::Open { name: "tc", .. } => {
            if let Some(row) = tables.last_mut().and_then(|table| table.last_mut()) {
                row.push(String::new());
            }
        }
        Node::Close { name: "tbl" } => {
            if let Some(table) = tables.pop() {
                slide.order.push(Content::Table(table));
            }
        }
        _ => {}
    })?;
    Ok(slide)
}

fn is_title(shape: &Shape) -> bool {
    matches!(shape.placeholder.as_deref(), Some("title" | "ctrTitle"))
}

fn render_slide(number: usize, slide: &Slide, notes: Option<&str>) -> String {
    let title = slide
        .shapes
        .iter()
        .find(|shape| is_title(shape) && !shape.paragraphs.is_empty())
        .map(|shape| {
            shape
                .paragraphs
                .iter()
                .map(|(_, text)| text.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" ")
        });
    let mut lines = vec![match &title {
        Some(title) => format!("## Slide {number}: {title}"),
        None => format!("## Slide {number}"),
    }];
    if slide.hidden {
        lines.push("(hidden slide)".to_string());
    }
    let mut body = Vec::new();
    for content in &slide.order {
        match content {
            Content::Shape(index) => {
                let shape = &slide.shapes[*index];
                if is_title(shape)
                    || matches!(
                        shape.placeholder.as_deref(),
                        Some("sldNum" | "dt" | "ftr" | "hdr")
                    )
                {
                    continue;
                }
                let bulleted = shape.placeholder.is_some() || shape.paragraphs.len() > 1;
                let text = shape
                    .paragraphs
                    .iter()
                    .map(|(level, text)| {
                        if bulleted {
                            format!(
                                "{}- {}",
                                "  ".repeat((*level).min(8)),
                                text.replace('\n', " ")
                            )
                        } else {
                            text.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    body.push(text);
                }
            }
            Content::Table(rows) => {
                let table = markdown_table(rows);
                if !table.is_empty() {
                    body.push(table);
                }
            }
        }
    }
    if body.is_empty() && title.is_none() {
        body.push("(no text on this slide)".to_string());
    }
    lines.extend(body);
    if let Some(notes) = notes {
        lines.push(format!("Notes:\n{}", notes.trim()));
    }
    lines.join("\n\n")
}
