/**
 * `/commit` without a message: the engine's drafted message and what the
 * commit would take, in the composer's place. Nothing is committed until
 * Enter here:
 *
 *   enter  commit with this message
 *   e      edit: `/commit <message>` goes into the input, to change and send
 *   esc    cancel (also Ctrl-C)
 *
 * Enter and e are ignored for the first 300 ms, so the Enter that sent
 * `/commit` (or one held down) cannot commit a message nobody has read.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { truncateToWidth } from './model-picker'
import {
  acceptCommitProposal,
  cancelCommitProposal,
  editCommitProposal,
} from '../engine/git-views'
import { useTheme } from '../hooks/use-theme'
import { lineCounts, statusMark, type CommitProposal } from '../state/git-views-store'
import { BORDER_CHARS } from '../utils/ui-constants'

export const COMMIT_KEY_GUARD_MS = 300
/** Lines of the message shown before "…". */
const MESSAGE_MAX_LINES = 12

export type CommitKeyAction = 'commit' | 'edit' | 'cancel' | 'none'

/** What a key does on the commit card. */
export function resolveCommitProposalKey(
  key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>,
  options: { armed: boolean },
): CommitKeyAction {
  if (key.name === 'escape' || (key.ctrl && !key.meta && key.name === 'c')) return 'cancel'
  if (!options.armed || key.ctrl || key.meta) return 'none'
  if (key.name === 'return' || key.name === 'enter') return 'commit'
  if (key.name === 'e') return 'edit'
  return 'none'
}

/** Runs a key's action on the proposal in the store. */
export function applyCommitKeyAction(action: CommitKeyAction): void {
  if (action === 'commit') acceptCommitProposal()
  else if (action === 'edit') editCommitProposal()
  else if (action === 'cancel') cancelCommitProposal()
}

export const CommitProposalCard = ({
  proposal,
  width,
  maxVisibleFiles = 8,
}: {
  proposal: CommitProposal
  width: number
  maxVisibleFiles?: number
}) => {
  const theme = useTheme()
  const [armed, setArmed] = useState(false)

  useEffect(() => {
    setArmed(false)
    const timer = setTimeout(() => setArmed(true), COMMIT_KEY_GUARD_MS)
    return () => clearTimeout(timer)
  }, [proposal])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = resolveCommitProposalKey(key, { armed })
        if (action === 'none') return
        key.preventDefault?.()
        applyCommitKeyAction(action)
      },
      [armed],
    ),
  )

  const { added, removed } = lineCounts(proposal.files)
  const shown = proposal.files.slice(0, maxVisibleFiles)
  const hidden = proposal.files.length - shown.length
  const messageLines = proposal.message.replace(/\r\n/g, '\n').trimEnd().split('\n')
  const cutMessage = messageLines.length > MESSAGE_MAX_LINES
  // Inside the border and padding.
  const innerWidth = Math.max(10, Math.max(20, width) - 4)
  const option = (keyLabel: string, label: string, last = false) => (
    <span>
      <span fg={theme.primary} attributes={TextAttributes.BOLD}>
        {keyLabel}
      </span>
      <span fg={theme.foreground}>{last ? ` ${label}` : ` ${label}   `}</span>
    </span>
  )

  return (
    <box
      title=" Commit "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: theme.primary,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      <text style={{ wrapMode: 'word' }}>
        <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
          {proposal.branch ? `On ${proposal.branch}` : 'Detached HEAD'}
        </span>
        <span fg={theme.muted}>{` · ${proposal.files.length} ${proposal.files.length === 1 ? 'file' : 'files'} `}</span>
        <span fg={theme.success}>{`+${added}`}</span>
        <span fg={theme.error}>{` -${removed}`}</span>
        <span fg={theme.muted}>
          {proposal.stage_all
            ? ' · nothing is staged, so it takes every change'
            : ' · only what is staged'}
        </span>
      </text>
      {shown.map((file) => {
        const counts = `  +${file.added} -${file.removed}`
        return (
          <text key={file.path} style={{ wrapMode: 'none' }}>
            <span fg={theme.muted}>{`${statusMark(file.status)} `}</span>
            <span fg={theme.foreground}>
              {truncateToWidth(file.path, Math.max(4, innerWidth - 2 - counts.length))}
            </span>
            <span fg={theme.muted}>{counts}</span>
          </text>
        )
      })}
      {hidden > 0 ? (
        <text style={{ fg: theme.muted }}>{`and ${hidden} more`}</text>
      ) : null}
      <box style={{ flexDirection: 'column', paddingTop: 1 }}>
        {messageLines.slice(0, MESSAGE_MAX_LINES).map((line, index) => (
          <text
            key={index}
            style={{ wrapMode: 'word', fg: theme.foreground }}
            attributes={index === 0 ? TextAttributes.BOLD : undefined}
          >
            {line || ' '}
          </text>
        ))}
        {cutMessage ? <text style={{ fg: theme.muted }}>…</text> : null}
      </box>
      <box style={{ flexDirection: 'column', paddingTop: 1 }}>
        <text>
          {option('enter', 'commit')}
          {option('e', 'edit the message')}
          {option('esc', 'cancel', true)}
        </text>
      </box>
    </box>
  )
}
