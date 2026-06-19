package media

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/opencode-ai/opencode/internal/message"
	tuiimage "github.com/opencode-ai/opencode/internal/tui/image"
)

const MaxAttachmentSize = int64(5 * 1024 * 1024)

func IsSupportedImage(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".jpg", ".jpeg", ".png", ".webp", ".gif":
		return true
	default:
		return false
	}
}

func AttachmentFromFile(path string) (message.Attachment, error) {
	path = strings.Trim(strings.TrimSpace(path), "\"'")
	if path == "" {
		return message.Attachment{}, fmt.Errorf("image path is empty")
	}

	info, err := os.Stat(path)
	if err != nil {
		return message.Attachment{}, fmt.Errorf("unable to read image: %w", err)
	}
	if info.IsDir() {
		return message.Attachment{}, fmt.Errorf("selected path is a directory")
	}
	if !IsSupportedImage(path) {
		return message.Attachment{}, fmt.Errorf("unsupported image type")
	}

	isFileLarge, err := tuiimage.ValidateFileSize(path, MaxAttachmentSize)
	if err != nil {
		return message.Attachment{}, fmt.Errorf("unable to read image: %w", err)
	}
	if isFileLarge {
		return message.Attachment{}, fmt.Errorf("image is too large; max size is 5MB")
	}

	content, err := os.ReadFile(path)
	if err != nil {
		return message.Attachment{}, fmt.Errorf("unable to read image: %w", err)
	}
	if len(content) == 0 {
		return message.Attachment{}, fmt.Errorf("image file is empty")
	}

	mimeBufferSize := min(512, len(content))
	mimeType := http.DetectContentType(content[:mimeBufferSize])
	if mimeType == "application/octet-stream" {
		mimeType = mimeTypeFromExtension(path)
	}

	return message.Attachment{
		FilePath: path,
		FileName: filepath.Base(path),
		MimeType: mimeType,
		Content:  content,
	}, nil
}

func mimeTypeFromExtension(path string) string {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".png":
		return "image/png"
	case ".webp":
		return "image/webp"
	case ".gif":
		return "image/gif"
	default:
		return "application/octet-stream"
	}
}
