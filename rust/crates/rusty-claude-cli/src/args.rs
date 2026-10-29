//! CLI argument parsing, action resolution, and slash command specs.
//!
//! CliAction, CliOutputFormat, parse_args, and all parse_* helpers.

use super::*;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliAction {
    DumpManifests {
        output_format: CliOutputFormat,
        manifests_dir: Option<PathBuf>,
    },
    BootstrapPlan {
        output_format: CliOutputFormat,
    },
    Agents {
        args: Option<String>,
        output_format: CliOutputFormat,
    },
    Mcp {
        args: Option<String>,
        output_format: CliOutputFormat,
    },
    Skills {
        args: Option<String>,
        output_format: CliOutputFormat,
    },
    Plugins {
        action: Option<String>,
        target: Option<String>,
        output_format: CliOutputFormat,
    },
    PrintSystemPrompt {
        cwd: PathBuf,
        date: String,
        output_format: CliOutputFormat,
    },
    Version {
        output_format: CliOutputFormat,
    },
    ResumeSession {
        session_path: PathBuf,
        commands: Vec<String>,
        output_format: CliOutputFormat,
        launch: ResumeLaunchOptions,
    },
    /// `neuron --resume SESSION prompt TEXT` (or `-p TEXT`): one more turn on a saved
    /// session, run as `neuron prompt` runs and saved to the same file.
    ResumePrompt {
        session_path: PathBuf,
        prompt: String,
        output_format: CliOutputFormat,
        launch: ResumeLaunchOptions,
        compact: bool,
    },
    Status {
        model: String,
        permission_mode: PermissionMode,
        output_format: CliOutputFormat,
    },
    Sandbox {
        output_format: CliOutputFormat,
    },
    Prompt {
        prompt: String,
        model: String,
        output_format: CliOutputFormat,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        compact: bool,
        base_commit: Option<String>,
        reasoning_effort: Option<String>,
        allow_broad_cwd: bool,
    },
    Doctor {
        output_format: CliOutputFormat,
    },
    Acp {
        output_format: CliOutputFormat,
    },
    State {
        output_format: CliOutputFormat,
    },
    Init {
        output_format: CliOutputFormat,
    },
    Export {
        session_reference: String,
        output_path: Option<PathBuf>,
        output_format: CliOutputFormat,
    },
    Repl {
        model: String,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        base_commit: Option<String>,
        reasoning_effort: Option<String>,
        allow_broad_cwd: bool,
    },
    HelpTopic(LocalHelpTopic),
    // prompt-mode formatting is only supported for non-interactive runs
    Help {
        output_format: CliOutputFormat,
    },
}

/// Launch flags given with `--resume` (before or after it), applied when the
/// resumed session continues interactively. `None` means "not given": the
/// session's stored model and the configured permission mode apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResumeLaunchOptions {
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub allowed_tools: Option<AllowedToolSet>,
    pub reasoning_effort: Option<String>,
    pub base_commit: Option<String>,
    pub allow_broad_cwd: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalHelpTopic {
    Status,
    Sandbox,
    Doctor,
    Acp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliOutputFormat {
    Text,
    Json,
}

impl CliOutputFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text or json)"
            )),
        }
    }
}

pub(crate) fn parse_args(args: &[String]) -> Result<CliAction, String> {
    parse_args_with_stdin(args, read_piped_stdin)
}

/// Standard input read to the end when it is not a terminal (a pipe or a file), else
/// `None`. Test builds never read the test harness's stdin.
fn read_piped_stdin() -> Option<String> {
    if cfg!(test) || std::io::stdin().is_terminal() {
        return None;
    }
    let mut piped = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut piped);
    Some(piped)
}

/// `--no-tui`, which `main` takes out of the arguments before they are parsed.
static NO_TUI_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records whether this run had `--no-tui`: it asks for the line REPL, so piped input
/// meant for it is refused rather than sent to the model.
pub(crate) fn record_no_tui_flag(given: bool) {
    NO_TUI_FLAG.store(given, std::sync::atomic::Ordering::Relaxed);
}

/// Piped text goes to the model as one prompt, so `/model` and `/exit` on their own
/// lines would be a question for the model, not commands.
const PIPED_SLASH_COMMANDS: &str = "piped input is sent to the model as one prompt, so slash \
commands can't be piped. Use subcommands (`neuron status`), `neuron --resume latest /status`, \
or run `neuron` in a terminal.";

/// The prompt piped stdin carries when the command line has none: `None` when it is
/// empty (the REPL arm decides), an error when it was meant for the REPL (its first
/// line is a slash command, or `--no-tui` asked for the line REPL).
pub(crate) fn piped_prompt(piped: &str, no_tui: bool) -> Result<Option<String>, String> {
    // Windows PowerShell pipes, and files saved as UTF-8 with BOM, start with U+FEFF,
    // which `trim` keeps: `\u{feff}/model` would not look like a slash command.
    let prompt = piped.trim_start_matches('\u{feff}').trim();
    if prompt.is_empty() {
        return Ok(None);
    }
    if prompt
        .lines()
        .next()
        .is_some_and(looks_like_slash_command_token)
    {
        return Err(PIPED_SLASH_COMMANDS.to_string());
    }
    if no_tui {
        return Err(format!(
            "--no-tui starts the line REPL, which needs a terminal, and {PIPED_SLASH_COMMANDS} \
To send the piped text as a prompt, leave out --no-tui."
        ));
    }
    Ok(Some(prompt.to_string()))
}

/// The slash command Git Bash turned into a path: MSYS rewrites an argument that starts
/// with `/` to one under its install folder, so `neuron /model` arrives as
/// `C:/Program Files/Git/model` (`/usr/...` under `Git/usr`). The install folder is
/// named `Git` (Git for Windows) or holds `usr/bin/msys-2.0.dll` (MSYS2 at
/// `C:/msys64`, Scoop's `apps/git/current`). `msystem` is `MSYSTEM`, which Git Bash
/// and MSYS2 set.
pub(crate) fn msys_slash_command(arg: &str, msystem: Option<&str>) -> Option<String> {
    msys_slash_command_under(arg, msystem, |root| {
        Path::new(root).join("usr/bin/msys-2.0.dll").is_file()
    })
}

/// [`msys_slash_command`] with `is_msys_root` telling whether a folder is an MSYS
/// install.
fn msys_slash_command_under(
    arg: &str,
    msystem: Option<&str>,
    is_msys_root: impl FnOnce(&str) -> bool,
) -> Option<String> {
    msystem.filter(|value| !value.is_empty())?;
    let bytes = arg.as_bytes();
    if !(bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":/") {
        return None;
    }
    let (folder, name) = arg.rsplit_once('/')?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        return None;
    }
    let folder = folder.strip_suffix("/usr").unwrap_or(folder);
    // Not a drive's root folder (`C:/usr/model`).
    let (_, last) = folder.rsplit_once('/')?;
    let command = format!("/{name}");
    if !looks_like_slash_command_token(&command) {
        return None;
    }
    (last.eq_ignore_ascii_case("git") || is_msys_root(folder)).then_some(command)
}

/// Puts back the slash commands Git Bash turned into paths ([`msys_slash_command`])
/// where a slash command goes: the first word (`neuron /skills list`), and every word
/// after `--resume` (`neuron --resume latest /status`). Returns the commands put back.
fn restore_msys_slash_commands(words: &mut [String], msystem: Option<&str>) -> Vec<String> {
    let checked = if words.first().is_some_and(|word| word == "--resume") {
        words.len()
    } else {
        1
    };
    let mut restored = Vec::new();
    for word in words.iter_mut().take(checked) {
        if let Some(command) = msys_slash_command(word, msystem) {
            word.clone_from(&command);
            restored.push(command);
        }
    }
    restored
}

