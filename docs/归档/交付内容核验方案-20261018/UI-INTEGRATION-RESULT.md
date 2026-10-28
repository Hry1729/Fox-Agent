# UI 集成结果：交付审查行为迁移

本轮为隔离集成，不合入 main、不推送、不正式启用。以下结论均来自当前工作树实测。

## 1. 基线核对

开工前实测，两侧均无新提交，与准备报告一致：

| 对象 | HEAD |
| --- | --- |
| `main` | `0df9162`（未动） |
| `origin/main` | `0df9162`（未推送） |
| 完整候选 `merge-candidate/fox-closeout` | `048dff0` |
| UI 分支 `ui/improvements-20261009` | `59c31ff` |

本轮的完整候选仍是 21 前置 + 5 修复 + 1 合并 + 1 输入框修复 = 28 个提交，未扩大。

## 2. 集成工作树与提交

| 项 | 值 |
| --- | --- |
| 工作树 | `D:\python\projects\Fox\worktrees\ui-integration` |
| 分支 | `integration/ui-delivery-review` |
| 合并提交 | **`9cecdea`** |
| 父 1（UI） | `59c31ff` |
| 父 2（候选） | `048dff0` |
| 相对父 1 | 94 文件，+37,663 / −130 |
| 相对父 2 | 561 文件，+100,782 / −9,867 |

未提交的其余改动：无业务代码改动；仅本报告与证据文件（在 `output/` 下，不在仓库内）。原候选的换行符差异（`Cargo.toml`、`gen/schemas/*.json`）只在原工作树存在，本工作树未受其影响，也未重置。

## 3. 冲突解决方式

冲突文件恰好 2 个，与准备报告一致；其余自动合并。行号仅作定位，实际合并结果为准。

### 3.1 `apps/desktop/src/features/chat/workbench.tsx`（3 个冲突块）

| 块 | 内容 | 解决方式 |
| --- | --- | --- |
| 1 | import 区 | 合并两侧。保留 HEAD 侧 import，补回候选独有且存活代码实际使用的 `useProjectCatalog`、`RuntimeArtifacts`、`asAgentCard`。未整块取任一侧。 |
| 2 | 约 4,050 行旧单体代码 | 取 UI 侧。该块是旧时间线、输入框、侧栏、右键面板的单体实现，UI 分支已抽成 `timeline/`、`composer/`、`shell/`、`right-panel/` 独立组件。两侧全取会导致每个组件重复定义一次。 |
| 3 | 聊天舞台挂载点 | 取 UI 侧，并显式插入 `<DeliveryReviewPanel />`，位置在 `assistantUiThreadPanel` 与 `composerPanel` 之间。 |

**块 2 的取舍不是"取一侧即完成"。** 丢弃该块会连带丢掉只存在于候选侧的内容，实际丢弃了 `DeliveryReviewPanel` 本体——块 2 内它只是定义，不在块 3 的挂载行里。因此从候选原文件按真实边界（文档注释起 `:3318` 至函数结束 `:3400`，共 83 行）取出该组件，重新插入集成文件。这是本轮唯一需要人工搬运的代码块。

顺带发现并修正的两处接线：

- 迁移块使用 `useContext`，UI 分支的 React import 未包含它 → 补入。
- 组件用到 `ShieldAlert`、`ShieldCheck`（lucide）与 `DeliveryAcceptanceCard` → 补入 import。

### 3.2 `apps/desktop/src/styles/workbench.css`（1 个冲突块）

**两侧全部保留。** 两个冲突区声明的是不同选择器：HEAD 侧为 `.fox-topbar-toggle`、`.fox-animate-file-tree`（UI 的新控件样式），候选侧为 `.fox-delivery-review`、`.fox-delivery-acceptance`（交付审查样式）。二者不是替代关系，取任一侧都会丢失另一侧功能。合并后两者各出现 1 次。

### 3.3 自动合并、直接复用（未重复实现）

