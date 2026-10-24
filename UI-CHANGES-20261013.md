> 历史文档，内容截至 2026-10-14，现状见 [docs/02-架构/架构文档导航.md](docs/02-架构/架构文档导航.md) 与当前代码（本文不再维护）。

# Fox UI 组件改造与最终验收 · 2026-10-13

本报告覆盖本次用户任务的组件改造、四类交互问题及最终视觉回归，包括 onboarding、聊天顶栏与右栏、快捷键、侧栏、设置、本机资料库、通知和专家挂卡。页面结构、数据接口和既有业务回调沿用 Fox 实现；官方组件经本地适配接入，不将独立演示页当作整站验收。

唯一源码为 `D:\python\projects\Fox\worktrees\ui-improvements-20261009`。本次最后修正只编辑 `shortcut-recorder.tsx`、其 CSS Module 和 `fox-chat-topbar.tsx`；未编辑 `workbench.tsx`、依赖目录或预算脚本，未提交。验证副本始终从当前源码单向更新，没有从旧副本覆盖源文件。

## 用户提出的四项 Bug

| 问题 | 最终实现与验收 |
| --- | --- |
| 1. 打开右栏后聊天内容挤入左侧历史导航 | 右栏宽度以内容卡为界，宽屏限制占比，compact 抽屉定位在内容卡内部。1180px 断点同步开启状态；打开和关闭跨宽窄尺寸、1180/1181px 边界、窄屏关闭后回宽屏、历史 A/B 真实点击均通过。 |
| 2. 模型消息操作图标间距与复制动画 | 活跃消息使用官方 assistant-ui `elements-message-actions` 组件；统一 28px 按钮、14px 图标、4px 间距，保留操作回调与复制成功后的 Copy/Check 动画。用户消息复制同步加入确认动画。复制成功、失败和动画复位已验证。 |
| 3. 摘要与右栏开关选中效果不一致 | 两个开关均接 Animate UI Radix Toggle，pressed 状态对应真实打开状态，共用选中背景与尺寸。 |
| 4. 摘要点击后保持悬浮，再次点击才关闭 | 非模态受控 Popover 仅摘要 toggle 关闭；外部点击、Escape、历史切换与摘要条目操作不关闭摘要。virtual anchor 每次定位读取可见聊天区域右上坐标；右栏最大化隐藏聊天区域时改读内容区，避免锚点归零。1280/1440/1080px 普通/右栏/最大化、跨尺寸均验证坐标。最大化时浮层内保留同图标 toggle，可点击或 Enter 关闭。 |

验收中还修复了两项问题：新建快捷键进入与新建按钮相同的路径并清空旧草稿，录制器键帽保持正常文档流，避免动画副本重叠；onboarding 在 <=980px 的最后一步恢复可操作单列卡片，有模型可以完成，无模型可以跳过。录制器即时帧及动画结束、blur、取消均已检查，onboarding 在 1440/981/980/760/390px × 有/无模型的十组三步流程均通过。

## 17 项组件覆盖与真实来源

来源以下列源码头部的 registry URL 为准；Fox 的尺寸、主题变量、受控状态、业务动作和异步加载属于本地适配。动态图标作为一类计数，内部 Accordion 等依赖不额外计入 17 项。

