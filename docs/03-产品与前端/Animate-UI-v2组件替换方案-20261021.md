# Fox Animate UI v2 组件替换方案

初稿日期：2026-10-21；文档修订：2026-10-23  
状态：方案；尚未安装依赖、替换组件或完成兼容性验证。  
原方案记录的确认范围：Code Block 暂不更换；其余清单组件纳入规划范围，不等于兼容性已通过。Page Transition 的效果组合仍为建议。  
本次修订范围：仅整理本文与配套需求文档，不安装依赖、不修改产品代码，不创建或切换分支。

## 1. 目标与边界

用 Animate UI v2 统一通用交互动效，减少旧 Animate UI 源码、Arc、Corr 与自写动画的重复维护。保留 Fox 的颜色、字体、尺寸、圆角、小狐形象、页面信息层级和业务状态。

“替换”指替换组件或动画实现，不代表删除其业务逻辑。输入框、会话、审批、通知、文件树和主题状态仍由 Fox 管理。下表中的使用位置区分已核实的源码入口与建议接入的位置；通用组件文件存在，不等于所有页面都正在使用。

明确排除：

- Code Block，包括现有代码高亮、流式代码展示和代码块内复制按钮，本轮保持原样。
- 整体替换 assistant-ui、AI Elements、Streamdown、文件预览器或 Agent 状态管理。
- 为统一品牌而更换所有图标、加入装饰背景或全局随机动画。
- 在本次方案阶段修改 main、测试 checkout、运行时包、数据库或验收证据。

## 2. 历史基线与实施前核对

以下为原方案在 2026-10-21 记录的基线。2026-10-23 文档整理仅复核主仓库仍为 `main@0607664`；未重新核对改造线、远端、依赖与业务验收。实施时重新记录实际基线。

本地主仓库：`D:/python/projects/Fox/Fox`，核对时为 `main@0607664`。旧 UI 提交 `59c31ff` 已在 main 的祖先链中。main 有未跟踪的文档和图片，必须保留。

四 Agent 改造线：`feat/four-agent-phase0-1`。原方案记录的验证聊天曾以 `a5e2c9d` 为目标，但本地 `output/four-agent-host-round6-a5e2c9d-20261021/repo` 在检查时已为 `5f2f2d83`。目录名不代表当前 HEAD。本文没有确认远端最新状态，也不把该提交视为已通过完整验收。

原方案记录：改造线相对当时的 main 涉及 20 个前端文件，包括 Sidebar、Composer、Workbench、会话状态与错误展示。Sidebar 增加了“等你处理”审批标记，后续替换必须保留。

实施建议：从重新核对后的四 Agent 改造提交创建独立 `ui/animate-ui-v2` 分支及 worktree。不要直接在验证 checkout 内开发。暂不从旧 UI 分支继续扩展，也不直接改 main。基础组件、侧边栏与导航分别提交，方便同步改造线和定位回归。

## 3. 依赖与接入方式

| 项目 | 2026-10-21 原方案记录，待实施前复核 | 实施要求 |
|---|---|---|
| Animate UI | 本地复制的旧版组件；未声明 `@animate-ui/react` | 引入 v2 包，逐项迁移本地实现 |
| Motion | 锁文件为 `12.42.2` | 原方案引用的 v2 更新记录要求 Motion 14，需复核目标版本的 peer dependencies；先验证 Arc、Corr 和自写 Motion 的兼容性 |
| React / React DOM | 锁文件为 `19.2.7` | 原方案引用的 Page Transition 要求 React 19.3+，需复核目标版本的实际 API 与依赖声明；单独验证依赖组合与桌面 WebView |
| Radix | 现有通用组件主要使用 `radix-ui` | 优先使用 `@animate-ui/react/primitives/radix-ui`，保留现有包装组件 API |
| Base UI | 旧包名 `@base-ui-components/react` | 新版文档使用 `@base-ui/react`；Switch、Progress 等需明确选择保留 Base 并迁移，或统一到 Radix |
| 动画样式 | 现有 CSS 与 Motion 并存 | 移除被新 primitive 接管的重复动画，保留布局、颜色和交互状态样式 |

