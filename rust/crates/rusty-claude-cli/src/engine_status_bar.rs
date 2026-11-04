//! The engine's side of the full-screen UI's prompt status bar
//! ([`neuron_tui::protocol::FEATURE_STATUS_BAR`]): who serves the model and its thinking
//! levels (from the gateway catalog), what fills the context window, which permission
//! mode a status-bar choice means, and the `/context` report.

use neuron_tui::protocol::{ContextBreakdown, ReportSection};
use runtime::context_manager::{estimate_block_tokens, estimate_text_tokens, ContextSnapshot};
use runtime::{ContentBlock, ConversationMessage, PermissionMode};

use crate::engine_reports::Report;

/// The heading the system prompt's project-instructions section starts with.
const INSTRUCTIONS_HEADING: &str = "# Project instructions";

/// Levels a model without a catalog entry takes (`--reasoning-effort`'s).
pub(crate) const DEFAULT_REASONING_EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// What the status bar shows about the current model, from the gateway catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModelFields {
    pub(crate) provider: Option<String>,
    pub(crate) display_name: Option<String>,
    /// `None` when unknown; empty when the model does not reason.
    pub(crate) reasoning_efforts: Option<Vec<String>>,
}

/// The status fields [`neuron_tui::protocol::FEATURE_STATUS_BAR`] adds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatusBarFields {
    pub(crate) model: ModelFields,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) full_access_allowed: bool,
    pub(crate) context: Option<ContextBreakdown>,
}

/// The provider, display name and thinking levels of `model`. `auto` is Zero-X's router.
pub(crate) fn model_fields(model: &str) -> ModelFields {
    let entry = crate::gateway_catalog::model_entry(model);
    let is_auto = crate::provider::shown_model(model)
        .trim()
        .eq_ignore_ascii_case(crate::provider::AUTO_MODEL);
    let Some(entry) = entry else {
        return ModelFields {
            provider: is_auto.then(|| "zero-x".to_string()),
            ..ModelFields::default()
        };
    };
    let reasoning_efforts = if entry.supports_reasoning == Some(false) {
        Some(Vec::new())
    } else {
        entry.reasoning_efforts.clone()
    };
    ModelFields {
        provider: entry
            .provider
            .clone()
            .or_else(|| is_auto.then(|| "zero-x".to_string())),
        // The name without the gateway's route to it: `GLM-4.7 Flash`, not
        // `GLM-4.7 Flash (OpenRouter, free)`.
        display_name: entry
            .display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| {
                crate::gateway_catalog::clean_model_name(
                    &entry.id,
                    Some(name),
                    entry.provider.as_deref(),
                )
            }),
        reasoning_efforts,
    }
}

/// The level `requested` means for a model that takes `efforts`: one of them (in its own
/// spelling), or `None` for the model's default (`default`, `auto` or nothing, unless the
/// model lists `default` itself). Without a list, the levels `--reasoning-effort` takes.
pub(crate) fn resolve_reasoning_effort(
    requested: Option<&str>,
    efforts: Option<&[String]>,
) -> Result<Option<String>, String> {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let wanted = requested.to_ascii_lowercase();
    let known: Vec<String> = match efforts {
        Some(efforts) if !efforts.is_empty() => efforts.to_vec(),
        Some(_) => return Err("This model has no thinking levels to choose from.".to_string()),
        None => DEFAULT_REASONING_EFFORTS
            .iter()
            .map(ToString::to_string)
            .collect(),
    };
    if let Some(level) = known
        .iter()
        .find(|level| level.eq_ignore_ascii_case(&wanted))
    {
        return Ok(Some(level.clone()));
    }
    if matches!(wanted.as_str(), "default" | "auto") {
        return Ok(None);
    }
    Err(format!(
        "Unknown thinking level `{requested}`. This model takes: {}, or default.",
        known.join(", ")
    ))
}

