//! The plan's rolling token window (Free: 1,000,000 tokens every 5 hours) as this
//! process knows it, and what Neuron does about it.
//!
//! - [`remember`] keeps what each `GET /api/account` read said; [`current`] moves it
//!   by the quota headers of the gateway replies since (on a plan with a window, the
//!   token header is what the window still allows).
//! - [`near_limit_notice`] warns once when 90% of the window is used.
//! - [`WindowGuard`] is the conversation's [`RequestGuard`]: a smaller working set on
//!   a plan with a window (`workingSetTokens`), and a question before a request would
//!   need more tokens than the window has left. A sub-agent gets one too
//!   ([`subagent_guard_factory`]) that stops instead of asking.
//! - Once the window's `resets_at` has passed, what the last read said is no longer
//!   what is left: the figures count as unknown (nothing is asked or warned about) and
//!   the account is read again in the background.

use std::io::{self, BufRead as _, Write as _};
use std::sync::{Mutex, PoisonError};

use runtime::ui_hooks::{self, PromptStream, UserQuestion};
use runtime::{RequestEstimate, RequestGuard};

use crate::account::{short_tokens, AccountSummary, PlanWindow};

/// The working set on a plan with a window, in estimated transcript tokens: about 40
/// requests of this size fill the Free window.
pub(crate) const DEFAULT_WORKING_SET_TOKENS: usize = 40_000;

/// Overrides the `workingSetTokens` setting; `0` turns the working set off.
pub(crate) const WORKING_SET_ENV_VAR: &str = "NEURON_WORKING_SET_TOKENS";

/// What the gateway holds back for a request's answer while it runs: its prompt
/// plus up to this many output tokens count in the window.
const GATEWAY_OUTPUT_RESERVE_TOKENS: usize = 1_024;

/// What this process knows of the window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Known {
    /// An account was read since the start (or the last sign-out).
    read: bool,
    /// What the last account read said: `None` for a plan without a window, and
    /// before any read.
    window: Option<PlanWindow>,
    /// The quota headers `window` was last moved by, or that were current when it
    /// was read (the read is newer than them).
    quota_seen: Option<api::QuotaRemaining>,
    /// The near-limit notice was shown since the window last went below 90%.
    warned: bool,
    /// The passed `resets_at` the account was last read again for: one passed reset
    /// time starts one read.
    reread_for: Option<String>,
}

impl Known {
    const fn new() -> Self {
        Self {
            read: false,
            window: None,
            quota_seen: None,
            warned: false,
            reread_for: None,
        }
    }

    fn remember(&mut self, window: Option<PlanWindow>, quota: Option<api::QuotaRemaining>) {
        self.read = true;
        self.window = window;
        self.quota_seen = quota;
    }

    /// Moves the window by the quota headers of the last reply, when they changed. A
    /// reply after the window's reset time says what is left now; when it next frees up
    /// waits for the next read.
    fn apply_quota(&mut self, quota: Option<api::QuotaRemaining>, now: u64) {
        if quota.is_none() || quota == self.quota_seen {
            return;
        }
        self.quota_seen = quota;
        if let (Some(window), Some(left)) =
            (self.window.as_mut(), quota.and_then(|quota| quota.tokens))
        {
            if window.is_past_reset(now) {
                window.resets_at = None;
            }
            window.remaining_tokens = Some(left);
            window.used_tokens = Some(window.limit_tokens.saturating_sub(left));
        }
    }

    /// The window at `now` (see [`PlanWindow::as_of`]).
    fn window(&self, now: u64) -> Option<PlanWindow> {
        self.window.as_ref().map(|window| window.as_of(now))
    }

    /// Whether to read the account again: the window's reset time has passed, and no
    /// read was started for it yet.
    fn needs_reread(&mut self, now: u64) -> bool {
        let Some(window) = self
            .window
            .as_ref()
            .filter(|window| window.is_past_reset(now))
        else {
            return false;
        };
        if self.reread_for == window.resets_at {
            return false;
        }
        self.reread_for.clone_from(&window.resets_at);
        true
    }

    /// See [`has_window`].
    fn has_window(&self, free_login: bool) -> bool {
        if self.read {
            self.window.is_some()
        } else {
            free_login
        }
    }

    /// The near-limit warning, once per stretch above 90% of the window.
    fn near_limit_notice(&mut self, now: u64) -> Option<String> {
        let window = self.window(now)?;
        if !window.is_near_limit() {
            self.warned = false;
            return None;
        }
        if std::mem::replace(&mut self.warned, true) {
            return None;
        }
        Some(near_limit_message(&window, now))
    }
}

