//! Redaction of token-shaped secrets.
//!
//! [`redact`] replaces credentials in free text with `[REDACTED:<kind>]`:
//! provider API keys (`sk-…`, `sk-ant-…`, `sk-or-…`, `AIza…`, `gsk_…`,
//! `nvapi-…`, `xai-…`), GitHub tokens, AWS access key ids and the secret on the
//! same line, Slack `xox*` tokens, JWTs, `Bearer`/`Basic` credentials, PEM
//! private key blocks, passwords in URLs, `KEY=value` / `"api_key": "…"`
//! assignments whose name ends in a credential word, and the exact values of
//! credential environment variables present in this process (plus values
//! registered with [`register_secret`]).
//!
//! It is tuned for few false positives: git SHAs, UUIDs, lockfile hashes and
//! ordinary identifiers are left alone, prefixed keys need a digit, and an
//! assignment's value must look like data (not a type, a call or a
//! placeholder). Every pattern runs on the `regex` crate, whose matching is
//! linear in the input, so no input can make it backtrack catastrophically.
//!
//! Redaction is on by default; [`set_enabled`] turns it off process-wide
//! (the `redactSecrets: false` setting).

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use regex::{Captures, Regex, RegexSet};
use serde_json::Value;

static ENABLED: AtomicBool = AtomicBool::new(true);
static REGISTERED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Shortest value redacted by exact match (environment and registered
/// secrets); shorter values would match ordinary text.
pub const MIN_EXACT_SECRET_LEN: usize = 8;

/// Turns redaction on or off for this process ([`redact`] and
/// [`redact_json`]). On by default.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether [`redact`] changes anything (see [`set_enabled`]).
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Registers a secret the process holds outside its environment (a key read
/// from the OS keychain, say), so its exact value is redacted too. `label`
/// becomes the kind: `[REDACTED:<label>]`. Values shorter than
/// [`MIN_EXACT_SECRET_LEN`] are ignored.
pub fn register_secret(label: &str, value: &str) {
    let value = value.trim();
    if value.len() < MIN_EXACT_SECRET_LEN {
        return;
    }
    let mut registered = REGISTERED.lock().unwrap_or_else(PoisonError::into_inner);
    if !registered.iter().any(|(_, known)| known == value) {
        registered.push((label.to_string(), value.to_string()));
    }
}

/// Redacts `text` with the process environment's credential values and the
/// registered secrets, unless redaction is turned off.
#[must_use]
pub fn redact(text: &str) -> Cow<'_, str> {
    if !is_enabled() {
        return Cow::Borrowed(text);
    }
    let mut exact = credential_env_values(
        std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?))),
    );
    exact.extend(
        REGISTERED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned(),
    );
    redact_with(text, &exact)
}

/// Redacts every string in `value` (object keys are kept) with [`redact`].
/// Returns whether anything changed.
pub fn redact_json(value: &mut Value) -> bool {
    if !is_enabled() {
        return false;
    }
    match value {
        Value::String(text) => {
            if let Cow::Owned(redacted) = redact(text) {
                *text = redacted;
                true
            } else {
                false
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| redact_json(item) | changed),
        Value::Object(map) => map
            .values_mut()
            .fold(false, |changed, item| redact_json(item) | changed),
        _ => false,
    }
}

/// Redacts `text` with explicit exact secrets as `(label, value)` pairs,
/// ignoring the process-wide switch. The pure core of [`redact`].
#[must_use]
pub fn redact_with<'a>(text: &'a str, exact: &[(String, String)]) -> Cow<'a, str> {
    let mut current = Cow::Borrowed(text);
    // Longest first, so a secret containing another is replaced whole.
    let mut exact: Vec<&(String, String)> = exact
        .iter()
        .filter(|(_, value)| value.len() >= MIN_EXACT_SECRET_LEN)
        .collect();
    exact.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    for (label, value) in exact {
        if current.contains(value.as_str()) {
            current = Cow::Owned(current.replace(value.as_str(), &format!("[REDACTED:{label}]")));
        }
    }

    let patterns = patterns();
    let hits = patterns.set.matches(&current);
    if !hits.matched_any() {
        return current;
    }
    if hits.matched(patterns.aws_rule) {
        current = redact_aws_secret_pairs(current, &patterns.aws_secret);
    }
    for (index, rule) in patterns.rules.iter().enumerate() {
        if !hits.matched(index) {
            continue;
        }
        let haystack = current.as_ref();
        let mut changed = false;
        let replaced = rule.regex.replace_all(haystack, |caps: &Captures<'_>| {
            if let Some(replacement) = (rule.replace)(caps, haystack) {
                changed = true;
                replacement
            } else {
                caps[0].to_string()
            }
        });
        if changed {
            current = Cow::Owned(replaced.into_owned());
        }
    }
    current
}

