# Neuron

Neuron by [Zero-X](https://zero-x.live/neuron-cli) is an open-source AI coding agent
that runs in your terminal. It reads your project, edits files and runs commands, and
asks before it does anything your permission mode does not allow. It is a single
native binary written in Rust. New to coding agents? Read
[how an AI coding agent works](https://zero-x.live/ai-coding-agent).

By default Neuron talks to models through the [Zero-X gateway](https://zero-x.live).
You sign in once with `neuron auth login`; the provider API keys stay on the gateway
and never reach your machine. You can also use your own provider key instead.

> 7.0.0-alpha.2 is Neuron's second alpha, built and published by this repository's
> release pipeline, and the first on npm and PyPI. Its known limitations are listed in
> the [changelog](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/CHANGELOG.md).
> It is a GitHub prerelease, so the install scripts need the versioned commands below.
> The npm 6.2.0 and PyPI 6.2.4 (and earlier) packages were built outside the pipeline
> and are not supported.

## Install

Every method installs the same `neuron` binary from
[GitHub Releases](https://github.com/RAHUL-DevelopeRR/neuron-v2/releases) and checks
its SHA-256 checksum before using it.

A prerelease is never the "latest" GitHub release, so the install script commands below
name the alpha's version. Without one, the install scripts look up the latest release,
and there is none until the first stable release is out.

**macOS and Linux**

```sh
curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.2/install.sh | sh -s -- --version 7.0.0-alpha.2
```

The script installs to `~/.local/bin` and tells you if that directory is not on your
`PATH`. Pass options after `sh -s --`, for example
`sh -s -- --version 7.0.0-alpha.2 --install-dir "$HOME/bin"`, or set `NEURON_VERSION`
and `NEURON_INSTALL_DIR`.

**Windows (PowerShell 5.1 or 7)**

```powershell
& ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.2/install.ps1))) -Version 7.0.0-alpha.2
```

The script installs to `%LOCALAPPDATA%\Programs\Neuron\bin` and only changes your
`PATH` if you ask it to:

```powershell
& ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.2/install.ps1))) -Version 7.0.0-alpha.2 -AddToPath
```

**npm** (Node.js 22.14 or later)

```sh
npm install --global @zero-x.live/neuron
```

**pip** (Python 3.9 or later). The alpha is a pre-release on PyPI, so pip needs
`--pre`:

```sh
pip install --pre neuroncli
```

The npm and pip packages are small launchers that download the binaries for the
package's exact version and verify their checksums. npm downloads them during
`npm install`, without showing progress: `neuron` (about 17 MB) and `neuron-tui`
(about 110 MB) go into the package's `bin/native` folder. pip downloads them the first
time you run `neuron`, into a per-version cache (`~/.cache/neuroncli` on Linux,
`~/Library/Caches/neuroncli` on macOS, `%LOCALAPPDATA%\neuroncli` on Windows); npm
uses that cache only when its package folder is read-only. After that, Neuron starts
without network access. Ctrl+C goes to Neuron, and the launcher exits with Neuron's
exit code.

With the installers or pip, the two default MCP servers (CodebaseMap and Playwright)
start through `npx -y`, so they need Node.js; the npm package installs them with
Neuron.

On a terminal, `neuron` with no prompt opens the full-screen UI, `neuron-tui`. The
installers put it beside `neuron`, and the launchers fetch it the same way, verified
by checksum. `neuron --no-tui` (or `NEURON_NO_TUI=1`) keeps the line REPL. Without
the UI, for example after a failed download, `neuron` runs the line REPL as
before. Prompts (`neuron "fix the test"`, `neuron -p ...`) and subcommands never
start it.

**From source**

```sh
git clone https://github.com/RAHUL-DevelopeRR/neuron-v2.git
cd neuron-v2/rust
cargo build --release -p rusty-claude-cli
./target/release/neuron --version
```

### Supported platforms

| Platform | Release assets |
| --- | --- |
| Linux x64, glibc 2.35 or later | `neuron-linux-x64`, `neuron-tui-linux-x64` |
| Linux arm64, glibc 2.35 or later | `neuron-linux-arm64`, `neuron-tui-linux-arm64` |
| macOS, Apple silicon | `neuron-macos-arm64`, `neuron-tui-macos-arm64` |
| macOS, Intel | `neuron-macos-x64`, `neuron-tui-macos-x64` |
| Windows x64 | `neuron-windows-x64.exe`, `neuron-tui-windows-x64.exe` (Windows on Arm runs them under emulation) |

glibc 2.35 or later means Ubuntu 22.04, Debian 12, Fedora 36 or newer. On older
distributions (for example Ubuntu 20.04, Debian 11 or RHEL 9) the binary may not
start; the installers check that before they install anything, and you can build
from source instead. musl-based Linux such as Alpine is not supported. The binaries are not code-signed or
notarized. Each release lists SHA-256 checksums (`SHA256SUMS`, plus a `.sha256` file
per binary) and GitHub build provenance, which you can check with
`gh attestation verify neuron-linux-x64 --repo RAHUL-DevelopeRR/neuron-v2`.

## Sign in

```sh
neuron login          # shows a code; approve it on neuron.zero-x.live/device
neuron auth status
neuron logout         # revokes the session on the gateway and deletes the local copy
```

Login prints a link and a short code and opens the link in your browser when it can.
Approve the code there (or on any other device, such as your phone) and Neuron
continues on its own; no local port is opened, so it also works over SSH. It stores the
session in the operating system's credential store on macOS and Windows. On Linux (or
when that store is unavailable) it goes to `~/.neuroncli/credentials.json`, readable
only by you; protect that directory like a password file. The
[security policy](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/SECURITY.md#how-neuron-handles-credentials)
explains what to do if it leaks. On CI and machines without a browser,
create a personal API key (`nzx_…`) in the Zero-X dashboard and pass it with
`neuron auth login --with-token` or the `NEURON_TOKEN` environment variable. See
[Sign in](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md#sign-in)
in the usage guide.

## Plans and limits

**Free.** No card needed. Free-plan requests run on free tiers of upstream providers:
Groq, the Gemini API free tier, Cloudflare Workers AI, NVIDIA's API catalog and
OpenRouter's free models. The limits are real and you will run into them:

| Free plan | Limit |
| --- | --- |
| Requests per day | 100 |
| Tokens per day | 500,000 |
| Requests per minute | 10 |
| Concurrent requests | 2 |

- Everyone on the free plan shares the same upstream capacity. When a shared pool
  runs out, requests fail with `capacity_exhausted` and a time to retry.
- Some free models have small per-minute token budgets, so a long prompt may fit on
  only some models, or on none.
- The Gemini free tier is not offered to callers in the EEA, the UK or Switzerland.
- Which models are free depends on what the providers offer, and can change at any
  time. `/model` lists what is available to you now; outside a session,
  `curl https://zero-x.live/v1/models` shows the current catalog.

**Pro and Max.** Paid plans raise the daily and per-minute limits and add paid
upstream capacity, billed against monthly credits included with the plan; credit
top-ups are also sold. Prices and current limits are at
<https://zero-x.live/pricing>. In Neuron, `/upgrade` opens that page, and
`neuron usage` (or `/usage`) shows your plan, today's usage and your credit balance.

## Bring your own key

You can bypass the gateway with a provider account of your own: for example
`OPENAI_API_KEY` (plus `OPENAI_BASE_URL` for any OpenAI-compatible endpoint, including
a local model server), `ANTHROPIC_API_KEY`, `XAI_API_KEY`, Azure AI Foundry or
`OPENROUTER_API_KEY`. The gateway offers two of OpenRouter's free models; your own key
gives you the rest of OpenRouter. While you are signed in to Zero-X the gateway stays
the default; a provider key is used when you select that provider (`--provider`,
`NEURON_PROVIDER` or the `provider` setting) or when you are not signed in. Neuron
prints which provider and credential it uses when it starts. See
[Bring your own key](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md#bring-your-own-key).

## Privacy

What you send in a session (prompts, the contents of files Neuron reads, command
output) is processed by third-party model providers: the upstream provider the
gateway routes to (Groq, Google, Cloudflare, NVIDIA, OpenRouter or a paid upstream),
or the provider whose key you configured. Their terms apply. Free tiers can have
different data-use terms from paid ones; Google, for example, may use Gemini API
free-tier content to improve its products. Paid Zero-X plans also use the free pools
first and fall back to paid capacity, so if your code must stay under one provider's
terms, use your own key for that provider.

The gateway forwards each request to the upstream provider and records usage metadata
(model, token counts, latency, status) for quotas and billing. Neuron sends no
analytics of its own. Tools you allow, such as shell commands, web fetches and MCP
servers, can contact other services.

## Using Neuron

```sh
cd your-project
neuron                                   # interactive session
neuron prompt "explain src/main.rs"      # one prompt, then exit
neuron --resume latest prompt "and the tests?"   # one more prompt in the newest session
neuron --permission-mode read-only       # read and search only
```

Neuron asks you to trust a directory before working in it. The default permission
mode, `workspace-write`, lets it read and edit files in the workspace and run shell
commands that only read; it asks before any other command, before fetching from a new
web domain and before other tools that need full access. The
[usage guide](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md) covers
commands, slash commands, permission modes, models, sessions and configuration.

### Scripts and CI

```sh
neuron --output-format json prompt "run the tests and summarise the failures"
neuron --resume latest prompt "now fix the first failure"
```

Launch flags go before `prompt` (or `-p`). Everything after `-p` is the prompt, so a
flag such as `--output-format` there is refused rather than sent to the model. Without
a terminal on standard input nobody can approve a tool call, so a call that would ask
is denied instead, the model is told it did not run, and Neuron exits with code 2 after
the answer. Each denial names the `permissions.allow` rules that would let that call
run, when any would, and the last warning lists them all.

Allow what your script needs in your user settings, `~/.neuron/settings.json`. A
script never gets the trust question, so the workspace's own settings, allow rules
included, are ignored until you
[trust it](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md#workspace-trust).

```json
{
  "permissions": {
    "defaultMode": "workspace-write",
    "allow": ["bash(npm:*)", "PowerShell(npm:*)"]
  }
}
```

`bash(npm:*)` lets a shell command run when every part of it that needs approval is
an `npm` command (`npm install && npm run build`). Rules count together: with
`bash(cargo:*)` as well, `cargo test && npm run build` runs too. No prefix rule covers
a command that writes a file through a redirection (`npm test > out.txt`), sets an
environment variable first (`CI=true npm test`) or uses a substitution; a rule for the
exact command does (`"bash(npm test > out.txt)"`). `"WebSearch"` allows web searches.
Use `--permission-mode danger-full-access` only in a disposable environment such as a
container: it lets every tool call run.

Each `neuron prompt` run starts a new session; `neuron --resume latest prompt "..."`
sends the newest session another prompt. The JSON answer's `session_id` is what
`--resume` takes, so a script can continue its own session by id. One run at a time can
continue a session: a second one is refused as "in use" rather than overwriting the
first one's turns.

## Documentation

- [Usage guide](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md)
- [Default skills and integrations](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/docs/DEFAULT_INTEGRATIONS.md)
  (bundled skills are listed in the system prompt and loaded on demand)
- [Security policy](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/SECURITY.md)
- [Contributing](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/CONTRIBUTING.md)
- [Release runbook](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/PUBLISHING.md)
- [Changelog](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/CHANGELOG.md)
- [Neuron and Zero-X platform contract](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/docs/GATEWAY_CONTRACT.md)
- [Code provenance](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/docs/PROVENANCE.md)

The parity, roadmap and architecture documents describe the upstream project Neuron
grew out of and may still use its `claw` command names.

## License

MIT; see [LICENSE](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/LICENSE).
Neuron is derived from [Claw Code](https://github.com/ultraworkers/claw-code), also
MIT. The full-screen UI (`ui/neuron-tui`, the `neuron-tui` binaries) is derived from
[Freebuff](https://github.com/CodebuffAI/freebuff) and is under the Apache License 2.0.
[NOTICE](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/NOTICE) records both
lineages and the bundled third-party skills, and every release attaches
`THIRD_PARTY_NOTICES.md` with the licenses of the Rust crates compiled into `neuron`
and of the Bun runtime and npm packages compiled into `neuron-tui`.
Neuron and Zero-X are not affiliated with Anthropic, OpenAI, Google, Groq, Cloudflare
or any other model provider.
