//! Web searches and page reads for research runs (`/research`, `neuron
//! research`). They go through the same gateway, SSRF guard, sandbox
//! allow-list and `--allow-origin` limits as `WebSearch` and `WebFetch`, but
//! hand plain data to the research driver instead of tool output for the
//! model. The driver wraps page text with [`wrap_external`] before any model
//! reads it.
//!
//! Signed in, a search is `POST /v1/search` with `purpose: "research"` and the
//! run's id in the `x-neuron-research-run` header, so the gateway counts the
//! run once against the plan's daily research runs (Free 3, Pro 30, Max 150).

use std::fmt;

use crate::web_fetch;
use crate::zerox_search::{self, GatewayOutcome, SearchGateway, SearchRequest};

/// The `purpose` research searches send.
pub const RESEARCH_PURPOSE: &str = "research";

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchHit {
    pub title: String,
    pub url: String,
    /// From the search provider; empty on the direct search.
    pub snippet: String,
    pub published: Option<String>,
}

/// What the gateway said is left of today's searches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResearchQuota {
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at: Option<String>,
}

/// The results of one research search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchSearch {
    pub hits: Vec<ResearchHit>,
    /// `zerox:<provider>`, `duckduckgo`, or the custom search host.
    pub backend: String,
    pub quota: Option<ResearchQuota>,
}

/// Why a research search returned nothing to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchSearchError {
    /// HTTP 429: today's searches or research runs are used up.
    QuotaExhausted { resets_at: Option<String> },
    /// HTTP 401: the Zero-X sign-in is no longer accepted.
    SignedOut,
    /// Anything else (network, bad query, blocked direct search).
    Failed(String),
}

impl fmt::Display for ResearchSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuotaExhausted {
                resets_at: Some(at),
            } => {
                write!(
                    f,
                    "today's web search allowance is used up (resets at {at})"
                )
            }
            Self::QuotaExhausted { resets_at: None } => {
                write!(f, "today's web search allowance is used up")
            }
            Self::SignedOut => write!(
                f,
                "the Zero-X sign-in was not accepted; sign in again with `neuron auth login`"
            ),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// A page read for research: its readable text, cut to the caller's limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchPage {
    /// Where the text came from, after same-host redirects.
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    /// Characters of readable text before the cut.
    pub total_chars: usize,
    /// Bytes of the response body that were read.
    pub bytes: usize,
}

/// Searches for `query` as part of research run `run_id`, asking for `count`
/// results (1 to 10). Signed in, through the gateway; signed out, or when the
/// gateway has no search backend (HTTP 503), straight to the search host as
/// `WebSearch` does.
pub fn research_search(
    query: &str,
    run_id: &str,
    count: u8,
) -> Result<ResearchSearch, ResearchSearchError> {
    let gateway = if super::custom_search_base_url().is_some() {
        None
    } else {
        zerox_search::current_gateway()
    };
    research_search_with(query, run_id, count, gateway.as_ref())
}

fn research_search_with(
    query: &str,
    run_id: &str,
    count: u8,
    gateway: Option<&SearchGateway>,
) -> Result<ResearchSearch, ResearchSearchError> {
    zerox_search::validate_query(query).map_err(ResearchSearchError::Failed)?;
    if let Some(refusal) = crate::outbound_secrets::refusal("WebSearch", "query", query) {
        return Err(ResearchSearchError::Failed(refusal));
    }
    let count = count.clamp(1, 10);
    if let Some(gateway) = gateway {
        let request = SearchRequest {
            query: query.trim(),
            count,
            purpose: RESEARCH_PURPOSE,
            freshness: None,
        };
        match zerox_search::search_in_run(gateway, &request, Some(run_id))
            .map_err(ResearchSearchError::Failed)?
        {
            GatewayOutcome::Results(results) => {
                let mut hits = results
                    .hits
                    .into_iter()
                    .map(|hit| super::SearchHit {
                        title: hit.title,
                        url: hit.url,
                        snippet: hit.snippet,
                        published: hit.published,
                    })
                    .collect::<Vec<_>>();
                super::dedupe_hits(&mut hits);
                hits.truncate(usize::from(count));
                return Ok(ResearchSearch {
                    hits: hits.into_iter().map(research_hit).collect(),
                    backend: format!("zerox:{}", results.provider),
                    quota: results.quota.map(|quota| ResearchQuota {
                        remaining: quota.remaining,
                        limit: quota.limit,
                        resets_at: quota.resets_at,
                    }),
                });
            }
            GatewayOutcome::QuotaExhausted { resets_at } => {
                return Err(ResearchSearchError::QuotaExhausted { resets_at });
            }
            GatewayOutcome::SignedOut => return Err(ResearchSearchError::SignedOut),
            GatewayOutcome::Unavailable => {}
        }
    }
    let (mut hits, backend) =
        super::direct_search_hits(query, gateway.is_none()).map_err(ResearchSearchError::Failed)?;
    super::dedupe_hits(&mut hits);
    hits.truncate(usize::from(count));
    Ok(ResearchSearch {
        hits: hits.into_iter().map(research_hit).collect(),
        backend,
        quota: None,
    })
}

