#Requires -Version 5.1
<#
.SYNOPSIS
    Integrity level and elevation of the running Fox processes and of this shell
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Fox must be tested as a normal user at Medium integrity. Started from an elevated shell
    it runs at High integrity, and many results (ACL, owner, Job clean-up) are no longer
    meaningful. This script reads the token of every Fox process (Fox.exe / fox-desktop.exe)
    and reports:
        pid, name, integrity (Untrusted / Low / Medium / MediumPlus / High / System),
        integrityRid, elevated, account
    plus the same for the current shell. "ok" is true when every Fox process is Medium and
    not elevated. Nothing is changed.

.PARAMETER OutPath
    JSON file to write. Without it the result object is returned.

.EXAMPLE
    .\Get-FoxIntegrity.ps1
#>
[CmdletBinding()]
param(
    [string] $OutPath,
    [string[]] $FoxProcessName = @('Fox', 'fox-desktop')
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if (-not ('FoxForensics.Integrity' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Security.Principal;
namespace FoxForensics {
public static class Integrity {
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr h);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetTokenInformation(IntPtr token, int cls, IntPtr info, int len, out int retLen);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern IntPtr GetSidSubAuthority(IntPtr sid, uint index);

    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    const uint TOKEN_QUERY = 0x0008;
    const int TokenUser = 1;
    const int TokenElevation = 20;
    const int TokenIntegrityLevel = 25;

    static IntPtr Query(IntPtr token, int cls) {
        int len;
        GetTokenInformation(token, cls, IntPtr.Zero, 0, out len);
        if (len <= 0) return IntPtr.Zero;
        IntPtr buf = Marshal.AllocHGlobal(len);
        if (!GetTokenInformation(token, cls, buf, len, out len)) { Marshal.FreeHGlobal(buf); return IntPtr.Zero; }
        return buf;
    }

    // Returns "rid|elevated|account" or "error:<win32>".
    public static string Read(int pid) {
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (process == IntPtr.Zero) return "error:" + Marshal.GetLastWin32Error();
        try {
            IntPtr token;
            if (!OpenProcessToken(process, TOKEN_QUERY, out token)) return "error:" + Marshal.GetLastWin32Error();
            try {
                long rid = -1; int elevated = -1; string account = "";
                IntPtr il = Query(token, TokenIntegrityLevel);
                if (il != IntPtr.Zero) {
                    IntPtr sid = Marshal.ReadIntPtr(il);
                    int count = Marshal.ReadByte(GetSidSubAuthorityCount(sid));
                    rid = (uint)Marshal.ReadInt32(GetSidSubAuthority(sid, (uint)(count - 1)));
                    Marshal.FreeHGlobal(il);
                }
                IntPtr el = Query(token, TokenElevation);
                if (el != IntPtr.Zero) { elevated = Marshal.ReadInt32(el); Marshal.FreeHGlobal(el); }
                IntPtr user = Query(token, TokenUser);
                if (user != IntPtr.Zero) {
                    try { account = new SecurityIdentifier(Marshal.ReadIntPtr(user)).Translate(typeof(NTAccount)).Value; }
                    catch (Exception) { account = new SecurityIdentifier(Marshal.ReadIntPtr(user)).Value; }
                    Marshal.FreeHGlobal(user);
                }
                return rid + "|" + elevated + "|" + account;
            } finally { CloseHandle(token); }
        } finally { CloseHandle(process); }
    }
}
}
'@
}

function ConvertTo-FoxIntegrityName([long] $Rid) {
    if ($Rid -lt 0) { return 'unknown' }
    if ($Rid -lt 0x1000) { return 'Untrusted' }
    if ($Rid -lt 0x2000) { return 'Low' }
    if ($Rid -lt 0x2100) { return 'Medium' }
    if ($Rid -lt 0x3000) { return 'MediumPlus' }
    if ($Rid -lt 0x4000) { return 'High' }
    return 'System'
}

function Read-FoxToken([int] $ProcessId, [string] $Name) {
    try { $raw = [FoxForensics.Integrity]::Read($ProcessId) } catch { $raw = 'error:' + $_.Exception.Message }
    if ($raw -like 'error:*') {
        return [ordered]@{ pid = $ProcessId; name = $Name; integrity = 'unknown'; integrityRid = $null; elevated = $null; account = $null; error = $raw.Substring(6) }
    }
    $parts = $raw.Split('|')
    $rid = [long]$parts[0]
    return [ordered]@{
        pid = $ProcessId; name = $Name
        integrity = (ConvertTo-FoxIntegrityName $rid)
        integrityRid = ('0x{0:X4}' -f $rid)
        elevated = $(if ($parts[1] -eq '-1') { $null } else { $parts[1] -ne '0' })
        account = $parts[2]
        error = $null
    }
}

$fox = @()
foreach ($p in @(Get-Process -Name $FoxProcessName -ErrorAction SilentlyContinue)) {
    $fox += (Read-FoxToken $p.Id $p.ProcessName)
}
$shell = Read-FoxToken $PID 'this-shell'
$ok = ($fox.Count -gt 0)
foreach ($f in $fox) { if ($f.integrity -ne 'Medium' -or $f.elevated -ne $false) { $ok = $false } }

$result = [ordered]@{
    kind = 'fox-integrity'
    takenAt = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    fox = $fox
    shell = $shell
    ok = $ok
    note = $(if ($fox.Count -eq 0) { 'no Fox process is running' } elseif ($ok) { 'Fox runs at Medium integrity, not elevated' } else { 'Fox is NOT at Medium / non-elevated: restart it from a normal (non-admin) shell' })
}

if ($OutPath) {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($OutPath, ($result | ConvertTo-Json -Depth 5), $utf8)
}
foreach ($f in $fox) { Write-Host ("Fox pid={0} {1} integrity={2} ({3}) elevated={4} account={5} {6}" -f $f.pid, $f.name, $f.integrity, $f.integrityRid, $f.elevated, $f.account, $f.error) }
Write-Host ("shell pid={0} integrity={1} elevated={2}" -f $shell.pid, $shell.integrity, $shell.elevated)
Write-Host ("ok={0}  {1}" -f $result.ok, $result.note)
if (-not $OutPath) { return [pscustomobject]$result }
