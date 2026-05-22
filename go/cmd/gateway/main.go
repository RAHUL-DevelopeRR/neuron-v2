// NeuronCLI Local Gateway Server
//
// This is the LOCAL DEVELOPMENT version of the gateway.
// In production, this runs on zero-x.live/neuroncli and holds all API secrets.
//
// Architecture:
//   ┌─────────────────────────────────────────────────────────────┐
//   │                  zero-x.live server                        │
//   │  ┌────────────┐   ┌────────────────┐   ┌──────────────┐   │
//   │  │ Azure API   │   │ OpenRouter.ai  │   │ Google OAuth │   │
//   │  │ (GPT-5.5,   │   │ (Qwen, Llama,  │   │ (user auth)  │   │
//   │  │  Kimi, etc) │   │  free models)  │   │              │   │
//   │  └─────┬───────┘   └───────┬────────┘   └──────┬───────┘   │
//   │        │                   │                    │           │
//   │        └───────────┬───────┘                    │           │
//   │               ┌────┴────────────────────────────┘           │
//   │               │  Gateway Logic (this server)               │
//   │               │  - Session management                      │
//   │               │  - API key vault (NEVER sent to client)    │
//   │               │  - Model routing                           │
//   │               │  - Quota tracking                          │
//   │               │  - OpenRouter PKCE callback                │
//   │               └────┬────────────────────────────           │
//   └────────────────────┼───────────────────────────────────────┘
//                        │  HTTPS / SSE
//                   ┌────┴────┐
//                   │ Client  │  (neuron.exe — no secrets, just session token)
//                   └─────────┘
//
// For local testing, set env vars:
//   AZURE_OPENAI_API_KEY     — Azure AI Foundry key (held by server, NOT client)
//   AZURE_OPENAI_ENDPOINT    — Azure endpoint URL
//   OPENROUTER_API_KEY       — OpenRouter key (for free-tier fallback)
//
// These vars are SERVER-SIDE ONLY. The client never sees them.

package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

// ── Configuration (Server-side secrets) ─────────────────────

type ServerConfig struct {
	AzureAPIKey      string
	AzureEndpoint    string
	OpenRouterAPIKey string
	AzureModel       string
}

func loadConfig() ServerConfig {
	cfg := ServerConfig{
		AzureAPIKey:      os.Getenv("AZURE_OPENAI_API_KEY"),
		AzureEndpoint:    os.Getenv("AZURE_OPENAI_ENDPOINT"),
		OpenRouterAPIKey: os.Getenv("OPENROUTER_API_KEY"),
		AzureModel:       os.Getenv("AZURE_OPENAI_MODEL"),
	}
	if cfg.AzureEndpoint == "" {
		cfg.AzureEndpoint = "https://rahul-ma5zoiby-eastus2.services.ai.azure.com"
	}
	if cfg.AzureModel == "" {
		cfg.AzureModel = "Kimi-K2.5"
	}
	return cfg
}

// ── Session Store ───────────────────────────────────────────

type Session struct {
	Token       string    `json:"session_token"`
	Fingerprint string    `json:"machine_fingerprint"`
	CreatedAt   time.Time `json:"created_at"`
	QuotaUsed   int64     `json:"quota_used"`
}

var (
	sessions   = make(map[string]*Session)
	sessionsMu sync.RWMutex
	serverCfg  ServerConfig
)

const DailyQuotaLimit = 44000

func generateToken() string {
	return fmt.Sprintf("nrn_%d_%d", time.Now().UnixNano(), os.Getpid())
}

// ── Routing — which backend to use ──────────────────────────

type Backend struct {
	URL    string
	APIKey string
	Name   string
}

func resolveBackend(model string) Backend {
	// Priority 1: Azure AI Foundry (if configured)
	if serverCfg.AzureAPIKey != "" {
		return Backend{
			URL:    fmt.Sprintf("%s/models/chat/completions?api-version=2024-05-01-preview", strings.TrimRight(serverCfg.AzureEndpoint, "/")),
			APIKey: serverCfg.AzureAPIKey,
			Name:   "azure",
		}
	}

	// Priority 2: OpenRouter (free tier fallback)
	if serverCfg.OpenRouterAPIKey != "" {
		return Backend{
			URL:    "https://openrouter.ai/api/v1/chat/completions",
			APIKey: serverCfg.OpenRouterAPIKey,
			Name:   "openrouter",
		}
	}

	// No backend available
	return Backend{}
}

// ── Available Models ────────────────────────────────────────