fn research_hit(hit: super::SearchHit) -> ResearchHit {
    ResearchHit {
        title: hit.title,
        url: hit.url,
        snippet: hit.snippet,
        published: hit.published,
    }
}

/// Reads `url` with `WebFetch`'s guards (no credential-shaped address, public
/// hosts only, same-host redirects, the sandbox allow-list, `--allow-origin`) and returns up to
/// `max_chars` characters of its readable text. A redirect to another host,
/// an error status and binary content are errors: there is nothing to read.
pub fn research_fetch(url: &str, max_chars: usize) -> Result<ResearchPage, String> {
    if let Some(refusal) = crate::outbound_secrets::refusal("WebFetch", "address", url) {
        return Err(refusal);
    }
    let request_url = super::normalize_fetch_url(url)?;
    let request_url = reqwest::Url::parse(&request_url).map_err(|error| error.to_string())?;
    web_fetch::ensure_gated_host(url, &request_url)?;
    web_fetch::ensure_allowlisted(&request_url)?;
    super::ensure_run_origin(&request_url)?;
    let page = web_fetch::fetch(request_url, super::ensure_run_origin)?;
    page_text(&page, max_chars)
}

fn page_text(page: &web_fetch::FetchedPage, max_chars: usize) -> Result<ResearchPage, String> {
    if let Some(target) = &page.cross_host_redirect {
        return Err(format!(
            "it redirects to another host ({target}), which was not followed"
        ));
    }
    if !page.status.is_success() {
        return Err(format!("HTTP {}", page.status.as_u16()));
    }
    if web_fetch::is_binary_content_type(&page.content_type) {
        return Err(format!("binary content ({} bytes)", page.bytes));
    }
    let content = super::normalize_fetched_content(&page.body, &page.content_type);
    let title = if page.content_type.contains("html") {
        super::extract_title("", &page.body, &page.content_type)
    } else {
        None
    };
    let total_chars = content.chars().count();
    if total_chars == 0 {
        return Err("no readable text".to_string());
    }
    Ok(ResearchPage {
        url: page.final_url.to_string(),
        title,
        text: content.chars().take(max_chars).collect(),
        total_chars,
        bytes: page.bytes,
    })
}

/// Wraps text from the web in the external-content envelope (`source` is
/// `web:<host>`), so a model reads it as data, never as instructions.
#[must_use]
pub fn wrap_external(source: &str, text: &str) -> String {
    crate::external_content::wrap(source, text)
}

