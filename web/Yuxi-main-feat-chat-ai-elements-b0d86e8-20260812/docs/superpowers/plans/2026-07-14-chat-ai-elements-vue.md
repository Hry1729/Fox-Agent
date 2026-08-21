# Chat 主区 AI Elements Vue 换肤 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不改后端的前提下，把 FoxOps 对话页右侧主对话区（气泡、思考、工具卡、输入外壳、来源、加载/空态、审批视觉）全部换成 AI Elements Vue 组件渲染。

**Architecture:** 保留现有 SSE / Pinia / `displayItems` 管道；新增 adapter 把现有气泡数据映射到 Elements props；在对话页作用域内引入 shadcn-vue + ai-elements 源码组件；工具解析逻辑（`toolRegistry`）保留，UI 外壳统一为 Elements `tool`。左侧会话列表与 Art 外壳不动。

**Tech Stack:** Vue 3、Vite、Tailwind 4、shadcn-vue、ai-elements-vue（registry 拷贝）、现有 Element Plus（外壳/侧栏）、现有自研 chat hooks。

## Global Constraints

- 后端 API / SSE / tool_call schema 零改动
- 左侧 thread 侧栏不迁移
- 不引入 `@ai-sdk/*` 运行时作为聊天引擎（可用 Elements 组件 only）
- shadcn 主题变量限定在对话主区作用域，避免全站样式漂移
- 中文注释，UTF-8；不顺手重构无关模块
- 测试优先放 `foxops-web/tests/*.test.mjs`（现有契约测试风格）

## File Map

| Path | Responsibility |
|------|----------------|
| `foxops-web/components.json` | shadcn-vue 配置 |
| `foxops-web/src/components/ui/*` | shadcn 基础组件（CLI 生成） |
| `foxops-web/src/components/ai-elements/*` | AI Elements 组件（CLI 生成后可改） |
| `foxops-web/src/assets/styles/chat-ai-elements.scss` | 对话岛 CSS 变量 / 限定作用域 |
| `foxops-web/src/utils/chat/elementsAdapter.ts` | displayItem → Elements message parts |
| `foxops-web/src/utils/chat/toolElementsAdapter.ts` | toolCall → Elements tool props |
| `foxops-web/src/components/chat/ElementsConversation.vue` | 消息列表（替 BubbleList） |
| `foxops-web/src/components/chat/ElementsPromptShell.vue` | 输入外壳（接现有发送逻辑） |
| `foxops-web/src/components/chat/ElementsToolCall.vue` | 统一工具卡外壳 |
| `foxops-web/src/components/ToolCallingResult/*` | 逐步改渲染为包一层 Elements，或删外壳改用 ElementsToolCall |
| `foxops-web/src/views/chat/agent/index.vue` | 接线：用新组件替换 EP-X 块 |
| `foxops-web/tests/chat-elements-adapter.test.mjs` | adapter 契约 |
| `foxops-web/tests/chat-elements-presence.test.mjs` | 页面不再依赖 EP-X 关键符号 |

---

### Task 1: 初始化 shadcn-vue + 安装 AI Elements 组件

**Files:**
- Create: `foxops-web/components.json`
- Create: `foxops-web/src/components/ui/**`（CLI）
- Create: `foxops-web/src/components/ai-elements/**`（CLI）
- Create: `foxops-web/src/assets/styles/chat-ai-elements.scss`
- Modify: `foxops-web/src/assets/styles/index.scss`（仅 import 对话岛样式）
- Modify: `foxops-web/package.json` / `pnpm-lock.yaml`（CLI 依赖）

**Interfaces:**
- Produces: 可从 `@/components/ai-elements/conversation` 等路径导入组件

- [ ] **Step 1: 写失败探测测试（组件目录尚不存在）**

Create `foxops-web/tests/chat-elements-presence.test.mjs`:

