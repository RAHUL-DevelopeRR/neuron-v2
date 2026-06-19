/**
 * Neuron Sync Engine — Core Supabase Client
 *
 * Handles: Authentication, session/message CRUD, real-time sync,
 * device registration, API key storage, and conflict resolution.
 */

import { createClient } from '@supabase/supabase-js';

// ═══════════════════════════════════════════════════════════
// Configuration — Replace with your Supabase project details
// ═══════════════════════════════════════════════════════════

const SUPABASE_URL = import.meta.env.VITE_SUPABASE_URL || 'https://YOUR_PROJECT.supabase.co';
const SUPABASE_ANON_KEY = import.meta.env.VITE_SUPABASE_ANON_KEY || 'YOUR_ANON_KEY';

export const supabase = createClient(SUPABASE_URL, SUPABASE_ANON_KEY, {
  auth: {
    autoRefreshToken: true,
    persistSession: true,
    detectSessionInUrl: true,
  },
  realtime: {
    params: {
      eventsPerSecond: 10,
    },
  },
});

// ═══════════════════════════════════════════
// Device Fingerprint
// ═══════════════════════════════════════════

function generateDeviceId() {
  const stored = localStorage.getItem('neuron_device_id');
  if (stored) return stored;
  const id = `${navigator.platform}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  localStorage.setItem('neuron_device_id', id);
  return id;
}

function getDeviceType() {
  const ua = navigator.userAgent;
  if (/tablet|ipad/i.test(ua)) return 'tablet';
  if (/mobile|android|iphone/i.test(ua)) return 'mobile';
  return 'web';
}

function getDeviceName() {
  const platform = navigator.platform || navigator.userAgent;
  return `${platform.split(/[()]/)[0].trim()} — ${new Date().toLocaleDateString()}`;
}

export const DEVICE_ID = generateDeviceId();

// ═══════════════════════════════════════════
// Auth Module
// ═══════════════════════════════════════════

export const auth = {
  async signUp(email, password, displayName) {
    const { data, error } = await supabase.auth.signUp({
      email,
      password,
      options: {
        data: { display_name: displayName || email.split('@')[0] },
      },
    });
    if (error) throw error;
    if (data.user) await this._registerDevice(data.user.id);
    return data;
  },

  async signIn(email, password) {
    const { data, error } = await supabase.auth.signInWithPassword({ email, password });
    if (error) throw error;
    if (data.user) await this._registerDevice(data.user.id);
    return data;
  },

  async signInWithGitHub() {
    const { data, error } = await supabase.auth.signInWithOAuth({
      provider: 'github',
      options: { redirectTo: window.location.origin },
    });
    if (error) throw error;
    return data;
  },

  async signInWithGoogle() {
    const { data, error } = await supabase.auth.signInWithOAuth({
      provider: 'google',
      options: { redirectTo: window.location.origin },
    });
    if (error) throw error;
    return data;
  },

  async signOut() {
    const { error } = await supabase.auth.signOut();
    if (error) throw error;
  },

  async getUser() {
    const { data: { user } } = await supabase.auth.getUser();
    return user;
  },

  async getProfile() {
    const user = await this.getUser();
    if (!user) return null;
    const { data, error } = await supabase
      .from('profiles')
      .select('*')
      .eq('id', user.id)
      .single();
    if (error) throw error;
    return data;
  },

  async updateProfile(updates) {
    const user = await this.getUser();
    if (!user) throw new Error('Not authenticated');
    const { data, error } = await supabase
      .from('profiles')
      .update(updates)
      .eq('id', user.id)
      .select()
      .single();
    if (error) throw error;
    return data;
  },

  onAuthStateChange(callback) {
    return supabase.auth.onAuthStateChange(callback);
  },

  async _registerDevice(userId) {
    await supabase.from('devices').upsert({
      id: DEVICE_ID,
      user_id: userId,
      device_name: getDeviceName(),
      device_type: getDeviceType(),
      os: navigator.platform,
      last_sync_at: new Date().toISOString(),
    });
  },
};

// ═══════════════════════════════════════════
// Sessions Module
// ═══════════════════════════════════════════

export const sessions = {
  async create(title = 'New Session', model = 'Kimi-K2.5', provider = 'neuron') {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');
    const { data, error } = await supabase
      .from('sessions')
      .insert({
        user_id: user.id,
        title,
        model,
        provider,
        device_id: DEVICE_ID,
      })
      .select()
      .single();
    if (error) throw error;
    return data;
  },

  async list(includeArchived = false) {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');
    let query = supabase
      .from('sessions')
      .select('*, messages(count)')
      .eq('user_id', user.id)
      .order('updated_at', { ascending: false });
    if (!includeArchived) {
      query = query.eq('is_archived', false);
    }
    const { data, error } = await query;
    if (error) throw error;
    return data;
  },

  async get(sessionId) {
    const { data, error } = await supabase
      .from('sessions')
      .select('*')
      .eq('id', sessionId)
      .single();
    if (error) throw error;
    return data;
  },

  async update(sessionId, updates) {
    const { data, error } = await supabase
      .from('sessions')
      .update(updates)
      .eq('id', sessionId)
      .select()
      .single();
    if (error) throw error;
    return data;
  },

  async archive(sessionId) {
    return this.update(sessionId, { is_archived: true });
  },

  async pin(sessionId, pinned = true) {
    return this.update(sessionId, { is_pinned: pinned });
  },

  async delete(sessionId) {
    const { error } = await supabase.from('sessions').delete().eq('id', sessionId);
    if (error) throw error;
  },

  /**
   * Subscribe to real-time session changes across all devices
   */
  subscribeToChanges(userId, callback) {
    return supabase
      .channel('sessions-sync')
      .on('postgres_changes', {
        event: '*',
        schema: 'public',
        table: 'sessions',
        filter: `user_id=eq.${userId}`,
      }, callback)
      .subscribe();
  },
};

// ═══════════════════════════════════════════
// Messages Module
// ═══════════════════════════════════════════

export const messages = {
  async create(sessionId, role, content, model = null, metadata = {}) {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');
    const { data, error } = await supabase
      .from('messages')
      .insert({
        session_id: sessionId,
        user_id: user.id,
        role,
        content,
        model,
        metadata,
        device_id: DEVICE_ID,
        token_count: Math.ceil(content.length / 4), // rough estimate
      })
      .select()
      .single();
    if (error) throw error;

    // Update session's updated_at
    await supabase
      .from('sessions')
      .update({ updated_at: new Date().toISOString() })
      .eq('id', sessionId);

    return data;
  },

  async listBySession(sessionId) {
    const { data, error } = await supabase
      .from('messages')
      .select('*')
      .eq('session_id', sessionId)
      .order('created_at', { ascending: true });
    if (error) throw error;
    return data;
  },

  async delete(messageId) {
    const { error } = await supabase.from('messages').delete().eq('id', messageId);
    if (error) throw error;
  },

  /**
   * Subscribe to real-time message additions for a session
   */
  subscribeToSession(sessionId, callback) {
    return supabase
      .channel(`messages-${sessionId}`)
      .on('postgres_changes', {
        event: 'INSERT',
        schema: 'public',
        table: 'messages',
        filter: `session_id=eq.${sessionId}`,
      }, callback)
      .subscribe();
  },
};

// ═══════════════════════════════════════════
// API Keys Module (client-side encrypted)
// ═══════════════════════════════════════════

export const apiKeys = {
  /**
   * Simple XOR-based obfuscation (NOT cryptographic security).
   * For production, use Web Crypto API with user's derived key.
   */
  _obfuscate(text, userEmail) {
    const key = userEmail || 'neuron-default-key';
    return btoa(
      text
        .split('')
        .map((c, i) => String.fromCharCode(c.charCodeAt(0) ^ key.charCodeAt(i % key.length)))
        .join('')
    );
  },

  _deobfuscate(encoded, userEmail) {
    const key = userEmail || 'neuron-default-key';
    const decoded = atob(encoded);
    return decoded
      .split('')
      .map((c, i) => String.fromCharCode(c.charCodeAt(0) ^ key.charCodeAt(i % key.length)))
      .join('');
  },

  async save(provider, apiKey, label = '') {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');

    const encrypted = this._obfuscate(apiKey, user.email);

    const { data, error } = await supabase
      .from('api_keys')
      .upsert({
        user_id: user.id,
        provider,
        encrypted_key: encrypted,
        label,
        is_active: true,
      }, { onConflict: 'user_id,provider' })
      .select()
      .single();
    if (error) throw error;
    return data;
  },

  async get(provider) {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');

    const { data, error } = await supabase
      .from('api_keys')
      .select('*')
      .eq('user_id', user.id)
      .eq('provider', provider)
      .eq('is_active', true)
      .single();
    if (error && error.code !== 'PGRST116') throw error; // PGRST116 = not found
    if (!data) return null;

    return {
      ...data,
      decrypted_key: this._deobfuscate(data.encrypted_key, user.email),
    };
  },

  async list() {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');

    const { data, error } = await supabase
      .from('api_keys')
      .select('id, provider, label, is_active, created_at, updated_at')
      .eq('user_id', user.id);
    if (error) throw error;
    return data;
  },

  async delete(provider) {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');
    const { error } = await supabase
      .from('api_keys')
      .delete()
      .eq('user_id', user.id)
      .eq('provider', provider);
    if (error) throw error;
  },
};

// ═══════════════════════════════════════════
// Settings Module (cross-device sync)
// ═══════════════════════════════════════════

export const settings = {
  async set(key, value) {
    const user = await auth.getUser();
    if (!user) throw new Error('Not authenticated');
    const { data, error } = await supabase
      .from('user_settings')
      .upsert({
        user_id: user.id,
        setting_key: key,
        setting_value: JSON.stringify(value),
      }, { onConflict: 'user_id,setting_key' })
      .select()
      .single();
    if (error) throw error;
    return data;
  },

  async get(key, defaultValue = null) {
    const user = await auth.getUser();
    if (!user) return defaultValue;
    const { data, error } = await supabase
      .from('user_settings')
      .select('setting_value')
      .eq('user_id', user.id)
      .eq('setting_key', key)
      .single();
    if (error || !data) return defaultValue;
    try {
      return JSON.parse(data.setting_value);
    } catch {
      return data.setting_value;
    }
  },

  async getAll() {
    const user = await auth.getUser();
    if (!user) return {};
    const { data, error } = await supabase
      .from('user_settings')
      .select('setting_key, setting_value')
      .eq('user_id', user.id);
    if (error) return {};
    const result = {};
    for (const row of data) {
      try {
        result[row.setting_key] = JSON.parse(row.setting_value);
      } catch {
        result[row.setting_key] = row.setting_value;
      }
    }
    return result;
  },

  subscribeToChanges(userId, callback) {
    return supabase
      .channel('settings-sync')
      .on('postgres_changes', {
        event: '*',
        schema: 'public',
        table: 'user_settings',
        filter: `user_id=eq.${userId}`,
      }, callback)
      .subscribe();
  },
};

// ═══════════════════════════════════════════
// Devices Module
// ═══════════════════════════════════════════

export const devices = {
  async list() {
    const user = await auth.getUser();
    if (!user) return [];
    const { data, error } = await supabase
      .from('devices')
      .select('*')
      .eq('user_id', user.id)
      .order('last_sync_at', { ascending: false });
    if (error) return [];
    return data;
  },

  async updateLastSync() {
    await supabase
      .from('devices')
      .update({ last_sync_at: new Date().toISOString() })
      .eq('id', DEVICE_ID);
  },

  async remove(deviceId) {
    const { error } = await supabase.from('devices').delete().eq('id', deviceId);
    if (error) throw error;
  },
};

// ═══════════════════════════════════════════
// Sync Engine — Real-time cross-device sync
// ═══════════════════════════════════════════

export class SyncEngine {
  constructor() {
    this.subscriptions = [];
    this.listeners = new Map();
  }

  /** Start listening for real-time changes */
  async start(userId) {
    // Clean up any existing subscriptions
    this.stop();

    // Subscribe to session changes
    const sessionSub = sessions.subscribeToChanges(userId, (payload) => {
      // Don't react to our own changes
      if (payload.new?.device_id === DEVICE_ID) return;
      this._emit('session_change', payload);
    });
    this.subscriptions.push(sessionSub);

    // Subscribe to settings changes
    const settingsSub = settings.subscribeToChanges(userId, (payload) => {
      this._emit('settings_change', payload);
    });
    this.subscriptions.push(settingsSub);

    // Update device last-sync timestamp
    await devices.updateLastSync();
  }

  /** Subscribe to a specific message session */
  subscribeToSession(sessionId) {
    const sub = messages.subscribeToSession(sessionId, (payload) => {
      if (payload.new?.device_id === DEVICE_ID) return;
      this._emit('message_received', payload);
    });
    this.subscriptions.push(sub);
    return sub;
  }

  /** Stop all subscriptions */
  stop() {
    for (const sub of this.subscriptions) {
      supabase.removeChannel(sub);
    }
    this.subscriptions = [];
  }

  /** Add event listener */
  on(event, callback) {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event).add(callback);
    return () => this.listeners.get(event)?.delete(callback);
  }

  /** Emit event to listeners */
  _emit(event, data) {
    const callbacks = this.listeners.get(event);
    if (callbacks) {
      for (const cb of callbacks) {
        try { cb(data); } catch (e) { console.error('Sync listener error:', e); }
      }
    }
  }
}

export const syncEngine = new SyncEngine();
