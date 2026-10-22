// The desktop UI in a browser against the real engine and the mock Anthropic
// service, with no account and no network: `bun run dev:mock`, then open
// http://127.0.0.1:1420. Prompts of the form `PARITY_SCENARIO:<name>` pick a
// scripted turn (streaming_text, bash_permission_prompt_approved,
// write_file_allowed, read_file_roundtrip, multi_tool_turn_roundtrip, ...); the
// mock picks one per conversation, so `/clear` before the next.
//
// Needs `cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/.
// The pairing token goes to a temp file that Vite reads; it is never printed.
import { randomBytes } from 'crypto'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { NeuronHost } from '../../neuron-client/src/node/host'
import { haveEngineBinaries, makeMockHome, neuronBinary, startMockService } from '../../neuron-client/src/node/mock'
import { StdioTransport } from '../../neuron-client/src/node/stdio-transport'

if (!haveEngineBinaries) {
  console.error('Build the engine and the mock first: `cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/.')
  process.exit(1)
}

const mock = await startMockService()
const home = makeMockHome(mock.baseUrl, 'neuron-desktop-mock')
fs.mkdirSync(path.join(home.workspace, 'src'), { recursive: true })
fs.writeFileSync(path.join(home.workspace, 'src', 'main.rs'), 'fn main() {\n    println!("hello from the mock workspace");\n}\n')
fs.writeFileSync(path.join(home.workspace, 'README.md'), '# Mock workspace\n\nThe engine runs here against the mock Anthropic service.\n')

const token = randomBytes(24).toString('base64url')
const tokenFile = path.join(os.tmpdir(), `neuron-desktop-${process.pid}.token`)
fs.writeFileSync(tokenFile, token, { mode: 0o600 })

const transport = new StdioTransport({
  binary: neuronBinary,
  cwd: home.workspace,
  model: 'sonnet',
  permissionMode: 'workspace-write',
  env: home.env,
})
const host = new NeuronHost({ transport, token, root: home.workspace, name: 'Neuron (mock)', terminalEnv: { ...home.env } })
await host.start()

const vite = Bun.spawn([process.execPath, 'x', 'vite', ...process.argv.slice(2)], {
  cwd: path.resolve(import.meta.dir, '..'),
  env: { ...process.env, NEURON_HOST_URL: host.url, NEURON_HOST_TOKEN_FILE: tokenFile },
  stdio: ['inherit', 'inherit', 'inherit'],
})
console.log(`Engine ${host.info.version} on the mock service, workspace ${home.workspace}`)

let stopping = false
async function stop() {
  if (stopping) return
  stopping = true
  vite.kill()
  await host.stop().catch(() => {})
  mock.stop()
  fs.rmSync(tokenFile, { force: true })
  home.remove()
  process.exit(0)
}
process.on('SIGINT', () => void stop())
process.on('SIGTERM', () => void stop())
await vite.exited
await stop()
