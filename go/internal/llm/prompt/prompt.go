package prompt

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/llm/models"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/repomap"
)

func GetAgentPrompt(agentName config.AgentName, provider models.ModelProvider) string {
	basePrompt := ""
	switch agentName {
	case config.AgentCoder:
		basePrompt = CoderPrompt(provider)
	case config.AgentTitle:
		basePrompt = TitlePrompt(provider)
	case config.AgentTask:
		basePrompt = TaskPrompt(provider)
	case config.AgentSummarizer:
		basePrompt = SummarizerPrompt(provider)
	default:
		basePrompt = "You are a helpful assistant"
	}

	if agentName == config.AgentCoder || agentName == config.AgentTask {
		// Inject repo map (Windsurf Cascade-style codebase context)
		repoContext := getRepoMapContext()
		if repoContext != "" {
			basePrompt = fmt.Sprintf("%s\n\n%s", basePrompt, repoContext)
		}

		// Add context from project-specific instruction files if they exist
		contextContent := getContextFromPaths()
		logging.Debug("Context content", "Context", contextContent)
		if contextContent != "" {
			return fmt.Sprintf("%s\n\n# Project-Specific Context\n Make sure to follow the instructions in the context below\n%s", basePrompt, contextContent)
		}
	}
	return basePrompt
}

var (
	onceContext    sync.Once
	contextContent string
	promptMu       sync.Mutex
)

// ResetWorkspaceCaches forces project context and repo-map content to be
// rebuilt after the active workspace changes.
func ResetWorkspaceCaches() {
	promptMu.Lock()
	defer promptMu.Unlock()
	onceContext = sync.Once{}
	contextContent = ""
	onceRepoMap = sync.Once{}
	repoMapContent = ""
}

func getContextFromPaths() string {
	promptMu.Lock()
	defer promptMu.Unlock()

	onceContext.Do(func() {
		var (
			cfg          = config.Get()
			workDir      = cfg.WorkingDir
			contextPaths = cfg.ContextPaths
		)

		contextContent = processContextPaths(workDir, contextPaths)
	})

	return contextContent
}

func processContextPaths(workDir string, paths []string) string {
	processedFiles := make(map[string]bool)
	results := make([]string, 0)

	for _, path := range paths {
		if strings.HasSuffix(path, "/") {
			_ = filepath.WalkDir(filepath.Join(workDir, path), func(path string, d os.DirEntry, err error) error {
				if err != nil {
					return err
				}
				if !d.IsDir() {
					lowerPath := strings.ToLower(path)
					if !processedFiles[lowerPath] {
						processedFiles[lowerPath] = true
						if result := processFile(path); result != "" {
							results = append(results, result)
						}
					}
				}
				return nil
			})
			continue
		}

		fullPath := filepath.Join(workDir, path)
		lowerPath := strings.ToLower(fullPath)
		if !processedFiles[lowerPath] {
			processedFiles[lowerPath] = true
			result := processFile(fullPath)
			if result != "" {
				results = append(results, result)
			}
		}
	}

	return strings.Join(results, "\n")
}

func processFile(filePath string) string {
	content, err := os.ReadFile(filePath)
	if err != nil {
		return ""
	}
	return "# From:" + filepath.ToSlash(filePath) + "\n" + string(content)
}

// Repo map — Windsurf Cascade equivalent
var (
	onceRepoMap    sync.Once
	repoMapContent string
)

func getRepoMapContext() string {
	promptMu.Lock()
	defer promptMu.Unlock()

	onceRepoMap.Do(func() {
		cfg := config.Get()
		workDir := cfg.WorkingDir
		if workDir == "" {
			return
		}

		rm := repomap.Build(workDir)
		if rm.TotalFiles > 0 {
			repoMapContent = rm.Render()
			logging.Info("Repo map built", "files", rm.TotalFiles, "symbols", rm.TotalSymbols)
		}
	})
	return repoMapContent
}
