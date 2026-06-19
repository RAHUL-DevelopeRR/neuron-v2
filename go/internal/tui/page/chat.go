package page

import (
	"context"
	"strings"

	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/opencode-ai/opencode/internal/app"
	"github.com/opencode-ai/opencode/internal/completions"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/session"
	"github.com/opencode-ai/opencode/internal/tui/components/chat"
	"github.com/opencode-ai/opencode/internal/tui/components/dialog"
	"github.com/opencode-ai/opencode/internal/tui/components/terminal"
	"github.com/opencode-ai/opencode/internal/tui/layout"
	"github.com/opencode-ai/opencode/internal/tui/util"
)

var ChatPage PageID = "chat"

type chatPage struct {
	app                  *app.App
	editor               layout.Container
	messages             layout.Container
	layout               layout.SplitPaneLayout
	session              session.Session
	completionDialog     dialog.CompletionDialog
	showCompletionDialog bool
	focusTerminal        bool // true = terminal has focus, false = editor has focus
}

type ChatKeyMap struct {
	ShowCompletionDialog key.Binding
	NewSession           key.Binding
	Cancel               key.Binding
	ToggleTerminal       key.Binding
	ChatWider            key.Binding
	SidebarWider         key.Binding
	ConversationTaller   key.Binding
	EditorTaller         key.Binding
}

var keyMap = ChatKeyMap{
	ShowCompletionDialog: key.NewBinding(
		key.WithKeys("@"),
		key.WithHelp("@", "Complete"),
	),
	NewSession: key.NewBinding(
		key.WithKeys("ctrl+n"),
		key.WithHelp("ctrl+n", "new session"),
	),
	Cancel: key.NewBinding(
		key.WithKeys("esc"),
		key.WithHelp("esc", "cancel"),
	),
	ToggleTerminal: key.NewBinding(
		key.WithKeys("ctrl+x"),
		key.WithHelp("ctrl+x", "toggle terminal focus"),
	),
	ChatWider: key.NewBinding(
		key.WithKeys("alt+right"),
		key.WithHelp("alt+right", "wider chat"),
	),
	SidebarWider: key.NewBinding(
		key.WithKeys("alt+left"),
		key.WithHelp("alt+left", "wider sidebar"),
	),
	ConversationTaller: key.NewBinding(
		key.WithKeys("alt+up"),
		key.WithHelp("alt+up", "taller chat"),
	),
	EditorTaller: key.NewBinding(
		key.WithKeys("alt+down"),
		key.WithHelp("alt+down", "taller editor"),
	),
}

func (p *chatPage) Init() tea.Cmd {
	cmds := []tea.Cmd{
		p.layout.Init(),
		p.completionDialog.Init(),
	}
	return tea.Batch(cmds...)
}

func (p *chatPage) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		cmd := p.layout.SetSize(msg.Width, msg.Height)
		cmds = append(cmds, cmd)
	case dialog.CompletionDialogCloseMsg:
		p.showCompletionDialog = false
	case chat.SendMsg:
		cmd := p.sendMessage(msg.Text, msg.Attachments)
		if cmd != nil {
			return p, cmd
		}
	case dialog.CommandRunCustomMsg:
		// Check if the agent is busy before executing custom commands
		if p.app.CoderAgent != nil && p.app.CoderAgent.IsBusy() {
			return p, util.ReportWarn("Agent is busy, please wait before executing a command...")
		}

		// Process the command content with arguments if any
		content := msg.Content
		if msg.Args != nil {
			// Replace all named arguments with their values
			for name, value := range msg.Args {
				placeholder := "$" + name
				content = strings.ReplaceAll(content, placeholder, value)
			}
		}

		// Handle custom command execution
		cmd := p.sendMessage(content, nil)
		if cmd != nil {
			return p, cmd
		}
	case chat.SessionSelectedMsg:
		// Sidebar already exists from startup — just update session.
		// The SessionSelectedMsg will flow through layout.Update() to the
		// sidebar, which will update its session, diff panel, etc.
		p.session = msg
	case tea.KeyMsg:
		switch {
		case key.Matches(msg, keyMap.ToggleTerminal):
			p.focusTerminal = !p.focusTerminal
			return p, tea.Batch(
				util.CmdHandler(chat.EditorFocusMsg(!p.focusTerminal)),
				util.CmdHandler(terminal.TerminalFocusMsg{Focused: p.focusTerminal}),
			)
		case key.Matches(msg, keyMap.ChatWider):
			return p, p.layout.ResizeHorizontal(0.04)
		case key.Matches(msg, keyMap.SidebarWider):
			return p, p.layout.ResizeHorizontal(-0.04)
		case key.Matches(msg, keyMap.ConversationTaller):
			return p, p.layout.ResizeVertical(0.04)
		case key.Matches(msg, keyMap.EditorTaller):
			return p, p.layout.ResizeVertical(-0.04)
		}

		if p.focusTerminal {
			break
		}

		switch {
		case key.Matches(msg, keyMap.ShowCompletionDialog):
			p.showCompletionDialog = true
			// Continue sending keys to layout->chat
		case key.Matches(msg, keyMap.NewSession):
			p.session = session.Session{}
			p.focusTerminal = false
			return p, tea.Batch(
				util.CmdHandler(chat.EditorFocusMsg(true)),
				util.CmdHandler(chat.SessionClearedMsg{}),
			)
		case key.Matches(msg, keyMap.Cancel):
			if p.session.ID != "" && p.app.CoderAgent != nil {
				p.app.CoderAgent.Cancel(p.session.ID)
				return p, nil
			}
		}
	}
	if p.showCompletionDialog {
		context, contextCmd := p.completionDialog.Update(msg)
		p.completionDialog = context.(dialog.CompletionDialog)
		cmds = append(cmds, contextCmd)

		// Doesn't forward event if enter key is pressed
		if keyMsg, ok := msg.(tea.KeyMsg); ok {
			if keyMsg.String() == "enter" {
				return p, tea.Batch(cmds...)
			}
		}
	}

	u, cmd := p.layout.Update(msg)
	cmds = append(cmds, cmd)
	p.layout = u.(layout.SplitPaneLayout)

	return p, tea.Batch(cmds...)
}

