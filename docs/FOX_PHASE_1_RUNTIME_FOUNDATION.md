# Fox 第一阶段：Runtime 基础链路设计与实施方案

## 1. 阶段目标

第一阶段的目标不是完成全部产品功能，而是证明 Fox 可以在不依赖 Yuxi 的情况下，使用 Pi 完成一条稳定、可恢复、可替换的本地 Agent 链路。

阶段完成后，用户应能在现有 Fox 界面中：

- 选择一个 Fox 原生 Agent 并创建会话。
- 使用配置的模型进行流式对话。
- 查看思考过程、工具调用和最终回复。
- 在授权项目内使用基础只读文件工具。
- 停止生成，关闭并重新打开应用后恢复历史。

## 2. 本阶段范围

### 包含

- Fox Runtime、Session、Tool 和 Event 的稳定契约。
- Pi Runtime Adapter 和最小 Pi Sidecar。
- Tauri RuntimeHost 与 Sidecar 生命周期管理。
- stdin/stdout JSONL 本地进程协议。
- SQLite 最小数据模型和迁移机制。
- Fox 会话 ID 与 Pi Session ID 独立映射。
- 对话 UI 从 Mock 数据切换到真实 Runtime 事件。
- 基础错误、取消和进程异常恢复。

### 不包含

- Yuxi 知识库和知识图谱接入。
- Yuxi Agent 对话兼容。
- 文件写入、命令执行和完整审批流程。
- 可配置的自动审批规则、授权记忆和“始终允许”能力；这些能力后置，不阻塞首版纯文本链路。
- Skills、MCP、子 Agent 和其他 Runtime。
- 云端同步、多设备同步和远程 Fox Runtime。

虽然 Yuxi 的知识库、图谱和 Agent 业务能力仍属于第二阶段，但第一阶段提前包含其连接基础：Fox 只保存一个 Yuxi 服务配置，允许使用本机、局域网或远程 HTTPS 地址，并提供公开健康检查、版本识别和系统凭证库 Token 存储。该连接不能成为 Fox 原生 Agent 启动的前置条件。

## 3. 阶段架构

```text
Fox UI
  -> ConversationService
  -> PiRuntimeAdapter
  -> Tauri RuntimeHost
  -> Pi Worker Sidecar
  -> Pi SDK
  -> Model Provider

Pi Event
  -> JSONL
  -> RuntimeHost
  -> PiRuntimeAdapter
  -> FoxRuntimeEvent
  -> SQLite + UI
```

### 3.1 当前工程基线

当前 Fox 桌面端已经完成工作台和主要静态页面，但运行层仍是空壳：

- React 工作台集中在 `apps/desktop/src/features/chat/workbench.tsx`，消息和会话来自 Mock 数据。
- Tauri Rust 当前只负责启动窗口，尚无 Command、事件流、数据库和进程管理。
- 项目中尚无 SQLite、Runtime Client、Sidecar 或稳定的对话领域模型。
- AI Elements 和 shadcn/ui 已具备消息、思考过程和工具过程的表现组件，可以直接承接标准化事件。

第一阶段保持现有 UI 外观不变，新增数据层和 Runtime 层，逐步把 Mock 行为替换为真实行为。

### 3.2 组件职责

| 组件 | 第一阶段职责 |
| --- | --- |
| React UI | 展示会话、消息、思考、工具状态和运行错误；提交用户操作 |
| Conversation Store | 保存当前窗口的临时视图状态，不作为持久化事实来源 |
| Tauri Commands | 提供会话查询、发送、取消、模型设置和 Runtime 状态接口 |
| Conversation Service | 组织数据库事务、Runtime 调用和 UI 事件发布 |
| Runtime Registry | 根据 Runtime 类型创建对应 Adapter；第一阶段只注册 Pi |
| PiRuntimeAdapter | 将 Fox 命令和事件映射为 Sidecar JSONL 协议 |
| RuntimeHost | 启停 Worker、读写 stdio、超时、崩溃检测和资源回收 |
| SQLite Repository | 保存 Fox 产品数据、幂等事件和 Runtime Session 映射 |
| Pi Worker Sidecar | 加载 Pi SDK、创建 Agent Session、执行 Agent Loop 和已批准的只读工具 |
| Rust Approval Host | 在 Pi 工具执行前校验工具、项目路径并返回审批决定 |

依赖方向保持单向：

```text
UI -> Tauri Command -> Application Service -> Runtime Adapter -> RuntimeHost -> Sidecar
                                      |
                                      -> Repository -> SQLite
```

Sidecar 不能直接写 `fox.db`，React 也不能直接读取 Pi Session 文件。

### 3.3 Worker 进程策略

首版建议按活动会话懒加载 Pi Worker：

- 会话开始运行时启动对应 Worker。
- 未活动会话只保留 SQLite 和 Pi Session 文件，不常驻进程。
- Worker 空闲一段时间后可以退出，需要时再恢复。
- 单个 Worker 崩溃只影响对应活动会话，不影响 Fox UI 和其他会话。
- RuntimeHost 的内部结构按多个 Worker 设计，但第一阶段策略限制为最多一个温热 Worker；这是明确的首版并发和隔离决定。
- 切换会话时先等待当前 Worker 完成持久化，再按 LRU 策略关闭或替换 Worker。

若后续测试证明多进程启动成本明显，再评估单 Sidecar 管理多 Session，不在第一阶段提前复杂化。

该策略参考 Craft 的每 Session Pi 子进程隔离方式，但 Fox 第一阶段不实现 Craft 的后台任务和多会话并行能力。

## 4. 关键设计

### 4.1 Fox Runtime 契约

UI 和业务层只依赖 Fox 契约，不接触 Pi 原始类型。概念能力包括：

