# Host 验收覆盖映射：U03/U04/U05/U06/U07/U08/U09/U10

基线为 `main / daa799ffd22dcc1e5fd17815643ef4ba4b1247ff`，叠加本轮未提交 WIP。本次补记 U06 七个真实进程中断点的两次增量证据，以及 `root-native-isolation-jobs-v3.log` 的 U10 记忆/知识隔离两项通过。U09 Job 与 U08 SDK 精确通过继续保留。此前 `root-native-overlap-crash-smoke-v2.log` 为 5 pass、1 fail，U03 真 Host 纯读时线通过，保留该次 U06 失败；`root-native-restore-conflict.log` 的 14/14 包含编译 sidecar 的 U04 真 Host 用例。覆盖映射基于已有日志和源码，本 Agent 没有启动 cargo，没有改产品行为或 `docs/`。root 另外授权的测试夹具/测试专用探针单独登记，未执行的新增用例不计通过。

定义来自 `docs/05-开发指南/Fox单路径运行时收敛方案-20261028.md:180–201`；阶段门槛来自同目录执行方案第七节。本文不是最终构建或业务验收报告。各日志来自不同增量构建，不能把通过数相加称为同一最终构建全过。

## 证据层级与日志索引

| 标识 | 已核对证据 | 边界 |
| --- | --- | --- |
| H1 | `root-native-failure-reset.log:1006–1010`，最新精确 3/3 通过 | 其中 `native_host_real_sidecar_orders_commits_and_isolates_two_sessions` 为真实 windowless Tauri RuntimeHost、磁盘临时 DB、真实 Pi 子进程和 faux 模型；不是外部模型或桌面 UI。最新已含 A reset、B 活动隔离。 |
| H2 | H1 中另外两个 `native_approval_scope` 测试通过 | 真实 RuntimeHost、原审批/文件处理器和临时文件；DB 为 `:memory:`，worker 是 `node -e process.stdin.resume()`。这是 Host 组件接线证据，不是真实 Pi 审批重启。 |
| H3 | `root-native-compiled-sidecar.log`：1/1 | root 使用 `FOX_TEST_NATIVE_SIDECAR_BIN` 跑 H1 同一强断言测试；实际 compiled sidecar 通过。仍为 faux/windowless Host 夹具，不是已安装桌面或外部模型。exe/SDK/构建哈希由 root 的候选报告负责。 |
| H4 | `root-native-restore-conflict.log`：14/14，包含 U04 真实 Host 共文件竞争用例 | root 指定本轮 dist executable；两个实际 Pi 会话同项目、先读同版本后 edit、唯一成功/原失败 receipt wire 重放及各自 JSONL 身份均通过。其余 13 项不转计为 U04。 |
| H5 | `root-native-overlap-crash-smoke-v2.log`：U03 精确项通过，组内 5/6 | 真实 Host + SQLite + dist compiled Pi + faux，6 个实际 Rust 文件读取循环峰值 4。组内 U06 失败保留，不能记为整组通过。 |
| H6 | `root-native-isolation-jobs-v3.log`：3/3，运行 4.58s、编译 53.67s | U09 Job 为真实 windowless RuntimeHost + 磁盘 SQLite + Pi 子进程 + 本地 SSE 模型 + QuickJS CSV 执行器。另两项 U10 为真实 Host 工具处理器、磁盘 SQLite/知识库，worker 只回显协议，不运行 SDK 或模型。三项均不是桌面 UI 或外部模型验收。 |
| H7 | `root-native-crash-guard-v2.log`：21 pass、1 fail；U06 原报告 7 点中前三点 valid | 真实 Rust Host + Pi + 磁盘 DB/JSONL，在隔离 Windows Job Object 内硬终止后重开。`register_before_ack / register_after_ack / native_claimed` 有效，后四点未到达；该组及原 report 的失败保持不变。 |
| H8 | `root-native-crash-last-four-v3.log`：精确 1/1，运行 13.56s | 同一 U06 精确入口仅选择后四点；修改原文件的请求经真实资源执行后，在 resource_returned、Host 提交、native 提交未回传、Pi 已落盘处中断/重开均有效。与 H7 是不同增量构建，不能记成某一份原日志七点一次全过。不是断电/fsync 或桌面 UI 证明。 |
| R1 | `root-native-contracts-final.log`：117 项，116 pass、1 fail | 已通过项覆盖真实 Rust 台账/临时 DB/资源处理器或数据库契约。原唯一 startup scope 失败已由 H1 精确复跑闭合；保留该历史失败，不能改写为本文件原运行 117/117。 |
| R2 | `root-native-final-exact.log`：5 项，4 pass、1 fail | 真实 Host 链和拒绝 receipt 三项通过；同一 startup scope 当时仍失败，后由 H1 闭合。 |
| R3 | `root-native-lifecycle-final.log`：27/27 | 生命周期/重置组件测试，多数使用脚本化 runtime link，不替代 H1 或桌面验证。 |
| R4 | `root-read-version-contract-workspace.log`：精确 1/1 | 同一 perf-build test exe 的 `r4_read_version_reaches_model_and_drives_conflict_detection`。默认 Temp 运行在 `root-read-version-contract.log` 因 backup rename OS5 失败；只将进程 TEMP/TMP 改为独立 workspace 临时目录后通过，未放宽断言。这是底层版本契约，不升级为 native Host U04。 |
| R5 | `root-native-isolation-user-copy-v2.log`：1 pass、2 fail | 原 U10 记忆夹具固定复用已退休代次、知识夹具误读 bounded 结果结构而失败；后由 H6 按实际会话映射/结果结构修正通过。该旧组仍为 1/3；其中通过项是 U11 用户副本升级回退，不转计 U10。 |
| J1 | `js-final-directed-v4.log:153–159`：149 项，148 pass、0 fail、1 TODO | 真实 SDK、真实 Pi 子进程或 JS 单元测试；Host 为桩。TODO 为既有 Legacy 混合读写屏障失败，不能写为 149 全过。 |
| J2 | `js-integration-registration-core-v2.log`：40/40；`js-integration-registration-subprocess-v2.log`：6/6 | 登记 ACK/FIFO 修正后的精确 JS 回归。实际子进程与 faux，但仍是 Host 桩。J1 是此前较宽快照，不是此修正之后全量重跑。 |
| J3 | `js-s3-crash-recovery.log`、`js-s3-sdk-cancel-late-receipt.log`；完整 7 个状态见 J1:128–134 | 真正 `process.exit(73)` 终止并重开 Pi，Host facts、效果计数和提交状态由 JS 测试模拟。 |
| J4 | `js-s3-model-budget.log`：10/10；`js-s4-http-isolation.log`：1/1 | 请求/重试/压缩预算 hooks；两个本地 HTTP 端点的模型和测试鉴权隔离。不是外部供应商验收。 |
| J5 | `root-native-compaction-reopen.log`：精确 1/1，测试 303.36ms、Node 总计 2059.70ms | 真实 SDK 手动 `session.compact()`、关闭 AgentSession、同进程重开真实 JSONL；模型为本地 faux，Host facts 为夹具。不是自动阈值触发、Host/进程重启或外部模型/UI 验收。 |
| K1 | `rust-kernel-http-groups-unsandboxed.log`：32/32 | 17 个 delivery_live、8 个 limit_fix、5 个 steering_live 和 2 个 native budget。主要是旧 Kernel authoritative 回归；不能转记为 PiNative 的完整业务链。日志保留 detached fixture 线程的 `missing model request for stop` panic，不把它当作无诊断的全绿。 |
| S | 下文列出的现有源码或 `rust-kernel-list.log` 测试名 | 仅发现入口。除明确关联上表日志的测试外，本轮未执行，不能算通过。 |

