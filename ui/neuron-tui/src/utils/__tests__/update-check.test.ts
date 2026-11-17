import fs from 'fs'
import os from 'os'
import path from 'path'

import { afterAll, afterEach, beforeAll, describe, expect, test } from 'bun:test'

import {
  CACHE_TTL_MS,
  DEFAULT_INSTALL_COMMANDS,
  DEFAULT_MANIFEST_URL,
  UPDATE_CHECK_USER_AGENT,
  compareSemver,
  defaultInstallCommand,
  detectInstallMethod,
  evaluateManifest,
  fetchManifest,
  formatUpdateReport,
  installCommand,
  manifestUrl,
  parseManifest,
  parseSemver,
  readCachedManifest,
  runUpdateCheck,
  updateCheckSkipReason,
  writeCachedManifest,
  type FetchLike,
  type UpdateCheckOptions,
} from '../update-check'
import {
  refreshUpdateCheck,
  resetUpdateCheckForTesting,
  updateNotice,
  useUpdateStore,
} from '../../state/update-store'

const MANIFEST = {
  schema_version: 1,
  product: 'cli',
  version: '7.0.0-alpha.6',
  released_at: '2026-11-16T10:00:00Z',
  notes_url: 'https://zero-x.live/changelog#7.0.0-alpha.6',
  minimum_version: null,
  downloads: [],
  install: {
    npm: 'npm install -g @zero-x.live/neuron',
    pip: 'python -m pip install --pre neuroncli',
    powershell: 'irm https://zero-x.live/install.ps1 | iex',
    shell: 'curl -fsSL https://zero-x.live/install.sh | sh',
  },
}

const jsonFetch =
  (body: unknown, status = 200, seen: Array<{ url: string; init: RequestInit }> = []): FetchLike =>
  async (url, init) => {
    seen.push({ url, init })
    return new Response(typeof body === 'string' ? body : JSON.stringify(body), { status })
  }

let tmp: string
beforeAll(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-update-check-'))
})
afterAll(() => {
  fs.rmSync(tmp, { recursive: true, force: true })
})

const options = (overrides: Partial<UpdateCheckOptions> = {}): UpdateCheckOptions => ({
  version: '7.0.0-alpha.5',
  env: {},
  isTTY: true,
  execPath: '/home/dev/.local/bin/neuron-tui',
  platform: 'linux',
  home: '/home/dev',
  cacheFile: null,
  fetchImpl: jsonFetch(MANIFEST),
  ...overrides,
})

describe('versions', () => {
  test('prereleases order by their identifiers, below the release', () => {
    const ordered = [
      '6.9.9',
      '7.0.0-alpha',
      '7.0.0-alpha.1',
      '7.0.0-alpha.9',
      '7.0.0-alpha.10',
      '7.0.0-alpha.beta',
      '7.0.0-beta.1',
      '7.0.0-beta.2',
      '7.0.0-rc.1',
      '7.0.0',
      '7.0.1',
      '7.1.0',
      '10.0.0',
    ]
    for (let i = 0; i < ordered.length; i++) {
      for (let j = 0; j < ordered.length; j++) {
        expect(compareSemver(ordered[i]!, ordered[j]!)).toBe(Math.sign(i - j) as -1 | 0 | 1)
      }
    }
  })

  test('equal versions, build metadata and a leading v', () => {
    expect(compareSemver('7.0.0-alpha.5', '7.0.0-alpha.5')).toBe(0)
    expect(compareSemver('7.0.0+build.1', '7.0.0+build.2')).toBe(0)
    expect(compareSemver('v7.0.0-alpha.5', '7.0.0-alpha.5')).toBe(0)
    expect(parseSemver('7.0.0-alpha.10+sha.abc')).toEqual({
      major: 7,
      minor: 0,
      patch: 0,
      prerelease: ['alpha', 10],
    })
  })

  test('malformed versions compare as null', () => {
    for (const bad of ['', 'dev', '7', '7.0', '7.0.0.0', '07.0.0', '7.0.0-', '7.0.0-alpha..1', '7.0.0-01', 'latest']) {
      expect(parseSemver(bad)).toBeNull()
      expect(compareSemver(bad, '7.0.0')).toBeNull()
      expect(compareSemver('7.0.0', bad)).toBeNull()
    }
    expect(parseSemver(null)).toBeNull()
    expect(parseSemver(7)).toBeNull()
  })
})

