Implement Phase 4 only (child-run ownership for Xiao Fox) in /workspace/fox-xiaofox-20261111, branch feat/xiaofox-independent-agent, HEAD 95f44258. Source of truth: /workspace/fox-xiaofox-spec-20261111.md §7 (especially 7.1–7.2), §6.1 / §6.2 budget notes around A17, §9, §10 work package「子 Agent」, A16–A18 and A23.

Do not implement phase 5 (数字同事 cleanup). Do not start other agents. Do not commit, push, force-push, reset --hard, or skip hooks. Do not touch other worktrees. Do not hand-edit generated protocol files; if a protocol change is truly required, edit crates/fox-engine-protocol/src/lib.rs and run pnpm protocol:generate (COREPACK_HOME=/home/box/.cache/node/corepack). Do not print secrets or dump the environment. Do not commit apps/desktop/src-tauri/gen/schemas/linux-schema.json. apply_patch is not available in this shell; use Python or git apply to edit files.

Write a concise implementer report to /workspace/fox-xiaofox-20261111/.xiaofox-phase4-result.md (the coordinator will move it). Include: files changed, tests run and results, whether child_run_* was added to the companion whitelist and why, what was reused from Hermes/OpenMausBot (SHAs OpenMausBot 1a4e4da9 Apache-2.0 except enterprise/; Hermes 45a694b9 MIT) with FOX-PORT updates if you adapt anything, and any leftover risks. Clones are at /workspace/oss-refs/OpenMausBot and /workspace/oss-refs/hermes-agent. Prefer adapting ideas with attribution over vendoring files. License-compatible only.

## The gap (do not regress ordinary chats)

companion_workspace.rs:allows_tool does not include child_run_start / child_run_collect / child_run_cancel, so Xiao Fox private chat and background tasks cannot delegate. That whitelist addition is LAST, only after A23 automated tests pass. Never open the tools first and backfill ownership.

child_runs.rs creates the child conversation WITHOUT conversations.companion_id (conversation_kind='child', parent_conversation_id set). resolve_conversation_on in companion_ownership.rs only accepts a literal conversations.companion_id or a companion_bound digital_colleagues row. It does not walk parent_conversation_id / parent_run_id. A Xiao Fox child therefore resolves as Ok(None) and takes the unrestricted ordinary-conversation path, bypassing workspace write limits, capability grants, and note ownership.

A companion allows only one occupant of its execution slot. If a child simply inherits the parent's slot, the parent waiting on child_run_collect holds the slot and the child can never start (deadlock). Children must not occupy the companion slot or the unique private-chat binding.

## Required behavior

1. Ownership walks the parent chain. A child session resolves to the Xiao Fox of the root Run and is marked as a child run (extend CompanionScope; do not break queue_slot() for private chat vs background). If this conversation is a child (conversation_kind='child' or a child_runs row) and it belongs under a Xiao Fox whose chain is broken, return an error and STOP — never Ok(None) fallback to the ordinary path. If the walk completes and the root is an ordinary conversation (no companion), keep Ok(None) so A19 ordinary-chat delegation does not regress.

2. The child inherits the Xiao Fox workspace write restriction and the parent Run's frozen capability set. It must not grow mid-run from newly granted tools and must not exceed the parent's tool set. Every tool execution still goes through the existing Host authorization check; a revocation is immediately effective (same as the parent, runtime_host/mod.rs around the existing grant check).

3. Children are work inside the parent task: they do not take the companion execution slot and do not take the unique private-chat binding. When the parent ends or is stopped, its children must be stopped first; no leftover child work may continue in the same workspace after the next top-level task is admitted. drain_companion_runs / companion_active_run_ids must count those children (archive/delete stop paths). Verify stop-propagation for ordinary-chat stop, Xiao Fox private-chat stop, and background-task stop. Partial results and errors return to the parent; the parent must not treat a failed child as success.

4. Keep existing executor depth, duration, and budget constraints. Do not add a new concurrency quota (child_runs.rs currently has none).

5. Children must not be offered companion_profile_update or note-write tools (note_organize / memory_write as applicable — follow §4.3: open only when the run belongs to a Xiao Fox AND is not a child). Note-read permission must not exceed the parent. Runtime tool filtering and Host execution use the same predicate; a direct Host request from a child is also rejected.

6. A17 budget: child token and tool-call usage counts toward the owning task's daily and per-run limits at the ROOT run, so delegation cannot bypass the cap. Current code counts only the current Run / the trigger row itself. Sum the latest cumulative usage of each parent and child run; do not add repeated usage.updated events, and do not double-count already-rolled-up children. When the combined total would exceed the limit, stop and record the reason. Default 16 tool calls / 128000 tokens / 4096 output / 300000 ms stay as Host defaults; add a focused test that records whether 16 calls is tight for parent+child (report the measurement in the result file — do not silently raise the default).

## Tests (real repository / Host; injectable clocks only, no copied scheduler)

Automated tests must cover A23 (and the A16–A18 Host/repo slices that do not need a live model):

- Child cannot write outside the owning companion workspace.
- Child cannot use a tool the parent does not have.
- Child tool catalog has no config-update tool; a direct Host request is rejected.
- Mid-run revocation of a capability rejects the next call.
- Parent waiting on a child does not deadlock (child does not take the slot; child can start).
- Parent finish/stop leaves no residual child; the next top-level task can enter the workspace.
- Broken ownership on a companion child stops execution; no ordinary-path fallback.
- drain_companion_runs / archive / delete include children.
- Root-run budget: latest-cumulative-per-run sum, no double count of usage.updated, combined overflow stops with a reason.
- Ordinary-chat child delegation still works (A19, no regression).

Use cargo +stable, unset CARGO_TARGET_DIR (do not inherit a prefetch target), prefer --offline, CARGO_BUILD_JOBS=2, CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0. PATH must include /home/box/.cargo/bin. Filter cargo tests with a substring (not a regex, and do not put `|` after `--`). Run targeted tests and `cargo +stable check -p fox-desktop --no-default-features --lib --offline`. If you touch TS, `apps/desktop/node_modules/.bin/tsc -p apps/desktop/tsconfig.json --noEmit`. If you touch protocol, pnpm protocol:check. Desktop bun tests must run from the repo root.

## Whitelist last

Only after the A23 automated tests above pass, add child_run_start, child_run_collect, and child_run_cancel to companion_workspace::allows_tool (and any matching Runtime companion catalog filter). If a test is still failing, leave the whitelist unchanged and say so in the result file.

Do not fake real-desktop or real-model acceptance. Linux-schema stays untracked. When done, write the result file and exit 0 if the implementation and automated tests succeeded, or non-zero with the result file explaining what failed.
