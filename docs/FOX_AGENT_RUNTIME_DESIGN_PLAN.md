# Fox Agent Runtime 简要设计计划

## 1. 方案结论

Fox 将参考 Craft Agents 的实现方式，采用“稳定的 Fox 运行边界 + 可替换的 Agent Runtime”架构。

- 首版使用 Pi 作为 Fox 原生 Agent 的执行引擎。
- Fox 不自研 Agent Loop、模型流式协议、上下文压缩等已有成熟实现。
- Fox 保留自己的 Agent、会话、事件、权限和工具协议，UI 不直接依赖 Pi 类型。
- Yuxi 主要负责知识库、知识图谱，以及已有 Yuxi Agent 的兼容接入。
- Fox 不连接 Yuxi 时，原生 Agent、模型调用和本地工具仍可使用。
- 未来允许增加或替换其他 Agent Runtime，但当前不规划 Claude Agent SDK 后端。

本文取代旧架构中“Fox 的 Agent Loop 完全由 Yuxi 提供”的设计；Yuxi 不再是 Fox 原生 Agent 的必需运行依赖。

## 2. 整体架构

```text
Fox Desktop
Tauri 2 + React + shadcn/ui
│
├── Fox UI
│   ├── Agent 与会话管理
│   ├── AI Elements 消息和工作过程渲染
│   ├── 工具调用、审批和文件变更展示
│   └── 知识库与知识图谱页面
│
├── Fox Application Layer
│   ├── AgentService
│   ├── ConversationService
│   ├── RuntimeRegistry
│   ├── ToolRegistry
│   ├── PermissionService
│   └── KnowledgeService
│
├── Tauri Runtime Host (Rust)
│   ├── Sidecar 生命周期管理
│   ├── 授权目录与文件系统边界
│   ├── 工具执行前审批与路径校验
│   ├── 凭证安全存储
│   └── 系统能力与进程恢复
│
├── Agent Runtime
│   ├── PiRuntimeAdapter（首版默认）
│   │   └── Fox Pi Sidecar
│   ├── YuxiRuntimeAdapter（兼容已有 Yuxi Agent）
│   └── FutureRuntimeAdapter（预留，不指定具体 SDK）
│
└── Knowledge Provider
    └── YuxiKnowledgeProvider
        ├── 知识库与文档
        ├── RAG 检索
        └── 知识图谱
```

## 3. 职责边界

### 3.1 Pi 负责

- Agent Loop 与模型工具调用循环。
- 多模型 Provider 和流式响应。
- Agent Session、取消和 Steering。
- 上下文压缩与基本历史管理。
- 模型及思考级别切换。
- 工具调用决策、参数生成、基础工具实现和 Tool Result 回灌。

Fox 优先复用 Pi 的 SDK 层，不采用 Pi 自带的终端 UI、产品提示词和 Coding Agent 产品界面。第一阶段的只读文件工具复用 Pi 实现，但每次执行前必须等待 Fox/Rust 返回 `allow`、`block` 或修改后的参数。写入、编辑和 Bash 暂不注册。第二阶段再按风险决定是否继续复用 Pi、迁移到 Rust ToolHost 或增加操作系统沙箱。

### 3.2 Fox 负责

- 定义稳定的 `FoxAgent`、`FoxConversation` 和 `FoxRuntimeEvent`。
- 将会话绑定到固定 Agent 和 Runtime。
- Agent Runtime 的选择、启动、停止、恢复和替换。
- 工具注册、启用策略和产品级权限审批。
- 用户授权目录及文件访问边界。
- 控制工具注册、执行前审批、路径范围和参数修改。
- 消息、工具过程、审批、错误和用量的统一事件映射。
- UI 会话记录及 Pi Session ID 映射。
- Yuxi 知识工具和其他 Fox 专属工具。

### 3.3 Yuxi 负责

- 知识库列表、文件和只读内容。
- 知识检索、引用来源和 RAG。
- 知识图谱浏览、搜索和后续来源跳转。
- Yuxi Agent 的创建、编辑、删除和管理。
- 作为可选 Runtime，为 Fox 提供已有 Yuxi Agent 对话能力。

## 4. 稳定抽象

Fox UI 只能使用 Fox 自己的领域类型：

