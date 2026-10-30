# Zvec Windows resource

This directory is source-only. Do not commit vendor binaries.

The desktop Cargo manifest pins `zvec-rust-sys = "=0.6.0"` with the `bundled`
feature on Windows. `zvec-rust-sys` resolves a prebuilt library for the target
and exposes the exact selected directory through `DEP_ZVEC_C_API_LIB_DIR`.
`apps/desktop/src-tauri/build.rs` reads that directory, verifies the PE machine
type, stages the matching DLL beside Cargo test executables, and maps the
verified DLL to the Tauri bundle resource. It does not search `PATH` or pick a
stale `target/**` glob.

## Pinned offline resource

`scripts/prepare-zvec-resource.ps1` prepares the exact Windows x64/MSVC
resource from the official zvec-rust 0.6.0 release:

- archive:
  `https://github.com/zvec-ai/zvec-rust/releases/download/v0.6.0/zvec-prebuilt-x86_64-pc-windows-msvc.tar.gz`
- archive SHA-256:
  `c823448a39a0f69aacdfe1fe06aaccb0e26eb9e0a4eea65fbd96edf749093152`
- `zvec_c_api.dll` SHA-256:
  `2137ad6f24c00e8337efd54d8408df4690aa356773ab596beb495956dfeb7cd4`
- `zvec_c_api.lib` SHA-256:
  `c5fa14ea2530726be869f784a0521c1697a4216ef4c916282805fbed6fe2722d`
- license: Apache-2.0

The script verifies the archive before extraction, verifies both native files,
and publishes the directory atomically with a `resource.json` identity record.
It never silently substitutes a system DLL. A mismatched cached directory
fails explicitly.

```powershell
./scripts/prepare-zvec-resource.ps1 -Destination .test-target/zvec/0.6.0
./scripts/zvec-offline-smoke.ps1
```

`prepare-zvec-resource.ps1` is resource preparation only. The smoke itself
runs the real Rust test `vector_store::tests::zvec_vector_store_offline_smoke`:
create generation, write vectors, flush, vector search, count/dimension
verification, drop, reopen, search again. The smoke verifies that the DLL
staged beside the test executable and the import library used for linking have
the same pinned SHA-256 values. CI performs the verified preparation as a
required step; a missing resource fails the job rather than producing a green
skip.
