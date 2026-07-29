package cmd

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

type installInfo struct {
	Source        string
	Executable    string
	UpdateCommand string
	Notes         string
}

func detectInstallInfo() installInfo {
	exe, _ := os.Executable()
	exe, _ = filepath.EvalSymlinks(exe)
	source := strings.TrimSpace(os.Getenv("NEURON_INSTALL_SOURCE"))
	if source == "" {
		source = inferInstallSource(exe)
	}
	return installInfo{
		Source:        source,
		Executable:    exe,
		UpdateCommand: updateCommandForSource(source),
		Notes:         installNotesForSource(source),
	}
}

func inferInstallSource(exe string) string {
	normalized := strings.ToLower(filepath.ToSlash(exe))
	switch {
	case strings.Contains(normalized, "/node_modules/"):
		return "npm"
	case strings.Contains(normalized, "/site-packages/") || strings.Contains(normalized, "/dist-packages/"):
		return "pypi"
	case strings.Contains(normalized, "/cellar/") || strings.Contains(normalized, "/homebrew/") || strings.HasPrefix(normalized, "/opt/homebrew/"):
		return "homebrew"
	case runtime.GOOS == "windows" && strings.Contains(normalized, "/program files/"):
		return "msi"
	case runtime.GOOS == "linux" && (normalized == "/usr/bin/neuron" || normalized == "/bin/neuron"):
		if hasCommand("dpkg") {
			return "deb"
		}
		if hasCommand("rpm") {
			return "rpm"
		}
		return "linux-package"
	case strings.Contains(normalized, "/.cache/neuroncli/"):
		return "release-cache"
	default:
		return "archive"
	}
}

func updateCommandForSource(source string) string {
	switch strings.ToLower(source) {
	case "npm":
		return "npm update -g @zero-x/neuron"
	case "pypi", "pipx":
		return "pipx upgrade neuroncli"
	case "homebrew", "brew":
		return "brew update && brew upgrade neuroncli"
	case "deb":
		return "sudo apt update && sudo apt install --only-upgrade neuroncli"
	case "rpm":
		return "sudo dnf upgrade neuroncli"
	case "msi":
		return "Download and run the latest NeuronCLI .msi from the GitHub release"
	default:
		return "neuron update --download"
	}
}

func installNotesForSource(source string) string {
	switch strings.ToLower(source) {
	case "npm":
		return "Installed through the npm launcher and platform package."
	case "pypi", "pipx":
		return "Installed through the Python shim."
	case "homebrew", "brew":
		return "Installed through the Homebrew formula."
	case "deb", "rpm", "linux-package":
		return "Installed as a Linux system package."
	case "msi":
		return "Installed as a Windows system package."
	case "release-cache":
		return "Downloaded by the package-manager launcher from GitHub Releases."
	default:
		return "Installed from a raw archive or local build."
	}
}

func hasCommand(name string) bool {
	_, err := execLookPath(name)
	return err == nil
}

var execLookPath = func(name string) (string, error) {
	return filepath.Abs(name)
}
