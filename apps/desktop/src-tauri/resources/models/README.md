# Offline ONNX embedding model packages

Fox uses the `ort` crate with `default-features = false` and explicitly enables
`ndarray` and `download-binaries`. `ort-sys` downloads and hash-verifies the
pinned ONNX Runtime archive, then links it **statically** into the desktop
executable. There is no production dependency on a separate `onnxruntime.dll`
and `ORT_DYLIB_PATH` must not be used by the smoke.

Model files are downloaded only after the user starts an installation. A model
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
an empty or fabricated resource to make the smoke gate appear green. Large
model files are not committed; CI caches the verified package by its pinned
identity.

## CI test package

The smoke uses the controlled catalog's `multilingual-e5-small` package, not an
invented local fixture:

- source: `https://huggingface.co/Xenova/multilingual-e5-small`
- revision: `761b726dd34fb83930e26aab4e9ac3899aa1fa78`
- model file: `onnx/model_int8.onnx` (installed as `model.onnx`)
- model SHA-256:
  `4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c`
- model bytes: `118054593`
- tokenizer: `tokenizer.json`
- tokenizer SHA-256:
  `0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39`
- tokenizer bytes: `17082730`
- dimension: `384`
- pooling: `mean`
- normalize: `true`
- query prefix: `query: `
- passage prefix: `passage: `
- license: MIT

These are the exact revision, sizes, and hashes already listed by
`apps/desktop/src-tauri/src/local_knowledge_vector.rs` for the controlled E5
catalog entry. `scripts/prepare-onnx-model.ps1` downloads and verifies the two
files, writes a BOM-free `manifest.json`, and publishes the package atomically.
The download/preparation step and the offline inference step are separate.
The inference step does not call Hugging Face or any model service.

```powershell
./scripts/prepare-onnx-model.ps1
./scripts/onnx-offline-smoke.ps1
```

## Manifest

`manifest.json` must contain these fields:

```json
{
  "id": "multilingual-e5-small",
  "name": "Multilingual E5 Small",
  "version": "761b726dd34fb83930e26aab4e9ac3899aa1fa78",
  "dimension": 384,
  "languages": ["zh", "en"],
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

With a prepared package:

```powershell
./scripts/prepare-onnx-model.ps1
./scripts/onnx-offline-smoke.ps1
```

The smoke validates the manifest and both file hashes, loads the real
`model.onnx` and `tokenizer.json`, runs one query-prefixed and one
passage-prefixed tokenized input through the real ONNX session, verifies the
384-dimension output, finite values, mean pooling, and unit norm, and checks
that `ort-sys` selected `cargo:rustc-link-lib=static=onnxruntime`. Missing
resources, a dynamic runtime link, a manifest/prefix mismatch, or a failed
inference are hard failures. CI no longer skips the job on a missing model;
the required preparation step fails explicitly.

## Storage boundary

ONNX model packages and the tokenizer generate embeddings only. SQLite keeps
knowledge-base, document, chunk, job, and generation metadata. Vector payloads
are written to one Zvec collection per generation under the local knowledge
`indexes/` directory. A generation is activated only after Zvec flush, count,
and dimension verification succeeds; retrieval reopens the persisted Zvec
collection after an app restart.
