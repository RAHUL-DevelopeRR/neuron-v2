<#
.SYNOPSIS
    Installs the Neuron coding agent on Windows (x64).

.DESCRIPTION
    Downloads neuron-windows-x64.exe from GitHub Releases, verifies it against
    the release's SHA-256 file, and installs it as neuron.exe. Works in Windows
    PowerShell 5.1 and PowerShell 7+.

        irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1 | iex

    To pass parameters when piping, create a script block:

        & ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1))) -AddToPath

.PARAMETER Version
    Release to install, for example 6.3.0. Defaults to $env:NEURON_VERSION, then the latest release.

.PARAMETER InstallDir
    Install directory. Defaults to $env:NEURON_INSTALL_DIR, then %LOCALAPPDATA%\Programs\Neuron\bin.

.PARAMETER AddToPath
    Add the install directory to the user PATH. Without it, PATH is left unchanged.

.NOTES
    Environment: NEURON_VERSION, NEURON_INSTALL_DIR, NEURON_DOWNLOAD_BASE (HTTPS, or loopback HTTP
    for testing), NEURON_RELEASES_API, GITHUB_TOKEN.
#>
[CmdletBinding()]
param(
    [string]$Version = $env:NEURON_VERSION,
    [string]$InstallDir = $env:NEURON_INSTALL_DIR,
    [switch]$AddToPath
)

# Strict mode and 'Stop' are set inside Install-Neuron, not here: under `irm | iex`
# this scope is the caller's session, which must keep its own preferences.
$NeuronRepository = 'RAHUL-DevelopeRR/neuron-v2'
$NeuronAsset = 'neuron-windows-x64.exe'
$InvokedAsFile = $MyInvocation.MyCommand.CommandType -eq 'ExternalScript'

function Write-Step([string]$Message) { Write-Host $Message }
function Write-Detail([string]$Message) { Write-Host "  $Message" }
function Write-Warn([string]$Message) { Write-Host "warning: $Message" -ForegroundColor Yellow }

function Test-AllowedUrl([string]$Url) {
    if ($Url -match '^https://') { return $true }
    return $Url -match '^http://(127\.0\.0\.1|localhost|\[::1\])(:\d+)?(/|$)'
}

function Get-HttpStatus($ErrorRecord) {
    # WebException (Windows PowerShell 5.1) and HttpResponseException (PowerShell 7+) both expose Response.
    $exception = $ErrorRecord.Exception
    if (($exception.PSObject.Properties.Name -contains 'Response') -and $exception.Response) {
        try { return [int]$exception.Response.StatusCode } catch { return 0 }
    }
    return 0
}

function Get-NeuronDownload([string]$Url, [string]$OutFile, [string]$Version) {
    # Local to this function: hides the progress bar, which slows Invoke-WebRequest down in 5.1.
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        $status = Get-HttpStatus $_
        if ($status -eq 404) {
            throw "${Url} returned HTTP 404. Neuron $Version may not be published for Windows; see https://github.com/$NeuronRepository/releases"
        }
        if ($status -gt 0) { throw "downloading ${Url} failed with HTTP ${status}. Try again later." }
        throw "could not download ${Url}: $($_.Exception.Message). Check your network or proxy settings and retry."
    }
}

function Assert-SupportedPlatform {
    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "install.ps1 is for Windows. On Linux and macOS run: curl -fsSL https://github.com/$NeuronRepository/releases/latest/download/install.sh | sh"
    }
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { return }
        'ARM64' {
            Write-Warn 'Windows on ARM: installing the x64 build, which runs under Windows 11 x64 emulation.'
            return
        }
        default {
            throw "unsupported CPU architecture: $arch. Neuron publishes Windows binaries for x64 only."
        }
    }
}

function Resolve-NeuronVersion([string]$Requested) {
    if ($Requested) { return $Requested }
    $api = $env:NEURON_RELEASES_API
    if (-not $api) { $api = "https://api.github.com/repos/$NeuronRepository/releases/latest" }
    if (-not (Test-AllowedUrl $api)) { throw "NEURON_RELEASES_API must be an HTTPS URL (or loopback HTTP): $api" }
    $headers = @{ 'Accept' = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)" }
    try {
        $release = Invoke-RestMethod -Uri $api -Headers $headers -UseBasicParsing
    } catch {
        throw "could not look up the latest Neuron release at ${api}: $($_.Exception.Message). Pass -Version 6.3.0 (or set NEURON_VERSION) to skip the lookup."
    }
    $tag = $null
    if ($release -and ($release.PSObject.Properties.Name -contains 'tag_name')) { $tag = [string]$release.tag_name }
    if (-not $tag) { throw "the latest-release response from $api has no tag_name; pass -Version." }
    return $tag
}