## 总览

| 项目 | 本轮已证据 | 仍缺证据/下一步 |
| --- | --- | --- |
| U03 读并发、混合屏障、稳定结果顺序 | H5 真 Host 的 6 个实际文件读取循环峰值 4、每次完整 8MiB；H1 真混合屏障/源序；SDK 和 Rust claim 契约补充 | 所选 Host+compiled Pi+faux 场景已覆盖；门外同步起跑下的资源循环重叠不代表物理磁盘同时在途或无干预性能收益。 |
| U04 共文件冲突、版本变化、未知 MCP/命令、权限 | H4 已测真 Host 两会话同版本共文件争写与失败 receipt 重放；R4 版本变化契约；R1 原生凭据/撤销/只读/原审批保护；分类把 MCP、命令放在独占屏障；H2 重建 scope | 共文件关键链已通过；未知 MCP/命令与实际审批/资源冲突组合仍未闭合，不能把白名单分类测试当实际 MCP/命令效果验收。 |
| U05 不合法位置、重放、乱序、缺回调和取消 | R1/R2 的协议/台账测试及 J2 的 SDK 生命周期矩阵通过；H1 证实提交先于交付 | 选定协议夹具已覆盖。最终候选仍应按构建快照复跑；Legacy 既有 TODO 保留，不以 PiNative 通过消除它。 |
| U06 逐点中断、不重做、只补交付 | H7 前三点与 H8 后四点覆盖七个实际 Rust Host/Pi 进程中断窗口；真实 DB/JSONL 重开、已提交结果补交一次、未知效果阻断且不重执行通过；J3/R1 组件证据补充 | 七点已有真实进程证据，不再记为尚未联合注入。两次增量运行不冒充同一最终构建全量；不外推断电/fsync、审批 UI 或外部模型。 |
| U07 审批/撤销/过期/重启/批量等待、总预算 | H2 Host-only startup 批准/拒绝/跳过已 claim/失败不重放；R1 native 审批保护与持久总预算；J4 重试/压缩预算 hooks | 缺真实 Pi+磁盘 DB 重启+审批 UI 决策续接；native 过期语义及多工具同时等待的完整 Host 链待补。并发请求、重试和压缩共享预算尚无完整 Host 组合链。 |
| U08 压缩后重开 | J5 真实 SDK 手动压缩、compaction 记录持久化、同进程关闭重开后下一模型上下文保留摘要/原证据/近期约束 | 只闭合该精确 SDK 场景；自动阈值、Host/进程重启及桌面/外部模型不在用例范围。 |
| U09 Job/子任务/待用户/交付/通知 | H6 真实 Native Job start/status/result；真实 Host 子运行 collect/父续聊精确通过；R1 completion 守卫与 graph 数据库恢复；K1 既有 Kernel 回归 | 正常 Job 与最小子运行链已通过。模型 Job notice 是默认关闭的 Kernel 实验；待用户公开续接仅 Yuxi。应用通知/交付的实际边界见下文，不据局部通过宣称整个 U09 或桌面验收完成。 |
| U10 普通/项目/联系人隔离 | H6 两项真实 Host 记忆/知识工具隔离及越权拒绝已通过；H1 两会话/项目、独立 Pi 文件、A cancel/reset/reject 不改 B；R1 身份与 scope 拒绝 | 后端记忆/知识缺口已闭合，仍需当前包内桌面实际会话、专家/项目与知识引用展示核对。U10 两项的协议回显 worker 不等于 Pi/模型链。未交付联系人 M1/M2 仅记 NA。 |

## U03：可复用精确入口

已通过的 Rust 全名（R1）：

- `runtime_host::native_tool_batch::tests::six_readers_claim_at_most_four_slots_atomically`
- `runtime_host::native_tool_batch::tests::mixed_batch_waits_for_every_earlier_barrier`
- `runtime_host::native_tool_batch::tests::only_the_frozen_read_whitelist_may_overlap`
- `runtime_host::native_tool_batch::tests::native_reader_executes_through_the_real_rust_gateway_and_replays_the_receipt`

