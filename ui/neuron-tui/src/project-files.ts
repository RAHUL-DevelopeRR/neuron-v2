// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { createHash } from 'crypto'
import { mkdirSync, readdirSync, readFileSync, statSync } from 'fs'
import path from 'path'

import { CHAT_MESSAGES_FILENAME } from './utils/chat-meta'
import { getConfigDir } from './utils/config-dir'

let projectRoot: string | undefined
let currentChatId: string | undefined

function ensureChatDirectory(dir: string) {
  mkdirSync(dir, { recursive: true })
}

export function setProjectRoot(dir: string) {
  projectRoot = dir
  return projectRoot
}

export function getProjectRoot() {
  if (!projectRoot) {
    throw new Error('Project root not set')
  }
  return projectRoot
}

export function tryGetProjectRoot() {
  return projectRoot
}

export function getCurrentChatId() {
  if (!currentChatId) {
    currentChatId = new Date().toISOString().replace(/:/g, '-')
  }
  return currentChatId
}

export function setCurrentChatId(chatId: string) {
  currentChatId = chatId
  return currentChatId
}

export function startNewChat() {
  currentChatId = new Date().toISOString().replace(/:/g, '-')
  return currentChatId
}

/**
 * The name of a project's data directory: its folder name, so it stays
 * readable, and a short hash of its full resolved path, so two repositories
 * that share a folder name (`~/work/app`, `~/oss/app`) keep their chats and
 * `--continue` apart. Windows paths are case-insensitive, and a shell may
 * hand over either case, so they are lowercased first.
 */
export function projectDataDirName(root: string): string {
  const resolved = path.resolve(root)
  const key = process.platform === 'win32' ? resolved.toLowerCase() : resolved
  const hash = createHash('sha256').update(key).digest('hex').slice(0, 8)
  return `${path.basename(key) || 'root'}-${hash}`
}

// Get the project-specific data directory
export function getProjectDataDir(): string {
  const root = getProjectRoot()
  if (!root) {
    throw new Error('Project root not set')
  }

  return path.join(getConfigDir(), 'projects', projectDataDirName(root))
}

/**
 * Where this project's chats were kept before they were keyed by full path:
 * `projects/<folder name>`, shared by every folder of that name. Read only:
 * new chats always go to {@link getProjectDataDir}.
 */
export function getLegacyProjectDataDir(): string | null {
  const baseName = path.basename(getProjectRoot())
  return baseName ? path.join(getConfigDir(), 'projects', baseName) : null
}

/**
 * Every data directory that can hold this project's chats, newest scheme
 * first: its own directory, then the legacy one, so chats saved before the
 * change are still listed, resumed and continued. (A chat continued from the
 * legacy directory is saved to the new one from then on.)
 */
export function getProjectDataDirs(): string[] {
  const dirs = [getProjectDataDir()]
  const legacy = getLegacyProjectDataDir()
  if (legacy && !dirs.includes(legacy)) dirs.push(legacy)
  return dirs
}

/** The chat directory `chatId` names in the first data directory that has it. */
export function findChatDirIn(
  dataDirs: string[],
  chatId: string,
): string | null {
  const id = chatId.trim()
  if (!id || id === '.' || id === '..' || path.basename(id) !== id) return null
  for (const dataDir of dataDirs) {
    const candidate = path.join(dataDir, 'chats', id)
    try {
      if (statSync(candidate, { throwIfNoEntry: false })?.isDirectory()) {
        return candidate
      }
    } catch {
      // Unreadable: look in the next one.
    }
  }
  return null
}

/**
 * Whether a saved chat has a prompt in it. Slash commands (`/status`) are saved
 * as user messages too, but they leave the engine's session empty.
 */
function hasPrompt(messagesFile: string): boolean {
  try {
    const messages: unknown = JSON.parse(readFileSync(messagesFile, 'utf8'))
    return (
      Array.isArray(messages) &&
      messages.some(
        (message) =>
          message?.variant === 'user' &&
          typeof message.content === 'string' &&
          !message.content.trim().startsWith('/'),
      )
    )
  } catch {
    return false
  }
}

/**
 * The most recently modified chat directory across `dataDirs` that has a
 * prompt in it, or `null`. A run that ended before anything was asked leaves a
 * folder with only its log, or only slash commands; `--continue` skips those.
 */
export function mostRecentChatDirIn(dataDirs: string[]): string | null {
  const chats: { fullPath: string; mtime: number }[] = []
  for (const dataDir of dataDirs) {
    const chatsDir = path.join(dataDir, 'chats')
    let names: string[]
    try {
      if (!statSync(chatsDir, { throwIfNoEntry: false })) continue
      names = readdirSync(chatsDir)
    } catch {
      continue
    }
    for (const name of names) {
      const fullPath = path.join(chatsDir, name)
      try {
        const stat = statSync(fullPath)
        if (!stat.isDirectory()) continue
        chats.push({ fullPath, mtime: stat.mtime.getTime() })
      } catch {
        // Skip entries we can't stat
      }
    }
  }
  // Newest first; only as many message files are read as it takes.
  chats.sort((a, b) => b.mtime - a.mtime)
  const found = chats.find((chat) =>
    hasPrompt(path.join(chat.fullPath, CHAT_MESSAGES_FILENAME)),
  )
  return found?.fullPath ?? null
}

/**
 * Find the most recent chat directory based on modification time
 * Returns null if no chat directories exist
 */
export function getMostRecentChatDir(): string | null {
  try {
    return mostRecentChatDirIn(getProjectDataDirs())
  } catch {
    return null
  }
}

export function getCurrentChatDir(): string {
  const chatId = getCurrentChatId()
  const dir = path.join(getProjectDataDir(), 'chats', chatId)
  ensureChatDirectory(dir)
  return dir
}
