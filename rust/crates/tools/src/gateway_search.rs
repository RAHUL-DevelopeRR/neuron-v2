//! `WebSearch` through the Zero-X gateway (`POST /v1/search`, platform contract section 17).
//!
//! When the user is signed in to Zero-X the search goes to the gateway with the same
//! sign-in the chat client uses, so it is metered against the plan's daily search budget and
//! served from `SearXNG` or `Brave`. The gateway's own origin is the only place the bearer is
//! sent, and neither the bearer nor the query is ever logged here.
//!
//! What happens on each answer:
//!
//! - `200`: the results, plus the remaining quota, are shown to the model.
//! - `429` / `402` (quota used up) and `403` (suspended): the search is refused with a clear
//!   message; the tool does not quietly scrape instead, so the budget means something.
//! - `503 search_unavailable`, any other server error, a network error, a rejected sign-in
//!   (`401`) or no sign-in at all: the caller falls back to the local `DuckDuckGo` scrape and
//!   marks the answer "local fallback".

use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{json, Value};

/// Freshness values the gateway accepts.
pub(crate) const FRESHNESS_VALUES: [&str; 4] = ["day", "week", "month", "year"];

/// Largest result count the gateway allows.
const MAX_COUNT: u32 = 10;

/// One result from the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayHit {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) snippet: Option<String>,
    pub(crate) published: Option<String>,
}

/// The search budget the gateway reported with a `200`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayQuota {
    pub(crate) remaining: u64,
    pub(crate) limit: u64,
    pub(crate) resets_at: Option<String>,
}

impl GatewayQuota {
    pub(crate) fn describe(&self) -> String {
        let reset = self
            .resets_at
            .as_deref()
            .map(|at| format!(", resets {at}"))
            .unwrap_or_default();
        format!(
            "Zero-X search quota: {} of {} searches left today{reset}.",
            self.remaining, self.limit
        )
    }
}

/// A successful gateway search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayAnswer {
    pub(crate) hits: Vec<GatewayHit>,
    pub(crate) quota: Option<GatewayQuota>,
    pub(crate) cached: bool,
    pub(crate) provider: Option<String>,
}

/// What the gateway attempt decided.
#[derive(Debug)]
pub(crate) enum GatewayOutcome {
    /// The gateway answered.
    Answer(GatewayAnswer),
    /// Use the local search instead, for this (secret-free) reason.
    Fallback(String),
    /// Stop: the search is refused and must not be scraped locally.
    Refused(String),
}

/// Asks the gateway for up to `count` results.
pub(crate) fn search(
    client: &Client,
    query: &str,
    freshness: Option<&str>,
    count: u32,
) -> GatewayOutcome {
    let Ok(token) = runtime::zerox_bearer_token() else {
        return GatewayOutcome::Fallback(
            "not signed in to Zero-X (run `neuron login`)".to_string(),
        );
    };
    let origin = match runtime::zerox_origin() {
        Ok(origin) => origin,
        Err(error) => return GatewayOutcome::Fallback(error),
    };

    let mut body = json!({ "query": query, "count": count.clamp(1, MAX_COUNT) });
    if let Some(freshness) = freshness {
        body["freshness"] = Value::String(freshness.to_string());
    }

    let response = client
        .post(format!("{origin}/v1/search"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .json(&body)
        .send();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            // The error text can name the URL; it never carries the bearer or the body.
            let kind = if error.is_timeout() {
                "timed out"
            } else {
                "could not be reached"
            };
            return GatewayOutcome::Fallback(format!("the Zero-X gateway {kind}"));
        }
    };
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    interpret(status, &text)
}

/// Turns the gateway's status and body into an outcome.
fn interpret(status: u16, text: &str) -> GatewayOutcome {
    let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    match status {
        200 => match parse_answer(&value) {
            Some(answer) => GatewayOutcome::Answer(answer),
            None => GatewayOutcome::Fallback("the Zero-X gateway answered unexpectedly".into()),
        },
        429 | 402 => GatewayOutcome::Refused(quota_message(&value)),
        403 => GatewayOutcome::Refused(
            "Zero-X web search is not available: this account is suspended (contact support@zero-x.live). Not falling back to a local scrape."
                .to_string(),
        ),
        401 => GatewayOutcome::Fallback(
            "Zero-X rejected the sign-in (run `neuron login` again)".to_string(),
        ),
        503 => GatewayOutcome::Fallback("Zero-X search is unavailable (503)".to_string()),
        other => GatewayOutcome::Fallback(format!("the Zero-X gateway answered HTTP {other}")),
    }
}

fn quota_message(value: &Value) -> String {
    let what = match value.get("exhausted").and_then(Value::as_str) {
        Some("research_runs") => "research runs",
        _ => "searches",
    };
    let reset = value
        .get("resets_at")
        .and_then(Value::as_str)
        .map(|at| format!(" It resets {at}."))
        .unwrap_or_default();
    format!(
        "Zero-X web search quota exhausted: today's {what} are used up.{reset} Not falling back to a local scrape; tell the user, who can wait for the reset or upgrade at https://zero-x.live/pricing."
    )
}

fn parse_answer(value: &Value) -> Option<GatewayAnswer> {
    let items = value.get("results")?.as_array()?;
    let text = |item: &Value, key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let hits = items
        .iter()
        .filter_map(|item| {
            let url = text(item, "url")?;
            Some(GatewayHit {
                title: text(item, "title").unwrap_or_else(|| url.clone()),
                url,
                snippet: text(item, "snippet"),
                published: text(item, "published"),
            })
        })
        .collect();
    let quota = value.get("quota").and_then(|quota| {
        Some(GatewayQuota {
            remaining: quota.get("remaining")?.as_u64()?,
            limit: quota.get("limit")?.as_u64()?,
            resets_at: quota
                .get("resets_at")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    });
    Some(GatewayAnswer {
        hits,
        quota,
        cached: value
            .get("cached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::{interpret, GatewayOutcome};

    #[test]
    fn a_200_without_a_results_array_falls_back() {
        assert!(matches!(
            interpret(200, r#"{"nope":1}"#),
            GatewayOutcome::Fallback(_)
        ));
        assert!(matches!(interpret(200, "x"), GatewayOutcome::Fallback(_)));
    }

    #[test]
    fn quota_and_suspension_refuse_while_503_401_and_5xx_fall_back() {
        assert!(matches!(interpret(429, "{}"), GatewayOutcome::Refused(_)));
        assert!(matches!(interpret(402, "{}"), GatewayOutcome::Refused(_)));
        assert!(matches!(interpret(403, "{}"), GatewayOutcome::Refused(_)));
        assert!(matches!(interpret(503, "{}"), GatewayOutcome::Fallback(_)));
        assert!(matches!(interpret(502, "{}"), GatewayOutcome::Fallback(_)));
        assert!(matches!(interpret(401, "{}"), GatewayOutcome::Fallback(_)));
    }

    #[test]
    fn the_quota_message_names_what_ran_out_and_when_it_resets() {
        let GatewayOutcome::Refused(message) = interpret(
            429,
            r#"{"error":"search_quota_exhausted","exhausted":"research_runs","resets_at":"2026-11-08T00:00:00.000Z"}"#,
        ) else {
            panic!("expected a refusal");
        };
        assert!(message.contains("research runs"), "{message}");
        assert!(message.contains("2026-11-08"), "{message}");
        assert!(message.contains("Not falling back"), "{message}");
    }
}
