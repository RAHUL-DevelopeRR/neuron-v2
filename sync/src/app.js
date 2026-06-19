/**
 * Neuron Sync — Main Application
 *
 * Complete working prototype demonstrating:
 * 1. Auth (email/password sign up & sign in + OAuth placeholders)
 * 2. Persistent chat sessions & messages (Supabase PostgreSQL)
 * 3. Cross-device real-time sync (Supabase Realtime)
 * 4. Encrypted API key storage
 * 5. Device tracking
 * 6. Settings sync
 *
 * ────────────────────────────────────────────────────────────
 * DEMO MODE: When Supabase is not configured, the app runs
 * with a local in-memory store so you can explore the UI.
 * ────────────────────────────────────────────────────────────
 */

import { auth, sessions, messages, apiKeys, settings, devices, syncEngine, DEVICE_ID, supabase } from './neuron-sync.js';

// ═══════════════════════════════════════════
// Demo/Local Mode Fallback
// ═══════════════════════════════════════════

const SUPABASE_URL = import.meta.env.VITE_SUPABASE_URL || '';
const IS_DEMO = !SUPABASE_URL || SUPABASE_URL.includes('YOUR_PROJECT');

// Local in-memory store for demo mode
const demoStore = {
  user: null,
  sessions: [],
  messages: {},
  apiKeys: {},
  settings: {},
  devices: [],
};

const demo = {
  async signUp(email, password, name) {
    demoStore.user = {
      id: 'demo-' + Date.now(),
      email,
      user_metadata: { display_name: name || email.split('@')[0] },
    };
    demoStore.devices.push({
      id: DEVICE_ID,
      device_name: navigator.platform,
      device_type: 'web',
      os: navigator.platform,
      last_sync_at: new Date().toISOString(),
    });
    return { user: demoStore.user };
  },

  async signIn(email, password) {
    // In demo mode, just auto-create a user
    return this.signUp(email, password);
  },

  async getUser() {
    return demoStore.user;
  },

  async getProfile() {
    if (!demoStore.user) return null;
    return {
      id: demoStore.user.id,
      display_name: demoStore.user.user_metadata?.display_name || demoStore.user.email,
      preferred_model: 'Kimi-K2.5',
      preferred_provider: 'neuron',
    };
  },

  async createSession(title, model, provider) {
    const session = {
      id: crypto.randomUUID(),
      title,
      model: model || 'Kimi-K2.5',
      provider: provider || 'neuron',
      prompt_tokens: 0,
      completion_tokens: 0,
      cost: 0,
      is_pinned: false,
      is_archived: false,
      device_id: DEVICE_ID,
      sync_version: 1,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      messages: [{ count: 0 }],
    };
    demoStore.sessions.unshift(session);
    demoStore.messages[session.id] = [];
    return session;
  },

  async listSessions() {
    return demoStore.sessions.filter(s => !s.is_archived);
  },

  async createMessage(sessionId, role, content) {
    const msg = {
      id: crypto.randomUUID(),
      session_id: sessionId,
      role,
      content,
      model: role === 'assistant' ? 'Kimi-K2.5' : null,
      device_id: DEVICE_ID,
      token_count: Math.ceil(content.length / 4),
      metadata: {},
      created_at: new Date().toISOString(),
    };
    if (!demoStore.messages[sessionId]) demoStore.messages[sessionId] = [];
    demoStore.messages[sessionId].push(msg);

    // Update session
    const session = demoStore.sessions.find(s => s.id === sessionId);
    if (session) {
      session.updated_at = new Date().toISOString();
      session.messages = [{ count: demoStore.messages[sessionId].length }];
    }
    return msg;
  },

  async listMessages(sessionId) {
    return demoStore.messages[sessionId] || [];
  },

  async saveApiKey(provider, key) {
    demoStore.apiKeys[provider] = { provider, masked: '••••' + key.slice(-4), saved_at: new Date().toISOString() };
    return demoStore.apiKeys[provider];
  },

  async listApiKeys() {
    return Object.values(demoStore.apiKeys);
  },

  async listDevices() {
    return demoStore.devices;
  },

  async deleteSession(sessionId) {
    demoStore.sessions = demoStore.sessions.filter(s => s.id !== sessionId);
    delete demoStore.messages[sessionId];
  },
};


