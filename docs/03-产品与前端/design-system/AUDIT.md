# Fox 设计体系审计

> 审计日期：2026-11-06<br>
> 范围：`apps/desktop/src` 的样式与 UI 组件<br>
> 基准：`themes.css` + `globals.css` + `components/ui`，以及本目录的 `DESIGN-SYSTEM.md`<br>
> 说明：所有计数来自 `rg` 扫描，用于判断量级，不作为逐行清单。

## 结论

体系已经有清晰的令牌骨架（三主题、语义色、字号阶梯、控件高度、随主题变化的圆角），但**令牌没有被强制执行**：根字号与 Tailwind 的 rem 单位冲突，600 余处硬编码字号，110 余处主题外的颜色，两个页面样式文件合计超过 500KB 且大量 `!important`。这导致同一产品在不同页面呈现不同的密度、圆角和颜色，文字缩放也只对一部分文本生效。

修复顺序应当是：**先统一单位与令牌入口（P0-1）→ 再清颜色与字号（P0-2/3）→ 再拆样式与去 `!important`（P0-4）→ 最后修组件与局部偏差（P1）**。不建议先做视觉微调。

---

## P0：会持续产生新问题的结构性问题

### P0-1 根字号 14px 与 Tailwind rem 间距冲突

**证据**

- `apps/desktop/src/styles/globals.css:63` 设置 `html { font-size: 14px; }`。
- Tailwind 默认间距是 `0.25rem`，字号 `text-xs` 是 `0.75rem`、`text-sm` 是 `0.875rem`。
- 因此 `gap-2` = 7px（而非 8px），`p-3` = 10.5px（而非 12px），`text-xs` = 10.5px（低于 11px 下限），`text-sm` = 12.25px（而非 14px）。
- 现有代码大量依赖这些类：`text-xs` 150 处、`text-sm` 107 处（`*.tsx`）。

**影响**

- 4px 网格在 Tailwind 写的部分实际是 3.5px 网格，与手写 CSS 的 4/8/12 对不齐。
- `text-xs` 只有 10.5px，正好落在体系规定的 11px 下限之下。
- 文字缩放依赖 `--fox-type-*`；Tailwind 字号不随之变化，140% 时层级会错乱。

**修复**

1. 在 `@theme` 中显式覆盖间距与字号，而不是依赖 rem 默认值：

```css
@theme {
  --spacing: 4px;
  --text-xs: 11px;
  --text-sm: 13px;
  --text-base: 14px;
}
```

2. 保留 14px 根字号（桌面密度是有意选择），但所有 rem 型工具类必须来自上面的显式映射。
3. 若选择改回 16px 根字号，必须同时把所有以 rem 表达的设计值重新核算，不能只改一行。

**验收**

- 浏览器计算样式中 `gap-2` 为 8px、`p-3` 为 12px、`text-xs` 为 11px。
- `rg "text-xs"` 不再产生低于 11px 的实际字号。

---

### P0-2 615 处硬编码 font-size，94 处低于 11px

**证据**

- `apps/desktop/src` 下 CSS 中 `font-size: <数字>` 共 615 处。
- 其中 7 / 8 / 8.5 / 9 / 9.5 / 10 / 10.5px 共 94 处。
- 例：`apps/desktop/src/styles/workbench.css:4549`、`:8872`、`:9028` 均为 `font-size: 8.5px`；`:3353` 为 `9px`；`:2364` 为 `9.5px`。
- 同一文件里还存在 `font: 10px "HarmonyOS Sans SC"` 这类简写（如 `workbench.css:3487`、`:5359`），不会被 `font-size` 检索覆盖。

**影响**

- 8.5px / 9px 中文在 1x 屏基本不可读，在 125% / 150% 缩放下会糊。
- 这些值不随 `--fox-text-scale` 变化，文字缩放形同虚设。
- 与字号阶梯并存，同一个“辅助说明”在不同页面可能是 9、10、11、12、13px。

**修复**

1. 全量替换为 `--fox-type-micro`（11px）及以上令牌；确需更小的角标改为 11px 并调整布局，而不是继续缩小。
2. 统一 `font:` 简写，拆成 `font-family` + `font-size` + `line-height`，避免遗漏。
3. 在 stylelint 中禁止 `<11px` 的 `font-size`（配置见 `DESIGN-SYSTEM.md` 13.2）。
4. 对 94 处逐一确认是否真的需要显示；能删的元数据优先删除。

**验收**

- `rg "font-size:\s*(7|8|8\.5|9|9\.5|10|10\.5)px" apps/desktop/src` 结果为 0。
- 文字缩放 70% 与 140% 下无截断、无重叠。

---

### P0-3 主题外的硬编码颜色

**证据**