static KNOWN: Mutex<Known> = Mutex::new(Known::new());

fn known() -> std::sync::MutexGuard<'static, Known> {
    KNOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps what an account read said about the window (or that there is none).
pub(crate) fn remember(account: &AccountSummary) {
    known().remember(account.window(), api::last_quota_remaining());
}

/// Forgets the window (after `/logout`).
pub(crate) fn forget() {
    *known() = Known::new();
}

/// The window as it stands now, when the plan has one and it was read. Past its reset
/// time the account is read again in the background.
pub(crate) fn current() -> Option<PlanWindow> {
    let now = crate::clock::now_unix();
    let (window, reread) = {
        let mut known = known();
        let reread = known.needs_reread(now);
        known.apply_quota(api::last_quota_remaining(), now);
        (known.window(now), reread)
    };
    if reread {
        read_in_background();
    }
    window
}

/// The near-limit warning, once each time the window passes 90%.
pub(crate) fn near_limit_notice() -> Option<String> {
    let now = crate::clock::now_unix();
    let mut known = known();
    known.apply_quota(api::last_quota_remaining(), now);
    known.near_limit_notice(now)
}

/// Whether the plan has a window: as the last account read said, else (before one)
/// whether the stored login's plan is Free, the plan with a window.
fn has_window(free_login: bool) -> bool {
    known().has_window(free_login)
}

/// `plan` names the Free plan.
pub(crate) fn is_free_plan(plan: Option<&str>) -> bool {
    plan.is_some_and(|plan| plan.trim().eq_ignore_ascii_case("free"))
}

fn near_limit_message(window: &PlanWindow, now: u64) -> String {
    let plan = window
        .plan_name
        .as_deref()
        .map_or_else(String::new, |plan| format!("{plan} plan: "));
    let span = window.length().map_or_else(
        || "token window".to_string(),
        |length| format!("{length} token window"),
    );
    format!(
        "{plan}{} of the {} tokens of your {span} are used{}. Requests are refused once it is used up; /usage has the details.",
        short_tokens(window.used().unwrap_or(0)),
        short_tokens(window.limit_tokens),
        frees_up(window, now),
    )
}

/// `; it frees up at 14:20 UTC (in 1h 5m)`, when the window says.
fn frees_up(window: &PlanWindow, now: u64) -> String {
    window
        .resets_at
        .as_deref()
        .and_then(crate::clock::parse_timestamp)
        .map_or_else(String::new, |at| {
            let stamp = crate::clock::format_utc(at);
            let time = stamp
                .split_once(' ')
                .map_or(stamp.as_str(), |(_, time)| time);
            format!(
                "; it frees up at {time} ({})",
                crate::clock::describe_relative(at, now)
            )
        })
}

/// The `Plan` section of `/status`: the window, the size of the next request and the
/// working set.
pub(crate) fn status_section(
    window: Option<&PlanWindow>,
    request_tokens: usize,
    working_set: Option<usize>,
) -> String {
    let mut lines = vec!["Plan".to_string()];
    if let Some(window) = window {
        lines.push(format!("  Token window     {}", window.summary()));
    }
    lines.push(format!(
        "  Next request     about {} tokens (sent again at every step of a turn)",
        short_tokens(u64::try_from(request_tokens).unwrap_or(u64::MAX))
    ));
    if let Some(working_set) = working_set {
        lines.push(format!(
            "  Working set      {} tokens; older tool results become stubs",
            short_tokens(u64::try_from(working_set).unwrap_or(u64::MAX))
        ));
    }
    lines.join("\n")
}

/// Reads `/api/account` on a thread of its own, so the line REPL and a `neuron prompt`
/// run know the window before the first reply (or again once it has reset). Does
/// nothing without a usable Zero-X login.
pub(crate) fn read_in_background() {
    let Ok(Some(credential)) = crate::auth::current_credential() else {
        return;
    };
    if credential.is_expired(crate::clock::now_unix()) {
        return;
    }
    let Ok(base) = crate::auth::gateway_base_url() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("neuron-plan-window".to_string())
        .spawn(move || {
            // `fetch_account` remembers the window; a failure leaves it unknown.
            let _ = crate::account::fetch_account(&base, &credential.token);
        });
}

