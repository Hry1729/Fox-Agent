# S5 default entry switch — engineering record

- Start point: `71ffbb7ef0fc07ffaae8b3be8cefcfbdc3973739` (local `main`, no push)
- S5 branch: `s5-default-pinative-20261029`
- Production code commits: `81e6a2fd` (default PiNative + kernel fallback), `9540b68e` (damaged native session rejection + route docs)
- Candidate: `D:\python\projects\Fox\output\s5-pinative-20261029-2200`
- Candidate source: `9540b68e`; candidate manifest: `candidate-manifest.json`

## Route contract

New local Runs freeze **Authoritative + PiNative** when `FOX_EXECUTOR_ROUTE` is unset or `pi_native`.
`FOX_EXECUTOR_ROUTE=kernel` is the controlled fallback: new Runs freeze no route and run on Kernel Authoritative.
The value is read only at Run freeze. Existing bindings are returned unchanged; a switch change never reassigns an existing PiNative Run, a running Run, or a waiting approval.
`FOX_KERNEL_MODE` is no longer consulted for a new Run's authority or route. Old frozen authority/route remains authoritative for old Runs.
Invalid values fail closed before a binding is written. Child Runs inherit the parent binding.

## U01–U12

| Item | Status on this candidate | Evidence / boundary |
|---|---|---|
| U01 | PASS (engineering) | Final payload: new Run default `PiNative` while `FOX_KERNEL_MODE=legacy`; continuous chat, same native session across restart, reset generation 2; damaged-header fixture rejected with no fresh session. `evidence/app-evidence.json`, `evidence/corrupt-result.json` |
| U02 | PASS (engineering) | Final payload: A running, B completed, A cancelled; A waiting approval, B completed, approval denied on cancel. `evidence/app-evidence.json`; Rust Host concurrency tests |
| U03 | PASS | 34 `native_tool_batch` tests; real Host mixed barrier + six-reader peak 4 (`s5-final-tool-batch.log`, `s5-final-host-integration.log`) |
| U04 | PARTIAL / Unconfirmed | Shared-file conflict and read-only whitelist PASS; unknown MCP/command actual execution on this candidate not run. No permission widening observed. |
| U05 | PASS | 34 `native_tool_batch` tests, including duplicate/unknown/order/cancel/no-false-success. |
| U06 | PASS | Seven real Windows Job Object crash windows, `report.valid=true`: `migration-work/native-crash-suite-8966aa5a-7582-4e17-9308-38a8a68c1ce5/report.json` |
| U07 | PASS (engineering) | 8 approval tests; app pending approval/cancel; total-budget/retry/compaction remains covered by prior controlled-provider evidence, not this success scenario. |
| U08 | PASS | Final payload: real Provider full compaction (`18db53a1...`), then app exit/reopen and continuation (`04c65676...`) completed; same native session/generation. |
| U09 | PARTIAL | Native Job and child-collect tests PASS (`s5-final-u09.log`); application notification/delivery-card/待用户 desktop flow not verified on this candidate. |
| U10 | PARTIAL | Native memory and knowledge Host tests PASS (`s5-final-u10.log`); ordinary/project/expert desktop display not re-verified on this candidate. Contact M1/M2 = NA. |
| U11 | PASS | User-data copy v97 → v103, automatic backup/rollback v97, old core rows unchanged, both new binding routes selected: `migration-work/user-data-copy-20261028-2340-v2/u11-f3a9dc28-1381-48f9-be24-2f5c36f21a74/report.json` |
| U12 | PARTIAL | Package built, extracted and launched; desktop/sidecar hashes and SDK 1.4.0 recorded in `s5-candidate-manifest-20261029.json`. Package integrity PASS; whole U12 closure follows the U04/U09/U10 partials. |

## Performance

Offline paired Host fixture, 30 warm and 5 cold samples per cell, loads 1/2/3/4, contracts equal and source unchanged. Decision `within_initial_offline_threshold`, `exceededTenPercent=false`.

Native-over-Kernel ratios (lower is faster; load 1 means one task; load 4 means four submitted tasks with fixture queue):

| Load | TTFT p50 / p95 | Whole-turn p50 / p95 |
|---:|---:|---:|
| 1 | 0.176 / 0.183 | 0.183 / 0.191 |
| 2 | 0.201 / 0.277 | 0.209 / 0.281 |
| 3 | 0.409 / 0.340 | 0.412 / 0.344 |
| 4 | 0.465 / 0.465 | 0.469 / 0.470 |

Summary: `migration-work/s5-performance-summary-20261029.json`; raw report: `migration-work/native-performance-25ebea1a-7936-48a2-90a4-168eaf754a24/report.json`. Real Provider latency is recorded separately in the T3 evidence and is not used as a local performance decision.

## Release status

Code is committed locally; candidate package exists and was launched from the extracted payload. S5 is **not fully closed**: U04, U09, U10 and U12 remain partial, and no release observation period exists. Not merged, pushed, or published.

The engineering candidate is reviewable and the local performance gate is within threshold, but it is not a release declaration. Before a release decision, the remaining U04 actual MCP/command combination, U09 notification/delivery-card/待用户 desktop flow, U10 ordinary/project/expert desktop ownership display, and U12 same-build full closure must be completed; then a release observation period without fallback is required. S6 stays blocked.
