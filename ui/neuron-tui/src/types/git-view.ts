import type { DiffEvent, RewoundEvent } from '../engine/protocol'

/**
 * A git view kept in the transcript (a `git-view` block): a `/diff`, or what
 * a rewind put back. Plain data, so it is saved with the chat.
 */
export type GitView =
  | ({ kind: 'diff' } & Omit<DiffEvent, 'type'>)
  | ({ kind: 'rewound' } & Omit<RewoundEvent, 'type'>)