// ═══════════════════════════════════════════
// State
// ═══════════════════════════════════════════

let state = {
  user: null,
  profile: null,
  currentSession: null,
  sessionList: [],
  messageList: [],
  showSettings: false,
  loading: false,
  authMode: 'signin', // 'signin' or 'signup'
};


// ═══════════════════════════════════════════
// Toast System
// ═══════════════════════════════════════════

function showToast(message, type = 'info') {
  const container = document.getElementById('toast-container');
  const icons = { success: '✓', error: '✕', info: 'ℹ', sync: '⟳' };
  const toast = document.createElement('div');
  toast.className = `toast ${type}`;
  toast.innerHTML = `<span>${icons[type] || '•'}</span><span>${message}</span>`;
  container.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = '0';
    toast.style.transform = 'translateX(40px)';
    toast.style.transition = 'all 0.3s ease-out';
    setTimeout(() => toast.remove(), 300);
  }, 3500);
}


// ═══════════════════════════════════════════
// Rendering
// ═══════════════════════════════════════════

function render() {
  const app = document.getElementById('app');
  if (!state.user) {
    app.innerHTML = renderAuthScreen();
    attachAuthListeners();
  } else {
    app.innerHTML = renderDashboard();
    attachDashboardListeners();
  }
}


// ── Auth Screen ──

function renderAuthScreen() {
  const isSignUp = state.authMode === 'signup';
  return `
    <div class="auth-wrapper">
      <div class="auth-card">
        <div class="auth-logo">
          <h1>⚡ Neuron Sync</h1>
          <p>Persistent history · Cross-device sync · Secure auth</p>
          ${IS_DEMO ? '<p style="color: var(--status-warning); margin-top: 8px; font-size: 0.75rem;">🧪 Running in Demo Mode — data stored locally</p>' : ''}
        </div>

        <div class="auth-tabs">
          <button class="auth-tab ${!isSignUp ? 'active' : ''}" data-tab="signin">Sign In</button>
          <button class="auth-tab ${isSignUp ? 'active' : ''}" data-tab="signup">Sign Up</button>
        </div>

        <div id="auth-error" class="auth-error" style="display: none;"></div>

        <form class="auth-form" id="auth-form">
          ${isSignUp ? `
            <div class="form-group">
              <label for="auth-name">Display Name</label>
              <input type="text" id="auth-name" class="form-input" placeholder="Your name" autocomplete="name" />
            </div>
          ` : ''}
          <div class="form-group">
            <label for="auth-email">Email</label>
            <input type="email" id="auth-email" class="form-input" placeholder="you@example.com" required autocomplete="email" />
          </div>
          <div class="form-group">
            <label for="auth-password">Password</label>
            <input type="password" id="auth-password" class="form-input" placeholder="${isSignUp ? 'Min. 6 characters' : '••••••••'}" required minlength="6" autocomplete="${isSignUp ? 'new-password' : 'current-password'}" />
          </div>
          <button type="submit" class="btn btn-primary" id="auth-submit">
            ${state.loading ? '<span class="spinner"></span>' : (isSignUp ? 'Create Account' : 'Sign In')}
          </button>
        </form>

        ${!IS_DEMO ? `
          <div class="oauth-divider">or continue with</div>
          <div class="oauth-buttons">
            <button class="btn btn-secondary" id="btn-github">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/></svg>
              GitHub
            </button>
            <button class="btn btn-secondary" id="btn-google">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 0 1-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4"/><path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/><path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05"/><path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/></svg>
              Google
            </button>
          </div>
        ` : ''}
      </div>
    </div>
  `;
}


// ── Dashboard ──

