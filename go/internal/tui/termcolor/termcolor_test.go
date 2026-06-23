package termcolor

import (
	"os"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"
)

func TestForceInteractiveOverridesDumbNoColorEnv(t *testing.T) {
	t.Setenv("NO_COLOR", "1")
	t.Setenv("TERM", "dumb")
	t.Setenv("NEURON_NO_COLOR", "")

	ForceInteractive()

	assertProcessEnvValue(t, "NO_COLOR", "")
	assertProcessEnvValue(t, "TERM", "xterm-256color")
	assertProcessEnvValue(t, "COLORTERM", "truecolor")
	if lipgloss.ColorProfile() != termenv.TrueColor {
		t.Fatalf("color profile = %v, want %v", lipgloss.ColorProfile(), termenv.TrueColor)
	}
	if !lipgloss.HasDarkBackground() {
		t.Fatal("expected dark background profile")
	}
}

func TestEnvForcesColorHints(t *testing.T) {
	env := Env([]string{
		"NO_COLOR=1",
		"TERM=dumb",
		"COLORTERM=",
		"PATH=C:\\bin",
	})

	assertEnv(t, env, "NO_COLOR", false)
	assertEnvValue(t, env, "TERM", "xterm-256color")
	assertEnvValue(t, env, "COLORTERM", "truecolor")
	assertEnvValue(t, env, "CLICOLOR_FORCE", "1")
	assertEnvValue(t, env, "FORCE_COLOR", "3")
	assertEnvValue(t, env, "PATH", "C:\\bin")
}

func TestEnvRespectsNeuronNoColor(t *testing.T) {
	env := Env([]string{
		"NEURON_NO_COLOR=1",
		"NO_COLOR=1",
		"TERM=dumb",
	})

	assertEnvValue(t, env, "NO_COLOR", "1")
	assertEnvValue(t, env, "TERM", "dumb")
}

func assertEnv(t *testing.T, env []string, key string, wantFound bool) {
	t.Helper()
	_, found := lookupEnv(env, key)
	if found != wantFound {
		t.Fatalf("%s found = %v, want %v", key, found, wantFound)
	}
}

func assertEnvValue(t *testing.T, env []string, key, want string) {
	t.Helper()
	got, found := lookupEnv(env, key)
	if !found {
		t.Fatalf("%s not found", key)
	}
	if got != want {
		t.Fatalf("%s = %q, want %q", key, got, want)
	}
}

func assertProcessEnvValue(t *testing.T, key, want string) {
	t.Helper()
	got := os.Getenv(key)
	if got != want {
		t.Fatalf("%s = %q, want %q", key, got, want)
	}
}

func lookupEnv(env []string, key string) (string, bool) {
	for _, entry := range env {
		name, value, ok := strings.Cut(entry, "=")
		if ok && strings.EqualFold(name, key) {
			return value, true
		}
	}
	return "", false
}
