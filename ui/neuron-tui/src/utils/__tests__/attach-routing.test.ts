import { describe, expect, test } from 'bun:test'

import {
  DOCUMENT_EXTENSIONS,
  MAX_INLINE_FILE_BYTES,
  fileReferenceText,
  isPathInside,
  parseAttachArgs,
  routeAttachment,
  type AttachFacts,
} from '../attach-routing'

const facts = (overrides: Partial<AttachFacts>): AttachFacts => ({
  path: '/ws/notes.txt',
  isDirectory: false,
  size: 120,
  isBinary: false,
  insideWorkspace: true,
  imagesReachModel: false,
  workspaceRoot: '/ws',
  ...overrides,
})

describe('routeAttachment', () => {
  test('text and code files are inlined', () => {
    expect(routeAttachment(facts({ path: '/ws/notes.txt' }))).toEqual({ kind: 'text' })
    expect(routeAttachment(facts({ path: '/ws/src/main.rs' }))).toEqual({ kind: 'text' })
    expect(routeAttachment(facts({ path: '/ws/Makefile' }))).toEqual({ kind: 'text' })
  })

  test('a text file outside the workspace is still inlined: the UI reads it', () => {
    expect(routeAttachment(facts({ path: '/home/me/todo.md', insideWorkspace: false }))).toEqual({ kind: 'text' })
  })

  test('every Office and PDF extension goes to read_document', () => {
    for (const ext of ['.docx', '.pptx', '.xlsx', '.xlsm', '.xls', '.ods', '.pdf', '.PDF']) {
      expect(routeAttachment(facts({ path: `/ws/report${ext}`, isBinary: true }))).toEqual({
        kind: 'reference',
        tool: 'read_document',
      })
    }
    expect(DOCUMENT_EXTENSIONS.size).toBe(7)
  })

  test('a document outside the workspace is refused, saying where it must be', () => {
    const route = routeAttachment(
      facts({ path: 'C:\\Users\\me\\Downloads\\deck.pptx', insideWorkspace: false, workspaceRoot: 'C:\\ws' }),
    )
    expect(route.kind).toBe('refuse')
    if (route.kind === 'refuse') {
      expect(route.reason).toContain('deck.pptx')
      expect(route.reason).toContain('outside the project folder')
      expect(route.reason).toContain('C:\\ws')
    }
  })

  test('binary and oversized files go by path to read_file inside the workspace', () => {
    expect(routeAttachment(facts({ path: '/ws/app.wasm', isBinary: true }))).toEqual({
      kind: 'reference',
      tool: 'read_file',
    })
    expect(routeAttachment(facts({ path: '/ws/huge.log', size: MAX_INLINE_FILE_BYTES + 1 }))).toEqual({
      kind: 'reference',
      tool: 'read_file',
    })
    expect(routeAttachment(facts({ path: '/ws/edge.log', size: MAX_INLINE_FILE_BYTES }))).toEqual({ kind: 'text' })
  })

  test('binary or oversized files outside the workspace are refused', () => {
    expect(routeAttachment(facts({ path: '/tmp/app.bin', isBinary: true, insideWorkspace: false })).kind).toBe(
      'refuse',
    )
    expect(
      routeAttachment(facts({ path: '/tmp/huge.log', size: MAX_INLINE_FILE_BYTES + 1, insideWorkspace: false })).kind,
    ).toBe('refuse')
  })

  test('images are refused while they cannot reach the model, and attached once they can', () => {
    const refused = routeAttachment(facts({ path: '/ws/shot.png', isBinary: true }))
    expect(refused.kind).toBe('refuse')
    if (refused.kind === 'refuse') expect(refused.reason).toContain("can't see images")
    expect(routeAttachment(facts({ path: '/ws/shot.png', isBinary: true, imagesReachModel: true }))).toEqual({
      kind: 'image',
    })
  })

  test('a folder is attached as a folder', () => {
    expect(routeAttachment(facts({ path: '/ws/src', isDirectory: true }))).toEqual({ kind: 'folder' })
  })
})

describe('isPathInside', () => {
  test('on a separator boundary only', () => {
    expect(isPathInside('C:\\ws\\a.pdf', 'C:\\ws', 'win32')).toBe(true)
    expect(isPathInside('C:\\ws2\\a.pdf', 'C:\\ws', 'win32')).toBe(false)
    expect(isPathInside('/ws2/a.pdf', '/ws', 'linux')).toBe(false)
    expect(isPathInside('/ws/deep/a.pdf', '/ws/', 'linux')).toBe(true)
    expect(isPathInside('/ws', '/ws', 'linux')).toBe(true)
  })

  test('case-insensitively on Windows only', () => {
    expect(isPathInside('c:\\WS\\a.pdf', 'C:\\ws', 'win32')).toBe(true)
    expect(isPathInside('/WS/a.pdf', '/ws', 'linux')).toBe(false)
  })

  test('a relative climb out is outside', () => {
    expect(isPathInside('C:\\ws\\..\\other\\a.pdf', 'C:\\ws', 'win32')).toBe(false)
  })
})

describe('parseAttachArgs', () => {
  const none = () => false
  test('nothing typed is no paths', () => {
    expect(parseAttachArgs('   ', none)).toEqual([])
  })

  test('a bare path with spaces is one path when it exists', () => {
    expect(parseAttachArgs('My Report.pdf', (p) => p === 'My Report.pdf')).toEqual(['My Report.pdf'])
  })

  test('otherwise words are paths', () => {
    expect(parseAttachArgs('a.txt b.pdf', none)).toEqual(['a.txt', 'b.pdf'])
  })

  test('quoted paths keep their spaces', () => {
    expect(parseAttachArgs(`"C:\\My Docs\\a b.pdf" notes.txt 'x y.md'`, none)).toEqual([
      'C:\\My Docs\\a b.pdf',
      'notes.txt',
      'x y.md',
    ])
  })
})

test('a reference names the path and the tool to read it with', () => {
  expect(fileReferenceText('/ws/report.pdf', 'read_document')).toBe(
    '[File: /ws/report.pdf]\n(an attached document: read it with the read_document tool.)',
  )
  expect(fileReferenceText('/ws/app.wasm', 'read_file')).toContain('read_file tool')
})
