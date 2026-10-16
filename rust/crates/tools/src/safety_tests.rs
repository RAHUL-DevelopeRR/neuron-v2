//! Tests for tool permission escalation, cancellation, tool advertisement and
//! `WebFetch` safety.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use runtime::cancel::CancellationToken;
use runtime::permission_enforcer::PermissionEnforcer;
use runtime::{
    PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
    PermissionPrompter, PermissionRequest, SessionApprovals,
};
use serde_json::{json, Value};

use super::{execute_tool, mvp_tool_specs, GlobalToolRegistry};

fn workspace_write_policy() -> PermissionPolicy {
    mvp_tool_specs()
        .into_iter()
        .fold(
            PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            |policy, spec| policy.with_tool_requirement(spec.name, spec.required_permission),
        )
        .with_session_approvals(SessionApprovals::isolated())
}

struct AllowOnce {
    requests: Vec<PermissionRequest>,
}

impl PermissionPrompter for AllowOnce {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        self.requests.push(request.clone());
        PermissionPromptDecision::Allow
    }
}

fn parse(output: &str) -> Value {
    serde_json::from_str(output).expect("tool output should be JSON")
}

#[test]
fn approved_bash_escalation_runs_and_unapproved_escalation_is_explained() {
    let command = if cfg!(windows) {
        "cmd /D /C echo approved-run"
    } else {
        "sh -c 'echo approved-run'"
    };
    let input = json!({ "command": command, "timeout": 60_000 });
    let policy = workspace_write_policy();
    let registry =
        GlobalToolRegistry::builtin().with_enforcer(PermissionEnforcer::new(policy.clone()));

    let denied = registry
        .execute("bash", &input)
        .expect_err("an unapproved escalation must be denied");
    assert!(denied.contains("--permission-mode"), "{denied}");
    assert!(denied.contains("--allowedTools"), "{denied}");

    // The conversation loop asks the user, who allows the call once.
    let mut prompter = AllowOnce {
        requests: Vec::new(),
    };
    assert_eq!(
        policy.authorize("bash", &input.to_string(), Some(&mut prompter)),
        PermissionOutcome::Allow
    );
    assert_eq!(prompter.requests.len(), 1);
    assert_eq!(
        prompter.requests[0].required_mode,
        PermissionMode::DangerFullAccess
    );

    let output = parse(
        &registry
            .execute("bash", &input)
            .expect("an approved escalation must run"),
    );
    assert!(
        output["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("approved-run")),
        "{output}"
    );
    // The approval was for one call only.
    assert!(registry.execute("bash", &input).is_err());
}

#[test]
fn approved_powershell_escalation_actually_runs() {
    if super::powershell::detect_powershell().is_err() {
        eprintln!("skipping: PowerShell is not installed");
        return;
    }
    let input = json!({
        "command": "Set-Variable -Name marker -Value approved-ps; Write-Output $marker",
        "timeout": 60_000
    });
    let policy = workspace_write_policy();
    let registry =
        GlobalToolRegistry::builtin().with_enforcer(PermissionEnforcer::new(policy.clone()));
    assert!(registry.execute("PowerShell", &input).is_err());

    let mut prompter = AllowOnce {
        requests: Vec::new(),
    };
    assert_eq!(
        policy.authorize("PowerShell", &input.to_string(), Some(&mut prompter)),
        PermissionOutcome::Allow
    );
    let output = parse(
        &registry
            .execute("PowerShell", &input)
            .expect("approved PowerShell must run"),
    );
    assert!(
        output["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("approved-ps")),
        "{output}"
    );
}

#[test]
fn read_only_powershell_runs_without_approval_in_workspace_write() {
    if super::powershell::detect_powershell().is_err() {
        eprintln!("skipping: PowerShell is not installed");
        return;
    }
    let registry = GlobalToolRegistry::builtin()
        .with_enforcer(PermissionEnforcer::new(workspace_write_policy()));
    let output = parse(
        &registry
            .execute(
                "PowerShell",
                &json!({ "command": "Write-Output 'read only'", "timeout": 60_000 }),
            )
            .expect("read-only PowerShell runs without a prompt"),
    );
    assert!(output["stdout"]
        .as_str()
        .is_some_and(|stdout| stdout.contains("read only")));
}

#[test]
fn execute_with_cancel_stops_a_running_bash_command() {
    let token = CancellationToken::new();
    let trigger = token.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        trigger.cancel();
    });
    let command = if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    };
    let started = Instant::now();
    let output = parse(
        &GlobalToolRegistry::builtin()
            .execute_with_cancel("bash", &json!({ "command": command }), &token)
            .expect("a cancelled command still returns its output"),
    );
    canceller.join().expect("canceller thread");
    assert_eq!(output["interrupted"], true, "{output}");
    assert_eq!(output["returnCodeInterpretation"], "cancelled", "{output}");
    assert!(started.elapsed() < Duration::from_secs(15));
}

