/**
 * UI-flow eval: one scripted session through the chat as the user sees it.
 *
 *   prompt -> streamed tool call cell -> approval prompt -> `y` -> diff and
 *   changed-files line -> final answer
 *
 * A scripted engine stands in for `neuron engine --stdio`: it plays the
 * events a live_progress engine sends for an `edit_file` call that needs
 * approval, waits for the UI's permission reply, and plays the rest of the turn
 * according to that reply. Everything between the engine and the screen is the
 * real UI code: `useSendMessage` (what the composer's Enter calls), the turn
 * event sink, `handleEngineEvent` (the app's engine-wide listener), the stores,
 * and the components chat.tsx draws (`MessageBlock`, `PermissionPrompt`,
 * `ChangesSummary`), rendered to text frames with OpenTUI's test renderer. The
 * approval is a real key press on that renderer.
 *
 * Deterministic and keyless (no engine binary, no network): it runs in
 * `bun run test`, and alone with `bun run eval:ui`.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React, { useEffect, useRef } from 'react'

import { ChangesSummary } from '../src/components/changes-panel'
import { MessageBlock } from '../src/components/message-block'
import { PERMISSION_KEY_GUARD_MS, PermissionPrompt } from '../src/components/permission-prompt'
import { handleEngineEvent, setEngineTurnActive } from '../src/engine/engine-session'
import { resetGitViews } from '../src/engine/git-views'
import {
  setNeuronEngineClientForTesting,
  type EngineExit,
  type NeuronEngineClient,
} from '../src/engine/neuron-engine-client'
import { initializeThemeStore, useThemeStore } from '../src/hooks/use-theme'
import { useSendMessage } from '../src/hooks/use-send-message'
import { setProjectRoot, tryGetProjectRoot } from '../src/project-files'
import { useChatStore } from '../src/state/chat-store'
import { useEngineStore } from '../src/state/engine-store'
import { useGitViewsStore } from '../src/state/git-views-store'
import { resetLiveTurnStore } from '../src/state/live-turn-store'
import { stopActiveRun } from '../src/utils/active-run'
import { setChatDirOverrideForTesting } from '../src/utils/run-state-storage'
import { createMarkdownPalette } from '../src/utils/theme-system'

import type {
  Attachment,
  EngineCommand,
  PermissionDecision,
  PermissionReply,
  ReadyEvent,
  StatusEvent,
  UiEvent,
} from '../src/engine/protocol'
import type { SendMessageFn } from '../src/types/contracts/send-message'
import type { ElapsedTimeTracker } from '../src/hooks/use-elapsed-time'

const WIDTH = 100
const HEIGHT = 60
const PATH = 'src/greet.py'
const TOOL_ID = 'toolu_edit_1'
const PERMISSION_ID = 7
const OLD_LINE = '    return "Hello " + nam'
const NEW_LINE = '    return f"Hello, {name}!"'
const FINAL_ANSWER = 'Fixed the greeting in src/greet.py: greet("Ada") now returns Hello, Ada!'
const DENIED_ANSWER = 'Understood: I left src/greet.py unchanged.'
/** Each flow takes about a second; the waits inside give up after 4 s with the last frame. */
const FLOW_TIMEOUT_MS = 20_000

/** A step of the script: an event, or a point where the engine waits for the test. */
type Step = UiEvent | { pause: string }

