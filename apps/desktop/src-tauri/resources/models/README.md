# Offline ONNX embedding model packages

Fox downloads the pinned ONNX Runtime binary through `ort-sys` while building
the desktop application. `ort-sys` verifies the archive hash before extracting
it, then links the runtime into the desktop executable. Model
files are downloaded only after the user starts an installation. A model
package is accepted only when all required metadata and hash checks pass.

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
└── tokenizer.json
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
  "id": "multilingual-e5-small",
  "name": "Multilingual E5 Small",
  "version": "1.0.0",
  "dimension": 384,
  "languages": ["中文", "英文"],
  "tokenizer": "tokenizer.json",
  "hash": "sha256:<64 lowercase hexadecimal characters>",
  "license": "MIT",
  "pooling": "mean",
  "normalize": true,
  "maxTokens": 512,
  "queryPrefix": "query: ",
  "passagePrefix": "passage: "
}
```

`hash` is the SHA-256 digest of the package's `model.onnx`. The adapter also
checks that `version`, `dimension`, `tokenizer`, and `license` are non-empty
and within their bounded sizes.

The controlled catalog currently exposes `bge-small-zh-v1.5` for Chinese
(512 dimensions) and `multilingual-e5-small` for Chinese/English
(384 dimensions). E5 packages must keep the `query: ` prefix for retrieval
queries and the `passage: ` prefix for indexed chunks. Those prefixes are part
of the embedding contract; changing or removing them requires rebuilding the
Zvec index generation.

## Runtime loading

The `local-embedding` feature uses the official `ort` crate with
`default-features = false` and explicitly enables `ndarray` and
`download-binaries`. The pinned runtime is downloaded, hash-verified, and
statically linked by `ort-sys`; model fixtures are never fetched by a build
script and users do not need to install a separate runtime DLL.

## Offline smoke

With a complete package:

```powershell
$env:FOX_ONNX_MODEL_DIR = (Resolve-Path 'apps/desktop/src-tauri/resources/models/<package-id>').Path
./scripts/onnx-offline-smoke.ps1
```

The smoke validates the manifest and model hash, loads `model.onnx`, runs one
tokenized input, and verifies the output dimension. Missing resources or a
failed inference are hard failures when the script is invoked. CI reports the
independent gate as not passed when no complete package is checked in; it never
claims a fabricated success.

## Storage boundary

ONNX model packages and the tokenizer generate embeddings only. SQLite keeps
knowledge-base, document, chunk, job, and generation metadata. Vector payloads
are written to one Zvec collection per generation under the local knowledge
`indexes/` directory. A generation is activated only after Zvec flush, count,
and dimension verification succeeds; retrieval reopens the persisted Zvec
collection after an app restart.
