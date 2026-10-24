//! Tool-call input as weaker models send it: arguments that are almost JSON, and
//! tool names that are almost right.
//!
//! [`parse_tool_input`] accepts a valid JSON object as it is and otherwise tries a
//! small set of safe repairs (code fences, text around the object, a JSON string
//! holding the object, raw line breaks inside strings, trailing commas, up to two
//! missing closing brackets). It never closes an unterminated string: that is what a
//! cut-off write looks like, and running it would write half a file.
//! [`unknown_tool_message`] names the closest real tools and lists them all.

use serde_json::Value;

/// Missing `}`/`]` a repair may add; more means the call was cut off.
const MAX_ADDED_CLOSERS: usize = 2;
/// Tool names listed in an unknown-tool error.
const MAX_LISTED_TOOLS: usize = 40;

/// A tool call's arguments, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInput {
    pub value: Value,
    /// What was repaired, when the arguments were not valid JSON as sent.
    pub repaired: Option<String>,
}

/// Parses a tool call's arguments into a JSON object, repairing near-JSON (see the
/// module docs). The error says what was wrong and how to send it instead.
pub fn parse_tool_input(input: &str) -> Result<ToolInput, String> {
    if input.trim().is_empty() {
        return Err(
            "invalid tool input: the arguments are empty; send them as one JSON object".to_string(),
        );
    }
    match serde_json::from_str::<Value>(input) {
        Ok(value) if value.is_object() => Ok(ToolInput {
            value,
            repaired: None,
        }),
        Ok(value) => repair_tool_json(input).ok_or_else(|| {
            format!(
                "invalid tool input: expected a JSON object of arguments, got {}",
                json_kind(&value)
            )
        }),
        Err(error) => repair_tool_json(input).ok_or_else(|| {
            format!(
                "invalid tool input JSON ({error}). Send the arguments as one JSON object, for example {{\"path\": \"src/main.rs\"}}, with line breaks inside strings written as \\n and quotes inside strings escaped as \\\"."
            )
        }),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Repairs near-JSON tool arguments into a JSON object, or `None` when no safe
/// repair applies.
#[must_use]
pub fn repair_tool_json(input: &str) -> Option<ToolInput> {
    let mut fixes = Vec::new();
    let mut text = input.trim();

    if let Some(inner) = strip_code_fence(text) {
        fixes.push("removed a code fence");
        text = inner;
    }
    // A JSON string that holds the object ("{\"path\": ...}").
    if text.starts_with('"') {
        if let Ok(Value::String(inner)) = serde_json::from_str::<Value>(text) {
            if let Ok(value) = serde_json::from_str::<Value>(&inner) {
                if value.is_object() {
                    fixes.push("decoded an object sent as a JSON string");
                    return Some(ToolInput {
                        value,
                        repaired: Some(fixes.join(", ")),
                    });
                }
            }
        }
    }
    let start = text.find('{')?;
    if start > 0 {
        fixes.push("dropped text before the object");
    }
    let (normalized, scan) = normalize_object(&text[start..])?;
    if scan.escaped_controls {
        fixes.push("escaped line breaks or tabs inside strings");
    }
    if scan.trailing_commas {
        fixes.push("removed trailing commas");
    }
    if scan.added_closers > 0 {
        fixes.push("added missing closing brackets");
    }
    if scan.trailing_text {
        fixes.push("dropped text after the object");
    }
    let value = serde_json::from_str::<Value>(&normalized).ok()?;
    if !value.is_object() || fixes.is_empty() {
        return None;
    }
    Some(ToolInput {
        value,
        repaired: Some(fixes.join(", ")),
    })
}

fn strip_code_fence(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("```")?;
    let rest = rest.split_once('\n').map_or("", |(_, body)| body);
    Some(rest.trim_end().strip_suffix("```").unwrap_or(rest).trim())
}

#[derive(Default)]
struct Scan {
    escaped_controls: bool,
    trailing_commas: bool,
    added_closers: usize,
    trailing_text: bool,
}

/// Rewrites one JSON object (from its first `{`): control characters inside strings
/// escaped, trailing commas removed, text after the object dropped, and up to
/// [`MAX_ADDED_CLOSERS`] missing closers added. `None` when a string is left open or
/// more closers are missing.
fn normalize_object(text: &str) -> Option<(String, Scan)> {
    let mut out = String::with_capacity(text.len() + 8);
    let mut scan = Scan::default();
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut closed_at = None;

    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                out.push(ch);
                continue;
            }
            match ch {
                '\\' => {
                    escaped = true;
                    out.push(ch);
                }
                '"' => {
                    in_string = false;
                    out.push(ch);
                }
                '\n' => {
                    scan.escaped_controls = true;
                    out.push_str("\\n");
                }
                '\r' => {
                    scan.escaped_controls = true;
                    out.push_str("\\r");
                }
                '\t' => {
                    scan.escaped_controls = true;
                    out.push_str("\\t");
                }
                control if u32::from(control) < 0x20 => {
                    scan.escaped_controls = true;
                    out.push_str(&format!("\\u{:04x}", u32::from(control)));
                }
                _ => out.push(ch),
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '{' | '[' => {
                stack.push(ch);
                out.push(ch);
            }
            '}' | ']' => {
                if drop_trailing_comma(&mut out) {
                    scan.trailing_commas = true;
                }
                stack.pop();
                out.push(ch);
                if stack.is_empty() {
                    closed_at = Some(index + ch.len_utf8());
                    break;
                }
            }
            _ => out.push(ch),
        }
    }

    if in_string {
        return None;
    }
    match closed_at {
        Some(end) => {
            if !text[end..].trim().is_empty() {
                scan.trailing_text = true;
            }
        }
        None => {
            if stack.len() > MAX_ADDED_CLOSERS {
                return None;
            }
            while let Some(open) = stack.pop() {
                if drop_trailing_comma(&mut out) {
                    scan.trailing_commas = true;
                }
                out.push(if open == '{' { '}' } else { ']' });
                scan.added_closers += 1;
            }
        }
    }
    Some((out, scan))
}

