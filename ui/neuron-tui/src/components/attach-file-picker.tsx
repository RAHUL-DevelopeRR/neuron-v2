/**
 * The in-UI file browser the Attach button opens when no OS file dialog can
 * (over SSH, no desktop, the dialog tool missing or failing). It sits in the
 * panel slot under the chat, built on the project picker's directory listing
 * and path completion, with files listed after the folders.
 *
 *   type      filter the list, or type a path (C:\..., /..., ~/...)
 *   tab       complete the typed path
 *   ↑/↓       move
 *   enter     open a folder, attach a file (again: remove it)
 *   backspace with nothing typed: the parent folder
 *   esc       clear what was typed, then close
 *
 * Every row is clickable, and so are Up and Done.
 */

import { existsSync, statSync } from 'fs'
import os from 'os'
import path from 'path'

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useMemo, useRef, useState } from 'react'

import { Button } from './button'
import { truncateToWidth, visibleWindow } from './model-picker'
import { useTheme } from '../hooks/use-theme'
import { useChatStore } from '../state/chat-store'
import { attachFiles } from '../utils/attach-files'
import { getDirectories, type DirectoryEntry } from '../utils/directory-browser'
import { GLYPHS } from '../utils/glyphs'
import { getPathCompletion } from '../utils/path-completion'
import { isPlainEnterKey } from '../utils/terminal-enter-detection'
import { BORDER_CHARS } from '../utils/ui-constants'

/** Whether what was typed is a path to go to rather than a filter. */
export function isTypedPath(query: string, platform: NodeJS.Platform = process.platform): boolean {
  const trimmed = query.trim()
  if (!trimmed) return false
  if (trimmed.startsWith('~') || trimmed.startsWith('./') || trimmed.startsWith('../')) return true
  if (platform === 'win32') {
    return /^[A-Za-z]:/.test(trimmed) || /^[\\/]/.test(trimmed) || /^\.\.?\\/.test(trimmed)
  }
  return trimmed.startsWith('/')
}

/** A typed path as an absolute one: `~` is home, a relative path is under `currentDir`. */
export function resolveTypedPath(query: string, currentDir: string, homeDir: string = os.homedir()): string {
  const trimmed = query.trim()
  if (trimmed.startsWith('~')) return path.join(homeDir, trimmed.slice(1))
  return path.resolve(currentDir, trimmed)
}

/** The listing's rows: filtered by name unless a path is being typed. */
export function filterEntries(entries: DirectoryEntry[], query: string): DirectoryEntry[] {
  const trimmed = query.trim().toLowerCase()
  if (!trimmed || isTypedPath(query)) return entries
  return entries.filter((entry) => entry.isParent || entry.name.toLowerCase().includes(trimmed))
}

/** Characters a key adds to the typed text, or `null` for a key that types nothing. */
export function typedText(key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta' | 'option'> & { sequence?: string }): string | null {
  if (key.ctrl || key.meta || key.option) return null
  const sequence = key.sequence ?? ''
  if (!sequence || sequence.startsWith('\u001b')) return null
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(sequence)) return null
  return sequence
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const kb = bytes / 1024
  if (kb < 1024) return `${kb.toFixed(1)} KB`
  return `${(kb / 1024).toFixed(1)} MB`
}

function fileSize(filePath: string): number | null {
  try {
    return statSync(filePath).size
  } catch {
    return null
  }
}

