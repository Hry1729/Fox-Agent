#Requires -Version 5.1
<#
.SYNOPSIS
  Prepare the exact Windows x64/MSVC Zvec native resource used by the
  locked zvec-rust-sys dependency, with a pinned, traceable identity.

.DESCRIPTION
  This is resource preparation only. It does not run the smoke test and does
  not use the Zvec library. It downloads the prebuilt archive published for
  zvec-rust 0.6.0, checks the archive SHA-256 before extraction, extracts into
  a temporary directory, checks both the DLL and its matching MSVC import
  library, and only then publishes the directory atomically.

  The smoke test uses the published directory through ZVEC_LIB_DIR and
  ZVEC_AUTO_BUILD=0. The Cargo build script still resolves and links that exact
  directory; no system DLL is searched or substituted.
#>
[CmdletBinding()]
param(
    [string] $Destination = '',
    [string] $ArchivePath
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) { $Destination = Join-Path $repoRoot '.test-target\zvec\0.6.0' }

$sourceUrl = 'https://github.com/zvec-ai/zvec-rust/releases/download/v0.6.0/zvec-prebuilt-x86_64-pc-windows-msvc.tar.gz'
$packageVersion = '0.6.0'
$target = 'x86_64-pc-windows-msvc'
$archiveSha256 = 'c823448a39a0f69aacdfe1fe06aaccb0e26eb9e0a4eea65fbd96edf749093152'
$dllSha256 = '2137ad6f24c00e8337efd54d8408df4690aa356773ab596beb495956dfeb7cd4'
$libSha256 = 'c5fa14ea2530726be869f784a0521c1697a4216ef4c916282805fbed6fe2722d'
$dllName = 'zvec_c_api.dll'
$libName = 'zvec_c_api.lib'

function Get-FileSha256([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "missing file: $Path"
    }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-Sha256([string] $Path, [string] $Expected) {
    $actual = Get-FileSha256 $Path
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "SHA-256 mismatch for $Path`: expected $Expected, got $actual"
    }
    return $actual
}

function Test-PreparedResource([string] $Directory) {
    $dll = Join-Path $Directory $dllName
    $lib = Join-Path $Directory $libName
    $manifest = Join-Path $Directory 'resource.json'
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf) -or
        -not (Test-Path -LiteralPath $lib -PathType Leaf) -or
        -not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
        return $false
    }
    if ((Get-FileSha256 $dll) -ne $dllSha256) { return $false }
    if ((Get-FileSha256 $lib) -ne $libSha256) { return $false }
    try {
        $record = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
        return $record.packageVersion -eq $packageVersion -and
            $record.target -eq $target -and
            $record.dllSha256 -eq $dllSha256 -and
            $record.libSha256 -eq $libSha256 -and
            $record.archiveSha256 -eq $archiveSha256
    } catch {
        return $false
    }
}

if (Test-PreparedResource $Destination) {
    Write-Output "Zvec resource already prepared: $Destination"
    Write-Output "package=zvec-rust-sys/$packageVersion target=$target source=$sourceUrl"
    Write-Output "dll=$dllSha256 lib=$libSha256 archive=$archiveSha256"
    return
}

if (Test-Path -LiteralPath $Destination -PathType Container) {
    throw "Zvec resource directory exists but does not match the pinned identity: $Destination. Remove the cache deliberately, then rerun this preparation."
}

$destinationParent = Split-Path -Parent ([System.IO.Path]::GetFullPath($Destination))
New-Item -ItemType Directory -Force -Path $destinationParent | Out-Null

$staging = Join-Path $destinationParent ('.zvec-staging-' + [Guid]::NewGuid().ToString('N').Substring(0, 12))
New-Item -ItemType Directory -Path $staging | Out-Null

try {
    if (-not $ArchivePath) {
        $ArchivePath = Join-Path $destinationParent 'zvec-prebuilt-v0.6.0-x86_64-pc-windows-msvc.tar.gz'
        if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
            $download = $ArchivePath + '.download'
            if (Test-Path -LiteralPath $download) { Remove-Item -LiteralPath $download -Force }
            Write-Output "Preparing Zvec resource: downloading $sourceUrl"
            $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
            if ($curl) {
                & $curl.Source -L --retry 3 --retry-delay 2 -f -o $download $sourceUrl
                if ($LASTEXITCODE -ne 0) { throw "Zvec archive download failed with exit code $LASTEXITCODE" }
            } else {
                Invoke-WebRequest -Uri $sourceUrl -OutFile $download -UseBasicParsing
            }
            Move-Item -LiteralPath $download -Destination $ArchivePath
        } else {
            Write-Output "Preparing Zvec resource: reusing archive $ArchivePath"
        }
    }

    $archivePathFull = [System.IO.Path]::GetFullPath($ArchivePath)
    Assert-Sha256 $archivePathFull $archiveSha256 | Out-Null
    Write-Output "Zvec archive verified: $archivePathFull"

    $tar = Get-Command tar.exe -ErrorAction SilentlyContinue
    if (-not $tar) { $tar = Get-Command tar -ErrorAction SilentlyContinue }
    if (-not $tar) { throw 'tar is required to extract the pinned Zvec archive' }
    & $tar.Source -xzf $archivePathFull -C $staging
    if ($LASTEXITCODE -ne 0) { throw "Zvec archive extraction failed with exit code $LASTEXITCODE" }

    $dll = Join-Path $staging $dllName
    $lib = Join-Path $staging $libName
    Assert-Sha256 $dll $dllSha256 | Out-Null
    Assert-Sha256 $lib $libSha256 | Out-Null

    $record = [ordered]@{
        schemaVersion = 1
        package = 'zvec-rust-sys'
        packageVersion = $packageVersion
        target = $target
        sourceUrl = $sourceUrl
        license = 'Apache-2.0'
        archiveSha256 = $archiveSha256
        archiveBytes = (Get-Item -LiteralPath $archivePathFull).Length
        dll = $dllName
        dllSha256 = $dllSha256
        dllBytes = (Get-Item -LiteralPath $dll).Length
        importLibrary = $libName
        libSha256 = $libSha256
        libBytes = (Get-Item -LiteralPath $lib).Length
        preparedAt = (Get-Date).ToUniversalTime().ToString('o')
    }
    $record | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $staging 'resource.json') -Encoding UTF8

    Move-Item -LiteralPath $staging -Destination $Destination
} catch {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
    throw
}

if (-not (Test-PreparedResource $Destination)) {
    throw "Zvec resource publication did not verify: $Destination"
}
Write-Output "Zvec resource prepared: $Destination"
Write-Output "package=zvec-rust-sys/$packageVersion target=$target source=$sourceUrl"
Write-Output "dll=$dllSha256 lib=$libSha256 archive=$archiveSha256"
