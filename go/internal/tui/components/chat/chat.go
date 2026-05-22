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

func header(width int) string {
	return lipgloss.JoinVertical(
		lipgloss.Top,
		logo(width),
		repo(width),
		"",
		cwd(width),
	)
}

func lspsConfigured(width int) string {
	cfg := config.Get()
	title := "LSP Configuration"
	title = ansi.Truncate(title, width, "…")

	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	lsps := baseStyle.
		Width(width).
		Foreground(t.Primary()).
		Bold(true).
		Render(title)

	// Get LSP names and sort them for consistent ordering
	var lspNames []string
	for name := range cfg.LSP {
		lspNames = append(lspNames, name)
	}
	sort.Strings(lspNames)

	var lspViews []string
	for _, name := range lspNames {
		lsp := cfg.LSP[name]
		lspName := baseStyle.
			Foreground(t.Text()).
			Render(fmt.Sprintf("• %s", name))

		cmd := lsp.Command
		cmd = ansi.Truncate(cmd, width-lipgloss.Width(lspName)-3, "…")

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

// neuronBannerMultiColor renders NEURON in colored block characters using raw ANSI codes.
// Bypasses lipgloss width calculation entirely because █ (U+2588) has ambiguous cell width
// that lipgloss miscalculates, causing garbled rendering in Windows Terminal.
// Colors: N=blue(#4169C3) E=red(#C83228) U=orange(#F0A028) R=orange(#F0A028) O=green(#2D8C3C) N=green(#2D8C3C)
func neuronBannerMultiColor() string {
	// Raw ANSI color codes (same as Rust brand.rs)
	b := "\x1b[1;38;2;65;105;195m"  // blue bold
	r := "\x1b[1;38;2;200;50;40m"   // red bold
	o := "\x1b[1;38;2;240;160;40m"  // orange bold
	g := "\x1b[1;38;2;45;140;60m"   // green bold
	x := "\x1b[0m"                   // reset

	// Each row is hand-built with per-letter colors embedded directly.
	// Letters: N(blue) E(red) U(orange) R(orange) O(green) N(green)
	// Using ▀▄ half-blocks for compact 3-row banner that avoids width issues.
	rows := []string{
		b + "█▄ █" + x + " " + r + "████" + x + " " + o + "█  █" + x + " " + o + "███▄" + x + " " + g + "▄██▄" + x + " " + g + "█▄ █" + x,
		b + "█ ██" + x + " " + r + "█▄▄ " + x + " " + o + "█  █" + x + " " + o + "█▀▀▄" + x + " " + g + "█  █" + x + " " + g + "█ ██" + x,
		b + "█  █" + x + " " + r + "████" + x + " " + o + "▀██▀" + x + " " + o + "█  █" + x + " " + g + "▀██▀" + x + " " + g + "█  █" + x,
	}

	return strings.Join(rows, "\n")
}

func logo(width int) string {
	banner := neuronBannerMultiColor()
	// Don't use lipgloss Width/Align for the banner — it miscalculates █/▀/▄ widths.
	// Just return the raw ANSI banner as-is; the parent layout handles positioning.
	return banner
}

func repo(width int) string {
	t := theme.CurrentTheme()
	gatewayInfo := fmt.Sprintf("%s NeuronCLI %s  •  zero-x.live gateway", styles.NeuronIcon, version.Version)

	return styles.BaseStyle().
		Foreground(t.TextMuted()).
		Width(width).
		Align(lipgloss.Center).
		Render(gatewayInfo)
}

func cwd(width int) string {
	cwd := fmt.Sprintf("cwd: %s", config.WorkingDirectory())
	t := theme.CurrentTheme()

	return styles.BaseStyle().
		Foreground(t.TextMuted()).
		Width(width).
		Render(cwd)
}
