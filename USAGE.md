# NeuronCLI usage

Build from `rust/` with `cargo build --release -p rusty-claude-cli`. The executable is `rust/target/release/neuron` (`neuron.exe` on Windows).

Run `neuron auth login` to open the Zero-X account sign-in page. The client binds an OS-assigned loopback port before opening the browser, validates the callback state and origin, and stores only the encrypted gateway session. Run `neuron auth logout` to revoke and remove it; clear `NEURON_TOKEN` separately if supplied in your shell.

For headless use, set `NEURON_TOKEN` and optionally `NEURON_API_BASE` (default `https://zero-x.live/v1`). The server must have account migrations and provider configuration applied. Gateway provider credentials are never sent to the CLI. Run `/doctor` inside the interactive client for diagnostics.

Direct providers are optional: configure `OPENAI_API_KEY` plus `OPENAI_BASE_URL`, or an Anthropic API key and explicit Anthropic model. Gateway `auto` selects a configured tool-capable model. Model availability and quotas come from the gateway rather than a hard-coded client list.

Use `neuron --help` for current flags. Start in the directory to work on and review the trust prompt before allowing file edits and commands. The JavaScript launcher uses the same native terminal runtime; build it with `npm ci` and `npm run tui:build` from the repository root.

Verification and release commands are in [README.md](README.md). Historical parity and roadmap documents retain upstream terminology and are not the source of current cloud provisioning instructions.
