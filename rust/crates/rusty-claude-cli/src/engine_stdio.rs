//! `neuron engine --stdio`: the engine for a UI in another process. JSON lines both ways:
//! [`ClientMessage`]s on stdin, [`UiEvent`]s on stdout, one per line, flushed after each.
//! Nothing else goes to stdout; logs and panics go to stderr. The first event is
//! [`UiEvent::Ready`]; a launch that fails sends one [`UiEvent::Error`] instead and exits
//! with status 1. End of input shuts the engine down (a running turn is interrupted
//! first) and the process exits with status 0, or 130 after a forced quit (two
//! interrupts during a turn).
//!
//! There is no trust prompt: like a one-shot prompt with piped input, an untrusted
//! workspace runs without its own settings, hooks and MCP servers
//! (`UiEvent::Status::workspace_trusted` tells the UI). There is no sign-in prompt either:
//! when Zero-X is the provider and nobody is signed in (or the session ended), the engine
//! starts signed out (`Status::signed_in` is false) and `/login` signs in from the UI.

// Stdout carries only the protocol.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::*;
use crate::engine::{EngineExit, EngineHandle, EngineOptions};
use neuron_tui::protocol::{ClientMessage, EngineCommand, PermissionReply, UiEvent};
use std::io::{BufRead, BufReader};

const USAGE: &str = "usage: neuron engine --stdio [--model MODEL] [--permission-mode MODE] \
[--allowedTools TOOLS] [--reasoning-effort low|medium|high] [--resume SESSION] [--allow-broad-cwd]";

/// The stdio engine's flags when `args` ask for it: `[flags] engine [flags]` with
/// `--stdio` among them, so the engine's flags may come before `engine` as global options
/// do. `None` leaves `args` to the usual parsing, where `neuron engine ...` is a prompt
/// (`neuron fix the engine --stdio flag` is one).
pub(crate) fn stdio_engine_args(args: &[String]) -> Option<Vec<String>> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "engine")
        .find_map(|(position, _)| {
            let before = &args[..position];
            parse_flags(before).ok()?;
            let flags = before
                .iter()
                .chain(&args[position + 1..])
                .cloned()
                .collect::<Vec<_>>();
            flags.iter().any(|flag| flag == "--stdio").then_some(flags)
        })
}

/// Runs the stdio engine until its input ends.
pub(crate) fn run_engine_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // `ready` must not wait for the MCP servers.
    crate::progress::set_mcp_startup(crate::progress::McpStartup::Background);
    let handle = match start(args) {
        Ok(handle) => handle,
        Err(message) => {
            // The UI reads stdout: it hears why. `main` logs the error on stderr too.
            let _ = write_event(
                &mut io::stdout(),
                &UiEvent::Error {
                    message: message.clone(),
                    detail: None,
                },
            );
            return Err(message.into());
        }
    };
    match serve(handle, BufReader::new(io::stdin()), &mut io::stdout()) {
        EngineExit::Shutdown => Ok(()),
        // As after Ctrl-C twice in the line REPL; the session is saved.
        EngineExit::ForceQuit => std::process::exit(130),
    }
}

/// Flags of `neuron engine`: the line REPL's launch flags that make sense without a
/// terminal.
#[derive(Debug, Default, PartialEq, Eq)]
struct EngineArgs {
    stdio: bool,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
    allowed_tools: Vec<String>,
    reasoning_effort: Option<String>,
    resume: Option<String>,
    allow_broad_cwd: bool,
}

fn parse_engine_args(args: &[String]) -> Result<EngineArgs, String> {
    let parsed = parse_flags(args)?;
    if !parsed.stdio {
        return Err(format!("`neuron engine` needs --stdio\n{USAGE}"));
    }
    Ok(parsed)
}

