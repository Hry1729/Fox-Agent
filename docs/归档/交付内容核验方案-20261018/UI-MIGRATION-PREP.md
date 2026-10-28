# UI 迁移准备：交付待审入口

本轮只做准备：不合并、不改业务代码、不扩大后端修复范围。本文所有事实均为读取当前 HEAD 实测，未推定。

## 1. 提交分类（修正后）

`main` = `0df9162`（未动），最终候选 `merge-candidate/fox-closeout` = `048dff0`。

| 分类 | 数量 | 范围 |
| --- | --- | --- |
| 前置提交 | **21** | `0df9162..b893ba7` |
| 本轮修复 | **5** | `77bb2a0` `8c6dd86` `6a9b76a` `d14692e` `73ca350` |
| 合并提交 | **1** | `c6a7a84` |
| 输入框修复 | **1** | `048dff0` |
| 合计 | **28** | 与 `git rev-list --count main..候选` = 28 一致 |

修正说明：上一轮报告写“前置基线链 22 个”，实测 `git rev-list --count 0df9162..b893ba7` = **21**，已经按 21 更正。21+5+1+1=28 自洽。

## 2. 受测后端、受测前端与最终候选的对应关系

### 2.1 后端（可精确绑定）

| 项 | 值 |
| --- | --- |
| 运行提交 | `c6a7a84` |
| 可执行文件 | `output\_mc-target\debug\fox-desktop.exe` |
| exe SHA256 | `6D373C1DC4609B60E035EFF8C71814702442F3AB54DE76AD091673787FE95CBF` |

该后端二进制**不含** `048dff0`（纯前端改动，且 `048dff0` 晚于 `c6a7a84`）。这一点必须明确：不能只用 exe 哈希证明前端修复已包含。

### 2.2 受测前端（有可追溯记录，但不是构建产物）

受测前端由 vite dev server 在 `http://127.0.0.1:1422` 实时编译源码提供，**不存在 `dist` 构建产物**（已确认 `apps/desktop/dist` 不存在），因此没有前端 bundle 的哈希可引用。

可追溯的替代绑定：

| 项 | 值 |
| --- | --- |
| 受测源码提交 | `048dff0` |
| `workbench.tsx` git blob | `cee35ca6f48c7d7854788074eae6c53d83e54dcb` |
| `workbench.tsx` raw SHA256 | `38ff560ab71e1035db7a0e7acbc386204ffa42cdf542de41f50afe3776f31194` |
| `apps/desktop/src` 树对象 | `da6e00baaaaaef9e56b79d812a7597271e1c26b3` |

时间线证据（这是“受测源码 = `048dff0`”的实际依据）：

- `048dff0` 提交于 **13:29:25**
- 主样本 Run `af77d0bf` 创建于 **13:29:53**，后继 `d2be69af` 创建于 **13:31:38**
- 运行结束后 `git status` 对该文件无改动；`workbench.tsx` 的 git blob 与 `048dff0` 一致

**缺口（必须记录，不推定为完全一致）**：受测期间 vite 处于 HMR 模式，日志显示 13:24:58 / 13:25:06 / 13:25:25 三次 `hmr update ... workbench.tsx`。也就是说，13:25 前后存在**未提交的中间状态**被浏览器加载过。主样本闭环全部发生在 13:29:53 之后、且源码此后无改动，因此主样本可绑定到 `048dff0`；但**同一会话内更早的探索性操作（13:24–13:29）不能声称绑定到某一提交**。结论：主样本有效，中间探索不作为证据。

## 3. 当前 UI 分支缺失项复核（实测）

UI HEAD = `59c31ff`。以下为 `git grep` 在 `ui/improvements-20261009` 上的实测结果：

| 检查项 | 结果 |
| --- | --- |
| `DeliveryReviewPanel` | 无任何匹配 |
| `awaiting_delivery_review` | 无任何匹配 |
| `DeliveryAcceptanceCard` | 无任何匹配 |
| `DeliveryAcceptance`（全仓任意路径） | 无任何匹配 |
| `resolveDeliveryAcceptance`（`desktop-client.ts`） | 无任何匹配 |
| `DeliveryAcceptanceState`（`types.ts`） | 无任何匹配 |

