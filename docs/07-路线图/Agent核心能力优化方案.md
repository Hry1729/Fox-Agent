# Fox Agent 核心能力优化方案

> Fox 对话、工具、权限、审批、重试、取消与多引擎接入的统一控制改造，见 [Fox Agent Kernel 统一控制架构改造方案](./Fox%20Agent%20Kernel统一控制架构改造方案.md)。该文档同时记录 Pi 公开扩展通道的七项契约实验和实施门槛。

> 状态：实施中<br>
> 适用版本：Fox `0.1.x` 及后续版本<br>
> 维护范围：吸收 Maka、Kun、DeepSeek-Reasonix 与 OpenAI Codex 优点时的架构原则、实施顺序和验收门槛<br>

## 实施进度（2026-08-27）

本路线图已经进入实施，但仍不是“全部完成”：

- 阶段 0A 已完成：冻结 30-case fast 清单、报告身份 Hash、基线比较和 Runtime 恢复边界；
- 阶段 0B 的确定性 Eval 平台已完成：完整基线扩展为 121 cases、9 个 suite、65/65 工具目录覆盖和 6 个零容忍安全门槛；冻结的 30-case fast 清单保持 10/10/5/5 分类与原 Manifest Hash 不变。完整与快速基线均已全绿；真实 Provider、Host 故障注入、官方评测集和 CI 自动晋升仍未完成；
- Execution Profile Runtime/Host 已形成五个固定 Profile（含不对主模型暴露的 `graph_reviewer_v1`）、Strategy 组合校验、`ready`/Run Snapshot、Runtime 裁剪与 Host 权限门禁；`tool.execute` 和工具生命周期 Event 现在都必须同时通过当前 Worker Profile、Run 的 Host 冻结 Profile（canonical JSON/Hash）和 Ready Manifest 三重校验，缺失/篡改/身份冲突 fail-closed；默认仍为 `legacy`；
- Shadow/只读 Profile 不暴露 `git_read`：Git 的 textconv、fsmonitor 与可选锁可能执行仓库配置或改写索引，因此在完成独立的安全读取适配器前不把 Git 子进程宣称为零副作用；Profile ID 同样只接受注册表自身键；
- `ContinuationDecision v1`、v33 追加表、Host 事实校验和 accepted/rejected 审计已完成第一段；当前 Decision 只核对已提交事实，不代替 Goal/Task/Acceptance 状态转换；
- `TaskLedgerProjection` 与 `WorkSnapshotV2` 已完成 Host 原子快照阶段：A1、Work Graph、Ledger、selected Goal 与根 Source identity 在一个 SQLite read transaction 生成；投影只读、按需从原事实重建，不建立第二套任务库；Host 使用关键事实优先的结构化尾窗与 total/returned summary，Runtime 已收口为 Prompt Composer 唯一 Host-authority 注入点，并提供恢复关键字段保留和片段 Hash/diagnostics；
- `ValidationPolicy v1` Host/Runtime 合同与第一段执行闭环已完成：固定 `legacy_v1`、`standard_v1`、`high_risk_v1` 三套不可混搭快照和稳定 Hash；Host 冻结 Run profile 与 Task policy，未知/混搭/篡改 fail-closed，旧 Task 才 Legacy fallback；Attempt/Repair、fresh Evidence/Review、atomic Acceptance 已落地；
- Prompt Registry + Typed Context 基础契约及安全加固已完成：`fox.runtime.system` 的不可变 `stable_v1/registry_v1`、唯一固定 preview matrix、完整 content/context Hash、统一 fragment authority/trust/version/budget/lifecycle、伪 marker 等长中和、唯一 Host WorkSnapshot、Policy/Attempt 恢复保留、stable-prefix/dynamic-tail cache identity，以及只允许 offline/mock/read-only-shadow 的 Prompt Experiment/Report；四个生产 Profile 在 freeze/resolve 两层继续拒绝 `registry_v1`，尚未上线；
- 阶段 1 的 Host Task Policy、Attempt/Repair、Run 终态/重启补偿、跨 Repository atomic Acceptance 与一次性人类 Repair escalation 已完成；尚缺按 ContinuationDecision 自动调度下一 Attempt/Repair 和生产 UI；
- 阶段 1.5A 的内存态只读 Graph 预览已完成；1.5B 已完成批准 PlanRevision 的不可变激活、持久快照、节点启动/取消/终态对账、逐条 criterion↔Evidence 绑定、独立 Reviewer、Lead 明确 finish 和当前 Plan 的最终 Graph Acceptance。所有权威转换均由 Host/Repository 原子完成并支持重启恢复；生产 UI、可写 Graph、动态扩图、跨节点共享写入和真实任务试点仍未完成。

对应稳定事实已同步到《Agent运行时架构》《数据架构》《Runtime协议》《数据库结构》和《Agent能力测试基准》。下文未标为已完成的内容继续按候选方案理解。
> 最后更新：2026-08-27

本文回答两个问题：Fox 是否应该学习其他优秀 Agent；如果学习很多项目的长处，怎样避免最终产品臃肿、混乱和互相冲突。

结论是：**可以学习，而且很值得学习；但只能学习经过验证的设计原则，不能把其他项目的模块原样拼进 Fox。** Fox 应继续以自己的工作治理内核为主，把 Maka、Kun、Reasonix 和 Codex 的优点翻译成 Fox 已有的 Goal、Task、Evidence、Acceptance、Run/Event、Child Run、Expert、Permission 和 Prompt Context，而不是再建立第二套任务系统、第二套状态机或第二套权限体系。

相关对比基础见[基础 Agent 核心能力对比](../01-项目概览/基础Agent核心能力对比.md)和[Fox 与主流 Agent 能力及 Prompt 对比](../01-项目概览/Agent能力与Prompt对比.md)。

## 外部工程评审的吸收结论

本方案经过第二位工程师评审后，保留原有架构原则，并补充执行层细节。评审意见的处理如下：

| 评审建议 | 处理结论 | 优化方式 |
| --- | --- | --- |
| 增加时间和资源估算 | 采纳 | 增加带前提条件的 ROM 粗估，不把估算写成承诺 |
| 明确数据迁移和回滚 | 采纳 | 增加逐阶段 Schema 候选、Legacy 语义、备份恢复和 Expand/Switch 流程 |
| 建立性能预算 | 采纳并修正 | 分开 Host 本地计算和模型/Provider 延迟，不用 `<100ms` 约束模型生成 |
| 量化成功指标 | 采纳 | 增加安全硬门槛、质量提升门槛、成本和延迟预算 |
| 描述用户体验变化 | 采纳 | 增加各阶段用户可见信息和不应展示的内部细节 |
| 展开测试策略 | 采纳 | 增加单元、迁移、契约、集成、E2E、故障注入、安全、性能和 Eval 金字塔 |
| 明确 Feature Flag | 采纳并收敛 | 使用少量受支持的执行 Profile，不允许任意布尔开关自由组合 |
| `ContinuationDecision` 确定性校验 | 采纳 | Runtime 可以提议，Host 必须确定性校验后才能写入或执行 |
| `TaskLedgerProjection` 声明依赖 | 采纳 | 增加来源水位、Schema 版本和只读接口，保证可删除重建 |
| Prompt 影子部署 | 有条件采纳 | 默认只做离线或只读影子评测；有外部副作用的任务禁止静默双跑 |
| Graph Lead 明确并发控制 | 采纳 | 只读并行优先，Writer 必须有写域声明、冲突检测和隔离 |
| 合并耐久循环与验证阶段 | 采纳 | 合成 `Durable Loop with Validation`，防止 Agent 更耐久地执行错误方案 |
| 提前做只读 Graph MVP | 有条件采纳 | 作为 Phase 1 通过后的限量设计试点，不提前成为生产主路径 |

其中两个关键修正是：模型生成 `ContinuationDecision` 的时间受模型和网络影响，性能预算只能严格约束 Host 校验与投影计算；Prompt 候选如果会写文件、发网络请求或触发外部动作，不能为了 A/B 测试而在后台重复执行。

## 先回答：集百家之长会不会变乱

### 会乱的做法

如果按照下面的方式学习，几乎一定会乱：

- 看到 Maka 有 Task Ledger，就在 Fox 现有 Goal/Task 旁边再建一套任务表。
- 看到 Kun 有 Graph，就让 Graph 自己保存状态，不再经过 Fox Evidence 和 Acceptance。
- 看到 Codex 的 Prompt 很完整，就把整份 Prompt 复制到 Fox 的 System Prompt 后面。
- 看到 Reasonix 按需加载 Skill，就另写一套绕开 Expert Package 的 Skill 管理器。
- 每增加一种 Agent 模式，就增加一套独立工具、权限、状态和页面。
- 同一个“完成”在对话、Workflow、Child Run、Graph 和数字同事里有五种不同含义。

这种做法的问题不是功能多，而是同一件事有多套答案：任务到底存在哪里、谁说了算、哪种状态才是真实状态、谁负责验收、权限由谁控制。系统一旦出现多个事实源，恢复、审计、测试和用户理解都会迅速变复杂。

### 不会乱的做法

正确方法可以概括为一句话：

> 学习外部项目的“方法”，统一落到 Fox 的“骨架”里。

例如：

- 学 Maka 的耐久循环，但记录仍写进 Fox Run/Event 和工作图。
- 学 Codex 的完成审计，但完成仍由 Fox Acceptance 和 Host 校验决定。
- 学 Kun 的 Graph Lead，但 Graph Node 最终映射成 Fox Task/Child Run/Evidence。
- 学 Reasonix 的按需 Skill，但仍通过 Fox Expert Package、Prompt Composer 和 Host 权限加载。

这样增加的是 Fox 原有能力的深度，不是新增四套互不相干的框架。

## 防止混乱的六条“宪法”

后续任何竞品能力进入 Fox 前，都必须满足以下六条原则。

### 1. 只有一个产品事实源

SQLite 与 Host Repository 继续是本地产品事实源。Goal、Task、Evidence、Review Finding、Acceptance、Run、Child Run 和 Approval 的最终状态只能由 Host 写入。

可以增加事件、投影和缓存，但它们必须满足：

- Event 是事实变化记录。
- Projection 是从事实计算出的便捷视图。
- Runtime Session 是恢复加速缓存。
- Prompt Context 是给模型看的当前快照。
- 上述任何内容都不能偷偷成为第二份权威数据库。

大白话理解：公司只能有一套正式账本。部门可以做报表、草稿和缓存，但出现冲突时必须回到正式账本。

