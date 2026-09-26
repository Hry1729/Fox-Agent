# B 阶段交付清单（供协调者最终收口审查）

**编制**：B 阶段开发执行者　**日期**：2026-09-25　**卡性质**：只整理证据，**未编码、未构建、未重跑测试、未合并**

**一句话摘要**：`fbaf7ce..df9901c` 共 46 提交、24 文件（全在 `apps/desktop/src-tauri/src/`，无迁移/schema/前端），
9 项能力均有限定范围的证据；**两个真实生产缺陷已修复并复跑**；F3 历史 20 秒超时根因仍开放，
27 个 ignored、真实桌面/真实 Provider/安装包未验收；代码未进 main（`fee9d4e`），绿色集成仍 `fbaf7ce`。

## 0. 取数方式与运行者口径

- 本清单**只读** Git 历史与 `team-evidence/*.json|log` 现有记录；未运行任何测试，未访问生产数据库或凭据。
- **运行者由 runner JSON 的 `cwd` 区分**，不靠文件名猜：
  - `worktrees/b2b-review-20260924` → **协调者独立审查树**（本文写作"协调者"）
  - `worktrees/b2b-20260924` → **开发者树**（写作"开发者"）
- 测试计数取各日志顶级 `test result:` 行；**不同范围重叠，不相加**。
- 每条证据的 `sourceHead` 以 JSON 为准；**父提交上的测试不等于当前 HEAD 的测试**，下表逐条标注固定点。

## 1. 版本与范围

### 1.1 当前状态（实测）

| 项 | 值 |
| --- | --- |
| 工作树 | `D:\python\projects\Fox\worktrees\b2b-20260924` |
| 分支 / HEAD | `codex/b2b-20260924` / `df9901c38500fc8f030905336252e49e2a3dd50b` |
| 已验收集成基线 | `fbaf7ce32a5ce928b2712e4299ec6580da36143f`（`f1-r1-20260924` 树 tip 仍为该提交） |
| 主仓 | `D:/python/projects/Fox/Fox` = `fee9d4e0bb67a3b7378413c784c6d1877633bf10`（main） |
| 是否已入 main | `git merge-base --is-ancestor df9901c main` 退出码 **1** ⇒ **未并入** |
| 变更规模 | 46 提交，**24 文件，+3913 / −85**；全部位于 `apps/desktop/src-tauri/src/` |
| 未改动的边界 | **无新增迁移、无 schema 版本提升、无 `tauri.conf`/capability、无前端/协议/Node 改动** |

### 1.2 WIP（保留，未提交，未改动）

- `docs/f3-timeout-20260924/f3_round_timeout_evidence.md`：未跟踪，SHA-256 `DF2308C2741D2F5D7EF8EF232C8EEAB8DCCC9822174F733FDAD3085DD4668469`（本卡之前已更新 §19.7/§19.8/§20）。
- `apps/desktop/src-tauri/gen/schemas/desktop-schema.json`、`windows-schema.json`：**实测零内容差异**。
  `HEAD:path`、index、工作区三者 blob 同为 `328664596978cc7913c66cc10dbc6b9c0fb14f94`，`git diff` 输出为空；
  `git status` 的 ` M` 来自 stat/行尾（git 报 `LF will be replaced by CRLF`）。两文件在 HEAD 中本来就同内容。
  ⇒ **不存在待提交的 schema 语义改动**；按要求原状保留。

### 1.3 变更范围分类

**A. 生产修改**

| 文件 | 生产内容 |
| --- | --- |
| `runtime_host/waiting_jobs_wake.rs`（新增 170） | 持久 wake 的进程内信号槽 + watcher：原 deadline 收敛、精确冲突有限退避、shutdown 收束 |
| `runtime_host/kernel_host.rs` | park/终态后自动 wake 入口；OS 锁内 wake 决策；来源/身份/权限/取消优先级；恢复扫描；停止 |
| `runtime_host/kernel_coordinator.rs` | `reopen_cancelled_wait` 窄取消对账（不复活 scope）；`DecisionLease::WaitingWakePolicy/WaitingAccount` |
| `database/repositories/kernel.rs` | `kernel_commit_waiting_wake`（当前 policy version CAS + pending cancel 优先）、`kernel_commit_waiting_account` |
| `runtime_host/background_jobs.rs` | `cancel_unfinished_for_terminal`；**同 Run 范围守卫**；`on_terminal` 透传 |
| `runtime_host/kernel_gateway.rs` | `execute_context_resource_with_terminal_notice` 透传 |
| `runtime_host/attachment_compute/host_lifecycle.rs` | 终态 Job 与 notice 提交后回调 |
| `runtime_host/mod.rs` | wake 状态字段；run 状态判定纳入 wake in-flight；取消路径信号 |
| `runtime_host/attachment_compute.rs` | 共享 `code_validation_error`（复用 `data_compute::MAX_CODE_BYTES`），删除重复 `128*1024` 字面量 |
| `runtime_host/attachment_compute/jobs.rs` | 程序长度前置检查直接构造 `invalid_params`；分类收窄为 JS 产地标记优先 + 三条校验措辞 |

