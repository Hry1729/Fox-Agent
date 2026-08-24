# Changelog

本项目的显著变更记录在此。版本号正式发布前仍以仓库配置为准。

## [Unreleased]

### Added

- A0 Goal -> Task -> Evidence 最小工作闭环，包含状态机、乐观锁、阻塞、重试与中断恢复。
- A1 Plan Revision、Reviewer、Finding、Acceptance 和审批 Gate 完整工作图。
- A2 受治理长期记忆、A3 会话归档/回收站/Fork、A4 Span Tree 与离线评测。
- A5 Host-owned Child Run，包含隔离 Session、有限并发、预算、权限交集、取消传播和结果聚合。
- A6 持久 stdio/Streamable HTTP MCP、OpenAPI 3.x、声明式 Lifecycle Hooks 与统一健康审计。
- E1-E4 专家能力：版本化能力包、持久工作流、串行 Supervisor 团队和数字同事首版。
- 插件中心与本地知识工作区，包括导入、分块、检索、引用、资源管理和可选本地向量能力。
- 通用 Host 工具 `http_request`、`system_info` 和只读 `sqlite_read`。
- 覆盖 Runtime、桌面前端、Rust Host、契约 Fixture、Sidecar 和 35-case Agent Harness 的离线回归门禁。

### Changed

- 文档按项目概览、架构、产品与前端、技术参考、开发、运维、路线图、架构决策和历史归档重新收口。
- Assistant、Expert 与 Worker 使用独立语义；专家以冻结绑定叠加到会话，不再占用基础助手身份。

### Known limitations

- 团队首版为串行 Supervisor，不提供 mailbox、成员并行领取任务或自动共享写 worktree。
- Child Run 限制深度、并发与每父 Run 总数；远程知识库 Agent 尚不参与本地 Memory 和 Child Run 池。
- MCP/OpenAPI 暂不支持完整 OAuth、证书固定、外部 `$ref` 和操作级限流；本地向量依赖显式提供的受信运行库与模型包。
- 完整清单以 `docs/07-路线图/已知限制.md` 为准。
