> 历史文档，内容截至 2026-10-11，现状见 [docs/02-架构/架构文档导航.md](docs/02-架构/架构文档导航.md) 与当前代码（本文不再维护）。

# 第 5 步执行计划：拆分 workbench.tsx，聊天界面迁移到 assistant-ui

给执行 agent 用。按阶段做，每个阶段单独验收，前一阶段验收通过再开始下一阶段。

## 0. 必读约束

- **工作目录**：只在 `D:\python\projects\Fox\worktrees\ui-improvements-20261009`（分支 `ui/improvements-20261009`）工作。不要进入或修改 `D:\python\projects\Fox\Fox`（main）。
- **只换界面层，不换后端**：`useDesktopConversation()`（`src/features/conversations/hooks/use-desktop-conversation.ts`）、`desktopClient`、Tauri 事件、`runtime-event-reducer.ts` 都保持不动。assistant-ui 通过 `ExternalStoreRuntime` 读它们的数据、调用它们的方法。
- **端口**：1421 是用户自己的开发服务器，不能占用、不能杀。截图/预览用独立端口（例如 Vite 1440–1449、Edge CDP 9350–9359，`--strictPort`），临时 vite 配置文件用唯一名字，结束后删除，只停自己启动的进程。
- **换行符**：仓库内容为 LF，`.gitattributes` 已把检出也固定为 LF（Windows 系统级 `core.autocrlf=true` 不再生效）。用 LF 写文件，不要把 CRLF 写回去。
- **设计规范**：只用 `UI-PLAN.md`「设计规范」表里的档位和变量（圆角、间距、字号、字重、控件高度、阴影、颜色）。按钮、输入框、菜单、对话框用 `src/components/ui` 的 shadcn 组件。新样式不再写进 `src/styles/workbench.css`，放在新组件旁边的 CSS 文件或用 Tailwind 类。
- **页面整洁规则**：遵守 `PAGE-REDESIGN.md` 第 1 节的 10 条通用规则（容器层级、主按钮数量、状态表达等）。
- **验证命令**（每个阶段结束都要跑，全部通过才算完成）：
  - `pnpm --dir apps/desktop build`（含 tsc、vite build、包体积预算、发布隔离检查）
  - `bun test ./apps/desktop/tests`（当前基线：550 通过 / 2 跳过 / 0 失败；`a0-performance.test.tsx` 偶发计时失败，单独重跑确认）
  - 截图对比：明暗两种模式，见第 8 节
- **提交**：开始前确认第 1–4 步已经提交（由用户决定）。之后每个阶段一个提交，提交信息说明阶段编号。不要 push。

## 1. 现状（2026-10-10 调查结果）

### 1.1 workbench.tsx（6167 行）结构

| 行号 | 内容 |
|---|---|
| 1–290 | imports（ai-elements 148–231；懒加载 275–290） |
| 292–463 | `MarkdownResponse` 包装、布局存储、`useStableCallback`、演示数据、`requestConversationFork` |
| 464–545 | `IconButton`、通知辅助函数 |
| 546–868 | `NotificationCenter` |
| 869–972 | `WindowTitlebar`、`WindowMenu` |
| 973–999 | `ConversationRunStatus` |
| 1000–1622 | `Sidebar`（导出） |
| 1624–1803 | 仅演示用：`AssistantProcess`、`ApprovalPrompt`、`ErrorPrompt`、`QuestionPrompt`、`LicenseInlineCitation` 等 |
| 1804–2131 | 工具/过程模型：`RuntimeToolStep`、`toolCategory`、`toolIcon`、`runtimeProcess(events)`（2047） |
| 2132–2159 | `RuntimeSources` |
| 2168–2403 | `RuntimeAssistantMessage`（过程分组、折叠、复制/分叉/反馈、反馈对话框；2375 自定义 memo 比较） |
| 2404–2764 | 过程行：`RuntimeReasoningItem`、`RuntimeToolItem`、`RuntimeProcessRow`、`RuntimeProcess` |
| 2765–2814 | `splitAssistantContent`（拆 `<think>`）、`SentMessageAttachments`、`ConversationBottomDock` |
| 2815–3085 | `RuntimeTimeline`（导出，真实消息列表 + 编辑/重跑/分叉） |
| 3086–3164 | `Timeline`（空状态 + 演示分支 3115–3161） |
| 3166–3236 | `ConversationTurnRail` |
| 3237–3316 | 输入框附件预览、`slashCommands`、`ComposerRuntimeContext` / `TimelineRuntimeContext` |
| 3375–3993 | `Composer` + `MemoizedComposer` |
| 3995–4167 | `KnowledgeBindingDialog`（导出）、会话管理对话框 |
| 4168–4952 | 右侧面板：文件树、预览、浏览器、子 agent、`RightPanel`（导出） |
| 4973–5070 | `ConversationSummaryPopover`、`ChatTopbar` |
| 5071–6167 | `Workbench` 容器：约 60 个 `useState`、各种 handler、JSX |

