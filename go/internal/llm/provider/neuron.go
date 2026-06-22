package provider

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/opencode-ai/opencode/internal/auth"
	"github.com/opencode-ai/opencode/internal/llm/models"
	"github.com/opencode-ai/opencode/internal/llm/tools"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/message"
)

// NeuronCLI gateway provider — routes through zero-x.live auth server.
// No API keys are stored or needed on the client. The gateway holds all secrets.

// Shared session cache — all NeuronClient instances share one session token.
// The token is cached to disk and fetched asynchronously to avoid blocking TUI startup.
var (
	sharedSessionOnce  sync.Once
	sharedSessionCh    = make(chan struct{}) // closed when session is ready
	sharedSessionToken string
	sharedGatewayURL   string
)

func loadCachedSession() string {
	cache, _, ok := auth.LoadSession()
	if !ok {
		return ""
	}
	return cache.SessionToken
}

func saveCachedSession(session neuronSessionResponse, gatewayURL string) {
	cache := auth.SessionCache{
		SessionToken: session.SessionToken,
		UserID:       session.UserID,
		Email:        session.Email,
		Name:         session.Name,
		ImageURL:     session.ImageURL,
		Plan:         session.Plan,
		GatewayURL:   gatewayURL,
		Provider:     session.Provider,
		Timestamp:    time.Now().Unix(),
		ExpiresAt:    session.ExpiresAt,
		Quota:        session.Quota,
		Usage:        session.Usage,
	}
	if err := auth.SaveSession(cache); err != nil {
		logging.Warn("Failed to cache NeuronCLI session", "error", err)
	}
}

// initSharedSession starts the async session fetch. Call once at startup.
func initSharedSession(gatewayURL string) {
	sharedSessionOnce.Do(func() {
		sharedGatewayURL = gatewayURL

		// Try disk cache first (instant, no network)
		if cached := loadCachedSession(); cached != "" {
			sharedSessionToken = cached
			logging.Info("NeuronCLI session loaded from cache (instant)")
			close(sharedSessionCh)
			return
		}

		// Fetch async — don't block TUI rendering
		go func() {
			defer close(sharedSessionCh)
			fetchGatewaySession(gatewayURL)
		}()
	})
}

func fetchGatewaySession(gatewayURL string) {
	hostname, _ := os.Hostname()
	fingerprint := fmt.Sprintf("%s-%s", os.Getenv("USERNAME"), hostname)
	if fingerprint == "-" {
		fingerprint = "neuron-cli-go"
	}

	body, _ := json.Marshal(map[string]string{
		"machine_fingerprint": fingerprint,
		"version":             "6.2.5",
	})

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Post(
		gatewayURL+"/auth/session",
		"application/json",
		bytes.NewReader(body),
	)
	if err != nil {
		logging.Warn("NeuronCLI gateway unreachable", "url", gatewayURL, "error", err)
		return
	}
	defer resp.Body.Close()

	var session neuronSessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		logging.Warn("Failed to parse gateway session", "error", err)
		return
	}

	if session.SessionToken != "" {
		sharedSessionToken = session.SessionToken
		saveCachedSession(session, gatewayURL)
		logging.Info("NeuronCLI gateway session created",
			"token", session.SessionToken[:16]+"...",
			"models", len(session.Models))
	} else {
		logging.Warn("Gateway returned no session token", "error", session.Error)
	}
}

// waitForSession blocks until the async session fetch completes.
// Called lazily on first send/stream, NOT during startup.
func waitForSession() string {
	<-sharedSessionCh
	return sharedSessionToken
}

type NeuronClient struct {
	gatewayURL string
	model      models.Model
	maxTokens  int64
	systemMsg  string
	httpClient *http.Client
}

func newNeuronClient(opts providerClientOptions) *NeuronClient {
	gatewayURL := os.Getenv("NEURON_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "https://api.zero-x.live"
	}

	// Start async session fetch (idempotent via sync.Once)
	initSharedSession(gatewayURL)

	return &NeuronClient{
		gatewayURL: gatewayURL,
		model:      opts.model,
		maxTokens:  opts.maxTokens,
		systemMsg:  opts.systemMessage,
		httpClient: &http.Client{Timeout: 120 * time.Second},
	}
}

