//! The reports the engine's UIs show in a panel ([`UiEvent::Report`]): `/usage`, `/init`,
//! `/memory` and `/doctor`, each a title, Markdown sections and links. A UI without the
//! `reports` capability gets the same report as one plain-text notice.

use std::path::Path;

use neuron_tui::protocol::{ReportLink, ReportSection, UiEvent};

use crate::account::{group_digits, AccountSummary};
use crate::doctor::DoctorReport;
use crate::init::InitReport;

/// A report, before it becomes an event or a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// The command's name without the slash.
    pub(crate) command: &'static str,
    pub(crate) title: String,
    pub(crate) sections: Vec<ReportSection>,
    pub(crate) links: Vec<ReportLink>,
}

impl Report {
    pub(crate) fn into_event(self) -> UiEvent {
        UiEvent::Report {
            command: self.command.to_string(),
            title: self.title,
            sections: self.sections,
            links: self.links,
        }
    }

    /// The report as one notice, for a UI that does not show report panels.
    pub(crate) fn render_text(&self) -> String {
        let mut text = self.title.clone();
        for section in &self.sections {
            text.push_str("\n\n");
            if let Some(heading) = &section.heading {
                text.push_str(heading);
                text.push('\n');
            }
            text.push_str(&section.markdown);
        }
        for link in &self.links {
            text.push_str(&format!("\n{}: {}", link.label, link.url));
        }
        text
    }
}

fn section(heading: &str, markdown: impl Into<String>) -> ReportSection {
    ReportSection {
        heading: Some(heading.to_string()),
        markdown: markdown.into(),
    }
}

fn link(label: &str, url: impl Into<String>) -> ReportLink {
    ReportLink {
        label: label.to_string(),
        url: url.into(),
    }
}

/// `text` as inline Markdown code.
fn code(text: &str) -> String {
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

/// A count, grouped by thousands; "-" when unknown.
fn amount(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), group_digits)
}

/// When `stamp` (ISO 8601) is, in UTC and from now.
fn describe_time(stamp: &str, now: u64) -> String {
    crate::clock::parse_timestamp(stamp).map_or_else(
        || stamp.to_string(),
        |unix| {
            format!(
                "{} ({})",
                crate::clock::format_utc(unix),
                crate::clock::describe_relative(unix, now)
            )
        },
    )
}

/// `/usage`: today's requests and tokens against the plan, when they reset, the credit,
/// and links to upgrade and to the dashboard.
pub(crate) fn usage(
    account: &AccountSummary,
    now: u64,
    pricing_url: &str,
    dashboard_url: &str,
) -> Report {
    let mut today = format!(
        "| | Used | Limit |\n|---|---:|---:|\n| Requests | {} | {} |\n| Tokens | {} | {} |",
        amount(account.requests_today),
        amount(account.daily_requests_limit),
        amount(account.tokens_today),
        amount(account.daily_tokens_limit),
    );
    if let Some(resets_at) = &account.resets_at {
        today.push_str(&format!("\n\nResets {}.", describe_time(resets_at, now)));
    }
    let mut plan = Vec::new();
    match (&account.plan_name, &account.plan_id) {
        (Some(name), _) | (None, Some(name)) => plan.push(format!("**{name}**")),
        (None, None) => {}
    }
    if account.suspended {
        plan.push("Suspended: contact support@zero-x.live.".to_string());
    }
    if let Some(email) = &account.email {
        plan.push(format!("Signed in as {email}."));
    }
    if let Some(balance) = account.credit_balance_usd {
        let grant = account
            .monthly_grant_usd
            .filter(|grant| *grant > 0.0)
            .map(|grant| format!(" (monthly grant ${grant:.2})"))
            .unwrap_or_default();
        plan.push(format!("Credit: ${balance:.2}{grant}."));
    }
    if let Some(status) = &account.subscription_status {
        let renewal = account
            .current_period_end
            .as_deref()
            .map_or_else(String::new, |end| {
                let verb = if account.cancel_at_period_end {
                    "ends"
                } else {
                    "renews"
                };
                format!(", {verb} {}", describe_time(end, now))
            });
        plan.push(format!("Subscription: {status}{renewal}."));
    }
    let mut sections = vec![section("Today", today)];
    if !plan.is_empty() {
        sections.push(section("Plan", plan.join("\n\n")));
    }
    Report {
        command: "usage",
        title: "Usage and plan".to_string(),
        sections,
        links: vec![
            link("Plans and pricing", pricing_url),
            link("Usage dashboard", dashboard_url),
        ],
    }
}