### 1.2 数据与动作

- 消息类型 `ConversationMessage`（`conversations/model/types.ts:375`）：`{ id, conversationId, runId, role, kind, content, status, ordinal, createdAt, updatedAt, kernelPreview? }`。**没有结构化的 parts**：思考内容在 `content` 里的 `<think>` 标签或 `reasoning.delta` 事件里；工具调用、来源、计划/目标来自 `detail.runtimeEvents`（`RunEventRecord`）和 `detail.toolCalls`；附件 `AttachmentRecord` 用 `messageId` 关联；产物 `ArtifactRecord` 用 `runId` 关联；失败用 `deriveRunTerminalStates`。
- 流式：`use-runtime-event-stream.ts` 每帧（rAF）批量合并事件 → `reduceRuntimeNotifications`；`message.delta` 追加到 `content`。
- 动作对应表：

| 功能 | 现有实现 | assistant-ui 对应 |
|---|---|---|
| 发送 | `send`（hook 890）经 `handleComposerSubmitPrompt`（workbench 6020），乐观消息 `pendingUserMessage` | `onNew` |
| 停止 | `cancel`（hook 1185） | `onCancel` |
| 重试 | `retryLatestRun`（workbench 5431） | `onReload`（只允许最后一条） |
| 编辑并重发 | 仅最后一条用户消息；`rerunFromMessage`（hook 1048） | `onEdit` + 只在最后一条用户消息上显示编辑按钮 |
| 分叉 | `requestConversationFork` → 新会话 | 自定义 ActionBar 按钮（不用 assistant-ui 分支） |
| 复制 | 自有 | `ActionBarPrimitive.Copy` |
| 反馈 | `desktopClient.saveMessageFeedback` | `adapters.feedback` |
| 加载更早消息 | `loadEarlierMessages`（hook 416） | `hasEarlier` + `onLoadEarlier` |
| 审批/提问/目标/计划/工作流闸门 | `resolveApproval`、`resumeQuestion`、`resolvePlanRevision` 等；决策卡替换输入框（workbench 3753–3792） | 保留自有组件，放在 Composer 区域条件渲染 |
| 运行中补充指令 | `useRunSteering` + `RunSteeringStrip` | 第一版保留自有实现（见 4.4） |

### 1.3 依赖与预算

- `@assistant-ui/*` 尚未安装。当前最新：`@assistant-ui/react@0.15.26`（依赖 `radix-ui`、`zustand`、`zod`、`assistant-stream` 等；项目已有 `radix-ui`、`zustand`）。项目 shadcn 风格是 `radix-mira`，所以选 **Radix 版本**的 assistant-ui 组件，不选 Base UI 版本。
- Markdown 已经是 Streamdown（`components/ai-elements/message-response.tsx`，含 cjk、math、mermaid、本地全屏表格、可折叠代码块）。第一版**继续用它**渲染文本部分，不换 `@assistant-ui/react-markdown`。
- 包体积预算（`scripts/check-bundle-budget.mjs`）：入口 ≤ 240 KiB gzip，首屏 JS ≤ 316 KiB gzip；当前首屏 304.6 KiB，只剩约 11 KiB 余量。
- 测试中依赖 workbench.tsx 的地方（拆分时必须照顾）：
  - 用 `ssrLoadModule('/src/features/chat/workbench.tsx')` 取 `RuntimeTimeline`：`turn-process-header-dom.test.mjs`、`runtime-process-row-dom.test.mjs`、`runtime-process-groups-dom.test.mjs`、`row-overflow.test.mjs`；取 `RightPanel`：`right-panel-files-dom.test.mjs`
  - `tests/fixtures/knowledge-picker.tsx` 导入 `KnowledgeBindingDialog`；`tests/browser/sidebar-status-acceptance.tsx`、`runtime-fixes-acceptance.tsx` 导入 `Sidebar`；`src/features/profiling/ProfilingHarness.tsx:19` 导入 `RuntimeTimeline`
  - `conversation-draft-selection.test.ts`（153、220 行）按源码文本切片 `beginNewDraft`…`resetConversation`、`switchAssistantMode`…`createProjectDraft`
  - `process-row-slots-css.test.ts` 读取 `src/styles/workbench.css`
  - 断言聊天标记/类名的测试：`steering-composer-dom`、`steering-composer-lanes`、`prompt-input-keyboard-dom`、`prompt-input-attachment-submit-dom`、`prompt-input-reset`、`prompt-send-shortcut`、`tool-output-performance`、`mermaid-diagram-message`、`runtime-artifacts-dom`、`runtime-approval-retry`、`managed-files-panel-dom`、`conversation-status-icon`、`conversation-completion-notices`

