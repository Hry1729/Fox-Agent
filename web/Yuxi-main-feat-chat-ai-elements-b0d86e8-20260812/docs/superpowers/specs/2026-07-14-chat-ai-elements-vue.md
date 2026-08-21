# FoxOps 对话主区 AI Elements Vue 换肤 — 设计说明

> 日期：2026-07-14  
> 状态：已确认需求，待按实施计划执行  
> 相关库：[vuepont/ai-elements-vue](https://github.com/vuepont/ai-elements-vue)

## 1. 目标

在不改后端接口与业务协议的前提下，把 **对话页右侧主对话区** 的前端 UI 渲染，全部换成 **AI Elements Vue（shadcn-vue）** 体系，以获得 Elements 的大气观感。

**不在范围：** 左侧会话列表侧栏、ArtDesignPro 外壳导航、全站 Element Plus 替换、后端 / SSE / agent 协议。

## 2. 范围边界

### 2.1 要换成 Elements 的 UI

| 现有 | Elements 目标 |
|------|----------------|
| `BubbleList` / 消息气泡 | `conversation` + `message` |
| `Thinking` | `reasoning` |
| 工具调用外壳 + 各工具卡（`ToolCallingResult/*`） | `tool`（及必要时 `code-block` / `confirmation`） |
| 空态 `Welcome` / `Prompts` | `suggestion` + conversation 空态 |
| 底部输入（`MessageInputComponent` 外壳） | `prompt-input` 外壳，内部继续接 Mention / 附件 / 模型 / 发送停止 |
| 来源 `RefsComponent` 外壳 | `sources` / `inline-citation`（数据仍来自现有 extractSources） |
| 加载中三点 | `loader` / `shimmer` |
| 审批 `ApprovalCard` 视觉 | `confirmation`（业务回调不变） |
| Artifacts 卡片外层（可选同批） | `artifact` |

### 2.2 明确不换

- 左侧 thread 列表（布局、筛选、置顶删除等）
- 顶部 Agent 下拉、状态面板 / 文件工作区 **面板本体**（按钮区可微调样式，面板逻辑保留）
- 任意后端 API、run SSE、HITL 状态机、thread 模型
- 全站 shadcn 化（设置页、工作区、扩展页仍 Element Plus）

### 2.3 约束

- **后端零改动**（含消息 schema、tool_call 结构）
- 对话页允许多一套 CSS 变量（shadcn），与 Art/Element 外壳并存（「对话内容岛」）
- 组件以 **源码拷贝** 进 `foxops-web`（ai-elements CLI / shadcn-vue registry），可本地改
- 不引入 Vercel AI SDK 作为运行时依赖；仅用 Elements 组件做展示，数据仍来自现有 Pinia/hooks

## 3. 架构

```
现有 hooks / stores / API
        │
        ▼
adapter（displayItems → Elements message parts）
        │
        ▼
AI Elements 组件（conversation / message / reasoning / tool / prompt-input …）
        │
        ▼
业务插槽：MentionTextRenderer、Feedback、重试、模型名等仍挂在 message footer
```

- **Adapter 层**隔离：流式 chunk → 现有 `bubbleListItems` / display 逻辑尽量复用，再映射到 Elements 的 props/slots。
- **Tool 层：** `toolRegistry` 的解析与隐藏规则保留；渲染从自定义 `BaseToolCall` 迁到 Elements `tool` 容器，专用工具内容（图表、SQL、Mindmap 等）改为 Elements `tool` 的 content slot + 必要时保留少量领域可视化（ECharts 等），外壳统一 Elements。

## 4. 主题与依赖

1. 在 `foxops-web` 初始化 shadcn-vue（CSS Variables），路径建议 `src/components/ui`、`src/components/ai-elements`。
2. 对话页根节点加 `chat-ai-elements` 作用域 class，挂 shadcn 变量，避免污染全站。
3. 按需安装：`conversation` `message` `reasoning` `tool` `prompt-input` `loader` `suggestion` `sources` `confirmation` `code-block` `artifact`。
4. 迁移完成后，对话页移除对 `vue-element-plus-x` 的依赖；若全项目无引用再从 `package.json` 删除。

## 5. 验收标准

- [ ] 主对话区（气泡、思考、工具卡、输入外壳、来源、加载）视觉为 Elements 体系
- [ ] 左侧侧栏观感与交互与现网一致
- [ ] 流式输出、停止、重试、复制、反馈、Mention、附件、多线程、HITL 审批行为不回归
- [ ] 工具调用展示完整：running / completed / error；隐藏工具（如 `present_artifacts`）规则不变
- [ ] 不改后端；`pnpm` 能正常 build；相关契约测试通过

## 6. Checklist（实施前）

- [x] 需求确认：主对话区全 Elements UI（含内部工具卡）
- [x] 侧栏不换、后端不换
- [x] 当前进度已 git 提交（checkpoint）
- [ ] 按 `docs/superpowers/plans/2026-07-14-chat-ai-elements-vue.md` 执行
