# 小 Fox 实施验收记录

更新时间：2026-11-12 15:20（北京时间，Asia/Shanghai）

## 交付状态

- 阶段1：本地提交 `1b8ed637`，独立身份/来源解绑/技能快照/显式项目笔记引用。
- 阶段2：本地提交 `8d7ca891`（`feat(companions): add profile updates, blank creation, and side-card settings`）。含 `companion_profile_update`（注册在 memory_write 之后）、空白创建流、侧卡职责与沟通偏好、保存成功后对账归一化返回值。Sol（gpt-6.1-sol，只读，已 unset ARK/QODER）最终结论：无新的可操作 P0–P2。
- 阶段3a：本地提交 `d59e63d2`（`feat(companions): add typed timezone-aware schedules (v115)`）。含 typed IANA 调度计算器、v115 迁移、既有 due-claim/补跑/本地零点入院集成、nullable 专家字段，以及 OpenMausBot `1a4e4da9` / Hermes `45a694b9` 归因。Sol 首审两个 P2（周计划 DST 缺口搜索窗口过短；兼容保存覆盖已存时区）已修并复审：无新的可操作 P0–P2。
- 阶段3b：本地提交 `43ca1263`（`feat(companions): add owned companion task tools and schedules`，20 files, +1108/-249）。八个 `companion_task_*` 工具紧随 `companion_profile_update`；Host 根运行归属；原子创建（专家字段 NULL、固定默认限额）；更新保留已存时区，且省略 `enabled` 时保留计划启用状态（Sol P2 已修并复审）。project-note 拒绝内部任务会话。offline-eval 134→142。OpenMausBot `1a4e4da9` / Hermes `45a694b9` FOX-PORT 已更新。Sol 首审 1 个 P2（计划编辑默默重新启用），修复后复审无新的可操作 P0–P2。
- 阶段3c：本地提交 `95f44258`（`feat(companions): add scheduled tasks to the contact side card`，9 files, +898/-5）。ContactSideCard 仍三个顶层 tab；详情入口含数量/待处理；嵌入子页列表/添加/编辑/启停/立即执行/删除/历史；「排队中」来自持久化 trigger status；无专家/预算表单；创建设备时区、已存时区不改写。两步删除确认。owned history 并集活动触发。名称-only 编辑保留整天/非整分钟间隔与原 onceAt。Sol 三轮 + Astra 建议后无未修阻塞 P2。child_run_* 仍不在白名单。
- 阶段4：未完成、未提交。唯一 Ark 489361 跑约 2h30m 后仍无编译/测试/结果文件，于 15:20:23 CST SIGTERM（exit 143）。仅有未完成的 ownership 草稿（已还原，WIP 见 `/workspace/xiaofox-phase4-wip.patch`）。`child_run_*` 仍不在白名单。A16–A18/A23 自动化与真实模型验收均未完成。
- 阶段5：未开始。
- 仅 box/Linux，未操作用户机器、未 Windows 测试、未 push、未 force/reset/跳 hooks。linux-schema.json 仍未跟踪、未提交。
- 前一次 Ark 环境转储事件已由用户轮换密钥；后续日志均凭证扫描干净。详见 security-note。

## 补丁

- `/workspace/fox-xiaofox-20261111.patch`：仅阶段1，已过时，勿当作全部功能。
- 已用 `git format-patch 792c0684..HEAD --stdout` 生成 `/workspace/fox-xiaofox-20261111.final.patch`（663456 字节，SHA256 `8450f15e3c847fc3dd7c5dbde7852ce26d29c87ba10290b0a9c15131b182fe25`），含已提交 5 个提交：`1b8ed637` + `8d7ca891` + `d59e63d2` + `43ca1263` + `95f44258`（相对 origin/main `792c0684`，超前 5）。**不含**未完成的阶段4。fetch 后 `4142498d` 仍不是有效对象，未 rebase，未 push。

## 实际测试与基线（阶段2提交时，协调器独立复核）