## 2. 总体做法

1. **先拆后换**：5A 只搬代码不改行为；5B 起才引入 assistant-ui。
2. **新旧并存，开关切换**：新界面放在 `src/features/chat/thread/`，由偏好开关 `fox.ui.chatEngine`（`legacy` | `assistant-ui`，开发期默认 `legacy`）切换。功能对齐清单（第 7 节）全部通过后再把默认值改成 `assistant-ui`，最后删除旧实现。
3. **数据只转换不复制**：`useDesktopConversation()` 仍是唯一数据源。用 `useExternalMessageConverter` 把 `ConversationDetail` 转成 `ThreadMessageLike[]`，每条消息带稳定 `id`，让未变化的消息命中缓存。
4. **过程（思考 + 工具调用）第一版用一个数据部件承载**：把现有 `runtimeProcess(events)` 和 `runtime-process-groups.ts` 算出的分组结果放进 `{ type: 'data-fox-process', data }`，用现有过程组件（按新规范改样式）渲染。原因：Fox 的过程模型（阶段、尝试状态、分组、折叠标题）比 assistant-ui 的 tool-call 部件丰富，直接拆成原生 tool-call 风险大。原生 tool-call / reasoning 部件留作后续可选项。

## 3. 阶段 5A：拆分 workbench.tsx（纯搬家，不改行为）

目标结构（都在 `src/features/chat/` 下）：

```
workbench.tsx                  只剩 Workbench 容器 + 重新导出（保留旧的导出名，测试不用改路径）
chat-contexts.ts               ComposerRuntimeContext、TimelineRuntimeContext
workbench-layout-storage.ts    layoutStorage、localStorage 读取函数
shell/
  window-titlebar.tsx          WindowTitlebar、WindowMenu
  notification-center.tsx      NotificationCenter + 通知辅助函数
  sidebar.tsx                  Sidebar、ConversationRunStatus
  chat-topbar.tsx              ChatTopbar、ConversationSummaryPopover
  conversation-dialogs.tsx     ConversationManagementDialogs、ProjectDeleteDialog、KnowledgeBindingDialog
right-panel/
  right-panel.tsx              RightPanel 及其内部组件（4168–4952）
timeline/
  runtime-process-model.ts     RuntimeToolStep、toolCategory、toolRowSlots、toolIcon、runtimeProcess
  runtime-process-view.tsx     RuntimeReasoningItem、RuntimeToolItem、RuntimeProcessRow、RuntimeProcess
  runtime-assistant-message.tsx
  runtime-sources.tsx
  runtime-timeline.tsx         RuntimeTimeline、splitAssistantContent、SentMessageAttachments
  conversation-bottom-dock.tsx
  conversation-turn-rail.tsx
  timeline.tsx                 Timeline（空状态）
composer/
  composer.tsx                 Composer、MemoizedComposer
  slash-commands.ts
  composer-attachments.tsx     ComposerAttachmentPreview、ComposerAttachmentCommandBridge
demo/
  demo-prompts.tsx             1624–1803 的演示组件 + 演示数据（无 Tauri 时的预览会用到，先保留）
```

要求：

