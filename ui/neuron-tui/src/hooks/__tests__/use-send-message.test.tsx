// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the run
// lifecycle is driven through a fake engine client instead of the Codebuff
// SDK's client.run(), and an interrupted turn keeps its engine session and
// says how its running tool call ended. See ui/neuron-tui/PORTING.md.
import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React, { useEffect, useRef } from 'react'

import { useSendMessage } from '../use-send-message'
import {
  EngineNotConnectedError,
  getNeuronEngineClient,
  type EngineExit,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import { handleEngineEvent } from '../../engine/engine-session'
import { setProjectRoot, tryGetProjectRoot } from '../../project-files'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { stopActiveRun } from '../../utils/active-run'
import {
  flushLiveChatState,
  resumeSessionFor,
  saveChatState,
  setChatDirOverrideForTesting,
  settleCheckpointSave,
} from '../../utils/run-state-storage'

import type {
  Attachment,
  EngineCommand,
  PermissionReply,
  ReadyEvent,
  StatusEvent,
  UiEvent,
} from '../../engine/protocol'
import type { SendMessageFn } from '../../types/contracts/send-message'
import type { ElapsedTimeTracker } from '../use-elapsed-time'

/** An engine that records commands and emits whatever the test tells it to. */
class FakeEngine implements NeuronEngineClient {
  readonly connection = 'ready' as const
  submits: Array<{ text: string; attachments: Attachment[] }> = []
  interrupts = 0
  private listeners = new Set<(event: UiEvent) => void>()

  start(): Promise<ReadyEvent> {
    return Promise.resolve({ type: 'ready', protocol: 1, version: 'test' })
  }
  send(_command: EngineCommand): void {}
  submit(text: string, attachments: Attachment[] = []): void {
    this.submits.push({ text, attachments })
  }
  slash(): void {}
  setModel(): void {}
  listModels(): void {}
  interrupt(): void {
    this.interrupts++
  }
  replyToPermission(_reply: PermissionReply): void {}
  subscribe(listener: (event: UiEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  onExit(_listener: (exit: EngineExit) => void): () => void {
    return () => {}
  }
  shutdown(): Promise<EngineExit> {
    return Promise.resolve({ code: 0, signal: null })
  }
  emit(event: UiEvent): void {
    for (const listener of [...this.listeners]) listener(event)
  }
}

let sendMessageFromHost: SendMessageFn | null = null
let engine: NeuronEngineClient
let testRoot: string
let originalProjectRoot: string

const makeTimer = (): ElapsedTimeTracker => ({
  start: () => {},
  stop: () => {},
  pause: () => {},
  resume: () => {},
  elapsedSeconds: 0,
  startTime: null,
  isPaused: false,
})

const Host = () => {
  const inputRef = useRef<any>(null)
  const activeSubagentsRef = useRef(new Set<string>())
  const isChainInProgressRef = useRef(false)
  const isQueuePausedRef = useRef(false)
  const isProcessingQueueRef = useRef(false)
  const mainAgentTimer = useRef(makeTimer()).current

  const { sendMessage } = useSendMessage({
    inputRef,
    activeSubagentsRef,
    isChainInProgressRef,
    setStreamStatus: () => {},
    setCanProcessQueue: () => {},
    onBeforeMessageSend: async () => ({ success: true, errors: [] }),
    mainAgentTimer,
    scrollToLatest: () => {},
    isQueuePausedRef,
    isProcessingQueueRef,
    resumeQueue: () => {},
    requeueMessageAtFront: () => {},
    continueChat: false,
    getEngine: () => engine,
  })

  useEffect(() => {
    sendMessageFromHost = sendMessage
  }, [sendMessage])

  return <text>send-message-test-host</text>
}

const waitFor = async (label: string, predicate: () => boolean) => {
  const deadline = Date.now() + 4_000
  while (Date.now() < deadline) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  throw new Error(`Timed out waiting for ${label}`)
}

const savedMessages = () =>
  JSON.parse(fs.readFileSync(path.join(testRoot, 'chat-messages.json'), 'utf8'))
const savedRunState = () =>
  JSON.parse(fs.readFileSync(path.join(testRoot, 'run-state.json'), 'utf8'))

/** The engine's footer status, naming its session. */
const engineStatus = (
  session_id: string,
  model = 'nvidia/stepfun-ai/step-5-preview',
): StatusEvent => ({
  type: 'status',
  model,
  permission_mode: 'workspace-write',
  session_id,
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 0,
  git: null,
})

beforeEach(() => {
  sendMessageFromHost = null
  engine = getNeuronEngineClient()
  testRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'use-send-message-'))
  originalProjectRoot = tryGetProjectRoot() ?? process.cwd()
  setProjectRoot(process.cwd())
  setChatDirOverrideForTesting(testRoot)
  useChatStore.getState().reset()
  useEngineStore.getState().reset()
})

afterEach(() => {
  stopActiveRun('process-exit')
  useEngineStore.getState().reset()
  setChatDirOverrideForTesting(undefined)
  setProjectRoot(originalProjectRoot)
  fs.rmSync(testRoot, { recursive: true, force: true })
})

const mountHost = async () => {
  const setup = await createTestRenderer({ width: 80, height: 3 })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<Host />))
  await setup.renderOnce()
  expect(sendMessageFromHost).not.toBeNull()
  return { setup, root }
}

