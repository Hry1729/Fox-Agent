# 右侧栏与对话页统一 token：实现提示词

> 交给实现 agent。只统一样式，不改布局、不改摘要页的结构。把侧栏并进对话卡片、摘要改成扁平列表，等原型（`合并版.html`）通过后另起任务。

## 背景

- 仓库：`D:\python\projects\Fox\Fox`，桌面端在 `apps/desktop`。React 19 + Tauri 2，样式是普通 CSS 加 shadcn 组件。
- 工作区里有大量**未提交**的右侧栏改版（`fox-right-panel.tsx`、`conversation-summary.tsx`、`right-panel.css`、`components/fox-ui/side-section.*`、`components/ui/item.tsx` 等）。在这些改动上继续做，**不要还原、不要丢弃**。
- token 定义在：
  - `apps/desktop/src/styles/globals.css`：`--fox-hover`、`--fox-border`、`--fox-card`、`--fox-muted`、`--fox-text`、`--fox-accent-soft`、`--fox-control-sm/md/lg`、`--fox-icon-size*`、`--fox-space-*`、`--fox-type-*`、`--fox-shadow-*`
  - `apps/desktop/src/styles/themes.css`：每个主题自己的 `--fox-radius-small/control/card/large`。石墨、暖纸、雾蓝三个主题的圆角都不一样。
- 对话页顶栏的两个按钮（`fox-chat-topbar.tsx:142-143`，CSS 在 `workbench.css:11686-11698` 的 `.fox-topbar-toggle`）是这次的**基准样式**：28px，圆角 `--fox-radius-control`，悬停 `--fox-hover`，选中 `--fox-accent-soft`。

## 现在的问题（逐条已核实）

### 1. 图标按钮：形状、尺寸、悬停、选中各不相同

| 位置 | 尺寸 | 圆角 | 悬停 | 选中 |
|---|---|---|---|---|
| 对话顶栏「对话摘要」「右侧边栏」`.fox-topbar-toggle`（workbench.css:11686） | 28×28 | `--fox-radius-control`（石墨 6px） | `--fox-hover` 底 | `--fox-accent-soft` 底 + accent 色 |
| 右侧栏右上角 ＋ / 全屏 / 关闭 `.fox-context-tab-actions > button`（workbench.css:6811-6848，right-panel.css:11 覆盖尺寸） | **28×30** | **14px，看起来是圆形** | 加 1px 边框 + `--fox-hover` | 「关闭面板」常驻边框 + 灰底（`.is-sidebar-open`） |
| 对话标题旁「…」`.fox-chat-title-more`（workbench.css:2355） | 25×25 | 写死 7px | 走 `.fox-icon-button` | — |
| 项目图标 `.fox-chat-project-trigger`（workbench.css:2255） | 25×25 | 写死 7px | `--fox-panel` 底 | 同悬停 |
| 通用 `.fox-icon-button`（workbench.css:981-1014） | 32×32 | `--fox-radius-control` | **加边框 + 白色卡片底** | 同悬停 |
| 侧栏标签的关闭 × `.fox-context-tab-close`（workbench.css:6919） | 20×20 | 写死 5px | 另一种混色 | — |
| 侧栏分组标题右侧的刷新 / 打开文件夹 `.fox-side-section-action button`（side-section.css:22） | 无固定尺寸，padding 2px | 0 | **没有悬停效果** | — |

实测（浅色石墨，320px 宽面板）：右上角按钮 `border-radius: 14px; 28×30`；悬停时多出 `0.8px solid` 边框。顶栏按钮没有边框。

键盘焦点也有两套：顶栏是 `outline 2px accent, offset +2px`，其他是 `outline 2px ring, offset -2px`。

### 2. 很多按钮和行悬停没有反应

实测悬停前后计算样式完全一样的：
- 摘要页分组标题右侧的「全部 ›」（`.fox-side-section-action button`）
- 摘要页的列表行 `.fox-side-row`：也**不能点**，只是展示
- 「显示更多」`.fox-side-more`（right-panel.css:85）
- 文件页「项目文件」「文件版本」标题右侧的刷新、打开文件夹图标按钮（`fox-right-panel.tsx:747-748`，没有 class）
- `fox-right-panel.tsx:445`「预览网页」、`:751`「重试」、`:752` 文件搜索结果、`:785` 关联证据按钮：都是没有 class 的 `<button>`，没有统一悬停

