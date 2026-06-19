// Standalone prototype: split TUI with CWD dialog, diff panel, and live terminal.
// Build: go build -o ptytest.exe ./cmd/ptytest
package main

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"sync/atomic"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/vt"
	"github.com/charmbracelet/x/xpty"
)

// ── Messages ──────────────────────────────────────────────────────────

type ptyOutputMsg struct {
	data []byte
	gen  uint64
}
type ptyClosedMsg struct {
	err error
	gen uint64
}

// ── Colors ────────────────────────────────────────────────────────────

var (
	colorPrimary = lipgloss.Color("#61AFEF")
	colorBorder  = lipgloss.Color("#44475A")
	colorMuted   = lipgloss.Color("#6C7086")
	colorText    = lipgloss.Color("#CDD6F4")
	colorErr     = lipgloss.Color("#FF5555")
	colorGreen   = lipgloss.Color("#A6E3A1")
	colorYellow  = lipgloss.Color("#F9E2AF")
	colorOverlay = lipgloss.Color("#313244")
	colorHighBg  = lipgloss.Color("#45475A")
)

// ── Layout constants ──────────────────────────────────────────────────

const (
	rightPanelPercent = 35
	minRightW         = 30
	diffPanelPercent  = 40
	statusBarH        = 1
)

// ── CWD Browser state ─────────────────────────────────────────────────

type cwdBrowser struct {
	open       bool
	input      string
	entries    []dirEntry
	selected   int
	errMsg     string
}

type dirEntry struct {
	name  string
	isDir bool
}

func (b *cwdBrowser) listDir(path string) {
	b.entries = nil
	b.selected = 0
	b.errMsg = ""

	info, err := os.Stat(path)
	if err != nil {
		b.errMsg = "Path not found"
		return
	}
	if !info.IsDir() {
		path = filepath.Dir(path)
	}

	entries, err := os.ReadDir(path)
	if err != nil {
		b.errMsg = fmt.Sprintf("Cannot read: %v", err)
		return
	}

	// Add parent directory
	b.entries = append(b.entries, dirEntry{name: "..", isDir: true})

	// Directories first, then files
	var dirs, files []dirEntry
	for _, e := range entries {
		if strings.HasPrefix(e.Name(), ".") {
			continue // skip hidden
		}
		de := dirEntry{name: e.Name(), isDir: e.IsDir()}
		if e.IsDir() {
			dirs = append(dirs, de)
		} else {
			files = append(files, de)
		}
	}
	sort.Slice(dirs, func(i, j int) bool { return dirs[i].name < dirs[j].name })
	sort.Slice(files, func(i, j int) bool { return files[i].name < files[j].name })
	b.entries = append(b.entries, dirs...)
	b.entries = append(b.entries, files...)
}

func (b *cwdBrowser) currentDir() string {
	path := b.input
	info, err := os.Stat(path)
	if err != nil {
		return filepath.Dir(path)
	}
	if info.IsDir() {
		return path
	}
	return filepath.Dir(path)
}

// ── Model ─────────────────────────────────────────────────────────────

type model struct {
	width, height int

	// PTY
	pty      xpty.Pty
	cmd      *exec.Cmd
	emulator *vt.Emulator
	mu       sync.Mutex
	started  bool
	ptyErr   error
	ptyGen   uint64

	// State
	termFocused bool
	cwd         string
	browser     cwdBrowser

	// REPL
	replInput     string
	replHistory   []chatMsg
	replOutput    []replEntry // rendered chat log
	replStreaming  bool
	replStreamBuf string
	replToken     string
	replModelIdx  int // index into availableModels
	replScroll    int // scroll offset for chat log
	replStreamCh  <-chan tea.Msg

	// Cached layout dimensions
	leftW, rightW          int
	diffH, termH           int
	termInnerW, termInnerH int
}

type replEntry struct {
	role string // "user" or "assistant"
	text string
}

var genCounter atomic.Uint64