`DeliveryAcceptanceCard.tsx` / `.test.tsx`、`desktop-client.ts` 的 `resolveDeliveryAcceptance` 与 `deliveryAcceptanceState`、`types.ts` 的 `DeliveryAcceptanceState`、`kernel-snapshot.ts` 全部自动合并，按任务要求直接复用。

## 4. 14 项迁移清单完成情况

| # | 能力 | 状态 | 依据 |
| --- | --- | --- | --- |
| 1 | 待审面板骨架 | 已迁移 | `workbench.tsx` 中 `function DeliveryReviewPanel()` 存在 1 处 |
| 2 | 面板实际挂载 | 已迁移 | `<DeliveryReviewPanel />` 1 处，位于舞台内、composer 之前；**真实 UI 已渲染**（见 §5.3） |
| 3 | 逐项接受卡 | 已迁移 | `<DeliveryAcceptanceCard` 1 处，非孤儿文件 |
| 4 | 接受 | 已迁移 | `DeliveryAcceptanceCard.tsx` 中 `'accept'` |
| 5 | 拒绝 | 已迁移 | `'reject'` |
| 6 | 撤销 | 已迁移 | `'revoke'` |
| 7 | 读取接受状态 | 已迁移 | `loadState` → `desktopClient.deliveryAcceptanceState`（自动合并） |
| 8 | 提交接受决定 | 已迁移 | `resolve` → `desktopClient.resolveDeliveryAcceptance` |
| 9 | 客户端方法 | 直接复用 | `desktop-client.ts` 自动合并，未重写 |
| 10 | wire 类型 | 直接复用 | `types.ts` 自动合并，未重写 |
| 11 | recognition 问题入口 | 已迁移 | `.fox-delivery-review__question` 渲染 `finding.reason`；**真实 UI 已显示**（见 §5.3） |
| 12 | recognition 与可接受项过滤边界 | 已迁移 | `:161` 单独筛 `recognition:`，`:165` 可接受项额外排除 `recognition:`；`passed` 项不进可接受列表（`:165` 的 `status !== 'passed'`） |
| 13 | 等待时可输入、可发送 | 已迁移 | `desktopParkedForReview` / `desktopBusy`（`:344-353`）；`chatState` 用 `desktopBusy`（`:475`）；composer `steeringActive` 用 `desktopBusy`（`:1410`）。**真实 UI 已验证**（见 §5.3） |
| 14 | 样式 | 已迁移 | `.fox-delivery-review` 与 `.fox-delivery-acceptance` 各 1 处 |

补充说明（对应任务点名的行为）：

- **已完成 Run 的接受状态与撤销入口**：代码路径在位。`mayDecide = !state.runTerminal`；Run 结束后未接受项显示「任务已结束，未对该项作出接受」，已接受项保留「撤销接受」。面板对 `completed`/`failed` 仍渲染（`:175` 的放行条件），不会因 Run 结束而整块消失。此项为**源码在位**，未在真实 UI 上取得实测样本（原因见 §6）。
- **继续处理（新消息创建后继 Run）**：kernel 侧未接 yuxi 专用的 `run_resume`。本轮真实闭环正是「发新消息 → 新 Run」（`f722681e` → `181252d4`），已在真实 UI 上验证。
- **正在执行与仍属活动 Run 的区别**：保留 `desktopRunning` 供专家/工具锁使用（`:376`），仅 composer 与 steering 收窄为 `desktopBusy`。未把两者合并。

## 5. 受测身份与真实 UI 证据

### 5.1 后端身份

| 项 | 值 |
| --- | --- |
| 构建来源 | 集成提交 `9cecdea` |
| 构建目录 | `output\_ui-integration-target` |
| exe | `debug\fox-desktop.exe` |
| exe SHA256 | `C78A612EB8D25CE94FCC6BA4F0CF84BF6B07CB6EC49384461D219A8181002F6D` |
| 构建时间 | 2026-10-18 14:18:37 |
| 构建结果 | exit 0（184 warnings，均为既有） |

该哈希与上一轮候选 exe（`6D373C1D…`）不同，因为本次包含 UI 分支与集成改动，属预期。

### 5.2 前端身份