fn parse_flags(args: &[String]) -> Result<EngineArgs, String> {
    let mut parsed = EngineArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let switch = inline.is_none();
        let mut value = || {
            inline
                .clone()
                .or_else(|| rest.next().cloned())
                .ok_or_else(|| format!("missing value for {flag}\n{USAGE}"))
        };
        match flag {
            "--stdio" if switch => parsed.stdio = true,
            "--model" => parsed.model = Some(value()?),
            "--permission-mode" => {
                parsed.permission_mode = Some(parse_permission_mode_arg(&value()?)?);
            }
            "--dangerously-skip-permissions" if switch => {
                parsed.permission_mode = Some(PermissionMode::DangerFullAccess);
            }
            "--allowedTools" | "--allowed-tools" => parsed.allowed_tools.push(value()?),
            "--reasoning-effort" => {
                let effort = value()?;
                if !matches!(effort.as_str(), "low" | "medium" | "high") {
                    return Err(format!(
                        "invalid value for --reasoning-effort: '{effort}'; must be low, medium, or high"
                    ));
                }
                parsed.reasoning_effort = Some(effort);
            }
            "--resume" => parsed.resume = Some(value()?),
            "--allow-broad-cwd" if switch => parsed.allow_broad_cwd = true,
            _ => {
                return Err(format!(
                    "unknown argument for `neuron engine`: {arg}\n{USAGE}"
                ))
            }
        }
    }
    Ok(parsed)
}

/// The line REPL's launch, minus the terminal: model and provider resolved the same way,
/// a too-broad working directory refused instead of asked about.
fn start(args: &[String]) -> Result<EngineHandle, String> {
    let args = parse_engine_args(args)?;
    if !args.allow_broad_cwd {
        if let Some(cwd) = detect_broad_cwd() {
            return Err(format!(
                "neuron is running from a very broad directory ({}). The agent can read and \
                 search everything under this path. Pass --allow-broad-cwd to proceed anyway, \
                 or start it from inside your project.",
                cwd.display()
            ));
        }
    }
    let requested = requested_model(&args, |reference| {
        saved_session_model(&current_session_store().ok()?, reference)
    });
    // Signed out of Zero-X (or the session ended): start anyway, and the UI signs in.
    let (model, signed_out) = match start_provider(&requested).map_err(|error| error.to_string())? {
        ProviderStart::Ready(model) => (model, None),
        ProviderStart::SignInRequired(reason) => (requested, Some(reason)),
    };
    let permission_mode = args.permission_mode.unwrap_or_else(default_permission_mode);
    let allowed_tools = normalize_allowed_tools(&args.allowed_tools)?;
    crate::engine::spawn(EngineOptions {
        model,
        allowed_tools,
        permission_mode,
        reasoning_effort: args.reasoning_effort,
        resume: args.resume,
        signed_out,
    })
}

/// The model to start on: `--model`, else the one the chat `--resume` continues last ran
/// on (as the line REPL's `--resume` does), else the default.
fn requested_model(args: &EngineArgs, saved_model: impl FnOnce(&str) -> Option<String>) -> String {
    let model = match (&args.model, &args.resume) {
        (Some(model), _) => resolve_model_alias_with_config(model),
        (None, Some(reference)) => {
            saved_model(reference).unwrap_or_else(|| DEFAULT_MODEL.to_string())
        }
        (None, None) => DEFAULT_MODEL.to_string(),
    };
    resolve_repl_model(model)
}

/// The model stored with a saved chat. Provider setup adds the `openai/` routing prefix
/// back where the transport needs it.
fn saved_session_model(store: &runtime::SessionStore, reference: &str) -> Option<String> {
    let (_, session) = load_session_reference_in_store(store, reference).ok()?;
    session.model.filter(|model| !model.trim().is_empty())
}

