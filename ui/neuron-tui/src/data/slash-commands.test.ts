import { describe, expect, test } from 'bun:test'

import { helpSections } from '../components/help-banner'
import { getSlashCommandsWithSkills, SLASH_COMMANDS } from './slash-commands'

describe('extension commands', () => {
  test('/skills and /plugins are in the slash menu, run by the engine', () => {
    for (const id of ['skills', 'plugins', 'colony']) {
      expect(SLASH_COMMANDS.find((command) => command.id === id)).toMatchObject({
        engineCommand: true,
      })
    }
  })

  test('/help lists them', () => {
    const keys = helpSections('linux').flatMap((section) =>
      section.rows.map(([key]) => key),
    )
    expect(keys).toContain('/skills')
    expect(keys).toContain('/plugins')
    expect(keys).toContain('/colony')
  })
})

describe('getSlashCommandsWithSkills', () => {
  test('keeps user-only skills in the composer for additional context', () => {
    const commands = getSlashCommandsWithSkills({
      interview: {
        name: 'interview',
        description: 'Ask questions before implementing',
        content: 'Interview instructions',
        disableModelInvocation: true,
        filePath: '/skills/interview/SKILL.md',
      },
    })

    expect(
      commands.find((command) => command.id === 'skill:interview'),
    ).toMatchObject({
      label: 'skill:interview',
      insertText: '/skill:interview ',
    })
  })
})
