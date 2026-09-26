# F3 逐轮全量超时：根因取证（独立复现与时间线）

> **本文件已由修正卡更新。** 第一次取证提交（`34693d1`）的诊断代码本身污染了取证：它吞掉了 Host 派发错误、在 deadline 断言里持有已中毒的事件锁，
> 并在关闭开关时仍维护计数表。这些缺陷已由修正卡全部移除并复验，详见第 10 节"对上一份报告的更正"。
> 第 1–9 节保留原始观测事实（其结论未因此改变），第 10–12 节为修正后的证据、负例结果与最终状态。

本卡目标：查清全量运行中 `f3_round_conversation_grant_reuse` 与 `f3_round_read_only_ask` 在本地 Provider 20 秒绝对上限处失败、
"第四次请求为何没有在期限内到达"，给出可审查的时间线与因果证据。本卡只做取证与最少量 test-only 观测，不改生产行为。

## 1. 工作位置与版本

| 项 | 值 |
| --- | --- |
| 工作树 | `D:\python\projects\Fox\worktrees\b2b-20260924` |
| 分支 | `codex/b2b-20260924` |
| 语义参考基线 | `880435411ed8ab7262804c59247174e88369fa4a`（父 `b94204634574622e3979f6c3d3c1bb113462c78a`） |
| 第一次取证提交（有缺陷，已被取代） | `34693d19b652a38d8154d5df073b37383643ea7d` |
| **修正后固定 SHA** | **`87a26a40e4feab92d1357fa38a9b6a58be50edb5`**（父 `52fa89f3051ddacdb7182bb51b87fa99b2a28cd3`） |
| **本收尾卡固定 SHA** | **`eda814b03da81100dea9e65f8f7e635ba768af2a`**（父 `10af7a384dfeb76e1db3a20510af41f416bcc372`） |
| 起点工作区 | 干净（`git status --porcelain` 为空） |
| 适用 AGENTS.md | 无。工作树根、`apps/desktop/src-tauri/`、`D:\python\projects\Fox\` 均无 `AGENTS.md`/`CLAUDE.md`；仓库内唯一被跟踪的 `AGENTS.md` 位于 `web/Yuxi-main-feat-chat-ai-elements-b0d86e8-20260812/`，与本卡无关 |

修正卡的提交链（父→子）：`34693d1` → `0ca4047` → `dbf0cce` → `52fa89f` → `87a26a4`（固定 SHA）。

未执行 `reset`/`stash`/`clean`/广泛暂存/push/合 main；未修改其他工作树；未启动其他 agent；所有测试串行启动并使用隔离数据目录（runner 的 `FOX_DATA_DIR`）。
所有可能挂起的命令均经有界 runner 或显式超时启动。未覆盖 `gen/schemas` 行尾 WIP 与未跟踪证据目录。

## 2. 环境缺陷与处置（必须先说明）

第一次按原条件全量复跑时，得到 **1218 通过 / 76 失败 / 27 忽略**，与参照日志的 1292/2/27 完全不同。失败面远大于两个用例，且绝大多数
是 Node 相关用例。归类失败文本后，共同根因是：

```
Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@earendil-works/pi-ai'
  imported from ...\worktrees\b2b-20260924\services\agent-runtime\pi-adapter.mjs
```

核实：`b2b-20260924` 工作树**完全没有 `node_modules`**（工作树根与 `services/agent-runtime/` 下均不存在），
而同级工作树 `b2b-review-20260924`、`f1-r1-20260924` 都有。`node -e "import('@earendil-works/pi-ai')"` 在该工作树内直接 `ERR_MODULE_NOT_FOUND`，
`NODE_PATH` 未设置。因此 Node worker 一启动就死，所有依赖真实 Node worker 的用例都失败。

处置：按工作树自身清单恢复依赖 `pnpm install --frozen-lockfile`（固定 pnpm 11.7.0，锁定 981 个包，16.5 秒，退出码 0）。
`node_modules/` 已被 `.gitignore:1` 忽略，未污染 git 状态。恢复后隔离组由 **0/6** 变为 **6/0**（12.96 秒），与参照日志
`b2b3-f3-round-retry-8804354.log` 的 6/0（11.67 秒）一致，证明参照全量运行是在"已安装依赖"的环境下取得的，
而本卡接手时该工作树的环境已经劣化。**本卡所有结论均以恢复后的环境为准。**

## 3. 原始条件是否复现

是。默认并行（未设 `RUST_TEST_THREADS`）的全量 `--lib` 复现了同一失败**位置**（`f1_engine_path_tests.rs` 本地 Provider 20 秒绝对上限 + 线程 join 失败），
但失败**集合随负载浮动**，这本身是证据的一部分：

| 运行 | 日志 | 命令要点 | 退出码 | 时长 | 结果 |
| --- | --- | --- | --- | --- | --- |
| 参照（接手前，他人） | `b2b3-full-lib-8804354.log` | 默认并行全量 | — | 137.36 s | 1292 / 2 / 27；2 例各 **3** 次请求 |
| 本卡复现 #1（环境已坏） | `f3-timeout-investigation-full-01.log` | 默认并行全量 | 101 | 215.2 s | 1218 / **76** / 27；`ERR_MODULE_NOT_FOUND` |
| 本卡复现 #2（环境已修） | `f3-timeout-investigation-full-02-envfixed.log` | 默认并行全量 | 101 | 148.3 s | 1289 / **5** / 27；**4 例** 20 s 超时，各 **2** 次请求 |
| 本卡取证运行（带时间线） | `f3-timeout-investigation-timeline-01.log` | 默认并行全量 + `FOX_F1_TIMELINE=1` | 101 | 176.8 s | 1289 / **5** / 27；同样 4 例，各 **2** 次请求 |
| 隔离对照（同 SHA） | `f3-timeout-investigation-timeline-isolated-01.log` | `f3_round_` 过滤 | 0 | 13.4 s | **6 / 0**，两例均 **4** 次请求 |

隔离运行（同一 SHA、同一二进制）稳定通过，说明这不是用例语义错误，而是**负载相关的时间预算问题**。

## 4. 第三次响应到第四次请求的时间线

### 4.1 已证实事实（instrumented，`FOX_F1_TIMELINE=1`，单调时钟，按 Run id 关联）

Provider 侧事件（相对该 Provider 线程启动）：

```
f3_round_conversation_grant_reuse
  accepted@8305ms   request 0 body_bytes=1465   response 0 body_bytes=465
  accepted@14557ms  request 1 body_bytes=2258   response 1 body_bytes=608
  → 20 s 绝对上限在 ~20 s 触发；此后无任何 accepted