真实 Host 链全名（H1）：`runtime_host::native_host_integration_tests::native_host_real_sidecar_orders_commits_and_isolates_two_sessions`。在三个实际工具结果交付给 Pi 前，核对 native receipt 与 Host 工具结果已提交、读前/写后值、真实磁盘内容；交付后核对 `mix-before / mix-write / mix-after` 源序。

已通过的 JS 标题：

- `test/pi-native-tool-bridge.test.mjs`：`a mixed batch keeps the read-only prefix parallel and the write a barrier`（J2）。真实 SDK 调用、桩 Host 窗口断言两前置读取重叠，写晚于两读结束，后读晚于写结束。
- 同文件：`the read-only prefix never exceeds the concurrency limit`（J2）。6 个读取的窗口扫描验证峰值不超过 4。
- `test/pi-kernel-readiness.test.mjs`：`parallel tool batch commits every result and presents them to the next model call in source order`（J1）。这是 SDK/桥接结果序证据，不是 Rust Host 纯读资源时线。

H5 已运行真实 Host 精确入口：`runtime_host::native_host_integration_tests::native_host_six_readers_overlap_real_file_loops_within_four_slots`。6 个独立 8MiB 文本，真实 SDK 单批读取，核对完整文件版本和 64 字符预览。测试专用 probe 只匹配 6 个精确 canonical 路径；首波 4 个文件打开后最多等待 5 秒，起跑门和 probe 分配在计时外；实际 `File::read` 循环内无新增 sleep/gate/共享锁，只做局部调用/字节计数。

原始证据：`migration-work/fixtures/fox-native-pure-read-e3cb243e-9cf1-459f-b7dd-2edb42683b71/resource-read-intervals.json`。同目录 `sidecar-launch.json` 明确启动 `services/agent-runtime/dist/fox-agent-runtime-x86_64-pc-windows-msvc.exe`、`args=[]`、`compiledSidecar=true`。

| 文件 | 实际线程 | 循环耗时 ms | 字节数 | Read 调用数 | EOF 完成 |
| --- | --- | --- | --- | --- | --- |
| pure-read-0.txt | ThreadId(7) | 7.7795 | 8,388,608 | 513 | true |
| pure-read-3.txt | ThreadId(10) | 7.9803 | 8,388,608 | 513 | true |
| pure-read-1.txt | ThreadId(8) | 7.9716 | 8,388,608 | 513 | true |
| pure-read-2.txt | ThreadId(9) | 8.4144 | 8,388,608 | 513 | true |
| pure-read-5.txt | ThreadId(12) | 7.5676 | 8,388,608 | 513 | true |
| pure-read-4.txt | ThreadId(11) | 6.2160 | 8,388,608 | 513 | true |

独立扫描原始 `enteredNs/exitedNs` 事件复算得到峰值 **4**、区间结束后 active **0**，总计 **50,331,648 bytes（48MiB）**。513 次调用是 512 个 16KiB 数据块加 EOF；六次读取都完整完成。`gateReleasedNs=1,164,518,900`，首波四条读取区间均在放行后开始并重叠；后两条也重叠。

阶段含义：`openedNs` 采在真实文件打开、类型与目录验证之后；`enteredNs` 在起跑门和 probe 分配完成之后、读取循环之前；`exitedNs` 在 EOF 后、记录共享锁之前；`completed` 只表示读取循环正常结束，不单独证明 Host 结果已提交。整体工具提交、回包前台账和 Pi 源序由该真实 Host 测试后续断言核对。以上不是 claim 时间，不包含哈希、DB 提交或回包等待，也不是物理磁盘同时在途/性能收益测量。

## U04：已测边界与共文件缺口

已通过的精确入口（R1/H2）：

- `database::repositories::kernel_execution_admission::tests::native_file_credential_preserves_host_facts_and_the_single_execution_claim`
- `database::repositories::kernel_execution_admission::tests::native_file_credential_refuses_read_only_stale_policy_and_revocation_before_claim`
- `database::repositories::kernel_execution_admission::tests::native_file_credential_still_requires_the_original_consumed_approval`
- `runtime_host::native_approval_scope::tests::approval_recovery_rebuilds_child_and_digital_tool_and_mcp_scopes`
- `runtime_host::native_tool_batch::tests::only_the_frozen_read_whitelist_may_overlap`（`call_mcp_tool`、`run_command` 均不进入并行读取白名单）。

仅源码发现、可精确复用但本轮未执行（S）：

- `tool_host::revision_regression_tests::r4_stale_preparation_and_stale_read_cannot_overwrite_new_content`。
- `runtime_host::managed_files::tests::admitted_file_second_execution_rejects_an_externally_changed_old_baseline`。
- `runtime_host::managed_files::tests::a_generic_or_unfrozen_connector_is_never_a_managed_write`。

R4 已精确运行 `resource_gateway::tests::r4_read_version_reaches_model_and_drives_conflict_detection`：真实读取版本→外部修改→原版本写入拒绝→重读后可写。原默认 Temp OS5 失败保留，不转写为产品冲突契约失败或无条件成功。

H4 的 `runtime_host::native_host_integration_tests::native_host_shared_file_conflict_preserves_the_failed_receipt` 已验证同一项目目标、两次真实读取都在写前、恰一成功/一冲突、磁盘只保留成功者、失败 receipt 原样 wire 重放且不再执行，以及两会话身份。上述其他旧资源入口仍只记源码，不因 H4 通过而冒称已运行。

## U05：协议与 SDK 异常闭合

已通过的 Rust 全名（R1）：

- `runtime_host::native_tool_batch::tests::registration_refuses_sparse_duplicate_and_bad_digest_without_rows`
- `runtime_host::native_tool_batch::tests::registration_is_atomic_idempotent_and_survives_registry_restart`
- `runtime_host::native_tool_batch::tests::effective_input_and_committed_result_are_immutable`
- `runtime_host::native_tool_batch::tests::unknown_and_foreign_ids_never_claim_or_change_a_position`
- `runtime_host::native_tool_batch::tests::closed_and_unclaimed_cancelled_positions_release_waiters_without_execution`
- `runtime_host::native_tool_batch::tests::cancelled_inflight_write_keeps_blocking_and_late_success_is_failure`
- `runtime_host::native_tool_batch::tests::registration_after_cancellation_commits_only_nonexecution_and_can_reconcile_pi_results`
- `runtime_host::native_tool_batch::tests::response_is_not_constructed_until_the_result_commit_succeeds`

