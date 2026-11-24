import { readFileSync } from 'fs'
import path from 'path'

import { describe, expect, test } from 'bun:test'

import {
  DOCUMENT_EXTENSIONS,
  ENGINE_MAX_READ_BYTES,
  MAX_INLINE_FILE_BYTES,
  documentSizeLimit,
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
  ...overrides,
})

const MB = 1024 * 1024

describe('routeAttachment', () => {
  test('small text and code files are inlined', () => {
    expect(routeAttachment(facts({ path: '/ws/notes.txt' }))).toEqual({ kind: 'text' })
    expect(routeAttachment(facts({ path: '/ws/src/main.rs' }))).toEqual({ kind: 'text' })
    expect(routeAttachment(facts({ path: '/ws/Makefile' }))).toEqual({ kind: 'text' })
  })

  test('a small text file outside the workspace is still inlined: the UI reads it', () => {
    expect(routeAttachment(facts({ path: '/home/me/todo.md', insideWorkspace: false }))).toEqual({ kind: 'text' })
  })

  test('every Office and PDF extension goes to read_document', () => {
    for (const ext of ['.docx', '.pptx', '.xlsx', '.xlsm', '.xlsb', '.xls', '.ods', '.pdf', '.PDF']) {
      expect(routeAttachment(facts({ path: `/ws/report${ext}`, isBinary: true }))).toEqual({
        kind: 'reference',
        tool: 'read_document',
      })
    }
  })

  test("the document extensions are the engine's DOCUMENT_KINDS", () => {
    // Read from the engine's source, so the two lists cannot drift apart.
    const fileOps = path.resolve(import.meta.dir, '../../../../../rust/crates/runtime/src/file_ops.rs')
    const source = readFileSync(fileOps, 'utf8')
    const block = source.slice(source.indexOf('pub const DOCUMENT_KINDS'))
    const list = block.slice(0, block.indexOf('];'))
    const engineKinds = [...list.matchAll(/\("([a-z0-9]+)",/g)].map((match) => `.${match[1]}`)
    expect(engineKinds.length).toBeGreaterThan(0)
    expect([...DOCUMENT_EXTENSIONS].sort()).toEqual(engineKinds.sort())
  })

  test('a document outside the workspace is copied into it, then read', () => {
    expect(
      routeAttachment(facts({ path: 'C:\\Users\\me\\Downloads\\deck.pptx', isBinary: true, insideWorkspace: false })),
    ).toEqual({ kind: 'copy', tool: 'read_document' })
  })

  test("a document over the engine's limit is refused, saying the limit", () => {
    expect(documentSizeLimit('/ws/a.docx')).toBe(200 * MB)
    expect(documentSizeLimit('/ws/a.PPTX')).toBe(200 * MB)
    expect(documentSizeLimit('/ws/a.pdf')).toBe(100 * MB)
    expect(documentSizeLimit('/ws/a.xlsb')).toBe(100 * MB)
    const route = routeAttachment(facts({ path: '/ws/scan.pdf', isBinary: true, size: 100 * MB + 1 }))
    expect(route).toEqual({
      kind: 'refuse',
      reason: 'scan.pdf was not attached: it is larger than 100 MB, the most Neuron reads.',
    })
    expect(routeAttachment(facts({ path: '/ws/big.docx', isBinary: true, size: 150 * MB })).kind).toBe('reference')
  })

  test('a binary file that is not a document is refused: read_file cannot read it', () => {
    for (const insideWorkspace of [true, false]) {
      expect(routeAttachment(facts({ path: '/ws/app.wasm', isBinary: true, insideWorkspace }))).toEqual({
        kind: 'refuse',
        reason: 'app.wasm was not attached: it is a binary file Neuron cannot read.',
      })
    }
    // UTF-16 text has NUL bytes, and the engine reads it as binary too.
    expect(routeAttachment(facts({ path: '/ws/utf16.txt', isBinary: true })).kind).toBe('refuse')
  })

  test('text too long to inline whole goes by path to read_file, copied in from outside', () => {
    expect(MAX_INLINE_FILE_BYTES).toBe(100 * 1024)
    expect(routeAttachment(facts({ path: '/ws/edge.log', size: MAX_INLINE_FILE_BYTES }))).toEqual({ kind: 'text' })
    expect(routeAttachment(facts({ path: '/ws/long.log', size: MAX_INLINE_FILE_BYTES + 1 }))).toEqual({
      kind: 'reference',
      tool: 'read_file',
    })
    expect(
      routeAttachment(facts({ path: '/tmp/long.log', size: MAX_INLINE_FILE_BYTES + 1, insideWorkspace: false })),
    ).toEqual({ kind: 'copy', tool: 'read_file' })
  })

  test("text over the engine's 10 MB read limit is refused", () => {
    expect(ENGINE_MAX_READ_BYTES).toBe(10 * MB)
    expect(routeAttachment(facts({ path: '/ws/max.log', size: ENGINE_MAX_READ_BYTES })).kind).toBe('reference')
    for (const insideWorkspace of [true, false]) {
      const route = routeAttachment(facts({ path: '/ws/huge.log', size: ENGINE_MAX_READ_BYTES + 1, insideWorkspace }))
      expect(route.kind).toBe('refuse')
      if (route.kind === 'refuse') expect(route.reason).toContain('larger than 10 MB')
    }
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

  test('a bare path with an apostrophe and spaces is one path when it exists', () => {
    const exists = (p: string) => p === "Rahul's resume.pdf" || p === "docs/Q3 client's notes.docx"
    expect(parseAttachArgs("Rahul's resume.pdf", exists)).toEqual(["Rahul's resume.pdf"])
    expect(parseAttachArgs("docs/Q3 client's notes.docx", exists)).toEqual(["docs/Q3 client's notes.docx"])
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

test('a reference names the path and the tool to read it with, and a copy its source', () => {
  expect(fileReferenceText('/ws/report.pdf', 'read_document')).toBe(
    '[File: /ws/report.pdf]\n(an attached document: read it with the read_document tool.)',
  )
  expect(fileReferenceText('/ws/long.log', 'read_file')).toBe(
    '[File: /ws/long.log]\n(an attached file: read it with the read_file tool.)',
  )
  expect(fileReferenceText('/ws/.neuron/attachments/ab/deck.pptx', 'read_document', '/home/me/deck.pptx')).toBe(
    '[File: /ws/.neuron/attachments/ab/deck.pptx]\n(an attached document, copied from /home/me/deck.pptx: read it with the read_document tool.)',
  )
})
