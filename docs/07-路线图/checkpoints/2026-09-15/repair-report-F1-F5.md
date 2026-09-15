# Fox F1–F5 修复报告（第二轮复审后，2026-09-15 接续）

> 依据：第二次复审的 F1–F5（`output/claude-design-re-review-20260915/report.md`）。
> 本轮**未提交、未推送**；未改长期记忆或技能；未覆盖原始 Excel、旧成果与审查证据。
> 承接 N1–N7 报告：`output/claude-design-repair-20260915/report.md`（同一输出目录，本轮日志追加在 `logs/`）。
> 原始源工作簿本轮复核：`apps/desktop/tests/execl-ceshi/AGV长时间任务汇总统计表.xlsx`，258,185 B，
> SHA-256 `b01b6b7ce5dbc8344f2e0866ee9a533973824679601f1d3ebb60abc1ac34b09a`（与此前留存一致）。

---

## 0. 结论与证据级别

| 项 | 结论 | 修复位置 | 本轮证据 |
|---|---|---|---|
| **F1** | 已修复 | `kernel_coordinator/live.rs`、`kernel_coordinator.rs`、`repositories/kernel.rs`、`repositories/run_steering.rs` | 3 个新测试（Live 确定性屏障 → 真实外层循环 → `applied` + completed；非 Live 同窗口；重启 rehydrate 证据） |
| **F5** | 已修复 | `kernel_coordinator/steering.rs` + 两处调用点 | 2 个测试断言**完整消息序列**（回复恰一次、提示恰一次、工具调用/结果配对与顺序不变） |
| **F2** | 已修复（共用执行缝已接通，评测与生产同路） | `runtime_host/managed_files.rs`（新 `execute_with_managed_versions`）、`kernel_host.rs`、`real_eval_tests.rs` | 1 个新测试：同一函数里"被执行的受管写入被登记、可恢复；非受管工具不登记" |
| **F4** | 已修复 | `runtime_host/delivery.rs`（`ooxml_sheet_charts`） | 用**真实 xlsx 关系链**测试：两表各一图通过 / 两图同表失败 / 孤立图表不计数 / 关系损坏=未核验 |
| **F3** | 已实现生产侧源数据核验（口径有界，见局限） | `runtime_host/delivery.rs`（`SourceDistribution`）、`database/repositories/delivery_checks.rs`、`runtime_host/mod.rs`（Host 侧绑定） | 1 个新测试：源重算通过 / 两产物同错皆失败 / 右分子错占比失败 / 源记录变化后旧数失败 / 口径不明=未核验 |

证据级别：**本轮实际编译并运行了全部 Rust 测试、前端测试、Node 合同测试与 tsc/协议检查**（日志见 `logs/`，含命令与退出码）。
真实云模型、GUI、真实 OfficeCLI 端到端仍为未验收（§7）。

### 本轮全量证据

| 范围 | 命令 | 结果（日志） |
|---|---|---|
| Rust 全量 | `cargo test --lib -- --test-threads=2` | **814 passed / 0 failed / 8 ignored**（`logs/rust-lib-full.log`） |
| F4 交付要求 | `cargo test --lib runtime_host::delivery` | **18 / 0**（`logs/rust-delivery.log`） |
| F2 受管执行缝 | `cargo test --lib runtime_host::managed_files::tests` | **15 / 0**（`logs/rust-managed-files.log`） |
| F1/F5 补充要求 | `cargo test --lib kernel_coordinator::tests::steering` | **15 / 0**（`logs/rust-steering.log`） |
| F4 生产 gate（真实协调器） | `cargo test --lib delivery_live_tests` | **4 / 0**（`logs/rust-delivery-live.log`） |
| 迁移（v67 重建） | `cargo test --lib database::migrations` | **28 / 0**（`logs/rust-migrations.log`） |
| R8 契约层 | `cargo test --lib real_eval` | 10 / 0 / 2 ignored（`logs/rust-real-eval-contract.log`） |
| 技能发现 | `cargo test --lib skill_load_tests` / `skills::` | 3 / 0、10 / 0（`logs/rust-skill-load.log`、`logs/rust-skills.log`） |
| 前端 | `bun test ./apps/desktop/tests` | **243 pass / 0 fail**（`logs/bun-desktop-test.log`） |
| 前端类型 | `npx tsc --noEmit` | 日志内含命令与 `exit=0`（`logs/tsc.log`） |
| 协议绑定 | `node scripts/generate-engine-protocol.mjs --check` | `exit=0`（`logs/protocol-check.log`） |
| Node 合同 | `node --test test/{host-tools,pi-kernel-host-tools,offline-evals}.test.mjs` | **30 pass / 0 fail**（`logs/node-host-tools.log`） |

