# Agent 改造工程交接

> 交接日期：2026-08-03  
> 当前阶段：A0 工程完成，等待 Alpha 人工签收  
> 下一阶段：A1 完整工作闭环

## 1. 当前进度

| 阶段 | 状态 | 说明 |
| --- | --- | --- |
| A0 最小工作闭环 | 工程完成 | Goal -> Task -> Evidence、Work Event、重启恢复、工作模式确认和基础 Trace 已落地；审查修复候选包已重新生成，仍需人工签收 |
| A1 完整工作闭环 | 未开始，下一阶段 | 增加 PlanRevision、ReviewFinding、Acceptance 和完整工作图 |
| A3 会话生命周期 | 未开始 | 归档、收藏、回收站、恢复、Fork 和会话谱系；完成 A1 后实施 |
| A2 长期记忆 | 未开始 | Memory 提议、审批、作用域、召回、冲突与审计；依赖 A3 稳定谱系 |
| A4 可观测与评估 | 仅基础字段 | A0 已有可空 trace/span 字段和脱敏 Trace 导出；完整 Span、指标和 Eval 尚未实施 |
| A5 原生子 Agent | 未开始 | 先串行 Child Run，再考虑有限并发 Worker Pool |
| A6 扩展源协议 | 未开始 | Source Registry、HTTP MCP、OpenAPI 和受限 Hooks |

结论：当前不是“A1 做到一半”，而是 **A0 已完成、A1 尚未开工**。实施顺序保持 `A1 -> A3 -> A2 -> A4 -> A5-serial -> A5-parallel -> A6`。

## 2. 当前工程基线

- 远端 `main` 当前基线提交：`9b76775 FOX-9 add A0 diagnostics and release documentation (#19)`。
- 本地工作区还有 A0 审查修复尚未提交，包括迁移 16、待确认 Run 恢复、Work Event 前端订阅、Evidence 跳转、Trace 脱敏和文档同步。
- 新工程分支必须在这批修复提交并合入 `main` 后创建，不能从 `9b76775` 直接开始 A1。
- 当前数据库 Schema 最高为 16；后续只能追加迁移，不得改写迁移 1-16。
- 最近验证：Rust 163 passed、前端 52 passed（500 Task P95 46.70ms）、Runtime 63 passed、知识预览契约 3 passed；TypeScript + Vite 构建、Runtime sidecar 构建/冒烟、`cargo fmt --check` 和 `git diff --check` 通过。本轮按要求只验证开发环境，未生成 MSI/NSIS 安装包。

### 2.1 2026-08-03 Runtime 稳定性改造

- 新增并固定 `@earendil-works/pi-coding-agent@0.79.9`，Fox Pi Runtime 改由 `createAgentSession` 和 `DefaultResourceLoader` 承载扩展生命周期。
- 新增 Fox 内联规划扩展；只注册 Fox Work Tool，并在每轮注入 Host 掌握的授权项目根目录、权限模式和 Goal/Task/Evidence 快照。
- 不启用 Coding Agent 内置 `bash/read/edit/write`，不照搬 Pi plan-mode/todo 示例的本地状态。SQLite、Host Gate、工具审批和稳定 UUID 仍是唯一产品事实边界。
- `work_snapshot_get` 不再让模型填写 Conversation ID；`ls/find/grep` 省略路径时默认从授权项目根目录开始。
- `task_create_many` 只允许写入 active Goal，彻底禁止 proposed Goal 下提前出现 queued Task。目标确认后由 Host 恢复同一个 Run，再创建任务。
- MiniMax 的 `<mm:think>`、缺失开标签的 `</mm:think>` 以及强特征规划文本会进入 reasoning 流，不再渲染成助手回复。
- Runtime sidecar 构建脚本已支持递归物化 Coding Agent 依赖闭包，仅保留 Fox 支持的 Anthropic/OpenAI provider 入口，并保留 Coding Agent 需要的 provider reset 与会话资源清理契约。
- 本轮只改开发环境与源码，不生成安装包。Runtime 自动化测试现为 63 项；Rust、前端、构建和格式结果以本轮最终验收记录为准。

### 2.2 2026-08-06 Harness 后续决策

- 已接通 provider 原生 `cacheRead/cacheWrite` usage：Runtime 事件、SQLite 使用统计、设置页和对话摘要使用同一口径；旧事件缺字段时按 0 兼容。
- 缓存命中率定义为 `cacheRead / (input + cacheRead)`。Prompt 稳定前缀与工具目录共享 provider 缓存，Fox 不展示无法真实测量的“工具独立命中率”。
- Guardian 后续仅用于高风险审批的辅助审查，必须运行在独立 Reviewer Session，只能返回 `allow|ask|deny` 建议；Host 规则和用户决定不可被覆盖。
- 不接入形成第二套状态源的 Pi plan/todo 扩展；所有规划仍写入 Fox Work Graph。
- 子 Agent 继续后置到 A5。当前先完成单 Agent 评测基线、错误 taxonomy 和缓存/重试/压缩观测。
- 首批外部 Eval 建议组合：SWE-bench、Aider Polyglot、Terminal-Bench、BFCL、tau-bench、AgentDojo 与 RAGAS + Fox 脱敏真实问答集；OSWorld/WebArena 在对应工具能力稳定后启用。

