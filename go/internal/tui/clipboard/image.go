package clipboard

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/tui/media"
)

func ReadImageAttachment() (message.Attachment, error) {
	path, err := readImageToTempFile()
	if err != nil {
		return message.Attachment{}, err
	}
	return media.AttachmentFromFile(path)
}

func readImageToTempFile() (string, error) {
	switch runtime.GOOS {
	case "windows":
		return readWindowsClipboardImage()
	case "darwin":
		return readMacClipboardImage()
	default:
		return readLinuxClipboardImage()
	}
}

func readWindowsClipboardImage() (string, error) {
	script := `$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if (-not [System.Windows.Forms.Clipboard]::ContainsImage()) { exit 2 }
$img = [System.Windows.Forms.Clipboard]::GetImage()
$path = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), ('neuron-clipboard-' + [guid]::NewGuid().ToString() + '.png'))
$img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
Write-Output $path`

	out, err := runClipboardCommand("powershell.exe", "-NoProfile", "-STA", "-Command", script)
	if err != nil {
		return "", fmt.Errorf("clipboard does not contain an image")
	}
	path := strings.TrimSpace(out)
	if path == "" {
		return "", fmt.Errorf("clipboard does not contain an image")
	}
	return path, nil
}

func readMacClipboardImage() (string, error) {
	if _, err := exec.LookPath("pngpaste"); err != nil {
		return "", fmt.Errorf("install pngpaste or use ctrl+f to attach an image file")
	}
	path := filepath.Join(os.TempDir(), fmt.Sprintf("neuron-clipboard-%d.png", time.Now().UnixNano()))
	if _, err := runClipboardCommand("pngpaste", path); err != nil {
		return "", fmt.Errorf("clipboard does not contain an image")
	}
	return path, nil
}

func readLinuxClipboardImage() (string, error) {
	if out, err := runClipboardCommand("wl-paste", "--type", "image/png", "--no-newline"); err == nil && len(out) > 0 {
		return writeTempImage([]byte(out))
	}
	if out, err := runClipboardCommand("xclip", "-selection", "clipboard", "-t", "image/png", "-o"); err == nil && len(out) > 0 {
		return writeTempImage([]byte(out))
	}
	return "", fmt.Errorf("clipboard does not contain an image, or wl-paste/xclip is unavailable")
}

func writeTempImage(data []byte) (string, error) {
	file, err := os.CreateTemp("", "neuron-clipboard-*.png")
	if err != nil {
		return "", err
	}
	defer file.Close()
	if _, err := file.Write(data); err != nil {
		return "", err
	}
	return file.Name(), nil
}

func runClipboardCommand(name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	cmd := exec.CommandContext(ctx, name, args...)
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		if stderr.Len() > 0 {
			return "", fmt.Errorf("%w: %s", err, strings.TrimSpace(stderr.String()))
		}
		return "", err
	}
	return stdout.String(), nil
}
