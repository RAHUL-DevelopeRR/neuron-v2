//! `WebFetch` transport: SSRF-guarded HTTP(S) fetching and HTML-to-text
//! conversion.
//!
//! Every hop of a fetch is validated before a connection is made: only
//! `http`/`https` URLs are accepted, and the host must not be (or resolve to)
//! a loopback, private, link-local or otherwise non-public address. The
//! connection is pinned to the addresses that were validated, so a DNS answer
//! cannot change between the check and the request, and redirects are
//! followed manually so each `Location` is validated the same way.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::{StatusCode, Url};

/// Characters of readable text returned to the model.
pub(crate) const MAX_FETCH_TEXT_CHARS: usize = 20_000;

/// Bytes read from a response body; the rest is not downloaded.
const MAX_FETCH_BODY_BYTES: u64 = 5 * 1024 * 1024;

const MAX_REDIRECTS: usize = 10;

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

const USER_AGENT: &str = "neuron-web-fetch/1.0";

/// A fetched response body plus the metadata the tool reports.
#[derive(Debug)]
pub(crate) struct FetchedPage {
    pub final_url: Url,
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
    pub bytes: usize,
    pub body_truncated: bool,
}

#[cfg(test)]
thread_local! {
    static ALLOW_LOOPBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lets fetches from the current test thread reach loopback addresses (the
/// local fixture server); private and link-local addresses stay blocked.
#[cfg(test)]
pub(crate) fn allow_loopback_on_this_thread() {
    ALLOW_LOOPBACK.with(|allow| allow.set(true));
}

#[cfg(test)]
fn loopback_allowed() -> bool {
    ALLOW_LOOPBACK.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn loopback_allowed() -> bool {
    false
}

/// Why `ip` must not be fetched from, or `None` for public addresses.
#[must_use]
pub(crate) fn blocked_address_reason(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_ipv4_reason(v4),
        IpAddr::V6(v6) => blocked_ipv6_reason(v6),
    }
}

fn blocked_ipv4_reason(ip: Ipv4Addr) -> Option<&'static str> {
    let octets = ip.octets();
    if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_private() {
        Some("private")
    } else if ip.is_link_local() {
        Some("link-local")
    } else if octets[0] == 0 {
        Some("unspecified")
    } else if ip.is_broadcast() || octets[0] >= 240 {
        Some("reserved")
    } else if ip.is_multicast() {
        Some("multicast")
    } else if octets[0] == 100 && (octets[1] & 0xc0) == 64 {
        Some("shared (carrier-grade NAT)")
    } else if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        Some("reserved")
    } else if octets[0] == 198 && (octets[1] & 0xfe) == 18 {
        Some("reserved (benchmarking)")
    } else {
        None
    }
}

fn blocked_ipv6_reason(ip: Ipv6Addr) -> Option<&'static str> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_ipv4_reason(v4);
    }
    let segments = ip.segments();
    if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_unspecified() {
        Some("unspecified")
    } else if (segments[0] & 0xfe00) == 0xfc00 {
        Some("private (unique local)")
    } else if (segments[0] & 0xffc0) == 0xfe80 {
        Some("link-local")
    } else if (segments[0] & 0xffc0) == 0xfec0 {
        Some("site-local")
    } else if ip.is_multicast() {
        Some("multicast")
    } else if segments[..6] == [0, 0, 0, 0, 0, 0]
        || (segments[0] == 0x64 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0])
    {
        // IPv4-compatible and NAT64 addresses embed an IPv4 address.
        let [a, b] = segments[6].to_be_bytes();
        let [c, d] = segments[7].to_be_bytes();
        blocked_ipv4_reason(Ipv4Addr::new(a, b, c, d))
    } else {
        None
    }
}

