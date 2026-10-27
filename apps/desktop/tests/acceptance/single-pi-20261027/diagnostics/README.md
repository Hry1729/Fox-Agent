# SenseNova 404 on the legacy (SDK) path — request-level diagnosis

Work unit 3 · single-Pi acceptance (2026-10-27) · read-only diagnosis, no product code changed.

## 1. Result in one paragraph

The legacy/SDK path and the authoritative Kernel path **send the same provider
request**: `POST <baseUrl>/chat/completions`, `model: sensenova-6.8-flash-lite`,
`api: openai-completions`, `stream: true`, `max_tokens: 65535`,
`stream_options: {include_usage: true}`, `Authorization: Bearer …`, same
`x-stainless-*`/`user-agent` headers. There is no double-appended `/v1`, no
missing `/chat/completions`, no `/v4`, no second protocol and no different model
id. The single credential *name* is the same on both paths
(`com.fox.agent.model` + the provider base URL). Resuming a Pi-native session
does **not** re-derive the provider or base URL from the session header: a
resumed capture, and the older `fox-snapshot` legacy session, are identical to a
fresh one. So the recorded
`404 {"message":"model route not found","type":"not_found_error","code":"5"}`
**cannot be explained by anything the SDK path puts on the wire** — and its body
is not a body this provider produces for any route shape that can be probed
without a key (the provider's own 404s are
`{"error":{"code":5,"message":"NOT_FOUND","details":[]}}`, and its correct chat
route answers `401 {"error":{"code":16,…}}`). Two candidates survive; both are
outside the request-construction code, and §7 says exactly what a human must run
to pick between them.

**This means: do not "fix" the base URL/path handling — at this checkout it is
provably correct. The actionable defects are diagnostic gaps (§8).**

## 2. What was actually captured

Four runs against one local recording stub, all configured with the *same*
`modelService` (`baseUrl: http://127.0.0.1:<port>/v1`, `modelId:
sensenova-6.8-flash-lite`, `apiType: openai-completions`, context 256000, max
output 65535), same fake credential:

| field | `sdk-fresh-session` | `sdk-resumed-session` | `sdk-legacy-snapshot` | `kernel-round` |
| --- | --- | --- | --- | --- |
| method | `POST` | `POST` | `POST` | `POST` |
| path | `/v1/chat/completions` | `/v1/chat/completions` | `/v1/chat/completions` | `/v1/chat/completions` |
| query | *(none)* | *(none)* | *(none)* | *(none)* |
| model | `sensenova-6.8-flash-lite` | same | same | same |
| stream | `true` | `true` | `true` | `true` |
| max token field | `max_tokens: 65535` | same | same | same |
| `stream_options` | `{"include_usage":true}` | same | same | same |
| `store` | absent | absent | absent | absent |
| authorization | `Bearer <redacted>` | same | same | same |
| run outcome on the stub | `run.completed` | `run.completed` | `run.completed` | `kernel.model_response` |

`sdk-fresh-session` and `sdk-resumed-session` are two *different processes*:
the first creates a Pi-native session, the second reopens that exact file
(`resume_session`). `sdk-legacy-snapshot` is the older `fox-snapshot` session
format (`setFoxAgentHistory`), i.e. the pre-pi-native SDK path.

Body-level fields that are **not** routing facts and therefore differ:

| field | SDK path | Kernel path |
| --- | --- | --- |
| body size | ~68 KB each for the three SDK runs (they carry the Fox tool catalog and system prompt) | 348 bytes in this probe (it froze an empty proposal-tool set) |
| `tools` | 50 tool schemas (Fox catalog) | none in this probe (a live Kernel round carries the Host-frozen proposal tools) |
| messages | `system,user` (fresh / snapshot) / `system,user,assistant,user` (resumed) | `system,user` |
| `x-stainless-timeout` | `90` (`RUNTIME_HTTP_IDLE_TIMEOUT_MS`) | `300` (Pi default; the Kernel worker does not set the idle-timeout setting) |

