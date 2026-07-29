# Fox 第二阶段实施状态

更新日期：2026-07-18

## 1. 当前结论

第二阶段的功能开发、真实 Yuxi 联调和桌面人工验收已经完成，当前只剩重新构建最终安装包这一项交付工作。

Fox 现在具备两条独立运行链路：

- Fox 原生 Agent 通过 Pi Runtime 工作，不依赖 Yuxi。
- Yuxi Agent 通过远程 Thread、Run 和 SSE 工作；Yuxi 同时向 Fox 提供只读知识库与知识图谱能力。

一个会话创建后始终绑定固定 Agent 和 Runtime。切换 Agent 会创建新会话，不在原会话中替换 Runtime。

## 2. 已完成能力

### 数据与持久化

- SQLite 已完成第二阶段 schema 和 v5 迁移。
- 持久化 Agent、会话、消息、Run、Runtime Session、项目、工具调用、审批、附件、产物、知识绑定和服务连接。
- SQLite 是产品事实来源，Pi Session 只用于运行态恢复，JSONL 只用于 Runtime 进程通信与诊断。
- Yuxi Run 保存远程 Run ID 与 SSE cursor，支持 `Last-Event-ID` 续传。

### 项目、文件工具与审批

- 支持 `read_only`、`ask`、`allow` 三种项目权限模式。
- Rust 负责项目路径、父目录穿越、符号链接和工作目录边界校验。
- 已接入 `write_file`、`edit_file`、`run_command`。
- `ask` 模式和命令执行进入真实审批生命周期，前端使用 AI Elements Confirmation 展示。
- 写入与编辑结果自动形成 Artifact 记录。

### 附件

- Composer 使用 AI Elements Attachments 即时显示待发送附件；发送后附件继续显示在对应用户消息下。
- 前端将浏览器 blob URL 转换为 data URL 后传给 Tauri，单个附件上限统一为 5 MiB。
- SQLite 持久化附件 ID、所属消息、文件名、媒体类型、大小与 SHA-256；创建 Run 时附件会绑定到真实用户消息。
- Fox 原生 Agent 可通过 `read_attachment` 按 ID 读取当前会话的 UTF-8 文本与 DOCX 文本内容。
- `read_attachment` 禁止跨会话访问；拒绝不支持的二进制格式，提取后的模型上下文上限为 1 MiB。
- Yuxi Agent 会先将附件上传到对应远程 Thread，再把 Yuxi `file_id` 写入 Run 的 `meta.attachment_file_ids`。

### Yuxi 服务与身份

- 支持本机、局域网和远程 HTTPS Yuxi 地址。
- Token 使用系统凭证库存储，不写入 SQLite。
- 支持用户名密码登录、当前用户读取、Agent 同步和连接状态展示。
- 登录后侧边栏优先显示真实 Yuxi 用户；不可用时回退为 Fox 本地用户。
- Yuxi Agent 同步会读取详情接口中的 `configurable_items`，只有服务端明确开放 `model` 时才允许会话级模型覆盖；候选项来自 Yuxi 普通用户模型列表接口。

### Yuxi Agent Runtime

- Yuxi Agent 映射为 Fox Agent，创建会话时同步创建远程 Thread。
- 支持远程 Run 创建、SSE 流式消息、推理、工具事件、取消、重命名与删除。
- Yuxi SSE 映射到统一 Fox Runtime Event，不让前端直接依赖 Yuxi 原始事件结构。
- Yuxi Agent 调用 `query_kb` 时，检索结果也会投影为统一 `source.added` 事件，与 Fox 原生 Agent 共用 AI Elements Sources。
- 短暂断线自动重试；失败后标记为可恢复中断。
- 应用重启或重新登录后查询远程 Run 状态，并从已保存 cursor 续接 SSE。
- 已使用真实 Worker 中断场景验证恢复：远程 Run 在 Worker 恢复后完成时，Fox 会重放漏掉的 SSE 事件并恢复完整回复。

### 知识库与图谱

- 已接入普通用户 Viewer API：知识库列表、详情、文档列表、文档内容、检索与图谱子图。
- 知识库页面和图谱页面使用真实 Yuxi 数据。
- 当前会话可选择一个或多个有权访问的知识库，绑定持久化到 SQLite。
- Pi 提供 `list_knowledge_bases`、`search_knowledge`、`read_knowledge_document`。
- Rust 在调用 Yuxi 前校验目标知识库是否绑定到当前会话。
- 检索结果投影为稳定 `source.added` 事件，并使用 AI Elements Sources 渲染来源。