拒绝响应不可变的精确补测（R2，三项均通过）：

- `runtime_host::native_tool_batch::tests::capacity_and_policy_refusals_return_the_committed_nonexecution_receipt`
- `runtime_host::native_tool_batch::tests::claim_failure_cancellation_and_timeout_return_the_same_committed_receipt`
- `runtime_host::native_tool_batch::tests::refusal_never_settles_foreign_identity_changed_input_or_an_unknown_claim`

J2 对应 `test/pi-native-tool-bridge.test.mjs` 标题包括 `a position that never executes is closed with an honest reason`、`beforeToolCall waits for registration only, not early close or execution settlement`、`early unknown position releases the barrier (await events: false)`、`early invalid position releases the barrier (await events: true)`、`a lost-ack close failure propagates and duplicate SDK events can retry`、`an aborted SDK turn registers and closes positions that never reached preparation`。这些覆盖未准备/无执行回调位置、ACK 丢失重试与取消清理；不能把 Host 桩当真实资源执行。

J2 子进程精确标题 `an entirely unknown and invalid native batch registers before lifecycle projection and closes every position` 覆盖真实 SDK 登记顺序；H1 已越过原 SDK `tool.started` 早于登记的真实接缝故障。

## U06：中断窗口逐项对照

精确入口为 `runtime_host::native_crash_integration_tests::native_host_crash_boundaries_reconcile_without_reexecution`。夹具在 nonce/PID/Run/调用 ID 和持久状态均符合目标窗口后，关闭仅持有本测试进程树的 Windows `KILL_ON_JOB_CLOSE` Job Object，随后另起真实 Host 重开原磁盘 DB 与 Pi JSONL。检查点不伪造或改写 Host 回执。原始报告：

- H7：`migration-work/native-crash-suite-38e7021f-0e4b-4160-93e1-a326b012852f/report.json`，整份 `valid=false`；前三点为 true，后四点为 false。
- H8：`migration-work/native-crash-suite-3fc3ecce-d665-43dc-ad04-d55f0e1cf0ad/report.json`，仅运行后四点，四点及整份 `valid=true`。

| 实际窗口 | 真实进程证据 | 恢复判定 |
| --- | --- | --- |
| 登记已提交，ACK 尚未交给 Pi：`register_before_ack` | H7 valid=true；Host 台账 pending、未 claim。此点不代表登记事务提交前。 | facts 为 not_executed，补交一条原失败回执；无工具重新执行，资源字节不变。 |
| Pi 已收到登记 ACK、未领取：`register_after_ack` | H7 valid=true；Host 台账仍 pending、未 claim。 | 同上，原 session/代次保留并完成下一 Run。 |
| 已领取、尚无结果：`native_claimed` | H7 valid=true；台账 running 且 claimed_at 已写。 | outcome_unknown，拒绝新 prompt；原 JSONL 与资源字节保持，不发送 tool_call.execute。 |
| 实际文件效果已返回、结果未提交：`resource_returned` | H8 valid=true；真实文件已修改，Host/native 尚无完整提交结果。 | outcome_unknown，保持已经发生的文件效果；不重执行、不伪补结果。 |
| Host 工具结果提交、native receipt 未提交：`host_result_committed` | H8 valid=true；Host 原结果存在，native response 尚空。 | 按持久原结果重建并补交一次，原 claimed_at/inputDigest 保持。 |
| native receipt 提交、尚未回传：`native_result_before_delivery` | H8 valid=true；relay held-frame payload 等于已提交 native receipt。 | 补交一次原结果，文件保持中断后的字节，不再执行调用。 |
| Pi 已写入结果：`pi_result_persisted` | H8 valid=true；真实 JSONL 已有该结果。 | 保持唯一结果，原 session/代次和资源字节不变，下一 Run 可完成。 |

H7 原后四点未到达：当时请求没有明确要求修改/保存原文件，被既有输入分类拒绝，不能视为有效崩溃。H8 只修正该夹具请求，保留真实资源分类与文件字节强断言后复跑四点通过。H5 的原 U06 失败、H7 的 21/22 和原 `valid=false` 报告均保留；不能把两次增量拼成一份原运行全绿。测量边界为进程硬终止，不是机器断电或 fsync 持久性证明，也不是审批/桌面 UI 验收。

J3 的 Host 桩退出矩阵、R1 的双提交窗口/未知效果数据库契约仍作为组件补充，不再用于替代上述真实进程证据。

其他已通过的 Rust 精确入口：

- `runtime_host::native_recovery::tests::pi_only_results_abort_before_any_partial_nonexecution_write`
- `runtime_host::native_recovery::tests::a_startup_failed_status_without_a_real_result_stays_unknown`
- `runtime_host::native_recovery::tests::unknown_outcomes_and_unclaimed_approvals_block_without_appendable_results`
- `runtime_host::native_tool_batch::tests::finalize_retains_late_native_output_without_reopening_terminal_host_business_state`
- `runtime_host::native_tool_batch::tests::disconnected_receipts_retire_only_the_original_run_after_all_claims_settle`

本轮采用更保守的约定：`outcome_unknown` / `waiting_approval` 在任何 branch/append 前阻断，原 JSONL 分支不变，不补未知合成失败。已记录于 `HOST-native-recovery-contract-20261028.md`；不把它描述为已经执行了旧执行稿的“未知失败补交”。

## U07：审批和总预算

H2 最新通过：

