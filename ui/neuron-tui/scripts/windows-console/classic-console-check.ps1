# Runs `bun run dev:mock <Mode>` in a classic console window (conhost.exe, which
# Windows PowerShell 5.1 and cmd.exe open in), types $Keys, and checks that the
# window outlives the UI's exit and shows $Expect afterwards. Started by
# windows-console-check.ts. Keys: chunks split by `|`, `<CC>` is Ctrl-C, `<CR>`
# is Enter, `<SLEEP>` waits 4 s. Prints one `RESULT pass|fail ...` line.
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
  $after = [ConsoleKeys]::Screen($pid_)
  if ($Expect -and -not ($after -like "*$Expect*")) { $result = "fail: '$Expect' is not on the screen after the exit`n$after"; return }
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
