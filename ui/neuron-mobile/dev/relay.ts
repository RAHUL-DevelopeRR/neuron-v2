/**
 * A local stand-in for the zero-x.live relay and the gateway endpoints the
 * phone uses, until WP5 deploys them. One Bun process serves:
 *
 * - `GET /v1/relay`: the relay WebSocket (./relay-room.ts), `neuron-relay/1`;
 * - device sign-in (`/auth/device/code`, `/auth/device/token`, and a page at
 *   `/device` that approves as any dev account);
 * - `GET /api/account`, `GET|DELETE /api/sessions[/:id]`, `POST|DELETE /api/push/devices`;
 * - dev-only helpers for scripts and tests under `/dev/*`.
 *
 * It is for development only: anyone who can reach it can approve a sign-in,
 * so it binds 127.0.0.1 unless `--lan` is given. Tokens are never printed.
 *
 *   bun run dev/relay.ts [--port 8790] [--lan] [--expo-push]
 */

import { createHash, randomBytes } from 'crypto'

import { AUTH_SUBPROTOCOL_PREFIX, CloseCode, LIMITS, RELAY_SUBPROTOCOL } from '../src/relay/contract'
import { RelayHub, type HubOptions, type PushRequest, type RelayAccount } from './relay-room'

export interface DevRelayOptions extends HubOptions {
  port?: number
  hostname?: string
  /** Seconds between device-code polls (the real gateway uses 5). */
  pollIntervalS?: number
  /** Send pushes through Expo's push service (needs network and real tokens). */
  expoPush?: boolean
  log?: (line: string) => void
}

interface DeviceGrant {
  deviceCode: string
  userCode: string
  status: 'pending' | 'approved' | 'denied'
  email: string | null
  expiresAt: number
  interval: number
  lastPoll: number
  client: { name: string; device_name: string | null; os: string | null }
}

interface CliSession {
  id: string
  email: string
  createdAt: number
  lastUsedAt: number
  expiresAt: number
  client: string
  deviceName: string | null
  os: string | null
}

interface PushDevice {
  deviceId: string
  token: string
  platform: string
  needs_me: boolean
  finished: boolean
}

export interface RecordedPush {
  to: string
  deviceId: string
  kind: PushRequest['kind']
  title: string
  body: string
  data: { neuron: { host_id: string; share_id: string; kind: string } }
  at: number
}

const USER_CODE_ALPHABET = 'BCDFGHJKLMNPQRSTVWXZ'
const SESSION_TTL_MS = 30 * 24 * 3600_000

const accountOf = (email: string): RelayAccount => ({ id: `acct_${createHash('sha256').update(email.toLowerCase()).digest('hex').slice(0, 12)}`, email: email.toLowerCase() })

function userCode(): string {
  let code = ''
  for (const byte of randomBytes(32)) if (byte < 240 && code.length < 8) code += USER_CODE_ALPHABET[byte % 20]
  return `${code.slice(0, 4)}-${code.slice(4)}`
}