---

## 1. F1（P1）：终态竞争不再让 Run 失败，旧派发被正确结算

**根因（与复审一致）**：终态事务里"发现未投递补充要求"时整笔回滚，而**模型结果的 lease 完成也在同一事务内**，于是模型请求仍处 `leased`；返回重新规划信号后，外层循环读到持久化快照中的 leased InitialModel/ContinuationModel/DeliverToolBatch，按 `kernel.uncertain_execution` 处理 → Run 失败。信号本身没有创建可执行的 steering 派发，也没有结算已收到的模型回复。

**修复**（三层，均为"在已收到且已结算的边界上重新规划"，未放宽 uncertain-execution 保护，也未重放已完成工具）：

1. **决策窗口闭合（消除竞争，而不是失败）** —— `SteeringDecision` 新增 `adopt_all_received`（`database/repositories/run_steering.rs`）：凡本次决策会**武装指令**（Live 的 batch/continuation 指令）时置真，事务内用新函数 `adopt_received_steering_in_tx` 把"提交这一刻仍是 `received`"的行全部绑定到本次派发；指令内容与下一轮历史改为提交后按 `applied_dispatch_key` **回读**（`Database::bound_run_steering`），因此"指令携带什么"与"事务绑定了什么"不可能不一致。绝不用于 Final 决策（否则等于静默丢弃用户输入）。
2. **终态竞争的等价恢复** —— 只有"Final 决策遇上窗口内到达的行"才可能被守卫拒绝。此时 Host 手里还握着本轮模型结果、事务已回滚、控制器候选已丢弃，因此唯一安全且有意义的做法是**在本轮内把它改判为补充要求轮并重新提交**：
   - Live：`live.rs` 把提交抽成 `commit_round_decision(...)`，`dispatch_initial_live` 在 `STEERING_COMPETITION` 上重读队列、用 `pre_history + 本轮回复 + 提示` 重建 `StopFollowup::Steering`、置 `adopt_all_received` 后重试（`STEERING_COMPETITION_ATTEMPTS = 4`；重试后的决策非终态，守卫在构造上不可能再拒绝）。
   - 非 Live：`kernel_coordinator.rs` 的 initial 与 batch 两条路径同构地重试一次并改判为 steering continuation，不再 `.map_err(replan_or_error)` 直接外抛。
3. **重启可恢复性** —— 失败路径不再留下 leased 派发，因此 `KernelCoordinator::reopen` 能正常 rehydrate：Run 保持 `running`，待派发的 `continuationModel` 效果仍在，补充要求轮可继续投递。

