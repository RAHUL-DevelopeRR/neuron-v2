# Neuron Sync — Auth, Persistence & Cross-Platform Sync

> **The missing persistence layer for NeuronCLI.** Sign in once, chat everywhere, never lose history.

## What This Solves

| Problem | Before | After (Neuron Sync) |
|---------|--------|---------------------|
| **Chat history** | Lost on exit | Persistent in PostgreSQL |
| **Authentication** | None — raw env vars | Email, GitHub, Google OAuth |
| **API keys** | Scattered env vars per machine | Encrypted, synced across devices |
| **Cross-device** | Zero sync | Real-time via WebSocket |
| **Settings** | Per-machine config files | Synced user preferences |
| **Device tracking** | No awareness | Knows which device sent what |

## Quick Start

### 1. Demo Mode (No Supabase needed)

```bash
cd sync
npm install
npm run dev
```

Opens at **http://localhost:5173** — runs entirely in-memory with local demo data.

### 2. Production Mode (with Supabase)

1. Create a free project at [supabase.com](https://supabase.com)

2. Run the schema in SQL Editor:
   - Go to Supabase Dashboard → SQL Editor
   - Paste contents of `supabase-schema.sql`
   - Click **Run**

3. Configure environment:
   ```bash
   # Create .env file
   echo VITE_SUPABASE_URL=https://YOUR_PROJECT.supabase.co > .env
   echo VITE_SUPABASE_ANON_KEY=YOUR_ANON_KEY >> .env
   ```

4. Enable Auth providers (optional):
   - Dashboard → Authentication → Providers
   - Enable GitHub and/or Google OAuth

5. Run:
   ```bash
   npm run dev
   ```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Mobile / Web / CLI                 │
│              (Any device, any browser)               │
├──────────┬──────────┬──────────┬────────────────────┤
│   Auth   │ Sessions │ Messages │   API Keys         │
│  Module  │  Module  │  Module  │   Module           │
├──────────┴──────────┴──────────┴────────────────────┤
│              Neuron Sync Engine                      │
│    ┌──────────────────────────────────────┐          │
│    │  Real-time Subscriptions (WebSocket) │          │
│    │  Conflict Resolution (sync_version)  │          │
│    │  Device Fingerprinting               │          │
│    └──────────────────────────────────────┘          │
├─────────────────────────────────────────────────────┤
│                 Supabase Backend                     │
│  ┌─────────┐ ┌──────────┐ ┌───────────┐            │
│  │  Auth   │ │ Postgres │ │ Realtime  │            │
│  │ (JWT)   │ │  (RLS)   │ │ (CDC)     │            │
│  └─────────┘ └──────────┘ └───────────┘            │
└─────────────────────────────────────────────────────┘
```

## Files

| File | Purpose |
|------|---------|
| `supabase-schema.sql` | Complete database schema (run in Supabase SQL Editor) |
| `src/neuron-sync.js` | Core sync engine — auth, CRUD, real-time, encryption |
| `src/app.js` | Full UI app with demo mode fallback |
| `styles.css` | Premium dark theme design system |
| `index.html` | Entry point |

## How to Integrate with NeuronCLI Go Engine

The sync engine is designed to slot into the existing Go architecture:

```go
// In go/internal/config/config.go — add sync config
type SyncConfig struct {
    SupabaseURL    string `json:"supabaseUrl"`
    SupabaseKey    string `json:"supabaseKey"`
    SyncEnabled    bool   `json:"syncEnabled"`
    DeviceID       string `json:"deviceId"`
}

// In go/internal/session/session.go — add sync-on-save
func (s *service) Create(ctx context.Context, title string) (Session, error) {
    // ... existing SQLite save ...

    // Sync to cloud
    if syncEnabled {
        go syncToSupabase(session)
    }
    return session, nil
}
```

## Database Schema

### Tables

- **profiles** — User profiles (auto-created on signup)
- **sessions** — Chat conversations with metadata
- **messages** — Individual messages with token tracking
- **api_keys** — Encrypted API keys per provider
- **user_settings** — Cross-device preferences
- **devices** — Connected device registry
- **sync_log** — Audit trail for conflict resolution

### Security

- **Row Level Security (RLS)** on every table
- Users can only access their own data
- API keys encrypted client-side before storage
- JWT-based auth with auto-refresh

## Cross-Platform Sync Flow

```
Device A (Desktop CLI)          Supabase           Device B (Phone)
─────────────────────          ─────────          ─────────────────
  User sends message ──────────► INSERT ──────────► Real-time push
                                  │                      │
                               Triggers:              UI updates
                               - sync_version++       automatically
                               - updated_at = NOW()
                               - Realtime broadcast
```
