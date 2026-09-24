# Run with PowerShell 7 (pwsh) from any directory:
#   ./scripts/test-desktop-rust.ps1 -Mode Suite
#   ./scripts/test-desktop-rust.ps1 -Mode Suite -DefaultFeatures
#   ./scripts/test-desktop-rust.ps1 -Mode Executable -ExecutablePath <test-exe> -TargetDirectory <target-dir>
# Suite and Performance compile with --no-default-features unless -DefaultFeatures
# is given. Suite keeps Rust's default test parallelism; Performance preserves its
# existing isolated one-thread check. Executable only launches an already built
# test binary: it does not recompile SQLite or change that binary's MEMSTATUS.
# LIBSQLITE3_FLAGS and DLL search path are scoped to this PowerShell process.
#requires -Version 7.0
param(
    [ValidateSet('Suite', 'Performance', 'Executable')]
    [string]$Mode = 'Suite',
    [switch]$DefaultFeatures,
    [string]$ExecutablePath,
    [string[]]$ExecutableArguments = @(),
    [string]$TargetDirectory,
    [ValidateRange(1, 86400)]
    [int]$TimeoutSeconds = 1800
)

$ErrorActionPreference = 'Stop'
if ($Mode -eq 'Executable' -and -not $ExecutablePath) {
    throw 'Executable mode requires -ExecutablePath.'
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = 'apps/desktop/src-tauri/Cargo.toml'
$targetDirectory = if ($TargetDirectory) {
    [System.IO.Path]::GetFullPath($TargetDirectory, $repoRoot)
} elseif ($env:CARGO_TARGET_DIR) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repoRoot)
} elseif ($Mode -eq 'Executable') {
    $selectedParent = Split-Path -Parent ([System.IO.Path]::GetFullPath($ExecutablePath, $repoRoot))
    $debugDirectory = Split-Path -Parent $selectedParent
    if ((Split-Path -Leaf $selectedParent) -ne 'deps' -or
        (Split-Path -Leaf $debugDirectory) -ne 'debug') {
        throw 'Executable outside target/debug/deps: provide -TargetDirectory or CARGO_TARGET_DIR to select its Zvec build.'
    }
    Split-Path -Parent $debugDirectory
} else {
    Join-Path $repoRoot 'apps/desktop/src-tauri/target'
}
$runId = [System.IO.Path]::GetRandomFileName().Substring(0, 8)
$runDirectory = Join-Path $repoRoot ".test-target/rust-host-runs/$runId"
$scratchDirectory = Join-Path $repoRoot ".test-target/r/$runId"
$tempDirectory = Join-Path $scratchDirectory 't'
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
New-Item -ItemType Directory -Path $tempDirectory -Force | Out-Null
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)

function Get-SqliteTestFlags([string]$previous) {
    $tokens = @($previous -split '\s+' | Where-Object { $_ })
    $alreadyDisabled = $false
    foreach ($token in $tokens) {
        if ($token -match '^(?:-D)?SQLITE_DEFAULT_MEMSTATUS(?:=(.*))?$') {
            if ($Matches[1] -ne '0') {
                throw "LIBSQLITE3_FLAGS has conflicting $token; test SQLite requires SQLITE_DEFAULT_MEMSTATUS=0."
            }
            $alreadyDisabled = $true
        } elseif ($token -match '^-USQLITE_DEFAULT_MEMSTATUS$') {
            throw "LIBSQLITE3_FLAGS has conflicting $token; test SQLite requires SQLITE_DEFAULT_MEMSTATUS=0."
        }
    }
    if ($alreadyDisabled) { return $previous }
    return (($tokens + 'SQLITE_DEFAULT_MEMSTATUS=0') -join ' ')
}

