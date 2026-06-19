package chat

import (
	"os"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/tui/components/terminal"
)

// terminalPanelCmp is a thin wrapper around the modular terminal.Model,
// adapting it for the Neuron sidebar. It delegates all PTY lifecycle,
// focus management, and keystroke forwarding to the terminal package.
type terminalPanelCmp struct {
	term terminal.Model
}

// NewTerminalPanel creates a new terminal panel rooted at the current
// working directory. Falls back to user home if config isn't loaded yet.
func NewTerminalPanel() *terminalPanelCmp {
	cwd := safeWorkingDir()
	shellPath, shellArgs := safeShellConfig()
	return &terminalPanelCmp{
		term: terminal.New(cwd, shellPath, shellArgs),
	}
}

// safeWorkingDir returns config.WorkingDirectory() with a safe fallback.
func safeWorkingDir() string {
	defer func() { recover() }() // config may panic if not loaded
	if dir := config.WorkingDirectory(); dir != "" {
		return dir
	}
	home, err := os.UserHomeDir()
	if err == nil {
		return home
	}
	return "."
}

func safeShellConfig() (string, []string) {
	defer func() { recover() }()
	cfg := config.Get()
	if cfg == nil {
		return "", nil
	}
	return cfg.Shell.Path, cfg.Shell.Args
}

// Init implements tea.Model.
func (tp *terminalPanelCmp) Init() tea.Cmd {
	return tp.term.Init()
}

// startShell spawns the PTY shell. Called by the sidebar during Init.
func (tp *terminalPanelCmp) startShell() tea.Cmd {
	return tp.term.StartShell()
}

// Update delegates all messages to the inner terminal model.
func (tp *terminalPanelCmp) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	return tp, tp.term.Update(msg)
}

// View renders the terminal panel.
func (tp *terminalPanelCmp) View() string {
	return tp.term.View()
}

// SetSize updates the dimensions of the terminal panel.
func (tp *terminalPanelCmp) SetSize(width, height int) {
	tp.term.SetSize(width, height)
}

// SetFocus sets the keyboard focus state of the terminal.
func (tp *terminalPanelCmp) SetFocus(focused bool) {
	tp.term.SetFocus(focused)
}

// IsFocused returns whether the terminal has keyboard focus.
func (tp *terminalPanelCmp) IsFocused() bool {
	return tp.term.IsFocused()
}

// Close cleans up the PTY process.
func (tp *terminalPanelCmp) Close() {
	tp.term.Close()
}

// WriteCommand writes a command string to the PTY stdin.
func (tp *terminalPanelCmp) WriteCommand(command string) {
	tp.term.WriteCommand(command)
}

// AddOutput sends tool call output to the terminal.
func (tp *terminalPanelCmp) AddOutput(command, output string, isError bool) {
	_ = command
	_ = output
	_ = isError
	// Tool outputs are displayed in the chat message view.
	// The terminal is a live interactive shell, not a log viewer.
}

// Ensure terminalPanelCmp is compatible with the message types used
// by the sidebar for forwarding terminal events.
var (
	_ = terminal.TerminalOutputMsg{}
	_ = terminal.TerminalFocusMsg{}
	_ = terminal.TerminalRefreshMsg{}
	_ = terminal.TerminalClosedMsg{}
	_ = message.Message{}
)