/// [`parse_args`] with standard input from `piped_stdin` (`None` on a terminal), which
/// is read only when the command line has no prompt.
#[allow(clippy::too_many_lines)]
pub(crate) fn parse_args_with_stdin(
    args: &[String],
    piped_stdin: impl FnOnce() -> Option<String>,
) -> Result<CliAction, String> {
    *MODEL_ARG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let mut model = DEFAULT_MODEL.to_string();
    // Only an explicit --model; `model` falls back to the default.
    let mut model_flag: Option<String> = None;
    let mut output_format = CliOutputFormat::Text;
    let mut permission_mode_override = None;
    let mut wants_help = false;
    let mut wants_version = false;
    let mut allowed_tool_values = Vec::new();
    let mut compact = false;
    let mut base_commit: Option<String> = None;
    let mut reasoning_effort: Option<String> = None;
    let mut allow_broad_cwd = false;
    let mut rest: Vec<String> = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" if rest.is_empty() => {
                wants_help = true;
                index += 1;
            }
            "--help" | "-h"
                if !rest.is_empty()
                    && matches!(
                        rest[0].as_str(),
                        "prompt"
                            | "--resume"
                            | "version"
                            | "state"
                            | "init"
                            | "export"
                            | "commit"
                            | "pr"
                            | "issue"
                    ) =>
            {
                // `--help` following a subcommand that would otherwise forward
                // the arg to the API (e.g. `neuron prompt --help`, `neuron --resume
                // latest prompt --help`) should show top-level help instead.
                // Subcommands that consume their own
                // args (agents, mcp, plugins, skills) and local help-topic
                // subcommands (status, sandbox, doctor) must NOT be intercepted
                // here — they handle --help in their own dispatch paths.
                wants_help = true;
                index += 1;
            }
            "--version" | "-V" => {
                wants_version = true;
                index += 1;
            }
            "--model" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --model".to_string())?;
                model = resolve_model_arg(value);
                model_flag = Some(model.clone());
                index += 2;
            }
            flag if flag.starts_with("--model=") => {
                model = resolve_model_arg(&flag[8..]);
                model_flag = Some(model.clone());
                index += 1;
            }
            "--output-format" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --output-format".to_string())?;
                output_format = CliOutputFormat::parse(value)?;
                index += 2;
            }
            "--permission-mode" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --permission-mode".to_string())?;
                permission_mode_override = Some(parse_permission_mode_arg(value)?);
                index += 2;
            }
            flag if flag.starts_with("--output-format=") => {
                output_format = CliOutputFormat::parse(&flag[16..])?;
                index += 1;
            }
            flag if flag.starts_with("--permission-mode=") => {
                permission_mode_override = Some(parse_permission_mode_arg(&flag[18..])?);
                index += 1;
            }
            "--dangerously-skip-permissions" => {
                permission_mode_override = Some(PermissionMode::DangerFullAccess);
                index += 1;
            }
            "--compact" => {
                compact = true;
                index += 1;
            }
            "--base-commit" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --base-commit".to_string())?;
                base_commit = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--base-commit=") => {
                base_commit = Some(flag[14..].to_string());
                index += 1;
            }
            "--reasoning-effort" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --reasoning-effort".to_string())?;
                if !matches!(value.as_str(), "low" | "medium" | "high") {
                    return Err(format!(
                        "invalid value for --reasoning-effort: '{value}'; must be low, medium, or high"
                    ));
                }
                reasoning_effort = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--reasoning-effort=") => {
                let value = &flag[19..];
                if !matches!(value, "low" | "medium" | "high") {
                    return Err(format!(
                        "invalid value for --reasoning-effort: '{value}'; must be low, medium, or high"
                    ));
                }
                reasoning_effort = Some(value.to_string());
                index += 1;
            }
            "--allow-broad-cwd" => {
                allow_broad_cwd = true;
                index += 1;
            }
            "-p" if rest.first().is_some_and(|word| word == "--resume") => {
                // `--resume SESSION -p TEXT`: the words continue the saved session.
                if let Some(action) = p_words_action(&args[index + 1..], output_format) {
                    return action;
                }
                rest.extend(args[index..].iter().cloned());
                break;
            }
            "-p" => {
                // NeuronCLI compat: -p "prompt" = one-shot prompt
                // Allow empty prompt so piped stdin works: echo "hello" | neuron -p
                if let Some(action) = p_words_action(&args[index + 1..], output_format) {
                    return action;
                }
                let prompt = args[index + 1..].join(" ");
                return Ok(CliAction::Prompt {
                    prompt,
                    model: resolve_model_alias_with_config(&model),
                    output_format,
                    allowed_tools: normalize_allowed_tools(&allowed_tool_values)?,
                    permission_mode: permission_mode_override
                        .unwrap_or_else(default_permission_mode),
                    compact,
                    base_commit: base_commit.clone(),
                    reasoning_effort: reasoning_effort.clone(),
                    allow_broad_cwd,
                });
            }
            "--print" => {
                // NeuronCLI compat: --print makes output non-interactive
                output_format = CliOutputFormat::Text;
                index += 1;
            }
            "--resume" if rest.is_empty() => {
                rest.push("--resume".to_string());
                index += 1;
            }
            flag if rest.is_empty() && flag.starts_with("--resume=") => {
                rest.push("--resume".to_string());
                rest.push(flag[9..].to_string());
                index += 1;
            }
            // Among a prompt's words (or after `--resume SESSION`): it would be prompt text.
            flag if is_session_flag(flag) => return Err(session_flag_error(flag)),
            "--acp" | "-acp" => {
                rest.push("acp".to_string());
                index += 1;
            }
            "--allowedTools" | "--allowed-tools" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --allowedTools".to_string())?;
                allowed_tool_values.push(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--allowedTools=") => {
                allowed_tool_values.push(flag[15..].to_string());
                index += 1;
            }
            flag if flag.starts_with("--allowed-tools=") => {
                allowed_tool_values.push(flag[16..].to_string());
                index += 1;
            }
            other if rest.is_empty() && other.starts_with('-') => {
                return Err(format_unknown_option(other))
            }
            other => {
                rest.push(other.to_string());
                index += 1;
            }
        }
    }

    if wants_help {
        return Ok(CliAction::Help { output_format });
    }

    if wants_version {
        return Ok(CliAction::Version { output_format });
    }

    let allowed_tools = normalize_allowed_tools(&allowed_tool_values)?;

    // Git Bash turned `neuron /model` into `neuron "C:/Program Files/Git/model"`.
    let restored = restore_msys_slash_commands(&mut rest, env::var("MSYSTEM").ok().as_deref());
    // In JSON mode stderr carries only JSON (the error object `main` prints).
    if output_format != CliOutputFormat::Json {
        for command in restored {
            eprintln!(
                "Git Bash turned {command} into a path; treating it as {command}. Set MSYS_NO_PATHCONV=1 to avoid this."
            );
        }
    }

    if rest.is_empty() {
        let permission_mode = permission_mode_override.unwrap_or_else(default_permission_mode);
        // When stdin is not a terminal (pipe/redirect) and no prompt is given on the
        // command line, read stdin as the prompt and dispatch as a one-shot Prompt
        // rather than starting the interactive REPL (which would consume the pipe and
        // print the startup banner, then exit without sending anything to the API).
        let piped = piped_stdin().unwrap_or_default();
        if let Some(prompt) = piped_prompt(
            &piped,
            NO_TUI_FLAG.load(std::sync::atomic::Ordering::Relaxed),
        )? {
            return Ok(CliAction::Prompt {
                model,
                prompt,
                allowed_tools,
                permission_mode,
                output_format,
                compact: false,
                base_commit,
                reasoning_effort,
                allow_broad_cwd,
            });
        }
        return Ok(CliAction::Repl {
            model,
            allowed_tools,
            permission_mode,
            base_commit,
            reasoning_effort: reasoning_effort.clone(),
            allow_broad_cwd,
        });
    }
    if rest.first().map(String::as_str) == Some("--resume") {
        return parse_resume_args(
            &rest[1..],
            output_format,
            ResumeLaunchOptions {
                model: model_flag,
                permission_mode: permission_mode_override,
                allowed_tools,
                reasoning_effort,
                base_commit,
                allow_broad_cwd,
            },
            compact,
        );
    }
    if let Some(action) = parse_local_help_action(&rest) {
        return action;
    }
    if let Some(action) =
        parse_single_word_command_alias(&rest, &model, permission_mode_override, output_format)
    {
        return action;
    }
    // `neuron help me debug this test` is a prompt; `neuron init the schema` is refused.
    let command = match classify_subcommand_words(&rest) {
        SubcommandWords::Command => rest[0].as_str(),
        // Matches no subcommand below: the words are the prompt.
        SubcommandWords::Prompt => "",
        SubcommandWords::Refused(error) => return Err(error),
    };

    let permission_mode = permission_mode_override.unwrap_or_else(default_permission_mode);

    match command {
        "dump-manifests" => parse_dump_manifests_args(&rest[1..], output_format),
        "bootstrap-plan" => Ok(CliAction::BootstrapPlan { output_format }),
        "agents" => Ok(CliAction::Agents {
            args: join_optional_args(&rest[1..]),
            output_format,
        }),
        "mcp" => Ok(CliAction::Mcp {
            args: join_optional_args(&rest[1..]),
            output_format,
        }),
        "skills" => {
            let args = join_optional_args(&rest[1..]);
            match classify_skills_slash_command(args.as_deref()) {
                SkillSlashDispatch::Invoke(prompt) => {
                    // `neuron skills NAME [ARGS]` runs skill NAME. When there is none, the
                    // words are an unquoted prompt.
                    let cwd = env::current_dir().map_err(|error| error.to_string())?;
                    if resolve_skill_invocation(&cwd, args.as_deref()).is_err() {
                        let word = rest[1].trim_start_matches(['$', '/']);
                        return Err(unknown_skill_error(&cwd, word));
                    }
                    Ok(CliAction::Prompt {
                        prompt,
                        model,
                        output_format,
                        allowed_tools,
                        permission_mode,
                        compact,
                        base_commit,
                        reasoning_effort: reasoning_effort.clone(),
                        allow_broad_cwd,
                    })
                }
                // `install` takes one path.
                SkillSlashDispatch::Local if rest.len() > 3 => Err(subcommand_arguments_error(
                    "skills",
                    SKILLS_USAGE,
                    &rest[1..],
                )),
                SkillSlashDispatch::Local => Ok(CliAction::Skills {
                    args,
                    output_format,
                }),
            }
        }
        "system-prompt" => parse_system_prompt_args(&rest[1..], output_format),
        "acp" => parse_acp_args(&rest[1..], output_format),
        "login" | "logout" | "auth" | "usage" | "upgrade" => {
            Err(removed_auth_surface_error(command))
        }
        "init" => Ok(CliAction::Init { output_format }),
        "export" => parse_export_args(&rest[1..], output_format),
        "prompt" => {
            let prompt = rest[1..].join(" ");
            if prompt.trim().is_empty() {
                return Err("prompt subcommand requires a prompt string".to_string());
            }
            Ok(CliAction::Prompt {
                prompt,
                model,
                output_format,
                allowed_tools,
                permission_mode,
                compact,
                base_commit: base_commit.clone(),
                reasoning_effort: reasoning_effort.clone(),
                allow_broad_cwd,
            })
        }
        other if other.starts_with('/') => parse_direct_slash_cli_action(
            &rest,
            model,
            output_format,
            allowed_tools,
            permission_mode,
            compact,
            base_commit,
            reasoning_effort,
            allow_broad_cwd,
        ),
        _other => Ok(CliAction::Prompt {
            prompt: rest.join(" "),
            model,
            output_format,
            allowed_tools,
            permission_mode,
            compact,
            base_commit,
            reasoning_effort: reasoning_effort.clone(),
            allow_broad_cwd,
        }),
    }
}

