/**
 * The status bar's pickers, in the panel slot under the chat: the thinking
 * level, the permission mode, the workspace folder and the git branch. Each
 * is a {@link ChoicePanel}; the model has its own picker (model-picker.tsx).
 */

import path from 'path'

import React, { useEffect, useMemo, useState } from 'react'

import { ChoicePanel, type ChoiceRow } from './choice-panel'
import {
  effectivePermissionChoice,
  permissionChoices,
  requestBranches,
  setPermissionChoice,
  setReasoningEffort,
  switchBranch,
  type PermissionChoice,
} from '../engine/status-bar-actions'
import { useEngineStore } from '../state/engine-store'
import { useLiveTurnStore } from '../state/live-turn-store'
import { loadRecentProjects } from '../utils/recent-projects'
import {
  catalogModel,
  effortLabel,
  reasoningEffortsOf,
} from '../utils/status-bar-parts'

import type { BranchInfo, StatusEvent, ModelsEvent } from '../engine/protocol'

interface PickerProps {
  onClose: () => void
  width: number
  maxVisibleRows?: number
}

/** The thinking levels to pick from: the model's own, then its default. */
export function thinkRows(status: StatusEvent | null, models: ModelsEvent | null): ChoiceRow[] {
  if (!status) return []
  const efforts = reasoningEffortsOf(status, catalogModel(models, status.model)) ?? []
  const current = status.reasoning_effort ?? null
  const rows: ChoiceRow[] = efforts.map((effort) => ({
    id: effort,
    label: effortLabel(effort),
    current: current === effort,
  }))
  if (!efforts.includes('default')) {
    rows.push({ id: '', label: 'default', detail: "the model's own level", current: current === null })
  }
  return rows
}

export const ThinkPicker = ({ onClose, width, maxVisibleRows }: PickerProps) => {
  const status = useEngineStore((state) => state.status)
  const models = useEngineStore((state) => state.models)
  const rows = useMemo(() => thinkRows(status, models), [status, models])
  return (
    <ChoicePanel
      title="Thinking level"
      rows={rows}
      width={width}
      maxVisibleRows={maxVisibleRows}
      emptyText="This model has no thinking levels to choose from."
      onClose={onClose}
      onPick={(row) => {
        setReasoningEffort(row.id === '' ? null : row.id)
        onClose()
      }}
    />
  )
}

/**
 * The permission modes to pick from; Full access only when enabled at launch.
 * A choice sent that no status shows yet (`pending`) is the one marked.
 */
export function permissionRows(
  status: StatusEvent | null,
  pending: PermissionChoice | null = null,
): ChoiceRow[] {
  const current = effectivePermissionChoice(status, pending)
  return permissionChoices(status).map((option) => ({
    id: option.id,
    label: option.label,
    detail: option.description,
    current: option.id === current,
  }))
}

export const PermissionPicker = ({ onClose, width, maxVisibleRows }: PickerProps) => {
  const status = useEngineStore((state) => state.status)
  const pending = useEngineStore((state) => state.pendingPermission)
  const rows = useMemo(() => permissionRows(status, pending), [status, pending])
  return (
    <ChoicePanel
      title="Permissions"
      rows={rows}
      width={width}
      maxVisibleRows={maxVisibleRows}
      hint="shift+tab cycles"
      onClose={onClose}
      onPick={(row) => {
        if (!row.current) setPermissionChoice(row.id as PermissionChoice)
        onClose()
      }}
    />
  )
}

/** The recent project folders, the current one first. */
export function workspaceRows(cwd: string | null, recent: { path: string }[]): ChoiceRow[] {
  const seen = new Set<string>()
  const rows: ChoiceRow[] = []
  const add = (folder: string, current: boolean) => {
    const key = path.resolve(folder).toLowerCase()
    if (seen.has(key)) return
    seen.add(key)
    rows.push({ id: folder, label: path.basename(folder) || folder, detail: folder, current })
  }
  if (cwd) add(cwd, true)
  for (const project of recent) add(project.path, false)
  return rows
}

/**
 * What Enter on a workspace row does. Switching restarts the engine, which
 * stops a running turn (or one waiting on a permission or a question), so
 * then it takes a second Enter on the same row, as the branch picker does.
 */
