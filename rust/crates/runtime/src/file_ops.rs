mod walk;

use std::cmp::Reverse;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use glob::Pattern;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::sensitive_paths::sensitive_path_reason;

pub use walk::DEFAULT_SKIPPED_DIRS;

/// Maximum file size that can be read (10 MB).
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum file size that can be written (10 MB).
const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024;

/// Lines returned by `read_file` when the caller gives no `limit`.
pub const DEFAULT_READ_LINE_LIMIT: usize = 2_000;

/// Characters kept per line by `read_file`; longer lines are cut.
pub const MAX_READ_LINE_CHARS: usize = 2_000;

/// Maximum diff lines (context included) returned by `edit_file`.
const MAX_DIFF_LINES: usize = 160;

/// Maximum characters of the textual diff returned by `edit_file`.
const MAX_DIFF_CHARS: usize = 12_000;

/// Context lines around each change in `edit_file` diffs.
const DIFF_CONTEXT_LINES: usize = 3;

/// Match locations reported when `old_string` is ambiguous.
const MAX_REPORTED_MATCH_LINES: usize = 20;

/// Check whether a file appears to contain binary content by examining
/// the first chunk for NUL bytes.
fn is_binary_file(path: &Path) -> io::Result<bool> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 8192];
    let bytes_read = file.read(&mut buffer)?;
    Ok(buffer[..bytes_read].contains(&0))
}

/// Validate that a resolved path stays within the given workspace root.
/// Returns the canonical path on success, or an error if the path escapes
/// the workspace boundary (e.g. via `../` traversal or symlink).
#[allow(dead_code)]
fn validate_workspace_boundary(resolved: &Path, workspace_root: &Path) -> io::Result<()> {
    if !path_starts_with_normalized(resolved, workspace_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "path {} escapes workspace boundary {}",
                resolved.display(),
                workspace_root.display()
            ),
        ));
    }
    Ok(())
}

fn path_starts_with_normalized(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }

    #[cfg(windows)]
    {
        let normalize = |value: &Path| {
            value
                .to_string_lossy()
                .trim_start_matches("\\\\?\\")
                .replace('/', "\\")
                .to_ascii_lowercase()
        };
        let path = normalize(path);
        let mut root = normalize(root);
        if !root.ends_with('\\') {
            root.push('\\');
        }
        path == root.trim_end_matches('\\') || path.starts_with(&root)
    }

    #[cfg(not(windows))]
    {
        false
    }
}

/// Text payload returned by file-reading operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextFilePayload {
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub content: String,
    #[serde(rename = "numLines")]
    pub num_lines: usize,
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "totalLines")]
    pub total_lines: usize,
    /// Number of lines cut to [`MAX_READ_LINE_CHARS`] characters.
    #[serde(rename = "truncatedLines", default, skip_serializing_if = "is_zero")]
    pub truncated_lines: usize,
    /// How to read the rest of the file when the window does not reach the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Output envelope for the `read_file` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileOutput {
    #[serde(rename = "type")]
    pub kind: String,
    pub file: TextFilePayload,
}

/// Structured patch hunk emitted by edit operations (unified-diff lines
/// prefixed with ` `, `-` or `+`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredPatchHunk {
    #[serde(rename = "oldStart")]
    pub old_start: usize,
    #[serde(rename = "oldLines")]
    pub old_lines: usize,
    #[serde(rename = "newStart")]
    pub new_start: usize,
    #[serde(rename = "newLines")]
    pub new_lines: usize,
    pub lines: Vec<String>,
}

/// Compact summary returned by full-file writes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteFileOutput {
    /// `create` for a new file, `update` when an existing file was overwritten.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub bytes: usize,
    pub lines: usize,
    pub summary: String,
}

/// Output envelope for targeted string-replacement edits: a bounded unified
/// diff instead of the whole file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditFileOutput {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "replaceAll")]
    pub replace_all: bool,
    /// Number of occurrences replaced.
    pub replacements: usize,
    /// Line endings of the file (`lf` or `crlf`), preserved on write.
    #[serde(rename = "lineEnding")]
    pub line_ending: String,
    /// Unified diff of the change (bounded).
    pub diff: String,
    #[serde(rename = "structuredPatch")]
    pub structured_patch: Vec<StructuredPatchHunk>,
    /// Whether the diff was cut to stay compact.
    #[serde(rename = "diffTruncated")]
    pub diff_truncated: bool,
}

/// Result of a glob-based filename search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobSearchOutput {
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
    #[serde(rename = "numFiles")]
    pub num_files: usize,
    pub filenames: Vec<String>,
    pub truncated: bool,
}

/// Parameters accepted by the grep-style search tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrepSearchInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    #[serde(rename = "output_mode")]
    pub output_mode: Option<String>,
    #[serde(rename = "-B")]
    pub before: Option<usize>,
    #[serde(rename = "-A")]
    pub after: Option<usize>,
    #[serde(rename = "-C")]
    pub context_short: Option<usize>,
    pub context: Option<usize>,
    #[serde(rename = "-n")]
    pub line_numbers: Option<bool>,
    #[serde(rename = "-i")]
    pub case_insensitive: Option<bool>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    pub head_limit: Option<usize>,
    pub offset: Option<usize>,
    pub multiline: Option<bool>,
}

