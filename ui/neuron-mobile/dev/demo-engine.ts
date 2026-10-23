/**
 * A scripted engine for trying the phone without a model: it answers each
 * prompt with Markdown, a bit of reasoning, an edit that needs approval (with
 * its diff) and a result, the way `neuron engine --stdio` reports them. A
 * prompt ending in "?" gets a question with options instead.
 */

import type { ClientMessage, EngineTransport, SessionAction, TransportClose, UiEvent } from '@neuron/client'

export interface DemoEngineOptions {
  cwd: string
  /** Milliseconds between streamed events (0 in tests). */
  pace?: number
}

const ANSWER = `Here is the plan:

1. Read \`src/math.ts\`
2. Fix the off-by-one in **sum**
3. Run the tests

> The fix is one line, so I will ask before editing.

\`\`\`ts
export const sum = (items: number[]) => items.reduce((total, item) => total + item, 0)
\`\`\`

See [the docs](https://zero-x.live/docs) for more.`

export class DemoEngine implements EngineTransport {
  readonly echoesActions = false
  /** Everything the clients sent, in order (tests read it). */
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private permissionWaiters = new Map<number, (message: Extract<ClientMessage, { kind: 'permission_reply' }>) => void>()
  private nextPermission = 1
  private nextQuestion = 1
  private nextTool = 1
  private sessionId = 'demo-1'
  private chats: Array<{ id: string; title: string; message_count: number; updated_at_ms: number }> = []
  private busy = false

  constructor(private readonly options: DemoEngineOptions) {}

  async start(): Promise<void> {
    this.emit({ type: 'ready', protocol: 1, version: '6.3.0-demo', features: ['sessions'] })
    this.status()
  }

  private status(): void {
    this.emit({
      type: 'status',
      model: 'demo-model',
      permission_mode: 'workspace-write',
      session_id: this.sessionId,
      cwd: this.options.cwd,
      workspace_trusted: true,
      plan_mode: false,
      context_percent: 4,
      git: { branch: 'main', changed: 1, staged: 0, unstaged: 1, untracked: 0, conflicted: 0 },
    })
  }

  /** Emits an engine event to every listener (tests call it too). */
  emit(event: UiEvent): void {
    for (const listener of [...this.listeners]) listener(event)
  }

  private wait(): Promise<void> {
    const pace = this.options.pace ?? 60
    return pace > 0 ? new Promise((resolve) => setTimeout(resolve, pace)) : Promise.resolve()
  }

  send(message: ClientMessage): void {
    this.sent.push(message)
    if (message.kind === 'permission_reply') {
      this.permissionWaiters.get(message.id)?.(message)
      this.permissionWaiters.delete(message.id)
      return
    }
    switch (message.type) {
      case 'submit':
        void this.turn(message.text)
        return
      case 'answer':
        void Promise.resolve().then(() => this.reply(`You picked **${message.text}**.`))
        return
      case 'interrupt':
        this.busy = false
        this.emit({ type: 'notice', message: 'Interrupted.' })
        this.emit({ type: 'turn_finished' })
        return
      case 'list_sessions':
        this.emit({
          type: 'sessions',
          current: this.sessionId,
          sessions: [
            { id: this.sessionId, title: this.chats[0]?.title ?? null, model: 'demo-model', message_count: this.chats[0]?.message_count ?? 0, updated_at_ms: Date.now(), current: true, branch: 'main' },
          ],
        })
        return
      default:
        return
    }
  }

  private async reply(text: string): Promise<void> {
    for (const word of text.split(/(?<=\s)/)) {
      this.emit({ type: 'text_delta', text: word })
      await this.wait()
    }
    this.emit({ type: 'text_block_end' })
    this.emit({ type: 'turn_finished' })
  }

  private async turn(prompt: string): Promise<void> {
    if (this.busy) return
    this.busy = true
    // A real engine answers on its own time: the host records the prompt first.
    await new Promise((resolve) => setTimeout(resolve, 0))
    if (!this.chats.length) this.chats.push({ id: this.sessionId, title: prompt.slice(0, 60), message_count: 0, updated_at_ms: Date.now() })
    this.chats[0].message_count += 2
    if (prompt.trim().endsWith('?')) {
      this.emit({ type: 'question', id: this.nextQuestion++, question: 'Which branch should I use?', options: ['main', 'dev'] })
      this.busy = false
      return
    }
    this.emit({ type: 'reasoning_started', chars: null, redacted: false })
    for (const part of ['The user wants ', 'the sum fixed. ', 'Read the file, then propose a one-line edit.']) {
      this.emit({ type: 'reasoning_delta', text: part })
      await this.wait()
    }
    for (const chunk of ANSWER.match(/[\s\S]{1,40}/g) ?? []) {
      this.emit({ type: 'text_delta', text: chunk })
      await this.wait()
    }
    this.emit({ type: 'text_block_end' })

    const readId = `tool-${this.nextTool++}`
    this.emit({ type: 'tool_call_started', id: readId, name: 'read_file', input: JSON.stringify({ path: `${this.options.cwd}/src/math.ts` }), cut_off: false })
    await this.wait()
    this.emit({
      type: 'tool_result',
      id: readId,
      name: 'read_file',
      output: JSON.stringify({ file: { content: 'export const sum = (items: number[]) =>\n  items.reduce((total, item) => total + item, 1)\n' } }),
      is_error: false,
    })

    const editId = `tool-${this.nextTool++}`
    const input = JSON.stringify({
      path: `${this.options.cwd}/src/math.ts`,
      old_string: '  items.reduce((total, item) => total + item, 1)',
      new_string: '  items.reduce((total, item) => total + item, 0)',
    })
    this.emit({ type: 'tool_call_started', id: editId, name: 'edit_file', input, cut_off: false })
    const permission = this.nextPermission++
    const answer = new Promise<Extract<ClientMessage, { kind: 'permission_reply' }>>((resolve) => this.permissionWaiters.set(permission, resolve))
    this.emit({
      type: 'permission_requested',
      id: permission,
      request: { tool: 'edit_file', current_mode: 'read-only', required_mode: 'workspace-write', reason: 'Editing a file', input, session_prefixes: [] },
    })
    const reply = await answer
    if (!this.busy) return
    if (reply.decision.kind === 'deny') {
      this.emit({ type: 'tool_result', id: editId, name: 'edit_file', output: `Denied${reply.decision.note ? `: ${reply.decision.note}` : ''}`, is_error: true })
      await this.reply(reply.decision.note ? `Understood: "${reply.decision.note}". I left the file as it was.` : 'I left the file as it was.')
    } else {
      this.emit({
        type: 'tool_result',
        id: editId,
        name: 'edit_file',
        output: JSON.stringify({ diff: '@@ -1,2 +1,2 @@\n export const sum = (items: number[]) =>\n-  items.reduce((total, item) => total + item, 1)\n+  items.reduce((total, item) => total + item, 0)\n' }),
        is_error: false,
      })
      await this.reply('Fixed. `sum([])` is now `0`.')
    }
    this.busy = false
  }

  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  onClose(listener: (close: TransportClose) => void): () => void {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }

  async close(): Promise<void> {
    for (const listener of [...this.closeListeners]) listener({ code: 0, error: null })
  }
}