### 2. 只有一套核心工作循环

所有 Assistant、Expert、Workflow、Team、Child Run 和数字同事最终都遵循同一条循环：

```text
接收目标
  → 判断是否需要澄清/规划
  → 执行动作
  → 读取真实结果
  → 验证
  → 修复或继续
  → 验收完成 / 等待审批 / 确认阻塞
```

不同模式只能改变策略，例如是否启用 Planner、是否允许委派、需要什么验收，不能改变“工具必须真实返回、完成必须有证据、权限不能扩大”等核心语义。

### 3. 只有一条工具与权限边界

所有文件写入、命令、网络、知识库、MCP、OpenAPI、Child Run 和未来插件工具继续经过 Host Preflight/Execute、审批、范围校验和审计。

不能因为某个新 Graph、Skill 或专家工作台“内部已经校验”，就允许它绕过 Host。Prompt 负责告诉模型规则，Host 负责真正强制执行规则。

### 4. 只有一条 Prompt 组装管线

所有基础指令、人格、专家、权限、项目规则、记忆、插件能力、目标、环境和本轮状态继续由 `composeFoxPrompt` 的演进版本统一组装。

可以把上下文拆成更多类型化片段，但不能让每个功能各自在消息前后随意拼字符串。每个片段都应有来源、权威度、可信度、版本、Hash、预算和生命周期。

### 5. 只有一套完成与阻塞定义

“完成”必须满足目标对应的 Task 已终结、必要 Evidence 有效、Review/Acceptance 通过、没有待处理高风险动作。“阻塞”必须有可解释且可重复确认的外部条件，不能因为第一次工具失败就标记阻塞。

普通对话可以快速结束，但只要进入 Goal/Workflow/Graph，就必须服从统一完成和阻塞审计。

### 6. 所有新增能力先过评测和回滚门

任何新循环、Prompt、Graph 策略或自动化只有在以下条件满足后才能默认开启：

- 有固定任务集和旧版本基线。
- 能证明任务质量、权限、安全和成本没有不可接受回退。
- 有 Feature Flag、版本号或迁移边界。
- 出问题时能回滚到旧策略，不破坏已有数据。
- 用户能理解新能力带来的可见价值。

## 总体吸收方案

| Fox 要增强的能力 | 主要学习对象 | 学什么 | 融入 Fox 的位置 | 明确不复制什么 |
| --- | --- | --- | --- | --- |
| 闭环执行 | Maka、Codex | 耐久事件循环、续跑、完成审计、根因修复、验证后结束 | RuntimeHost、Run/Event、Work Tools、Runtime Session、Acceptance | 不引入第二套 Runtime 事实源；不复制 Codex Prompt 代替系统能力 |
| 任务状态 | Maka | Task Ledger、事件回放、上下文与历史分离 | 从现有 Goal/Task/Evidence/Run 派生 `TaskLedgerProjection` 和 `WorkSnapshotV2` | 不新建与 Goal/Task 平行的 Maka 式任务库 |
| 工具可靠性 | Codex、Kun | 动态工具说明、按实际能力选择工具、工具失败继续处理 | Capability Manifest、Runtime Tool Catalog、Host Preflight/Execute | 不让 Prompt 或 Graph 自己授予工具权限 |
| 验证与纠错 | Kun、Codex、Maka | Review/Repair、根因验证、批量回归 | A1 Review Finding/Acceptance、Child Run、离线 Eval | 不接受子 Agent 自报完成；不只看单次 benchmark 分数 |
| Prompt 工程 | Maka、Reasonix、Codex、Kun | A/B、短稳定前缀、类型化片段、专用角色契约 | Prompt Composer、Expert Package、Eval Harness | 不把四个项目 Prompt 全部拼接成超长 Prompt |
| 权限安全 | Codex、Reasonix | 显式 Sandbox/Approval 说明、目录指令边界、符号链接防逃逸 | Host Permission、Project Root、Prompt Permission Context | 不弱化 Fox 现有 Host 收口和权限交集 |
| 规划 | Kun、Maka、Codex | DAG、节点依赖、持续更新、完成前审计 | Planner、PlanRevision、Task、Workflow v2 | 不让 Planner 创建第二套任务事实 |
| 上下文与记忆 | Reasonix、Codex、Maka | 按需加载、Compaction、动态尾部、恢复摘要 | Prompt Context、Runtime Session、受治理 Memory | 不把摘要当最终事实；不把未确认资料写入长期记忆 |
| 多 Agent | Kun、Maka、Codex、Reasonix | Graph Lead、Worktree 隔离、角色化子 Agent、聚焦 Skills | Child Run、Expert Team、Workflow、Trace | 不先追求数量和深度；不允许子 Agent 扩权 |

## 一、闭环优化：重点学习 Maka 和 Codex

这是最值得优先实施的部分。

### Maka 值得学习什么

Maka 最值得学习的不是“循环次数多”，而是每一次运行都留下足够明确的事件和任务状态，使系统可以回答：

- 上一步发生了什么。
- 当前在等待什么。
- 为什么失败。
- 重启后应该从哪里继续。
- 同一个任务用不同 Prompt/策略运行时，结果有什么差异。

Fox 已经有 Run/Event、SQLite 事实源、Runtime Session 和恢复机制，因此不需要引入 Maka Runtime。应学习它的事件耐久与 Task Ledger 思路，把现有状态投影得更清楚。

### Codex 值得学习什么

Codex 最值得学习的是执行纪律：

- 用户要求没有真正解决前，继续工作。
- 优先修根因，不只隐藏表面报错。
- 每次修改后按风险进行验证。
- “做过”不等于“完成”。
- 只有满足完成条件，或者存在经过审计的真实阻塞时才停止。

这部分不能只复制进 Prompt。Fox 应把它落实为 Host 可判断的状态和完成门。

### 建议的统一循环

建议把 Fox 的运行阶段明确为以下逻辑状态。它不一定全部成为数据库枚举，但必须在 Runtime、Host、Trace 和测试里具有一致语义。

```text
intake
  接收请求并解析目标、范围和授权

clarify_or_plan
  必要时澄清；复杂任务生成只读计划

execute
  选择并调用一个动作或一组安全并行动作

observe
  读取真实工具结果，更新事实状态

validate
  根据任务风险和接受标准验证

repair
  验证失败后定位原因、修复或更换方案

accept
  写入 Evidence、Review、Acceptance

complete / wait_approval / blocked / cancelled
  进入具有明确原因的终态或等待态
```

### 建议新增 `ContinuationDecision`

每次准备结束或继续时，Runtime 可以提议一份结构化决策，而不是只依靠模型自由文本。Host 读取当前事实并确定性校验；校验不通过时，原决策不能生效：

```json
{
  "schemaVersion": 1,
  "decisionId": "decision-1",
  "runId": "run-1",
  "eventCursor": 128,
  "decision": "continue | repair | wait_approval | complete | blocked",
  "reasonCode": "validation_failed",
  "activeTaskIds": ["task-1"],
  "evidenceIds": ["evidence-1"],
  "missingAcceptance": ["tests_pass"],
  "nextAction": "run targeted test",
  "retryClass": "recoverable",
  "blockedDependencyRefs": []
}
```

Host 应校验：

- `complete` 是否满足 Goal/Task/Evidence/Acceptance 条件。
- `blocked` 是否包含真实外部条件，而不是普通工具失败。
- `repair` 是否仍在预算、权限和重试策略内。
- `wait_approval` 是否存在真实 Pending Approval。

建议把决策保存为一对多的追加记录，例如 `run_continuation_decisions`，而不是在 `runs` 表只增加一个可覆盖字段。一个 Run 可能经历多次 continue、repair 和 wait，追加记录才能保留完整审计历史。

`reasonCode` 必须来自版本化枚举，例如 `validation_failed`、`approval_pending`、`external_dependency_unavailable`、`budget_exhausted`、`user_input_required`。自由文本只能作为补充说明，不能决定状态转换。

确定性校验至少覆盖：

```text
complete
  所有目标 Task 已进入允许终态
  必需 Evidence 有效
  必需 Acceptance 已通过
  没有 Pending Approval 或未处理 Finding

blocked
  存在可引用的外部依赖、用户决策或不可恢复权限条件
  普通工具错误、测试失败和首次重试不得伪装成 blocked

repair
  存在未解决 Finding 或验证失败
  仍在重试、Token、工具次数和时间预算内

wait_approval
  Host 中存在与当前 Run/ToolCall 对应的 Pending Approval
```

大白话理解：Agent 每次想下班，都要填一张“为什么可以下班”的单子；系统检查工作是不是确实完成了。

### 恢复方式

恢复时不应该只把整段聊天重新喂给模型。建议按顺序重建：

1. 从 SQLite 读取 Goal、Task、Evidence、Acceptance 和 Run 终态。
2. 从 Run/Event 重建最近一次有效执行阶段。
3. 读取 Pending Approval、Child Run 和外部等待条件。
4. 生成新的 `WorkSnapshotV2`。
5. Runtime Session 可用时作为加速；不可用时仍可根据事实状态继续。

第 4 步已经收口为 `apps/desktop/src-tauri/src/database/repositories/work_graph.rs` 的单 read transaction；`apps/desktop/src-tauri/src/database/repositories/a1_workflow.rs` 只提供共享连接读取，不新增事实写口；`apps/desktop/src-tauri/src/runtime_host/work_tools.rs` 负责根 Hash/high-watermark/生成时间与结构化预算。实现吸收 `../maka-agent-main/packages/storage/src/sqlite-session-metadata-store.ts` 的同事务 snapshot/version 边界和 `../maka-agent-main/packages/core/src/task-ledger.ts` 的稳定投影顺序，也吸收 `../codex-latest/codex-rs/state/src/runtime/goals.rs`、`recovery.rs`、`recovery_tests.rs` 的陈旧版本与恢复测试思路；没有照搬外部可写 Projection、Goal Store 或恢复数据库。

### 闭环验收标准

- 工具成功、失败、超时、取消和审批拒绝均能进入明确下一状态。
- 应用在工具完成后、验证中和等待审批时退出，重启后均可恢复。
- 没有有效 Evidence 的 Task 不能因为模型说“完成”而完成。
- 普通可恢复错误不会被误判为永久阻塞。
- 相同错误超过策略限制后停止重试，并给出结构化失败原因。
- 最终答复只引用已经发生的动作和已验证结果。

## 二、状态优化：重点学习 Maka

### 学习目标

