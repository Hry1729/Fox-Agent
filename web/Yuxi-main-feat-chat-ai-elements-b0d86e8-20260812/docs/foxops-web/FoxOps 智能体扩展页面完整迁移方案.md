# FoxOps 智能体扩展页面完整迁移方案

## 一、迁移目标

在 `foxops-web` 中完整迁移 Yuxi 的“智能体扩展”模块，包括知识库、内置工具、MCP 和 Skills 四类资源以及对应详情页。

迁移边界：

- 后端继续使用 Yuxi 现有接口，不修改接口语义。
- 前端统一使用 Vue 3、Element Plus 和 ArtDesignPro 设计体系。
- 不复制 Ant Design Vue 组件实现。
- 列表页、详情页、弹窗、权限、加载状态和错误状态均需迁移。
- 扩展模块较大，按独立可验收子模块分阶段交付，最终范围必须完整。
- 所有接口集中在 `foxops-web/src/api`，页面组件不得自行拼接 URL。
- LITE 模式下必须允许知识库、图谱和评估等重依赖能力不可用。

## 二、功能范围与权限

### 管理员

管理员可访问：

- 知识库列表与详情。
- 内置工具列表与详情。
- MCP 列表、创建、编辑、测试、启停、删除及工具管理。
- Skills 列表与其权限范围内的管理能力。

### 普通用户

普通用户仅显示 Skills 标签页，并且：

- 可以查看有权访问的 Skills。
- 可以使用可管理 Skill 的启停、编辑、依赖、导出和删除能力。
- 对仅共享给自己的 Skill 保持只读。
- 不得通过手工 URL 绕过管理员路由权限。

### 特殊资源

- 内置 Skill 不允许删除。
- 无管理权限的 Skill 不允许编辑文件、修改依赖或共享范围。
- 系统 MCP 的“移除”语义为禁用，自定义 MCP 才执行删除。
- 知识库、工具和 MCP 详情页继续由后端权限校验兜底。

## 三、路由设计

```text
/extensions
/extensions/knowledgebase/:kbId
/extensions/mcp/:slug
/extensions/skill/:slug
```

路由行为：

- `/extensions` 使用 `tab` 查询参数保存当前标签。
- 管理员默认进入知识库，普通用户默认进入 Skills。
- 非法或无权限标签自动回退到当前用户默认标签。
- 详情页通过嵌套路由渲染，不保留列表页的 Tab 内容。
- 知识库与 MCP 详情路由要求管理员权限。
- Skill 详情要求登录，并根据接口返回的 `can_manage` 决定读写能力。

## 四、建议文件结构

```text
foxops-web/src/
├─ api/
│  ├─ knowledge.ts
│  ├─ tools.ts
│  ├─ mcp.ts
│  ├─ skills.ts
│  ├─ organization.ts
│  ├─ graph.ts
│  └─ tasker.ts
├─ router/modules/
│  └─ extensions.ts
├─ views/extensions/
│  ├─ index.vue
│  ├─ knowledge/detail.vue
│  ├─ mcp/detail.vue
│  └─ skill/detail.vue
├─ components/extensions/
│  ├─ common/
│  │  ├─ ExtensionToolbar.vue
│  │  ├─ ExtensionCardGrid.vue
│  │  ├─ ExtensionInfoCard.vue
│  │  ├─ ExtensionDetailHeader.vue
│  │  ├─ ExtensionEmptyState.vue
│  │  └─ ShareScopeForm.vue
│  ├─ tools/
│  │  ├─ ToolsCardList.vue
│  │  └─ ToolDetailDialog.vue
│  ├─ mcp/
│  │  ├─ McpCardList.vue
│  │  ├─ McpFormDialog.vue
│  │  ├─ McpEnvEditor.vue
│  │  └─ McpToolList.vue
│  ├─ skills/
│  │  ├─ SkillCardList.vue
│  │  ├─ SkillPreviewDialog.vue
│  │  ├─ SkillRemoteInstallDialog.vue
│  │  ├─ SkillInstallDraftDialog.vue
│  │  ├─ SkillFileManager.vue
│  │  └─ SkillDependencyPanel.vue
│  └─ knowledge/
│     ├─ KnowledgeCardList.vue
│     ├─ KnowledgeCreateDialog.vue
│     ├─ KnowledgeEditDialog.vue
│     ├─ KnowledgeDocumentTable.vue
│     ├─ KnowledgeUploadDialog.vue
│     ├─ KnowledgeFileDetailDialog.vue
│     ├─ KnowledgeQueryPanel.vue
│     ├─ KnowledgeSearchConfig.vue
│     ├─ KnowledgeGraphPanel.vue
│     ├─ KnowledgeMindMapPanel.vue
│     ├─ KnowledgeEvaluationPanel.vue
│     └─ KnowledgeBenchmarkPanel.vue
└─ store/modules/
   ├─ knowledge.js
   └─ tasker.js
```

