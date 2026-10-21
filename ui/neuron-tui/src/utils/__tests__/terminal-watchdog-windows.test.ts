// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import path from 'path'

import { describe, expect, test } from 'bun:test'

import {
  admitWindowsWatchdog,
  buildWindowsWatchdogScripts,
  inspectWindowsWatchdogMarkers,
  MAX_CONCURRENT_WINDOWS_WATCHDOGS,
  windowsWatchdogPidPath,
  windowsWatchdogSpawnOptions,
} from '../terminal-watchdog'

import type { WindowsWatchdogInspection } from '../terminal-watchdog'

const TMP = 'C:\\Temp'
const DISARM = `${TMP}\\neuron-tui-watchdog-disarm-4242-abc123`

function scripts(
  overrides: { ownerStartOverride?: string; workingDirectory?: string } = {},
) {
  return buildWindowsWatchdogScripts({
    ownerPid: 4242,
    disarmPath: DISARM,
    armedPath: `${DISARM}.armed`,
    powershell:
      'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe',
    ...overrides,
  })
}

describe('Windows watchdog scripts', () => {
  test('the watchdog script keeps the no-double-quote invariant', () => {
    // It rides inside the one double-quoted region of Start-Process's
    // argument string; a `"` would split it into separate arguments.
    expect(scripts().watchdogScript).not.toContain('"')
  })

  test('waits only on a process whose pid AND creation time are its owner', () => {
    const { watchdogScript } = scripts()
    expect(watchdogScript).toContain('Get-Process -Id 4242')
    expect(watchdogScript).toContain(
      '$o.StartTime.ToFileTimeUtc() - __NEURON_TUI_OWNER_START__',
    )
    // The only wait is on the identity-checked handle...
    expect(watchdogScript).toContain('if ($w) { try { $w.WaitForExit() }')
    // ...never on a bare pid, which a reused pid would satisfy.
    expect(watchdogScript).not.toContain('Wait-Process')
  })

  test('announces armed only after the owner is resolved, and records both pids', () => {
    const { watchdogScript } = scripts()
    const resolved = watchdogScript.indexOf('Get-Process -Id 4242')
    const pidFile = watchdogScript.indexOf(windowsWatchdogPidPath(DISARM))
    const armed = watchdogScript.indexOf(`${DISARM}.armed`)
    const wait = watchdogScript.indexOf('WaitForExit')
    expect(resolved).toBeGreaterThanOrEqual(0)
    expect(resolved).toBeLessThan(pidFile)
    expect(pidFile).toBeLessThan(armed)
    expect(armed).toBeLessThan(wait)
    expect(watchdogScript).toContain("[string]$PID + ' 4242'")
    // The pid file goes away with the watchdog, which is what makes a surviving
    // one mean "still running".
    expect(watchdogScript.trimEnd()).toEndWith(
      `Remove-Item -LiteralPath '${windowsWatchdogPidPath(DISARM)}' -Force -ErrorAction SilentlyContinue`,
    )
  })

  test('the bootstrap reads the owner creation time and substitutes it', () => {
    const { bootstrapScript } = scripts()
    expect(bootstrapScript).toStartWith(
      '$t = (Get-Process -Id 4242 -ErrorAction Stop).StartTime.ToFileTimeUtc(); ',
    )
    expect(bootstrapScript).toContain(
      ".Replace('__NEURON_TUI_OWNER_START__', [string]$t)",
    )
  })

  test('a test override can only ever inject digits', () => {
    expect(scripts({ ownerStartOverride: '1' }).bootstrapScript).toStartWith(
      '$t = 1; ',
    )
    expect(
      scripts({ ownerStartOverride: "1; Remove-Item 'x'" }).bootstrapScript,
    ).toStartWith('$t = 0; ')
  })

  test('the grandchild starts in the given directory, not the project', () => {
    const { bootstrapScript } = scripts({ workingDirectory: "C:\\Temp\\o'brien" })
    expect(bootstrapScript).toEndWith(" -WorkingDirectory 'C:\\Temp\\o''brien'")
    expect(scripts().bootstrapScript).not.toContain('-WorkingDirectory')
  })
})

describe('windowsWatchdogSpawnOptions', () => {
  // PowerShell's caches live under %LOCALAPPDATA%; without it the path is
  // relative and they appeared in the project as Microsoft/Windows/PowerShell.
  test('runs in the temp dir with the module analysis cache there', () => {
    const { cwd, env } = windowsWatchdogSpawnOptions(
      { LOCALAPPDATA: 'C:\\Users\\me\\AppData\\Local', PATH: 'x' },
      TMP,
    )
    expect(cwd).toBe(TMP)
    expect(env.PSModuleAnalysisCachePath).toBe(
      path.join(TMP, 'neuron-tui-powershell-ModuleAnalysisCache'),
    )
    expect(env.LOCALAPPDATA).toBe('C:\\Users\\me\\AppData\\Local')
    expect(env.PATH).toBe('x')
  })

  test('a missing LOCALAPPDATA points at the temp dir, never the cwd', () => {
    expect(windowsWatchdogSpawnOptions({}, TMP).env.LOCALAPPDATA).toBe(TMP)
    expect(
      windowsWatchdogSpawnOptions({ LOCALAPPDATA: '' }, TMP).env.LOCALAPPDATA,
    ).toBe(TMP)
  })
})

type FakeFile = { content?: string; ageMs?: number }

function fakeTmp(files: Record<string, FakeFile>, alive: number[]) {
  const removed: string[] = []
  const deps = {
    dir: TMP,
    selfPid: 1,
    listDir: () => Object.keys(files),
    readFile: (file: string) => {
      const entry = files[path.basename(file)]
      if (entry?.content === undefined) throw new Error('ENOENT')
      return entry.content
    },
    ageMs: (file: string) => files[path.basename(file)]?.ageMs ?? 0,
    remove: (file: string) => {
      removed.push(path.basename(file))
    },
    isAlive: (pid: number) => alive.includes(pid),
  }
  return { deps, removed }
}

