# FoxOps 工作区页面完整迁移方案

## 一、迁移目标

在 `foxops-web` 中新增独立的 `/workspace` 页面，完整迁移 Yuxi 工作区的业务能力，同时遵守以下边界：

- 后端继续使用 Yuxi 现有接口，不修改后端。
- 前端使用 Vue 3、Element Plus 和 ArtDesignPro 样式体系。
- 不复制 Ant Design Vue 组件实现。
- 个人工作区可读写，知识库文件只读。
- 桌面端和移动端均可正常浏览与预览文件。
- 所有 API 统一定义在 `foxops-web/src/api`。

## 二、计划新增的文件

```text
foxops-web/src/
├─ api/
│  ├─ workspace.ts
│  └─ knowledge.ts
├─ router/modules/
│  └─ workspace.ts
├─ views/workspace/
│  └─ index.vue
├─ components/workspace/
│  ├─ WorkspaceSidebar.vue
│  ├─ WorkspaceFileTable.vue
│  ├─ WorkspacePreviewPane.vue
│  └─ WorkspaceFilePreview.vue
├─ components/common/
│  └─ FileTypeIcon.vue
└─ utils/
   ├─ file.ts
   └─ filePreview.ts
```

同时修改：

```text
foxops-web/src/router/modules/index.ts
foxops-web/src/api/request.ts（仅在 Blob 响应能力不足时修改）
foxops-web/package.json（仅在测试或 Markdown 渲染确实缺少依赖时修改）
```

页面状态保持在 `views/workspace/index.vue`，暂不新增 Pinia Store。

## 三、页面布局

### 1. 页面头部

显示：

- 页面标题“工作区”。
- 全局加载状态。
- “新建文件夹”按钮。
- “上传文件”按钮。
- 隐藏的多文件选择器。

规则：

- 仅个人工作区显示并启用新建、上传能力。
- 浏览知识库时按钮禁用或隐藏。
- 上传过程中显示 Loading，并禁止重复提交。

### 2. 左侧资源导航

包含：

- 个人工作区。
- 快速访问：
  - Saved Artifacts：`/saved_artifacts`
  - Agents：`/agents/`
- 我的知识库。
- 共享知识库。
- 知识库加载状态。
- 无可访问知识库的空状态。
- 侧栏折叠与展开。

知识库按 `created_by === 当前用户 UID` 区分“我的”和“共享”。

### 3. 文件主区域

使用 Element Plus 表格，包含：

- 面包屑导航。
- 当前文件夹项目数量。
- 名称、文件大小、修改时间和操作菜单。
- 文件类型图标。
- 文件夹进入与文件预览。
- 空目录和加载状态。
- 多选模式和批量删除。
- 单文件下载。
- 单文件或文件夹删除。
- 知识库分页。

### 4. 文件预览

宽度达到 960px 时：

- 在文件表格右侧显示内联预览。
- 表格与预览之间提供拖动分隔条。
- 预览宽度限制为页面的 30%–70%。
- 默认预览宽度为 50%。

宽度小于 960px 时：

- 使用 Element Plus Dialog 打开预览。
- 最大宽度约为视口的 92%。
- 高度约为视口的 82%。
- 支持全屏预览。

## 四、API 迁移

### 个人工作区

```text
GET    /api/workspace/tree
GET    /api/workspace/file
PUT    /api/workspace/file
DELETE /api/workspace/file
POST   /api/workspace/directory
POST   /api/workspace/upload
GET    /api/workspace/download
```

请求能力：

- 目录浏览支持 `path`、`recursive`、`files_only`。
- 文件读取返回 Blob。
- 文本保存提交 `{ path, content }`。
- 新建目录提交 `{ parent_path, name }`。
- 上传使用 FormData，包含 `parent_path` 和多个 `files`。
- 下载返回 Blob，并解析 `Content-Disposition` 文件名。

### 知识库

```text
GET /api/knowledge/databases/accessible
GET /api/workspace/knowledge/tree
GET /api/workspace/knowledge/file
GET /api/workspace/knowledge/download
```

知识库目录参数：

