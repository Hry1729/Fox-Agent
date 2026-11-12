# Hermes Agent scheduling port

Confirmed source revision: `45a694b98cf95638f86da2d1ea22434b010e1e28`.
Copyright (c) 2025 Nous Research. Licensed under MIT; see `LICENSE`.

## Adapted

- `cron/jobs_time.py` `normalize_timezone` and `zone_for_job`: validate and
  canonicalize an IANA key before persistence, then use the plan's pinned zone
  rather than the host zone.
- `cron/jobs_time.py` `oneshot_instant`: interpret naive wall-clock input in
  the pinned zone. Fox rejects gaps and selects the earliest fold instant
  strictly after the supplied base.
- `cron/jobs.py` `compute_next_run`: preserve interval duration with UTC
  arithmetic, while calendar schedules use local wall-clock values in the
  pinned IANA zone. Fox considers both folds and never refires in the past.
- `cron/missed_runs.py`: only the bounded-misfire ideas. Fox retains its
  existing catchup window (default 3,600 seconds), records catchup/skip, does
  not replay all history, and reuses trigger idempotency on restart/recovery.

The Rust adaptation lives in `apps/desktop/src-tauri/src/database/schedule.rs`
and the existing digital-colleague due-claim/recovery repository path. No
Python runtime, scheduler daemon, or complete source file is vendored.

## Rejected / out of scope

Cron expressions and the croniter path, Hermes's 120-second one-shot grace,
`catch_up_missed` defaults, `cron/scheduler*.py` daemons, monthly/holiday or
adjusted-working-day approximation, webhooks, Cloud VM behavior, and any new
standalone scheduler service. Fox uses its existing schema constraints,
catchup policy, and dispatch/idempotency implementation.