function Find-ZvecDllDirectory {
    if ($env:ZVEC_LIB_DIR) {
        $explicit = [System.IO.Path]::GetFullPath($env:ZVEC_LIB_DIR, $repoRoot)
        if (-not (Test-Path -LiteralPath (Join-Path $explicit 'zvec_c_api.dll') -PathType Leaf)) {
            throw "ZVEC_LIB_DIR is set but zvec_c_api.dll is absent: $explicit"
        }
        return $explicit
    }
    $buildDirectory = Join-Path $targetDirectory 'debug/build'
    $candidates = @()
    if (Test-Path -LiteralPath $buildDirectory -PathType Container) {
        $candidates = @(Get-ChildItem -LiteralPath $buildDirectory -Directory -Filter 'zvec-rust-sys-*' |
            ForEach-Object { Join-Path $_.FullName 'out/zvec-prebuilt/zvec_c_api.dll' } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Sort-Object)
    }
    if ($candidates.Count -eq 0) {
        throw "No built zvec_c_api.dll found under $buildDirectory. Set ZVEC_LIB_DIR to the real DLL directory."
    }
    $byHash = @($candidates | Group-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash })
    if ($byHash.Count -ne 1) {
        throw "Several different zvec_c_api.dll files exist under $buildDirectory. Set ZVEC_LIB_DIR to the matching build's real DLL directory."
    }
    return Split-Path -Parent $candidates[0]
}

function Format-Argument([string]$value) {
    if ($value.Contains('"')) { throw "Unsupported quote in test argument: $value" }
    if ($value -match '\s') { return '"' + $value + '"' }
    return $value
}

function Stop-TimedOutProcess([System.Diagnostics.Process]$process) {
    $timedOutPid = $process.Id
    $stopNotes = [System.Collections.Generic.List[string]]::new()
    $killFailed = $false
    try {
        if (-not $process.HasExited) { $process.Kill($true) }
    } catch {
        $killFailed = $true
        $stopNotes.Add("Kill(true) failed for PID ${timedOutPid}: $($_.Exception.Message)")
    }
    $rootExited = $process.WaitForExit(5000)
    if ($rootExited -and -not $killFailed) { return ($stopNotes -join '; ') }

    $stopStdout = Join-Path $runDirectory 'taskkill.stdout.log'
    $stopStderr = Join-Path $runDirectory 'taskkill.stderr.log'
    try {
        $killer = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32/taskkill.exe') `
            -ArgumentList "/PID $timedOutPid /T /F" -WorkingDirectory $repoRoot `
            -RedirectStandardOutput $stopStdout -RedirectStandardError $stopStderr `
            -WindowStyle Hidden -PassThru
        if (-not $killer.WaitForExit(5000)) {
            $killer.Kill()
            $killer.WaitForExit(2000) | Out-Null
            $stopNotes.Add("taskkill timed out for PID $timedOutPid")
        } elseif ($killer.ExitCode -ne 0) {
            $stopNotes.Add("taskkill exited $($killer.ExitCode) for PID $timedOutPid; see $stopStderr")
        }
    } catch {
        $stopNotes.Add("taskkill failed for PID ${timedOutPid}: $($_.Exception.Message)")
    }
    if (-not $process.WaitForExit(5000)) {
        throw "Timed-out PID $timedOutPid is still running after bounded process-tree kill attempts. $($stopNotes -join '; ')"
    }
    return ($stopNotes -join '; ')
}

