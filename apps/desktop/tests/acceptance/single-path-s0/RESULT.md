# Fox 单路径运行时 S0 探针报告

生成时间：2026-10-28　｜　探针目录：`services/agent-runtime/.probe-s0/`（未跟踪，`git status` 仅显示 `?? services/agent-runtime/.probe-s0/`）

## 0. 实验方法与环境

- 仓库：`D:/python/projects/Fox/Fox`，Windows，PowerShell 7，Node v24.15.0。
- 依赖解析：探针代码放在 `services/agent-runtime/.probe-s0/`，因此能直接解析 `services/agent-runtime/node_modules` 中的 `@earendil-works/pi-agent-core`、`pi-ai`、`pi-coding-agent`（均 1.4.0）。
- **不修改任何已跟踪文件、不修改 node_modules、不 fork Pi、不 commit**。`git status --porcelain` 输出只有 `?? services/agent-runtime/.probe-s0/` 一项（其余 `docs/` 条目是任务开始前就存在的改动）。
- **不调用真实模型**。使用的公开接口是 pi-ai 自带的 faux provider：
  - `createFauxCore({ api, provider, models })` → `core.streamSimple` 即 `StreamFn`（`pi-ai/dist/providers/faux.js:313`、`:398`）；
  - `fauxAssistantMessage(content, { stopReason })`、`fauxToolCall(name, args, { id })` 构造脚本化响应；
  - `core.setResponses([…])` 按请求顺序逐条下发。
  - pi-agent-core 的 `Agent` 通过公开选项 `AgentOptions.streamFn` 接入（`pi-agent-core/dist/agent.d.ts`，类型 `StreamFn` 见 `types.d.ts:22`）。
  - AgentSession 路径用公开工厂 `createAgentSession(options)`（`pi-coding-agent/dist/core/sdk.d.ts:116`）+ `ModelRuntime.registerProvider(providerId, { streamSimple })`，与 `services/agent-runtime/src/pi-adapter.mjs:60-83` 的接法一致。
- **每个会话一个独立 faux provider**（独立 `api` id + 独立响应队列），因为 `pi-ai` 的 faux 注册表是进程级全局状态；共享一份队列会让两个并发会话互相消费对方的脚本响应（P8 首版即因此出现"串话"伪象，已改为每会话独立 core）。
- 会话文件全部写到 `.probe-s0/tmp/`，不读写用户数据目录，未连接或停止任何正在运行的 Fox 进程。
- 已核对：`pi-agent-core` 中 `tool_execution_end` 事件、`beforeToolCall`/`afterToolCall` 钩子、`AgentEvent`、`AgentState` 都是公开导出；`transformMessages` 不在 pi-ai 根 index 上，但包的 `exports` 显式提供 `./api/*` 子路径，故经 `@earendil-works/pi-ai/api/transform-messages` 可达（标注为"经子路径导出，非顶层 API"）。

---

## 1. 总表

### 1.1 源码事实核对（任务给出的"已知源码事实"）

| 编号 | 结论 | 一句话依据 | 证据 |
|---|---|---|---|
| S1 | 是 | 批次中任一工具的 `executionMode === "sequential"` 即整批串行 | `pi-agent-core/dist/agent-loop.js:374-378` |
| S2 | 是 | 并行模式逐个 `await emit(tool_execution_start)` 并 `await prepareToolCall`（含 beforeToolCall）后才进 Promise.all | `agent-loop.js:417-470`，实测 P1/P2 时间线 |
| S3 | 是 | `beforeToolCall` 实参键恰为 `{assistantMessage, toolCall, args, context}` | `agent-loop.js:500-505`；实测 P2 输出 `["assistantMessage","toolCall","args","context"]` |
| S4 | 是 | 未知工具、参数校验失败在 beforeToolCall 之前返回 immediate 结果，不调用 execute / afterToolCall | `agent-loop.js:488-498`、`:539-545`；实测 P3：`afterToolCall ran for: ["call_read_A","call_write_D"]` |
| S5 | 是 | 准备阶段中止即 `break`，后续调用连 `tool_execution_start` 都没有 | `agent-loop.js:455-457`；实测 P4：第 3 个调用既无 start 也无 end |
| S6 | 是 | `SessionManager` 有 `appendMessage` / `branch(id)` / `branchWithSummary` / `getLeafId` | `pi-coding-agent/dist/core/session-manager.d.ts`（类声明）、`session-manager.js:827-837`、`:863` |
| S7 | 是 | Fox 正式 Pi 路径确实用 `SessionManager`（第 814/980/1019 行）与"按工具设 parallel/sequential"（`pi-kernel-loop.mjs:137`） | `services/agent-runtime/src/pi-runtime.mjs:814,980,1019`；`src/pi-kernel-loop.mjs:137`（`HOST_PARALLEL_READ_ONLY_TOOLS` = read/ls/find/grep/skill_load） |

**补充事实（任务未列出但影响判断）**：
- 并行模式下 `tool_execution_start` 在**准备阶段**就发出，`tool_execution_end` 对 immediate 结果也在准备阶段发出（`agent-loop.js:420-434`）。
- 工具结果消息（`message_start`/`message_end` role=toolResult）**整批结束后**才按源顺序发出（`agent-loop.js:459-465`）。
- 运行期中止后 `Promise.all` 不可中断（`agent-loop.js:459`），运行必须等所有工具 promise 结算。

### 1.2 A 部分