- CSS 中十六进制颜色共 53 处，其中 `themes.css` 仅 4 处，其余 49 处散落在页面样式中。
- TS/TSX 中十六进制颜色 63 处。
- 例：
  - `apps/desktop/src/components/ai-elements/file-tree.tsx:176`、`:263`：选中态 `bg-[#e8f4ff] dark:bg-[#233b53]`，完全不随三主题变化。
  - `apps/desktop/src/features/chat/components/TaskItem.tsx:29`：`text-blue-600`，在石墨主题中引入体系外蓝色。
  - `apps/desktop/src/styles/workspace-pages.css:9`、`:25`：`.is-violet { color:#7656d8; background:#f3f0ff }`，深色模式没有对应值。
  - `workspace-pages.css:10`、`:25` 还有大量 `var(--radius)`、`calc(var(--radius) - 2px)`，绕过了按主题定义的 `--fox-radius-*`。

**影响**

- 切到暖纸或雾蓝后，这些区域仍是冷色/固定色，主题只换了一半。
- 深色模式下固定浅色底会造成刺眼或对比度不足。
- 颜色无法集中调整，后续做品牌色或高对比模式必须逐处改。

**修复**

1. 选中态 → `--brand-soft` / `--brand`；蓝色加载 → `--brand`；错误 → `--destructive`。
2. 分类色（紫/绿/琥珀）如果确有必要，新增语义令牌 `--category-*` 并在三主题 × 明暗中定义；否则合并到 `--brand-soft`。
3. `var(--radius)` 全部替换为 `--fox-radius-small / control / card / large`。
4. 打开 `color-no-hex`，仅对 `themes.css` 和插画/图表组件放行。

**验收**

- 除 `themes.css` 与明确豁免的插画文件外，`rg "#[0-9a-fA-F]{3,8}"` 为 0。
- 三主题 × 明暗下不再出现固定蓝色/紫色。

---

### P0-4 样式文件失控与 `!important` 泛滥

**证据**

- `apps/desktop/src/styles/workbench.css`：304,545 字节。
- `apps/desktop/src/styles/workspace-pages.css`：217,669 字节（大量规则压缩为单行）。
- 使用 `!important` 的 CSS 文件 23 个；其中 `workspace-pages.css` 263 处、`workbench.css` 181 处。
- 例：`workspace-pages.css:10` 一张卡片上同时出现 `display:flex!important`、`gap:0!important`、`padding:0!important`、`border-radius:var(--radius)!important`、`box-shadow:none!important`。

**影响**

- 组件自身的 Tailwind 类被页面样式强行覆盖，改组件等于全局回归。
- 单行超长规则无法 review、无法定位、无法安全删除。
- 新页面只能继续加 `!important` 才能生效，债务递增。

**修复**

1. 引入 `@layer base, components, utilities`，页面样式放 `components`，工具类保持 `utilities` 优先级。
2. 按功能拆分 `workspace-pages.css`：`knowledge.css`、`agents.css`、`graph.css`、`entity.css`，单文件不超过 30KB。
3. 先删“整段重置型” `!important`（如卡片上的 `padding/gap/box-shadow`），改用组件 `className` 或变体。
4. 每次删除后用该页面截图对比，避免一次性大改。

**验收**

- `!important` 总数降到 0；若必须保留，单文件 ≤ 5 处并写明原因。
- 单条 CSS 规则不再出现整页压缩成一行的写法。

---

## P1：组件与令牌的不一致

### P1-1 卡片标题实际 13px，令牌是 16px

- 令牌：`globals.css:181` `--fox-type-card-title: 16px`。
- 实现：`apps/desktop/src/components/ui/card.tsx:40` 的 `CardTitle` 使用 `--fox-type-ui`（13px）。
- 影响：文档与实现不一致；页面里出现 16px 区块标题和 13px 卡片标题混用。
- 修复：`CardTitle` 改用 `--fox-type-card-title`，或在体系文档中明确“卡片标题 = 13px，16px 仅用于区块标题”并删除未使用的令牌。二选一，不要并存。

### P1-2 Button `lg` 是 40px，控件令牌是 36px

- 令牌：`globals.css:130` `--fox-control-lg: 36px`。
- 实现：`apps/desktop/src/components/ui/button.tsx:31` `lg: "h-10 …"`，即 40px。
- 影响：大按钮与 36px 的输入框、选择器不同高，表单底部对不齐。
- 修复：改为 `h-[var(--fox-control-lg)]`；若产品确实需要 40px，则新增 `--fox-control-xl`，不要复用 `lg` 名义。

### P1-3 未定义令牌 + 局部覆盖

- `apps/desktop/src/styles/local-knowledge.css:1053` 使用 `--fox-type-section-title`，该变量在 `globals.css` / `themes.css` 中不存在，声明会被浏览器丢弃，标题回落到继承字号。
- `apps/desktop/src/styles/workbench.css:5952` 在局部作用域把 `--fox-type-section` 覆盖为 18px，而全局是 16px（`globals.css:182`），导致同一个“区块标题”在不同对话框里是 16 或 18px。
- 修复：改用 `--fox-type-section`；删除局部 18px 覆盖，若确需更大标题则新增明确的 `--fox-type-dialog-title` 并在文档登记。
- 验收：`rg "fox-type-section-title"` 为 0；`--fox-type-section` 全局只有一个定义。