```js
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const aiDir = path.join(root, 'src/components/ai-elements')

test('ai-elements 组件目录已安装', () => {
  assert.ok(fs.existsSync(aiDir), '缺少 src/components/ai-elements')
  for (const name of ['conversation', 'message', 'reasoning', 'tool', 'prompt-input']) {
    const hits = fs.readdirSync(aiDir).some((f) => f.includes(name) || fs.existsSync(path.join(aiDir, name)))
    assert.ok(hits, `缺少 ai-elements: ${name}`)
  }
})
```

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test tests/chat-elements-presence.test.mjs`  
Expected: FAIL（目录不存在）

- [ ] **Step 3: 初始化并安装组件**

在 `foxops-web` 下：

```bash
pnpm dlx shadcn-vue@latest init
pnpm dlx ai-elements-vue@latest add conversation message reasoning tool prompt-input loader suggestion sources confirmation code-block artifact
```

若 CLI 交互无法非交互完成：改用  

`pnpm dlx shadcn-vue@latest add https://registry.ai-elements-vue.com/all.json`  

并只保留上表需要的组件文件。

- [ ] **Step 4: 增加对话岛样式作用域**

`chat-ai-elements.scss` 示例骨架：

```scss
.chat-ai-elements {
  /* shadcn CSS variables 挂在此作用域；具体值来自 shadcn init 生成的主题 */
  color-scheme: inherit;
}
```

在 `index.scss` 中：`@use './chat-ai-elements';`

- [ ] **Step 5: 再跑测试确认通过**

Run: `node --test tests/chat-elements-presence.test.mjs`  
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add foxops-web/components.json foxops-web/src/components/ui foxops-web/src/components/ai-elements foxops-web/src/assets/styles foxops-web/package.json foxops-web/pnpm-lock.yaml foxops-web/tests/chat-elements-presence.test.mjs
git commit -m "chore(foxops-web): 引入 shadcn-vue 与 AI Elements 对话组件"
```

---

### Task 2: displayItem → Elements adapter

**Files:**
- Create: `foxops-web/src/utils/chat/elementsAdapter.ts`
- Create: `foxops-web/tests/chat-elements-adapter.test.mjs`

**Interfaces:**
- Consumes: 现有 bubble/display 项字段（`_type` `_reasoning` `content` `placement` `_toolCalls` `_sources` …）见 `views/chat/agent/index.vue` 中 `bubbleListItems` 构造
- Produces:

```ts
export type ElementsRole = 'user' | 'assistant' | 'system'

export interface ElementsMessagePart {
  type: 'text' | 'reasoning' | 'tool-group' | 'sources' | 'artifacts' | 'error' | 'attachments' | 'image'
  // 透传原始字段，供 slot 渲染
  payload: Record<string, unknown>
}

export interface ElementsMessageView {
  id: string
  role: ElementsRole
  parts: ElementsMessagePart[]
  meta: {
    sender?: string
    time?: string
    avatar?: string
    modelName?: string
    msgId?: string
    feedback?: unknown
  }
}

export function toElementsMessages(items: any[]): ElementsMessageView[]
```

- [ ] **Step 1: 写失败测试**

```js
import assert from 'node:assert/strict'
import test from 'node:test'
import { toElementsMessages } from '../src/utils/chat/elementsAdapter.ts'

test('maps user text and assistant reasoning', () => {
  const views = toElementsMessages([
    { key: 'u1', placement: 'end', content: '你好', _sender: '我' },
    { key: 'a1', placement: 'start', content: '答案', _reasoning: '思考中', _sender: 'Bot' }
  ])
  assert.equal(views[0].role, 'user')
  assert.equal(views[1].parts.some((p) => p.type === 'reasoning'), true)
})
```

（若项目测试不用直接 import `.ts`，改为读文件断言导出符号存在 + 用纯 `.js` 实现文件 `elementsAdapter.js`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test tests/chat-elements-adapter.test.mjs`  
Expected: FAIL

- [ ] **Step 3: 实现 `toElementsMessages`**

按 `placement === 'end'` → `user`，否则 `assistant`；`_type === 'tool-group'` → part `tool-group`；有 `_reasoning` → part `reasoning` 后再跟 `text`。

