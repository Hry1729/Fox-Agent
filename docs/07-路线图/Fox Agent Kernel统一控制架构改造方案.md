# Fox Agent Kernel 统一控制架构改造方案

> 状态：设计与实验基线已确认；Kernel 决策核与持久化骨架已实现，生产接线、Shadow 对比和权威切换尚未完成。  
> 适用范围：Fox 对话循环、工具、权限、审批、重试、取消、上下文压缩、Skill、专家、子 Agent，以及 Pi、DeepSeek Harness、Codex 等执行引擎的接入。  
> 阅读说明：本文中的“目标结构”表示待实施设计；只有“当前事实”和“实验结果”表示已经过代码或测试确认的现状。

## 一、结论

Fox 应把所有 Agent 的关键控制决策统一到 Rust 侧的 **Fox Agent Kernel**，而不是继续让 React、Rust Host、Node Runtime 和引擎各自决定一部分。

用大白话说：

- **Fox Kernel 是总指挥**：决定现在处于什么状态、能不能调用工具、要不要审批、是否重试、何时结束。
- **Runtime Host 是翻译和接线员**：启动引擎、收发消息、维持会话、把不同引擎事件翻译成 Fox 协议。
- **Resource Gateway（Tool Host）是执行窗口**：真正接触文件、网络、数据库和系统资源，并再次检查权限。
- **Pi、DeepSeek Harness、Codex 是可替换的发动机**：负责模型推理和它们擅长的循环能力，但不能成为 Fox 权限、状态和事实的最终裁判。
- **React 只负责展示和提交用户操作**：不再自己推断“是否完成”“是否需要审批”或“工具到底成功没有”。

这样设计不是为了把所有逻辑都塞进一个巨型模块，而是为了做到：**一个规则只有一个权威来源，其他层各司其职。**

## 二、为什么要统一

当前 Agent 控制逻辑横跨多层：

1. React 负责一部分自动执行、审批和状态展示。
2. Rust Desktop Host 负责运行生命周期、权限、持久化和 Host 工具。
3. Node Pi Runtime 负责提示词、会话、重试、继续判断、工具适配和部分只读工具执行。
4. Pi 第三方包内部还拥有模型循环、工具批次、压缩和重试能力。

这会产生几类典型问题：

- 自动执行已开启，某一层仍然弹审批。
- 用户点击取消，工具确实停了，但运行状态被记成失败。
- 多个工具并行返回时，某一层只消费或展示了其中一个结果。
- Provider 已经重试，外层又重试一次，造成请求慢、费用增加或 429 加重。
- UI 根据零散事件猜状态，重启以后无法恢复待审批操作。
- 将来再接一个 Agent 引擎时，需要复制整套权限、重试和状态逻辑。

统一后的原则是：**引擎可以提出动作，Kernel 决定动作是否成立，Resource Gateway 决定动作是否能安全落地，数据库保存最终事实。**

## 三、当前结构的客观约束

### 3.1 Rust 目前仍是单个 Tauri crate

Rust 代码主要位于 `apps/desktop/src-tauri`，并且许多模块直接依赖 Tauri、命令层和 App State。当前仓库还没有可直接承载 Kernel 的完整 Cargo workspace 拆分。

因此不能一上来就机械地把文件搬到 `crates/`。正确顺序是：

1. 先在现有 crate 内建立不依赖 Tauri 的 Kernel façade、领域类型和端口接口。
2. 把 Tauri 调用留在适配层。
3. 等依赖方向稳定后，再抽取独立 crate。

### 3.2 Pi 不只存在于 Rust 侧

Fox 实际使用的是 Node sidecar 中的 `@earendil-works/pi-coding-agent`。Node wrapper 里已经存在：

- Validation Policy；
- Execution Profile；
- Continuation Decision；
- Prompt Registry；
- Planning Extension；
- Runtime 工具与 Host 工具分流；
- 重试、会话、事件映射和提示词装配。

所以改造不能只改 Rust。阶段计划必须同时处理 Rust 与 Node 的职责迁移，并先验证第三方 Pi 的公开扩展契约。

### 3.3 “只读”也属于安全边界

读文件、读知识库和读取 Git 信息同样可能越过项目目录或泄露敏感信息。“没有写入副作用”不等于“没有权限风险”。