/// Environment variables whose values are credentials, as `(env:NAME, value)`.
pub fn credential_env_values(
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    vars.into_iter()
        .filter(|(key, value)| is_credential_env_name(key) && looks_like_secret_value(value))
        .map(|(key, value)| (format!("env:{key}"), value))
        .collect()
}

/// Whether an environment variable name marks a credential: a known
/// provider/Neuron variable or a name ending in a credential word.
#[must_use]
pub fn is_credential_env_name(name: &str) -> bool {
    const KNOWN: &[&str] = &[
        "NEURON_TOKEN",
        "NEURON_SESSION_TOKEN",
        "ANTHROPIC_AUTH_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "NPM_TOKEN",
        "HF_TOKEN",
        "PGPASSWORD",
    ];
    const SUFFIXES: &[&str] = &[
        "API_KEY",
        "APIKEY",
        "TOKEN",
        "SECRET",
        "SECRET_KEY",
        "ACCESS_KEY",
        "PRIVATE_KEY",
        "PASSWORD",
        "PASSWD",
        "AUTH_TOKEN",
        "CLIENT_SECRET",
    ];
    let upper = name.to_ascii_uppercase();
    KNOWN.contains(&upper.as_str())
        || SUFFIXES.iter().any(|suffix| {
            upper == *suffix
                || upper
                    .strip_suffix(suffix)
                    .is_some_and(|stem| stem.ends_with('_'))
        })
}

fn looks_like_secret_value(value: &str) -> bool {
    let value = value.trim();
    value.len() >= MIN_EXACT_SECRET_LEN
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "disabled" | "enabled" | "undefined"
        )
        && !value.chars().all(|c| c.is_ascii_digit())
}

/// Computes a match's replacement from its captures and the whole haystack;
/// `None` keeps the match unchanged.
type Replacer = fn(&Captures<'_>, &str) -> Option<String>;

struct Rule {
    regex: Regex,
    replace: Replacer,
}

struct Patterns {
    set: RegexSet,
    rules: Vec<Rule>,
    /// Index in `set` of the AWS access key id pattern (also a rule).
    aws_rule: usize,
    aws_secret: Regex,
}

fn whole(kind: &'static str) -> String {
    format!("[REDACTED:{kind}]")
}

fn has_digit(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_digit())
}

fn has_letter(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_alphabetic())
}

/// A prefixed key: the part after the prefix holds both letters and digits
/// (a real key always does; a kebab-case word that happens to start with
/// `sk-` does not).
fn keyish(caps: &Captures<'_>, kind: &'static str) -> Option<String> {
    let body = caps.get(1).map_or(&caps[0], |m| m.as_str());
    (has_digit(body) && has_letter(body)).then(|| whole(kind))
}