pub(crate) fn parse_local_help_action(rest: &[String]) -> Option<Result<CliAction, String>> {
    if rest.len() != 2 || !is_help_flag(&rest[1]) {
        return None;
    }

    let topic = match rest[0].as_str() {
        "status" => LocalHelpTopic::Status,
        "sandbox" => LocalHelpTopic::Sandbox,
        "doctor" => LocalHelpTopic::Doctor,
        "acp" => LocalHelpTopic::Acp,
        _ => return None,
    };
    Some(Ok(CliAction::HelpTopic(topic)))
}

pub(crate) fn is_help_flag(value: &str) -> bool {
    matches!(value, "--help" | "-h")
}

pub(crate) fn parse_single_word_command_alias(
    rest: &[String],
    model: &str,
    permission_mode_override: Option<PermissionMode>,
    output_format: CliOutputFormat,
) -> Option<Result<CliAction, String>> {
    if rest.len() != 1 {
        return None;
    }

    match rest[0].as_str() {
        "help" => Some(Ok(CliAction::Help { output_format })),
        "version" => Some(Ok(CliAction::Version { output_format })),
        "status" => Some(Ok(CliAction::Status {
            model: model.to_string(),
            permission_mode: permission_mode_override.unwrap_or_else(default_permission_mode),
            output_format,
        })),
        "sandbox" => Some(Ok(CliAction::Sandbox { output_format })),
        "doctor" => Some(Ok(CliAction::Doctor { output_format })),
        "state" => Some(Ok(CliAction::State { output_format })),
        other => bare_slash_command_guidance(other).map(Err),
    }
}

pub(crate) fn bare_slash_command_guidance(command_name: &str) -> Option<String> {
    if matches!(
        command_name,
        "dump-manifests"
            | "bootstrap-plan"
            | "agents"
            | "mcp"
            | "skills"
            | "system-prompt"
            | "init"
            | "prompt"
            | "export"
            // Handled by `removed_auth_surface_error`, which points at
            // `neuron auth login|logout`.
            | "login"
            | "logout"
    ) {
        return None;
    }
    // Aliases too: `neuron plugins` is `/plugins`, not a one-word prompt.
    let slash_command = slash_command_specs()
        .iter()
        .find(|spec| spec.name == command_name || spec.aliases.contains(&command_name))?;
    let guidance = if stub_slash_command(command_name) {
        format!(
            "`neuron {command_name}` is a slash command that is not yet implemented in this build; to send it as a prompt, use `neuron prompt {command_name}`."
        )
    } else if resume_safe_slash_command(command_name) {
        format!(
            "`neuron {command_name}` is a slash command. Use `neuron --resume SESSION.jsonl /{command_name}` or start `neuron` and run `/{command_name}`."
        )
    } else {
        format!(
            "`neuron {command_name}` is a slash command. Start `neuron` and run `/{command_name}` inside the REPL."
        )
    };
    Some(guidance)
}

/// The account commands (`neuron login`, `logout`, `auth`, `usage`, `upgrade`) run before
/// argument parsing (`auth::account_command`), also after global options. The parser sees
/// one only with arguments it does not take, which is usually an unquoted prompt that
/// starts with its name (`neuron --model m login to the server and fix it`): neither is
/// guessed.
pub(crate) fn removed_auth_surface_error(command_name: &str) -> String {
    let usage = match command_name {
        "login" => "neuron login [--no-browser | --with-token]",
        "logout" => "neuron logout",
        "usage" => "neuron usage",
        "upgrade" => "neuron upgrade [--no-browser]",
        _ => "neuron auth <login|logout|status>",
    };
    format!(
        "`{command_name}` is a Zero-X account command (`{usage}`) and takes no other arguments. {}",
        quote_prompt_hint(command_name)
    )
}

/// How to send a prompt that starts with command `name` instead of running the command.
pub(crate) fn quote_prompt_hint(name: &str) -> String {
    format!(
        "To send a prompt that starts with \"{name}\", quote it (`neuron \"{name} ...\"`) or use `neuron prompt \"{name} ...\"`."
    )
}

const SKILLS_USAGE: &str = "neuron skills [list|install <path>|help|<skill> [args]]";

/// How many skill names an unknown-skill error suggests.
const SKILL_SUGGESTIONS: usize = 3;