func newModel() *model {
	cwd, _ := os.Getwd()
	token, _ := getSessionToken()
	return &model{
		cwd:         cwd,
		replToken:   token,
		replModelIdx: defaultModelIndex,
		replHistory: []chatMsg{
			{Role: "system", Content: "You are a helpful coding assistant. Give concise, direct answers. When showing code, use markdown code blocks."},
		},
	}
}

func (m *model) Init() tea.Cmd {
	return m.startShell()
}

// ── Layout calculation ────────────────────────────────────────────────

func (m *model) recalcLayout() {
	m.rightW = m.width * rightPanelPercent / 100
	if m.rightW < minRightW {
		m.rightW = minRightW
	}
	if m.rightW > m.width-20 {
		m.rightW = m.width - 20
	}
	m.leftW = m.width - m.rightW

	bodyH := m.height - statusBarH
	m.diffH = bodyH * diffPanelPercent / 100
	if m.diffH < 5 {
		m.diffH = 5
	}
	m.termH = bodyH - m.diffH
	if m.termH < 5 {
		m.termH = 5
	}

	m.termInnerW = m.rightW - 4
	m.termInnerH = m.termH - 3
	if m.termInnerW < 5 {
		m.termInnerW = 5
	}
	if m.termInnerH < 2 {
		m.termInnerH = 2
	}
}

// ── PTY ───────────────────────────────────────────────────────────────

func (m *model) startShell() tea.Cmd {
	cols := 60
	rows := 15
	if m.termInnerW > 0 {
		cols = m.termInnerW
	}
	if m.termInnerH > 0 {
		rows = m.termInnerH
	}

	p, err := xpty.NewPty(cols, rows)
	if err != nil {
		m.ptyErr = fmt.Errorf("xpty.NewPty: %w", err)
		return nil
	}

	shell := defaultShell()
	cmd := exec.Command(shell)
	cmd.Dir = m.cwd
	cmd.Env = os.Environ()

	if err := p.Start(cmd); err != nil {
		p.Close()
		m.ptyErr = fmt.Errorf("pty.Start(%s): %w", shell, err)
		return nil
	}

	// New generation — all messages from previous shells will be ignored
	gen := genCounter.Add(1)

	m.mu.Lock()
	m.pty = p
	m.cmd = cmd
	m.emulator = vt.NewEmulator(cols, rows)
	m.started = true
	m.ptyErr = nil
	m.ptyGen = gen
	m.mu.Unlock()

	return m.readPTY(gen)
}

func (m *model) readPTY(gen uint64) tea.Cmd {
	m.mu.Lock()
	p := m.pty
	m.mu.Unlock()
	if p == nil {
		return nil
	}
	return func() tea.Msg {
		buf := make([]byte, 4096)
		n, err := p.Read(buf)
		if err != nil {
			if err == io.EOF {
				return ptyClosedMsg{gen: gen}
			}
			return ptyClosedMsg{err: err, gen: gen}
		}
		out := make([]byte, n)
		copy(out, buf[:n])
		return ptyOutputMsg{data: out, gen: gen}
	}
}

func (m *model) resizePTY() {
	m.mu.Lock()
	defer m.mu.Unlock()
	cols := m.termInnerW
	rows := m.termInnerH
	if cols < 1 {
		cols = 1
	}
	if rows < 1 {
		rows = 1
	}
	if m.emulator != nil {
		m.emulator.Resize(cols, rows)
	}
	if m.pty != nil {
		_ = m.pty.Resize(cols, rows)
	}
}

