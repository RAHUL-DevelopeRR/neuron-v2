//! Shared retry policy pieces: `Retry-After` parsing, the total backoff
//! budget and the progress notice printed before each retry.

use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on the total time a single request may spend sleeping
/// between retries, across every attempt. A `Retry-After` that does not fit
/// in the remaining budget ends the retry loop instead of being shortened.
pub const MAX_TOTAL_BACKOFF: Duration = Duration::from_secs(60);

/// One retry that is about to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryNotice {
    /// Provider display name (e.g. `Zero-X`, `OpenAI`).
    pub provider: String,
    /// 1-based number of the retry about to be sent.
    pub retry: u32,
    /// Maximum number of retries the policy allows.
    pub max_retries: u32,
    /// How long the client will wait before retrying.
    pub delay: Duration,
    /// Short reason, e.g. `capacity_exhausted` or `503 Service Unavailable`.
    pub reason: String,
}

impl RetryNotice {
    /// One-line progress message for stderr.
    #[must_use]
    pub fn render(&self) -> String {
        let delay = self.delay.as_secs_f64();
        let delay = if delay >= 10.0 || delay.fract() == 0.0 {
            format!("{delay:.0}s")
        } else {
            format!("{delay:.1}s")
        };
        format!(
            "{} {}; retrying in {delay} (retry {} of {})",
            self.provider, self.reason, self.retry, self.max_retries
        )
    }
}

/// Callback invoked before each retry sleep.
#[derive(Clone)]
pub struct RetryObserver(Arc<dyn Fn(&RetryNotice) + Send + Sync>);

impl RetryObserver {
    pub fn new(observer: impl Fn(&RetryNotice) + Send + Sync + 'static) -> Self {
        Self(Arc::new(observer))
    }

    /// Prints each notice as one line on stderr.
    #[must_use]
    pub fn stderr() -> Self {
        Self::new(|notice| eprintln!("{}", notice.render()))
    }

    pub(crate) fn notify(&self, notice: &RetryNotice) {
        (self.0)(notice);
    }
}

impl Debug for RetryObserver {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetryObserver(..)")
    }
}

/// Parses a `Retry-After` header given in (possibly fractional) seconds.
/// HTTP-date values are ignored so the caller falls back to its backoff.
/// A delay too large for [`Duration`] (or infinite) saturates to
/// [`Duration::MAX`], which never fits the retry budget and so ends the
/// retry loop instead of panicking.
pub(crate) fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds = value.trim().parse::<f64>().ok()?;
    // `>=` is false for NaN, so only non-negative numbers get through.
    (seconds >= 0.0).then(|| Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX))
}

/// Chooses the next sleep: the server's `Retry-After` when present, otherwise
/// the client's backoff. Returns `None` when the delay does not fit in what
/// is left of `budget` after `spent`, which ends the retry loop.
pub(crate) fn plan_retry_delay(
    retry_after: Option<Duration>,
    backoff: Duration,
    spent: Duration,
    budget: Duration,
) -> Option<Duration> {
    let remaining = budget.saturating_sub(spent);
    match retry_after {
        Some(requested) => (requested <= remaining).then_some(requested),
        None if remaining.is_zero() => None,
        None => Some(backoff.min(remaining)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{plan_retry_delay, retry_after_from_headers, RetryNotice, MAX_TOTAL_BACKOFF};

    #[test]
    fn retry_after_header_accepts_integer_and_fractional_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(
            retry_after_from_headers(&headers),
            Some(Duration::from_secs(7))
        );
        headers.insert(reqwest::header::RETRY_AFTER, "0.5".parse().unwrap());
        assert_eq!(
            retry_after_from_headers(&headers),
            Some(Duration::from_millis(500))
        );
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_from_headers(&headers), None);
    }

    #[test]
    fn huge_or_odd_retry_after_values_never_panic() {
        let mut headers = reqwest::header::HeaderMap::new();
        for huge in ["99999999999999999999", "1e300", "inf"] {
            headers.insert(reqwest::header::RETRY_AFTER, huge.parse().unwrap());
            assert_eq!(
                retry_after_from_headers(&headers),
                Some(Duration::MAX),
                "{huge}"
            );
        }
        for rejected in ["-5", "NaN", "-inf"] {
            headers.insert(reqwest::header::RETRY_AFTER, rejected.parse().unwrap());
            assert_eq!(retry_after_from_headers(&headers), None, "{rejected}");
        }
        // A saturated delay never fits the budget, so the retry loop ends.
        assert_eq!(
            plan_retry_delay(
                Some(Duration::MAX),
                Duration::from_secs(1),
                Duration::ZERO,
                MAX_TOTAL_BACKOFF
            ),
            None
        );
    }

    #[test]
    fn retry_after_is_honoured_only_when_it_fits_the_total_budget() {
        let backoff = Duration::from_secs(2);
        assert_eq!(
            plan_retry_delay(
                Some(Duration::from_secs(5)),
                backoff,
                Duration::ZERO,
                MAX_TOTAL_BACKOFF
            ),
            Some(Duration::from_secs(5))
        );
        // A server asking for longer than the remaining budget ends the loop
        // instead of being retried early.
        assert_eq!(
            plan_retry_delay(
                Some(Duration::from_secs(120)),
                backoff,
                Duration::ZERO,
                MAX_TOTAL_BACKOFF
            ),
            None
        );
        assert_eq!(
            plan_retry_delay(
                Some(Duration::from_secs(10)),
                backoff,
                Duration::from_secs(55),
                MAX_TOTAL_BACKOFF
            ),
            None
        );
    }

    #[test]
    fn backoff_is_clamped_to_the_remaining_budget() {
        assert_eq!(
            plan_retry_delay(
                None,
                Duration::from_secs(32),
                Duration::from_secs(40),
                MAX_TOTAL_BACKOFF
            ),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            plan_retry_delay(
                None,
                Duration::from_secs(1),
                MAX_TOTAL_BACKOFF,
                MAX_TOTAL_BACKOFF
            ),
            None
        );
    }

    #[test]
    fn notice_renders_a_single_progress_line() {
        let notice = RetryNotice {
            provider: "Zero-X".to_string(),
            retry: 1,
            max_retries: 2,
            delay: Duration::from_secs(5),
            reason: "capacity_exhausted".to_string(),
        };
        assert_eq!(
            notice.render(),
            "Zero-X capacity_exhausted; retrying in 5s (retry 1 of 2)"
        );
        assert!(!notice.render().contains('\n'));
    }
}