**测试（本轮新增 3 项）**
- `steering_live_tests::a_request_accepted_inside_the_live_decision_window_is_answered_and_applied`：**确定性屏障** `live::test_barrier`（按 run id 安装、触发即移除，测试内不会互相干扰）在"最后一次队列读取之后、写集之前"入队；随后驱动**真实外层 Live 循环**（真实 worker、真实脚本供应商、真实协调器）直到：Run `completed`、该行 `applied`（`applied_at` 非空）、steering lane 轮数 1、continuation 轮数 0、**恰好两次模型请求**，第二次请求同时含用户文本与 steering 提示、且**不含** stop-review 提示、被竞争轮的回复在其中**恰好出现一次**。
- `steering_tests::a_request_accepted_inside_the_decision_window_is_answered_not_failed`（非 Live）：同一窗口内入队后，`dispatch_initial` **返回 Ok**（不再有错误）、Run `running`、lane 轮数 1；并断言完整消息序列：初始用户消息/本轮回复/补充要求提示各恰一次、回复紧接提示之前。**重启证据**：用 `KernelCoordinator::reopen` 在同一数据库上重新 rehydrate，断言状态 `running`、`kernel_events` 中 `kernel.uncertain_execution` 计数为 0、快照里仍存在待派发的 continuation 效果。
- 既有 `a_lost_terminal_race_is_reported_as_a_replan_signal` 保留（覆盖映射本身）。

**局限**：重启证据是**同进程内 rehydrate**（`reopen`），不是进程级崩溃/重启演练；也未注入并发线程屏障（用的是轮内确定性钩子），因此"两个真实并发事务"的极端交错仍属推理范围。

---

## 2. F5（P2）：非 Live 续答不再重复本轮回复

**根因**：两条调用路径都先 `history.push(回复)` 再调用 helper，而 helper 内部又 push 同一个回复，于是每次非 Live steering 续答都复制一份最终回复。

**修复**：把契约写清并只在一处追加——`steering::steering_followup_input(database, binding, history_before_reply, refused_assistant)` 的参数改名为 **`history_before_reply`**（该轮所见历史，**不含**最终回复），函数内按 `历史 + 回复 + 提示` 组装一次；initial 路径传 `frame.input.messages`，batch 路径传 `batch_history`（批次历史 + assistant 工具调用 + 已结算工具结果），两者都不再自行 push。

**测试**：`a_request_accepted_inside_the_decision_window_is_answered_not_failed` 与 `the_batch_steering_follow_up_appends_the_reply_exactly_once` 断言**完整序列**：初始用户消息恰一次、本轮回复恰一次、steering 提示恰一次、提示紧跟在回复之后（initial）；batch 路径额外断言 `toolResult` 与工具输出仍在、且顺序为"工具结果 → 回复 → 提示"。

**局限**：非 Live 传输不会自行把该轮驱动到 `applied`（Live 传输负责），因此 `applied` 的端到端由第 1 节的 Live 测试覆盖。

---

## 3. F2（P1）：评测与生产走同一条受管写入执行缝

**根因**：评测入口自建 execute 回调，只调 `policy.execute_context_resource` / `policy.execute`，**绕过了生产的受管登记**；新加的登记断言在没有登记的路径上不可能通过（断言不等于接通被测路径）。

**修复**：把生产的"分类 → 写前保护 → 执行 → 写后登记"抽成无 UI 依赖的共用函数
`runtime_host::managed_files::execute_with_managed_versions(ManagedExecutionContext{…}, tool, input, tool_call_id, execute)`：

- **桌面 Host**（`kernel_host.rs` 的 Live 执行回调）改用它，行为与之前逐行一致（同一个 `verified_managed_write` 分类、同一 `capture_before`、同一 `record_after`/`record_office_write_from_result`，登记仍是 best-effort，失败只记日志不改写工具结果）。
- **真实链路评测**（`real_eval_tests.rs` 的 `drive_real_run`）改用它，并给出**隔离的数据目录** `root/managed-files`；`is_context_resource` 分支保持不变。
- 原来的 `KernelHost::record_office_managed_write` 包装函数删除，避免出现第二条登记路径。

**测试（新增 1 项）**：`runtime_host::managed_files::tests::the_shared_execution_seam_registers_exactly_what_it_executes`
—— 一次受管写入经该缝执行后：回调确实执行、登记行存在（`tool=write_file`、`tool_call_id` 相符、`source_verified`、`after_hash` 与磁盘一致）、并能通过公开恢复入口取回**逐字节相同**内容；随后一个非受管工具（`mcp__other__write_file`，结果里带同样的 `foxManagedFile` 声明）执行后**登记行数不变**。

