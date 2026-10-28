# UI 接受/拒绝补充验收（样本 A / B）

本轮只补齐样本 A、B，未重做集成、未改动后端核验规则、未合入 main、未推送。

## 1. 基线核对

| 项 | 执行前实测 |
| --- | --- |
| 工作树 | `D:\python\projects\Fox\worktrees\ui-integration` |
| 分支 | `integration/ui-delivery-review` |
| HEAD | `9cecdea`（与预期一致） |
| 工作树状态 | 干净（`git status --porcelain` 为空） |
| 其他任务工作树 | 未触碰，未重置 |

## 2. 身份

### 2.1 后端

| 项 | 值 |
| --- | --- |
| 源码提交 | `9cecdea` |
| exe | `output\_ui-integration-target\debug\fox-desktop.exe` |
| exe SHA256 | `EE75057EC78D05C9D63DE6917FF9C50EFC258D40AFD95A3BA8633035FCBC0552` |
| 构建时间 | 2026-10-18 14:44:49 |

说明：上一轮报告记录的 `C78A612E…` 是手工 `cargo build` 的产物。本轮用 `tauri dev` 启动，它会重新链接同一份源码，因此 exe 哈希变为 `EE75057E…`。源码未变（`git status` 对 `apps/desktop/src-tauri` 零改动），两者都构建自 `9cecdea`。

### 2.2 前端

| 项 | 值 |
| --- | --- |
| 形态 | Vite 6.4.3 dev server，`http://127.0.0.1:1422/`（profile 模式） |
| 源码提交 | `9cecdea` |
| `workbench.tsx` blob | `7015ecd12dc4c2d8a317597b802b5fded7377ff2` |
| 轮次内改动 | 无（验收全程 `apps/desktop/src` 保持干净） |

### 2.3 开关状态

| 项 | 值 |
| --- | --- |
| 变量 | `FOX_DELIVERY_V2=1` |
| 作用域 | 仅本验收进程的启动环境（进程级，不持久化） |
| 生产默认值 | **未改**（代码默认关闭） |
| 用户环境变量 / 用户实例配置 | **未改** |
| 校验 | 生效值 `1`，属代码支持的开启值之一（`1`/`true`/`on`/`yes`；其他值 fail-closed 作关闭） |

## 3. 合法样本的形成

### 3.1 先确认入口真的冻结了 v2

不能对任意文件请求假定进入 v2。本实例 `allows_v2` 只认 `Contract::Concat`，而 `Concat` 需要任务文本同时满足一组严格条件（`freeze_concat`）：含「合并」+「大小/字节」、明确「文件名升序 / 空行 / 完整文件名」、平手规则、`选出 N 个`、方向、字面 `.md 文件`、以及 `在 "目录" 目录` 形式的输入范围。

前四次尝试都被 `freeze_concat` 拒绝，冻结为 `unbound`，我按实际 reason 逐条修正措辞：

| 尝试 | 冻结结果 | reason（来自库内 spec） |
| --- | --- | --- |
| A1 `md 文件` | `unbound` | 输入文件类型未明确 |
| A2 `.md 文件` | `unbound` | 选择子句的输入目录未明确 |
| A3 `把 "sources" 目录` | `unbound` | 同上（正则要求 `在 "dir" 目录`） |
| A4 `在 "sources" 目录` | **`concat`（v2）** | — |

A4（`48bd173a`）的 spec 键为 `checkerVersion` / `envelopeVersion` / `mergeRule` / `policy` / `rule` / `ruleId` / `schemaVersion`，`type=concat` —— 这是 v2 已冻结的直接证据。

**同实例对照**：全部交付行中 7 个 spec 带 v2 信封，3 个仅 v1（非 `Concat` 契约）。允许清单在真实数据上成立，不是对所有契约一律开 v2。

### 3.2 合法 Unconfirmed 的真实来源

依据 `H-P2E-independent-review-result.md` §3.1 与 `G-P2E-production-wiring-result.md` §4.1，并逐条核查当前源码后确认，合法 Unconfirmed 来自**空但确实发生的受管写入**，属真实能力边界而非伪造：

- `p2b_decision.rs:475-480`：`Ok(bytes) if bytes.is_empty()` → `DecisionFact::unconfirmed(EVIDENCE_UNAVAILABLE, "候选产物为空，没有可核验内容")`。
- `p2c2_execution.rs:240-241` 用 `classify_read_result` 读候选；`reads` 非空使 L345 的 `reads.is_empty()` 不成立，叶算法被跳过、`concat.all_sources` 义务未履行 → 覆盖门给出 `unconfirmed`。
- 受管写入仍产生真实回执，`checkedHash` 是**非空的空摘要** `e3b0c442…`，与磁盘复算一致，因此**无漂移**。