Fox 当前状态对象很丰富，但模型和用户不一定能快速看懂“任务现在究竟进行到哪里”。Maka 的 Task Ledger 值得学习，因为它把长运行过程压缩成一张始终更新的任务账单。

Fox 不应该复制一套新 Ledger 数据库，而应从现有事实派生一张统一视图。

### 建议新增 `TaskLedgerProjection`

```text
TaskLedgerProjection
  goal
    id / title / status / completion_policy

  active_tasks[]
    task_id / title / status / attempt
    acceptance_checks[]
    latest_evidence[]
    blockers[]
    assigned_run_id / child_run_id

  pending_actions[]
    approval / external_wait / retry / review / repair

  completed_summary[]
    task_id / outcome / evidence_ids / accepted_at

  execution_cursor
    current_phase / last_event_id / resumable_from

  projection_meta
    schema_version
    source_high_watermark
    generated_at
    derived_goal_ids / task_ids / run_ids / event_range
```

Projection 可以按需计算或缓存，但必须能够从 SQLite 事实重新生成。它服务三个消费者：

- Runtime：获得紧凑、结构化的当前工作状态。
- UI：向用户展示“进行中、等待审批、需要返工、已验收”。
- Eval/诊断：判断 Agent 是否丢失目标、重复执行或错误结束。

Projection 必须是只读 API。任何状态更新都必须调用 Goal/Task/Evidence/Acceptance/Run Repository，成功提交后再重新计算投影。代码层不提供 `projection.update()` 一类接口，防止工程师误把 View 当事实源。

当前 `load_work_graph_snapshot` 已经从 SQLite 一次加载 Goal、Task 和每 Task 最近 Evidence，`TaskLedgerProjection` 应在这条现有读取路径上扩展，并补充 Acceptance、Finding、Pending Approval、Child Run 和事件水位，而不是另起一条数据同步管线。

依赖方向固定为：

```text
SQLite 产品事实
  → TaskLedgerProjection
  → Prompt Context / UI / Eval
  → Runtime 执行
  → Host 校验并回写 SQLite
```

Prompt Context 不能反向修改 Projection，Projection 也不能反向修改 SQLite。

### 状态分层

建议明确五类状态，避免互相污染：

| 状态类型 | 例子 | 是否权威 | 保存位置 |
| --- | --- | --- | --- |
| 产品事实 | Goal、Task、Evidence、Acceptance、Approval | 是 | SQLite / Host Repository |
| 执行事件 | tool.started、tool.completed、run.failed | 是，表示发生过的事件 | Run/Event |
| 派生投影 | Task Ledger、进度摘要、统计 | 否，可重建 | 查询结果或缓存 |
| Runtime 恢复状态 | 模型消息、检查点、压缩上下文 | 否，是运行缓存 | Runtime Session |
| Prompt 上下文 | 当前提供给模型的快照 | 否，是输入视图 | Prompt Composer |

大白话理解：正式合同、监控录像、日报、电脑临时文件和给员工的任务简报不是同一种东西。把它们分清楚，系统才不会在恢复时相互覆盖。

### 状态优化验收标准

- 任意 Run 都能生成一份紧凑 Task Ledger。
- Task Ledger 删除后可以从正式事实无损重建。
- UI 刷新和应用重启后，任务状态不依赖前端内存。
- 模型不需要读取完整历史，也能知道当前目标、完成项、阻塞项和下一步。
- 同一事实不会同时由 Runtime Session 和 SQLite 竞争写入。

## 三、工具优化：学习 Codex 和 Kun，但保留 Fox 门禁

### 值得学习的部分

Codex 的优势是工具和 Sandbox/Approval 紧密结合；Kun 的优势是根据本轮实际公布的工具动态生成使用偏好。Fox 已有 Capability Manifest、Tool Catalog 和 Host Preflight/Execute，可以在现有基础上增强模型对“当前真正能做什么”的理解。

### 建议做法

- 每个 Run 生成 `EffectiveCapabilitySnapshot`，包含工具、来源、风险等级、是否需审批、项目范围和关键预算。
- Prompt 只注入当前有效工具，不注入已安装但本轮无权使用的工具。
- 相似工具提供统一选择提示，例如优先专用只读工具，再考虑 Shell。
- 工具失败返回稳定 `reasonCode`、是否可重试、建议动作和脱敏诊断。
- Tool Result 进入 Event、Trace，并可被 Evidence 引用。
- Graph/Child Run 只能继承有效能力的交集。

### 不应做的事

- 不要让模型根据 Prompt 自己判断是否获得权限。
- 不要让 Expert、Skill 或插件直接打开文件、网络或凭据通道。
- 不要同时维护“桌面工具目录”“Runtime 工具目录”“Graph 工具目录”三套不一致清单。

## 四、验证与纠错：学习 Kun、Codex 和 Maka

### 三个项目分别提供什么

- Codex：每次修改后按风险验证，优先确认根因是否解决。
- Kun：Lead 对子节点执行 Review，不通过时进入 Repair，再重新验收。
- Maka：用固定任务集批量比较 Prompt 或策略变化，发现系统性回退。

### 融入 Fox 的方式

Fox 已有 Evidence、Reviewer、Finding 和 Acceptance，因此不需要新增一套 Review 数据模型。建议扩展现有对象关系：

```text
Task / Graph Node
  → Attempt
  → Artifact + Evidence
  → Review Finding
  ├── pass → Acceptance
  └── fail → Repair Task / Repair Attempt
                 └── 再次 Review
```

每种 Task 应声明 `ValidationPolicy`：

```text
risk_level
required_checks
allowed_check_types
reviewer_policy
max_repair_attempts
completion_requires_acceptance
```

低风险任务可以只做一个轻量检查；代码、数据删除、发布和权限变更必须具有更严格验证。这样可以学习 Codex 的“按风险验证”，而不是所有任务都跑同样昂贵的流程。

### 已实现的 Runtime 纯契约（2026-08-26）

`services/agent-runtime/src/validation-policy.mjs` 已把候选设计收敛成 schema v1 的三套固定快照，不提供任意字段组合：

| Policy | `riskLevel` | `requiredChecks` | `allowedCheckTypes` | `reviewerPolicy` | 最大 Repair | 完成需 Acceptance |
| --- | --- | --- | --- | --- | ---: | --- |
| `legacy_v1` | `legacy` | 无 | `test/inspection/review/manual/other` | `legacy` | 0 | 否 |
| `standard_v1` | `standard` | `inspection` | `test/inspection/review/manual` | `host_validated` | 2 | 是 |
| `high_risk_v1` | `high` | `inspection/review` | `test/inspection/review/manual` | `independent` | 1 | 是 |

每个冻结快照只包含 `schemaVersion/id/riskLevel/requiredChecks/allowedCheckTypes/reviewerPolicy/maxRepairAttempts/completionRequiresAcceptance/hash`。Hash 是对不含 `hash` 字段的规范 JSON 计算的完整 SHA-256：

- `legacy_v1`: `4129f5db32d88d7070c59ed2351aab9c6ad59c8cc380451901bb10962c5bee92`
- `standard_v1`: `bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d`
- `high_risk_v1`: `bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea`

`resolveValidationPolicy` 把缺失值解释为 `legacy_v1`，保证旧 Task 不被追加强制检查、Acceptance 或自动 Repair；`freezeValidationPolicy` 返回深度冻结的规范快照；`validateValidationPolicy` 精确核对字段集合、版本、Policy ID、每个值和 Hash。未知版本/ID、多余或缺失字段、跨 Policy 混搭、任意 Check 类型和 Repair 次数都会失败，不能静默降级。

这只是 Runtime 模块的只读策略契约，不选择 Task 风险、不创建 Finding/Acceptance、不递增 Repair，也不写 Host 状态。Host 现已在 Task 创建时选择并冻结规范快照，并由 Repository 保存与执行；旧 Task 缺少快照时才使用 `legacy_v1` 读取语义。

设计上沿用 Fox Acceptance 已支持的 `test/inspection/review/manual/other` 方法与 Finding/独立 Reviewer 事实；参考 Kun 的“无有效验证不能 pass、独立只读 Reviewer、失败后 Repair 再 Review”，以及 Codex 的有限风险等级、只读补充检查和高风险 fail-closed 决策。没有复制 Kun Graph 状态或 Codex Guardian 权限系统。

### 验收标准

- 子 Agent 自报完成不能直接让节点 Accepted。
- Review Finding 必须能定位到产物、Evidence 或具体文件位置。
- Repair 后原 Finding 被标记为 resolved、superseded 或仍未解决。
- 达到最大修复次数时进入明确人工决策，不无限循环。
- Prompt/策略升级必须跑固定 Eval，不能只用一次成功案例证明有效。

## 五、Prompt 优化：吸收四家长处，但只保留一条管线

### 学习内容

| 来源 | 值得学习的点 | Fox 中的实现方式 |
| --- | --- | --- |
| Maka | Prompt A/B、候选、回放、趋势和晋升 | Prompt Registry + Eval Case Set + Experiment Report |
| Kun | Graph Lead、Reviewer、Designer 等专用角色契约 | Expert/Workflow/Role Fragment，不进入通用 Base Prompt |
| Reasonix | 短稳定前缀、按需 Skills、缓存友好 | 高变化方法论延后加载，稳定前缀保持字节级稳定 |
| Codex | 权限、人格、目标、模式、插件和环境类型化拆分 | `PromptFragment` Schema 和独立 Hash/预算 |

### 建议的类型化片段

```text
BaseInstructions
PermissionContext
PersonaContext
ExpertContext
ProjectInstructionContext
MemoryFactsContext
CapabilityContext
GoalContext
WorkStateContext
RuntimeWorldState
TurnContext
```

每个片段至少包含：

```text
kind
authority
trust
volatility
source
version
hash
token_budget
redaction_policy
```

### Prompt Registry

建议为生产 Prompt 建立正式版本和评测记录：

- 哪个角色使用。
- 稳定模板 Hash。
- 片段 Schema 版本。
- 对应源码 Commit。
- 经过哪些任务集评测。
- 相比基线提升和回退了什么。
- 谁批准成为默认版本。
- 如何回滚。

基础实现已经落在 `services/agent-runtime/src/prompt-registry.mjs` 与 `typed-context.mjs`：

