> 历史文档，内容截至 2026-10-11，现状见 [docs/02-架构/架构文档导航.md](docs/02-架构/架构文档导航.md) 与当前代码（本文不再维护）。

# 自建组件复用清单

目标：把 app 里自己手写的组件，尽量换成 shadcn/ui、AI Elements、assistant-ui 三个库里的现成组件。本文列出全部自建组件、可替代的库组件和文档链接，并按迁移成本排序。

- 盘点范围：`apps/desktop/src` 下的外壳、聊天区、右侧面板、各功能页，共 **105 项**自建组件（外壳和聊天 60 项，功能页 45 项）。
- 详细盘点（含代码行号、需保留的行为、可删 CSS 行数）：`output/component-reuse/inventory-shell-chat.md`、`output/component-reuse/inventory-pages.md`。
- 盘点时另一个 agent 正在改 composer、timeline、thread 和部分页面。本文表格里的代码位置已重新逐条核对，行号以本文为准，核对方法见第 0 节末尾的“代码位置说明”。
- 文中所有链接都已逐个打开验证（152 个，全部可访问）。

## 0. 三个库怎么看、怎么装

| 库 | 文档（看长什么样） | 安装方式 | 说明 |
|---|---|---|---|
| shadcn/ui | `https://ui.shadcn.com/docs/components/radix/<名字>` | `npx shadcn add <名字>` | 基础组件。项目用的是 Radix 版本（`radix-mira` 风格），链接都指向 Radix 版文档 |
| AI Elements | `https://elements.ai-sdk.dev/components/<名字>` | `npx shadcn add @ai-elements/<名字>` | 聊天相关组件，项目里 `components/ai-elements/` 已有 50 个（17 个在用） |
| assistant-ui | `https://www.assistant-ui.com/elements/<名字>` | `npx shadcn add @assistant-ui/<注册名>` | 聊天和 agent 相关组件，第 5 步迁移用的就是它。设计套件另见 `https://www.assistant-ui.com/design/components/<名字>` |

已配置的工具（在 Droid 用户配置 `~/.factory/mcp.json`）：
- **shadcn MCP**（固定 4.13.0，指向 `apps/desktop`）：可以搜索、查看、安装三个库的组件，因为 AI Elements 和 assistant-ui 都通过 shadcn 注册表分发。AI Elements 没有单独的 MCP。
- **assistant-ui 文档 MCP**（`https://www.assistant-ui.com/mcp`）：查 assistant-ui 的 API 和用法。
- 命令行也能直接用：`npx shadcn search @assistant-ui -q approval`、`npx shadcn view @assistant-ui/elements-approval-card`。三个库的完整组件清单已存为 `output/component-reuse/registry-*.json`。

注意：
- assistant-ui 的聊天类组件（消息、工具调用、审批等）依赖它的运行时，只能用在聊天区，要随第 5 步一起换。页面上用的（热力图、任务进度、规格表等）装之前先用 `npx shadcn view` 看依赖。
- assistant-ui 自带一套视觉风格。装进来后是源码，要改成我们的设计变量（`UI-PLAN.md` 的规范），不能原样用。

图例：
- 匹配：**完全** = 直接换；**接近** = 少量调整；**部分** = 只能用骨架，业务部分自己写。
- 成本：**S** = 半天以内；**M** = 0.5–2 天；**L** = 2 天以上或要改数据结构。
- 链接缩写：[sc] = shadcn/ui，[ae] = AI Elements，[au] = assistant-ui。标 ✓ 的表示项目里已经装了。

代码位置说明：
- 2026-10-10 对照提交 `cd32b99`（第 5B 步，接入 assistant-ui 运行时）逐条核对。此前未提交的 composer、timeline、thread 改动已包含在这个提交里，它之前的两个提交 `61e4947`、`7e4a3ad` 是第 6 步的知识库详情页和设置页重设计。
- 两边还在继续改，行号会漂移。行号对不上时，按表里给出的组件名或类名搜索（例如 `rg -n "fox-provider-row" apps/desktop/src`）。
- 路径都相对于 `apps/desktop/src/`。常用文件用缩写：

| 缩写 | 文件 |
|---|---|
| `SP` | `features/settings/settings-pages.tsx` |
| `LK` | `features/local-knowledge/local-knowledge-pages.tsx` |
| `LKS` | `features/local-knowledge/local-knowledge-sidebar.tsx` |
| `AP` | `features/agents/agent-pages.tsx` |
| `PC` | `features/plugins/plugin-center-page.tsx` |
| `KP` | `features/knowledge/knowledge-pages.tsx` |
| `KRE` | `features/knowledge/knowledge-resource-explorer.tsx` |
| `chat/` | `features/chat/` 目录 |
| `WB` | `styles/workbench.css`（约 12,125 行） |
| `WP` | `styles/workspace-pages.css` |
| `LKC` | `styles/local-knowledge.css` |
| `PCC` | `styles/plugin-center.css` |
| `SPC` | `features/settings/settings-pages.css` |
| `GL` | `styles/globals.css` |

- 样式位置写的是该类名在文件里第一条规则的行号，同一类名往往还有更多规则。
- 编号沿用详细盘点：**S** = 外壳，**C** = 输入区，**T** = 时间线，**R** = 右侧面板，**DL** = 对话框和杂项（详细盘点里叫 D1–D8，为了不和重复簇 D1–D14 混淆，本文改叫 DL1–DL8），**P** = 功能页。
- 一行里实例超过 6 处时，表里只列 6 处代表，完整清单在文末“附录 A”。
- 标“（第 6 步已改动）”的行，组件在第 6 步重设计里改过，迁移前先看最新代码。
- “在哪看到”按界面路径写。左侧边栏顶部有“助手 / 知识”两个模式；设置在左下角齿轮里。

## 1. 你提到的两个例子