describe('install method', () => {
  const win = { platform: 'win32' as const, home: 'C:\\Users\\dev', env: { LOCALAPPDATA: 'C:\\Users\\dev\\AppData\\Local' } }
  const linux = { platform: 'linux' as const, home: '/home/dev', env: {} }
  const mac = { platform: 'darwin' as const, home: '/Users/dev', env: {} }

  test('npm: under node_modules/@zero-x.live/neuron, platform packages included', () => {
    expect(
      detectInstallMethod({
        ...win,
        execPath: 'C:\\Users\\dev\\AppData\\Roaming\\npm\\node_modules\\@zero-x.live\\neuron\\bin\\neuron-tui.exe',
      }),
    ).toBe('npm')
    expect(
      detectInstallMethod({
        ...linux,
        execPath: '/usr/lib/node_modules/@zero-x.live/neuron-linux-x64/bin/neuron-tui',
      }),
    ).toBe('npm')
    expect(
      detectInstallMethod({ ...mac, execPath: '/opt/homebrew/lib/node_modules/@zero-x.live/neuron/bin/neuron-tui' }),
    ).toBe('npm')
    // Another package's node_modules is not Neuron's npm install.
    expect(detectInstallMethod({ ...linux, execPath: '/srv/app/node_modules/other/neuron-tui' })).toBe('shell')
  })

  test('pip: inside the launcher cache on each platform', () => {
    expect(
      detectInstallMethod({ ...win, execPath: 'C:\\Users\\dev\\AppData\\Local\\neuroncli\\7.0.0\\neuron-tui.exe' }),
    ).toBe('pip')
    // Windows paths match whatever their case.
    expect(
      detectInstallMethod({ ...win, execPath: 'c:\\users\\DEV\\appdata\\local\\NeuronCLI\\bin\\neuron-tui.exe' }),
    ).toBe('pip')
    // Without LOCALAPPDATA, the default under the home folder.
    expect(
      detectInstallMethod({
        ...win,
        env: {},
        execPath: 'C:\\Users\\dev\\AppData\\Local\\neuroncli\\neuron-tui.exe',
      }),
    ).toBe('pip')
    expect(detectInstallMethod({ ...mac, execPath: '/Users/dev/Library/Caches/neuroncli/bin/neuron-tui' })).toBe('pip')
    expect(detectInstallMethod({ ...linux, execPath: '/home/dev/.cache/neuroncli/bin/neuron-tui' })).toBe('pip')
    expect(
      detectInstallMethod({
        ...linux,
        env: { XDG_CACHE_HOME: '/var/cache/dev' },
        execPath: '/var/cache/dev/neuroncli/bin/neuron-tui',
      }),
    ).toBe('pip')
    // A sibling folder whose name only starts the same is not inside.
    expect(detectInstallMethod({ ...linux, execPath: '/home/dev/.cache/neuroncli-old/neuron-tui' })).toBe('shell')
    // Unix paths are case-sensitive.
    expect(detectInstallMethod({ ...linux, execPath: '/home/dev/.Cache/neuroncli/neuron-tui' })).toBe('shell')
  })

  test('powershell: the installer folder; shell: ~/.local/bin', () => {
    expect(
      detectInstallMethod({
        ...win,
        execPath: 'C:\\Users\\dev\\AppData\\Local\\Programs\\Neuron\\bin\\neuron-tui.exe',
      }),
    ).toBe('powershell')
    expect(detectInstallMethod({ ...linux, execPath: '/home/dev/.local/bin/neuron-tui' })).toBe('shell')
    expect(detectInstallMethod({ ...mac, execPath: '/Users/dev/.local/bin/neuron-tui' })).toBe('shell')
  })

  test('anything else: the platform installer', () => {
    expect(detectInstallMethod({ ...win, execPath: 'D:\\tools\\neuron-tui.exe' })).toBe('powershell')
    expect(detectInstallMethod({ ...linux, execPath: '/opt/neuron/neuron-tui' })).toBe('shell')
    expect(detectInstallMethod({ ...mac, execPath: '/usr/local/bin/neuron-tui' })).toBe('shell')
  })

  test('NEURON_INSTALL_METHOD overrides the path; an unknown value is ignored', () => {
    const npmPath = '/usr/lib/node_modules/@zero-x.live/neuron/bin/neuron-tui'
    for (const method of ['npm', 'pip', 'powershell', 'shell'] as const) {
      expect(detectInstallMethod({ ...linux, env: { NEURON_INSTALL_METHOD: method }, execPath: '/opt/x' })).toBe(method)
    }
    expect(detectInstallMethod({ ...linux, env: { NEURON_INSTALL_METHOD: ' PIP ' }, execPath: npmPath })).toBe('pip')
    expect(detectInstallMethod({ ...linux, env: { NEURON_INSTALL_METHOD: 'brew' }, execPath: npmPath })).toBe('npm')
  })

  test("npm's cache fallback is npm when the npm launcher says so", () => {
    // A read-only npm package folder puts the UI in the cache the pip launcher uses too.
    const linuxCache = '/home/dev/.cache/neuroncli/7.0.0-alpha.6/neuron-tui-linux-x64'
    expect(detectInstallMethod({ ...linux, env: { NEURON_INSTALL_METHOD: 'npm' }, execPath: linuxCache })).toBe('npm')
    const winCache = 'C:\\Users\\dev\\AppData\\Local\\neuroncli\\7.0.0-alpha.6\\neuron-tui-win32-x64.exe'
    expect(
      detectInstallMethod({ ...win, env: { ...win.env, NEURON_INSTALL_METHOD: 'npm' }, execPath: winCache }),
    ).toBe('npm')
    // An older launcher that says nothing: the shared cache reads as pip.
    expect(detectInstallMethod({ ...linux, execPath: linuxCache })).toBe('pip')
  })

  test('pip with NEURON_CACHE_DIR, and the UI bundled in the pip package', () => {
    const custom = '/data/neuron-cache/7.0.0-alpha.6/neuron-tui-linux-x64'
    expect(
      detectInstallMethod({
        ...linux,
        env: { NEURON_CACHE_DIR: '/data/neuron-cache', NEURON_INSTALL_METHOD: 'pip' },
        execPath: custom,
      }),
    ).toBe('pip')
    // Bundled in site-packages/neuron_cli: pip, said or not.
    expect(
      detectInstallMethod({ ...linux, execPath: '/home/dev/.venv/lib/python3.12/site-packages/neuron_cli/neuron-tui' }),
    ).toBe('pip')
    expect(
      detectInstallMethod({ ...win, execPath: 'C:\\Python312\\Lib\\site-packages\\Neuron_CLI\\neuron-tui.exe' }),
    ).toBe('pip')
    expect(
      detectInstallMethod({
        ...mac,
        env: { NEURON_INSTALL_METHOD: 'pip' },
        execPath: '/Users/dev/Library/Python/3.12/lib/python/site-packages/neuron_cli/neuron-tui',
      }),
    ).toBe('pip')
    // A file that only ends the same is not the bundled UI.
    expect(detectInstallMethod({ ...linux, execPath: '/opt/neuron_cli/neuron-tui-old' })).toBe('shell')
  })
})

