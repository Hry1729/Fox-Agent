#Requires -Version 5.1
<#
.SYNOPSIS
  Fox：诊断 srt（Windows）下沙箱进程的 stdin / stdout 是否可用，以及经 ACP 桥接的往返（docs/sandbox.md 9）。

.DESCRIPTION
  以普通用户运行（不要提权）。用 .NET Process（与 Fox 相同：清空环境后只给 Host 白名单变量，
  RedirectStandardInput/Output/Error）依次运行以下变体，结果写成 JSON（-ResultPath）：

    direct               同一个 node，不经 srt（对照组）
    srt                  node <srt cli.js> --settings <临时设置> -c "<set ... && "node.exe" -e "<回显脚本>">"
                         与 Fox 在 Windows 上构造的方式一致：-c + cmd.exe 行、set 传会话变量、
                         NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main
    srt-no-node-options  同上但不设 NODE_OPTIONS（区分原因）
    srt-bridge           经 Fox 的 ACP 桥接（<RuntimeRoot>\acp-bridge\bridge.mjs）：本脚本建一次性命名管道
                         （DACL：当前用户 + srt 沙箱 SID），srt 的 stdin 不用；桥接连回管道、握手（token）后
                         启动回显后端，ECHO 经管道往返

  stdio 变体：回显脚本先打印 READY；看到 READY 后写一行 hello，保持 stdin 打开 5 秒，再关闭 stdin。
  记录：是否收到 ECHO 及时间、是否收到 END 及时间（END 早于关闭 stdin = stdin 提前 EOF）、
  退出码、stdout/stderr 全文（脱敏）及每行时间戳（判断是否实时转发）。
  srt 0.0.80 已知结果（工程师 2026-10-20）：srt 变体约在 READY 后 1 ms 就收到 END（stdin 立即 EOF），
  stdout/stderr 实时；direct 正常。本脚本保留作回归项：srt 修复后 srt 变体应与 direct 一致。

  授权目录（srt 设置里唯一的 allowRead/allowWrite）默认建在
  %ProgramData%\Fox\sandbox\<你的 SID>\tmp\fox-diag-<随机> 下（Fox 已为该路径加过祖先 ACE；
  若 Fox 从未启动过沙箱会话，可用 -WorkDir 指定其他自有目录）。脚本结束时删除该目录。

  兼容 Windows PowerShell 5.1：UTF-8（带 BOM）、LF 换行。

.PARAMETER RuntimeRoot
  Fox 受管运行时目录；默认 %ProgramFiles%\Fox\sandbox-runtime 下版本号最大的子目录。
.PARAMETER SrtPath
  srt 的 cli.js；默认 <RuntimeRoot>\srt\node_modules\@anthropic-ai\sandbox-runtime\dist\cli.js。
.PARAMETER NodePath
  node.exe；默认 <RuntimeRoot>\node\node.exe。
.PARAMETER BridgePath
  bridge.mjs；默认 <RuntimeRoot>\acp-bridge\bridge.mjs（不存在则跳过 srt-bridge 变体）。
.PARAMETER WorkDir
  授权目录（见上）。
.PARAMETER ResultPath
  结果 JSON；默认 %TEMP%\fox-diag-srt-stdio-<时间>.json。
.PARAMETER Variants
  要运行的变体，默认全部。
