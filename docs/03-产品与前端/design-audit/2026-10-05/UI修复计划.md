# Fox 桌面端 UI 修复计划（Refactoring UI 诊断落地）

- 日期：2026-10-05
- 基线提交：`b66d54e6`（main）
- 依据：
  1. 项目冻结的设计规范 `docs/03-产品与前端/设计系统.md`（以下简称「设计系统」）。冲突时以它为准。
  2. Refactoring UI skill（https://github.com/s0xDk/refactoring-ui-skill）的规则：固定刻度、用字重和颜色建立层级、弱化竞争元素、少用边框和阴影、文字对比度 4.5:1、非文字控件边界 3:1、不要在彩色背景上放灰字。
- 本计划只改样式和少量展示组件，不改业务逻辑、数据流和 Rust 端。

---

## 0. 给执行 agent 的规则（必须先读）

### 0.1 不要碰的文件

下列文件有用户未提交的改动。开工前先运行 `git status --short`，以实际结果为准。如果计划某一条要改这些文件，**跳过这一条并在报告里列出**，不要改、不要 stash、不要 revert：

```
apps/desktop/src/components/ai-elements/prompt-input.tsx
apps/desktop/src/components/ai-elements/prompt-send-shortcut.ts
apps/desktop/src/features/chat/turn-process-header.tsx
apps/desktop/src/features/chat/tool-attempt-status.ts（未跟踪）
apps/desktop/src/features/conversations/hooks/*.ts
apps/desktop/src/features/settings/settings-pages.tsx
apps/desktop/src-tauri/**（本计划本来就不涉及）
apps/desktop/tests/*（被修改的那几个）
```

### 0.2 工作方式

1. 按阶段顺序做。每个阶段单独提交，提交信息说明阶段编号，方便单独回滚。
2. **禁止全局查找替换**数值（例如把所有 `10px` 换成 `8px`）。每处改动都要知道它属于哪个选择器、是否还在使用、是否被后面的规则覆盖。
3. 改字号或间距前，先检查所在容器有没有固定高度（见 3.1 的清单）。字变大但容器不变高，会导致裁切或溢出。
4. 每个阶段结束时用同一组页面重新截图，和第 0 阶段的基线对比。
5. 「渲染中性」的阶段（第 2 阶段）要求截图**没有可见差异**。有差异就说明某条规则判断错了，回退那一条。
6. 写注释只解释不明显的原因（例如“静态字体只有 400/500/600/700 四档”），不要写“按 UI 修复计划修改”这类注释。

### 0.3 每个阶段的验证命令

```powershell
pnpm --dir apps/desktop build          # tsc + vite build + 包体预算 + 构建隔离检查
bun test ./apps/desktop/tests          # 桌面端单元/DOM 测试
pnpm --dir apps/desktop lint:css       # 第 0 阶段新增的 stylelint 报告
```

`build` 失败或测试失败时先修复再进入下一阶段。现有测试基本不断言 CSS 值（只有 `tests/fixtures/knowledge-picker.tsx` 引入了样式），所以**测试通过不等于视觉正确**，截图对比是唯一的视觉验证手段。

### 0.4 需要用户拍板的事项（遇到时停下来问，不要自己决定）

| 编号 | 问题 | 现状 | 建议 |
|---|---|---|---|
| D1 | 卡片标题用 14px 还是 16px | 设计系统 §4.1（`--fox-type-ui: 14px` 用于卡片标题）和 §6.5（“卡片标题 14px”）写的是 14px；但 `globals.css:141` 定义了 `--fox-type-card-title: 16px`，并且被 `workbench.css`、`workspace-pages.css`、`local-knowledge.css` 使用 | 跟随文档：列表卡片标题 14px / 600；`--fox-type-card-title` 改成 14px 或删掉。改之前先确认 |
| D2 | 输入框外壳（composer）是否保留浮起阴影 | 设计系统 §4.6 写“输入框…不得使用大面积发光或多层阴影”；但 `workbench.css:5385` 的 `--fox-composer-elevation` 是三层阴影加 `backdrop-filter: blur(24px)`，代码注释说明这是刻意设计 | 保留浮起感，但减到一层弱阴影（理由见 3.3）。需要用户确认 |
| D3 | 设置页隐藏的分区描述 | `workspace-pages.css:2977` 用 `.fox-settings-section-head>div>p{display:none!important}` 把所有分区描述隐藏了 | 不在本计划处理，只记录 |

---

## 第 0 阶段：建立基线（不改任何界面）

### 0-1 截取基线截图

**为什么**：项目没有视觉回归测试。没有基线，就无法证明第 2 阶段“没有可见变化”，也无法让用户审阅第 3 阶段的视觉变化。

**怎么做**：

1. 启动带调试端口的真实应用（纯浏览器打开 Vite 页面时 `desktopRuntimeAvailable` 为 false，很多页面没有数据）：
   ```powershell
   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9335'; pnpm --dir apps/desktop tauri dev
   ```
   用后台方式运行，然后通过 CDP（端口 9335）连接 WebView2 截图。
