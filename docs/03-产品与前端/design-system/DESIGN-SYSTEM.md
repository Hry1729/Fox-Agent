# Fox 设计体系

> 状态：生效 · v1.0<br>
> 适用版本：Fox 0.1.x 桌面端<br>
> 事实来源：`apps/desktop/src/styles/themes.css`、`globals.css`、`components/ui`（2026-11-06）<br>
> 配套产物：`index.html`（可视规范）、`tokens.json`（机器可读令牌）、`AUDIT.md`（问题清单）

## 1. 这份体系怎么用

1. **先选语义，再选数值。** 颜色、字号、间距、圆角、控件高度都从令牌取值；不要写 `#e8f4ff`、`10.5px`、`7px` 这类一次性数值。
2. **先定信息主次，再决定容器。** 一个视图里带边框或底色的容器最多一层；卡片内部用细分隔线，不再嵌套卡片。
3. **每套主题都要单独检查。** 石墨、暖纸、雾蓝不是换色，圆角、强调色和表面关系都会变化；深色模式不能从浅色推断。

## 2. 设计原则

### 2.1 内容优先，界面退后

文档、对话、任务和数据是主角。导航、工具栏、状态只提供方向，不与内容争夺注意力。能用留白分开的，不再加一层容器。

### 2.2 每套主题只有一个强调色

品牌色只出现在选中、焦点、进度和链接上。红、绿、黄只表示状态，并且必须同时有文字或图标说明。石墨的“强调色”是近黑（深色为近白），不是蓝色。

### 2.3 用留白和细线分层

层级首先来自间距、字号和背景差，而不是边框与阴影。普通卡片默认无阴影；只有浮层、菜单和对话框可以浮起来。

### 2.4 少而确定的档位

字号、字重、圆角、间距、控件高度都从固定档位取值。新代码只引用令牌。

### 2.5 状态必须可读

状态使用“6px 圆点 + 文字”，危险和警告只出现在真正出问题的那一项上。所有可交互元素都有可见的键盘焦点。

## 3. 主题与颜色

### 3.1 三套主题

| id | 名称 | 性格 | 说明 |
|---|---|---|---|
| `graphite` | 石墨 | 中性、紧凑 | 纯灰阶，墨色主按钮，默认主题 |
| `paper` | 暖纸 | 温暖、柔和 | 米白暖灰，暖金强调色，圆角更大 |
| `mist` | 雾蓝 | 冷静、经典 | 冷调蓝灰，延续 Fox 经典配色 |

- 主题写在 `<html data-color-theme="…">`，保存到 `localStorage` 的 `fox.preferences.colorTheme`。
- 只支持浅色与深色两种模式，没有“跟随系统”。
- 颜色唯一来源是 `themes.css`。旧 `--fox-*` 颜色都是语义令牌的别名。

### 3.2 语义令牌（石墨，默认主题）

| 令牌 | 浅色 | 深色 | 用途 |
|---|---|---|---|
| `--background` | `oklch(1 0 0)` | `oklch(0.145 0 0)` | 页面与内容底色 |
| `--foreground` | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | 正文、标题、图标 |
| `--card` | `oklch(1 0 0)` | `oklch(0.18 0 0)` | 卡片与内容块 |
| `--popover` | `oklch(1 0 0)` | `oklch(0.2 0 0)` | 菜单、浮层、对话框 |
| `--muted` | `oklch(0.97 0 0)` | `oklch(0.24 0 0)` | 次级面板、分段控件底 |
| `--muted-foreground` | `oklch(0.54 0 0)` | `oklch(0.708 0 0)` | 次要文字与说明 |
| `--brand` | `oklch(0.205 0 0)` | `oklch(0.922 0 0)` | 唯一强调色 |
| `--brand-soft` | `oklch(0.955 0 0)` | `oklch(0.269 0 0)` | 选中态与品牌浅底 |
| `--border` | `oklch(0.922 0 0)` | `oklch(1 0 0 / 9%)` | 细线、分隔、输入描边 |
| `--destructive` | `oklch(0.577 0.245 27.325)` | `oklch(0.704 0.191 22.216)` | 错误与危险 |
| `--success` | `oklch(0.56 0.12 160)` | `oklch(0.74 0.13 160)` | 完成、可用 |
| `--warning` | `oklch(0.55 0.13 70)` | `oklch(0.8 0.13 75)` | 需注意，不阻断 |
| `--sidebar` | `oklch(0.975 0 0)` | `oklch(0.175 0 0)` | 侧栏底色 |
| `--shell` | `oklch(0.965 0 0)` | `oklch(0.12 0 0)` | 窗口外壳 |