- 创建、恢复和销毁 Runtime Session。
- 发送消息并订阅流式事件。
- 取消当前 Run。
- 更新模型和允许开放的运行参数。
- 查询 Session 和 Run 状态。

统一事件至少包括：

- `message.delta`
- `reasoning.delta`
- `tool.started`
- `tool.updated`
- `tool.completed`
- `usage.updated`
- `run.completed`
- `run.cancelled`
- `run.failed`

事件需要带有 `conversationId`、`runId`、顺序号和时间戳，以支持去重、恢复和按顺序持久化。

Runtime Adapter 还需要声明能力，而不是让 UI 根据 `pi` 名称写判断。第一阶段能力清单包括：

| 能力 | Pi 首版 |
| --- | --- |
| 流式文本 | 支持 |
| 思考过程 | 模型支持时启用 |
| 只读文件工具 | 支持 |
| 写入和 Bash | 不注册 |
| 取消 | 支持 |
| Steering | 协议预留，首版可暂不开放 UI |
| 自动压缩 | 使用 Pi 能力 |
| 手动压缩 | 协议预留 |
| 动态模型切换 | 支持受控切换 |
| 图片和附件 | 暂不支持 |

### 4.2 Runtime 与 Run 状态机

Worker 状态：

```text
stopped -> starting -> ready -> busy -> ready
             |          |       |
             v          v       v
           crashed <------------+

ready/busy -> stopping -> stopped
```

Run 状态：

```text
queued -> running -> completed
                  -> cancelling -> cancelled
                  -> failed
                  -> interrupted
```

规则：

- 用户消息和 `queued` Run 必须先落库，再向 Sidecar 发起执行。
- 收到 `run.started` 后切换为 `running`。
- `completed`、`cancelled` 和 `failed` 是终态。
- 应用异常退出时，残留的 `running` 或 `cancelling` Run 在下次启动时标记为 `interrupted`。
- 第一阶段不自动重放被中断的用户消息，避免重复工具调用；用户确认后发起新的 Run。
- Worker 崩溃与模型请求失败分开记录，便于 UI 给出不同恢复操作。

### 4.3 JSONL 进程协议

协议名称暂定为 `fox-runtime-jsonl`，首版版本号为 `1`。stdout 只能输出协议 JSONL，所有调试日志必须写入 stderr。

统一包络包含：

| 字段 | 说明 |
| --- | --- |
| `protocol` | 固定为 `fox-runtime-jsonl` |
| `version` | 协议主版本 |
| `kind` | `request`、`response` 或 `event` |
| `id` | 当前消息唯一 ID |
| `requestId` | 响应关联的请求 ID |
| `conversationId` | Fox 会话 ID |
| `runtimeSessionId` | Pi Session ID，可在创建前为空 |
| `runId` | 当前 Run ID |
| `seq` | 同一 Run 内单调递增序号 |
| `timestamp` | UTC ISO 时间 |
| `type` | 具体命令或事件名称 |
| `payload` | 具体数据 |

握手顺序：

```text
RuntimeHost starts process
-> host.hello
<- runtime.ready(protocolVersion, runtimeVersion, capabilities)
-> session.open(create or resume)
<- session.ready(runtimeSessionId)
```

握手失败、协议主版本不兼容或超时后，Worker 进入 `crashed`，不继续发送 Prompt。

首版协议按请求和事件两类消息组织。

主要请求：

- `initialize`
- `create_session`
- `resume_session`
- `prompt`
- `cancel`
- `set_model`
- `shutdown`

主要响应与事件：

- `ready`
- `session_created`
- `runtime_event`
- `session_updated`
- `request_succeeded`
- `request_failed`
- `fatal_error`

每条消息必须包含协议版本和关联 ID。未知字段允许忽略，不兼容的协议版本必须明确拒绝。

协议附加约束：

- 单行消息设置大小上限，超大工具结果必须改用文件引用或摘要。
- Sidecar 对每个 Run 维护独立递增序号。
- Rust 按 `runId + seq` 去重，拒绝同一 Run 内倒退的关键状态事件。
- 未知事件记录到诊断日志，但不能导致应用崩溃。
- stderr 保留有限大小的环形缓冲，用于连接失败和崩溃诊断。
- `shutdown` 应优先进行会话 flush，超时后 RuntimeHost 才强制终止进程。

### 4.4 工具桥接边界

第一阶段只注册以下只读工具：

- `read`
- `grep`
- `find`
- `ls`

工程师评审指出的关键隐藏假设是：Pi 是否能够在工具真正执行前暂停并等待宿主决定。Craft 的实现已经证明该流程可行，但其做法不是依赖一个抽象的全局审批 Hook，而是包装传入 Pi Session 的工具定义，在调用原始 `execute` 前发送 `pre_tool_use_request` 并等待主进程返回。

第一阶段采用接近 Craft 的工具包装方案：复用 Pi 的只读工具实现，但包装每个工具的 `execute`。包装器必须先请求 Rust 审批，只有收到 `allow` 或修改后的参数后才调用 Pi 原始工具实现。

```text
Pi Tool Call
-> wrapped execute sends tool.approval_requested
-> Rust validates tool + path + project
-> tool.approval_result(allow / block / modify)
-> Sidecar calls the original Pi tool only when allowed
-> Pi tool returns Tool Result
-> Sidecar emits standardized tool events
```

第一阶段不注册 `write`、`edit` 和 `bash`，防止 UI 审批和目录安全尚未完成时提前获得写能力。

实施时先完成纯文本对话、持久化和取消，再进入只读工具里程碑。只读工具阶段只实现 Fox 固定的项目目录校验策略，不提供用户可配置的自动审批规则；成熟的 `allow / ask / deny` 策略引擎、授权记忆和自动审批界面放到后续阶段。

