package main

// Coding REPL connected to the Neuron gateway (api.zero-x.live).
// Uses the same auth flow as the full Neuron app:
//   1. Load session token from ~/.neuron/session_cache.json
//   2. If expired, fetch a new one from POST /auth/session
//   3. Stream chat completions from POST /v1/chat/completions

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
)

// ── Gateway config ────────────────────────────────────────────────────

const (
	defaultGatewayURL  = "https://api.zero-x.live"
	sessionCacheMaxAge = 12 * 60 * 60 // 12 hours in seconds
)

// Available models — use the APIModel name (what the gateway expects), not the ModelID.
type neuronModel struct {
	displayName string
	apiModel    string
}

var availableModels = []neuronModel{
	{"GPT-5.4 Mini", "gpt-5.4-mini"},
	{"GPT-5.4 Pro", "gpt-5.4-pro"},
	{"GPT-5.5", "gpt-5.5-2"},
	{"Codex Max", "gpt-5.1-codex-max"},
	{"Kimi K2.5", "Kimi-K2.5"},
	{"Kimi K2.6", "Kimi-K2.6"},
	{"DeepSeek V4 Flash", "DeepSeek-V4-Flash"},
	{"DeepSeek V3.2", "FW-DeepSeek-V3.2"},
	{"MiniMax M2.5", "FW-MiniMax-M2.5"},
	{"Model Router", "model-router"},
}

var defaultModelIndex = 4 // Kimi K2.5

// ── Session cache ─────────────────────────────────────────────────────

type sessionCacheFile struct {
	Token     string `json:"token"`
	Timestamp int64  `json:"timestamp"`
}

func gatewayURL() string {
	if u := os.Getenv("NEURON_GATEWAY_URL"); u != "" {
		return u
	}
	return defaultGatewayURL
}

func sessionCachePath() string {
	home, _ := os.UserHomeDir()
	return home + "/.neuron/session_cache.json"
}

func loadSessionToken() string {
	data, err := os.ReadFile(sessionCachePath())
	if err != nil {
		return ""
	}
	var cache sessionCacheFile
	if err := json.Unmarshal(data, &cache); err != nil {
		return ""
	}
	if time.Now().Unix()-cache.Timestamp > sessionCacheMaxAge {
		return ""
	}
	return cache.Token
}

func saveSessionToken(token string) {
	path := sessionCachePath()
	dir := path[:strings.LastIndex(path, "/")]
	os.MkdirAll(dir, 0755)
	data, _ := json.Marshal(sessionCacheFile{Token: token, Timestamp: time.Now().Unix()})
	os.WriteFile(path, data, 0644)
}

func fetchSessionToken() (string, error) {
	hostname, _ := os.Hostname()
	fingerprint := fmt.Sprintf("%s-%s", os.Getenv("USERNAME"), hostname)
	if fingerprint == "-" {
		fingerprint = "neuron-cli-repl"
	}

	body, _ := json.Marshal(map[string]string{
		"machine_fingerprint": fingerprint,
		"version":            "6.2.5",
	})

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Post(
		gatewayURL()+"/auth/session",
		"application/json",
		bytes.NewReader(body),
	)
	if err != nil {
		return "", fmt.Errorf("gateway unreachable: %w", err)
	}
	defer resp.Body.Close()

	var result struct {
		SessionToken string   `json:"session_token"`
		Models       []string `json:"models"`
		Error        string   `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return "", fmt.Errorf("invalid response: %w", err)
	}
	if result.SessionToken == "" {
		return "", fmt.Errorf("no token: %s", result.Error)
	}

	saveSessionToken(result.SessionToken)
	return result.SessionToken, nil
}

func getSessionToken() (string, error) {
	if token := loadSessionToken(); token != "" {
		return token, nil
	}
	return fetchSessionToken()
}

// ── Chat history ──────────────────────────────────────────────────────

type chatMsg struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// ── Bubbletea messages ────────────────────────────────────────────────

type replStreamChunk struct {
	text string
}
type replStreamDone struct {
	fullText string
	err      error
}

// ── Streaming request ─────────────────────────────────────────────────

func streamChatCompletion(token string, history []chatMsg, model string) <-chan tea.Msg {
	ch := make(chan tea.Msg, 64)

	go func() {
		defer close(ch)

		body := map[string]interface{}{
			"model":    model,
			"messages": history,
			"stream":   true,
		}
		// OpenAI models require max_completion_tokens; others use max_tokens
		if strings.HasPrefix(model, "gpt-") {
			body["max_completion_tokens"] = int64(8192)
		} else {
			body["max_tokens"] = int64(8192)
		}
		bodyBytes, _ := json.Marshal(body)

		req, _ := http.NewRequest("POST",
			gatewayURL()+"/v1/chat/completions",
			bytes.NewReader(bodyBytes))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("Accept", "text/event-stream")

		client := &http.Client{Timeout: 120 * time.Second}
		resp, err := client.Do(req)
		if err != nil {
			ch <- replStreamDone{err: fmt.Errorf("request failed: %w", err)}
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode != 200 {
			errBody, _ := io.ReadAll(resp.Body)
			ch <- replStreamDone{err: fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(errBody[:min(300, len(errBody))]))}
			return
		}

		scanner := bufio.NewScanner(resp.Body)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

		var fullText strings.Builder

		for scanner.Scan() {
			line := scanner.Text()
			if !strings.HasPrefix(line, "data: ") {
				continue
			}
			data := strings.TrimPrefix(line, "data: ")
			if data == "[DONE]" {
				break
			}

			var event struct {
				Choices []struct {
					Delta struct {
						Content          string `json:"content"`
						ReasoningContent string `json:"reasoning_content"`
					} `json:"delta"`
				} `json:"choices"`
			}
			if err := json.Unmarshal([]byte(data), &event); err != nil {
				continue
			}
			if len(event.Choices) > 0 {
				// Only use content — skip reasoning_content (internal thinking tokens)
				text := event.Choices[0].Delta.Content
				if text != "" {
					fullText.WriteString(text)
					ch <- replStreamChunk{text: text}
				}
			}
		}

		ch <- replStreamDone{fullText: fullText.String()}
	}()

	return ch
}
