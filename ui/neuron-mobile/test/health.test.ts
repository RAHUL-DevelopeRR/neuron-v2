/**
 * The phone's connection health: what the session screen says about it, and
 * waking the connection when the app comes back to the foreground. No socket
 * is opened: the transport and the app state are fakes.
 */
import { describe, expect, test } from 'bun:test'

import { healthText, watchForeground, type AppStateLike } from '../src/connection'

describe('healthText', () => {
  test('says nothing while connected', () => {
    expect(healthText({ state: 'connected' })).toBeNull()
  })

  test('words each state plainly', () => {
    expect(healthText({ state: 'connecting' })).toBe('Connecting…')
    expect(healthText({ state: 'reconnecting', attempt: 2, retryInMs: 4_000 })).toBe('Reconnecting… (2), next try in 4 s')
    expect(healthText({ state: 'reconnecting', attempt: 1, retryInMs: 1_100 })).toBe('Reconnecting… (1), next try in 2 s')
    expect(healthText({ state: 'reconnecting', attempt: 3, retryInMs: 0 })).toBe('Reconnecting… (3)')
    expect(healthText({ state: 'offline', reason: 'The pairing token is not right.' })).toBe('Offline: The pairing token is not right.')
    expect(healthText({ state: 'offline', reason: null })).toBe('Offline')
  })
})

class FakeAppState implements AppStateLike {
  currentState = 'active'
  listeners = new Set<(state: string) => void>()
  addEventListener(_type: 'change', listener: (state: string) => void) {
    this.listeners.add(listener)
    return { remove: () => void this.listeners.delete(listener) }
  }
  set(state: string) {
    this.currentState = state
    for (const listener of [...this.listeners]) listener(state)
  }
}

describe('watchForeground', () => {
  test('wakes the connection each time the app comes back, and not otherwise', () => {
    const appState = new FakeAppState()
    let wakes = 0
    const stop = watchForeground(appState, { wake: () => void wakes++ })
    appState.set('inactive')
    appState.set('background')
    expect(wakes).toBe(0)
    appState.set('active')
    expect(wakes).toBe(1)
    // Active to active is no return.
    appState.set('active')
    expect(wakes).toBe(1)
    appState.set('background')
    appState.set('active')
    expect(wakes).toBe(2)
    stop()
    appState.set('background')
    appState.set('active')
    expect(wakes).toBe(2)
    expect(appState.listeners.size).toBe(0)
  })
})