describe('the command', () => {
  test("the manifest's command for the method, else the built-in one", () => {
    const manifest = parseManifest({ ...MANIFEST, install: { npm: 'npm i -g @zero-x.live/neuron@next', pip: '' } })!
    expect(installCommand(manifest, 'npm')).toBe('npm i -g @zero-x.live/neuron@next')
    expect(installCommand(manifest, 'pip', 'win32')).toBe(DEFAULT_INSTALL_COMMANDS.pip)
    expect(installCommand(manifest, 'pip', 'linux')).toBe('python3 -m pip install --upgrade --pre neuroncli')
    expect(installCommand(manifest, 'shell')).toBe(DEFAULT_INSTALL_COMMANDS.shell)
    expect(installCommand(null, 'powershell')).toBe('irm https://zero-x.live/install.ps1 | iex')
    const { pip: _pip, ...others } = MANIFEST.install
    expect({ ...DEFAULT_INSTALL_COMMANDS, pip: undefined }).toEqual({ ...others, pip: undefined })
  })

  test('the built-in pip command upgrades, with the interpreter each platform has', () => {
    expect(defaultInstallCommand('pip', 'win32')).toBe('python -m pip install --upgrade --pre neuroncli')
    for (const platform of ['linux', 'darwin'] as const) {
      expect(defaultInstallCommand('pip', platform)).toBe('python3 -m pip install --upgrade --pre neuroncli')
    }
    for (const method of ['npm', 'powershell', 'shell'] as const) {
      expect(defaultInstallCommand(method, 'linux')).toBe(DEFAULT_INSTALL_COMMANDS[method])
    }
  })

  test('a check names the pip command for its own platform', async () => {
    const { pip: _pip, ...install } = MANIFEST.install
    const result = await runUpdateCheck(
      options({ env: { NEURON_INSTALL_METHOD: 'pip' }, fetchImpl: jsonFetch({ ...MANIFEST, install }) }),
    )
    expect(result).toMatchObject({ status: 'available', method: 'pip', command: 'python3 -m pip install --upgrade --pre neuroncli' })
    const onWindows = await runUpdateCheck(
      options({
        env: { NEURON_INSTALL_METHOD: 'pip' },
        platform: 'win32',
        execPath: 'C:\\tools\\neuron-tui.exe',
        home: 'C:\\Users\\dev',
        fetchImpl: jsonFetch({ ...MANIFEST, install }),
      }),
    )
    expect(onWindows).toMatchObject({ status: 'available', command: 'python -m pip install --upgrade --pre neuroncli' })
  })

  test('control characters never reach the terminal', () => {
    const manifest = parseManifest({ ...MANIFEST, install: { shell: 'curl x | sh\u001b]52;c;AAAA\u0007', npm: 42 } })!
    expect(installCommand(manifest, 'shell')).toBe('curl x | sh ]52;c;AAAA')
    expect(installCommand(manifest, 'npm')).toBe(DEFAULT_INSTALL_COMMANDS.npm)
  })
})