2. 对话时间线还可以用合成夹具截图：`pnpm --dir apps/desktop dev:profile` 后打开 `http://127.0.0.1:1422/?foxPerf=1`（只渲染夹具，不写用户数据，见 `src/features/profiling/README.md`）。
3. 截图矩阵：亮色和暗色 × 1280×800 和 1440×900。
4. 页面清单（每个页面固定同一份数据和同一滚动位置）：
   - 新对话空状态（composer 未聚焦、聚焦）
   - 一段包含工具调用、任务清单（TaskItem / GoalProgress）、证据列表（EvidenceList）的对话
   - 专家列表、专家详情、专家选择对话框
   - 数字同事管理
   - 本地知识库列表、本地知识库详情（含切块列表、PDF 搜索结果）
   - 知识库（远程服务）页面，包括未配置服务的空状态
   - 插件中心
   - 设置中心的每个分页
5. 保存到 `docs/03-产品与前端/design-audit/2026-10-05/before/`，命名沿用上次审计的格式：`01-chat-light-1280.png`。

**验收**：所有页面 × 主题 × 尺寸都有截图；记录下每张图使用的数据和操作步骤，后续阶段按同样步骤复拍。

### 0-2 引入 stylelint，只报告不阻断

**为什么**：之前的违规数量是用正则粗略统计的（例如小于 11.5px 的字号约 321 处，`!important` 约 722 处）。`workspace-pages.css` 第 3–33 行和 3142–3150 行是压缩成一行、单行最长 4500 字符的 CSS，正则统计不可靠，也没法在 diff 里审阅。需要一个准确、可重复的违规清单，后续每个阶段都用它衡量进度。

**改哪里**：

- `apps/desktop/package.json`：devDependencies 加 `stylelint`，scripts 加 `"lint:css": "stylelint \"src/**/*.css\""`。
- 新建 `apps/desktop/.stylelintrc.json`。

**规则建议**（全部设为 `warning`，暂不阻断构建）：

| 规则 | 配置 | 对应问题 |
|---|---|---|
| `declaration-property-value-disallowed-list` | `font-size`: `/^(7\|7\.5\|8\|8\.5\|9\|9\.5\|10\|10\.5\|11)px$/` | 低于设计系统最小字号 11.5px |
| 同上 | `font-weight`: `/^(300\|550\|650\|680\|750\|800)$/` | 字体只有四档静态字重 |
| 同上 | `border-radius`: `/^(5\|7\|9\|11\|13\|14)px$/` | 设计系统 §4.4 明确禁止的圆角 |
| `declaration-no-important` | `true` | `!important` 覆盖链 |
| `color-no-hex` | `true`，`globals.css` 用 `overrides` 关闭 | token 之外的裸色值 |

**验收**：`pnpm --dir apps/desktop lint:css` 能跑出完整报告；把报告的统计数字（按规则、按文件）写进本目录的 `lint-baseline.md`，作为后续对比的起点。

---

## 第 1 阶段：修 token（全局生效，影响面最大，最先做）

修 token 的好处是一处改动覆盖所有使用点，不需要逐个页面修。这一阶段**会产生可见变化**（文字变深、边框变深），完成后要给用户看前后对比图。

所有对比度用 WCAG 2.x 相对亮度公式计算。目标：正文和小字 ≥ 4.5:1；输入框、复选框、开关等控件的边界 ≥ 3:1。选色时在目标之上留 0.2 左右余量，避免显示器差异导致不达标。

### 1-1 `--fox-faint` 改成不透明且达标的颜色

**为什么**：`--fox-faint` 是元数据和弱提示文字的颜色（设计系统 §4.5），在代码里被使用约 205 次。亮色主题下它是 `color-mix(in oklch, var(--muted-foreground) 72%, transparent)`，即 72% 透明度的灰。实测在白色卡片上对比度只有 **2.75:1**，远低于 4.5:1。另外，半透明文字的实际颜色取决于下面的背景，同一个 token 在白卡片、灰面板、蓝色选中态上会呈现不同的对比度，无法保证可读。

Refactoring UI 的做法是：弱化文字用一个**固定的、不透明的**浅一级灰，而不是用透明度。

**改哪里**：`apps/desktop/src/styles/globals.css`
- 第 74 行（亮色）：`--fox-faint: color-mix(in oklch, var(--muted-foreground) 72%, transparent);`
- 第 181 行（暗色）：`--fox-faint: color-mix(in oklch, var(--muted-foreground) 75%, transparent);`

**怎么做**：

1. 亮色：改成不透明的 `oklch(L 0.018 213.5)`，从 L=0.60 往下调，直到在 `--fox-card`（白）和 `--fox-panel`（`--muted`，`oklch(0.963 0.002 197.1)`）上都 ≥ 4.6:1。
2. 如果算出来和 `--muted-foreground`（`oklch(0.56 0.021 213.5)`，第 244 行）几乎一样，就直接 `--fox-faint: var(--fox-muted)`。层级差异改由字号和字重来表达（Refactoring UI：层级主要靠字重和颜色两档，不需要三档灰）。
3. 暗色当前实测约 4.59:1，刚好达标但余量太小，也改成不透明色，目标 ≥ 4.8:1（在 `--card` `oklch(0.218 0.008 223.9)` 和 `--muted` `oklch(0.275 0.011 216.9)` 上都测）。
4. 不要改 `--fox-thinking-text`（第 71、178 行），它有独立的设计意图（见第 67–70 行注释）。

