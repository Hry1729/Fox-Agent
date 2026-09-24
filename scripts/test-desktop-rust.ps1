# Run with PowerShell 7 (pwsh) from any directory:
#   ./scripts/test-desktop-rust.ps1 -Mode Suite
#   ./scripts/test-desktop-rust.ps1 -Mode Suite -DefaultFeatures
#   ./scripts/test-desktop-rust.ps1 -Mode Executable -ExecutablePath <test-exe> -TargetDirectory <target-dir>
#   ./scripts/test-desktop-rust.ps1 -Mode Executable -ExecutablePath <test-exe> -NoZvec
#   ./scripts/test-desktop-rust.ps1 -Mode Executable -ExecutablePath <test-exe> -ScratchRoot <short-dir>
# For an executable under <target>/<triple?>/<profile>/deps, its own profile is
# authoritative for DLL lookup; -TargetDirectory/CARGO_TARGET_DIR are fallbacks
# only when the executable is outside that Cargo layout.
# Suite and Performance compile with --no-default-features unless -DefaultFeatures
# is given. Suite keeps Rust's default test parallelism; Performance preserves its
# existing isolated one-thread check. Executable only launches an already built
# test binary: it does not recompile SQLite or change that binary's MEMSTATUS.
# Executable mode defaults to the historical Zvec preflight. Pass -NoZvec only
# when the selected binary was built without the zvec feature; dependency features
# cannot be determined reliably from a prebuilt test executable.
# LIBSQLITE3_FLAGS and DLL search path are scoped to this PowerShell process.
#requires -Version 7.4
param(
    [ValidateSet('Suite', 'Performance', 'Executable')]
    [string]$Mode = 'Suite',
    [switch]$DefaultFeatures,
    [switch]$NoZvec,
    [string]$ExecutablePath,
    [string[]]$ExecutableArguments = @(),
    [string]$TargetDirectory,
    [string]$ScratchRoot,
    [ValidateRange(1, 86400)]
    [int]$TimeoutSeconds = 1800
)

