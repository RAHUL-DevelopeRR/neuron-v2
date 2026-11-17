/**
 * Whether a newer Neuron release exists, and the exact command that installs
 * it, for the "Update available! Run: ..." notice in the prompt status bar and
 * for `/update`.
 *
 * Once per launch, after the UI is drawn, the UI reads the public release
 * manifest (https://zero-x.live/releases/cli/latest.json): a plain GET with a
 * 3 s timeout, no auth, no cookies and a fixed User-Agent that names no user,
 * machine or version. The manifest is kept in `update-check.json` in the
 * config dir for 24 hours; the verdict is worked out again on every launch, so
 * an upgrade clears the notice at once. Every failure is swallowed: no crash,
 * no line in the chat.
 *
 * No check runs for a development build (a version that is not semver, or the
 * package's `0.0.0`), with `CI` set (to anything but `false` or `0`), with `NEURON_NO_UPDATE_CHECK=1`, or when
 * stdout is not a terminal. `NEURON_UPDATE_MANIFEST_URL` points it at another
 * manifest (https, or http on loopback only) for tests.
 *
 * The parts are pure (environment, paths and fetch are passed in) so they are
 * tested without a network or this machine's layout.
 */

import fs from 'fs'
import os from 'os'
import path from 'path'

import { getConfigDir } from './config-dir'
import { isOpenableWebUrl } from './open-url'
import { writeFileAtomic } from './write-file-atomic'

export const DEFAULT_MANIFEST_URL = 'https://zero-x.live/releases/cli/latest.json'
export const UPDATE_CHECK_FILE = 'update-check.json'
export const CACHE_TTL_MS = 24 * 60 * 60 * 1000
export const FETCH_TIMEOUT_MS = 3000
/** Names the client kind only: no user, machine or version. */
export const UPDATE_CHECK_USER_AGENT = 'neuron-update-check'
/** A manifest is a few hundred bytes; anything this size is not one. */
const MAX_MANIFEST_BYTES = 256 * 1024

export type Env = Record<string, string | undefined>

// ---------------------------------------------------------------- versions

export interface Semver {
  major: number
  minor: number
  patch: number
  /** Prerelease identifiers: numbers where they are numeric. Empty for a release. */
  prerelease: Array<string | number>
}

const SEMVER =
  /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/

/** `7.0.0-alpha.10` (or with a leading `v`); build metadata is dropped. `null` when it is not semver. */
export function parseSemver(text: unknown): Semver | null {
  if (typeof text !== 'string' || text.length > 256) return null
  const match = SEMVER.exec(text.trim())
  if (!match) return null
  const numbers = [match[1], match[2], match[3]].map(Number)
  if (numbers.some((value) => !Number.isSafeInteger(value))) return null
  const prerelease: Array<string | number> = []
  if (match[4]) {
    for (const part of match[4].split('.')) {
      if (/^\d+$/.test(part)) {
        // A numeric identifier has no leading zero (semver 2.0.0, item 9).
        if (part.length > 1 && part.startsWith('0')) return null
        const value = Number(part)
        if (!Number.isSafeInteger(value)) return null
        prerelease.push(value)
      } else {
        prerelease.push(part)
      }
    }
  }
  return { major: numbers[0]!, minor: numbers[1]!, patch: numbers[2]!, prerelease }
}

function compareIdentifiers(a: string | number, b: string | number): number {
  if (typeof a === 'number' && typeof b === 'number') return Math.sign(a - b)
  // Numeric identifiers sort before alphanumeric ones.
  if (typeof a === 'number') return -1
  if (typeof b === 'number') return 1
  return a < b ? -1 : a > b ? 1 : 0
}

/**
 * -1, 0 or 1 by semver precedence (`7.0.0-alpha.9 < 7.0.0-alpha.10 <
 * 7.0.0-beta.1 < 7.0.0`, build metadata ignored); `null` when either side is
 * not semver.
 */
