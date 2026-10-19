#Requires -Version 5.1
<#
.SYNOPSIS
  Fox：停用 Windows 沙箱 / 卸载 Fox 时调用（一次管理员授权，幂等）。

.DESCRIPTION
  只有同时满足以下条件才执行 srt windows-uninstall 并删除 srt 的共享状态目录
  （与 Rust 端 fox_agent_backend::winruntime::decide_srt_uninstall 相同的规则）：
    1. Fox 的记录（<StateDir>\srt-install.json）表明 srt 是 Fox 安装的（installed_by_fox）；
    2. 现在的 HKLM\SOFTWARE\sandbox-runtime 仍是那一次安装：CreatedAt、SandboxUserSid、MarkerVersion 一致；
    3. 运行时里的 srt 包版本与记录一致；
    4. 没有活跃的 srt 会话：没有以沙箱账户运行的进程（需提权才能看到）、没有命令行含
       sandbox-runtime / srt-win 的进程、Fox 的祖先 ACE 台账（win-ancestor-aces.json）没有会话；
    5. 没有其他使用者的证据：其他用户配置文件下不存在 AppData\Local\sandbox-runtime\state.db；
       调用用户自己的 state.db 无法区分 Fox 与其他 srt 客户端（state.db 只记录 broker 的 pid 与进程
       创建时间、ACE 引用与占位文件，不记录客户端程序），因此除非传入 -ConfirmNoOtherSrtClients
       （用户在界面中明确确认本机没有其他使用 sandbox-runtime 的程序），一律视为“无法排除”→ 保留。
  任一条件不满足就保留 srt，只在结果里列出原因。

  不删除任何用户的私有 HOME（%ProgramData%\Fox\sandbox 下的内容一律不动）。
  -RemoveRuntime 时删除 %ProgramFiles%\Fox\sandbox-runtime\<Version>（在 windows-uninstall 之后）。
  卸载前应先由 Fox 运行 repair_sandbox_ownership（见 docs/sandbox.md 7.8）；其中因普通权限被拒
  （needs_elevation）的对象，由 Fox 生成清单并通过 -RepairOwnershipList 交给本脚本在 windows-uninstall
  之前以管理员身份修复（repair-ownership.ps1）。

  兼容 Windows PowerShell 5.1：UTF-8（带 BOM）、LF 换行。

.PARAMETER StateDir
  调用用户的 Fox 状态目录（%LOCALAPPDATA%\Fox\state），由 Fox 传入（提权进程可能是另一个账户）。
.PARAMETER InvokingUserProfile
  调用用户的配置文件目录（%USERPROFILE%），用来区分“自己的”与“其他用户的” state.db。
.PARAMETER Version
  运行时版本（目录名）。
.PARAMETER ResultPath
  结果 JSON 写入位置。
.PARAMETER RemoveRuntime
  同时删除运行时目录。
.PARAMETER ConfirmNoOtherSrtClients
  用户已确认本账户下没有其他使用 sandbox-runtime 的程序。
.PARAMETER LogPath
  日志文件；默认 %ProgramData%\Fox\logs\disable-sandbox-<时间>.log。
.PARAMETER RepairOwnershipList
  可选：Fox 非提权所有权扫描输出的修复清单 JSON；在卸载判断之前执行（repair-ownership.ps1）。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $StateDir,
    [Parameter(Mandatory = $true)] [string] $InvokingUserProfile,
    [Parameter(Mandatory = $true)] [string] $Version,
    [Parameter(Mandatory = $true)] [string] $ResultPath,
    [switch] $RemoveRuntime,
    [switch] $ConfirmNoOtherSrtClients,
    [string] $LogPath,
    [string] $RepairOwnershipList
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$SrtRegistryKey = 'HKLM:\SOFTWARE\sandbox-runtime'
$SrtCliRelative = 'srt\node_modules\@anthropic-ai\sandbox-runtime\dist\cli.js'
$SrtPackageJsonRelative = 'srt\node_modules\@anthropic-ai\sandbox-runtime\package.json'

