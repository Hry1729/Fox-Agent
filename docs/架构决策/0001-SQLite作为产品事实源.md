# 0001：SQLite 作为产品事实源

> 状态：已接受<br>
> 决策日期：2026-07-29 之前形成，2026-07-29 补录<br>
> 影响范围：会话、运行、工具、项目、配置与恢复

## 背景

Fox 同时具有 React 实时状态、Tauri Host、Agent Runtime Session 和外部知识库任务。如果每一层各自决定会话或 Run 的最终状态，应用容易出现“后端已经完成但前端仍在思考”“刷新后才看到回复”或重启后状态倒退。

Fox 需要一个能够事务更新、离线读取、迁移、备份并随桌面应用分发的本地事实源。

## 决策

使用 SQLite `fox.db` 作为 Fox 核心产品事实源。本地知识库的大体量文档、分块、索引代次与任务事实独立保存在 `knowledge.db`，具体边界见 [0004：本地知识使用独立 SQLite 事实库](0004-本地知识使用独立SQLite事实库.md)。

- 会话、消息、Run、运行事件、工具调用、项目、模型配置和用户资料写入 SQLite。
- React 实时状态是 SQLite 快照与实时事件的投影，不是最终事实。
- Runtime Session 保存执行上下文，但不能覆盖 SQLite 已确认的消息和终态。
- 外部知识库服务拥有其远程数据；Fox 只保存连接、本地绑定、缓存和必要投影。
- 状态恢复优先读取 SQLite，再结合 Runtime 或远程状态补全仍未终结的执行。

## 后果

正面影响：

- 可以通过事务和约束维护状态一致性。
- 应用刷新与重启后能够恢复产品状态。
- 备份、诊断、搜索和迁移有统一入口。
- Runtime 可以替换或重启，而不会带走会话事实。

成本与风险：

- 数据库迁移成为公共兼容契约，不能随意修改旧表。
- 实时事件与 SQLite 写入顺序需要幂等和乱序处理。
- 远程任务与本地投影之间仍需要明确的对账逻辑。
- 大历史需要索引、分页、压缩和保留策略。

## 未采用方案

- **仅使用前端状态**：无法可靠恢复，也不能处理进程异常退出。
- **以 Runtime transcript 为事实源**：Runtime 只负责执行，且 transcript 可能缺失、过期或包含供应商专用字段。
- **以外部知识库服务统一保存全部状态**：会破坏本地离线能力并扩大外部服务故障范围。

## 相关实现

- `apps/desktop/src-tauri/src/database`
- `apps/desktop/src-tauri/src/app_state.rs`
- `apps/desktop/src-tauri/src/runtime_host`
- `apps/desktop/src/features/conversations/model/runtime-event-reducer.ts`
- `apps/desktop/src-tauri/src/local_knowledge.rs`

## 后续要求

- A0 工作闭环的新实体继续进入 SQLite。
- 每次 Schema 变更必须包含升级、数据完整性和备份恢复验证。
- 文档中的表与状态枚举必须以迁移和 Repository 为准。
