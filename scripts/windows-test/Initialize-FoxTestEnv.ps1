#Requires -Version 5.1
<#
.SYNOPSIS
    Prepares the whole Fox Windows test root in one go, inside the repo checkout
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Run from a NORMAL (not "Run as administrator") PowerShell after `git pull` and installing
    Fox. Safe to run again: existing folders, the test project, its backup and the results
    file are left as they are; only the Fox-app copy is refreshed.

    Test root: <repo>\docs\windows-test\run-<Version> (derived from this script's folder,
    git-ignored by docs/windows-test/run-*/). Created there:
        <Chinese test project>\readme.txt, sub\readme.txt   seeded only when missing
        backup\project-<Version>\                          copy of the project (Start-FoxBatch restores from it)
        project2\readme.txt                                second project (R2-4)
        shots-<Version>\  logs\  outside\  fox-evidence\
        Fox-app\                                           installed Fox copied from -AppSource
                                                           (robocopy /E, sandbox-runtime excluded)
        results-<Version>.md                               header template, only when missing
    Prints one ok / created / missing / skipped line per item and the next step.

.PARAMETER AppSource
    Installed Fox folder. Default %ProgramFiles%\Fox (64-bit Program Files).

.PARAMETER RefreshBackup
    Copy the current test project over backup\project-<Version> again (robocopy /MIR).

.PARAMETER SkipAppCopy
    Do not touch Fox-app.

.EXAMPLE
    cd <repo>\scripts\windows-test
    .\Initialize-FoxTestEnv.ps1
