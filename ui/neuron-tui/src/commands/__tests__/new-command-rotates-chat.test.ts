// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the child
// no longer needs Codebuff's web env. See ui/neuron-tui/PORTING.md.
import { describe, expect, test } from 'bun:test'

describe('/new command', () => {
  test('rotates the chat id so the previous chat is not overwritten', () => {
    const commandRegistryUrl = new URL(
      '../command-registry.ts',
      import.meta.url,
    ).href
    const projectFilesUrl = new URL('../../project-files.ts', import.meta.url)
      .href
    const activeRunUrl = new URL('../../utils/active-run.ts', import.meta.url)
      .href

    const result = Bun.spawnSync({
      cmd: [
        'bun',
        '--eval',
        `
          import { findCommand } from ${JSON.stringify(commandRegistryUrl)}
          import { getCurrentChatId, setCurrentChatId } from ${JSON.stringify(projectFilesUrl)}
          import { registerActiveRun } from ${JSON.stringify(activeRunUrl)}

          setCurrentChatId('previous-chat-id')

          // Simulate an in-flight run: record which chat was current when the
          // abort fired.
          let abortedAtChatId = null
          let stopReason = null
          registerActiveRun('run-1', (reason) => {
            abortedAtChatId = getCurrentChatId()
            stopReason = reason
          })

          const newCommand = findCommand('new')
          if (!newCommand) throw new Error('new command missing')

          const noop = () => {}
          newCommand.handler(
            {
              setMessages: noop,
              clearMessages: noop,
              saveToHistory: noop,
              inputValue: '/new',
              setInputValue: noop,
              setCanProcessQueue: noop,
            },
            '',
          )

          if (getCurrentChatId() === 'previous-chat-id') {
            throw new Error(
              '/new did not rotate the chat id — the next save would overwrite the previous chat',
            )
          }

          if (abortedAtChatId === null) {
            throw new Error(
              '/new did not abort the in-flight run — an orphaned run would keep checkpointing across the chat switch',
            )
          }
          if (abortedAtChatId !== 'previous-chat-id') {
            throw new Error(
              '/new aborted the run only after rotating the chat id — late writes could land in the new chat',
            )
          }
          if (stopReason !== 'new-chat') {
            throw new Error('/new did not use the new-chat cancellation policy')
          }
        `,
      ],
      cwd: process.cwd(),
      env: {
        ...process.env,
        NODE_ENV: 'test',
      },
      stderr: 'pipe',
      stdout: 'pipe',
    })

    const stderr = new TextDecoder().decode(result.stderr)
    expect(result.exitCode, stderr).toBe(0)
  })
})