**未做的事**：未手改 SQLite、未改合同/事件/接受记录、未移除回执、未伪造哈希、未强制设 Unconfirmed、未把明确失败降级、未把 recognition 行当作可接受项。

### 3.3 唯一被采纳的样本形态

我保留了两个同形态的合法样本（A 与 B 各自独立），均为：`checkedHash = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`、磁盘 0 字节、spec 带 v2 信封、事件 `run.awaiting_delivery_review`、行 `status=pending`。

被**弃用**的两种形态（都不是合法可接受样本，如实记录）：

- `5f78f192`（`file:samplec.md`）：产物根本未写出，事实为 `checkUnbound`「内容核验没有绑定真实产物」，`checkedHash` 为空。UI 卡片自带提示「候选文件当前无法在授权范围内读取，接受有效性未确认」。未用于 A/B。
- `48bd173a` / `97975e4e` / `2399ff0f`：模型写成非空或非空后又被改动，判为 `refused` 或 `repairNeeded`。未用于 A/B。

## 4. 样本 A：接受 → 完成 → 撤销

样本 Run `4e5f5fd2`（会话 `28908c7d` / ACC-A9，项目 `output\_ui-acc-ab\project`）。

| 步骤 | UI 实测 | 数据库只读证据 |
| --- | --- | --- |
| 1 停车 | 面板「交付内容尚未核验」；卡片 `sampleE.md 内容尚未核验`、`核验结果 unconfirmed`、`当前接受 未决定`，按钮 `接受未核验风险` / `判定不可接受` | `state=awaiting_delivery_review`，`terminal_written=0`；`checkedHash=e3b0c442…`；`status=pending`；`verdict.class=unconfirmed` |
| 2 点击接受 | 卡片变为「用户已接受未核验风险 — **这不是核验通过**」，`核验结果 unconfirmed`，`当前接受 已接受（window:main）` | `accepted_at=1792308288909`、`accepted_by=window:main`、`acceptance_class=unconfirmed`、`candidate_sha256=e3b0c442…`；audit `accepted`「用户接受未核验交付，不代表核验通过」 |
| 3 满足其余条件后完成 | **同一时刻**面板标题变为「交付内容尚未核验——任务已结束」，副文案「该任务的核验结论保留原样。你此前的接受仍在，可以逐项撤销；任务不会因此重启或重跑」 | `state=completed`、`terminal_written=1`；事件 `seq31 run.awaiting_delivery_review` → `seq32 run.completed` → `seq33 run.delivery_accepted` |
| 4 完成后仍可见接受记录与撤销入口 | `当前接受 已接受（window:main）`，按钮为 **`撤销接受`**（面板未因 Run 结束而消失） | 面板对 `completed` 放行；`accepted_at>0` 且 `revoked_at is null` → `live_acceptance` 成立 |
| 5 真实 UI 撤销 | 点击 `撤销接受` → `当前接受 未决定`，并显示「任务已结束，未对该项作出接受」，按钮消失；`核验结果 unconfirmed` 不变 | `revoked_at=1792308314957`、`revoke_reason=用户撤销接受`；audit 追加 `revoked`「用户撤销此前对未核验交付的接受」，先前的 `accepted` 记录保留 |
| 6 撤销不改写终态、不自动启动 | 面板仍为「任务已结束」，无新 Run | `state` 仍 `completed`、`terminal_written=1`；该会话**只有** `4e5f5fd2` 一个 Run，撤销未派生新 Run |
| 7 仍保留未确认语义 | `核验结果 unconfirmed`，卡片文案明确「不会把它改成"已通过"」 | `status=pending`（非 `passed`）、`acceptance_class=unconfirmed`、`verdict.class=unconfirmed` |

截图：`A-01-before-accept.png`（接受前）、`A-02-completed-with-revoke.png`（已完成且撤销入口可见）。
结构化快照：`A-sample-run.json`（停车时，32 事件）、`A-completed-accepted.json`（完成且已接受，33 事件）。

**A 通过。**

## 5. 样本 B：拒绝

样本 Run `5817e43b`（会话 `dce35279` / ACC-B1）。与 A 相互独立：不同的源目录（`sourcesB`）、不同的产物（`sampleB.md`）、不同的 Run 与会话。

