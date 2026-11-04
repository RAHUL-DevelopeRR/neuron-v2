/**
 * A reproducible measure of the chat while a turn streams: the transcript
 * (as chat.tsx draws it, through useChatMessages and MessageWithAgents), the
 * status line above the input (StatusBar with the live activity) and the
 * prompt status bar under it, fed by the turn's real event sink
 * (engine/turn-events.ts) and the batched message updater send-message.ts
 * uses. The engine side is replayed: `tokens` text deltas, a tool call
 * (written, run, answered) every `toolEvery` tokens, and the engine's
 * throttled `output_progress`.
 *
 * It counts how often the busiest components draw (utils/render-probe.ts),
 * the transcript's updates, React's render time per region (Profiler), times
 * each frame the renderer draws (at most 60 a second, as the real renderer),
 * the event path (the sink's `handle`) and how late a 1 ms timer fires (the
 * event loop's lag: what typing feels like meanwhile).
 *
 * `bun run bench:streaming` prints the numbers; streaming-render-budget.test
 * keeps the render counts in check.
 */

import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React, { Profiler, useLayoutEffect, type ProfilerOnRenderCallback } from 'react'

import { MessageWithAgents } from '../components/message-with-agents'
import { PromptStatusBar } from '../components/prompt-status-bar'
import { StatusBar } from '../components/status-bar'
import { createTurnEventSink } from '../engine/turn-events'
import { useChatMessages } from '../hooks/use-chat-messages'
import { initializeThemeStore, useThemeStore } from '../hooks/use-theme'
import { useChatStore } from '../state/chat-store'
import { useEngineStore } from '../state/engine-store'
import { resetLiveTurnStore } from '../state/live-turn-store'
import { useMessageBlockStore } from '../state/message-block-store'
import { renderProbe } from '../utils/render-probe'
import {
  createBatchedMessageUpdater,
  createMessageUpdater,
  type MessageUpdater,
} from '../utils/message-updater'
import { createMarkdownPalette } from '../utils/theme-system'
import { STATUS_BAR_ACCOUNT, STATUS_BAR_MODELS, STATUS_BAR_STATUS } from './status-bar-fixtures'

import type { UiEvent } from '../engine/protocol'
import type { ChatMessage } from '../types/chat'
import type { EventHandlerState } from '../utils/sdk-event-handlers'
import type { StatusIndicatorState } from '../utils/status-indicator-state'
import type { Logger } from '@neuron-tui/common/types/contracts/logger'

export interface StreamingBenchOptions {
  /** Text deltas the turn streams. */
  tokens?: number
  /** A tool call every this many tokens (0: none). */
  toolEvery?: number
  /** Earlier turns in the transcript (a user and an AI message each). */
  historyTurns?: number
  /** Milliseconds between two text deltas. */
  tokenIntervalMs?: number
  width?: number
  height?: number
}

export interface Timings {
  count: number
  totalMs: number
  meanMs: number
  p95Ms: number
  maxMs: number
}

export interface StreamingBenchResult {
  tokens: number
  toolCalls: number
  engineEvents: number
  wallMs: number
  /** CPU time this process used meanwhile (user and system): steadier than wall times on a busy machine. */
  cpuMs: number
  /** Times the chat's message list changed (the batched updater's flushes and the end). */
  transcriptUpdates: number
  /** How often each component drew while the turn streamed. */
  renders: {
    /** Earlier, finished messages drawn again. */
    oldMessages: number
    streamingMessage: number
    statusBar: number
    liveActivityLine: number
    promptStatusBar: number
  }
  /** React render work (Profiler actualDuration) per region. */
  reactMs: { transcript: number; statusLine: number; promptStatusBar: number }
  /** The renderer's frames (layout and draw). */
  frames: Timings
  /** The sink's `handle` per engine event. */
  eventPath: Timings
  /** How late a 1 ms timer fired. */
  eventLoopLag: Timings
}