- `PromptDefinition/PromptVersion` 是不可变注册表记录，读取时复算完整 SHA-256；`stable_v1` 保持现有模型输入，`registry_v1` 标记为 development；
- `registry_v1` 只能通过固定 `fox.prompt.registry.development.v1`/`prompt_registry_preview`（Hash `65c3e97d4cca918161d5bc29415f19ed26c7561cbee684d98094ee5880766322`）；freeze 与 resolve 都拒绝现有四个生产 Execution Profile；
- Fragment 固定 `id/kind/authority/trust/version/hash/budget/lifecycle/content`，Composer 拒绝裸字符串、额外字段、future version、原型对象、篡改 Hash 和多个 WorkSnapshot；统一 renderer 等长中和伪 marker，并对安全内容与最终 Prompt 做硬预算检查；
- WorkSnapshot compact 在截 Task 前从原始 Attempt、Ledger active 与 cursor 提取关键 Task，按 running/cursor/active/原序补位；全部原始 running Attempt 及其 Task/Policy 独立保留，fallback 放不下时 durable/graph fail-closed，Legacy 稳定合同语义不变；
- cache key 排除 WorkSnapshot/turn/env 动态尾部，只记录 eligibility/identity，不伪造 provider hit；
- Experiment 只接受 offline replay、mock、read-only shadow，文件写入、网络、MCP action、Child Run、计费、外部消息、生产写入和静默双跑必须全部显式为字面量 false；缺失/未知/null/类型错误均拒绝，Token 与 Safety failure 必须为非负整数；
- safety gate 不能被平均分抵消，自动逻辑只产生 rollback 建议，永不自动改生产 Prompt。

仍未完成的是生产候选流量、真实 Provider held-out、多次试验置信度、人工批准/晋升记录、Cache Provider、数据库 Prompt Registry 和冻结的新 Execution Profile。上线这些能力前必须单独扩充 Profile 矩阵和 Eval，不得把 development support matrix 当 Feature Flag 使用。

### 防止 Prompt 变乱的规则

- 同一个规则只能有一个权威来源。
- 专业方法论进入 Expert/Skill，不进入所有 Agent 的基础 Prompt。
- 动态环境放尾部，不能破坏稳定前缀缓存。
- Memory 作为事实片段，不得提升成 System 权限。
- 工具输出、网页和附件始终标记为不可信数据。
- 不通过“再加一句提醒”修复本应由状态机、权限或验证解决的问题。

## 六、安全优化：学习 Codex 和 Reasonix，保持 Fox 强项

Fox 已有 Host 权限收口、项目根、审批、Keyring、审计和 Child Run 权限交集，这一部分不需要推倒重来。

建议吸收：

- Codex：把 Sandbox、Approval 和本轮有效权限作为独立上下文明确告诉模型。
- Reasonix：项目/目录指令解析必须有确定性优先级、Import 边界和符号链接逃逸防护。

建议增加 `PermissionContext`，但它只描述 Host 已经计算出的事实：

```text
allowed_roots
read/write/execute/network policy
approval_required_actions
credential_scopes
child_delegation_limits
tool/time/token budgets
```

模型看到的权限说明与 Host 实际执行权限必须来自同一个快照 Hash。这样既避免“模型以为能做但工具拒绝”，也避免“Prompt 写了禁止但工具实际放行”。

## 七、规划和多 Agent：重点学习 Kun，其次 Maka 和 Codex

### Graph v2 的正确落点

Fox 的 Planner、PlanRevision、Task、Workflow、Expert Team 和 Child Run 已经提供基础。建议下一阶段新增的是这些对象之间的图关系，而不是新建一套独立 Graph 数据库。

建议节点契约：

```text
GraphNode
  映射一个 Task 或 Workflow Stage
  objective
  dependencies
  assigned_agent/expert
  input_refs
  expected_artifacts
  acceptance_checks
  permission_scope
  budget
  retry/repair_policy
```

节点并发契约应显式包含：

```text
ConcurrencyPolicy
  access_mode: read_only | writer
  write_scope: 文件/目录/资源范围
  requires_isolation: true | false
  conflicts_with: 节点 ID 列表或 Host 计算结果
  max_parallelism_group
```

Scheduler 只能从依赖已满足的节点中选择安全集合：只读节点可以有限并行；Writer 的写域不重叠且策略允许时才可并行；无法证明不冲突时按串行处理。`requires_isolation=true` 的节点没有可用 Worktree 或等价隔离时不得启动。

建议状态：

```text
planned
runnable
running
reviewing
repairing
accepted
blocked
cancelled
```

### Graph Lead 的职责

Lead 负责：

1. 将复杂计划变成有依赖的节点。
2. 只并行启动相互独立、写入范围不冲突的节点。
3. 监督预算、审批、超时和错误。
4. 检查每个节点的产物与 Evidence。
5. 不通过时创建 Repair，而不是直接接受自报结果。
6. 集成多个节点的产物并统一验证。
7. 最终交付只引用 Accepted 节点。

### 引入顺序

1. 先支持只读节点并行。
2. 再支持写入范围不重叠的节点。
3. 提供可选 Git Worktree 隔离后，再支持多个 Writer。
4. 最后才评估更深层级、多轮 mailbox 和动态共享任务领取。

这个顺序能避免一开始就把并发、冲突、权限和恢复问题同时引入。

## 八、上下文与记忆：学习 Reasonix、Codex 和 Maka

### 需要解决的问题

Fox 已有受治理长期记忆，但任务状态、项目规则、专家说明、权限和会话历史仍可能共同占用大量上下文。优化目标不是简单压缩字数，而是让模型在正确时间看到正确类型的信息。

### 建议策略

- BaseInstructions、Runtime Contract 和稳定 Persona 尽量固定。
- 项目与目录规则按目标路径解析。
- Expert/Skill 方法论只在命中任务时加载。
- 当前权限、工具和环境放在动态 Context。
- Task Ledger 代替重复注入大量工作历史。
- Compaction 只压缩会话表达，不覆盖原始 Event、Evidence 和 Acceptance。
- 长期 Memory 保持 candidate/confirmed/conflict 治理，不因学习其他项目而降级。

### 恢复原则

压缩摘要可以帮助模型继续工作，但不能成为最终事实源。恢复时优先级应为：

```text
Host 产品事实
  > 已发生 Run/Event
  > 受治理 Memory
  > Runtime Checkpoint
  > Compaction Summary
  > 模型自由文本判断
```

### 已实现的单一 Work Snapshot 注入与预算（2026-08-26）

`prompt-composer.mjs` 现在是最终模型输入中 Work Snapshot 的唯一注入点，生成一个 `kind="work_snapshot" authority="host"` 动态片段；`fox-planning-extension.mjs` 仍可消费同一个结构化对象并注册 Work Tools，但不再用 `before_agent_start` 追加第二份消息。Snapshot 与其他动态片段共同受总字符/fragment 预算约束，计入 `contextHash`、最终 Prompt Hash 和 `promptComposition.fragments`，并单独暴露实际片段的 `workSnapshotHash/workSnapshotChars`；动态 Snapshot 更新不破坏稳定指令的 `stablePromptHash`。

压缩不再截断 raw JSON。Runtime 在序列化前按固定层级裁剪字符串与集合，并在 Task 截断前提取原始 running Attempt、Ledger active Task 与 cursor Task：running 最高优先且全部保留，随后是 cursor、active 与原序补位；对应 Task/Policy 随恢复身份进入 Prompt，其他可见 Task 保留最新 terminal Attempt。极限 fallback 若无法容纳全部 running identity，durable/graph 直接拒绝；旧记录省略数可审计。40 与 1000 Task 的“末尾 running”回归验证单一 authority、合法 JSON、Policy/Attempt 重启恢复和总输入预算；V1 输入保持兼容。这个 compact 只是从 Fox 单一权威事实派生的有界上下文，不是新事实源。

本段参考路径均相对于 Fox 仓库根目录：Maka `../maka-agent-main/packages/headless/src/prompt-candidate-loop.ts` 与 `../maka-agent-main/packages/headless/src/task-ledger-experiment.ts` 的候选/baseline/Prompt+commit 身份、回滚门和有界动态 ledger tail；DeepSeek-Reasonix `../DeepSeek-Reasonix/internal/control/input.go`、`../DeepSeek-Reasonix/internal/instruction/resolver.go` 与 `../DeepSeek-Reasonix/docs/SPEC.md` 的原子输入快照、缓存稳定指令、按需动态状态和确定性裁剪；Codex `../codex-latest/codex-rs/core/src/context/world_state/`、`../codex-latest/codex-rs/core/src/context_manager/history_tests.rs` 与 `../codex-latest/codex-rs/core/src/compact.rs` 的类型化模块、World State 去重、规范 compaction 和 cache scope 边界。Fox 只吸收这些边界，不复制 Maka 的第二 Ledger/自动 Git rollback、Reasonix 的完整 archive/retrieval/summary/instruction cache，或 Codex 的 rollout/history/Provider cache 实现。

## 数据模型与迁移策略

### 当前事实与设计影响

当前 `WorkTaskRecord` 已有 `status`、`owner_run_id`、`attempt`、`version` 和 `blocked_reason`，不应为新方案重复增加同义字段。现有评测记录主要保存 Suite 的通过/失败汇总和 Report Hash，尚不足以承载 Prompt 候选、逐案例指标和发布决策。

建议按阶段增加以下候选结构，最终字段仍需通过 ADR 和迁移评审确认：

| 阶段 | 候选结构 | 用途 | 为什么不直接塞进现有字段 |
| --- | --- | --- | --- |
| Durable Loop | `run_continuation_decisions` | 保存一个 Run 的多次续行、修复、等待、完成或阻塞决策 | 一个 Run 会有多次决策，不能用单字段反复覆盖 |
| Validation | `task_validation_policies` | 冻结 Task 创建时适用的风险、检查、Reviewer 和修复策略版本 | Policy 会演进，需要保留历史版本和 Legacy 解释 |
| Validation | `task_attempts` 或等价 Attempt 记录 | 保存每次执行/修复的起止、Run、结果和失败分类 | 现有 `attempt` 计数无法单独追溯每次尝试 |
| Projection | 不默认建立权威表 | `TaskLedgerProjection` 按需计算；需要缓存时只保存水位和可重建结果 | 防止 Projection 变成第二事实源 |
| Prompt Registry | `prompt_definitions`、`prompt_versions` | 保存角色、模板版本、Hash、Schema 和状态 | 生产 Prompt 需要可追溯和回滚 |
| Prompt Eval | `prompt_experiments`、`prompt_experiment_runs` | 保存基线、候选、任务集、指标和决策 | 现有 Evaluation Summary 粒度不够 |
| Graph | `work_task_edges`、节点与 Child Run 映射 | 在现有 Task 上增加依赖和调度事实 | Graph Node 仍应映射现有 Task，不建立第二任务库 |