type neuronSessionResponse struct {
	SessionToken string     `json:"session_token"`
	Models       []string   `json:"models"`
	UserID       string     `json:"user_id"`
	Email        string     `json:"email"`
	Name         string     `json:"name"`
	ImageURL     string     `json:"image_url"`
	Plan         string     `json:"plan"`
	Provider     string     `json:"provider"`
	ExpiresAt    int64      `json:"expires_at"`
	Quota        auth.Quota `json:"quota"`
	Usage        auth.Usage `json:"usage"`
	Error        string     `json:"error"`
}

// Converts our internal message format to OpenAI-compatible format for the gateway
func (c *NeuronClient) buildMessages(msgs []message.Message) []map[string]interface{} {
	var result []map[string]interface{}

	// System message first
	if c.systemMsg != "" {
		result = append(result, map[string]interface{}{
			"role":    "system",
			"content": c.systemMsg,
		})
	}

	for _, msg := range msgs {
		// Collect parts by type
		var textParts []string
		var binaryParts []message.BinaryContent
		var toolCalls []message.ToolCall
		var toolResults []message.ToolResult

		for _, part := range msg.Parts {
			switch p := part.(type) {
			case message.TextContent:
				textParts = append(textParts, p.Text)
			case message.BinaryContent:
				binaryParts = append(binaryParts, p)
			case message.ToolCall:
				toolCalls = append(toolCalls, p)
			case message.ToolResult:
				toolResults = append(toolResults, p)
			}
		}

		switch msg.Role {
		case message.Assistant:
			entry := map[string]interface{}{
				"role": "assistant",
			}
			if len(textParts) > 0 {
				entry["content"] = strings.Join(textParts, "\n")
			}
			if len(toolCalls) > 0 {
				var tcList []map[string]interface{}
				for _, tc := range toolCalls {
					tcList = append(tcList, map[string]interface{}{
						"id":   tc.ID,
						"type": "function",
						"function": map[string]interface{}{
							"name":      tc.Name,
							"arguments": tc.Input,
						},
					})
				}
				entry["tool_calls"] = tcList
			}
			result = append(result, entry)

		case message.Tool:
			// Each tool result must be a separate message with role=tool
			for _, tr := range toolResults {
				// Truncate large tool results to save tokens
				content := tr.Content
				if len(content) > 4000 {
					content = content[:2000] + "\n\n... [truncated] ...\n\n" + content[len(content)-1000:]
				}
				result = append(result, map[string]interface{}{
					"role":         "tool",
					"tool_call_id": tr.ToolCallID,
					"content":      content,
				})
			}

		default: // user
			if len(binaryParts) > 0 {
				content := make([]map[string]interface{}, 0, len(binaryParts)+1)
				if len(textParts) > 0 {
					content = append(content, map[string]interface{}{
						"type": "text",
						"text": strings.Join(textParts, "\n"),
					})
				}
				for _, binaryContent := range binaryParts {
					content = append(content, map[string]interface{}{
						"type": "image_url",
						"image_url": map[string]interface{}{
							"url": binaryContent.String(models.ProviderOpenAI),
						},
					})
				}
				result = append(result, map[string]interface{}{
					"role":    "user",
					"content": content,
				})
			} else if len(textParts) > 0 {
				result = append(result, map[string]interface{}{
					"role":    "user",
					"content": strings.Join(textParts, "\n"),
				})
			}
		}
	}

	return result
}

func (c *NeuronClient) buildToolDefs(toolList []tools.BaseTool) []map[string]interface{} {
	var defs []map[string]interface{}
	for _, t := range toolList {
		info := t.Info()
		def := map[string]interface{}{
			"type": "function",
			"function": map[string]interface{}{
				"name":        info.Name,
				"description": info.Description,
				"parameters": map[string]interface{}{
					"type":       "object",
					"properties": info.Parameters,
					"required":   info.Required,
				},
			},
		}
		defs = append(defs, def)
	}
	return defs
}

