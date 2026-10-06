# NeuronCLI

NeuronCLI is the coding agent in this repository. Its Rust runtime lives in `rust/`; the JavaScript terminal launcher lives in `ui/react-tui/`. The project derives from Claw Code; upstream reference and parity documents remain for development history.

## Build and run

```sh
git clone https://github.com/RAHUL-DevelopeRR/neuron-v2.git
cd neuron-v2/rust
cargo build --release -p rusty-claude-cli
./target/release/neuron --version
./target/release/neuron auth login
./target/release/neuron
```

On Windows, run `target\release\neuron.exe`. Login opens the Zero-X account page and saves an encrypted gateway session locally. Provider keys stay on the gateway. Login requires the updated Zero-X gateway and account database migrations to be deployed.

## Install from package registries

Install the released CLI with `npm install --global @zero-x.live/neuron` or `python -m pip install neuroncli`, then run `neuron auth login`. Package installers download the matching versioned native binary from GitHub Releases and verify its SHA-256 checksum. The initial download requires GitHub Releases access.

For an existing gateway session in automation:

```sh
export NEURON_API_BASE=https://zero-x.live/v1
export NEURON_TOKEN=ses_your_gateway_session
```

Use `neuron auth status` to inspect credential availability and `neuron auth logout` to revoke the session. Gateway credentials expire; sign in again when the gateway returns 401. Available models come from the server's configured catalog. Free provider quotas and availability are determined by those providers and server configuration.

Advanced direct-provider use remains available through `OPENAI_API_KEY` and `OPENAI_BASE_URL`, or explicit Anthropic credentials and an Anthropic model. Do not embed provider credentials in distributed clients.

Ponytail, AntiSlop and the gstack router are included in every new conversation.
Their companion skills are embedded in the native binary. See
[default skills and integrations](docs/DEFAULT_INTEGRATIONS.md) for custom skills,
plugins, MCP servers and optional browser control.

## Verification

```sh
cd rust
cargo fmt --all --check
cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cd ..
npm ci
npm run tui:build
python .github/scripts/check_doc_source_of_truth.py
```

Tests run serially because fixtures change process-wide environment variables and working directories. CI checks the `ux-audit-fixes` default branch and pull requests, including Windows, Linux, and macOS workspace tests. Release artifacts use the `neuron` binary name and include SHA-256 files.

See [USAGE.md](USAGE.md) for current setup and [rust/](rust/) for implementation. Older parity and architecture documents describe upstream behavior and may include historical `claw` commands.