var availableModels = []map[string]interface{}{
	{"id": "Kimi-K2.5", "name": "Kimi K2.5", "provider": "azure"},
	{"id": "Kimi-K2.6", "name": "Kimi K2.6", "provider": "azure"},
	{"id": "FW-DeepSeek-V3.2", "name": "DeepSeek V3.2", "provider": "azure"},
	{"id": "FW-MiniMax-M2.5", "name": "MiniMax M2.5", "provider": "azure"},
	{"id": "gpt-5.5", "name": "GPT-5.5", "provider": "azure"},
	{"id": "model-router", "name": "Model Router", "provider": "azure"},
	{"id": "Codex-Max", "name": "Codex Max", "provider": "azure"},
	{"id": "qwen/qwen3-coder-480b-a35b-instruct:free", "name": "Qwen3 Coder (Free)", "provider": "openrouter"},
}

// ── Handlers ────────────────────────────────────────────────

func corsMiddleware(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Access-Control-Allow-Origin", "*")
		w.Header().Set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
		w.Header().Set("Access-Control-Allow-Headers", "Content-Type, Authorization, Accept")
		if r.Method == "OPTIONS" {
			w.WriteHeader(200)
			return
		}
		next(w, r)
	}
}

func handleSession(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	var req struct {
		Fingerprint string `json:"machine_fingerprint"`
		Version     string `json:"version"`
	}
	json.NewDecoder(r.Body).Decode(&req)

	token := generateToken()
	session := &Session{
		Token:       token,
		Fingerprint: req.Fingerprint,
		CreatedAt:   time.Now(),
	}

	sessionsMu.Lock()
	sessions[token] = session
	sessionsMu.Unlock()

	// Determine available backend
	backend := resolveBackend("")
	providerName := "none"
	if backend.Name != "" {
		providerName = backend.Name
	}

	modelIDs := make([]string, len(availableModels))
	for i, m := range availableModels {
		modelIDs[i] = m["id"].(string)
	}

	json.NewEncoder(w).Encode(map[string]interface{}{
		"session_token": token,
		"models":        modelIDs,
		"provider":      providerName,
		"quota": map[string]interface{}{
			"daily_limit": DailyQuotaLimit,
			"used":        0,
			"remaining":   DailyQuotaLimit,
		},
	})
	log.Printf("[SESSION] Created for %s (v%s) → backend: %s", req.Fingerprint, req.Version, providerName)
}

func handleModels(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"models": availableModels,
	})
}

func handleChatCompletions(w http.ResponseWriter, r *http.Request) {
	// Read body
	body, err := io.ReadAll(r.Body)
	if err != nil {
		jsonError(w, "failed to read body", 400)
		return
	}

	var req map[string]interface{}
	if err := json.Unmarshal(body, &req); err != nil {
		jsonError(w, "invalid json", 400)
		return
	}

	model, _ := req["model"].(string)
	isStream, _ := req["stream"].(bool)

	// Resolve backend (server holds the keys, not the client)
	backend := resolveBackend(model)
	if backend.APIKey == "" {
		jsonError(w, "no API backend configured on gateway server — set AZURE_OPENAI_API_KEY or OPENROUTER_API_KEY on the server", 503)
		return
	}

	// Proxy the request to the backend
	proxyReq, _ := http.NewRequest("POST", backend.URL, bytes.NewReader(body))
	proxyReq.Header.Set("Content-Type", "application/json")
	proxyReq.Header.Set("Authorization", "Bearer "+backend.APIKey)
	if backend.Name == "azure" {
		proxyReq.Header.Set("api-key", backend.APIKey)
	}
	if isStream {
		proxyReq.Header.Set("Accept", "text/event-stream")
	}

	log.Printf("[PROXY] %s → %s via %s (stream=%v)", model, backend.URL, backend.Name, isStream)

	client := &http.Client{Timeout: 180 * time.Second}
	resp, err := client.Do(proxyReq)
	if err != nil {
		log.Printf("[ERROR] Backend proxy failed: %v", err)
		jsonError(w, fmt.Sprintf("backend proxy failed: %s", err), 502)
		return
	}
	defer resp.Body.Close()

	// Copy response headers
	for k, v := range resp.Header {
		for _, vv := range v {
			w.Header().Add(k, vv)
		}
	}
	w.WriteHeader(resp.StatusCode)

	if isStream {
		flusher, ok := w.(http.Flusher)
		buf := make([]byte, 4096)
		for {
			n, err := resp.Body.Read(buf)
			if n > 0 {
				w.Write(buf[:n])
				if ok {
					flusher.Flush()
				}
			}
			if err != nil {
				break
			}
		}
	} else {
		io.Copy(w, resp.Body)
	}

	log.Printf("[DONE] %s → %d via %s", model, resp.StatusCode, backend.Name)
}