/// `neuron skills <words>` where the first word is no skill: usually an unquoted prompt
/// that starts with "skills". The quoting hint comes first, then the closest skill
/// names (not the whole list: `neuron skills` lists them).
fn unknown_skill_error(cwd: &Path, word: &str) -> String {
    let names = commands::handle_skills_slash_command_json(None, cwd)
        .ok()
        .and_then(|report| report.get("skills").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter(|skill| skill.get("shadowed_by").is_none_or(Value::is_null))
        .filter_map(|skill| {
            skill
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let names = names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut message = format!(
        "`skills` runs a skill (`{SKILLS_USAGE}`), and there is no skill named `{word}`. {}",
        quote_prompt_hint("skills")
    );
    let close = ranked_suggestions(word, &names);
    if !close.is_empty() {
        message.push_str(&format!(
            "\n  Did you mean: {}",
            close
                .iter()
                .take(SKILL_SUGGESTIONS)
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    message.push_str("\n  `neuron skills` lists the installed skills.");
    message
}

/// What the words after a subcommand's name are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubcommandWords {
    /// The subcommand with arguments it takes (or no subcommand at all).
    Command,
    /// An unquoted prompt that starts with the name of a subcommand that only shows
    /// something (`neuron help me debug this test`, `neuron mcp server crashes on
    /// start`): the words go to the model.
    Prompt,
    /// Words a subcommand with side effects (or a developer command) does not take
    /// (`neuron init the database schema`, `neuron login to the server`): refused with
    /// the quoting hint, so the subcommand never runs and the prompt is not guessed.
    Refused(String),
}

/// Sorts the words after a subcommand's name ([`SubcommandWords`]). A first word that
/// is an option (`neuron status -x`) was meant for the subcommand: refused, or named by
/// the subcommand's own parser. `skills` and `export` are checked where they are parsed
/// (a skill's name, a path); one-word commands by [`parse_single_word_command_alias`].
pub(crate) fn classify_subcommand_words(rest: &[String]) -> SubcommandWords {
    let Some((name, args)) = rest.split_first() else {
        return SubcommandWords::Command;
    };
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let option_first = words
        .first()
        .is_some_and(|word| word.len() > 1 && word.starts_with('-'));
    // Shows something: words it does not take are a prompt.
    let shows = |usage: &str, takes: bool| match (takes, option_first) {
        (true, _) => SubcommandWords::Command,
        (false, false) => SubcommandWords::Prompt,
        (false, true) => SubcommandWords::Refused(subcommand_arguments_error(name, usage, args)),
    };
    // Changes something, or is a developer command: words it does not take are refused.
    let changes = |usage: &str, takes: bool| {
        if takes {
            SubcommandWords::Command
        } else {
            SubcommandWords::Refused(subcommand_arguments_error(name, usage, args))
        }
    };
    match name.as_str() {
        "help" | "version" | "status" | "sandbox" | "doctor" | "state" => {
            shows(&format!("neuron {name}"), words.is_empty())
        }
        "agents" => shows(
            "neuron agents [list|help]",
            matches!(without_help_word(&words), [] | ["list"]),
        ),
        "mcp" => shows(
            "neuron mcp [list|show <server>|serve|help]",
            matches!(
                without_help_word(&words),
                [] | ["list" | "show" | "serve"] | ["show", _]
            ),
        ),
        "acp" => shows(
            "neuron acp [serve]",
            matches!(bare_words(&words, &[]).as_slice(), [] | ["serve"]),
        ),
        // The account commands run before parsing (`auth::account_command`) with the
        // arguments they take; here they have others. `usage` and `auth status` only
        // show something; signing in or out and opening the plans page never run on a
        // guess.
        "usage" if !option_first => SubcommandWords::Prompt,
        "auth" if !option_first && !matches!(words.first(), Some(&("login" | "logout"))) => {
            SubcommandWords::Prompt
        }
        "init" | "bootstrap-plan" => changes(&format!("neuron {name}"), words.is_empty()),
        "dump-manifests" => changes(
            "neuron dump-manifests [--manifests-dir PATH]",
            bare_words(&words, &["--manifests-dir"]).is_empty(),
        ),
        "system-prompt" => changes(
            "neuron system-prompt [--cwd PATH] [--date YYYY-MM-DD]",
            bare_words(&words, &["--cwd", "--date"]).is_empty(),
        ),
        // `login`, `logout`, `upgrade` and `auth login|logout ...` are refused by their
        // arm in `parse_args` (`removed_auth_surface_error`); `export` by its parser.
        _ => SubcommandWords::Command,
    }
}

fn subcommand_arguments_error(name: &str, usage: &str, args: &[String]) -> String {
    format!(
        "`{name}` is a neuron command (`{usage}`) and does not take `{}`. {}",
        args.join(" "),
        quote_prompt_hint(name)
    )
}

/// `words` without a trailing help word (`neuron mcp show help`).
fn without_help_word<'w, 'a>(words: &'w [&'a str]) -> &'w [&'a str] {
    match words.split_last() {
        Some((last, init)) if is_help_flag(last) || *last == "help" => init,
        _ => words,
    }
}

/// The words that are neither options nor the values of `value_flags`.
fn bare_words<'a>(words: &[&'a str], value_flags: &[&str]) -> Vec<&'a str> {
    let mut bare = Vec::new();
    let mut index = 0;
    while let Some(word) = words.get(index) {
        if value_flags.contains(word) {
            index += 2;
            continue;
        }
        if !word.starts_with('-') {
            bare.push(*word);
        }
        index += 1;
    }
    bare
}

/// Global flags whose next argument is their value (so it is never mistaken
/// for a subcommand).
const VALUE_FLAGS: &[&str] = &[
    "--model",
    "--output-format",
    "--permission-mode",
    "--base-commit",
    "--reasoning-effort",
    "--allowedTools",
    "--allowed-tools",
];

/// Removes `--provider <id>` / `--provider=<id>` from `args` and returns the
/// remaining arguments with the parsed choice. Scanning stops where prompt
/// text begins: at `-p`, at a `prompt` subcommand, and at `--`; place
/// `--provider` before those.
pub(crate) fn extract_provider_flag(
    args: &[String],
) -> Result<(Vec<String>, Option<crate::provider::ProviderId>), String> {
    let mut remaining = Vec::with_capacity(args.len());
    let mut provider = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let is_flag_value = index > 0 && VALUE_FLAGS.contains(&args[index - 1].as_str());
        if arg == "-p" || arg == "--" || (arg == "prompt" && !is_flag_value) {
            remaining.extend_from_slice(&args[index..]);
            break;
        }
        let value = if arg == "--provider" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| "missing value for --provider".to_string())?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--provider=")
        };
        match value {
            Some(value) => {
                provider = Some(crate::provider::parse_provider_id(value, "--provider")?);
            }
            None => remaining.push(arg.clone()),
        }
        index += 1;
    }
    Ok((remaining, provider))
}

pub(crate) fn parse_acp_args(
    args: &[String],
    output_format: CliOutputFormat,
) -> Result<CliAction, String> {
    match args {
        [] => Ok(CliAction::Acp { output_format }),
        [subcommand] if subcommand == "serve" => Ok(CliAction::Acp { output_format }),
        _ => Err(String::from(
            "unsupported ACP invocation. Use `neuron acp`, `neuron acp serve`, `neuron --acp`, or `neuron -acp`.",
        )),
    }
}

pub(crate) fn try_resolve_bare_skill_prompt(cwd: &Path, trimmed: &str) -> Option<String> {
    let bare_first_token = trimmed.split_whitespace().next().unwrap_or_default();
    let looks_like_skill_name = !bare_first_token.is_empty()
        && !bare_first_token.starts_with('/')
        && bare_first_token
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if !looks_like_skill_name {
        return None;
    }
    match resolve_skill_invocation(cwd, Some(trimmed)) {
        Ok(SkillSlashDispatch::Invoke(prompt)) => Some(prompt),
        _ => None,
    }
}

