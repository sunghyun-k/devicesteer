# Installs DeviceSteer on Windows from GitHub Releases:
#
#   irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1 | iex
#
# A given version (also through $env:DEVICESTEER_VERSION, since `iex` takes no arguments):
#
#   & ([scriptblock]::Create((irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1))) -Version 1.0.0
#
# -NoAgents (also $env:DEVICESTEER_NO_AGENTS = '1') skips setting up the devicesteer command and the
# agent skill, which it otherwise does after installing, through the installed CLI's
# `devicesteer agents install` (the same code as the app's Settings > Agents).
#
# It downloads DeviceSteer_x64-setup.exe and the release's SHA256SUMS, checks the
# installer against it, and runs the installer silently for the current user (no administrator
# rights). Why a script and not only the installer: a file a browser downloads carries the Mark of
# the Web, and SmartScreen stops unsigned installers that carry it; curl.exe and
# Invoke-WebRequest don't add it. Smart App Control (Windows 11) still blocks unsigned apps,
# whichever way they arrive. The checksum catches a broken download, not a compromised release:
# both files come from the same place.
#
# Kept in scripts/ and attached to every release by .github/workflows/release-windows.yml.
[CmdletBinding()]
param(
    # Version to install, like 1.0.0; the latest release when empty.
    [string]$Version = $env:DEVICESTEER_VERSION,
    # Don't set up the devicesteer command and the agent skill (the app's Settings > Agents can).
    [switch]$NoAgents = ($env:DEVICESTEER_NO_AGENTS -eq '1')
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is far slower with its progress bar.

$repo = 'sunghyun-k/devicesteer'

function Fail([string]$message) {
    Write-Host "DeviceSteer: $message" -ForegroundColor Red
    # `exit` would close the window of someone who ran this through `iex`.
    throw "DeviceSteer: $message"
}

# Downloads with curl.exe (Windows 10 1803 and later), else Invoke-WebRequest. Windows' own curl
# first: another one can come earlier on PATH (Git for Windows puts its own there), and then
# Get-Command returns them all, so take only the first.
function Get-File([string]$url, [string]$out) {
    $curl = if ($env:SystemRoot) { Join-Path $env:SystemRoot 'System32\curl.exe' }
    if (-not ($curl -and (Test-Path -LiteralPath $curl -PathType Leaf))) {
        $curl = Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
    }
    if ($curl) {
        & $curl --fail --location --silent --show-error --retry 3 --output $out $url
        if ($LASTEXITCODE -ne 0) { Fail "could not download $url (curl exit code $LASTEXITCODE)" }
    } else {
        Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing
    }
}

if (-not [Environment]::Is64BitOperatingSystem) { Fail 'DeviceSteer needs 64-bit Windows.' }
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Version = "$Version".Trim().TrimStart('v')
$api = if ($Version) { "https://api.github.com/repos/$repo/releases/tags/v$Version" } else { "https://api.github.com/repos/$repo/releases/latest" }
try {
    $release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'devicesteer-install'; Accept = 'application/vnd.github+json' }
} catch {
    if ($Version) { Fail "no release v$Version on github.com/$repo ($($_.Exception.Message))" }
    Fail "could not read the latest release from github.com/$repo ($($_.Exception.Message))"
}
$Version = "$($release.tag_name)".TrimStart('v')
# Releases up to 1.0.0-beta.4 named the installer after the version; later ones don't, so that
# releases/latest/download/DeviceSteer_x64-setup.exe works. -Version may name either kind.
$installer = $null
foreach ($candidate in @('DeviceSteer_x64-setup.exe', "DeviceSteer_${Version}_x64-setup.exe")) {
    $installer = $release.assets | Where-Object { $_.name -eq $candidate } | Select-Object -First 1
    if ($installer) { $name = $candidate; break }
}
if (-not $installer) { $name = 'DeviceSteer_x64-setup.exe' }
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1
if (-not $installer) { Fail "release v$Version has no Windows installer ($name)" }
if (-not $sums) { Fail "release v$Version has no SHA256SUMS to check the installer against" }

