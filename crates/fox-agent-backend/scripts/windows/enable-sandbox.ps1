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
    5. 运行 srt windows-install --proxy-port-range 60080-60089 --skip-profile-grant（不授权用户主目录）；
    6. 把结果写成 JSON（-ResultPath），含 srt 沙箱账户 SID，供 Fox 写入 srt-sids 台账与 srt-install 记录；
    7. 可选：-RepairOwnershipList 给出 Fox 非提权扫描输出的所有权修复清单时，以管理员身份逐项修复
       （repair-ownership.ps1，见 docs/sandbox.md 7.8）。

  解压（第五轮修复）：先按包内条目名做路径长度检查（目标目录 + 相对路径 < -MaxPathLength，超出即失败并
  列出文件，不解压）；优先 %SystemRoot%\System32\tar.exe -xf（bsdtar，支持超过 260 字符的路径），失败再退回
  Expand-Archive。暂存目录取短名（%ProgramFiles%\Fox\.s<8 位>\c），不比最终目录长。任何异常先把原始错误
  （类型、消息、步骤、脚本位置）写日志并记入结果 JSON 的 primaryError，再做回滚与清理；清理失败单独记入
  cleanupErrors，不覆盖原始错误。暂存目录的删除在 Remove-Item 失败时改用 cmd rd /s /q "\\?\..."（长路径）。

  失败处理（第六轮）：“保留原始错误 + 回滚/清理”抽成 Resolve-FoxFailure：先把原始错误写入结果
  （error / primaryError）与日志，再逐项运行回滚/清理脚本块；任一项抛出只记入 cleanupErrors，绝不覆盖
  primaryError。结果 JSON 另含 stagingDir（本次暂存目录，便于排查残留）。

  测试用故障注入（仅测试；生产路径不受影响）：环境变量 FOX_SCRIPT_TEST_FAULTS 为逗号分隔的注入点列表
  （extract、cleanup）。extract：解压开始时抛出注入错误；cleanup：Remove-FoxTree 不删除并记一条清理错误。
  未设置该变量（或为空）时注入点是空操作；设置时日志首行会写明已启用的注入点。用于验证“解压与清理同时
  失败”时 primaryError 为解压错误、cleanupErrors 含清理错误（scripts/windows/tests/test-enable-faults.ps1）。

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
.PARAMETER RepairOwnershipList
  可选：Fox 非提权所有权扫描输出的修复清单 JSON（fox_agent_backend::winowner::ownership_repair_list）。
  只处理清单内条目，且读回确认当前所有者是清单记录的 srt SID 后，才把所有者改为清单给定的用户 SID。
.PARAMETER MaxPathLength
  解压后任一文件完整路径允许的最大长度（不含），默认 260（MAX_PATH）。构建脚本用更严的 240 预算。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $PackagePath,
    [Parameter(Mandatory = $true)] [string] $ExpectedSha256,
    [Parameter(Mandatory = $true)] [string] $Version,
    [Parameter(Mandatory = $true)] [string] $ResultPath,
    [string] $LogPath,
    [string] $ProxyPortRange = '60080-60089',
    [string] $RepairOwnershipList,
    [int] $MaxPathLength = 260
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
    Get-Command $FilePath -ErrorAction Stop | Out-Null
    $previousErrorAction = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 wraps native stderr as ErrorRecord objects.
        # Capture diagnostics and use the exit code, including success warnings.
        $ErrorActionPreference = 'Continue'
        $global:LASTEXITCODE = $null
        $output = & $FilePath @Arguments 2>&1 | Out-String
        $code = $global:LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($null -eq $code) { throw "Failed to start native command: $FilePath" }
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

