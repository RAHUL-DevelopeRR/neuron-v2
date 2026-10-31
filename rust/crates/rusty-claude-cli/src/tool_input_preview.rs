//! What a tool call is about while the model is still writing its input. The input is
//! JSON text that grows a few bytes at a time, so it is read as far as it goes: the
//! object's top-level string fields (a path, a command, a file's content), each with
//! whether its closing quote arrived. Nested arrays and objects are skipped.

/// Lines of the main text a preview keeps.
const PREVIEW_LINES: usize = 8;
/// Bytes a preview keeps at most.
const PREVIEW_BYTES: usize = 1024;
/// Characters of a target kept: the start of its first line.
const TARGET_CHARS: usize = 200;

/// A tool call's input as far as the model wrote it, as
/// [`neuron_tui::protocol::UiEvent::ToolInputProgress`] reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolInputPreview {
    /// What the call is about (a path, a command, a pattern, a URL), once its string is
    /// complete. A long or multi-line one is cut after its first line or
    /// [`TARGET_CHARS`] characters, with `…`.
    pub(crate) target: Option<String>,
    /// Lines of the main text so far (a written file's content, an edit's new text, a
    /// command); 0 before it starts.
    pub(crate) lines: u64,
    /// The main text's last lines (at most [`PREVIEW_LINES`], about [`PREVIEW_BYTES`]).
    pub(crate) preview: Option<String>,
}

/// Reads `input`, the JSON text of a `tool` call so far.
pub(crate) fn preview_tool_input(tool: &str, input: &str) -> ToolInputPreview {
    let fields = top_level_strings(input);
    // A repeated key counts as JSON parsers read it: the last one wins.
    let field = |key: &str| fields.iter().rev().find(|field| field.key == key);
    let target_keys = target_keys(tool);
    let target = target_keys
        .iter()
        .filter_map(|key| field(key))
        .find(|field| field.complete)
        .and_then(|field| short_target(&field.value));
    let main = match main_text_key(tool) {
        Some(key) => field(key),
        // The longest string that is not the target (a read's path is no preview).
        None => fields
            .iter()
            .filter(|field| !target_keys.contains(&field.key.as_str()))
            .reduce(|longest, field| {
                if field.value.len() > longest.value.len() {
                    field
                } else {
                    longest
                }
            }),
    };
    ToolInputPreview {
        target,
        lines: main.map_or(0, |field| field.value.lines().count() as u64),
        preview: main.and_then(|field| tail(&field.value)),
    }
}

/// The fields that say what a `tool` call is about, in order of preference.
fn target_keys(tool: &str) -> &'static [&'static str] {
    match tool {
        "read_file" | "write_file" | "edit_file" => &["path", "file_path"],
        "NotebookEdit" => &["notebook_path", "path"],
        "bash" | "PowerShell" => &["command"],
        "glob_search" | "grep_search" => &["pattern"],
        "WebFetch" => &["url"],
        "WebSearch" | "ToolSearch" => &["query"],
        "Agent" => &["description"],
        "Skill" => &["skill"],
        // MCP and plugin tools: whatever they name like the built-in ones do.
        _ => &[
            "path",
            "file_path",
            "notebook_path",
            "command",
            "pattern",
            "url",
            "query",
            "description",
            "name",
        ],
    }
}

/// The field holding a `tool` call's main text; otherwise its longest string is.
fn main_text_key(tool: &str) -> Option<&'static str> {
    match tool {
        "write_file" => Some("content"),
        "edit_file" => Some("new_string"),
        "bash" | "PowerShell" => Some("command"),
        "NotebookEdit" => Some("new_source"),
        "Agent" => Some("prompt"),
        _ => None,
    }
}

/// The start of `value`'s first line, `…` where something was left out; `None` for a
/// blank one.
fn short_target(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let first_line = trimmed.lines().next().unwrap_or_default().trim_end();
    if first_line.is_empty() {
        return None;
    }
    let mut short: String = first_line.chars().take(TARGET_CHARS).collect();
    if short.len() < trimmed.len() {
        short.push('\u{2026}');
    }
    Some(short)
}