$ProgramDataDir = $env:ProgramData
if (-not $ProgramDataDir) { $ProgramDataDir = 'C:\ProgramData' }
if (-not $LogPath) {
    $logDir = Join-Path (Join-Path $ProgramDataDir 'Fox') 'logs'
    $LogPath = Join-Path $logDir ('disable-sandbox-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
}

function Write-FoxLog([string] $Message) {
    $line = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK') + ' ' + $Message
    try {
        $dir = Split-Path -Parent $LogPath
        if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        Add-Content -LiteralPath $LogPath -Value $line -Encoding UTF8
    } catch {
        # Logging must never fail the setup itself.
        Write-Verbose ("log write failed: " + $_.Exception.Message)
    }
    Write-Verbose $line
}

function Write-Result([hashtable] $Result) {
    $Result['log'] = $LogPath
    $json = $Result | ConvertTo-Json -Depth 6 -Compress
    $dir = Split-Path -Parent $ResultPath
    if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllText($ResultPath, $json, (New-Object System.Text.UTF8Encoding($false)))
    Write-Output $json
}

function Invoke-Native([string] $FilePath, [string[]] $Arguments) {
    Write-FoxLog ("run: " + $FilePath + ' ' + ($Arguments -join ' '))
    $output = & $FilePath @Arguments 2>&1 | Out-String
    $code = $LASTEXITCODE
    Write-FoxLog ("exit " + $code + ": " + $output.Trim())
    return @{ Code = $code; Output = $output }
}

function Test-Elevated {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-ProgramFilesDir {
    if ($env:ProgramW6432) { return $env:ProgramW6432 }
    return $env:ProgramFiles
}

function Get-SrtRegistry {
    if (-not (Test-Path -LiteralPath $SrtRegistryKey)) { return $null }
    $p = Get-ItemProperty -LiteralPath $SrtRegistryKey
    $names = $p.PSObject.Properties.Name
    if (-not ($names -contains 'MarkerVersion') -or -not ($names -contains 'SandboxUserSid')) { return $null }
    $created = $null
    if ($names -contains 'CreatedAt') { $created = [int64] $p.CreatedAt }
    $user = 'srt-sandbox'
    if ($names -contains 'SandboxUser') { $user = [string] $p.SandboxUser }
    return @{ markerVersion = [int] $p.MarkerVersion; sandboxUserSid = [string] $p.SandboxUserSid; sandboxUser = $user; createdAt = $created }
}

function Read-JsonFile([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    return Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

$result = @{ ok = $false; decision = $null; reasons = @(); uninstalled = $false; removedDirs = @(); runtimeRemoved = $false; error = $null; ownershipRepair = $null }
try {
    Write-FoxLog "disable-sandbox start: version=$Version user=$([System.Security.Principal.WindowsIdentity]::GetCurrent().Name)"
    if (-not (Test-Elevated)) { throw 'must run elevated (administrator)' }
    if ($Version -notmatch '^[0-9A-Za-z._-]+$') { throw "invalid version '$Version'" }
    $runtime = Join-Path (Join-Path (Join-Path (Get-ProgramFilesDir) 'Fox') 'sandbox-runtime') $Version
    if ($RepairOwnershipList) {
        # Before srt may be uninstalled: owners are compared as SID strings, but the repair
        # must run while the recorded srt SIDs are still meaningful to the user.
        try {
            $result.ownershipRepair = & (Join-Path $PSScriptRoot 'repair-ownership.ps1') -ListPath $RepairOwnershipList -LogPath $LogPath
        } catch {
            $result.ownershipRepair = @{ ok = $false; error = $_.Exception.Message }
            Write-FoxLog "ownership repair failed: $($_.Exception.Message)"
        }
    }
    $registry = Get-SrtRegistry
    $reasons = New-Object System.Collections.Generic.List[string]

    if (-not $registry) {
        $result.decision = 'nothing_to_do'
        Write-FoxLog 'srt is not installed'
    } else {
        # 1-3. Fox installed this very installation, at this version.
        $record = $null
        try { $record = Read-JsonFile (Join-Path $StateDir 'srt-install.json') } catch { $reasons.Add("Fox's srt-install.json is unreadable: $($_.Exception.Message)") }
        if (-not $record) {
            $reasons.Add('Fox has no record of installing srt')
        } else {
            if (-not $record.installed_by_fox) { $reasons.Add('srt was already installed when Fox enabled the sandbox') }
            $sameInstall = ([string] $record.sandbox_user_sid -eq $registry.sandboxUserSid) -and ([int] $record.marker_version -eq $registry.markerVersion) -and ("$($record.created_at)" -eq "$($registry.createdAt)")
            if (-not $sameInstall) { $reasons.Add('srt has been reinstalled since Fox installed it (CreatedAt / SID / marker differ)') }
            $packageJson = Join-Path $runtime $SrtPackageJsonRelative
            if (Test-Path -LiteralPath $packageJson) {
                $installedVersion = [string] (Read-JsonFile $packageJson).version
                if ($installedVersion -ne [string] $record.srt_version) { $reasons.Add("srt package version $installedVersion differs from the one Fox installed ($($record.srt_version))") }
            } else {
                $reasons.Add("the srt package version cannot be determined ($packageJson missing)")
            }
        }

        # 4. Active sessions.
        $active = New-Object System.Collections.Generic.List[string]
        try {
            foreach ($proc in Get-Process -IncludeUserName -ErrorAction Stop) {
                if ($proc.UserName -and ($proc.UserName -split '\\')[-1] -ieq $registry.sandboxUser) { $active.Add("$($proc.ProcessName) pid $($proc.Id) as $($proc.UserName)") }
            }
        } catch { $reasons.Add("active srt sessions cannot be ruled out: process owners unreadable ($($_.Exception.Message))") }
        try {
            foreach ($proc in Get-CimInstance Win32_Process -ErrorAction Stop) {
                $line = [string] $proc.CommandLine
                if ($line -and ($line -match 'sandbox-runtime' -or $line -match 'srt-win') -and $proc.ProcessId -ne $PID) { $active.Add("pid $($proc.ProcessId): $($line.Substring(0, [Math]::Min(160, $line.Length)))") }
            }
        } catch { $reasons.Add("active srt sessions cannot be ruled out: command lines unreadable ($($_.Exception.Message))") }
        try {
            $ledger = Read-JsonFile (Join-Path $StateDir 'win-ancestor-aces.json')
            if ($ledger -and $ledger.entries) {
                foreach ($entry in $ledger.entries) {
                    if ($entry.sessions -and @($entry.sessions).Count -gt 0) { $active.Add("Fox ancestor ACE ledger still references sessions on $($entry.path) (run Fox once to recover)"); break }
                }
            }
        } catch { $reasons.Add("Fox's ancestor ACE ledger is unreadable: $($_.Exception.Message)") }
        if ($active.Count -gt 0) { $reasons.Add('srt sessions are active: ' + ($active -join '; ')) }

        # 5. Other users.
        $others = New-Object System.Collections.Generic.List[string]
        $own = (Resolve-Path -LiteralPath $InvokingUserProfile).Path.TrimEnd('\')
        $profiles = Get-CimInstance Win32_UserProfile -ErrorAction SilentlyContinue | Where-Object { $_.LocalPath }
        foreach ($userProfile in $profiles) {
            $local = [string] $userProfile.LocalPath
            if ($local.TrimEnd('\') -ieq $own) { continue }
            $db = Join-Path $local 'AppData\Local\sandbox-runtime\state.db'
            if (Test-Path -LiteralPath $db) { $others.Add($db) }
        }
        if ($others.Count -gt 0) { $reasons.Add('other srt users found: ' + ($others -join ', ')) }
        if (-not $ConfirmNoOtherSrtClients) {
            $reasons.Add('other srt users cannot be ruled out: state.db does not name its client program; confirm in Fox that no other program of this account uses sandbox-runtime')
        }

        if ($reasons.Count -eq 0) {
            $result.decision = 'uninstall'
            $node = Join-Path $runtime 'node\node.exe'
            $cli = Join-Path $runtime $SrtCliRelative
            $r = Invoke-Native $node @($cli, 'windows-uninstall')
            if ($r.Code -eq 2) { throw 'srt windows-uninstall was cancelled at its UAC prompt' }
            if ($r.Code -ne 0) { throw "srt windows-uninstall failed (exit $($r.Code)): $($r.Output.Trim())" }
            $result.uninstalled = $true
            # srt leaves these behind (README "Uninstall"): machine CA material and
            # the invoking user's session DB. Other users' per-user dirs are theirs.
            foreach ($dir in @((Join-Path $ProgramDataDir 'sandbox-runtime'), (Join-Path $InvokingUserProfile 'AppData\Local\sandbox-runtime'))) {
                if (Test-Path -LiteralPath $dir) {
                    Remove-Item -LiteralPath $dir -Recurse -Force
                    $result.removedDirs += $dir
                    Write-FoxLog "removed $dir"
                }
            }
            $recordPath = Join-Path $StateDir 'srt-install.json'
            if (Test-Path -LiteralPath $recordPath) { Remove-Item -LiteralPath $recordPath -Force }
        } else {
            $result.decision = 'keep'
            Write-FoxLog ('keeping srt: ' + ($reasons -join ' | '))
        }
    }
    $result.reasons = @($reasons)

    if ($RemoveRuntime -and (Test-Path -LiteralPath $runtime)) {
        Remove-Item -LiteralPath $runtime -Recurse -Force
        $result.runtimeRemoved = $true
        Write-FoxLog "removed runtime $runtime"
    }
    $result.ok = $true
    Write-FoxLog "disable-sandbox done: $($result.decision)"
} catch {
    $result.error = $_.Exception.Message
    Write-FoxLog ("FAILED: " + $result.error)
}
Write-Result $result
if ($result.ok) { exit 0 } else { exit 1 }
