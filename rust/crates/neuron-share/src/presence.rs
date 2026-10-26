//! What a shared session tells the phone's sessions list (`RELAY_CONTRACT.md` §6), computed from
//! the session as its local host streams it (remote protocol 1: a `snapshot`, then `action`s).
//!
//! The apps compute presence from the full session reducer (`ui/neuron-client/src/session.ts`)
//! with `presenceOf` (`ui/neuron-mobile/src/relay/presence.ts`). This tracker keeps only what that
//! reads: pending permissions, open questions, the connection, whether a turn runs, the last
//! user/assistant/error item, the first prompt, the status line and the saved chats. The vectors
//! in `tests/fixtures/presence-vectors.json` come from the TypeScript reducer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A shared session's state on the phone's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareStatus {
    Running,
    NeedsMe,
    Idle,
    Failed,
}

/// Inside a presence blob (sealed with the computer's presence key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub v: u8,
    /// The workspace folder's name.
    pub project: String,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub status: ShareStatus,
    /// The remote protocol `seq` of the session's latest action (the phone compares it for "unread").
    pub last_seq: u64,
    pub updated_at_ms: u64,
}

impl Presence {
    /// Everything but the time: a new blob is sent only when this changes.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut value = serde_json::to_value(self).expect("presence serializes");
        if let Value::Object(fields) = &mut value {
            fields.remove("updated_at_ms");
        }
        value.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolState {
    Running,
    AwaitingPermission,
    Done,
    Error,
    Denied,
    CutOff,
}

impl ToolState {
    fn parse(text: &str) -> Self {
        match text {
            "running" => Self::Running,
            "awaiting_permission" => Self::AwaitingPermission,
            "done" => Self::Done,
            "denied" => Self::Denied,
            "cut_off" => Self::CutOff,
            _ => Self::Error,
        }
    }

