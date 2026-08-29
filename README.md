# Fox Agent

Fox 是一个面向个人和小型技术团队的桌面智能体工作台。它使用 React、Tauri/Rust、SQLite 和独立 Node Agent Runtime，把本地项目、模型、知识库、Skills、MCP/OpenAPI 工具、专家能力与可恢复工作流放在统一的 Host 权限边界内。

Fox 不把 Agent 理解为“会调用几个工具的聊天机器人”。一次需要执行的工作会形成可追踪的 `Goal → Plan → Task → Attempt → Evidence → Review → Acceptance` 闭环：模型负责理解和决策，Host 负责权限、状态和执行，数据库保存可恢复事实，独立 Reviewer 复核高风险结果。

## 核心能力

### 可执行、可恢复的 Agent

- 普通问答保持轻量；复杂任务才进入 Goal、Plan Revision 和 Task 工作模式。
- 每次任务执行都有独立 Attempt、预算、状态、证据和错误原因。
- 支持中断、取消、重试、应用重启恢复、重复请求幂等和迟到事件对账。
- 有界只读 Graph Lead 可以拆分最多三个节点，让无依赖节点并行、后继节点等待前置验收。
- Child Run 使用独立 Conversation、Run、上下文、工具范围和资源预算。
- 高风险节点由隐藏的只读 Reviewer 独立检查；Reviewer 通过后仍需 Lead 完成节点，整张 Graph 通过机械重验后才能完成 Goal。

### 受治理的 Prompt、上下文与工具

- Prompt Registry 管理稳定指令、版本、内容 Hash 和实验边界。
- Typed Context 区分 Host 事实、用户输入、知识内容和工具结果，外部内容不能冒充系统指令。
- Execution Profile 决定不同 Run 能看到哪些工具、能否写入、能否委派以及采用什么验证策略。
- Runtime 工具清单、Host 协议和冻结的 Run Profile 必须一致；未知工具、未知权限和身份冲突默认拒绝。

### 本地项目与工具执行

- 会话可以绑定本地项目，读取文件、搜索内容、编辑文件、运行命令和测试。
- 路径经过规范化和项目根校验，阻止绝对路径、`..` 与符号链接逃逸。
- 写文件、命令、MCP 等能力由 Tauri Host 执行，并按项目策略或高风险规则请求审批。
- Tool Call、审批决定、执行结果、错误和证据进入 SQLite，便于审计与恢复。

### 知识、文档与扩展

- 连接外部知识库服务，支持查询、知识图谱、引用定位、原文件预览和下载。
- 独立的本地知识工作区支持文件导入、解析、分块、检索、引用和资源管理。
- 支持 PDF、DOCX、PPTX、XLSX、Markdown、文本、代码和常见图片预览。
- 通过 Skills、MCP、OpenAPI 与声明式 Lifecycle Hook 扩展能力；凭据由系统 Keyring 管理。

### 专家、团队与数字同事

- Assistant、Expert 和 Worker 职责分离；会话可以在基础助手上叠加一个受约束专家。
- 专家能力包支持版本、Hash、导入导出、升级和回滚。
- 专家工作流保存 Stage、Gate 与恢复状态，串行 Supervisor Team 将成员映射为独立 Child Run。
- 数字同事可以冻结专家版本、项目和知识范围，并通过受控 Schedule 或签名 Channel 触发工作。

### 桌面工作台

- 提供会话搜索、重命名、归档恢复、回收站、Fork、附件、引用、工具过程和产物记录。
- 管理项目权限、模型供应商、知识库、专家、Skills、MCP/OpenAPI、插件和诊断信息。
- 通过实时事件与 SQLite 快照双通道归并状态，应用刷新或重启后仍能恢复会话与运行事实。

## 安全与可信执行

模型不能直接访问操作系统。它只能提出结构化工具请求，请求必须依次通过工具白名单、Execution Profile、项目路径、参数约束、审批策略和 Host 状态校验，才会由 Tauri Host 执行。

