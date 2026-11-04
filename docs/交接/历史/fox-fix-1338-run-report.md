# Fox 13.38 运行链路修复（第二段）报告

- 仓库：Hry1729/Fox-Agent
- 分支：feat/fox-fix-1338-run（已快进合入 main，远端分支已删除）
- **main 现在的 sha：`b9730626cae9356472696e1d01e3ee76c261edcf`**
- 合入时 main 没有前进（基于 dc4a0331），不需要 rebase，也没有强推
- 定向 CI：**windows-ci.yml run `41632461506`**，ref=feat/fox-fix-1338-run@b9730626，targeted=true，acp=true，全部通过
  - Agent Runtime tests ✅
  - Agent backend (ACP) tests（Windows）✅
  - Agent backend (ACP) tests（Linux）✅
  - Rust Host tests ✅（`ci_failure_summary.py`：No failing tests）
  - rust_test_filter：`released_terminal_tests cancelling_run_ host_tool_live_output provider_trouble command_jobs pi_source tool_host restores_runtime_diagnostics native_host_integration_tests external_backend`
  - 跳过（targeted 设计如此）：Desktop build + typecheck、ONNX、Zvec、Runtime package

## 提交（按顺序，都在 main 上）

| sha | 内容 |
|---|---|
| 687b95ae | 上一轮 WIP：A-2 实时输出、R1-2 拒绝访问兜底（这一轮编译、测试通过） |
| 1f0442bb | A-2 节流测试、终端占位修正；R1-3/B4-2 拒绝时同步写终态、释放时写 provider 失败、前端中文错误映射 |
| 41ab4249 | R1-4 后台任务终态说明，同一会话可只读查询；B3-4 Pi 默认用模型服务时写审计 |
| b9730626 | 取消期间只接受「已在运行」的工具卡输出（防止找不到卡时报投影错误） |

没有改 runtime 包（services/agent-runtime），CANCEL_SWEEP_GRACE 仍是 1.5s。UI 那一路（feat/fox-fix-1338-ui：B4-5、R2-4、.bat、代理、web_read）的文件没有碰。

---

## A-2：取消后工具卡和输出消失（Pi / dsh / Codex / Fox 原生）

**根因**
1. pi-acp、codex-acp 把命令输出放在 `_meta.terminal_output` / `_meta.terminal_output_delta`，`content` 里只有 `[terminal] <id>` 占位。投影层原来不读 `_meta`，所以数据库里只有 `status=in_progress`、没有内容。
2. Run 进入 `cancelling` 后，apply_runtime_event 会丢掉 `tool.updated` / `tool.completed`，取消宽限期里的输出也一起丢了。
3. Fox 原生 `run_command` 只在结束时写结果，取消时没有中间输出。
4. 前端在这一轮先结束时，没结束的工具卡没有定格状态，过程默认折叠，界面只剩「过程已取消」。

**改法**
- `projection.rs`
  - 累积 `terminal_output` 和 `terminal_output_delta`，只保留末尾 64KiB（超出时 `outputTruncated`），同时记下退出码 `terminal_exit.exit_code`。
  - 只带输出的更新按 250ms 节流；状态变化、结束、`flush_pending`、`finish` 时立即写出。
  - 本轮修正：`content` 里只有终端占位时，用真实输出作为卡片 `text`，同时写 `output`。占位文本不再当作快照覆盖输出。
  - 新增 `project_at(update, now)`，测试可以用自己的时钟。
- `external.rs`：发 cancel 之前先 flush 节流中的输出。
- `repositories.rs`
  - cancelling 期间接受 Runtime 工具卡的 `tool.updated` / `tool.completed`。本轮收紧为只接受这一轮已经在运行的卡；取消期间新出现的卡仍然丢弃，不报投影错误。
  - `run.cancelled` 把卡定格为 cancelled，`result_json` 原样保留。
  - 新增 `update_host_tool_live_output`。
- `tool_host` / `runtime_host`：前台 `run_command` 每秒把已产生的输出写进 Host 工具卡；取消时先写一次再结束进程树。
- 前端
  - `partialOutput` / `settled`：这一轮以 cancelled 结束时，未结束的卡显示「已取消」；以 failed / interrupted 结束时显示「已中断」。展开后能看到之前的输出，末尾带一行「[已取消，以上是取消前已经产生的输出]」。
  - `withSettledToolCalls` 用数据库里的工具卡补全事件流。
  - 取消的这一轮默认展开过程。
  - 摘要显示「N 项已取消」。

