import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import * as fs from 'fs'
import * as os from 'os'
import * as path from 'path'

import {
  continueChatAtStart,
  continuedChatDirIn,
  findChatDirIn,
  getCurrentChatDir,
  getCurrentChatId,
  getProjectRoot,
  mostRecentChatDirIn,
  projectDataDirName,
  setProjectRoot,
  startNewChat,
  tryGetProjectRoot,
} from '../project-files'
import {
  loadMostRecentChatState,
  setChatDirOverrideForTesting,
} from '../utils/run-state-storage'

import type { ChatMessage } from '../types/chat'

describe('projectDataDirName', () => {
  test('two folders with the same name get different directories', () => {
    const work = projectDataDirName(path.join(os.tmpdir(), 'work', 'app'))
    const oss = projectDataDirName(path.join(os.tmpdir(), 'oss', 'app'))
    expect(work).not.toBe(oss)
    // The folder name stays readable.
    expect(work).toMatch(/^app-[0-9a-f]{8}$/)
    expect(oss).toMatch(/^app-[0-9a-f]{8}$/)
  })

  test('the same folder, however it is written, gets one directory', () => {
    const root = path.join(os.tmpdir(), 'work', 'app')
    expect(projectDataDirName(root + path.sep)).toBe(projectDataDirName(root))
    expect(
      projectDataDirName(path.join(os.tmpdir(), 'work', 'x', '..', 'app')),
    ).toBe(projectDataDirName(root))
    if (process.platform === 'win32') {
      expect(projectDataDirName(root.toUpperCase())).toBe(projectDataDirName(root))
      expect(projectDataDirName(root.replace(/\\/g, '/'))).toBe(
        projectDataDirName(root),
      )
    }
  })
})

