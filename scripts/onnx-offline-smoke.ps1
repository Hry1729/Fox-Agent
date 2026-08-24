$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$modelDirectory = if ($env:FOX_ONNX_MODEL_DIR) {
    [System.IO.Path]::GetFullPath($env:FOX_ONNX_MODEL_DIR)
} else {
    Join-Path $repoRoot 'apps\desktop\src-tauri\resources\models\default'
}

if (-not (Test-Path -LiteralPath $modelDirectory -PathType Container)) {
    throw "ONNX offline gate failed: model package directory is missing: $modelDirectory"
}

$manifestPath = Join-Path $modelDirectory 'manifest.json'
$modelPath = Join-Path $modelDirectory 'model.onnx'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "ONNX offline gate failed: missing model manifest: $manifestPath"
}
if (-not (Test-Path -LiteralPath $modelPath -PathType Leaf)) {
    throw "ONNX offline gate failed: missing ONNX model: $modelPath"
}

$runtimePath = if ($env:ORT_DYLIB_PATH) {
    [System.IO.Path]::GetFullPath($env:ORT_DYLIB_PATH)
} else {
    Join-Path $modelDirectory 'onnxruntime.dll'
}
if (-not (Test-Path -LiteralPath $runtimePath -PathType Leaf)) {
    throw "ONNX offline gate failed: missing onnxruntime.dll. Set ORT_DYLIB_PATH or package it beside model.onnx: $runtimePath"
}

$env:ORT_DYLIB_PATH = $runtimePath
$manifestFile = Join-Path $repoRoot 'apps\desktop\src-tauri\Cargo.toml'
cargo run --manifest-path $manifestFile --locked --no-default-features --features local-embedding --bin local-embedding-smoke -- $modelDirectory
if ($LASTEXITCODE -ne 0) {
    throw "ONNX offline smoke failed with exit code $LASTEXITCODE."
}