暖纸与雾蓝的完整值见 `tokens.json`。不要从本文抄色值到组件里，组件只写语义名。

### 3.3 状态色规则

- 状态 = 6px 圆点 + 12px 文字。实心表示可用，空心表示缺失。
- 红、橙只标出有问题的项，不用于汇总、标题或边框。
- 12px 文字不要直接用 `--warning`；使用混合前景色后的 `--fox-warning-text`。
- 对比度：正文 ≥ 4.5:1；大字号、图标、描边 ≥ 3:1。深色模式单独检查。

### 3.4 禁止

- 在组件里写十六进制、`rgb()` 或 Tailwind 调色板类（`text-blue-600`、`bg-yellow-100`）。
- 用颜色单独表达状态。
- 在浅色主题使用纯黑阴影，或在深色主题使用浅色阴影。
- 把插画、点阵、3D 材质的调色板当作 UI 语义色。

## 4. 字体

### 4.1 字体族

```css
--font-sans: "HarmonyOS Sans SC", "Segoe UI Variable", "Segoe UI",
  "Microsoft YaHei", system-ui, sans-serif;
```

正文 400，强调 500，标题 600。字重止于 600；不加载未使用的 Bold / Black。

### 4.2 字号阶梯

| 令牌 | 基准 | 用途 |
|---|---|---|
| `--fox-type-micro` | 11px | 最小字号：元数据、角标 |
| `--fox-type-caption` / `--fox-type-meta` | 12px | 辅助说明、时间、快捷键、徽标 |
| `--fox-type-chrome` / `--fox-type-ui` / `--fox-type-control` | 13px | 侧栏、菜单、控件、页面说明 |
| `--fox-type-body` | 14px | 正文、卡片内容、表单值 |
| `--fox-type-workbench` | 15px | 对话正文 |
| `--fox-type-title` / `--fox-type-card-title` / `--fox-type-section` | 16px | 对话框标题、区块标题 |
| `--fox-type-page` | 24px | 页面标题 |
| `--fox-type-display` | 32px | 首页式大标题 |

所有字号都乘以 `--fox-text-scale`（70%–140%）。行高：正文 1.6–1.75，控件 1.4，标题 1.2–1.3。行长：桌面 65–75 字符，移动 35–60。

### 4.3 文字缩放

`text-scale.ts` 只写入 `--fox-text-scale`。**只有引用 `--fox-type-*` 的文字会缩放**；Tailwind 字号类和硬编码 px 不缩放。新代码必须使用令牌，否则 140% 时会出现部分文字变大、部分不变。

### 4.4 关于 14 / 15px 正文

网页正文的通用下限是 16px，但 Fox 是桌面高密度工具。正文 14px、对话 15px 是有意的产品取舍；用 1.6–1.75 行高、克制的行长和 ≥ 4.5:1 对比度保证可读性。面向弱视力或投屏场景时使用产品内的文字缩放。

## 5. 间距与布局

### 5.1 4px 网格

