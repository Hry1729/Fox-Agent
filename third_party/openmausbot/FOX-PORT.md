# OpenMausBot

## 小狐 P0 管家移植（2026-11-16）

固定来源提交：`1a4e4da9f380804ad98c7fd1b141bb0dbfafced4`。
已从该提交核对 Apache-2.0 LICENSE，以及下表源码和对应测试。
本地参考副本的 `server/drivers/agents-catalog.ts`、`server/index.ts` 与该提交不同，
本次以固定提交下载的版本为准。未使用 `enterprise/`。

| 上游来源 | Fox 落点 | 改编范围 |
| --- | --- | --- |
| `server/peer-roster.ts`、`server/system-prompt.ts`、`server/chief-of-staff.ts` | `services/agent-runtime/src/companion-team-prompt.mjs`、`prompt-composer.mjs` | 管家人设、按需新建、真实结果汇总、名单清洗和最多 40 项；中文名称上限沿用 Fox 的 40 字；不加入 P1 派活提示 |
| `server/drivers/agents-catalog.ts` | `services/agent-runtime/src/host-tools.mjs`、`companion-tools.mjs`；`crates/fox-engine-protocol/src/lib.rs` | 列表、新建、资料、模型、删除提议的工具含义；参数适配 Fox；默认伙伴专属；另加已在方案列明的归档提议 |
| `server/team-setup-requests.ts`、`server/profile-requests.ts`、`server/direct-apply.ts`、`server/default-bot.ts`、`server/new-bot-defaults.ts`、`server/decision-log.ts`、`server/index.ts` | `apps/desktop/src-tauri/src/database/repositories/companion_team.rs` 及伙伴命令 | 先应用再撤销、删除先确认、默认伙伴保护、每回合 4 个/总数 100 个/工作要求 4000 字、持久化操作记录；使用 Fox SQLite、工作区与删除计划校验 |
| `server/peer-roster.ts` | 同上 `companion_team.rs` | 活动伙伴过滤、名称解析和状态映射；Fox 无心跳信号，不伪造“没有响应” |
| `src/components/ApprovalCard.tsx` | `apps/desktop/src/features/companions/companion-team-cards.tsx` | `AppliedChangeLine` 的已生效与撤销语义，以及确认/结案状态；复用 Fox `ContactDecision`、`ContactChoice` 和删除清单组件，按回合挂载并持久恢复 |
| `server/peer-roster.test.ts`、`server/default-bot.test.ts`、`server/team-setup-requests.test.ts`、`server/profile-requests.test.ts` | Runtime 管家测试、Rust 管家仓储测试、前端操作卡测试 | 移植单人场景的保护、上限、过滤、冲突、确认及撤销用例；使用 Fox 的真实存储和组件测试入口 |

未移植部门、团队云、多所有者、技能分享、教程、语音和上游独立状态存储。
状态和权限逻辑在 Rust 执行，Runtime 不写数据库；新伙伴默认配置不授予额外工具或文件权限。
临时子 Agent 沿用现有机制，P1 伙伴协作不在本次移植范围内。

## 小狐 P1 伙伴协作移植（2026-11-16）

固定来源提交为 `1a4e4da9f380804ad98c7fd1b141bb0dbfafced4`，沿用 Apache-2.0 LICENSE 和 NOTICE。P1 采用单用户场景。临时子 Agent 使用原有接口。

