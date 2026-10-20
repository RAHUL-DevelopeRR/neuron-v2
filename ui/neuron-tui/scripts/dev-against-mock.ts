// Runs the UI against the real engine and the mock Anthropic service, with no
// account and no network: `bun run dev:mock [prompt...]`. Prompts of the form
// `PARITY_SCENARIO:<name>` pick a scripted turn (streaming_text,
// bash_permission_prompt_approved, write_file_allowed, read_file_roundtrip,
// grep_chunk_assembly, multi_tool_turn_roundtrip, token_cost_reporting, ...).
//
// `bun run dev:mock --signed-out` starts the engine signed out of Zero-X
// against a local fake gateway (fake-zero-x-gateway.ts) instead, to try the
// sign-in panel, /login, /model and /logout. Its approval comes at the first
// poll; prompts after it fail, since the fake has no chat endpoint.
//
// Needs `cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/,
// or NEURON_BIN and NEURON_MOCK_BIN. Everything (engine config, UI settings,
// chat history, the workspace) lives in a temp folder that is removed on exit.
import fs from 'fs'
import os from 'os'
import path from 'path'

import { startFakeGateway } from './fake-zero-x-gateway'

const exe = (name: string) => (process.platform === 'win32' ? `${name}.exe` : name)
const repoRoot = path.resolve(import.meta.dir, '../../..')
const debugDir = path.join(repoRoot, 'rust', 'target', 'debug')
const neuronBin = process.env.NEURON_BIN || path.join(debugDir, exe('neuron'))
const mockBin = process.env.NEURON_MOCK_BIN || path.join(debugDir, exe('mock-anthropic-service'))

for (const [label, file] of [
  ['neuron', neuronBin],
  ['mock-anthropic-service', mockBin],
] as const) {
  if (!fs.existsSync(file)) {
    console.error(`${label} not found at ${file}. Build it in rust/ or set its path.`)
    process.exit(1)
  }
}

const args = process.argv.slice(2)
const signedOut = args[0] === '--signed-out'
if (signedOut) args.shift()

const mock = Bun.spawn([mockBin], { stdout: 'pipe', stderr: 'ignore' })
const reader = mock.stdout.getReader()
const decoder = new TextDecoder()
let baseUrl = ''
let seen = ''
while (!baseUrl) {
  const { value, done } = await reader.read()
  if (done) {
    console.error('The mock Anthropic service exited before printing its URL.')
    process.exit(1)
  }
  seen += decoder.decode(value)
  baseUrl = /MOCK_ANTHROPIC_BASE_URL=(\S+)/.exec(seen)?.[1] ?? ''
}
reader.releaseLock()

const home = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-mock-'))
const workspace = path.join(home, 'workspace')
fs.mkdirSync(workspace)
fs.writeFileSync(path.join(workspace, 'fixture.txt'), 'alpha parity line\nbeta line\ngamma parity line\n')

// Only what a terminal and the OS need, so the user's own provider keys and
// Neuron settings stay out of it.
const passThrough = [
  'PATH', 'PATHEXT', 'SystemRoot', 'SystemDrive', 'ComSpec', 'windir', 'TEMP', 'TMP',
  'APPDATA', 'LOCALAPPDATA', 'TERM', 'COLORTERM', 'TERM_PROGRAM', 'TERM_PROGRAM_VERSION',
  'WT_SESSION', 'WT_PROFILE_ID', 'LANG', 'LC_ALL',
]
const env: Record<string, string> = {}
for (const name of passThrough) {
  const value = process.env[name]
  if (value !== undefined) env[name] = value
}
Object.assign(env, {
  HOME: home,
  USERPROFILE: home,
  NEURON_CONFIG_HOME: path.join(home, 'config'),
  NEURON_HOME: path.join(home, 'neuron'),
  // Logins go to a file in the temp home, never the OS keychain.
  NEURON_CREDENTIAL_STORE: 'file',
  NEURON_NO_BROWSER: '1',
  NEURON_TUI_CONFIG_DIR: path.join(home, 'tui'),
  NEURON_BIN: neuronBin,
})
const gateway = signedOut ? startFakeGateway() : null
if (gateway) {
  env.NEURON_API_BASE = gateway.base
} else {
  env.ANTHROPIC_API_KEY = 'mock-service-key'
  env.ANTHROPIC_BASE_URL = baseUrl
}

const ui = Bun.spawn(
  [process.execPath, 'run', path.join(import.meta.dir, '..', 'src', 'entry.ts'), ...args],
  { cwd: workspace, env, stdio: ['inherit', 'inherit', 'inherit'] },
)
const code = await ui.exited
mock.kill()
gateway?.stop()
fs.rmSync(home, { recursive: true, force: true })
process.exit(code)
