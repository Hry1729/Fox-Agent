# Fox 第三阶段：扩展能力与长期稳定性设计和实施方案

## 1. 阶段目标

第三阶段建立在前两阶段已经稳定可用的产品上，重点完善知识图谱、扩展生态、Runtime 可替换能力、数据治理和生产级稳定性。

本阶段不要求一次性完成所有扩展，而是建立可以持续增加能力且不破坏现有架构的边界。

## 2. 本阶段范围

### 包含

- 知识图谱节点浏览、搜索和关系展开。
- 后续的知识来源跳转和图谱辅助检索。
- 知识库远程原文件的只读预览与本地下载，详见 [知识库远程文件预览与下载实施计划](./FOX_KNOWLEDGE_FILE_PREVIEW_AND_DOWNLOAD_PLAN.md)。
- Skills 的发现、查看、启用和运行注入。
- MCP Server 的连接、工具目录和权限控制。
- Runtime Adapter 注册和兼容性校验机制。
- Sidecar 自动恢复、升级和诊断。
- 数据备份、迁移、保留期限和清理。
- 大型历史、附件、产物和日志的性能治理。
- Fox 原生 Agent 的多模态图片理解与附件能力声明。
- 根据真实需求接入其他非 Claude Runtime。

### 候选能力

- 远程 Fox Runtime。
- 本地模型服务。
- 跨设备同步。
- 长任务和后台运行。
- 子 Agent 或任务委派。

候选能力必须单独评估，不因进入第三阶段而自动实施。

### 不包含

- Claude Agent SDK 和 Claude 专属 Runtime。
- 同一会话同时运行多个 Runtime。
- 在 Fox 中复制 Yuxi 的知识库管理后台。
- 未经权限审查的第三方工具自动安装和执行。

## 3. 扩展架构

```text
Fox Application Layer
├── RuntimeRegistry
│   ├── PiRuntimeAdapter
│   ├── YuxiRuntimeAdapter
│   └── OtherRuntimeAdapter
├── ToolRegistry
│   ├── Built-in Tools
│   ├── Knowledge Tools
│   ├── Skill Tools
│   └── MCP Tools
├── CapabilityPolicy
└── DiagnosticsService
```

扩展能力必须进入现有 Registry 和 Fox 契约，不允许 Runtime、Skill 或 MCP 直接修改 UI Store 或绕过权限层。

## 4. Runtime 扩展机制

### 4.1 Adapter 要求

新的 Runtime Adapter 至少需要实现：

- Session 创建、恢复和销毁。
- 消息流式执行和取消。
- 统一 Runtime Event 映射。
- 工具注册或工具代理。
- 模型和可配置参数能力声明。
- 错误分类、运行状态和诊断信息。

每个 Runtime 需要提供 Capability Manifest，例如：

- 是否支持工具。
- 是否支持图片。
- 是否支持思考过程。
- 是否支持 Steering。
- 是否支持上下文压缩。
- 是否支持动态切换模型。
- 是否支持 Session 恢复。

UI 根据能力声明展示控制项，不根据 Runtime 名称写特殊判断。

### 4.3 多模态图片理解

Fox 原生 Agent 在第三阶段接入真正的图片输入，而不只保存或展示图片附件：

- 根据模型与 Provider 的 Capability Manifest 判断是否支持视觉输入。
- OpenAI 兼容协议映射为标准图片内容块或图片 URL/data URL 内容块。
- Anthropic Messages 协议映射为 `image` source 内容块。
- 图片在进入 Runtime 前执行格式、尺寸、文件大小和像素上限校验。
- 支持的图片格式、数量和限制由模型能力与 Fox 安全策略共同决定。
- 不支持视觉能力的模型不发送图片二进制，并在发送前明确提示用户切换模型或使用其他解析方式。
- 会话历史继续由 Fox 保存稳定附件元数据，不把 Provider 私有消息结构作为事实来源。
- Yuxi Agent 仍使用 Yuxi 的附件与 OCR/解析链路，Fox 不重复实现其服务端逻辑。

