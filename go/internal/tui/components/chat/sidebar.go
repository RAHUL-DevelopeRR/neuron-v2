package chat

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/opencode-ai/opencode/internal/auth"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/diff"
	"github.com/opencode-ai/opencode/internal/history"
	"github.com/opencode-ai/opencode/internal/llm/agent"
	"github.com/opencode-ai/opencode/internal/llm/tools"
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
	focusTerminal bool
	modFiles      map[string]struct {
		additions int
		removals  int
	}
	toolTimeline          []toolTimelineEntry
	toolIndex             map[string]int
	timelineExpanded      bool
	timelineSelectedIndex int

	// New panels
	diffPanel     *diffPanelCmp
	terminalPanel *terminalPanelCmp
}

type toolTimelineEntry struct {
	id      string
	name    string
	summary string
	detail  string
	output  string
	status  string
	isError bool
}

type sidebarKeyMap struct {
	DiffTaller            key.Binding
	TerminalTaller        key.Binding
	ToggleCommandDetails  key.Binding
	PreviousCommandDetail key.Binding
	NextCommandDetail     key.Binding
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
	ToggleCommandDetails: key.NewBinding(
		key.WithKeys("ctrl+]"),
		key.WithHelp("ctrl+]", "expand command"),
	),
	PreviousCommandDetail: key.NewBinding(
		key.WithKeys("ctrl+left"),
		key.WithHelp("ctrl+left", "previous command"),
	),
	NextCommandDetail: key.NewBinding(
		key.WithKeys("ctrl+right"),
		key.WithHelp("ctrl+right", "next command"),
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
	if m.terminalPanel != nil {
		if cmd := m.terminalPanel.startShell(); cmd != nil {
			cmds = append(cmds, cmd)
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
		m.focusTerminal = msg.Focused
		if m.diffPanel != nil {
			m.diffPanel.SetFocus(!msg.Focused)
		}
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
		if key.Matches(msg, sidebarKeys.ToggleCommandDetails) {
			m.toggleTimelineDetails()
			return m, nil
		}
		if key.Matches(msg, sidebarKeys.PreviousCommandDetail) {
			m.selectTimelineEntry(-1)
			return m, nil
		}
		if key.Matches(msg, sidebarKeys.NextCommandDetail) {
			m.selectTimelineEntry(1)
			return m, nil
		}
		if m.diffPanel != nil {
			_, cmd := m.diffPanel.Update(msg)
			return m, cmd
		}
	case pubsub.Event[message.Message]:
		if msg.Payload.SessionID == m.session.ID {
			m.recordToolCalls(msg.Payload.ToolCalls())
			m.recordToolResults(msg.Payload.ToolResults())
		}
		// Extract tool results and forward to terminal panel if this panel opts into it.
		if m.terminalPanel != nil && msg.Payload.Role == message.Tool {
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

	accountHeight := 7
	if m.height < 34 {
		accountHeight = 5
	}
	if m.height < 24 {
		accountHeight = 3
	}
	accountView := clipContent(m.accountBoard(contentWidth, accountHeight), contentWidth, accountHeight)

	// ── Remaining height for panels ──
	timelineHeight := 4
	if len(m.toolTimeline) > 3 && m.height >= 26 {
		timelineHeight = 6
	}
	if m.height < 20 {
		timelineHeight = 3
	}
	if m.timelineExpanded && len(m.toolTimeline) > 0 && m.height >= 28 {
		timelineHeight = min(10, max(timelineHeight, m.height/4))
	}
	timelineView := clipContent(m.commandTimelineView(contentWidth, timelineHeight), contentWidth, timelineHeight)

	remainingHeight := m.height - bannerHeight - accountHeight - timelineHeight - (dividerHeight * 4) - 1
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
		accountView,
		divider,
		timelineView,
		divider,
		diffView,
		divider,
		termView,
	)

	// Use PaddingLeft/Right via Place offset, not Width constraint
	padded := baseStyle.PaddingLeft(4).PaddingRight(2).Render(content)
	return lipgloss.Place(m.width, m.height, lipgloss.Left, lipgloss.Top, padded)
}

func (m *sidebarCmp) accountBoard(width, height int) string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()
	cache, _, ok := auth.LoadSession()

	themeCount := len(theme.AvailableThemes())
	titleText := "PROFILE / USAGE"
	if themeCount > 1 {
		titleText = fmt.Sprintf("%s  theme %s (%d)", titleText, theme.CurrentThemeName(), themeCount)
	}
	title := baseStyle.
		Width(width).
		Foreground(t.Secondary()).
		Background(t.BackgroundSecondary()).
		Bold(true).
		Render(ansi.Truncate(titleText, width, "..."))
	if height <= 1 {
		return title
	}

	if !ok {
		signIn := baseStyle.
			Width(width).
			Foreground(t.Warning()).
			Render("  Not signed in")
		action := baseStyle.
			Width(width).
			Foreground(t.TextMuted()).
			Render(ansi.Truncate("  Run: neuron auth login", width, "..."))
		return lipgloss.JoinVertical(lipgloss.Top, title, signIn, action)
	}

	name := auth.DisplayName(cache)
	if isGuestSession(cache) {
		name = "Guest session"
	}
	plan := strings.ToUpper(cache.Plan)
	if plan == "" {
		plan = "FREE"
	}
	used := cache.Quota.Used
	if used == 0 && cache.Usage.TokensUsed > 0 {
		used = cache.Usage.TokensUsed
	}
	limit := cache.Quota.DailyLimit
	if limit == 0 {
		limit = auth.PlanLimit(cache.Plan)
	}
	remaining := cache.Quota.Remaining
	if remaining == 0 && limit > 0 {
		remaining = max(int64(0), limit-used)
	}

	identityLine := baseStyle.
		Width(width).
		Foreground(t.Text()).
		Render(ansi.Truncate("  "+name+" | "+plan, width, "..."))
	usageLine := baseStyle.
		Width(width).
		Foreground(t.Accent()).
		Render(ansi.Truncate(fmt.Sprintf("  Usage %s/%s  left %s", compactCount(used), compactCount(limit), compactCount(remaining)), width, "..."))
	barLine := baseStyle.
		Width(width).
		Foreground(t.Success()).
		Render("  " + usageBar(width-4, used, limit))
	mediaLine := baseStyle.
		Width(width).
		Foreground(t.TextMuted()).
		Render(ansi.Truncate("  ctrl+f image file  ctrl+y paste image  ctrl+v paste text", width, "..."))
	themeLine := baseStyle.
		Width(width).
		Foreground(t.TextMuted()).
		Render(ansi.Truncate("  ctrl+t themes  alt+arrows resize panes", width, "..."))

	lines := []string{title, identityLine, usageLine}
	if height >= 5 {
		lines = append(lines, barLine, mediaLine)
	}
	if height >= 7 {
		lines = append(lines, themeLine)
	}
	return lipgloss.JoinVertical(lipgloss.Top, lines...)
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
		sidebarKeys.ToggleCommandDetails,
		sidebarKeys.PreviousCommandDetail,
		sidebarKeys.NextCommandDetail,
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

func (m *sidebarCmp) commandTimelineView(width, height int) string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()
	titleText := "COMMAND TIMELINE"
	if len(m.toolTimeline) > 0 {
		titleText += "  ctrl+] expand"
		if m.timelineExpanded {
			titleText += "  ctrl+left/right"
		}
	}
	title := baseStyle.
		Width(width).
		Foreground(t.Primary()).
		Background(t.BackgroundSecondary()).
		Bold(true).
		Render(ansi.Truncate(titleText, width, "..."))

	if height <= 1 {
		return title
	}

	detailLines := []string{}
	detailHeight := 0
	selectedIndex := m.selectedTimelineIndex()
	if m.timelineExpanded && selectedIndex >= 0 && height >= 4 {
		detailHeight = min(4, height-2)
		detailLines = m.timelineDetailLines(m.toolTimeline[selectedIndex], width, detailHeight)
	}

	lineHeight := height - 1 - detailHeight
	if lineHeight < 0 {
		lineHeight = 0
	}
	if len(m.toolTimeline) == 0 {
		empty := baseStyle.
			Width(width).
			Height(lineHeight).
			Foreground(t.TextMuted()).
			Italic(true).
			Render("  Waiting for tool calls")
		return lipgloss.JoinVertical(lipgloss.Top, title, empty)
	}

	start := max(0, len(m.toolTimeline)-lineHeight)
	if m.timelineExpanded && selectedIndex >= 0 && lineHeight > 0 {
		start = min(start, selectedIndex)
		if selectedIndex >= start+lineHeight {
			start = selectedIndex - lineHeight + 1
		}
		start = max(0, start)
	}
	rows := make([]string, 0, lineHeight)
	end := min(len(m.toolTimeline), start+lineHeight)
	for idx := start; idx < end; idx++ {
		item := m.toolTimeline[idx]
		statusColor := t.TextMuted()
		switch item.status {
		case "running":
			statusColor = t.Primary()
		case "done":
			statusColor = t.Success()
		case "error":
			statusColor = t.Error()
		}
		name := baseStyle.Foreground(statusColor).Bold(true).Render(item.name)
		status := baseStyle.Foreground(statusColor).Render(item.status)
		prefix := "  "
		if m.timelineExpanded && idx == selectedIndex {
			prefix = "> "
		}
		summaryWidth := max(1, width-lipgloss.Width(prefix)-lipgloss.Width(name)-lipgloss.Width(status)-4)
		summary := baseStyle.
			Foreground(t.Text()).
			Render(ansi.Truncate(item.summary, summaryWidth, "..."))
		rows = append(rows, baseStyle.Width(width).Render(prefix+name+" "+summary+" "+status))
	}
	parts := []string{title}
	if len(rows) > 0 {
		parts = append(parts, strings.Join(rows, "\n"))
	}
	if len(detailLines) > 0 {
		parts = append(parts, strings.Join(detailLines, "\n"))
	}
	return lipgloss.JoinVertical(lipgloss.Top, parts...)
}

func (m *sidebarCmp) selectedTimelineIndex() int {
	if len(m.toolTimeline) == 0 {
		return -1
	}
	if m.timelineSelectedIndex < 0 || m.timelineSelectedIndex >= len(m.toolTimeline) {
		m.timelineSelectedIndex = len(m.toolTimeline) - 1
	}
	return m.timelineSelectedIndex
}

func (m *sidebarCmp) toggleTimelineDetails() {
	if len(m.toolTimeline) == 0 {
		return
	}
	if m.timelineSelectedIndex < 0 || m.timelineSelectedIndex >= len(m.toolTimeline) {
		m.timelineSelectedIndex = len(m.toolTimeline) - 1
	}
	m.timelineExpanded = !m.timelineExpanded
}

func (m *sidebarCmp) selectTimelineEntry(delta int) {
	if len(m.toolTimeline) == 0 {
		return
	}
	if m.timelineSelectedIndex < 0 || m.timelineSelectedIndex >= len(m.toolTimeline) {
		m.timelineSelectedIndex = len(m.toolTimeline) - 1
	}
	m.timelineSelectedIndex += delta
	if m.timelineSelectedIndex < 0 {
		m.timelineSelectedIndex = 0
	}
	if m.timelineSelectedIndex >= len(m.toolTimeline) {
		m.timelineSelectedIndex = len(m.toolTimeline) - 1
	}
	m.timelineExpanded = true
}

func (m *sidebarCmp) timelineDetailLines(item toolTimelineEntry, width, height int) []string {
	if height <= 0 {
		return nil
	}
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()
	header := baseStyle.
		Width(width).
		Foreground(t.Secondary()).
		Render(ansi.Truncate("  [-] "+item.name+" command details", width, "..."))
	lines := []string{header}
	if height == 1 {
		return lines
	}

	detail := strings.TrimSpace(item.detail)
	if detail == "" {
		detail = strings.TrimSpace(item.summary)
	}
	if detail == "" {
		detail = "No command payload yet"
	}
	lines = append(lines, wrapTimelineText("$ "+detail, width, height-len(lines))...)
	if len(lines) >= height || strings.TrimSpace(item.output) == "" {
		return lines[:min(len(lines), height)]
	}
	output := "=> " + oneLine(item.output)
	lines = append(lines, wrapTimelineText(output, width, height-len(lines))...)
	return lines[:min(len(lines), height)]
}

func wrapTimelineText(text string, width, maxLines int) []string {
	if maxLines <= 0 {
		return nil
	}
	width = max(1, width)
	available := max(1, width-2)
	words := strings.Fields(text)
	if len(words) == 0 {
		return []string{strings.Repeat(" ", width)}
	}
	lines := make([]string, 0, maxLines)
	current := ""
	for _, word := range words {
		next := word
		if current != "" {
			next = current + " " + word
		}
		if lipgloss.Width(next) > available && current != "" {
			lines = append(lines, padVisualLine("  "+ansi.Truncate(current, available, "..."), width))
			current = word
			if len(lines) == maxLines {
				return lines
			}
			continue
		}
		current = next
	}
	if len(lines) < maxLines && current != "" {
		lines = append(lines, padVisualLine("  "+ansi.Truncate(current, available, "..."), width))
	}
	return lines
}

func (m *sidebarCmp) recordToolCalls(calls []message.ToolCall) {
	if len(calls) == 0 {
		return
	}
	if m.toolIndex == nil {
		m.toolIndex = make(map[string]int)
	}
	for _, call := range calls {
		name := toolName(call.Name)
		summary := summaryForToolCall(call)
		detail := detailForToolCall(call)
		status := "running"
		if call.Finished {
			status = "sent"
		}
		if idx, ok := m.toolIndex[call.ID]; ok && idx < len(m.toolTimeline) {
			m.toolTimeline[idx].name = name
			m.toolTimeline[idx].summary = summary
			m.toolTimeline[idx].detail = detail
			m.toolTimeline[idx].status = status
			continue
		}
		m.toolIndex[call.ID] = len(m.toolTimeline)
		m.toolTimeline = append(m.toolTimeline, toolTimelineEntry{
			id:      call.ID,
			name:    name,
			summary: summary,
			detail:  detail,
			status:  status,
		})
		if m.timelineSelectedIndex < 0 || m.timelineSelectedIndex == len(m.toolTimeline)-2 {
			m.timelineSelectedIndex = len(m.toolTimeline) - 1
		}
	}
	m.trimTimeline()
}

func (m *sidebarCmp) recordToolResults(results []message.ToolResult) {
	if len(results) == 0 || m.toolIndex == nil {
		return
	}
	for _, result := range results {
		idx, ok := m.toolIndex[result.ToolCallID]
		if !ok || idx >= len(m.toolTimeline) {
			continue
		}
		m.toolTimeline[idx].isError = result.IsError
		if result.IsError {
			m.toolTimeline[idx].status = "error"
		} else {
			m.toolTimeline[idx].status = "done"
		}
		if strings.TrimSpace(result.Content) != "" {
			m.toolTimeline[idx].output = result.Content
		}
	}
}

func (m *sidebarCmp) trimTimeline() {
	const maxEntries = 80
	if len(m.toolTimeline) <= maxEntries {
		return
	}
	m.toolTimeline = append([]toolTimelineEntry(nil), m.toolTimeline[len(m.toolTimeline)-maxEntries:]...)
	m.toolIndex = make(map[string]int, len(m.toolTimeline))
	for i, item := range m.toolTimeline {
		m.toolIndex[item.id] = i
	}
	if m.timelineSelectedIndex >= len(m.toolTimeline) {
		m.timelineSelectedIndex = len(m.toolTimeline) - 1
	}
}

func summaryForToolCall(call message.ToolCall) string {
	switch call.Name {
	case tools.BashToolName:
		var params tools.BashParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Command) != "" {
			return "$ " + oneLine(params.Command)
		}
	case agent.AgentToolName:
		var params agent.AgentParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Prompt) != "" {
			return oneLine(params.Prompt)
		}
	case tools.EditToolName:
		var params tools.EditParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.FilePath) != "" {
			return removeWorkingDirPrefix(params.FilePath)
		}
	case tools.WriteToolName:
		var params tools.WriteParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.FilePath) != "" {
			return removeWorkingDirPrefix(params.FilePath)
		}
	case tools.ViewToolName:
		var params tools.ViewParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.FilePath) != "" {
			return removeWorkingDirPrefix(params.FilePath)
		}
	case tools.GrepToolName:
		var params tools.GrepParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Pattern) != "" {
			return oneLine(params.Pattern)
		}
	case tools.GlobToolName:
		var params tools.GlobParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Pattern) != "" {
			return oneLine(params.Pattern)
		}
	case tools.LSToolName:
		var params tools.LSParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil {
			if strings.TrimSpace(params.Path) == "" {
				return "."
			}
			return removeWorkingDirPrefix(params.Path)
		}
	}
	if strings.TrimSpace(call.Input) == "" {
		return "preparing"
	}
	return oneLine(call.Input)
}

