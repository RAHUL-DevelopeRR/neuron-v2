//! A scripted engine for UI tests: each submitted prompt plays the next scripted turn's
//! events, then [`UiEvent::TurnFinished`]. Permission requests in a script wait for the
//! UI's [`PermissionReply`] and record it, so tests can drive the dialog. Other commands
//! get what an answer function returns; [`samples::answer`] answers each the way the real
//! engine does, with [`samples`]' events.
//!
//! The samples are also written, one JSON line each, to `fixtures/protocol-v2-events.jsonl`
//! and `fixtures/protocol-v2-commands.jsonl` in this crate: the TypeScript mirrors parse
//! them, and UI tests can play them.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::protocol::{EngineCommand, PermissionReply, UiEvent};

/// What the fake engine sends for a command other than `Submit`.
pub type AnswerFn = Box<dyn Fn(&EngineCommand) -> Vec<UiEvent> + Send>;

/// Handles to a running fake engine.
pub struct FakeEngine {
    pub commands: Sender<EngineCommand>,
    pub replies: Sender<PermissionReply>,
    pub events: Receiver<UiEvent>,
    handle: Option<JoinHandle<FakeEngineLog>>,
}

/// What the fake engine saw, for assertions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FakeEngineLog {
    pub commands: Vec<EngineCommand>,
    pub replies: Vec<PermissionReply>,
}

impl FakeEngine {
    /// Starts an engine that answers the n-th `Submit` with `turns[n]`, and nothing else.
    #[must_use]
    pub fn spawn(turns: Vec<Vec<UiEvent>>) -> Self {
        Self::spawn_answering(turns, Box::new(|_| Vec::new()))
    }

    /// [`Self::spawn`] that answers every other command with `answer(command)`, for
    /// example [`samples::answer`].
    #[must_use]
    pub fn spawn_answering(turns: Vec<Vec<UiEvent>>, answer: AnswerFn) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<EngineCommand>();
        let (reply_tx, reply_rx) = mpsc::channel::<PermissionReply>();
        let (event_tx, event_rx) = mpsc::channel::<UiEvent>();
        let handle = thread::spawn(move || run(&turns, &answer, &command_rx, &reply_rx, &event_tx));
        Self {
            commands: command_tx,
            replies: reply_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    /// Stops the engine and returns what it saw.
    #[must_use]
    pub fn finish(mut self) -> FakeEngineLog {
        let _ = self.commands.send(EngineCommand::Shutdown);
        self.handle
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    }
}

fn run(
    turns: &[Vec<UiEvent>],
    answer: &AnswerFn,
    commands: &Receiver<EngineCommand>,
    replies: &Receiver<PermissionReply>,
    events: &Sender<UiEvent>,
) -> FakeEngineLog {
    let mut log = FakeEngineLog::default();
    let mut next_turn = turns.iter();
    while let Ok(command) = commands.recv() {
        log.commands.push(command.clone());
        match command {
            EngineCommand::Shutdown => break,
            EngineCommand::Submit { .. } => {
                for event in next_turn.next().into_iter().flatten() {
                    let waits = matches!(event, UiEvent::PermissionRequested { .. });
                    if events.send(event.clone()).is_err() {
                        return log;
                    }
                    if waits {
                        match replies.recv() {
                            Ok(reply) => log.replies.push(reply),
                            Err(_) => return log,
                        }
                    }
                }
                if events.send(UiEvent::TurnFinished).is_err() {
                    return log;
                }
            }
            EngineCommand::Interrupt => {
                let _ = events.send(UiEvent::Notice {
                    message: "Interrupting".into(),
                });
            }
            EngineCommand::Slash { .. }
            | EngineCommand::SetModel { .. }
            | EngineCommand::ListModels
            | EngineCommand::Capabilities { .. }
            | EngineCommand::Answer { .. }
            | EngineCommand::ListSessions { .. }
            | EngineCommand::Resume { .. }
            | EngineCommand::TrustWorkspace
            | EngineCommand::ListExtensions
            | EngineCommand::TrustPreview
            | EngineCommand::RefreshAccount => {
                for event in answer(&command) {
                    if events.send(event).is_err() {
                        return log;
                    }
                }
            }
        }
    }
    log
}

/// Sample events and commands of protocol v2, as the real engine sends them, and an
/// answer function that plays them.
pub mod samples {
    use crate::protocol::{
        AccountDetails, ClientMessage, EngineCommand, ErrorDetail, GitStatus, HookPreview,
        McpServerPreview, ModelInfo, PermissionRulePreview, ReportLink, ReportSection,
        SessionRunStatus, SessionScope, SessionSummary, UiEvent, UsageAmounts,
        CAPABILITY_QUESTIONS, CAPABILITY_REPORTS,
    };

