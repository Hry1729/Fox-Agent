#Requires -Version 5.1
<#
.SYNOPSIS
    Grep Fox logs for cancel sweeps, processes outside the Fox Job and error codes
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Fox writes its diagnostics (lines starting with "fox.external", "fox." ...) to stderr, so
    the log is whatever file stderr was redirected to, e.g.
        & "$env:LOCALAPPDATA\Programs\Fox\Fox.exe" 2> D:\fox-test\fox-stderr.log
    -Path accepts files and directories (directories are searched for *.log / *.txt).

    Reported:
      - cancelSwept: every "cancel_swept" line, parsed into killed / failed / no_baseline /
        outside_job / outside_job_unverified (and the conversation / backend);
      - outsideJob: lines mentioning outside_job / outsideJob with a non-zero count;
      - errorCodes: counts of error codes (code=..., "code":"...", errorCode, error_code,
        agent_backend_* / model.* / executor_route_* identifiers), with the first line seen;
      - fox.external lines (last -Tail of them) for context.

.PARAMETER Path
    Log files or directories.

.PARAMETER Since
    Only files written at or after this time.

.PARAMETER Tail
    How many of the last fox.external lines to keep. Default 200.

.PARAMETER OutPath
    JSON file to write. Without it the result object is returned.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string[]] $Path,
    [datetime] $Since = [datetime]::MinValue,
    [int] $Tail = 200,
    [string] $OutPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Get-FoxLogFiles([string[]] $Paths, [datetime] $After) {
    $files = New-Object System.Collections.Generic.List[string]
    foreach ($p in $Paths) {
        if (Test-Path -LiteralPath $p -PathType Container) {
            foreach ($f in @(Get-ChildItem -LiteralPath $p -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Extension -in @('.log', '.txt') -and $_.LastWriteTime -ge $After })) { $files.Add($f.FullName) }
        } elseif (Test-Path -LiteralPath $p -PathType Leaf) {
            $files.Add((Resolve-Path -LiteralPath $p).ProviderPath)
        } else {
            Write-Warning "log path not found: $p"
        }
    }
    return ,$files
}

function Read-FoxLogLines([string] $File) {
    # Fox writes UTF-8; a BOM-less UTF-8 file must not be read with the ANSI code page (5.1 default).
    $bytes = [System.IO.File]::ReadAllBytes($File)
    $text = (New-Object System.Text.UTF8Encoding($false, $false)).GetString($bytes)
    if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) { $text = [System.Text.Encoding]::Unicode.GetString($bytes, 2, $bytes.Length - 2) }
    return $text -split "`r?`n"
}

$sweepPattern = 'cancel_swept'
$outsidePattern = '(?i)outside_?job'
$codePatterns = @(
    '(?i)\bcode[=:]\s*"?([a-z][a-z0-9_.]{2,})',
    '"(?:code|errorCode|error_code)"\s*:\s*"([A-Za-z][A-Za-z0-9_.]{2,})"',
    '\b(agent_backend_[a-z0-9_]+)\b',
    '\b(executor_route_[a-z0-9_]+)\b',
    '\b(model\.[a-z_]+)\b',
    '\b(sandbox_[a-z0-9_]+)\b'
)

$files = Get-FoxLogFiles $Path $Since
$sweeps = New-Object System.Collections.Generic.List[object]
$outside = New-Object System.Collections.Generic.List[object]
$codes = @{}
$external = New-Object System.Collections.Generic.List[string]
$lineCount = 0

foreach ($file in $files) {
    $n = 0
    foreach ($line in (Read-FoxLogLines $file)) {
        $n++
        if (-not $line) { continue }
        $lineCount++
        if ($line -match '^\s*fox\.external\b' -or $line -match '\bfox\.external\b') {
            $external.Add(('{0}:{1}: {2}' -f [System.IO.Path]::GetFileName($file), $n, $line))
            if ($external.Count -gt $Tail) { $external.RemoveAt(0) }
        }
        if ($line -match $sweepPattern) {
            $entry = [ordered]@{ file = $file; line = $n; text = $line }
            foreach ($key in @('conversation', 'agent_backend', 'killed', 'failed', 'no_baseline', 'outside_job', 'outside_job_unverified')) {
                $m = [regex]::Match($line, '\b' + $key + '=([^\s]+)')
                if ($m.Success) { $entry[$key] = $m.Groups[1].Value }
            }
            $sweeps.Add($entry)
        }
        if ($line -match $outsidePattern) {
            $count = [regex]::Match($line, '(?i)outside_job=(\d+)')
            $jsonCount = [regex]::Match($line, '"outsideJob"\s*:\s*\{\s*"count"\s*:\s*(\d+)')
            $nonZero = ($count.Success -and $count.Groups[1].Value -ne '0') -or ($jsonCount.Success -and $jsonCount.Groups[1].Value -ne '0')
            if ($nonZero -or -not ($count.Success -or $jsonCount.Success)) { $outside.Add([ordered]@{ file = $file; line = $n; text = $line }) }
        }
        $lineCodes = @{}
        foreach ($pattern in $codePatterns) {
            foreach ($m in [regex]::Matches($line, $pattern)) { $lineCodes[$m.Groups[1].Value.TrimEnd('.')] = $true }
        }
        foreach ($code in @($lineCodes.Keys)) {
            if (-not $codes.ContainsKey($code)) { $codes[$code] = [ordered]@{ code = $code; hits = 0; first = ('{0}:{1}: {2}' -f [System.IO.Path]::GetFileName($file), $n, $line) } }
            $codes[$code]['hits'] = [int]$codes[$code]['hits'] + 1
        }
    }
}

$codeRows = @($codes.Values | Sort-Object -Property @{ Expression = { $_['hits'] }; Descending = $true })
$result = [ordered]@{
    kind = 'fox-log-search'
    version = 1
    takenAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
    files = @($files)
    lines = $lineCount
    cancelSweptCount = $sweeps.Count
    cancelSwept = $sweeps.ToArray()
    outsideJobCount = $outside.Count
    outsideJob = $outside.ToArray()
    errorCodes = $codeRows
    foxExternalTail = $external.ToArray()
}

if ($OutPath) {
    $full = [System.IO.Path]::GetFullPath($OutPath)
    $dir = Split-Path -Parent $full
    if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllText($full, ($result | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))
    Write-Host ("log search: {0} files, cancel_swept {1}, outside-job lines {2}, error codes {3} -> {4}" -f @($files).Count, $sweeps.Count, $outside.Count, $codeRows.Count, $OutPath)
} else {
    $result
}
