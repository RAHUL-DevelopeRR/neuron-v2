// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// post-login Discord banner is gone, so this covers the git-root banner.
// See ui/neuron-tui/PORTING.md.
import { afterEach, beforeAll, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { useChatStore } from '../../state/chat-store'
import { TopBanner } from '../top-banner'

beforeAll(initializeThemeStore)
afterEach(() => useChatStore.getState().setActiveTopBanner(null))

test.each([100, 48])(
  'the git-root banner offers to switch to the repository root at %s columns',
  async (width) => {
    useChatStore.getState().setActiveTopBanner('gitRoot')
    const setup = await createTestRenderer({ width, height: 12 })
    const root = createRoot(setup.renderer)
    try {
      flushSync(() =>
        root.render(
          <TopBanner gitRoot="/work/repo" onSwitchToGitRoot={() => {}} />,
        ),
      )
      await setup.renderOnce()
      const frame = setup.captureCharFrame()
      // word-wrapped at the narrow width, so compare with the borders, the close
      // button's lone "x" and the wrapping whitespace taken out
      const text = frame
        .replace(/[│─╭╮╰╯┌┐└┘]/g, ' ')
        .replace(/\s+/g, ' ')
        .replace(/ x /g, ' ')
      expect(text).toContain('in a subdirectory of a git repo.')
      expect(text).toContain('Switch to git root')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  },
)
