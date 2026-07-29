# Knowledge Service Original File Gateway Requirements

Updated: 2026-07-23

This document records the backend contract required by Fox for remote original-file preview and download. It is a requirements document only. The current Fox work does not modify the Yuxi backend.

## Scope

The knowledge service remains responsible for MinIO object lookup, user authorization, source version identity, and byte streaming. Fox never receives MinIO credentials or internal object paths.

The first backend delivery only needs the authenticated original-file gateway. Office conversion and derived preview jobs are a later delivery.

## Required Endpoint Contract

The existing endpoint may be extended:

```text
HEAD /api/workspace/knowledge/download?kb_id={kb_id}&file_id={file_id}&variant=original
GET  /api/workspace/knowledge/download?kb_id={kb_id}&file_id={file_id}&variant=original
```

Every request must authenticate the current user and verify access to both the knowledge base and the file.

### HEAD response

Required headers:

```text
Content-Length: {original object byte length}
Content-Type: {original media type}
Accept-Ranges: bytes
X-Source-Revision: {stable opaque revision}
X-File-Name: {UTF-8 original filename}
```

Recommended headers:

```text
ETag: {opaque object ETag}
Last-Modified: {HTTP date}
X-Version-Id: {MinIO version id when available}
Content-Disposition: attachment; filename*=UTF-8''...
```

`X-Source-Revision` is the only version identity consumed by Fox. Fox will not derive or combine ETag, size, or last-modified values.

Revision generation order:

1. Immutable MinIO `versionId`.
2. Persisted content SHA-256 generated during upload, synchronization, or first streaming read.
3. Temporary compatibility revision derived by Yuxi from opaque ETag, size, and last-modified until a SHA-256 is persisted.

Multipart ETags must remain opaque and must not be treated as MD5 hashes.

### Range response

Fox sends one byte range per request:

```text
Range: bytes={start}-{end}
If-Range: {X-Source-Revision value}
```

A valid response must be:

```text
HTTP/1.1 206 Partial Content
Content-Length: {end - start + 1}
Content-Range: bytes {start}-{end}/{complete length}
Accept-Ranges: bytes
```

The response body must contain exactly the requested bytes. The service must not silently return `200` with the complete file when a Range request is present.

If the supplied revision no longer matches the source object, the request must fail explicitly. It must never splice bytes from different source revisions. A stable `409` or `412` response with a machine-readable error code is preferred.

Invalid or unsatisfiable ranges should return `416 Range Not Satisfiable` and include `Content-Range: bytes */{complete length}`.

## Streaming Requirements

- Stream MinIO bytes to the HTTP response; do not read the complete object into application memory first.
- Apply bounded buffering and backpressure.
- Stop reading MinIO promptly when the client disconnects.
- Preserve the original byte length and media type.
- Do not serialize binary content through JSON or Base64.
- Support local and remote HTTPS deployments without requiring Fox and Yuxi to share a filesystem.

## Security Requirements

- Re-run authorization for every HEAD, full GET, and Range GET request.
- Reject a `file_id` that does not belong to the requested `kb_id`.
- Never expose bucket names, object keys, filesystem paths, MinIO credentials, or unrestricted presigned URLs.
- Sanitize response filenames and encode non-ASCII filenames with RFC 5987 `filename*`.
- Log user, knowledge base, file, operation, result, byte count, and latency, but never file content or credentials.
- Apply request-rate, response-size, and timeout limits appropriate to the deployment.

## Acceptance Tests

1. An authorized user can HEAD and download DOCX, XLSX, PDF, image, and Chinese-named files with the exact original size and bytes.
2. An unauthorized user cannot access the object by changing `kb_id` or `file_id`.
3. `bytes=0-1023`, a middle range, the final partial range, and a one-byte range all return exact `206` responses.
4. Invalid, reversed, overflow, and out-of-file ranges return stable errors and never return the full object.
5. A stale `If-Range` revision is rejected and no mixed-version body is returned.
6. Large files are streamed with bounded backend memory use.
7. Client cancellation stops the upstream MinIO read.
8. Local HTTP and remote HTTPS deployments behave identically at the contract level.
9. Missing objects and deleted knowledge bases return stable machine-readable errors.
10. Parallel ranges for the same file are safe and do not mutate the source object.

## Deferred Backend Work

The following items are intentionally outside this first contract:

- LibreOffice conversion workers.
- Server-side Office conversion or derived layout-PDF preview jobs.
- Preview-job deduplication and renderer-version cache keys.
- Citation page/anchor metadata enrichment.

The complete stage-D contract is documented separately in [YUXI_KNOWLEDGE_LAYOUT_PREVIEW_REQUIREMENTS.md](./YUXI_KNOWLEDGE_LAYOUT_PREVIEW_REQUIREMENTS.md).

Those items should start only after the original-file HEAD, Range, revision, permission, and streaming contract passes the acceptance tests above.