新 UI 侧完全没有这些代码。

**但必须区分“UI 分支自身缺失”与“合并后是否缺失”**——两者结论不同，实测如下：

合并结果树 `6d2577ca308498ec85da0f8039b5dc390f9a1f55`（`git merge-tree --write-tree 候选 UI`）中：

| 项 | 合并结果 |
| --- | --- |
| `DeliveryAcceptanceCard.tsx` / `.test.tsx` | **存在**（候选侧带来，UI 未删除，只是从未有过） |
| `desktop-client.ts` 的 `resolveDeliveryAcceptance` | **存在** |
| `types.ts` 的 `DeliveryAcceptanceState` | **存在**（2 处） |
| `workbench.tsx` 的 `DeliveryReviewPanel` | **存在，但位于冲突块内**（`:2547`，属候选一侧） |

因此准确说法是：**缺失发生在“解决冲突”这一步，而不是文件层面**。`workbench.tsx` 有 3 处冲突块（`:1–123`、`:197–4251`、`:5552–5561`），`DeliveryReviewPanel` 的定义（`:2547`）和挂载（`:5554` 的 stage 行）都落在冲突的候选一侧。若解决冲突时取 UI 一侧，面板与挂载会被静默丢弃，而 `DeliveryAcceptanceCard.tsx` 会作为孤儿文件留下——文件在、引用没了。这是本次迁移最需要防的失败模式。

冲突实测：仅 2 个文件内容冲突
- `apps/desktop/src/features/chat/workbench.tsx`（3 个冲突块）
- `apps/desktop/src/styles/workbench.css`

其余（`desktop-client.ts`、`types.ts`、`Cargo.toml`、`lib.rs`）自动合并。

## 4. 旧逻辑 → 新组件迁移表

迁移源均在候选 `048dff0`；迁移目标为 UI 分支现有结构。

| # | 能力 | 迁移源（候选 `048dff0`） | 迁移目标（UI `59c31ff`） |
| --- | --- | --- | --- |
| 1 | 待审面板骨架 | `workbench.tsx:3337` `function DeliveryReviewPanel()`（`parked` 判据在 `:3339`） | 新建 `features/chat/components/DeliveryReviewPanel.tsx`，挂载在 `fox-chat-stage` 内、composer 之前 |
| 2 | 面板挂载点 | `workbench.tsx:6244` `<DeliveryReviewPanel />` | `fox-timeline.tsx` / `workbench.tsx` 的 stage 容器，等价位置 |
| 3 | 逐项接受卡 | `features/chat/components/DeliveryAcceptanceCard.tsx`（189 行，独立文件可直接搬迁） | 同路径新建 |
| 4 | 接受 | `DeliveryAcceptanceCard.tsx:173` `act('accept')` → `resolve(...)` | 同上，逻辑不变 |
| 5 | 拒绝 | `:179` `act('reject')` | 同上 |
| 6 | 撤销接受 | `:185` `act('revoke')` | 同上 |
| 7 | 读取接受状态 | `workbench.tsx:3395` `loadState` → `desktopClient.deliveryAcceptanceState` | 合并后方法已在 `desktop-client.ts`，只需在 `workbench.tsx` 冲突解决时保留调用 |
| 8 | 提交接受决定 | `workbench.tsx:3396` `resolve` → `desktopClient.resolveDeliveryAcceptance` | 同上 |
| 9 | 客户端方法 | `desktop-client.ts:799` `resolveDeliveryAcceptance` | **合并自动保留**，无需重写 |
| 10 | wire 类型 | `model/types.ts:584` `DeliveryAcceptanceState` | **合并自动保留**，无需重写 |
| 11 | 识别问题入口 | `workbench.tsx:3384` `.fox-delivery-review__question`，取自 `finding.reason` | 并入 #1 面板 |
| 12 | `recognition:` 行不渲染成可接受卡 | `workbench.tsx:3345` 起 `recognition` 过滤 + 注释说明 | 迁移时保持该过滤，勿把该行渲染为可接受项 |
| 13 | 等待时可发送 | `workbench.tsx:5250-5259` `desktopParkedForReview` / `desktopBusy`；`:5381` chatState；`:6244` `runtimeRunning={desktopBusy}`、`steeringActive={... desktopBusy}` | 新组件的新 composer（`composer/fox-composer.tsx`）需等价实现：停车待审不得置为 streaming，否则提交控件变 Stop |
| 14 | 样式 | `workbench.css:12126` 起 `.fox-delivery-review`、`:12169` 起 `.fox-delivery-acceptance`（约 160 行） | 迁入 UI 分支对应样式文件 |