const STATUS: StatusEvent = {
  type: 'status',
  model: 'auto',
  permission_mode: 'read-only',
  session_id: 'ui-flow-session',
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 3,
  git: { branch: 'main', changed: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
  signed_in: true,
}

const EDIT_INPUT = JSON.stringify({ path: PATH, old_string: OLD_LINE, new_string: NEW_LINE })

/** What the engine sends after `submit`, up to its permission request. */
function untilApproval(): Step[] {
  return [
    { type: 'text_delta', text: "The greeting drops the comma and a letter. I'll fix " },
    { type: 'text_delta', text: '`greet`.' },
    { type: 'text_block_end' },
    { type: 'output_progress', output_tokens: 14 },
    { type: 'tool_input_started', id: TOOL_ID, name: 'edit_file' },
    {
      type: 'tool_input_progress',
      id: TOOL_ID,
      name: 'edit_file',
      bytes: 48,
      lines: 1,
      target: PATH,
      preview: 'return f"Hello',
    },
    { pause: 'input-streaming' },
    {
      type: 'tool_input_progress',
      id: TOOL_ID,
      name: 'edit_file',
      bytes: EDIT_INPUT.length,
      lines: 1,
      target: PATH,
      preview: NEW_LINE.trim(),
    },
    { type: 'tool_call_started', id: TOOL_ID, name: 'edit_file', input: EDIT_INPUT, cut_off: false },
    {
      type: 'request_usage',
      usage: { input: 1200, output: 64, cache_read: 0, cache_write: 0 },
    },
    {
      type: 'permission_requested',
      id: PERMISSION_ID,
      request: {
        tool: 'edit_file',
        current_mode: 'read-only',
        required_mode: 'workspace-write',
        reason: "tool 'edit_file' requires workspace-write",
        input: EDIT_INPUT,
        session_prefixes: [],
      },
    },
  ]
}

/** The rest of the turn, by the UI's answer. */
function afterReply(decision: PermissionDecision): Step[] {
  const usage: Step = {
    type: 'usage',
    turn: { input: 2500, output: 120, cache_read: 0, cache_write: 0 },
    cumulative: { input: 2500, output: 120, cache_read: 0, cache_write: 0 },
    estimated: false,
  }
  if (decision.kind === 'deny') {
    return [
      {
        type: 'tool_result',
        id: TOOL_ID,
        name: 'edit_file',
        output: 'The user denied this edit.',
        is_error: true,
      },
      { type: 'text_delta', text: DENIED_ANSWER },
      { type: 'text_block_end' },
      usage,
      { ...STATUS },
      { type: 'turn_finished' },
    ]
  }
  return [
    {
      type: 'tool_result',
      id: TOOL_ID,
      name: 'edit_file',
      output: JSON.stringify({
        filePath: `/work/${PATH}`,
        diff: `@@ -1,2 +1,2 @@\n def greet(name):\n-${OLD_LINE}\n+${NEW_LINE}`,
      }),
      is_error: false,
    },
    { type: 'changes', files: [{ path: PATH, status: 'modified', added: 1, removed: 1 }] },
    { type: 'text_delta', text: FINAL_ANSWER },
    { type: 'text_block_end' },
    usage,
    { ...STATUS, git: { ...STATUS.git!, changed: 1, unstaged: 1 } },
    { type: 'turn_finished' },
  ]
}

/**
 * Plays the script, one event per tick (as engine lines arrive), stopping at
 * a pause until the test resumes it and at a permission request until the UI
 * replies.
 */
class ScriptedEngine implements NeuronEngineClient {
  readonly connection = 'ready' as const
  submits: string[] = []
  replies: PermissionReply[] = []
  pausedAt: string | null = null
  done = false
  private listeners = new Set<(event: UiEvent) => void>()
  private queue: Step[] = []
  private resumeWaiter: (() => void) | null = null

  start(): Promise<ReadyEvent> {
    return Promise.resolve({ type: 'ready', protocol: 1, version: 'ui-flow', features: ['live_progress'] })
  }
  send(_command: EngineCommand): void {}
  submit(text: string, _attachments: Attachment[] = []): void {
    this.submits.push(text)
    this.play(untilApproval())
  }
  slash(): void {}
  setModel(): void {}
  listModels(): void {}
  interrupt(): void {}
  replyToPermission(reply: PermissionReply): void {
    this.replies.push(reply)
    if (reply.id === PERMISSION_ID) this.play(afterReply(reply.decision))
  }
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
  resume(): void {
    this.pausedAt = null
    this.resumeWaiter?.()
  }

  private play(steps: Step[]): void {
    const idle = this.queue.length === 0
    this.queue.push(...steps)
    if (idle) void this.drain()
  }

  private async drain(): Promise<void> {
    while (this.queue.length > 0) {
      await new Promise((resolve) => setTimeout(resolve, 1))
      const step = this.queue.shift()!
      if ('pause' in step) {
        this.pausedAt = step.pause
        await new Promise<void>((resolve) => {
          this.resumeWaiter = resolve
        })
        continue
      }
      for (const listener of [...this.listeners]) listener(step)
      if (step.type === 'turn_finished') this.done = true
    }
  }
}

let engine: ScriptedEngine
let sendMessage: SendMessageFn | null = null
let chatDir: string
let originalProjectRoot: string

const timer = (): ElapsedTimeTracker => ({
  start: () => {},
  stop: () => {},
  pause: () => {},
  resume: () => {},
  elapsedSeconds: 0,
  startTime: null,
  isPaused: false,
})

/** The chat, its permission prompt and the changes line, as chat.tsx stacks them. */
function Screen() {
  const inputRef = useRef<any>(null)
  const activeSubagentsRef = useRef(new Set<string>())
  const isChainInProgressRef = useRef(false)
  const isQueuePausedRef = useRef(false)
  const isProcessingQueueRef = useRef(false)
  const mainAgentTimer = useRef(timer()).current
  const send = useSendMessage({
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
  }).sendMessage
  useEffect(() => {
    sendMessage = send
  }, [send])

  const messages = useChatStore((state) => state.messages)
  const streamingAgents = useChatStore((state) => state.streamingAgents)
  const permission = useEngineStore((state) => state.permissionQueue[0] ?? null)
  const changes = useGitViewsStore((state) => state.changes)
  const theme = useThemeStore((state) => state.theme)
  const palette = createMarkdownPalette(theme)

  return (
    <box style={{ flexDirection: 'column', width: WIDTH }}>
      {messages.map((message) => (
        <MessageBlock
          key={message.id}
          messageId={message.id}
          blocks={message.blocks}
          content={message.content}
          isUser={message.variant === 'user'}
          isAi={message.variant === 'ai'}
          isLoading={message.variant === 'ai' && !message.isComplete}
          timestamp={message.timestamp}
          isComplete={message.isComplete}
          completionTime={message.completionTime}
          timerStartTime={null}
          textColor={theme.foreground}
          timestampColor={theme.muted}
          markdownOptions={{ codeBlockWidth: WIDTH - 8, palette }}
          availableWidth={WIDTH}
          markdownPalette={palette}
          onToggleCollapsed={() => {}}
          onBuildFast={() => {}}
          onBuildMax={() => {}}
          onBuildLite={() => {}}
          userError={message.userError}
          isLastMessage={message === messages.at(-1)}
        />
      ))}
      {permission ? (
        <PermissionPrompt request={permission} width={WIDTH} />
      ) : (
        <ChangesSummary files={changes} />
      )}
      <text>{`streaming rows: ${streamingAgents.size}`}</text>
    </box>
  )
}

async function mount() {
  const setup = await createTestRenderer({ width: WIDTH, height: HEIGHT })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<Screen />))
  await setup.renderOnce()
  const frame = async () => {
    await setup.renderOnce()
    await new Promise((resolve) => setTimeout(resolve, 5))
    await setup.renderOnce()
    return setup.captureCharFrame()
  }
  /** Renders until the frame satisfies `ready` (UI updates are throttled), then returns it. */
  const frameWhen = async (label: string, ready: (frame: string) => boolean, timeoutMs = 4_000) => {
    const deadline = Date.now() + timeoutMs
    let last = ''
    while (Date.now() < deadline) {
      last = await frame()
      if (ready(last)) return last
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    const shown = last.replace(/ +\n/g, '\n').trimEnd()
    throw new Error(`Timed out waiting for ${label}; last frame:\n${shown}`)
  }
  const unmount = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  return { setup, frame, frameWhen, unmount }
}