Headers captured for every request and compared field by field (redacted):
`accept`, `accept-encoding`, `accept-language`, `authorization`,
`connection`, `content-type`, `host`, `sec-fetch-mode`, `user-agent`,
`x-stainless-arch`, `-lang`, `-os`, `-package-version`, `-retry-count`,
`-runtime`, `-runtime-version`. Identical apart from `content-length` and the
two fields above; the provider never sees a header that differs between paths.

Evidence files:

* [`evidence/captured-requests.json`](evidence/captured-requests.json) — redacted,
  bounded (routing facts + headers + body keys, not the prompt text);
* [`evidence/comparison.md`](evidence/comparison.md) — the table above, generated;
* [`evidence/compare-run.txt`](evidence/compare-run.txt) — console output of the
  parity run, including the final PASS line;
* [`evidence/real-endpoint-run.txt`](evidence/real-endpoint-run.txt) — console
  output of the fake-credential real-endpoint probe;
* [`evidence/vendor-routes-run.txt`](evidence/vendor-routes-run.txt) — console
  output of the route-table probe;
* [`evidence/run-facts.txt`](evidence/run-facts.txt) — the two production runs,
  read from a DB copy with `inspect-run-facts.py`;
* [`evidence/replay-recorded-session.txt`](evidence/replay-recorded-session.txt) —
  replaying the failing run's own session file.

## 3. The production runs, field by field

`python inspect-run-facts.py <fox.db> <runId…>` (read-only; DB copy at
`%TEMP%\fox-acceptance-single-pi\data\fox.db`, copied with its `-wal`) produced
[`evidence/run-facts.txt`](evidence/run-facts.txt). Summary:

| fact | Kernel run `03d4938d…` (completed) | SDK run `5ba50bda…` (failed) |
| --- | --- | --- |
| `run_control_bindings.authority` | `authoritative` | `legacy` |
| frozen `kernel_model_configs.modelService.baseUrl` | `https://token.sensenova.cn/v1` | *(no frozen config — sidecar path)* |
| sidecar `run.request_snapshot.baseUrl` / `provider` | *(no snapshot — Kernel path)* | `https://token.sensenova.cn/v1` / `fox-openai-compatible` |
| `usage.request.config.raw` | apiType/baseUrl-less `{apiType, modelId, contextWindow, maxOutputTokens, supportsImageInput}` | same |
| `usage.request.config.resolved` | `api openai-completions`, model, maxOutputTokens 65535, reasoning false | same |
| `usage.request.config.sent` (parsed from the real request body) | `{"model":"sensenova-6.8-flash-lite","stream":true,"max_tokens":65535}` | `{"model":"sensenova-6.8-flash-lite","stream":true,"max_tokens":65535}` |
| provider recorded on the usage record | `fox-openai-compatible` | `fox-openai-compatible` |
| outcome | `success`, 12593 input / 6144 cache-read / 75 output tokens | `failure`, no usage |

Note that `config.sent` is not predicted: `model-usage-runtime.mjs` parses it
from `init.body` inside the wrapped `fetch`
(`services/agent-runtime/src/model-usage-runtime.mjs:75-83`). Both runs really
serialized the same routing fields.

## 4. Why the recorded 404 body points away from this endpoint

`probe-vendor-routes.mjs` (no credential; fixed `route probe` text) collected the
provider's own answers:

| probe | status | body |
| --- | --- | --- |
| `POST /v1/chat/completions` no credential | 401 | `{"error": {"code": 16,"message": "Authorization Not Found"}}` |
| `POST /v1/chat/completions` fake credential | 401 | `{"error": {"code": 16,"message": "Forbidden"}}` |
| `GET /v1/models` fake credential | 401 | `{"error": {"code": 16,"message": "Forbidden"}}` |
| `POST /v1/responses` | 401 | `{"error": {"code": 16,"message": "Authorization Not Found"}}` |
| `POST /chat/completions` (no `/v1`) | 404 | `{"error": {"code": 5,"message": "NOT_FOUND","details": []}}` |
| `POST /v1/v1/chat/completions` (double `/v1`) | 404 | `{"error": {"code": 5,"message": "NOT_FOUND","details": []}}` |
| `POST /v4/chat/completions` | 404 | `{"error": {"code": 5,"message": "NOT_FOUND","details": []}}` |
| `POST /v1/chat/completions/` (trailing slash) | 401 | `{"error": {"code": 16,"message": "Authorization Not Found"}}` |
| `api.sensenova.cn` `POST /v1/chat/completions` | 404 | `{"message":"no Route matched with those values"}` |
| `api.sensenova.cn` `POST /v1/llm/chat-completions` | 401 | `{"error": {"code": 16,"message": "Authorization Not Found"}}` |

