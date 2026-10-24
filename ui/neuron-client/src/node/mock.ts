/**
 * The engine against the mock Anthropic service, for tests and dev scripts,
 * with no account and no network. Needs `cargo build -p rusty-claude-cli -p
 * mock-anthropic-service` in rust/, or NEURON_BIN and NEURON_MOCK_BIN.
 * Everything the engine writes lives in a temp home; the user's own keys,
 * settings and keychain stay out of it.
 */

import fs from 'fs'
import os from 'os'
import path from 'path'

const exe = (name: string) => (process.platform === 'win32' ? `${name}.exe` : name)
const debugDir = path.resolve(import.meta.dir, '../../../../rust/target/debug')

export const neuronBinary = process.env.NEURON_BIN || path.join(debugDir, exe('neuron'))
export const mockBinary = process.env.NEURON_MOCK_BIN || path.join(debugDir, exe('mock-anthropic-service'))
export const haveEngineBinaries = fs.existsSync(neuronBinary) && fs.existsSync(mockBinary)

export interface MockService {
  baseUrl: string
  stop(): void
}

/** Starts the mock service and waits for its URL. */
export async function startMockService(): Promise<MockService> {
  const child = Bun.spawn([mockBinary], { stdout: 'pipe', stderr: 'ignore' })
  const reader = child.stdout.getReader()
  const decoder = new TextDecoder()
  let seen = ''
  let baseUrl = ''
  while (!baseUrl) {
    const { value, done } = await reader.read()
    if (done) throw new Error('The mock Anthropic service exited before printing its URL.')
    seen += decoder.decode(value)
    baseUrl = /MOCK_ANTHROPIC_BASE_URL=(\S+)/.exec(seen)?.[1] ?? ''
  }
  reader.releaseLock()
  return { baseUrl, stop: () => child.kill() }
}

/** Only what a process and the OS need: no provider keys or Neuron settings from this one. */
const PASS_THROUGH = ['PATH', 'PATHEXT', 'SystemRoot', 'SystemDrive', 'ComSpec', 'windir', 'TEMP', 'TMP', 'LANG', 'LC_ALL']

export interface MockHome {
  home: string
  workspace: string
  /** The engine's environment, pointed at the mock service. */
  env: Record<string, string>
  remove(): void
}

/** A throwaway home with a workspace holding `fixture.txt`, as the mock's scenarios expect. */
export function makeMockHome(baseUrl: string, label = 'neuron-client'): MockHome {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `${label}-`))
  const workspace = path.join(home, 'workspace')
  fs.mkdirSync(workspace, { recursive: true })
  fs.writeFileSync(path.join(workspace, 'fixture.txt'), 'alpha parity line\nbeta line\ngamma parity line\n')
  // The bundled MCP servers start through npx, which downloads them into each fresh
  // home (hundreds of MB) and slows every start; the mock scenarios use none.
  const config = path.join(home, 'config')
  fs.mkdirSync(config, { recursive: true })
  fs.writeFileSync(path.join(config, 'settings.json'), JSON.stringify({ defaultMcpServers: false }))
  const env: Record<string, string> = {}
  for (const name of PASS_THROUGH) {
    const value = process.env[name]
    if (value !== undefined) env[name] = value
  }
  Object.assign(env, {
    HOME: home,
    USERPROFILE: home,
    NEURON_CONFIG_HOME: config,
    NEURON_HOME: path.join(home, 'neuron'),
    NEURON_CREDENTIAL_STORE: 'file',
    NEURON_NO_BROWSER: '1',
    NO_COLOR: '1',
    ANTHROPIC_API_KEY: 'mock-service-key',
    ANTHROPIC_BASE_URL: baseUrl,
  })
  return {
    home,
    workspace,
    env,
    remove: () => {
      try {
        // The engine can hold files for a moment after it exits (Windows refuses then).
        fs.rmSync(home, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 })
      } catch {
        // Left for the OS to clean up with the rest of the temp folder.
      }
    },
  }
}