```text
Pi Events    -> PiRuntimeAdapter    -> FoxRuntimeEvent -> UI
Yuxi Events -> YuxiRuntimeAdapter  -> FoxRuntimeEvent -> UI
Other SDK   -> FutureRuntimeAdapter -> FoxRuntimeEvent -> UI
```

Runtime 接口在概念上需要支持：

- 创建和恢复 Session。
- 发送消息并返回流式事件。
- 取消、Steering 和上下文压缩。
- 动态切换允许修改的模型和运行参数。
- 注册工具并转发工具执行结果。
- 销毁 Session 和释放资源。

统一事件至少覆盖：

- 文本与思考增量。
- 工具开始、更新、完成和失败。
- 用户审批或补充输入请求。
- 用量与上下文变化。
- Run 完成、取消和错误。

## 5. Sidecar 设计

Pi 运行在独立的 Fox Sidecar 中，不进入 React 渲染进程，也不直接嵌入 Tauri Rust 业务代码。

```text
React UI
  -> Tauri Command/Event
  -> Rust RuntimeHost
  -> stdin/stdout JSONL
  -> Fox Pi Sidecar
  -> Pi SDK / Model Provider
```

首版使用 JSONL 作为本机进程协议，避免占用端口和额外本地鉴权。Rust 负责：

- 启动、停止和监控 Sidecar。
- 为每个请求分配关联 ID。
- 转发流式事件。
- 处理崩溃、超时和取消。
- 限制环境变量、工作目录和可访问路径。

生产打包前再根据体积和兼容性选择 Bundled Bun、独立可执行文件或 Node Runtime。

## 6. 工具与权限

工具分为三类：

| 类型 | 示例 | 执行位置 |
| --- | --- | --- |
| Pi 只读工具 | Read、Grep、Find、Ls | Pi 执行，Fox/Rust 在执行前审批并校验路径 |
| 高风险主机工具 | Write、Edit、Bash | 第一阶段不注册；第二阶段决定 Pi、Rust ToolHost 或沙箱方案 |
| Fox 本地工具 | 项目授权、桌面能力、文件审批 | Tauri/Rust 或 Fox Tool Host |
| Yuxi 知识工具 | 检索知识库、打开文档、查询图谱 | Fox 调用 Yuxi API |

所有危险工具都必须先经过 Fox 的权限层。Pi 可以提出工具调用，但不能绕过 Fox 的目录授权、审批和安全策略。

权限分成三层：

| 层级 | 作用 | 是否为硬边界 |
| --- | --- | --- |
| Prompt 和 Agent 策略 | 告诉模型当前模式与允许行为 | 否，属于行为引导 |
| Tool Registry 和审批 | 决定哪些工具可见、是否需要用户确认 | 是，调用级强制 |
| Rust 审批与路径校验 | 在 Pi 工具执行前返回允许、拒绝或修改后的参数 | 是，调用级强制；不是完整进程沙箱 |

仅设置 Sidecar 工作目录不能限制它访问其他路径，因此不能把 `cwd` 当作安全沙箱。第一阶段只注册 Pi 的只读工具，并通过工具包装器确保 Rust 审批完成后才调用 Pi 原始 `execute`。针对写入、命令以及恶意或被攻陷 Sidecar 的风险，在第二阶段评估 Rust ToolHost 和操作系统级隔离。

首版建议保留三种执行方式：

- 只读：只能读取、搜索和分析。
- 询问：写入、执行命令或敏感访问前要求用户确认。
- 允许：在当前已授权项目范围内自动执行允许的操作。

## 7. Yuxi 知识接入

Yuxi 通过 `KnowledgeProvider` 接口接入，不与 Pi Runtime 强绑定。首批工具建议为：

- `list_knowledge_bases`
- `search_knowledge`
- `read_knowledge_document`
- `search_knowledge_graph`
- `get_graph_neighbors`

调用过程：

```text
模型请求知识工具
-> Fox ToolRegistry
-> 权限和知识库访问检查
-> YuxiKnowledgeProvider
-> Yuxi API
-> 工具结果返回 Pi
-> Pi 继续生成回答
```

Yuxi 断开后，这些工具变为不可用，但 Fox 原生 Agent 和本地工具继续工作。

