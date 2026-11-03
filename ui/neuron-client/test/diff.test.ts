import { describe, expect, test } from 'bun:test'

import { additionDiff, diffLines, parseUnifiedDiff } from '../src/diff'

const render = (diff: ReturnType<typeof diffLines>) =>
  diff.lines.map((line) => ({ hunk: '@', add: '+', remove: '-', context: ' ' })[line.kind] + line.text)

describe('diffLines', () => {
  test('a changed line in the middle, with context around it', () => {
    const before = 'a\nb\nc\nd\ne\nf\ng\nh\n'
    const after = 'a\nb\nc\nd\nE\nf\ng\nh\n'
    const diff = diffLines(before, after, { context: 1 })
    expect(render(diff)).toEqual([' d', '-e', '+E', ' f'])
    expect(diff.added).toBe(1)
    expect(diff.removed).toBe(1)
    expect(diff.lines[1].oldNumber).toBe(5)
    expect(diff.lines[2].newNumber).toBe(5)
  })

  test('far-apart changes become separate hunks', () => {
    const before = Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n')
    const after = before.replace('line 2', 'line two').replace('line 17', 'line seventeen')
    const diff = diffLines(before, after, { context: 1 })
    expect(render(diff)).toEqual([
      ' line 1',
      '-line 2',
      '+line two',
      ' line 3',
      '@…',
      ' line 16',
      '-line 17',
      '+line seventeen',
      ' line 18',
    ])
  })

  test('insertions and deletions, with CRLF treated as LF', () => {
    expect(render(diffLines('one\r\ntwo\r\n', 'one\r\nnew\r\ntwo\r\n'))).toEqual([' one', '+new', ' two'])
    expect(render(diffLines('x\ny\nz', 'x\nz'))).toEqual([' x', '-y', ' z'])
    expect(render(diffLines('', 'fresh'))).toEqual(['+fresh'])
    expect(diffLines('same', 'same').lines).toEqual([])
  })

  test('firstLine numbers an edit inside its file', () => {
    const diff = diffLines('old', 'new', { firstLine: 41 })
    expect(diff.lines.map((line) => [line.oldNumber, line.newNumber])).toEqual([
      [41, undefined],
      [undefined, 41],
    ])
  })

  test('long diffs are cut, and still count every line', () => {
    const diff = additionDiff(Array.from({ length: 50 }, (_, i) => `l${i}`).join('\n'), 10)
    expect(diff.lines).toHaveLength(10)
    expect(diff.added).toBe(50)
    expect(diff.truncated).toBe(true)
  })
})

describe('parseUnifiedDiff', () => {
  test('reads the engine edit_file diff with line numbers', () => {
    const diff = parseUnifiedDiff(
      ['--- a/src/lib.rs', '+++ b/src/lib.rs', '@@ -10,3 +10,3 @@ fn main', ' keep', '-old', '+new', ' tail', '\\ No newline at end of file'].join('\n'),
    )
    expect(diff.lines.map((line) => [line.kind, line.text, line.oldNumber ?? null, line.newNumber ?? null])).toEqual([
      ['hunk', '@@ -10,3 +10,3 @@ fn main', null, null],
      ['context', 'keep', 10, 10],
      ['remove', 'old', 11, null],
      ['add', 'new', null, 11],
      ['context', 'tail', 12, 12],
    ])
    expect([diff.added, diff.removed]).toEqual([1, 1])
  })
})
