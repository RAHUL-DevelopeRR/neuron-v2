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
    pub credit_balance_usd: Option<f64>,
    pub monthly_grant_usd: Option<f64>,
    pub subscription_status: Option<String>,
    pub current_period_end: Option<String>,
    pub cancel_at_period_end: bool,
    pub suspended: bool,
}

impl AccountSummary {
    fn is_free(&self) -> bool {
        self.plan_id.as_deref().is_none_or(|plan| plan == "free")
    }

    fn near_daily_limit(&self) -> bool {
        let near = |used: Option<u64>, limit: Option<u64>| match (used, limit) {
            (Some(used), Some(limit)) if limit > 0 => {
                used.saturating_mul(10) >= limit.saturating_mul(9)
            }
            _ => false,
        };
        near(self.requests_today, self.daily_requests_limit)
            || near(self.tokens_today, self.daily_tokens_limit)
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
    Ok(parse_account(&value))
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
    if account.is_free() || account.near_daily_limit() {
        lines.push(format!("  Upgrade          {} (/upgrade)", pricing_url()));
    }
    lines
}

/// Full `/usage` report.
pub fn render_usage(account: &AccountSummary, now: u64) -> String {
    let mut lines = vec!["Zero-X usage".to_string()];
    lines.extend(usage_lines(account, now));
    lines.join("\n")
}

/// Fetches the account of the active Zero-X credential.
fn current_account() -> Result<AccountSummary, Box<dyn std::error::Error>> {
    let base = crate::auth::gateway_base_url()?;
    let credential = crate::auth::current_credential()?.ok_or(
        "Not signed in to Zero-X. Run `neuron auth login` (or set NEURON_TOKEN) to see usage.",
    )?;
    Ok(fetch_account(&base, &credential.token)?)
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
    fn digits_are_grouped_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(10_000_000), "10,000,000");
    }
}
