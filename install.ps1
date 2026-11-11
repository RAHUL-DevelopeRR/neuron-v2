<#
.SYNOPSIS
    Installs the Neuron coding agent on Windows (x64).

.DESCRIPTION
    Downloads neuron-windows-x64.exe from GitHub Releases, verifies it against
    the release's SHA-256 file and SHA256SUMS, and installs it as neuron.exe.
    neuron.exe is installed (and usable) first; then comes neuron-tui.exe, the
    full-screen UI that neuron opens in a terminal. Nothing about the UI fails the
    install (without it, neuron runs its line REPL). Downloads show their progress.
    Works in Windows PowerShell 5.1 and PowerShell 7+, including under the default
    "Restricted" execution policy: everything it installs is an .exe, never a script.

    Install the release this script was published with and add it to your user PATH, with
    <version> a release from 7.0.0-alpha.6 on (the install.ps1 of 7.0.0-alpha.5 and earlier
    needs the script block form below, with -Version and -AddToPath):

        irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v<version>/install.ps1 | iex

    From cmd.exe:

        powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v<version>/install.ps1 | iex"

    To pass parameters when piping, create a script block:

        & ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v<version>/install.ps1))) -Version <version> -NoPathChange

.PARAMETER Version
    Release to install, for example 7.0.0-alpha.5. Defaults to $env:NEURON_VERSION, then the
    release this script was published with, then the newest release on GitHub (prereleases
    included: every Neuron 7 release so far is a prerelease, which GitHub never marks "latest").

.PARAMETER InstallDir
    Install directory. Defaults to $env:NEURON_INSTALL_DIR, then %LOCALAPPDATA%\Programs\Neuron\bin.

.PARAMETER NoPathChange
    Leave the user PATH alone (so does NEURON_NO_MODIFY_PATH=1). By default the install
    directory is appended to the user PATH.

.PARAMETER AddToPath
    Accepted for older install commands; adding to PATH is now the default.

.NOTES
    Environment: NEURON_VERSION, NEURON_INSTALL_DIR, NEURON_NO_MODIFY_PATH, NEURON_DOWNLOAD_BASE
    (HTTPS, or loopback HTTP for testing), NEURON_CHECKSUMS_BASE (where SHA256SUMS comes from;
    default the GitHub release), NEURON_RELEASES_API, GITHUB_TOKEN, NEURON_NO_TUI (1 skips the
    full-screen UI), NEURON_DOWNLOADER (curl or dotnet; default curl.exe when Windows has it).
#>
[CmdletBinding()]
param(
    [string]$Version = $env:NEURON_VERSION,
    [string]$InstallDir = $env:NEURON_INSTALL_DIR,
    [switch]$NoPathChange,
    [switch]$AddToPath
)

# Strict mode and 'Stop' are set inside Install-Neuron, not here: under `irm | iex`
# this scope is the caller's session, which must keep its own preferences.
$NeuronRepository = 'RAHUL-DevelopeRR/neuron-v2'
$NeuronAsset = 'neuron-windows-x64.exe'
$NeuronUiAsset = 'neuron-tui-windows-x64.exe'
# The release workflow writes the release's version here, so `irm <release>/install.ps1 | iex`
# installs that release without a lookup (the GitHub API allows 60 anonymous calls an hour).
$NeuronReleaseVersion = ''
$InvokedAsFile = $MyInvocation.MyCommand.CommandType -eq 'ExternalScript'

function Write-Step([string]$Message) { Write-Host $Message }
function Write-Detail([string]$Message) { Write-Host "  $Message" }
function Write-Warn([string]$Message) { Write-Host "warning: $Message" -ForegroundColor Yellow }

function Test-Truthy([string]$Value) {
    return -not (@('', '0', 'false', 'no') -contains "$Value".Trim().ToLowerInvariant())
}

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

function Format-Megabytes([double]$Bytes) {
    return ('{0:0.0} MB' -f ($Bytes / 1MB))
}

function Test-InteractiveConsole {
    try { return ($Host.Name -eq 'ConsoleHost') -and (-not [System.Console]::IsOutputRedirected) } catch { return $false }
}