describe('the manifest', () => {
  test('only schema 1 manifests of the CLI', () => {
    expect(parseManifest(MANIFEST)).toMatchObject({ version: '7.0.0-alpha.6', minimumVersion: null })
    expect(parseManifest({ ...MANIFEST, schema_version: 2 })).toBeNull()
    expect(parseManifest({ ...MANIFEST, schema_version: '1' })).toBeNull()
    expect(parseManifest({ ...MANIFEST, product: 'desktop' })).toBeNull()
    expect(parseManifest({ ...MANIFEST, version: 'latest' })).toBeNull()
    expect(parseManifest([])).toBeNull()
    expect(parseManifest(null)).toBeNull()
    expect(parseManifest('{}')).toBeNull()
  })

  test('no version yet, a bad minimum and a link that is not a web link', () => {
    const parsed = parseManifest({ ...MANIFEST, version: null, minimum_version: 'soon', notes_url: 'file:///etc/passwd' })!
    expect(parsed).toEqual({ version: null, minimumVersion: null, notesUrl: null, install: parsed.install })
  })

  test('newer, equal, older and below the minimum', () => {
    const at = (version: string | null, minimum: string | null = null) =>
      parseManifest({ ...MANIFEST, version, minimum_version: minimum })!
    expect(evaluateManifest('7.0.0-alpha.5', at('7.0.0-alpha.6'), 'npm', DEFAULT_MANIFEST_URL)).toMatchObject({
      status: 'available',
      latest: '7.0.0-alpha.6',
      command: 'npm install -g @zero-x.live/neuron',
      required: false,
    })
    expect(evaluateManifest('7.0.0-alpha.6', at('7.0.0-alpha.6'), 'npm', DEFAULT_MANIFEST_URL).status).toBe('up-to-date')
    // A local build newer than the release is not told to go back.
    expect(evaluateManifest('7.0.0', at('7.0.0-alpha.6'), 'npm', DEFAULT_MANIFEST_URL).status).toBe('up-to-date')
    expect(evaluateManifest('7.0.0-alpha.5', at(null), 'npm', DEFAULT_MANIFEST_URL)).toMatchObject({
      status: 'up-to-date',
      latest: null,
    })
    expect(
      evaluateManifest('7.0.0-alpha.2', at('7.0.0-alpha.6', '7.0.0-alpha.4'), 'shell', DEFAULT_MANIFEST_URL),
    ).toMatchObject({ status: 'available', required: true, minimum: '7.0.0-alpha.4' })
    expect(
      evaluateManifest('7.0.0-alpha.5', at('7.0.0-alpha.6', '7.0.0-alpha.4'), 'shell', DEFAULT_MANIFEST_URL),
    ).toMatchObject({ status: 'available', required: false })
  })
})

