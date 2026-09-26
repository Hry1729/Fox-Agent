# C 阶段一页设计：`compute_job` 相邻完成事件的合并投递与可比较验收

**性质**：只读代码、只写方案；**未实施、未构建、未运行测试**。
**基线**：工作树 `worktrees/b2b-20260924`，HEAD `df9901c38500fc8f030905336252e49e2a3dd50b`（与预期一致）；原有 WIP 与未跟踪证据保留未动。
**目标**：围绕**现有** compute_job，定义相邻完成事件如何合并投递，并给出可比较的轮次/费用/延迟验收。
**不做**：不重写引擎、不建新消息平台、不扩展全量工具并发、不接命令 Job/任意 Shell。

---

## 1. 先核实已有行为（当前 HEAD 实读，不预设要改）

| 问题 | 现状 | 代码位置 |
| --- | --- | --- |
| 一次续答能否携带多个通知 | **能**。通知通道是**向量**：`KernelInitialModelInput.hostJobNotices` / `KernelRoundDirective.host_job_notices`；wake 时把**整批**装进同一条 continuation。**"整批"带容量条件**：该批是"wake 决策时刻全部未投递通知"按 `ORDER BY finished_at, job_id` 在 `16 KiB − 历史标记字节` 内的**最长前缀**；只有全部未投递通知都能装下时，"整批"才等于"全部"，溢出时该批只含前缀、余量仍 `pending`（durable 账本校验要求 supplied 与 `pending_model_facts` 逐条相等：`kernel.rs:600-608`） | `kernel_coordinator.rs:196-203`、`:461-464`；`live.rs:1053,1082`；`kernel_job_execution.rs:153-188` |
| 批次如何成形 | `pending_model_facts` 取该 Run **所有无投递行**的通知，`ORDER BY finished_at, job_id`，按 16 KiB − 历史标记字节装箱，**溢出即 break 不丢** | `kernel_job_execution.rs:153-188`；`kernel_coordinator.rs:164-175` |
| 何时才允许 wake | 要求 Run 处于 `waiting_jobs` **且该 Run 没有任何未终态 compute Job**（`state IN ('queued','running','paused')` 为空） | `kernel_coordinator.rs:445`；`kernel_jobs.rs:317-338` |
| 一个 park 能 wake 几次 | **至多一次成功唤醒**（有条件，不是"恰好一次"）：`wake_waiting_jobs` 要求 `state==WaitingJobs`、`wall_ms<=deadline`，**且该 Run 无任何未终态 compute Job**；只有全部条件同时成立才成功写入一条 `run.jobs_woken`（置 `Running` 并清空 `wait_deadline_wall_ms`）。条件不成立时该决策**可以反复执行而成功唤醒为 0**（`kernel_coordinator.rs:445` 提前返回、无副作用）；durable 守卫另有 `wake_events.len()==1`／`unfinished==0` 的逐决策上限，二者合起来是"至多一次"，不蕴含"必然一次" | `controller.rs:934-992`（守卫 940-950，状态清空 977-980）；`kernel_coordinator.rs:443-467`；`kernel.rs:741-744,960-964,1000-1004` |
| 通知是第几次正式结果 | 是**独立 typed lane**（`role:"hostJobNotice"` 历史标记），不是原 `toolCallId` 的第二次结果，也不是伪造用户消息 | `kernel_coordinator.rs:177-179`（doc 明示）；`kernel_job_execution.rs:433-438` |

**结论：同一 park 内的相邻完成事件，现行实现已经合并为"一次续答 + 一整批通知"。** 因此本卡的默认产物应是**验收卡**而非实施卡。

**仍会产生额外轮次的情形（需在验收中分别量化）**

1. **不同 park（因果新批次）**：不同模型轮次启动的 Job，或续答响应里又启动的新 Job —— 各自 park、各自一次续答。这不是"相邻完成事件"，不应合并。
2. **同 park 的慢兄弟**：今规则要求**全部** run-scoped Job 终态才 wake（§上表第 3 行），先完成者的通知被最慢兄弟拖住。若 `wait_deadline_wall_ms`（= `min(停驻时刻+剩余Run预算, max(Job deadline))`）先到，`account_waiting_jobs` 直接以 `kernel.job_wait_deadline_reached` 终止 Run；Host 顺序是先 account 后 wake，于是**本 Run 内这些通知不再进入模型历史**（durable 事实仍在）。
   `controller.rs:847-858, 910-924`；`kernel_host.rs:1453-1472`。
