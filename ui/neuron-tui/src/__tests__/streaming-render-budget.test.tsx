import { describe, expect, test } from 'bun:test'

import { runStreamingBench } from '../testing/streaming-bench'

/**
 * What a streaming turn may cost the chat around it: the transcript's
 * finished messages, the status bar and the prompt status bar are not drawn
 * again for it, the status line's activity only when what it says changes,
 * and the streaming message once per batched update. See
 * src/testing/streaming-bench.tsx and `bun run bench:streaming`.
 */
describe('a streaming turn', () => {
  test('draws only what changed', async () => {
    const tokens = 160
    const result = await runStreamingBench({
      tokens,
      toolEvery: 80,
      historyTurns: 2,
      tokenIntervalMs: 1,
    })
    expect(result.toolCalls).toBe(2)
    expect(result.renders.oldMessages).toBe(0)
    expect(result.renders.statusBar).toBe(0)
    expect(result.renders.promptStatusBar).toBe(0)
    // Once per update of the message list, not twice (the tree it reads stays the same).
    expect(result.renders.streamingMessage).toBeLessThanOrEqual(result.transcriptUpdates)
    // Per change of what it says and per clock tick, far fewer than per token.
    expect(result.renders.liveActivityLine).toBeLessThan(tokens / 4)
    // Mixed Markdown is cut into finished blocks: updates stay quick.
    expect(result.quickUpdatesOfLongText).toBe(0)
  }, 120_000)

  test('a long list is not parsed whole 30 times a second', async () => {
    // About 2.6 KB of list with no blank line in it: no finished block to
    // cut off, so each update parses all of it. Past about 1 KB the updates
    // come every 100 ms instead of every 33 ms.
    const result = await runStreamingBench({
      tokens: 460,
      toolEvery: 0,
      historyTurns: 1,
      tokenIntervalMs: 2,
      shape: 'list',
    })
    expect(result.transcriptUpdates).toBeGreaterThan(5)
    expect(result.quickUpdatesOfLongText).toBe(0)
    expect(result.renders.oldMessages).toBe(0)
    expect(result.renders.streamingMessage).toBeLessThanOrEqual(result.transcriptUpdates)
  }, 180_000)
})
