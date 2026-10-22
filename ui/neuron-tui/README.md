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
bun run build:binary 6.3.0
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