# Progress for one download: a line redrawn in place twice a second on a console, or a line
# every 10 seconds when the output is a log. Only the binaries get one; small files are silent.
function New-DownloadProgress([string]$Label) {
    if (-not $Label) { return $null }
    $interactive = Test-InteractiveConsole
    $first = 10.0
    if ($interactive) { $first = 0.0 }
    return @{ Label = $Label; Clock = [System.Diagnostics.Stopwatch]::StartNew(); Next = $first; Interactive = $interactive; Width = 0 }
}

function Update-DownloadProgress($State, [long]$Done, [long]$Total, [bool]$Final = $false) {
    if (-not $State) { return }
    $seconds = $State.Clock.Elapsed.TotalSeconds
    if ((-not $Final) -and ($seconds -lt $State.Next)) { return }
    if ($State.Interactive) { $State.Next = $seconds + 0.5 } else { $State.Next = $seconds + 10 }
    $rate = 0.0
    if ($seconds -gt 0.2) { $rate = $Done / $seconds }
    if ($Final) {
        $text = '{0}: {1} in {2:0} s ({3}/s)' -f $State.Label, (Format-Megabytes $Done), $seconds, (Format-Megabytes $rate)
    } elseif ($Total -gt 0) {
        $percent = [math]::Floor(100 * $Done / $Total)
        $left = ''
        if ($rate -gt 0) { $left = ', {0:0} s left' -f (($Total - $Done) / $rate) }
        $text = '{0}: {1} of {2} ({3}%), {4}/s{5}' -f $State.Label, (Format-Megabytes $Done), (Format-Megabytes $Total), $percent, (Format-Megabytes $rate), $left
    } else {
        $text = '{0}: {1}, {2}/s' -f $State.Label, (Format-Megabytes $Done), (Format-Megabytes $rate)
    }
    if ($State.Interactive) {
        Write-Host -NoNewline ("`r  " + $text.PadRight($State.Width))
        $State.Width = $text.Length
        if ($Final) { Write-Host '' }
    } else {
        Write-Host "  $text"
    }
}

function Get-FileLength([string]$Path) {
    try { return (New-Object System.IO.FileInfo $Path).Length } catch { return 0 }
}

# The Content-Length of the final response in a curl --dump-header file (it holds every redirect hop).
function Get-DumpedContentLength([string]$Path) {
    try {
        $stream = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
        try { $text = (New-Object System.IO.StreamReader $stream).ReadToEnd() } finally { $stream.Dispose() }
    } catch {
        return -1
    }
    $length = -1
    foreach ($line in ($text -split "`r?`n")) {
        if ($line -match '^HTTP/') { $length = -1 }
        elseif ($line -match '^content-length:\s*(\d+)') { $length = [long]$Matches[1] }
    }
    return $length
}

function Get-NeuronCurl {
    # curl.exe ships with Windows 10 1803 and later. The System32 copy, not whatever `curl` means
    # on PATH: in Windows PowerShell `curl` is an alias for Invoke-WebRequest.
    if ($env:NEURON_DOWNLOADER -eq 'dotnet') { return $null }
    if (-not $env:SystemRoot) { return $null }
    $curl = Join-Path $env:SystemRoot 'System32\curl.exe'
    if (Test-Path -LiteralPath $curl -PathType Leaf) { return $curl }
    return $null
}

