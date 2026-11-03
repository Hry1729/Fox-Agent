#Requires -Version 5.1
<#
.SYNOPSIS
    Recent Fox runs and events from fox.db (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Copies fox.db (with -wal / -shm) to a snapshot folder first, so the running Fox is not
    locked or changed, then runs read-only queries on the copy:
      runs            the last -Limit runs (status, error code / message, backend);
      events          the last -EventsPerRun events of each of those runs;
      cancelSwept     every agent_backend.cancel_swept event (killed / outsideJob counts);
      backendAudit    the last 100 rows of agent_backend_audit_log;
      permissionAudit the last 50 rows of permission_audit;
      jobs            the last 20 kernel_jobs rows (background run_command and other jobs);
      toolCalls       the tool cards of those runs (status, output length, streamedOutput /
                      outputTruncated flags, head and tail of result_json) - 13.39 A-2.

    Query engine, first one that works:
      1. sqlite3(.exe) on PATH (or -Sqlite3Path), needs -json (sqlite3 3.33+);
      2. System.Data.SQLite (-SqliteAssembly, or found in the GAC);
      3. Python's built-in sqlite3 module (python / py -3 on PATH);
      4. none: the snapshot and fox-queries.sql stay in -OutDir, so the queries can be run
         later (sqlite3 fox.db < fox-queries.sql) or the snapshot sent instead.
    The engine used and every engine that failed (with its error) are in the result.

.PARAMETER DataDir
    Fox data directory. Default: $env:FOX_DATA_DIR, else %APPDATA%\com.fox.agent.

.PARAMETER OutDir
    Where the snapshot, the SQL file and (with -OutPath unset) nothing else go. Default: a temp folder.

.PARAMETER OutPath
    JSON file to write. Without it the result object is returned.

.PARAMETER KeepSnapshot
    Keep the copied database in OutDir (it holds conversation content; default: deleted
    unless no engine could read it).
#>
[CmdletBinding()]
param(
    [string] $DataDir,
    [string] $OutDir,
    [string] $OutPath,
    [int] $Limit = 20,
    [int] $EventsPerRun = 60,
    [string] $Sqlite3Path,
    [string] $SqliteAssembly,
    [switch] $KeepSnapshot
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if (-not $DataDir) {
    if ($env:FOX_DATA_DIR) { $DataDir = $env:FOX_DATA_DIR } else { $DataDir = Join-Path $env:APPDATA 'com.fox.agent' }
}
if (-not $OutDir) { $OutDir = Join-Path ([System.IO.Path]::GetTempPath()) ('fox-db-' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)

# Windows PowerShell 5.1 turns redirected native stderr into a terminating error under
# ErrorActionPreference=Stop; native tools run with Continue and are judged by exit code.
function Invoke-FoxNative([string] $FilePath, [object[]] $Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $FilePath @Arguments 2>&1 | ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ } }
        return [pscustomobject]@{ Code = $LASTEXITCODE; Text = (($out | Out-String).Trim()) }
    } finally { $ErrorActionPreference = $saved }
}

$ms = 'CASE WHEN {0} > 100000000000 THEN {0} / 1000 ELSE {0} END'
function Ts([string] $Column) { return "datetime(" + ($ms -f $Column) + ", 'unixepoch')" }

$queries = [ordered]@{
    runs = "SELECT r.id, r.conversation_id, c.agent_backend, r.status, r.error_code, r.error_message, r.model, $(Ts 'r.created_at') AS created_utc, $(Ts 'r.started_at') AS started_utc, $(Ts 'r.finished_at') AS finished_utc FROM runs r LEFT JOIN conversations c ON c.id = r.conversation_id ORDER BY r.created_at DESC LIMIT $Limit"
    events = "SELECT run_id, seq, event_type, substr(event_json, 1, 2000) AS event_json, $(Ts 'created_at') AS created_utc FROM (SELECT e.*, ROW_NUMBER() OVER (PARTITION BY e.run_id ORDER BY e.seq DESC) AS n FROM run_events e WHERE e.run_id IN (SELECT id FROM runs ORDER BY created_at DESC LIMIT $Limit)) WHERE n <= $EventsPerRun ORDER BY run_id, seq"
    cancelSwept = "SELECT run_id, seq, event_type, event_json, $(Ts 'created_at') AS created_utc FROM run_events WHERE event_type LIKE '%cancel_swept%' OR event_json LIKE '%cancel_swept%' ORDER BY created_at DESC LIMIT 50"
    backendAudit = "SELECT conversation_id, agent_backend, event, substr(details_json, 1, 2000) AS details_json, $(Ts 'created_at') AS created_utc FROM agent_backend_audit_log ORDER BY created_at DESC LIMIT 100"
    permissionAudit = "SELECT conversation_id, run_id, tool, mode, class, risk, reason, summary, $(Ts 'created_at') AS created_utc FROM permission_audit ORDER BY created_at DESC LIMIT 50"
    jobs = "SELECT * FROM kernel_jobs ORDER BY rowid DESC LIMIT 20"
    toolCalls = "SELECT t.run_id, t.tool_name, t.status, t.execution_location, t.error_message, substr(t.input_json, 1, 300) AS input_head, length(t.result_json) AS result_len, CASE WHEN instr(replace(t.result_json, ': ', ':'), char(34) || 'streamedOutput' || char(34) || ':true') > 0 THEN 1 ELSE 0 END AS streamed_output, CASE WHEN instr(replace(t.result_json, ': ', ':'), char(34) || 'outputTruncated' || char(34) || ':true') > 0 THEN 1 ELSE 0 END AS output_truncated, substr(t.result_json, 1, 1500) AS result_head, CASE WHEN length(t.result_json) > 1500 THEN substr(t.result_json, -600) ELSE NULL END AS result_tail, $(Ts 't.started_at') AS started_utc, $(Ts 't.completed_at') AS completed_utc FROM tool_calls t WHERE t.run_id IN (SELECT id FROM runs ORDER BY created_at DESC LIMIT $Limit) ORDER BY t.started_at DESC LIMIT 60"
}

