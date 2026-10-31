/**
 * The engine's git views in the UI (features `git_views` and `checkpoints`;
 * the `git_views` capability itself is declared with the others elsewhere).
 *
 * {@link handleGitViewsEvent} takes the engine's events for them, before
 * engine-session.ts sees any event:
 *
 * - `changes` feeds the changed-files summary and the `/changes` panel;
 * - `diff` becomes a transcript cell;
 * - `commit_proposal` opens the commit card, `checkpoints` the rewind picker;
 * - `rewound` is the picker's dry-run preview, or what a real rewind did: the
 *   rewound prompts leave the transcript and the checkpoint's prompt goes back
 *   into the input.
 *
 * The rest of this module sends their commands. Nothing is committed or
 * rewound without the user's own key in a panel or their own `/commit <message>`.
 */

import { getNeuronEngineClient, runExchange, type NeuronEngineClient } from './neuron-engine-client'
import { useChatStore } from '../state/chat-store'
import { dropRewoundTurns, useGitViewsStore } from '../state/git-views-store'
import { consoleSafeText } from '../utils/glyphs'
import { getSystemMessage, getUserMessage } from '../utils/message-history'

import type { UiEvent } from './protocol'
import type { GitView } from '../types/git-view'

let lineCount = 0

/** A system message under an id of its own (several can come in one millisecond). */
function systemMessage(content: Parameters<typeof getSystemMessage>[0]) {
  lineCount += 1
  return { ...getSystemMessage(content), id: `sys-git-${Date.now()}-${lineCount}` }
}

function showLine(text: string): void {
  const message = systemMessage(consoleSafeText(text))
  useChatStore.getState().setMessages((prev) => [...prev, message])
}

/** Adds a diff or a rewind's result to the transcript. */
export function showGitViewCell(view: GitView): void {
  const message = systemMessage([{ type: 'git-view', view }])
  useChatStore.getState().setMessages((prev) => [...prev, message])
}

/** A user bubble for a command the UI sent in the user's name (`/diff src/a.ts`). */
function showCommand(text: string): void {
  useChatStore.getState().setMessages((prev) => [...prev, getUserMessage(text)])
}

/** The engine is running; otherwise says so in the chat. */
function readyEngine(): NeuronEngineClient | null {
  const engine = getNeuronEngineClient()
  if (engine.connection === 'ready') return engine
  showLine('The Neuron engine is not running.')
  return null
}

/**
 * Runs a command that ends with `turn_finished`. Resolves with the engine's
 * error, if it sent one; a send that failed (or an engine that exited) is said
 * in the chat and resolves with its message.
 */
function exchange(
  engine: NeuronEngineClient,
  send: (engine: NeuronEngineClient) => void,
): Promise<string | null> {
  return runExchange(engine, () => send(engine), () => {}).then(
    (outcome) => outcome.error,
    (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      showLine(message)
      return message
    },
  )
}

// ------------------------------------------------------------------ events

function applyRewound(event: Extract<UiEvent, { type: 'rewound' }>): void {
  const { type: _type, ...result } = event
  const store = useGitViewsStore.getState()
  const flow = store.rewind
  if (event.dry_run) {
    const awaited =
      flow !== null &&
      !flow.confirmed &&
      flow.checkpoint?.id === event.checkpoint &&
      flow.mode === event.mode
    // One the picker no longer waits for (Esc went back) is dropped.
    if (awaited) store.setRewindPreview(result)
    else if (flow === null) showGitViewCell({ kind: 'rewound', ...result })
    return
  }
  if (flow) store.closeRewind()
  const chat = useChatStore.getState()
  if (event.removed_turns > 0) {
    chat.setMessages((prev) => dropRewoundTurns(prev, event.removed_turns))
  }
  showGitViewCell({ kind: 'rewound', ...result })
  if (event.prompt) {
    chat.setInputValue({
      text: event.prompt,
      cursorPosition: event.prompt.length,
      lastEditDueToNav: false,
    })
  }
}

/**
 * Takes the git-view events; `true` when the event was one of them. `ready`
 * is read (the engine's features) but left to engine-session.ts too.
 */
