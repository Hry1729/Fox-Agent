#Requires -Version 5.1
<#
.SYNOPSIS
  Fox：以管理员身份修复沙箱创建的文件的所有者（docs/sandbox.md 7.8）。

.DESCRIPTION
  只处理 Fox 非提权扫描输出的清单（fox_agent_backend::winowner::ownership_repair_list 生成的 JSON，
  source = fox-nonelevated-ownership-scan）。对每一项：
    1. 路径必须是本机绝对路径（不接受 UNC / \\?\ / 设备路径）；
    2. 该路径本身及其每一级上级都不是重解析点（符号链接、junction 等）——不跟随重解析点；
    3. 读回当前所有者：必须等于该项记录的 expectedOwner，且属于清单的 srtSids；否则跳过；
    4. 启用 SeRestorePrivilege / SeTakeOwnershipPrivilege，把所有者改为该项的 newOwner（必须等于清单 userSid）；
    5. 读回确认所有者已改变。
  每项写日志，返回（并可写入 -ResultPath）结果对象；不修改清单之外的任何对象，不改 DACL。

  可单独运行，也被 enable-sandbox.ps1 / disable-sandbox.ps1 的 -RepairOwnershipList 调用。
  兼容 Windows PowerShell 5.1：UTF-8（带 BOM）、LF 换行。

.PARAMETER ListPath
  修复清单 JSON。
.PARAMETER ResultPath
  可选：结果 JSON 写入位置。
.PARAMETER LogPath
  日志文件；默认 %ProgramData%\Fox\logs\repair-ownership-<时间>.log。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $ListPath,
    [string] $ResultPath,
    [string] $LogPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$ExpectedSource = 'fox-nonelevated-ownership-scan'
