import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { isUpdaterSigned } from '../scripts/updater-signed'

describe('updater signing flag', () => {
  test('needs a real public key', () => {
    expect(isUpdaterSigned({ plugins: { updater: { pubkey: 'dW50cnVzdGVk' } } })).toBe(true)
    for (const pubkey of ['', '   ', '\n\t', undefined, 5]) expect(isUpdaterSigned({ plugins: { updater: { pubkey } } })).toBe(false)
    expect(isUpdaterSigned({})).toBe(false)
  })
})

describe('vite config', () => {
  async function defines(command: 'build' | 'serve') {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-vite-'))
    const file = path.join(dir, 'token')
    fs.writeFileSync(file, 'pairing-token-123\n')
    process.env.NEURON_HOST_TOKEN_FILE = file
    process.env.NEURON_HOST_URL = 'ws://127.0.0.1:9/neuron'
    try {
      const make = (await import('../vite.config')).default as unknown as (env: { command: string; mode: string }) => { define: Record<string, string> }
      return make({ command, mode: command === 'serve' ? 'development' : 'production' }).define
    } finally {
      delete process.env.NEURON_HOST_TOKEN_FILE
      delete process.env.NEURON_HOST_URL
      fs.rmSync(dir, { recursive: true, force: true })
    }
  }

  test('a production build never embeds the host token or URL', async () => {
    const define = await defines('build')
    expect(define['import.meta.env.VITE_NEURON_HOST_TOKEN']).toBe('""')
    expect(define['import.meta.env.VITE_NEURON_HOST_URL']).toBe('""')
  })

  test('the dev server injects them', async () => {
    const define = await defines('serve')
    expect(define['import.meta.env.VITE_NEURON_HOST_TOKEN']).toBe('"pairing-token-123"')
    expect(define['import.meta.env.VITE_NEURON_HOST_URL']).toBe('"ws://127.0.0.1:9/neuron"')
  })
})

describe('tauri.conf.json', () => {
  const conf = JSON.parse(fs.readFileSync(path.resolve(import.meta.dir, '../src-tauri/tauri.conf.json'), 'utf8'))

  test('the CSP allows blob: image and media previews and nothing else new', () => {
    expect(conf.app.security.csp).toBe(
      "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src ipc: http://ipc.localhost",
    )
  })

  test('the main window leaves drag and drop to the webview', () => {
    expect(conf.app.windows.find((w: { label: string }) => w.label === 'main').dragDropEnabled).toBe(false)
  })
})