**局限**：
- 该函数是"评测与生产共用"的证据；**真实 OfficeCLI**（`call_mcp_tool → fox-office → office_create/edit`）经此缝的执行、登记、恢复仍未在本机跑通（§7），因此 N1 的正向端到端仍标"未执行"。
- 复审提到的评测入口启动阻塞（32 分钟无进展）本轮**未再复现调试**：本轮未重跑该 ignore 入口，原因是先完成 F1–F5 的修复与可确定复现的验证；下一步应先加阶段标记（构建/初始化/数据库/子进程握手）与超时定位，而不是长等。

---

## 4. F4（P2）：逐工作表图表按真实 OOXML 关系核验

**根因**：实际判定是 `总图表数 ≥ 每表要求 × 工作表数`，从不记录图表属于哪个工作表；两张图都在 Sheet1、Sheet2 没有图也会通过，包内未被引用的 chart 部件也算数。

**修复**：新增 `delivery::ooxml_sheet_charts(bytes) -> SheetChartCounts { per_sheet: Vec<(名称, 数量)>, orphan_charts }`，按查看器实际解析的链路逐跳解析：`xl/workbook.xml`（`<sheet name r:id>`）→ `xl/_rels/workbook.xml.rels`（rId → 工作表部件）→ 工作表 XML 的 `<drawing r:id>` → `xl/worksheets/_rels/sheetN.xml.rels` → 绘图部件 `<c:chart r:id>` → `xl/drawings/_rels/drawingN.xml.rels` → chart 部件；**只有真实存在的 chart 部件才计数**。命名空间前缀（`<c:chart>`）与相对路径（`../`）都按包内规则解析。
`ArtifactFacts.sheet_charts: Option<SheetChartCounts>`（`None` = 关系链不可解析）。判定改为：逐表比较；未达标时按工作表名字报出实际数量与未达标表，并说明有多少孤立图表部件被排除；关系链不可解析 / 无可识别工作表 → **未核验**（既不算通过也不算用户无法处理的失败）。为此把"要求判定"从 `Option<String>` 提升为三态 `RequirementVerdict { Met, Unmet, Unverified }`，并让 `apply_delivery_requirements` 把 `Unverified` 如实写进交付报告（`verified: false`，`state: "unverified"`）。

**测试（重写 1 项，全部使用真实 xlsx 部件）**：`a_per_sheet_chart_demand_is_checked_per_sheet`
- 两表各一图（工作表→drawing→chart 关系齐备）：通过，且解析出 `[("Sheet1",1),("Sheet2",1)]`、孤立图表 0；
- **两图都在 Sheet1、Sheet2 无图**：<br>整包图表数 2（等于旧捷径 `1 × 2`）但**必须失败**，且失败文本点名 `Sheet2 … 未达标`；
- **孤立 chart 部件**（无人引用）：不计入任何工作表，失败文本说明"未被任何工作表引用"；
- **关系损坏**（删掉被引用的 drawing 部件）：`sheet_charts == None` → **未核验**，绝不当作 0 张、也不当作通过。
生产 gate 同步加强：`delivery_live_tests` 的修复产物现在由 `zip_attach_chart_per_sheet` 生成**逐表挂载**的图表（工作表 `<drawing>` + 两组 `.rels` + chart 部件 + 内容类型 Override），因此那条"交付修复"真的能通过逐表核验。

---

## 5. F3（P1）：统计核验接入生产，期望值来自源数据重算

**根因（复审认定，非新增范围）**：期望值仍取自任务文本中的数字启发式，没有绑定源附件/工作表/口径，产物为空时也不会从源附件补建统计要求。

**修复（有界、可解释，且不使用模型自己的输出当期望值）**