function Get-PackageLongPath([string] $ZipPath, [string[]] $Prefixes, [int] $Limit) {
    # Path budget from the zip entry names alone (no extraction, no long-path API needed).
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    $long = New-Object System.Collections.Generic.List[object]
    $max = 0
    try {
        foreach ($entry in $zip.Entries) {
            $relative = $entry.FullName.Replace('/', '\').TrimEnd('\')
            if (-not $relative) { continue }
            foreach ($prefix in $Prefixes) {
                $full = (Join-Path $prefix $relative)
                if ($full.Length -gt $max) { $max = $full.Length }
                if ($full.Length -ge $Limit) { $long.Add(@{ path = $full; length = $full.Length }); break }
            }
        }
    } finally {
        $zip.Dispose()
    }
    return @{ maxLength = $max; tooLong = @($long | Sort-Object { $_.length } -Descending | Select-Object -First 20); tooLongCount = $long.Count }
}

function Expand-FoxPackage([string] $ZipPath, [string] $Destination) {
    # tar.exe (bsdtar, Windows 10 1803+) handles paths over 260 characters; Expand-Archive on
    # Windows PowerShell 5.1 does not (DirectoryNotFoundException on the long DeepSeek paths).
    Invoke-FoxTestFault 'extract'
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (Test-Path -LiteralPath $tar -PathType Leaf) {
        $r = Invoke-Native $tar @('-xf', $ZipPath, '-C', $Destination)
        if ($r.Code -eq 0) { return 'tar' }
        Write-FoxLog "tar.exe failed (exit $($r.Code)); falling back to Expand-Archive"
        Remove-FoxTree $Destination 'partial tar extraction'
        New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    } else {
        Write-FoxLog "$tar not found; using Expand-Archive"
    }
    Expand-Archive -LiteralPath $ZipPath -DestinationPath $Destination -Force
    return 'Expand-Archive'
}

$script:CleanupErrors = New-Object System.Collections.Generic.List[string]

function Get-FoxTestFault {
    # TEST ONLY: injection points named in FOX_SCRIPT_TEST_FAULTS (comma-separated,
    # e.g. "extract,cleanup"). Empty when the variable is unset: production runs inject nothing.
    $raw = [string] $env:FOX_SCRIPT_TEST_FAULTS
    if (-not $raw.Trim()) { return @() }
    return @($raw.Split(',') | ForEach-Object { $_.Trim().ToLowerInvariant() } | Where-Object { $_ })
}

function Test-FoxTestFault([string] $Point) {
    # $true only when the test variable names this point.
    return (@($script:FoxTestFaults) -contains $Point)
}

function Invoke-FoxTestFault([string] $Point) {
    # No-op unless FOX_SCRIPT_TEST_FAULTS names $Point; then throws a recognisable error.
    if (Test-FoxTestFault $Point) { throw "FOX_SCRIPT_TEST_FAULTS: injected $Point failure" }
}

function Resolve-FoxFailure {
    # Record the original error FIRST (result + log), then run each roll-back / clean-up step.
    # A step that throws is recorded in cleanupErrors and never replaces primaryError.
    param(
        [Parameter(Mandatory = $true)] [hashtable] $Result,
        [Parameter(Mandatory = $true)] [System.Management.Automation.ErrorRecord] $ErrorRecord,
        [Parameter(Mandatory = $true)] [string] $Step,
        [System.Collections.IDictionary] $Cleanup
    )
    $exception = $ErrorRecord.Exception
    $Result['error'] = $exception.Message
    $Result['primaryError'] = @{
        step = $Step
        type = $exception.GetType().FullName
        message = $exception.Message
        inner = $(if ($exception.InnerException) { $exception.InnerException.Message } else { $null })
        at = [string] $ErrorRecord.InvocationInfo.PositionMessage
    }
    Write-FoxLog ("FAILED at step ${Step}: " + $exception.GetType().FullName + ': ' + $exception.Message)
    if ($ErrorRecord.ScriptStackTrace) { Write-FoxLog ("stack: " + $ErrorRecord.ScriptStackTrace) }
    if ($Cleanup) {
        foreach ($name in @($Cleanup.Keys)) {
            try {
                & $Cleanup[$name]
            } catch {
                $script:CleanupErrors.Add("${name}: $($_.Exception.Message)")
                Write-FoxLog "clean-up step '$name' failed: $($_.Exception.Message)"
            }
        }
    }
}

function Remove-FoxTree([string] $Path, [string] $What) {
    # Never throws: a clean-up failure is recorded on its own and must not replace the
    # primary error. Falls back to rd /s /q on the \\?\ form for paths over 260 characters.
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return }
    if (Test-FoxTestFault 'cleanup') {
        $script:CleanupErrors.Add("$What ${Path}: injected cleanup failure (FOX_SCRIPT_TEST_FAULTS)")
        Write-FoxLog "test fault: not removing $What $Path"
        return
    }
    try {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
        Write-FoxLog "removed $What $Path"
        return
    } catch {
        Write-FoxLog "Remove-Item failed for $What ${Path}: $($_.Exception.GetType().FullName): $($_.Exception.Message); retrying with rd /s /q"
    }
    $long = '\\?\' + $Path
    $r = Invoke-Native "$env:SystemRoot\System32\cmd.exe" @('/d', '/c', 'rd', '/s', '/q', $long)
    if ((Test-Path -LiteralPath $Path) -or $r.Code -ne 0) {
        $script:CleanupErrors.Add("$What ${Path}: could not be removed (rd exit $($r.Code))")
    } else {
        Write-FoxLog "removed $What $Path (rd)"
    }
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

$script:FoxTestFaults = Get-FoxTestFault
$result = @{ ok = $false; version = $Version; error = $null; primaryError = $null; cleanupErrors = @(); runtimeRoot = $null; privateBase = $null; srt = $null; extractMethod = $null; packagePaths = $null; ownershipRepair = $null; stagingDir = $null }
$staging = $null
$backup = $null
$installedNew = $false
$step = 'start'
try {
    Write-FoxLog "enable-sandbox start: version=$Version package=$PackagePath user=$([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) ps=$($PSVersionTable.PSVersion)"
    if (@($script:FoxTestFaults).Count -gt 0) { Write-FoxLog ("TEST FAULTS ACTIVE (FOX_SCRIPT_TEST_FAULTS): " + (@($script:FoxTestFaults) -join ',')) }
    $step = 'validate'
    if (-not (Test-Elevated)) { throw 'must run elevated (administrator)' }
    if ($Version -notmatch '^[0-9A-Za-z._-]+$') { throw "invalid version '$Version'" }
    if ($ProxyPortRange -notmatch '^\d{1,5}-\d{1,5}$') { throw "invalid proxy port range '$ProxyPortRange'" }
    if ($ExpectedSha256 -notmatch '^[0-9A-Fa-f]{64}$') { throw 'ExpectedSha256 must be 64 hex characters' }
    if (-not (Test-Path -LiteralPath $PackagePath -PathType Leaf)) { throw "package not found: $PackagePath" }
    if ($RepairOwnershipList -and -not (Test-Path -LiteralPath $RepairOwnershipList -PathType Leaf)) { throw "repair list not found: $RepairOwnershipList" }

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
        $step = 'stage'
        # Short staging name next to sandbox-runtime: never longer than the final directory.
        $staging = Join-Path $foxProgramFiles ('.s' + [guid]::NewGuid().ToString('N').Substring(0, 8))
        New-Item -ItemType Directory -Path $staging -Force | Out-Null
        $result.stagingDir = $staging
        $zipCopy = Join-Path $staging 'p.zip'
        Copy-Item -LiteralPath $PackagePath -Destination $zipCopy -Force
        $step = 'verify-hash'
        $actual = (Get-FileHash -LiteralPath $zipCopy -Algorithm SHA256).Hash
        Write-FoxLog "package sha256 $actual"
        if ($actual.ToLowerInvariant() -ne $ExpectedSha256.ToLowerInvariant()) { throw "package hash mismatch: expected $ExpectedSha256, got $actual" }
        $content = Join-Path $staging 'c'
        $step = 'path-budget'
        $paths = Get-PackageLongPath $zipCopy @($target, $content) $MaxPathLength
        $result.packagePaths = @{ maxLength = $paths.maxLength; limit = $MaxPathLength; tooLongCount = $paths.tooLongCount; tooLong = $paths.tooLong }
        Write-FoxLog "longest installed path: $($paths.maxLength) characters (limit $MaxPathLength)"
        if ($paths.tooLongCount -gt 0) {
            foreach ($item in $paths.tooLong) { Write-FoxLog "path too long ($($item.length)): $($item.path)" }
            throw "package_path_too_long: $($paths.tooLongCount) file(s) would exceed $MaxPathLength characters; rebuild the runtime package (build-runtime-package.py check)"
        }
        $step = 'extract'
        $result.extractMethod = Expand-FoxPackage $zipCopy $content
        Write-FoxLog "extracted with $($result.extractMethod)"
        foreach ($required in @('node\node.exe', $SrtCliRelative, 'acp-bridge\bridge.mjs')) {
            if (-not (Test-Path -LiteralPath (Join-Path $content $required))) { throw "package lacks $required" }
        }
        $markerJson = @{ version = $Version; sha256 = $ExpectedSha256.ToLowerInvariant(); installedAt = (Get-Date).ToUniversalTime().ToString('o') } | ConvertTo-Json -Compress
        [System.IO.File]::WriteAllText((Join-Path $content '.fox-runtime.json'), $markerJson, (New-Object System.Text.UTF8Encoding($false)))
        $step = 'install'
        if (Test-Path -LiteralPath $target) {
            $backup = Join-Path $runtimeParent ('.o' + [guid]::NewGuid().ToString('N').Substring(0, 8))
            Move-Item -LiteralPath $target -Destination $backup
            Write-FoxLog "moved previous runtime to $backup"
        }
        Move-Item -LiteralPath $content -Destination $target
        $installedNew = $true
        Write-FoxLog "installed runtime at $target"
    }
    # 3. Runtime ACL (only this version directory and its contents).
    $step = 'runtime-acl'
    Set-RuntimeAcl $target

    # 4. %ProgramData%\Fox and %ProgramData%\Fox\sandbox.
    $step = 'private-base'
    $foxData = Join-Path $ProgramDataDir 'Fox'
    $base = Join-Path $foxData 'sandbox'
    $result.privateBase = $base
    Initialize-FoxDirectory $foxData { param($p) Set-FoxDataAcl $p }
    Initialize-FoxDirectory $base { param($p) Set-SandboxBaseAcl $p }

    # 5. srt windows-install (idempotent in srt; never --force over someone else's config).
    $step = 'srt-windows-install'
    $before = Get-SrtRegistry
    $alreadyInstalled = [bool] $before
    $node = Join-Path $target 'node\node.exe'
    $cli = Join-Path $target $SrtCliRelative
    $srtPackage = Get-Content -LiteralPath (Join-Path $target $SrtPackageJsonRelative) -Raw | ConvertFrom-Json
    if ([string] $srtPackage.version -ne '0.0.81') { throw 'Fox requires sandbox-runtime 0.0.81 with its matching Windows helper' }
    $helper = Join-Path $target 'srt\node_modules\@anthropic-ai\sandbox-runtime\vendor\srt-win\x64\srt-win.exe'
    $probe = Invoke-Native $helper @('user', 'status')
    if ($probe.Code -ne 0) { throw 'Cannot inspect the Windows sandbox helper' }
    $helperStatus = $probe.Output | ConvertFrom-Json
    if ([int] $helperStatus.setup_version -ne 3) { throw 'Unexpected Windows sandbox setup version' }
    if ($helperStatus.profile_grant.state -ne 'none') { throw 'Remove the existing srt profile read grant before installing the Fox sandbox' }
    foreach ($capability in @('profile-deny-expansion', 'profile-session-grant-check', 'kept-grant-holders', 'read-deny-except', 'exec-holder')) {
        if (-not ($helperStatus.capabilities -contains $capability)) { throw "Windows sandbox helper is missing $capability" }
    }
    $r = Invoke-Native $node @($cli, 'windows-install', '--proxy-port-range', $ProxyPortRange, '--skip-profile-grant')
    if ($r.Code -eq 2) { throw 'srt windows-install was cancelled at its UAC prompt' }
    if ($r.Code -ne 0) { throw "srt windows-install failed (exit $($r.Code)): $($r.Output.Trim())" }
    $after = Get-SrtRegistry
    if (-not $after) { throw 'srt windows-install finished but HKLM\SOFTWARE\sandbox-runtime has no MarkerVersion / SandboxUserSid' }
    if ($after.markerVersion -ne $helperStatus.setup_version) { throw 'Windows sandbox installation did not reach the required setup version' }

    # 6. Result.
    $result.srt = @{
        alreadyInstalled = $alreadyInstalled
        packageVersion = [string] $srtPackage.version
        sandboxUserSid = $after.sandboxUserSid
        markerVersion = $after.markerVersion
        createdAt = $after.createdAt
    }
    $result.ok = $true
    if ($backup -and (Test-Path -LiteralPath $backup)) { Remove-FoxTree $backup 'previous runtime' }

    # 7. Optional elevated ownership repair (does not change ok: its own result).
    if ($RepairOwnershipList) {
        $step = 'repair-ownership'
        try {
            $result.ownershipRepair = & (Join-Path $PSScriptRoot 'repair-ownership.ps1') -ListPath $RepairOwnershipList -LogPath $LogPath
        } catch {
            $result.ownershipRepair = @{ ok = $false; error = $_.Exception.Message }
            Write-FoxLog "ownership repair failed: $($_.Exception.Message)"
        }
    }
    Write-FoxLog 'enable-sandbox done'
} catch {
    # The original error first, before any rollback or clean-up can fail and hide it.
    # Roll back only what this run copied; never touch anything else.
    Resolve-FoxFailure -Result $result -ErrorRecord $_ -Step $step -Cleanup ([ordered]@{
        'rollback of the new runtime' = {
            if ($installedNew -and $result.runtimeRoot -and (Test-Path -LiteralPath $result.runtimeRoot)) {
                Remove-FoxTree $result.runtimeRoot 'new runtime (rollback)'
            }
        }
        'restore of the previous runtime' = {
            if ($backup -and (Test-Path -LiteralPath $backup)) {
                Move-Item -LiteralPath $backup -Destination $result.runtimeRoot
                Write-FoxLog 'restored previous runtime'
            }
        }
    })
} finally {
    if ($staging) { Remove-FoxTree $staging 'staging' }
}
$result.cleanupErrors = $script:CleanupErrors.ToArray()
foreach ($e in $result.cleanupErrors) { Write-FoxLog "cleanup error: $e" }
Write-Result $result
if ($result.ok) { exit 0 } else { exit 1 }
