import { describe, expect, test } from 'bun:test'

import { SessionController, type EngineTransport, type TransportClose } from '../src/controller'
import { DESKTOP_CAPABILITIES, type ClientMessage } from '../src/protocol'
import type { SessionAction } from '../src/session'

/** A local engine that says nothing but what the test emits. */
class FakeTransport implements EngineTransport {
  readonly echoesActions = false
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  async start(): Promise<void> {}
  send(message: ClientMessage): void {
    this.sent.push(message)
  }
  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  onClose(_listener: (close: TransportClose) => void): () => void {
    return () => {}
  }
  async close(): Promise<void> {}
  emit(action: SessionAction): void {
    for (const listener of this.listeners) listener(action)
  }
}

const ready: SessionAction = { type: 'ready', protocol: 1, version: '1', features: ['capabilities'] }

describe('capabilities', () => {
  test('default to questions, as before', () => {
    const transport = new FakeTransport()
    new SessionController(transport)
    transport.emit(ready)
    expect(transport.sent).toEqual([{ kind: 'command', type: 'capabilities', features: ['questions'] }])
  })

  test('the options set them, and the client name goes along', () => {
    const transport = new FakeTransport()
    new SessionController(transport, { client: 'desktop', capabilities: DESKTOP_CAPABILITIES })
    transport.emit(ready)
    expect(transport.sent).toEqual([
      { kind: 'command', type: 'capabilities', features: [...DESKTOP_CAPABILITIES], client: 'desktop' },
    ])
  })

  test('an engine without the capabilities feature hears nothing', () => {
    const transport = new FakeTransport()
    new SessionController(transport, { capabilities: DESKTOP_CAPABILITIES })
    transport.emit({ type: 'ready', protocol: 1, version: '1' })
    expect(transport.sent).toEqual([])
  })
})

describe('the clock', () => {
  test('apply stamps tool calls with the injected time', () => {
    const transport = new FakeTransport()
    let time = 1000
    const controller = new SessionController(transport, { now: () => time })
    transport.emit({ type: 'tool_input_started', id: 't1', name: 'bash' })
    time = 2500
    transport.emit({ type: 'tool_result', id: 't1', name: 'bash', output: 'x', is_error: false })
    const item = controller.getState().items[0]
    expect(item).toMatchObject({ kind: 'tool', startedAt: 1000, endedAt: 2500 })
  })

  test('without one it reads the wall clock', () => {
    const transport = new FakeTransport()
    const controller = new SessionController(transport)
    const before = Date.now()
    transport.emit({ type: 'tool_input_started', id: 't1', name: 'bash' })
    const item = controller.getState().items[0] as { startedAt: number }
    expect(item.startedAt).toBeGreaterThanOrEqual(before)
    expect(item.startedAt).toBeLessThanOrEqual(Date.now())
  })
})

describe('the new commands', () => {
  const setup = () => {
    const transport = new FakeTransport()
    const controller = new SessionController(transport)
    return { transport, controller }
  }

  test('commit sends the message and closes the proposal', () => {
    const { transport, controller } = setup()
    transport.emit({ type: 'commit_proposal', message: 'm', branch: 'main', files: [], stage_all: true })
    expect(controller.getState().commitProposal).not.toBeNull()
    controller.commit('feat: x')
    expect(transport.sent).toEqual([{ kind: 'command', type: 'commit', message: 'feat: x' }])
    expect(controller.getState().commitProposal).toBeNull()
  })

  test('dismissCommit closes the proposal and sends nothing', () => {
    const { transport, controller } = setup()
    transport.emit({ type: 'commit_proposal', message: 'm', branch: null, files: [], stage_all: false })
    controller.dismissCommit()
    expect(controller.getState().commitProposal).toBeNull()
    expect(transport.sent).toEqual([])
  })

  test('rewind sends the checkpoint, the mode and whether it is a dry run', () => {
    const { transport, controller } = setup()
    controller.rewind(3, 'chat_only')
    controller.rewind(2, 'code_and_chat', true)
    expect(transport.sent).toEqual([
      { kind: 'command', type: 'rewind', checkpoint: 3, mode: 'chat_only', dry_run: false },
      { kind: 'command', type: 'rewind', checkpoint: 2, mode: 'code_and_chat', dry_run: true },
    ])
  })

  test('setMcpServerEnabled sends the name and the switch', () => {
    const { transport, controller } = setup()
    controller.setMcpServerEnabled('github', false)
    expect(transport.sent).toEqual([{ kind: 'command', type: 'set_mcp_server_enabled', name: 'github', enabled: false }])
  })

  test('requestDiff, requestCheckpoints and proposeCommit are slash commands that are not shown as messages', () => {
    const { transport, controller } = setup()
    controller.requestDiff()
    controller.requestDiff('src/a.rs')
    controller.requestCheckpoints()
    controller.proposeCommit()
    expect(transport.sent).toEqual([
      { kind: 'command', type: 'slash', command: 'diff', args: '' },
      { kind: 'command', type: 'slash', command: 'diff', args: 'src/a.rs' },
      { kind: 'command', type: 'slash', command: 'rewind', args: '' },
      { kind: 'command', type: 'slash', command: 'commit', args: '' },
    ])
    expect(controller.getState().items).toEqual([])
    expect(controller.getState().busy).toBe(false)
  })
})
