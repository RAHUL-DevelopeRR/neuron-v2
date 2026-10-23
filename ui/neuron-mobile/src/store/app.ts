/**
 * The app's state and what changes it: the Zero-X sign-in, paired computers
 * and their relays, settings, push, and which sessions the user has seen.
 * Screens read it with `useApp()` (./context.tsx); nothing here imports React
 * Native, so it runs in the tests as it does on the phone.
 *
 * Secrets stay out of the state: the session token and pair keys live in the
 * secure store and in private fields here, and are never logged.
 */

import { SessionController } from '@neuron/client'

import { Gateway, GatewayError, gatewayForRelay, type Account, type DeviceCode, type LinkedSession } from '../account/gateway'
import { randomId } from '../relay/crypto'
import { RelayLink, type SocketFactory } from '../relay/link'
import { PairingError, TRUSTED_RELAY_HOSTS, parsePairingLink } from '../relay/pairing'
import { PhoneRelay, type HostView, type PairedComputer } from '../relay/phone'
import type { LinkStatus } from '../relay/reconnect'
import { RelayEngineTransport } from '../relay/transport'
import { shareKey } from './sessions'
import { KEYS, type KeyValueStore } from './storage'

export type ThemeSetting = 'system' | 'light' | 'dark'

export interface Settings {
  theme: ThemeSetting
  /** Push when a session waits for an approval or an answer. */
  notifyNeedsMe: boolean
  /** Push when a turn finished. */
  notifyFinished: boolean
}

export const DEFAULT_SETTINGS: Settings = { theme: 'system', notifyNeedsMe: true, notifyFinished: true }

/** A paired computer as the screens see it (no keys). */
export interface ComputerInfo {
  hostId: string
  name: string
  relayUrl: string
  pairedAt: number
}

export type PushState =
  | { status: 'off' }
  | { status: 'registered' }
  | { status: 'unsupported'; reason: string }
  | { status: 'denied' }
  | { status: 'error'; reason: string }

export interface RelayStatus {
  status: LinkStatus
  delayMs: number | null
  error: string | null
}

export interface AppState {
  loaded: boolean
  settings: Settings
  signedIn: boolean
  gateway: string | null
  account: Account | null
  accountError: string | null
  linkedSessions: LinkedSession[] | null
  computers: ComputerInfo[]
  hosts: HostView[]
  /** The relay connection (the worst of them, when computers use several relays). */
  relay: RelayStatus | null
  viewed: Record<string, number>
  push: PushState
}

/** What the store needs from the platform; the app passes Expo's, tests their own. */
export interface Platform {
  storage: KeyValueStore
  os: string
  deviceName: string
  appVersion: string
  /** Development builds pair with relays on the developer's machine. */
  devBuild: boolean
  /** Asks for permission and returns this phone's Expo push token. */
  pushToken?: () => Promise<{ token: string } | { unsupported: string } | { denied: true }>
  fetch?: ConstructorParameters<typeof Gateway>[0]['fetch']
  connect?: SocketFactory
}

interface SavedSession {
  gateway: string
  token: string
  expiresAt: string | null
}

interface Relay {
  link: RelayLink
  phone: PhoneRelay
  stop: () => void
}

export class NeedsSignIn extends Error {
  constructor(readonly gateway: string) {
    super('Sign in to Zero-X first.')
    this.name = 'NeedsSignIn'
  }
}

const RANK: Record<LinkStatus, number> = { failed: 0, offline: 1, waiting: 2, connecting: 3, idle: 4, open: 5 }

export class AppStore {
  private state: AppState = {
    loaded: false,
    settings: DEFAULT_SETTINGS,
    signedIn: false,
    gateway: null,
    account: null,
    accountError: null,
    linkedSessions: null,
    computers: [],
    hosts: [],
    relay: null,
    viewed: {},
    push: { status: 'off' },
  }
  private listeners = new Set<() => void>()
  private session: SavedSession | null = null
  private deviceId = ''
  private secrets = new Map<string, PairedComputer>()
  private relays = new Map<string, Relay>()
  private viewedTimer: ReturnType<typeof setTimeout> | null = null

  constructor(private readonly platform: Platform) {}

  getState = (): AppState => this.state

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private set(patch: Partial<AppState>): void {
    this.state = { ...this.state, ...patch }
    for (const listener of [...this.listeners]) listener()
  }

