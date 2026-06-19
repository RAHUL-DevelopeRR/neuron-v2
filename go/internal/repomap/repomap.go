// Package repomap implements Windsurf Cascade-style codebase context indexing.
// It walks the workspace directory, extracts symbols from source files, and
// generates a compressed [CODEBASE_CONTEXT] block for the system prompt.
//
// Ported from claw-code/rust/crates/rusty-claude-cli/src/repo_map.rs
package repomap

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// MaxFiles caps the number of files indexed to prevent OOM on huge repos.
const MaxFiles = 150

// MaxMapChars caps the total output size of the repo map.
const MaxMapChars = 6000

// MaxSymbolsPerFile caps symbols extracted per file to keep the map compact.
const MaxSymbolsPerFile = 15

// SupportedExtensions lists file types we extract symbols from.
var SupportedExtensions = map[string]bool{
	"rs": true, "py": true, "js": true, "ts": true, "tsx": true, "jsx": true,
	"go": true, "java": true, "rb": true, "c": true, "cpp": true, "h": true,
	"hpp": true, "toml": true, "yaml": true, "yml": true, "json": true, "md": true,
}

// SkipDirs lists directories to always skip.
var SkipDirs = map[string]bool{
	".git": true, "node_modules": true, "target": true, "dist": true,
	"build": true, "__pycache__": true, ".neuron": true, ".claw": true,
	".venv": true, "venv": true, ".tox": true, "vendor": true, ".next": true,
	".opencode": true,
}

// FileEntry represents a single file in the repo map.
type FileEntry struct {
	RelativePath string
	Symbols      []string
	SizeBytes    int64
}

// RepoMap is the full repo map for a workspace.
type RepoMap struct {
	Root         string
	Entries      []FileEntry
	TotalFiles   int
	TotalSymbols int
}

// MaxDirsScanned caps the total number of directories visited to prevent
// unbounded scans on huge workspaces (e.g. user home directory).
const MaxDirsScanned = 500

// Build constructs a RepoMap by walking the workspace directory.
// Returns an empty RepoMap if the workspace is too large or is the user's
// home directory (which contains Downloads, AppData, etc.).
func Build(workspaceRoot string) *RepoMap {
	// Guard: skip home directory — scanning it takes minutes
	if home, err := os.UserHomeDir(); err == nil {
		cleanRoot := filepath.Clean(workspaceRoot)
		cleanHome := filepath.Clean(home)
		if cleanRoot == cleanHome {
			return &RepoMap{Root: workspaceRoot}
		}
	}

	entries := make(map[string]*FileEntry)
	fileCount := 0
	dirCount := 0

	walkDir(workspaceRoot, workspaceRoot, entries, &fileCount, &dirCount)

	totalSymbols := 0
	sortedEntries := make([]FileEntry, 0, len(entries))
	for _, e := range entries {
		totalSymbols += len(e.Symbols)
		sortedEntries = append(sortedEntries, *e)
	}

	// Sort entries by path for consistent output
	sort.Slice(sortedEntries, func(i, j int) bool {
		return sortedEntries[i].RelativePath < sortedEntries[j].RelativePath
	})

	return &RepoMap{
		Root:         workspaceRoot,
		Entries:      sortedEntries,
		TotalFiles:   len(sortedEntries),
		TotalSymbols: totalSymbols,
	}
}

// Render generates a compact text block suitable for system prompt injection.
func (rm *RepoMap) Render() string {
	var sb strings.Builder
	sb.Grow(MaxMapChars)

	sb.WriteString("[CODEBASE_CONTEXT]\n")
	sb.WriteString(fmt.Sprintf("Workspace: %s (%d files, %d symbols)\n\n",
		rm.Root, rm.TotalFiles, rm.TotalSymbols))

	for _, entry := range rm.Entries {
		var line string
		if len(entry.Symbols) == 0 {
			line = fmt.Sprintf("  %s\n", entry.RelativePath)
		} else {
			line = fmt.Sprintf("  %s — %s\n", entry.RelativePath, strings.Join(entry.Symbols, ", "))
		}
		if sb.Len()+len(line) > MaxMapChars {
			sb.WriteString("  … (truncated)\n")
			break
		}
		sb.WriteString(line)
	}

	sb.WriteString("[/CODEBASE_CONTEXT]\n")
	return sb.String()
}

