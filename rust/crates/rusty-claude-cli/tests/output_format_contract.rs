use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
use runtime::Session;
use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn help_emits_json_when_requested() {
    let root = unique_temp_dir("help-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let parsed = assert_json_command(&root, &["--output-format", "json", "help"]);
    assert_eq!(parsed["kind"], "help");
    assert!(parsed["message"]
        .as_str()
        .expect("help text")
        .contains("Usage:"));
}

#[test]
fn version_emits_json_when_requested() {
    let root = unique_temp_dir("version-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let parsed = assert_json_command(&root, &["--output-format", "json", "version"]);
    assert_eq!(parsed["kind"], "version");
    assert_eq!(parsed["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn status_and_sandbox_emit_json_when_requested() {
    let root = unique_temp_dir("status-sandbox-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let status = assert_json_command(&root, &["--output-format", "json", "status"]);
    assert_eq!(status["kind"], "status");
    assert!(status["workspace"]["cwd"].as_str().is_some());

    let sandbox = assert_json_command(&root, &["--output-format", "json", "sandbox"]);
    assert_eq!(sandbox["kind"], "sandbox");
    assert!(sandbox["filesystem_mode"].as_str().is_some());
}

#[test]
fn acp_guidance_emits_json_when_requested() {
    let root = unique_temp_dir("acp-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let acp = assert_json_command(&root, &["--output-format", "json", "acp"]);
    assert_eq!(acp["kind"], "acp");
    assert_eq!(acp["status"], "discoverability_only");
    assert_eq!(acp["supported"], false);
    assert_eq!(acp["serve_alias_only"], true);
    assert_eq!(acp["discoverability_tracking"], "ROADMAP #64a");
    assert_eq!(acp["tracking"], "ROADMAP #76");
    assert!(acp["message"]
        .as_str()
        .expect("acp message")
        .contains("discoverability alias"));
}

#[test]
fn inventory_commands_emit_structured_json_when_requested() {
    let root = unique_temp_dir("inventory-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let isolated_home = root.join("home");
    let isolated_config = root.join("config-home");
    let isolated_codex = root.join("codex-home");
    fs::create_dir_all(&isolated_home).expect("isolated home should exist");

    let agents = assert_json_command_with_env(
        &root,
        &["--output-format", "json", "agents"],
        &[
            ("HOME", isolated_home.to_str().expect("utf8 home")),
            (
                "CLAW_CONFIG_HOME",
                isolated_config.to_str().expect("utf8 config home"),
            ),
            (
                "CODEX_HOME",
                isolated_codex.to_str().expect("utf8 codex home"),
            ),
        ],
    );
    assert_eq!(agents["kind"], "agents");
    assert_eq!(agents["action"], "list");
    assert_eq!(agents["count"], 0);
    assert_eq!(agents["summary"]["active"], 0);
    assert_eq!(agents["agents"], serde_json::json!([]));

    let mcp = assert_json_command(&root, &["--output-format", "json", "mcp"]);
    assert_eq!(mcp["kind"], "mcp");
    assert_eq!(mcp["action"], "list");

    let skills = assert_json_command(&root, &["--output-format", "json", "skills"]);
    assert_eq!(skills["kind"], "skills");
    assert_eq!(skills["action"], "list");
}

/// `neuron skills search` and `neuron skills install owner/repo/skill` talk to the
/// registry named by `NEURON_SKILL_REGISTRY_URL` (here a one-file server) and install
/// into Neuron's skills folder; `neuron skills` then lists the new skill.
#[test]
fn skills_search_and_install_from_a_registry() {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let body = if request.contains("/api/search") {
                r#"{"skills":[{"skillId":"triage","name":"triage","source":"acme/tools","installs":12}]}"#
            } else if request.contains("/api/download/acme/tools/triage") {
                r#"{"files":[{"path":"SKILL.md","contents":"---\nname: triage\ndescription: Sort new issues\n---\nSteps.\n"}]}"#
            } else {
                "{}"
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    let root = unique_temp_dir("skills-registry");
    fs::create_dir_all(&root).expect("root");
    let config_home = root.join("config-home");
    let envs = [
        ("NEURON_CONFIG_HOME", config_home.to_str().expect("utf8")),
        ("NEURON_SKILL_REGISTRY_URL", base.as_str()),
    ];

    let found = assert_json_command_with_env(
        &root,
        &["--output-format", "json", "skills", "search", "issue", "triage"],
        &envs,
    );
    assert_eq!(found["action"], "search");
    assert_eq!(found["query"], "issue triage");
    assert_eq!(found["results"][0]["id"], "acme/tools/triage");

    let installed = assert_json_command_with_env(
        &root,
        &["--output-format", "json", "skills", "install", "acme/tools/triage"],
        &envs,
    );
    assert_eq!(installed["invocation_name"], "triage");
    assert!(config_home
        .join("skills")
        .join("triage")
        .join("SKILL.md")
        .is_file());

    let listed =
        assert_json_command_with_env(&root, &["--output-format", "json", "skills"], &envs);
    assert!(
        listed["skills"]
            .as_array()
            .expect("skills")
            .iter()
            .any(|skill| skill["name"] == "triage"),
        "{listed}"
    );
    let _ = fs::remove_dir_all(root);
}

/// `neuron plugins ...` runs the plugin manager without a session or a model call:
/// install from a folder, list, disable, enable and uninstall, each in its own run.
/// Words that are no action print the usage and fail.
#[test]
fn plugins_subcommand_manages_plugins_without_a_session() {
    let root = unique_temp_dir("plugins-cli");
    let config_home = root.join("config-home");
    let plugin = root.join("tracker-kit");
    fs::create_dir_all(plugin.join(".neuron-plugin")).expect("plugin dir");
    fs::create_dir_all(plugin.join("skills").join("triage")).expect("skill dir");
    fs::write(
        plugin.join("skills").join("triage").join("SKILL.md"),
        "---\nname: triage\ndescription: Sort new issues\n---\nSteps.\n",
    )
    .expect("skill");
    fs::write(
        plugin.join(".neuron-plugin").join("plugin.json"),
        r#"{"name": "tracker-kit", "version": "1.2.0", "description": "Issue tools",
            "skills": "skills", "mcpServers": {"tracker": {"command": "node"}}}"#,
    )
    .expect("manifest");
    let envs = [(
        "NEURON_CONFIG_HOME",
        config_home.to_str().expect("utf8 config home"),
    )];
    let run = |args: &[&str]| assert_json_command_with_env(&root, args, &envs);

    let installed = run(&[
        "--output-format",
        "json",
        "plugins",
        "install",
        plugin.to_str().expect("utf8 plugin path"),
    ]);
    assert_eq!(installed["kind"], "plugin");
    assert_eq!(installed["action"], "install");
    let message = installed["message"].as_str().expect("message");
    assert!(
        message.contains("installed tracker-kit@external"),
        "{message}"
    );
    assert!(
        message.contains("tracker (wait for your approval"),
        "{message}"
    );
    assert!(message.contains("1 folder(s)"), "{message}");

    let listed = run(&["--output-format", "json", "plugins", "list"]);
    let message = listed["message"].as_str().expect("message");
    assert!(message.contains("tracker-kit"), "{message}");
    assert!(message.contains("enabled"), "{message}");

    let disabled = run(&[
        "--output-format",
        "json",
        "plugins",
        "disable",
        "tracker-kit",
    ]);
    assert!(disabled["message"]
        .as_str()
        .expect("message")
        .contains("disabled tracker-kit@external"));
    let mcp = run(&["--output-format", "json", "mcp"]);
    assert_eq!(mcp["needs_approval_servers"], serde_json::json!([]));

    run(&[
        "--output-format",
        "json",
        "plugins",
        "enable",
        "tracker-kit",
    ]);
    let mcp = run(&["--output-format", "json", "mcp"]);
    assert_eq!(
        mcp["needs_approval_servers"],
        serde_json::json!(["tracker"])
    );

    let removed = run(&[
        "--output-format",
        "json",
        "plugins",
        "uninstall",
        "tracker-kit",
    ]);
    assert!(removed["message"]
        .as_str()
        .expect("message")
        .contains("uninstalled tracker-kit@external"));

    let output = run_claw(&root, &["plugins", "frobnicate"], &envs);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage: neuron plugins"), "{stderr}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn agents_command_emits_structured_agent_entries_when_requested() {
    let root = unique_temp_dir("agents-json-populated");
    let workspace = root.join("workspace");
    let project_agents = workspace.join(".codex").join("agents");
    let home = root.join("home");
    let user_agents = home.join(".codex").join("agents");
    let isolated_config = root.join("config-home");
    let isolated_codex = root.join("codex-home");
    fs::create_dir_all(&workspace).expect("workspace should exist");
    write_agent(
        &project_agents,
        "planner",
        "Project planner",
        "gpt-5.4",
        "medium",
    );
    write_agent(
        &project_agents,
        "verifier",
        "Verification agent",
        "gpt-5.4-mini",
        "high",
    );
    write_agent(
        &user_agents,
        "planner",
        "User planner",
        "gpt-5.4-mini",
        "high",
    );

    let parsed = assert_json_command_with_env(
        &workspace,
        &["--output-format", "json", "agents"],
        &[
            ("HOME", home.to_str().expect("utf8 home")),
            (
                "CLAW_CONFIG_HOME",
                isolated_config.to_str().expect("utf8 config home"),
            ),
            (
                "CODEX_HOME",
                isolated_codex.to_str().expect("utf8 codex home"),
            ),
        ],
    );

    assert_eq!(parsed["kind"], "agents");
    assert_eq!(parsed["action"], "list");
    assert_eq!(parsed["count"], 3);
    assert_eq!(parsed["summary"]["active"], 2);
    assert_eq!(parsed["summary"]["shadowed"], 1);
    assert_eq!(parsed["agents"][0]["name"], "planner");
    assert_eq!(parsed["agents"][0]["source"]["id"], "project_claw");
    assert_eq!(parsed["agents"][0]["active"], true);
    assert_eq!(parsed["agents"][1]["name"], "verifier");
    assert_eq!(parsed["agents"][2]["name"], "planner");
    assert_eq!(parsed["agents"][2]["active"], false);
    assert_eq!(parsed["agents"][2]["shadowed_by"]["id"], "project_claw");
}

#[test]
fn bootstrap_and_system_prompt_emit_json_when_requested() {
    let root = unique_temp_dir("bootstrap-system-prompt-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let plan = assert_json_command(&root, &["--output-format", "json", "bootstrap-plan"]);
    assert_eq!(plan["kind"], "bootstrap-plan");
    assert!(plan["phases"].as_array().expect("phases").len() > 1);

    let prompt = assert_json_command(&root, &["--output-format", "json", "system-prompt"]);
    assert_eq!(prompt["kind"], "system-prompt");
    assert!(prompt["message"]
        .as_str()
        .expect("prompt text")
        .contains("interactive agent"));
}

#[test]
fn dump_manifests_and_init_emit_json_when_requested() {
    let root = unique_temp_dir("manifest-init-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let upstream = write_upstream_fixture(&root);
    let manifests = assert_json_command(
        &root,
        &[
            "--output-format",
            "json",
            "dump-manifests",
            "--manifests-dir",
            upstream.to_str().expect("utf8 upstream"),
        ],
    );
    assert_eq!(manifests["kind"], "dump-manifests");
    assert_eq!(manifests["commands"], 1);
    assert_eq!(manifests["tools"], 1);

    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace should exist");
    let init = assert_json_command(&workspace, &["--output-format", "json", "init"]);
    assert_eq!(init["kind"], "init");
    assert!(workspace.join("NEURON.md").exists());
}

#[test]
fn doctor_and_resume_status_emit_json_when_requested() {
    let root = unique_temp_dir("doctor-resume-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let doctor = assert_json_command(&root, &["--output-format", "json", "doctor"]);
    assert_eq!(doctor["kind"], "doctor");
    assert!(doctor["message"].is_string());
    let summary = doctor["summary"].as_object().expect("doctor summary");
    assert!(summary["ok"].as_u64().is_some());
    assert!(summary["warnings"].as_u64().is_some());
    assert!(summary["failures"].as_u64().is_some());

    let checks = doctor["checks"].as_array().expect("doctor checks");
    assert_eq!(checks.len(), 6);
    let check_names = checks
        .iter()
        .map(|check| {
            assert!(check["status"].as_str().is_some());
            assert!(check["summary"].as_str().is_some());
            assert!(check["details"].is_array());
            check["name"].as_str().expect("doctor check name")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        check_names,
        vec![
            "auth",
            "config",
            "install source",
            "workspace",
            "sandbox",
            "system"
        ]
    );

    let install_source = checks
        .iter()
        .find(|check| check["name"] == "install source")
        .expect("install source check");
    assert_eq!(
        install_source["official_repo"],
        "https://github.com/RAHUL-DevelopeRR/neuron-v2"
    );
    assert_eq!(
        install_source["recommended_install"],
        "npm install -g @zero-x.live/neuron or pip install --pre neuroncli"
    );
    assert!(install_source.get("deprecated_install").is_none());

    let workspace = checks
        .iter()
        .find(|check| check["name"] == "workspace")
        .expect("workspace check");
    assert!(workspace["cwd"].as_str().is_some());
    assert!(workspace["in_git_repo"].is_boolean());

    let sandbox = checks
        .iter()
        .find(|check| check["name"] == "sandbox")
        .expect("sandbox check");
    assert!(sandbox["filesystem_mode"].as_str().is_some());
    assert!(sandbox["enabled"].is_boolean());
    assert!(sandbox["fallback_reason"].is_null() || sandbox["fallback_reason"].is_string());

    let session_path = write_session_fixture(&root, "resume-json", Some("hello"));
    let resumed = assert_json_command(
        &root,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 session path"),
            "/status",
        ],
    );
    assert_eq!(resumed["kind"], "status");
    // model is null in resume mode (not known without --model flag)
    assert!(resumed["model"].is_null());
    assert_eq!(resumed["usage"]["messages"], 1);
    assert!(resumed["workspace"]["cwd"].as_str().is_some());
    assert!(resumed["sandbox"]["filesystem_mode"].as_str().is_some());
}

#[test]
fn resumed_inventory_commands_emit_structured_json_when_requested() {
    let root = unique_temp_dir("resume-inventory-json");
    let config_home = root.join("config-home");
    let home = root.join("home");
    fs::create_dir_all(&config_home).expect("config home should exist");
    fs::create_dir_all(&home).expect("home should exist");

    let session_path = write_session_fixture(&root, "resume-inventory-json", Some("inventory"));

    let mcp = assert_json_command_with_env(
        &root,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 session path"),
            "/mcp",
        ],
        &[
            (
                "CLAW_CONFIG_HOME",
                config_home.to_str().expect("utf8 config home"),
            ),
            ("HOME", home.to_str().expect("utf8 home")),
        ],
    );
    assert_eq!(mcp["kind"], "mcp");
    assert_eq!(mcp["action"], "list");
    assert!(mcp["servers"].is_array());

    let skills = assert_json_command_with_env(
        &root,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 session path"),
            "/skills",
        ],
        &[
            (
                "CLAW_CONFIG_HOME",
                config_home.to_str().expect("utf8 config home"),
            ),
            ("HOME", home.to_str().expect("utf8 home")),
        ],
    );
    assert_eq!(skills["kind"], "skills");
    assert_eq!(skills["action"], "list");
    assert!(skills["summary"]["total"].is_number());
    assert!(skills["skills"].is_array());
}

#[test]
fn resumed_version_and_init_emit_structured_json_when_requested() {
    let root = unique_temp_dir("resume-version-init-json");
    fs::create_dir_all(&root).expect("temp dir should exist");

    let session_path = write_session_fixture(&root, "resume-version-init-json", None);

    let version = assert_json_command(
        &root,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 session path"),
            "/version",
        ],
    );
    assert_eq!(version["kind"], "version");
    assert_eq!(version["version"], env!("CARGO_PKG_VERSION"));

    let init = assert_json_command(
        &root,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 session path"),
            "/init",
        ],
    );
    assert_eq!(init["kind"], "init");
    assert!(root.join("NEURON.md").exists());
}

/// `--resume X -p` in JSON: `usage` and `estimated_cost` are this run's, not the totals
/// of the session it continued, which are in `session_usage`.
#[test]
fn a_resumed_prompt_reports_its_own_usage_and_the_session_total() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let root = unique_temp_dir("resume-prompt-usage");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home");
    fs::write(root.join("settings.json"), r#"{"defaultMcpServers":false}"#).expect("settings");
    let prompt = format!("{SCENARIO_PREFIX}token_cost_reporting");
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
        command
            .current_dir(&root)
            .env_clear()
            .env("ANTHROPIC_API_KEY", "test-parity-key")
            .env("ANTHROPIC_BASE_URL", server.base_url())
            .env("CLAW_CONFIG_HOME", &root)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("NO_COLOR", "1")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(["--model", "sonnet", "--output-format", "json"])
            .args(args)
            .stdin(Stdio::null());
        // Windows networking needs SystemRoot even in a cleared environment.
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }
        let output = command.output().expect("neuron should run");
        assert!(
            output.status.success(),
            "stdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be valid json")
    };

    // Each turn of the scenario costs 1,000 input and 500 output tokens.
    let first = run(&["prompt", &prompt]);
    assert_eq!(first["usage"]["input_tokens"], 1_000, "{first}");
    assert_eq!(first["session_usage"]["input_tokens"], 1_000, "{first}");
    let session_id = first["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let resumed = run(&["--resume", &session_id, "-p", &prompt]);
    assert_eq!(resumed["session_id"], session_id, "{resumed}");
    assert_eq!(resumed["usage"]["input_tokens"], 1_000, "{resumed}");
    assert_eq!(resumed["usage"]["output_tokens"], 500, "{resumed}");
    assert_eq!(resumed["session_usage"]["input_tokens"], 2_000, "{resumed}");
    assert_eq!(
        resumed["session_usage"]["output_tokens"], 1_000,
        "{resumed}"
    );
    // The cost of this run alone, the same as the first run's.
    assert_eq!(resumed["estimated_cost_known"], true, "{resumed}");
    assert_eq!(
        resumed["estimated_cost"], first["estimated_cost"],
        "{resumed}"
    );
    let _ = fs::remove_dir_all(&root);
}

fn assert_json_command(current_dir: &Path, args: &[&str]) -> Value {
    assert_json_command_with_env(current_dir, args, &[])
}

fn assert_json_command_with_env(current_dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Value {
    let output = run_claw(current_dir, args, envs);
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be valid json")
}

fn run_claw(current_dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command.current_dir(current_dir).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("claw should launch")
}

fn write_upstream_fixture(root: &Path) -> PathBuf {
    let upstream = root.join("claw-code");
    let src = upstream.join("src");
    let entrypoints = src.join("entrypoints");
    fs::create_dir_all(&entrypoints).expect("upstream entrypoints dir should exist");
    fs::write(
        src.join("commands.ts"),
        "import FooCommand from './commands/foo'\n",
    )
    .expect("commands fixture should write");
    fs::write(
        src.join("tools.ts"),
        "import ReadTool from './tools/read'\n",
    )
    .expect("tools fixture should write");
    fs::write(
        entrypoints.join("cli.tsx"),
        "if (args[0] === '--version') {}\nstartupProfiler()\n",
    )
    .expect("cli fixture should write");
    upstream
}

fn write_session_fixture(root: &Path, session_id: &str, user_text: Option<&str>) -> PathBuf {
    let session_path = root.join("session.jsonl");
    let mut session = Session::new()
        .with_workspace_root(root.to_path_buf())
        .with_persistence_path(session_path.clone());
    session.session_id = session_id.to_string();
    if let Some(text) = user_text {
        session
            .push_user_text(text)
            .expect("session fixture message should persist");
    } else {
        session
            .save_to_path(&session_path)
            .expect("session fixture should persist");
    }
    session_path
}

fn write_agent(root: &Path, name: &str, description: &str, model: &str, reasoning: &str) {
    fs::create_dir_all(root).expect("agent root should exist");
    fs::write(
        root.join(format!("{name}.toml")),
        format!(
            "name = \"{name}\"\ndescription = \"{description}\"\nmodel = \"{model}\"\nmodel_reasoning_effort = \"{reasoning}\"\n"
        ),
    )
    .expect("agent fixture should write");
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_millis();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "claw-output-format-{label}-{}-{millis}-{counter}",
        std::process::id()
    ))
}
