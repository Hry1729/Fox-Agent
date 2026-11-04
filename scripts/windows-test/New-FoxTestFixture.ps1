#Requires -Version 5.1
<#
.SYNOPSIS
    Prepares (and undoes) the special set-ups some Fox Windows test items need
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    One -Action per call. Every action prints what it did and writes a JSON record to
    -OutPath when given. Close Fox first for Skills / Junction / DeleteSub / Hide /
    RestoreHidden (the script refuses while Fox is running, unless -Force).

      AclTest          (ADMIN shell) R2-1: create -Path (default <TestRoot>\acl-test), set its
                       owner to BUILTIN\Administrators and grant -User (default: the signed-in
                       user) Modify with inheritance. Reports the owner read back.
      AclTestOwnerFix  (ADMIN shell) R2-1 second half: set the owner of -Path back to -User.
      Skills           B5-4: under <DataDir>\skills create bad-yaml (broken YAML header),
                       bad-gbk (Chinese text saved as GBK) and num-ver (version: 1.0).
      SkillsRemove     B5-4: remove those three folders again.
      Junction         B5-6: in -ProjectRoot replace the folder "sub" with a junction to
                       -OutsideDir (default <TestRoot>\outside). The real "sub" is moved to
                       sub.fox-backup first.
      DeleteSub        B5-6: remove "sub" (junction or folder) from -ProjectRoot.
      SubRestore       B5-6: remove "sub" and move sub.fox-backup back.
      Fsmonitor        B5-8: make -ProjectRoot a git repo if needed and set
                       core.fsmonitor = cmd /c echo pwned > <TestRoot>\pwned.txt
                       (deletes an old pwned.txt first). Refuses when the git repo found
                       there is not -ProjectRoot itself (e.g. the Fox checkout above it).
      FsmonitorRestore B5-8: unset core.fsmonitor and report whether pwned.txt exists.
      Hide             B4-13 / B6-2 / B6-3: rename -Path to <Path>.fox-hidden and remember it
                       in <TestRoot>\hidden-files.json (Program Files paths need ADMIN).
      RestoreHidden    Rename everything in hidden-files.json back.
      Find             List candidate files for Hide: dsh / codex-acp / pi launchers under
                       %ProgramFiles%\Fox\sandbox-runtime, auth.json files in the private
                       HOMEs under %ProgramData%\Fox\sandbox (ADMIN to see them all), and
                       *.onnx model files under -DataDir.

.PARAMETER TestRoot
    Test root. Default <repo>\docs\windows-test\run-<Version>, derived from this script's
    folder (git-ignored; prepared by Initialize-FoxTestEnv.ps1).

