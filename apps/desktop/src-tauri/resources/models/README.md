# Offline ONNX embedding model packages

Fox does not download model files or ONNX Runtime binaries at build time or
first run. A model package is accepted only when all required metadata and
hash checks pass.

Resource-center downloads are distributed as signed ZIP archives. Trusted
Ed25519 public keys live in `resources/models/trusted-release-keys/` and are
bundled with the desktop app. Private release keys must stay outside every Git
repository and must never be copied into this directory.

## Package layout

Put each complete package in its own directory, for example:

```text
resources/models/<package-id>/
├── manifest.json
├── model.onnx
├── tokenizer.json
└── onnxruntime.dll
```

The repository intentionally contains no model or DLL placeholder. Do not add
an empty or fabricated resource to make the smoke gate appear green.

Downloaded resource-center archives may use `model.int8.onnx`; the installer
must verify the archive SHA-256 and Ed25519 signature before extracting it into
the user's model directory. The legacy offline smoke package described below
continues to use `model.onnx` until the resource-center installer is wired in.

## Manifest

`manifest.json` must contain these fields:

```json
{
  "version": "1.0.0",
  "dimension": 384,
  "tokenizer": "tokenizer.json",
  "hash": "sha256:<64 lowercase hexadecimal characters>",
  "license": "Apache-2.0"
}
```

`hash` is the SHA-256 digest of the package's `model.onnx`. The adapter also
checks that `version`, `dimension`, `tokenizer`, and `license` are non-empty
and within their bounded sizes.

## Runtime loading

The `local-embedding` feature uses the official `ort` crate with
`default-features = false` and only `ndarray` and `load-dynamic`. It
does not enable `download-binaries`, `fetch-models`, or a build script that
downloads ONNX Runtime.

At runtime, set `ORT_DYLIB_PATH` to an absolute `onnxruntime.dll` path, or put
`onnxruntime.dll` beside `model.onnx` in the package. The DLL must be supplied
by the release/package process and must match the `ort` API gate used by this
adapter.

## Offline smoke

With a complete package:

```powershell
$env:FOX_ONNX_MODEL_DIR = (Resolve-Path 'apps/desktop/src-tauri/resources/models/<package-id>').Path
$env:ORT_DYLIB_PATH = (Resolve-Path "$env:FOX_ONNX_MODEL_DIR/onnxruntime.dll").Path
./scripts/onnx-offline-smoke.ps1
```

The smoke loads the DLL explicitly, validates the manifest and model hash,
loads `model.onnx`, runs one tokenized input, and verifies the output
dimension. Missing resources or a failed inference are hard failures when the
script is invoked. CI reports the independent gate as not passed when no
complete package is checked in; it never claims a fabricated success.