function Invoke-TestProcess([string]$phase, [string]$file, [string[]]$arguments) {
    $remaining = [int][Math]::Ceiling(($deadline - [DateTime]::UtcNow).TotalSeconds)
    if ($remaining -le 0) { throw "Rust Host test budget of $TimeoutSeconds seconds expired before $phase." }
    $stdout = Join-Path $runDirectory "$phase.stdout.log"
    $stderr = Join-Path $runDirectory "$phase.stderr.log"
    $argumentLine = (@($arguments | ForEach-Object { Format-Argument $_ }) -join ' ')
    Write-Host "[$phase] $file $argumentLine"
    Write-Host "[$phase] logs: $stdout ; $stderr"
    $start = @{
        FilePath = $file
        WorkingDirectory = $repoRoot
        RedirectStandardOutput = $stdout
        RedirectStandardError = $stderr
        WindowStyle = 'Hidden'
        PassThru = $true
    }
    if ($argumentLine) { $start.ArgumentList = $argumentLine }
    $process = Start-Process @start
    if (-not $process.WaitForExit($remaining * 1000)) {
        $stopNotes = Stop-TimedOutProcess $process
        throw "[$phase] exceeded the $TimeoutSeconds-second hard timeout. PID $($process.Id) stopped. $stopNotes Logs: $stdout ; $stderr"
    }
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        Write-Host "[$phase] stdout tail:"
        Get-Content -LiteralPath $stdout -Tail 30 | Write-Host
        Write-Host "[$phase] stderr tail:"
        Get-Content -LiteralPath $stderr -Tail 30 | Write-Host
        throw "[$phase] exited $($process.ExitCode). Logs: $stdout ; $stderr"
    }
    Write-Host "[$phase] passed. Logs: $stdout ; $stderr"
}

$savedEnvironment = @{}
foreach ($name in @('LIBSQLITE3_FLAGS', 'CARGO_TARGET_DIR', 'PATH', 'FOX_DATA_DIR', 'TMP', 'TEMP')) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $env:FOX_DATA_DIR = Join-Path $scratchDirectory 'd'
    $env:TMP = $tempDirectory
    $env:TEMP = $tempDirectory
    New-Item -ItemType Directory -Path $env:FOX_DATA_DIR -Force | Out-Null

    if ($Mode -ne 'Executable') {
        $env:CARGO_TARGET_DIR = $targetDirectory
        $env:LIBSQLITE3_FLAGS = Get-SqliteTestFlags $savedEnvironment['LIBSQLITE3_FLAGS']
        $cargoArguments = @('test', '--manifest-path', $manifest, '--locked')
        if (-not $DefaultFeatures) { $cargoArguments += '--no-default-features' }
        if ($DefaultFeatures) {
            # Compile first: the selected zvec-rust-sys output directory appears only after build.
            Invoke-TestProcess 'compile' 'cargo' ($cargoArguments + @('--no-run'))
            $zvecDirectory = Find-ZvecDllDirectory
            $depsDirectory = Join-Path $targetDirectory 'debug/deps'
            $env:PATH = "$zvecDirectory;$depsDirectory;$($savedEnvironment['PATH'])"
            Write-Host "Using built Zvec DLL directory: $zvecDirectory"
        } elseif ($env:ZVEC_LIB_DIR) {
            # Explicit ZVEC_LIB_DIR is validated even for an optional-Zvec build.
            $zvecDirectory = Find-ZvecDllDirectory
            $env:PATH = "$zvecDirectory;$($savedEnvironment['PATH'])"
        }
        if ($Mode -eq 'Suite') {
            $cargoArguments += @('--', '--skip', 'a0_snapshot_and_state_update_p95_stay_within_budget')
        } else {
            $cargoArguments += @('a0_snapshot_and_state_update_p95_stay_within_budget', '--', '--test-threads=1')
        }
        Invoke-TestProcess 'test' 'cargo' $cargoArguments
    } else {
        $selectedExecutable = [System.IO.Path]::GetFullPath($ExecutablePath, $repoRoot)
        if (-not (Test-Path -LiteralPath $selectedExecutable -PathType Leaf)) {
            throw "Test executable is absent: $selectedExecutable"
        }
        $zvecDirectory = Find-ZvecDllDirectory
        $depsDirectory = Join-Path $targetDirectory 'debug/deps'
        $env:PATH = "$zvecDirectory;$depsDirectory;$($savedEnvironment['PATH'])"
        Write-Host "Using built Zvec DLL directory: $zvecDirectory"
        Invoke-TestProcess 'test' $selectedExecutable $ExecutableArguments
    }
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
}
