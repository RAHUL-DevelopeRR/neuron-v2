use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::ContentBlock;
use runtime::Session;
use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn resumed_binary_accepts_slash_commands_with_arguments() {
    // given
    let temp_dir = unique_temp_dir("resume-slash-commands");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let session_path = temp_dir.join("session.jsonl");
    let export_path = temp_dir.join("notes.txt");

    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("ship the slash command harness")
        .expect("session write should succeed");
    session
        .save_to_path(&session_path)
        .expect("session should persist");

    // when
    let output = run_claw(
        &temp_dir,
        &[
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/export",
            export_path.to_str().expect("utf8 path"),
            "/clear",
            "--confirm",
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Export"));
    assert!(stdout.contains("wrote transcript"));
    assert!(stdout.contains(export_path.to_str().expect("utf8 path")));
    assert!(stdout.contains("Session cleared"));
    assert!(stdout.contains("Mode             resumed session reset"));
    assert!(stdout.contains("Previous session"));
    assert!(stdout.contains("Resume previous  neuron --resume"));
    assert!(stdout.contains("Backup           "));
    assert!(stdout.contains("Session file     "));

    let export = fs::read_to_string(&export_path).expect("export file should exist");
    assert!(export.contains("# Conversation Export"));
    assert!(export.contains("ship the slash command harness"));

    let restored = Session::load_from_path(&session_path).expect("cleared session should load");
    assert_eq!(restored.messages, []);

    let backup_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("  Backup           "))
        .map(PathBuf::from)
        .expect("clear output should include backup path");
    let backup = Session::load_from_path(&backup_path).expect("backup session should load");
    assert_eq!(backup.messages.len(), 1);
    assert!(matches!(
        backup.messages[0].blocks.first(),
        Some(ContentBlock::Text { text }) if text == "ship the slash command harness"
    ));
}

#[test]
fn status_command_applies_cli_flags_end_to_end() {
    // given
    let temp_dir = unique_temp_dir("status-command-flags");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    // when
    let output = run_claw(
        &temp_dir,
        &[
            "--model",
            "sonnet",
            "--permission-mode",
            "read-only",
            "status",
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Status"));
    assert!(stdout.contains("Model            claude-sonnet-4-6"));
    assert!(stdout.contains("Permission mode  read-only"));
}

#[test]
fn resumed_config_command_loads_settings_files_end_to_end() {
    // given
    let temp_dir = unique_temp_dir("resume-config");
    let project_dir = temp_dir.join("project");
    let config_home = temp_dir.join("home").join(".claw");
    fs::create_dir_all(project_dir.join(".claw")).expect("project config dir should exist");
    fs::create_dir_all(&config_home).expect("config home should exist");

    let session_path = project_dir.join("session.jsonl");
    workspace_session(&project_dir)
        .with_persistence_path(&session_path)
        .save_to_path(&session_path)
        .expect("session should persist");

    fs::write(config_home.join("settings.json"), r#"{"model":"haiku"}"#)
        .expect("user config should write");
    fs::write(
        project_dir.join(".claw").join("settings.local.json"),
        r#"{"model":"opus"}"#,
    )
    .expect("local config should write");

    // when
    let output = run_claw_with_env(
        &project_dir,
        &[
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/config",
            "model",
        ],
        &[
            ("CLAW_CONFIG_HOME", config_home.to_str().expect("utf8 path")),
            // Workspace settings only apply to a trusted workspace.
            ("NEURON_TRUST_WORKSPACE", "1"),
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Config"));
    assert!(stdout.contains("Loaded files      2"));
    assert!(stdout.contains(
        config_home
            .join("settings.json")
            .to_str()
            .expect("utf8 path")
    ));
    assert!(stdout.contains(
        project_dir
            .join(".claw")
            .join("settings.local.json")
            .to_str()
            .expect("utf8 path")
    ));
    assert!(stdout.contains("Merged section: model"));
    assert!(stdout.contains("opus"));
}

#[test]
fn resume_latest_restores_the_most_recent_managed_session() {
    // given
    let temp_dir = unique_temp_dir("resume-latest");
    let project_dir = temp_dir.join("project");
    let store = runtime::SessionStore::from_cwd(&project_dir).expect("session store should build");
    let older_path = store.create_handle("session-older").path;
    let newer_path = store.create_handle("session-newer").path;

    let mut older = workspace_session(&project_dir).with_persistence_path(&older_path);
    older
        .push_user_text("older session")
        .expect("older session write should succeed");
    older
        .save_to_path(&older_path)
        .expect("older session should persist");

    let mut newer = workspace_session(&project_dir).with_persistence_path(&newer_path);
    newer
        .push_user_text("newer session")
        .expect("newer session write should succeed");
    newer
        .push_user_text("resume me")
        .expect("newer session write should succeed");
    newer
        .save_to_path(&newer_path)
        .expect("newer session should persist");

    // when
    let output = run_claw(&project_dir, &["--resume", "latest", "/status"]);

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Status"));
    assert!(stdout.contains("Messages         2"));
    assert!(stdout.contains(newer_path.to_str().expect("utf8 path")));
}

#[test]
fn resumed_status_command_emits_structured_json_when_requested() {
    // given
    let temp_dir = unique_temp_dir("resume-status-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");

    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("resume status json fixture")
        .expect("session write should succeed");
    session
        .save_to_path(&session_path)
        .expect("session should persist");

    // when
    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/status",
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let parsed: Value =
        serde_json::from_str(stdout.trim()).expect("resume status output should be json");
    assert_eq!(parsed["kind"], "status");
    // model is null in resume mode (not known without --model flag)
    assert!(parsed["model"].is_null());
    assert_eq!(parsed["permission_mode"], "workspace-write");
    assert_eq!(parsed["usage"]["messages"], 1);
    assert!(parsed["usage"]["turns"].is_number());
    assert!(parsed["workspace"]["cwd"].as_str().is_some());
    assert_eq!(
        parsed["workspace"]["session"],
        session_path.to_str().expect("utf8 path")
    );
    assert!(parsed["workspace"]["changed_files"].is_number());
    assert_eq!(parsed["workspace"]["loaded_config_files"].as_u64(), Some(0));
    assert!(parsed["sandbox"]["filesystem_mode"].as_str().is_some());
}

#[test]
fn resumed_status_surfaces_persisted_model() {
    // given — create a session with model already set
    let temp_dir = unique_temp_dir("resume-status-model");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");

    let mut session = workspace_session(&temp_dir);
    session.model = Some("claude-sonnet-4-6".to_string());
    session
        .push_user_text("model persistence fixture")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");

    // when
    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/status",
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "status");
    assert_eq!(
        parsed["model"], "claude-sonnet-4-6",
        "model should round-trip through session metadata"
    );
}

#[test]
fn resumed_sandbox_command_emits_structured_json_when_requested() {
    // given
    let temp_dir = unique_temp_dir("resume-sandbox-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");

    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("session should persist");

    // when
    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/sandbox",
        ],
    );

    // then
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let parsed: Value =
        serde_json::from_str(stdout.trim()).expect("resume sandbox output should be json");
    assert_eq!(parsed["kind"], "sandbox");
    assert!(parsed["enabled"].is_boolean());
    assert!(parsed["active"].is_boolean());
    assert!(parsed["supported"].is_boolean());
    assert!(parsed["filesystem_mode"].as_str().is_some());
    assert!(parsed["allowed_mounts"].is_array());
    assert!(parsed["markers"].is_array());
}

#[test]
fn resumed_version_command_emits_structured_json() {
    let temp_dir = unique_temp_dir("resume-version-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("session should persist");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/version",
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "version");
    assert!(parsed["version"].as_str().is_some());
    assert!(parsed["git_sha"].as_str().is_some());
    assert!(parsed["target"].as_str().is_some());
}

#[test]
fn resumed_export_command_emits_structured_json() {
    let temp_dir = unique_temp_dir("resume-export-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("export json fixture")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/export",
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "export");
    assert!(parsed["file"].as_str().is_some());
    assert_eq!(parsed["message_count"], 1);
}

#[test]
fn resumed_help_command_emits_structured_json() {
    let temp_dir = unique_temp_dir("resume-help-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("persist ok");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/help",
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "help");
    assert!(parsed["text"].as_str().is_some());
    let text = parsed["text"].as_str().unwrap();
    assert!(text.contains("/status"), "help text should list /status");
}

#[test]
fn resumed_no_command_emits_restored_json() {
    let temp_dir = unique_temp_dir("resume-no-cmd-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("restored json fixture")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "restored");
    assert!(parsed["session_id"].as_str().is_some());
    assert!(parsed["path"].as_str().is_some());
    assert_eq!(parsed["message_count"], 1);
}

#[test]
fn resume_without_terminal_prints_summary_instead_of_entering_the_repl() {
    // given a saved session and a non-interactive stdin/stdout
    let temp_dir = unique_temp_dir("resume-no-tty");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("keep going later")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");

    // when
    let output = run_claw(
        &temp_dir,
        &["--resume", session_path.to_str().expect("utf8 path")],
    );

    // then the scriptable summary is printed and the process exits
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("Restored session from"));
    assert!(stdout.contains("(1 messages)"));
}

#[test]
fn resumed_context_command_reports_window_and_usage() {
    let temp_dir = unique_temp_dir("resume-context-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    let mut session = workspace_session(&temp_dir);
    session.model = Some("gemini/gemini-2.5-flash".to_string());
    session
        .push_user_text("how big is my context?")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/context",
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("should be json");
    assert_eq!(parsed["kind"], "context");
    assert_eq!(parsed["context_window"], 1_048_576);
    assert_eq!(parsed["messages"], 1);
    assert!(parsed["estimated_tokens"]
        .as_u64()
        .is_some_and(|tokens| tokens > 0));
    assert!(parsed["last_compaction"].is_null());
}

/// `/context` only shows the report: `clear` and unknown arguments exit 2 with the
/// session's message instead of printing the report as if they had worked.
#[test]
fn resumed_context_rejects_clear_and_unknown_arguments() {
    let temp_dir = unique_temp_dir("resume-context-args");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("persist ok");
    let session_arg = session_path.to_str().expect("utf8 path");

    for (argument, expected) in [
        ("clear", "/context clear is not supported; use /compact"),
        ("bogus", "Usage: /context [show]"),
    ] {
        let command = format!("/context {argument}");
        let output = run_claw(&temp_dir, &["--resume", session_arg, &command]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        assert!(output.stdout.is_empty(), "{command} printed the report");
        let stderr = String::from_utf8(output.stderr).expect("utf8");
        assert!(stderr.contains(expected), "{stderr}");

        let output = run_claw(
            &temp_dir,
            &["--output-format", "json", "--resume", session_arg, &command],
        );
        assert_eq!(output.status.code(), Some(2), "{command}");
        assert!(output.stdout.is_empty(), "{command} printed the report");
        let stderr = String::from_utf8(output.stderr).expect("utf8");
        let parsed: Value = serde_json::from_str(stderr.trim()).expect("should be json");
        assert_eq!(parsed["type"], "error");
        assert!(
            parsed["error"]
                .as_str()
                .is_some_and(|error| error.contains(expected)),
            "{parsed}"
        );
    }
}

/// `/skills install` under `--resume` installs once and reports it, in text and JSON;
/// it used to install for the text and fail with "already installed" for the JSON.
#[test]
fn resumed_skills_install_installs_once() {
    let temp_dir = unique_temp_dir("resume-skills-install");
    let config_home = temp_dir.join("config-home");
    for name in ["text-skill", "json-skill"] {
        let source = temp_dir.join(name);
        fs::create_dir_all(&source).expect("skill dir should exist");
        fs::write(
            source.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: resume test\n---\n\n# {name}\n"),
        )
        .expect("skill file should write");
    }
    let session_path = temp_dir.join("session.jsonl");
    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("persist ok");
    let session_arg = session_path.to_str().expect("utf8 path");
    let envs = [(
        "NEURON_CONFIG_HOME",
        config_home.to_str().expect("utf8 path"),
    )];

    let output = run_claw_with_env(
        &temp_dir,
        &["--resume", session_arg, "/skills install text-skill"],
        &envs,
    );
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("installed text-skill"), "{stdout}");

    let output = run_claw_with_env(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_arg,
            "/skills install json-skill",
        ],
        &envs,
    );
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value =
        serde_json::from_str(String::from_utf8(output.stdout).expect("utf8").trim())
            .expect("should be json");
    assert_eq!(parsed["action"], "install");
    assert_eq!(parsed["result"], "installed");
    assert_eq!(parsed["invocation_name"], "json-skill");
    for name in ["text-skill", "json-skill"] {
        assert!(config_home
            .join("skills")
            .join(name)
            .join("SKILL.md")
            .is_file());
    }
}

#[test]
fn resume_tolerates_a_session_file_with_a_truncated_record() {
    let temp_dir = unique_temp_dir("resume-truncated-record");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    let mut session = workspace_session(&temp_dir);
    session
        .push_user_text("survives a crash")
        .expect("write ok");
    session.save_to_path(&session_path).expect("persist ok");
    let mut contents = fs::read_to_string(&session_path).expect("read session");
    contents.push_str("{\"type\":\"message\",\"message\":{\"role\":\"assist");
    fs::write(&session_path, contents).expect("append truncated record");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
        ],
    );

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value =
        serde_json::from_str(String::from_utf8(output.stdout).expect("utf8").trim())
            .expect("should be json");
    assert_eq!(parsed["message_count"], 1);
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("skipped 1 malformed record(s)"), "{stderr}");
}

