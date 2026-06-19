package dialog

import (
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/charmbracelet/bubbles/key"
	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/tui/layout"
	"github.com/opencode-ai/opencode/internal/tui/styles"
	"github.com/opencode-ai/opencode/internal/tui/theme"
	"github.com/opencode-ai/opencode/internal/tui/util"
)

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

// SwitchCWDMsg is dispatched when the user confirms a new working directory.
type SwitchCWDMsg struct {
	NewPath string
}

// CloseCWDDialogMsg is dispatched when the dialog is dismissed.
type CloseCWDDialogMsg struct{}

// ShowCWDDialogMsg is dispatched to open the CWD dialog.
type ShowCWDDialogMsg struct{}

// ---------------------------------------------------------------------------
// Interface
// ---------------------------------------------------------------------------

// CWDDialog is the public interface for the working-directory switcher.
type CWDDialog interface {
	tea.Model
	layout.Bindings
	SetSize(width, height int)
}

// ---------------------------------------------------------------------------
// Key bindings
// ---------------------------------------------------------------------------

type cwdKeyMap struct {
	Up     key.Binding
	Down   key.Binding
	Enter  key.Binding
	Escape key.Binding
	Tab    key.Binding
	Back   key.Binding
	J      key.Binding
	K      key.Binding
}

var cwdKeys = cwdKeyMap{
	Up: key.NewBinding(
		key.WithKeys("up"),
		key.WithHelp("up", "previous dir"),
	),
	Down: key.NewBinding(
		key.WithKeys("down"),
		key.WithHelp("down", "next dir"),
	),
	Enter: key.NewBinding(
		key.WithKeys("enter"),
		key.WithHelp("enter", "select"),
	),
	Escape: key.NewBinding(
		key.WithKeys("esc"),
		key.WithHelp("esc", "cancel"),
	),
	Tab: key.NewBinding(
		key.WithKeys("tab"),
		key.WithHelp("tab", "switch focus"),
	),
	Back: key.NewBinding(
		key.WithKeys("backspace"),
		key.WithHelp("backspace", "parent dir"),
	),
	J: key.NewBinding(
		key.WithKeys("j"),
		key.WithHelp("j", "next dir"),
	),
	K: key.NewBinding(
		key.WithKeys("k"),
		key.WithHelp("k", "previous dir"),
	),
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

const maxVisibleDirs = 15

type cwdDialogCmp struct {
	input       textinput.Model
	dirs        []string // subdirectory names in currentPath
	currentPath string
	selectedIdx int
	scrollOff   int // first visible index
	inputFocus  bool
	width       int
	height      int
	errMsg      string
}

// NewCWDDialogCmp creates a new CWD switching dialog rooted at the current
// working directory.
func NewCWDDialogCmp() CWDDialog {
	ti := textinput.New()
	ti.Prompt = "> "
	ti.CharLimit = 512
	ti.Width = 54

	cwd := config.WorkingDirectory()

	c := &cwdDialogCmp{
		input:       ti,
		currentPath: cwd,
		inputFocus:  true,
	}
	c.input.Focus()
	c.input.SetValue(cwd)
	c.refreshDirs()
	return c
}

// ---------------------------------------------------------------------------
// tea.Model
// ---------------------------------------------------------------------------

func (c *cwdDialogCmp) Init() tea.Cmd {
	return textinput.Blink
}

func (c *cwdDialogCmp) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		c.width = msg.Width
		c.height = msg.Height
		return c, nil

	case tea.KeyMsg:
		// --- Tab always toggles focus ---
		if key.Matches(msg, cwdKeys.Tab) {
			c.inputFocus = !c.inputFocus
			if c.inputFocus {
				c.input.Focus()
			} else {
				c.input.Blur()
			}
			return c, nil
		}

		// --- Escape always closes ---
		if key.Matches(msg, cwdKeys.Escape) {
			return c, util.CmdHandler(CloseCWDDialogMsg{})
		}

		// --- When the text input is focused ---
		if c.inputFocus {
			if key.Matches(msg, cwdKeys.Enter) {
				return c, c.switchToInput()
			}
			var cmd tea.Cmd
			c.input, cmd = c.input.Update(msg)
			return c, cmd
		}

		// --- Directory list navigation ---
		switch {
		case key.Matches(msg, cwdKeys.Up) || key.Matches(msg, cwdKeys.K):
			if c.selectedIdx > 0 {
				c.selectedIdx--
				c.ensureVisible()
			}
			return c, nil

		case key.Matches(msg, cwdKeys.Down) || key.Matches(msg, cwdKeys.J):
			if c.selectedIdx < len(c.dirs)-1 {
				c.selectedIdx++
				c.ensureVisible()
			}
			return c, nil

		case key.Matches(msg, cwdKeys.Enter):
			return c, c.handleEnter()

		case key.Matches(msg, cwdKeys.Back):
			c.navigateUp()
			return c, nil
		}
	}
	return c, nil
}