官方命令 `npm install @animate-ui/react motion` 是依赖安装命令，不是 Sidebar 的组件名。实际实施遵循仓库 pnpm 和锁文件约定，固定经验证的版本。不得用强制忽略 peer dependency 冲突作为完成标准。

不要直接全库执行自动迁移后提交。先检查迁移差异；旧版专有部件、文件树、延迟加载包装和未迁移引用需要保留。确认没有使用者后，才能删除旧文件或依赖。

### 3.1 依赖验证与暂停条件

先选定目标版本，再核对包导出、peer dependencies、现有包装 API 和桌面 WebView 支持。官网的示例入口只用于定位资料，不保证 Radix 与 Base UI 版本具有相同属性或行为。

| 检查 | 通过条件 | 未通过时的处理 |
|---|---|---|
| 依赖解析 | 使用仓库包管理器安装固定版本，锁文件变更可解释，无忽略冲突的安装参数 | 暂停该依赖组合，不继续批量替换 |
| 包装 API | 逐项核对 ref、受控状态、组合渲染、Portal、挂载策略与事件语义 | 在 Fox 包装层适配；不能兼容时保留原实现并记录原因 |
| 最小接入 | 先选一个已有基础组件调用点，验证打开、关闭、键盘和焦点 | 修复适配问题后再扩展到其他调用点 |
| 旧动画共存 | Arc、Corr 与现有 Motion 关键路径可运行，无重复动画或事件 | 将问题限定到相关组件，不扩大迁移范围 |
| Page Transition | 目标 React API 与 WebView 可用，聊天状态保持通过 | 独立延后此项；不要为页面动画阻塞已经验证的基础组件迁移 |

每个调用点只保留一个动画责任方。迁移时记录新 primitive、Fox 包装层和业务组件各自负责的内容，避免 CSS、Motion 和新 primitive 同时控制同一属性。

## 4. 组件替换清单

以下按 2026-10-21 官网导航顺序排列，仅列本轮范围内的组件。基础组件链接进入官网默认 Base UI 页签；实施时按上表选择 Radix 或 Base 版本。

### 4.1 Components