**B. 回归测试**

- 新增：`waiting_jobs_auto_tests.rs`(674)、`waiting_jobs_recovery_tests.rs`(566)、`waiting_jobs_boundary_tests.rs`(455)、`waiting_jobs_terminal_tests.rs`(185)、`run_lease_component_tests.rs`(154)
- 修改：`f1_engine_path_tests.rs`(297)、`rev_repro_tests.rs`(183)、`unified_tests.rs`(134)、`waiting_jobs_storage_tests.rs`(125)、`f3_approval_tests.rs`(46)、`tests.rs`(10, 注册)、`host_job_notice_tests.rs`(5)；`jobs.rs` 的 `mod tests` 内新增约 200 行
- 删除：`kernel_coordinator/run_lock_probe_tests.rs`（−260，`0e33322` 撤除临时探针）

**C. 保留诊断（全部 `#[cfg(test)]`，生产构建不含）**

| 诊断 | 位置 |
| --- | --- |
| `host_trace` 时间线（另由 `FOX_F1_TIMELINE` 开关，关闭时零工作） | `kernel_model_worker.rs`（`#[cfg(test)]` 模块） |
| live session spawn→ready 记录 | `live.rs`（`#[cfg(test)]` + host_trace） |
| `waiting_wake_test_hooks`（before_park_unlock / before_wake_account / continuation transport 归集） | `kernel_host.rs:768` |
| `disable_auto_wake_for_test` / `force_auto_wake_round_for_test` / `auto_wake_lock_conflicts_for_test`；`auto_wake_disabled` / `auto_wake_force_round` | `waiting_jobs_wake.rs`、`mod.rs` |
| `wake_kernel_waiting_run_forced_round_for_test` / `start_kernel_run_forced_round_for_test` | `kernel_host.rs` |
| `jobs::test_hooks::set_at_settle` | `attachment_compute/jobs.rs:313` |

各调用点在非 test 构建被逐条 `#[cfg(test)]` 屏蔽（例：`kernel_host.rs:1011/1016`）。

**D. 曾引入但现已撤除的错误改动**

| 引入 | 错误改动 | 撤除于 | 撤除内容 |
| --- | --- | --- | --- |
| `34693d1` | `drive_with_actions_transport` 把 dispatch `Result` 绑到 `_dispatched` 并以 `Ok(())` 结束分支——**绕过生产错误处理**（非 test-only 污染）；LocalProvider 断言持 events 锁 panic 致中毒 | `0ca4047` | 恢复 match Result 流入外层绑定；锁内取快照后再断言/打印；`printed` 仅在真打印时置位 |
| `87a26a4` | 诊断用例调用进程级 `take_hook`/`set_hook`；`spawn_started` 在非 test 构建执行 | `10af7a3` | 删除全局 panic hook 改动，保留 catch_unwind/锁中毒断言；spawn 计时收进 `cfg(test)` |
| `513eecb` | 临时锁探针 `run_lock_probe_tests.rs` + 全局 `lock_trace`；把手工持锁线程当"实际 watch"验证 | `0e33322` | 删除探针文件（−260），`kernel_run_lock.rs` 回到 `eda814b` 语义，改有界租约交接组件测试 |
| `0e33322` | 给无竞争的单测加 5 秒重试，放宽释放契约 | `4927a9e` | 恢复直接非阻塞 acquire 断言（+1/−17） |
| `b942046` | 终态用例把 `compute.invalid_params` 当预期现状 | `8804354` | 改为 `job.error_code == notice.error_code == compute.execution_failed`/`compute.cancelled`，新增 `job.error_code` 直接断言 |
| `8fb3e1f` | 删除宽泛 `contains("code ")` 后，非空超长 code 落入 `execution_failed` | `df9901c` | job 前置检查复用 `MAX_CODE_BYTES`，结构性构造 `invalid_params` |
| 文档 19.3/19.4 | 红例写成"提交 `2223740`"、绿例写成"提交 `8fb3e1f`" | 本卡前一轮文档修订 | 按 JSON 实际 `sourceHead` 更正，**原日志未回写** |
| 文档 19.6 | "processing 非法值经 job 路径不可达" | 本卡前一轮文档修订 | 更正为可达，并补真实入口矩阵 |