describe('skip rules', () => {
  const skip = (version: string, env: Record<string, string> = {}, isTTY = true) =>
    updateCheckSkipReason({ version, env, isTTY })

  test('a release in a terminal is checked', () => {
    expect(skip('7.0.0-alpha.5')).toBeNull()
  })

  test('a development build, CI, the opt-out and no terminal are not', () => {
    expect(skip('0.0.0')).toContain('development build')
    expect(skip('dev')).toContain('development build')
    expect(skip('')).toContain('development build')
    expect(skip('7.0.0-alpha.5', { CI: 'true' })).toBe('CI is set')
    expect(skip('7.0.0-alpha.5', { CI: '1' })).toBe('CI is set')
    expect(skip('7.0.0-alpha.5', { NEURON_NO_UPDATE_CHECK: '1' })).toBe('NEURON_NO_UPDATE_CHECK is set')
    expect(skip('7.0.0-alpha.5', {}, false)).toBe('output is not a terminal')
    // An empty CI or NEURON_NO_UPDATE_CHECK=0 does not turn it off.
    expect(skip('7.0.0-alpha.5', { CI: '', NEURON_NO_UPDATE_CHECK: '0' })).toBeNull()
  })

  test('CI=false and CI=0 are not CI', () => {
    for (const value of ['false', '0', ' FALSE ', 'False', '  ']) {
      expect(skip('7.0.0-alpha.5', { CI: value })).toBeNull()
    }
    for (const value of ['true', 'TRUE', '1', 'yes', 'github']) {
      expect(skip('7.0.0-alpha.5', { CI: value })).toBe('CI is set')
    }
  })

  test('a rejected manifest URL is never shown with its credentials', async () => {
    for (const bad of ['https://user:s3cret@example.test/m.json', 'https://:s3cret@example.test/m.json', 'http://user:s3cret@example.test/m']) {
      const target = manifestUrl({ NEURON_UPDATE_MANIFEST_URL: bad })
      expect('error' in target).toBe(true)
      expect(target.url).not.toContain('s3cret')
      expect(target.url).not.toContain('user')
      for (const env of [{ NEURON_UPDATE_MANIFEST_URL: bad }, { NEURON_UPDATE_MANIFEST_URL: bad, CI: 'true' }]) {
        const result = await runUpdateCheck(options({ env }))
        expect(result.status).toBe('disabled')
        expect(result.manifestUrl).not.toContain('s3cret')
        expect(formatUpdateReport(result)).not.toContain('s3cret')
      }
    }
    expect(manifestUrl({ NEURON_UPDATE_MANIFEST_URL: 'https://user:s3cret@example.test/m.json' }).url).toBe(
      'https://example.test/m.json',
    )
    // Text that is not a URL, or a URL of another scheme, is not echoed either.
    for (const odd of ['user:s3cret@ not a url', 'https://exa mple.test/s3cret', 'ftp://s3cret.test/m']) {
      const target = manifestUrl({ NEURON_UPDATE_MANIFEST_URL: odd })
      expect('error' in target).toBe(true)
      expect(target.url).not.toContain('s3cret')
    }
    // A plain http URL off loopback carries nothing secret and is shown.
    expect(manifestUrl({ NEURON_UPDATE_MANIFEST_URL: 'http://example.test/m.json' }).url).toBe('http://example.test/m.json')
  })

  test('the manifest URL override is https, or http on loopback only', () => {
    expect(manifestUrl({})).toEqual({ url: DEFAULT_MANIFEST_URL })
    expect(manifestUrl({ NEURON_UPDATE_MANIFEST_URL: 'https://example.test/m.json' })).toEqual({
      url: 'https://example.test/m.json',
    })
    for (const ok of ['http://127.0.0.1:8080/m.json', 'http://localhost:3/m', 'http://[::1]:9/m']) {
      expect('error' in manifestUrl({ NEURON_UPDATE_MANIFEST_URL: ok })).toBe(false)
    }
    for (const bad of ['http://example.test/m.json', 'ftp://127.0.0.1/m', 'not a url', 'https://u:p@example.test/m']) {
      expect('error' in manifestUrl({ NEURON_UPDATE_MANIFEST_URL: bad })).toBe(true)
    }
  })

  test('a skipped check makes no request', async () => {
    let calls = 0
    const counting: FetchLike = async () => {
      calls++
      return new Response(JSON.stringify(MANIFEST))
    }
    for (const overrides of [
      { version: '0.0.0' },
      { env: { CI: 'true' } },
      { env: { NEURON_NO_UPDATE_CHECK: '1' } },
      { isTTY: false },
      { env: { NEURON_UPDATE_MANIFEST_URL: 'http://example.test/m.json' } },
    ]) {
      const result = await runUpdateCheck(options({ ...overrides, fetchImpl: counting }))
      expect(result.status).toBe('disabled')
    }
    expect(calls).toBe(0)
  })
})