// ── Update ────────────────────────────────────────────────────────────

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		m.recalcLayout()
		m.resizePTY()
		return m, nil

	case ptyOutputMsg:
		// Ignore messages from old shell generations
		if msg.gen != m.ptyGen {
			return m, nil
		}
		m.mu.Lock()
		if m.emulator != nil {
			m.emulator.Write(msg.data)
		}
		m.mu.Unlock()
		return m, m.readPTY(msg.gen)

	case ptyClosedMsg:
		// Ignore messages from old shell generations
		if msg.gen != m.ptyGen {
			return m, nil
		}
		m.mu.Lock()
		m.started = false
		m.ptyErr = msg.err
		m.mu.Unlock()
		return m, nil

	case replStreamChunk:
		m.replStreamBuf += msg.text
		return m, m.waitForStream()

	case replStreamDone:
		m.replStreaming = false
		if msg.err != nil {
			m.replOutput = append(m.replOutput, replEntry{
				role: "error",
				text: fmt.Sprintf("Error: %v", msg.err),
			})
		} else {
			m.replOutput = append(m.replOutput, replEntry{
				role: "assistant",
				text: m.replStreamBuf,
			})
			m.replHistory = append(m.replHistory, chatMsg{
				Role:    "assistant",
				Content: m.replStreamBuf,
			})
		}
		m.replStreamBuf = ""
		m.replStreamCh = nil
		return m, nil

	case tea.KeyMsg:
		// CWD browser takes priority when open
		if m.browser.open {
			return m, m.handleBrowserKey(msg)
		}

		switch msg.String() {
		case "ctrl+c":
			m.cleanup()
			return m, tea.Quit
		case "ctrl+x":
			m.termFocused = !m.termFocused
			return m, nil
		case "ctrl+g":
			m.browser.open = true
			m.browser.input = m.cwd
			m.browser.listDir(m.cwd)
			return m, nil
		case "ctrl+m":
			// Cycle to next model
			m.replModelIdx = (m.replModelIdx + 1) % len(availableModels)
			return m, nil
		}

		if m.termFocused {
			return m, m.sendKey(msg)
		}

		// REPL input handling (left panel, not streaming)
		if !m.replStreaming {
			return m, m.handleREPLKey(msg)
		}
		return m, nil
	}
	return m, nil
}

func (m *model) handleBrowserKey(msg tea.KeyMsg) tea.Cmd {
	switch msg.Type {
	case tea.KeyEscape:
		m.browser.open = false
		return nil

	case tea.KeyEnter:
		// Enter = SELECT. If a folder is highlighted, use THAT folder as CWD.
		// If a file is highlighted, use the current listed directory as CWD.
		if len(m.browser.entries) > 0 && m.browser.selected < len(m.browser.entries) {
			entry := m.browser.entries[m.browser.selected]
			if entry.isDir && entry.name != ".." {
				// Select this specific folder as CWD
				dir := filepath.Join(m.browser.currentDir(), entry.name)
				absDir, err := filepath.Abs(dir)
				if err == nil {
					m.browser.input = absDir
				}
			} else if entry.name == ".." {
				// Select the parent as CWD
				m.browser.input = filepath.Dir(m.browser.currentDir())
			}
			// For files, just confirm the current directory
		}
		return m.confirmCWD()

	case tea.KeyRight:
		// Right arrow = DRILL INTO a directory (browse its contents)
		if len(m.browser.entries) > 0 && m.browser.selected < len(m.browser.entries) {
			entry := m.browser.entries[m.browser.selected]
			if entry.isDir {
				dir := m.browser.currentDir()
				if entry.name == ".." {
					dir = filepath.Dir(dir)
				} else {
					dir = filepath.Join(dir, entry.name)
				}
				absDir, err := filepath.Abs(dir)
				if err == nil {
					m.browser.input = absDir
					m.browser.listDir(absDir)
				}
			}
		}
		return nil

	case tea.KeyLeft:
		// Left arrow = go to parent directory
		parent := filepath.Dir(m.browser.currentDir())
		absDir, err := filepath.Abs(parent)
		if err == nil {
			m.browser.input = absDir
			m.browser.listDir(absDir)
		}
		return nil

	case tea.KeyUp:
		if m.browser.selected > 0 {
			m.browser.selected--
		}
		return nil

	case tea.KeyDown:
		if m.browser.selected < len(m.browser.entries)-1 {
			m.browser.selected++
		}
		return nil

	case tea.KeyBackspace:
		if len(m.browser.input) > 0 {
			m.browser.input = m.browser.input[:len(m.browser.input)-1]
			m.browser.listDir(m.browser.input)
		}
		return nil

	case tea.KeyRunes:
		m.browser.input += string(msg.Runes)
		m.browser.listDir(m.browser.input)
		return nil

	case tea.KeySpace:
		m.browser.input += " "
		m.browser.listDir(m.browser.input)
		return nil
	}
	return nil
}