**保留下来的历史非绿（不抹去）**：`95ba9bc` 0/1、`ec49f8c` 3/1、`504dbcf` 3/1、`40197e0` 0/1、
`b942046` 终端 0/1、`f0f4666` 1/1、`b2b3-full-lib-8804354` 1292/2/27。

**净变更口径**：`fbaf7ce..HEAD` 中真正改动**生产行为**的提交只有
`8068ded`、`95150a2`（wake/停止/权限 CAS）、`ec49f8c`、`504dbcf`（均 RETURN）、`6543786`（采纳的取消对账）、
`7096f70`（同 Run 守卫）、`0ca4047`（恢复被 `34693d1` 破坏的 dispatch 错误传播）、`8fb3e1f`、`df9901c`（错误分类）。
其余多为测试/诊断；`kernel_run_lock.rs` 在整段范围的**净 diff 为零**（探针 `513eecb` 引入、`0e33322` 全部撤除），
⇒ 本范围**锁生产代码零净变更**。

## 2. 能力与证据对应表

> **运行者**：协调者=独立审查树 `b2b-review-20260924`；开发者=`b2b-20260924`。
> 「层级」指证据能达到的最强结论，不含真实桌面/真实 Provider/安装包。

| # | 能力 | 关键生产提交 | 红 / 退回 → 绿 | 固定点·日志·运行者 | 层级 |
| --- | --- | --- | --- | --- | --- |
| 1 | **自动通知 / 续答** | `8068ded`、`95150a2` | 红 `95ba9bc` 0/1（live 25 s 仍 waiting_jobs，父 fail-fast）→ 绿 `cd3ff1a` 2/0（live/强制逐轮 after-park + 真实 OS 锁竞争，测试未调 wake） | `b2b3-auto-red-95ba9bc`(cap150,57.44s,协调者)；`b2b3-auto-race-cd3ff1a`(cap180,39.23s,协调者)；回归 `b2b3-waiting-regression-cd3ff1a` 19/0、`b2b3-host-regression-cd3ff1a` 15/0（均协调者）；持久 Job 通知 `b2b3-terminal-8804354` 1/0（4 子模式，协调者） | **Authoritative 真实 RuntimeHost→Pi/Node→本地 HTTP 模拟 Provider + QuickJS**；4 份隔离 DB 只读核验 |
| 2 | **取消** | `ec49f8c`→`504dbcf`（**均 RETURN**）→ **`6543786`（采纳）** | 红 `32c47ce` 0/1（真实 cancel 后 Job 仍在结账边界，6 s 仍 waiting_jobs）→ 退回 `ec49f8c` 3/1、`504dbcf` 3/1（更早 `reopen` 无条件 register_run 拒绝已取消 scope）→ 绿 `6543786` auto 4/1（唯一失败是尚未到达队列断言的 fixture 错误）+ storage 17/0 | `b2b3-cancel-red-32c47ce`(cap150,33.21s)、`b2b3-auto-stop-ec49f8c`(71.38s)、`b2b3-auto-stop-504dbcf`(59.40s)、`b2b3-auto-stop-6543786`(65.38s)、`b2b3-waiting-regression-6543786` 17/0，均**协调者** | 真实 Host/DB/Node/Pi + 模拟 Provider；隔离 DB 显示 Job cancelled、attempts=1、formal=1、wake=0 |
| 3 | **原截止** | `ec49f8c`→`6543786` | `e23d39b` **实际 1/0（绿）**，仅文件名沿"红例"预设，**不能称已发现/修复 deadline 基本能力**；`fb758f8` 1/0（孤儿 running Job 保持 paused 至原 deadline，limited/unlimited） | `b2b3-deadline-red-e23d39b`(cap150,59.83s,协调者)、`b2b3-orphan-fb758f8`(cap150,52.77s,协调者)+durable proof | 受控持久 fixture + 真实 recover 入口；孤儿用例无 model transport |
| 4 | **恢复** | `8068ded`（恢复扫描 parked 后信号）、`6543786`（对账/注销） | `c4ad33a` 1/0（已提交 notice 跨重开 Host/传输只恢复一次）；`a3bf5d6` 1/0 + recovery 组 3/0（bound 未知 lease 不重放、不假 ack） | `b2b3-reopen-c4ad33a`(cap150,40.15s)、`b2b3-unknown-a3bf5d6`(cap120,30.39s)、`b2b3-recovery-group-a3bf5d6` 3/0，均**协调者** | DB/Host 重建（**非进程崩溃重启**）；unknown 层本地 loopback 零连接 |
| 5 | **去重（唯一且不重放）** | `8068ded`（唯一 continuation intent 的 CAS）、`6543786` | `cd3ff1a`：park/wake/tool.completed/continuation_response/run.completed 各 1；`c4ad33a`：唯一第三 Provider 请求、initial/park/wake/continuation/formal 各 1；`8804354`：typed notice 出现 1 次、`requests.len()==3`、attempts=1、正式工具结果不重复；`a3bf5d6`：无新 continuation | 同 #1/#4 日志（均协调者） | 同 #1 |
| 6 | **同 Run 范围守卫** | `7096f70`（`background_jobs.rs` 6 行） | 红 `f0f4666` 1 通过/1 失败（实验 Authoritative 跨 Run status/wait/result/cancel 均成功、兄弟 queued Job 被取消）→ 绿 `7096f70` 6/0 | `b2b3-scope-red-f0f4666`(cap120,30.97s)、`b2b3-scope-7096f70`(cap120,30.22s)，均**协调者** | 真实 DB + 真实 `background_jobs` 消费者；**无 Provider/桌面** |
| 7 | **权限切换** | `95150a2`（当前 policy version CAS） | `b942046` 1/0（live/逐轮）：真实 `change_execution_policy` 收紧 read_only，模型提议 `write_file` → `kernel.policy_denied` 且文件未落盘；两路径各 4 次本地 HTTP，park/wake 各 1 | `b2b3-policy-b942046`(cap150,14.89s,**协调者**) | Authoritative 真实 Host+Node/Pi+模拟 Provider |
| 8 | **停止** | `95150a2`（shutdown 准入） | `40197e0` 0/1（夹具 HTTP 非阻塞，未到 park）→ `348ac11` 夹具时序 → 绿 `b2bad60` 1/0（live/逐轮）：Job 已终态、auto wake 持 OS 锁+inflight 时 stop，父 Run cancelled、0 wake/0 continuation、仅原 2 次 HTTP 请求、slot/active/inflight/scope/OS 锁收束 | `b2b3-stop-40197e0`(33.48s)、`b2b3-stop-b2bad60`(cap150,25.69s)，均**协调者** | 真实 Host+Pi/Node+本地 HTTP；非真实桌面 |
| 9 | **错误分类** | `8fb3e1f`（JS 产地标记优先 + 参数措辞收窄）、`df9901c`（长度边界结构化） | 红 `bcf81dc` **0/1 固定提交**（超长 code → `execution_failed`，预期 `invalid_params`）→ 绿 `df9901c` | 协调者独立：`review-compute-jobs-8fb3e1f` 12/0（cap180）、`review-compute-params-df9901c` 17/0（cap180,120.60s）、`review-compute-terminal-df9901c` 1/0（18.89s）。开发者固定：`compute-overlong-red-bcf81dc` 0/1、`compute-overlong-green-df9901c` 1/0、`compute-jobs-module-df9901c` 17/0、`compute-terminal-df9901c` 1/0 | 真实 DB/会话/附件 + 真实 `execute_with_options`；**超长拒绝不运行 QuickJS**（零 progress + 源码提前返回）；恰好 `MAX_CODE_BYTES` 合法侧真实执行；终端用例为 Authoritative Host+Node/Pi+模拟 Provider |