### 4.2 兼容性策略

- Runtime 协议独立版本化。
- Adapter 必须通过统一契约测试。
- Runtime 升级前执行 Session 和事件兼容测试。
- 无法恢复旧 Session 时，保留 Fox 历史并提供创建新 Runtime Session 的迁移路径。
- 新 Runtime 不得要求修改已有消息表和 UI 时间线模型，除非先升级 Fox 稳定契约。
- Runtime Adapter 使用统一黑盒契约测试，至少验证握手、Capability Manifest、Session、事件序号、消息生命周期和终态唯一性。

## 5. Skills 设计

Skills 首先作为 Agent 指令和资源包，而不是任意代码插件。

首批能力：

- 扫描配置目录中的 Skill。
- 读取名称、描述、版本和所需工具。
- 在 Agent 或会话级启用。
- 将 Skill 指令按需注入 Runtime。
- 展示来源、校验状态和权限要求。

后续若允许 Skill 携带可执行代码，需要增加签名、来源信任、沙箱和供应链审查，不与纯指令 Skill 混为一类。

## 6. MCP 设计

MCP 由 Fox 统一管理连接和工具目录：

```text
MCP Server
-> Fox MCP Manager
-> ToolRegistry
-> PermissionService
-> Runtime Tool Proxy
```

关键要求：

- MCP Server 配置和凭证分离存储。
- 工具 schema 经过校验和规范化。
- 每个 MCP Server 可以单独启用、禁用和查看状态。
- 工具调用继续经过 Fox 权限模式和审计记录。
- 连接异常不能拖垮 Agent Runtime。
- 工具数量过多时采用搜索或按需暴露，避免上下文膨胀。

## 7. 知识图谱设计

首批图谱能力：

- 按知识库加载子图。
- 节点搜索和标签筛选。
- 点击节点展开邻居。
- 限制节点数量和查询深度。
- 将图谱查询作为 Agent 工具。

后续能力：

- 节点和关系来源跳转。
- 从图谱节点定位知识文件和片段。
- RAG 与图谱联合检索。
- 将当前选中图谱范围作为会话上下文。

图谱页面只使用 Yuxi 的授权只读 API，不直接访问图数据库。

## 8. 数据治理

### 8.1 备份与恢复

备份范围默认包括：

- `fox.db`
- 附件和未导出的产物
- Agent 配置和项目元数据
- 必要的 Runtime Session

日志和缓存默认不备份。恢复时先执行 schema 迁移，再恢复 Runtime 映射。

### 8.2 保留和清理

- Runtime 原始 JSONL 支持按天数和大小清理。
- 缓存支持全量重建。
- 临时附件和失败产物支持定期清理。
- 删除会话时展示会删除的数据范围。
- 导出到用户目录的文件不受 Fox 内部清理影响。

### 8.3 大历史性能

- 会话列表和消息使用分页或游标加载。
- SQLite 增加必要索引和全文搜索。
- 大型工具结果只保存摘要和外部文件引用。
- UI 不一次性渲染全部历史。
- Runtime 上下文和 Fox 完整历史保持分离。

## 9. 稳定性与诊断

生产级诊断包括：

- Runtime 和 Sidecar 版本。
- 启动、退出和最近错误原因。
- 活动 Session、Run 和队列状态。
- Yuxi、MCP 和模型 Provider 连接状态。
- 数据库 schema 和迁移状态。
- 可导出的脱敏诊断包。

诊断信息不得包含 API Key、Token、完整私密文件内容或未经用户确认的对话全文。

Sidecar 恢复策略：

- 意外退出后对活动 Run 标记失败或可恢复。
- 有稳定 Session 时尝试一次自动恢复。
- 连续失败时停止重启并提示用户。
- 版本不兼容时禁止循环启动。

## 10. 安全与供应链

