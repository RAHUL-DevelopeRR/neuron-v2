package terminal

import (
	"fmt"
	"io"
	"strings"
	"sync"
	"sync/atomic"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/vt"
	"github.com/opencode-ai/opencode/internal/tui/styles"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

// termGenCounter is a global monotonic counter used to identify PTY
// generations. Each StartShell() call increments this, and all messages
// carry the generation so stale messages from previous shells are discarded.
var termGenCounter atomic.Uint64

const (
	// titleBarHeight is the height consumed by the title bar line.
	titleBarHeight = 1
	// defaultCols is the default number of columns if none specified.
	defaultCols = 80
	// defaultRows is the default number of rows if none specified.
	defaultRows = 24
	// readBufSize is the buffer size for PTY reads.
	readBufSize = 4096
	// scrollbackSize is the number of terminal lines retained for review.
	scrollbackSize = 10000
)

// Model is the Bubbletea model for the embedded terminal component.
// Embed this in a parent model and forward Update/View calls to it.
type Model struct {
	width        int
	height       int
	pty          *PTY
	emulator     *vt.Emulator
	mu           sync.Mutex
	started      bool
	focused      bool
	err          error
	cwd          string
	shell        string
	args         []string
	scrollOffset int
	gen          uint64 // current PTY generation (for stale message filtering)
}

// New creates a new terminal Model. The cwd parameter sets the working
// directory for the shell process.
func New(cwd, shell string, args []string) Model {
	return Model{
		width:    defaultCols,
		height:   defaultRows,
		emulator: newEmulator(defaultCols, defaultRows-titleBarHeight),
		cwd:      cwd,
		shell:    shell,
		args:     append([]string(nil), args...),
	}
}

func newEmulator(cols, rows int) *vt.Emulator {
	if cols < 1 {
		cols = 1
	}
	if rows < 1 {
		rows = 1
	}
	emulator := vt.NewEmulator(cols, rows)
	emulator.SetScrollbackSize(scrollbackSize)
	return emulator
}

// Init implements tea.Model. Returns nil; use StartShell() to begin.
func (m Model) Init() tea.Cmd {
	return nil
}

// StartShell spawns the PTY shell process and returns a tea.Cmd that
// begins reading output. Call this from the parent's Init or Update.
// Each call increments the generation counter so stale messages from
// previous shells are discarded.
func (m *Model) StartShell() tea.Cmd {
	m.mu.Lock()

	cols := m.width
	rows := m.height - titleBarHeight
	if cols < 1 {
		cols = defaultCols
	}
	if rows < 1 {
		rows = defaultRows
	}

	p, err := NewPTY(cols, rows, m.cwd, m.shell, m.args)
	if err != nil {
		m.err = err
		m.mu.Unlock()
		return nil
	}

	// Increment generation to invalidate any in-flight messages from old PTY
	gen := termGenCounter.Add(1)

	m.pty = p
	m.started = true
	m.err = nil
	m.gen = gen

	// MUST unlock before calling readPTY -- readPTY also acquires m.mu.
	// Using defer here would deadlock since Go's sync.Mutex is not reentrant.
	m.mu.Unlock()

	return m.readPTY(gen)
}

// Update handles incoming messages and mutates the live terminal model.
// Messages with a stale Gen are silently discarded to prevent a race
// where an old PTY's TerminalClosedMsg kills a newly started shell.
func (m *Model) Update(msg tea.Msg) tea.Cmd {
	switch msg := msg.(type) {
	case TerminalOutputMsg:
		// Discard output from a previous shell generation
		if msg.Gen != m.gen {
			return nil
		}
		m.mu.Lock()
		if m.emulator != nil {
			m.emulator.Write(msg.Data)
		}
		m.clampScrollLocked()
		m.mu.Unlock()
		return m.readPTY(m.gen)

	case TerminalFocusMsg:
		m.focused = msg.Focused
		if m.focused && !m.started && m.err == nil {
			return m.StartShell()
		}
		return nil

	case TerminalRefreshMsg:
		m.mu.Lock()
		if m.pty != nil {
			m.pty.Close()
			m.pty = nil
		}
		// Reset the emulator
		cols := m.width
		rows := m.height - titleBarHeight
		if cols < 1 {
			cols = defaultCols
		}
		if rows < 1 {
			rows = defaultRows
		}
		m.emulator = newEmulator(cols, rows)
		m.scrollOffset = 0
		m.started = false
		m.err = nil
		m.mu.Unlock()
		return m.StartShell()

	case TerminalClosedMsg:
		// Discard close from a previous shell generation
		if msg.Gen != m.gen {
			return nil
		}
		m.mu.Lock()
		m.started = false
		m.err = msg.Err
		m.mu.Unlock()
		return nil

	case tea.KeyMsg:
		if !m.focused {
			return nil
		}
		return m.handleKeyInput(msg)
	case tea.MouseMsg:
		switch msg.Type {
		case tea.MouseWheelUp:
			m.ScrollUp(3)
		case tea.MouseWheelDown:
			m.ScrollDown(3)
		}
	}

	return nil
}

// View renders the terminal panel as a styled string.
func (m *Model) View() string {
	t := theme.CurrentTheme()
	base := styles.BaseStyle().Width(m.width)

	// -- Title bar --
	titleBar := m.renderTitleBar(t)

	// -- Content area --
	contentHeight := m.height - titleBarHeight
	if contentHeight < 0 {
		contentHeight = 0
	}

	var content string
	switch {
	case m.err != nil && !m.started:
		errStyle := lipgloss.NewStyle().
			Foreground(t.Error()).
			Width(m.width)
		content = errStyle.Render(fmt.Sprintf("Error: %v", m.err))
	case !m.started:
		mutedStyle := lipgloss.NewStyle().
			Foreground(t.TextMuted()).
			Width(m.width)
		content = mutedStyle.Render("Shell idle")
	default:
		content = m.renderScreen(contentHeight)
	}

	return base.Render(lipgloss.JoinVertical(lipgloss.Left, titleBar, content))
}

// SetSize updates the terminal dimensions and resizes the PTY and emulator.
func (m *Model) SetSize(width, height int) {
	m.mu.Lock()
	defer m.mu.Unlock()

	m.width = width
	m.height = height

	cols := width
	rows := height - titleBarHeight
	if cols < 1 {
		cols = 1
	}
	if rows < 1 {
		rows = 1
	}

	if m.emulator != nil {
		m.emulator.Resize(cols, rows)
		m.emulator.SetScrollbackSize(scrollbackSize)
	}
	if m.pty != nil {
		_ = m.pty.Resize(cols, rows)
	}
	m.clampScrollLocked()
}

// SetFocus sets whether the terminal has keyboard focus.
func (m *Model) SetFocus(focused bool) {
	m.focused = focused
}

// IsFocused returns whether the terminal currently has keyboard focus.
func (m Model) IsFocused() bool {
	return m.focused
}

// Close cleans up the PTY process and resources.
func (m *Model) Close() {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.pty != nil {
		m.pty.Close()
		m.pty = nil
	}
	m.started = false
}

// WriteCommand writes a command string followed by a newline to the
// PTY stdin. This is a convenience method for programmatic input.
func (m *Model) WriteCommand(cmd string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.pty != nil {
		m.pty.Write([]byte(cmd + "\n"))
	}
}

// readPTY returns a tea.Cmd that performs a blocking read from the PTY.
// It captures the PTY pointer and generation at call time for goroutine safety.
// The generation is embedded in the resulting message so Update() can
// discard output from a previous shell.
func (m *Model) readPTY(gen uint64) tea.Cmd {
	m.mu.Lock()
	p := m.pty
	m.mu.Unlock()

	if p == nil {
		return nil
	}

	return func() tea.Msg {
		buf := make([]byte, readBufSize)
		n, err := p.Read(buf)
		if err != nil {
			if err == io.EOF {
				return TerminalClosedMsg{Err: nil, Gen: gen}
			}
			return TerminalClosedMsg{Err: err, Gen: gen}
		}
		data := make([]byte, n)
		copy(data, buf[:n])
		return TerminalOutputMsg{Data: data, Gen: gen}
	}
}

// handleKeyInput translates a Bubbletea KeyMsg into raw bytes and writes
// them to the PTY stdin.
func (m *Model) handleKeyInput(msg tea.KeyMsg) tea.Cmd {
	var data []byte

	switch msg.Type {
	case tea.KeyPgUp:
		m.ScrollUp(max(1, m.height-titleBarHeight-1))
		return nil
	case tea.KeyPgDown:
		m.ScrollDown(max(1, m.height-titleBarHeight-1))
		return nil
	case tea.KeyRunes:
		if msg.Alt {
			// Alt+key: send ESC prefix followed by the rune
			for _, r := range msg.Runes {
				buf := make([]byte, utf8.UTFMax)
				n := utf8.EncodeRune(buf, r)
				data = append(data, 0x1b)
				data = append(data, buf[:n]...)
			}
		} else {
			for _, r := range msg.Runes {
				buf := make([]byte, utf8.UTFMax)
				n := utf8.EncodeRune(buf, r)
				data = append(data, buf[:n]...)
			}
		}
	case tea.KeySpace:
		data = []byte(" ")
	case tea.KeyEnter:
		data = []byte("\r")
	case tea.KeyBackspace:
		data = []byte{0x7f}
	case tea.KeyTab:
		data = []byte("\t")
	case tea.KeyEscape:
		data = []byte{0x1b}
	case tea.KeyCtrlC:
		data = []byte{0x03}
	case tea.KeyCtrlD:
		data = []byte{0x04}
	case tea.KeyCtrlZ:
		data = []byte{0x1a}
	case tea.KeyCtrlL:
		data = []byte{0x0c}
	case tea.KeyCtrlA:
		data = []byte{0x01}
	case tea.KeyCtrlB:
		data = []byte{0x02}
	case tea.KeyCtrlE:
		data = []byte{0x05}
	case tea.KeyCtrlF:
		data = []byte{0x06}
	case tea.KeyCtrlG:
		data = []byte{0x07}
	case tea.KeyCtrlH:
		data = []byte{0x08}
	case tea.KeyCtrlJ:
		data = []byte{0x0a}
	case tea.KeyCtrlK:
		data = []byte{0x0b}
	case tea.KeyCtrlN:
		data = []byte{0x0e}
	case tea.KeyCtrlO:
		data = []byte{0x0f}
	case tea.KeyCtrlP:
		data = []byte{0x10}
	case tea.KeyCtrlQ:
		data = []byte{0x11}
	case tea.KeyCtrlR:
		data = []byte{0x12}
	case tea.KeyCtrlS:
		data = []byte{0x13}
	case tea.KeyCtrlT:
		data = []byte{0x14}
	case tea.KeyCtrlU:
		data = []byte{0x15}
	case tea.KeyCtrlV:
		data = []byte{0x16}
	case tea.KeyCtrlW:
		data = []byte{0x17}
	case tea.KeyCtrlX:
		data = []byte{0x18}
	case tea.KeyCtrlY:
		data = []byte{0x19}
	case tea.KeyUp:
		data = []byte("\x1b[A")
	case tea.KeyDown:
		data = []byte("\x1b[B")
	case tea.KeyRight:
		data = []byte("\x1b[C")
	case tea.KeyLeft:
		data = []byte("\x1b[D")
	case tea.KeyHome:
		data = []byte("\x1b[H")
	case tea.KeyEnd:
		data = []byte("\x1b[F")
	case tea.KeyDelete:
		data = []byte("\x1b[3~")
	default:
		return nil
	}

	if len(data) == 0 {
		return nil
	}

	m.mu.Lock()
	p := m.pty
	gen := m.gen
	m.scrollOffset = 0
	m.mu.Unlock()

	if p == nil {
		return nil
	}

	return func() tea.Msg {
		_, err := p.Write(data)
		if err != nil {
			return TerminalClosedMsg{Err: err, Gen: gen}
		}
		return nil
	}
}

// renderTitleBar builds the styled title bar string.
func (m *Model) renderTitleBar(t theme.Theme) string {
	titleColor := t.TextMuted()
	if m.focused {
		titleColor = t.Primary()
	}

	titleStyle := lipgloss.NewStyle().
		Foreground(titleColor).
		Bold(true)

	title := titleStyle.Render("TERMINAL")
	if m.focused {
		badgeStyle := lipgloss.NewStyle().
			Foreground(t.Success()).
			Bold(true)
		title += " " + badgeStyle.Render("[ACTIVE]")
	}

	helpStyle := lipgloss.NewStyle().
		Foreground(t.TextMuted())
	helpText := "ctrl+x terminal  pgup/pgdn scroll"
	if m.focused {
		helpText = "ctrl+x editor  pgup/pgdn scroll"
	}
	helpLabel := helpStyle.Render(helpText)

	// Calculate spacing between title and help label
	titleWidth := lipgloss.Width(title)
	helpWidth := lipgloss.Width(helpLabel)
	spacerWidth := m.width - titleWidth - helpWidth
	if spacerWidth < 1 {
		spacerWidth = 1
	}

	spacer := strings.Repeat(" ", spacerWidth)

	barStyle := lipgloss.NewStyle().
		Width(m.width).
		Background(t.BackgroundSecondary())

	return barStyle.Render(title + spacer + helpLabel)
}

// renderScreen extracts visible lines from the emulator and scrollback.
// scrollOffset is measured from the bottom: 0 follows the live terminal,
// larger values let the user review earlier output.
func (m *Model) renderScreen(maxHeight int) string {
	m.mu.Lock()
	allLines := m.terminalLinesLocked()
	scrollOffset := m.scrollOffset
	if scrollOffset > m.maxScrollLocked(maxHeight) {
		scrollOffset = m.maxScrollLocked(maxHeight)
	}
	m.mu.Unlock()

	if len(allLines) == 0 {
		return ""
	}

	totalLines := len(allLines)

	// Reserve 1 col for scrollbar when content overflows
	hasScrollbar := totalLines > maxHeight
	contentWidth := m.width
	if hasScrollbar {
		contentWidth = m.width - 1
		if contentWidth < 1 {
			contentWidth = 1
		}
	}

	maxStart := max(0, totalLines-maxHeight)
	start := max(0, maxStart-scrollOffset)
	end := min(totalLines, start+maxHeight)
	lines := append([]string(nil), allLines[start:end]...)

	// Truncate each line to fit content width
	for i, line := range lines {
		if lipgloss.Width(line) > contentWidth {
			runes := []rune(line)
			w := 0
			cut := len(runes)
			for j, r := range runes {
				w += runeWidth(r)
				if w > contentWidth {
					cut = j
					break
				}
			}
			lines[i] = string(runes[:cut])
		}
	}

	content := strings.Join(lines, "\n")

	if !hasScrollbar || maxHeight < 2 {
		return content
	}

	// Build scrollbar column
	scrollbar := renderScrollbar(maxHeight, totalLines, start)
	return lipgloss.JoinHorizontal(lipgloss.Top, content, scrollbar)
}

func (m *Model) ScrollUp(lines int) {
	if lines < 1 {
		lines = 1
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.scrollOffset += lines
	m.clampScrollLocked()
}

func (m *Model) ScrollDown(lines int) {
	if lines < 1 {
		lines = 1
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.scrollOffset -= lines
	m.clampScrollLocked()
}

func (m *Model) clampScrollLocked() {
	maxScroll := m.maxScrollLocked(m.height - titleBarHeight)
	if m.scrollOffset < 0 {
		m.scrollOffset = 0
	}
	if m.scrollOffset > maxScroll {
		m.scrollOffset = maxScroll
	}
}

func (m *Model) maxScrollLocked(maxHeight int) int {
	if maxHeight < 1 {
		maxHeight = 1
	}
	return max(0, len(m.terminalLinesLocked())-maxHeight)
}

func (m *Model) terminalLinesLocked() []string {
	if m.emulator == nil {
		return nil
	}

	lines := make([]string, 0, m.emulator.ScrollbackLen()+m.emulator.Height())
	if !m.emulator.IsAltScreen() {
		if scrollback := m.emulator.Scrollback(); scrollback != nil {
			for _, line := range scrollback.Lines() {
				lines = append(lines, line.String())
			}
		}
	}

	if raw := m.emulator.String(); raw != "" {
		lines = append(lines, strings.Split(raw, "\n")...)
	}
	return lines
}

// renderScrollbar builds a single-column scrollbar string for the given
// viewport parameters. The thumb position and size are proportional to
// the visible area within the total content.
func renderScrollbar(viewHeight, totalLines, scrollOffset int) string {
	if viewHeight < 1 || totalLines <= viewHeight {
		return ""
	}

	// Calculate thumb size and position
	thumbSize := viewHeight * viewHeight / totalLines
	if thumbSize < 1 {
		thumbSize = 1
	}
	scrollRange := totalLines - viewHeight
	if scrollRange <= 0 {
		return ""
	}
	thumbPos := scrollOffset * (viewHeight - thumbSize) / scrollRange
	if thumbPos+thumbSize > viewHeight {
		thumbPos = viewHeight - thumbSize
	}

	trackChar := " "
	thumbChar := "┃"

	var sb strings.Builder
	for i := 0; i < viewHeight; i++ {
		if i > 0 {
			sb.WriteByte('\n')
		}
		if i >= thumbPos && i < thumbPos+thumbSize {
			sb.WriteString(thumbChar)
		} else {
			sb.WriteString(trackChar)
		}
	}
	return sb.String()
}

// runeWidth returns a simple character width estimate.
// For most Latin/ASCII characters this is 1; CJK characters are 2.
func runeWidth(r rune) int {
	if r >= 0x1100 &&
		(r <= 0x115f || r == 0x2329 || r == 0x232a ||
			(r >= 0x2e80 && r <= 0xa4cf && r != 0x303f) ||
			(r >= 0xac00 && r <= 0xd7a3) ||
			(r >= 0xf900 && r <= 0xfaff) ||
			(r >= 0xfe10 && r <= 0xfe19) ||
			(r >= 0xfe30 && r <= 0xfe6f) ||
			(r >= 0xff00 && r <= 0xff60) ||
			(r >= 0xffe0 && r <= 0xffe6) ||
			(r >= 0x20000 && r <= 0x2fffd) ||
			(r >= 0x30000 && r <= 0x3fffd)) {
		return 2
	}
	return 1
}
