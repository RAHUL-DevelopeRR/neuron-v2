/**
 * Counts how often the chat's busiest components draw, for the streaming
 * benchmark and its render-budget test (src/testing/streaming-bench.tsx).
 * Off (`null`) everywhere else: a component pays one null check per render.
 */

export type ProbedComponent =
  | 'MessageWithAgents'
  | 'StatusBar'
  | 'LiveActivityLine'
  | 'PromptStatusBar'

export const renderProbe: {
  /** Called once per render of a probed component, with its name and key. */
  onRender: ((component: ProbedComponent, key?: string) => void) | null
} = { onRender: null }
