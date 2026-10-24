> 历史文档，内容截至 2026-10-14，现状见 [docs/02-架构/架构文档导航.md](docs/02-架构/架构文档导航.md) 与当前代码（本文不再维护）。

# 交接说明：第 6 步剩余页面重设计（B 线）

给下一个 agent 的交接文档。目标：做完第 6 步剩下的页面重设计，同时把每个页面在 `COMPONENT-REUSE.md` 里对应的自建组件换成库组件。

## 1. 环境与现状

- 仓库：`D:\python\projects\Fox\worktrees\ui-improvements-20261009`，分支 `ui/improvements-20261009`，HEAD `f391401`（已推送到 origin）。**只在这个工作树里改代码。**
- 依赖已修复（2026-10-11）。允许 `pnpm install --frozen-lockfile`；**禁止**手工建 junction / `ln -s` / 删改 `node_modules` 里的东西。如果安装报 ELOOP 之类错误，停下来报告，不要自己修。
- 验证基线：`tsc` 0 错误；`bun test ./apps/desktop/tests` 585 通过 / 2 跳过；构建入口 236.6 KiB（预算 240）、首屏 307.6 KiB（预算 316）。改动后不能更差。
- 行尾：仓库已用 `.gitattributes` 固定 LF，编辑后保持 LF。
- 当前基线上 `tsc` 通过、`git status` 只有 `?? .workbuddy/`（不要动它，也不要提交它）。

## 2. 设计依据（按优先级）

1. **`docs/03-产品与前端/知识库重设计/index.html`**（728 行）——主要设计说明。开头有 4 条设计原则和「页面结构」对照表（原页面 → 重设计后），每个页面有「突出 / 收进图标菜单 / 去掉或改说法」三组要点，还有一份**用词替换表**（「索引」→「整理」、「分块」→「分段」、「向量检索」→「智能搜索」等）。动任何一页之前先把整个文件读完。
2. **原型截图**：`docs/03-产品与前端/知识库重设计/images/<主题>-<明暗>/01..10.png`，3 套主题 × 明暗各 10 张。以 graphite（石墨）为准，paper / mist 只验证token 不缺。
3. **`PAGE-REDESIGN.md`**——10 条通用规则（第 1 节）是验收标准，每改完一页逐条自查。分页细节（第 3 节）与原型不一致时**以原型为准**。
4. **`COMPONENT-REUSE.md`**——组件替换清单。改某一页时，把该页对应的条目一并换掉，并在报告里引用编号。

## 3. 范围与顺序

分两阶段。第一阶段 6 页没有争议，直接做。第二阶段是知识库详情页**重做**，它会推翻 `61e4947` 已实现的「三标签 + 检查器」结构（原型「页面结构」表第 4 行明确要求合成一个知识库主页），开工前先找用户确认。

### 第一阶段（按此顺序，每页一笔提交）

| # | 页面 | 原型 | PAGE-REDESIGN | 组件替换（COMPONENT-REUSE 编号） |
|---|---|---|---|---|
| 1 | 任务中心 | `08-jobs.png` | 3.2 | P26（任务条目 → assistant-ui Job progress / Timeline）、P25（进度条 → Progress）、P39（按天折叠 → Collapsible）、P10（分段筛选 → Toggle Group）、P24（状态点统一）、P21/P22（空状态 / 加载 → Empty / Spinner） |
| 2 | 向量模型 | `09-models.png` | 3.6 | P35（模型行）、P25（下载进度 → Progress）、P24（状态点）、P21（空状态） |
| 3 | 知识首页（「最近」） | `01-home.png` | 3.8 | P21（空状态）；删掉 4 个重复导航按钮 |
| 4 | 本地知识库列表 | `03-kb-list.png` | 3.7 上半 | P2（KnowledgeBaseCard，`LK:751` → Card / Item，删掉 `!important` 覆盖）、P12（搜索框 → Input Group）、P21（空状态） |
| 5 | 本地文件 | `02-files.png` | 3.7 下半 | P33（文件行 / 来源行 → Table + Checkbox）、P16（导入目标单选 → Radio Group）、P32（两套文件图标函数合并成一个）、P12（搜索）、P10（类型筛选） |
| 6 | 新建知识库对话框 | `10-kb-create.png` | 3.9 | P14（表单字段 → Field + Label）、P39（「更多选项」→ Collapsible）、P37（拖放区 → Attachment，接近匹配，先看组件再决定） |