最终目标仍是所有资源访问经过 Resource Gateway。迁移期间如果保留 Node 只读执行器，它必须使用与 Rust 等价的路径、作用域和策略快照校验，并被明确视为 Resource Gateway 的临时延伸，不能视为无条件例外。

## 四、八条架构原则

1. **单一决策源**：权限、审批、重试、预算、终态和状态转换由 Kernel 最终裁决。
2. **单一事实源**：Run、Turn、Tool Call、审批、产物和终态以 SQLite 中的权威事实为准。
3. **引擎能力显式声明**：不假设所有引擎都支持同样的 Hook、取消、压缩、缓存和恢复能力。
4. **副作用双层校验**：Kernel 作出决策，Resource Gateway 在执行前使用冻结策略再校验一次。
5. **恢复优先于内存便利**：待审批、重试计划和工具状态不能只存在于 Promise 或进程内存中。
6. **事件可重放，快照可重建**：UI 读取 Run Snapshot；Snapshot 来自持久事件与事实投影，不是前端猜测。
7. **迁移可比较、可回退**：Legacy、Shadow、Authoritative 三种模式相互隔离，每次只切换一个变量。
8. **不复制外部 Agent 的整套骨架**：学习 Maka、Codex、DeepSeek Harness 的方法，但映射进 Fox 的统一模型。

## 五、目标职责边界

### 5.1 Fox Agent Kernel

Kernel 负责：

- Run、Turn、Tool Call、Child Run 的状态机；
- 自动执行、审批和权限决策；
- 工具批次调度规则；
- Provider 重试与 Turn 重试策略；
- 预算、超时、取消和恢复策略；
- 上下文预算与是否压缩的决策；
- 完成条件、失败分类和最终验收；
- Expert、Skill、Child Agent 的解析与冻结；
- 引擎能力校验和降级方案；
- 生成供 UI 读取的权威 Run Snapshot。

Kernel 不负责：

- 直接渲染页面；
- 直接调用模型厂商 HTTP API；
- 直接操作本地文件或浏览器；
- 实现每个引擎私有的流式协议。

### 5.2 Runtime Host

Runtime Host 负责：

- 启动、监控、重建和关闭引擎进程；
- 将 Fox 的标准请求发送给引擎；
- 将 Pi、DeepSeek Harness、Codex 的事件映射为 Fox 事件；
- 维护连接、心跳、序列号和背压；
- 把 Kernel 的工具决策传回引擎；
- 传递取消信号，但不自己重新解释权限规则。

### 5.3 Resource Gateway / Tool Host

Resource Gateway 负责：

- 文件、Git、知识库、网络、MCP、进程等资源操作；
- 路径规范化、项目作用域和命令参数校验；
- 根据冻结的 Permission Snapshot 再做一次硬校验；
- 幂等键、超时、取消令牌、审计记录；
- 返回结构化 Tool Result 和可诊断错误。

### 5.4 Engine Adapter

每种引擎只实现统一协议和能力声明：

- Pi Adapter；
- DeepSeek Harness Adapter；
- Codex Adapter。

适配器不能自行扩展为第二套 Kernel。某个能力如果引擎不支持，就在能力清单里明确写 `unsupported` 或使用降级路径。

## 六、统一协议与数据模型

### 6.1 协议权威源

跨进程协议采用“一份 Schema，多端生成”：

1. Rust DTO 和 `schemars` 生成的 JSON Schema 作为 Fox 协议权威定义。
2. TypeScript 类型由 Schema 生成。
3. TypeBox 只用于动态工具参数 Schema，不再维护一份重复的运行协议。

计划中的目录结构：

```text
crates/
├─ fox-agent-kernel/       # 与 Tauri、具体引擎无关的控制核心
└─ fox-engine-protocol/    # DTO、JSON Schema、版本兼容

packages/
└─ fox-engine-protocol/    # 自动生成的 TypeScript 绑定

services/
├─ agent-runtime/          # Pi Adapter
├─ deepseek-runtime/       # DeepSeek Harness Adapter
└─ codex-runtime/          # Codex Adapter
```

目录是目标形态。拆分前先在现有 Tauri crate 内验证依赖方向，避免为了目录好看而产生循环依赖。

### 6.2 核心身份

所有事件至少携带：

- `engineId`：使用哪个执行引擎；
- `runId`：一次完整运行；
- `turnId`：一轮模型输入输出；
- `toolCallId`：一次工具调用；
- `eventId` 与单调序列号：去重和重放；
- `policySnapshotId`：本次运行冻结的策略；
- `parentRunId`：子 Agent 与父运行的关系。