$ProgramDataDir = $env:ProgramData
if (-not $ProgramDataDir) { $ProgramDataDir = 'C:\ProgramData' }
if (-not $LogPath) {
    $LogPath = Join-Path (Join-Path (Join-Path $ProgramDataDir 'Fox') 'logs') ('repair-ownership-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
}

function Write-FoxLog([string] $Message) {
    $line = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK') + ' repair-ownership: ' + $Message
    try {
        $dir = Split-Path -Parent $LogPath
        if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        Add-Content -LiteralPath $LogPath -Value $line -Encoding UTF8
    } catch {
        Write-Verbose ("log write failed: " + $_.Exception.Message)
    }
    Write-Verbose $line
}

function Test-Elevated {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-StringSid([string] $Text) {
    return [bool] ($Text -match '^S-1-\d+(-\d+)+$')
}

$nativeSource = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class FoxOwnerNative {
    [StructLayout(LayoutKind.Sequential)]
    struct LUID { public uint LowPart; public int HighPart; }
    [StructLayout(LayoutKind.Sequential)]
    struct TOKEN_PRIVILEGES { public uint PrivilegeCount; public LUID Luid; public uint Attributes; }

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool LookupPrivilegeValue(string system, string name, out LUID luid);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TOKEN_PRIVILEGES state, uint length, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool ConvertStringSidToSid(string sid, out IntPtr psid);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr mem);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    static extern uint SetNamedSecurityInfo(string name, int type, uint info, IntPtr owner, IntPtr group, IntPtr dacl, IntPtr sacl);

    const uint TOKEN_ADJUST_PRIVILEGES = 0x20, TOKEN_QUERY = 0x8, SE_PRIVILEGE_ENABLED = 0x2;
    const int SE_FILE_OBJECT = 1;
    const uint OWNER_SECURITY_INFORMATION = 0x1;

    // 0 = enabled; otherwise the Win32 error (1300 = not all assigned: the token lacks it).
    public static int EnablePrivilege(string name) {
        IntPtr token;
        if (!OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, out token)) return Marshal.GetLastWin32Error();
        try {
            TOKEN_PRIVILEGES tp = new TOKEN_PRIVILEGES();
            tp.PrivilegeCount = 1;
            tp.Attributes = SE_PRIVILEGE_ENABLED;
            if (!LookupPrivilegeValue(null, name, out tp.Luid)) return Marshal.GetLastWin32Error();
            if (!AdjustTokenPrivileges(token, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) return Marshal.GetLastWin32Error();
            return Marshal.GetLastWin32Error();
        } finally { CloseHandle(token); }
    }

    // Owner only (OWNER_SECURITY_INFORMATION): the DACL is not touched. 0 = success.
    public static int SetOwner(string path, string sid) {
        IntPtr psid;
        if (!ConvertStringSidToSid(sid, out psid)) return Marshal.GetLastWin32Error();
        try {
            return (int) SetNamedSecurityInfo(path, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, psid, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
        } finally { LocalFree(psid); }
    }
}
'@

function Get-OwnerSid([string] $Path) {
    # FileSecurity / DirectorySecurity with the Owner section only.
    $sections = [System.Security.AccessControl.AccessControlSections]::Owner
    if ([System.IO.Directory]::Exists($Path)) {
        $sd = New-Object System.Security.AccessControl.DirectorySecurity($Path, $sections)
    } else {
        $sd = New-Object System.Security.AccessControl.FileSecurity($Path, $sections)
    }
    return $sd.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
}

function Test-ReparseChain([string] $Path) {
    # $null when neither the path nor any ancestor is a reparse point; otherwise the first one found.
    $current = $Path
    while ($current) {
        $attributes = [System.IO.File]::GetAttributes($current)
        if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { return $current }
        $parent = [System.IO.Path]::GetDirectoryName($current)
        if (-not $parent -or $parent -eq $current) { break }
        $current = $parent
    }
    return $null
}

$result = @{ ok = $false; error = $null; repaired = 0; skipped = 0; failed = 0; items = @(); privileges = @{} }
$items = New-Object System.Collections.Generic.List[object]
try {
    Write-FoxLog "start: list=$ListPath user=$([System.Security.Principal.WindowsIdentity]::GetCurrent().Name)"
    if (-not (Test-Elevated)) { throw 'must run elevated (administrator)' }
    $list = Get-Content -LiteralPath $ListPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int] $list.version -ne 1) { throw "unsupported repair list version '$($list.version)'" }
    if ([string] $list.source -ne $ExpectedSource) { throw "repair list source must be '$ExpectedSource'" }
    $userSid = [string] $list.userSid
    if (-not (Test-StringSid $userSid)) { throw "userSid '$userSid' is not a SID" }
    $srtSids = @($list.srtSids | ForEach-Object { [string] $_ })
    if ($srtSids.Count -eq 0) { throw 'repair list has no srtSids' }
    foreach ($sid in $srtSids) { if (-not (Test-StringSid $sid)) { throw "srtSid '$sid' is not a SID" } }

    Add-Type -TypeDefinition $nativeSource -Language CSharp
    foreach ($privilege in @('SeRestorePrivilege', 'SeTakeOwnershipPrivilege')) {
        $code = [FoxOwnerNative]::EnablePrivilege($privilege)
        $result.privileges[$privilege] = $code
        Write-FoxLog "enable $privilege -> $code"
    }

    foreach ($entry in @($list.items)) {
        $path = [string] $entry.path
        $expected = [string] $entry.expectedOwner
        $newOwner = [string] $entry.newOwner
        $item = @{ path = $path; status = $null; previousOwner = $null; detail = $null }
        try {
            if ($path -notmatch '^[A-Za-z]:\\' -or $path.StartsWith('\\')) { throw 'skip:not a local absolute path' }
            if ($newOwner -ne $userSid) { throw 'skip:newOwner differs from the list userSid' }
            if (-not ($srtSids -contains $expected)) { throw 'skip:expectedOwner is not a recorded srt SID' }
            if (-not (Test-Path -LiteralPath $path)) { throw 'skip:path no longer exists' }
            $reparse = Test-ReparseChain $path
            if ($reparse) { throw "skip:reparse point at $reparse (not followed)" }
            $owner = Get-OwnerSid $path
            $item.previousOwner = $owner
            if ($owner -ne $expected -or -not ($srtSids -contains $owner)) { throw "skip:current owner $owner is not the recorded srt SID" }
            $code = [FoxOwnerNative]::SetOwner($path, $newOwner)
            if ($code -ne 0) { throw "fail:SetNamedSecurityInfo returned Win32 error $code" }
            $after = Get-OwnerSid $path
            if ($after -ne $newOwner) { throw "fail:owner read back as $after" }
            $item.status = 'repaired'
            $result.repaired++
        } catch {
            $text = $_.Exception.Message
            if ($text.StartsWith('skip:')) {
                $item.status = 'skipped'; $item.detail = $text.Substring(5); $result.skipped++
            } elseif ($text.StartsWith('fail:')) {
                $item.status = 'failed'; $item.detail = $text.Substring(5); $result.failed++
            } else {
                $item.status = 'failed'; $item.detail = $text; $result.failed++
            }
        }
        Write-FoxLog ("{0}: {1} (previous owner {2}) {3}" -f $item.status, $path, $item.previousOwner, $item.detail)
        $items.Add($item)
    }
    $result.ok = ($result.failed -eq 0)
} catch {
    $result.error = $_.Exception.Message
    Write-FoxLog ("FAILED: " + $result.error)
}
$result.items = $items.ToArray()
if ($ResultPath) {
    $json = $result | ConvertTo-Json -Depth 6 -Compress
    [System.IO.File]::WriteAllText($ResultPath, $json, (New-Object System.Text.UTF8Encoding($false)))
}
return $result