代码位置（`apps/desktop/src/`）：

- 页面 1、2、4、5、6 都在 `features/local-knowledge/local-knowledge-pages.tsx`（约 2800 行）：任务中心 `LK:2237 LocalKnowledgeJobsPage`、任务条目 `LK:2403 JobTimelineItem`、向量模型 `LK:2566` 起、知识库卡片 `LK:751`、本地文件 `LK:1036`–`LK:1338`、新建对话框 `LK:449 CreateKnowledgeBaseDialog`。样式在 `styles/local-knowledge.css`。
- 页面 3（知识首页）在 `features/knowledge/`，自行定位（问候语 + 4 个描边按钮 + 最近访问卡片；改动前截图在 `output/ui-audit/knowledge-home-*.png`）。
- 行号来自 `cd32b99` 基线，可能已漂移；对不上就按组件名 / 类名搜索（`COMPONENT-REUSE.md` 第 0 节有缩写表和搜索方法）。
- **这些页面都在同一个大文件里，必须顺序做，不要开多个 agent 并行改。**

### 第二阶段（先找用户确认再做）

知识库详情页 + 阅读器：原型 `04-kb-home` / `05-kb-search` / `06-reader` / `07-kb-settings`。原型要求把 `61e4947` 做好的「文档 / 检索测试 / 任务」三标签 + 280px 检查器**合并成一个知识库主页**：资料列表是主体、检索测试变成顶部搜索框、任务变成时钟弹层、检查器内容移进齿轮打开的设置抽屉。涉及条目：P10、P26、P31（文件树 → AI Elements File Tree）、P33/P34（列表 → Table）、P36（面包屑 → Breadcrumb）、P37（拖放区）、P38（检查器 → Item + Collapsible，窄屏 Sheet）、P42（检索结果 → assistant-ui Retrieval chunks）。

## 4. 组件安装

第一阶段需要的 shadcn 组件（装之前先 `npx shadcn view <名字>` 看会带进什么）：

```
npx shadcn add item empty field label table checkbox radio-group toggle-group
```

已装可直接用（`apps/desktop/src/components/ui/`，以目录实际内容为准）：button、card、badge、alert、alert-dialog、dialog、sheet、popover、tooltip、hover-card、tabs、collapsible、accordion、input、input-group、textarea、select、switch、button-group、command、context-menu、dropdown-menu、scroll-area、separator、resizable、avatar、progress、spinner、skeleton、sonner、carousel、kbd。

assistant-ui 的页面级组件（job-progress、timeline、retrieval-chunks 等）在第二阶段才需要，装之前用 `npx shadcn view @assistant-ui/<注册名>` 确认依赖，装进来是源码，**必须改成项目的设计变量**（`UI-PLAN.md` 第 2 节的规范），不能带着它自带的视觉风格上线。

每装一批组件后跑：`pnpm exec tsc --noEmit -p apps/desktop`、`bun test ./apps/desktop/tests`、`pnpm --dir apps/desktop build`，注意首屏 JS 预算。

## 5. 不做的事

- **聊天区一行都不动**：`features/chat/` 整个目录、`COMPONENT-REUSE.md` 里 C / T / R / S 编号的条目、`fox-thread-composer.tsx`、`fox-thread-empty.tsx`。这些属于第 5 步，另一条线在收。
- **不做全库死 CSS 大扫除**（COMPONENT-REUSE 第 0 档，约 4,700 行）。只删「这次重设计替换掉的、确认没有引用的」样式，随该页提交一起删。剩下的等 8 页全做完单独一笔。
- **不改 Paper 主题强调色**：原型 `index.html` 顶部有一段 paper 暖墨色 accent 覆盖（注释写着 "Mirror this into themes.css if adopted"），那是第 8 步的事。
- **不把「向量模型」改名**（原型 710 行的建议），导航名保持现状。
- 不改 `PAGE-REDESIGN.md`、`COMPONENT-REUSE.md` 本身；发现错误或行号漂移，在报告里写，不要顺手改文档。本文件可以更新进度。

## 6. 工作流程（每一页都走一遍）