### 2.3 2026-08-06 跨模型适配与离线评测落地

- 新增 `services/agent-runtime/src/model-profile.mjs`，统一解析 provider、模型家族、Pi API、reasoning transport、thinking level、工具/图片/并行/缓存能力、Planner 预算、压缩预算和重试策略。
- `pi-runtime.mjs` 每轮只解析一次 Profile，并将其接入 `createModel`、`createAgentSession`、Planner Session、Prompt Composer、SettingsManager、图片校验、`ready` 和 `run.request_snapshot`。
- 不为每个模型复制整套系统提示词；稳定 Harness 保持一致，仅注入少量模型兼容规则。Host 的 Goal/Task/Evidence、审批和 SQLite 唯一事实边界不变。
- 新增 `services/agent-runtime/evals/run-offline-evals.mjs` 与四组 Fixture。当前离线基线为 `30/30` 通过，属于适配与契约测试，不等价于官方模型质量排行榜。
- 后续接入真实模型时，沿用同一输出 Schema 增加 live adapter eval；凭据必须由显式环境变量/Fox Host 提供，评测工作区必须隔离，默认不把原始 Prompt、文件正文或凭据写入报告。

## 3. 下一阶段 A1 范围

A1 将现有 A0 扩展为：

```text
Goal -> PlanRevision -> WorkTask -> Evidence -> ReviewFinding -> Acceptance
```

必须交付：

1. Plan 每次修改产生新 Revision，旧版本不可覆盖。
2. Task 归属 Plan Revision，并支持依赖、阻塞、重试和证据追溯。
3. Review 与执行结果分离，至少支持严重度、证据和处理状态。
4. Acceptance 按验收条件逐条记录方法、状态和证据。
5. Plan 审批后恢复同一个工作上下文；刷新或重启不丢失待审批状态。
6. 验收失败时重新打开对应 Task，不能仅凭模型文本把 Goal 标为完成。
7. 简单问答继续使用普通对话，不出现多余计划 UI。

现有实施方案中的 SQL 是设计草案。A1 必须扩展当前 A0 表和 Repository，不能平行创建另一套 Goal/Task 事实源。

## 4. 建议 Issue 拆分

1. **A1-01 规格与状态机**：冻结 Plan、Review、Acceptance 枚举、转换图、失败矩阵和事件 Schema。
2. **A1-02 数据与迁移**：追加迁移 17，增加 Plan Revision、Review、Acceptance 及必要索引、外键和乐观版本。
3. **A1-03 Repository**：事务、状态约束、并发版本、重试、验收失败回开 Task 和迁移恢复测试。
4. **A1-04 Runtime 协议**：扩展 Capability Manifest、Host Tools 和 Work Event；旧 Runtime 必须安全降级。
5. **A1-05 前端交互**：Plan 审批、版本历史、Review Finding、Acceptance 和右侧 Task 投影。
6. **A1-06 恢复与验收**：重启、取消、计划修订、验收失败、旧 Runtime、普通问答负向场景和性能基线。
7. **A1-07 文档与发布**：同步数据库、Runtime、Tauri、前端、运维、验收清单和真实任务记录。

开始写业务代码前，先建立 A1 Milestone、规格文档和上述 Issues，并明确依赖顺序。

## 5. 不可破坏的约束

- SQLite 是工作图唯一事实源；Runtime 只通过 Host Tool 读写。
- 不从模型回复文本推断 Goal、Plan、Task、Review 或 Acceptance 状态。
- Work Event 与 Runtime Event 保持独立序号和 reducer。
- 完成 Task 必须有 Evidence；Evidence 失效保留历史。
- 计划、审批和恢复必须持久化，不能只放 React State 或 Runtime Session。
- Trace/诊断默认不导出对话正文、任务正文、Evidence 内容、工具参数、文件内容或凭证。
- A1 不顺带实现 Memory、Fork、子 Agent、OpenAPI 或任意脚本 Hook。

## 6. 开工入口

先阅读：

- `docs/07-路线图/Agent能力实施方案.md` 第 6、12-16 节。
- `docs/07-路线图/A0最小工作闭环规格.md`。
- `docs/07-路线图/A0测试验收报告.md`。
- `docs/04-技术参考/数据库结构.md`。
- `docs/04-技术参考/Runtime协议.md`。
- `docs/03-产品与前端/A0工作闭环使用指南.md`。

核心代码：

- `apps/desktop/src-tauri/src/database/migrations.rs`
- `apps/desktop/src-tauri/src/database/repositories/work_graph.rs`
- `apps/desktop/src-tauri/src/runtime_host/work_tools.rs`
- `apps/desktop/src-tauri/src/work_mode_gate.rs`
- `apps/desktop/src/features/conversations/model/runtime-event-reducer.ts`
- `apps/desktop/src/features/chat/components/GoalProgress.tsx`

每个 A1 PR 至少运行 Rust 测试、前端测试、Runtime 测试、前端构建、Runtime 构建、格式检查和 `git diff --check`。阶段完成后再生成安装包并按真实复杂任务执行人工验收。