function renderDashboard() {
  const displayName = state.profile?.display_name || state.user?.email?.split('@')[0] || 'User';
  const initials = displayName.slice(0, 2).toUpperCase();

  return `
    <div class="dashboard">
      <!-- Sidebar -->
      <aside class="sidebar" id="sidebar">
        <div class="sidebar-header">
          <div class="sidebar-brand">
            <h2>⚡ Neuron</h2>
            <span class="sync-badge">${IS_DEMO ? 'DEMO' : 'SYNCED'}</span>
          </div>
          <button class="btn btn-primary btn-new-session" id="btn-new-session">
            <span>+</span> New Session
          </button>
        </div>

        <div class="session-list" id="session-list">
          ${state.sessionList.length === 0 ? `
            <div style="padding: 24px; text-align: center; color: var(--text-muted); font-size: 0.85rem;">
              No sessions yet.<br/>Start a new conversation!
            </div>
          ` : state.sessionList.map(s => renderSessionItem(s)).join('')}
        </div>

        <div class="sidebar-footer">
          <div class="user-avatar">${initials}</div>
          <div class="user-info">
            <div class="user-name">${displayName}</div>
            <div class="user-device">
              <span class="sync-dot"></span> This device
            </div>
          </div>
          <button class="btn btn-ghost btn-icon" id="btn-settings" title="Settings">⚙</button>
          <button class="btn btn-ghost btn-icon" id="btn-signout" title="Sign Out">⏻</button>
        </div>
      </aside>

      <!-- Main Chat Area -->
      <main class="main-area">
        <button class="btn btn-ghost mobile-menu-btn" id="btn-mobile-menu" style="position:absolute;top:16px;left:16px;z-index:50;">☰</button>

        ${state.currentSession ? renderChatArea() : renderEmptyState()}
      </main>

      ${state.showSettings ? renderSettingsPanel() : ''}
    </div>
  `;
}


function renderSessionItem(session) {
  const isActive = state.currentSession?.id === session.id;
  const date = new Date(session.updated_at);
  const timeStr = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  const msgCount = session.messages?.[0]?.count || 0;

  return `
    <div class="session-item ${isActive ? 'active' : ''}" data-session-id="${session.id}">
      <div class="session-item-title">
        ${session.is_pinned ? '<span class="session-pinned">📌 </span>' : ''}
        ${escapeHtml(session.title)}
      </div>
      <div class="session-item-meta">
        <span>${timeStr}</span>
        <span>·</span>
        <span>${msgCount} msg${msgCount !== 1 ? 's' : ''}</span>
        ${session.device_id !== DEVICE_ID ? `<span class="session-device-badge">other device</span>` : ''}
        <div class="session-actions">
          <button class="btn btn-ghost btn-icon" data-action="delete" data-id="${session.id}" title="Delete">🗑</button>
        </div>
      </div>
    </div>
  `;
}


function renderChatArea() {
  const session = state.currentSession;
  return `
    <div class="main-header">
      <div class="main-title">${escapeHtml(session.title)}</div>
      <span class="main-model-badge">${session.model || 'Kimi-K2.5'}</span>
      <div class="sync-indicator">
        <span class="sync-dot"></span>
        <span>Synced</span>
      </div>
    </div>

    <div class="chat-messages" id="chat-messages">
      ${state.messageList.length === 0 ? `
        <div class="message system">
          <div class="message-content">Session started. Your messages are ${IS_DEMO ? 'stored locally in demo mode' : 'synced across all your devices in real-time'}.</div>
        </div>
      ` : ''}
      ${state.messageList.map(m => renderMessage(m)).join('')}
    </div>

    <div class="chat-input-area">
      <div class="chat-input-wrapper">
        <textarea class="chat-input" id="chat-input" placeholder="Type a message... (Enter to send)" rows="1"></textarea>
        <button class="btn btn-primary btn-send" id="btn-send" title="Send">↑</button>
      </div>
    </div>
  `;
}


