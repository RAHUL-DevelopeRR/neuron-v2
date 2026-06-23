package termcolor

import (
	"os"
	"runtime"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"
)

const optOutEnv = "NEURON_NO_COLOR"

// ForceInteractive enables Neuron's colorful TUI even when it is launched from
// wrappers that export generic no-color or dumb-terminal hints.
func ForceInteractive() {
	if isEnabled(os.Getenv(optOutEnv)) {
		return
	}

	_ = os.Unsetenv("NO_COLOR")
	_ = os.Setenv("CLICOLOR", "1")
	_ = os.Setenv("CLICOLOR_FORCE", "1")
	_ = os.Setenv("FORCE_COLOR", "3")
	_ = os.Setenv("COLORTERM", "truecolor")

	if term := os.Getenv("TERM"); term == "" || strings.EqualFold(term, "dumb") || runtime.GOOS == "windows" {
		_ = os.Setenv("TERM", "xterm-256color")
	}

	lipgloss.SetColorProfile(termenv.TrueColor)
	lipgloss.SetHasDarkBackground(true)
}

// Env returns a process environment that preserves the caller's variables while
// removing inherited no-color hints for child shells spawned inside the TUI.
func Env(base []string) []string {
	if envEnabled(base, optOutEnv) {
		return append([]string(nil), base...)
	}

	clean := removeEnv(base,
		"NO_COLOR",
		"CLICOLOR",
		"CLICOLOR_FORCE",
		"FORCE_COLOR",
		"COLORTERM",
		"TERM",
	)

	return append(clean,
		"CLICOLOR=1",
		"CLICOLOR_FORCE=1",
		"FORCE_COLOR=3",
		"COLORTERM=truecolor",
		"TERM=xterm-256color",
	)
}

func envEnabled(env []string, key string) bool {
	for _, entry := range env {
		name, value, ok := strings.Cut(entry, "=")
		if !ok {
			continue
		}
		if strings.EqualFold(name, key) {
			return isEnabled(value)
		}
	}
	return false
}

func isEnabled(value string) bool {
	value = strings.TrimSpace(strings.ToLower(value))
	return value != "" && value != "0" && value != "false" && value != "no"
}

func removeEnv(env []string, keys ...string) []string {
	keySet := make(map[string]struct{}, len(keys))
	for _, key := range keys {
		keySet[strings.ToUpper(key)] = struct{}{}
	}

	out := make([]string, 0, len(env)+5)
	for _, entry := range env {
		name, _, ok := strings.Cut(entry, "=")
		if !ok {
			out = append(out, entry)
			continue
		}
		if _, found := keySet[strings.ToUpper(name)]; found {
			continue
		}
		out = append(out, entry)
	}
	return out
}