#>
[CmdletBinding()]
param(
    [string] $Version = '1339',
    [string] $TestRoot,
    [string] $AppSource,
    [switch] $RefreshBackup,
    [switch] $SkipAppCopy
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = [System.IO.Path]::GetFullPath((Join-Path $here '..\..'))
if (-not $TestRoot) { $TestRoot = [System.IO.Path]::GetFullPath((Join-Path $repo "docs\windows-test\run-$Version")) }
if (-not $AppSource) {
    $pf = $env:ProgramW6432; if (-not $pf) { $pf = $env:ProgramFiles }
    $AppSource = Join-Path $pf 'Fox'
}
$utf8 = New-Object System.Text.UTF8Encoding($false)

# Chinese text without putting non-ASCII characters in this file (PowerShell 5.1 reads a
# BOM-less .ps1 in the ANSI code page).
function U([int[]] $Codes) { return -join ($Codes | ForEach-Object { [char]$_ }) }
function FromB64([string] $Text) { return [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($Text)) }
$zhProject = U @(0x6D4B, 0x8BD5, 0x9879, 0x76EE)    # test project (folder name the docs use)

$rows = New-Object System.Collections.Generic.List[object]
function Add-Row([string] $Item, [string] $State, [string] $Detail) {
    $rows.Add([pscustomobject]@{ item = $Item; state = $State; detail = $Detail })
}
function Write-TextIfMissing([string] $File, [string] $Text) {
    if (Test-Path -LiteralPath $File) { return $false }
    New-Item -ItemType Directory -Path (Split-Path -Parent $File) -Force | Out-Null
    [System.IO.File]::WriteAllText($File, $Text, $utf8)
    return $true
}
function Invoke-FoxRobocopy([string[]] $Arguments) {
    $saved = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { & robocopy @Arguments | Out-Null; return $LASTEXITCODE } finally { $ErrorActionPreference = $saved }
}

$principal = New-Object System.Security.Principal.WindowsPrincipal([System.Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'This PowerShell is elevated. Run Initialize-FoxTestEnv.ps1 from a normal PowerShell, so the test folders are owned by you (an admin-owned project changes the dsh / ACL results).'
}

# 1. root and plain folders
$rootExisted = Test-Path -LiteralPath $TestRoot
New-Item -ItemType Directory -Path $TestRoot -Force | Out-Null
Add-Row 'root' $(if ($rootExisted) { 'ok' } else { 'created' }) $TestRoot
foreach ($name in @("shots-$Version", 'logs', 'outside', 'fox-evidence')) {
    $p = Join-Path $TestRoot $name
    $had = Test-Path -LiteralPath $p
    New-Item -ItemType Directory -Path $p -Force | Out-Null
    Add-Row $name $(if ($had) { 'ok' } else { 'created' }) $p
}

# 2. test project (the only non-ASCII folder in the root; Start-FoxBatch finds it by that)
$others = @(Get-ChildItem -LiteralPath $TestRoot -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '[^\x00-\x7F]' -and $_.Name -ne $zhProject })
$project = Join-Path $TestRoot $zhProject
$had = Test-Path -LiteralPath $project
New-Item -ItemType Directory -Path (Join-Path $project 'sub') -Force | Out-Null
# readme.txt: "Fox <ver> test project. Start-FoxBatch.ps1 restores this folder from backup at each batch start."
$readme = "Fox $Version " + (U @(0x6D4B, 0x8BD5, 0x9879, 0x76EE, 0x3002)) + "`r`n" +
    "Start-FoxBatch.ps1 restores this folder from backup\project-$Version at the start of every batch.`r`n"
$seeded = Write-TextIfMissing (Join-Path $project 'readme.txt') $readme
[void](Write-TextIfMissing (Join-Path $project 'sub\readme.txt') ("sub folder (B5-6 junction test)`r`n"))
Add-Row 'project' $(if (-not $had) { 'created' } elseif ($seeded) { 'seeded' } else { 'ok' }) $project
if ($others.Count -gt 0) {
    Add-Row 'project-unique' 'warn' ("other non-ASCII folders in the root confuse Start-FoxBatch: " + (@($others | ForEach-Object { $_.Name }) -join ', '))
}

# 3. backup of the project
$backup = Join-Path $TestRoot "backup\project-$Version"
if ($RefreshBackup -or -not (Test-Path -LiteralPath $backup)) {
    New-Item -ItemType Directory -Path $backup -Force | Out-Null
    $code = Invoke-FoxRobocopy @($project, $backup, '/MIR', '/NFL', '/NDL', '/NJH', '/NJS', '/NP')
    if ($code -ge 8) { Add-Row 'backup' 'FAILED' "robocopy exit $code" }
    else { Add-Row 'backup' $(if ($RefreshBackup) { 'refreshed' } else { 'created' }) $backup }
} else { Add-Row 'backup' 'ok' "$backup (kept; -RefreshBackup copies the project again)" }

# 4. second project
$p2 = Join-Path $TestRoot 'project2'
$made = Write-TextIfMissing (Join-Path $p2 'readme.txt') "Fox $Version project2 (R2-4).`r`n"
Add-Row 'project2' $(if ($made) { 'created' } else { 'ok' }) $p2

# 5. Fox-app copy
$appDir = Join-Path $TestRoot 'Fox-app'
New-Item -ItemType Directory -Path $appDir -Force | Out-Null
if ($SkipAppCopy) { Add-Row 'Fox-app' 'skipped' '-SkipAppCopy' }
elseif (-not (Test-Path -LiteralPath $AppSource)) { Add-Row 'Fox-app' 'missing' "Fox is not installed at $AppSource; install it, then run this script again" }
elseif (@(Get-Process -Name 'Fox', 'fox-desktop' -ErrorAction SilentlyContinue).Count -gt 0) { Add-Row 'Fox-app' 'skipped' 'Fox is running; close it and run this script again to refresh the copy' }
else {
    $code = Invoke-FoxRobocopy @($AppSource, $appDir, '/E', '/XD', 'sandbox-runtime', '/NFL', '/NDL', '/NJH', '/NJS', '/NP')
    if ($code -ge 8) { Add-Row 'Fox-app' 'FAILED' "robocopy exit $code from $AppSource" }
    else { Add-Row 'Fox-app' 'copied' "$AppSource -> $appDir (sandbox-runtime excluded)" }
}
$exeFound = $false
foreach ($name in @('fox-desktop.exe', 'Fox.exe')) {
    $src = Join-Path $AppSource $name; $dst = Join-Path $appDir $name
    if (Test-Path -LiteralPath $dst) {
        $exeFound = $true
        $srcTime = if (Test-Path -LiteralPath $src) { (Get-Item -LiteralPath $src).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss') } else { '-' }
        Add-Row "exe $name" 'ok' ("copy {0}  installed {1}" -f (Get-Item -LiteralPath $dst).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss'), $srcTime)
    }
}
if (-not $exeFound) { Add-Row 'exe' 'missing' "no fox-desktop.exe / Fox.exe in $appDir" }
$rt = Join-Path $AppSource 'sandbox-runtime'
Add-Row 'sandbox-runtime' $(if (Test-Path -LiteralPath $rt) { 'ok' } else { 'missing' }) $rt

# 6. results file header (values left blank; see the results section of the test doc)
$results = Join-Path $TestRoot "results-$Version.md"
$dotted = if ($Version -match '^(\d+)(\d\d)$') { "$($Matches[1]).$($Matches[2])" } else { $Version }
# Lines: title / machine model / Windows version / commit sha / installer SHA256 / runtime SHA256 / Fox integrity.
$template = (FromB64 'Rm94IHtWRH0gV2luZG93cyDlrp7mtYvnu5PmnpzvvIh7Vn3vvIkK5py65Zmo5Z6L5Y+377yaCldpbmRvd3Mg54mI5pys77yaCuaPkOS6pCBzaGHvvJoK5a6J6KOF5YyFIFNIQTI1Nu+8mgrov5DooYzml7YgU0hBMjU277yaMGY5YzE5OWTigKbvvIjlt7LmoKHpqowgLyDmnKrmoKHpqozvvIkKRm94IOWujOaVtOaAp++8mk1lZGl1beOAgeacquaPkOadg++8iEdldC1Gb3hJbnRlZ3JpdHkgb2s9VHJ1Ze+8iQo=').Replace('{VD}', $dotted).Replace('{V}', $Version)
$made = Write-TextIfMissing $results ($template -replace "`n", "`r`n")
Add-Row "results-$Version.md" $(if ($made) { 'created' } else { 'ok' }) $(if ($made) { "$results (header template, fill it in)" } else { "$results (kept)" })

# 7. git: the root must be ignored, and the checkout should be the expected commit
$git = Get-Command git -ErrorAction SilentlyContinue
if ($git) {
    $saved = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    $probe = Join-Path $TestRoot 'results-probe.txt'
    $ign = & git -C $repo check-ignore -q -- $probe 2>$null; $ignCode = $LASTEXITCODE
    $head = (& git -C $repo rev-parse HEAD 2>$null | Out-String).Trim()
    $dirty = @(& git -C $repo status --porcelain --untracked-files=no 2>$null).Count
    $ErrorActionPreference = $saved
    Add-Row 'git-ignored' $(if ($ignCode -eq 0) { 'ok' } else { 'WARN' }) $(if ($ignCode -eq 0) { 'docs/windows-test/run-*/ is ignored' } else { 'run root is NOT ignored by git; do not commit it' })
    Add-Row 'repo HEAD' 'ok' ("$head" + $(if ($dirty -gt 0) { "  ($dirty tracked files modified)" } else { '' }))
} else { Add-Row 'git' 'skipped' 'git not on PATH' }

Write-Host ''
Write-Host "Fox $Version test root: $TestRoot"
foreach ($r in $rows) { Write-Host ("  {0,-22} {1,-9} {2}" -f $r.item, $r.state, $r.detail) }
Write-Host ''
$bad = @($rows | Where-Object { $_.state -in @('missing', 'FAILED', 'WARN') })
if ($bad.Count -gt 0) { Write-Warning ("not ready: " + (@($bad | ForEach-Object { $_.item }) -join ', ')) }
Write-Host 'Use this in both PowerShell windows (normal and admin):'
Write-Host ("  cd '{0}'" -f $here)
Write-Host ("  `$R = '{0}'" -f $TestRoot)
Write-Host 'Next step (normal PowerShell): build the base data dir, see the doc section 4:'
Write-Host "  .\Start-FoxBatch.ps1 -Batch $Version-base -Fresh -KeepProject"
