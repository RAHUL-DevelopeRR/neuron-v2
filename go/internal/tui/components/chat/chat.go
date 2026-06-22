package chat

import (
	"fmt"
	"sort"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/session"
	"github.com/opencode-ai/opencode/internal/tui/styles"
	"github.com/opencode-ai/opencode/internal/tui/theme"
	"github.com/opencode-ai/opencode/internal/version"
)

type SendMsg struct {
	Text        string
	Attachments []message.Attachment
}

type SessionSelectedMsg = session.Session

type SessionClearedMsg struct{}

type EditorFocusMsg bool

type PaneFocusMsg struct {
	Pane string
}

const (
	PaneChat     = "chat"
	PaneTerminal = "terminal"
)

// logo renders the original Neuron block banner.
// Keep the glyphs as real UTF-8 block characters; the earlier broken header
// came from mojibake literals, not from the brand mark itself.
func logo(width int) string {
	if width < 10 {
		return ""
	}

	t := theme.CurrentTheme()
	colors := []lipgloss.TerminalColor{
		t.Primary(),
		t.Secondary(),
		t.Accent(),
		t.Success(),
		t.Warning(),
		t.Info(),
	}

	if width < 34 {
		var compact strings.Builder
		for i, r := range "NEURON" {
			style := lipgloss.NewStyle().Foreground(colors[i]).Bold(true)
			compact.WriteString(style.Render(string(r)))
		}
		return compact.String()
	}

	letterSlices := []struct{ col0, col1, col2 string }{
		{" █▄ █", " █ ██", " █  █"},
		{" ████", " █▄▄ ", " ████"},
		{" █  █", " █  █", " ▀██▀"},
		{" ███▄", " █▀▀▄", " █  █"},
		{" ▄██▄", " █  █", " ▀██▀"},
		{" █▄ █", " █ ██", " █  █"},
	}

	var rows [3]string
	for i, letter := range letterSlices {
		style := lipgloss.NewStyle().Foreground(colors[i]).Bold(true)
		rows[0] += style.Render(letter.col0)
		rows[1] += style.Render(letter.col1)
		rows[2] += style.Render(letter.col2)
	}

	return rows[0] + "\n" + rows[1] + "\n" + rows[2]
}

func repo(width int) string {
	t := theme.CurrentTheme()
	gatewayInfo := fmt.Sprintf("NeuronCLI %s  |  zero-x.live gateway", version.Version)

	return styles.BaseStyle().
		Foreground(t.TextMuted()).
		Width(width).
		Align(lipgloss.Center).
		Render(gatewayInfo)
}

func cwd(width int) string {
	cwdStr := fmt.Sprintf("cwd: %s", config.WorkingDirectory())
	cwdStr = ansi.Truncate(cwdStr, width, "...")
	t := theme.CurrentTheme()

	return styles.BaseStyle().
		Foreground(t.TextMuted()).
		Width(width).
		Render(cwdStr)
}

func lspsConfigured(width int) string {
	cfg := config.Get()
	title := "LSP Configuration"
	title = ansi.Truncate(title, width, "...")

	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	lsps := baseStyle.
		Width(width).
		Foreground(t.Primary()).
		Bold(true).
		Render(title)

	var lspNames []string
	for name := range cfg.LSP {
		lspNames = append(lspNames, name)
	}
	sort.Strings(lspNames)
	if len(lspNames) == 0 {
		return ""
	}

	var lspViews []string
	for _, name := range lspNames {
		lsp := cfg.LSP[name]
		lspName := baseStyle.
			Foreground(t.Text()).
			Render(fmt.Sprintf("  %s", name))

		cmd := lsp.Command
		cmd = ansi.Truncate(cmd, width-lipgloss.Width(lspName)-3, "...")

		lspPath := baseStyle.
			Foreground(t.TextMuted()).
			Render(fmt.Sprintf(" (%s)", cmd))

		lspViews = append(lspViews,
			baseStyle.
				Width(width).
				Render(
					lipgloss.JoinHorizontal(
						lipgloss.Left,
						lspName,
						lspPath,
					),
				),
		)
	}

	return baseStyle.
		Width(width).
		Render(
			lipgloss.JoinVertical(
				lipgloss.Left,
				lsps,
				lipgloss.JoinVertical(
					lipgloss.Left,
					lspViews...,
				),
			),
		)
}