  private gatewayClient(base: string): Gateway {
    return new Gateway({
      base,
      fetch: this.platform.fetch,
      client: { id: 'neuron-mobile', version: this.platform.appVersion, deviceName: this.platform.deviceName, os: this.platform.os },
    })
  }

  private async readJson<T>(key: string): Promise<T | null> {
    try {
      const text = await this.platform.storage.get(key)
      return text ? (JSON.parse(text) as T) : null
    } catch {
      return null
    }
  }

  // ------------------------------------------------------------- loading

  async load(): Promise<void> {
    const { storage } = this.platform
    this.deviceId = (await storage.get(KEYS.device)) ?? ''
    if (!/^[A-Za-z0-9_-]{8,64}$/.test(this.deviceId)) {
      this.deviceId = randomId()
      await storage.set(KEYS.device, this.deviceId)
    }
    const settings = { ...DEFAULT_SETTINGS, ...((await this.readJson<Partial<Settings>>(KEYS.settings)) ?? {}) }
    this.session = await this.readJson<SavedSession>(KEYS.session)
    const ids = (await this.readJson<string[]>(KEYS.computers)) ?? []
    for (const hostId of ids) {
      const computer = await this.readJson<PairedComputer>(KEYS.computer(hostId))
      if (computer) this.secrets.set(hostId, computer)
    }
    const viewed = (await this.readJson<Record<string, number>>(KEYS.viewed)) ?? {}
    this.set({ loaded: true, settings, viewed, signedIn: !!this.session, gateway: this.session?.gateway ?? null, computers: this.computerInfos() })
    if (this.session) {
      this.startRelays()
      void this.refreshAccount()
      void this.syncPush()
    }
  }

  private computerInfos(): ComputerInfo[] {
    return [...this.secrets.values()]
      .map(({ hostId, name, relayUrl, pairedAt }) => ({ hostId, name, relayUrl, pairedAt }))
      .sort((a, b) => a.name.localeCompare(b.name))
  }

  private async saveComputers(): Promise<void> {
    await this.platform.storage.set(KEYS.computers, JSON.stringify([...this.secrets.keys()]))
    this.set({ computers: this.computerInfos() })
  }

  // ------------------------------------------------------------- sign-in

  /** The gateway to sign in to: the one the user is on, or Zero-X. */
  defaultGateway(): string {
    return this.session?.gateway ?? 'https://zero-x.live'
  }

  startSignIn(gateway: string): Promise<DeviceCode> {
    return this.gatewayClient(gateway).requestDeviceCode()
  }

  /** Waits for the browser approval, then signs in and connects. */
  async completeSignIn(gateway: string, code: DeviceCode, signal?: { aborted: boolean }): Promise<void> {
    const client = this.gatewayClient(gateway)
    const signedIn = await client.waitForApproval(code, { signal })
    this.session = { gateway: client.base, token: signedIn.token, expiresAt: signedIn.expiresAt }
    await this.platform.storage.set(KEYS.session, JSON.stringify(this.session))
    this.set({ signedIn: true, gateway: client.base, accountError: null })
    // A failed relay (an expired session) starts over with the new token.
    for (const relay of this.relays.values()) relay.link.start()
    this.startRelays()
    await this.refreshAccount()
    void this.syncPush()
  }

  async refreshAccount(): Promise<void> {
    const session = this.session
    if (!session) return
    const client = this.gatewayClient(session.gateway)
    try {
      const [account, linkedSessions] = await Promise.all([client.account(session.token), client.linkedSessions(session.token).catch(() => null)])
      this.set({ account, linkedSessions, accountError: null })
    } catch (error) {
      if (error instanceof GatewayError && error.status === 401) {
        this.set({ accountError: 'Your Zero-X sign-in expired. Sign in again.' })
      } else {
        this.set({ accountError: error instanceof Error ? error.message : String(error) })
      }
    }
  }

  async revokeLinkedSession(id: string): Promise<void> {
    if (!this.session) return
    await this.gatewayClient(this.session.gateway).revokeSession(this.session.token, id)
    await this.refreshAccount()
  }