describe('chat lookup across data directories', () => {
  let configDir = ''
  let current = ''
  let legacy = ''

  const writeChat = (dataDir: string, chatId: string, prompt: string) => {
    const chatDir = path.join(dataDir, 'chats', chatId)
    fs.mkdirSync(chatDir, { recursive: true })
    const messages: ChatMessage[] = [
      { id: `${chatId}-1`, variant: 'user', content: prompt, timestamp: 'now' },
    ]
    fs.writeFileSync(path.join(chatDir, 'chat-messages.json'), JSON.stringify(messages))
    fs.writeFileSync(
      path.join(chatDir, 'run-state.json'),
      JSON.stringify({ sessionId: `session-${chatId}`, output: { type: 'lastMessage' } }),
    )
    return chatDir
  }

  beforeEach(() => {
    configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-data-dirs-'))
    current = path.join(configDir, 'projects', 'app-0123abcd')
    legacy = path.join(configDir, 'projects', 'app')
  })

  afterEach(() => {
    fs.rmSync(configDir, { recursive: true, force: true })
  })

  test('a chat id resolves in the project directory first, then the legacy one', () => {
    const oldChat = writeChat(legacy, 'old', 'before')
    expect(findChatDirIn([current, legacy], 'old')).toBe(oldChat)

    const newCopy = writeChat(current, 'old', 'continued since')
    expect(findChatDirIn([current, legacy], 'old')).toBe(newCopy)

    expect(findChatDirIn([current, legacy], 'missing')).toBeNull()
    expect(findChatDirIn([current, legacy], '..')).toBeNull()
    expect(findChatDirIn([current, legacy], '../app/chats/old')).toBeNull()
  })

  test('the most recent chat is found in either directory', () => {
    const older = writeChat(current, 'a', 'one')
    const newer = writeChat(legacy, 'b', 'two')
    fs.utimesSync(older, new Date(1_000_000), new Date(1_000_000))
    fs.utimesSync(newer, new Date(2_000_000), new Date(2_000_000))
    expect(mostRecentChatDirIn([current, legacy])).toBe(newer)
    expect(mostRecentChatDirIn([path.join(configDir, 'none')])).toBeNull()

    // A newer run that sent nothing leaves only its log: not a chat to continue.
    const empty = path.join(current, 'chats', 'c')
    fs.mkdirSync(empty, { recursive: true })
    fs.writeFileSync(path.join(empty, 'log.jsonl'), '')
    fs.utimesSync(empty, new Date(3_000_000), new Date(3_000_000))
    expect(mostRecentChatDirIn([current, legacy])).toBe(newer)

    // Nor is one where only slash commands were typed: the engine's session is empty.
    const slashOnly = writeChat(current, 'd', '/status')
    fs.utimesSync(slashOnly, new Date(4_000_000), new Date(4_000_000))
    expect(mostRecentChatDirIn([current, legacy])).toBe(newer)
  })

  test('--continue picks the named chat, else the most recent one with a prompt', () => {
    const older = writeChat(current, 'a', 'one')
    const newer = writeChat(current, 'b', 'two')
    fs.utimesSync(older, new Date(1_000_000), new Date(1_000_000))
    fs.utimesSync(newer, new Date(2_000_000), new Date(2_000_000))
    expect(continuedChatDirIn([current, legacy], 'a')).toBe(older)
    expect(continuedChatDirIn([current, legacy], null)).toBe(newer)
    // An id that is not there falls back, as loadMostRecentChatState does.
    expect(continuedChatDirIn([current, legacy], 'missing')).toBe(newer)
    expect(continuedChatDirIn([path.join(configDir, 'none')], null)).toBeNull()
  })

  test('--continue makes the continued chat current, so no new chat directory is made', () => {
    const savedEnv = process.env.NEURON_TUI_CONFIG_DIR
    const savedRoot = tryGetProjectRoot()
    try {
      process.env.NEURON_TUI_CONFIG_DIR = configDir
      setProjectRoot(path.join(configDir, 'repos', 'app'))
      const dataDir = path.join(configDir, 'projects', projectDataDirName(getProjectRoot()))
      const chat = writeChat(dataDir, '2026-01-02T03-04-05.678Z', 'old prompt')

      expect(continueChatAtStart(null)).toBe('2026-01-02T03-04-05.678Z')
      expect(getCurrentChatId()).toBe('2026-01-02T03-04-05.678Z')
      expect(getCurrentChatDir()).toBe(chat)
      expect(fs.readdirSync(path.join(dataDir, 'chats'))).toEqual(['2026-01-02T03-04-05.678Z'])
    } finally {
      if (savedEnv === undefined) delete process.env.NEURON_TUI_CONFIG_DIR
      else process.env.NEURON_TUI_CONFIG_DIR = savedEnv
      if (savedRoot !== undefined) setProjectRoot(savedRoot)
      startNewChat()
    }
  })

  test('--continue <chatId> still opens a chat saved under the folder name only', () => {
    const savedEnv = process.env.NEURON_TUI_CONFIG_DIR
    const savedRoot = tryGetProjectRoot()
    try {
      process.env.NEURON_TUI_CONFIG_DIR = configDir
      setProjectRoot(path.join(configDir, 'repos', 'app'))
      setChatDirOverrideForTesting(undefined)
      writeChat(path.join(configDir, 'projects', 'app'), '2026-01-02T03-04-05.678Z', 'old prompt')

      const loaded = loadMostRecentChatState('2026-01-02T03-04-05.678Z')
      expect(loaded?.chatId).toBe('2026-01-02T03-04-05.678Z')
      expect(loaded?.messages[0]?.content).toBe('old prompt')
      expect(loaded?.runState.sessionId).toBe('session-2026-01-02T03-04-05.678Z')
    } finally {
      if (savedEnv === undefined) delete process.env.NEURON_TUI_CONFIG_DIR
      else process.env.NEURON_TUI_CONFIG_DIR = savedEnv
      if (savedRoot !== undefined) setProjectRoot(savedRoot)
    }
  })
})