/// The thinking level a model switch keeps: `kept` in the spelling of the new model,
/// which takes `efforts`, or `None` (the model's default) when it does not take that
/// level or has none.
pub(crate) fn carried_reasoning_effort(
    kept: Option<&str>,
    efforts: Option<&[String]>,
) -> Option<String> {
    resolve_reasoning_effort(kept, efforts).ok().flatten()
}

/// What a status-bar permission choice asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionChoice {
    /// Plan mode on, whatever the mode under it.
    PlanOn,
    /// Plan mode off: the mode from before it comes back.
    PlanOff,
    /// This mode, with plan mode off.
    Mode(PermissionMode),
}

/// `read-only`, `workspace-write`, `danger-full-access`, `plan` or `plan-off`. Full access
/// is refused unless the engine started with it.
pub(crate) fn parse_permission_choice(
    mode: &str,
    full_access_allowed: bool,
) -> Result<PermissionChoice, String> {
    match mode.trim() {
        "plan" | "plan-on" => Ok(PermissionChoice::PlanOn),
        "plan-off" => Ok(PermissionChoice::PlanOff),
        "read-only" => Ok(PermissionChoice::Mode(PermissionMode::ReadOnly)),
        "workspace-write" => Ok(PermissionChoice::Mode(PermissionMode::WorkspaceWrite)),
        "danger-full-access" if full_access_allowed => {
            Ok(PermissionChoice::Mode(PermissionMode::DangerFullAccess))
        }
        "danger-full-access" => Err(
            "Full access is only available when Neuron starts with it (--permission-mode danger-full-access)."
                .to_string(),
        ),
        other => Err(format!(
            "Unknown permission mode `{other}`. Use read-only, workspace-write, danger-full-access, plan or plan-off."
        )),
    }
}

/// The system prompt's project-instructions section and the files it names.
fn instructions(system_prompt: &[String]) -> (usize, Vec<String>) {
    let Some(section) = system_prompt
        .iter()
        .find(|section| section.starts_with(INSTRUCTIONS_HEADING))
    else {
        return (0, Vec::new());
    };
    // The prompt's estimate counts each section with two tokens of its own.
    let tokens = estimate_text_tokens(section) + 2;
    let files = section
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .filter_map(|line| line.rsplit_once(" (scope: ").map(|(path, _)| path))
        .map(str::to_string)
        .collect();
    (tokens, files)
}

/// What fills the context window: the snapshot's budget split into the system prompt,
/// tool schemas, project instructions, conversation and tool outputs. The parts add up to
/// `used_tokens`.
pub(crate) fn context_breakdown(
    snapshot: &ContextSnapshot,
    system_prompt: &[String],
    messages: &[ConversationMessage],
    tool_count: usize,
) -> ContextBreakdown {
    let (instruction_tokens, instruction_files) = instructions(system_prompt);
    let instruction_tokens = instruction_tokens.min(snapshot.system_prompt_tokens);
    let tool_output_tokens: usize = messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
        .map(estimate_block_tokens)
        .sum();
    let tool_output_tokens = tool_output_tokens.min(snapshot.estimated_tokens);
    let budget = snapshot.budget;
    let used =
        snapshot.system_prompt_tokens + budget.tool_schema_tokens + snapshot.estimated_tokens;
    // The threshold counts the transcript only; the meter counts what a request sends.
    let threshold =
        snapshot.threshold_tokens + snapshot.system_prompt_tokens + budget.tool_schema_tokens;
    let as_u64 = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    ContextBreakdown {
        context_window: as_u64(budget.context_window),
        output_reserve: as_u64(budget.output_reserve),
        system_tokens: as_u64(snapshot.system_prompt_tokens - instruction_tokens),
        tool_schema_tokens: as_u64(budget.tool_schema_tokens),
        tool_count: as_u64(tool_count),
        instruction_tokens: as_u64(instruction_tokens),
        instruction_files,
        conversation_tokens: as_u64(snapshot.estimated_tokens - tool_output_tokens),
        tool_output_tokens: as_u64(tool_output_tokens),
        used_tokens: as_u64(used),
        threshold_tokens: as_u64(threshold),
    }
}

