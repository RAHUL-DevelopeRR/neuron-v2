# Neuron Desktop

An IDE-like workspace on the Neuron engine, built with Tauri 2 and React 19. The agent runs in
the engine (`neuron engine --stdio`). This app only shows the session and steers it:

- a chat with streamed replies, reasoning and tool cards
- the permission dialog: y allows once, a allows for the session, n denies, e denies with a note
- diffs for edits and writes
- a file tree and a side panel with the files the session changed
- a terminal pane, the model picker, sign-in (Zero-X device code) and the status bar

The colours are the NeuronCLI palette, from `ui/neuron-client/src/theme.ts`.

## Two ways to run it

**In Tauri.** `src-tauri` starts the `neuron` binary as a child process and passes its JSON lines
to the window. It also reads the workspace for the file tree, read-only and never outside it, and
runs terminal panes with portable-pty. The workspace is `NEURON_WORKSPACE`, the first argument, or
the folder the app started in. The binary is `NEURON_BIN`, one beside the app, or the one on the
PATH.

```bash
bun run tauri dev
```

**In a browser,** against the real engine and the mock Anthropic service. There is no account and
no network.

```bash
bun run dev:mock
```

Then open http://127.0.0.1:1420. The page attaches to a Neuron host (`ui/neuron-client`) over
WebSocket. The pairing token goes through a temp file that Vite reads, and is never printed. Try
prompts such as `PARITY_SCENARIO:bash_permission_prompt_approved` or
`PARITY_SCENARIO:write_file_allowed`, and send `/clear --confirm` between scenarios.

Both need the engine and the mock service built: `cargo build -p rusty-claude-cli -p
mock-anthropic-service` in `rust/`.

## Tests

```bash
bun test
```

This runs the components against a fake engine, and the whole app against the real engine on the
mock service through the host (`test/engine.e2e.test.tsx`).

```bash
bun run typecheck
```

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

That runs the native side: the engine bridge on the mock service, workspace bounds and a PTY
shell.

`src-tauri` is a Cargo project of its own, so the engine's workspace in `rust/` never builds
Tauri. Its dev profile has no debuginfo, which keeps `target/` near 1 GB instead of several.

## Installing an unsigned build

Until the signing phase, v1 installers are unsigned. Each OS warns on the first run:

- **Windows.** SmartScreen shows "Windows protected your PC": choose More info, then Run anyway.
  Defender may also quarantine the bundled engine, `neuron.exe`, as a false positive. The app then
  cannot start its engine: restore the file from Windows Security, Protection history, or reinstall
  after allowing it.
- **macOS.** Gatekeeper says the app "cannot be opened because the developer cannot be verified".
  Open System Settings, Privacy & Security, scroll down and choose Open Anyway. Or remove the
  quarantine flag: `xattr -dr com.apple.quarantine /Applications/Neuron.app`. (macOS 15 removed
  the right-click, Open shortcut.)
- **Linux.** The AppImage must be executable: `chmod +x Neuron_*.AppImage`. The `.deb` and `.rpm`
  install normally with the package manager.
