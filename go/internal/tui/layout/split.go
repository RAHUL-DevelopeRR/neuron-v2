package layout

import (
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

type SplitPaneLayout interface {
	tea.Model
	Sizeable
	Bindings
	SetLeftPanel(panel Container) tea.Cmd
	SetRightPanel(panel Container) tea.Cmd
	SetBottomPanel(panel Container) tea.Cmd
	ResizeHorizontal(delta float64) tea.Cmd
	ResizeVertical(delta float64) tea.Cmd

	ClearLeftPanel() tea.Cmd
	ClearRightPanel() tea.Cmd
	ClearBottomPanel() tea.Cmd
}

type splitPaneLayout struct {
	width         int
	height        int
	ratio         float64
	verticalRatio float64
	dragging      bool
	draggingStack bool

	rightPanel  Container
	leftPanel   Container
	bottomPanel Container
}

type SplitPaneOption func(*splitPaneLayout)

func (s *splitPaneLayout) Init() tea.Cmd {
	var cmds []tea.Cmd

	if s.leftPanel != nil {
		cmds = append(cmds, s.leftPanel.Init())
	}

	if s.rightPanel != nil {
		cmds = append(cmds, s.rightPanel.Init())
	}

	if s.bottomPanel != nil {
		cmds = append(cmds, s.bottomPanel.Init())
	}

	return tea.Batch(cmds...)
}

func (s *splitPaneLayout) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	if mouseMsg, ok := msg.(tea.MouseMsg); ok {
		if handled, cmd := s.handleMouse(mouseMsg); handled {
			return s, cmd
		}
		return s, s.routeMouse(mouseMsg)
	}

	if _, ok := msg.(tea.KeyMsg); ok && s.rightPanel != nil {
		if capturer, ok := s.rightPanel.(KeyCapturer); ok && capturer.CapturesKeys() {
			u, cmd := s.rightPanel.Update(msg)
			s.rightPanel = u.(Container)
			return s, cmd
		}
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		return s, s.SetSize(msg.Width, msg.Height)
	}

	if s.rightPanel != nil {
		u, cmd := s.rightPanel.Update(msg)
		s.rightPanel = u.(Container)
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	if s.leftPanel != nil {
		u, cmd := s.leftPanel.Update(msg)
		s.leftPanel = u.(Container)
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	if s.bottomPanel != nil {
		u, cmd := s.bottomPanel.Update(msg)
		s.bottomPanel = u.(Container)
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	return s, tea.Batch(cmds...)
}

func (s *splitPaneLayout) handleMouse(msg tea.MouseMsg) (bool, tea.Cmd) {
	if s.width <= 0 || s.height <= 0 {
		return false, nil
	}

	topHeight, _ := s.splitHeights()
	leftWidth, _ := s.splitWidths()

	switch msg.Action {
	case tea.MouseActionPress:
		if msg.Button != tea.MouseButtonLeft {
			return false, nil
		}
		if s.leftPanel != nil && s.rightPanel != nil && msg.Y < topHeight && msg.X >= leftWidth-1 && msg.X <= leftWidth+1 {
			s.dragging = true
			return true, nil
		}
		if s.bottomPanel != nil && msg.Y >= topHeight-1 && msg.Y <= topHeight+1 {
			s.draggingStack = true
			return true, nil
		}
	case tea.MouseActionMotion:
		if s.dragging {
			if s.width <= 1 {
				return true, nil
			}
			s.ratio = clampFloat(float64(msg.X)/float64(s.width-1), 0.20, 0.85)
			return true, s.SetSize(s.width, s.height)
		}
		if s.draggingStack {
			if s.height <= 1 {
				return true, nil
			}
			s.verticalRatio = clampFloat(float64(msg.Y)/float64(s.height), 0.45, 0.94)
			return true, s.SetSize(s.width, s.height)
		}
	case tea.MouseActionRelease:
		if s.dragging || s.draggingStack {
			s.dragging = false
			s.draggingStack = false
			return true, nil
		}
	}

	return false, nil
}

func (s *splitPaneLayout) routeMouse(msg tea.MouseMsg) tea.Cmd {
	topHeight, _ := s.splitHeights()
	leftWidth, _ := s.splitWidths()

	if s.bottomPanel != nil && msg.Y >= topHeight {
		local := msg
		local.Y -= topHeight
		u, cmd := s.bottomPanel.Update(local)
		s.bottomPanel = u.(Container)
		return cmd
	}

	if s.leftPanel != nil && s.rightPanel != nil {
		if msg.X < leftWidth {
			u, cmd := s.leftPanel.Update(msg)
			s.leftPanel = u.(Container)
			return cmd
		}
		if msg.X > leftWidth {
			local := msg
			local.X -= leftWidth + 1
			u, cmd := s.rightPanel.Update(local)
			s.rightPanel = u.(Container)
			return cmd
		}
		return nil
	}

	if s.leftPanel != nil {
		u, cmd := s.leftPanel.Update(msg)
		s.leftPanel = u.(Container)
		return cmd
	}

	if s.rightPanel != nil {
		u, cmd := s.rightPanel.Update(msg)
		s.rightPanel = u.(Container)
		return cmd
	}

	return nil
}

func (s *splitPaneLayout) View() string {
	if s.width <= 0 || s.height <= 0 {
		return ""
	}

	var topSection string

	if s.leftPanel != nil && s.rightPanel != nil {
		leftView := s.leftPanel.View()
		rightView := s.rightPanel.View()

		topHeight, _ := s.splitHeights()
		barStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#555555"))
		if s.dragging {
			barStyle = barStyle.Foreground(lipgloss.Color("#7B93DB"))
		}
		bar := strings.Repeat("|\n", max(0, topHeight-1)) + "|"
		resizeBar := barStyle.Render(bar)

		topSection = lipgloss.JoinHorizontal(lipgloss.Top, leftView, resizeBar, rightView)
	} else if s.leftPanel != nil {
		topSection = s.leftPanel.View()
	} else if s.rightPanel != nil {
		topSection = s.rightPanel.View()
	} else {
		topSection = ""
	}

	var finalView string

	if s.bottomPanel != nil && topSection != "" {
		bottomView := s.bottomPanel.View()
		finalView = lipgloss.JoinVertical(lipgloss.Left, topSection, bottomView)
	} else if s.bottomPanel != nil {
		finalView = s.bottomPanel.View()
	} else {
		finalView = topSection
	}

	if finalView != "" {
		return lipgloss.Place(s.width, s.height, lipgloss.Left, lipgloss.Top, finalView)
	}

	return finalView
}

func (s *splitPaneLayout) SetSize(width, height int) tea.Cmd {
	s.width = max(0, width)
	s.height = max(0, height)

	topHeight, bottomHeight := s.splitHeights()
	leftWidth, rightWidth := s.splitWidths()

	var cmds []tea.Cmd
	if s.leftPanel != nil {
		cmd := s.leftPanel.SetSize(leftWidth, topHeight)
		cmds = append(cmds, cmd)
	}

	if s.rightPanel != nil {
		cmd := s.rightPanel.SetSize(rightWidth, topHeight)
		cmds = append(cmds, cmd)
	}

	if s.bottomPanel != nil {
		cmd := s.bottomPanel.SetSize(s.width, bottomHeight)
		cmds = append(cmds, cmd)
	}
	return tea.Batch(cmds...)
}

func (s *splitPaneLayout) ResizeHorizontal(delta float64) tea.Cmd {
	s.ratio = clampFloat(s.ratio+delta, 0.20, 0.85)
	return s.SetSize(s.width, s.height)
}

func (s *splitPaneLayout) ResizeVertical(delta float64) tea.Cmd {
	s.verticalRatio = clampFloat(s.verticalRatio+delta, 0.45, 0.94)
	return s.SetSize(s.width, s.height)
}

func (s *splitPaneLayout) GetSize() (int, int) {
	return s.width, s.height
}

func (s *splitPaneLayout) SetLeftPanel(panel Container) tea.Cmd {
	s.leftPanel = panel
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) SetRightPanel(panel Container) tea.Cmd {
	s.rightPanel = panel
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) SetBottomPanel(panel Container) tea.Cmd {
	s.bottomPanel = panel
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) ClearLeftPanel() tea.Cmd {
	s.leftPanel = nil
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) ClearRightPanel() tea.Cmd {
	s.rightPanel = nil
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) ClearBottomPanel() tea.Cmd {
	s.bottomPanel = nil
	if s.width > 0 && s.height > 0 {
		return s.SetSize(s.width, s.height)
	}
	return nil
}

func (s *splitPaneLayout) BindingKeys() []key.Binding {
	keys := []key.Binding{}
	if s.leftPanel != nil {
		if b, ok := s.leftPanel.(Bindings); ok {
			keys = append(keys, b.BindingKeys()...)
		}
	}
	if s.rightPanel != nil {
		if b, ok := s.rightPanel.(Bindings); ok {
			keys = append(keys, b.BindingKeys()...)
		}
	}
	if s.bottomPanel != nil {
		if b, ok := s.bottomPanel.(Bindings); ok {
			keys = append(keys, b.BindingKeys()...)
		}
	}
	return keys
}

func (s *splitPaneLayout) splitHeights() (int, int) {
	height := max(0, s.height)
	if s.bottomPanel == nil {
		return height, 0
	}
	if height == 0 {
		return 0, 0
	}

	minTop := 6
	minBottom := 3
	if height < minTop+minBottom {
		minBottom = min(2, max(0, height/3))
		minTop = max(0, height-minBottom)
	}

	topHeight := int(float64(height) * clampFloat(s.verticalRatio, 0.05, 0.98))
	topHeight = clampInt(topHeight, minTop, height-minBottom)
	bottomHeight := height - topHeight
	return topHeight, bottomHeight
}

func (s *splitPaneLayout) splitWidths() (int, int) {
	width := max(0, s.width)
	if s.leftPanel != nil && s.rightPanel != nil {
		availWidth := max(0, width-1)
		minLeft := 32
		minRight := 28
		if availWidth < minLeft+minRight {
			rightWidth := min(minRight, max(0, availWidth/3))
			return availWidth - rightWidth, rightWidth
		}
		leftWidth := int(float64(availWidth) * clampFloat(s.ratio, 0.05, 0.95))
		leftWidth = clampInt(leftWidth, minLeft, availWidth-minRight)
		return leftWidth, availWidth - leftWidth
	}
	if s.leftPanel != nil {
		return width, 0
	}
	if s.rightPanel != nil {
		return 0, width
	}
	return 0, 0
}

func NewSplitPane(options ...SplitPaneOption) SplitPaneLayout {
	layout := &splitPaneLayout{
		ratio:         0.7,
		verticalRatio: 0.9,
	}
	for _, option := range options {
		option(layout)
	}
	layout.ratio = clampFloat(layout.ratio, 0.20, 0.85)
	layout.verticalRatio = clampFloat(layout.verticalRatio, 0.45, 0.94)
	return layout
}

func WithLeftPanel(panel Container) SplitPaneOption {
	return func(s *splitPaneLayout) {
		s.leftPanel = panel
	}
}

func WithRightPanel(panel Container) SplitPaneOption {
	return func(s *splitPaneLayout) {
		s.rightPanel = panel
	}
}

func WithRatio(ratio float64) SplitPaneOption {
	return func(s *splitPaneLayout) {
		s.ratio = ratio
	}
}

func WithBottomPanel(panel Container) SplitPaneOption {
	return func(s *splitPaneLayout) {
		s.bottomPanel = panel
	}
}

func WithVerticalRatio(ratio float64) SplitPaneOption {
	return func(s *splitPaneLayout) {
		s.verticalRatio = ratio
	}
}

func clampFloat(value, lower, upper float64) float64 {
	if value < lower {
		return lower
	}
	if value > upper {
		return upper
	}
	return value
}

func clampInt(value, lower, upper int) int {
	if upper < lower {
		return lower
	}
	if value < lower {
		return lower
	}
	if value > upper {
		return upper
	}
	return value
}