工作区迁移中创建的文件类型图标、文件预览、文件树和 Blob 处理能力应当复用，不再创建第二套实现。

## 五、扩展首页

### 页面头部

显示“智能体扩展”标题和标签：

- 知识库
- 工具
- MCP
- Skills

页面头部的加载状态读取当前激活子模块，不把其他未显示 Tab 的请求状态混入。

### 标签同步

1. 从 `route.query.tab` 读取当前标签。
2. 根据角色生成允许访问的标签集合。
3. 非法标签回退默认值并替换 URL。
4. 用户切换标签时只更新查询参数，不产生浏览器历史堆积。
5. 从详情页返回时恢复原标签。

### 通用组件

统一实现：

- 搜索框。
- 分类筛选。
- 刷新按钮。
- 右侧创建或导入操作。
- 响应式卡片网格。
- 卡片标题、图标、标签、描述和状态。
- 加载、空数据、搜索无结果和请求失败状态。

## 六、内置工具模块

### 列表能力

- 管理员可见。
- 获取 `/api/system/tools`。
- 按名称、Slug、描述和配置说明搜索。
- 分类筛选：内置、知识库、MySQL、调试及后端新增分类。
- 支持手动刷新。
- 卡片显示名称、Slug、描述、分类和最多两个标签。

### 详情弹窗

显示：

- 工具名称和描述。
- 配置说明。
- 分类和标签。
- 参数名称、类型、是否必填和描述。

该模块只读，不提供前端虚构的编辑或删除能力。

## 七、MCP 模块

### MCP 列表

- 搜索名称、Slug、描述和标签。
- 分为“已添加”和“可添加”。
- 已添加项可以移除或删除。
- 可添加项可以先查看基础信息，再执行添加。
- 基础信息包含图标、名称、传输类型、描述、标签和创建人。
- 支持刷新和新增 MCP。

### MCP 创建与编辑

支持三种传输类型：

- `streamable_http`
- `sse`
- `stdio`

通用字段：

- 稳定 Slug，创建后不可修改。
- 展示名称。
- 描述。
- Emoji 图标。
- 标签。

HTTP/SSE 字段：

- URL。
- JSON 请求头。
- HTTP 超时，范围 1–300 秒。
- SSE 读取超时，范围 1–300 秒。

Stdio 字段：

- 启动命令。
- 参数数组。
- 环境变量键值编辑器。

校验要求：

- Slug 和名称不能为空。
- HTTP/SSE 必须填写合法 URL。
- Stdio 必须填写命令。
- 请求头必须是 JSON 对象，禁止数组和无效 JSON。
- 环境变量不允许重复键和空键。

### MCP 详情

头部操作：

- 返回。
- 测试连接。
- 编辑。
- 添加、移除或删除。

“信息”标签显示连接配置、标签、超时、命令、参数、环境变量、创建人与时间；敏感请求头和环境变量应按后端返回内容展示，不在日志中输出。

“工具”标签支持：

- 获取和刷新工具列表。
- 搜索工具。
- 显示工具 ID、名称、描述和参数 Schema。
- 标记必填参数。
- 单独启用或禁用工具。
- 复制工具名称。
- 独立显示工具请求失败状态。

### MCP API

```text
GET    /api/system/mcp-servers
GET    /api/system/mcp-servers/:slug
POST   /api/system/mcp-servers
PUT    /api/system/mcp-servers/:slug
DELETE /api/system/mcp-servers/:slug
POST   /api/system/mcp-servers/:slug/test
PUT    /api/system/mcp-servers/:slug/status
GET    /api/system/mcp-servers/:slug/tools
POST   /api/system/mcp-servers/:slug/tools/refresh
PUT    /api/system/mcp-servers/:slug/tools/:toolName/toggle
```

