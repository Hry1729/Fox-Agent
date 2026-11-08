# Fox UI 优化评审

日期：2026-11-01。范围：用户给出的 8 张截图和 9 项问题。输出：真实组件对比预览、模型配置流程演示、源码定位和后端待办。

## 版本与工作边界

- 本地仓库：`D:/python/projects/Fox/Fox`，分支 `main`，HEAD `5068da21d50907c8284ac5d91b9bd6e7096a5c01`。
- 用户指定的 `bb6f88a9` 已在本地。HEAD 比它多 6 个文档提交，本轮相关前后端实现未变化。
- 当前运行路径是旧 `MemoizedComposer` 输入区 + assistant-ui 消息列表。不能只改保留的 `thread/composer/fox-thread-composer.tsx`。
- 本次新增内容全部位于 `.ui-design-1101/`。产品源码未修改。原有 `.ui-review/` 和 `services/agent-runtime/.probe-s0/` 均保留。
- 真实组件预览复用现有评审夹具，并补齐实际知识库引用、工作台列宽等上下文。数据和提交边界为演示。
- 审查依据是用户提供的截图及当前源码。Fox 桌面未启动；浏览器预览检查不等于真实桌面验收。

## 使用方式

打开：<http://127.0.0.1:1461/.ui-design-1101/>。

左侧逐项选择；第 1–8 项可切换“当前实现 / 优化方案”，并展开原始截图。第 9 项“当前实现”显示源码中的流程说明，“优化方案”是可操作的模拟流程。

主题选择器支持石墨、雾蓝、暖纸；右上角可切换深浅色。所有模型、导入和审批操作均不连接真实后端。

服务停止后，从 `D:/python/projects/Fox/Fox/apps/desktop` 启动：

```powershell
node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 1461 --strictPort
```

## 逐项结论

| 编号 | 源码中的原因 | 方案 | 后端 |
| --- | --- | --- | --- |
| 1 底部留白 | 底栏 `padding: 4px 0 12px`，叠加行高及外框内距 | 收紧底栏，使用上下对称的 `space-xs`；保留输入区高度 | 不需要 |
| 2 Agent 菜单 | 名称与备注子节点使用 grid 两行 | 名称、弱化说明同排，选中标记固定在末端；长禁用说明单独处理 | 不需要 |
| 3 项目菜单 | CommandGroup 自带内距；添加项在 Command 外，另有 margin | 统一内缩与分隔线留白；搜索无结果时仍保留添加入口 | 不需要 |
| 4 取消与操作栏 | StoppedRun 的胶囊/居中样式独立于过程区；消息动作设为 `autohide="never"` | 取消行与过程区左对齐，撤掉胶囊；按每条消息的 hover/focus-within 显示动作，保留占位；触摸设备显示可发现入口 | 不需要 |
| 5 交付文件 | `declaredDescription` 被渲染在卡片下；文件组件已有但未完全使用主题 token | 仅保留文件图标、名称、大小和打开入口；聊天区隐藏说明，详情字段保留 | 不需要 |
| 6 引用与附件 | 图中是知识库 chip，硬编码 6px；文件附件另用 rounded-md | 依据外框内距计算内层圆角，两类附件均接主题 token；不是把小 chip 直接设成外框同一数值 | 圆角不需要；UUID 名称缺失另核对 |
| 7 审批 | decisionPending 用审批替换输入区；项目规则区在详情外，且缺对应样式 | 原审批组件移至对话流；常驻授权及其说明进入详情；命令、风险提示和本次允许/拒绝保留主区 | 不需要新增接口 |
| 8 设置 | SettingsGroup 标题分隔线 + 行分隔线，缺统一内容框 | 标题置于圆角边框外；内容框复用 card/radius/padding token；组内行保持轻分隔 | 不需要 |
| 9 模型服务 | 添加 Dialog、编辑 Sheet、模型再开 Dialog；导入与服务配置入口分散 | 模型服务与 Agent 来源分组；添加/编辑共用页内表单；预设驱动配置，协议放高级区 | 需要，见下表 |

项目菜单对应原附件图 4，取消状态对应原附件图 3；本文编号按用户文字问题排序。

## 源码定位与复用