const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`)

function pushText(push: PushRequest): { title: string; body: string } {
  return push.kind === 'needs_me'
    ? { title: 'Neuron needs you', body: `A session on ${push.hostName} is waiting for your answer.` }
    : { title: 'Neuron finished', body: `A session on ${push.hostName} finished its turn.` }
}

type SocketData = { account: RelayAccount | null; refuse: { code: number; reason: string } | null; connection: ReturnType<RelayHub['connect']> | null }

export function startDevRelay(options: DevRelayOptions = {}) {
  const log = options.log ?? (() => {})
  const grants = new Map<string, DeviceGrant>()
  const sessions = new Map<string, CliSession>()
  /** accountId → deviceId → device */
  const pushDevices = new Map<string, Map<string, PushDevice>>()
  const pushes: RecordedPush[] = []

  const hub = new RelayHub({
    ...options,
    onPush(push) {
      options.onPush?.(push)
      const devices = pushDevices.get(push.accountId)
      const { title, body } = pushText(push)
      for (const phoneId of push.phoneIds) {
        const device = devices?.get(phoneId)
        if (!device || !device[push.kind]) continue
        const message: RecordedPush = {
          to: device.token,
          deviceId: phoneId,
          kind: push.kind,
          title,
          body,
          data: { neuron: { host_id: push.hostId, share_id: push.shareId, kind: push.kind } },
          at: Date.now(),
        }
        pushes.push(message)
        log(`push ${push.kind} to a phone of ${push.accountId}`)
        if (options.expoPush) {
          void fetch('https://exp.host/--/api/v2/push/send', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
            body: JSON.stringify({ to: message.to, title, body, data: message.data, sound: 'default', channelId: push.kind === 'needs_me' ? 'needs-me' : 'finished' }),
          }).catch((error) => log(`expo push failed: ${error instanceof Error ? error.message : error}`))
        }
      }
    },
  })

  function mintSession(email: string, client = 'neuron-dev', deviceName: string | null = null, os: string | null = null): string {
    const token = `ses_${randomBytes(24).toString('base64url')}`
    const now = Date.now()
    sessions.set(token, { id: `sess_${randomBytes(6).toString('hex')}`, email: email.toLowerCase(), createdAt: now, lastUsedAt: now, expiresAt: now + SESSION_TTL_MS, client, deviceName, os })
    return token
  }

  function sessionOf(token: string | null): CliSession | null {
    if (!token) return null
    const session = sessions.get(token)
    if (!session || session.expiresAt < Date.now()) return null
    session.lastUsedAt = Date.now()
    return session
  }

  function bearer(request: Request): string | null {
    const header = request.headers.get('authorization') ?? ''
    return /^Bearer\s+(\S+)$/i.exec(header)?.[1] ?? null
  }

  function approve(code: string, email: string, decision: 'approve' | 'deny' = 'approve'): boolean {
    const wanted = code.toUpperCase().replace(/[^A-Z]/g, '')
    for (const grant of grants.values()) {
      if (grant.userCode.replace('-', '') !== wanted || grant.status !== 'pending' || grant.expiresAt < Date.now()) continue
      grant.status = decision === 'approve' ? 'approved' : 'denied'
      grant.email = email.toLowerCase()
      return true
    }
    return false
  }

  const cors = (request: Request): Record<string, string> => {
    const origin = request.headers.get('origin')
    // The web build runs on localhost or the LAN during development.
    return origin && /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\]|10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(origin)
      ? { 'Access-Control-Allow-Origin': origin, 'Access-Control-Allow-Headers': 'Authorization, Content-Type', 'Access-Control-Allow-Methods': 'GET, POST, DELETE, OPTIONS', Vary: 'Origin' }
      : {}
  }

  const json = (request: Request, body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...cors(request) } })
  const failure = (request: Request, status: number, code: string, message: string, extra: Record<string, unknown> = {}) =>
    json(request, { error: { message, type: status === 401 ? 'authentication_error' : 'invalid_request_error', code, param: null, request_id: `req_${randomBytes(6).toString('hex')}`, ...extra } }, status)

  const server = Bun.serve<SocketData>({
    hostname: options.hostname ?? '127.0.0.1',
    port: options.port ?? 8790,
    async fetch(request, server) {
      const url = new URL(request.url)
      const origin = `${url.protocol}//${request.headers.get('host') ?? url.host}`
      if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(request) })

      if (url.pathname === '/v1/relay') {
        const offered = (request.headers.get('sec-websocket-protocol') ?? '').split(',').map((part) => part.trim()).filter(Boolean)
        const token = bearer(request) ?? offered.find((part) => part.startsWith(AUTH_SUBPROTOCOL_PREFIX))?.slice(AUTH_SUBPROTOCOL_PREFIX.length) ?? null
        const session = sessionOf(token)
        // Refusals are close codes after the upgrade, which browsers can read; a 401 would look like a network error.
        const refuse = token?.startsWith('nzx_')
          ? { code: CloseCode.forbidden, reason: 'API keys cannot use the relay.' }
          : session
            ? null
            : { code: CloseCode.unauthorized, reason: 'Sign in to Zero-X again.' }
        const upgraded = server.upgrade(request, {
          headers: offered.includes(RELAY_SUBPROTOCOL) ? { 'Sec-WebSocket-Protocol': RELAY_SUBPROTOCOL } : {},
          data: { account: session ? accountOf(session.email) : null, refuse, connection: null },
        })
        return upgraded ? undefined : new Response('WebSocket upgrade required', { status: 426 })
      }

      if (url.pathname === '/auth/device/code' && request.method === 'POST') {
        const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
        const deviceCode = `zdc_${randomBytes(32).toString('base64url')}`
        const grant: DeviceGrant = {
          deviceCode,
          userCode: userCode(),
          status: 'pending',
          email: null,
          expiresAt: Date.now() + 600_000,
          interval: options.pollIntervalS ?? 1,
          lastPoll: 0,
          client: { name: String(body.client_id ?? 'neuron-cli').slice(0, 64), device_name: typeof body.device_name === 'string' ? body.device_name.slice(0, 128) : null, os: typeof body.os === 'string' ? body.os.slice(0, 64) : null },
        }
        grants.set(deviceCode, grant)
        const uri = `${origin}/device`
        return json(request, { device_code: deviceCode, user_code: grant.userCode, verification_uri: uri, verification_uri_complete: `${uri}?user_code=${grant.userCode}`, expires_in: 600, interval: grant.interval })
      }

      if (url.pathname === '/auth/device/token' && request.method === 'POST') {
        const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
        if (body.grant_type !== 'urn:ietf:params:oauth:grant-type:device_code') return failure(request, 400, 'unsupported_grant_type', 'Wrong grant_type.')
        const grant = grants.get(String(body.device_code ?? ''))
        if (!grant) return failure(request, 400, 'invalid_grant', 'Unknown device code.')
        if (grant.expiresAt < Date.now()) return failure(request, 400, 'expired_token', 'The sign-in code expired.')
        if (grant.status === 'denied') return failure(request, 400, 'access_denied', 'Sign-in was denied in the browser.')
        if (grant.status === 'pending') {
          const tooFast = Date.now() - grant.lastPoll < grant.interval * 1000 - 200
          grant.lastPoll = Date.now()
          return tooFast
            ? failure(request, 400, 'slow_down', 'Polling too fast.', { interval: grant.interval })
            : failure(request, 400, 'authorization_pending', 'Waiting for approval in the browser.', { interval: grant.interval })
        }
        grants.delete(grant.deviceCode)
        const token = mintSession(grant.email!, grant.client.name, grant.client.device_name, grant.client.os)
        const session = sessions.get(token)!
        return json(request, { access_token: token, token_type: 'Bearer', expires_in: SESSION_TTL_MS / 1000, expires_at: new Date(session.expiresAt).toISOString(), session_token: token })
      }

      if (url.pathname === '/device' && request.method === 'GET') {
        const code = escapeHtml(url.searchParams.get('user_code') ?? '')
        return new Response(
          `<!doctype html><meta name="viewport" content="width=device-width"><title>Approve Neuron sign-in (dev)</title>
<body style="font-family:system-ui;max-width:28rem;margin:3rem auto;padding:0 1rem">
<h1>Approve a Neuron sign-in</h1><p>Development relay: this approves as any account you type. Never expose it to a network you do not trust.</p>
<form method="post" action="/device/approve">
<label>Code <input name="user_code" value="${code}" autocomplete="off"></label><br><br>
<label>Account <input name="email" value="dev@zero-x.live"></label><br><br>
<button name="decision" value="approve">Approve</button> <button name="decision" value="deny">Deny</button></form></body>`,
          { headers: { 'Content-Type': 'text/html; charset=utf-8' } },
        )
      }

      if (url.pathname === '/device/approve' && request.method === 'POST') {
        const form = new URLSearchParams(await request.text())
        const ok = approve(String(form.get('user_code') ?? ''), String(form.get('email') ?? 'dev@zero-x.live'), form.get('decision') === 'deny' ? 'deny' : 'approve')
        return new Response(`<!doctype html><body style="font-family:system-ui;margin:3rem"><h1>${ok ? 'Done. Go back to Neuron.' : 'That code is wrong or expired.'}</h1></body>`, {
          status: ok ? 200 : 404,
          headers: { 'Content-Type': 'text/html; charset=utf-8' },
        })
      }

      if (url.pathname.startsWith('/api/')) {
        const token = bearer(request)
        if (token?.startsWith('nzx_') && url.pathname.startsWith('/api/push')) return failure(request, 403, 'forbidden', 'API keys cannot register phones.')
        const session = sessionOf(token)
        if (!session) return failure(request, 401, 'invalid_session', 'Sign in to Zero-X again.')
        const account = accountOf(session.email)

        if (url.pathname === '/api/account' && request.method === 'GET') {
          const midnight = new Date()
          midnight.setUTCHours(24, 0, 0, 0)
          return json(request, {
            user: { id: account.id, email: account.email, name: account.email.split('@')[0], image_url: null },
            plan: { id: 'free', name: 'Free' },
            limits: { daily_requests: 200, daily_tokens: 256_000, rpm: 10, concurrency: 2 },
            usage_today: { requests: 12, tokens: 48_210, resets_at: midnight.toISOString() },
            credits: { balance_usd: 0, monthly_grant_usd: 0 },
            subscription: null,
            flags: { suspended: false },
          })
        }
        if (url.pathname === '/api/sessions' && request.method === 'GET') {
          const mine = [...sessions.entries()].filter(([, other]) => other.email === session.email)
          return json(request, {
            sessions: mine.map(([key, other]) => ({
              id: other.id,
              client: other.client,
              device_name: other.deviceName,
              os: other.os,
              created_at: new Date(other.createdAt).toISOString(),
              last_used_at: new Date(other.lastUsedAt).toISOString(),
              current: key === token,
            })),
          })
        }
        const sessionMatch = /^\/api\/sessions\/([\w-]+)$/.exec(url.pathname)
        if (sessionMatch && request.method === 'DELETE') {
          for (const [key, other] of sessions) {
            if (other.email !== session.email) continue
            if ((sessionMatch[1] === 'current' && key === token) || other.id === sessionMatch[1]) sessions.delete(key)
          }
          return json(request, { status: 'revoked' })
        }
        if (url.pathname === '/api/push/devices' && request.method === 'POST') {
          const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
          const deviceId = typeof body.device_id === 'string' && /^[A-Za-z0-9_-]{8,64}$/.test(body.device_id) ? body.device_id : null
          const pushToken = typeof body.expo_push_token === 'string' && /^Expo(nent)?PushToken\[[^\]]{1,200}\]$/.test(body.expo_push_token) ? body.expo_push_token : null
          if (!deviceId || !pushToken) return failure(request, 400, 'invalid_request', 'device_id and expo_push_token are required.')
          const devices = pushDevices.get(account.id) ?? new Map()
          pushDevices.set(account.id, devices)
          devices.set(deviceId, { deviceId, token: pushToken, platform: String(body.platform ?? 'unknown').slice(0, 16), needs_me: body.needs_me !== false, finished: body.finished !== false })
          return json(request, { status: 'registered' })
        }
        const pushMatch = /^\/api\/push\/devices\/([\w-]+)$/.exec(url.pathname)
        if (pushMatch && request.method === 'DELETE') {
          pushDevices.get(account.id)?.delete(pushMatch[1])
          return json(request, { status: 'removed' })
        }
        return failure(request, 404, 'not_found', 'No such endpoint on the dev relay.')
      }

      // Dev helpers: scripts and tests.
      if (url.pathname === '/dev/approve' && request.method === 'POST') {
        const body = (await request.json().catch(() => ({}))) as Record<string, string>
        return approve(body.user_code ?? '', body.email ?? 'dev@zero-x.live', body.decision === 'deny' ? 'deny' : 'approve')
          ? json(request, { status: 'ok' })
          : failure(request, 404, 'not_found', 'No pending code like that.')
      }
      if (url.pathname === '/dev/session' && request.method === 'POST') {
        const body = (await request.json().catch(() => ({}))) as Record<string, string>
        return json(request, { token: mintSession(body.email ?? 'dev@zero-x.live', body.client ?? 'neuron-dev-share') })
      }
      if (url.pathname === '/dev/pushes' && request.method === 'GET') return json(request, { pushes })
      if (url.pathname === '/') return new Response('Neuron dev relay: /v1/relay, /auth/device/*, /api/*', { headers: { 'Content-Type': 'text/plain' } })
      return new Response('not found', { status: 404 })
    },
    websocket: {
      maxPayloadLength: LIMITS.maxFrameBytes,
      open(socket) {
        if (socket.data.refuse) return socket.close(socket.data.refuse.code, socket.data.refuse.reason)
        socket.data.connection = hub.connect(socket.data.account!, {
          send: (text) => socket.send(text),
          close: (code, reason) => socket.close(code, reason),
        })
      },
      message(socket, message) {
        socket.data.connection?.message(typeof message === 'string' ? message : Buffer.from(message).toString('utf8'))
      },
      close(socket) {
        socket.data.connection?.closed()
      },
    },
  })

  const shown = server.hostname === '0.0.0.0' ? '127.0.0.1' : server.hostname
  return {
    hub,
    server,
    port: server.port,
    url: `http://${shown}:${server.port}`,
    relayUrl: `ws://${shown}:${server.port}/v1/relay`,
    pushes,
    mintSession,
    approve,
    accountOf,
    async stop() {
      hub.close()
      server.stop(true)
    },
  }
}

if (import.meta.main) {
  const args = process.argv.slice(2)
  const flag = (name: string) => args.includes(name)
  const value = (name: string) => {
    const index = args.indexOf(name)
    return index >= 0 ? args[index + 1] : undefined
  }
  const relay = startDevRelay({
    port: Number(value('--port') ?? 8790),
    hostname: flag('--lan') ? '0.0.0.0' : '127.0.0.1',
    expoPush: flag('--expo-push'),
    log: (line) => console.log(`[relay] ${line}`),
  })
  console.log(`Neuron dev relay on ${relay.url}`)
  console.log(`  relay:    ${relay.relayUrl}`)
  console.log(`  sign-in:  ${relay.url}/device`)
  if (flag('--lan')) console.log('  Listening on the LAN: anyone on this network can approve sign-ins here.')
}
