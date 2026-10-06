# Neuron release progress

Updated: 2026-10-06

## Completed

- Fixed the Neuron MCP stdio transport default to use newline-delimited JSON-RPC and initialize servers correctly.
- Kept the legacy Content-Length protocol available for compatible servers and made the legacy test fixtures select it explicitly.
- Corrected the Unix test-only MCP initialization timeout so parallel CI has enough time to start fixture processes.
- Fixed non-blocking stdin check stall on Windows in `rust/crates/rusty-claude-cli/src/args.rs`.
- Fixed Windows non-blocking socket inheritance issue in `rust/crates/rusty-claude-cli/tests/gateway_roundtrip.rs`.
- Full Rust workspace test suite passes (1000 passed, 0 failed, 20 ignored). Clippy and format checks clean.
- Produced comprehensive architecture and scaling blueprints:
  - `CURRENT_ARCHITECTURE.md`: Grounded audit of existing gateway and CLI components.
  - `SERVER_SCALING_PLAN.md`: 4-phase production scaling and enterprise roadmap.
- Implemented Phase 1 multi-provider gateway architecture in `zero-x.live` (`auth-server`):
  - Modular `ProviderAdapter` and configurable `OpenAICompatibleAdapter` (`src/provider-adapter.js`).
  - Native `CloudflareWorkersAIAdapter` (`src/cloudflare-adapter.js`).
  - Asynchronous `ProviderHealthTracker` with circuit breakers (`src/provider-health.js`).
  - Deterministic `RoutingEngine` with capability matching and fallback logic (`src/router.js`).
  - Request and attempt correlation IDs (`req_*`, `att_*`) integrated into `src/chat-handler.js`.
  - Database observability migration (`migrations/20261006050000_gateway_observability.sql`).
  - Phase 1 architecture verification suite (`check-phase1-architecture.mjs`).
- Executed live end-to-end manual external user verification (`test-external-user-flow.mjs`):
  - Session creation via `/auth/session` and validation via `/auth/session`.
  - Model catalog discovery via `/v1/models`.
  - `neuron doctor` gateway authentication verification.
  - Non-interactive streaming inference via Neuron CLI (`target/release/neuron.exe`).
  - Autonomous multi-turn tool execution (`read_file` round-trip).
  - Secret isolation audit confirming zero platform key leakage.

## Current state

- Neuron changes are on branch `codex/gateway-client-fixes`, associated with PR #3: https://github.com/RAHUL-DevelopeRR/neuron-v2/pull/3
- Gateway server changes are on branch `main` in `RAHUL-DevelopeRR/zero-x.live`.
- All local server test suites (`npm test`, `check:production`, `check:phase1`) pass 100%.
- Live production gateway at `https://zero-x.live` is online, serving Cloudflare Workers AI with database connectivity and quota enforcement.

## Next steps

1. Merge PR #3 and cut `v6.2.5` release tag for `neuron-v2`.
2. Deploy Zero-X gateway Phase 1 observability migration and updated Worker to staging/production via Cloudflare Wrangler.
3. Add Cloudflare Hyperdrive connection pooling and Cloudflare Queues for asynchronous attempt ingestion (Phase 2).
4. Integrate Stripe billing webhooks and customer credit ledger (Phase 3).
5. Implement encrypted Customer BYOK for enterprise team seats (Phase 4).
