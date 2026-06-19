package terminal

import (
	"os"
	"os/exec"
	"runtime"
	"strings"

	"github.com/charmbracelet/x/xpty"
)

// PTY wraps a pseudo-terminal with its associated shell process.
// It provides read, write, resize, and close operations for the
// underlying PTY device.
type PTY struct {
	pty xpty.Pty
	cmd *exec.Cmd
}

// NewPTY creates a new pseudo-terminal with the given dimensions and
// working directory. It starts the configured shell, falling back to an
// OS-appropriate default when no shell is configured.
func NewPTY(cols, rows int, cwd, shell string, args []string) (*PTY, error) {
	p, err := xpty.NewPty(cols, rows)
	if err != nil {
		return nil, err
	}

	shell, args = shellCommand(shell, args)
	cmd := exec.Command(shell, args...)
	cmd.Dir = cwd
	cmd.Env = append(os.Environ(), "TERM=xterm-256color", "COLORTERM=truecolor")

	if err := p.Start(cmd); err != nil {
		p.Close()
		return nil, err
	}

	return &PTY{
		pty: p,
		cmd: cmd,
	}, nil
}

// Read reads bytes from the PTY output (i.e. the shell's stdout/stderr).
func (p *PTY) Read(buf []byte) (int, error) {
	return p.pty.Read(buf)
}

// Write writes bytes to the PTY input (i.e. the shell's stdin).
func (p *PTY) Write(data []byte) (int, error) {
	return p.pty.Write(data)
}

// Resize changes the dimensions of the PTY.
func (p *PTY) Resize(cols, rows int) error {
	return p.pty.Resize(cols, rows)
}

// Close closes the PTY and kills the shell process if still running.
func (p *PTY) Close() error {
	// Kill the process first to avoid orphans
	if p.cmd != nil && p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
	return p.pty.Close()
}

func shellCommand(shell string, args []string) (string, []string) {
	shell = strings.TrimSpace(shell)
	if shell != "" {
		return shell, append([]string(nil), args...)
	}
	return defaultShell()
}

// defaultShell returns the OS-appropriate default shell command.
func defaultShell() (string, []string) {
	if runtime.GOOS == "windows" {
		if ps, err := exec.LookPath("pwsh.exe"); err == nil {
			return ps, []string{"-NoLogo"}
		}
		if ps, err := exec.LookPath("powershell.exe"); err == nil {
			return ps, []string{"-NoLogo"}
		}
		if comspec := os.Getenv("COMSPEC"); comspec != "" {
			return comspec, nil
		}
		return "cmd.exe", nil
	}

	if shell := os.Getenv("SHELL"); shell != "" {
		return shell, []string{"-l"}
	}
	return "/bin/sh", nil
}
