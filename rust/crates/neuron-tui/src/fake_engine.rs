//! A scripted engine for UI tests: each submitted prompt plays the next scripted turn's
//! events, then [`UiEvent::TurnFinished`]. Permission requests in a script wait for the
//! UI's [`PermissionReply`] and record it, so tests can drive the dialog.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::protocol::{EngineCommand, PermissionReply, UiEvent};

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
    /// Starts an engine that answers the n-th `Submit` with `turns[n]`.
    #[must_use]
    pub fn spawn(turns: Vec<Vec<UiEvent>>) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<EngineCommand>();
        let (reply_tx, reply_rx) = mpsc::channel::<PermissionReply>();
        let (event_tx, event_rx) = mpsc::channel::<UiEvent>();
        let handle = thread::spawn(move || run(&turns, &command_rx, &reply_rx, &event_tx));
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
                    kind: None,
                });
            }
            EngineCommand::Slash { .. }
            | EngineCommand::SetModel { .. }
            | EngineCommand::ListModels
            | EngineCommand::Capabilities { .. }
            | EngineCommand::Commit { .. }
            | EngineCommand::Rewind { .. } => {}
        }
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PermissionDecision, PermissionView};

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
