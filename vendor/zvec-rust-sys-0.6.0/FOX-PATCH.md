# Fox patch to zvec-rust-sys 0.6.0

Source: [`zvec-ai/zvec-rust`](https://github.com/zvec-ai/zvec-rust), published
as the crates.io package `zvec-rust-sys` version `0.6.0` (registry checksum
`f89deb26358085908592b89a97af9155e8f91b88d78006289589a394931b7664`).
The original Apache-2.0 license, package metadata, resolution order, download
behavior, and native build behavior are retained. This directory contains only
the published package's `Cargo.toml`, `build.rs`, `README.md`, `src/lib.rs`, and
the Apache-2.0 license text; no native DLL, import library, cache, or generated
file is vendored.

Fox's only build-script change is to emit `cargo:lib_dir=<canonical path>` for
the actual `resolve_lib_dir` result. Cargo exposes that as
`DEP_ZVEC_C_API_LIB_DIR` to Fox's direct native dependency. Fox uses this
metadata to place the same real DLL beside Windows test executables. If this
package is updated, recheck the build-script delta and library provenance.

Minimum validation for a change to this patch:

1. In a fresh isolated target, use a direct normal dependency on this crate
   and verify that the consumer build script receives
   `DEP_ZVEC_C_API_LIB_DIR` before it runs, pointing to the selected DLL and
   import library. A build-dependency alone does not supply that metadata.
2. Build Fox's Windows test binary with `zvec`, confirm the DLL copied beside
   the test executable has the same SHA-256 as the selected source, then run
   `--list` and a real Zvec health test with no Zvec directory added to PATH.
   Suppress Windows loader dialogs during this bounded test so failures exit.
3. Check that `--no-default-features` remains free of any Zvec native resource
   requirement. The Tauri installer bundle has a separate acceptance path.
