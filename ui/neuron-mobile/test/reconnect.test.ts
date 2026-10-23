/** The reconnect state machine, and the link that runs it, on a fake clock and fake sockets. */
import { describe, expect, test } from 'bun:test'

import { CloseCode } from '../src/relay/contract'
import { RelayLink, type RelaySocket, type Timers } from '../src/relay/link'
import { backoffDelay, initialReconnect, stepReconnect, type BackoffOptions, type ReconnectEvent, type ReconnectState } from '../src/relay/reconnect'

const options: BackoffOptions = { baseMs: 500, maxMs: 30_000, factor: 2, stableMs: 10_000, random: () => 0.999 }

function run(events: ReconnectEvent[], from: ReconnectState = initialReconnect()) {
  let state = from
  const effects: string[] = []
  for (const event of events) {
    const next = stepReconnect(state, event, options)
    state = next.state
    effects.push(...next.effects.map((effect) => (effect.type === 'schedule' ? `schedule:${effect.ms}` : effect.type)))
  }
  return { state, effects }
}

describe('backoff', () => {
  test('grows exponentially to the cap, with full jitter and a floor', () => {
    expect([0, 1, 2, 3, 4, 5, 6, 7, 10].map((attempt) => backoffDelay(attempt, options))).toEqual([
      500, 999, 1998, 3996, 7992, 15984, 29970, 29970, 29970,
    ])
    expect(backoffDelay(3, { ...options, random: () => 0 })).toBe(50)
    expect(backoffDelay(3, { ...options, random: () => 0.5 })).toBe(2000)
  })
})

describe('stepReconnect', () => {
  test('connects, opens and resumes', () => {
    const { state, effects } = run([{ type: 'start' }, { type: 'opened', at: 0 }])
    expect(state.status).toBe('open')
    expect(effects).toEqual(['cancel_timer', 'connect', 'resume'])
  })

  test('a drop waits a growing delay, then reconnects; a stable connection starts over', () => {
    const dropped = run([{ type: 'start' }, { type: 'opened', at: 0 }, { type: 'closed', code: 1006, at: 100 }])
    expect(dropped.state).toMatchObject({ status: 'waiting', attempt: 1, delayMs: 500 })
    expect(dropped.effects.at(-1)).toBe('schedule:500')

    const again = run([{ type: 'timer' }, { type: 'closed', code: 1006, at: 200 }, { type: 'timer' }, { type: 'closed', code: null, at: 300 }], dropped.state)
    expect(again.effects).toEqual(['connect', 'schedule:999', 'connect', 'schedule:1998'])
    expect(again.state.attempt).toBe(3)

    // Up for longer than stableMs: the next drop waits the shortest delay again.
    const stable = run([{ type: 'timer' }, { type: 'opened', at: 1_000 }, { type: 'closed', code: 1006, at: 20_000 }], again.state)
    expect(stable.state).toMatchObject({ status: 'waiting', attempt: 1, delayMs: 500 })

    // Flapping (drops right after opening) keeps backing off.
    const flap = run([{ type: 'timer' }, { type: 'opened', at: 1_000 }, { type: 'closed', code: 1006, at: 1_500 }], again.state)
    expect(flap.state).toMatchObject({ attempt: 4, delayMs: 3996 })
  })

  test('codes reconnecting cannot fix stop it until the user acts', () => {
    for (const code of [CloseCode.unauthorized, CloseCode.forbidden, CloseCode.protocol, CloseCode.replaced]) {
      const { state, effects } = run([{ type: 'start' }, { type: 'opened', at: 0 }, { type: 'closed', code, at: 5 }])
      expect(state.status).toBe('failed')
      expect(effects.at(-1)).toBe('cancel_timer')
      expect(run([{ type: 'timer' }, { type: 'online' }, { type: 'foreground' }], state).effects).toEqual([])
    }
    const signedOut = run([{ type: 'start' }, { type: 'closed', code: CloseCode.unauthorized, at: 0 }])
    expect(signedOut.state.lastError).toBe('Your Zero-X sign-in expired. Sign in again.')
    // Signing in again starts afresh.
    expect(run([{ type: 'start' }], signedOut.state).state).toMatchObject({ status: 'connecting', attempt: 0 })
  })

  test('over a limit, it waits the longest delay', () => {
    const { state } = run([{ type: 'start' }, { type: 'closed', code: CloseCode.limited, at: 0 }])
    expect(state.delayMs).toBe(30_000)
  })

  test('offline waits for the network, online and foreground retry at once', () => {
    const waiting = run([{ type: 'start' }, { type: 'opened', at: 0 }, { type: 'closed', code: 1006, at: 1 }]).state
    const offline = run([{ type: 'offline' }], waiting)
    expect(offline.state.status).toBe('offline')
    expect(offline.effects).toEqual(['cancel_timer'])
    // A drop while offline does not schedule anything.
    expect(run([{ type: 'closed', code: 1006, at: 2 }], { ...offline.state, status: 'open', openedAt: 1 }).effects).toEqual([])
    expect(run([{ type: 'online' }], offline.state)).toMatchObject({ state: { status: 'connecting', online: true }, effects: ['cancel_timer', 'connect'] })
    expect(run([{ type: 'foreground' }], waiting).effects).toEqual(['cancel_timer', 'connect'])
    // Starting while offline waits.
    expect(run([{ type: 'start' }], initialReconnect(false)).state.status).toBe('offline')
  })

  test('stop is final, and late events after it do nothing', () => {
    const { state, effects } = run([{ type: 'start' }, { type: 'opened', at: 0 }, { type: 'stop' }])
    expect(state.status).toBe('idle')
    expect(effects.slice(-2)).toEqual(['cancel_timer', 'disconnect'])
    expect(run([{ type: 'closed', code: 1006, at: 9 }, { type: 'timer' }], state).effects).toEqual([])
  })
})

