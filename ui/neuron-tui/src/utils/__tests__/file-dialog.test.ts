import { describe, expect, test } from 'bun:test'

import {
  FILE_DIALOG_ENV,
  MAC_DIALOG_SCRIPT_LINES,
  START_DIR_ENV,
  WINDOWS_DIALOG_SCRIPT,
  buildDialogCommand,
  buildKdialogCommand,
  buildMacDialogCommand,
  buildWindowsDialogCommand,
  buildZenityCommand,
  fileDialogAvailability,
  interpretDialogExit,
  parseDialogOutput,
  runFileDialog,
  type DialogCommand,
  type SpawnedDialog,
} from '../file-dialog'

const found = (names: string[]) => (command: string) => (names.includes(command) ? `/bin/${command}` : null)

describe('fileDialogAvailability', () => {
  test('Windows uses PowerShell', () => {
    expect(fileDialogAvailability('win32', {}, found(['powershell.exe']))).toEqual({
      available: true,
      tool: 'powershell',
      executable: '/bin/powershell.exe',
    })
    expect(fileDialogAvailability('win32', {}, found([])).available).toBe(false)
  })

  test('macOS uses osascript', () => {
    expect(fileDialogAvailability('darwin', {}, found(['osascript']))).toMatchObject({ tool: 'osascript' })
  })

  test('Linux needs a desktop session and zenity or kdialog', () => {
    expect(fileDialogAvailability('linux', {}, found(['zenity']))).toEqual({
      available: false,
      reason: 'no desktop session',
    })
    expect(fileDialogAvailability('linux', { DISPLAY: ':0' }, found(['zenity', 'kdialog']))).toMatchObject({
      tool: 'zenity',
    })
    expect(fileDialogAvailability('linux', { WAYLAND_DISPLAY: 'wayland-0' }, found(['kdialog']))).toMatchObject({
      tool: 'kdialog',
    })
    expect(fileDialogAvailability('linux', { DISPLAY: ':0' }, found([])).available).toBe(false)
  })

  test('never over SSH, even with a forwarded display', () => {
    expect(
      fileDialogAvailability('linux', { DISPLAY: 'localhost:10', SSH_CONNECTION: '1 2 3 4' }, found(['zenity'])),
    ).toEqual({ available: false, reason: 'running over SSH' })
    expect(fileDialogAvailability('darwin', { SSH_TTY: '/dev/ttys001' }, found(['osascript'])).available).toBe(
      false,
    )
  })

  test(`${FILE_DIALOG_ENV}=0 turns it off`, () => {
    expect(fileDialogAvailability('win32', { [FILE_DIALOG_ENV]: '0' }, found(['powershell.exe'])).available).toBe(
      false,
    )
    expect(fileDialogAvailability('win32', { [FILE_DIALOG_ENV]: '1' }, found(['powershell.exe'])).available).toBe(
      true,
    )
  })
})

// A folder name that would run something if it were ever put into a shell
// line or a script.
const HOSTILE = `C:\\ws\\a'; calc; $(whoami) "x" \`y\``
const HOSTILE_POSIX = `/home/me/a'; rm -rf ~; $(id) "x" \`y\``

describe('dialog command builders', () => {
  test('PowerShell runs a fixed encoded script; the folder only travels in the environment', () => {
    const command = buildWindowsDialogCommand('powershell.exe', HOSTILE)
    expect(command.argv.slice(0, 5)).toEqual(['powershell.exe', '-NoProfile', '-NonInteractive', '-STA', '-EncodedCommand'])
    expect(command.argv).toHaveLength(6)
    const script = Buffer.from(command.argv[5]!, 'base64').toString('utf16le')
    expect(script).toBe(WINDOWS_DIALOG_SCRIPT)
    expect(script).toContain('OpenFileDialog')
    expect(script).toContain('$dialog.Multiselect = $true')
    expect(script).toContain(`$env:${START_DIR_ENV}`)
    expect(command.argv.join('\n')).not.toContain('calc')
    expect(command.env).toEqual({ [START_DIR_ENV]: HOSTILE })
  })

  test('osascript gets the folder as its own argument, after the script', () => {
    const command = buildMacDialogCommand('osascript', HOSTILE_POSIX)
    expect(command.argv[0]).toBe('osascript')
    expect(command.argv.at(-1)).toBe(HOSTILE_POSIX)
    const scriptParts = command.argv.slice(1, -1)
    expect(scriptParts.filter((part) => part === '-e')).toHaveLength(MAC_DIALOG_SCRIPT_LINES.length)
    expect(scriptParts.join('\n')).not.toContain('rm -rf')
    expect(scriptParts.join('\n')).toContain('with multiple selections allowed')
  })

  test('zenity gets the folder inside one --filename argument', () => {
    const command = buildZenityCommand('zenity', HOSTILE_POSIX)
    expect(command.argv).toContain('--multiple')
    expect(command.argv).toContain('--file-selection')
    expect(command.argv.filter((part) => part.includes('rm -rf'))).toEqual([`--filename=${HOSTILE_POSIX}/`])
  })

  test('kdialog gets the folder as one argument after --getopenfilename', () => {
    const command = buildKdialogCommand('kdialog', HOSTILE_POSIX)
    const at = command.argv.indexOf('--getopenfilename')
    expect(command.argv[at + 1]).toBe(HOSTILE_POSIX)
    expect(command.argv).toContain('--multiple')
    expect(command.argv).toContain('--separate-output')
  })

  test('a folder that is not absolute can never read as an option', () => {
    expect(buildKdialogCommand('kdialog', '--help').argv).toContain('/--help')
  })

  test('buildDialogCommand picks the builder for the tool', () => {
    expect(buildDialogCommand('zenity', 'zenity', '/ws').tool).toBe('zenity')
    expect(buildDialogCommand('powershell', 'pwsh', 'C:\\ws').argv[0]).toBe('pwsh')
  })
})