fn check_address(host: &str, ip: IpAddr) -> Result<(), String> {
    let Some(reason) = blocked_address_reason(ip) else {
        return Ok(());
    };
    if reason == "loopback" && loopback_allowed() {
        return Ok(());
    }
    let target = if host == ip.to_string() {
        format!("`{host}`")
    } else {
        format!("`{host}` (resolves to {ip})")
    };
    Err(format!(
        "WebFetch refused {target}: it is a {reason} address. WebFetch only fetches public http(s) URLs; local, private and link-local addresses are blocked"
    ))
}

/// Validates `url` as a fetch target and returns the socket addresses the
/// connection must use.
pub(crate) fn validate_fetch_target(url: &Url) -> Result<Vec<SocketAddr>, String> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "WebFetch only supports http and https URLs, not `{other}:`"
            ))
        }
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("`{url}` has no usable port"))?;
    match url_host(url) {
        Some(UrlHost::Ip(ip)) => {
            check_address(&ip.to_string(), ip)?;
            Ok(vec![SocketAddr::new(ip, port)])
        }
        Some(UrlHost::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain == "localhost" || domain.ends_with(".localhost") {
                if loopback_allowed() {
                    return resolve_host(&domain, port);
                }
                return Err(format!(
                    "WebFetch refused `{domain}`: it is a loopback host. WebFetch only fetches public http(s) URLs; local, private and link-local addresses are blocked"
                ));
            }
            let addresses = resolve_host(&domain, port)?;
            for address in &addresses {
                check_address(&domain, address.ip())?;
            }
            Ok(addresses)
        }
        None => Err(format!("`{url}` has no host")),
    }
}

enum UrlHost<'a> {
    Ip(IpAddr),
    Domain(&'a str),
}

/// The URL's host as an IP literal or a domain. The URL parser already
/// normalizes alternative IPv4 spellings (`2130706433`, `0x7f.0.0.1`).
fn url_host(url: &Url) -> Option<UrlHost<'_>> {
    let host = url.host_str()?;
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    Some(match bare.parse::<IpAddr>() {
        Ok(ip) => UrlHost::Ip(ip),
        Err(_) => UrlHost::Domain(host),
    })
}

fn resolve_host(domain: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addresses = (domain, port)
        .to_socket_addrs()
        .map_err(|error| format!("could not resolve `{domain}`: {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("could not resolve `{domain}`"));
    }
    Ok(addresses)
}

fn client_for(url: &Url, addresses: &[SocketAddr]) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT);
    if let Some(UrlHost::Domain(domain)) = url_host(url) {
        builder = builder.resolve_to_addrs(domain, addresses);
    }
    builder.build().map_err(|error| error.to_string())
}

/// Fetches `url`, following up to ten redirects, validating every hop.
pub(crate) fn fetch(url: Url) -> Result<FetchedPage, String> {
    let mut current = url;
    for _ in 0..=MAX_REDIRECTS {
        let addresses = validate_fetch_target(&current)?;
        let client = client_for(&current, &addresses)?;
        let response = client
            .get(current.clone())
            .send()
            .map_err(|error| format!("request to {current} failed: {error}"))?;
        let status = response.status();
        if status.is_redirection() {
            if let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            {
                current = current
                    .join(location)
                    .map_err(|error| format!("invalid redirect location `{location}`: {error}"))?;
                continue;
            }
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut raw = Vec::new();
        response
            .take(MAX_FETCH_BODY_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|error| format!("reading the response from {current} failed: {error}"))?;
        let body_truncated = u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_FETCH_BODY_BYTES;
        if body_truncated {
            raw.truncate(usize::try_from(MAX_FETCH_BODY_BYTES).unwrap_or(usize::MAX));
        }
        return Ok(FetchedPage {
            final_url: current,
            status,
            content_type,
            bytes: raw.len(),
            body: String::from_utf8_lossy(&raw).into_owned(),
            body_truncated,
        });
    }
    Err(format!("too many redirects (more than {MAX_REDIRECTS})"))
}

/// Whether a content type is binary data that should not be shown as text.
#[must_use]
pub(crate) fn is_binary_content_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("image/")
        || essence.starts_with("audio/")
        || essence.starts_with("video/")
        || essence.starts_with("font/")
        || matches!(
            essence.as_str(),
            "application/octet-stream"
                | "application/pdf"
                | "application/zip"
                | "application/gzip"
                | "application/x-tar"
                | "application/wasm"
        )
}