一个 Run 启动后冻结 `engineId`，恢复时禁止偷偷换引擎。不同引擎的会话状态通常不兼容；需要换引擎时应新建 Run，并显式关联来源。

### 6.3 状态机

建议的主要状态：

```text
created
  → running
  → waiting_approval
  → running
  → retry_scheduled
  → running
  → compacting
  → running
  → completed | failed | cancelled | budget_exhausted
```

要求：

- `retry_scheduled` 与 `compacting` 必须分开；前者是错误恢复，后者是上下文管理。
- 等待审批期间保存完整 Tool Call 和 Policy Snapshot。
- 超预算不能笼统记为失败，必须有独立事件、原因和下一步策略。
- 终态只能写入一次；迟到事件不能把 `cancelled` 改回 `failed`。

### 6.4 工具批次语义

模型一次可以提出多个 Tool Call。统一规则为：

1. 每个 Tool Call 都有独立状态和结果，不允许用“最后一个结果”代表整批。
2. 只读且无冲突的工具可以并行。
3. 写工具默认按原始调用顺序串行；后续可根据写域证明安全后放宽。
4. 执行完成顺序可以不同，但模型下一轮必须等待本批所有调用进入终态。
5. 传给模型的结果数组按原始 Tool Call 顺序排列。
6. UI 可实时展示各工具的实际完成顺序，不需要伪装成串行。

## 七、Engine Capability Manifest

每个引擎接入前必须填写能力清单，不能靠“应该支持”判断：

| 字段 | 说明 |
| --- | --- |
| `loopOwnership` | 引擎循环能否由 Fox 驱动、暂停或单步推进 |
| `toolInterception` | 能否在执行前阻止、等待、修改或取消工具 |
| `toolExecutionOwnership` | 工具由引擎、Node 还是 Rust 执行 |
| `parallelToolCalls` | 是否支持并行工具及最大并发数 |
| `toolResultBarrier` | 能否等齐整批结果再进入下一轮 |
| `toolResultOrdering` | 能否按模型原始调用顺序回传结果 |
| `cancellation` | 能否取消 Hook、Host 请求、模型流和运行中工具 |
| `providerRetryControl` | Provider HTTP 重试能否关闭、配置和观测 |
| `turnRetryControl` | 整轮 Agent 重试能否独立关闭和配置 |
| `compactionOwnership` | 上下文压缩由谁决定、谁执行 |
| `pendingApprovalResume` | 重启后能否恢复待审批工具 |
| `promptCacheMode` | 支持显式断点、自动缓存还是不支持 |
| `promptCacheUsage` | 是否返回缓存写入/命中用量 |
| `childAgent` | 是否原生支持子 Agent，如何映射为 Child Run |
| `multimodal` | 支持哪些输入和输出类型 |

Manifest 是接入考试卷，也是运行时选择引擎和降级策略的依据。

## 八、Pi 七项契约实验

### 8.1 实验范围

实验通过 Fox 当前实际入口 `createFoxAgentSession → createAgentSession` 和完整 `pi-runtime.mjs` sidecar 进行，不以 Pi 内部私有 Hook 的存在代替 Fox 可用性。

测试文件：`services/agent-runtime/test/pi-kernel-readiness.test.mjs`

运行方式：

```bash
cd services/agent-runtime
node --test test/pi-kernel-readiness.test.mjs
```

当前完整结果：**10 项通过、0 项失败、0 项 TODO**。其中 Provider 429 请求次数实验额外连续运行 5 次，5 次均通过。

### 8.2 结果总表

