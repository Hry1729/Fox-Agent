#Requires -Version 5.1
<#
.SYNOPSIS
  Run the real Zvec offline smoke against the pinned native resource.

.DESCRIPTION
  Preparation and inference are deliberately separate. The preparation script
  downloads/validates the exact zvec-rust-sys 0.6.0 Windows x64/MSVC archive.
  This script then links the test executable against the matching
  zvec_c_api.lib and proves the test executable loaded the same DLL.

  The test itself is the real Rust smoke assertion: create, write, flush,
  vector search, count/dimension verification, drop, reopen, and search again.
  Missing or mismatched resources fail; they never turn into a green skip.
#>
[CmdletBinding()]
param(
    [string] $ResourceDir = '',
    [string] $TargetDir = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $ResourceDir) { $ResourceDir = Join-Path $repoRoot '.test-target\zvec\0.6.0' }
if (-not $TargetDir) { $TargetDir = Join-Path $repoRoot '.test-target\zvec-target' }
$expectedDll = '2137ad6f24c00e8337efd54d8408df4690aa356773ab596beb495956dfeb7cd4'
$expectedLib = 'c5fa14ea2530726be869f784a0521c1697a4216ef4c916282805fbed6fe2722d'

& (Join-Path $PSScriptRoot 'prepare-zvec-resource.ps1') -Destination $ResourceDir

$recordPath = Join-Path $ResourceDir 'resource.json'
if (-not (Test-Path -LiteralPath $recordPath -PathType Leaf)) {
    throw "Zvec resource record is missing: $recordPath"
}
$record = Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json
if ($record.packageVersion -ne '0.6.0' -or
    $record.archiveSha256 -ne 'c823448a39a0f69aacdfe1fe06aaccb0e26eb9e0a4eea65fbd96edf749093152' -or
    $record.dllSha256 -ne $expectedDll -or $record.libSha256 -ne $expectedLib) {
    throw 'Zvec resource record does not match the pinned identity'
}

$lockPath = Join-Path $repoRoot 'apps\desktop\src-tauri\Cargo.lock'
$lock = Get-Content -LiteralPath $lockPath -Raw
if ($lock -notmatch '(?m)^name = "zvec-rust-sys"\r?\nversion = "0\.6\.0"$') {
    throw 'Cargo.lock does not pin zvec-rust-sys 0.6.0'
}

$env:ZVEC_LIB_DIR = [System.IO.Path]::GetFullPath($ResourceDir)
$env:ZVEC_AUTO_BUILD = '0'

$cargoArgs = @(
    'test',
    '--manifest-path', (Join-Path $repoRoot 'apps\desktop\src-tauri\Cargo.toml'),
    '--target-dir', [System.IO.Path]::GetFullPath($TargetDir),
    '--locked',
    '--no-default-features',
    '--features', 'zvec',
    'zvec_vector_store_offline_smoke',
    '--',
    '--nocapture'
)
Write-Output ('cargo ' + ($cargoArgs -join ' '))
$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    $output = & cargo @cargoArgs 2>&1 | Tee-Object -Variable smokeOutput
    $exitCode = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorAction
}
$text = ($smokeOutput | Out-String)
Write-Output $text
if ($exitCode -ne 0) { throw "Zvec offline smoke failed with exit code $exitCode" }
if ($text -notmatch 'vector_store::tests::zvec_vector_store_offline_smoke \.\.\. ok' -or
    $text -notmatch 'test result: ok\. 1 passed') {
    throw 'Zvec smoke command exited 0 without executing the real smoke test'
}

$stagedDll = Join-Path ([System.IO.Path]::GetFullPath($TargetDir)) 'debug\deps\zvec_c_api.dll'
if (-not (Test-Path -LiteralPath $stagedDll -PathType Leaf)) {
    throw "Zvec test executable has no staged DLL: $stagedDll"
}
$stagedHash = (Get-FileHash -LiteralPath $stagedDll -Algorithm SHA256).Hash.ToLowerInvariant()
$libHash = (Get-FileHash -LiteralPath (Join-Path $ResourceDir 'zvec_c_api.lib') -Algorithm SHA256).Hash.ToLowerInvariant()
if ($stagedHash -ne $expectedDll) { throw "staged Zvec DLL hash mismatch: $stagedHash" }
if ($libHash -ne $expectedLib) { throw "Zvec import library hash mismatch: $libHash" }

Write-Output 'Zvec offline smoke passed.'
Write-Output "test=vector_store::tests::zvec_vector_store_offline_smoke"
Write-Output "assertions=create,upsert,flush,search,verify,drop,reopen,search"
Write-Output "dll=$stagedHash"
Write-Output "importLibrary=$libHash"
Write-Output "resourceRecord=$recordPath"
