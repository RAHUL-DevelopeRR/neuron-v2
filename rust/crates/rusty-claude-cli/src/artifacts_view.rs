//! `/artifacts`: what the session's artifact store holds ([`runtime::artifacts`]).

use runtime::artifacts::{format_listing, ArtifactStore};

/// The listing of `session_id`'s artifacts, or why there is none.
pub(crate) fn artifacts_report(session_id: &str) -> String {
    match ArtifactStore::open_default(session_id) {
        Ok(store) => format_listing(&store),
        Err(error) => format!("Artifacts\n  Unavailable: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::artifacts_report;

    #[test]
    fn an_invalid_session_says_why() {
        assert!(artifacts_report("../x").contains("Unavailable"));
    }
}