| 编号 | 结论 | 一句话依据 | 证据 |
|---|---|---|---|
| P1 | 是 | 假模型一次给 read A / grep B / write C / read C，Host 屏障成立：A、B 重叠；C 在两者结束后才开始；read C 在 C 结束后才开始；toolResult 顺序 = 源顺序 | `node .probe-s0/p1-mixed-batch.mjs`（`out-p1-mixed-batch.txt`） |
| P1b | 是 | 6 个 readonly 调用实测峰值并发 = 4 | 同上；区间重算 `peak=4` |
| P2 | 是 | 第一次 beforeToolCall 内整批登记（50ms await），不死锁、每批只登记 1 次、execute 不早于登记确认 | `node .probe-s0/p2-batch-registration.mjs` |
| P2b | 是（负面对照） | 若 beforeToolCall 改为等待整批"结算"，运行必然死锁，且 `abort()` 无法解除 | `node .probe-s0/p2b-settle-deadlock.mjs` |
| P3a | 是 | 未知工具、校验失败、被 block 三个位置**都**会发出 `tool_execution_start` 与 `tool_execution_end`（isError=true），事件法可靠 | `node .probe-s0/p3-missing-execute.mjs`（模式 a，屏障等待 70.2ms 后正常放行） |
| P3b | 有条件 | 登记时用同一工具表做"存在性 + schema"预校验可行，但**用 `Value.Check` 与 SDK 判定不等价**（SDK 先 `Value.Convert` 强制转换），会给出错误的"必失败"预测 | `p3` 模式 b；`pi-ai/dist/utils/validation.js:286` + `p3` 的 P3c 表 |
| P3c | 否 | `agent.state.pendingToolCalls` 与 toolResult 消息都不能作为"该位置已结束"的信号 | `agent.js:412-420`；P3c 时间戳（首个 tool_execution_end t+8.6ms vs 首个 toolResult t+82.3ms） |
| P4-1 | 否（后续调用没有结果） | 第 2 个调用内 abort：第 3 个调用**既无 toolResult 也无 start/end**；第 1、2 个都拿到 `Operation aborted` 且**都没有执行** | `node .probe-s0/p4-prep-abort.mjs`（`execute() actually ran for: []`） |
| P4-2 | 是 | JSONL 出现"assistant 有 3 个 toolCall，只有 2 个 toolResult"的条目 | `out-p4.txt`：`orphans=["call_p4_r3"] of ["call_p4_r1","call_p4_r2","call_p4_r3"]` |
| P4-3 | 有条件 | 会话层**不修复**：原始 transcript 原样保留孤立 toolCall；只有 provider 适配层的 `transformMessages` 补入合成结果（文本 `No result provided`，isError=true） | `pi-ai/dist/api/transform-messages.js:63`、`:138-166`、`:147-166`、`:210-211`；`out-p4.txt` 的 transformMessages 段 |
| P5-1 | 否（写不会"不进入 execute"） | write 的 `execute()` **一定会被调用**，因为并行批次所有工具在同一个 Promise.all 里同时启动；能保证的只是屏障不放行其效果 | `out-p5.txt`：三组实验均为 `write execute() was entered: true` |
| P5-2 | 是 | abort 信号能传到工具（`signal` 形参），工具可据此跳过效果；不检查则效果照写 | `agent-loop.js:574-580`；P5a `write effect performed after abort: true`，P5b `false` |
| P5-3 | 有条件 | 结果分类取决于工具自身：抛错/返回 isError → `isError=true`；忽略信号正常返回 → `isError=false`（被当成功） | P5c：`read result: isError=false text="read done"`（该 read 已 abort 但忽略信号） |
| P5-4 | 是 | 运行不会在 abort 时提前结算，必须等所有工具 promise 结算 | P5d：`工具忽略信号(800ms)，abort 后运行又等了 751.6ms` |
| P6 | 有条件 | **没有任何公开事件表示"该 toolResult 已落盘"**；`entry_appended` 不对 message 发出。落盘发生在 `message_end` 事件之后、同一同步栈内 | `agent-session.js:753`（先 emit）→ `:755-770`（再 appendMessage）；`session-manager.js:794-813` |
| P6-「何时可见」 | 有条件 | `session.subscribe` 监听器内同步读文件 = 不在盘；**仅一次 microtask 让出后** = 已在盘。`agent.subscribe`（在 AgentSession 自身监听器之后注册）内读 = 已在盘 | `out-p6.txt`：`disk@sync=false / disk@+1mt=true` 与 `[agent.subscribe] disk@sync=true` |
| P6-「entry id」 | 是 | 每个 toolResult 条目有独立 entry id（8 位 hex，`randomUUID().slice(0,8)`），可经 `message.toolCallId` 映射；落盘后 `getLeafId()` 恰等于该 entry id | `session-manager.js:21-29`、`:827-837`；`out-p6.txt` 中 `entryId === leafAfter` |
| P7a | 是 | `appendMessage` 补入同一 toolCallId 的 toolResult 后，上下文合法、无重复结果、provider 层不再补合成结果 | `node .probe-s0/p7-resume.mjs`（P7a 段全 PASS） |
| P7b | 是 | `branch()` 回到 assistant 之前后，孤立 toolCall 完全离开活动路径，provider 层无需合成结果 | `p7-resume.mjs`（P7b 段全 PASS） |
| P7c | 是（控制组） | 不做修复直接重开也能跑通：会话层不校验配对，孤立 toolCall 原样进入 provider 请求 | `p7-resume.mjs`（P7c 段） |
| P8 | 是 | 同进程两个 AgentSession：事件、abort、工具互不串话；中止 A 不影响 B | `node .probe-s0/p8-multi-session.mjs`（全 PASS） |
| M1（内存） | 事实 | 裸 node 44.5 MiB；加载 Pi 后约 119 MiB（+~75）；每会话增量 1.9~8.4 MiB；独立进程冷启动 import 753.7ms、建会话 46.7ms | `out-p8.txt` |

### 1.3 B 部分（源码盘点，详见 `PART-B-raw.md`）

| 编号 | 结论 | 一句话依据 | 证据 |
|---|---|---|---|
| Q1 | 事实 | `state.worker`/`WorkerHandle`/`active_run_id`/`legacy_available` 共 **29 处**（mod.rs 27 + kernel_host.rs 2）；桶计数：派发 1 / 请求响应 1 / 事件路由 1 / 取消 3 / 退出回调 2 / 启动停止状态机 14 / 其他 7。Yuxi、codex、deepseek_harness **都不经过**这个 worker | `PART-B-raw.md` §Q1；`mod.rs:747/795/4019`、`:4105-4121`；`commands/mod.rs:3944-3955`、`pi-kernel-worker.mjs:17,115-119` |
| Q2 | 事实 | `RunControlBinding` 生产构造点唯一（`run_control.rs:335-341`），持久化在 `run_control_bindings.binding_json`（JSON 文本）+ `binding_hash`；版本 +1 只在 `validate()` 处失败（`unsupported frozen control schema`），有 6 类路径；新增字段会被 `deny_unknown_fields` 拒绝（struct 及 3 个嵌套类型） | `PART-B-raw.md` §Q2；`control.rs:211,212,227`、`migrations.rs:6811-6819`、`run_control.rs:380-396` |
| Q3 | 有条件 | `skill_load` 唯一持久化确为 `INSERT OR IGNORE` 激活行，且读函数无生产调用方；同一 Run 内工具目录与系统提示词**不会改变**（worker 初始化时冻结，每请求只做严格 schema 投影并断言 tool 数组全等） | `PART-B-raw.md` §Q3；`skill_load.rs:282-330`、`skill_activations.rs:69`、`pi-kernel-worker.mjs:150,155-167`、`pi-kernel-loop.mjs:224-233,251-256` |
| Q4 | 事实 | 六模块合计 **5156** 行 / 4094 非空非注释；`live.rs` 循环控制段 632-1449 + 1490-1717 = 1046 行；外部调用点 37/1/1/14/1/9 | `PART-B-raw.md` §Q4；两个 JS 模块实际位于 `services/agent-runtime/src/` |