/// [`read_in_background`] when the active provider is Zero-X (the line REPL and
/// `neuron prompt`; the full-screen engine reads the account itself).
pub(crate) fn read_on_gateway() {
    if crate::provider::active_provider()
        .is_some_and(|active| active.id == crate::provider::ProviderId::ZeroX)
    {
        read_in_background();
    }
}

/// The working set a conversation keeps: `configured` (the setting, or
/// [`WORKING_SET_ENV_VAR`]) when given, `0` meaning none; else
/// [`DEFAULT_WORKING_SET_TOKENS`] on the gateway for a plan with a window.
fn working_set(configured: Option<usize>, gateway: bool, has_window: bool) -> Option<usize> {
    match configured {
        Some(0) => None,
        Some(tokens) => Some(tokens),
        None => (gateway && has_window).then_some(DEFAULT_WORKING_SET_TOKENS),
    }
}

/// The working set configured: [`WORKING_SET_ENV_VAR`], else the `workingSetTokens`
/// setting.
pub(crate) fn configured_working_set(setting: Option<usize>) -> Option<usize> {
    std::env::var(WORKING_SET_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .or(setting)
}

/// What to do about a request, given the window.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Send,
    /// Ask first; the question.
    Ask(UserQuestion),
}

/// Whether a request of `request` fits what is left of `window`; when it does not,
/// the question to ask.
fn decide(window: Option<&PlanWindow>, request: &RequestEstimate, now: u64) -> Decision {
    let Some(window) = window else {
        return Decision::Send;
    };
    let Some(remaining) = window.remaining() else {
        return Decision::Send;
    };
    let needed = request
        .input_tokens
        .saturating_add(request.output_reserve.min(GATEWAY_OUTPUT_RESERVE_TOKENS));
    let needed = u64::try_from(needed).unwrap_or(u64::MAX);
    if needed <= remaining {
        return Decision::Send;
    }
    let plan = window.plan_name.as_deref().map_or_else(
        || "your plan".to_string(),
        |plan| format!("your {plan} plan"),
    );
    let span = window.length().map_or_else(
        || "token window".to_string(),
        |length| format!("{length} token window"),
    );
    Decision::Ask(UserQuestion {
        question: format!(
            "The next request needs about {} tokens, but {plan} has {} left in its {span}{}. Send it anyway? The gateway may refuse it; /compact first makes the conversation smaller.",
            short_tokens(needed),
            short_tokens(remaining),
            frees_up(window, now),
        ),
        options: Some(vec!["Send anyway".to_string(), "Stop here".to_string()]),
    })
}

/// Whether `answer` (raw, as typed or as the UI returned it) says to send.
fn answer_sends(answer: &str) -> bool {
    let answer = answer.trim().to_ascii_lowercase();
    matches!(answer.as_str(), "1" | "y" | "yes" | "continue") || answer.starts_with("send")
}

/// Asks `question`: through the UI's handler when one is installed, else at the
/// terminal. `None` when nobody can answer (a script); `Some(false)` when the question
/// could not be answered (the turn was interrupted, the UI closed).
fn ask(question: &UserQuestion) -> Option<bool> {
    ask_with(
        crate::headless::unattended(),
        ui_hooks::ask_handler(),
        question,
    )
}

/// [`ask`], given whether nobody attends this run (`neuron prompt` with stdin not a
/// terminal, whose handler refuses every question: that is not the user choosing to
/// stop) and the UI's handler.
fn ask_with(
    unattended: bool,
    handler: Option<impl Fn(&UserQuestion) -> io::Result<String>>,
    question: &UserQuestion,
) -> Option<bool> {
    if unattended {
        return None;
    }
    if let Some(handler) = handler {
        return Some(handler(question).is_ok_and(|answer| answer_sends(&answer)));
    }
    if !ui_hooks::stdin_is_terminal() {
        return None;
    }
    let stream = PromptStream::current()?;
    let mut out = stream.writer();
    let asked = (|| -> io::Result<String> {
        writeln!(out, "\n[Question] {}", question.question)?;
        for (index, option) in question.options.iter().flatten().enumerate() {
            writeln!(out, "  {}. {option}", index + 1)?;
        }
        write!(out, "Enter choice (1-2): ")?;
        out.flush()?;
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        Ok(answer)
    })();
    Some(asked.is_ok_and(|answer| answer_sends(&answer)))
}

