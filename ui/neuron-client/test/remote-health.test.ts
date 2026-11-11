/**
 * The remote transport's connection health: keepalives, the silence watchdog,
 * jittered backoff on reconnect and the stall report. A fake socket and a fake
 * clock stand in for the network and time; no real socket is opened.
 */
import { describe, expect, test } from 'bun:test'

import { initialSession } from '../src/session'
import {
  RECONNECT_BASE_MS,
  RECONNECT_CAP_MS,
  reconnectDelay,
  WebSocketTransport,
  type ConnectionHealth,
  type ConnectionStall,
  type HostFrame,
  type SocketLike,
  type TransportClock,
  type WebSocketTransportOptions,
} from '../src/remote'
import type { TransportClose } from '../src/controller'

class FakeClock implements TransportClock {
  time = 0
  private nextId = 1
  private timers = new Map<number, { at: number; fn: () => void; every: number | null }>()
  now = () => this.time
  setTimeout = (fn: () => void, ms: number) => this.add(fn, ms, null)
  setInterval = (fn: () => void, ms: number) => this.add(fn, ms, ms)
  clearTimeout = (id: unknown) => void this.timers.delete(id as number)
  clearInterval = (id: unknown) => void this.timers.delete(id as number)
  private add(fn: () => void, ms: number, every: number | null): number {
    const id = this.nextId++
    this.timers.set(id, { at: this.time + Math.max(0, ms), fn, every })
    return id
  }
  /** Moves time on, firing every timer that falls due in order. */
  advance(ms: number): void {
    const end = this.time + ms
    for (;;) {
      let due: [number, { at: number; fn: () => void; every: number | null }] | null = null
      for (const entry of this.timers) if (entry[1].at <= end && (!due || entry[1].at < due[1].at)) due = entry
      if (!due) break
      const [id, timer] = due
      this.time = timer.at
      if (timer.every === null) this.timers.delete(id)
      else timer.at += Math.max(1, timer.every)
      timer.fn()
    }
    this.time = end
  }
}

class FakeSocket implements SocketLike {
  readyState = 0
  sent: Array<Record<string, unknown>> = []
  closedWith: { code?: number; reason?: string } | null = null
  onopen: ((event: unknown) => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: ((event: { code: number; reason: string }) => void) | null = null
  onerror: ((event: unknown) => void) | null = null
  constructor(readonly url: string) {}
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close(code?: number, reason?: string) {
    this.closedWith = { code, reason }
    this.readyState = 3
  }
  open() {
    this.readyState = 1
    this.onopen?.({})
  }
  receive(frame: HostFrame) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
  /** The network or the host ends the connection. */
  drop(code = 1006, reason = '') {
    this.readyState = 3
    this.onclose?.({ code, reason })
  }
  pings() {
    return this.sent.filter((frame) => frame.t === 'ping').length
  }
}

const readyState = () => ({ ...initialSession(), connection: 'ready' as const })

function setup(options: Partial<WebSocketTransportOptions> = {}) {
  const clock = new FakeClock()
  const sockets: FakeSocket[] = []
  const transport = new WebSocketTransport({
    url: 'ws://host/neuron',
    token: 't',
    client: { name: 'phone', kind: 'mobile' },
    connect: (url) => {
      const socket = new FakeSocket(url)
      sockets.push(socket)
      return socket
    },
    clock,
    random: () => 0,
    reconnect: true,
    ...options,
  })
  const health: ConnectionHealth[] = []
  const closes: TransportClose[] = []
  const stalls: ConnectionStall[] = []
  const snapshots: number[] = []
  transport.onHealth((value) => health.push(value))
  transport.onClose((close) => closes.push(close))
  transport.onStall((stall) => stalls.push(stall))
  transport.onSnapshot(() => snapshots.push(clock.time))
  const last = () => sockets[sockets.length - 1]
  /** The host welcomes the newest socket and sends its snapshot. */
  const welcome = () => {
    last().open()
    last().receive({ t: 'welcome', protocol: 1, host: { name: 'laptop', version: '1', cwd: '/w' }, clients: 1 })
    last().receive({ t: 'snapshot', seq: 0, state: readyState() })
  }
  const start = async () => {
    const started = transport.start()
    welcome()
    await started
  }
  return { clock, sockets, transport, health, closes, stalls, snapshots, last, welcome, start }
}

describe('reconnectDelay', () => {
  test('doubles from the base, stops at the cap, and adds up to a fifth of jitter', () => {
    expect([1, 2, 3, 4, 5, 6, 7, 10].map((attempt) => reconnectDelay(attempt, () => 0))).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000])
    expect(reconnectDelay(1, () => 1)).toBe(RECONNECT_BASE_MS * 1.2)
    expect(reconnectDelay(50, () => 0.5)).toBe(RECONNECT_CAP_MS * 1.1)
  })
})