#[test]
fn resumed_stub_command_emits_not_implemented_json() {
    let temp_dir = unique_temp_dir("resume-stub-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let session_path = temp_dir.join("session.jsonl");
    workspace_session(&temp_dir)
        .save_to_path(&session_path)
        .expect("persist ok");

    let output = run_claw(
        &temp_dir,
        &[
            "--output-format",
            "json",
            "--resume",
            session_path.to_str().expect("utf8 path"),
            "/allowed-tools",
        ],
    );

    // Stub commands exit with code 2
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    let parsed: Value = serde_json::from_str(stderr.trim()).expect("should be json");
    assert_eq!(parsed["type"], "error");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap()
            .contains("not yet implemented"),
        "error should say not yet implemented: {:?}",
        parsed["error"]
    );
}

fn run_claw(current_dir: &Path, args: &[&str]) -> Output {
    run_claw_with_env(current_dir, args, &[])
}

fn workspace_session(root: &Path) -> Session {
    Session::new().with_workspace_root(root.to_path_buf())
}

/// Runs the binary with an isolated home and config directory so user-level
/// settings (`~/.neuron/settings.json`, `~/.claw*`), stored credentials and
/// model overrides on the developer's machine cannot leak into assertions.
/// Explicit `envs` override the isolated defaults.
fn run_claw_with_env(current_dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let home = unique_temp_dir("isolated-home");
    let config_home = home.join(".neuron");
    fs::create_dir_all(&config_home).expect("isolated config home should exist");

    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command.current_dir(current_dir).args(args);
    for key in [
        "NEURON_CONFIG_HOME",
        "NEURON_TOKEN",
        "NEURON_MODEL",
        "NEURON_API_BASE",
        "ANTHROPIC_MODEL",
        "RUSTY_CLAUDE_PERMISSION_MODE",
    ] {
        command.env_remove(key);
    }
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", home.join("AppData").join("Local"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CLAW_CONFIG_HOME", &config_home);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("claw should launch");
    let _ = fs::remove_dir_all(&home);
    output
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_millis();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "claw-{label}-{}-{millis}-{counter}",
        std::process::id()
    ))
}