/// Result payload returned by the grep-style search tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrepSearchOutput {
    pub mode: Option<String>,
    #[serde(rename = "numFiles")]
    pub num_files: usize,
    pub filenames: Vec<String>,
    pub content: Option<String>,
    #[serde(rename = "numLines")]
    pub num_lines: Option<usize>,
    #[serde(rename = "numMatches")]
    pub num_matches: Option<usize>,
    #[serde(rename = "appliedLimit")]
    pub applied_limit: Option<usize>,
    #[serde(rename = "appliedOffset")]
    pub applied_offset: Option<usize>,
}

/// Reads a text file and returns a line-windowed payload.
///
/// Without `limit` at most [`DEFAULT_READ_LINE_LIMIT`] lines are returned,
/// with a hint naming the `offset` that continues the read. Lines longer than
/// [`MAX_READ_LINE_CHARS`] characters are cut.
pub fn read_file(
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> io::Result<ReadFileOutput> {
    let absolute_path = normalize_path(path)?;

    // Check file size before reading
    let metadata = fs::metadata(&absolute_path)?;
    if metadata.len() > MAX_READ_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file is too large ({} bytes, max {} bytes)",
                metadata.len(),
                MAX_READ_SIZE
            ),
        ));
    }

    // Detect binary files
    if is_binary_file(&absolute_path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file appears to be binary",
        ));
    }

    let content = fs::read_to_string(&absolute_path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start_index = offset.unwrap_or(0).min(lines.len());
    let window = limit.unwrap_or(DEFAULT_READ_LINE_LIMIT);
    let end_index = start_index.saturating_add(window).min(lines.len());

    let mut truncated_lines = 0;
    let selected = lines[start_index..end_index]
        .iter()
        .map(|line| {
            let length = line.chars().count();
            if length > MAX_READ_LINE_CHARS {
                truncated_lines += 1;
                let kept = line.chars().take(MAX_READ_LINE_CHARS).collect::<String>();
                format!("{kept}… [line truncated: {length} characters]")
            } else {
                (*line).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let hint = (end_index < lines.len()).then(|| {
        format!(
            "Showing lines {}-{} of {}. Call read_file with offset={} to continue.",
            start_index + 1,
            end_index,
            lines.len(),
            end_index
        )
    });

    Ok(ReadFileOutput {
        kind: String::from("text"),
        file: TextFilePayload {
            file_path: absolute_path.to_string_lossy().into_owned(),
            content: selected,
            num_lines: end_index.saturating_sub(start_index),
            start_line: start_index.saturating_add(1),
            total_lines: lines.len(),
            truncated_lines,
            hint,
        },
    })
}

/// Replaces a file's contents and returns a compact summary.
pub fn write_file(path: &str, content: &str) -> io::Result<WriteFileOutput> {
    if content.len() > MAX_WRITE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "content is too large ({} bytes, max {} bytes)",
                content.len(),
                MAX_WRITE_SIZE
            ),
        ));
    }

    let absolute_path = normalize_path_allow_missing(path)?;
    let existed = absolute_path.is_file();
    if let Some(parent) = absolute_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&absolute_path, content)?;

    let file_path = absolute_path.to_string_lossy().into_owned();
    let lines = content.lines().count();
    let summary = format!(
        "{} {file_path} ({lines} lines, {} bytes)",
        if existed { "Overwrote" } else { "Created" },
        content.len()
    );
    Ok(WriteFileOutput {
        kind: if existed {
            String::from("update")
        } else {
            String::from("create")
        },
        file_path,
        bytes: content.len(),
        lines,
        summary,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    fn detect(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        if crlf > 0 && crlf * 2 >= lf {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }

    /// Rewrites `text`'s line breaks in this style.
    fn apply(self, text: &str) -> String {
        let normalized = text.replace("\r\n", "\n");
        match self {
            Self::Lf => normalized,
            Self::Crlf => normalized.replace('\n', "\r\n"),
        }
    }
}

/// Byte offsets of every non-overlapping occurrence of `needle`.
fn match_offsets(haystack: &str, needle: &str) -> Vec<usize> {
    haystack
        .match_indices(needle)
        .map(|(offset, _)| offset)
        .collect()
}

fn line_number_at(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Performs an in-file string replacement and returns a compact diff.
///
/// `old_string` written with `\n` line breaks also matches a CRLF file, and
/// the replacement is written with the file's own line endings. When
/// `old_string` occurs more than once and `replace_all` is false the edit is
/// refused with the match count and line numbers, instead of silently
/// changing the first occurrence.
pub fn edit_file(
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> io::Result<EditFileOutput> {
    let absolute_path = normalize_path(path)?;
    let original_file = fs::read_to_string(&absolute_path)?;
    if old_string.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "old_string must not be empty; use write_file to create or replace a whole file",
        ));
    }
    if old_string == new_string {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "old_string and new_string must differ",
        ));
    }

    let line_ending = LineEnding::detect(&original_file);
    let (needle, mut offsets) = {
        let exact = match_offsets(&original_file, old_string);
        let adapted = line_ending.apply(old_string);
        if exact.is_empty() && adapted != old_string {
            let adapted_offsets = match_offsets(&original_file, &adapted);
            (adapted, adapted_offsets)
        } else {
            (old_string.to_string(), exact)
        }
    };
    if offsets.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "old_string not found in file (check whitespace and indentation, or re-read the file)",
        ));
    }
    if offsets.len() > 1 && !replace_all {
        let lines = offsets
            .iter()
            .take(MAX_REPORTED_MATCH_LINES)
            .map(|offset| line_number_at(&original_file, *offset).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let more = if offsets.len() > MAX_REPORTED_MATCH_LINES {
            ", ..."
        } else {
            ""
        };
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "old_string matches {} times (lines {lines}{more}); include more surrounding context to make it unique, or set replace_all to true",
                offsets.len()
            ),
        ));
    }
    if !replace_all {
        offsets.truncate(1);
    }

    let replacement = if line_ending == LineEnding::Crlf || new_string.contains("\r\n") {
        line_ending.apply(new_string)
    } else {
        new_string.to_string()
    };
    let mut updated = String::with_capacity(original_file.len());
    let mut cursor = 0;
    for offset in &offsets {
        updated.push_str(&original_file[cursor..*offset]);
        updated.push_str(&replacement);
        cursor = offset + needle.len();
    }
    updated.push_str(&original_file[cursor..]);
    fs::write(&absolute_path, &updated)?;

    let file_path = absolute_path.to_string_lossy().into_owned();
    let (structured_patch, diff, diff_truncated) =
        build_edit_diff(&file_path, &original_file, &offsets, &needle, &replacement);
    Ok(EditFileOutput {
        file_path,
        replace_all,
        replacements: offsets.len(),
        line_ending: line_ending.label().to_string(),
        diff,
        structured_patch,
        diff_truncated,
    })
}