| 检查 | 基线 | 阶段2提交时 | 说明 |
|---|---|---|---|
| 桌面完整Bun |1274pass/8fail/2skip|1288pass/9fail/2skip|原8个基线失败不变；多1个 `联系人资料库 > 按今天/更早分组`（contact-files.test.tsx:169）。阶段2对该文件仅删除另一用例的 template_status mock；该用例单独连跑3次均通过，判定套件偶发，不是本阶段回归。阶段2侧卡对账 23/23。|
| 完整Agent Runtime（Node v24.22.1，`pnpm --dir services/agent-runtime test`）|541pass/0fail/1TODO|545pass/0fail/1同TODO|write-barrier TODO 为既有预期；Codex 自行的 44/10 不可信|
| TS |通过|通过|`apps/desktop/node_modules/.bin/tsc -p apps/desktop/tsconfig.json --noEmit`|
| protocol:check |通过|通过|只改 Rust 事实源并运行生成脚本|
| git diff --check |—|干净|含未跟踪的两份新 DOM 测试|
| Rust `--all-targets` |阶段2首轮修复后 check/先前 test 通过|本次独立 `cargo +stable test -p fox-desktop --no-default-features --all-targets --offline` 在提交时仍在跑；本 P2 未改 Rust。日志中曾见无关的 `agent_config::tests::model_verification_uses_http_and_records_only_success` FAILED（不在阶段2 diff，本地 HTTP fixture），待跑完记入进度|v114 迁移等已通过|

聚焦日志：`/workspace/xiaofox-phase2-fix2-indep-focused.log`、`-tsc.log`、`-protocol.log`、`-bun-full.log`、`-runtime.log`。Sol 安全日志：`/workspace/xiaofox-phase2-fix2-sol.safe.log`。

## 实际测试（阶段3a提交时，协调器独立复核，全部 cargo --offline）

| 检查 | 结果 | 说明 |
|---|---|---|
| `database::schedule` | 8 pass / 0 fail | 含 Sol 复现的每周 DST 缺口：Sunday 02:30 America/New_York，基线 2026-03-01T07:30:00Z → 2026-03-15T06:30:00Z |
| populated v114 迁移 | 1 pass | 保留任务/计划/触发链接与 FK |
| `database::repositories::digital_colleagues` | 23 pass / 0 fail | 含兼容保存保留已存时区与节奏；once 停用不删除；本地零点入院 |
| 完整 `database::migrations` | 67 pass / 0 fail / 1 ignored | 忽略项为既有 `FOX_REAL_DATABASE` 真实库测试 |
| `cargo +stable check -p fox-desktop --no-default-features --lib --offline` | 通过 | |
| app tsc | 通过 | 仅合同可空所需的 manager 最小修改，无新调度 UI |
| git diff --check | 干净 | linux-schema.json 未跟踪、未提交 |

日志：`/workspace/xiaofox-phase3a-indep-*.log`、`xiaofox-phase3a-fix-*.log`。Sol：`/workspace/xiaofox-phase3a-sol.safe.log`、`xiaofox-phase3a-sol2.safe.log`。复用说明见提交说明与 `third_party/{openmausbot,hermes-agent}/FOX-PORT.md`。

基线限制：未改动的 `ensure_builtin_agents` 启动时会重置可编辑 fox-general manifest。A04 重开 DB 测试为隔离来源变化而重新应用相同共享默认设置，不证明任意全局默认 manifest 跨重启保持。


## 实际测试（阶段3b提交时，协调器独立复核）

| 检查 | 结果 | 说明 |
|---|---|---|
| `digital_colleagues::tests` | 28 pass / 0 fail | 含原子归属创建、run-now 不移光标、once 完成后不可静默再启用、归档恢复不自动启用、**省略 enabled 的计划编辑保持停用**（Sol P2） |
| Host `companion_task_tools_enforce_root_owner_and_return_saved_values` | 1 pass | 根运行归属与回执含已存值 |
| `project_note_options_are_scoped_to_valid_grants_and_current_conversation` | 1 pass | 显式拒绝内部任务会话（不依赖空 projectId） |
| `companion_profile_update_enforces_root_run_and_persists_actual_values` | 1 pass | 既有 profile 回归 |
| `cargo +stable check -p fox-desktop --no-default-features --lib --offline` | 通过 | 仅既有 warnings |
| app tsc `--noEmit` | 通过 | |
| `pnpm protocol:check` | 通过 | 绑定由 `protocol:generate` 生成，未手改 |
| git diff --check | 干净 | linux-schema.json 未跟踪、未提交 |
| 完整 Runtime（Node v24.22.1，`pnpm --dir services/agent-runtime test`） | **545 pass / 0 fail / 1 既有 S2 TODO** | 与阶段2基线记账一致。首轮 543/2/1 为过期测试（普通会话目录未排除 `companion_task_*`；prompt 仍要求「已有计划工具」），协调者只改测试后复跑通过。协议 8 工具顺序与生成物、host-tools 一致（索引 69–76）；companion 白名单未加入 `child_run_*`。 |