async function waitFor(label: string, predicate: () => boolean, timeoutMs = 4_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  throw new Error(`Timed out waiting for ${label}`)
}

beforeAll(() => initializeThemeStore())

let unsubscribeSession: (() => void) | null = null

beforeEach(() => {
  engine = new ScriptedEngine()
  sendMessage = null
  // The app's engine-wide listener, as startEngineSession installs it; the
  // permission prompt answers through the process-wide client.
  unsubscribeSession = engine.subscribe(handleEngineEvent)
  setNeuronEngineClientForTesting(engine)
  chatDir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-ui-flow-'))
  originalProjectRoot = tryGetProjectRoot() ?? process.cwd()
  setProjectRoot(process.cwd())
  setChatDirOverrideForTesting(chatDir)
  useChatStore.getState().reset()
  useEngineStore.getState().reset()
  resetGitViews()
  resetLiveTurnStore()
  setEngineTurnActive(false)
  handleEngineEvent({ type: 'ready', protocol: 1, version: 'ui-flow', features: ['live_progress'] })
  handleEngineEvent(STATUS)
})

afterEach(() => {
  unsubscribeSession?.()
  stopActiveRun('process-exit')
  setNeuronEngineClientForTesting(null)
  useEngineStore.getState().reset()
  resetGitViews()
  resetLiveTurnStore()
  setChatDirOverrideForTesting(undefined)
  setProjectRoot(originalProjectRoot)
  fs.rmSync(chatDir, { recursive: true, force: true })
})

