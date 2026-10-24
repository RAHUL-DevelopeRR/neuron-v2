// A local stand-in for the Zero-X gateway's sign-in and catalog routes, the
// same ones rust/crates/rusty-claude-cli/tests/engine_stdio.rs fakes: the
// device code, its approval at the first poll, the session, the model list and
// sign-out. Point the engine at it with NEURON_API_BASE=<base>. Everything it
// hands out is made up; it never talks to the real gateway.

export const FAKE_USER_CODE = 'BCDF-GHJK'
export const FAKE_EMAIL = 'dev@example.com'
// The engine checks these formats; the values are the Rust test's made-up ones.
const FAKE_DEVICE_CODE = 'zdc_0123456789abcdefghijABCDEFGHIJ_-01234567890'
const FAKE_SESSION = 'ses_abcdefghijklmnopqrstuvwxyz012345'

export const FAKE_MODELS = [
  {
    id: 'zx-fast',
    display_name: 'Fast',
    context_window: 131_072,
    capabilities: { tools: true },
    tiers: ['free'],
    available: true,
  },
  {
    id: 'zx-big',
    display_name: 'Big',
    context_window: 262_144,
    tiers: ['pro'],
    available: false,
    unavailable_reason: 'Pro plan only',
  },
]

export interface FakeGateway {
  /** What NEURON_API_BASE points at. */
  base: string
  /** `METHOD /path`, with ` (signed)` when it carried the session. */
  requests: string[]
  stop(): void
}

export function startFakeGateway(): FakeGateway {
  const requests: string[] = []
  const server = Bun.serve({
    hostname: '127.0.0.1',
    port: 0,
    async fetch(request) {
      const url = new URL(request.url)
      const signed = request.headers.get('authorization') === `Bearer ${FAKE_SESSION}`
      requests.push(`${request.method} ${url.pathname}${signed ? ' (signed)' : ''}`)
      const page = `${url.origin}/device`
      const route = `${request.method} ${url.pathname}`
      if (route === 'POST /auth/device/code') {
        return Response.json({
          device_code: FAKE_DEVICE_CODE,
          user_code: FAKE_USER_CODE,
          verification_uri: page,
          verification_uri_complete: `${page}?code=${FAKE_USER_CODE}`,
          expires_in: 600,
          interval: 5,
        })
      }
      // Approved at the first poll.
      if (route === 'POST /auth/device/token') {
        return Response.json({ access_token: FAKE_SESSION, email: FAKE_EMAIL, plan: 'pro' })
      }
      if (route === 'GET /auth/session' && signed) {
        return Response.json({ plan: 'pro', email: FAKE_EMAIL, ttl_remaining_seconds: 86_400 })
      }
      if (route === 'GET /v1/models' && signed) {
        return Response.json({ data: FAKE_MODELS })
      }
      if (route === 'DELETE /auth/session' && signed) {
        return new Response(null, { status: 204 })
      }
      return Response.json({ error: { code: 'not_found', message: 'not found' } }, { status: 404 })
    },
  })
  return {
    base: `http://127.0.0.1:${server.port}/v1`,
    requests,
    stop: () => server.stop(true),
  }
}
