package layout

import (
	"testing"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
)

type testContainer struct {
	width  int
	height int
}

func (t *testContainer) Init() tea.Cmd { return nil }

func (t *testContainer) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	return t, nil
}

func (t *testContainer) View() string { return "" }

func (t *testContainer) SetSize(width, height int) tea.Cmd {
	t.width = width
	t.height = height
	return nil
}

func (t *testContainer) GetSize() (int, int) { return t.width, t.height }

func (t *testContainer) BindingKeys() []key.Binding { return nil }

func TestSplitPaneKeepsPanelsUsableWhenResized(t *testing.T) {
	left := &testContainer{}
	right := &testContainer{}
	bottom := &testContainer{}
	pane := NewSplitPane(
		WithLeftPanel(left),
		WithRightPanel(right),
		WithBottomPanel(bottom),
	)

	pane.SetSize(120, 32)
	if left.width <= 0 || right.width <= 0 || bottom.height <= 0 {
		t.Fatalf("expected positive panel sizes, left=%dx%d right=%dx%d bottom=%dx%d", left.width, left.height, right.width, right.height, bottom.width, bottom.height)
	}

	pane.SetSize(42, 7)
	if left.width < 0 || right.width < 0 || bottom.height < 0 {
		t.Fatalf("panel sizes went negative after tight resize, left=%dx%d right=%dx%d bottom=%dx%d", left.width, left.height, right.width, right.height, bottom.width, bottom.height)
	}
}

func TestSplitPaneKeyboardResizeChangesRatios(t *testing.T) {
	left := &testContainer{}
	right := &testContainer{}
	pane := NewSplitPane(WithLeftPanel(left), WithRightPanel(right))
	pane.SetSize(100, 20)
	before := left.width
	pane.ResizeHorizontal(0.08)
	if left.width <= before {
		t.Fatalf("ResizeHorizontal did not increase left pane width: before=%d after=%d", before, left.width)
	}
}
