import { describe, expect, test } from 'bun:test'

import { quietReadKind, quietReadSummary, toolView, withResult } from '../src/tools'

const bash = (output: string, isError = false, name = 'bash') => withResult(toolView(name, '{"command":"ls"}'), name, output, isError)

describe('exit codes on shell results', () => {
  test('reads exit_code:<n> from returnCodeInterpretation', () => {
    const view = bash('{"stdout":"","stderr":"boom","returnCodeInterpretation":"exit_code:3"}')
    expect(view.exitCode).toBe(3)
    expect(view.output).toBe('boom')
  })
  test('reads the phrase "exit code 2" too', () => {
    expect(bash('{"stdout":"","stderr":"x","returnCodeInterpretation":"exit code 2"}', false, 'PowerShell').exitCode).toBe(2)
  })
  test('reads a numeric exitCode field', () => {
    expect(bash('{"stdout":"ok","stderr":"","exitCode":0}').exitCode).toBe(0)
  })
  test('is absent when the output carries none', () => {
    expect(bash('{"stdout":"ok","stderr":""}').exitCode).toBeUndefined()
    expect(bash('plain text').exitCode).toBeUndefined()
  })
  test('is absent for timeouts and signals', () => {
    expect(bash('{"stdout":"","stderr":"","returnCodeInterpretation":"timeout"}').exitCode).toBeUndefined()
  })
  test('is read from an error result that is still JSON', () => {
    const view = bash('{"stdout":"","stderr":"bad","returnCodeInterpretation":"exit_code:1"}', true)
    expect(view.exitCode).toBe(1)
  })
  test('other tools have none', () => {
    const view = withResult(toolView('read_file', '{"path":"a"}'), 'read_file', '{"file":{"content":"x"},"exitCode":4}', false)
    expect(view.exitCode).toBeUndefined()
  })
})

describe('quiet reads', () => {
  test('classifies read-only calls by tool name', () => {
    expect(quietReadKind('read_file')).toBe('read')
    expect(quietReadKind('list_directory')).toBe('list')
    expect(quietReadKind('glob_search')).toBe('search')
    expect(quietReadKind('grep_search')).toBe('search')
    expect(quietReadKind('bash')).toBeNull()
    expect(quietReadKind('write_file')).toBeNull()
    expect(quietReadKind('WebFetch')).toBeNull()
  })
  test('summarizes in plain words', () => {
    expect(quietReadSummary(['read_file', 'read_file', 'read_file'])).toBe('Read 3 files')
    expect(quietReadSummary(['read_file'])).toBe('Read 1 file')
    expect(quietReadSummary(['grep_search', 'glob_search', 'grep_search'])).toBe('Searched 3 times')
    expect(quietReadSummary(['list_directory'])).toBe('Listed 1 folder')
    expect(quietReadSummary(['read_file', 'read_file', 'grep_search', 'list_directory'])).toBe('Read 2 files, searched once, listed 1 folder')
  })
})