func (m *model) confirmCWD() tea.Cmd {
	path := strings.TrimSpace(m.browser.input)
	path = strings.Trim(path, `"'`)
	absPath, err := filepath.Abs(path)
	if err != nil {
		m.browser.errMsg = fmt.Sprintf("Invalid path: %v", err)
		return nil
	}
	info, err := os.Stat(absPath)
	if err != nil || !info.IsDir() {
		m.browser.errMsg = "Not a valid directory"
		return nil
	}
	m.cwd = absPath
	m.browser.open = false
	// Restart shell in new directory
	m.cleanup()
	return m.startShell()
}

// ── REPL methods ──────────────────────────────────────────────────────

func (m *model) handleREPLKey(msg tea.KeyMsg) tea.Cmd {
	switch msg.Type {
	case tea.KeyEnter:
		input := strings.TrimSpace(m.replInput)
		if input == "" {
			return nil
		}
		return m.sendREPL(input)
	case tea.KeyBackspace:
		if len(m.replInput) > 0 {
			m.replInput = m.replInput[:len(m.replInput)-1]
		}
		return nil
	case tea.KeyRunes:
		m.replInput += string(msg.Runes)
		return nil
	case tea.KeySpace:
		m.replInput += " "
		return nil
	}
	return nil
}

func (m *model) sendREPL(input string) tea.Cmd {
	// Auto-retry session if missing
	if m.replToken == "" {
		token, err := getSessionToken()
		if err != nil || token == "" {
			m.replOutput = append(m.replOutput, replEntry{
				role: "error",
				text: "Session fetch failed. Check gateway at " + gatewayURL(),
			})
			return nil
		}
		m.replToken = token
	}

	// Add user message to output and history
	m.replOutput = append(m.replOutput, replEntry{role: "user", text: input})
	m.replHistory = append(m.replHistory, chatMsg{Role: "user", Content: input})
	m.replInput = ""
	m.replStreaming = true
	m.replStreamBuf = ""

	// Start streaming
	m.replStreamCh = streamChatCompletion(m.replToken, m.replHistory, availableModels[m.replModelIdx].apiModel)
	return m.waitForStream()
}

func (m *model) waitForStream() tea.Cmd {
	ch := m.replStreamCh
	if ch == nil {
		return nil
	}
	return func() tea.Msg {
		msg, ok := <-ch
		if !ok {
			return replStreamDone{fullText: m.replStreamBuf}
		}
		if msg == nil {
			return replStreamDone{fullText: m.replStreamBuf}
		}
		return msg
	}
}

func (m *model) sendKey(msg tea.KeyMsg) tea.Cmd {
	var data []byte
	switch msg.Type {
	case tea.KeyRunes:
		data = []byte(string(msg.Runes))
	case tea.KeyEnter:
		data = []byte("\r")
	case tea.KeyBackspace:
		data = []byte{0x7f}
	case tea.KeyTab:
		data = []byte("\t")
	case tea.KeySpace:
		data = []byte(" ")
	case tea.KeyEscape:
		data = []byte{0x1b}
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
	case tea.KeyCtrlA:
		data = []byte{0x01}
	case tea.KeyCtrlB:
		data = []byte{0x02}
	case tea.KeyCtrlD:
		data = []byte{0x04}
	case tea.KeyCtrlE:
		data = []byte{0x05}
	case tea.KeyCtrlK:
		data = []byte{0x0b}
	case tea.KeyCtrlL:
		data = []byte{0x0c}
	case tea.KeyCtrlU:
		data = []byte{0x15}
	case tea.KeyCtrlW:
		data = []byte{0x17}
	default:
		return nil
	}
	if len(data) == 0 {
		return nil
	}
	m.mu.Lock()
	p := m.pty
	m.mu.Unlock()
	if p == nil {
		return nil
	}
	return func() tea.Msg {
		_, err := p.Write(data)
		if err != nil {
			return ptyClosedMsg{err: err, gen: m.ptyGen}
		}
		return nil
	}
}