/// What a [`WindowGuard`] does about a request the window does not cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverWindow {
    /// Ask the user (the conversation).
    Ask,
    /// Stop without asking (a sub-agent); the conversation's own next request is then
    /// asked about.
    Stop,
}

/// The conversation's [`RequestGuard`] (see the module docs).
pub(crate) struct WindowGuard {
    /// The requests go to the Zero-X gateway; other providers have no window.
    gateway: bool,
    over_window: OverWindow,
    /// `workingSetTokens` or [`WORKING_SET_ENV_VAR`].
    configured: Option<usize>,
    /// The login's plan is Free, for before the account is read.
    free_login: bool,
    /// The user chose to send past the window this turn: not asked again until the
    /// next one.
    approved: bool,
}

impl WindowGuard {
    pub(crate) fn new(gateway: bool, configured: Option<usize>) -> Self {
        let free_login = gateway
            && crate::auth::current_credential()
                .ok()
                .flatten()
                .is_some_and(|credential| is_free_plan(credential.plan.as_deref()));
        Self {
            gateway,
            over_window: OverWindow::Ask,
            configured,
            free_login,
            approved: false,
        }
    }

    /// A sub-agent's guard: the same working set, and a request the window does not
    /// cover stops the sub-agent without asking. Sub-agents run beside the conversation
    /// (some in the background), where a question would compete with its prompt.
    pub(crate) fn for_subagent(gateway: bool, configured: Option<usize>) -> Self {
        Self {
            over_window: OverWindow::Stop,
            ..Self::new(gateway, configured)
        }
    }

    /// [`RequestGuard::allow_request`], given the window, the time and who answers.
    fn allow(
        &mut self,
        window: Option<&PlanWindow>,
        request: &RequestEstimate,
        now: u64,
        ask: impl FnOnce(&UserQuestion) -> Option<bool>,
    ) -> bool {
        if request.iteration <= 1 {
            self.approved = false;
        }
        if !self.gateway || self.approved {
            return true;
        }
        let Decision::Ask(question) = decide(window, request, now) else {
            return true;
        };
        if self.over_window == OverWindow::Stop {
            return false;
        }
        match ask(&question) {
            Some(send) => {
                self.approved = send;
                send
            }
            None => {
                // Nobody to ask (a script): the gateway decides, and says why it refuses.
                ui_hooks::notice(&format!("warning: {}", question.question));
                true
            }
        }
    }
}

impl RequestGuard for WindowGuard {
    fn working_set_tokens(&self) -> Option<usize> {
        working_set(
            self.configured,
            self.gateway,
            self.gateway && has_window(self.free_login),
        )
    }

    fn allow_request(&mut self, request: &RequestEstimate) -> bool {
        let window = if self.gateway { current() } else { None };
        self.allow(window.as_ref(), request, crate::clock::now_unix(), ask)
    }
}

