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
	"time"

	"github.com/opencode-ai/opencode/internal/llm/models"
	"github.com/opencode-ai/opencode/internal/llm/tools"
	"github.com/opencode-ai/opencode/internal/logging"
	"github.com/opencode-ai/opencode/internal/message"
)

// NeuronCLI gateway provider — routes through zero-x.live auth server.
// No API keys are stored or needed on the client. The gateway holds all secrets.

type NeuronClient struct {
	gatewayURL   string
	sessionToken string
	model        models.Model
	maxTokens    int64
	systemMsg    string
	httpClient   *http.Client
}

func newNeuronClient(opts providerClientOptions) *NeuronClient {
	gatewayURL := os.Getenv("NEURON_GATEWAY_URL")
	if gatewayURL == "" {
		// Default: localhost for dev (same machine as server.js)
		// For production zero-x.live, set NEURON_GATEWAY_URL=https://zero-x.live/neuroncli
		gatewayURL = "http://localhost:19284"
	}

	client := &NeuronClient{
		gatewayURL: gatewayURL,
		model:      opts.model,
		maxTokens:  opts.maxTokens,
		systemMsg:  opts.systemMessage,
		httpClient: &http.Client{Timeout: 120 * time.Second},
	}

	// Create session on initialization
	client.createSession()
	return client
}

type neuronSessionResponse struct {
	SessionToken string   `json:"session_token"`
	Models       []string `json:"models"`
	Error        string   `json:"error"`
}

func (c *NeuronClient) createSession() {
	hostname, _ := os.Hostname()
	fingerprint := fmt.Sprintf("%s-%s", os.Getenv("USERNAME"), hostname)
	if fingerprint == "-" {
		fingerprint = "neuron-cli-go"
	}

	body, _ := json.Marshal(map[string]string{
		"machine_fingerprint": fingerprint,
		"version":            "6.2.5",
	})

	resp, err := c.httpClient.Post(
		c.gatewayURL+"/auth/session",
		"application/json",
		bytes.NewReader(body),
	)
	if err != nil {
		logging.Warn("NeuronCLI gateway unreachable", "url", c.gatewayURL, "error", err)
		return
	}
	defer resp.Body.Close()

	var session neuronSessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		logging.Warn("Failed to parse gateway session", "error", err)
		return
	}

	if session.SessionToken != "" {
		c.sessionToken = session.SessionToken
		logging.Info("NeuronCLI gateway session created",
			"token", session.SessionToken[:16]+"...",
			"models", len(session.Models))
	} else {
		logging.Warn("Gateway returned no session token", "error", session.Error)
	}
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
		role := "user"
		if msg.Role == message.Assistant {
			role = "assistant"
		}

		// Build content from parts
		var contentParts []string
		for _, part := range msg.Parts {
			switch p := part.(type) {
			case message.TextContent:
				contentParts = append(contentParts, p.Text)
			}
		}

		if len(contentParts) > 0 {
			result = append(result, map[string]interface{}{
				"role":    role,
				"content": strings.Join(contentParts, "\n"),
			})
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
				"parameters":  info.Parameters,
			},
		}
		defs = append(defs, def)
	}
	return defs
}

func (c *NeuronClient) send(ctx context.Context, msgs []message.Message, toolList []tools.BaseTool) (*ProviderResponse, error) {
	if c.sessionToken == "" {
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
	req.Header.Set("Authorization", "Bearer "+c.sessionToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("gateway request failed: %w", err)
	}
	defer resp.Body.Close()

	var result struct {
		Choices []struct {
			Message struct {
				Content   string `json:"content"`
				ToolCalls []struct {
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
		response.Content = choice.Message.Content

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

		if c.sessionToken == "" {
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
		req, _ := http.NewRequestWithContext(ctx, "POST",
			c.gatewayURL+"/v1/chat/completions",
			bytes.NewReader(bodyBytes))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Authorization", "Bearer "+c.sessionToken)
		req.Header.Set("Accept", "text/event-stream")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			ch <- ProviderEvent{Type: EventError, Error: err}
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode != 200 {
			bodyBytes, _ := io.ReadAll(resp.Body)
			errMsg := fmt.Sprintf("gateway returned %d: %s", resp.StatusCode, string(bodyBytes))
			logging.ErrorPersist(fmt.Sprintf("[NeuronClient.stream] HTTP %d | URL: %s | Token: %s... | Body: %s",
				resp.StatusCode,
				c.gatewayURL+"/v1/chat/completions",
				c.sessionToken[:min(16, len(c.sessionToken))],
				string(bodyBytes[:min(500, len(bodyBytes))]),
			))
			ch <- ProviderEvent{
				Type:  EventError,
				Error: fmt.Errorf("%s", errMsg),
			}
			return
		}

		ch <- ProviderEvent{Type: EventContentStart}

		scanner := bufio.NewScanner(resp.Body)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

		var currentToolCall *message.ToolCall
		var toolInputBuffer strings.Builder

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
					ch <- ProviderEvent{
						Type:     EventToolUseStop,
						ToolCall: currentToolCall,
					}
					currentToolCall = nil
				}

				ch <- ProviderEvent{Type: EventComplete}
				return
			}

			var chunk struct {
				Choices []struct {
					Delta struct {
						Content   string `json:"content"`
						ToolCalls []struct {
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

			// Text content
			if delta.Content != "" {
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

			// Usage info
			if chunk.Usage != nil {
				// We'll send this with the complete event
			}
		}

		// If we got here without [DONE], still complete
		if currentToolCall != nil {
			currentToolCall.Input = toolInputBuffer.String()
			ch <- ProviderEvent{
				Type:     EventToolUseStop,
				ToolCall: currentToolCall,
			}
		}
		ch <- ProviderEvent{Type: EventComplete}
	}()

	return ch
}
