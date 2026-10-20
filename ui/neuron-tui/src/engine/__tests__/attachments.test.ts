import { describe, expect, test } from 'bun:test'

import { engineAttachments } from '../attachments'

describe('engineAttachments', () => {
  test('ready images go to the engine; files and text stay in the prompt', () => {
    expect(
      engineAttachments([
        {
          kind: 'image',
          path: '/tmp/shot.PNG',
          filename: 'shot.PNG',
          status: 'ready',
        },
        {
          kind: 'image',
          path: '/tmp/pasted',
          filename: 'pasted',
          status: 'ready',
          processedImage: { base64: 'AAAA', mediaType: 'image/jpeg' },
        },
        { kind: 'image', path: '/tmp/broken.png', filename: 'broken.png', status: 'error' },
        {
          kind: 'file',
          id: 'f1',
          path: '/work/a.rs',
          filename: 'a.rs',
          isDirectory: false,
          content: 'fn main() {}',
          status: 'ready',
        },
        { kind: 'text', id: 't1', content: 'long text', preview: 'long', charCount: 9 },
      ]),
    ).toEqual([
      { kind: 'image', path: '/tmp/shot.PNG', media_type: 'image/png' },
      { kind: 'image', path: '/tmp/pasted', media_type: 'image/jpeg' },
    ])
    expect(engineAttachments(undefined)).toEqual([])
  })
})