pub(crate) fn join_optional_args(args: &[String]) -> Option<String> {
    let joined = args.join(" ");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn parse_direct_slash_cli_action(
    rest: &[String],
    model: String,
    output_format: CliOutputFormat,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    compact: bool,
    base_commit: Option<String>,
    reasoning_effort: Option<String>,
    allow_broad_cwd: bool,
) -> Result<CliAction, String> {
    // Before parsing: a stub without a parse arm would read as an unknown command
    // that suggests itself.
    if stub_slash_command(&rest[0]) {
        return Err(format!(
            "slash command {} is not yet implemented in this build.",
            rest[0]
        ));
    }
    let raw = rest.join(" ");
    match SlashCommand::parse(&raw) {
        Ok(Some(SlashCommand::Help)) => Ok(CliAction::Help { output_format }),
        Ok(Some(SlashCommand::Agents { args })) => Ok(CliAction::Agents {
            args,
            output_format,
        }),
        Ok(Some(SlashCommand::Mcp { action, target })) => Ok(CliAction::Mcp {
            args: match (action, target) {
                (None, None) => None,
                (Some(action), None) => Some(action),
                (Some(action), Some(target)) => Some(format!("{action} {target}")),
                (None, Some(target)) => Some(target),
            },
            output_format,
        }),
        Ok(Some(SlashCommand::Skills { args })) => {
            match classify_skills_slash_command(args.as_deref()) {
                SkillSlashDispatch::Invoke(prompt) => Ok(CliAction::Prompt {
                    prompt,
                    model,
                    output_format,
                    allowed_tools,
                    permission_mode,
                    compact,
                    base_commit,
                    reasoning_effort: reasoning_effort.clone(),
                    allow_broad_cwd,
                }),
                SkillSlashDispatch::Local => Ok(CliAction::Skills {
                    args,
                    output_format,
                }),
            }
        }
        Ok(Some(SlashCommand::Unknown(name))) => Err(format_unknown_direct_slash_command(&name)),
        Ok(Some(command)) => Err({
            let command_name = &rest[0];
            if resume_safe_slash_command(command_name) {
                format!(
                    "slash command {command_name} is interactive-only. Start `neuron` and run it there, or use `neuron --resume SESSION.jsonl {command_name}` / `neuron --resume {latest} {command_name}`.",
                    latest = LATEST_SESSION_REFERENCE,
                )
            } else {
                let mut message = format!(
                    "slash command {command_name} works only inside a session: start `neuron` on a terminal and run it there."
                );
                if matches!(command, SlashCommand::Model { .. }) {
                    message.push_str(" The current catalog is at https://zero-x.live/v1/models.");
                }
                message
            }
        }),
        Ok(None) => Err(format!("unknown subcommand: {}", rest[0])),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether `neuron --resume SESSION /name` runs this command: it is marked [resume] in
/// /help and is not one of the stubs that only say "not yet implemented". `--help`'s
/// "Resume-safe commands" line and the hints for `neuron X` / `neuron /X` all use this;
/// the test `every_resume_safe_command_runs_under_resume` runs each one through
/// `run_resume_command`.
pub(crate) fn resume_safe_slash_command(command_name: &str) -> bool {
    let name = slash_command_root(command_name);
    slash_command_specs().iter().any(|spec| {
        spec.resume_supported
            && !STUB_COMMANDS.contains(&spec.name)
            && (spec.name == name || spec.aliases.contains(&name))
    })
}

/// Whether `/name` (or an alias of it) is one of the stubs that only say "not yet
/// implemented".
fn stub_slash_command(command_name: &str) -> bool {
    let name = slash_command_root(command_name);
    STUB_COMMANDS.contains(&name)
        || slash_command_specs()
            .iter()
            .any(|spec| STUB_COMMANDS.contains(&spec.name) && spec.aliases.contains(&name))
}

/// `status` for `/status`, `status` or `/status --json`.
fn slash_command_root(command_name: &str) -> &str {
    command_name
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('/')
}

pub(crate) fn format_unknown_option(option: &str) -> String {
    let mut message = format!("unknown option: {option}");
    if let Some(suggestion) = suggest_closest_term(option, CLI_OPTION_SUGGESTIONS) {
        message.push_str("\nDid you mean ");
        message.push_str(suggestion);
        message.push('?');
    }
    message.push_str("\nRun `neuron --help` for usage.");
    message
}

pub(crate) fn format_unknown_direct_slash_command(name: &str) -> String {
    let mut message = format!("unknown slash command outside the REPL: /{name}");
    if let Some(suggestions) = render_suggestion_line("Did you mean", &suggest_slash_commands(name))
    {
        message.push('\n');
        message.push_str(&suggestions);
    }
    if let Some(note) = omc_compatibility_note_for_unknown_slash_command(name) {
        message.push('\n');
        message.push_str(note);
    }
    message.push_str("\nRun `neuron --help` for CLI usage, or start `neuron` and use /help.");
    message
}

pub(crate) fn format_unknown_slash_command(name: &str) -> String {
    let mut message = format!("Unknown slash command: /{name}");
    if let Some(suggestions) = render_suggestion_line("Did you mean", &suggest_slash_commands(name))
    {
        message.push('\n');
        message.push_str(&suggestions);
    }
    if let Some(note) = omc_compatibility_note_for_unknown_slash_command(name) {
        message.push('\n');
        message.push_str(note);
    }
    message.push_str("\n  Help             /help lists available slash commands");
    message
}

pub(crate) fn omc_compatibility_note_for_unknown_slash_command(name: &str) -> Option<&'static str> {
    name.starts_with("oh-my-claudecode:")
        .then_some(
            "Note: this looks like a plugin slash command. `neuron` does not yet load plugin slash commands.",
        )
}

pub(crate) fn render_suggestion_line(label: &str, suggestions: &[String]) -> Option<String> {
    (!suggestions.is_empty()).then(|| format!("  {label:<16} {}", suggestions.join(", "),))
}

pub(crate) fn suggest_slash_commands(input: &str) -> Vec<String> {
    let mut candidates = slash_command_specs()
        .iter()
        .flat_map(|spec| {
            std::iter::once(spec.name)
                .chain(spec.aliases.iter().copied())
                .map(|name| format!("/{name}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let candidate_refs = candidates.iter().map(String::as_str).collect::<Vec<_>>();
    ranked_suggestions(input.trim_start_matches('/'), &candidate_refs)
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(crate) fn suggest_closest_term<'a>(input: &str, candidates: &'a [&'a str]) -> Option<&'a str> {
    ranked_suggestions(input, candidates).into_iter().next()
}

pub(crate) fn ranked_suggestions<'a>(input: &str, candidates: &'a [&'a str]) -> Vec<&'a str> {
    let normalized_input = input.trim_start_matches('/').to_ascii_lowercase();
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            let normalized_candidate = candidate.trim_start_matches('/').to_ascii_lowercase();
            let distance = levenshtein_distance(&normalized_input, &normalized_candidate);
            let prefix_bonus = usize::from(
                !(normalized_candidate.starts_with(&normalized_input)
                    || normalized_input.starts_with(&normalized_candidate)),
            );
            let score = distance + prefix_bonus;
            (score <= 4).then_some((score, *candidate))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.cmp(right).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(3)
        .collect()
}

pub(crate) fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != *right_char);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
        }
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

pub(crate) fn resolve_model_alias(model: &str) -> &str {
    match model {
        "opus" => "claude-opus-4-6",
        "sonnet" => "claude-sonnet-4-6",
        "haiku" => "claude-haiku-4-5-20251213",
        _ => model,
    }
}

/// Resolve a model name through user-defined config aliases first, then fall
/// back to the built-in alias table. This is the entry point used wherever a
/// user-supplied model string is about to be dispatched to a provider.
pub(crate) fn resolve_model_alias_with_config(model: &str) -> String {
    let trimmed = model.trim();
    if let Some(resolved) = config_alias_for_current_dir(trimmed) {
        return resolve_model_alias(&resolved).to_string();
    }
    resolve_model_alias(trimmed).to_string()
}

/// The `--model` value as typed and the name it resolved to while arguments
/// were parsed, which happens before the trust prompt.
static MODEL_ARG: Mutex<Option<(String, String)>> = Mutex::new(None);

/// `--model` as the user typed it, when this run had one.
pub(crate) fn typed_model_arg() -> Option<String> {
    MODEL_ARG
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(typed, _)| typed.clone())
}

fn resolve_model_arg(value: &str) -> String {
    let resolved = resolve_model_alias_with_config(value);
    *MODEL_ARG.lock().unwrap_or_else(PoisonError::into_inner) =
        Some((value.to_string(), resolved.clone()));
    resolved
}

/// Re-resolves a `--model` alias after the trust prompt: arguments are parsed
/// before it, while workspace aliases only apply to a trusted workspace. A
/// model that did not come from `--model` is kept.
pub(crate) fn model_after_trust(resolved: String) -> String {
    let recorded = MODEL_ARG
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match recorded {
        Some((typed, parsed)) if parsed == resolved => resolve_model_alias_with_config(&typed),
        _ => resolved,
    }
}

pub(crate) fn config_alias_for_current_dir(alias: &str) -> Option<String> {
    if alias.is_empty() {
        return None;
    }
    let cwd = env::current_dir().ok()?;
    let loader = ConfigLoader::default_for(&cwd);
    let config = loader.load().ok()?;
    config.aliases().get(alias).cloned()
}

pub(crate) fn normalize_allowed_tools(values: &[String]) -> Result<Option<AllowedToolSet>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    current_tool_registry()?.normalize_allowed_tools(values)
}

pub(crate) fn current_tool_registry() -> Result<GlobalToolRegistry, String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load().map_err(|error| error.to_string())?;
    let state = build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config)
        .map_err(|error| error.to_string())?;
    let registry = state.tool_registry.clone();
    if let Some(mcp_state) = state.mcp_state {
        mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
            .map_err(|error| error.to_string())?;
    }
    Ok(registry)
}

pub(crate) fn parse_permission_mode_arg(value: &str) -> Result<PermissionMode, String> {
    normalize_permission_mode(value)
        .ok_or_else(|| {
            format!(
                "unsupported permission mode '{value}'. Use read-only, workspace-write, or danger-full-access."
            )
        })
        .map(permission_mode_from_label)
}

pub(crate) fn permission_mode_from_label(mode: &str) -> PermissionMode {
    match mode {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => panic!("unsupported permission mode label: {other}"),
    }
}

pub(crate) fn permission_mode_from_resolved(mode: ResolvedPermissionMode) -> PermissionMode {
    match mode {
        ResolvedPermissionMode::ReadOnly => PermissionMode::ReadOnly,
        ResolvedPermissionMode::WorkspaceWrite => PermissionMode::WorkspaceWrite,
        ResolvedPermissionMode::DangerFullAccess => PermissionMode::DangerFullAccess,
    }
}

/// Set when a permission mode came from settings or the built-in default
/// rather than `--permission-mode` or the environment, so it can be resolved
/// again once the workspace trust prompt has been answered.
static PERMISSION_MODE_FROM_DEFAULTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn default_permission_mode() -> PermissionMode {
    if let Some(mode) = env::var("RUSTY_CLAUDE_PERMISSION_MODE")
        .ok()
        .as_deref()
        .and_then(normalize_permission_mode)
        .map(permission_mode_from_label)
    {
        return mode;
    }
    PERMISSION_MODE_FROM_DEFAULTS.store(true, std::sync::atomic::Ordering::Relaxed);
    config_permission_mode_for_current_dir().unwrap_or(PermissionMode::WorkspaceWrite)
}

/// Re-resolves a permission mode that came from defaults after the trust
/// prompt: arguments are parsed before it, while workspace settings only
/// apply to a trusted workspace. An explicit flag or env value is kept.
pub(crate) fn permission_mode_after_trust(resolved: PermissionMode) -> PermissionMode {
    if PERMISSION_MODE_FROM_DEFAULTS.load(std::sync::atomic::Ordering::Relaxed) {
        default_permission_mode()
    } else {
        resolved
    }
}

pub(crate) fn config_permission_mode_for_current_dir() -> Option<PermissionMode> {
    let cwd = env::current_dir().ok()?;
    let loader = ConfigLoader::default_for(&cwd);
    loader
        .load()
        .ok()?
        .permission_mode()
        .map(permission_mode_from_resolved)
}

pub(crate) fn config_model_for_current_dir() -> Option<String> {
    let cwd = env::current_dir().ok()?;
    let loader = ConfigLoader::default_for(&cwd);
    loader.load().ok()?.model().map(ToOwned::to_owned)
}

pub(crate) fn resolve_repl_model(cli_model: String) -> String {
    if cli_model != DEFAULT_MODEL {
        return cli_model;
    }
    if let Some(env_model) = env::var("ANTHROPIC_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return resolve_model_alias_with_config(&env_model);
    }
    if let Some(config_model) = config_model_for_current_dir() {
        return resolve_model_alias_with_config(&config_model);
    }
    cli_model
}

// -- Provider (extracted to provider.rs) --

pub(crate) fn filter_tool_specs(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
) -> Vec<ToolDefinition> {
    tool_registry.definitions(allowed_tools)
}

pub(crate) fn parse_system_prompt_args(
    args: &[String],
    output_format: CliOutputFormat,
) -> Result<CliAction, String> {
    let mut cwd = env::current_dir().map_err(|error| error.to_string())?;
    let mut date = DEFAULT_DATE.to_string();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --cwd".to_string())?;
                cwd = PathBuf::from(value);
                index += 2;
            }
            "--date" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --date".to_string())?;
                date.clone_from(value);
                index += 2;
            }
            other => return Err(format!("unknown system-prompt option: {other}")),
        }
    }

    Ok(CliAction::PrintSystemPrompt {
        cwd,
        date,
        output_format,
    })
}