Fox 首版只配置一个 Yuxi 服务。配置包含服务名称和规范化 Base URL，既可以指向本机 Docker 的 `http://127.0.0.1:5050`，也可以指向局域网或远程 HTTPS 地址。健康检测统一请求 `<baseUrl>/api/system/health`；Token 不写入 SQLite，而是按服务地址保存到系统安全存储。切换 Base URL 时不得自动复用其他服务的凭证。

知识库文档和检索结果一律按不可信外部内容处理：

- 工具结果携带来源和内容边界，明确标记为资料而不是系统指令。
- 检索内容不能扩大 Agent、项目或工具权限。
- 内容中的指令即使影响模型决策，后续工具调用仍必须经过 Tool Registry 和执行前审批；高风险工具启用后还需经过其确定的宿主执行或沙箱边界。
- 对高风险写入和命令执行保留显式审批，不因知识库内容声称“已获得授权”而放行。
- Yuxi 超时或不可用时返回标准工具失败事件，Agent 可以降级回答，但不能伪造检索结果。

## 8. Agent 与会话模型

Fox 对用户统一展示 `FoxAgent`，内部记录执行来源：

```text
FoxAgent
├── runtimeType: pi
├── runtimeType: yuxi
└── runtimeType: future
```

关键规则：

- 一个会话从创建开始始终绑定一个 Agent。
- Agent 决定行为、提示词、工具和默认模型。
- 知识库决定本次会话允许使用的资料范围。
- 用户只能修改 Agent 明确标记为可配置的运行参数。
- 切换 Agent 时创建新会话，不在原会话中替换 Runtime。
- Agent 和 Runtime 绑定不可在会话内更换；模型是否可以临时切换由该 Agent 的可配置字段决定。这是有意的产品取舍，不等于所有会话都禁止切换模型。

## 9. 数据与历史记录存储

Fox 采用混合存储，不把所有数据都交给 SQLite，也不把 Pi Session 或 JSONL 当作产品数据库。

```text
Fox Data Directory
├── fox.db                     # Fox 产品数据
├── runtime/
│   └── pi-sessions/           # Pi 的运行状态与恢复文件
├── events/                    # 可选 Runtime 原始 JSONL 事件
├── attachments/               # 用户附件
├── artifacts/                 # Agent 生成的产物
├── logs/                      # 应用和 Sidecar 日志
└── cache/                     # 可安全重建的缓存
```

### 9.1 SQLite：产品事实来源

`fox.db` 是 Fox 本地产品数据的唯一事实来源，主要保存：

- Agent、会话、消息和 Run。
- 工具调用、审批和用户补充输入。
- 项目授权、知识库绑定和运行参数。
- Runtime 类型、Runtime Session ID 及恢复状态。
- 附件、产物和引用的元数据，不保存大文件二进制内容。

Fox 使用自己的稳定 ID，并单独记录底层 Runtime ID：

```text
FoxConversation.id          # Fox 会话 ID
FoxConversation.agentId     # 固定绑定的 Fox Agent
FoxConversation.runtimeType # pi / yuxi / future
RuntimeSession.runtimeId    # Pi Session ID 或 Yuxi Thread ID
```

底层 Runtime 被替换或升级后，Fox 的会话列表、消息历史和用户数据仍然可用。

### 9.2 Pi Session：运行恢复数据

Pi Session 只用于：

- Agent 上下文和执行状态恢复。
- 上下文压缩、分支和 SDK 检查点。
- Pi 内部需要的工具调用历史。

Pi Session 不是 Fox UI 的历史来源，也不能成为 Fox 的数据格式。Fox 应将 Pi 事件标准化并持久化到 SQLite。

### 9.3 JSONL：原始事件与诊断

Fox 可按会话保存 Runtime 原始事件：

```text
events/{conversationId}.jsonl
```

JSONL 主要用于崩溃恢复、问题诊断、协议回放和适配器测试。正常 UI 读取 SQLite，不直接扫描 JSONL。原始事件日志应支持保留期限和清理策略，避免长期无限增长。

### 9.4 附件、产物与凭证

- 附件和产物保存为普通文件，SQLite 只记录路径、类型、大小、哈希和所属会话。
- API Key、OAuth Token 和 Yuxi Token 不以明文写入 SQLite、JSONL 或日志。
- 凭证优先保存到 Windows Credential Manager、macOS Keychain 或 Linux Secret Service，必要时再评估 Tauri Stronghold。
- Yuxi 服务端数据以 Yuxi 为事实来源，Fox 只缓存列表、展示信息和必要的离线数据。
- `fox.db` 第一阶段依赖操作系统账户权限和磁盘加密保护；进入正式敏感数据场景前，需要根据威胁模型决定是否引入 SQLCipher 等库级静态加密。