# The proxy Windows itself uses for $Url (Internet Options, a PAC script or WPAD), or '' for a
# direct connection. curl.exe reads only HTTPS_PROXY and friends, so without this it would try a
# direct connection that a proxy-only network drops.
function Get-NeuronSystemProxy([string]$Url) {
    foreach ($name in @('HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'all_proxy')) {
        if ([System.Environment]::GetEnvironmentVariable($name)) { return '' }
    }
    try {
        $uri = New-Object System.Uri $Url
        if ($uri.IsLoopback) { return '' }
        $proxy = [System.Net.WebRequest]::GetSystemWebProxy()
        if ((-not $proxy) -or $proxy.IsBypassed($uri)) { return '' }
        $address = $proxy.GetProxy($uri)
        if ((-not $address) -or ($address.AbsoluteUri -eq $uri.AbsoluteUri)) { return '' }
        if (@('http', 'https', 'socks4', 'socks5') -notcontains $address.Scheme) { return '' }
        return $address.AbsoluteUri
    } catch {
        return ''
    }
}

# Returns @{ Ok; Status (HTTP status, 0 when the server was never reached); Message }.
function Invoke-CurlDownload([string]$Curl, [string]$Url, [string]$OutFile, [string]$Label) {
    $headers = "$OutFile.headers"
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Curl
    # --retry covers timeouts and 408/429/5xx; --speed-limit/--speed-time turn a stalled
    # connection (under 1 KB/s for a minute) into a timeout, which is retried too. A 10 s
    # connect timeout keeps a network curl cannot reach to under a minute before .NET takes over.
    # With a Windows proxy, "-U :" and --proxy-anyauth let curl answer a proxy that asks for
    # Windows sign-in (NTLM or Negotiate) with the current user's credentials.
    $proxyArgument = ''
    $proxy = Get-NeuronSystemProxy $Url
    if ($proxy) { $proxyArgument = '--proxy "{0}" --proxy-anyauth --proxy-user : ' -f $proxy }
    $info.Arguments = ('--fail --location --silent --show-error --retry 3 --retry-delay 2 --connect-timeout 10 ' +
        '--speed-limit 1024 --speed-time 60 --proto-redir =https {3}--dump-header "{0}" --output "{1}" ' +
        '--write-out "%{{http_code}}" "{2}"') -f $headers, $OutFile, $Url, $proxyArgument
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    try {
        $process = [System.Diagnostics.Process]::Start($info)
    } catch {
        # For example "This program is blocked by group policy" (AppLocker or WDAC): .NET takes over.
        $inner = $_.Exception
        while ($inner.InnerException) { $inner = $inner.InnerException }
        return @{ Ok = $false; Status = 0; Message = "curl.exe could not start: $($inner.Message)" }
    }
    $progress = New-DownloadProgress $Label
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $total = -1
        while (-not $process.WaitForExit(250)) {
            if (-not $progress) { continue }
            $done = Get-FileLength $OutFile
            if (($total -lt 0) -and ($done -gt 0)) { $total = Get-DumpedContentLength $headers }
            Update-DownloadProgress $progress $done $total
        }
        $process.WaitForExit()
        $code = $process.ExitCode
        $http = 0
        [void][int]::TryParse(($stdout.Result + '').Trim(), [ref]$http)
        $message = ($stderr.Result + '').Trim()
    } finally {
        if (-not $process.HasExited) {
            try { $process.Kill(); [void]$process.WaitForExit(5000) } catch { }
        }
        $process.Dispose()
        Remove-Item -LiteralPath $headers -Force -ErrorAction SilentlyContinue
    }
    # Exit 0 alone is not trusted: curl 8.14 with --retry exits 0 after a refused connection.
    # The final response's HTTP status (000 when there was none) must be a success as well.
    if (($code -eq 0) -and ($http -ge 200) -and ($http -lt 300) -and (Test-Path -LiteralPath $OutFile -PathType Leaf)) {
        Update-DownloadProgress $progress (Get-FileLength $OutFile) 0 $true
        return @{ Ok = $true; Status = $http; Message = '' }
    }
    if ($progress -and $progress.Interactive -and $progress.Width) { Write-Host '' }
    if ($http -ge 400) { return @{ Ok = $false; Status = $http; Message = "HTTP $http" } }
    if (-not $message) { $message = "curl.exe exit $code" }
    return @{ Ok = $false; Status = 0; Message = $message }
}