| # | 必须回答的问题 | 实验结论 | 对改造的影响 |
| --- | --- | --- | --- |
| 1 | 公开扩展通道能否阻止、等待和取消工具 | **通过**。`tool_call` 公开扩展处理器能够阻止工具；异步 Promise 能暂停执行；`extensionContext.signal` 能收到 `session.abort()` | Pi 可以在不 fork 第三方包的前提下接入 Kernel 决策，但应使用扩展系统的公开 `tool_call` 通道，不依赖私有 `beforeToolCall` 槽位 |
| 2 | 多个扩展处理器的顺序和参数修改风险 | **确认存在风险**。处理器按注册顺序串行执行；后面的处理器能看到并继续原地修改前面批准过的参数，最终工具收到修改后的值 | 不能“先审批、后允许其他扩展改参数”。应在全部非安全扩展结束后生成规范化参数，再由 Kernel 最终审批并冻结；Resource Gateway 还要对最终参数重验 |
| 3 | Provider 重试和 Turn 重试能否分别关闭 | **通过**。Provider 的 `maxRetries` 能独立设为 0；真实本地 429 探针验证 0 次重试为 1 个请求、1 次重试为 2 个请求。Turn retry 的 `enabled/maxRetries` 可单独控制 | 两层策略可以收归 Kernel，但 Adapter 必须明确传值并上报每次重试；禁止使用不透明默认值 |
| 4 | 审批等待是否受 Pi、Fox Host 或 Run Budget 超时影响 | **部分通过**。Pi 公开 Hook 能挂起；Kernel 已把执行预算与审批等待分成两只时钟并有单测，但生产 Host 的 6 分钟请求超时尚未迁移到该模型 | 生产接线必须让审批等待使用持久化审批时钟，普通 Host 请求超时不得提前结束审批 |
| 5 | 取消能否传到 Hook、Host 请求和运行中工具 | **部分通过**。公开 Hook 能收到取消；sidecar 已按 `runId` 中止 Host 请求，取消终态稳定为 `run.cancelled`，且并发 Run 不互相误伤；已经进入 Rust 执行器的工具仍没有统一的逐 Tool Call 取消保证 | 将 Kernel 的 run-scoped、tool-scoped Cancellation Token 接到 Rust Host 和每类执行器；迟到结果只记审计 |
| 6 | 重建会话后能否恢复待审批 Tool Call | **部分通过**。Pi sidecar 自身仍不会恢复内存中的待审批调用；Rust/SQLite 已能跨数据库重开恢复 Tool Call、参数和 pending 审批，并用 CAS 防止重复批准，但生产恢复编排和恢复后派发尚未接线 | 恢复必须由 Kernel/Host 从 SQLite 重放并重新发布审批，不能依赖 Pi session；在完成真实进程重启与派发测试前不得称为端到端通过 |
| 7 | 并行结果能否完整、有序进入下一轮 | **通过（Pi/Node 与 Kernel 决策核）**。三个工具实际完成顺序为 B、C、A，下一轮模型收到 A、B、C；Kernel 也会等齐批次并按 `source_order` 生成一次屏障 | 仍需补 Rust 持久化、Snapshot 和 UI 投影的五层端到端合同，防止后续层再次吞结果 |

### 8.3 对七个问题的直白解释

1. **Pi 能让 Fox 插手工具执行。** 工具真正运行前，Fox 可以说“等一下”“不允许”或“取消”。这说明目前不需要为了审批功能 fork Pi。
2. **扩展不是天然安全链。** 后注册的扩展可以把前面已经批准的参数改掉，所以最终安全检查必须放在最后，并且执行入口还要复核。
3. **两种重试确实是两回事。** Provider 重试是一次 HTTP 请求失败后再请求；Turn 重试是整轮 Agent 失败后重来。当前技术上能分别开关，之后必须由 Kernel 统一配置。
4. **Pi 能等人，Kernel 也已经把“等人时间”和“执行时间”分开了，但生产 Host 还没接上。** 当前真正的遗留限制是 Host 请求超时与恢复编排。
5. **取消的 Node 半链路已经修好。** 取消等待中的 Hook/Host 请求会得到 `run.cancelled`，也不会误伤别的 Run；Rust 中已经开始执行的具体工具仍要继续接统一取消令牌。
6. **数据库已经记得待审批内容，但应用重启后还不会自动接着执行。** Tool Call 和审批事实可以恢复，下一步要由生产 Host 重发审批并在批准后只派发一次。
7. **Pi 没有吞掉并行工具结果。** 本次实验证明 Pi 和 Node 事件映射层能处理三个结果；如果产品里仍只显示或消费一个，问题应继续往 Rust 事件持久化、批次聚合或前端投影查。

## 九、改造步骤

### 阶段 0：冻结基线与契约验证

目标：先知道哪些能力是真的，哪些是假设。

- 固定多工具、自动审批、429、取消、恢复、压缩和终态基线。
- 把测试分成两类：
  - bug 复现测试：先红后绿；
  - 引擎合同测试：描述引擎现在真实支持什么。