// ── View ──────────────────────────────────────────────────────────────

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
				out := make([]rune, 0, len(runes))
				w := 0
				for _, r := range runes {
					w++
					if w > cols {
						break
					}
					out = append(out, r)
				}
				result[i] = string(out)
			} else {
				result[i] = line
			}
		} else {
			result[i] = ""
		}
	}
	return strings.Join(result, "\n")
}

func (m *model) View() string {
	if m.width < 10 || m.height < 5 {
		return "Terminal too small"
	}

	m.recalcLayout()

	bodyH := m.height - statusBarH

	leftPanel := m.renderLeftPanel(m.leftW, bodyH)
	leftPanel = clipContent(leftPanel, m.leftW, bodyH)

	rightTop := m.renderDiffPanel(m.rightW, m.diffH)
	rightBottom := m.renderTerminal(m.rightW, m.termH)
	rightCol := lipgloss.JoinVertical(lipgloss.Left, rightTop, rightBottom)
	rightCol = clipContent(rightCol, m.rightW, bodyH)

	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, rightCol)
	statusBar := m.renderStatusBar()

	screen := lipgloss.JoinVertical(lipgloss.Left, body, statusBar)

	// Overlay the CWD browser dialog if open
	if m.browser.open {
		dialog := m.renderBrowserDialog()
		screen = m.overlayDialog(screen, dialog)
	}

	return screen
}