fn percent_of(tokens: u64, usable: u64) -> String {
    if usable == 0 {
        return "-".to_string();
    }
    #[allow(clippy::cast_precision_loss)]
    let share = tokens as f64 * 100.0 / usable as f64;
    format!("{share:.1}%")
}

/// `/context` as a report: the breakdown as a table, against the usable window.
pub(crate) fn context_report(model: &str, context: &ContextBreakdown) -> Report {
    let usable = context
        .context_window
        .saturating_sub(context.output_reserve);
    let digits = crate::account::group_digits;
    let free = usable.saturating_sub(context.used_tokens);
    let instructions = if context.instruction_files.is_empty() {
        "Project instructions & memory".to_string()
    } else {
        format!(
            "Project instructions & memory ({})",
            context.instruction_files.join(", ")
        )
    };
    let rows = [
        ("System prompt".to_string(), context.system_tokens),
        (
            format!("Tools ({})", context.tool_count),
            context.tool_schema_tokens,
        ),
        (instructions, context.instruction_tokens),
        ("Conversation".to_string(), context.conversation_tokens),
        ("Tool outputs".to_string(), context.tool_output_tokens),
        ("Free".to_string(), free),
    ];
    let mut table = String::from("| Part | Tokens | Share |\n|---|---:|---:|\n");
    for (label, tokens) in rows {
        table.push_str(&format!(
            "| {label} | ~{} | {} |\n",
            digits(tokens),
            percent_of(tokens, usable)
        ));
    }
    let summary = format!(
        "~{} of {} usable tokens ({}) · {} kept for the reply · auto-compacts at ~{}. `/compact` summarizes now.",
        digits(context.used_tokens),
        digits(usable),
        percent_of(context.used_tokens, usable),
        digits(context.output_reserve),
        digits(context.threshold_tokens),
    );
    Report {
        command: "context",
        title: format!(
            "Context · {} · {} window",
            crate::provider::shown_model(model),
            digits(context.context_window)
        ),
        sections: vec![
            ReportSection {
                heading: None,
                markdown: summary,
            },
            ReportSection {
                heading: Some("What fills it".to_string()),
                markdown: table,
            },
        ],
        links: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::context_manager::{ContextBudget, ContextManager, ContextPolicy};
    use runtime::MessageRole;

    fn efforts(levels: &[&str]) -> Vec<String> {
        levels.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn thinking_levels_follow_the_model_and_default_clears_them() {
        let gemini = efforts(&["none", "low", "medium", "high"]);
        assert_eq!(
            resolve_reasoning_effort(Some("HIGH"), Some(&gemini)),
            Ok(Some("high".into()))
        );
        assert_eq!(
            resolve_reasoning_effort(Some("none"), Some(&gemini)),
            Ok(Some("none".into()))
        );
        assert_eq!(
            resolve_reasoning_effort(Some("default"), Some(&gemini)),
            Ok(None)
        );
        assert_eq!(resolve_reasoning_effort(None, Some(&gemini)), Ok(None));
        assert!(resolve_reasoning_effort(Some("max"), Some(&gemini)).is_err());
        // A model whose own vocabulary has `default` keeps it.
        let qwen = efforts(&["none", "default"]);
        assert_eq!(
            resolve_reasoning_effort(Some("default"), Some(&qwen)),
            Ok(Some("default".into()))
        );
        // Without a catalog: the launch flag's levels.
        assert_eq!(
            resolve_reasoning_effort(Some("low"), None),
            Ok(Some("low".into()))
        );
        assert!(resolve_reasoning_effort(Some("xhigh"), None).is_err());
        assert!(resolve_reasoning_effort(Some("low"), Some(&[])).is_err());
    }

    #[test]
    fn a_model_switch_keeps_only_a_level_the_new_model_takes() {
        let gemini = efforts(&["none", "low", "medium", "high"]);
        let gpt = efforts(&["low", "medium", "high", "xhigh"]);
        assert_eq!(
            carried_reasoning_effort(Some("high"), Some(&gpt)),
            Some("high".into())
        );
        // Gemini's `none` means nothing to a model without it: its default instead.
        assert_eq!(carried_reasoning_effort(Some("none"), Some(&gpt)), None);
        assert_eq!(
            carried_reasoning_effort(Some("none"), Some(&gemini)),
            Some("none".into())
        );
        // A model that does not reason, or an unknown one, takes no `xhigh`.
        assert_eq!(carried_reasoning_effort(Some("high"), Some(&[])), None);
        assert_eq!(carried_reasoning_effort(Some("xhigh"), None), None);
        assert_eq!(
            carried_reasoning_effort(Some("MEDIUM"), None),
            Some("medium".into())
        );
        assert_eq!(carried_reasoning_effort(None, Some(&gpt)), None);
    }

    #[test]
    fn full_access_is_refused_unless_the_engine_started_with_it() {
        assert_eq!(
            parse_permission_choice("danger-full-access", true),
            Ok(PermissionChoice::Mode(PermissionMode::DangerFullAccess))
        );
        assert!(parse_permission_choice("danger-full-access", false).is_err());
        assert_eq!(
            parse_permission_choice("read-only", false),
            Ok(PermissionChoice::Mode(PermissionMode::ReadOnly))
        );
        assert_eq!(
            parse_permission_choice("plan", false),
            Ok(PermissionChoice::PlanOn)
        );
        assert_eq!(
            parse_permission_choice("plan-off", false),
            Ok(PermissionChoice::PlanOff)
        );
        assert!(parse_permission_choice("yolo", true).is_err());
    }

    #[test]
    fn the_breakdown_adds_up_to_what_a_request_sends() {
        let system_prompt = vec![
            "You are Neuron.".repeat(40),
            format!(
                "{INSTRUCTIONS_HEADING}\n\n## NEURON.md (scope: /work)\n\nUse tabs.\n\n## Not a file heading\n\nmore"
            ),
        ];
        let messages = vec![
            ConversationMessage::user_text("fix the bug please"),
            ConversationMessage {
                role: MessageRole::Assistant,
                blocks: vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input: "{\"path\":\"a.rs\"}".into(),
                }],
                usage: None,
            },
            ConversationMessage::tool_result("t1", "read_file", "fn main() {}".repeat(50), false),
        ];
        let manager = ContextManager::new(
            ContextBudget {
                context_window: 200_000,
                output_reserve: 32_000,
                tool_schema_tokens: 5_000,
            },
            ContextPolicy::default(),
        );
        let snapshot = manager.snapshot(&messages, &system_prompt);
        let context = context_breakdown(&snapshot, &system_prompt, &messages, 23);
        assert_eq!(context.instruction_files, vec!["NEURON.md".to_string()]);
        assert!(context.instruction_tokens > 0);
        assert!(context.tool_output_tokens > 0);
        assert_eq!(context.tool_count, 23);
        assert_eq!(
            context.system_tokens
                + context.tool_schema_tokens
                + context.instruction_tokens
                + context.conversation_tokens
                + context.tool_output_tokens,
            context.used_tokens
        );
        assert!(context.threshold_tokens > context.used_tokens);

        let report = context_report("test-model", &context);
        assert_eq!(report.command, "context");
        assert!(report.title.contains("200,000"), "{}", report.title);
        let table = &report.sections[1].markdown;
        assert!(table.contains("| Tools (23) |"), "{table}");
        assert!(
            table.contains("Project instructions & memory (NEURON.md)"),
            "{table}"
        );
        assert!(table.contains("| Free |"), "{table}");
    }
}