- `kb_id`
- `parent_id`
- `path_prefix`
- `page`
- `page_size`
- `recursive`
- `files_only`

知识库文件使用 `kb_id + file_id` 定位，不使用本地路径定位。

## 五、个人工作区功能流程

### 初始化

页面挂载时并行执行：

1. 加载个人工作区根目录 `/`。
2. 加载当前用户可访问知识库。
3. 初始化主区域宽度监听。

任意一项失败不能阻断另一项。

### 目录浏览

点击文件夹：

1. 关闭当前预览。
2. 清空多选状态。
3. 请求目标目录。
4. 更新当前路径。
5. 重新生成面包屑。
6. 更新表格内容。

点击面包屑时重新加载对应目录。

### 新建文件夹

1. 打开名称输入弹窗。
2. 禁止空名称。
3. 调用创建目录接口。
4. 成功后刷新当前目录。
5. 关闭弹窗并提示成功。
6. 失败时保留弹窗内容并显示后端错误。

### 上传文件

1. 支持一次选择多个文件。
2. 一次最多上传 50 个文件。
3. 上传到当前目录。
4. 上传完成后刷新当前目录。
5. 清空原生文件选择器，保证可以再次选择同名文件。
6. 显示成功数量或错误原因。

### 删除

支持单文件、单文件夹递归删除和多选批量删除。

删除前确认文案：

- 文件：删除后不可恢复。
- 文件夹：将删除文件夹及全部内容。
- 批量：显示选中项目数量。

删除期间：

- 对应行进入禁用或加载状态。
- 禁止重复删除。
- 如果删除项包含当前预览文件，立即关闭预览。
- 无论成功还是部分失败，最后重新拉取当前目录。

### 下载

1. 请求 Blob。
2. 优先从 `Content-Disposition` 读取 UTF-8 文件名。
3. 回退到表格中的文件名。
4. 创建临时 Object URL。
5. 触发浏览器下载。
6. 完成后释放 Object URL。

文件夹不显示下载操作。

## 六、知识库浏览流程

知识库保持完全只读。

选择知识库时：

1. 关闭个人文件预览。
2. 清空选择状态。
3. 请求知识库根目录。
4. 创建虚拟面包屑。
5. 默认每页加载 100 项。

分页规格：

- 默认 100 项。
- 可选择 100、300、500。
- 切换目录时回到第一页。
- 切换每页数量时回到第一页。
- 显示总文件数量。
- 使用后端返回的 `parent_id` 和 `path_prefix` 继续浏览虚拟目录。

知识库允许进入目录、查看文件、下载文件和翻页。

知识库禁止新建文件夹、上传文件、编辑保存、删除和批量选择。

## 七、文件预览能力

`filePreview.ts` 负责把 Blob 和响应头转换为统一对象：

```text
{
  name,
  path,
  content,
  previewType,
  previewUrl,
  mimeType,
  supported,
  message
}
```

支持类型：

- `markdown`：渲染 Markdown，可切换编辑。
- `text`：等宽文本预览，可编辑。
- `image`：使用 Object URL 显示图片。
- `pdf`：使用 iframe 预览。
- `html`：使用 iframe `srcdoc` 或安全 Blob URL。
- DOCX、PPTX：使用后端返回的 HTML 预览。
- 其他二进制格式：显示“不支持预览，请下载后查看”。

编辑规则：

- 只有个人工作区的 Markdown 和纯文本文件可以编辑。
- 知识库文件永远只读。
- 保存中禁止再次保存或关闭编辑状态。
- 保存成功后同步预览内容并刷新当前目录。
- 切换文件时丢弃未保存内容前应进行确认。

资源清理：

- 切换文件、关闭预览和页面卸载时释放旧 Object URL。
- 使用请求序号避免快速切换文件时旧请求覆盖新文件。

## 八、响应式行为

### 桌面端

- 左侧导航固定约 195px，并支持折叠。
- 表格占剩余空间。
- 预览打开后采用表格、分隔条、预览三列布局。
- 拖动预览宽度时禁止页面选中文本。

### 小屏端

