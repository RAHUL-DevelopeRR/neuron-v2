package tui

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/opencode-ai/opencode/internal/app"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

// AppReadyMsg is sent when background initialization completes.
type AppReadyMsg struct {
	App *app.App
}

// InitErrorMsg is sent when background initialization fails.
type InitErrorMsg struct {
	Err error
}

// InitProgressMsg is sent to update the splash screen with progress.
type InitProgressMsg struct {
	Step    int
	Total   int
	Message string
}

// lazyModel shows a loading screen immediately, then transitions to the
// full TUI when the app is ready.
type lazyModel struct {
	width, height int
	cwd           string
	debug         bool
	ready         bool
	initErr       error
	realModel     tea.Model
	frame         int
	createdAt     time.Time

	progressStep    int
	progressTotal   int
	progressMessage string
}

func NewLazy(cwd string, debug bool) tea.Model {
	return &lazyModel{
		cwd:             cwd,
		debug:           debug,
		createdAt:       time.Now(),
		progressMessage: "Starting up...",
		progressTotal:   5,
	}
}

func (m *lazyModel) Init() tea.Cmd {
	return m.tick()
}

type tickMsg struct{}

func (m *lazyModel) tick() tea.Cmd {
	return tea.Tick(
		80*time.Millisecond,
		func(t time.Time) tea.Msg {
			return tickMsg{}
		},
	)
}

func (m *lazyModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	if m.ready && m.realModel != nil {
		updated, cmd := m.realModel.Update(msg)
		m.realModel = updated
		return m, cmd
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		if m.realModel != nil {
			updated, cmd := m.realModel.Update(msg)
			m.realModel = updated
			return m, cmd
		}

	case tickMsg:
		m.frame++
		return m, m.tick()

	case InitProgressMsg:
		m.progressStep = msg.Step
		m.progressTotal = msg.Total
		m.progressMessage = msg.Message
		return m, nil

	case AppReadyMsg:
		logging.Info("Splash to TUI transition", "elapsed_ms", time.Since(m.createdAt).Milliseconds())
		m.realModel = New(msg.App)
		m.ready = true

		var cmds []tea.Cmd
		cmds = append(cmds, m.realModel.Init())
		if m.width > 0 && m.height > 0 {
			sizeMsg := tea.WindowSizeMsg{Width: m.width, Height: m.height}
			updated, cmd := m.realModel.Update(sizeMsg)
			m.realModel = updated
			cmds = append(cmds, cmd)
		}
		return m, tea.Batch(cmds...)

	case InitErrorMsg:
		m.initErr = msg.Err
		return m, nil

	case tea.KeyMsg:
		if msg.String() == "ctrl+c" || msg.String() == "q" {
			return m, tea.Quit
		}
	}

	return m, nil
}

func (m *lazyModel) View() string {
	if m.ready && m.realModel != nil {
		return m.realModel.View()
	}

	if m.initErr != nil {
		return lipgloss.Place(
			m.width, m.height,
			lipgloss.Center, lipgloss.Center,
			lipgloss.NewStyle().
				Foreground(lipgloss.Color("#FF5555")).
				Bold(true).
				Render(fmt.Sprintf("Initialization failed: %v\n\nPress q to quit.", m.initErr)),
		)
	}

	t := theme.CurrentTheme()
	spinnerFrames := []string{"|", "/", "-", "\\"}
	spinner := spinnerFrames[m.frame%len(spinnerFrames)]

	barWidth := 30
	progress := 0.0
	if m.progressTotal > 0 {
		progress = float64(m.progressStep) / float64(m.progressTotal)
	}
	filled := int(progress * float64(barWidth))
	if filled > barWidth {
		filled = barWidth
	}

	bar := strings.Repeat("#", filled) + strings.Repeat("-", barWidth-filled)
	progressBar := lipgloss.NewStyle().
		Foreground(t.Primary()).
		Render(" [" + bar + "]")

	pctText := fmt.Sprintf(" %d%%", int(progress*100))
	stepInfo := lipgloss.NewStyle().
		Foreground(t.TextMuted()).
		Render(fmt.Sprintf(" [%d/%d]", m.progressStep, m.progressTotal))

	statusMsg := lipgloss.NewStyle().
		Foreground(t.Text()).
		Render(fmt.Sprintf(" %s  %s", spinner, m.progressMessage))

	elapsedStr := lipgloss.NewStyle().
		Foreground(t.TextMuted()).
		Render(fmt.Sprintf(" %.1fs elapsed", time.Since(m.createdAt).Seconds()))

	content := lipgloss.JoinVertical(
		lipgloss.Center,
		splashLogo(),
		"",
		statusMsg,
		"",
		progressBar+pctText+stepInfo,
		elapsedStr,
	)

	return lipgloss.Place(
		m.width, m.height,
		lipgloss.Center, lipgloss.Center,
		content,
	)
}

func splashLogo() string {
	t := theme.CurrentTheme()
	colors := []lipgloss.TerminalColor{
		t.Primary(),
		t.Secondary(),
		t.Accent(),
		t.Success(),
		t.Warning(),
		t.Info(),
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
