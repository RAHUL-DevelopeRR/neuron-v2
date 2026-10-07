# Zero-X Server Scaling Plan: Multi-Provider AI Gateway

## 1. Executive Summary

This plan outlines the staged evolution of Zero-X (`zero-x.live`) from a single-attempt proxy into a production-grade, highly available, multi-provider AI gateway capable of supporting high-concurrency Neuron client workloads without downtime or breaking changes.

---

## 2. Architecture Target & Layers

```
Layer 1: Public API / Data Plane (Hono, Cloudflare Workers / Node)
Layer 2: Authentication & Session Verification (ses_*, API keys, Operator Auth)
Layer 3: Account & Plan Management (Tier resolving, entitlements, plan limits)
Layer 4: Routing Engine (Capability matching, health filtering, candidate ranking, failover)
Layer 5: Provider Adapters (ProviderAdapter interface, OpenAICompatibleAdapter, WorkersAIAdapter)
Layer 6: Model Catalog (Public model registry, upstream mappings, capability flags)
Layer 7: Provider Health & Circuit Breakers (Async tracking, consecutive failures, backoff window)
Layer 8: Customer Quotas (Atomic reserve/settle via PostgreSQL RPCs)
Layer 9: Upstream Provider Quota & Concurrency Management
Layer 10: Usage & Billing Ledger (Auditable event ledger)
Layer 11: Observability & Tracing (request_id, attempt_id, latency, TTFT, token counts)
Layer 12: Asynchronous Background Processing (Cloudflare Queues, health probes)
```

---

## 3. Rollout Phases

### Phase 1: Provider Abstraction, Resilient Routing Engine, and Request Tracing (Current Implementation Phase)
- **Goal**: Decompose monolithic provider code into clean `ProviderAdapter` implementations, introduce deterministic capability-aware routing with automatic failover, implement an in-memory circuit breaker, and attach consistent `request_id` / `attempt_id` tracing.
- **Affected Files**:
  - `src/provider-adapter.js` (NEW): `ProviderAdapter` interface and `OpenAICompatibleAdapter`.
  - `src/cloudflare-adapter.js` (NEW): Dedicated adapter for Cloudflare Workers AI.
  - `src/router.js` (NEW): Deterministic capability matcher, candidate scoring, and failover engine.
  - `src/provider-health.js` (NEW): In-memory health state machine & circuit breaker (`HEALTHY`, `DEGRADED`, `RATE_LIMITED`, `QUOTA_EXHAUSTED`, `DOWN`).
  - `src/providers.js`: Refactored to coordinate adapters and delegate routing.
  - `src/chat-handler.js`: Integrated with router failover, request IDs, and unified error formatting.
  - `migrations/20261006050000_gateway_observability.sql`: Adds tables for provider health samples and request audit ledger.
- **Migrations**: Additive migration for health and usage audit tracking (zero downtime, non-breaking).
- **Tests**:
  - Adapter contract tests (OpenAI compatible, Cloudflare, error normalization).
  - Routing failover tests (fallback on upstream 502/503/429).
  - Circuit breaker trips and recovery windows.
  - Request ID & Attempt ID propagation in headers and responses.
  - 100% preservation of existing `check:production` and `check:database` suites.
- **Rollback Strategy**: Revert to previous `chat-handler.js` and `providers.js` commit. Schema changes are strictly additive.

### Phase 2: Asynchronous Health Probes & Cloudflare Queues
- **Goal**: Decouple provider health probes from user traffic using Cloudflare Queues and scheduled Worker crons.
- **Affected Files**:
  - `src/queue-consumer.js`: Processes background usage events and health metrics.
  - `wrangler.toml`: Adds `[[queues.producers]]` and `[[queues.consumers]]`.
- **Migrations**: None.
- **Rollback Strategy**: Disable queue consumers and revert worker configuration.

### Phase 3: PostgreSQL Hyperdrive Connection Pooling & High-Concurrency DOs
- **Goal**: Integrate Cloudflare Hyperdrive for zero-latency connection pooling to PostgreSQL; introduce Durable Objects for per-account concurrency throttling and distributed circuit breaker synchronization.
- **Affected Files**:
  - `src/account-store.js`: Hyperdrive direct connection support alongside REST RPC fallback.
  - `src/rate-limiter-do.js`: Durable Object for distributed concurrency counters.
- **Migrations**: Index tuning on `zerox_accounts` and `gateway_session_store`.
- **Rollback Strategy**: Revert Hyperdrive binding to PostgREST RPC.

### Phase 4: Customer BYOK (Bring Your Own Key) & Vault Encryption
- **Goal**: Allow enterprise users to bring private keys for upstream providers, encrypted at rest using Cloudflare Secrets Store / envelope encryption.
- **Affected Files**:
  - `src/byok-vault.js`: Encrypted key retrieval and decryption in memory.
  - `migrations/20261007_byok_credentials.sql`: Encrypted credential storage.
- **Security Implications**: Master encryption keys stored in Cloudflare KMS/Secrets; private keys never exposed to clients or logs.

---

## 4. Security & Isolation Invariants

1. **Server-Side Secret Secrecy**: Provider API keys (`GROQ_API_KEY`, `NVIDIA_API_KEY`, etc.) reside strictly in server environment variables / secrets and are NEVER transmitted to Neuron clients.
2. **SSRF Prevention**: Upstream URLs are derived strictly from trusted server configuration and whitelisted domains; user-supplied upstream endpoints are forbidden.
3. **Loopback Callback Isolation**: CLI OAuth handoffs use random loopback ports with strict Origin / Referer validation.
4. **Deterministic Anti-Farming**: System adheres to official organizational provider quotas; no automated multi-account rotation or credential cycling.
5. **Session Token Confidentiality**: Session tokens are hashed via SHA-256 before storage in database or KV.
