//! Process-wide hooks for user-facing output that does not belong to a turn's stream:
//! warnings and notices, and questions the agent asks the user.
//!
//! The line terminal keeps the defaults (stderr for notices, stdin for answers). A
//! full-screen UI installs handlers before it takes over the terminal, so nothing writes
//! over its screen; [`clear_handlers`] restores the defaults when it leaves.

use std::io;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

type NoticeHandler = Arc<dyn Fn(&str) + Send + Sync>;
type AskHandler = Arc<dyn Fn(&UserQuestion) -> io::Result<String> + Send + Sync>;

static NOTICE: RwLock<Option<NoticeHandler>> = RwLock::new(None);
static ASK: RwLock<Option<AskHandler>> = RwLock::new(None);

/// A question for the user (the `AskUserQuestion` tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserQuestion {
    pub question: String,
    /// When present, the answer is one of these (the UI may return its 1-based index).
    pub options: Option<Vec<String>>,
}

/// Routes [`notice`] to `handler` instead of stderr.
pub fn set_notice_handler(handler: impl Fn(&str) + Send + Sync + 'static) {
    *NOTICE.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(handler));
}

/// Routes questions to `handler` instead of the terminal prompt. The handler returns the
/// raw answer, exactly as a user would have typed it.
pub fn set_ask_handler(
    handler: impl Fn(&UserQuestion) -> io::Result<String> + Send + Sync + 'static,
) {
    *ASK.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(handler));
}

/// Restores the line-terminal defaults.
pub fn clear_handlers() {
    *NOTICE.write().unwrap_or_else(PoisonError::into_inner) = None;
    *ASK.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether a UI has taken over notices (callers that draw their own terminal effects,
/// such as an in-place countdown, fall back to plain notices then).
#[must_use]
pub fn notice_handler_installed() -> bool {
    NOTICE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// Shows a one-line notice to the user: the installed handler, else stderr.
#[allow(clippy::print_stderr)]
pub fn notice(message: &str) {
    let handler = NOTICE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match handler {
        Some(handler) => handler(message),
        None => eprintln!("{message}"),
    }
}

/// Whether stdin is a terminal, found out once per process. Code that may run while
/// another thread reads stdin asks this rather than `io::stdin().is_terminal()`: on
/// Windows a question about a pipe waits for a read pending on it, so it would hang
/// until the pipe sends something. Asked first before such a read starts.
#[must_use]
pub fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal as _;
    static STDIN_IS_TERMINAL: OnceLock<bool> = OnceLock::new();
    *STDIN_IS_TERMINAL.get_or_init(|| io::stdin().is_terminal())
}

/// Where the line terminal shows a question it reads the answer to from stdin (a tool
/// approval, `AskUserQuestion`, the workspace trust question): stdout, or stderr when
/// stdout is redirected (`neuron prompt ... > out.txt`), so the user still sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStream {
    Stdout,
    Stderr,
}

impl PromptStream {
    /// Stdout when it is a terminal, else stderr when that is one. When neither is:
    /// stdout for a script that feeds stdin (stdin is not a terminal either), and `None`
    /// for a person at the terminal, who would answer a question they cannot see.
    #[must_use]
    pub fn choose(
        stdin_is_terminal: bool,
        stdout_is_terminal: bool,
        stderr_is_terminal: bool,
    ) -> Option<Self> {
        if stdout_is_terminal {
            Some(Self::Stdout)
        } else if stderr_is_terminal {
            Some(Self::Stderr)
        } else if stdin_is_terminal {
            None
        } else {
            Some(Self::Stdout)
        }
    }

    /// [`Self::choose`] for this process's stdin, stdout and stderr.
    #[must_use]
    pub fn current() -> Option<Self> {
        use std::io::IsTerminal as _;
        Self::choose(
            stdin_is_terminal(),
            io::stdout().is_terminal(),
            io::stderr().is_terminal(),
        )
    }

    /// The stream to write the question to.
    #[must_use]
    pub fn writer(self) -> Box<dyn io::Write> {
        match self {
            Self::Stdout => Box::new(io::stdout()),
            Self::Stderr => Box::new(io::stderr()),
        }
    }
}

/// The installed question handler, if a UI provides one.
#[must_use]
pub fn ask_handler() -> Option<impl Fn(&UserQuestion) -> io::Result<String>> {
    ASK.read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .map(|handler| move |question: &UserQuestion| handler(question))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The hooks are process-wide; these tests take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn notices_reach_the_installed_handler_until_cleared() {
        let _guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        set_notice_handler(move |message| sink.lock().unwrap().push(message.to_string()));
        assert!(notice_handler_installed());
        notice("ui-hooks-test: retrying in 3 s");
        clear_handlers();
        assert!(!notice_handler_installed());
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|message| message == "ui-hooks-test: retrying in 3 s"));
    }

    /// `neuron prompt ... > out.txt` from a terminal: the question goes to stderr, which
    /// the user sees; with stderr redirected too, nobody would see it.
    #[test]
    fn a_question_goes_where_the_user_sees_it() {
        use PromptStream::{Stderr, Stdout};
        // (stdin, stdout, stderr) are terminals:
        assert_eq!(PromptStream::choose(true, true, true), Some(Stdout));
        assert_eq!(PromptStream::choose(true, true, false), Some(Stdout));
        assert_eq!(PromptStream::choose(true, false, true), Some(Stderr));
        assert_eq!(PromptStream::choose(true, false, false), None);
        // A script feeds the answers: as before, on stdout.
        assert_eq!(PromptStream::choose(false, false, false), Some(Stdout));
        assert_eq!(PromptStream::choose(false, false, true), Some(Stderr));
    }

    #[test]
    fn questions_go_to_the_installed_handler() {
        let _guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(ask_handler().is_none());
        set_ask_handler(|question| Ok(format!("answer to {}", question.question)));
        let handler = ask_handler().expect("installed");
        let answer = handler(&UserQuestion {
            question: "which branch?".into(),
            options: None,
        })
        .unwrap();
        clear_handlers();
        assert_eq!(answer, "answer to which branch?");
        assert!(ask_handler().is_none());
    }
}
