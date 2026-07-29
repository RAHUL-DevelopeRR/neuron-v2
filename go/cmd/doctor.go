package cmd

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/opencode-ai/opencode/internal/auth"
	"github.com/opencode-ai/opencode/internal/version"
	"github.com/spf13/cobra"
)

func newDoctorCommand() *cobra.Command {
	var noNetwork bool
	cmd := &cobra.Command{
		Use:   "doctor",
		Short: "Show install, shell, auth, gateway, and update diagnostics",
		RunE: func(cmd *cobra.Command, args []string) error {
			return runDoctor(noNetwork)
		},
	}
	cmd.Flags().BoolVar(&noNetwork, "no-network", false, "Skip gateway health checks")
	return cmd
}

func runDoctor(noNetwork bool) error {
	info := detectInstallInfo()
	cache, path, signedIn := auth.LoadSession()
	gateway := defaultAuthGateway()
	if cache.GatewayURL != "" {
		gateway = cache.GatewayURL
	}

	fmt.Println("NeuronCLI Doctor")
	fmt.Println()
	fmt.Printf("Version:        %s\n", version.Version)
	fmt.Printf("Commit:         %s\n", version.Commit)
	fmt.Printf("Build date:     %s\n", version.Date)
	fmt.Printf("OS/Arch:        %s/%s\n", runtime.GOOS, runtime.GOARCH)
	fmt.Printf("Executable:     %s\n", info.Executable)
	fmt.Printf("Install source: %s\n", info.Source)
	fmt.Printf("Update command: %s\n", info.UpdateCommand)
	fmt.Printf("Shell:          %s\n", detectShell())
	fmt.Printf("Terminal:       %s\n", detectTerminal())
	fmt.Printf("Gateway:        %s\n", gateway)
	fmt.Printf("Auth cache:     %s\n", valueOr(path, "none"))
	if signedIn {
		fmt.Printf("Signed in:      yes (%s, %s)\n", auth.DisplayName(cache), strings.ToUpper(valueOr(cache.Plan, "free")))
		if cache.Quota.DailyLimit > 0 {
			fmt.Printf("Usage:          %s / %s tokens\n", formatCount(cache.Quota.Used), formatCount(cache.Quota.DailyLimit))
		}
	} else {
		fmt.Println("Signed in:      no")
	}
	if !noNetwork {
		fmt.Printf("Gateway health: %s\n", gatewayHealth(gateway))
	}
	fmt.Printf("Notes:          %s\n", info.Notes)
	return nil
}

func detectShell() string {
	if runtime.GOOS == "windows" {
		if os.Getenv("PSModulePath") != "" {
			return "powershell"
		}
		return valueOr(os.Getenv("COMSPEC"), "cmd")
	}
	return valueOr(os.Getenv("SHELL"), "unknown")
}

func detectTerminal() string {
	switch {
	case os.Getenv("WT_SESSION") != "":
		return "Windows Terminal"
	case os.Getenv("TERM_PROGRAM") != "":
		return os.Getenv("TERM_PROGRAM")
	case os.Getenv("TERM") != "":
		return os.Getenv("TERM")
	case os.Getenv("COLORTERM") != "":
		return os.Getenv("COLORTERM")
	default:
		return "unknown"
	}
}

func gatewayHealth(gateway string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(gateway, "/")+"/health", nil)
	if err != nil {
		return "invalid gateway: " + err.Error()
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "unreachable: " + err.Error()
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 200 && resp.StatusCode < 300 {
		return "ok"
	}
	return resp.Status
}