#[cfg(test)]
mod tests {
    use super::{page_text, research_search_with, ResearchSearchError, RESEARCH_PURPOSE};
    use crate::web_fetch::FetchedPage;
    use crate::zerox_search::SearchGateway;
    use reqwest::StatusCode;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    /// A gateway answering every request with `status` and `body`; returns its
    /// `/v1/search` URL and the raw requests it got.
    fn mock_gateway(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock gateway");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("read timeout");
                let mut data = Vec::new();
                let mut buffer = [0_u8; 4096];
                while let Ok(size) = stream.read(&mut buffer) {
                    if size == 0 {
                        break;
                    }
                    data.extend_from_slice(&buffer[..size]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&data).to_string());
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}/v1/search"), requests)
    }

    fn gateway(endpoint: &str) -> SearchGateway {
        SearchGateway {
            endpoint: endpoint.to_string(),
            token: "ses_research_test".to_string(),
        }
    }

    const RESULTS: &str = r#"{"results":[{"title":"A","url":"https://a.example/x","snippet":"first"},{"title":"A again","url":"https://a.example/x"},{"title":"B","url":"https://b.example/y","snippet":"second","published":"2026-11-01"}],"provider":"searxng","cached":false,"quota":{"remaining":2,"limit":3,"resets_at":"2026-11-05T00:00:00Z"}}"#;

    #[test]
    fn searches_send_the_research_purpose_and_the_run_header() {
        let (endpoint, requests) = mock_gateway(200, RESULTS);
        let search = research_search_with(
            "rust async runtimes",
            "rr_0123",
            6,
            Some(&gateway(&endpoint)),
        )
        .expect("search");
        assert_eq!(search.backend, "zerox:searxng");
        // The duplicate URL is dropped.
        assert_eq!(search.hits.len(), 2);
        assert_eq!(search.hits[0].snippet, "first");
        assert_eq!(search.hits[1].published.as_deref(), Some("2026-11-01"));
        assert_eq!(search.quota.unwrap().remaining, Some(2));

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        let lowered = request.to_ascii_lowercase();
        assert!(
            lowered.contains("x-neuron-research-run: rr_0123"),
            "{request}"
        );
        assert!(
            lowered.contains("authorization: bearer ses_research_test"),
            "{request}"
        );
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"query": "rust async runtimes", "count": 6, "purpose": RESEARCH_PURPOSE})
        );
    }

    #[test]
    fn quota_and_sign_in_failures_stop_without_another_call() {
        let (endpoint, requests) = mock_gateway(
            429,
            r#"{"error":"search_quota_exhausted","resets_at":"2026-11-05T00:00:00Z"}"#,
        );
        let error = research_search_with("q", "rr_1", 8, Some(&gateway(&endpoint))).unwrap_err();
        assert_eq!(
            error,
            ResearchSearchError::QuotaExhausted {
                resets_at: Some("2026-11-05T00:00:00Z".to_string())
            }
        );
        assert!(error.to_string().contains("resets at 2026-11-05T00:00:00Z"));
        assert_eq!(requests.lock().unwrap().len(), 1);

        let (endpoint, _) = mock_gateway(401, "{}");
        assert_eq!(
            research_search_with("q", "rr_1", 8, Some(&gateway(&endpoint))).unwrap_err(),
            ResearchSearchError::SignedOut
        );
        assert!(matches!(
            research_search_with("  ", "rr_1", 8, None),
            Err(ResearchSearchError::Failed(_))
        ));
    }

    #[test]
    fn credential_shaped_queries_and_addresses_are_never_sent() {
        let (endpoint, requests) = mock_gateway(200, RESULTS);
        let error = research_search_with(
            "ghp_0123456789abcdefABCDEF0123456789abcd",
            "rr_1",
            8,
            Some(&gateway(&endpoint)),
        )
        .unwrap_err();
        assert!(
            matches!(&error, ResearchSearchError::Failed(message) if message.contains("looks like a secret")),
            "{error:?}"
        );
        assert!(requests.lock().unwrap().is_empty());
        let error =
            super::research_fetch("https://user:hunter2pass@example.com/", 100).unwrap_err();
        assert!(error.contains("looks like a secret"), "{error}");
    }

    fn page(status: u16, content_type: &str, body: &str) -> FetchedPage {
        FetchedPage {
            final_url: reqwest::Url::parse("https://docs.example/guide").unwrap(),
            status: StatusCode::from_u16(status).unwrap(),
            content_type: content_type.to_string(),
            body: body.to_string(),
            bytes: body.len(),
            body_truncated: false,
            cross_host_redirect: None,
        }
    }

    #[test]
    fn pages_give_their_title_and_text_cut_to_the_limit() {
        let html = "<html><head><title>The Guide</title></head><body><p>Alpha beta gamma delta.</p></body></html>";
        let read = page_text(&page(200, "text/html; charset=utf-8", html), 10).expect("page");
        assert_eq!(read.title.as_deref(), Some("The Guide"));
        assert_eq!(read.text.chars().count(), 10);
        assert!(read.total_chars > 10);
        assert_eq!(read.url, "https://docs.example/guide");

        assert_eq!(
            page_text(&page(404, "text/html", "<p>missing</p>"), 100).unwrap_err(),
            "HTTP 404"
        );
        assert!(page_text(&page(200, "application/pdf", "%PDF"), 100)
            .unwrap_err()
            .contains("binary"));
        assert!(page_text(&page(200, "text/plain", "   "), 100).is_err());
        let mut redirected = page(302, "", "");
        redirected.cross_host_redirect =
            Some(reqwest::Url::parse("https://other.example/").unwrap());
        assert!(page_text(&redirected, 100)
            .unwrap_err()
            .contains("other.example"));
    }

    #[test]
    fn fetches_of_private_addresses_are_refused() {
        let error = super::research_fetch("http://127.0.0.1:9/secret", 100).unwrap_err();
        assert!(!error.is_empty());
        let error =
            super::research_fetch("http://169.254.169.254/latest/meta-data", 100).unwrap_err();
        assert!(!error.is_empty());
    }
}
