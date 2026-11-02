//! Hook entries as settings files and plugins declare them: the command, an optional
//! matcher that picks which tool calls (or session sources, or compaction triggers) it
//! runs for, and where it came from.
//!
//! A hook list in settings is backwards compatible with the plain list of commands:
//!
//! ```json
//! "PreToolUse": [
//!   "audit.sh",
//!   {"matcher": "bash|write_file", "command": "guard.sh"},
//!   {"matcher": "mcp__.*", "hooks": [{"type": "command", "command": "mcp-audit.sh"}]}
//! ]
//! ```

use crate::json::JsonValue;
use regex::Regex;

/// Where a hook was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookSource {
    /// The user's own settings (`~/.neuron/settings.json`).
    User,
    /// The workspace's shared settings (`.neuron/settings.json`, `.neuron.json`).
    Project,
    /// The workspace's machine-local settings (`.neuron/settings.local.json`).
    Local,
    /// An enabled plugin.
    Plugin,
    /// Built in code (tests, embedders).
    Builtin,
}

impl HookSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Plugin => "plugin",
            Self::Builtin => "built-in",
        }
    }
}

/// One hook command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommand {
    pub command: String,
    /// `None` (or `*`) runs for everything.
    pub matcher: Option<String>,
    pub source: HookSource,
}

impl HookCommand {
    #[must_use]
    pub fn new(command: impl Into<String>, source: HookSource) -> Self {
        Self {
            command: command.into(),
            matcher: None,
            source,
        }
    }

    #[must_use]
    pub fn with_matcher(mut self, matcher: Option<String>) -> Self {
        self.matcher = matcher.filter(|matcher| !is_match_all(matcher));
        self
    }

    /// Whether this hook runs for `target` (a tool name, a session source or a
    /// compaction trigger). Events without a target run every hook.
    #[must_use]
    pub fn matches(&self, target: Option<&str>) -> bool {
        matcher_matches(self.matcher.as_deref(), target)
    }
}

fn is_match_all(matcher: &str) -> bool {
    let trimmed = matcher.trim();
    trimmed.is_empty() || trimmed == "*"
}

/// A matcher made only of names and `|` is a list of exact names (case-insensitive);
/// anything else is a regular expression that must match the whole target.
fn is_plain_alternation(matcher: &str) -> bool {
    matcher
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '|' | ' '))
}

/// Checks that a matcher is usable; the error says why not.
pub fn validate_matcher(matcher: &str) -> Result<(), String> {
    if is_match_all(matcher) || is_plain_alternation(matcher) {
        return Ok(());
    }
    compile(matcher)
        .map(|_| ())
        .map_err(|error| format!("matcher `{matcher}` is not a valid regular expression: {error}"))
}

fn compile(matcher: &str) -> Result<Regex, regex::Error> {
    Regex::new(&format!("(?i)^(?:{})$", matcher.trim()))
}

#[must_use]
pub fn matcher_matches(matcher: Option<&str>, target: Option<&str>) -> bool {
    let Some(matcher) = matcher.filter(|matcher| !is_match_all(matcher)) else {
        return true;
    };
    let Some(target) = target else {
        return true;
    };
    if is_plain_alternation(matcher) {
        return matcher
            .split('|')
            .map(str::trim)
            .any(|name| !name.is_empty() && name.eq_ignore_ascii_case(target));
    }
    // An invalid matcher is rejected when settings load; one that slips through
    // (a plugin, an embedder) matches nothing rather than everything.
    compile(matcher).is_ok_and(|regex| regex.is_match(target))
}

/// Parses one event's hook list: strings, `{"command", "matcher"?}` objects, or
/// `{"matcher"?, "hooks": [string | {"type"?: "command", "command"}]}` groups.
pub fn parse_hook_list(
    value: &JsonValue,
    source: HookSource,
    context: &str,
) -> Result<Vec<HookCommand>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{context} must be an array"));
    };
    let mut commands = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item_context = format!("{context}[{index}]");
        match item {
            JsonValue::String(command) => commands.push(HookCommand::new(command, source)),
            JsonValue::Object(object) => {
                let matcher = match object.get("matcher") {
                    None | Some(JsonValue::Null) => None,
                    Some(JsonValue::String(matcher)) => {
                        validate_matcher(matcher)
                            .map_err(|error| format!("{item_context}: {error}"))?;
                        Some(matcher.clone())
                    }
                    Some(_) => return Err(format!("{item_context}.matcher must be a string")),
                };
                let mut found = false;
                if let Some(command) = object.get("command") {
                    let command = command
                        .as_str()
                        .ok_or_else(|| format!("{item_context}.command must be a string"))?;
                    commands.push(HookCommand::new(command, source).with_matcher(matcher.clone()));
                    found = true;
                }
                if let Some(group) = object.get("hooks") {
                    let group = group
                        .as_array()
                        .ok_or_else(|| format!("{item_context}.hooks must be an array"))?;
                    for (inner_index, inner) in group.iter().enumerate() {
                        let command = match inner {
                            JsonValue::String(command) => command.as_str(),
                            JsonValue::Object(inner_object) => {
                                if let Some(kind) = inner_object.get("type") {
                                    if kind.as_str() != Some("command") {
                                        return Err(format!(
                                            "{item_context}.hooks[{inner_index}].type must be \"command\""
                                        ));
                                    }
                                }
                                inner_object
                                    .get("command")
                                    .and_then(JsonValue::as_str)
                                    .ok_or_else(|| {
                                        format!(
                                            "{item_context}.hooks[{inner_index}].command must be a string"
                                        )
                                    })?
                            }
                            _ => {
                                return Err(format!(
                                "{item_context}.hooks[{inner_index}] must be a string or an object"
                            ))
                            }
                        };
                        commands
                            .push(HookCommand::new(command, source).with_matcher(matcher.clone()));
                    }
                    found = true;
                }
                if !found {
                    return Err(format!(
                        "{item_context} needs a \"command\" string or a \"hooks\" array"
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "{item_context} must be a command string or an object with \"command\""
                ))
            }
        }
    }
    Ok(commands)
}