1. **从任务派生绑定**（`source_statistics_demand`，纯函数）：任务提出统计诉求（"统计/汇总/分布/占比/比例/频次"）时，
   - 若同时**指明数据文件**（`.xlsx/.xlsm/.csv/.tsv`，且以"读取/基于/根据/依据/参照/按照/依照/输入/来自/来源/分析/解析"引入，因此不会被误当成"要生成的文件"）**与聚合列**（`按 X 列/字段/分组/维度`，列名 2–16 字符、不含数字与空白）→ 产出 `SourceDistribution { source, alternates, label_header }`；
   - 否则产出 `SourceStatsUnbound { demand }` —— **显式未核验**并带上原文，而不是悄悄没有要求。
   - 中文散文与文件名共用字符（如"在"是散文停用字），因此同一处提及会给出"保守读法"与"最大读法"两个候选，由 Host 选**真实存在**的那个（`alternates`）。
2. **Host 侧绑定**（`bind_source_distribution`，在 `runtime_host/mod.rs` 任务开始处用**已授权项目根**调用）：只有文件真的存在、且源表里**确实存在该列表头**才绑定，并登记 `source_sha256`；否则回落为未核验（附尝试过的候选名）。
3. **核验时重算**（`evaluate_requirement` 的 `SourceDistribution` 分支）：读取源文件（相对路径在 Run 项目根下解析）→ `source_distribution_records`：定位含表头的工作表（前 20 行内）、按该列逐条非空记录计数 → 总数与各标签的期望占比 → 与每个交付产物逐标签比较：**同一行**必须给出该标签、该数量（按 token 匹配）与在 `SOURCE_RATIO_TOLERANCE_PERCENT = 0.5` 个百分点内的占比；总数也必须被声明。任一标签不一致即失败并点名产物、标签、源数据值。
   - 期望值**只**来自源记录：产物自身的数字不参与推导，两个产物写同样的错数仍然都失败。
   - 源记录在任务开始后改变时，按**当前**源数据重算 → 仍写旧数的产物失败，并在原因里附登记/当前哈希前 8 位（可解释的漂移证据）。
4. 该要求与比例要求一样属于**跨交付项**要求，经既有 gate 走真实交付检查入口（`evaluate_stop → apply_delivery_requirements`），未新增旁路。

**测试（新增 1 项）**：`source_statistics_are_recomputed_from_the_named_source`
- 用真实 xlsx 夹具承载源表（把某个工作表的 `sheetData` 换成"表头 + 记录"），并用**独立解析器 calamine** 读回期望：3 卸船 / 1 装船；
- 任务解析出 `SourceDistribution`（源文件名与列名正确）→ Host 绑定得到与文件字节一致的 `source_sha256`；
- 正确产物（总计 4、卸船 3/75.0%、装船 1/25.0%）通过；
- 写错数量的产物（两个产物用**同一个错数**）都失败，原因含"源数据"与标签名；
- 右数量、错占比失败；
- **源记录改变**后仍用旧数的产物失败，原因含"已变化"；按新源数据重算的产物通过；
- 无法绑定口径的统计诉求（无源文件/无列）→ `Unverified`；源文件不存在 → `Unverified`。

**局限（明确）**：
- 口径是"按该列逐条记录计数 + 各标签占比"，**没有**去重键、分类归并或指标 ID 概念；任务未显式给出"按 X 列"时一律未核验（不猜口径）。
- 因此对"根据附件统计"但没有指明列名的真实指令，本实现会给出**未核验**而不是伪造期望值——这符合"无法确定口径的要求显式留为未核验"，也意味着这类任务仍需更完整的口径声明（例如交付清单里写明统计列）。
- 未对 CSV 源做编码/分隔符的全矩阵测试（走同一 calamine 读取路径）。

---

## 6. 过程事故与恢复（如实记录）

本轮中我曾用 PowerShell 的 `Get-Content`/`Set-Content` 批量替换 `runtime_host/delivery.rs` 的一处调用点，导致该文件的中文被**二次编码**（GBK 往返），文件内容损坏。

