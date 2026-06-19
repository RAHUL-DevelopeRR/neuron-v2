# NeuronCLI Auth And Usage Dashboard Contract

This repository now uses one client-facing gateway contract for the CLI and
dashboard:

- `POST /auth/session` creates a short-lived CLI session token.
- `GET /auth/session` verifies the current token and returns session metadata.
- `GET /auth/usage` returns quota and usage data for the dashboard.
- `GET /auth/plan` returns the current plan data for upgrade/plan UI.
- `POST /v1/chat/completions` proxies OpenAI-compatible chat requests.

The CLI stores only the gateway session response at:

```text
~/.neuroncli/session.json
```

The client does not store provider API keys. The gateway owns Azure/OpenRouter
credentials and exchanges the CLI session token for model access server-side.

## Expected Zero-X.Live Storage Model

The local `zero-x.live` checkout was not present in this workspace during this
pass. The closest existing schema is `sync/supabase-schema.sql`, which models
the auth and sync layer as Supabase-backed data:

- `auth.users`: Supabase Auth source of truth for user identity.
- `profiles`: app profile fields keyed by `auth.users.id`.
- `sessions`: chat sessions, token usage, cost, device, and sync version.
- `messages`: persisted conversation messages per user/session.
- `api_keys`: encrypted per-user provider keys when BYOK is enabled.
- `user_settings`: cross-device preferences.
- `devices`: CLI/web/mobile device registrations.
- `sync_log`: audit and conflict-resolution records.

The production `zero-x.live` gateway should validate the web/dashboard auth
session, map it to `auth.users.id`, then mint the CLI `session_token` returned
by `/auth/session`. Usage written by the gateway should aggregate back into
the same dashboard-visible user record.

## Local Development Gateway

`go/cmd/gateway` is a development proxy. It keeps sessions and usage in memory
only, but exposes the same route shape as production so the Go TUI, Rust CLI,
and dashboard can agree on one auth path.