/// `/init`: the files it created or left alone.
pub(crate) fn init(report: &InitReport) -> Report {
    let files = report
        .artifacts
        .iter()
        .map(|artifact| format!("- {} {}", code(artifact.name), artifact.status.label()))
        .collect::<Vec<_>>()
        .join("\n");
    Report {
        command: "init",
        title: "Project instructions".to_string(),
        sections: vec![
            section(
                "Files",
                format!(
                    "In {}:\n\n{files}",
                    code(&report.project_root.display().to_string())
                ),
            ),
            section(
                "Next",
                "Review `NEURON.md` and tailor it: Neuron reads it at the start of every session.",
            ),
        ],
        links: Vec::new(),
    }
}

/// `/memory`: the instruction files this session loads, with their size and first line.
pub(crate) fn memory(cwd: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    let context = runtime::ProjectContext::discover(cwd, crate::DEFAULT_DATE)?;
    let loaded = if context.instruction_files.is_empty() {
        "None yet. `/init` creates a starter `NEURON.md`.".to_string()
    } else {
        context
            .instruction_files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let shown = file.path.strip_prefix(cwd).map_or_else(
                    |_| file.path.display().to_string(),
                    |path| path.display().to_string(),
                );
                let lines = file.content.lines().count();
                let first = file
                    .content
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|line| format!(": {}", line.chars().take(80).collect::<String>()))
                    .unwrap_or_default();
                format!(
                    "{}. {} ({lines} line{}){first}",
                    index + 1,
                    code(&shown),
                    if lines == 1 { "" } else { "s" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Report {
        command: "memory",
        title: "Instruction files".to_string(),
        sections: vec![
            section("Loaded", loaded),
            section(
                "Where Neuron looks",
                "`NEURON.md`, `NEURON.local.md`, `.neuron/NEURON.md` and `AGENTS.md` in this folder and every folder above it, so one in your home folder applies to every project under it.",
            ),
        ],
        links: Vec::new(),
    })
}

/// `/doctor`: each check with its result, summary and details.
pub(crate) fn doctor(report: &DoctorReport) -> Report {
    let (ok, warn, fail) = report.counts();
    let mut sections = vec![section(
        "Summary",
        format!("{ok} ok, {warn} warning{}, {fail} failed", plural(warn)),
    )];
    sections.extend(report.checks().iter().map(|check| {
        let mut markdown = format!("**{}**: {}", check.level.label(), check.summary);
        for detail in &check.details {
            markdown.push_str(&format!("\n- {detail}"));
        }
        section(check.name, markdown)
    }));
    Report {
        command: "doctor",
        title: "Doctor".to_string(),
        sections,
        links: Vec::new(),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{DiagnosticCheck, DiagnosticLevel};
    use crate::init::{InitArtifact, InitStatus};

    fn account() -> AccountSummary {
        crate::account::parse_account(&serde_json::json!({
            "user": {"email": "dev@example.com", "name": "Dev"},
            "plan": {"id": "pro", "name": "Pro"},
            "limits": {"daily_requests": 2000, "daily_tokens": 10_000_000},
            "usage_today": {"requests": 740, "tokens": 1_204_310, "resets_at": "2026-10-24T00:00:00Z"},
            "credits": {"balance_usd": 7.42, "monthly_grant_usd": 8},
            "subscription": {"status": "active", "current_period_end": "2026-11-01T00:00:00Z", "cancel_at_period_end": false},
            "flags": {"suspended": false}
        }))
    }

    #[test]
    fn usage_shows_today_against_the_plan_with_links() {
        let report = usage(
            &account(),
            1_792_800_000,
            "https://zero-x.live/pricing",
            "https://zero-x.live/dashboard",
        );
        assert_eq!(report.command, "usage");
        let today = &report.sections[0].markdown;
        assert!(today.contains("| Requests | 740 | 2,000 |"), "{today}");
        assert!(
            today.contains("| Tokens | 1,204,310 | 10,000,000 |"),
            "{today}"
        );
        assert!(today.contains("Resets 2026-10-24 00:00 UTC"), "{today}");
        let plan = &report.sections[1].markdown;
        assert!(plan.contains("**Pro**"));
        assert!(plan.contains("Credit: $7.42 (monthly grant $8.00)."));
        assert!(plan.contains("Subscription: active, renews 2026-11-01 00:00 UTC"));
        assert_eq!(report.links[0].url, "https://zero-x.live/pricing");
        assert_eq!(report.links[1].label, "Usage dashboard");

        let text = report.render_text();
        assert!(text.starts_with("Usage and plan\n\nToday\n|"), "{text}");
        assert!(text.ends_with("Usage dashboard: https://zero-x.live/dashboard"));
        match report.into_event() {
            UiEvent::Report { command, title, .. } => {
                assert_eq!(
                    (command.as_str(), title.as_str()),
                    ("usage", "Usage and plan")
                );
            }
            other => panic!("a report event, got {other:?}"),
        }
    }

    #[test]
    fn init_lists_the_files_it_touched() {
        let report = init(&InitReport {
            project_root: "/repo".into(),
            artifacts: vec![
                InitArtifact {
                    name: "NEURON.md",
                    status: InitStatus::Created,
                },
                InitArtifact {
                    name: ".neuron.json",
                    status: InitStatus::Skipped,
                },
            ],
        });
        let files = &report.sections[0].markdown;
        assert!(files.contains("- `NEURON.md` created"), "{files}");
        assert!(
            files.contains("- `.neuron.json` skipped (already exists)"),
            "{files}"
        );
    }

    #[test]
    fn memory_lists_the_instruction_files_this_folder_loads() {
        let root = std::env::temp_dir().join(format!(
            "neuron-memory-report-{}-{}",
            std::process::id(),
            crate::clock::now_unix()
        ));
        std::fs::create_dir_all(&root).expect("workspace");
        let empty = memory(&root).expect("a report");
        assert!(empty.sections[0].markdown.contains("/init"));
        std::fs::write(root.join("NEURON.md"), "\n# Rules\nRun the tests.\n").expect("write");
        let report = memory(&root).expect("a report");
        let loaded = &report.sections[0].markdown;
        assert!(
            loaded.contains("1. `NEURON.md` (3 lines): # Rules"),
            "{loaded}"
        );
        for brand in ["Claude", "Anthropic"] {
            assert!(!report.render_text().contains(brand));
        }
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn doctor_has_a_section_per_check() {
        let mut warn = DiagnosticCheck::new("Workspace", DiagnosticLevel::Warn, "untrusted");
        warn.details
            .push("Trust it to load its settings".to_string());
        let report = doctor(&DoctorReport::from_checks(vec![
            DiagnosticCheck::new("Auth", DiagnosticLevel::Ok, "signed in"),
            warn,
        ]));
        assert_eq!(report.sections[0].markdown, "1 ok, 1 warning, 0 failed");
        assert_eq!(report.sections[1].heading.as_deref(), Some("Auth"));
        assert_eq!(report.sections[1].markdown, "**ok**: signed in");
        assert_eq!(
            report.sections[2].markdown,
            "**warn**: untrusted\n- Trust it to load its settings"
        );
    }
}