function renderMessage(msg) {
  const time = new Date(msg.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const isOtherDevice = msg.device_id && msg.device_id !== DEVICE_ID;

  return `
    <div class="message ${msg.role}">
      <div class="message-role">${msg.role === 'user' ? '👤 You' : msg.role === 'assistant' ? '🤖 Neuron' : '⚙ System'}</div>
      <div class="message-content">${escapeHtml(msg.content)}</div>
      <div class="message-meta">
        <span>${time}</span>
        ${msg.token_count ? `<span>· ${msg.token_count} tokens</span>` : ''}
        ${isOtherDevice ? `<span class="message-device-tag">📱 other device</span>` : ''}
        <span class="message-synced-icon" title="Synced to cloud">☁</span>
      </div>
    </div>
  `;
}


function renderEmptyState() {
  return `
    <div class="empty-state">
      <div class="empty-state-icon">💬</div>
      <h3>Welcome to Neuron Sync</h3>
      <p>Your chats are now persistent and synced across devices. Start a new session or select an existing one from the sidebar.</p>
      <button class="btn btn-primary" id="btn-empty-new-session">
        <span>+</span> New Session
      </button>
    </div>
  `;
}


function renderSettingsPanel() {
  return `
    <div class="settings-overlay" id="settings-overlay">
      <div class="settings-panel">
        <div class="settings-header">
          <h2>⚙ Settings & Sync</h2>
          <button class="btn btn-ghost btn-icon" id="btn-close-settings">✕</button>
        </div>
        <div class="settings-body">

          <!-- Profile -->
          <div class="settings-section">
            <h3>👤 Profile</h3>
            <div class="settings-row">
              <span class="settings-row-label">Email</span>
              <span class="settings-row-value">${state.user?.email || 'demo@neuron.dev'}</span>
            </div>
            <div class="settings-row">
              <span class="settings-row-label">Device ID</span>
              <span class="settings-row-value" style="font-size: 0.7rem;">${DEVICE_ID.slice(0, 20)}...</span>
            </div>
          </div>

          <!-- API Keys -->
          <div class="settings-section">
            <h3>🔑 API Keys <span style="font-weight: 400; font-size: 0.75rem; color: var(--text-muted);">(encrypted & synced)</span></h3>
            ${['openai', 'anthropic', 'gemini', 'openrouter', 'azure'].map(provider => `
              <div class="settings-row">
                <span class="settings-row-label">${provider.charAt(0).toUpperCase() + provider.slice(1)}</span>
                <input type="password" class="form-input api-key-input" data-provider="${provider}" placeholder="sk-..." />
                <button class="btn btn-ghost btn-icon api-key-save" data-provider="${provider}" title="Save">💾</button>
              </div>
            `).join('')}
          </div>

          <!-- Connected Devices -->
          <div class="settings-section">
            <h3>📱 Connected Devices</h3>
            <div class="device-list" id="device-list">
              <div class="device-item">
                <span class="device-icon">💻</span>
                <div class="device-info">
                  <div class="device-name">${navigator.platform}</div>
                  <div class="device-meta">This browser · Last sync: just now</div>
                </div>
                <span class="device-current">CURRENT</span>
              </div>
            </div>
          </div>

          <!-- Sync Status -->
          <div class="settings-section">
            <h3>⟳ Sync Status</h3>
            <div class="settings-row">
              <span class="settings-row-label">Mode</span>
              <span class="settings-row-value" style="color: ${IS_DEMO ? 'var(--status-warning)' : 'var(--status-success)'};">
                ${IS_DEMO ? '🧪 Demo (Local)' : '☁ Cloud (Supabase)'}
              </span>
            </div>
            <div class="settings-row">
              <span class="settings-row-label">Sessions</span>
              <span class="settings-row-value">${state.sessionList.length} sessions</span>
            </div>
            <div class="settings-row">
              <span class="settings-row-label">Real-time</span>
              <span class="settings-row-value" style="color: var(--status-success);">● Connected</span>
            </div>
          </div>

          <!-- Danger Zone -->
          <div class="settings-section">
            <h3>⚠ Account</h3>
            <button class="btn btn-danger" id="btn-signout-settings" style="width: 100%;">Sign Out</button>
          </div>
        </div>
      </div>
    </div>
  `;
}


// ═══════════════════════════════════════════
// Event Listeners
// ═══════════════════════════════════════════

function attachAuthListeners() {
  // Tab switching
  document.querySelectorAll('.auth-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      state.authMode = tab.dataset.tab === 'signup' ? 'signup' : 'signin';
      render();
    });
  });

  // Form submission
  document.getElementById('auth-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const email = document.getElementById('auth-email').value;
    const password = document.getElementById('auth-password').value;
    const name = document.getElementById('auth-name')?.value;
    const errorEl = document.getElementById('auth-error');

    state.loading = true;
    render();

    try {
      if (IS_DEMO) {
        const result = state.authMode === 'signup'
          ? await demo.signUp(email, password, name)
          : await demo.signIn(email, password);
        state.user = result.user;
        state.profile = await demo.getProfile();
      } else {
        const result = state.authMode === 'signup'
          ? await auth.signUp(email, password, name)
          : await auth.signIn(email, password);
        state.user = result.user;
        state.profile = await auth.getProfile();
        // Start real-time sync
        await syncEngine.start(state.user.id);
      }
      state.loading = false;
      showToast('Welcome to Neuron Sync!', 'success');
      await loadSessions();
      render();
    } catch (err) {
      state.loading = false;
      render();
      const errorEl = document.getElementById('auth-error');
      if (errorEl) {
        errorEl.textContent = err.message || 'Authentication failed';
        errorEl.style.display = 'block';
      }
    }
  });

  // OAuth
  document.getElementById('btn-github')?.addEventListener('click', async () => {
    try { await auth.signInWithGitHub(); } catch (e) { showToast(e.message, 'error'); }
  });
  document.getElementById('btn-google')?.addEventListener('click', async () => {
    try { await auth.signInWithGoogle(); } catch (e) { showToast(e.message, 'error'); }
  });
}