$dir = Join-Path ([IO.Path]::GetTempPath()) ("devicesteer-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $dir | Out-Null
try {
    Write-Host "Downloading DeviceSteer $Version..."
    $exe = Join-Path $dir $name
    $sumsFile = Join-Path $dir 'SHA256SUMS'
    Get-File $installer.browser_download_url $exe
    Get-File $sums.browser_download_url $sumsFile

    # Lines are "<hash>  <name>" (sha256sum's format; a `*` marks binary mode).
    $expected = $null
    foreach ($line in Get-Content -LiteralPath $sumsFile) {
        if ($line -match '^([0-9a-fA-F]{64}) [ *](.+)$' -and $Matches[2].Trim() -eq $name) { $expected = $Matches[1].ToLowerInvariant() }
    }
    if (-not $expected) { Fail "SHA256SUMS of v$Version doesn't list $name" }
    $actual = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { Fail "the downloaded installer's SHA-256 is $actual, not $expected; nothing was installed" }

    Write-Host 'Installing (for this user)...'
    # /S: silent. The installer installs per user (bundle.windows.nsis.installMode) and closes a
    # running DeviceSteer and its daemons first.
    $p = Start-Process -FilePath $exe -ArgumentList '/S' -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "the installer exited with code $($p.ExitCode)" }
} finally {
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
}

# Where the installer puts a per-user install (bundle.windows.nsis.installMode currentUser).
$installDir = Join-Path $env:LOCALAPPDATA 'DeviceSteer'
$cli = Join-Path $installDir 'devicesteer.exe'
Write-Host "DeviceSteer $Version is installed in $installDir." -ForegroundColor Green

# Text in this file stays ASCII: `irm | iex` may decode it as Windows-1252 in Windows PowerShell.
if ($NoAgents) {
    Write-Host "Agents weren't set up (-NoAgents). Run `"$cli`" agents install, or use Settings > Agents in the app."
} elseif (-not (Test-Path -LiteralPath $cli -PathType Leaf)) {
    Write-Host "Open DeviceSteer from the Start menu; it offers to set up the devicesteer command and the agent skill."
} else {
    # Native commands: with 'Stop', Windows PowerShell turns a line on stderr into a terminating
    # error, so the CLI's own error JSON would abort the script instead of being reported.
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        # Releases up to 1.0.0-beta.4 have no `agents` command.
        & $cli agents --help *> $null
        $hasAgents = ($LASTEXITCODE -eq 0)
        $result = $null
        $ok = $false
        if ($hasAgents) {
            $result = & $cli agents install 2> $null
            $ok = ($LASTEXITCODE -eq 0)
        }
    } finally {
        $ErrorActionPreference = $saved
    }
    if (-not $hasAgents) {
        Write-Host "This version can't set up agents from the command line: open DeviceSteer and use Settings > Agents."
    } elseif (-not $ok) {
        Write-Host "Setting up agents failed: open DeviceSteer and use Settings > Agents, or run `"$cli`" agents install to see why." -ForegroundColor Yellow
    } else {
        $r = ($result -join "`n") | ConvertFrom-Json
        foreach ($i in @($r.installed)) {
            if ($i.id -eq 'cli') { Write-Host "The devicesteer command: $($i.path)" }
            else { Write-Host "The DeviceSteer skill for $($i.name): $($i.path)" }
        }
        foreach ($i in @($r.skipped)) { Write-Host "Skipped $($i.path): $($i.error)" -ForegroundColor Yellow }
        foreach ($n in @($r.notes)) { Write-Host $n }
        $skipped = @($r.skipped | ForEach-Object { $_.id })
        if ($skipped.Count -gt 0) {
            Write-Host ("To replace what is there (it is moved aside, not deleted): devicesteer agents install --replace " + ($skipped -join ' '))
        }
        # The Path a new terminal gets, from the registry: this session's $env:Path may be older.
        $bin = (Join-Path $env:USERPROFILE '.local\bin').TrimEnd('\')
        $paths = @([Environment]::GetEnvironmentVariable('Path', 'User'), [Environment]::GetEnvironmentVariable('Path', 'Machine')) -join ';'
        $onPath = @($paths -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $bin) }).Count -gt 0
        if (-not $onPath) {
            Write-Host "$bin isn't on your Path: add it to your user Path (Edit environment variables for your account) and open a new terminal." -ForegroundColor Yellow
        }
    }
}
Write-Host 'For USB devices, install Apple Devices from the Microsoft Store (or iTunes), open it once and accept its license.'