### 产品 UI 闭环

- Agent 列表、Agent 详情、知识库列表、知识库详情、文档查看和图谱页面接入真实数据。
- Agent 卡片可创建绑定该 Agent 的新会话。
- 会话侧边栏支持真实重命名与删除。
- Composer 支持项目授权、附件和知识库绑定。
- Composer 的模型、Agent、项目执行模式和上下文 token 环已接入真实配置与 Runtime 用量；切换 Agent 会创建新会话。
- 对话顶部使用真实会话标题与 Agent 名称；设置页账户使用真实 Yuxi 用户信息并保留本地回退。
- 原有 Kun 风格三栏布局、侧边栏、右侧工具栏、对话列表和 AI Elements 工作过程保持不变。
- 临时 Runtime 调试条已删除；右侧工具面板的最终取舍延后到第三阶段决定。

## 3. 自动验证

- Rust 单元测试：`43/43` 通过，覆盖附件消息绑定、DOCX 提取和 Yuxi 恢复等场景。
- Agent Runtime 测试：`32/32` 通过。
- 前端 TypeScript 与 Vite 生产构建通过。
- `cargo fmt --check` 通过。
- Runtime Sidecar 冒烟测试通过。
- 之前的 NSIS 安装包已生成；本轮附件与恢复修复合入后的最终安装包将在全部人工验收通过后重新构建。
- Vite 仍提示部分代码块大于 500 kB，这是现有体积警告，不影响功能正确性。

## 4. 人工验收状态

Yuxi 使用完整模式运行，不能启用 `LITE_MODE`。当前结果：

| 验收项 | 状态 |
| --- | --- |
| Yuxi 地址、登录、Agent 与模型同步 | 已通过 |
| Fox 与 Yuxi Agent 对话、SSE、推理和工具过程 | 已通过 |
| 会话重命名、删除与历史恢复 | 已通过 |
| 知识库列表、文档内容与图谱浏览 | 已通过 |
| Fox 原生 Agent 知识工具与来源渲染 | 已通过 |
| Worker 中断后的远程 Run 恢复 | 已通过 |
| Yuxi 离线时 Fox 原生 Agent 独立工作 | 已通过 |
| Fox 原生 Agent 附件读取与消息保留 | 已通过 |
| Yuxi Agent 附件上传、读取与消息保留 | 已通过 |
| 右侧工具面板最终取舍 | 延后到第三阶段 |
| 最终安装包 | 待重新构建 |

## 5. 第二阶段总进度

| 工作项 | 状态 |
| --- | --- |
| SQLite schema 与迁移 | 已完成 |
| 项目实体、路径边界与权限模式 | 已完成 |
| 写入、编辑、命令与审批链路 | 已完成 |
| 附件、产物和知识绑定 | 已完成并通过真实附件验收 |
| Yuxi 服务配置、登录与用户信息 | 已完成 |
| Yuxi Knowledge Provider 与 Viewer API | 已完成 |
| Yuxi Runtime Adapter | 已完成 |
| Agent、知识库、图谱与设置页面真实数据 | 已完成 |
| 会话重命名、删除与知识库选择 | 已完成 |
| 来源渲染与远程 Run 恢复 | 已完成并通过真实服务验收 |
| 第二阶段人工验收 | 已完成 |
| Runtime Sidecar | 已完成 |
| 最终 NSIS 安装包 | 待重新构建 |

## 6. 已知后续优化

- 前端主包体积较大，第三阶段可按页面动态拆分 Mermaid、Shiki 和图谱依赖。
- 知识来源目前支持展示，不在第二阶段实现从来源直接跳转到知识库文档位置。
- Fox 原生 Agent 首版支持 UTF-8 文本与 DOCX 提取；将图片原始内容发送给支持视觉能力的多模态模型、PDF 和其他多模态解析明确留到第三阶段。Yuxi Agent 继续复用 Yuxi 自己的附件解析能力。
- 数据库内容加密、SQLCipher 和企业级备份策略留到后续安全增强。