| 令牌 | 值 | 典型用途 |
|---|---|---|
| `--fox-space-micro` | 2px | 图标与文字紧贴 |
| `--fox-space-xs` | 4px | 标签内距、紧凑间隔 |
| `--fox-space-s` | 6px | 图标与标签 |
| `--fox-space-m` | 8px | 控件内距、行内间隔 |
| `--fox-space-l` | 12px | 表单行、按钮组 |
| `--fox-space-xl` | 16px | 卡片内边距 |
| `--fox-space-2xl` | 20px | 区块内距 |
| `--fox-space-3xl` | 24px | 分区之间 |
| `--fox-space-4xl` | 32px | 页面顶部、主按钮区 |
| `--fox-space-5xl` | 40px | 大区块 |
| `--fox-space-6xl` | 48px | 页面底部 |
| `--fox-space-8xl` | 64px | 大留白 |

### 5.2 页面骨架

- 内容最大宽度 1120px，居中。
- 一级页面内边距：上 32、左右 24、下 48。
- 窗口标题栏 30px；侧栏默认 264px。
- 页面标题 24/600，说明 13px `--muted-foreground`，标题行与右侧 32px 按钮同一基线。
- 工具栏最多一个分段控件 + 一个筛选浮层；数据实时更新的页面不要手动刷新按钮。

> **实现注意**：当前根字号是 14px，Tailwind 的 rem 间距会变成 3.5 / 7 / 10.5px。修复前不要把 Tailwind 的 `gap-2`、`p-3` 当作 4px 网格的示例。详见 `AUDIT.md` P0-1。

## 6. 圆角与层级

### 6.1 圆角随主题变化

| 令牌 | 石墨 | 暖纸 | 雾蓝 |
|---|---|---|---|
| `--fox-radius-small` | 4px | 7px | 6px |
| `--fox-radius-control` | 6px | 10px | 8px |
| `--fox-radius-card` | 8px | 16px | 12px |
| `--fox-radius-large` | 12px | 20px | 14px |
| `--fox-radius-overlay` | 12px | 20px | 16px |
| `--fox-radius-pill` | 999px | 999px | 999px |

- 嵌套：内层圆角 = 外层圆角 − 内边距。
- 胶囊只用于头像、开关、状态圆点。
- 不要把某一个主题的圆角写死到组件里。

### 6.2 阴影

| 档位 | 用途 |
|---|---|
| `--fox-shadow-sm` | 凸起控件、分段选中项、被拖起的卡片 |
| `--fox-shadow-md` | Select、Dropdown、Popover、Tooltip |
| `--fox-shadow-lg` | Dialog、AlertDialog、Sheet |

普通卡片无阴影。阴影色由 `--fox-shadow-color` 按主题和明暗提供；不要写 `rgba(0,0,0,.1)`。

### 6.3 表面层级

`--shell`（窗口外壳）→ `--background`（内容底色）→ `--card`（卡片）→ `--popover`（浮层）。

### 6.4 层级（z-index）

| 组件 | z-index |
|---|---|
| Sheet、AlertDialog | 50 |
| Dialog | 100 |
| Select、Popover | 110 |

> **已知风险**：AlertDialog（50）低于 Dialog（100），从对话框内触发确认框可能被遮挡。见 `AUDIT.md` P1-5。

## 7. 动效

| 令牌 | 值 | 用途 |
|---|---|---|
| `--fox-duration-fast` | 120ms | 悬停、按下、颜色与描边 |
| `--fox-duration-base` | 160ms | 默认状态切换、展开收起 |
| `--fox-duration-slow` | 240ms | 浮层出现、较大空间移动 |
| `--fox-ease-out` | `cubic-bezier(.16, 1, .3, 1)` | 进入、落地 |
| `--fox-ease-standard` | `cubic-bezier(.2, 0, 0, 1)` | 常规状态变化 |

规则：

- 动效只表达状态变化、层级或因果，不做装饰。
- 进入用 ease-out；离开比进入更快。
- 列表项错开 30–50ms；移动保持方向连续。
- 尊重 `prefers-reduced-motion`，保留静态可读的最终状态。

## 8. 图标