1. 改之前：读 `index.html` 里该页的设计说明和三组要点；从 `output/ui-audit/` 找该页改动前的截图（根目录有 `knowledge-jobs-*.png`、`knowledge-models-*.png`、`local-files-*.png`、`local-knowledge-list-*.png`、`knowledge-new-dialog-*.png`、`knowledge-home-*.png`）。
2. 改：页面结构按原型，组件按 COMPONENT-REUSE 替换，文案按用词替换表。样式写进该页已有的 CSS 文件，用设计变量，不写死颜色。
3. 验证：`tsc`、测试、构建（看预算）；起 dev server 截图明暗两套（**端口不要 1421**，那是用户在跑的实例），存到 `output/ui-audit/step6/<页面名>/`。
4. 自查：`PAGE-REDESIGN.md` 第 1 节 10 条逐条过一遍，写进报告。
5. 提交：每页一笔，提交信息仿照 `feat(desktop): 第6步 知识库详情页重设计（…）` 的格式。**不要 push**，由用户决定。

## 7. 报告格式

全部做完后写一份报告（参考 `UI-CHANGES-STAGE5-6.md` 的风格），每页一节：改动文件和行数、替换了哪些组件（引用 COMPONENT-REUSE 编号）、删了多少 CSS、10 条规则自查结果、截图路径、测试 / 构建数字。最后一张汇总表：剩余问题、没替换成的组件及原因。

## 8. 开工前向用户确认的两件事

1. 第二阶段（详情页按原型重做，推翻 `61e4947` 的结构）是否执行。
2. 用词替换表是否全量执行（它改动的是用户可见文案，不限于界面结构）。

第一阶段不依赖这两个答案，可以先做。

## 9. 进度（2026-10-12）

第 1–6 页 + 第二阶段（知识库主页 / 阅读页 / 资料列表 / 设置抽屉）全部改完，用词替换表全量执行：`node scripts/local-knowledge-browser-check.mjs` 的旧词扫描在 8 个视图 × 明暗 / 纸感 3 套主题下为 0 条残留，32 个场景全绿。改动**尚未提交**（受「本次不要提交」约束）。

- 验证通道：`apps/desktop/scripts/local-knowledge-browser-check.mjs`（vite 1458 + headless Chrome CDP 9358，证据写在 `output/local-knowledge-browser/`）。它同时是回归测试：悬停出现的 ⋯ 会额外断言键盘聚焦也能显形，面包屑会量几何而不是只看文案。
- 已删的死 CSS 仅限本次重设计替换掉的部分：旧任务时间线、旧知识库卡片、本地文件来源卡片（含大号文件夹插画）、`.fox-local-kb-home-actions`，共约 340 行。全库死 CSS 大扫除仍未做。
- **未做**：`COMPONENT-REUSE.md` 里 P / LK 编号的库组件替换（Job progress、Progress、Collapsible、Toggle Group、Table + Checkbox、Radio Group、Field + Label、Attachment 等），页面结构改完后再单独排。
- **未做 / 需要产品决定**：① 阅读页的「问问这份资料」需要一条 4 层聊天桥（目前没有 `fox:composer-draft` 事件，聊天里也没有持久化的知识库选择），先落地了「复制这份资料的引用」；② 知识库列表页保留了搜索框（原型要求只在超过 8 个知识库时才显示）；③ 全站不做拖拽导入，改用点击选文件（原型对话框里是拖放区）；④ 任务中心不给预计剩余时间；⑤ 首页搜索框只搜单个知识库（没有跨库检索能力），默认落在「最近打开过的那个库」。

## 10. 第 2 轮还原度自查（2026-10-12）

按用户反馈（「很多页面都还不够还原，例如最近页面，布局、边距等等」）逐页把原型的度量搬进实现，数值一律取自 `docs/03-产品与前端/知识库重设计/index.html` 的样式块，并在浏览器检查里量几何而不是看截图。

