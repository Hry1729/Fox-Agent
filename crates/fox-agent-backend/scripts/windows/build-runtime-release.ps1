<#
.SYNOPSIS
    Build the Fox Windows external-Agent runtime package (zip + .sha256 + .manifest.json)
    from the pins committed in runtime-package/. Windows only (npm writes the .cmd shims
    and runs the native install scripts of node-pty / koffi for the host it runs on).

.DESCRIPTION
    1. Download the pinned Node (runtime-package/node.json) and check its SHA256.
    2. For each backend in runtime-package/backends/<pi|codex|dsh>: copy package.json and
       package-lock.json to a work directory and run `npm ci` with that Node (so native
       modules are built for the Node that ships in the package).
    3. build-runtime-package.py stage (--prune-dts) and pack: no srt\ or acp-bridge\,
       pi-gate\{pi-gated.cmd, fox-approval-gate.ts, fox-mcp-tools.ts}, dsh-patches, the
       pinned model adapters, path budget.
    4. Check the packed layout the way the Host's pi preflight does (plan C1/C2/C3):
       installed entry -> pi-gate -> pi-acp adapter -> gate able to load the Fox tools.

    Never publishes anything. Output: <OutDir>\fox-sandbox-runtime-<Version>-windows-x64.zip
    and its .sha256 / .manifest.json. Pin the result in sandbox-release.json (version,
    sha256), launch.rs RUNTIME_PACKAGE_VERSION and build-runtime-package.py DEFAULT_VERSION.

.EXAMPLE
    pwsh -File crates\fox-agent-backend\scripts\windows\build-runtime-release.ps1 -Version 0.1.7 -OutDir D:\releases
#>
param(
    [Parameter(Mandatory = $true)][string] $Version,
    [Parameter(Mandatory = $true)][string] $OutDir,
    [string] $WorkDir = (Join-Path ([System.IO.Path]::GetTempPath()) ('fox-rt-' + [guid]::NewGuid().ToString('N').Substring(0, 8))),
    [string] $Python = 'python'
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "invalid version '$Version'" }
$here = $PSScriptRoot
$pins = Join-Path $here 'runtime-package'
$builder = Join-Path $here 'build-runtime-package.py'
New-Item -ItemType Directory -Path $WorkDir, $OutDir -Force | Out-Null
$WorkDir = (Resolve-Path -LiteralPath $WorkDir).Path
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
Write-Host "work: $WorkDir"

function Invoke-Checked([string] $What, [scriptblock] $Block) {
    Write-Host "== $What"
    & $Block
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}

# 1. Node.
$node = Get-Content -LiteralPath (Join-Path $pins 'node.json') -Raw | ConvertFrom-Json
$nodeZip = Join-Path $WorkDir 'node.zip'
Write-Host "== Node $($node.version)"
Invoke-WebRequest -Uri $node.url -OutFile $nodeZip -UseBasicParsing
$actual = (Get-FileHash -LiteralPath $nodeZip -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $node.sha256) { throw "Node zip SHA256 mismatch: expected $($node.sha256), got $actual" }
Expand-Archive -LiteralPath $nodeZip -DestinationPath $WorkDir -Force
$nodeDir = Join-Path $WorkDir $node.directory
$npm = Join-Path $nodeDir 'npm.cmd'
if (-not (Test-Path -LiteralPath (Join-Path $nodeDir 'node.exe'))) { throw "node.exe missing in $nodeDir" }
$env:PATH = "$nodeDir;$env:PATH"

# 2. Backends from the committed lockfiles.
$backendArgs = @()
foreach ($backend in @('pi', 'codex', 'dsh')) {
    $prefix = Join-Path $WorkDir "b\$backend"
    New-Item -ItemType Directory -Path $prefix -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $pins "backends\$backend\package.json") -Destination $prefix
    Copy-Item -LiteralPath (Join-Path $pins "backends\$backend\package-lock.json") -Destination $prefix
    Push-Location $prefix
    try { Invoke-Checked "npm ci ($backend)" { & $npm ci --no-audit --no-fund --loglevel=error } } finally { Pop-Location }
    $backendArgs += @('--backend', "$backend=$prefix")
}

