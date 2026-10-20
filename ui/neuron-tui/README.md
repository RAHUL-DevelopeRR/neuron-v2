# neuron-tui

Neuron's full-screen terminal UI: a port of the Codebuff/Freebuff CLI
(Apache-2.0, see `LICENSE` and `NOTICE`), built on OpenTUI and React. The Rust
`neuron` binary stays the engine; this UI talks to it over the JSON-lines
protocol of `neuron engine --stdio` (`src/engine/`).

What was taken out of Freebuff, and why, is in [PORTING.md](PORTING.md).

## Status (P1)

The UI builds, typechecks, passes its tests and starts to the chat screen. It
is **not connected to the engine yet**: `src/engine/neuron-engine-client.ts`
is a typed stub, so a sent message ends with "The Neuron engine is not
connected yet". The bridge is P2.

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

`bun run test` runs every `*.test.ts(x)` under `src/` except
`*.integration.test.*`. `bun run dev` starts the UI in the current folder;
pass `--cwd <dir>` to open another project.

Settings, chat history and logs go to `~/.neuron/tui` (override with an
absolute `FREEBUFF_CONFIG_DIR`). `NEURON_TUI_ENV=dev` logs to
`<project>/debug/cli.jsonl` instead of the chat folder.