describe('the cache', () => {
  const file = () => path.join(tmp, `cache-${Math.random().toString(36).slice(2)}.json`)
  const manifest = parseManifest(MANIFEST)!
  const NOW = Date.parse('2026-11-17T12:00:00Z')

  test('fresh for 24 hours, for the same URL only', () => {
    const cache = file()
    writeCachedManifest(cache, DEFAULT_MANIFEST_URL, manifest, NOW)
    expect(readCachedManifest(cache, DEFAULT_MANIFEST_URL, NOW + 1000)).toEqual(manifest)
    expect(readCachedManifest(cache, DEFAULT_MANIFEST_URL, NOW + CACHE_TTL_MS - 1)).toEqual(manifest)
    expect(readCachedManifest(cache, DEFAULT_MANIFEST_URL, NOW + CACHE_TTL_MS)).toBeNull()
    expect(readCachedManifest(cache, 'https://example.test/other.json', NOW + 1000)).toBeNull()
    // Written in the future (a clock set back): stale.
    expect(readCachedManifest(cache, DEFAULT_MANIFEST_URL, NOW - 1000)).toBeNull()
  })

  test('a corrupt, missing or unwritable cache is ignored', async () => {
    const corrupt = file()
    fs.writeFileSync(corrupt, '{ not json')
    expect(readCachedManifest(corrupt, DEFAULT_MANIFEST_URL, NOW)).toBeNull()
    fs.writeFileSync(corrupt, JSON.stringify({ url: DEFAULT_MANIFEST_URL, fetchedAt: NOW, manifest: { version: 'x' } }))
    expect(readCachedManifest(corrupt, DEFAULT_MANIFEST_URL, NOW)).toBeNull()
    expect(readCachedManifest(path.join(tmp, 'missing.json'), DEFAULT_MANIFEST_URL, NOW)).toBeNull()

    // A regular file where the folder should be: the write fails quietly.
    const blocker = file()
    fs.writeFileSync(blocker, 'x')
    const unwritable = path.join(blocker, 'update-check.json')
    expect(() => writeCachedManifest(unwritable, DEFAULT_MANIFEST_URL, manifest, NOW)).not.toThrow()

    // And the check still answers from the network.
    const result = await runUpdateCheck(options({ cacheFile: unwritable }))
    expect(result.status).toBe('available')
    const fromCorrupt = await runUpdateCheck(options({ cacheFile: corrupt }))
    expect(fromCorrupt.status).toBe('available')
  })

  test('a fresh cache answers without a request; a new launch works the verdict out again', async () => {
    const cache = file()
    let calls = 0
    const counting: FetchLike = async () => {
      calls++
      return new Response(JSON.stringify(MANIFEST))
    }
    const first = await runUpdateCheck(options({ cacheFile: cache, fetchImpl: counting, now: NOW }))
    expect(first.status).toBe('available')
    // Upgraded since: the same cached manifest now says up to date.
    const upgraded = await runUpdateCheck(
      options({ cacheFile: cache, fetchImpl: counting, now: NOW + 60_000, version: '7.0.0-alpha.6' }),
    )
    expect(upgraded.status).toBe('up-to-date')
    expect(calls).toBe(1)
    // A day later it reads the manifest again; /update always does.
    await runUpdateCheck(options({ cacheFile: cache, fetchImpl: counting, now: NOW + CACHE_TTL_MS }))
    await runUpdateCheck(options({ cacheFile: cache, fetchImpl: counting, now: NOW + CACHE_TTL_MS, force: true }))
    expect(calls).toBe(3)
  })
})

