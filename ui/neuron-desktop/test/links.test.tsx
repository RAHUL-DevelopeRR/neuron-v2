import { afterEach, describe, expect, spyOn, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

import { App } from '../src/App'
import { hostPlatform } from '../src/platform/host'
import { safeExternalUrl } from '../src/platform/links'
import { FakeEngine, fakePlatform } from './fake-platform'

afterEach(cleanup)

const BAD = ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,hi', 'file:///etc/passwd', 'not a url', '', '//evil.example']

describe('external links', () => {
  test('only http and https URLs pass', () => {
    expect(safeExternalUrl('https://zero-x.live/a?b=1')).toBe('https://zero-x.live/a?b=1')
    expect(safeExternalUrl('http://localhost:3000')).toBe('http://localhost:3000/')
    for (const url of BAD) expect(safeExternalUrl(url)).toBeNull()
  })

  test('the browser build never opens anything else', () => {
    const open = spyOn(window, 'open').mockImplementation(() => null)
    try {
      const platform = hostPlatform({ url: 'ws://127.0.0.1:1/neuron', token: 'x'.repeat(24) })
      for (const url of BAD) platform.openExternal(url)
      expect(open).not.toHaveBeenCalled()
      platform.openExternal('https://zero-x.live')
      expect(open).toHaveBeenCalledTimes(1)
    } finally {
      open.mockRestore()
    }
  })

  test('a report link of javascript: never reaches the platform', async () => {
    const engine = new FakeEngine({})
    const platform = fakePlatform(engine)
    render(<App platform={platform} />)
    await screen.findByLabelText('Message Neuron')
    act(() => {
      engine.emit({
        type: 'report',
        command: 'usage',
        title: 'Usage today',
        sections: [],
        links: [
          { label: 'Bad link', url: 'javascript:alert(1)' },
          { label: 'Good link', url: 'https://zero-x.live/dashboard' },
        ],
      } as Parameters<FakeEngine['emit']>[0])
    })
    fireEvent.click(await screen.findByRole('button', { name: 'Bad link' }))
    expect(platform.opened).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Good link' }))
    expect(platform.opened).toEqual(['https://zero-x.live/dashboard'])
  })
})