/** Sends the prompt and plays the turn up to the waiting permission prompt. */
async function upToApproval(ui: Awaited<ReturnType<typeof mount>>) {
  await waitFor('the send hook', () => sendMessage !== null)
  const turn = sendMessage!({ content: 'Fix the greeting in src/greet.py', agentMode: 'DEFAULT' })
  await waitFor('submit', () => engine.submits.length === 1)
  expect(engine.submits).toEqual(['Fix the greeting in src/greet.py'])

  // 1. The model is still writing the call: its cell is already there, live.
  await waitFor('the streaming input', () => engine.pausedAt === 'input-streaming')
  const streaming = await ui.frameWhen('the live cell', (f) => f.includes('return f"Hello') && f.includes("I'll fix"))
  expect(streaming).toContain('Fix the greeting in src/greet.py')
  expect(streaming).toContain("I'll fix")
  expect(streaming).toMatch(/Edit src\/greet\.py\s+1 line · 48 B/)
  expect(streaming).toContain('return f"Hello')
  expect(streaming).toContain('streaming rows: 1')
  expect(streaming).not.toContain('Permission needed')

  // 2. The complete call waits for approval: the prompt takes the composer's place.
  engine.resume()
  await waitFor('the permission request', () => useEngineStore.getState().permissionQueue.length === 1)
  const asking = await ui.frameWhen('the permission prompt', (f) => f.includes('Permission needed'))
  expect(asking).toContain('Permission needed')
  expect(asking).toContain('edit_file wants approval (read-only does not cover this call)')
  expect(asking).toContain('d deny for session')
  expect(asking).toContain(PATH)
  expect(asking).toMatch(/y\s+allow/)
  expect(useChatStore.getState().messages.at(-1)!.isComplete).toBeFalsy()
  // Wrapped: an async function returning the promise itself would wait for it.
  return { turn }
}

describe('UI flow: prompt, streamed tool call, approval, diff, answer', () => {
  test('approving the edit shows its diff, the changed file and the final answer', async () => {
    const ui = await mount()
    try {
      const { turn } = await upToApproval(ui)

      // A key already on its way is not an answer: the guard holds it.
      ui.setup.mockInput.pressKey('y')
      await ui.frame()
      expect(engine.replies).toEqual([])

      // 3. `y` once the guard is over: allow once, and the turn goes on.
      await new Promise((resolve) => setTimeout(resolve, PERMISSION_KEY_GUARD_MS + 50))
      await ui.frame()
      ui.setup.mockInput.pressKey('y')
      await waitFor('the reply', () => engine.replies.length === 1)
      expect(engine.replies).toEqual([{ id: PERMISSION_ID, decision: { kind: 'allow_once' } }])

      await waitFor('turn_finished', () => engine.done)
      await turn

      // 4. The diff in the tool cell, the changed-files line, the final answer.
      const finished = await ui.frameWhen('the final answer', (f) => f.includes(FINAL_ANSWER))
      expect(finished).not.toContain('Permission needed')
      expect(finished).toContain('Edit')
      expect(finished).toContain(PATH)
      expect(finished).toContain(NEW_LINE.trim())
      expect(finished).toContain('1 file changed +1 -1 · /changes')
      expect(finished).toContain(FINAL_ANSWER)
      expect(finished).toContain('streaming rows: 0')
      // The answer comes after the cell, as it came after the call.
      expect(finished.indexOf(FINAL_ANSWER)).toBeGreaterThan(finished.indexOf(NEW_LINE.trim()))

      const ai = useChatStore.getState().messages.at(-1)!
      expect(ai.variant).toBe('ai')
      expect(ai.isComplete).toBe(true)
      expect(ai.userError).toBeUndefined()
      const tools = (ai.blocks ?? []).filter((block) => block.type === 'tool')
      expect(tools).toHaveLength(1)
      expect(useEngineStore.getState().permissionQueue).toEqual([])
      expect(useEngineStore.getState().usage?.cumulative.output).toBe(120)
      expect(useEngineStore.getState().status?.git?.changed).toBe(1)
      expect(useChatStore.getState().isChainInProgress).toBe(false)
    } finally {
      ui.unmount()
    }
  }, FLOW_TIMEOUT_MS)

  test('denying the edit changes nothing and the answer says so', async () => {
    const ui = await mount()
    try {
      const { turn } = await upToApproval(ui)
      await new Promise((resolve) => setTimeout(resolve, PERMISSION_KEY_GUARD_MS + 50))
      await ui.frame()
      ui.setup.mockInput.pressKey('n')
      await waitFor('the reply', () => engine.replies.length === 1)
      expect(engine.replies).toEqual([
        { id: PERMISSION_ID, decision: { kind: 'deny', note: null } },
      ])

      await waitFor('turn_finished', () => engine.done)
      await turn

      const finished = await ui.frameWhen('the final answer', (f) => f.includes(DENIED_ANSWER))
      expect(finished).not.toContain('Permission needed')
      expect(finished).not.toContain('file changed')
      expect(finished).toContain(DENIED_ANSWER)
      expect(useGitViewsStore.getState().changes).toEqual([])
      expect(useChatStore.getState().messages.at(-1)!.isComplete).toBe(true)
    } finally {
      ui.unmount()
    }
  }, FLOW_TIMEOUT_MS)
})