**No probed route produces `{"message":"model route not found","type":
"not_found_error","code":"5"}`.** That dialect (flat, `code` as a string,
`type: not_found_error`) is a gateway/router dialect, not this endpoint's. The
same host *does* get reached: `probe-real-endpoint.mjs`, with a fake key, shows
both paths reaching it and receiving `401` (see §5), which is exactly what a
correct route looks like and exactly not what a wrong route looks like.

There are two ways to read that:

* **F (vendor/route)** — authentication is checked before routing, so an
  *authenticated* request for a model that the key's plan has no route for can
  still be answered by a router layer with a different 404 body. Under this
  reading the SDK request was correct and the vendor refused the model route at
  that moment (the Kernel run, 26 minutes earlier, had a route).
Latency is **not** discriminating: the failing run reports `modelRequestMs
99.9`, and from this machine the same endpoint answers a cold TLS request in
607 ms, then 78 ms / 49 ms on warm name resolution and connection reuse. A 100 ms
rejection is compatible with either reading.

* **G (intermediary)** — something between the app and the provider (LAN proxy,
  security appliance, vendor edge canary) answered with its own JSON. No proxy
  configuration exists in the app, its environment, or Windows; nothing in Fox
  emits this body (`model route not found` appears nowhere in the checkout).

Both are outside the request-construction code. §7 gives the two commands that
separate them.

## 5. Real-endpoint check with no credential

`probe-real-endpoint.mjs` points *both* paths at the real configured base URL
with an obviously fake key:

```
sdk     provider_rejected   401: {"code":16,"message":"Forbidden"}
kernel  model_round_settled POST /v1/chat/completions → upstream 401 {"error": {"code": 16,"message": "Forbidden"}}
PASS  both paths dial POST <baseUrl>/chat/completions at the real provider; it answers 401 auth, never 404 route
```

The Kernel side is dialled through a local pass-through recorder so its exact
method/path/body is kept. So against the real host, both paths are on the
correct route; the only thing a fake key proves is the route, not a successful
completion.

## 6. Where each path gets its configuration (and its credential name)

| | Kernel / authoritative | SDK / legacy sidecar |
| --- | --- | --- |
| config source | DB singleton `model_service` → frozen into `kernel_model_configs` (`apps/desktop/src-tauri/src/runtime_host/mod.rs:2559-2562` (fresh run) / `:2211-2219` (continuation), frozen by `kernel_model_worker.rs:157-190`) | DB singleton `model_service` → `initialize` payload (`apps/desktop/src-tauri/src/runtime_host/mod.rs:3911-3931`) |
| model object | `services/agent-runtime/src/pi-kernel-worker.mjs:131` (`baseUrl: config.baseUrl`) | `services/agent-runtime/src/pi-runtime.mjs:246-260` (`baseUrl: config.baseUrl`) |
| provider registration | `services/agent-runtime/src/pi-adapter.mjs:73-80` | same function, same call |
| provider id | `transportProvider(profile)` → `fox-openai-compatible` (`model-profile.mjs:269-275`) | identical |
| URL adapter | Pi `openai-completions` → OpenAI SDK `baseURL: model.baseUrl` + `/chat/completions` (`services/agent-runtime/node_modules/@earendil-works/pi-ai/dist/api/openai-completions.js:559-566`) | identical |
| credential name | `apps/desktop/src-tauri/src/runtime_host/kernel_host.rs:2219-2228` → `model_service::get_api_key(base_url)` | `apps/desktop/src-tauri/src/runtime_host/mod.rs:3908` → same function |
| keyring service | `com.fox.agent.model`, entry name = provider base URL (`apps/desktop/src-tauri/src/model_service.rs:9,335-337`) | identical |

