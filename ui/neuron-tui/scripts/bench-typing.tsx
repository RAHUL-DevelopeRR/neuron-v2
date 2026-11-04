// What typing and sitting idle cost the chat: `bun run scripts/bench-typing.tsx [--json]`.
//
// Lays out the chat as chat.tsx does (the header with its logo, the visible
// transcript, the input), then
// - idle: counts the frames the renderer is asked for in 3 s with nothing
//   happening, with the logo's sheen on (the header in view, as on a new
//   chat) and off;
// - typing: types 60 characters, one per frame, and times each keystroke from
//   the input's state change to the drawn frame, for an empty chat and for
//   15 earlier messages (the most chat.tsx shows before "load previous") of
//   short and of long answers.
// React runs its development build here, so compare runs with each other.
process.env.NODE_ENV ||= 'development'

import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React, { useLayoutEffect, useState } from 'react'

import { ChatHeader } from '../src/components/chat-header'
import { MessageWithAgents } from '../src/components/message-with-agents'
import { MultilineInput } from '../src/components/multiline-input'
import { initializeThemeStore, useThemeStore } from '../src/hooks/use-theme'
import { useMessageBlockStore } from '../src/state/message-block-store'
import { createMarkdownPalette } from '../src/utils/theme-system'

import type { ChatMessage } from '../src/types/chat'

const WIDTH = 120
const HEIGHT = 40

function answer(lines: number): string {
  const out: string[] = ['## Plan', '']
  for (let i = 0; i < lines; i++) {
    if (i % 25 === 10) out.push('```ts', 'export function total(items: Item[]): number {', '  return items.reduce((sum, item) => sum + item.price * item.qty, 0)', '}', '```')
    else out.push(`- Step ${i}: update \`src/cart/${i}.ts\` so the **cart total** follows the [pricing rules](https://example.com/${i}).`)
  }
  return out.join('\n')
}

function history(turns: number, lines: number): ChatMessage[] {
  const messages: ChatMessage[] = []
  for (let turn = 0; turn < turns; turn++) {
    messages.push({ id: `u${turn}`, variant: 'user', content: `Make part ${turn} of the shop.`, timestamp: 'earlier' })
    const text = answer(lines)
    messages.push({
      id: `a${turn}`,
      variant: 'ai',
      content: text,
      blocks: [{ type: 'text', content: text }] as ChatMessage['blocks'],
      timestamp: 'earlier',
      isComplete: true,
    })
  }
  return messages.slice(-15)
}

let setValueRef: (value: string) => void = () => {}

function Harness({ messages, sheen }: { messages: ChatMessage[]; sheen: boolean }) {
  const [value, setValue] = useState('')
  setValueRef = setValue
  const theme = useThemeStore((state) => state.theme)
  const setContext = useMessageBlockStore((state) => state.setContext)
  const markdownPalette = React.useMemo(() => createMarkdownPalette(theme), [theme])
  useLayoutEffect(() => {
    setContext({
      readOnly: false,
      theme,
      markdownPalette,
      messageTree: new Map(),
      isWaitingForResponse: false,
      timerStartTime: null,
      availableWidth: WIDTH - 3,
    })
  }, [theme, markdownPalette, setContext])
  return (
    <box style={{ flexDirection: 'column', width: WIDTH, flexGrow: 1 }}>
      <scrollbox style={{ flexGrow: 1 }} stickyScroll stickyStart="bottom">
        <ChatHeader projectRoot="/work/shop" animationEnabled={sheen} />
        {messages.map((message, index) => (
          <MessageWithAgents
            key={message.id}
            message={message}
            depth={0}
            isLastMessage={index === messages.length - 1}
            availableWidth={WIDTH - 3}
          />
        ))}
      </scrollbox>
      <MultilineInput
        value={value}
        cursorPosition={value.length}
        onChange={() => {}}
        onSubmit={() => {}}
        onPaste={() => {}}
        shouldBlinkCursor
      />
    </box>
  )
}

const round = (value: number) => Math.round(value * 100) / 100
function stats(samples: number[]) {
  const sorted = [...samples].sort((a, b) => a - b)
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? 0
  return {
    mean: round(sorted.reduce((sum, v) => sum + v, 0) / Math.max(1, sorted.length)),
    p50: round(at(0.5)),
    p95: round(at(0.95)),
    max: round(sorted[sorted.length - 1] ?? 0),
  }
}
const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms))

async function mount(messages: ChatMessage[], sheen: boolean) {
  const setup = await createTestRenderer({ width: WIDTH, height: HEIGHT })
  let requested = 0
  const request = setup.renderer.requestRender.bind(setup.renderer)
  setup.renderer.requestRender = () => {
    requested++
    request()
  }
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<Harness messages={messages} sheen={sheen} />))
  await setup.renderOnce()
  await setup.renderOnce()
  return {
    setup,
    requests: () => requested,
    close: () => {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    },
  }
}

async function idle(sheen: boolean) {
  const view = await mount([], sheen)
  const before = view.requests()
  const cpu0 = process.cpuUsage()
  await sleep(3000)
  const cpu = process.cpuUsage(cpu0)
  const requests = view.requests() - before
  // Draw what was asked for, to time one idle frame.
  const t0 = performance.now()
  await view.setup.renderOnce()
  const frameMs = performance.now() - t0
  view.close()
  return { sheen, frameRequestsPerSec: round(requests / 3), cpuMsPerSec: round((cpu.user + cpu.system) / 1000 / 3), frameMs: round(frameMs) }
}

async function typing(label: string, messages: ChatMessage[]) {
  const view = await mount(messages, false)
  const react: number[] = []
  const frame: number[] = []
  let value = ''
  for (let i = 0; i < 60; i++) {
    value += 'abcdefghij'[i % 10]
    const t0 = performance.now()
    setValueRef(value)
    await new Promise<void>((resolve) => setImmediate(resolve))
    const t1 = performance.now()
    await view.setup.renderOnce()
    const t2 = performance.now()
    if (i >= 5) {
      react.push(t1 - t0)
      frame.push(t2 - t1)
    }
  }
  const total = react.map((ms, i) => ms + frame[i]!)
  view.close()
  return { label, messages: messages.length, reactMs: stats(react), frameMs: stats(frame), keystrokeMs: stats(total) }
}

initializeThemeStore()
useMessageBlockStore.getState().reset()

const results = {
  idle: [await idle(true), await idle(false)],
  typing: [
    await typing('empty chat', []),
    await typing('15 messages, short answers (20 lines)', history(8, 20)),
    await typing('15 messages, long answers (200 lines)', history(8, 200)),
  ],
}

if (process.argv.includes('--json')) {
  console.log(JSON.stringify(results, null, 2))
} else {
  for (const r of results.idle) {
    console.log(`idle, logo sheen ${r.sheen ? 'on ' : 'off'}: ${r.frameRequestsPerSec} frame requests/s, ${r.cpuMsPerSec} ms CPU/s, one frame ${r.frameMs} ms`)
  }
  for (const r of results.typing) {
    console.log(`typing, ${r.label}: keystroke mean ${r.keystrokeMs.mean} ms, p95 ${r.keystrokeMs.p95} ms (react ${r.reactMs.mean}, frame ${r.frameMs.mean})`)
  }
}
process.exit(0)