  /** Signs out: forgets the session, the paired computers (their keys) and the push registration. */
  async signOut(): Promise<void> {
    const session = this.session
    for (const computer of this.secrets.values()) this.relays.get(computer.relayUrl)?.phone.unpair(computer.hostId)
    if (session) {
      const client = this.gatewayClient(session.gateway)
      await client.unregisterPush(session.token, this.deviceId).catch(() => {})
      await client.signOut(session.token)
    }
    for (const relay of this.relays.values()) relay.stop()
    this.relays.clear()
    const { storage } = this.platform
    for (const hostId of this.secrets.keys()) await storage.delete(KEYS.computer(hostId))
    this.secrets.clear()
    await storage.delete(KEYS.computers)
    await storage.delete(KEYS.session)
    await storage.delete(KEYS.viewed)
    this.session = null
    this.set({ signedIn: false, gateway: null, account: null, linkedSessions: null, computers: [], hosts: [], relay: null, viewed: {}, push: { status: 'off' }, accountError: null })
  }

  // --------------------------------------------------------------- relays

  private relayFor(relayUrl: string): Relay {
    let relay = this.relays.get(relayUrl)
    if (relay) return relay
    const link = new RelayLink({
      url: relayUrl,
      role: 'phone',
      device: { id: this.deviceId, name: this.platform.deviceName, platform: this.platform.os },
      token: () => this.session?.token ?? null,
      connect: this.platform.connect,
    })
    const phone = new PhoneRelay(link, (hostId) => this.secrets.get(hostId))
    const unsubscribers = [phone.onChange(() => this.refreshHosts()), link.onStatus(() => this.refreshRelayStatus())]
    relay = {
      link,
      phone,
      stop: () => {
        for (const unsubscribe of unsubscribers) unsubscribe()
        link.stop()
      },
    }
    this.relays.set(relayUrl, relay)
    link.start()
    return relay
  }

  private relayOf(hostId: string): Relay | null {
    const computer = this.secrets.get(hostId)
    return computer ? this.relayFor(computer.relayUrl) : null
  }

  private startRelays(): void {
    for (const computer of this.secrets.values()) this.relayFor(computer.relayUrl)
  }

  private refreshHosts(): void {
    const hosts: HostView[] = []
    for (const relay of this.relays.values()) for (const host of relay.phone.hosts.values()) if (this.secrets.has(host.hostId)) hosts.push(host)
    // Paired computers the relay has not listed (yet) show as offline.
    for (const computer of this.secrets.values()) {
      if (!hosts.some((host) => host.hostId === computer.hostId)) hosts.push({ hostId: computer.hostId, name: computer.name, online: false, shares: [] })
    }
    this.set({ hosts })
  }

  private refreshRelayStatus(): void {
    let worst: RelayStatus | null = null
    for (const relay of this.relays.values()) {
      const { status, delayMs, lastError } = relay.link.state
      if (!worst || RANK[status] < RANK[worst.status]) worst = { status, delayMs, error: lastError }
    }
    this.set({ relay: worst })
  }

  /** The app came back to the foreground, or the network changed. */
  wake(online?: boolean): void {
    for (const relay of this.relays.values()) {
      if (online !== undefined) relay.link.setOnline(online)
      relay.link.foreground()
    }
  }

  /** Pull to refresh: the computers and sessions again. */
  refresh(): void {
    for (const relay of this.relays.values()) {
      if (relay.link.ready) relay.phone.refresh()
      else relay.link.foreground()
    }
  }

  /** Retries a relay that stopped (after the user fixed what stopped it). */
  retryRelays(): void {
    for (const relay of this.relays.values()) relay.link.start()
  }

  // -------------------------------------------------------------- pairing

  /**
   * Pairs with the computer in a scanned or pasted pairing link. Throws
   * {@link NeedsSignIn} when the phone must sign in first (the pairing screen
   * signs in, then calls this again).
   */
  async pair(linkText: string, timeoutMs = 20_000): Promise<ComputerInfo> {
    const payload = parsePairingLink(linkText, { trustedRelayHosts: this.platform.devBuild ? null : TRUSTED_RELAY_HOSTS })
    const gateway = gatewayForRelay(payload.relayUrl)
    if (!this.session) throw new NeedsSignIn(gateway)
    if (new URL(this.session.gateway).host !== new URL(gateway).host) {
      throw new PairingError('untrusted_relay', `This computer uses another Zero-X server (${new URL(gateway).host}). Sign out first to pair with it.`)
    }
    const relay = this.relayFor(payload.relayUrl)
    await this.waitForRelay(relay.link, timeoutMs)
    const computer = await relay.phone.pair(payload, this.platform.deviceName, timeoutMs)
    this.secrets.set(computer.hostId, computer)
    await this.platform.storage.set(KEYS.computer(computer.hostId), JSON.stringify(computer))
    await this.saveComputers()
    relay.phone.refresh()
    this.refreshHosts()
    return { hostId: computer.hostId, name: computer.name, relayUrl: computer.relayUrl, pairedAt: computer.pairedAt }
  }

