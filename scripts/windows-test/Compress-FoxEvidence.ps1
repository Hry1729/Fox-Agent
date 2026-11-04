#Requires -Version 5.1
<#
.SYNOPSIS
    Packs the results file, the evidence folder and screenshots into one zip and checks
    which test items have no result line yet (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    TestRoot defaults to <repo>\docs\windows-test\run-<Version> (derived from this script's
    folder). Writes <TestRoot>\fox-evidence-<Version>.zip (UTF-8 entry names, so Chinese file names
    survive) with:
        results-<Version>.md      from -Results
        fox-evidence\...          from -EvidenceDir (database snapshots *.parts are skipped)
        shots\...                 from -ShotsDir, when it exists
        logs\...                  Fox stderr logs of this round (-IncludeLogs; they hold
                                  conversation text, so they are off by default)
        manifest.json             file list with sizes and SHA256
    Then prints the zip size and SHA256, and every -ItemId that does not start a line
    in the results file (so a forgotten item is visible before sending).

.EXAMPLE
    .\Compress-FoxEvidence.ps1
#>
[CmdletBinding()]
param(
    [string] $Version = '1339',
    [string] $TestRoot,
    [string] $Results,
    [string] $EvidenceDir,
    [string] $ShotsDir,
    [string] $OutZip,
    [switch] $IncludeLogs,
    [string[]] $ItemId = @(
        'R1-1', 'R1-5', 'R1-6',
        'R1-2', 'R2-1',
        'R1-4', 'R1-3', 'B4-2',
        'B4-5', 'B4-5b', 'R2-4', 'B4-11',
        'B5-1', 'B5-2', 'B5-3',
        'B3-4', 'B4-12', 'B4-13',
        'U-1', 'U-2', 'U-3', 'U-4', 'U-5', 'U-6', 'U-7', 'U-8', 'B4-8',
        'B5-4', 'B5-5', 'B5-6', 'B5-7', 'B5-8',
        'B6-2', 'B6-3',
        'B6-5', 'B6-6', 'B6-7'))

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $TestRoot) { $TestRoot = [System.IO.Path]::GetFullPath((Join-Path $here "..\..\docs\windows-test\run-$Version")) }
if (-not $Results) { $Results = Join-Path $TestRoot "results-$Version.md" }
if (-not $EvidenceDir) { $EvidenceDir = Join-Path $TestRoot 'fox-evidence' }
if (-not $ShotsDir) { $ShotsDir = Join-Path $TestRoot "shots-$Version" }
if (-not $OutZip) { $OutZip = Join-Path $TestRoot "fox-evidence-$Version.zip" }

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("fox-pack-$Version-" + (Get-Date -Format 'yyyyMMddHHmmss'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null
try {
    if (Test-Path -LiteralPath $Results) { Copy-Item -LiteralPath $Results -Destination (Join-Path $stage ([System.IO.Path]::GetFileName($Results))) }
    else { Write-Warning "results file not found: $Results" }
    if (Test-Path -LiteralPath $EvidenceDir) {
        $dest = Join-Path $stage 'fox-evidence'
        New-Item -ItemType Directory -Path $dest -Force | Out-Null
        Get-ChildItem -LiteralPath $EvidenceDir -File | Copy-Item -Destination $dest
    } else { Write-Warning "evidence folder not found: $EvidenceDir" }
    if (Test-Path -LiteralPath $ShotsDir) { Copy-Item -LiteralPath $ShotsDir -Destination (Join-Path $stage 'shots') -Recurse }
    if ($IncludeLogs -and (Test-Path -LiteralPath (Join-Path $TestRoot 'logs'))) {
        Copy-Item -LiteralPath (Join-Path $TestRoot 'logs') -Destination (Join-Path $stage 'logs') -Recurse
    }

    $files = @(Get-ChildItem -LiteralPath $stage -Recurse -File)
    $manifest = @($files | ForEach-Object {
        [ordered]@{ path = $_.FullName.Substring($stage.Length + 1); bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
    })
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $stage 'manifest.json'), (ConvertTo-Json -InputObject $manifest -Depth 3), $utf8)

    if (Test-Path -LiteralPath $OutZip) { Remove-Item -LiteralPath $OutZip -Force }
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $OutZip, [System.IO.Compression.CompressionLevel]::Optimal, $false, [System.Text.Encoding]::UTF8)
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}

$zip = Get-Item -LiteralPath $OutZip
$hash = (Get-FileHash -LiteralPath $OutZip -Algorithm SHA256).Hash
Write-Host ("zip: {0}  files: {1}  bytes: {2}  SHA256: {3}" -f $zip.FullName, ($files.Count + 1), $zip.Length, $hash)

if (Test-Path -LiteralPath $Results) {
    $text = [System.IO.File]::ReadAllText($Results, [System.Text.Encoding]::UTF8)
    $missing = @($ItemId | Where-Object { $text -notmatch ('(?m)^\s*[-*]?\s*' + [regex]::Escape($_) + '\s') })
    if ($missing.Count -gt 0) { Write-Warning ("no result line yet for: " + ($missing -join ', ')) }
    else { Write-Host 'every item has a result line' }
}
