import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { createTurnEventSink } from '../../engine/turn-events'
import { initializeThemeStore, useThemeStore } from '../../hooks/use-theme'
import { createMessageUpdater } from '../../utils/message-updater'
import { createMarkdownPalette } from '../../utils/theme-system'
import { MessageBlock } from '../message-block'
import { RefusalBanner } from '../refusal-banner'

import type { UiEvent } from '../../engine/protocol'
import type { ChatMessage } from '../../types/chat'
import type { EventHandlerState } from '../../utils/sdk-event-handlers'
import type { Logger } from '@neuron-tui/common/types/contracts/logger'

beforeAll(() => {
  initializeThemeStore()
})

// The engine's refusal line for the gateway's note in the cyber category.
const REFUSAL =
  'The model declined to continue this request (safety category: cyber). Rephrase it or choose another model.'

/** The AI message a turn's engine events leave in the chat, as use-send-message wires it. */
function messageAfter(events: UiEvent[]): ChatMessage {
  let messages: ChatMessage[] = [
    { id: 'ai-1', variant: 'ai', content: '', blocks: [], timestamp: 'now' },
  ]
  const noop = () => {}
  const streamState = {
    rootStreamBuffer: '',
    agentStreamAccumulators: new Map<string, string>(),
    rootStreamSeen: false,
    planExtracted: false,
    wasAbortedByUser: false,
    spawnAgentsMap: new Map(),
  }
  const updater = createMessageUpdater('ai-1', (fn: (prev: ChatMessage[]) => ChatMessage[]) => {
    messages = fn(messages)
  })
  const state = {
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
      setStreamingAgents: noop,
      setStreamStatus: noop,
    },
    message: { aiMessageId: 'ai-1', updater, hasReceivedContentRef: { current: false } },
    subagents: { addActiveSubagent: noop, removeActiveSubagent: noop },
    mode: { agentMode: 'DEFAULT', setHasReceivedPlanResponse: noop },
    logger: { info: noop, warn: noop, error: noop, debug: noop } as unknown as Logger,
    setIsRetrying: noop,
  } as unknown as EventHandlerState
  const sink = createTurnEventSink(state)
  for (const event of events) sink.handle(event)
  sink.finish()
  updater.markComplete()
  return messages[0]!
}

/** The message as the chat draws it, as text. */
async function frame(message: ChatMessage, height = 16): Promise<string> {
  const theme = useThemeStore.getState().theme
  const palette = createMarkdownPalette(theme)
  const setup = await createTestRenderer({ width: 80, height })
  const root = createRoot(setup.renderer)
  const noop = () => {}
  flushSync(() => {
    root.render(
      <MessageBlock
        messageId={message.id}
        blocks={message.blocks}
        content={message.content}
        isUser={false}
        isAi
        isLoading={false}
        timestamp={message.timestamp}
        isComplete
        timerStartTime={null}
        timestampColor={theme.muted}
        markdownOptions={{ codeBlockWidth: 72, palette }}
        availableWidth={80}
        markdownPalette={palette}
        onToggleCollapsed={noop}
        onBuildFast={noop}
        onBuildMax={noop}
        onBuildLite={noop}
        userError={message.userError}
        refusal={message.refusal}
      />,
    )
  })
  try {
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

function lineIndex(lines: string[], needle: string): number {
  return lines.findIndex((line) => line.includes(needle))
}

describe('RefusalBanner', () => {
  test('names the refusal and shows the line', () => {
    const markup = renderToStaticMarkup(<RefusalBanner message={REFUSAL} />)
    expect(markup).toContain('Declined')
    expect(markup).toContain('The model declined to continue this request')
    expect(markup).toContain('safety category: cyber')
  })

  test('strips escape sequences from the provider note', () => {
    const markup = renderToStaticMarkup(
      <RefusalBanner message={'Declined\x1b]0;pwned\x07\x1b[2J by policy'} />,
    )
    expect(markup).not.toContain('\x1b')
    expect(markup).not.toContain('pwned')
    expect(markup).toContain('by policy')
  })

  test('renders nothing for an empty line', () => {
    expect(renderToStaticMarkup(<RefusalBanner message={'  '} />)).toBe('')
  })
})

describe('a refused turn as the chat draws it', () => {
  test('the refusal is a box of its own after the text the model wrote', async () => {
    const message = messageAfter([
      { type: 'text_delta', text: 'Here is how the lock works. First, the pins' },
      { type: 'text_block_end' },
      { type: 'notice', message: REFUSAL, kind: 'refusal' },
      { type: 'turn_finished' },
    ])
    const lines = (await frame(message)).split('\n')

    const answer = lineIndex(lines, 'Here is how the lock works.')
    const title = lineIndex(lines, 'Declined')
    const line = lineIndex(lines, 'The model declined to continue')
    expect(answer).toBeGreaterThanOrEqual(0)
    expect(title).toBeGreaterThan(answer)
    expect(line).toBeGreaterThan(title)
    // Drawn in a border, not as the answer's own text.
    expect(lines[title - 1]).toMatch(/[─┌┐+-]{4}/)
    expect(lines[answer]).not.toContain('declined')
  })

  test('a refusal before any text is still drawn, not an empty reply', async () => {
    const message = messageAfter([
      { type: 'notice', message: REFUSAL, kind: 'refusal' },
      { type: 'turn_finished' },
    ])
    expect(message.blocks ?? []).toEqual([])
    const shown = await frame(message)
    expect(shown).toContain('Declined')
    expect(shown).toContain('The model declined to continue this request')
  })
})

describe('a reasoning cell as the chat draws it', () => {
  test('stands apart from the answer', async () => {
    const message = messageAfter([
      { type: 'reasoning_started', chars: 0, redacted: false },
      { type: 'reasoning_delta', text: 'The user asks for 17 * 23.' },
      { type: 'reasoning_delta', text: ' That is 391.\n\n' },
      { type: 'text_delta', text: 'The product is 391.' },
      { type: 'text_block_end' },
      { type: 'turn_finished' },
    ])
    const lines = (await frame(message)).split('\n')

    const header = lineIndex(lines, 'Thinking')
    const reasoning = lineIndex(lines, 'The user asks for 17 * 23.')
    const answer = lineIndex(lines, 'The product is 391.')
    expect(header).toBeGreaterThanOrEqual(0)
    expect(reasoning).toBeGreaterThan(header)
    expect(answer).toBeGreaterThan(reasoning)
    // The reasoning is indented under its header ("• Thinking"); the answer
    // starts at the margin, after a blank line, and holds none of it.
    expect(lines[reasoning]!.indexOf('The user asks')).toBeGreaterThan(0)
    expect(lines[answer]!.indexOf('The product is')).toBe(0)
    expect(lines.slice(reasoning + 1, answer).every((line) => !line.trim())).toBe(true)
    expect(lines[answer]).not.toContain('The user asks')
    expect(lines[reasoning]).not.toContain('The product is')
  })
})