- 只移动代码和改 import，不改逻辑、不改类名、不改 CSS。`workbench.tsx` 继续导出 `Sidebar`、`RuntimeTimeline`、`RightPanel`、`KnowledgeBindingDialog`，这样第 1.3 节列出的测试和 `ProfilingHarness` 不用改。
- `conversation-draft-selection.test.ts` 的源码切片依赖 `beginNewDraft`、`resetConversation`、`switchAssistantMode`、`createProjectDraft` 在 `workbench.tsx` 中的相对顺序。这几个函数留在 `Workbench` 里不动；如果必须移动，就同步修改测试里的切片锚点。
- 每移出一块就跑一次 `tsc`，避免最后一次性排错。
- 可以分给 2–3 个 agent 并行：A 负责 `shell/` + `right-panel/`；B 负责 `timeline/`；C 负责 `composer/` + `demo/`。共同编辑 `workbench.tsx` 时用 Edit 工具小块修改，冲突就重新读取再改。

**验收**：build 和测试全部通过；`workbench.tsx` ≤ 1300 行；聊天页明暗截图与拆分前像素级一致（允许抗锯齿差异）；React Profiler 中 `RuntimeTimeline` 的渲染次数不变（用 `dev:profile` 合成时间线检查，见第 8 节）。

## 4. 阶段 5B：接入 assistant-ui 运行时（还不换界面）

### 4.1 安装与体积探测

- `pnpm --dir apps/desktop add @assistant-ui/react`（锁定当前小版本，例如 `~0.15.26`）。
- 立刻跑 `pnpm --dir apps/desktop build` 看首屏体积。如果超出 316 KiB：
  - 首选：新聊天界面整体懒加载（`React.lazy` 加载 `thread/` 入口，`Suspense` 里放与最终布局同尺寸的骨架），保证首屏只多一个很小的壳。
  - 不要自行调高预算；确实需要调整，先把数据写进报告让用户决定。
- `vite.config.ts` 的 `manualChunks` 如有需要可以为 assistant-ui 加一个独立 vendor 块，但它不能进入预加载列表（预算脚本只允许 `react-vendor` 和 `icons-vendor` 预加载）。

### 4.2 消息转换 `src/features/chat/thread/convert-detail.ts`

纯函数，输入 `ConversationDetail` + 乐观消息 + 流式状态，输出 `FoxThreadMessage[]`（自有中间类型，带 `id`），再交给 `useExternalMessageConverter` 的 `callback` 转成 `ThreadMessageLike`：

| Fox 数据 | ThreadMessageLike |
|---|---|
| `role: 'user'` 消息 | `{ role: 'user', id, createdAt, content: [{ type: 'text', text }], attachments }`（附件由 `AttachmentRecord` 按 `messageId` 关联，转成 `CompleteAttachment`，`type` 用 `image`/`document`/`file`） |
| `role: 'assistant'` 消息 | `{ role: 'assistant', id, createdAt, status, content: [...] }` |
| `<think>` 内容 / `reasoning.delta` | 并入 `data-fox-process` 的思考项（与现有 `RuntimeAssistantMessage` 的处理一致） |
| 该 run 的过程事件 | `{ type: 'data-fox-process', id: runId, data: <runtimeProcess 分组结果> }`，放在正文之前 |
| 正文 | `{ type: 'text', text: normalizeAssistantMarkdown(...) }` |
| 来源/引用 | `{ type: 'source', sourceType: 'url', id, url, title }` |
| 产物 | `{ type: 'data-fox-artifacts', data }` |
| 失败 | `status: { type: 'incomplete', reason: 'error', error }`，另加 `{ type: 'data-fox-error', data: RunFailure }` 用于渲染现有错误卡 |
| `role: 'tool'` / `'system'` | 不单独成消息；按现有界面的处理方式并入或忽略 |
| 运行状态 | 运行中 `{ type: 'running' }`；完成 `{ type: 'complete', reason: 'stop' }`；中断 `{ type: 'incomplete', reason: 'cancelled' }` |

要求：

- `joinStrategy: 'none'`，Fox 自己决定消息边界。
- 用 `metadata` + `getMetadataKey` 传运行事件，只让最后一个运行中的 assistant 消息在流式时重新转换，其余消息保持引用不变（这是性能关键点）。
- 给 `convert-detail.ts` 写纯函数单测（`apps/desktop/tests/thread-convert-detail.test.ts`）：普通问答、带思考、带工具调用、带附件、失败、取消、流式中、加载更早消息后 id 不变。