/// A change in original-line coordinates: lines `start..end` are replaced by
/// `new_lines`.
struct LineChange {
    start: usize,
    end: usize,
    new_lines: Vec<String>,
}

fn strip_line_break(line: &str) -> &str {
    line.strip_suffix('\n')
        .map_or(line, |rest| rest.strip_suffix('\r').unwrap_or(rest))
}

/// Builds bounded unified-diff hunks for replacing `needle` at `offsets`
/// (ascending, non-overlapping) with `replacement`.
fn build_edit_diff(
    file_path: &str,
    original: &str,
    offsets: &[usize],
    needle: &str,
    replacement: &str,
) -> (Vec<StructuredPatchHunk>, String, bool) {
    let mut line_starts = vec![0];
    line_starts.extend(original.match_indices('\n').map(|(index, _)| index + 1));
    let original_lines = original
        .split_inclusive('\n')
        .map(|line| strip_line_break(line).to_string())
        .collect::<Vec<_>>();
    let line_count = original_lines.len();
    let line_of = |offset: usize| {
        (line_starts.partition_point(|start| *start <= offset) - 1)
            .min(line_count.saturating_sub(1))
    };
    let line_end = |line: usize| line_starts.get(line + 1).copied().unwrap_or(original.len());
    // Last original line touched by a replacement at `offset`. When the match
    // ends with a line break that the replacement drops, the following line
    // is joined onto the replacement, so it changes too.
    let last_line_of = |offset: usize| {
        let mut last = line_of(offset + needle.len() - 1);
        if needle.ends_with('\n') && !replacement.ends_with('\n') && last + 1 < line_count {
            last += 1;
        }
        last
    };

    // Group replacements whose line ranges touch and rebuild each group.
    let mut changes: Vec<LineChange> = Vec::new();
    let mut index = 0;
    while index < offsets.len() {
        let start = line_of(offsets[index]);
        let mut last = last_line_of(offsets[index]);
        let mut members = vec![offsets[index]];
        index += 1;
        while index < offsets.len() && line_of(offsets[index]) <= last {
            last = last.max(last_line_of(offsets[index]));
            members.push(offsets[index]);
            index += 1;
        }
        let block_end = line_end(last);
        let mut rebuilt = String::new();
        let mut cursor = line_starts[start];
        for offset in &members {
            rebuilt.push_str(&original[cursor..*offset]);
            rebuilt.push_str(replacement);
            cursor = offset + needle.len();
        }
        rebuilt.push_str(&original[cursor..block_end]);
        changes.push(LineChange {
            start,
            end: last + 1,
            new_lines: rebuilt
                .split_inclusive('\n')
                .map(|line| strip_line_break(line).to_string())
                .collect(),
        });
    }

    // Merge changes whose context windows overlap into hunks.
    let mut hunks = Vec::new();
    let mut line_delta: isize = 0;
    let mut position = 0;
    while position < changes.len() {
        let hunk_start = changes[position].start.saturating_sub(DIFF_CONTEXT_LINES);
        let mut group_end = position + 1;
        while group_end < changes.len()
            && changes[group_end].start.saturating_sub(DIFF_CONTEXT_LINES)
                <= changes[group_end - 1].end + DIFF_CONTEXT_LINES
        {
            group_end += 1;
        }
        let hunk_end = (changes[group_end - 1].end + DIFF_CONTEXT_LINES).min(line_count);
        let new_start = isize::try_from(hunk_start).unwrap_or(isize::MAX) + line_delta;
        let mut hunk = StructuredPatchHunk {
            old_start: hunk_start + 1,
            old_lines: hunk_end - hunk_start,
            new_start: usize::try_from(new_start).unwrap_or(0) + 1,
            new_lines: 0,
            lines: Vec::new(),
        };
        let mut cursor = hunk_start;
        for change in &changes[position..group_end] {
            for line in &original_lines[cursor..change.start] {
                hunk.lines.push(format!(" {line}"));
            }
            for line in &original_lines[change.start..change.end] {
                hunk.lines.push(format!("-{line}"));
            }
            for line in &change.new_lines {
                hunk.lines.push(format!("+{line}"));
            }
            line_delta += isize::try_from(change.new_lines.len()).unwrap_or(0)
                - isize::try_from(change.end - change.start).unwrap_or(0);
            cursor = change.end;
        }
        for line in &original_lines[cursor..hunk_end] {
            hunk.lines.push(format!(" {line}"));
        }
        hunk.new_lines = hunk
            .lines
            .iter()
            .filter(|line| !line.starts_with('-'))
            .count();
        hunks.push(hunk);
        position = group_end;
    }

    bound_hunks(file_path, hunks)
}

