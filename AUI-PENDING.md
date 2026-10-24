> 历史文档，内容截至 2026-10-11，现状见 [docs/02-架构/架构文档导航.md](docs/02-架构/架构文档导航.md) 与当前代码（本文不再维护）。

# 暂缓替换的 assistant-ui 组件：Fox 侧数据缺口与立项条件

配套 `UI-PLAN.md` 第 5/6 步。这里只收**当前不做**的四项，以及三个名字对不上的条目怎么归位。
每条都给"缺什么 / 要动哪张表或哪个事件 / 前端接哪个组件 / 最小可行做法"。行号为 2026-10-11 现场。

## 1. Inline citation（行内引用）

- **要接的组件**：`elements-inline-citation`（依赖 `elements-sources`、`elements-href`），运行时侧走 source part：`sourceType: 'url' | 'document' | 'id'`，字段 `id/url/title/snippet`。
- **Fox 现状**：来源是**独立事件** `source.added{id,title,knowledgeBaseId,documentId,excerpt,chunkId,page,anchor}`（`apps/desktop/src-tauri/src/yuxi/runtime.rs:1100-1110`），前端聚合进 `runtimeProcess().sources`（`apps/desktop/src/features/chat/timeline/fox-runtime-process-model.ts:277-288`）；正文由 `thread/parts/fox-text.tsx:9` 直接交给 Markdown，不解析任何引用标记。仓库里 `InlineCitation*` 组件已在，但只在示例提示词里硬编码使用（`features/chat/demo/fox-demo-prompts.tsx:193-197`）。
- **缺口**：正文与来源的对应关系不存在。要么 kernel 产出带引用锚的正文（脚注 `[^3]` 或自定义 directive），要么 Host 侧后处理补写编号。
- **牵连面**：`message.delta` 现在只带 `deltaLength` + `deltaFingerprint`，改成结构化 segment 要同时动增量指纹与持久化——`messages.content` 是单字符串（`database/migrations.rs:35-48`）。
- **最小可行**：先不平行内标记，做"折叠来源列表 + 点击定位到 chunk"。定位器字段已经够用：`KnowledgeSourceLocator{chunkId,page,anchor,excerpt}`（`features/workspace/types.ts:40-45`）。

## 2. Quote（选区引用后再提问）

- **要接的组件**：`SelectionToolbarPrimitive.{Root,Quote}`、`ComposerPrimitive.{Quote,QuoteText,QuoteDismiss}`、`MessagePrimitive.Quote`、`useMessageQuote`，以及 `quote.aui.tsx`。
- **Fox 现状**：`document.getSelection()` 只用于判断要不要折叠过程块（`timeline/fox-runtime-assistant-message.tsx:84-90`）。`startRun` 请求体只有 `{conversationId,text,runtimeText,model,attachmentIds,budget}`（`features/conversations/api/desktop-client.ts:735`）。消息表没有 `reply_to`/`parent_message_id`，最接近的结构性关系是 fork：`parentConversationId`/`forkedFromMessageId`/`lineageRootId`（`features/conversations/model/types.ts:359-361`）。
- **缺口**：① 选区采集成 `{messageId, 文本, 角色}`；② `startRun` 增加 quote 载荷并由 kernel 拼进 prompt；③ 持久化选择——加列，或把引用块编码进 content 再在渲染时还原。
- **最小可行**：先做"引用文本随消息一起发送、不建立结构关系"，只动 composer 与 `startRun` 载荷；真正的 reply-to 归入 kernel 协议改动一起做。

## 3. Confidence（置信度）

- **要接的组件**：`elements-confidence-marker`（纯展示件，props 为 `ConfidenceClaim[]` + `onHover`，运行时不产出）。
- **Fox 现状**：记忆侧有 `MemoryEntityRecord.confidence`（0..1，`model/types.ts:408`）与 `MemoryRecallRecord.score/rank`（`:437-438`，打分实现 `src-tauri/src/database/repositories/memory.rs:843-877`）；知识库检索有 `items[].score/keywordScore/vectorScore`（`features/local-knowledge/model.ts:317-327`）与重排 `sort_by_key(Reverse(score))`（`src-tauri/src/yuxi/knowledge_search.rs:455`）。**但** `source.added` 不带分数（`runtime.rs:1100-1110`），模型侧无 logprob（全仓检索 0 命中），工具结果无可靠性字段。
- **缺口**：答案侧要显示置信度，最小改动是给 `source.added` 补 `score`——Rust 事件结构体 + 协议生成（`scripts/generate-engine-protocol.mjs`）+ TS 类型三处同步。
- **风险**：本地 KB、远端 Yuxi、`web_search` 三条路的分数不可比，直接摊出数值会误导。建议先离散成高/中/低三档，hover 再显示原始分数与来源。

## 4. Structured output（结构化输出 / Generative UI）

- **要接的组件**：官方页面叫 **Generative UI**，没有 "Structured output" 条目。运行时侧 `generative-ui` part + `GenerativeUIRender` + `MessagePrimitive.GenerativeUI` + `GenerativeUISpec` 在 0.15.26 可用；**但 registry 组件依赖未安装的 `@assistant-ui/generative-ui@^0.0.2`**，且 `assistant-stream@0.3.49` 不导出 `JSONGenerativeUI`。
- **Fox 现状**：无 `response_format`/`json_schema`/`structuredOutput`（全仓 0 命中）。有：工具入参 `McpToolRecord.inputSchema`（`types.ts:103`）、`structured_data` 工具（`src-tauri/src/runtime_host/capability_tools.rs:295,742`）、artifact 的 `artifactType/artifactClass/delivery.verificationStatus`（`types.ts:598-622`）、工作流阶段 `output: unknown`（`:1129`，无 schema）。
- **缺口**：① 模型调用层要能声明并强制 schema（kernel provider 适配）；② 新增一个直接依赖 `@assistant-ui/generative-ui`，需按供应链规则核对精确版本、发布年龄与传递依赖，不能通配放行。
- **最小可行**：先用 `structured_data` 工具结果喂 `elements-data-table` / `elements-spec-sheet`（展示件，零新依赖），把 schema 强制留给 kernel 侧立项。

## 名字对不上，按这个归位

- **File AUI** → 你已确认指"消息里的附件/文件渲染"。官方没有 `file.aui`，只有展示件 `file`；做运行时接线要用 `attachment.aui.tsx` + `elements-message-attachment`，数据侧对应 `thread/use-fox-runtime.ts:185` 的附件 part `{type:'file',filename,mimeType,data,sourceType}`。归入阶段③ composer/附件改造一起做。
- **Knowledge** → 无同名条目，对应 `sources`（已实现：`thread/parts/fox-sources.tsx`）、`elements-retrieval-chunks`、`elements-document-reference`。
- **Agents** → 无同名条目，对应 `agent-card`、`agent-status.aui`、`elements-subagent-list`、`elements-agent-plan`、`elements-agent-handoff`、`elements-task-card`、`elements-todo-list`。数据是齐的（`ChildRunRecord{parentRunId,childRunId,depth,status,teamMemberId}`，`types.ts:493-518`；右侧子 agent 会话面板 `features/chat/right-panel/fox-right-panel.tsx:575-638`）。**接之前要先去掉** `fox-right-panel.tsx:464-491` 的 `previewRuns` 假数据兜底——`detail.childRuns` 为空时它会渲染不存在的子 agent。
