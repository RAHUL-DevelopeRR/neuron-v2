import { forwardRef, useEffect, useId, useImperativeHandle, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react'

import type { Attachment } from '@neuron/client'

import { availableCommands, findCommand, matchCommands, parseSlash, type CommandSpec, type DesktopCommand } from '../commands'
import type { KeyValueStore, Platform } from '../platform'

/** The permission modes, as the mode menu offers them. */
export const MODES: Array<{ id: string; label: string; help: string }> = [
  { id: 'read-only', label: 'Read only', help: 'Reads and searches; asks before any change' },
  { id: 'workspace-write', label: 'Edit files', help: 'Edits in this project; asks before commands that reach further' },
  { id: 'danger-full-access', label: 'Full access', help: 'Runs anything without asking. Use with care' },
]

/** The order Ctrl+Shift+M goes through: read only, edit files, then plan mode. Full access is never in it; only the menu picks it. */
export function nextMode(mode: string, planMode: boolean): { mode: string; plan: boolean } {
  const steps = [
    { mode: 'read-only', plan: false },
    { mode: 'workspace-write', plan: false },
    { mode, plan: true },
  ]
  const at = planMode ? steps.length - 1 : steps.findIndex((step) => step.mode === mode)
  // From full access (not in the cycle) the first step is the safest one.
  return steps[(at + 1) % steps.length]
}

const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp']
const MAX_IMAGE_BYTES = 10 * 1024 * 1024
const HISTORY_KEPT = 100

type Suggestion =
  | { kind: 'command'; value: string; help: string; args?: string; names: string[] }
  | { kind: 'file'; value: string }
  | { kind: 'folder'; value: string }

/** A file or folder mentioned with `@`; a folder's `path` has no trailing slash. */
interface Mention {
  path: string
  kind: 'file' | 'folder'
}

interface StagedImage {
  id: number
  name: string
  path: string
  media_type: string
  /** For the thumbnail; absent for an image put back after a failed send. */
  url: string | null
}

interface Queued {
  id: number
  text: string
  attachments: Attachment[]
}

const shown = (mention: Mention) => (mention.kind === 'folder' ? `${mention.path}/` : mention.path)
const basename = (path: string) => path.split(/[\\/]/).pop() || path
const errorText = (error: unknown) => (typeof error === 'string' ? error : error instanceof Error ? error.message : 'Something went wrong.')

/** The `@word` being typed at the caret, with where it starts. */
function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret)
  const match = /(^|\s)@([^\s@]*)$/.exec(before)
  return match ? { start: caret - match[2].length - 1, query: match[2] } : null
}

/** What the window can do with the message box from outside it. */
export interface ComposerHandle {
  /** Sends what is in the box (a card's Retry). */
  submit(): void
  focus(): void
}

