#Requires -Version 5.1
<#
.SYNOPSIS
    One-shot Fox test evidence: process tree, log grep and database report in one JSON file
    plus a short text summary (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Runs Get-FoxProcessTree.ps1, Search-FoxLog.ps1 and Get-FoxDbReport.ps1 from this folder
    and writes
        <OutDir>\<Label>-<yyyyMMdd-HHmmss>.json   everything
        <OutDir>\<Label>-<yyyyMMdd-HHmmss>.txt    the summary to paste back
    A part that fails is recorded under "errors" and does not stop the others.

    Typical cancel test (Codex / Pi / dsh):
        .\Collect-FoxEvidence.ps1 -Label b2-before -ProcessOnly
        # start the turn, wait for the child process, press Stop, wait 3 s
        .\Collect-FoxEvidence.ps1 -Label b2-after -Baseline .\fox-evidence\b2-before-*.json -LogPath D:\fox-test\fox-stderr.log

.PARAMETER Label
    Short name for the test item, used in the file names (e.g. b2-codex-cancel).

.PARAMETER OutDir
    Default: .\fox-evidence under the current directory.

.PARAMETER LogPath
    Fox stderr log file(s) or folder(s). Without it the log part is skipped.

.PARAMETER DataDir
    Fox data directory (default $env:FOX_DATA_DIR, else %APPDATA%\com.fox.agent).

.PARAMETER Baseline
    An earlier evidence JSON (from this script) or process snapshot JSON; processes new since
    then are listed. Wildcards are allowed; the newest match is used.

.PARAMETER ProcessOnly
    Only the process snapshot (fast; use right before the action under test).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $Label,
    [string] $OutDir = (Join-Path (Get-Location).Path 'fox-evidence'),
    [string[]] $LogPath,
    [string] $DataDir,
    [string] $Baseline,
    [datetime] $LogSince = [datetime]::MinValue,
    [switch] $ProcessOnly,
    [string[]] $FoxProcessName = @('Fox', 'fox-desktop')
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$utf8 = New-Object System.Text.UTF8Encoding($false)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$safeLabel = ($Label -replace '[^A-Za-z0-9._-]', '_')
$base = Join-Path $OutDir ("$safeLabel-$stamp")
$work = $base + '.parts'
New-Item -ItemType Directory -Path $work -Force | Out-Null

$evidence = [ordered]@{
    kind = 'fox-evidence'; version = 1; label = $Label
    takenAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
    localTime = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    machine = [ordered]@{
        computer = $env:COMPUTERNAME
        os = [System.Environment]::OSVersion.VersionString
        psVersion = $PSVersionTable.PSVersion.ToString()
        psEdition = $(if ($PSVersionTable.ContainsKey('PSEdition')) { $PSVersionTable.PSEdition } else { 'Desktop' })
        user = $(try { [System.Security.Principal.WindowsIdentity]::GetCurrent().Name } catch { [string]$env:USERNAME })
        runtimes = @()
    }
    process = $null; log = $null; db = $null; errors = [ordered]@{}
}
try {
    $programFiles = $env:ProgramW6432
    if (-not $programFiles) { $programFiles = $env:ProgramFiles }
    $rt = Join-Path $programFiles 'Fox\sandbox-runtime'
    if (Test-Path -LiteralPath $rt) { $evidence.machine.runtimes = @(Get-ChildItem -LiteralPath $rt -Directory | ForEach-Object { $_.Name }) }
} catch { }

function Read-FoxJson([string] $File) { return Get-Content -LiteralPath $File -Raw -Encoding UTF8 | ConvertFrom-Json }

# Baseline: an evidence file (has .process) or a bare process snapshot.
$baselineFile = $null
if ($Baseline) {
    $match = @(Get-ChildItem -Path $Baseline -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending)
    if ($match.Count -eq 0) { $evidence.errors['baseline'] = "no file matches $Baseline" }
    else {
        $b = Read-FoxJson $match[0].FullName
        if ($null -ne $b.PSObject.Properties['process'] -and $b.process) {
            $baselineFile = Join-Path $work 'baseline-process.json'
            [System.IO.File]::WriteAllText($baselineFile, ($b.process | ConvertTo-Json -Depth 8), $utf8)
        } else { $baselineFile = $match[0].FullName }
        $evidence['baseline'] = $match[0].FullName
    }
}

# 1. Process tree
try {
    $procOut = Join-Path $work 'process.json'
    $args1 = @{ OutPath = $procOut; FoxProcessName = $FoxProcessName }
    if ($baselineFile) { $args1['Baseline'] = $baselineFile }
    & (Join-Path $here 'Get-FoxProcessTree.ps1') @args1 | Out-Null
    $evidence.process = Read-FoxJson $procOut
} catch { $evidence.errors['process'] = $_.Exception.Message }

if (-not $ProcessOnly) {
    # 2. Logs
    if ($LogPath) {
        try {
            $logOut = Join-Path $work 'log.json'
            & (Join-Path $here 'Search-FoxLog.ps1') -Path $LogPath -Since $LogSince -OutPath $logOut | Out-Null
            $evidence.log = Read-FoxJson $logOut
        } catch { $evidence.errors['log'] = $_.Exception.Message }
    } else { $evidence.errors['log'] = 'skipped: no -LogPath (Fox stderr log)' }

    # 3. Database
    try {
        $dbOut = Join-Path $work 'db.json'
        $args3 = @{ OutPath = $dbOut; OutDir = (Join-Path $work 'db') }
        if ($DataDir) { $args3['DataDir'] = $DataDir }
        & (Join-Path $here 'Get-FoxDbReport.ps1') @args3 | Out-Null
        $evidence.db = Read-FoxJson $dbOut
    } catch { $evidence.errors['db'] = $_.Exception.Message }
}

[System.IO.File]::WriteAllText($base + '.json', ($evidence | ConvertTo-Json -Depth 10), $utf8)

# Text summary
$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("Fox evidence: $Label  ($($evidence.localTime), $($evidence.machine.os), PowerShell $($evidence.machine.psVersion))")
$lines.Add("file: $([System.IO.Path]::GetFileName($base + '.json'))")
if ($evidence.machine.runtimes) { $lines.Add("runtime packages: " + (@($evidence.machine.runtimes) -join ', ')) }
$p = $evidence.process
if ($p) {
    $lines.Add('')
    $lines.Add("[process] Fox hosts: $(@($p.foxHostPids) -join ',')  Fox Job members: $($p.foxJobMemberCount)  suspects: $($p.suspectCount)  elevated: $($p.elevated)")
    if ($p.foxJobError) { $lines.Add("  Fox Job lookup failed: $($p.foxJobError)") }
    foreach ($s in @($p.suspects)) {
        if (-not $s) { continue }
        $lines.Add(("  SUSPECT pid={0} ppid={1} {2} account={3} inAnyJob={4} reasons={5} cmd={6}" -f $s.pid, $s.ppid, $s.name, $s.account, $s.inAnyJob, (@($s.suspectReasons) -join '+'), $s.commandLine))
    }
    if ($null -ne $p.PSObject.Properties['comparedToBaseline'] -and $p.comparedToBaseline) {
        $c = $p.comparedToBaseline
        $lines.Add("  since baseline: $($c.newSinceBaseline) new processes, Fox-related still alive: $(@($c.newFoxRelated | Where-Object { $_ }).Count)")
        foreach ($n in @($c.newFoxRelated)) {
            if (-not $n) { continue }
            $lines.Add(("    ALIVE pid={0} ppid={1} {2} created={3} account={4} inFoxJob={5} reasons={6} cmd={7}" -f $n.pid, $n.ppid, $n.name, $n.created, $n.account, $n.inFoxJob, (@($n.suspectReasons) -join '+'), $n.commandLine))
        }
    }
}
$l = $evidence.log
if ($l) {
    $lines.Add('')
    $lines.Add("[log] files: $(@($l.files).Count)  cancel_swept: $($l.cancelSweptCount)  outside-job lines: $($l.outsideJobCount)")
    foreach ($s in @($l.cancelSwept | Select-Object -Last 10)) { if ($s) { $lines.Add("  " + $s.text) } }
    foreach ($o in @($l.outsideJob | Select-Object -Last 10)) { if ($o) { $lines.Add("  OUTSIDE " + $o.text) } }
    foreach ($e in @($l.errorCodes | Select-Object -First 15)) { if ($e) { $lines.Add(("  code {0} x{1}" -f $e.code, $e.hits)) } }
}
$d = $evidence.db
if ($d) {
    $lines.Add('')
    $lines.Add("[db] engine: $($d.engine)  data dir: $($d.dataDir)")
    foreach ($note in @($d.engineErrors)) { if ($note) { $lines.Add("  note: $note") } }
    $tables = $d.tables
    if ($tables -and $null -ne $tables.PSObject.Properties['runs']) {
        foreach ($r in @($tables.runs | Select-Object -First 8)) {
            if ($r) { $lines.Add(("  run {0} {1} {2} status={3} error={4} {5}" -f $r.created_utc, $r.agent_backend, $r.id, $r.status, $r.error_code, $r.error_message)) }
        }
    }
    if ($tables -and $null -ne $tables.PSObject.Properties['cancelSwept']) {
        foreach ($e in @($tables.cancelSwept | Select-Object -First 5)) { if ($e) { $lines.Add(("  sweep {0} run={1} {2}" -f $e.created_utc, $e.run_id, $e.event_json)) } }
    }
}
foreach ($k in @($evidence.errors.Keys)) { $lines.Add("[error] ${k}: $($evidence.errors[$k])") }
[System.IO.File]::WriteAllText($base + '.txt', ($lines -join "`r`n") + "`r`n", $utf8)
if ($d -and $d.engine -eq 'none') {
    Write-Host "database snapshot kept for manual queries: $work\db"
} else {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
$lines | ForEach-Object { Write-Host $_ }
Write-Host ""
Write-Host "evidence: $base.json"
Write-Host "summary:  $base.txt"
