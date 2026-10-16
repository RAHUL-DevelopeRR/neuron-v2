//! Shared retry policy pieces: `Retry-After` parsing, the total backoff
//! budget and the progress notice printed before each retry.

use std::fmt::{Debug, Formatter};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on the total time a single request may spend sleeping
/// between retries, across every attempt. A `Retry-After` that does not fit
/// in the remaining budget ends the retry loop instead of being shortened.
pub const MAX_TOTAL_BACKOFF: Duration = Duration::from_secs(60);

/// Longest single `Retry-After` honoured when the Zero-X gateway reports
/// `capacity_exhausted` or `rate_limited`. Its shared free pools ask for
/// waits of 30 to 60 seconds, which the default budget cannot cover twice.
pub const CAPACITY_MAX_WAIT: Duration = Duration::from_secs(90);

/// Total sleep allowed across the retries of one request while the Zero-X
/// gateway reports `capacity_exhausted` or `rate_limited`.
pub const CAPACITY_TOTAL_BACKOFF: Duration = Duration::from_secs(180);

/// How long one request may wait between retries for a class of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryBudget {
    /// Total sleep across all retries of the request.
    pub total: Duration,
    /// Longest single wait; a longer `Retry-After` ends the retry loop.
    pub max_wait: Option<Duration>,
}

impl RetryBudget {
    /// The default budget: [`MAX_TOTAL_BACKOFF`] in total, no per-wait cap.
    pub(crate) const fn standard(total: Duration) -> Self {
        Self {
            total,
            max_wait: None,
        }
    }

    /// Next sleep under this budget (see [`plan_retry_delay`]), or `None`
    /// when it does not fit.
    pub(crate) fn plan(
        self,
        retry_after: Option<Duration>,
        backoff: Duration,
        spent: Duration,
    ) -> Option<Duration> {
        plan_retry_delay(retry_after, backoff, spent, self.total)
            .filter(|delay| self.max_wait.is_none_or(|max| *delay <= max))
    }
}

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
        self.render_remaining(self.delay)
    }

    /// The progress line with `remaining` left before the retry is sent.
    #[must_use]
    pub fn render_remaining(&self, remaining: Duration) -> String {
        let seconds = remaining.as_secs_f64();
        let wait = if remaining.is_zero() {
            "retrying now".to_string()
        } else if seconds >= 10.0 || seconds.fract() == 0.0 {
            format!("retrying in {seconds:.0}s")
        } else {
            format!("retrying in {seconds:.1}s")
        };
        format!(
            "{} {}; {wait} (retry {} of {})",
            self.provider, self.reason, self.retry, self.max_retries
        )
    }
}

type NoticeCallback = Arc<dyn Fn(&RetryNotice) + Send + Sync>;
type CountdownCallback = Arc<dyn Fn(&RetryNotice, Duration) + Send + Sync>;

/// Callbacks invoked before each retry sleep and, optionally, once per
/// second while it lasts (a countdown).
#[derive(Clone)]
pub struct RetryObserver {
    notify: NoticeCallback,
    countdown: Option<CountdownCallback>,
}

impl RetryObserver {
    pub fn new(observer: impl Fn(&RetryNotice) + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(observer),
            countdown: None,
        }
    }

    /// Also calls `countdown` with the time left, once per second of the
    /// wait and a last time with zero right before the retry is sent.
    #[must_use]
    pub fn with_countdown(
        mut self,
        countdown: impl Fn(&RetryNotice, Duration) + Send + Sync + 'static,
    ) -> Self {
        self.countdown = Some(Arc::new(countdown));
        self
    }

    /// Progress on stderr. On a terminal the notice is one line that counts
    /// down to the retry; otherwise (logs, pipes) it is printed once.
    #[must_use]
    pub fn stderr() -> Self {
        if !std::io::stderr().is_terminal() {
            return Self::new(|notice| eprintln!("{}", notice.render()));
        }
        Self::new(|notice| {
            let mut stderr = std::io::stderr().lock();
            let _ = write!(stderr, "\r\x1b[2K{}", notice.render());
            let _ = stderr.flush();
        })
        .with_countdown(|notice, remaining| {
            let mut stderr = std::io::stderr().lock();
            let line = notice.render_remaining(remaining);
            let _ = if remaining.is_zero() {
                writeln!(stderr, "\r\x1b[2K{line}")
            } else {
                write!(stderr, "\r\x1b[2K{line}")
            };
            let _ = stderr.flush();
        })
    }

    pub(crate) fn notify(&self, notice: &RetryNotice) {
        (self.notify)(notice);
    }

    /// Sleeps for `delay`, reporting the countdown when one is registered.
    pub(crate) async fn wait(&self, notice: &RetryNotice, delay: Duration) {
        let Some(countdown) = &self.countdown else {
            tokio::time::sleep(delay).await;
            return;
        };
        let tick = Duration::from_secs(1);
        let mut remaining = delay;
        while !remaining.is_zero() {
            let step = remaining.min(tick);
            tokio::time::sleep(step).await;
            remaining -= step;
            countdown(notice, remaining);
        }
        if delay.is_zero() {
            countdown(notice, Duration::ZERO);
        }
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
    fn capacity_budget_covers_the_waits_the_free_pools_ask_for() {
        use super::{RetryBudget, CAPACITY_MAX_WAIT, CAPACITY_TOTAL_BACKOFF};
        let backoff = Duration::from_secs(1);
        let capacity = RetryBudget {
            total: CAPACITY_TOTAL_BACKOFF,
            max_wait: Some(CAPACITY_MAX_WAIT),
        };
        let standard = RetryBudget::standard(MAX_TOTAL_BACKOFF);
        let secs = Duration::from_secs;

        // The failed live run: Retry-After 36 s, then 58 s.
        assert_eq!(
            capacity.plan(Some(secs(36)), backoff, secs(0)),
            Some(secs(36))
        );
        assert_eq!(
            capacity.plan(Some(secs(58)), backoff, secs(36)),
            Some(secs(58))
        );
        // The default budget gives up on the second wait.
        assert_eq!(
            standard.plan(Some(secs(36)), backoff, secs(0)),
            Some(secs(36))
        );
        assert_eq!(standard.plan(Some(secs(58)), backoff, secs(36)), None);

        // Each wait is capped at 90 s and all of them at 180 s.
        assert_eq!(
            capacity.plan(Some(secs(90)), backoff, secs(0)),
            Some(secs(90))
        );
        assert_eq!(capacity.plan(Some(secs(91)), backoff, secs(0)), None);
        assert_eq!(
            capacity.plan(Some(secs(90)), backoff, secs(90)),
            Some(secs(90))
        );
        assert_eq!(capacity.plan(Some(secs(1)), backoff, secs(180)), None);
        assert_eq!(capacity.plan(None, secs(32), secs(170)), Some(secs(10)));
    }

    #[test]
    fn notice_counts_down_to_the_retry() {
        let notice = RetryNotice {
            provider: "Zero-X".to_string(),
            retry: 2,
            max_retries: 2,
            delay: Duration::from_secs(58),
            reason: "capacity_exhausted".to_string(),
        };
        assert_eq!(
            notice.render_remaining(Duration::from_secs(41)),
            "Zero-X capacity_exhausted; retrying in 41s (retry 2 of 2)"
        );
        assert_eq!(
            notice.render_remaining(Duration::ZERO),
            "Zero-X capacity_exhausted; retrying now (retry 2 of 2)"
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
