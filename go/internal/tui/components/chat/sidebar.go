package chat

import (
	"context"
	"fmt"
	"sort"
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/diff"
	"github.com/opencode-ai/opencode/internal/history"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/pubsub"
	"github.com/opencode-ai/opencode/internal/session"
	"github.com/opencode-ai/opencode/internal/tui/components/terminal"
	"github.com/opencode-ai/opencode/internal/tui/styles"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

type sidebarCmp struct {
	width, height int
	session       session.Session
	history       history.Service
	diffRatio     float64
	modFiles      map[string]struct {
		additions int
		removals  int
	}

	// New panels
	diffPanel     *diffPanelCmp
	terminalPanel *terminalPanelCmp
}

type sidebarKeyMap struct {
	DiffTaller     key.Binding
	TerminalTaller key.Binding
}

var sidebarKeys = sidebarKeyMap{
	DiffTaller: key.NewBinding(
		key.WithKeys("ctrl+up"),
		key.WithHelp("ctrl+up", "taller diff"),
	),
	TerminalTaller: key.NewBinding(
		key.WithKeys("ctrl+down"),
		key.WithHelp("ctrl+down", "taller terminal"),
	),
}

func (m *sidebarCmp) Init() tea.Cmd {
	var cmds []tea.Cmd

	// History subscription only makes sense when a session exists
	if m.history != nil && m.session.ID != "" {
		ctx := context.Background()
		filesCh := m.history.Subscribe(ctx)

		m.modFiles = make(map[string]struct {
			additions int
			removals  int
		})

		m.loadModifiedFiles(ctx)

		if m.diffPanel != nil {
			m.diffPanel.Init()
		}

		cmds = append(cmds, func() tea.Msg {
			return <-filesCh
		})
	} else {
		// Initialize modFiles map even without session
		m.modFiles = make(map[string]struct {
			additions int
			removals  int
		})
		if m.diffPanel != nil {
			m.diffPanel.Init()
		}
	}

	return tea.Batch(cmds...)
}

func (m *sidebarCmp) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case SessionSelectedMsg:
		wasEmpty := m.session.ID == ""
		if msg.ID != m.session.ID {
			m.session = msg
			ctx := context.Background()
			m.loadModifiedFiles(ctx)
			if m.diffPanel != nil {
				m.diffPanel.sessionID = msg.ID
				m.diffPanel.rebuildDiff(ctx)
			}
			// Start history subscription on first session assignment
			if wasEmpty && m.history != nil {
				filesCh := m.history.Subscribe(ctx)
				return m, func() tea.Msg {
					return <-filesCh
				}
			}
		}
		if m.terminalPanel != nil {
			m.terminalPanel.Update(msg)
		}
	case pubsub.Event[session.Session]:
		if msg.Type == pubsub.UpdatedEvent {
			if m.session.ID == msg.Payload.ID {
				m.session = msg.Payload
			}
		}
	case pubsub.Event[history.File]:
		if msg.Payload.SessionID == m.session.ID {
			ctx := context.Background()
			m.processFileChanges(ctx, msg.Payload)

			if m.diffPanel != nil {
				m.diffPanel.rebuildDiff(ctx)
			}

			return m, func() tea.Msg {
				ctx := context.Background()
				filesCh := m.history.Subscribe(ctx)
				return <-filesCh
			}
		}
	case terminal.TerminalOutputMsg:
		if m.terminalPanel != nil {
			_, cmd := m.terminalPanel.Update(msg)
			return m, cmd
		}
	case terminal.TerminalClosedMsg:
		if m.terminalPanel != nil {
			_, cmd := m.terminalPanel.Update(msg)
			return m, cmd
		}
	case terminal.TerminalFocusMsg:
		if m.terminalPanel != nil {
			_, cmd := m.terminalPanel.Update(msg)
			return m, cmd
		}
	case terminal.TerminalRefreshMsg:
		if m.terminalPanel != nil {
			_, cmd := m.terminalPanel.Update(msg)
			return m, cmd
		}
	case tea.MouseMsg:
		switch msg.Type {
		case tea.MouseWheelUp, tea.MouseWheelDown:
			if m.mouseInTerminal(msg.Y) && m.terminalPanel != nil {
				_, cmd := m.terminalPanel.Update(msg)
				return m, cmd
			}
			if m.diffPanel != nil {
				if msg.Type == tea.MouseWheelUp {
					m.diffPanel.ScrollUp(3)
				} else {
					m.diffPanel.ScrollDown(3)
				}
			}
		}
	case tea.KeyMsg:
		if key.Matches(msg, sidebarKeys.DiffTaller) {
			m.resizeDiff(0.05)
			return m, nil
		}
		if key.Matches(msg, sidebarKeys.TerminalTaller) {
			m.resizeDiff(-0.05)
			return m, nil
		}
		// When the terminal is focused, forward all keys to it
		if m.terminalPanel != nil && m.terminalPanel.IsFocused() {
			// Intercept [R] for refresh when terminal is focused but not typing
			_, cmd := m.terminalPanel.Update(msg)
			return m, cmd
		}
		if m.diffPanel != nil {
			_, cmd := m.diffPanel.Update(msg)
			return m, cmd
		}
	case pubsub.Event[message.Message]:
		// Extract bash tool results and forward to terminal panel
		if m.terminalPanel != nil && msg.Payload.Role == message.User {
			for _, result := range msg.Payload.ToolResults() {
				if result.Content != "" {
					m.terminalPanel.AddOutput("", result.Content, result.IsError)
				}
			}
		}
	}
	return m, nil
}