| 项 | 值 |
| --- | --- |
| 形态 | Vite 6.4.3 dev server，`http://127.0.0.1:1422/`，profile 模式 |
| 无独立构建产物 | `apps/desktop/dist` 不存在，因此无 bundle 哈希 |
| 源码提交 | `9cecdea` |
| `workbench.tsx` blob | `7015ecd12dc4c2d8a317597b802b5fded7377ff2` |
| `workbench.css` blob | `bbbcfa0382aa6012a90ae0a45ecb6ed07e078a44` |

**受测源码冻结情况**：本轮全程未修改前端源码，前端源码始终为 `9cecdea` 的干净状态，未发生需要重新绑定与复跑的 HMR 改动。证据：验证期间 `git status` 对 `apps/desktop/src` 无改动。

### 5.3 真实 UI 验证（隔离环境）

隔离条件：独立 `FOX_DATA_DIR=output\_ui-integration\data`，独立项目目录 `output\_ui-integration\project`，端口 1422/9333；未触碰用户实例（用户 DB 未写入）、未复制密钥（模型服务复用系统凭据管理器中的既有条目）。

模型服务：通过真实 `model_service_save` 复用既有 SenseNova 配置，`model_service_test` 返回 `status: connected`、延迟 57ms、7 个模型。未新建 provider。

**（a）真实澄清闭环——通过**

会话 `205cc4e8`：

| 步骤 | 实测 | 结果 |
| --- | --- | --- |
| 1 模糊请求「帮我生成一份报告」 | Run `f722681e` | 通过 |
| 2 识别失败并停车 | `state=awaiting_delivery_review`，`terminal_written=0`，末事件 `run.awaiting_delivery_review`，清单仅 `recognition:delivery_unresolved`（`pending`），无无目标重复修复 | 通过 |
| 3 新 UI 显示问题且可发送 | 面板真实渲染，标题「交付要求未识别——需要你补充」；composer 提交控件为 **Submit**（非 Stop）；经真实鼠标事件点击提交 | 通过 |
| 4 后继 Run 重新识别并完成 | Run `181252d4` → `completed`，`terminal_written=1`；新契约 `file:q3-summary.md`，`status=passed` | 通过 |
| 5 旧契约保留、不继承、不覆盖 | `f722681e` 仍 `awaiting_delivery_review` 且 recognition 行仍 `pending`；后继 `inherited_from_run_id` 为 `None`（未继承 recognition）；先前产物未被覆盖 | 通过 |

文件身份一致：`checkedHash` = `fea3251f9ffad2ace956c2cf8dc10b63ce3f56b9409700de75c3bbf8ea87b68b`，与磁盘 `project\q3-summary.md` 实际 SHA256 完全一致；`checks` 四项均 `passed`。

**（b）普通交付审查样本 A（未确认项接受 → 完成 → 撤销）：未验证**

**（c）样本 B（拒绝及对应结束状态）：未验证**

原因见 §6，属于**未覆盖**，不计为通过，也不计为产品失败。

## 6. 样本 A / B 未能覆盖的原因

要在真实 UI 上验证接受/拒绝/撤销，必须先有一个「未确认」类的交付项。本轮用 5 个真实 Run 尝试触发，全部未产生该类别：

| Run | 输入意图 | 结果 |
| --- | --- | --- |
| `f722681e` | 模糊请求 | `recognition:delivery_unresolved`（停车，非未确认） |
| `181252d4` | 澄清后明确文件名 | `file:q3-summary.md` = `passed` |
| `29017f0c` | 「格式由你决定，只要不是空的」 | `file:out%2faccept-test.md` = `passed` |
| `fd2bfcfd` | 三节结构、每节三条 | 完成，无交付清单行 |
| `1d45bb94` | 「交付已有文件，不需重写」 | 完成，无交付清单行 |

实测约束：本实例 **`FOX_DELIVERY_V2` 未设置，v2 类型链默认关闭**。而「未确认」类的产生路径（`p2c2_execution.rs` 的 `CheckedHash::Absent`、`EvidenceUnavailable`，及 `p2b_decision` 的 UNCONFIRMED 事实）在真实业务中需要「产物缺少可核验回执哈希」等条件；模型真实写出文件时总会带写回执，因此走进 `passed`。

