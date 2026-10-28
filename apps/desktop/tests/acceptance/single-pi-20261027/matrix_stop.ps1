param(
  [Parameter(Mandatory=$true)][string]$Record,
  [Parameter(Mandatory=$true)][string]$Name
)
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath('D:/python/projects/Fox/output/single-pi-terra-20261028/evidence')
$recordPath = [System.IO.Path]::GetFullPath($Record)
if (-not $recordPath.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Record outside evidence directory' }
if ($Name -notmatch '^[a-z0-9-]+$') { throw 'Invalid record name' }
$out = Join-Path $root "$Name-close.json"
if (Test-Path -LiteralPath $out) { throw 'Close record already exists' }
$launch = Get-Content -Raw -LiteralPath $recordPath | ConvertFrom-Json
$proc = Get-Process -Id ([int]$launch.pid) -ErrorAction Stop
$actualExe = $proc.Path
if ([System.IO.Path]::GetFullPath($actualExe) -ne [System.IO.Path]::GetFullPath($launch.exe)) { throw 'PID does not match the recorded desktop executable' }
if ([math]::Abs(($proc.StartTime - [DateTimeOffset]::Parse($launch.started).LocalDateTime).TotalSeconds) -gt 30) { throw 'PID start time does not match launch record' }
$children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($launch.pid)" | Where-Object { $_.ExecutablePath -and [System.IO.Path]::GetFullPath($_.ExecutablePath) -eq [System.IO.Path]::GetFullPath($launch.sidecarRequested) }
$runtimeIds = @($children | ForEach-Object { [int]$_.ProcessId })
$sent = $proc.CloseMainWindow()
$exited = $proc.WaitForExit(12000)
$remaining = @($runtimeIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
$result = @{pid=[int]$launch.pid;exe=$actualExe;closeMainWindowSent=$sent;exited=$exited;runtimePids=$runtimeIds;runtimeStillAlive=$remaining;checked=(Get-Date).ToString('o')}
$result | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $out
$result | ConvertTo-Json -Depth 4
if (-not $sent -or -not $exited -or $remaining.Count) { exit 1 }