func (m *sidebarCmp) CapturesKeys() bool {
	return m.terminalPanel != nil && m.terminalPanel.IsFocused()
}

func (m *sidebarCmp) View() string {
	baseStyle := styles.BaseStyle()
	t := theme.CurrentTheme()

	contentWidth := max(1, m.width-6) // padding left 4 + right 2

	// ── Section 1: Banner (Center alignment — same as splash screen) ──
	bannerLogo := logo(contentWidth)
	if m.height < 18 {
		bannerLogo = logo(min(contentWidth, 33))
	}
	bannerSection := lipgloss.JoinVertical(
		lipgloss.Center,
		bannerLogo,
		repo(contentWidth),
		"",
		cwd(contentWidth),
	)
	bannerHeight := lipgloss.Height(bannerSection)

	// ── Horizontal divider ──
	divider := baseStyle.
		Width(contentWidth).
		Foreground(t.TextMuted()).
		Render(strings.Repeat("─", contentWidth))
	dividerHeight := 1

	// ── Remaining height for panels ──
	remainingHeight := m.height - bannerHeight - (dividerHeight * 2) - 1
	if remainingHeight < 4 {
		remainingHeight = 4
	}

	diffHeight, termHeight := m.panelHeights(remainingHeight)

	// ── Section 2: Diff View ──
	var diffView string
	if m.diffPanel != nil {
		m.diffPanel.SetSize(contentWidth, diffHeight)
		diffView = clipContent(m.diffPanel.View(), contentWidth, diffHeight)
	} else {
		diffView = baseStyle.
			Width(contentWidth).
			MaxWidth(contentWidth).
			Height(diffHeight).
			MaxHeight(diffHeight).
			Foreground(t.TextMuted()).
			Render("CHANGES\n  No file changes")
	}

	// ── Section 3: Terminal status ──
	var termView string
	if m.terminalPanel != nil {
		m.terminalPanel.SetSize(contentWidth, termHeight)
		termView = clipContent(m.terminalPanel.View(), contentWidth, termHeight)
	} else {
		termView = baseStyle.
			Width(contentWidth).
			MaxWidth(contentWidth).
			Height(termHeight).
			MaxHeight(termHeight).
			Foreground(t.TextMuted()).
			Render("Terminal\n  Waiting for commands...")
	}

	// ── Assemble using Place (not Width/Height) to protect banner ──
	content := lipgloss.JoinVertical(
		lipgloss.Top,
		bannerSection,
		divider,
		diffView,
		divider,
		termView,
	)

	// Use PaddingLeft/Right via Place offset, not Width constraint
	padded := baseStyle.PaddingLeft(4).PaddingRight(2).Render(content)
	return lipgloss.Place(m.width, m.height, lipgloss.Left, lipgloss.Top, padded)
}

func (m *sidebarCmp) sessionSection() string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	sessionKey := baseStyle.
		Foreground(t.Primary()).
		Bold(true).
		Render("Session")

	sessionValue := baseStyle.
		Foreground(t.Text()).
		Width(m.width - lipgloss.Width(sessionKey)).
		Render(fmt.Sprintf(": %s", m.session.Title))

	return lipgloss.JoinHorizontal(
		lipgloss.Left,
		sessionKey,
		sessionValue,
	)
}