#[allow(clippy::too_many_lines)]
fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let specs: Vec<(&str, Replacer)> = vec![
            (
                r"(?s)-----BEGIN [A-Z0-9 ]{0,40}PRIVATE KEY(?: BLOCK)?-----(?:.*?-----END [A-Z0-9 ]{0,40}PRIVATE KEY(?: BLOCK)?-----|.*)",
                |_, _| Some(whole("private-key")),
            ),
            (
                r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                |_, _| Some(whole("jwt")),
            ),
            (r"\bsk-ant-([A-Za-z0-9_-]{20,})", |caps, _| {
                keyish(caps, "anthropic-key")
            }),
            (r"\bsk-or-([A-Za-z0-9_-]{20,})", |caps, _| {
                keyish(caps, "openrouter-key")
            }),
            (r"\bsk-([A-Za-z0-9_-]{20,})", |caps, _| keyish(caps, "api-key")),
            (r"\bAIza([0-9A-Za-z_-]{35})", |caps, _| {
                keyish(caps, "google-api-key")
            }),
            (r"\bgsk_([A-Za-z0-9]{20,})", |caps, _| keyish(caps, "groq-key")),
            (r"\bnvapi-([A-Za-z0-9_-]{20,})", |caps, _| {
                keyish(caps, "nvidia-key")
            }),
            (r"\bxai-([A-Za-z0-9_-]{20,})", |caps, _| keyish(caps, "xai-key")),
            (
                r"\b(?:gh[pousr]_([A-Za-z0-9]{30,})|github_pat_([A-Za-z0-9_]{22,}))",
                |_, _| Some(whole("github-token")),
            ),
            (r"\bxox[abposr]-([A-Za-z0-9-]{10,})", |caps, _| {
                keyish(caps, "slack-token")
            }),
            (r"\b(?:nzx|ses)_([A-Za-z0-9_-]{20,})", |caps, _| {
                keyish(caps, "neuron-token")
            }),
            (r"\b(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}\b", |_, _| {
                Some(whole("aws-access-key-id"))
            }),
            (
                r"(?i)\b(bearer|basic)(\s+)([A-Za-z0-9._~+/-]{16,}=*)",
                |caps, _| {
                    let token = &caps[3];
                    (has_digit(token) && !token.starts_with("[REDACTED")).then(|| {
                        format!("{}{}{}", &caps[1], &caps[2], whole("bearer"))
                    })
                },
            ),
            (
                r"\b([a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:)([^\s@/]{3,})(@)",
                |caps, _| {
                    (!caps[2].starts_with("[REDACTED"))
                        .then(|| format!("{}{}{}", &caps[1], whole("url-password"), &caps[3]))
                },
            ),
            (
                r#"([A-Za-z0-9_.-]{0,80})(["']?[ \t]*[:=][ \t]*)(["']?)([^\s"'`,;]+)"#,
                assignment,
            ),
        ];
        let set = RegexSet::new(specs.iter().map(|(pattern, _)| *pattern))
            .expect("redaction patterns compile");
        let rules: Vec<Rule> = specs
            .into_iter()
            .map(|(pattern, replace)| Rule {
                regex: Regex::new(pattern).expect("redaction pattern compiles"),
                replace,
            })
            .collect();
        let aws_rule = rules
            .iter()
            .position(|rule| rule.regex.as_str().contains("AKIA"))
            .expect("aws rule present");
        Patterns {
            set,
            rules,
            aws_rule,
            aws_secret: Regex::new(r"(^|[^A-Za-z0-9/+=])([A-Za-z0-9/+]{40})([^A-Za-z0-9/+=]|$)")
                .expect("aws secret pattern compiles"),
        }
    })
}

/// The 40-character secret printed on the same line as an AWS access key id
/// (`aws configure`-style output, CSV credential exports).
fn redact_aws_secret_pairs<'a>(text: Cow<'a, str>, secret: &Regex) -> Cow<'a, str> {
    let id = Regex::new(r"\b(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}\b").expect("aws id compiles");
    if !text.lines().any(|line| id.is_match(line)) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if id.is_match(line) {
            let replaced = secret.replace_all(line, |caps: &Captures<'_>| {
                let candidate = &caps[2];
                let mixed = candidate.bytes().any(|b| b.is_ascii_uppercase())
                    && candidate.bytes().any(|b| b.is_ascii_lowercase())
                    && has_digit(candidate);
                if mixed {
                    format!("{}{}{}", &caps[1], whole("aws-secret"), &caps[3])
                } else {
                    caps[0].to_string()
                }
            });
            out.push_str(&replaced);
        } else {
            out.push_str(line);
        }
    }
    Cow::Owned(out)
}

/// Credential words an assignment's name must end with (its last segment
/// after splitting on `_`, `-`, `.` and camelCase).
const CREDENTIAL_WORDS: &[&str] = &[
    "key",
    "apikey",
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "credential",
    "credentials",
    "secretkey",
    "accesskey",
    "privatekey",
    "authtoken",
    "accesstoken",
    "refreshtoken",
    "clientsecret",
];

fn last_name_segment(name: &str) -> String {
    let mut segment = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | '.') {
            segment.clear();
            previous_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && previous_lower {
            segment.clear();
        }
        previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        segment.push(c.to_ascii_lowercase());
    }
    segment
}