- [ ] **Step 4: 跑测试确认通过**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add foxops-web/src/utils/chat/elementsAdapter.* foxops-web/tests/chat-elements-adapter.test.mjs
git commit -m "feat(foxops-web): 新增对话气泡到 AI Elements 的数据适配层"
```

---

### Task 3: 消息列表 UI — conversation / message / reasoning

**Files:**
- Create: `foxops-web/src/components/chat/ElementsConversation.vue`
- Modify: `foxops-web/src/views/chat/agent/index.vue`（消息列表区域包 `chat-ai-elements`，用新组件替换 `BubbleList`/`Thinking`/`Welcome`/`Prompts`）

**Interfaces:**
- Consumes: `toElementsMessages(bubbleListItems)`
- Produces: 滚动、复制、反馈、重试事件与现有 handler 对齐（`copyMessage` `retryMessage` `MessageFeedback`）

- [ ] **Step 1: 写失败测试（agent 页不再 import BubbleList）**

Extend `chat-elements-presence.test.mjs`:

```js
test('chat agent 不再依赖 vue-element-plus-x BubbleList', () => {
  const src = fs.readFileSync(path.join(root, 'src/views/chat/agent/index.vue'), 'utf8')
  assert.doesNotMatch(src, /BubbleList/)
  assert.match(src, /ElementsConversation|ai-elements/)
})
```

- [ ] **Step 2: 跑测试确认失败**（仍含 BubbleList）

- [ ] **Step 3: 实现 `ElementsConversation.vue`**

- 外层用 Elements `Conversation`
- 每条用 `Message`；reasoning part 用 `Reasoning`
- text：用户继续 `MentionTextRenderer`，助手继续 `v-html="renderMarkdown(...)"`
- footer：保留复制 / Feedback / 重试 / 模型名
- 暴露 `scrollToBottom` 方法供流式滚动

- [ ] **Step 4: 在 `index.vue` 接线并去掉 BubbleList/Thinking/Welcome/Prompts 引用**

根消息区：

```vue
<div class="chat-ai-elements flex-1 min-h-0 flex flex-col ...">
  <ElementsConversation ... />
</div>
```

- [ ] **Step 5: 手动/脚本验证 + 单测 PASS**

Run: `node --test tests/chat-elements-presence.test.mjs tests/chat-elements-adapter.test.mjs`

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(foxops-web): 对话消息列表切换为 AI Elements conversation/message"
```

---

### Task 4: 工具卡外壳与内含渲染 → Elements `tool`

**Files:**
- Create: `foxops-web/src/utils/chat/toolElementsAdapter.ts`
- Create: `foxops-web/src/components/chat/ElementsToolCall.vue`
- Modify: `foxops-web/src/components/ToolCallingResult/ToolCallRenderer.vue`
- Modify: `foxops-web/src/components/ToolCallingResult/BaseToolCall.vue`（改为薄封装或删除后由 ElementsToolCall 承接）
- Modify: 各 `tools/*.vue`——去掉自定义「卡片 chrome」，只保留领域 body，放入 Elements tool content slot
- Create: `foxops-web/tests/chat-tool-elements-adapter.test.mjs`

**Interfaces:**
- Consumes: `parseToolCallArgs` / `getToolCallId` / `isHiddenToolCall`（`toolRegistry.js`）
- Produces:

```ts
export function toElementsToolProps(toolCall: any): {
  name: string
  type: string
  state: 'input-streaming' | 'input-available' | 'output-available' | 'output-error'
  input?: unknown
  output?: unknown
  errorText?: string
}
```

- [ ] **Step 1: 写 adapter 失败测试**（running/success/error 三态映射）

- [ ] **Step 2: 实现 `toElementsToolProps` + `ElementsToolCall.vue`**

`ElementsToolCall` 使用 ai-elements `Tool` 组件；在 content slot 内：