// StatusLine returns a human-readable summary for display.
func (rm *RepoMap) StatusLine() string {
	return fmt.Sprintf("Indexed: %d files, %d symbols", rm.TotalFiles, rm.TotalSymbols)
}

// walkDir recursively walks directories, collecting file entries.
// Aborts when fileCount exceeds MaxFiles or dirCount exceeds MaxDirsScanned.
func walkDir(root, current string, entries map[string]*FileEntry, fileCount *int, dirCount *int) {
	if *dirCount >= MaxDirsScanned {
		return
	}
	*dirCount++

	dirEntries, err := os.ReadDir(current)
	if err != nil {
		return
	}

	for _, de := range dirEntries {
		if *fileCount >= MaxFiles || *dirCount >= MaxDirsScanned {
			return
		}

		name := de.Name()
		path := filepath.Join(current, name)

		if de.IsDir() {
			if SkipDirs[name] || strings.HasPrefix(name, ".") {
				continue
			}
			walkDir(root, path, entries, fileCount, dirCount)
		} else if de.Type().IsRegular() {
			ext := strings.TrimPrefix(filepath.Ext(name), ".")
			if !SupportedExtensions[ext] {
				continue
			}

			relPath, _ := filepath.Rel(root, path)
			relPath = filepath.ToSlash(relPath)

			info, err := de.Info()
			var sizeBytes int64
			if err == nil {
				sizeBytes = info.Size()
			}

			// Extract symbols
			var symbols []string
			switch ext {
			case "rs":
				symbols = extractRustSymbols(path)
			case "py":
				symbols = extractPythonSymbols(path)
			case "js", "ts", "tsx", "jsx":
				symbols = extractJSSymbols(path)
			case "go":
				symbols = extractGoSymbols(path)
			case "java", "rb", "c", "cpp", "h", "hpp":
				symbols = extractGenericSymbols(path)
			case "toml", "yaml", "yml":
				symbols = extractConfigKeys(path, ext)
			}

			entries[relPath] = &FileEntry{
				RelativePath: relPath,
				Symbols:      symbols,
				SizeBytes:    sizeBytes,
			}
			*fileCount++
		}
	}
}

// ── Symbol Extractors ───────────────────────────────────────────

func readFileLines(path string) []string {
	content, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	return strings.Split(string(content), "\n")
}

func extractRustSymbols(path string) []string {
	lines := readFileLines(path)
	var symbols []string
	prefixes := []struct{ prefix, display string }{
		{"pub fn ", "pub fn"},
		{"fn ", "fn"},
		{"pub struct ", "pub struct"},
		{"struct ", "struct"},
		{"pub enum ", "pub enum"},
		{"enum ", "enum"},
		{"pub trait ", "pub trait"},
		{"trait ", "trait"},
		{"impl ", "impl"},
		{"pub mod ", "pub mod"},
		{"mod ", "mod"},
	}
	for _, line := range lines {
		if len(symbols) >= MaxSymbolsPerFile {
			break
		}
		trimmed := strings.TrimSpace(line)
		for _, p := range prefixes {
			if strings.HasPrefix(trimmed, p.prefix) {
				rest := trimmed[len(p.prefix):]
				name := extractIdentifier(rest)
				if name != "" && name != "self" && name != "crate" {
					symbols = append(symbols, p.display+" "+name)
				}
				break
			}
		}
	}
	return symbols
}

func extractPythonSymbols(path string) []string {
	lines := readFileLines(path)
	var symbols []string
	for _, line := range lines {
		if len(symbols) >= MaxSymbolsPerFile {
			break
		}
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "def ") {
			name := extractIdentifier(trimmed[4:])
			if name != "" {
				symbols = append(symbols, "def "+name+"()")
			}
		} else if strings.HasPrefix(trimmed, "class ") {
			name := extractIdentifier(trimmed[6:])
			if name != "" {
				symbols = append(symbols, "class "+name)
			}
		} else if strings.HasPrefix(trimmed, "async def ") {
			name := extractIdentifier(trimmed[10:])
			if name != "" {
				symbols = append(symbols, "async def "+name+"()")
			}
		}
	}
	return symbols
}