### 3. 悬停颜色至少有 5 种写法

- `var(--fox-hover)`：顶栏、侧栏右上角、协作助手列表
- `color-mix(in srgb, var(--foreground) 6%, transparent)`：managed-files.css:21/52/141、contact-side-card.css:296
- `color-mix(in srgb, var(--foreground) 5%, transparent)`：contact-side-card.css:260
- `color-mix(in srgb, var(--fox-canvas) 90%, var(--foreground))`：contact-side-card.css:74
- `color-mix(in oklch, var(--foreground) 4.5%, var(--rp-card))`：right-panel.css:51

### 4. 卡片：边框粗细、颜色、阴影不统一

- 右侧栏卡片 `.fox-side-card`（side-section.css:23）：边框用 `--rp-line`（`--border` 的 80%，比对话页的线浅），圆角写死 12px，**带阴影**。
- **深色模式的阴影是错的**：阴影颜色用了 `--foreground`，深色下 `--foreground` 接近白色，实测 `oklch(0.985 0 0 / 0.06) 0 1px 2px`，卡片下面是一圈发白的光。
- 其他带阴影的：当前标签 `.fox-context-tab.is-active`（right-panel.css:5、workbench.css:6888）、「打开 ▾」按钮 `.fox-file-open-trigger`（right-panel.css:50）、网页页分段控件当前项（right-panel.css:116，`--shadow-sm`）、小 Fox 侧栏外框（contact-side-card.css:16）和头像编辑按钮（:70）、侧栏面板外框（workbench.css:2170 `--fox-shadow-sm`）。
- 圆角写死且各不相同：8 / 9 / 10 / 12 / 14px（right-panel.css 第 4、6、19、23、39、45、50、54、60、66、87、88、91、94、101、121 行；contact-side-card.css 第 123、136、187、253、277、288、306、319 行）。切到暖纸主题（`--fox-radius-card` 16px、`--fox-radius-control` 10px）后，对话页跟着主题变，右侧栏不变，差别更明显。

### 5. 颜色变量有两套

右侧栏 CSS 直接用 shadcn 原始变量（`--border`、`--muted-foreground`、`--foreground`、`--card`），还有一套自己的 `--rp-*`（side-section.css:1-15）。对话页用的是 `--fox-*`。同一个「边框」「次要文字」在两边是不同的值。

### 6. 字号和图标大小写死

- right-panel.css、side-section.css 里写死 11px、11.5px、12px、12.5px、13px。对话页用 `--fox-type-*`，这些值乘了 `--fox-text-scale`。写死的字号**不跟随文字缩放设置**。
- 图标：顶栏 16px，右侧栏右上角 14px，行内小方块 15px。

## 统一规则（照这个改）

先在 `globals.css` 补一个缺的 token，亮色和暗色都要定义：

```css
--fox-selected: color-mix(in oklch, var(--foreground) 6%, transparent);
```

| 元素 | 规则 |
|---|---|
| 图标按钮（顶栏、侧栏右上角、所有卡片标题右侧） | 尺寸 `--fox-control-sm`（28px），行内紧凑版 24px；圆角 `--fox-radius-control`；静止时透明、`--fox-muted`；悬停 `--fox-hover` + `--fox-text`；选中 / 展开（`aria-pressed=true`、`data-state=on/open`、`aria-expanded=true`）用 `--fox-accent-soft` + `--fox-accent`；**任何状态都没有边框**；图标 16px（`--fox-icon-size`），24px 版用 14px；`transition: background 140ms ease, color 140ms ease` |
| 文字按钮（「全部 ›」「显示更多」「打开文件夹」「重试」「预览网页」等） | 高 24px（行内）或 28px（操作行）；左右内边距 8px；圆角 `--fox-radius-control`；`--fox-muted`；悬停 `--fox-hover` + `--fox-text` |
| 可点的列表行 | 悬停 `--fox-hover`，选中 `--fox-selected`，`cursor: pointer`。放在卡片里时整行铺满、圆角 0；不在卡片里时圆角 `--fox-radius-control` |
| 卡片（侧栏里所有的框：分组卡片、搜索框、地址栏、网页预览框、网络记录行、协作助手头部、文件信息和预览卡片、小 Fox 侧栏的各个框） | `border: 1px solid var(--fox-border)`；圆角 `--fox-radius-card`；底色 `--fox-card`；**`box-shadow: none`**；卡片内的分隔线也是 `1px var(--fox-border)` |
| 标签（侧栏顶部的页面标签、网页页分段控件） | 当前项用 `--fox-selected` 底 + `--fox-text`，不加边框和阴影；其他项悬停 `--fox-hover` |
| 阴影 | 只留给浮层：下拉菜单、弹出框、提示、对话输入框。侧栏和侧栏里的任何东西都不加阴影 |
| 键盘焦点 | 统一 `outline: 2px solid var(--ring); outline-offset: -2px`，顶栏按钮也改成这样 |
| 颜色 | 只用 `--fox-*`。`--rp-line` 换成 `--fox-border`，`--rp-card` 换成 `--fox-card`，行前的小方块不要底色。各种 `color-mix(... foreground N% ...)` 的悬停写法，全部换成 `--fox-hover` 或 `--fox-selected`。`--rp-bg` 先保留，合并布局时再处理 |
| 圆角 | 只用 `--fox-radius-small / control / card`，`--fox-radius-pill` 只给状态徽标和头像 |
| 字号 | 行标题 `--fox-type-ui`，说明和数值 `--fox-type-meta`，分组标题 `--fox-type-caption`，最小的提示 `--fox-type-micro`；不写死 px |