/// `NAME=value`, `NAME: value`, `"name": "value"`: redacts the value when the
/// name ends in a credential word and the value looks like data.
fn assignment(caps: &Captures<'_>, haystack: &str) -> Option<String> {
    let name = &caps[1];
    let separator = &caps[2];
    let quote = &caps[3];
    let value = caps.get(4)?;
    let text = value.as_str();
    if name.is_empty() || !CREDENTIAL_WORDS.contains(&last_name_segment(name).as_str()) {
        return None;
    }
    if !plausible_secret_value(text, !quote.is_empty()) {
        return None;
    }
    // A call (`api_key = os.getenv("X")`) or an index (`token = tokens[0]`).
    if matches!(
        haystack[value.end()..].chars().next(),
        Some('(' | '[' | '{')
    ) {
        return None;
    }
    let kind = last_name_segment(name);
    Some(format!("{name}{separator}{quote}{}", whole_kind(&kind)))
}

fn whole_kind(segment: &str) -> String {
    let kind = match segment {
        "pwd" | "passwd" | "passphrase" => "password",
        "credentials" => "credential",
        other if other.ends_with("token") => "token",
        other if other.ends_with("secret") => "secret",
        other if other.ends_with("key") => "key",
        other => other,
    };
    format!("[REDACTED:{kind}]")
}

