/**
 * The operating system's file dialog, for the Attach button: PowerShell with
 * System.Windows.Forms.OpenFileDialog on Windows, osascript's `choose file` on
 * macOS, zenity or kdialog on a Linux desktop. When none can open (over SSH,
 * no desktop, the tool missing, or it fails), the caller opens the in-UI file
 * browser instead.
 *
 * Nothing here goes through a shell, and no path is ever put into a script:
 * the start folder reaches PowerShell as an environment variable and the
 * others as an argument of its own, so a folder or file name cannot inject a
 * command. Picked paths are only read back as text, never run.
 *
 * `NEURON_TUI_FILE_DIALOG=0` turns the dialog off (the in-UI browser opens).
 */

export type DialogTool = 'powershell' | 'osascript' | 'zenity' | 'kdialog'

export interface DialogCommand {
  tool: DialogTool
  /** The program and its arguments, run without a shell. */
  argv: string[]
  /** Variables added to the dialog's environment. */
  env?: Record<string, string>
}

export type DialogAvailability =
  | { available: true; tool: DialogTool; executable: string }
  | { available: false; reason: string }

export type DialogResult =
  | { status: 'picked'; paths: string[] }
  | { status: 'cancelled' }
  | { status: 'failed'; reason: string }

/** Turns the OS dialog off when set to 0/false/no/off. */
export const FILE_DIALOG_ENV = 'NEURON_TUI_FILE_DIALOG'

/** The variable the Windows script reads its start folder from. */
export const START_DIR_ENV = 'NEURON_ATTACH_START_DIR'

/** How long a dialog may stay open before it is closed (a person is browsing). */
export const DIALOG_TIMEOUT_MS = 10 * 60 * 1000

const DIALOG_TITLE = 'Attach files to Neuron'

type Env = Record<string, string | undefined>
type Which = (command: string) => string | null

function isOff(value: string | undefined): boolean {
  return ['0', 'false', 'no', 'off'].includes(value?.trim().toLowerCase() ?? '')
}

/** Which dialog can open here, or why none can. */
export function fileDialogAvailability(
  platform: NodeJS.Platform,
  env: Env,
  which: Which,
): DialogAvailability {
  if (isOff(env[FILE_DIALOG_ENV])) {
    return { available: false, reason: `${FILE_DIALOG_ENV} is off` }
  }
  if (env.SSH_CONNECTION || env.SSH_CLIENT || env.SSH_TTY) {
    return { available: false, reason: 'running over SSH' }
  }
  if (platform === 'win32') {
    const executable = which('powershell.exe') ?? which('powershell') ?? which('pwsh.exe') ?? which('pwsh')
    return executable
      ? { available: true, tool: 'powershell', executable }
      : { available: false, reason: 'PowerShell was not found' }
  }
  if (platform === 'darwin') {
    const executable = which('osascript')
    return executable
      ? { available: true, tool: 'osascript', executable }
      : { available: false, reason: 'osascript was not found' }
  }
  if (!env.DISPLAY && !env.WAYLAND_DISPLAY) {
    return { available: false, reason: 'no desktop session' }
  }
  const zenity = which('zenity')
  if (zenity) return { available: true, tool: 'zenity', executable: zenity }
  const kdialog = which('kdialog')
  if (kdialog) return { available: true, tool: 'kdialog', executable: kdialog }
  return { available: false, reason: 'neither zenity nor kdialog is installed' }
}

/**
 * The Windows dialog. A fixed script, sent encoded so nothing is quoted; the
 * start folder comes from {@link START_DIR_ENV}. A topmost owner keeps the
 * dialog in front of the terminal, and the output is UTF-8 so non-ASCII names
 * come back intact. One picked path per line; nothing on cancel.
 */