**验收**：两个主题下 `--fox-faint` 在卡片和面板背景上都 ≥ 4.5:1；截图中元数据文字明显可读，但仍比正文浅。

### 1-2 新增状态文字 token，补齐 warning 系列

**为什么**：`--fox-success: #20845d` 和 `--fox-warning: #a66a14`（`globals.css:77-78`）同时被用作圆点、图标和文字颜色。用作文字放在自己 8% 的浅色底上时，实测分别是 **4.19:1** 和 **4.05:1**，不达标。另外 success 和 danger 都有 `-border`、`-surface` token（第 87–91 行），warning 却没有，导致各个页面各自写了一套黄色（见 3.4）。

**改哪里**：`apps/desktop/src/styles/globals.css` 的 `:root`（第 47–151 行）和 `.dark`（第 161–189 行）。

**怎么做**：

1. 保留 `--fox-success` / `--fox-warning` 作为圆点、图标、边框的颜色。
2. 新增 `--fox-success-text`、`--fox-warning-text`：同色相加深，要求在对应 `-surface` 上 ≥ 4.6:1。暗色主题同理（暗色下是在深底上，需要比圆点色更亮）。
3. 新增 `--fox-warning-border`、`--fox-warning-surface`，写法和第 90–91 行的 success 一致。
4. 本阶段只新增 token，不替换使用点。替换在 3.4 做。

**验收**：新 token 在两个主题下都满足对比度；`build` 通过。

### 1-3 主色留出对比度余量，新增强调文字 token

**为什么**：

- 主按钮是白字放在 `--primary: #3978c5` 上，实测 **4.50:1**，刚好压线，任何抗锯齿或显示差异都可能不达标。亮色和暗色主题的 `--primary` 都是这个值。
- 强调色文字（`--fox-accent`，即 `--fox-blue`）放在 `--fox-accent-soft: #e8f4ff` 浅蓝底上（选中态、标签、提示条），实测 **4.03:1**，不达标。

**改哪里**：`apps/desktop/src/styles/globals.css`
- 第 59 行 `--fox-blue: #3978c5`、第 239 行 `--primary`、第 259 行 `--sidebar-primary`
- 暗色第 273 行 `--primary`、第 292 行 `--sidebar-primary`（暗色的 `--fox-blue` 是 `#78aee8`，单独评估）

**怎么做**：

1. 把 `#3978c5` 稍微加深（保持色相），使白字对比度 ≥ 4.8:1。三个位置用同一个值。改完对比按钮截图，确认品牌色感觉没变。
2. 新增 `--fox-accent-text`：用于浅蓝底上的蓝色文字，要求在 `--fox-accent-soft` 上 ≥ 4.6:1。暗色主题在 `#233b53` 上测。
3. 本阶段不替换使用点，在第 3 阶段按页面替换。

**验收**：白字在主色上 ≥ 4.8:1；`--fox-accent-text` 在浅蓝底上 ≥ 4.5:1。

### 1-4 新增控件边框 token

**为什么**：输入框、下拉框、复选框的边框现在用的是 `--border` / `--input`（`oklch(0.925 0.005 214.3)`，`globals.css:248-249`），在白底上只有 **1.25:1**。WCAG 1.4.11 要求“识别控件所需的边界”至少 3:1。Refactoring UI 也把这条列为硬规则。

但卡片、分隔线等装饰性边框**不需要**达到 3:1，而且把它们加深会让界面变得很“框”（Refactoring UI 的另一条规则：少用边框）。所以要拆成两个 token，只加深控件边框。

**改哪里**：

- `globals.css`：新增 `--fox-control-border`（亮色在白底上 ≥ 3:1；暗色在 `--card` 上 ≥ 3:1）。暗色 `--input` 当前是 `oklch(1 0 0 / 15%)`（第 283 行），一起评估。
- `apps/desktop/src/components/ui/` 下的控件：`input.tsx`、`textarea.tsx`、`select.tsx`（trigger）、`input-group.tsx`、`switch.tsx`（关闭状态的轨道）。复选框如果有自定义样式也要处理。
- **不改** composer 外壳（`.fox-prompt-input`，在 `workbench.css`），它的边框有专门的激活设计（`workbench.css:5404-5409`）。
- **不改** `prompt-input.tsx`（有未提交改动）。

**怎么做**：在上述组件里把边框色从 `border-input` 换成新 token（例如 `border-[var(--fox-control-border)]`），或者在 `@theme inline` 里注册 `--color-control-border` 后用 `border-control-border`。先用一个组件试，截图确认效果后再推广。

**验收**：设置页和表单对话框里的输入框边框清晰可辨；卡片和分隔线的颜色不变。

### 1-5 统一字号 token 的下限

**为什么**：设计系统规定的最小字号是 Caption 11.5px，但 `globals.css:135` 还有一个 `--fox-type-card-stat: 10.5px`，等于在 token 层面允许了低于下限的字号。

**改哪里**：`globals.css:135`。

**怎么做**：改为 `var(--fox-type-caption)`。使用它的文件有 `workbench.css`、`workspace-pages.css`、`local-knowledge.css`，改完检查这些位置的固定高度容器（卡片统计行）是否溢出。D1 拍板后再处理 `--fox-type-card-title`。

