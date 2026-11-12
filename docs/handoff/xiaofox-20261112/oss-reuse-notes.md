# OpenMausBot / Hermes 复用核对（阶段3–5准备，2026-11-12 北京时间）

用户新要求（t163u）：后续实现尽可能直接复用 OpenMausBot 与 Hermes Agent 开源代码，先查本仓库vendored副本，再核许可，能用则改编并署名；找不到已确认仓库须如实说明。规格§12原把外部项目当行为参考而非技术依据；本要求在不违反Fox既有调度/归属/白名单门禁的前提下，优先改编开源实现并署名。

## 已确认仓库与许可（与Fox Apache-2.0兼容）

### OpenMausBot — 已确认
- 官方仓库：https://github.com/milind-soni/OpenMausBot（规格§12链接指向同一组织/仓库，Fox已有记录）。
- 许可：Apache License 2.0（Copyright 2026 Milind Soni and OpenMausBot contributors）。`enterprise/` 目录为单独的 OpenMausBot Enterprise License，**禁止复用该目录**。OSS部分与Fox兼容。
- 本仓库已有 `third_party/openmausbot/{LICENSE,NOTICE,FOX-PORT.md}`，来源提交在 FOX-PORT 记为 `58378e85449ca49f3370d72e6c70cece4fee413f`。**未vendored实际源码**，仅许可与移植说明。既有移植范围是笔记整理（memory-organize / memory-upkeep），不是调度或委派。
- 与阶段3相关、可改编的OSS路径（公开README/文档与DeepWiki核对，实施前须再拉当时HEAD的OSS文件并核对许可证未变）：
  - `server/routine-schedules.ts`：once / interval / 日历（选星期+本地时刻）解析与下次时间；interval有5–1440分钟限制（Fox规格是60–2,592,000秒，须按Fox范围改编，不能照搬5分钟下限）。
  - `server/routines.ts`：到期领取、目标忙碌时排队、补跑窗口（文档称错过≤12h只补最近一次，更早标为错过；Fox沿用已有补跑窗口，不能改成另一套默认而不加测试）。
  - 对话工具 `trigger` / `trigger_info` / `delete_trigger`：创建/更新/暂停/恢复/立即执行；规格要求协议源+Host共用保存服务、归属当前小Fox、拒绝不支持的月/节假日规则。Cron表达式若源码支持，规格本期不支持，不得引入近似保存。
  - 不得引入其webhook、Cloud VM runner、enterprise功能。
- 商标：OpenMausBot/MausBot名称与吉祥物属商标，Apache不授商标权；Fox产品名不变，只署名代码来源。

### Hermes Agent — 已确认
- 官方仓库：https://github.com/NousResearch/hermes-agent
- 许可：MIT（Copyright (c) 2025 Nous Research），与Fox兼容，复用须保留版权与许可声明。
- 本仓库**没有**vendored Hermes源码或NOTICE；文档里仅有行为参考链接（规格§12、`docs/归档/研究与方案/Agent能力实施方案.md`）。
- 与阶段3/4相关、可改编的路径（实施前拉OSS源码核对）：
  - 调度：`cronjob_manage` 工具与 cron 实现（一次性/周期、暂停/恢复/立即/删除、结果回到来源聊天）；Bot Mode例程挂在固定Bot聊天旁。规格要本地时区持久化、三类计划、不新起调度服务——应改编算法与工具形状，接入Fox既有digital_colleagues调度，不替换本机运行服务。
  - 委派：`delegate_task` / delegation-patterns：子Agent独立上下文、只向父返回摘要、默认深度1、父停止影响子任务、非持久进程内委派。Fox规格有更严的：沿父链归属、失败关闭不回落普通对话、不占小Fox执行位、能力上限继承且撤权即时、父结束先停子、根Run预算汇总；**A23自动化通过前不得把child_run_*加入白名单**。可改编隔离/停止/结果回报思路，不能照搬其默认可选深度或绕过Fox Host授权。

### 不复用或暂不复用
- OpenMausBot `enterprise/`：许可不兼容。
- OpenMausBot / Hermes 的完整产品入口、独立调度守护进程、webhook、群聊跨bot、云VM：规格明确本期不增加。
- Rakazo（已有 `third_party/rakazo`，Apache-2.0）：已用于companion记忆提示，与本阶段调度/委派关系不大。
- 阶段2剩余P2是Fox侧栏草稿与后端trim对齐，与这两项目无关，继续按Astra建议小范围修，不引入外部UI框架。