pub(crate) fn parse_export_args(
    args: &[String],
    output_format: CliOutputFormat,
) -> Result<CliAction, String> {
    let mut session_reference = LATEST_SESSION_REFERENCE.to_string();
    let mut output_path: Option<PathBuf> = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--session" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --session".to_string())?;
                session_reference.clone_from(value);
                index += 2;
            }
            flag if flag.starts_with("--session=") => {
                session_reference = flag[10..].to_string();
                index += 1;
            }
            "--output" | "-o" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {}", args[index]))?;
                output_path = Some(PathBuf::from(value));
                index += 2;
            }
            flag if flag.starts_with("--output=") => {
                output_path = Some(PathBuf::from(&flag[9..]));
                index += 1;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown export option: {other}"));
            }
            // A bare word (`neuron export summary`) is more often the start of an
            // unquoted prompt than a file name: the path must look like one.
            other if output_path.is_none() && !looks_like_a_path(other) => {
                return Err(format!(
                    "`neuron export` writes the session to a file, and `{other}` does not look like one: give a file name with an extension (`{other}.md`), a path (`./{other}`), or `--output {other}`. {}",
                    quote_prompt_hint("export")
                ));
            }
            other if output_path.is_none() => {
                output_path = Some(PathBuf::from(other));
                index += 1;
            }
            other => {
                return Err(format!(
                    "unexpected export argument: {other}. {}",
                    quote_prompt_hint("export")
                ));
            }
        }
    }

    Ok(CliAction::Export {
        session_reference,
        output_path,
        output_format,
    })
}

/// Whether `word` names a file: it has an extension or a directory part.
fn looks_like_a_path(word: &str) -> bool {
    word.contains(['/', '\\'])
        || Path::new(word)
            .extension()
            .is_some_and(|ext| !ext.is_empty())
}

pub(crate) fn parse_dump_manifests_args(
    args: &[String],
    output_format: CliOutputFormat,
) -> Result<CliAction, String> {
    let mut manifests_dir: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--manifests-dir" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| String::from("--manifests-dir requires a path"))?;
            manifests_dir = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--manifests-dir=") {
            if value.is_empty() {
                return Err(String::from("--manifests-dir requires a path"));
            }
            manifests_dir = Some(PathBuf::from(value));
            index += 1;
            continue;
        }
        return Err(format!("unknown dump-manifests option: {arg}"));
    }

    Ok(CliAction::DumpManifests {
        output_format,
        manifests_dir,
    })
}

/// How to send a saved session another prompt.
const RESUME_PROMPT_USAGE: &str = "neuron --resume latest prompt \"...\"";

/// `--resume` or `--continue`, alone or with `=VALUE`.
fn is_session_flag(word: &str) -> bool {
    matches!(
        word.split('=').next().unwrap_or(word),
        "--resume" | "--continue"
    )
}

/// `neuron prompt TEXT --resume SESSION`: the flag would reach the model as prompt text,
/// and the prompt would start a new session.
fn session_flag_error(word: &str) -> String {
    let flag = word.split('=').next().unwrap_or(word);
    format!(
        "{flag} among a prompt's words would be sent to the model as text and start a new session; to send a saved session another prompt use `{RESUME_PROMPT_USAGE}` (or a session id in place of latest)"
    )
}

/// The options `neuron prompt TEXT` still applies after its text (the arms of
/// [`parse_args_with_stdin`]). After `-p` every word is prompt text, so there they would
/// be sent to the model instead.
pub(crate) const OPTIONS_APPLIED_AFTER_PROMPT: &[&str] = &[
    "--model",
    "--output-format",
    "--permission-mode",
    "--dangerously-skip-permissions",
    "--compact",
    "--base-commit",
    "--reasoning-effort",
    "--allow-broad-cwd",
    "--print",
    "--allowedTools",
    "--allowed-tools",
];

/// What the words after `-p`, all prompt text, ask for besides the prompt: help, as after
/// `prompt`, or an error for an option or session flag that would be sent to the model as
/// text where `neuron prompt` would apply it.
fn p_words_action(
    words: &[String],
    output_format: CliOutputFormat,
) -> Option<Result<CliAction, String>> {
    if words.iter().any(|word| is_help_flag(word)) {
        return Some(Ok(CliAction::Help { output_format }));
    }
    words.iter().find_map(|word| {
        let option = word.split('=').next().unwrap_or(word);
        if is_session_flag(word) {
            Some(Err(session_flag_error(word)))
        } else {
            OPTIONS_APPLIED_AFTER_PROMPT.contains(&option).then(|| {
                Err(format!(
                    "{option} after `-p` would be sent to the model as prompt text: put options before `-p` (`neuron {option} ... -p \"...\"`), or quote the prompt"
                ))
            })
        }
    })
}

/// Whether `word` starts the prompt in `neuron --resume SESSION prompt TEXT`.
fn is_resume_prompt_word(word: &str) -> bool {
    matches!(word, "prompt" | "-p")
}

pub(crate) fn parse_resume_args(
    args: &[String],
    output_format: CliOutputFormat,
    launch: ResumeLaunchOptions,
    compact: bool,
) -> Result<CliAction, String> {
    let (session_path, command_tokens): (PathBuf, &[String]) = match args.first() {
        None => (PathBuf::from(LATEST_SESSION_REFERENCE), &[]),
        Some(first) if looks_like_slash_command_token(first) || is_resume_prompt_word(first) => {
            (PathBuf::from(LATEST_SESSION_REFERENCE), args)
        }
        Some(first) => (PathBuf::from(first), &args[1..]),
    };
    if let Some((word, words)) = command_tokens.split_first() {
        if is_resume_prompt_word(word) {
            let prompt = words.join(" ");
            // `echo "..." | neuron --resume latest -p`: the prompt is on stdin.
            if prompt.trim().is_empty() && word != "-p" {
                return Err(format!(
                    "--resume ... {word} requires a prompt string: `{RESUME_PROMPT_USAGE}`"
                ));
            }
            return Ok(CliAction::ResumePrompt {
                session_path,
                prompt,
                output_format,
                launch,
                compact,
            });
        }
    }
    let mut commands = Vec::new();
    let mut current_command = String::new();

    for token in command_tokens {
        if token.trim_start().starts_with('/') {
            if resume_command_can_absorb_token(&current_command, token) {
                current_command.push(' ');
                current_command.push_str(token);
                continue;
            }
            if !current_command.is_empty() {
                commands.push(current_command);
            }
            current_command = String::from(token.as_str());
            continue;
        }

        if current_command.is_empty() {
            return Err(format!(
                "--resume takes slash commands such as /status; to send a prompt use `{RESUME_PROMPT_USAGE}`"
            ));
        }

        current_command.push(' ');
        current_command.push_str(token);
    }

    if !current_command.is_empty() {
        commands.push(current_command);
    }

    Ok(CliAction::ResumeSession {
        session_path,
        commands,
        output_format,
        launch,
    })
}