export const WINDOWS_DIALOG_SCRIPT = [
  "$ErrorActionPreference = 'Stop'",
  '[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false',
  'Add-Type -AssemblyName System.Windows.Forms',
  '[System.Windows.Forms.Application]::EnableVisualStyles()',
  '$owner = New-Object System.Windows.Forms.Form',
  '$owner.TopMost = $true',
  '$owner.ShowInTaskbar = $false',
  '$dialog = New-Object System.Windows.Forms.OpenFileDialog',
  '$dialog.Multiselect = $true',
  `$dialog.Title = '${DIALOG_TITLE}'`,
  "$dialog.Filter = 'All files (*.*)|*.*|Documents (*.pdf;*.docx;*.pptx;*.xlsx;*.xlsm;*.xls;*.ods)|*.pdf;*.docx;*.pptx;*.xlsx;*.xlsm;*.xls;*.ods|Images|*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp'",
  `$start = $env:${START_DIR_ENV}`,
  'if ($start -and (Test-Path -LiteralPath $start -PathType Container)) { $dialog.InitialDirectory = $start }',
  '$result = $dialog.ShowDialog($owner)',
  '$owner.Dispose()',
  'if ($result -eq [System.Windows.Forms.DialogResult]::OK) { foreach ($name in $dialog.FileNames) { [Console]::Out.WriteLine($name) } }',
].join('\n')

/** PowerShell's -EncodedCommand form: base64 of the UTF-16LE script. */
export function encodePowerShellCommand(script: string): string {
  return Buffer.from(script, 'utf16le').toString('base64')
}

export function buildWindowsDialogCommand(executable: string, startDir: string): DialogCommand {
  return {
    tool: 'powershell',
    argv: [
      executable,
      '-NoProfile',
      '-NonInteractive',
      '-STA',
      '-EncodedCommand',
      encodePowerShellCommand(WINDOWS_DIALOG_SCRIPT),
    ],
    env: { [START_DIR_ENV]: startDir },
  }
}

/**
 * The macOS dialog: the start folder is the script's first argument (`argv`
 * of its run handler), never part of the script. Cancel (-128) prints nothing.
 */
export const MAC_DIALOG_SCRIPT_LINES = [
  'on run argv',
  'set startDir to item 1 of argv',
  'try',
  `set picked to choose file with prompt "${DIALOG_TITLE}" default location (POSIX file startDir) with multiple selections allowed`,
  'on error number -128',
  'return ""',
  'end try',
  'set out to ""',
  'repeat with f in picked',
  'set out to out & POSIX path of f & linefeed',
  'end repeat',
  'return out',
  'end run',
]

export function buildMacDialogCommand(executable: string, startDir: string): DialogCommand {
  const argv = [executable]
  for (const line of MAC_DIALOG_SCRIPT_LINES) argv.push('-e', line)
  // Arguments after the script go to its run handler.
  argv.push(startDir)
  return { tool: 'osascript', argv }
}

/** A folder as one argument: absolute, so it can never read as an option. */
function folderArgument(startDir: string): string {
  return startDir.startsWith('/') ? startDir : `/${startDir}`
}

export function buildZenityCommand(executable: string, startDir: string): DialogCommand {
  const folder = folderArgument(startDir)
  return {
    tool: 'zenity',
    argv: [
      executable,
      '--file-selection',
      '--multiple',
      '--separator=\n',
      `--title=${DIALOG_TITLE}`,
      `--filename=${folder.endsWith('/') ? folder : `${folder}/`}`,
    ],
  }
}

export function buildKdialogCommand(executable: string, startDir: string): DialogCommand {
  return {
    tool: 'kdialog',
    argv: [
      executable,
      '--title',
      DIALOG_TITLE,
      '--getopenfilename',
      folderArgument(startDir),
      '--multiple',
      '--separate-output',
    ],
  }
}

export function buildDialogCommand(tool: DialogTool, executable: string, startDir: string): DialogCommand {
  switch (tool) {
    case 'powershell':
      return buildWindowsDialogCommand(executable, startDir)
    case 'osascript':
      return buildMacDialogCommand(executable, startDir)
    case 'zenity':
      return buildZenityCommand(executable, startDir)
    case 'kdialog':
      return buildKdialogCommand(executable, startDir)
  }
}

