# FoxOps 工作区迁移 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在现有 ArtDesignPro 框架中新增侧边栏“工作区”，完整接入 Yuxi 个人工作区和只读知识库浏览能力。

**Architecture:** 复用 ArtDesignPro 路由、菜单、权限、页面高度、Element Plus 和请求层。工作区由一个页面编排器、三个职责清晰的业务组件、一个 API 模块和纯函数工具组成；页面保持局部状态，不新增全局 Store。

**Tech Stack:** Vue 3、Element Plus、ArtDesignPro、TypeScript/JavaScript、Axios、Node `node:test`、Yuxi FastAPI。

## Global Constraints

- 不修改 Yuxi 后端接口。
- 不迁移 Yuxi `AppLayout.vue`，不重建 ArtDesignPro 框架。
- 菜单“工作区”必须紧跟“对话”。
- 个人工作区可读写；知识库严格只读。
- 不覆盖当前未提交的 Chat、package.json 和 pnpm-lock.yaml 改动。
- 所有业务请求统一经过 `foxops-web/src/api/request.ts`。

---

### Task 1: 工作区纯函数契约

**Files:**
- Create: `foxops-web/tests/workspace-utils.test.mjs`
- Create: `foxops-web/src/utils/workspace.js`

**Interfaces:**
- Produces: `buildQuery(params)`、`normalizeWorkspacePath(path)`、`buildBreadcrumbs(path, rootLabel)`、`formatFileSize(bytes)`、`getPreviewType(path, contentType)`、`parseDownloadFilename(header)`。

- [x] **Step 1: 写失败测试**

覆盖空 query、布尔参数、根路径、嵌套面包屑、0 字节、Markdown/图片/PDF/HTML/Office/文本/未知类型及 RFC 5987 UTF-8 文件名。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为 `src/utils/workspace.js` 不存在。

- [x] **Step 3: 最小实现纯函数**

实现无 Vue、无 DOM、无别名依赖的纯函数，供 API、组件和页面共同复用。

- [x] **Step 4: 验证绿灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: 全部 PASS。

### Task 2: Yuxi 工作区 API 适配

**Files:**
- Create: `foxops-web/src/api/workspace.ts`
- Create: `foxops-web/src/api/knowledge.ts`

**Interfaces:**
- Consumes: `buildQuery(params)` from Task 1；`request` from `src/api/request.ts`。
- Produces: tree/file/save/delete/directory/upload/download，以及 accessible knowledge/tree/file/download 方法。

- [x] **Step 1: 扩展契约测试**

静态检查两个 API 文件必须声明 Yuxi 规格中的全部 endpoint，并要求 Blob 接口使用 `responseType: 'blob'`。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为 API 文件不存在。

- [x] **Step 3: 实现 API**

个人工作区：`/api/workspace/tree|file|directory|upload|download`；知识库：`/api/knowledge/databases/accessible` 与 `/api/workspace/knowledge/tree|file|download`。

- [x] **Step 4: 验证绿灯与类型检查**

Run: `node --test tests/workspace-utils.test.mjs`

Run: `pnpm exec vue-tsc --noEmit`

Expected: 两条命令退出码 0。

### Task 3: 路由和侧边栏菜单

**Files:**
- Create: `foxops-web/src/router/modules/workspace.ts`
- Modify: `foxops-web/src/router/modules/index.ts`
- Create: `foxops-web/src/views/workspace/index.vue`

**Interfaces:**
- Produces: `/workspace`、route name `Workspace`、component `/workspace/index`、标题“工作区”。

- [x] **Step 1: 写路由契约失败测试**

要求 `workspaceRoutes` 紧跟 `chatRoutes` 出现在 `routeModules`，并指向 `/workspace`。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为 workspace route 不存在。

- [x] **Step 3: 最小路由与页面空壳**

新增 route module 和可编译页面；不改 MenuProcessor，利用现有前端路由模式自动生成侧边栏。

- [x] **Step 4: 验证绿灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: PASS。

### Task 4: 资源侧栏与文件表格