function timings(samples: number[]): Timings {
  const sorted = [...samples].sort((a, b) => a - b)
  const total = sorted.reduce((sum, value) => sum + value, 0)
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? 0
  return {
    count: sorted.length,
    totalMs: round(total),
    meanMs: round(sorted.length > 0 ? total / sorted.length : 0),
    p95Ms: round(at(0.95)),
    maxMs: round(sorted[sorted.length - 1] ?? 0),
  }
}

const round = (value: number) => Math.round(value * 100) / 100

const noop = () => {}
const silentLogger = { info: noop, warn: noop, error: noop, debug: noop } as unknown as Logger

/** The sink's view of the chat, as use-send-message wires it. */
function handlerState(aiMessageId: string, updater: MessageUpdater): EventHandlerState {
  const streamState = {
    rootStreamBuffer: '',
    agentStreamAccumulators: new Map<string, string>(),
    rootStreamSeen: false,
    planExtracted: false,
    wasAbortedByUser: false,
    spawnAgentsMap: new Map(),
  }
  const chat = useChatStore.getState()
  return {
    isActive: () => true,
    streaming: {
      streamRefs: {
        state: streamState,
        reset: noop,
        setters: {
          setRootStreamBuffer: (value: string) => {
            streamState.rootStreamBuffer = value
          },
          appendRootStreamBuffer: (value: string) => {
            streamState.rootStreamBuffer += value
          },
          setAgentAccumulator: noop,
          removeAgentAccumulator: noop,
          setRootStreamSeen: noop,
          setPlanExtracted: noop,
          setWasAbortedByUser: noop,
          setSpawnAgentInfo: noop,
          removeSpawnAgentInfo: noop,
        },
      },
      setStreamingAgents: chat.setStreamingAgents,
      setStreamStatus: noop,
    },
    message: {
      aiMessageId,
      updater,
      hasReceivedContentRef: { current: false },
    },
    subagents: { addActiveSubagent: noop, removeActiveSubagent: noop },
    mode: { agentMode: 'DEFAULT', setHasReceivedPlanResponse: noop },
    logger: silentLogger,
    setIsRetrying: chat.setIsRetrying,
  } as unknown as EventHandlerState
}

const WORDS = [
  'the', 'cart', 'total', 'renders', 'once', 'per', 'update,', 'so', 'we', 'keep',
  'state', 'in', 'one', 'store', 'and', '`useCart()`', 'reads', 'it.', '**Note:**', 'tests',
]

/** The text the model streams: paragraphs, a list, now and then a code block. */
export function benchTokens(count: number): string[] {
  const tokens: string[] = []
  for (let index = 0; tokens.length < count; index++) {
    const position = index % 120
    if (position === 0 && index > 0) tokens.push('\n\n')
    else if (position === 40) tokens.push('\n\n- ')
    else if (position === 50 || position === 60) tokens.push('\n- ')
    else if (position === 80) tokens.push('\n\n```ts\nconst total = items')
    else if (position === 90) tokens.push('.reduce((sum, item) => sum + item.price, 0)\n```\n\n')
    else tokens.push(`${WORDS[index % WORDS.length]} `)
  }
  return tokens
}

/** One tool call as the engine streams it: named, written, started, answered. */
function toolCallEvents(n: number): UiEvent[] {
  const id = `call_${n}`
  const path = `grocery-shop/src/part-${n}.ts`
  const content = Array.from({ length: 40 }, (_, line) => `export const value${line} = ${line}`).join('\n')
  const events: UiEvent[] = [{ type: 'tool_input_started', id, name: 'write_file' } as UiEvent]
  for (let step = 1; step <= 4; step++) {
    events.push({
      type: 'tool_input_progress',
      id,
      name: 'write_file',
      bytes: step * 300,
      lines: step * 10,
      target: path,
      preview: content.split('\n').slice(0, step * 10).slice(-4).join('\n'),
    } as UiEvent)
  }
  events.push({
    type: 'tool_call_started',
    id,
    name: 'write_file',
    input: JSON.stringify({ path, content }),
  } as UiEvent)
  events.push({
    type: 'tool_result',
    id,
    name: 'write_file',
    output: JSON.stringify({ type: 'create', filePath: path, bytes: content.length, lines: 40, summary: '' }),
    is_error: false,
  } as UiEvent)
  return events
}

