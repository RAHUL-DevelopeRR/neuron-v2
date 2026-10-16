//! Cooperative cancellation shared by the conversation loop, model streams and
//! tool child processes.
//!
//! A [`CancellationToken`] is cheap to clone; every clone observes the same
//! state. The CLI creates one token per turn, wires Ctrl-C to
//! [`CancellationToken::cancel`], and hands clones to the API stream and the
//! tool executor so both can stop promptly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How often [`CancellationToken::cancelled`] re-checks the flag.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Shared, clonable cancellation flag.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Returns `true` once [`cancel`](Self::cancel) has been called on any clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled. Suitable for `tokio::select!`.
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    #[test]
    fn clones_share_cancellation_state() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn cancelled_future_resolves_after_cancel() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let token = CancellationToken::new();
        let waiter = token.clone();
        runtime.block_on(async move {
            let handle = tokio::spawn(async move { waiter.cancelled().await });
            token.cancel();
            handle.await.expect("join");
        });
    }
}
