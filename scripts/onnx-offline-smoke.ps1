#Requires -Version 5.1
<#
.SYNOPSIS
  Run the real tokenizer + ONNX inference smoke against the pinned controlled
  model package.

.DESCRIPTION
  Model download/verification is owned by prepare-onnx-model.ps1. This script
  consumes only the prepared package, validates its manifest identity again,
  and runs the real `local-embedding-smoke` binary.

  The production `ort` configuration uses default-features = false with
  `download-binaries`; the build links ONNX Runtime statically. This smoke does
  NOT require, set, or search for a separate onnxruntime.dll. The `ORT_DYLIB_PATH`
  override is removed so the tested runtime is the one selected by Cargo.

  The binary tokenizes a query-prefixed and a passage-prefixed input, runs both
  through the real ONNX model, checks the 384-dimension output, finite values,
  and normalized vectors. Missing resources, a manifest/prefix mismatch, a
  model/tokenizer hash mismatch, or a dynamic runtime link are hard failures.
#>
[CmdletBinding()]
param(
    [string] $ModelDir = '',
    [string] $TargetDir = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $ModelDir) { $ModelDir = Join-Path $repoRoot '.test-target\onnx-models\multilingual-e5-small-761b726dd34fb83930e26aab4e9ac3899aa1fa78' }
if (-not $TargetDir) { $TargetDir = Join-Path $repoRoot '.test-target\onnx-target' }

& (Join-Path $PSScriptRoot 'prepare-onnx-model.ps1') -Destination $ModelDir

$manifestPath = Join-Path $ModelDir 'manifest.json'
$modelPath = Join-Path $ModelDir 'model.onnx'
$tokenizerPath = Join-Path $ModelDir 'tokenizer.json'
foreach ($path in @($manifestPath, $modelPath, $tokenizerPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "ONNX package is incomplete: missing $path" }
}

$expectedModelSha = '4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c'
$expectedTokenizerSha = '0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.id -ne 'multilingual-e5-small' -or
    $manifest.version -ne '761b726dd34fb83930e26aab4e9ac3899aa1fa78' -or
    $manifest.dimension -ne 384 -or
    $manifest.tokenizer -ne 'tokenizer.json' -or
    $manifest.hash -ne ('sha256:' + $expectedModelSha) -or
    $manifest.license -ne 'MIT' -or
    $manifest.pooling -ne 'mean' -or
    $manifest.normalize -ne $true -or
    $manifest.maxTokens -ne 512 -or
    $manifest.queryPrefix -ne 'query: ' -or
    $manifest.passagePrefix -ne 'passage: ') {
    throw 'ONNX manifest does not match the controlled E5 model contract'
}

$modelSha = (Get-FileHash -LiteralPath $modelPath -Algorithm SHA256).Hash.ToLowerInvariant()
$tokenizerSha = (Get-FileHash -LiteralPath $tokenizerPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($modelSha -ne $expectedModelSha) { throw "ONNX model hash mismatch: $modelSha" }
if ($tokenizerSha -ne $expectedTokenizerSha) { throw "ONNX tokenizer hash mismatch: $tokenizerSha" }

# The production behavior is static linking through ort-sys. Do not let an
# unrelated runtime DLL from the caller environment replace it.
Remove-Item Env:ORT_DYLIB_PATH -ErrorAction SilentlyContinue

$cargoArgs = @(
    'run',
    '--manifest-path', (Join-Path $repoRoot 'apps\desktop\src-tauri\Cargo.toml'),
    '--target-dir', [System.IO.Path]::GetFullPath($TargetDir),
    '--locked',
    '--no-default-features',
    '--features', 'local-embedding',
    '--bin', 'local-embedding-smoke',
    '--',
    [System.IO.Path]::GetFullPath($ModelDir)
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
if ($exitCode -ne 0) { throw "ONNX offline smoke failed with exit code $exitCode" }
if ($text -notmatch 'ONNX offline smoke passed: dimension=384') {
    throw 'ONNX smoke command exited 0 without the expected 384-dimension inference result'
}

$ortOutputs = @(Get-ChildItem -Path (Join-Path ([System.IO.Path]::GetFullPath($TargetDir)) 'debug\build') -Directory -Filter 'ort-sys-*' -ErrorAction SilentlyContinue | ForEach-Object {
    $out = Join-Path $_.FullName 'output'
    if (Test-Path -LiteralPath $out -PathType Leaf) { $out }
})
if ($ortOutputs.Count -eq 0) { throw 'ONNX build produced no ort-sys build output to verify' }
$static = $false
foreach ($out in $ortOutputs) {
    if (Select-String -LiteralPath $out -Pattern '^cargo:rustc-link-lib=static=onnxruntime$' -Quiet) { $static = $true }
}
if (-not $static) {
    throw 'ONNX Runtime was not statically linked by ort-sys; expected cargo:rustc-link-lib=static=onnxruntime'
}

$cargoVersion = (& cargo --version) -join ' '
$rustVersion = (& rustc --version) -join ' '
Write-Output 'ONNX offline smoke passed.'
Write-Output 'input=query-prefixed and passage-prefixed tokenizer inputs'
Write-Output 'assertions=manifest,model-sha256,tokenizer-sha256,prefixes,mean-pooling,normalize,dimension=384,finite,unit-norm'
Write-Output "modelSha256=$modelSha"
Write-Output "tokenizerSha256=$tokenizerSha"
Write-Output "runtimeLink=static onnxruntime (ort-sys download-binaries)"
Write-Output "modelRevision=$($manifest.version)"
Write-Output "modelSource=$($manifest.source)"
Write-Output "license=$($manifest.license)"
Write-Output "toolchain=$cargoVersion; $rustVersion"
Write-Output "modelDirectory=$ModelDir"
