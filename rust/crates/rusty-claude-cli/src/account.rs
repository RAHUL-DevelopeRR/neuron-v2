//! Zero-X account surfaces: `/usage` and `neuron usage` (`GET /api/account`)
//! and `/upgrade` (opens the pricing page).

use std::io::{self, IsTerminal};

use serde_json::Value;

use crate::gateway_http::{self, string_field, u64_field, GatewayHttpError, ACCOUNT_TIMEOUT};
use crate::vault::SecureString;

/// Default pricing page (contract §3.3 `upgrade_url`).
pub const DEFAULT_PRICING_URL: &str = "https://zero-x.live/pricing";

/// `NEURON_PRICING_URL`, or the default pricing page.
pub fn pricing_url() -> String {
    std::env::var("NEURON_PRICING_URL")
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .unwrap_or_else(|| DEFAULT_PRICING_URL.to_string())
}

/// The parts of `GET /api/account` the CLI renders.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct AccountSummary {
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan_id: Option<String>,
    pub plan_name: Option<String>,
    pub daily_requests_limit: Option<u64>,
    pub daily_tokens_limit: Option<u64>,
    pub requests_today: Option<u64>,
    pub tokens_today: Option<u64>,
    pub resets_at: Option<String>,
    /// The plan's rolling token window (Free: 1,000,000 tokens every 5 hours), when it
    /// has one. With a window, the daily token figures refuse nothing.
    pub window_tokens_limit: Option<u64>,
    pub window_seconds: Option<u64>,
    /// Tokens counted in the window now, and what it still allows.
    pub window_tokens_used: Option<u64>,
    pub window_tokens_remaining: Option<u64>,
    /// When enough of the window frees up (ISO 8601); `None` while nothing counts in it.
    pub window_resets_at: Option<String>,
    pub credit_balance_usd: Option<f64>,
    pub monthly_grant_usd: Option<f64>,
    pub subscription_status: Option<String>,
    pub current_period_end: Option<String>,
    pub cancel_at_period_end: bool,
    pub suspended: bool,
}

/// A plan's rolling token window as `GET /api/account` reports it (`limits.window_*`
/// and `usage_window`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanWindow {
    /// The plan as people read it ("Free"), when known.
    pub plan_name: Option<String>,
    pub limit_tokens: u64,
    pub window_seconds: Option<u64>,
    pub used_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    /// When enough of the window frees up (ISO 8601).
    pub resets_at: Option<String>,
}

impl PlanWindow {
    /// Tokens counted in the window: as reported, else what the remaining ones leave.
    #[must_use]
    pub fn used(&self) -> Option<u64> {
        self.used_tokens.or_else(|| {
            self.remaining_tokens
                .map(|remaining| self.limit_tokens.saturating_sub(remaining))
        })
    }

    /// What the window still allows: as reported, else the limit less what is used.
    #[must_use]
    pub fn remaining(&self) -> Option<u64> {
        self.remaining_tokens.or_else(|| {
            self.used_tokens
                .map(|used| self.limit_tokens.saturating_sub(used))
        })
    }

    /// `resets_at` has passed by `now`: some of what was used has left the window since
    /// it was read, so how much is left is not known until the next read.
    #[must_use]
    pub fn is_past_reset(&self, now: u64) -> bool {
        self.resets_at
            .as_deref()
            .and_then(crate::clock::parse_timestamp)
            .is_some_and(|at| at <= now)
    }

    /// The window as it stands at `now`: once [`Self::is_past_reset`], what is used and
    /// left is unknown (and when it frees up next too) until the account is read again.
    #[must_use]
    pub fn as_of(&self, now: u64) -> Self {
        if !self.is_past_reset(now) {
            return self.clone();
        }
        Self {
            used_tokens: None,
            remaining_tokens: None,
            resets_at: None,
            ..self.clone()
        }
    }

    /// 90% of the window or more is used.
    #[must_use]
    pub fn is_near_limit(&self) -> bool {
        near_limit(self.used(), Some(self.limit_tokens))
    }