describe('keepalive', () => {
  test('pings every 15 s once welcomed, and a host that answers stays connected', async () => {
    const { clock, sockets, transport, last, start } = setup()
    await start()
    expect(transport.health).toEqual({ state: 'connected' })
    for (let tick = 0; tick < 6; tick++) {
      clock.advance(15_000)
      last().receive({ t: 'pong' })
    }
    expect(last().pings()).toBe(6)
    expect(sockets).toHaveLength(1)
    expect(transport.health).toEqual({ state: 'connected' })
    expect(transport.stats().lastFrameAgeMs).toBe(0)
  })

  test('no pings before the welcome', () => {
    const { clock, last, transport } = setup({ timeoutMs: 120_000, silenceMs: 120_000 })
    void transport.start().catch(() => {})
    last().open()
    clock.advance(40_000)
    expect(last().pings()).toBe(0)
  })
})

describe('the silence watchdog', () => {
  test('30 s without a frame drops the socket, reports the stall and reconnects with backoff', async () => {
    const { clock, sockets, transport, health, closes, stalls, snapshots, welcome, start } = setup()
    await start()
    clock.advance(30_000)
    expect(sockets[0].closedWith).not.toBeNull()
    expect(stalls).toHaveLength(1)
    expect(stalls[0]).toMatchObject({ phase: 'stream', frames: 2 })
    expect(stalls[0].silentMs).toBeGreaterThanOrEqual(30_000)
    expect(transport.health).toEqual({ state: 'reconnecting', attempt: 1, retryInMs: 1000 })
    // The session is not over: nobody is told it closed.
    expect(closes).toEqual([])
    clock.advance(999)
    expect(sockets).toHaveLength(1)
    clock.advance(1)
    expect(sockets).toHaveLength(2)
    welcome()
    expect(transport.health).toEqual({ state: 'connected' })
    expect(snapshots).toHaveLength(2)
    expect(health.map((value) => value.state)).toEqual(['connecting', 'connected', 'reconnecting', 'connected'])
  })

  test('a host that never welcomes is a stall in the open phase', async () => {
    const { clock, transport, stalls, last } = setup({ timeoutMs: 120_000 })
    const started = transport.start()
    last().open()
    clock.advance(30_000)
    await expect(started).rejects.toThrow(/stopped answering/)
    expect(stalls.map((stall) => stall.phase)).toEqual(['open'])
  })

  test('keepalive answers keep a quiet session attached', async () => {
    const { clock, sockets, stalls, last, start } = setup()
    await start()
    for (let tick = 0; tick < 20; tick++) {
      clock.advance(15_000)
      last().receive({ t: 'pong' })
    }
    expect(stalls).toEqual([])
    expect(sockets).toHaveLength(1)
  })
})