路径检查至少包含：

- 转换为绝对规范路径。
- 验证路径位于当前项目根目录内。
- 拒绝通过 `..`、符号链接或 Windows 路径前缀越界。
- 工具参数缺少路径或路径无法解析时默认拒绝。

Pi Worker 是 Fox 固定版本并随应用打包的受信任进程，但不能因此被视为完整安全边界。第一阶段由 Fox RuntimeHost/Rust Approval Host 强制完成调用前审批，Pi 只执行获准的只读工具。

权限边界分为：

- Prompt 约束：行为引导，不构成安全保证。
- Tool Registry：决定 Runtime 能看到和调用哪些工具。
- Rust Approval Host：校验规范路径、权限模式和工具参数，并返回 `allow`、`block` 或 `modify`。
- Pi 只读工具：审批通过后执行实际读取和搜索。
- Rust ToolHost：第二阶段针对写入、编辑和命令工具评估。
- 操作系统沙箱：约束 Sidecar 进程本身，后续作为纵深防御评估，不能用 `cwd` 或审批机制替代。

第一阶段明确的威胁模型是防止模型、Prompt Injection 和工具参数通过已注册工具越过 Fox 权限；不宣称能够抵御恶意 Runtime 二进制。若未来允许下载第三方 Runtime，操作系统级沙箱将成为上线前置条件。

#### 4.4.1 Pi 审批能力技术验证

在接入真实 Pi 前必须完成以下验证：

1. Pi 工具的 `execute` 是否可以被可靠包装，Pi 不会绕过包装器直接执行同名内置工具。
2. 包装器是否可以异步等待任意时长的宿主审批结果。
3. 等待审批期间是否可以响应取消和进程退出。
4. `allow`、`block` 和修改参数三种结果能否在调用原始工具前正确生效。
5. 多个并行工具调用能否通过 `toolCallId` 独立关联。
6. 默认工具是否可以被替换为 Fox 包装后的同名工具，确保模型只看到受控版本。

若其中任何一项不满足，第一阶段不开放该工具；不得降级为“执行后再上报”。

### 4.5 SQLite 最小模型

第一阶段只建立必要表：

| 表 | 用途 |
| --- | --- |
| `agents` | Fox 原生 Agent 的基本信息和 Runtime 类型 |
| `conversations` | 会话、Agent 绑定、标题和状态 |
| `messages` | 标准化后的用户和助手消息 |
| `runs` | 一次发送对应的运行状态和错误 |
| `runtime_sessions` | Fox 会话与 Pi Session 的映射 |
| `run_events` | 需要持久化的标准里程碑事件和幂等键 |
| `schema_migrations` | 数据库版本管理 |

Pi Session 文件只负责 Runtime 恢复，不能替代 `messages` 和 `conversations`。

建议的关键约束：

- `conversations.agent_id` 创建后不可修改。
- `runtime_sessions.conversation_id` 和当前有效 Session 保持唯一映射。
- `messages` 使用 Fox 自己的 UUID，不复用 Pi Message ID。
- `run_events` 对 `(run_id, seq)` 建立唯一约束。
- 所有时间保存为 UTC，UI 再按本地时区展示。
- JSON 扩展字段只存非核心元数据，关键查询字段必须是独立列。

事件写入事务：

```text
receive sidecar event
-> validate and normalize
-> begin SQLite transaction
-> insert run_events for deduplication
-> update messages / runs / runtime_sessions projections
-> commit
-> publish FoxRuntimeEvent to React
```

UI 事件必须在事务提交后发布，避免界面显示了数据库尚未保存的状态。

流式 Delta 不需要每个字符都插入一行：

- Delta 实时转发给 UI。
- Rust 按时间或字符数合并并检查点更新 `messages.content`。
- 消息结束、Run 终态和工具里程碑必须立即落库。
- Worker 崩溃时保留最后一次内容检查点，并将消息标记为 `interrupted`。

#### 4.5.1 数据库归属与实现

第一阶段由 Rust 持有唯一 SQLite 连接池或串行连接，建议使用 `rusqlite`：

- Tauri Command 通过 Repository 读写数据库。
- Sidecar 不获得数据库路径，也不连接 `fox.db`。
- React 不引入 `tauri-plugin-sql`，防止数据规则分散到前端。
- 开启 WAL、foreign keys 和合理的 busy timeout。
- 所有 schema 变化通过内置顺序迁移执行。

数据库位置使用 Tauri 应用数据目录，例如：

```text
<app-data>/Fox/fox.db
```

Pi Session、日志和缓存与数据库同属 Fox 应用数据根目录，但各自独立子目录。

#### 4.5.2 核心字段草案

`agents`：

- `id`
- `name`
- `description`
- `runtime_type`
- `system_prompt`
- `default_model`
- `created_at`
- `updated_at`

`conversations`：

- `id`
- `agent_id`
- `title`
- `project_root`
- `status`
- `created_at`
- `updated_at`
- `last_message_at`

`messages`：

- `id`
- `conversation_id`
- `run_id`
- `role`
- `kind`
- `content`
- `status`
- `ordinal`
- `runtime_message_id`
- `metadata_json`
- `created_at`
- `updated_at`

`runs`：

- `id`
- `conversation_id`
- `runtime_session_id`
- `status`
- `model`
- `started_at`
- `finished_at`
- `error_code`
- `error_message`
- `last_seq`

`runtime_sessions`：

- `id`
- `conversation_id`
- `runtime_type`
- `runtime_session_id`
- `runtime_version`
- `session_path`
- `status`
- `created_at`
- `updated_at`