  private waitForRelay(link: RelayLink, timeoutMs: number): Promise<void> {
    if (link.ready) return Promise.resolve()
    link.foreground()
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => done(new Error('Could not reach the relay. Check your connection and try again.')), timeoutMs)
      const offReady = link.onReady(() => done())
      const offStatus = link.onStatus((state) => {
        if (state.status === 'failed') done(new Error(state.lastError ?? 'The relay refused this phone.'))
      })
      function done(error?: Error) {
        clearTimeout(timer)
        offReady()
        offStatus()
        if (error) reject(error)
        else resolve()
      }
    })
  }

  /** Forgets a computer here and on the relay; the computer hears it and forgets this phone. */
  async forget(hostId: string): Promise<void> {
    this.relayOf(hostId)?.phone.unpair(hostId)
    this.secrets.delete(hostId)
    await this.platform.storage.delete(KEYS.computer(hostId))
    await this.saveComputers()
    this.refreshHosts()
  }

  // ------------------------------------------------------------- sessions

  /** Attaches to a shared session. The caller starts and closes the controller. */
  openSession(hostId: string, shareId: string): { controller: SessionController; transport: RelayEngineTransport } {
    const relay = this.relayOf(hostId)
    if (!relay) throw new Error('This computer is not paired with this phone any more.')
    const transport = new RelayEngineTransport({ relay: relay.phone, hostId, shareId, client: { name: this.platform.deviceName, kind: 'mobile' } })
    return { controller: new SessionController(transport), transport }
  }

  computerName(hostId: string): string | null {
    return this.secrets.get(hostId)?.name ?? null
  }

  /** The user saw the session up to `seq`: its unread badge goes. */
  markViewed(hostId: string, shareId: string, seq: number): void {
    const key = shareKey(hostId, shareId)
    if ((this.state.viewed[key] ?? -1) >= seq) return
    this.set({ viewed: { ...this.state.viewed, [key]: seq } })
    // A streaming turn moves this many times a second: write it down once things settle.
    if (this.viewedTimer) clearTimeout(this.viewedTimer)
    this.viewedTimer = setTimeout(() => {
      this.viewedTimer = null
      void this.platform.storage.set(KEYS.viewed, JSON.stringify(this.state.viewed))
    }, 1_000)
  }

  // ------------------------------------------------------- settings, push

  async updateSettings(patch: Partial<Settings>): Promise<void> {
    const settings = { ...this.state.settings, ...patch }
    this.set({ settings })
    await this.platform.storage.set(KEYS.settings, JSON.stringify(settings))
    if ('notifyNeedsMe' in patch || 'notifyFinished' in patch) await this.syncPush()
  }

  /** Registers this phone for push with the switches as they are, or unregisters it when both are off. */
  async syncPush(): Promise<void> {
    const session = this.session
    if (!session) return
    const { notifyNeedsMe, notifyFinished } = this.state.settings
    const client = this.gatewayClient(session.gateway)
    if (!notifyNeedsMe && !notifyFinished) {
      await client.unregisterPush(session.token, this.deviceId).catch(() => {})
      return this.set({ push: { status: 'off' } })
    }
    if (!this.platform.pushToken) return this.set({ push: { status: 'unsupported', reason: 'Notifications are not available here.' } })
    try {
      const result = await this.platform.pushToken()
      if ('denied' in result) return this.set({ push: { status: 'denied' } })
      if ('unsupported' in result) return this.set({ push: { status: 'unsupported', reason: result.unsupported } })
      await client.registerPush(session.token, { deviceId: this.deviceId, expoPushToken: result.token, platform: this.platform.os, needsMe: notifyNeedsMe, finished: notifyFinished })
      this.set({ push: { status: 'registered' } })
    } catch (error) {
      this.set({ push: { status: 'error', reason: error instanceof Error ? error.message : String(error) } })
    }
  }

  stop(): void {
    for (const relay of this.relays.values()) relay.stop()
    this.relays.clear()
  }
}