- 使用 deterministic fake runtime 做 CI 主合同，真实 Provider 只做受控 smoke。
- 建立 Engine Capability Manifest。
- Pi Spike 建议两天完成，硬上限三个工作日；超过时间仍无法确认的能力按“不支持”设计。

本文第八章已完成七项首轮 Pi 合同实验和修复后复测。它只证明列出的边界，不表示 Kernel 改造已经完成。

当前实施边界：

- 阶段 0 已完成；Agent Runtime 全量合同测试为 219 项通过。
- 阶段 1 完成了 Node 取消终态、按 Run 隔离和 Provider/Turn 重试拆分；多工具在 Kernel+持久化层的批次屏障/逐调用幂等/原序回传已实现并测试，五层（Pi/Fake→Node→JSONL→Rust→React）生产端到端合同与 React/Node/Rust 读取同一冻结策略的统一审批仍待接线。
- 阶段 2 的 Rust `RunController` 决策核已具备状态机、审批、批次屏障、取消终态、`retry_scheduled`/`compacting` 独立流程、Provider/Turn 两层重试计数、持久化重试 due time 与工具执行超时；重启后的 Run/Tool 单调超时会在首个 tick 重新锚定并继续累计。模型请求超时的 Adapter 层执行仍未接线，决策核尚无生产调用者，因此这里只能称“决策核大部分完成”，不能称生产阶段完成。
- 阶段 3 的**关闭路径持久化基础设施**已实现并通过测试，但尚不能称“真实生产 Run 已恢复”：v49 创建 durable Effect/Dispatch outbox，v50 增加 durable `deliver_tool_batch`；事件连续序号、Batch/Tool（含 canonical input）状态、审批 CAS、dispatch intent 与 Run 聚合在一个事务提交，审批结论/Tool 状态/dispatch 意图必须一致，工具结果与 leased dispatch 完成也在一个事务结算。rehydrate 会逐项核对冻结身份、事件游标、终态、批次成员/顺序、审批、dispatch 和 barrier delivery 一致性并 fail-closed；恢复计划重发仍有效审批与安全 pending 动作，终态只继续排空取消，leased 动作必须对账。富 Snapshot 使用单一致读事务并复核冻结身份与事件 cursor。身份裁决为 batch_id 全局唯一、tool call `(run_id,tool_call_id)`、outbox `(run_id,effect_key)`。这些只证明 SQLite/纯 Rust 合同；生产 Host、Pi/Node/JSONL/React 尚未调用该控制面，Kernel 表不驱动真实 Run。
- 阶段 5（生产 Shadow，2026-09-07 接续收尾）：正式 RuntimeHost 已委托同一 `ShadowReconciler`，不再维护平行 ShadowContext 状态机；父/子/数字同事 Host 共用观察注册表。v53 新增独立 `kernel_shadow_checkpoints`：RunController 语义状态、工具/审批/批次、模型请求锚点、冻结权限内容、独立比较事实以及未齐备 Pi 批次观察均可恢复，检查点与 diff 同事务提交，失败/关闭状态不会重启为健康空上下文。真实 Pi `turn_start/message_start/message_end` 提供模型请求生命周期与实际批次/source_order，经正式数据库事件投影后进入共用观察入口；已增加受控真实 Pi 子进程 + 本地 faux provider + SQLite 联测（非真实远程模型、非 Tauri 审批界面测试）。权限模式、项目根和作用域授权在 Run 开始时冻结并纳入内容哈希。诊断返回 `kernelShadowGate`：最新 100 个 Pi Run（包括缺少观察记录者） 使用严格零非 Match 门槛，缺样本/未闭合/未建立实际批次或模型生命周期/不可比较/落库错误均不放行。**Legacy 仍是唯一执行权威，Shadow 不写或消费可执行 outbox。工程收尾不等于生产放量验收；`productionRolloutApproved` 固定为 false，仍需实际业务样本及覆盖审阅。** 详细测试与边界见 [Phase4A 接续收尾记录](../评审/Phase4A-接续收尾记录-2026-09-07.md)。
- 阶段 4（Cargo workspace 拆分）、阶段 6（7A 切决策权）、阶段 7（7B Resource Gateway）、阶段 8（React Snapshot）、阶段 9（多引擎）未开始；当前生产链路仍由 Legacy 控制，**不是 authoritative**。

### 阶段 1：并行修复当前 P0 问题

这一阶段不必等待整个 Kernel 落地：

