/**
 * The Rust `neuron engine --share` against this package's TypeScript ends: the dev relay, a
 * `NeuronHost` serving the demo engine, and the phone. It pairs from the link the Rust process
 * prints, steers the session over an encrypted channel (a prompt, a denial with a note), checks
 * what the bridge refuses (terminals, trusting the workspace) although the host would take them
 * from a local client, gets a push from Rust's `notify`, and checks no plaintext crossed the relay.
 *
 * Needs a built binary: NEURON_BIN=<path to neuron> bun test test/share-rust.integration.test.ts
 * (cargo build -p rusty-claude-cli in rust/). Skipped without it.
 */
import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { SessionController, commandMessage } from '@neuron/client'
import { NeuronHost } from '../../neuron-client/src/node/host'
import { DemoEngine } from '../dev/demo-engine'
import { startDevRelay } from '../dev/relay'
import { Gateway } from '../src/account/gateway'
import { randomId } from '../src/relay/crypto'
import { RelayLink } from '../src/relay/link'
import { parsePairingLink } from '../src/relay/pairing'
import { PhoneRelay, type PairedComputer } from '../src/relay/phone'
import { RelayEngineTransport } from '../src/relay/transport'

const bin = process.env.NEURON_BIN && fs.existsSync(process.env.NEURON_BIN) ? process.env.NEURON_BIN : null

async function waitFor(label: string, ok: () => boolean, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!ok()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(10)
  }
}

setDefaultTimeout(30_000)

