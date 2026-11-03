#Requires -Version 5.1
<#
.SYNOPSIS
    Resets and starts Fox for one test batch, as a normal user at Medium integrity
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Run this from a NORMAL (not "Run as administrator") PowerShell. It refuses to run
    elevated, because Fox started from an elevated shell runs at High integrity.

      1. checks that no Fox process is running (close Fox first);
      2. data directory <TestRoot>\data-<Batch>: copied from -BaseDataDir unless it exists
         already (-Fresh: start from an empty directory instead; -Reset: delete and copy again);
      3. project: -ProjectDir is mirrored from -ProjectBackup (robocopy /MIR) unless -KeepProject;
      4. clears FOX_DEV_UI, sets FOX_DATA_DIR and starts the exe with stderr redirected to
         <TestRoot>\logs\fox-<Batch>.log (appends -<n> when that log exists);
      5. waits a few seconds and prints Get-FoxIntegrity.ps1 (must say ok=True).

.EXAMPLE
    .\Start-FoxBatch.ps1 -Batch R1
    .\Start-FoxBatch.ps1 -Batch B3-4 -Fresh
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $Batch,
    [string] $TestRoot = 'D:\fox-test',
    [string] $Version = '1339',
    [string] $BaseDataDir,
    [string] $ProjectDir,
    [string] $ProjectBackup,
    [string] $Exe,
    [switch] $Fresh,
    [switch] $Reset,
    [switch] $KeepProject,
    [hashtable] $ExtraEnv
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$principal = New-Object System.Security.Principal.WindowsPrincipal([System.Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'This PowerShell is elevated. Open a normal PowerShell (not Run as administrator) and run Start-FoxBatch.ps1 there, so Fox runs at Medium integrity.'
}
$running = @(Get-Process -Name 'Fox', 'fox-desktop' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) { throw "Fox is still running (pid $(@($running | ForEach-Object { $_.Id }) -join ',')). Close it, check Get-FoxProcessTree.ps1, then run again." }

if (-not $BaseDataDir) { $BaseDataDir = Join-Path $TestRoot "data-$Version-base" }
# The test project folder name is Chinese; find it under TestRoot when not given.
if (-not $ProjectBackup) { $ProjectBackup = Join-Path $TestRoot "backup\project-$Version" }
if (-not $ProjectDir) {
    $candidates = @(Get-ChildItem -LiteralPath $TestRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '[^\x00-\x7F]' } | Select-Object -First 1)
    if ($candidates.Count -gt 0) { $ProjectDir = $candidates[0].FullName }
}
if (-not $Exe) {
    foreach ($name in @('fox-desktop.exe', 'Fox.exe')) {
        $p = Join-Path $TestRoot "Fox-app\$name"
        if (Test-Path -LiteralPath $p) { $Exe = $p; break }
    }
}
if (-not $Exe) { throw "No exe found in $TestRoot\Fox-app (pass -Exe)." }

$dataDir = Join-Path $TestRoot "data-$Batch"
if ($Reset -and (Test-Path -LiteralPath $dataDir)) { Remove-Item -LiteralPath $dataDir -Recurse -Force }
if (-not (Test-Path -LiteralPath $dataDir)) {
    if ($Fresh) { New-Item -ItemType Directory -Path $dataDir | Out-Null; Write-Host "data: new empty $dataDir" }
    else {
        if (-not (Test-Path -LiteralPath $BaseDataDir)) { throw "Base data dir $BaseDataDir not found (see the common preparation, or pass -Fresh)." }
        Copy-Item -LiteralPath $BaseDataDir -Destination $dataDir -Recurse
        Write-Host "data: copied $BaseDataDir -> $dataDir"
    }
} else { Write-Host "data: reusing $dataDir (pass -Reset to copy the base again)" }

if (-not $KeepProject -and $ProjectDir -and (Test-Path -LiteralPath $ProjectBackup)) {
    $saved = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    & robocopy $ProjectBackup $ProjectDir /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
    $code = $LASTEXITCODE; $ErrorActionPreference = $saved
    if ($code -ge 8) { throw "robocopy failed with exit code $code" }
    Write-Host "project: $ProjectDir restored from $ProjectBackup"
} elseif (-not $KeepProject) { Write-Host "project: not restored (project or backup folder not found)" }

$logDir = Join-Path $TestRoot 'logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$log = Join-Path $logDir "fox-$Batch.log"
$n = 1
while (Test-Path -LiteralPath $log) { $n++; $log = Join-Path $logDir "fox-$Batch-$n.log" }

Remove-Item Env:\FOX_DEV_UI -ErrorAction SilentlyContinue
$env:FOX_DATA_DIR = $dataDir
if ($ExtraEnv) { foreach ($k in $ExtraEnv.Keys) { Set-Item -Path "Env:\$k" -Value ([string]$ExtraEnv[$k]) } }
Start-Process -FilePath $Exe -RedirectStandardError $log | Out-Null
Write-Host "started: $Exe"
Write-Host "FOX_DATA_DIR=$dataDir"
Write-Host "log: $log"
Start-Sleep -Seconds 6
& (Join-Path $here 'Get-FoxIntegrity.ps1') | Out-Null
