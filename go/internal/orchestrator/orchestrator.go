// Package orchestrator implements NeuronCLI's multi-model orchestration modes.
// These modes route through the zero-x.live gateway — no direct API keys needed.
//
// Model-to-Feature Mapping:
//
//	DEFAULT mode  → Kimi-K2.5 (single model, agentic)
//	/chain  mode  → Kimi-K2.5 (architect) → DeepSeek-V3.2 (coder) → MiniMax-M2.5 (reviewer)
//	/power  mode  → Kimi-K2.5 + DeepSeek-V3.2 + MiniMax-M2.5 (parallel) → model-router (merge)
//	/divide mode  → Kimi-K2.5 (prompt-only, per-file splitting)
package orchestrator

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"sync"
	"time"

	"github.com/opencode-ai/opencode/internal/logging"
)

// ── Model roster ────────────────────────────────────────────
// All models are Azure AI Foundry deployments served via the gateway.

// Models for chain mode
const (
	ModelArchitect = "Kimi-K2.5"       // Phase 1: designs architecture
	ModelCoder     = "FW-DeepSeek-V3.2" // Phase 2: implements code
	ModelReviewer  = "FW-MiniMax-M2.5"  // Phase 3: reviews and hardens
	ModelMerge     = "model-router"     // Power mode merge agent
)

// Models for power mode (parallel ensemble)
var PowerModels = []string{
	"Kimi-K2.5",
	"FW-DeepSeek-V3.2",
	"FW-MiniMax-M2.5",
}

type ModelResponse struct {
	Model   string
	Content string
	Tokens  int
}

// ── Gateway communication ───────────────────────────────────

func gatewayURL() string {
	url := os.Getenv("NEURON_GATEWAY_URL")
	if url == "" {
		url = "https://api.zero-x.live"
	}
	return url
}