export function compareSemver(a: string, b: string): -1 | 0 | 1 | null {
  const left = parseSemver(a)
  const right = parseSemver(b)
  if (!left || !right) return null
  for (const key of ['major', 'minor', 'patch'] as const) {
    if (left[key] !== right[key]) return left[key] < right[key] ? -1 : 1
  }
  const lp = left.prerelease
  const rp = right.prerelease
  // A release outranks its prereleases.
  if (lp.length === 0 || rp.length === 0) {
    if (lp.length === rp.length) return 0
    return lp.length === 0 ? 1 : -1
  }
  for (let i = 0; i < Math.max(lp.length, rp.length); i++) {
    if (i >= lp.length) return -1
    if (i >= rp.length) return 1
    const order = compareIdentifiers(lp[i]!, rp[i]!)
    if (order !== 0) return order < 0 ? -1 : 1
  }
  return 0
}

// --------------------------------------------------------- install methods

export type InstallMethod = 'npm' | 'pip' | 'powershell' | 'shell'
export const INSTALL_METHODS: readonly InstallMethod[] = ['npm', 'pip', 'powershell', 'shell']

/**
 * The command for each install method when the manifest names none. The pip
 * one is for Windows; {@link defaultInstallCommand} names `python3` elsewhere,
 * where a bare `python` is often missing (stock macOS, Ubuntu). `--upgrade`
 * matters: without it pip leaves an installed `neuroncli` as it is.
 */
export const DEFAULT_INSTALL_COMMANDS: Readonly<Record<InstallMethod, string>> = {
  npm: 'npm install -g @zero-x.live/neuron',
  pip: 'python -m pip install --upgrade --pre neuroncli',
  powershell: 'irm https://zero-x.live/install.ps1 | iex',
  shell: 'curl -fsSL https://zero-x.live/install.sh | sh',
}

/** The built-in command for `method` on `platform`. */
export function defaultInstallCommand(method: InstallMethod, platform: NodeJS.Platform): string {
  if (method === 'pip' && platform !== 'win32') return 'python3 -m pip install --upgrade --pre neuroncli'
  return DEFAULT_INSTALL_COMMANDS[method]
}

export interface DetectInput {
  /** The UI binary's own path (`process.execPath`). */
  execPath: string
  platform: NodeJS.Platform
  env: Env
  /** The user's home folder (`os.homedir()`). */
  home: string
}

/** Forward slashes, no doubled or trailing ones; lower case on Windows. */
function normalizePath(value: string, platform: NodeJS.Platform): string {
  const forward = value.replace(/\\/g, '/').replace(/\/{2,}/g, '/').replace(/\/+$/, '')
  return platform === 'win32' ? forward.toLowerCase() : forward
}

function isInside(file: string, dir: string | undefined | null, platform: NodeJS.Platform): boolean {
  if (!dir || !dir.trim()) return false
  const folder = normalizePath(dir.trim(), platform)
  if (!folder) return false
  return file.startsWith(`${folder}/`)
}

function join(...parts: string[]): string {
  return parts.join('/')
}

/**
 * How this copy of Neuron was installed. The npm and pip launchers say so in
 * `NEURON_INSTALL_METHOD` (both share one cache layout, so the path cannot
 * tell them apart); without it, from where the UI binary lives:
 *   - in `node_modules` under `@zero-x.live/neuron` (or its platform packages): npm;
 *   - beside the pip launcher, in a `neuron_cli` package folder: pip;
 *   - in the launchers' default cache, a `neuroncli` folder: pip (npm uses it
 *     only when its package folder is read-only, and then says so);
 *   - in `%LOCALAPPDATA%\Programs\Neuron\bin`: the PowerShell installer;
 *   - in `~/.local/bin`: the shell installer;
 *   - otherwise the platform's installer.
 * `NEURON_INSTALL_METHOD` (npm, pip, powershell or shell) overrides all of it.
 */