describe('inspectWindowsWatchdogMarkers', () => {
  const HOUR = 60 * 60 * 1000

  test('a pid file whose owner died but whose watchdog runs is an orphan', () => {
    const { deps, removed } = fakeTmp(
      {
        'neuron-tui-watchdog-disarm-100-aaa.pid': { content: '900 100' },
        'neuron-tui-watchdog-disarm-100-aaa.armed': { content: 'armed' },
      },
      [900],
    )
    expect(inspectWindowsWatchdogMarkers(deps)).toEqual({
      active: 0,
      orphaned: 1,
      legacyStale: 0,
      swept: 0,
    })
    expect(removed).toEqual([])
  })

  test('a live owner means a concurrent session, not a leak', () => {
    const { deps } = fakeTmp(
      { 'neuron-tui-watchdog-disarm-200-bbb.pid': { content: '901 200' } },
      [200, 901],
    )
    expect(inspectWindowsWatchdogMarkers(deps).active).toBe(1)
  })

  test('files of a watchdog that is provably gone are swept', () => {
    const { deps, removed } = fakeTmp(
      {
        'neuron-tui-watchdog-disarm-300-ccc.pid': { content: '902 300' },
        'neuron-tui-watchdog-disarm-300-ccc.armed': { content: 'armed' },
      },
      [],
    )
    const result = inspectWindowsWatchdogMarkers(deps)
    expect(result.orphaned).toBe(0)
    expect(result.legacyStale).toBe(0)
    expect(removed.sort()).toEqual([
      'neuron-tui-watchdog-disarm-300-ccc.armed',
      'neuron-tui-watchdog-disarm-300-ccc.pid',
    ])
  })

  test('an older-format marker with a dead owner is counted once, then swept', () => {
    const { deps, removed } = fakeTmp(
      { 'neuron-tui-watchdog-disarm-400-ddd.armed': { content: 'armed' } },
      [],
    )
    expect(inspectWindowsWatchdogMarkers(deps).legacyStale).toBe(1)
    expect(removed).toEqual(['neuron-tui-watchdog-disarm-400-ddd.armed'])
  })

  test('only disarm files an hour past a dead owner are swept', () => {
    const { deps, removed } = fakeTmp(
      {
        'neuron-tui-watchdog-disarm-500-eee': { ageMs: 2 * HOUR },
        'neuron-tui-watchdog-disarm-501-fff': { ageMs: 60_000 },
        'neuron-tui-watchdog-disarm-502-ggg': { ageMs: 2 * HOUR },
        'unrelated-file.armed': {},
      },
      [502],
    )
    inspectWindowsWatchdogMarkers(deps)
    expect(removed).toEqual(['neuron-tui-watchdog-disarm-500-eee'])
  })

  test("this process's own files are never judged", () => {
    const { deps, removed } = fakeTmp(
      { 'neuron-tui-watchdog-disarm-1-hhh.pid': { content: '903 1' } },
      [903],
    )
    expect(inspectWindowsWatchdogMarkers(deps)).toEqual({
      active: 0,
      orphaned: 0,
      legacyStale: 0,
      swept: 0,
    })
    expect(removed).toEqual([])
  })

  test('an unreadable temp dir yields nothing rather than throwing', () => {
    expect(
      inspectWindowsWatchdogMarkers({
        listDir: () => {
          throw new Error('EACCES')
        },
      }),
    ).toEqual({ active: 0, orphaned: 0, legacyStale: 0, swept: 0 })
  })
})

describe('admitWindowsWatchdog', () => {
  const inspection =
    (
      overrides: Partial<WindowsWatchdogInspection>,
    ): (() => WindowsWatchdogInspection) =>
    () => ({ active: 0, orphaned: 0, legacyStale: 0, swept: 0, ...overrides })

  test('a clean machine arms and reports nothing', () => {
    const reports: unknown[] = []
    expect(
      admitWindowsWatchdog(inspection({ active: 2 }), (...args) => {
        reports.push(args)
      }),
    ).toBe(true)
    expect(reports).toEqual([])
  })

  test('leaked watchdogs are reported as outliving their parent', () => {
    const reports: unknown[] = []
    expect(
      admitWindowsWatchdog(
        inspection({ orphaned: 2, legacyStale: 1, active: 1 }),
        (...args) => {
          reports.push(args)
        },
      ),
    ).toBe(true)
    expect(reports).toEqual([
      [
        'cli.helper_outlived_parent',
        { kind: 'terminal_watchdog', count: 2, legacyStale: 1, active: 1 },
      ],
    ])
  })

  test('a machine at the cap gets no new watchdog', () => {
    const reports: Array<[string, Record<string, unknown>]> = []
    expect(
      admitWindowsWatchdog(
        inspection({
          active: MAX_CONCURRENT_WINDOWS_WATCHDOGS - 1,
          orphaned: 1,
        }),
        (event, data) => {
          reports.push([event, data])
        },
      ),
    ).toBe(false)
    expect(reports.map(([event]) => event)).toEqual([
      'cli.helper_outlived_parent',
      'cli.helper_process_flood',
    ])
    expect(reports[1]![1]).toMatchObject({
      kind: 'terminal_watchdog',
      scope: 'machine',
      live: MAX_CONCURRENT_WINDOWS_WATCHDOGS,
      skipped: true,
    })
  })

  test('an inspection that throws never blocks arming', () => {
    expect(
      admitWindowsWatchdog(
        () => {
          throw new Error('boom')
        },
        () => {},
      ),
    ).toBe(true)
  })
})