/// Removes a `,` (and the whitespace after it) at the end of `out`.
fn drop_trailing_comma(out: &mut String) -> bool {
    let trimmed = out.trim_end().len();
    if out[..trimmed].ends_with(',') {
        out.truncate(trimmed - 1);
        true
    } else {
        false
    }
}

/// Well-known tool names from other agents, mapped to Neuron's.
const ALIASES: &[(&str, &str)] = &[
    ("read", "read_file"),
    ("readfile", "read_file"),
    ("cat", "read_file"),
    ("view", "read_file"),
    ("open", "read_file"),
    ("write", "write_file"),
    ("writefile", "write_file"),
    ("create", "write_file"),
    ("createfile", "write_file"),
    ("edit", "edit_file"),
    ("editfile", "edit_file"),
    ("strreplace", "edit_file"),
    ("strreplaceeditor", "edit_file"),
    ("replace", "edit_file"),
    ("multiedit", "edit_file"),
    ("applypatch", "edit_file"),
    ("patch", "edit_file"),
    ("shell", "bash"),
    ("terminal", "bash"),
    ("run", "bash"),
    ("runcommand", "bash"),
    ("runterminalcommand", "bash"),
    ("execute", "bash"),
    ("exec", "bash"),
    ("command", "bash"),
    ("grep", "grep_search"),
    ("search", "grep_search"),
    ("rg", "grep_search"),
    ("ripgrep", "grep_search"),
    ("codesearch", "grep_search"),
    ("searchcode", "grep_search"),
    ("glob", "glob_search"),
    ("find", "glob_search"),
    ("findfiles", "glob_search"),
    ("ls", "glob_search"),
    ("listfiles", "glob_search"),
    ("listdirectory", "glob_search"),
    ("listdir", "glob_search"),
    ("todo", "TodoWrite"),
    ("todos", "TodoWrite"),
    ("writetodos", "TodoWrite"),
    ("fetch", "WebFetch"),
    ("readurl", "WebFetch"),
    ("websearch", "WebSearch"),
    ("searchweb", "WebSearch"),
];

fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (row, a) in left.chars().enumerate() {
        let mut current = vec![row + 1; right.len() + 1];
        for (column, b) in right.iter().enumerate() {
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + usize::from(a != *b));
        }
        previous = current;
    }
    previous[right.len()]
}

/// Up to three real tool names close to `name`: a known alias, then names that
/// contain it (or are contained in it), then near spellings.
#[must_use]
pub fn suggest_tool_names(name: &str, candidates: &[&str]) -> Vec<String> {
    let wanted = squash(name);
    let mut suggestions: Vec<String> = Vec::new();
    let mut push = |candidate: &str| {
        if !suggestions.iter().any(|existing| existing == candidate) && suggestions.len() < 3 {
            suggestions.push(candidate.to_string());
        }
    };
    if let Some((_, target)) = ALIASES.iter().find(|(alias, _)| *alias == wanted) {
        if candidates.contains(target) {
            push(target);
        }
    }
    if wanted.len() >= 3 {
        for candidate in candidates {
            let squashed = squash(candidate);
            if squashed.contains(&wanted) || wanted.contains(&squashed) {
                push(candidate);
            }
        }
    }
    let mut near = candidates
        .iter()
        .map(|candidate| (edit_distance(&wanted, &squash(candidate)), *candidate))
        .filter(|(distance, _)| *distance <= 2)
        .collect::<Vec<_>>();
    near.sort();
    for (_, candidate) in near {
        push(candidate);
    }
    suggestions
}

