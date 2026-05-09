//! Tool call and tool result UI formatting.
//!
//! Every function that renders a tool invocation or its result lives here.
//! Extracted from the main module for maintainability.

use crate::brand::*;
use std::path::Path;

pub fn format_tool_call_start(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));

    let detail = match name {
        "bash" | "Bash" => format_bash_call(&parsed),
        "read_file" | "Read" => {
            let path = extract_tool_path(&parsed);
            format!("{ICON_FILE} {DIM}Reading {path}{R}")
        }
        "write_file" | "Write" => {
            let path = extract_tool_path(&parsed);
            let lines = parsed
                .get("content")
                .and_then(|value| value.as_str())
                .map_or(0, |content| content.lines().count());
            format!("{ICON_WRITE} {GREEN}{BOLD}Writing {path}{R} {DIM}({lines} lines){R}")
        }
        "edit_file" | "Edit" => {
            let path = extract_tool_path(&parsed);
            let old_value = parsed
                .get("old_string")
                .or_else(|| parsed.get("oldString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let new_value = parsed
                .get("new_string")
                .or_else(|| parsed.get("newString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            format!(
                "{ICON_EDIT} {ORANGE}{BOLD}Editing {path}{R}{}",
                format_patch_preview(old_value, new_value)
                    .map(|preview| format!("\n{BLUE}\u{2502}{R}  {preview}"))
                    .unwrap_or_default()
            )
        }
        "glob_search" | "Glob" => {
            format_search_start(&format!("{ICON_SEARCH} {BLUE}Glob{R}"), &parsed)
        }
        "grep_search" | "Grep" => {
            format_search_start(&format!("{ICON_SEARCH} {BLUE}Grep{R}"), &parsed)
        }
        "web_search" | "WebSearch" => {
            let query = parsed
                .get("query")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            format!("{ICON_WEB} {ORANGE}Searching:{R} {query}")
        }
        _ => summarize_tool_payload(input),
    };

    // Full-width expanding box with aligned borders
    let tw = term_width().saturating_sub(2).max(30);
    let label = format!(" {name} ");
    let label_len = strip_ansi_len(&format!("{ORANGE}{BOLD}{label}{R}"));
    let left_pad = 3; // "───"
    let right_pad = tw.saturating_sub(left_pad + label_len);
    format!(
        "{BLUE}\u{256d}{left}{ORANGE}{BOLD}{label}{R}{BLUE}{right}\u{256e}{R}\n{BLUE}\u{2502}{R}  {detail}\n{BLUE}\u{2570}{bottom}\u{256f}{R}",
        left = "\u{2500}".repeat(left_pad),
        right = "\u{2500}".repeat(right_pad),
        bottom = "\u{2500}".repeat(tw.saturating_sub(2)),
    )
}

pub fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let icon = if is_error { ICON_ERR } else { ICON_OK };

    if is_error {
        let summary = truncate_for_summary(output.trim(), 160);
        return if summary.is_empty() {
            format!("{icon} {DIM}{name}{R}")
        } else {
            format!("{icon} {DIM}{name}{R}\n{RED}{summary}{R}")
        };
    }

    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_result(icon, &parsed),
        "read_file" | "Read" => format_read_result(icon, &parsed),
        "write_file" | "Write" => format_write_result(icon, &parsed),
        "edit_file" | "Edit" => format_edit_result(icon, &parsed),
        "glob_search" | "Glob" => format_glob_result(icon, &parsed),
        "grep_search" | "Grep" => format_grep_result(icon, &parsed),
        _ => format_generic_tool_result(icon, name, &parsed),
    }
}

pub const DISPLAY_TRUNCATION_NOTICE: &str =
    "\x1b[2m\u{2026} output truncated for display; full result preserved in session.\x1b[0m";
pub const READ_DISPLAY_MAX_LINES: usize = 80;
pub const READ_DISPLAY_MAX_CHARS: usize = 6_000;
pub const TOOL_OUTPUT_DISPLAY_MAX_LINES: usize = 60;
pub const TOOL_OUTPUT_DISPLAY_MAX_CHARS: usize = 4_000;

pub fn extract_tool_path(parsed: &serde_json::Value) -> String {
    let raw = parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    display_clean_path(raw)
}

/// Strip Windows `\\?\` UNC prefix and convert to relative path for display.
pub fn display_clean_path(raw: &str) -> String {
    // Strip the \\?\ extended-length prefix that Windows APIs inject
    let cleaned = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    // Try to make it relative to the current working directory
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = Path::new(cleaned).strip_prefix(&cwd) {
            let rel_str = rel.display().to_string();
            return if rel_str.is_empty() {
                ".".to_string()
            } else {
                rel_str
            };
        }
    }
    cleaned.to_string()
}