export function detectInstallMethod({ execPath, platform, env, home }: DetectInput): InstallMethod {
  const stated = env.NEURON_INSTALL_METHOD?.trim().toLowerCase()
  if (stated && (INSTALL_METHODS as readonly string[]).includes(stated)) {
    return stated as InstallMethod
  }
  const file = normalizePath(execPath, platform)
  const lower = file.toLowerCase()
  if (lower.includes('/node_modules/') && lower.includes('/@zero-x.live/neuron')) return 'npm'
  // The UI bundled in the pip package: site-packages/neuron_cli/neuron-tui[.exe].
  if (/\/neuron_cli\/neuron-tui(\.exe)?$/.test(file)) return 'pip'

  const localAppData =
    env.LOCALAPPDATA?.trim() || (platform === 'win32' && home ? join(home, 'AppData', 'Local') : '')
  const xdgCache = env.XDG_CACHE_HOME?.trim()
  const pipCaches = [
    localAppData ? join(localAppData, 'neuroncli') : null,
    home ? join(home, 'Library', 'Caches', 'neuroncli') : null,
    // A relative XDG_CACHE_HOME is invalid by the spec, and ignored.
    xdgCache && (xdgCache.startsWith('/') || /^[a-z]:[\\/]/i.test(xdgCache)) ? join(xdgCache, 'neuroncli') : null,
    home ? join(home, '.cache', 'neuroncli') : null,
  ]
  if (pipCaches.some((dir) => isInside(file, dir, platform))) return 'pip'

  if (localAppData && isInside(file, join(localAppData, 'Programs', 'Neuron', 'bin'), platform)) {
    return 'powershell'
  }
  if (home && isInside(file, join(home, '.local', 'bin'), platform)) return 'shell'
  return platform === 'win32' ? 'powershell' : 'shell'
}

/** Control characters (a terminal escape among them) never reach the screen or the clipboard. */
function cleanCommand(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const text = value.replace(/[\x00-\x1f\x7f-\x9f]/g, ' ').replace(/\s+/g, ' ').trim()
  return text.length > 0 && text.length <= 500 ? text : null
}

/** The manifest's command for `method` when it has one, else the built-in one for `platform`. */
export function installCommand(
  manifest: Pick<ReleaseManifest, 'install'> | null,
  method: InstallMethod,
  platform: NodeJS.Platform = process.platform,
): string {
  const fromManifest = cleanCommand(manifest?.install?.[method])
  if (fromManifest === null) return defaultInstallCommand(method, platform)
  return method === 'pip' ? withPipUpgrade(fromManifest) : fromManifest
}

/**
 * The manifest's pip line is the site's first-install command
 * (`python -m pip install --pre neuroncli`); run over an existing install, pip
 * only says "Requirement already satisfied". An update needs `--upgrade`, so it
 * is added after `install` when the line has neither `--upgrade` nor `-U`.
 */
export function withPipUpgrade(command: string): string {
  if (/(^|\s)(--upgrade|-U)(\s|$)/.test(command)) return command
  return command.replace(/(\bpip\s+install)(\s|$)/, '$1 --upgrade$2')
}

// --------------------------------------------------------------- manifest

export interface ReleaseManifest {
  version: string | null
  minimumVersion: string | null
  notesUrl: string | null
  install: Partial<Record<InstallMethod, string>>
}

/**
 * The manifest's fields this UI uses, or `null` for anything that is not a
 * schema 1 manifest of the CLI. A version that is not semver is no version.
 */
