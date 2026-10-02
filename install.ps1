# Install a verified native Windows release without administrator access.
[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'JevObserver\bin')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$officialBase = 'https://github.com/LimePencil/jev-observer/releases'
$releaseBase = if ($env:JEV_OBSERVER_RELEASE_BASE_URL) { $env:JEV_OBSERVER_RELEASE_BASE_URL.TrimEnd('/') } else { $officialBase }
$releaseUri = [uri]$releaseBase
if (-not $releaseUri.IsAbsoluteUri -or $releaseUri.UserInfo -or $releaseUri.Query -or $releaseUri.Fragment -or
    ($releaseUri.Scheme -ne 'https' -and -not ($releaseUri.Scheme -eq 'http' -and $releaseUri.IsLoopback -and $env:JEV_OBSERVER_ALLOW_INSECURE_HTTP -eq '1'))) {
    throw 'The release base URL must be HTTPS without credentials, query or fragment.'
}
if ($Version) {
    $Version = $Version -replace '^v', ''
    if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') { throw 'Version must be MAJOR.MINOR.PATCH.' }
}
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'This installer requires native Windows.' }
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$target = switch ($architecture) {
    'X64' { 'x86_64-pc-windows-msvc' }
    'Arm64' { 'aarch64-pc-windows-msvc' }
    default { throw "No prebuilt Windows release supports $architecture." }
}
$tag = if ($Version) { "v$Version" } else { 'latest' }
$useGh = $false
if ($releaseBase -eq $officialBase -and (Get-Command gh -ErrorAction SilentlyContinue)) {
    & gh auth status --active --hostname github.com *> $null
    $useGh = $LASTEXITCODE -eq 0
}
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$work = Join-Path ([IO.Path]::GetTempPath()) ('jev-observer-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
function Download-Asset([string]$AssetTag, [string]$Name, [string]$Destination) {
    if ($useGh) {
        $arguments = @('release', 'download')
        if ($AssetTag -ne 'latest') { $arguments += $AssetTag }
        $arguments += @('--repo', 'github.com/LimePencil/jev-observer', '--pattern', $Name, '--output', $Destination)
        & gh @arguments
        if ($LASTEXITCODE -ne 0) { throw "Could not download $Name. Confirm repository access with gh auth login." }
    } else {
        $assetUrl = if ($AssetTag -eq 'latest') { "$releaseBase/latest/download/$Name" } else { "$releaseBase/download/$AssetTag/$Name" }
        Invoke-WebRequest -UseBasicParsing -Uri $assetUrl -OutFile $Destination
    }
}
try {
    $manifest = Join-Path $work 'SHA256SUMS'
    Download-Asset $tag 'SHA256SUMS' $manifest
    if ((Get-Item -LiteralPath $manifest).Length -gt 1048576) { throw 'Checksum manifest is unexpectedly large.' }
    $pattern = '^([0-9a-fA-F]{64})  (jev-observer-v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)-' + [regex]::Escape($target) + '\.zip)$'
    $matchesForTarget = @(Get-Content -LiteralPath $manifest | ForEach-Object {
        if ($_ -match $pattern) { [pscustomobject]@{ Hash = $Matches[1]; Asset = $Matches[2]; Version = $Matches[3] } }
    })
    if ($matchesForTarget.Count -ne 1) { throw "Expected exactly one checksum for $target." }
    $entry = $matchesForTarget[0]
    if ($Version -and $entry.Version -ne $Version) { throw 'Checksum manifest does not match the requested version.' }
    $archivePath = Join-Path $work $entry.Asset
    # Pin the archive to the manifest version even when downloading latest.
    Download-Asset ('v' + $entry.Version) $entry.Asset $archivePath
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash -ne $entry.Hash) { throw 'SHA-256 checksum mismatch; the existing installation was preserved.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        if ($archive.Entries.Count -ne 1 -or $archive.Entries[0].FullName -cne 'jev-observer.exe') { throw 'Release archive must contain exactly one root jev-observer.exe.' }
        $executable = Join-Path $work 'jev-observer.exe'
        [IO.Compression.ZipFileExtensions]::ExtractToFile($archive.Entries[0], $executable, $false)
    } finally { $archive.Dispose() }
    $reported = & $executable --version
    if ($LASTEXITCODE -ne 0 -or "$reported".Trim() -cne ('jev-observer ' + $entry.Version)) { throw 'Executable version does not match the release; the existing installation was preserved.' }
    $directory = New-Item -ItemType Directory -Path $InstallDir -Force
    if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Installation directory must not be a reparse point.' }
    $destination = Join-Path $directory.FullName 'jev-observer.exe'
    if (Test-Path -LiteralPath $destination) {
        $existing = Get-Item -LiteralPath $destination -Force
        if ($existing.PSIsContainer -or ($existing.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'The install destination must be a regular executable.' }
    }
    $staging = Join-Path $directory.FullName ('.jev-observer-' + [guid]::NewGuid().ToString('N') + '.exe')
    try {
        Copy-Item -LiteralPath $executable -Destination $staging
        if (Test-Path -LiteralPath $destination) {
            # PowerShell converts ordinary $null to an empty string here.
            # NullString passes the .NET null required for no backup file.
            [IO.File]::Replace($staging, $destination, [NullString]::Value)
        } else {
            [IO.File]::Move($staging, $destination)
        }
    } finally {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Force }
    }
    Write-Output "Installed jev-observer $($entry.Version) to $destination"
    Write-Output 'For this terminal, add its directory to PATH:'
    $quotedDirectory = $directory.FullName.Replace("'", "''")
    Write-Output ('$env:PATH = ''' + $quotedDirectory + ';'' + $env:PATH')
    Write-Output 'Then run: jev-observer --demo'
} finally { Remove-Item -LiteralPath $work -Recurse -Force }