/// The last [`PREVIEW_LINES`] lines of `text`, at most [`PREVIEW_BYTES`] cut on a
/// character boundary; `None` while it is empty.
fn tail(text: &str) -> Option<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let start = text
        .rmatch_indices('\n')
        .nth(PREVIEW_LINES - 1)
        .map_or(0, |(at, _)| at + 1);
    let mut tail = &text[start..];
    if tail.len() > PREVIEW_BYTES {
        let mut cut = tail.len() - PREVIEW_BYTES;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail = &tail[cut..];
    }
    Some(tail.replace("\r\n", "\n"))
}

/// A top-level string field of a JSON object, read as far as the text goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StringField {
    pub(crate) key: String,
    pub(crate) value: String,
    /// The closing quote arrived: `value` is the whole string. Otherwise it is the
    /// start of it, never a character the rest could change.
    pub(crate) complete: bool,
}

/// The top-level string fields of `json`, a JSON object that may stop anywhere (inside
/// a key, a value, an escape or a `\uXXXX` pair). Fields whose key is incomplete, and
/// values that are not strings, are left out. Text that is not an object gives none.
pub(crate) fn top_level_strings(json: &str) -> Vec<StringField> {
    let mut scanner = Scanner { text: json, at: 0 };
    let mut fields = Vec::new();
    scanner.skip_whitespace();
    if scanner.peek() != Some(b'{') {
        return fields;
    }
    scanner.at += 1;
    loop {
        scanner.skip_whitespace();
        match scanner.peek() {
            Some(b',') => {
                scanner.at += 1;
                continue;
            }
            Some(b'"') => scanner.at += 1,
            // The end of the object or of the text, or not JSON.
            _ => return fields,
        }
        let (key, key_complete) = scanner.string();
        if !key_complete {
            return fields;
        }
        scanner.skip_whitespace();
        if scanner.peek() != Some(b':') {
            return fields;
        }
        scanner.at += 1;
        scanner.skip_whitespace();
        match scanner.peek() {
            None => return fields,
            Some(b'"') => {
                scanner.at += 1;
                let (value, complete) = scanner.string();
                fields.push(StringField {
                    key,
                    value,
                    complete,
                });
                if !complete {
                    return fields;
                }
            }
            Some(b'{' | b'[') => {
                if !scanner.skip_nested() {
                    return fields;
                }
            }
            Some(_) => scanner.skip_scalar(),
        }
    }
}

/// A cursor over JSON text. It only ever stops on ASCII bytes or at the end, so `at`
/// is always a character boundary.
struct Scanner<'a> {
    text: &'a str,
    at: usize,
}