/// Elements whose content is never readable text.
const SKIPPED_ELEMENTS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "head", "iframe", "object",
];

/// Elements that start a new line in the readable text.
const BLOCK_ELEMENTS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tr",
    "ul",
];

/// Converts HTML to readable text: drops scripts, styles and markup, keeps
/// one line per block element, marks list items, and decodes entities.
#[must_use]
pub(crate) fn html_to_readable_text(html: &str) -> String {
    let lowered = html.to_ascii_lowercase();
    let mut text = String::with_capacity(html.len() / 2);
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        let Some(next_tag) = rest.find('<') else {
            push_text(&mut text, rest);
            break;
        };
        push_text(&mut text, &rest[..next_tag]);
        index += next_tag;

        if lowered[index..].starts_with("<!--") {
            index = lowered[index..]
                .find("-->")
                .map_or(html.len(), |end| index + end + 3);
            continue;
        }
        let Some(tag_end) = lowered[index..].find('>') else {
            break;
        };
        let tag = &lowered[index + 1..index + tag_end];
        index += tag_end + 1;

        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|ch: char| ch.is_ascii_whitespace() || ch == '/')
            .next()
            .unwrap_or_default();
        if !closing && SKIPPED_ELEMENTS.contains(&name) && !tag.ends_with('/') {
            let close = format!("</{name}");
            index = lowered[index..].find(&close).map_or(html.len(), |start| {
                let after = index + start;
                lowered[after..]
                    .find('>')
                    .map_or(html.len(), |end| after + end + 1)
            });
            continue;
        }
        if name == "li" {
            // One line per item: only the opening tag breaks the line.
            if !closing {
                text.push_str("\n- ");
            }
        } else if BLOCK_ELEMENTS.contains(&name) {
            text.push('\n');
        } else if matches!(name, "td" | "th") && !closing {
            text.push(' ');
        }
    }
    normalize_lines(&decode_entities(&text))
}

fn push_text(text: &mut String, raw: &str) {
    for ch in raw.chars() {
        text.push(if ch.is_whitespace() { ' ' } else { ch });
    }
}

