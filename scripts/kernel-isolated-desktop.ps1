param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [ValidateSet('authoritative', 'legacy')][string]$Mode = 'authoritative'
)
$ErrorActionPreference = 'Stop'
$foxExe = (Resolve-Path -LiteralPath $ExePath).Path
if ((Split-Path -Leaf $foxExe) -ne 'fox-desktop.exe') { throw 'Select the installed fox-desktop.exe.' }
$foxData = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'TestData'))
$foxMarker = Join-Path $foxData '.fox-kernel-acceptance'
if ((Test-Path -LiteralPath (Join-Path $foxData 'fox.db')) -and !(Test-Path -LiteralPath $foxMarker)) {
    throw 'Existing data lacks the isolated-test marker; refusing to start.'
}
$foxLaunchRecord = Join-Path $PSScriptRoot 'last-launch.json'
if (Test-Path -LiteralPath $foxLaunchRecord) {
    $foxPrevious = Get-Content -LiteralPath $foxLaunchRecord -Raw | ConvertFrom-Json
    $foxRunning = Get-Process -Id $foxPrevious.pid -ErrorAction SilentlyContinue
    if ($foxRunning -and (!$foxPrevious.processStartedUtcTicks -or $foxRunning.StartTime.ToUniversalTime().Ticks -eq [Int64]$foxPrevious.processStartedUtcTicks)) {
        throw 'The previous test instance is still running. Exit it before changing mode.'
    }
}
New-Item -ItemType Directory -Path $foxData -Force | Out-Null
Set-Content -LiteralPath $foxMarker -Value 'Isolated Fox acceptance data; retain for rollback checks.' -Encoding UTF8
$foxStart = New-Object System.Diagnostics.ProcessStartInfo
$foxStart.FileName = $foxExe
$foxStart.WorkingDirectory = Split-Path -Parent $foxExe
$foxStart.UseShellExecute = $false
$foxStart.EnvironmentVariables['FOX_DATA_DIR'] = $foxData
$foxStart.EnvironmentVariables['FOX_KERNEL_MODE'] = $Mode
$foxStart.EnvironmentVariables['FOX_KERNEL_ENGINE'] = 'pi'
foreach ($foxOverride in @('FOX_KERNEL_WORKER_BINARY', 'FOX_KERNEL_CODEX_BINARY', 'TAURI_CONFIG', 'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS')) {
    $foxStart.EnvironmentVariables.Remove($foxOverride)
}
$foxProcess = [Diagnostics.Process]::Start($foxStart)
$foxStartedUtc = $foxProcess.StartTime.ToUniversalTime().ToString('o')
[ordered]@{
    pid = $foxProcess.Id
    processStartedUtc = $foxStartedUtc
    processStartedUtcTicks = [string]$foxProcess.StartTime.ToUniversalTime().Ticks
    mode = $Mode
    engine = 'pi'
    executable = $foxExe
    executableSha256 = (Get-FileHash -LiteralPath $foxExe -Algorithm SHA256).Hash
    dataDirectory = $foxData
} | ConvertTo-Json | Set-Content -LiteralPath $foxLaunchRecord -Encoding UTF8
Write-Host "Fox test PID $($foxProcess.Id), mode $Mode"
Write-Host "Test data: $foxData"
Write-Host 'Keep this directory when switching modes. This script does not change system environment settings.'