export function parseManifest(value: unknown): ReleaseManifest | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const raw = value as Record<string, unknown>
  if (raw.schema_version !== 1 || raw.product !== 'cli') return null
  if (raw.version !== null && raw.version !== undefined && !parseSemver(raw.version)) return null
  const version = typeof raw.version === 'string' ? raw.version.trim() : null
  const minimumVersion =
    typeof raw.minimum_version === 'string' && parseSemver(raw.minimum_version)
      ? raw.minimum_version.trim()
      : null
  const notesUrl = isOpenableWebUrl(raw.notes_url) ? raw.notes_url : null
  const install: Partial<Record<InstallMethod, string>> = {}
  if (raw.install && typeof raw.install === 'object' && !Array.isArray(raw.install)) {
    const commands = raw.install as Record<string, unknown>
    for (const method of INSTALL_METHODS) {
      const command = cleanCommand(commands[method])
      if (command) install[method] = command
    }
  }
  return { version, minimumVersion, notesUrl, install }
}

// ------------------------------------------------------------ skip rules

export interface SkipInput {
  version: string
  env: Env
  isTTY: boolean
}

/** Why no check runs, or `null` when one may. */
export function updateCheckSkipReason({ version, env, isTTY }: SkipInput): string | null {
  const optOut = env.NEURON_NO_UPDATE_CHECK?.trim().toLowerCase()
  if (optOut === '1' || optOut === 'true') return 'NEURON_NO_UPDATE_CHECK is set'
  // CI=false and CI=0 are how a shell says "not CI".
  const ci = env.CI?.trim().toLowerCase()
  if (ci && ci !== 'false' && ci !== '0') return 'CI is set'
  const parsed = parseSemver(version)
  if (!parsed || (parsed.major === 0 && parsed.minor === 0 && parsed.patch === 0)) {
    return 'this is a development build with no release version'
  }
  if (!isTTY) return 'output is not a terminal'
  return null
}

function isLoopback(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, '').toLowerCase()
  return host === 'localhost' || host === '::1' || /^127(\.\d{1,3}){3}$/.test(host)
}

/**
 * The manifest to read: `NEURON_UPDATE_MANIFEST_URL` when it is https (or http
 * on loopback). The `url` of a rejected override is what `/update` shows, so it
 * never carries the override's credentials, nor text that is not a URL.
 */
export function manifestUrl(env: Env): { url: string } | { url: string; error: string } {
  const override = env.NEURON_UPDATE_MANIFEST_URL?.trim()
  if (!override) return { url: DEFAULT_MANIFEST_URL }
  let parsed: URL
  try {
    parsed = new URL(override)
  } catch {
    return { url: '(not shown: not a URL)', error: 'NEURON_UPDATE_MANIFEST_URL is not a URL' }
  }
  if (parsed.username || parsed.password) {
    parsed.username = ''
    parsed.password = ''
    return { url: parsed.toString(), error: 'NEURON_UPDATE_MANIFEST_URL must not carry credentials' }
  }
  if (parsed.protocol === 'https:') return { url: parsed.toString() }
  if (parsed.protocol === 'http:' && isLoopback(parsed.hostname)) return { url: parsed.toString() }
  // Another scheme's URL may keep a secret where http keeps none (`user:s3cret@...`).
  const shown = parsed.protocol === 'http:' ? parsed.toString() : `(not shown: a ${parsed.protocol} URL)`
  return { url: shown, error: 'NEURON_UPDATE_MANIFEST_URL must be https (http only on loopback)' }
}

// ------------------------------------------------------------------ cache

interface CacheEntry {
  url: string
  fetchedAt: number
  manifest: ReleaseManifest
}

/** The cached manifest of `url` when it is younger than the TTL; `null` for anything else. */
export function readCachedManifest(
  file: string,
  url: string,
  now: number,
  ttlMs = CACHE_TTL_MS,
): ReleaseManifest | null {
  try {
    const entry = JSON.parse(fs.readFileSync(file, 'utf8')) as Partial<CacheEntry> | null
    if (!entry || typeof entry !== 'object' || entry.url !== url) return null
    const fetchedAt = entry.fetchedAt
    // A time in the future (a clock set back) is stale too.
    if (typeof fetchedAt !== 'number' || !Number.isFinite(fetchedAt) || fetchedAt > now) return null
    if (now - fetchedAt >= ttlMs) return null
    const manifest = entry.manifest
    if (!manifest || typeof manifest !== 'object') return null
    // Kept in the parsed shape; checked again as a schema 1 manifest.
    return parseManifest({
      schema_version: 1,
      product: 'cli',
      version: manifest.version,
      minimum_version: manifest.minimumVersion,
      notes_url: manifest.notesUrl,
      install: manifest.install,
    })
  } catch {
    return null
  }
}

