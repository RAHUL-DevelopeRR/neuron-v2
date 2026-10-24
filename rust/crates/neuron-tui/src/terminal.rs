//! Owning the terminal: raw mode, the alternate screen and bracketed paste, always given
//! back — when the guard drops, when anything panics, and before a forced exit (which
//! runs no destructors, so it calls [`restore`] itself).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};

/// A guard holds the terminal.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Holds the terminal for the full-screen UI until dropped.
#[derive(Debug)]
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Switches to raw mode and the alternate screen. A panic from here on restores the
    /// terminal before the panic message prints.
    ///
    /// # Errors
    ///
    /// When the terminal cannot be switched; it is left as it was.
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste) {
            restore();
            return Err(error);
        }
        Ok(Self { _private: () })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Gives the terminal back if a [`TerminalGuard`] holds it; does nothing otherwise. Safe to
/// call from any thread, any number of times.
pub fn restore() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

/// Whether the full-screen UI holds the terminal.
#[must_use]
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_without_a_guard_leaves_the_terminal_alone() {
        assert!(!is_active());
        restore();
        restore();
        assert!(!is_active());
    }
}