- `runtime_host::native_approval_scope::tests::startup_recovers_only_decided_unclaimed_native_host_approvals`：普通和 Repair 专用类别，选择 approved/denied 未 claim；pending 和实际已 claim 保持不动；普通批准实际修改临时文件，拒绝不修改。Repair 的非法参数是原 handler 拒绝样本，不等同 Repair 成功效果。
- `runtime_host::native_approval_scope::tests::approval_recovery_preserves_failed_file_execution_and_never_replays_it`：真实备份目录故障，原文件不变、Host/native 都保留失败和原 execution receipt；移除故障后 direct recovery 和 startup selector 仍不重做。

R1 另有以下通过：

- `runtime_host::native_tool_batch::tests::recovered_repair_approval_executes_the_original_dedicated_budget_transaction_once`（真实专用 Repair 预算事务）。
- `runtime_host::native_tool_batch::tests::approval_recovery_refuses_current_read_only_permission_and_actual_claims`。
- `runtime_host::native_tool_batch::tests::tool_cancellation_is_visible_to_an_approval_waiter_without_settling_the_claim`。
- `database::repositories::native_route_tests::native_restart_preserves_unclaimed_host_approvals_and_their_native_facts`。
- `runtime_host::native_model_budget::tests::initial_retry_and_compaction_requests_share_the_persisted_token_budget`。
- `runtime_host::native_model_budget::tests::admission_replay_cannot_reset_any_model_time_window_or_token_reservation`。
- `runtime_host::native_model_budget::tests::missing_usage_retains_the_reservation_after_database_reopen`。
- `runtime_host::native_model_budget::tests::host_run_time_budget_is_tightened_by_elapsed_execution`。
- `runtime_host::native_model_budget::tests::terminal_cleanup_is_allowed_but_no_new_model_request_can_start`。
- `runtime_host::native_model_budget::tests::approval_wait_is_excluded_from_the_whole_run_execution_clock`（记录现有执行时钟语义，不将等待时长伪计为模型用量）。

J4 的 `each HTTP retry is admitted and settled independently before transport, with its current output cap` 与 `a result-only compaction consumer awaits admission and completion with matching usage identity` 分别覆盖实际重试 hook 和 result-only 压缩消费；J1 的 `real SDK compaction uses Host request admission and records compaction usage` 验证 SDK history/turn-prefix 两次 summary。它们仍用 Host 桩，未证明真实并发 Rust 预算组合链。

仅源码可复用（S）：`database::repositories::kernel_authorization::tests::revocation_takes_effect_immediately`、`database::repositories::kernel_authorization::tests::unknown_or_stale_approval_version_cannot_register_a_grant`、`runtime_host::kernel_coordinator::tests::f3_approval_tests::f3_live_cancel_wins_over_approval_in_same_host_batch`。后者是旧 Kernel 入口，不能原样充当 PiNative 审批重启。native pending 审批重启后保留是有意行为，不能借旧“统一过期”测试把它清掉。

## U08：真实 SDK 手动压缩与同进程重开

J5 精确标题为 `services/agent-runtime/test/pi-native-recovery.test.mjs` 的 `real SDK manual compaction retains native evidence and required context after closing and reopening`，`root-native-compaction-reopen.log` 记录 **1/1 通过**。

该用例调用真实 `createFoxAgentSession` / `session.compact()`，由 SDK 写入 compaction entry，没有手造压缩记录。实际摘要请求包含原 Host 证据；关闭 AgentSession 后，用 `SessionManager.open` 重开同一 JSONL，核对 compaction 记录及其保留起点完全一致，两个 native 调用仍有结果，facts 对账为 `checked=2 / appended=0` 且文件不变。随后真实 SDK 的下一次模型上下文含压缩摘要、原证据、已完成工作和近期项目约束，不再携带最老 filler 全文。

模型为本地 faux，Host facts 由夹具提供。这是手动压缩与同进程 reopen 的 SDK 证据，不代表自动压缩阈值、真实 Host/进程重启、外部模型或桌面验收。现有损坏会话拒绝测试仍保留其原证据状态，不因本项通过而计为新通过。

## U09：业务契约与 native 组合缺口

R1 已通过的精确入口：

- `database::repositories::native_route_tests::native_completion_requires_settled_host_business_state`：pending/无效交付检查、活动 child、活动 Job 都阻止模型 `run.completed`；业务事实结算后才可完成。
- `database::repositories::native_route_tests::native_completion_cannot_skip_unsettled_or_unknown_tool_outcomes`。
- `database::repositories::graph_lead::tests::native_parent_cancel_intent_stays_in_the_host_recovery_query`。
- `database::repositories::graph_lead::tests::active_node_attempt_and_child_snapshot_survive_database_restart`。
- `database::repositories::graph_lead::tests::graph_child_negative_terminals_reconcile_once_using_actual_status`。
- `database::repositories::graph_lead::tests::high_risk_graph_review_finish_and_final_acceptance_complete_end_to_end`（数据库业务契约端到端，不是 PiNative 真实协作/桌面端到端）。

K1 已通过：

- `runtime_host::kernel_coordinator::tests::delivery_live_tests::a_crash_after_the_round_decision_is_recovered_from_the_staged_verdicts`。
- `runtime_host::kernel_coordinator::tests::delivery_live_tests::a_crash_before_the_round_decision_never_finalizes_the_staged_verdicts`。
- `runtime_host::kernel_coordinator::tests::delivery_live_tests::live_delivery_repair_loop_rides_continuation_without_consuming_review_budget_or_replaying_tools`。
- `runtime_host::kernel_coordinator::tests::steering_live_tests::a_request_accepted_inside_a_tool_batch_window_executes_the_tool_once`。

可复用的精确入口（S，本轮只查到源码和 test list，未运行）：

- `runtime_host::kernel_coordinator::tests::host_job_notice_tests::real_runtime_host_job_start_parks_and_wakes_on_both_pi_transports`。
- `runtime_host::kernel_coordinator::tests::host_job_notice_tests::real_host_to_local_provider_delivers_notice_once_in_live_and_single_round`。
- `runtime_host::kernel_coordinator::tests::waiting_jobs_recovery_tests::committed_job_notice_recovers_once_across_reopened_hosts_and_pi_transports`。
- `runtime_host::kernel_coordinator::tests::waiting_jobs_recovery_tests::bound_job_notice_continuation_lease_is_not_replayed_after_reopen`。
- `runtime_host::kernel_coordinator::tests::waiting_jobs_host_tests::waiting_jobs_recovery_detached_accounts_and_releases_host_without_retiring_job_token`。

