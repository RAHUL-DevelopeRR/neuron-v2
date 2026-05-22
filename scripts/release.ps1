<#
.SYNOPSIS
  NeuronCLI — One-command release script

.DESCRIPTION
  Bumps version, builds Rust, copies binary, publishes to npm + PyPI.

.USAGE
  .\scripts\release.ps1 patch     # 6.2.3 → 6.2.4
  .\scripts\release.ps1 minor     # 6.2.3 → 6.3.0
  .\scripts\release.ps1 7.0.0     # exact version
#>

param(
    [Parameter(Mandatory=$true)]
    [string]$BumpType
)

$ErrorActionPreference = "Stop"
$ROOT = Split-Path -Parent $PSScriptRoot

Write-Host "`n  ────────────────────────────────────────" -ForegroundColor DarkGray
Write-Host "  NeuronCLI Release Pipeline" -ForegroundColor Cyan
Write-Host "  ────────────────────────────────────────`n" -ForegroundColor DarkGray

# Step 1: Bump versions
Write-Host "  [1/5] Bumping version ($BumpType)..." -ForegroundColor Yellow
node "$ROOT\scripts\bump.mjs" $BumpType
if ($LASTEXITCODE -ne 0) { throw "Version bump failed" }

# Read the new version
$newVersion = (Get-Content "$ROOT\package.json" | ConvertFrom-Json).version
Write-Host "`n  Version: $newVersion`n" -ForegroundColor Green

# Step 2: Build Rust
Write-Host "  [2/5] Building Rust (release)..." -ForegroundColor Yellow
Push-Location "$ROOT\rust"
cargo build --release 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "Rust build failed" }
Pop-Location

# Step 3: Copy binary
Write-Host "  [3/5] Copying neuron.exe..." -ForegroundColor Yellow
Copy-Item "$ROOT\rust\target\release\neuron.exe" "$ROOT\neuron_cli\neuron.exe" -Force
Write-Host "    ✓ neuron_cli/neuron.exe updated" -ForegroundColor Green

# Step 4: Publish npm
Write-Host "  [4/5] Publishing to npm..." -ForegroundColor Yellow
Push-Location $ROOT
npm publish --access public 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "npm publish failed" }
Pop-Location
Write-Host "    ✓ neuron-cli-app@$newVersion published" -ForegroundColor Green

# Step 5: Publish PyPI
Write-Host "  [5/5] Publishing to PyPI..." -ForegroundColor Yellow
Push-Location $ROOT
Remove-Item "dist\*" -Force -ErrorAction SilentlyContinue
python -m build 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
python -m twine upload "dist\neuroncli-$newVersion*" 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "PyPI publish failed" }
Pop-Location
Write-Host "    ✓ neuroncli@$newVersion published" -ForegroundColor Green

# Done
Write-Host "`n  ════════════════════════════════════════" -ForegroundColor Green
Write-Host "  ✓ NeuronCLI v$newVersion released!" -ForegroundColor Green
Write-Host "    npm:  https://npmjs.com/package/neuron-cli-app" -ForegroundColor DarkGray
Write-Host "    PyPI: https://pypi.org/project/neuroncli/$newVersion/" -ForegroundColor DarkGray
Write-Host "  ════════════════════════════════════════`n" -ForegroundColor Green