- 家族：Lucide，24 网格，2px 描边，圆角端点。
- 尺寸：12 / 14 / 16 / 20 / 24px；默认 16，紧凑 14，最小 12。
- 不混用实心与描边；不用 emoji 代替功能图标。
- 只放在帮助扫读的位置：文件类型、导航、状态、工具动作。
- “标签：值”行和区块标题不加装饰图标。

## 9. 组件

### 9.1 Button

- 变体：`default`（实心主按钮）、`outline`、`secondary`、`ghost`、`destructive`（卡片底 + 红边红字）、`link`。
- 尺寸：`xs` 24、`sm` 28、`default` 32、`lg` 40、`icon` 方形。
- 每个视图最多一个实心主按钮；同一动作不要出现两个入口。
- 焦点：2px 实线 + 3px 半透明光晕，偏移 2px。
- 禁用态仍保持文字可读，不用 `opacity: 0.5` 把内容一起淡掉。

### 9.2 Badge

- 高度 20px，字号 12，圆角 `--fox-radius-small`。
- 只承载短状态，不承载句子。
- 不重复页面已经说明的信息（例如在“本地知识库”页给每张卡片再打“本地知识库”标签）。

### 9.3 Input / Select / Textarea

- 高度 32px，圆角 `--fox-radius-control`，字号 13px。
- 聚焦时边框变为 `--ring`，并显示统一焦点圈。
- 错误使用 `aria-invalid` 与 `--destructive`；错误说明放在字段下方，不只改边框颜色。
- 选择器统一使用 `components/ui/select`，不要混用原生下拉。

### 9.4 Card

- 圆角 `--fox-radius-card`，1px `--border`，背景 `--card`，默认无阴影。
- 内边距 16px；`sm` 尺寸 12px。
- 卡片内不再嵌套带边框的卡片；列表用细分隔线。

### 9.5 Tabs / 分段控件

- 高度 32px，底 `--muted`，选中项 `--card` + `--fox-shadow-sm`。
- 用于同层级视图切换；不要用它承载跨层级导航。

### 9.6 Dialog / AlertDialog / Sheet

- Dialog：居中，`--popover`，`--fox-radius-overlay`，`--fox-shadow-lg`，内边距 24px，标题 16/600。
- AlertDialog：用于不可逆或需要确认的动作；确认按钮右置。
- Sheet：从标题栏下方开始，右侧宽 480px（不超过视口）。
- 底部按钮靠右；危险动作使用描边式 destructive。
- 遮罩统一为浅色半透明 + 轻微模糊。

### 9.7 状态与空状态

- 状态：6px 圆点 + 文字；实心可用、空心缺失。
- 空状态：一句话 + 一个动作，居中、不加框、不放大图标块。
- 没有数据时隐藏筛选、排序和工具栏。

### 9.8 侧栏行与设置行

- 侧栏行高 32px，图标 16px；右侧动作只在悬停或聚焦时出现，并保留占位避免标题跳动。
- 设置页使用 `SettingsPageLayout` / `SettingsGroup` / `SettingsRow`：页宽 760px，行最小高 60px，左右两栏，标题 14/500，说明 12px，控件右对齐。
- 设置页只用细线分隔，不用卡片套卡片。

## 10. 页面模式

### 10.1 应用外壳

标题栏 30px；侧栏 264px（可折叠）；内容区最大 1120px 居中。深色模式下侧栏比内容区暗一档，用背景差分层。

### 10.2 页面头

```
标题 24/600
说明 13px muted
[ 分段控件 ]                         [ 主按钮 ]
```

标题行与右侧 32px 控件对齐；说明只写事实，不写营销句。

### 10.3 列表

- 行高 32–44px，用细分隔线，不用每行卡片。
- 状态只在不是默认值时出现。
- 行尾动作收进 `⋯` 菜单或悬停浮层。
- 日期用相对时间（“今天 17:03”“昨天”），不重复显示年份。

### 10.4 对话输入区

- 唯一允许有轻微浮起感的常驻元素，只用一层弱阴影。
- 底部工具栏左右分组：左侧附件与知识库，右侧发送。
- 快捷键提示使用 `kbd`，不写在占位文字里。