`model_providers` / `provider_models` are **not** read on either path for a
`fox` conversation; they are the UI/credential catalog and the default-provider
source that is mirrored into the `model_service` singleton. No `/v1`-fixup or
suffix logic exists in either path: `normalize_model_base_url`
(`apps/desktop/src-tauri/src/model_service.rs:204-233`) only trims trailing
slashes, and both paths then append `/chat/completions` exactly once through the
adapter.

Code-pinning: `pi-runtime.mjs` in the working tree is byte-identical
(`sha256 260be614…`, 48 106 bytes) to the copy installed in
`apps/desktop/src-tauri/target/debug/agent-runtime/pi-runtime.mjs` at
2026-10-27T14:49:49Z — i.e. to the file that the debug app executed for the
15:11Z failing run (debug builds prefer
`services/agent-runtime/src/pi-runtime.mjs`). The pre-failure bundle
(`services/agent-runtime/dist/fox-agent-runtime-…exe`, built 13:37Z) contains the
same `createModel`/`transportProvider` source, so the request construction is
unchanged across the failing run and today.

## 7. Two one-line checks a human must run (only these can settle F vs G)

1. **Does the provider answer that exact body for an authenticated request?**
   PowerShell (reads the key from the environment, never prints it):

   ```powershell
   $k=[Environment]::GetEnvironmentVariable('SENSENOVA_API_KEY'); $r=Invoke-WebRequest -Method Post -Uri 'https://token.sensenova.cn/v1/chat/completions' -Headers @{Authorization="Bearer $k"} -ContentType 'application/json' -Body '{"model":"sensenova-6.8-flash-lite","messages":[{"role":"user","content":"ping"}],"stream":false,"max_tokens":16}' -SkipHttpErrorCheck -TimeoutSec 60; "$($r.StatusCode) $($r.Content)"
   ```

   Run it on the same machine/network as the failing app, with
   `SENSENOVA_API_KEY` set from the keyring once (e.g. by copying it in the UI);
   the command prints only the status and the provider's answer.

   * `200` (or an SSE stream) → the route and key are fine right now; F was
     transient, re-run the legacy turn.
   * the recorded `404 {"message":"model route not found",…}` → the vendor is
     refusing this model route for this key: an account/model-route problem, not
     a Fox request problem.
   * a different 404/401 → compare byte-for-byte with what `compare-requests.mjs`
     captured; that would reopen a Fox-side hypothesis (G).

2. **Which URL does the app really dial for the failing conversation?**
   `node compare-requests.mjs` already shows the only shape this code can produce
   (and `probe-real-endpoint.mjs` confirms it against the live host with a fake
   key). If a future run still fails, set that provider's base URL to a local
   recorder — `lib-recording-server.mjs` is the exact stub these scripts use — and
   read the recorded `path`; a path other than `/v1/chat/completions` would be the
   first Fox-side fact in this case.

## 8. Minimal fix recommendation

No change to `baseUrl`, URL joining or the api shape: those are verified
correct, and changing them would break the working path. The defect this run
exposed is that **the failure cannot be attributed from the recorded facts** —
the URL that was dialled is deliberately dropped. Minimal, ordered changes:

1. **Record the request target (redacted) in the usage record.**
   `services/agent-runtime/src/model-usage-runtime.mjs:75-83` — the wrapped
   `fetch(url, init)` already holds the URL; add e.g.
   `config.target = { origin, path }` (bounded strings, no query, no credentials)
   next to `sent`. This single field would have turned this investigation into
   one glance, and it is the only durable evidence gap.