fn normalize_lines(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() || collapsed == "-" {
            if lines.last().is_some_and(|last| !last.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(collapsed);
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// Decodes named entities common in prose plus numeric character references.
#[must_use]
pub(crate) fn decode_entities(input: &str) -> String {
    let mut decoded = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start..];
        let entity_end = rest
            .char_indices()
            .take(12)
            .find(|(_, ch)| *ch == ';')
            .map(|(offset, _)| offset);
        let replacement = entity_end.and_then(|end| decode_entity(&rest[1..end]));
        if let (Some(end), Some(ch)) = (entity_end, replacement) {
            decoded.push(ch);
            rest = &rest[end + 1..];
        } else {
            decoded.push('&');
            rest = &rest[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        _ => return None,
    })
}

/// Cuts `text` to [`MAX_FETCH_TEXT_CHARS`] characters, returning the kept
/// text and the original character count when it was cut.
#[must_use]
pub(crate) fn bound_text(text: &str) -> (String, Option<usize>) {
    let total = text.chars().count();
    if total <= MAX_FETCH_TEXT_CHARS {
        return (text.to_string(), None);
    }
    (
        text.chars().take(MAX_FETCH_TEXT_CHARS).collect(),
        Some(total),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        blocked_address_reason, bound_text, decode_entities, html_to_readable_text,
        is_binary_content_type, validate_fetch_target, MAX_FETCH_TEXT_CHARS,
    };
    use reqwest::Url;
    use std::net::IpAddr;

    fn reason(ip: &str) -> Option<&'static str> {
        blocked_address_reason(ip.parse::<IpAddr>().expect("ip"))
    }

    #[test]
    fn non_public_addresses_are_blocked() {
        assert_eq!(reason("127.0.0.1"), Some("loopback"));
        assert_eq!(reason("127.255.0.9"), Some("loopback"));
        assert_eq!(reason("10.1.2.3"), Some("private"));
        assert_eq!(reason("172.16.0.1"), Some("private"));
        assert_eq!(reason("172.31.255.255"), Some("private"));
        assert_eq!(reason("192.168.1.1"), Some("private"));
        assert_eq!(reason("169.254.169.254"), Some("link-local"));
        assert_eq!(reason("0.0.0.0"), Some("unspecified"));
        assert_eq!(reason("100.64.0.1"), Some("shared (carrier-grade NAT)"));
        assert_eq!(reason("255.255.255.255"), Some("reserved"));
        assert_eq!(reason("224.0.0.1"), Some("multicast"));
        assert_eq!(reason("::1"), Some("loopback"));
        assert_eq!(reason("::"), Some("unspecified"));
        assert_eq!(reason("fd00::1"), Some("private (unique local)"));
        assert_eq!(reason("fe80::1"), Some("link-local"));
        assert_eq!(reason("::ffff:127.0.0.1"), Some("loopback"));
        assert_eq!(reason("::ffff:10.0.0.1"), Some("private"));
        assert_eq!(reason("64:ff9b::a9fe:a9fe"), Some("link-local"));

        assert_eq!(reason("93.184.216.34"), None);
        assert_eq!(reason("172.32.0.1"), None);
        assert_eq!(reason("2606:4700::6810:84e5"), None);
    }

    #[test]
    fn fetch_targets_reject_other_schemes_and_local_hosts() {
        let check = |url: &str| validate_fetch_target(&Url::parse(url).expect("url"));
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/file",
            "http://127.0.0.1:8080/admin",
            "http://localhost/",
            "http://api.localhost/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://192.168.0.10:3000/",
            // Alternative IPv4 spellings are normalized by the URL parser.
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://[::ffff:7f00:1]/",
        ] {
            let error = check(url).expect_err(url);
            assert!(
                error.contains("WebFetch refused") || error.contains("only supports http"),
                "{url}: {error}"
            );
        }
        assert!(check("file:///etc/passwd")
            .expect_err("file")
            .contains("not `file:`"));
        assert!(check("http://8.8.8.8/").is_ok());
    }

    #[test]
    fn html_is_converted_to_readable_lines() {
        let html = r"<html><head><title>T</title><style>body{color:red}</style></head>
            <body><script>var secret = 1;</script><h1>Heading</h1>
            <p>Hello <b>bold</b>&nbsp;world &amp; friends &#8212; &#x41;</p>
            <!-- a comment -->
            <ul><li>one</li><li>two</li></ul><table><tr><td>a</td><td>b</td></tr></table>
            </body></html>";
        let text = html_to_readable_text(html);
        assert_eq!(
            text,
            "Heading\n\nHello bold world & friends \u{2014} A\n\n- one\n- two\n\na b"
        );
        assert!(!text.contains("secret"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn entities_and_bounds() {
        assert_eq!(
            decode_entities("a &lt;b&gt; &unknown; &"),
            "a <b> &unknown; &"
        );
        let long = "x".repeat(MAX_FETCH_TEXT_CHARS + 10);
        let (kept, total) = bound_text(&long);
        assert_eq!(kept.chars().count(), MAX_FETCH_TEXT_CHARS);
        assert_eq!(total, Some(MAX_FETCH_TEXT_CHARS + 10));
        assert_eq!(bound_text("short"), ("short".to_string(), None));
        assert!(is_binary_content_type("image/png"));
        assert!(is_binary_content_type("application/pdf; q=1"));
        assert!(!is_binary_content_type("text/html; charset=utf-8"));
    }
}