### 4.3 运行时 `src/features/chat/thread/use-fox-runtime.ts`

```ts
const runtime = useExternalStoreRuntime({
  messages: converted,
  isRunning,                      // 由 lastRun.status 推导
  isSendDisabled,                 // 有待处理的审批/提问时为 true
  onNew,                          // → conversation.send(...)；文本 + 附件 + 斜杠命令改写 + 当前专家/知识库/项目绑定
  onCancel,                       // → conversation.cancel()
  onEdit,                         // → rerunFromMessage(parentId 对应的用户消息, 新文本)
  onReload,                       // → retryLatestRun()
  hasEarlier, onLoadEarlier,      // → detail.hasEarlierMessages / loadEarlierMessages()
  adapters: { feedback, attachments },
  // 不传 setMessages：Fox 没有会话内分支，分叉仍走新会话
})
```

- 所有 handler 用 `useCallback`（或现有 `useStableCallback`）保持引用稳定，否则运行时会被反复重建。
- `onNew` 要复用现有的乐观消息逻辑（`handleComposerPromptCommit` 6001），保证发送瞬间用户消息就出现。
- `adapters.attachments`：包装现有附件规则（最多 8 个、单个 5 MB、PDF/TXT/MD/DOCX/XLSX/PPTX/CSV/TSV，运行时支持时允许图片），复用 `prompt-attachment-data` 里的读取和校验逻辑。
- `adapters.feedback`：调用 `desktopClient.saveMessageFeedback`。

### 4.4 暂不迁移（第一版保留 Fox 自有实现）

- 运行中补充指令（steer / 排队到下一轮 / 停止）：assistant-ui 的 `queue`（`createMessageQueue`，steer 和普通两条队列）语义接近，但先保留 `useRunSteering`。等界面迁移稳定后再评估是否换成 `queue` 适配器。
- `@` 提及和 `/` 命令：assistant-ui 的触发器 API（`unstable_useMentionAdapter` 等）还是 unstable。第一版沿用 Fox 自己的斜杠菜单，用 shadcn `Popover` + `Command` 实现，挂在新输入框上。
- 会话列表：侧栏不换成 `ThreadList`，继续用现有 Sidebar。

**验收**：开关为 `assistant-ui` 时，新运行时能驱动一个最简的 `ThreadPrimitive` 列表正常收发、流式、停止；开关为 `legacy` 时行为完全不变；转换单测通过；build 预算通过。

## 5. 阶段 5C：消息列表（Thread）

新文件放在 `src/features/chat/thread/`，用 assistant-ui 原语（`ThreadPrimitive`、`MessagePrimitive`、`ActionBarPrimitive`、`ComposerPrimitive`、`AuiIf`）自己组装。可以参考官方注册表的 Radix 版 `thread` 组件源码，但**不要直接用 shadcn CLI 把它生成进项目**（会带进不符合我们规范的样式）。

| 文件 | 内容 |
|---|---|
| `fox-thread.tsx` | `ThreadPrimitive.Root` + `Viewport`（自动贴底、`LoadEarlier`）+ `Messages` + 回到底部按钮 + 底部停靠区（计划浮层） |
| `fox-user-message.tsx` | 用户气泡（复用 `UserMessageBubble` 的视觉）、附件、操作栏（复制、编辑：只在最后一条用户消息上） |
| `fox-edit-composer.tsx` | 编辑最后一条用户消息（`ComposerPrimitive` 编辑模式：取消 / 发送，按钮靠右，主按钮只有一个） |
| `fox-assistant-message.tsx` | `MessagePrimitive.Parts`，部件映射见下表；操作栏：复制、点赞/点踩、重试（仅最后一条）、分叉（自定义按钮调用 `requestConversationFork`） |
| `parts/fox-text.tsx` | 懒加载现有 `MessageResponse`（Streamdown） |
| `parts/fox-process.tsx` | `data-fox-process` → 现有过程组件（从 `timeline/runtime-process-view.tsx` 复用） |
| `parts/fox-sources.tsx` | `source` 部件合并成一行紧凑来源列表 |
| `parts/fox-artifacts.tsx` | `data-fox-artifacts` → `runtime-artifacts.tsx` |
| `parts/fox-error.tsx` | `data-fox-error` → 现有错误展示，带重试 |
| `fox-thread-empty.tsx` | 新对话空状态（吉祥物 + 建议问题，`SuggestionPrimitive` 或现有 Suggestions） |
| `fox-thread.css` | 本目录全部样式，只用规范变量 |