---

## 2. 每项详细记录

### 2.1 公共装置

- `lib/harness.mjs`：高精度时钟（`performance.now()`）、事件时间线、**Host 屏障调度器**（`HostScheduler`，readonly 上限 4，write 为排他屏障）、探针工具集（`read`/`ls`/`find`/`grep`/`write`，**全部 `executionMode:"parallel"`**）、faux 脚本模型封装、`makeAgent`。
- `lib/session.mjs`：为每个探针构造**真实 `AgentSession`**（`createAgentSession` + 独立 `ModelRuntime` + `SessionManager.create(cwd, sessionDir)` + `SettingsManager` + `DefaultResourceLoader` + 每会话独立 faux provider）。工具以 `customTools` 注册，并用 `tools: [名字…]` 白名单激活。
- 调度器语义（`lib/harness.mjs` 的 `HostScheduler`）：readonly 类工具取许可（≤4）；write 进入排他段——先等所有在飞的 readonly 结算，期间后续调用一律等待，结算后放行。工具效果的先后完全由这个调度器决定，SDK 侧只被要求"全部 parallel"。

> 注意（观测而非推断）：并行批次里所有工具的 `execute()` 都在同一个 `Promise.all` 里被同时调用，所以"屏障"发生在 `execute()` **内部**，而不是"阻止进入 execute"。

### 2.2 P1 混合批次屏障

- 代码：`.probe-s0/p1-mixed-batch.mjs`（场景 a 混批，场景 b 六路 readonly）
- 复现：`node services/agent-runtime/.probe-s0/p1-mixed-batch.mjs`
- 关键输出（`out-p1-mixed-batch.txt`）：

```
-- tool execution windows (ms since process start)
  read   call_read_A      arrived=    7.379 start=    7.457 end=   58.592
  grep   call_grep_B      arrived=    7.414 start=    7.585 end=   58.948
  write  call_write_C     arrived=    7.418 start=   58.999 end=   60.912
  read   call_read_C2     arrived=    7.449 start=   60.927 end=  104.438

-- SDK tool-result message order (message_end, role=toolResult)
  ["call_read_A","call_grep_B","call_write_C","call_read_C2"]

PASS  A and B overlap in time  (A=[7.457,58.592] B=[7.585,58.948])
PASS  write C starts only after A and B both ended  (C.start=58.999 max(A,B).end=58.948)
PASS  read C starts only after write C ended  (D.start=60.927 C.end=60.912)
PASS  toolResult message order equals source order
```

- 六路 readonly 峰值并发（区间重算，独立于调度器自报）：

```
  read   call_r1      start=  106.501 end=  167.848
  read   call_r2      start=  106.537 end=  167.963
  read   call_r3      start=  106.546 end=  168.011
  read   call_r4      start=  106.553 end=  168.038
  read   call_r5      start=  167.872 end=  230.141
  read   call_r6      start=  167.969 end=  230.22
PASS  peak concurrent read-only calls <= 4  (peak=4 (scheduler.peak=4))
```

- 结论：**是**。A、B 时间重叠；C 在 A、B 都结束之后才开始；read C 在 C 结束后才开始；toolResult 源序；峰值并发 4。

### 2.3 P2 整批登记

- 代码：`.probe-s0/p2-batch-registration.mjs`（正例）＋ `.probe-s0/p2b-settle-deadlock.mjs`（负面对照）
- 复现：`node .../p2-batch-registration.mjs`；`node .../p2b-settle-deadlock.mjs`
- 关键输出：

```
  t+    61.28ms  ledger   batch registered (3 calls) ids=["call_b1_r1","call_b1_r2","call_b1_r3"]
  t+  153.624ms  ledger   batch registered (2 calls) ids=["call_b2_r1","call_b2_r2"]

  t+     6.981ms  call_b1_r1   batch_already_known=false
  t+    61.448ms  call_b1_r2   batch_already_known=true
  t+    61.485ms  call_b1_r3   batch_already_known=true
  t+    93.419ms  call_b2_r1   batch_already_known=false
  t+   153.766ms  call_b2_r2   batch_already_known=true

  beforeToolCall argument keys: ["assistantMessage","toolCall","args","context"]
PASS  beforeToolCall receives exactly { assistantMessage, toolCall, args, context }
PASS  no deadlock: both batches and the final turn completed
PASS  one registration per assistant batch (3+2 calls -> 2 registrations)  ([3,2])
PASS  beforeToolCall ran for every toolCall (3+2)  (entries=5)
PASS  re-entry within a batch never registers a second time  (batch1=[false,true,true] batch2=[false,true])
PASS  no tool execute starts before its batch registration is acknowledged  (batch1 firstStart=61.692 ack=61.182)
registration latency sits on the batch critical path: first tool ran 54.7ms after registration began (ack=50ms)
```

- 结论：**是**（不死锁、幂等、execute 不早于确认）。附带两个事实：
  1. 登记耗时在批次**关键路径**上——第一次 `await` 期间没有任何工具开始；
  2. 并行模式下准备阶段是**串行**的，第 2、3 个 `beforeToolCall` 要等第 1 个返回后才被调用（`t+61.448` 晚于登记完成 `t+61.28`），所以"后续 beforeToolCall 只等待登记完成"是自然成立的，代价是登记延迟无法被并行摊薄。
- 负面对照（`out-p2b-settle-deadlock.txt`）：

```
  outcome after 1500ms: hung
  tool execute() calls that ever ran: 0 of 2 registered toolCalls
  after agent.abort() + 300ms: tool execute() calls that ran = 0
  agent.signal.aborted = true
PASS  awaiting batch settlement inside beforeToolCall deadlocks the run, and abort() does not break it
```

  即：在 `beforeToolCall` 里等"整批结算"是**不可恢复的死锁**——准备阶段无法推进，工具永远不会执行，`abort()` 也无法解开（signal 已 aborted，但钩子内部 await 的是只由工具结算才能满足的 Promise）。