- 输入区底栏：[composer.css:44](D:/python/projects/Fox/Fox/apps/desktop/src/styles/areas/composer.css:44)，[fox-composer.tsx:845](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/composer/fox-composer.tsx:845)。
- Agent 选择：[composer-agent-picker.tsx:31](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/composer/composer-agent-picker.tsx:31)，[workbench.css:4932](D:/python/projects/Fox/Fox/apps/desktop/src/styles/workbench.css:4932)。沿用 DropdownMenu。
- 项目选择：[composer-project-bar.tsx:28](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/composer/composer-project-bar.tsx:28)，[command.tsx:126](D:/python/projects/Fox/Fox/apps/desktop/src/components/ui/command.tsx:126)。沿用 Popover/Command。
- 取消状态：[error-state.tsx:10](D:/python/projects/Fox/Fox/apps/desktop/src/components/assistant-ui/elements/error-state.tsx:10)，[fox-elements.css:134](D:/python/projects/Fox/Fox/apps/desktop/src/components/assistant-ui/elements/fox-elements.css:134)。在 Fox 适配层调整，共同覆盖持久化运行和备用提示。
- 操作栏：[fox-assistant-message.tsx:92](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/thread/fox-assistant-message.tsx:92)，[fox-user-message.tsx:48](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/thread/fox-user-message.tsx:48)，[fox-thread.css:173](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/thread/fox-thread.css:173)。
- 交付说明：[runtime-artifacts.tsx:76](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/runtime-artifacts.tsx:76)，聊天渲染在同文件 99 行；字段来自 [present_files.rs:40](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/present_files.rs:40) 的模型交付声明，非前端读取文件后拼接。
- 引用 chip：[fox-composer.tsx:59](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/composer/fox-composer.tsx:59)，[workbench.css:5919](D:/python/projects/Fox/Fox/apps/desktop/src/styles/workbench.css:5919)。文件附件：[attachments.tsx:214](D:/python/projects/Fox/Fox/apps/desktop/src/components/ai-elements/attachments.tsx:214)。知识库名称已有多级回落，缺名称时最终显示 reference.id；圆角改动不会补齐缺失数据。
- 审批挂载：[fox-composer.tsx:612](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/composer/fox-composer.tsx:612)。项目规则：[RuntimeApprovalPrompt.tsx:155](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/components/RuntimeApprovalPrompt.tsx:155)。目标位置可放在 [fox-thread.tsx:93](D:/python/projects/Fox/Fox/apps/desktop/src/features/chat/thread/fox-thread.tsx:93) 消息之后、状态栏之前。保留原 `ApprovalRecord.id` 和 Host 提供的可用决策。
- 设置分组：[settings-layout.tsx:20](D:/python/projects/Fox/Fox/apps/desktop/src/features/settings/settings-layout.tsx:20)，[settings-layout.css:61](D:/python/projects/Fox/Fox/apps/desktop/src/features/settings/settings-layout.css:61)。使用记录独立 Card 在 [settings-data-page.tsx:320](D:/python/projects/Fox/Fox/apps/desktop/src/features/settings/settings-data-page.tsx:320)，统一分组后须避免双层边框。
- 服务入口：[settings-pages.tsx:730](D:/python/projects/Fox/Fox/apps/desktop/src/features/settings/settings-pages.tsx:730) 添加 Dialog、736 行编辑 Sheet；479 行嵌套模型弹窗。复用表单字段及 [provider-protocol-editor.tsx:54](D:/python/projects/Fox/Fox/apps/desktop/src/features/settings/provider-protocol-editor.tsx:54) 的高级能力。
- 基础 token：[globals.css:154](D:/python/projects/Fox/Fox/apps/desktop/src/styles/globals.css:154) 的间距、控件、字号；[themes.css:25](D:/python/projects/Fox/Fox/apps/desktop/src/styles/themes.css:25) 的主题圆角。继续使用当前 HarmonyOS Sans SC、Lucide 和现有组件，不增加 UI 库。
- 额外发现：[switch.tsx:13](D:/python/projects/Fox/Fox/apps/desktop/src/components/ui/switch.tsx:13) 的延迟加载 fallback 将原生 button 传给 Base UI，却未设 `nativeButton`，会产生一次控制台告警；加载后的正式实现已设为 true。本预览直接使用现有 `switch-implementation`，产品 wrapper 暂未修改，列为实装时的小修复。

## 第 9 项：推荐流程