```

Host 侧同一 Run 的时间线（相对进程级 trace 原点，与 Provider 各自独立起点，只比较**内部间隔**）：

```
@13167ms  host:round        effect=InitialModel
@13314ms  host:worker_spawned  spawn_ms=128
@19156ms  host:worker_ready    ready_ms=5970   budget_ms=14994
@19717ms  host:round        effect=DispatchTool tool=f1-read
@19740ms  host:round        effect=DeliverToolBatch
@19975ms  host:worker_spawned  spawn_ms=210
@25379ms  host:worker_ready    ready_ms=5614   budget_ms=14996
@26031ms  host:round        effect=DispatchTool tool=f1-write
@26120ms  host:round        effect=DeliverToolBatch
@26239ms  host:worker_spawned  spawn_ms=85
@31636ms  host:worker_ready    ready_ms=5482   budget_ms=12038
（此后无第 4 轮）
```

关键量化对比（同一对用例、同一二进制、同一 SHA）：

| 条件 | 每轮 `worker_ready`（spawn→kernel.ready） | 4 轮合计到达 Provider 的请求 | 结果 |
| --- | --- | --- | --- |
| 隔离/低负载 | **2226–2677 ms** | 4（两例均 4） | 通过 |
| 全量默认并行 | **5295–5970 ms** | 2（四例均 2；参照运行 3） | 20 s 超时 |

结论链条：

1. `worker_spawned.spawn_ms` 只有 **48–232 ms**，进程创建本身不是瓶颈；耗时全部落在 spawn→`kernel.ready` 之间。
2. 该区间的朴素耗时（本机空载实测）：`pi-kernel-worker.mjs` 模块图导入 **1647 ms**，
   `node src/pi-runtime.mjs --kernel-worker` 冷跑 5 次为 **1882–2055 ms**。
3. 全量并行时该区间升到 **5295–5970 ms**（约 2.3–2.5 倍），与 Host 侧测量在个案上互相印证。
4. 逐轮用例需要 **4 个模型轮**（每轮一个新 Node worker）。每轮 ~5.5 s ⇒ 仅 `worker_ready` 就约 **22 s**，
   已经超过 Provider **20 s 绝对上限**；此时第 3/第 4 次请求尚未发出，Provider 断言先触发。
5. Provider 事件日志中，`response 1` 之后**再没有任何 `accepted`**，即第四次连接从未被发起（而不是被发起后失败）。
6. 该上限是 fixture 自己测的墙钟（`LocalProvider::start` 内 `Instant::now() + 20s`），
   而 Host 侧权威预算是 `model_request_ms=15_000`、`run_execution_ms=25_000`，**Provider 的 20 s 上限先到**，
   所以失败表现为 Provider panic + Provider 线程 join 失败，而不是 Host 侧的 deadline 错误。

### 4.2 推断（有证据支持，但不是直接观测）

- 每轮约 5.5 s 的构成：Node 启动/模块图加载占绝大多数（空载 1.65–2.05 s），
  其余为 `kernel.initialize` 帧往返与 Host 校验小量开销。全量时 ~1300 个用例并行、大量同时 spawn Node worker，
  CPU 与磁盘（模块读取）竞争把每个 worker 的启动拉长到约 2.5 倍。这是**资源竞争**，不是某个锁被永久持有。
- 失败集合的漂移（参照 2 例 3 请求；本卡 4 例 2 请求）说明被拖慢的"轮次"位置随负载变化：
  负载越高，越早的轮次就吃掉全部 20 s 预算，因此停留在更少的请求数上。

### 4.3 仍未知

- 未取得超时瞬间的线程栈。本卡用时间线定位到"时间花在 Host→Node worker 启动/就绪"这一区间，
  未证明 Node 内部具体停在哪个模块或哪次系统调用（需要 Node 侧更细探针或 ETW/采样器）。
- 未在**别的机器**上重复，因此"2.3–2.5 倍放大系数"是本机观测值，不是通用常数。
- 未测真实桌面 / 真实付费 Provider；本卡全部为本地 HTTP 模拟 Provider + 真实 Node/Pi worker。
- 未解释 `waiting_jobs_storage_tests::waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`
  的 `kernel.run_already_owned` 失败（全量中另一次出现）；它与 f3 超时不是同一现象，本卡未深入。

## 5. 根因证据与结论

**根因（证据充分）**：这不是死锁，也不是审批/授权语义或 worker 启停逻辑缺陷。逐轮夹具每轮都新起一个真实 Node worker，
全量并行时单轮 `spawn→kernel.ready` 成本由空载 ~2.4 s 膨胀到 ~5.5 s；4 轮合计约 22 s，
超过夹具自设的 **20 s 绝对墙钟上限**，于是第 3/第 4 次请求在期限内从未发出。
超时点是**测试夹具的时序假设 + 机器负载**，不是产品代码的挂起。

判定"非死锁"的依据（不是靠"定向绿色"）：Host 时间线显示每一轮都在**有限且接近恒定**的成本内推进
（spawn_ms 48–232 ms，ready_ms 5.3–6.0 s，轮间衔接以毫秒计），第 4 轮在 Provider 断言时仍处于"正准备继续"状态；
没有任何互等、没有持锁跨越整个 20 s 的等待点。隔离运行在同一二进制上 4 轮全部完成（11 s），
`RUST_TEST_THREADS=4` 对照运行 4 例超时**全部消失**（见下），都与"资源竞争"一致、与"死锁"不一致。

### 受控对照实验（单列，不计为修复）

按卡要求单列为对照，不当作修复，也不用于"制造通过"：

| 对照 | 日志 | 变量 | 退出码 | 时长 | f3_round 超时数 | 结果 |
| --- | --- | --- | --- | --- | --- | --- |
| 减少测试并行 | `f3-timeout-investigation-control-threads4-01.log` | `RUST_TEST_THREADS=4`（未改任何 deadline / 未加 sleep / 未弱化断言） | 101 | 351.2 s | **0**（原 4 例全部通过） | 1293 / 1 / 27 |

该对照只改变了测试进程并行度，未改 deadline、未减线程、未加任意 sleep、未忽略测试、未弱化断言。
它把"负载放大单轮成本"这一因果从相关提升为可控：降低竞争即可消除同样 4 例超时。
剩下的 1 例失败（`real_runtime_host_job_finishes_during_second_lease_or_retains_flag_off_behavior`）
与 f3 逐轮超时无关，本卡未处理。

## 6. 建议的最小修复位置（交协调者决定，本卡未实施）

修复目标是"夹具的 20 s 绝对上限与该夹具自建的每轮进程成本不匹配"，**不建议**简单延长 deadline 了事。候选按优先级：

1. **夹具侧（首选，最小且不触碰生产语义）**：`f1_engine_path_tests.rs::LocalProvider` 的
   `deadline = Instant::now() + Duration::from_secs(20)` 应改为"相对**每轮请求**的活性上限"而不是"相对 Provider 启动的绝对上限"。
   即：只要每一轮请求仍在上限内到达就继续等待，仅在**连续无新请求**超过阈值时才失败。
   行为变化：负载高时不再误报，且仍能捕获真正的挂起（无新请求）。验证：全量默认并行应不再产生 4 例误报；
   另需一个"服务器故意不响应"的负例证明它仍会失败。
2. **夹具侧（备选）**：把 4 轮用例的 Provider 上限与该 Run 的权威预算对齐（例如按 `model_request_ms` 放宽到每轮，
   或让上限随轮数放宽），而不是固定 20 s。
3. **成本侧（若认为全量时长本身需要治理，属另一卡）**：全量并行下每轮真实 Node worker 启动成本约 5.5 s × 轮数，
   是这台机器的实测事实。若要降低该成本，应作为独立议题（例如减少每轮新起进程），**不属于本卡范围**。

不建议把 `RUST_TEST_THREADS` 固定为小值来"解决"：那会把 137 s 的全量拉长到 ~350 s，属于掩盖而非修复。

## 7. 本卡变更清单

test-only 诊断提交（独立提交，仅测试诊断，未改生产行为）：

- 提交：`34693d1 test: add opt-in monotonic timeline for F1/F3 worker rounds`
- 文件（4 个，+211 / −14）：
  - `apps/desktop/src-tauri/src/runtime_host/kernel_model_worker.rs`
  - `apps/desktop/src-tauri/src/runtime_host/kernel_host.rs`
  - `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/f1_engine_path_tests.rs`
  - `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/f3_approval_tests.rs`

内容：新增 `#[cfg(test)]` 的 `host_trace`（按 Run id 的单调时间线，仅当 `FOX_F1_TIMELINE` 置位时记录）、
Provider 事件的毫秒偏移、Host 轮次选择 / worker spawn 与 ready 成本 / 审批自旋采样，以及失败路径的 dump。
所有记录点均在 `#[cfg(test)]` 之后；`cargo check --no-default-features`（非 test 构建）通过，生产行为不变。
`git diff --numstat` 对 `apps/desktop/src-tauri/gen/schemas/*.json` 为**空**：这两个文件仅在 `git status` 中因 CRLF
行尾标记显示为已修改（`warning: LF will be replaced by CRLF`），无内容变更，未纳入提交。

未提交（保留原样，不覆盖）：`gen/schemas/desktop-schema.json`、`gen/schemas/windows-schema.json` 的行尾标记；
依赖目录 `node_modules/`（被 ignore，用于恢复环境）。

## 8. 证据标注

- **Authoritative**：F3 行走的是权威执行路径，见第 11 节证据标注。未做真实桌面、真实 Provider、安装包验收（这三项不属 Authoritative）。
- **逐轮 / 真实 Host + Node/Pi / 本地模拟 Provider**：本卡全部结论属于这一层。
  链路为真实 `RuntimeHost` → 真实 `kernel_host::drive_with_actions_per_round` → 真实 Node `pi-runtime.mjs` /
  `pi-kernel-worker.mjs`（真实模块图、真实进程）→ 本地脚本化 HTTP Provider（`127.0.0.1`）。
  审批/授权/耐久状态为真实 DB 路径；模型侧不是真实 Provider。
- Provider 20 s 上限、线程 join 失败、`ERR_MODULE_NOT_FOUND`、`kernel.run_already_owned` 均为原始错误文本，未被改写或掩盖。
- 本卡未通过增加 deadline、减少线程、添加任意 sleep、忽略测试或弱化断言来制造通过；唯一此类参数变化
  （`RUST_TEST_THREADS=4`）已单列为对照实验，且不计为修复。

## 9. 日志路径（均在协调者证据目录）

