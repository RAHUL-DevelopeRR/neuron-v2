// Serves one Neuron engine to remote clients (the desktop app in a browser,
// the mobile app on the LAN):
//
//   bun run host [--mock] [--lan] [--port N] [--cwd DIR] [--token-file FILE] [--peer-token-file FILE] [--show-token]
//
// Two tokens, two roles, decided by the token and not by the address. The owner token (--token-file) is
// yours: a client on this machine that says hello with it is the owner, which the desktop app in a browser
// needs (every slash command, a new session, the permission mode, login, resume by path, a terminal, an
// engine restart). The peer token (--peer-token-file, made only when you name a file) is for a bridge such
// as `neuron engine --share`, whose phones reach this host from 127.0.0.1 too: give the bridge that file
// and never the owner's. Whoever holds the peer token is a peer wherever it connects from.
//
// --mock runs the engine against the mock Anthropic service in a temp home,
// with no account and no network (needs `cargo build -p rusty-claude-cli -p
// mock-anthropic-service` in rust/). Otherwise it runs `neuron engine --stdio`
// in --cwd (default: here) with your own Neuron login and settings.
//
// The tokens are random per run. The owner token goes to --token-file (default: a
// file in the temp folder, readable only by you) and is printed only with
// --show-token; the peer token is never printed. --lan listens on every interface
// so a phone on the same network can connect; the traffic is not encrypted, so use
// it on networks you trust (the zero-x.live relay will carry it over TLS).
import os from 'os'
import path from 'path'

import { NeuronHost } from '../src/node/host'
import { haveEngineBinaries, makeMockHome, neuronBinary, startMockService, type MockHome, type MockService } from '../src/node/mock'
import { StdioTransport } from '../src/node/stdio-transport'
import { removeHostTokens, writeHostTokens, type HostTokens } from '../src/node/token-files'

const args = process.argv.slice(2)
const flag = (name: string) => args.includes(name)
const value = (name: string) => {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : undefined
}
const fail = (message: string): never => {
  console.error(message)
  return process.exit(1)
}

// Before anything is started: a mistake here leaves nothing behind.
if (flag('--peer-token-file') && !value('--peer-token-file')?.trim()) fail('--peer-token-file needs a file name.')
if (flag('--peer-token-file') && value('--peer-token-file')!.startsWith('--')) fail('--peer-token-file needs a file name, not another flag.')
let tokens: HostTokens
try {
  tokens = writeHostTokens({
    tokenFile: value('--token-file') ?? path.join(os.tmpdir(), `neuron-host-${process.pid}.token`),
    peerTokenFile: value('--peer-token-file'),
  })
} catch (error) {
  tokens = fail(error instanceof Error ? error.message : String(error))
}

const mockMode = flag('--mock')
let mock: MockService | null = null
let mockHome: MockHome | null = null
let cwd = path.resolve(value('--cwd') ?? process.cwd())
let env: Record<string, string | undefined> = { ...process.env }
let binary = process.env.NEURON_BIN ?? 'neuron'

if (mockMode) {
  if (!haveEngineBinaries) {
    console.error('The engine or the mock service is not built: run `cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/.')
    process.exit(1)
  }
  mock = await startMockService()
  mockHome = makeMockHome(mock.baseUrl, 'neuron-host-mock')
  cwd = mockHome.workspace
  env = mockHome.env
  binary = neuronBinary
}

// A new engine for each (re)start; a restart continues the chat it was on.
const engineFor = (resume: string | null) =>
  new StdioTransport(
    {
      binary,
      cwd,
      env,
      model: value('--model') ?? (mockMode ? 'sonnet' : undefined),
      permissionMode: value('--permission-mode') ?? 'workspace-write',
      resume: resume ?? undefined,
    },
    { onStderr: (line) => flag('--verbose') && console.error(`[engine] ${line}`) },
  )
const host = new NeuronHost({
  transport: engineFor(null),
  restart: engineFor,
  token: tokens.token,
  peerToken: tokens.peerToken,
  root: cwd,
  name: mockMode ? 'Neuron (mock)' : os.hostname(),
  hostname: flag('--lan') ? '0.0.0.0' : '127.0.0.1',
  port: Number(value('--port') ?? 0),
})

let stopping = false
async function stop(code = 0) {
  if (stopping) return
  stopping = true
  await host.stop().catch(() => {})
  mock?.stop()
  mockHome?.remove()
  removeHostTokens(tokens)
  process.exit(code)
}
process.on('SIGINT', () => void stop())
process.on('SIGTERM', () => void stop())

try {
  await host.start()
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error))
  await stop(1)
}

console.log(`Neuron host ${host.info.version ?? ''} for ${cwd}`)
console.log(`  url:        ${host.url}`)
if (flag('--lan')) {
  const lan = Object.values(os.networkInterfaces())
    .flat()
    .filter((address) => address && address.family === 'IPv4' && !address.internal)
    .map((address) => `ws://${address!.address}:${new URL(host.url).port}/neuron`)
  for (const url of lan) console.log(`  lan:        ${url}`)
}
console.log(`  token file: ${tokens.tokenFile}   (the owner's: for the desktop app on this machine)`)
if (flag('--show-token')) console.log(`  token:      ${tokens.token}`)
if (tokens.peerTokenFile) {
  console.log(`  peer token: ${tokens.peerTokenFile}   (for \`neuron engine --share --peer-token-file\`; never give a bridge the owner's file)`)
}
console.log('Ctrl-C stops it.')