### 2.4 P3 没有 execute 的位置

- 代码：`.probe-s0/p3-missing-execute.mjs`（模式 a / 模式 b / P3c 参数校验现实检查）
- 复现：`node .../p3-missing-execute.mjs`
- 批次（源序）：`read A` / `zzz_unknown_tool` / `read {}`（缺必填参数）/ `read B`（被 beforeToolCall block）/ `write D`（屏障，等前四位结算）
- 关键输出（模式 a）：

```
  call_read_A     start=t+ 5.255  end=t+ 80.113  isError=false
  call_unknown    start=t+ 8.555  end=t+ 8.624   isError=true
  call_badargs    start=t+ 8.650  end=t+ 9.259   isError=true
  call_blocked    start=t+ 9.268  end=t+ 9.339   isError=true
  call_write_D    start=t+ 9.343  end=t+ 82.248  isError=false

-- execute() called for: ["call_read_A","call_write_D"]
-- toolResult emitted for: ["call_read_A","call_unknown","call_badargs","call_blocked","call_write_D"]
-- write barrier: {"released":true,"waited":70.194}

YES  every no-execute position still emits tool_execution_start AND tool_execution_end (isError=true)
YES  none of the three reached execute()
  afterToolCall ran for: ["call_read_A","call_write_D"]
YES  afterToolCall is skipped for every no-execute position
```

- 三种做法的事实比较：
  - **(a) 监听 `tool_execution_end`：可靠**。三个无 execute 位置都有 start/end，`isError=true`，并且**在准备阶段就发出**（t+8.6/9.3/9.3ms，远早于屏障放行）。屏障等待 70.2ms 后正常放行，无死锁。
  - **(b) 登记时用同一工具表做存在性 + schema 预校验：有条件**。存在性判断与 SDK 一致；schema 判断**不一致**——SDK 在 `validateToolArguments` 里先 `Value.Convert(parameters, args)` 强制转换再校验（`pi-ai/dist/utils/validation.js:280-311`，`:286`），而 Host 常用的 `Value.Check` 不做转换。P3c 实测：

```
  missing required property          Value.Check=false -> REJECTED by SDK
  number where string is required    Value.Check=false -> EXECUTED
  null where string is required      Value.Check=false -> EXECUTED
  array where string is required     Value.Check=false -> REJECTED by SDK
  extra unknown property             Value.Check=true  -> EXECUTED
```

    即 `{path:123}` / `{path:null}` 会被 SDK 转换成字符串后**真的执行**。用 `Value.Check` 预测"必失败"会把这位置提前标记为已结算，而它其实还在跑——屏障可能提前放行。
  - **(c) 其他公开途径：不可用**。`agent.state.pendingToolCalls` 只是 `tool_execution_start/end` 的派生集合（`agent.js:412-420`），无法区分"已结算"与"从未进入批次"；toolResult 消息在整批 `Promise.all` 之后才发出（首个 end t+8.6ms vs 首个 toolResult t+82.3ms），作为屏障释放信号太晚。
  - 补充可用信号（源码层面，未单独实验）：immediate 结果的 `tool_execution_end` 不带 `durationMs` 字段（`agent-loop.js:652-661` 只在 `durationMs !== undefined` 时附带），可与"真执行过"区分。

### 2.5 P4 准备阶段中止

- 代码：`.probe-s0/p4-prep-abort.mjs`（3 个 read 一批，在第 2 个 `beforeToolCall` 里 `agent.abort()`；用真实 `SessionManager.create` 落盘）
- 复现：`node .../p4-prep-abort.mjs`
- 关键事件：

```
  tool_execution_start call_p4_r1  t+7.728
  tool_execution_start call_p4_r2  t+8.767
  tool_execution_end   call_p4_r2  t+9.010   <-- 完成序，与源序相反
  tool_execution_end   call_p4_r1  t+9.069
  message_end(toolResult call_p4_r1) t+9.115
  message_end(toolResult call_p4_r2) t+9.559
  turn_end  t+9.910
  message_start/end (assistant, stopReason=aborted) t+10.176/10.182
  agent_end t+10.448

-- execute() actually ran for: []
  tool_execution_start: ["call_p4_r1","call_p4_r2"]
  tool_execution_end:   ["call_p4_r2","call_p4_r1"]
  toolResult messages:  ["call_p4_r1","call_p4_r2"]
```

- (1) **后续调用有没有 toolResult：没有**。第 3 个调用完全没有 start、没有 end、没有 toolResult；第 1、2 个都拿到 `Operation aborted` 的 error 结果，且**都没有真正执行**（`execute() actually ran for: []`）——对应 `agent-loop.js:440-449` 的 deferred thunk 在 `signal.aborted` 时直接返回错误结果。
- (2) **JSONL 里是否出现"有 toolCall 没有 toolResult"**：是。

```
  message 65d57a39 parent=849a6b0c role=assistant stopReason=toolUse toolCalls=["call_p4_r1","call_p4_r2","call_p4_r3"]
  message e5420116 parent=65d57a39 role=toolResult toolCallId=call_p4_r1
  message dec81ba8 parent=e5420116 role=toolResult toolCallId=call_p4_r2
  message 6790aa10 parent=dec81ba8 role=assistant stopReason=aborted toolCalls=[]
PASS  JSONL has an assistant message with orphaned toolCalls  (orphans=["call_p4_r3"])
```

- (3) **再发一轮 prompt 时孤立 toolCall 的状态**：分两层，答案不同。
  - 会话/Agent 层（假模型实际收到的 messages）**原样保留**：`[...assistant(toolUse: r1,r2,r3)…]`，只有 r1、r2 的结果；孤立 r3 没有被补结果、也没有被丢弃。
  - provider 适配层才修复：`transformMessages` 对孤立 toolCall 补合成结果 `No result provided`（isError=true），并**整体丢弃** `stopReason` 为 `error`/`aborted` 的 assistant 消息。
  - 实现位置：`pi-ai/dist/api/transform-messages.js:63`（函数）、`:138-166`（`closePendingToolCalls` 插入合成结果，文本在 `:155`）、`:167-213`（两遍扫描）、`:178-180`（跳过 aborted/error 的 assistant）、`:210-211`（收尾补齐）。
  - 实测（P4(3b)，真正的新 prompt）：