日志：`/workspace/xiaofox-phase3b-indep-*.log`、`xiaofox-phase3b-indep-enablement.log`、`xiaofox-phase3b-indep-colleagues-after-p2.log`、`xiaofox-phase3b-result.md`。Sol：`/workspace/xiaofox-phase3b-sol.safe.log`、`xiaofox-phase3b-sol2.safe.log`。Ark 381282 于 11:45:41 CST 以 0 退出，会话与产物 SCAN_CLEAN。

## 实际测试（阶段3c提交时，协调器独立复核）

| 检查 | 结果 | 说明 |
|---|---|---|
| app tsc `--noEmit` | 通过 | |
| `bun test ./apps/desktop/tests/contact-side-card-dom.test.tsx`（仓库根，bunfig preload） | **33 pass / 0 fail / 206 asserts** | 含入口数量/待处理、嵌入返回、无专家预算、类型化创建、两步删除、整天间隔保留、queued 不被较新 skipped 遮蔽、保存失败保留草稿、90 秒间隔保留并按秒展示、任务子页刷新错误、onceAt 秒毫秒保留 |
| `bun test ./apps/desktop/tests/no-native-confirm.test.ts` | 7 pass / 0 fail | 协调者去掉 `window.confirm` 后恢复；完整 Bun 首轮曾因此新增 1 个失败（1292/9/2），修复后该用例通过。其余 8 个失败与阶段2基线一致（agent config、5 个 change-expert、repeated submits、reduced motion）。资料库日期分组偶发本次未现 |
| `cargo +stable check -p fox-desktop --no-default-features --lib --offline` | 通过 | |
| `owned_task_`（unset CARGO_TARGET_DIR，`--offline`） | 4 pass / 0 fail | 含 3b 启用态 P2 与新测试 `owned_task_history_keeps_active_triggers_outside_recent_limit`（20 条较新 skipped + 仍 queued 时 owned 历史 21 条） |
| `database::schedule::tests` | 8 pass / 0 fail | 3a 回归 |
| git diff --check | 干净 | linux-schema.json 未跟踪、未提交 |

日志：`/workspace/xiaofox-phase3c-indep-*.log`、`xiaofox-phase3c-result.md`、`xiaofox-phase3c-sol.safe.log`、`xiaofox-phase3c-sol2.safe.log`、`xiaofox-phase3c-astra.safe.log`、`xiaofox-phase3c-sol3.safe.log`。Ark 457560 于 12:35:28 CST 退出 0，SCAN_CLEAN。

## A01–A24

“通过”仅指已记录自动化，不等于真实桌面/真实模型整体验收。所有真实桌面流程均未执行，A05/A11/A16/A17/A23 明确**未完成真实模型验收**。