function attachDashboardListeners() {
  // New session
  document.getElementById('btn-new-session')?.addEventListener('click', createNewSession);
  document.getElementById('btn-empty-new-session')?.addEventListener('click', createNewSession);

  // Session selection
  document.querySelectorAll('.session-item').forEach(item => {
    item.addEventListener('click', async (e) => {
      if (e.target.closest('[data-action]')) return;
      const sessionId = item.dataset.sessionId;
      await selectSession(sessionId);
    });
  });

  // Session actions (delete)
  document.querySelectorAll('[data-action="delete"]').forEach(btn => {
    btn.addEventListener('click', async (e) => {
      e.stopPropagation();
      const id = btn.dataset.id;
      if (confirm('Delete this session?')) {
        if (IS_DEMO) {
          await demo.deleteSession(id);
        } else {
          await sessions.delete(id);
        }
        if (state.currentSession?.id === id) {
          state.currentSession = null;
          state.messageList = [];
        }
        await loadSessions();
        render();
        showToast('Session deleted', 'info');
      }
    });
  });

  // Chat input
  const chatInput = document.getElementById('chat-input');
  chatInput?.addEventListener('keydown', async (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      await sendMessage();
    }
  });

  // Auto-resize textarea
  chatInput?.addEventListener('input', () => {
    chatInput.style.height = 'auto';
    chatInput.style.height = Math.min(chatInput.scrollHeight, 200) + 'px';
  });

  // Send button
  document.getElementById('btn-send')?.addEventListener('click', sendMessage);

  // Settings
  document.getElementById('btn-settings')?.addEventListener('click', () => {
    state.showSettings = true;
    render();
  });

  document.getElementById('btn-close-settings')?.addEventListener('click', () => {
    state.showSettings = false;
    render();
  });

  document.getElementById('settings-overlay')?.addEventListener('click', (e) => {
    if (e.target.id === 'settings-overlay') {
      state.showSettings = false;
      render();
    }
  });

  // Sign out
  document.getElementById('btn-signout')?.addEventListener('click', signOut);
  document.getElementById('btn-signout-settings')?.addEventListener('click', signOut);

  // API key save
  document.querySelectorAll('.api-key-save').forEach(btn => {
    btn.addEventListener('click', async () => {
      const provider = btn.dataset.provider;
      const input = document.querySelector(`input[data-provider="${provider}"]`);
      const key = input?.value;
      if (!key) return;

      try {
        if (IS_DEMO) {
          await demo.saveApiKey(provider, key);
        } else {
          await apiKeys.save(provider, key);
        }
        input.value = '';
        input.placeholder = '••••' + key.slice(-4) + ' (saved)';
        showToast(`${provider} API key saved & synced`, 'success');
      } catch (err) {
        showToast(`Failed to save key: ${err.message}`, 'error');
      }
    });
  });

  // Mobile menu
  document.getElementById('btn-mobile-menu')?.addEventListener('click', () => {
    document.getElementById('sidebar')?.classList.toggle('mobile-open');
  });

  // Scroll chat to bottom
  scrollChatToBottom();
}


// ═══════════════════════════════════════════
// Actions
// ═══════════════════════════════════════════