`run_events`：

- `id`
- `run_id`
- `seq`
- `event_type`
- `event_json`
- `created_at`

工具调用第一阶段作为 `messages.kind = tool` 和事件保存；第二阶段再拆为独立 `tool_calls` 表，避免第一阶段 schema 过度扩张。

#### 4.5.3 标准消息模型

第一阶段持久化的消息种类：

| `role` | `kind` | 用途 |
| --- | --- | --- |
| `user` | `text` | 用户输入 |
| `assistant` | `text` | 助手最终或流式回复 |
| `assistant` | `reasoning` | 可展示的思考摘要或推理增量 |
| `tool` | `tool_call` | 工具调用、参数摘要和状态 |
| `tool` | `tool_result` | 工具结果摘要 |
| `system` | `notice` | 中断、恢复和运行错误提示 |

Fox 不要求 Runtime 提供完整私有思维链。只有 Runtime 明确标记为可展示的 reasoning 内容才进入 `reasoning` 消息。

#### 4.5.4 标题策略

第一阶段不为标题生成单独模型请求：

- 新会话标题先取首条用户消息的单行截断。
- 用户可以后续手动重命名。
- 使用小模型自动生成标题放到第二阶段评估。

这样可以减少首版模型配置和额外失败路径。

#### 4.5.5 完整历史与 Runtime 压缩

Pi 的上下文压缩只影响 Pi 下一轮发送给模型的运行上下文，不允许改写 Fox SQLite 中已经保存的原始用户消息、助手消息和工具里程碑。

- Fox 保存完整标准历史。
- Pi 保存运行所需的 Session、摘要和压缩状态。
- 压缩摘要可以作为 Runtime 元数据或系统 Notice 保存，但不能替代原始消息。
- UI 查看历史时始终读取 Fox SQLite，而不是读取 Pi 压缩后的上下文。
- Runtime 替换或 Pi Session 失效时，可以从 Fox 历史构建有限的恢复上下文。

### 4.6 Pi 接入边界

优先复用 Pi 的以下能力：

- Agent Loop。
- 多模型流式调用。
- Session 和上下文压缩。
- 取消、Steering 和模型切换基础能力。
- 自定义工具注册、Tool Call 和 Tool Result 回灌能力。

第一阶段不使用 Pi 的终端 UI、产品提示词、Agent 管理界面和默认 Coding Agent 产品行为。Fox 提供自己的 Agent 配置和系统提示词。

正式选定依赖前需要验证：

- 官方上游包与维护分支的差异。
- 许可证和桌面分发条件。
- Windows Sidecar 打包兼容性。
- Session 文件格式和版本升级行为。

Pi 依赖选择设置一个技术决策门：优先验证官方 `@mariozechner/pi-*`；只有官方版本缺少 Fox 必需且无法通过薄 Adapter 补齐的能力时，才评估 Craft 使用的维护分支。该决策需要记录版本、许可证、缺失能力和迁移成本。

#### 4.6.1 Pi 版本兼容策略

- Pi 包固定精确版本，不使用宽松版本范围自动升级。
- Sidecar 构建时记录 Pi 包版本、Fox Runtime 协议版本和能力指纹。
- Pi 原始事件只在 Sidecar 内出现，`pi/event-mapper` 负责转换为固定的 JSONL v1 事件。
- Pi 升级必须先运行 Adapter 契约测试和保存的回放样例，确认事件、工具、取消和 Session 行为。
- 新增 Pi 字段允许忽略；缺失必需字段、语义变化或未知终态必须阻止升级。
- 若旧 Pi Session 无法恢复，Fox 保留标准历史并创建新 Runtime Session。

### 4.7 UI 事件映射

现有 AI Elements 和 shadcn/ui 组件继续作为表现层：

```text
Pi 原始事件
-> PiRuntimeAdapter
-> FoxTimelineItem
-> Message / ChainOfThought / Tool 等组件
```

组件不能读取 Pi Event，也不能把 Pi Session 当作 UI 状态源。

React 中的 Zustand 只保存以下临时状态：

- 当前会话和可见消息。
- 正在流式生成的增量内容。
- 当前工具展开状态和滚动位置。
- 输入框草稿和面板布局。

会话列表、完整消息、Run 状态和 Runtime 映射必须从 Tauri Application Service 查询，不能只存在于 Zustand 或 `localStorage`。

### 4.8 Tauri Command 与事件面

第一阶段建议提供以下桌面命令：

| Command | 作用 |
| --- | --- |
| `runtime_initialize` | 初始化数据库、迁移和 RuntimeHost |
| `agents_list` | 获取 Fox 原生 Agent |
| `conversations_list` | 获取会话列表 |
| `conversation_create` | 创建绑定 Agent 的会话 |
| `conversation_load` | 加载消息和最后运行状态 |
| `run_start` | 持久化用户消息并启动 Run |
| `run_cancel` | 取消当前 Run |
| `runtime_status` | 查询 Worker 和 Runtime 状态 |
| `conversation_delete` | 删除会话及相关内部数据 |

运行事件通过一个持续订阅通道发送给 React。UI 在应用初始化时订阅一次，所有事件携带 `conversationId` 和 `runId`，由前端按当前视图路由。

Command 返回统一结果结构：

```text
success: { ok: true, data }
failure: { ok: false, error: { code, message, retryable, details? } }
```

Rust 内部错误不能直接序列化为任意字符串暴露给 UI。调试细节进入脱敏日志，UI 只接收稳定错误码和可操作信息。

第一阶段 UI Controller 建议拆为：

```text
features/conversations/
├── api/desktop-client.ts
├── model/types.ts
├── store/conversation-store.ts
├── hooks/use-runtime-events.ts
└── controllers/conversation-controller.ts
```

