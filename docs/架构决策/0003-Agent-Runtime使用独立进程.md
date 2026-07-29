# 0003：Agent Runtime 使用独立进程

> 状态：已接受<br>
> 决策日期：2026-07-29 之前形成，2026-07-29 补录<br>
> 影响范围：模型循环、会话恢复、打包和故障隔离

## 背景

Fox 桌面 Host 使用 Rust，而 Pi Agent 生态和模型适配主要位于 JavaScript/Node.js。把 Agent 循环直接重写进 Rust 会增加维护成本；把全部桌面能力放进 Node 进程又会削弱 Tauri 的权限和数据边界。

## 决策

Agent Runtime 作为独立 Node Sidecar 进程运行，通过版本化 JSONL 协议与 Tauri Runtime Host 通信。

- Tauri Host 负责进程启动、初始化、请求关联、取消、重启和关闭。
- Runtime 负责模型循环、上下文整理、Pi 事件映射和工具请求。
- 每行标准输出只能包含一个协议 Envelope。
- 协议名称、版本和能力清单在初始化时校验。
- Runtime 崩溃不能直接决定产品终态；Host 和 SQLite 负责恢复与修复。
- 测试使用 Fake Runtime 和真实 Pi Runtime 共同验证 Adapter 契约。

## 后果

正面影响：

- Rust Host 与 JavaScript Agent 生态保持清晰边界。
- Runtime 可以独立测试、构建、替换和故障恢复。
- 模型供应商差异被限制在 Runtime 与模型服务层。
- Host 能继续掌握权限和产品状态。

成本与风险：

- 需要维护跨进程协议、请求关联和事件幂等。
- Sidecar 构建、依赖收集和桌面打包链路更复杂。
- 标准输出污染、进程卡死和协议版本不匹配都需要诊断能力。
- 多 Run 真并发需要额外调度和 Session 隔离设计。

## 未采用方案

- **全部重写为 Rust Agent Runtime**：短期成本高，也会重复维护模型适配生态。
- **将 Node 作为主进程并直接访问系统**：权限、数据库和桌面生命周期边界不够清晰。
- **仅调用远程 Agent 服务**：会失去本地项目与离线执行能力。

## 相关实现

- `apps/desktop/src-tauri/src/runtime_host`
- `services/agent-runtime/src/pi-runtime.mjs`
- `services/agent-runtime/src/fake-runtime.mjs`
- `services/agent-runtime/src/protocol.mjs`
- `services/agent-runtime/test`

## 后续要求

- 协议字段变更必须保持版本兼容或明确拒绝旧版本。
- Sidecar 构建必须包含所有运行时模块和依赖。
- A5 子 Agent 先在现有协议上验证串行 Child Run，再引入真正并发。