fn advertised_names(registry: &GlobalToolRegistry) -> BTreeSet<String> {
    registry
        .definitions(None)
        .into_iter()
        .map(|definition| definition.name)
        .collect()
}

#[test]
fn default_definitions_advertise_only_functional_tools() {
    let registry = GlobalToolRegistry::builtin();
    let names = advertised_names(&registry);
    for tool in [
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "glob_search",
        "grep_search",
        "WebFetch",
        "WebSearch",
        "ToolSearch",
        "PowerShell",
        "TodoWrite",
    ] {
        assert!(names.contains(tool), "{tool} must be advertised");
    }
    for stub in [
        "TestingPermission",
        "RunTaskPacket",
        "CronCreate",
        "LSP",
        "McpAuth",
    ] {
        assert!(!names.contains(stub), "{stub} must not be advertised");
    }
    assert!(names.len() < mvp_tool_specs().len());

    // Test fixtures cannot be found or activated through ToolSearch.
    let hidden = registry.search("select:TestingPermission", 5, None, None);
    assert_eq!(hidden.matches, Vec::<String>::new());
    assert!(!advertised_names(&registry).contains("TestingPermission"));

    // An explicit allow-list still advertises exactly what it names.
    let allowed = BTreeSet::from(["CronCreate".to_string()]);
    let listed = registry
        .definitions(Some(&allowed))
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(listed, vec!["CronCreate".to_string()]);
}

#[test]
fn tool_search_activates_deferred_tools_for_later_requests() {
    let registry = GlobalToolRegistry::builtin();
    // Only this test selects WorkerObserveCompletion.
    assert!(!advertised_names(&registry).contains("WorkerObserveCompletion"));

    let found = registry.search("select:WorkerObserveCompletion", 5, None, None);
    assert_eq!(found.matches, vec!["WorkerObserveCompletion".to_string()]);
    assert!(advertised_names(&registry).contains("WorkerObserveCompletion"));
}

#[test]
fn mcp_tools_stay_deferred_until_tool_search_finds_them() {
    // Only this test registers mcp__deferral_probe__*.
    let registry = GlobalToolRegistry::builtin()
        .with_runtime_tools(vec![
            super::RuntimeToolDefinition {
                name: "mcp__deferral_probe__navigate".to_string(),
                description: Some("Open a page in the probe browser".to_string()),
                input_schema: json!({ "type": "object" }),
                required_permission: PermissionMode::ReadOnly,
            },
            super::RuntimeToolDefinition {
                name: "mcp__deferral_probe__click".to_string(),
                description: Some("Click an element in the probe browser".to_string()),
                input_schema: json!({ "type": "object" }),
                required_permission: PermissionMode::ReadOnly,
            },
            super::RuntimeToolDefinition {
                name: "MCPTool".to_string(),
                description: Some("Call a configured MCP tool".to_string()),
                input_schema: json!({ "type": "object" }),
                required_permission: PermissionMode::DangerFullAccess,
            },
        ])
        .expect("runtime tools register");
    let names = advertised_names(&registry);
    assert!(names.contains("MCPTool"), "the wrapper stays advertised");
    assert!(!names.contains("mcp__deferral_probe__navigate"));
    assert!(!names.contains("mcp__deferral_probe__click"));
    assert!(registry.has_runtime_tool("mcp__deferral_probe__navigate"));

    let found = registry.search("select:mcp__deferral_probe__navigate", 5, None, None);
    assert_eq!(
        found.matches,
        vec!["mcp__deferral_probe__navigate".to_string()]
    );
    let names = advertised_names(&registry);
    assert!(names.contains("mcp__deferral_probe__navigate"));
    assert!(!names.contains("mcp__deferral_probe__click"));

    // An explicit allow-list still advertises a listed MCP tool.
    let allowed = BTreeSet::from(["mcp__deferral_probe__click".to_string()]);
    let listed = registry
        .definitions(Some(&allowed))
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(listed, vec!["mcp__deferral_probe__click".to_string()]);
}

/// Serves `responses` (one per connection, in order) on a loopback port.
fn serve(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let handle = thread::spawn(move || {
        let mut request_lines = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .expect("read timeout");
            let mut buffer = [0_u8; 4096];
            let size = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..size]).into_owned();
            request_lines.push(request.lines().next().unwrap_or_default().to_string());
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
        request_lines
    });
    (base, handle)
}

