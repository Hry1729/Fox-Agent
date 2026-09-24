# Controlled launcher regression fixture; no Cargo build or downloaded DLLs.
# Run from the repository root with: pwsh -NoProfile -File ./scripts/test-desktop-rust-executable.test.ps1
# It copies the Windows cmd.exe into a temporary Cargo-shaped tree, then removes
# only that unique temporary tree. Runner logs and this transcript stay under
# .test-target for review.
#requires -Version 7.4
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$runner = Join-Path $PSScriptRoot 'test-desktop-rust.ps1'
$pwshPath = Join-Path $PSHOME 'pwsh.exe'
$cmdPath = Join-Path $env:SystemRoot 'System32/cmd.exe'
foreach ($requiredPath in @($runner, $pwshPath, $cmdPath)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Required fixture input is absent: $requiredPath"
    }
}

$evidenceDirectory = Join-Path $repoRoot '.test-target/test-desktop-rust-fixtures'
New-Item -ItemType Directory -Path $evidenceDirectory -Force | Out-Null
$evidencePath = Join-Path $evidenceDirectory "executable-fixture-$([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')).log"
$fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) "FoxRustLauncherFixture-$([Guid]::NewGuid().ToString('N'))"
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\', '/')
$fixtureRoot = [System.IO.Path]::GetFullPath($fixtureRoot)
$tempPrefix = $tempRoot + [System.IO.Path]::DirectorySeparatorChar
if (-not $fixtureRoot.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create fixture outside the temp directory: $fixtureRoot"
}