## 11. 内容与语气

- 界面文案用中文，动词开头，句子短；避免“请”“您可以”堆叠。
- 状态写事实：“可关键词检索 · 向量检索未启用”，不写“已就绪”掩盖缺失能力。
- 错误 = 发生了什么 + 下一步；原始报错放进“详细信息”展开项。
- 空状态 = 一句话 + 一个动作。
- 数字与单位不重复；数量、日期、百分比在一屏只出现一次。
- 不编造指标、好评或能力；示例数据要明确标注为示例。

## 12. 无障碍

- 正文 ≥ 4.5:1；大字号、图标、描边 ≥ 3:1；三套主题 × 明暗分别检查。
- 状态不只靠颜色，必须有文字或图标。
- 所有操作可用键盘完成，焦点顺序与视觉顺序一致。
- 文字缩放 70% / 140% 无溢出、无截断。
- 图片有替代文本；装饰图 `alt=""`。
- 尊重 `prefers-reduced-motion`。
- 触摸目标不小于 28×28px，推荐 32×32px。

## 13. 落地与校验

### 13.1 代码规则

1. 新样式只引用 `--fox-*` 或语义令牌；禁止新增十六进制与 Tailwind 调色板类。
2. 字号最小 11px；禁止 9 / 9.5 / 10 / 10.5px。
3. 圆角只取 `--fox-radius-small / control / card / large / overlay / pill`；插画组件例外需注释说明。
4. 删除 `!important`，按 `@layer base / components / utilities` 分层。
5. 新增组件先查 `components/ui` 与 `COMPONENT-REUSE.md`，不重复造按钮和卡片。
6. 文案改动同步检查空状态、错误态和加载态。

### 13.2 建议的自动化检查

```jsonc
// stylelint（示意）
{
  "rules": {
    "declaration-property-value-disallowed-list": {
      "font-size": ["/^(7|8|8\\.5|9|9\\.5|10|10\\.5)px$/"],
      "border-radius": ["/^(5|7|9|11|13|14)px$/"]
    },
    "color-no-hex": true
  }
}
```

`color-no-hex` 需要为 `themes.css` 与插画组件配置例外。Tailwind 项目建议在 `@theme` 中把 `--spacing` 设为 `4px`，并把 `--text-xs` / `--text-sm` 对齐到 11 / 13px，直接消除 rem 与 14px 根字号的冲突。

### 13.3 提交前检查

- [ ] 三套主题 × 浅色 / 深色都看过。
- [ ] 文字缩放 70% 与 140% 无溢出。
- [ ] 键盘可以完成主流程；焦点可见。
- [ ] 状态不只靠颜色。
- [ ] 没有新增硬编码颜色、字号、圆角。
- [ ] 没有新增 `!important`。

## 14. 与现有文档的关系

- `docs/03-产品与前端/设计系统.md` 是代码地图，记录文件与实现位置；本文是面向设计决策的规范。
- 两者冲突时以当前代码和本文为准；`设计系统.md` 中已过期的值（例如 `--muted-foreground` 0.556）应随代码更新。
- 具体问题、证据与修复顺序见 `AUDIT.md`；可视规范见 `index.html`；机器可读值见 `tokens.json`。

## 15. 相关代码

- `apps/desktop/src/styles/themes.css` — 颜色唯一来源
- `apps/desktop/src/styles/globals.css` — 尺寸、间距、字号、派生色
- `apps/desktop/src/components/ui/` — Button、Badge、Input、Select、Card、Dialog、Sheet、Tabs
- `apps/desktop/src/features/settings/settings-layout.tsx` — 设置页行布局
- `apps/desktop/src/features/settings/color-theme.ts` — 配色主题
- `apps/desktop/src/features/settings/text-scale.ts` — 文字缩放
- `apps/desktop/src/styles/workbench.css`、`workspace-pages.css` — 现有页面样式（含待收敛的旧规则）