$sqlFile = Join-Path $OutDir 'fox-queries.sql'
$sqlText = ($queries.Keys | ForEach-Object { "-- $_`n.print '== $_'`n" + $queries[$_] + ";`n" }) -join "`n"
[System.IO.File]::WriteAllText($sqlFile, ".headers on`n.mode line`n" + $sqlText, $utf8)

$result = [ordered]@{
    kind = 'fox-db-report'; version = 1
    takenAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
    dataDir = $DataDir; database = $null; snapshot = $null; engine = $null; engineErrors = @(); queryErrors = [ordered]@{}
    sqlFile = $sqlFile; tables = [ordered]@{}
}

$db = Join-Path $DataDir 'fox.db'
if (-not (Test-Path -LiteralPath $db -PathType Leaf)) {
    $result.engineErrors += "database not found: $db (set -DataDir or FOX_DATA_DIR)"
} else {
    $result.database = $db
    $snapDir = Join-Path $OutDir 'snapshot'
    New-Item -ItemType Directory -Path $snapDir -Force | Out-Null
    foreach ($suffix in @('', '-wal', '-shm')) {
        if (Test-Path -LiteralPath ($db + $suffix)) {
            # FileShare.ReadWrite: Fox keeps the database open.
            $src = New-Object System.IO.FileStream(($db + $suffix), [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]'ReadWrite, Delete')
            try {
                $dst = [System.IO.File]::Create((Join-Path $snapDir ('fox.db' + $suffix)))
                try { $src.CopyTo($dst) } finally { $dst.Dispose() }
            } finally { $src.Dispose() }
        }
    }
    $snapshot = Join-Path $snapDir 'fox.db'
    $result.snapshot = $snapshot

    # 1. sqlite3
    $sqlite3 = $Sqlite3Path
    if (-not $sqlite3) { $cmd = Get-Command sqlite3 -ErrorAction SilentlyContinue; if ($cmd) { $sqlite3 = $cmd.Source } }
    if ($sqlite3 -and -not $result.engine) {
        $previous = [Console]::OutputEncoding
        try {
            [Console]::OutputEncoding = $utf8
            $tables = [ordered]@{}
            foreach ($name in $queries.Keys) {
                $r = Invoke-FoxNative $sqlite3 @('-json', $snapshot, $queries[$name])
                if ($r.Code -ne 0) {
                    if ($r.Text -match 'unknown option|-json') { throw "sqlite3 does not support -json (needs 3.33+): $($r.Text)" }
                    $result.queryErrors[$name] = $r.Text; continue
                }
                # 5.1's ConvertFrom-Json emits a JSON array as one object; re-enumerate it.
                if ($r.Text) { $parsed = $r.Text | ConvertFrom-Json; $tables[$name] = @($parsed | ForEach-Object { $_ }) } else { $tables[$name] = @() }
            }
            $result.tables = $tables
            $result.engine = "sqlite3 ($sqlite3)"
        } catch {
            $result.engineErrors += "sqlite3: $($_.Exception.Message)"
        } finally { [Console]::OutputEncoding = $previous }
    } elseif (-not $sqlite3) { $result.engineErrors += 'sqlite3: not on PATH' }

    # 2. System.Data.SQLite
    if (-not $result.engine) {
        try {
            if ($SqliteAssembly) { Add-Type -Path $SqliteAssembly }
            elseif (-not ('System.Data.SQLite.SQLiteConnection' -as [type])) {
                $loaded = [System.Reflection.Assembly]::LoadWithPartialName('System.Data.SQLite')
                if (-not $loaded) { throw 'System.Data.SQLite not found (pass -SqliteAssembly <path to System.Data.SQLite.dll>)' }
            }
            $connection = New-Object System.Data.SQLite.SQLiteConnection("Data Source=$snapshot;Read Only=False")
            $connection.Open()
            try {
                $tables = [ordered]@{}
                foreach ($name in $queries.Keys) {
                    try {
                        $command = $connection.CreateCommand(); $command.CommandText = $queries[$name]
                        $reader = $command.ExecuteReader()
                        $rows = New-Object System.Collections.Generic.List[object]
                        while ($reader.Read()) {
                            $row = [ordered]@{}
                            for ($i = 0; $i -lt $reader.FieldCount; $i++) { $v = $reader.GetValue($i); if ($v -is [System.DBNull]) { $v = $null }; $row[$reader.GetName($i)] = $v }
                            $rows.Add($row)
                        }
                        $reader.Dispose()
                        $tables[$name] = $rows.ToArray()
                    } catch { $result.queryErrors[$name] = $_.Exception.Message }
                }
                $result.tables = $tables
                $result.engine = 'System.Data.SQLite'
            } finally { $connection.Dispose() }
        } catch { $result.engineErrors += "System.Data.SQLite: $($_.Exception.Message)" }
    }

    # 3. Python sqlite3
    if (-not $result.engine) {
        $python = $null; $pyArgs = @()
        $cmd = Get-Command python -ErrorAction SilentlyContinue
        if ($cmd -and $cmd.Source -notmatch 'WindowsApps') { $python = $cmd.Source }
        else { $cmd = Get-Command py -ErrorAction SilentlyContinue; if ($cmd) { $python = $cmd.Source; $pyArgs = @('-3') } }
        if ($python) {
            $queryJson = Join-Path $OutDir 'queries.json'
            $pyOut = Join-Path $OutDir 'python-result.json'
            $pyScript = Join-Path $OutDir 'fox-db-query.py'
            [System.IO.File]::WriteAllText($queryJson, ($queries | ConvertTo-Json -Depth 3), $utf8)
            [System.IO.File]::WriteAllText($pyScript, @'
import json, sqlite3, sys
db, queries, out = sys.argv[1:4]
connection = sqlite3.connect(db)
connection.row_factory = sqlite3.Row
tables, errors = {}, {}
for name, sql in json.load(open(queries, encoding="utf-8")).items():
    try:
        tables[name] = [dict(row) for row in connection.execute(sql)]
    except Exception as error:
        errors[name] = str(error)
json.dump({"tables": tables, "errors": errors, "sqlite": sqlite3.sqlite_version}, open(out, "w", encoding="utf-8"), ensure_ascii=False, default=str)
'@, $utf8)
            $r = Invoke-FoxNative $python (@($pyArgs) + @($pyScript, $snapshot, $queryJson, $pyOut))
            if ($r.Code -eq 0 -and (Test-Path -LiteralPath $pyOut)) {
                $py = Get-Content -LiteralPath $pyOut -Raw -Encoding UTF8 | ConvertFrom-Json
                $tables = [ordered]@{}
                foreach ($name in $queries.Keys) {
                    if ($null -ne $py.tables.PSObject.Properties[$name]) { $tables[$name] = @($py.tables.$name | ForEach-Object { $_ }) }
                    if ($null -ne $py.errors.PSObject.Properties[$name]) { $result.queryErrors[$name] = $py.errors.$name }
                }
                $result.tables = $tables
                $result.engine = "python sqlite3 $($py.sqlite) ($python)"
                Remove-Item -LiteralPath $pyOut, $queryJson, $pyScript -Force -ErrorAction SilentlyContinue
            } else { $result.engineErrors += "python: exit $($r.Code) $($r.Text)" }
        } else { $result.engineErrors += 'python: not on PATH' }
    }

    if (-not $result.engine) {
        $result.engine = 'none'
        $result.engineErrors += "no engine could read the database; the snapshot is kept at $snapDir. Run: sqlite3 `"$snapshot`" < `"$sqlFile`" (sqlite-tools from https://sqlite.org/download.html), or send the snapshot folder."
    } elseif (-not $KeepSnapshot) {
        Remove-Item -LiteralPath $snapDir -Recurse -Force -ErrorAction SilentlyContinue
        $result.snapshot = $null
    }
}

$summary = [ordered]@{}
foreach ($name in @($result.tables.Keys)) { $summary[$name] = @($result.tables[$name]).Count }
$result['rowCounts'] = $summary

if ($OutPath) {
    $full = [System.IO.Path]::GetFullPath($OutPath)
    $dir = Split-Path -Parent $full
    if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllText($full, ($result | ConvertTo-Json -Depth 8), $utf8)
    Write-Host ("db report: engine {0}; rows {1} -> {2}" -f $result.engine, (($summary.Keys | ForEach-Object { "$_=$($summary[$_])" }) -join ' '), $OutPath)
    foreach ($e in $result.engineErrors) { Write-Host "  engine note: $e" }
} else {
    $result
}