- Runtime、Skill 和 MCP 都需要记录来源和版本。
- 固定关键依赖版本，升级通过自动化契约测试。
- 下载或安装扩展必须由用户明确触发。
- 可执行扩展需要哈希、签名或可信发布者机制。
- 权限以最小开放为默认，不因扩展声明而自动扩大项目访问范围。
- 凭证继续使用系统安全存储，诊断和日志统一脱敏。

## 11. 实施顺序

1. 稳定 Runtime、Tool 和 Capability Manifest 契约。
2. 建立 Runtime Adapter 契约测试套件。
3. 完成 Sidecar 版本管理、自动恢复和诊断。
4. 实现 Yuxi 图谱 Viewer API 和 Fox 图谱交互。
5. 实现 Skills 发现、查看和指令注入。
6. 实现 MCP Manager、工具目录和权限代理。
7. 完成备份、恢复、迁移和数据清理。
8. 实现 Fox 原生 Agent 的模型视觉能力检测和多模态图片消息映射。
9. 优化大型会话、工具结果和附件性能。
10. 根据真实需求评估并接入新的 Runtime Adapter。
11. 评估远程 Runtime、长任务或子 Agent 是否进入独立后续版本。

## 12. 交付物

- 可扩展的 Runtime Registry 和 Capability Manifest。
- Runtime Adapter 契约测试套件。
- Skills 和 MCP 的受控接入能力。
- 知识图谱浏览和 Agent 图谱工具。
- Sidecar 自动恢复、升级和诊断机制。
- 数据备份、恢复、迁移和清理能力。
- 支持 OpenAI 兼容与 Anthropic Messages 协议的受控图片输入能力。
- 大型历史和附件场景的性能优化。

## 13. 验收标准

- 新 Runtime Adapter 可以在不修改聊天 UI 的情况下接入。
- Runtime 不支持的能力不会出现在 UI 控件中。
- Skill 和 MCP 工具不能绕过 Fox 权限体系。
- 图谱页面只能看到当前用户有权限的知识库数据。
- Sidecar 连续崩溃不会形成无限重启循环。
- 备份可以在新安装环境恢复 Fox 核心数据。
- 视觉模型可以读取用户图片；非视觉模型不会收到无效图片内容，并能给出明确提示。
- 大型会话仍能快速打开、搜索和继续对话。
- 诊断包经过脱敏，不包含敏感凭证。

## 14. 决策门槛

以下能力只有在真实需求明确后才进入实施：

- 其他 Runtime：现有 Pi 无法满足模型、授权或部署需求。
- 远程 Fox Runtime：存在多设备或集中算力需求。
- 子 Agent：单 Agent 工具调用无法满足任务拆分需求。
- 长任务：用户确实需要关闭窗口后继续运行。
- 本地模型：隐私、离线或成本需求足以覆盖运维复杂度。

第三阶段的核心不是堆叠功能，而是确保 Fox 可以在保持现有数据和 UI 稳定的前提下安全扩展。


## 15. 第三阶段落地参数

本阶段最终采用以下实现参数：

- MCP：仅 stdio；每个请求独立子进程；15 秒超时；最多 200 个工具。
- Sidecar：一次崩溃链路最多自动恢复一次；契约错误不恢复。
- 大历史：初始加载最近 120 条消息；更早记录游标分页。
- 全文搜索：SQLite FTS5 覆盖会话标题、项目、Agent 和消息正文。
- 大型工具结果：超过 128 KB 时持久化摘要。
- 备份：自定义 `.foxbackup`，逐条 SHA-256，单条 512 MB、总量 4 GB 上限。
- 恢复：下次启动前应用；先校验并重定位路径；失败时从 `restore-rollback` 恢复原数据。
- Runtime Session：随备份保存，但跨 Runtime 版本仅尽力恢复。
- 删除会话：删除数据库记录、Fox 内部附件/未导出产物、Runtime Session 和可用的远程 Yuxi 线程；不删除项目目录的导出文件。
- UI 能力策略：图片入口和动态模型切换由 Capability Manifest 控制，不根据 Runtime 名称猜测。