- **阅读页**改成原型的 `.rtop / .article` 结构：52px 工具条贴住内容区左右与上边（跳出 32/24 内边距），知识库页头在阅读态不再重复出现（面包屑已带库名），正文 640px 版心落在 96px 版肩，14px / 1.9 行高，32px h1；内嵌查看器（PDF / Office）才保留卡片。
- **新建对话框** 560 → 460px、内边距 24、字段 36px、拖放区 22px 内边距、页脚 24 / 8 节奏；**设置抽屉** 400px、标题行 20px、单选改成原型的无边框行（选中态只由 15px 圆点表达）、页脚贴底且横线贯通。
- **知识库主页**：搜索出结果时资料列表让位（原型 05-kb-search），命中卡片悬停浮出「复制这段引用 / 打开资料」，列表末尾补回 `.dropline`（改成点击入口）。
- **任务中心**：进行中的任务不再默认展开阶段清单，和原型一样是一行 + 3px 进度条 + 百分比，行首补 16px 状态图标；分段筛选带计数，工具条搬进页头。
- **最近页**：最近打开的悬停改成向外扩 12px 的圆角底板（与命中卡片一致），空状态改成左对齐一行。
- 检查脚本新增/改写约 20 条几何断言（对话框 460/24/36、抽屉 400 + 15px 圆点 + 页脚贴底、阅读页满宽 52px 条 + 96px 版肩 + 640 版心、任务行 3px 条与展开/收起、文件列表表头与行的列对齐、主页搜索框 36px 与 44px 行高）。当前 32 个场景全绿，旧词扫描 0 条；`tsc`、`bun test`（601 pass / 0 fail）、`vite build` + 包体预算（entry 200.4 / initial 271.8 KiB）、release 隔离检查均通过。改动**仍未提交**。
- 本轮仍留的偏差：阅读页没有右侧目录（需要先做标题抽取）；任务中心保留「已完成」分段（原型只有三个）；进行中任务的「取消」在展开面板里而不是行尾（行尾是 `<button>`，再塞按钮要先把行头改成 div）；没有 Ctrl+K；没有「让助手根据这些内容回答」。

## 11. 第 3 轮：设置入口与浮层锚定（2026-10-12）

用户反馈三张真机截图：① 本地文件的已选条压在列表上方且按钮白底白字；② 内置「Fox 使用指南」没有设置入口；③ 新建对话框里「选择一个文件夹」是个没有样式的裸按钮。

- **指南库的设置入口**：齿轮不再按库隐藏，内置库改为打开**只读**抽屉（`KnowledgeBaseSettingsSheet` 新增 `readOnly`）——名称、简介、搜索方式禁用，去掉保存与删除，页脚只剩「关闭」，高级区只保留存放位置等只读信息。检查脚本走 `view=detail&kb=fox-user-guide` 逐条断言。
- **浮层错位的根因在共享组件里**：`src/components/ui/select.tsx` 的 `SelectContent` 默认 `position="item-aligned" align="center"`，Radix 会把面板按**视口**居中，触发器偏离中线时看起来就是「弹窗错位」。已把默认值改成 `position="popper" align="start" sideOffset={4}`（贴着触发器左下展开），一次性覆盖全站另外 29 处没有显式传参的下拉（`settings-pages` 10、`agent-pages` 8、`knowledge/knowledge-pages` 6、`digital-colleague-manager` 3）。知识库原先手写的 10 处 `position="popper"…` 因此删掉，改由默认值统一负责。
  - 该原语里 `data-[position=popper]` 的视口样式本来就有，popper 才是这个组件的设计路径；`tsc`、`bun test`、其余浏览器检查脚本都不依赖 item-aligned。
- **锚定断言**：检查脚本新增 `anchored(label, trigger, layer)`，量浮层与触发器的垂直间隙（允许 -4～16px）和水平重叠（必须 > 0），覆盖 9 个浮层：卡片菜单、最近动态、阅读页 ⋯、资料列表范围、模型页 ⋯、本地文件范围、文件行 ⋯、排序下拉、任务筛选。排序下拉实测 gap 3px、水平重叠 122px（面板左边缘 1000 对触发器 997），走的正是改后的默认值。
- **已选条**：移到列表之后靠 `sticky bottom` 贴底（原型 `.selbar`）。按钮白底白字的原因是 shadcn 默认 variant 带 `!text-primary-foreground`——分层规则的 `!important` 会赢过未分层的 `!important`，所以覆盖 `color` 无效，改为在 `.fox-local-files-selection` 上覆盖 `--primary-foreground`。检查里加了 oklch 明度差断言（实测 0.855，阈值 0.4）。
- **选文件夹按钮**：改成共享 `Button variant="outline" size="sm"`（实测 28px 高、1px solid 边框、与拖放区 8px 间距、水平居中）。浏览器检查的 mock 网关原先没有 `pickImportFiles / pickImportFolder`，对话框会退化成原生 `<input type=file>` 分支，这个按钮在检查里根本不会出现——已在 fixture 里补上，并加了防回归断言。
- 回归：`local-knowledge-browser-check.mjs` 32 场景全绿、旧词 0 条；`tsc --noEmit` 干净；`bun test ./apps/desktop/tests` 601 pass / 0 fail；`vite build` 通过，包体 entry 200.4 / initial 271.8 KiB（预算 240 / 316）；release 隔离检查 OK。改动**仍未提交**。