### P1-4 文字缩放只覆盖令牌

- `--fox-text-scale` 只参与 `--fox-type-*` 的 `calc()`（`globals.css:172–194`）。
- Tailwind 字号类与 615 处硬编码 px 不缩放，因此 140% 时只有部分文字变大。
- 这是 P0-1 / P0-2 的直接后果；两者修完后此项自然消失。
- 验收：140% 下整页文字同比增大，无局部不变。

### P1-5 AlertDialog 的层级低于 Dialog

- Dialog：`apps/desktop/src/components/ui/dialog.tsx:41`、`:67` 使用 `z-[100]`。
- AlertDialog：`apps/desktop/src/components/ui/alert-dialog.tsx:39`、`:61` 使用 `z-50`。
- Sheet 也是 `z-50`（`sheet.tsx:39`、`:67`）。
- 影响：从 Dialog 内部触发删除确认时，AlertDialog 可能被 Dialog 遮住；Sheet 与 AlertDialog 同层，顺序依赖 DOM。
- 修复：建立统一层级令牌：`--fox-z-sheet: 50`、`--fox-z-dialog: 100`、`--fox-z-alert: 120`、`--fox-z-popover: 130`，所有组件引用令牌。
- 验收：在 Dialog 内打开 AlertDialog，确认框始终在最上层且可操作。

### P1-6 警示小字直接使用 `--warning`

- `globals.css:101` 专门定义了混合后的 `--fox-warning-text`，但全仓库只有 `apps/desktop/src/styles/areas/sidebar.css:144` 使用。
- 多处 12px 状态文字直接用 `--warning`，例：`local-knowledge.css:250`、`:1389`、`:3661`，`workbench.css:12264`、`:12308`。
- 影响：12px 下对比度可能低于 4.5:1，尤其是在暖纸和深色主题。
- 修复：所有小字号警示文字改用 `--fox-warning-text`；图标和圆点可继续用 `--warning`。
- 验收：对比度脚本检查三主题 × 明暗，警示小字 ≥ 4.5:1。

---

## P2：一致性与文档

### P2-1 圆角裸值仍多

- `apps/desktop/src` CSS 中 `border-radius: <数字>` 450 处，包含 `999px`、`50%` 等合理值，也包含 5/7/9/11/13/14px 等体系外档位。
- 修复：只保留 `--fox-radius-*`、`999px`、`50%`；其余按最近档位归并。

### P2-2 字体栈重复

- `globals.css:15`、`:64`、`:312` 与 `workbench.css:1244`、`:3487`、`:5359` 各自重复书写 `"HarmonyOS Sans SC"` 字体栈，并混用 `font:` 简写。
- 修复：统一使用 `var(--font-sans)`；简写拆开，避免字号被简写覆盖。

### P2-3 现有文档有过期值

- `docs/03-产品与前端/设计系统.md:68` 记录 `--muted-foreground: oklch(0.556 0 0)`。
- 实际值：`apps/desktop/src/styles/themes.css:43` 为 `oklch(0.54 0 0)`，注释说明 0.556 在侧栏底色上只有 4.4:1，已压低到 0.54 以达到 4.6:1。
- 修复：更新该行，并在文档中注明“与 `themes.css` 冲突时以代码为准”。

### P2-4 缺少视觉回归基线

- 目前没有组件级或页面级截图基线，P0-4 的样式拆分无法安全验证。
- 修复：先为 8 个关键页面（对话、知识库、任务、设置、图谱、智能体、技能、主页）× 三主题 × 明暗建立截图基线，再开始拆 CSS。

---

## 修复顺序与验收

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| 1 | P0-1 统一 Tailwind 间距/字号；建立截图基线（P2-4） | 计算样式符合 4px 网格与 11/13/14px 阶梯 |
| 2 | P0-3 清颜色；P1-6 警示文字 | 主题外 hex 为 0，三主题 × 明暗无固定色 |
| 3 | P0-2 字号令牌化 | <11px 为 0，140% 缩放一致 |
| 4 | P1-1/2/3/5 组件与层级 | 卡片标题、按钮高度、标题令牌、z-index 全部一致 |
| 5 | P0-4 拆分样式、去 `!important` | `!important` 为 0，单文件 ≤ 30KB |
| 6 | P2 收尾与文档同步 | 文档值与代码一致，检查进入 CI |

### 进入 CI 的最小检查

```bash
rg -n "font-size:\s*(7|8|8\.5|9|9\.5|10|10\.5)px" apps/desktop/src && exit 1
rg -n "!important" apps/desktop/src --glob "*.css" && exit 1
rg -n "#[0-9a-fA-F]{3,8}\b" apps/desktop/src --glob "!themes.css" && exit 1
```

第一条与第三条在清理完成前会失败，建议先作为告警，阶段 3 / 5 完成后转为阻断。
