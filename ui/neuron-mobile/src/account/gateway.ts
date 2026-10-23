/**
 * The Zero-X gateway calls the phone makes (docs/GATEWAY_CONTRACT.md §3, and
 * docs/RELAY_CONTRACT.md §7 for push and linked devices): device-code sign-in,
 * the account and today's usage, push registration and sign-out.
 *
 * The session token is held by the caller and passed in; this module never
 * logs it, and it only ever travels in an Authorization header.
 */

import { isPrivateHost } from '../relay/pairing'

export const DEVICE_GRANT_TYPE = 'urn:ietf:params:oauth:grant-type:device_code'
export const PRODUCTION_GATEWAY = 'https://zero-x.live'

export interface DeviceCode {
  device_code: string
  user_code: string
  verification_uri: string
  verification_uri_complete: string | null
  expires_in: number
  interval: number
}

export interface SignedIn {
  token: string
  expiresAt: string | null
}

export interface Account {
  user: { id: string; email: string; name: string | null; image_url: string | null }
  plan: { id: string; name: string }
  limits: { daily_requests?: number; daily_tokens?: number }
  usage_today: { requests: number; tokens: number; resets_at: string | null }
  flags?: { suspended?: boolean }
}

export interface LinkedSession {
  id: string
  client: string | null
  device_name: string | null
  os: string | null
  created_at: string
  last_used_at: string | null
  current: boolean
}

export class GatewayError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly retryAfter: number | null = null,
  ) {
    super(message)
    this.name = 'GatewayError'
  }
}

/** The gateway that goes with a relay: Zero-X's for Zero-X's relay, the dev relay itself otherwise. */
export function gatewayForRelay(relayUrl: string): string {
  const url = new URL(relayUrl)
  const host = url.hostname.toLowerCase()
  if (host === 'zero-x.live' || host.endsWith('.zero-x.live')) return PRODUCTION_GATEWAY
  return `${url.protocol === 'wss:' ? 'https' : 'http'}://${url.host}`
}

/** Plain http is for development gateways on this network only. */
export function checkGateway(base: string): string {
  const url = new URL(base)
  if (url.protocol === 'https:' || (url.protocol === 'http:' && isPrivateHost(url.hostname))) return `${url.protocol}//${url.host}`
  throw new GatewayError(0, 'insecure_gateway', 'Zero-X is only reached over https.')
}

type Fetch = (input: string, init?: { method?: string; headers?: Record<string, string>; body?: string }) => Promise<{
  ok: boolean
  status: number
  json(): Promise<unknown>
}>

export interface GatewayOptions {
  base: string
  fetch?: Fetch
  client?: { id: string; version: string; deviceName: string; os: string }
}

export class Gateway {
  readonly base: string
  private readonly fetcher: Fetch

  constructor(private readonly options: GatewayOptions) {
    this.base = checkGateway(options.base)
    this.fetcher = options.fetch ?? ((input, init) => fetch(input, init) as never)
  }

  private async call<T>(path: string, init: { method?: string; token?: string | null; body?: unknown } = {}): Promise<T> {
    const headers: Record<string, string> = { Accept: 'application/json' }
    if (init.body !== undefined) headers['Content-Type'] = 'application/json'
    if (init.token) headers.Authorization = `Bearer ${init.token}`
    let response
    try {
      response = await this.fetcher(`${this.base}${path}`, {
        method: init.method ?? (init.body === undefined ? 'GET' : 'POST'),
        headers,
        body: init.body === undefined ? undefined : JSON.stringify(init.body),
      })
    } catch {
      throw new GatewayError(0, 'network', 'Could not reach Zero-X. Check your connection.')
    }
    const body = (await response.json().catch(() => null)) as Record<string, unknown> | null
    if (!response.ok) {
      const error = (body?.error ?? {}) as Record<string, unknown>
      throw new GatewayError(
        response.status,
        typeof error.code === 'string' ? error.code : 'error',
        typeof error.message === 'string' ? error.message : `Zero-X answered ${response.status}.`,
        typeof error.interval === 'number' ? error.interval : typeof error.retry_after_seconds === 'number' ? error.retry_after_seconds : null,
      )
    }
    return body as T
  }

  /** Starts a device sign-in: the user approves `user_code` in the browser. */
  requestDeviceCode(): Promise<DeviceCode> {
    const client = this.options.client
    return this.call<DeviceCode>('/auth/device/code', {
      body: { client_id: client?.id ?? 'neuron-mobile', client_version: client?.version, device_name: client?.deviceName, os: client?.os },
    })
  }

  /**
   * Polls until the browser approved (or denied, or the code expired), as RFC
   * 8628 says: every `interval` seconds, five more after a `slow_down`.
   */
  async waitForApproval(
    code: DeviceCode,
    options: { signal?: { aborted: boolean }; sleep?: (ms: number) => Promise<void>; now?: () => number } = {},
  ): Promise<SignedIn> {
    const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)))
    const now = options.now ?? Date.now
    const deadline = now() + code.expires_in * 1000
    let interval = Math.max(1, code.interval)
    for (;;) {
      await sleep(interval * 1000)
      if (options.signal?.aborted) throw new GatewayError(0, 'cancelled', 'Sign-in was cancelled.')
      if (now() > deadline) throw new GatewayError(400, 'expired_token', 'The sign-in code expired. Start again.')
      try {
        const result = await this.call<{ access_token: string; expires_at?: string }>('/auth/device/token', {
          body: { grant_type: DEVICE_GRANT_TYPE, device_code: code.device_code },
        })
        return { token: result.access_token, expiresAt: result.expires_at ?? null }
      } catch (error) {
        if (!(error instanceof GatewayError)) throw error
        if (error.code === 'authorization_pending') continue
        if (error.code === 'slow_down') {
          interval += 5
          continue
        }
        if (error.code === 'network') continue
        if (error.code === 'access_denied') throw new GatewayError(400, error.code, 'Sign-in was denied in the browser.')
        if (error.code === 'expired_token') throw new GatewayError(400, error.code, 'The sign-in code expired. Start again.')
        throw error
      }
    }
  }

  account(token: string): Promise<Account> {
    return this.call<Account>('/api/account', { token })
  }

  /** Devices signed in to the account (A9); `null` where the gateway does not have the endpoint yet. */
  async linkedSessions(token: string): Promise<LinkedSession[] | null> {
    try {
      return (await this.call<{ sessions: LinkedSession[] }>('/api/sessions', { token })).sessions
    } catch (error) {
      if (error instanceof GatewayError && error.status === 404) return null
      throw error
    }
  }

  revokeSession(token: string, id: string): Promise<unknown> {
    return this.call(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE', token })
  }

  registerPush(token: string, registration: { deviceId: string; expoPushToken: string; platform: string; needsMe: boolean; finished: boolean }): Promise<unknown> {
    return this.call('/api/push/devices', {
      token,
      body: {
        device_id: registration.deviceId,
        expo_push_token: registration.expoPushToken,
        platform: registration.platform,
        needs_me: registration.needsMe,
        finished: registration.finished,
      },
    })
  }

  unregisterPush(token: string, deviceId: string): Promise<unknown> {
    return this.call(`/api/push/devices/${encodeURIComponent(deviceId)}`, { method: 'DELETE', token })
  }

  /** Signs this phone out on the gateway; a gateway without the endpoint just forgets nothing. */
  async signOut(token: string): Promise<void> {
    await this.revokeSession(token, 'current').catch(() => {})
  }
}