- 修复 Host 预检取消被记成 `run.failed`。
- 为多工具建立批次 ID、逐调用结果和 all-settled 屏障，补 Rust/UI 端到端测试。
- 自动执行只决定“满足策略的工具不弹窗”，高风险与超出权限的动作仍然阻止或审批。
- 合并分散的权限判断，至少先让 React、Node 和 Rust 读取同一份冻结策略。
- 将 Provider 429 与 Turn 失败分开记录，使用服务端 `Retry-After` 和有界退避。
- 模型 ID、Base URL 和兼容路由在发送前诊断，404 不进入盲目循环。
- 为“长时间思考”增加首事件、Provider 请求、首 Token、工具等待和重试阶段耗时。

### 阶段 2：在现有 Rust crate 内建立 Kernel façade

先不拆 Cargo workspace，建立纯 Rust 领域边界：

- `RunController`；
- `PolicyDecisionPort`；
- `ToolDispatchPort`；
- `EnginePort`；
- `EventStorePort`；
- `SnapshotProjectionPort`；
- `Clock` 与 `CancellationPort`。

要求 Tauri command、SQLite Repository、Node IPC 都成为适配器。Kernel 核心代码不得依赖 `tauri::AppHandle`、React 类型或 Pi 类型。

退出条件：相同输入事件能得到确定的决策和状态转换，并且可以用纯 Rust 测试运行。

### 阶段 3：建立持久状态、决策语料和 Snapshot

- 增加 Run、Turn、Tool Call、Tool Batch、Approval、Retry、Budget、Engine Profile 的持久事实。
- 把 `legacy / shadow / authoritative` 和冻结配置保存在 Run 上。
- 生成一批来自真实脱敏日志的 `输入 → 期望决策/状态` 黄金语料。
- 建立 Run Snapshot 读模型和增量投影。
- 待审批 Tool Call 必须可在重启后恢复。
- 迁移采用 Expand → Backfill → Dual Read → Switch → Cleanup，不直接破坏旧表。

Shadow 模式允许落“影子决策和比较结果”，但不得触发第二次外部副作用，也不得改写 Legacy 权威状态。

### 阶段 4：拆分 Cargo workspace 与统一协议

- 抽取 `fox-agent-kernel`。
- 抽取 `fox-engine-protocol`。
- 生成 TypeScript 协议包。
- 保留 Tauri、SQLite 和 sidecar 适配层。
- 增加 Rust ↔ TypeScript 协议兼容测试和版本协商。

### 阶段 5：Pi Shadow

- Pi 继续实际执行，Kernel 同步计算决策但不产生副作用。
- 对比权限、审批、重试、终态、批次结果和 Snapshot。
- 差异写入审计表并可从诊断页面查看。
- 使用黄金语料回放，不能只看人工日志。

退出条件：零安全差异；状态与工具批次差异达到约定门槛；可以明确定位差异来自 Kernel、Adapter 还是旧路径。

### 阶段 6：7A——只切换决策权

- `shadow → authoritative`。
- 权限、审批、状态、重试、预算和终态由 Kernel 决定。
- Node 只读执行器暂时不搬，先保持执行位置不变。
- 通过一个开关独立回退到 Legacy 决策。

这一阶段只验证“谁作决定”，避免与工具搬迁问题混在一起。

### 阶段 7：7B——迁移资源执行器

- 将 Node 特权与只读资源执行逐步迁到 Rust Resource Gateway。
- 每类工具单独测行为、权限、取消和性能。
- 保留逐工具类别回退能力。
- 完成后 Node Runtime 不再直接接触受保护资源。

这一阶段只验证“在哪里执行”，不再改决策语义。

### 阶段 8：前端改为 Snapshot 驱动

- 对话页、审批、任务中心、子 Agent 和通知中心读取统一 Snapshot。
- UI 不再根据零散文案或单个事件推测终态。
- 当前页面已经明确展示的审批完成、失败等消息默认静默入历史，不重复制造未读红点。
- 自动执行、等待审批、重试、取消和恢复使用统一状态组件。

### 阶段 9：接入 DeepSeek Harness 和 Codex

顺序不是“谁名气大先接谁”，而是谁先通过 Manifest 和合同测试：

1. 实现标准 Adapter。
2. 填写 Capability Manifest。
3. 跑相同的七项合同及错误、压缩、缓存、子 Agent 扩展测试。
4. 不支持的能力明确降级，不在 Adapter 内补一套私有控制系统。
5. 先 Shadow，再 Authoritative。

