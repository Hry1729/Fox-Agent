#Requires -Version 5.1
<#
.SYNOPSIS
  测试（不随运行时包发布）：enable-sandbox.ps1 在“解压与清理同时失败”时保留原始错误；
  不再安装 SRT，只在旧版 Fox 安装回执证明时才清理遗留的 SRT（B2）。

.DESCRIPTION
  两种模式：

  默认（函数级，任何平台的 PowerShell 5.1 / 7 均可，Linux 上用 pwsh 7 也能跑）：
    从 enable-sandbox.ps1 的语法树取出全部顶层函数（不执行脚本主体，不需要管理员），
    设置 FOX_SCRIPT_TEST_FAULTS=extract,cleanup，用与主体相同的方式调用
    Expand-FoxPackage / Resolve-FoxFailure / Remove-FoxTree，检查：
      - primaryError.step = extract，message 为注入的解压错误，error 与之相同；
      - cleanupErrors 含注入的清理错误（暂存目录）与抛出异常的清理步骤，且未覆盖 primaryError；
      - 未设置变量时注入点为空操作：Invoke-FoxTestFault 不抛出，Remove-FoxTree 真正删除。

  -FullRun（Windows，需提权，工程师实测用）：
    以子进程 powershell.exe 运行真实 enable-sandbox.ps1（FOX_SCRIPT_TEST_FAULTS=extract,cleanup），
    读结果 JSON 检查同样的结论，并删除注入清理失败留下的暂存目录（结果 JSON 的 stagingDir）。
    -PackagePath / -ExpectedSha256 用任一真实运行时包；-Version 默认随机的 fault-test-xxxx，
    断言该版本目录在回滚后不存在。

  退出码：0 全部通过；1 有失败（每条检查一行 PASS/FAIL）。
