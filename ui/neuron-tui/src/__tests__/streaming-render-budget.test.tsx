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
  }, 120_000)
})