$savedCargoTarget = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
$savedZvecLibrary = [Environment]::GetEnvironmentVariable('ZVEC_LIB_DIR', 'Process')
$null = Start-Transcript -Path $evidencePath -Force
try {
    $targetRoot = Join-Path $fixtureRoot 'target'
    $executableDirectory = Join-Path $targetRoot 'x86_64-pc-windows-msvc/debug/deps'
    $profileDirectory = Split-Path -Parent $executableDirectory
    $profileBuildDirectory = Join-Path $profileDirectory 'build'
    $selectedExecutable = Join-Path $executableDirectory 'launcher-fixture.exe'
    $decoyTargetDirectory = Join-Path $fixtureRoot 'decoy-target'
    $decoyDllDirectory = Join-Path $decoyTargetDirectory 'debug/build/zvec-rust-sys-decoy/out/zvec-prebuilt'
    $decoyDll = Join-Path $decoyDllDirectory 'zvec_c_api.dll'

    New-Item -ItemType Directory -Path $executableDirectory -Force | Out-Null
    New-Item -ItemType Directory -Path $decoyDllDirectory -Force | Out-Null
    Copy-Item -LiteralPath $cmdPath -Destination $selectedExecutable
    Set-Content -LiteralPath $decoyDll -Value 'decoy-only-fixture' -NoNewline
    $env:CARGO_TARGET_DIR = $decoyTargetDirectory
    Remove-Item -LiteralPath Env:ZVEC_LIB_DIR -ErrorAction SilentlyContinue

    function ConvertTo-PowerShellLiteral([string]$value) {
        return "'" + $value.Replace("'", "''") + "'"
    }

    function Invoke-LauncherFixture([string]$name, [string]$runnerArguments) {
        Write-Host "=== $name ==="
        $runnerCommand = "& $(ConvertTo-PowerShellLiteral $runner) -Mode Executable -ExecutablePath $(ConvertTo-PowerShellLiteral $selectedExecutable) $runnerArguments"
        $output = & $pwshPath -NoLogo -NoProfile -Command $runnerCommand 2>&1
        $exitCode = $LASTEXITCODE
        $text = ($output | ForEach-Object { $_.ToString() }) -join "`n"
        if ($text) { Write-Host $text }
        Write-Host "fixture child exit code: $exitCode"
        return [PSCustomObject]@{ ExitCode = $exitCode; Output = $text }
    }

    $noZvec = Invoke-LauncherFixture 'explicit no-Zvec launch with no adjacent or matching DLL' "-NoZvec -TimeoutSeconds 15 -ExecutableArguments @('/c', 'exit 0')"
    if ($noZvec.ExitCode -ne 0 -or $noZvec.Output -notmatch 'Skipping Zvec DLL preflight') {
        throw 'The explicit no-Zvec executable did not launch without a DLL preflight.'
    }

    $missingActual = Invoke-LauncherFixture 'required Zvec fails closed when only the CARGO_TARGET_DIR decoy has a DLL' "-TimeoutSeconds 15 -ExecutableArguments @('/c', 'exit 0')"
    if ($missingActual.ExitCode -eq 0 -or
        $missingActual.Output -notmatch 'No built zvec_c_api.dll found under' -or
        $missingActual.Output -notmatch [regex]::Escape('x86_64-pc-windows-msvc\debug\build')) {
        throw 'Required-Zvec preflight did not fail closed against the selected executable profile.'
    }

    $actualDllDirectory = Join-Path $profileBuildDirectory 'zvec-rust-sys-fixture/out/zvec-prebuilt'
    New-Item -ItemType Directory -Path $actualDllDirectory -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $actualDllDirectory 'zvec_c_api.dll') -Value 'selected-profile-fixture' -NoNewline
    $profileDll = Invoke-LauncherFixture 'required Zvec resolves the selected target-triple profile' "-TimeoutSeconds 15 -ExecutableArguments @('/c', 'exit 0')"
    if ($profileDll.ExitCode -ne 0 -or
        $profileDll.Output -notmatch [regex]::Escape("Using built Zvec DLL directory: $actualDllDirectory")) {
        throw 'Required-Zvec resolution did not use the selected executable target triple/profile.'
    }

    $adjacentDll = Join-Path $executableDirectory 'zvec_c_api.dll'
    Set-Content -LiteralPath $adjacentDll -Value 'app-local-fixture' -NoNewline
    $sidecarDll = Invoke-LauncherFixture 'required Zvec prefers the DLL adjacent to the selected test executable' "-TimeoutSeconds 15 -ExecutableArguments @('/c', 'exit 0')"
    if ($sidecarDll.ExitCode -ne 0 -or
        $sidecarDll.Output -notmatch [regex]::Escape("Using built Zvec DLL directory: $executableDirectory")) {
        throw 'Required-Zvec resolution did not prefer the executable-adjacent app-local DLL.'
    }

    $failingChild = Invoke-LauncherFixture 'nonzero executable result remains visible' "-NoZvec -TimeoutSeconds 15 -ExecutableArguments @('/c', 'exit 23')"
    if ($failingChild.ExitCode -eq 0 -or $failingChild.Output -notmatch '\[test\] exited 23') {
        throw 'The launcher hid a nonzero child result.'
    }

    Write-Host 'PASS: no-Zvec bypass, fail-closed Zvec preflight, target-triple profile resolution, adjacent DLL resolution, and child failure propagation.'
    Write-Host "Evidence: $evidencePath"
} finally {
    if ($null -eq $savedCargoTarget) {
        Remove-Item -LiteralPath Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    } else {
        [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $savedCargoTarget, 'Process')
    }
    if ($null -eq $savedZvecLibrary) {
        Remove-Item -LiteralPath Env:ZVEC_LIB_DIR -ErrorAction SilentlyContinue
    } else {
        [Environment]::SetEnvironmentVariable('ZVEC_LIB_DIR', $savedZvecLibrary, 'Process')
    }
    if (Test-Path -LiteralPath $fixtureRoot) {
        $resolvedFixtureRoot = [System.IO.Path]::GetFullPath($fixtureRoot)
        if (-not $resolvedFixtureRoot.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove fixture outside the temp directory: $resolvedFixtureRoot"
        }
        Remove-Item -LiteralPath $resolvedFixtureRoot -Recurse -Force
    }
    Stop-Transcript | Out-Null
}