- 处置：先尝试逆向往返修复（**有损**：`至�?`、`张图�?` 等出现，判定不可接受）→ 从上一轮结束快照恢复该文件 → **只用文件编辑工具**重做 F4/F3（`delivery.rs`）；其余文件的 F1/F5/F2 修改未受影响。
- 证据：该文件在重做后重新通过全部 18 项交付测试、生产 gate 4 项与全量 814 项测试；`delivery.rs` 中 F4/F3 的新增内容（`SheetChartCounts`、`SourceDistribution`、`source_distribution_records`）在报告中列出的测试里逐条被断言。
- 教训（已落实）：**不再用 PowerShell 文本管道改写含多字节字符的源文件**；批量替换一律使用编辑工具，或先用 ASCII 锚点定位。

### 快照与基线（本轮）

- **本轮改动前的基线**：`output/claude-design-repair-20260915/snapshot/`（N1–N7 轮结束时的工作树）。F 轮恢复 `delivery.rs` 时用的就是它；由于该快照是 N 轮结束状态，它**不能**用来回退 N 轮的修改（N 轮自身的逐文件开工基线不存在，见 `report.md` 的"基线快照"一节，不伪造）。
- **本轮结束状态**：`output/claude-design-repair-20260915/snapshot-f1-f5/`（390 个文件，约 9.8 MB：`apps/desktop/src-tauri/src`、`apps/desktop/src-tauri/tests/fixtures`、`apps/desktop/src`、`apps/desktop/tests`、`services/agent-runtime/{src,test}`、`crates`、`packages/fox-engine-protocol`；不含 `docs/` 与 `resources/` 中的二进制大文件）。

---

## 7. 未验收项（本轮未做，且不以本地可确定复现的修复代替）

1. **真实云模型（`FOX_EVAL_*`）**：无凭据 → 未执行。
2. **真实 OfficeCLI 端到端**：`cargo test --lib real_task_evaluation_through_host_kernel_runtime_and_real_tools -- --ignored` 本轮**未重跑**；上一轮该入口 32 分钟无进展（CPU 约 11 s、未创建输出目录、未拉起 OfficeCLI），已按"未执行"记录。因此 N1 的正向端到端、以及 F2 的"真实 Office 写入经共用缝登记并可逐字节恢复"仍标未验收（评测断言已就位）。
3. **GUI / 安装**：未验收。
4. **F1 的进程级崩溃/重启演练**：只做了同进程 `reopen` rehydrate 证据。
5. **F3 的真实 AGV 指令**：本轮测试用构造的任务文本与源表；真实指令若未给出统计列名，将按设计落到"未核验"。

---

## 8. 本轮改动文件

产品代码：
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/live.rs`（`adopt_all_received` 决策、`commit_round_decision` 抽取、竞争重试、提交后回读绑定行、`test_barrier`）
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs`（initial/batch 两条非 Live 路径的竞争重试）
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/steering.rs`（`history_before_reply` 契约）
- `apps/desktop/src-tauri/src/runtime_host/managed_files.rs`（`ManagedExecutionContext` + `execute_with_managed_versions`）
- `apps/desktop/src-tauri/src/runtime_host/kernel_host.rs`（改用共用执行缝；删除重复登记包装）
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs`（评测改用共用执行缝 + 隔离 managed-files 目录）
- `apps/desktop/src-tauri/src/runtime_host/delivery.rs`（逐表图表关系解析、三态判定、源数据统计）
- `apps/desktop/src-tauri/src/runtime_host/mod.rs`（任务开始时用项目根绑定源数据要求）
- `apps/desktop/src-tauri/src/database/repositories/{kernel.rs,run_steering.rs,delivery_checks.rs,repositories.rs}`（采用式绑定、绑定行回读、测试用只读查询、`alternates` 字段）
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/{steering_tests.rs,steering_live_tests.rs,delivery_live_tests.rs,managed_files.rs 测试模块}`
- `output/claude-design-repair-20260915/logs/*`（本轮证据日志，含命令与退出码）
- `output/claude-design-repair-20260915/snapshot-f1-f5/`（本轮结束快照）

未改历史迁移 1–66；未改长期记忆/技能；未提交、未推送。
