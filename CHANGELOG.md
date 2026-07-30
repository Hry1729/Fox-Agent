# Changelog

本项目的显著变更记录在此。版本号正式发布前仍以仓库配置为准。

## [Unreleased]

### Added

- A0 Goal -> Task -> Evidence 最小工作闭环，包含状态机、乐观锁、阻塞、重试与中断恢复。
- SQLite 迁移 14/15：`goals`、`work_tasks`、`task_evidence`、`work_events`，以及 Run/Event/Tool Call 的可空 Trace 字段。
- 七个 Host-owned Runtime Work Tool 与 Event Schema v1 的 12 类 Goal/Task/Evidence 事件。
- 工作模式确定性门控、用户确认、Goal 进度 UI、Evidence 定位及刷新/重启状态归并。
- Evidence 有效性校验与 stale/missing/invalid 历史保留。
- `diagnose_work_state` 与 `export_work_trace` A0 诊断命令。
- A0 前数据库升级、失败恢复、跨三文件真实任务、负向门控和性能基线自动化测试。

### Known limitations

- A0 不包含 Task 级审批、Plan 修订/版本历史、独立 Reviewer、Acceptance、完整 Span 树、Memory 或并发子 Agent。
- Trace 字段允许为空；Runtime Session 仍为尽力恢复，不保证跨 Runtime 版本兼容。
