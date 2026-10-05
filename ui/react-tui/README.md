# Terminal launcher

`npm run tui` starts the Rust Neuron terminal runtime with inherited input/output. All prompt, tool, permission, session and model behavior is handled by that runtime.

Build the Rust binary first (`cargo build --release -p rusty-claude-cli` in `rust/`) or set `NEURON_BINARY` to its path. CLI arguments are forwarded unchanged. `npm run tui:build` compiles the launcher. The earlier React/Ink prototype remains in `src/App.tsx` as reference; it is not the active runtime interface.