**测试**
- Rust（fox-agent-backend，本地 Linux 和 CI 都通过）：
  - `terminal_output_meta_accumulates_on_the_card_and_is_throttled`
  - `tool_output_keeps_only_the_tail_and_finish_flushes_pending_output`
- Rust（desktop，CI 通过）：
  - `cancelling_run_still_records_runtime_tool_output_and_keeps_it_on_cancel`：包括取消期间未知卡被丢弃。
  - `host_tool_live_output_only_touches_running_host_cards`
  - 已有的 `cancelling_run_ignores_late_non_terminal_runtime_events` 仍然通过。
- 前端：新增 `apps/desktop/tests/runtime-tool-settled.test.ts`，8 个用例，覆盖定格、partialOutput、withSettledToolCalls、「N 项已取消」。

## R1-2：dsh 取消后 pwsh 残留（TerminateProcess Win32 5）

**根因（推断，没有在真机上确认）**：srt-win 通过 Secondary Logon 以沙箱账户创建进程，所以这些进程不在 Fox 的 Job 里，Fox 也无权直接结束它们。

**改法**（上一轮写好，这一轮编译并测试通过）
- `acp.rs`
  - 启动时用 `verify_no_breakaway` 读回 Job 实际生效的限制：不允许 breakaway，必须有 KILL_ON_JOB_CLOSE，进程确实在 Job 里，否则不让后端运行。
  - 清扫时遇到拒绝访问，按父进程链兜底：结束 Job 外、本轮创建的祖先，再看目标进程是否随之结束。
  - 仍然结束不了的写审计 `agent_backend.cancel_sweep_access_denied`，内容包括 pid、映像名、父进程链和原因。这种情况不算清扫失败，也不会卡住取消。
- `proctree.rs`：新增 `outside_job_ancestors` 和 `is_access_denied`。

**测试**：`outside_job_ancestors_walk_up_to_the_job_member_and_skip_older_processes`（本地 Linux 和 CI 通过）。Windows ACP job 编译通过。

## R1-3 / B4-2：「Pi released its Run before a terminal result was recorded.」英文原文漏到界面

**根因**
1. `run.failed`（provider.request_failed）的投影被拒后，`reject_runtime_run_event` 在后台线程里写失败终态，和随后在读取线程里内联处理的 `run_released` 竞争。`run_released` 先把 Run 写成 `runtime.released_without_terminal`（interrupted），后台线程再写 failed 时被「late Host failure cannot replace terminal」挡掉。这就是数据库里两个错误码同时出现的原因。
2. 英文原文的泄漏路径：`FoxErrorPart` 把 `failure.message` 直接传给 ErrorState 的 `detail`。`userFacingError` 的 `INTERNAL_SURFACE` 只识别错误码、路径、环境变量等形态，不识别普通英文句子，所以这句英文原样显示。B5-3 的「web_read blocks private, …」也是同一个缺口。

**改法**
- `runtime_host/mod.rs`
  - `reject_runtime_run_event` 改为在调用线程里同步执行 `mark_run_failed` 并发出事件，只把 `cancel_run` 和 mark ready 放到后台。
  - 被拒的事件本身是 `run.failed` / `run.interrupted`、且投影错误是通用的 `storage.event_projection_failed` 时，终态沿用它自己的 code 和 message（`rejected_terminal_outcome`）。投影错误只写进日志。预算类错误码仍然以投影错误为准。
  - `run_released` 发现 Run 还没有终态时，如果这一轮有过 `run.retrying` / `run.retry.completed`，或者事件里带 `provider.*` 错误码（`run_had_provider_trouble`），就写成 failed、`provider.request_failed`、「模型服务连接中断，可重试。」；否则保持原来的 interrupted。
- 前端
  - `runFailureUserMessage`：按错误码映射中文提示，覆盖 provider.* / released_without_terminal / crashed / agent_backend_*。没有映射时，只有含中文且不像调试信息的原文才直接显示。
  - `FoxErrorPart`：普通模式只显示映射后的中文；错误码、英文原文和诊断只在开发者模式的「查看错误详情」里显示，开发者模式下总是带上。
  - `userFacingError` 新增 `isInternalRawText`：不含汉字、却有成句英文（3 个以上英文单词）时，普通模式改用 fallback。这样所有经过 ErrorState、toast、面板的路径都会被兜住。

**测试**
- Rust（CI 通过）：
  - `released_terminal_tests`：2 个用例。
  - `provider_trouble_is_detected_from_retry_and_provider_failure_events`