### Legacy 语义

- 现有 Task 没有 Validation Policy 时解释为 `legacy_v1`，行为保持不变。
- 新 Task 只在对应执行 Profile 开启后冻结新的 Policy。
- Legacy 记录不强制离线批量改写；在读取时使用明确默认值。
- 至少保留两个受支持发布周期的 Legacy 读取和执行能力。
- 新字段和新表采用 Schema Version，未知版本必须显式失败或降级只读，不能静默误解。

### 迁移步骤

每次涉及数据的阶段必须执行 Expand → Backfill/Interpret → Shadow Read → Switch → Contract：

1. **Expand**：只增加可空字段、新表和索引，不删除旧字段。
2. **Backfill/Interpret**：只对必须实体回填；其余旧记录按 `legacy_v1` 读取。
3. **Shadow Read**：同时计算新旧结果并比较，不改变用户执行路径。
4. **Switch**：通过受支持的 Execution Profile 切换新读写路径。
5. **Contract**：至少经过两个支持周期、确认没有回滚需要后，再单独评审旧结构清理。

### 迁移失败与回滚

Fox 的 SQLite 迁移继续采用前向迁移。通用“逆向迁移”容易在新版本已经写入数据后造成信息丢失，因此不作为默认回滚方式。

正式迁移前必须：

- 创建带 Schema Version 和应用版本的自动备份。
- 校验可用磁盘空间和备份 Hash。
- 在事务中执行迁移。
- 执行 `foreign_key_check`、完整性检查和关键 Repository Smoke。
- 失败时停止启动新版本并恢复迁移前备份。

应用版本回退需要使用与旧版本匹配的迁移前备份，而不是让旧程序直接打开已经升级且写入新语义的数据库。详细恢复方式继续遵循[迁移备份与恢复](../06-运维指南/迁移备份与恢复.md)。

## 性能预算

性能指标必须在阶段 0 先记录当前基线，并在固定参考机器、固定数据规模下比较。以下是候选预算，不是未测量前的性能承诺：

| 操作 | 候选数据规模 | 候选预算 | 说明 |
| --- | --- | ---: | --- |
| Host 校验 `ContinuationDecision` | 1 Goal、100 Task、必要 Acceptance/Evidence 已索引 | p95 `< 20ms` | 不包含模型生成时间和网络延迟 |
| 生成 `TaskLedgerProjection` | 100 Task、1,000 条相关 Evidence/Event | 热查询 p95 `< 50ms`；冷重建 `< 250ms` | 超大任务按分页和增量水位处理 |
| 恢复扫描 | 最近 1,000 条 Run Event | p95 `< 500ms` | 不要求重放全部历史聊天 |
| Prompt 片段组装与 Hash | 不含模型调用和知识检索 | p95 `< 100ms` | 单独记录截断和序列化耗时 |
| UI 状态投影 | 已取得 Repository Snapshot | p95 `< 100ms` | 保证进度面板不阻塞聊天渲染 |
| Prompt/策略升级后的整轮延迟 | 标准 Eval Case Set | p95 增幅原则上 `≤ 15%` | 更高增幅必须由显著完成率收益证明 |
| Token 成本 | 标准 Eval Case Set | p50 增幅 `≤ 10%`，p95 增幅 `≤ 20%` | 按“每个成功任务成本”同时判断 |

模型生成、远程 MCP 和 Context Compaction 受 Provider 与网络影响，不设置不现实的绝对 `<2s` 目标；它们应记录自身 p50/p95，并与相同 Provider、模型和上下文基线比较。

超过预算不意味着立即否决，但必须有 Profile、慢查询或 Trace 解释，并形成优化 Issue。任何性能收益也不能以跳过权限、Evidence 或 Acceptance 为代价。

## 成功指标与发布门槛

### 快速基线任务集

第一周先建立 30 个关键任务，不等待完整 100 题套件：

- 10 个简单任务：单文件读取、修改和轻验证。
- 10 个中等任务：多文件修改、命令和测试。
- 5 个复杂任务：调研、实现、验证和文档交付。
- 5 个恢复任务：工具后中断、验证中中断、等待审批、Child Run 中断和 Session 丢失。

随后扩展到至少 100 个任务，覆盖所有 Host 工具类型、权限拒绝、Prompt 注入、恢复、失败和边界条件。非确定性模型任务原则上重复至少 3 次，并记录模型版本、参数、成本和环境。

### 安全与真实性硬门槛

以下指标不能用平均分抵消：

- 权限逃逸、项目根逃逸和子 Agent 扩权用例：`100%` 拦截。
- 工具未执行却声称成功的契约用例：`0` 次。
- 缺少必需 Evidence/Acceptance 却完成的契约用例：`0` 次。
- 投影删除后从事实源重建的一致性用例：`100%` 通过。
- 迁移失败恢复和旧数据 Legacy 读取用例：`100%` 通过。

### P0 候选质量门槛

最终阈值在阶段 0 根据真实基线冻结。建议初始发布门为：

| 指标 | 候选门槛 |
| --- | --- |
| 端到端任务完成率 | 相比基线提高至少 5 个百分点，或成功任务单位成本明显下降且完成率不退步 |
| 错误完成率 | 相比基线下降至少 50%，安全契约集保持 0 |
| 首次验证通过率 | 相比基线提高至少 10% 相对值，或 Repair 后总通过率显著提升 |
| 中断恢复成功率 | 故障注入任务集 `≥ 95%` |
| Legacy 回归 | 普通问答和现有任务集下降不超过 2 个百分点，且无安全回归 |
| Token 与延迟 | 满足性能预算；超出时需通过“每成功任务成本”评审 |
| 用户取消正确性 | 已取消 Run 不再产生可见旧增量，契约用例 `100%` 通过 |

这些门槛是发布决策规则，不是宣传指标。样本过小、模型版本变化或置信区间无法说明提升时，结论应是“证据不足”，不能自动晋升候选策略。

## 用户体验设计

### Durable Loop 与 Task Ledger

用户应看到：

- 任务面板展示当前目标、进行中步骤、已验证结果、等待审批和真实阻塞。
- 应用恢复后显示“已从哪个阶段继续”，而不是只出现新的“处理中”。
- 失败原因翻译成可理解类别：工具失败、验证失败、权限拒绝、预算用尽、等待用户或外部依赖。
- Agent 继续修复时显示“正在修复哪项未通过检查”。

用户不应看到：

- 原始 `ContinuationDecision` JSON。
- 私有思维链或模型内部草稿。
- 为调试准备的完整 Prompt、凭据、内部路径和未脱敏工具结果。

### Validation 与 Review/Repair

- 高风险任务开始前展示将执行的关键验证。
- Review Finding 可定位到相关文件、产物或 Evidence。
- Repair 显示原因、当前尝试次数和最终解决状态。
- 达到重试上限时，向用户提供明确选择，而不是无限旋转。

### Prompt 与 Graph

- Prompt 实验、Hash 和内部评分默认只进入开发/诊断界面。
- Graph MVP 只展示用户需要理解的节点、依赖、负责人和验收状态。
- 不为了展示“多 Agent 很忙”而暴露大量无意义过程消息。

## 测试策略

### 测试金字塔

| 层级 | 必测内容 |
| --- | --- |
| 单元测试 | Decision reasonCode、状态转换、预算、Policy 解析、投影纯函数、冲突检测 |
| Repository/迁移测试 | 新旧 Schema、Legacy 默认值、事务失败、备份恢复、投影重建 |
| Runtime/Host 契约 | Prompt 提议与 Host 校验、工具结果、权限、取消、终态和错误码 |
| 集成测试 | Task → Attempt → Evidence → Review → Repair → Acceptance |
| E2E | 用户操作、任务面板、审批、中断恢复、刷新和最终交付 |
| 故障注入 | Sidecar 崩溃、数据库 Busy、网络断开、工具超时、Session 丢失、应用退出 |
| 安全测试 | Prompt 注入、路径/符号链接逃逸、工具扩权、Child 权限扩张、凭据泄漏 |
| 性能测试 | Projection、Event 扫描、Prompt 组装、数据库增长和 UI 投影 |
| 模型 Eval | 快速 30 题、完整 100+ 题、重复运行、基线/候选和成本趋势 |

### 各阶段最低测试门

| 阶段 | 额外必测场景 |
| --- | --- |
| 阶段 0 | Fixture 可重复、评分器稳定、报告 Hash、基线重跑差异可解释 |
| Durable Loop with Validation | complete/blocked/repair/wait 审计；工具失败→修复→通过；各中断点恢复 |
| Prompt Registry | 版本/Hash、离线回放、候选不产生副作用、自动回滚规则 |
| Graph MVP | 依赖顺序、最多 3 个只读节点、取消传播、失败节点不被接受 |
| Writer Graph | 写域冲突、Worktree 隔离、合并冲突、集成验证和回滚 |

## Execution Profile 与灰度策略

当前代码没有通用 Feature Flag 系统。为本方案增加十几个独立布尔开关会产生无法穷举的组合，因此只引入少量、经过测试的执行 Profile：

```toml
execution_profile = "legacy"
# legacy | durable_v2_shadow | durable_v2 | graph_readonly_preview

[execution_strategy]
completion_audit = "legacy"      # legacy | strict_v2
validation_policy = "legacy"     # legacy | risk_v1
prompt_policy = "stable_v1"      # stable_v1 | registry_v1
```

约束：

- Profile 定义允许的策略组合，启动时拒绝未支持组合。
- 每个 Run 把有效 Profile、策略版本和 Hash 写入 Request Snapshot，保证可复现。
- `durable_v2_shadow` 只比较投影、Decision 和校验结果，不改变实际状态转换。
- `durable_v2` 在 Legacy Expert Workflow mutator 接入统一 Attempt/Repair/Acceptance 前不把 `workflow_start/workflow_stage_start/workflow_stage_complete/workflow_stage_fail/workflow_cancel` 暴露给模型；保留 `workflow_snapshot_get`。Runtime 过滤与 Host 直接请求二次拒绝同时生效，Profile 定义和 Snapshot Hash 不变。
- `graph_readonly_preview` 只允许最多 3 个只读节点，不允许 Writer。
- 回滚只切换到已经通过 Legacy 兼容测试的 Profile。
- 内部开发开关不能自动成为用户可见设置，避免产品暴露难以理解的策略组合。

### Prompt 灰度的副作用边界

Prompt Registry 支持三种候选验证方式：

