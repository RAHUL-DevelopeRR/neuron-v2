// The request/answer shape is ported from Freebuff, common/src/desktop-cloud/
// hosted-workspace.ts (`quit.onRequest`, `quit.answer`), commit 0dc95cb
// (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache License
// 2.0. Changed for Neuron: a Tauri event and command instead of Electron IPC,
// with the counts of working and waiting chats. See ui/neuron-desktop/NOTICE.

//! Closing the window while chats work: the app asks once before it quits.
//!
//! When the window is closed and a chat is working or waiting for the user, the
//! close is held and the window is sent a numbered request
//! (`neuron://quit-request {id, working, waiting}`). It answers with
//! `quit_answer(id, quit)`: Quit closes the window (and the registry's shutdown
//! runs as before), Keep working leaves everything as it was. Asking is done
//! once: closing again while the question is open quits, so a window that
//! cannot answer never keeps the app open. The request/answer pair follows
//! Freebuff's desktop-cloud `hosted-workspace.ts` (`quit.onRequest`,
//! `quit.answer`); see ui/neuron-desktop/NOTICE.

use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuitRequest {
    pub id: u64,
    /// Chats working on a turn.
    pub working: usize,
    /// Chats waiting for the user's answer or approval.
    pub waiting: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseDecision {
    /// Close as today.
    Close,
    /// Hold the close and ask the window.
    Ask(QuitRequest),
}

#[derive(Default)]
struct GateState {
    last: u64,
    pending: Option<u64>,
    confirmed: bool,
}

#[derive(Default)]
pub struct QuitGate(Mutex<GateState>);

impl QuitGate {
    /// What a close request does, given how many chats work and wait now.
    pub fn on_close(&self, working: usize, waiting: usize) -> CloseDecision {
        let mut state = crate::lock(&self.0);
        if state.confirmed || working + waiting == 0 || state.pending.is_some() {
            state.pending = None;
            return CloseDecision::Close;
        }
        state.last += 1;
        state.pending = Some(state.last);
        CloseDecision::Ask(QuitRequest {
            id: state.last,
            working,
            waiting,
        })
    }

    /// The window's answer to request `id`. True when the window should close
    /// now; an answer to any other request (an old one) changes nothing.
    pub fn answer(&self, id: u64, quit: bool) -> bool {
        let mut state = crate::lock(&self.0);
        if state.pending != Some(id) {
            return false;
        }
        state.pending = None;
        state.confirmed = quit;
        quit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_working_closes_at_once() {
        let gate = QuitGate::default();
        assert_eq!(gate.on_close(0, 0), CloseDecision::Close);
    }

    #[test]
    fn working_chats_ask_and_quit_closes() {
        let gate = QuitGate::default();
        let CloseDecision::Ask(request) = gate.on_close(2, 1) else {
            panic!("should ask");
        };
        assert_eq!((request.working, request.waiting), (2, 1));
        assert!(gate.answer(request.id, true));
        // The close that follows the answer goes through.
        assert_eq!(gate.on_close(2, 1), CloseDecision::Close);
    }

    #[test]
    fn keep_working_holds_and_the_next_close_asks_again() {
        let gate = QuitGate::default();
        let CloseDecision::Ask(first) = gate.on_close(1, 0) else {
            panic!("should ask");
        };
        assert!(!gate.answer(first.id, false));
        let CloseDecision::Ask(second) = gate.on_close(1, 0) else {
            panic!("should ask again");
        };
        assert_ne!(first.id, second.id);
        // An answer to the old question is ignored.
        assert!(!gate.answer(first.id, true));
        assert!(gate.answer(second.id, true));
    }

    #[test]
    fn closing_again_while_asked_quits() {
        let gate = QuitGate::default();
        let CloseDecision::Ask(request) = gate.on_close(0, 1) else {
            panic!("should ask");
        };
        assert_eq!(gate.on_close(0, 1), CloseDecision::Close);
        // The late answer finds no question.
        assert!(!gate.answer(request.id, true));
    }

    #[test]
    fn the_request_reads_as_the_window_expects() {
        let json = serde_json::to_value(QuitRequest {
            id: 3,
            working: 2,
            waiting: 0,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": 3, "working": 2, "waiting": 0})
        );
    }
}