func detailForToolCall(call message.ToolCall) string {
	switch call.Name {
	case tools.BashToolName:
		var params tools.BashParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Command) != "" {
			return strings.TrimSpace(params.Command)
		}
	case agent.AgentToolName:
		var params agent.AgentParams
		if err := json.Unmarshal([]byte(call.Input), &params); err == nil && strings.TrimSpace(params.Prompt) != "" {
			return strings.TrimSpace(params.Prompt)
		}
	}
	input := strings.TrimSpace(call.Input)
	if input == "" {
		return ""
	}
	var payload any
	if err := json.Unmarshal([]byte(input), &payload); err == nil {
		if pretty, err := json.MarshalIndent(payload, "", "  "); err == nil {
			return string(pretty)
		}
	}
	return input
}

func oneLine(value string) string {
	return strings.Join(strings.Fields(value), " ")
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
	timelineHeight := 4
	if len(m.toolTimeline) > 3 && m.height >= 26 {
		timelineHeight = 6
	}
	if m.height < 20 {
		timelineHeight = 3
	}
	if m.timelineExpanded && len(m.toolTimeline) > 0 && m.height >= 28 {
		timelineHeight = min(10, max(timelineHeight, m.height/4))
	}
	accountHeight := 7
	if m.height < 34 {
		accountHeight = 5
	}
	if m.height < 24 {
		accountHeight = 3
	}
	remainingHeight := m.height - lipgloss.Height(bannerSection) - accountHeight - timelineHeight - 5
	if remainingHeight < 4 {
		remainingHeight = 4
	}
	diffHeight, _ := m.panelHeights(remainingHeight)
	terminalStart := lipgloss.Height(bannerSection) + accountHeight + timelineHeight + 4 + diffHeight
	return y >= terminalStart
}

