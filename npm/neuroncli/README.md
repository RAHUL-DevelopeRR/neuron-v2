# @zero-x.live/neuron

Neuron is a terminal coding agent. It reads your project, edits files and runs
commands with your permission, and talks to models through the Zero-X gateway
(free plan included) or your own provider key.

Requires Node.js 22.14 or later on Linux (x64, arm64, glibc), macOS (Apple
silicon, Intel) or Windows (x64).

```sh
npm install --global @zero-x.live/neuron   # the 7.0.0 alpha
neuron auth login
neuron
```

## How the binary is installed

Neuron is a native Rust program. This package contains a small launcher:

- `postinstall` downloads `neuron-<platform>` and the full-screen UI,
  `neuron-tui-<platform>`, for this exact package version from the project's
  GitHub Releases and checks each against the release's SHA-256 file before
  installing it next to the launcher.
- If install scripts were skipped (`npm install --ignore-scripts`) or the
  download failed because of the network, the first `neuron` run downloads and
  verifies it instead. If the package directory is read-only, the binary goes
  to the user cache (`%LOCALAPPDATA%\neuroncli`, `~/Library/Caches/neuroncli` or
  `~/.cache/neuroncli`), shared with the Python package.
- After that, `neuron` starts without network access. Ctrl+C reaches Neuron
  itself, and the launcher exits with Neuron's exit code.

Environment variables:

| Variable | Purpose |
| --- | --- |
| `NEURON_SKIP_DOWNLOAD=1` | Skip the download during `postinstall`. |
| `NEURON_BINARY_PATH` | Run this native binary instead (development builds). |
| `NEURON_CACHE_DIR` | Cache directory for downloaded binaries. |
| `NEURON_DOWNLOAD_BASE` | Release mirror base URL (HTTPS, or loopback HTTP for testing). |

Behind a proxy on Node 24+, set `NODE_USE_ENV_PROXY=1` together with
`HTTPS_PROXY` so the download goes through it.

## Default integrations

Ponytail, antislop, gstack and Agent Reach instructions are embedded in the
binary and loaded on demand. The Codebase Memory and Playwright MCP servers are installed with this
package and enabled by default. Agent Reach's CLI channels need their own setup;
Playwright provides browser control, while desktop control needs a separately
configured MCP server. See
[default integrations](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/docs/DEFAULT_INTEGRATIONS.md)
to override or disable them.

## More

- [Usage guide](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md)
- [Plans and pricing](https://zero-x.live/pricing)
- [Security policy](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/SECURITY.md)

MIT licensed; the full-screen UI it downloads is Apache-2.0 (see `NOTICE`).
Prompts and code you send are processed by third-party model providers.
