# Neuron Rust workspace

The Rust workspace that builds the `neuron` binary. For installing and using Neuron,
see the [README](../README.md) and the [usage guide](../USAGE.md); for contributing,
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Build and test

```bash
cd rust/
cargo build -p rusty-claude-cli            # target/debug/neuron
cargo run -p rusty-claude-cli -- --help    # the live command list
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests are hermetic: they run against fake gateways and a mock model service on
loopback ports, with throwaway `HOME`, `NEURON_HOME` and config directories, so they
never touch your login or keychain.

## Mock parity harness

`crates/mock-anthropic-service` is a deterministic Anthropic-compatible mock, and
`crates/rusty-claude-cli/tests/mock_parity_harness.rs` drives the real binary through
twelve scripted scenarios in a clean environment (streaming text, file tools, bash
with approval and denial, plugin tools, auto-compaction, token and cost reporting).

```bash
./scripts/run_mock_parity_harness.sh                 # or: cargo test -p rusty-claude-cli --test mock_parity_harness
cargo run -p mock-anthropic-service -- --bind 127.0.0.1:0   # the mock on its own
python3 scripts/run_mock_parity_diff.py              # scenario checklist against PARITY.md
```

`mock_parity_scenarios.json` maps each scenario to the parity items it covers.

## Crates

| Crate | Responsibility |
| --- | --- |
| `api` | Provider clients (Anthropic, OpenAI-compatible, the Zero-X gateway), streaming, gateway error envelopes and the retry policy |
| `commands` | Slash command registry, parsing and help, skill discovery |
| `compat-harness` | Manifest extraction from the upstream TypeScript sources (parity tooling) |
| `mock-anthropic-service` | Deterministic `/v1/messages` mock for the parity harness |
| `plugins` | Plugin manifests, install/enable/disable, plugin tools, hooks, shared subprocess helpers (timeouts, tree kill, credential scrubbing) |
| `runtime` | Conversation loop, context manager, sessions and transcript repair, config loading and validation, permissions and the shell command classifier, MCP client, system prompt and bundled skills |
| `rusty-claude-cli` | The `neuron` binary: argument parsing, provider selection, Zero-X login and credential store, REPL, rendering |
| `telemetry` | Session trace event types |
| `tools` | Built-in tool specs and execution (bash, PowerShell, file tools, search, web, Agent, Skill, ToolSearch) |

Crate and module names (`rusty-claude-cli`, `.claw` settings fallbacks) are inherited
from [Claw Code](https://github.com/ultraworkers/claw-code); see
[docs/PROVENANCE.md](../docs/PROVENANCE.md).

## Defaults worth knowing

- Binary: `neuron`. Default model: `auto` (the provider chooses).
- Default permission mode: `workspace-write`.
- Provider: the Zero-X gateway unless `NEURON_TOKEN`, `--provider`, `NEURON_PROVIDER`,
  the `provider` setting or a provider key selects another (see
  [USAGE.md](../USAGE.md#bring-your-own-key)).
- `neuron system-prompt` prints the system prompt for a directory; the bundled skills
  appear there as a short index and load through the Skill tool.
