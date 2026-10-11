# assistant-ui Elements 来源与适配

这里保存 2026-10-11 从官方 [Elements registry](https://r.assistant-ui.com/) 获取的完整组件源码。Elements 使用 shadcn 源码分发方式，不是一个提供这些组件导出的 npm 二进制包。

- `elements/`：官方 registry 的组件及内部依赖。实际 Fox 代码调用这些导出。
- `utils/`：官方 range、布局 effect 工具。
- `tw-shimmer/`：npm `tw-shimmer@0.4.13` 的 CSS、包信息及 MIT 许可。`globals.css` 导入这份 CSS，由 Tailwind 编译官方 ShimmerLabel 动效。
- `LICENSE`：assistant-ui MIT 许可。
- `registry-sources.json`：逐项上游 URL、源码位置、依赖及原始 JSON SHA-256。原始下载快照留在工作区 `output/chat-ui-components/upstream/`，不作为产品构建输入。

## Fox 改动

| 上游文件 | 必要改动 |
| --- | --- |
| thinking-indicator | 用 span 嵌入头像右侧整行按钮；新增 active/showDot；状态点与折叠 icon 共用位置，hover/focus 切换；主题点色；减少动态效果模式。仍使用 ShimmerLabel。 |
| typing-indicator | 中文可访问名称。Fox 调用 bare 形态，尺寸限制在 32px 圆按钮内。 |
| streaming-text | 新增 inline/fresh/cursor；在 Markdown 文本叶子中渲染原双 span、take、鲜字入场和颜色过渡；保留中文与空格，不将 Markdown 展平；cursor 单独导出供全文尾部使用。鲜字通过 CSS 在 700ms 内收敛到主题正文色。 |
| composer | mention 匹配支持中文及路径，文件具有 file 类型；角色及移除名称中文化。Fox 使用官方 standalone Menu/CommandItem/PersonItem/ModelTrigger/ModelItem，cmdk 管理真实候选和键盘选择。按用户最新要求，输入附件/项目/知识库 chips 已恢复 HEAD 的 Fox 原 Attachments inline + Attachment/Preview/Info/Remove 结构；官方附件导出仍留在来源快照中，当前输入区不调用它们。 |
| tool-call | 请求、结果、目标允许 ReactNode，以保留原有结构化工具数据和 Host 文件操作；增加工具图标与真实错误状态；请求/结果中文化。仍由上游折叠、标签切换与详情结构渲染。 |
| tool-timeline | 步骤允许承载实际 ToolCall 或 reasoning；增加 Fox 图标及有界滚动容器；首开挂载、访问后保留内容状态；公开 aria-hidden；保留既有过程标题。仍由上游 take(steps, visibleSteps)、header、折叠及逐项映射渲染。 |
| run-activity | 可注入 Fox 头像/状态/计时整行 header；传入 ref 和焦点事件；闭合时保留已挂载 entry。上游负责中途回复/工具 entries、折叠、状态通知；最终正文与待处理事项在折叠外。 |
| approval-card | 增加 busy、detailsContent、onAllowConversation；中文化。按钮按拒绝、会话允许、一次允许排序，一次允许是最右唯一主按钮。Fox 展示短标题/意图、单次精确目标或命令灰条；普通授权说明归详情，覆盖/绑定影响仍常显，完整内容与校验可展开。宽度沿用原容器，未强制限宽。onAllowConversation 对接真实 allow_conversation，未将上游 onAlwaysAllow 映射为它。 |
| permission-grant | 中文范围标签；新增 allowedScopes、busy、description、detailsContent 及 once/conversation 范围。Fox 只对带路径的只读 read_file 请求使用独立 Grant；onGrant 经过同一允许决策交集，并提交原票据。默认显示简要范围，完整范围可展开。普通写入/命令/替换/Office/修复仍用 ApprovalCard，两者互斥。Fox 不传 session/always，不将 Always 映射成一次授权。 |
| feedback-dialog | 中文化、busy、1000 字上限；singleSelection 对接 Fox 单分类字段，并提供 radio 键盘操作。Dialog 外壳由 Fox 提供，保存仍调用 Host 原接口。 |
| edit-message | 中文化、busy、自动聚焦及空文本禁用；仍展示 discardedReplies。Fox 适配器保留 IME 和快捷键，并接原重发路径。 |
| message-actions | 中文可访问名称；可选真实重新生成；更多操作绑定分支对话；运行中禁用；不提供 Host 不支持的清除反馈操作。 |
| error-state | 中文化；onRetry 可选，诊断详情可折叠。重试仅绑定当前轮真实回调。 |
| stopped-run | 中文化；未提供继续/丢弃回调时不出现空操作；Fox 已生成正文仍由消息正文呈现，不重复输出。 |
| surfaces | inkButton 添加共享样式类，与 Fox 的主题前景/背景规则兼容。 |
| file | 未改动上游组件逻辑。Fox 按默认 outline 行使用 Root/Icon/Name/Size：左 type 图标、中间文件名与大小两行、8px 圆角。名称列 minmax(0,1fr) 并省略长名，title 保留全文。右侧仅在真实 Host preview 回调存在时提供打开图标，未将 Windows 路径伪装成下载 URL。 |

上游 demo 的计时推进、固定词数、假工具结果、假审批状态均未移入产品。真实 Run、审批身份、允许的决策集合、文件身份和发送回调继续由 Fox 提供。