fn http_response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn web_fetch_refuses_local_private_and_non_http_targets() {
    for url in [
        "http://127.0.0.1:9/admin",
        "http://localhost:9/",
        "http://[::1]:9/",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://192.168.1.1/",
        "http://2130706433/",
        "file:///etc/passwd",
        // Spellings whose fetched host could differ from the approved one.
        "https://docs%2Ers/guide",
        "https:docs.rs/",
        "https:\\\\docs.rs/",
    ] {
        let error = execute_tool("WebFetch", &json!({ "url": url, "prompt": "read" }))
            .expect_err("local targets must be refused");
        assert!(
            error.contains("WebFetch refused") || error.contains("only supports http"),
            "{url}: {error}"
        );
    }
}

#[test]
fn web_fetch_validates_every_redirect_hop() {
    super::web_fetch::allow_loopback_on_this_thread();
    let (base, server) = serve(vec![
        http_response("302 Found", "Location: /final\r\n", ""),
        http_response(
            "200 OK",
            "Content-Type: text/plain\r\n",
            "redirected content",
        ),
        http_response(
            "302 Found",
            "Location: http://169.254.169.254/latest/meta-data/\r\n",
            "",
        ),
    ]);

    let followed = parse(
        &execute_tool(
            "WebFetch",
            &json!({ "url": format!("{base}/hop"), "prompt": "read" }),
        )
        .expect("a redirect to a public path is followed"),
    );
    assert_eq!(followed["url"], format!("{base}/final"));
    assert!(followed["result"]
        .as_str()
        .is_some_and(|result| result.contains("redirected content")));

    let error = execute_tool(
        "WebFetch",
        &json!({ "url": format!("{base}/escape"), "prompt": "read" }),
    )
    .expect_err("a redirect into link-local space must be refused");
    assert!(error.contains("link-local"), "{error}");

    let requests = server.join().expect("server thread");
    assert_eq!(
        requests,
        vec![
            "GET /hop HTTP/1.1",
            "GET /final HTTP/1.1",
            "GET /escape HTTP/1.1"
        ]
    );
}

#[test]
fn web_fetch_reports_redirects_to_another_host_instead_of_following_them() {
    super::web_fetch::allow_loopback_on_this_thread();
    // The fixture listens on 127.0.0.1 and redirects to `localhost`, a
    // different host name (like an approved domain's open redirect). The
    // target is never contacted.
    let (base, server) = serve(vec![http_response(
        "302 Found",
        "Location: http://localhost:9/leak?data=secret\r\n",
        "",
    )]);

    let output = parse(
        &execute_tool(
            "WebFetch",
            &json!({ "url": format!("{base}/redirect"), "prompt": "read" }),
        )
        .expect("a cross-host redirect is reported"),
    );
    let result = output["result"].as_str().expect("result text");
    assert!(
        result.contains("redirected to a different host"),
        "{result}"
    );
    assert!(result.contains("/leak?data=secret"), "{result}");
    assert!(result.contains("call WebFetch again"), "{result}");
    assert_eq!(output["code"], 302);
    // Only the original request was made.
    assert_eq!(
        server.join().expect("server thread"),
        vec!["GET /redirect HTTP/1.1"]
    );
}

#[test]
fn web_fetch_returns_readable_text_up_to_20000_characters() {
    super::web_fetch::allow_loopback_on_this_thread();
    let body = format!(
        "<html><head><title>Big page</title><script>var hidden = 1;</script></head><body><p>{}</p><p>tail-marker</p></body></html>",
        "word ".repeat(6_000)
    );
    let (base, server) = serve(vec![http_response(
        "200 OK",
        "Content-Type: text/html; charset=utf-8\r\n",
        &body,
    )]);

    let output = parse(
        &execute_tool(
            "WebFetch",
            &json!({ "url": format!("{base}/big"), "prompt": "summarize" }),
        )
        .expect("fetch succeeds"),
    );
    server.join().expect("server thread");
    let result = output["result"].as_str().expect("result text");
    assert!(result.contains("Title: Big page"), "{result}");
    assert!(result.contains("word word word"));
    assert!(!result.contains("hidden"));
    assert!(!result.contains("tail-marker"));
    assert!(result.contains("[Content truncated: showing the first 20000 of"));
    assert!(result.chars().count() < 20_000 + 500);
    // Far more than the old 900-character preview.
    assert!(result.chars().count() > 19_000);
}