$ErrorActionPreference = 'Stop'
if ($Mode -eq 'Executable' -and -not $ExecutablePath) {
    throw 'Executable mode requires -ExecutablePath.'
}
if ($NoZvec -and $Mode -ne 'Executable') {
    throw '-NoZvec is only valid in Executable mode.'
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = 'apps/desktop/src-tauri/Cargo.toml'
$selectedExecutable = $null
$executableDirectory = $null
$executableProfileDirectory = $null
if ($Mode -eq 'Executable') {
    $selectedExecutable = [System.IO.Path]::GetFullPath($ExecutablePath, $repoRoot)
    if (-not (Test-Path -LiteralPath $selectedExecutable -PathType Leaf)) {
        throw "Test executable is absent: $selectedExecutable"
    }
    $executableDirectory = Split-Path -Parent $selectedExecutable
    if ((Split-Path -Leaf $executableDirectory) -eq 'deps') {
        # Cargo test executables live at <target>/<triple?>/<profile>/deps.
        # Use this exact profile for DLL discovery; CARGO_TARGET_DIR can refer to
        # a different target tree when several builds are present.
        $executableProfileDirectory = Split-Path -Parent $executableDirectory
    }
}
$targetDirectory = if ($Mode -eq 'Executable' -and $executableProfileDirectory) {
    Split-Path -Parent $executableProfileDirectory
} elseif ($TargetDirectory) {
    [System.IO.Path]::GetFullPath($TargetDirectory, $repoRoot)
} elseif ($env:CARGO_TARGET_DIR) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repoRoot)
} elseif ($Mode -eq 'Executable' -and $NoZvec) {
    # No Cargo tree is needed to launch a binary explicitly declared Zvec-free.
    $null
} elseif ($Mode -eq 'Executable') {
    throw 'Cannot resolve the selected executable Cargo profile. Place it under <target>/<triple?>/<profile>/deps, or provide -TargetDirectory/CARGO_TARGET_DIR for Zvec DLL lookup.'
} else {
    Join-Path $repoRoot 'apps/desktop/src-tauri/target'
}
$zvecBuildDirectory = if ($Mode -eq 'Executable' -and $executableProfileDirectory) {
    Join-Path $executableProfileDirectory 'build'
} elseif ($targetDirectory) {
    Join-Path $targetDirectory 'debug/build'
} else {
    $null
}
$runId = [System.IO.Path]::GetRandomFileName().Substring(0, 8)
$runDirectory = Join-Path $repoRoot ".test-target/rust-host-runs/$runId"
$scratchBase = if ($ScratchRoot) {
    [System.IO.Path]::GetFullPath($ScratchRoot, $repoRoot)
} else {
    [System.IO.Path]::GetTempPath()
}
$scratchDirectory = Join-Path $scratchBase "fxr-$runId"
$tempDirectory = Join-Path $scratchDirectory 't'
# Zvec's real test files add deep subpaths. Keep the scratch prefix short and
# fail with a useful path diagnostic before the native test reports MAX_PATH.
if ((Join-Path $scratchDirectory 'd').Length -gt 64 -or $tempDirectory.Length -gt 64) {
    throw "Rust test scratch path is too long ($scratchDirectory). Use -ScratchRoot with a short directory; relative paths resolve from $repoRoot."
}
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
New-Item -ItemType Directory -Path $tempDirectory -Force | Out-Null
Write-Host "Rust test scratch: $scratchDirectory"
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
    if ($Mode -eq 'Executable' -and $executableDirectory) {
        $adjacentDll = Join-Path $executableDirectory 'zvec_c_api.dll'
        if (Test-Path -LiteralPath $adjacentDll -PathType Leaf) {
            # The app-local DLL staged next to this exact test binary is the
            # runtime copy selected for its Cargo profile and the loader finds it first.
            return $executableDirectory
        }
    }
    if ($env:ZVEC_LIB_DIR) {
        $explicit = [System.IO.Path]::GetFullPath($env:ZVEC_LIB_DIR, $repoRoot)
        if (-not (Test-Path -LiteralPath (Join-Path $explicit 'zvec_c_api.dll') -PathType Leaf)) {
            throw "ZVEC_LIB_DIR is set but zvec_c_api.dll is absent: $explicit"
        }
        return $explicit
    }
    $buildDirectory = $zvecBuildDirectory
    if (-not $buildDirectory) {
        throw 'Cannot resolve the selected executable Zvec build directory. Provide -TargetDirectory/CARGO_TARGET_DIR or use a Cargo <profile>/deps executable.'
    }
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
        # Start-Process must receive the modified script environment explicitly:
        # PATH locates the real Zvec DLL and Cargo receives the SQLite build flag.
        Environment = @{
            PATH = $env:PATH
            FOX_DATA_DIR = $env:FOX_DATA_DIR
            TMP = $env:TMP
            TEMP = $env:TEMP
            CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR
            LIBSQLITE3_FLAGS = $env:LIBSQLITE3_FLAGS
        }
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
$savedErrorMode = $null
try {
    # Match the bounded review runner: children inherit this process error mode,
    # so loader/critical-error dialogs surface as exit codes instead of hanging.
    if (-not ('FoxTest.NativeErrorMode' -as [type])) {
        Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
namespace FoxTest {
    public static class NativeErrorMode {
        [DllImport("kernel32.dll")] public static extern uint GetErrorMode();
        [DllImport("kernel32.dll")] public static extern uint SetErrorMode(uint mode);
    }
}
'@
    }
    $savedErrorMode = [FoxTest.NativeErrorMode]::GetErrorMode()
    [void][FoxTest.NativeErrorMode]::SetErrorMode([uint32]($savedErrorMode -bor 0x8003))
    Write-Host "Windows error mode for test children: 0x$('{0:X4}' -f [FoxTest.NativeErrorMode]::GetErrorMode())"
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
        $zvecDirectory = $null
        if ($NoZvec) {
            Write-Host 'Skipping Zvec DLL preflight: -NoZvec asserts this executable was built without the zvec feature.'
        } else {
            $zvecDirectory = Find-ZvecDllDirectory
            Write-Host "Using built Zvec DLL directory: $zvecDirectory"
        }
        $depsDirectory = if ($executableProfileDirectory) {
            Join-Path $executableProfileDirectory 'deps'
        } elseif ($targetDirectory) {
            Join-Path $targetDirectory 'debug/deps'
        } else {
            $null
        }
        $pathEntries = @($zvecDirectory, $executableDirectory, $depsDirectory) |
            Where-Object { $_ } | Select-Object -Unique
        $env:PATH = (@($pathEntries + @($savedEnvironment['PATH'])) | Where-Object { $_ }) -join ';'
        Invoke-TestProcess 'test' $selectedExecutable $ExecutableArguments
    }
} finally {
    if ($null -ne $savedErrorMode) {
        [void][FoxTest.NativeErrorMode]::SetErrorMode([uint32]$savedErrorMode)
    }
    foreach ($name in $savedEnvironment.Keys) {
        if ($null -eq $savedEnvironment[$name]) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
        }
    }
}