async function createNewSession() {
  try {
    const session = IS_DEMO
      ? await demo.createSession('New Session')
      : await sessions.create('New Session');

    state.currentSession = session;
    state.messageList = [];
    await loadSessions();
    render();

    // Focus the input
    setTimeout(() => document.getElementById('chat-input')?.focus(), 100);
    showToast('New session created', 'success');
  } catch (err) {
    showToast('Failed to create session: ' + err.message, 'error');
  }
}


async function selectSession(sessionId) {
  try {
    const session = state.sessionList.find(s => s.id === sessionId);
    if (!session) return;

    state.currentSession = session;
    state.messageList = IS_DEMO
      ? await demo.listMessages(sessionId)
      : await messages.listBySession(sessionId);

    render();
    scrollChatToBottom();

    // Subscribe to real-time messages for this session
    if (!IS_DEMO) {
      syncEngine.subscribeToSession(sessionId);
    }
  } catch (err) {
    showToast('Failed to load session: ' + err.message, 'error');
  }
}


async function sendMessage() {
  const input = document.getElementById('chat-input');
  const content = input?.value?.trim();
  if (!content || !state.currentSession) return;

  input.value = '';
  input.style.height = 'auto';

  try {
    // Save user message
    const userMsg = IS_DEMO
      ? await demo.createMessage(state.currentSession.id, 'user', content)
      : await messages.create(state.currentSession.id, 'user', content);

    state.messageList.push(userMsg);
    render();
    scrollChatToBottom();

    // Simulate assistant response (in a real app, this would call the LLM provider)
    setTimeout(async () => {
      const reply = generateDemoReply(content);
      const assistantMsg = IS_DEMO
        ? await demo.createMessage(state.currentSession.id, 'assistant', reply)
        : await messages.create(state.currentSession.id, 'assistant', reply, 'Kimi-K2.5');

      state.messageList.push(assistantMsg);

      // Auto-update session title from first message
      if (state.messageList.filter(m => m.role === 'user').length === 1) {
        const title = content.length > 50 ? content.slice(0, 50) + '...' : content;
        if (IS_DEMO) {
          const s = demoStore.sessions.find(s => s.id === state.currentSession.id);
          if (s) s.title = title;
        } else {
          await sessions.update(state.currentSession.id, { title });
        }
        state.currentSession.title = title;
        await loadSessions();
      }

      render();
      scrollChatToBottom();
    }, 800 + Math.random() * 800);

  } catch (err) {
    showToast('Failed to send message: ' + err.message, 'error');
  }
}


async function loadSessions() {
  try {
    state.sessionList = IS_DEMO
      ? await demo.listSessions()
      : await sessions.list();
  } catch (err) {
    console.error('Failed to load sessions:', err);
  }
}


async function signOut() {
  if (!IS_DEMO) {
    syncEngine.stop();
    await auth.signOut();
  }
  state = {
    user: null,
    profile: null,
    currentSession: null,
    sessionList: [],
    messageList: [],
    showSettings: false,
    loading: false,
    authMode: 'signin',
  };
  // Clear demo store too
  demoStore.user = null;
  demoStore.sessions = [];
  demoStore.messages = {};
  render();
  showToast('Signed out', 'info');
}


function scrollChatToBottom() {
  requestAnimationFrame(() => {
    const container = document.getElementById('chat-messages');
    if (container) {
      container.scrollTop = container.scrollHeight;
    }
  });
}


// ═══════════════════════════════════════════
// Demo Reply Generator
// ═══════════════════════════════════════════

