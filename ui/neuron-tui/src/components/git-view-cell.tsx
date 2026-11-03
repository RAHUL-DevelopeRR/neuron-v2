/**
 * A git view in the transcript (a `git-view` block):
 *
 * - a `/diff`: its title, then each file's status mark, path and +/- counts
 *   with its hunks (drawn by tools/diff-viewer.tsx), a note for a binary file
 *   or one left out, and a note when the engine cut the diff short;
 * - what a rewind put back (or, for a dry run, would): files restored,
 *   removed and not restored, the shell commands it could not undo, and the
 *   prompts taken out of the chat.
 */

import { TextAttributes } from '@opentui/core'
import React from 'react'

import { DiffViewer } from './tools/diff-viewer'
import { useTheme } from '../hooks/use-theme'
import { lineCounts, statusMark } from '../state/git-views-store'
import {
  diffFileNote,
  rewoundLines,
  splitHunks,
  type RewoundTone,
} from '../utils/git-view-text'

import type { DiffFile } from '../engine/protocol'
import type { GitView } from '../types/git-view'

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`

const DiffFileSection = ({ file, truncated }: { file: DiffFile; truncated: boolean }) => {
  const theme = useTheme()
  const note = diffFileNote(file, truncated)
  const hunks = file.binary ? [] : splitHunks(file.patch)
  const markColor =
    file.status === 'deleted'
      ? theme.error
      : file.status === 'created'
        ? theme.success
        : theme.warning
  return (
    <box style={{ flexDirection: 'column', width: '100%' }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={markColor} attributes={TextAttributes.BOLD}>
          {statusMark(file.status)}
        </span>
        <span fg={theme.foreground} attributes={TextAttributes.BOLD}>{` ${file.path}`}</span>
        <span fg={theme.success}>{`  +${file.added}`}</span>
        <span fg={theme.error}>{` -${file.removed}`}</span>
        {note ? <span fg={theme.muted}>{`  (${note})`}</span> : null}
      </text>
      {hunks.map((hunk, index) => (
        <box key={index} style={{ flexDirection: 'column', paddingLeft: 2, width: '100%' }}>
          {hunk.header ? (
            <text style={{ wrapMode: 'none', fg: theme.muted }}>{hunk.header}</text>
          ) : null}
          <DiffViewer diffText={hunk.text} />
        </box>
      ))}
    </box>
  )
}

const DiffCell = ({ view }: { view: Extract<GitView, { kind: 'diff' }> }) => {
  const theme = useTheme()
  const { added, removed } = lineCounts(view.files)
  return (
    <box style={{ flexDirection: 'column', width: '100%', gap: 0 }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
          {view.title}
        </span>
        {view.files.length > 0 ? (
          <span fg={theme.muted}>{` · ${plural(view.files.length, 'file')} +${added} -${removed}`}</span>
        ) : null}
      </text>
      {view.files.length === 0 ? (
        <text style={{ fg: theme.muted }}>No changes.</text>
      ) : (
        view.files.map((file) => (
          <DiffFileSection key={file.path} file={file} truncated={view.truncated} />
        ))
      )}
      {view.truncated ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>
          {'Some files or hunks were left out to keep this short. /diff <path> shows one file.'}
        </text>
      ) : null}
    </box>
  )
}

const RewoundCell = ({ view }: { view: Extract<GitView, { kind: 'rewound' }> }) => {
  const theme = useTheme()
  const color = (tone: RewoundTone) =>
    tone === 'warning' ? theme.warning : tone === 'muted' ? theme.muted : theme.foreground
  return (
    <box style={{ flexDirection: 'column', width: '100%' }}>
      {rewoundLines(view).map((line, index) => (
        <text
          key={index}
          style={{ wrapMode: 'word', fg: color(line.tone) }}
          attributes={index === 0 ? TextAttributes.BOLD : undefined}
        >
          {line.text}
        </text>
      ))}
    </box>
  )
}

export const GitViewCell = ({ view }: { view: GitView; availableWidth?: number }) =>
  view.kind === 'diff' ? <DiffCell view={view} /> : <RewoundCell view={view} />
