/**
 * One end of an encrypted channel: its keys and its frame counters. Counters
 * start at 0 in each direction and go up by one per frame; a receiver takes
 * only the next one, so the relay cannot replay, reorder or drop a frame
 * without the channel noticing. Frames the phone sent stay in an outbox until
 * the relay reports the computer got them (`resumed.received`), so a dropped
 * socket loses nothing.
 */

import { E2EError, openFrame, sealFrame, type ChannelKeys, type Direction } from './crypto'
import type { Role } from './contract'

const MAX_OUTBOX = 512

export type Opened = { kind: 'frame'; text: string } | { kind: 'duplicate' } | { kind: 'gap'; expected: number }

export class ChannelCrypto {
  /** The counter of the next frame this end sends. */
  nextSend = 0
  /** The counter of the last frame this end opened (-1: none yet). */
  lastReceived = -1
  private outbox: Array<{ n: number; c: string }> = []

  constructor(
    readonly ch: string,
    readonly role: Role,
    private readonly keys: ChannelKeys,
  ) {}

  private get sendDir(): Direction {
    return this.role === 'phone' ? 'p2h' : 'h2p'
  }

  private get receiveDir(): Direction {
    return this.role === 'phone' ? 'h2p' : 'p2h'
  }

  /** Seals the next frame and keeps it for a resend. */
  seal(text: string): { n: number; c: string } {
    const n = this.nextSend++
    const c = sealFrame(this.role === 'phone' ? this.keys.p2h : this.keys.h2p, this.ch, this.sendDir, n, text)
    const frame = { n, c }
    this.outbox.push(frame)
    // The oldest go first; `acknowledge` notices if the far end still needed them.
    if (this.outbox.length > MAX_OUTBOX) this.outbox.shift()
    return frame
  }

  /** Opens a received frame. Throws {@link E2EError} when it does not open. */
  open(n: number, c: unknown): Opened {
    if (!Number.isSafeInteger(n) || n < 0) throw new E2EError('bad_frame', 'A frame has no valid counter.')
    if (n <= this.lastReceived) return { kind: 'duplicate' }
    if (n !== this.lastReceived + 1) return { kind: 'gap', expected: this.lastReceived + 1 }
    const text = openFrame(this.role === 'phone' ? this.keys.h2p : this.keys.p2h, this.ch, this.receiveDir, n, c)
    this.lastReceived = n
    return { kind: 'frame', text }
  }

  /**
   * The far end got every frame up to `received`: forget those, and return the
   * rest to send again. `null` when some of the rest are gone (the outbox
   * overflowed past them), so the channel cannot be resumed.
   */
  acknowledge(received: number): Array<{ n: number; c: string }> | null {
    this.outbox = this.outbox.filter((frame) => frame.n > received)
    const first = this.outbox[0]?.n ?? this.nextSend
    if (first !== received + 1) return null
    return this.outbox.slice()
  }
}