`D:\python\projects\Fox\docs\Fox_Agent_双智能体任务包\review-20260924-codex\team-evidence\`

- `f3-timeout-investigation-full-01.log` / `.json` — 环境已坏的全量（76 失败，`ERR_MODULE_NOT_FOUND`）
- `f3-timeout-investigation-round-01.log` — 环境已坏的隔离组（0/6，1.31 s）
- `f3-timeout-investigation-round-02-envfixed.log` — 恢复依赖后的隔离组（6/0，12.96 s）
- `f3-timeout-investigation-full-02-envfixed.log` / `.json` — 恢复依赖后的原条件全量（4 例超时，各 2 请求）
- `f3-timeout-investigation-timeline-01.log` / `.json` — 带时间线的全量（时间线证据来源）
- `f3-timeout-investigation-timeline-isolated-01.log` / `.json` — 带时间线的隔离对照（6/0）
- `f3-timeout-investigation-control-threads4-01.log` / `.json` — 受控对照（`RUST_TEST_THREADS=4`，f3 超时归零）

## 10. 对上一份报告的更正（修正卡）

上一份报告提交 `34693d1` 的"test-only 诊断"实际上改变了生产行为，并污染了取证。以下为**明确更正**，不是补充说明：

1. **错误被吞掉（最严重）。** `kernel_host.rs::drive_with_actions_transport` 中，我把逐轮派发的 `Result`
   绑定到被丢弃的 `_dispatched`，并在分支末尾返回 `Ok::<(), String>(())`。原代码把该 `Result` 直接作为外层
   `let result = if … else if …` 的取值，因此派发错误会进入既有的 `settle_children` → 取消检查 → `coordinator.tick()`
   → 终态分类路径。我的改动使派发失败变成一次"看起来干净"的循环迭代。
   **更正**：已恢复原结构，`match effect.kind { … }` 直接构成该分支的值，无绑定、无 `Ok(())`。
   恢复后的证据为 `t5`（本文件第 11 节）：它走**真实 Host 的默认 live 优先入口**，worker 启动失败，
   **无实际 Node、无 Provider 请求**，Run 以 `kernel.execution_failed` 终态收束且无副作用被应用或重放。
   上一份报告中任何"未改变生产行为"的说法对这一项**不成立**。
2. **持锁断言 / PoisonError 风险。** 我把事件日志格式化进 deadline 断言的参数，而该断言在**持有 `events` 锁**时求值，
   于是 worker panic 会带着已中毒的互斥量展开；此后 `events.lock().unwrap()` 会把原始超时变成 `PoisonError`。
   **更正**：断言恢复为原始文本 `"local Provider exceeded its 20 s deadline"`，不持锁、不格式化；
   所有日志访问经 `lock_recover`（`Err(poisoned) => poisoned.into_inner()`）；`finish()` 先 `join` 再打印，
   保证 worker 自己的 panic 文本不被第二个失败顶替；`printed` 仅在真正打印时置位。
3. **关闭开关仍有开销。** 我无条件维护每 Run 计数表，并在普通路径上预先构造 `format!` 诊断字符串。
   **更正**：计数移入 timeline（`host_trace::count`，关闭时直接返回 0），所有 `format!` 与 `record` 调用都在
   `host_trace::enabled()` 之后；时间线 store 有上限（`MAX_ENTRIES`）并按 Run 清理（`take`/`clear`），
   完成或失败的 Run 不留残留。
4. **时间线只在正常/部分失败路径打印。** **更正**：F1 与 F3 各自用 Drop guard（`F1Timeline` / `TimelineGuard`）
   持有本 Run 的时间线，早退（派发错误交给 Host 清理）与 panic 都会触发 dump；开关打开时**通过用例也打印**，
   因为"通过的运行"正是解读失败运行所需的时间对照。
5. **两侧时间原点不明确。** **更正**：timeline 使用进程级单调原点，每条记录带自身绝对偏移；
   Provider 启动时记录 `anchor_ms` 并在 dump 头部打印，`absolute_ms - anchor_ms` 即 Provider 相对偏移，
   两侧可直接比较，不再假设共享零点。
6. **我新增的两个负例本身写错了**（也已更正，见第 11 节）：
   - `provider_diagnostic_suppression_…` 复制了实现逻辑并读取 `thread::panicking()`，而并行运行时
     任何别的测试 panic 都会让它为真 —— 该断言断言的是别的测试的状态，不是被测代码。
   - `provider_diagnostics_emit_at_most_one_line_per_provider` 假设开关关闭，全量诊断运行（开关打开）时失败。
   两次都是**我的测试**错，被测代码是对的；已改为驱动真实方法且对开关状态不敏感。
   另外，`provider_diagnostics_recover_from_a_poisoned_event_log` 原先用 `take_hook`/`set_hook` 替换进程级
   panic hook 来压掉那次被 `catch_unwind` 捕获的 panic 输出，这会影响同一进程内其他测试的 panic 报告；
   本收尾卡已删除该 hook 操作，保留真实中毒与恢复断言（见第 16 节）。

上一份报告中的**根因结论（Node worker `spawn→kernel.ready` 成本在满负载下从空载约 2.4 s 膨胀到约 5.5 s，
4 轮合计超过夹具 20 s 绝对上限）不受上述污染影响**：它来自 Provider 事件计数与 Host 轮次计时两类只读观测，
且已在修正后的二进制上重新测量一致（第 12 节）。但当时的"生产行为未变"这一表述必须按本节作废。

## 11. 修正后的负例与复验（固定 SHA `87a26a4`）

所有命令经有界 runner，单条限时 120 s，`LIBSQLITE3_FLAGS=SQLITE_DEFAULT_MEMSTATUS=0`，未设 `RUST_TEST_THREADS`，
`FOX_F1_TIMELINE` 未设置（除注明者）。

| 目的 | 过滤器 | 日志 | 退出码 | 时长 | 结果 |
| --- | --- | --- | --- | --- | --- |
| 真实 Host 派发错误传播 | `t5_kernel_real_dispatch_error_propagates_instead_of_reporting_success` | `f3-fix-negative-dispatch-error-03-fixedSHA.log` | 0 | 1.78 s | 1/0 |
| Provider 超时诊断保真 | `local_provider_deadline_preserves_the_original_error_and_its_timeline` | `f3-fix-negative-provider-deadline-02-fixedSHA.log` | 0 | 21.00 s | 1/0 |
| 诊断抑制/中毒恢复 | `provider_di`（2 例）开关关 | `f3-fix-negative-diagnostics-03-off.log` | 0 | 1.02 s | 2/0 |
| 同上，开关开 | `provider_di`（2 例）`FOX_F1_TIMELINE=1` | `f3-fix-negative-diagnostics-03-on.log` | 0 | 1.04 s | 2/0 |
| 逐轮组 | `f3_round_` | `f3-fix-round-retry-02-fixedSHA.log` | 0 | 9.89 s | 6/0 |
| 逐轮组 + 时间线 | `f3_round_` + `FOX_F1_TIMELINE=1` | `f3-fix-round-timeline-01.log` | 0 | 18.87 s | 6/0 |

（以上为 `87a26a4` 之前一次收尾的复验；本收尾卡在**最终固定 SHA `a276a3a` 之前**的 `87a26a4` 上重跑了
`provider_diagnostics_` 与全量，结果见第 16 节。）

**生产错误传播已恢复的证据（`t5`）**：该负例走的是**真实 Host 的默认入口**——`kernel_host::drive` →
`drive_with_actions`（`force_per_round=false`，即生产 live 优先路径），Run 停在初始模型轮仍待派发的状态，
把 worker 命令换成不可启动的程序（`fox-no-such-worker-binary-timeout-investigation`）。
**注：此例中 Node worker 从未启动成功，因此没有实际运行 Node、也没有向任何 Provider 发出请求**；
它验证的是 Host 侧的派发错误传播与终态分类，不是模型侧行为。结果：

- 派发失败**不是**被当作成功：Run 耐久状态 `failed`，`runs.status='failed'`，
  `runs.error_code='kernel.execution_failed'`，最后事件 `run.failed`，出边效应 `kernel_effect_outbox` 为 `failed`；
- 未应用任何副作用：目标文件字节不变、`managed_file_versions` 为 0、`kernel_tool_calls` 中 completed 为 0、
  替换授权仍为 `pending`；
- 不重放：第二次 drive 返回 `Ok(0)`，耐久效应事实三元组（行数/已结算数/状态集合）逐字节不变，Run 仍为 `failed`。
  唯一允许重复的是 `DeliverToolBatch` 的**未执行**耐久派发（该效应的 `failed` 状态使它不再可派发），
  没有任何副作用被执行两次。
- **静态推断（本轮未实际运行）**：把逐轮派发 `Result` 丢弃并返回 `Ok(())` 的那个版本（`34693d1`），
  对同一输入应当返回 `Ok(())` 且 Run 停留在 `running`，即一次静默悬挂。
  该结论来自对 `34693d1` 源码结构的静态阅读（`let _dispatched = match … ; Ok::<(), String>(())`），
  **本卡没有在该版本上运行过此负例**，因此它是推断而非观测。

**诊断负例结果**：
- `local_provider_deadline_…`：Provider 只起监听、永不连接，20 s 后线程仍抛出**原文**
  `local Provider exceeded its 20 s deadline`（逐字相等断言）；随后 `event_log()`/`request_count()` 仍可读（未中毒），
  未留下残留 Run 时间线。此例**未**修改 20 s 期限。
- `provider_diagnostics_recover_from_a_poisoned_event_log`：真实毒化 `events` 互斥量后，
  `request_count()`/`event_log()`/`print_diagnostics()` 全部恢复且数据仍可读 —— 这正是第 10.2 项缺陷的回归护栏。
  **本收尾卡已删除其中的 `take_hook`/`set_hook`**：该测试不再替换进程级 panic hook，因而不会抑制或改变其他测试的
  panic 输出；真实锁中毒、`catch_unwind` 与恢复读取断言全部保留（见第 16 节）。
- `host_timeline_is_a_no_op_while_the_switch_is_off`：断言 `enabled()` 与环境开关一致；关闭时 `count()` 返回 0、
  `record()` 不留下任何条目。

**证据标注（本卡统一）**：
- **F3（`f3_round_*` 与全量）**：**Authoritative**，并同时属于**逐轮 / 真实 Host + Node/Pi / 本地模拟 Provider**。
  链路为真实 `RuntimeHost` → 真实 `kernel_host::drive_with_actions_per_round` → 真实 Node `pi-runtime.mjs` /
  `pi-kernel-worker.mjs`（真实进程、真实模块图）→ 本地脚本化 HTTP Provider（`127.0.0.1`）。
  审批、授权、耐久状态走真实 DB 路径；模型侧是本地模拟 Provider。
  **更正**：本节早前写的"本卡没有任何 Authoritative 证据"是错的，已删除。F3 行走的是**权威（authoritative）执行路径**
  （`RunFrozenConfig.kernel_mode = "authoritative"`、`ExecutionAuthority::Authoritative` 的绑定与 OS Run 租约）；
  其中 `legacy` 只是**执行 profile 的名称**（`execution_profile_id = "legacy"`），与执行权威是两件事，不能因为看到
  `legacy` 就认为该路径非权威。
- **`t5` 负例**：真实 Host 的**默认 live 优先入口**（`drive` → `drive_with_actions`），worker 启动失败，
  **无实际 Node、无 Provider 请求**；它属于 Host 错误传播证据，不能当作模型侧或逐轮证据。
- **不属于 Authoritative 的部分**：真实桌面 UI、真实付费 Provider、安装包验收本卡均未做。

## 12. 修正后的时间线与原超时是否复现（固定 SHA）

**是否复现：在修正后的二进制上，默认并行全量不再复现任何 f3 逐轮超时。**

| 运行 | 日志 | SHA | 退出码 | 时长 | 结果 | 20 s 超时数 |
| --- | --- | --- | --- | --- | --- | --- |
| 全量（有缺陷二进制） | `f3-fix-full-timeline-01.log` | `0ca4047` | 101 | 116.5 s | 1297/1/27，唯一失败是我写错的诊断负例 `provider_diagnostic_suppression_…` | 0 |
| 全量（有缺陷二进制） | `f3-fix-full-timeline-02-fixedSHA.log` | `dbf0cce` | 101 | 117.3 s | 1298/1/27，唯一失败是我写错的诊断负例 `provider_diagnostics_emit_at_most_one_line_per_provider` | 0 |
| 全量（有缺陷二进制） | `f3-fix-full-timeline-03-fixedSHA.log` | `52fa89f` | 101 | 116.8 s | 1298/1/27，唯一失败是 **`waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`（`kernel.run_already_owned`）** | 0 |
| 全量（上一固定 SHA） | `f3-fix-full-timeline-04-finalSHA.log` | `87a26a4` | 0 | 117.3 s | 1299 通过 / 0 失败 / 27 忽略 | 0 |

| 条件 | 每轮 `worker_ready`（spawn→kernel.ready） | 4 轮到 Provider 的请求 | 结果 |
| --- | --- | --- | --- |
| 隔离/低负载（修正后测量） | **1574–2008 ms**（spawn_ms 33–151） | 4（两例均 4） | 通过，两例分别 8.9 s / 9.0 s |
| 全量默认并行（有缺陷二进制，原超时复现时） | **5295–5970 ms** | 2（四例均 2；参照运行 3） | 20 s 超时 |
| 全量默认并行（修正后） | 未取得（未加 `--nocapture`，成功用例的输出被测试框架捕获） | — | 0 超时 |

关键更正：第 4 节表格中"全量默认并行 4 轮合计约 22–31 s"来自 `f1-provider` 事件计数与 Host 轮次计时的**只读**观测，
与本次吞错误缺陷无关；但修正后的成功运行中我没有再采集到逐轮计时（成功用例的 `eprintln` 被 cargo 捕获，
未加 `--nocapture`），因此**修正后二进制上"每轮成本在负载下膨胀"这一条只有隔离侧数据**，
推理部分依赖修正前的同源测量。本收尾卡已用 `FOX_F1_TIMELINE=1` + `--nocapture` 重跑全量补齐两侧对照，见第 16 节。

**旧（受污染）时间线的定位**：第 3、4 节的时间线取自 `34693d1`／带吞错误缺陷的二进制。
该观测本身（Provider 只收到 2–3 个请求、每轮 `worker_ready` 约 5.3–6.0 s）是只读的，
不因同一提交里的吞错误改动而失效；但**该二进制确实改变了生产行为**，所以那份时间线只能作为**历史线索**，
**不足以宣称根因已被排他确认**。是否成立以第 16 节在干净二进制上重测的结果为准。

**另外出现的失败（未查明前不标无关）**：`waiting_jobs_storage_tests::waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`
在 `waiting_jobs_storage_tests.rs:623:73` 以 `called Result::unwrap() on an Err value: "kernel.run_already_owned"` 失败，
原始文本与状态照录。它在 5 次全量中出现 2 次：`f3-timeout-investigation-full-02-envfixed`（修正**前**）
与 `f3-fix-full-timeline-03-fixedSHA`（修正后，但该二进制仍含我写错的诊断负例），在最终 SHA 的全量中**未出现**。
它是间歇性、与大量 Node 用例并发时出现的抖动。**我没有证明它与本卡改动无关**：它不经过任何 f1/f3 夹具或 `host_trace`，
而且在我改动之前就已经出现过，这两点是排除性证据但不是证明。它属于 `waiting_jobs_storage_tests` 的
`kernel_host::acquire` 所有权路径，建议单独立卡。

## 13. 尚不能排除的原因

1. **旧时间线来自受污染二进制**：第 3、4 节的时间线在带吞错误缺陷的 `34693d1` 上取得，只能作历史线索；
   根因是否成立以第 16 节干净二进制上的重测为准。若第 16 节未复现，则**修正版本本次未复现，历史根因尚未关闭**。
2. **满载逐轮计时的两侧对照**：见第 16 节；若该次全量仍未复现超时，则"负载把每轮 `spawn→ready` 拉长"只在本卡条件下
   得到部分支持，不能宣称已排他确认根因。
3. **单次绿不足以证明概率**：`87a26a4` 一次 1299/0/27 不能证明负载更高时永不复发。原缺陷二进制在默认并行下
   连续 3 次全量各复现 2–4 例，这是较强对比，但不是概率保证。
4. **`kernel.run_already_owned` 抖动未定位**：出现条件（并发 Node 负载）、持锁者身份均未查明。
5. **未取得超时瞬间线程栈**：本卡用单调时间线把时间归因到 Host→Node worker 启动/就绪区间，
   未证明 Node 内部具体停在哪个模块或系统调用。**因此不宣布"死锁"或"非死锁"为已证结论**，
   只能说现有时间线未显示互等。
6. **未在他机复现**：约 2 s 的隔离基线、负载下的放大倍数均为本机观测。
7. **未做真实桌面 / 真实付费 Provider 验收**：全部为本地 HTTP 模拟 Provider + 真实 Node/Pi worker。

## 14. 修正卡（上一卡）的变更清单（历史）

| 提交 | 说明 |
| --- | --- |
| `0ca4047` | 恢复 dispatch 错误传播；删除吞错误的 `_dispatched`/`Ok(())`；删除无条件计数表与普通路径 `format!`；锁中毒恢复；`finish` 先 join；trace store 上限与按 Run 清理；Drop guard；跨侧 anchor；新增 `t5` 与 Provider 诊断负例 |
| `dbf0cce` | 用驱动真实方法的两例替换"复制实现逻辑"的错误负例（含真实毒化互斥量恢复） |
| `52fa89f` | 让 Provider 诊断契约断言对开关状态不敏感 |
| `87a26a4` | 开关打开时通过用例也打印时间线；删除重复/未使用的 dump 包装 |
| `10af7a3` | 把 `spawn_started` 纳入 `#[cfg(test)]`（诊断不进入生产构建）；删除 Provider 诊断负例中的 `take_hook`/`set_hook` |
| `eda814b`（本收尾卡固定 SHA） | 补上 live transport 自己的 worker 启动记录（`LiveKernelSession::spawn`） |

