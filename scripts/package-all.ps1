<#
.SYNOPSIS
  Build NeuronCLI release artifacts for all supported package managers.

.DESCRIPTION
  The Go runtime is the canonical shipped binary. This script builds native
  OS/arch binaries, release archives, checksums, npm platform packages, the
  main npm launcher package, PyPI distributions, a Homebrew formula, and
  Linux deb/rpm packages when nfpm is installed.
#>

param(
    [switch]$SkipTests,
    [switch]$Publish,
    [switch]$Clean,
    [int]$BuildTimeoutSeconds = 300,
    [string[]]$Targets = @("windows-amd64", "linux-amd64", "linux-arm64", "darwin-amd64", "darwin-arm64")
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$GoDir = Join-Path $Root "go"
$ReleaseDir = Join-Path $Root "dist\release"
$NpmDir = Join-Path $Root "dist\npm"
$PyPiDir = Join-Path $Root "dist\pypi"
$NfpmWorkDir = Join-Path $Root "dist\nfpm"
$FormulaDir = Join-Path $Root "packaging\homebrew"
$Version = (Get-Content (Join-Path $Root "package.json") | ConvertFrom-Json).version
$Commit = (git -C $Root rev-parse --short HEAD 2>$null)
if (-not $Commit) { $Commit = "unknown" }
$BuildDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

if ($Clean) {
    Remove-Item (Join-Path $Root "dist") -Recurse -Force -ErrorAction SilentlyContinue
}

New-Item -ItemType Directory -Force -Path $ReleaseDir, $NpmDir, $PyPiDir, $NfpmWorkDir, $FormulaDir | Out-Null

if (-not $SkipTests) {
    Push-Location $GoDir
    go test ./...
    Pop-Location

    node --check (Join-Path $Root "bin\cli.js")
}

$AllTargets = @(
    @{ Key = "windows-amd64"; GOOS = "windows"; GOARCH = "amd64"; Npm = "win32-x64"; Ext = ".exe"; Archive = "zip"; NfpmArch = "" },
    @{ Key = "linux-amd64";   GOOS = "linux";   GOARCH = "amd64"; Npm = "linux-x64"; Ext = ""; Archive = "tar.gz"; NfpmArch = "amd64" },
    @{ Key = "linux-arm64";   GOOS = "linux";   GOARCH = "arm64"; Npm = "linux-arm64"; Ext = ""; Archive = "tar.gz"; NfpmArch = "arm64" },
    @{ Key = "darwin-amd64";  GOOS = "darwin";  GOARCH = "amd64"; Npm = "darwin-x64"; Ext = ""; Archive = "tar.gz"; NfpmArch = "" },
    @{ Key = "darwin-arm64";  GOOS = "darwin";  GOARCH = "arm64"; Npm = "darwin-arm64"; Ext = ""; Archive = "tar.gz"; NfpmArch = "" }
)

$SelectedTargets = $AllTargets | Where-Object { $Targets -contains $_.Key }
$Built = @{}

function Invoke-GoBuild {
    param(
        [hashtable]$Target,
        [string]$BinaryPath
    )

    $ldflags = "-s -w -X github.com/opencode-ai/opencode/internal/version.Version=$Version -X github.com/opencode-ai/opencode/internal/version.Commit=$Commit -X github.com/opencode-ai/opencode/internal/version.Date=$BuildDate"
    $job = Start-Job -ArgumentList $GoDir, $BinaryPath, $Target.GOOS, $Target.GOARCH, $ldflags -ScriptBlock {
        param($GoDir, $BinaryPath, $Goos, $Goarch, $Ldflags)
        $env:GOOS = $Goos
        $env:GOARCH = $Goarch
        go build -C $GoDir -trimpath -ldflags $Ldflags -o $BinaryPath .
        if ($LASTEXITCODE -ne 0) {
            throw "go build failed for $Goos-$Goarch"
        }
    }

    if (-not (Wait-Job $job -Timeout $BuildTimeoutSeconds)) {
        Stop-Job $job -ErrorAction SilentlyContinue
        Remove-Job $job -Force -ErrorAction SilentlyContinue
        throw "go build timed out for $($Target.Key)"
    }

    Receive-Job $job
    if ($job.State -ne "Completed") {
        Remove-Job $job -Force -ErrorAction SilentlyContinue
        throw "go build failed for $($Target.Key)"
    }
    Remove-Job $job -Force -ErrorAction SilentlyContinue
}

function New-Archive {
    param(
        [hashtable]$Target,
        [string]$BinaryPath
    )

    $staging = Join-Path $ReleaseDir "stage-$($Target.Key)"
    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    $installedName = if ($Target.GOOS -eq "windows") { "neuron.exe" } else { "neuron" }
    Copy-Item $BinaryPath (Join-Path $staging $installedName) -Force

    $archiveName = "neuroncli-$Version-$($Target.Key).$($Target.Archive)"
    $archivePath = Join-Path $ReleaseDir $archiveName
    Remove-Item $archivePath -Force -ErrorAction SilentlyContinue

    if ($Target.Archive -eq "zip") {
        Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archivePath -Force
    } else {
        tar -czf $archivePath -C $staging .
    }

    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
    return $archivePath
}

function New-NpmPlatformPackage {
    param(
        [hashtable]$Target,
        [string]$BinaryPath
    )

    $source = Join-Path $Root "packaging\npm\platforms\$($Target.Npm)"
    if (-not (Test-Path $source)) {
        Write-Warning "Missing npm platform package template: $source"
        return
    }

    $pkgDir = Join-Path $Root "dist\npm-work\$($Target.Npm)"
    Remove-Item $pkgDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path (Join-Path $pkgDir "bin") | Out-Null
    Copy-Item (Join-Path $source "package.json") (Join-Path $pkgDir "package.json") -Force
    if (Test-Path (Join-Path $Root "README.md")) {
        Copy-Item (Join-Path $Root "README.md") (Join-Path $pkgDir "README.md") -Force
    }
    if (Test-Path (Join-Path $Root "LICENSE")) {
        Copy-Item (Join-Path $Root "LICENSE") (Join-Path $pkgDir "LICENSE") -Force
    }
    $installedName = if ($Target.GOOS -eq "windows") { "neuron.exe" } else { "neuron" }
    Copy-Item $BinaryPath (Join-Path $pkgDir "bin\$installedName") -Force

    Push-Location $pkgDir
    npm pack --pack-destination $NpmDir | Out-Host
    Pop-Location
}

function New-NfpmPackage {
    param(
        [hashtable]$Target,
        [string]$BinaryPath,
        [string]$Packager
    )

    if (-not $Target.NfpmArch) { return }
    if (-not (Get-Command nfpm -ErrorAction SilentlyContinue)) {
        Write-Warning "nfpm not found; skipping $Packager for $($Target.Key)"
        return
    }

    $configPath = Join-Path $NfpmWorkDir "nfpm-$($Target.Key)-$Packager.yaml"
    $src = $BinaryPath.Replace("\", "/")
    $config = @"
name: neuroncli
arch: $($Target.NfpmArch)
platform: linux
version: "$Version"
section: default
priority: optional
maintainer: "RAHUL-DevelopeRR <rahultech72216@gmail.com>"
description: "NeuronCLI AI coding agent"
vendor: "zero-x.live"
homepage: "https://zero-x.live/neuroncli"
license: "MIT"
contents:
  - src: "$src"
    dst: /usr/bin/neuron
"@
    Set-Content -Path $configPath -Value $config -Encoding UTF8
    nfpm package --packager $Packager --config $configPath --target $ReleaseDir
}

foreach ($target in $SelectedTargets) {
    $binaryName = "neuron-$($target.GOOS)-$($target.GOARCH)$($target.Ext)"
    $binaryPath = Join-Path $ReleaseDir $binaryName
    Invoke-GoBuild -Target $target -BinaryPath $binaryPath
    $archivePath = New-Archive -Target $target -BinaryPath $binaryPath
    New-NpmPlatformPackage -Target $target -BinaryPath $binaryPath
    New-NfpmPackage -Target $target -BinaryPath $binaryPath -Packager "deb"
    New-NfpmPackage -Target $target -BinaryPath $binaryPath -Packager "rpm"
    $Built[$target.Key] = @{
        Binary = $binaryPath
        Archive = $archivePath
    }
}

if ($Built.ContainsKey("windows-amd64") -and (Get-Command wix -ErrorAction SilentlyContinue)) {
    $msiPath = Join-Path $ReleaseDir "neuroncli-$Version-windows-amd64.msi"
    wix build (Join-Path $Root "packaging\windows\neuron.wxs") `
        -d "Version=$Version" `
        -d "BinaryPath=$($Built["windows-amd64"].Binary)" `
        -out $msiPath
} elseif ($Built.ContainsKey("windows-amd64")) {
    Write-Warning "WiX CLI not found; skipping Windows .msi. Install with: dotnet tool install --global wix"
}

$darwinArm = Join-Path $ReleaseDir "neuroncli-$Version-darwin-arm64.tar.gz"
$darwinX64 = Join-Path $ReleaseDir "neuroncli-$Version-darwin-amd64.tar.gz"
$linuxArm = Join-Path $ReleaseDir "neuroncli-$Version-linux-arm64.tar.gz"
$linuxX64 = Join-Path $ReleaseDir "neuroncli-$Version-linux-amd64.tar.gz"

$formula = @"
class Neuroncli < Formula
  desc "NeuronCLI AI coding agent"
  homepage "https://zero-x.live/neuroncli"
  version "$Version"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-darwin-arm64.tar.gz"
      sha256 "$(if (Test-Path $darwinArm) { (Get-FileHash $darwinArm -Algorithm SHA256).Hash.ToLowerInvariant() })"
    else
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-darwin-amd64.tar.gz"
      sha256 "$(if (Test-Path $darwinX64) { (Get-FileHash $darwinX64 -Algorithm SHA256).Hash.ToLowerInvariant() })"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-linux-arm64.tar.gz"
      sha256 "$(if (Test-Path $linuxArm) { (Get-FileHash $linuxArm -Algorithm SHA256).Hash.ToLowerInvariant() })"
    else
      url "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v#{version}/neuroncli-#{version}-linux-amd64.tar.gz"
      sha256 "$(if (Test-Path $linuxX64) { (Get-FileHash $linuxX64 -Algorithm SHA256).Hash.ToLowerInvariant() })"
    end
  end

  def install
    bin.install "neuron"
  end

  test do
    system "#{bin}/neuron", "--version"
  end
end
"@
Set-Content -Path (Join-Path $FormulaDir "neuroncli.rb") -Value $formula -Encoding UTF8

Push-Location $Root
npm pack --pack-destination $NpmDir | Out-Host
python -m build --outdir $PyPiDir
Pop-Location

$checksumPath = Join-Path $ReleaseDir "checksums.txt"
Get-ChildItem $ReleaseDir -File |
    Where-Object { $_.Name -ne "checksums.txt" -and $_.Extension -ne ".yaml" } |
    Sort-Object Name |
    ForEach-Object {
        "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $($_.Name)"
    } | Set-Content -Path $checksumPath -Encoding ASCII

if ($Publish) {
    Get-ChildItem $NpmDir -Filter "zero-x-neuron-*-*.tgz" | Sort-Object Name | ForEach-Object {
        npm publish $_.FullName --access public
    }
    Get-ChildItem $NpmDir -Filter "zero-x-neuron-[0-9]*.tgz" | Sort-Object Name | ForEach-Object {
        npm publish $_.FullName --access public
    }
    Get-ChildItem $PyPiDir | ForEach-Object {
        python -m twine upload $_.FullName
    }
} else {
    Write-Host "Release assets: $ReleaseDir"
    Write-Host "npm tarballs:   $NpmDir"
    Write-Host "PyPI dists:     $PyPiDir"
    Write-Host "Homebrew:       $(Join-Path $FormulaDir "neuroncli.rb")"
    Write-Host "Use -Publish only from a clean tagged release after GitHub assets are uploaded."
}
