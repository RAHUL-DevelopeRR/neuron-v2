<#
.SYNOPSIS
  Build NeuronCLI artifacts for npm, PyPI, and Homebrew-style releases.

.DESCRIPTION
  Builds platform-specific Go binaries into neuron_cli/bin, creates release
  zip archives, runs npm pack, runs python -m build, and generates a Homebrew
  formula with local SHA256 values. Publishing is opt-in with -Publish.
#>

param(
    [switch]$SkipTests,
    [switch]$Publish,
    [int]$BuildTimeoutSeconds = 180,
    [string[]]$Targets = @("windows-amd64", "linux-amd64", "linux-arm64", "darwin-amd64", "darwin-arm64")
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$GoDir = Join-Path $Root "go"
$BinDir = Join-Path $Root "neuron_cli\bin"
$PackageDir = Join-Path $Root "dist\packages"
$FormulaDir = Join-Path $Root "packaging\homebrew"
$Version = (Get-Content (Join-Path $Root "package.json") | ConvertFrom-Json).version

New-Item -ItemType Directory -Force -Path $BinDir, $PackageDir, $FormulaDir | Out-Null

if (-not $SkipTests) {
    Push-Location $GoDir
    go test ./...
    Pop-Location

    Push-Location (Join-Path $Root "rust")
    cargo check --workspace
    Pop-Location
}

$AllTargets = @(
    @{ GOOS = "windows"; GOARCH = "amd64"; Ext = ".exe" },
    @{ GOOS = "linux";   GOARCH = "amd64"; Ext = "" },
    @{ GOOS = "linux";   GOARCH = "arm64"; Ext = "" },
    @{ GOOS = "darwin";  GOARCH = "amd64"; Ext = "" },
    @{ GOOS = "darwin";  GOARCH = "arm64"; Ext = "" }
)

$SelectedTargets = $AllTargets | Where-Object { $Targets -contains "$($_.GOOS)-$($_.GOARCH)" }
$Hashes = @{}

function Invoke-GoBuild {
    param(
        [hashtable]$Target,
        [string]$BinaryPath
    )

    $Job = Start-Job -ArgumentList $GoDir, $BinaryPath, $Target.GOOS, $Target.GOARCH -ScriptBlock {
        param($GoDir, $BinaryPath, $Goos, $Goarch)
        $env:GOOS = $Goos
        $env:GOARCH = $Goarch
        Set-Location $GoDir
        go build -trimpath -ldflags "-s -w" -o $BinaryPath .
        if ($LASTEXITCODE -ne 0) {
            throw "go build failed for $Goos-$Goarch"
        }
    }

    if (-not (Wait-Job $Job -Timeout $BuildTimeoutSeconds)) {
        Stop-Job $Job -ErrorAction SilentlyContinue
        Remove-Job $Job -Force -ErrorAction SilentlyContinue
        throw "go build timed out for $($Target.GOOS)-$($Target.GOARCH) after $BuildTimeoutSeconds seconds"
    }

    Receive-Job $Job
    if ($Job.State -ne "Completed") {
        Remove-Job $Job -Force -ErrorAction SilentlyContinue
        throw "go build failed for $($Target.GOOS)-$($Target.GOARCH)"
    }
    Remove-Job $Job -Force -ErrorAction SilentlyContinue
}

foreach ($Target in $SelectedTargets) {
    $BinaryName = "neuron-$($Target.GOOS)-$($Target.GOARCH)$($Target.Ext)"
    $BinaryPath = Join-Path $BinDir $BinaryName

    Invoke-GoBuild -Target $Target -BinaryPath $BinaryPath

    $ArchiveName = "neuroncli-$Version-$($Target.GOOS)-$($Target.GOARCH).zip"
    $ArchivePath = Join-Path $PackageDir $ArchiveName
    if (Test-Path $ArchivePath) {
        Remove-Item $ArchivePath -Force
    }
    Compress-Archive -Path $BinaryPath -DestinationPath $ArchivePath
    $Hashes[$ArchiveName] = (Get-FileHash $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
}

$Formula = @"
class Neuroncli < Formula
  desc "NeuronCLI AI coding agent"
  homepage "https://zero-x.live/neuroncli"
  version "$Version"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-darwin-arm64.zip"
      sha256 "$($Hashes["neuroncli-$Version-darwin-arm64.zip"])"
    else
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-darwin-amd64.zip"
      sha256 "$($Hashes["neuroncli-$Version-darwin-amd64.zip"])"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-linux-arm64.zip"
      sha256 "$($Hashes["neuroncli-$Version-linux-arm64.zip"])"
    else
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-linux-amd64.zip"
      sha256 "$($Hashes["neuroncli-$Version-linux-amd64.zip"])"
    end
  end

  def install
    binary = Dir["neuron-*"].first
    chmod 0755, binary
    bin.install binary => "neuron"
  end

  test do
    system "#{bin}/neuron", "--version"
  end
end
"@

Set-Content -Path (Join-Path $FormulaDir "neuroncli.rb") -Value $Formula -Encoding UTF8

Push-Location $Root
npm pack
python -m build
Pop-Location

if ($Publish) {
    Push-Location $Root
    npm publish --access public
    python -m twine upload "dist/*"
    Pop-Location
} else {
    Write-Host "Artifacts built in dist/packages."
    Write-Host "NPM package packed locally; PyPI distributions built in dist."
    Write-Host "Homebrew formula generated at packaging/homebrew/neuroncli.rb."
    Write-Host "Re-run with -Publish only from a clean release branch after uploading GitHub release zips."
}
