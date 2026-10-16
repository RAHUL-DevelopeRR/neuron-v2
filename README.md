# Neuron

Neuron is a coding agent that runs in your terminal. It reads your project, edits
files and runs commands, and asks before it does anything your permission mode does
not allow. It is a single native binary written in Rust.

By default Neuron talks to models through the [Zero-X gateway](https://zero-x.live).
You sign in once with `neuron auth login`; the provider API keys stay on the gateway
and never reach your machine. You can also use your own provider key instead.

> 6.3.0 is the first version built and published by this repository's release
> pipeline. The npm 6.2.0 and PyPI 6.2.4 (and earlier) packages were built outside it
> and are deprecated. Install 6.3.0 or later.

## Install

Every method installs the same `neuron` binary from
[GitHub Releases](https://github.com/RAHUL-DevelopeRR/neuron-v2/releases) and checks
its SHA-256 checksum before using it.

**macOS and Linux**

```sh
curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.sh | sh
```

The script installs to `~/.local/bin` and tells you if that directory is not on your
`PATH`. Pass options after `sh -s --`, for example
`sh -s -- --version 6.3.0 --install-dir "$HOME/bin"`, or set `NEURON_VERSION` and
`NEURON_INSTALL_DIR`.

**Windows (PowerShell 5.1 or 7)**

```powershell
irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1 | iex
```

The script installs to `%LOCALAPPDATA%\Programs\Neuron\bin` and only changes your
`PATH` if you ask it to:

```powershell
& ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1))) -AddToPath
```

**npm** (Node.js 22.14 or later)

```sh
npm install --global @zero-x.live/neuron
```

**pip**

```sh
pipx install neuroncli        # or: python -m pip install neuroncli
```

Uploads to PyPI are paused until access to the PyPI project is restored (see
[PUBLISHING.md](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/PUBLISHING.md)).
Until a 6.3.0 wheel is on PyPI, use one of the other methods.

The npm and pip packages are small launchers. The first run downloads the binary for
the package's exact version, verifies it and caches it; later runs start without
network access. Ctrl+C goes to Neuron, and the launcher exits with Neuron's exit code.

**From source**

```sh
git clone https://github.com/RAHUL-DevelopeRR/neuron-v2.git
cd neuron-v2/rust
cargo build --release -p rusty-claude-cli
./target/release/neuron --version
```

### Supported platforms

| Platform | Release asset |
| --- | --- |
| Linux x64 (glibc) | `neuron-linux-x64` |
| Linux arm64 (glibc) | `neuron-linux-arm64` |
| macOS, Apple silicon | `neuron-macos-arm64` |
| macOS, Intel | `neuron-macos-x64` |
| Windows x64 | `neuron-windows-x64.exe` (Windows on Arm runs it under emulation) |

musl-based Linux such as Alpine is not supported. The binaries are not code-signed or
notarized. Each release lists SHA-256 checksums (`SHA256SUMS`, plus a `.sha256` file
per binary) and GitHub build provenance, which you can check with
`gh attestation verify neuron-linux-x64 --repo RAHUL-DevelopeRR/neuron-v2`.

## Sign in

```sh
neuron auth login     # opens zero-x.live in your browser
neuron auth status
neuron auth logout    # revokes the session on the gateway and deletes the local copy
```

Login waits for the browser callback on a random `127.0.0.1` port and stores the
session, encrypted, under `~/.neuroncli/`. On CI and machines without a browser,
create a personal API key (`nzx_…`) in the Zero-X dashboard and pass it with
`neuron auth login --with-token` or the `NEURON_TOKEN` environment variable. See
[Sign in](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md#sign-in)
in the usage guide.

## Plans and limits

**Free.** No card needed. Free-plan requests run on free tiers of upstream providers
whose terms allow serving an application's users: Groq, the Gemini API free tier and
Cloudflare Workers AI. The limits are real and you will run into them:

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
  time. `/model` lists what is available to you now.

**Pro and Max.** Paid plans raise the daily and per-minute limits and add paid
upstream capacity, billed against monthly credits included with the plan; credit
top-ups are also sold. Prices and current limits are at
<https://zero-x.live/pricing>. In Neuron, `/upgrade` opens that page, and
`neuron usage` (or `/usage`) shows your plan, today's usage and your credit balance.

## Bring your own key

You can bypass the gateway with a provider account of your own: for example
`OPENAI_API_KEY` (plus `OPENAI_BASE_URL` for any OpenAI-compatible endpoint, including
a local model server), `ANTHROPIC_API_KEY`, `XAI_API_KEY`, `DASHSCOPE_API_KEY` or
`OPENROUTER_API_KEY`. OpenRouter is available only this way. While you are signed in
to Zero-X the gateway stays the default; a provider key is used when you select that
provider or when you are not signed in. See
[Bring your own key](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md#bring-your-own-key).

## Privacy

What you send in a session (prompts, the contents of files Neuron reads, command
output) is processed by third-party model providers: the upstream provider the
gateway routes to (Groq, Google, Cloudflare or a paid upstream), or the provider whose
key you configured. Their terms apply. Free tiers can have different data-use terms
from paid ones; Google, for example, may use Gemini API free-tier content to improve
its products. Paid Zero-X plans also use the free pools first and fall back to paid
capacity, so if your code must stay under one provider's terms, use your own key for
that provider.

The gateway forwards each request to the upstream provider and records usage metadata
(model, token counts, latency, status) for quotas and billing. Neuron sends no
analytics of its own. Tools you allow, such as shell commands, web fetches and MCP
servers, can contact other services.

## Using Neuron

```sh
cd your-project
neuron                                   # interactive session
neuron prompt "explain src/main.rs"      # one prompt, then exit
neuron --permission-mode read-only       # read and search only
```

Neuron asks you to trust a directory before working in it. The default permission
mode, `workspace-write`, lets it read and edit files and asks before it runs shell
commands or other tools that need full access. The
[usage guide](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md) covers
commands, slash commands, permission modes, models, sessions and configuration.

## Documentation

- [Usage guide](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/USAGE.md)
- [Default skills and integrations](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/docs/DEFAULT_INTEGRATIONS.md)
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
MIT. [NOTICE](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/NOTICE) records
that lineage and the bundled third-party skills, and every release attaches
`THIRD_PARTY_NOTICES.md` with the licenses of the Rust crates compiled into the binary.
Neuron and Zero-X are not affiliated with Anthropic, OpenAI, Google, Groq, Cloudflare
or any other model provider.