## 八、Skills 模块

### Skills 首页

Skills 按以下分组展示：

- 已安装的用户或共享 Skills。
- 内置 Skills。
- 推荐安装 Skills。

卡片显示名称、描述、来源类型、启用状态和管理权限。

操作包括：

- 搜索。
- 启用或禁用。
- 快速预览 `SKILL.md`。
- 进入详情管理。
- 卸载单个 Skill。
- 批量管理和批量删除。
- 上传 `.zip` 或文件名为 `SKILL.md` 的本地 Skill。
- 远程安装。
- 安装推荐 Skill。
- 刷新列表。

内置或只读 Skill 不进入可删除集合。

### 快速预览

- 显示名称、来源、禁用状态和 `SKILL.md` Markdown。
- 支持在权限允许时启停和卸载。
- 支持跳转详情管理。
- 加载失败、没有 `SKILL.md` 和无权限分别提示。

### 本地上传与安装草稿

1. 上传 `.zip` 或 `SKILL.md`。
2. 调用 prepare 接口，仅解析，不立即安装。
3. 展示每个解析项的名称、描述、来源、警告和失败原因。
4. 选择生效范围。
5. 确认后逐个提交草稿。
6. 汇总成功和失败数量。
7. 用户取消时丢弃全部草稿。

### 远程安装

支持两种入口：

1. 按仓库拉取，支持 `owner/repo`、GitHub URL 和 ModelScope 单 Skill 地址。
2. 在远程市场按关键词搜索。

功能包括：

- 远程来源历史，最多保存 10 条。
- 删除单条或清空历史。
- 拉取仓库中的 Skill 列表。
- 本地过滤。
- 搜索结果分页。
- 全选、反选、清空和多选安装。
- 只有一个结果时自动选择。
- 按来源分组生成安装草稿。
- 进入统一草稿确认与共享范围流程。

### Skill 详情

头部显示名称、来源和状态，并根据权限提供导出和删除。

#### 代码管理

- 展示完整文件树。
- 默认打开 `SKILL.md`。
- 展开目录、刷新文件树。
- 新建文件和目录。
- 查看和编辑文本文件。
- 保存文件。
- 删除文件或目录。
- 全屏预览。
- 只读 Skill 禁止所有写操作。

#### 生效范围

- 启用或禁用 Skill。
- 配置全局、部门或指定用户范围。
- 内置 Skill 的共享范围固定，只允许控制启用状态。
- 无管理权限时仅展示范围摘要。

#### 依赖管理

支持声明：

- 内置工具依赖。
- MCP 依赖。
- 其他 Skill 依赖。

每类依赖支持搜索、多选、已选数量、标签展示和移除。当前 Skill 自身不得出现在 Skill 依赖选项中。

### Skill API

```text
GET    /api/system/skills
GET    /api/skills/accessible
POST   /api/skills/import/prepare
POST   /api/skills/remote/list
POST   /api/skills/remote/prepare
POST   /api/skills/remote/search
POST   /api/skills/install-drafts/:draftId/confirm
DELETE /api/skills/install-drafts/:draftId
GET    /api/system/skills/dependency-options
GET    /api/system/skills/builtin
POST   /api/system/skills/builtin/sync
GET    /api/system/skills/:slug/tree
GET    /api/system/skills/:slug/file
POST   /api/system/skills/:slug/file
PUT    /api/system/skills/:slug/file
DELETE /api/system/skills/:slug/file
PUT    /api/system/skills/:slug/dependencies
PUT    /api/system/skills/:slug/share-config
PUT    /api/system/skills/:slug/enabled
GET    /api/system/skills/:slug/export
DELETE /api/system/skills/:slug
POST   /api/system/skills/delete-batch
```

## 九、知识库模块

知识库模块是扩展页中最大的独立子系统，应分阶段迁移。

### 知识库列表

- 管理员可见。
- 搜索名称、ID 和描述。
- 按知识库类型筛选。
- 动态加载后端支持的知识库类型。
- 卡片显示名称、类型、ID、描述、文件数和共享范围。
- 空状态提供创建入口。