func (m *sidebarCmp) modifiedFile(filePath string, additions, removals int) string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	stats := ""
	if additions > 0 && removals > 0 {
		additionsStr := baseStyle.
			Foreground(t.Success()).
			PaddingLeft(1).
			Render(fmt.Sprintf("+%d", additions))

		removalsStr := baseStyle.
			Foreground(t.Error()).
			PaddingLeft(1).
			Render(fmt.Sprintf("-%d", removals))

		content := lipgloss.JoinHorizontal(lipgloss.Left, additionsStr, removalsStr)
		stats = baseStyle.Width(lipgloss.Width(content)).Render(content)
	} else if additions > 0 {
		additionsStr := fmt.Sprintf(" %s", baseStyle.
			PaddingLeft(1).
			Foreground(t.Success()).
			Render(fmt.Sprintf("+%d", additions)))
		stats = baseStyle.Width(lipgloss.Width(additionsStr)).Render(additionsStr)
	} else if removals > 0 {
		removalsStr := fmt.Sprintf(" %s", baseStyle.
			PaddingLeft(1).
			Foreground(t.Error()).
			Render(fmt.Sprintf("-%d", removals)))
		stats = baseStyle.Width(lipgloss.Width(removalsStr)).Render(removalsStr)
	}

	filePathStr := baseStyle.Render(filePath)

	return baseStyle.
		Width(m.width).
		Render(
			lipgloss.JoinHorizontal(
				lipgloss.Left,
				filePathStr,
				stats,
			),
		)
}

func (m *sidebarCmp) modifiedFiles() string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	modifiedFiles := baseStyle.
		Width(m.width).
		Foreground(t.Primary()).
		Bold(true).
		Render("Modified Files:")

	// If no modified files, show a placeholder message
	if m.modFiles == nil || len(m.modFiles) == 0 {
		message := "No modified files"
		remainingWidth := m.width - lipgloss.Width(message)
		if remainingWidth > 0 {
			message += strings.Repeat(" ", remainingWidth)
		}
		return baseStyle.
			Width(m.width).
			Render(
				lipgloss.JoinVertical(
					lipgloss.Top,
					modifiedFiles,
					baseStyle.Foreground(t.TextMuted()).Render(message),
				),
			)
	}

	// Sort file paths alphabetically for consistent ordering
	var paths []string
	for path := range m.modFiles {
		paths = append(paths, path)
	}
	sort.Strings(paths)

	// Create views for each file in sorted order
	var fileViews []string
	for _, path := range paths {
		stats := m.modFiles[path]
		fileViews = append(fileViews, m.modifiedFile(path, stats.additions, stats.removals))
	}

	return baseStyle.
		Width(m.width).
		Render(
			lipgloss.JoinVertical(
				lipgloss.Top,
				modifiedFiles,
				lipgloss.JoinVertical(
					lipgloss.Left,
					fileViews...,
				),
			),
		)
}

func (m *sidebarCmp) SetSize(width, height int) tea.Cmd {
	m.width = width
	m.height = height
	return nil
}

func (m *sidebarCmp) GetSize() (int, int) {
	return m.width, m.height
}

func (m *sidebarCmp) BindingKeys() []key.Binding {
	return []key.Binding{
		sidebarKeys.DiffTaller,
		sidebarKeys.TerminalTaller,
	}
}

func (m *sidebarCmp) resizeDiff(delta float64) {
	m.diffRatio += delta
	if m.diffRatio < 0.25 {
		m.diffRatio = 0.25
	}
	if m.diffRatio > 0.80 {
		m.diffRatio = 0.80
	}
}

func (m *sidebarCmp) panelHeights(available int) (int, int) {
	available = max(0, available)
	if available == 0 {
		return 0, 0
	}
	ratio := m.diffRatio
	if ratio <= 0 {
		ratio = 0.60
	}
	minDiff := 3
	minTerm := 3
	if available < minDiff+minTerm {
		minTerm = min(2, max(0, available/3))
		minDiff = max(0, available-minTerm)
	}
	diffHeight := int(float64(available) * ratio)
	diffHeight = max(minDiff, min(diffHeight, available-minTerm))
	return diffHeight, available - diffHeight
}

func (m *sidebarCmp) mouseInTerminal(y int) bool {
	contentWidth := max(1, m.width-6)
	bannerLogo := logo(contentWidth)
	if m.height < 18 {
		bannerLogo = logo(min(contentWidth, 33))
	}
	bannerSection := lipgloss.JoinVertical(
		lipgloss.Center,
		bannerLogo,
		repo(contentWidth),
		"",
		cwd(contentWidth),
	)
	remainingHeight := m.height - lipgloss.Height(bannerSection) - 3
	if remainingHeight < 4 {
		remainingHeight = 4
	}
	diffHeight, _ := m.panelHeights(remainingHeight)
	terminalStart := lipgloss.Height(bannerSection) + 2 + diffHeight
	return y >= terminalStart
}

func NewSidebarCmp(session session.Session, history history.Service) tea.Model {
	return &sidebarCmp{
		session:       session,
		history:       history,
		diffRatio:     0.60,
		diffPanel:     NewDiffPanel(session.ID, history),
		terminalPanel: NewTerminalPanel(),
	}
}

