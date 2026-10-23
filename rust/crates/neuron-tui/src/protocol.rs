//! Messages between the full-screen UI and the agent engine.
//!
//! The engine runs a turn on its own thread and reports everything the user should see as
//! [`UiEvent`]s; the UI sends [`EngineCommand`]s and answers permission requests with
//! [`PermissionReply`]. Everything is plain data (serde-serialisable): `neuron engine
//! --stdio` speaks the same contract as JSON lines, [`ClientMessage`]s in and [`UiEvent`]s
//! out, starting with [`UiEvent::Ready`].

use serde::{Deserialize, Serialize};

/// The contract's version, sent in [`UiEvent::Ready`]. Bumped when a change would break a
/// UI written against the previous version (new event kinds do not count: a UI ignores
/// events it does not know).
pub const PROTOCOL_VERSION: u32 = 1;

/// Token counts as the provider reported them (or estimated before it did).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What a tool call needs approval for, as the permission dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionView {
    pub tool: String,
    pub current_mode: String,
    pub required_mode: String,
    pub reason: Option<String>,
    /// The tool input as JSON text (commands, paths, diffs are inside it).
    pub input: String,
    /// What "allow for this session" would approve from now on (command prefixes such as
    /// `cargo test`, or a web domain). Empty when only a one-time approval is possible.
    pub session_prefixes: Vec<String>,
}

/// The workspace's git state, for the footer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// The branch, `detached HEAD`, or `None` when git could not tell.
    pub branch: Option<String>,
    /// Changed paths in all: staged, unstaged, untracked and conflicted ones.
    pub changed: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

/// One model the user can switch to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    /// Whether the user's plan can use it now; `unavailable_reason` says why not.
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub context_window: Option<u32>,
    /// Plan tiers that include it (`free`, `pro`, ...).
    pub tiers: Vec<String>,
    /// The gateway's view now: `available`, `degraded`, `busy` or `blurred`
    /// (not usable until `available_again_at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// When a `blurred` model is back (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_again_at: Option<String>,
}

/// The Zero-X login, as the UI may show it. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Known right after a sign-in in this engine; not stored with the login.
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Where the login comes from: `NEURON_TOKEN`, or the stored login and where it is kept.
    pub source: String,
    /// When the session expires (Unix seconds), when the gateway said.
    pub expires_at: Option<u64>,
}

/// Engine → UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// The first event: the engine is ready for commands.
    Ready {
        /// [`PROTOCOL_VERSION`] of the engine.
        protocol: u32,
        /// The Neuron version.
        version: String,
    },
    /// What the footer shows. Sent after `Ready`, after every `TurnFinished`, and after a
    /// model switch.
    Status {
        model: String,
        permission_mode: String,
        session_id: String,
        cwd: String,
        /// Untrusted workspaces run without their own settings, hooks and MCP servers.
        workspace_trusted: bool,
        plan_mode: bool,
        /// Estimated share of the model's input budget the conversation uses.
        context_percent: u32,
        /// `None` outside a git repository.
        git: Option<GitStatus>,
        /// A usable Zero-X login: `NEURON_TOKEN`, or a stored session that has not expired.
        /// Other providers do not need one, so this alone says nothing about the session.
        #[serde(default)]
        signed_in: bool,
        /// The session cannot run until the user signs in to Zero-X (`/login`): the engine
        /// started signed out, or the session is on Zero-X and has no usable login.
        #[serde(default)]
        sign_in_required: bool,
        /// The login, also when its session expired; `None` when there is none.
        #[serde(default)]
        account: Option<AccountInfo>,
    },
    /// After a sign-in from the signed-out start: the engine is opening the session
    /// (plugins, MCP servers), which can take a while. `TurnFinished` and a `Status` with
    /// the session follow, or an `Error`.
    SessionStarting,
    /// `/login` started a device sign-in: open `url` (or `url_complete`, which has the code
    /// filled in) and enter `user_code`. The engine waits for the approval; an interrupt
    /// cancels it. A notice and a `Status` follow when it is done, then `TurnFinished`.
    SignInCode {
        url: String,
        url_complete: Option<String>,
        user_code: String,
        expires_in_secs: u64,
        /// The engine opened `url_complete` (or `url`) in the browser.
        browser_opened: bool,
    },
    /// Answer to [`EngineCommand::ListModels`]. Empty when the provider has no catalog
    /// (only the Zero-X gateway publishes one).
    Models {
        current: String,
        models: Vec<ModelInfo>,
    },
    /// A streamed piece of assistant text (Markdown, not yet rendered).
    TextDelta { text: String },
    /// The current text block ended.
    TextBlockEnd,
    /// A reasoning block started. `chars` is known for complete blocks; `redacted` when the
    /// provider hides the reasoning.
    ReasoningStarted {
        chars: Option<usize>,
        redacted: bool,
    },
    /// Reasoning text, when the provider streams it.
    ReasoningDelta { text: String },
    /// A tool call with its complete input. `cut_off`: the output limit truncated it and it
    /// will not run.
    ToolCallStarted {
        id: String,
        name: String,
        input: String,
        cut_off: bool,
    },
    /// A tool call's result as the model sees it, for every call: one that ran (`is_error`
    /// when it failed), one denied or blocked by a hook, and one that never started
    /// (interrupted, cut off). `id` is the [`UiEvent::ToolCallStarted`] id.
    ToolResult {
        #[serde(default)]
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    /// Token usage: this turn so far, the session total, and whether it is an estimate.
    Usage {
        turn: TokenCounts,
        cumulative: TokenCounts,
        estimated: bool,
    },
    /// A tool call waits for approval; answer with a [`PermissionReply`] carrying `id`.
    PermissionRequested { id: u64, request: PermissionView },
    /// A one-line notice (retries, warnings, interrupts).
    Notice { message: String },
    /// The turn is over (finished, cancelled or failed).
    TurnFinished,
    /// The turn failed; `message` is what to show.
    Error { message: String },
}

