# Neuron React TUI

This is a React/Ink terminal frontend for Neuron. It uses theme tokens adapted from the local `opencode/internal/tui/theme/opencode.go` palette while keeping Rust as the execution engine.

Run locally after installing npm dependencies:

```bash
npm install
npm run tui -- --model claude-opus-4-6 --permission-mode workspace-write
```

Build:

```bash
npm run tui:build
```

The first version is intentionally thin: it provides a polished status shell and command map. Tool execution, sessions, permissions, and providers remain in the Rust CLI.