describe('the request', () => {
  test('a plain GET with a generic User-Agent and no credentials', async () => {
    const seen: Array<{ url: string; init: RequestInit }> = []
    await fetchManifest(DEFAULT_MANIFEST_URL, { fetchImpl: jsonFetch(MANIFEST, 200, seen) })
    expect(seen).toHaveLength(1)
    expect(seen[0]!.url).toBe(DEFAULT_MANIFEST_URL)
    expect(seen[0]!.init.method).toBe('GET')
    expect(seen[0]!.init.credentials).toBe('omit')
    const headers = seen[0]!.init.headers as Record<string, string>
    expect(Object.keys(headers).sort()).toEqual(['Accept', 'User-Agent'])
    expect(headers['User-Agent']).toBe(UPDATE_CHECK_USER_AGENT)
    expect(UPDATE_CHECK_USER_AGENT).not.toMatch(/\d/)
  })

  test('a timeout, an error status, bad JSON and a foreign manifest are failures, never throws', async () => {
    const hang: FetchLike = (_url, init) =>
      new Promise((_, reject) => {
        init.signal?.addEventListener('abort', () => reject(new Error('aborted')))
      })
    const deaf: FetchLike = () => new Promise(() => {})
    const thrown: FetchLike = async () => {
      throw new TypeError('network down')
    }
    const cases: Array<[FetchLike, RegExp]> = [
      [hang, /timed out/],
      [deaf, /timed out/],
      [jsonFetch({ error: 'no' }, 500), /500/],
      [jsonFetch('<html>'), /not JSON/],
      [jsonFetch({ ...MANIFEST, schema_version: 2 }), /not one this version reads/],
      [jsonFetch({ ...MANIFEST, product: 'desktop' }), /not one this version reads/],
      [thrown, /network down/],
    ]
    for (const [fetchImpl, reason] of cases) {
      const started = Date.now()
      const result = await runUpdateCheck(options({ fetchImpl, timeoutMs: 50 }))
      expect(result.status).toBe('failed')
      if (result.status === 'failed') expect(result.reason).toMatch(reason)
      expect(Date.now() - started).toBeLessThan(2000)
    }
  })

  test('a local test server through NEURON_UPDATE_MANIFEST_URL', async () => {
    const server = Bun.serve({
      port: 0,
      hostname: '127.0.0.1',
      fetch: (request) =>
        new URL(request.url).pathname === '/latest.json'
          ? Response.json({ ...MANIFEST, version: '7.0.0-beta.1', minimum_version: '7.0.0-alpha.6' })
          : new Response('missing', { status: 404 }),
    })
    try {
      const url = `http://127.0.0.1:${server.port}/latest.json`
      const result = await runUpdateCheck(
        options({
          fetchImpl: undefined,
          env: { NEURON_UPDATE_MANIFEST_URL: url },
          execPath: '/home/dev/.cache/neuroncli/bin/neuron-tui',
        }),
      )
      expect(result).toMatchObject({
        status: 'available',
        latest: '7.0.0-beta.1',
        required: true,
        command: 'python -m pip install --pre neuroncli',
        manifestUrl: url,
      })
      const missing = await runUpdateCheck(
        options({ fetchImpl: undefined, env: { NEURON_UPDATE_MANIFEST_URL: `http://127.0.0.1:${server.port}/x` } }),
      )
      expect(missing).toMatchObject({ status: 'failed', reason: 'the server answered 404' })
    } finally {
      server.stop(true)
    }
  })
})

