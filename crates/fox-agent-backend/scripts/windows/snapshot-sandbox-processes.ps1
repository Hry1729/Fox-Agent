#Requires -Version 5.1
<#
.SYNOPSIS
  Fox：快照 srt 沙箱账户名下的全部进程（docs/sandbox.md 9 第⑤步，每步记录明确进程数）。

.DESCRIPTION
  需提权运行（管理员 PowerShell）：只有提权进程才能取得其他账户进程的所有者与命令行。
  沙箱账户从 HKLM\SOFTWARE\sandbox-runtime 读取（SandboxUser / SandboxUserSid，srt windows-install 写入）；
  -ExtraSid 可追加历代 SID（Fox 的 srt-sids 台账）。

  方法：Get-CimInstance Win32_Process 枚举全部进程，对每个进程调用 Win32_Process.GetOwnerSid 取所有者 SID
  并与沙箱 SID 比较。取不到所有者的进程（进程已退出、受保护进程、权限不足等）不猜测归属，单独计数并列出
  PID / 名称。

  输出 JSON（-ResultPath，同时写到标准输出）：
    count                沙箱账户名下进程数（明确数字，0 也写；失败时为 null 且 ok=false）
    processes            每个进程：pid、name、parentPid、startTime（UTC ISO 8601）、ownerSid、
                         commandLine（仅 -IncludeCommandLine 时，已脱敏）
    ownerUnknownCount / ownerUnknown   取不到所有者的进程
    capturedAt（UTC）、method、label（-Label，如 "after-host-kill"）、elevated、sandboxUser、sandboxSids、
    totalProcesses（本次枚举到的进程总数）

  脱敏：KEY/TOKEN/SECRET/PASSWORD 结尾的 NAME=值、URL 中的 user:pass@、FOX_ACP_TOKEN、32 位以上十六进制串。
  兼容 Windows PowerShell 5.1：UTF-8（带 BOM）、LF 换行。

.PARAMETER ResultPath
  结果 JSON；默认 %TEMP%\fox-sandbox-processes-<时间>.json。
.PARAMETER Label
  记录在结果里的步骤标签（如 two-sessions、after-host-kill、after-cross-account-kill）。
.PARAMETER IncludeCommandLine
  同时记录（脱敏后的）命令行。默认不记录。
.PARAMETER ExtraSid
  额外视为沙箱账户的 SID（历代 srt 沙箱账户）。
.PARAMETER AllowNonElevated
  诊断用：未提权也继续（结果 elevated=false，所有者多半取不到）。默认未提权即失败。
#>
[CmdletBinding()]
param(
    [string] $ResultPath,
    [string] $Label,
    [switch] $IncludeCommandLine,
    [string[]] $ExtraSid = @(),
    [switch] $AllowNonElevated
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$SrtRegistryKey = 'HKLM:\SOFTWARE\sandbox-runtime'

function Hide-CommandLine([string] $Text) {
    if ($null -eq $Text) { return $null }
    $out = [regex]::Replace($Text, '://[^@/\s"]+@', '://[redacted]@')
    $out = [regex]::Replace($out, '(?i)\b([A-Z0-9_]*(KEY|TOKEN|SECRET|PASSWORD))=("[^"]*"|\S+)', '$1=[redacted]')
    $out = [regex]::Replace($out, '(?i)\b[0-9a-f]{32,}\b', '[redacted-hex]')
    return $out
}

function ConvertTo-IsoTime($Value) {
    if ($null -eq $Value) { return $null }
    try { return ([datetime] $Value).ToUniversalTime().ToString('o') } catch { return [string] $Value }
}

function Get-SandboxAccount {
    $p = Get-ItemProperty -LiteralPath $SrtRegistryKey -ErrorAction Stop
    $names = $p.PSObject.Properties.Name
    $sid = $(if ($names -contains 'SandboxUserSid') { [string] $p.SandboxUserSid } else { $null })
    $user = $(if ($names -contains 'SandboxUser') { [string] $p.SandboxUser } else { $null })
    return @{ sid = $sid; user = $user }
}

$result = [ordered]@{
    ok = $false
    error = $null
    label = $Label
    capturedAt = (Get-Date).ToUniversalTime().ToString('o')
    method = 'Get-CimInstance Win32_Process + Invoke-CimMethod GetOwnerSid (owner SID compared with HKLM\SOFTWARE\sandbox-runtime SandboxUserSid and -ExtraSid)'
    elevated = $null
    sandboxUser = $null
    sandboxSids = @()
    count = $null
    processes = @()
    ownerUnknownCount = $null
    ownerUnknown = @()
    totalProcesses = $null
    commandLineIncluded = [bool] $IncludeCommandLine
}
try {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
    $result.elevated = $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $result.elevated -and -not $AllowNonElevated) { throw 'must run elevated (administrator): owners of other accounts'' processes are not readable otherwise' }

    $account = Get-SandboxAccount
    if (-not $account.sid) { throw "no SandboxUserSid under $SrtRegistryKey (srt windows-install has not run)" }
    $result.sandboxUser = $account.user
    $sids = New-Object System.Collections.Generic.List[string]
    $sids.Add($account.sid)
    foreach ($extra in $ExtraSid) { if ($extra -and -not $sids.Contains($extra)) { $sids.Add($extra) } }
    $result.sandboxSids = $sids.ToArray()

    $all = @(Get-CimInstance -ClassName Win32_Process)
    $result.totalProcesses = $all.Count
    $mine = New-Object System.Collections.Generic.List[object]
    $unknown = New-Object System.Collections.Generic.List[object]
    foreach ($proc in $all) {
        $ownerSid = $null
        try {
            $owner = Invoke-CimMethod -InputObject $proc -MethodName GetOwnerSid -ErrorAction Stop
            if ($owner.ReturnValue -eq 0 -and $owner.Sid) { $ownerSid = [string] $owner.Sid }
        } catch {
            $ownerSid = $null
        }
        if (-not $ownerSid) {
            $unknown.Add([ordered]@{ pid = [int] $proc.ProcessId; name = [string] $proc.Name })
            continue
        }
        if (-not $sids.Contains($ownerSid)) { continue }
        $entry = [ordered]@{
            pid = [int] $proc.ProcessId
            name = [string] $proc.Name
            parentPid = [int] $proc.ParentProcessId
            startTime = ConvertTo-IsoTime $proc.CreationDate
            ownerSid = $ownerSid
        }
        if ($IncludeCommandLine) { $entry.commandLine = Hide-CommandLine ([string] $proc.CommandLine) }
        $mine.Add($entry)
    }
    $result.processes = @($mine | Sort-Object { $_.pid })
    # Index form: on a dictionary, .Count could mean the entry count.
    $result['count'] = $mine.Count
    $result.ownerUnknown = @($unknown | Sort-Object { $_.pid })
    $result.ownerUnknownCount = $unknown.Count
    $result.ok = $true
} catch {
    $result.error = $_.Exception.Message
}

if (-not $ResultPath) { $ResultPath = Join-Path $env:TEMP ('fox-sandbox-processes-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.json') }
$json = $result | ConvertTo-Json -Depth 6
$dir = Split-Path -Parent $ResultPath
if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
[System.IO.File]::WriteAllText($ResultPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output $json
Write-Output ("result: {0}  label={1}  count={2}  ownerUnknown={3}  total={4}" -f $ResultPath, $Label, $result['count'], $result.ownerUnknownCount, $result.totalProcesses)
if ($result.ok) { exit 0 } else { exit 1 }