## 12. 全站词汇替换（2026-10-12）

按用户给的 21 节对照表替换界面用词。表里的「新描述」只用于已有说明位，**没有对应位置的一律不新增**；原文已经不在代码里的行（21 条：引用工作区文件、操作历史、命中数、预期文档、文档管理、查看使用指南、今天想一起读点什么？等）直接跳过，不删不改。

- **范围**：只改渲染层 `apps/desktop/src/**` 的用户可见字符串，外加 `apps/desktop/tests` 与 `apps/desktop/scripts` 里写死的旧文案断言。
  - **没改 Rust 侧**：`src-tauri` 里的同名串是发给模型的提示词（`Fox 交付核验：…`）、错误匹配串和文档注释，改它们等于改协议并要求 cargo 重建；`web/Yuxi-main-…` 是 vendored 快照，不是本应用界面。
- **三遍替换**（脚本留在 `output/copy-*.mjs`，可复查每一处）：
  1. `copy-apply.mjs`：整串字面量（`'old'` / `"old"` / `` `old` `` / `>old<`）按表替换，364 处。
  2. `copy-words.mjs`：第 1 节的全局词规则（知识库→资料库、本地→本机、会话→对话、子 Agent→协作助手），只作用在**显示字符串 span** 内；对象 key、`value=`/`key=`、比较右侧、正则、路径与 `.md` 文件名一律跳过并单列报告，289 处。
  3. `copy-targeted*.mjs`：歧义词逐处判断（扩展→插件、产物→生成文件、调度→定时、压缩→精简、召唤→开始对话、回滚→恢复版本、能力包→专家包、模型供应商→模型服务、系统提示词→工作要求、来源→资料来源、节点→条目、任务中心→处理进度…），105 处。
- **歧义词按表限定的范围处理**：「授权→批准」只改按钮和状态（`本次操作未获批准`），描述性长句和「授权范围」保留；「配置→设置」只改标签/状态（`连接器设置已变化`、`专家设置无效`），技术字段（`配置值`、`未配置模型`）保留；「压缩→精简」只作用于对话压缩，Office 的「压缩比异常」是文件压缩，不动。
- **两处需要改代码而不是改字**：
  - 终止状态按结果分别显示（表 §19 末行）：`run-phase.ts` 新增 `RunSettlement` + `settlementOf()`，`settled` 阶段现在按 `run.completed/failed/cancelled/interrupted` 显示 已完成 / 处理失败 / 已取消 / 已中断，不再共用「已完成」。
  - 「切换侧边栏」按状态显示「显示侧栏 / 隐藏侧栏」：`fox-window-titlebar.tsx` 用已有的 `leftSidebarCollapsed` 直接分支。
- **连带修正**：知识选择器的 cmdk `value` 搜索串还写着旧词，用户输入新词会搜不到，已同步；`协作助手` 前多出的空格清掉；对话选择器提示里残留的「关键词检索 / 待解析 / 导入文档」对齐到资料库页面在用的说法。
- 回归：`tsc` 干净、`bun test` 601 pass / 0 fail、`local-knowledge-browser-check` 32 场景全绿 0 残留词、`message-controls` 与 `mermaid` 浏览器检查 0 failures、`vite build` + 包体（entry 200.5 / initial 271.8）+ release 隔离 OK。改动**未提交**。
- **本轮发现但没动的既有问题**：`process-layout-browser-check.mjs` 和 `runtime-process-browser-check.mjs` 仍从 `workbench.tsx` 导入 `RuntimeTimeline`，而 d5203d8 删旧聊天引擎时已经不再导出它——这两个 harness 在本次词汇替换之前就会失败（`SyntaxError: does not provide an export named 'RuntimeTimeline'`），要单独排一次修。