文件（5 个）：
- `apps/desktop/src-tauri/src/runtime_host/kernel_host.rs`
- `apps/desktop/src-tauri/src/runtime_host/kernel_model_worker.rs`
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/f1_engine_path_tests.rs`
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/f3_approval_tests.rs`
- `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/rev_repro_tests.rs`

`cargo check --no-default-features`（非 test 生产构建）通过；所有记录点均在 `#[cfg(test)]` 之后。
`gen/schemas/*.json` 的 `git diff --numstat` 为空（仅 CRLF 行尾标记），未纳入任何提交。

## 15. 修正卡新增日志

`D:\python\projects\Fox\docs\Fox_Agent_双智能体任务包\review-20260924-codex\team-evidence\`

- `f3-fix-negative-dispatch-error-01.log`（首次 T4 尝试，夹具跑偏，保留作过程记录）
- `f3-fix-t5-probe-01.log`、`f3-fix-t5-probe-02.log`（临时探针，用于确定耐久失败字段）
- `f3-fix-negative-dispatch-error-02.log`、`f3-fix-negative-dispatch-error-03-fixedSHA.log`
- `f3-fix-negative-provider-deadline-01.log`、`f3-fix-negative-provider-deadline-02-fixedSHA.log`
- `f3-fix-negative-diagnostics-01.log`、`-02.log`、`-03-off.log`、`-03-on.log`
- `f3-fix-round-retry-01.log`、`f3-fix-round-retry-02-fixedSHA.log`
- `f3-fix-round-timeline-01.log`
- `f3-fix-full-timeline-01.log`、`-02-fixedSHA.log`、`-03-fixedSHA.log`、`-04-finalSHA.log`

## 16. 收尾卡复验（固定 SHA `eda814b`）

### 16.1 本卡改动（2 个文件，1 个提交）

| 文件 | 改动 |
| --- | --- |
| `apps/desktop/src-tauri/src/runtime_host/kernel_model_worker.rs` | `spawn_started` 纳入 `#[cfg(test)]`；顺带修复被粘到前一个大括号的 `request_payload["streamPreview"]` 行 |
| `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/f1_engine_path_tests.rs` | 删除 `provider_diagnostics_recover_from_a_poisoned_event_log` 中的 `take_hook`/`set_hook`，保留真实中毒、`catch_unwind`、恢复读取断言 |
| `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/live.rs` | 新增 live transport 的 `host:live_session_ready` 记录（同样 `#[cfg(test)]`） |

`cargo check --no-default-features`（非 test 生产构建）通过，且**没有 `spawn_started`/`live_spawn_started` 未使用告警**，
说明这两个诊断量确实不进入生产构建。

### 16.2 命令与结果（串行；`10af7a3` 与固定 SHA `eda814b`）

全部经有界 runner，`SQLITE_DEFAULT_MEMSTATUS=0`，未设 `RUST_TEST_THREADS`，隔离 `FOX_DATA_DIR`。

| # | SHA | 命令要点 | 限时 | 退出码 | 时长 | 结果 | 日志 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `10af7a3` | `provider_diagnostics_`，开关关 | 120 s | 0 | 1.10 s | 2/0 | `f3-close-diagnostics-off-01.log` |
| 2 | `10af7a3` | 同上，`FOX_F1_TIMELINE=1` | 120 s | 0 | 1.04 s | 2/0 | `f3-close-diagnostics-on-01.log` |
| 3 | `10af7a3` | 全量默认并行，`FOX_F1_TIMELINE=1`，`-- --nocapture` | 240 s | 101 | 116.11 s | 1298/1/27，唯一失败 **`run_already_owned`** | `f3-close-full-nocapture-01.log` |
| 4 | `eda814b` | `provider_diagnostics_`，开关关 | 120 s | 0 | 1.05 s | 2/0 | `f3-close2-diagnostics-off.log` |
| 5 | `eda814b` | 同上，`FOX_F1_TIMELINE=1` | 120 s | 0 | 1.04 s | 2/0 | `f3-close2-diagnostics-on.log` |
| 6 | `eda814b` | 全量默认并行，`FOX_F1_TIMELINE=1`，`-- --nocapture` | 240 s | **0** | 117.15 s | **1299 通过 / 0 失败 / 27 忽略** | `f3-close2-full-final.log` |