#>
[CmdletBinding()]
param(
    [string] $RuntimeRoot,
    [string] $SrtPath,
    [string] $NodePath,
    [string] $BridgePath,
    [string] $WorkDir,
    [string] $ResultPath,
    [string[]] $Variants = @('direct', 'srt', 'srt-no-node-options', 'srt-bridge')
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

# Fox's Windows allowlist (crates/fox-agent-backend/src/launch.rs WINDOWS_ENV_ALLOWLIST).
$EnvAllowlist = @('PATH', 'PATHEXT', 'SystemRoot', 'WINDIR', 'COMSPEC', 'TEMP', 'TMP', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH',
    'LOCALAPPDATA', 'APPDATA', 'PROGRAMDATA', 'PROGRAMFILES', 'PROGRAMFILES(X86)', 'PROGRAMW6432', 'USERNAME', 'USERDOMAIN',
    'NUMBER_OF_PROCESSORS', 'PROCESSOR_ARCHITECTURE', 'OS', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TERM')
$NodeOptions = '--preserve-symlinks --preserve-symlinks-main'

$EchoScript = @'
process.stdout.write('READY\n');process.stdin.on('data',d=>process.stdout.write('ECHO:'+d));process.stdin.on('end',()=>{process.stdout.write('END\n');process.exit(0)});setTimeout(()=>{process.stdout.write('TIMEOUT\n');process.exit(3)},15000)
'@.Trim()
# Bridge variant: a pipe cannot be half-closed, so the backend also ends on a QUIT line.
$BridgeEchoScript = @'
process.stdout.write('READY\n');process.stdin.on('data',d=>{process.stdout.write('ECHO:'+d);if(String(d).includes('QUIT')){process.stdout.write('END\n');process.exit(0)}});process.stdin.on('end',()=>{process.stdout.write('EOF\n');process.exit(0)});setTimeout(()=>{process.stdout.write('TIMEOUT\n');process.exit(3)},15000)
'@.Trim()

$script:Secrets = New-Object System.Collections.Generic.List[string]

function Hide-Secret([string] $Text) {
    if ($null -eq $Text) { return $null }
    $out = $Text
    foreach ($secret in $script:Secrets) { if ($secret) { $out = $out.Replace($secret, '[redacted]') } }
    $out = [regex]::Replace($out, '://[^@/\s"]+@', '://[redacted]@')
    $out = [regex]::Replace($out, '(?i)\b([A-Z0-9_]*(KEY|TOKEN|SECRET|PASSWORD))=\S+', '$1=[redacted]')
    return $out
}

function ConvertTo-ArgString([string[]] $Arguments) {
    # Windows command-line quoting (CommandLineToArgvW / MSVCRT rules), as Rust's
    # std::process::Command uses for Fox.
    $parts = foreach ($arg in $Arguments) {
        if ($arg -ne '' -and $arg -notmatch '[\s"]') { $arg; continue }
        $sb = New-Object System.Text.StringBuilder
        [void] $sb.Append('"')
        $backslashes = 0
        foreach ($ch in $arg.ToCharArray()) {
            if ($ch -eq '\') { $backslashes++; continue }
            if ($ch -eq '"') { [void] $sb.Append('\' * ($backslashes * 2 + 1)); [void] $sb.Append('"') }
            else { if ($backslashes) { [void] $sb.Append('\' * $backslashes) }; [void] $sb.Append($ch) }
            $backslashes = 0
        }
        [void] $sb.Append('\' * ($backslashes * 2))
        [void] $sb.Append('"')
        $sb.ToString()
    }
    return ($parts -join ' ')
}

function ConvertTo-CmdArg([string] $Text) {
    # Same rule as fox windows_command_line: quote when cmd.exe would split or interpret it.
    if ($Text -match '["%\r\n]') { throw "cannot pass through cmd.exe safely: $Text" }
    if ($Text -eq '' -or $Text -match '[ \t&|<>^(),;=!]') {
        $trailing = $Text.Length - $Text.TrimEnd('\').Length
        return '"' + $Text + ('\' * $trailing) + '"'
    }
    return $Text
}

function New-CmdLine([hashtable] $Vars, [string[]] $Command) {
    $parts = New-Object System.Collections.Generic.List[string]
    foreach ($key in ($Vars.Keys | Sort-Object)) { $parts.Add('set "' + $key + '=' + $Vars[$key] + '"') }
    $parts.Add((($Command | ForEach-Object { ConvertTo-CmdArg $_ }) -join ' '))
    return ($parts -join ' && ')
}

function New-StartInfo([string] $File, [string[]] $Arguments, [string] $Cwd) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $File
    $psi.Arguments = ConvertTo-ArgString $Arguments
    $psi.WorkingDirectory = $Cwd
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $psi.StandardErrorEncoding = New-Object System.Text.UTF8Encoding($false)
    $parent = [System.Environment]::GetEnvironmentVariables()
    $psi.EnvironmentVariables.Clear()
    foreach ($name in $parent.Keys) {
        if ($EnvAllowlist -contains ([string] $name).ToUpperInvariant() -or ($EnvAllowlist | Where-Object { $_ -ieq $name })) {
            $psi.EnvironmentVariables[[string] $name] = [string] $parent[$name]
        }
    }
    return $psi
}

function New-LineReader($Reader) {
    return @{ Reader = $Reader; Task = $Reader.ReadLineAsync(); Done = $false; Lines = (New-Object System.Collections.Generic.List[object]) }
}

function Step-LineReader($LineReader, $Clock) {
    while (-not $LineReader.Done -and $LineReader.Task.IsCompleted) {
        $line = $null
        try { $line = $LineReader.Task.Result } catch { $line = $null }
        if ($null -eq $line) { $LineReader.Done = $true; break }
        $LineReader.Lines.Add(@{ ms = $Clock.ElapsedMilliseconds; line = (Hide-Secret $line) })
        $LineReader.Task = $LineReader.Reader.ReadLineAsync()
    }
}

function Find-Line($LineReader, [string] $Pattern) {
    foreach ($entry in $LineReader.Lines) { if ($entry.line -like $Pattern) { return $entry } }
    return $null
}

function Wait-Until([scriptblock] $Condition, [int] $TimeoutMs, [object[]] $Readers, $Clock) {
    $deadline = $Clock.ElapsedMilliseconds + $TimeoutMs
    while ($Clock.ElapsedMilliseconds -lt $deadline) {
        foreach ($r in $Readers) { Step-LineReader $r $Clock }
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 5
    }
    foreach ($r in $Readers) { Step-LineReader $r $Clock }
    return [bool] (& $Condition)
}

function Format-Lines($LineReader) {
    return @($LineReader.Lines | ForEach-Object { '[{0,6} ms] {1}' -f $_.ms, $_.line })
}

function Invoke-StdioVariant([string] $Name, [string] $File, [string[]] $Arguments, [string] $Cwd) {
    $out = @{ variant = $Name; command = (Hide-Secret ($File + ' ' + (ConvertTo-ArgString $Arguments))); error = $null }
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = New-StartInfo $File $Arguments $Cwd
    try {
        [void] $proc.Start()
        $stdout = New-LineReader $proc.StandardOutput
        $stderr = New-LineReader $proc.StandardError
        $readers = @($stdout, $stderr)
        $ready = Wait-Until { [bool] (Find-Line $stdout 'READY*') } 20000 $readers $clock
        $out.readyMs = $(if ($ready) { (Find-Line $stdout 'READY*').ms } else { $null })
        $out.endBeforeWrite = [bool] (Find-Line $stdout 'END*')
        $out.writeMs = $clock.ElapsedMilliseconds
        try {
            $proc.StandardInput.Write("hello`n")
            $proc.StandardInput.Flush()
            $out.writeError = $null
        } catch {
            $out.writeError = $_.Exception.Message
        }
        # Keep stdin open 5 s.
        [void] (Wait-Until { $false } 5000 $readers $clock)
        $out.closeMs = $clock.ElapsedMilliseconds
        try { $proc.StandardInput.Close() } catch { $out.closeError = $_.Exception.Message }
        $exited = Wait-Until { $proc.HasExited -and $stdout.Done -and $stderr.Done } 20000 $readers $clock
        if (-not $exited) { try { $proc.Kill() } catch { $out.killError = $_.Exception.Message }; $out.killed = $true } else { $out.killed = $false }
        $echo = Find-Line $stdout 'ECHO:*'
        $end = Find-Line $stdout 'END*'
        $out.echoReceived = [bool] $echo
        $out.echoMs = $(if ($echo) { $echo.ms } else { $null })
        $out.endReceived = [bool] $end
        $out.endMs = $(if ($end) { $end.ms } else { $null })
        $out.endBeforeStdinClosed = [bool] ($end -and $end.ms -lt $out.closeMs)
        $out.stdinEofAfterReadyMs = $(if ($end -and $null -ne $out.readyMs -and $end.ms -lt $out.closeMs) { $end.ms - $out.readyMs } else { $null })
        $out.timeoutLine = [bool] (Find-Line $stdout 'TIMEOUT*')
        $out.exitCode = $(if ($proc.HasExited) { $proc.ExitCode } else { $null })
        $out.exitMs = $clock.ElapsedMilliseconds
        # Real time: READY arrived well before the process ended.
        $out.stdoutRealtime = [bool] ($null -ne $out.readyMs -and $null -ne $out.exitCode -and $out.readyMs + 1000 -lt $out.closeMs)
        $out.stdout = Format-Lines $stdout
        $out.stderr = Format-Lines $stderr
    } catch {
        $out.error = Hide-Secret $_.Exception.Message
    } finally {
        if (-not $proc.HasExited) { try { $proc.Kill() } catch { Write-Verbose 'kill failed' } }
        $proc.Dispose()
    }
    return $out
}

function Invoke-BridgeVariant([string] $Node, [string] $Srt, [string] $Settings, [string] $Bridge, [string] $Cwd, [string] $SrtSid, [hashtable] $Vars) {
    $out = @{ variant = 'srt-bridge'; error = $null }
    $name = 'fox-acp-diag-' + [guid]::NewGuid().ToString('N')
    $tokenBytes = New-Object byte[] 16
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $rng.GetBytes($tokenBytes)
    $rng.Dispose()
    $token = -join ($tokenBytes | ForEach-Object { $_.ToString('x2') })
    $script:Secrets.Add($token)
    $user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $security = New-Object System.IO.Pipes.PipeSecurity
    $security.SetAccessRuleProtection($true, $false)
    $security.AddAccessRule((New-Object System.IO.Pipes.PipeAccessRule($user, [System.IO.Pipes.PipeAccessRights]::FullControl, [System.Security.AccessControl.AccessControlType]::Allow)))
    $sandbox = New-Object System.Security.Principal.SecurityIdentifier($SrtSid)
    $security.AddAccessRule((New-Object System.IO.Pipes.PipeAccessRule($sandbox, [System.IO.Pipes.PipeAccessRights]::ReadWrite, [System.Security.AccessControl.AccessControlType]::Allow)))
    $server = New-Object System.IO.Pipes.NamedPipeServerStream($name, [System.IO.Pipes.PipeDirection]::InOut, 1,
        [System.IO.Pipes.PipeTransmissionMode]::Byte, [System.IO.Pipes.PipeOptions]::Asynchronous, 4096, 4096, $security)
    $vars2 = @{}
    foreach ($k in $Vars.Keys) { $vars2[$k] = $Vars[$k] }
    $vars2['FOX_ACP_PIPE'] = '\\.\pipe\' + $name
    $vars2['FOX_ACP_TOKEN'] = $token
    $line = New-CmdLine $vars2 @($Node, $Bridge, '--', $Node, '-e', $BridgeEchoScript)
    $arguments = @($Srt, '--settings', $Settings, '-c', $line)
    $out.command = Hide-Secret ($Node + ' ' + (ConvertTo-ArgString $arguments))
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = New-StartInfo $Node $arguments $Cwd
    try {
        [void] $proc.Start()
        # Fox gives srt a null stdin in bridge mode; closing it at once is the nearest equivalent.
        $proc.StandardInput.Close()
        $stdout = New-LineReader $proc.StandardOutput
        $stderr = New-LineReader $proc.StandardError
        $readers = @($stdout, $stderr)
        $connect = $server.WaitForConnectionAsync()
        $connected = Wait-Until { $connect.IsCompleted -or $proc.HasExited } 30000 $readers $clock
        $out.connected = [bool] ($connected -and $connect.IsCompleted -and -not $connect.IsFaulted)
        $out.connectMs = $(if ($out.connected) { $clock.ElapsedMilliseconds } else { $null })
        if ($out.connected) {
            $encoding = New-Object System.Text.UTF8Encoding($false)
            $pipeReader = New-Object System.IO.StreamReader($server, $encoding)
            $pipeWriter = New-Object System.IO.StreamWriter($server, $encoding)
            $pipeWriter.NewLine = "`n"
            $pipe = New-LineReader $pipeReader
            $all = @($stdout, $stderr, $pipe)
            $gotHandshake = Wait-Until { $pipe.Lines.Count -gt 0 -or $pipe.Done } 10000 $all $clock
            $handshake = $(if ($pipe.Lines.Count -gt 0) { $pipe.Lines[0].line } else { $null })
            # Hide-Secret already replaced the token in the stored line.
            $out.handshakeOk = [bool] ($gotHandshake -and $handshake -eq 'FOX-ACP-BRIDGE/1 [redacted]')
            if ($out.handshakeOk) {
                $pipeWriter.Write("OK`n"); $pipeWriter.Flush()
                [void] (Wait-Until { [bool] (Find-Line $pipe 'READY*') } 15000 $all $clock)
                $out.readyViaPipeMs = $(if (Find-Line $pipe 'READY*') { (Find-Line $pipe 'READY*').ms } else { $null })
                $out.writeMs = $clock.ElapsedMilliseconds
                $pipeWriter.Write("hello`n"); $pipeWriter.Flush()
                [void] (Wait-Until { [bool] (Find-Line $pipe 'ECHO:hello*') } 10000 $all $clock)
                $echo = Find-Line $pipe 'ECHO:hello*'
                $out.echoViaPipe = [bool] $echo
                $out.echoRoundTripMs = $(if ($echo) { $echo.ms - $out.writeMs } else { $null })
                $out.earlyEofViaPipe = [bool] (Find-Line $pipe 'EOF*')
                $pipeWriter.Write("QUIT`n"); $pipeWriter.Flush()
                [void] (Wait-Until { [bool] (Find-Line $pipe 'END*') -or $pipe.Done } 10000 $all $clock)
                $out.endViaPipe = [bool] (Find-Line $pipe 'END*')
            }
            $out.pipe = Format-Lines $pipe
        }
        try { $server.Dispose() } catch { Write-Verbose 'pipe dispose failed' }
        $exited = Wait-Until { $proc.HasExited -and $stdout.Done -and $stderr.Done } 20000 $readers $clock
        if (-not $exited) { try { $proc.Kill() } catch { $out.killError = $_.Exception.Message }; $out.killed = $true } else { $out.killed = $false }
        $out.exitCode = $(if ($proc.HasExited) { $proc.ExitCode } else { $null })
        $out.stdout = Format-Lines $stdout
        $out.stderr = Format-Lines $stderr
    } catch {
        $out.error = Hide-Secret $_.Exception.Message
    } finally {
        if (-not $proc.HasExited) { try { $proc.Kill() } catch { Write-Verbose 'kill failed' } }
        $proc.Dispose()
        $server.Dispose()
    }
    return $out
}

$result = @{ ok = $false; error = $null; startedAt = (Get-Date).ToUniversalTime().ToString('o'); variants = @() }
$createdWorkDir = $null
try {
    $identity = $null
    try {
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
        $result.elevated = $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
    } catch {
        $result.elevated = $null
    }
    if ($result.elevated) { Write-Warning 'running elevated: run this as the normal user instead (Fox does)' }
    $result.psVersion = [string] $PSVersionTable.PSVersion
    $result.os = [string] [System.Environment]::OSVersion.VersionString

    $programFiles = $(if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles })
    if (-not $RuntimeRoot) {
        $parent = Join-Path (Join-Path $programFiles 'Fox') 'sandbox-runtime'
        $latest = Get-ChildItem -LiteralPath $parent -Directory -ErrorAction Stop | Where-Object { $_.Name -notlike '.*' } | Sort-Object { [version] ($_.Name -replace '[^0-9.].*$', '') } -Descending -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $latest) { throw "no runtime under $parent; pass -RuntimeRoot" }
        $RuntimeRoot = $latest.FullName
    }
    if (-not $NodePath) { $NodePath = Join-Path $RuntimeRoot 'node\node.exe' }
    if (-not $SrtPath) { $SrtPath = Join-Path $RuntimeRoot 'srt\node_modules\@anthropic-ai\sandbox-runtime\dist\cli.js' }
    if (-not $BridgePath) { $BridgePath = Join-Path $RuntimeRoot 'acp-bridge\bridge.mjs' }
    foreach ($p in @($NodePath, $SrtPath)) { if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { throw "not found: $p" } }
    $result.runtimeRoot = $RuntimeRoot
    $result.nodePath = $NodePath
    $result.srtPath = $SrtPath
    $result.bridgePath = $BridgePath
    $result.nodeVersion = (& $NodePath --version) -join ''
    $srtPackage = Join-Path (Split-Path -Parent (Split-Path -Parent $SrtPath)) 'package.json'
    if (Test-Path -LiteralPath $srtPackage) { $result.srtVersion = [string] (Get-Content -LiteralPath $srtPackage -Raw | ConvertFrom-Json).version }

    $srtSid = $null
    try { $srtSid = [string] (Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\sandbox-runtime' -ErrorAction Stop).SandboxUserSid } catch { $srtSid = $null }
    $result.srtSandboxSid = $srtSid

    if (-not $WorkDir) {
        $base = Join-Path (Join-Path (Join-Path $env:ProgramData 'Fox') 'sandbox') $identity.User.Value
        $tmpBase = Join-Path $base 'tmp'
        if (-not (Test-Path -LiteralPath $tmpBase)) { $tmpBase = $env:TEMP }
        $WorkDir = Join-Path $tmpBase ('fox-diag-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    }
    if (-not (Test-Path -LiteralPath $WorkDir)) { New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null; $createdWorkDir = $WorkDir }
    $result.workDir = $WorkDir
    $settingsPath = Join-Path $WorkDir 'srt-settings.json'
    $settings = @{
        network = @{ allowedDomains = @(); deniedDomains = @(); allowLocalBinding = $false }
        filesystem = @{ denyRead = @(); allowRead = @(@{ path = $WorkDir; literal = $true }); allowWrite = @(@{ path = $WorkDir; literal = $true }); denyWrite = @() }
        enableWeakerNestedSandbox = $false
        allowNestedUserNamespaces = $false
    }
    [System.IO.File]::WriteAllText($settingsPath, ($settings | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))
    # Settings outside the authorized folder, like Fox (the sandbox must not read them): move next to it.
    $settingsOutside = Join-Path (Split-Path -Parent $WorkDir) ((Split-Path -Leaf $WorkDir) + '-srt-settings.json')
    Move-Item -LiteralPath $settingsPath -Destination $settingsOutside -Force
    $settingsPath = $settingsOutside

    # Session variables as Fox sets them (private HOME stand-in: the work dir).
    $vars = @{
        HOME = $WorkDir; USERPROFILE = $WorkDir; TMP = $WorkDir; TEMP = $WorkDir; TMPDIR = $WorkDir; CLAUDE_CODE_TMPDIR = $WorkDir
        APPDATA = (Join-Path $WorkDir 'AppData\Roaming'); LOCALAPPDATA = (Join-Path $WorkDir 'AppData\Local')
        NODE_USE_ENV_PROXY = '1'; PATH = ((Split-Path -Parent $NodePath) + ';' + $env:SystemRoot + '\System32;' + $env:SystemRoot)
        NODE_OPTIONS = $NodeOptions
    }
    $varsNoNodeOptions = @{}
    foreach ($k in $vars.Keys) { if ($k -ne 'NODE_OPTIONS') { $varsNoNodeOptions[$k] = $vars[$k] } }

    $list = New-Object System.Collections.Generic.List[object]
    foreach ($variant in $Variants) {
        Write-Verbose "variant $variant"
        switch ($variant) {
            'direct' { $list.Add((Invoke-StdioVariant 'direct' $NodePath @('-e', $EchoScript) $WorkDir)) }
            'srt' {
                $line = New-CmdLine $vars @($NodePath, '-e', $EchoScript)
                $list.Add((Invoke-StdioVariant 'srt' $NodePath @($SrtPath, '--settings', $settingsPath, '-c', $line) $WorkDir))
            }
            'srt-no-node-options' {
                $line = New-CmdLine $varsNoNodeOptions @($NodePath, '-e', $EchoScript)
                $list.Add((Invoke-StdioVariant 'srt-no-node-options' $NodePath @($SrtPath, '--settings', $settingsPath, '-c', $line) $WorkDir))
            }
            'srt-bridge' {
                if (-not (Test-Path -LiteralPath $BridgePath -PathType Leaf)) { $list.Add(@{ variant = 'srt-bridge'; skipped = "bridge not found: $BridgePath" }); break }
                if (-not $srtSid) { $list.Add(@{ variant = 'srt-bridge'; skipped = 'no SandboxUserSid in HKLM\SOFTWARE\sandbox-runtime' }); break }
                $list.Add((Invoke-BridgeVariant $NodePath $SrtPath $settingsPath $BridgePath $WorkDir $srtSid $vars))
            }
            default { $list.Add(@{ variant = $variant; skipped = 'unknown variant' }) }
        }
    }
    $result.variants = $list.ToArray()
    $result.ok = $true
} catch {
    $result.error = Hide-Secret $_.Exception.Message
    $result.errorAt = Hide-Secret ([string] $_.InvocationInfo.PositionMessage)
} finally {
    if ($createdWorkDir) {
        try { Remove-Item -LiteralPath $createdWorkDir -Recurse -Force } catch { $result.cleanupError = $_.Exception.Message }
        $leftover = (Split-Path -Parent $createdWorkDir) + '\' + (Split-Path -Leaf $createdWorkDir) + '-srt-settings.json'
        if (Test-Path -LiteralPath $leftover) { Remove-Item -LiteralPath $leftover -Force -ErrorAction SilentlyContinue }
    }
}
if (-not $ResultPath) { $ResultPath = Join-Path $env:TEMP ('fox-diag-srt-stdio-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.json') }
$json = $result | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($ResultPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output "result: $ResultPath"
foreach ($v in $result.variants) {
    if ($v.ContainsKey('skipped')) { Write-Output ("{0,-20} skipped: {1}" -f $v.variant, $v.skipped); continue }
    if ($v.variant -eq 'srt-bridge') {
        Write-Output ("{0,-20} connected={1} handshake={2} echoViaPipe={3} endViaPipe={4} exit={5} error={6}" -f $v.variant, $v['connected'], $v['handshakeOk'], $v['echoViaPipe'], $v['endViaPipe'], $v['exitCode'], $v.error)
    } else {
        Write-Output ("{0,-20} echo={1} end={2} endBeforeStdinClosed={3} exit={4} error={5}" -f $v.variant, $v['echoReceived'], $v['endReceived'], $v['endBeforeStdinClosed'], $v['exitCode'], $v.error)
    }
}
if ($result.ok) { exit 0 } else { exit 1 }