func (p *chatPage) sendMessage(text string, attachments []message.Attachment) tea.Cmd {
	// Wait for agent to be ready (near-instant with repomap guard)
	if !p.app.IsAgentReady() {
		return util.ReportWarn("Agent is initializing, please wait a moment...")
	}
	if p.app.CoderAgent == nil {
		return util.ReportError(p.app.WaitForAgent())
	}

	var cmds []tea.Cmd
	if p.session.ID == "" {
		session, err := p.app.Sessions.Create(context.Background(), "New Session")
		if err != nil {
			return util.ReportError(err)
		}

		p.session = session
		// Sidebar already exists — SessionSelectedMsg flows through
		// layout.Update() to update the sidebar's session and diff panel
		cmds = append(cmds, util.CmdHandler(chat.SessionSelectedMsg(session)))
	}

	_, err := p.app.CoderAgent.Run(context.Background(), p.session.ID, text, attachments...)
	if err != nil {
		return util.ReportError(err)
	}
	return tea.Batch(cmds...)
}

func (p *chatPage) SetSize(width, height int) tea.Cmd {
	return p.layout.SetSize(width, height)
}

func (p *chatPage) GetSize() (int, int) {
	return p.layout.GetSize()
}

func (p *chatPage) CapturesKeys() bool {
	return p.focusTerminal
}

func (p *chatPage) View() string {
	layoutView := p.layout.View()

	if p.showCompletionDialog {
		_, layoutHeight := p.layout.GetSize()
		editorWidth, editorHeight := p.editor.GetSize()

		p.completionDialog.SetWidth(editorWidth)
		overlay := p.completionDialog.View()

		layoutView = layout.PlaceOverlay(
			0,
			layoutHeight-editorHeight-lipgloss.Height(overlay),
			overlay,
			layoutView,
			false,
		)
	}

	return layoutView
}

func (p *chatPage) BindingKeys() []key.Binding {
	bindings := layout.KeyMapToSlice(keyMap)
	bindings = append(bindings, p.messages.BindingKeys()...)
	bindings = append(bindings, p.editor.BindingKeys()...)
	return bindings
}

func NewChatPage(app *app.App) tea.Model {
	cg := completions.NewFileAndFolderContextGroup()
	completionDialog := dialog.NewCompletionDialogCmp(cg)

	messagesContainer := layout.NewContainer(
		chat.NewMessagesCmp(app),
		layout.WithPadding(1, 1, 0, 1),
	)
	editorContainer := layout.NewContainer(
		chat.NewEditorCmp(app),
		layout.WithBorder(true, false, false, false),
	)

	// Sidebar visible from startup — diff panel + real terminal
	sidebarContainer := layout.NewContainer(
		chat.NewSidebarCmp(session.Session{}, app.History),
		layout.WithPadding(1, 1, 1, 1),
	)

	return &chatPage{
		app:              app,
		editor:           editorContainer,
		messages:         messagesContainer,
		completionDialog: completionDialog,
		layout: layout.NewSplitPane(
			layout.WithLeftPanel(messagesContainer),
			layout.WithBottomPanel(editorContainer),
			layout.WithRightPanel(sidebarContainer),
		),
	}
}
