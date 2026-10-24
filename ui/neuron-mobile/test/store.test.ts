/**
 * The app store against the dev relay, as the screens drive it: sign in with a
 * device code, pair from a link, see the computer's session, persist across a
 * restart, forget, and sign out (which leaves no secret behind).
 */
import { afterAll, describe, expect, setDefaultTimeout, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { NeuronHost } from '../../neuron-client/src/node/host'
import { DemoEngine } from '../dev/demo-engine'
import { neuronHostSource } from '../dev/host-source'
import { startDevRelay } from '../dev/relay'
import { randomId } from '../src/relay/crypto'
import { RelayLink } from '../src/relay/link'
import { ShareHost, newHostIdentity } from '../src/relay/share-host'
import { AppStore, NeedsSignIn, type Platform } from '../src/store/app'
import { groupSessions } from '../src/store/sessions'
import { KEYS, memoryStore } from '../src/store/storage'

setDefaultTimeout(20_000)

async function waitFor(label: string, ok: () => boolean, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!ok()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(10)
  }
}

describe('AppStore', () => {
  const relay = startDevRelay({ port: 0 })
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-store-test-'))
  const engine = new DemoEngine({ cwd: '/work/store-project', pace: 0 })
  const hostToken = randomId()
  const host = new NeuronHost({ transport: engine, token: hostToken, root, isLocal: () => false })
  const identity = newHostIdentity('Store laptop')
  const computerToken = relay.mintSession('dev@zero-x.live')
  const hostLink = new RelayLink({ url: relay.relayUrl, role: 'host', device: { id: identity.hostId, name: 'Store laptop', platform: 'test' }, token: () => computerToken })
  const share = new ShareHost({ link: hostLink, identity, phones: new Map() })
  const storage = memoryStore()
  const platform: Platform = {
    storage,
    os: 'ios',
    deviceName: 'Store phone',
    appVersion: '0.2.0',
    devBuild: true,
    pushToken: async () => ({ token: 'ExponentPushToken[store-phone]' }),
  }
  const stores: AppStore[] = []
  const makeStore = () => {
    const store = new AppStore(platform)
    stores.push(store)
    return store
  }

  afterAll(async () => {
    for (const store of stores) store.stop()
    share.stop()
    hostLink.stop()
    await host.stop()
    await relay.stop()
    fs.rmSync(root, { recursive: true, force: true })
  })

  test('signs in, pairs, lists, remembers, forgets and signs out', async () => {
    await host.start()
    share.share(neuronHostSource(host, { token: hostToken, shareId: randomId(), root: '/work/store-project' }))
    hostLink.start()
    await waitFor('the computer', () => hostLink.ready)

    const store = makeStore()
    await store.load()
    expect(store.getState()).toMatchObject({ loaded: true, signedIn: false, computers: [] })
    const { link } = await share.offerPairing(relay.relayUrl)

    // Pairing first asks for a sign-in, on the gateway that goes with the relay.
    const needs = await store.pair(link).catch((error) => error)
    expect(needs).toBeInstanceOf(NeedsSignIn)
    expect((needs as NeedsSignIn).gateway).toBe(relay.url)

    const code = await store.startSignIn(relay.url)
    relay.approve(code.user_code, 'dev@zero-x.live')
    await store.completeSignIn(relay.url, { ...code, interval: 0 })
    expect(store.getState()).toMatchObject({ signedIn: true, gateway: relay.url, account: { user: { email: 'dev@zero-x.live' } } })
    // Push registers in the background, with both switches on.
    await waitFor('push registration', () => store.getState().push.status === 'registered')

    const computer = await store.pair(link)
    expect(computer.name).toBe('Store laptop')
    await waitFor('the shared session', () => groupSessions(store.getState().hosts, store.getState().viewed).length === 1)
    expect(groupSessions(store.getState().hosts, {})[0]).toMatchObject({ project: 'store-project', rows: [{ computer: 'Store laptop', badge: 'unread' }] })

    // The secrets are in the store's storage, not in its state.
    expect(JSON.stringify(store.getState())).not.toContain(JSON.parse((await storage.get(KEYS.computer(computer.hostId)))!).pairKey)

    // A restart (a new store on the same storage) comes back paired and connected.
    store.stop()
    const again = makeStore()
    await again.load()
    expect(again.getState().computers.map((info) => info.name)).toEqual(['Store laptop'])
    await waitFor('the session after a restart', () => again.getState().hosts.some((item) => item.online && item.shares.length === 1))

    await again.forget(computer.hostId)
    expect(again.getState().computers).toEqual([])
    await waitFor('the computer to forget the phone', () => share.phones.size === 0)

    await again.signOut()
    expect(again.getState()).toMatchObject({ signedIn: false, computers: [], hosts: [] })
    for (const key of [KEYS.session, KEYS.computers, KEYS.computer(computer.hostId)]) expect(await storage.get(key)).toBeNull()
  })

  test('refuses a pairing link for a relay it does not trust in a store build', async () => {
    const store = new AppStore({ ...platform, storage: memoryStore(), devBuild: false })
    stores.push(store)
    await store.load()
    const { link } = await share.offerPairing(relay.relayUrl)
    expect(await store.pair(link).catch((error: Error) => error.message)).toBe("This app only pairs through Zero-X's relay, not 127.0.0.1.")
  })
})
