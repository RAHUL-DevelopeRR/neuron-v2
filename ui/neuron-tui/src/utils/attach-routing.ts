/**
 * Where a file picked with the Attach button (or /attach) goes. Pure: the
 * caller reads the file's facts (attach-files.ts) and acts on the route.
 *
 * - Images go through the image path, while images reach the model
 *   (IMAGES_REACH_THE_MODEL); until then they are refused with a message.
 * - Text and code files are read and inlined into the prompt, as a dragged or
 *   pasted file is, within the same size limits (pending-attachments.ts).
 * - Office and PDF files, and binary or oversized files, go by reference: the
 *   prompt names the path and the model reads it with `read_document` or
 *   `read_file`. The engine reads only inside its workspace (the tools check
 *   the boundary), so a reference to a file outside it is refused with a
 *   message instead of attaching a path the model cannot open.
 */

import path from 'path'

import { isImageFile } from './image-handler'

/** The Office and PDF files the engine's `read_document` tool converts to Markdown. */
export const DOCUMENT_EXTENSIONS: ReadonlySet<string> = new Set([
  '.docx',
  '.pptx',
  '.xlsx',
  '.xlsm',
  '.xls',
  '.ods',
  '.pdf',
])

/** A text file larger than this is not read into the prompt (as pending-attachments.ts). */
export const MAX_INLINE_FILE_BYTES = 1024 * 1024

export type ReferenceTool = 'read_document' | 'read_file'

export type AttachRoute =
  | { kind: 'image' }
  | { kind: 'text' }
  | { kind: 'folder' }
  | { kind: 'reference'; tool: ReferenceTool }
  | { kind: 'refuse'; reason: string }

export interface AttachFacts {
  /** The absolute path. */
  path: string
  isDirectory: boolean
  /** Bytes. */
  size: number
  /** A NUL byte in the first few KB. */
  isBinary: boolean
  /** Inside the engine's workspace, where its tools can read it. */
  insideWorkspace: boolean
  /** Whether a picked image can reach the model (IMAGES_REACH_THE_MODEL). */
  imagesReachModel: boolean
  /** The workspace folder, for the outside-the-workspace message. */
  workspaceRoot: string
}

/** Said when an image is picked while images cannot reach the model. */
export function imageRefusal(filename: string): string {
  return `${filename} was not attached: the model can't see images in Neuron yet. Describe it in words, or paste the text it shows.`
}

/** Said when a file the model would read by path is outside the workspace. */
export function outsideWorkspaceRefusal(filename: string, workspaceRoot: string): string {
  return `${filename} was not attached: it is outside the project folder, and Neuron reads files only inside ${workspaceRoot}. Copy it into the project and attach it again.`
}

/** What the prompt says about a file attached by reference. */
export function fileReferenceText(filePath: string, tool: ReferenceTool): string {
  const what =
    tool === 'read_document'
      ? 'an attached document: read it with the read_document tool'
      : 'an attached file: read it with the read_file tool'
  return `[File: ${filePath}]\n(${what}.)`
}

export function isDocumentFile(filePath: string): boolean {
  return DOCUMENT_EXTENSIONS.has(path.extname(filePath).toLowerCase())
}

export function routeAttachment(facts: AttachFacts): AttachRoute {
  const filename = path.basename(facts.path) || facts.path
  if (facts.isDirectory) return { kind: 'folder' }
  if (isImageFile(facts.path)) {
    return facts.imagesReachModel
      ? { kind: 'image' }
      : { kind: 'refuse', reason: imageRefusal(filename) }
  }
  const byReference = (tool: ReferenceTool): AttachRoute =>
    facts.insideWorkspace
      ? { kind: 'reference', tool }
      : { kind: 'refuse', reason: outsideWorkspaceRefusal(filename, facts.workspaceRoot) }
  if (isDocumentFile(facts.path)) return byReference('read_document')
  if (!facts.isBinary && facts.size <= MAX_INLINE_FILE_BYTES) return { kind: 'text' }
  return byReference('read_file')
}

/**
 * Whether `child` is `parent` or inside it, compared as the engine's boundary
 * check does: on a separator boundary (`C:\ws2` is not inside `C:\ws`), and
 * case-insensitively on Windows. Both should already be real paths.
 */
export function isPathInside(
  child: string,
  parent: string,
  platform: NodeJS.Platform = process.platform,
): boolean {
  const lib = platform === 'win32' ? path.win32 : path.posix
  const fold = (value: string) => (platform === 'win32' ? value.toLowerCase() : value)
  const resolvedParent = fold(lib.resolve(parent))
  const resolvedChild = fold(lib.resolve(child))
  if (resolvedChild === resolvedParent) return true
  const prefix = resolvedParent.endsWith(lib.sep) ? resolvedParent : resolvedParent + lib.sep
  return resolvedChild.startsWith(prefix)
}

/**
 * The paths an `/attach` argument names. Quoted paths may hold spaces;
 * without quotes, the whole argument is one path when it exists (a path with
 * spaces typed bare), else each word is one.
 */
export function parseAttachArgs(args: string, exists: (candidate: string) => boolean): string[] {
  const trimmed = args.trim()
  if (!trimmed) return []
  if (!/["']/.test(trimmed)) {
    if (exists(trimmed)) return [trimmed]
    return trimmed.split(/\s+/)
  }
  const paths: string[] = []
  const pattern = /"([^"]*)"|'([^']*)'|(\S+)/g
  for (const match of trimmed.matchAll(pattern)) {
    const value = match[1] ?? match[2] ?? match[3]
    if (value) paths.push(value)
  }
  return paths
}
