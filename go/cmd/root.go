package cmd

import (
	"context"
	"fmt"
	"os"
	"sync"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	zone "github.com/lrstanley/bubblezone"
	"github.com/opencode-ai/opencode/internal/app"
	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/db"
	"github.com/opencode-ai/opencode/internal/format"
	"github.com/opencode-ai/opencode/internal/llm/agent"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/pubsub"
	"github.com/opencode-ai/opencode/internal/tui"
	"github.com/opencode-ai/opencode/internal/tui/termcolor"
	"github.com/opencode-ai/opencode/internal/version"
	"github.com/spf13/cobra"
)

var rootCmd = &cobra.Command{
	Use:   "neuron",
	Short: "NeuronCLI — AI-powered coding agent for the terminal",
	Long: `NeuronCLI is a powerful terminal-based AI coding agent powered by the Neuron Gateway.
It provides an interactive chat interface with AI capabilities, code analysis, and LSP integration
to assist developers in writing, debugging, and understanding code directly from the terminal.

Models are served through the NeuronCLI Gateway (zero-x.live) — no API keys needed.`,
	Example: `
  # Run in interactive mode
  neuron

  # Run with debug logging
  neuron -d

  # Run with debug logging in a specific directory
  neuron -d -c /path/to/project

  # Print version
  neuron -v

  # Run a single non-interactive prompt
  neuron -p "Explain the use of context in Go"

  # Run a single non-interactive prompt with JSON output format
  neuron -p "Explain the use of context in Go" -f json
  `,
	RunE: func(cmd *cobra.Command, args []string) error {
		// If the help flag is set, show the help message
		if cmd.Flag("help").Changed {
			cmd.Help()
			return nil
		}
		if cmd.Flag("version").Changed {
			fmt.Println(version.Version)
			return nil
		}

		// Load the config
		debug, _ := cmd.Flags().GetBool("debug")
		cwd, _ := cmd.Flags().GetString("cwd")
		prompt, _ := cmd.Flags().GetString("prompt")
		outputFormat, _ := cmd.Flags().GetString("output-format")
		quiet, _ := cmd.Flags().GetBool("quiet")

		// Validate format option
		if !format.IsValid(outputFormat) {
			return fmt.Errorf("invalid format option: %s\n%s", outputFormat, format.GetHelpText())
		}

		if cwd != "" {
			err := os.Chdir(cwd)
			if err != nil {
				return fmt.Errorf("failed to change directory: %v", err)
			}
		}
		if cwd == "" {
			c, err := os.Getwd()
			if err != nil {
				return fmt.Errorf("failed to get current working directory: %v", err)
			}
			cwd = c
		}

		// Non-interactive mode: sequential init (must complete before output)
		if prompt != "" {
			_, err := config.Load(cwd, debug)
			if err != nil {
				return err
			}
			conn, err := db.Connect()
			if err != nil {
				return err
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			application, err := app.New(ctx, conn)
			if err != nil {
				return err
			}
			defer application.Shutdown()
			// Agent is created async — wait for it in non-interactive mode
			if err := application.WaitForAgent(); err != nil {
				return fmt.Errorf("agent initialization failed: %w", err)
			}
			initMCPTools(ctx, application)
			return application.RunNonInteractive(ctx, prompt, outputFormat, quiet)
		}

		// ━━━ Interactive mode: show TUI FIRST, init in background ━━━
		termcolor.ForceInteractive()
		zone.NewGlobal()
		program := tea.NewProgram(
			tui.NewLazy(cwd, debug),
			tea.WithAltScreen(),
			tea.WithMouseCellMotion(),
		)

		// Background initialization — TUI is already rendering
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()

		var application *app.App
		var cancelSubs func()
		var tuiCancel context.CancelFunc
		var tuiWg sync.WaitGroup

		go func() {
			defer logging.RecoverPanic("background-init", func() {
				attemptTUIRecovery(program)
			})

			initStart := time.Now()
			totalSteps := 5

			// Step 1: Config
			program.Send(tui.InitProgressMsg{Step: 1, Total: totalSteps, Message: "Loading configuration..."})
			stepStart := time.Now()
			logging.Info("Background init: loading config...")
			_, err := config.Load(cwd, debug)
			if err != nil {
				program.Send(tui.InitErrorMsg{Err: err})
				return
			}
			logging.Info("Background init: config loaded", "step_ms", time.Since(stepStart).Milliseconds(), "total_ms", time.Since(initStart).Milliseconds())

			// Step 2: Database
			program.Send(tui.InitProgressMsg{Step: 2, Total: totalSteps, Message: "Connecting to database..."})
			stepStart = time.Now()
			logging.Info("Background init: connecting to database...")
			conn, err := db.Connect()
			if err != nil {
				program.Send(tui.InitErrorMsg{Err: err})
				return
			}
			logging.Info("Background init: database connected", "step_ms", time.Since(stepStart).Milliseconds(), "total_ms", time.Since(initStart).Milliseconds())

			// Step 3: App creation (includes LSP goroutine launch + agent creation)
			program.Send(tui.InitProgressMsg{Step: 3, Total: totalSteps, Message: "Creating app & agent..."})
			stepStart = time.Now()
			logging.Info("Background init: creating app...")
			var appErr error
			application, appErr = app.New(ctx, conn)
			if appErr != nil {
				program.Send(tui.InitErrorMsg{Err: appErr})
				return
			}
			logging.Info("Background init: app created", "step_ms", time.Since(stepStart).Milliseconds(), "total_ms", time.Since(initStart).Milliseconds())

			// Step 4: MCP tools (non-blocking)
			program.Send(tui.InitProgressMsg{Step: 4, Total: totalSteps, Message: "Starting MCP tools..."})
			stepStart = time.Now()
			initMCPTools(ctx, application)
			logging.Info("Background init: MCP tools started", "step_ms", time.Since(stepStart).Milliseconds(), "total_ms", time.Since(initStart).Milliseconds())

			// Step 5: Subscriptions
			program.Send(tui.InitProgressMsg{Step: 5, Total: totalSteps, Message: "Wiring event subscriptions..."})
			stepStart = time.Now()
			var ch <-chan tea.Msg
			ch, cancelSubs = setupSubscriptions(application, ctx)

			tuiCtx, tc := context.WithCancel(ctx)
			tuiCancel = tc
			tuiWg.Add(1)

			go func() {
				defer tuiWg.Done()
				defer logging.RecoverPanic("TUI-message-handler", func() {
					attemptTUIRecovery(program)
				})
				for {
					select {
					case <-tuiCtx.Done():
						return
					case msg, ok := <-ch:
						if !ok {
							return
						}
						program.Send(msg)
					}
				}
			}()
			logging.Info("Background init: subscriptions wired", "step_ms", time.Since(stepStart).Milliseconds(), "total_ms", time.Since(initStart).Milliseconds())

			logging.Info("Background init: COMPLETE", "total_ms", time.Since(initStart).Milliseconds())

			// Tell TUI that app is ready
			program.Send(tui.AppReadyMsg{App: application})
		}()

		// Cleanup function for when the program exits
		cleanup := func() {
			if application != nil {
				application.Shutdown()
			}
			if cancelSubs != nil {
				cancelSubs()
			}
			if tuiCancel != nil {
				tuiCancel()
			}
			tuiWg.Wait()
		}

		// Run the TUI (appears IMMEDIATELY)
		result, err := program.Run()
		cleanup()

		// Explicit terminal cleanup for Windows — prevents prompt overlap
		fmt.Print("\033[?1049l")   // Exit alt screen
		fmt.Print("\033[H\033[2J") // Clear screen + reset cursor
		fmt.Print("\033[?25h")     // Show cursor

		if err != nil {
			return fmt.Errorf("TUI error: %v", err)
		}

		logging.Info("TUI exited with result: %v", result)
		return nil
	},
}

// attemptTUIRecovery tries to recover the TUI after a panic
func attemptTUIRecovery(program *tea.Program) {
	logging.Info("Attempting to recover TUI after panic")

	// We could try to restart the TUI or gracefully exit
	// For now, we'll just quit the program to avoid further issues
	program.Quit()
}

func initMCPTools(ctx context.Context, app *app.App) {
	go func() {
		defer logging.RecoverPanic("MCP-goroutine", nil)

		// Create a context with timeout for the initial MCP tools fetch
		ctxWithTimeout, cancel := context.WithTimeout(ctx, 30*time.Second)
		defer cancel()

		// Set this up once with proper error handling
		agent.GetMcpTools(ctxWithTimeout, app.Permissions)
		logging.Info("MCP message handling goroutine exiting")
	}()
}

func setupSubscriber[T any](
	ctx context.Context,
	wg *sync.WaitGroup,
	name string,
	subscriber func(context.Context) <-chan pubsub.Event[T],
	outputCh chan<- tea.Msg,
) {
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer logging.RecoverPanic(fmt.Sprintf("subscription-%s", name), nil)

		subCh := subscriber(ctx)

		for {
			select {
			case event, ok := <-subCh:
				if !ok {
					logging.Info("subscription channel closed", "name", name)
					return
				}

				var msg tea.Msg = event

				select {
				case outputCh <- msg:
				case <-time.After(2 * time.Second):
					logging.Warn("message dropped due to slow consumer", "name", name)
				case <-ctx.Done():
					logging.Info("subscription cancelled", "name", name)
					return
				}
			case <-ctx.Done():
				logging.Info("subscription cancelled", "name", name)
				return
			}
		}
	}()
}