**上一轮非固定提交证据（保留但不作固定 SHA 依据）**：`compute-red-01` 的 `sourceHead` 是 `4927a9e` ＋ `jobs.rs` 未提交改动；
`compute-green-01` 是 `2223740` ＋ 未提交改动；`compute-jobs-regression-01`/`compute-executor-regression-01`/`compute-terminal-green-01`
是 `2223740` ＋两文件未提交改动。其上轮唯一落在提交点的是 `compute-fix-full-01`（`8fb3e1f`，1301/0/27）。

**全量回归**

| 固定点 | 结果 | 运行者 | 日志 |
| --- | --- | --- | --- |
| `fbaf7ce` | 1276 / 0 / 27 ignored（87.00 s 测试） | **协调者** | `b2b2-full-lib-fbaf7ce` |
| `8804354` | 1292 / **2 failed** / 27 ignored ⇒ 整 B3 RETURN | **协调者** | `b2b3-full-lib-8804354` |
| `0e33322` | 1300 / 0 / 27 | 开发者（父提交口径） | `lease-fix-full-01` |
| `8fb3e1f` | 1301 / 0 / 27 | 开发者 | `compute-fix-full-01` |
| **`df9901c`** | **1306 / 0 / 27**（127.71 s 测试，cap240，默认并行） | **开发者**（协调者已核验 JSON/日志，**非独立全量**） | `compute-fix-full-02-df9901c` |