    fn open(self) -> bool {
        matches!(
            self,
            Self::Running | Self::AwaitingPermission | Self::CutOff
        )
    }
}

#[derive(Debug, Clone)]
enum Item {
    User(String),
    Assistant,
    Error,
    Tool {
        key: String,
        id: String,
        name: String,
        input: String,
        state: ToolState,
    },
    Question {
        id: Option<i64>,
        answered: bool,
    },
    /// Reasoning, notices: nothing presence reads.
    Other,
}

#[derive(Debug, Clone)]
struct Permission {
    id: i64,
    tool_key: Option<String>,
}

/// One shared session as far as presence goes.
#[derive(Debug, Clone)]
pub struct PresenceTracker {
    fallback_cwd: String,
    seq: u64,
    exited: bool,
    busy: bool,
    session_starting: bool,
    cwd: Option<String>,
    model: Option<String>,
    branch: Option<String>,
    session_id: Option<String>,
    sessions: Vec<(String, Option<String>)>,
    items: Vec<Item>,
    permissions: Vec<Permission>,
    next_key: u64,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn same_json(a: &str, b: &str) -> bool {
    match (
        serde_json::from_str::<Value>(a),
        serde_json::from_str::<Value>(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

impl PresenceTracker {
    /// A session not heard from yet; `fallback_cwd` names the project until the engine does.
    #[must_use]
    pub fn new(fallback_cwd: &str) -> Self {
        Self {
            fallback_cwd: fallback_cwd.to_string(),
            seq: 0,
            exited: false,
            busy: false,
            session_starting: false,
            cwd: None,
            model: None,
            branch: None,
            session_id: None,
            sessions: Vec::new(),
            items: Vec::new(),
            permissions: Vec::new(),
            next_key: 1,
        }
    }

    /// The `seq` of the latest action seen.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The host's `snapshot {seq, state}`: replaces everything.
    pub fn snapshot(&mut self, seq: u64, state: &Value) {
        let fallback = std::mem::take(&mut self.fallback_cwd);
        *self = Self::new(&fallback);
        self.seq = seq;
        self.exited = state.get("connection").and_then(Value::as_str) == Some("exited");
        self.busy = flag(state, "busy");
        self.session_starting = flag(state, "sessionStarting");
        self.next_key = state.get("nextKey").and_then(Value::as_u64).unwrap_or(1);
        if let Some(status) = state.get("status").filter(|status| status.is_object()) {
            self.apply_status(status);
        }
        self.apply_sessions(state.get("sessions"));
        for item in state
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = text(item, "key").unwrap_or_default();
            self.items
                .push(match item.get("kind").and_then(Value::as_str) {
                    Some("user") => Item::User(text(item, "text").unwrap_or_default()),
                    Some("assistant") => Item::Assistant,
                    Some("error") => Item::Error,
                    Some("tool") => Item::Tool {
                        key,
                        id: text(item, "id").unwrap_or_default(),
                        name: text(item, "name").unwrap_or_default(),
                        input: text(item, "input").unwrap_or_default(),
                        state: ToolState::parse(&text(item, "state").unwrap_or_default()),
                    },
                    Some("question") => Item::Question {
                        id: item.get("id").and_then(Value::as_i64),
                        answered: flag(item, "answered"),
                    },
                    _ => Item::Other,
                });
        }
        for permission in state
            .get("permissions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = permission.get("id").and_then(Value::as_i64) {
                self.permissions.push(Permission {
                    id,
                    tool_key: text(permission, "toolKey"),
                });
            }
        }
    }

    fn apply_status(&mut self, status: &Value) {
        self.cwd = text(status, "cwd");
        self.model = text(status, "model");
        self.session_id = text(status, "session_id");
        self.branch = status.get("git").and_then(|git| text(git, "branch"));
    }

    fn apply_sessions(&mut self, sessions: Option<&Value>) {
        self.sessions = sessions
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|session| Some((text(session, "id")?, text(session, "title"))))
            .collect();
    }

    fn push(&mut self, item: Item) {
        self.next_key += 1;
        self.items.push(item);
    }

    fn new_key(&self) -> String {
        format!("i{}", self.next_key)
    }

    fn end_open_tools(&mut self) {
        for item in &mut self.items {
            if let Item::Tool { state, .. } = item {
                if matches!(state, ToolState::Running | ToolState::AwaitingPermission) {
                    *state = ToolState::Error;
                }
            }
        }
    }

    fn last_unanswered_question(&mut self) -> Option<&mut bool> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Question { answered, .. } if !*answered => Some(answered),
            _ => None,
        })
    }

    /// The host's `action {seq, action}`.
    #[allow(clippy::too_many_lines)] // one arm per action, as in the reducer
    pub fn action(&mut self, seq: u64, action: &Value) {
        self.seq = seq;
        let Some(kind) = action.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "local/connecting" | "ready" => self.exited = false,
            "local/reset" => {
                let next_key = self.next_key;
                let fallback = std::mem::take(&mut self.fallback_cwd);
                *self = Self::new(&fallback);
                self.seq = seq;
                self.next_key = next_key;
            }
            "local/disconnected" => {
                self.end_open_tools();
                self.exited = true;
                self.busy = false;
                self.permissions.clear();
            }
            "local/submitted" => {
                let answered_question = match self.last_unanswered_question() {
                    Some(answered) => {
                        *answered = true;
                        true
                    }
                    None => false,
                };
                self.push(Item::User(text(action, "text").unwrap_or_default()));
                if !answered_question {
                    self.busy = true;
                }
            }
            "local/slash" => {
                let command = text(action, "command").unwrap_or_default();
                let args = text(action, "args").unwrap_or_default();
                let line = if args.is_empty() {
                    format!("/{command}")
                } else {
                    format!("/{command} {args}")
                };
                self.push(Item::User(line));
                self.busy = true;
            }
            "local/permission_answered" | "local/permission_resolved" => {
                let Some(id) = action.get("id").and_then(Value::as_i64) else {
                    return;
                };
                let Some(index) = self.permissions.iter().position(|p| p.id == id) else {
                    return;
                };
                let permission = self.permissions.remove(index);
                let denied = kind == "local/permission_answered"
                    && action
                        .get("decision")
                        .and_then(|decision| decision.get("kind"))
                        .and_then(Value::as_str)
                        == Some("deny");
                if let Some(tool_key) = permission.tool_key {
                    for item in &mut self.items {
                        if let Item::Tool { key, state, .. } = item {
                            if *key == tool_key && *state == ToolState::AwaitingPermission {
                                *state = if denied {
                                    ToolState::Denied
                                } else {
                                    ToolState::Running
                                };
                            }
                        }
                    }
                }
            }
            "local/answered" => {
                let id = action.get("id").and_then(Value::as_i64);
                let found = self.items.iter_mut().rev().find_map(|item| match item {
                    Item::Question {
                        id: question,
                        answered,
                    } if *question == id && id.is_some() => Some(answered),
                    _ => None,
                });
                if let Some(answered) = found {
                    *answered = true;
                    self.push(Item::User(text(action, "text").unwrap_or_default()));
                }
            }
            "status" => self.apply_status(action),
            "session_starting" => self.session_starting = true,
            "sign_in_code" => self.busy = true,
            "text_delta" => {
                if !matches!(self.items.last(), Some(Item::Assistant)) {
                    self.push(Item::Assistant);
                }
            }
            "reasoning_started" | "reasoning_delta" => {
                if !matches!(self.items.last(), Some(Item::Other)) {
                    self.push(Item::Other);
                }
            }
            "tool_call_started" => self.tool_started(
                text(action, "id").unwrap_or_default(),
                text(action, "name").unwrap_or_default(),
                text(action, "input").unwrap_or_default(),
                flag(action, "cut_off"),
            ),
            "tool_result" => self.tool_result(action),
            "permission_requested" => self.permission_requested(action),
            "notice" => {
                let message = text(action, "message").unwrap_or_default();
                self.push(if message.starts_with("Question:") {
                    Item::Question {
                        id: None,
                        answered: false,
                    }
                } else {
                    Item::Other
                });
            }
            "question" => self.push(Item::Question {
                id: action.get("id").and_then(Value::as_i64),
                answered: false,
            }),
            "turn_finished" => {
                self.end_open_tools();
                self.busy = false;
                self.session_starting = false;
                self.permissions.clear();
            }
            "error" => self.push(Item::Error),
            "sessions" => self.apply_sessions(action.get("sessions")),
            "transcript" => self.restore(action),
            _ => {}
        }
    }

    fn tool_started(&mut self, id: String, name: String, input: String, cut_off: bool) {
        let key = self.new_key();
        self.push(Item::Tool {
            key,
            id,
            name,
            input,
            state: if cut_off {
                ToolState::CutOff
            } else {
                ToolState::Running
            },
        });
    }

    fn tool_result(&mut self, action: &Value) {
        let id = text(action, "id").filter(|id| !id.is_empty());
        let name = text(action, "name").unwrap_or_default();
        let find = |items: &[Item]| {
            items.iter().position(|item| match (item, &id) {
                (Item::Tool { id: tool, .. }, Some(id)) => tool == id,
                (
                    Item::Tool {
                        name: tool, state, ..
                    },
                    None,
                ) => *tool == name && state.open(),
                _ => false,
            })
        };
        let index = if let Some(index) = find(&self.items) {
            index
        } else {
            // A result for a call this client never saw (it attached late).
            self.tool_started(
                id.clone().unwrap_or_default(),
                name.clone(),
                "{}".into(),
                false,
            );
            self.items.len() - 1
        };
        let is_error = flag(action, "is_error");
        let output = text(action, "output").unwrap_or_default();
        if let Item::Tool { key, state, .. } = &mut self.items[index] {
            let denied = *state == ToolState::Denied
                || (is_error
                    && (output.contains("denied by user")
                        || output.contains("denied by the user")));
            *state = if denied {
                ToolState::Denied
            } else if is_error {
                if *state == ToolState::CutOff {
                    ToolState::CutOff
                } else {
                    ToolState::Error
                }
            } else {
                ToolState::Done
            };
            let key = key.clone();
            self.permissions
                .retain(|permission| permission.tool_key.as_deref() != Some(key.as_str()));
        }
    }

    fn permission_requested(&mut self, action: &Value) {
        let Some(id) = action.get("id").and_then(Value::as_i64) else {
            return;
        };
        if self
            .permissions
            .iter()
            .any(|permission| permission.id == id)
        {
            return;
        }
        let request = action.get("request").cloned().unwrap_or(Value::Null);
        let tool = text(&request, "tool").unwrap_or_default();
        let input = text(&request, "input").unwrap_or_default();
        let mut tool_key = None;
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                key,
                name,
                input: called,
                state,
                ..
            } = item
            {
                if *state == ToolState::Running
                    && *name == tool
                    && (*called == input || same_json(called, &input))
                {
                    *state = ToolState::AwaitingPermission;
                    tool_key = Some(key.clone());
                    break;
                }
            }
        }
        self.permissions.push(Permission { id, tool_key });
    }

    fn restore(&mut self, action: &Value) {
        self.items.clear();
        self.permissions.clear();
        if let Some(session_id) = text(action, "session_id") {
            if self.cwd.is_some() || self.model.is_some() || self.session_id.is_some() {
                self.session_id = Some(session_id);
            }
        }
        if flag(action, "truncated") {
            self.push(Item::Other);
        }
        for entry in action
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match entry.get("kind").and_then(Value::as_str) {
                Some("user") => self.push(Item::User(text(entry, "text").unwrap_or_default())),
                Some("assistant") => self.push(Item::Assistant),
                Some("tool_call") => {
                    let key = self.new_key();
                    self.push(Item::Tool {
                        key,
                        id: text(entry, "id").unwrap_or_default(),
                        name: text(entry, "name").unwrap_or_default(),
                        input: text(entry, "input").unwrap_or_default(),
                        state: ToolState::Error,
                    });
                }
                Some("tool_result") => {
                    let id = text(entry, "id").unwrap_or_default();
                    let is_error = flag(entry, "is_error");
                    let output = text(entry, "output").unwrap_or_default();
                    if let Some(Item::Tool { state, .. }) = self
                        .items
                        .iter_mut()
                        .rev()
                        .find(|item| matches!(item, Item::Tool { id: tool, .. } if *tool == id))
                    {
                        let denied = is_error
                            && (output.contains("denied by user")
                                || output.contains("denied by the user"));
                        *state = if denied {
                            ToolState::Denied
                        } else if is_error {
                            ToolState::Error
                        } else {
                            ToolState::Done
                        };
                    }
                }
                _ => {}
            }
        }
    }

    /// `presence.ts` `shareStatus`.
    #[must_use]
    pub fn status(&self) -> ShareStatus {
        let open_question = self.items.iter().any(|item| {
            matches!(
                item,
                Item::Question {
                    answered: false,
                    ..
                }
            )
        });
        if !self.permissions.is_empty() || open_question {
            return ShareStatus::NeedsMe;
        }
        if self.exited {
            return ShareStatus::Failed;
        }
        if self.busy || self.session_starting {
            return ShareStatus::Running;
        }
        let last_turn = self
            .items
            .iter()
            .rev()
            .find(|item| matches!(item, Item::User(_) | Item::Assistant | Item::Error));
        if matches!(last_turn, Some(Item::Error)) {
            ShareStatus::Failed
        } else {
            ShareStatus::Idle
        }
    }

    /// `presence.ts` `presenceOf`.
    #[must_use]
    pub fn presence(&self, now_ms: u64) -> Presence {
        let cwd = self.cwd.as_deref().unwrap_or(&self.fallback_cwd);
        let current = self
            .session_id
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|(session, _)| session == id));
        let first_prompt = self.items.iter().find_map(|item| match item {
            Item::User(text) => Some(text),
            _ => None,
        });
        let title = match current {
            Some((_, Some(title))) => Some(title.clone()),
            _ => first_prompt.map(|text| {
                text.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(80)
                    .collect::<String>()
            }),
        };
        Presence {
            v: 1,
            project: basename(cwd),
            title: title.filter(|title| !title.is_empty()),
            branch: self.branch.clone(),
            model: self.model.clone(),
            status: self.status(),
            last_seq: self.seq,
            updated_at_ms: now_ms,
        }
    }
}
