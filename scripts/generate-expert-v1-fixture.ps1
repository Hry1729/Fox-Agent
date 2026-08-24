$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot 'apps/desktop/src-tauri/Cargo.toml'

$env:FOX_GENERATE_EXPERT_V1_FIXTURE = '1'
try {
    cargo test --manifest-path $manifestPath generate_legacy_expert_v1_fixture --lib -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Fixture generation failed with exit code $LASTEXITCODE."
    }
}
finally {
    Remove-Item Env:FOX_GENERATE_EXPERT_V1_FIXTURE -ErrorAction SilentlyContinue
}
