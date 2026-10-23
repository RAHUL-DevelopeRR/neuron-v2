# Runs `bun run dev:mock <Mode>` in a classic console window (conhost.exe, which
# Windows PowerShell 5.1 and cmd.exe open in), types $Keys, and checks that the
# window outlives the UI's exit and shows $Expect afterwards, still there
# after the next line the console prints (a line that the shell's prompt
# would overwrite if the cursor had gone back up). Started by
# windows-console-check.ts. Keys: chunks split by `|`, `<CC>` is Ctrl-C, `<CR>`
# is Enter, `<SLEEP>` waits 4 s, `<KILLENGINE>` kills the UI's engine and waits
# for the UI to say it stopped, `<ALIVE>` types a probe and checks it shows up
# (the UI still reads the keyboard), then erases it. $Ready lists what the
# screen must show before typing (`;;` between them). -CheckTitle checks the
# window's title while the UI runs (Neuron's) and after it (the shell's again).
# -NoEscapes fails when an escape code shows on screen as text (`←[`).
# -Direct runs `neuron <Mode>` itself in the window, as a customer's shell does,
# against a mock service this script starts, instead of the dev script under
# Bun (which turns on the console's escape-code processing first, and stays
# attached to it); `bare` as the Mode means no arguments. Prints one
# `RESULT pass|fail ...` line.
param(
  [Parameter(Mandatory)] [string]$Work,
  [Parameter(Mandatory)] [string]$Bun,
  [Parameter(Mandatory)] [string]$DevScript,
  [string]$Mode = '--compiled',
  [string]$Keys = '<CC>|<CC>',
  [string]$Expect = '',
  [string]$Ready = 'Enter a coding task;;claude-opus',
  [switch]$CheckTitle,
  [switch]$NoEscapes,
  [switch]$Direct,
  # Typed during the exit (after `<THEN>` in $Keys), this must reach the shell whole:
  # the window then ends in an interactive cmd, whose output must show it ran.
  [string]$ShellGets = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ConsoleKeys.cs')
# NEURON_CONSOLE_TRACE names a file to log each step to, to debug a run that hangs.
function Trace([string]$step) {
  if ($env:NEURON_CONSOLE_TRACE) { Add-Content -Path $env:NEURON_CONSOLE_TRACE -Value "$(Get-Date -Format HH:mm:ss.fff) $step" }
}

$tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$marker = Join-Path $Work "classic-$tag.exit"
$wrapper = Join-Path $Work "classic-$tag.cmd"
$home_ = Join-Path $Work "classic-$tag-home"
New-Item -ItemType Directory -Force (Join-Path $home_ 'config') | Out-Null
# The bundled MCP servers start in the background and only slow the run down.
Set-Content -Encoding ascii (Join-Path $home_ 'config\settings.json') '{"defaultMcpServers": false}'
$originalTitle = 'ShellTitleBefore'
$mock = $null
# `call`, so a Bun that is a .cmd shim (npm's) returns to this script afterwards.
$launch = @("set `"NEURON_TUI_DEV_HOME=$home_`"", "call `"$Bun`" `"$DevScript`" $Mode")
if ($Direct) {
  # What dev-against-mock.ts sets up, without Bun: the mock service, a home and
  # a workspace, and only the environment a terminal and the OS need.
  $repo = (Resolve-Path (Join-Path (Split-Path $DevScript) '..\..\..')).Path
  $debug = Join-Path $repo 'rust\target\debug'
  $neuron = if ($env:NEURON_BIN) { $env:NEURON_BIN } else { Join-Path $debug 'neuron.exe' }
  $mockExe = if ($env:NEURON_MOCK_BIN) { $env:NEURON_MOCK_BIN } else { Join-Path $debug 'mock-anthropic-service.exe' }
  $ui = if ($env:NEURON_TUI_BIN) { $env:NEURON_TUI_BIN } else { Join-Path $repo 'ui\neuron-tui\bin\neuron-tui.exe' }
  $mockOut = Join-Path $Work "classic-$tag-mock.txt"
  $mock = Start-Process -FilePath $mockExe -RedirectStandardOutput $mockOut -PassThru -NoNewWindow
  $base = $null
  for ($i = 0; $i -lt 100 -and -not $base; $i++) {
    Start-Sleep -Milliseconds 100
    $text = Get-Content -Raw -ErrorAction SilentlyContinue $mockOut
    if ($text -match 'MOCK_ANTHROPIC_BASE_URL=(\S+)') { $base = $Matches[1] }
  }
  $workspace = Join-Path $home_ 'workspace'
  New-Item -ItemType Directory -Force $workspace | Out-Null
  Set-Content -Encoding ascii (Join-Path $workspace 'fixture.txt') "alpha parity line`nbeta line`ngamma parity line"
  $keep = 'PATH', 'PATHEXT', 'SystemRoot', 'SystemDrive', 'ComSpec', 'windir', 'TEMP', 'TMP', 'APPDATA', 'LOCALAPPDATA', 'NEURON_CONSOLE_TRACE'
  Get-ChildItem env: | Where-Object { $keep -notcontains $_.Name } | ForEach-Object { Remove-Item -LiteralPath "env:$($_.Name)" }
  $env:HOME = $home_
  $env:USERPROFILE = $home_
  $env:NEURON_CONFIG_HOME = Join-Path $home_ 'config'
  $env:NEURON_HOME = Join-Path $home_ 'neuron'
  $env:NEURON_CREDENTIAL_STORE = 'file'
  $env:NEURON_NO_BROWSER = '1'
  $env:NEURON_TUI_CONFIG_DIR = Join-Path $home_ 'tui'
  $env:NEURON_TUI_BIN = $ui
  $env:ANTHROPIC_API_KEY = 'mock-service-key'
  $env:ANTHROPIC_BASE_URL = $base
  $arguments = if ($Mode -eq 'bare') { '' } else { $Mode }
  $launch = @("cd /d `"$workspace`"", "`"$neuron`" $arguments")
}
Set-Content -Encoding ascii $wrapper (@(
  '@echo off',
  "title $originalTitle") + $launch + @(
  "echo exit %errorlevel% > `"$marker`"",
  # What a shell prints next: it must land below the hint, not over it.
  'echo AFTER_THE_UI',
  # Keeps the window open, so a window that dies is conhost's doing.
  $(if ($ShellGets) { 'cmd /d /k prompt $G' } else { 'ping -n 20 127.0.0.1 >nul' })
))

$conhost = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "cmd.exe /d /c `"$wrapper`"" -PassThru -WindowStyle Minimized
$result = $null
$pid_ = [uint32]0
try {
  # conhost.exe starts the command line it was given as its child.
  for ($i = 0; $i -lt 100 -and -not $pid_; $i++) {
    Start-Sleep -Milliseconds 200
    $pid_ = [ConsoleKeys]::ChildNamed([uint32]$conhost.Id, 'cmd.exe')
  }
  if (-not $pid_) { $result = 'fail: the console window did not start'; return }
  Trace "console up, cmd $pid_"
  $escape = [string][char]0x2190 + '['
  $readyPatterns = @($Ready -split ';;' | Where-Object { $_ })
  $screen = ''
  $isReady = $false
  for ($i = 0; $i -lt 180 -and -not $isReady; $i++) {
    Start-Sleep -Milliseconds 500
    $screen = [ConsoleKeys]::Screen($pid_)
    $isReady = [bool]$screen -and -not ($readyPatterns | Where-Object { $screen -notmatch $_ })
  }
  if (-not $isReady) { $result = "fail: the UI did not start`n$screen"; return }
  Trace "ready`n$screen"
  # With a trace, keep every process of the window, to report how each one ended.
  $watched = @()
  if ($env:NEURON_CONSOLE_TRACE) {
    # Reading Handle keeps one open, so the exit code stays readable after the exit.
    $watched = @(@([uint32]$conhost.Id) + [ConsoleKeys]::Descendants($pid_) | ForEach-Object {
      try { $process = [System.Diagnostics.Process]::GetProcessById([int]$_); [void]$process.Handle; @{ Process = $process; Name = $process.ProcessName } } catch {}
    })
    Trace "watching: $([ConsoleKeys]::Names([uint32]$conhost.Id))"
  }
  if ($NoEscapes -and $screen.Contains($escape)) { $result = "fail: escape codes show as text`n$screen"; return }
  Start-Sleep -Milliseconds 800
  if ($CheckTitle) {
    # Past the 1 s and 3 s the UI waits before setting its title again.
    Start-Sleep -Seconds 4
    $title = [ConsoleKeys]::Title($pid_)
    if ($title -cnotmatch '^Neuron') { $result = "fail: the window title is '$title' while the UI runs"; return }
  }
  foreach ($chunk in ($Keys -split '\|')) {
    Trace "key $chunk`n$(if ($env:NEURON_CONSOLE_TRACE) { [ConsoleKeys]::Screen($pid_) })"
    if ($chunk -eq '<SLEEP>') { Start-Sleep -Seconds 4; continue }
    if ($chunk -eq '<ALIVE>') {
      $probe = 'zq9'
      if (-not [ConsoleKeys]::Type($pid_, $probe)) { $result = 'fail: could not type the probe'; return }
      $shown = $false
      for ($i = 0; $i -lt 24 -and -not $shown; $i++) {
        Start-Sleep -Milliseconds 250
        $shown = ([ConsoleKeys]::Screen($pid_)) -match $probe
      }
      if (-not $shown) {
        $result = "fail: the UI no longer reads the keyboard (unread input events: $([ConsoleKeys]::PendingInput($pid_)); running: $([ConsoleKeys]::Names($pid_)))`n$([ConsoleKeys]::Screen($pid_))"
        return
      }
      [void][ConsoleKeys]::Type($pid_, "`b`b`b")
      Start-Sleep -Milliseconds 400
      continue
    }
    if ($chunk -eq '<KILLENGINE>') {
      $engine = [ConsoleKeys]::EngineBelow($pid_)
      if (-not $engine) { $result = 'fail: no engine process to kill'; return }
      Stop-Process -Id $engine -Force
      $stopped = $false
      for ($i = 0; $i -lt 40 -and -not $stopped; $i++) {
        Start-Sleep -Milliseconds 250
        $stopped = ([ConsoleKeys]::Screen($pid_)) -match 'engine stopped|engine exited|engine is not running'
      }
      if (-not $stopped) { $result = "fail: the UI did not say the engine stopped`n$([ConsoleKeys]::Screen($pid_))"; return }
      Start-Sleep -Milliseconds 800
      continue
    }
    # `<THEN>` types what follows it 150 ms after what precedes it, as a person
    # typing the next command right after quitting does.
    # A chunk ending in `<SHOWN>` must then be on the screen as typed: a dropped
    # key is reported as such, not as whatever the shortened text did next.
    $shownCheck = $chunk.EndsWith('<SHOWN>')
    $chunk = $chunk -replace '<SHOWN>$', ''
    foreach ($part in ($chunk -split '<THEN>')) {
      $text = $part -replace '<CC>', [string][char]3 -replace '<CR>', "`r"
      if (-not [ConsoleKeys]::Type($pid_, $text)) { $result = "fail: could not type [$chunk]"; return }
      Start-Sleep -Milliseconds 150
    }
    if ($shownCheck) {
      $shown = $false
      for ($i = 0; $i -lt 12 -and -not $shown; $i++) {
        $shown = ([ConsoleKeys]::Screen($pid_)).Contains($chunk)
        if (-not $shown) { Start-Sleep -Milliseconds 250 }
      }
      if (-not $shown) { $result = "fail: typed keys were dropped: '$chunk' is not on the screen`n$([ConsoleKeys]::Screen($pid_))"; return }
    }
    Start-Sleep -Milliseconds 900
  }
  Trace 'waiting for the exit'
  $last = ''
  for ($i = 0; $i -lt 80 -and -not (Test-Path $marker); $i++) {
    $seen = [ConsoleKeys]::Screen($pid_)
    if ($seen -and $seen -ne $last) { $last = $seen; Trace "waiting $i`n$seen" }
    if (-not (Get-Process -Id $conhost.Id -ErrorAction SilentlyContinue)) { break }
    Start-Sleep -Milliseconds 100
  }
  Trace "marker: $(Test-Path $marker)"
  foreach ($entry in $watched) {
    $process = $entry.Process
    $ended = try { if ($process.HasExited) { "exited $($process.ExitTime.ToString('HH:mm:ss.fff')) code 0x$('{0:X}' -f $process.ExitCode)" } else { 'running' } } catch { "unknown ($($_.Exception.Message))" }
    Trace "  $($entry.Name)#$($process.Id): $ended"
  }
  for ($i = 0; $i -lt 10; $i++) {
    $seen = [ConsoleKeys]::Screen($pid_)
    if ($seen -and $seen -ne $last) { $last = $seen; Trace "after $i`n$seen" }
    Start-Sleep -Milliseconds 50
  }
  Start-Sleep -Milliseconds 500
  if (-not (Get-Process -Id $conhost.Id -ErrorAction SilentlyContinue)) {
    $result = "fail: the console window (conhost.exe) died as the UI exited`n$last"
    return
  }
  if (-not (Test-Path $marker)) { $result = "fail: the UI did not exit`n$([ConsoleKeys]::Screen($pid_))"; return }
  for ($i = 0; $i -lt 20 -and -not (([ConsoleKeys]::Screen($pid_)) -match 'AFTER_THE_UI'); $i++) { Start-Sleep -Milliseconds 250 }
  $after = [ConsoleKeys]::Screen($pid_)
  if ($NoEscapes -and $after.Contains($escape)) { $result = "fail: escape codes show as text`n$after"; return }
  if ($CheckTitle) {
    $title = [ConsoleKeys]::Title($pid_)
    if ($title -ne $originalTitle) { $result = "fail: the window title is '$title' after the UI, not '$originalTitle' again"; return }
  }
  if ($Expect) {
    $lines = @($after -split "`n")
    $hintAt = [Array]::FindIndex($lines, [Predicate[string]] { param($line) $line -like "*$Expect*" })
    $nextAt = [Array]::FindIndex($lines, [Predicate[string]] { param($line) $line.Trim() -eq 'AFTER_THE_UI' })
    if ($hintAt -lt 0) { $result = "fail: '$Expect' is not on the screen after the exit`n$after"; return }
    if ($nextAt -le $hintAt) { $result = "fail: the next line did not land below the hint (it was overwritten?)`n$after"; return }
  }
  if ($ShellGets) {
    $ran = $false
    for ($i = 0; $i -lt 40 -and -not $ran; $i++) {
      Start-Sleep -Milliseconds 250
      $ran = @(([ConsoleKeys]::Screen($pid_)) -split "`n" | Where-Object { $_.Trim() -eq $ShellGets }).Count -gt 0
    }
    if (-not $ran) { $result = "fail: what was typed during the exit did not reach the shell whole`n$([ConsoleKeys]::Screen($pid_))"; return }
  }
  $result = 'pass'
}
finally {
  if ($pid_) { [ConsoleKeys]::Descendants($pid_) | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue } }
  Stop-Process -Id $conhost.Id -Force -ErrorAction SilentlyContinue
  if ($mock) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 500
  Remove-Item $wrapper, $marker -Force -ErrorAction SilentlyContinue
  Remove-Item $home_ -Recurse -Force -ErrorAction SilentlyContinue
  "RESULT $result"
}
