// Package terminal provides a reusable, embeddable PTY terminal panel
// for Bubbletea TUI applications. It manages a pseudo-terminal with a
// virtual terminal emulator, supporting focus-based keyboard forwarding,
// live output rendering, and shell lifecycle management.
package terminal

// TerminalOutputMsg is sent when the PTY produces output bytes.
// The parent model should pass this back into Update to feed the emulator.
// Gen identifies the shell generation so stale output from a previous
// PTY instance is safely discarded.
type TerminalOutputMsg struct {
	Data []byte
	Gen  uint64
}

// TerminalFocusMsg toggles focus on/off for the terminal.
// When focused, the terminal captures all keyboard input and forwards
// it to the PTY stdin.
type TerminalFocusMsg struct {
	Focused bool
}

// TerminalRefreshMsg triggers the terminal to restart the shell process.
// The old PTY is closed and a new one is spawned with a fresh emulator.
type TerminalRefreshMsg struct{}

// TerminalClosedMsg is sent when the PTY shell process exits.
// Err is nil on a clean exit. Gen identifies the shell generation
// so stale close messages from a previous PTY are safely discarded.
type TerminalClosedMsg struct {
	Err error
	Gen uint64
}
