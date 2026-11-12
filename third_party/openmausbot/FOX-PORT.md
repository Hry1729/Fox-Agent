# OpenMausBot

来源为 [milind-soni/OpenMausBot](https://github.com/milind-soni/OpenMausBot/tree/58378e85449ca49f3370d72e6c70cece4fee413f)，许可及版权见本目录 LICENSE 和 NOTICE。

Fox 的专题整理参考 server/memory-organize.ts 和 server/memory-upkeep.ts 的分工。模型指定待移条目及专题，Host 负责移动原文。Fox 按 SQLite 条目标识操作，对用户手写的条目先请求确认，并对专题文件检查内容哈希。

## Phase 3a scheduling port

Confirmed source revision: `1a4e4da9f380804ad98c7fd1b141bb0dbfafced4`
(Apache-2.0; Copyright 2026 Milind Soni and OpenMausBot contributors).

Fox adapts the control flow of `server/routine-schedules.ts`
`nextOccurrence`, the schedule/weekday validation ideas in `parseSchedule` and
`cleanDays`, and the anchored cadence of `server/routine-intervals.ts`
`nextAlignedInterval` into Rust in
`apps/desktop/src-tauri/src/database/schedule.rs`. Fox uses its required
60..=2,592,000 second interval range, persisted IANA zones, and strict
strictly-after once semantics. No TypeScript runtime or complete source file
is vendored.

Fox deliberately does not port the host-local `Date.setHours` calculation,
cron expressions, `limit_reset`, webhooks, Cloud VM behavior, scheduler
daemons, or anything under `enterprise/` (which has a separate Enterprise
License). Calendar calculation uses `chrono-tz`; DST gaps are skipped and fold
candidates are evaluated explicitly.

## Phase 3b conversational task controls

Fox adapts the conversational create/update/pause/resume/run-now/delete tool
shape and the rule that mutation results include the next due time. Host resolves
the current root companion, the typed Rust scheduler persists the plan, pause
only disables future work, and run-now never advances the plan cursor.

Rejected: host-local `Date` arithmetic, cron, webhooks, Cloud VM behavior,
independent daemons, arbitrary companion targeting, and `enterprise/`.

## Phase 3c contact scheduled-task surface

Confirmed source revision: `1a4e4da9f380804ad98c7fd1b141bb0dbfafced4`.
Fox adapts the task-list status, next-due display, and pause/run-now/delete
control ideas into the existing ContactSideCard embedded view. Host commands
reuse the owned task repository and trigger acceptance/dispatch services; the
UI reads persisted trigger status rather than maintaining a queue.

Rejected: host-local `Date` arithmetic, cron expressions and cron editors,
expert/template selectors, technical budget forms, independent schedulers or
panels, Cloud VM behavior, and `enterprise/`. No complete source file is
vendored.
