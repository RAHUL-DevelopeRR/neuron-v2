/**
 * The real `neuron engine --stdio` against the mock Anthropic service: a
 * session controller over stdio, and the host with two remote clients (the
 * desktop and the phone) steering one engine. Needs `cargo build -p
 * rusty-claude-cli -p mock-anthropic-service` in rust/ (or NEURON_BIN and
 * NEURON_MOCK_BIN); skipped without them.
 */
import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import { SessionController } from '../src/controller'
import { NeuronHost } from '../src/node/host'
import { haveEngineBinaries, makeMockHome, neuronBinary, startMockService, type MockService } from '../src/node/mock'
import { StdioTransport } from '../src/node/stdio-transport'
import { WebSocketTransport } from '../src/remote'
import type { SessionState, TranscriptItem } from '../src/session'

const TIMEOUT = 120_000
let mock: MockService

const until = async (label: string, predicate: () => boolean, timeoutMs = 60_000) => {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(20)
  }
}

function engineFor(label: string) {
  const home = makeMockHome(mock.baseUrl, `neuron-client-${label}`)
  const transport = new StdioTransport({
    binary: neuronBinary,
    cwd: home.workspace,
    model: 'sonnet',
    permissionMode: 'workspace-write',
    env: home.env,
  })
  return { home, transport }
}

const tools = (state: SessionState) => state.items.filter((item): item is Extract<TranscriptItem, { kind: 'tool' }> => item.kind === 'tool')
const text = (state: SessionState) =>
  state.items
    .filter((item) => item.kind === 'assistant')
    .map((item) => (item as Extract<TranscriptItem, { kind: 'assistant' }>).text)
    .join('')

/**
 * Runs one scenario through a controller, answering permission requests with
 * `allow`; `check` sees the workspace before it is removed.
 */
async function scenario(name: string, allow: boolean, check: (workspace: string) => void = () => {}) {
  const { home, transport } = engineFor(name)
  const controller = new SessionController(transport)
  controller.subscribe(() => {
    for (const request of controller.getState().permissions) {
      controller.answerPermission(request.id, allow ? { kind: 'allow_once' } : { kind: 'deny', note: 'the test says no' })
    }
  })
  try {
    await controller.start()
    controller.send(`PARITY_SCENARIO:${name}`)
    await until('the turn to finish', () => {
      const state = controller.getState()
      return !state.busy && state.usage !== null
    })
    check(home.workspace)
    return { state: controller.getState() }
  } finally {
    await controller.close()
    home.remove()
  }
}

