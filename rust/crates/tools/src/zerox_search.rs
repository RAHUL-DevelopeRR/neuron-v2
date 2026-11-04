//! `WebSearch` through the Zero-X gateway (`POST /v1/search`) for signed-in
//! users: results with snippets, a daily allowance per plan.
//!
//! Contract (shared with the gateway): the body is `{"query", "count",
//! "purpose", "freshness"?}`; 200 answers `{"results": [{"title", "url",
//! "snippet", "published"?}], "provider", "cached", "quota": {"remaining",
//! "limit", "resets_at"}}`; 429 is `{"error": "search_quota_exhausted",
//! "resets_at"}`; 503 `{"error": "search_unavailable"}` when the gateway has
//! no search backend; 401 means the session is no longer valid. The full
//! contract, quotas included, is in `docs/GATEWAY_CONTRACT.md` section 8.

use std::sync::{OnceLock, PoisonError, RwLock};
use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Results asked for (the gateway accepts 1 to 10).
pub(crate) const DEFAULT_COUNT: u8 = 8;
/// Longest query the gateway accepts, in characters.
pub(crate) const MAX_QUERY_CHARS: usize = 400;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Where signed-in searches go and the credential that pays for them.
#[derive(Clone)]
pub struct SearchGateway {
    /// The full `/v1/search` URL (`https://zero-x.live/v1/search`).
    pub endpoint: String,
    /// The Neuron session token or `nzx_` key, sent as a bearer token.
    pub token: String,
}

impl std::fmt::Debug for SearchGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchGateway")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

type Provider = Box<dyn Fn() -> Option<SearchGateway> + Send + Sync>;

fn provider_slot() -> &'static RwLock<Option<Provider>> {
    static SLOT: OnceLock<RwLock<Option<Provider>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Installs what `WebSearch` asks for the gateway and credential at each
/// call; `None` from it means signed out, and the search goes direct. The CLI
/// installs one that reads the current Zero-X login.
pub fn set_search_gateway_provider(
    provider: impl Fn() -> Option<SearchGateway> + Send + Sync + 'static,
) {
    *provider_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(provider));
}