**右上角通知**：现在有两套 toast。一套是自己写的 `fox-notification-toast`（`features/chat/shell/fox-notification-center.tsx`，带队列、进度条、提示音），另一套是已经在用的 sonner（`@/features/notifications` 的 `notify`）。
- 弹出提示 → [sc] [Sonner ✓](https://ui.shadcn.com/docs/components/radix/sonner)。去掉自写的那套，提示音和系统通知回退迁到 `notify`。成本 M，可删约 100 行 CSS。
- 通知中心面板（铃铛点开的列表）→ [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Empty](https://ui.shadcn.com/docs/components/radix/empty) + [Popover ✓](https://ui.shadcn.com/docs/components/radix/popover)；也可以参考 [au] [Background runs](https://www.assistant-ui.com/elements/background-inbox)（后台任务收件箱）。轮询和数据逻辑保留，只换展示层。成本 M，可删约 430 行 CSS。

| 编号 | 项 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|---|
| S1 | 自写 toast | 窗口右上角弹出的提示 | `chat/shell/fox-notification-center.tsx:366`（容器 `.fox-notification-toast-host`，单条 `:367`）；sonner 的 `notify` 在 `features/notifications.ts` | `WB:646 .fox-notification-toast-host`、`WB:658 .fox-notification-toast*` |
| S2、S3 | 通知中心和铃铛 | 左侧边栏右上角的铃铛（助手、知识模式，侧栏展开时才有）。点开后列表显示在左侧边栏里，不是弹出层 | `chat/shell/fox-notification-center.tsx:54` NotificationCenter；铃铛 `:329`；面板 `:332`；条目 `:314`；进度条 `:318`；空、加载、错误 `:349`-`:351` | `WB:146 .fox-notification-trigger`、`WB:203 .fox-notification-sidebar`、`WB:367 .fox-notification-item`、`WB:631 .fox-notification-state` |

**知识库 / 专家 / 技能卡片**：外层用了 shadcn Card，但有约 600 行 `!important` 覆盖样式，等于把 Card 的样式全部推翻了。同类的卡片和条目一共有约 12 套 CSS 前缀（知识库卡片、专家卡片、插件卡片、能力卡片、记忆卡片、项目卡片、技能卡片、MCP 卡片、服务商行……）。
- 网格卡片 → [sc] [Card ✓](https://ui.shadcn.com/docs/components/radix/card)，只保留 Card 的默认样式加少量变体。
- 列表型条目（图标 + 标题 + 描述 + 操作）→ [sc] [Item](https://ui.shadcn.com/docs/components/radix/item)，这是最适合的组件，变体有 default / outline / muted，尺寸有 default / sm / xs。
- 专家卡片可以参考 [au] [Agent card](https://www.assistant-ui.com/elements/agent-card)；MCP 服务卡片可以参考 [au] [Server panel](https://www.assistant-ui.com/elements/mcp-server-panel)。
- 合计可删约 1,300 行 CSS，是收益最大的一项。成本 M。

| 编号 | 项 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|---|
| P2 | 专家卡片、知识库卡片 | 助手 → 专家列表；助手 → 远程知识库列表；知识 → 本地知识库列表；选择专家对话框 | `features/workspace/entity-card.tsx:29` AgentCard、`:60` KnowledgeCard；`LK:751` KnowledgeBaseCard | `WP:10 .fox-shadcn-kb-card`（26 条规则）、`WP:5893 .fox-library-card`（16 条）、`PCC:802 .fox-library-card`（22 条）、`LKC:3221`、`WB:3060` |
| P3 至 P8 | 其余卡片和条目 | 见第 1 档“服务商行……”一行和第 2 档“技能 / MCP 卡片”一行 | 见附录 A.7 和第 2 档 P7 行 | 见附录 A.7 和第 2 档 P7 行 |

## 2. 按成本排序的清单

### 第 0 档：不用换组件，直接清理（先确认再删）

| 编号 | 项 | 内容 | 在哪看到 | 代码位置 | 样式位置 | 收益 |
|---|---|---|---|---|---|---|
| 无 | 死 CSS | `workbench.css` 约 2,564 行（约占 24%）、`local-knowledge.css` 约 1,297 行、`workspace-pages.css` 约 771 行、`plugin-center.css` 约 48 行。这些类名在代码里没有任何引用 | 界面上看不到 | 无 | `WB`、`LKC`、`WP`、`PCC` 全文件，按前缀逐个复核 | 约 4,700 行。行数是脚本按前缀估的上限，删之前每个前缀都要用 `rg` 复核，并截图检查 |
| DL5、DL6、DL7、DL8、P3（目录分支）、P11、P37（导入页那份） | 死页面和死组件 | `LocalKnowledgeImportPage`（没有任何地方使用）；插件中心的目录分支（`showInstalled` 永远为 true）和 `CatalogPluginCard`；`RunReliabilityPanel` 只有测试引用，连带 `ContextBudgetPanel`、`JobProgressCard` 也没人用；`shiny-text` | 界面上看不到 | `LK:2050` LocalKnowledgeImportPage；`PC:206` `showInstalled = true`（分支 `PC:326`）、`PC:105` CatalogPluginCard、`PC:382` 分类胶囊；`chat/components/RunReliabilityPanel.tsx:27`；`chat/components/ContextBudgetPanel.tsx:21`；`features/jobs/JobProgressCard.tsx:31`；`components/effects/shiny-text.tsx:11` | `PCC:162 .fox-plugin-categories`、`PCC:207 .fox-plugin-card`；`chat/context-budget.css:3`；`components/effects/shiny-text.css`；RunReliabilityPanel 用 Tailwind | 约 190 行 TSX 以上。`RunReliabilityPanel` 是否计划接入需要你确认 |
| 无 | 没用的 ai-elements | 33 个文件没有使用。其中 plan、task、queue、context、model-selector、reasoning、terminal、checkpoint 后面可能用得上，先留着；canvas、node、edge、persona、语音相关等可以删 | 界面上看不到 | `components/ai-elements/`（目前 50 个 `.tsx`） | 无 | 减少维护量 |

### 第 1 档：S，半天以内，换上就行

| 编号 | 项 | 现在 | 在哪看到 | 代码位置 | 样式位置 | 替换为 | 匹配 |
|---|---|---|---|---|---|---|---|
| P41、DL2、DL3 | 原生确认框 | 多处 `window.confirm` / `window.prompt`；删除会话、删除项目用普通 Dialog | 设置 → 模型供应商 → 删除或重命名供应商；设置 → 数据与诊断 → 清理、恢复；专家详情 → 删除专家；左侧边栏回收站 → 永久删除对话；项目“…” → 删除项目，共 14 处 | `SP:557`（confirm）、`SP:561`（prompt）、`SP:1087`、`AP:647`、`chat/conversation-dialogs.tsx:163`（永久删除）、`:173` ProjectDeleteDialog<br>另见附录 A.1 | 原生对话框没有样式；两个普通 Dialog 用默认样式 | [sc] [Alert Dialog ✓](https://ui.shadcn.com/docs/components/radix/alert-dialog) | 完全 |
| P22、S15 | 加载状态（约 10 种） | 各页面自写的“加载中…”和转圈 | 本地知识库各页、插件中心、设置 → 使用统计 / 数据与诊断、右侧面板 → 文件、应用启动和切换页面 | `LK:433` LocalKnowledgeLoading、`PC:341`、`SP:797`、`SP:1086`、`chat/right-panel/fox-right-panel.tsx:689`、`chat/workbench.tsx:1159`<br>另见附录 A.2 | `LKC:325 .fox-local-kb-loading`、`PCC:591 .fox-plugin-loading`、`WP:3151 .fox-usage-loading`、`SPC:572 .fox-diagnostic-loading`、`WB:7099 .fox-panel-loading`、`WB:5060 .fox-app-loading` | [sc] [Spinner ✓](https://ui.shadcn.com/docs/components/radix/spinner)、[Skeleton ✓](https://ui.shadcn.com/docs/components/radix/skeleton) | 完全 |
| P23、P9 | 错误提示、连接测试结果 | 4 种错误块，2 处连接结果 | 设置页读取失败；本地知识库各页；插件中心；文件预览失败；设置 → 知识库服务 → 服务连接 → 测试连接 | `SP:96` SettingsLoadError、`LK:437` LocalKnowledgeError、`PC:338`、`LK:882`、`features/knowledge/knowledge-file-viewer.tsx:217`、`SP:1139`<br>另见附录 A.3 | SettingsLoadError 用 Tailwind；`LKC:326 .fox-local-kb-error`、`PCC:390 .fox-plugin-error`、`WP:745 .fox-file-preview-state`、`WP:27 .fox-connection-result` | [sc] [Alert ✓](https://ui.shadcn.com/docs/components/radix/alert)；参考 [au] [Error state](https://www.assistant-ui.com/elements/error-state) | 完全 |
| P25 | 进度条（6 处） | `div` 加百分比宽度 | 本地知识库存储位置迁移；本地知识库详情顶部任务条；任务中心；向量模型下载；设置 → 数据与诊断 → 预览缓存；设置 → 使用统计 | `LK:401`、`LK:1839`、`LK:2422`、`LK:2570`、`SP:1087`、`SP:820`<br>另见附录 A.4（实际 7 处） | `LKC:1627 .fox-local-kb-storage-progress`、`LKC:1644 .fox-local-kb-progress`、`WP:2882 .fox-preview-cache-meter`、`WP:3154 .fox-usage-rank` | [sc] [Progress ✓](https://ui.shadcn.com/docs/components/radix/progress) | 完全 |
| P39、R5、DL4、C8、DL5 | 手写展开收起（约 9 处）（第 6 步已改动） | 按钮加箭头旋转、原生 `<details>` | 新建 / 编辑本地知识库 → 高级选项；设置 → 数据与诊断 → 正常项；本地知识库检查器 → 存储；任务中心按天分组；连接器卡片展开；右侧面板 → 运行详情 | `LK:572`、`SP:1086`、`LK:1536`、`LK:2380`、`SP:944`、`chat/components/ActivityDetailPanel.tsx:53`<br>另见附录 A.5 | `LKC:4848 .fox-local-kb-advanced-toggle`、`SPC:580 .fox-diagnostic-ok-toggle`、`LKC:4466 .fox-local-kb-inspector-toggle`、`LKC:1212 .fox-local-kb-job-day-toggle`、`WP:2713 .fox-mcp-card`、`WB:6837 .fox-activity-detail` | [sc] [Collapsible ✓](https://ui.shadcn.com/docs/components/radix/collapsible) | 完全 |
| P40 | 插件安装范围弹窗 | 手写遮罩和原生勾选框 | 插件中心 → 设置插件给哪些专家使用 | `PC:164` ScopeDialog（遮罩 `PC:178`，勾选框 `PC:185`），在 `PC:346`、`PC:392` 渲染 | `PCC:610 .fox-plugin-scope-backdrop`、`PCC:620 .fox-plugin-scope-dialog` | [sc] [Dialog ✓](https://ui.shadcn.com/docs/components/radix/dialog) + [Checkbox](https://ui.shadcn.com/docs/components/radix/checkbox) | 完全 |
| P19 | 文字大小滑块 | 原生 `input[type=range]` | 设置 → 应用 → 外观 | `SP:201`（SettingsPage `SP:174` 内） | `SPC:148 .fox-text-scale-control` | [sc] [Slider](https://ui.shadcn.com/docs/components/radix/slider) | 完全 |
| P17、P16、T5、DL4 | 单选：主题选项、导入目标、反馈原因、对账选项 | 手写 `role=radio` 或原生 radio | 设置 → 应用 → 外观（主题）；知识 → 本地文件 → 选中文件后选目标知识库；聊天回复 → 反馈；运行中断后的恢复对账卡片 | `features/settings/appearance-theme.tsx:36` ThemeOption（`:47`）、`:68` OptionGroup（`:72`）；`LK:1343`；`chat/timeline/fox-runtime-assistant-message.tsx:224`；`chat/kernel-reconciliation-panel.tsx:85` | 主题选项用 Tailwind，没有独立 CSS；`LKC:2375 .fox-local-files-base-options`；`WB:11710 .fox-feedback-categories`；`chat/kernel-reconciliation.css:1 .fox-reconciliation` | [sc] [Radio Group](https://ui.shadcn.com/docs/components/radix/radio-group) | 完全 |
| P18、S9 | 头像 / 图标选择（4 处，其中 2 处完全重复） | 手写网格单选 | 设置 → 模型供应商 → 编辑供应商（图标）；助手 → 专家 → 新建专家（图标）；专家详情 → 编辑资料；左下角头像菜单 → 个人资料（头像） | `SP:488` ProviderIconPicker（`SP:509`）；`AP:760`；`AP:835` AgentProfileEditDialog；`features/profile/user-profile.tsx:122` | `WP:3077 .fox-provider-icon-grid`、`WP:4691 .fox-agent-editor-icon-field`、`WP:82 .fox-avatar-picker` | [sc] [Toggle Group](https://ui.shadcn.com/docs/components/radix/toggle-group) + [Avatar ✓](https://ui.shadcn.com/docs/components/radix/avatar)，抽成一个共用组件 | 完全 |
| P15、DL1 | 勾选列表：专家资源、绑定知识库 | 原生 checkbox | 新建专家 → 能力配置；聊天输入框“+” → 选择知识库 | `AP:276` ExpertResourceList（`AP:770` 使用）；`chat/conversation-dialogs.tsx:21` KnowledgeBindingDialog（勾选 `:135`） | `WP:4818 .fox-agent-editor-resource-list`；`WB:5685 .fox-knowledge-binding-dialog`、`WB:5729 .fox-knowledge-picker-row` | [sc] [Checkbox](https://ui.shadcn.com/docs/components/radix/checkbox) + [Item](https://ui.shadcn.com/docs/components/radix/item) | 完全 |
| P13 | 设置行（约 25 处） | `fox-setting-row` | 设置各页 | `SP:88` PreferenceRow、`SP:92` SettingValue、`SP:167`（通知）、`SP:201`、`SP:391`、`SP:737`<br>另见附录 A.6 | `SPC:95 .fox-setting-row`、`SPC:111 .fox-setting-value`、`WP:3062 .fox-provider-default-toggle` | [sc] [Field](https://ui.shadcn.com/docs/components/radix/field)（横向排列） | 完全 |
| P36 | 面包屑 | 本地知识库文档页自写 | 知识 → 本地知识库 → 打开知识库 → 文档 | `LK:2020`（LocalKnowledgeDocumentsPage `LK:1850` 内） | `LKC:2888 .fox-local-kb-breadcrumbs` | [sc] [Breadcrumb](https://ui.shadcn.com/docs/components/radix/breadcrumb) | 完全 |
| P8、P3、P4、P5、P6 | 服务商行、服务卡片、插件卡片、能力卡片、记忆卡片、项目卡片（第 6 步已改动） | 各自一套 CSS | 设置 → 模型供应商 / 知识库服务 / 扩展 / 项目与权限；助手 → 插件；专家详情 → 知识库、技能、连接器、工具、记忆 | `SP:584`、`SP:624`、`PC:149`、`AP:1018`、`AP:231`、`SP:700`<br>另见附录 A.7 | `SPC:601 .fox-provider-row`、`SPC:246 .fox-service-card`、`PCC:333 .fox-plugin-installed-card`、`WP:6783 .fox-cap-card`、`WP:4221 .fox-memory-card`、`WP:5416 .fox-managed-project` | [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Switch ✓](https://ui.shadcn.com/docs/components/radix/switch) | 接近–完全 |
| P45 | 选择专家对话框 | 自写搜索加列表 | 聊天输入框“+” → 添加专家 | `features/agents/ExpertPickerDialog.tsx:34`（搜索 `:36`），在 `chat/workbench.tsx:1170` 渲染 | `WB:3039 .fox-expert-picker-dialog`、`WB:3048 .fox-expert-picker-search`、`WB:3051 .fox-expert-picker-grid` | [sc] [Command ✓](https://ui.shadcn.com/docs/components/radix/command)（CommandDialog） | 接近 |
| S13、S14 | 聊天顶栏的弹出层、会话摘要 | 自写 | 聊天页顶栏：项目名弹出层、运行中和待回答标记、会话摘要 | `chat/shell/fox-chat-topbar.tsx:57` ChatTopbar（项目弹出层 `:77`，标记 `:103`-`:107`）、`:16` ConversationSummaryPopover | `WB:2139 .fox-chat-topbar`、`WB:2206 .fox-chat-project-popover`、`WB:2324 .fox-running-badge`、`WB:2344 .fox-question-badge`、`WB:2431 .fox-conversation-summary`、`WB:2459 .fox-summary-item` | [sc] [Popover ✓](https://ui.shadcn.com/docs/components/radix/popover) + [Item](https://ui.shadcn.com/docs/components/radix/item) + [Badge ✓](https://ui.shadcn.com/docs/components/radix/badge) | 接近 |
| R8 | 右侧面板空状态 | `fox-empty-panel` | 右侧面板各模式（文件、知识库、更改、子 Agent 等）没有内容时 | `chat/right-panel/fox-right-panel.tsx:262`、`:673`、`:702`、`:713`；另有 `.fox-context-empty` `:730` | `WB:7959 .fox-empty-panel`；`.fox-context-empty` 没有样式 | [sc] [Empty](https://ui.shadcn.com/docs/components/radix/empty) | 完全 |
| P37 | 上传拖放区 | 2 份（其中一份在死页面里） | 本地知识库详情 → 文档 → 导入 | `LK:2038`；死页面里那份在 `LK:2125` | `LKC:1047 .fox-local-kb-import-dropzone` | [sc] [Attachment](https://ui.shadcn.com/docs/components/radix/attachment) | 接近 |
| P43 | 预览工具栏 | 自写按钮排 | 远程知识库 → 打开文件预览（PDF、Word、PPT、图片、文本） | `features/knowledge/knowledge-preview-controls.tsx:4` PreviewToolbar、`:8` PreviewIconButton；`features/knowledge/knowledge-document-activity-toolbar.tsx:9`；使用处如 `features/knowledge/knowledge-pdf-viewer.tsx:291`、`features/knowledge/knowledge-file-viewer.tsx:227` | `WP:740 .fox-preview-toolbar`、`WP:1442 .fox-document-activity-toolbar` | [sc] [Button Group ✓](https://ui.shadcn.com/docs/components/radix/button-group) | 接近 |
| S5 | 会话右键菜单和“更多”菜单 | 同样的菜单项写了两遍 | 左侧边栏 → 对话或项目对话，右键或点“…” | `chat/shell/fox-sidebar.tsx:457` DropdownMenu、`:479` ContextMenu；项目对话 `:407`、`:420`；项目菜单 `:378`、`:391` | `WB:1548 .fox-conversation-menu`、`WB:2391 .fox-project-menu` | 已经用的是 [Context Menu ✓](https://ui.shadcn.com/docs/components/radix/context-menu) 和 [Dropdown Menu ✓](https://ui.shadcn.com/docs/components/radix/dropdown-menu)，只需合并成一份 | 完全 |
| P32 | 文件类型图标 | 两套函数 | 远程知识库的文件树；知识 → 本地文件列表 | `KRE:103` knowledgeDocumentIcon；`features/local-knowledge/local-file-type-icon.tsx:4` LocalFileTypeIcon | 只返回图标，没有专用样式 | 没有对应组件，合并成一个函数 | 无 |

### 第 2 档：M，0.5–2 天，要调整结构

| 编号 | 项 | 现在 | 在哪看到 | 代码位置 | 样式位置 | 替换为 | 匹配 |
|---|---|---|---|---|---|---|---|
| S1 | **通知弹窗** | 两套 toast | 窗口右上角 | `chat/shell/fox-notification-center.tsx:366`；sonner 在 `features/notifications.ts` | `WB:646 .fox-notification-toast-host`、`WB:658 .fox-notification-toast*` | [sc] [Sonner ✓](https://ui.shadcn.com/docs/components/radix/sonner) | 接近 |
| S2、S3 | **通知中心** | 自写面板 | 左侧边栏右上角铃铛，点开后在左侧边栏内显示 | `chat/shell/fox-notification-center.tsx:54`（铃铛 `:329`，面板 `:332`，条目 `:314`） | `WB:146 .fox-notification-trigger`、`WB:203 .fox-notification-sidebar`、`WB:367 .fox-notification-item` | [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Empty](https://ui.shadcn.com/docs/components/radix/empty) + [Popover ✓](https://ui.shadcn.com/docs/components/radix/popover)；参考 [au] [Background runs](https://www.assistant-ui.com/elements/background-inbox) | 部分 |
| P2 | **知识库 / 专家卡片** | Card 加约 600 行覆盖 | 助手 → 专家列表 / 远程知识库列表；知识 → 本地知识库列表 | `features/workspace/entity-card.tsx:29` AgentCard、`:60` KnowledgeCard；`LK:751` KnowledgeBaseCard | `WP:10 .fox-shadcn-kb-card`、`WP:5893 .fox-library-card`、`PCC:802 .fox-library-card` | [sc] [Card ✓](https://ui.shadcn.com/docs/components/radix/card) + [Item](https://ui.shadcn.com/docs/components/radix/item)；参考 [au] [Agent card](https://www.assistant-ui.com/elements/agent-card) | 接近 |
| P7 | 技能 / MCP 卡片 | 两套，MCP 带展开 | 专家详情 → 技能 / 连接器 → 添加技能 / 添加连接器（进入技能页、连接器页） | `SP:850` SkillsPage（卡片 `SP:856`）；`SP:913` McpPage（卡片 `SP:944`，Hook `SP:948`） | `WP:2606 .fox-skill-card`、`WP:2713 .fox-mcp-card` | [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Collapsible ✓](https://ui.shadcn.com/docs/components/radix/collapsible)；参考 [au] [Server panel](https://www.assistant-ui.com/elements/mcp-server-panel) | 接近 |
| P21 | 空状态（10 种，约 30 处） | 每页一种写法 | 各页面没有数据时 | `KP:78`、`AP:1193`、`PC:193` EmptyState、`LK:441` LocalKnowledgeEmpty、`SP:856`、`chat/shell/fox-sidebar.tsx:497`<br>另见附录 A.8 | `WP:88 .fox-page-empty-state`、`WP:5285 .fox-agent-list-empty`、`PCC:592 .fox-plugin-empty`、`LKC:327 .fox-local-kb-empty`、`WP:2618 .fox-skill-empty`、`WP:124 .fox-project-empty` | [sc] [Empty](https://ui.shadcn.com/docs/components/radix/empty) | 完全 |
| P10、P11 | 分段控件（7 处）（第 6 步已改动） | 用 CSS 把 Tabs 硬改成胶囊。大部分没有对应面板，其实是筛选 | 本地知识库详情（文档 / 检索测试 / 任务）；任务中心状态筛选；本地文件类型；检索方式；插件类型；新建专家 | `LK:1810`、`LK:2348`、`LK:1325`、`LK:2783`、`PC:335`、`AP:1200`<br>另见附录 A.9 | `LKC:550 .fox-local-kb-detail-switcher`、`LKC:558 .fox-local-kb-job-filter-tabs`、`LKC:559 .fox-local-files-categories`、`LKC:4590 .fox-local-retrieval-mode-tabs`、`PCC:57 .fox-plugin-kind-tabs`、`WP:4528 .fox-agent-editor-tabs` | 筛选用 [sc] [Toggle Group](https://ui.shadcn.com/docs/components/radix/toggle-group)；真正切换面板的继续用 [Tabs ✓](https://ui.shadcn.com/docs/components/radix/tabs) | 完全 |
| P12、S7 | 搜索框（9 种） | label 加图标加 Input | 专家列表、远程知识库列表、本地文件、本地知识库列表、插件中心、专家能力分区等 | `AP:1180`、`KP:69`、`LK:1316`、`LK:1477`、`PC:332`、`AP:1010`<br>另见附录 A.10 | `WP:5262 .fox-list-page-search`、`LKC:96 .fox-local-kb-search`、`PCC:51 .fox-plugin-search`、`WP:6745 .fox-capability-search` | [sc] [Input Group ✓](https://ui.shadcn.com/docs/components/radix/input-group) | 完全 |
| P14 | 表单字段（约 40 个） | label 包 input，错误提示各写各的 | 新建 / 编辑本地知识库；编辑模型供应商；连接器表单；新建专家；数据与诊断 | `LK:449` CreateKnowledgeBaseDialog、`LK:593` EditKnowledgeBaseDialog、`SP:526` ProviderEditor、`SP:951`、`AP:751` AgentEditorProfileFields、`SP:1089`<br>另见附录 A.11 | `LKC:1677 .fox-local-kb-create-form`、`WP:3065 .fox-provider-form-grid`、`WP:2841 .fox-mcp-form`、`WP:4591 .fox-agent-editor-grid`、`WP:2924 .fox-maintenance-field` | [sc] [Field](https://ui.shadcn.com/docs/components/radix/field) + [Label](https://ui.shadcn.com/docs/components/radix/label) | 完全 |
| P24 | 状态点 / 状态标签（9 种以上）（第 6 步已改动） | 各页自写圆点和胶囊 | 模型供应商行；数据与诊断；本地知识库详情就绪状态；任务中心；向量模型；插件中心 | `SP:588`、`SP:1086`、`LK:1790`、`LK:2417`、`LK:2572`、`PC:94` StatusBadge<br>另见附录 A.12 | `GL:315 .fox-status-dot`、`SPC:644 .fox-provider-row-status`、`LKC:4356 .fox-local-kb-readiness-line`、`LKC:4720 .fox-local-kb-job-row-status`、`LKC:4780 .fox-local-model-row-status`、`PCC:510 .fox-plugin-status` | [sc] [Badge ✓](https://ui.shadcn.com/docs/components/radix/badge) 加 success / warning 变体，统一成一个 StatusBadge；运行中动画参考 [au] [Agent status](https://www.assistant-ui.com/elements/agent-status) | 接近 |
| P1 | 页面头部（4 套） | `WorkspacePage` 接收标题参数却不显示，所以每页自己写 | 专家、知识库、本地知识库、设置各页顶部 | `features/workspace/page-shell.tsx:3` WorkspacePage（`:4`-`:8` 丢弃参数）、`LK:418` LocalKnowledgeHeader、`SP:83` SettingsScaffold、`AP:471` AgentSectionLayout | `WP:1 .fox-page-shell`、`LKC:25 .fox-local-kb-header`、`SPC:10 .fox-settings-layout`、`WP:4030 .fox-agent-section-layout` | 先修 `WorkspacePage`，再统一用 [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Breadcrumb](https://ui.shadcn.com/docs/components/radix/breadcrumb) | 接近 |
| S7 | 全局搜索 | 自写对话框 | 左侧边栏顶部搜索按钮，或按 Ctrl K | `chat/shell/fox-sidebar.tsx:644` Dialog（输入框 `:657`，状态 `:663`） | `WB:1563 .fox-global-search-dialog`、`WB:1577 .fox-global-search-input-shell`、`WB:1684 .fox-global-search-state` | [sc] [Command ✓](https://ui.shadcn.com/docs/components/radix/command)（CommandDialog）；参考 [au] [Command palette](https://www.assistant-ui.com/elements/command-palette) | 完全 |
| P31 | 文件树（2 套） | 知识库详情和本地知识库侧栏各写一套 | 远程知识库详情 → 左侧边栏文件树；知识 → 我的知识库 → 展开某个知识库的文档树 | `KRE:136` DocumentTree（建树 `KRE:114`，在 `chat/shell/fox-sidebar.tsx:510` 渲染）；`LKS:154` renderTree（建树 `LKS:44`） | `WP:463 .fox-file-tree`、`WP:467 .fox-file-tree-node`、`LKC:2559 .fox-local-knowledge-tree-folder`、`LKC:2563 .fox-local-knowledge-tree-children` | [ae] [File Tree ✓](https://elements.ai-sdk.dev/components/file-tree)（右侧面板已经在用）；备选 [au] [File tree](https://www.assistant-ui.com/elements/file-tree) | 完全 |
| P33、P34、P35 | 文件列表、文档列表、模型行 | CSS 网格模拟表格 | 知识 → 本地文件；本地知识库详情 → 文档；知识 → 最近；知识 → 向量模型；检索测试 → 三模式对比 | `LK:1061` LocalFileRow、`LK:1036` LocalFileSourceRow、`LK:1338` 列表头、`LK:2023`、`LK:1411`、`LK:2566`<br>另见附录 A.13 | `LKC:2117 .fox-local-files-list`、`LKC:4823 .fox-local-files-source-row`、`LKC:2934 .fox-local-kb-browser-list`、`LKC:2662 .fox-local-kb-recent-list`、`LKC:4768 .fox-local-model-row` | [sc] [Table](https://ui.shadcn.com/docs/components/radix/table) + [Checkbox](https://ui.shadcn.com/docs/components/radix/checkbox)；需要排序时参考 [au] [Data table](https://www.assistant-ui.com/elements/data-table) | 接近 |
| P38 | 知识库检查器（右侧 280px）（第 6 步已改动） | `dl` 加自写折叠 | 本地知识库详情右侧栏；远程知识库 → 知识图谱右侧栏 | `LK:1498` KnowledgeInspector（根 `LK:1511`，存储折叠 `LK:1536`）；`KP:249` | `LKC:4403 .fox-local-kb-inspector`、`LKC:4466 .fox-local-kb-inspector-toggle`、`WP:25 .fox-graph-inspector` | [sc] [Item](https://ui.shadcn.com/docs/components/radix/item) + [Collapsible ✓](https://ui.shadcn.com/docs/components/radix/collapsible)，窄屏用 [Sheet ✓](https://ui.shadcn.com/docs/components/radix/sheet)；键值部分参考 [au] [Spec sheet](https://www.assistant-ui.com/elements/spec-sheet) | 接近 |
| P26 | 任务中心的任务条目 | 自写时间线 | 知识 → 任务中心；本地知识库详情 → 任务 | `LK:2403` JobTimelineItem；`LK:2237` LocalKnowledgeJobsPage（时间线 `LK:2369`，按天折叠 `LK:2380`） | `LKC:1185 .fox-local-kb-job-timeline`、`LKC:1212 .fox-local-kb-job-day-toggle`、`LKC:4734 .fox-local-kb-job-row-progress` | [au] [Job progress](https://www.assistant-ui.com/elements/job-progress) + [Timeline](https://www.assistant-ui.com/elements/timeline)；备选 [ae] [Task](https://elements.ai-sdk.dev/components/task) | 接近 |
| P28 | 活动热力图（2 处逐行复制） | 专家工作记录、用量统计 | 专家详情首页的工作记录；设置 → 使用统计 | `AP:342` AgentWorkRecord（热力图 `AP:407`-`:416`，在 `AP:1310` 使用）；`SP:750` UsageStatisticsPage（`SP:804`-`:815`） | `WP:3466 .fox-agent-activity-panel` | [au] [Heat graph](https://www.assistant-ui.com/elements/heat-graph)；备选 [Activity graph](https://www.assistant-ui.com/elements/activity-graph) | 完全 |
| P29 | 统计数字块（7 种） | 各页自写 | 专家工作记录；使用统计；数据与诊断；项目卡片；知识图谱概览；文件信息 | `AP:402`、`SP:801`、`SP:1087`、`SP:707`、`KP:252`、`KP:194`<br>另见附录 A.14 | `WP:3153 .fox-agent-work-metrics`、`SPC:512 .fox-maintenance-stats`、`WP:5481 .fox-managed-project-metrics`、`WP:2142 .fox-graph-overview`、`WP:750 .fox-document-meta-strip` | [au] [Spec sheet](https://www.assistant-ui.com/elements/spec-sheet)，数字动画 [Number ticker](https://www.assistant-ui.com/elements/number-ticker)；或 [sc] [Card ✓](https://ui.shadcn.com/docs/components/radix/card) 加共用组件 | 接近 |
| P30 | 用量条形图 | 自写横条 | 设置 → 使用统计 → 专家使用、Token 构成 | `SP:820`、`SP:821`（UsageStatisticsPage `SP:750` 内） | `WP:3154 .fox-usage-agents`、`.fox-usage-rank`、`WP:3156 .fox-token-summary` | [sc] [Chart](https://ui.shadcn.com/docs/components/radix/chart)；参考 [au] [Cost meter](https://www.assistant-ui.com/elements/cost-meter) | 接近 |
| P42 | 检索测试结果 | 自写分数和片段 | 本地知识库详情 → 检索测试 | `LK:2793`（列表 `LK:2795`，分数 `LK:2796`） | `LKC:3473 .fox-local-retrieval-result` | [au] [Retrieval chunks](https://www.assistant-ui.com/elements/retrieval-chunks)；备选 [ae] [Sources ✓](https://elements.ai-sdk.dev/components/sources) | 接近 |
| P27 | 运行记录 / 检索追踪 | 自写列表 | 专家列表 → 数字同事 → 运行记录 | `features/agents/digital-colleague-manager.tsx:250`（事实块 `:259`，状态 `:261`）。检索测试页（`LK:2611`）里已经找不到追踪列表，现在只有三模式对比 `LK:2802` 和回归问题 `LK:2806` | `WP:5714 .fox-colleague-run-section`、`WP:5759 .fox-colleague-run-facts`、`WP:5819 .fox-colleague-run-state` | [au] [Trace waterfall](https://www.assistant-ui.com/elements/trace-waterfall)、[Run activity](https://www.assistant-ui.com/elements/run-activity) | 部分 |
| P20 | 模型选择 | Select 加描述行 | 新建 / 编辑本地知识库 → 向量模型；设置 → 助手设置 → 默认模型 | `LK:570`、`LK:689`、`SP:391`（AiSettingsPage `SP:304`）。检查器 `LK:1498` 和向量模型页 `LK:2447` 里现在没有 Select | `LKC:3095 .fox-local-kb-index-settings-title`（所在区块）；其余沿用 Select 组件样式 | 先保留 [sc] [Select ✓](https://ui.shadcn.com/docs/components/radix/select)；要分组和能力展示时用 [au] [Model selector](https://www.assistant-ui.com/elements/model-selector) | 部分 |
| P44 | 新手引导步骤点 | 自写 | 设置 → 应用 → 首次设置向导 | `SP:210` OnboardingPage（进度点 `SP:267`，步骤 `SP:268`） | `WP:3141 .fox-onboarding-progress` | [au] [Onboarding](https://www.assistant-ui.com/elements/onboarding)；或 [sc] [Carousel ✓](https://ui.shadcn.com/docs/components/radix/carousel) | 部分 |
| S11 | 窗口标题栏菜单 | DropdownMenu 拼的 | 窗口顶部标题栏的菜单 | `chat/shell/fox-window-titlebar.tsx:102` WindowMenu（标题栏 `:7`，`<header>` `:57`） | `WB:20 .fox-window-titlebar`、`WB:131 .fox-window-menu-content` | [sc] [Menubar](https://ui.shadcn.com/docs/components/radix/menubar)。最小化、最大化、关闭按钮保留自写 | 接近 |
| S12 | 拖拽分隔条（2 处） | pointer 事件手写 | 左侧边栏和聊天区之间；聊天区和右侧面板之间 | `chat/workbench.tsx:92` SidebarResizeDivider、`:77` ResizeDivider | `WB:895 .fox-sidebar-resize-divider`、`WB:6535 .fox-resize-divider` | [sc] [Resizable ✓](https://ui.shadcn.com/docs/components/radix/resizable) | 接近 |
| S6 | 侧栏折叠后的悬停菜单 | 手写 `role=menu` | 左侧边栏折叠成图标后，鼠标悬停在图标上 | `chat/shell/fox-sidebar.tsx:529`-`:609`（弹出层 `:546`） | `WB:1831 .fox-sidebar-collapsed-entry`、`WB:1883 .fox-sidebar-collapsed-popover` | [sc] [Hover Card ✓](https://ui.shadcn.com/docs/components/radix/hover-card)，或随侧边栏一起换 | 接近 |
| R1、R2 | 右侧面板标签栏 | 手写 tablist（可关闭、动态增加） | 右侧面板顶部标签（知识库、文件、更改、运行详情、网页与网络、子 Agent）；网页与网络面板里的标签 | `chat/right-panel/fox-right-panel.tsx:752` RightPanel（tablist `:767`，模式列表 `:740`）；网页面板 `:334`（标签 `:366`） | `WB:6596 .fox-context-tabbar`、`WB:6605 .fox-context-tabs`、`WB:6676 .fox-context-tab`、`WB:8104 .fox-browser-tabs` | [sc] [Tabs ✓](https://ui.shadcn.com/docs/components/radix/tabs) 加关闭按钮 | 接近 |
| R3 | 子智能体列表 | 自写 | 右侧面板 → 子 Agent | `chat/right-panel/fox-right-panel.tsx:498` ChildAgentPanel（列表 `:512`）、`:535` ChildAgentConversationPanel、`:395` ChildRunStatusIcon | `WB:9994 .fox-agent-list`、`WB:10008 .fox-agent-list-item`、`WB:10281 .fox-agent-user-bubble` | [au] [Subagent list](https://www.assistant-ui.com/elements/subagent-list)；备选 [ae] [Agent ✓](https://elements.ai-sdk.dev/components/agent) | 接近 |
| R4 | 改动和差异视图 | 自写 span | 右侧面板 → 更改 | `chat/right-panel/fox-right-panel.tsx:705`-`:728` | `WB:7808 .fox-change-panel`、`WB:7865 .fox-diff-view`、`WB:11425 .fox-command-detail` | [au] [Code diff](https://www.assistant-ui.com/elements/code-diff)、[Diff Viewer](https://www.assistant-ui.com/design/components/diff-viewer)；命令输出 [Terminal block](https://www.assistant-ui.com/elements/terminal-block) | 接近 |
| R5 | 活动详情 | 原生 `<details>` | 右侧面板 → 运行详情 | `chat/components/ActivityDetailPanel.tsx:59`（根 `:99`，`<details>` `:53`、`:112`），在 `chat/right-panel/fox-right-panel.tsx:730` 使用 | `WB:6837 .fox-activity-detail` | [au] [Tool call](https://www.assistant-ui.com/elements/tool-call) + [sc] [Collapsible ✓](https://ui.shadcn.com/docs/components/radix/collapsible) | 接近 |

### 第 3 档：L，2 天以上

| 编号 | 项 | 现在 | 在哪看到 | 代码位置 | 样式位置 | 替换为 | 匹配 |
|---|---|---|---|---|---|---|---|
| S4、S8 | 左侧边栏 | 自写 `<aside>`，676 行 TSX，约 636 行 CSS | 整个左侧边栏（助手、知识两种模式，底部头像菜单） | `chat/shell/fox-sidebar.tsx:53` Sidebar（`<aside>` `:331`）；头像菜单 `:611`（菜单头 `:629`，状态 `:638`） | `WB:817 .fox-sidebar*`、`WB:1204 .fox-profile`、`WB:1995 .fox-profile-menu-head` | [sc] [Sidebar](https://ui.shadcn.com/docs/components/radix/sidebar)，参考 [sidebar-07 区块](https://ui.shadcn.com/blocks/sidebar)（可折叠成图标）；会话列表部分参考 [au] [Thread list](https://www.assistant-ui.com/elements/thread-list)、[Thread list sidebar](https://www.assistant-ui.com/elements/thread-list-sidebar) | 部分 |

### 随第 5 步一起换（聊天区，现在不要单独动）

这些都在 composer、timeline、thread 里，另一个 agent 正在迁移。把这张表交给它，迁移时直接用对应组件。下表的 `composer` 指 `chat/composer/fox-composer.tsx`，`timeline` 指 `chat/timeline/fox-runtime-timeline.tsx`。

| 编号 | 项 | 在哪看到 | 代码位置 | 样式位置 | 替换为 | 匹配 |
|---|---|---|---|---|---|---|
| C1、C2 | 斜杠命令菜单、@ 提及菜单 | 聊天输入框输入 `/` 或 `@` | `composer:548` 斜杠菜单（命令定义在 `chat/composer/fox-slash-commands.tsx`）；`composer:576` 提及菜单 | `WB:5103 .fox-slash-menu`、`WB:5104 .fox-mention-menu` | [au] [Composer trigger popover](https://www.assistant-ui.com/elements/composer-trigger-popover)（示例：[Slash commands](https://www.assistant-ui.com/elements/composer-slash-commands)、[Mentions](https://www.assistant-ui.com/elements/composer-mentions)） | 接近 |
| C6 | 模型选择（现在缺失） | 聊天输入框里没有入口 | `composer:122` 接收 `modelService`、`yuxiModels`；`composer:152` 算出 `modelOptions`；`composer:160` 模型状态，只在提交时 `composer:654` 使用 | 无 | [au] [Models](https://www.assistant-ui.com/elements/composer-model-picker)；备选 [ae] [Model Selector](https://elements.ai-sdk.dev/components/model-selector) | 完全 |
| C3 | 模式标签 | 聊天输入框底部的执行模式标签 | `composer:719` `.fox-command-mode-chip`（执行控制 `composer:712`） | `WB:6118 .fox-command-mode-chip`、`WB:6094 .fox-execution-control` | [sc] [Toggle Group](https://ui.shadcn.com/docs/components/radix/toggle-group)；参考 [au] [Reasoning effort](https://www.assistant-ui.com/elements/reasoning-effort) | 接近 |
| C4 | 附件、项目和知识标签 | 聊天输入框上方的附件和知识库标签；输入框“+”菜单 | `composer:40` ComposerAttachmentPreview、`composer:59` ComposerAttachmentCommandBridge；“+”菜单 `composer:701`-`:708` | `WB:2597 .fox-composer-attachments` | [ae] [Attachments ✓](https://elements.ai-sdk.dev/components/attachments)；[au] [Attachments](https://www.assistant-ui.com/elements/composer-attachments) | 接近 |
| C7 | 提问 / 决策卡片 | 助手提问时，在输入框位置出现 | `composer:501` 决策卡片、`composer:507` 提问 | `WB:4782 .fox-decision-card`、`WB:4804 .fox-question-decision-content` | [au] [Question flow](https://www.assistant-ui.com/elements/question-flow)、[Option list](https://www.assistant-ui.com/elements/option-list)；备选 [sc] [Questionnaire](https://ui.shadcn.com/docs/components/radix/questionnaire) | 接近 |
| C8 | 运行审批卡片 | 运行需要批准时 | `chat/components/RuntimeApprovalPrompt.tsx:27`（`<details>` `:83`） | `WB:4792 .fox-runtime-confirmation`、`WB:5981 .fox-approval-disclosure`、`WB:3258 .fox-confirmation` | [au] [Approval card](https://www.assistant-ui.com/elements/approval-card)、[Permission grant](https://www.assistant-ui.com/elements/permission-grant)；现在用的是 [ae] [Confirmation ✓](https://elements.ai-sdk.dev/components/confirmation) | 接近 |
| C9 | 错误提示、拒绝提示 | 时间线里运行出错或被拒绝时 | `chat/demo/fox-demo-prompts.tsx:142` ErrorPrompt（在 `timeline:329` 使用）、`:96` DeniedPrompt；同文件还有 `:79` ApprovalPrompt、`:159` QuestionPrompt | `WB:3260 .fox-error-prompt`、`WB:3385 .fox-denied-prompt` | [au] [Error state](https://www.assistant-ui.com/elements/error-state)、[Stopped run](https://www.assistant-ui.com/elements/stopped-run) | 接近 |
| C10 | 运行中追加指令条 | 运行中，输入框上方 | `chat/components/RunSteeringStrip.tsx:29`（在 `composer:577` 使用）；输入框里的追加按钮 `composer:455` | `WB:11784 .fox-steering-strip`、`WB:11936 .fox-steering-actions` | [au] [Message queue](https://www.assistant-ui.com/elements/message-queue)；备选 [ae] [Queue](https://elements.ai-sdk.dev/components/queue) | 接近 |
| C11 | 托管文件面板 | 输入框上方的托管文件列表 | `chat/components/ManagedFilesPanel.tsx:62`（`window.confirm` `:148`，在 `composer:583` 使用） | `WB:11972 .fox-managed-panel` | [au] [Checkpoints](https://www.assistant-ui.com/elements/checkpoint-history) + [sc] [Alert Dialog ✓](https://ui.shadcn.com/docs/components/radix/alert-dialog) | 接近 |
| C12 | 专家绑定标签 | 输入框底部的专家标签；时间线里的专家激活卡片 | `chat/components/ExpertBindingChip.tsx:24`（在 `composer:720` 使用）、`:73` ExpertActivationCard（在 `timeline:246` 使用） | `WB:6160 .fox-expert-chip`、`WB:2851 .fox-expert-activation` | [au] [Handoff](https://www.assistant-ui.com/elements/agent-handoff)、[Agent card](https://www.assistant-ui.com/elements/agent-card) | 接近 |
| C14、T10 | 目标进度、计划浮层 | 输入框上方的目标进度卡片；时间线底部的计划浮层 | `chat/components/GoalProgress.tsx:22`（在 `composer:535` 使用）、`chat/components/TaskItem.tsx:62`、`chat/components/EvidenceList.tsx:64`；`timeline:43` ConversationBottomDock | `WB:4805 .fox-goal-progress-card` 加内联 Tailwind；`WB:4565 .fox-conversation-bottom-dock`、`WB:4594 .fox-plan-floater` | [au] [Agent plan](https://www.assistant-ui.com/elements/agent-plan)、[Todo list](https://www.assistant-ui.com/elements/todo-list)；备选 [ae] [Plan](https://elements.ai-sdk.dev/components/plan) + [Task](https://elements.ai-sdk.dev/components/task) | 接近 |
| T1 | 新会话空状态 | 新对话还没有消息时 | `chat/timeline/fox-timeline.tsx:35` ConversationEmptyState（建议 `:40`）；`chat/components/NewConversationMascot.tsx:5` | `WB:2733 .fox-empty-conversation`、`WB:2755 .fox-empty-mascot` | [au] [Empty state](https://www.assistant-ui.com/elements/empty-state) + [Follow-up suggestions](https://www.assistant-ui.com/elements/follow-up-suggestions) | 接近 |
| T2 | 轮次导航条 | 聊天区边上的轮次导航 | `chat/timeline/fox-timeline.tsx:100` ConversationTurnRail（`<nav>` `:157`） | `WB:10950 .fox-conversation-rail`（`WB:2620 .fox-turn-rail` 可能是旧样式） | [au] [Conversation map](https://www.assistant-ui.com/elements/conversation-map) | 完全 |
| T3 | 用户消息气泡 | 时间线里用户发的消息 | `chat/components/UserMessageBubble.tsx:14`（在 `timeline:276` 使用） | `WB:2990 .fox-user-bubble` | [sc] [Bubble](https://ui.shadcn.com/docs/components/radix/bubble)、[Message](https://ui.shadcn.com/docs/components/radix/message)；现在用 [ae] [Message ✓](https://elements.ai-sdk.dev/components/message) | 接近 |
| T4、T5 | 消息编辑、操作栏、反馈 | 编辑已发送的消息；助手回复下方的操作栏和反馈弹窗 | `timeline:255` `.fox-user-edit`；`chat/timeline/fox-runtime-assistant-message.tsx:33`（操作栏 `:212`，反馈弹窗 `:222`，分类 `:224`） | `WB:4672 .fox-user-edit`、`WB:4420 .fox-message-actions`、`WB:11706 .fox-feedback-dialog`、`WB:11710 .fox-feedback-categories` | [au] [Edit a sent message](https://www.assistant-ui.com/elements/edit-message)、[Message actions](https://www.assistant-ui.com/elements/message-actions)、[Feedback dialog](https://www.assistant-ui.com/elements/feedback-dialog) | 完全 |
| T6 | 运行过程（思考和工具步骤） | 助手回复里的思考和工具步骤 | `chat/timeline/fox-runtime-process-view.tsx:270` RuntimeProcess、`:183` RuntimeProcessRow、`:65` RuntimeToolItem、`:20` RuntimeReasoningItem | `WB:3615 .fox-runtime-process-head`、`chat/runtime-process-groups.css:49`、`WB:4022 .fox-runtime-step-icon`、`chat/components/fox-row-shimmer.css:3`、`chat/runtime-process-row.css:3` | [au] [Tool timeline](https://www.assistant-ui.com/elements/tool-timeline) + [Tool group](https://www.assistant-ui.com/elements/tool-group) + [Reasoning](https://www.assistant-ui.com/elements/reasoning)。分组模型保留，只换展示层，成本 L | 部分 |
| T7 | 轮次耗时 | 每轮运行过程的头部 | `chat/turn-process-header.tsx:35` TurnProcessHeader（`:57`） | `chat/runtime-process-groups.css:1 .fox-process-turn-header` | [au] [Message timing](https://www.assistant-ui.com/elements/message-timing) | 接近 |
| T8 | 产物卡片 | 助手回复里的产物和文件结果 | `chat/runtime-artifacts.tsx:105` RuntimeArtifacts、`:86` ArtifactResultCard | `WB:4489 .fox-message-artifact`、`WB:11521 .fox-message-file-results` | [au] [Artifact card](https://www.assistant-ui.com/elements/artifact-card)；现在用 [ae] [Artifact ✓](https://elements.ai-sdk.dev/components/artifact) | 接近 |
| T9 | 引用来源 | 助手回复下方的来源 | `chat/timeline/fox-runtime-sources.tsx:10` | `WB:4347 .fox-sources`、`WB:5908 .fox-source-dialog` | [au] [Sources](https://www.assistant-ui.com/elements/sources)、[Document reference](https://www.assistant-ui.com/elements/document-reference) | 接近 |
| T11、T12、T13 | 消息附件、日期分隔、加载更早消息 | 用户消息的附件；时间线的日期分隔（写死“今天”）；向上翻时加载更早消息 | `timeline:25` SentMessageAttachments；`timeline:242` 和 `chat/timeline/fox-timeline.tsx:54` 的 `.fox-thread-date`；`timeline:241` `.fox-history-loader` | `WB:5556 .fox-message-attachments`、`WB:2725 .fox-thread-date`、`WB:2713 .fox-history-loader` | [au] [Attachment](https://www.assistant-ui.com/elements/attachment)、[Timestamps](https://www.assistant-ui.com/elements/day-separator)、[Thread](https://www.assistant-ui.com/elements/thread) | 完全 |
| T14 | 助手运行状态文字 | 助手头像旁的运行状态 | `chat/components/FoxAssistantAvatar.tsx:13` RunStatusText（头像 `:6`） | `WB:3638 .fox-run-status-text`、`WB:3014 .fox-assistant-avatar` | [au] [Thinking indicator](https://www.assistant-ui.com/elements/thinking-indicator) | 接近 |
| DL6 | 上下文用量条（目前没人用） | 界面上看不到 | `chat/components/ContextBudgetPanel.tsx:21` | `chat/context-budget.css:3 .fox-context-budget` | [au] [Context display](https://www.assistant-ui.com/elements/context-display)、[Context breakdown](https://www.assistant-ui.com/elements/context-breakdown)；备选 [ae] [Context](https://elements.ai-sdk.dev/components/context) | 接近 |

迁移风险：ai-elements 的 message-response、prompt-input、chain-of-thought、conversation 有本地补丁（分别 8、7、5、3 次提交），换掉之前要确认这些补丁的功能在新组件里还在。

### 建议保留自建

| 编号 | 项 | 在哪看到 | 代码位置 | 样式位置 | 原因 |
|---|---|---|---|---|---|
| S10 | 侧栏会话状态点阵图标 | 左侧边栏每个对话前的状态图标 | `chat/conversation-status-icon.tsx:59` ConversationStatusIcon（`:86`）；`chat/shell/fox-sidebar.tsx:39` ConversationRunStatus | `chat/conversation-status-icon.css:1 .fox-status-matrix`、`WB:1390 .fox-sidebar-conversation-status` | 品牌特色动画，assistant-ui 的 [Dot Matrix](https://www.assistant-ui.com/design/components/dot-matrix) 思路一样但效果不同。建议反过来把其他状态点统一到它 |
| S11（部分） | 窗口最小化、最大化、关闭按钮 | 窗口右上角 | `chat/shell/fox-window-titlebar.tsx:91` | `WB:35 .fox-window-controls` | Tauri 窗口控制，没有对应组件 |
| C13 | 发送按钮金属效果 | 聊天输入框的发送按钮 | `components/effects/metal-send-button.tsx`、`components/effects/border-beam.ts`；在 `chat/composer/fox-composer.tsx:495` 使用 | `WB:1423 .fox-send-metal`、`WB:1438 .fox-send-button`、`WB:5054 .fox-prompt-border-beam` | 品牌视觉 |
| R7 | HTML 文件预览 | 右侧面板 → 文件 → 打开 HTML 文件 | `chat/html-file-preview.tsx:5`（在 `chat/right-panel/fox-right-panel.tsx:182`、`:307` 使用） | `WB:11769 .fox-html-file-preview` | 调用 Tauri 原生窗口 |

## 3. 重复实现（统一后收益最大）

| 主题 | 重复次数 | 包含编号 | 统一为 | 可删 CSS |
|---|---|---|---|---|
| 卡片和条目 | 约 12 套 | P2、P3、P4、P5、P6、P7、P8 | Item / Card | 约 1,300 行 |
| 空状态、加载、错误 | 约 22 种 | P21、P22、P23、P9、R8、S15 | Empty / Spinner / Alert | 约 340 行 |
| 表单字段、设置行 | 约 65 处 | P13、P14 | Field | 约 260 行 |
| 进度条和任务 | 7 处 | P25、P26 | Progress / Job progress | 约 270 行 |
| 文件树和文件图标 | 2 套树、2 套图标 | P31、P32 | AI Elements File Tree | 约 270 行 |
| 分段控件 | 7 处 | P10、P11 | Toggle Group | 约 240 行 |
| 搜索框 | 9 种 | P12、S7 | Input Group | 约 180 行 |
| 单选和头像选择 | 8 处 | P16、P17、P18、S9、T5、DL4 | Radio Group / Toggle Group | 约 380 行 |
| 状态点和状态标签 | 15 种以上 | P24、S10 | 一个 StatusBadge 加点阵图标 | 约 140 行 |
| 页面头部 | 4 套 | P1 | 修好的 WorkspacePage | 约 120 行 |
| toast | 2 套 | S1 | Sonner | 约 100 行 |
| 热力图 | 2 处逐行复制 | P28 | Heat graph | 约 80 行 |
| 原生确认框 | 多处 | P41、DL2、DL3 | Alert Dialog | 无 |

全部替换完，三个页面 CSS 文件加 `workbench.css`（合计约 2.4 万行）预计能删约 11,000 行；另有约 4,700 行现在就是死代码。两者都是按类名前缀估算的上限。

## 4. 顺带发现的问题

这些不只是样式问题，建议单独处理：

1. **插件不能启用或停用**：`InstalledPluginCard` 收到了 `onToggle`、`onConfigure`、`busy`，但没有用上，界面上没有开关。位置：`PC:131`，三个参数在 `PC:142`-`:144` 被 `void` 掉。
2. **@ 提及菜单用的是写死的假文件路径**，是半成品功能。位置：`chat/composer/fox-composer.tsx:576`。
3. **输入框没有模型选择**：composer 拿到了模型数据，但界面上没有入口。位置：`chat/composer/fox-composer.tsx:122`（参数）、`:152`（`modelOptions`）、`:160`（模型状态），界面部分 `:690`-`:730` 没有模型控件。
4. **目标进度、任务条目直接写死了蓝、黄、绿色**，不跟随主题。位置：`chat/components/GoalProgress.tsx:82`、`:110`、`:117`；`chat/components/TaskItem.tsx:29`-`:33`、`:71`-`:73`、`:97`；`chat/components/EvidenceList.tsx:14`-`:20`、`:100`。
5. **`WorkspacePage` 不显示标题**：接收了 `title`、`subtitle`、`onBack` 但没有渲染。位置：`features/workspace/page-shell.tsx:3`-`:8`。
6. **右侧面板导入了 `Workbench`**，形成循环引用，而且看起来没用到。位置：`chat/right-panel/fox-right-panel.tsx:22`。

## 5. 安装清单

```
# shadcn（第 1、2 档需要）
npx shadcn add item empty field label table checkbox toggle-group radio-group slider breadcrumb attachment
# 按需
npx shadcn add chart menubar
npx shadcn add @assistant-ui/heat-graph @assistant-ui/elements-job-progress @assistant-ui/elements-timeline @assistant-ui/elements-retrieval-chunks @assistant-ui/elements-spec-sheet @assistant-ui/elements-mcp-server-panel
```

装之前用 `npx shadcn view <名字>` 看会带进哪些依赖。每装一批都要跑类型检查、构建（注意首屏 JS 预算）和桌面测试。

## 6. 建议的执行顺序

1. **第 0 档清理**：删死 CSS 和死代码，逐个前缀复核。和第 5 步不冲突的文件可以马上做。
2. **装第一批 shadcn 组件**，做第 1 档（确认框、加载、错误、进度、折叠、单选、设置行等）。改动小，每项都能单独验证。
3. **第 2 档按重复簇做**：卡片和条目 → 空状态 → 分段控件和搜索框 → 表单字段 → 状态标签 → 通知。每一簇改完截图对比。
4. **聊天区的项**交给第 5 步，迁移时直接用第 2 节“随第 5 步”表里的组件。
5. **侧边栏（第 3 档）**放在第 5 步完成之后，因为它和聊天区布局相关。

第 0–2 档主要改 `features/` 下的页面和三个页面 CSS，和第 5 步改的 composer、timeline、thread 不重叠，可以并行。但第 6 步的页面重设计也在改这些页面，两边要错开文件，或者合并给同一个 agent 做。

## 附录 A：分散实例完整位置

第 2 节里实例超过 6 处的行，完整位置列在这里。缩写见第 0 节“代码位置说明”。

### A.1 原生确认框（14 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P41 | 设置 → 模型供应商 → 删除供应商 | `SP:557` `window.confirm` | 原生对话框，没有样式 |
| P41 | 设置 → 模型供应商 → 重命名供应商 | `SP:561` `window.prompt` | 同上 |
| P41 | 连接器页 → 删除连接器 | `SP:944` | 同上 |
| P41 | 连接器页 → 删除 Hook | `SP:948` | 同上 |
| P41 | 设置 → 数据与诊断 → 预览缓存 → 清理 | `SP:1087` | 同上 |
| P41 | 设置 → 数据与诊断 → 恢复备份 | `SP:1089` | 同上 |
| P41 | 设置 → 数据与诊断 → 清理孤立文件 | `SP:1090` | 同上 |
| P41 | 编辑本地知识库，有未保存修改时关闭对话框 | `LK:667` | 同上 |
| P41 | 专家详情 → 删除专家 | `AP:647`（`AP:482` useAgentEditor 内） | 同上 |
| P41 | 专家详情 → 回滚到历史版本 | `AP:684` | 同上 |
| P41 | 专家列表 → 数字同事 → 撤销数字同事 | `features/agents/digital-colleague-manager.tsx:244` | 同上 |
| P41 | 输入框上方托管文件面板 → 强制恢复文件版本 | `chat/components/ManagedFilesPanel.tsx:148` | 同上 |
| DL2 | 左侧边栏 → 对话“…” → 回收站 → 永久删除 | `chat/conversation-dialogs.tsx:163`（ConversationManagementDialogs `:152`） | 普通 Dialog 默认样式 |
| DL3 | 左侧边栏 → 项目“…” → 删除项目 | `chat/conversation-dialogs.tsx:173` ProjectDeleteDialog | 普通 Dialog 默认样式 |

### A.2 加载状态（20 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P22 | 本地知识库各页 | `LK:433` LocalKnowledgeLoading（`LK:434`） | `LKC:325 .fox-local-kb-loading` |
| P22 | 知识 → 最近 | `LK:1416` | `LKC:2744 .fox-local-kb-recent-loading` |
| P22 | 助手 → 插件 | `PC:341`、`PC:371` | `PCC:591 .fox-plugin-loading` |
| P22 | 专家列表 → 数字同事 → 运行详情 | `features/agents/digital-colleague-manager.tsx:261` | `WP:5819 .fox-colleague-run-state` |
| P22 | 知识 → 本地文件 → 预览文件 | `LK:880`、`LK:881` | `WP:745 .fox-file-preview-state` |
| P22 | 本地知识库详情 → 文档 → 打开文档 | `LK:1013`、`LK:1014` | `WP:745 .fox-file-preview-state` |
| P22 | 远程知识库 → 文件预览 | `features/knowledge/knowledge-file-viewer.tsx:213`、`:347` PreviewModuleLoading | `WP:745 .fox-file-preview-state` |
| P22 | 远程知识库详情 → 左侧边栏文件区 | `KRE:93` | `WB:10892 .fox-knowledge-sidebar-status` |
| P22 | 远程知识库 → 知识图谱 | `KP:247`（加载和空状态共用一处） | `WP:36 .fox-page-empty` |
| P22 | 知识 → 我的知识库 → 展开知识库 | `LKS:188` | 没有专用类名 |
| P22 | 设置 → 模型供应商 | `SP:575` | 用 `.fox-settings-section` 卡片样式 |
| P22 | 设置 → 使用统计 | `SP:797` | `WP:3151 .fox-usage-loading` |
| P22 | 设置 → 数据与诊断（第 6 步已改动） | `SP:1086` | `SPC:572 .fox-diagnostic-loading`（`WP:5701` 还有一条） |
| P22 | 专家详情 | `AP:1290` | `WP:6387 .fox-agent-manage-loading` |
| P22 | 助手 → 专家列表 | `AP:1193`（加载时也用空状态的类） | `WP:5285 .fox-agent-list-empty` |
| P22 | 专家详情 → 记忆 | `AP:231` | `WP:4060 .fox-agent-memory-empty` |
| P22 | 右侧面板 → 文件 | `chat/right-panel/fox-right-panel.tsx:689` | `WB:7099 .fox-panel-loading` |
| P22 | 全局搜索 | `chat/shell/fox-sidebar.tsx:663` | `WB:1684 .fox-global-search-state` |
| P22 | 通知中心 | `chat/shell/fox-notification-center.tsx:350` | `WB:631 .fox-notification-state` |
| S15 | 应用启动、切换页面 | `chat/workbench.tsx:1159`、`:1181`；`features/workspace/workspace-shell.tsx:50` | `WB:5060 .fox-app-loading`、`WB:5059 .fox-route-loading` |

### A.3 错误提示、连接测试结果（9 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P23 | 设置各页读取失败 | `SP:96` SettingsLoadError | Tailwind，没有独立 CSS |
| P23 | 本地知识库各页 | `LK:437` LocalKnowledgeError | `LKC:326 .fox-local-kb-error` |
| P23 | 助手 → 插件 | `PC:338`、`PC:367` | `PCC:390 .fox-plugin-error` |
| P23 | 知识 → 本地文件 → 预览失败 | `LK:882` | `WP:745 .fox-file-preview-state` |
| P23 | 本地知识库详情 → 文档读取失败 | `LK:1015` | `WP:745 .fox-file-preview-state` |
| P23 | 远程知识库 → 文件预览失败 | `features/knowledge/knowledge-file-viewer.tsx:217` | `WP:745 .fox-file-preview-state` |
| P9 | 设置 → 知识库服务 → 服务卡片 → 服务连接页 → 测试连接 | `SP:1139`（ServicePage `SP:1104`） | `WP:27 .fox-connection-result` |
| P9 | 模型服务页 → 测试连接（没找到进入入口） | `SP:1185`（ModelServicePage `SP:1144`） | `WP:27 .fox-connection-result` |

### A.4 进度条（7 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P25 | 本地知识库 → 存储位置对话框 → 迁移进度 | `LK:401`（StorageLocationDialog `LK:290`） | `LKC:1627 .fox-local-kb-storage-progress`、`LKC:1644 .fox-local-kb-progress` |
| P25 | 本地知识库详情顶部任务条 | `LK:1839`（任务条 `LK:1836`） | `LKC:1644 .fox-local-kb-progress` |
| P25 | 知识 → 任务中心；本地知识库详情 → 任务 | `LK:2422` | `LKC:4734 .fox-local-kb-job-row-progress`、`LKC:1644` |
| P25 | 知识 → 向量模型 → 下载进度 | `LK:2570` | `LKC:1644 .fox-local-kb-progress` |
| P25 | 设置 → 数据与诊断 → 预览缓存 | `SP:1087` | `WP:2882 .fox-preview-cache-meter` |
| P25、P30 | 设置 → 使用统计 → 专家使用 | `SP:820` | `WP:3154 .fox-usage-agents`、`.fox-usage-rank` |
| P25、P30 | 设置 → 使用统计 → Token 构成 | `SP:821` | `WP:3156 .fox-token-summary` |

### A.5 手写展开收起（10 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P39 | 新建 / 编辑本地知识库 → 高级选项 | `LK:572` | `LKC:4848 .fox-local-kb-advanced-toggle` |
| P39 | 设置 → 数据与诊断 → 统一诊断 → 正常项（第 6 步已改动） | `SP:1086` | `SPC:580 .fox-diagnostic-ok-toggle` |
| P39 | 本地知识库详情 → 右侧检查器 → 存储（第 6 步已改动） | `LK:1536` | `LKC:4466 .fox-local-kb-inspector-toggle` |
| P39 | 任务中心 → 按天分组 | `LK:2380` | `LKC:1212 .fox-local-kb-job-day-toggle` |
| P39 | 任务中心 → 展开单个任务 | `LK:2403` JobTimelineItem（`expanded` 参数） | `LKC:1185 .fox-local-kb-job-timeline` |
| P39 | 连接器页 → 展开连接器 | `SP:944` | `WP:2713 .fox-mcp-card` |
| R5 | 右侧面板 → 运行详情 | `chat/components/ActivityDetailPanel.tsx:53`、`:112`（`<details>`） | `WB:6837 .fox-activity-detail` |
| DL4 | 运行中断后的恢复对账卡片 | `chat/kernel-reconciliation-panel.tsx:54`、`:65`、`:74`（`<details>`，在 `chat/workbench.tsx:932` 使用） | `chat/kernel-reconciliation.css:1 .fox-reconciliation` |
| C8 | 运行审批卡片（随第 5 步） | `chat/components/RuntimeApprovalPrompt.tsx:83`（`<details>`） | `WB:5981 .fox-approval-disclosure` |
| DL5 | 界面上看不到（组件未使用） | `chat/components/RunReliabilityPanel.tsx:64`（`<details>`） | Tailwind |

### A.6 设置行（约 25 处调用）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P13 | 组件定义 | `SP:88` PreferenceRow、`SP:92` SettingValue | `SPC:95 .fox-setting-row`、`SPC:111 .fox-setting-value`（`WP:26` 还有一条合并规则） |
| P13 | 设置 → 应用 → 通知 | `SP:167`、`SP:168`、`SP:169`、`SP:170` | 同上 |
| P13 | 设置 → 应用 → 外观、首次设置向导 | `SP:201`、`SP:203` | 同上 |
| P13 | 设置 → 助手设置 | `SP:391` | 同上 |
| P13 | 设置 → 模型供应商 → 编辑供应商 → 设为默认 | `SP:528` `.fox-provider-default-toggle` | `WP:3062 .fox-provider-default-toggle` |
| P13 | 设置 → 项目与权限 | `SP:728`、`SP:731` | 同上 |
| P13 | 设置 → 输入与对话 | `SP:737` | 同上 |
| P13 | 设置 → 关于 Fox | `SP:847` | 同上 |
| P13 | 设置 → 数据与诊断 | `SP:1087` | 同上 |
| P13 | 连接器页 → 新建 / 编辑连接器对话框 | `SP:951`（直接写 `fox-setting-row`） | `SPC:95 .fox-setting-row` |
| P13 | 模型服务页 | `SP:1185`（直接写 `fox-setting-row`） | `SPC:95 .fox-setting-row` |

### A.7 服务商行、服务卡片、插件卡片、能力卡片、记忆卡片、项目卡片（9 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P8（第 6 步已改动） | 设置 → 模型供应商 → 已配置的供应商 | `SP:584`（状态点 `SP:588`） | `SPC:601 .fox-provider-row`、`SPC:644 .fox-provider-row-status` |
| P8（第 6 步已改动） | 设置 → 模型供应商 → 可添加的预设 | `SP:600` | `SPC:601 .fox-provider-row` |
| P8 | 设置 → 知识库服务 → 服务卡片 | `SP:624` | `SPC:246 .fox-service-card`（`WP:26` 还有一条） |
| P8 | 设置 → 扩展 → 服务卡片 | `SP:843` | `SPC:246 .fox-service-card` |
| P3 | 助手 → 插件 → 已安装插件 | `PC:149`（InstalledPluginCard `PC:131`） | `PCC:333 .fox-plugin-installed-card`、`PCC:207 .fox-plugin-card`、`PCC:802 .fox-library-card` |
| P3 | 界面上看不到（目录分支） | `PC:115`（CatalogPluginCard `PC:105`） | `PCC:207 .fox-plugin-card` |
| P4 | 专家详情 → 知识库、技能、连接器、工具 | `AP:1018`（AgentCapabilitySection `AP:942`） | `WP:6783 .fox-cap-card` |
| P5 | 专家详情 → 记忆 | `AP:231`（AgentMemoryManager `AP:78`） | `WP:4221 .fox-memory-card` |
| P6 | 设置 → 项目与权限 | `SP:700` renderProject | `WP:5416 .fox-managed-project` |

### A.8 空状态（约 20 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P21 | 助手 → 远程知识库，未配置知识库服务 | `KP:78` | `WP:88 .fox-page-empty-state` |
| P21 | 助手 → 远程知识库，搜索无结果 | `KP:81` | `WP:36 .fox-page-empty` |
| P21 | 远程知识库 → 文件与详情，未选文件 | `KP:202` | `WP:1900 .fox-document-placeholder` |
| P21 | 远程知识库 → 知识图谱为空 | `KP:247` | `WP:36 .fox-page-empty` |
| P21 | 助手 → 专家列表为空 | `AP:1193` | `WP:5285 .fox-agent-list-empty` |
| P21 | 专家详情 → 记忆（非桌面环境、读取中） | `AP:223`、`AP:231` | `WP:4060 .fox-agent-memory-empty` |
| P21 | 专家详情 → 能力分区为空 | `AP:1016` | `.fox-capability-empty` 没有对应 CSS |
| P21 | 助手 → 插件为空 | `PC:193`（EmptyState `PC:192`） | `PCC:592 .fox-plugin-empty` |
| P21 | 本地知识库各页 | `LK:441` LocalKnowledgeEmpty（`LK:442`） | `LKC:327 .fox-local-kb-empty` |
| P21 | 技能页为空 | `SP:856` | `WP:2618 .fox-skill-empty` |
| P21 | 连接器页为空 | `SP:943` | `WP:2618 .fox-skill-empty` |
| P21 | 设置 → 项目与权限 | `SP:729` | `WP:5444 .fox-managed-project-state` |
| P21 | 设置 → 使用统计 | `SP:820`、`SP:827`、`SP:833` | `WP:3011 .fox-settings-empty-copy` |
| P21 | 左侧边栏 → 项目下没有对话 | `chat/shell/fox-sidebar.tsx:430` | `WP:124 .fox-project-empty` |
| P21 | 左侧边栏 → 对话、已归档、回收站为空 | `chat/shell/fox-sidebar.tsx:497` | `WP:124 .fox-project-empty` |
| P21 | 知识 → 我的知识库为空 | `LKS:195` | `WP:124 .fox-project-empty` |
| P21 | 选择专家对话框无结果 | `features/agents/ExpertPickerDialog.tsx:43` | `WB:3067 .fox-expert-picker-empty` |
| P21 | 选择知识库对话框无结果 | `chat/conversation-dialogs.tsx:140` | `WB:5743 .fox-knowledge-picker-empty` |

### A.9 分段控件（7 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P10（第 6 步已改动） | 本地知识库详情 → 文档 / 检索测试 / 任务 | `LK:1810`（TabsList `LK:1811`） | `LKC:550 .fox-local-kb-detail-switcher` |
| P10 | 知识 → 任务中心 → 状态筛选 | `LK:2348` | `LKC:558 .fox-local-kb-job-filter-tabs` |
| P10 | 知识 → 本地文件 → 文件类型 | `LK:1325`（TabsList `LK:1326`） | `LKC:559 .fox-local-files-categories` |
| P10 | 本地知识库详情 → 检索测试 → 检索方式 | `LK:2783` | `LKC:4590 .fox-local-retrieval-mode-tabs` |
| P10 | 助手 → 插件 → 插件类型 | `PC:335`、`PC:363` | `PCC:57 .fox-plugin-kind-tabs` |
| P10 | 助手 → 专家 → 新建专家 → 基本设置 / 能力配置 | `AP:1200` | `WP:4528 .fox-agent-editor-tabs` |
| P11 | 界面上看不到（死分支） | `PC:382` | `PCC:162 .fox-plugin-categories` |

### A.10 搜索框（17 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P12 | 助手 → 专家列表 | `AP:1180` | `WP:5262 .fox-list-page-search` |
| P12 | 助手 → 远程知识库列表 | `KP:69` | `WP:5262 .fox-list-page-search` |
| P12 | 知识 → 本地文件 | `LK:1316` | `LKC:96 .fox-local-kb-search` |
| P12 | 知识 → 本地知识库 | `LK:1477` | `LKC:96 .fox-local-kb-search` |
| P12 | 助手 → 插件 | `PC:332`、`PC:357` | `PCC:51 .fox-plugin-search` |
| P12 | 专家详情 → 能力分区 | `AP:1010` | `WP:6745 .fox-capability-search` |
| P12 | 专家详情 → 记忆 | `AP:226` | `WP:4220 .fox-memory-toolbar` |
| P12 | 选择专家对话框 | `features/agents/ExpertPickerDialog.tsx:36` | `WB:3048 .fox-expert-picker-search` |
| P12 | 远程知识库 → 知识图谱 | `KP:234`、`KP:240` | `WP:2501 .fox-graph-toolbar` |
| P12 | 本地知识库详情 → 检索测试 | `LK:2782` | `LKC:4574 .fox-local-retrieval-search` |
| P12 | 远程知识库 → 文本和 PDF 预览内搜索 | `features/knowledge/knowledge-file-viewer.tsx:299`、`features/knowledge/knowledge-pdf-viewer.tsx:300` | `WP:1330 .fox-preview-search` |
| P12 | 选择知识库对话框 | `chat/conversation-dialogs.tsx:124` | `WB:5724 .fox-knowledge-picker-search` |
| P12 | 右侧面板 → 文件 | `chat/right-panel/fox-right-panel.tsx:687` | `WB:7160 .fox-panel-search` |
| S7 | 全局搜索输入框 | `chat/shell/fox-sidebar.tsx:657` | `WB:1577 .fox-global-search-input-shell` |

### A.11 表单字段（7 组，约 40 个字段）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P14 | 知识 → 本地知识库 → 新建知识库 | `LK:449` CreateKnowledgeBaseDialog（表单 `LK:559`） | `LKC:17 .fox-local-kb-create-dialog`、`LKC:1677 .fox-local-kb-create-form` |
| P14 | 本地知识库 → 编辑知识库 | `LK:593` EditKnowledgeBaseDialog（表单 `LK:678`） | 同上 |
| P14 | 设置 → 模型供应商 → 编辑供应商 | `SP:526` ProviderEditor（表单 `SP:528`） | `WP:3060 .fox-provider-editor`、`WP:3065 .fox-provider-form-grid` |
| P14 | 连接器页 → 新建 / 编辑连接器 | `SP:951` | `WP:2841 .fox-mcp-form` |
| P14 | 助手 → 专家 → 新建专家 → 基本设置；专家详情 → 编辑资料 | `AP:751` AgentEditorProfileFields（`AP:754`） | `WP:4591 .fox-agent-editor-grid` |
| P14 | 设置 → 数据与诊断 → 恢复备份、清理孤立文件 | `SP:1089`、`SP:1090` | `WP:2924 .fox-maintenance-field` |
| P14 | 本地知识库详情 → 文档 → 导入 → 导入到 | `LK:2039` | `LKC:1085 .fox-local-kb-import-target` |

### A.12 状态点 / 状态标签（7 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P24（第 6 步已改动） | 设置 → 模型供应商 → 供应商行状态 | `SP:588` | `GL:315 .fox-status-dot`、`SPC:644 .fox-provider-row-status` |
| P24（第 6 步已改动） | 设置 → 数据与诊断 → 统一诊断各项 | `SP:1086` | `GL:315 .fox-status-dot`、`SPC:470 .fox-diagnostic-checks` |
| P24（第 6 步已改动） | 本地知识库详情 → 就绪状态行 | `LK:1790` | `LKC:4356 .fox-local-kb-readiness-line` |
| P24 | 知识 → 任务中心 → 任务状态 | `LK:2417` | `GL:315 .fox-status-dot`、`LKC:4720 .fox-local-kb-job-row-status` |
| P24 | 知识 → 向量模型 → 模型状态 | `LK:2572` | `LKC:4780 .fox-local-model-row-status` |
| P24 | 本地知识库详情 → 检索测试 → 提示 | `LK:2792` | `GL:315 .fox-status-dot` |
| P24 | 助手 → 插件 → 状态胶囊 | `PC:94` StatusBadge | `PCC:510 .fox-plugin-status` |

### A.13 文件列表、文档列表、模型行（8 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P33 | 知识 → 本地文件 → 文件行 | `LK:1061` LocalFileRow（原生勾选 `LK:1068`） | `LKC:2117 .fox-local-files-list` |
| P33 | 知识 → 本地文件 → 列表头 | `LK:1338` | `LKC:2117 .fox-local-files-list` |
| P33 | 知识 → 本地文件 → 来源列表 | `LK:1036` LocalFileSourceRow（`LK:1038`） | `LKC:4823 .fox-local-files-source-row` |
| P33 | 知识 → 本地文件 → 全部来源 | `LK:1054` AllLocalFileSourcesRow | `LKC:4823 .fox-local-files-source-row` |
| P34 | 本地知识库详情 → 文档 → 浏览列表 | `LK:2023` | `LKC:2934 .fox-local-kb-browser-list` |
| P34 | 知识 → 最近 | `LK:1411` | `LKC:2662 .fox-local-kb-recent-list` |
| P35 | 知识 → 向量模型 → 模型文件行 | `LK:2566` | `LKC:4768 .fox-local-model-row` |
| P35 | 本地知识库详情 → 检索测试 → 三模式对比 | `LK:2804` | `LKC:4637 .fox-local-retrieval-comparison-row` |

### A.14 统计数字块（8 处）

| 编号 | 在哪看到 | 代码位置 | 样式位置 |
|---|---|---|---|
| P29 | 专家详情 → 工作记录 | `AP:402` | `WP:3153 .fox-agent-work-metrics` |
| P29 | 设置 → 使用统计 | `SP:801` | `WP:3153 .fox-agent-work-metrics` |
| P29 | 设置 → 数据与诊断（第 6 步已改动） | `SP:1087` | `SPC:512 .fox-maintenance-stats`（`WP:2875` 还有一份） |
| P29 | 设置 → 项目与权限 → 项目卡片 | `SP:707` | `WP:5481 .fox-managed-project-metrics` |
| P29 | 远程知识库 → 知识图谱 → 右侧概览 | `KP:252` | `WP:2142 .fox-graph-overview` |
| P29 | 远程知识库 → 文件与详情 → 文件信息 | `KP:194` | `WP:750 .fox-document-meta-strip` |
| P29 | 专家详情对话框 | `AP:812`（AgentDetailDialog `AP:781`） | `WP:6140 .fox-agent-view-meta` |
| P29 | 专家列表 → 数字同事 → 运行详情 | `features/agents/digital-colleague-manager.tsx:259` | `WP:5759 .fox-colleague-run-facts` |