function Test-PathEntry([string]$PathValue, [string]$Directory) {
    if (-not $PathValue) { return $false }
    $wanted = $Directory.TrimEnd('\')
    foreach ($entry in $PathValue.Split(';')) {
        if ($entry -and ($entry.Trim().TrimEnd('\') -ieq $wanted)) { return $true }
    }
    return $false
}

function Add-UserPathEntry([string]$Directory) {
    # Read the raw registry value so %VARIABLE% references in the user PATH survive.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if (Test-PathEntry $current $Directory) { return $false }
        $updated = if ($current) { $current.TrimEnd(';') + ';' + $Directory } else { $Directory }
        $key.SetValue('Path', $updated, [Microsoft.Win32.RegistryValueKind]::ExpandString)
    } finally {
        $key.Close()
    }
    # Setting a user variable through .NET broadcasts WM_SETTINGCHANGE so new terminals see the change.
    $marker = 'NEURON_INSTALL_PATH_REFRESH'
    [System.Environment]::SetEnvironmentVariable($marker, '1', 'User')
    [System.Environment]::SetEnvironmentVariable($marker, $null, 'User')
    return $true
}

function Install-Neuron {
    Set-StrictMode -Version 2.0
    $ErrorActionPreference = 'Stop'
    Assert-SupportedPlatform
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor [System.Net.SecurityProtocolType]::Tls12
    }

    $base = $env:NEURON_DOWNLOAD_BASE
    if (-not $base) { $base = "https://github.com/$NeuronRepository/releases/download" }
    $base = $base.TrimEnd('/')
    if (-not (Test-AllowedUrl $base)) { throw "NEURON_DOWNLOAD_BASE must be an HTTPS URL (or loopback HTTP): $base" }

    $resolved = (Resolve-NeuronVersion $Version).Trim()
    if ($resolved.StartsWith('v')) { $resolved = $resolved.Substring(1) }
    if ($resolved -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
        throw "invalid version: $resolved (expected something like 6.3.0)"
    }

    $target = $InstallDir
    if (-not $target) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set; pass -InstallDir.' }
        $target = Join-Path $env:LOCALAPPDATA 'Programs\Neuron\bin'
    }
    $target = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($target).TrimEnd('\')

    $url = "$base/v$resolved/$NeuronAsset"
    Write-Step "Installing Neuron $resolved (windows-x64)"
    Write-Detail "from $url"

    $work = Join-Path ([System.IO.Path]::GetTempPath()) ("neuron-install-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $checksumFile = Join-Path $work "$NeuronAsset.sha256"
        Get-NeuronDownload "$url.sha256" $checksumFile $resolved
        $checksumText = [System.IO.File]::ReadAllText($checksumFile).Trim()
        $expected = ''
        if ($checksumText) { $expected = ($checksumText -split '\s+')[0].ToLowerInvariant() }
        if ($expected -notmatch '^[0-9a-f]{64}$') { throw "$url.sha256 does not contain a SHA-256 checksum." }

        $download = Join-Path $work $NeuronAsset
        Get-NeuronDownload $url $download $resolved
        $actual = (Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "checksum mismatch for ${NeuronAsset}: expected $expected, got $actual. Nothing was installed."
        }
        Write-Detail "verified SHA-256 $actual"

        # Native stderr must not become a terminating error under 'Stop' in Windows PowerShell 5.1.
        $preference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $versionOutput = & $download --version 2>&1 | Out-String
            $versionExit = $LASTEXITCODE
        } catch {
            throw "the downloaded binary could not start: $($_.Exception.Message). Nothing was installed."
        } finally {
            $ErrorActionPreference = $preference
        }
        if ($versionExit -ne 0) {
            throw "the downloaded binary does not run on this system (exit $versionExit): $($versionOutput.Trim()). Nothing was installed."
        }

        New-Item -ItemType Directory -Path $target -Force | Out-Null
        $destination = Join-Path $target 'neuron.exe'
        if (Test-Path -LiteralPath $destination) {
            # A running neuron.exe cannot be overwritten, but it can be renamed.
            $previous = "$destination.old"
            Remove-Item -LiteralPath $previous -Force -ErrorAction SilentlyContinue
            Move-Item -LiteralPath $destination -Destination $previous -Force
        }
        Move-Item -LiteralPath $download -Destination $destination -Force
        Remove-Item -LiteralPath "$destination.old" -Force -ErrorAction SilentlyContinue
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Installed $destination" -ForegroundColor Green
    $firstLine = ($versionOutput -split "`r?`n" | Where-Object { $_ } | Select-Object -First 1)
    if ($firstLine) { Write-Detail $firstLine }

    $run = 'neuron'
    if (Test-PathEntry $env:Path $target) {
        $found = Get-Command neuron -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found -and ($found.Path -ine $destination)) {
            Write-Warn "another neuron at $($found.Path) comes first on PATH and runs instead of $destination."
            Write-Detail 'Remove it (for example: pip uninstall neuroncli, or npm uninstall -g @zero-x.live/neuron) or reorder PATH.'
        }
    } elseif ($AddToPath) {
        if (Add-UserPathEntry $target) {
            Write-Detail "added $target to your user PATH; open a new terminal to use it."
        }
        $env:Path = "$env:Path;$target"
    } else {
        Write-Warn "$target is not on your PATH."
        if ($InvokedAsFile) {
            Write-Detail 'Rerun with -AddToPath to add it to your user PATH, or run neuron by its full path.'
        } else {
            Write-Detail 'To add it to your user PATH, rerun the installer with -AddToPath:'
            Write-Detail "  & ([scriptblock]::Create((irm https://github.com/$NeuronRepository/releases/latest/download/install.ps1))) -AddToPath"
        }
        $run = "& '$destination'"
    }

    Write-Host ''
    Write-Host 'Next steps:'
    Write-Host "  $run auth login     # sign in to Zero-X (a free plan is available)"
    Write-Host "  $run                # start a session in your project directory"
    Write-Host ''
    Write-Host "Docs: https://github.com/$NeuronRepository#readme"
}

try {
    Install-Neuron
} catch {
    Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
    if ($InvokedAsFile) { exit 1 }
    throw
}