fn plausible_secret_value(value: &str, quoted: bool) -> bool {
    let value = value.trim_end_matches(['.', ')', ']', '}']);
    if value.len() < 8 || value.starts_with("[REDACTED") {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if value.starts_with([
        '$', '%', '<', '{', '[', '(', '*', '&', '@', '#', '/', '\\', '~', '.',
    ]) || matches!(
        lower.as_str(),
        "true" | "false" | "null" | "none" | "undefined" | "required" | "optional"
    ) || [
        "your",
        "example",
        "changeme",
        "placeholder",
        "xxxx",
        "redacted",
        "process.env",
        "os.environ",
        "env::",
        "getenv",
    ]
    .iter()
    .any(|word| lower.contains(word))
        || lower.contains("::")
        || value
            .chars()
            .all(|c| c == value.chars().next().unwrap_or(' '))
    {
        return false;
    }
    if quoted {
        return true;
    }
    // Unquoted: code identifiers and dotted paths are not secrets.
    let identifier_path = value.split('.').all(|part| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }) && !has_digit(value);
    if identifier_path {
        return false;
    }
    has_digit(value)
        || (value.len() >= 16
            && value.bytes().any(|b| b.is_ascii_uppercase())
            && value.bytes().any(|b| b.is_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::{credential_env_values, is_credential_env_name, redact_json, redact_with};

    fn r(text: &str) -> String {
        redact_with(text, &[]).into_owned()
    }

    #[test]
    fn provider_keys_are_redacted_by_kind() {
        let cases = [
            (
                "sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdefGHIJ",
                "anthropic-key",
            ),
            (
                "sk-or-v1-0123456789abcdef0123456789abcdef",
                "openrouter-key",
            ),
            ("sk-proj-Abc123def456GHI789jkl012MNO", "api-key"),
            ("AIzaSyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q", "google-api-key"),
            ("gsk_abcDEF123456ghiJKL789mno", "groq-key"),
            ("nvapi-AbC123dEf456GhI789jKl012", "nvidia-key"),
            ("xai-AbC123dEf456GhI789jKl012", "xai-key"),
            ("ghp_0123456789abcdefABCDEF0123456789abcd", "github-token"),
            (
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
                "github-token",
            ),
            ("xoxb-1234567890-abcdefghij", "slack-token"),
            ("AKIAIOSFODNN7EXAMPLE", "aws-access-key-id"),
            ("nzx_abc123DEF456ghi789JKL012", "neuron-token"),
        ];
        for (secret, kind) in cases {
            let out = r(&format!("value: {secret} end"));
            assert_eq!(out, format!("value: [REDACTED:{kind}] end"), "{secret}");
        }
    }

    #[test]
    fn jwt_bearer_and_url_password() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_eq!(r(&format!("t={jwt}")), "t=[REDACTED:jwt]");
        assert_eq!(
            r("Authorization: Bearer abc123def456ghi789jkl"),
            "Authorization: Bearer [REDACTED:bearer]"
        );
        assert_eq!(
            r("git remote: https://user:hunter2pass@github.com/o/r.git"),
            "git remote: https://user:[REDACTED:url-password]@github.com/o/r.git"
        );
    }

    #[test]
    fn pem_private_keys_are_redacted_whole_even_when_truncated() {
        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\nafter";
        assert_eq!(r(pem), "before\n[REDACTED:private-key]\nafter");
        let open = "x\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n";
        assert_eq!(r(open), "x\n[REDACTED:private-key]");
        // Public keys stay.
        let public = "-----BEGIN PUBLIC KEY-----\nMIIBIjAN\n-----END PUBLIC KEY-----";
        assert_eq!(r(public), public);
    }

    #[test]
    fn credential_assignments_are_redacted() {
        assert_eq!(
            r("OPENAI_API_KEY=abc123xyz789"),
            "OPENAI_API_KEY=[REDACTED:key]"
        );
        assert_eq!(
            r(r#"{"api_key": "hunter2hunter2", "model": "gpt"}"#),
            r#"{"api_key": "[REDACTED:key]", "model": "gpt"}"#
        );
        assert_eq!(
            r("DB_PASSWORD: s3cretPass!"),
            "DB_PASSWORD: [REDACTED:password]"
        );
        assert_eq!(
            r("clientSecret = 'abcdefghij'"),
            "clientSecret = '[REDACTED:secret]'"
        );
        assert_eq!(
            r("export GITHUB_TOKEN=abc123def456"),
            "export GITHUB_TOKEN=[REDACTED:token]"
        );
    }

    #[test]
    fn aws_secret_on_the_access_key_line() {
        let line = "AKIAIOSFODNN7EXAMPLE,wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert_eq!(
            r(line),
            "[REDACTED:aws-access-key-id],[REDACTED:aws-secret]"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let keep = [
            "commit 3f2a9c1e5b7d4f6a8c0e2b4d6f8a0c2e4b6d8f0a",
            "id 123e4567-e89b-12d3-a456-426614174000",
            r#""integrity": "sha512-Q2Fyb2xpbmUgaXMgYSBnb29kIGRvZw==""#,
            r#"checksum = "f0805222e57f7521d6a62e36fa9163bc891acd422f971defe97d64e70d0a4fe5""#,
            "max_tokens: 4096",
            "TOKEN_LIMIT=4096",
            "let api_key = std::env::var(\"OPENAI_API_KEY\")?;",
            "api_key = os.getenv(\"KEY\")",
            "apiKey: string;",
            "token: String,",
            "password = self.password",
            "API_KEY=${OPENAI_API_KEY}",
            "OPENAI_API_KEY=your-key-here",
            "monkey=banana12345",
            "risk-assessment-framework-documentation",
            "task-runner-for-the-workspace-tests",
            "a basic introduction to tokenization",
            "primary_key: id",
            "https://github.com/rahul/neuron",
            "keyboard=us-intl-2024",
            "PASSWORD=********",
        ];
        for text in keep {
            assert_eq!(r(text), text, "should not redact: {text}");
        }
    }

    #[test]
    fn exact_secrets_and_env_values() {
        let exact = vec![(
            "env:MY_SERVICE_TOKEN".to_string(),
            "plainword-secret".to_string(),
        )];
        assert_eq!(
            redact_with("got plainword-secret back", &exact),
            "got [REDACTED:env:MY_SERVICE_TOKEN] back"
        );
        let values = credential_env_values(vec![
            (
                "MY_SERVICE_TOKEN".to_string(),
                "plainword-secret".to_string(),
            ),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("GITHUB_TOKEN".to_string(), "short".to_string()),
            ("SOME_TOKEN".to_string(), "12345678".to_string()),
        ]);
        assert_eq!(values.len(), 1);
        assert!(is_credential_env_name("stripe_secret"));
        assert!(is_credential_env_name("AWS_SECRET_ACCESS_KEY"));
        assert!(!is_credential_env_name("TOKENIZER"));
        assert!(!is_credential_env_name("KEYBOARD"));
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = r("OPENAI_API_KEY=sk-proj-Abc123def456GHI789jkl012MNO");
        assert_eq!(r(&once), once);
    }

    #[test]
    fn json_strings_are_redacted_and_keys_kept() {
        let mut value = serde_json::json!({
            "sk-ant-api03-AbCdEf0123456789xyzXYZ": 1,
            "out": ["ghp_0123456789abcdefABCDEF0123456789abcd", 3],
        });
        assert!(redact_json(&mut value));
        assert_eq!(value["out"][0], "[REDACTED:github-token]");
        assert!(value
            .as_object()
            .unwrap()
            .contains_key("sk-ant-api03-AbCdEf0123456789xyzXYZ"));
    }

    #[test]
    fn large_inputs_stay_fast() {
        let text = "a=".repeat(200_000) + &"x".repeat(200_000) + "-----BEGIN ";
        let started = std::time::Instant::now();
        let _ = r(&text);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
