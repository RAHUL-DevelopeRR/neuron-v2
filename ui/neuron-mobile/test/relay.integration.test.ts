/**
 * The phone and a computer through the dev relay, end to end: device sign-in,
 * pairing by link, a session over an encrypted channel (a prompt, a diff to
 * approve, a denial with a note), a dropped socket that resumes, a lost replay
 * buffer that falls back to a snapshot, a push while nobody watches, and a
 * phone on another account that gets nowhere. The relay is tapped throughout
 * to check it never carries plaintext.
 */
import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { SessionController } from '@neuron/client'
import { NeuronHost } from '../../neuron-client/src/node/host'
import { DemoEngine } from '../dev/demo-engine'
import { neuronHostSource } from '../dev/host-source'
import { startDevRelay } from '../dev/relay'
import { Gateway } from '../src/account/gateway'
import { b64u, randomBytes, randomId } from '../src/relay/crypto'
import { RelayLink } from '../src/relay/link'
import { parsePairingLink } from '../src/relay/pairing'
import { ChannelClose, PairingFailed, PhoneRelay, type PairedComputer } from '../src/relay/phone'
import { ShareHost, newHostIdentity } from '../src/relay/share-host'
import { RelayEngineTransport } from '../src/relay/transport'

async function waitFor(label: string, ok: () => boolean, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!ok()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(10)
  }
}

setDefaultTimeout(20_000)

const SECRET_WORDS = ['fix the sum please', 'use reduce with a zero', 'neuron-demo-project', 'while you were away', 'after the buffer was lost']