**Files:**
- Create: `foxops-web/src/components/workspace/WorkspaceSidebar.vue`
- Create: `foxops-web/src/components/workspace/WorkspaceFileTable.vue`
- Create: `foxops-web/src/components/workspace/FileTypeIcon.vue`

**Interfaces:**
- Sidebar emits: `select-personal`、`select-path`、`select-database`。
- Table emits: `open`、`breadcrumb`、`selection-change`、`delete`、`download`、`page-change`。

- [x] **Step 1: 增加结构契约测试**

验证组件包含个人工作区、Saved Artifacts、Agents、我的知识库、共享知识库，以及名称/大小/修改时间/操作列和只读控制。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为组件不存在。

- [x] **Step 3: 使用 Element Plus 实现组件**

使用 `ElTable`、`ElBreadcrumb`、`ElDropdown`、`ElPagination`，不复制 Ant Design Vue。

- [x] **Step 4: 验证绿灯和编译**

Run: `node --test tests/workspace-utils.test.mjs`

Run: `pnpm exec vue-tsc --noEmit`

Expected: 退出码 0。

### Task 5: 文件预览和编辑

**Files:**
- Create: `foxops-web/src/components/workspace/WorkspaceFilePreview.vue`
- Create: `foxops-web/src/components/workspace/WorkspacePreviewPane.vue`

**Interfaces:**
- Preview props: normalized file、editable、saving、fullHeight。
- Preview emits: `close`、`save(content)`、`download`。

- [x] **Step 1: 增加预览结构契约测试**

验证 Markdown/text/image/pdf/html/office/unsupported 分支、个人文本编辑和知识库只读控制。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为预览组件不存在。

- [x] **Step 3: 实现预览**

Markdown 复用 `renderMarkdown`；文本使用 textarea；图片 object URL；PDF/HTML/Office iframe；未知格式给下载提示。

- [x] **Step 4: 验证绿灯和编译**

Run: `node --test tests/workspace-utils.test.mjs`

Run: `pnpm exec vue-tsc --noEmit`

Expected: 退出码 0。

### Task 6: 工作区页面编排

**Files:**
- Modify: `foxops-web/src/views/workspace/index.vue`

**Interfaces:**
- Consumes Tasks 1–5。
- Produces: 个人目录 CRUD、多文件上传、批量删除、下载、预览编辑；知识库分组、虚拟目录、分页、预览下载；桌面内联和移动弹窗预览。

- [x] **Step 1: 增加页面行为契约测试**

静态要求页面使用全部 API、限制 50 个上传文件、知识库传入 readonly、存在预览请求序号、Object URL 清理、ResizeObserver 和 30%–70% 调整边界。

- [x] **Step 2: 验证红灯**

Run: `node --test tests/workspace-utils.test.mjs`

Expected: FAIL，原因为页面尚未包含业务行为。

- [x] **Step 3: 完成业务编排**

按 `docs/foxops-web/FoxOps 工作区页面完整迁移方案.md` 的初始化、浏览、CRUD、预览、知识库、响应式和异常状态逐项实现。

- [x] **Step 4: 验证绿灯和编译**

Run: `node --test tests/workspace-utils.test.mjs`

Run: `pnpm exec vue-tsc --noEmit`

Expected: 退出码 0。

### Task 7: 完整验证

**Files:**
- Verify only: `foxops-web/**`

- [x] **Step 1: 运行单元/契约测试**

Run: `node --test tests/workspace-utils.test.mjs`

- [x] **Step 2: 运行类型检查与构建**

Run: `pnpm exec vue-tsc --noEmit`

Run: `pnpm build`

- [x] **Step 3: 运行 Lint**

Run: `pnpm lint -- src/api/workspace.ts src/api/knowledge.ts src/router/modules/workspace.ts src/views/workspace/index.vue src/components/workspace src/utils/workspace.js tests/workspace-utils.test.mjs`

- [x] **Step 4: 运行差异和范围审计**

确认未修改 Chat、package.json、pnpm-lock.yaml 和后端；逐项核对最终验收标准。

