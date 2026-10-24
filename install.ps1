<#
.SYNOPSIS
    Installs the Neuron coding agent on Windows (x64).

.DESCRIPTION
    Downloads neuron-windows-x64.exe from GitHub Releases, verifies it against
    the release's SHA-256 file, and installs it as neuron.exe. Beside it goes
    neuron-tui.exe, the full-screen UI that neuron opens in a terminal; nothing
    about the UI fails the install (without it, neuron runs its line REPL).
    Works in Windows PowerShell 5.1 and PowerShell 7+.

    The latest stable release:

        irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1 | iex

    A given release; a prerelease such as 7.0.0-alpha.1 is never the latest release, so
    name it. To pass parameters when piping, create a script block:

        & ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.1/install.ps1))) -Version 7.0.0-alpha.1 -AddToPath

.PARAMETER Version
    Release to install, for example 7.0.0-alpha.1. Defaults to $env:NEURON_VERSION, then the latest stable release.

.PARAMETER InstallDir
    Install directory. Defaults to $env:NEURON_INSTALL_DIR, then %LOCALAPPDATA%\Programs\Neuron\bin.

.PARAMETER AddToPath
    Add the install directory to the user PATH. Without it, PATH is left unchanged.

.NOTES
    Environment: NEURON_VERSION, NEURON_INSTALL_DIR, NEURON_DOWNLOAD_BASE (HTTPS, or loopback HTTP
    for testing), NEURON_RELEASES_API, GITHUB_TOKEN, NEURON_NO_TUI (1 skips the full-screen UI).
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
$NeuronUiAsset = 'neuron-tui-windows-x64.exe'
$InvokedAsFile = $MyInvocation.MyCommand.CommandType -eq 'ExternalScript'

function Write-Step([string]$Message) { Write-Host $Message }
function Write-Detail([string]$Message) { Write-Host "  $Message" }
function Write-Warn([string]$Message) { Write-Host "warning: $Message" -ForegroundColor Yellow }

function Test-AllowedUrl([string]$Url) {
    # No userinfo: in http://127.0.0.1:x@evil.example/ the host contacted is evil.example.
    if ($Url -match '^https?://[^/]*@') { return $false }
    if ($Url -match '^https://') { return $true }
    return $Url -match '^http://(127\.0\.0\.1|localhost|\[::1\])(:\d+)?(/|$)'
}