### 新建知识库

表单根据知识库类型动态显示：

- 名称和描述。
- 嵌入模型。
- 分块策略与说明。
- 后端返回的额外创建字段，支持文本、密码、数字、布尔和下拉类型。
- AI 生成或优化描述。
- 全局、部门或指定用户共享范围。

创建前校验类型、名称、必填动态字段、嵌入模型和共享配置。

### 知识库详情

头部提供返回、复制 ID、编辑和删除。编辑包含名称、描述、自动生成问题、分块策略、连接器凭据和共享范围。

Milvus 类型包含：

- 文件管理。
- 检索测试。
- 知识图谱。
- 知识导图。
- RAG 评估。
- 评估基准。

Dify、Notion 等连接器类型只显示检索测试，并保持只读文档语义。

### 文件管理

- 文件和虚拟文件夹列表。
- 分页、排序、搜索、面包屑和多选。
- 上传单文件、批量文件、文件夹和工作区文件。
- 创建文件夹。
- 文件是否重名检查。
- 解析方式与分块参数。
- 待解析、待入库、文件、大小、Chunk 和 Token 统计。
- 批量解析和批量入库。
- 修复统计。
- 单项和批量删除。
- 文件下载。
- 文件详情、原文预览、解析内容与 Chunk 查看。
- 后台任务进度与完成后自动刷新。

### 检索测试与配置

- 输入查询并执行知识库检索。
- 展示命中内容、分数和元数据。
- 支持 JSON 调试视图。
- 加载并保存检索参数。
- 生成示例问题。
- 连接器类型使用自身支持的检索参数。

### 知识图谱

- 图谱构建状态和配置。
- 启动、重置和继续构建。
- 节点、关系和统计展示。
- 搜索、筛选、布局和详情查看。
- 后台任务状态订阅。
- LITE 模式或图谱服务不可用时显示明确提示。

### 知识导图

- 加载已有导图。
- 根据指定文件或全部文件生成。
- 支持自定义提示词和增量生成。
- 使用 Markmap 展示、缩放和全屏。
- 生成期间显示任务进度和失败原因。

### RAG 评估与基准

- 数据集上传、查看、下载和删除。
- 自动生成评估数据集。
- 创建、编辑和删除评估基准。
- 运行评估任务。
- 查看历史运行、指标汇总和分页结果。
- 跳转基准与评估页面。
- 后台任务取消、轮询或订阅、完成刷新。
- 非 Milvus 知识库不显示评估能力。

## 十、共享范围组件

知识库和 Skill 共用 `ShareScopeForm.vue`：

- 全局可见。
- 当前部门可见。
- 指定部门可见。
- 指定用户可见。
- 加载部门和用户列表。
- 自动选择当前用户部门。
- 按后端返回的 `allowed_access_levels` 限制选项。
- 保存前统一验证。
- 只读状态显示摘要，不展示可编辑控件。

## 十一、状态与错误处理

每个子模块独立维护：

- 初始加载。
- 刷新中。
- 创建、更新、删除中。
- 空数据和搜索无结果。
- 请求失败与重试。

必须特别处理：

- 401 登录失效。
- 403 无管理权限。
- 404 资源已删除或 Slug 无效。
- 409 名称、Slug、文件或目录冲突。
- MCP 测试连接失败与超时。
- Skill 远程来源不可访问、解析失败和部分安装失败。
- 安装草稿过期或被丢弃。
- 知识库解析、入库、图谱和评估后台任务失败。
- LITE 模式下接口未注册。
- 快速切换路由时旧请求不得覆盖新详情。

错误信息优先展示后端 `detail`，同时避免把 Token、请求头和环境变量写入控制台。

## 十二、实施阶段

### 第一阶段：路由、权限、API 与通用组件

完成扩展路由、Tab 权限、查询参数同步、四类 API、共享卡片、工具栏、详情头部、空状态和共享范围组件。

验收：管理员和普通用户看到正确标签；非法 URL 无法越权；四类列表接口可以独立调用。

### 第二阶段：内置工具

完成工具搜索、分类、刷新、卡片和详情弹窗。

验收：分类、搜索和参数 Schema 展示与 Yuxi 一致。