interface ComposerProps {
  busy: boolean
  disabled: boolean
  /** The agent asked something: the next message answers it. */
  questionWaiting: boolean
  enterSends: boolean
  /** Workspace paths for `@`; folders end with `/`. */
  searchFiles: (query: string) => Promise<string[]>
  /** The mode and model menus. */
  toolbar?: ReactNode
  /** A prompt to put back in an empty box (one a gateway failure stopped). */
  restore?: { text: string; attachments: Attachment[] } | null
  /** Why the box is off, for its placeholder; "not connected" by default. */
  disabledReason?: string
  onSend: (text: string, attachments: Attachment[]) => void
  onInterrupt: () => void
  /** The engine's `ready.features`: the slash menu lists what these allow. */
  features?: string[]
  /** The model reads images (`status.supports_images === true`). */
  supportsImages?: boolean
  stageAttachment?: Platform['stageAttachment']
  /** Where earlier prompts are kept, under `composer.history.<projectPath>`. */
  store?: KeyValueStore
  projectPath?: string | null
  /** An approval is open: queued prompts wait for it. */
  permissionWaiting?: boolean
  /** A command this window runs; what it returns is shown under the box. */
  onCommand?: (name: DesktopCommand, args: string) => string | void | Promise<string | void>
  /** Ctrl+Shift+M (Cmd+Shift+M). */
  onCycleMode?: () => void
  /** The chat's session id: queued prompts belong to one session and are dropped when it changes. */
  sessionId?: string | null
  /** The last turn failed: queued prompts wait for the user instead of running into the same failure. */
  turnFailed?: boolean
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer(
  {
    busy,
    disabled,
    questionWaiting,
    enterSends,
    searchFiles,
    toolbar,
    restore,
    disabledReason,
    onSend,
    onInterrupt,
    features = [],
    supportsImages = false,
    stageAttachment,
    store,
    projectPath = null,
    permissionWaiting = false,
    onCommand,
    onCycleMode,
    sessionId = null,
    turnFailed = false,
  },
  ref,
) {
  const [text, setText] = useState('')
  const [caret, setCaret] = useState(0)
  const [mentions, setMentions] = useState<Mention[]>([])
  const [images, setImages] = useState<StagedImage[]>([])
  const [found, setFound] = useState<string[]>([])
  const [pick, setPick] = useState(0)
  const [hint, setHint] = useState<string | null>(null)
  const [helpOpen, setHelpOpen] = useState(false)
  const [queue, setQueue] = useState<Queued[]>([])
  const [editing, setEditing] = useState<{ id: number; text: string } | null>(null)
  // The queue waits after an interrupt, a stop or a failed turn, until the user resumes it.
  const [paused, setPaused] = useState(false)
  // Esc closed the open suggestion menu; typing opens it again.
  const [menuClosed, setMenuClosed] = useState(false)
  const listId = useId()
  const queueRef = useRef<Queued[]>([])
  queueRef.current = queue
  const box = useRef<HTMLTextAreaElement>(null)
  const nextId = useRef(0)
  const lastSent = useRef(-1)
  const history = useRef<string[]>([])
  const historyAt = useRef<number | null>(null)
  const draft = useRef('')
  const imagesRef = useRef<StagedImage[]>([])
  imagesRef.current = images
  // An answer to the agent's question carries no attachments, so `@` offers none.
  const mention = questionWaiting ? null : mentionAt(text, caret)
  const historyKey = projectPath ? `composer.history.${projectPath}` : null

  // Nothing typed is lost: a prompt the gateway refused comes back, unless something new is being typed.
  useEffect(() => {
    if (!restore) return
    setText((current) => (current.trim() ? current : restore.text))
    setMentions((current) =>
      current.length
        ? current
        : restore.attachments.flatMap((item): Mention[] => (item.kind === 'file' || item.kind === 'folder' ? [{ path: item.path, kind: item.kind }] : [])),
    )
    setImages((current) =>
      current.length
        ? current
        : restore.attachments.flatMap((item): StagedImage[] =>
            item.kind === 'image' ? [{ id: ++nextId.current, name: basename(item.path), path: item.path, media_type: item.media_type, url: null }] : [],
          ),
    )
  }, [restore])

  // The prompts of this project, oldest first.
  useEffect(() => {
    history.current = []
    historyAt.current = null
    if (!store || !historyKey) return
    let cancelled = false
    store.get<string[]>(historyKey).then(
      (saved) => {
        if (cancelled || !Array.isArray(saved)) return
        history.current = [...saved.filter((entry) => typeof entry === 'string'), ...history.current].slice(-HISTORY_KEPT)
      },
      () => {},
    )
    return () => {
      cancelled = true
    }
  }, [store, historyKey])

  useEffect(() => {
    if (!mention) {
      setFound([])
      return
    }
    let cancelled = false
    const timer = setTimeout(() => {
      searchFiles(mention.query).then(
        (paths) => !cancelled && setFound(paths.slice(0, 8)),
        () => !cancelled && setFound([]),
      )
    }, 120)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mention?.query, mention?.start, searchFiles])

  // Thumbnails are released when the box goes away.
  useEffect(
    () => () => {
      for (const image of imagesRef.current) if (image.url) URL.revokeObjectURL?.(image.url)
    },
    [],
  )

  const suggestions = useMemo<Suggestion[]>(() => {
    if (menuClosed) return []
    if (mention) return found.map((value): Suggestion => (value.endsWith('/') ? { kind: 'folder', value } : { kind: 'file', value }))
    const command = /^\/(\S*)$/.exec(text)
    if (!command || questionWaiting) return []
    return matchCommands(features, command[1]).map(
      (spec: CommandSpec): Suggestion => ({
        kind: 'command',
        value: spec.name,
        help: spec.summary,
        args: spec.args,
        names: [spec.name, ...(spec.aliases ?? [])],
      }),
    )
  }, [mention, found, text, questionWaiting, features, menuClosed])

  const choose = (suggestion: Suggestion) => {
    if (suggestion.kind === 'command') {
      setText(`/${suggestion.value} `)
    } else if (mention) {
      const next = `${text.slice(0, mention.start)}@${suggestion.value} ${text.slice(caret)}`
      setText(next)
      const item: Mention =
        suggestion.kind === 'folder' ? { path: suggestion.value.replace(/\/+$/, ''), kind: 'folder' } : { path: suggestion.value, kind: 'file' }
      setMentions((current) => (current.some((entry) => entry.path === item.path && entry.kind === item.kind) ? current : [...current, item]))
      const at = mention.start + suggestion.value.length + 2
      setCaret(at)
      requestAnimationFrame(() => box.current?.setSelectionRange(at, at))
    }
    setPick(0)
    box.current?.focus()
  }

  const showResult = (result: string | void | Promise<string | void>) =>
    Promise.resolve(result).then(
      (message) => {
        if (message) setHint(message)
      },
      (error: unknown) => setHint(errorText(error)),
    )

  /** Sends one prompt: a command this window runs, or the engine's. */
  const run = (item: Queued) => {
    const command = questionWaiting ? null : parseSlash(item.text)
    const spec = command ? findCommand(command.name, features) : undefined
    if (command && spec?.handler === 'desktop') {
      if (spec.name === 'help') setHelpOpen((open) => !open)
      else void showResult(onCommand?.(spec.name as DesktopCommand, command.args))
      return
    }
    onSend(item.text, item.attachments)
  }

  const remember = (value: string) => {
    historyAt.current = null
    if (history.current.at(-1) === value) return
    history.current = [...history.current, value].slice(-HISTORY_KEPT)
    if (store && historyKey) void store.set(historyKey, history.current).catch(() => {})
  }

  const send = () => {
    const value = text.trim()
    if (!value || disabled) return
    // Only the files still mentioned in the text go along. An answer to a question carries none: they stay for the next message.
    const attachments: Attachment[] = questionWaiting
      ? []
      : [
      ...mentions.filter((item) => value.includes(`@${shown(item)}`)).map((item): Attachment => ({ kind: item.kind, path: item.path })),
      ...images.map((image): Attachment => ({ kind: 'image', path: image.path, media_type: image.media_type })),
        ]
    const item = { id: ++nextId.current, text: value, attachments }
    remember(value)
    if (!questionWaiting) {
      for (const image of images) if (image.url) URL.revokeObjectURL?.(image.url)
      setMentions([])
      setImages([])
    }
    setText('')
    setCaret(0)
    setPick(0)
    setHint(null)
    const command = questionWaiting ? null : parseSlash(value)
    const spec = command ? findCommand(command.name, features) : undefined
    if (busy && !questionWaiting && !spec?.immediate) setQueue((current) => [...current, item])
    else run(item)
  }

  // The turn is over (and nothing waits for an answer): the next queued prompt goes.
  useEffect(() => {
    if (busy || disabled || questionWaiting || permissionWaiting || paused || queue.length === 0) return
    const next = queue[0]
    if (lastSent.current === next.id) return
    lastSent.current = next.id
    setQueue((current) => current.filter((item) => item.id !== next.id))
    run(next)
    // `run` follows the props above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, disabled, questionWaiting, permissionWaiting, paused, queue])

  // Nothing waits any more: nothing is paused.
  useEffect(() => {
    if (queue.length === 0) setPaused(false)
  }, [queue.length])

  // Another session: its queue is not this one's.
  const lastSession = useRef(sessionId)
  useEffect(() => {
    if (lastSession.current === sessionId) return
    lastSession.current = sessionId
    setQueue([])
    setPaused(false)
  }, [sessionId])

  // A failed turn (or a prompt put back after one) holds the queue.
  useEffect(() => {
    if ((turnFailed || restore) && queueRef.current.length > 0) setPaused(true)
  }, [turnFailed, restore])

  const interrupt = () => {
    if (queueRef.current.length > 0) setPaused(true)
    onInterrupt()
  }

  useImperativeHandle(ref, () => ({ submit: send, focus: () => box.current?.focus() }))

  const addImages = async (files: File[]) => {
    if (questionWaiting) {
      setHint('Answer the question first; images and files can go with your next message.')
      return
    }
    if (!supportsImages) {
      setHint("This model can't read images, so the image was not attached.")
      return
    }
    if (!stageAttachment) {
      setHint('Images cannot be attached here.')
      return
    }
    for (const file of files) {
      if (!IMAGE_TYPES.includes(file.type)) {
        setHint('Only PNG, JPEG, GIF or WebP images can be attached.')
        continue
      }
      if (file.size > MAX_IMAGE_BYTES) {
        setHint('Images can be at most 10 MB.')
        continue
      }
      try {
        const staged = await stageAttachment(file.name || 'pasted-image', new Uint8Array(await file.arrayBuffer()), file.type)
        const url = typeof URL.createObjectURL === 'function' ? URL.createObjectURL(file) : null
        setImages((current) => [
          ...current,
          { id: ++nextId.current, name: file.name || 'pasted image', path: staged.path, media_type: staged.media_type, url },
        ])
        setHint(null)
      } catch (error) {
        setHint(errorText(error))
      }
    }
  }

  const imageFiles = (list: FileList | File[] | null | undefined) => Array.from(list ?? []).filter((file) => file.type.startsWith('image/'))

  const recall = (event: ReactKeyboardEvent<HTMLTextAreaElement>): boolean => {
    const field = event.currentTarget
    const start = field.selectionStart ?? 0
    const end = field.selectionEnd ?? start
    const recalling = historyAt.current !== null
    if (event.key === 'ArrowUp') {
      const atStart = start === 0 && end === 0
      if (!atStart && !(recalling && !text.slice(0, start).includes('\n'))) return false
      if (history.current.length === 0) return false
      if (!recalling) draft.current = text
      historyAt.current = recalling ? Math.max(0, historyAt.current! - 1) : history.current.length - 1
      setText(history.current[historyAt.current])
      return true
    }
    if (!recalling) return false
    const atEnd = start === text.length && end === text.length
    if (!atEnd && text.slice(end).includes('\n')) return false
    const at = historyAt.current!
    if (at < history.current.length - 1) {
      historyAt.current = at + 1
      setText(history.current[at + 1])
    } else {
      historyAt.current = null
      setText(draft.current)
    }
    return true
  }

  const commands = availableCommands(features)

  return (
    <div className="composer">
      {suggestions.length > 0 && (
        <ul id={listId} className="suggestions" role="listbox" aria-label={mention ? 'Files' : 'Commands'}>
          {suggestions.map((suggestion, index) => (
            <li
              key={`${suggestion.kind}:${suggestion.value}`}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={index === pick}
              className={index === pick ? 'active' : ''}
              onMouseDown={(event) => {
                event.preventDefault()
                choose(suggestion)
              }}
            >
              {suggestion.kind === 'command' ? (
                <>
                  <span className="command">/{suggestion.value}</span>
                  {suggestion.args && <span className="muted"> {suggestion.args}</span>} <span className="muted">{suggestion.help}</span>
                </>
              ) : (
                <span className="command">@{suggestion.value}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      {helpOpen && (
        <section className="composer-help" role="region" aria-label="Commands">
          <div className="composer-help-head">
            <strong>Commands</strong>
            <button type="button" className="link-button" onClick={() => setHelpOpen(false)}>
              Close
            </button>
          </div>
          <ul>
            {commands.map((command) => (
              <li key={command.name}>
                <span className="command">/{command.name}</span>
                {command.args && <span className="muted"> {command.args}</span>} <span className="muted">{command.summary}</span>
              </li>
            ))}
          </ul>
          <p className="muted small">
            Enter sends · Shift+Enter adds a line · Ctrl+Shift+M changes the permission mode · Up and Down bring back earlier prompts · Esc stops a turn
          </p>
        </section>
      )}
      {paused && queue.length > 0 && (
        <div className="composer-hint" role="status">
          Queue paused{' '}
          <button type="button" className="link-button" onClick={() => setPaused(false)}>
            Resume
          </button>
        </div>
      )}
      {queue.length > 0 && (
        <ul className="queue" aria-label="Queued messages">
          {queue.map((item) => (
            <li key={item.id}>
              {editing?.id === item.id ? (
                <>
                  <textarea
                    autoFocus
                    aria-label="Edit queued message"
                    rows={2}
                    value={editing.text}
                    onChange={(event) => setEditing({ id: item.id, text: event.target.value })}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' && !event.shiftKey) {
                        event.preventDefault()
                        const value = editing.text.trim()
                        setQueue((current) => (value ? current.map((entry) => (entry.id === item.id ? { ...entry, text: value } : entry)) : current.filter((entry) => entry.id !== item.id)))
                        setEditing(null)
                      } else if (event.key === 'Escape') {
                        event.preventDefault()
                        event.stopPropagation()
                        setEditing(null)
                      }
                    }}
                  />
                  <button type="button" onClick={() => setEditing(null)}>
                    Cancel
                  </button>
                </>
              ) : (
                <>
                  <span className="queue-text">{item.text}</span>
                  {item.attachments.length > 0 && <span className="muted small">+{item.attachments.length} attached</span>}
                  <button type="button" aria-label={`Edit queued message: ${item.text}`} onClick={() => setEditing({ id: item.id, text: item.text })}>
                    Edit
                  </button>
                  <button
                    type="button"
                    aria-label={`Remove queued message: ${item.text}`}
                    onClick={() => setQueue((current) => current.filter((entry) => entry.id !== item.id))}
                  >
                    ✕
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {questionWaiting && <div className="composer-hint">Answering Neuron's question</div>}
      <div
        className="composer-box"
        onDragOver={(event) => {
          if (event.dataTransfer?.types?.includes('Files')) event.preventDefault()
        }}
        onDrop={(event) => {
          const dropped = imageFiles(event.dataTransfer?.files)
          if (dropped.length === 0) return
          event.preventDefault()
          void addImages(dropped)
        }}
      >
        <textarea
          ref={box}
          aria-label="Message Neuron"
          role="combobox"
          aria-autocomplete="list"
          aria-haspopup="listbox"
          aria-expanded={suggestions.length > 0}
          aria-controls={suggestions.length > 0 ? listId : undefined}
          aria-activedescendant={suggestions.length > 0 ? `${listId}-${pick}` : undefined}
          placeholder={
            disabled
              ? (disabledReason ?? 'Neuron is not connected')
              : questionWaiting
                ? 'Your answer…'
                : busy
                  ? 'Neuron is working. Enter adds this to the queue'
                  : 'Ask Neuron to build, fix or explain…  @ adds a file, / runs a command'
          }
          value={text}
          disabled={disabled}
          rows={Math.min(10, Math.max(2, text.split('\n').length))}
          onChange={(event) => {
            setText(event.target.value)
            setCaret(event.target.selectionStart ?? event.target.value.length)
            setPick(0)
            setMenuClosed(false)
            setHint(
              questionWaiting && mentionAt(event.target.value, event.target.selectionStart ?? event.target.value.length)
                ? 'Files cannot go with an answer; attach them to your next message.'
                : null,
            )
            historyAt.current = null
          }}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? 0)}
          onPaste={(event) => {
            const pasted = imageFiles(event.clipboardData?.files)
            if (pasted.length === 0) return
            event.preventDefault()
            void addImages(pasted)
          }}
          onKeyDown={(event) => {
            // A word being composed (an IME) is not a key press: Enter confirms the word.
            if (event.nativeEvent.isComposing || event.keyCode === 229) return
            if (event.key.toLowerCase() === 'm' && event.shiftKey && (event.ctrlKey || event.metaKey) && !event.altKey && onCycleMode) {
              event.preventDefault()
              onCycleMode()
              return
            }
            // Esc closes an open menu first; it stops the turn only when none is open.
            if (event.key === 'Escape' && suggestions.length > 0) {
              event.preventDefault()
              setMenuClosed(true)
              return
            }
            if (suggestions.length > 0 && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
              event.preventDefault()
              setPick((pick + (event.key === 'ArrowDown' ? 1 : suggestions.length - 1)) % suggestions.length)
              return
            }
            if (suggestions.length > 0 && (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey))) {
              const chosen = suggestions[pick]
              // A command typed out in full (or by an alias) runs on Enter; anything else completes.
              if (!(event.key === 'Enter' && chosen.kind === 'command' && chosen.names.some((name) => text === `/${name}`))) {
                event.preventDefault()
                choose(chosen)
                return
              }
            }
            if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && !event.shiftKey && !event.ctrlKey && !event.metaKey && recall(event)) {
              event.preventDefault()
              return
            }
            const sends = enterSends
              ? event.key === 'Enter' && !event.shiftKey
              : event.key === 'Enter' && (event.ctrlKey || event.metaKey)
            if (sends) {
              event.preventDefault()
              send()
            } else if (event.key === 'Escape' && busy) {
              interrupt()
            }
          }}
        />
        {mentions.length > 0 && (
          <div className="chips" aria-label="Attached files">
            {mentions.map((item) => (
              <span key={`${item.kind}:${item.path}`} className="chip">
                @{shown(item)}
                <button
                  aria-label={`Remove ${shown(item)}`}
                  onClick={() => {
                    setMentions(mentions.filter((entry) => entry !== item))
                    setText(text.replace(`@${shown(item)} `, '').replace(`@${shown(item)}`, ''))
                  }}
                >
                  ✕
                </button>
              </span>
            ))}
          </div>
        )}
        {images.length > 0 && (
          <ul className="thumbs" aria-label="Attached images">
            {images.map((image) => (
              <li key={image.id} className="thumb">
                {image.url ? <img src={image.url} alt={image.name} /> : <span className="chip">{image.name}</span>}
                <button
                  type="button"
                  aria-label={`Remove image ${image.name}`}
                  onClick={() => {
                    if (image.url) URL.revokeObjectURL?.(image.url)
                    setImages((current) => current.filter((entry) => entry.id !== image.id))
                  }}
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        )}
        {hint && (
          <div className="composer-hint" role="status">
            {hint}
          </div>
        )}
        <div className="composer-bar">
          {toolbar}
          <div className="spacer" />
          {busy && (
            <button className="stop" onClick={interrupt} title="Interrupt (Esc)">
              ■ Stop
            </button>
          )}
          {busy && !questionWaiting ? (
            <button className="primary" onClick={send} disabled={disabled || !text.trim()}>
              Queue
            </button>
          ) : (
            !busy && (
              <button className="primary" onClick={send} disabled={disabled || !text.trim()}>
                Send
              </button>
            )
          )}
        </div>
      </div>
    </div>
  )
})

/** How hard the model thinks (low, medium, high, or its own default). */
export function EffortPicker({
  value,
  pending,
  disabled,
  onChange,
}: {
  value: 'low' | 'medium' | 'high' | undefined
  /** A new level waits for the turn to end. */
  pending: boolean
  disabled: boolean
  onChange: (value: 'low' | 'medium' | 'high' | undefined) => void
}) {
  return (
    <span className="effort">
      <select
        aria-label="Reasoning effort"
        title="How hard the model thinks. A change restarts the engine on this chat, once the current turn is over."
        value={value ?? ''}
        disabled={disabled}
        onChange={(event) => onChange((event.target.value || undefined) as 'low' | 'medium' | 'high' | undefined)}
      >
        <option value="">Default effort</option>
        <option value="low">Low effort</option>
        <option value="medium">Medium effort</option>
        <option value="high">High effort</option>
      </select>
      {pending && <span className="effort-note muted small">Applies after this turn (restarts the engine on this chat)</span>}
    </span>
  )
}

/** The permission mode and plan mode, in one menu. */
export function ModeMenu({
  mode,
  planMode,
  disabled,
  onMode,
  onPlan,
}: {
  mode: string
  planMode: boolean
  disabled: boolean
  onMode: (mode: string) => void
  onPlan: (on: boolean) => void
}) {
  const [open, setOpen] = useState(false)
  const current = planMode ? 'Plan' : (MODES.find((entry) => entry.id === mode)?.label ?? mode)
  return (
    <div className="menu-anchor">
      <button
        className={`mode-button${mode === 'danger-full-access' ? ' risky' : ''}${planMode ? ' plan' : ''}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label="Permission mode"
        title="Permission mode (Ctrl+Shift+M changes it)"
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        {current} ▾
      </button>
      {open && (
        <div className="menu up" role="listbox" aria-label="Permission modes" onMouseLeave={() => setOpen(false)}>
          {MODES.map((entry) => (
            <button
              key={entry.id}
              role="option"
              aria-selected={!planMode && entry.id === mode}
              className={`menu-item${!planMode && entry.id === mode ? ' current' : ''}`}
              onClick={() => {
                setOpen(false)
                if (planMode) onPlan(false)
                if (entry.id !== mode) onMode(entry.id)
              }}
            >
              <span>{entry.label}</span>
              <span className="muted">{entry.help}</span>
            </button>
          ))}
          <button
            role="option"
            aria-selected={planMode}
            className={`menu-item${planMode ? ' current' : ''}`}
            onClick={() => {
              setOpen(false)
              onPlan(!planMode)
            }}
          >
            <span>Plan {planMode ? '(on)' : ''}</span>
            <span className="muted">Read-only: Neuron plans first, then you ask it to build</span>
          </button>
        </div>
      )}
    </div>
  )
}