/** The whole turn's events, in order. */
export function benchEvents(tokens: number, toolEvery: number): UiEvent[] {
  const events: UiEvent[] = []
  let calls = 0
  benchTokens(tokens).forEach((text, index) => {
    events.push({ type: 'text_delta', text } as UiEvent)
    // The engine sends its estimate at most every 250 ms: about every 25 tokens here.
    if (index % 25 === 24) events.push({ type: 'output_progress', output_tokens: index + 1 } as UiEvent)
    if (toolEvery > 0 && index % toolEvery === toolEvery - 1) {
      events.push({ type: 'text_block_end' } as UiEvent)
      events.push(...toolCallEvents(++calls))
    }
  })
  events.push({ type: 'text_block_end' } as UiEvent)
  return events
}

/** Earlier turns, streamed through the same sink and finished. */
function historyMessages(turns: number): ChatMessage[] {
  let messages: ChatMessage[] = []
  const setMessages = (update: (prev: ChatMessage[]) => ChatMessage[]) => {
    messages = update(messages)
  }
  for (let turn = 0; turn < turns; turn++) {
    const aiId = `ai-history-${turn}`
    messages.push({ id: `user-history-${turn}`, variant: 'user', content: `Make part ${turn} of the shop.`, timestamp: 'earlier' })
    messages.push({ id: aiId, variant: 'ai', content: '', blocks: [], timestamp: 'earlier' })
    const updater = createMessageUpdater(aiId, setMessages)
    const sink = createTurnEventSink(handlerState(aiId, updater))
    for (const event of benchEvents(300, 150)) sink.handle(event)
    sink.finish()
    updater.markComplete()
  }
  resetLiveTurnStore()
  return messages
}

/** Chat's transcript and the lines around the input, as chat.tsx lays them out. */
function BenchChat({
  width,
  onRender,
}: {
  width: number
  onRender: ProfilerOnRenderCallback
}) {
  const messages = useChatStore((state) => state.messages)
  const setMessages = useChatStore((state) => state.setMessages)
  const { messageTree, visibleTopLevelMessages } = useChatMessages({ messages, setMessages })
  const theme = useThemeStore((state) => state.theme)
  const setContext = useMessageBlockStore((state) => state.setContext)
  const markdownPalette = React.useMemo(() => createMarkdownPalette(theme), [theme])
  const availableWidth = width - 3
  useLayoutEffect(() => {
    setContext({
      readOnly: false,
      theme,
      markdownPalette,
      messageTree,
      isWaitingForResponse: false,
      timerStartTime: null,
      availableWidth,
    })
  }, [theme, markdownPalette, messageTree, availableWidth, setContext])

  return (
    <box style={{ flexDirection: 'column', width, flexGrow: 1 }}>
      <scrollbox style={{ flexGrow: 1 }} stickyScroll stickyStart="bottom">
        <Profiler id="transcript" onRender={onRender}>
          {visibleTopLevelMessages.map((message, index) => (
            <MessageWithAgents
              key={message.id}
              message={message}
              depth={0}
              isLastMessage={index === visibleTopLevelMessages.length - 1}
              availableWidth={availableWidth}
            />
          ))}
        </Profiler>
      </scrollbox>
      <Profiler id="statusLine" onRender={onRender}>
        <StatusBar
          timerStartTime={null}
          isAtBottom
          scrollToLatest={noop}
          // A new object on each render, as chat.tsx makes it.
          statusIndicatorState={{ kind: 'streaming' } satisfies StatusIndicatorState}
          onStop={noop}
        />
      </Profiler>
      <Profiler id="promptStatusBar" onRender={onRender}>
        <PromptStatusBar width={width} onActivate={noop} glyphMode="unicode" />
      </Profiler>
    </box>
  )
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms))