func (m *model) renderLeftPanel(w, h int) string {
	// Banner (compact)
	banner := lipgloss.NewStyle().
		Foreground(colorPrimary).
		Bold(true).
		Render(" NEURON REPL")

	// CWD + model
	maxPathW := w - 30
	if maxPathW < 10 {
		maxPathW = 10
	}
	pathStr := m.cwd
	if len(pathStr) > maxPathW {
		pathStr = "..." + pathStr[len(pathStr)-maxPathW+3:]
	}
	cwdLine := lipgloss.NewStyle().Foreground(colorMuted).Render(" cwd: ") +
		lipgloss.NewStyle().Foreground(colorText).Render(pathStr) +
		lipgloss.NewStyle().Foreground(colorYellow).Bold(true).Render("  [ctrl+g]")

	modelLine := lipgloss.NewStyle().Foreground(colorMuted).Render(" model: ") +
		lipgloss.NewStyle().Foreground(colorGreen).Render(availableModels[m.replModelIdx].displayName+"  [ctrl+m]")

	// Token status
	tokenStatus := ""
	if m.replToken == "" {
		tokenStatus = lipgloss.NewStyle().Foreground(colorErr).Render(" [NO SESSION]")
	}

	header := lipgloss.JoinVertical(lipgloss.Left, banner+tokenStatus, cwdLine, modelLine)
	headerH := strings.Count(header, "\n") + 1

	// Input area (bottom)
	promptPrefix := " > "
	if m.replStreaming {
		promptPrefix = " ... "
	}
	inputText := m.replInput + "_"
	maxInputW := w - len(promptPrefix) - 2
	if maxInputW < 5 {
		maxInputW = 5
	}
	if len(inputText) > maxInputW {
		inputText = inputText[len(inputText)-maxInputW:]
	}
	promptLabel := lipgloss.NewStyle().Foreground(colorGreen).Bold(true).Render(promptPrefix)
	promptInput := lipgloss.NewStyle().Foreground(colorText).Render(inputText)
	inputLine := promptLabel + promptInput
	inputH := 2 // input line + separator

	// Chat log area (middle)
	chatH := h - headerH - inputH - 2 // 2 for separators/spacing
	if chatH < 3 {
		chatH = 3
	}
	chatW := w - 2
	if chatW < 5 {
		chatW = 5
	}

	var chatLines []string
	for _, entry := range m.replOutput {
		switch entry.role {
		case "user":
			prefix := lipgloss.NewStyle().Foreground(colorPrimary).Bold(true).Render(" You: ")
			text := lipgloss.NewStyle().Foreground(colorText).Render(entry.text)
			chatLines = append(chatLines, prefix+text)
		case "assistant":
			prefix := lipgloss.NewStyle().Foreground(colorGreen).Bold(true).Render(" AI: ")
			// Word-wrap the response
			wrapped := wrapText(entry.text, chatW-6)
			for i, line := range strings.Split(wrapped, "\n") {
				if i == 0 {
					chatLines = append(chatLines, prefix+lipgloss.NewStyle().Foreground(colorText).Render(line))
				} else {
					chatLines = append(chatLines, "      "+lipgloss.NewStyle().Foreground(colorText).Render(line))
				}
			}
		case "error":
			chatLines = append(chatLines, lipgloss.NewStyle().Foreground(colorErr).Render(" ERR: "+entry.text))
		}
		chatLines = append(chatLines, "") // spacing
	}

	// Show streaming buffer
	if m.replStreaming && m.replStreamBuf != "" {
		prefix := lipgloss.NewStyle().Foreground(colorGreen).Bold(true).Render(" AI: ")
		wrapped := wrapText(m.replStreamBuf, chatW-6)
		for i, line := range strings.Split(wrapped, "\n") {
			if i == 0 {
				chatLines = append(chatLines, prefix+lipgloss.NewStyle().Foreground(colorText).Render(line))
			} else {
				chatLines = append(chatLines, "      "+lipgloss.NewStyle().Foreground(colorText).Render(line))
			}
		}
		chatLines = append(chatLines, lipgloss.NewStyle().Foreground(colorYellow).Render("      ..."))
	}

	// Show empty state
	if len(m.replOutput) == 0 && !m.replStreaming {
		chatLines = append(chatLines,
			"",
			lipgloss.NewStyle().Foreground(colorMuted).Render("  Type a message and press Enter to chat."),
			lipgloss.NewStyle().Foreground(colorMuted).Render("  ctrl+x to switch to terminal."),
		)
	}

	// Auto-scroll to bottom
	if len(chatLines) > chatH {
		chatLines = chatLines[len(chatLines)-chatH:]
	}
	chatContent := strings.Join(chatLines, "\n")

	// Separator
	sep := lipgloss.NewStyle().Foreground(colorBorder).Render(strings.Repeat("─", w-2))

	content := lipgloss.JoinVertical(lipgloss.Left,
		header, sep, chatContent, sep, inputLine)

	return lipgloss.NewStyle().
		Width(w).Height(h).MaxWidth(w).MaxHeight(h).
		Render(content)
}

// wrapText does simple word-wrapping at the given width.
func wrapText(text string, width int) string {
	if width < 5 {
		width = 5
	}
	var result strings.Builder
	for _, paragraph := range strings.Split(text, "\n") {
		if result.Len() > 0 {
			result.WriteByte('\n')
		}
		words := strings.Fields(paragraph)
		lineLen := 0
		for i, word := range words {
			wLen := len(word)
			if i > 0 && lineLen+1+wLen > width {
				result.WriteByte('\n')
				lineLen = 0
			} else if i > 0 {
				result.WriteByte(' ')
				lineLen++
			}
			result.WriteString(word)
			lineLen += wLen
		}
	}
	return result.String()
}

func (m *model) renderDiffPanel(w, h int) string {
	innerW := w - 2
	innerH := h - 2
	if innerW < 3 {
		innerW = 3
	}
	if innerH < 1 {
		innerH = 1
	}

	title := lipgloss.NewStyle().Foreground(colorPrimary).Bold(true).Render("CHANGES")
	body := lipgloss.NewStyle().Foreground(colorMuted).Render("  No file changes yet")
	content := lipgloss.JoinVertical(lipgloss.Left, title, "", body)
	content = clipContent(content, innerW-2, innerH)

	return lipgloss.NewStyle().
		Width(innerW).Height(innerH).MaxWidth(innerW).MaxHeight(innerH).
		Border(lipgloss.RoundedBorder()).BorderForeground(colorBorder).
		Padding(0, 1).
		Render(content)
}