export const AttachFilePicker = ({
  startDir,
  onClose,
  width,
  maxVisibleRows = 8,
}: {
  startDir: string
  onClose: () => void
  width: number
  maxVisibleRows?: number
}) => {
  const theme = useTheme()
  const [currentDir, setCurrentDir] = useState(startDir)
  const [query, setQueryState] = useState('')
  // Keys can arrive faster than a render: each reads and writes the latest text.
  const queryRef = useRef('')
  const setQuery = useCallback((next: string) => {
    queryRef.current = next
    setQueryState(next)
  }, [])
  const [focus, setFocus] = useState(0)
  const [notice, setNotice] = useState<string | null>(null)
  const pendingAttachments = useChatStore((state) => state.pendingAttachments)
  const removePendingAttachment = useChatStore((state) => state.removePendingAttachment)

  const attachedPaths = useMemo(
    () =>
      new Set(
        pendingAttachments
          .filter((attachment) => attachment.kind !== 'text')
          .map((attachment) => path.resolve(attachment.path)),
      ),
    [pendingAttachments],
  )
  const entries = useMemo(() => getDirectories(currentDir, { includeFiles: true }), [currentDir])
  const rows = useMemo(() => filterEntries(entries, query), [entries, query])
  const clamped = Math.min(focus, Math.max(0, rows.length - 1))

  const goTo = useCallback((dir: string) => {
    setCurrentDir(dir)
    setQuery('')
    setFocus(0)
    setNotice(null)
  }, [setQuery])

  const goUp = useCallback(() => {
    const parent = path.dirname(currentDir)
    if (parent !== currentDir) goTo(parent)
  }, [currentDir, goTo])

  const toggleFile = useCallback(
    (filePath: string) => {
      const resolved = path.resolve(filePath)
      if (attachedPaths.has(resolved)) {
        removePendingAttachment(resolved)
        setNotice(`Removed ${path.basename(resolved)}`)
        return
      }
      // A file that can't be attached says why in the attachments banner.
      attachFiles([resolved])
      setNotice(null)
    },
    [attachedPaths, removePendingAttachment],
  )

  const open = useCallback(
    (entry: DirectoryEntry | undefined) => {
      if (!entry) return
      if (entry.isFile) toggleFile(entry.path)
      else goTo(entry.path)
    },
    [goTo, toggleFile],
  )

  const openTypedPath = useCallback(() => {
    const typed = queryRef.current
    const target = resolveTypedPath(typed, currentDir)
    try {
      if (existsSync(target)) {
        if (statSync(target).isDirectory()) goTo(target)
        else {
          toggleFile(target)
          setQuery('')
        }
        return
      }
    } catch {
      // Unreadable: said below.
    }
    setNotice(`${typed.trim()} was not found.`)
  }, [currentDir, goTo, setQuery, toggleFile])

  const complete = useCallback(() => {
    const current = queryRef.current
    if (!current.trim()) return
    const typed = isTypedPath(current) ? resolveTypedPath(current, currentDir) : path.join(currentDir, current)
    const completed = getPathCompletion(typed, os.homedir(), { includeFiles: true })
    if (!completed) return
    if (/[\\/]$/.test(completed)) {
      const dir = completed.replace(/[\\/]+$/, '') || completed
      try {
        if (statSync(dir).isDirectory()) {
          goTo(dir)
          return
        }
      } catch {
        // Fall through to showing the completion.
      }
    }
    setQuery(completed)
  }, [currentDir, goTo, setQuery])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const consume = () => key.preventDefault?.()
        if (key.ctrl && key.name === 'c') {
          consume()
          onClose()
          return
        }
        if (key.name === 'escape') {
          consume()
          if (queryRef.current) setQuery('')
          else onClose()
          return
        }
        if (isPlainEnterKey(key)) {
          consume()
          if (isTypedPath(queryRef.current)) openTypedPath()
          else open(rows[clamped])
          return
        }
        switch (key.name) {
          case 'up':
            consume()
            setNotice(null)
            setFocus(Math.max(0, clamped - 1))
            return
          case 'down':
            consume()
            setNotice(null)
            setFocus(Math.min(Math.max(0, rows.length - 1), clamped + 1))
            return
          case 'tab':
            consume()
            complete()
            return
          case 'backspace':
            consume()
            if (queryRef.current) setQuery(queryRef.current.slice(0, -1))
            else goUp()
            return
        }
        const text = typedText(key)
        if (text !== null) {
          consume()
          const next = queryRef.current + text
          setQuery(next)
          // The first match, not the parent folder that is always listed.
          const firstMatch = filterEntries(entries, next).findIndex((entry) => !entry.isParent)
          setFocus(Math.max(0, firstMatch))
        }
      },
      [clamped, complete, entries, goUp, onClose, open, openTypedPath, rows, setQuery],
    ),
  )

  const panelWidth = Math.max(24, width)
  const rowWidth = Math.max(8, panelWidth - 4 - 2)
  const [start, end] = visibleWindow(rows.length, clamped, maxVisibleRows)
  const attachedCount = attachedPaths.size

  return (
    <box
      title={' Attach files '}
      titleAlignment="center"
      style={{
        width: panelWidth,
        borderStyle: 'single',
        borderColor: theme.border,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      <box style={{ flexDirection: 'row', width: '100%' }}>
        <box style={{ flexGrow: 1, flexShrink: 1, overflow: 'hidden' }}>
          <text style={{ wrapMode: 'none', fg: theme.muted }}>
            {truncateToWidth(currentDir, Math.max(4, rowWidth - 16))}
          </text>
        </box>
        <Button onClick={goUp} style={{ flexShrink: 0, paddingLeft: 1 }}>
          <text style={{ wrapMode: 'none', fg: theme.info }}>[Up]</text>
        </Button>
        <Button onClick={onClose} style={{ flexShrink: 0, paddingLeft: 1 }}>
          <text style={{ wrapMode: 'none', fg: theme.primary }}>[Done]</text>
        </Button>
      </box>
      <text style={{ wrapMode: 'none' }}>
        <span fg={theme.primary}>{`${GLYPHS.pointer} `}</span>
        {query ? (
          <span fg={theme.foreground}>{truncateToWidth(query, rowWidth - 3)}</span>
        ) : (
          <span fg={theme.muted}>type to filter, or a path</span>
        )}
      </text>
      {rows.length === 0 ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>
          {query ? 'Nothing here matches.' : 'This folder is empty.'}
        </text>
      ) : (
        rows.slice(start, end).map((entry, offset) => {
          const index = start + offset
          const focused = index === clamped
          const attached = entry.isFile === true && attachedPaths.has(path.resolve(entry.path))
          const icon = entry.isParent
            ? GLYPHS.folderOpen
            : entry.isFile
              ? attached
                ? GLYPHS.check
                : GLYPHS.file
              : GLYPHS.folder
          const name = entry.isFile || entry.isParent ? entry.name : `${entry.name}${path.sep}`
          const size = entry.isFile ? fileSize(entry.path) : null
          const detail = attached ? 'attached' : size !== null ? formatSize(size) : ''
          const label = truncateToWidth(`${icon} ${name}`, Math.max(4, rowWidth - detail.length - 4))
          return (
            <Button
              key={entry.path}
              onClick={() => {
                setFocus(index)
                open(entry)
              }}
            >
              <text style={{ wrapMode: 'none' }}>
                <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
                <span
                  fg={attached ? theme.success : theme.foreground}
                  attributes={focused ? TextAttributes.BOLD : undefined}
                >
                  {label}
                </span>
                {detail ? <span fg={attached ? theme.success : theme.muted}>{`  ${detail}`}</span> : null}
              </text>
            </Button>
          )
        })
      )}
      {notice ? <text style={{ wrapMode: 'word', fg: theme.warning }}>{notice}</text> : null}
      <text style={{ wrapMode: 'word', fg: theme.muted }}>
        {rows.length > maxVisibleRows ? `${clamped + 1}/${rows.length} · ` : ''}
        {attachedCount > 0 ? `${attachedCount} attached · ` : ''}
        {'enter open/attach · tab complete · backspace up · esc done'}
      </text>
    </box>
  )
}
