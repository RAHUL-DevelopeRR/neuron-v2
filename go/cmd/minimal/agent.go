// NeuronCLI Minimal Agent — The entire coding agent in one file.
// This is the REPL loop that the 43MB binary does, minus the UI chrome.
//
// Usage: go run agent.go
// Dependencies: none (just net/http + stdlib)

package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// ── Config ────────────────────────────────────────────────────
var (
	gatewayURL = "https://api.zero-x.live"
	model      = "Kimi-K2.5"
	maxTokens  = 8192
	sessionTok = ""
	cwd, _     = os.Getwd()
)

// ── Messages ──────────────────────────────────────────────────
type Msg map[string]interface{}

var messages = []Msg{}

var systemPrompt = `You are an expert coding assistant. You have tools to read/write files and run commands.
Working directory: ` + cwd + `
OS: ` + runtime.GOOS + `
Rules:
- Use tools to accomplish tasks. Don't just describe what to do — DO it.
- After writing files, briefly confirm what you did.
- Be concise.`

var tools = []map[string]interface{}{
	{
		"type": "function",
		"function": map[string]interface{}{
			"name":        "bash",
			"description": "Run a shell command and return stdout+stderr",
			"parameters": map[string]interface{}{
				"type":       "object",
				"properties": map[string]interface{}{"command": map[string]string{"type": "string", "description": "Command to run"}},
				"required":   []string{"command"},
			},
		},
	},
	{
		"type": "function",
		"function": map[string]interface{}{
			"name":        "write_file",
			"description": "Write content to a file (creates dirs if needed)",
			"parameters": map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"path":    map[string]string{"type": "string", "description": "File path (relative to cwd)"},
					"content": map[string]string{"type": "string", "description": "File content"},
				},
				"required": []string{"path", "content"},
			},
		},
	},
	{
		"type": "function",
		"function": map[string]interface{}{
			"name":        "read_file",
			"description": "Read a file's contents",
			"parameters": map[string]interface{}{
				"type":       "object",
				"properties": map[string]interface{}{"path": map[string]string{"type": "string", "description": "File path"}},
				"required":   []string{"path"},
			},
		},
	},
}

// ── Tool Execution ────────────────────────────────────────────
func execTool(name, argsJSON string) string {
	var args map[string]string
	json.Unmarshal([]byte(argsJSON), &args)

	switch name {
	case "bash":
		cmd := args["command"]
		fmt.Printf("  \033[33m$ %s\033[0m\n", cmd)
		var c *exec.Cmd
		if runtime.GOOS == "windows" {
			c = exec.Command("cmd.exe", "/C", cmd)
		} else {
			c = exec.Command("/bin/sh", "-c", cmd)
		}
		c.Dir = cwd
		out, err := c.CombinedOutput()
		result := string(out)
		if err != nil {
			result += "\nError: " + err.Error()
		}
		if len(result) > 4000 {
			result = result[:2000] + "\n...[truncated]...\n" + result[len(result)-1000:]
		}
		return result

	case "write_file":
		path := args["path"]
		if !filepath.IsAbs(path) {
			path = filepath.Join(cwd, path)
		}
		os.MkdirAll(filepath.Dir(path), 0755)
		err := os.WriteFile(path, []byte(args["content"]), 0644)
		if err != nil {
			return "Error: " + err.Error()
		}
		fmt.Printf("  \033[32m✓ wrote %s\033[0m\n", path)
		return "File written successfully: " + path

	case "read_file":
		path := args["path"]
		if !filepath.IsAbs(path) {
			path = filepath.Join(cwd, path)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return "Error: " + err.Error()
		}
		content := string(data)
		if len(content) > 4000 {
			content = content[:2000] + "\n...[truncated]...\n" + content[len(content)-1000:]
		}
		return content

	default:
		return "Unknown tool: " + name
	}
}

// ── Gateway Auth ──────────────────────────────────────────────
func authenticate() {
	// Try cache first
	cacheFile := filepath.Join(os.Getenv("USERPROFILE"), ".neuron", "session_cache.json")
	if runtime.GOOS != "windows" {
		cacheFile = filepath.Join(os.Getenv("HOME"), ".neuron", "session_cache.json")
	}
	if data, err := os.ReadFile(cacheFile); err == nil {
		var cache map[string]string
		json.Unmarshal(data, &cache)
		if t, ok := cache["token"]; ok && t != "" {
			sessionTok = t
			return
		}
	}

	// Get new session
	hostname, _ := os.Hostname()
	body, _ := json.Marshal(map[string]string{
		"machine_fingerprint": hostname + "-minimal-agent",
		"version":             "minimal-1.0",
	})
	resp, err := http.Post(gatewayURL+"/auth/session", "application/json", bytes.NewReader(body))
	if err != nil {
		fmt.Println("Auth failed:", err)
		os.Exit(1)
	}
	defer resp.Body.Close()
	var result map[string]interface{}
	json.NewDecoder(resp.Body).Decode(&result)
	sessionTok = result["session_token"].(string)

	// Cache it
	os.MkdirAll(filepath.Dir(cacheFile), 0755)
	cache, _ := json.Marshal(map[string]string{"token": sessionTok})
	os.WriteFile(cacheFile, cache, 0644)
}