```
  raw: {"role":"assistant","stopReason":"toolUse","toolCalls":["call_p4_r1","call_p4_r2","call_p4_r3"]}
  raw: {"role":"toolResult","toolCallId":"call_p4_r1","isError":true,"text":"Operation aborted"}
  raw: {"role":"toolResult","toolCallId":"call_p4_r2","isError":true,"text":"Operation aborted"}
  raw: {"role":"assistant","stopReason":"aborted",...}
  raw: {"role":"user","content":"go again"}
  after transformMessages:
   assistant(toolUse:[r1,r2,r3]) / tr(r1) / tr(r2) / tr(r3, "No result provided") / user
PASS  the aborted assistant message is dropped by the provider transform
PASS  exactly one synthetic result is inserted for the orphan
```
- 另一个观测：中止后循环**仍会再发一次** provider 请求（`message_start` assistant aborted，t+10.176），该请求因 signal 已 aborted 立即返回 aborted —— 一次"空转"请求。
### 2.6 P5 执行中取消

- 代码：`.probe-s0/p5-exec-cancel.mjs`（三组：a=write 不查信号；b=write 查信号；c=read 忽略信号；d=中止后结算延迟）
- 复现：`node .../p5-exec-cancel.mjs`
- 关键输出：

```
P5a  writeChecksSignal=false, readHonoursSignal=true
  abort called at t+111.751ms
  execute() entered for: ["call_slow_read","call_write"]
  write EFFECTS performed: ["call_write"]
  read  result: isError=true text="Operation aborted"
  write result: isError=false text="wrote"
  abort -> run settled: 0.8ms

P5b  writeChecksSignal=true
  write EFFECTS performed: []
  write result: isError=true text="skipped: run aborted"
  abort -> run settled: 0.4ms

P5c  readHonoursSignal=false（read 忽略信号）
  read  result: isError=false text="read done"        <-- 已 abort，但结果被当成成功
  abort -> run settled: 502.6ms (total run 610.2ms)

P5d  工具忽略信号(800ms)；abort 后运行又等了 751.6ms
```

- 判定：
  - **write 是否完全不会进入 execute：否**。三组都是 `write execute() was entered: true`——并行批次里所有工具同时在 `Promise.all` 中启动，SDK 不提供"阻止进入 execute"的手段；能控制的只有 execute 内部是否产生效果。
  - **signal 能否传到工具：能**。`execute(toolCallId, args, signal, onUpdate)` 的第 3 个参数就是本次运行的 abort signal（`agent-loop.js:574-580`），实测工具内可读到 `signal.aborted`。
  - **结果是 isError 还是成功**：取决于工具自身。工具抛错 → `isError=true`；工具忽略信号正常返回 → `isError=false`，**被当成成功**（P5c 的 `read done`）——SDK 不会在工具返回后再覆盖判定。
  - **中止不缩短运行**：运行必须等所有工具 promise 结算（`agent-loop.js:459` 的 `Promise.all`），P5d 显示 abort 后又等了 751.6ms。

### 2.7 P6 持久化时机

- 代码：`.probe-s0/p6-persistence.mjs`（真实 `createAgentSession` + `SessionManager.create(临时目录)` 跑 P1 的混合批次）
- 复现：`node .../p6-persistence.mjs`
- 首先确认工具真的执行了（否则持久化时序没有意义）：
```
  read   call_read_A      start=   261.37 end=  309.009
  grep   call_grep_B      start=  261.426 end=  310.389
  write  call_write_C     start=  310.447 end=  311.603
  read   call_read_C2     start=  312.308 end=  356.846
PASS  all four tools really executed
PASS  write C started after read A and grep B both ended
PASS  read C2 started after write C ended
```
- 落盘时序（`disk@sync` = 事件回调里**同步**读文件；`disk@+1mt` = 只让出一次 microtask 后再读）：

```
   t(ms)      source   type                  toolCallId        disk@sync  disk@+1mt  entryId   leafAfter
    359.157  session  message_end           call_read_A       false      true       04813a35  04813a35
    359.408  agent    message_end           call_read_A       true       true       04813a35  04813a35
    360.597  session  message_end           call_grep_B       false      true       a8c126d6  a8c126d6
    ...
    371.972  session  agent_end                               false      false

PASS  [session.subscribe] at message_end the toolResult is NOT yet on disk
PASS  [session.subscribe] after one microtask hop it IS on disk
PASS  [agent.subscribe] at message_end it is ALREADY on disk
PASS  the entry id on disk equals SessionManager.getLeafId() right after that hop
PASS  each toolResult entry carries an id that maps to its toolCallId
  entry_appended events observed: 0
```

- 回答两个问题：
  1. **有没有一个公开信号能可靠表示"该 toolResult 已经落盘"：没有。** `entry_appended` 对 message 不发出（源码：`agent-session.js:753` 先 `_emit` 给公开监听器，`:755-770` 才 `appendMessage`；`entry_appended` 只在 custom entry / context edit / cache-warm 处发出，见 `:186`、`:476`、`:639`、`:868`、`:2787`）。落盘本身是同步的 `appendFileSync`（`session-manager.js:794-813`，**无 fsync**）。可用的只是"时序事实"：
     - 在 `session.subscribe`（AgentSessionEvent）回调里：此刻**还没落盘**，让出一次 microtask 后**已落盘**；
     - 在 `agent.subscribe`（raw Agent，且在 AgentSession 自身监听器之后注册）回调里：此刻**已落盘**（AgentSession 的监听器先跑完整段持久化）。
     补充：`AgentSession.subscribe` 只是把监听器推入同步数组（`agent-session.js:990-998`），**不 await**；而 `Agent.processEvents` 会 `await` 每个 Agent 监听器（`agent.js:436-438`）。
  2. **每个 toolResult 条目是否带有可映射到 toolCallId 的 entry id：有。** entry id = `randomUUID().slice(0,8)`，会话内查重（`session-manager.js:21-29`）；条目结构是 `{type:"message", id, parentId, timestamp, message:{role:"toolResult", toolCallId, …}}`（`session-manager.js:827-837`）。落盘后 `getLeafId()` 恰好等于该条目 id（实测 `entryId === leafAfter`）。