impl Scanner<'_> {
    fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes().get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// A string whose opening quote was read: its text, and whether the closing quote
    /// was there. Stops before an escape the text ends inside.
    fn string(&mut self) -> (String, bool) {
        let mut value = String::new();
        loop {
            let rest = &self.bytes()[self.at..];
            let run = rest
                .iter()
                .position(|byte| matches!(byte, b'"' | b'\\'))
                .unwrap_or(rest.len());
            value.push_str(&self.text[self.at..self.at + run]);
            self.at += run;
            match self.peek() {
                None => return (value, false),
                Some(b'"') => {
                    self.at += 1;
                    return (value, true);
                }
                Some(_) => match self.escape() {
                    Some(character) => value.push(character),
                    None => return (value, false),
                },
            }
        }
    }

    /// The escape at the cursor (a backslash), read past; `None` when the text ends
    /// inside it, with the cursor left where it was.
    fn escape(&mut self) -> Option<char> {
        let kind = *self.bytes().get(self.at + 1)?;
        if kind != b'u' {
            let character = match kind {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                // `\"`, `\\`, `\/`, and lenient for anything else ASCII.
                kind if kind.is_ascii() => char::from(kind),
                // Not JSON: keep the backslash; the character after it is copied as
                // it is.
                _ => {
                    self.at += 1;
                    return Some('\\');
                }
            };
            self.at += 2;
            return Some(character);
        }
        let unit = match self.hex4(self.at + 2) {
            Hex::Unit(unit) => unit,
            Hex::Truncated => return None,
            Hex::Invalid => {
                self.at += 2;
                return Some(char::REPLACEMENT_CHARACTER);
            }
        };
        match unit {
            0xD800..=0xDBFF => match self.low_surrogate(self.at + 6) {
                Hex::Unit(low) => {
                    self.at += 12;
                    let code =
                        0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                    Some(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER))
                }
                Hex::Truncated => None,
                Hex::Invalid => {
                    self.at += 6;
                    Some(char::REPLACEMENT_CHARACTER)
                }
            },
            unit => {
                self.at += 6;
                Some(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER))
            }
        }
    }

    /// Four hex digits at `at`.
    fn hex4(&self, at: usize) -> Hex {
        let digits = self.bytes().get(at..).unwrap_or_default();
        let digits = &digits[..digits.len().min(4)];
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return Hex::Invalid;
        }
        if digits.len() < 4 {
            return Hex::Truncated;
        }
        // Four ASCII hex digits.
        let text = std::str::from_utf8(digits).unwrap_or_default();
        u16::from_str_radix(text, 16).map_or(Hex::Invalid, Hex::Unit)
    }

    /// The `\uDC00`-`\uDFFF` escape that must follow a high surrogate, at `at`.
    fn low_surrogate(&self, at: usize) -> Hex {
        let rest = self.bytes().get(at..).unwrap_or_default();
        match rest {
            [] | [b'\\'] => Hex::Truncated,
            [b'\\', b'u', ..] => match self.hex4(at + 2) {
                Hex::Unit(low @ 0xDC00..=0xDFFF) => Hex::Unit(low),
                Hex::Unit(_) | Hex::Invalid => Hex::Invalid,
                Hex::Truncated => Hex::Truncated,
            },
            _ => Hex::Invalid,
        }
    }

    /// Skips the array or object at the cursor; `false` when the text ends first.
    fn skip_nested(&mut self) -> bool {
        let mut depth = 0_usize;
        let mut in_string = false;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.at += 1;
            if in_string {
                match byte {
                    _ if escaped => escaped = false,
                    b'\\' => escaped = true,
                    b'"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                _ => {}
            }
        }
        // Stopped inside a string: back to a boundary is not needed, the text ended.
        false
    }

    /// Skips a number, `true`, `false` or `null`.
    fn skip_scalar(&mut self) {
        while let Some(byte) = self.peek() {
            if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                return;
            }
            self.at += 1;
        }
    }
}

/// Four hex digits of a `\u` escape, read as far as the text goes.
enum Hex {
    Unit(u16),
    Truncated,
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(json: &str) -> Vec<(String, String, bool)> {
        top_level_strings(json)
            .into_iter()
            .map(|field| (field.key, field.value, field.complete))
            .collect()
    }

    fn field(key: &str, value: &str, complete: bool) -> (String, String, bool) {
        (key.to_string(), value.to_string(), complete)
    }