1. 打开模型设置，先看到“模型服务”和“Agent 使用配置”。服务信息维护一次，Agent 行说明实际使用哪种来源。
2. 只读发现本机可运行的 Agent 与已有配置。明确区分已安装、本机有配置、未配置、检测失败、受管运行时不可用；配置目录存在不能直接证明程序已安装。
3. 已有配置：先显示来源和导入范围，再导入至 Fox 内该 Agent 的私有目录。订阅登录、API Key 和不同 Agent 的认证方式分别处理。
4. 已安装未配置：选择兼容的已有服务，或直接进入同一套添加服务表单。新建与编辑均使用页内编辑器，不再层叠弹窗。
5. 普通流程只填预设、服务地址、凭据和模型。预设提供可靠的协议入口；不兼容时给出具体原因和补充入口。协议类型不是普通用户的必答题。
6. 分开显示“已保存”“模型请求验证通过”“Agent 输出/工具验证通过”。保存不是连接成功，协议可匹配也不是工具可用。
7. 默认作用于 Fox 内运行的 Agent。若用户要同时配置系统中的外部 Agent，应使用明确的第二种作用范围，展示将修改的文件和回退方式。

## 后端待办

| 编号 | 已有基础 | 需要补齐 | 完成判据 |
| --- | --- | --- | --- |
| B1 安装发现 | `external_launch.rs:253/479` 查受管程序和显式路径 | 补系统已安装 CLI 发现；区分发现和可启动；探测失败不能归为未配置 | 系统已安装、仅残留目录、程序不可用和未安装分别得到正确状态 |
| B2 纯读取状态 | `external_model.rs:130–168` 已能解析/验证/生成配置 | 拆开只读状态、网络验证和应用；打开设置不顺带写配置 | 仅进入页面不发模型请求，不生成或改写 Agent 配置 |
| B3 独立绑定 | `external_model.rs:27/46` 会匹配协议但主要使用全局默认供应商 | 每个 Agent 可选来源、服务、模型；显式绑定与跟随默认区分 | 给 Codex 换服务，不改 Pi/dsh 的显式绑定 |
| B4 导入范围与入口 | `onboarding.rs:388` 已有 Codex/Pi/dsh 白名单导入、备份和私有 HOME；前端仅 Pi 暴露原生导入 | 统一导入预览、冲突、当前项目和新项目策略；补 Codex/dsh 界面 | 导入前可看来源和范围；保留本机原配置；切换来源后实际启动一致 |
| B5 兼容和验证回执 | 现有协议 profile、连接验证和系统 keyring | 预设维护不同 Agent 的真实入口和版本适配；区分模型验证与 Agent 验证 | 文本、工具、审批、取消、新会话和恢复分别得到证据 |

系统 Agent 文件写入属于另一种作用范围；若需要实现，应在方案确认后单独列任务。复用现有凭据库、私有目录和受管文件归属机制，不新建通用代理平台，不把接口地址机械替换视为协议兼容。

## 外部参考

- [CC Switch 添加供应商](https://github.com/farion1231/cc-switch/blob/main/docs/user-manual/zh/2-providers/2.1-add.md)：按目标应用选择预设，分步填写必需项；参考其任务顺序。
- [CC Switch 编辑供应商](https://github.com/farion1231/cc-switch/blob/main/docs/user-manual/zh/2-providers/2.3-edit.md)：添加和编辑均为页内编辑器，配置字段与作用范围可读。
- [Magpie Agent wiring](https://github.com/yetone/magpie/blob/main/docs/subsystems/agent-wiring.md)：用户已确认“magine”指 `yetone/magpie`，作为第 9 项的主要后端适配参考。重点是逐 Agent 的安装发现、配置路径与字段适配、当前配置来源、应用记录和断开恢复。
- Magpie 通过修改 Agent 原配置来接入其网关，并在断开时恢复原有值。Fox 已有私有 HOME：本轮建议复用逐 Agent 适配思路，先解决发现、导入与独立绑定；系统 Agent 配置写入和网关能力分别确定范围。
- CC Switch 主要参考预设选择与统一表单流程，Magpie 主要参考 Agent 配置适配。参考来源是此次读取的官方仓库 main 文档，后续实施时须按当时版本核对。

## 实施和验收顺序

先评审本页 1–8 的视觉与交互，确认后再落产品源码。第 9 项先明确来源与作用范围、完成 B1–B5，再接正式前端。

产品实现后做定向检查：三主题及深浅色；长名称/长路径/无项目；用户与助手消息 hover、键盘及触摸入口；附件和引用；审批只显示一次、跨会话不串票、失败保留、滚动和焦点；实际外部 Agent 的导入、启动、流式输出、工具调用、审批、取消与恢复。浏览器演示不能替代这些桌面验收。