3. **容量溢出**：单批 >16 KiB 时余量保持 `pending`（无丢、无拆单条）。余量的排空**不靠"同一个 park 再 wake 一次"**，实际有两条路径：
   (a) **直接续答（不 park、不写 `run.jobs_woken`）**：任一模型响应落定时（初始轮、续答轮、或多个工具调用之后的批次轮；两种 transport 共用同一响应规划路径 `dispatch_initial_request`／`dispatch_batch`），若 Run 仍 `running` 且存在未投递通知，`job_notice_followup_input` 取"当时全部未投递通知"在容量内的最长前缀，`request_job_notice_followup` 直接武装**一条** `lane=job_notice` 续答（`kernel_coordinator.rs:180-204,793,1021-1094,1303-1308`；`live.rs:778,896,1563`；`controller.rs:2461-2502`；durable 守卫 `kernel.rs:745-774` 要求恰一个已落定响应 + 恰一条 notice 事件/outbox）。
   (b) **后续 park 的 wake**：只有**新的模型工具调用**留下未终态 Job 才会再次 park（§2「不得刷新原截止」），届时该 wake 重新取当时的最长前缀。
   **wake 成功后该 park 即结束**，所以"同 park 内再 wake"不是成功投递后余量的通用排空机制。容量边界还随历史增长而收紧：历史 `hostJobNotice` 标记同样计入这 16 KiB（第 41 行），直接续答另有 `notices_json.len() > 16*1024` 的 fail-closed（`controller.rs:2475`）⇒ **不承诺余量日后必能装下，也不承诺必达**；确定成立的只有"不丢、每条至多投递一次、余量持续可追溯"。当前无计数、无独立测量。
4. **迟到通知撞 Final**：`Completed` 守卫拒绝"存在未终态 Job"或（flag on 时）"存在未 ack 通知"的完成，退成 `kernel.jobs_pending` / `kernel.job_notice_competition:`；同一次落定调用内按分支重规划吸收（`attempts < STEERING_COMPETITION_ATTEMPTS`）：重新读未终态 Job ⇒ 有则转 park，无则重算**直接续答**（第 3 条 (a)），队列中已有 steering 输入时优先走 steering 轮。该重规划位于**两种 transport 共用的响应规划路径**上，所以不是"live 由下一轮折入、逐轮则需再 park"。`kernel.rs:3356-3386`；`kernel_coordinator.rs:1021-1094`（重规划 `1102-1136`）、`:1303-1308`；常量见 `controller.rs:124-128`。
5. **保守对照（flag off）**：模型只能靠 `compute_job_status(waitMs≤5000)` 轮询，**每次轮询本身就是一个模型轮次**。`background_jobs.rs:88-97`。

---

## 2. 合并规则（现行 + 仅两处待批准的扩展）

| 维度 | 现行规则 | 建议（**仅当验收数据支持**才实施） |
| --- | --- | --- |
| **"相邻"的定义** | 同一 Run、同一 wait generation（`park_seq`）内所有终态 Job 的通知；等价于"wake 决策时刻该 Run 的全部未投递通知" | 保持。**不以时间差定义**（不用"完成间隔 < X"），避免抖动改变语义 |
| **何时关闭一批** | 唯一条件：该 Run 全部 run-scoped Job 终态 | 可选放宽为「全部终态 **或**（≥1 条就绪 且 收敛窗 `τ` 到期）」，`τ = min(τ_max, wait_deadline_wall_ms − now)`，`τ_max` 为常量（如 250 ms） |
| **何时发起续答** | 两条入口：①park wake —— 批次关闭、仍在 `WaitingJobs`、仍在截止内**且该 Run 无未终态 Job** 时，该决策内才发出唯一一条 `run.jobs_woken` + 一条 `lane=job_notice` 的 `ContinuationModel`，Run 转 `Running`；条件不成立则该决策不唤醒（成功唤醒为 0，可反复重试），故整体是**有条件的至多一次成功唤醒**。②**直接续答** —— 任一模型响应落定且 Run 仍 `running` 时，若有未投递通知则直接武装一条同 lane 续答（不带 `run.jobs_woken`，与 park 无关） | 保持；同一 park 内**不得**出现第二条 wake 路径 |
| **排序** | `ORDER BY finished_at, job_id`（稳定全序），`history_position = history_start + order` 持久化 | 保持 |
| **容量上限** | 16 KiB − 历史标记字节；单条通知须过 `validate()` | 保持 |
| **溢出处理** | 不丢、不拆单条；余量留 `pending`，由**直接续答路径**（任一已落定响应经 `job_notice_followup_input`→`request_job_notice_followup`，无需 park）或**后续 park 的 wake** 投递；**不靠同 park 重试**，也**不限定"下一次 park"**；**不承诺必达**（历史标记占同一容量，见 §1 第 3 条）。整批为空才 fail-closed（`kernel.job_notice_capacity_blocked`） | 保持，另加**只诊断**的溢出计数（放 `run.jobs_woken` payload，避免迁移） |
| **不得无限等待** | 批次关闭与 wake 均不得越过原截止；越过即在同一决策内终止 | 保持；`τ` 只在**截止之前**生效 |
| **不得刷新原截止** | `wait_deadline_wall_ms` 仅在 `park_waiting_jobs` 写入；wake 要求 `wall_ms<=deadline` 并在成功时清空 | 保持；续答后的新 park 只能由**新的模型工具调用**产生，禁止为投递余量而人造 park |