|编号|状态|证据/尚缺|
|---|---|---|
|A01|部分|blank 创建 repository/命令/加号 DOM 即时打开已测；未真实桌面|
|A02|通过（自动化）|同来源重复创建不同 ID、工作区/绑定测试|
|A03|通过（自动化）|blank 不继承默认个人数据；内置助手隔离 Host 测试|
|A04|部分|来源修改/技能/删除快照不变已测；共享默认启动 manifest 基线限制见上|
|A05|部分|真实 Host 落库及后续 context、保存对账归一化（职责尾空格/清空/刷新失败/切联系人）已测；未完成真实模型验收|
|A06|部分|偏好/共享记忆/临时要求提示边界与 pending 确认已测；语言行为未真实模型验|
|A07|部分|UI 保存/次轮 Host 读取已测；trim 后对账 P2 已修（23/23 DOM + Sol 无新 P0–P2）；未真实桌面|
|A08|部分|重名/非法字段/任意目标拒绝、错误保草稿已测；模型语言回执未验|
|A09|通过（自动化）|仅加号两操作、旧专家创建入口移除、侧卡三 tab/功能 DOM 回归|
|A10|通过（自动化）|项目显式选 companion ID、授权/撤权/归档/删除隔离及选择 UI 已测|
|A11|部分|3b 对话工具已自动化；3c 侧栏创建/编辑 UI 已测（类型化间隔、设备时区、无专家预算、不支持频率不出现）。未完成真实模型/真实桌面验收|
|A12|部分|无专家归属任务链已在 repository/Host 测：权威/工作区来自所有者，任务会话无专家绑定，task-local knowledge 可保留；未真实桌面|
|A13|部分|3b update/run-now 已测；3c 侧栏编辑/立即执行控件已测（名称-only 保留间隔与 onceAt；时区展示为保持不变）。未真实桌面|
|A14|部分|3b 仓储/工具已测；3c 侧栏暂停/删除（两步确认，首次不发命令）已测。未真实桌面|
|A15|部分|3a/3b 已测；3c 侧栏任务页已测（列表、下次执行、已存时区不改写、5s 轮询在子页激活时）。未真实桌面、未重启补跑整链桌面验|
|A16|未完成|子 Run 接通阶段4未做；未完成真实模型验收|
|A17|未完成|根 Run 父子最新累计预算未做，16次工具实测未做；未完成真实模型验收|
|A18|未完成|子 Run 停止/归档/删除传播阶段4未做|
|A19|部分|普通专家绑定选择/切换/移除 Rust 回归及完整 Runtime 通过；未桌面整体验|
|A20|通过（自动化）|v113→114 身份/聊天/笔记/授权/工作区/绑定快照 fixture 升级通过；v114→v115 保留任务/间隔计划/触发链接及 FK 已测。未动真实用户库|
|A21|未完成|数字同事独立创建/签名 Channel 链清理阶段5尚未做|
|A22|部分|3b 工具已有；3c 侧栏运行记录已测（「排队中」来自持久化 status；活动触发不被截断历史隐藏）。产物打开与定位来源的真实桌面整链未做|
|A23|未完成|仅配置根 Run/子写拒绝已测，父链归属/冻结能力/位置/停止尚未做；未开放委派白名单；未完成真实模型验收|
|A24|部分|3c UI 以持久化 `queued` 显示「排队中」已测；私聊占用后到期排队再执行的真实桌面并发未做|

## 模型与审查

- Ark/ark-code-latest：阶段1、2a、2b、阶段2首轮修复、fix2 对账实现、阶段3a、阶段3b（PID 381282，11:45:41 CST 退出 0）、阶段3c（PID 457560，12:35:28 CST 退出 0，约 37m）。未出现已知 429/5小时额度错误，未把开发切到其他模型。Ark 自行嵌套 Sol（env -i / inherit=none / ephemeral）失败，不以该次为审查。
- gpt-6.1-sol：逐轮只读审查。阶段1最终无新增 P0–P2；阶段2 首审/复审后剩余 trim-dirty P2，fix2 后再审明确无新的可操作 P0–P2。阶段3a 首审 2 个 P2（周计划 DST 缺口搜索、兼容保存覆盖时区），修复后复审无新的可操作 P0–P2。阶段3b 首审 1 个 P2（计划编辑省略 enabled 时按 task.status 默默重新启用），协调者改为保留 `existing.schedule.enabled` 并补回归测试后复审：Prior P2 is resolved，无新的可操作 P0–P2。阶段3c 首审 3 个 P2（整天间隔、skipped 遮蔽 queued、保存失败关编辑器）已修；Sol2 再提非整分钟间隔、历史截断、刷新失败关编辑器；Sol3 提 onceAt 截断秒毫秒，已按「未改字段保留原值」修。
- gpt-6-astra：阶段1来源删除分类建议；阶段2剩余 P2 只读建议（已按该建议实现）。阶段3c Sol2 后只读裁决：修秒间隔与历史并集；刷新失败关编辑器不阻塞，仅在任务子页展示错误。
- 协调者：依赖/基线/独立全量验证、本地提交、日志保密处置、进度与 t163u 开源复用调研；没有把模拟验收冒充真实模型。

## 下一步

阶段4 在 2.5h 未收敛已停止，WIP 未提交。阶段5未开始。继续阶段4时先看 `/workspace/xiaofox-phase4-wip.patch` 与 `/workspace/xiaofox-phase4.prompt`；A23 自动化通过前不得把 `child_run_*` 加入白名单。不以模拟回执冒充真实模型验收。额度限制下本轮不再启动 Codex。
