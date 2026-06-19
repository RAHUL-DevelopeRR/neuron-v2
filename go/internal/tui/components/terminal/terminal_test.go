package terminal

import (
	"fmt"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestShellCommandUsesConfiguredShell(t *testing.T) {
	path, args := shellCommand("custom-shell", []string{"--login", "--flag"})
	if path != "custom-shell" {
		t.Fatalf("path = %q, want custom-shell", path)
	}
	if !reflect.DeepEqual(args, []string{"--login", "--flag"}) {
		t.Fatalf("args = %#v", args)
	}
}

func TestDefaultShellForCurrentOS(t *testing.T) {
	path, _ := shellCommand("", nil)
	if path == "" {
		t.Fatal("default shell path is empty")
	}
	if runtime.GOOS != "windows" && path == "powershell.exe" {
		t.Fatalf("non-Windows default shell should not be powershell.exe")
	}
}

func TestNewPTYRunsConfiguredShell(t *testing.T) {
	shell, args := echoShellCommand()
	pty, err := NewPTY(80, 12, t.TempDir(), shell, args)
	if err != nil {
		t.Fatalf("NewPTY() error = %v", err)
	}
	defer pty.Close()

	outCh := make(chan string, 1)
	errCh := make(chan error, 1)
	go func() {
		var out strings.Builder
		buf := make([]byte, 1024)
		for {
			n, err := pty.Read(buf)
			if n > 0 {
				out.Write(buf[:n])
				if strings.Contains(out.String(), "NEURON_PTY_OK") {
					outCh <- out.String()
					return
				}
			}
			if err != nil {
				errCh <- err
				return
			}
		}
	}()

	select {
	case out := <-outCh:
		if !strings.Contains(out, "NEURON_PTY_OK") {
			t.Fatalf("output = %q, want marker", out)
		}
	case err := <-errCh:
		t.Fatalf("PTY read error before marker: %v", err)
	case <-time.After(5 * time.Second):
		pty.Close()
		t.Fatal("timed out waiting for PTY shell output")
	}
}

func TestRenderScreenSupportsScrollbackOffset(t *testing.T) {
	m := New(t.TempDir(), "", nil)
	m.SetSize(24, 4)
	for i := 1; i <= 12; i++ {
		_, _ = m.emulator.WriteString(fmt.Sprintf("line-%02d\r\n", i))
	}

	bottom := m.renderScreen(3)
	m.ScrollUp(4)
	if m.scrollOffset == 0 {
		t.Fatal("ScrollUp did not move terminal away from the live bottom")
	}
	scrolled := m.renderScreen(3)
	if scrolled == bottom {
		t.Fatalf("scrolled view did not change\nbottom:\n%s\nscrolled:\n%s", bottom, scrolled)
	}

	m.ScrollDown(100)
	if m.scrollOffset != 0 {
		t.Fatalf("ScrollDown did not return to live bottom, offset=%d", m.scrollOffset)
	}
}

func echoShellCommand() (string, []string) {
	if runtime.GOOS == "windows" {
		return "cmd.exe", []string{"/d", "/c", "echo NEURON_PTY_OK"}
	}
	return "/bin/sh", []string{"-lc", "echo NEURON_PTY_OK"}
}