| 上游来源 | Fox 落点 | 改编范围 |
| --- | --- | --- |
| `server/drivers/agents-catalog.ts` 的 `coordinate_bots`、`assignment_status` | `services/agent-runtime/src/host-tools.mjs`、`companion-tools.mjs`；`crates/fox-engine-protocol/src/lib.rs`；`apps/desktop/src-tauri/src/runtime_host/companion_coordination.rs` | 1 至 4 个目标、4,000 字说明、回复策略、咨询、返工和完成标准。Fox 用 `targets`、`instructions`，每对发起方与接收方使用固定协作对话。状态查询去掉面向外部运行时的长轮询。用户答复通过 Fox 的确认或答复记录投递 |
| `server/runtime/envelope.ts`、`server/runtime/questions.ts`、`server/peer-provenance.ts` | `services/agent-runtime/src/companion-coordination-prompt.mjs`、`prompt-composer.mjs`；`apps/desktop/src-tauri/src/database/repositories/companion_coordination.rs` | Runtime 保留任务信封、回传来源、文本清洗、JSON 引用和 `NEEDS INPUT` 提示。Rust 解析问题及默认选项并更新协作状态。Fox 以独立上下文片段注入信封、回传和提及；较长的回传保留可用于查询的派活 ID。接收方使用自己的权限，伙伴消息不产生授权 |
| `server/system-prompt.ts`、`server/peer-roster.ts`、`server/chief-of-staff.ts` | `services/agent-runtime/src/companion-team-prompt.mjs`、`companion-coordination-prompt.mjs` | 伙伴按需协作、工作页 @伙伴、如实汇报投递和回执、失败处理。普通伙伴可派活和查询，管理工具仍由小狐独占 |
| `server/runtime/{types,runtime,tree,questions}.ts`、`server/runtime-coordinator.ts`、`server/peer-delivery.ts`、`server/usage-attribution.ts`、`server/auto-approve.ts`、`server/host/delegated-level.ts` | `apps/desktop/src-tauri/src/database/repositories/companion_coordination.rs`、`runtime_host/companion_coordination.rs`、`database/migrations.rs` 的 `MIGRATION_118_COMPANION_COORDINATION` 及 Host 权限和用量入口 | 将派活生命周期、投递及唤醒、路径和循环拒绝、上限、权限收紧和根运行用量归属接入 Fox SQLite、运行队列和通知。咨询使用只读工具 |
| `server/drivers/agents-mausbot-tools.ts`、`server/routes/mausbot-notifications.ts`、`server/mausbot-notifications.ts`、`server/host/chief-notices.ts`、`server/drivers/agents-catalog.ts` 的 `retry_thread` | `services/agent-runtime/src/host-tools.mjs`；`apps/desktop/src-tauri/src/runtime_host/companion_chief.rs`、`database/repositories/companion_chief.rs` | 未读通知、完成事项标为已读、失败定时任务通知和每项最多两次重试。Fox 以通知 ID 解析具体任务及运行，重试沿用现有定时任务权限 |
| `shared/helper-followup.ts`、`server/helper-followup.ts` | `apps/desktop/src-tauri/src/database/repositories/companion_chief.rs` 的 `ensure_companion_helper_followup` | 按模型、其他伙伴和启用的定时任务持久化一次性跟进。Fox 保留单用户和本地设置场景 |
| `src/components/HandoffReply.tsx`、`src/components/ThreadChip.tsx` | `apps/desktop/src/features/companions/assignment-thread.tsx` | 复用头像、标题、进入线程的入口、完成标准和可展开回传。Fox 使用现有确认卡片和样式。已查阅的 `TeammateChip.tsx` 用于团队云人员转述，本次保留 Fox 自己的工作页 @ 菜单 |
| `server/runtime/{runtime,scenarios,envelope,questions}.test.ts`、`server/runtime-coordinator.test.ts`、`server/peer-provenance.test.ts`、`server/peer-delivery.test.ts`、`server/mausbot-notifications.test.ts` | Runtime 协作提示及工具测试、Rust 协作仓储与工具测试、桌面协作界面测试 | 按 Fox 合同改编成功、拒绝、投递、权限、咨询、回传、到期、上限和用户输入场景 |

部门、团队云、群聊、任意工作目录继承、外部运行时轮询和 `enterprise/` 未移植。群聊、历史搜索、条件等待和外部能力见 P2 方案。

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