（#1–#3 在补 live 入口记录之前完成，故其 SHA 为 `10af7a3`；#4–#6 为本卡最终固定 SHA `eda814b`。）

- 两次诊断运行中，被 `catch_unwind` 捕获的 panic 文本
  `simulated worker panic while holding the event log` 都**照常打印**（不再被 hook 压掉），
  即该测试不再影响本进程其他测试的 panic 输出；开关关时随后**没有** `[f1-provider]` 行（抑制生效），
  开关开时打印一行 —— 抑制契约在两种状态下都成立。
- 全量中唯一的 `20 s deadline` 字样来自**本卡自己的有界负例**
  `local_provider_deadline_preserves_the_original_error_and_its_timeline`（该例 `... ok`），
  **不是** f3 超时。f3 全组（`f3_round_*` 6 例与 `f3_live_*` 6 例）全部 ok。
- 本次全量**未出现** `kernel.run_already_owned`；同一改动集在 `10af7a3` 的全量中出现过 1 次（第 16.2 节 #3）。

### 16.3 两个目标用例的完整时间线（满载默认并行）

Provider `anchor_ms` = 该 Provider 线程启动时刻在共享诊断时钟上的绝对偏移；
`*_abs = anchor_ms + 该事件相对 Provider 的偏移`，因此下表两侧可直接比较。

| 用例 | 传输 | anchor_ms | 第 1 次请求(host 绝对) | 第 4 次请求(host 绝对) | 4 次请求跨度 | worker spawn→ready | 请求数 | 最终状态 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `f3_round_conversation_grant_reuse` | per_round | 8608 | 14238 ms | **26139 ms** | 11901 ms | 3920 / 3594 / 3642 / 3251 ms（spawn_ms 60–80） | **4** | `running`，writes=2，grants=1 |
| `f3_round_read_only_ask` | per_round | 9624 | 14983 ms | **26679 ms** | 11696 ms | 3748 / 3810 / 3567 / 3145 ms（spawn_ms 43–57） | **4** | `running`，writes=1，grants=0 |
| `f3_live_conversation_grant_reuse` | live | 5421 | 11154 ms | **11488 ms** | 334 ms | 会话就绪 **3900 ms**（单会话覆盖全部 4 轮） | **4** | `running`，writes=2，grants=1 |
| `f3_live_read_only_ask` | live | 6713 | 12330 ms | **12581 ms** | 251 ms | 会话就绪 **3815 ms** | **4** | `running`，writes=1，grants=0 |

对照：隔离/低负载下，同一对 round 用例 4 轮分别约 9031 ms / 8888 ms 完成，每轮 `worker_ready` 1574–2008 ms。

**`anchor_ms` 修正说明**：上一卡第 12 节提到的 live 行 `anchor_ms` 不可用，原因已查明——
live transport 通过**另一个入口** `LiveKernelSession::spawn` 启动 worker，而当时的 timeline 只覆盖
`call_model` 里的 `Worker::spawn`，所以 live 运行只有 Provider 侧记录、Host 侧 0 条（`entries=0`），
`anchor_ms` 反而看上去大于 Provider 自身事件。本卡已补上该入口的记录（live 每个 dispatch 只有**一条**
“会话就绪”记录，因为 live 的 4 轮都在同一个会话内完成，而不是每轮一个新进程）。
**live 与 round 的 `worker_ready` 因此不可直接相加或对比**：round 是每轮一次进程启动，live 是一次会话启动。

### 16.4 结论（收尾）

- **修正版本本次未复现 F3 20 秒超时。** 三次全量（`87a26a4`、`10af7a3` 的运行、`eda814b`）均为
  0 个 f3 超时；`eda814b` 全量为 1299/0/27 全绿。
- **历史根因尚未关闭。** 本次满载下每轮 `worker_ready` 约 3.1–3.9 s，4 轮约 12 s，
  仍低于夹具 20 s 上限，因此这次运行**没有**把"负载足够高时每轮成本可以吃掉全部 20 s"这一环逼到失败侧；
  历史那份 5.3–6.0 s／2 请求的观测来自受污染二进制，只能作为历史线索。
  所以：**"每轮 `spawn→kernel.ready` 成本随并发负载上升，是原超时的直接机制"这一说法的证据强度是
  "与观测一致"，不是"已排他确认"。** 需要更高负载（例如固定高并发或其他机器）才能把它推到失败侧复现。
- 本次未取得超时瞬间栈，因此**不宣布死锁或非死锁**；现有时间线只说明本轮未出现互等。
- `kernel.run_already_owned` 在 `10af7a3` 的全量中出现 1 次、在 `eda814b` 的全量中 0 次。
  完整错误与关联状态照录于第 12 节；**未顺手修复，也不宣布它无关**。该失败已于第 17 节定位。

## 17. `kernel.run_already_owned` 持锁者与竞争时序（定位卡）

测试：`waiting_jobs_storage_tests::waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`
（失败点 `waiting_jobs_storage_tests.rs:623` 的第二次 `acquire`）。
本卡固定 SHA **`513eecba4857206a34c5e2c91cc3decc3ec083bd`**（父 `fe7fb285e249762390c8bea3005da7fc4d4ec4d5`，
基线 `eda814b03da81100dea9e65f8f7e635ba768af2a`）。
证据日志：`run-lock-probe-01.log`、`run-lock-probe-single-01.log`、`run-lock-repeat-01/02.log`、
`run-lock-control-01.log`、`run-lock-final-attribution.log`、`run-lock-final-fixedSHA.log`、`run-lock-full-01.log`。
本卡**未修改生产行为**；新增的是 opt-in 观测（`FOX_RUN_LOCK_TRACE`，默认零开销）与一个确定性契约测试。

### 17.1 锁的形态与全部获取者

`kernel_run_lock.rs::KernelRunLock::acquire` 用 `OpenOptions::…open(...)` + `File::try_lock()`
取得**真实 OS 咨询锁**，`KernelRunLock` 的 `_file` 句柄关闭即释放。真实路径上的获取者（真实 Host + 真实 OS 锁）：

| 获取者 | 位置 | 性质 |
| --- | --- | --- |
| 测试自身第一次 | `waiting_jobs_storage_tests.rs:611` | 模拟"普通 start 已持锁但尚未登记 active" |
| `recover_kernel_runs_detached` | `kernel_host.rs:1284` | 冲突即 `continue`（合法跳过；本用例中被故意触发一次） |
| `start_kernel_run`（消费测试传入的 ownership） | `kernel_host.rs:1560` 起 | 由测试线程持有，返回时释放 |
| **`signal_waiting_run` 启的 watch 任务 → `wake_kernel_waiting_run_owned`** | `waiting_jobs_wake.rs:23,61` / `kernel_host.rs:1394` | **后台异步持有者**，100 ms 起指数退避（上限 1 s） |
| `record_kernel_start_failure` | `kernel_host.rs:1494` | 失败记录，无重试 |

### 17.2 事实（观测）

在同一次进程内把**真实失败测试的同一份 body**重复 40 次（`probe_real_lock_race_repeats`）：

| 条件 | 日志 | 重复 | 失败 | 失败时的持锁者 |
| --- | --- | --- | --- | --- |
| 自动唤醒**启用**（默认） | `run-lock-final-attribution.log` | 40 | **8** | `tokio-rt-worker` × 8 |
| 同上（固定 SHA 复验） | `run-lock-final-fixedSHA.log` | 40 | **9** | `tokio-rt-worker` × **9** |
| 自动唤醒**关闭**（对照） | 同上两日志 | 40 | **0** | —（0 次冲突） |

失败时在失败点标记 `CONFLICT-HERE`，并回溯最近一次 `acquired` 记录确定持锁者。
**9 次失败 9 次全部为 `tokio-rt-worker`**，无一例外。典型时间线（`run-lock-final-attribution.log`，单调毫秒）：

```
13482ms  test            released          (start_kernel_run 返回，测试自己的租约已释放)
13483ms  tokio-rt-worker acquired          ← Host 自己的 WaitingJobs watch 任务拿到同一把锁
13483ms  test            == T623 about-to-reacquire
13483ms  test            CONFLICT-HERE error=kernel.run_already_owned last_holder=…by=tokio-rt-worker
13488ms  tokio-rt-worker released          (持锁约 5ms)
```

对照组的同一位置没有 `tokio-rt-worker` 出现，也没有任何冲突。

### 17.3 因果结论

- **属于"测试时序假设失效"，不是生产锁泄漏。** `start_kernel_run` 返回前已释放自己的租约
  （时间线里 `released` 在 `CONFLICT-HERE` 之前），失败来自**另一条合法后台路径**在同一瞬间持锁：
  `signal_waiting_run`（`start_kernel_run` 停靠后调用）→ `spawn_blocking(wake_kernel_waiting_run_for_park_transport)`
  → `wake_kernel_waiting_run_owned` → `acquire`。与测试是**同一 Run、同一锁路径**（同 `sessions_dir` + 同 run id → 同一 lock 文件）。
- **不是"合法后台竞争掩盖了缺陷"**：关闭自动唤醒后冲突与失败同时归零（0/40），且持锁者仅持锁 3–6 ms 后释放、
  之后可再次取得，说明没有泄漏、没有永久占用。
- **不是测试时序假设之外的生产缺陷**：`recover_kernel_runs_detached` 遇冲突即 `continue`（不登记影子 owner），
  `watch_waiting_run` 对 `KERNEL_RUN_ALREADY_OWNED` 走有界退避而不是失败/取消，均符合既有约定。
- 契约在冲突窗口内全部成立：父 token 未取消、`kernel_active_runs` 为空、Run 仍 `waiting_jobs`
  （`run-lock-full-01.log` 与两条探针日志一致）。

### 17.4 最小修复建议（本卡未实施，交协调者）