/// The error for a call to a tool that does not exist: the closest real names and the
/// list of tools the model can call.
#[must_use]
pub fn unknown_tool_message(name: &str, candidates: &[&str]) -> String {
    let suggestions = suggest_tool_names(name, candidates);
    let hint = match suggestions.as_slice() {
        [] => String::new(),
        [only] => format!(" Did you mean `{only}`?"),
        several => format!(
            " Did you mean one of: {}?",
            several
                .iter()
                .map(|candidate| format!("`{candidate}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut listed = candidates.iter().take(MAX_LISTED_TOOLS).copied().collect::<Vec<_>>();
    listed.sort_unstable();
    let more = if candidates.len() > MAX_LISTED_TOOLS {
        ", ..."
    } else {
        ""
    };
    format!(
        "unsupported tool: `{name}` does not exist.{hint} Call one of the available tools: {}{more}.",
        listed.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_tool_input, repair_tool_json, suggest_tool_names, unknown_tool_message};

    #[test]
    fn valid_objects_pass_through_untouched() {
        let parsed = parse_tool_input(r#"{"path":"a.rs"}"#).expect("valid");
        assert_eq!(parsed.value, json!({"path": "a.rs"}));
        assert_eq!(parsed.repaired, None);
    }

    #[test]
    fn near_json_is_repaired_and_says_how() {
        let cases = [
            ("```json\n{\"path\": \"a.rs\"}\n```", json!({"path": "a.rs"}), "code fence"),
            ("\"{\\\"path\\\": \\\"a.rs\\\"}\"", json!({"path": "a.rs"}), "JSON string"),
            ("Calling the tool: {\"path\": \"a.rs\"}", json!({"path": "a.rs"}), "text before"),
            ("{\"content\": \"line one\nline two\"}", json!({"content": "line one\nline two"}), "line breaks"),
            ("{\"paths\": [\"a\", \"b\",], \"x\": 1,}", json!({"paths": ["a", "b"], "x": 1}), "trailing commas"),
            ("{\"path\": \"a.rs\", \"edits\": [{\"x\": 1}", json!({"path": "a.rs", "edits": [{"x": 1}]}), "closing brackets"),
            ("{\"path\": \"a.rs\"} and then some prose", json!({"path": "a.rs"}), "text after"),
        ];
        for (input, expected, fix) in cases {
            let parsed = parse_tool_input(input).unwrap_or_else(|error| panic!("{input}: {error}"));
            assert_eq!(parsed.value, expected, "{input}");
            assert!(parsed.repaired.as_deref().is_some_and(|text| text.contains(fix)), "{input}: {:?}", parsed.repaired);
        }
    }

    #[test]
    fn cut_off_calls_are_not_repaired() {
        // An open string is what a write cut off at the output limit looks like.
        assert!(repair_tool_json("{\"path\": \"a.rs\", \"content\": \"fn main() {").is_none());
        // Too many closers missing.
        assert!(repair_tool_json("{\"a\": {\"b\": {\"c\": [1").is_none());
        let error = parse_tool_input("{\"path\": \"a.rs\", \"content\": \"half").expect_err("cut off");
        assert!(error.contains("invalid tool input JSON") && error.contains("\\n"), "{error}");
        assert!(parse_tool_input("[1, 2]").expect_err("array").contains("got an array"));
        assert!(parse_tool_input("  ").expect_err("empty").contains("empty"));
    }

    #[test]
    fn unknown_tools_get_the_closest_real_names() {
        let tools = ["bash", "read_file", "write_file", "edit_file", "glob_search", "grep_search", "TodoWrite"];
        assert_eq!(suggest_tool_names("Edit", &tools), vec!["edit_file"]);
        assert_eq!(suggest_tool_names("str_replace_editor", &tools), vec!["edit_file"]);
        assert_eq!(suggest_tool_names("run_terminal_command", &tools), vec!["bash"]);
        assert_eq!(suggest_tool_names("read_fiel", &tools), vec!["read_file"]);
        assert_eq!(suggest_tool_names("grep", &tools)[0], "grep_search");
        assert!(suggest_tool_names("teleport", &tools).is_empty());
        let message = unknown_tool_message("Read", &tools);
        assert!(message.starts_with("unsupported tool: `Read` does not exist. Did you mean `read_file`?"), "{message}");
        assert!(message.contains("TodoWrite, bash, edit_file"), "{message}");
    }
}