    /// The workspace the samples run in.
    pub const CWD: &str = "/home/dev/zero-x.live";

    /// What [`crate::fake_engine::FakeEngine`] answers when driven with this function:
    /// the events the real engine sends for each command other than `Submit`.
    #[must_use]
    pub fn answer(command: &EngineCommand) -> Vec<UiEvent> {
        match command {
            EngineCommand::ListSessions { scope } => vec![sessions(*scope)],
            EngineCommand::TrustPreview => vec![trust_preview()],
            EngineCommand::TrustWorkspace => vec![workspace_trusted(), status(true)],
            EngineCommand::RefreshAccount => vec![account()],
            EngineCommand::ListModels => vec![models()],
            EngineCommand::Slash { command, .. } => {
                let name = command.trim().trim_start_matches('/');
                let first = match name {
                    "usage" | "init" | "memory" | "doctor" => report(name),
                    "login" => account(),
                    _ => UiEvent::Notice {
                        message: format!("/{name} ran."),
                    },
                };
                vec![first, UiEvent::TurnFinished, status(false)]
            }
            EngineCommand::SetModel { .. } => vec![status(false)],
            EngineCommand::Submit { .. }
            | EngineCommand::Interrupt
            | EngineCommand::Shutdown
            | EngineCommand::Capabilities { .. }
            | EngineCommand::Answer { .. }
            | EngineCommand::Resume { .. }
            | EngineCommand::ListExtensions => Vec::new(),
        }
    }

    /// Every event protocol v2 adds or extends, one of each kind.
    #[must_use]
    pub fn v2_events() -> Vec<UiEvent> {
        vec![
            status(false),
            models(),
            account(),
            UiEvent::Account {
                signed_in: false,
                account: None,
            },
            report("usage"),
            report("init"),
            report("memory"),
            report("doctor"),
            quota_error(),
            UiEvent::Error {
                message: "Can't reach Zero-X right now.".into(),
                detail: Some(ErrorDetail {
                    code: "unreachable".into(),
                    status: None,
                    upgrade_url: None,
                    resets_at: None,
                    retry_after_seconds: None,
                }),
            },
            sessions(SessionScope::Project),
            sessions(SessionScope::All),
            trust_preview(),
            workspace_trusted(),
        ]
    }

    /// Every command protocol v2 adds or extends.
    #[must_use]
    pub fn v2_commands() -> Vec<ClientMessage> {
        [
            EngineCommand::ListSessions {
                scope: SessionScope::All,
            },
            EngineCommand::ListSessions {
                scope: SessionScope::Project,
            },
            EngineCommand::TrustPreview,
            EngineCommand::RefreshAccount,
            EngineCommand::Capabilities {
                features: vec![CAPABILITY_QUESTIONS.into(), CAPABILITY_REPORTS.into()],
                client: Some("terminal".into()),
            },
            EngineCommand::Slash {
                command: "usage".into(),
                args: String::new(),
            },
        ]
        .into_iter()
        .map(ClientMessage::Command)
        .collect()
    }

    #[must_use]
    pub fn status(trusted: bool) -> UiEvent {
        UiEvent::Status {
            model: "glm-5.2".into(),
            permission_mode: "workspace-write".into(),
            session_id: "session-1".into(),
            cwd: CWD.into(),
            workspace_trusted: trusted,
            plan_mode: false,
            context_percent: 12,
            git: Some(GitStatus {
                branch: Some("main".into()),
                changed: 2,
                staged: 0,
                unstaged: 2,
                untracked: 0,
                conflicted: 0,
            }),
            signed_in: true,
            sign_in_required: false,
            account: None,
            supports_images: Some(false),
            served_model: None,
        }
    }

    #[must_use]
    pub fn models() -> UiEvent {
        let model = |id: &str, name: &str, images: bool| ModelInfo {
            id: id.into(),
            display_name: Some(name.into()),
            available: true,
            unavailable_reason: None,
            context_window: Some(131_072),
            tiers: vec!["free".into(), "paid".into()],
            status: Some("available".into()),
            available_again_at: None,
            supports_images: Some(images),
        };
        UiEvent::Models {
            current: "glm-5.2".into(),
            models: vec![
                model("glm-5.2", "GLM 5.2", false),
                model("gemini-flash", "Gemini Flash", true),
            ],
        }
    }

    #[must_use]
    pub fn account() -> UiEvent {
        UiEvent::Account {
            signed_in: true,
            account: Some(AccountDetails {
                email: Some("dev@example.com".into()),
                name: Some("Dev Example".into()),
                plan: Some("pro".into()),
                plan_name: Some("Pro".into()),
                source: "stored login (credentials file)".into(),
                expires_at: Some(1_793_404_800),
                limits: Some(UsageAmounts {
                    requests: Some(2_000),
                    tokens: Some(10_000_000),
                }),
                usage_today: Some(UsageAmounts {
                    requests: Some(740),
                    tokens: Some(1_204_310),
                }),
                resets_at: Some("2026-10-24T00:00:00Z".into()),
                credit_balance_cents: Some(742),
                suspended: false,
                dashboard_url: "https://zero-x.live/dashboard".into(),
                pricing_url: "https://zero-x.live/pricing".into(),
                usage_updated_at: Some(1_792_800_000),
                usage_error: None,
            }),
        }
    }

