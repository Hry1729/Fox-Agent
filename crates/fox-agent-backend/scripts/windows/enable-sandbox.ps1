#Requires -Version 5.1
<#
.SYNOPSIS
  Fox：启用 Windows 沙箱（一次管理员授权，幂等）。

.DESCRIPTION
  Fox 以管理员身份运行本脚本一次（一个 UAC 提示），依次：
    1. 校验运行时包（zip）的 SHA256（先复制到仅管理员可写的暂存目录再校验，避免校验后被替换）；
    2. 解压并安装到 %ProgramFiles%\Fox\sandbox-runtime\<Version>（暂存后整体移动；失败回滚）；
    3. 设置该目录 ACL：去继承；Administrators / SYSTEM 完全控制；Users 读取和执行；所有者 Administrators；
    4. 创建 %ProgramData%\Fox\sandbox 并设 ACL：Administrators / SYSTEM 完全控制；Users 仅
       “创建子文件夹 + 读属性 + 遍历”（只对该目录本身）；CREATOR OWNER 对子项完全控制（仅继承）；
       Users 不能列出或读取他人的目录；
    5. 运行 srt windows-install --proxy-port-range 60080-60089（srt 自身幂等）；
    6. 把结果写成 JSON（-ResultPath），含 srt 沙箱账户 SID，供 Fox 写入 srt-sids 台账与 srt-install 记录。

  只修改本脚本创建或属于 Fox 的目录（运行时版本目录、%ProgramData%\Fox、%ProgramData%\Fox\sandbox）
  的 ACL，不改任何其他目录。每一步写日志（-LogPath，默认 %ProgramData%\Fox\logs）。

  兼容 Windows PowerShell 5.1：文件以 UTF-8（带 BOM）保存、LF 换行；不使用 PowerShell 7 专有语法。
  提权进程可能是另一个管理员账户（标准用户输入管理员凭据），所以与调用用户相关的路径一律由参数传入，
  不读 $env:LOCALAPPDATA / $env:USERPROFILE。

.PARAMETER PackagePath
  运行时包 zip（顶层含 node\、srt\、backends\、dsh-patches\ 等，见 docs/sandbox.md 7.5）。
.PARAMETER ExpectedSha256
  该 zip 的 SHA256（十六进制，大小写不敏感），由 Fox 安装包内置。
.PARAMETER Version
  运行时版本（目录名），只允许 [0-9A-Za-z._-]。
.PARAMETER ResultPath
  结果 JSON 的写入位置（通常是调用用户的 %LOCALAPPDATA%\Fox\state\enable-sandbox-result.json）。
.PARAMETER LogPath
  日志文件；默认 %ProgramData%\Fox\logs\enable-sandbox-<时间>.log。
.PARAMETER ProxyPortRange
  传给 srt windows-install 的端口段，默认 60080-60089（必须与 Fox 写入 srt 设置的一致）。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $PackagePath,
    [Parameter(Mandatory = $true)] [string] $ExpectedSha256,
    [Parameter(Mandatory = $true)] [string] $Version,
    [Parameter(Mandatory = $true)] [string] $ResultPath,
    [string] $LogPath,
    [string] $ProxyPortRange = '60080-60089'
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$SidAdministrators = 'S-1-5-32-544'
$SidSystem = 'S-1-5-18'
$SidUsers = 'S-1-5-32-545'
$SidCreatorOwner = 'S-1-3-0'
$SrtRegistryKey = 'HKLM:\SOFTWARE\sandbox-runtime'
$SrtCliRelative = 'srt\node_modules\@anthropic-ai\sandbox-runtime\dist\cli.js'
$SrtPackageJsonRelative = 'srt\node_modules\@anthropic-ai\sandbox-runtime\package.json'

function Get-ProgramFilesDir {
    # 64-bit Program Files even when started from a 32-bit host.
    if ($env:ProgramW6432) { return $env:ProgramW6432 }
    return $env:ProgramFiles
}