### 第三阶段：MCP 列表和表单

完成列表分组、搜索、添加、移除、删除、基础预览、三种传输表单和环境变量编辑器。

验收：三种传输配置可以创建；无效 JSON、URL 和命令在请求前被阻止。

### 第四阶段：MCP 详情与工具管理

完成信息、编辑、测试、启停、删除、工具刷新、搜索、单工具启停和参数查看。

验收：服务状态和工具状态刷新后与后端一致；测试错误可见但不破坏页面。

### 第五阶段：Skills 首页和安装流程

完成 Skill 分组、搜索、启停、预览、单删、批量删除、上传、远程仓库、全局搜索、推荐安装、历史和安装草稿确认。

验收：本地上传与远程安装都必须经过草稿和共享范围确认；取消后草稿被清理；部分失败正确汇总。

### 第六阶段：Skill 详情

完成文件树、编辑保存、新建、删除、导出、启停、共享范围、依赖选择和只读权限。

验收：内置 Skill 不可删除；只读 Skill 无写入口；依赖保存后重新加载一致。

### 第七阶段：知识库列表与创建

完成知识库 API、Store、搜索、类型筛选、卡片、新建表单、动态字段、嵌入模型、分块策略、AI 描述和共享配置。

验收：所有后端支持类型可创建；必填项和动态字段验证正确。

### 第八阶段：知识库文件管理

完成详情头部、知识库编辑、文件表、上传、目录、解析、入库、统计、详情、下载和删除。

验收：文件完整生命周期可运行；后台任务完成后状态自动刷新；连接器类型不显示不支持的文件写操作。

### 第九阶段：检索测试与配置

完成查询、结果、调试信息、检索配置、示例问题和参数保存。

验收：不同知识库类型只提交其支持的参数；配置刷新后保持一致。

### 第十阶段：知识图谱与知识导图

完成图谱构建与浏览、任务状态、Markmap 导图、生成、增量更新和全屏。

验收：服务不可用和 LITE 模式有明确状态；任务失败可恢复重试。

### 第十一阶段：RAG 评估与评估基准

完成数据集、基准、运行任务、结果、指标和历史记录。

验收：评估任务全过程可追踪；非 Milvus 类型不出现相关标签。

### 第十二阶段：响应式、暗色模式与回归

完成 ArtDesignPro 视觉统一、移动端、暗色模式、键盘可访问性、类型检查、Lint、构建和 Docker 端到端验证。

## 十三、测试重点

### 权限

- 普通用户只能看到 Skills。
- 普通用户访问管理员详情路由时被拦截。
- 只读 Skill 不出现写操作。
- 内置 Skill 不可删除。

### MCP

- 三种传输表单及字段切换。
- 请求头 JSON 和环境变量校验。
- 测试、编辑、启停和删除。
- 单个工具启停失败时回滚。

### Skills

- 本地上传格式校验。
- 安装草稿确认和取消。
- 仓库历史最多 10 条。
- 多来源远程安装。
- 批量删除过滤不可删除资源。
- 文件树读写、依赖和共享范围。

### 知识库

- 动态类型和创建字段。
- 文件上传、解析、入库和删除。
- 检索配置保存。
- 图谱、导图和评估的可用性判断。
- 后台任务完成、失败和取消。

## 十四、最终验收标准

- `/extensions` 和三个详情路由正常工作。
- 管理员与普通用户标签和权限正确。
- Tab 状态与 URL 同步。
- 工具列表与详情完整。
- MCP 的创建、编辑、测试、启停、删除和工具管理完整。
- Skills 的预览、启停、上传、远程安装、草稿、批量管理、详情编辑、依赖和共享完整。
- 知识库的创建、文件管理、检索、图谱、导图、评估和基准完整。
- 连接器与 Milvus 的能力差异正确。
- LITE 模式不会导致整个扩展页面崩溃。
- 页面无 Ant Design Vue 组件和样式残留。
- ArtDesignPro 明暗主题和响应式布局正常。
- 敏感 MCP 配置不会进入日志。
- TypeScript、ESLint、生产构建和 Docker 端到端测试通过。
- 不修改 Yuxi 后端接口，不用静态占位掩盖未迁移功能。