func (m *sidebarCmp) loadModifiedFiles(ctx context.Context) {
	if m.history == nil || m.session.ID == "" {
		return
	}

	// Get all latest files for this session
	latestFiles, err := m.history.ListLatestSessionFiles(ctx, m.session.ID)
	if err != nil {
		return
	}

	// Get all files for this session (to find initial versions)
	allFiles, err := m.history.ListBySession(ctx, m.session.ID)
	if err != nil {
		return
	}

	// Clear the existing map to rebuild it
	m.modFiles = make(map[string]struct {
		additions int
		removals  int
	})

	// Process each latest file
	for _, file := range latestFiles {
		// Skip if this is the initial version (no changes to show)
		if file.Version == history.InitialVersion {
			continue
		}

		// Find the initial version for this specific file
		var initialVersion history.File
		for _, v := range allFiles {
			if v.Path == file.Path && v.Version == history.InitialVersion {
				initialVersion = v
				break
			}
		}

		// Skip if we can't find the initial version
		if initialVersion.ID == "" {
			continue
		}
		if initialVersion.Content == file.Content {
			continue
		}

		// Calculate diff between initial and latest version
		_, additions, removals := diff.GenerateDiff(initialVersion.Content, file.Content, file.Path)

		// Only add to modified files if there are changes
		if additions > 0 || removals > 0 {
			// Remove working directory prefix from file path
			displayPath := file.Path
			workingDir := config.WorkingDirectory()
			displayPath = strings.TrimPrefix(displayPath, workingDir)
			displayPath = strings.TrimPrefix(displayPath, "/")

			m.modFiles[displayPath] = struct {
				additions int
				removals  int
			}{
				additions: additions,
				removals:  removals,
			}
		}
	}
}

func (m *sidebarCmp) processFileChanges(ctx context.Context, file history.File) {
	// Skip if this is the initial version (no changes to show)
	if file.Version == history.InitialVersion {
		return
	}

	// Find the initial version for this file
	initialVersion, err := m.findInitialVersion(ctx, file.Path)
	if err != nil || initialVersion.ID == "" {
		return
	}

	// Skip if content hasn't changed
	if initialVersion.Content == file.Content {
		// If this file was previously modified but now matches the initial version,
		// remove it from the modified files list
		displayPath := getDisplayPath(file.Path)
		delete(m.modFiles, displayPath)
		return
	}

	// Calculate diff between initial and latest version
	_, additions, removals := diff.GenerateDiff(initialVersion.Content, file.Content, file.Path)

	// Only add to modified files if there are changes
	if additions > 0 || removals > 0 {
		displayPath := getDisplayPath(file.Path)
		m.modFiles[displayPath] = struct {
			additions int
			removals  int
		}{
			additions: additions,
			removals:  removals,
		}
	} else {
		// If no changes, remove from modified files
		displayPath := getDisplayPath(file.Path)
		delete(m.modFiles, displayPath)
	}
}

// Helper function to find the initial version of a file
func (m *sidebarCmp) findInitialVersion(ctx context.Context, path string) (history.File, error) {
	// Get all versions of this file for the session
	fileVersions, err := m.history.ListBySession(ctx, m.session.ID)
	if err != nil {
		return history.File{}, err
	}

	// Find the initial version
	for _, v := range fileVersions {
		if v.Path == path && v.Version == history.InitialVersion {
			return v, nil
		}
	}

	return history.File{}, fmt.Errorf("initial version not found")
}

// Helper function to get the display path for a file
func getDisplayPath(path string) string {
	workingDir := config.WorkingDirectory()
	displayPath := strings.TrimPrefix(path, workingDir)
	return strings.TrimPrefix(displayPath, "/")
}

// clipContent clips a multi-line string to exactly cols visual width and rows
// lines. Lines beyond rows are dropped. Lines wider than cols are truncated
// rune-by-rune. If there are fewer lines than rows, empty lines are appended.
// This prevents panels from overflowing their allocated region on resize.
func clipContent(s string, cols, rows int) string {
	if cols < 1 {
		cols = 1
	}
	if rows < 1 {
		rows = 1
	}
	lines := strings.Split(s, "\n")
	result := make([]string, rows)
	for i := 0; i < rows; i++ {
		if i < len(lines) {
			line := lines[i]
			if lipgloss.Width(line) > cols {
				runes := []rune(line)
				w := 0
				cut := len(runes)
				for j, _ := range runes {
					w++
					if w > cols {
						cut = j
						break
					}
				}
				result[i] = string(runes[:cut])
			} else {
				result[i] = line
			}
		}
	}
	return strings.Join(result, "\n")
}