describe.skipIf(!haveEngineBinaries)('the real engine', () => {
  beforeAll(async () => {
    mock = await startMockService()
  })
  afterAll(() => mock?.stop())

  test(
    'a turn with a file write: the diff, the result and the reply',
    async () => {
      const { state } = await scenario('write_file_allowed', true)
      expect(state.version).toMatch(/^\d+\.\d+\.\d+/)
      expect(state.status?.permission_mode).toBe('workspace-write')
      const [write] = tools(state)
      expect(write).toMatchObject({ name: 'write_file', state: 'done' })
      expect(write.view.target).toBe('generated/output.txt')
      expect(write.view.diff?.lines.map((line) => [line.kind, line.text])).toEqual([['add', 'created by mock service']])
      expect(text(state)).toContain('write_file succeeded')
      expect(state.usage?.turn.output).toBeGreaterThan(0)
    },
    TIMEOUT,
  )

  test(
    'a bash call that needs approval, approved',
    async () => {
      const { state } = await scenario('bash_permission_prompt_approved', true, (workspace) =>
        expect(fs.readFileSync(path.join(workspace, 'approved.txt'), 'utf8')).toBe('approved via prompt'),
      )
      const [bash] = tools(state)
      expect(bash).toMatchObject({ name: 'bash', state: 'done', permissionId: 1 })
      expect(bash.view.output).toContain('approved via prompt')
      expect(state.permissions).toEqual([])
    },
    TIMEOUT,
  )

  test(
    'a bash call that needs approval, denied',
    async () => {
      const { state } = await scenario('bash_permission_prompt_denied', false, (workspace) =>
        // Nothing but the fixture and the engine's own .neuron folder.
        expect(fs.readdirSync(workspace).filter((name) => !name.startsWith('.'))).toEqual(['fixture.txt']),
      )
      const [bash] = tools(state)
      expect(bash.state).toBe('denied')
      expect(bash.output).toContain('the test says no')
    },
    TIMEOUT,
  )

  test(
    'two calls in one turn, matched by id',
    async () => {
      const { state } = await scenario('multi_tool_turn_roundtrip', true)
      expect(tools(state).map((tool) => [tool.name, tool.state])).toEqual([
        ['read_file', 'done'],
        ['grep_search', 'done'],
      ])
      expect(tools(state)[0].view.output).toContain('alpha parity line')
    },
    TIMEOUT,
  )

  test(
    'the host: the phone starts a turn, the desktop watches, the phone approves',
    async () => {
      const { home, transport } = engineFor('host')
      const host = new NeuronHost({ transport, token: 'integration-pairing-token', root: home.workspace, name: 'integration' })
      const clients: SessionController[] = []
      const attach = async (kind: 'desktop' | 'mobile') => {
        const controller = new SessionController(
          new WebSocketTransport({ url: host.url, token: 'integration-pairing-token', client: { name: kind, kind } }),
        )
        clients.push(controller)
        await controller.start()
        return controller
      }
      try {
        await host.start()
        const desktop = await attach('desktop')
        const phone = await attach('mobile')
        phone.send('PARITY_SCENARIO:bash_permission_prompt_approved')
        await until('the desktop sees the request', () => desktop.getState().permissions.length === 1)
        const request = phone.getState().permissions[0]
        expect(request.request.tool).toBe('bash')
        phone.answerPermission(request.id, { kind: 'allow_once' })
        for (const client of [desktop, phone]) {
          await until('the turn to finish', () => !client.getState().busy && client.getState().usage !== null)
          expect(tools(client.getState()).map((tool) => tool.state)).toEqual(['done'])
          expect(client.getState().items.filter((item) => item.kind === 'user')).toHaveLength(1)
        }
        expect(fs.readFileSync(path.join(home.workspace, 'approved.txt'), 'utf8')).toBe('approved via prompt')
        // A client that joins late gets the whole session.
        const late = await attach('mobile')
        expect(tools(late.getState()).map((tool) => tool.state)).toEqual(['done'])
        expect(text(late.getState())).toContain('approved')
      } finally {
        for (const client of clients) await client.close()
        await host.stop()
        home.remove()
      }
    },
    TIMEOUT,
  )

  test(
    'chat history: list the chats, resume one, and its conversation comes back',
    async () => {
      const { home, transport } = engineFor('history')
      const controller = new SessionController(transport)
      try {
        await controller.start()
        const state = () => controller.getState()
        expect(state().features).toEqual(expect.arrayContaining(['sessions', 'trust_workspace', 'capabilities']))
        controller.send('PARITY_SCENARIO:write_file_allowed')
        await until('the first turn', () => !state().busy && state().usage !== null)
        const first = state().status!.session_id

        controller.newChat()
        await until('a fresh chat', () => !state().busy && state().status?.session_id !== first)
        expect(state().items.filter((item) => item.kind !== 'user' && item.kind !== 'notice')).toEqual([])

        controller.listSessions()
        await until('the chat list', () => state().sessions !== null)
        const listed = state().sessions!.find((session) => session.id === first)
        expect(listed).toMatchObject({ title: 'PARITY_SCENARIO:write_file_allowed', current: false })

        controller.resume(first)
        await until('the old chat', () => state().status?.session_id === first && state().items.some((item) => item.kind === 'tool'))
        expect(state().items[0]).toMatchObject({ kind: 'user', text: 'PARITY_SCENARIO:write_file_allowed' })
        const [write] = tools(state())
        expect(write).toMatchObject({ name: 'write_file', state: 'done' })
        expect(write.view.target).toBe('generated/output.txt')
        expect(text(state())).toContain('write_file succeeded')
      } finally {
        await controller.close()
        home.remove()
      }
    },
    TIMEOUT,
  )

  test(
    'extensions: a failing MCP server and the workspace skill',
    async () => {
      const { home, transport } = engineFor('extensions')
      // A server whose command does not exist, beside the settings the home already has.
      const settings = path.join(home.env.NEURON_CONFIG_HOME, 'settings.json')
      fs.writeFileSync(
        settings,
        JSON.stringify({ defaultMcpServers: false, mcpServers: { ghost: { command: 'neuron-test-missing-mcp-server' } } }),
      )
      const skill = path.join(home.workspace, '.claw', 'skills', 'release-notes')
      fs.mkdirSync(skill, { recursive: true })
      fs.writeFileSync(path.join(skill, 'SKILL.md'), '---\nname: release-notes\ndescription: Drafts release notes\n---\nWrite them.\n')
      const controller = new SessionController(transport)
      try {
        await controller.start()
        const state = () => controller.getState()
        expect(state().features).toContain('extensions')
        // The servers start in the background: ask until the ghost is past starting.
        const ghost = () => state().extensions?.mcpServers.find((server) => server.name === 'ghost')
        let asked = 0
        await until('the ghost server settles', () => {
          const server = ghost()
          if (server && server.status !== 'starting') return true
          if (Date.now() - asked > 500) {
            asked = Date.now()
            controller.listExtensions()
          }
          return false
        })
        const server = ghost()!
        expect(server).toMatchObject({ scope: 'user', transport: 'stdio', tools: [] })
        expect(['failed', 'pending']).toContain(server.status)
        // Nothing about how it starts leaves the engine.
        expect(JSON.stringify(state().extensions)).not.toContain('neuron-test-missing-mcp-server')
        expect(state().extensions!.skills).toContainEqual(
          expect.objectContaining({ name: 'release-notes', description: 'Drafts release notes', active: true }),
        )
      } finally {
        await controller.close()
        home.remove()
      }
    },
    TIMEOUT,
  )
})