class FakeSocket implements RelaySocket {
  readyState = 0
  sent: string[] = []
  onopen: ((event: unknown) => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: ((event: { code: number; reason: string }) => void) | null = null
  onerror: ((event: unknown) => void) | null = null
  constructor(
    readonly url: string,
    readonly protocols: string[],
  ) {}
  open() {
    this.readyState = 1
    this.onopen?.({})
  }
  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
  send(data: string) {
    this.sent.push(data)
  }
  close(code = 1000, reason = '') {
    if (this.readyState === 3) return
    this.readyState = 3
    this.onclose?.({ code, reason })
  }
}

class FakeTimers implements Timers {
  time = 0
  private queue: Array<{ at: number; callback: () => void; id: number }> = []
  private next = 1
  setTimeout(callback: () => void, ms: number) {
    const id = this.next++
    this.queue.push({ at: this.time + ms, callback, id })
    return id
  }
  clearTimeout(handle: unknown) {
    this.queue = this.queue.filter((entry) => entry.id !== handle)
  }
  now() {
    return this.time
  }
  advance(ms: number) {
    const until = this.time + ms
    for (;;) {
      this.queue.sort((a, b) => a.at - b.at)
      const due = this.queue[0]
      if (!due || due.at > until) break
      this.queue.shift()
      this.time = due.at
      due.callback()
    }
    this.time = until
  }
}

describe('RelayLink', () => {
  function makeLink(token: string | null = 'ses_test') {
    const sockets: FakeSocket[] = []
    const timers = new FakeTimers()
    const link = new RelayLink({
      url: 'wss://relay.example/v1/relay',
      role: 'phone',
      device: { id: 'phone-device-1', name: 'Test phone', platform: 'ios' },
      token: () => token,
      connect: (url, protocols) => {
        const socket = new FakeSocket(url, protocols)
        sockets.push(socket)
        return socket
      },
      backoff: { random: () => 0.5 },
      timers,
    })
    return { link, sockets, timers }
  }

  const joined = { t: 'joined', relay: 1, role: 'phone', device_id: 'phone-device-1', account: { tag: '0123456789abcdef' } }

  test('puts the token in a subprotocol, joins, then resumes on every reconnect', () => {
    const { link, sockets, timers } = makeLink()
    let readies = 0
    link.onReady(() => readies++)
    link.start()
    expect(sockets[0].url).toBe('wss://relay.example/v1/relay')
    expect(sockets[0].protocols).toEqual(['neuron-relay.v1', 'neuron-auth.ses_test'])
    sockets[0].open()
    expect(JSON.parse(sockets[0].sent[0])).toEqual({ t: 'join', role: 'phone', device: { id: 'phone-device-1', name: 'Test phone', platform: 'ios' } })
    expect(link.ready).toBe(false)
    sockets[0].receive(joined)
    expect(link.ready).toBe(true)
    expect(link.accountTag).toBe('0123456789abcdef')
    expect(readies).toBe(1)

    sockets[0].close(1006, '')
    expect(link.state).toMatchObject({ status: 'waiting', delayMs: 250 })
    timers.advance(249)
    expect(sockets.length).toBe(1)
    timers.advance(1)
    expect(sockets.length).toBe(2)
    sockets[1].open()
    sockets[1].receive(joined)
    expect(readies).toBe(2)
  })

  test('a socket that stops answering pings is dropped and replaced', () => {
    const { link, sockets, timers } = makeLink()
    link.start()
    sockets[0].open()
    sockets[0].receive(joined)
    timers.advance(25_000)
    expect(sockets[0].sent.at(-1)).toBe('{"t":"ping"}')
    timers.advance(10_000)
    expect(sockets[0].readyState).toBe(3)
    expect(link.state.status).toBe('waiting')
  })

  test('no join answer in time closes the socket', () => {
    const { link, sockets, timers } = makeLink()
    link.start()
    sockets[0].open()
    timers.advance(10_000)
    expect(sockets[0].readyState).toBe(3)
  })

  test('without a token it fails at once, with words to show', () => {
    const { link, sockets } = makeLink(null)
    link.start()
    expect(sockets.length).toBe(0)
    expect(link.state).toMatchObject({ status: 'failed', lastError: 'Your Zero-X sign-in expired. Sign in again.' })
  })

  test('send waits for the join; stop closes and stays stopped', () => {
    const { link, sockets, timers } = makeLink()
    link.start()
    expect(link.send({ t: 'list' })).toBe(false)
    sockets[0].open()
    sockets[0].receive(joined)
    expect(link.send({ t: 'list' })).toBe(true)
    link.stop()
    expect(sockets[0].readyState).toBe(3)
    timers.advance(60_000)
    expect(sockets.length).toBe(1)
    expect(link.state.status).toBe('idle')
  })
})