`workbench.tsx` 只接收当前会话、时间线、运行状态和回调，不继续承担会话模拟状态机。

### 4.9 启动、恢复与关闭流程

应用启动：

```text
open database
-> run migrations
-> repair interrupted runs
-> start RuntimeHost without Worker
-> seed default Fox Agent if absent
-> return conversations and runtime availability
```

打开会话不会立即启动 Worker。用户发送消息时才：

```text
persist user message + queued run
-> resolve agent/runtime config
-> start or resume Worker
-> send prompt
-> persist and publish events
```

应用关闭时：

- 禁止接受新 Run。
- 请求活动 Worker 取消或 flush。
- 等待有限时间写入 Pi Session 和 SQLite。
- 正常关闭 Worker；超时后强制终止并把 Run 标记为 `interrupted`。
- SQLite 关闭前执行 WAL checkpoint。

### 4.10 关键业务时序

#### 4.10.1 创建会话

```text
React conversation_create(agentId, projectRoot?)
-> Rust validates agent and project
-> insert conversation
-> return FoxConversation
```

创建会话不启动 Worker，也不创建 Pi Session。Pi Session 在第一次发送时按需建立。

#### 4.10.2 发送消息

```text
React run_start(conversationId, text, model?)
-> validate conversation is idle
-> transaction: insert user message + queued run
-> commit
-> emit user.message.persisted
-> RuntimeRegistry selects PiRuntimeAdapter
-> RuntimeHost starts/resumes Worker
-> send prompt
-> process normalized events transactionally
-> finalize assistant message and Run
```

如果 Worker 启动失败，用户消息保留，Run 标记 `failed`，用户可以重试，不删除输入内容。

#### 4.10.3 取消运行

```text
React run_cancel(runId)
-> atomically mark cancelling
-> send cancel to Worker
-> wait for run.cancelled or timeout
-> timeout: terminate Worker and mark interrupted
```

取消操作必须幂等。对已经完成的 Run 再次取消返回当前终态，不创建新错误。

#### 4.10.4 应用重启恢复

```text
startup repair
-> running/cancelling runs become interrupted
-> partial assistant messages remain visible as interrupted
-> runtime session mapping remains available
-> next user send attempts Pi Session resume
```

若 Pi Session 无法恢复：

- 不删除 Fox 历史。
- 将原映射标记为 `unavailable`。
- 创建新 Pi Session。
- 第一条新 Prompt 注入有限的 Fox 历史恢复摘要或近期消息。

第一阶段可以先只注入最近若干条消息；复杂的结构化恢复摘要放到后续迭代。

#### 4.10.5 会话删除

```text
ensure no active run
-> stop associated Worker
-> transaction: delete Fox database rows
-> delete Pi Session directory and optional JSONL events
-> publish conversation.deleted
```

删除文件失败不能回滚数据库删除，但需要记录待清理项，由下次启动重试。

### 4.11 错误分类

Fox 第一阶段至少区分：

| 错误类型 | 示例 | 用户动作 |
| --- | --- | --- |
| `runtime_unavailable` | Sidecar 不存在或无法启动 | 查看诊断、重新启动 |
| `protocol_error` | JSONL 损坏或版本不兼容 | 更新或重新安装 Runtime |
| `provider_auth` | API Key 无效 | 前往模型设置 |
| `provider_rate_limit` | 模型限流 | 稍后重试 |
| `provider_network` | 网络不可用 | 检查网络并重试 |
| `session_restore_failed` | Pi Session 不兼容或损坏 | 创建新 Runtime Session |
| `tool_denied` | 工具或路径被 Fox 拒绝 | 调整项目或请求范围 |
| `storage_error` | SQLite 或磁盘写入失败 | 检查磁盘和数据目录 |
| `cancelled` | 用户主动停止 | 可继续发送新消息 |

Sidecar 原始错误先由 PiRuntimeAdapter 转为 Fox 错误类型，再进入 UI；UI 不解析 Provider 错误字符串。

### 4.12 不可信内容与 Prompt Injection

第一阶段读取的项目文件也可能包含恶意或误导性指令。Fox 将所有文件内容视为数据，而不是新的系统指令：

- 文件内容以明确的工具结果边界返回模型，并保留来源路径。
- 文件中的文字不能修改 Agent 权限、项目根目录和工具白名单。
- 模型即使受内容诱导发起新的工具调用，也必须重新经过 Fox/Rust 执行前审批和路径校验。
- Tool Result 中声称“用户已批准”或“忽略之前规则”不构成授权依据。
- 敏感工具的批准只能来自 Fox 当前会话的真实审批状态。
- 超大或二进制内容在进入模型前进行限制、摘要或文件引用，避免上下文和日志失控。

Prompt Injection 不能只靠提示词彻底解决，Fox 的主要防线是工具最小化、宿主执行、逐次校验和权限不随内容变化。

## 5. 推荐目录

```text
Fox/
├── apps/desktop/
│   ├── src/features/runtime/
│   ├── src/features/conversations/
│   └── src-tauri/src/runtime_host/
├── services/agent-runtime/
│   └── src/
│       ├── protocol/
│       ├── pi/
│       ├── sessions/
│       └── main.ts
└── packages/contracts/
    └── src/
        ├── runtime/
        ├── agents/
        └── conversations/
```

若第一阶段不需要跨包共享大量代码，保持包数量最少；只有真正被桌面端和 Sidecar 同时使用的类型才进入 `packages/contracts`。

Rust 侧建议进一步分为：

