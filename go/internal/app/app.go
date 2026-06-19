package app

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"maps"
	"sync"
	"time"

	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/db"
	"github.com/opencode-ai/opencode/internal/format"
	"github.com/opencode-ai/opencode/internal/history"
	"github.com/opencode-ai/opencode/internal/llm/agent"
	"github.com/opencode-ai/opencode/internal/llm/prompt"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/lsp"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/permission"
	"github.com/opencode-ai/opencode/internal/session"
	"github.com/opencode-ai/opencode/internal/tui/theme"
)

type App struct {
	Sessions    session.Service
	Messages    message.Service
	History     history.Service
	Permissions permission.Service

	CoderAgent agent.Service

	LSPClients map[string]*lsp.Client

	clientsMutex sync.RWMutex

	watcherCancelFuncs []context.CancelFunc
	cancelFuncsMutex   sync.Mutex
	watcherWG          sync.WaitGroup

	// agentReady is closed when CoderAgent is initialized.
	agentReady chan struct{}
	agentErr   error
}

// WaitForAgent blocks until the CoderAgent is ready. Returns the agent
// init error if it failed. In practice this returns near-instantly after
// the repomap guard was added, but it guarantees correctness.
func (app *App) WaitForAgent() error {
	<-app.agentReady
	return app.agentErr
}

// IsAgentReady returns true if the CoderAgent has finished initialization.
func (app *App) IsAgentReady() bool {
	select {
	case <-app.agentReady:
		return true
	default:
		return false
	}
}

func New(ctx context.Context, conn *sql.DB) (*App, error) {
	q := db.New(conn)
	sessions := session.NewService(q)
	messages := message.NewService(q)
	files := history.NewService(q, conn)

	app := &App{
		Sessions:    sessions,
		Messages:    messages,
		History:     files,
		Permissions: permission.NewPermissionService(),
		LSPClients:  make(map[string]*lsp.Client),
		agentReady:  make(chan struct{}),
	}

	// Initialize theme based on configuration
	app.initTheme()

	// Initialize LSP clients in the background
	go app.initLSPClients(ctx)

	// Initialize CoderAgent in the background — this calls
	// prompt.GetAgentPrompt which builds the repomap. With the
	// repomap guard, this completes in <1s for most workspaces.
	go func() {
		defer close(app.agentReady)
		var err error
		app.CoderAgent, err = agent.NewAgent(
			config.AgentCoder,
			app.Sessions,
			app.Messages,
			agent.CoderAgentTools(
				app.Permissions,
				app.Sessions,
				app.Messages,
				app.History,
				app.LSPClients,
			),
		)
		if err != nil {
			logging.Error("Failed to create coder agent", err)
			app.agentErr = err
		}
	}()

	return app, nil
}

// initTheme sets the application theme based on the configuration
func (app *App) initTheme() {
	cfg := config.Get()
	if cfg == nil || cfg.TUI.Theme == "" {
		return // Use default theme
	}

	// Try to set the theme from config
	err := theme.SetTheme(cfg.TUI.Theme)
	if err != nil {
		logging.Warn("Failed to set theme from config, using default theme", "theme", cfg.TUI.Theme, "error", err)
	} else {
		logging.Debug("Set theme from config", "theme", cfg.TUI.Theme)
	}
}

