import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'

import { flushExchanges, installFakeGitEngine } from '../../__tests__/helpers/git-views-engine'
import { getSlashCommandsWithSkills } from '../../data/slash-commands'
import { setNeuronEngineClientForTesting } from '../../engine/neuron-engine-client'
import { useChatStore } from '../../state/chat-store'
import { useGitViewsStore } from '../../state/git-views-store'
import { findCommand, type RouterParams } from '../command-registry'
import { routeUserPrompt } from '../router'

const GIT = ['diff', 'commit', 'changes', 'rewind']

function params(inputValue: string) {
  const lines: string[] = []
  const setMessages = mock((update: any) => {
    for (const message of update([])) lines.push(`${message.variant}: ${message.content}`)
  })
  const history: string[] = []
  return {
    lines,
    history,
    params: {
      agentMode: 'DEFAULT',
      inputRef: { current: null },
      inputValue,
      isChainInProgressRef: { current: false },
      isStreaming: false,
      streamMessageIdRef: { current: null },
      saveToHistory: (text: string) => history.push(text),
      setInputValue: mock(() => {}),
      setInputFocused: mock(() => {}),
      setMessages,
      sendMessage: mock(() => {}),
      addToQueue: mock(() => {}),
      scrollToLatest: () => {},
    } as unknown as RouterParams,
  }
}

const withFeatures = (features: string[]) => useGitViewsStore.getState().setFeatures(features)

beforeEach(() => {
  useGitViewsStore.getState().reset()
  useChatStore.getState().reset()
})
afterEach(() => setNeuronEngineClientForTesting(null))

describe('gating by the engine features', () => {
  test('an older engine has none of them: the router forwards them as before', () => {
    for (const name of GIT) expect(findCommand(name)).toBeUndefined()
    const ids = getSlashCommandsWithSkills({}).map((command) => command.id)
    for (const name of GIT) expect(ids).not.toContain(name)
  })

  test('git_views brings /diff, /commit and /changes; checkpoints brings /rewind', () => {
    withFeatures(['git_views'])
    expect(['diff', 'commit', 'changes'].map((name) => findCommand(name)?.name)).toEqual([
      'diff',
      'commit',
      'changes',
    ])
    expect(findCommand('rewind')).toBeUndefined()
    withFeatures(['checkpoints'])
    expect(findCommand('rewind')?.name).toBe('rewind')
    expect(findCommand('diff')).toBeUndefined()
  })

  test('the slash menu lists them after /model, only with their feature', () => {
    const ids = getSlashCommandsWithSkills({}, ['git_views', 'checkpoints']).map((c) => c.id)
    const model = ids.indexOf('model')
    expect(ids.slice(model + 1, model + 5)).toEqual(['changes', 'diff', 'commit', 'rewind'])
    const gitOnly = getSlashCommandsWithSkills({}, ['git_views']).map((c) => c.id)
    expect(gitOnly).toContain('diff')
    expect(gitOnly).not.toContain('rewind')
  })

  test('without the feature, /diff still goes to the engine as a slash command', async () => {
    const { sent } = installFakeGitEngine()
    const { params: p, lines } = params('/diff src/a.ts')
    await routeUserPrompt(p)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'diff', args: 'src/a.ts' }])
    expect(lines).toEqual(['user: /diff src/a.ts'])
  })
})

describe('the commands', () => {
  test('/diff <path> asks the engine for that file', async () => {
    withFeatures(['git_views'])
    const { sent } = installFakeGitEngine()
    const { params: p, lines, history } = params('/diff src/a.ts')
    await routeUserPrompt(p)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'diff', args: 'src/a.ts' }])
    expect(lines).toEqual(['user: /diff src/a.ts'])
    expect(history).toEqual(['/diff src/a.ts'])
  })

  test('/commit alone only asks for a proposal', async () => {
    withFeatures(['git_views'])
    const { sent } = installFakeGitEngine()
    await routeUserPrompt(params('/commit').params)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'commit', args: '' }])
  })

  test('/commit <message> keeps the message as typed, body lines too', async () => {
    withFeatures(['git_views'])
    const { sent } = installFakeGitEngine()
    const message = 'fix: parser\n\nHandles  empty input.'
    await routeUserPrompt(params(`/commit ${message}`).params)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'commit', args: message }])
  })

  test('/changes opens the panel and sends nothing', async () => {
    withFeatures(['git_views'])
    const { sent } = installFakeGitEngine()
    const { params: p, lines } = params('/changes')
    await routeUserPrompt(p)
    await flushExchanges()
    expect(sent).toEqual([])
    expect(lines).toEqual([])
    expect(useGitViewsStore.getState().changesPanelOpen).toBe(true)
  })

  test('/rewind asks for the checkpoints', async () => {
    withFeatures(['checkpoints'])
    const { sent } = installFakeGitEngine()
    await routeUserPrompt(params('/rewind').params)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'rewind', args: '' }])
  })

  test('/commit and /rewind wait for no turn: refused while one runs or prompts are queued', async () => {
    withFeatures(['git_views', 'checkpoints'])
    const { sent } = installFakeGitEngine()
    for (const typed of ['/commit', '/commit fix: it', '/rewind']) {
      const streaming = params(typed)
      streaming.params.isStreaming = true
      await routeUserPrompt(streaming.params)
      expect(streaming.lines.at(-1)).toMatch(/^ai: Wait for the running turn to end \(or stop it\), then run \/(commit|rewind) again\.$/)
      const chained = params(typed)
      chained.params.isChainInProgressRef.current = true
      await routeUserPrompt(chained.params)
      const queued = params(typed)
      queued.params.hasQueuedMessages = () => true
      await routeUserPrompt(queued.params)
    }
    await flushExchanges()
    expect(sent).toEqual([])
    // /diff only reads: it still goes.
    const diff = params('/diff')
    diff.params.isStreaming = true
    await routeUserPrompt(diff.params)
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'diff', args: '' }])
  })

  test('Enter on an empty input brings up a card that waits behind it', async () => {
    withFeatures(['git_views'])
    installFakeGitEngine()
    useGitViewsStore.getState().setProposal(
      { message: 'fix: it', branch: 'main', files: [{ path: 'a', status: 'modified', added: 1, removed: 0 }], stage_all: true },
      { deferred: true },
    )
    const empty = params('   ')
    await routeUserPrompt(empty.params)
    expect(useGitViewsStore.getState().deferred).toBe(false)
    expect(empty.params.sendMessage).not.toHaveBeenCalled()
  })

  test('without a running engine they say so', async () => {
    withFeatures(['git_views'])
    const { sent } = installFakeGitEngine('exited')
    await routeUserPrompt(params('/diff').params)
    await flushExchanges()
    expect(sent).toEqual([])
    expect(useChatStore.getState().messages.map((m) => m.content)).toContain(
      'The Neuron engine is not running.',
    )
  })
})
