<#
.SYNOPSIS
  NeuronCLI release wrapper.

.DESCRIPTION
  Bumps versions only when requested, then delegates to package-all.ps1.
  Publishing is opt-in and should normally be handled by GitHub Actions.
#>

param(
    [string]$BumpType = "",
    [switch]$SkipTests,
    [switch]$Publish,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot

if ($BumpType) {
    node (Join-Path $Root "scripts\bump.mjs") $BumpType
}

& (Join-Path $Root "scripts\package-all.ps1") `
    -SkipTests:$SkipTests `
    -Publish:$Publish `
    -Clean:$Clean