/// Whether `value` has the shape of a hook list (for the settings validator).
#[must_use]
pub fn is_hook_list(value: &JsonValue) -> bool {
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .all(|item| matches!(item, JsonValue::String(_) | JsonValue::Object(_)))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_hook_list, matcher_matches, parse_hook_list, validate_matcher, HookCommand, HookSource,
    };
    use crate::json::JsonValue;

    fn json(source: &str) -> JsonValue {
        JsonValue::parse(source).expect("test JSON")
    }

    #[test]
    fn absent_empty_and_star_matchers_match_everything() {
        for matcher in [None, Some(""), Some("*"), Some("  ")] {
            assert!(matcher_matches(matcher, Some("bash")));
            assert!(matcher_matches(matcher, None));
        }
    }

    #[test]
    fn plain_names_and_alternations_match_exact_names_ignoring_case() {
        assert!(matcher_matches(Some("bash"), Some("bash")));
        assert!(matcher_matches(Some("Bash"), Some("bash")));
        assert!(matcher_matches(
            Some("write_file|edit_file"),
            Some("edit_file")
        ));
        assert!(matcher_matches(
            Some("write_file | edit_file"),
            Some("write_file")
        ));
        assert!(!matcher_matches(
            Some("write_file|edit_file"),
            Some("read_file")
        ));
        // A name is not a prefix match.
        assert!(!matcher_matches(Some("bash"), Some("bash_output")));
    }

    #[test]
    fn other_matchers_are_whole_target_regexes() {
        assert!(matcher_matches(Some("mcp__.*"), Some("mcp__github__list")));
        assert!(!matcher_matches(Some("mcp__.*"), Some("bash")));
        assert!(matcher_matches(Some(".*_file"), Some("read_file")));
        assert!(!matcher_matches(Some("read.*"), Some("xread_file")));
    }

    #[test]
    fn events_without_a_target_run_every_hook() {
        assert!(matcher_matches(Some("bash"), None));
    }

    #[test]
    fn invalid_regexes_are_rejected_and_match_nothing() {
        let error = validate_matcher("mcp__(").expect_err("unbalanced group");
        assert!(error.contains("not a valid regular expression"), "{error}");
        assert!(!matcher_matches(Some("mcp__("), Some("mcp__(")));
        assert!(validate_matcher("bash|read_file").is_ok());
        assert!(validate_matcher("mcp__.*").is_ok());
    }

    #[test]
    fn parses_strings_objects_and_groups_in_order() {
        let list = json(
            r#"[
            "audit.sh",
            {"matcher": "bash", "command": "guard.sh"},
            {"matcher": "*", "command": "all.sh"},
            {"matcher": "mcp__.*", "hooks": [{"type": "command", "command": "mcp.sh"}, "mcp2.sh"]}
        ]"#,
        );
        let parsed = parse_hook_list(&list, HookSource::Project, "hooks.PreToolUse").unwrap();
        assert_eq!(
            parsed,
            vec![
                HookCommand::new("audit.sh", HookSource::Project),
                HookCommand::new("guard.sh", HookSource::Project).with_matcher(Some("bash".into())),
                HookCommand::new("all.sh", HookSource::Project),
                HookCommand::new("mcp.sh", HookSource::Project)
                    .with_matcher(Some("mcp__.*".into())),
                HookCommand::new("mcp2.sh", HookSource::Project)
                    .with_matcher(Some("mcp__.*".into())),
            ]
        );
    }

    #[test]
    fn rejects_malformed_entries_with_their_position() {
        let cases = [
            (json(r#""x""#), "must be an array"),
            (json(r#"[42]"#), "hooks.Stop[0] must be a command string"),
            (json(r#"[{"matcher": "bash"}]"#), "needs a \"command\""),
            (json(r#"[{"command": 1}]"#), "command must be a string"),
            (
                json(r#"[{"matcher": 3, "command": "x"}]"#),
                "matcher must be a string",
            ),
            (
                json(r#"[{"matcher": "(", "command": "x"}]"#),
                "not a valid regular expression",
            ),
            (
                json(r#"[{"hooks": [{"type": "prompt", "command": "x"}]}]"#),
                "type must be \"command\"",
            ),
        ];
        for (value, expected) in cases {
            let error = parse_hook_list(&value, HookSource::User, "hooks.Stop")
                .expect_err("malformed hook list");
            assert!(
                error.contains(expected),
                "{error} should contain {expected}"
            );
        }
    }

    #[test]
    fn hook_list_shape_accepts_strings_and_objects_only() {
        assert!(is_hook_list(&json(r#"["a", {"command": "b"}]"#)));
        assert!(!is_hook_list(&json(r#"["a", 1]"#)));
        assert!(!is_hook_list(&json(r#""a""#)));
    }
}