func (m *model) renderTerminal(w, h int) string {
	innerW := w - 2
	innerH := h - 2
	if innerW < 3 {
		innerW = 3
	}
	if innerH < 1 {
		innerH = 1
	}
	contentW := innerW - 2
	contentH := innerH - 1
	if contentW < 3 {
		contentW = 3
	}
	if contentH < 1 {
		contentH = 1
	}

	titleColor := colorMuted
	badge := ""
	if m.termFocused {
		titleColor = colorGreen
		badge = lipgloss.NewStyle().Foreground(colorGreen).Bold(true).Render(" [ACTIVE]")
	}
	title := lipgloss.NewStyle().Foreground(titleColor).Bold(true).Render("TERMINAL") + badge

	var screen string
	if m.ptyErr != nil {
		screen = lipgloss.NewStyle().Foreground(colorErr).
			Render(fmt.Sprintf("Error: %v", m.ptyErr))
	} else if !m.started {
		screen = lipgloss.NewStyle().Foreground(colorMuted).Render("Shell starting...")
	} else {
		m.mu.Lock()
		if m.emulator != nil {
			screen = m.emulator.String()
		}
		m.mu.Unlock()
	}
	screen = clipContent(screen, contentW, contentH)

	content := lipgloss.JoinVertical(lipgloss.Left, title, screen)

	borderColor := colorBorder
	if m.termFocused {
		borderColor = colorGreen
	}

	return lipgloss.NewStyle().
		Width(innerW).Height(innerH).MaxWidth(innerW).MaxHeight(innerH).
		Border(lipgloss.RoundedBorder()).BorderForeground(borderColor).
		Padding(0, 1).
		Render(content)
}

func (m *model) renderStatusBar() string {
	left := lipgloss.NewStyle().Foreground(colorMuted).Render(" ctrl+? help")
	focusLabel := "editor"
	if m.termFocused {
		focusLabel = "terminal"
	}
	right := lipgloss.NewStyle().Foreground(colorYellow).Bold(true).
		Render(fmt.Sprintf("Focus: %s ", focusLabel))
	spacer := m.width - lipgloss.Width(left) - lipgloss.Width(right)
	if spacer < 0 {
		spacer = 0
	}
	return lipgloss.NewStyle().Width(m.width).Background(lipgloss.Color("#313244")).
		Render(left + strings.Repeat(" ", spacer) + right)
}

// ── CWD Browser Dialog ───────────────────────────────────────────────

