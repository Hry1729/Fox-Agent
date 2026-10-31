# Kernel 改造进度

> 状态：维护中<br>
> 适用版本：Fox 0.1.x<br>
> 维护范围：Fox 自身执行路径（Kernel / Legacy / Shadow）的改造进度与待办<br>
> 最后更新：2026-10-28（单 Pi 分支同步；本轮未复跑产品测试）

本文记录 Kernel 改造状态。当前本地引擎只保留 Pi，旧外部驱动已退休；原生会话阶段二与 C gap 见[单 Pi 主线状态](../01-项目概览/单Pi主线状态.md)。与单 Pi 无关的余项沿用 2026-10-25 核对，不作为本轮新验收。

2026-09-12 及更早的逐轮进度、测试数字和验收包说明已从本文移除，可在 Git 历史（`763a019` 之前的本文件）和 [实施记录](实施记录/) 目录里查到。

## 结论

> **2026-10-31 更正**：Kernel 执行循环已在 B1 删除（controller 驱动、`kernel_gateway`、`shadow_reconcile`、`kernel_coordinator/`、`kernel_model_worker`、`kernel_delegation`）。新 Run 和子 Run 一律 Authoritative + PiNative，没有路由开关（`FOX_EXECUTOR_ROUTE` 已删除），失败不回退；历史 Kernel Run 在启动时标为中断。本页以下内容保留为删除前的历史进度。

- **（历史）Kernel / Authoritative 是 Fox 会话的默认执行路径。** 新 Run 在未设置 `FOX_KERNEL_MODE` 时由 Kernel 驱动真实执行；这一默认值从 2026-09-29 起对普通开发、普通构建和验收包统一生效（`apps/desktop/src-tauri/src/runtime_host/kernel_authority.rs`）。
- **Legacy 只用于调试和兼容旧 Run**：需要显式设置 `FOX_KERNEL_MODE=legacy`。已有 Run 始终按 `run_control_bindings` 里冻结的 `authority` 执行，不跟着新的环境设置变化。
- **Shadow 只观察 Legacy Run**，零副作用；它的退出判据已实现（`kernelShadowGate`）。
- `kernel-default` 构建特性只为兼容旧构建命令保留，不再改变行为。
- **还没做**：Legacy 和 Shadow 路径的清理（四 Agent 方案阶段 4）没有开始；Issue #24、#25 未关闭；共享 Host 加固（四 Agent 方案阶段 1）第 2 项未关闭、第 4 项只在 Windows 上关闭。

## 各项状态

| 项目 | 状态 | 依据 |
| --- | --- | --- |
| Kernel 决策核（独立 crate） | 已完成 | `crates/fox-agent-kernel`；`apps/desktop/src-tauri/src/kernel/mod.rs` 只是兼容入口 |
| Kernel 驱动真实 Run（7A 决策权切换） | 已完成，默认 | `runtime_host/kernel_authority.rs`（含单元测试） |
| Durable outbox 生产执行器 | 已完成 | `KernelCoordinator::dispatch_tool` 经 `kernel_outbox_lease_effect` 领取，核对工具名和 canonical input 后才执行 |
| 模型请求超时的 begin / settle 接线 | 已完成（Authoritative 路径） | `runtime_host/kernel_coordinator.rs` |
| Kernel 模型引擎 | 仅 Pi | `kernel_model_config.rs`；其他值返回 `kernel_engine_retired` |
| Kernel 既有启动恢复 | 已实现；不等于阶段二原生会话恢复 | 生产使用 `runtime_host/kernel_host.rs::recover_kernel_runs_detached`；`crates/fox-agent-kernel/src/recovery.rs::plan_recovery` 只在测试中调用 |
| Shadow 观察与退出判据 | 已完成 | `runtime_host/shadow_reconcile.rs`、`database/repositories/kernel.rs` 的 `kernel_shadow_exit_gate` / `kernel_shadow_recent_exit_gate` |
| 7B Resource Gateway | 部分完成 | `resource_gateway.rs` 已实现四个项目读取器；其他资源类别是否全部迁入**未逐项确认** |
| React 改读统一 Snapshot | 部分完成 | 前端有 `kernel-snapshot.ts`，Host 发 `fox://kernel-state-invalidated`；是否所有界面都只读快照**未逐个确认** |
| 子 Agent（Child Run）走 Kernel | 已完成 | `kernel_delegation.rs`，Host Action `StartChild`，见[子 Agent 与 Child Run 架构](../02-架构/子Agent与ChildRun架构.md) |
| 真实进程崩溃后 UI 续跑的端到端验收 | **无法确认** | 只有 CI 的打包 Kernel worker 测试和人工脚本 `scripts/kernel-isolated-desktop.ps1`；没找到 Pi → Node → JSONL → Rust → React 真实崩溃续跑的自动化测试或验收记录 |
| Legacy / Shadow 路径清理（阶段 4） | **未开始** | 四 Agent 实施方案第 9 节 |

| 单 Pi 阶段二与 C gap | **未完成** | SDK 与 v98 映射部分落地，默认路径及恢复边界未完成验收 |

## 待办

| 事项 | 状态 | 说明 |
| --- | --- | --- |
| [Issue #25](https://github.com/Hry1729/Fox-Agent/issues/25)：`attemptId` 由 Host 生成 | 未开始（Issue open） | `runtime_host/work_tools.rs` 仍用 `required_string(input, "attemptId")`。重名冲突目前作为可纠正错误返回模型，但会消耗连续拒绝预算 |
| [Issue #24](https://github.com/Hry1729/Fox-Agent/issues/24)：模型自我验证证据 | 未开始（Issue open，方案待产品拍板） | `work_graph.rs` 的 `EVIDENCE_REFERENCE_PAIRS` 没有新增配对 |
| 阶段 1 第 2 项：测试授权语义 | **按 A1 改写（2026-10-31）** | 按输入哈希的「本会话允许」已删除，`test_run` 与其他命令类工具一起由四档审批引擎判定；前缀规则不绑定脚本内容，靠审计追溯（已接受的取舍）。见[安全架构](../02-架构/安全架构.md#风险取舍用户已接受) |
| 阶段 1 第 4 项：子进程收尾 | Windows 已关闭；非 Windows 残余 | Windows 用 Job Object 终止整棵进程树；非 Windows 只杀直接子进程 |
| 阶段 4：Legacy / Shadow 清理 | 未开始 | 前置：确认没有需要 Legacy 兼容的旧 Run，以及 FoxBackend 的长期定位 |
| 真实崩溃续跑端到端验收 | 待补 | 见上表 |
| 7B Resource Gateway 全覆盖、React 全面只读快照 | 待核实 | 见上表 |

## 相关文档

- [Agent 运行时架构](../02-架构/Agent运行时架构.md)（Kernel 当前状态表、外层 Agent 后端）
- [四 Agent 改造实施方案 v0.3.1](../02-架构/四Agent改造实施方案-v0.3.1.md)
- [Kernel 原生引擎验收与回退](Kernel-原生引擎验收与回退.md)
- [Fox Agent Kernel 统一控制架构改造方案](Fox%20Agent%20Kernel统一控制架构改造方案.md)（设计原文）
- [已知限制](已知限制.md)