func (c *NeuronClient) send(ctx context.Context, msgs []message.Message, toolList []tools.BaseTool) (*ProviderResponse, error) {
	// Lazy: wait for async session (instant if cached)
	sessionToken := waitForSession()
	if sessionToken == "" {
		return nil, fmt.Errorf("no gateway session — is the NeuronCLI server running at %s?", c.gatewayURL)
	}

	body := map[string]interface{}{
		"model":      c.model.APIModel,
		"messages":   c.buildMessages(msgs),
		"max_tokens": c.maxTokens,
		"stream":     false,
	}
	if len(toolList) > 0 {
		body["tools"] = c.buildToolDefs(toolList)
	}

	bodyBytes, _ := json.Marshal(body)
	req, _ := http.NewRequestWithContext(ctx, "POST",
		c.gatewayURL+"/v1/chat/completions",
		bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+sessionToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("gateway request failed: %w", err)
	}
	defer resp.Body.Close()

	var result struct {
		Choices []struct {
			Message struct {
				Content          string `json:"content"`
				ReasoningContent string `json:"reasoning_content"`
				ToolCalls        []struct {
					ID       string `json:"id"`
					Function struct {
						Name      string `json:"name"`
						Arguments string `json:"arguments"`
					} `json:"function"`
				} `json:"tool_calls"`
			} `json:"message"`
			FinishReason string `json:"finish_reason"`
		} `json:"choices"`
		Usage struct {
			PromptTokens     int64 `json:"prompt_tokens"`
			CompletionTokens int64 `json:"completion_tokens"`
		} `json:"usage"`
	}

	if resp.StatusCode != 200 {
		errBody, _ := io.ReadAll(resp.Body)
		logging.ErrorPersist(fmt.Sprintf("[NeuronClient.send] HTTP %d | URL: %s | Body: %s",
			resp.StatusCode,
			c.gatewayURL+"/v1/chat/completions",
			string(errBody[:min(500, len(errBody))]),
		))
		return nil, fmt.Errorf("gateway returned %d: %s", resp.StatusCode, string(errBody))
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("failed to parse gateway response: %w", err)
	}

	response := &ProviderResponse{
		Usage: TokenUsage{
			InputTokens:  result.Usage.PromptTokens,
			OutputTokens: result.Usage.CompletionTokens,
		},
	}

	if len(result.Choices) > 0 {
		choice := result.Choices[0]
		// Use content if available, fall back to reasoning_content for reasoning models
		response.Content = choice.Message.Content
		if response.Content == "" && choice.Message.ReasoningContent != "" {
			response.Content = choice.Message.ReasoningContent
		}

		for _, tc := range choice.Message.ToolCalls {
			response.ToolCalls = append(response.ToolCalls, message.ToolCall{
				ID:    tc.ID,
				Name:  tc.Function.Name,
				Input: tc.Function.Arguments,
			})
		}

		switch choice.FinishReason {
		case "stop":
			response.FinishReason = message.FinishReasonEndTurn
		case "tool_calls":
			response.FinishReason = message.FinishReasonToolUse
		case "length":
			response.FinishReason = message.FinishReasonMaxTokens
		}
	}

	return response, nil
}

