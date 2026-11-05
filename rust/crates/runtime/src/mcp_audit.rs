//! The MCP audit log: one JSON line per MCP tool call, refused calls included, in
//! `<config home>/audit/mcp-YYYY-MM-DD.jsonl` (the UTC day of the call).
//!
//! Metadata only, as Neuron's alpha logging is: when, the session, the server, the
//! tool, its catalog connector and the catalog version, whether the call was allowed
//! and what decided it (a rule, the mode, the user, a session choice or a hook), how
//! long it ran, the bytes in and out, the sha256 of the arguments, and an error class.
//! Never the text of the arguments or of the output. The log stays on this computer;
//! nothing uploads it.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::McpConfigCollection;
use crate::mcp::mcp_tool_prefix;
use crate::mcp_catalog::{encode_hex, McpCatalog};
use crate::permissions::PermissionDecider;

/// The audit folder in the config home.
pub const MCP_AUDIT_DIR: &str = "audit";

/// One configured server, as the log names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditedServer {
    /// `mcp__<server>__`.
    prefix: String,
    name: String,
    catalog_id: Option<String>,
}

/// Appends audit lines for one session's MCP calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuditLog {
    dir: PathBuf,
    session: String,
    servers: Vec<AuditedServer>,
    catalog_version: String,
}

/// An MCP call to record.
#[derive(Debug, Clone, Copy)]
pub struct McpAuditCall<'a> {
    /// The tool as the model called it: `mcp__<server>__<tool>`, or a wrapper
    /// (`MCPTool`) naming one.
    pub tool_name: &'a str,
    /// The call's input as the model sent it.
    pub input: &'a str,
    pub allowed: bool,
    pub decided_by: PermissionDecider,
    /// How long the tool ran (zero when it did not).
    pub duration: Duration,
    /// Size of what the tool returned (or of the refusal).
    pub output_bytes: usize,
    /// `denied`, `interrupted` or `tool_error`; `None` for a call that succeeded.
    pub error_class: Option<&'static str>,
}

impl McpAuditLog {
    /// A log in `<config_home>/audit` for `session`, naming the servers `mcp` configures
    /// (and their catalog connectors).
    #[must_use]
    pub fn new(config_home: &Path, session: &str, mcp: &McpConfigCollection) -> Self {
        let mut servers = mcp
            .servers()
            .iter()
            .map(|(name, server)| AuditedServer {
                prefix: mcp_tool_prefix(name),
                name: name.clone(),
                catalog_id: crate::mcp_policy::server_catalog_id(name, server),
            })
            .collect::<Vec<_>>();
        // The longest prefix first: `mcp__a_b__` before `mcp__a__`.
        servers.sort_by_key(|server| std::cmp::Reverse(server.prefix.len()));
        Self {
            dir: config_home.join(MCP_AUDIT_DIR),
            session: session.to_string(),
            servers,
            catalog_version: McpCatalog::builtin().version.clone(),
        }
    }

    /// The file for the UTC day `unix_secs` falls on.
    #[must_use]
    pub fn path_for(&self, unix_secs: u64) -> PathBuf {
        let (year, month, day, ..) = utc_civil(unix_secs);
        self.dir
            .join(format!("mcp-{year:04}-{month:02}-{day:02}.jsonl"))
    }

    /// Whether `tool_name` with `input` is an MCP call (and so is recorded).
    #[must_use]
    pub fn is_mcp_call(tool_name: &str, input: &str) -> bool {
        crate::permissions::mcp_call_target(tool_name, input).is_some()
    }

    /// Appends the line for `call`; does nothing for a call that is not an MCP call.
    pub fn record(&self, call: &McpAuditCall<'_>) -> std::io::Result<()> {
        let Some((qualified, arguments)) =
            crate::permissions::mcp_call_target(call.tool_name, call.input)
        else {
            return Ok(());
        };
        let server = self
            .servers
            .iter()
            .find(|server| qualified.starts_with(&server.prefix));
        let (server_name, tool) = if let Some(server) = server {
            (
                server.name.clone(),
                qualified[server.prefix.len()..].to_string(),
            )
        } else {
            // A server this session does not configure: split at the first `__`.
            let rest = qualified.trim_start_matches("mcp__");
            match rest.split_once("__") {
                Some((server, tool)) => (server.to_string(), tool.to_string()),
                None => (rest.to_string(), String::new()),
            }
        };
        let catalog_id = server.and_then(|server| server.catalog_id.clone());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let line = json!({
            "ts": utc_timestamp(now.as_secs(), now.subsec_millis()),
            "session": self.session,
            "server": server_name,
            "tool": tool,
            "catalog_id": catalog_id,
            "catalog_version": catalog_id.as_ref().map(|_| self.catalog_version.clone()),
            "decision": if call.allowed { "allow" } else { "deny" },
            "decided_by": call.decided_by.as_str(),
            "duration_ms": u64::try_from(call.duration.as_millis()).unwrap_or(u64::MAX),
            "bytes_in": arguments.len(),
            "bytes_out": call.output_bytes,
            "args_sha256": encode_hex(&Sha256::digest(arguments.as_bytes())),
            "error_class": call.error_class,
        });
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path_for(now.as_secs());
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        // One write per line, so lines from two sessions do not interleave.
        file.write_all(format!("{line}\n").as_bytes())
    }
}