export async function runStreamingBench(options: StreamingBenchOptions = {}): Promise<StreamingBenchResult> {
  const tokens = options.tokens ?? 2000
  const toolEvery = options.toolEvery ?? 250
  const tokenIntervalMs = options.tokenIntervalMs ?? 4
  const width = options.width ?? 120
  const height = options.height ?? 40

  initializeThemeStore()
  resetLiveTurnStore()
  useMessageBlockStore.getState().reset()
  const engine = useEngineStore.getState()
  engine.setStatus(STATUS_BAR_STATUS)
  engine.setModels(STATUS_BAR_MODELS)
  engine.setAccount(STATUS_BAR_ACCOUNT)

  const aiMessageId = 'ai-streaming'
  useChatStore.getState().setMessages([
    ...historyMessages(options.historyTurns ?? 7),
    { id: 'user-streaming', variant: 'user', content: 'Build the cart page.', timestamp: 'now' },
    { id: aiMessageId, variant: 'ai', content: '', blocks: [], timestamp: 'now' },
  ])

  const reactMs = { transcript: 0, statusLine: 0, promptStatusBar: 0 }
  const renders = {
    oldMessages: 0,
    streamingMessage: 0,
    statusBar: 0,
    liveActivityLine: 0,
    promptStatusBar: 0,
  }
  let transcriptUpdates = 0
  let measuring = false
  const onRender: ProfilerOnRenderCallback = (id, _phase, actualDuration) => {
    if (measuring) reactMs[id as keyof typeof reactMs] += actualDuration
  }
  renderProbe.onRender = (component, key) => {
    if (!measuring) return
    if (component === 'MessageWithAgents') {
      if (key === aiMessageId) renders.streamingMessage++
      else if (key !== 'user-streaming') renders.oldMessages++
    } else if (component === 'StatusBar') renders.statusBar++
    else if (component === 'LiveActivityLine') renders.liveActivityLine++
    else renders.promptStatusBar++
  }
  const unsubscribe = useChatStore.subscribe((state, previous) => {
    if (measuring && state.messages !== previous.messages) transcriptUpdates++
  })

  const setup = await createTestRenderer({ width, height })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<BenchChat width={width} onRender={onRender} />))
  await setup.renderOnce()

  const updater = createBatchedMessageUpdater(aiMessageId, useChatStore.getState().setMessages)
  const sink = createTurnEventSink(handlerState(aiMessageId, updater))
  const events = benchEvents(tokens, toolEvery)

  const frameSamples: number[] = []
  const eventSamples: number[] = []
  const lagSamples: number[] = []
  let running = true
  measuring = true

  // The renderer: at most 60 frames a second.
  const frames = (async () => {
    while (running) {
      const start = performance.now()
      await setup.renderOnce()
      frameSamples.push(performance.now() - start)
      await sleep(16)
    }
  })()
  // The event loop's lag.
  const lag = (async () => {
    while (running) {
      const start = performance.now()
      await sleep(1)
      lagSamples.push(Math.max(0, performance.now() - start - 1))
    }
  })()

  const started = performance.now()
  const cpuAtStart = process.cpuUsage()
  for (const event of events) {
    const start = performance.now()
    sink.handle(event)
    eventSamples.push(performance.now() - start)
    if (event.type === 'text_delta') await sleep(tokenIntervalMs)
  }
  sink.finish()
  updater.markComplete()
  await sleep(50)
  const wallMs = performance.now() - started
  const cpu = process.cpuUsage(cpuAtStart)
  running = false
  await Promise.all([frames, lag])
  measuring = false

  unsubscribe()
  renderProbe.onRender = null
  flushSync(() => root.unmount())
  setup.renderer.destroy()
  resetLiveTurnStore()
  useChatStore.getState().setMessages([])

  return {
    tokens,
    toolCalls: events.filter((event) => event.type === 'tool_call_started').length,
    engineEvents: events.length,
    wallMs: round(wallMs),
    cpuMs: round((cpu.user + cpu.system) / 1000),
    transcriptUpdates,
    renders,
    reactMs: {
      transcript: round(reactMs.transcript),
      statusLine: round(reactMs.statusLine),
      promptStatusBar: round(reactMs.promptStatusBar),
    },
    frames: timings(frameSamples),
    eventPath: timings(eventSamples),
    eventLoopLag: timings(lagSamples),
  }
}
