-- ============================================================
-- Neuron Sync — Supabase Schema
-- ============================================================
-- Run this in your Supabase SQL Editor (Dashboard → SQL Editor)
-- This creates all tables needed for auth, chat persistence,
-- cross-device sync, and settings storage.
-- ============================================================

-- Enable UUID generation
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ──────────────────────────────────────────
-- 1. User Profiles (extends Supabase Auth)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS profiles (
    id UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
    display_name TEXT,
    avatar_url TEXT,
    preferred_model TEXT DEFAULT 'Kimi-K2.5',
    preferred_provider TEXT DEFAULT 'neuron',
    theme TEXT DEFAULT 'dark',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Auto-create profile on signup
CREATE OR REPLACE FUNCTION handle_new_user()
RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO profiles (id, display_name)
    VALUES (NEW.id, COALESCE(NEW.raw_user_meta_data->>'display_name', NEW.email));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

DROP TRIGGER IF EXISTS on_auth_user_created ON auth.users;
CREATE TRIGGER on_auth_user_created
    AFTER INSERT ON auth.users
    FOR EACH ROW EXECUTE FUNCTION handle_new_user();


-- ──────────────────────────────────────────
-- 2. Sessions (Chat Conversations)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS sessions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    title TEXT NOT NULL DEFAULT 'New Session',
    model TEXT,
    provider TEXT,
    prompt_tokens BIGINT DEFAULT 0,
    completion_tokens BIGINT DEFAULT 0,
    cost DOUBLE PRECISION DEFAULT 0.0,
    is_archived BOOLEAN DEFAULT FALSE,
    is_pinned BOOLEAN DEFAULT FALSE,
    device_id TEXT,  -- which device created this session
    sync_version BIGINT DEFAULT 1,  -- increments on each change for conflict resolution
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_updated_at ON sessions(updated_at DESC);
CREATE INDEX idx_sessions_sync_version ON sessions(sync_version);


-- ──────────────────────────────────────────
-- 3. Messages (Chat History)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS messages (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL DEFAULT '',
    model TEXT,
    token_count INTEGER DEFAULT 0,
    metadata JSONB DEFAULT '{}',  -- tool calls, code blocks, etc.
    device_id TEXT,
    sync_version BIGINT DEFAULT 1,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX idx_messages_session_id ON messages(session_id);
CREATE INDEX idx_messages_user_id ON messages(user_id);
CREATE INDEX idx_messages_created_at ON messages(session_id, created_at ASC);


-- ──────────────────────────────────────────
-- 4. API Keys (Encrypted secrets per user)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    encrypted_key TEXT NOT NULL,  -- client-side encrypted before storing
    label TEXT DEFAULT '',
    is_active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, provider)
);

CREATE INDEX idx_api_keys_user_id ON api_keys(user_id);


-- ──────────────────────────────────────────
-- 5. User Settings (cross-device preferences)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_settings (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    setting_key TEXT NOT NULL,
    setting_value JSONB NOT NULL,
    sync_version BIGINT DEFAULT 1,
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, setting_key)
);

CREATE INDEX idx_user_settings_user_id ON user_settings(user_id);


-- ──────────────────────────────────────────
-- 6. Devices (track connected devices)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,  -- device fingerprint
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    device_name TEXT NOT NULL,
    device_type TEXT NOT NULL CHECK (device_type IN ('desktop', 'mobile', 'tablet', 'web', 'cli')),
    os TEXT,
    last_sync_at TIMESTAMPTZ DEFAULT NOW(),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_devices_user_id ON devices(user_id);


-- ──────────────────────────────────────────
-- 7. Sync Log (conflict resolution audit trail)
-- ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS sync_log (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL,  -- 'session', 'message', 'setting', 'api_key'
    entity_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('create', 'update', 'delete', 'conflict_resolved')),
    device_id TEXT,
    sync_version BIGINT,
    payload JSONB,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_sync_log_user_id ON sync_log(user_id);
CREATE INDEX idx_sync_log_entity ON sync_log(entity_type, entity_id);


-- ──────────────────────────────────────────
-- Row Level Security (RLS)
-- ──────────────────────────────────────────
ALTER TABLE profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE messages ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE devices ENABLE ROW LEVEL SECURITY;
ALTER TABLE sync_log ENABLE ROW LEVEL SECURITY;

-- Users can only access their own data
CREATE POLICY "Users can view own profile" ON profiles FOR ALL USING (auth.uid() = id);
CREATE POLICY "Users can manage own sessions" ON sessions FOR ALL USING (auth.uid() = user_id);
CREATE POLICY "Users can manage own messages" ON messages FOR ALL USING (auth.uid() = user_id);
CREATE POLICY "Users can manage own api_keys" ON api_keys FOR ALL USING (auth.uid() = user_id);
CREATE POLICY "Users can manage own settings" ON user_settings FOR ALL USING (auth.uid() = user_id);
CREATE POLICY "Users can manage own devices" ON devices FOR ALL USING (auth.uid() = user_id);
CREATE POLICY "Users can view own sync_log" ON sync_log FOR ALL USING (auth.uid() = user_id);


-- ──────────────────────────────────────────
-- Auto-update timestamps
-- ──────────────────────────────────────────
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    NEW.sync_version = COALESCE(OLD.sync_version, 0) + 1;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER sessions_updated_at BEFORE UPDATE ON sessions
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER api_keys_updated_at BEFORE UPDATE ON api_keys
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER user_settings_updated_at BEFORE UPDATE ON user_settings
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER profiles_updated_at BEFORE UPDATE ON profiles
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();


-- ──────────────────────────────────────────
-- Enable Realtime for cross-device sync
-- ──────────────────────────────────────────
ALTER PUBLICATION supabase_realtime ADD TABLE sessions;
ALTER PUBLICATION supabase_realtime ADD TABLE messages;
ALTER PUBLICATION supabase_realtime ADD TABLE user_settings;
ALTER PUBLICATION supabase_realtime ADD TABLE devices;