/// Caps the hunks at [`MAX_DIFF_LINES`] lines and the text at
/// [`MAX_DIFF_CHARS`] characters.
fn bound_hunks(
    file_path: &str,
    hunks: Vec<StructuredPatchHunk>,
) -> (Vec<StructuredPatchHunk>, String, bool) {
    let mut kept = Vec::new();
    let mut budget = MAX_DIFF_LINES;
    let mut omitted = 0;
    for mut hunk in hunks {
        if budget == 0 {
            omitted += hunk.lines.len();
            continue;
        }
        if hunk.lines.len() > budget {
            omitted += hunk.lines.len() - budget;
            hunk.lines.truncate(budget);
        }
        budget -= hunk.lines.len();
        kept.push(hunk);
    }

    let mut diff = format!("--- {file_path}\n+++ {file_path}\n");
    for hunk in &kept {
        let _ = writeln!(
            diff,
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        );
        for line in &hunk.lines {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    let mut truncated = omitted > 0;
    if diff.chars().count() > MAX_DIFF_CHARS {
        diff = diff.chars().take(MAX_DIFF_CHARS).collect();
        truncated = true;
    }
    if truncated {
        let _ = writeln!(
            diff,
            "… diff truncated ({omitted} more lines); re-read the file to see the full result"
        );
    }
    (kept, diff, truncated)
}

/// Expands a glob pattern and returns matching filenames.
///
/// The walk skips [`DEFAULT_SKIPPED_DIRS`] and `.gitignore`d paths unless the
/// pattern's literal prefix points inside them.
pub fn glob_search(pattern: &str, path: Option<&str>) -> io::Result<GlobSearchOutput> {
    let started = Instant::now();
    let base_dir = path
        .map(normalize_path)
        .transpose()?
        .unwrap_or(std::env::current_dir()?);
    let search_pattern = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        base_dir.join(pattern)
    };

    // The `glob` crate does not support brace expansion ({a,b,c}).
    // Expand braces into multiple patterns so patterns like
    // `Assets/**/*.{cs,uxml,uss}` work correctly.
    let expanded = expand_braces(&search_pattern.to_string_lossy());

    let mut seen = std::collections::HashSet::new();
    let mut matches = Vec::new();
    let mut walk_truncated = false;
    for candidate in &expanded {
        let (root, rest) = walk::split_glob_root(Path::new(candidate));
        let Some(rest) = rest else {
            let literal = root;
            if literal.is_file() && seen.insert(literal.clone()) {
                matches.push(literal);
            }
            continue;
        };
        let compiled = Pattern::new(&rest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let (files, truncated) = walk::collect_workspace_files(&root);
        walk_truncated |= truncated;
        for file in files {
            let Ok(relative) = file.strip_prefix(&root) else {
                continue;
            };
            if walk::glob_matches(&compiled, relative) && seen.insert(file.clone()) {
                matches.push(file);
            }
        }
    }

    matches.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(Reverse)
    });

    let truncated = matches.len() > 100 || walk_truncated;
    let filenames = matches
        .into_iter()
        .take(100)
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    Ok(GlobSearchOutput {
        duration_ms: started.elapsed().as_millis(),
        num_files: filenames.len(),
        filenames,
        truncated,
    })
}

