$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$resourceDir = Join-Path $repoRoot 'apps\desktop\src-tauri\resources\vector'
$libraryDir = if ($env:ZVEC_LIB_DIR) { [System.IO.Path]::GetFullPath($env:ZVEC_LIB_DIR) } else { $resourceDir }
$dllPath = Join-Path $libraryDir 'zvec_c_api.dll'
$importLibraryPath = Join-Path $libraryDir 'zvec_c_api.lib'

if (-not (Test-Path -LiteralPath $dllPath -PathType Leaf)) {
    throw "Zvec offline smoke blocked: missing $dllPath. Provide the official Windows x64/MSVC zvec_c_api.dll via ZVEC_LIB_DIR or resources/vector."
}

if (-not (Test-Path -LiteralPath $importLibraryPath -PathType Leaf)) {
    throw "Zvec offline smoke blocked: missing $importLibraryPath. MSVC linking requires the official import library beside zvec_c_api.dll."
}

$env:ZVEC_LIB_DIR = $libraryDir
$env:ZVEC_AUTO_BUILD = '0'

cargo test --manifest-path (Join-Path $repoRoot 'apps\desktop\src-tauri\Cargo.toml') --locked --no-default-features --features zvec zvec_vector_store_offline_smoke -- --nocapture
if ($LASTEXITCODE -ne 0) {
    throw "Zvec offline smoke failed with exit code $LASTEXITCODE."
}
