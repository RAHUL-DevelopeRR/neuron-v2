<#
.SYNOPSIS
  NeuronCLI — Windows installer

.DESCRIPTION
  Builds the Go binary, installs it to %LOCALAPPDATA%\Programs\NeuronCLI,
  and adds it to the User PATH. Safe to run repeatedly (idempotent).

.USAGE
  .\install.ps1              # Build and install (default)
  .\install.ps1 -SkipBuild   # Install existing binary without rebuilding
  .\install.ps1 -Uninstall   # Remove from PATH and delete install dir

.NOTES
  Requires: Go 1.22+ (for building)
  Installs to: %LOCALAPPDATA%\Programs\NeuronCLI\neuron.exe
  Adds to: User PATH (no admin rights needed)
#>

param(
    [switch]$SkipBuild,
    [switch]$Uninstall,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

# ── Config ───────────────────────────────────────────────────
$AppName     = "NeuronCLI"
$BinaryName  = "neuron.exe"
$InstallDir  = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$RepoRoot    = Split-Path -Parent $PSScriptRoot  # assumes scripts/install.ps1
$GoDir       = Join-Path $RepoRoot "go"

# ── Helpers ──────────────────────────────────────────────────

function Write-Step  { param([string]$msg) Write-Host "`n  [$script:step/5] $msg" -ForegroundColor Yellow; $script:step++ }
function Write-Ok    { param([string]$msg) Write-Host "    ✓ $msg" -ForegroundColor Green }
function Write-Info  { param([string]$msg) Write-Host "    → $msg" -ForegroundColor DarkGray }
function Write-Err   { param([string]$msg) Write-Host "    ✗ $msg" -ForegroundColor Red }

$script:step = 1

# ── Banner ───────────────────────────────────────────────────

Write-Host ""
Write-Host "  ╔══════════════════════════════════════════╗" -ForegroundColor DarkCyan
Write-Host "  ║     NeuronCLI Windows Installer          ║" -ForegroundColor Cyan
Write-Host "  ╚══════════════════════════════════════════╝" -ForegroundColor DarkCyan
Write-Host ""

if ($Help) {
    Get-Help $MyInvocation.MyCommand.Path -Detailed
    exit 0
}

# ── Uninstall ────────────────────────────────────────────────

if ($Uninstall) {
    Write-Host "  Uninstalling $AppName..." -ForegroundColor Yellow

    # Remove from PATH
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    $entries = ($userPath -split ";") | Where-Object { $_ -ne $InstallDir -and $_ -ne "" }
    [Environment]::SetEnvironmentVariable("PATH", ($entries -join ";"), "User")
    Write-Ok "Removed from User PATH"

    # Delete install directory
    if (Test-Path $InstallDir) {
        Remove-Item $InstallDir -Recurse -Force
        Write-Ok "Deleted $InstallDir"
    }

    Write-Host "`n  ✓ $AppName uninstalled.`n" -ForegroundColor Green
    exit 0
}

# ── Step 1: Check prerequisites ─────────────────────────────

Write-Step "Checking prerequisites"

if (-not $SkipBuild) {
    $goVersion = go version 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Go is not installed or not in PATH"
        Write-Info "Install Go from https://go.dev/dl/"
        exit 1
    }
    Write-Ok "Go found: $goVersion"
}

# ── Step 2: Remove legacy installations ─────────────────────

Write-Step "Removing legacy neuron installations"

$ErrorActionPreference = "Continue"  # Don't fail on cleanup errors

# 2a. Remove npm shims (these shadow .exe in PowerShell)
$npmDir = Join-Path $env:APPDATA "npm"
foreach ($ext in @("", ".cmd", ".ps1")) {
    $shim = Join-Path $npmDir "neuron$ext"
    if (Test-Path $shim) { Remove-Item $shim -Force; Write-Info "Removed npm shim: neuron$ext" }
}

# 2b. Remove npm package (neuron-cli-app)
$npmPkg = Join-Path $npmDir "node_modules\neuron-cli-app"
if (Test-Path $npmPkg) { Remove-Item $npmPkg -Recurse -Force; Write-Info "Removed npm package: neuron-cli-app" }

# 2c. Uninstall pip neuroncli package
$pipCheck = pip show neuroncli 2>$null
if ($LASTEXITCODE -eq 0) {
    pip uninstall neuroncli -y 2>&1 | Out-Null
    Write-Info "Uninstalled pip package: neuroncli"
}

# 2d. Remove old install directory (legacy "Neuron" with capital N)
$oldInstallDir = Join-Path $env:LOCALAPPDATA "Programs\Neuron"
if (Test-Path $oldInstallDir) {
    Remove-Item $oldInstallDir -Recurse -Force
    Write-Info "Removed old install dir: $oldInstallDir"
}

