# A0 Alpha 发布验收清单

> 状态：工程验收完成，待发布人按脚本录屏签收<br>
> 验收日期：2026-07-30<br>
> 基线：A0-01 至 A0-09 集成分支

## 工程门禁

| 检查 | 命令或证据 | 2026-07-30 结果 |
| --- | --- | --- |
| Rust 格式 | `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check` | 通过 |
| Rust 测试 | `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml` | 158 passed |
| 前端测试 | `bun test ./apps/desktop/tests` | 43 passed；500 Task P95 58.10ms |
| Runtime 测试 | `pnpm.cmd runtime:test` | 48 passed |
| 知识契约 | `pnpm.cmd knowledge:verify-preview:test` | 3 passed |
| 前端构建 | `pnpm.cmd --dir apps/desktop build` | 通过；保留已知大 Chunk warning |
| Runtime 构建/冒烟 | `pnpm.cmd runtime:build`、`pnpm.cmd runtime:smoke` | 通过 |
| 安装包 | `pnpm.cmd tauri:build` | MSI 与 NSIS 均成功 |
| Git 空白错误 | `git diff --check` | 通过 |

实际执行结果必须记录在提交对应的 A0 测试验收报告或发布记录中，不能只勾选清单。

### 本次产物哈希

| 产物 | SHA-256 |
| --- | --- |
| `Fox_0.1.0_x64_en-US.msi` | `D5717E30D241E653A8AFD90D382F88BE96FA31F6B2026200E844CC10F9A80D3B` |
| `Fox_0.1.0_x64-setup.exe` | `6295A9BD51DCF6FF293BCC0603147A6BB311043AE6078792CE24BBEA31405DA4` |
| Runtime Sidecar | `D1F8ACC81C2B6A79462193FB182920D5E4AC8C1C5BFC052793A5AD731E5D361B` |

### 2026-07-31 审查修复候选包

本候选包包含迁移 16、待确认 Run 恢复、独立 Work Event 前端订阅、Evidence 跳转、Work Trace 脱敏和项目浏览路径状态修复。工程复验结果为 Rust 160 passed、前端 46 passed（500 Task P95 43.44ms）、Runtime 48 passed、知识预览契约 3 passed；格式、前端构建、Runtime 构建/冒烟、Tauri 打包和 `git diff --check` 均通过。

| 产物 | 字节 | SHA-256 |
| --- | ---: | --- |
| `Fox_0.1.0_x64_en-US.msi` | 99,106,816 | `A076AEFA5AD9397399693166BCDC671047262F750E3C3097E4147B087555340E` |
| `Fox_0.1.0_x64-setup.exe` | 84,427,961 | `55A5EAF9E16F10D2851E6AF1AA456B51F598462FBD7052E77D812DED6EAFA601` |
| Runtime Sidecar | 99,972,096 | `D1F8ACC81C2B6A79462193FB182920D5E4AC8C1C5BFC052793A5AD731E5D361B` |

录屏签收必须使用本节候选包，并记录最终提交号；上节 2026-07-30 哈希仅作为历史发布证据保留。

## A0 功能验收

- [x] 跨三个文件的修复流程创建定位、修改、验证三个 Task。
- [x] completed Task 均有关联 Evidence；修改 Task 有 file_diff/artifact，验证 Task 有 test_result/tool_call。
- [x] 普通解释、一次性只读和“暂不修改”不会自动创建 Goal。
- [x] 刷新/重启后 Goal、Task、attempt 与 Evidence 从 SQLite 恢复。
- [x] Sidecar/Run 中断不会把 Task 标记为 completed；重试追加 Evidence。
- [x] stale/missing/invalid Evidence 保留历史并可见。
- [x] `diagnose_work_state` 返回结构化不变量检查。
- [x] `export_work_trace` 导出工作图与 Work Event，且不包含对话正文、项目文件或凭证。

## 数据升级与回滚

- [x] 迁移 13 的 A0 前数据库可升级到迁移 16。
- [x] 运行中 Run、终态 Run、异常 Event、附件和知识库绑定数量不变。
- [x] 重复启动迁移幂等。
- [x] 失败升级不登记迁移 14；可恢复升级前备份并重新升级。
- [x] 回滚采用升级前 `.foxbackup`/SQLite 一致性快照与旧版本程序，不执行逆向删表。

演练步骤和测试名见 `docs/06-运维指南/迁移备份与恢复.md`。

## 录屏签收脚本

发布人在干净 Windows 用户环境录制一段连续视频，视频中需显示版本/提交号，并依次完成：

1. 覆盖安装 Alpha 包，打开迁移 13 的样本数据，确认旧会话可读。
2. 执行跨文件修复任务，展开 Goal，展示三个 Task、attempt 和 Evidence 跳转。
3. 中断一个运行中 Task，重启 App，展示 interrupted 状态与重试。
4. 删除测试 Artifact，重新验证 Evidence，展示 missing/stale 警告且历史仍在。
5. 对同一会话运行工作状态诊断并导出 Work Trace。
6. 创建备份、排队恢复并重启，确认数据恢复。
7. 执行普通解释请求，确认不出现 Goal 区域。

签收记录应附视频链接、Alpha 包 SHA-256、样本数据库版本、操作系统版本和验收人。仓库不保存包含真实用户数据或凭证的视频。

## 发布阻断条件

- 任一自动化门禁失败。
- 迁移或回滚演练失败、`integrity_check` 非 `ok`。
- completed Task 无 Evidence，或中断 Task 被显示为 completed。
- Work Trace/诊断包包含凭证、对话正文或项目文件内容。
- 录屏脚本未完成或安装包哈希无法对应本次构建。
