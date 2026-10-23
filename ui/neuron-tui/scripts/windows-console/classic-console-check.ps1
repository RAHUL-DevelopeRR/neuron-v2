# Runs `bun run dev:mock <Mode>` in a classic console window (conhost.exe, which
# Windows PowerShell 5.1 and cmd.exe open in), types $Keys, and checks that the
# window outlives the UI's exit and shows $Expect afterwards, still there
# after the next line the console prints (a line that the shell's prompt
# would overwrite if the cursor had gone back up). Started by
# windows-console-check.ts. Keys: chunks split by `|`, `<CC>` is Ctrl-C, `<CR>`
# is Enter, `<SLEEP>` waits 4 s, `<KILLENGINE>` kills the UI's engine and waits
# for the UI to say it stopped. Prints one `RESULT pass|fail ...` line.
param(
  [Parameter(Mandatory)] [string]$Work,
  [Parameter(Mandatory)] [string]$Bun,
  [Parameter(Mandatory)] [string]$DevScript,
  [string]$Mode = '--compiled',
  [string]$Keys = '<CC>|<CC>',
  [string]$Expect = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ConsoleKeys.cs')

$tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$marker = Join-Path $Work "classic-$tag.exit"
$wrapper = Join-Path $Work "classic-$tag.cmd"
$home_ = Join-Path $Work "classic-$tag-home"
New-Item -ItemType Directory -Force (Join-Path $home_ 'config') | Out-Null
# The bundled MCP servers start in the background and only slow the run down.
Set-Content -Encoding ascii (Join-Path $home_ 'config\settings.json') '{"defaultMcpServers": false}'
Set-Content -Encoding ascii $wrapper @(
  '@echo off',
  "set `"NEURON_TUI_DEV_HOME=$home_`"",
  "`"$Bun`" `"$DevScript`" $Mode",
  "echo exit %errorlevel% > `"$marker`"",
  # What a shell prints next: it must land below the hint, not over it.
  'echo AFTER_THE_UI',
  # Keeps the window open, so a window that dies is conhost's doing.
  'ping -n 20 127.0.0.1 >nul'
)

function Descendants([int]$root) {
  $all = Get-CimInstance Win32_Process
  $found = @($root)
  do {
    $before = $found.Count
    $found = @($found + ($all | Where-Object { $found -contains [int]$_.ParentProcessId -and $found -notcontains [int]$_.ProcessId } | ForEach-Object { [int]$_.ProcessId }))
  } while ($found.Count -gt $before)
  $found
}

$conhost = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "cmd.exe /d /c `"$wrapper`"" -PassThru -WindowStyle Minimized
$result = $null
$cmd = $null
try {
  for ($i = 0; $i -lt 100 -and -not $cmd; $i++) {
    Start-Sleep -Milliseconds 200
    $cmd = Get-CimInstance Win32_Process -Filter "Name='cmd.exe'" | Where-Object { $_.CommandLine -like "*classic-$tag*" } | Select-Object -First 1
  }
  if (-not $cmd) { $result = 'fail: the console window did not start'; return }
  $pid_ = [uint32]$cmd.ProcessId
  $screen = ''
  for ($i = 0; $i -lt 180; $i++) {
    Start-Sleep -Milliseconds 500
    $screen = [ConsoleKeys]::Screen($pid_)
    if ($screen -and $screen -match 'Enter a coding task' -and $screen -match 'claude-opus') { break }
  }
  if (-not ($screen -match 'claude-opus')) { $result = "fail: the UI did not start`n$screen"; return }
  Start-Sleep -Milliseconds 800
  foreach ($chunk in ($Keys -split '\|')) {
    if ($chunk -eq '<SLEEP>') { Start-Sleep -Seconds 4; continue }
    if ($chunk -eq '<KILLENGINE>') {
      $engine = Get-CimInstance Win32_Process | Where-Object {
        (Descendants ([int]$cmd.ProcessId)) -contains [int]$_.ProcessId -and $_.CommandLine -like '*engine --stdio*'
      } | Select-Object -First 1
      if (-not $engine) { $result = 'fail: no engine process to kill'; return }
      Stop-Process -Id $engine.ProcessId -Force
      $stopped = $false
      for ($i = 0; $i -lt 40 -and -not $stopped; $i++) {
        Start-Sleep -Milliseconds 250
        $stopped = ([ConsoleKeys]::Screen($pid_)) -match 'engine stopped'
      }
      if (-not $stopped) { $result = "fail: the UI did not say the engine stopped`n$([ConsoleKeys]::Screen($pid_))"; return }
      Start-Sleep -Milliseconds 800
      continue
    }
    $text = $chunk -replace '<CC>', [string][char]3 -replace '<CR>', "`r"
    if (-not [ConsoleKeys]::Type($pid_, $text)) { $result = "fail: could not type [$chunk]"; return }
    Start-Sleep -Milliseconds 900
  }
  for ($i = 0; $i -lt 80 -and -not (Test-Path $marker); $i++) {
    Start-Sleep -Milliseconds 250
    if (-not (Get-Process -Id $conhost.Id -ErrorAction SilentlyContinue)) { break }
  }
  Start-Sleep -Milliseconds 500
  if (-not (Get-Process -Id $conhost.Id -ErrorAction SilentlyContinue)) {
    $result = 'fail: the console window (conhost.exe) died as the UI exited'
    return
  }
  if (-not (Test-Path $marker)) { $result = "fail: the UI did not exit`n$([ConsoleKeys]::Screen($pid_))"; return }
  for ($i = 0; $i -lt 20 -and -not (([ConsoleKeys]::Screen($pid_)) -match 'AFTER_THE_UI'); $i++) { Start-Sleep -Milliseconds 250 }
  $after = [ConsoleKeys]::Screen($pid_)
  if ($Expect) {
    $lines = @($after -split "`n")
    $hintAt = [Array]::FindIndex($lines, [Predicate[string]] { param($line) $line -like "*$Expect*" })
    $nextAt = [Array]::FindIndex($lines, [Predicate[string]] { param($line) $line.Trim() -eq 'AFTER_THE_UI' })
    if ($hintAt -lt 0) { $result = "fail: '$Expect' is not on the screen after the exit`n$after"; return }
    if ($nextAt -le $hintAt) { $result = "fail: the next line did not land below the hint (it was overwritten?)`n$after"; return }
  }
  $result = 'pass'
}
finally {
  if ($cmd) { Descendants ([int]$cmd.ProcessId) | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue } }
  Stop-Process -Id $conhost.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 500
  Remove-Item $wrapper, $marker -Force -ErrorAction SilentlyContinue
  Remove-Item $home_ -Recurse -Force -ErrorAction SilentlyContinue
  "RESULT $result"
}