---

## 3. 不变量（现状已强制，候选不得放松）

1. **可追溯、不重复**：`kernel_job_notices.job_id` 唯一，`kernel_job_notice_deliveries.job_id` 是 **PRIMARY KEY** ⇒ 每 Job 至多一行投递；`same_worker_terminal` 保证同 attempt/state/owner 下通知写入幂等。`migrations.rs:14875-14887`；`kernel_job_execution.rs:190-204`。
2. **不丢失（历史口径）**：`validate_host_job_notice_history_on` 要求历史中每个 `hostJobNotice` 都有 `acknowledged` 投递、`(checkpointSeq, position)` 严格递增、无重复 job_id，且 `expected == seen.len()`（已 ack 集合必须完整出现在历史里）。`kernel_job_execution.rs:442-493`。
3. **批内原子**：绑定行数必须等于通知数（`kernel.rs:497`），ack 必须精确更新 `notices.len()` 行（`kernel.rs:509-514`）⇒ 半批确认是错误。
4. **`job_start` 只正式返回一次**：`compute_job_start` 幂等（同 `idempotencyKey` 返回同一 Job）。`background_jobs.rs:38-60`；测试 `waiting_jobs_auto_tests.rs:198`、`host_job_notice_tests.rs:637,649`（`formal == 1`）。
5. **后台完成不是原 toolCallId 的第二次正式结果**：通知走 `hostJobNotice` 标记，不经 `tool` 消息；`job_notice_followup_input` doc 明确"never a fabricated user turn or a second tool result"。
6. **不伪造用户消息**：标记由 `validate_kernel_history` + data_root/conversation/run 作用域校验，模型自造字符串无法铸造。`kernel_job_execution.rs:433-438`。
7. **不重放副作用**：Job 结果只有 `result_ref/sha256/bytes` 完整性引用（`kernel_job_execution.rs:118-127`）；`Completed` 守卫阻止带未 ack 通知完成。
8. **未投递 ≠ 丢失（已声明边界）**：Run 因原截止失败终止时，通知仍是 durable 事实但不进模型历史。候选必须继续只声明"**可追溯**"，不得宣称"必达"。

---

## 4. 竞争与恢复

| 场景 | 现行行为 | 验收断言 |
| --- | --- | --- |
| 同时完成 | 两 Job 同拍终态 → 未终态集合为空 → 一批、`(finished_at,job_id)` 序、一次 wake | `continuation==1`、`notices==2`、`jobs_woken==1` |
| 完成时间错开 | 最慢兄弟决定批次（§1.2） | 报告"首个就绪 → 续答派发"延迟；若启用 `τ`，断言 `<= min(τ_max, deadline)` 且仍 `jobs_woken==1` |
| 续答已绑定后又来事件 | `kernel_model_notice_inputs(run_id,dispatch_key)` + `input_hash` 已绑定；迟到通知无投递行，保持 `pending`，不得回填已绑定输入 | 绑定批 ack 精确；迟到通知仍 `d.job_id IS NULL`；Run 已 `Running` ⇒ 无第二次 wake |
| 取消 | pending cancel 优先于 wake；`job_wake_cancel_pending` → `settle_parked_cancel` | `jobs_woken==0`、0 continuation、0 通知投递、取消终态、通知仍 durable |
| 截止 | `account_waiting_jobs` 在截止点终止；wake 在窗口外被拒 | 无 continuation、终态码 `kernel.job_wait_deadline_reached`、`wait_deadline_wall_ms` 未被刷新 |
| 重启 | 恢复扫描对**已冻结开启**的 Run 重发信号；已 ack 通知不重放，`bound` 未 ack 按原 `dispatch_key` 重验 `input_hash` | 跨重开 Host 只投递一次（复用 `b2b3-reopen-c4ad33a` 1/0，加**多通知**版本） |
| 未知投递结果 | 绑定不一致 / 其他 data_root → fail-closed（`orphan model notice binding`、`belongs to another data root`） | 复跑 a3bf5d6 类用例；不得新增第二条 wake 或放宽 fail-closed |
| 唯一认领 | `kernel_commit_waiting_wake` 要求恰好一条 `run.jobs_woken`；policy version CAS；重复事实被 `validate_job_wait_decision` 拒绝 | `kernel.rs:1987-1994`、`:741-744`；候选不得绕过 |

