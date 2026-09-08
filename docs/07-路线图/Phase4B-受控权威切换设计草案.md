# Phase 4B：受控权威切换设计草案

> 状态：**设计草案，未实施。** 当前生产链路仍由 Legacy Runtime Host 作为唯一权威，Kernel 仅以 Shadow 模式做零副作用对照。本文件只描述切换方案与硬前置条件；在硬前置全部验收通过前，不改变任何运行时权威。

## 1. 目标与非目标

**目标**：在可回退、可观测、零数据风险的前提下，把 Run/工具/审批/终态的权威从 Legacy Runtime Host 逐步迁移到 Agent Kernel。

**非目标**：一次性替换 Legacy；不跳过 Shadow 验收；不引入不可回退的执行路径；不替换 Pi 作为模型引擎。

## 2. 执行权安全不变量（切权后必须成立）

1. **同一 Run/Effect 只有一个执行权持有者**：任一时刻一个工具/Effect 由 Legacy 或 Kernel 其中之一执行真实副作用，**禁止 Kernel 与 Legacy 同时执行**。
2. **双写对照数据不得被执行器租赁**：Shadow/双写阶段产生的对照记录（`kernel_shadow_*`、以及任何 `shadow` 模式 kernel_runs/outbox）永远不进入可执行 outbox 租约扫描；租约查询强制 `kernel_mode <> 'shadow'`。
3. **不确定 Effect 回退前必须核验/调和**：回退到 Legacy 时，对副作用是否已发生不确定的 Effect（leased 未确认）必须先核验幂等键 / 调和，**不能无条件交回 Legacy 重执行**。
4. **终态与幂等**：终态唯一、状态机幂等；重复事件 / 重启 / 重放不得产生第二次副作用。

## 3. 硬前置条件逐项状态（2026-09-07 更新）

4A 本轮工程收尾已补入正式代码；不能继续把“仅 cursor 恢复”“Host 尚未委托”“尚无真实 Pi 子进程”当作当前状态。也不能由受控样本通过，推导整个生产面已经通过退出验收。

| 硬前置 | 当前证据 | 证据边界 |
|---|---|---|
| 正式委托与独立事实 | RuntimeHost 逐事件调用 `ShadowReconciler::observe_applied_event`，preflight/启动失败/crash 委托同一协调器 | Legacy 仍执行真实工具；不是 Kernel 权威切换 |
| toolCallId / input / isError | 真 Pi 子进程事件经过 `Database::apply_runtime_event` 与共用观察入口，断言持久化工具身份/结果/终态 | faux provider；读取 preflight 在测试夹具中应答，不代表 Tauri 审批 UI 已联测 |
| 实际多工具批次 | Pi message_end 保留 batchId/sourceOrder；跨批次累计、乱序事实、部分结果跨重启测试 | 独立 Legacy 事实齐备后才生成批次比较；缺事实不会假装 Match |
| Graph 嵌套和无 preflight 投影 | 现有合法父/伪造父、claimed/unclaimed、未知工具 fail-closed 集成测试仍保留 | Graph 真 UI 全路径样本仍需业务验收 |
| 取消/失败/存储故障 | 原有取消/终态测试；新增 diff/checkpoint 事务故障注入，Failed 状态跨重启不复活 | 不将错误包装成健康关闭 |
| 跨重启语义恢复 | v53 检查点 + `RunController::rehydrate`；工具、审批、批次、模型锚点和未齐备观察均保存；启动入口已接线 | 恢复的是观察上下文，不重启真实工具或自动重发审批 |
| 冻结权限 | 模式/项目根/作用域授权冻结并参与内容哈希；允许/只读/审批分类独立计算 | 动态 Hook、计划/Expert 限制仍由 Legacy 执行，不能把基础权限比较称为全安全面等价 |
| 零副作用与门信号 | 隔离表、两种 Run ID 的零 outbox 断言；`kernelShadowGate` 对缺样本/错误/不可比较一律不放行 | 严格门只评价选定样本，不自动批准生产放量 |

完整接续记录见 [Phase4A 接续收尾记录](../评审/Phase4A-接续收尾记录-2026-09-07.md) 与本轮对照代码的核验 [Phase4A 真实门核验记录](../评审/Phase4A-真实门核验记录-2026-09-08.md)。

### 三层状态必须区分（不可互相推导）

1. **选定样本门通过**：受控（真实 Pi 子进程 + faux provider + 临时数据库）的零差异样本，严格门 `passed`。这只覆盖被观察的选定 Run。
2. **真实业务覆盖充分**：尚未达成。真实审批 UI、动态 Hook、计划/Expert 限制、作用域授权运行中变化、Graph 完整 UI 仍需真实业务样本审阅。最新 100 个 Pi Run 统计不自动证明覆盖。
3. **生产切权批准**：未批准。`authoritativeEnabled=false`、`productionRolloutApproved=false` 保持；任何切权必须单独批准。

## 4. 切权前仍需的验收

1. 采集真实业务 Shadow 样本并审阅安全、状态、工具批次的覆盖范围与差异归因；受控 faux provider 测试不替代该步骤。
2. **未覆盖安全面的处理方式是“排除出首批切权范围”或“明确保留 Legacy 安全检查”，不是豁免**：动态 Hook、计划/Expert 限制、授权变化等未独立建模的场景，首批切权时必须路由到 Legacy 或显式排除；不能通过豁免就宣称权限等价。
3. 对每个拟切权场景确认唯一执行者、回退和不确定副作用调和策略；按本草案单独批准 4B 实施。
4. 保留真实 P95 门禁及固定阈值；不以降低百分位、放宽阈值或自动重试作为切权依据。
5. 模型请求生命周期必须是严格的 begin/settle 配对（无重复 begin、无悬空 begin、无未配对 settle）；门禁按事件顺序校验配对而非仅计数。

### 默认特性验证的替代路线（已验证可行）

默认特性（local-embedding + zvec）全量与 P95 不需要关闭正在运行的桌面程序：使用独立构建输出目录
`CARGO_TARGET_DIR=target-default-check` 可避开被占用的 zvec DLL，`Compiling fox-desktop` 后 `Finished`（真实编译成功），
且不删除 DLL、不关闭特性、不结束进程。本轮已在该目录完成默认特性全量（576 通过 / 0 失败 / 2 既有忽略）与 P95
（snapshot 1.97ms ≤50ms、transaction 0.90ms ≤20ms）。后续默认特性复跑可继续使用该目录；tauri.conf.json 资源路径绑定不受影响。

## 5. 切换阶段（灰度，每阶段独立可回退）

- **4B-0 Shadow 收敛（当前）**：Shadow 持续在线比较，聚合 diff category 达零安全差异阈值并能定位来源。authority=legacy。
- **4B-1 双写只读权威**：Kernel 决策写正式记录但**执行权仍唯一在 Legacy**；双写对照不被执行器租赁。authority=legacy。
- **4B-2 影子执行器（默认关）**：Kernel outbox 执行器仅在显式 feature flag 下运行；同一 Effect 不同时执行；对不确定 Effect 先核验幂等键再回退。
- **4B-3 受控切权**：按 Profile/Run 维度灰度，Kernel 为权威，Legacy 为可调和回退热备。
- **4B-4 回退移除**：长期稳定后评估。

## 6. 回退原则

- 任何阶段 Kernel 异常/超时/分歧超阈值 → 自动回退 Legacy 权威。
- 回退前对不确定 Effect 调和（核验幂等键/已有结果），不盲目重执行。
- 切权不改数据 schema；新表均为 additive 迁移。
