/**
 * The keyboard shortcuts of the chat shell: one table feeds the key handler
 * and the shortcuts sheet, so they cannot disagree. `Mod` is Ctrl, or Cmd on a
 * Mac (the handler takes either).
 */

export type ShortcutId = 'palette' | 'newChat' | 'newWorktreeChat' | 'nextWaiting' | 'previousChat' | 'nextChat' | 'shortcuts' | 'cycleMode'

export interface Shortcut {
  id: ShortcutId
  /** The key as `KeyboardEvent.key` names it, lowercased. */
  key: string
  shift?: boolean
  alt?: boolean
  title: string
}

export const SHORTCUTS: readonly Shortcut[] = [
  { id: 'palette', key: 'k', title: 'Open the command palette' },
  { id: 'newChat', key: 'n', title: 'New chat' },
  { id: 'newWorktreeChat', key: 'n', shift: true, title: 'New chat in its own copy of the project' },
  { id: 'nextWaiting', key: 'j', title: 'Go to the chat that needs you' },
  { id: 'previousChat', key: 'arrowup', alt: true, title: 'Previous chat' },
  { id: 'nextChat', key: 'arrowdown', alt: true, title: 'Next chat' },
  { id: 'shortcuts', key: '/', title: 'Show these shortcuts' },
  { id: 'cycleMode', key: 'm', shift: true, title: 'Cycle the permission mode' },
]

/** Shortcuts that already existed outside this table, listed on the sheet too. */
export const OTHER_SHORTCUTS: ReadonlyArray<{ keys: string; title: string }> = [
  { keys: 'Mod+B', title: 'Show or hide the side panel' },
  { keys: 'Mod+Shift+B', title: 'Collapse or expand the sidebar' },
  { keys: 'Mod+`', title: 'Show or hide the terminal' },
  { keys: 'Mod+,', title: 'Settings' },
  { keys: 'Esc', title: 'Close a page or a menu' },
]

/** Which shortcut a key press is, if any (Ctrl or Cmd held). */
export function shortcutFor(event: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey'>): ShortcutId | null {
  if (!(event.ctrlKey || event.metaKey)) return null
  const key = event.key.toLowerCase()
  const found = SHORTCUTS.find((shortcut) => shortcut.key === key && (shortcut.shift ?? false) === event.shiftKey && (shortcut.alt ?? false) === event.altKey)
  return found?.id ?? null
}

const ARROW: Record<string, string> = { arrowup: '↑', arrowdown: '↓' }

/** The keys as the person reads them: `Ctrl+Shift+N`, or `⌘⇧N` on a Mac. */
export function keysOf(shortcut: Shortcut, mac = false): string {
  const key = ARROW[shortcut.key] ?? shortcut.key.toUpperCase()
  const parts = [mac ? '⌘' : 'Ctrl', ...(shortcut.alt ? [mac ? '⌥' : 'Alt'] : []), ...(shortcut.shift ? [mac ? '⇧' : 'Shift'] : []), key]
  return mac ? parts.join('') : parts.join('+')
}

/** `Mod+B` as `Ctrl+B` or `⌘B`. */
export function displayKeys(keys: string, mac = false): string {
  if (!keys.startsWith('Mod')) return keys
  const rest = keys.slice(4).replace('Shift+', mac ? '⇧' : 'Shift+')
  return mac ? `⌘${rest}` : `Ctrl+${rest}`
}

export const isMac = (): boolean => typeof navigator !== 'undefined' && /mac/i.test(navigator.platform ?? '')

export function shortcutKeys(id: ShortcutId): string {
  const shortcut = SHORTCUTS.find((entry) => entry.id === id)!
  return keysOf(shortcut, isMac())
}
