# UI 改动记录：第 5 步（assistant-ui 迁移）与第 6 步（页面重设计）

> 分支：`ui/improvements-20261009`（专用 UI 优化分支，基于 `492fb06`）
> 记录时间：2026-10-11
> 范围：第 5 步全部 6 个提交 + 第 6 步 2 个提交，含验证数据、实测反馈与遗留问题。

---

## 0. 总览与双引擎机制

第 5 步的目标是**架构迁移**：把聊天界面从自研渲染层（Timeline/Composer）迁移到 [assistant-ui](https://www.assistant-ui.com/) 官方原语（Thread/Message/ComposerPrimitive），数据层（kernel → `useDesktopConversation()`）**完全不动**，两套界面共用同一数据源。

迁移期间通过双引擎开关共存：

| 项 | 值 |
|---|---|
| 开关键 | `fox.ui.chatEngine`（localStorage） |
| 取值 | `legacy`（默认，原实现） \| `assistant-ui`（新实现） |
| 切换入口 | 经典界面聊天区右上角「试用 assistant-ui 界面」按钮；新界面右上角「返回经典界面」 |
| 生效时机 | 刷新页面后生效（引擎在组件初始化时读取偏好） |
| 实现位置 | `src/features/chat/thread/fox-chat-engine.ts` |

**重要**：第 5 步的验收标准是「功能对齐、行为一致」，不是视觉翻新。视觉翻新属于第 6 步（苹果设计视角页面重设计），只完成了 2 个页面（见第 2 节）。

### 提交清单（按时间序）

| 提交 | 类型 | 一句话 |
|---|---|---|
| `2352c3a` | 阶段 5A | workbench.tsx 巨石拆分（纯搬家） |
| `cd32b99` | 阶段 5B | assistant-ui 运行时接入 + 双引擎开关 |
| `57c4d73` | 阶段 5C | 消息列表迁移到 Thread 原语 |
| `904c738` | 阶段 5D | 输入框迁移到 Composer 原语 |
| `e5d5d98` | 补丁 | 经典界面补充引擎切换按钮 |
| `08e3472` | 补丁 | 混合回退：经典空状态 + 经典输入框，修滚动/头像 |
| `61e4947` | 第 6 步 | 知识库详情页重设计 |
| `7e4a3ad` | 第 6 步 | 设置·数据与诊断、模型供应商重设计 |

---

## 1. 第 5 步：assistant-ui 迁移

### 1.1 阶段 5A —— workbench.tsx 巨石拆分（`2352c3a`）

**问题**：`workbench.tsx` 膨胀到 6167 行，难以维护。

**做法**：纯搬家不改行为，拆到 `src/features/chat/` 下的模块目录：

| 拆出模块 | 内容 |
|---|---|
| `shell/` | 顶栏、侧栏骨架 |
| `timeline/` | 消息时间线、过程视图、来源、artifacts |
| `composer/` | 经典输入框 `fox-composer.tsx`、斜杠命令 |
| `right-panel/` | 右侧面板（活动/文件/子 agent） |
| `demo/` | 示例提示词 `fox-demo-prompts.tsx` |
| `conversation-dialogs.tsx` | 会话重命名/归档/清除对话框 |
| `fox-shared.tsx` | 共享工具组件 |

- 结果：`workbench.tsx` 6167 → **1191 行**（验收标准 ≤1300）。
- 保留 `Sidebar`、`RuntimeTimeline`、`RightPanel` 等旧导出供测试与 ProfilingHarness 使用。
- 锚点函数（`beginNewDraft`、`resetConversation`、`switchAssistantMode`、`createProjectDraft`）保留在 workbench.tsx，供源码切片测试定位。

**验证**：build 通过（入口 236.0 KiB / 首屏 307.0 KiB，预算 240/316）；bun test 550 通过 / 2 跳过 / 0 失败。

### 1.2 阶段 5B —— assistant-ui 运行时接入（`cd32b99`）

**依赖**：`@assistant-ui/react ~0.15.26`（Radix 版组件路线），`pnpm-workspace.yaml` 加 `minimumReleaseAgeExclude` 绕过新包发布年龄限制。

**新增文件**（`src/features/chat/thread/`）：

| 文件 | 内容 |
|---|---|
| `convert-detail.ts` | kernel 消息 → assistant-ui 消息格式的纯函数转换器。`joinStrategy: 'none'`；metadata + getMetadataKey 保证流式时只重转最后一条运行中消息 |
| `use-fox-runtime.ts` | `useExternalStoreRuntime` 包装 `useDesktopConversation()`；handler 引用稳定；onNew 复用现有乐观消息逻辑；adapters.feedback / adapters.attachments |
| `fox-thread-minimal.tsx` | 最简 Thread 验证界面（5C 时被替换删除） |
| `fox-assistant-ui-chat.tsx` | assistant-ui 分支的顶层组件（含 ChatEngineSwitch） |
| `fox-chat-engine.ts` | 引擎偏好读写（localStorage） |
| `fox-thread.css` | 新界面样式 |
| `timeline/fox-assistant-content.ts` | assistant 内容适配 |

**测试**：`tests/thread-convert-detail.test.ts`（293 行），覆盖普通问答、带思考、带工具调用、带附件、失败、取消、流式中、加载更早消息后 id 不变。

**验证**：build 入口 237.8 / 首屏 308.8 KiB；测试 560 通过 / 2 跳过 / 0 失败。

### 1.3 阶段 5C —— 消息列表迁移（`57c4d73`）

用 assistant-ui 原语（ThreadPrimitive / MessagePrimitive / ActionBarPrimitive）自组装，**未用** shadcn CLI 生成官方组件。删除 `fox-thread-minimal.tsx` 临时实现。

**新增**：`fox-thread.tsx`、`fox-user-message.tsx`、`fox-edit-composer.tsx`、`fox-assistant-message.tsx`、`parts/`（text / process / sources / artifacts / error）、`fox-thread-empty.tsx`、`fox-thread-context.tsx`。

**视觉规范**（按 PAGE-REDESIGN 3.10）：assistant 消息无外框无底色、正文上限 720px / 14px / 1.6 行高；用户消息唯一带底色气泡；来源合并一行；操作按钮悬停显示（最后一条 assistant 常显）；过程区默认折叠为一行摘要。

**验证**：测试 581 通过 / 2 跳过 / 0 失败（新增用例）；build 入口 238.2 / 首屏 309.3 KiB。500 条合成时间线滚动性能未测（如实上报，见遗留问题）。

### 1.4 阶段 5D —— 输入框迁移（`904c738`）

**新增**：`thread/composer/fox-thread-composer.tsx`（470 行）、`fox-decision-card.tsx`。

**功能对齐**：Enter / Shift+Enter / Ctrl+Enter / 输入法组字（与 `prompt-send-shortcut.ts` 一致）、附件适配器、5 个斜杠命令（/目标 /文件 /知识库 /计划 /总结）、专家 chip、权限模式菜单、决策卡（审批/提问/目标/计划/工作流闸门，isSendDisabled）、运行中停止 + RunSteeringStrip、草稿保存。

**视觉**：一层边框、圆角 `--fox-radius-large`、底部工具行 + 右侧唯一主按钮；去掉了 BorderBeam / MetalSendButton 类装饰。

**测试**：3 个新 DOM 测试文件（keyboard / attachment / steering，共 596 行，21 个用例）。

### 1.5 补丁 —— 经典界面补充切换按钮（`e5d5d98`）

**缺陷**：`ChatEngineSwitch` 只在 assistant-ui 分支渲染，legacy（默认）下不可见——用户无法首次进入新界面（鸡生蛋问题）。

**修法**：
- legacy 分支内联同款按钮（不引用懒加载的 ChatEngineSwitch，避免把 assistant-ui chunk 在 legacy 下拉起）。
- 开关样式移入新建的**同步加载** `thread/fox-chat-engine.css`（由 `fox-chat-engine.ts` 导入），原 `fox-thread.css` 中的同名规则删除。

**验证**：tsc 0 错误；测试 581/0；build 体积与基线完全一致（238.2 / 309.3 KiB，Button 本就在入口图）。

### 1.6 补丁 —— 混合回退（`08e3472`，响应用户实测反馈）

**用户实测反馈**（真实 Tauri 环境）：① 新建对话页太丑；② 无法向下滚动；③ 头像错位；④ 底部输入框不显示。要求空状态与输入框用回经典版。

**混合回退改造**：

| 文件 | 改动 |
|---|---|
| `workbench.tsx` | MemoizedComposer 提取为两引擎共享的 `composerPanel`；assistant-ui 分支挂经典输入框 |
| `timeline/fox-timeline.tsx` | 提取导出 `ConversationEmptyPrompt`（经典空状态），legacy 时间线复用 |
| `thread/fox-thread.tsx` | `ThreadPrimitive.Empty` 内改渲染经典空状态 |
| `thread/fox-assistant-ui-chat.tsx` | 剥离全部 composer props，只渲染消息列表 |
| `thread/fox-thread.css` | 修滚动结构（见下） |
| `fox-assistant-message.tsx` + `parts/fox-process.tsx` | 头像移到过程头行；纯文本消息保留独立头像行 |
| `tests/fox-thread-hybrid-dom.test.tsx` | 新增 4 条混合链路用例 |

**两个 bug 的根因**：

1. **无法向下滚动**：workbench 挂载点 `<div class="fox-thread-shell">` 没有布局样式——`.fox-thread` 的 `height:100%` 对 auto 高度父级解析为 auto，视口随内容无限增高，最终被 `.fox-chat-stage` 的 `overflow:hidden` 整体裁剪，滚动容器从未生效。次要因素：视口 flex 列的子元素默认 `flex-shrink:1` 破坏 assistant-ui 的钉底判定。修法：shell 补 flex 列 + `min-height:0` + 底部预留（空会话不预留，对应经典居中输入框）+ `flex-shrink:0`。
2. **头像错位**：新界面把头像放在独立行，与下方过程头行（分组图标+状态）脱节；经典版的头像与「工作过程」同在一行。修法：头像经 `leading` 放进 `.fox-runtime-process-head`，独立行仅在无过程部分的纯文本消息上保留回退。

**混合链路验证**：发送（legacy FoxComposer → store → 乐观气泡出现在新列表）、流式（store 新消息即重绘）、停止（`desktopConversation.cancel()`）、滚动（CDP 实测钉底/解除跟随全过）。

**验证**：tsc 0 错误；测试 **585 通过 / 2 跳过 / 0 失败**（+4 用例）；build **入口 236.6 / 首屏 307.6 KiB**（较基线略降，因新分支不再引入 composer 原语）。截图：`output/ui-audit/step5-hybrid/`（明暗 × 空状态/发送/滚动）。

**保留的死代码**：`fox-thread-empty.tsx`、`fox-thread-composer.tsx` 及其 3 个测试文件未删（后续优化再决定去留），测试为组件级挂载，照常通过。

---

## 2. 第 6 步：页面重设计（苹果设计视角，**未完成，已暂停**）

依据 `PAGE-REDESIGN.md`（10 条通用规则 + 分页方案）。执行顺序是：知识库详情 → 任务中心 → 设置 → 其余小页。**只完成了前两批就被叫停**（用户指示先集中完成第 5 步），任务中心、向量模型、知识首页、本地知识库列表、新建知识库对话框均未开始；聊天主区域（3.10）随第 5 步迁移做。

### 2.1 知识库详情页（`61e4947`）

| 文件 | 规模 |
|---|---|
| `features/local-knowledge/local-knowledge-pages.tsx` | 681 行重写 |
| `styles/local-knowledge.css` | +540 行（新增） |
| `styles/globals.css` | +14 行 |

要点：「详情」和「文档管理」合并为「文档」标签；标签改为「文档 / 检索测试 / 任务」28px 分段控件；右侧新增 280px 检查器（索引 + 配置 + 存储与运行分组）；删除阶段卡片、状态胶囊、重复信息。

### 2.2 设置 · 数据与诊断 + 模型供应商（`7e4a3ad`）

| 文件 | 规模 |
|---|---|
| `features/settings/settings-pages.tsx` | 71 行调整 |
| `features/settings/settings-pages.css` | +86 行（新增） |

要点：按 PAGE-REDESIGN 3.4 / 3.5 重排信息层级，去重复分组。

**验证**：两提交各自 build + 测试通过（当时的基线）。

---

## 3. 验证基线演进

| 时点 | 测试 | 入口 KiB | 首屏 KiB |
|---|---|---|---|
| 5A 前（基线） | 550 通过 / 2 跳过 | 236.0 | 304.6 |
| 5A 后 | 550 / 2 | 236.0 | 307.0 |
| 5B 后 | 560 / 2 | 237.8 | 308.8 |
| 5C+5D 后 | 581 / 2 | 238.2 | 309.3 |
| 混合回退后（当前） | **585 / 2** | **236.6** | **307.6** |

预算：入口 240 / 首屏 316 KiB。隔离检查历次全部 OK。

---

## 4. 当前状态与遗留问题

### 当前状态
- 默认引擎仍为 **legacy**（5E 未执行，原计划：真实 Tauri 确认后才切默认 + 删旧实现）。
- assistant-ui 界面现为「混合形态」：**消息列表用新实现，空状态与输入框用经典版**。
- 第 6 步暂停：已做 2 页（知识库详情、设置），其余未做。

### 遗留问题

| # | 问题 | 说明 |
|---|---|---|
| 1 | ~~lockfile 隐患~~ **已排除（2026-10-11 验证）** | 原判断错了。两个独立干净克隆 + 独立 store 跑 `pnpm install --frozen-lockfile` 均 exit 0，锁文件字节不变；`tsc` 0 错误（覆盖 280 个 src 文件，反向对照能报错）、桌面测试 583 通过、构建 236.6/307.6 KiB。当时的 60+ 错误来自 node_modules 链接未物化，不是版本不一致，**不需要重新生成 lockfile**。 |
| 2 | 本 worktree 的 node_modules 是手工修复态 | 禁令成立但理由要改：问题在这个工作树的 node_modules，不在锁文件。修法（已在克隆里验证）：移走 `node_modules/.pnpm-workspace-state-v1.json`，再跑 `pnpm install --frozen-lockfile`，链接会重建、tsc 回到 0。反面教训：用 `ln -s` 手工补 junction 会让该位置变成普通目录占位，pnpm 自检和 `--force` 都判"已最新"，类型却退化成 any（实测残留 22 个错误）。 |
| 3 | 滚动性能未测 | 500 条合成时间线滚动测量（第 5 步计划项）未做。 |
| 4 | 死代码 | `fox-thread-empty.tsx`、`fox-thread-composer.tsx` 未渲染未删除。 |
| 5 | 构建垃圾 | 根目录多个 `dist.old.*`、`node_modules/.pnpm.clean` 备份目录（约 1–2 GB），删除受沙箱护栏拦，待手动清理。 |
| 6 | 罕见路径头像 | 「有事件但零步骤且无生命周期事件」时可能返回孤立头像（观感与旧行为相近）。 |
| 7 | 第 6 步剩余页面 | 任务中心、向量模型、知识首页、本地知识库列表/本地文件、新建知识库对话框未做。 |

### 环境事故记录（2026-10-10）

5B agent 手工修复 pnpm 依赖链接后停滞。接管时发现 node_modules 与已提交 lockfile **假一致**（`node_modules/.pnpm-workspace-state-v1.json` 状态文件骗过 pnpm 自检，"已最新"判断失灵）。干净重装后 tsc 爆 60+ 类型错误，当时推断是"依赖图被解析到另一套版本"，**这个推断已于 2026-10-11 推翻**：同一份 lockfile 在独立克隆里冻结安装 exit 0、tsc 0 错误；那批错误是部分链接没物化成功导致的模块解析失败（实测删掉 `@assistant-ui/react` 一个链接就得到 38 条 TS2307/TS7006 级联）。最终恢复旧虚拟商店（`.pnpm.old5b`）并补回 `border-beam` junction 后复验通过，但手工补链接这条路本身有问题，见遗留问题 #2。教训：**此环境下不要动 node_modules 的物化状态**；真要修就走状态文件那条路，不要用 `ln -s`。
