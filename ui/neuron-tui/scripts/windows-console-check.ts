// Windows only: runs the UI in a real Windows pseudoconsole (ConPTY, which
// Windows Terminal runs programs in) and uses it as a person would. It covers
// typing and Enter, the permission dialog (allow, allow for the session,
// deny), resizing, Ctrl-C to quit, a bare `neuron` handing the console to the
// UI, and `neuron --continue <chat>` reopening a chat. The steps are
// scripts/windows-console/*.txt, played by ConPtyDrive.cs, which this compiles
// with the csc.exe that ships with Windows. Then two exits in a classic console
// window (classic-console-check.ps1 with ConsoleKeys.cs): the window must
// survive them and show the --continue hint.
//
//   bun run check:console
//
// Needs what `bun run dev:mock --compiled` needs: the Rust debug binaries
// (`cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/) and
// the compiled UI (`bun run build:binary 0.0.0-dev`).
import fs from 'fs'
import os from 'os'
import path from 'path'

if (process.platform !== 'win32') {
  console.log('check:console runs on Windows only; skipped.')
  process.exit(0)
}

const here = import.meta.dir
const steps = path.join(here, 'windows-console')
const devScript = path.join(here, 'dev-against-mock.ts')
const csc = path.join(process.env.WINDIR ?? 'C:\\Windows', 'Microsoft.NET', 'Framework64', 'v4.0.30319', 'csc.exe')
if (!fs.existsSync(csc)) {
  console.error(`csc.exe not found at ${csc}; it ships with the .NET Framework 4 in Windows.`)
  process.exit(1)
}

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-console-check-'))
const driver = path.join(work, 'ConPtyDrive.exe')
const compile = Bun.spawnSync([csc, '-nologo', `-out:${driver}`, path.join(steps, 'ConPtyDrive.cs')])
if (compile.exitCode !== 0) {
  console.error(compile.stdout.toString() + compile.stderr.toString())
  process.exit(1)
}

const quote = (value: string) => `"${value}"`

/** Plays one step file; returns the driver's report. */
function play(file: string, flags: string[], env: Record<string, string> = {}) {
  const run = Bun.spawnSync(
    [driver, '120', '36', path.join(steps, file), work, quote(process.execPath), quote(devScript), ...flags],
    { env: { ...process.env, ...env }, timeout: 280_000 },
  )
  const report = run.stdout.toString() + run.stderr.toString()
  const passed = run.exitCode === 0 && /^RESULT pass/m.test(report)
  console.log(`${passed ? 'pass' : 'FAIL'}  ${file} ${flags.join(' ')}`)
  if (!passed) {
    // Every step, then the screen where it stopped.
    const lines = report.split('\n')
    console.log(lines.filter((line) => /^(ok|FAIL) /.test(line)).join('\n'))
    const stop = lines.findIndex((line) => /screen: (after failed|still running)/.test(line))
    if (stop >= 0) console.log(lines.slice(stop, stop + 40).filter((line) => line.trim()).join('\n'))
  }
  return { passed, report }
}

const results = [
  play('type-submit-allow.txt', ['--compiled']).passed,
  play('resize-deny-quit.txt', ['--compiled']).passed,
  play('neuron-hand-off.txt', ['--via-neuron']).passed,
]

// The chat from the first run, named by its exit hint, reopened by the second
// (one home for both). The hint must name the chat the UI saved.
const home = path.join(work, 'home')
fs.mkdirSync(home)
const first = play('continue-1-chat.txt', ['--via-neuron'], { NEURON_TUI_DEV_HOME: home })
const chat = /neuron --continue (\S+)/.exec(first.report)?.[1]
const named = Boolean(chat) && savedChatIds(home).includes(chat!)
if (first.passed && !named) console.log('FAIL  the exit hint does not name the saved chat')
results.push(first.passed && named)
if (chat) {
  results.push(play('continue-2-reopen.txt', ['--via-neuron', '--continue', chat], { NEURON_TUI_DEV_HOME: home }).passed)
} else {
  console.log('FAIL  no chat id in the exit hint')
  results.push(false)
}

/** Ids of the chats saved under `home`'s UI config, newest first. */
function savedChatIds(home: string): string[] {
  const projects = path.join(home, 'tui', 'projects')
  const chats: { id: string; mtime: number }[] = []
  for (const project of fs.existsSync(projects) ? fs.readdirSync(projects) : []) {
    const dir = path.join(projects, project, 'chats')
    for (const id of fs.existsSync(dir) ? fs.readdirSync(dir) : []) {
      const messages = path.join(dir, id, 'chat-messages.json')
      if (fs.existsSync(messages)) chats.push({ id, mtime: fs.statSync(messages).mtimeMs })
    }
  }
  return chats.sort((a, b) => b.mtime - a.mtime).map((chat) => chat.id)
}

// A classic console window (conhost.exe), where Windows PowerShell 5.1 and
// cmd.exe open: quitting must leave the window alive, with the hint on it.
// conhost used to crash there (an access violation) as the UI quit, when the
// process lived on after its renderer's teardown and the key-up of the
// quitting key arrived (see exit-cleanly.ts).
for (const [mode, keys, label] of [
  ['--via-neuron', 'PARITY_SCENARIO:streaming_text<CR>|<SLEEP>|<CC>|<CC>', 'Ctrl-C twice, through neuron'],
  ['--compiled', 'PARITY_SCENARIO:streaming_text<CR>|<SLEEP>|/exit<CR>', '/exit, the UI on its own'],
  // A dead engine has nothing to wait for, so the exit would tear down at once.
  [
    '--compiled',
    'PARITY_SCENARIO:streaming_text<CR>|<SLEEP>|<KILLENGINE>|<CC>|<CC>',
    'Ctrl-C twice after the engine died, the UI on its own',
  ],
] as const) {
  results.push(classic(mode, keys, label))
}

/** Runs classic-console-check.ps1: types `keys`, then checks the window and its hint. */
function classic(mode: string, keys: string, label: string): boolean {
  const run = Bun.spawnSync(
    [
      'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
      '-File', path.join(steps, 'classic-console-check.ps1'),
      '-Work', work, '-Bun', process.execPath, '-DevScript', devScript,
      '-Mode', mode, '-Keys', keys, '-Expect', 'neuron --continue',
    ],
    { env: process.env, timeout: 200_000 },
  )
  const report = run.stdout.toString() + run.stderr.toString()
  const passed = /^RESULT pass/m.test(report)
  console.log(`${passed ? 'pass' : 'FAIL'}  classic console window: ${label}`)
  if (!passed) console.log(report.trim())
  return passed
}

try {
  fs.rmSync(work, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 })
} catch {
  // The engine may still hold the home for a moment; it is in the temp folder.
}
const failed = results.filter((ok) => !ok).length
console.log(failed === 0 ? `all ${results.length} console checks passed` : `${failed} of ${results.length} console checks failed`)
process.exit(failed === 0 ? 0 : 1)
