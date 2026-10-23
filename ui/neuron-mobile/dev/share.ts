/**
 * A development "computer": it runs an engine behind a local `NeuronHost`,
 * shares that session through the relay, and prints a pairing QR code for the
 * phone. It stands in for `neuron engine --share` (WP1) until that lands.
 *
 *   bun run dev/share.ts [--relay ws://127.0.0.1:8790/v1/relay] [--email dev@zero-x.live]
 *                        [--name "Dev laptop"] [--cwd <project>] [--state <dir>] [--neuron]
 *
 * With `--neuron` it runs your own `neuron engine --stdio` in `--cwd` instead
 * of the scripted demo engine. The computer's id, keys and paired phones are
 * kept in `--state` (default: ~/.neuron-dev-share), so a phone stays paired
 * across restarts. The session token comes from the dev relay's `/dev/session`
 * and is never printed.
 */

import fs from 'fs'
import os from 'os'
import path from 'path'
import QRCode from 'qrcode'

import type { EngineTransport } from '@neuron/client'
import { NeuronHost } from '../../neuron-client/src/node/host'
import { randomId } from '../src/relay/crypto'
import { RelayLink } from '../src/relay/link'
import { buildPairingWebLink } from '../src/relay/pairing'
import { ShareHost, newHostIdentity, type HostIdentity, type PairedPhone } from '../src/relay/share-host'
import { DemoEngine } from './demo-engine'
import { neuronHostSource } from './host-source'

const args = process.argv.slice(2)
const value = (name: string, fallback: string) => {
  const index = args.indexOf(name)
  return index >= 0 && args[index + 1] ? args[index + 1] : fallback
}
const relayUrl = value('--relay', 'ws://127.0.0.1:8790/v1/relay')
const email = value('--email', 'dev@zero-x.live')
const cwd = path.resolve(value('--cwd', process.cwd()))
const stateDir = path.resolve(value('--state', path.join(os.homedir(), '.neuron-dev-share')))
const name = value('--name', `${os.hostname()} (dev)`)

interface SavedState {
  identity: HostIdentity
  phones: PairedPhone[]
  /** Kept so the phone sees the same session (and its read state) after a restart. */
  shareId: string
}

function loadState(): SavedState {
  fs.mkdirSync(stateDir, { recursive: true, mode: 0o700 })
  const file = path.join(stateDir, 'share.json')
  try {
    const saved = JSON.parse(fs.readFileSync(file, 'utf8')) as SavedState
    if (saved.identity?.hostId) return { ...saved, identity: { ...saved.identity, name }, shareId: saved.shareId ?? randomId() }
  } catch {
    // First run.
  }
  return { identity: newHostIdentity(name), phones: [], shareId: randomId() }
}

function saveState(state: SavedState): void {
  fs.writeFileSync(path.join(stateDir, 'share.json'), JSON.stringify(state, null, 2), { mode: 0o600 })
}

async function main() {
  const saved = loadState()
  saveState(saved)
  const gateway = relayUrl.replace(/^ws/, 'http').replace(/\/v1\/relay$/, '')
  const response = await fetch(`${gateway}/dev/session`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email }) })
  if (!response.ok) throw new Error(`The dev relay at ${gateway} refused a session (${response.status}). Is it running?`)
  const { token } = (await response.json()) as { token: string }

  let engine: EngineTransport
  if (args.includes('--neuron')) {
    const { StdioTransport } = await import('../../neuron-client/src/node/stdio-transport')
    engine = new StdioTransport({ binary: process.env.NEURON_BIN ?? 'neuron', cwd, env: { ...process.env } })
  } else {
    engine = new DemoEngine({ cwd })
  }
  const hostToken = randomId()
  // The bridge is the only client here; phones count as remote (no terminals, no restarts).
  const host = new NeuronHost({ transport: engine, token: hostToken, root: cwd, name, isLocal: () => false })
  await host.start()

  const link = new RelayLink({ url: relayUrl, role: 'host', device: { id: saved.identity.hostId, name, platform: process.platform }, token: () => token })
  const phones = new Map(saved.phones.map((phone) => [phone.phoneId, phone]))
  const share = new ShareHost({
    link,
    identity: saved.identity,
    phones,
    savePhones: (all) => saveState({ ...saved, phones: [...all.values()] }),
  })
  share.onPaired((phone) => console.log(`Paired with ${phone.name}.`))
  share.share(neuronHostSource(host, { token: hostToken, shareId: saved.shareId, root: cwd }))

  let shown = false
  link.onStatus((state) => console.log(`[relay] ${state.status}${state.delayMs ? ` (retry in ${state.delayMs} ms)` : ''}`))
  link.onReady(async () => {
    if (shown) return
    shown = true
    const offer = await share.offerPairing(relayUrl)
    console.log(`\nSharing ${path.basename(cwd)} as "${name}". Scan this with Neuron on your phone (Computers → Pair):\n`)
    console.log(await QRCode.toString(offer.link, { type: 'terminal', small: true }))
    console.log(`Or paste this pairing link into the app (the web build has no camera):\n${offer.link}\n`)
    console.log(`https form: ${buildPairingWebLink(offer.payload)}`)
    console.log(`The code works once and expires at ${new Date(offer.expiresAt).toLocaleTimeString()}. Paired phones: ${phones.size}.`)
  })
  link.start()

  process.on('SIGINT', async () => {
    share.stop()
    link.stop()
    await host.stop()
    process.exit(0)
  })
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