- 有专用组件：`<component :is="specialized" ... />`（原 tools/*）
- 无专用：展示 input/output 的 `CodeBlock`

- [ ] **Step 3: `ToolCallRenderer` 改为只渲染 `ElementsToolCall`**

- [ ] **Step 4: 批量改 tools/\*** — 删除外层 `.tool-call-display` 依赖，只保留结果可视化

建议顺序：先通用（Read/Write/Grep/Glob/Execute）→ KB/MySQL → Chart/Mindmap/Image → AskUser/Todo/Task。

- [ ] **Step 5: 契约测试 + 手工一条含工具的流式对话**

Run: `node --test tests/chat-tool-elements-adapter.test.mjs`

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(foxops-web): 工具调用 UI 切换为 AI Elements tool"
```

---

### Task 5: 输入区外壳 → `prompt-input`（逻辑保留）

**Files:**
- Create: `foxops-web/src/components/chat/ElementsPromptShell.vue`
- Modify: `foxops-web/src/components/chat/MessageInputComponent.vue`（或被 Shell 包裹）
- Modify: `foxops-web/src/views/chat/agent/index.vue`

**Interfaces:**
- Consumes: 现有 `submit` / `stop` / Mention / 附件 / `ModelSelectorComponent`
- Produces: 同现有输入事件契约

- [ ] **Step 1: 用 Elements `PromptInput` 做外框，内部继续挂现有 Mention 编辑器与按钮**

- [ ] **Step 2: 确保发送/停止/附件/模型选择回归可用**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(foxops-web): 对话输入区外壳切换为 AI Elements prompt-input"
```

---

### Task 6: 来源 / 加载 / 空态 / 审批视觉

**Files:**
- Modify: `foxops-web/src/components/chat/RefsComponent.vue` → Elements `sources`
- Modify: `foxops-web/src/components/chat/ApprovalCard.vue` → Elements `confirmation` 视觉
- Modify: `ElementsConversation.vue` 空态 + loader
- Optional: `ArtifactsCard.vue` → `artifact` 外壳

- [ ] **Step 1: Refs → Sources，数据字段不变**

- [ ] **Step 2: ApprovalCard 改 confirmation 布局，仍调用 `handleApprovalSubmit/Cancel`**

- [ ] **Step 3: 空态 suggestion + loader

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(foxops-web): 来源/空态/审批视觉对齐 AI Elements"
```

---

### Task 7: 移除对话页对 vue-element-plus-x 的依赖并收尾

**Files:**
- Modify: `foxops-web/src/views/chat/agent/index.vue`
- Modify: `foxops-web/package.json`（若全仓无引用则删除依赖）
- Modify: `foxops-web/tests/chat-elements-presence.test.mjs`（断言无 `vue-element-plus-x` import）

- [ ] **Step 1: rg 确认无残留 `vue-element-plus-x`**

```bash
rg "vue-element-plus-x" foxops-web/src
```

Expected: 无匹配（或仅文档）

- [ ] **Step 2: 删除依赖并 `pnpm install`**

- [ ] **Step 3: 跑全套相关测试 + `pnpm exec vue-tsc --noEmit`（若过慢可限定 chat 相关）**

```bash
node --test tests/chat-*.test.mjs tests/page-content-transition.test.mjs tests/agent-state-fetch.test.mjs
```

- [ ] **Step 4: Commit**

```bash
git commit -m "refactor(foxops-web): 对话页移除 vue-element-plus-x，收尾 AI Elements 换肤"
```

---

## Spec Coverage Self-Review

| Spec 要求 | Task |
|-----------|------|
| conversation/message/reasoning | Task 3 |
| 工具卡含内部 chrome → Elements tool | Task 4 |
| prompt-input 外壳 | Task 5 |
| sources/loader/suggestion/confirmation/artifact | Task 6 |
| 侧栏不换、后端不换 | Global + 不改 `thread` 侧栏模板与 backend |
| shadcn 岛化 | Task 1 |
| 去掉 EP-X | Task 7 |
| adapter | Task 2 / 4 |

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-14-chat-ai-elements-vue.md`.

**Two execution options:**

1. **Subagent-Driven（推荐）** — 每任务新开子代理，任务间审查  
2. **Inline Execution** — 本会话按 executing-plans 连续做并设检查点  

Which approach?