- 最终 JSONL（`tmp/p6/sessions/*.jsonl`）：
```
  header {"version":3, "cwd": "...\\.probe-s0\\tmp\\p6"}
  model_change / thinking_level_change / message/system / message/user
  message 0a172908 parent=2c3aa8b2 assistant
  message 04813a35 parent=0a172908 toolResult toolCallId=call_read_A
  message a8c126d6 parent=04813a35 toolResult toolCallId=call_grep_B
  message 81b2f850 parent=a8c126d6 toolResult toolCallId=call_write_C
  message c76d43a4 parent=81b2f850 toolResult toolCallId=call_read_C2
  message 5fd48d54 parent=c76d43a4 assistant
```

### 2.8 P7 结果补交恢复

- 产物生成：`.probe-s0/p7-artifact.mjs` — 真实会话，raw Agent 监听器里在 assistant 消息落盘之后立刻 `process.exit(0)`，因此 JSONL 留下"assistant 有 2 个 toolCall、0 个 toolResult"（不是手写 JSONL）：
```
  message fc409486 parent=f1e1e3c5 system
  message ff7f34f8 parent=fc409486 user
  message 8f4454aa parent=ff7f34f8 assistant calls=call_orphan_1,call_orphan_2
```
- 复现：`node .../p7-artifact.mjs` 然后 `node .../p7-resume.mjs`（每个子实验各自复制一份会话文件，避免互相污染）
- P7c 控制组（不修复直接重开）：
```
  SDK-built context:
   {"role":"user","text":"go"}
   {"role":"assistant","stopReason":"toolUse","toolCalls":["call_orphan_1","call_orphan_2"]}
PASS  (c) the raw transcript still contains the orphaned toolCalls
PASS  (c) the provider layer repairs them with synthetic results   (2 条 "No result provided")
```
  → 会话层不校验配对，孤立 toolCall 原样进入请求。
- P7a（`appendMessage` 补交）：
```
  appendMessage(toolResult call_orphan_1) -> entry 4041533a
  appendMessage(toolResult call_orphan_2) -> entry 6f67cd91
  修后上下文: user(go) / assistant(toolUse:[1,2]) / tr(1) / tr(2)
  下一请求:    user(go) / assistant(toolUse) / tr(1) / tr(2) / user(carry on) / assistant(stop)
PASS  (a) no orphaned toolCalls remain right after the repair
PASS  (a) no duplicate toolResult per toolCallId  ({"call_orphan_1":1,"call_orphan_2":1})
PASS  (a) the provider layer adds no synthetic result
PASS  (a) the repaired results are positioned before the new user turn
```
- P7b（`branch()` 回到 assistant 之前）：
```
  orphaned assistant entry: 8f4454aa (parent ff7f34f8)
  leaf moved to ff7f34f8
  下一请求: user(go) / user(carry on) / assistant(stop)
PASS  (b) the orphaned assistant message is gone from the active path
PASS  (b) no toolResult at all for the abandoned toolCalls
PASS  (b) the file stays append-only: the abandoned entry is still in it  (entries=7)
PASS  (b) the provider layer needs no synthetic result
```
- 事实比较：
  - 两种方式重开后上下文都**合法**（无孤立 toolCall 到达 provider）。
  - 假模型收到的消息**不一致**：(a) 能看到 `assistant(toolUse)` + 两条 Host 造的 error 结果；(b) 完全看不到那次调用，只看到两轮 user。
  - **无重复结果**（(a) 每个 toolCallId 恰好 1 条）。
  - (a) 需要 Host 伪造结果（内容为"outcome unknown / 未执行"），因此结果文本必须由 Host 负责其语义；(b) 不需要伪造，但丢弃了 assistant 的整轮意图。(b) 依赖 `branch` 只移动叶子指针——旧条目仍在文件里（append-only，实测 entries=7）。
  - 若不修复（P7c），运行**不会报错**，provider 层会静默补 `No result provided`。

### 2.9 P8 单进程多会话

- 代码：`.probe-s0/p8-multi-session.mjs`，冷启动：`.probe-s0/lib/cold-start.mjs`
- 复现：`node .../p8-multi-session.mjs`
- 关键输出：
```
  RSS at script start (SDK already imported) : 118.1 MiB
  RSS after session A                        : 120 MiB  (delta 1.9)
  RSS after session B                        : 128.4 MiB  (delta 8.4)

  tool executions: [{"tag":"a","id":"a-call-1","name":"a_read"},{"tag":"b","id":"b-call-1","name":"b_read"}]
  signal state when each tool returned: [{"tag":"b","aborted":false},{"tag":"a","aborted":true}]
  A tool events: ["a-call-1","a-call-1"]   B tool events: ["b-call-1","b-call-1"]
  A messages: ["user","assistant:toolUse","toolResult:a-call-1","assistant:error"]
  B messages: ["user","assistant:toolUse","toolResult:b-call-1","assistant:stop"]
  A terminal assistant: stopReason=error errorMessage="This operation was aborted"

PASS  each session only ever saw its own toolCallIds
PASS  each session executed only its own tool
PASS  aborting A did not abort B
PASS  A saw the abort signal, B did not
PASS  different cwd and different session file per session
```
- 内存/冷启动：
```
  RSS after both runs                        : 132.4 MiB
  RSS of a bare `node -e 0` process          : 44.5 MiB
  separate process: import Pi + 1 session    : {"rssAtStartMiB":44.6,"importMs":753.7,"rssAfterImportMiB":119,"sessionMs":46.7,"rssAfterSessionMiB":121.6,"rssDeltaPerSessionMiB":2.5}
```
  → 裸进程 44.5 MiB；加载 Pi 后约 119 MiB（**+约 75 MiB**）；每增加一个会话 +1.9~8.4 MiB（GC 噪声使区间偏宽，冷启动进程量到 +2.5 MiB）；import 753.7ms，建会话 46.7ms。
- 观测：A 的终止消息为 `stopReason=error, errorMessage="This operation was aborted"`（工具忽略信号、返回后循环继续，最终以异常收尾），而 B 正常以 `stop` 结束——中止只作用于 A。
---

## 3. 与预期不符

以下各条与"已知源码事实"的字面表述不冲突（那些条目都已核对为真），但与由它们容易推出的直觉相反，或界定了它们的适用范围。