describe('the phone through the relay', () => {
  const frames: string[] = []
  const relay = startDevRelay({ port: 0, needsMeDelayMs: 150, channelAwayMs: 5_000, tap: (_direction, _role, text) => frames.push(text) })
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-relay-test-'))
  const cwd = '/work/neuron-demo-project'
  const engine = new DemoEngine({ cwd, pace: 0 })
  const hostToken = randomId()
  const host = new NeuronHost({ transport: engine, token: hostToken, root, name: 'Dev laptop', isLocal: () => false })
  const identity = newHostIdentity('Dev laptop')
  const computerToken = relay.mintSession('dev@zero-x.live')
  const hostLink = new RelayLink({ url: relay.relayUrl, role: 'host', device: { id: identity.hostId, name: 'Dev laptop', platform: 'test' }, token: () => computerToken })
  const share = new ShareHost({ link: hostLink, identity, phones: new Map() })
  const shareId = randomId()

  const phoneId = randomId()
  const computers = new Map<string, PairedComputer>()
  let phoneToken = ''
  const phoneLink = new RelayLink({
    url: relay.relayUrl,
    role: 'phone',
    device: { id: phoneId, name: 'Test phone', platform: 'ios' },
    token: () => phoneToken,
    backoff: { baseMs: 40, maxMs: 200 },
  })
  const phone = new PhoneRelay(phoneLink, (id) => computers.get(id))
  let transport: RelayEngineTransport
  let controller: SessionController
  const others: RelayLink[] = []

  beforeAll(async () => {
    await host.start()
    share.share(neuronHostSource(host, { token: hostToken, shareId, root: cwd }))
    hostLink.start()
    await waitFor('the computer to join', () => hostLink.ready)
  })

  afterAll(async () => {
    await controller?.close().catch(() => {})
    share.stop()
    for (const link of [hostLink, phoneLink, ...others]) link.stop()
    await host.stop()
    await relay.stop()
    fs.rmSync(root, { recursive: true, force: true })
  })

  test('signs in with a device code approved in the browser', async () => {
    const gateway = new Gateway({ base: relay.url, client: { id: 'neuron-mobile', version: '0.2.0', deviceName: 'Test phone', os: 'ios' } })
    const code = await gateway.requestDeviceCode()
    expect(code.verification_uri_complete).toContain(`/device?user_code=${code.user_code}`)
    let polls = 0
    const approval = gateway.waitForApproval(code, {
      sleep: async () => {
        polls++
        await Bun.sleep(5)
        // The user approves after the app has polled a couple of times.
        if (polls === 3) await fetch(`${relay.url}/dev/approve`, { method: 'POST', body: JSON.stringify({ user_code: code.user_code, email: 'dev@zero-x.live' }) })
      },
    })
    const signedIn = await approval
    expect(signedIn.token.startsWith('ses_')).toBe(true)
    phoneToken = signedIn.token
    const account = await gateway.account(phoneToken)
    expect(account.user.email).toBe('dev@zero-x.live')
    expect(account.plan.name).toBe('Free')
    expect(account.usage_today.tokens).toBeGreaterThan(0)
    await gateway.registerPush(phoneToken, { deviceId: phoneId, expoPushToken: 'ExponentPushToken[test-phone]', platform: 'ios', needsMe: true, finished: false })
  })

  test('pairs by QR link, and lists the computer with its sealed presence opened', async () => {
    phoneLink.start()
    await waitFor('the phone to join', () => phoneLink.ready)
    const offer = await share.offerPairing(relay.relayUrl, 120)
    const payload = parsePairingLink(offer.link, { trustedRelayHosts: null })
    expect(payload.accountTag).toBe(phoneLink.accountTag!)
    const computer = await phone.pair(payload, 'Test phone')
    computers.set(computer.hostId, computer)
    expect(computer).toMatchObject({ hostId: identity.hostId, name: 'Dev laptop', relayUrl: relay.relayUrl })
    expect(share.phones.get(phoneId)?.name).toBe('Test phone')

    // The code worked once. (Bun's `expect(promise).rejects` stalls socket delivery while it waits, so these catch.)
    expect(await phone.pair(payload, 'Test phone').catch((error: PairingFailed) => error.code)).toBe('pair_not_found')

    phone.refresh()
    await waitFor('the shared session', () => !!phone.hosts.get(identity.hostId)?.shares.find((s) => s.shareId === shareId)?.presence)
    const listed = phone.hosts.get(identity.hostId)!
    expect(listed.online).toBe(true)
    expect(listed.shares[0].presence).toMatchObject({ v: 1, project: 'neuron-demo-project', status: 'idle', branch: 'main', model: 'demo-model' })
  })

  test('steers the session: a prompt, a diff to approve, a denial with a note', async () => {
    transport = new RelayEngineTransport({ relay: phone, hostId: identity.hostId, shareId, reattachBaseMs: 40 })
    controller = new SessionController(transport)
    await controller.start()
    expect(controller.getState().status?.cwd).toBe(cwd)
    expect(transport.host?.name).toBe('Dev laptop')
    expect(transport.snapshots).toBe(1)

    controller.send('fix the sum please')
    await waitFor('the approval', () => controller.getState().permissions.length === 1)
    const pending = controller.getState().permissions[0]
    const tool = controller.getState().items.find((item) => item.kind === 'tool' && item.key === pending.toolKey)
    expect(tool?.kind === 'tool' && tool.view.diff).toMatchObject({ added: 1, removed: 1 })
    expect(controller.getState().items.some((item) => item.kind === 'assistant' && item.text.includes('```ts'))).toBe(true)
    await waitFor('presence to say it needs the user', () => phone.hosts.get(identity.hostId)?.shares[0]?.presence?.status === 'needs_me')

    controller.answerPermission(pending.id, { kind: 'deny', note: 'use reduce with a zero' })
    await waitFor('the denial to reach the engine', () => engine.sent.some((message) => message.kind === 'permission_reply'))
    expect(engine.sent.find((message) => message.kind === 'permission_reply')).toEqual({ kind: 'permission_reply', id: pending.id, decision: { kind: 'deny', note: 'use reduce with a zero' } })
    await waitFor('the reply', () => controller.getState().items.some((item) => item.kind === 'assistant' && item.text.includes('use reduce with a zero')))
    await waitFor('the turn to end', () => !controller.getState().busy)
    // Someone was watching: no push.
    await Bun.sleep(250)
    expect(relay.pushes).toEqual([])
  })

  test('a dropped socket resumes from the last frame: nothing is lost, no snapshot needed', async () => {
    const snapshots = transport.snapshots
    phoneLink.drop()
    // While the phone is away the computer goes on, and the phone sends a prompt that waits in its outbox.
    engine.emit({ type: 'notice', message: 'while you were away' })
    controller.send('second prompt')
    await waitFor('the notice', () => controller.getState().items.some((item) => item.kind === 'notice' && item.text === 'while you were away'))
    await waitFor('the queued prompt to reach the engine', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'submit' && message.text === 'second prompt'))
    expect(transport.snapshots).toBe(snapshots)
    expect(phoneLink.state.status).toBe('open')

    await waitFor('the second approval', () => controller.getState().permissions.length === 1)
    controller.answerPermission(controller.getState().permissions[0].id, { kind: 'allow_once' })
    await waitFor('the second turn to end', () => !controller.getState().busy && controller.getState().items.some((item) => item.kind === 'assistant' && item.text.includes('Fixed.')))
  })

  test('when the relay lost its buffer, the session attaches again from a snapshot', async () => {
    const snapshots = transport.snapshots
    relay.hub.forgetReplay()
    phoneLink.drop()
    engine.emit({ type: 'notice', message: 'after the buffer was lost' })
    await waitFor('a fresh snapshot', () => transport.snapshots === snapshots + 1)
    await waitFor('the notice from the snapshot', () => controller.getState().items.some((item) => item.kind === 'notice' && item.text === 'after the buffer was lost'))
    expect(transport.attachState).toBe('attached')
    expect(controller.getState().connection).toBe('ready')
  })

  test('pushes "needs me" when an approval waits and no phone is attached', async () => {
    await controller.close()
    await waitFor('the channel to close', () => share.attached === 0)
    engine.emit({
      type: 'permission_requested',
      id: 99,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input: '{"command":"rm -rf build"}', session_prefixes: [] },
    })
    await waitFor('the push', () => relay.pushes.length === 1, 3_000)
    expect(relay.pushes[0]).toMatchObject({
      to: 'ExponentPushToken[test-phone]',
      deviceId: phoneId,
      kind: 'needs_me',
      title: 'Neuron needs you',
      body: 'A session on Dev laptop is waiting for your answer.',
      data: { neuron: { host_id: identity.hostId, share_id: shareId, kind: 'needs_me' } },
    })
    // The push says nothing about what the session is doing.
    expect(JSON.stringify(relay.pushes[0])).not.toContain('rm -rf')
  })

  test('a phone on another account cannot pair, attach or inject frames', async () => {
    const otherToken = relay.mintSession('other@zero-x.live')
    const otherId = randomId()
    const otherLink = new RelayLink({ url: relay.relayUrl, role: 'phone', device: { id: otherId, name: 'Other phone', platform: 'android' }, token: () => otherToken })
    others.push(otherLink)
    const otherComputers = new Map<string, PairedComputer>()
    const other = new PhoneRelay(otherLink, (id) => otherComputers.get(id))
    const relayErrors: string[] = []
    otherLink.onFrame((frame) => {
      if (frame.t === 'error') relayErrors.push(String(frame.code))
    })
    otherLink.start()
    await waitFor('the other phone to join', () => otherLink.ready)
    expect(otherLink.accountTag).not.toBe(phoneLink.accountTag)

    const offer = await share.offerPairing(relay.relayUrl, 120)
    const payload = parsePairingLink(offer.link, { trustedRelayHosts: null })
    // The app sees the account tag and says so before asking the relay...
    expect(await other.pair(payload, 'Other phone').catch((error: PairingFailed) => error.code)).toBe('other_account')
    // ...and without the tag, the relay itself refuses: the code is not in this account's room.
    const refused = await other.pair({ ...payload, accountTag: null }, 'Other phone').catch((error: PairingFailed) => error)
    expect(refused).toBeInstanceOf(PairingFailed)
    expect((refused as PairingFailed).code).toBe('pair_not_found')
    expect(share.phones.has(otherId)).toBe(false)

    // Attaching with made-up keys to the first account's computer is refused by the relay.
    otherComputers.set(identity.hostId, { ...computers.get(identity.hostId)!, pairKey: b64u(randomBytes(32)) })
    const channel = other.openChannel(identity.hostId, shareId)
    const closed = new Promise<{ code: number; reason: string }>((resolve) => (channel.onclose = resolve))
    expect(await closed).toMatchObject({ code: ChannelClose.refused })

    // A data frame aimed at a channel of the other account goes nowhere.
    const before = engine.sent.length
    otherLink.send({ t: 'data', ch: randomId(), n: 0, c: 'AAAAAAAAAAAAAAAAAAAAAA' })
    otherLink.send({ t: 'unpair', host_id: identity.hostId })
    await waitFor('the relay errors', () => relayErrors.includes('not_paired') && relayErrors.includes('unknown_channel'))
    expect(engine.sent.length).toBe(before)
    // The first phone is still paired.
    expect(share.phones.has(phoneId)).toBe(true)
  })

  test('the relay never carried plaintext', () => {
    expect(frames.length).toBeGreaterThan(20)
    const all = frames.join('\n')
    for (const words of SECRET_WORDS) expect(all).not.toContain(words)
    // Nor the session token or the pairing code.
    expect(all).not.toContain(phoneToken)
  })
})