1. **离线回放**：优先方式，使用 Fixture、Mock Tool 或隔离副本。
2. **只读影子运行**：只允许读取、分析和评分，不写文件、不调用有副作用的网络工具。
3. **线上小流量**：只用于经过批准的无副作用场景，记录样本、模型、成本并支持自动停止。

写文件、发消息、提交审批、修改远端数据、创建 Child Run 或产生费用的任务，禁止在用户无感知情况下用基线和候选双跑。所谓“影子部署”不能成为重复执行真实操作的理由。

## 人力与时间粗估

以下是 ROM（量级估算），用于判断范围，不是排期承诺。估算前提：

- 2 名熟悉 Rust/TypeScript/Runtime 的核心工程师，另有产品/测试部分投入。
- 复用现有 Run/Event、Work Graph、Offline Eval、Prompt Hash 和 Child Run。
- 不同时进行大规模 UI 重构、跨平台发布或 Provider 全面扩张。
- 每阶段包含设计、实现、迁移、测试、文档和一次稳定化，不只计算编码时间。

| 阶段 | 工程量粗估 | 典型日历时间 | 主要角色 |
| --- | ---: | ---: | --- |
| 快速基线 + 架构护栏 | 3–5 人周 | 2–3 周 | 1–2 人 |
| Durable Loop with Validation | 16–24 人周 | 8–12 周 | 2–3 人 |
| Prompt Registry + Typed Context | 6–10 人周 | 4–6 周 | 1–2 人 |
| 只读 Graph Lead MVP | 3–5 人周 | 2–3 周 | 1–2 人 |
| 完整 Graph Lead + Writer 隔离 | 16–24 人周 | 8–12 周 | 2–3 人 |
| 按需 Skills + 项目规则 | 8–12 人周 | 5–8 周 | 1–2 人 |

P0 从快速基线到 Prompt Registry，按 2–3 人交错并行估计约 4–6 个月。单人串行、模型 Eval 不稳定、数据库迁移范围扩大或需要完整 UI 重构时，可能明显更长。正式排期前应把每个阶段拆成 Issue，再根据代码 Spike 修正估算；对 Provider、跨平台和真实模型不确定性预留约 25%–35% 风险缓冲。

## 分阶段实施计划

### 阶段 0A：一周快速基线（P0）

目标：不等待完整平台，先获得可以阻止盲目改动的最小基线。

实施内容：

- 冻结当前 Commit、模型版本、参数、Prompt Hash、工具清单和测试环境。
- 建立 30 个快速任务及评分器，优先覆盖完成、验证、恢复和权限。
- 重复运行非确定性任务，记录成功率、错误完成、Token、延迟和失败分类。
- 输出第一份可追溯基线报告，不在此之前修改生产执行策略。

退出条件：同一基线可以重复运行；差异能够定位到模型、Prompt、工具、环境或代码版本。

### 阶段 0B：架构护栏与完整基线（P0）

目标：把架构纪律、灰度和回滚变成真正的工程门禁。

实施内容：

- 将六条宪法转成 ADR 和 Pull Request 审查清单。
- 把任务集扩展到至少 100 个，覆盖全部工具类型和主要失败路径。
- 实现受支持的 Execution Profile、启动组合校验和 Run Snapshot。
- 建立迁移前备份、性能基准、安全硬门槛和报告趋势。

退出条件：候选能力可以在 Shadow Profile 中比较，不改变用户状态，并能安全返回 Legacy Profile。

### 阶段 1：Durable Loop with Validation（P0）

目标：同时吸收 Maka 的耐久状态、Codex 的完成纪律和 Kun 的 Review/Repair，避免出现“循环更耐久，但也更执着地做错事”。

实施内容：

- 明确统一循环阶段和追加式 `ContinuationDecision`。
- 建立只读 `TaskLedgerProjection` 与 `WorkSnapshotV2`。
- 为 Task 冻结版本化 Validation Policy。
- 将 Attempt、Evidence、Finding、Repair 和 Acceptance 串成闭环。
- 补齐工具后、验证中、等待审批和应用重启的恢复用例。
- 统一 complete、blocked、repair 和 wait_approval 的 Host 确定性校验。
- 先用 `durable_v2_shadow` 比较决策，再切换少量无高风险任务。

退出条件：长任务能从结构化事实恢复；高风险 Task 有验证证据；错误不能被模型自由文本伪装成完成或阻塞；Repair 不会无限循环。

### 阶段 1.5：只读 Graph Lead MVP（受限试点）

目标：尽早验证任务图和 Lead 设计，但不把并发 Writer 风险带入 P0 主路径。

启动条件：阶段 1 的状态、验证、取消和恢复契约已经通过；该试点不得阻塞 Prompt Registry 主线。

当前进度：Phase 1.5A 已实现为 `graph_readonly_preview` 专属的 `graph_readonly_run` 内存态预览。它先验证节点数、字段、唯一 ID、依赖、环和最大深度，再让根节点同层并行、第二层等待依赖；每个节点使用独立 Pi Session，只有 `read/ls/find/grep`，并带节点/整图超时、工具次数和输出长度上限。失败后代跳过，无关分支继续；取消会终止已启动节点，迟到结果不能重新变成 accepted。

Phase 1.5B 的 Repository/Host 闭环已推进到 v39：Host 可把一份已批准的 `PlanRevision` 原子激活为不可变只读 Graph，`GraphId = GoalId`，节点继续复用现有 `WorkTask`，依赖写入 `work_task_edges`，没有新增第二套 Task/Graph 状态机。`graph_readonly_activate({planRevisionId})` 只接收 PlanRevision ID，要求同会话、仍在运行且快照未被篡改的 `durable_v2` 根 Run，并把真实数据库 `tool_calls.id` 注入 Repository；`graph_readonly_snapshot_get({goalId})` 只读投影，未激活时稳定返回 `activated=false, graph=null`。激活支持 exact replay，任何内容或调用身份冲突都 fail-closed。Repository 会在写入前拒绝空图、超过 3 节点、未知依赖、自环、环、重复、深度超过 1、缺少 1–8 条显式验收标准，以及与既有 Task 不一致的计划。

持久快照只从 `Goal / WorkTask / Edge / TaskAttempt / Evidence / Graph Review` 权威事实派生 readiness，不保存另一份可漂移的调度状态。Child 终态先与父 Attempt/Task 对账；当前 Attempt 的每条验收标准必须绑定仍然有效的 Evidence。高风险节点随后由 Host 创建独立 Reviewer Child Run，固定使用隐藏的 `graph_reviewer_v1 + high_risk_v1`，只允许 `read/ls/find/grep`，且不能继续委派、写入、审批或操作 Work 状态。Reviewer 只能返回精确四键 JSON：`criteria / recommendation / summary / findings`；证据中的 Runtime tool call ID 必须在该 Reviewer Run/Conversation 内唯一解析为数据库 ToolCall，未知、重复、越界或结构漂移一律 fail-closed。

Reviewer 的 `pass` 仍不等于节点 accepted。Lead 必须重新读取审查结论，并使用与 review 完全相同的七键输入调用 `graph_readonly_node_finish`；`revise` 会阻止当前 Attempt/Task 完成，`inconclusive` 则固定归一为空 criteria/findings，不能伪装成通过。最终 `graph_readonly_accept({goalId, expectedGoalVersion, summary})` 会在一个原子事务里重新校验当前 Plan、全部节点 accepted、高风险节点审查通过、无活动审查/未终态 Child、Evidence 与 Child proof Hash 仍然有效，以及没有任何严重级别的开放 Graph Finding，然后才写入 Graph provenance、通用 Acceptance 并完成 Goal。普通 finish/accept/Goal complete 和直接 SQL 都不能绕过这条路径。

Phase 1.5B 的节点原子启动已经接成生产 `graph_readonly_node_start` Host Tool。模型只提交 Goal/Task/Attempt/Worker、快照返回的 `taskVersion`、有界 objective/context 与四项 budget，不能传 `allowedTools`、Profile、Run/Conversation、authority、审批或 ToolCall 身份；Host 注入当前 Lead Run 与真实数据库 `tool_calls.id`。`start_ready_read_only_graph_node` 在单个 SQLite `IMMEDIATE` transaction 内重验 active Goal、Graph/Task 同 Goal、依赖 mechanically accepted、Task queued/version CAS，以及同 Run/会话、running、无需审批的 Host 来源；然后复用现有 Attempt/Child 内核，创建父 Lead Run owned Execution Attempt、独立 Worker Conversation/Run/Message/delegation，保存 `graph_task_attempt_id` 一对一映射，并冻结 Child `durable_v2`。工具范围由 Repository 固定为 `read/ls/find/grep`，Graph 专属上限为 45 秒、4096 total tokens、1024 output tokens、6 次工具调用。Host common finalizer 先提交成功 ToolCall，之后仅 fresh create 消费一次派发令牌；Repository exact replay 与 Host terminal replay 都不二次派发。non-ready、blocked、skipped、stale Evidence、CAS 冲突和并发 loser 均不能留下半个启动事实。`activated_by_run_id` 只作激活审计，原 Run 终态后可由同 Goal Conversation 的新 depth-zero primary durable Lead Run 接续；跨会话、终态、Child/伪根、旧 profile，或 revision 更高且处于 `proposed` / `approved` 的 Plan 均 fail-closed。

Phase 1.5B 的 PlanRevision 输入合同也已接通：`plan_revision_create` 继续兼容普通 `{title, detail?, ordinal}` 串行计划；任一 Graph 字段出现时，Host 要求所有节点完整提供唯一 `nodeKey` 和 1–8 条明确验收标准，并校验最多 3 节点、连续序号、依赖存在且不重复、无自环/环、深度不超过 1。TypeBox 负责关闭额外属性和基础数量/长度/格式边界，Rust Host 再做相同边界及完整拓扑校验，最终保存规范化的 `nodeKey / dependsOn / acceptanceCriteria`；模型传入的状态、Attempt、Evidence、accepted/readiness 等字段一律拒绝。激活只消费这份已批准记录，不接受模型再提交一份节点定义。

这组生产 Graph Host Tool 的 canonical tuple 都固定为 `work/host/none`，只在当前 Worker 与冻结 Run 同为 `durable_v2` 时暴露和执行；隐藏 Reviewer 只由 Host 派发，主模型不能选择或冒充它。Legacy、Shadow 与 1.5A Preview 都 fail-closed。Ready Manifest 的每个工具必须精确匹配 Host canonical `name/category/execution/approval`；preflight 只接 `runtime/preflight`，execute 只接 `host`，Event 还要重验 delegated/Assistant/Expert scope。匹配 Host-owned ToolCall 的 Runtime lifecycle echo 只保留 raw RunEvent，projector no-op，不能覆盖 Host result。