// gatewayCall makes a chat completion request through the NeuronCLI gateway.
// The gateway holds all API keys — client just sends session token.
func gatewayCall(model string, messages []map[string]string, maxTokens int, sessionToken string) (*ModelResponse, error) {
	body := map[string]interface{}{
		"model":      model,
		"messages":   messages,
		"max_tokens": maxTokens,
		"stream":     false,
	}

	bodyBytes, _ := json.Marshal(body)

	client := &http.Client{Timeout: 120 * time.Second}
	req, _ := http.NewRequest("POST", gatewayURL()+"/v1/chat/completions", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	if sessionToken != "" {
		req.Header.Set("Authorization", "Bearer "+sessionToken)
	}

	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("gateway unreachable: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		b, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("gateway %d: %s", resp.StatusCode, string(b)[:min(len(b), 200)])
	}

	var result struct {
		Choices []struct {
			Message struct {
				Content          string `json:"content"`
				ReasoningContent string `json:"reasoning_content"`
			} `json:"message"`
		} `json:"choices"`
		Usage struct {
			TotalTokens int `json:"total_tokens"`
		} `json:"usage"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("parse error: %w", err)
	}

	content := ""
	if len(result.Choices) > 0 {
		content = result.Choices[0].Message.Content
		if content == "" {
			content = result.Choices[0].Message.ReasoningContent
		}
	}

	return &ModelResponse{
		Model:   model,
		Content: content,
		Tokens:  result.Usage.TotalTokens,
	}, nil
}

// getSessionToken creates a gateway session and returns the token.
func getSessionToken() string {
	hostname, _ := os.Hostname()
	fingerprint := fmt.Sprintf("%s-%s", os.Getenv("USERNAME"), hostname)
	if fingerprint == "-" {
		fingerprint = "neuron-orchestrator"
	}

	body, _ := json.Marshal(map[string]string{
		"machine_fingerprint": fingerprint,
		"version":            "7.0.0",
	})

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Post(gatewayURL()+"/auth/session", "application/json", bytes.NewReader(body))
	if err != nil {
		logging.Warn("Gateway session failed", "error", err)
		return ""
	}
	defer resp.Body.Close()

	var session struct {
		SessionToken string `json:"session_token"`
	}
	json.NewDecoder(resp.Body).Decode(&session)
	return session.SessionToken
}

// ── Chain Mode ──────────────────────────────────────────────
// Pipeline: Architect (Kimi) → Coder (DeepSeek) → Reviewer (MiniMax)

func RunChain(userInput string) string {
	token := getSessionToken()
	if token == "" {
		logging.Warn("Chain mode: no gateway session, falling back to single-model")
		return userInput
	}

	// Phase 1: Architect
	logging.Info("[chain] Phase 1: architect", "model", ModelArchitect)
	archMsgs := []map[string]string{{
		"role": "user",
		"content": fmt.Sprintf(
			"You are a senior software architect. Design the approach for this task.\n"+
				"Output ONLY the technical design:\n"+
				"- Components needed and why\n- Data flow and interfaces\n- Edge cases\n"+
				"NO code — just architecture.\n\nTask: %s", userInput),
	}}
	archResult, err := gatewayCall(ModelArchitect, archMsgs, 4000, token)
	archContent := ""
	if err != nil {
		logging.Warn("[chain] Architect failed", "error", err)
		archContent = fmt.Sprintf("(architect unavailable) Task: %s", userInput)
	} else {
		logging.Info("[chain] Architect OK", "tokens", archResult.Tokens)
		archContent = archResult.Content
	}

	// Phase 2: Coder
	logging.Info("[chain] Phase 2: coder", "model", ModelCoder)
	codeMsgs := []map[string]string{
		{"role": "system", "content": "You are an expert coder. Implement the architect's design with clean, production-quality code. Include error handling, type hints, docstrings."},
		{"role": "user", "content": fmt.Sprintf("ARCHITECTURE DESIGN:\n%s\n\nORIGINAL TASK:\n%s\n\nImplement this now. Write complete, working code files.", archContent, userInput)},
	}
	codeResult, err := gatewayCall(ModelCoder, codeMsgs, 8000, token)
	codeContent := ""
	if err != nil {
		logging.Warn("[chain] Coder failed", "error", err)
		codeContent = archContent
	} else {
		logging.Info("[chain] Coder OK", "tokens", codeResult.Tokens)
		codeContent = codeResult.Content
	}

	// Phase 3: Reviewer
	logging.Info("[chain] Phase 3: reviewer", "model", ModelReviewer)
	reviewMsgs := []map[string]string{
		{"role": "system", "content": "You are a senior code reviewer. Review the code below. Find bugs, security issues, missing error handling. Output the FIXED final code."},
		{"role": "user", "content": fmt.Sprintf("ARCHITECTURE:\n%s\n\nCODE TO REVIEW:\n%s\n\nReview and output the hardened, fixed code.", archContent, codeContent)},
	}
	reviewResult, err := gatewayCall(ModelReviewer, reviewMsgs, 8000, token)
	reviewContent := ""
	if err != nil {
		logging.Warn("[chain] Reviewer failed", "error", err)
		reviewContent = codeContent
	} else {
		logging.Info("[chain] Reviewer OK", "tokens", reviewResult.Tokens)
		reviewContent = reviewResult.Content
	}

	return fmt.Sprintf(
		"[CHAIN MODE — 3 MODELS COMPLETED]\n"+
			"Three specialized models have processed this task:\n\n"+
			"=== ARCHITECT (%s) ===\n%s\n\n"+
			"=== CODER (%s) ===\n%s\n\n"+
			"=== REVIEWER (%s) ===\n%s\n\n"+
			"Execute the REVIEWER's final output using your tools (write_file, bash, etc.).\n"+
			"Write the files exactly as the reviewer specified.\n"+
			"Original task: %s\n",
		ModelArchitect, archContent,
		ModelCoder, codeContent,
		ModelReviewer, reviewContent,
		userInput)
}

// ── Power Mode ──────────────────────────────────────────────
// Parallel ensemble: 3 models run simultaneously → merge agent combines best parts

func RunPower(userInput string) string {
	token := getSessionToken()
	if token == "" {
		logging.Warn("Power mode: no gateway session, falling back to single-model")
		return userInput
	}

	// Spawn all agents in parallel
	var wg sync.WaitGroup
	results := make([]*ModelResponse, len(PowerModels))
	errors := make([]error, len(PowerModels))

	for i, model := range PowerModels {
		wg.Add(1)
		go func(idx int, modelName string) {
			defer wg.Done()
			logging.Info("[power] Agent started", "index", idx+1, "model", modelName)

			msgs := []map[string]string{{
				"role": "user",
				"content": fmt.Sprintf(
					"You are an expert developer. Solve this task with maximum quality.\n"+
						"Write complete, production-ready code.\n\nTask: %s", userInput),
			}}

			resp, err := gatewayCall(modelName, msgs, 4000, token)
			if err != nil {
				logging.Warn("[power] Agent failed", "model", modelName, "error", err)
				errors[idx] = err
				return
			}
			logging.Info("[power] Agent OK", "model", modelName, "tokens", resp.Tokens)
			results[idx] = resp
		}(i, model)
	}

	wg.Wait()

	// Collect successful results
	var successful []*ModelResponse
	for _, r := range results {
		if r != nil && r.Content != "" {
			successful = append(successful, r)
		}
	}

	if len(successful) == 0 {
		return userInput
	}

	if len(successful) == 1 {
		return fmt.Sprintf(
			"[POWER MODE — SINGLE MODEL]\nOnly one model produced a solution. Using output from %s.\n\n%s\n\n"+
				"Execute this code using your tools (write_file, bash, etc.).\nOriginal task: %s\n",
			successful[0].Model, successful[0].Content, userInput)
	}

	// Merge phase: combine all solutions
	logging.Info("[power] Merging solutions", "count", len(successful), "merge_model", ModelMerge)

	mergeContent := "You are a merge agent. Below are solutions from multiple AI models.\n" +
		"COMBINE the BEST PARTS into ONE concise solution (max 150 lines):\n" +
		"- Pick best algorithm\n- Pick best structure\n- Pick best error handling\n" +
		"Produce ONE merged implementation. Be concise.\n\n"

	for _, r := range successful {
		content := r.Content
		if len(content) > 3000 {
			content = content[:3000] + "\n...(truncated)"
		}
		mergeContent += fmt.Sprintf("=== %s ===\n%s\n\n", r.Model, content)
	}
	mergeContent += fmt.Sprintf("TASK: %s\n", userInput)

	mergeMsgs := []map[string]string{{"role": "user", "content": mergeContent}}
	mergeResult, err := gatewayCall(ModelMerge, mergeMsgs, 4000, token)

	if err != nil {
		logging.Warn("[power] Merge failed, using first result", "error", err)
		return fmt.Sprintf(
			"[POWER MODE — FALLBACK]\nMerge failed, using best single output from %s.\n\n%s\n\n"+
				"Execute this code. Original task: %s\n",
			successful[0].Model, successful[0].Content, userInput)
	}

	logging.Info("[power] Merge OK", "tokens", mergeResult.Tokens)
	return fmt.Sprintf(
		"[POWER MODE — %d MODELS MERGED]\n"+
			"Multiple models generated solutions IN PARALLEL, a merge agent combined the best parts.\n\n"+
			"=== MERGED RESULT ===\n%s\n\n"+
			"Execute this merged code using your tools (write_file, bash, etc.).\n"+
			"Write the files exactly as specified.\nOriginal task: %s\n",
		len(successful), mergeResult.Content, userInput)
}

// ── Divide Mode ─────────────────────────────────────────────
// Prompt-only: instructs the main runtime model to split work per file/module

func BuildDividePrompt(userInput string) string {
	return fmt.Sprintf(
		"[DIVIDE MODE — MULTI-FILE PARALLEL STRATEGY]\n"+
			"You are operating in DIVIDE mode. Follow this workflow:\n\n"+
			"1. ANALYZE the task and identify all files/modules needed.\n"+
			"2. For EACH file, act as a specialized sub-agent.\n"+
			"3. Design each file independently with clean interfaces.\n"+
			"4. After generating all files, act as INTEGRATOR:\n"+
			"   - Ensure imports/exports are consistent\n"+
			"   - Verify shared state and data flow\n"+
			"   - Fix cross-file dependencies\n"+
			"5. Summary of files created and how they connect.\n\n"+
			"User's request: %s\n", userInput)
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