function generateDemoReply(userMessage) {
  const lower = userMessage.toLowerCase();

  if (lower.includes('sync') || lower.includes('device')) {
    return `Great question about sync! In the Neuron Sync architecture:\n\n• **Real-time sync** uses Supabase Realtime (WebSocket-based PostgreSQL change data capture)\n• Each message carries a \`device_id\` so you can see which device sent it\n• Conflict resolution uses \`sync_version\` — last-write-wins with version incrementing\n• API keys are encrypted client-side before syncing\n\nOpen another browser tab to see messages appear in real-time! 🔄`;
  }

  if (lower.includes('auth') || lower.includes('login') || lower.includes('sign')) {
    return `Neuron Sync supports multiple auth methods:\n\n• **Email/Password** — Supabase Auth with automatic profile creation\n• **GitHub OAuth** — One-click sign in with your GitHub account\n• **Google OAuth** — Sign in with Google\n\nAll sessions are automatically linked to your user ID. Row Level Security (RLS) ensures you can only see your own data. 🔐`;
  }

  if (lower.includes('api') || lower.includes('key')) {
    return `API keys in Neuron Sync are:\n\n1. **Client-side encrypted** before being sent to the database\n2. **Synced across devices** — set your OpenAI key once, use everywhere\n3. **Per-provider** — store keys for OpenAI, Anthropic, Gemini, etc.\n4. **Securely stored** — RLS prevents any user from accessing another's keys\n\nGo to ⚙ Settings to manage your keys! 🔑`;
  }

  if (lower.includes('hello') || lower.includes('hi') || lower.includes('hey')) {
    return `Hello! 👋 Welcome to Neuron Sync — the persistence & cross-platform sync layer for NeuronCLI.\n\nThis prototype demonstrates:\n• 🔐 Authentication (email, GitHub, Google)\n• 💬 Persistent chat history\n• ⟳ Real-time cross-device sync\n• 🔑 Encrypted API key storage\n• 📱 Device tracking\n\nTry asking about any of these features!`;
  }

  if (lower.includes('help') || lower.includes('what can')) {
    return `Here's what Neuron Sync adds to NeuronCLI:\n\n**Currently missing (and now solved):**\n• ❌ No persistent chat history → ✅ Supabase PostgreSQL\n• ❌ No auth/sign-in → ✅ Supabase Auth (email + OAuth)\n• ❌ No cross-device sync → ✅ Supabase Realtime\n• ❌ API keys only in env vars → ✅ Encrypted cloud storage\n• ❌ No settings persistence → ✅ User settings table\n\nAll data is scoped per-user with Row Level Security.`;
  }

  const responses = [
    `I've processed your message. In a production setup, this would be routed to your configured LLM provider (${state.currentSession?.model || 'Kimi-K2.5'}) via the Neuron gateway.\n\nThe key point: this message is now **persistently stored** and would sync to any other device you're signed into. ✨`,
    `Message received and persisted! 📝\n\nIn the real Neuron integration, this sync engine connects to the Go backend's session/message stores. The SQLite data at \`.neuron/opencode.db\` would be mirrored to Supabase for cross-device access.\n\nDevice: ${DEVICE_ID.slice(0, 15)}...`,
    `Your message has been saved to the sync layer. Here's what happened behind the scenes:\n\n1. Message inserted into \`messages\` table with your \`user_id\`\n2. Session \`updated_at\` bumped for sort order\n3. Supabase Realtime broadcast sent to other connected devices\n4. \`sync_version\` incremented for conflict resolution\n\nAll automatic! 🚀`,
  ];

  return responses[Math.floor(Math.random() * responses.length)];
}


// ═══════════════════════════════════════════
// Utilities
// ═══════════════════════════════════════════

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}


// ═══════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════

async function init() {
  // Check for existing Supabase session (non-demo mode)
  if (!IS_DEMO) {
    try {
      const user = await auth.getUser();
      if (user) {
        state.user = user;
        state.profile = await auth.getProfile();
        await syncEngine.start(user.id);
        await loadSessions();

        // Listen for real-time sync events
        syncEngine.on('session_change', async (payload) => {
          showToast('Session updated from another device', 'sync');
          await loadSessions();
          render();
        });

        syncEngine.on('message_received', async (payload) => {
          if (state.currentSession?.id === payload.new.session_id) {
            state.messageList.push(payload.new);
            render();
            scrollChatToBottom();
            showToast('New message from another device', 'sync');
          }
        });
      }
    } catch (err) {
      console.log('No existing session');
    }

    // Listen for auth state changes (OAuth redirects)
    auth.onAuthStateChange(async (event, session) => {
      if (event === 'SIGNED_IN' && session?.user) {
        state.user = session.user;
        state.profile = await auth.getProfile();
        await syncEngine.start(session.user.id);
        await loadSessions();
        render();
      }
    });
  }

  render();
}

init();