describe('/update', () => {
  afterEach(() => resetUpdateCheckForTesting())

  test('what it says for each result', () => {
    const available = evaluateManifest('7.0.0-alpha.5', parseManifest(MANIFEST)!, 'powershell', DEFAULT_MANIFEST_URL)
    const report = formatUpdateReport(available, true)
    expect(report).toContain('Installed: 7.0.0-alpha.5')
    expect(report).toContain('Latest: 7.0.0-alpha.6')
    expect(report).toContain('Release notes: https://zero-x.live/changelog#7.0.0-alpha.6')
    expect(report).toContain('Update with: irm https://zero-x.live/install.ps1 | iex')
    expect(report).toContain('copied')

    expect(
      formatUpdateReport({ status: 'up-to-date', current: '7.0.0-alpha.6', latest: '7.0.0-alpha.6', notesUrl: null, manifestUrl: DEFAULT_MANIFEST_URL }),
    ).toBe('Neuron 7.0.0-alpha.6 is up to date (latest release: 7.0.0-alpha.6).')
    const off = formatUpdateReport({ status: 'disabled', current: '0.0.0', reason: 'CI is set', manifestUrl: DEFAULT_MANIFEST_URL })
    expect(off).toContain('The update check is off: CI is set.')
    expect(off).toContain(`Release manifest: ${DEFAULT_MANIFEST_URL}`)
    const failed = formatUpdateReport({ status: 'failed', current: '7.0.0', reason: 'timed out', manifestUrl: DEFAULT_MANIFEST_URL })
    expect(failed).toContain('Could not check for updates: timed out.')
    expect(failed).toContain(`Release manifest: ${DEFAULT_MANIFEST_URL}`)

    for (const text of [report, off, failed]) {
      expect(text).not.toMatch(/\u2014|claude|anthropic/i)
    }
  })

  test('the store holds the result the status bar shows', async () => {
    const result = await refreshUpdateCheck(options())
    expect(result.status).toBe('available')
    const state = useUpdateStore.getState()
    expect(state).toMatchObject({
      status: 'available',
      current: '7.0.0-alpha.5',
      latest: '7.0.0-alpha.6',
      minimum: null,
      notesUrl: MANIFEST.notes_url,
      command: 'curl -fsSL https://zero-x.live/install.sh | sh',
      required: false,
    })
    expect(updateNotice(state)).toEqual({ command: 'curl -fsSL https://zero-x.live/install.sh | sh', required: false })

    // Offline on a later /update: the report says so, the notice stays.
    const offline = await refreshUpdateCheck(options({ fetchImpl: jsonFetch('down', 503) }))
    expect(offline.status).toBe('failed')
    expect(updateNotice(useUpdateStore.getState())).not.toBeNull()

    await refreshUpdateCheck(options({ version: '7.0.0-alpha.6' }))
    expect(useUpdateStore.getState().status).toBe('up-to-date')
    expect(updateNotice(useUpdateStore.getState())).toBeNull()
  })
})