// RunNonInteractive handles the execution flow when a prompt is provided via CLI flag.
func (a *App) RunNonInteractive(ctx context.Context, prompt string, outputFormat string, quiet bool) error {
	logging.Info("Running in non-interactive mode")

	// Start spinner if not in quiet mode
	var spinner *format.Spinner
	if !quiet {
		spinner = format.NewSpinner("Thinking...")
		spinner.Start()
		defer spinner.Stop()
	}

	const maxPromptLengthForTitle = 100
	titlePrefix := "Non-interactive: "
	var titleSuffix string

	if len(prompt) > maxPromptLengthForTitle {
		titleSuffix = prompt[:maxPromptLengthForTitle] + "..."
	} else {
		titleSuffix = prompt
	}
	title := titlePrefix + titleSuffix

	sess, err := a.Sessions.Create(ctx, title)
	if err != nil {
		return fmt.Errorf("failed to create session for non-interactive mode: %w", err)
	}
	logging.Info("Created session for non-interactive run", "session_id", sess.ID)

	// Automatically approve all permission requests for this non-interactive session
	a.Permissions.AutoApproveSession(sess.ID)

	done, err := a.CoderAgent.Run(ctx, sess.ID, prompt)
	if err != nil {
		return fmt.Errorf("failed to start agent processing stream: %w", err)
	}

	result := <-done
	if result.Error != nil {
		if errors.Is(result.Error, context.Canceled) || errors.Is(result.Error, agent.ErrRequestCancelled) {
			logging.Info("Agent processing cancelled", "session_id", sess.ID)
			return nil
		}
		return fmt.Errorf("agent processing failed: %w", result.Error)
	}

	// Stop spinner before printing output
	if !quiet && spinner != nil {
		spinner.Stop()
	}

	// Get the text content from the response
	// For reasoning models (e.g. Kimi-K2.5), output may be in ReasoningContent instead of Content
	content := "No content available"
	contentSource := "none"
	if result.Message.Content().String() != "" {
		content = result.Message.Content().String()
		contentSource = "Content"
	} else if result.Message.ReasoningContent().String() != "" {
		content = result.Message.ReasoningContent().String()
		contentSource = "ReasoningContent"
	}
	logging.Info("[NON-INTERACTIVE] About to print output", "contentSource", contentSource, "contentLen", len(content))

	fmt.Println(format.FormatOutput(content, outputFormat))

	logging.Info("Non-interactive run completed", "session_id", sess.ID)

	return nil
}

// Shutdown performs a clean shutdown of the application
func (app *App) Shutdown() {
	app.shutdownWorkspaceServices()
}

// SwitchWorkingDirectory restarts workspace-scoped services so the next model
// request and LSP operation use the new directory rather than stale context.
func (app *App) SwitchWorkingDirectory(ctx context.Context, dir string) error {
	if app.CoderAgent.IsBusy() {
		return fmt.Errorf("cannot switch directory while the agent is busy")
	}

	if err := config.SetWorkingDirectory(dir); err != nil {
		return err
	}
	prompt.ResetWorkspaceCaches()

	app.shutdownWorkspaceServices()
	go app.initLSPClients(ctx)

	coderAgent, err := agent.NewAgent(
		config.AgentCoder,
		app.Sessions,
		app.Messages,
		agent.CoderAgentTools(
			app.Permissions,
			app.Sessions,
			app.Messages,
			app.History,
			app.LSPClients,
		),
	)
	if err != nil {
		return fmt.Errorf("failed to recreate coder agent: %w", err)
	}
	app.CoderAgent = coderAgent
	return nil
}

func (app *App) shutdownWorkspaceServices() {
	// Cancel all watcher goroutines
	app.cancelFuncsMutex.Lock()
	cancelFuncs := app.watcherCancelFuncs
	app.watcherCancelFuncs = nil
	app.cancelFuncsMutex.Unlock()

	for _, cancel := range cancelFuncs {
		cancel()
	}
	app.watcherWG.Wait()

	// Perform additional cleanup for LSP clients
	app.clientsMutex.Lock()
	clients := make(map[string]*lsp.Client, len(app.LSPClients))
	maps.Copy(clients, app.LSPClients)
	app.LSPClients = make(map[string]*lsp.Client)
	app.clientsMutex.Unlock()

	for name, client := range clients {
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		if err := client.Shutdown(shutdownCtx); err != nil {
			logging.Error("Failed to shutdown LSP client", "name", name, "error", err)
		}
		cancel()
	}
}