/// Pumps JSON lines until the engine stops: input lines go to the engine from a reader
/// thread, events go to `output` from here.
fn serve(
    handle: EngineHandle,
    input: impl BufRead + Send + 'static,
    output: &mut impl Write,
) -> EngineExit {
    let commands = Sender::clone(&handle.commands);
    let replies = Sender::clone(&handle.replies);
    // Not joined: after the engine stops on its own (`/exit`) the reader may still wait
    // for input, and the process ends without it.
    let reader = thread::Builder::new()
        .name("neuron-engine-stdin".to_string())
        .spawn(move || read_messages(input, &commands, &replies));
    if reader.is_err() {
        let _ = handle.commands.send(EngineCommand::Shutdown);
    }
    let mut output_open = true;
    for event in &handle.events {
        if output_open && write_event(output, &event).is_err() {
            // The UI is gone: stop, and keep draining until the session is saved.
            output_open = false;
            let _ = handle.commands.send(EngineCommand::Shutdown);
        }
    }
    handle.finish()
}

/// Delivers input lines until the input ends, which shuts the engine down. A line that is
/// not a message is reported as a notice and skipped.
fn read_messages(
    input: impl BufRead,
    commands: &Sender<EngineCommand>,
    replies: &Sender<PermissionReply>,
) {
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                runtime::ui_hooks::notice(&format!(
                    "neuron engine: skipped an input line that is not UTF-8 ({error})"
                ));
                continue;
            }
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let delivered = match serde_json::from_str::<ClientMessage>(line) {
            Ok(ClientMessage::Command(command)) => commands.send(command).is_ok(),
            Ok(ClientMessage::PermissionReply(reply)) => replies.send(reply).is_ok(),
            Err(error) => {
                runtime::ui_hooks::notice(&format!(
                    "neuron engine: skipped an input line that is not a command or a permission reply ({error})"
                ));
                true
            }
        };
        if !delivered {
            // The engine stopped.
            return;
        }
    }
    let _ = commands.send(EngineCommand::Shutdown);
}

