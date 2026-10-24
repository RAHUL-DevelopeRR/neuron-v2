# Zero-X Gateway: Standalone Isolated Repository Architecture

## 1. Executive Summary

To enable universal developer integration (OpenCode, Cursor, Zed, Continue.dev, custom Python/Node apps) and clean separation of concerns, the Zero-X AI Gateway is isolated as an independent microservice repository (`zero-x-gateway`).

The gateway is 100% **OpenAI-API compatible**, stateless at the edge, and proxies upstream LLMs while keeping provider API keys strictly server-side.

```
                      +------------------------------------+
                      |    Client Applications (Any Tool)   |
                      |  - OpenCode CLI (sst/opencode)     |
                      |  - Cursor / Zed / Windsurf / VSCode|
                      |  - Python SDK / Node.js SDK / Curl |
                      |  - Neuron CLI                      |
                      +-----------------+------------------+
                                        |
               Authorization: Bearer <ses_token>
               Base URL: https://zero-x.live/v1 (or http://127.0.0.1:8787/v1)
                                        |
                                        v
+---------------------------------------------------------------------------------+
|                       Zero-X AI Gateway Microservice                            |
|                                                                                 |
|  [Layer 1] Hono HTTP Router (/v1/chat/completions, /v1/models, /health)         |
|  [Layer 2] Session Auth & Quota Guard (Cloudflare KV / PostgreSQL)              |
|  [Layer 3] Routing Engine (Capability matching, prefix normalization)           |
|  [Layer 4] Circuit Breakers & Health Tracker                                    |
|  [Layer 5] Provider Adapters (Zero API key leakage):                            |
|            - Google Gemini Adapter                                              |
|            - Groq Cloud Adapter                                                 |
|            - NVIDIA NIM Adapter                                                 |
|            - OpenRouter Adapter                                                 |
|            - Cloudflare Workers AI Adapter                                      |
+---------------------------------------+-----------------------------------------+
                                        | Upstream HTTPS (Server-Side Secrets)
       +---------------+----------------+---------------+----------------+
       |               |                |               |                |
       v               v                v               v                v
[Google Gemini]  [Groq Cloud]    [NVIDIA NIM]     [OpenRouter]    [Cloudflare AI]
```

---

## 2. Isolated Repository Structure

When cloned into its own repository (e.g., `github.com/RAHUL-DevelopeRR/zero-x-gateway`), the tree is completely clean and independent:

```
zero-x-gateway/
├── .github/
│   └── workflows/
│       └── deploy.yml              # Automatic Cloudflare Worker deployment
├── src/
│   ├── index.js                    # Core Hono application entry & routes
│   ├── chat-handler.js             # /v1/chat/completions handler & quota accounting
│   ├── router.js                   # Intelligent capability matcher & failover engine
│   ├── providers.js                # Provider coordinator & model catalog registry
│   ├── provider-adapter.js         # Generic OpenAI-compatible upstream adapter
│   ├── cloudflare-adapter.js       # Cloudflare Workers AI adapter
│   ├── provider-health.js          # In-memory circuit breakers & status tracker
│   ├── session-store.js            # KV & database session store abstraction
│   ├── account-store.js            # PostgREST quota reservation/settlement RPC client
│   ├── plan-policy.js              # Subscription tier policies (free, pro, ultrawork)
│   └── operator-auth.js            # Admin token verification
├── migrations/
│   ├── 20261003_accounts.sql
│   ├── 20261004_quota_reservations.sql
│   ├── 20261006042028_plan_policies.sql
│   ├── 20261006042200_gateway_session_store.sql
│   ├── 20261006050000_gateway_observability.sql
│   └── 20261006060000_dynamic_provider_catalog.sql
├── server.js                       # Local / Node.js runtime entrypoint
├── wrangler.toml                   # Cloudflare Workers production configuration
├── package.json                    # Dependencies (Hono, pglite for testing)
├── .env.example                    # Template for environment variables
└── README.md                       # Service documentation & API spec
```

---

## 3. Standard OpenAI-Compatible Endpoints

### 1. `GET /v1/models`
Returns all active models across all configured providers.
```json
{
  "object": "list",
  "data": [
    { "id": "gemini/gemini-2.5-flash", "owned_by": "gemini", "capabilities": { "tools": true, "streaming": true } },
    { "id": "groq/openai/gpt-oss-120b", "owned_by": "groq", "capabilities": { "tools": true, "streaming": true } },
    { "id": "nvidia/meta/llama-3.2-11b-vision-instruct", "owned_by": "nvidia", "capabilities": { "tools": true, "streaming": true } },
    { "id": "cohere/north-mini-code:free", "owned_by": "openrouter", "capabilities": { "tools": true, "streaming": true } }
  ]
}
```

### 2. `POST /v1/chat/completions`
Accepts standard OpenAI JSON payloads with streaming (`stream: true`) and tool definitions (`tools: [...]`).
* Handles transport normalization: cleanly resolves models requested as `gemini/gemini-2.5-flash`, `gemini-2.5-flash`, or `openai/gemini/gemini-2.5-flash`.
* Supports SSE chunked streaming with `[DONE]` terminator.
* Emits OpenAI tool calls (`choice.delta.tool_calls`) with parallel tool call support.

### 3. `GET /health`
Returns gateway status and active providers:
```json
{
  "status": "ok",
  "service": "neuroncli-gateway-worker",
  "version": "2.0.0",
  "providers": [ "cloudflare", "openrouter", "groq", "gemini", "nvidia" ],
  "supabase_configured": true,
  "kv_bound": true
}
```

---

## 4. How to Deploy the Isolated Service

### Option A: Cloudflare Workers (Production Serverless Edge)
```bash
# 1. Provision secrets
wrangler secret put GROQ_API_KEY
wrangler secret put GEMINI_API_KEY
wrangler secret put OPENROUTER_API_KEY
wrangler secret put NVIDIA_API_KEY
wrangler secret put SUPABASE_SECRET_KEY

# 2. Deploy
wrangler deploy
```

### Option B: Node.js / Docker (Local or VPS)
```bash
# 1. Install dependencies
npm install

# 2. Start server
node server.js
# Listening on http://127.0.0.1:8787
```
