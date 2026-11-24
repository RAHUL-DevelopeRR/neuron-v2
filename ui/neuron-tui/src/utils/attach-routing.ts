/**
 * Where a file picked with the Attach button (or /attach, or dropped on the
 * terminal) goes. Pure: the caller reads the file's facts (attach-files.ts)
 * and acts on the route.
 *
 * - Images go through the image path, while images reach the model
 *   (IMAGES_REACH_THE_MODEL); until then they are refused with a message.
 * - Small text and code files are read and inlined into the prompt whole.
 * - Office and PDF files, and text too long to inline, go by reference: the
 *   prompt names the path and the model reads it with `read_document` or
 *   `read_file`. The engine reads only inside its workspace (the tools check
 *   the boundary), so such a file picked outside it is first copied into
 *   the workspace's `.neuron/attachments` folder and the copy is referenced.
 * - A file the engine cannot read is refused with a message, never attached
 *   with an instruction that cannot work: a binary file that is not a
 *   document (`read_file` rejects any NUL byte in the first 8 KB, as
 *   `looksBinary` does), and a file over the engine's size limits.
 */

import path from 'path'

import { isImageFile } from './image-handler'

/**
 * The Office and PDF files the engine's `read_document` tool converts to
 * Markdown: the engine's `DOCUMENT_KINDS` (rust/crates/runtime/src/file_ops.rs),
 * which a test keeps this in step with.
 */
export const DOCUMENT_EXTENSIONS: ReadonlySet<string> = new Set([
  '.docx',
  '.pptx',
  '.xlsx',
  '.xlsm',
  '.xlsb',
  '.xls',
  '.ods',
  '.pdf',
])

const MB = 1024 * 1024

/**
 * A text file up to this size is inlined into the prompt whole. Larger text
 * would be cut at pending-attachments.ts's 100 KB, so it goes by reference
 * for the model to read all of it.
 */
export const MAX_INLINE_FILE_BYTES = 100 * 1024

/** The most the engine's `read_file` reads (`MAX_READ_SIZE` in file_ops.rs). */
export const ENGINE_MAX_READ_BYTES = 10 * MB

/**
 * The most the engine's `read_document` reads (`document_size_limit` in
 * file_ops.rs): Word and PowerPoint are streamed, the rest read whole.
 */
export function documentSizeLimit(filePath: string): number {
  const ext = path.extname(filePath).toLowerCase()
  return ext === '.docx' || ext === '.pptx' ? 200 * MB : 100 * MB
}

export type ReferenceTool = 'read_document' | 'read_file'

export type AttachRoute =
  | { kind: 'image' }
  | { kind: 'text' }
  | { kind: 'folder' }
  /** Inside the workspace: the model reads the file where it is. */
  | { kind: 'reference'; tool: ReferenceTool }
  /** Outside the workspace: copied into it first, and the copy is read. */
  | { kind: 'copy'; tool: ReferenceTool }
  | { kind: 'refuse'; reason: string }

export interface AttachFacts {
  /** The absolute path. */
  path: string
  isDirectory: boolean
  /** Bytes. */
  size: number
  /** A NUL byte in the first 8 KB. */
  isBinary: boolean
  /** Inside the engine's workspace, where its tools can read it. */
  insideWorkspace: boolean
  /** Whether a picked image can reach the model (IMAGES_REACH_THE_MODEL). */
  imagesReachModel: boolean
}

function formatLimit(bytes: number): string {
  return `${Math.round(bytes / MB)} MB`
}

/** Said when an image is picked while images cannot reach the model. */
export function imageRefusal(filename: string): string {
  return `${filename} was not attached: the model can't see images in Neuron yet. Describe it in words, or paste the text it shows.`
}

/** Said when a binary file that is not a document is picked. */
export function binaryRefusal(filename: string): string {
  return `${filename} was not attached: it is a binary file Neuron cannot read.`
}

/** Said when a file is over the size the engine reads. */
export function tooLargeRefusal(filename: string, limitBytes: number): string {
  return `${filename} was not attached: it is larger than ${formatLimit(limitBytes)}, the most Neuron reads.`
}

/**
 * What the prompt says about a file attached by reference. A copy names the
 * file it was copied from, so the model can talk about it by its real name.
 */
export function fileReferenceText(filePath: string, tool: ReferenceTool, sourcePath?: string): string {
  const what = tool === 'read_document' ? 'an attached document' : 'an attached file'
  const copied = sourcePath ? `, copied from ${sourcePath}` : ''
  return `[File: ${filePath}]\n(${what}${copied}: read it with the ${tool} tool.)`
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
    facts.insideWorkspace ? { kind: 'reference', tool } : { kind: 'copy', tool }
  if (isDocumentFile(facts.path)) {
    const limit = documentSizeLimit(facts.path)
    if (facts.size > limit) return { kind: 'refuse', reason: tooLargeRefusal(filename, limit) }
    return byReference('read_document')
  }
  if (facts.isBinary) return { kind: 'refuse', reason: binaryRefusal(filename) }
  if (facts.size <= MAX_INLINE_FILE_BYTES) return { kind: 'text' }
  if (facts.size > ENGINE_MAX_READ_BYTES) {
    return { kind: 'refuse', reason: tooLargeRefusal(filename, ENGINE_MAX_READ_BYTES) }
  }
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
 * The paths an `/attach` argument (or a paste) names. The whole argument is
 * one path when it exists, whatever it holds (spaces, an apostrophe as in
 * `Rahul's resume.pdf`). Otherwise quoted paths may hold spaces and each
 * unquoted word is one path.
 */
export function parseAttachArgs(args: string, exists: (candidate: string) => boolean): string[] {
  const trimmed = args.trim()
  if (!trimmed) return []
  if (exists(trimmed)) return [trimmed]
  if (!/["']/.test(trimmed)) return trimmed.split(/\s+/)
  const paths: string[] = []
  const pattern = /"([^"]*)"|'([^']*)'|(\S+)/g
  for (const match of trimmed.matchAll(pattern)) {
    const value = match[1] ?? match[2] ?? match[3]
    if (value) paths.push(value)
  }
  return paths
}