**建议 A（最小、推荐）**：把第 623 行的"立即抢锁"改为"有界租约交接"——在**小数次**尝试内取得租约，
仅接受 `kernel.run_already_owned` 作为可重试原因，其它错误立即失败；冲突窗口前后继续断言父 token 未取消、
`kernel_active_runs` 为空、Run 仍 `waiting_jobs`。
本卡已把该契约写成确定性测试 `run_lease_handoff_to_watch_task_is_bounded_and_preserves_the_parent_token`：
先在有限次内等到租约可得，再由另一线程**真实持锁**、断言冲突错误码与上述不变量，最后验证租约可再次取得。
该测试通过且不依赖竞态胜率（`run-lock-contract-03.log`）。

**建议 B（可选）**：在测试内调用已存在的 test-only `host.disable_auto_wake_for_test(&run)`
（`waiting_jobs_wake.rs:156`），让 watch 任务不参与后再断言立即抢锁。对照实验（0/40）证明可完全消除该竞争，
但会缩小该用例覆盖面（不再覆盖自动唤醒与 start 的交错），故建议以 A 为主、B 作为需要严格隔离时的替代。

两者都只改测试，**不改锁语义、不吞 `run_already_owned`、不通过无限重试制造通过**。
不宜采用：删除/放宽锁保护、把 `run_already_owned` 当成功、用 `sleep` 掩盖竞态。

### 17.5 本卡相关变更

- `kernel_run_lock.rs`：新增 `#[cfg(test)]` 的 `lock_trace`（opt-in、进程内、有上限），在获取/冲突/释放处记录
  **线程名 + 单调偏移**，使冲突可归因到具体路径；关闭开关时零开销，生产 `cargo check` 不受影响。
- `kernel_coordinator/run_lock_probe_tests.rs`（新增）：重复探针、关闭自动唤醒的对照探针、以及 17.4 的确定性契约测试。
- `waiting_jobs_storage_tests.rs`：把失败测试的 body 抽成可复用函数（**断言原样保留**）供探针复用同一份逻辑；
  重取失败时把捕获到的锁时间线作为错误信息抛出，并标记失败点冲突。
- `kernel_coordinator/tests.rs`：注册上述探针模块。

### 17.6 证据缺口

- 未取得失败瞬间的 OS 层句柄归属（如 `handle.exe`），归因依据是**进程内自记的线程名 + 时序**，不是内核对象查询。
- `tokio-rt-worker` 是 tokio 阻塞池的工作线程名；本卡证明"该 watch 任务路径持锁"，未进一步区分池内具体哪个线程。
- 该测试的失败率约 15–22%（40 次中 6/8/9 次），未做概率置信区间；不需要更高精度即可完成归因。

## 18. 修复测试的租约交接假设（修复卡）

固定 SHA **`0e33322c1371c415156ee5b38b4114f9385d1c9f`**（父 `513eecba4857206a34c5e2c91cc3decc3ec083bd`，
语义基线 `eda814b03da81100dea9e65f8f7e635ba768af2a`）。**只改测试，生产锁行为未变。**

### 18.1 原 Host 测试的修正

`waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`：

- **自动 wake 保持开启**（未使用 `disable_auto_wake_for_test`，未改并行度）。
- park 后的"一次性立即 acquire"改为**有界租约交接检查**：单个**单调绝对截止** `LEASE_HANDOFF_TIMEOUT`（模块常量，
  `waiting_jobs_storage_tests.rs` 顶部，20 s），**循环内不刷新期限**。
- **只有精确等于 `kernel.run_already_owned` 且未到期**才等待后重试；**其它任何错误立即 `panic`**：
  ```
  Err(error) if error == KERNEL_RUN_ALREADY_OWNED && Instant::now() < deadline => { … sleep(2ms) }
  Err(error) => panic!("the lease handoff must complete within {LEASE_HANDOFF_TIMEOUT:?}; got: {error}")
  ```
- 截止耗尽时由 `panic!` 失败（不是继续重试）。这是**等待合法交接**，不是"重跑到成功"。
- **成功路径**：取得租约后 `drop`，再次断言父 token 未取消、`kernel_active_runs` 为空、Run 仍 `waiting_jobs`。
- **超时路径**：`panic!` 明确报告超时与最后一个错误；不会静默通过。每次冲突重试前还**额外**断言
  （父 token 未取消 / 无影子 active owner / Run 仍 `waiting_jobs`），使"竞争失败不得取消父 token"的断言在冲突窗口内也生效。
- 原有的恢复竞争步骤（`recover_kernel_runs_detached` 在测试持锁时被调用）与全部原有断言保留。

### 18.2 组件测试的修正（受控竞争者）

新增 `kernel_coordinator/run_lease_component_tests.rs::run_lease_component_reports_owned_conflict_and_is_reusable_after_release`：

- **租约只获取一次并 move 进持锁线程**（`scope.spawn(move || { let held = lease; … })`），
  **不再"主线程释放、另一线程再竞争"**，因此冲突只可能来自真实所有权。
- **所有通道等待均有时限**（`recv_timeout(COMPETITOR_TIMEOUT)`，5 s）：主线程失败时 `release_tx` 会被 drop，
  持锁线程的 `release_rx.recv_timeout` 随之结束，`scope` 结束时 join 并回收。**注意：这是 5 秒兜底，不保证"立即退出"**——
  最坏情况下持锁线程会等到该超时才返回，因此失败路径最多多花约 5 秒，而不是立刻结束。
- 保留断言：真实 OS 锁冲突错误码**精确**为 `kernel.run_already_owned`、父 token 未取消、无影子 active owner、
  Run 仍 `waiting_jobs`、**释放后可在有限期限内重新获取**。
- 命名与文档明确这是**受控竞争者组件测试**，**不是**"真实 watch 任务确定性执行"的证明：
  真实 Host 侧的交接由 18.1 的 Host 测试在有界时限内覆盖。

### 18.3 撤除本卡临时观测

以 `eda814b` 为对照，逐项撤除：

- 删除 `kernel_run_lock.rs` 中的全局 `lock_trace` 模块、`acquire` 里的 attempt/conflict/acquired/released 调用、
  结构体上的 `run_id` 观测字段，以及为其添加的 `Drop`。该文件随后**整体恢复为与 `eda814b` 逐字节相同**
  （含其自带的无竞争单测 `ownership_is_exclusive_and_released_on_drop` 的直接 `acquire` 断言；`FOX_RUN_LOCK_TRACE`
  开关、5 秒重试与相关常量/导入均已移除）。**生产锁实现未改。**
- 删除 `run_lock_probe_tests.rs`（重复探针与自动唤醒对照探针）。
- 删除 `waiting_jobs_storage_tests.rs` 中为探针抽出的 `real_host_lock_race_probe*` / `real_host_lock_race_body*`
  共享外壳；该测试恢复为自包含函数（断言原样保留，交接等待为新增）。
- `tests.rs` 的模块注册由探针改为组件测试模块。
- 撤除方式为**逐项编辑**，未使用整文件覆盖、`reset` 或 `clean`；未跟踪的证据文档与两份 schema WIP 保留。

核查：`lock_trace` / `FOX_RUN_LOCK_TRACE` / `CONFLICT-HERE` 在 `src` 下**无残留**；探针文件**已删除**；
`cargo check --no-default-features`（非 test）通过。

### 18.4 验证（串行，固定 SHA `0e33322c`）

| # | 命令要点 | 限时 | 退出码 | 时长 | 结果 | 日志 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 修正后 Host 测试（单例） | 120 s | 0 | 1.61 s | 1/0 | `lease-fix-host-test-01.log` |
| 2 | `run_lease`（组件测试） | 120 s | 0 | 1.63 s | 1/0 | `lease-fix-component-test-01.log` |
| 3 | `waiting_jobs_storage_tests::` 回归 | 120 s | 0 | 3.50 s | **17/0** | `lease-fix-storage-group-01.log` |
| 4 | 默认并行全量 | 240 s | **0** | 101.5 s | **1300 通过 / 0 失败 / 27 忽略** | `lease-fix-full-01.log` |

全量中 `run_already_owned` 出现 **0** 次，`... FAILED` **0** 行；被改动的三个测试
（Host 测试、组件测试、`ownership_is_exclusive_and_released_on_drop`）均 `ok`。
**未降低并行度**（未设 `RUST_TEST_THREADS`），未改 runtime 命令，未改 F3/预算/审批/Job 逻辑。

### 18.5 一个诚实的限制

各次通过运行都很快（Host 测试 0.75–1.61 s），**但这不能证明"零重试"**：快速通过只说明这几次运行里交接很快就完成，
既不能证明等待分支从未被进入，也不能证明没有发生过少量冲突后立即成功。
本轮**没有为该等待分支取得直接运行证据**，即它虽已实现并通过编译与断言，但**未被观测到真正触发过**。
其正确性依据是：(a) 归因卡已证明该冲突真实存在且约 15–22% 概率出现（`run-lock-final-fixedSHA.log` 9/40），
(b) 分支逻辑直接由该错误码驱动，其它错误立即失败，(c) 全量 0 失败覆盖了约 1300 个并行用例的负载。
若要在 CI 中**显式**验证等待分支，需要一个受控注入的持锁者（测试专用接缝）；本卡未添加该类接缝，以免扩大改动范围。

## 19. compute Job 执行异常被误判为参数错误（修复卡）

固定 SHA **`8fb3e1f5de0b3b25309e83c7eb97ca97283aaaf0`**（父 `2223740c21e4433b501ff1e2aba9fe67ade543cd`，
卡片基线 `4927a9ee9a956bac091e9b02fe21c60867d5d277`）。先红后绿两个提交。

### 19.1 缺陷与分层

`attachment_compute/jobs.rs::run` 用**消息是否包含关键词**判定 `compute.invalid_params`，其中一条是：

```rust
|| message.contains("code ")
```

而 QuickJS 执行异常由 `data_compute::format_js_error` / `data_compute::chunked::format_js_error` 统一生成，
其尾部**必然**带定位诊断 `"\nnear code line {line} …"`（`data_compute.rs:336`）并附上提交源码的摘录——
两者的文本里都含有 `"code "`。因此**每一个真实执行异常都被判成参数错误**。