```text
src-tauri/src/
├── app_state.rs
├── commands/
│   ├── conversations.rs
│   └── runtime.rs
├── database/
│   ├── migrations.rs
│   ├── repositories.rs
│   └── models.rs
├── runtime_host/
│   ├── manager.rs
│   ├── worker.rs
│   ├── jsonl.rs
│   └── state.rs
└── services/
    └── conversation_service.rs
```

Rust 持有 SQLite 与 Worker，避免 React 直接操作数据库或子进程。

### 5.1 RuntimeHost 内部接口

RuntimeHost 对上层只暴露与具体 Runtime 无关的能力：

- `ensure_worker(conversation_id, runtime_spec)`
- `send_request(worker_id, request)`
- `cancel_run(worker_id, run_id)`
- `stop_worker(worker_id, reason)`
- `status(worker_id)`
- `shutdown_all()`

`WorkerHandle` 内部维护：

- 子进程句柄和 stdin 写队列。
- stdout JSONL reader task。
- stderr 环形缓冲。
- Worker 状态和最后活动时间。
- 待响应请求映射与超时计时器。
- 当前 Conversation、Runtime Session 和 Run。

每个 Worker 的 stdin 写入必须串行，避免多条 JSON 拼接。stdout reader 只负责解析和投递，不在读循环中执行耗时数据库操作，以防阻塞模型流。

### 5.2 PiRuntimeAdapter 责任

PiRuntimeAdapter 位于 Fox Application Service 与 RuntimeHost 之间，负责：

- 将 Fox Agent 配置转换为 Sidecar 初始化配置。
- 将 Fox Session 操作转换为 JSONL Request。
- 将 Pi Worker Event 转换为 `FoxRuntimeEvent`。
- 维护 Pi 工具名到 Fox 工具名的映射。
- 将 Pi/Provider 错误转换为稳定 Fox 错误码。
- 过滤 Pi 特有但 Fox 不支持的事件和字段。
- 暴露 Pi Capability Manifest。

Adapter 不直接操作 React Store，也不自行绕过 Conversation Service 写数据库。

### 5.3 Sidecar 内部模块

```text
services/agent-runtime/src/
├── main.ts                 # stdin/stdout 与生命周期
├── protocol/
│   ├── envelopes.ts
│   ├── requests.ts
│   └── events.ts
├── pi/
│   ├── session.ts          # create/resume/dispose Agent Session
│   ├── event-mapper.ts     # Pi Event -> Sidecar Event
│   ├── model-registry.ts
│   ├── tools.ts
│   └── errors.ts
└── diagnostics/
    └── logger.ts           # stderr only
```

Sidecar 第一阶段不包含 SQLite、Yuxi Client、UI 逻辑和产品会话列表。

### 5.4 配置分层

配置分为三层：

| 配置 | 示例 | 所有者 |
| --- | --- | --- |
| Agent 配置 | system prompt、默认模型、允许工具 | Fox SQLite |
| 模型连接配置 | provider、endpoint、credential reference | Fox 设置与安全存储 |
| Runtime 启动配置 | session path、cwd、协议版本、能力开关 | RuntimeHost 临时生成 |

API Key 不通过普通日志或数据库传递。第一阶段具体凭证注入方式在模型连接设计时确定，优先使用受限环境变量或一次性 stdin 初始化字段，并确保 Sidecar 不回显。

### 5.5 UI 渐进迁移

为避免破坏现有工作台，UI 按以下顺序迁移：

1. 提取现有时间线所需的 `FoxTimelineItem`。
2. 使用现有 Mock 数据生成相同的标准类型，验证外观不变。
3. 引入 Desktop Client 和 Runtime Event 订阅。
4. 用 SQLite 会话列表替换左侧 Mock 会话。
5. 用真实 Run 替换 Composer 的计时器模拟。
6. 保留审批、问题和错误的 Mock 开关，直到第二阶段真实能力接入。

第一阶段完成后，聊天主路径不再包含基于关键词模拟 `question`、`approval` 和 `error` 的逻辑。

## 6. 实施顺序

1. 完成 Pi 包版本、许可证、Session API 和 Windows 打包技术验证。
2. 固化 Fox Runtime Event、Capability Manifest 和 JSONL v1 协议。
3. 先用 Fake Sidecar 验证 Rust RuntimeHost、超时、取消和异常协议处理。
4. 建立 SQLite schema、WAL、迁移、Repository 和启动修复逻辑。
5. 建立真实 Pi Sidecar，跑通纯文本流式对话和 Session 恢复。
6. 加入只读工具包装和 `tool.preflight_required` 权限往返。
7. 完成 PiRuntimeAdapter、事件标准化、幂等落库和 UI 订阅。
8. 从 `workbench.tsx` 提取 Conversation Controller，用真实数据替换聊天 Mock。
9. 接入会话创建、列表、切换、取消和重启恢复。
10. 补充崩溃、超时、重复事件、脏 JSONL、进程退出和数据库迁移测试。
11. 完成 Windows 开发包、安装包和首次启动冒烟测试。

### 6.1 第一阶段内部里程碑

| 里程碑 | 可验证结果 |
| --- | --- |
| P1：协议骨架 | Fake Sidecar 可以发送文本、错误和完成事件 |
| P2：持久化骨架 | 可以创建会话、保存消息、重启后恢复列表 |
| P3：Pi 文本链路 | 真实模型完成多轮流式对话 |
| P4：只读工具 | Agent 能在授权项目内读取和搜索文件 |
| P5：UI 接入 | 现有工作台使用真实事件，不再模拟提交结果 |
| P6：打包验证 | 安装包能够启动 Sidecar 并恢复历史 |

当前实施状态（2026-07-15）：