按本轮授权「不扩大识别规则或后端修复范围」「不正式启用」，我没有启用 v2 开关或改动判定规则去凑出该类别。因此接受/拒绝/撤销**只做到源码在位与组件非孤儿，未取得真实 UI 实测**。

这是本轮最重要的未完成项，不能以「41 个测试函数」替代——任务已明确禁止。

## 7. 通过 / 失败 / 未验证

**通过（真实 UI，隔离环境）**

- 集成编译：`tsc --noEmit` exit 0；`cargo build` exit 0。
- 面板真实挂载并渲染（不是孤儿组件）。
- recognition 问题入口显示正确文案。
- 等待时可输入、可发送（提交控件为 Submit，非 Stop）——048dff0 等价行为在新 UI 成立。
- 真实澄清闭环五步全过，含旧契约保留、不继承 recognition 行、文件哈希一致、旧产物未覆盖。
- 新消息创建后继 Run 的继续处理路径成立，未接 yuxi 的 `run_resume`。
- CSS 两侧样式并存，无丢失。

**未验证**

- 样本 A：未确认项接受、完成后仍可撤销。
- 样本 B：拒绝及对应结束状态。
- 已完成 Run 的接受状态展示与撤销入口的**实测**（仅源码在位）。
- 加载状态、错误状态的实测（仅源码在位）。

**失败**

- 无。本轮未发现由迁移直接引入的缺陷。

**不属于本轮范围、状态保持不变**

- B4、B5b、全量测试：均未运行，既有状态不变，不标记为通过。
- 识别对措辞敏感（`请交付到项目根目录下的 X` 不识别）：既有行为，本轮未改。
- `fd2bfcfd` 的一次 `tool.failed`：该 Run 仍正常完成，未复现，未归因。

## 8. 保护与清理

- 未合入 main、未推送：`main` 与 `origin/main` 仍为 `0df9162`。
- 未改动原候选、UI、main 及其他任务工作树；未重置任何东西。其他任务 WIP（`b4-h1m1-fix` 3 项、`b4-diagnosis-fix` 3 项、`d1-empty-body-fix` 13 项、UI 的 `.workbuddy/`）保持原状。
- 未操作用户业务实例，未复制密钥，未修改用户数据。
- 磁盘：构建时 D 盘剩余 0.6 GB 导致一次 `os error 112` 失败。已将取证用的 5 个既有 exe 复制到 `output\_mc-keep`（`fox-desktop.exe` 哈希仍为 `6D373C1D…`，与上轮一致），随后删除可重建的 `output\_mc-target`，释放约 14.3 GB。未删除任何源码或工作树。
- 本轮新建的临时驱动脚本（`_*.mjs`）位于集成工作树 `apps/desktop/`，未提交。

## 9. 是否具备进入「完整问题卡设计」的条件

**后端与迁移侧：具备。** 面板、接受卡、客户端方法、类型、样式均已在新组件结构中就位，真实澄清闭环在新 UI 上通过。

**验收侧：不完全具备。** 接受/拒绝/撤销三个动作在新 UI 上没有任何真实运行证据，而完整问题卡设计必然会复用同一套「问题 → 用户决定 → 落库」链路。在这些动作取得真实样本前，问题卡设计的交互细节（尤其"接受不是核验通过"的措辞边界、撤销与终态关系）缺少可对照的行为基准。

建议下一步之前先补齐一件事：在**不减配**的前提下，为该实例取得一个合法的「未确认」交付样本（例如在明确的验收环境下临时启用 v2 链并记录该开关状态，或由维护者提供一个已知可产生未确认项的输入），然后跑完样本 A/B。这一步不需要改代码，只需要一个能产生该类别的真实条件。

在此之前，本轮集成只能定位为「行为迁移已完成、接受动作待实测」的隔离候选，**不应推进到主线合并**。