**验收**：截图中卡片统计数字不溢出、不被裁切。

---

## 第 2 阶段：渲染中性的清理（截图必须无可见变化）

这一阶段的目的是降低后续修改的风险：先把代码整理成能看懂、能审阅的样子，再动视觉。每一条完成后都要截图对比，**出现可见差异就回退那一条**。

### 2-1 把压缩成一行的 CSS 展开（单独一个提交）

**为什么**：`workspace-pages.css` 第 3、5–10、23–28、33、82、3142–3150 行是压缩格式，单行 600–4500 字符，里面包含了大量 7–10.5px 的字号和裸色值。不展开就无法逐条审阅，也无法让 stylelint 给出准确的行号。

**改哪里**：`apps/desktop/src/styles/workspace-pages.css` 上述行。

**怎么做**：只做格式化（每个声明一行），**一个字符的值都不改**。用 prettier 或 stylelint 的格式化能力处理这些行，或者手工展开。提交里只能有空白和换行的变化，用 `git diff -w --stat` 确认去掉空白差异后没有内容变化。

**验收**：`git diff -w` 为空（或只有换行）；截图无变化。

### 2-2 字重按实际渲染值归一

**为什么**：字体是 HarmonyOS Sans SC 的四个静态字重文件（`globals.css:4-7`：Regular 400、Medium 500、Semibold 600、Bold 700）。CSS 里写的 550、650、680、800 并不存在，浏览器会按字体匹配算法挑一个现有字重。写一个不存在的值，读代码的人会以为有第五档字重。

**改哪里**：stylelint 报告中所有 `font-weight` 违规（`workbench.css` 约 27 处，`workspace-pages.css` 约 29 处，`local-knowledge.css` 约 8 处）。

**怎么做**：按 CSS 字体匹配规则映射到**实际渲染的值**，保证画面不变：

| 写的值 | 实际渲染 | 改成 |
|---|---|---|
| 300 | 400（低于 400 时先向下找，找不到再向上） | `400` |
| 550 | 600（高于 500 时先向上找） | `600` |
| 650、680 | 700 | `700` |
| 750、800 | 700（向上没有，再向下） | `700` |

优先改成 `var(--fox-weight-*)` token（`globals.css:145-148`）。注意：这一步**故意保留**现在偏重的 700，是否改成 600 在第 3 阶段按页面决定。

**验收**：stylelint 的 font-weight 违规数为 0；截图无变化。

### 2-3 删除确认无用的 CSS

**为什么**：压缩块里有整组选择器在 TSX/TS 中找不到任何引用。它们贡献了大量违规（例如 `.fox-pipeline small{font-size:7px}`），删掉可以直接减少噪音，避免执行 agent 浪费时间修死代码。

**改哪里**：`workspace-pages.css` 中以下类名开头的规则（在 `src/**/*.{ts,tsx}` 中搜索结果为 0）：

```
fox-onboarding-page  fox-entity-main  fox-settings-account  fox-page-metrics
fox-page-topbar-main  fox-architecture-row  fox-pipeline
```

**怎么做**：

