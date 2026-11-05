//! `/hooks`, and the lifecycle hooks the REPL and the engine run around sessions and
//! compaction.

// Output goes through the caller (a notice or stdout) or runtime::ui_hooks.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::fmt::Write as _;

use runtime::{HookEvent, RuntimeHookConfig};

use crate::hook_events::command_preview;
use crate::LiveCli;

/// The `/hooks` report: every configured hook by event, with its matcher and where it
/// was declared. `filter` (an event name, any case) lists that event only.
pub(crate) fn render_hooks_report(
    hooks: &RuntimeHookConfig,
    workspace_trusted: bool,
    filter: Option<&str>,
) -> String {
    let filter = filter.map(str::trim).filter(|filter| !filter.is_empty());
    let mut report = String::from("Hooks\n");
    let _ = writeln!(
        report,
        "  Workspace        {}",
        if workspace_trusted {
            "trusted (its settings' hooks load)"
        } else {
            "not trusted (its settings' hooks are not loaded)"
        }
    );
    let events: Vec<HookEvent> = HookEvent::ALL
        .into_iter()
        .filter(|event| filter.is_none_or(|name| event.as_str().eq_ignore_ascii_case(name)))
        .collect();
    if events.is_empty() {
        let names: Vec<&str> = HookEvent::ALL.iter().map(|event| event.as_str()).collect();
        let _ = write!(
            report,
            "\n  Unknown hook event `{}`. Events: {}.",
            filter.unwrap_or_default(),
            names.join(", ")
        );
        return report;
    }
    let mut listed = 0;
    for event in events {
        let commands = hooks.commands(event);
        if commands.is_empty() {
            continue;
        }
        let _ = write!(report, "\n  {}", event.as_str());
        let target = event.matcher_target();
        if !target.is_empty() {
            let _ = write!(report, " (matcher: {target})");
        }
        report.push('\n');
        for (index, hook) in commands.iter().enumerate() {
            listed += 1;
            let _ = writeln!(
                report,
                "    {}. [{}] {}  {}",
                index + 1,
                hook.source.as_str(),
                hook.matcher.as_deref().unwrap_or("*"),
                command_preview(&hook.command)
            );
        }
    }
    if listed == 0 {
        report.push_str(match filter {
            Some(_) => "\n  No hooks for this event.\n",
            None => "\n  No hooks configured.\n",
        });
    }
    report.push_str(
        "\n  Hooks are set under \"hooks\" in settings.json (user or workspace) or by plugins; see docs/hooks.md.",
    );
    report
}

/// Whether this workspace's own settings (and so its hooks) load.
pub(crate) fn workspace_trusted() -> bool {
    std::env::current_dir()
        .map(|cwd| runtime::ConfigLoader::default_for(cwd).workspace_is_trusted())
        .unwrap_or(false)
}

impl LiveCli {
    /// `/hooks [event]`.
    pub(crate) fn hooks_report(&self, args: Option<&str>) -> String {
        render_hooks_report(self.runtime.hook_config(), workspace_trusted(), args)
    }

    /// Runs the `SessionStart` hooks (`startup`, `resume`, `clear` or `compact`). What
    /// they print goes to the model with the next prompt; failures were already shown.
    pub(crate) fn run_session_start_hooks(&mut self, source: &str) {
        let _ = self.runtime.run_session_start_hooks(source);
    }

    /// Runs the `PreCompact` hooks before `/compact`.
    pub(crate) fn run_manual_pre_compact_hooks(&mut self) {
        let _ = self.runtime.run_pre_compact_hooks("manual");
    }
}

#[cfg(test)]
mod tests {
    use super::render_hooks_report;
    use runtime::{HookCommand, HookEvent, HookSource, RuntimeHookConfig};

    #[test]
    fn lists_hooks_by_event_with_matcher_and_source() {
        let hooks = RuntimeHookConfig::default()
            .with(
                HookEvent::PreToolUse,
                HookCommand::new("guard.sh --token abc", HookSource::Project)
                    .with_matcher(Some("bash|write_file".into())),
            )
            .with(
                HookEvent::Stop,
                HookCommand::new("check.sh", HookSource::Plugin),
            );

        let report = render_hooks_report(&hooks, true, None);

        assert!(
            report.contains("trusted (its settings' hooks load)"),
            "{report}"
        );
        assert!(
            report.contains("PreToolUse (matcher: tool name)"),
            "{report}"
        );
        assert!(
            report.contains("1. [project] bash|write_file  guard.sh --token ***"),
            "{report}"
        );
        assert!(
            report.contains("  Stop\n    1. [plugin] *  check.sh"),
            "{report}"
        );
        assert!(!report.contains("SessionStart"), "{report}");
        assert!(!report.contains("abc"), "{report}");
    }

    #[test]
    fn says_when_nothing_is_configured_or_the_workspace_is_untrusted() {
        let report = render_hooks_report(&RuntimeHookConfig::default(), false, None);
        assert!(report.contains("not trusted"), "{report}");
        assert!(report.contains("No hooks configured."), "{report}");
    }

    #[test]
    fn filters_by_event_name() {
        let hooks = RuntimeHookConfig::default()
            .with(HookEvent::Stop, HookCommand::new("a", HookSource::User))
            .with(
                HookEvent::PreToolUse,
                HookCommand::new("b", HookSource::User),
            );
        let report = render_hooks_report(&hooks, true, Some("stop"));
        assert!(report.contains("  Stop\n"), "{report}");
        assert!(!report.contains("PreToolUse"), "{report}");
        assert!(render_hooks_report(&hooks, true, Some("PreCompact"))
            .contains("No hooks for this event."));
        assert!(render_hooks_report(&hooks, true, Some("Nope")).contains("Unknown hook event"));
    }
}