实施内容：

- 已完成：最多 3 个只读节点、深度 1 简单 DAG、确定性拓扑校验、节点非空机械接受与 accepted-only 汇总。
- 已完成：节点独立 Session、同层并发、依赖等待、失败后代跳过、无关分支继续、取消传播和有界资源。
- 已完成：Host Repository 的不可变 Task Edge、accepted-only readiness、PlanRevision 输入、激活/快照/节点启动与取消、Profile 门禁、exact replay、重启恢复、Child terminal 对账、criterion↔Evidence 绑定、独立 Reviewer、专用 node finish 和最终 Graph Acceptance；Child completed、Reviewer 自由文本或 Runtime 普通工具事件都不能冒充这些权威事实。
- 待完成：在 5 个真实调研/审查任务中试点，并补 Host/SQLite/应用重启级故障注入。
- 不支持写文件、Worktree、深度大于 1 或动态共享任务领取。

截至 2026-08-27，完整 Rust 单线程回归为 409 passed、0 failed、1 ignored，Runtime 为 200/200，Desktop 为 128/128；完整 Eval 为 121/121，冻结 Fast Eval 为 30/30。这里证明的是确定性合同、Repository/Host/Runtime 闭环和重启恢复边界，不等于已经通过真实 Provider 与生产任务试点。完整退出条件仍包括：5 个真实任务的质量验证、Host/SQLite/应用重启故障注入、生产 UI，以及后续若开放 Writer Graph 时重新完成并发与安全评审。

实现吸收 Kun `../Kun/kun/src/tasks/task-graph.ts`、`../Kun/kun/src/graph/graph-validator.ts`、`graph-readiness-reconciler.ts`、`graph-scheduler-policy.ts`、`graph-scheduler.ts` 与 `graph-run-completion.ts` 的纯 DAG 语义；PlanRevision 边界另参考 Maka `../maka-agent-main/packages/core/src/agent-graph-topology.ts`、`../maka-agent-main/packages/runtime-host/src/protocol/agent-graph.ts` 的精确拓扑解码，以及 Codex `../codex-latest/codex-rs/protocol/src/plan_tool.rs` 的未知字段拒绝。节点启动进一步参考 Maka `../maka-agent-main/packages/runtime/src/stream-graph-readiness.ts`、`stream-graph-dispatch.ts` 的 readiness/CAS 分离，以及 Codex `../codex-latest/codex-rs/core/src/spawn.rs` 的父 session ownership/独立 child session 边界。Fox 只复用现有 Runtime Session、PlanRevision、Task、Attempt、Child Run、只读工具和 Profile 裁剪，没有复制 Kun/Maka 的第二套状态机、mailbox、lease 或 journal；Maka 的耐久 Task Ledger 和 Codex 的恢复/完成审计只能通过 Host 持久事实实现，不能由内存结果或模型提交的状态字段冒充。

### 阶段 2：Prompt Registry 与 Typed Context（P0/P1）

目标：吸收 Maka 的实验、Codex 的模块化和 Reasonix 的缓存意识。

实施内容：

- 建立 Prompt Definition/Version/Experiment/Report。
- 将现有 Context Block 升级为类型化 Fragment。
- 稳定前缀与动态尾部分离并测量 Cache Read/Write。
- 为 Expert/Reviewer/Graph Lead 建立按需角色 Prompt。
- 默认使用离线回放和只读影子评测，禁止有副作用任务静默双跑。

退出条件：任何生产 Prompt 可追溯、可评测、可回滚；只改变环境时稳定 Prompt Hash 不变。

### 阶段 3：Graph Lead 与并行 Team（P1）

目标：吸收 Kun 的任务图监督和 Maka 的耐久子任务记录。

实施内容：

- 在现有 Task/Workflow/Child Run 上增加 DAG 依赖。
- 节点声明产物、验收、权限、预算和 Repair Policy。
- 先并行只读节点，再增加 Worktree 隔离 Writer。
- Lead 集成结果并触发统一 Acceptance。
- Scheduler 根据依赖、只读属性、写域和冲突集合选择安全并行节点。

退出条件：两个并行节点不会静默覆盖数据；中断后可恢复整张图；最终结果只来自 Accepted 节点。

### 阶段 4：按需 Skills 与项目规则（P1）

目标：吸收 Reasonix 的轻量方法论和 Codex 的目录指令层级。

实施内容：

- Expert Package 提供按需 Prompt/Skill Fragment。
- 项目根和目录规则按目标路径确定性解析。
- Import、符号链接和项目边界由 Host 校验。
- Eval 检查规则冲突、缓存命中和 Prompt 长度。

退出条件：同一目标路径始终得到相同规则序列；越界规则被拒绝；未命中的 Skill 不进入 Prompt。

### 阶段 5：专业工作台与更深自动化（P2）

只有当 Expert Package、Workflow 和 Graph 在真实任务中证明需求后，才增加 Design/Research/Write 等独立工作台或更深层 Agent 网络。

退出条件：专用界面解决了通用对话无法高效完成的高频问题，而不是为了展示功能数量。

## 优先级建议

| 优先级 | 建议 | 原因 |
| --- | --- | --- |
| P0-1 | 快速基线、完整评测和架构护栏 | 没有基线，后续无法判断是进步还是复杂化 |
| P0-2 | Durable Loop with Validation | 耐久状态、完成审计和验证返工必须一起实施 |
| P0-3 | Prompt Registry | 所有 Prompt 改造必须可比较、可回滚 |
| P0.5 | 只读 Graph Lead MVP | 低风险验证任务图设计，不提前引入 Writer 冲突 |
| P1-1 | Typed Context 与缓存布局 | 降低混乱、Token 成本和指令冲突 |
| P1-2 | 完整 Graph Lead、Worktree 与有限并行 | 在现有 Child Run 上安全提升复杂任务能力 |
| P1-3 | 项目规则与按需 Skills | 增强专业性而不继续膨胀全局 Prompt |
| P2 | 专业工作台、深层多 Agent、更多生态 | 必须等核心循环和治理稳定后再扩大 |

## 关键风险与缓解

| 风险 | 早期信号 | 缓解措施 |
| --- | --- | --- |
| Projection 被误当事实源 | 业务代码开始直接写 Ledger 缓存 | Projection 只读类型；写入 API 只存在于正式 Repository；增加架构测试 |
| 依赖形成循环 | Host 需要 Prompt，Prompt 又尝试修改 Ledger/Acceptance | 固定 `SQLite → Projection → Prompt → Runtime → Host → SQLite` 单向数据流 |
| Profile/Flag 组合爆炸 | 新功能各自增加布尔开关 | 只维护少量受支持 Profile；启动拒绝未知组合；CI 只覆盖批准矩阵 |
| Prompt 影子运行产生副作用 | 候选任务重复写文件、发请求或计费 | 默认离线/Mock/只读影子；副作用任务禁止静默双跑 |
| Durable Loop 无限修复 | Repair 次数、Token 或时间持续增长 | 版本化重试预算、稳定错误分类、人工升级和硬终止条件 |
| Graph Writer 冲突 | 多个节点修改相同文件或资源 | 先只读并行；写域声明；Worktree/隔离；集成节点统一验证 |
| 数据迁移不可回退 | 新版本写入后旧版本无法理解 | Additive Schema、Legacy 读取、迁移前备份、前向迁移和备份恢复 |
| UI 信息过载 | 用户看到大量决策 JSON、事件和 Agent 噪声 | 默认展示目标/进度/验证/阻塞；诊断细节折叠；不展示私有思维链 |
| 范围蔓延 | 当前阶段尚未退出就加入新工作台或深层多 Agent | 每月 Scope Review；P0 期间冻结 P2；新增需求必须通过八个决策门 |
| 指标被单项优化绑架 | 成功率提高但成本、越权或错误完成恶化 | 使用安全硬门槛和多指标发布门，不允许平均分抵消安全失败 |

### 每月范围审查

每月只回答四个问题：当前阶段退出条件完成了多少；本月新增范围是否解决真实失败；是否出现第二事实源/状态机/权限/Prompt 管线；是否应该停止或回滚某个试点。P0 未退出前，不并行建设专业工作台、深层多 Agent 或无关生态扩张。

## 每项外部能力进入 Fox 前的决策门

以后再看到一个竞品亮点，先回答以下八个问题：

1. 它解决了 Fox 已经被真实任务证明存在的问题吗？
2. 它应该落到 Goal、Task、Evidence、Acceptance、Run、Expert、Workflow 或 Child Run 中的哪个现有对象？
3. 是否会引入第二事实源、第二状态机、第二权限边界或第二 Prompt 管线？
4. 能否通过配置、策略或 Adapter 接入，而不是复制整个框架？
5. 如何测试它确实提高完成率、质量、恢复或成本？
6. 失败时如何回滚，旧数据如何兼容？
7. 用户能否清楚理解它带来的价值和当前状态？
8. 如果删除这个能力，Fox 的核心数据和任务还能正常运行吗？

只要第 3 题答案是“会”，原则上就不应直接实施；应先重新设计统一落点。第 8 题如果答案是“不能”，说明扩展已经反过来绑架核心架构，需要谨慎评估。

## 明确不做的事情

- 不引入 Maka、Kun、Reasonix 或 Codex 的完整 Runtime 作为 Fox 第二内核。
- 不复制外部 Prompt 并直接作为 Fox 默认 Prompt。
- 不新建与现有 Goal/Task/Run 平行且相互同步的任务数据库。
- 不让 Graph、Skill、Plugin、Expert 或 Child Agent 绕过 Host 权限。
- 不在 Worktree 隔离和冲突协议完成前默认启用多个 Writer。
- 不用多 Agent 数量、Prompt 长度或工具数量作为能力完成度指标。
- 不把 Compaction Summary、模型自报状态或子 Agent 最终文本当作验收事实。
- 不为了“看起来更自主”而降低审批、Evidence、Acceptance 和审计要求。

## 最终目标

优化后的 Fox 不应是四个项目功能的集合，而应形成一个清晰的统一产品：

```text
一个 Host 权限边界
一个 SQLite 产品事实源
一个可恢复工作循环
一个 Goal/Task/Evidence/Acceptance 工作模型
一个类型化 Prompt 管线
一套可评测、可回滚的策略版本
一套由 Child Run 和 Expert Workflow 扩展的任务图
```