1. 对每个类名，在 `apps/desktop/src` 下用 `rg` 搜索完整类名**和**它的前缀片段（例如 `fox-entity-`、`` `fox-${ ``），排除通过模板字符串动态拼接的可能。
2. 也搜索 `apps/desktop/index.html` 和 `tests/fixtures`。
3. 确认没有引用后再删除。有任何疑问就保留并在报告中列出。
4. 用同样方法扫描其他候选（stylelint 报告里违规集中的选择器），只删能证明无用的。

**验收**：`build` 通过；所有页面截图无变化。

### 2-4 合并被覆盖的声明，减少 `!important`

**为什么**：同一个选择器在文件里被定义多次，后面再用 `!important` 覆盖前面的值。例如：

- `.fox-settings-section-head p`：第 26 行写 8px，第 2976 行用 `12px!important` 覆盖，第 3286 行又定义一次。
- `.fox-shadcn-kb-heading small`：在第 10、12、64、191、197、208、4412 行出现了 7 次。
- `.fox-setting-row b`：第 26 行和第 2980 行。

这种“打补丁”的写法让人无法判断最终生效的值，也是 `!important` 多达 700 多处的主要原因。

**改哪里**：`workspace-pages.css`（约 399 处 `!important`）为主，其次 `workbench.css`（约 191 处）。

**怎么做**：

1. 对每组重复选择器，在浏览器开发者工具里查看**计算后的值**（computed style）。
2. 保留一条规则，写入最终生效的值，删除其余重复声明和不再需要的 `!important`。
3. 注意 `!important` 有时是为了压过 shadcn 组件或 Tailwind 工具类（例如 `.fox-library-card`，`globals.css:153-159`），这类先保留，在报告里列出。
4. 分批进行，每批改完截图对比。

**验收**：截图无变化；`!important` 数量下降（记录前后数字）。

### 2-5 颜色值改成已有 token（值完全相同的部分）

**为什么**：有些裸色值和 token 完全一样，只是没有引用 token。这样暗色主题无法生效，以后改 token 也改不到它们。

**改哪里**：

- `apps/desktop/src/components/ai-elements/file-tree.tsx:176` 和 `:263`：`bg-[#e8f4ff] dark:bg-[#233b53]` 正好等于 `--fox-blue-soft` 的亮/暗值 → `bg-[var(--fox-accent-soft)]`。
- `workspace-pages.css:3416`、`:3725` 的 `#e8f4ff`，`workbench.css:4768` 的 `#e8f4ff` → `var(--fox-accent-soft)`。注意：这几处现在在暗色主题下**也是浅蓝**，换成 token 后暗色会变成深蓝，这是修复不是回归，但会造成暗色截图变化，要在报告里说明。
- `workbench.css:4683`、`:4860`、`:4994` 里 `color-mix(..., #e8f4ff ...)` 同理。

**验收**：亮色截图无变化；暗色截图中这些位置从浅蓝变成深蓝（预期变化，单独列出）。

### 2-6 不规范圆角归到刻度上

**为什么**：设计系统 §4.4 规定圆角只有 6 / 8 / 10 / 12 / 16 / 999px，并明确写了“禁止新增 7、9、11、13、14px”。当前约 110 处使用了这些值（`workbench.css` 约 64、`workspace-pages.css` 约 37、`local-knowledge.css` 约 9）。

**怎么做**：5→6，7→8，9→ 控件用 8、卡片用 10，11/13/14→12。用 `var(--fox-radius-*)` token。1px 的圆角差异肉眼几乎看不出，归为“近似中性”。

**验收**：stylelint 圆角违规为 0；截图只有像素级差异。

---

## 第 3 阶段：按页面修视觉（会有可见变化，每页单独提交并给用户看）

### 3.1 通用方法：先删减，再放大

Refactoring UI 的原则：信息太挤时，**先问这条信息是否需要显示**，而不是把所有字都放大。直接把 8px 放大到 12px，在固定高度的卡片里一定会溢出。

每处小字的处理顺序：

1. 这条信息用户需要一眼看到吗？不需要 → 删掉，或移到 tooltip、详情页。
2. 需要 → 放大到至少 `--fox-type-caption`（11.5px），并用颜色（`--fox-muted`/`--fox-faint`）而不是字号来弱化。
3. 检查所在容器的固定尺寸，必要时调整：

| token / 选择器 | 位置 | 固定尺寸 |
|---|---|---|
| `--fox-library-card-height` | `globals.css:111` | 126px |
| `--fox-library-card-header-height` / `-content-height` | `globals.css:109-110` | 56px / 68px |
| `--fox-badge-height`、`--fox-library-card-tag-height` | `globals.css:102-103` | 20px |
| `.fox-shadcn-kb-footer` | `workspace-pages.css:10`（展开后行号会变） | 42px |
| `.fox-shadcn-kb-heading` | 同上 | `grid-template-rows: 20px 34px` |
| `.fox-page-toolbar` | `workspace-pages.css:5` | 46px |

真实在用、确实过小的例子（已确认没有被后面覆盖）：`.fox-page-intro p` 10.5px（第 3 行，2 个 TSX 文件在用）、`.fox-page-toolbar button` 9px 和其中的 `.badge` 7px（第 5 行，2 个 TSX 文件在用）。其他小字以 stylelint 报告为准，并逐条确认是否被覆盖。

### 3-1 对话工作台（优先级最高，用户停留时间最长）

**为什么**：一轮对话里同时出现 7 种以上字号，层级靠字号堆出来，看起来乱。Refactoring UI 建议同一区域最多 3 档：正文、次要、说明。

**改哪里与怎么做**：

1. **任务和证据组件里的 10px 徽标**
   - `apps/desktop/src/features/chat/components/TaskItem.tsx:86`、`:90`
   - `apps/desktop/src/features/chat/components/GoalProgress.tsx:91`
   - `apps/desktop/src/features/chat/components/EvidenceList.tsx:89`、`:92`、`:100`

   删除 `text-[10px]`，让 Badge 使用默认字号（`components/ui/badge.tsx` 默认 11.5px）。`EvidenceList.tsx:100` 的 `text-red-600` 换成 `text-destructive`（裸 Tailwind 色板在暗色下不适配）。去掉 `py-0` 这类压缩高度的覆盖，使用 Badge 默认的 20px 高度。

2. **时间线字号收敛到三档**：正文 `--fox-type-workbench`（13.5px，设计系统指定的对话阅读字号）、次要 `--fox-type-meta`（13px）、说明 `--fox-type-caption`（11.5px）。在开发者工具里遍历一轮完整对话（用户消息、思考、工具调用、结果、任务清单），列出每个元素的计算字号，把不在这三档的改过去。注意 `turn-process-header.tsx` 有未提交改动，它自身不改，只改 CSS 中对应的选择器；如果必须改 TSX 就跳过。

3. **会话标题**：设计系统要求 13.5px / 500，当前实测是 12.5px / 650（渲染为 700）。在侧边栏和标题栏中找到对应选择器（用开发者工具定位），改为 `var(--fox-type-workbench)` + `var(--fox-weight-medium)`。

4. **ai-elements 里的 `text-xs`**：`components/ai-elements/` 下约 80 处 `text-xs`（Tailwind 默认 12px，不在 Fox 刻度上）。改成 `text-[length:var(--fox-type-caption)]` 或在 `@theme inline` 里注册 Fox 字号后使用。`prompt-input.tsx` 跳过。

5. **`components/ai-elements/confirmation.tsx:173`**：`<Button className="h-8 px-3 text-sm">` 把按钮写死成 32px，不在按钮高度规范（34px 主操作 / 28px 行内操作，`globals.css:97-98`）里。删除这些覆盖，改用 Button 的 `size` 属性（审批卡里的按钮用 `size="sm"`）。

6. **`components/ai-elements/artifact.tsx:20`**：`shadow-sm` 删除。设计系统 §4.6：普通卡片默认无阴影。

### 3-2 对话工作台的阴影和光晕

**为什么**：设计系统 §4.6 规定：页面区块不加阴影；普通卡片无阴影；只有 Popover、Dropdown、Dialog 可以有一层弱阴影；输入框不得用发光或多层阴影。Refactoring UI 的观点相同：阴影表示“浮在上面”，滥用会让所有东西都在抢注意力。

**改哪里与怎么做**：

1. **任务卡片翻页箭头的光晕**
   - `workbench.css:9044`：`box-shadow: 0 0 18px 10px color-mix(... var(--fox-card) 42% ...)`
   - `workbench.css:9056`（hover/focus）：两层，含 23px 15px 的光晕
   - `workbench.css:9612`（窄屏）：`0 0 18px 12px`

   同时 `.fox-agent-task-arrow` 的 `opacity: .52`（第 9042 行）让箭头图标本身对比度不足。改法：箭头按钮用实心 `--fox-card` 背景 + `--fox-border` 边框，`opacity: 1`，删除光晕。如果光晕的作用是遮挡后面滚动的卡片，改用滚动容器边缘的渐变遮罩（`mask-image: linear-gradient(...)`），不要用阴影。

2. **其他发光**：`workbench.css:6474`（`0 0 7px var(--fox-accent-soft)`）和 `:8986`（强调色两层阴影）。先确认所属组件，属于普通卡片或列表项就删除，改用边框颜色变化表示状态。

3. **保留的阴影**：`0 0 0 2px/3px` 这类焦点环、`inset 0 0 0 1px` 这类用阴影画的边框，不属于“浮起阴影”，保留。

4. **Composer（等 D2 拍板）**：`workbench.css:5385` 的 `--fox-composer-elevation` 改为一层弱阴影（例如参考 `--fox-shadow` 的强度再减弱）。只改这个变量，`:4581`、`:5386`、`:5408` 三处引用自动生效。是否保留 `backdrop-filter` 由用户决定。

**做法**：先用 `rg "box-shadow:(?!\s*none)" --pcre2` 列出 `workbench.css`（约 54 处）中所有非 none 的阴影，按“保留（浮层 / 焦点环 / inset 边框）”和“删除（卡片 / 输入框 / 光晕）”分类，把分类表写进提交说明。

**验收**：对话页截图中只有浮层（菜单、对话框）有阴影；任务箭头清晰可见。

### 3-3 状态色和分类色收敛

**为什么**：CSS 中有大约 268 个裸色值，形成了一套平行于 token 的调色板。仅绿色就有 `#2eb879`、`#209765`、`#18865a`、`#198c64`、`#31a46c` 五种，黄色有 `#d99a35`、`#b97718`、`#a76a11`、`#bd7b16`。后果：同一个“就绪”状态在不同页面颜色不一样；这些颜色都没有暗色版本。

另外，分类图标底色 `.fox-library-icon.is-blue/is-violet/is-green/is-amber/is-gray`、`.fox-shadcn-kb-icon.is-*`（`workspace-pages.css` 压缩块内，背景如 `#eef4ff`、`#f3f0ff`）**没有任何 `.dark` 覆盖**，暗色主题下会显示成浅色方块。

**改哪里**：stylelint `color-no-hex` 报告，`workspace-pages.css`（约 146 处）为主，`workbench.css`（约 64 处）、`local-knowledge.css`（约 27 处）。

**怎么做**：

1. **状态色**（就绪、处理中、成功、失败）：圆点用 `--fox-success` / `--fox-warning` / `--destructive`；文字用 1-2 新增的 `--fox-success-text` / `--fox-warning-text`；底色用 `-surface`。例如 `.fox-entity-status`、`.fox-shadcn-kb-footer .badge.is-ready/.is-processing`、`.is-ok`、`.fox-connection-result`。
2. **分类色**（蓝、紫、绿、琥珀、灰五种图标底色）：在 `globals.css` 新增一组 `--fox-tint-{blue,violet,green,amber,gray}-{fg,bg}`，亮色取现有值，暗色另配深底亮字。然后替换所有 `.is-blue` 等规则。
3. 第三方组件需要的颜色（例如 `settings-pages.tsx:224` 传给动画组件的 `color2`）不在本阶段处理；`settings-pages.tsx` 本身也在不碰清单里。

**验收**：`color-no-hex` 违规大幅下降（记录数字）；暗色截图中分类图标是深底；所有“就绪”状态颜色一致。

### 3-4 彩色底上的灰字

**为什么**：Refactoring UI 硬规则之一：不要在彩色背景上用灰色文字，灰色会显得脏而且对比度不足。应该用同色相的深色，或直接用正文色。

**改哪里**：`workspace-pages.css:1616-1624` 的 `.fox-pdf-search-excerpt`：背景是 `color-mix(in oklch, var(--fox-accent-soft) 42%, var(--fox-card))`（浅蓝），文字是 `var(--fox-muted)`（灰），字号 11px。

**怎么做**：文字改为 `var(--fox-text)`（摘录是要阅读的内容）或 1-3 的 `--fox-accent-text`；字号改为 `var(--fox-type-caption)` 或以上。同时用 stylelint 报告和人工检查其他“浅蓝/浅绿底 + `--fox-muted`/`--fox-faint` 文字”的组合，例如 `.fox-readonly-note span`、`.fox-doc-callout span`，同样处理。

**验收**：这些位置的文字对比度 ≥ 4.5:1。

### 3-5 列表页和卡片（专家、知识库、插件）

**为什么**：卡片里的标题、简介、统计、标签、页脚大量使用 7.5–10px 字号，标题 12px / 650。卡片内部有 5 层以上信息在争夺注意力。

**改哪里**：`workspace-pages.css` 中 `.fox-shadcn-kb-*`（3 个 TSX 文件在用）、`.fox-library-icon`、`.fox-page-toolbar`、`.fox-page-intro`。

**怎么做**（D1 拍板后）：

1. 卡片标题：14px / 600（设计系统 §6.5）。
2. 简介：12–13.5px，最多两行（设计系统 §6.5）。
3. 统计数字和标签说明：至少 11.5px。卡片高度不够时，**先减少显示项**（例如 4 个统计只留 2 个最重要的，其余进详情页），再考虑加高卡片。
4. 页脚按钮：使用 Button 组件默认尺寸，删除 `font-size:9px`、`height:28px` 之类的覆盖。
5. 页面顶部说明 `.fox-page-intro p` 10.5px → `var(--fox-type-meta)`；页面小标题（`.fox-section-kicker` 8px 大写字母）→ 至少 11.5px，或者干脆删除（Refactoring UI：标签是最后手段，标题本身能说明的就不要再加一个小标签）。
6. 工具栏 `.fox-page-toolbar button` 9px → 使用 Button 默认字号；`.badge` 7px → Badge 默认。

**验收**：卡片截图中最小文字 ≥ 11.5px；卡片不溢出；同一页面卡片高度一致。

### 3-6 本地知识库：去掉私有字号刻度

**为什么**：`local-knowledge.css:3742-3753` 在 `.fox-local-kb-detail-page` 上把所有 `--fox-type-*` token 重新定义了一遍，每一档都比全局大 0.5–1px（例如 caption 12.5px、body 14.5px、card-title 17px）。这等于在一个页面里有一套不同的设计系统，从这个页面切换到其他页面时字号会跳变。

**改哪里**：`apps/desktop/src/styles/local-knowledge.css:3742-3753`。

**怎么做**：

1. 删除这 11 行重新定义，回到全局刻度。
2. 截图对比详情页。如果某个区域因此显得太小，说明那里本来就该用更高一档的 token（例如正文用 `--fox-type-body` 而不是 `--fox-type-meta`），在具体选择器上改用更高一档，而不是重新放大整套刻度。
3. 同时处理该文件中约 42 处小于 11.5px 的字号。

**验收**：本地知识库详情页和其他页面的同类元素字号一致。

### 3-7 减少嵌套边框（本地知识库详情、设置页）

**为什么**：卡片里套卡片，每层都有 1px 边框，形成“框中框”。Refactoring UI 建议用间距、背景色差或者只保留外层边框来分组。

**改哪里**：本地知识库详情页的网格区块（`local-knowledge.css`）；设置页的 `.fox-settings-section` 内部的 `.fox-service-card` 等（`workspace-pages.css`）。设置页 TSX（`settings-pages.tsx`）在不碰清单里，只改 CSS。

**怎么做**：外层保留边框；内层改为无边框，用 `--fox-surface-subtle` 背景或 `--fox-space-l` 以上间距分组；内部列表用分隔线代替每行一个边框。

**验收**：截图中同一区域最多两层边框。

---

## 第 4 阶段：共享组件

### 4-1 统一空状态组件

**为什么**：空状态是用户第一次使用某个功能时看到的画面，Refactoring UI 特别强调要设计好：图标 + 一句说明 + 一个明确的下一步操作。项目里已经有设计好的 `.fox-page-empty-state` 样式（`workspace-pages.css:88-120`），但只有 `knowledge-pages.tsx:78-79` 在用。其他页面的空状态是一个普通 `<p>`，没有下一步操作。

**改哪里**：

- 新建 `apps/desktop/src/components/ui/empty-state.tsx`：属性 `icon`、`title`、`description`、`action?`，复用 `.fox-page-empty-state` 的样式（或把样式迁移到组件里）。
- 替换以下空状态：
  - `features/agents/agent-pages.tsx` 中 `<p>还没有对话任务。</p>`（当前约第 385 行）
  - `features/agents/ExpertPickerDialog.tsx:43`（`fox-expert-picker-empty`）
  - `features/agents/digital-colleague-manager.tsx:235`、`:255`（`fox-agent-resource-empty`）
  - `features/agents/agent-pages.tsx` 中的能力绑定空状态（约第 936–938 行的 `empty` 文案，渲染位置在附近）
- `knowledge-pages.tsx:78-79` 改为使用新组件，确认外观不变。
- 对话框内的空状态（如专家选择）用紧凑版本（只有图标和一行文字），不要大插画。

**验收**：所有空状态有统一外观，能引导下一步的都有操作按钮；`build` 和测试通过。

### 4-2 用应用内确认框替换 `window.confirm`

**为什么**：`window.confirm` 是系统原生对话框：不跟随应用主题（暗色主题下是白框），按钮文字是系统语言，不能把危险操作的按钮标成红色，还会阻塞整个页面的 JavaScript。项目已经有 `components/ui/alert-dialog.tsx`。

**改哪里**（可以改的）：

- `features/agents/agent-pages.tsx:647`（删除专家）、`:684`（回滚版本）
- `features/agents/digital-colleague-manager.tsx:244`（撤销数字同事）
- `features/local-knowledge/local-knowledge-pages.tsx:665`（放弃未保存修改）
- `features/chat/components/ManagedFilesPanel.tsx:148`

**暂不改**：`features/settings/settings-pages.tsx:539`、`:931`、`:935`、`:1071`、`:1073`、`:1074`（文件有未提交改动），列入报告，等用户提交后再做。

**怎么做**：新建一个 `useConfirm()` hook（或 `<ConfirmProvider>`），返回 `confirm({ title, description, confirmLabel, destructive }) => Promise<boolean>`，内部渲染 `AlertDialog`。这样调用点只需把 `window.confirm(...)` 换成 `await confirm({...})`，改动最小。删除、撤销类操作设置 `destructive: true`，确认按钮使用 Button 的 `destructive` 变体。

**验收**：上述操作弹出的是应用内对话框，跟随主题；取消和确认的行为与原来一致（包括 `local-knowledge-pages.tsx:665` 的“关闭时检查未保存”逻辑）；`bun test` 通过。

---

## 第 5 阶段：防止回退

### 5-1 stylelint 改为阻断新增违规

**为什么**：不加约束的话，新代码很快会重新引入小字号和裸色值。

**怎么做**：

1. 第 1–4 阶段完成后，把 0-2 的规则从 `warning` 改成 `error`。
2. 仍然存在的历史违规，用 `/* stylelint-disable-next-line ... -- 原因 */` 逐条标注，或在 `overrides` 中按文件豁免，**不要整体关闭规则**。
3. 在 `.github/workflows/windows-ci.yml` 中加入 `pnpm --dir apps/desktop lint:css` 步骤。
4. `10px` 这类不在间距刻度上的值（约 205 处）**只告警，不阻断，也不批量替换**。间距改动最容易引起布局偏移，只在修改相关组件时顺手改。

### 5-2 更新设计系统文档

**为什么**：新增的 token 如果不写进文档，下一个人不知道该用哪个。

**改哪里**：`docs/03-产品与前端/设计系统.md` §4.5 颜色表（第 199 行附近）和字号表。

**怎么做**：补充 `--fox-success-text`、`--fox-warning-text`、`--fox-warning-border/-surface`、`--fox-accent-text`、`--fox-control-border`、`--fox-tint-*`，每个写明用途和对比度要求；更新 `--fox-faint` 的定义；按 D1 的结论修正卡片标题字号的矛盾。

---

## 明确不做的事

- 不改整体布局、侧边栏宽度、页面结构，不重新设计页面。
- 不批量替换间距值。
- 不改 Rust、Runtime、数据模型。
- 不改 0.1 列出的有未提交改动的文件。
- 不引入新的 UI 库或图标库。

---

## 最终交付物

执行 agent 完成后应提交：

1. 每个阶段一个提交。
2. `docs/03-产品与前端/design-audit/2026-10-05/before/` 和 `after/` 两组截图，命名一一对应。
3. `lint-baseline.md`（第 0 阶段）和最终 stylelint 统计，对比每类违规的数量变化。
4. 一份报告，包含：
   - 每条计划的完成状态（完成 / 跳过 / 部分完成），跳过的写明原因；
   - 因为文件有未提交改动而跳过的条目；
   - 截图中出现的、计划内的可见变化，和计划外的意外变化；
   - 需要用户决定的事项（D1–D3 及执行中新发现的问题）；
   - 最后一次 `build`、`bun test`、`lint:css` 的结果。

## 优先级速查

| 优先级 | 条目 | 理由 |
|---|---|---|
| P0 | 0-1、0-2、1-1、1-3 | 基线是后续一切验证的前提；faint 和主色影响全局可读性 |
| P1 | 1-2、1-4、2-1、2-2、3-1、3-2 | 对话页是主场景；控件边框涉及可访问性 |
| P2 | 1-5、2-3 到 2-6、3-3 到 3-6、4-2 | 一致性和暗色主题问题 |
| P3 | 3-7、4-1、5-1、5-2 | 体验打磨和长期防回退 |