describe('reconnecting', () => {
  test('each failed attempt waits longer, up to the cap', async () => {
    const { clock, sockets, transport, start } = setup({ reconnect: { maxAttempts: 20 } })
    await start()
    sockets[0].drop()
    const waits: number[] = []
    for (let attempt = 1; attempt <= 8; attempt++) {
      const health = transport.health
      expect(health.state).toBe('reconnecting')
      if (health.state !== 'reconnecting') return
      expect(health.attempt).toBe(attempt)
      waits.push(health.retryInMs)
      clock.advance(health.retryInMs)
      expect(sockets).toHaveLength(attempt + 1)
      sockets[attempt].drop()
    }
    expect(waits).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000])
  })

  test('a reconnect resets the backoff', async () => {
    const { clock, sockets, transport, welcome, start } = setup()
    await start()
    sockets[0].drop()
    clock.advance(1000)
    sockets[1].drop()
    expect(transport.health).toMatchObject({ attempt: 2, retryInMs: 2000 })
    clock.advance(2000)
    welcome()
    sockets[2].drop()
    expect(transport.health).toMatchObject({ attempt: 1, retryInMs: 1000 })
  })

  test('jitter comes from the random source', async () => {
    const { sockets, transport, start } = setup({ random: () => 0.5 })
    await start()
    sockets[0].drop()
    expect(transport.health).toMatchObject({ attempt: 1, retryInMs: 1100 })
  })

  test('after the last attempt it is offline, and the session is told', async () => {
    const { clock, sockets, transport, closes, start } = setup({ reconnect: { maxAttempts: 2 } })
    await start()
    sockets[0].drop()
    clock.advance(1000)
    sockets[1].drop()
    clock.advance(2000)
    sockets[2].drop(1006, 'gone')
    expect(transport.health).toEqual({ state: 'offline', reason: 'gone' })
    expect(closes).toEqual([{ code: 1006, error: 'gone' }])
    clock.advance(120_000)
    expect(sockets).toHaveLength(3)
  })

  test('a host that refuses the pairing token is not retried', async () => {
    const { clock, sockets, transport, closes, start } = setup()
    await start()
    sockets[0].drop()
    clock.advance(1000)
    sockets[1].open()
    sockets[1].receive({ t: 'error', message: 'The pairing token is not right.', fatal: true })
    sockets[1].drop(4401, 'bad token')
    expect(transport.health).toEqual({ state: 'offline', reason: 'The pairing token is not right.' })
    expect(closes).toEqual([{ code: 4401, error: 'The pairing token is not right.' }])
    // Coming back to the foreground does not send the token to that host again; asking does.
    transport.wake()
    expect(sockets).toHaveLength(2)
    transport.reconnectNow()
    expect(sockets).toHaveLength(3)
  })

  test('requests in flight fail when the connection drops', async () => {
    const { sockets, transport, start } = setup()
    await start()
    const call = transport.rpc('fs.list')
    sockets[0].drop()
    await expect(call).rejects.toThrow(/closed the connection/)
    expect(() => transport.send({ kind: 'command', type: 'interrupt' } as never)).toThrow(/Not connected/)
  })

  test('leaving while reconnecting stops trying and tells the session', async () => {
    const { clock, sockets, transport, closes, start } = setup()
    await start()
    sockets[0].drop()
    await transport.close()
    expect(transport.health).toEqual({ state: 'offline', reason: null })
    expect(closes).toEqual([{ code: 1000, error: null }])
    clock.advance(120_000)
    expect(sockets).toHaveLength(1)
  })
})

describe('trying again by hand', () => {
  test('reconnectNow skips the wait', async () => {
    const { clock, sockets, transport, welcome, start } = setup()
    await start()
    sockets[0].drop()
    clock.advance(500)
    transport.reconnectNow()
    expect(sockets).toHaveLength(2)
    expect(transport.health).toMatchObject({ state: 'reconnecting', attempt: 1, retryInMs: 0 })
    welcome()
    expect(transport.health).toEqual({ state: 'connected' })
    // The timer of the skipped wait does not open a third socket.
    clock.advance(5_000)
    expect(sockets).toHaveLength(2)
  })

  test('from offline it starts again', async () => {
    const { clock, sockets, transport, welcome, start } = setup({ reconnect: { maxAttempts: 1 } })
    await start()
    sockets[0].drop()
    clock.advance(1000)
    sockets[1].drop()
    expect(transport.health.state).toBe('offline')
    transport.reconnectNow()
    expect(sockets).toHaveLength(3)
    welcome()
    expect(transport.health).toEqual({ state: 'connected' })
  })

  test('wake: a connection quiet for longer than a keepalive is replaced at once', async () => {
    const { clock, sockets, transport, start } = setup()
    await start()
    // The app slept: no timers ran, the clock moved.
    clock.time += 20_000
    transport.wake()
    expect(sockets[0].closedWith).not.toBeNull()
    expect(sockets).toHaveLength(2)
  })

  test('wake: a fresh connection is only pinged', async () => {
    const { clock, sockets, transport, start } = setup()
    await start()
    clock.time += 5_000
    transport.wake()
    expect(sockets).toHaveLength(1)
    expect(sockets[0].pings()).toBe(1)
  })
})

describe('without reconnect', () => {
  test('a dropped connection closes the session, as before', async () => {
    const { sockets, transport, closes, start } = setup({ reconnect: false })
    await start()
    sockets[0].drop(1006, '')
    expect(closes).toEqual([{ code: 1006, error: 'Disconnected from the Neuron host.' }])
    expect(transport.health).toEqual({ state: 'offline', reason: 'Disconnected from the Neuron host.' })
  })

  test('the watchdog still notices a dead host', async () => {
    const { clock, transport, closes, start } = setup({ reconnect: false })
    await start()
    clock.advance(30_000)
    expect(closes).toEqual([{ code: null, error: 'The Neuron host stopped answering.' }])
    expect(transport.health.state).toBe('offline')
  })
})