#[cfg(test)]
mod provider_flag_tests {
    use super::extract_provider_flag;
    use crate::provider::ProviderId;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn provider_flag_is_removed_and_parsed_in_both_spellings() {
        let (rest, provider) =
            extract_provider_flag(&strings(&["--provider", "openai", "--model", "gpt-4o"]))
                .unwrap();
        assert_eq!(rest, strings(&["--model", "gpt-4o"]));
        assert_eq!(provider, Some(ProviderId::OpenAi));

        let (rest, provider) = extract_provider_flag(&strings(&[
            "--provider=zero-x",
            "--compact",
            "prompt",
            "hi",
        ]))
        .unwrap();
        assert_eq!(rest, strings(&["--compact", "prompt", "hi"]));
        assert_eq!(provider, Some(ProviderId::ZeroX));
    }

    #[test]
    fn prompt_subcommand_words_are_prompt_text() {
        let words = strings(&["prompt", "explain", "the", "--provider", "flag"]);
        let (rest, provider) = extract_provider_flag(&words).unwrap();
        assert_eq!(rest, words);
        assert_eq!(provider, None);

        let words = strings(&["--", "--provider", "flag"]);
        assert_eq!(extract_provider_flag(&words).unwrap(), (words, None));

        // `prompt` as a flag value is not the subcommand.
        let (rest, provider) = extract_provider_flag(&strings(&[
            "--model",
            "prompt",
            "--provider",
            "local",
            "doctor",
        ]))
        .unwrap();
        assert_eq!(rest, strings(&["--model", "prompt", "doctor"]));
        assert_eq!(provider, Some(ProviderId::Local));
    }

    #[test]
    fn provider_flag_errors_are_actionable_and_prompt_text_is_untouched() {
        assert!(extract_provider_flag(&strings(&["--provider"]))
            .unwrap_err()
            .contains("missing value"));
        assert!(extract_provider_flag(&strings(&["--provider", "claude"]))
            .unwrap_err()
            .contains("expected one of"));
        let (rest, provider) =
            extract_provider_flag(&strings(&["-p", "explain", "--provider", "flags"])).unwrap();
        assert_eq!(rest, strings(&["-p", "explain", "--provider", "flags"]));
        assert_eq!(provider, None);
    }
}

#[cfg(test)]
mod resume_prompt_tests {
    use super::{parse_args, CliAction, CliOutputFormat, ResumeLaunchOptions};
    use std::path::PathBuf;

