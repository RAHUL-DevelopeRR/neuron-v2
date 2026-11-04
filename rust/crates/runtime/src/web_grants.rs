//! Run-only web grants from the command line: `--allow-web` lets `WebSearch`
//! and `WebFetch` run without a prompt for this run, and `--allow-origin
//! <host>` (repeatable) limits `WebFetch` and the browser to those hosts and
//! lets fetches from them run without a prompt.
//!
//! The grants live only in this process: they are never written to a
//! settings file, so a later run asks again.

use std::sync::{OnceLock, PoisonError, RwLock};

/// What `--allow-web` and `--allow-origin` granted for this run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunWebGrants {
    /// `--allow-web`: web searches and fetches run without a prompt.
    pub allow_web: bool,
    /// `--allow-origin` hosts, lowercase, without scheme, port or path. A
    /// leading `*.` also matches every subdomain. Empty means no limit.
    pub origins: Vec<String>,
}

impl RunWebGrants {
    /// Whether nothing was granted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.allow_web && self.origins.is_empty()
    }

    /// Adds an `--allow-origin` value (`example.com`, `https://example.com/`,
    /// `*.example.com`).
    pub fn add_origin(&mut self, raw: &str) -> Result<(), String> {
        let origin = parse_origin(raw)?;
        if !self.origins.contains(&origin) {
            self.origins.push(origin);
        }
        Ok(())
    }

    /// Whether `host` is reachable in this run: always when no
    /// `--allow-origin` was given, else only the listed hosts.
    #[must_use]
    pub fn origin_allowed(&self, host: &str) -> bool {
        self.origins.is_empty() || self.origin_listed(host)
    }

    fn origin_listed(&self, host: &str) -> bool {
        let host = normalize_host(host);
        self.origins.iter().any(|origin| {
            origin.strip_prefix("*.").map_or_else(
                || *origin == host,
                |base| host == base || host.ends_with(&format!(".{base}")),
            )
        })
    }

    /// Whether a fetch from `host` runs without a prompt: the host is listed
    /// with `--allow-origin`, or `--allow-web` was given and no list limits it.
    #[must_use]
    pub fn grants_fetch(&self, host: &str) -> bool {
        if self.origins.is_empty() {
            self.allow_web
        } else {
            self.origin_listed(host)
        }
    }

    /// Whether web searches run without a prompt (`--allow-web`).
    #[must_use]
    pub fn grants_search(&self) -> bool {
        self.allow_web
    }

    /// Why a fetch from `host` is refused in this run, or `None` when it is
    /// allowed.
    #[must_use]
    pub fn refusal_for(&self, tool: &str, host: &str) -> Option<String> {
        if self.origin_allowed(host) {
            return None;
        }
        Some(format!(
            "{tool} refused `{host}`: this run only reaches the hosts given with --allow-origin ({})",
            self.origins.join(", ")
        ))
    }
}

fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        return inner.to_string();
    }
    host
}

/// The host of an `--allow-origin` value.
fn parse_origin(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let invalid = || {
        format!(
            "invalid value for --allow-origin: `{raw}`; give a host such as example.com or https://example.com"
        )
    };
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let lowered = trimmed.to_ascii_lowercase();
    let wildcard = lowered.starts_with("*.");
    let host = if wildcard {
        lowered[2..].to_string()
    } else if lowered.starts_with("http://") || lowered.starts_with("https://") {
        crate::url_host(&lowered).ok_or_else(invalid)?
    } else {
        crate::url_host(&format!("https://{lowered}")).ok_or_else(invalid)?
    };
    let host = normalize_host(&host);
    if host.is_empty()
        || host == "*"
        || host
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':')))
    {
        return Err(invalid());
    }
    Ok(if wildcard { format!("*.{host}") } else { host })
}

fn slot() -> &'static RwLock<RunWebGrants> {
    static SLOT: OnceLock<RwLock<RunWebGrants>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(RunWebGrants::default()))
}

/// Sets this process's run grants (the CLI does, once, from its flags).
pub fn set_run_web_grants(grants: RunWebGrants) {
    *slot().write().unwrap_or_else(PoisonError::into_inner) = grants;
}

/// This process's run grants.
#[must_use]
pub fn run_web_grants() -> RunWebGrants {
    slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::RunWebGrants;

    fn with_origins(origins: &[&str]) -> RunWebGrants {
        let mut grants = RunWebGrants::default();
        for origin in origins {
            grants.add_origin(origin).unwrap();
        }
        grants
    }

    #[test]
    fn origins_parse_from_hosts_and_urls() {
        let grants = with_origins(&[
            "https://Example.com/path?q=1",
            "docs.rs",
            "*.github.com",
            "http://localhost:8080",
            "example.com",
        ]);
        assert_eq!(
            grants.origins,
            vec!["example.com", "docs.rs", "*.github.com", "localhost"]
        );
        for bad in ["", "   ", "*", "exa mple.com", "https://"] {
            assert!(
                RunWebGrants::default().add_origin(bad).is_err(),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn origins_limit_hosts_and_grant_fetches_from_them() {
        let grants = with_origins(&["example.com", "*.github.com"]);
        assert!(grants.origin_allowed("EXAMPLE.com"));
        assert!(grants.origin_allowed("api.github.com"));
        assert!(grants.origin_allowed("github.com"));
        assert!(!grants.origin_allowed("sub.example.com"));
        assert!(!grants.origin_allowed("evil.com"));
        assert!(grants.grants_fetch("example.com"));
        assert!(!grants.grants_fetch("evil.com"));
        assert!(
            !grants.grants_search(),
            "--allow-origin alone does not allow searches"
        );
        let refusal = grants.refusal_for("WebFetch", "evil.com").unwrap();
        assert!(
            refusal.contains("--allow-origin (example.com, *.github.com)"),
            "{refusal}"
        );
        assert_eq!(grants.refusal_for("WebFetch", "example.com"), None);
    }

    #[test]
    fn allow_web_grants_searches_and_fetches_everywhere_unless_origins_limit_it() {
        let mut grants = RunWebGrants {
            allow_web: true,
            origins: Vec::new(),
        };
        assert!(grants.grants_search());
        assert!(grants.grants_fetch("anything.example"));
        assert!(grants.origin_allowed("anything.example"));
        grants.add_origin("docs.rs").unwrap();
        assert!(grants.grants_fetch("docs.rs"));
        assert!(!grants.grants_fetch("anything.example"));
        assert!(!grants.origin_allowed("anything.example"));
    }

    #[test]
    fn no_grants_change_nothing() {
        let grants = RunWebGrants::default();
        assert!(grants.is_empty());
        assert!(!grants.grants_search());
        assert!(!grants.grants_fetch("example.com"));
        assert!(grants.origin_allowed("example.com"));
    }
}