    #[test]
    fn reads_top_level_strings_in_any_order_and_skips_the_rest() {
        let json = r#" {
            "count": 3, "flags": [1, "a\"]", {"x": "}"}],
            "path" : "src/a b.rs",
            "nested": {"path": "not top level", "deep": [[{}]]},
            "ok": true, "none": null, "neg": -1.5e3,
            "content": "line 1\nsaid \"hi\" \\ \/ \té😀 é"
        } trailing"#;
        assert_eq!(
            fields(json),
            vec![
                field("path", "src/a b.rs", true),
                field(
                    "content",
                    "line 1\nsaid \"hi\" \\ / \t\u{e9}\u{1F600} \u{e9}",
                    true
                ),
            ]
        );
    }

    #[test]
    fn a_truncated_string_is_kept_as_far_as_it_goes() {
        assert_eq!(
            fields(r#"{"path":"src/ma"#),
            vec![field("path", "src/ma", false)]
        );
        // Inside an escape, a \u escape and a surrogate pair: the escape is left out.
        for cut in [
            r#"{"content":"a\"#,
            r#"{"content":"a\u00"#,
            r#"{"content":"a\ud83d"#,
            r#"{"content":"a\ud83d\"#,
            r#"{"content":"a\ud83d\ude0"#,
        ] {
            assert_eq!(fields(cut), vec![field("content", "a", false)], "{cut}");
        }
        // Inside a key, before the colon or the value: nothing of that field yet.
        for cut in [
            r#"{"pa"#,
            r#"{"path""#,
            r#"{"path":"#,
            r#"{"path": "#,
            "{",
            "",
            "  ",
        ] {
            assert_eq!(fields(cut), Vec::new(), "{cut:?}");
        }
        // Inside a nested value: the fields before it.
        assert_eq!(
            fields(r#"{"path":"a","edits":[{"old":"x"#),
            vec![field("path", "a", true)]
        );
    }

    #[test]
    fn broken_escapes_become_replacement_characters() {
        assert_eq!(
            fields(r#"{"a":"\ud83dx","b":"\ude00","c":"\uzz12","d":"\ud83dA"}"#),
            vec![
                field("a", "\u{FFFD}x", true),
                field("b", "\u{FFFD}", true),
                field("c", "\u{FFFD}zz12", true),
                field("d", "\u{FFFD}A", true),
            ]
        );
        assert_eq!(fields("[1, 2]"), Vec::new());
        assert_eq!(fields("not json"), Vec::new());
    }

    /// A realistic `write_file` input cut after every character: never a panic, every
    /// value the start of its final one, the target only once its string is complete,
    /// and lines that only grow.
    #[test]
    fn every_prefix_of_a_realistic_input_reads_sanely() {
        let content = (1..=30)
            .map(|line| format!("  .row-{line} {{ content: \"caf\u{e9} \u{1F600}\\\\ {line}\"; }}"))
            .collect::<Vec<_>>()
            .join("\n");
        // In the order a model writes it, with escapes a model may use and serde_json
        // does not.
        let input = format!(
            "{{\"path\": {}, \"options\": {{\"mode\": [1, 2, {{\"x\": \"]\"}}]}},\n \"content\": {}}}",
            serde_json::to_string("shop/st\u{fc}le \"main\".css").unwrap(),
            serde_json::to_string(&content).unwrap(),
        )
        .replace('\u{e9}', "\\u00e9")
        .replace('\u{1F600}', "\\ud83d\\ude00");
        let full = top_level_strings(&input);
        assert_eq!(full.len(), 2);
        assert!(full.iter().all(|field| field.complete));
        assert_eq!(full[1].value, content);

        let mut lines = 0;
        let mut checked = 0;
        for end in 0..=input.len() {
            if !input.is_char_boundary(end) {
                continue;
            }
            checked += 1;
            let prefix = &input[..end];
            for field in top_level_strings(prefix) {
                let whole = full
                    .iter()
                    .find(|whole| whole.key == field.key)
                    .unwrap_or_else(|| panic!("unknown key {:?} at {end}", field.key));
                assert!(whole.value.starts_with(&field.value), "{field:?} at {end}");
                if field.complete {
                    assert_eq!(field.value, whole.value, "at {end}");
                }
            }
            let preview = preview_tool_input("write_file", prefix);
            if let Some(target) = &preview.target {
                assert_eq!(target, "shop/st\u{fc}le \"main\".css", "at {end}");
            }
            assert!(preview.lines >= lines, "lines shrank at {end}");
            lines = preview.lines;
            if let Some(shown) = &preview.preview {
                assert!(shown.len() <= PREVIEW_BYTES, "at {end}");
                assert!(shown.lines().count() <= PREVIEW_LINES, "at {end}");
                assert!(content.contains(shown.as_str()), "{shown:?} at {end}");
            }
        }
        assert!(checked > input.len() / 2);
        assert_eq!(lines, 30);
    }

    #[test]
    fn each_tool_names_its_target_and_main_text() {
        let write = preview_tool_input(
            "write_file",
            r#"{"path":"site/index.html","content":"<html>\n<body>\n"#,
        );
        assert_eq!(write.target.as_deref(), Some("site/index.html"));
        assert_eq!(write.lines, 2);
        assert_eq!(write.preview.as_deref(), Some("<html>\n<body>"));

        // Only a complete string is a target.
        let partial = preview_tool_input("write_file", r#"{"content":"x","path":"site/ind"#);
        assert_eq!(partial.target, None);
        assert_eq!(partial.preview.as_deref(), Some("x"));

        let edit = preview_tool_input(
            "edit_file",
            r#"{"file_path":"a.rs","old_string":"a much longer old text","new_string":"new"}"#,
        );
        assert_eq!(edit.target.as_deref(), Some("a.rs"));
        assert_eq!(edit.preview.as_deref(), Some("new"));

        let bash = preview_tool_input(
            "bash",
            r#"{"description":"Write the file","command":"cat > a.txt <<'EOF'\nhello\nEOF"}"#,
        );
        assert_eq!(bash.target.as_deref(), Some("cat > a.txt <<'EOF'\u{2026}"));
        assert_eq!(bash.lines, 3);
        let shell = preview_tool_input("PowerShell", r#"{"command":"Get-ChildItem"}"#);
        assert_eq!(shell.target.as_deref(), Some("Get-ChildItem"));

        for (tool, input, target) in [
            (
                "glob_search",
                r#"{"path":"src","pattern":"**/*.rs"}"#,
                "**/*.rs",
            ),
            (
                "grep_search",
                r#"{"pattern":"fn main","path":"src"}"#,
                "fn main",
            ),
            (
                "WebFetch",
                r#"{"url":"https://example.com/a","prompt":"summarize"}"#,
                "https://example.com/a",
            ),
            (
                "WebSearch",
                r#"{"query":"rust partial json"}"#,
                "rust partial json",
            ),
            (
                "NotebookEdit",
                r#"{"notebook_path":"a.ipynb","new_source":"x = 1"}"#,
                "a.ipynb",
            ),
            (
                "Agent",
                r#"{"description":"Audit MCP","prompt":"Look at every server"}"#,
                "Audit MCP",
            ),
            (
                "read_file",
                r#"{"path":"README.md","limit":20}"#,
                "README.md",
            ),
            (
                "mcp__docs__fetch",
                r#"{"id":7,"url":"https://docs.rs"}"#,
                "https://docs.rs",
            ),
        ] {
            assert_eq!(
                preview_tool_input(tool, input).target.as_deref(),
                Some(target),
                "{tool}"
            );
        }
        let agent = preview_tool_input("Agent", r#"{"description":"Audit","prompt":"Look"}"#);
        assert_eq!(agent.preview.as_deref(), Some("Look"));
        let notebook = preview_tool_input(
            "NotebookEdit",
            r#"{"notebook_path":"a.ipynb","new_source":"x = 1"}"#,
        );
        assert_eq!(notebook.preview.as_deref(), Some("x = 1"));
        let fetch = preview_tool_input(
            "WebFetch",
            r#"{"url":"https://a.b","prompt":"summarize it"}"#,
        );
        assert_eq!(fetch.preview.as_deref(), Some("summarize it"));
        // A read has no main text: its path is the target, not a preview.
        let read = preview_tool_input("read_file", r#"{"path":"README.md"}"#);
        assert_eq!((read.lines, read.preview), (0, None));
        assert_eq!(preview_tool_input("bash", ""), ToolInputPreview::default());
        assert_eq!(
            preview_tool_input("bash", r#"{"command":"  "}"#).target,
            None
        );
    }

    #[test]
    fn the_preview_keeps_the_last_lines_and_about_a_kilobyte() {
        let text = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            tail(&format!("{text}\n")).as_deref(),
            Some("line 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\nline 20")
        );
        assert_eq!(tail("a\r\nb\r\n").as_deref(), Some("a\nb"));
        assert_eq!(tail(""), None);
        assert_eq!(tail("\n"), None);
        // One long line of two-byte characters: cut on a boundary, within the limit.
        let long = "\u{e9}".repeat(1000);
        let shown = tail(&long).expect("a preview");
        assert!(shown.len() <= PREVIEW_BYTES && shown.len() >= PREVIEW_BYTES - 1);
        assert!(shown.chars().all(|character| character == '\u{e9}'));

        let long_target = "x".repeat(300);
        let target = short_target(&long_target).expect("a target");
        assert_eq!(target.chars().count(), TARGET_CHARS + 1);
        assert!(target.ends_with('\u{2026}'));
    }
}