// ── OpenRouter Callback (for PKCE auth flow) ────────────────

func handleCallback(w http.ResponseWriter, r *http.Request) {
	code := r.URL.Query().Get("code")
	if code != "" {
		log.Printf("[CALLBACK] Received auth code: %s...", code[:min(10, len(code))])
	}

	w.Header().Set("Content-Type", "text/html")
	w.Write([]byte(`<!DOCTYPE html>
<html><head><style>
body { font-family: -apple-system, 'Segoe UI', sans-serif; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; background: linear-gradient(135deg, #0f0f23 0%, #1a1a3e 100%); color: #e0e0e0; }
.card { text-align: center; padding: 60px; background: rgba(255,255,255,0.05); border: 1px solid rgba(65,105,195,0.3); border-radius: 24px; }
h1 { color: #F0A028; font-size: 2em; }
p { color: #aaa; font-size: 1.1em; }
.ok { color: #2D8C3C; font-size: 3em; }
</style></head><body><div class="card">
<div class="ok">✓</div>
<h1>Neuron Connected</h1>
<p>Your API key has been provisioned.<br>You can close this tab and return to the terminal.</p>
</div></body></html>`))
}

// ── Health ──────────────────────────────────────────────────

func handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	backend := resolveBackend("")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"status":   "ok",
		"service":  "neuron-gateway",
		"version":  "6.2.5",
		"backend":  backend.Name,
		"sessions": len(sessions),
		"note":     "API keys are held SERVER-SIDE. Client never sees them.",
	})
}

// ── Helpers ─────────────────────────────────────────────────

func jsonError(w http.ResponseWriter, msg string, code int) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// ── Main ────────────────────────────────────────────────────

func main() {
	serverCfg = loadConfig()

	port := os.Getenv("NEURON_GATEWAY_PORT")
	if port == "" {
		port = "19284"
	}

	mux := http.NewServeMux()

	// Auth routes (the paths the client hits)
	mux.HandleFunc("/neuroncli/auth/auth/session", corsMiddleware(handleSession))
	mux.HandleFunc("/neuroncli/auth/v1/chat/completions", corsMiddleware(handleChatCompletions))
	mux.HandleFunc("/neuroncli/auth/models", corsMiddleware(handleModels))
	mux.HandleFunc("/neuroncli/callback", corsMiddleware(handleCallback))

	// Health
	mux.HandleFunc("/health", corsMiddleware(handleHealth))
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/" {
			handleHealth(w, r)
			return
		}
		log.Printf("[404] %s %s", r.Method, r.URL.Path)
		http.Error(w, `{"error":"not found"}`, 404)
	})

	addr := ":" + port

	hasAzure := serverCfg.AzureAPIKey != ""
	hasOpenRouter := serverCfg.OpenRouterAPIKey != ""

	log.Printf("╔══════════════════════════════════════════════════╗")
	log.Printf("║  NeuronCLI Gateway Server v6.2.5                 ║")
	log.Printf("║  Listening on http://localhost%s             ║", addr)
	log.Printf("╠══════════════════════════════════════════════════╣")
	log.Printf("║  API keys are SERVER-SIDE ONLY.                  ║")
	log.Printf("║  Client (neuron.exe) NEVER needs API keys.       ║")
	log.Printf("╠══════════════════════════════════════════════════╣")
	if hasAzure {
		log.Printf("║  ✓ Azure AI Foundry: CONFIGURED                  ║")
		log.Printf("║    Endpoint: %s", serverCfg.AzureEndpoint)
	} else {
		log.Printf("║  ✗ Azure: NOT CONFIGURED (set AZURE_OPENAI_API_KEY) ║")
	}
	if hasOpenRouter {
		log.Printf("║  ✓ OpenRouter: CONFIGURED (fallback)              ║")
	} else {
		log.Printf("║  ✗ OpenRouter: NOT CONFIGURED (set OPENROUTER_API_KEY) ║")
	}
	log.Printf("╚══════════════════════════════════════════════════╝")

	if !hasAzure && !hasOpenRouter {
		log.Printf("")
		log.Printf("⚠ WARNING: No API backend configured!")
		log.Printf("  Set at least one of:")
		log.Printf("    AZURE_OPENAI_API_KEY    (for Azure AI Foundry)")
		log.Printf("    OPENROUTER_API_KEY      (for OpenRouter free tier)")
		log.Printf("  These are SERVER-SIDE env vars — the client never sees them.")
		log.Printf("")
	}

	if err := http.ListenAndServe(addr, mux); err != nil {
		log.Fatalf("Failed to start server: %v", err)
	}
}