> `df9901c` 上的协调者独立证据只有两个**定向组**（jobs 17/0、terminal 1/0）。

## 3. 未关闭事项

### 3.1 已发现缺陷

1. **F3 历史 20 秒 Provider 超时根因：开放（唯一开放的生产缺陷问题）**。
   历史记录 1231/4/27、1292/2/27 中 `f3_round_*` 失败在定向复跑转 6/0（`b2b3-f3-round-retry-8804354`，11.67 s），
   但根因未定位；时间线取证曾受 `34693d1` 生产错误传播污染，`0ca4047` 修正后仍未复现原超时。
   **不得借 `df9901c` 的 1306/0/27 关闭此项。**
2. 本范围内发现的两个真实缺陷均已修复且经独立复跑覆盖：`34693d1` 绕过 dispatch 错误传播（`0ca4047` 修）、
   `8fb3e1f` 超长 code 误分类（`df9901c` 修）。除此之外，本次整理**未发现新的未修生产缺陷**。

### 3.2 覆盖缺口（不是缺陷）

- **27 个 ignored 未验收**（取自 `compute-fix-full-02-df9901c` 实测名单）：
  - 固定 OfficeCLI 二进制 **13 项**：`office::tests` 12 项 + `runtime_host::managed_files` 1 项
  - 外部工作簿/XLSX/遗留 Office fixture **4 项**：`data_compute::sparse_allocation_tests::real_agv_workbook_loads_in_compute`、
    `local_knowledge_import::tests::reads_legacy_office_acceptance_files`、
    `kernel_coordinator::tests::display_tests::projectless_real_xlsx_code_acceptance`、
    `runtime_host::attachment_compute::tests::snapshot_peak_memory_stays_bounded_with_production_entry`（内存测量，需 `--test-threads=1`）
  - 无原因说明 **2 项**：`harness_security_tests::a03_baseline_empty_content_is_currently_rejected`、`ex_baseline_nonzero_exit_currently_returns_string_err`
  - 操作员真实数据库 1 项（`database::migrations::tests::the_placement_migration_preserves_the_real_activity_database`）、
    遗留 fixture 显式生成 1 项、需 `FOX_TEST_CODEX_BINARY` 1 项
  - 真实云/凭据 3 项（`real_eval_tests` ×2、`yuxi::knowledge_search::tests::live_remote_knowledge_search_and_read`）、诊断探针 1 项（`tool_host::a0_replace_recovery_tests::a0_probe_records_the_real_replace_failure_shape`）
- **真实桌面 UI、真实（付费）Provider、安装包/发布布局：全部未验收**；所有 Provider 证据均为本地 HTTP 模拟。
- Node 全包最近固定在 `a81c802`（475/0/2 skipped），**未在 `fbaf7ce`/`df9901c` 重跑**（`services/agent-runtime`、`packages/crates/fox-engine-protocol` 在 `a81c802..fbaf7ce` 无 diff）。
- 全量只各跑一次，无重复压测；测试进程设 `SQLITE_DEFAULT_MEMSTATUS=0`，**不能推广为默认 MEMSTATUS=1**。