pub fn format_search_start(label: &str, parsed: &serde_json::Value) -> String {
    let pattern = parsed
        .get("pattern")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let scope = parsed
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    format!("{label} {pattern}\n\x1b[2min {scope}\x1b[0m")
}

pub fn format_patch_preview(old_value: &str, new_value: &str) -> Option<String> {
    if old_value.is_empty() && new_value.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    let old_lines: Vec<&str> = old_value.lines().filter(|l| !l.trim().is_empty()).collect();
    let new_lines: Vec<&str> = new_value.lines().filter(|l| !l.trim().is_empty()).collect();
    // Header with change summary
    let removed = old_lines.len();
    let added = new_lines.len();
    if removed > 0 && added > 0 {
        lines.push(format!(
            "{DIM}{removed} line{} removed, {added} added{R}",
            if removed == 1 { "" } else { "s" }
        ));
    } else if removed > 0 {
        lines.push(format!(
            "{DIM}{removed} line{} removed{R}",
            if removed == 1 { "" } else { "s" }
        ));
    } else if added > 0 {
        lines.push(format!(
            "{DIM}{added} line{} added{R}",
            if added == 1 { "" } else { "s" }
        ));
    }
    // Show up to 3 removed lines with red minus prefix
    for line in old_lines.iter().take(3) {
        lines.push(format!("{RED}- {}{R}", truncate_for_summary(line, 80)));
    }
    let old_remaining = old_lines.len().saturating_sub(3);
    if old_remaining > 0 {
        lines.push(format!("{DIM}  ... {old_remaining} more removed{R}"));
    }
    // Show up to 3 added lines with green plus prefix
    for line in new_lines.iter().take(3) {
        lines.push(format!("{GREEN}+ {}{R}", truncate_for_summary(line, 80)));
    }
    let new_remaining = new_lines.len().saturating_sub(3);
    if new_remaining > 0 {
        lines.push(format!("{DIM}  ... {new_remaining} more added{R}"));
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub fn format_bash_call(parsed: &serde_json::Value) -> String {
    let command = parsed
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if command.is_empty() {
        String::new()
    } else {
        format!(
            "{}{} $ {} {}",
            crate::brand::BG_CODE,
            crate::brand::WHITE,
            truncate_for_summary(command, 160),
            crate::brand::R
        )
    }
}

pub fn first_visible_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

pub fn format_bash_result(icon: &str, parsed: &serde_json::Value) -> String {
    use std::fmt::Write as _;

    let mut lines = vec![format!(
        "{icon} {}{BOLD}bash{}{R}",
        crate::brand::BLUE,
        crate::brand::R
    )];
    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|value| value.as_str())
    {
        write!(&mut lines[0], " backgrounded ({task_id})").expect("write to string");
    } else if let Some(status) = parsed
        .get("returnCodeInterpretation")
        .and_then(|value| value.as_str())
        .filter(|status| !status.is_empty())
    {
        write!(&mut lines[0], " {status}").expect("write to string");
    }

    if let Some(stdout) = parsed.get("stdout").and_then(|value| value.as_str()) {
        if !stdout.trim().is_empty() {
            lines.push(truncate_output_for_display(
                stdout,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            ));
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|value| value.as_str()) {
        if !stderr.trim().is_empty() {
            lines.push(format!(
                "{}{}{}",
                crate::brand::RED,
                truncate_output_for_display(
                    stderr,
                    TOOL_OUTPUT_DISPLAY_MAX_LINES,
                    TOOL_OUTPUT_DISPLAY_MAX_CHARS,
                ),
                crate::brand::R,
            ));
        }
    }

    lines.join("\n\n")
}

pub fn format_read_result(icon: &str, parsed: &serde_json::Value) -> String {
    let file = parsed.get("file").unwrap_or(parsed);
    let path = extract_tool_path(file);
    let start_line = file
        .get("startLine")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let num_lines = file
        .get("numLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_lines = file
        .get("totalLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(num_lines);
    let content = file
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let end_line = start_line.saturating_add(num_lines.saturating_sub(1));

    format!(
        "{icon} {ICON_FILE} {DIM}Read {path} (lines {}-{} of {}){R}\n{}",
        start_line,
        end_line.max(start_line),
        total_lines,
        truncate_output_for_display(content, READ_DISPLAY_MAX_LINES, READ_DISPLAY_MAX_CHARS)
    )
}

pub fn format_write_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let kind = parsed
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("write");
    let line_count = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .map_or(0, |content| content.lines().count());
    format!(
        "{icon} {ICON_WRITE} {GREEN}{BOLD}{} {path}{R} {DIM}({line_count} lines){R}",
        if kind == "create" { "Wrote" } else { "Updated" },
    )
}

pub fn format_structured_patch_preview(parsed: &serde_json::Value) -> Option<String> {
    let hunks = parsed.get("structuredPatch")?.as_array()?;
    let mut preview = Vec::new();
    for hunk in hunks.iter().take(2) {
        let lines = hunk.get("lines")?.as_array()?;
        for line in lines.iter().filter_map(|value| value.as_str()).take(6) {
            match line.chars().next() {
                Some('+') => preview.push(format!(
                    "{GREEN}{line}{R}",
                    GREEN = crate::brand::GREEN,
                    R = crate::brand::R
                )),
                Some('-') => preview.push(format!(
                    "{RED}{line}{R}",
                    RED = crate::brand::RED,
                    R = crate::brand::R
                )),
                _ => preview.push(line.to_string()),
            }
        }
    }
    if preview.is_empty() {
        None
    } else {
        Some(preview.join("\n"))
    }
}

pub fn format_edit_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let suffix = if parsed
        .get("replaceAll")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        " (replace all)"
    } else {
        ""
    };
    let preview = format_structured_patch_preview(parsed).or_else(|| {
        let old_value = parsed
            .get("oldString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let new_value = parsed
            .get("newString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        format_patch_preview(old_value, new_value)
    });

    match preview {
        Some(preview) => format!(
            "{icon} {ICON_EDIT} {ORANGE}{BOLD}Edited {path}{suffix}{R}\n{preview}",
            ICON_EDIT = crate::brand::ICON_EDIT,
            ORANGE = crate::brand::ORANGE,
            BOLD = crate::brand::BOLD,
            R = crate::brand::R
        ),
        None => format!(
            "{icon} {ICON_EDIT} {ORANGE}{BOLD}Edited {path}{suffix}{R}",
            ICON_EDIT = crate::brand::ICON_EDIT,
            ORANGE = crate::brand::ORANGE,
            BOLD = crate::brand::BOLD,
            R = crate::brand::R
        ),
    }
}

pub fn format_glob_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .map(display_clean_path)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if filenames.is_empty() {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files")
    } else {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files\n{filenames}")
    }
}

pub fn format_grep_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_matches = parsed
        .get("numMatches")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let content = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let summary = format!(
        "{icon} \x1b[38;5;245mgrep_search\x1b[0m {num_matches} matches across {num_files} files"
    );
    if !content.trim().is_empty() {
        format!(
            "{summary}\n{}",
            truncate_output_for_display(
                content,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            )
        )
    } else if !filenames.is_empty() {
        format!("{summary}\n{filenames}")
    } else {
        summary
    }
}

pub fn format_generic_tool_result(icon: &str, name: &str, parsed: &serde_json::Value) -> String {
    let rendered_output = match parsed {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(parsed).unwrap_or_else(|_| parsed.to_string())
        }
        _ => parsed.to_string(),
    };
    let preview = truncate_output_for_display(
        &rendered_output,
        TOOL_OUTPUT_DISPLAY_MAX_LINES,
        TOOL_OUTPUT_DISPLAY_MAX_CHARS,
    );

    if preview.is_empty() {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
    } else if preview.contains('\n') {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n{preview}")
    } else {
        format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {preview}")
    }
}

pub fn summarize_tool_payload(payload: &str) -> String {
    let compact = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => payload.trim().to_string(),
    };
    truncate_for_summary(&compact, 96)
}

pub fn truncate_for_summary(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\u{2026}")
    } else {
        truncated
    }
}

pub fn truncate_output_for_display(content: &str, max_lines: usize, max_chars: usize) -> String {
    let original = content.trim_end_matches('\n');
    if original.is_empty() {
        return String::new();
    }

    let mut preview_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;

    for (index, line) in original.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }

        let newline_cost = usize::from(!preview_lines.is_empty());
        let available = max_chars.saturating_sub(used_chars + newline_cost);
        if available == 0 {
            truncated = true;
            break;
        }

        let line_chars = line.chars().count();
        if line_chars > available {
            preview_lines.push(line.chars().take(available).collect::<String>());
            truncated = true;
            break;
        }

        preview_lines.push(line.to_string());
        used_chars += newline_cost + line_chars;
    }

    let mut preview = preview_lines.join("\n");
    if truncated {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(DISPLAY_TRUNCATION_NOTICE);
    }
    preview
}
