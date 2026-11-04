//! Which permission an `Agent` call needs, by the kind of sub-agent it starts.
//!
//! Starting a sub-agent used to need full access whatever it did. A sub-agent whose
//! tools only read cannot change anything its parent could not read itself, so:
//!
//! - read-only types ([`READ_ONLY_SUBAGENT_TYPES`], and `Verification` when the
//!   session offers no shell tool) need [`PermissionMode::ReadOnly`];
//! - any other type in a git worktree of its own (`isolation: "worktree"`) needs
//!   [`PermissionMode::WorkspaceWrite`]: it writes only in that worktree, and nothing
//!   reaches the user's tree until someone merges its branch;
//! - everything else still needs [`PermissionMode::DangerFullAccess`].
//!
//! The sub-agent itself runs with its parent's mode and rules, never more, so a
//! read-only type in a read-only session stays read-only all the way down.
//!
//! The same split decides which `Agent` calls of one response may run at the same
//! time ([`agent_call_runs_in_parallel`]): read-only ones and worktree-isolated ones.

use crate::permissions::PermissionMode;

/// Sub-agent types whose tools only read (files, searches, web reads).
pub const READ_ONLY_SUBAGENT_TYPES: &[&str] = &["Explore", "Plan", "claw-guide"];

/// Sub-agent types that are read-only only when the session has no shell tool
/// (`bash`, `PowerShell`): without one they read and report.
pub const READ_ONLY_WITHOUT_SHELL_SUBAGENT_TYPES: &[&str] = &["Verification"];

/// The `isolation` value that runs a sub-agent in a worktree of its own.
pub const WORKTREE_ISOLATION: &str = "worktree";

/// The canonical name of a sub-agent type as the model wrote it (`explore`,
/// `Explorer`, `explore-agent` are all `Explore`); `general-purpose` when empty.
#[must_use]
pub fn canonical_subagent_type(subagent_type: Option<&str>) -> String {
    let trimmed = subagent_type.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return String::from("general-purpose");
    }
    let mut token = trimmed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if let Some(stripped) = token.strip_suffix("tool") {
        token = stripped.to_string();
    }
    match token.as_str() {
        "general" | "generalpurpose" | "generalpurposeagent" => String::from("general-purpose"),
        "explore" | "explorer" | "exploreagent" => String::from("Explore"),
        "plan" | "planagent" => String::from("Plan"),
        "verification" | "verificationagent" | "verify" | "verifier" => {
            String::from("Verification")
        }
        "clawguide" | "clawguideagent" | "guide" => String::from("claw-guide"),
        "statusline" | "statuslinesetup" => String::from("statusline-setup"),
        _ => trimmed.to_string(),
    }
}

/// Whether a sub-agent of `subagent_type` (canonical) only reads. `shell_tools` says
/// whether the session offers `bash` or `PowerShell` at all.
#[must_use]
pub fn is_read_only_subagent(subagent_type: &str, shell_tools: bool) -> bool {
    READ_ONLY_SUBAGENT_TYPES.contains(&subagent_type)
        || (!shell_tools && READ_ONLY_WITHOUT_SHELL_SUBAGENT_TYPES.contains(&subagent_type))
}

/// The parts of an `Agent` call's input the tier depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCallShape {
    /// The canonical sub-agent type.
    pub subagent_type: String,
    /// Whether it asks for a worktree of its own.
    pub worktree: bool,
}

impl AgentCallShape {
    /// Reads an `Agent` call's JSON input; `None` when it is not a JSON object.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(input).ok()?;
        let object = value.as_object()?;
        let field = |key: &str| object.get(key).and_then(serde_json::Value::as_str);
        Some(Self {
            subagent_type: canonical_subagent_type(field("subagent_type")),
            worktree: field("isolation").map(str::trim) == Some(WORKTREE_ISOLATION),
        })
    }
}

/// The permission an `Agent` call with `input` needs (see the module docs).
#[must_use]
pub fn agent_call_requirement(input: &str, shell_tools: bool) -> PermissionMode {
    match AgentCallShape::parse(input) {
        Some(shape) if is_read_only_subagent(&shape.subagent_type, shell_tools) => {
            PermissionMode::ReadOnly
        }
        Some(shape) if shape.worktree => PermissionMode::WorkspaceWrite,
        _ => PermissionMode::DangerFullAccess,
    }
}

/// Whether an `Agent` call may run at the same time as other such calls of the same
/// response: it starts a read-only sub-agent, or one in a worktree of its own.
/// Writing sub-agents that share the user's tree run one at a time.
#[must_use]
pub fn agent_call_runs_in_parallel(input: &str, shell_tools: bool) -> bool {
    AgentCallShape::parse(input).is_some_and(|shape| {
        shape.worktree || is_read_only_subagent(&shape.subagent_type, shell_tools)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        agent_call_requirement, agent_call_runs_in_parallel, canonical_subagent_type,
        is_read_only_subagent,
    };
    use crate::permissions::PermissionMode;

    #[test]
    fn type_names_are_canonical() {
        assert_eq!(canonical_subagent_type(None), "general-purpose");
        assert_eq!(canonical_subagent_type(Some("  ")), "general-purpose");
        assert_eq!(canonical_subagent_type(Some("explorer")), "Explore");
        assert_eq!(canonical_subagent_type(Some("Plan-Agent")), "Plan");
        assert_eq!(canonical_subagent_type(Some("verify")), "Verification");
        assert_eq!(canonical_subagent_type(Some("Custom")), "Custom");
    }

    #[test]
    fn read_only_types_need_read_only_access() {
        for kind in ["Explore", "plan", "claw-guide"] {
            let input = format!(r#"{{"description":"d","prompt":"p","subagent_type":"{kind}"}}"#);
            assert_eq!(
                agent_call_requirement(&input, true),
                PermissionMode::ReadOnly,
                "{kind}"
            );
        }
    }

    #[test]
    fn verification_is_read_only_only_without_a_shell() {
        let input = r#"{"description":"d","prompt":"p","subagent_type":"Verification"}"#;
        assert_eq!(
            agent_call_requirement(input, true),
            PermissionMode::DangerFullAccess
        );
        assert_eq!(
            agent_call_requirement(input, false),
            PermissionMode::ReadOnly
        );
        assert!(is_read_only_subagent("Verification", false));
        assert!(!is_read_only_subagent("Verification", true));
    }

    #[test]
    fn writers_need_full_access_outside_a_worktree_and_workspace_write_inside() {
        let shared = r#"{"description":"d","prompt":"p"}"#;
        let isolated = r#"{"description":"d","prompt":"p","isolation":"worktree"}"#;
        assert_eq!(
            agent_call_requirement(shared, true),
            PermissionMode::DangerFullAccess
        );
        assert_eq!(
            agent_call_requirement(isolated, true),
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            agent_call_requirement("not json", true),
            PermissionMode::DangerFullAccess
        );
    }

    #[test]
    fn only_read_only_and_isolated_calls_run_in_parallel() {
        assert!(agent_call_runs_in_parallel(
            r#"{"description":"d","prompt":"p","subagent_type":"Explore"}"#,
            true
        ));
        assert!(agent_call_runs_in_parallel(
            r#"{"description":"d","prompt":"p","isolation":"worktree"}"#,
            true
        ));
        assert!(!agent_call_runs_in_parallel(
            r#"{"description":"d","prompt":"p"}"#,
            true
        ));
        assert!(!agent_call_runs_in_parallel("[]", true));
    }
}
