//! `neuron engine --share`: shares a Neuron session with the user's phone through the Zero-X relay
//! (the `neuron-share` crate). The session keeps running here, in a local Neuron host that serves
//! it over remote protocol 1 (`bun run host` in `ui/neuron-client` today); this command bridges
//! that host to the relay, end-to-end encrypted, and shows the pairing QR code.
//!
//! The relay needs the user's Zero-X sign-in (a `ses_` session from `neuron login`, or
//! `NEURON_TOKEN`); API keys cannot share sessions.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use neuron_share::contract::DEFAULT_RELAY_URL;
use neuron_share::pairing::{
    build_pairing_link, build_pairing_web_link, terminal_qr, PairingPayload,
};
use neuron_share::{Reporter, ShareOptions};

use crate::credentials::TokenKind;

const USAGE: &str =
    "usage: neuron engine --share --host ws://127.0.0.1:<port>/neuron --host-token-file FILE \
[--relay wss://relay.zero-x.live/v1/relay] [--name NAME] [--no-pair]\n\
The host and its token file come from `bun run host` in ui/neuron-client (or NEURON_HOST_URL and \
NEURON_HOST_TOKEN_FILE). Press Enter while sharing for a new pairing code.";
/// How long a pairing code works.
const PAIR_TTL_S: u64 = 600;

/// The share's flags when `args` ask for it: `engine` with `--share` among the flags, and not
/// `--stdio` (that is the engine for a UI).
pub(crate) fn share_engine_args(args: &[String]) -> Option<Vec<String>> {
    let position = args.iter().position(|arg| arg == "engine")?;
    let flags: Vec<String> = args[..position]
        .iter()
        .chain(&args[position + 1..])
        .cloned()
        .collect();
    (flags.iter().any(|flag| flag == "--share") && !flags.iter().any(|flag| flag == "--stdio"))
        .then_some(flags)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ShareArgs {
    host: Option<String>,
    token_file: Option<PathBuf>,
    relay: Option<String>,
    name: Option<String>,
    no_pair: bool,
}

fn parse_share_args(args: &[String]) -> Result<ShareArgs, String> {
    let mut parsed = ShareArgs::default();
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
            "--share" if switch => {}
            "--no-pair" if switch => parsed.no_pair = true,
            "--host" => parsed.host = Some(value()?),
            "--host-token-file" => parsed.token_file = Some(PathBuf::from(value()?)),
            "--relay" => parsed.relay = Some(value()?),
            "--name" => parsed.name = Some(value()?),
            _ => {
                return Err(format!(
                    "unknown argument for `neuron engine --share`: {arg}\n{USAGE}"
                ))
            }
        }
    }
    Ok(parsed)
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The computer's name on the phone.
fn computer_name() -> String {
    non_empty_env("COMPUTERNAME")
        .or_else(|| non_empty_env("HOSTNAME"))
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "This computer".to_string())
}

/// Shares until Ctrl+C.
pub(crate) fn run_share_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let args = parse_share_args(args)?;
    let host = args
        .host
        .or_else(|| non_empty_env("NEURON_HOST_URL"))
        .ok_or_else(|| {
            format!("`neuron engine --share` needs the Neuron host to share (--host).\n{USAGE}")
        })?;
    let token_file = args
        .token_file
        .or_else(|| non_empty_env("NEURON_HOST_TOKEN_FILE").map(PathBuf::from))
        .ok_or_else(|| {
            format!(
                "`neuron engine --share` needs the host's token file (--host-token-file).\n{USAGE}"
            )
        })?;
    let local_token = fs::read_to_string(&token_file)
        .map_err(|error| {
            format!(
                "Could not read the host's token file {}: {error}",
                token_file.display()
            )
        })?
        .trim()
        .to_string();
    if local_token.is_empty() {
        return Err(format!("The host's token file {} is empty.", token_file.display()).into());
    }
    let relay_url = args
        .relay
        .or_else(|| non_empty_env("NEURON_RELAY_URL"))
        .unwrap_or_else(|| DEFAULT_RELAY_URL.to_string());
    let credential = crate::auth::current_credential()?
        .ok_or("Sharing needs your Zero-X sign-in: run `neuron login`, then share again.")?;
    if credential.kind != TokenKind::Session {
        return Err("Sharing needs your Zero-X sign-in, not an API key: run `neuron login` (NEURON_TOKEN holds an nzx_ key).".into());
    }
    if credential.is_expired(crate::clock::now_unix()) {
        return Err("Your Zero-X sign-in expired: run `neuron login`, then share again.".into());
    }
    let options = ShareOptions {
        relay_url,
        token: credential.token.expose().to_string(),
        config_dir: crate::credentials::neuron_home(),
        name: args.name.unwrap_or_else(computer_name),
        platform: env::consts::OS.to_string(),
        local_url: host,
        local_token,
        pair_ttl_s: PAIR_TTL_S,
        offer_at_start: !args.no_pair,
        read_stdin: true,
        stop: None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(neuron_share::run(options, Box::new(TerminalReporter)))?;
    Ok(())
}

/// Status on stderr; the pairing code on stdout.
struct TerminalReporter;

impl Reporter for TerminalReporter {
    fn status(&mut self, line: &str) {
        eprintln!("{line}");
    }

    fn offer(&mut self, payload: &PairingPayload) {
        let link = build_pairing_link(payload);
        println!("\nScan this with Neuron on your phone (Computers, then Pair):\n");
        if let Some(qr) = terminal_qr(&link) {
            println!("{qr}");
        }
        println!("Or paste this pairing link into the app:\n{link}\n");
        println!(
            "From the phone's camera app: {}",
            build_pairing_web_link(payload)
        );
        println!(
            "The code works once, for {} minutes. Press Enter for a new code, Ctrl+C to stop sharing.\n",
            PAIR_TTL_S / 60
        );
    }

    fn paired(&mut self, phone_name: &str) {
        println!("Paired with {phone_name}. It can open this session now.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn reads_the_share_flags() {
        assert_eq!(
            share_engine_args(&args("engine --stdio")),
            None,
            "the UI engine is another command"
        );
        assert_eq!(share_engine_args(&args("engine --share --stdio")), None);
        assert_eq!(share_engine_args(&args("fix the engine")), None);
        let flags = share_engine_args(&args(
            "--name Desk engine --share --host ws://127.0.0.1:9/neuron",
        ))
        .unwrap();
        let parsed = parse_share_args(&flags).unwrap();
        assert_eq!(parsed.host.as_deref(), Some("ws://127.0.0.1:9/neuron"));
        assert_eq!(parsed.name.as_deref(), Some("Desk"));
        assert!(!parsed.no_pair);
        let parsed = parse_share_args(&args(
            "--share --relay=ws://127.0.0.1:8790/v1/relay --no-pair --host-token-file t",
        ))
        .unwrap();
        assert_eq!(
            parsed.relay.as_deref(),
            Some("ws://127.0.0.1:8790/v1/relay")
        );
        assert_eq!(parsed.token_file, Some(PathBuf::from("t")));
        assert!(parsed.no_pair);
        assert!(parse_share_args(&args("--share --term")).is_err());
        assert!(parse_share_args(&args("--share --host")).is_err());
    }
}