describe('reading the dialog back', () => {
  test('one path per line, CRLF or LF, blanks dropped', () => {
    expect(parseDialogOutput('C:\\a b.pdf\r\nC:\\c.txt\r\n\r\n')).toEqual(['C:\\a b.pdf', 'C:\\c.txt'])
    // Names keep their spaces; only the line ends go.
    expect(parseDialogOutput('/ws/ odd .txt\n')).toEqual(['/ws/ odd .txt'])
  })

  test('exit 0 with paths is a pick, with none a cancel', () => {
    expect(interpretDialogExit('powershell', 0, 'C:\\a.pdf\n', '')).toEqual({ status: 'picked', paths: ['C:\\a.pdf'] })
    expect(interpretDialogExit('osascript', 0, '\n', '')).toEqual({ status: 'cancelled' })
  })

  test('zenity and kdialog cancel with exit 1, warnings or not', () => {
    expect(interpretDialogExit('zenity', 1, '', 'Gtk-WARNING: something')).toEqual({ status: 'cancelled' })
    expect(interpretDialogExit('kdialog', 1, '', '')).toEqual({ status: 'cancelled' })
  })

  test('any other exit is a failure that names the tool', () => {
    expect(interpretDialogExit('powershell', 1, '', 'Add-Type : cannot load')).toEqual({
      status: 'failed',
      reason: 'powershell failed: Add-Type : cannot load',
    })
    expect(interpretDialogExit('zenity', 255, '', '').status).toBe('failed')
    // PowerShell's progress records (CLIXML) are not the reason.
    expect(
      interpretDialogExit('powershell', 1, '', '#< CLIXML\r\n<Objs Version="1.1.0.1"></Objs>\r\nAccess denied\r\n'),
    ).toEqual({ status: 'failed', reason: 'powershell failed: Access denied' })
  })
})

function fakeDialog(result: { code?: number; stdout?: string; stderr?: string; hang?: boolean }) {
  let killed = false
  let release: (code: number | null) => void = () => {}
  const exited = new Promise<number | null>((resolve) => {
    release = resolve
    if (!result.hang) resolve(result.code ?? 0)
  })
  const spawned: SpawnedDialog = {
    exited,
    stdout: Promise.resolve(result.stdout ?? ''),
    stderr: Promise.resolve(result.stderr ?? ''),
    kill: () => {
      killed = true
      release(null)
    },
  }
  return { spawned, wasKilled: () => killed }
}

const command: DialogCommand = { tool: 'zenity', argv: ['zenity'] }

describe('runFileDialog', () => {
  test('returns the picked paths', async () => {
    const { spawned } = fakeDialog({ stdout: '/ws/a.pdf\n/ws/b.txt\n' })
    expect(await runFileDialog(command, { spawner: () => spawned })).toEqual({
      status: 'picked',
      paths: ['/ws/a.pdf', '/ws/b.txt'],
    })
  })

  test('a cancel picks nothing', async () => {
    const { spawned } = fakeDialog({ code: 1 })
    expect(await runFileDialog(command, { spawner: () => spawned })).toEqual({ status: 'cancelled' })
  })

  test('a spawn error is a failure', async () => {
    const result = await runFileDialog(command, {
      spawner: () => {
        throw new Error('ENOENT')
      },
    })
    expect(result.status).toBe('failed')
  })

  test('a dialog left open past the timeout is closed and counts as a failure', async () => {
    const dialog = fakeDialog({ hang: true })
    const result = await runFileDialog(command, { spawner: () => dialog.spawned, timeoutMs: 5 })
    expect(result.status).toBe('failed')
    expect(dialog.wasKilled()).toBe(true)
  })

  test('only one dialog at a time', async () => {
    const first = fakeDialog({ hang: true })
    const pending = runFileDialog(command, { spawner: () => first.spawned, timeoutMs: 50 })
    const second = await runFileDialog(command, { spawner: () => fakeDialog({}).spawned })
    expect(second).toEqual({ status: 'failed', reason: 'a file dialog is already open' })
    first.spawned.kill()
    await pending
  })
})