export function workspacePickAction(
  row: Pick<ChoiceRow, 'id' | 'current'>,
  busy: boolean,
  confirming: string | null,
): 'close' | 'confirm' | 'switch' {
  if (row.current) return 'close'
  if (busy && confirming !== row.id) return 'confirm'
  return 'switch'
}

export const WorkspacePicker = ({
  onClose,
  width,
  maxVisibleRows,
  onSwitchProject,
}: PickerProps & { onSwitchProject?: (folder: string) => void }) => {
  const status = useEngineStore((state) => state.status)
  const waiting = useEngineStore(
    (state) => state.permissionQueue.length > 0 || state.pendingQuestion !== null,
  )
  const turnRunning = useLiveTurnStore((state) => state.active)
  const busy = turnRunning || waiting
  const [confirming, setConfirming] = useState<string | null>(null)
  const rows = useMemo(() => workspaceRows(status?.cwd ?? null, loadRecentProjects()), [status?.cwd])
  const confirmingRow = confirming ? rows.find((row) => row.id === confirming) : undefined
  return (
    <ChoicePanel
      title="Workspace"
      rows={rows}
      width={width}
      maxVisibleRows={maxVisibleRows}
      header="Switching restarts the engine in the folder you pick."
      notice={
        confirmingRow
          ? {
              text: `Switching stops the running turn. Enter again to switch to ${confirmingRow.label}.`,
              tone: 'warning',
            }
          : null
      }
      emptyText="No recent folders yet."
      onClose={onClose}
      onMove={() => setConfirming(null)}
      onPick={(row) => {
        switch (workspacePickAction(row, busy, confirming)) {
          case 'confirm':
            setConfirming(row.id)
            return
          case 'switch':
            onSwitchProject?.(row.id)
            break
          case 'close':
            break
        }
        onClose()
      }}
    />
  )
}

function branchDetail(branch: BranchInfo): string | null {
  const parts: string[] = []
  if (branch.upstream) {
    const marks = [
      branch.ahead ? `↑${branch.ahead}` : '',
      branch.behind ? `↓${branch.behind}` : '',
    ].filter(Boolean)
    parts.push(marks.length > 0 ? `${branch.upstream} ${marks.join(' ')}` : branch.upstream)
  }
  return parts.length > 0 ? parts.join(' · ') : null
}

/** The local branches, the current one first, then by their last commit. */
export function branchRows(branches: { current: string | null; branches: BranchInfo[] } | null): ChoiceRow[] {
  if (!branches) return []
  const rows = branches.branches.map((branch) => ({
    id: branch.name,
    label: branch.name,
    detail: branchDetail(branch),
    current: branch.name === branches.current,
  }))
  return [...rows.filter((row) => row.current), ...rows.filter((row) => !row.current)]
}

export const BranchPicker = ({ onClose, width, maxVisibleRows }: PickerProps) => {
  const branches = useEngineStore((state) => state.branches)
  const [supported, setSupported] = useState(true)
  const [confirming, setConfirming] = useState<string | null>(null)
  useEffect(() => {
    useEngineStore.getState().setBranches(null)
    setSupported(requestBranches())
  }, [])
  const rows = useMemo(() => branchRows(branches), [branches])
  const notice = !supported
    ? { text: 'This engine cannot list branches yet; use git in a terminal.', tone: 'muted' as const }
    : confirming
      ? { text: `Switch to ${confirming}? Enter to switch, any arrow to cancel.`, tone: 'warning' as const }
      : branches?.dirty
        ? {
            text: 'The working tree has uncommitted changes: commit or stash them before switching.',
            tone: 'warning' as const,
          }
        : null
  return (
    <ChoicePanel
      title="Branch"
      rows={rows}
      width={width}
      maxVisibleRows={maxVisibleRows}
      notice={notice}
      hint="/branch new <name> creates one"
      emptyText={branches ? 'No local branches.' : 'Reading the branches…'}
      onClose={onClose}
      onMove={() => setConfirming(null)}
      onPick={(row) => {
        if (row.current) {
          onClose()
          return
        }
        if (branches?.dirty) return
        if (confirming !== row.id) {
          setConfirming(row.id)
          return
        }
        switchBranch(row.id, false)
        onClose()
      }}
    />
  )
}
