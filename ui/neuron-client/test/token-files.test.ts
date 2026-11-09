import { afterEach, describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import type { EngineTransport, TransportClose } from '../src/controller'
import { NeuronHost } from '../src/node/host'
import { removeHostTokens, writeHostTokens } from '../src/node/token-files'
import type { HostFrame } from '../src/remote'
import type { SessionAction } from '../src/session'

/** An engine that says nothing: the roles are settled at hello, before any engine event. */
class QuietEngine implements EngineTransport {
  readonly echoesActions = false
  async start() {}
  send() {}
  subscribe(_listener: (action: SessionAction) => void) {
    return () => {}
  }
  onClose(_listener: (close: TransportClose) => void) {
    return () => {}
  }
  async close() {}
}

let dir = ''
const fresh = () => (dir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-token-files-')))

afterEach(() => {
  fs.rmSync(dir, { recursive: true, force: true })
})

describe('the host token files', () => {
  test('an owner token is always written, a peer token only when a file is named for it', () => {
    const base = fresh()
    const alone = writeHostTokens({ tokenFile: path.join(base, 'owner.token') })
    expect(alone.peerToken).toBeUndefined()
    expect(alone.peerTokenFile).toBeUndefined()
    expect(fs.readdirSync(base)).toEqual(['owner.token'])
    expect(fs.readFileSync(alone.tokenFile, 'utf8')).toBe(alone.token)
    expect(alone.token).toMatch(/^[A-Za-z0-9_-]{32}$/)

    const both = writeHostTokens({ tokenFile: path.join(base, 'owner.token'), peerTokenFile: path.join(base, 'peer.token') })
    expect(fs.readFileSync(both.tokenFile, 'utf8')).toBe(both.token)
    expect(fs.readFileSync(both.peerTokenFile!, 'utf8')).toBe(both.peerToken!)
    expect(both.peerToken).toMatch(/^[A-Za-z0-9_-]{32}$/)
    expect(both.peerToken).not.toBe(both.token)
    // Every run makes new ones.
    expect(both.token).not.toBe(alone.token)
  })

  test('one file for both roles is refused before anything is written, however the path is spelled', () => {
    const base = fresh()
    const file = path.join(base, 'owner.token')
    expect(() => writeHostTokens({ tokenFile: file, peerTokenFile: file })).toThrow('different file')
    expect(() => writeHostTokens({ tokenFile: file, peerTokenFile: path.join(base, 'sub', '..', 'owner.token') })).toThrow('different file')
    expect(fs.readdirSync(base)).toEqual([])
  })

  test('removing deletes both files, and the peer file when there is none is no error', () => {
    const base = fresh()
    const both = writeHostTokens({ tokenFile: path.join(base, 'owner.token'), peerTokenFile: path.join(base, 'peer.token') })
    removeHostTokens(both)
    expect(fs.readdirSync(base)).toEqual([])
    const alone = writeHostTokens({ tokenFile: path.join(base, 'owner.token') })
    removeHostTokens(alone)
    expect(fs.readdirSync(base)).toEqual([])
  })

  // What `bun run host --peer-token-file` hands out: the owner's file makes the owner, the peer file a peer, on a real socket.
  test('a host started from the two files treats the owner file as the owner and the peer file as a peer', async () => {
    const base = fresh()
    const tokens = writeHostTokens({ tokenFile: path.join(base, 'owner.token'), peerTokenFile: path.join(base, 'peer.token') })
    const host = new NeuronHost({ transport: new QuietEngine(), token: tokens.token, peerToken: tokens.peerToken, root: base })
    await host.start()
    try {
      const roleOf = async (file: string) => {
        const token = fs.readFileSync(file, 'utf8')
        let role = null as string | null
        const socket = new WebSocket(host.url)
        socket.onopen = () => socket.send(JSON.stringify({ t: 'hello', protocol: 1, token, client: { name: 'raw', kind: 'cli' } }))
        socket.onmessage = (event) => {
          const frame = JSON.parse(String(event.data)) as HostFrame
          if (frame.t === 'welcome') role = frame.role ?? 'none'
        }
        const deadline = Date.now() + 5_000
        while (role === null && Date.now() < deadline) await Bun.sleep(10)
        socket.close()
        return role
      }
      expect(await roleOf(tokens.tokenFile)).toBe('owner')
      expect(await roleOf(tokens.peerTokenFile!)).toBe('peer')
    } finally {
      await host.stop()
    }
  })
})