/// The gateway for this call, or `None` when signed out (or none installed).
pub(crate) fn current_gateway() -> Option<SearchGateway> {
    provider_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|provider| provider())
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchRequest<'a> {
    pub query: &'a str,
    pub count: u8,
    pub purpose: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct GatewayHit {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Quota {
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayResults {
    pub hits: Vec<GatewayHit>,
    pub provider: String,
    pub cached: bool,
    pub quota: Option<Quota>,
}

/// What a gateway search came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GatewayOutcome {
    Results(GatewayResults),
    /// 429: today's allowance is used up.
    QuotaExhausted {
        resets_at: Option<String>,
    },
    /// 503: the gateway has no search backend; the caller searches direct.
    Unavailable,
    /// 401: the session is no longer valid.
    SignedOut,
}

/// Checks a query against the contract before it leaves the machine.
pub(crate) fn validate_query(query: &str) -> Result<(), String> {
    let chars = query.trim().chars().count();
    if chars == 0 {
        return Err("WebSearch needs a query".to_string());
    }
    if chars > MAX_QUERY_CHARS {
        return Err(format!(
            "WebSearch query is {chars} characters; the limit is {MAX_QUERY_CHARS}. Shorten it."
        ));
    }
    Ok(())
}

/// Accepted `freshness` values.
pub(crate) fn validate_freshness(freshness: Option<&str>) -> Result<(), String> {
    match freshness {
        None | Some("day" | "week" | "month" | "year") => Ok(()),
        Some(other) => Err(format!(
            "WebSearch freshness must be day, week, month or year, not `{other}`"
        )),
    }
}

/// The header naming a research run: the gateway counts each distinct id
/// once against the plan's daily research runs.
pub(crate) const RESEARCH_RUN_HEADER: &str = "x-neuron-research-run";

/// `POST` the query to the gateway.
pub(crate) fn search(
    gateway: &SearchGateway,
    request: &SearchRequest<'_>,
) -> Result<GatewayOutcome, String> {
    search_in_run(gateway, request, None)
}

/// [`search`] as part of research run `run_id` (sent as
/// [`RESEARCH_RUN_HEADER`]; the request's purpose should be `research`).
pub(crate) fn search_in_run(
    gateway: &SearchGateway,
    request: &SearchRequest<'_>,
    run_id: Option<&str>,
) -> Result<GatewayOutcome, String> {
    let client = Client::builder()
        .timeout(TIMEOUT)
        // Never forward the bearer token to another host.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("neuron/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut post = client
        .post(&gateway.endpoint)
        .bearer_auth(&gateway.token)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if let Some(run_id) = run_id {
        post = post.header(RESEARCH_RUN_HEADER, run_id);
    }
    let response = post
        .body(serde_json::to_vec(request).map_err(|error| error.to_string())?)
        .send()
        .map_err(|error| format!("Web search through Zero-X failed: {error}"))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    parse_response(status, &body)
}

/// The outcome of an HTTP `status` with `body` (split out for tests).
pub(crate) fn parse_response(status: u16, body: &str) -> Result<GatewayOutcome, String> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let resets_at = || {
        value
            .get("resets_at")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match status {
        200 => {
            let hits = value
                .get("results")
                .cloned()
                .map(serde_json::from_value::<Vec<GatewayHit>>)
                .transpose()
                .map_err(|error| format!("unexpected search response from Zero-X: {error}"))?
                .ok_or_else(|| "unexpected search response from Zero-X: no results".to_string())?;
            let quota = value
                .get("quota")
                .cloned()
                .and_then(|quota| serde_json::from_value::<Quota>(quota).ok());
            Ok(GatewayOutcome::Results(GatewayResults {
                hits,
                provider: value
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("zerox")
                    .to_string(),
                cached: value
                    .get("cached")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                quota,
            }))
        }
        401 => Ok(GatewayOutcome::SignedOut),
        429 => Ok(GatewayOutcome::QuotaExhausted {
            resets_at: resets_at(),
        }),
        503 => Ok(GatewayOutcome::Unavailable),
        other => Err(format!(
            "Web search through Zero-X failed (HTTP {other}){}",
            value
                .get("error")
                .and_then(Value::as_str)
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        )),
    }
}

/// The tool error for a used-up allowance.
pub(crate) fn quota_message(resets_at: Option<&str>) -> String {
    match resets_at {
        Some(at) => format!(
            "Web search allowance for today is used up; it resets at {at}. Do not retry WebSearch before then: answer from what you have, or ask the user."
        ),
        None => "Web search allowance for today is used up. Do not retry WebSearch today: answer from what you have, or ask the user.".to_string(),
    }
}

/// The tool error for a session the gateway no longer accepts.
pub(crate) const SIGNED_OUT_MESSAGE: &str =
    "Web search needs a valid Zero-X sign-in: the session was not accepted. Sign in again with `neuron auth login`.";

/// The quota as JSON for the tool output.
pub(crate) fn quota_json(quota: &Quota) -> Value {
    json!({
        "remaining": quota.remaining,
        "limit": quota.limit,
        "resets_at": quota.resets_at,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_response, quota_message, validate_freshness, validate_query, GatewayOutcome,
    };

    #[test]
    fn parses_results_with_snippets_and_quota() {
        let body = r#"{"results":[{"title":"Rust 1.91","url":"https://blog.rust-lang.org/","snippet":"Released today","published":"2026-10-30"},{"title":"No snippet","url":"https://example.com"}],"provider":"searxng","cached":true,"quota":{"remaining":39,"limit":40,"resets_at":"2026-11-05T00:00:00Z"}}"#;
        let GatewayOutcome::Results(results) = parse_response(200, body).unwrap() else {
            panic!("expected results");
        };
        assert_eq!(results.hits.len(), 2);
        assert_eq!(results.hits[0].snippet, "Released today");
        assert_eq!(results.hits[0].published.as_deref(), Some("2026-10-30"));
        assert_eq!(results.hits[1].snippet, "");
        assert_eq!(results.provider, "searxng");
        assert!(results.cached);
        assert_eq!(results.quota.unwrap().remaining, Some(39));
    }

    #[test]
    fn maps_error_statuses() {
        assert_eq!(
            parse_response(
                429,
                r#"{"error":"search_quota_exhausted","resets_at":"2026-11-05T00:00:00Z"}"#
            )
            .unwrap(),
            GatewayOutcome::QuotaExhausted {
                resets_at: Some("2026-11-05T00:00:00Z".to_string())
            }
        );
        assert_eq!(
            parse_response(503, r#"{"error":"search_unavailable"}"#).unwrap(),
            GatewayOutcome::Unavailable
        );
        assert_eq!(parse_response(401, "").unwrap(), GatewayOutcome::SignedOut);
        let error = parse_response(500, r#"{"error":"boom"}"#).unwrap_err();
        assert!(
            error.contains("HTTP 500") && error.contains("boom"),
            "{error}"
        );
        assert!(parse_response(200, "not json").is_err());
    }

    #[test]
    fn checks_the_query_and_freshness() {
        assert!(validate_query("rust").is_ok());
        assert!(validate_query("   ").is_err());
        assert!(validate_query(&"x".repeat(400)).is_ok());
        let error = validate_query(&"x".repeat(401)).unwrap_err();
        assert!(error.contains("limit is 400"), "{error}");
        assert!(validate_freshness(None).is_ok());
        assert!(validate_freshness(Some("week")).is_ok());
        assert!(validate_freshness(Some("hour")).is_err());
        assert!(
            quota_message(Some("2026-11-05T00:00:00Z")).contains("resets at 2026-11-05T00:00:00Z")
        );
    }
}