# 2e. Remove stale Python Scripts neuron.exe (pip entry point)
$pythonScriptsPattern = Join-Path $env:LOCALAPPDATA "Packages\*\LocalCache\local-packages\*\Scripts\neuron.exe"
Get-Item $pythonScriptsPattern -ErrorAction SilentlyContinue | ForEach-Object {
    Remove-Item $_.FullName -Force
    Write-Info "Removed pip shim: $($_.FullName)"
}

# 2f. Clean stale PATH entries (User PATH only — Machine PATH needs admin)
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
$cleanEntries = ($userPath -split ";") | Where-Object {
    $entry = $_
    $isStale = ($entry -ne "") -and ($entry -ne $InstallDir) -and (
        $entry -match "\\Neuron$" -or
        $entry -match "\\neuroncli$" -or
        $entry -match "\\neuroncli\\" -or
        $entry -match "\\Programs\\Neuron$"
    )
    if ($isStale) { Write-Info "Removed from PATH: $entry" }
    -not $isStale -and $entry -ne ""
}
[Environment]::SetEnvironmentVariable("PATH", ($cleanEntries -join ";"), "User")

$ErrorActionPreference = "Stop"
Write-Ok "Legacy cleanup complete"

# ── Step 3: Build ────────────────────────────────────────────

Write-Step "Building $BinaryName"

if ($SkipBuild) {
    $sourceBinary = Join-Path $GoDir $BinaryName
    if (-not (Test-Path $sourceBinary)) {
        Write-Err "No existing binary at $sourceBinary"
        Write-Info "Run without -SkipBuild to build first"
        exit 1
    }
    Write-Ok "Skipped build (using existing binary)"
} else {
    if (-not (Test-Path $GoDir)) {
        Write-Err "Go source directory not found: $GoDir"
        exit 1
    }

    Write-Info "Building in $GoDir ..."
    Push-Location $GoDir
    go build -o $BinaryName main.go 2>&1 | ForEach-Object { Write-Info $_ }
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw "Build failed" }
    Pop-Location
    $sourceBinary = Join-Path $GoDir $BinaryName
    Write-Ok "Built $sourceBinary"
}

# ── Step 4: Install to PATH ─────────────────────────────────

Write-Step "Installing to $InstallDir"

# Create install directory
if (!(Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Write-Info "Created $InstallDir"
}

# Copy binary
Copy-Item $sourceBinary (Join-Path $InstallDir $BinaryName) -Force
$size = [Math]::Round((Get-Item (Join-Path $InstallDir $BinaryName)).Length / 1MB, 1)
Write-Ok "Copied $BinaryName ($($size)MB)"

# Add to User PATH (idempotent)
$currentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($currentPath -split ";" | Where-Object { $_ -eq $InstallDir }) {
    Write-Ok "Already in User PATH"
} else {
    $newPath = $currentPath + ";" + $InstallDir
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
    Write-Ok "Added to User PATH"
}

# ── Step 5: Verify ───────────────────────────────────────────

Write-Step "Verifying installation"

$installed = Join-Path $InstallDir $BinaryName
$env:PATH = [Environment]::GetEnvironmentVariable("PATH", "Machine") + ";" + [Environment]::GetEnvironmentVariable("PATH", "User")

$resolvedPath = (Get-Command neuron.exe -ErrorAction SilentlyContinue).Source
if ($resolvedPath -eq $installed) {
    Write-Ok "neuron.exe resolves to $resolvedPath"
} elseif ($resolvedPath) {
    Write-Err "neuron.exe resolves to $resolvedPath (expected $installed)"
    Write-Info "Another neuron.exe may be shadowing — check Machine PATH"
} else {
    Write-Info "PATH change takes effect in new terminal windows"
}

# ── Done ─────────────────────────────────────────────────────

Write-Host ""
Write-Host "  ════════════════════════════════════════════" -ForegroundColor Green
Write-Host "  ✓ $AppName installed!" -ForegroundColor Green
Write-Host ""
Write-Host "    Location:  $InstallDir\$BinaryName" -ForegroundColor DarkGray
Write-Host "    Binary:    $($size)MB" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  Open a NEW terminal and run:" -ForegroundColor White
Write-Host "    neuron                          # interactive TUI" -ForegroundColor DarkGray
Write-Host "    neuron -p `"hello`"               # one-shot prompt" -ForegroundColor DarkGray
Write-Host "  ════════════════════════════════════════════" -ForegroundColor Green
Write-Host ""