    /// `Free 640K/1M, resets 14:20 UTC`: the plan, what is used of the window (`?` when
    /// not known), and when it frees up.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut line = String::new();
        if let Some(plan) = &self.plan_name {
            line.push_str(plan);
            line.push(' ');
        }
        line.push_str(&format!(
            "{}/{}",
            self.used().map_or_else(|| "?".to_string(), short_tokens),
            short_tokens(self.limit_tokens)
        ));
        if let Some(at) = self
            .resets_at
            .as_deref()
            .and_then(crate::clock::parse_timestamp)
        {
            line.push_str(&format!(", resets {}", clock_time(at)));
        }
        line
    }

    /// How long the window is, as people read it: `5-hour`.
    #[must_use]
    pub fn length(&self) -> Option<String> {
        let seconds = self.window_seconds.filter(|seconds| *seconds > 0)?;
        Some(if seconds.is_multiple_of(3_600) {
            format!("{}-hour", seconds / 3_600)
        } else {
            format!("{}-minute", seconds.div_ceil(60))
        })
    }
}

/// `used` is at least 90% of `limit`.
fn near_limit(used: Option<u64>, limit: Option<u64>) -> bool {
    match (used, limit) {
        (Some(used), Some(limit)) if limit > 0 => {
            used.saturating_mul(10) >= limit.saturating_mul(9)
        }
        _ => false,
    }
}

/// `14:20 UTC` for Unix seconds.
fn clock_time(unix: u64) -> String {
    let stamp = crate::clock::format_utc(unix);
    // `2026-10-17 14:20 UTC`: the time and the zone.
    stamp
        .split_once(' ')
        .map_or_else(|| stamp.clone(), |(_, time)| time.to_string())
}

/// A token count in a few characters: `999`, `12K`, `640K`, `1M`, `1.5M`.
#[must_use]
pub fn short_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        let tenths = value / 100_000;
        if tenths.is_multiple_of(10) || value >= 10_000_000 {
            format!("{}M", value / 1_000_000)
        } else {
            format!("{}.{}M", tenths / 10, tenths % 10)
        }
    } else if value >= 1_000 {
        format!("{}K", value / 1_000)
    } else {
        value.to_string()
    }
}

impl AccountSummary {
    fn is_free(&self) -> bool {
        self.plan_id.as_deref().is_none_or(|plan| plan == "free")
    }

    /// The plan's rolling token window, when it has one.
    #[must_use]
    pub fn window(&self) -> Option<PlanWindow> {
        let limit_tokens = self.window_tokens_limit.filter(|limit| *limit > 0)?;
        Some(PlanWindow {
            plan_name: self.plan_name.clone().or_else(|| self.plan_id.clone()),
            limit_tokens,
            window_seconds: self.window_seconds,
            used_tokens: self.window_tokens_used,
            remaining_tokens: self.window_tokens_remaining,
            resets_at: self.window_resets_at.clone(),
        })
    }

    /// Close to what refuses requests: the token window when the plan has one (its
    /// daily token figure then refuses nothing), else today's tokens; and today's
    /// requests.
    fn near_limit(&self) -> bool {
        let tokens = match self.window() {
            Some(window) => window.is_near_limit(),
            None => near_limit(self.tokens_today, self.daily_tokens_limit),
        };
        tokens || near_limit(self.requests_today, self.daily_requests_limit)
    }
}