    fn parse(values: &[&str]) -> Result<CliAction, String> {
        parse_args(&values.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    fn resume_prompt(session: &str, prompt: &str, launch: ResumeLaunchOptions) -> CliAction {
        CliAction::ResumePrompt {
            session_path: PathBuf::from(session),
            prompt: prompt.to_string(),
            output_format: CliOutputFormat::Text,
            launch,
            compact: false,
        }
    }

    #[test]
    fn resume_with_a_prompt_continues_the_saved_session() {
        assert_eq!(
            parse(&["--resume", "latest", "prompt", "fix", "the", "test"]),
            Ok(resume_prompt(
                "latest",
                "fix the test",
                ResumeLaunchOptions::default()
            ))
        );
        assert_eq!(
            parse(&["--resume", "latest", "-p", "x"]),
            Ok(resume_prompt("latest", "x", ResumeLaunchOptions::default()))
        );
        // No session named: the latest one, as with slash commands.
        assert_eq!(
            parse(&["--resume", "prompt", "x"]),
            Ok(resume_prompt("latest", "x", ResumeLaunchOptions::default()))
        );
    }

    #[test]
    fn resume_prompt_keeps_launch_flags_and_output_options() {
        assert_eq!(
            parse(&[
                "--model",
                "groq/openai/gpt-oss-120b",
                "--resume",
                "s-1",
                "prompt",
                "x"
            ]),
            Ok(resume_prompt(
                "s-1",
                "x",
                ResumeLaunchOptions {
                    model: Some("groq/openai/gpt-oss-120b".to_string()),
                    ..ResumeLaunchOptions::default()
                }
            ))
        );
        assert_eq!(
            parse(&[
                "--output-format",
                "json",
                "--compact",
                "--resume",
                "s-1",
                "prompt",
                "x"
            ]),
            Ok(CliAction::ResumePrompt {
                session_path: PathBuf::from("s-1"),
                prompt: "x".to_string(),
                output_format: CliOutputFormat::Json,
                launch: ResumeLaunchOptions::default(),
                compact: true,
            })
        );
    }

    #[test]
    fn resume_without_a_prompt_is_unchanged() {
        assert_eq!(
            parse(&["--resume", "latest"]),
            Ok(CliAction::ResumeSession {
                session_path: PathBuf::from("latest"),
                commands: vec![],
                output_format: CliOutputFormat::Text,
                launch: ResumeLaunchOptions::default(),
            })
        );
    }

    #[test]
    fn a_session_flag_after_prompt_is_refused_not_sent_to_the_model() {
        for words in [
            &["prompt", "--resume", "latest", "hi"][..],
            &["prompt", "--resume=latest", "hi"],
            &["prompt", "--continue", "hi"],
            &["-p", "--resume", "latest", "hi"],
        ] {
            let error = parse(words).expect_err("a session flag after prompt is refused");
            assert!(
                error.contains("neuron --resume latest prompt \"...\""),
                "{words:?}: {error}"
            );
        }
    }

    #[test]
    fn resume_words_that_are_no_slash_command_name_the_prompt_form() {
        let error = parse(&["--resume", "latest", "hi"]).expect_err("not a slash command");
        assert!(error.contains("/status"), "{error}");
        assert!(
            error.contains("neuron --resume latest prompt \"...\""),
            "{error}"
        );
        for words in [
            &["--resume", "latest", "prompt"][..],
            &["--resume", "latest", "prompt", " "],
        ] {
            let error = parse(words).expect_err("an empty prompt is refused");
            assert!(error.contains("requires a prompt"), "{words:?}: {error}");
        }
    }

    /// `echo "..." | neuron --resume latest -p`: as with `neuron -p`, the prompt comes on
    /// standard input.
    #[test]
    fn resume_with_an_empty_p_takes_the_prompt_from_stdin() {
        assert_eq!(
            parse(&["--resume", "latest", "-p"]),
            Ok(resume_prompt("latest", "", ResumeLaunchOptions::default()))
        );
    }

    /// `neuron prompt hello --resume latest` started a new session with the prompt
    /// "hello --resume latest": a session flag anywhere among a prompt's words is refused.
    #[test]
    fn a_session_flag_anywhere_in_the_prompt_is_refused() {
        for words in [
            &["prompt", "hello", "--resume", "latest"][..],
            &["-p", "hello", "--resume", "latest"],
            &["hello", "--resume", "latest"],
            &["prompt", "hello", "--continue"],
            &["--resume", "latest", "prompt", "--resume", "other", "x"],
            &["--resume", "latest", "prompt", "x", "--resume=other"],
            &["--resume", "latest", "-p", "x", "--continue"],
        ] {
            let error = parse(words).expect_err("a session flag in the prompt is refused");
            assert!(
                error.contains("neuron --resume latest prompt \"...\""),
                "{words:?}: {error}"
            );
        }
    }

    #[test]
    fn help_after_a_resumed_prompt_shows_help() {
        for words in [
            &["--resume", "latest", "prompt", "--help"][..],
            &["--resume", "latest", "prompt", "x", "-h"],
            &["--resume", "latest", "-p", "--help"],
            &["--resume", "latest", "--help"],
            &["--resume", "--help"],
            &["-p", "x", "--help"],
        ] {
            assert_eq!(
                parse(words),
                Ok(CliAction::Help {
                    output_format: CliOutputFormat::Text
                }),
                "{words:?}"
            );
        }
    }

    /// After `prompt` Neuron's own options still apply, as for `neuron prompt`.
    #[test]
    fn options_after_a_resumed_prompt_apply() {
        assert_eq!(
            parse(&[
                "--resume",
                "latest",
                "prompt",
                "again",
                "--output-format",
                "json"
            ]),
            Ok(CliAction::ResumePrompt {
                session_path: PathBuf::from("latest"),
                prompt: "again".to_string(),
                output_format: CliOutputFormat::Json,
                launch: ResumeLaunchOptions::default(),
                compact: false,
            })
        );
    }

    /// After `-p` every word is prompt text, so an option `prompt` would apply there was
    /// sent to the model instead (`--resume latest -p again --output-format json` printed
    /// a text answer): refused, saying where it goes.
    #[test]
    fn an_option_after_p_is_refused_not_sent_to_the_model() {
        for (words, option) in [
            (
                &[
                    "--resume",
                    "latest",
                    "-p",
                    "again",
                    "--output-format",
                    "json",
                ][..],
                "--output-format",
            ),
            (&["-p", "again", "--output-format=json"], "--output-format"),
            (&["-p", "again", "--model", "opus"], "--model"),
            (
                &[
                    "--resume",
                    "s-1",
                    "-p",
                    "x",
                    "--permission-mode",
                    "read-only",
                ],
                "--permission-mode",
            ),
            (&["-p", "x", "--compact"], "--compact"),
        ] {
            let error = parse(words).expect_err("an option after -p is refused");
            assert!(
                error.contains(&format!("{option} after `-p`")),
                "{words:?}: {error}"
            );
            assert!(error.contains("before `-p`"), "{words:?}: {error}");
        }
        // Words that only look like options are prompt text.
        match parse(&["-p", "what", "does", "--modelling", "-x", "mean"]) {
            Ok(CliAction::Prompt { prompt, .. }) => {
                assert_eq!(prompt, "what does --modelling -x mean");
            }
            other => panic!("expected a prompt: {other:?}"),
        }
    }
}

#[cfg(test)]
mod script_input_tests {
    use super::{
        msys_slash_command, msys_slash_command_under, parse_args, parse_args_with_stdin,
        piped_prompt, CliAction,
    };

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    fn parse_piped(values: &[&str], stdin: &str) -> Result<CliAction, String> {
        parse_args_with_stdin(&strings(values), || Some(stdin.to_string()))
    }

    #[test]
    fn piped_slash_commands_are_refused_not_sent_to_the_model() {
        // Windows PowerShell pipes, and files saved as UTF-8 with BOM, start with U+FEFF.
        for stdin in [
            "/help\n/exit",
            "\n  /model\n/exit\n",
            "/status\r\n",
            "\u{feff}/help\r\n/exit\r\n",
            "\u{feff}\r\n/model\r\n/exit\r\n",
        ] {
            let error = parse_piped(&[], stdin).expect_err("piped slash commands are refused");
            assert!(
                error.contains("slash commands can't be piped"),
                "{stdin:?}: {error}"
            );
            assert!(error.contains("neuron --resume latest /status"), "{error}");
        }
    }

    #[test]
    fn a_piped_prompt_is_unchanged() {
        let piped = parse_piped(&["--permission-mode", "read-only"], "fix the bug\n");
        assert_eq!(
            piped,
            parse_args(&strings(&["--permission-mode", "read-only", "fix the bug"]))
        );
        assert!(matches!(piped, Ok(CliAction::Prompt { prompt, .. }) if prompt == "fix the bug"));
        // A path is not a slash command.
        let piped = parse_piped(
            &["--permission-mode", "read-only"],
            "/usr/bin/env is missing",
        );
        assert!(
            matches!(piped, Ok(CliAction::Prompt { ref prompt, .. }) if prompt == "/usr/bin/env is missing"),
            "{piped:?}"
        );
    }

    #[test]
    fn piped_input_with_no_tui_names_the_terminal_it_needs() {
        let error = piped_prompt("fix the bug", true).expect_err("--no-tui needs a terminal");
        assert!(error.contains("--no-tui"), "{error}");
        assert!(error.contains("leave out --no-tui"), "{error}");
        let error = piped_prompt("\u{feff}fix the bug\r\n", true).expect_err("with a BOM too");
        assert!(error.contains("leave out --no-tui"), "{error}");
        // Nothing piped (a lone BOM is nothing): the REPL arm decides.
        for no_tui in [true, false] {
            assert_eq!(piped_prompt(" \n", no_tui), Ok(None));
            assert_eq!(piped_prompt("\u{feff}", no_tui), Ok(None));
            assert_eq!(piped_prompt("\u{feff}\r\n", no_tui), Ok(None));
        }
        assert!(matches!(
            parse_piped(&[], "\u{feff}"),
            Ok(CliAction::Repl { .. })
        ));
        // The BOM is not part of the prompt.
        for stdin in ["fix the bug\n", "\u{feff}fix the bug\r\n"] {
            assert_eq!(
                piped_prompt(stdin, false),
                Ok(Some("fix the bug".to_string())),
                "{stdin:?}"
            );
        }
    }

    #[test]
    fn git_bash_paths_of_slash_commands_are_slash_commands_again() {
        let git_bash = Some("MINGW64");
        for (arg, command) in [
            ("C:/Program Files/Git/model", "/model"),
            ("C:/Program Files/Git/usr/skills", "/skills"),
            ("D:/tools/Git/help", "/help"),
        ] {
            assert_eq!(
                msys_slash_command(arg, git_bash).as_deref(),
                Some(command),
                "{arg}"
            );
        }
        // Not Git Bash, not a slash command, or not under Git: left alone.
        assert_eq!(msys_slash_command("C:/Program Files/Git/model", None), None);
        assert_eq!(
            msys_slash_command("C:/Program Files/Git/average", git_bash),
            None
        );
        assert_eq!(msys_slash_command("C:/projects/app/model", git_bash), None);
        assert_eq!(msys_slash_command("C:/usr/model", git_bash), None);
        assert_eq!(msys_slash_command("fix the model", git_bash), None);
    }

    #[test]
    fn msys2_and_scoop_paths_of_slash_commands_are_slash_commands_again() {
        let msys2 = Some("UCRT64");
        for (arg, root, command) in [
            ("C:/msys64/model", "C:/msys64", "/model"),
            ("C:/msys64/usr/status", "C:/msys64", "/status"),
            (
                "C:/Users/me/scoop/apps/git/current/help",
                "C:/Users/me/scoop/apps/git/current",
                "/help",
            ),
        ] {
            assert_eq!(
                msys_slash_command_under(arg, msys2, |folder| folder == root).as_deref(),
                Some(command),
                "{arg}"
            );
            // A folder that is not an MSYS install is left alone.
            assert_eq!(msys_slash_command_under(arg, msys2, |_| false), None);
        }
    }

    /// The real check: an MSYS install folder holds `usr/bin/msys-2.0.dll`.
    #[cfg(windows)]
    #[test]
    fn an_msys_install_is_found_by_its_runtime_dll() {
        let root = std::env::temp_dir().join(format!("neuron-msys-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("usr/bin")).expect("usr/bin should create");
        std::fs::write(root.join("usr/bin/msys-2.0.dll"), b"").expect("dll should write");
        let root_text = root.display().to_string().replace('\\', "/");

        let found = msys_slash_command(&format!("{root_text}/model"), Some("MSYS"));
        let elsewhere = msys_slash_command(&format!("{root_text}/usr/bin/model"), Some("MSYS"));

        std::fs::remove_dir_all(&root).expect("temp dir should clean up");
        assert_eq!(found.as_deref(), Some("/model"));
        assert_eq!(elsewhere, None);
    }

    #[test]
    fn under_git_bash_a_mangled_slash_command_is_not_a_prompt() {
        let _guard = crate::tests::env_lock();
        let previous = std::env::var_os("MSYSTEM");
        std::env::set_var("MSYSTEM", "MINGW64");
        let model = parse_args(&strings(&["C:/Program Files/Git/model"]));
        let skills = parse_args(&strings(&["C:/Program Files/Git/skills"]));
        // The first of several words (`neuron /skills list`), and words after --resume.
        let skills_list = parse_args(&strings(&["C:/Program Files/Git/skills", "list"]));
        let resumed = parse_args(&strings(&[
            "--resume",
            "latest",
            "C:/Program Files/Git/status",
        ]));
        // Only where a slash command goes: a later word of a prompt is left alone.
        let prompt = parse_args(&strings(&["explain", "C:/Program Files/Git/model"]));
        match previous {
            Some(value) => std::env::set_var("MSYSTEM", value),
            None => std::env::remove_var("MSYSTEM"),
        }
        let error = model.expect_err("/model is interactive-only");
        assert!(error.contains("slash command /model"), "{error}");
        assert!(
            matches!(skills, Ok(CliAction::Skills { args: None, .. })),
            "{skills:?}"
        );
        assert!(
            matches!(skills_list, Ok(CliAction::Skills { args: Some(ref args), .. }) if args == "list"),
            "{skills_list:?}"
        );
        assert!(
            matches!(resumed, Ok(CliAction::ResumeSession { ref commands, .. }) if commands == &["/status"]),
            "{resumed:?}"
        );
        assert!(
            matches!(prompt, Ok(CliAction::Prompt { prompt: ref text, .. }) if text == "explain C:/Program Files/Git/model"),
            "{prompt:?}"
        );
    }
}