func (c *cwdDialogCmp) View() string {
	t := theme.CurrentTheme()
	base := styles.BaseStyle()

	dialogWidth := 60
	if c.width > 0 && c.width < dialogWidth+6 {
		dialogWidth = c.width - 6
	}
	if dialogWidth < 30 {
		dialogWidth = 30
	}
	innerWidth := dialogWidth - 4

	// Title
	title := base.
		Foreground(t.Primary()).
		Bold(true).
		Width(innerWidth).
		Padding(0, 1).
		Render("Switch Working Directory")

	// Path input
	c.input.Width = innerWidth - 4
	inputStyle := base.Width(innerWidth).Padding(0, 1)
	pathInput := inputStyle.Render(c.input.View())

	// Error line (if any)
	var errLine string
	if c.errMsg != "" {
		errLine = base.
			Foreground(t.Error()).
			Width(innerWidth).
			Padding(0, 1).
			Render(c.errMsg)
	}

	// Directory listing
	listItems := c.buildDirList(t, base, innerWidth)
	list := base.Width(innerWidth).Render(
		lipgloss.JoinVertical(lipgloss.Left, listItems...),
	)

	// Hint bar
	hintText := "[Enter] Switch typed path  [Tab] Browse  [Esc] Cancel"
	if !c.inputFocus {
		hintText = "[Enter] Open/Switch  [Backspace] Parent  [Tab] Path Input  [Esc] Cancel"
	}
	hint := base.
		Foreground(t.TextMuted()).
		Width(innerWidth).
		Padding(0, 1).
		Render(hintText)

	// Assemble content
	parts := []string{title, "", pathInput}
	if errLine != "" {
		parts = append(parts, errLine)
	}
	parts = append(parts, "", list, "", hint)
	content := lipgloss.JoinVertical(lipgloss.Left, parts...)

	return base.Padding(1, 2).
		Border(lipgloss.RoundedBorder()).
		BorderBackground(t.Background()).
		BorderForeground(t.BorderFocused()).
		Width(lipgloss.Width(content) + 4).
		Render(content)
}

// ---------------------------------------------------------------------------
// layout.Bindings
// ---------------------------------------------------------------------------

func (c *cwdDialogCmp) BindingKeys() []key.Binding {
	return layout.KeyMapToSlice(cwdKeys)
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

// SetSize lets the parent set the available viewport.
func (c *cwdDialogCmp) SetSize(width, height int) {
	c.width = width
	c.height = height
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

func (c *cwdDialogCmp) refreshDirs() {
	c.dirs = readSubDirs(c.currentPath)
	c.selectedIdx = 0
	c.scrollOff = 0
	c.errMsg = ""
	c.input.SetValue(c.currentPath)
}

func (c *cwdDialogCmp) ensureVisible() {
	if c.selectedIdx < c.scrollOff {
		c.scrollOff = c.selectedIdx
	} else if c.selectedIdx >= c.scrollOff+maxVisibleDirs {
		c.scrollOff = c.selectedIdx - maxVisibleDirs + 1
	}
}

func (c *cwdDialogCmp) navigateUp() {
	parent := filepath.Dir(c.currentPath)
	if parent == c.currentPath {
		return // already at root
	}
	c.currentPath = parent
	c.refreshDirs()
}

func (c *cwdDialogCmp) switchToInput() tea.Cmd {
	target := strings.Trim(strings.TrimSpace(c.input.Value()), "\"'")
	if target == "" {
		return nil
	}
	target = filepath.Clean(os.ExpandEnv(target))
	info, err := os.Stat(target)
	if err != nil || !info.IsDir() {
		c.errMsg = "Not a valid directory"
		return nil
	}
	abs, err := filepath.Abs(target)
	if err != nil {
		c.errMsg = "Unable to resolve directory"
		return nil
	}
	return util.CmdHandler(SwitchCWDMsg{NewPath: abs})
}

func (c *cwdDialogCmp) handleEnter() tea.Cmd {
	if len(c.dirs) == 0 {
		// No subdirs — confirm this directory as the new CWD
		return util.CmdHandler(SwitchCWDMsg{NewPath: c.currentPath})
	}
	if c.selectedIdx >= 0 && c.selectedIdx < len(c.dirs) {
		selected := c.dirs[c.selectedIdx]
		if selected == "[Switch Here]" {
			return util.CmdHandler(SwitchCWDMsg{NewPath: c.currentPath})
		}
		newPath := filepath.Join(c.currentPath, selected)
		info, err := os.Stat(newPath)
		if err == nil && info.IsDir() {
			c.currentPath = newPath
			c.refreshDirs()
		}
	}
	return nil
}

func (c *cwdDialogCmp) buildDirList(t theme.Theme, base lipgloss.Style, width int) []string {
	if len(c.dirs) == 0 {
		return []string{
			base.Foreground(t.TextMuted()).Width(width).Padding(0, 1).Render("(no subdirectories)"),
		}
	}

	visible := maxVisibleDirs
	if visible > len(c.dirs) {
		visible = len(c.dirs)
	}

	end := c.scrollOff + visible
	if end > len(c.dirs) {
		end = len(c.dirs)
	}

	items := make([]string, 0, end-c.scrollOff)
	for i := c.scrollOff; i < end; i++ {
		name := c.dirs[i]
		itemStyle := base.Width(width).Padding(0, 1)

		if i == c.selectedIdx {
			itemStyle = itemStyle.
				Background(t.Primary()).
				Foreground(t.Background()).
				Bold(true)
		}
		items = append(items, itemStyle.Render(name))
	}

	// Scroll indicators
	if c.scrollOff > 0 {
		indicator := base.Foreground(t.TextMuted()).Width(width).Padding(0, 1).Render("  ...")
		items = append([]string{indicator}, items...)
	}
	if end < len(c.dirs) {
		indicator := base.Foreground(t.TextMuted()).Width(width).Padding(0, 1).Render("  ...")
		items = append(items, indicator)
	}

	return items
}

// readSubDirs returns sorted, non-hidden subdirectory names under path.
func readSubDirs(path string) []string {
	entries, err := os.ReadDir(path)
	if err != nil {
		return nil
	}

	dirs := make([]string, 0, len(entries))
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		name := e.Name()
		if strings.HasPrefix(name, ".") {
			continue
		}
		dirs = append(dirs, name)
	}
	sort.Strings(dirs)

	// Prepend a "switch here" action item
	dirs = append([]string{"[Switch Here]"}, dirs...)
	return dirs
}
