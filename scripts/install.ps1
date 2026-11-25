# Installs DeviceSteer on Windows from GitHub Releases:
#
#   irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1 | iex
#
# A given version (also through $env:DEVICESTEER_VERSION, since `iex` takes no arguments):
#
#   & ([scriptblock]::Create((irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1))) -Version 1.0.0
#
# It downloads DeviceSteer_<version>_x64-setup.exe and the release's SHA256SUMS, checks the
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
    [string]$Version = $env:DEVICESTEER_VERSION
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is far slower with its progress bar.

$repo = 'sunghyun-k/devicesteer'

function Fail([string]$message) {
    Write-Host "DeviceSteer: $message" -ForegroundColor Red
    # `exit` would close the window of someone who ran this through `iex`.
    throw "DeviceSteer: $message"
}

# Downloads with curl.exe (Windows 10 1803 and later), else Invoke-WebRequest.
function Get-File([string]$url, [string]$out) {
    $curl = Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source --fail --location --silent --show-error --retry 3 --output $out $url
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
$name = "DeviceSteer_${Version}_x64-setup.exe"
$installer = $release.assets | Where-Object { $_.name -eq $name } | Select-Object -First 1
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

Write-Host "DeviceSteer $Version is installed. Open it from the Start menu; it offers to set up the devicesteer command and the agent skill." -ForegroundColor Green
Write-Host 'For USB devices, install Apple Devices from the Microsoft Store (or iTunes) if you have not.'
