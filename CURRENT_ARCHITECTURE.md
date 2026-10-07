# Current Zero-X / Neuron Architecture

## 1. Overview & System Context

Zero-X (`zero-x.live`) acts as the server-side authentication, account management, quota governance, and model routing gateway for the Neuron terminal coding agent (`RAHUL-DevelopeRR/neuron-v2`).

The client and server are decoupled across two distinct codebases:
1. **Client**: `c:\Users\DELL\neuron-v2-fixes` (Rust workspace with `rusty-claude-cli`, `runtime`, `tools`, `api`, `plugins`, `commands`, `telemetry`).
2. **Server / Gateway**: `C:\Users\DELL\zero-x.live\neuroncli\auth-server` (Hono application deployable to Cloudflare Workers and Node.js).

---

## 2. Codebase Organization

### Server Repository (`C:\Users\DELL\zero-x.live\neuroncli\auth-server`)
```
auth-server/
├── migrations/
│   ├── 20261003_accounts.sql              # Account tables & basic quota tracking
│   ├── 20261004_quota_reservations.sql    # Atomic reservation & settlement RPCs
│   ├── 20261006042028_plan_policies.sql   # Plan policies table (free, pro, ultrawork)
│   └── 20261006042200_gateway_session_store.sql # SHA256 hashed session persistence
├── src/
│   ├── index.js                           # Primary Hono app router & entry point
│   ├── chat-handler.js                    # /v1/chat/completions route handler & quota flow
│   ├── providers.js                       # Model catalog, upstream dispatch, SSE stream
│   ├── account-store.js                   # Supabase / PostgREST RPC client
│   ├── plan-policy.js                     # Plan limits loader & 60s cache
│   ├── session-store.js                   # KV / PostgreSQL session storage abstraction
│   └── operator-auth.js                   # GATEWAY_ADMIN_TOKEN constant-time authenticator
├── check-*.mjs / *.cjs                    # Comprehensive behavior & contract test suite
├── server.js                              # Node.js entry point (127.0.0.1:8787)
├── wrangler.toml                          # Full web + custom domain worker config
└── wrangler.api.toml                      # Lightweight API worker configuration
```

---

## 3. Current Request Flow (`/v1/chat/completions`)

```
Neuron CLI (Rust)
  │  Bearer ses_<random24>
  ▼
Hono App (Worker / Node) [src/index.js]
  │
  ├─► validateSession(c) [src/index.js -> session-store.js]
  │     Reads from Cloudflare KV (SESSIONS_KV) or Supabase (gateway_session_store).
  │
  ├─► Request Validation [src/chat-handler.js]
  │     Validates payload <= 1 MiB, messages schema, function tools schema, max_tokens (1..32768).
  │
  ├─► Catalog & Model Resolution [src/providers.js]
  │     Resolves model catalog from static allowlists or 60s discovery cache.
  │     If body.model is 'auto' or 'default', picks first tool-capable model in catalog.
  │
  ├─► Atomic Quota Reservation [src/account-store.js]
  │     Estimates prompt tokens: Math.ceil(JSON.stringify({messages, tools}).byteLength / 4).
  │     reserved = estimatedPromptTokens + maxTokens.
  │     Calls Postgres RPC: zerox_reserve_usage(p_user_id, p_tokens).
  │     Applies row lock (FOR UPDATE), checks daily limits, increments daily_requests & daily_tokens_used.
  │     Returns 429 insufficient_quota if limits exceeded.
  │
  ├─► Upstream Provider Call [src/providers.js -> requestCompletion()]
  │     Dispatches to Cloudflare AI (via env.AI binding or REST) or HTTP provider.
  │     Uses server-side secret (e.g. GROQ_API_KEY, OPENROUTER_API_KEY).
  │
  ├─► Streaming / SSE Transformation [src/providers.js -> completionStream()]
  │     Transforms upstream chunks, normalizes function calls & IDs.
  │     Emits OpenAI-compatible data: {...}\n\n chunks and data: [DONE]\n\n.
  │
  └─► Usage Reconciliation [src/chat-handler.js -> settle()]
        Extracts final usage from stream or completion.
        Calls Postgres RPC: zerox_settle_usage(p_user_id, p_reserved, p_actual, p_day).
        Adjusts daily_tokens_used by actual - reserved.
```

---

## 4. Current Data Model (PostgreSQL / Supabase)

### `public.zerox_accounts`
- `clerk_id uuid PRIMARY KEY`: maps to auth user ID.
- `plan text`: `'free'`, `'pro'`, or `'ultrawork'`.
- `daily_tokens_used bigint`, `daily_requests bigint`.
- `total_tokens_used bigint`, `total_requests bigint`.
- `last_usage_reset date`: UTC reset timestamp.

### `public.zerox_plan_policy`
- `plan text PRIMARY KEY`: `'free'`, `'pro'`, `'ultrawork'`.
- `daily_tokens bigint`, `daily_requests bigint`, `price_usd numeric(10,2)`.

### `public.gateway_session_store`
- `key text PRIMARY KEY`: 64-char hex SHA-256 hash of the session token.
- `value text`: JSON-encoded session payload.
- `expires_at timestamptz`: Expiration deadline.

---

## 5. Strengths & Reusable Components

1. **Atomic Quota Architecture**: `zerox_reserve_usage` and `zerox_settle_usage` execute atomically in PostgreSQL with row-level locking (`FOR UPDATE`), preventing concurrency race conditions.
2. **Hashed Session Storage**: Session tokens (`ses_*`) are hashed via SHA-256 before storage, preventing plaintext leakage if the store is inspected.
3. **Stream Transformation**: `completionStream()` correctly reconstructs SSE boundaries across read chunks, normalizes parallel tool calls, and handles disconnect signals.
4. **Shared Application Code**: Both Node.js (`server.js`) and Cloudflare Workers (`src/index.js`) run the identical Hono application logic.
5. **Robust Test Suite**: 8 behavior test scripts (`check-gateway.mjs`, `check-providers.mjs`, `check-quota-policy.mjs`, etc.) execute automated contract tests against real PGlite and simulated HTTP providers.

---

## 6. Technical Debt & Production Gaps

1. **Monolithic Provider Code**: `src/providers.js` combines HTTP dispatch, model catalog discovery, streaming parser, tool call normalizer, and provider configurations in a single file without an object/interface contract.
2. **No Fallback in Router**: If the chosen provider returns a 502/503 or transient 429 during generation, the request immediately fails with 502. There is no automated failover to alternate candidate models/providers.
3. **No Active Health Tracking**: Health state is only probed on-demand when an operator invokes `GET /v1/providers/health`. There is no circuit breaker or failure rate tracking during live user traffic.
4. **Lack of Request & Attempt Tracing**: Public requests lack structured `request_id` tracking, and upstream calls lack distinct `attempt_id` tracking.
5. **Error Format Inconsistencies**: Some error paths return `{ error: 'string' }` while others return OpenAI-compatible `{ error: { message, type, code } }`.
6. **No Upstream Provider Quota Accounting**: Upstream provider account rate limits and quotas are not tracked separately from customer account quotas.