| 新组件及效果页 | 现有组件／源码入口 | 放置位置与改动范围 |
|---|---|---|
| [Accordion](https://animate-ui.com/docs/primitives/base-ui/accordion) | `components/ui/accordion.tsx`；另有旧 `animate-ui/primitives/base/accordion.tsx` | 替换通用手风琴动画；已核实 AI Elements Agent 组件引用通用 Accordion。文件树使用的旧 Base 结构单独迁移，保留层级与选择逻辑 |
| [Alert Dialog](https://animate-ui.com/docs/primitives/base-ui/alert-dialog) | `components/ui/alert-dialog.tsx` | 用于现有确认弹窗调用处；保留确认、取消、危险操作文案及焦点返回 |
| [Collapsible](https://animate-ui.com/docs/primitives/base-ui/collapsible) | `components/ui/collapsible.tsx` | 用于现有内容折叠区。保留受控展开状态，不把业务折叠状态改成动画内部状态 |
| [Combobox](https://animate-ui.com/docs/primitives/base-ui/combobox) | `components/ui/command.tsx`、`features/chat/composer/composer-project-bar.tsx` | 首个目标是输入区的项目搜索选择器；保留项目筛选、空结果、当前项目和键盘选择。模型搜索选择器后续按相同边界评估。命令面板、斜杠命令仍保留 cmdk |
| [Context Menu](https://animate-ui.com/docs/primitives/base-ui/context-menu) | `components/ui/context-menu.tsx` | 升级已有右键菜单，包括侧边栏会话菜单。保留菜单动作、子菜单与禁用条件 |
| [Dialog](https://animate-ui.com/docs/primitives/base-ui/dialog) | `components/ui/dialog.tsx` | 升级通用模态窗口。保留 Fox 尺寸、滚动区、关闭规则和焦点约束；清理重复 CSS 进入／退出动画 |
| [Dropdown Menu](https://animate-ui.com/docs/primitives/base-ui/dropdown-menu) | `components/ui/dropdown-menu.tsx`；Arc `user-menu/user-menu.tsx` | 先升级已有更多菜单；再将侧边栏底部用户菜单的菜单行为与动效迁入，保留账户、主题、状态和退出等实际配置项 |
| [Hover Card](https://animate-ui.com/docs/primitives/base-ui/hover-card) | `components/ui/hover-card.tsx` | 用于引用、附件及上下文详情等既有悬浮卡片；已核实 `ai-elements/inline-citation.tsx`、`attachments.tsx`、`context.tsx` 引用 |
| [Popover](https://animate-ui.com/docs/primitives/base-ui/popover) | `components/ui/popover.tsx` | 升级现有锚点浮层；保留定位、边界避让、点击外部关闭与焦点返回 |
| [Progress](https://animate-ui.com/docs/primitives/base-ui/progress) | `components/ui/progress-implementation.tsx`；Corr `animated-progress.tsx` | 统一任务进度卡、通知进度、知识库处理进度、上下文预算及上传进度的基础实现。保留确定／不确定进度、错误和完成状态；Corr 上传逻辑保留 |
| [Select](https://animate-ui.com/docs/primitives/base-ui/select) | `components/ui/select.tsx` | 升级现有单选下拉框，不把带搜索的组合选择器强行改成 Select |
| [Sheet](https://animate-ui.com/docs/primitives/base-ui/sheet) | `components/ui/sheet.tsx` | 升级现有侧滑浮层。聊天右侧可拖拽常驻面板不自动改成模态 Sheet |
| [Sidebar](https://animate-ui.com/docs/primitives/radix-ui/sidebar) | `features/chat/shell/fox-sidebar.tsx`、`fox-sidebar-motion.tsx`、`fox-sidebar-motion-implementation.tsx`、`fox-sidebar.css` | 主侧边栏：用新 Sidebar primitive 承担菜单高亮和展开／收起动画；保留项目／会话分组、折叠入口、搜索、宽度记忆、拖拽、运行状态和审批标记。导航入口顺序和分割线保持既有关系 |
| [Slider](https://animate-ui.com/docs/primitives/base-ui/slider) | Arc `elastic-slider/elastic-slider.tsx`；`features/settings/settings-pages.tsx` | 替换设置页的 ElasticSlider；保留当前数值范围、步长、格式化、键盘调整和持久化 |
| [Switch](https://animate-ui.com/docs/primitives/base-ui/switch) | `components/ui/switch.tsx`、`switch-implementation.tsx`；旧 Animate UI Base Switch | 升级设置开关；保留受控状态、禁用、尺寸及延迟加载前后的一致性 |
| [Tabs](https://animate-ui.com/docs/primitives/base-ui/tabs) | `components/ui/tabs.tsx`；Arc `tabs/tabs.tsx`；`features/chat/shell/fox-sidebar-controls.tsx` | 先替换侧边栏模式切换，再统一通用 Tabs。右侧面板等业务标签需核对当前实现后接入；保留选中状态、键盘导航与面板挂载策略 |
| [Toast](https://animate-ui.com/docs/primitives/base-ui/toast) | Arc `toast-stack/toast-stack.tsx`；`features/notifications/notification-center-implementation.tsx`；`components/ui/sonner.tsx` | 替换通知中心的瞬时通知展示层。保留队列、去重、持续时间、操作按钮、错误提示和业务通知记录。核对实际使用者后决定是否移除 Sonner |
| [Toggle](https://animate-ui.com/docs/primitives/base-ui/toggle) | 旧 `animate-ui/components/radix/toggle.tsx`；`features/chat/shell/fox-chat-topbar.tsx` | 升级聊天顶栏现有切换按钮；保留 pressed 语义和业务状态 |
| [Tooltip](https://animate-ui.com/docs/primitives/base-ui/tooltip) | `components/ui/tooltip.tsx` | 升级图标按钮、折叠侧边栏等既有提示；保留延时、快捷键信息及键盘触发 |

### 4.2 Animate UI

| 新组件及效果页 | 现有入口 | 放置位置与改动范围 |
|---|---|---|
| [Page Transition](https://animate-ui.com/docs/primitives/animate-ui/page-transition) | `features/workspace/workspace-shell.tsx`；`features/chat/workbench.tsx` 中的页面调度 | 只包裹切换的内容区。一级页面建议 fade，列表／详情前进返回建议 slide；侧边栏、顶栏与仍在运行的聊天状态保持稳定。详细规则见第 5 节 |

Code Block 不在本轮范围内。

### 4.3 Buttons

| 新组件及效果页 | 现有入口 | 放置位置与改动范围 |
|---|---|---|
| [Button](https://animate-ui.com/docs/primitives/animate-ui/button) | `components/ui/button.tsx` | 给通用按钮接入一致的交互反馈，保留 variants、尺寸、asChild／组合行为与禁用状态；避免让全部按钮持续运动 |
| [Copy Button](https://animate-ui.com/docs/primitives/animate-ui/copy-button) | `features/chat/thread/fox-message-actions.tsx` 的复制确认逻辑 | 用于消息操作区的“复制→成功”反馈。保留原复制回调和失败反馈；本轮不改代码块里的复制按钮 |
| [Icon Button](https://animate-ui.com/docs/primitives/animate-ui/icon-button) | 现有图标按钮及 `components/fox-ui/animated-icons.tsx` 包装 | 先用于顶栏、侧边栏工具区的通用图标操作；保留按钮语义、Tooltip、焦点和点击区域。含特殊运行逻辑的按钮逐项接入 |
| [Theme Toggler Button](https://animate-ui.com/docs/primitives/animate-ui/theme-toggler-button) | `fox-sidebar.tsx` 中主题入口；Arc UserMenu 的主题项 | 用于既有主题切换入口，不新增重复入口。接入 Fox 的主题回调，不创建第二套主题存储 |

### 4.4 Texts / Numbers

| 新组件及效果页 | 现有入口 | 放置位置与改动范围 |
|---|---|---|
| [Shimmering Text](https://animate-ui.com/docs/primitives/animate-ui/shimmering-text) | `styles/workbench.css` 的处理中闪光文字；现有 tw-shimmer 样式 | 替换运行过程中的短状态文字效果。状态内容仍由运行数据驱动；不用于整段回复，也不接管流式文本输出 |
| [Counting Number](https://animate-ui.com/docs/primitives/animate-ui/counting-number) | `animate-ui/primitives/texts/counting-number.tsx`；旧 Base Progress | 升级现有进度百分比计数动画。若新 Progress 已提供相同效果，不再叠加一层 |
| [Scrolling Number](https://animate-ui.com/docs/primitives/animate-ui/scrolling-number) | Corr `animated-number.tsx`，当前依赖 NumberFlow | 用于 Corr 上传进度等既有数字显示位置，替换滚动数字实现；保留格式、小数、单位和读屏语义。没有新增统计卡片的计划 |

### 4.5 Effects / Icons

| 新组件及效果页 | 现有入口 | 放置位置与改动范围 |
|---|---|---|
| [Auto Height](https://animate-ui.com/docs/primitives/animate-ui/auto-height) | 尚未确认唯一的一对一替换文件 | 仅用于仍需自写高度动画的动态内容容器。实施时先列出具体调用点；Accordion／Collapsible 已处理的高度变化不再包一层。此项是条件接入项，不凭组件名称新增动画 |
| [Highlight](https://animate-ui.com/docs/primitives/animate-ui/highlight) | `animate-ui/primitives/effects/highlight.tsx`；旧 Base Files | 升级知识库文件树和聊天右侧文件树的选中高亮。使用入口为 `features/local-knowledge/library-sidebar-tree.tsx` 和 `features/chat/right-panel/fox-right-panel.tsx`；保留文件树结构和业务逻辑。Sidebar 若已使用内置高亮，则不重复接入 |
| [Lucide Icons](https://animate-ui.com/docs/icons/lucide-icons) | `components/animate-ui/icons/*`；`components/fox-ui/animated-icons-implementation.tsx` | 升级现有动画核心与对应图标文件，保留 Fox 图标包装 API、延迟加载和静态回退。继续使用现有 Lucide 图标含义，不整体换成 Hugeicons |

## 5. Page Transition 规则建议

| 导航类型 | 效果 | 例子 |
|---|---|---|
| 同级工作区页面 | fade | 知识库、插件、设置之间切换 |
| 列表进入详情 | slide，前进方向 | 知识库列表进入某个知识库详情 |
| 详情返回列表 | slide，返回方向 | 详情页返回原列表，并恢复原滚动位置 |
| 同页 Tabs | Tabs 自身动画 | 侧边栏模式或右侧面板标签切换 |
| 弹窗、侧滑层、菜单 | 对应组件自身动画 | Dialog、Sheet、Popover |
| 消息追加、流式 token、运行状态更新 | 不触发 Page Transition | 正在输出的对话、进度更新 |

每次导航只使用一种页面效果，不随机选择，也不叠加多种整页动画。首版不引入 shared element、stack、flow、chop 等额外效果。

Fox 当前按工作区状态调度页面，不能直接照搬官网路由器示例。接入时先核对实际 React API、页面 key 与 WebView 支持；不为动画单独更换整个路由架构。浏览器不支持或用户开启减少动态效果时，应正常完成导航，并使用无动画或适当减弱的效果。

聊天与其他页面切换时，应核对消息滚动位置、输入草稿、附件和正在运行的会话是否保留。不能为了播放进入动画而重新挂载整个聊天业务树。

## 6. 实施顺序与完成条件

| 批次 | 内容 | 完成条件 |
|---|---|---|
| 0 | 确认开发基线和独立 worktree；建立依赖升级提交 | 保存分支／HEAD／WIP 核对结果；React、Motion、Animate UI 组合可编译；旧动画库关键路径可运行 |
| 1 | Dialog、Alert Dialog、Dropdown Menu、Context Menu、Popover、Hover Card、Tooltip、Sheet、Select | 保留现有外观与行为，移除重复动画，键盘与焦点正确 |
| 2 | Accordion、Collapsible、Progress、Switch、Toggle、Slider、Button、Icon Button、Copy Button、主题入口 | 受控状态不丢失；禁用、错误、加载和延迟加载状态正确 |
| 3 | Sidebar、Tabs、文件树 Highlight、Lucide Icons | 保留导航关系、长列表滚动、宽度拖拽、审批标记；折叠展开和键盘操作正常 |
| 4 | Toast、Combobox、数字、状态文字与有明确调用点的 Auto Height | 业务回调、队列、搜索、数字格式及状态语义保持一致 |
| 5 | Page Transition | 经确认的 fade／slide 规则落地；内容区切换正常；聊天业务状态、焦点和滚动位置不丢失 |
| 6 | 清理重复实现 | 仅移除已无引用、已由新组件覆盖的文件与依赖；保留许可证要求；形成最终组件来源表 |

每批独立提交。前一批出现兼容问题时，先定位问题，不把后续迁移混入同一提交。无需为纯样式变化新增镜像实现的测试，但涉及状态、队列或焦点行为时应运行相关现有测试并补充必要验证。

### 6.1 与页面需求的衔接

配套需求见 [Fox 界面优化需求清单](1.md)。两份文档共用 UI 编号，便于分配工作和记录结果。

| 页面需求 | 相关组件 | 责任边界 |
|---|---|---|
| UI-01 历史阅读状态 | Sidebar、状态图标 | 阅读状态由业务数据决定，组件迁移不能清空未读状态 |
| UI-02 小狐遮挡 | Portal、浮层与层叠规则 | 先定位裁切和层叠问题，不把提高全局层级当作迁移结果 |
| UI-03 项目单行 | Sidebar、Tooltip | 单行截断由布局处理；完整名称提示复用通用组件 |
| UI-04 全类型文件可见 | 文件树、Highlight | 文件扫描与能力判断属于业务范围，动画替换不能解决漏文件 |
| UI-05 预览标题栏 | Button、Icon Button、Tooltip | 调整标题栏布局，保留预览器和原操作回调 |
| UI-06 搜索入口 | Icon Button、输入框或 Popover | 修复展开与搜索行为，统一图标外观只是其中一部分 |
| UI-07 文件视图 | 菜单、Tooltip、Highlight | 卡片方案需先选定；组件迁移不代替视图设计 |
| UI-08 关系图 | Button、Select、Popover | 工具栏可复用基础组件；图谱布局和右侧区域单独评估 |
| UI-09 设置页 | Tabs、Switch、Slider、Dialog | 信息架构、配置保存与权限语义独立核对 |
| UI-10 新对话输入区 | Combobox、Select 或 Dropdown Menu、Dialog | 项目、Agent 和专家分别绑定原业务状态；外框仅作用于新对话页 |

共享包装组件的修改可能影响历史页面。UI-10 的页面范围限制通过布局作用域落实；通用组件升级仍需检查其实际调用页面。两类修改分别记录，避免将全局外观变化混入新对话页需求。

### 6.2 提交与回退

每批提交说明目标组件、已迁移调用点、保留实现、检查结果和未验证项。依赖变更与页面布局变更尽量分开，便于定位问题。

出现输入丢失、焦点异常、审批入口不可达或运行状态错误时，暂停当前批次。回退必须同时考虑包装代码及其依赖和锁文件，不能只恢复一份组件文件。保留其他任务的未提交修改、测试环境和验收证据。

## 7. 验证与大型改造交界

共同检查：浅色／深色、常用窗口／小窗口、长列表、鼠标／键盘、焦点可见、退出后焦点返回、减少动态效果、快速重复点击和中途反向操作。

重点业务验证：

- Sidebar：项目与会话切换、折叠菜单、搜索、长列表滚动、宽度记忆、运行状态、“等你处理”标记。
- Composer 与 Combobox：项目选择、输入草稿、中文 IME、附件、发送和取消不受迁移影响。
- 四 Agent 接入：保留 AgentBackendBar、能力 chip、审批入口、后端错误详情及会话重开后的显示；UI 动画不改其数据来源。
- Toast：多条通知、持久通知、关闭、操作按钮、错误提示及历史记录不丢失。
- Page Transition：前进／返回、原滚动位置、无动画回退、流式消息不中断。

结果分别记录为源码检查、自动化测试、浏览器交互、真实桌面交互。编译通过不等于桌面动画通过；截图不等于键盘和状态保持通过。只运行与改动有关的检查，不以本次 UI 迁移为由重跑整个后端验收套件。

### 7.1 验证记录格式

每批使用以下字段记录结果。没有执行的检查填写“未验证”，不要填“通过”，也不要把零匹配测试当作通过。

| 字段 | 应记录的内容 |
|---|---|
| 基线 | worktree、分支、完整 HEAD、工作区修改范围 |
| 范围 | 批次、组件、调用页面及关联 UI 编号 |
| 源码检查 | API、状态来源、引用清理和重复动画检查结果 |
| 自动化测试 | 实际命令、匹配测试数、通过／失败数及日志位置 |
| 浏览器交互 | 环境、操作步骤、观察结果和证据位置 |
| 真实桌面交互 | 实际构建版本、WebView、输入／焦点／动画结果 |
| 用户验收 | 已确认的视觉选择及验收结论；未确认时保持待确认 |
| 遗留项 | 阻塞原因、影响范围和下一步 |

本次修订只完成文档整理。以上实现与验证项均未因文档更新而通过。

## 8. 资料与后续核对

- [Animate UI 总览与官网导航](https://animate-ui.com/docs)
- [npm 包、导入与 peer dependencies](https://animate-ui.com/docs/react)
- [v2 更新与迁移说明](https://animate-ui.com/docs/changelog)
- [Page Transition 效果与版本要求](https://animate-ui.com/docs/primitives/animate-ui/page-transition)
- 本地设计依据：同目录 `设计系统.md`。
- 本地依赖依据：`apps/desktop/package.json` 与 `pnpm-lock.yaml`。

本文源码路径除仓库绝对路径外，`components/`、`features/`、`styles/` 均相对 `apps/desktop/src/`。表内使用位置沿用初稿的源码核对记录，本次文档修订未重新验证调用关系；实施前须在目标 worktree 再次确认。官网版本、远端分支和验收结论可能继续变化，以实施时重新核对的结果为准。

