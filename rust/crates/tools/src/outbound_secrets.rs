//! The outbound secret check: a web search query or a fetch URL that holds
//! something credential-shaped is refused before it leaves the machine,
//! whatever the permission rules or a domain approval say. This closes the
//! path where text the model read (a page, a file) talks it into searching for
//! a key, or into fetching an approved site with a token in the URL.
//!
//! It uses the redaction patterns (provider keys, GitHub and Slack tokens,
//! JWTs, `Bearer` values, passwords in URLs, `api_key=...` assignments, PEM
//! blocks) and, while redaction is on, the exact values of credential
//! environment variables and registered secrets. With `redactSecrets: false`
//! the patterns still apply.

use std::borrow::Cow;

/// Why `text` (the `what` of a `tool` call: a query, an address) is refused;
/// `None` when it holds nothing credential-shaped. An address's query
/// parameters are also checked one by one, decoded, as `name=value`.
pub(crate) fn refusal(tool: &str, what: &str, text: &str) -> Option<String> {
    let mut pieces = vec![text.to_string()];
    if let Ok(url) = reqwest::Url::parse(text) {
        pieces.extend(
            url.query_pairs()
                .map(|(name, value)| format!("{name}={value}")),
        );
    }
    pieces.iter().any(|piece| looks_secret(piece)).then(|| {
        format!(
            "{tool} refused: the {what} looks like a secret (a key, token or password), so it was not sent. Ask the user instead of sending credentials to the web."
        )
    })
}

fn looks_secret(text: &str) -> bool {
    let checked = if runtime::redaction::is_enabled() {
        runtime::redaction::redact(text)
    } else {
        runtime::redaction::redact_with(text, &[])
    };
    matches!(checked, Cow::Owned(_))
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn credential_shaped_queries_and_urls_are_refused() {
        let refused = refusal(
            "WebSearch",
            "query",
            "ghp_0123456789abcdefABCDEF0123456789abcd leaked",
        )
        .expect("a GitHub token is refused");
        assert!(refused.starts_with("WebSearch refused: the query looks like a secret"));
        assert!(refusal(
            "WebFetch",
            "address",
            "https://user:hunter2pass@example.com/"
        )
        .is_some());
        assert!(refusal(
            "WebFetch",
            "address",
            "https://example.com/?api_key=AbCdEf0123456789xyz"
        )
        .is_some());
        // Ordinary queries and addresses, commit hashes included, pass.
        assert_eq!(refusal("WebSearch", "query", "tokio vs smol 2026"), None);
        assert_eq!(
            refusal(
                "WebFetch",
                "address",
                "https://github.com/tokio-rs/tokio/commit/0a1b2c3d4e5f60718293a4b5c6d7e8f901234567"
            ),
            None
        );
    }
}
