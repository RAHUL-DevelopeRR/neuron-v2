//! The sandbox network allowlist (`"sandbox": {"network": {"allow": [...]}}`).
//!
//! Entries are host names or IP literals, matched without case and without a
//! trailing dot: `github.com` matches only that host; `*.npmjs.org` matches
//! any subdomain (`registry.npmjs.org`, `a.b.npmjs.org`) but not `npmjs.org`
//! itself; `*` matches every host. An entry may carry a port
//! (`localhost:8080`), which then has to match too.
//!
//! Where it is enforced:
//! - `WebFetch`: always, before the request is made.
//! - Shell commands: they get `HTTP(S)_PROXY`/`ALL_PROXY` pointing at a
//!   loopback proxy ([`crate::net_proxy`]) that only tunnels to allowed hosts.
//!   On macOS the seatbelt profile also denies every other outbound
//!   connection, so the proxy is the only way out (enforced). On Linux and
//!   Windows the proxy variables are advisory: a program that ignores them,
//!   or opens raw sockets, is not stopped. (Linux `networkIsolation` cuts all
//!   network instead; the loopback proxy is unreachable from a fresh network
//!   namespace.)

/// A parsed allowlist. `None` in config means "no allowlist configured".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkAllowlist {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    host: HostPattern,
    port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostPattern {
    Any,
    Exact(String),
    /// `*.suffix`: strict subdomains of `suffix`.
    Subdomains(String),
}

fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    host.to_ascii_lowercase()
}

/// Splits `host[:port]` (and `[v6]:port`); a bare IPv6 literal has no port.
fn split_host_port(text: &str) -> (&str, Option<u16>) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[') {
        if let Some((host, after)) = rest.split_once(']') {
            let port = after.strip_prefix(':').and_then(|port| port.parse().ok());
            return (host, port);
        }
    }
    if text.matches(':').count() == 1 {
        if let Some((host, port)) = text.rsplit_once(':') {
            if let Ok(port) = port.parse() {
                return (host, Some(port));
            }
        }
    }
    (text, None)
}

impl NetworkAllowlist {
    /// Parses config entries; blank entries are ignored.
    #[must_use]
    pub fn new<S: AsRef<str>>(entries: &[S]) -> Self {
        let entries = entries
            .iter()
            .filter_map(|entry| {
                let entry = entry.as_ref().trim();
                // Tolerate a scheme or path copied from a URL.
                let entry = entry.split_once("://").map_or(entry, |(_, rest)| rest);
                let entry = entry.split('/').next().unwrap_or(entry);
                if entry.is_empty() {
                    return None;
                }
                let (host, port) = split_host_port(entry);
                let host = normalize_host(host);
                let host = if host == "*" {
                    HostPattern::Any
                } else if let Some(suffix) = host.strip_prefix("*.") {
                    HostPattern::Subdomains(suffix.to_string())
                } else {
                    HostPattern::Exact(host)
                };
                Some(Entry { host, port })
            })
            .collect();
        Self { entries }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a connection to `host` (and `port`, when known) is allowed.
    #[must_use]
    pub fn allows(&self, host: &str, port: Option<u16>) -> bool {
        let host = normalize_host(host);
        if host.is_empty() {
            return false;
        }
        self.entries.iter().any(|entry| {
            let host_ok = match &entry.host {
                HostPattern::Any => true,
                HostPattern::Exact(exact) => *exact == host,
                HostPattern::Subdomains(suffix) => host
                    .strip_suffix(suffix.as_str())
                    .is_some_and(|stem| stem.len() > 1 && stem.ends_with('.')),
            };
            host_ok && entry.port.is_none_or(|wanted| port == Some(wanted))
        })
    }

    /// Whether `host:port` text (as in an HTTP `CONNECT` line) is allowed.
    #[must_use]
    pub fn allows_authority(&self, authority: &str) -> bool {
        let (host, port) = split_host_port(authority);
        self.allows(host, port)
    }
}

/// Environment that sends proxy-aware programs through the loopback proxy at
/// `proxy` (`127.0.0.1:port`). Both cases are set: curl reads lowercase,
/// many others uppercase. `NO_PROXY` is cleared so nothing bypasses it.
#[must_use]
pub fn proxy_env(proxy: std::net::SocketAddr) -> Vec<(String, String)> {
    let url = format!("http://{proxy}");
    let mut env = Vec::new();
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        env.push((name.to_string(), url.clone()));
        env.push((name.to_ascii_lowercase(), url.clone()));
    }
    env.push(("NO_PROXY".to_string(), String::new()));
    env.push(("no_proxy".to_string(), String::new()));
    env
}

#[cfg(test)]
mod tests {
    use super::{proxy_env, NetworkAllowlist};

    #[test]
    fn exact_hosts_match_without_case_or_trailing_dot() {
        let list = NetworkAllowlist::new(&["github.com"]);
        assert!(list.allows("github.com", Some(443)));
        assert!(list.allows("GitHub.COM.", None));
        assert!(!list.allows("api.github.com", None));
        assert!(!list.allows("evilgithub.com", None));
        assert!(!list.allows("github.com.evil.io", None));
    }

    #[test]
    fn wildcard_matches_strict_subdomains_only() {
        let list = NetworkAllowlist::new(&["*.npmjs.org"]);
        assert!(list.allows("registry.npmjs.org", None));
        assert!(list.allows("a.b.npmjs.org", None));
        assert!(!list.allows("npmjs.org", None));
        assert!(!list.allows("evilnpmjs.org", None));
        assert!(!list.allows("npmjs.org.evil.io", None));
    }

    #[test]
    fn ports_ips_star_and_url_like_entries() {
        let list = NetworkAllowlist::new(&[
            "localhost:8080",
            "10.0.0.5",
            "[::1]:9000",
            "https://crates.io/some/path",
            "  ",
        ]);
        assert!(list.allows("localhost", Some(8080)));
        assert!(!list.allows("localhost", Some(8081)));
        assert!(!list.allows("localhost", None));
        assert!(list.allows("10.0.0.5", Some(22)));
        assert!(list.allows_authority("[::1]:9000"));
        assert!(!list.allows_authority("[::1]:9001"));
        assert!(list.allows_authority("crates.io:443"));
        assert!(!list.allows("", None));
        assert!(NetworkAllowlist::new(&["*"]).allows("anything.example", Some(1)));
        assert!(NetworkAllowlist::new::<&str>(&[]).is_empty());
        assert!(!NetworkAllowlist::new::<&str>(&[]).allows("github.com", None));
    }

    #[test]
    fn proxy_env_sets_both_cases_and_clears_no_proxy() {
        let env = proxy_env("127.0.0.1:4567".parse().expect("addr"));
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:4567"));
        assert_eq!(get("https_proxy"), Some("http://127.0.0.1:4567"));
        assert_eq!(get("NO_PROXY"), Some(""));
    }
}