export function handleGitViewsEvent(event: UiEvent): boolean {
  const store = useGitViewsStore.getState()
  switch (event.type) {
    case 'ready':
      store.setFeatures(event.features ?? [])
      return false
    case 'changes':
      store.setChanges(event.files)
      return true
    case 'diff': {
      const { type: _type, ...diff } = event
      showGitViewCell({ kind: 'diff', ...diff })
      return true
    }
    case 'commit_proposal': {
      const { type: _type, ...proposal } = event
      if (proposal.files.length === 0) showLine('Nothing to commit.')
      else store.setProposal(proposal)
      return true
    }
    case 'checkpoints':
      if (event.items.length === 0) {
        showLine('No checkpoints yet: each prompt you send makes one.')
      } else {
        store.openRewind(event.items)
      }
      return true
    case 'rewound':
      applyRewound(event)
      return true
    default:
      return false
  }
}

/** A new engine is starting: nothing of the last one's views is left. */
export function resetGitViews(): void {
  useGitViewsStore.getState().reset()
}

// ---------------------------------------------------------------- commands

/** `/diff [path]`: the engine answers with a `diff` cell. */
export function requestDiff(path: string, options: { echo?: boolean } = {}): void {
  const engine = readyEngine()
  if (!engine) return
  const target = path.trim()
  if (options.echo) showCommand(target ? `/diff ${target}` : '/diff')
  void exchange(engine, (client) => client.slash('diff', target))
}

/**
 * `/commit`: without a message the engine drafts one and the card asks;
 * with one it commits at once (the user typed it).
 */
export function requestCommit(message: string): void {
  const engine = readyEngine()
  if (!engine) return
  void exchange(engine, (client) => client.slash('commit', message.trim()))
}

/** `/rewind`: the engine lists its checkpoints for the picker. */
export function requestCheckpoints(args = ''): void {
  const engine = readyEngine()
  if (!engine) return
  void exchange(engine, (client) => client.slash('rewind', args.trim()))
}

/** Enter on the commit card: commit with its message. */
export function acceptCommitProposal(): void {
  const store = useGitViewsStore.getState()
  const proposal = store.proposal
  if (!proposal) return
  store.setProposal(null)
  const engine = readyEngine()
  if (!engine) return
  void exchange(engine, (client) => client.send({ type: 'commit', message: proposal.message }))
}

/** `e` on the commit card: `/commit <message>` in the input, to edit and send. */
export function editCommitProposal(): void {
  const store = useGitViewsStore.getState()
  const proposal = store.proposal
  if (!proposal) return
  store.setProposal(null)
  const text = `/commit ${proposal.message}`
  useChatStore.getState().setInputValue({
    text,
    cursorPosition: text.length,
    lastEditDueToNav: false,
  })
}

/** Esc on the commit card: nothing is committed. */
export function cancelCommitProposal(): void {
  useGitViewsStore.getState().setProposal(null)
}

/** The rewind picker's checkpoint and mode, once both are picked. */
function pickedRewind() {
  const flow = useGitViewsStore.getState().rewind
  if (!flow?.checkpoint || !flow.mode) return null
  return { flow, checkpoint: flow.checkpoint.id, mode: flow.mode }
}

/** Asks for a dry run of the picked rewind; its `rewound` is the preview. */
export function previewRewind(): void {
  const picked = pickedRewind()
  if (!picked) return
  const engine = readyEngine()
  if (!engine) {
    useGitViewsStore.getState().closeRewind()
    return
  }
  const { checkpoint, mode } = picked
  void exchange(engine, (client) =>
    client.send({ type: 'rewind', checkpoint, mode, dry_run: true }),
  ).then((error) => {
    const flow = useGitViewsStore.getState().rewind
    const waiting =
      flow !== null &&
      !flow.confirmed &&
      flow.preview === null &&
      flow.checkpoint?.id === checkpoint &&
      flow.mode === mode
    if (waiting) {
      useGitViewsStore
        .getState()
        .setRewindError(error ?? 'The engine sent no preview for this rewind.')
    }
  })
}

/** Enter on the preview: the real rewind, once (a second Enter does nothing). */
export function confirmRewind(): void {
  const picked = pickedRewind()
  if (!picked || !picked.flow.preview || picked.flow.confirmed) return
  const engine = readyEngine()
  if (!engine) {
    useGitViewsStore.getState().closeRewind()
    return
  }
  useGitViewsStore.getState().markRewindConfirmed()
  const { checkpoint, mode } = picked
  void exchange(engine, (client) =>
    client.send({ type: 'rewind', checkpoint, mode, dry_run: false }),
  ).then(() => {
    // Its `rewound` closed the picker; an error was said in the chat.
    const flow = useGitViewsStore.getState().rewind
    if (flow?.confirmed) useGitViewsStore.getState().closeRewind()
  })
}