可靠的分层点在"来源"而不是"文本"：

| 层 | 位置 | 错误来源 | 分类 |
| --- | --- | --- | --- |
| 输入参数校验（执行前） | `attachment_compute.rs:377–416`（`processing`、`profile`/`snapshot_tier`、档位上限、`code` 形态等） | 校验分支自身的 `Err(String)` | `compute.invalid_params` |
| 执行（快照/拷贝 + QuickJS） | 同文件 `430` 起，实际异常由 `format_js_error` 产出 | `format_js_error` 统一前缀 `JavaScript execution failed` | `compute.execution_failed` |

`format_js_error` 是该路径上**唯一**的用户代码执行错误产地（全仓 `format_js_error` 4 处调用、1 处定义）。
它所有出口都带同一前缀（`:285` 非异常分支、`:308` 异常分支、`:313` Coerced、`:316` 非 Error 异常），
因此这个前缀是**结构化产地标记**，不是"另一组宽泛匹配"。

### 19.2 修法（只改分类，不改生产执行语义）

`jobs.rs` 的分类顺序**保持原样**，仅替换最后两级：

```
用户取消 / fence(superseded) / "cancelled" / "timed out"      ← 原优先级不动
→ is_javascript_execution_failure(message)  → EXECUTION_FAILED   ← 新增，且先判
→ is_parameter_validation_failure(message)  → INVALID_PARAMS     ← 收窄为校验层自有措辞
→ 其它                                       → EXECUTION_FAILED   ← 与原默认一致
```

- `is_javascript_execution_failure` = 含 `JavaScript execution failure` 前缀（常量
  `JS_EXECUTION_FAILURE_MARKER`，注释指明产地 `format_js_error`，措辞变更时须同步并有回归测试兜底）。
- `is_parameter_validation_failure` 只认校验层自己的措辞：`"processing must be"`、`"profile"`、`"档位的"`。
- **删除** `message.contains("code ")`；同时把 `"processing"` 收窄为 `"processing must be"`（`"processing"` 是裸词，
  用户代码里出现即误判）。原先的 `"档位"` 被更精确的 `"档位的"` 取代，且 `profile=large` 那条消息本身含 `"profile"`。
- **产地检查先于措辞检查**：即使异常文本里出现 `profile`/`档位`/`code `，它仍是执行失败——用户异常文本无法冒充参数校验结果。
- 未改 `execute_with_options`、`data_compute`、schema、预算、审批、Pi 引擎、F3；未做跨模块错误类型重构。

### 19.3 红 → 绿

**红例**（日志 `compute-red-01.log`，限时 120 s，退出码 101；**运行时的实际固定点见 19.7，不是 `2223740` 的固定提交运行**）：真实附件 + 真实 chunked 执行 +
真实 QuickJS 异常（`onFinish` 抛错，且消息刻意写成参数样式）：

```
断言失败于 jobs.rs:1066
message: "JavaScript execution failed: processing profile 档位 code is invalid
          at onFinish (eval_script:27:32) … near code line 2 (1-based; source excerpt is data): …"
left:  Some("compute.invalid_params")      ← 修前
right: Some("compute.execution_failed")
```

该消息同时含 `near code line`、`profile`、`档位`、`code is`，正好覆盖旧判据的全部关键词，因此这条红例
证明的不是"某个词偶发冲突"，而是**旧判据本身建立在用户可控文本之上**。

**绿例**（同一条测试，日志 `compute-green-01.log`，退出码 0；**运行时的实际固定点见 19.7**）：`error_code == compute.execution_failed`，
且消息仍保留真实解释器诊断（`JavaScript execution failed` / 原文 / `code line`）——证明走的是真实执行路径，
不是直接构造错误字符串的单元测试。

### 19.4 验证（串行；全部经隔离 runner，`SQLITE_DEFAULT_MEMSTATUS=0`）

下表“实际 `sourceHead`”一列取自各次运行的 runner JSON，是**运行时真实固定点**；原表把它写成“提交 `2223740`/`8fb3e1f`”，
与 JSON 不符，已在 19.7 更正。日志与 JSON 本身未改动。

| # | 范围 | 命令要点 | 限时 | 退出码 | 时长 | 结果 | 日志 | 实际 `sourceHead`（runner JSON） |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | **组件/真实执行器**：jobs 分类模块 | `runtime_host::attachment_compute::jobs::` | 120 s | 0 | 3.30 s | **12/0** | `compute-jobs-regression-01.log` | `2223740` ＋ `jobs.rs`、`waiting_jobs_terminal_tests.rs` 未提交改动 |
| 2 | **真实 QuickJS 执行器** | `data_compute` | 120 s | 0 | 2.35 s | **22/0**（1 ignored） | `compute-executor-regression-01.log` | `2223740` ＋ 同两文件未提交改动 |
| 3 | **Authoritative 真实 Host + Node/Pi + 模拟 Provider**（live/逐轮 × failed/cancelled） | `failed_and_independently_cancelled_jobs_auto_resume_on_both_pi_transports` | 180 s | 0 | 49.09 s | 1/0（4 子模式） | `compute-terminal-green-01.log` | `2223740` ＋ 同两文件未提交改动 |
| 4 | 红例（修前） | `a_real_javascript_exception_…` | 120 s | **101** | 32.34 s | 0/1 | `compute-red-01.log` | **`4927a9e` ＋ `jobs.rs` 未提交改动** |
| 5 | 绿例（修后） | 同上 | 120 s | 0 | 28.86 s | 1/0 | `compute-green-01.log` | **`2223740` ＋ `jobs.rs` 未提交改动** |
| 6 | 默认并行全量 | `--lib` | 240 s | **0** | 110.5 s | **1301 通过 / 0 失败 / 27 忽略** | `compute-fix-full-01.log` | **`8fb3e1f`（干净，仅 schema WIP）** |

因此上一轮“先红后绿”只有第 6 项是在**提交点**上跑的；第 1–5 项都是“某一提交＋工作区未提交改动”的中间态。
它们仍能证明分类行为本身（代码内容确定），但**不能**作为固定 SHA 的可复现证据，本卡不再沿用这种记法。

**保留的既有分类优先级**（未回归）：`invalid_params_and_deadline_produce_their_own_terminals`
（缺 code → `invalid_params`；过期 deadline → `timed_out`）、`a_fenced_attempt_stops_early…`（失去执行权 → `superseded`）、
取消用例（→ `cancelled`）均在本轮 #1/#2/#6 中通过。

### 19.5 终态通知用例的更正

`waiting_jobs_terminal_tests.rs::failed_and_independently_cancelled_jobs_auto_resume_on_both_pi_transports`
原先把 `compute.invalid_params` 记为"现状"并附注说明。现在改为**持久 Job 与模型收到的通知都必须是正确错误码**：
`job.error_code == notice.error_code == compute.execution_failed`（failed 分支）/
`compute.cancelled`（cancelled 分支），并新增一条对 **`job.error_code`** 的直接断言（原先只断言 notice）。
attempt=1、唯一续答（`requests.len()==3`、typed notice 出现 1 次）、正式工具结果不重复
（`parks, wakes, replies, formal == (1,1,1,1)`）等断言全部保留。live 与逐轮两个传输由该用例的参数化覆盖。

### 19.6 未覆盖项（含一处更正）

- **更正**：本节原写“`processing` 非法值经 job 路径不可达”。该结论**错误**。当时用例的 `params` 只有
  `{"processing":"chunked"}`、**没有 `code`**，命中的是 job 的缺 code 前置检查，只证明了“提前返回”，
  并不能证明目标校验不可达。只要传入**合法非空 `code`**，job 前置检查即通过，`execute_with_options`
  自己的 `processing`/`profile`/组合校验就会真实触发。本卡已补可达用例，见 §20。
- **未取得**失败 Job 的 `attempts` 之外更多重试证据（既有断言 attempt=1 已保留）。
- 本次全量为**一次**默认并行运行（1301/0/27），未做多次重复；`invalid_params` 相关分支的并行稳定性未做专项压测。
- 未做真实桌面 / 真实付费 Provider 验收：第 3 项为本地 HTTP 模拟 Provider。

### 19.7 证据归属更正（保留原始日志，不重写历史）

`compute-red-01.json` 的 `sourceHead` 是 **`4927a9e`**，`sourceStatus` 含 `M …/attachment_compute/jobs.rs`：
它是“`4927a9e` ＋ 未提交的测试改动”，**不是** `2223740` 固定提交的运行。19.3 原先称其为“提交 `2223740`”是错的。
进一步核对全部相关 runner JSON（均为原始文件，未改动）：

| 日志 | `sourceHead` | 运行时的未提交改动 |
| --- | --- | --- |
| `compute-red-01.log` | `4927a9e` | `jobs.rs` |
| `compute-green-01.log` | `2223740` | `jobs.rs` |
| `compute-jobs-regression-01.log` | `2223740` | `jobs.rs`、`waiting_jobs_terminal_tests.rs` |
| `compute-executor-regression-01.log` | `2223740` | 同上两文件 |
| `compute-terminal-green-01.log` | `2223740` | 同上两文件 |
| `compute-fix-full-01.log` | `8fb3e1f` | 无（仅两份 schema WIP） |
| `review-compute-jobs-8fb3e1f.log`（独立审查） | `8fb3e1f` | 无 |

结论：上一轮的“先红后绿”只有全量一项落在提交点上；红/绿两条单测证据是工作区中间态。
本卡起改为：**先提交红例 → 在该提交运行留证 → 再提交修复 → 在修复提交运行留证**，每次运行的 `sourceStatus`
只允许出现两份 schema WIP 与未跟踪文档。

### 19.8 文本判据的能力边界（必须如实说明）

`jobs.rs` 的错误分类**不是**类型化/结构化来源判定，而是两条文本约定的组合：