$ProgramDataDir = $env:ProgramData
if (-not $ProgramDataDir) { $ProgramDataDir = 'C:\ProgramData' }
if (-not $LogPath) {
    $logDir = Join-Path (Join-Path $ProgramDataDir 'Fox') 'logs'
    $LogPath = Join-Path $logDir ('enable-sandbox-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
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
    # UTF-8 without BOM so any JSON parser reads it.
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

function Set-AdministratorsOwner([string] $Path, [switch] $Recurse) {
    $icaclsArgs = @($Path, '/setowner', ('*' + $SidAdministrators), '/C', '/Q')
    if ($Recurse) { $icaclsArgs += '/T' }
    $r = Invoke-Native 'icacls.exe' $icaclsArgs
    if ($r.Code -ne 0) { throw "icacls /setowner failed on $Path (exit $($r.Code))" }
}

function Set-RuntimeAcl([string] $Path) {
    # Protected DACL: Administrators / SYSTEM full, Users read+execute; children inherit only.
    $r = Invoke-Native 'icacls.exe' @($Path, '/inheritance:r', '/grant:r',
        ('*' + $SidAdministrators + ':(OI)(CI)F'),
        ('*' + $SidSystem + ':(OI)(CI)F'),
        ('*' + $SidUsers + ':(OI)(CI)RX'), '/C', '/Q')
    if ($r.Code -ne 0) { throw "icacls (runtime ACL) failed on $Path (exit $($r.Code))" }
    $children = Join-Path $Path '*'
    $r = Invoke-Native 'icacls.exe' @($children, '/reset', '/T', '/C', '/Q')
    if ($r.Code -ne 0) { throw "icacls /reset failed below $Path (exit $($r.Code))" }
    Set-AdministratorsOwner $Path -Recurse
}

function Set-FoxDataAcl([string] $Path) {
    # %ProgramData%\Fox: Administrators / SYSTEM full, Users read+execute (no create).
    $r = Invoke-Native 'icacls.exe' @($Path, '/inheritance:r', '/grant:r',
        ('*' + $SidAdministrators + ':(OI)(CI)F'),
        ('*' + $SidSystem + ':(OI)(CI)F'),
        ('*' + $SidUsers + ':(OI)(CI)RX'), '/C', '/Q')
    if ($r.Code -ne 0) { throw "icacls (Fox data ACL) failed on $Path (exit $($r.Code))" }
}

function Set-SandboxBaseAcl([string] $Path) {
    # %ProgramData%\Fox\sandbox: Users may create a subfolder (their own <SID>
    # folder) and stat this folder, nothing else; the creator owns what it creates.
    $r = Invoke-Native 'icacls.exe' @($Path, '/inheritance:r', '/grant:r',
        ('*' + $SidAdministrators + ':(OI)(CI)F'),
        ('*' + $SidSystem + ':(OI)(CI)F'),
        ('*' + $SidUsers + ':(RA,X,AD,S)'),
        ('*' + $SidCreatorOwner + ':(OI)(CI)(IO)F'), '/C', '/Q')
    if ($r.Code -ne 0) { throw "icacls (sandbox base ACL) failed on $Path (exit $($r.Code))" }
}

function Initialize-FoxDirectory([string] $Path, [scriptblock] $SetAcl) {
    # Creates the directory if missing. If it exists but is not owned by
    # Administrators / SYSTEM (pre-created by a user), take it back: it is Fox's namespace.
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
        Write-FoxLog "created $Path"
    }
    $owner = (Get-Acl -LiteralPath $Path).Owner
    $ownerSid = $null
    try { $ownerSid = (New-Object System.Security.Principal.NTAccount($owner)).Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { Write-FoxLog "owner of $Path not translatable: $owner" }
    if ($ownerSid -ne $SidAdministrators -and $ownerSid -ne $SidSystem) {
        Write-FoxLog "taking ownership of $Path (owner was $owner)"
        $r = Invoke-Native 'takeown.exe' @('/F', $Path, '/A')
        if ($r.Code -ne 0) { throw "takeown failed on $Path" }
    }
    & $SetAcl $Path
}

function Get-SrtRegistry {
    if (-not (Test-Path -LiteralPath $SrtRegistryKey)) { return $null }
    $p = Get-ItemProperty -LiteralPath $SrtRegistryKey
    $names = $p.PSObject.Properties.Name
    if (-not ($names -contains 'MarkerVersion') -or -not ($names -contains 'SandboxUserSid')) { return $null }
    $created = $null
    if ($names -contains 'CreatedAt') { $created = [int64] $p.CreatedAt }
    return @{
        markerVersion = [int] $p.MarkerVersion
        sandboxUserSid = [string] $p.SandboxUserSid
        sandboxUser = $(if ($names -contains 'SandboxUser') { [string] $p.SandboxUser } else { $null })
        createdAt = $created
    }
}

$result = @{ ok = $false; version = $Version; error = $null; runtimeRoot = $null; privateBase = $null; srt = $null }
$staging = $null
$backup = $null
$installedNew = $false
try {
    Write-FoxLog "enable-sandbox start: version=$Version package=$PackagePath user=$([System.Security.Principal.WindowsIdentity]::GetCurrent().Name)"
    if (-not (Test-Elevated)) { throw 'must run elevated (administrator)' }
    if ($Version -notmatch '^[0-9A-Za-z._-]+$') { throw "invalid version '$Version'" }
    if ($ProxyPortRange -notmatch '^\d{1,5}-\d{1,5}$') { throw "invalid proxy port range '$ProxyPortRange'" }
    if ($ExpectedSha256 -notmatch '^[0-9A-Fa-f]{64}$') { throw 'ExpectedSha256 must be 64 hex characters' }
    if (-not (Test-Path -LiteralPath $PackagePath -PathType Leaf)) { throw "package not found: $PackagePath" }

    $foxProgramFiles = Join-Path (Get-ProgramFilesDir) 'Fox'
    $runtimeParent = Join-Path $foxProgramFiles 'sandbox-runtime'
    $target = Join-Path $runtimeParent $Version
    $result.runtimeRoot = $target
    foreach ($dir in @($foxProgramFiles, $runtimeParent)) {
        # Inherit Program Files' ACL (Users read+execute); never changed if it exists.
        if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null; Write-FoxLog "created $dir" }
    }

    # 1-2. Runtime: skip the copy when the installed one has the same hash (idempotent).
    $marker = Join-Path $target '.fox-runtime.json'
    $currentHash = ''
    if (Test-Path -LiteralPath $marker) {
        try { $currentHash = [string] (Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json).sha256 } catch { $currentHash = '' }
    }
    if ($currentHash -and $currentHash.ToLowerInvariant() -eq $ExpectedSha256.ToLowerInvariant()) {
        Write-FoxLog "runtime $target already installed with the expected hash"
    } else {
        $staging = Join-Path $runtimeParent ('.staging-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $staging -Force | Out-Null
        $zipCopy = Join-Path $staging 'package.zip'
        Copy-Item -LiteralPath $PackagePath -Destination $zipCopy -Force
        $actual = (Get-FileHash -LiteralPath $zipCopy -Algorithm SHA256).Hash
        Write-FoxLog "package sha256 $actual"
        if ($actual.ToLowerInvariant() -ne $ExpectedSha256.ToLowerInvariant()) { throw "package hash mismatch: expected $ExpectedSha256, got $actual" }
        $content = Join-Path $staging 'content'
        Expand-Archive -LiteralPath $zipCopy -DestinationPath $content -Force
        foreach ($required in @('node\node.exe', $SrtCliRelative)) {
            if (-not (Test-Path -LiteralPath (Join-Path $content $required))) { throw "package lacks $required" }
        }
        $markerJson = @{ version = $Version; sha256 = $ExpectedSha256.ToLowerInvariant(); installedAt = (Get-Date).ToUniversalTime().ToString('o') } | ConvertTo-Json -Compress
        [System.IO.File]::WriteAllText((Join-Path $content '.fox-runtime.json'), $markerJson, (New-Object System.Text.UTF8Encoding($false)))
        if (Test-Path -LiteralPath $target) {
            $backup = Join-Path $runtimeParent ('.old-' + $Version + '-' + [guid]::NewGuid().ToString('N'))
            Move-Item -LiteralPath $target -Destination $backup
            Write-FoxLog "moved previous runtime to $backup"
        }
        Move-Item -LiteralPath $content -Destination $target
        $installedNew = $true
        Write-FoxLog "installed runtime at $target"
    }
    # 3. Runtime ACL (only this version directory and its contents).
    Set-RuntimeAcl $target

    # 4. %ProgramData%\Fox and %ProgramData%\Fox\sandbox.
    $foxData = Join-Path $ProgramDataDir 'Fox'
    $base = Join-Path $foxData 'sandbox'
    $result.privateBase = $base
    Initialize-FoxDirectory $foxData { param($p) Set-FoxDataAcl $p }
    Initialize-FoxDirectory $base { param($p) Set-SandboxBaseAcl $p }

    # 5. srt windows-install (idempotent in srt; never --force over someone else's config).
    $before = Get-SrtRegistry
    $alreadyInstalled = [bool] $before
    $node = Join-Path $target 'node\node.exe'
    $cli = Join-Path $target $SrtCliRelative
    $r = Invoke-Native $node @($cli, 'windows-install', '--proxy-port-range', $ProxyPortRange)
    if ($r.Code -eq 2) { throw 'srt windows-install was cancelled at its UAC prompt' }
    if ($r.Code -ne 0) { throw "srt windows-install failed (exit $($r.Code)): $($r.Output.Trim())" }
    $after = Get-SrtRegistry
    if (-not $after) { throw 'srt windows-install finished but HKLM\SOFTWARE\sandbox-runtime has no MarkerVersion / SandboxUserSid' }
    $srtPackage = Get-Content -LiteralPath (Join-Path $target $SrtPackageJsonRelative) -Raw | ConvertFrom-Json

    # 6. Result.
    $result.srt = @{
        alreadyInstalled = $alreadyInstalled
        packageVersion = [string] $srtPackage.version
        sandboxUserSid = $after.sandboxUserSid
        markerVersion = $after.markerVersion
        createdAt = $after.createdAt
    }
    $result.ok = $true
    if ($backup -and (Test-Path -LiteralPath $backup)) {
        Remove-Item -LiteralPath $backup -Recurse -Force
        Write-FoxLog "removed previous runtime $backup"
    }
    Write-FoxLog 'enable-sandbox done'
} catch {
    $result.error = $_.Exception.Message
    Write-FoxLog ("FAILED: " + $result.error)
    # Roll back what this run copied; never touch anything else.
    if ($installedNew -and $result.runtimeRoot -and (Test-Path -LiteralPath $result.runtimeRoot)) {
        try { Remove-Item -LiteralPath $result.runtimeRoot -Recurse -Force; Write-FoxLog "rolled back $($result.runtimeRoot)" } catch { Write-FoxLog "rollback failed: $($_.Exception.Message)" }
    }
    if ($backup -and (Test-Path -LiteralPath $backup)) {
        try { Move-Item -LiteralPath $backup -Destination $result.runtimeRoot; Write-FoxLog "restored previous runtime" } catch { Write-FoxLog "restore failed: $($_.Exception.Message)" }
    }
} finally {
    if ($staging -and (Test-Path -LiteralPath $staging)) {
        try { Remove-Item -LiteralPath $staging -Recurse -Force } catch { Write-FoxLog "staging cleanup failed: $($_.Exception.Message)" }
    }
}
Write-Result $result
if ($result.ok) { exit 0 } else { exit 1 }