- P1 已建立 `fox-runtime-jsonl v1`、Fake Sidecar 和子进程协议测试。
- P2 已建立 Rust SQLite 迁移、默认 Agent、会话/消息/Run 仓储、事件幂等投影和启动中断修复。
- React 已增加 Tauri Desktop Client 与 Runtime Event 订阅；普通 Web 预览仍保留原 Mock 界面，Tauri 环境启用真实持久化基础链路。
- 真实 `fox-pi-runtime`、OpenAI-compatible 模型适配和 Pi 事件映射已经实现；Fake Runtime 仅作为显式开发与协议测试入口保留。固定 Pi 依赖尚未在当前工作区安装，因此真实 Pi Agent Loop 仍待依赖恢复后验收。
- 自动审批规则引擎明确后置；当前代码不保存“始终允许”等授权规则，也不开放写入、编辑或 Bash。
- 已建立单一 Yuxi 服务配置：SQLite 通过 `singleton_id = 1` 强制单例，支持本机 HTTP、私有局域网 HTTP 与远程 HTTPS，记录 `/api/system/health` 的版本与延迟；访问 Token 按规范化服务地址隔离保存到系统凭证库。
- Yuxi 连接测试在存在 Token 时继续访问受保护的 `/api/agent`，验证 `Authorization: Bearer` 凭证；表单中尚未保存的 Token 也能直接参与测试，更换地址后会清理旧地址凭证。
- 已验证当前本机 Yuxi `http://127.0.0.1:5050` 的公开健康接口返回版本 `0.7.0`。
- Tauri 会话页面现在按 SQLite 顺序渲染完整多轮用户和助手消息；发送给 Runtime 的 Prompt 也携带最近 40 条标准消息，为真实 Pi 多轮上下文接入准备好边界。
- JSONL 已支持 Sidecar 主动向 Rust 发起 `tool.preflight` 请求。协议测试证明 Sidecar 会在 Host 返回审批结果前暂停工具流程，并能按 `toolCallId` 继续关联后续事件。
- Rust 已实现第一批只读工具的固定守卫：仅允许 `read`、`grep`、`find`、`ls`，要求会话绑定已授权项目目录，规范化路径并拒绝未知工具、缺失路径、无法解析路径和项目外路径。该守卫已接入 RuntimeHost 的双向 JSONL 通道，Fox 自定义只读工具也已注册到真实 Pi Tool Registry；真实 Pi 端到端执行仍待依赖安装后验收。
- RuntimeHost 不再把 Runtime 名称和能力写死为 Fake 值，而是读取握手返回的 Runtime 名称、版本和 Capability Manifest；协议时间戳统一为 UTC ISO 8601。
- Runtime 启动、握手、请求或进程异常会回收失效 Worker，并将活动 Run 投影为稳定的 `run.failed` 事件；后续发送可以重新创建 Worker。
- Tauri 发布配置已声明 Runtime 资源，并将生产入口固定为 Bun compile 的独立 Sidecar；发布版不会回退到系统 Node。当前尚未下载 Bun，因此独立 Runtime EXE 与 NSIS 安装包仍未完成验证。
- Yuxi 地址规则进一步收紧：本机和私有局域网允许 HTTP，公网或远程域名必须使用 HTTPS；Token 仍按规范化 Base URL 隔离保存在系统凭证库。
- 当前自动化验证为 Rust 19 项、Runtime 协议/Fake Sidecar/真实 Pi Faux Provider Agent Loop/事件映射/Session/工具执行器 14 项，前端 TypeScript、Vite 生产构建、Rust Clippy 和 Release 编译通过；另已使用正在运行的本机 Yuxi 完成真实健康检查，并确认受保护接口要求 Bearer 凭证。
- 真实会话现已从 SQLite 返回标准化 Runtime Events；React 会按 Run 关联实时和历史 `reasoning.*`、`tool.*` 事件，并复用 AI Elements 的 `ChainOfThought` 与 `Tool` 组件展示工作过程。
- 对话输入区现可授权现有本机文件夹并创建绑定目录的新会话，从而让只读工具在正常产品流程中获得 Rust 校验后的项目边界；取消状态也只允许活动 Run 进入，并在 Runtime 拒绝时收敛为 `interrupted`。

尚未完成且不能以 Fake Runtime 代替的第一阶段项目：

- 安装已固定为 `@earendil-works/pi-agent-core` 与 `@earendil-works/pi-ai` `0.79.9` 的依赖，并运行 Pi Faux Provider 契约测试。
- 使用真实 OpenAI-compatible 模型验证多轮流式调用、取消和四个只读工具的端到端行为。
- 通过 Pi Faux Provider 和真实模型验证 Pi Tool Registry 无法绕过 Rust `tool.preflight` 包装器。
- 使用已选定的 Bun compile 方案生成独立 Sidecar，使安装后的 Fox 不依赖用户机器预装 Node。
- 完成 NSIS 安装包、含空格/中文安装路径、首次启动和重启恢复的干净机器验证。

2026-07-15 尝试从 npm 官方注册表查询 `@mariozechner/pi-agent-core`、`@mariozechner/pi-ai` 和 `@mariozechner/pi-coding-agent` 时，当前宿主的网络审批服务连续返回 `503`。在拿到官方元数据和源码前，不猜测 Pi 的版本或 API，也不把 Craft 的内部依赖当作官方契约。

### 6.2 开发任务切片

建议按可独立验收的小任务实施：