function Test-GitHubApiUrl([string]$Url) {
    # GITHUB_TOKEN is only ever sent to the GitHub API, never to a mirror.
    $uri = $null
    if (-not [System.Uri]::TryCreate($Url, [System.UriKind]::Absolute, [ref]$uri)) { return $false }
    return ($uri.Scheme -eq 'https') -and ($uri.Host -eq 'api.github.com') -and (-not $uri.UserInfo)
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

# .NET directly instead of Get-FileHash: that cmdlet lives in an auto-loaded module and is missing
# when PSModulePath is unset or locked down (minimal environments, some CI and kiosk setups).
function Get-NeuronSha256([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash($stream)
    } finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
    return ([System.BitConverter]::ToString($hash) -replace '-', '').ToLowerInvariant()
}

function Get-PosixInstallCommand([string]$Requested) {
    # A named release gets its own install.sh and --version: a prerelease is never the latest release.
    $named = "$Requested".Trim()
    if ($named.StartsWith('v')) { $named = $named.Substring(1) }
    if ($named -match '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
        return "curl -fsSL https://github.com/$NeuronRepository/releases/download/v$named/install.sh | sh -s -- --version $named"
    }
    return "curl -fsSL https://github.com/$NeuronRepository/releases/latest/download/install.sh | sh"
}

function Assert-SupportedPlatform([string]$Requested) {
    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "install.ps1 is for Windows. On Linux and macOS run: $(Get-PosixInstallCommand $Requested)"
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
    if ($env:GITHUB_TOKEN -and (Test-GitHubApiUrl $api)) { $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)" }
    try {
        $release = Invoke-RestMethod -Uri $api -Headers $headers -UseBasicParsing
    } catch {
        throw "could not look up the latest Neuron release at ${api}: $($_.Exception.Message). Pass -Version 7.0.0-alpha.1 (or set NEURON_VERSION) to skip the lookup."
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

function Write-ShadowingNeuronWarning([string]$Destination) {
    # `neuron` runs the first neuron on PATH, which may be an older install or another program.
    $found = Get-Command neuron -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ((-not $found) -or ($found.Path -ieq $Destination)) { return }
    Write-Warn "another neuron at $($found.Path) comes first on PATH and runs instead of $Destination."
    Write-Detail 'An older Neuron is the usual cause. Remove it with the command for how it was installed:'
    Write-Detail '  an old NeuronCLI folder:  Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\NeuronCLI", then remove that folder from your user PATH'
    Write-Detail '  npm (6.x):                npm uninstall -g @zero-x.live/neuron neuron-cli-app'
    Write-Detail '  pip:                      pip uninstall neuroncli (or pipx uninstall neuroncli)'
    Write-Detail "If it is another program, move $(Split-Path -Parent $Destination) before $(Split-Path -Parent $found.Path) in PATH, or run neuron by its full path."
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

function Install-NeuronBinary([string]$Source, [string]$Destination) {
    # A running neuron.exe cannot be overwritten, but it can be renamed.
    $previous = "$Destination.old"
    $hadPrevious = Test-Path -LiteralPath $Destination
    if ($hadPrevious) {
        Remove-Item -LiteralPath $previous -Force -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $Destination -Destination $previous -Force
    }
    try {
        Move-Item -LiteralPath $Source -Destination $Destination -Force
    } catch {
        $reason = $_.Exception.Message
        if (-not $hadPrevious) { throw "could not install ${Destination}: $reason" }
        try {
            Move-Item -LiteralPath $previous -Destination $Destination -Force
        } catch {
            throw "could not install ${Destination}: $reason. The previous version is at $previous; rename it back to $(Split-Path -Leaf $Destination)."
        }
        throw "could not install ${Destination}: $reason. The previous version was kept."
    }
    Remove-Item -LiteralPath $previous -Force -ErrorAction SilentlyContinue
}

# The full-screen UI, verified and self-checked like neuron.exe. Any failure is a warning: neuron
# runs its line REPL without it.
function Install-NeuronUi([string]$Base, [string]$Version, [string]$Work, [string]$Target) {
    $url = "$Base/v$Version/$NeuronUiAsset"
    try {
        $checksumFile = Join-Path $Work "$NeuronUiAsset.sha256"
        Get-NeuronDownload "$url.sha256" $checksumFile $Version
        $checksumText = [System.IO.File]::ReadAllText($checksumFile).Trim()
        $expected = ''
        if ($checksumText) { $expected = ($checksumText -split '\s+')[0].ToLowerInvariant() }
        if ($expected -notmatch '^[0-9a-f]{64}$') { throw "$url.sha256 does not contain a SHA-256 checksum." }
        $download = Join-Path $Work $NeuronUiAsset
        Get-NeuronDownload $url $download $Version
        if ((Get-NeuronSha256 $download) -ne $expected) { throw "checksum mismatch for $NeuronUiAsset" }
        $preference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            & $download --version 2>&1 | Out-Null
            $exit = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $preference
        }
        if ($exit -ne 0) { throw "neuron-tui.exe does not run on this system (exit $exit)" }
        $destination = Join-Path $Target 'neuron-tui.exe'
        Install-NeuronBinary $download $destination
        Write-Detail "installed $destination"
    } catch {
        Write-Warn "the full-screen UI was not installed ($($_.Exception.Message)); neuron uses its line REPL. Rerun the installer to try again."
    }
}

function Get-ReportedVersion([string]$VersionOutput) {
    # The release binary prints a report whose second line is "  Version  X.Y.Z".
    $lines = @($VersionOutput -split "`r?`n" | Where-Object { $_.Trim() })
    foreach ($line in $lines) {
        if ($line -match '^\s*Version\s+(\S+)') { return "neuron $($Matches[1])" }
    }
    if ($lines.Count -gt 0) { return $lines[0].Trim() }
    return $null
}

function Install-Neuron {
    Set-StrictMode -Version 2.0
    $ErrorActionPreference = 'Stop'
    Assert-SupportedPlatform $Version
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
        throw "invalid version: $resolved (expected something like 7.0.0-alpha.1)"
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
        $actual = Get-NeuronSha256 $download
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
        Install-NeuronBinary $download $destination

        if (@('', '0', 'false', 'no') -contains "$env:NEURON_NO_TUI".Trim().ToLowerInvariant()) {
            Write-Detail 'installing the full-screen UI (neuron-tui.exe) ...'
            Install-NeuronUi $base $resolved $work $target
        } else {
            Write-Detail 'NEURON_NO_TUI is set; skipped the full-screen UI.'
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Installed $destination" -ForegroundColor Green
    $reported = Get-ReportedVersion $versionOutput
    if ($reported) { Write-Detail $reported }

    $run = 'neuron'
    if (Test-PathEntry $env:Path $target) {
        Write-ShadowingNeuronWarning $destination
    } elseif ($AddToPath) {
        if (Add-UserPathEntry $target) {
            Write-Detail "added $target to your user PATH; open a new terminal to use it."
        }
        # Appended, as in the user PATH: an older neuron earlier on PATH still wins.
        $env:Path = "$env:Path;$target"
        Write-ShadowingNeuronWarning $destination
    } else {
        Write-Warn "$target is not on your PATH."
        if ($InvokedAsFile) {
            Write-Detail 'Rerun with -AddToPath to add it to your user PATH, or run neuron by its full path.'
        } else {
            Write-Detail 'To add it to your user PATH, rerun the installer with -AddToPath:'
            Write-Detail "  & ([scriptblock]::Create((irm https://github.com/$NeuronRepository/releases/download/v$resolved/install.ps1))) -Version $resolved -AddToPath"
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