这些旧测试的两种 Pi transport 指旧 Kernel live/single-round，不是自动包含 `executorRoute=pi_native`。需要先核对或添加原生绑定/Host 路由，再跑 Job 和通知恢复；不能因名称有 `real_runtime_host` 就升级覆盖范围。实际待用户流程、UI 通知和完整业务组合仍需候选验证；最小原生子运行链的当前证据见下文，模型正文仍不具有修改业务结论的权限。

`runtime_host::native_business_integration_tests::native_worker_child_collect_inherits_binding_and_preserves_sibling` 已在 `root-native-child-collect-localhost.log` 精确 **1/1 通过，3.54s**。同一已编译 test exe 允许 localhost 后复跑，使用真实 Host、磁盘 SQLite、compiled Pi 和 Rust loopback scripted SSE；provider 只能从真实 `child_run_start` 的 role=tool JSON 结果解析 childRunId，再发 `child_run_collect`。兄弟和父 Run 先占满两槽，实际断言 child queued；collect 的 `SidecarExecutionWait` 释放父名额后，child 真正执行 Host read，兄弟保持运行，三会话独立且 occupied=2。随后恢复父名额、读取真实 collect 结果，核对完整冻结 PiNative binding/权限/限时继承、持久 child/token 预算、共享 PID、child cleanup 不杀兄弟、唯一收取结果和父会话下一 Run。

保留前两次失败及归因，不将原日志改写为通过：

- `root-native-child-collect.log`：首次编译通过，首个 sibling read 等待超时；临时 DB 为 `provider.request_failed / Connection error`，3 个失败模型请求、零工具。夹具对 accepted stream 遗漏的 `set_nonblocking(false)` 后续按已有 perf helper 补齐，并补实际请求/错误持久诊断；不能只凭该源码差异断言首次连接失败根因。
- `root-native-entry-guard-child-v2.log`：新编译组 **12 pass、1 fail**，唯一 U09 在沙箱中报相同连接错误，provider 记录为空，未进入工具或 collect。对同一已编译产物开放 localhost 后上述精确项通过，确认该次阻断为执行环境的回环连接限制，不是产品 child/collect 失败；该组历史结果仍为 12/13。

这一结果闭合最小真实 Host 子运行/收取/父续聊链。Job、通知、待用户、外部模型与 UI 不在该用例覆盖范围，不能据此将整个 U09 或最终业务验收记为通过。

### U09 余项只读核对：Job、待用户、通知与交付

本节初稿写于正式性能测试期间，当时只读查证，没有复查已通过的 child collect。性能测试结束后，root 授权增加最小 Job 用例，并在 H6 统一运行通过；下文已更新该项。其他“可复用”仍只代表源码入口存在，本轮未找到对应通过日志的项仍记未运行。

| 项目 | 当前源码与已有证据 | 实际尚缺的原生证据 |
| --- | --- | --- |
| Native compute Job | H6 的真实 `compute_job_start` 返回 jobId，再 `status(waitMs=5000)` 和 `result`；真实 QuickJS 完成 CSV 计算，单个 Job、attempt=1、结果引用/字节/hash、原文件不变及三条 receipt/Pi 源序均通过。路由为 `execute_job_operation` → `background_jobs` → `host_lifecycle`。 | 正常 start/status/result 链已闭合；本例不覆盖自动 notice、待用户或桌面 UI。 |
| 模型收到的 Job notice | `kernel_job_execution::notice_enabled` 查 `kernel_runs` 的冻结配置；`waiting_jobs_wake::signal_waiting_run` 只处理 Kernel `waiting_jobs`。现有 live/round 通知与重开测试均是该 Kernel 路线。 | 不能据旧测试宣称 Native 自动唤醒/通知已交付。它是 opt-in 实验，不能把未启用实验当成这轮新增功能要求。 |
| 应用通知列表 | `Database::sync_app_notifications` 从持久 primary Run / approval 状态 upsert `run:{id}` / `approval:{id}` merge key，和发给模型的 Job notice 是两条路径。 | 本轮没有 Native 真实终态→通知同步→再次同步/DB 重开仍不重复的联合日志，也没有桌面可见通知验收。 |
| 待用户 | 现有 pending-question/resume 测试直接向 DB 应用 `user.question.requested` 与 `completionReason=awaiting_user`。当前公开 `commands::run_resume` 参数是 `ResumeYuxiRunRequest`，明确拒绝非 Yuxi runtime；当前 `pi-runtime` / `host-tools` 未发现 `ask_user_question` 声明。 | 这些 DB 投影测试不是 Native 实际“提问→等待→回答→续跑”。按当前公开范围保留 Yuxi 证据，不在 Native 候选新建问答框架来凑验收。交付待用户见下一行，不能混为普通问答。 |
| 交付与待验收 | `start_run_inner` 明确不为新任务 seed 内容核验 checklist，只给 placement reference；已有冻结清单保留。K1 的 17 个 delivery_live 通过是旧 Kernel 合同回归；R1 Native completion 守卫只拒绝未结/无效清单，测试没有实际执行内容核验。 | Native 新任务的文件写盘、receipt 真伪可沿 H1/H4 已有证据，不能转称语义交付验收。旧冻结合同的核验/待用户兼容仍需按实际交付范围验证；不为凑 U09 恢复已简化的新任务自动 repair 流程。 |

实验性 Job notice 的具体开关：`FOX_EXPERIMENTAL_COMPUTE_JOB_NOTICE=1` 才在 `kernel_host` 写入冻结 `experimentalComputeJobNotice=true`；未设或其他值为 false。`RunFrozenConfig` 对旧记录该字段缺失也默认为 false。这是现存 Kernel opt-in，Native 最小 Job 测试不创建伪 `kernel_runs`、不开该开关、不实现新通知通道。