/// Runs a regex search over workspace files with optional context lines.
///
/// Like [`glob_search`], the walk skips [`DEFAULT_SKIPPED_DIRS`] and
/// `.gitignore`d paths below the search root.
pub fn grep_search(input: &GrepSearchInput) -> io::Result<GrepSearchOutput> {
    let base_path = input
        .path
        .as_deref()
        .map(normalize_path)
        .transpose()?
        .unwrap_or(std::env::current_dir()?);

    let regex = RegexBuilder::new(&input.pattern)
        .case_insensitive(input.case_insensitive.unwrap_or(false))
        .dot_matches_new_line(input.multiline.unwrap_or(false))
        .build()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let glob_filter = input
        .glob
        .as_deref()
        .map(Pattern::new)
        .transpose()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let file_type = input.file_type.as_deref();
    let output_mode = input
        .output_mode
        .clone()
        .unwrap_or_else(|| String::from("files_with_matches"));
    let context = input.context.or(input.context_short).unwrap_or(0);

    let mut filenames = Vec::new();
    let mut content_lines = Vec::new();
    let mut total_matches = 0usize;

    for file_path in collect_search_files(&base_path) {
        if !matches_optional_filters(&file_path, glob_filter.as_ref(), file_type) {
            continue;
        }
        // Naming a credential file as `path` is a read the user approved; a
        // directory search leaves them out.
        if file_path != base_path && is_sensitive_search_file(&base_path, &file_path) {
            continue;
        }

        let Ok(file_contents) = fs::read_to_string(&file_path) else {
            continue;
        };

        if output_mode == "count" {
            let count = regex.find_iter(&file_contents).count();
            if count > 0 {
                filenames.push(file_path.to_string_lossy().into_owned());
                total_matches += count;
            }
            continue;
        }

        let lines: Vec<&str> = file_contents.lines().collect();
        let mut matched_lines = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                total_matches += 1;
                matched_lines.push(index);
            }
        }

        if matched_lines.is_empty() {
            continue;
        }

        filenames.push(file_path.to_string_lossy().into_owned());
        if output_mode == "content" {
            for index in matched_lines {
                let start = index.saturating_sub(input.before.unwrap_or(context));
                let end = (index + input.after.unwrap_or(context) + 1).min(lines.len());
                for (current, line) in lines.iter().enumerate().take(end).skip(start) {
                    let prefix = if input.line_numbers.unwrap_or(true) {
                        format!("{}:{}:", file_path.to_string_lossy(), current + 1)
                    } else {
                        format!("{}:", file_path.to_string_lossy())
                    };
                    content_lines.push(format!("{prefix}{line}"));
                }
            }
        }
    }

    let (filenames, applied_limit, applied_offset) =
        apply_limit(filenames, input.head_limit, input.offset);
    let content_output = if output_mode == "content" {
        let (lines, limit, offset) = apply_limit(content_lines, input.head_limit, input.offset);
        return Ok(GrepSearchOutput {
            mode: Some(output_mode),
            num_files: filenames.len(),
            filenames,
            num_lines: Some(lines.len()),
            content: Some(lines.join("\n")),
            num_matches: None,
            applied_limit: limit,
            applied_offset: offset,
        });
    } else {
        None
    };

    Ok(GrepSearchOutput {
        mode: Some(output_mode.clone()),
        num_files: filenames.len(),
        filenames,
        content: content_output,
        num_lines: None,
        num_matches: (output_mode == "count").then_some(total_matches),
        applied_limit,
        applied_offset,
    })
}

fn collect_search_files(base_path: &Path) -> Vec<PathBuf> {
    walk::collect_workspace_files(base_path).0
}

/// Whether a file found under `base_path` holds credentials, judged by its
/// path below the search root and, for a symlink, by its target.
fn is_sensitive_search_file(base_path: &Path, file_path: &Path) -> bool {
    let relative = file_path.strip_prefix(base_path).unwrap_or(file_path);
    if sensitive_path_reason(&relative.to_string_lossy()).is_some() {
        return true;
    }
    fs::symlink_metadata(file_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        && fs::canonicalize(file_path)
            .is_ok_and(|target| sensitive_path_reason(&target.to_string_lossy()).is_some())
}

fn matches_optional_filters(
    path: &Path,
    glob_filter: Option<&Pattern>,
    file_type: Option<&str>,
) -> bool {
    if let Some(glob_filter) = glob_filter {
        let path_string = path.to_string_lossy();
        if !glob_filter.matches(&path_string) && !glob_filter.matches_path(path) {
            return false;
        }
    }

    if let Some(file_type) = file_type {
        let extension = path.extension().and_then(|extension| extension.to_str());
        if extension != Some(file_type) {
            return false;
        }
    }

    true
}

fn apply_limit<T>(
    items: Vec<T>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> (Vec<T>, Option<usize>, Option<usize>) {
    let offset_value = offset.unwrap_or(0);
    let mut items = items.into_iter().skip(offset_value).collect::<Vec<_>>();
    let explicit_limit = limit.unwrap_or(250);
    if explicit_limit == 0 {
        return (items, None, (offset_value > 0).then_some(offset_value));
    }

    let truncated = items.len() > explicit_limit;
    items.truncate(explicit_limit);
    (
        items,
        truncated.then_some(explicit_limit),
        (offset_value > 0).then_some(offset_value),
    )
}

fn normalize_path(path: &str) -> io::Result<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?.join(path)
    };
    candidate.canonicalize()
}

fn normalize_path_allow_missing(path: &str) -> io::Result<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut ancestor = candidate.as_path();
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                for name in missing.into_iter().rev() {
                    canonical.push(name);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::symlink_metadata(ancestor).is_ok() {
                    return Err(error);
                }
                missing.push(ancestor.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Cannot resolve missing path")
                })?);
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Read a file with workspace boundary enforcement.
#[allow(dead_code)]
pub fn read_file_in_workspace(
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    workspace_root: &Path,
) -> io::Result<ReadFileOutput> {
    let absolute_path = normalize_path(path)?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    validate_workspace_boundary(&absolute_path, &canonical_root)?;
    read_file(path, offset, limit)
}

/// Write a file with workspace boundary enforcement.
#[allow(dead_code)]
pub fn write_file_in_workspace(
    path: &str,
    content: &str,
    workspace_root: &Path,
) -> io::Result<WriteFileOutput> {
    let absolute_path = normalize_path_allow_missing(path)?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    validate_workspace_boundary(&absolute_path, &canonical_root)?;
    write_file(path, content)
}

/// Edit a file with workspace boundary enforcement.
#[allow(dead_code)]
pub fn edit_file_in_workspace(
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    workspace_root: &Path,
) -> io::Result<EditFileOutput> {
    let absolute_path = normalize_path(path)?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    validate_workspace_boundary(&absolute_path, &canonical_root)?;
    edit_file(path, old_string, new_string, replace_all)
}