/// One event as one line, written in a single call (so a stray print elsewhere can never
/// split it) and flushed.
fn write_event(output: &mut impl Write, event: &UiEvent) -> io::Result<()> {
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    output.write_all(line.as_bytes())?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::{parse_engine_args, read_messages, stdio_engine_args, write_event, EngineArgs};
    use neuron_tui::protocol::{EngineCommand, PermissionDecision, PermissionReply, UiEvent};
    use runtime::PermissionMode;
    use std::sync::mpsc;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_the_launch_flags_in_both_spellings() {
        let parsed = parse_engine_args(&args(&[
            "--stdio",
            "--model=opus",
            "--permission-mode",
            "read-only",
            "--allowedTools",
            "read_file,bash",
            "--reasoning-effort=high",
            "--resume",
            "latest",
            "--allow-broad-cwd",
        ]))
        .expect("parses");
        assert_eq!(
            parsed,
            EngineArgs {
                stdio: true,
                model: Some("opus".into()),
                permission_mode: Some(PermissionMode::ReadOnly),
                allowed_tools: vec!["read_file,bash".into()],
                reasoning_effort: Some("high".into()),
                resume: Some("latest".into()),
                allow_broad_cwd: true,
            }
        );
        let skip = parse_engine_args(&args(&["--dangerously-skip-permissions", "--stdio"]))
            .expect("parses");
        assert_eq!(skip.permission_mode, Some(PermissionMode::DangerFullAccess));

        for (bad, why) in [
            (&["--model", "opus"][..], "needs --stdio"),
            (&["--stdio", "--model"][..], "missing value for --model"),
            (
                &["--stdio", "--reasoning-effort", "max"][..],
                "invalid value",
            ),
            (&["--stdio", "--frobnicate"][..], "unknown argument"),
            (&["--stdio=yes"][..], "unknown argument"),
        ] {
            let error = parse_engine_args(&args(bad)).expect_err("rejected");
            assert!(error.contains(why), "{bad:?}: {error}");
        }
    }

    /// The engine's flags may come before `engine`; a prompt that mentions it stays a
    /// prompt.
    #[test]
    fn finds_the_engine_with_its_flags_on_either_side() {
        let engine = |values: &[&str]| stdio_engine_args(&args(values));
        assert_eq!(engine(&["engine", "--stdio"]), Some(args(&["--stdio"])));
        assert_eq!(
            engine(&["--model", "opus", "engine", "--stdio", "--allow-broad-cwd"]),
            Some(args(&["--model", "opus", "--stdio", "--allow-broad-cwd"]))
        );
        assert_eq!(
            engine(&["--model", "engine", "engine", "--stdio"]),
            Some(args(&["--model", "engine", "--stdio"])),
            "a model named engine is a value, not the subcommand"
        );
        assert_eq!(engine(&["fix", "the", "engine", "--stdio", "flag"]), None);
        assert_eq!(engine(&["engine", "is", "slow"]), None);
        assert_eq!(engine(&["--model", "opus", "explain", "this"]), None);
    }

    /// `--resume` without `--model` goes on with the model the chat was using; `--model`
    /// still wins.
    #[test]
    fn a_resumed_chat_starts_on_the_model_it_was_using() {
        let workspace =
            std::env::temp_dir().join(format!("neuron-engine-resume-model-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        let store = crate::session_mgmt::session_store_in(&workspace).expect("store");
        let handle = store.create_handle("chat-on-groq");
        let mut session = runtime::Session::new().with_persistence_path(handle.path.clone());
        session.model = Some("groq/openai/gpt-oss-20b".into());
        // `latest` is the newest chat with messages.
        session.push_user_text("hello").expect("message");
        session.save_to_path(&handle.path).expect("save");

        let saved = |reference: &str| super::saved_session_model(&store, reference);
        assert_eq!(
            saved("chat-on-groq").as_deref(),
            Some("groq/openai/gpt-oss-20b")
        );
        assert_eq!(saved("latest").as_deref(), Some("groq/openai/gpt-oss-20b"));
        assert_eq!(saved("no-such-chat"), None);

        let resumed = parse_engine_args(&args(&["--stdio", "--resume", "chat-on-groq"])).unwrap();
        assert_eq!(
            super::requested_model(&resumed, saved),
            "groq/openai/gpt-oss-20b"
        );
        let switched = parse_engine_args(&args(&[
            "--stdio",
            "--resume",
            "chat-on-groq",
            "--model",
            "zx-fast",
        ]))
        .unwrap();
        assert_eq!(super::requested_model(&switched, saved), "zx-fast");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn input_lines_become_commands_and_replies_and_the_end_shuts_down() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (replies_tx, replies_rx) = mpsc::channel();
        let input = "\n{\"kind\":\"command\",\"type\":\"submit\",\"text\":\"hi\",\"attachments\":[]}\r\n\
                     not json\n\
                     {\"kind\":\"permission_reply\",\"id\":2,\"decision\":{\"kind\":\"allow_once\"}}\n";
        read_messages(input.as_bytes(), &commands_tx, &replies_tx);
        assert_eq!(
            commands_rx.try_iter().collect::<Vec<_>>(),
            vec![
                EngineCommand::Submit {
                    text: "hi".into(),
                    attachments: Vec::new(),
                },
                EngineCommand::Shutdown,
            ]
        );
        assert_eq!(
            replies_rx.try_iter().collect::<Vec<_>>(),
            vec![PermissionReply {
                id: 2,
                decision: PermissionDecision::AllowOnce,
            }]
        );
    }

    #[test]
    fn each_event_is_one_flushed_json_line() {
        let mut out = Vec::new();
        write_event(
            &mut out,
            &UiEvent::TextDelta {
                text: "a\nb".into(),
            },
        )
        .unwrap();
        write_event(&mut out, &UiEvent::TurnFinished).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"type\":\"text_delta\",\"text\":\"a\\nb\"}\n{\"type\":\"turn_finished\"}\n"
        );
    }
}