- 前端（run-failure.test.ts）
  - 新增 3 个用例：错误码映射、英文原文隐藏和开发者模式保留、FoxErrorPart 传参。
  - 更新了 1 个旧断言：诊断只在开发者模式显示。

## R1-4：模型称「这只是抽样结果」

**根因**：`status` 结果和「前几轮后台任务」上下文里只有 `state` 字段，没有明确说明这是最终状态。另外，后一轮查询前一轮的 job 会被 `[tool.permission_denied] command job is outside this Run` 挡住，模型拿不到最终状态的证据。

**改法**
- `command_jobs::public_status` 增加三个字段：
  - `final`
  - `stateLabel`：已取消 / 已结束（退出码 N）/ 已结束，失败（退出码 N）/ 运行中……
  - `note`：终态时写明「这是 Host 记录的最终状态，不是抽样：该后台任务已取消，不会再变化，进程已不在运行。可以据此下结论。」
- `previous_run_background_jobs`：每个 job 带上 `stateLabel`、`final`、`exitCode`（从结果或检查点读）。总说明改成「最终状态（不是抽样）」，并说明可以用 status / output 只读查询。
- 授权：同一会话里可以跨 Run 只读执行 `status` / `output`；`cancel` 仍然只能作用于本 Run 的任务；跨会话照旧拒绝（原测试仍然通过）。

**测试**（CI 通过）
- `r1_4_final_state_note_says_final_not_sampled`
- `r1_4_a_later_run_in_the_same_conversation_reads_but_cannot_cancel_earlier_jobs`

## B3-4：没有 `agent_backend.pi_source_defaulted_to_fox` 审计

**根因**：审计只在 `choice_required`（本机有 Pi 原生配置但还没选）时写。在全新数据目录、本机没有 Pi 配置的场景下，`choice_required=false`，走 Proceed 分支，没有任何记录，Pi 就默认用了 Fox 模型服务。

**改法**
- `login_source::SourceStatus` 新增 `fox_chosen`（是否明确选过模型服务）。
- `pi_source_step` 新增 `implicit_fox` 参数：同时满足「没有待确认的选择」「current=fox」「没有明确选过」「模型服务可用」时，走 DefaultToFox，也就是按项目记录 `choose_fox(remember=false)`，并写审计。
- 审计详情新增 `reason`：`no_source_chosen` 或 `choice_pending`。
- 每个项目私有 HOME 只写一次审计，之后 `fox_chosen` 为真。

**测试**
- `no_native_config_and_no_choice_reads_as_implicit_fox_until_chosen`（login_source，本地和 CI 通过）
- `pi_source_defaults_to_fox_only_when_never_chosen_and_fox_is_ready` 补了两条断言（CI 通过）

---

## 本地验证

- `cargo test -p fox-agent-backend --lib -- login_source projection proctree`：28 passed（Linux，单独的 target 目录）。
- tsc：整个项目跑一次要超过 115 秒（2 核，负载 6～8，被 UI 一路的 cargo、vite、chromium 抢占），会被超时杀掉。改为对本轮改动的 9 个文件及其依赖闭包做类型检查，结果只有基线里的 2 条 mermaid 报错（beautiful-mermaid-diagram.ts 349、350 行），没有新增错误。
- bun（apps/desktop/tests）：
  - 通过：run-failure、send-failure-notice、run-activity-presentation、runtime-process-groups、runtime-process-activity、runtime-tool-settled，以及 agent-backend-failure、conversation-run-indicator、kernel-process-groups、process-row-slots-css、process-scroll-*、r4-run-status-facts、run-phase。
  - 有失败、但不是本轮引入：runtime-process-row-dom 和 runtime-process-groups-dom。
    - 第一个用例在高负载下 5 秒超时，后面的用例跟着连锁失败。
    - row-dom 失败的 7 个用例单独跑全部通过。
    - groups-dom 的 2 个失败在 main 基线（dc4a0331）上同样失败。
- wincheck（桌面 crate 的 windows-gnu check）本地没有跑完。共享的 target 目录长时间被 UI 子代理的 cargo 锁住，而单次命令最多 120 秒，fox-desktop 自身的 check 跑不完。这一项由 CI 的 Windows Rust Host job 和 ACP job 实际编译、测试代替，均已通过。

## 未完成 / 需要注意