| # | 发现 | 证据 |
|---|---|---|
| 1 | **"没有 execute"≠"没有事件"**。未知工具、参数校验失败、被 block 这三种位置**都会**发出 `tool_execution_start` 和 `tool_execution_end`（isError=true，且在准备阶段就发出），但 `afterToolCall` 不会调用。 | `agent-loop.js:420-434`（immediate 分支也 emit end）、`:487-546`；实测 P3：`tool_execution_end` 三条齐全，`afterToolCall ran for: ["call_read_A","call_write_D"]` |
| 2 | **参数校验会先做类型强制转换**。"参数校验失败"只覆盖**无法转换**的情形；`123` / `null` 这类会被 `Value.Convert` 转成字符串后**照常执行**。因此"用 schema 预判哪些调用必然失败"无法与 SDK 对齐。 | `pi-ai/dist/utils/validation.js:280-311`（`:286` `Value.Convert`）；P3c 表：`number where string is required  Value.Check=false -> EXECUTED` |
| 3 | **"并行模式"的准备阶段是串行的**：`beforeToolCall` 逐个被 await，第一个钩子的 `await`（如整批登记 50ms）会阻塞整批工具启动，也包括阻塞后续钩子的进入。 | `agent-loop.js:419-426`；P2 时间线：`call_b1_r1 t+6.981` → 登记完成 `t+61.28` → `call_b1_r2 t+61.448` |
| 4 | **write 一定会进入 execute**。并行批次所有工具在同一个 `Promise.all` 中启动，SDK 没有"不进入 execute"的机制；屏障只能存在于 execute 内部。 | `agent-loop.js:459`；P5 三组均为 `write execute() was entered: true` |
| 5 | **SDK 的 abort 早退只覆盖准备阶段**。`signal.aborted` 的提前返回在 deferred thunk 开头（Promise.all 之前）；一旦批次已经开始执行，SDK 不再拦截任何工具，也不会把"忽略信号的成功返回"改判为错误。 | `agent-loop.js:440-449`；P5c `read result: isError=false text="read done"` |
| 6 | **abort 不会让运行提前结束**：`Promise.all` 不可中断，运行要等所有工具 promise 结算。 | `agent-loop.js:459`；P5d `abort 后又等了 751.6ms`（工具 800ms 忽略信号） |
| 7 | **准备阶段中止后仍会再发一次 provider 请求**（该请求立即以 aborted 返回）。 | P4 事件表：`turn_end t+9.910` → `message_start(assistant aborted) t+10.176` |
| 8 | **`entry_appended` 看起来像"条目已追加"的公开信号，但对 message 类条目不发出**；而且 `message_end` 的语义取决于订阅位置：`session.subscribe` 在落盘**前**、`agent.subscribe`（AgentSession 监听器之后）在落盘**后**。 | `agent-session.js:649-653`、`:753`、`:755-770`、`:990-998`；P6 实测 `disk@sync=false/true` |
| 9 | **重开一个含孤立 toolCall 的会话不会报错**，SDK 不做配对校验，静默在 provider 适配层补 `No result provided`。同一份数据在会话层"不合法"，在 provider 层"被修好"。 | P7c；`transform-messages.js:138-166/210-211` |
| 10 | **`hasSequentialToolCall` 只在当前 tools 表里查**：表里不存在的工具名不会被判为 sequential，未知工具的批次仍走并行分支。 | `agent-loop.js:374`（`currentContext.tools?.find(...)`） |
| 11 | **`tool_execution_end` 是完成序，不是源序**（本次多数场景完成序恰好等于源序，容易误判）。 | `agent-loop.js:450-453`（每个工具一结束就 emit）；P4 实测 `r2 end t+9.010` 早于 `r1 end t+9.069` |
| 12 | 装置注记（非 SDK 缺陷）：`pi-ai` 的 faux 注册表是**进程级全局**的，多会话共享一份脚本队列会互相消费。本报告全部改用每会话独立 `createFauxCore`。 | P8 首版出现"B 的 toolCall 在 A 里执行"的伪象，改为独立 core 后消失 |

---

## 4. 阻塞项

判定口径：下表"依赖"若实测为"否"，"全部标 parallel + Host 屏障"方案即不可行。

| 依赖 | 若为"否"的后果 | 实测结论 | 是否阻塞 |
|---|---|---|---|
| 无 execute 的位置能被屏障感知为"已结算" | 屏障永久等待 → 整个 Run 卡死（不可被 abort 解开） | **是**：三个位置都发 `tool_execution_end`（P3a，屏障 70.2ms 后放行） | 否 |
| 屏障可放在 execute 内部实现 | 若依赖"未结算的 write 不会进入 execute"，逻辑直接错 | **是**：write 的 execute 一定被调用（P5），屏障必须在内部 | 否（前提必须被接受，见本节结论） |
| `beforeToolCall` 可以只等"登记" | 若在某处等"结算"→ 硬死锁且 abort 无效 | **是**：正例 P2 通过；负例 P2b 死锁 | 否（负例是设计禁区） |
| abort 能阻止 write 产生效果 | 中止后仍会落一次写入 | **否**：SDK 不阻止；只有工具自查 `signal.aborted` 才有效（P5a 写入 / P5b 未写入） | **是（条件性）**——方案依赖"每个工具自查信号"，这不是 SDK 保证 |
| Host 台账能拿到"该 toolResult 已落盘"的公开确认 | 若台账/恢复以"已落盘"为前置，缺少可信信号 | **否**：无公开事件；只有时序事实（`message_end` 后一次 microtask，或 `agent.subscribe` 时机）+ `getLeafId()`/读文件 | **是（硬缺口）** |
| 中止能使运行及时结算 | 若 Host 期望 abort 立即停，实际要等最慢工具 | **否**：`Promise.all` 不可中断（P5d 751.6ms） | **是（条件性）**——取决于工具是否响应信号 |
| 恢复可用"补交结果"或"回退分支" | 若两者都不可用，孤立 toolCall 无法收拾 | **是**：P7a / P7b 都通过 | 否 |
| 多会话在同一进程内互不干扰 | 若事件/abort/工具串话，单进程多会话不成立 | **是**：P8 全部隔离检查通过 | 否 |
| toolResult 顺序可预测 | 若无法对应 toolCall，台账无法对账 | **是**：源序（P1），entry id 可映射（P6） | 否 |
| readonly 并发可被外部限流到 4 | 若 SDK 自己就并发全部工具，限流无处安放 | **是**：限流在 execute 内有效（P1b peak=4） | 否 |

结论：**没有任何一个 A 部分结论为"否"到直接判死方案**；三条"是（条件性/硬缺口）"是：①**没有公开的"已落盘"信号**；②**abort 后是否产生效果完全取决于工具是否自查信号**，且运行结算会等最慢工具。