/// Check whether a path is a symlink that resolves outside the workspace.
#[allow(dead_code)]
pub fn is_symlink_escape(path: &Path, workspace_root: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_symlink() {
        return Ok(false);
    }
    let resolved = path.canonicalize()?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    Ok(!resolved.starts_with(&canonical_root))
}

/// Expand shell-style brace groups in a glob pattern.
///
/// Handles one level of braces: `foo.{a,b,c}` → `["foo.a", "foo.b", "foo.c"]`.
/// Nested braces are not expanded (uncommon in practice).
/// Patterns without braces pass through unchanged.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let Some(close) = pattern[open..].find('}').map(|i| open + i) else {
        // Unmatched brace — treat as literal.
        return vec![pattern.to_owned()];
    };
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let alternatives = &pattern[open + 1..close];
    alternatives
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{prefix}{alt}{suffix}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        edit_file, expand_braces, glob_search, grep_search, is_symlink_escape, read_file,
        read_file_in_workspace, write_file, write_file_in_workspace, GrepSearchInput,
        MAX_WRITE_SIZE,
    };

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("clawd-native-{name}-{unique}"))
    }

    #[test]
    fn reads_and_writes_files() {
        let path = temp_path("read-write.txt");
        let write_output = write_file(path.to_string_lossy().as_ref(), "one\ntwo\nthree")
            .expect("write should succeed");
        assert_eq!(write_output.kind, "create");

        let read_output = read_file(path.to_string_lossy().as_ref(), Some(1), Some(1))
            .expect("read should succeed");
        assert_eq!(read_output.file.content, "two");
    }

    #[test]
    fn edits_file_contents() {
        let path = temp_path("edit.txt");
        write_file(path.to_string_lossy().as_ref(), "alpha beta alpha")
            .expect("initial write should succeed");
        let output = edit_file(path.to_string_lossy().as_ref(), "alpha", "omega", true)
            .expect("edit should succeed");
        assert!(output.replace_all);
    }

    #[test]
    fn rejects_binary_files() {
        let path = temp_path("binary-test.bin");
        std::fs::write(&path, b"\x00\x01\x02\x03binary content").expect("write should succeed");
        let result = read_file(path.to_string_lossy().as_ref(), None, None);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("binary"));
    }

    #[test]
    fn rejects_oversized_writes() {
        let path = temp_path("oversize-write.txt");
        let huge = "x".repeat(MAX_WRITE_SIZE + 1);
        let result = write_file(path.to_string_lossy().as_ref(), &huge);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn enforces_workspace_boundary() {
        let workspace = temp_path("workspace-boundary");
        std::fs::create_dir_all(&workspace).expect("workspace dir should be created");
        let inside = workspace.join("inside.txt");
        write_file(inside.to_string_lossy().as_ref(), "safe content")
            .expect("write inside workspace should succeed");

        // Reading inside workspace should succeed
        let result =
            read_file_in_workspace(inside.to_string_lossy().as_ref(), None, None, &workspace);
        assert!(result.is_ok());

        let nested = workspace.join("missing/parents/new.txt");
        let output = write_file_in_workspace(nested.to_string_lossy().as_ref(), "new", &workspace)
            .expect("new nested file should remain inside the canonical workspace");
        assert_eq!(
            std::path::PathBuf::from(output.file_path),
            nested.canonicalize().unwrap()
        );

        let escape = workspace.join("missing/../../outside-new.txt");
        assert!(
            write_file_in_workspace(escape.to_string_lossy().as_ref(), "unsafe", &workspace)
                .is_err()
        );

        #[cfg(unix)]
        {
            let target = temp_path("missing-symlink-target.txt");
            let link = workspace.join("dangling-link.txt");
            std::os::unix::fs::symlink(&target, &link).expect("create dangling symlink");
            assert!(
                write_file_in_workspace(link.to_string_lossy().as_ref(), "unsafe", &workspace)
                    .is_err()
            );
            assert!(!target.exists());
        }

        // Reading outside workspace should fail
        let outside = temp_path("outside-boundary.txt");
        write_file(outside.to_string_lossy().as_ref(), "unsafe content")
            .expect("write outside should succeed");
        let result =
            read_file_in_workspace(outside.to_string_lossy().as_ref(), None, None, &workspace);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("escapes workspace"));
    }

    #[test]
    fn detects_symlink_escape() {
        let workspace = temp_path("symlink-workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir should be created");
        let outside = temp_path("symlink-target.txt");
        std::fs::write(&outside, "target content").expect("target should write");

        #[cfg(unix)]
        {
            let link_path = workspace.join("escape-link.txt");
            std::os::unix::fs::symlink(&outside, &link_path).expect("symlink should create");
            assert!(is_symlink_escape(&link_path, &workspace).expect("check should succeed"));
        }

        // Non-symlink file should not be an escape
        let normal = workspace.join("normal.txt");
        std::fs::write(&normal, "normal content").expect("normal file should write");
        assert!(!is_symlink_escape(&normal, &workspace).expect("check should succeed"));
    }

    #[test]
    fn globs_and_greps_directory() {
        let dir = temp_path("search-dir");
        std::fs::create_dir_all(&dir).expect("directory should be created");
        let file = dir.join("demo.rs");
        write_file(
            file.to_string_lossy().as_ref(),
            "fn main() {\n println!(\"hello\");\n}\n",
        )
        .expect("file write should succeed");

        let globbed = glob_search("**/*.rs", Some(dir.to_string_lossy().as_ref()))
            .expect("glob should succeed");
        assert_eq!(globbed.num_files, 1);

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("hello"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: Some(String::from("**/*.rs")),
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: Some(false),
            file_type: None,
            head_limit: Some(10),
            offset: Some(0),
            multiline: Some(false),
        })
        .expect("grep should succeed");
        assert!(grep_output.content.unwrap_or_default().contains("hello"));
    }

    #[test]
    fn expand_braces_no_braces() {
        assert_eq!(expand_braces("*.rs"), vec!["*.rs"]);
    }

    #[test]
    fn expand_braces_single_group() {
        let mut result = expand_braces("Assets/**/*.{cs,uxml,uss}");
        result.sort();
        assert_eq!(
            result,
            vec!["Assets/**/*.cs", "Assets/**/*.uss", "Assets/**/*.uxml",]
        );
    }

    #[test]
    fn expand_braces_nested() {
        let mut result = expand_braces("src/{a,b}.{rs,toml}");
        result.sort();
        assert_eq!(
            result,
            vec!["src/a.rs", "src/a.toml", "src/b.rs", "src/b.toml"]
        );
    }

    #[test]
    fn expand_braces_unmatched() {
        assert_eq!(expand_braces("foo.{bar"), vec!["foo.{bar"]);
    }

    #[test]
    fn glob_search_with_braces_finds_files() {
        let dir = temp_path("glob-braces");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("b.toml"), "[package]").unwrap();
        std::fs::write(dir.join("c.txt"), "hello").unwrap();

        let result =
            glob_search("*.{rs,toml}", Some(dir.to_str().unwrap())).expect("glob should succeed");
        assert_eq!(
            result.num_files, 2,
            "should match .rs and .toml but not .txt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fresh_dir(name: &str) -> std::path::PathBuf {
        let dir = temp_path(name);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn edit_matches_lf_old_string_in_crlf_file_and_keeps_crlf() {
        let dir = fresh_dir("crlf-edit");
        let path = dir.join("windows.txt");
        std::fs::write(&path, "first line\r\nsecond line\r\nthird line\r\n").expect("write");

        let output = edit_file(
            path.to_string_lossy().as_ref(),
            "second line\nthird line",
            "second LINE\nthird LINE\nfourth line",
            false,
        )
        .expect("LF old_string should match CRLF file");

        let written = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written,
            "first line\r\nsecond LINE\r\nthird LINE\r\nfourth line\r\n"
        );
        assert_eq!(output.line_ending, "crlf");
        assert_eq!(output.replacements, 1);
        assert!(output.diff.contains("-second line"));
        assert!(output.diff.contains("+fourth line"));
        assert!(!output.diff.contains('\r'));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_refuses_ambiguous_matches_with_count_and_lines() {
        let dir = fresh_dir("multi-match");
        let path = dir.join("dupes.rs");
        std::fs::write(&path, "let x = 1;\nfoo();\nlet y = 2;\nfoo();\nfoo();\n").expect("write");

        let error = edit_file(path.to_string_lossy().as_ref(), "foo();", "bar();", false)
            .expect_err("ambiguous edit must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let message = error.to_string();
        assert!(message.contains("matches 3 times"), "{message}");
        assert!(message.contains("lines 2, 4, 5"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged"),
            "let x = 1;\nfoo();\nlet y = 2;\nfoo();\nfoo();\n"
        );

        let all = edit_file(path.to_string_lossy().as_ref(), "foo();", "bar();", true)
            .expect("replace_all edits every match");
        assert_eq!(all.replacements, 3);
        assert!(!std::fs::read_to_string(&path)
            .expect("read")
            .contains("foo"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_returns_a_compact_bounded_diff() {
        let dir = fresh_dir("compact-diff");
        let path = dir.join("big.txt");
        let content = (1..=5_000)
            .map(|n| format!("line number {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &content).expect("write");

        let output = edit_file(
            path.to_string_lossy().as_ref(),
            "line number 2500\n",
            "line number 2500 (edited)\n",
            false,
        )
        .expect("edit");

        assert!(
            output.diff.len() < 1_000,
            "diff too large: {}",
            output.diff.len()
        );
        assert!(
            output.diff.contains("@@ -2497,7 +2497,7 @@"),
            "{}",
            output.diff
        );
        assert!(output.diff.contains(" line number 2497"));
        assert!(output.diff.contains("-line number 2500"));
        assert!(output.diff.contains("+line number 2500 (edited)"));
        assert!(!output.diff.contains("line number 1\n"));
        assert_eq!(output.structured_patch.len(), 1);
        let json = serde_json::to_string(&output).expect("serialize");
        assert!(json.len() < 3_000, "edit output must not echo the file");

        let replaced_everywhere =
            edit_file(path.to_string_lossy().as_ref(), "line number", "row", true)
                .expect("replace all");
        assert_eq!(replaced_everywhere.replacements, 5_000);
        assert!(replaced_everywhere.diff_truncated);
        assert!(replaced_everywhere.diff.chars().count() <= 12_200);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_diff_accounts_for_joined_lines_and_rejects_empty_old_string() {
        let dir = fresh_dir("joined-diff");
        let path = dir.join("join.txt");
        std::fs::write(&path, "a\nb\nc\n").expect("write");
        let output = edit_file(path.to_string_lossy().as_ref(), "a\n", "x", false).expect("edit");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "xb\nc\n");
        let hunk = &output.structured_patch[0];
        assert_eq!(hunk.lines, vec!["-a", "-b", "+xb", " c"]);
        assert_eq!((hunk.old_lines, hunk.new_lines), (3, 2));

        let error = edit_file(path.to_string_lossy().as_ref(), "", "y", false)
            .expect_err("empty old_string");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_returns_a_compact_summary() {
        let dir = fresh_dir("write-summary");
        let path = dir.join("out.txt");
        let created = write_file(path.to_string_lossy().as_ref(), "one\ntwo\n").expect("create");
        assert_eq!(created.kind, "create");
        assert_eq!(created.bytes, 8);
        assert_eq!(created.lines, 2);
        assert!(created.summary.starts_with("Created"));

        let big = "payload line\n".repeat(1_000);
        let updated = write_file(path.to_string_lossy().as_ref(), &big).expect("overwrite");
        assert_eq!(updated.kind, "update");
        assert_eq!(updated.lines, 1_000);
        assert!(updated.summary.starts_with("Overwrote"));
        let json = serde_json::to_string(&updated).expect("serialize");
        assert!(
            !json.contains("payload line"),
            "write output must not echo content"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_defaults_to_2000_lines_with_continuation_hint_and_cuts_long_lines() {
        let dir = fresh_dir("read-limits");
        let path = dir.join("long.txt");
        let mut lines = (1..=2_500).map(|n| format!("line {n}")).collect::<Vec<_>>();
        lines[9] = "x".repeat(5_000);
        std::fs::write(&path, lines.join("\r\n")).expect("write");

        let first = read_file(path.to_string_lossy().as_ref(), None, None).expect("read");
        assert_eq!(first.file.num_lines, 2_000);
        assert_eq!(first.file.total_lines, 2_500);
        assert_eq!(first.file.truncated_lines, 1);
        let hint = first.file.hint.as_deref().expect("hint");
        assert!(hint.contains("offset=2000"), "{hint}");
        assert!(first
            .file
            .content
            .contains("[line truncated: 5000 characters]"));
        assert!(!first.file.content.contains('\r'));

        let rest = read_file(path.to_string_lossy().as_ref(), Some(2_000), None).expect("read");
        assert_eq!(rest.file.num_lines, 500);
        assert_eq!(rest.file.start_line, 2_001);
        assert!(rest.file.hint.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn glob_and_grep_skip_build_dirs_and_gitignored_paths() {
        let dir = fresh_dir("skip-dirs");
        for sub in [
            "src",
            "node_modules/lib",
            "target/debug",
            "generated",
            ".git",
        ] {
            std::fs::create_dir_all(dir.join(sub)).expect("dir");
        }
        for file in [
            "src/app.js",
            "node_modules/lib/index.js",
            "target/debug/build.js",
            "generated/schema.js",
        ] {
            std::fs::write(dir.join(file), "needle\n").expect("file");
        }
        std::fs::write(dir.join(".gitignore"), "generated/\n").expect("gitignore");

        let globbed = glob_search("**/*.js", Some(dir.to_string_lossy().as_ref())).expect("glob");
        assert_eq!(globbed.num_files, 1, "{:?}", globbed.filenames);
        assert!(globbed.filenames[0].ends_with("app.js"));

        let explicit = glob_search("node_modules/**/*.js", Some(dir.to_string_lossy().as_ref()))
            .expect("explicit glob");
        assert_eq!(explicit.num_files, 1, "{:?}", explicit.filenames);

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("needle"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some(String::from("files_with_matches")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: None,
            case_insensitive: None,
            file_type: None,
            head_limit: None,
            offset: None,
            multiline: None,
        })
        .expect("grep");
        assert_eq!(grep_output.num_files, 1, "{:?}", grep_output.filenames);
        assert!(grep_output.filenames[0].ends_with("app.js"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn directory_grep_skips_credential_files_unless_named() {
        let dir = temp_path("grep-sensitive");
        std::fs::create_dir_all(dir.join(".neuron")).expect("settings dir");
        std::fs::write(dir.join("app.rs"), "const API_KEY_NAME: &str = \"x\";\n").expect("app");
        std::fs::write(dir.join(".env"), "OPENAI_API_KEY=sk-grep-secret\n").expect("env");
        std::fs::write(
            dir.join(".neuron").join("settings.local.json"),
            "{\"env\":{\"XAI_API_KEY\":\"xai-grep-secret\"}}\n",
        )
        .expect("settings");
        let grep = |path: &std::path::Path| {
            grep_search(&GrepSearchInput {
                pattern: String::from("API_KEY"),
                path: Some(path.to_string_lossy().into_owned()),
                glob: None,
                output_mode: Some(String::from("content")),
                before: None,
                after: None,
                context_short: None,
                context: None,
                line_numbers: None,
                case_insensitive: None,
                file_type: None,
                head_limit: None,
                offset: None,
                multiline: None,
            })
            .expect("grep")
        };

        let directory = grep(&dir);
        assert_eq!(directory.num_files, 1, "{:?}", directory.filenames);
        let content = directory.content.unwrap_or_default();
        assert!(content.contains("API_KEY_NAME"), "{content}");
        assert!(!content.contains("sk-grep-secret"), "{content}");
        assert!(!content.contains("xai-grep-secret"), "{content}");

        // Naming the file is a read the permission layer asked about.
        let named = grep(&dir.join(".env"));
        assert!(named.content.unwrap_or_default().contains("sk-grep-secret"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
