// Windows only: runs the UI in a real Windows pseudoconsole (ConPTY, which
// Windows Terminal runs programs in) and uses it as a person would. It covers
// typing and Enter, the permission dialog (allow, allow for the session,
// deny), resizing, Ctrl-C to quit, a bare `neuron` handing the console to the
// UI, and `neuron --continue <chat>` reopening a chat. The steps are
// scripts/windows-console/*.txt, played by ConPtyDrive.cs, which this compiles
// with the csc.exe that ships with Windows.
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

// The chat from the first run, reopened by the second (one home for both). The
// exit hint that names it can't be read here: under the pseudoconsole Windows
// itself provides (kernel32's CreatePseudoConsole, which this driver uses), the
// console ends for every process in it as OpenTUI's renderer tears down, so
// nothing printed after that arrives. A classic console window and Windows
// Terminal keep it, and there `neuron` prints the hint once the UI has exited
// (full_screen.rs). So the chat id comes from the saved chat instead.
const home = path.join(work, 'home')
fs.mkdirSync(home)
const first = play('continue-1-chat.txt', ['--via-neuron'], { NEURON_TUI_DEV_HOME: home })
const chat = savedChatIds(home)[0]
results.push(first.passed && Boolean(chat))
if (chat) {
  results.push(play('continue-2-reopen.txt', ['--via-neuron', '--continue', chat], { NEURON_TUI_DEV_HOME: home }).passed)
} else {
  console.log('FAIL  continue-1-chat.txt saved no chat')
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

try {
  fs.rmSync(work, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 })
} catch {
  // The engine may still hold the home for a moment; it is in the temp folder.
}
const failed = results.filter((ok) => !ok).length
console.log(failed === 0 ? `all ${results.length} console checks passed` : `${failed} of ${results.length} console checks failed`)
process.exit(failed === 0 ? 0 : 1)
