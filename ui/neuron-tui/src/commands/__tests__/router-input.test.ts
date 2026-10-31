// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: examples use
// commands that still exist (/init, /login, /usage, /feedback were removed),
// and the menu offers no /mode:* or /image. See ui/neuron-tui/PORTING.md.
import { describe, test, expect } from 'bun:test'

import { SLASH_COMMANDS } from '../../data/slash-commands'
import { findCommand, COMMAND_REGISTRY } from '../command-registry'
import {
  parseCommand,
  isSlashCommand,
  parseCommandInput,
} from '../router-utils'

describe('router-utils', () => {
  describe('isSlashCommand', () => {
    test('returns true for input starting with /', () => {
      expect(isSlashCommand('/help')).toBe(true)
      expect(isSlashCommand('/logout')).toBe(true)
      expect(isSlashCommand('/ref-abc123')).toBe(true)
      expect(isSlashCommand('/')).toBe(true)
    })

    test('returns false for input not starting with /', () => {
      expect(isSlashCommand('help')).toBe(false)
      expect(isSlashCommand('logout')).toBe(false)
      expect(isSlashCommand('ref-abc123')).toBe(false)
      expect(isSlashCommand('')).toBe(false)
    })

    test('handles whitespace correctly', () => {
      expect(isSlashCommand('  /help')).toBe(true)
      expect(isSlashCommand('  help')).toBe(false)
    })
  })

  describe('parseCommand', () => {
    test('extracts command from slashed input', () => {
      expect(parseCommand('/help')).toBe('help')
      expect(parseCommand('/logout')).toBe('logout')
      expect(parseCommand('/usage')).toBe('usage')
    })

    test('returns empty string for unslashed input (not a slash command)', () => {
      expect(parseCommand('help')).toBe('')
      expect(parseCommand('logout')).toBe('')
      expect(parseCommand('usage')).toBe('')
      expect(parseCommand('login to my database')).toBe('')
    })

    test('extracts first word as command when there are arguments', () => {
      expect(parseCommand('/help me')).toBe('help')
      expect(parseCommand('/usage stats')).toBe('usage')
    })

    test('converts command to lowercase', () => {
      expect(parseCommand('/HELP')).toBe('help')
      expect(parseCommand('/LOGOUT')).toBe('logout')
      expect(parseCommand('/UsAgE')).toBe('usage')
    })

    test('handles empty string', () => {
      expect(parseCommand('')).toBe('')
    })

    test('handles whitespace-only input', () => {
      expect(parseCommand('   ')).toBe('')
    })

    test('handles only slash', () => {
      expect(parseCommand('/')).toBe('')
    })

    test('handles multiple spaces between words', () => {
      expect(parseCommand('/help   me')).toBe('help')
    })
  })

  describe('parseCommandInput', () => {
    test('returns command info for exact slashless matches', () => {
      expect(parseCommandInput('help')).toEqual({
        command: 'help',
        args: '',
        implicitCommand: true,
      })
      expect(parseCommandInput('new')).toEqual({
        command: 'new',
        args: '',
        implicitCommand: true,
      })
    })

    test('is case-insensitive and trims whitespace for slashless matches', () => {
      expect(parseCommandInput('HELP')).toEqual({
        command: 'help',
        args: '',
        implicitCommand: true,
      })
      expect(parseCommandInput('  new  ')).toEqual({
        command: 'new',
        args: '',
        implicitCommand: true,
      })
    })

    test('returns null for slashless commands with arguments', () => {
      expect(parseCommandInput('help something')).toBe(null)
      expect(parseCommandInput('new my message')).toBe(null)
    })

    test('returns null for commands not configured for slashless invocation', () => {
      expect(parseCommandInput('history')).toBe(null)
      expect(parseCommandInput('bash')).toBe(null)
      expect(parseCommandInput('review')).toBe(null)
    })

    test('distinguishes slashed and slashless invocation', () => {
      expect(parseCommandInput('/help')).toEqual({
        command: 'help',
        args: '',
        implicitCommand: false,
      })
    })

    test('does not match aliases for slashless commands', () => {
      const newCmd = SLASH_COMMANDS.find((cmd) => cmd.id === 'new')
      for (const alias of newCmd?.aliases ?? []) {
        expect(parseCommandInput(alias)).toBe(null)
      }
    })

    test('returns null for empty input', () => {
      expect(parseCommandInput('')).toBe(null)
      expect(parseCommandInput('   ')).toBe(null)
    })

    test('commands with implicitCommand are configured correctly', () => {
      const helpCmd = SLASH_COMMANDS.find((cmd) => cmd.id === 'help')
      const newCmd = SLASH_COMMANDS.find((cmd) => cmd.id === 'new')

      expect(helpCmd?.implicitCommand).toBe(true)
      expect(newCmd?.implicitCommand).toBe(true)
    })

    test('parseCommandInput matches all implicitCommand commands', () => {
      const implicitCommands = SLASH_COMMANDS.filter((cmd) => cmd.implicitCommand)
      for (const cmd of implicitCommands) {
        expect(parseCommandInput(cmd.id)).toEqual({
          command: cmd.id.toLowerCase(),
          args: '',
          implicitCommand: true,
        })
      }
    })
  })

  describe('slash commands only work with / prefix', () => {
    const slashCommands = [
      'help',
      'history',
      'exit',
      'clear',
      'new',
      'bash',
      'review',
      'queue',
    ]

    for (const cmd of slashCommands) {
      test(`"/${cmd}" is recognized as slash command`, () => {
        expect(parseCommand(`/${cmd}`)).toBe(cmd)
      })

      test(`"${cmd}" without slash is NOT a slash command (sent to agent)`, () => {
        expect(parseCommand(cmd)).toBe('')
      })
    }
  })

  describe('words that look like commands but are not', () => {
    const nonCommands = [
      'login to my account',
      'I need help with logout functionality',
      'please help me',
      'usage of this function',
      'clear the database',
    ]

    for (const input of nonCommands) {
      test(`"${input}" is NOT a slash command`, () => {
        expect(parseCommand(input)).toBe('')
      })
    }
  })

})