视觉要求（按 `PAGE-REDESIGN.md` 第 3.10 节）：

- assistant 消息没有外框和底色，正文宽度上限约 720px，正文 14px、行高 1.6。
- 用户消息是唯一有底色的气泡（`--muted`，圆角 `--fox-radius-large`）。
- 表格只有细分隔线，不再套卡片；代码块一层边框。
- 来源合并成一行文字小标签，不用带边框的卡片。
- 模型名、时间、操作按钮默认隐藏，悬停消息或键盘聚焦时出现（最后一条 assistant 消息常显）。
- 过程区默认折叠为一行摘要（例如「思考 10 秒 · 调用 3 个工具」），运行中展开当前步骤。

**验收**：第 7 节清单中「消息列表」部分全部通过；500 条消息的合成时间线滚动无掉帧（`dev:profile` 下对比旧实现，提交 Profiler 数据）；`runtime-timeline-performance` 等性能测试通过。

## 6. 阶段 5D：输入框（Composer）

`src/features/chat/thread/composer/`，基于 `ComposerPrimitive.Root` / `Input` / `Send` / `Cancel` / `Attachments`：

- **按键**：Enter 发送；Shift+Enter 和 Ctrl/Cmd+Enter 换行；输入法组字时不发送。行为必须与 `prompt-send-shortcut.ts` 一致，把 `prompt-send-shortcut`、`prompt-input-keyboard-dom` 测试改为针对新输入框（旧测试在删除旧实现前保留）。
- **附件**：`ComposerPrimitive.Attachments` + 4.3 的附件适配器；项目、知识库标签和溢出浮层保留现有交互。
- **斜杠命令**：`/目标`、`/文件`、`/知识库`（内联选择远程/本地知识库）、`/计划`、`/总结`，选中后显示模式标签，发送时用 `slashCommandRuntimeText` 改写。
- **专家**：`ExpertBindingChip` + 懒加载 `ExpertPickerDialog`。
- **权限模式菜单**：询问 / 允许 / 只读，用 shadcn `DropdownMenu`。
- **决策卡**：有待处理的审批、提问、目标/计划/工作流闸门时，用决策卡替换输入区（复用 `RuntimeApprovalPrompt`、`GoalProgress` 等），同时 `isSendDisabled = true`。
- **运行中**：发送按钮变停止按钮（`ComposerPrimitive.Cancel`）；补充指令条 `RunSteeringStrip`、`ManagedFilesPanel` 保持现有逻辑。
- **其它**：拼写检查偏好、草稿保存（`composerDraft`）、`onHeightChange` 测量（舞台布局依赖它）。
- **视觉**：输入框一层边框、圆角 `--fox-radius-large`、底部一行工具按钮（28px 图标按钮）+ 右侧唯一主按钮（发送）；去掉 `BorderBeam` 和 `MetalSendButton` 这类装饰（如需保留，先问用户）。

**验收**：第 7 节「输入框」部分全部通过；`steering-composer-*`、`prompt-input-*` 测试迁移到新输入框后通过。

## 7. 功能对齐清单（开关切到 assistant-ui 前必须全部打勾）

消息列表：
- [ ] 发送后用户消息立即出现（乐观消息），运行结束后被真实消息替换且不闪烁
- [ ] 流式文本逐帧追加；`<think>` 内容不出现在正文
- [ ] 过程区：思考、工具调用、分组标题、耗时、尝试状态与旧实现一致；可折叠
- [ ] 停止生成后消息标记为已中断
- [ ] 失败消息显示错误和重试
- [ ] 只有最后一条用户消息可编辑；编辑后重跑会截断后续消息
- [ ] 重试最后一轮
- [ ] 分叉到新会话
- [ ] 复制、点赞、点踩（含反馈对话框）
- [ ] 加载更早消息后滚动位置不跳
- [ ] 自动贴底；用户上滚后不强制贴底；回到底部按钮
- [ ] 来源、引用、产物、文件预览入口（`openFile` / `openArtifact` / `openEvidence`）
- [ ] Markdown：代码块折叠/复制、表格本地全屏、数学公式、Mermaid、中英文混排
- [ ] 计划浮层、目标进度、会话轮次导航（`ConversationTurnRail`）
- [ ] 切换会话时不串消息（会话 id 变化时运行时状态重置）