describe.skipIf(!bin)('the Rust share host with the TypeScript relay, host and phone', () => {
  const frames: string[] = []
  const relay = startDevRelay({ port: 0, needsMeDelayMs: 150, tap: (_direction, _role, text) => frames.push(text) })
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-share-rust-'))
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-share-root-'))
  const engine = new DemoEngine({ cwd: '/work/neuron-demo-project', pace: 0 })
  const hostToken = randomId()
  // The host's own default: a client from this machine counts as local. The bridge connects from
  // loopback, so what keeps terminals and trust away from the phone is the bridge itself.
  const host = new NeuronHost({ transport: engine, token: hostToken, root, name: 'Rust laptop' })
  const tokenFile = path.join(home, 'host.token')
  const phoneId = randomId()
  const phoneToken = relay.mintSession('dev@zero-x.live')
  const computers = new Map<string, PairedComputer>()
  const phoneLink = new RelayLink({ url: relay.relayUrl, role: 'phone', device: { id: phoneId, name: 'Test phone', platform: 'android' }, token: () => phoneToken, backoff: { baseMs: 40, maxMs: 200 } })
  const phone = new PhoneRelay(phoneLink, (id) => computers.get(id))
  let share: ReturnType<typeof Bun.spawn> | null = null
  let stdout = ''
  let hostId = ''
  let shareId = ''
  let controller: SessionController

  beforeAll(async () => {
    await host.start()
    fs.writeFileSync(tokenFile, hostToken)
    share = Bun.spawn([bin!, 'engine', '--share', '--host', host.url, '--host-token-file', tokenFile, '--relay', relay.relayUrl, '--name', 'Rust laptop'], {
      env: { ...process.env, NEURON_TOKEN: relay.mintSession('dev@zero-x.live'), NEURON_HOME: home },
      stdin: 'pipe',
      stdout: 'pipe',
      stderr: 'inherit',
    })
    void (async () => {
      const reader = (share!.stdout as ReadableStream<Uint8Array>).getReader()
      const decoder = new TextDecoder()
      for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) stdout += decoder.decode(chunk.value)
    })()
    await new Gateway({ base: relay.url }).registerPush(phoneToken, { deviceId: phoneId, expoPushToken: 'ExponentPushToken[rust-share]', platform: 'android', needsMe: true, finished: false })
    phoneLink.start()
    await waitFor('the phone to join', () => phoneLink.ready)
  })

  afterAll(async () => {
    await controller?.close().catch(() => {})
    share?.kill()
    phoneLink.stop()
    await host.stop()
    await relay.stop()
    for (const dir of [home, root]) fs.rmSync(dir, { recursive: true, force: true })
  })

  test('pairs from the link the Rust process prints, and lists its session', async () => {
    await waitFor('the pairing link', () => /neuron:\/\/pair\?\S+/.test(stdout))
    const link = /neuron:\/\/pair\?\S+/.exec(stdout)![0]
    const payload = parsePairingLink(link, { trustedRelayHosts: null })
    expect(payload.accountTag).toBe(phoneLink.accountTag!)
    expect(payload.name).toBe('Rust laptop')
    const computer = await phone.pair(payload, 'Test phone')
    computers.set(computer.hostId, computer)
    hostId = computer.hostId
    await waitFor('the paired message', () => stdout.includes('Paired with Test phone'))
    phone.refresh()
    await waitFor('the shared session', () => !!phone.hosts.get(hostId)?.shares[0]?.presence)
    const listed = phone.hosts.get(hostId)!
    shareId = listed.shares[0].shareId
    expect(listed.online).toBe(true)
    expect(listed.shares[0].presence).toMatchObject({ v: 1, project: 'neuron-demo-project', status: 'idle', branch: 'main', model: 'demo-model' })
    const saved = JSON.parse(fs.readFileSync(path.join(home, 'remote.json'), 'utf8'))
    expect(saved.phones.map((p: { phone_id: string }) => p.phone_id)).toEqual([phoneId])
  })

  test('steers the session over the Rust bridge: a prompt, a diff to approve, a denial with a note', async () => {
    const transport = new RelayEngineTransport({ relay: phone, hostId, shareId, reattachBaseMs: 40 })
    controller = new SessionController(transport)
    await controller.start()
    expect(transport.host?.name).toBe('Rust laptop')
    controller.send('fix the sum please')
    await waitFor('the approval', () => controller.getState().permissions.length === 1)
    await waitFor('presence to say it needs the user', () => phone.hosts.get(hostId)?.shares[0]?.presence?.status === 'needs_me')
    const pending = controller.getState().permissions[0]
    controller.answerPermission(pending.id, { kind: 'deny', note: 'use reduce with a zero' })
    await waitFor('the denial to reach the engine', () => engine.sent.some((message) => message.kind === 'permission_reply'))
    expect(engine.sent.find((message) => message.kind === 'permission_reply')).toEqual({ kind: 'permission_reply', id: pending.id, decision: { kind: 'deny', note: 'use reduce with a zero' } })
    await waitFor('the turn to end', () => !controller.getState().busy && controller.getState().items.some((item) => item.kind === 'assistant' && item.text.includes('use reduce with a zero')))
    await waitFor('presence to say it is idle', () => phone.hosts.get(hostId)?.shares[0]?.presence?.status === 'idle')
    expect(relay.pushes).toEqual([])

    // What only the computer may do never reaches the host, although it would take it from loopback.
    const refused = await transport.rpc('term.open', { cols: 80, rows: 24 }).catch((error: Error) => error.message)
    expect(refused).toBe('Not available from the phone.')
    expect(await transport.rpc<{ path: string }>('fs.list', { path: '.' }).then((listing) => listing.path)).toBe('.')
    transport.send(commandMessage({ type: 'trust_workspace' }))
    await Bun.sleep(300)
    expect(engine.sent.some((message) => message.kind === 'command' && message.type === 'trust_workspace')).toBe(false)
  })

  test('pushes "needs me" from Rust\'s notify when no phone is attached', async () => {
    await controller.close()
    engine.emit({
      type: 'permission_requested',
      id: 99,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input: '{"command":"rm -rf build"}', session_prefixes: [] },
    })
    await waitFor('the push', () => relay.pushes.length === 1, 5_000)
    expect(relay.pushes[0]).toMatchObject({ deviceId: phoneId, kind: 'needs_me', body: 'A session on Rust laptop is waiting for your answer.', data: { neuron: { host_id: hostId, share_id: shareId, kind: 'needs_me' } } })
  })

  test('the relay never carried plaintext', () => {
    const all = frames.join('\n')
    expect(frames.length).toBeGreaterThan(20)
    for (const words of ['fix the sum please', 'use reduce with a zero', 'neuron-demo-project', 'rm -rf build', hostToken]) expect(all).not.toContain(words)
  })
})