### 9.5 生命周期原则

- 删除会话时同步清理其 Runtime 映射、可删除的 Pi Session、附件和事件日志。
- 产物导出到用户指定目录后，不随会话删除而删除导出副本。
- 数据迁移以 Fox SQLite schema 为主，不依赖 Pi 内部文件格式升级。
- 备份至少包含 `fox.db`、附件、产物和必要的 Runtime Session；日志和缓存默认不进入备份。Runtime Session 仅作尽力恢复，不保证跨 Pi 版本可用，Fox 标准历史不受此限制。

## 10. 实施阶段

对应的详细阶段文档：

- [第一阶段：Runtime 基础链路设计与实施方案](./FOX_PHASE_1_RUNTIME_FOUNDATION.md)
- [第二阶段：产品能力与 Yuxi 接入设计和实施方案](./FOX_PHASE_2_PRODUCT_AND_YUXI_INTEGRATION.md)
- [第三阶段：扩展能力与长期稳定性设计和实施方案](./FOX_PHASE_3_EXTENSIBILITY_AND_HARDENING.md)

### 第一阶段：Runtime 验证

- 定义 Fox Agent、Session、Tool 和 Runtime Event 契约。
- 创建最小 Pi Sidecar，跑通流式对话、主机工具代理和取消。
- 实现 Tauri RuntimeHost 与 JSONL 通信。
- 将 Pi 事件转换为 Fox UI 事件。
- 建立最小 SQLite schema，验证 Fox 会话 ID 与 Pi Session ID 的独立映射。
- 使用现有 Fox 对话界面完成端到端演示。

### 第二阶段：产品级基础能力

- 接入会话持久化、恢复和 Agent 绑定。
- 持久化消息、Run、工具调用、审批、附件和产物元数据。
- 完成授权目录、工具审批和文件变更展示。
- 接入模型配置、凭证存储和运行错误恢复。
- 接入 YuxiKnowledgeProvider 和首批知识工具。
- 接入 Yuxi Agent 列表及 `YuxiRuntimeAdapter`。

### 第三阶段：完善与扩展

- 完成知识库文件页和知识图谱浏览。
- 增加 Skills、MCP 和更多通用工具。
- 增加 Sidecar 自动恢复、版本迁移和运行诊断。
- 完成数据备份、迁移、保留期限和会话级清理机制。
- 根据真实需求增加其他 Runtime Adapter。
- 评估远程 Fox Runtime，但不影响本地 Pi Runtime。

## 11. 关键工程原则

1. 不在 Fox 中重新实现 Agent Loop。
2. UI 不直接依赖 Pi、Yuxi 或未来 SDK 的原始事件。
3. Pi 是首版实现，不是 Fox 的永久协议。
4. 权限和文件边界必须由 Fox 控制。
5. Yuxi 是可选知识服务，不能成为 Fox 原生对话的启动前提。
6. 首版只实现真正需要的 Runtime 接口，不提前建设复杂插件框架。
7. 固定并审查 Pi 版本，通过适配器契约测试控制升级风险。
8. 新增 Runtime 时只增加 Adapter，不修改 UI 消息模型和核心会话流程。
9. SQLite 是 Fox 产品数据的事实来源，Pi Session 只服务于运行恢复。
10. 大文件进入文件系统，凭证进入系统安全存储，不能混入消息数据库或日志。
11. 所有工具必须经过 Fox 控制的注册和执行前审批；高风险主机工具需由 Rust ToolHost、操作系统沙箱或等价机制提供更强约束。
12. 外部知识、网页和文件内容均视为不可信数据，不能改变工具权限和安全策略。

## 12. 暂不包含

- 自研模型 Provider 协议和 Agent Loop。
- Claude Agent SDK 及 Claude 专属 Runtime。
- 多 Runtime 同时参与一个会话。
- 子 Agent 编排和复杂工作流。
- Fox 中的知识库上传、解析、分块和 Embedding 管理。
- 定时任务和 IM 机器人。