| 编号 | 任务 | 依赖 |
| --- | --- | --- |
| F1 | Runtime Contracts 与协议样例 | 无 |
| F2 | Fake Sidecar 与协议测试夹具 | F1 |
| F3 | Rust RuntimeHost 和 Worker 状态机 | F1、F2 |
| F4 | SQLite migrations 与 Repository | F1 |
| F5 | Conversation Service 与 Tauri Commands | F3、F4 |
| F6 | Pi 技术验证与真实 Sidecar | F1 |
| F7 | PiRuntimeAdapter 和错误映射 | F3、F6 |
| F8 | 只读工具 Preflight | F3、F6 |
| F9 | React Desktop Client 和事件订阅 | F1、F5 |
| F10 | Workbench 真实会话迁移 | F7、F9 |
| F11 | 恢复、取消和崩溃测试 | F5、F7、F10 |
| F12 | Tauri Sidecar 打包验证 | F6、F11 |

在 F6 得出 Pi 包和运行时选择结论前，F1-F5 与 F9 可以使用 Fake Sidecar 推进，不让外部依赖选择阻塞整体开发。

## 7. 交付物

- 可打包运行的 Fox Pi Sidecar。
- Tauri RuntimeHost。
- Fox Runtime Contracts。
- PiRuntimeAdapter。
- 最小 SQLite 数据库和迁移机制。
- 一个默认 Fox 原生 Agent。
- 真实流式对话、历史恢复和取消功能。
- Runtime 契约测试与端到端冒烟测试。

## 8. 验收标准

- Fox 未连接 Yuxi 时可以正常完成多轮对话。
- UI 不引用 Pi 的原始事件和 Session 类型。
- 应用重启后，会话列表和标准消息历史完整保留。
- Pi Session 丢失时，Fox 历史仍可查看，并能明确提示重新创建 Runtime Session。
- 用户取消后，模型流和工具执行停止，Run 状态正确落库。
- Sidecar 崩溃不会导致桌面进程退出。
- 同一事件重复到达时不会产生重复消息。
- Windows 开发环境和打包环境都能启动 Sidecar。
- stdout 中出现非 JSON 日志时能够隔离并报告协议错误，不污染后续消息。
- 数据库写入失败时不向 UI 宣布 Run 已完成。
- 同一 Run 的事件顺序在 UI、SQLite 和诊断记录中一致。

## 9. 主要风险

- Pi 包版本变化影响协议或 Session 恢复。
- Bun、Node 或独立可执行文件的打包体积和杀毒软件误报。
- 流式事件顺序、取消和异常退出造成状态不一致。
- Pi 内置 Coding Agent 行为渗入 Fox 通用 Agent 产品。

应对方式是固定版本、保持 Adapter 边界、建立事件契约测试，并在第一阶段只开放最小工具集合。

## 10. 测试设计

### 10.1 协议契约测试

- 正常握手和版本不兼容。
- 响应关联 ID 正确与缺失。
- 重复、乱序和未知事件。
- 非 JSON stdout、超大行和进程提前退出。
- 取消与完成事件竞争。

### 10.2 Pi Adapter 契约测试

- Pi 文本、思考、工具、用量、完成和错误事件映射为稳定 Fox 事件。
- Pi 新增未知事件时可忽略并保留诊断，不破坏现有流。
- 必需字段缺失、未知终态或事件语义变化时测试失败并阻止升级。
- 自定义工具在 Rust 返回结果前不会继续执行原始主机 I/O。
- 并行 Tool Call 按 `toolCallId` 正确匹配请求和结果。
- 审批等待期间取消能够中止等待和当前 Run。
- Pi Session 创建、恢复、损坏和版本不兼容路径符合预期。
- 保存的 Pi Event 回放样例在升级前后得到相同 Fox Event Projection。

### 10.3 RuntimeHost 测试

- Worker 懒启动、空闲退出和 LRU 替换。
- 启动超时、强制终止和 stderr 环形缓冲。
- 会话切换前 flush。
- 应用关闭期间拒绝新 Run。

### 10.4 数据库测试

- 全新数据库和连续 schema 迁移。
- `(run_id, seq)` 幂等约束。
- 事件事务失败时 Projection 不更新。
- 异常退出后的 `interrupted` 修复。
- 删除会话的关联清理。

### 10.5 UI 测试

- Delta、思考、工具和终态的渲染。
- 切换会话时事件不会进入错误页面。
- 重启后从 SQLite 恢复标准历史。
- 取消、失败和 Sidecar 崩溃状态可见。

### 10.6 打包测试

- Windows 开发运行和安装包运行。
- 安装路径含空格和中文。
- 首次启动无数据库、无 Pi Session 和无模型凭证。
- 应用升级后旧数据库和旧 Session 的兼容提示。

## 11. 参考 Craft 的边界

第一阶段可以借鉴 Craft 的以下成熟做法：

- Pi 运行在独立子进程，主进程通过 JSONL 通信。
- 子进程 stdout 专用于协议，stderr 用于有限诊断缓冲。
- Runtime 事件经过 Adapter 转为产品统一事件。
- 工具执行前向主进程请求权限决定。
- 产品会话 ID 与 Pi Session ID 分离。
- Pi Session 用于运行恢复，产品历史由应用自己维护。

Fox 第一阶段不照搬 Craft 的以下范围：

- Claude Agent SDK 双后端。
- 后台任务、多会话并发和消息渠道。
- Browser、MCP、Skills 和 Session 自管理工具。
- 完整自动化、远程 Server 和复杂分支迁移。

## 12. 后续决策门

下列问题在技术验证得到数据后再决定；如果取舍会影响产品或长期架构，需要用户确认：

1. 官方 Pi 包是否满足需要，还是必须评估 Craft 使用的维护分支。
2. 生产包使用 Bundled Bun、Node Runtime 还是编译后的独立 Sidecar。
3. 第一批正式支持的模型连接类型和默认模型。
4. Worker 空闲退出时间及是否允许保留多个温热 Worker。

这些决策不阻塞协议、数据库、Fake Sidecar 和 RuntimeHost 的前置设计。
