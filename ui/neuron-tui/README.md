# neuron-tui

Neuron's full-screen terminal UI: a port of the Codebuff/Freebuff CLI
(Apache-2.0, see `LICENSE` and `NOTICE`), built on OpenTUI and React. The Rust
`neuron` binary stays the engine; this UI talks to it over the JSON-lines
protocol of `neuron engine --stdio` (`src/engine/`).

What was taken out of Freebuff, and why, is in [PORTING.md](PORTING.md).

## Status (P4)

The UI starts `neuron engine --stdio` in the project folder and runs real
turns through it: streamed text, tool calls and results, and permission
prompts. It also covers the Zero-X sign-in (`/login`, with the device code
shown in the UI), the model picker (`/model`) and a status footer, under
Neuron's name and logo.

Releases ship it as `neuron-tui-<target>` beside `neuron-<target>`. On a
terminal, a bare `neuron` starts it (`--no-tui` or `NEURON_NO_TUI=1` keeps the
line REPL). See [PORTING.md](PORTING.md) for the packaging.

It finds the engine through `NEURON_BIN` (which `neuron` sets to itself), then
a `neuron` binary beside the UI, then the PATH it started with. `--model`,
`--provider`, `--permission-mode`, `--allowedTools`, `--reasoning-effort`,
`--allow-broad-cwd` and `--dangerously-skip-permissions` pass through to the
engine.

```bash
bun run build:binary 7.0.0-alpha.4
```

`bun run build:binary <version>` writes `bin/neuron-tui` (`.exe` on Windows)
for this machine.

## Requirements

Bun 1.3.14.

## Commands

Run these from this folder (`ui/neuron-tui`):

```bash
bun install
```

```bash
bun run typecheck
```

```bash
bun run test
```

```bash
bun run dev
```

```bash
bun run dev:mock PARITY_SCENARIO:streaming_text
```

`bun run test` runs every `*.test.ts(x)` under `src/` except
`*.integration.test.*`. `bun run dev` starts the UI in the current folder;
pass `--cwd <dir>` to open another project.

`bun run dev:mock` runs the UI against the real engine and the mock Anthropic
service, with no account or network, in a throwaway home folder. It needs
`cargo build -p rusty-claude-cli -p mock-anthropic-service` in `rust/` first.
Prompts like `PARITY_SCENARIO:bash_permission_prompt_approved` pick a scripted
turn. `bun run dev:mock --signed-out /login` starts signed out of Zero-X
against a local fake gateway instead, to try the sign-in, `/model` and
`/logout`; the fake approves the sign-in at once. The same binaries run
`bun test src/engine/__tests__/engine.integration.test.ts`.

Settings, chat history and logs go to `~/.neuron/tui`, or to `tui` in
`NEURON_HOME` when that is set (override both with an absolute
`NEURON_TUI_CONFIG_DIR`). `NEURON_TUI_ENV=dev` logs to
`<project>/debug/cli.jsonl` instead of the chat folder.

## Update notice

When a newer Neuron release exists, the status bar under the input says so
on the right, with the exact command that installs it:

    Update available! Run: npm install -g @zero-x.live/neuron

Click it to copy the command, or type `/update` for the installed and latest
versions, the release notes link and the command (also copied). Below the
release's minimum supported version it reads `Update required: <command>` in
the error colour. In a narrow terminal it shortens to `Update available ·
/update` before it is dropped.

- **What it checks.** Once per launch, after the UI is drawn, a plain GET of
  the public release manifest `https://zero-x.live/releases/cli/latest.json`
  (3 s timeout, no sign-in, no cookies, a fixed User-Agent that names no user,
  machine or version). Only schema 1 manifests of the CLI are read. The
  command matches how this copy was installed (npm, pip, the PowerShell or the
  shell installer), found from where the UI binary lives; a launcher can state
  it with `NEURON_INSTALL_METHOD` (`npm`, `pip`, `powershell` or `shell`).
  Code: `src/utils/update-check.ts`, `src/state/update-store.ts`.
- **The cache.** The manifest is kept in `update-check.json` in the config
  folder above for 24 hours, so most launches make no request. `/update` reads
  it again. A corrupt or unwritable cache is ignored, and a failed check never
  shows an error in the chat.
- **Turning it off.** `NEURON_NO_UPDATE_CHECK=1` turns it off. It is also off
  with `CI` set, when stdout is not a terminal, and in development builds
  (the source's version `0.0.0`, or any version that is not semver).
  `NEURON_UPDATE_MANIFEST_URL` reads another manifest (https, or http on
  loopback only), for tests.
