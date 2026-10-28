param(
  [Parameter(Mandatory=$true)][string]$Data,
  [Parameter(Mandatory=$true)][string]$Name,
  [int]$Port=9222
)
$ErrorActionPreference = 'Stop'
$project = 'D:/python/projects/Fox/Fox'
$recordRoot = 'D:/python/projects/Fox/output/single-pi-terra-20261028/evidence'
if (-not [System.IO.Path]::IsPathFullyQualified($Data)) { throw 'Data must be an absolute path' }
if ($Name -notmatch '^[a-z0-9-]+$') { throw 'Invalid record name' }
$resolvedData = (Resolve-Path -LiteralPath $Data).Path
$allowed = @(
  'C:/Users/hury0424/AppData/Local/Temp/fox-fixture-check/data',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/lifecycle-data-v2',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/lifecycle-fix-data',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/memory-double-data',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/corrupt-data',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/corrupt-fix-data',
  'D:/python/projects/Fox/output/single-pi-terra-20261028/private/corrupt-ui-fix-data'
) | ForEach-Object { [System.IO.Path]::GetFullPath($_) }
if ($allowed -notcontains $resolvedData -or -not (Test-Path -LiteralPath (Join-Path $resolvedData '.fox-single-pi-acceptance'))) { throw 'Data is not an approved isolated fixture' }
$Data = $resolvedData
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { throw 'CDP port already occupied' }
$exe = "$project/apps/desktop/src-tauri/target/debug/fox-desktop.exe"
$sidecar = "$project/services/agent-runtime/dist/fox-agent-runtime-x86_64-pc-windows-msvc.exe"
$env:FOX_DATA_DIR = $Data
$env:FOX_KERNEL_MODE = 'legacy'
$env:FOX_RUNTIME_EXECUTABLE = $sidecar
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
$stdout = "$recordRoot/$Name.stdout.log"
$stderr = "$recordRoot/$Name.stderr.log"
$record = "$recordRoot/$Name-process.json"
if ((Test-Path -LiteralPath $stdout) -or (Test-Path -LiteralPath $stderr) -or (Test-Path -LiteralPath $record)) { throw 'Record already exists' }
$proc = Start-Process -FilePath $exe -WorkingDirectory $project -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Normal -PassThru
$details = @{pid=$proc.Id; exe=$exe; exeSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash; data=$Data; kernelMode='legacy'; sidecarRequested=$sidecar; sidecarSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $sidecar).Hash; cdpPort=$Port; started=(Get-Date).ToString('o'); launcher='Start-Process'; head=(git -C $project rev-parse HEAD); frontendSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath "$project/apps/desktop/src/features/chat/composer/fox-composer.tsx").Hash}
$details | ConvertTo-Json | Set-Content -Encoding utf8 $record
Start-Sleep -Seconds 2
$proc.Refresh()
@{pid=$proc.Id; hasExited=$proc.HasExited; exitCode=if($proc.HasExited){$proc.ExitCode}else{$null}; record=$record} | ConvertTo-Json
if ($proc.HasExited) { exit 1 }