// ── LLM Call (streaming) ──────────────────────────────────────
type ToolCall struct {
	ID   string
	Name string
	Args string
}

func callLLM(msgs []Msg) (string, []ToolCall) {
	body, _ := json.Marshal(map[string]interface{}{
		"model":      model,
		"messages":   msgs,
		"max_tokens": maxTokens,
		"stream":     true,
		"tools":      tools,
	})

	req, _ := http.NewRequest("POST", gatewayURL+"/v1/chat/completions", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+sessionTok)

	client := &http.Client{Timeout: 120 * time.Second}

	// Retry on 429
	var resp *http.Response
	for attempt := 0; attempt < 5; attempt++ {
		var err error
		resp, err = client.Do(req)
		if err != nil {
			fmt.Println("\n  Error:", err)
			return "", nil
		}
		if resp.StatusCode == 429 {
			resp.Body.Close()
			wait := time.Duration(2<<uint(attempt)) * time.Second
			fmt.Printf("  \033[33m⏳ Rate limited, waiting %s...\033[0m\n", wait)
			time.Sleep(wait)
			req.Body = io.NopCloser(bytes.NewReader(body)) // reset body
			continue
		}
		break
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		b, _ := io.ReadAll(resp.Body)
		fmt.Printf("\n  Error %d: %s\n", resp.StatusCode, string(b))
		return "", nil
	}

	// Parse SSE stream
	content := ""
	var toolCalls []ToolCall
	tcMap := map[int]*ToolCall{}

	scanner := bufio.NewScanner(resp.Body)
	scanner.Buffer(make([]byte, 256*1024), 256*1024)

	for scanner.Scan() {
		line := scanner.Text()
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		data := strings.TrimPrefix(line, "data: ")
		if data == "[DONE]" {
			break
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
			} `json:"choices"`
		}

		if err := json.Unmarshal([]byte(data), &chunk); err != nil || len(chunk.Choices) == 0 {
			continue
		}

		delta := chunk.Choices[0].Delta

		// Content
		if delta.Content != "" {
			fmt.Print(delta.Content)
			content += delta.Content
		}

		// Tool calls
		for _, tc := range delta.ToolCalls {
			if _, ok := tcMap[tc.Index]; !ok {
				tcMap[tc.Index] = &ToolCall{}
			}
			entry := tcMap[tc.Index]
			if tc.ID != "" {
				entry.ID = tc.ID
			}
			if tc.Function.Name != "" {
				entry.Name = tc.Function.Name
			}
			entry.Args += tc.Function.Arguments
		}
	}

	if content != "" {
		fmt.Println()
	}

	for i := 0; i < len(tcMap); i++ {
		toolCalls = append(toolCalls, *tcMap[i])
	}

	return content, toolCalls
}

// ── MAIN: The REPL ───────────────────────────────────────────
func main() {
	fmt.Println("\033[36m  NeuronCLI Minimal Agent\033[0m")
	fmt.Println("  Type your request. Ctrl+C to exit.")
	fmt.Println()

	authenticate()
	messages = append(messages, Msg{"role": "system", "content": systemPrompt})

	reader := bufio.NewReader(os.Stdin)

	for {
		fmt.Print("\033[32m> \033[0m")
		input, _ := reader.ReadString('\n')
		input = strings.TrimSpace(input)
		if input == "" {
			continue
		}
		if input == "exit" || input == "quit" {
			break
		}

		// Add user message
		messages = append(messages, Msg{"role": "user", "content": input})

		// Agent loop: call LLM → execute tools → repeat until done
		for {
			content, toolCalls := callLLM(messages)

			// Add assistant response
			assistantMsg := Msg{"role": "assistant"}
			if content != "" {
				assistantMsg["content"] = content
			}
			if len(toolCalls) > 0 {
				var tcList []map[string]interface{}
				for _, tc := range toolCalls {
					tcList = append(tcList, map[string]interface{}{
						"id": tc.ID, "type": "function",
						"function": map[string]interface{}{"name": tc.Name, "arguments": tc.Args},
					})
				}
				assistantMsg["tool_calls"] = tcList
			}
			messages = append(messages, assistantMsg)

			// If no tool calls, we're done
			if len(toolCalls) == 0 {
				break
			}

			// Execute each tool and add results
			for _, tc := range toolCalls {
				fmt.Printf("  \033[36m🔧 %s\033[0m\n", tc.Name)
				result := execTool(tc.Name, tc.Args)
				messages = append(messages, Msg{
					"role":         "tool",
					"tool_call_id": tc.ID,
					"content":      result,
				})
			}
			// Loop back to send tool results to LLM
		}
		fmt.Println()
	}
}