func setupSubscriptions(app *app.App, parentCtx context.Context) (chan tea.Msg, func()) {
	ch := make(chan tea.Msg, 100)

	wg := sync.WaitGroup{}
	ctx, cancel := context.WithCancel(parentCtx) // Inherit from parent context

	setupSubscriber(ctx, &wg, "logging", logging.Subscribe, ch)
	setupSubscriber(ctx, &wg, "sessions", app.Sessions.Subscribe, ch)
	setupSubscriber(ctx, &wg, "messages", app.Messages.Subscribe, ch)
	setupSubscriber(ctx, &wg, "permissions", app.Permissions.Subscribe, ch)

	// CoderAgent is created asynchronously — wait for it then subscribe
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer logging.RecoverPanic("subscription-coderAgent-deferred", nil)

		// Wait for agent to be ready
		if err := app.WaitForAgent(); err != nil {
			logging.Error("CoderAgent init failed, skipping subscription", "error", err)
			return
		}
		// Now safe to subscribe
		setupSubscriber(ctx, &wg, "coderAgent", app.CoderAgent.Subscribe, ch)
	}()

	cleanupFunc := func() {
		logging.Info("Cancelling all subscriptions")
		cancel() // Signal all goroutines to stop

		waitCh := make(chan struct{})
		go func() {
			defer logging.RecoverPanic("subscription-cleanup", nil)
			wg.Wait()
			close(waitCh)
		}()

		select {
		case <-waitCh:
			logging.Info("All subscription goroutines completed successfully")
			close(ch) // Only close after all writers are confirmed done
		case <-time.After(5 * time.Second):
			logging.Warn("Timed out waiting for some subscription goroutines to complete")
			close(ch)
		}
	}
	return ch, cleanupFunc
}

func Execute() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}

func init() {
	rootCmd.AddCommand(newAuthCommand())
	rootCmd.AddCommand(newDoctorCommand())
	rootCmd.AddCommand(newUpdateCommand())

	rootCmd.Flags().BoolP("help", "h", false, "Help")
	rootCmd.Flags().BoolP("version", "v", false, "Version")
	rootCmd.Flags().BoolP("debug", "d", false, "Debug")
	rootCmd.Flags().StringP("cwd", "c", "", "Current working directory")
	rootCmd.Flags().StringP("prompt", "p", "", "Prompt to run in non-interactive mode")

	// Add format flag with validation logic
	rootCmd.Flags().StringP("output-format", "f", format.Text.String(),
		"Output format for non-interactive mode (text, json)")

	// Add quiet flag to hide spinner in non-interactive mode
	rootCmd.Flags().BoolP("quiet", "q", false, "Hide spinner in non-interactive mode")

	// Register custom validation for the format flag
	rootCmd.RegisterFlagCompletionFunc("output-format", func(cmd *cobra.Command, args []string, toComplete string) ([]string, cobra.ShellCompDirective) {
		return format.SupportedFormats, cobra.ShellCompDirectiveNoFileComp
	})
}