    /// A report as `/usage`, `/init`, `/memory` or `/doctor` sends it.
    #[must_use]
    pub fn report(command: &str) -> UiEvent {
        let section = |heading: &str, markdown: &str| ReportSection {
            heading: Some(heading.into()),
            markdown: markdown.into(),
        };
        let (title, sections, links) = match command {
            "usage" => (
                "Usage and plan",
                vec![section(
                    "Today",
                    "| | Used | Limit |\n|---|---:|---:|\n| Requests | 740 | 2,000 |\n| Tokens | 1,204,310 | 10,000,000 |\n\nResets 2026-10-24 00:00 UTC.",
                )],
                vec![ReportLink {
                    label: "Plans and pricing".into(),
                    url: "https://zero-x.live/pricing".into(),
                }],
            ),
            "init" => (
                "Project instructions",
                vec![section(
                    "NEURON.md",
                    "Created `NEURON.md` with the detected build and test commands.",
                )],
                Vec::new(),
            ),
            "memory" => (
                "Instruction files",
                vec![section("Loaded", "1. `NEURON.md` (42 lines)")],
                Vec::new(),
            ),
            _ => (
                "Doctor",
                vec![section("Sign-in", "**ok**: signed in to Zero-X (Pro)")],
                Vec::new(),
            ),
        };
        UiEvent::Report {
            command: command.into(),
            title: title.into(),
            sections,
            links,
        }
    }

    /// The gateway's answer when the daily quota is used up.
    #[must_use]
    pub fn quota_error() -> UiEvent {
        UiEvent::Error {
            message: "Daily request limit reached for the Free plan (insufficient_quota); resets at 2026-10-24T00:00:00Z".into(),
            detail: Some(ErrorDetail {
                code: "insufficient_quota".into(),
                status: Some(429),
                upgrade_url: Some("https://zero-x.live/pricing".into()),
                resets_at: Some("2026-10-24T00:00:00Z".into()),
                retry_after_seconds: Some(3_600),
            }),
        }
    }

    #[must_use]
    pub fn sessions(scope: SessionScope) -> UiEvent {
        let session = |id: &str, title: &str, project: &str| SessionSummary {
            id: id.into(),
            title: Some(title.into()),
            model: Some("glm-5.2".into()),
            message_count: 12,
            updated_at_ms: 1_792_800_000_000,
            current: false,
            branch: None,
            project: Some(project.into()),
            git_branch: Some("main".into()),
            run_status: SessionRunStatus::Idle,
            failed: false,
            client: Some("desktop".into()),
        };
        let mut sessions = vec![
            SessionSummary {
                current: true,
                ..session("session-1", "Fix device sign-in", CWD)
            },
            SessionSummary {
                run_status: SessionRunStatus::Running,
                client: Some("terminal".into()),
                updated_at_ms: 1_792_799_000_000,
                ..session("session-2", "Billing webhook tests", CWD)
            },
        ];
        if scope == SessionScope::All {
            sessions.push(SessionSummary {
                failed: true,
                git_branch: Some("feat/desktop-shell".into()),
                updated_at_ms: 1_792_700_000_000,
                ..session("session-3", "Desktop sidebar", "/home/dev/neuron")
            });
        }
        UiEvent::Sessions {
            current: "session-1".into(),
            sessions,
            scope,
        }
    }

    #[must_use]
    pub fn trust_preview() -> UiEvent {
        let settings = format!("{CWD}/.neuron/settings.json");
        UiEvent::TrustPreview {
            cwd: CWD.into(),
            root: CWD.into(),
            is_git_root: true,
            session_only: false,
            trusted: false,
            settings_files: vec![settings.clone()],
            hooks: vec![
                HookPreview {
                    event: "PreToolUse".into(),
                    command: "./scripts/check-paths.sh".into(),
                    source: settings.clone(),
                },
                HookPreview {
                    event: "PostToolUse".into(),
                    command: "npm run lint --silent".into(),
                    source: settings.clone(),
                },
            ],
            mcp_servers: vec![McpServerPreview {
                name: "postgres-tools".into(),
                transport: "stdio".into(),
                target: Some("uvx".into()),
                source: settings.clone(),
            }],
            permission_rules: vec![
                PermissionRulePreview {
                    effect: "allow".into(),
                    rule: "bash(npm test)".into(),
                    source: settings.clone(),
                },
                PermissionRulePreview {
                    effect: "allow".into(),
                    rule: "bash(npm run lint)".into(),
                    source: settings.clone(),
                },
                PermissionRulePreview {
                    effect: "ask".into(),
                    rule: "write_file".into(),
                    source: settings,
                },
            ],
            other_settings: vec!["env".into()],
        }
    }