describe('useSendMessage with the engine client', () => {
  test('without a running engine the turn ends with the not-running error, and is saved', async () => {
    const { setup, root } = await mountHost()
    try {
      await sendMessageFromHost!({ content: 'hello', agentMode: 'DEFAULT' })

      const messages = useChatStore.getState().messages
      expect(
        messages.some((m) => m.variant === 'user' && m.content === 'hello'),
      ).toBe(true)
      const ai = messages.at(-1)!
      expect(ai.variant).toBe('ai')
      expect(ai.isComplete).toBe(true)
      expect(ai.userError).toContain(new EngineNotConnectedError().message)
      expect(useChatStore.getState().isChainInProgress).toBe(false)
      expect(savedMessages().at(-1).userError).toContain('not running')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('a turn submits the prompt and completes at turn_finished', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({
      content: 'fix the test',
      agentMode: 'DEFAULT',
    })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      expect(fake.submits[0]).toEqual({ text: 'fix the test', attachments: [] })
      expect(useChatStore.getState().isChainInProgress).toBe(true)

      fake.emit({ type: 'turn_finished' })
      await run

      const ai = useChatStore.getState().messages.at(-1)!
      expect(ai.isComplete).toBe(true)
      expect(ai.userError).toBeUndefined()
      expect(useChatStore.getState().runState?.output?.type).toBe('lastMessage')
      expect(useChatStore.getState().isChainInProgress).toBe(false)
    } finally {
      fake.emit({ type: 'turn_finished' })
      await run
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('an engine error event is shown when its turn finishes', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'go', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      fake.emit({ type: 'error', message: 'provider rejected the request' })
      fake.emit({ type: 'turn_finished' })
      await run

      expect(useChatStore.getState().messages.at(-1)!.userError).toBe(
        'provider rejected the request',
      )
      expect(useChatStore.getState().runState?.output).toEqual({
        type: 'error',
        message: 'provider rejected the request',
      })
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('engine events become text and tool blocks the renderers know', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'run it', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      // The engine's own lines for the approved-bash scenario.
      fake.emit({
        type: 'tool_call_started',
        id: 'toolu_bash_prompt_allow',
        name: 'bash',
        input: JSON.stringify({
          command: "printf 'approved via prompt' | tee approved.txt",
          timeout: 1000,
        }),
        cut_off: false,
      })
      fake.emit({
        type: 'tool_result',
        name: 'bash',
        output: JSON.stringify({ stdout: 'approved via prompt', stderr: '', interrupted: false }),
        is_error: false,
      })
      fake.emit({ type: 'text_delta', text: 'bash approved and executed' })
      fake.emit({ type: 'text_block_end' })
      fake.emit({ type: 'turn_finished' })
      await run

      const blocks = useChatStore.getState().messages.at(-1)!.blocks ?? []
      const tool = blocks.find((block) => block.type === 'tool') as any
      expect(tool.toolName).toBe('run_terminal_command')
      expect(tool.toolCallId).toBe('toolu_bash_prompt_allow')
      expect(tool.input.command).toBe("printf 'approved via prompt' | tee approved.txt")
      expect(tool.output).toBe('approved via prompt')
      const text = blocks.find((block) => block.type === 'text') as any
      expect(text.content).toBe('bash approved and executed')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('results with ids reach their own call, in any order', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'two', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      for (const id of ['call-a', 'call-b']) {
        fake.emit({
          type: 'tool_call_started',
          id,
          name: 'bash',
          input: JSON.stringify({ command: `echo ${id}` }),
          cut_off: false,
        })
      }
      for (const id of ['call-b', 'call-a']) {
        fake.emit({
          type: 'tool_result',
          id,
          name: 'bash',
          output: JSON.stringify({ stdout: `out ${id}`, stderr: '' }),
          is_error: false,
        })
      }
      fake.emit({ type: 'turn_finished' })
      await run

      const tools = (useChatStore.getState().messages.at(-1)!.blocks ?? []).filter(
        (block) => block.type === 'tool',
      ) as any[]
      expect(tools.map((tool) => [tool.toolCallId, tool.output])).toEqual([
        ['call-a', 'out call-a'],
        ['call-b', 'out call-b'],
      ])
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('a call the engine never answers (denied) is closed when the turn ends', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'try', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      fake.emit({
        type: 'tool_call_started',
        id: 'toolu_bash_prompt_deny',
        name: 'bash',
        input: '{"command":"printf x | tee denied.txt"}',
        cut_off: false,
      })
      fake.emit({ type: 'turn_finished' })
      await run

      const tool = (useChatStore.getState().messages.at(-1)!.blocks ?? []).find(
        (block) => block.type === 'tool',
      ) as any
      expect(tool.output).toContain('No result')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('stopping the run interrupts the engine turn', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({
      content: 'long task',
      agentMode: 'DEFAULT',
    })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      stopActiveRun('user-interrupt')
      expect(fake.interrupts).toBe(1)
      expect(useChatStore.getState().isChainInProgress).toBe(false)
    } finally {
      fake.emit({ type: 'turn_finished' })
      await run
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })
})

describe('an interrupted turn', () => {
  test('a chat whose only turn was interrupted still resumes the engine session on --continue', async () => {
    const fake = new FakeEngine()
    engine = fake
    // The engine named its session before the prompt (the status after `ready`).
    useEngineStore.getState().setStatus(engineStatus('session-42'))
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'run the slow check', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      // Ctrl-C stops the turn before it completes; then the UI quits.
      stopActiveRun('user-interrupt')
      fake.emit({ type: 'turn_finished' })
      await run
      await settleCheckpointSave()
      flushLiveChatState()

      // Was {"output":{"type":"error","message":"The session ended ..."}} with
      // no session, and `--continue` started a fresh engine session.
      const saved = savedRunState()
      expect(saved.sessionId).toBe('session-42')
      expect(saved.model).toBe('nvidia/stepfun-ai/step-5-preview')
      expect(resumeSessionFor()).toBe('session-42')

      // A later save without the session (an error output) keeps it.
      saveChatState(
        {
          output: {
            type: 'error',
            message: 'The session ended before this response completed.',
          },
        },
        useChatStore.getState().messages,
        testRoot,
      )
      expect(savedRunState().output.type).toBe('error')
      expect(resumeSessionFor()).toBe('session-42')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('a session named after the prompt is written into the saved chat at once', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'hello', agentMode: 'DEFAULT' })
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      // Saved at the turn's start, before the engine named its session.
      expect(savedRunState().sessionId).toBeUndefined()
      handleEngineEvent(engineStatus('session-7', 'groq/openai/gpt-oss-20b'))
      expect(savedRunState().sessionId).toBe('session-7')
      expect(savedRunState().model).toBe('groq/openai/gpt-oss-20b')
      // The engine's signed-out stage has no session: nothing is written.
      handleEngineEvent(engineStatus(''))
      expect(savedRunState().sessionId).toBe('session-7')
    } finally {
      stopActiveRun('user-interrupt')
      fake.emit({ type: 'turn_finished' })
      await run
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('the running command says it was stopped, and its late result still reaches its row', async () => {
    const fake = new FakeEngine()
    engine = fake
    const { setup, root } = await mountHost()
    const run = sendMessageFromHost!({ content: 'run it', agentMode: 'DEFAULT' })
    const tool = () =>
      (useChatStore.getState().messages.at(-1)!.blocks ?? []).find(
        (block) => block.type === 'tool',
      ) as any
    try {
      await waitFor('submit', () => fake.submits.length === 1)
      fake.emit({
        type: 'tool_call_started',
        id: 'toolu_slow',
        name: 'bash',
        input: JSON.stringify({ command: 'npm run slow-check' }),
        cut_off: false,
      })
      await waitFor('tool row', () => tool() !== undefined)
      stopActiveRun('user-interrupt')
      await waitFor('interrupted row', () => tool()?.stopReason === 'interrupted')
      expect(tool().output).toBeUndefined()

      // The engine's result for the cancelled command (runtime/src/bash.rs).
      fake.emit({
        type: 'tool_result',
        id: 'toolu_slow',
        name: 'bash',
        output: JSON.stringify({
          stdout: 'slow check started, pid 5184\n',
          stderr: 'Command was cancelled; the process tree was terminated',
          interrupted: true,
          returnCodeInterpretation: 'cancelled',
        }),
        is_error: true,
      })
      fake.emit({ type: 'turn_finished' })
      await run

      await waitFor('late result', () => tool()?.output !== undefined)
      expect(tool().output).toContain('slow check started')
      expect(tool().stopReason).toBe('interrupted')
      expect(tool().output).not.toContain('No result')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })
})