- 预览使用 Dialog。
- 文件表格保持横向滚动能力。
- 头部操作按钮允许收缩。
- 左侧导航折叠或切换为抽屉。
- 关闭内联模式时清理预览，避免两个预览实例并存。

## 九、状态与异常处理

必须覆盖：

- 工作区目录、知识库列表和知识库目录加载中。
- 文件预览加载中、失败和不支持预览。
- 当前目录为空和暂无可访问知识库。
- 上传、创建文件夹、保存和删除进行中。
- 下载失败。
- 登录状态失效和无知识库访问权限。
- 知识库不支持文档。
- 后端返回非法路径或重名文件。

接口错误统一复用 FoxOps 请求层和消息提示，不在组件中拼接后端 URL。

## 十、实施阶段

### 第一阶段：基础设施

完成 `workspace.ts`、`knowledge.ts`、`file.ts`、`filePreview.ts`、`/workspace` 路由、路由注册和菜单图标。

验收：可以请求工作区根目录和可访问知识库，Blob 响应不会被 JSON 请求封装破坏。

### 第二阶段：基础组件

完成 `FileTypeIcon.vue`、`WorkspaceSidebar.vue` 和 `WorkspaceFileTable.vue`。

验收：资源分组、表格、面包屑、文件操作和知识库只读状态正确。

### 第三阶段：个人工作区

完成目录浏览、新建文件夹、多文件上传、下载、单项删除、多选和批量删除，以及加载、空状态和错误状态。

验收：所有操作完成后表格与后端一致；删除当前预览文件时自动关闭预览；一次超过 50 个文件时在请求前阻止上传。

### 第四阶段：文件预览与编辑

完成文本、Markdown、图片、PDF、HTML 和 Office 预览，不支持格式提示，文本与 Markdown 编辑保存、全屏预览、Object URL 清理和竞态保护。

验收：快速切换文件只展示最后选择项；编辑保存后刷新仍保留内容；知识库无编辑入口；关闭预览后无 Blob URL 残留。

### 第五阶段：知识库浏览

完成可访问知识库加载、我的/共享分组、虚拟目录、分页、文件预览和文件下载。

验收：无权限和不支持文档的知识库不可浏览；目录、分页和面包屑组合正常；知识库始终只读。

### 第六阶段：响应式与视觉统一

完成侧栏折叠、960px 内联预览切换、预览宽度拖动、小屏 Dialog，以及 ArtDesignPro 颜色、间距、圆角、阴影和暗色模式适配。

验收：桌面端、平板端和窄屏无内容溢出；暗色模式可读；页面不残留 Ant Design 组件和样式类。

### 第七阶段：验证和回归

执行 TypeScript 类型检查、ESLint、Vite 生产构建、工作区 API 测试、预览归一化测试、页面交互测试和 Docker 环境端到端测试。

重点场景：

1. 进入个人工作区根目录。
2. 创建文件夹并进入。
3. 上传多个文件。
4. 预览并编辑 Markdown。
5. 下载文件。
6. 删除单个文件。
7. 批量删除文件。
8. 打开 Saved Artifacts 和 Agents。
9. 浏览我的知识库和共享知识库。
10. 切换知识库分页。
11. 预览并下载知识库文件。
12. 快速切换多个预览。
13. 调整预览宽度。
14. 从桌面宽度切换到移动端宽度。
15. 验证知识库中不存在写操作。

## 十一、最终验收标准

- `/workspace` 可以从菜单进入。
- 个人工作区全部读写操作可用。
- 我的知识库和共享知识库可浏览且严格只读。
- 文件预览类型与 Yuxi 一致。
- Markdown 和 TXT 编辑保存正常。
- 多文件上传、批量删除和下载正常。
- 面包屑、分页和快速访问正常。
- 宽屏内联预览和窄屏弹窗预览正常。
- 暗色模式正常。
- 无明显内存泄漏或 Blob URL 残留。
- TypeScript、Lint、构建和端到端验证全部通过。
- 不修改 Yuxi 后端接口。
- 不遗漏加载、空状态、失败和无权限状态。