`.fox-icon-button`（workbench.css:981）是全局类，左侧栏也在用。把它的悬停从「边框 + 白底」改成上面的图标按钮规则，再截图核对左侧栏、联系人列表、设置等页面，不能出现别的地方变样。

建议把图标按钮的写法收成一处：一个 `FoxIconButton` 组件，或者一个共享 class。对话顶栏和右侧栏都用它，不要在 right-panel.css 里再单独覆盖尺寸和圆角。

## 不要做

- 不改布局：侧栏仍是独立一张卡片，并进对话卡片是下一个任务。
- 不改摘要页的结构和文案。
- 不改行为、不改 IPC、不动 Rust。
- 不升级依赖。

## 验收

1. **静态检查**：新增一个测试（参考 `apps/desktop/tests/sidebar-sibling-card.test.ts` 读 CSS 文本的写法），断言：
   - `side-section.css`、`right-panel.css` 里没有 `box-shadow`（`none` 除外）；
   - 这两个文件里没有 `--rp-line`；
   - 这两个文件里的 `border-radius` 只用 `var(--fox-radius-*)` 或 0。
2. **计算样式**：用 fixture `http://127.0.0.1:<port>/tests/fixtures/ui-refresh.html?right-panel&frame=summary&state=running&mode=light|dark`，读 `getComputedStyle` 核对：
   - 顶栏按钮和侧栏右上角按钮的尺寸、`border-radius`、悬停背景完全一样；
   - 悬停后背景会变的元素：「全部 ›」、每个分组标题的图标按钮、「显示更多」、文件搜索结果、协作助手列表；
   - 所有 `.fox-side-card` 的 `box-shadow` 都是 `none`，边框颜色等于 `--fox-border`。
3. **截图**：石墨、暖纸两个主题，各截浅色和深色，看这几页：摘要、文件、文件预览（`frame=file-preview&artifact=ready`）、网页、协作助手、协作助手消息记录、小 Fox 侧栏（`ui-refresh.html`，点头像打开），还有左侧栏。重点看深色模式下卡片下面不能再有发白的光。
4. **完整测试**（在仓库根目录）：`bun test ./apps/desktop/tests`，必须 0 失败。
5. **类型检查和构建**：本机 pnpm 调起的命令行找不到 `node`，所以手动跑这几步：
   ```powershell
   cd D:\python\projects\Fox\Fox\apps\desktop
   D:\ruanjian\Node\node.exe scripts/generate-profile-avatars.mjs
   D:\ruanjian\Node\node.exe node_modules\typescript\bin\tsc
   D:\ruanjian\Node\node.exe node_modules\vite\bin\vite.js build --config vite.config.ts
   D:\ruanjian\Node\node.exe scripts/check-bundle-budget.mjs
   D:\ruanjian\Node\node.exe scripts/check-build-isolation.mjs --mode=release
   ```
6. **换行符**：所有文件保持 LF。`sidebar-sibling-card.test.ts` 会按 LF 匹配 CSS 文本，写成 CRLF 会直接失败。
7. 不要提交、不要推送，改完列出改了哪些文件、每项验收的结果。