    #[must_use]
    pub fn workspace_trusted() -> UiEvent {
        UiEvent::WorkspaceTrusted {
            cwd: CWD.into(),
            restart_required: true,
            root: Some(CWD.into()),
            session_only: false,
        }
    }

    /// `lines` as the fixture files hold them: one JSON value per line.
    #[must_use]
    pub fn json_lines<T: serde::Serialize>(lines: &[T]) -> String {
        lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("protocol values serialize") + "\n")
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        ClientMessage, PermissionDecision, PermissionView, SessionScope, CAPABILITY_REPORTS,
    };
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    /// The fixture files the TypeScript mirrors read hold exactly the samples. Run with
    /// `NEURON_UPDATE_FIXTURES=1` after changing a sample to rewrite them.
    #[test]
    fn fixture_files_hold_the_samples() {
        let files = [
            (
                "protocol-v2-events.jsonl",
                samples::json_lines(&samples::v2_events()),
            ),
            (
                "protocol-v2-commands.jsonl",
                samples::json_lines(&samples::v2_commands()),
            ),
        ];
        for (name, expected) in files {
            let path = fixture(name);
            if std::env::var_os("NEURON_UPDATE_FIXTURES").is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &expected).unwrap();
            }
            let found = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            assert_eq!(
                found, expected,
                "{name} is stale: run `NEURON_UPDATE_FIXTURES=1 cargo test -p neuron-tui`"
            );
        }
        for line in samples::json_lines(&samples::v2_commands()).lines() {
            serde_json::from_str::<ClientMessage>(line).expect("a command line parses");
        }
    }

    /// Driven with the samples' answers, every protocol v2 command gets its events.
    #[test]
    fn answers_v2_commands_with_their_events() {
        let engine = FakeEngine::spawn_answering(Vec::new(), Box::new(samples::answer));
        let send = |command| engine.commands.send(command).unwrap();
        send(EngineCommand::Capabilities {
            features: vec![CAPABILITY_REPORTS.into()],
            client: Some("terminal".into()),
        });
        send(EngineCommand::ListSessions {
            scope: SessionScope::All,
        });
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::Sessions { scope: SessionScope::All, sessions, .. } if sessions.len() == 3
        ));
        send(EngineCommand::TrustPreview);
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::TrustPreview { trusted: false, .. }
        ));
        send(EngineCommand::RefreshAccount);
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::Account {
                signed_in: true,
                account: Some(_)
            }
        ));
        send(EngineCommand::Slash {
            command: "usage".into(),
            args: String::new(),
        });
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::Report { command, .. } if command == "usage"
        ));
        assert_eq!(engine.events.recv().unwrap(), UiEvent::TurnFinished);
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::Status { .. }
        ));
        let log = engine.finish();
        assert_eq!(log.commands.len(), 6, "five commands and shutdown");
    }

    #[test]
    fn plays_scripted_turns_and_waits_for_permission_replies() {
        let ask = UiEvent::PermissionRequested {
            id: 1,
            request: PermissionView {
                tool: "bash".into(),
                current_mode: "workspace-write".into(),
                required_mode: "danger-full-access".into(),
                reason: None,
                input: r#"{"command":"rm -rf target"}"#.into(),
                session_prefixes: Vec::new(),
            },
        };
        let engine = FakeEngine::spawn(vec![vec![
            UiEvent::TextDelta {
                text: "Cleaning".into(),
            },
            ask.clone(),
            UiEvent::ToolResult {
                id: "toolu_1".into(),
                name: "bash".into(),
                output: "denied".into(),
                is_error: true,
            },
        ]]);
        engine
            .commands
            .send(EngineCommand::Submit {
                text: "clean up".into(),
                attachments: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            engine.events.recv().unwrap(),
            UiEvent::TextDelta {
                text: "Cleaning".into()
            }
        );
        assert_eq!(engine.events.recv().unwrap(), ask);
        engine
            .replies
            .send(PermissionReply {
                id: 1,
                decision: PermissionDecision::Deny { note: None },
            })
            .unwrap();
        assert!(matches!(
            engine.events.recv().unwrap(),
            UiEvent::ToolResult { is_error: true, .. }
        ));
        assert_eq!(engine.events.recv().unwrap(), UiEvent::TurnFinished);
        let log = engine.finish();
        assert_eq!(log.replies.len(), 1);
        assert_eq!(log.commands.len(), 2, "submit and shutdown");
    }
}