/// Builds each sub-agent's [`WindowGuard`] (see [`WindowGuard::for_subagent`]) for the
/// `Agent` tool; `configured` is the conversation's working set setting.
pub(crate) fn subagent_guard_factory(configured: Option<usize>) -> tools::SubagentGuardFactory {
    std::sync::Arc::new(move |client: &api::ProviderClient| {
        let guard: Box<dyn RequestGuard> = Box::new(WindowGuard::for_subagent(
            crate::api_client::is_gateway_client(client),
            configured,
        ));
        Some(guard)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: u64) -> PlanWindow {
        PlanWindow {
            plan_name: Some("Free".to_string()),
            limit_tokens: 1_000_000,
            window_seconds: Some(18_000),
            used_tokens: Some(used),
            remaining_tokens: Some(1_000_000 - used),
            resets_at: Some("2026-10-17T14:20:00Z".to_string()),
        }
    }

    fn request(input_tokens: usize) -> RequestEstimate {
        RequestEstimate {
            input_tokens,
            output_reserve: 32_000,
            iteration: 1,
        }
    }

    /// 13:15 UTC on 2026-10-17: the window frees up 1h 5m later.
    const NOW: u64 = 1_792_246_800 - 3_900;

    #[test]
    fn quota_headers_move_the_window_once_and_only_after_the_read() {
        let mut known = Known::new();
        let before_read = api::QuotaRemaining {
            requests: Some(99),
            tokens: Some(999_000),
        };
        known.remember(Some(window(640_000)), Some(before_read));
        known.apply_quota(Some(before_read), NOW);
        assert_eq!(
            known.window(NOW).and_then(|w| w.used()),
            Some(640_000),
            "older than the read"
        );

        known.apply_quota(
            Some(api::QuotaRemaining {
                requests: Some(98),
                tokens: Some(300_000),
            }),
            NOW,
        );
        let moved = known.window(NOW).expect("a window");
        assert_eq!(
            (moved.used(), moved.remaining()),
            (Some(700_000), Some(300_000))
        );
        assert_eq!(moved.summary(), "Free 700K/1M, resets 14:20 UTC");

        let mut none = Known::new();
        none.remember(None, None);
        none.apply_quota(Some(before_read), NOW);
        assert_eq!(none.window(NOW), None, "a plan without a window gets none");
    }

    /// 16:20 UTC: two hours after the window's reset time.
    const AFTER_RESET: u64 = 1_792_246_800 + 7_200;

    #[test]
    fn past_its_reset_time_the_window_counts_as_unknown_until_read_again() {
        let mut known = Known::new();
        known.remember(Some(window(990_000)), None);
        assert!(known.near_limit_notice(NOW).is_some());

        let stale = known.window(AFTER_RESET).expect("a window");
        assert_eq!((stale.used(), stale.remaining()), (None, None));
        assert_eq!(stale.summary(), "Free ?/1M");
        assert_eq!(
            decide(Some(&stale), &request(44_000), AFTER_RESET),
            Decision::Send,
            "not asked from old figures"
        );
        assert_eq!(known.near_limit_notice(AFTER_RESET), None, "not warned");
        assert!(
            status_section(Some(&stale), 900, None).contains("Token window     Free ?/1M\n"),
            "no old figure in /status"
        );

        assert!(!known.needs_reread(NOW));
        assert!(known.needs_reread(AFTER_RESET), "read again");
        assert!(!known.needs_reread(AFTER_RESET + 60), "once per reset time");

        // A reply after the reset says what is left now.
        known.apply_quota(
            Some(api::QuotaRemaining {
                requests: Some(50),
                tokens: Some(700_000),
            }),
            AFTER_RESET,
        );
        let moved = known.window(AFTER_RESET).expect("a window");
        assert_eq!(moved.remaining(), Some(700_000));
        assert_eq!(moved.summary(), "Free 300K/1M");
    }

    #[test]
    fn before_a_read_the_login_plan_says_whether_there_is_a_window() {
        let unread = Known::new();
        assert!(unread.has_window(true), "a Free login");
        assert!(!unread.has_window(false));

        let mut pro = Known::new();
        pro.remember(None, None);
        assert!(!pro.has_window(true), "the read wins over the stored plan");

        let mut free = Known::new();
        free.remember(Some(window(0)), None);
        assert!(free.has_window(false));
    }

    #[test]
    fn the_near_limit_notice_comes_once_per_stretch_above_ninety_percent() {
        let mut known = Known::new();
        known.remember(Some(window(500_000)), None);
        assert_eq!(known.near_limit_notice(NOW), None);
        known.remember(Some(window(920_000)), None);
        let notice = known.near_limit_notice(NOW).expect("a warning");
        assert_eq!(
            notice,
            "Free plan: 920K of the 1M tokens of your 5-hour token window are used; it frees up at 14:20 UTC (in 1h 5m). Requests are refused once it is used up; /usage has the details."
        );
        assert_eq!(known.near_limit_notice(NOW), None, "once");
        known.remember(Some(window(100_000)), None);
        assert_eq!(known.near_limit_notice(NOW), None);
        known.remember(Some(window(950_000)), None);
        assert!(
            known.near_limit_notice(NOW).is_some(),
            "again after it freed up"
        );
    }

    #[test]
    fn a_request_that_fits_is_sent_and_one_that_does_not_is_asked_about() {
        assert_eq!(decide(None, &request(900_000), NOW), Decision::Send);
        assert_eq!(
            decide(Some(&window(640_000)), &request(30_000), NOW),
            Decision::Send
        );
        let unknown = PlanWindow {
            used_tokens: None,
            remaining_tokens: None,
            ..window(0)
        };
        assert_eq!(
            decide(Some(&unknown), &request(900_000), NOW),
            Decision::Send
        );

        // 44K input and the gateway's 1,024 output tokens: more than the 40K left.
        let Decision::Ask(question) = decide(Some(&window(960_000)), &request(44_000), NOW) else {
            panic!("asks");
        };
        assert_eq!(
            question.question,
            "The next request needs about 45K tokens, but your Free plan has 40K left in its 5-hour token window; it frees up at 14:20 UTC (in 1h 5m). Send it anyway? The gateway may refuse it; /compact first makes the conversation smaller."
        );
        assert_eq!(
            question.options.as_deref(),
            Some(&["Send anyway".to_string(), "Stop here".to_string()][..])
        );
    }

    fn question() -> UserQuestion {
        UserQuestion {
            question: "Send it anyway?".to_string(),
            options: None,
        }
    }

    #[test]
    fn a_run_nobody_attends_is_not_asked_and_its_refusal_is_not_an_answer() {
        let refusing =
            |_: &UserQuestion| -> io::Result<String> { panic!("an unattended run is not asked") };
        assert_eq!(ask_with(true, Some(refusing), &question()), None);
        let answering = |_: &UserQuestion| -> io::Result<String> { Ok("Send anyway".into()) };
        assert_eq!(ask_with(false, Some(answering), &question()), Some(true));
        let closed = |_: &UserQuestion| -> io::Result<String> { Err(io::Error::other("closed")) };
        assert_eq!(ask_with(false, Some(closed), &question()), Some(false));
    }

    fn guard(interactive: bool) -> WindowGuard {
        WindowGuard {
            gateway: true,
            over_window: if interactive {
                OverWindow::Ask
            } else {
                OverWindow::Stop
            },
            configured: None,
            free_login: true,
            approved: false,
        }
    }

    #[test]
    fn the_guard_asks_warns_and_sends_when_nobody_answers_and_a_subagent_stops() {
        let short = window(960_000);
        let big = request(44_000);
        // Nobody to ask: a warning, and the gateway decides.
        assert!(guard(true).allow(Some(&short), &big, NOW, |_| None));
        // The user stops; then sends, and is not asked again this turn.
        assert!(!guard(true).allow(Some(&short), &big, NOW, |_| Some(false)));
        let mut asked = guard(true);
        assert!(asked.allow(Some(&short), &big, NOW, |_| Some(true)));
        let later = RequestEstimate {
            iteration: 2,
            ..big
        };
        assert!(asked.allow(Some(&short), &later, NOW, |_| panic!("asked again")));
        // A sub-agent stops without asking.
        assert!(!guard(false).allow(Some(&short), &big, NOW, |_| panic!("asked")));
        assert!(guard(false).allow(Some(&window(0)), &big, NOW, |_| panic!("asked")));
        // Past the reset time nothing is known: sent without asking.
        assert!(guard(true).allow(
            Some(&short.as_of(AFTER_RESET)),
            &big,
            AFTER_RESET,
            |_| panic!("asked from old figures")
        ));
        assert_eq!(
            WindowGuard::for_subagent(false, Some(30_000)).working_set_tokens(),
            Some(30_000)
        );
    }

    #[test]
    fn answers_that_send() {
        for answer in ["1", "y", "Yes\n", "Send anyway", "send", "continue"] {
            assert!(answer_sends(answer), "{answer}");
        }
        for answer in ["2", "n", "Stop here", "", "no thanks"] {
            assert!(!answer_sends(answer), "{answer}");
        }
    }

    #[test]
    fn the_working_set_follows_the_setting_then_the_plan() {
        assert_eq!(
            working_set(None, true, true),
            Some(DEFAULT_WORKING_SET_TOKENS)
        );
        assert_eq!(
            working_set(None, true, false),
            None,
            "a plan without a window"
        );
        assert_eq!(working_set(None, false, true), None, "not the gateway");
        assert_eq!(working_set(Some(0), true, true), None, "turned off");
        assert_eq!(
            working_set(Some(60_000), false, false),
            Some(60_000),
            "any plan"
        );
        assert!(is_free_plan(Some(" Free ")));
        assert!(!is_free_plan(Some("Pro")));
        assert!(!is_free_plan(None));
    }

    #[test]
    fn status_shows_the_window_the_next_request_and_the_working_set() {
        let section = status_section(Some(&window(640_000)), 12_400, Some(40_000));
        assert_eq!(
            section,
            "Plan\n  Token window     Free 640K/1M, resets 14:20 UTC\n  Next request     about 12K tokens (sent again at every step of a turn)\n  Working set      40K tokens; older tool results become stubs"
        );
        assert_eq!(
            status_section(None, 900, None),
            "Plan\n  Next request     about 900 tokens (sent again at every step of a turn)"
        );
    }
}