可精确复用的 Job 执行器入口（源码存在，本轮尚未执行）：

- `runtime_host::attachment_compute::host_lifecycle::tests::production_start_runs_the_real_executor_and_publishes_a_readable_result`：真实 worker / CSV 输入 / 结果 blob、字节与 hash。
- `runtime_host::attachment_compute::host_lifecycle::tests::production_start_is_idempotent_for_the_same_request`：同一个持久 start key 不增加新 Job。
- `runtime_host::attachment_compute::host_lifecycle::tests::production_restart_pauses_an_orphan_and_resume_reruns_without_replay`：该测试模拟 owner 消失，显式期望 resume 后 attempt 从 1 变 2；不能拿标题中的 without_replay 声称恢复零执行。

可复用的其它小入口（同样只查源码，未列本轮通过）：

- 应用通知：`database::repositories::app_management::tests::notification_merge_reopens_changed_status_and_preferences_persist`；`database::repositories::app_management::tests::resolved_approval_notifications_are_read_instead_of_reopened`。
- Yuxi/通用 DB 问答投影：`database::repositories::tests::restores_pending_user_question_events`；`database::repositories::tests::atomically_creates_a_resumed_run_and_records_the_response`。它们不调用 Native 公开问答入口。
- 原冻结交付决定：`runtime_host::delivery::delivery_u1::tests::a_first_acceptance_is_recorded_and_the_finding_is_untouched`；`runtime_host::delivery::delivery_u1::tests::a_repeated_identical_acceptance_is_a_no_op`；`runtime_host::delivery::delivery_u1::tests::a_rejection_is_recorded_without_an_acceptance_and_is_idempotent`。当前夹具的 frozen binding 为 `executor_route=None`，不能改称 Native Run。

H6 已运行正常 Native Job 精确链 `runtime_host::native_business_integration_tests::native_compute_job_start_status_and_result_use_the_real_executor`，该项通过。用例复用现有 `native_business_integration_tests.rs` 的真实 Host、SQLite、Pi、本地 SSE provider 和工具结果 JSON 解析；provider 只从实际 start 结果提取 jobId。项目内写隔离 CSV `value\n10\n20\n30\n`，实际使用下列工具参数：

```json
{"idempotencyKey":"u09-native-row-count","params":{"projectPaths":["job-input.csv"],"processing":"chunked","profile":"standard","code":"let rows=0; function onChunk(c){rows+=c.rows.length;} function onFinish(){return {rows:rows};}"}}
```

随后 `compute_job_status({jobId:实际ID,waitMs:5000})`，终态后 `compute_job_result({jobId:同一ID,offset:0,limit:4096})`。CSV 含表头，实际通过断言为 `computedBy=fox-quickjs / rowsProcessed=4 / result.rows=4`；持久 Job 一行、attempt=1，结果引用/hash/字节与完整 blob 一致，原 CSV 不变。三条 Host/native receipt 与 Pi JSONL 源序及 provider 实际所见逐一相同，各收一次。没有人工写 Job 状态或业务结果，没有扩展通知/问答/恢复框架。

现有执行器前置已由源码和 H6 正常链确认：Job 在 Host worker thread 上运行 `attachment_compute::jobs::run_on_lifecycle`，项目 CSV 经 `authorize_project_inputs` 形成只读私有快照，再由内嵌 QuickJS `data_compute::chunked::execute` 执行。沿用现有内存/栈/代码/输入限制、deadline 和 cancellation；此 CSV/纯计算路径不需要 command 的 verified OS process backend、Python、Office 或外部模型。模型运输仍需本地 HTTP 回环权限。H6 不覆盖实验性 Job notice，也不证明 UI 通知或待用户续接。历史 child collect 失败及其环境归因保持原样。

## U10：普通/项目是门槛，未交付联系人只记 NA

H1 的真实 Host 链使用两个独立项目目录，各自都有 `note.txt`。A 的真实混合写只改变 A 项目；B 读到 `survivor-project`。A cancel 后等待 `run_released`，在 B 的已提交读取响应仍被 relay hold 时调用真实 `reset_conversation_context`，验证：

- A 新 runtime session ID、generation +1、独立新路径；旧文件 byte-for-byte 不变且仍在退休归属台账。
- B 原 PID、Run、session ID、generation、文件映射及 held receipt 全部保持；随后 B 完成并在原会话继续下一 Run。
- A 新 Run 使用 reset 后身份；错 generation release 被拒绝；旧 transport 故障不杀新 worker；拒绝原错误原因保持。

R1 的 `runtime_host::native_recovery::tests::identity_generation_arguments_and_ambiguous_call_ownership_fail_closed`、`runtime_host::native_recovery::tests::retired_branch_calls_and_corrupt_frozen_bindings_are_never_reclassified_as_unexecuted` 和 H2 的 `runtime_host::native_approval_scope::tests::approval_recovery_rebuilds_child_and_digital_tool_and_mcp_scopes` 补充可信事实/审批 scope 的负例。J4 的 `concurrent native Runs send each actual HTTP request to its bound endpoint with its own model and credential` 为真实本地 HTTP + Host 桩配置隔离，不能替代普通/项目记忆和知识库归属验收。

H6 已通过两个精确入口：

- `runtime_host::native_isolation_tests::native_memory_tools_preserve_project_agent_and_source_ownership`：普通会话和两个项目只召回其允许的 global/agent/project 记忆；模型传入其他 conversation/project/agent 字段不能改归属。项目记忆提议仍绑定原会话/Run/message，保持未确认；外会话 sourceMessage 和无项目伪造项目 scope 均被拒绝。
- `runtime_host::native_isolation_tests::native_knowledge_tools_preserve_conversation_references_and_document_ownership`：各会话只列自己绑定的知识库；可读本库实际内容，跨库 reference、错库 documentId 及撤销后的原引用被拒绝，不泄露另一项目正文，另一会话仍可正常读取。