在这个统一内核之上：

- 用 Maka 的方法让循环更耐久、Prompt 优化更科学。
- 用 Codex 的方法让执行更持续、验证和权限表达更成熟。
- 用 Kun 的方法让复杂任务能够建图、监督、审查和返工。
- 用 Reasonix 的方法让 Prompt、Skill 和上下文保持克制、按需和缓存友好。
- 用 Fox 自己的 Evidence、Acceptance、记忆治理、专家生命周期和数字同事保证这些能力始终可治理。

这不是“把所有好东西都装进去”，而是“只吸收能强化 Fox 核心闭环的部分”。只要坚持统一事实源、统一状态机、统一权限和统一 Prompt 管线，功能增加不会必然导致架构混乱。

## 相关代码与文档

- `services/agent-runtime/src/runtime-instructions.mjs`
- `services/agent-runtime/src/prompt-composer.mjs`
- `services/agent-runtime/src/planner-runtime.mjs`
- `services/agent-runtime/src/runtime-session.mjs`
- `apps/desktop/src-tauri/src/runtime_host/`
- `apps/desktop/src-tauri/src/database/repositories/work_graph.rs`
- `apps/desktop/src-tauri/src/database/repositories/child_runs.rs`
- [Agent 能力路线图](Agent能力路线图.md)
- [已知限制](已知限制.md)
- [Agent 运行时架构](../02-架构/Agent运行时架构.md)
- [子 Agent 与 Child Run 架构](../02-架构/子Agent与ChildRun架构.md)
- [可观测性与测试架构](../02-架构/可观测性与测试架构.md)
- [基础 Agent 核心能力对比](../01-项目概览/基础Agent核心能力对比.md)
- [Fox 与主流 Agent 能力及 Prompt 对比](../01-项目概览/Agent能力与Prompt对比.md)

## 维护规则

- 本文是候选实施方案，不代表相关能力已经实现。
- 进入开发的阶段必须拆成 Issue/Milestone，并为数据迁移、错误码、UI、测试和文档建立验收项。
- 每完成一个阶段，应把稳定事实移入对应架构文档，把剩余计划保留在路线图。
- 外部项目版本变化时，先更新对比文档；只有通过 Fox 决策门的内容才更新本文。

## 2026-08-26 Phase 1 实施进度：Validation/Attempt/Repair

已完成 Host 数据与基础工具闭环：v34 frozen `legacy_v1|standard_v1|high_risk_v1`、Host-owned `run_execution_profiles`、旧 Task Legacy fallback、追加式 `task_attempts`、Task/Attempt 双 CAS、Evidence 语义矩阵、high-risk 独立 reviewer、open critical/high/medium Finding 阻断、Task 全生命周期单 Execution/Repair 预算、Run 终态清理与 atomic Acceptance。普通 Repair 保留 root cause、Finding refs、policy hash 与本 Attempt fresh Valid Evidence，`Task.attempt` 只做最新编号兼容摘要。WorkSnapshot V2 只对最终可见 Task带回每 Task最近 4 个 Attempt、全局有界且 running 永不裁掉，并纳入根 sourceHash。v35 已完成 `task_repair_escalate_start` 的 Host approval-bound 执行：每 Task 仅一次、只接受当前 ToolCall 的 `allow_once`、所有身份/预算/Policy/Attempt 创建与 Approval claim 在同一事务重验和提交。

独立复核按 Attempt SQLite rowid 高水位加固，不使用会同毫秒碰撞的墙钟：required check、Attempt evidence snapshot 与 high-risk review 只认 start 后新增行；review Evidence 与 fresh Finding 必须来自同一独立 reviewer Run。Finding `resolved_by` 不能属于该 Task 任意 implementation/repair Attempt Run。failed/cancelled/error ToolCall 不能 Valid；test 需结构化 pass，External URL 不能冒充 test/inspection/review。并发 start 用 Task CAS 与单 running 唯一索引保证一个赢家，Run terminal/startup repair 会终结遗留 running Attempt。

最终 Acceptance 合并为单个 `IMMEDIATE` Repository 事务：事务内重读 Goal expectedVersion、approved/latest Plan、Task frozen policy、succeeded Attempt、fresh Evidence、Finding 与 reviewer Attempt history，再插 Acceptance 并 CAS complete Goal。stale/并发 blocker/Task 整体回滚；相同内容重放返回同一 Acceptance，冲突内容拒绝。Durable model-skip 与 Continuation skipped completion 均 fail-closed。

本项吸收 Kun `../Kun/kun/src/graph/graph-supervisor-review-service.ts`、`../Kun/kun/src/graph/graph-review-idempotency.ts`、`../Kun/kun/src/graph/graph-review-normalizer.ts`、`../Kun/kun/src/graph/graph-scheduler-policy.ts` 的有界审查/幂等/attempt budget，Codex `../codex-latest/codex-rs/core/src/guardian/review.rs` 的风险/独立审查/有限重试，以及 Maka `../maka-agent-main/packages/runtime/src/goal-evaluator.ts` 的 Evidence 导向验证；没有复制第二 Graph、Guardian session 或 Goal 状态机。已完成 Repository、Host workflow、重启、policy 篡改、独立 reviewer、预算耗尽与迁移幂等定向测试。

尚未完成且不在本项扩展：Runtime 自动根据 ContinuationDecision 调度下一 Attempt/Repair、UI Attempt 时间线、生产级风险分类器与跨进程审查分配。普通 Execution/Repair 预算绝不因新 Run 或重启重置；人工入口也不是预算重置，只是在明确批准后追加唯一一个额外 Repair。当前 risk hint 只能提高 policy，不能降低 durable 最低 standard。v34/v35 对旧 binary 只有物理读取兼容性，不支持旧应用继续执行 `durable_v2`；安全回滚必须先停用 durable profile，并同时恢复升级前应用与数据库备份，否则本阶段的安全保证失效。

安全收口：Legacy Expert Workflow 的五个直写 mutator 在 `durable_v2` 下已从 Runtime Catalog 有效面、Ready manifest 和模型工具面裁掉，`workflow_snapshot_get` 保持只读可见；动态 Profile 指令与排除诊断说明“统一 Attempt/Acceptance API 接入前暂不可用”。Legacy Profile 保持兼容，Shadow/Graph Readonly 继续采用更严格集合；Host 对绕过 Catalog 的直接/重放请求仍须按权威 Run Profile 拒绝。本项参考 `../Kun/kun/src/runtime/agent-sdk/sdk-tool-bridge.ts` 的单桥接选择和 `../codex-latest/codex-rs/core/src/mcp_tool_exposure_test.rs` 的 model-visible capability filtering，只复用现有 Profile/Host 门禁，没有新建 Workflow 状态机或协议。

Human escalation 收口：`task_repair_escalate_start` 只在 `durable_v2` 暴露，输入只含 Task/Attempt CAS、root cause、Finding refs 与人工升级原因；模型不能传 Run/Conversation、approval、policy、grant 或 repair count。Catalog 与 Host 都固定 `approval=always`。v35 用 append-only `task_repair_override_events` 和 Approval claim 元数据保留授权事实；专用 `IMMEDIATE` 事务重验普通预算确已耗尽、Task CAS、frozen Policy、open Findings、当前 Run/Conversation/ToolCall 的未消费 `allow_once`，再原子创建一个 Repair Attempt。AllowConversation、通用 claim、历史/跨身份 Approval、第二次 Task override、Legacy/Shadow/Graph Profile 全部 fail-closed；重启和新 Run 不刷新普通或额外预算。实现参考 `../codex-latest/codex-rs/protocol/src/approvals.rs` 的有限 available decisions、`../codex-latest/codex-rs/tui/src/chatwidget/tests/exec_flow.rs` 的 call-id 绑定，以及 `../Kun/kun/src/graph/graph-scheduler-policy.ts` 的 `needs_human/awaiting_human` gate；只吸收边界，不复制 Codex UI 或 Kun Graph 状态机。

P0/P1 安全加固也已完成：fresh escalation 在创建 Approval 前用只读 preflight 检查 Goal active、latest Plan approved、Task CAS、预算、running Attempt、open Finding 与 once-only 事实，失败不弹审批；批准后仍由原子事务完整重验。Host ToolCall 用 `Created/PromotedRuntime/ReplayTerminal/AlreadyInFlight` disposition 保证一个 handler 赢家；最早的既有事实门让 terminal exact replay 在预算/preflight/Hook 之前返回持久结果，failed 重放返回稳定同形 error，in-flight duplicate 不重复执行。fresh 请求的无副作用 preflight 与 insert-once `before_tool` 审计可发生在 acquire 前；只有随后取得唯一执行权的赢家才进入审批和真实 handler。八类 Host 入口共用 finalizer：普通错误必持久化 `failed`，成功先持久化 `completed`，Repository 已原子终结时只复用权威事实；`after_tool` 仅由赢家 best-effort 执行，不能推翻已提交结果。fresh winner 必须在 `IMMEDIATE` 事务内获取 managed slot：Run running、duration/total/output/daily 未耗尽且 `count < maxToolCalls`；promotion/Approval claim/Repair override 不重复占槽，却在真实执行前按 reserved `count <= max` 重验，等待期间超限则 ToolCall failed、Approval consumed、handler 和 Work 写入为零。Runtime 投影只能 CAS 自己的 running 行。Run `started` 与四种终态都按明确状态机 CAS；同终态只有完整 canonical payload 相同才可重放，并在 observability 前短路，终态后其他 Runtime Event 全拒绝。active Usage 固定为 Host 校验的逐分量单调 `run_cumulative_v1`，全局/Agent 取每 Run 最终累计，日曲线对新合同按事件 delta 归日，对出现下降的无版本 Legacy Run 只把最后值归最后日；managed Child/Digital 的首次 `run.completed` 在一个事务内重验冻结 duration/total/output/daily 与 `count <= maxToolCalls`，`>=` token/time 或 `>` tool count 映射稳定 budget failed，monitor 和下一 ToolCall 仅提前阻断。Digital 每 Run 输出上限冻结为配置、当日剩余额度与 Model Service 上限的最小值，低于 Host 最小输出量时不创建 Run。Host failure 同步清理 streaming Message。`awaiting_user` 只由 Host 在核验冻结 terminal payload 后创建新 Run。late crash、late message/tool/usage 不改写结果。rewind 跨人工 override Run 会以 `rewind.repair_override_boundary` 拒绝，不能删除审计事实或刷新一次性预算。内部 exactly-once 不替代外部命令/MCP 自身的幂等设计。
