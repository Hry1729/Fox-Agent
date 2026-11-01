#Requires -Version 5.1
<#
.SYNOPSIS
    Process tree snapshot for Fox cancel / clean-up tests (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    One row per process: pid, ppid, creation time (UTC), executable, account (token user),
    restricted token, session id, whether the process is in any job, and whether it is in a
    job object held by a Fox process ("Fox Job").

    "In Fox Job" is decided exactly, not by guessing from the parent: the script finds every
    job handle the Fox processes hold (NtQuerySystemInformation, extended handle table),
    duplicates each one into this process and reads the job's process id list
    (JobObjectBasicProcessIdList; nested child jobs are included). Run it as the same user
    as Fox (or elevated): duplicating a handle needs PROCESS_DUP_HANDLE on the Fox process.

    It also flags processes that look like they left the Fox Job:
      - foxDescendant: the parent chain (pid + creation time checked, so a reused pid does
        not count) leads to a Fox process, but the process is not in a Fox Job;
      - viaSeclogon: the parent is the Secondary Logon service (seclogon), which is how a
        process started with CreateProcessWithLogonW (e.g. a Codex sandbox user) appears;
      - accountMatch: the token user matches -SandboxAccountPattern.
    A suspect's own descendants are suspects too.

    Account names of processes owned by another user usually need an elevated shell; without
    it the account is reported as null with accessDenied = true. Nothing is changed or killed.

.PARAMETER OutPath
    JSON file to write. Without it the result object is returned.

.PARAMETER FoxProcessName
    Executable names (without .exe) of the Fox Host. Default: Fox, fox-desktop.

.PARAMETER SandboxAccountPattern
    Regex matched against the account name. Default matches Codex sandbox accounts.

.PARAMETER Baseline
    An earlier snapshot JSON from this script. When given, the result lists processes that are
    new since the baseline (same pid and creation time = same process) and still alive.

.EXAMPLE
    .\Get-FoxProcessTree.ps1 -OutPath .\proc-before.json
    # ... start the Codex turn, press Stop, wait 3 s ...
    .\Get-FoxProcessTree.ps1 -OutPath .\proc-after.json -Baseline .\proc-before.json
#>
[CmdletBinding()]
param(
    [string] $OutPath,
    [string[]] $FoxProcessName = @('Fox', 'fox-desktop'),
    [string] $SandboxAccountPattern = '(?i)codex|sandbox',
    [string] $Baseline
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if (-not ('FoxForensics.ProcTree' -as [type])) {
    # C# 5 only: Windows PowerShell 5.1 compiles this with the .NET Framework compiler.
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace FoxForensics {
    public class ProcRow {
        public int Pid; public int Ppid; public string Exe; public long CreatedFileTime;
        public string Sid; public bool AccessDenied; public bool? InAnyJob; public bool? Restricted;
        public int SessionId = -1;
    }
    public class JobInfo {
        public int OwnerPid; public long Handle; public int AssignedCount; public int[] Pids; public string Error;
    }
    public static class ProcTree {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        struct PROCESSENTRY32W {
            public uint dwSize; public uint cntUsage; public uint th32ProcessID; public IntPtr th32DefaultHeapID;
            public uint th32ModuleID; public uint cntThreads; public uint th32ParentProcessID; public int pcPriClassBase;
            public uint dwFlags; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExeFile;
        }
        [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool Process32FirstW(IntPtr snap, ref PROCESSENTRY32W e);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool Process32NextW(IntPtr snap, ref PROCESSENTRY32W e);
        [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr h);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetProcessTimes(IntPtr h, out long create, out long exit, out long kernel, out long user);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool ProcessIdToSessionId(uint pid, out uint session);
        [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool DuplicateHandle(IntPtr srcProc, IntPtr src, IntPtr dstProc, out IntPtr dst, uint access, bool inherit, uint options);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern IntPtr CreateJobObjectW(IntPtr attrs, string name);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool QueryInformationJobObject(IntPtr job, int infoClass, IntPtr info, int length, out int returned);
        [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
        [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int infoClass, IntPtr info, int length, out int returned);
        [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool ConvertSidToStringSidW(IntPtr sid, out IntPtr text);
        [DllImport("advapi32.dll", SetLastError = true)] static extern bool IsTokenRestricted(IntPtr token);
        [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
        [DllImport("ntdll.dll")] static extern int NtQuerySystemInformation(int infoClass, IntPtr info, int length, out int returned);

        const uint TH32CS_SNAPPROCESS = 0x2;
        const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
        const uint PROCESS_DUP_HANDLE = 0x40;
        const uint TOKEN_QUERY = 0x8;
        const uint DUPLICATE_SAME_ACCESS = 0x2;
        static readonly IntPtr INVALID = new IntPtr(-1);

        public static List<ProcRow> Snapshot() {
            List<ProcRow> rows = new List<ProcRow>();
            IntPtr snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if (snap == INVALID) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            try {
                PROCESSENTRY32W e = new PROCESSENTRY32W();
                e.dwSize = (uint)Marshal.SizeOf(typeof(PROCESSENTRY32W));
                if (!Process32FirstW(snap, ref e)) return rows;
                do {
                    ProcRow r = new ProcRow();
                    r.Pid = (int)e.th32ProcessID; r.Ppid = (int)e.th32ParentProcessID; r.Exe = e.szExeFile;
                    Fill(r);
                    rows.Add(r);
                } while (Process32NextW(snap, ref e));
            } finally { CloseHandle(snap); }
            return rows;
        }

        static void Fill(ProcRow r) {
            uint session;
            if (ProcessIdToSessionId((uint)r.Pid, out session)) r.SessionId = (int)session;
            if (r.Pid == 0) return;
            IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, (uint)r.Pid);
            if (h == IntPtr.Zero) { r.AccessDenied = true; return; }
            try {
                long c, x, k, u;
                if (GetProcessTimes(h, out c, out x, out k, out u)) r.CreatedFileTime = c;
                bool inJob;
                if (IsProcessInJob(h, IntPtr.Zero, out inJob)) r.InAnyJob = inJob;
                IntPtr token;
                if (OpenProcessToken(h, TOKEN_QUERY, out token)) {
                    try {
                        r.Restricted = IsTokenRestricted(token);
                        int len;
                        GetTokenInformation(token, 1, IntPtr.Zero, 0, out len);
                        if (len > 0) {
                            IntPtr buf = Marshal.AllocHGlobal(len);
                            try {
                                if (GetTokenInformation(token, 1, buf, len, out len)) {
                                    IntPtr sid = Marshal.ReadIntPtr(buf);
                                    IntPtr text;
                                    if (ConvertSidToStringSidW(sid, out text)) { r.Sid = Marshal.PtrToStringUni(text); LocalFree(text); }
                                }
                            } finally { Marshal.FreeHGlobal(buf); }
                        }
                    } finally { CloseHandle(token); }
                } else { r.AccessDenied = true; }
            } finally { CloseHandle(h); }
        }

        // Object type index of job objects on this system: create one and find our own handle.
        static int JobTypeIndex() {
            IntPtr job = CreateJobObjectW(IntPtr.Zero, null);
            if (job == IntPtr.Zero) return -1;
            try {
                long self = System.Diagnostics.Process.GetCurrentProcess().Id;
                foreach (long[] h in Table()) if (h[0] == self && h[1] == job.ToInt64()) return (int)h[2];
                return -1;
            } finally { CloseHandle(job); }
        }

        // Rows of {ownerPid, handleValue, objectTypeIndex} from the extended handle table (x64 layout).
        static List<long[]> Table() {
            int size = 1 << 20;
            for (int attempt = 0; attempt < 12; attempt++) {
                IntPtr buf = Marshal.AllocHGlobal(size);
                try {
                    int returned;
                    int status = NtQuerySystemInformation(64, buf, size, out returned);
                    if (status == unchecked((int)0xC0000004)) { size = Math.Max(size * 2, returned + (1 << 16)); continue; }
                    if (status != 0) throw new InvalidOperationException("NtQuerySystemInformation(64) status 0x" + status.ToString("X8"));
                    long count = Marshal.ReadIntPtr(buf).ToInt64();
                    int entry = IntPtr.Size == 8 ? 40 : 28;
                    int first = IntPtr.Size * 2;
                    List<long[]> rows = new List<long[]>((int)Math.Min(count, 4000000));
                    for (long i = 0; i < count; i++) {
                        IntPtr p = new IntPtr(buf.ToInt64() + first + i * entry);
                        long pid = Marshal.ReadIntPtr(p, IntPtr.Size).ToInt64();
                        long handle = Marshal.ReadIntPtr(p, IntPtr.Size * 2).ToInt64();
                        int typeIndex = (ushort)Marshal.ReadInt16(p, IntPtr.Size * 3 + 6);
                        rows.Add(new long[] { pid, handle, typeIndex });
                    }
                    return rows;
                } finally { Marshal.FreeHGlobal(buf); }
            }
            throw new InvalidOperationException("handle table kept growing");
        }

        // Every job held by one of ownerPids, with its member process ids.
        public static List<JobInfo> FoxJobs(int[] ownerPids) {
            List<JobInfo> jobs = new List<JobInfo>();
            if (ownerPids.Length == 0) return jobs;
            List<long[]> table = Table();
            int jobType = JobTypeIndex();
            if (jobType < 0) throw new InvalidOperationException("cannot determine the job object type index");
            HashSet<long> owners = new HashSet<long>();
            foreach (int o in ownerPids) owners.Add(o);
            Dictionary<long, IntPtr> procs = new Dictionary<long, IntPtr>();
            try {
                foreach (long[] h in table) {
                    if (h[2] != jobType || !owners.Contains(h[0])) continue;
                    JobInfo info = new JobInfo(); info.OwnerPid = (int)h[0]; info.Handle = h[1];
                    IntPtr proc;
                    if (!procs.TryGetValue(h[0], out proc)) {
                        proc = OpenProcess(PROCESS_DUP_HANDLE, false, (uint)h[0]);
                        procs[h[0]] = proc;
                    }
                    if (proc == IntPtr.Zero) { info.Error = "OpenProcess(PROCESS_DUP_HANDLE) failed: " + Marshal.GetLastWin32Error(); jobs.Add(info); continue; }
                    IntPtr dup;
                    if (!DuplicateHandle(proc, new IntPtr(h[1]), GetCurrentProcess(), out dup, 0, false, DUPLICATE_SAME_ACCESS)) {
                        info.Error = "DuplicateHandle failed: " + Marshal.GetLastWin32Error(); jobs.Add(info); continue;
                    }
                    try {
                        int capacity = 16384;
                        int length = 8 + IntPtr.Size * capacity;
                        IntPtr buf = Marshal.AllocHGlobal(length);
                        try {
                            int returned;
                            if (!QueryInformationJobObject(dup, 3, buf, length, out returned)) {
                                info.Error = "QueryInformationJobObject failed: " + Marshal.GetLastWin32Error();
                            } else {
                                info.AssignedCount = Marshal.ReadInt32(buf);
                                int listed = Marshal.ReadInt32(buf, 4);
                                info.Pids = new int[listed];
                                for (int i = 0; i < listed; i++) info.Pids[i] = (int)Marshal.ReadIntPtr(buf, 8 + i * IntPtr.Size).ToInt64();
                            }
                        } finally { Marshal.FreeHGlobal(buf); }
                    } finally { CloseHandle(dup); }
                    jobs.Add(info);
                }
            } finally {
                foreach (IntPtr p in procs.Values) if (p != IntPtr.Zero) CloseHandle(p);
            }
            return jobs;
        }
    }
}
'@
}

function ConvertTo-FoxIsoTime([long] $FileTime) {
    if ($FileTime -le 0) { return $null }
    return [DateTime]::FromFileTimeUtc($FileTime).ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
}

function Resolve-FoxAccount([string] $Sid, [hashtable] $Cache) {
    if (-not $Sid) { return $null }
    if ($Cache.ContainsKey($Sid)) { return $Cache[$Sid] }
    $name = $Sid
    try { $name = (New-Object System.Security.Principal.SecurityIdentifier($Sid)).Translate([System.Security.Principal.NTAccount]).Value } catch { }
    $Cache[$Sid] = $name
    return $name
}

function Get-FoxProcessTreeSnapshot {
    param([string[]] $FoxNames, [string] $AccountPattern)
    $taken = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
    $raw = [FoxForensics.ProcTree]::Snapshot()
    $byPid = @{}
    foreach ($r in $raw) { $byPid[[int]$r.Pid] = $r }

    $foxNamesLower = @($FoxNames | ForEach-Object { ($_ -replace '\.exe$', '').ToLowerInvariant() })
    $foxRoots = @($raw | Where-Object { $foxNamesLower -contains ([string]$_.Exe -replace '\.exe$', '').ToLowerInvariant() })
    $foxRootPids = @($foxRoots | ForEach-Object { [int]$_.Pid })

    $jobs = @()
    $jobError = $null
    try { $jobs = @([FoxForensics.ProcTree]::FoxJobs([int[]]$foxRootPids)) } catch { $jobError = $_.Exception.Message }
    $inFoxJob = @{}
    foreach ($j in $jobs) { if ($j.Pids) { foreach ($p in $j.Pids) { $inFoxJob[[int]$p] = ('{0}:0x{1:X}' -f $j.OwnerPid, $j.Handle) } } }

    $seclogonPid = 0
    try {
        $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='seclogon'" -ErrorAction Stop
        if ($svc) { $seclogonPid = [int]$svc.ProcessId }
    } catch { }

    $commandLines = @{}
    try {
        foreach ($p in @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)) { $commandLines[[int]$p.ProcessId] = $p.CommandLine }
    } catch { }

    $accounts = @{}
    $foxRootSet = @{}
    foreach ($p in $foxRootPids) { $foxRootSet[$p] = $true }

    # Parent chain with creation-time check: a parent created after its child is a reused pid.
    function Get-FoxChain([int] $ProcessId) {
        $chain = New-Object System.Collections.Generic.List[int]
        $seen = @{}
        $current = $byPid[$ProcessId]
        while ($current -and -not $seen.ContainsKey([int]$current.Pid)) {
            $seen[[int]$current.Pid] = $true
            $parent = $byPid[[int]$current.Ppid]
            if (-not $parent -or [int]$current.Ppid -eq [int]$current.Pid) { break }
            if ($parent.CreatedFileTime -gt 0 -and $current.CreatedFileTime -gt 0 -and $parent.CreatedFileTime -gt $current.CreatedFileTime) { break }
            $chain.Add([int]$parent.Pid)
            $current = $parent
        }
        return ,$chain
    }

    $rows = New-Object System.Collections.Generic.List[object]
    $suspectPids = @{}
    foreach ($r in $raw) {
        $chain = Get-FoxChain ([int]$r.Pid)
        $foxDescendant = $false
        foreach ($a in $chain) { if ($foxRootSet.ContainsKey($a)) { $foxDescendant = $true; break } }
        $account = Resolve-FoxAccount $r.Sid $accounts
        $viaSeclogon = ($seclogonPid -gt 0 -and [int]$r.Ppid -eq $seclogonPid)
        $accountMatch = ($account -and $AccountPattern -and ($account -match $AccountPattern))
        $isFoxRoot = $foxRootSet.ContainsKey([int]$r.Pid)
        $job = $null
        if ($inFoxJob.ContainsKey([int]$r.Pid)) { $job = $inFoxJob[[int]$r.Pid] }
        $reasons = @()
        if (-not $isFoxRoot -and -not $job) {
            if ($foxDescendant) { $reasons += 'foxDescendant' }
            if ($viaSeclogon) { $reasons += 'viaSeclogon' }
            if ($accountMatch) { $reasons += 'accountMatch' }
        }
        if ($reasons.Count -gt 0) { $suspectPids[[int]$r.Pid] = $true }
        $parentRow = $byPid[[int]$r.Ppid]
        $parentName = $null
        if ($parentRow) { $parentName = $parentRow.Exe }
        $cmd = $null
        if ($commandLines.ContainsKey([int]$r.Pid)) { $cmd = $commandLines[[int]$r.Pid] }
        $rows.Add([ordered]@{
            pid = [int]$r.Pid
            ppid = [int]$r.Ppid
            parentName = $parentName
            name = $r.Exe
            created = ConvertTo-FoxIsoTime $r.CreatedFileTime
            createdFileTime = [long]$r.CreatedFileTime
            account = $account
            sid = $r.Sid
            accessDenied = [bool]$r.AccessDenied
            restrictedToken = $r.Restricted
            sessionId = [int]$r.SessionId
            inAnyJob = $r.InAnyJob
            inFoxJob = [bool]$job
            foxJob = $job
            isFoxHost = $isFoxRoot
            foxDescendant = $foxDescendant
            ancestors = @($chain)
            commandLine = $cmd
            suspectReasons = $reasons
        })
    }
    # A suspect's descendants are suspects too.
    foreach ($row in $rows) {
        if ($suspectPids.ContainsKey($row.pid)) { continue }
        foreach ($a in $row.ancestors) {
            if ($suspectPids.ContainsKey([int]$a) -and -not $row.inFoxJob) { $row.suspectReasons = @($row.suspectReasons) + ('descendantOf:' + $a); break }
        }
    }

    $jobRows = @($jobs | ForEach-Object {
        [ordered]@{ ownerPid = $_.OwnerPid; handle = ('0x{0:X}' -f $_.Handle); assignedProcesses = $_.AssignedCount; pids = $(if ($_.Pids) { @($_.Pids) } else { @() }); error = $_.Error }
    })
    $suspects = @($rows | Where-Object { @($_.suspectReasons).Count -gt 0 })
    return [ordered]@{
        kind = 'fox-process-tree'
        version = 1
        takenAt = $taken
        host = $env:COMPUTERNAME
        user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
        elevated = ([System.Security.Principal.WindowsPrincipal][System.Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
        psVersion = $PSVersionTable.PSVersion.ToString()
        foxHostPids = $foxRootPids
        seclogonPid = $seclogonPid
        foxJobs = $jobRows
        foxJobError = $jobError
        foxJobMemberCount = $inFoxJob.Count
        suspectCount = $suspects.Count
        suspects = $suspects
        processes = $rows.ToArray()
    }
}

function Compare-FoxProcessTree($Before, $After) {
    $old = @{}
    foreach ($p in @($Before.processes)) { $old[('{0}@{1}' -f $p.pid, $p.createdFileTime)] = $true }
    $new = @($After.processes | Where-Object { -not $old.ContainsKey(('{0}@{1}' -f $_.pid, $_.createdFileTime)) -and $_.createdFileTime -gt 0 })
    $foxRelated = @($new | Where-Object { $_.foxDescendant -or $_.inFoxJob -or @($_.suspectReasons).Count -gt 0 })
    return [ordered]@{
        baselineTakenAt = $Before.takenAt
        newSinceBaseline = $new.Count
        newFoxRelated = @($foxRelated | ForEach-Object { [ordered]@{ pid = $_.pid; ppid = $_.ppid; name = $_.name; created = $_.created; account = $_.account; inFoxJob = $_.inFoxJob; suspectReasons = $_.suspectReasons; commandLine = $_.commandLine } })
    }
}

$result = Get-FoxProcessTreeSnapshot -FoxNames $FoxProcessName -AccountPattern $SandboxAccountPattern
if ($Baseline) {
    $before = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
    # Round-trip the fresh snapshot through JSON so both sides have the same shape.
    $afterObject = ($result | ConvertTo-Json -Depth 8) | ConvertFrom-Json
    $result['comparedToBaseline'] = Compare-FoxProcessTree $before $afterObject
}

if ($OutPath) {
    $dir = Split-Path -Parent ([System.IO.Path]::GetFullPath($OutPath))
    if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllText([System.IO.Path]::GetFullPath($OutPath), ($result | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding($false)))
    Write-Host ("process snapshot: {0} processes, Fox hosts [{1}], Fox Job members {2}, suspects {3} -> {4}" -f @($result.processes).Count, ($result.foxHostPids -join ','), $result.foxJobMemberCount, $result.suspectCount, $OutPath)
    if ($result.foxJobError) { Write-Warning ("Fox Job lookup failed: " + $result.foxJobError) }
} else {
    $result
}