# The fallback when curl.exe is missing or cannot connect (for example behind a proxy that only
# the Windows settings name: curl.exe reads HTTPS_PROXY, .NET also reads the system proxy).
function Invoke-DotNetDownload([string]$Url, [string]$OutFile, [string]$Label) {
    if ($PSVersionTable.PSVersion.Major -lt 6) { Add-Type -AssemblyName System.Net.Http }
    $client = New-Object System.Net.Http.HttpClient
    $client.Timeout = [System.TimeSpan]::FromMinutes(30)
    $response = $null
    try {
        try {
            $response = $client.GetAsync($Url, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        } catch {
            $inner = $_.Exception
            while ($inner.InnerException) { $inner = $inner.InnerException }
            return @{ Ok = $false; Status = 0; Message = $inner.Message }
        }
        if (-not $response.IsSuccessStatusCode) {
            $status = [int]$response.StatusCode
            return @{ Ok = $false; Status = $status; Message = "HTTP $status" }
        }
        $total = -1
        if ($null -ne $response.Content.Headers.ContentLength) { $total = [long]$response.Content.Headers.ContentLength }
        $progress = New-DownloadProgress $Label
        $done = 0
        try {
            $source = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $target = [System.IO.File]::Create($OutFile)
            try {
                $buffer = New-Object byte[] (1MB)
                while (($read = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $target.Write($buffer, 0, $read)
                    $done += $read
                    Update-DownloadProgress $progress $done $total
                }
            } finally {
                $target.Dispose()
                $source.Dispose()
            }
        } catch {
            if ($progress -and $progress.Interactive -and $progress.Width) { Write-Host '' }
            $inner = $_.Exception
            while ($inner.InnerException) { $inner = $inner.InnerException }
            return @{ Ok = $false; Status = 0; Message = "the download was interrupted: $($inner.Message)" }
        }
        Update-DownloadProgress $progress $done 0 $true
        return @{ Ok = $true; Status = 200; Message = '' }
    } finally {
        if ($response) { $response.Dispose() }
        $client.Dispose()
    }
}

# Downloads with curl.exe when Windows has it (fast, retries, resumes nothing it cannot verify),
# else with .NET. Either way the result is only trusted after the SHA-256 checks. Once curl.exe
# fails without reaching the server, the remaining downloads of this install go straight to .NET,
# so a network that curl cannot use costs that wait once, not once per file.
# $NeuronDownloadState comes from Install-Neuron; a test that calls this function alone has none.
function Get-NeuronDownload([string]$Url, [string]$OutFile, [string]$Version, [string]$Label = '') {
    $state = $null
    if (Test-Path variable:NeuronDownloadState) { $state = $NeuronDownloadState }
    $result = $null
    $curl = Get-NeuronCurl
    if ($curl -and -not ($state -and $state.DotNetOnly)) { $result = Invoke-CurlDownload $curl $Url $OutFile $Label }
    if ((-not $result) -or ((-not $result.Ok) -and ($result.Status -eq 0) -and ($env:NEURON_DOWNLOADER -ne 'curl'))) {
        if ($result) {
            Write-Detail "curl.exe could not download $(Split-Path -Leaf $OutFile) ($($result.Message)); using .NET for it and the remaining downloads ..."
            if ($state) { $state.DotNetOnly = $true }
        }
        $result = Invoke-DotNetDownload $Url $OutFile $Label
    }
    if ($result.Ok) { return }
    if ($result.Status -eq 404) {
        throw "${Url} returned HTTP 404. Neuron $Version may not be published for Windows; see https://github.com/$NeuronRepository/releases"
    }
    if ($result.Status -gt 0) { throw "downloading ${Url} failed with HTTP $($result.Status). Try again later." }
    throw "could not download ${Url}: $($result.Message). Check your network or proxy settings and retry."
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
    # A named release gets its own install.sh and --version.
    $named = "$Requested".Trim()
    if ($named.StartsWith('v')) { $named = $named.Substring(1) }
    if ($named -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') { $named = $NeuronReleaseVersion }
    if ($named) {
        return "curl -fsSL https://github.com/$NeuronRepository/releases/download/v$named/install.sh | sh -s -- --version $named"
    }
    return "curl -fsSL https://github.com/$NeuronRepository/releases/download/<version>/install.sh | sh, with the newest version from https://github.com/$NeuronRepository/releases"
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

# The newest published release in a GitHub API response: the release list (newest first; a
# draft only shows up for a token that can see it) or a single release object.
function Select-NeuronReleaseTag($Response) {
    foreach ($release in @($Response)) {
        if (-not $release) { continue }
        $names = $release.PSObject.Properties.Name
        if (-not ($names -contains 'tag_name')) { continue }
        if (($names -contains 'draft') -and ($release.draft -eq $true)) { continue }
        if ($release.tag_name) { return [string]$release.tag_name }
    }
    return $null
}

function Resolve-NeuronVersion([string]$Requested) {
    if ($Requested) { return $Requested }
    if ($NeuronReleaseVersion) { return $NeuronReleaseVersion }
    # Not /releases/latest: GitHub never marks a prerelease "latest", so that is a 404 while
    # every Neuron 7 release is a prerelease.
    $api = $env:NEURON_RELEASES_API
    if (-not $api) { $api = "https://api.github.com/repos/$NeuronRepository/releases?per_page=10" }
    if (-not (Test-AllowedUrl $api)) { throw "NEURON_RELEASES_API must be an HTTPS URL (or loopback HTTP): $api" }
    $headers = @{ 'Accept' = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN -and (Test-GitHubApiUrl $api)) { $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)" }
    try {
        $response = Invoke-RestMethod -Uri $api -Headers $headers -UseBasicParsing
    } catch {
        $status = Get-HttpStatus $_
        $hint = 'Pass -Version 7.0.0-alpha.5 (or set NEURON_VERSION) to skip the lookup.'
        if (($status -eq 403) -or ($status -eq 429)) { $hint = "GitHub's hourly limit for anonymous lookups was reached. $hint" }
        throw "could not look up the newest Neuron release at ${api}: $($_.Exception.Message). $hint"
    }
    $tag = Select-NeuronReleaseTag $response
    if (-not $tag) { throw "the release lookup at $api returned no published release; pass -Version." }
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
    # ExternalScript too: npm's neuron.ps1 shim wins in PowerShell (and fails under "Restricted").
    $found = Get-Command neuron -CommandType Application, ExternalScript -ErrorAction SilentlyContinue | Select-Object -First 1
    if ((-not $found) -or ($found.Path -ieq $Destination)) { return }
    Write-Warn "another neuron at $($found.Path) comes first on PATH and runs instead of $Destination."
    Write-Detail 'An older Neuron is the usual cause. Remove it with the command for how it was installed:'
    Write-Detail '  an old NeuronCLI folder:  Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\NeuronCLI", then remove that folder from your user PATH'
    Write-Detail '  npm:                      npm.cmd uninstall -g @zero-x.live/neuron neuron-cli-app'
    Write-Detail '  pip:                      pip uninstall neuroncli (or pipx uninstall neuroncli)'
    Write-Detail "If it is another program, move $(Split-Path -Parent $Destination) before $(Split-Path -Parent $found.Path) in PATH, or run neuron by its full path."
}

# Puts $Directory first in a ;-separated PATH value, dropping any later copy of it.
function Get-PathWithEntryFirst([string]$PathValue, [string]$Directory) {
    $wanted = $Directory.TrimEnd('\')
    $rest = @()
    if ($PathValue) {
        $rest = @($PathValue.Split(';') | Where-Object { $_ -and ($_.Trim().TrimEnd('\') -ine $wanted) })
    }
    return (@($Directory) + $rest) -join ';'
}

# Puts $Directory FIRST in the user PATH (moving it if it is already there), so this install's
# neuron wins over an older neuron in the user PATH, such as a NeuronCLI folder from an earlier
# release. Returns $true when the stored value changed.
# The system PATH comes before the user PATH in every new terminal, so a neuron there still wins
# even with this install first in the user PATH. Moving it needs an administrator: only warn.
function Write-SystemPathNeuronWarning([string]$Destination) {
    $machine = [System.Environment]::GetEnvironmentVariable('Path', 'Machine')
    if (-not $machine) { return }
    foreach ($entry in $machine.Split(';')) {
        $dir = [System.Environment]::ExpandEnvironmentVariables($entry.Trim())
        if (-not $dir) { continue }
        foreach ($name in @('neuron.exe', 'neuron.cmd', 'neuron.bat', 'neuron.ps1')) {
            try { $candidate = Join-Path $dir $name } catch { continue }
            if ((Test-Path -LiteralPath $candidate -PathType Leaf) -and ($candidate -ine $Destination)) {
                Write-Warn "new terminals will still run $candidate first: the system PATH comes before your user PATH."
                Write-Detail "Remove that copy (it needs an administrator), or run neuron by its full path: & '$($Destination -replace "'", "''")'"
                return
            }
        }
    }
}

function Add-UserPathEntry([string]$Directory) {
    # Read the raw registry value so %VARIABLE% references in the user PATH survive.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $updated = Get-PathWithEntryFirst $current $Directory
        if ($updated -ceq $current) { return $false }
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

# Removes neuron.exe.old* left by earlier upgrades. One that is still running (neuron open in
# another terminal during that upgrade) cannot be deleted yet and is left for a later install.
function Remove-StaleNeuronCopies([string]$Destination) {
    $folder = Split-Path -Parent $Destination
    $leaf = Split-Path -Leaf $Destination
    try {
        foreach ($stale in @(Get-ChildItem -LiteralPath $folder -Filter "$leaf.old*" -File -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $stale.FullName -Force -ErrorAction SilentlyContinue
        }
    } catch { }
}

function Install-NeuronBinary([string]$Source, [string]$Destination) {
    # A running neuron.exe cannot be overwritten, but it can be renamed.
    $hadPrevious = Test-Path -LiteralPath $Destination
    $previous = "$Destination.old"
    if ($hadPrevious) {
        Remove-StaleNeuronCopies $Destination
        # A neuron.exe.old that is still running cannot be removed: set this one aside under its own name.
        if (Test-Path -LiteralPath $previous) { $previous = "$Destination.old-$([System.Guid]::NewGuid().ToString('N'))" }
        try {
            Move-Item -LiteralPath $Destination -Destination $previous -Force
        } catch {
            throw "could not replace ${Destination}: $($_.Exception.Message). Close every terminal that is running neuron, then run the installer again."
        }
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

# Succeeds only when the release's SHA256SUMS lists exactly this hash for this asset. The file is
# fetched from the GitHub release (NEURON_CHECKSUMS_BASE overrides it for tests), never from the
# mirror the binary may have come from.
function Test-NeuronSums([string]$SumsFile, [string]$Asset, [string]$Hash) {
    foreach ($line in [System.IO.File]::ReadAllLines($SumsFile)) {
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Count -ne 2) { continue }
        if ($parts[1].TrimStart('*') -eq $Asset) { return ($parts[0].ToLowerInvariant() -eq $Hash) }
    }
    return $false
}

# The full-screen UI, verified like neuron.exe. Any failure is a warning: neuron runs its line
# REPL without it.
function Install-NeuronUi([string]$Base, [string]$Version, [string]$Work, [string]$Target, [string]$SumsFile) {
    $url = "$Base/v$Version/$NeuronUiAsset"
    try {
        $checksumFile = Join-Path $Work "$NeuronUiAsset.sha256"
        Get-NeuronDownload "$url.sha256" $checksumFile $Version
        $checksumText = [System.IO.File]::ReadAllText($checksumFile).Trim()
        $expected = ''
        if ($checksumText) { $expected = ($checksumText -split '\s+')[0].ToLowerInvariant() }
        if ($expected -notmatch '^[0-9a-f]{64}$') { throw "$url.sha256 does not contain a SHA-256 checksum." }
        $download = Join-Path $Work $NeuronUiAsset
        Get-NeuronDownload $url $download $Version 'neuron-tui.exe'
        if ((Get-NeuronSha256 $download) -ne $expected) { throw "checksum mismatch for $NeuronUiAsset" }
        if (-not (Test-NeuronSums $SumsFile $NeuronUiAsset $expected)) { throw "$NeuronUiAsset is not listed in the release SHA256SUMS with that checksum" }
        # Not run here: the two checksums already prove these are the release's bytes, and the
        # first start of a new, large, unsigned program is when Windows Defender scans it. That
        # scan can hold up process creation itself, which no timeout around the run would cover.
        # neuron starts the UI on demand and falls back to its line REPL if it ever fails to start.
        $destination = Join-Path $Target 'neuron-tui.exe'
        Install-NeuronBinary $download $destination
        Write-Detail "installed $destination"
        return $true
    } catch {
        Write-Warn "the full-screen UI was not installed ($($_.Exception.Message)); neuron uses its line REPL. Rerun the installer to try again."
        return $false
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
    $changePath = -not ($NoPathChange -or (Test-Truthy $env:NEURON_NO_MODIFY_PATH))
    # Read by Get-NeuronDownload, a child scope, so nothing is left behind in an `irm | iex` session.
    $NeuronDownloadState = @{ DotNetOnly = $false }

    $base = $env:NEURON_DOWNLOAD_BASE
    if (-not $base) { $base = "https://github.com/$NeuronRepository/releases/download" }
    $base = $base.TrimEnd('/')
    if (-not (Test-AllowedUrl $base)) { throw "NEURON_DOWNLOAD_BASE must be an HTTPS URL (or loopback HTTP): $base" }

    $checksumsBase = $env:NEURON_CHECKSUMS_BASE
    if (-not $checksumsBase) { $checksumsBase = "https://github.com/$NeuronRepository/releases/download" }
    $checksumsBase = $checksumsBase.TrimEnd('/')
    if (-not (Test-AllowedUrl $checksumsBase)) { throw "NEURON_CHECKSUMS_BASE must be an HTTPS URL (or loopback HTTP): $checksumsBase" }

    $resolved = (Resolve-NeuronVersion $Version).Trim()
    if ($resolved.StartsWith('v')) { $resolved = $resolved.Substring(1) }
    if ($resolved -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
        throw "invalid version: $resolved (expected something like 7.0.0-alpha.5)"
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
        $sumsFile = Join-Path $work 'SHA256SUMS'
        Get-NeuronDownload "$checksumsBase/v$resolved/SHA256SUMS" $sumsFile $resolved
        $checksumFile = Join-Path $work "$NeuronAsset.sha256"
        Get-NeuronDownload "$url.sha256" $checksumFile $resolved
        $checksumText = [System.IO.File]::ReadAllText($checksumFile).Trim()
        $expected = ''
        if ($checksumText) { $expected = ($checksumText -split '\s+')[0].ToLowerInvariant() }
        if ($expected -notmatch '^[0-9a-f]{64}$') { throw "$url.sha256 does not contain a SHA-256 checksum." }

        $download = Join-Path $work $NeuronAsset
        Get-NeuronDownload $url $download $resolved 'neuron.exe'
        $actual = Get-NeuronSha256 $download
        if ($actual -ne $expected) {
            throw "checksum mismatch for ${NeuronAsset}: expected $expected, got $actual. Nothing was installed."
        }
        if (-not (Test-NeuronSums $sumsFile $NeuronAsset $actual)) {
            throw "the release SHA256SUMS ($checksumsBase/v$resolved/SHA256SUMS) does not list $NeuronAsset with SHA-256 $actual. Nothing was installed."
        }
        Write-Detail "verified SHA-256 $actual (matches the per-file checksum and SHA256SUMS)"

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

        # neuron.exe is usable from here on, before the much larger UI download starts.
        Write-Host "Installed $destination" -ForegroundColor Green
        $reported = Get-ReportedVersion $versionOutput
        if ($reported) { Write-Detail $reported }

        $onPath = Test-PathEntry $env:Path $target
        if ($changePath) {
            if (Add-UserPathEntry $target) {
                Write-Detail "put $target first in your user PATH"
            }
            # First in this window too (irm | iex runs in the user's own session), so `neuron`
            # works here straight away and an older neuron on PATH no longer wins.
            $env:Path = Get-PathWithEntryFirst $env:Path $target
            $onPath = $true
            Write-ShadowingNeuronWarning $destination
            Write-SystemPathNeuronWarning $destination
        } elseif ($onPath) {
            Write-ShadowingNeuronWarning $destination
        } else {
            Write-Warn "$target is not on your PATH (you asked not to change it); run neuron by its full path."
        }

        if (-not (Test-Truthy $env:NEURON_NO_TUI)) {
            Write-Step 'neuron is ready. Now the full-screen UI (neuron-tui.exe), the largest download.'
            Write-Detail 'If you stop here, neuron still works with its line REPL; run this installer again to add the UI.'
            [void](Install-NeuronUi $base $resolved $work $target $sumsFile)
        } else {
            Write-Detail 'NEURON_NO_TUI is set; skipped the full-screen UI.'
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    $run = 'neuron'
    if (-not ($onPath -or $changePath)) { $run = "& '$($destination -replace "'", "''")'" }

    Write-Host ''
    Write-Host "Neuron $resolved is installed." -ForegroundColor Green
    Write-Host 'Next steps:'
    $step = 1
    if ($run -eq 'neuron' -and $changePath -and $InvokedAsFile) {
        # Run as a file (powershell -File), this window's PATH is not the caller's.
        Write-Host "  $step. Open a new terminal window (PowerShell or Command Prompt) so it sees the updated PATH."
        $step++
    }
    Write-Host "  $step. Sign in:  $run login     (a free Zero-X plan is available)"
    $step++
    Write-Host "  $step. Start:    $run           (in your project folder)"
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