describe('command-registry', () => {
  describe('findCommand', () => {
    test('finds command by name', () => {
      const help = findCommand('help')
      expect(help).toBeDefined()
      expect(help?.name).toBe('help')

      const history = findCommand('history')
      expect(history).toBeDefined()
      expect(history?.name).toBe('history')
    })

    test('finds command by alias', () => {
      const chats = findCommand('chats')
      expect(chats).toBeDefined()
      expect(chats?.name).toBe('history')

      const quit = findCommand('quit')
      expect(quit).toBeDefined()
      expect(quit?.name).toBe('exit')

      const questionMark = findCommand('?')
      expect(questionMark).toBeDefined()
      expect(questionMark?.name).toBe('help')
    })

    test('returns undefined for unknown command', () => {
      expect(findCommand('unknown')).toBeUndefined()
      expect(findCommand('notacommand')).toBeUndefined()
    })

    test('is case insensitive', () => {
      expect(findCommand('HELP')?.name).toBe('help')
      expect(findCommand('HiStOrY')?.name).toBe('history')
      expect(findCommand('CHATS')?.name).toBe('history')
    })
  })

  describe('COMMAND_REGISTRY', () => {
    test('all commands have unique names', () => {
      const names = COMMAND_REGISTRY.map((c) => c.name)
      const uniqueNames = new Set(names)
      expect(names.length).toBe(uniqueNames.size)
    })

    test('all aliases are unique across all commands', () => {
      const allAliases = COMMAND_REGISTRY.flatMap((c) => c.aliases)
      const uniqueAliases = new Set(allAliases)
      expect(allAliases.length).toBe(uniqueAliases.size)
    })

    test('no alias conflicts with command names', () => {
      const names = new Set(COMMAND_REGISTRY.map((c) => c.name))
      const allAliases = COMMAND_REGISTRY.flatMap((c) => c.aliases)
      for (const alias of allAliases) {
        expect(names.has(alias)).toBe(false)
      }
    })

    test('slash command metadata maps to registered commands', () => {
      const registered = new Set([
        ...COMMAND_REGISTRY.map((c) => c.name),
        ...COMMAND_REGISTRY.flatMap((c) => c.aliases),
      ])

      // Commands with insertText are UI-only shortcuts that insert text into
      // the input field instead of executing a command. The engine's commands
      // have no UI handler: the router forwards them.
      const executableCommands = SLASH_COMMANDS.filter(
        (cmd) => !cmd.insertText && !cmd.engineCommand,
      )

      for (const slashCommand of executableCommands) {
        expect(registered.has(slashCommand.id)).toBe(true)
        for (const alias of slashCommand.aliases ?? []) {
          expect(registered.has(alias)).toBe(true)
        }
      }
    })

    test("the engine's commands have no UI handler, so they reach the engine", () => {
      const engineCommands = SLASH_COMMANDS.filter((cmd) => cmd.engineCommand)
      expect(engineCommands.map((cmd) => cmd.id)).toContain('login')
      for (const command of engineCommands) {
        expect(findCommand(command.id)).toBeUndefined()
      }
    })

    test('the menu offers no agent modes and no /image, which do nothing yet', () => {
      // The engine ignores the agent modes, and only lists an image's path.
      const ids = SLASH_COMMANDS.map((cmd) => cmd.id)
      expect(ids.filter((id) => id.startsWith('mode:'))).toEqual([])
      expect(ids).not.toContain('image')
      expect(findCommand('model:default')).toBeUndefined()
    })

    test('the ChatGPT connect command is gone from both products', () => {
      // It was freebuff-only while it existed; the integration has since been
      // removed, so neither the slash menu nor the router knows the name.
      expect(SLASH_COMMANDS.some((cmd) => cmd.id === 'connect')).toBe(false)
      expect(findCommand('connect')).toBeUndefined()
      expect(findCommand('connect:chatgpt')).toBeUndefined()
      expect(findCommand('chatgpt')).toBeUndefined()
    })
  })
})