# 3. Stage and pack.
$stage = Join-Path $WorkDir 'stage'
$zip = Join-Path $OutDir "fox-sandbox-runtime-$Version-windows-x64.zip"
Invoke-Checked 'stage' { & $Python $builder stage $stage --node-dir $nodeDir @backendArgs --prune-dts }
Invoke-Checked 'pack' { & $Python $builder pack $stage --version $Version --out $zip }

# 4. Layout check on the packed zip, in the Host's preflight order.
$check = Join-Path $WorkDir 'check'
Expand-Archive -LiteralPath $zip -DestinationPath $check -Force
$problems = @()
foreach ($entry in @('backends\pi\node_modules\.bin\pi-acp.cmd', 'backends\codex\node_modules\.bin\codex-acp.cmd', 'backends\dsh\node_modules\.bin\dsh.cmd', 'backends\pi\node_modules\.bin\pi.cmd', 'node\node.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $check $entry))) { $problems += "not installed: $entry (agent_backend_not_installed)" }
}
if (-not (Test-Path -LiteralPath (Join-Path $check 'pi-gate\pi-gated.cmd'))) { $problems += 'missing pi-gate\pi-gated.cmd (agent_backend_pi_gate_missing)' }
$adapterPins = (Get-Content -LiteralPath (Join-Path $here '..\..\runtime-assets\model-adapters\pins.json') -Raw | ConvertFrom-Json).files
foreach ($key in @('backends/pi/node_modules/pi-acp/dist/index.js', 'backends/pi/node_modules/pi-acp/dist/fox-turn-outcome.mjs')) {
    $file = Join-Path $check ($key -replace '/', '\')
    if (-not (Test-Path -LiteralPath $file)) { $problems += "missing $key (agent_backend_runtime_adapter_missing)"; continue }
    $text = [System.IO.File]::ReadAllText($file) -replace "`r`n", "`n"
    $hash = [System.BitConverter]::ToString([System.Security.Cryptography.SHA256]::Create().ComputeHash([System.Text.Encoding]::UTF8.GetBytes($text))).Replace('-', '').ToLowerInvariant()
    if ($hash -ne $adapterPins.$key) { $problems += "$key differs from the pinned adapter (agent_backend_runtime_adapter_missing)" }
}
$gateText = Get-Content -LiteralPath (Join-Path $check 'pi-gate\pi-gated.cmd') -Raw -ErrorAction SilentlyContinue
if (-not ($gateText -and $gateText.Contains('FOX_PI_MCP_EXTENSION'))) { $problems += 'pi-gated.cmd cannot load the Fox tools (agent_backend_fox_tools_unsupported)' }
if (-not (Test-Path -LiteralPath (Join-Path $check 'pi-gate\fox-mcp-tools.ts'))) { $problems += 'missing pi-gate\fox-mcp-tools.ts' }
foreach ($retired in @('srt', 'acp-bridge')) {
    if (Test-Path -LiteralPath (Join-Path $check $retired)) { $problems += "retired tree in package: $retired\" }
}
if ($problems.Count -gt 0) { $problems | ForEach-Object { Write-Host "layout: $_" }; throw "runtime package layout check failed ($($problems.Count))" }
Invoke-Checked 'node --version' { & (Join-Path $check 'node\node.exe') --version }

$sha = (Get-Content -LiteralPath "$zip.sha256" -Raw).Split(' ')[0].Trim()
Write-Host "package: $zip"
Write-Host "sha256:  $sha"
Write-Host "pin: sandbox-release.json version=$Version sha256=$sha; launch.rs RUNTIME_PACKAGE_VERSION; build-runtime-package.py DEFAULT_VERSION"
