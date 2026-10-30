#Requires -Version 5.1
<#
.SYNOPSIS
  Prepare the controlled multilingual-e5-small test package at a pinned
  Xenova/Hugging Face revision.

.DESCRIPTION
  Model preparation and inference are separate. This script downloads only the
  two files already listed by Fox's controlled model catalog:
    - onnx/model_int8.onnx
    - tokenizer.json
  It verifies the revision, byte sizes, and SHA-256 values before writing
  manifest.json. The manifest carries the dimension, pooling, normalization,
  query/passage prefixes, license, source, and revision used by the smoke test.

  The model and tokenizer are large third-party assets and are not committed to
  Git. CI caches the prepared directory by the pinned identity; a cache miss
  performs the verified download again.
#>
[CmdletBinding()]
param(
    [string] $Destination = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) { $Destination = Join-Path $repoRoot '.test-target\onnx-models\multilingual-e5-small-761b726dd34fb83930e26aab4e9ac3899aa1fa78' }

$modelId = 'multilingual-e5-small'
$modelName = 'Multilingual E5 Small'
$revision = '761b726dd34fb83930e26aab4e9ac3899aa1fa78'
$sourceUrl = 'https://huggingface.co/Xenova/multilingual-e5-small'
$rootUrl = "$sourceUrl/resolve/$revision"
$modelDownloadName = 'onnx/model_int8.onnx'
$modelUrl = "$rootUrl/$modelDownloadName`?download=true"
$tokenizerUrl = "$rootUrl/tokenizer.json`?download=true"
$modelSha256 = '4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c'
$tokenizerSha256 = '0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39'
$modelBytes = 118054593
$tokenizerBytes = 17082730
$dimension = 384
$license = 'MIT'
$promptQueryPrefix = 'query: '
$promptPassagePrefix = 'passage: '

function Get-FileSha256([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "missing file: $Path" }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-File([string] $Path, [string] $ExpectedSha256, [long] $ExpectedBytes) {
    $item = Get-Item -LiteralPath $Path
    if ($item.Length -ne $ExpectedBytes) {
        throw "size mismatch for $Path`: expected $ExpectedBytes, got $($item.Length)"
    }
    $actual = Get-FileSha256 $Path
    if ($actual -ne $ExpectedSha256) {
        throw "SHA-256 mismatch for $Path`: expected $ExpectedSha256, got $actual"
    }
    return $actual
}

function Test-PreparedModel([string] $Directory) {
    $manifestPath = Join-Path $Directory 'manifest.json'
    $modelPath = Join-Path $Directory 'model.onnx'
    $tokenizerPath = Join-Path $Directory 'tokenizer.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $modelPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $tokenizerPath -PathType Leaf)) {
        return $false
    }
    if ((Get-FileSha256 $modelPath) -ne $modelSha256) { return $false }
    if ((Get-FileSha256 $tokenizerPath) -ne $tokenizerSha256) { return $false }
    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        return $manifest.id -eq $modelId -and
            $manifest.version -eq $revision -and
            $manifest.dimension -eq $dimension -and
            $manifest.hash -eq ('sha256:' + $modelSha256) -and
            $manifest.tokenizer -eq 'tokenizer.json' -and
            $manifest.license -eq $license -and
            $manifest.pooling -eq 'mean' -and
            $manifest.normalize -eq $true -and
            $manifest.queryPrefix -eq $promptQueryPrefix -and
            $manifest.passagePrefix -eq $promptPassagePrefix -and
            $manifest.modelSha256 -eq $modelSha256 -and
            $manifest.tokenizerSha256 -eq $tokenizerSha256
    } catch {
        return $false
    }
}

if (Test-PreparedModel $Destination) {
    Write-Output "ONNX model already prepared: $Destination"
    Write-Output "model=$modelId revision=$revision license=$license dimension=$dimension"
    Write-Output "modelSha256=$modelSha256 tokenizerSha256=$tokenizerSha256"
    return
}

if (Test-Path -LiteralPath $Destination -PathType Container) {
    throw "ONNX model directory exists but does not match the pinned identity: $Destination. Remove the cache deliberately, then rerun this preparation."
}

$parent = Split-Path -Parent ([System.IO.Path]::GetFullPath($Destination))
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$staging = Join-Path $parent ('.onnx-staging-' + [Guid]::NewGuid().ToString('N').Substring(0, 12))
New-Item -ItemType Directory -Path $staging | Out-Null

function Download-Verified([string] $Url, [string] $OutFile, [string] $ExpectedSha256, [long] $ExpectedBytes) {
    $temporary = $OutFile + '.download'
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    Write-Output "Downloading $Url"
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source -L --retry 3 --retry-delay 2 -f -o $temporary $Url
        if ($LASTEXITCODE -ne 0) { throw "download failed with exit code $LASTEXITCODE`: $Url" }
    } else {
        Invoke-WebRequest -Uri $Url -OutFile $temporary -UseBasicParsing
    }
    Assert-File $temporary $ExpectedSha256 $ExpectedBytes | Out-Null
    Move-Item -LiteralPath $temporary -Destination $OutFile
}

try {
    Download-Verified $modelUrl (Join-Path $staging 'model.onnx') $modelSha256 $modelBytes
    Download-Verified $tokenizerUrl (Join-Path $staging 'tokenizer.json') $tokenizerSha256 $tokenizerBytes

    $manifest = [ordered]@{
        id = $modelId
        name = $modelName
        version = $revision
        dimension = $dimension
        languages = @('zh', 'en')
        tokenizer = 'tokenizer.json'
        hash = 'sha256:' + $modelSha256
        license = $license
        pooling = 'mean'
        normalize = $true
        maxTokens = 512
        queryPrefix = $promptQueryPrefix
        passagePrefix = $promptPassagePrefix
        source = $sourceUrl
        revision = $revision
        modelFile = $modelDownloadName
        modelSha256 = $modelSha256
        tokenizerFile = 'tokenizer.json'
        tokenizerSha256 = $tokenizerSha256
        files = @(
            [ordered]@{
                name = 'model.onnx'
                downloadName = $modelDownloadName
                url = $modelUrl
                sha256 = $modelSha256
                sizeBytes = $modelBytes
            },
            [ordered]@{
                name = 'tokenizer.json'
                downloadName = 'tokenizer.json'
                url = $tokenizerUrl
                sha256 = $tokenizerSha256
                sizeBytes = $tokenizerBytes
            }
        )
    }
    $manifestJson = $manifest | ConvertTo-Json -Depth 6
    [System.IO.File]::WriteAllText((Join-Path $staging 'manifest.json'), $manifestJson, (New-Object System.Text.UTF8Encoding($false)))

    Move-Item -LiteralPath $staging -Destination $Destination
} catch {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
    throw
}

if (-not (Test-PreparedModel $Destination)) {
    throw "ONNX model publication did not verify: $Destination"
}
Write-Output "ONNX model prepared: $Destination"
Write-Output "model=$modelId revision=$revision license=$license dimension=$dimension"
Write-Output "modelSha256=$modelSha256 tokenizerSha256=$tokenizerSha256"