1. R1-2 的根因仍是推断。父进程链兜底能否结束沙箱账户下的 pwsh，取决于真机上这条链上有没有本轮创建、且 Fox 有权结束的祖先。结束不了时只写 `cancel_sweep_access_denied` 审计，进程可能仍然残留。如果实测仍有残留，下一步是通过 srt 自己的停止入口或 broker 清理，这需要改 runtime 或 srt 侧（单独开 feat/fox-runtime-1338 分支）。
2. A-2 依赖 pi-acp / codex-acp 实际发出 `_meta.terminal_output[_delta]`（按 Zed 约定实现）。如果某个后端只在 `content` 里给文本快照，也已经按快照处理；如果它什么都不发，界面仍然只有卡片、没有输出。
3. R1-4 只保证模型拿到明确的终态字段和说明，模型最后怎么措辞仍然取决于模型本身。
4. 整个项目的 tsc 和 Desktop build CI job 没有跑（targeted CI 会跳过它们）。建议下一次全量 CI 时一并确认。
5. 本地临时文件：`/workspace/work/f1338run-wincheck.sh`、`/workspace/work/tools/target-f1338`（Linux 测试用的 target 目录，可以删除）。

---

## 13.39 Windows 实测步骤与预期

前提：用 main@b9730626 打包；运行时包与 13.38 相同（0.1.8）。

1. **A-2 Pi**
   - 步骤：Pi 会话，询问档，执行 `ping -n 60 127.0.0.1`，批准后等 3～5 秒再点停止。
   - 预期：
     - 这一轮结束后过程区默认展开。
     - 「执行命令」卡片显示「· 已取消」，展开能看到取消前已经输出的「来自 127.0.0.1 的回复」若干行，末尾有「[已取消，以上是取消前已经产生的输出]」。
     - 摘要显示「1 项已取消」。
     - 数据库里该卡 `status=cancelled`，`result_json` 含 `text`、`output` 和 `streamedOutput:true`。
     - 运行期间每约 250ms 有一条 tool.updated 带输出。
2. **A-2 Codex**：同第 1 步，后端换成 Codex。预期相同（数据来自 `terminal_output_delta`）。
3. **A-2 dsh 与 R1-2**
   - 步骤：dsh 会话，后台计数循环 1..600，运行中点停止。
   - 预期：
     - 计数输出在卡片上可见。
     - 审计 `agent_backend.cancel_swept` 里有 `accessDenied.count`。
     - 如果 pwsh 仍被拒绝访问：多一条 `agent_backend.cancel_sweep_access_denied`，里面有 pid、parentChain、parentChainKilled，取消照常在 1～2 秒内完成，不会卡住。
     - 记录 pwsh 是否仍然存活（inFoxJob=False）。如果 parentChainKilled 非空，进程应该已经消失。
4. **A-2 Fox 原生**
   - 步骤：Fox 会话里 run_command `ping -n 60 127.0.0.1`，几秒后停止。
   - 预期：卡片「已取消」，展开能看到中文 ping 输出（每秒刷新一次）。
5. **R1-3 / B4-2**
   - 步骤：复现 13.38 R1-3（Pi 运行中，模型服务连接中断或重试）。也可以先让 Magpie 中途断开。
   - 预期：
     - 普通模式下失败卡片显示「运行失败 / 模型服务连接中断，可重试。」，界面上没有任何英文。
     - db `runs.error_code=provider.request_failed`，不再出现 `runtime.released_without_terminal`。
     - 打开开发者模式后，「查看错误详情」里显示 code 和英文原文。
6. **B5-3 顺带检查**：web_read 读 https://example.com 被拦截时，普通模式不显示「web_read blocks …」英文，显示中文兜底文案。
7. **R1-4**
   - 步骤：在 R1-2 或 R1-3 同一个会话里追问「上一轮那个后台任务现在什么状态？」。
   - 预期：
     - 回复明确说「已取消」或「已结束（退出码 N）」，不再说「抽样」。
     - 模型调用 run_command status / output 查询上一轮 jobId 时成功（只读），不出现 `[tool.permission_denied] outside this Run`。
     - 如果对上一轮的 job 调 cancel，仍然被拒。
8. **B3-4**
   - 步骤：全新数据目录，本机没有 ~/.pi 配置，新建 Magpie 并测试连接通过，只启用 Pi，发送「回复 ok」。
   - 预期：
     - 回复 ok。
     - 审计里出现 `agent_backend.pi_source_defaulted_to_fox`，`reason=no_source_chosen`，带 provider 和 model。
     - 同一项目再发一次，不会重复写这条审计。