func NewSidebarCmp(session session.Session, history history.Service) tea.Model {
	return &sidebarCmp{
		session:               session,
		history:               history,
		diffRatio:             0.60,
		toolIndex:             make(map[string]int),
		timelineSelectedIndex: -1,
		diffPanel:             NewDiffPanel(session.ID, history),
		terminalPanel:         NewTerminalPanel(),
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

func isGuestSession(cache auth.SessionCache) bool {
	return strings.TrimSpace(cache.UserID) == "" &&
		strings.TrimSpace(cache.Email) == "" &&
		strings.TrimSpace(cache.Name) == ""
}

func compactCount(v int64) string {
	switch {
	case v >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(v)/1_000_000)
	case v >= 1_000:
		return fmt.Sprintf("%.0fK", float64(v)/1_000)
	default:
		return fmt.Sprintf("%d", v)
	}
}

func usageBar(width int, used, limit int64) string {
	if width < 8 {
		width = 8
	}
	if width > 32 {
		width = 32
	}
	if limit <= 0 {
		return "[" + strings.Repeat("-", width-2) + "]"
	}
	ratio := float64(used) / float64(limit)
	if ratio < 0 {
		ratio = 0
	}
	if ratio > 1 {
		ratio = 1
	}
	inner := width - 2
	filled := int(float64(inner) * ratio)
	return "[" + strings.Repeat("=", filled) + strings.Repeat("-", inner-filled) + "]"
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
		result[i] = padVisualLine(result[i], cols)
	}
	return strings.Join(result, "\n")
}

func padVisualLine(line string, width int) string {
	if width <= 0 {
		return ""
	}
	lineWidth := lipgloss.Width(line)
	if lineWidth >= width {
		return line
	}
	return line + strings.Repeat(" ", width-lineWidth)
}