---

## 5. 未验证项

| 项 | 原因 |
|---|---|
| 真实 provider（非 faux）链路上 `transformMessages` 的实际效果 | 硬性约束禁止调用真实模型；只做了源码核对（各 `api/*.js` 调用 `transformMessages`）+ 直接调用该函数验证输出 |
| 断电/崩溃后的持久性（fsync） | 源码确认 `appendFileSync` 无 fsync（`session-manager.js:812`），未做断电实验 |
| 20+ 会话、长时间运行的内存曲线 | 只测了 2 个会话与单会话冷启动 |
| 多进程同时打开同一会话文件 | 超出本次范围，未测 |
| 与 SDK 判定**完全等价**的 Host 端预校验 | `Value.Convert` 的转换语义未复刻；只证明了朴素 `Value.Check` 不等价 |
| "`durationMs` 缺失 ⇒ 未执行"作为判据 | 仅源码推断（`agent-loop.js:652-661`），未单独实验 |
| Fox 真实 Kernel 路径（`pi-kernel-loop.mjs` 的 `settledFor` 机制）运行时行为 | 只读源码（`src/pi-kernel-loop.mjs:120-145`），未启动 Kernel worker |
| B 部分：旧二进制 × 旧数据库的真实升级 | 只做源码路径分析，未在真实 DB 上跑迁移（见 `PART-B-raw.md` 未验证清单） |

---

### 5.1 依赖内部实现的标注

以下结论所依据的代码**不在 SDK 的类型契约或顶层导出里**，只能从打包产物/子路径读到，按要求单独标注：

| 结论 | 依赖点 | 性质 |
|---|---|---|
| P4-3 / P7：provider 适配层会用 `No result provided` 补齐孤立 toolCall、并丢弃 aborted/error 的 assistant 消息 | `pi-ai/dist/api/transform-messages.js:63`（`transformMessages`） | **依赖内部实现**。该函数不在 `pi-ai` 顶层 index 上，只能经包 `exports` 的 `./api/*` 子路径导入；行为也与具体 provider 适配器有关（各 `api/*.js` 自行调用），未在真实 provider 上验证 |
| P6：`entry_appended` 不对 message 类条目发出 | `pi-coding-agent/dist/core/agent-session.js:753/755-770` | **依赖内部实现**。`AgentSessionEvent` 类型里声明了 `entry_appended`，但"何时发出"没有契约，结论来自源码阅读 + 实测 0 次观测 |
| P6：落盘是 `appendFileSync`（无 fsync） | `session-manager.js:794-813` | **依赖内部实现**（未在 `.d.ts` 承诺） |
| P3 补充：immediate 结果的 `tool_execution_end` 不带 `durationMs` | `agent-loop.js:652-661` | **依赖内部实现**（`durationMs` 在 `AgentEvent` 类型里是可选字段，未说明何时缺省）；未单独实验 |
| P5：abort 的提前返回只在 deferred thunk 开头生效 | `agent-loop.js:440-449` | 源码定位，但**行为已被实验观测到**（P5a/P5b 的 execute 未执行 vs P5c 执行完） |

其余结论（P1、P2、P2b、P3a、P5-1/2/4、P6 的落盘时刻、P7a/P7b、P8）只用到顶层公开 API：`Agent`/`AgentOptions.streamFn`/`subscribe`/`abort`、`createAgentSession`、`SessionManager`、`createFauxCore`/`fauxAssistantMessage`/`fauxToolCall`、`Type`。

## 6. 复现命令清单

全部命令在 `D:\python\projects\Fox\Fox\services\agent-runtime` 下执行（PowerShell 7）：

```
node .probe-s0/p1-mixed-batch.mjs          # P1 混合批次屏障 + 六路并发上限
node .probe-s0/p2-batch-registration.mjs   # P2 整批登记
node .probe-s0/p2b-settle-deadlock.mjs     # P2b 负面对照（会打印 hung，正常退出码 0）
node .probe-s0/p3-missing-execute.mjs      # P3 无 execute 位置 + 三种做法 + 参数校验现实检查
node .probe-s0/p4-prep-abort.mjs           # P4 准备阶段中止（含真实 JSONL）
node .probe-s0/p5-exec-cancel.mjs          # P5 执行中取消
node .probe-s0/p6-persistence.mjs          # P6 落盘时机（真实 AgentSession）
node .probe-s0/p7-artifact.mjs             # P7 产物：assistant 有 toolCall、无 toolResult（会主动退出）
node .probe-s0/p7-resume.mjs               # P7 两种恢复方式（依赖上一条）
node .probe-s0/p8-multi-session.mjs        # P8 单进程多会话 + RSS
node .probe-s0/lib/cold-start.mjs          # 独立进程冷启动计时/RSS（P8 内部会调用）
```

每条命令的完整输出已保存在 `.probe-s0/out-*.txt`。

> 数值稳定性说明：报告里引用的毫秒时间戳取自一次代表性运行，`out-*.txt` 是最后一次整体复跑的输出，两者会有数毫秒到数十毫秒差异（机器噪声、JIT、磁盘缓存）。**判定行（PASS/FAIL/YES）与结构性结论稳定**；会话 entry id 由 `randomUUID().slice(0,8)` 生成（`session-manager.js:21-29`），每次运行都不同，引用值仅用于展示结构关系；会话文件名含 uuidv7 会话 id，同样每次不同。

## 7. 文件清单

| 路径 | 作用 |
|---|---|
| `lib/harness.mjs` | 时钟/时间线、Host 屏障调度器、探针工具集、faux 封装、Agent 封装 |
| `lib/session.mjs` | 真实 `AgentSession` 构造（每会话独立 faux provider、工具白名单） |
| `lib/cold-start.mjs` | 独立进程冷启动测量 |
| `p1-mixed-batch.mjs` … `p8-multi-session.mjs` | A 部分各实验 |
| `p2b-settle-deadlock.mjs` | P2 负面对照 |
| `p7-artifact.mjs` | P7 的会话产物生成器 |
| `out-*.txt` | 各实验的完整输出（本报告引用的片段来源） |
| `PART-B-raw.md` | B 部分源码盘点原始记录（410 行） |
| `tmp/` | 临时目录：真实会话 JSONL（p4/p6/p7/p8）与写入目标文件，可随时删除 |