/// `2026-11-05T09:30:12.345Z`.
fn utc_timestamp(unix_secs: u64, millis: u32) -> String {
    let (year, month, day, hour, minute, second) = utc_civil(unix_secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// The UTC calendar date and time of `unix_secs`: year, month, day, hour, minute,
/// second (Howard Hinnant's `civil_from_days`).
fn utc_civil(unix_secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = i64::try_from(unix_secs / 86_400).unwrap_or(i64::MAX);
    let seconds = unix_secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (
        year,
        month as u32,
        day as u32,
        (seconds / 3_600) as u32,
        (seconds % 3_600 / 60) as u32,
        (seconds % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::{utc_civil, utc_timestamp, McpAuditCall, McpAuditLog};
    use crate::config::ConfigLoader;
    use crate::permissions::PermissionDecider;

    #[test]
    fn days_and_times_are_utc_calendar_dates() {
        assert_eq!(utc_civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(utc_civil(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(utc_civil(1_793_870_245), (2026, 11, 5, 9, 17, 25));
        assert_eq!(utc_timestamp(1_793_870_245, 7), "2026-11-05T09:17:25.007Z");
    }

    #[test]
    fn a_line_names_the_call_and_never_its_text() {
        let root = std::env::temp_dir().join(format!(
            "neuron-mcp-audit-{}-{}",
            std::process::id(),
            crate::mcp_policy::unix_now()
        ));
        let home = root.join("home");
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("settings.json"),
            r#"{"defaultMcpServers":false,"mcpServers":{"alpha":{"command":"alpha-mcp"}}}"#,
        )
        .unwrap();
        let config = ConfigLoader::new(root.join("repo"), &home).load().unwrap();
        let log = McpAuditLog::new(&home, "session-1", config.mcp());
        let sentinel = "SENTINEL-audit-argument-4f2a";
        let input = format!(r#"{{"text":"{sentinel}"}}"#);

        log.record(&McpAuditCall {
            tool_name: "mcp__alpha__echo",
            input: &input,
            allowed: true,
            decided_by: PermissionDecider::Mode,
            duration: Duration::from_millis(12),
            output_bytes: 40,
            error_class: None,
        })
        .unwrap();
        let wrapped = format!(
            r#"{{"qualifiedName":"mcp__alpha__echo","arguments":{{"text":"{sentinel}"}}}}"#
        );
        log.record(&McpAuditCall {
            tool_name: "MCPTool",
            input: &wrapped,
            allowed: false,
            decided_by: PermissionDecider::Rule,
            duration: Duration::ZERO,
            output_bytes: 70,
            error_class: Some("denied"),
        })
        .unwrap();
        // Not an MCP call: nothing is written.
        log.record(&McpAuditCall {
            tool_name: "bash",
            input: &input,
            allowed: true,
            decided_by: PermissionDecider::Mode,
            duration: Duration::ZERO,
            output_bytes: 0,
            error_class: None,
        })
        .unwrap();

        let files = fs::read_dir(home.join("audit"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("mcp-"), "{name}");
        assert_eq!(
            files[0]
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("jsonl")
        );
        let text = fs::read_to_string(&files[0]).unwrap();
        assert!(!text.contains(sentinel), "{text}");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["server"], "alpha");
        assert_eq!(lines[0]["tool"], "echo");
        assert_eq!(lines[0]["session"], "session-1");
        assert_eq!(lines[0]["decision"], "allow");
        assert_eq!(lines[0]["decided_by"], "mode");
        assert_eq!(lines[0]["duration_ms"], 12);
        assert_eq!(lines[0]["bytes_in"], input.len());
        assert_eq!(lines[0]["args_sha256"].as_str().unwrap().len(), 64);
        assert!(lines[0]["catalog_id"].is_null());
        assert!(lines[0]["error_class"].is_null());
        // The wrapper call is recorded as the tool it runs, hashing only its arguments.
        assert_eq!(lines[1]["tool"], "echo");
        assert_eq!(lines[1]["decision"], "deny");
        assert_eq!(lines[1]["decided_by"], "rule");
        assert_eq!(lines[1]["error_class"], "denied");
        assert_eq!(lines[1]["args_sha256"], lines[0]["args_sha256"]);
        let _ = fs::remove_dir_all(&root);
    }
}