| 步骤 | UI 实测 | 数据库只读证据 |
| --- | --- | --- |
| 1 停车 | 卡片 `sampleB.md 内容尚未核验`、`核验结果 unconfirmed`、`当前接受 未决定` | `state=awaiting_delivery_review`、`terminal_written=0`、`checkedHash=e3b0c442…`、`status=pending` |
| 2 点击「判定不可接受」 | 卡片变为 `当前接受 已拒绝`，操作按钮消失；`核验结果 unconfirmed` 不变 | `rejected_at=1792307524769`、`accepted_at=0`、`revoked_at=None`；audit `rejected`「用户判定该交付不可接受」 |
| 3 对应结束状态 | 面板「交付内容尚未核验——任务已结束」 | `state=failed`、`terminal_written=1`；事件 `seq24 run.awaiting_delivery_review` → `seq25 run.failed` |
| 4 重新打开会话 | 仍显示 `当前接受 已拒绝`、无可用按钮、`核验结果 unconfirmed` | 记录未变；行 `status=pending`、`verdict.class=unconfirmed`（未被伪造成通过） |

截图：`B-01-before-reject.png`。结构化快照：`B-rejected.json`。

**B 通过。**

## 6. 通过 / 失败 / 未验证

**通过**

- 仅在隔离验收进程临时启用 `FOX_DELIVERY_V2=1`，生产默认、用户环境、用户实例均未改。
- 正常入口确实冻结了 v2 合同（spec 带 `envelopeVersion`/`checkerVersion`/`ruleId`/`policy`），且同实例另有 3 个 v1-only spec 证明允许清单生效。
- 合法 Unconfirmed 由真实受管空写入产生，未做任何数据或规则伪造。
- A：接受 → 同刻完成 → 完成后撤销入口仍可见 → 真实 UI 撤销 → 有效接受失效、审计保留、终态未改、无自动重启、语义仍为未确认。
- B：拒绝 → `failed` 结束 → 持久 `rejected_at` 与审计 → 重开仍显示已拒绝。
- 两个样本相互独立，均为真实 UI 点击（`Input.dispatchMouseEvent` 命中真实按钮），未以直接调用后端命令代替 UI 操作，未用既有单元测试代替实测。

**失败**

- 无迁移直接造成的缺陷。

**未验证 / 记录项**

- 后端 exe 由 `tauri dev` 重建，哈希与本轮上文记录不同（源码同提交），已在 §2.1 说明。
- 已完成 Run 若之后又产生后继 Run，`conversation_load` 的 `lastRun` 会指向后继 Run 且 `deliveryChecklist` 为空（`delivery_checks.rs:147-151` 按单 Run 取行），面板随即不再显示旧 Run 的接受记录。此行为在候选实现中**逐字相同**（候选 `workbench.tsx:6006` 同为 `lastRun?.id`），属既有设计，不是本轮迁移引入；本轮 A 的时序（接受即完成、不产生后继 Run）下撤销入口正常可见。
- 已完成面板的副文案「你此前的接受仍在，可以逐项撤销」在**已拒绝**项上并不贴切（已拒绝项无可撤销的接受）。该文案与候选逐字一致（候选 `:3376`），属既有措辞，按「不扩大范围」只记录不修改。
- 识别措辞敏感性仍在：`freeze_concat` 对「在 "dir" 目录」「.md 文件」等字面形式敏感，四次措辞调整才冻结成功。既有行为，未改。

## 7. 边界与清理

- 不合入 main、不推送：`main` / `origin/main` 仍为 `0df9162`。
- 不实现完整问题卡；B4、B5b、全量测试状态不变，未运行、未标记通过。
- 未触碰其他任务工作树，未操作用户实例，未复制或输出密钥（模型服务复用系统凭据管理器既有条目，`credentialConfigured=true`，`model_service_test` 返回 `connected`，全程未打印密钥）。
- 独立数据目录 `output\_ui-acc-ab\data`、独立项目目录 `output\_ui-acc-ab\project`、端口 1422/9333。
- 证据保留在 `output\_ui-acc-ab\evidence\`（3 张截图 + 4 份结构化快照）。
- 本轮创建的隔离实例在验收结束后关闭，端口释放。

## 8. 结论

交付审查迁移验收**已完成**。此前唯一缺口——接受/拒绝/撤销在真实 UI 上无实测证据——现已补齐：A 覆盖「接受 → 完成 → 撤销」全链，B 覆盖「拒绝 → 结束」。两者语义均保持「未确认不等于通过」，撤销只失效接受记录而不改写历史终态、不自动重启。

可以进入下一步「完整问题卡设计」。需要带走的两条既有事项（非本轮缺陷、不阻塞）：`lastRun` 只投影单个 Run 的清单所导致的旧记录可见性边界；以及 `freeze_concat` 的措辞敏感性。
