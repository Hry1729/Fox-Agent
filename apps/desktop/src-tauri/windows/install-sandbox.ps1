#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $PackagePath,
    [Parameter(Mandatory = $true)] [string] $ExpectedSha256,
    [Parameter(Mandatory = $true)] [string] $Version
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$logDir = Join-Path $env:ProgramData 'Fox\logs'
$runId = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + $PID
$logPath = Join-Path $logDir ("sandbox-installer-$runId.log")
$resultPath = Join-Path $PSScriptRoot 'result.json'
try {
    # The inner script may exit; a separate process leaves this wrapper in control.
    & "$PSHOME\powershell.exe" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass `
        -File (Join-Path $PSScriptRoot 'enable-sandbox.ps1') `
        -PackagePath $PackagePath -ExpectedSha256 $ExpectedSha256 -Version $Version `
        -ResultPath $resultPath -LogPath $logPath
    if ($LASTEXITCODE -ne 0) { throw "Sandbox setup exited with code $LASTEXITCODE. See $logPath" }
    $result = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
    if (-not $result.ok -or -not $result.runtimeRoot) {
        throw 'Sandbox setup did not return a successful installation result.'
    }
    $programFilesDir = $env:ProgramW6432
    if (-not $programFilesDir) { $programFilesDir = $env:ProgramFiles }
    $expectedRoot = Join-Path $programFilesDir "Fox\sandbox-runtime\$Version"
    if ($result.version -ne $Version -or $result.runtimeRoot -ne $expectedRoot) {
        throw 'Sandbox setup returned an unexpected runtime path or version.'
    }
    $receipt = Join-Path $expectedRoot '.fox-installer-result.json'
    # A legacy SRT clean-up that did not finish does not stop the install; say so in the log.
    if ($result.legacySrt -and $result.legacySrt.decision -eq 'failed') {
        New-Item -ItemType Directory -Path $logDir -Force | Out-Null
        Add-Content -LiteralPath $logPath -Value ("Installer: legacy SRT clean-up failed: " + $result.legacySrt.error) -Encoding UTF8
    }
    [System.IO.File]::WriteAllText($receipt, ($result | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding($false)))
    & "$env:SystemRoot\System32\icacls.exe" $receipt /reset /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot protect the sandbox installation receipt.' }
    & "$env:SystemRoot\System32\icacls.exe" $receipt /setowner '*S-1-5-32-544' /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot set the sandbox installation receipt owner.' }
    exit 0
} catch {
    try {
        New-Item -ItemType Directory -Path $logDir -Force | Out-Null
        Add-Content -LiteralPath $logPath -Value ("Installer: " + $_.Exception.Message) -Encoding UTF8
    } catch { }
    Write-Error $_ -ErrorAction Continue
    exit 1
}
