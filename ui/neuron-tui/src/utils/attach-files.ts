/**
 * The Attach button, /attach and a file dropped on the terminal: open the OS
 * file dialog, or the in-UI file browser when none can open, and attach what
 * was picked. Each file is routed by utils/attach-routing.ts; a file that
 * cannot be attached says why in the attachments banner instead. A file the
 * model reads by path that is outside the engine's workspace is copied into
 * `<workspace>/.neuron/attachments/<id>/` (git-ignored) and the copy attached.
 */

import { closeSync, existsSync, mkdirSync, openSync, readSync, realpathSync, statSync, writeFileSync } from 'fs'
import { copyFile } from 'fs/promises'
import os from 'os'
import path from 'path'

import {
  addPendingFileCopy,
  addPendingFileFromPath,
  addPendingFileReference,
  validateAndAddImage,
} from './pending-attachments'
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
import { useChatStore } from '../state/chat-store'
import { useEngineStore } from '../state/engine-store'

import type { PendingAttachment } from '../types/store'

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

/** A NUL byte in the first 8 KB, as the engine's `read_file` judges binary. */
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

/**
 * What identifies an attached file: its real path, case-folded on Windows,
 * so `readme.md` and `README.md` (or a path through a link) are one file.
 * Attachments keep the path as picked; only comparisons use this.
 */
export function attachmentKey(filePath: string, platform: NodeJS.Platform = process.platform): string {
  const real = realPath(filePath)
  return platform === 'win32' ? real.toLowerCase() : real
}

/** The key of each file or folder already attached (a copy counts as its source), to its stored path. */
export function attachedKeys(
  attachments: readonly PendingAttachment[] = useChatStore.getState().pendingAttachments,
): Map<string, string> {
  const keys = new Map<string, string>()
  for (const attachment of attachments) {
    if (attachment.kind === 'text') continue
    const source = attachment.kind === 'file' ? (attachment.sourcePath ?? attachment.path) : attachment.path
    keys.set(attachmentKey(source), attachment.path)
  }
  return keys
}

/** Where copies of files picked outside the workspace go, under the workspace. */
export const ATTACHMENTS_DIR = path.join('.neuron', 'attachments')

const ATTACHMENTS_GITIGNORE =
  '# Neuron: copies of files attached from outside this project. Never commit them.\n*\n'

/**
 * A new path for a copy of `source`: its own folder, so the copy keeps the
 * file's name and two files with one name never collide.
 */
export function copyDestination(root: string, source: string): string {
  const id = crypto.randomUUID().replace(/-/g, '').slice(0, 12)
  return path.join(root, ATTACHMENTS_DIR, id, path.basename(source))
}

/** Makes the attachments folder, git-ignored, before the first copy into it. */
function prepareAttachmentsDir(root: string): void {
  const dir = path.join(root, ATTACHMENTS_DIR)
  mkdirSync(dir, { recursive: true })
  const gitignore = path.join(dir, '.gitignore')
  if (!existsSync(gitignore)) writeFileSync(gitignore, ATTACHMENTS_GITIGNORE)
}

export type CopyFile = (from: string, to: string) => Promise<void>

const defaultCopy: CopyFile = (from, to) => copyFile(from, to)

const pendingCopies = new Set<Promise<boolean>>()

/** Resolves once every copy started so far has finished (for tests; the send waits on the card's status). */
export async function waitForAttachCopies(): Promise<void> {
  while (pendingCopies.size > 0) await Promise.all([...pendingCopies])
}

function copyRefusal(filename: string, reason: string): string {
  return `${filename} was not attached: it could not be copied into the project (${reason}).`
}

export interface AttachOutcome {
  attached: string[]
  /** Of `attached`, the files outside the workspace being copied into it. */
  copied: string[]
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
 * Nothing is run: a path is stat'ed and, for a text file, read; a file the
 * model reads by path from outside the workspace is copied into it.
 */
export function attachFiles(
  paths: string[],
  options: { root?: string; imagesReachModel?: boolean; copy?: CopyFile } = {},
): AttachOutcome {
  const root = options.root ?? attachWorkspaceRoot()
  const realRoot = realPath(root)
  const imagesReachModel = options.imagesReachModel ?? IMAGES_REACH_THE_MODEL
  const copy = options.copy ?? defaultCopy
  const outcome: AttachOutcome = { attached: [], copied: [], refused: [] }
  const known = attachedKeys()

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
    const key = attachmentKey(resolved)
    if (known.has(key)) {
      // Already attached, maybe spelled differently: nothing to add.
      outcome.attached.push(resolved)
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
      case 'copy': {
        let destination: string
        try {
          prepareAttachmentsDir(root)
          destination = copyDestination(root, resolved)
          mkdirSync(path.dirname(destination), { recursive: true })
        } catch (error) {
          const code = (error as { code?: string }).code
          outcome.refused.push(copyRefusal(filename, code ?? String(error)))
          continue
        }
        const pending = addPendingFileCopy(
          resolved,
          destination,
          route.tool,
          stats.size,
          () => copy(resolved, destination),
          (reason) => {
            logger.warn({ reason }, '[attach] copy into the workspace failed')
            showAttachNotice(copyRefusal(filename, reason))
          },
        )
        pendingCopies.add(pending)
        void pending.finally(() => pendingCopies.delete(pending))
        outcome.copied.push(resolved)
        break
      }
      case 'refuse':
        outcome.refused.push(route.reason)
        continue
    }
    known.set(key, resolved)
    outcome.attached.push(resolved)
  }

  for (const reason of outcome.refused) showAttachNotice(reason)
  if (outcome.attached.length > 0) {
    const count = outcome.attached.length
    const copies = outcome.copied.length
    const message =
      count === 1
        ? `Attached ${path.basename(outcome.attached[0]!)}${copies === 1 ? ', copied into the project so Neuron can read it' : ''}`
        : `Attached ${count} files${copies > 0 ? `, ${copies} copied into the project so Neuron can read them` : ''}`
    showClipboardMessage(message, { durationMs: 3000 })
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