---

## 5. 可比较的验收（C 卡核心）

**三条对照**：A = 当前 HEAD（flag on，推送通知）｜B = flag off（模型 `compute_job_status(waitMs)` 轮询）｜C = 候选（仅当 A 有可测缺口才存在）。

**同一输入与完成时序**：同一段假脚本，同一轮用 `compute_job_start` 启动 N 个 Job；由本地模拟 Provider + 受控完成时刻摆布，偏移集合固定为 `{0, 50, 400, 2000} ms` 的全组合与逆序；**live 与逐轮两种 Pi 传输分别跑**。夹具、断言与运行器沿用 B 阶段 `waiting_jobs_auto_tests.rs` 的真实 Host + 本地 HTTP 模拟 Provider 结构，不引入新平台。

| 指标 | 取数口径 | 备注 |
| --- | --- | --- |
| **模型请求数** | 模拟 Provider 收到的 HTTP 请求数，按 kind(initial/continuation/final) + lane 分类 | 主指标，可分解 |
| **通知覆盖** | `kernel_job_notice_deliveries`（每 Job 一行）× 模型历史中的 `hostJobNotice` 标记 | 覆盖率 = 已投递 / 已终态，须 100%（除 §3.8 声明的截止失败边界） |
| **token usage** | 模拟 Provider 记录的请求/响应字节与 `usage` 字段 | **只证记账**；输入含历史标记字节，输出含响应文本 |
| **完成延迟** | `kernel_job_notices.finished_at`(最后一条) → `engine.continuation_requested` 的 wall 差；以及 Run 到终态的总 wall | 用**事件时间戳**，不用测试挂钟 |
| **轮次效率** | continuation 数 / 批次数、continuation 数 / 终态 Job 数 | 合并收益的直接读数 |

**必须声明的限制**：模拟 Provider 只能证明**请求数与用量记账**，**不能**证明真实费用节省、prompt cache 命中或真实分词器差异（不得写"成本降低 X%"）；非真实桌面、非真实/付费 Provider、非安装包；`SQLITE_DEFAULT_MEMSTATUS=0` 不得外推。逐轮与 live 的额外轮次来源不同（§1.2/.4 vs `live.rs` 折入），**必须分别报告，不得合并成一个数字**。

**预先约定的判据**（避免事后挑指标）：若 A 在"同轮 N 个 Job、完成时刻错开 ≤ `τ_max`"下已恒为 1 次 continuation 且覆盖 100% ⇒ **不实施**，C 卡即为验收卡，结论写成"A 已满足合并目标 + 边界不变"；只有出现 >1 次 continuation 或覆盖 <100%（除上述声明边界）才进入实施卡。

---

## 6. 最小实施范围与停止条件

**首选（推荐）：验收卡，零生产改动**
- 新增 `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/waiting_jobs_batch_tests.rs`，在 `tests.rs` 注册；复用既有真实 Host + 模拟 Provider 夹具。
- **不改** `crates/fox-agent-kernel`（`RunController` 公共接口不动）、不改 schema/迁移、不动 flag、不动预算与审批语义。

**次选（仅当验收证明需要）：实施卡，改动上限**
- `runtime_host/kernel_coordinator.rs::wake_waiting_jobs_with_policy_version`：把"全终态"放宽为"全终态 **或** `τ` 到期且有就绪通知"，加一个 `τ_max` 常量。
- 溢出诊断计数写进 `run.jobs_woken` payload（避免迁移）。
- 禁止：新增配置项/实验开关/迁移、第二条 wake 路径、改动投递绑定与 ack 契约。

**应退回方案（不得继续编码）**
- 需要改 `wait_deadline_wall_ms` 语义、`kernel_job_notice_deliveries`/`kernel_model_notice_inputs` 契约、`validate_job_wait_decision`，或 kernel crate 控制器 API；
- 需要新增迁移、schema 版本、实验开关，或改预算/审批语义；
- 需要改变 flag off 行为，或把通知变成第二次 tool result / 伪造用户消息；
- 验收无法用确定性夹具区分"合并"与"逐完成续答"（先改夹具，不实施）；
- 需要触碰命令 Job / 任意 Shell（本卡不接，另行审批设计）。

**边界复述**：不接命令 Job、不启任意 Shell；不调用真实或付费 Provider；不访问生产数据；不改代码/开关/schema/预算/审批；不启动其他 agent；不 push/合 main；不覆盖 WIP；不修改协调者已有报告。