func (m *model) renderBrowserDialog() string {
	dialogW := m.width * 60 / 100
	if dialogW < 40 {
		dialogW = 40
	}
	if dialogW > m.width-4 {
		dialogW = m.width - 4
	}
	dialogH := m.height * 60 / 100
	if dialogH < 12 {
		dialogH = 12
	}
	if dialogH > m.height-4 {
		dialogH = m.height - 4
	}

	innerW := dialogW - 4 // border + padding

	// Title
	title := lipgloss.NewStyle().Foreground(colorPrimary).Bold(true).
		Render("Change Working Directory")

	// Input field
	inputLabel := lipgloss.NewStyle().Foreground(colorMuted).Render("Path: ")
	inputText := m.browser.input + "_"
	if lipgloss.Width(inputText) > innerW-8 {
		// Show tail
		runes := []rune(inputText)
		maxR := innerW - 8
		if maxR > 0 && len(runes) > maxR {
			inputText = "..." + string(runes[len(runes)-maxR:])
		}
	}
	inputField := lipgloss.NewStyle().
		Foreground(colorText).
		Background(colorOverlay).
		Width(innerW - 6).
		Render(inputText)
	inputLine := inputLabel + inputField

	// Error message
	var errLine string
	if m.browser.errMsg != "" {
		errLine = lipgloss.NewStyle().Foreground(colorErr).Render("  " + m.browser.errMsg)
	}

	// Directory listing
	separator := lipgloss.NewStyle().Foreground(colorBorder).
		Render(strings.Repeat("─", innerW))

	listH := dialogH - 8 // title + input + separator + hints
	if listH < 3 {
		listH = 3
	}

	var listLines []string
	for i, entry := range m.browser.entries {
		if len(listLines) >= listH {
			break
		}
		icon := "  "
		if entry.isDir {
			icon = " /"
		}
		name := entry.name
		if lipgloss.Width(name) > innerW-6 {
			name = name[:innerW-9] + "..."
		}
		line := icon + " " + name

		if i == m.browser.selected {
			line = lipgloss.NewStyle().
				Foreground(colorPrimary).
				Background(colorHighBg).
				Bold(true).
				Width(innerW).
				Render(line)
		} else {
			nameColor := colorText
			if entry.isDir {
				nameColor = colorPrimary
			}
			line = lipgloss.NewStyle().
				Foreground(nameColor).
				Width(innerW).
				Render(line)
		}
		listLines = append(listLines, line)
	}
	listing := strings.Join(listLines, "\n")

	// Hints
	hints := lipgloss.NewStyle().Foreground(colorMuted).
		Render("  Up/Down: select  Enter: use folder  Right: browse in  Left: parent  Esc: cancel")

	// Assemble
	parts := []string{title, "", inputLine}
	if errLine != "" {
		parts = append(parts, errLine)
	}
	parts = append(parts, separator, listing, "", hints)
	content := lipgloss.JoinVertical(lipgloss.Left, parts...)

	return lipgloss.NewStyle().
		Width(dialogW).
		Height(dialogH).
		MaxWidth(dialogW).
		MaxHeight(dialogH).
		Border(lipgloss.DoubleBorder()).
		BorderForeground(colorPrimary).
		Background(lipgloss.Color("#181825")).
		Padding(1, 1).
		Render(content)
}

func (m *model) overlayDialog(bg, dialog string) string {
	bgLines := strings.Split(bg, "\n")
	dialogLines := strings.Split(dialog, "\n")

	dialogW := lipgloss.Width(dialog)
	dialogH := len(dialogLines)

	// Center the dialog
	startX := (m.width - dialogW) / 2
	startY := (m.height - dialogH) / 2
	if startX < 0 {
		startX = 0
	}
	if startY < 0 {
		startY = 0
	}

	// Ensure bg has enough lines
	for len(bgLines) < m.height {
		bgLines = append(bgLines, strings.Repeat(" ", m.width))
	}

	// Overlay
	for i, dLine := range dialogLines {
		y := startY + i
		if y >= len(bgLines) {
			break
		}
		bgLine := bgLines[y]
		bgRunes := []rune(bgLine)

		// Pad bg line to full width
		for len(bgRunes) < m.width {
			bgRunes = append(bgRunes, ' ')
		}

		dRunes := []rune(dLine)
		endX := startX + len(dRunes)
		if endX > m.width {
			endX = m.width
			dRunes = dRunes[:m.width-startX]
		}

		result := make([]rune, 0, m.width)
		result = append(result, bgRunes[:startX]...)
		result = append(result, dRunes...)
		if endX < m.width {
			result = append(result, bgRunes[endX:]...)
		}
		bgLines[y] = string(result)
	}

	return strings.Join(bgLines[:m.height], "\n")
}

// ── Cleanup ───────────────────────────────────────────────────────────

func (m *model) cleanup() {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.cmd != nil && m.cmd.Process != nil {
		m.cmd.Process.Kill()
	}
	if m.pty != nil {
		m.pty.Close()
		m.pty = nil
	}
	m.started = false
}

func defaultShell() string {
	if runtime.GOOS == "windows" {
		if ps, err := exec.LookPath("pwsh.exe"); err == nil {
			return ps
		}
		return "powershell.exe"
	}
	if shell := os.Getenv("SHELL"); shell != "" {
		return shell
	}
	return "/bin/sh"
}

func main() {
	p := tea.NewProgram(newModel(), tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
}
