# Zvec Windows resource

This directory is intentionally source-only. Do not commit vendor binaries.

For a local Windows x64/MSVC Zvec build, place the official files from the same
`zvec-rust 0.6.x` distribution here:

- `zvec_c_api.dll`
- `zvec_c_api.lib`

Alternatively set `ZVEC_LIB_DIR` to a directory containing both files and run
`scripts/zvec-offline-smoke.ps1`. The project disables the `bundled` feature and
sets `ZVEC_AUTO_BUILD=0`; missing resources must fail explicitly rather than
trigger a download or source build.

The Tauri bundle does not include a wildcard resource entry until an official
binary set is supplied. This prevents a clean checkout from referring to a
non-existent or fabricated DLL.