这两项使用真实 windowless RuntimeHost、磁盘 SQLite、LocalKnowledgeStore 与 native register/execute/持久 receipt 链。worker 是仅回显 Host 协议的 Node 子进程，不运行 Pi SDK 或模型；它们是 Host 工具接线证据，不能改称真实 Pi 对话或桌面隔离验收。尚需当前包内桌面的普通/项目会话、专家身份和知识引用展示核对。

历史 `root-native-isolation-user-copy-v2.log` 仍为 1 pass、2 fail：记忆夹具曾固定复用已退休 generation，知识夹具曾误读 bounded 结果形状。按实际会话映射和结果结构修正后，两项在 H6 新构建通过；旧日志不改写。资源层额外入口 `resource_gateway::tests::resource_gateway_uses_frozen_root_when_directory_path_is_omitted`、`resource_gateway::tests::resource_gateway_blocks_nested_links_outside_frozen_root` 在本文仍只记源码；`runtime_host::context_reset_tests::run_session_binding_refuses_other_conversations_and_existing_different_bindings` 已在 R3 通过。

M1 私聊禁止 reset、M2 开放后的联系人旧事件不串代：本候选未交付的联系人专属能力标为 **NA**。NA 不计 Pass，不从既有能力中删除功能来规避回归，也不把全部联系人功能反向设为普通运行时收敛的前置条件。普通会话/项目隔离仍必须验证。

## 当前隔离桌面的剩余项与实际范围

以下是尚未在本文登记通过的桌面证据，不否定上表已通过的真实 Host 或组件契约。当前包内 desktop 的构建、窗口与哈希状态以 root 的 `native-convergence-evidence-20261028.md` 最新候选段落为准；本文没有操作桌面或调用模型。

| 项目 | 最小现有流程 | 取证边界 |
| --- | --- | --- |
| U01/U02 | 实际会话连续追问→应用退出重开→追问原约束→重置；两会话中 A 待审批/取消、B 继续。 | 核对 Run/session/代次、JSONL、实际文件与界面；不把整个共享 sidecar 被终止描述成单 Run 故障隔离。 |
| U04/U07 | 隔离普通会话接已有 `fake-mcp-server.mjs valid` stdio echo；沿 `mcp_server_save/test/set_enabled`、`list_mcp_tools`、`call_mcp_tool`，先拒绝再新调用单次批准；核对实际批次的读→MCP→后读屏障。待审批退出重开后再决策，另核对撤销权限后的拒绝。 | 只测本地受控 MCP 和临时资源，不借用真实外部账号。模型未产出所需批次就不计批次覆盖。命令先看 Host `runtime_capabilities`：可用才测无害命令；不可用记录 sandbox_unavailable/未启动进程，不绕过后端。 |
| U08 | 沿真实模型会话触发实际 Pi 压缩，确认 compaction entry 后退出应用、重开追问摘要中的证据和近期约束。 | J5 仅手动 SDK compact/同进程 reopen；不能直接覆盖应用进程重启或自动阈值。 |
| U09 | 现有 Job 结果写入新的隔离项目文件→`present_files`→实际打开交付卡；primary Run 完成后两次打开通知列表，再重开应用核对同一 `run:{id}` 与已读状态。 | `app_notifications_list`→`sync_app_notifications` 为应用通知，和模型 Job notice 分开。`present_files` 只检查授权目录内实际文件并声明交付，不宣称内容核验通过。新任务不 seed 旧核验清单；只对已有冻结合同保留 `delivery_acceptance_state/resolve` 兼容。待用户公开 resume 仍为 Yuxi，不新建 Native 问答或 repair 框架。 |
| U10 | 当前两位专家/项目在实际 UI 中各读自己的记忆/知识引用，并核对普通会话归属和显示。 | 后端隔离已由 H6 通过；联系人尚未交付的 M1/M2 项继续 NA。 |
| U11/U12 | 用户副本升级/自动备份回退已有 root 精确通过；核对匹配旧二进制实际打开回退副本，以及同一最终包内 desktop/sidecar/SDK 和受影响桌面场景。 | 不让旧包直接打开新 schema；解包窗口不是实际 NSIS 安装。系统安装/提权或接触原用户库若超出现有授权，另行确认。 |

S5 仍需默认入口切换决策、同一最终构建完整证据和至少一个无回退发布周期；S6 必须在该周期之后处理未结旧 Kernel Run、删除替代实现并验证历史可读。当前隔离桌面通过不能代替这些阶段前提。

## 后续精确验证顺序

1. root 保持唯一 cargo 槽；本文没有启动测试。
2. U04 共文件链已由 H4 通过，U03 真实资源时线已由 H5 通过；仅在相关源码或未解问题需要时精确复跑，不无条件重改/重跑。
3. 可选 `FOX_TEST_NATIVE_SIDECAR_BIN` 编译 sidecar 入口已由 root 运行 H3/H4/H5；relay 记录 `sidecar-launch.json`，保留全部 Host 强断言。root 负责候选 executable/SDK/hash 记录。
4. U06 七窗口、U09 正常 Job/子运行、U10 Host 记忆/知识隔离已有上述增量通过；不按过期缺口重复安排。优先补 U04/U07 审批和未知工具、U01/U08 真实模型重开、U09 应用通知/实际交付及 U10 桌面归属。
5. root 已登记的用户副本升级回退、正式性能与 Windows 构建结果以其报告为准；同一最终包验收、默认切换和发布周期继续单独登记，不把当前局部证据当作 S5/S6 已完成。


## 2026-10-29 S5 最终候选增量

S5 默认入口切换、最终 Windows 候选、U01–U12 当前状态和缺口见 `migration-work/s5-default-pinative-20261029.md`；候选哈希见 `migration-work/s5-candidate-manifest-20261029.json`，离线性能见 `s5-performance-summary-20261029.json`。本文此前的历史日志和失败记录保持原样，不因新候选改写。