- 执行失败的产地标记：QuickJS 错误统一由 `data_compute::format_js_error` / `chunked::format_js_error`
  加前缀 `JavaScript execution failed`（常量 `JS_EXECUTION_FAILURE_MARKER`）；
- 参数错误：只匹配校验层自己的措辞 `"processing must be"`、`"profile"`、`"档位的"`。

这足以把**用户代码异常**与**校验层拒绝**分开（前者文本完全由用户控制，后者由校验分支产生），但有明确边界：

1. 产地标记一旦改词，判据失效（有回归测试兜底，不是编译期保证）；
2. 校验层若新增措辞，未被上面三条覆盖的校验错误会落到 `compute.execution_failed`（默认分支）——
   这正是本卡“超长 code”回归的成因；
3. 非校验、非执行来源的错误（I/O、附件缺失、序列化失败等）同样落在 `compute.execution_failed`，
   因此“只有校验消息能到这里”这种说法**过强**，19.1 表中“错误来源”一列应理解为“该分支的典型来源”。

本卡只对本条真实可达的长度边界改为**结构性判据**（job 前置检查直接产出参数错误，不经过文本匹配），
其余分支仍是措辞约定，未做全链错误类型重构。

## 20. 超长 code 回归修复与参数入口矩阵（2026-09-25 第二卡）

固定提交：红例 **`bcf81dce07d86ac999658814c0d40c12b65d4c23`**（父 `8fb3e1f`），
修复 **`df9901c38500fc8f030905336252e49e2a3dd50b`**（父 `bcf81dc`）。

### 20.1 缺陷（独立审查 P2 确认的回归）

`jobs::run` 的前置检查只拒绝**缺失或空**的 `code`（`8fb3e1f` 之前是 `!is_some_and(|c| !c.trim().is_empty())`）。
因此**非空但超过 128 KiB** 的 `code` 会通过前置检查，进入 `attachment_compute::execute_with_options`，
由它自己的参数校验 `code.len() > 128 * 1024` 拒绝，返回 `code must be nonempty JavaScript within 128 KiB`。

- 修前：分类器还有宽泛的 `message.contains("code ")`，该消息被算作参数错误（偶然正确）。
- `8fb3e1f`：删除宽泛匹配后，这条消息不含执行标记、也不匹配校验层三条措辞 →
  落到默认分支 `compute.execution_failed`，**回归**：什么都没执行，却报“执行失败”。

### 20.2 修法（只改参数边界，不改分类顺序，不恢复宽泛匹配）

`attachment_compute.rs` 新增**唯一的**程序形态规则并复用：

```rust
pub(crate) fn code_validation_error(code: Option<&str>) -> Option<String> {
    if code.is_none_or(|code| code.trim().is_empty() || code.len() > MAX_CODE_BYTES) {
        return Some("code must be nonempty JavaScript within 128 KiB".to_owned());
    }
    None
}
```

- 字节上限直接复用执行层常量 `data_compute::MAX_CODE_BYTES`（`data_compute.rs:31`，同时被
  `data_compute.rs:102` 与 `data_compute/chunked.rs:132` 使用），并**删除** `attachment_compute.rs` 里
  重复的 `128 * 1024` 字面量——前置检查与执行层因此不可能再漂移；
- `jobs::run` 的前置检查改为调用同一函数，并**直接**返回 `codes::INVALID_PARAMS`：
  这一支的分类是**构造性的**（这条检查本身就是参数错误），完全不读消息文本；
- `is_javascript_execution_failure` / `is_parameter_validation_failure`、优先级顺序
  （用户取消 → fence/superseded → `"cancelled"` → `"timed out"` → JS 标记 → 参数措辞 → 默认）**未改动**；
- 未改 `execute_with_options` 的执行语义（快照/QuickJS/预算）、`data_compute` 逻辑、schema、Pi 引擎、
  审批、F3 超时、批次 C；未做跨模块接口或错误类型重构。

### 20.3 红例（修前，固定提交运行）

`bcf81dc`，日志 `compute-overlong-red-bcf81dc.log`，限时 120 s，退出码 **101**，114.84 s（含一次性编译）：
真实 DB/会话/附件 + 真实 chunked 执行 + **非空、`MAX_CODE_BYTES + 1` 字节**的代码，
该代码若被真正执行会**成功完成**（不是抛错），因此 `Failed` 不可能是执行附带结果：

```
left:  Some("compute.execution_failed")   ← 修前：什么都没执行却报执行失败
right: Some("compute.invalid_params")
message: "code must be nonempty JavaScript within 128 KiB"
```

用例另断言：无任何 progress `report` 事件（执行未开始）、消息不含 `JavaScript execution failed`。

### 20.4 修复后验证（串行，隔离 runner，`SQLITE_DEFAULT_MEMSTATUS=0`，默认并行）

| # | 范围 | 命令要点 | 限时 | 退出码 | 时长 | 结果 | 日志 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 修后绿例（同一条红例测试） | `an_over_long_program_is_rejected_as_invalid_params_without_running_javascript` | 120 s | 0 | 6.63 s | **1/0** | `compute-overlong-green-df9901c.log` |
| 2 | jobs 分类模块整组 | `attachment_compute::jobs::tests::` | 120 s | 0 | 3.75 s | **17/0** | `compute-jobs-module-df9901c.log` |
| 3 | 真实 Host/Node/Pi ＋ 本地模拟 Provider（live/逐轮 × failed/cancelled） | `failed_and_independently_cancelled_jobs_auto_resume_on_both_pi_transports` | 180 s | 0 | 21.24 s | **1/0（4 子模式）** | `compute-terminal-df9901c.log` |
| 4 | 默认并行全量 | `--lib` | 240 s | 0 | 128.77 s | **1306 通过 / 0 失败 / 27 忽略** | `compute-fix-full-02-df9901c.log` |

全部四项的 runner JSON `sourceHead` 均为 `df9901c`，`sourceStatus` 仅两份 schema WIP ＋未跟踪文档目录。
编译缓存预热由 `compute-fix-warmup-build.log/json`（`--no-run`，47.26 s，退出码 0，运行于提交前的工作区）
完成，**该文件不是证据**，仅用于让固定提交的运行落在 120 s 限时内；未因此修改任何源码内容。

### 20.5 参数入口矩阵与长度边界（新增用例，均为真实 job 路径）

均经 `run_on_lifecycle` → 生产 `jobs::run`，配真实 DB/会话/附件；每个用例都传**合法非空 `code`**，
因此 job 前置检查不会短路，错误必然来自被点名的校验，并断言**该校验自己的消息**、单一
`settle_failed:compute.invalid_params`、且**没有任何 progress 事件**（执行未开始）：

| 用例 | 输入 | 命中的校验消息 | 结果 |
| --- | --- | --- | --- |
| `a_real_job_rejects_an_unknown_processing_mode_as_an_invalid_parameter` | `processing:"streaming"` ＋合法 code | `processing must be 'whole' or 'chunked'` | `invalid_params` |
| `a_real_job_rejects_an_unknown_profile_as_an_invalid_parameter` | `profile:"enormous"` ＋合法 code | `data-compute profile 'enormous' 不存在…` | `invalid_params` |
| `a_real_job_rejects_an_incompatible_profile_and_processing_combination` | `profile:"large"` ＋ `processing:"whole"` ＋合法 code | `profile=large 需要 processing=chunked…` | `invalid_params` |
| `the_job_accepts_a_program_at_the_byte_limit_and_rejects_one_byte_more` | 恰好 `MAX_CODE_BYTES` 字节 | 无（参数合法） | **Completed**，且结果已发布 |
| 同上（第二个 harness） | `MAX_CODE_BYTES + 1` 字节 | `code must be nonempty JavaScript within 128 KiB` | `invalid_params` |
| `an_over_long_program_is_rejected_as_invalid_params_without_running_javascript` | `MAX_CODE_BYTES + 1` 字节，含真实附件 | 同上 | `invalid_params`，无执行 |

长度边界因此两侧都被钉住：**等于上限仍合法并真的跑完**，**上限＋1 被拒**，前置检查不会把合法长度的代码一并拒绝。

### 20.6 既有行为保留（第 2、3、4 项运行覆盖）

- 真实 JS 异常仍为 `compute.execution_failed`：`a_real_javascript_exception_is_an_execution_failure_not_an_invalid_parameter`
  （异常文本刻意含 `processing`/`profile`/`档位`/`code `），在 #2 通过；
- 缺 code → `invalid_params`、过期 deadline → `timed_out`、fence → `superseded`、取消 → `cancelled`：
  `invalid_params_and_deadline_produce_their_own_terminals`、`a_fenced_attempt_stops_early_and_writes_no_terminal`、
  取消/结算竞态用例在 #2 通过；
- 持久 Job 与模型通知错误码一致、`attempts == 1`、唯一续答（`requests.len() == 3`）、
  typed notice 逐字段对照、正式工具结果不重复（`parks, wakes, replies, formal == (1,1,1,1)`）：#3 通过，
  该用例的断言与源码未在本卡改动。

### 20.7 未覆盖项 / 需协调者判断

- 分类器对其余参数分支仍是**措辞约定**（§19.8），本卡未做类型化来源重构：若日后校验层新增措辞，
  仍可能落到 `execution_failed`。是否需要把“校验层错误来源”结构化（例如返回 `(Kind, String)`）超出本卡范围，
  请协调者判断。
- `runtime_host/background_jobs.rs:57` 另有一处重复的 `v.len()>131072` 字面量（同步工具调用路径，
  非 job 执行器）。本卡未动它，以免扩大范围；它与 `MAX_CODE_BYTES` 是否应收敛为同一常量，请协调者判断。
- 未做真实桌面 UI、真实（付费）Provider、安装包验收；#3 为本地 HTTP 模拟 Provider。
- 全量为**一次**默认并行运行（1306/0/27），未做重复压测。