2. **Stop dropping non-retryable HTTP statuses on the Kernel path.**
   `services/agent-runtime/src/pi-kernel-model-failure.mjs:28-38` sets
   `rejection` only for `[429,500,502,503,504,529]`; any other non-2xx (a 404,
   a 401) becomes `null`, and the round is reported as
   `model_transport_failure` with `httpStatus: null`. Reproduced live in §5: the
   Kernel path hides the 401 that the SDK path prints. Carry the status for
   every response (retry admission is a separate decision and stays unchanged),
   so a 404 is as visible as an `empty_final_answer`.
3. **Surface the provider's own identity on a legacy provider failure.**
   `services/agent-runtime/src/pi-event-mapper.mjs:414-420` produces
   `provider.request_failed` from `event.message.errorMessage` only. Attach the
   provider `x-request-id` (when present) and the status so a vendor ticket has
   something to quote.

If check 1 in §7 returns the recorded body, the product-level follow-up is not a
URL fix but classification: a provider 404 for a configured model should be
reported as a non-retryable **configuration/entitlement** failure (with the
provider base URL shown in Settings), not as a transient
`provider.request_failed`. That is a Host-side policy change, still not a URL
change.

## 9. Reproduce

```powershell
cd apps/desktop/tests/acceptance/single-pi-20261027/diagnostics

# 1. Request parity (local stub only; no credential, no network)
node compare-requests.mjs
#    PASS = method/path/query/model/stream/max-token/stream-options/store/
#           authorization-scheme identical across fresh SDK, resumed SDK,
#           legacy snapshot SDK and Kernel

# 2. Real endpoint, fake credential (leaves the machine; "route probe" text only)
node probe-real-endpoint.mjs
#    PASS = both paths POST /v1/chat/completions and get 401 (auth), never 404

# 3. The provider's route table, no credential (leaves the machine)
node probe-vendor-routes.mjs
#    prints MISMATCH while the recorded 404 body cannot be produced by this host

# 4. Replay the failing run's own Pi session file against the stub
node replay-recorded-session.mjs
#    PASS = that session resumes and still dials /v1/chat/completions with the
#           configured model (a copy is made; the file is never modified)

# 5. Recorded production facts from a DB copy (read-only, no credential)
python inspect-run-facts.py "$env:TEMP\fox-acceptance-single-pi\data\fox.db" `
  03d4938d-986e-4b37-a4ea-45f526f09b80 5ba50bda-ff24-46e5-94c2-bd244820f17d
```

Prerequisites: Node ≥ 20 with the workspace dependencies installed for
`services/agent-runtime` (scripts import the runtime's own
`createEnvelope`/`controlBinding`), Python 3 with `sqlite3` (stdlib only).
Nothing builds, installs or needs a key; no script writes outside
`diagnostics/evidence` and the OS temp directory.

## 10. Verified vs not verified

Verified:

* the routing-relevant request of both paths is identical (captured, compared
  programmatically, and re-checked against the live host with a fake key);
* the configuration each path consumes, the frozen Kernel copy included, and the
  credential *name* (base URL) of each path;
* that a Pi-native resume does not change the base URL: `createAgentSession`
  uses the `model` the Fox adapter passes and only consults the session's
  `model_change` entry when no model was supplied
  (`pi-coding-agent/dist/core/sdk.js:91-101`), and `Fox` always supplies it
  (`services/agent-runtime/src/pi-runtime.mjs:809`, `session-manager.js:146-160`
  only records `{provider, modelId}` — no base URL is persisted or re-derived);
* re-running the *exact* production session file
  (`…2026-10-27T15-08-45-763Z_01a17de8….jsonl`) through the sidecar against the
  stub resumes it and sends `/v1/chat/completions` with the configured model
  (`replay-recorded-session.mjs`);
* that the recorded 404 body is not the provider's 404 dialect for any probed
  route.

Not verified (needs the real key, which this work unit must not touch):

* what an **authenticated** request to the configured chat route returns for
  this key/model today — i.e. whether candidate F is real;
* whether an intermediary on the operator's network produced the body (candidate
  G);
* therefore the final root cause. The narrowed set is F vs G in §4, and §7 is
  the discriminator.