Provider 缓存只要求“能力如实声明并上报实际 usage”。不能要求所有引擎都允许 Fox 设置同一种缓存断点。

## 十、迁移、回退与数据安全

### 10.1 三态隔离

| 模式 | 谁执行 | Kernel 是否落事实 | 用途 |
| --- | --- | --- | --- |
| `legacy` | 当前链路 | 仅记录必要兼容信息 | 安全回退 |
| `shadow` | 当前链路 | 落影子决策和差异，不产生副作用 | 比较新旧逻辑 |
| `authoritative` | Kernel 决策链 | 落权威事件和 Snapshot | 正式路径 |

每个 Run 在创建时冻结模式、引擎、能力清单版本、权限策略和提示词配置，运行中不允许悄悄切换。

### 10.2 回退原则

- 决策权切换与执行器搬迁使用两个独立开关。
- 回退不能删除新事件，只改变后续 Run 的模式。
- 已在 Authoritative 中启动的 Run 按原模式恢复；不能换成 Legacy 后接着跑同一会话。
- Schema 只做向前兼容扩展，清理旧字段放到稳定运行后的独立阶段。

## 十一、测试与验收

### 11.1 测试层级

1. Kernel 纯函数与状态机单元测试。
2. Rust/TypeScript 协议合同测试。
3. 每个 Engine Adapter 的统一合同测试。
4. Resource Gateway 权限、路径、取消和幂等测试。
5. SQLite 迁移、事件重放和 Snapshot 重建测试。
6. Desktop 端到端测试：对话、批量工具、审批、自动执行、取消、重启恢复。
7. 故障注入：429、断网、进程退出、迟到事件、重复事件和半完成工具。
8. 安全测试：目录穿越、符号链接、参数审批后篡改、跨项目复用策略。
9. 性能回归测试。

### 11.2 性能门槛

在同一硬件和固定工具集上冻结改造前基线，建议初始门槛为：

- Kernel 本地决策 P95 不高于 10ms；
- 新增 IPC 与 Resource Gateway 校验 P95 不高于 20ms；
- 不含模型网络耗时的本地工具控制链 P95 相对基线增长不超过 15%；
- 并行只读工具不能因为结果排序要求退化为串行执行。

若真实基线表明绝对值不合理，允许在阶段 0 调整一次并写入基线文件；不能在测试失败后随意放宽。

### 11.3 安全硬门槛

- 未知策略、未知 Profile、Schema 不兼容一律 fail-closed。
- 审批后的参数如果发生变化，原审批立即失效。
- 任何工具结果都按 `toolCallId` 幂等提交，重复事件不产生第二次副作用。
- 取消后的迟到成功结果只能记为审计信息，不能复活 Run。
- 新项目默认权限必须在项目创建流程真实读取，并有自动化测试防止权限绕过。

## 十二、文档同步规则

文档不使用“某月某日新增了某功能”的更新日志式措辞，而描述当前结构、能力边界和使用方式。

- 协议或状态变化随代码同步更新《Runtime 协议》《错误与状态》《权限参考》及迁移说明。
- Pi 迁移到 Authoritative 后，再统一更新《Agent 运行时架构》、项目 README 和项目概览。
- DeepSeek Harness、Codex 只有通过合同测试后才写入正式能力矩阵。
- 未验证能力必须标为“候选”“实验中”或“不支持”，不能写成已经可用。
- 架构裁决使用 ADR 保存，路线图只保留结论和阶段门槛。

## 十三、实施前最终决策门

满足以下条件后才进入 Kernel Authoritative：

1. 七项引擎合同均有明确答案，不允许“应该可以”。
2. Host 预检取消能正确进入 `run.cancelled`。
3. 待审批 Tool Call 能持久化并恢复。
4. 权限、审批、重试和终态在 Shadow 中没有安全差异。
5. Rust/TypeScript 协议和数据库迁移可回退。
6. 多工具在 Node、Rust、SQLite、Snapshot、UI 五层均不丢结果。
7. 性能、安全和故障注入门槛通过。
8. Pi 决策切换与执行器迁移能独立回退。

如果某个引擎无法满足其中一项，应在 Capability Manifest 中声明限制并采用降级方案，而不是把例外偷偷写进 Kernel。