### 4.1 必须一并迁移的四类操作（不可只迁外观）

- **接受**：`accept`，仅 `unconfirmed` 类可接受，且不得表述为“核验通过”
- **拒绝**：`reject`
- **撤销**：`revoke`
- **继续处理**：实测结论——候选前端**没有**独立的“继续”按钮；`run_resume` 仅服务 yuxi 类型（`commands/mod.rs:2702`）。kernel 待审 Run 的“继续”实际是**用户发送新消息启动新 Run**（本轮闭环正是如此：`af77d0bf` → `d2be69af`）。因此迁移项 #13 就是“继续处理”的落地方式，新 UI 必须保证该路径可用，而不是去找一个不存在的 continue 按钮。

另外，Run 已结束时面板仍需保留（以便撤销已有接受），该分支逻辑在 `workbench.tsx:3322` 注释与 `:3360` 的 `if (!parked && ...) return null` 条件中。

## 5. 迁移后必须复跑的步骤（以已有闭环为基准）

基准：`output/_mc-clarify/evidence/CLARIFICATION-LOOP.md` + 四份 JSON，会话 `7b378dcc`。新 UI 集成后，以下步骤需在新 UI 上重跑，且必须重新绑定构建身份：

| 步骤 | 期望（基准值） |
| --- | --- |
| 1 | 模糊请求「帮我生成一份报告」启动 Run |
| 2 | Run 进入 `awaiting_delivery_review`；`terminal_written=0`；末事件 `run.awaiting_delivery_review`；checklist 仅 `recognition:delivery_unresolved`；无无目标重复修复 |
| 3 | 新 UI 上面板显示识别问题；composer 可提交（非 Stop）；经真实 UI 提交补充 |
| 4 | 后继 Run 重新识别为 `file:<name>`（基准为 `file:summary.md`），`status=passed`，`boundPath` 与 `checkedHash` 与磁盘一致 |
| 5 | 旧 Run 仍 `awaiting_delivery_review` 且其 `recognition:` 行 `pending`；后继不继承该行；先前产物未被覆盖 |
| 6 | 普通交付接受三段：待审 → 接受/拒绝 → 撤销；Run 结束后仍可撤销 |

不得因已有闭环通过，就将新 UI、B4、B5b 或全量测试记为通过。新 UI 上未复跑前，新 UI 视为**未验证**。

## 6. 本轮结论

- 提交分类已按 21+5+1+1=28 修正并实测自洽（上一轮的“22 个前置”已更正为 21）。
- 后端身份可精确绑定（`c6a7a84` + exe 哈希）；前端受测身份可用“提交 + blob + 时间线”绑定到 `048dff0`，但**无构建产物哈希**，且 13:24–13:29 的 HMR 中间态不可绑定——此缺口已记录，不推定为完全一致。
- 缺失要分层看：UI 分支自身无任何交付代码；但合并后 `DeliveryAcceptanceCard`、客户端方法、wire 类型会自动保留，**真正会丢的是 `workbench.tsx` 冲突块里的面板定义与挂载**——若解决冲突取 UI 一侧，面板静默消失而卡片文件变孤儿。
- 迁移表覆盖接受/拒绝/撤销/继续处理、识别问题入口、等待时可发送行为（`048dff0`）。
- 本轮未合并、未改业务代码、未扩大后端修复范围；新 UI、B4、B5b、全量测试均未标记为通过。