/** Keeps the manifest for the TTL; a folder or file that cannot be written is ignored. */
export function writeCachedManifest(file: string, url: string, manifest: ReleaseManifest, now: number): void {
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true })
    const entry: CacheEntry = { url, fetchedAt: now, manifest }
    writeFileAtomic(file, `${JSON.stringify(entry)}\n`)
  } catch {
    // The next launch reads the manifest again.
  }
}

/** `update-check.json` in the config dir, or `null` when the config dir is unusable. */
export function defaultCacheFile(): string | null {
  try {
    return path.join(getConfigDir(), UPDATE_CHECK_FILE)
  } catch {
    return null
  }
}

// ------------------------------------------------------------------ fetch

export type FetchLike = (url: string, init: RequestInit) => Promise<Response>

/** The manifest at `url`, or an error: timeout, HTTP status, not JSON, not a manifest. */
export async function fetchManifest(
  url: string,
  { fetchImpl = fetch as FetchLike, timeoutMs = FETCH_TIMEOUT_MS }: { fetchImpl?: FetchLike; timeoutMs?: number } = {},
): Promise<ReleaseManifest> {
  const controller = new AbortController()
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      controller.abort()
      reject(new Error('timed out'))
    }, timeoutMs)
  })
  try {
    const read = (async () => {
      const response = await fetchImpl(url, {
        method: 'GET',
        headers: { 'User-Agent': UPDATE_CHECK_USER_AGENT, Accept: 'application/json' },
        credentials: 'omit',
        signal: controller.signal,
      })
      if (!response.ok) throw new Error(`the server answered ${response.status}`)
      const text = await response.text()
      if (text.length > MAX_MANIFEST_BYTES) throw new Error('the manifest is too large')
      let body: unknown
      try {
        body = JSON.parse(text)
      } catch {
        throw new Error('the manifest is not JSON')
      }
      const manifest = parseManifest(body)
      if (!manifest) throw new Error('the manifest is not one this version reads')
      return manifest
    })()
    // A fetch that ignores the abort still loses the race.
    read.catch(() => {})
    return await Promise.race([read, timeout])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

// ------------------------------------------------------------------ check

export type UpdateCheckResult =
  | { status: 'disabled'; current: string; reason: string; manifestUrl: string }
  | { status: 'failed'; current: string; reason: string; manifestUrl: string }
  | {
      status: 'up-to-date'
      current: string
      /** `null` when no release is published yet. */
      latest: string | null
      notesUrl: string | null
      manifestUrl: string
    }
  | {
      status: 'available'
      current: string
      latest: string
      minimum: string | null
      notesUrl: string | null
      command: string
      method: InstallMethod
      /** The installed version is older than the manifest's minimum. */
      required: boolean
      manifestUrl: string
    }

/** What the manifest means for the installed `current` version. */
export function evaluateManifest(
  current: string,
  manifest: ReleaseManifest,
  method: InstallMethod,
  url: string,
  platform: NodeJS.Platform = process.platform,
): UpdateCheckResult {
  const required =
    manifest.minimumVersion !== null && compareSemver(current, manifest.minimumVersion) === -1
  const newer = manifest.version !== null && compareSemver(manifest.version, current) === 1
  if (!newer && !required) {
    return { status: 'up-to-date', current, latest: manifest.version, notesUrl: manifest.notesUrl, manifestUrl: url }
  }
  return {
    status: 'available',
    current,
    // Not newer but required: a manifest without a version still names the minimum.
    latest: newer ? manifest.version! : manifest.minimumVersion!,
    minimum: manifest.minimumVersion,
    notesUrl: manifest.notesUrl,
    command: installCommand(manifest, method, platform),
    method,
    required,
    manifestUrl: url,
  }
}

export interface UpdateCheckOptions {
  version: string
  env: Env
  isTTY: boolean
  execPath: string
  platform: NodeJS.Platform
  home: string
  now?: number
  fetchImpl?: FetchLike
  timeoutMs?: number
  /** Where the manifest is cached; `null` for no cache. */
  cacheFile?: string | null
  /** Read the manifest even when the cached one is fresh (`/update`). */
  force?: boolean
}

/** The whole check. Never throws: every failure is a `failed` result. */
export async function runUpdateCheck(options: UpdateCheckOptions): Promise<UpdateCheckResult> {
  const current = options.version
  let url = DEFAULT_MANIFEST_URL
  try {
    const target = manifestUrl(options.env)
    url = target.url
    const skip = updateCheckSkipReason(options)
    if (skip) return { status: 'disabled', current, reason: skip, manifestUrl: url }
    if ('error' in target) return { status: 'disabled', current, reason: target.error, manifestUrl: url }

    const now = options.now ?? Date.now()
    const method = detectInstallMethod(options)
    const cacheFile = options.cacheFile === undefined ? defaultCacheFile() : options.cacheFile
    let manifest = !options.force && cacheFile ? readCachedManifest(cacheFile, url, now) : null
    if (!manifest) {
      manifest = await fetchManifest(url, { fetchImpl: options.fetchImpl, timeoutMs: options.timeoutMs })
      if (cacheFile) writeCachedManifest(cacheFile, url, manifest, now)
    }
    return evaluateManifest(current, manifest, method, url, options.platform)
  } catch (error) {
    const reason = error instanceof Error && error.message ? error.message : 'the request failed'
    return { status: 'failed', current, reason, manifestUrl: url }
  }
}

/** The inputs of a check in this process. */
export function processUpdateCheckOptions(version: string): UpdateCheckOptions {
  let home = ''
  try {
    home = os.homedir()
  } catch {
    // No home: only the env override and the platform default remain.
  }
  return {
    version,
    env: process.env,
    isTTY: process.stdout.isTTY === true,
    execPath: process.execPath,
    platform: process.platform,
    home,
  }
}

// ----------------------------------------------------------------- report

/** What `/update` says for a result. */
export function formatUpdateReport(result: UpdateCheckResult, copied = false): string {
  switch (result.status) {
    case 'available': {
      const lines = [
        result.required
          ? `Update required: Neuron ${result.current} is older than the minimum supported version, ${result.minimum}.`
          : `Update available: Neuron ${result.latest}.`,
        `Installed: ${result.current}`,
        `Latest: ${result.latest}`,
      ]
      if (result.notesUrl) lines.push(`Release notes: ${result.notesUrl}`)
      lines.push(`Update with: ${result.command}`)
      lines.push(
        copied
          ? 'The command is copied. Exit Neuron and run it in a terminal.'
          : 'Exit Neuron and run it in a terminal.',
      )
      return lines.join('\n')
    }
    case 'up-to-date':
      return result.latest
        ? `Neuron ${result.current} is up to date (latest release: ${result.latest}).`
        : `Neuron ${result.current} is up to date: no release is published yet.`
    case 'disabled':
      return [
        `The update check is off: ${result.reason}.`,
        `Installed: ${result.current}`,
        `Release manifest: ${result.manifestUrl}`,
      ].join('\n')
    case 'failed':
      return [
        `Could not check for updates: ${result.reason}.`,
        `Installed: ${result.current}`,
        `Release manifest: ${result.manifestUrl}`,
      ].join('\n')
  }
}