.EXAMPLE
    # admin PowerShell
    .\New-FoxTestFixture.ps1 -Action AclTest -User $env:USERNAME
    .\New-FoxTestFixture.ps1 -Action Skills -DataDir ..\..\docs\windows-test\run-1339\data-1339-8
    .\New-FoxTestFixture.ps1 -Action Find -DataDir ..\..\docs\windows-test\run-1339\data-1339-9
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('AclTest', 'AclTestOwnerFix', 'Skills', 'SkillsRemove', 'Junction', 'DeleteSub', 'SubRestore',
        'Fsmonitor', 'FsmonitorRestore', 'Hide', 'RestoreHidden', 'Find')]
    [string] $Action,
    [string] $Path,
    [string] $User,
    [string] $DataDir,
    [string] $ProjectRoot,
    [string] $OutsideDir,
    [string] $TestRoot,
    [string] $Version = '1339',
    [string] $OutPath,
    [switch] $Force
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $TestRoot) { $TestRoot = [System.IO.Path]::GetFullPath((Join-Path $here "..\..\docs\windows-test\run-$Version")) }
if (-not $OutsideDir) { $OutsideDir = Join-Path $TestRoot 'outside' }
$utf8 = New-Object System.Text.UTF8Encoding($false)
$record = [ordered]@{ kind = 'fox-fixture'; action = $Action; takenAt = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss'); ok = $false; details = [ordered]@{} }

# Windows PowerShell 5.1 turns redirected native stderr into a terminating error under
# ErrorActionPreference=Stop; native tools run with Continue and are judged by their output.
function Invoke-FoxNative([string] $FilePath, [object[]] $Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $FilePath @Arguments 2>&1 | ForEach-Object { [string]$_ }
        return (($out | Out-String).Trim())
    } finally { $ErrorActionPreference = $saved }
}
function Test-FoxElevated {
    $p = New-Object System.Security.Principal.WindowsPrincipal([System.Security.Principal.WindowsIdentity]::GetCurrent())
    return $p.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Assert-FoxElevated { if (-not (Test-FoxElevated)) { throw "Action $Action needs an elevated (Run as administrator) PowerShell." } }
function Assert-FoxClosed {
    if ($Force) { return }
    $running = @(Get-Process -Name 'Fox', 'fox-desktop' -ErrorAction SilentlyContinue)
    if ($running.Count -gt 0) { throw "Close Fox first (running pid: $(@($running | ForEach-Object { $_.Id }) -join ',')), or pass -Force." }
}
function Get-FoxDataDir {
    if ($DataDir) { return $DataDir }
    if ($env:FOX_DATA_DIR) { return $env:FOX_DATA_DIR }
    return (Join-Path $env:APPDATA 'com.fox.agent')
}
# The test root sits inside the Fox checkout: never let git -C fall through to that repo.
function Assert-FoxOwnRepo([string] $Root) {
    # At a repo's top level "rev-parse --git-dir" prints ".git"; below another repo it prints that
    # repo's absolute path (ASCII-only check, so console code pages do not matter).
    $gd = Invoke-FoxNative 'git' @('-C', $Root, 'rev-parse', '--git-dir')
    if (-not $gd -or $gd.Trim() -ne '.git') { throw "git repo for $Root is not $Root itself (git-dir: '$gd'); refusing to touch it." }
}
function Get-FoxOwner([string] $Target) { return (Get-Acl -LiteralPath $Target).Owner }
function Write-FoxBytes([string] $File, [byte[]] $Bytes) {
    New-Item -ItemType Directory -Path (Split-Path -Parent $File) -Force | Out-Null
    [System.IO.File]::WriteAllBytes($File, $Bytes)
}
# Chinese text without putting non-ASCII characters in this file.
function U([int[]] $Codes) { return -join ($Codes | ForEach-Object { [char]$_ }) }
$zhSkill = U @(0x6D4B, 0x8BD5, 0x6280, 0x80FD)                 # test skill
$zhBody = U @(0x8FD9, 0x662F, 0x4E00, 0x6BB5, 0x4E2D, 0x6587, 0x8BF4, 0x660E, 0x3002)  # a line of Chinese text.
$hiddenList = [System.IO.Path]::Combine($TestRoot, 'hidden-files.json')

switch ($Action) {
    'AclTest' {
        Assert-FoxElevated
        if (-not $Path) { $Path = Join-Path $TestRoot 'acl-test' }
        if (-not $User) { $User = $env:USERNAME }
        $account = if ($User -match '\\') { $User } else { "$env:USERDOMAIN\$User" }
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
        $o1 = Invoke-FoxNative 'icacls' @($Path, '/setowner', '*S-1-5-32-544')
        $o2 = Invoke-FoxNative 'icacls' @($Path, '/grant', "${account}:(OI)(CI)M")
        $record.details.path = $Path; $record.details.user = $account
        $record.details.owner = Get-FoxOwner $Path
        $record.details.icacls = @($o1, $o2)
        $record.ok = ($record.details.owner -match 'Administrators|S-1-5-32-544')
    }
    'AclTestOwnerFix' {
        Assert-FoxElevated
        if (-not $Path) { $Path = Join-Path $TestRoot 'acl-test' }
        if (-not $User) { $User = $env:USERNAME }
        $account = if ($User -match '\\') { $User } else { "$env:USERDOMAIN\$User" }
        $o1 = Invoke-FoxNative 'icacls' @($Path, '/setowner', $account, '/T', '/C')
        $record.details.path = $Path; $record.details.owner = Get-FoxOwner $Path
        $record.details.icacls = $o1
        $record.ok = ($record.details.owner -ieq $account)
    }
    'Skills' {
        Assert-FoxClosed
        $root = Join-Path (Get-FoxDataDir) 'skills'
        $nl = "`r`n"
        $badYaml = "---${nl}name: bad-yaml${nl}description: Use when: X${nl}---${nl}${nl}# bad-yaml${nl}Broken YAML header (mapping value in description).${nl}"
        Write-FoxBytes (Join-Path $root 'bad-yaml\SKILL.md') $utf8.GetBytes($badYaml)
        $gbkText = "---${nl}name: bad-gbk${nl}description: $zhSkill GBK${nl}---${nl}${nl}# $zhSkill${nl}$zhBody${nl}"
        Write-FoxBytes (Join-Path $root 'bad-gbk\SKILL.md') ([System.Text.Encoding]::GetEncoding(936).GetBytes($gbkText))
        $numVer = "---${nl}name: num-ver${nl}description: Numeric version test skill.${nl}version: 1.0${nl}---${nl}${nl}# num-ver${nl}Valid skill whose version is a YAML number.${nl}"
        Write-FoxBytes (Join-Path $root 'num-ver\SKILL.md') $utf8.GetBytes($numVer)
        $record.details.skillsDir = $root
        $record.details.created = @('bad-yaml', 'bad-gbk', 'num-ver')
        $record.ok = $true
    }
    'SkillsRemove' {
        Assert-FoxClosed
        $root = Join-Path (Get-FoxDataDir) 'skills'
        foreach ($n in @('bad-yaml', 'bad-gbk', 'num-ver')) { Remove-Item -LiteralPath (Join-Path $root $n) -Recurse -Force -ErrorAction SilentlyContinue }
        $record.details.skillsDir = $root; $record.ok = $true
    }
    'Junction' {
        Assert-FoxClosed
        if (-not $ProjectRoot) { throw '-ProjectRoot is required' }
        $sub = Join-Path $ProjectRoot 'sub'; $backup = Join-Path $ProjectRoot 'sub.fox-backup'
        New-Item -ItemType Directory -Path $OutsideDir -Force | Out-Null
        if (Test-Path -LiteralPath $sub) {
            if (Test-Path -LiteralPath $backup) { throw "$backup already exists; run -Action SubRestore first" }
            Move-Item -LiteralPath $sub -Destination $backup
        }
        $o = Invoke-FoxNative 'cmd' @('/c', 'mklink', '/J', $sub, $OutsideDir)
        $item = Get-Item -LiteralPath $sub -Force
        $record.details.sub = $sub; $record.details.target = $OutsideDir; $record.details.mklink = $o
        $record.details.isReparsePoint = [bool]($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)
        $record.details.outsideHasAtxt = Test-Path -LiteralPath (Join-Path $OutsideDir 'a.txt')
        $record.ok = $record.details.isReparsePoint
    }
    'DeleteSub' {
        Assert-FoxClosed
        if (-not $ProjectRoot) { throw '-ProjectRoot is required' }
        $sub = Join-Path $ProjectRoot 'sub'
        if (Test-Path -LiteralPath $sub) {
            $item = Get-Item -LiteralPath $sub -Force
            if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { [void](Invoke-FoxNative 'cmd' @('/c', 'rmdir', $sub)) } else { Remove-Item -LiteralPath $sub -Recurse -Force }
        }
        $record.details.sub = $sub; $record.details.exists = Test-Path -LiteralPath $sub
        $record.details.outsideHasAtxt = Test-Path -LiteralPath (Join-Path $OutsideDir 'a.txt')
        $record.ok = -not $record.details.exists
    }
    'SubRestore' {
        Assert-FoxClosed
        if (-not $ProjectRoot) { throw '-ProjectRoot is required' }
        $sub = Join-Path $ProjectRoot 'sub'; $backup = Join-Path $ProjectRoot 'sub.fox-backup'
        if (Test-Path -LiteralPath $sub) {
            $item = Get-Item -LiteralPath $sub -Force
            if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { [void](Invoke-FoxNative 'cmd' @('/c', 'rmdir', $sub)) } else { Remove-Item -LiteralPath $sub -Recurse -Force }
        }
        if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $sub }
        $record.details.sub = $sub; $record.ok = $true
    }
    'Fsmonitor' {
        if (-not $ProjectRoot) { throw '-ProjectRoot is required' }
        $pwned = Join-Path $TestRoot 'pwned.txt'
        Remove-Item -LiteralPath $pwned -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath (Join-Path $ProjectRoot '.git'))) { [void](Invoke-FoxNative 'git' @('-C', $ProjectRoot, 'init', '-q')) }
        Assert-FoxOwnRepo $ProjectRoot
        [void](Invoke-FoxNative 'git' @('-C', $ProjectRoot, 'config', 'core.fsmonitor', "cmd /c echo pwned > $pwned"))
        $record.details.projectRoot = $ProjectRoot
        $record.details.fsmonitor = Invoke-FoxNative 'git' @('-C', $ProjectRoot, 'config', '--get', 'core.fsmonitor')
        $record.details.pwnedPath = $pwned
        $record.ok = $true
    }
    'FsmonitorRestore' {
        if (-not $ProjectRoot) { throw '-ProjectRoot is required' }
        $pwned = Join-Path $TestRoot 'pwned.txt'
        $record.details.pwnedExists = Test-Path -LiteralPath $pwned
        if (Test-Path -LiteralPath (Join-Path $ProjectRoot '.git')) {
            Assert-FoxOwnRepo $ProjectRoot
            [void](Invoke-FoxNative 'git' @('-C', $ProjectRoot, 'config', '--unset', 'core.fsmonitor'))
        } else { $record.details.note = 'no .git in ProjectRoot (project restored from backup); nothing to unset' }
        $record.details.projectRoot = $ProjectRoot
        $record.ok = $true
    }
    'Hide' {
        Assert-FoxClosed
        if (-not $Path) { throw '-Path is required' }
        $full = (Resolve-Path -LiteralPath $Path).ProviderPath
        $hidden = $full + '.fox-hidden'
        Rename-Item -LiteralPath $full -NewName ([System.IO.Path]::GetFileName($hidden))
        $list = @()
        if (Test-Path -LiteralPath $hiddenList) { $list = @(Get-Content -LiteralPath $hiddenList -Raw -Encoding UTF8 | ConvertFrom-Json) }
        $list += [pscustomobject]@{ original = $full; hidden = $hidden; at = $record.takenAt }
        [System.IO.File]::WriteAllText($hiddenList, (ConvertTo-Json -InputObject @($list) -Depth 3), $utf8)
        $record.details.original = $full; $record.details.hidden = $hidden; $record.ok = $true
    }
    'RestoreHidden' {
        Assert-FoxClosed
        $restored = @(); $left = @()
        if (Test-Path -LiteralPath $hiddenList) {
            foreach ($h in @(Get-Content -LiteralPath $hiddenList -Raw -Encoding UTF8 | ConvertFrom-Json)) {
                if (-not $h) { continue }
                try {
                    if (Test-Path -LiteralPath $h.hidden) { Rename-Item -LiteralPath $h.hidden -NewName ([System.IO.Path]::GetFileName($h.original)) }
                    $restored += $h.original
                } catch { $left += $h }
            }
            if ($left.Count -gt 0) { [System.IO.File]::WriteAllText($hiddenList, (ConvertTo-Json -InputObject @($left) -Depth 3), $utf8) }
            else { Remove-Item -LiteralPath $hiddenList -Force }
        }
        $record.details.restored = $restored; $record.details.failed = @($left | ForEach-Object { $_.original })
        $record.ok = ($left.Count -eq 0)
    }
    'Find' {
        $programFiles = $env:ProgramW6432; if (-not $programFiles) { $programFiles = $env:ProgramFiles }
        $rt = Join-Path $programFiles 'Fox\sandbox-runtime'
        $found = @()
        if (Test-Path -LiteralPath $rt) {
            $found += @(Get-ChildItem -LiteralPath $rt -Recurse -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match '^(dsh|codex-acp|pi-acp|pi)(\.cmd|\.exe|\.ps1)?$' } |
                ForEach-Object { $_.FullName })
        }
        $models = @()
        $dd = Get-FoxDataDir
        if (Test-Path -LiteralPath $dd) {
            $models = @(Get-ChildItem -LiteralPath $dd -Recurse -File -Include '*.onnx' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
        }
        $auth = @()
        $sandboxRoot = Join-Path $env:ProgramData 'Fox\sandbox'
        if (Test-Path -LiteralPath $sandboxRoot) {
            $auth = @(Get-ChildItem -LiteralPath $sandboxRoot -Recurse -File -Force -Filter 'auth.json' -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match '\\homes\\' } | ForEach-Object { $_.FullName })
        }
        $record.details.runtimeRoot = $rt; $record.details.launchers = $found
        $record.details.privateHomeAuth = $auth
        $record.details.dataDir = $dd; $record.details.onnxModels = $models
        $record.ok = $true
    }
}

$record.details.GetEnumerator() | ForEach-Object { Write-Host ("{0}: {1}" -f $_.Key, (@($_.Value) -join ' | ')) }
Write-Host ("ok={0}" -f $record.ok)
if ($OutPath) { [System.IO.File]::WriteAllText($OutPath, ($record | ConvertTo-Json -Depth 5), $utf8) }
