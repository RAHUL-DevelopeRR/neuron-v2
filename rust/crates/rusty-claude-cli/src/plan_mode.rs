//! `/plan` mode: read-only research-and-plan turns.
//!
//! `/plan on` switches the session to read-only and remembers the permission
//! mode that was active; `/plan off` restores exactly that mode (it never
//! escalates to danger-full-access on its own).

use std::sync::{Mutex, PoisonError};

use runtime::PermissionMode;

/// Remembers the permission mode that was active before `/plan on`.
#[derive(Debug, Default)]
pub(crate) struct PlanModeMemory {
    previous_mode: Option<PermissionMode>,
}

impl PlanModeMemory {
    pub(crate) const fn new() -> Self {
        Self {
            previous_mode: None,
        }
    }

    /// Records `current` and returns the mode used while planning.
    pub(crate) fn enter(&mut self, current: PermissionMode) -> PermissionMode {
        self.previous_mode = Some(current);
        PermissionMode::ReadOnly
    }

    /// The mode to restore when planning ends: the one recorded by
    /// [`Self::enter`], or `current` when nothing was recorded.
    pub(crate) fn exit(&mut self, current: PermissionMode) -> PermissionMode {
        self.previous_mode.take().unwrap_or(current)
    }
}

/// Process-wide memory used by the interactive CLI (one session per process).
static PLAN_MODE_MEMORY: Mutex<PlanModeMemory> = Mutex::new(PlanModeMemory::new());

/// Enters plan mode from `current` and returns the mode to use while planning.
pub(crate) fn enter_plan_mode(current: PermissionMode) -> PermissionMode {
    PLAN_MODE_MEMORY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .enter(current)
}

/// Leaves plan mode and returns the permission mode to restore.
pub(crate) fn exit_plan_mode(current: PermissionMode) -> PermissionMode {
    PLAN_MODE_MEMORY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .exit(current)
}

/// Wraps a user prompt with the plan-mode instructions.
pub(crate) fn wrap_plan_prompt(input: &str) -> String {
    format!(
        "[PLAN MODE \u{2014} RESEARCH FIRST, THEN PLAN]\n\
         You are in plan mode. Follow this two-phase workflow:\n\n\
         PHASE 1 \u{2014} RESEARCH:\n\
         Use read_file, glob_search, grep_search to explore the codebase.\n\
         Understand existing patterns, file structure, and dependencies.\n\
         Read NEURON.md, AGENTS.md or README.md if they exist.\n\
         Do NOT skip this \u{2014} your plan quality depends on real context.\n\n\
         PHASE 2 \u{2014} STRUCTURED PLAN:\n\
         After researching, produce a detailed implementation plan:\n\n\
         ## Goal\n\
         What we are building and why (2-3 sentences).\n\n\
         ## Current State\n\
         What exists now \u{2014} relevant files, patterns, tech stack found.\n\n\
         ## Architecture\n\
         For EACH file (new or modified):\n\
         ### `filename`\n\
         - Purpose: what this file does\n\
         - Key functions/components to create or modify\n\
         - How it connects to other files (data flow)\n\n\
         ## Implementation Steps\n\
         Numbered, detailed steps. Each step should explain:\n\
         - What to do\n\
         - Why this approach (design rationale)\n\
         - Edge cases to handle\n\n\
         ## Dependencies\n\
         Packages needed with versions.\n\n\
         ## Risks & Edge Cases\n\
         What could go wrong, browser compat, error handling.\n\n\
         ## Verification\n\
         Specific test steps to validate the implementation.\n\n\
         RULES:\n\
         - Do NOT generate full file contents\n\
         - Code snippets max 3-5 lines (illustrative only)\n\
         - Be detailed in WHAT and WHY, not in raw code\n\n\
         User's request: {input}\n"
    )
}

/// What the restored permission mode allows, in words for the model.
fn mode_guidance(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => {
            "Only read-only tools are available; describe the changes instead of making them."
        }
        PermissionMode::WorkspaceWrite => {
            "You can edit files in the workspace. Commands and other actions that need more access ask the user for approval."
        }
        PermissionMode::Prompt => "Each tool call asks the user for approval.",
        PermissionMode::DangerFullAccess | PermissionMode::Allow => {
            "Tools run without approval prompts."
        }
    }
}

/// Wraps the first prompt after `/plan off`, telling the model which
/// permission mode is active again.
pub(crate) fn wrap_post_plan_prompt(input: &str, mode: PermissionMode) -> String {
    format!(
        "[PLAN MODE ENDED]\n\
         Plan mode is off and the permission mode is back to {}. {}\n\
         Implement the plan with the available tools.\n\n\
         User's request: {input}\n",
        mode.as_str(),
        mode_guidance(mode)
    )
}

#[cfg(test)]
mod tests {
    use super::{wrap_plan_prompt, wrap_post_plan_prompt, PlanModeMemory};
    use runtime::PermissionMode;

    #[test]
    fn plan_off_restores_the_mode_active_before_plan_on() {
        for before in [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::Prompt,
            PermissionMode::DangerFullAccess,
        ] {
            let mut memory = PlanModeMemory::new();
            assert_eq!(memory.enter(before), PermissionMode::ReadOnly);
            assert_eq!(memory.exit(PermissionMode::ReadOnly), before);
        }
    }

    #[test]
    fn plan_off_without_plan_on_keeps_the_current_mode() {
        let mut memory = PlanModeMemory::new();
        assert_eq!(
            memory.exit(PermissionMode::ReadOnly),
            PermissionMode::ReadOnly
        );
        memory.enter(PermissionMode::WorkspaceWrite);
        assert_eq!(
            memory.exit(PermissionMode::ReadOnly),
            PermissionMode::WorkspaceWrite
        );
        // The remembered mode is used once.
        assert_eq!(
            memory.exit(PermissionMode::ReadOnly),
            PermissionMode::ReadOnly
        );
    }

    #[test]
    fn process_wide_plan_toggle_round_trips() {
        let planning = super::enter_plan_mode(PermissionMode::WorkspaceWrite);
        assert_eq!(planning, PermissionMode::ReadOnly);
        assert_eq!(
            super::exit_plan_mode(planning),
            PermissionMode::WorkspaceWrite
        );
    }

    #[test]
    fn post_plan_prompt_names_the_restored_mode_and_never_waives_approval() {
        let prompt = wrap_post_plan_prompt("execute the plan", PermissionMode::WorkspaceWrite);
        assert!(prompt.contains("back to workspace-write"));
        assert!(prompt.contains("ask the user for approval"));
        assert!(prompt.ends_with("User's request: execute the plan\n"));
        let lowered = prompt.to_lowercase();
        assert!(!lowered.contains("do not ask for permission"));
        assert!(!lowered.contains("full access restored"));
    }

    #[test]
    fn plan_prompts_contain_no_mojibake() {
        for prompt in [
            wrap_plan_prompt("x"),
            wrap_post_plan_prompt("x", PermissionMode::ReadOnly),
        ] {
            for marker in ["\u{e2}\u{20ac}", "\u{c3}", "\u{c2}", "\u{f0}\u{178}"] {
                assert!(!prompt.contains(marker), "{prompt}");
            }
        }
        assert!(wrap_plan_prompt("x").contains("PLAN MODE \u{2014} RESEARCH FIRST"));
    }
}
