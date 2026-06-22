package chat

import (
	"context"
	"fmt"
	"sort"
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/diff"
	"github.com/opencode-ai/opencode/internal/history"
	"github.com/opencode-ai/opencode/internal/pubsub"
	"github.com/opencode-ai/opencode/internal/tui/styles"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

// diffPanelCmp shows a scrollable unified diff of all modified files.
type diffPanelCmp struct {
	width, height  int
	history        history.Service
	sessionID      string
	scrollOffset   int
	diffLines      []string // pre-rendered diff lines
	fileList       []string // sorted file paths
	focused        bool
	totalAdditions int
	totalRemovals  int
}

func NewDiffPanel(sessionID string, history history.Service) *diffPanelCmp {
	return &diffPanelCmp{
		history:   history,
		sessionID: sessionID,
		diffLines: []string{},
		fileList:  []string{},
		focused:   true,
	}
}

func (d *diffPanelCmp) Init() tea.Cmd {
	if d.history != nil {
		ctx := context.Background()
		d.rebuildDiff(ctx)
	}
	return nil
}

func (d *diffPanelCmp) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case SessionSelectedMsg:
		d.sessionID = msg.ID
		d.rebuildDiff(context.Background())
	case pubsub.Event[history.File]:
		if msg.Payload.SessionID == d.sessionID {
			d.rebuildDiff(context.Background())
		}
	case tea.KeyMsg:
		switch {
		case key.Matches(msg, messageKeys.PageUp):
			d.ScrollUp(max(1, d.height-2))
		case key.Matches(msg, messageKeys.PageDown):
			d.ScrollDown(max(1, d.height-2))
		case key.Matches(msg, messageKeys.HalfPageUp):
			d.ScrollUp(max(1, (d.height-2)/2))
		case key.Matches(msg, messageKeys.HalfPageDown):
			d.ScrollDown(max(1, (d.height-2)/2))
		}
	}
	return d, nil
}

func (d *diffPanelCmp) ScrollUp(lines int) {
	if lines < 1 {
		lines = 1
	}
	d.scrollOffset -= lines
	d.clampScroll()
}

func (d *diffPanelCmp) ScrollDown(lines int) {
	if lines < 1 {
		lines = 1
	}
	d.scrollOffset += lines
	d.clampScroll()
}

func (d *diffPanelCmp) View() string {
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	title := d.titleBar()

	if len(d.diffLines) == 0 {
		noChanges := baseStyle.
			Width(d.width).
			Foreground(t.TextMuted()).
			Italic(true).
			Render("  No file changes")
		return lipgloss.JoinVertical(lipgloss.Top, title, noChanges)
	}

	visibleHeight := d.height - 1
	if visibleHeight < 1 {
		visibleHeight = 1
	}

	d.clampScroll()
	start := d.scrollOffset
	end := start + visibleHeight
	if end > len(d.diffLines) {
		end = len(d.diffLines)
	}
	if start >= end {
		start = 0
		if end > len(d.diffLines) {
			end = len(d.diffLines)
		}
	}

	contentWidth := d.width
	hasScrollbar := len(d.diffLines) > visibleHeight
	if hasScrollbar {
		contentWidth = max(1, d.width-1)
	}
	visible := append([]string(nil), d.diffLines[start:end]...)
	for i, line := range visible {
		visible[i] = ansi.Truncate(line, contentWidth, "...")
	}
	content := strings.Join(visible, "\n")
	if hasScrollbar {
		content = lipgloss.JoinHorizontal(lipgloss.Top, content, renderPanelScrollbar(visibleHeight, len(d.diffLines), start))
	}

	return lipgloss.JoinVertical(lipgloss.Top,
		d.titleBarWithPosition(start+1, len(d.diffLines), visibleHeight),
		content,
	)
}

func (d *diffPanelCmp) SetSize(width, height int) {
	widthChanged := d.width != width
	d.width = width
	d.height = height
	d.clampScroll()
	if widthChanged && d.sessionID != "" && d.history != nil {
		d.rebuildDiff(context.Background())
	}
}

func (d *diffPanelCmp) SetFocus(focused bool) {
	d.focused = focused
}

func (d *diffPanelCmp) titleBar() string {
	t := theme.CurrentTheme()
	color := t.TextMuted()
	if d.focused {
		color = t.Primary()
	}
	stats := "CHANGES"
	if len(d.fileList) > 0 {
		stats = fmt.Sprintf("CHANGES %d files +%d -%d", len(d.fileList), d.totalAdditions, d.totalRemovals)
	}
	return styles.BaseStyle().
		Width(d.width).
		Foreground(color).
		Background(t.BackgroundSecondary()).
		Bold(true).
		Render(ansi.Truncate(stats, d.width, "..."))
}

func (d *diffPanelCmp) titleBarWithPosition(start, total, visibleHeight int) string {
	if total <= visibleHeight || d.width <= 0 {
		return d.titleBar()
	}
	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()
	color := t.TextMuted()
	if d.focused {
		color = t.Primary()
	}
	titleText := "CHANGES"
	if len(d.fileList) > 0 {
		titleText = fmt.Sprintf("CHANGES %d files +%d -%d", len(d.fileList), d.totalAdditions, d.totalRemovals)
	}
	position := baseStyle.Foreground(t.TextMuted()).Render(fmt.Sprintf("%d/%d", start, total))
	title := baseStyle.Foreground(color).Bold(true).Render(ansi.Truncate(titleText, max(1, d.width-lipgloss.Width(position)-1), "..."))
	spacerWidth := d.width - lipgloss.Width(title) - lipgloss.Width(position)
	if spacerWidth < 1 {
		spacerWidth = 1
	}
	return baseStyle.
		Width(d.width).
		Background(t.BackgroundSecondary()).
		Render(title + strings.Repeat(" ", spacerWidth) + position)
}