## 阶段3–5实施约定
1. 每个子任务开始时，在worktree外只读抓取上述OSS文件（或浅克隆到 `/workspace/oss-refs/`，不写入Fox工作树除非决定vendored），核对LICENSE仍为Apache-2.0 / MIT，记录提交SHA。
2. 改编进Fox时：保留版权与许可到 `third_party/openmausbot` 或新增 `third_party/hermes-agent` NOTICE；代码注释与FOX-PORT说明改编了哪些函数、哪些因规格差异未采用（如cron表达式、5分钟下限、enterprise、webhook）。
3. 不得削弱Fox已有测试/白名单门禁；新工具必须协议顺序与offline-eval完整覆盖（阶段2 P1教训）。
4. 进度日志记录：复用了何处、为何有的不能用。


## 阶段3开始时已浅克隆（2026-11-12 08:14 北京时间）

均在 `/workspace/oss-refs/`，**未写入 Fox 工作树**。许可证与调研时一致。

| 仓库 | 路径 | HEAD SHA | 许可 |
|---|---|---|---|
| OpenMausBot | `/workspace/oss-refs/OpenMausBot` | `1a4e4da9f380804ad98c7fd1b141bb0dbfafced4`（2026-11-12，chore(release) 0.1.154） | Apache-2.0，NOTICE Copyright 2026 Milind Soni and OpenMausBot contributors。`enterprise/` 仍为单独 Enterprise License，禁止复用。 |
| Hermes Agent | `/workspace/oss-refs/hermes-agent` | `45a694b98cf95638f86da2d1ea22434b010e1e28`（2026-11-11） | MIT，Copyright (c) 2025 Nous Research |

先前调研提到的 `server/routine-schedules.ts` 在当前 HEAD **仍存在**（225行）。另有 `shared/routine-schedule.ts`（cron，本期不用）、`server/routine-intervals.ts`、`server/routine-due.ts`。

### 阶段3a 可改编（算法移植到 Rust，不是把 TS/Python 拷进仓库）

OpenMausBot（Apache-2.0）：
- `nextOccurrence` / `parseSchedule` / `cleanDays`（`server/routine-schedules.ts`）：once（at>after 才返回）、interval 锚定对齐、daily+weekdays。
- `nextAlignedInterval`（`server/routine-intervals.ts`）：`anchorAt + (elapsed+1)*intervalMs`，间隔计划保持锚定节奏，不要改成 now+interval。
- 类型拆分 once / daily+weekdays / interval 的思路。

Hermes（MIT）：
- `normalize_timezone` / `zone_for_job` / `oneshot_instant`（`cron/jobs_time.py`）：保存前校验 IANA 并持久化规范名；朴素墙钟按计划时区解释，不按宿主机本地时区。
- `compute_next_run`（`cron/jobs.py` 约 1191）：interval 在 UTC 上加时长（跨 DST 保持真实间隔）；日历墙钟必须落在配置的 IANA 时区。DST 回拨（fold 0/1）取严格晚于基线的最早瞬间——Fox 日历计划应吸收此策略并写清、测试 America/New_York。
- `fast_forward_missed_recurring` / `retire_expired_oneshot`（`cron/missed_runs.py`）：只吸收「过补跑窗口记 skip/catchup、不重放全部历史、重启不重复业务触发」的思路；窗口秒数用 Fox 已有默认 3600，不要改成 Hermes 的 120s 一次性宽限或 `catch_up_missed` 默认。

### 明确不照搬及原因

- OpenMausBot **每日计划用 `new Date(after); d.setHours(...)`**，走**宿主本地时区**，不是持久化 IANA，无法正确测 DST / Asia/Shanghai 本地零点。结构可参考，实现必须用 chrono-tz（规格要求），不得把这套 Date API 译成 Rust。
- OpenMausBot 间隔限制 **5–1440 分钟**；Fox 是 **60–2,592,000 秒**。按 Fox 范围校验。
- OpenMausBot / Hermes 的 **cron 表达式**、limit_reset、webhook、Cloud VM、独立调度守护进程：规格本期不支持，不得引入或近似保存。
- Hermes 完整 `cron/scheduler*.py` 守护进程：不替换 Fox 既有 digital_colleagues 领取/补跑管道，不新起调度服务。
- `enterprise/`：许可不兼容。
- 商标：产品名仍为小 Fox，只在 NOTICE / FOX-PORT 署名代码来源。