/// A file, folder or image the user attached to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    File { path: String },
    Folder { path: String },
    Image { path: String, media_type: String },
}

/// UI → engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineCommand {
    /// Run a turn with this prompt.
    Submit {
        text: String,
        attachments: Vec<Attachment>,
    },
    /// Run a slash command (`/model`, `/compact`, ...), without the leading slash.
    Slash { command: String, args: String },
    /// Switch the model for the next turn.
    SetModel { model: String },
    /// Ask for the models the user can switch to ([`UiEvent::Models`]).
    ListModels,
    /// Interrupt the running turn (a second one quits after saving, like Ctrl-C twice).
    /// Sent right after a `Submit` the engine has not started yet, it cancels that
    /// prompt's turn; with nothing running or waiting, it is dropped.
    Interrupt,
    /// Save and stop.
    Shutdown,
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Run this call.
    AllowOnce,
    /// Run this call and similar ones for the rest of the session.
    AllowForSession,
    /// Do not run it; `note` is passed to the model.
    Deny { note: Option<String> },
}

/// Answer to [`UiEvent::PermissionRequested`] with the same `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionReply {
    pub id: u64,
    pub decision: PermissionDecision,
}

/// One input line of `neuron engine --stdio`: a command, or an answer to a permission
/// request. `{"kind":"command","type":"submit","text":"hi","attachments":[]}`,
/// `{"kind":"permission_reply","id":1,"decision":{"kind":"allow_once"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    Command(EngineCommand),
    PermissionReply(PermissionReply),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_commands_round_trip_as_tagged_json() {
        let event = UiEvent::ToolCallStarted {
            id: "toolu_1".into(),
            name: "bash".into(),
            input: r#"{"command":"cargo test"}"#.into(),
            cut_off: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_started");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), event);

        let command = EngineCommand::Submit {
            text: "fix the test".into(),
            attachments: vec![Attachment::Image {
                path: "shot.png".into(),
                media_type: "image/png".into(),
            }],
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["type"], "submit");
        assert_eq!(json["attachments"][0]["kind"], "image");
        assert_eq!(
            serde_json::from_value::<EngineCommand>(json).unwrap(),
            command
        );

        let reply = PermissionReply {
            id: 7,
            decision: PermissionDecision::Deny {
                note: Some("use a dry run".into()),
            },
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["decision"]["kind"], "deny");
        assert_eq!(
            serde_json::from_value::<PermissionReply>(json).unwrap(),
            reply
        );
    }

    /// Fields added after protocol 1 shipped read as their defaults from older engines.
    #[test]
    fn later_fields_default_when_absent() {
        let result: UiEvent = serde_json::from_str(
            r#"{"type":"tool_result","name":"bash","output":"ok","is_error":false}"#,
        )
        .unwrap();
        assert_eq!(
            result,
            UiEvent::ToolResult {
                id: String::new(),
                name: "bash".into(),
                output: "ok".into(),
                is_error: false,
            }
        );
        let status: UiEvent = serde_json::from_str(
            r#"{"type":"status","model":"m","permission_mode":"read-only","session_id":"s","cwd":".","workspace_trusted":false,"plan_mode":false,"context_percent":0,"git":null}"#,
        )
        .unwrap();
        assert!(matches!(
            status,
            UiEvent::Status {
                signed_in: false,
                sign_in_required: false,
                account: None,
                ..
            }
        ));
    }

    /// The stdio transport's input lines: a top-level `kind`, then the command's own
    /// `type` (or the reply's fields) beside it.
    #[test]
    fn client_messages_are_flat_tagged_json_lines() {
        let line = r#"{"kind":"command","type":"submit","text":"hi","attachments":[]}"#;
        assert_eq!(
            serde_json::from_str::<ClientMessage>(line).unwrap(),
            ClientMessage::Command(EngineCommand::Submit {
                text: "hi".into(),
                attachments: Vec::new(),
            })
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"kind":"command","type":"interrupt"}"#)
                .unwrap(),
            ClientMessage::Command(EngineCommand::Interrupt)
        );
        let reply = r#"{"kind":"permission_reply","id":3,"decision":{"kind":"allow_once"}}"#;
        assert_eq!(
            serde_json::from_str::<ClientMessage>(reply).unwrap(),
            ClientMessage::PermissionReply(PermissionReply {
                id: 3,
                decision: PermissionDecision::AllowOnce,
            })
        );
        let message = ClientMessage::Command(EngineCommand::Slash {
            command: "model".into(),
            args: "opus".into(),
        });
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["kind"], "command");
        assert_eq!(json["type"], "slash");
        assert_eq!(
            serde_json::from_value::<ClientMessage>(json).unwrap(),
            message
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"submit"}"#).is_err());
    }
}