#>
[CmdletBinding()]
param(
    [switch] $FullRun,
    [string] $PackagePath,
    [string] $ExpectedSha256,
    [string] $Version
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$script:Failures = 0
$script:Passes = 0

function Assert-Fox([bool] $Condition, [string] $Name) {
    if ($Condition) { $script:Passes++; Write-Output "PASS $Name" } else { $script:Failures++; Write-Output "FAIL $Name" }
}

$enable = Join-Path (Split-Path -Parent $PSScriptRoot) 'enable-sandbox.ps1'
$tmpRoot = [System.IO.Path]::GetTempPath()
$work = Join-Path $tmpRoot ('fox-enable-faults-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$savedFaults = $env:FOX_SCRIPT_TEST_FAULTS

try {
    if (-not $FullRun) {
        # Load the functions only (the script body needs elevation and Windows).
        $tokens = $null; $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($enable, [ref] $tokens, [ref] $errors)
        Assert-Fox ($errors.Count -eq 0) 'enable-sandbox.ps1 parses'
        $functions = $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)
        foreach ($fn in $functions) { . ([scriptblock]::Create($fn.Extent.Text)) }
        foreach ($name in @('Get-FoxTestFault', 'Test-FoxTestFault', 'Invoke-FoxTestFault', 'Resolve-FoxFailure', 'Remove-FoxTree', 'Expand-FoxPackage', 'Find-FoxSrtReceipt', 'Find-LegacySrtCli', 'Remove-LegacyFoxSrt')) {
            Assert-Fox ([bool] (Get-Command $name -CommandType Function -ErrorAction SilentlyContinue)) "function $name present"
        }
        # B2: Fox no longer installs SRT; the srt install and ownership-repair parameters are gone.
        $params = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
        foreach ($gone in @('ProxyPortRange', 'RepairOwnershipList')) {
            Assert-Fox (-not ($params -contains $gone)) "no -$gone parameter"
        }
        $scriptText = Get-Content -LiteralPath $enable -Raw
        Assert-Fox ($scriptText.IndexOf("'windows-install'") -lt 0) 'no srt windows-install call'
        Assert-Fox ($scriptText.IndexOf('repair-ownership.ps1') -lt 0) 'no repair-ownership.ps1 call'
        $InstallerReceiptName = '.fox-installer-result.json'
        $LegacySrtCliRelative = 'srt\node_modules\@anthropic-ai\sandbox-runtime\dist\cli.js'
        $LogPath = Join-Path $work 'enable.log'
        if ($env:OS -eq 'Windows_NT') {
            foreach ($exitCode in @(0, 7)) {
                $native = Invoke-Native $env:ComSpec @('/d', '/c', "echo native-warning 1>&2 & exit /b $exitCode")
                Assert-Fox ($native.Code -eq $exitCode) "native stderr: preserves exit $exitCode"
                Assert-Fox ($native.Output.Contains('native-warning')) "native stderr: preserves diagnostics for exit $exitCode"
            }
            Assert-Fox ($ErrorActionPreference -eq 'Stop') 'native stderr: restores error policy'
        }

        # 0. Legacy SRT ownership: only a Fox receipt matching the registry proves Fox made it.
        $runtimeParent = Join-Path $work 'sandbox-runtime'
        $registry = @{ markerVersion = 3; sandboxUserSid = 'S-1-5-21-1-2-3-1001'; sandboxUser = 'srt-sandbox'; createdAt = [int64] 1767225600 }
        $old = Join-Path $runtimeParent '0.1.6'
        New-Item -ItemType Directory -Path $old -Force | Out-Null
        Assert-Fox ($null -eq (Find-FoxSrtReceipt $runtimeParent $registry)) 'legacy: no receipt, no proof'
        $receipt = Join-Path $old '.fox-installer-result.json'
        [System.IO.File]::WriteAllText($receipt, '{"ok":true,"srt":{"alreadyInstalled":true,"sandboxUserSid":"S-1-5-21-1-2-3-1001","createdAt":1767225600}}')
        Assert-Fox ($null -eq (Find-FoxSrtReceipt $runtimeParent $registry)) 'legacy: SRT that was already there is not Fox''s'
        [System.IO.File]::WriteAllText($receipt, '{"ok":true,"srt":{"alreadyInstalled":false,"sandboxUserSid":"S-1-5-21-1-2-3-1001","createdAt":1767225599}}')
        Assert-Fox ($null -eq (Find-FoxSrtReceipt $runtimeParent $registry)) 'legacy: a reinstalled SRT (CreatedAt differs) is not Fox''s'
        [System.IO.File]::WriteAllText($receipt, '{"ok":true,"srt":{"alreadyInstalled":false,"sandboxUserSid":"S-1-5-21-1-2-3-1001","createdAt":1767225600}}')
        Assert-Fox ((Find-FoxSrtReceipt $runtimeParent $registry) -eq $receipt) 'legacy: matching pre-B2 receipt proves Fox made it'
        $newer = Join-Path $runtimeParent '0.1.7'
        New-Item -ItemType Directory -Path $newer -Force | Out-Null
        Remove-Item -LiteralPath $receipt -Force
        $carried = Join-Path $newer '.fox-installer-result.json'
        [System.IO.File]::WriteAllText($carried, '{"ok":true,"legacySrtRecord":{"alreadyInstalled":false,"sandboxUserSid":"S-1-5-21-1-2-3-1001","createdAt":1767225600}}')
        Assert-Fox ((Find-FoxSrtReceipt $runtimeParent $registry) -eq $carried) 'legacy: a carried legacySrtRecord still proves it'
        Assert-Fox ($null -eq (Find-LegacySrtCli $runtimeParent)) 'legacy: no srt CLI found when no runtime has one'
        $cliPath = Join-Path $old $LegacySrtCliRelative
        New-Item -ItemType Directory -Path (Split-Path -Parent $cliPath) -Force | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $old 'node') -Force | Out-Null
        [System.IO.File]::WriteAllText($cliPath, '// cli')
        [System.IO.File]::WriteAllText((Join-Path $old 'node\node.exe'), 'MZ')
        $found = Find-LegacySrtCli $runtimeParent
        Assert-Fox ($found -and $found.cli -eq $cliPath) 'legacy: srt CLI of an older runtime is found'

        # 1. Variable unset: injection points do nothing.
        Remove-Item Env:\FOX_SCRIPT_TEST_FAULTS -ErrorAction SilentlyContinue
        $script:FoxTestFaults = Get-FoxTestFault
        $script:CleanupErrors = New-Object System.Collections.Generic.List[string]
        Assert-Fox (@($script:FoxTestFaults).Count -eq 0) 'unset: no faults'
        $threw = $false
        try { Invoke-FoxTestFault 'extract' } catch { $threw = $true }
        Assert-Fox (-not $threw) 'unset: extract injection point is a no-op'
        $victim = Join-Path $work 'victim'
        New-Item -ItemType Directory -Path (Join-Path $victim 'a') -Force | Out-Null
        Remove-FoxTree $victim 'victim'
        Assert-Fox (-not (Test-Path -LiteralPath $victim)) 'unset: Remove-FoxTree really removes'
        Assert-Fox ($script:CleanupErrors.Count -eq 0) 'unset: no cleanup errors'

        # 2. extract + cleanup both fail.
        $env:FOX_SCRIPT_TEST_FAULTS = ' Extract , cleanup '
        $script:FoxTestFaults = Get-FoxTestFault
        $script:CleanupErrors = New-Object System.Collections.Generic.List[string]
        Assert-Fox ((@($script:FoxTestFaults) -join ',') -eq 'extract,cleanup') 'set: faults parsed (trimmed, lower case)'
        $result = @{ ok = $false; error = $null; primaryError = $null; cleanupErrors = @() }
        $staging = Join-Path $work '.s0000test'
        New-Item -ItemType Directory -Path $staging -Force | Out-Null
        $zip = Join-Path $staging 'p.zip'
        [System.IO.File]::WriteAllBytes($zip, [byte[]] @(80, 75, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
        $step = 'start'
        try {
            $step = 'extract'
            [void] (Expand-FoxPackage $zip (Join-Path $staging 'c'))
            $result.ok = $true
        } catch {
            Resolve-FoxFailure -Result $result -ErrorRecord $_ -Step $step -Cleanup ([ordered]@{
                'rollback of the new runtime' = { Remove-FoxTree $staging 'new runtime (rollback)' }
                'restore of the previous runtime' = { throw 'simulated restore failure' }
            })
        } finally {
            Remove-FoxTree $staging 'staging'
        }
        $result.cleanupErrors = $script:CleanupErrors.ToArray()
        $json = $result | ConvertTo-Json -Depth 6 -Compress
        $parsed = $json | ConvertFrom-Json
        Assert-Fox (-not $parsed.ok) 'faults: not ok'
        Assert-Fox ($parsed.primaryError.step -eq 'extract') 'faults: primaryError.step = extract'
        Assert-Fox ($parsed.primaryError.message -eq 'FOX_SCRIPT_TEST_FAULTS: injected extract failure') 'faults: primaryError is the extract error'
        Assert-Fox ($parsed.error -eq $parsed.primaryError.message) 'faults: error equals primaryError.message'
        $cleanup = @($parsed.cleanupErrors)
        Assert-Fox (@($cleanup | Where-Object { $_ -like 'staging *injected cleanup failure*' }).Count -eq 1) 'faults: cleanupErrors has the staging clean-up failure'
        Assert-Fox (@($cleanup | Where-Object { $_ -like 'new runtime (rollback) *injected cleanup failure*' }).Count -eq 1) 'faults: cleanupErrors has the rollback clean-up failure'
        Assert-Fox (@($cleanup | Where-Object { $_ -eq 'restore of the previous runtime: simulated restore failure' }).Count -eq 1) 'faults: a throwing clean-up step is recorded, not raised'
        Assert-Fox ($cleanup.Count -eq 3) 'faults: exactly three clean-up errors'
        $log = Get-Content -LiteralPath $LogPath -Raw
        Assert-Fox ($log.IndexOf('FAILED at step extract') -ge 0 -and $log.IndexOf('FAILED at step extract') -lt $log.IndexOf('clean-up step')) 'faults: primary error logged before clean-up'
    } else {
        if (-not ($PackagePath -and $ExpectedSha256)) { throw '-FullRun needs -PackagePath and -ExpectedSha256' }
        if (-not $Version) { $Version = 'fault-test-' + [guid]::NewGuid().ToString('N').Substring(0, 8) }
        $resultPath = Join-Path $work 'result.json'
        $logPath = Join-Path $work 'enable.log'
        $env:FOX_SCRIPT_TEST_FAULTS = 'extract,cleanup'
        $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        & $ps -NoProfile -ExecutionPolicy Bypass -File $enable -PackagePath $PackagePath -ExpectedSha256 $ExpectedSha256 -Version $Version -ResultPath $resultPath -LogPath $logPath | Out-Null
        $code = $LASTEXITCODE
        Assert-Fox ($code -eq 1) 'full: exit code 1'
        $parsed = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
        Assert-Fox ($parsed.primaryError.step -eq 'extract') 'full: primaryError.step = extract'
        Assert-Fox ($parsed.primaryError.message -eq 'FOX_SCRIPT_TEST_FAULTS: injected extract failure') 'full: primaryError is the extract error'
        Assert-Fox (@(@($parsed.cleanupErrors) | Where-Object { $_ -like 'staging *injected cleanup failure*' }).Count -eq 1) 'full: cleanupErrors has the staging clean-up failure'
        Assert-Fox ((Get-Content -LiteralPath $logPath -Raw).Contains('TEST FAULTS ACTIVE')) 'full: log names the active faults'
        Assert-Fox (-not (Test-Path -LiteralPath $parsed.runtimeRoot)) 'full: no runtime directory left for the test version'
        if ($parsed.stagingDir -and (Test-Path -LiteralPath $parsed.stagingDir)) {
            Remove-Item -LiteralPath $parsed.stagingDir -Recurse -Force
            Write-Output "removed staging left by the injected clean-up failure: $($parsed.stagingDir)"
        }
    }
} finally {
    if ($null -eq $savedFaults) { Remove-Item Env:\FOX_SCRIPT_TEST_FAULTS -ErrorAction SilentlyContinue } else { $env:FOX_SCRIPT_TEST_FAULTS = $savedFaults }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "passed $($script:Passes), failed $($script:Failures)"
if ($script:Failures -gt 0) { exit 1 } else { exit 0 }
