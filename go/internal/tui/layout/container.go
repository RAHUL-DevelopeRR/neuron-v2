package layout

import (
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

type Container interface {
	tea.Model
	Sizeable
	Bindings
}

type KeyCapturer interface {
	CapturesKeys() bool
}

type container struct {
	width  int
	height int

	content tea.Model

	// Style options
	paddingTop    int
	paddingRight  int
	paddingBottom int
	paddingLeft   int

	borderTop    bool
	borderRight  bool
	borderBottom bool
	borderLeft   bool
	borderStyle  lipgloss.Border
}

func (c *container) Init() tea.Cmd {
	return c.content.Init()
}

func (c *container) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	if mouseMsg, ok := msg.(tea.MouseMsg); ok {
		contentWidth, contentHeight := c.contentSize()
		local := mouseMsg
		local.X -= c.paddingLeft
		local.Y -= c.paddingTop
		if c.borderLeft {
			local.X--
		}
		if c.borderTop {
			local.Y--
		}
		if local.X < 0 || local.Y < 0 || local.X >= contentWidth || local.Y >= contentHeight {
			return c, nil
		}
		msg = local
	}

	u, cmd := c.content.Update(msg)
	c.content = u
	return c, cmd
}

func (c *container) View() string {
	t := theme.CurrentTheme()
	if !c.borderTop && !c.borderRight && !c.borderBottom && !c.borderLeft {
		return c.renderBorderless()
	}

	style := lipgloss.NewStyle()
	width := c.width
	height := c.height

	style = style.Background(t.Background())

	// Apply border if any side is enabled
	if c.borderTop || c.borderRight || c.borderBottom || c.borderLeft {
		// Adjust width and height for borders
		if c.borderTop {
			height--
		}
		if c.borderBottom {
			height--
		}
		if c.borderLeft {
			width--
		}
		if c.borderRight {
			width--
		}
		style = style.Border(c.borderStyle, c.borderTop, c.borderRight, c.borderBottom, c.borderLeft)
		style = style.BorderBackground(t.Background()).BorderForeground(t.BorderNormal())
	}
	style = style.
		Width(width).
		Height(height).
		PaddingTop(c.paddingTop).
		PaddingRight(c.paddingRight).
		PaddingBottom(c.paddingBottom).
		PaddingLeft(c.paddingLeft)

	return style.Render(c.content.View())
}

func (c *container) renderBorderless() string {
	if c.width <= 0 || c.height <= 0 {
		return ""
	}

	lines := make([]string, 0)
	blank := strings.Repeat(" ", c.width)
	for i := 0; i < c.paddingTop; i++ {
		lines = append(lines, blank)
	}

	leftPad := strings.Repeat(" ", c.paddingLeft)
	rightPad := strings.Repeat(" ", c.paddingRight)
	content := c.content.View()
	if content != "" {
		for _, line := range strings.Split(content, "\n") {
			lines = append(lines, leftPad+line+rightPad)
		}
	}

	for i := 0; i < c.paddingBottom; i++ {
		lines = append(lines, blank)
	}

	return lipgloss.Place(c.width, c.height, lipgloss.Left, lipgloss.Top, strings.Join(lines, "\n"))
}

func (c *container) SetSize(width, height int) tea.Cmd {
	c.width = width
	c.height = height

	// If the content implements Sizeable, adjust its size to account for padding and borders
	if sizeable, ok := c.content.(Sizeable); ok {
		contentWidth, contentHeight := c.contentSize()
		return sizeable.SetSize(contentWidth, contentHeight)
	}
	return nil
}

func (c *container) contentSize() (int, int) {
	horizontalSpace := c.paddingLeft + c.paddingRight
	if c.borderLeft {
		horizontalSpace++
	}
	if c.borderRight {
		horizontalSpace++
	}

	verticalSpace := c.paddingTop + c.paddingBottom
	if c.borderTop {
		verticalSpace++
	}
	if c.borderBottom {
		verticalSpace++
	}

	return max(0, c.width-horizontalSpace), max(0, c.height-verticalSpace)
}

func (c *container) GetSize() (int, int) {
	return c.width, c.height
}

func (c *container) BindingKeys() []key.Binding {
	if b, ok := c.content.(Bindings); ok {
		return b.BindingKeys()
	}
	return []key.Binding{}
}

func (c *container) CapturesKeys() bool {
	if capturer, ok := c.content.(KeyCapturer); ok {
		return capturer.CapturesKeys()
	}
	return false
}

type ContainerOption func(*container)

func NewContainer(content tea.Model, options ...ContainerOption) Container {

	c := &container{
		content:     content,
		borderStyle: lipgloss.NormalBorder(),
	}

	for _, option := range options {
		option(c)
	}

	return c
}

// Padding options
func WithPadding(top, right, bottom, left int) ContainerOption {
	return func(c *container) {
		c.paddingTop = top
		c.paddingRight = right
		c.paddingBottom = bottom
		c.paddingLeft = left
	}
}

func WithPaddingAll(padding int) ContainerOption {
	return WithPadding(padding, padding, padding, padding)
}

func WithPaddingHorizontal(padding int) ContainerOption {
	return func(c *container) {
		c.paddingLeft = padding
		c.paddingRight = padding
	}
}

func WithPaddingVertical(padding int) ContainerOption {
	return func(c *container) {
		c.paddingTop = padding
		c.paddingBottom = padding
	}
}

func WithBorder(top, right, bottom, left bool) ContainerOption {
	return func(c *container) {
		c.borderTop = top
		c.borderRight = right
		c.borderBottom = bottom
		c.borderLeft = left
	}
}

func WithBorderAll() ContainerOption {
	return WithBorder(true, true, true, true)
}

func WithBorderHorizontal() ContainerOption {
	return WithBorder(true, false, true, false)
}

func WithBorderVertical() ContainerOption {
	return WithBorder(false, true, false, true)
}

func WithBorderStyle(style lipgloss.Border) ContainerOption {
	return func(c *container) {
		c.borderStyle = style
	}
}

func WithRoundedBorder() ContainerOption {
	return WithBorderStyle(lipgloss.RoundedBorder())
}

func WithThickBorder() ContainerOption {
	return WithBorderStyle(lipgloss.ThickBorder())
}

func WithDoubleBorder() ContainerOption {
	return WithBorderStyle(lipgloss.DoubleBorder())
}