/** The picked paths in a dialog's output, one per line. */
export function parseDialogOutput(stdout: string): string[] {
  // Only the line ends go: a file name may start or end with a space.
  return stdout.split(/\r?\n/).filter((line) => line.trim().length > 0)
}

/**
 * What a dialog's exit means. Every tool prints nothing on cancel: PowerShell
 * and osascript exit 0 (the scripts handle it), zenity and kdialog exit 1.
 */
export function interpretDialogExit(
  tool: DialogTool,
  exitCode: number | null,
  stdout: string,
  stderr: string,
): DialogResult {
  const paths = parseDialogOutput(stdout)
  if (exitCode === 0) return paths.length > 0 ? { status: 'picked', paths } : { status: 'cancelled' }
  // GTK and KDE often print warnings on stderr, so exit 1 alone decides.
  if ((tool === 'zenity' || tool === 'kdialog') && exitCode === 1 && paths.length === 0) {
    return { status: 'cancelled' }
  }
  // PowerShell writes its progress records to a redirected stderr as CLIXML.
  const detail = stderr
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line && !line.startsWith('#<') && !line.startsWith('<'))
  return {
    status: 'failed',
    reason: detail ? `${tool} failed: ${detail.slice(0, 160)}` : `${tool} exited with code ${exitCode}`,
  }
}

export interface SpawnedDialog {
  exited: Promise<number | null>
  stdout: Promise<string>
  stderr: Promise<string>
  kill: () => void
}

export type DialogSpawner = (command: DialogCommand) => SpawnedDialog

/** Runs the dialog with Bun, without a shell and without a console window. */
export const spawnDialog: DialogSpawner = (command) => {
  const child = Bun.spawn(command.argv, {
    env: { ...process.env, ...command.env },
    stdin: 'ignore',
    stdout: 'pipe',
    stderr: 'pipe',
    windowsHide: true,
  })
  return {
    exited: child.exited,
    stdout: new Response(child.stdout).text(),
    stderr: new Response(child.stderr).text(),
    kill: () => child.kill(),
  }
}

let running: SpawnedDialog | null = null

/** Closes a dialog still open (the app is exiting). */
export function closeOpenDialog(): void {
  running?.kill()
  running = null
}

/** Whether a dialog is open now: one at a time. */
export function isDialogOpen(): boolean {
  return running !== null
}

/**
 * Opens the dialog and waits for it without blocking the UI. A timeout closes
 * it and counts as a failure; a spawn error is a failure too.
 */
export async function runFileDialog(
  command: DialogCommand,
  options: { spawner?: DialogSpawner; timeoutMs?: number } = {},
): Promise<DialogResult> {
  if (running) return { status: 'failed', reason: 'a file dialog is already open' }
  const spawner = options.spawner ?? spawnDialog
  let dialog: SpawnedDialog
  try {
    dialog = spawner(command)
  } catch (error) {
    return { status: 'failed', reason: `${command.tool} could not start: ${String(error)}` }
  }
  running = dialog
  let timer: ReturnType<typeof setTimeout> | undefined
  const timedOut = new Promise<'timeout'>((resolve) => {
    timer = setTimeout(() => resolve('timeout'), options.timeoutMs ?? DIALOG_TIMEOUT_MS)
  })
  try {
    const outcome = await Promise.race([dialog.exited, timedOut])
    if (outcome === 'timeout') {
      dialog.kill()
      return { status: 'failed', reason: 'the file dialog was open too long and was closed' }
    }
    const [stdout, stderr] = await Promise.all([dialog.stdout, dialog.stderr])
    return interpretDialogExit(command.tool, outcome, stdout, stderr)
  } catch (error) {
    return { status: 'failed', reason: `${command.tool} failed: ${String(error)}` }
  } finally {
    if (timer) clearTimeout(timer)
    if (running === dialog) running = null
  }
}

process.once('exit', closeOpenDialog)
