/**
 * The gateway calls against the shapes zero-x.live answers with (neuroncli/auth-server,
 * docs/PLATFORM_CONTRACT.md §10 and docs/RELAY.md §5): linked devices, sign-out and push.
 */
import { expect, test } from 'bun:test'

import { Gateway, GatewayError } from '../src/account/gateway'

function fakeGateway(answers: Record<string, { status: number; body: unknown }>) {
  const calls: Array<{ url: string; method: string; body: unknown }> = []
  const gateway = new Gateway({
    base: 'https://zero-x.live',
    fetch: async (url, init) => {
      calls.push({ url, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(init.body) : undefined })
      const answer = answers[`${init?.method ?? 'GET'} ${new URL(url).pathname}`] ?? { status: 404, body: { error: { code: 'not_found', message: 'no' } } }
      return { ok: answer.status < 400, status: answer.status, json: async () => answer.body }
    },
  })
  return { gateway, calls }
}

test('linked devices read Zero-X client objects and dev relay strings', async () => {
  const session = { id: 'a'.repeat(24), device_name: null, os: 'ios', created_at: '2026-10-26T00:00:00.000Z', last_used_at: null, current: false }
  const { gateway } = fakeGateway({
    'GET /api/sessions': { status: 200, body: { sessions: [{ ...session, client: { name: 'neuron-cli', version: '7.0.0' } }, { ...session, client: 'neuron-mobile' }, { ...session, client: null }] } },
  })
  expect((await gateway.linkedSessions('ses_x'))?.map((linked) => linked.client)).toEqual(['neuron-cli', 'neuron-mobile', null])
  expect(await fakeGateway({}).gateway.linkedSessions('ses_x')).toBeNull()
})

test('sign-out and push use the contract endpoints', async () => {
  const { gateway, calls } = fakeGateway({
    'DELETE /api/sessions/current': { status: 200, body: { status: 'revoked', current: true } },
    'POST /api/push/devices': { status: 200, body: { status: 'registered' } },
    'DELETE /api/push/devices/phone-0001': { status: 200, body: { status: 'removed' } },
  })
  await gateway.signOut('ses_x')
  await gateway.registerPush('ses_x', { deviceId: 'phone-0001', expoPushToken: 'ExponentPushToken[abcdefghijkl]', platform: 'android', needsMe: true, finished: false })
  await gateway.unregisterPush('ses_x', 'phone-0001')
  expect(calls.map((call) => `${call.method} ${new URL(call.url).pathname}`)).toEqual(['DELETE /api/sessions/current', 'POST /api/push/devices', 'DELETE /api/push/devices/phone-0001'])
  expect(calls[1].body).toEqual({ device_id: 'phone-0001', expo_push_token: 'ExponentPushToken[abcdefghijkl]', platform: 'android', needs_me: true, finished: false })
  await expect(fakeGateway({}).gateway.unregisterPush('ses_x', 'phone-0001')).rejects.toBeInstanceOf(GatewayError)
})
