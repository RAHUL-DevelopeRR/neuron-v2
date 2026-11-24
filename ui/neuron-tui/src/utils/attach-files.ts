/**
 * The Attach button and /attach: open the OS file dialog, or the in-UI file
 * browser when none can open, and attach what was picked. Each file is routed
 * by utils/attach-routing.ts; a file that cannot be attached says why in the
 * attachments banner instead.
 */

import { closeSync, openSync, readSync, realpathSync, statSync } from 'fs'
import os from 'os'
import path from 'path'

import { addPendingFileFromPath, addPendingFileReference, validateAndAddImage } from './pending-attachments'
import { routeAttachment, isPathInside, type AttachFacts } from './attach-routing'
import { showClipboardMessage } from './clipboard'
import {
  buildDialogCommand,
  fileDialogAvailability,
  isDialogOpen,
  runFileDialog,
  type DialogSpawner,
} from './file-dialog'
import { logger } from './logger'
import { IS_TEST } from './runtime-env'
import { IMAGES_REACH_THE_MODEL } from '../engine/attachments'
import { tryGetProjectRoot } from '../project-files'
import { useAttachStore } from '../state/attach-store'
import { useEngineStore } from '../state/engine-store'

/** How long a "not attached" line stays in the banner. */
export const ATTACH_NOTICE_MS = 12_000

/** The engine's workspace: the folder its tools read in. */
export function attachWorkspaceRoot(): string {
  return useEngineStore.getState().status?.cwd ?? tryGetProjectRoot() ?? process.cwd()
}

function realPath(target: string): string {
  try {
    return realpathSync.native(target)
  } catch {
    return path.resolve(target)
  }
}

/**
 * A picked or typed path as an absolute one, taken literally: only a leading
 * `~` is expanded. No un-escaping, since on Windows `\(` is a separator and
 * a folder name (`C:\ws\(final)\deck.pptx`).
 */
export function resolveAttachPath(picked: string, root: string, homeDir: string = os.homedir()): string {
  if (picked === '~') return homeDir
  if (picked.startsWith('~/') || picked.startsWith('~\\')) {
    return path.resolve(homeDir, picked.slice(2))
  }
  return path.resolve(root, picked)
}

/** A NUL byte in the first 8 KB, as pending-attachments.ts judges binary. */
function looksBinary(filePath: string): boolean {
  let fd: number | null = null
  try {
    fd = openSync(filePath, 'r')
    const buffer = Buffer.alloc(8192)
    const read = readSync(fd, buffer, 0, buffer.length, 0)
    return buffer.subarray(0, read).includes(0)
  } catch {
    return true
  } finally {
    if (fd !== null) closeSync(fd)
  }
}

export interface AttachOutcome {
  attached: string[]
  refused: string[]
}

/** Shows a "not attached" line in the banner for a while. */
export function showAttachNotice(text: string): void {
  const store = useAttachStore.getState()
  const id = store.addNotice(text)
  const timer = setTimeout(() => useAttachStore.getState().removeNotice(id), ATTACH_NOTICE_MS)
  ;(timer as { unref?: () => void }).unref?.()
}

/**
 * Attach each path (absolute, or relative to the workspace; `~` is home).
 * Nothing is run: a path is only stat'ed and, for a text file, read.
 */
export function attachFiles(
  paths: string[],
  options: { root?: string; imagesReachModel?: boolean } = {},
): AttachOutcome {
  const root = options.root ?? attachWorkspaceRoot()
  const realRoot = realPath(root)
  const imagesReachModel = options.imagesReachModel ?? IMAGES_REACH_THE_MODEL
  const outcome: AttachOutcome = { attached: [], refused: [] }

  for (const picked of paths) {
    const resolved = resolveAttachPath(picked, root)
    const filename = path.basename(resolved) || resolved
    let stats
    try {
      stats = statSync(resolved)
    } catch {
      outcome.refused.push(`${filename} was not attached: it was not found.`)
      continue
    }
    const isDirectory = stats.isDirectory()
    const facts: AttachFacts = {
      path: resolved,
      isDirectory,
      size: stats.size,
      isBinary: isDirectory || stats.size === 0 ? false : looksBinary(resolved),
      insideWorkspace: isPathInside(realPath(resolved), realRoot),
      imagesReachModel,
      workspaceRoot: root,
    }
    const route = routeAttachment(facts)
    switch (route.kind) {
      case 'image':
        validateAndAddImage(resolved, root).catch((error) => {
          logger.error({ error }, '[attach] image failed')
        })
        break
      case 'text':
        addPendingFileFromPath(resolved, false)
        break
      case 'folder':
        addPendingFileFromPath(resolved, true)
        break
      case 'reference':
        addPendingFileReference(resolved, route.tool, stats.size)
        break
      case 'refuse':
        outcome.refused.push(route.reason)
        continue
    }
    outcome.attached.push(resolved)
  }

  for (const reason of outcome.refused) showAttachNotice(reason)
  if (outcome.attached.length > 0) {
    const count = outcome.attached.length
    showClipboardMessage(
      count === 1
        ? `Attached ${path.basename(outcome.attached[0]!)}`
        : `Attached ${count} files`,
      { durationMs: 3000 },
    )
  }
  return outcome
}

export interface StartAttachDeps {
  platform?: NodeJS.Platform
  env?: Record<string, string | undefined>
  which?: (command: string) => string | null
  spawner?: DialogSpawner
  root?: string
}

export type StartAttachResult = 'dialog' | 'picker' | 'busy'

/**
 * What the Attach button and a bare /attach do: the OS dialog when one can
 * open, else the in-UI browser (also when the dialog fails to open).
 */
export async function startAttach(deps: StartAttachDeps = {}): Promise<StartAttachResult> {
  const store = useAttachStore.getState()
  if (store.dialogOpen || isDialogOpen()) {
    showClipboardMessage('The file dialog is already open', { durationMs: 3000 })
    return 'busy'
  }
  const root = deps.root ?? attachWorkspaceRoot()
  if (store.pickerOpen) return 'picker'

  // Tests never open a real dialog unless they bring their own spawner.
  const availability =
    IS_TEST && !deps.spawner
      ? ({ available: false, reason: 'test run' } as const)
      : fileDialogAvailability(
          deps.platform ?? process.platform,
          deps.env ?? process.env,
          deps.which ?? ((command) => Bun.which(command)),
        )
  if (!availability.available) {
    logger.debug({ reason: availability.reason }, '[attach] using the in-UI browser')
    store.openPicker(root)
    return 'picker'
  }

  const command = buildDialogCommand(availability.tool, availability.executable, root)
  store.setDialogOpen(true)
  showClipboardMessage('Pick files in the dialog that opened', { durationMs: 4000 })
  try {
    const result = await runFileDialog(command, { spawner: deps.spawner })
    switch (result.status) {
      case 'picked':
        attachFiles(result.paths, { root })
        break
      case 'cancelled':
        showClipboardMessage(null)
        break
      case 'failed':
        logger.warn({ reason: result.reason }, '[attach] file dialog failed')
        showClipboardMessage(null)
        showAttachNotice(`The file dialog did not work (${result.reason}). Pick files in the list instead.`)
        useAttachStore.getState().openPicker(root)
        return 'picker'
    }
    return 'dialog'
  } finally {
    useAttachStore.getState().setDialogOpen(false)
  }
}