### 3.3 已接受的局限

1. **错误分类仍是措辞约定**：执行层产地标记 `JavaScript execution failed` 改词即失效（有回归测试兜底，无编译期保证）；
   其余参数分支匹配 `"processing must be"`/`"profile"`/`"档位的"`，校验层新增措辞会落到 `execution_failed`；
   I/O、附件缺失等非校验来源同样落 `execution_failed`。本卡只把真实可达的长度边界改为结构性判据，**未做全链类型化**。
2. `runtime_host/background_jobs.rs:57` 保留第三处 `v.len()>131072` 字面量（同步工具调用路径，job 执行器之前的重复防御），未与 `MAX_CODE_BYTES` 收敛。
3. 长度边界"未进入执行"的见证是**零 progress 事件 + 源码提前返回**，不是解释器启动计数（协调者已在 22 中指出不得推广为启动监测）。
4. 自动 wake 的进程内 watcher 在**跨进程持锁冲突**（ALREADY_OWNED / `job_wake_policy_changed` / `job_wake_cancel_pending`）时，
   墙钟重试上限是 `原 wait deadline + 5 s`（`waiting_jobs_wake.rs:106`），随后 break；一般错误与 `Ok(false)` 仍以原 deadline 收敛。
   该等待分支**未被单独观测触发**（`cd3ff1a` 只见证冲突计数 ≥1 后成功）。持久 CAS 未因此放宽，但"绝不越过原 deadline"在**墙钟**意义上对这三分支不严格成立，是否需要收口由协调者判断。

### 3.4 是否阻挡 B 收口（仅建议，最终由协调者判断）

- 建议**不因 F3 历史超时阻挡**：它是 F3 自身开放项，无证据显示由本范围改动引入，也不影响本文 9 项能力。
- 建议**不因 27 ignored / 真实 Provider / 桌面 / 安装包阻挡**：这些是贯穿 F1–B 的既有未验收范围，B 的结论应继续限定为"Authoritative 自动化证据"。
- **需协调者裁决**：`df9901c` 缺少协调者独立全量（只有两个定向组）。若收口口径要求"最终 SHA 有独立全量"，
  需再排一次独立全量；否则应在收口结论中明示 1306/0/27 属**开发者运行 + 协调者定向复跑**的组合证据。

## 4. 交付边界

1. **实验开关默认状态**
   - `experimental_compute_job_notice` **默认关**（仅 `FOX_EXPERIMENTAL_COMPUTE_JOB_NOTICE=1` 开启，`kernel_host.rs:1663`）。
     它门控：wire notice 模式、终态 Job 对账、**同 Run 范围守卫**（`Authoritative && flag` 才生效；`Legacy` 或 flag=false 保留会话兼容）。
   - **自动 WaitingJobs wake 本身不受该开关门控**（`signal_waiting_run` 无 flag 检查），但 wake 决策要求冻结绑定为
     `ExecutionAuthority::Authoritative`（`kernel_host.rs:1412–1415`），否则 fail-closed 并在原 deadline 收敛。
     ⇒ 对 Authoritative Run，这是**默认生效的行为变更**，请在收口结论中明确它是有意交付而非实验特性。
2. **迁移与兼容**：范围**无新增迁移、无 schema 版本提升、无配置/capability 变更**；
   `kernel_commit_waiting_wake/account` 只读既有表（`kernel_execution_policies`、`kernel_host_commands`）。
   ⇒ **回退到 `fbaf7ce` 不需要数据迁移**。已持久化的 `waiting_jobs` Run 在升级后由恢复路径自动纳入 wake（运行期行为变化，非 schema 变化）。
3. **部署状态**：代码只存在于隔离工作树 `codex/b2b-20260924`；`main` 仍为 `fee9d4e`，绿色集成仍 `fbaf7ce`，分支**未并入 main、未 push**。
   ⇒ **隔离测试通过 ≠ 已部署 ≠ 用户已获得修复**；真实桌面/真实 Provider/安装包均未验收。
4. **本卡边界**：未改代码、未构建、未重跑测试；未修改协调者维护的 04/21/22；
   未 push / 合 main / reset / stash / clean；未删除任何 WIP、历史日志或未跟踪证据；
   未启动 C，未提出并实施额外优化。
