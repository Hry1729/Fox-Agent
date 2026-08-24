# Fox Agent

Fox 是一个以 React、Tauri/Rust、SQLite 和独立 Node Agent Runtime 构建的桌面智能体工作台。它把本地项目、模型、知识、Skills、MCP/OpenAPI 工具、可恢复工作流、专家能力和数字同事放在统一的 Host 权限边界内。

## 开始使用

环境要求和 Windows 依赖以[开发环境搭建](docs/05-开发指南/开发环境搭建.md)为准。常用命令：

```powershell
pnpm install
pnpm dev
pnpm desktop:test
pnpm runtime:test
```

生产构建与 Sidecar 打包见[构建与发布](docs/06-运维指南/构建与发布.md)。

## 文档入口

- [完整文档导航](docs/README.md)
- [产品概览](docs/01-项目概览/产品概览.md)
- [当前能力矩阵](docs/01-项目概览/能力矩阵.md)
- [总体架构](docs/02-架构/总体架构.md)
- [开发工作流](docs/05-开发指南/开发工作流.md)
- [Agent 能力路线图](docs/07-路线图/Agent能力路线图.md)
- [已知限制](docs/07-路线图/已知限制.md)

## 仓库边界

- `apps/desktop`：React 前端与 Tauri Host。
- `services/agent-runtime`：Pi Agent 适配、Prompt Harness、工具协议与 Sidecar。
- `scripts`：跨模块契约、Fixture 和离线 Smoke。
- `docs`：常青文档、路线图、架构决策与历史归档。
- `web/Yuxi-main-feat-chat-ai-elements-b0d86e8-20260812`：锁定版本的外部参考快照，不属于 Fox pnpm 工作区或产品运行链路。

`node_modules`、`dist`、Rust `target`、`.runtime-*` 和本地 Agent 指令目录均为可再生或机器本地产物，不应提交。