func extractJSSymbols(path string) []string {
	lines := readFileLines(path)
	var symbols []string
	prefixes := []string{
		"export function ", "export default function ",
		"function ", "export class ", "class ",
		"export const ", "export default ",
	}
	for _, line := range lines {
		if len(symbols) >= MaxSymbolsPerFile {
			break
		}
		trimmed := strings.TrimSpace(line)
		for _, p := range prefixes {
			if strings.HasPrefix(trimmed, p) {
				name := extractIdentifier(trimmed[len(p):])
				if name != "" {
					symbols = append(symbols, name)
				}
				break
			}
		}
	}
	return symbols
}

func extractGoSymbols(path string) []string {
	lines := readFileLines(path)
	var symbols []string
	for _, line := range lines {
		if len(symbols) >= MaxSymbolsPerFile {
			break
		}
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "func ") {
			rest := trimmed[5:]
			// Skip method receivers: func (t *Type) Name(
			if strings.HasPrefix(rest, "(") {
				// Method receiver — find closing paren, then get name
				if idx := strings.Index(rest, ") "); idx >= 0 {
					name := extractIdentifier(rest[idx+2:])
					if name != "" {
						symbols = append(symbols, "func "+name)
					}
				}
			} else {
				name := extractIdentifier(rest)
				if name != "" {
					symbols = append(symbols, "func "+name)
				}
			}
		} else if strings.HasPrefix(trimmed, "type ") &&
			(strings.Contains(trimmed, " struct") || strings.Contains(trimmed, " interface")) {
			name := extractIdentifier(trimmed[5:])
			if name != "" {
				symbols = append(symbols, "type "+name)
			}
		}
	}
	return symbols
}

func extractGenericSymbols(path string) []string {
	lines := readFileLines(path)
	var symbols []string
	for _, line := range lines {
		if len(symbols) >= MaxSymbolsPerFile {
			break
		}
		trimmed := strings.TrimSpace(line)
		if (strings.HasPrefix(trimmed, "public ") ||
			strings.HasPrefix(trimmed, "private ") ||
			strings.HasPrefix(trimmed, "protected ")) &&
			(strings.Contains(trimmed, " class ") ||
				strings.Contains(trimmed, " void ") ||
				strings.Contains(trimmed, " int ")) {
			words := strings.Fields(trimmed)
			if len(words) >= 3 {
				name := strings.TrimRight(words[2], "({")
				if name != "" {
					symbols = append(symbols, name)
				}
			}
		}
	}
	return symbols
}

func extractConfigKeys(path, ext string) []string {
	lines := readFileLines(path)
	var keys []string
	maxLines := 30
	if ext == "yaml" || ext == "yml" {
		maxLines = 20
	}
	for i, line := range lines {
		if i >= maxLines || len(keys) >= 10 {
			break
		}
		switch ext {
		case "toml":
			trimmed := strings.TrimSpace(line)
			if strings.HasPrefix(trimmed, "[") && strings.HasSuffix(trimmed, "]") {
				keys = append(keys, trimmed)
			}
		case "yaml", "yml":
			if !strings.HasPrefix(line, " ") && !strings.HasPrefix(line, "#") && strings.Contains(line, ":") {
				parts := strings.SplitN(line, ":", 2)
				if len(parts) > 0 {
					keys = append(keys, strings.TrimSpace(parts[0]))
				}
			}
		}
	}
	return keys
}

// extractIdentifier extracts a valid identifier from the beginning of a string.
func extractIdentifier(s string) string {
	var name strings.Builder
	for _, c := range s {
		if (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
			(c >= '0' && c <= '9') || c == '_' {
			name.WriteRune(c)
		} else {
			break
		}
	}
	return name.String()
}