输入框：
- [ ] Enter / Shift+Enter / Ctrl+Enter / 输入法组字
- [ ] 附件：数量、大小、类型限制及错误提示；图片仅在支持时可用
- [ ] 斜杠命令 5 个，知识库内联选择
- [ ] 专家选择、项目与知识库标签
- [ ] 权限模式
- [ ] 审批、提问、目标/计划/工作流闸门的决策卡
- [ ] 运行中：停止、补充指令、排队到下一轮
- [ ] 草稿保留、重置（`composerResetKey`）
- [ ] 键盘可达：Tab 顺序、焦点环、所有图标按钮有 `aria-label`

## 8. 验证方法

- **合成时间线**：`pnpm --dir apps/desktop dev:profile` 默认端口 1422；为避免冲突，用自己的临时配置改端口后访问 `/?foxPerf=1`（见 `src/features/profiling/README.md`）。用于性能对比和长对话截图。
- **截图**：参考 `output/ui-audit/step4/*/capture.py`、`cdp.py`、`pairs.py` 的做法（无头 Edge + CDP，1400×900，明暗各一套）。新旧实现各截一套，生成前后对比图放到 `output/ui-audit/step5/`。至少覆盖：空对话、普通问答、带思考和工具调用、失败、运行中、编辑状态、审批决策卡、斜杠菜单、附件、长表格、代码块、深色模式。
- **注意**：无 Tauri 后端时页面会显示 `invoke` 相关报错，这是预览环境的限制，不是回归。

## 9. 阶段 5E：切换默认值并删除旧实现

- 开关默认值改为 `assistant-ui`，用户在真实 Tauri 环境确认一轮后再继续。
- 删除旧实现：`timeline/runtime-timeline.tsx`、`runtime-assistant-message.tsx`、旧 `composer/`（逻辑已复用的部分保留）、`demo/` 中不再需要的部分。
- 删除只被旧聊天界面使用的 ai-elements：`prompt-input*`、`conversation`、`message`、`chain-of-thought`、`suggestion`、`sources`、`inline-citation`、`attachments`、`confirmation`（确认 `RuntimeApprovalPrompt` 已不再依赖）等；`message-response`、`code-block`、`tool` 仍被知识库/沙盒使用，保留。依赖包同理（`use-stick-to-bottom` 等），以 `rg` 结果为准。
- `workbench.css` 中聊天相关的旧规则（约 2580–5510、10500–10711、11706–11970 行附近）逐块删除，每删一块截图确认无影响；`process-row-slots-css.test.ts` 随之改为读新 CSS 或删除。
- 更新 `docs/03-产品与前端/前端架构.md`、`对话体验.md`、`docs/02-架构/对话系统架构.md` 中关于 Timeline / Composer 的描述。
- 这一阶段与 `UI-PLAN.md` 第 7 步（删除无用组件和旧 CSS）合并完成。

**验收**：build、测试、截图全部通过；`rg "ai-elements/(prompt-input|conversation|chain-of-thought)" apps/desktop/src` 无结果；`workbench.css` 行数明显下降（在报告里给出前后行数）。

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| assistant-ui 增加首屏体积超预算 | 新界面懒加载；不擅自改预算 |
| 流式时整列表重渲染导致卡顿 | 稳定 id + `getMetadataKey` 只重转最后一条；Profiler 对比旧实现 |
| 过程模型语义丢失 | 第一版用 `data-fox-process` 原样承载，不拆成原生 tool-call |
| assistant-ui 版本变动（0.x，部分 API 为 unstable） | 锁定小版本；不使用 `unstable_*` API |
| 会话切换串消息 | 会话 id 作为 `ThreadPrimitive.Root` 的 `key`，切换时重建 |
| 测试依赖源码文本和旧类名 | 5A 保留导出和函数顺序；5C/5D 新增测试，5E 再删除旧测试 |
| 多 agent 并行冲突 | 按第 3 节划分文件归属；共享文件只用小块 Edit |

## 11. 交付物

每个阶段结束时提交一份英文或中文报告：改动文件列表、对齐清单勾选情况、build/测试结果、包体积数字（入口/首屏）、性能对比数据、截图对比路径、未完成项和原因。
