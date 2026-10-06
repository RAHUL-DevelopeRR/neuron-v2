# OpenCode upstream review

Reviewed 2026-10-06 against the official [anomalyco/opencode](https://github.com/anomalyco/opencode)
`dev` branch and latest release [v1.18.34](https://github.com/anomalyco/opencode/releases/tag/v1.18.34).
The latest dev snapshot, `652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef`, was fetched
into `refs/remotes/opencode-review/dev` for inspection. The nine commits since the
previous snapshot update console navigation, CI, ecosystem documentation, GitLab's
TypeScript provider dependency, Nix hashes, and unused artifacts. They require no
changes to Neuron's Rust runtime.

Neuron is a Rust Claw Code derivative, not a Git fork of the current TypeScript/Bun
OpenCode project. Neuron's initial import is `b723d5e`; its runtime is
`rust/crates/rusty-claude-cli`. The Go architecture in `OPENCODE_INTEGRATION.md`
describes the archived [opencode-ai/opencode](https://github.com/opencode-ai/opencode).
Merging unrelated OpenCode commits into this workspace would not update Neuron's runtime.

Recent changes reviewed for applicability:

| Official change | Neuron decision |
| --- | --- |
| [Gateway timeout coverage](https://github.com/anomalyco/opencode/commit/35fc7a776cdd) | Keep bounded transport across every route. Neuron's shared Rust HTTP client already has a 30-second connection timeout and five-minute request timeout. Gateway catalog/chat adapters have their own bounded requests. OpenCode's Bun fetch wrapper cannot be transplanted into Rust. |
| [Preserve Bedrock model IDs](https://github.com/anomalyco/opencode/commit/ac1758c0e6b8) | Preserve ARN and regional profile IDs; do not broadly rewrite DeepSeek IDs. The Zero-X gateway forwards each catalog entry's upstream model ID verbatim. |
| [Windows plugin display paths](https://github.com/anomalyco/opencode/commit/97a86b7677c3) | Neuron uses native `Path::file_name()` for plugin names. Keep the native path implementation. |
| [Namespaced session headers](https://github.com/anomalyco/opencode/commit/e9f8a210b9e2) | Do not send OpenCode identity headers from Neuron. Adding Neuron session affinity later requires tracing session IDs through the provider interface and an explicit gateway contract. |
| [macOS signing](https://github.com/anomalyco/opencode/commit/f66b86ceec1a) | OpenCode repairs Bun cross-compiled signatures. Neuron ships native Rust binaries; Developer ID signing needs its own certificate and release setup. |

The review found two actionable Neuron bugs addressed independently of OpenCode:

- The Python launcher could execute another `neuron` on PATH, bypassing its version-matched release and potentially recursing into another launcher. It now uses the packaged or checksum-verified downloaded binary; developers can explicitly set `NEURON_BINARY_PATH`.
- Provider selection reported an exhausted Azure quota even when Azure was never configured. Azure quota and reachability checks now run only with both explicit Azure credentials and endpoint configured.

These changes preserve the existing agent, plugin, MCP, skill, streaming, and
gateway interfaces. No unrelated OpenCode source or dependencies were merged.