| # | 组件 | 官方来源与 Fox 接入 |
| --- | --- | --- |
| 1 | Stepper | Arc [stepper](https://uiarc.dev/r/stepper.json)；onboarding 三步、已访问步骤返回、模型条件与跳过。 |
| 2 | ShortcutRecorder | Arc [shortcut-recorder](https://uiarc.dev/r/shortcut-recorder.json)；设置录制、冲突确认、立即持久化，复用 Fox shortcut-model；最终键帽稳定性修正见上表。 |
| 3 | ElasticSlider | Arc [elastic-slider](https://uiarc.dev/r/elastic-slider.json)；受控文字大小、百分比、100% 标记、1/5 步长，保留数字输入与 ±。 |
| 4 | ExpandingSearch | Arc [expanding-search](https://uiarc.dev/r/expanding-search.json)；本机资料工具栏 portal、受控查询、真实提交、关闭结果与动态快捷键。 |
| 5 | FileDropzone | Arc [file-dropzone](https://uiarc.dev/r/file-dropzone.json)；真实添加资料 Dialog，文件/文件夹选择、保留目录结构、浏览器/原生拖放反馈、移除、禁用，256 MB/5000 文件限制。 |
| 6 | Tabs | Arc [tabs](https://uiarc.dev/r/tabs.json)；明确接入侧栏助手/资料切换，加载前同样式 Radix 回退。业务页仍有原 `ui/tabs`，不宣称所有 Tabs 都已替换。 |
| 7 | UserMenu | Arc [user-menu](https://uiarc.dev/r/user-menu.json)；侧栏头像、姓名、连接状态、资料/设置动作、受控 open 与主题回调。 |
| 8 | Skeleton | Arc [skeleton](https://uiarc.dev/r/skeleton.json)；保留 label、行数、头像和 busy，纯占位即时显示，reveal 按需加载。 |
| 9 | Empty States | Arc [empty-states](https://uiarc.dev/r/empty-states.json)；empty/search/error 与业务 action，动画到达前静态插画回退。 |
| 10 | ToastStack | Arc [toast-stack](https://uiarc.dev/r/toast-stack.json)；通知映射、队列 12/可见 3、普通 5 秒/失败审批 8 秒、“查看”定位通知。 |
| 11 | Highlight | Animate UI [primitives-effects-highlight](https://animate-ui.com/r/primitives-effects-highlight.json)；侧栏活动行测量代理，保留导航 DOM，代理不接收指针，减少动画时过渡为零。 |
| 12 | 动态图标 | Animate UI [icons-icon](https://animate-ui.com/r/icons-icon.json) 及源码列出的 11 个图标 registry；整个按钮 hover/tap 驱动、无循环，减少动画时关闭动效。 |
| 13 | Toggle | Animate UI [components-radix-toggle](https://animate-ui.com/r/components-radix-toggle.json) / [primitives-radix-toggle](https://animate-ui.com/r/primitives-radix-toggle.json)；聊天摘要和右栏 pressed API、真实按钮属性、Tooltip 接入。 |
| 14 | Switch | Animate UI [components-base-switch](https://animate-ui.com/r/components-base-switch.json) / [primitives-base-switch](https://animate-ui.com/r/primitives-base-switch.json)；checked 由 owner 决定，取消/失败不擅自切换显示，value/form 接隐藏输入。 |
| 15 | Progress | Animate UI [components-base-progress](https://animate-ui.com/r/components-base-progress.json) / [primitives-base-progress](https://animate-ui.com/r/primitives-base-progress.json)；单元素 API、规范化 max/value、null 与 asChild，通知进度行复用。 |
| 16 | Files | Animate UI [components-base-files](https://animate-ui.com/r/components-base-files.json) / [primitives-base-files](https://animate-ui.com/r/primitives-base-files.json)；兄弟目录独立展开、选择路径补齐祖先、异步目录及保留手动展开，回传原路径。 |
| 17 | Lanyard | React Bits [Lanyard](https://reactbits.dev/components/lanyard)，[固定上游版本](https://github.com/DavidHDev/react-bits/blob/3329f3bde763a37a2a89b24598e9f50fa0d4de3d/src/ts-default/Components/Lanyard/Lanyard.tsx) 与许可记录于 `expert-lanyard-scene.ts`；专家头像、双带、默认正面、click/Enter/Space 翻面、Three 按需加载与 DOM 回退。 |

动态图标 registry 位于 `https://animate-ui.com/r/`：`icons-message-circle-plus.json`、`icons-users.json`、`icons-blocks.json`、`icons-panel-left.json`、`icons-panel-right.json`、`icons-settings.json`、`icons-sun-moon.json`、`icons-maximize.json`、`icons-minimize.json`、`icons-search.json`、`icons-bell.json`。对应源码在 `apps/desktop/src/components/animate-ui/icons/`；Arc 源码在 `components/arc/`，Animate UI 源码在 `components/animate-ui/`，Fox 封装在 `components/fox-ui/`。

## 最终正式检查

复用 `/tmp/fox-integration-validation-20261013` 依赖及 `/tmp/fox-integration-tools-20261013` 工具。最新源单向同步到 integration 和 latest-browser 两个副本，最终 **541 个文件 SHA-256 一致、0 差异**。此前 537 文件口径现扩展了根 package/lock/workspace/bun 配置四项；没有向正式测试目录加入临时浏览器测试。详细路径/哈希见 `final-visual-sync-manifest.json`。

| 检查 | 本次最终结果 |
| --- | --- |
| `tsc --noEmit` | 通过 |
| `tsc -p tsconfig.node.json --noEmit` | 通过 |
| `bun test ./apps/desktop/tests` | **636 pass / 2 skip / 0 fail**；638 tests、92 files、2465 expect calls |
| `node --test tests/project-file-tree-dom.test.mjs` | **3 pass / 0 fail**，独立计数；真实 React/Base/Motion 树 |
| `pnpm build` | 通过；本环境 `--max-old-space-size=4096` 在 rendering chunks 被系统终止，最终使用 **2560 MiB** 完成原构建流程，未修改 Vite 配置 |
| `pnpm check:bundle` | 通过；entry **27.7 KiB gzip**，initial JS **87.0 KiB gzip**，1 preload；门槛仍 **240/316 KiB** |
| `pnpm check:isolation --mode=release` | 通过；release 无 telemetry、synthetic harness、real-App controller、foxPerf 入口及 profiling renderer |

## lazy 预算的统计范围

87.0 KiB 是 `dist/index.html` 引用的 entry 与 modulepreload gzip 合计，脚本不递归统计动态依赖，也不计 CSS、字体或图片，**不能代表完整工作台重量**。`main.tsx` 静态导入轻量 `app/App.tsx`；当前没有独立 `App-*.js` chunk，App 包含于 HTML entry。App 的 `React.lazy` 加载的是 Workbench，首次渲染就发起请求，不能解释成等用户点击才下载。

本次生产构建的 `workbench-OLqOP-bI.js` 为 **702.80 kB raw / 210.68 kB gzip**（Vite 十进制显示），独立于 entry/preload 的 87.0 KiB 统计；它还会加载静态依赖与首屏 mount 时触发的 deferred module。大 viewer、图表、Mermaid 与 Three 另有 lazy chunk。87.0 KiB 加一个 Workbench chunk 也不是完整首次可交互网络总量；本次没有宣称已测量这一总量。预算脚本、门槛和 manualChunks 均未改动，Vite 的大 chunk 提示保留。

## Chromium 最终视觉与交互证据

Chromium **156.0.8078.4**，真实 Workbench/当前组件及完整样式；Tauri API 由 mockIPC 提供。

- `final-visual-workbench-browser-evidence.json`：摘要在 1280/1440/1080px 的普通/右栏/最大化坐标、打开后跨尺寸和 toggle 实际命中；键帽录制、即时提交、动画结束、blur 与取消的完整矩形快照。所有键帽无重复/重叠，摘要位于内容区右上且不覆盖左 sidebar。四动作改绑、旧 Ctrl+N/O/,/B 失效、新建清草稿、picker 一次调用、设置/侧栏动作、聚焦录制器/冲突/IME/229/Process/repeat 抑制均通过，未捕获 pageerror。
- `final-visual-upload-browser-evidence.json`：从**当前** `local-knowledge-browser-check.mjs` 的入口构造资料库页，导入与主入口一致的 `globals.css`、`workbench.css`、`workspace-pages.css` 及组件 CSS Module；真实 Radix Dialog 和 FileDropzone，不复用早期缺 CSS standalone。Host picker mock 增加模拟 `sourcePath`，与当前路径校验契约一致，不代表原生 picker 验收。1280/1440/1080px × 明/暗六组通过：空列表禁用开始、选择文件/文件夹三行后启用、文件名在 metadata 上方且不与移除按钮重叠、CSS grid 已加载、Dialog 在 viewport 内、移除和取消。未修改上传产品源码。
- 前序完整验收证据保留：onboarding 十组有/无模型三步与跳过；受控 Switch 延迟保存失败/重试时 aria、data-state、输入值、context、图标 opacity 一致；Files 嵌套折叠卸载与重开状态；挂卡 Three/DOM、默认正面、click/Enter/Space、专家切换、路由解绑清理、闲置无 RAF、减少动画与 WebGL context loss。

交付截图以 **`final-visual-`** 为最新前缀，onboarding 沿用已通过的 `final-onboarding-`。旧 `final-shortcuts-rebound-settings.png`、`final-summary-maximized-wide-fixed.png` 等曾含视觉缺陷，已由本次新截图取代；不得作为最终可交付证据。失败调试截图归入 `diagnostics/`。截图、JSON、最新正式日志及复现脚本同时保存在 session outputs 和 `output/ui-audit/20261013-components/`，其中最新日志位于 `logs/final-visual/`，脚本位于 `scripts/final-visual/`。

## 验证边界

未覆盖 Windows 真机、Tauri WebView、系统快捷键、真实文件夹 picker 或实际模型/资料库后端联调。工作台 `yuxi_agents_sync`、`app_notification_publish` 使用 mock 响应；资料库页面使用当前 mock gateway。正式两项 skip 是已有 Mermaid happy-dom 用例，未改为通过。Linux 无法解析挂载 worktree 的 Windows gitdir，因此未宣称完成 Git 状态检查或提交。
