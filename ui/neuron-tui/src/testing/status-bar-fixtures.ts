/**
 * Engine events for the status bar's tests and the mock: a Zero-X session on
 * `auto` served by Gemini, signed in on the Free plan.
 */

import type { AccountDetails, ModelsEvent, StatusEvent } from '../engine/protocol'

export const STATUS_BAR_STATUS: StatusEvent = {
  type: 'status',
  model: 'gemini-2.5-flash',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: '/home/dev/neuron-v2',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 4,
  git: {
    branch: 'feat/tui-status-bar',
    changed: 3,
    staged: 1,
    unstaged: 2,
    untracked: 0,
    conflicted: 0,
  },
  signed_in: true,
  sign_in_required: false,
  account: { email: 'dev@example.com', plan: 'free', source: 'stored login', expires_at: null },
  supports_images: true,
  served_model: null,
  provider: 'gemini',
  model_display_name: 'Gemini 2.5 Flash',
  reasoning_effort: 'high',
  reasoning_efforts: ['none', 'low', 'medium', 'high'],
  full_access_allowed: false,
  context: {
    context_window: 1_048_576,
    output_reserve: 65_536,
    system_tokens: 4_100,
    tool_schema_tokens: 11_800,
    tool_count: 23,
    instruction_tokens: 2_300,
    instruction_files: ['/home/dev/neuron-v2/NEURON.md', '/home/dev/.neuron/memory.md'],
    conversation_tokens: 180_000,
    tool_output_tokens: 146_000,
    used_tokens: 344_200,
    threshold_tokens: 786_432,
  },
}

export const STATUS_BAR_ACCOUNT: AccountDetails = {
  email: 'dev@example.com',
  name: 'Dev Example',
  plan: 'free',
  plan_name: 'Free',
  source: 'stored login',
  expires_at: null,
  limits: { requests: 100, tokens: null },
  usage_today: { requests: 37, tokens: 120_000 },
  resets_at: '2026-11-04T00:00:00Z',
  credit_balance_cents: null,
  suspended: false,
  dashboard_url: 'https://example.com/dashboard',
  pricing_url: 'https://example.com/pricing',
  usage_updated_at: 1_793_404_800,
  usage_error: null,
  usage_window: {
    tokens: 120_000,
    limit_tokens: 1_000_000,
    remaining_tokens: 880_000,
    resets_at: '2026-11-03T14:32:00Z',
    window_seconds: 18_000,
  },
}

export const STATUS_BAR_MODELS: ModelsEvent = {
  type: 'models',
  current: 'gemini-2.5-flash',
  models: [
    {
      id: 'auto',
      display_name: 'Auto',
      available: true,
      unavailable_reason: null,
      context_window: 1_048_576,
      tiers: ['free', 'pro', 'max'],
      provider: 'zero-x',
      reasoning_efforts: null,
      supports_reasoning: null,
    },
    {
      id: 'gemini-2.5-flash',
      display_name: 'Gemini 2.5 Flash',
      available: true,
      unavailable_reason: null,
      context_window: 1_048_576,
      tiers: ['free', 'pro', 'max'],
      status: 'available',
      provider: 'gemini',
      reasoning_efforts: ['none', 'low', 'medium', 'high'],
      supports_reasoning: true,
    },
    {
      id: 'glm-5.2',
      display_name: 'GLM 5.2',
      available: false,
      unavailable_reason: 'upgrade_required',
      context_window: 200_000,
      tiers: ['pro', 'max'],
      provider: 'cloudflare',
      reasoning_efforts: ['low', 'medium', 'high', 'max'],
      supports_reasoning: true,
    },
  ],
}