fn f64_field(value: &Value, key: &str) -> Option<f64> {
    match value.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Parses the contract §3.4 `GET /api/account` body.
pub fn parse_account(value: &Value) -> AccountSummary {
    let section = |key: &str| value.get(key).filter(|section| section.is_object());
    let user = section("user");
    let plan = section("plan");
    let limits = section("limits");
    let usage = section("usage_today");
    // `null` on plans without a window.
    let window = section("usage_window");
    let credits = section("credits");
    let subscription = section("subscription");
    AccountSummary {
        email: user.and_then(|user| string_field(user, "email")),
        name: user.and_then(|user| string_field(user, "name")),
        plan_id: plan.and_then(|plan| string_field(plan, "id")),
        plan_name: plan.and_then(|plan| string_field(plan, "name")),
        daily_requests_limit: limits.and_then(|limits| u64_field(limits, "daily_requests")),
        daily_tokens_limit: limits.and_then(|limits| u64_field(limits, "daily_tokens")),
        requests_today: usage.and_then(|usage| u64_field(usage, "requests")),
        tokens_today: usage.and_then(|usage| u64_field(usage, "tokens")),
        resets_at: usage.and_then(|usage| string_field(usage, "resets_at")),
        window_tokens_limit: window
            .and_then(|window| u64_field(window, "limit_tokens"))
            .or_else(|| limits.and_then(|limits| u64_field(limits, "window_tokens"))),
        window_seconds: window
            .and_then(|window| u64_field(window, "window_seconds"))
            .or_else(|| limits.and_then(|limits| u64_field(limits, "window_seconds"))),
        window_tokens_used: window.and_then(|window| u64_field(window, "tokens")),
        window_tokens_remaining: window.and_then(|window| u64_field(window, "remaining_tokens")),
        window_resets_at: window.and_then(|window| string_field(window, "resets_at")),
        credit_balance_usd: credits.and_then(|credits| f64_field(credits, "balance_usd")),
        monthly_grant_usd: credits.and_then(|credits| f64_field(credits, "monthly_grant_usd")),
        subscription_status: subscription.and_then(|sub| string_field(sub, "status")),
        current_period_end: subscription.and_then(|sub| string_field(sub, "current_period_end")),
        cancel_at_period_end: subscription
            .and_then(|sub| sub.get("cancel_at_period_end"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        suspended: value
            .get("flags")
            .and_then(|flags| flags.get("suspended"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

pub fn fetch_account(
    base_url: &str,
    token: &SecureString,
) -> Result<AccountSummary, GatewayHttpError> {
    let value = gateway_http::get_json(
        &gateway_http::account_url(base_url, "/api/account"),
        Some(token.expose()),
        ACCOUNT_TIMEOUT,
    )?;
    let account = parse_account(&value);
    // Every read keeps the plan window current for the status lines and the guard.
    crate::plan_window::remember(&account);
    Ok(account)
}

/// `1234567` -> `1,234,567`.
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn used_of(used: Option<u64>, limit: Option<u64>) -> Option<String> {
    match (used, limit) {
        (Some(used), Some(limit)) => {
            Some(format!("{} / {}", group_digits(used), group_digits(limit)))
        }
        (Some(used), None) => Some(group_digits(used)),
        (None, Some(limit)) => Some(format!("0 / {}", group_digits(limit))),
        (None, None) => None,
    }
}

fn describe_time(text: &str, now: u64) -> String {
    crate::clock::parse_timestamp(text).map_or_else(
        || text.to_string(),
        |stamp| {
            format!(
                "{} ({})",
                crate::clock::format_utc(stamp),
                crate::clock::describe_relative(stamp, now)
            )
        },
    )
}

/// Usage lines in the CLI's sectioned layout (no heading).
pub fn usage_lines(account: &AccountSummary, now: u64) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(email) = &account.email {
        lines.push(format!("  Account          {email}"));
    }
    match (&account.plan_name, &account.plan_id) {
        (Some(name), Some(id)) if !name.eq_ignore_ascii_case(id) => {
            lines.push(format!("  Plan             {name} ({id})"));
        }
        (Some(plan), _) | (None, Some(plan)) => lines.push(format!("  Plan             {plan}")),
        (None, None) => {}
    }
    if account.suspended {
        lines.push("  Status           suspended; contact support@zero-x.live".to_string());
    }
    if let Some(requests) = used_of(account.requests_today, account.daily_requests_limit) {
        lines.push(format!("  Requests today   {requests}"));
    }
    if let Some(tokens) = used_of(account.tokens_today, account.daily_tokens_limit) {
        lines.push(format!("  Tokens today     {tokens}"));
    }
    if let Some(resets_at) = &account.resets_at {
        lines.push(format!(
            "  Resets           {}",
            describe_time(resets_at, now)
        ));
    }
    if let Some(window) = account.window() {
        lines.extend(window_lines(&window.as_of(now), now));
    }
    if let Some(balance) = account.credit_balance_usd {
        let grant = account
            .monthly_grant_usd
            .filter(|grant| *grant > 0.0)
            .map(|grant| format!(" (monthly grant ${grant:.2})"))
            .unwrap_or_default();
        lines.push(format!("  Credits          ${balance:.2}{grant}"));
    }
    if let Some(status) = &account.subscription_status {
        let renewal = account
            .current_period_end
            .as_deref()
            .map_or_else(String::new, |end| {
                let verb = if account.cancel_at_period_end {
                    "ends"
                } else {
                    "renews"
                };
                format!(", {verb} {}", describe_time(end, now))
            });
        lines.push(format!("  Subscription     {status}{renewal}"));
    }
    if account.is_free() || account.near_limit() {
        lines.push(format!("  Upgrade          {} (/upgrade)", pricing_url()));
    }
    lines
}

/// The token window's lines of the usage report: what is used of it, when it frees up,
/// and a warning near its limit.
fn window_lines(window: &PlanWindow, now: u64) -> Vec<String> {
    let mut lines = vec![format!("  Token window     {}", window.summary())];
    lines.push(format!(
        "  Window tokens    {}{}",
        window.used().map_or_else(
            || format!("? / {}", group_digits(window.limit_tokens)),
            |used| format!(
                "{} / {}",
                group_digits(used),
                group_digits(window.limit_tokens)
            )
        ),
        window
            .length()
            .map(|length| format!(" ({length} window)"))
            .unwrap_or_default()
    ));
    if let Some(resets_at) = &window.resets_at {
        lines.push(format!(
            "  Window resets    {}",
            describe_time(resets_at, now)
        ));
    }
    if window.is_near_limit() {
        lines.push(format!(
            "  Warning          {} window tokens left: requests are refused once they are used",
            group_digits(window.remaining().unwrap_or(0))
        ));
    }
    lines
}

/// Full `/usage` report.
pub fn render_usage(account: &AccountSummary, now: u64) -> String {
    let mut lines = vec!["Zero-X usage".to_string()];
    lines.extend(usage_lines(account, now));
    lines.join("\n")
}

/// Fetches the account of the active Zero-X credential. A login that expired (by its
/// stored expiry, or as the gateway says) is signed out, as `neuron auth status` says.
fn current_account() -> Result<AccountSummary, Box<dyn std::error::Error>> {
    let base = crate::auth::gateway_base_url()?;
    let credential = crate::auth::current_credential()?.ok_or(
        "Not signed in to Zero-X. Run `neuron auth login` (or set NEURON_TOKEN) to see usage.",
    )?;
    signed_in(&credential, crate::clock::now_unix())?;
    match fetch_account(&base, &credential.token) {
        Err(error) if error.is_invalid_session() => {
            Err(crate::auth::signed_out_line(&credential, None).into())
        }
        result => Ok(result?),
    }
}

/// `Err` with the signed-out line when `credential` expired by `now`.
fn signed_in(credential: &crate::auth::GatewayCredential, now: u64) -> Result<(), String> {
    if credential.is_expired(now) {
        Err(crate::auth::signed_out_line(credential, Some(now)))
    } else {
        Ok(())
    }
}

/// Fetches and renders the usage report for the active Zero-X credential.
pub fn usage_report() -> Result<String, Box<dyn std::error::Error>> {
    Ok(render_usage(&current_account()?, crate::clock::now_unix()))
}

/// `neuron usage`; with `json`, the account as JSON.
pub fn run_usage_command(args: &[String], json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !args.is_empty() {
        return Err("Usage: neuron usage".into());
    }
    if json {
        return crate::auth::print_json(&usage_json(&current_account()?)?);
    }
    println!("{}", usage_report()?);
    Ok(())
}

fn usage_json(account: &AccountSummary) -> Result<Value, serde_json::Error> {
    Ok(serde_json::json!({
        "type": "usage",
        "account": serde_json::to_value(account)?,
        "upgrade_url": pricing_url(),
    }))
}

/// `/upgrade` and `neuron upgrade [--no-browser]`: prints the pricing URL
/// and opens it when running in an interactive terminal.
pub fn upgrade(open_browser: bool) -> String {
    let (url, opened) = open_pricing_page(open_browser);
    if opened {
        format!("Opening Zero-X plans: {url}")
    } else {
        format!("Zero-X plans and upgrades: {url}")
    }
}

/// The pricing URL, and whether it opened in a browser.
fn open_pricing_page(open_browser: bool) -> (String, bool) {
    // Model availability changes with the plan: refetch the catalog.
    crate::gateway_catalog::invalidate_cache();
    let url = pricing_url();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let opened = open_browser && interactive && webbrowser::open(&url).is_ok();
    (url, opened)
}

/// `neuron upgrade`; with `json`, `{"type":"upgrade","url":…,"opened":…}`.
pub fn run_upgrade_command(args: &[String], json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let open_browser = match args {
        [] => true,
        [flag] if flag == "--no-browser" => false,
        _ => return Err("Usage: neuron upgrade [--no-browser]".into()),
    };
    if json {
        let (url, opened) = open_pricing_page(open_browser);
        return crate::auth::print_json(&serde_json::json!({
            "type": "upgrade",
            "url": url,
            "opened": opened,
        }));
    }
    println!("{}", upgrade(open_browser));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "user": {"id": "u1", "email": "dev@example.com", "name": "Dev", "image_url": null},
            "plan": {"id": "pro", "name": "Pro"},
            "limits": {"daily_requests": 2000, "daily_tokens": 10_000_000, "rpm": 30, "concurrency": 4},
            "usage_today": {"requests": 37, "tokens": 120_431, "resets_at": "2026-10-17T00:00:00Z"},
            "credits": {"balance_usd": 7.42, "monthly_grant_usd": 8},
            "subscription": {"status": "active", "current_period_end": "2026-11-01T00:00:00Z", "cancel_at_period_end": false},
            "flags": {"suspended": false}
        })
    }

    #[test]
    fn parses_the_contract_account_shape() {
        let account = parse_account(&sample());
        assert_eq!(account.email.as_deref(), Some("dev@example.com"));
        assert_eq!(account.name.as_deref(), Some("Dev"));
        assert_eq!(account.plan_id.as_deref(), Some("pro"));
        assert_eq!(account.daily_requests_limit, Some(2000));
        assert_eq!(account.tokens_today, Some(120_431));
        assert_eq!(account.credit_balance_usd, Some(7.42));
        assert!(!account.suspended);
    }

    #[test]
    fn usage_report_shows_limits_reset_time_and_credits() {
        let report = render_usage(&parse_account(&sample()), 1_792_195_200 - 3_600);
        assert!(report.starts_with("Zero-X usage"));
        assert!(report.contains("Plan             Pro\n"), "{report}");
        assert!(!report.contains("(pro)"), "same name and id are shown once");
        assert!(report.contains("Requests today   37 / 2,000"));
        assert!(report.contains("Tokens today     120,431 / 10,000,000"));
        assert!(report.contains("Resets           2026-10-17 00:00 UTC (in 1h 0m)"));
        assert!(report.contains("Credits          $7.42 (monthly grant $8.00)"));
        assert!(report.contains("Subscription     active, renews 2026-11-01 00:00 UTC"));
        assert!(!report.contains("Upgrade"), "paid plan far from limits");
    }

    #[test]
    fn free_or_nearly_exhausted_plans_get_an_upgrade_hint() {
        let mut value = sample();
        value["plan"] = serde_json::json!({"id": "free", "name": "Free"});
        let report = render_usage(&parse_account(&value), 0);
        assert!(report.contains("Upgrade          https://zero-x.live/pricing (/upgrade)"));

        let mut busy = sample();
        busy["usage_today"]["requests"] = serde_json::json!(1950);
        assert!(render_usage(&parse_account(&busy), 0).contains("Upgrade"));
    }

    /// Free with its rolling window, as `GET /api/account` reports it.
    fn free_with_window(used: u64) -> Value {
        let mut value = sample();
        value["plan"] = serde_json::json!({"id": "free", "name": "Free"});
        value["limits"] = serde_json::json!({
            "daily_requests": 100, "daily_tokens": 5_000_000,
            "window_tokens": 1_000_000, "window_seconds": 18_000, "rpm": 10, "concurrency": 2
        });
        value["usage_window"] = serde_json::json!({
            "tokens": used, "limit_tokens": 1_000_000, "remaining_tokens": 1_000_000 - used,
            "window_seconds": 18_000, "resets_at": "2026-10-17T14:20:00Z"
        });
        value
    }

    #[test]
    fn parses_the_plans_token_window() {
        let account = parse_account(&free_with_window(640_000));
        assert_eq!(account.window_tokens_limit, Some(1_000_000));
        assert_eq!(account.window_seconds, Some(18_000));
        assert_eq!(account.window_tokens_used, Some(640_000));
        assert_eq!(account.window_tokens_remaining, Some(360_000));
        let window = account.window().expect("a window");
        assert_eq!(window.summary(), "Free 640K/1M, resets 14:20 UTC");
        assert_eq!(window.length().as_deref(), Some("5-hour"));
        assert!(!window.is_near_limit());

        // `usage_window` is null without a window; the limits alone still say there is one.
        let mut limits_only = free_with_window(0);
        limits_only["usage_window"] = Value::Null;
        let window = parse_account(&limits_only).window().expect("a window");
        assert_eq!((window.limit_tokens, window.used()), (1_000_000, None));
        assert_eq!(window.summary(), "Free ?/1M");

        assert_eq!(parse_account(&sample()).window(), None, "Pro has no window");
    }

    #[test]
    fn the_window_decides_the_near_limit_warning() {
        let report = render_usage(&parse_account(&free_with_window(640_000)), 1_792_195_200);
        assert!(
            report.contains("Token window     Free 640K/1M, resets 14:20 UTC\n"),
            "the compact form: {report}"
        );
        assert!(
            report.contains("Window tokens    640,000 / 1,000,000 (5-hour window)"),
            "{report}"
        );
        assert!(
            report.contains("Window resets    2026-10-17 14:20 UTC"),
            "{report}"
        );
        assert!(!report.contains("Warning"), "{report}");

        let near = parse_account(&free_with_window(950_000));
        assert!(near.near_limit());
        let report = render_usage(&near, 0);
        assert!(
            report.contains("Warning          50,000 window tokens left"),
            "{report}"
        );

        // Past `resets_at` what is left is not known until the next read: no figure, no
        // reset time and no warning from the old read.
        let after_reset = 1_792_246_800 + 60;
        let report = render_usage(&near, after_reset);
        assert!(report.contains("Token window     Free ?/1M\n"), "{report}");
        assert!(
            report.contains("Window tokens    ? / 1,000,000"),
            "{report}"
        );
        assert!(!report.contains("Window resets"), "{report}");
        assert!(!report.contains("Warning"), "{report}");
        let window = near.window().expect("a window");
        assert!(!window.is_past_reset(after_reset - 120));
        assert_eq!(window.as_of(after_reset - 120), window);
        assert!(window.is_past_reset(after_reset));

        // Under a window the daily token figure refuses nothing: no warning from it.
        let mut daily = free_with_window(10_000);
        daily["usage_today"]["tokens"] = serde_json::json!(4_900_000);
        assert!(!parse_account(&daily).near_limit());
    }

    #[test]
    fn token_counts_shorten_for_a_status_line() {
        assert_eq!(short_tokens(999), "999");
        assert_eq!(short_tokens(12_345), "12K");
        assert_eq!(short_tokens(640_000), "640K");
        assert_eq!(short_tokens(1_000_000), "1M");
        assert_eq!(short_tokens(1_500_000), "1.5M");
        assert_eq!(short_tokens(12_400_000), "12M");
    }

    #[test]
    fn usage_json_carries_the_account_summary() {
        let report = usage_json(&parse_account(&sample())).expect("serializes");
        assert_eq!(report["type"], "usage");
        assert_eq!(report["account"]["plan_id"], "pro");
        assert_eq!(report["account"]["requests_today"], 37);
        assert_eq!(report["account"]["daily_tokens_limit"], 10_000_000);
        assert_eq!(report["account"]["credit_balance_usd"], 7.42);
        assert!(report["upgrade_url"].is_string());
    }

    #[test]
    fn a_login_that_expired_here_is_signed_out_without_asking_the_gateway() {
        let mut credential = crate::auth::GatewayCredential {
            token: SecureString::new("ses_test".to_string()),
            kind: crate::credentials::TokenKind::Session,
            source: crate::auth::CredentialSource::EnvToken,
            expires_at: Some(1_000),
            plan: None,
            email: None,
            name: None,
        };
        let error = signed_in(&credential, 1_000).expect_err("expired");
        assert!(
            error.starts_with("Signed out of Zero-X: the session expired"),
            "{error}"
        );
        assert!(signed_in(&credential, 999).is_ok());
        credential.expires_at = None;
        assert!(signed_in(&credential, u64::MAX).is_ok());
    }

    #[test]
    fn digits_are_grouped_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(10_000_000), "10,000,000");
    }
}