Fox 的安全不是只靠“限制工具数量”，而是多层约束：

1. **能力最小化**：Assistant、Expert、Worker、Child 与 Reviewer 获得不同工具集合，最终权限取 Host 与各层声明的交集。
2. **Host 强制执行**：Runtime、模型输出、知识内容和 MCP 返回都视为不可信，不能自行写数据库或操作系统。
3. **路径和参数边界**：所有本地操作绑定授权项目或受管文件记录，不能靠文本路径扩大范围。
4. **高风险审批**：命令、MCP 和敏感写入需要明确授权，批准绑定具体 Tool Call，执行时不能替换参数。
5. **证据化完成**：模型说“完成了”不等于完成；任务必须具备有效 Evidence，高风险结果还要经过独立 Reviewer 与 Host Acceptance。
6. **失败默认关闭**：未知工具、权限、Schema、Profile、过期版本、审批超时和恢复冲突都拒绝执行。
7. **可审计与可恢复**：调用、审批、状态转换、Review 和 Acceptance 都持久化，重复执行和崩溃恢复不能绕过安全门。

面向答辩与非安全专业读者的完整说明见 [Agent 安全与可信执行](docs/01-项目概览/Agent安全与可信执行.md)，工程边界见 [安全架构](docs/02-架构/安全架构.md)和[权限参考](docs/04-技术参考/权限参考.md)。

## 系统结构

```text
React 工作台
    ↓ Tauri Command / Event
Rust Host ── SQLite 产品事实
    ↓ JSONL Runtime 协议
Node Agent Runtime ── 模型供应商
    ↓ Host Tool 请求
Rust Host ── 项目文件 / 命令 / 知识库 / MCP / OpenAPI / Keyring
```

| 模块 | 职责 |
| --- | --- |
| `apps/desktop` | React 前端、Tauri Host、SQLite Repository、权限和系统集成 |
| `services/agent-runtime` | 模型循环、Prompt、Typed Context、Execution Profile、工具协议和离线 Eval |
| `scripts` | 跨模块契约、Fixture、知识预览校验和构建辅助 |
| `docs` | 产品介绍、架构、协议、开发指南、运维与路线图 |

`web/Yuxi-main-feat-chat-ai-elements-b0d86e8-20260812` 是锁定版本的外部参考快照，不属于 Fox pnpm 工作区或产品运行链路。

## 开始使用

环境要求和 Windows 依赖以[开发环境搭建](docs/05-开发指南/开发环境搭建.md)为准。

```powershell
pnpm install
pnpm dev
```

常用验证命令：

```powershell
pnpm desktop:test
pnpm runtime:test
pnpm runtime:eval
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --offline
```

生产构建与 Sidecar 打包见[构建与发布](docs/06-运维指南/构建与发布.md)。

## 阅读项目

- [产品概览](docs/01-项目概览/产品概览.md)：产品定位、用户与核心场景。
- [能力矩阵](docs/01-项目概览/能力矩阵.md)：已经实现的能力及边界。
- [系统概览](docs/01-项目概览/系统概览.md)：组件关系、数据流和恢复过程。
- [Agent 安全与可信执行](docs/01-项目概览/Agent安全与可信执行.md)：安全机制与答辩口径。
- [总体架构](docs/02-架构/总体架构.md)：前端、Host、Runtime 与数据层架构。
- [Agent 运行时架构](docs/02-架构/Agent运行时架构.md)：Agent 循环、Profile、Prompt 和工作闭环。
- [App 产品能力完善清单](docs/07-路线图/App产品能力完善清单.md)：页面、文件体验和 UI 的完善顺序。
- [完整文档导航](docs/README.md)：按角色和工作内容查找全部文档。

项目仍有明确边界，包括 Agent 产物打开链路、部分页面占位、导航历史、跨平台验收、CSP 和进程级隔离等，详见[已知限制](docs/07-路线图/已知限制.md)。