func (c *NeuronClient) stream(ctx context.Context, msgs []message.Message, toolList []tools.BaseTool) <-chan ProviderEvent {
	ch := make(chan ProviderEvent, 64)

	go func() {
		defer close(ch)

		// Lazy: wait for async session (instant if cached)
		sessionToken := waitForSession()
		if sessionToken == "" {
			ch <- ProviderEvent{
				Type:  EventError,
				Error: fmt.Errorf("no gateway session — start the NeuronCLI server at %s", c.gatewayURL),
			}
			return
		}

		body := map[string]interface{}{
			"model":      c.model.APIModel,
			"messages":   c.buildMessages(msgs),
			"max_tokens": c.maxTokens,
			"stream":     true,
		}
		if len(toolList) > 0 {
			body["tools"] = c.buildToolDefs(toolList)
		}

		bodyBytes, _ := json.Marshal(body)

		// Retry loop for rate limits and transient errors
		var resp *http.Response
		for attempt := 0; attempt < 5; attempt++ {
			bodyReader := bytes.NewReader(bodyBytes)
			req, _ := http.NewRequestWithContext(ctx, "POST",
				c.gatewayURL+"/v1/chat/completions",
				bodyReader)
			req.Header.Set("Content-Type", "application/json")
			req.Header.Set("Authorization", "Bearer "+sessionToken)
			req.Header.Set("Accept", "text/event-stream")

			var err error
			resp, err = c.httpClient.Do(req)
			if err != nil {
				ch <- ProviderEvent{Type: EventError, Error: err}
				return
			}

			// Retry on 429 (rate limit)
			if resp.StatusCode == 429 {
				resp.Body.Close()
				backoff := time.Duration(2<<uint(attempt)) * time.Second
				logging.WarnPersist(fmt.Sprintf("Rate limited (429), retrying in %s... (attempt %d/5)", backoff, attempt+1))
				select {
				case <-ctx.Done():
					ch <- ProviderEvent{Type: EventError, Error: ctx.Err()}
					return
				case <-time.After(backoff):
					continue
				}
			}

			// Retry on "model output" error (Azure returns this when model produces empty response)
			if resp.StatusCode != 200 {
				respBody, _ := io.ReadAll(resp.Body)
				resp.Body.Close()
				if strings.Contains(string(respBody), "model output") {
					backoff := time.Duration(2<<uint(attempt)) * time.Second
					logging.WarnPersist(fmt.Sprintf("Empty model output, retrying in %s... (attempt %d/5)", backoff, attempt+1))
					select {
					case <-ctx.Done():
						ch <- ProviderEvent{Type: EventError, Error: ctx.Err()}
						return
					case <-time.After(backoff):
						continue
					}
				}
				// Non-retryable error
				errMsg := fmt.Sprintf("gateway returned %d: %s", resp.StatusCode, string(respBody))
				logging.ErrorPersist(fmt.Sprintf("[NeuronClient.stream] HTTP %d | Body: %s",
					resp.StatusCode, string(respBody[:min(500, len(respBody))])))
				ch <- ProviderEvent{Type: EventError, Error: fmt.Errorf("%s", errMsg)}
				return
			}

			break // success
		}
		defer resp.Body.Close()

		ch <- ProviderEvent{Type: EventContentStart}

		// Create a pipe so we can read with context cancellation.
		// Without this, bufio.Scanner blocks forever on a stalled stream.
		pr, pw := io.Pipe()
		go func() {
			defer pw.Close()
			buf := make([]byte, 32*1024)
			for {
				// Set a deadline: if no data in 90s, we're stalled
				n, err := resp.Body.Read(buf)
				if n > 0 {
					pw.Write(buf[:n])
				}
				if err != nil {
					if err != io.EOF {
						pw.CloseWithError(err)
					}
					return
				}
				// Check context
				select {
				case <-ctx.Done():
					pw.CloseWithError(ctx.Err())
					return
				default:
				}
			}
		}()

		scanner := bufio.NewScanner(pr)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

		var currentToolCall *message.ToolCall
		var toolInputBuffer strings.Builder
		var accumulatedContent strings.Builder
		var accumulatedReasoning strings.Builder
		var accumulatedToolCalls []message.ToolCall
		var finalUsage TokenUsage
		finalFinishReason := message.FinishReasonEndTurn

		for scanner.Scan() {
			line := scanner.Text()

			if !strings.HasPrefix(line, "data: ") {
				continue
			}

			data := strings.TrimPrefix(line, "data: ")
			if data == "[DONE]" {
				// Flush any pending tool call
				if currentToolCall != nil {
					currentToolCall.Input = toolInputBuffer.String()
					accumulatedToolCalls = append(accumulatedToolCalls, *currentToolCall)
					ch <- ProviderEvent{
						Type:     EventToolUseStop,
						ToolCall: currentToolCall,
					}
					currentToolCall = nil
				}

				if len(accumulatedToolCalls) > 0 {
					finalFinishReason = message.FinishReasonToolUse
				}

				// Fall back to reasoning_content if content is empty
				// (Kimi K2.5 sometimes returns only reasoning_content)
				// Note: reasoning was already streamed live as EventContentDelta,
				// so we don't emit it again here — just set finalContent for the response.
				finalContent := accumulatedContent.String()
				if finalContent == "" && accumulatedReasoning.Len() > 0 {
					finalContent = accumulatedReasoning.String()
				}

				ch <- ProviderEvent{
					Type: EventComplete,
					Response: &ProviderResponse{
						Content:      finalContent,
						ToolCalls:    accumulatedToolCalls,
						Usage:        finalUsage,
						FinishReason: finalFinishReason,
					},
				}
				return
			}

			var chunk struct {
				Choices []struct {
					Delta struct {
						Content          string `json:"content"`
						ReasoningContent string `json:"reasoning_content"`
						ToolCalls        []struct {
							Index    int    `json:"index"`
							ID       string `json:"id"`
							Function struct {
								Name      string `json:"name"`
								Arguments string `json:"arguments"`
							} `json:"function"`
						} `json:"tool_calls"`
					} `json:"delta"`
					FinishReason *string `json:"finish_reason"`
				} `json:"choices"`
				Usage *struct {
					PromptTokens     int64 `json:"prompt_tokens"`
					CompletionTokens int64 `json:"completion_tokens"`
				} `json:"usage"`
			}

			if err := json.Unmarshal([]byte(data), &chunk); err != nil {
				continue
			}

			if len(chunk.Choices) == 0 {
				continue
			}

			delta := chunk.Choices[0].Delta

			// Reasoning content — stored separately, shown as status indicator
			if delta.ReasoningContent != "" {
				accumulatedReasoning.WriteString(delta.ReasoningContent)
				ch <- ProviderEvent{
					Type:    EventThinkingDelta,
					Content: delta.ReasoningContent,
				}
			}

			// Text content
			if delta.Content != "" {
				accumulatedContent.WriteString(delta.Content)
				ch <- ProviderEvent{
					Type:    EventContentDelta,
					Content: delta.Content,
				}
			}

			// Tool calls
			for _, tc := range delta.ToolCalls {
				if tc.ID != "" {
					// New tool call starting — flush previous if any
					if currentToolCall != nil {
						currentToolCall.Input = toolInputBuffer.String()
						accumulatedToolCalls = append(accumulatedToolCalls, *currentToolCall)
						ch <- ProviderEvent{
							Type:     EventToolUseStop,
							ToolCall: currentToolCall,
						}
					}

					currentToolCall = &message.ToolCall{
						ID:   tc.ID,
						Name: tc.Function.Name,
					}
					toolInputBuffer.Reset()
					toolInputBuffer.WriteString(tc.Function.Arguments)

					ch <- ProviderEvent{
						Type:     EventToolUseStart,
						ToolCall: currentToolCall,
					}
				} else if tc.Function.Arguments != "" {
					toolInputBuffer.WriteString(tc.Function.Arguments)
					ch <- ProviderEvent{
						Type:    EventToolUseDelta,
						Content: tc.Function.Arguments,
					}
				}
			}

			// Finish reason
			if chunk.Choices[0].FinishReason != nil {
				switch *chunk.Choices[0].FinishReason {
				case "stop":
					finalFinishReason = message.FinishReasonEndTurn
				case "tool_calls":
					finalFinishReason = message.FinishReasonToolUse
				case "length":
					finalFinishReason = message.FinishReasonMaxTokens
				}
			}

			// Usage info
			if chunk.Usage != nil {
				finalUsage = TokenUsage{
					InputTokens:  chunk.Usage.PromptTokens,
					OutputTokens: chunk.Usage.CompletionTokens,
				}
			}
		}

		// If we got here without [DONE], still complete
		if currentToolCall != nil {
			currentToolCall.Input = toolInputBuffer.String()
			accumulatedToolCalls = append(accumulatedToolCalls, *currentToolCall)
			ch <- ProviderEvent{
				Type:     EventToolUseStop,
				ToolCall: currentToolCall,
			}
		}
		if len(accumulatedToolCalls) > 0 {
			finalFinishReason = message.FinishReasonToolUse
		}
		finalContent := accumulatedContent.String()
		if finalContent == "" && accumulatedReasoning.Len() > 0 {
			finalContent = accumulatedReasoning.String()
			ch <- ProviderEvent{
				Type:    EventContentDelta,
				Content: finalContent,
			}
		}
		ch <- ProviderEvent{
			Type: EventComplete,
			Response: &ProviderResponse{
				Content:      finalContent,
				ToolCalls:    accumulatedToolCalls,
				Usage:        finalUsage,
				FinishReason: finalFinishReason,
			},
		}
	}()

	return ch
}