func (d *diffPanelCmp) clampScroll() {
	visibleHeight := max(1, d.height-1)
	maxScroll := len(d.diffLines) - visibleHeight
	if maxScroll < 0 {
		maxScroll = 0
	}
	if d.scrollOffset < 0 {
		d.scrollOffset = 0
	}
	if d.scrollOffset > maxScroll {
		d.scrollOffset = maxScroll
	}
}

func renderPanelScrollbar(viewHeight, totalLines, start int) string {
	if viewHeight < 1 || totalLines <= viewHeight {
		return ""
	}
	thumbSize := max(1, viewHeight*viewHeight/totalLines)
	scrollRange := max(1, totalLines-viewHeight)
	thumbPos := start * (viewHeight - thumbSize) / scrollRange
	if thumbPos+thumbSize > viewHeight {
		thumbPos = viewHeight - thumbSize
	}
	t := theme.CurrentTheme()
	trackStyle := lipgloss.NewStyle().Foreground(t.TextMuted())
	thumbStyle := lipgloss.NewStyle().Foreground(t.Primary())
	var sb strings.Builder
	for i := 0; i < viewHeight; i++ {
		if i > 0 {
			sb.WriteByte('\n')
		}
		if i >= thumbPos && i < thumbPos+thumbSize {
			sb.WriteString(thumbStyle.Render("|"))
		} else {
			sb.WriteString(trackStyle.Render(" "))
		}
	}
	return sb.String()
}

func (d *diffPanelCmp) rebuildDiff(ctx context.Context) {
	if d.history == nil || d.sessionID == "" {
		return
	}

	t := theme.CurrentTheme()
	baseStyle := styles.BaseStyle()

	latestFiles, err := d.history.ListLatestSessionFiles(ctx, d.sessionID)
	if err != nil {
		return
	}

	allFiles, err := d.history.ListBySession(ctx, d.sessionID)
	if err != nil {
		return
	}

	d.diffLines = nil
	d.fileList = nil
	d.totalAdditions = 0
	d.totalRemovals = 0

	// Collect files with changes
	type fileChange struct {
		path                string
		additions, removals int
		diffText            string
	}
	var changes []fileChange

	for _, file := range latestFiles {
		if file.Version == history.InitialVersion {
			continue
		}
		var initial history.File
		for _, v := range allFiles {
			if v.Path == file.Path && v.Version == history.InitialVersion {
				initial = v
				break
			}
		}
		if initial.ID == "" || initial.Content == file.Content {
			continue
		}

		unifiedDiff, additions, removals := diff.GenerateDiff(initial.Content, file.Content, file.Path)
		if additions == 0 && removals == 0 {
			continue
		}

		displayPath := file.Path
		wd := config.WorkingDirectory()
		displayPath = strings.TrimPrefix(displayPath, wd)
		displayPath = strings.TrimPrefix(displayPath, "/")
		displayPath = strings.TrimPrefix(displayPath, "\\")

		changes = append(changes, fileChange{
			path:      displayPath,
			additions: additions,
			removals:  removals,
			diffText:  unifiedDiff,
		})
	}

	sort.Slice(changes, func(i, j int) bool {
		return changes[i].path < changes[j].path
	})

	for _, fc := range changes {
		d.fileList = append(d.fileList, fc.path)
		d.totalAdditions += fc.additions
		d.totalRemovals += fc.removals

		// File header line
		addStr := baseStyle.Foreground(t.Success()).Render(fmt.Sprintf("+%d", fc.additions))
		remStr := ""
		if fc.removals > 0 {
			remStr = baseStyle.Foreground(t.Error()).Render(fmt.Sprintf(" -%d", fc.removals))
		}
		header := baseStyle.
			Width(d.width).
			Foreground(t.Text()).
			Render(fmt.Sprintf("--- %s %s%s", fc.path, addStr, remStr))
		d.diffLines = append(d.diffLines, header)

		// Render actual diff lines with colors
		for _, line := range strings.Split(fc.diffText, "\n") {
			if line == "" {
				continue
			}
			var rendered string
			if strings.HasPrefix(line, "+") {
				rendered = baseStyle.Width(d.width).Foreground(t.Success()).Render(line)
			} else if strings.HasPrefix(line, "-") {
				rendered = baseStyle.Width(d.width).Foreground(t.Error()).Render(line)
			} else if strings.HasPrefix(line, "@@") {
				rendered = baseStyle.Width(d.width).Foreground(t.Primary()).Render(line)
			} else {
				rendered = baseStyle.Width(d.width).Foreground(t.TextMuted()).Render(line)
			}
			d.diffLines = append(d.diffLines, rendered)
		}
		d.diffLines = append(d.diffLines, "") // spacer
	}
	d.clampScroll()
}
