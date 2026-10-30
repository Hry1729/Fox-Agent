# Package 2-D closeout: isolated acceptance drivers

Test facility for the 2-D U1 delivery-acceptance closeout (R-1/R-2/R-3). None of
these files is part of the production bundle: they are plain Node scripts that
drive the real desktop over WebView2's remote-debugging endpoint, or run the
emitted component test.

## Isolation contract

Every run must use all four axes, and must never touch the user's own Fox:

| axis | value |
|---|---|
| data dir | `FOX_DATA_DIR` → SQLite, `webviews/`, `runtime-sessions/`, `attachments/`, `skills/`, `local-knowledge/` all under it |
| port | `1422` (a first-class profile port; the user's instance owns `1421`) |
| model | loopback-only fixture provider on `127.0.0.1:1433` |
| target | `CARGO_TARGET_DIR=E:\fox-targets\g4-p2d-closeout-target` |

`lib.rs::run()` reads `FOX_DATA_DIR` and redirects every durable path under it, so
the user's `%APPDATA%\com.fox.agent` is never opened. The `kernel-desktop-acceptance`
binary additionally refuses an existing database that lacks the
`.fox-kernel-acceptance` marker.

## Files

| file | what it does |
|---|---|
| `run-component-test.mjs` | runs the emitted `DeliveryAcceptanceCard.test.js` under Node with real happy-dom + real testing-library. Idempotent `.js` patch (see the result report's errata). |
| `cdp-drive.mjs` | the WebView2 CDP driver: `evaluate`, `evaluateAsync`, `clickByText`, `snapshot`. |
| `r3-sequence.mjs` | R-3: accept → Completed, reload, revoke. |
| `r3-reject.mjs` | R-3: the reject path → `failed`. |
| `r3-blocked.mjs` | R-3: a real on-disk candidate drift → the real `candidate_drift` blocked reason. |
| `r3-after-restart.mjs` | R-3: after a real application restart, both conversations are still findable. |
| `r2-model-service.mjs` | R-2: configure the isolated local model service through the real `model_service_save`. |
| `r2-continue.mjs` | R-2: drive the real `kernel_run_continue`. |
| `r2-verify.mjs` | R-2: parent/child IDs, single-active constraint, no acceptance inheritance, per-run checklist. |
| `r2-child-reason.mjs` | R-2: the child Run's real terminal reason. |
| `r2-parent-state.mjs` | R-2: prove the PARENT Run is still parked after its child ran, read through the real commands. |
| `r2-model-service.mjs` | R-2: configure the isolated local model service through the real `model_service_save`. |
| `r3-list.mjs`, `r3-verify-state.mjs`, `r3-ipc-list.mjs`, `r3-ipc-detail.mjs`, `r3-open-conversation.mjs` | read the real conversation list / detail / acceptance state, and open a conversation through the real sidebar. |

## Rust side

`src-tauri/src/bin/closeout-u1-fixture.rs` + `src-tauri/src/closeout_fixture.rs`
build the **injected starting point**: a real parked `awaiting_delivery_review`
Run in the isolated database, using only production APIs
(`Database::kernel_create_run`, `KernelCoordinator::start_prepared`,
`RunController::begin_initial_model_request` / `record_initial_model_response` /
`park_awaiting_delivery_review`, `Database::kernel_commit_decision` /
`kernel_commit_initial_model`). `src-tauri/src/bin/closeout-db-dump.rs` +
`closeout_fixture::dump_context_budget_reports` read the persisted
`kernel_context_budget_reports` row and each Run's `kernel_runs.state`, which is
how the real trigger of a compaction stop is established from data.

All three are gated behind the `kernel-acceptance` feature, so
`cargo build --locked --lib` does not contain them.

The fixture decides on the Host's behalf only the task text and the verdict
envelope each row carries; everything after it is the user's own decision, taken
through the real commands from the real UI.

## §5 note: the context window is test configuration, not a policy change

A continuation request's measured budget for this fixture is
system 8,808 + tools 12,041 + history 403 + append 1,366 + output reserve 2,048 +
protocol overhead 2,048 estimated tokens, plus the uncalibrated 50% safety margin
on the estimated components (11,309) — about 38k tokens against a 32k window.
That reports `within_budget: false`, and the Run stops with
`kernel.context_compaction_no_candidates` **before it ever reaches the model**,
even though the history is only 403 tokens.

The isolated model service is therefore saved with a 200k context window
(`FOX_CLOSEOUT_CONTEXT_WINDOW`), which leaves the same fixture at
`within_budget: true`, `pressure_ratio: 0.197`, `available_tokens: 160,579`.
No production compaction threshold, pass bound, recent-tail budget or safety
margin was touched — see `kernel_compaction.rs` constants, all unchanged.

## Running

```bat
REM 1. the isolated model provider (loopback only)
set FOX_ACCEPTANCE_PROVIDER_PORT=1433
node scripts\kernel-desktop-acceptance-provider.mjs

REM 2. the isolated vite server on 1422 (from a snapshot that has node_modules)
pnpm dev:profile

REM 3. the desktop, with the isolated data dir and CDP enabled
set FOX_DATA_DIR=E:\fox-targets\g4-p2d-closeout-data
REM A0: every new Run is PiNative; there is no route switch and no Kernel fallback.
set CARGO_TARGET_DIR=E:\fox-targets\g4-p2d-closeout-target
node <tauri-cli> dev --config "{\"build\":{\"beforeDevCommand\":\"\",\"devUrl\":\"http://127.0.0.1:1422\"},\"app\":{\"windows\":[{\"additionalBrowserArgs\":\"... --remote-debugging-port=9333\"}]}}" --no-dev-server --features kernel-acceptance

REM 4. the injected starting point (with the desktop stopped)
set FOX_ACCEPTANCE_PROJECT_ROOT=E:\fox-targets\g4-p2d-closeout-project
E:\fox-targets\g4-p2d-closeout-target\debug\closeout-u1-fixture.exe

REM 5. the drivers
node r3-sequence.mjs <conversationId>
node r3-reject.mjs <conversationId>
node r2-continue.mjs <sourceRunId> <conversationId>

REM §5: the context-budget / kernel-state reader (desktop stopped, read-only)
set FOX_DATA_DIR=E:\fox-targets\g4-p2d-closeout-data
E:\fox-targets\g4-p2d-closeout-target\debug\closeout-db-dump.exe
```
