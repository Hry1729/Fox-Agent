# PART-B 只读源码审计（state.worker / RunControlBinding / skill_load / 删除成本）

审计方式：全部结论均来自 `rg -n` 与逐行 `Get-Content` 阅读，未构建、未运行测试、未修改除本文件外的任何文件。
行号来源：`rg -n` 输出，或对 `Get-Content` 输出逐行编号。
凡标注 `未验证` 的条目均给出原因。

---

## Q1 — state.worker 清单

### 1.1 标识符作用域澄清（重要，避免计数混淆）

`WorkerHandle` 在整个仓库只有 3 处（`rg -n "WorkerHandle"`）：

- `apps/desktop/src-tauri/src/runtime_host/mod.rs:747`（struct 定义）
- `apps/desktop/src-tauri/src/runtime_host/mod.rs:795`（`RuntimeHostState.worker` 字段）
- `apps/desktop/src-tauri/src/runtime_host/mod.rs:4019`（唯一构造点）

`legacy_available` 全仓库只有 2 行，且是**函数内局部变量**，不是结构体字段：
`mod.rs:1967`（定义）、`mod.rs:1973`（使用）。

`state.worker` 的读取跨越 `mod.rs` 与 `kernel_host.rs` 两个文件；`rg -n "\.worker\b"` 在
`kernel_model_worker.rs` 里另有 27 行 `self.worker.*`，那是 `KernelModelWorker` 自己的
`worker` 字段（Kernel 独立进程），与 `state.worker` **无关**，不计入下表。

同名但不同概念、已排除：`runs.active_run_id` / `ConversationSummary.active_run_id`
（`database/models.rs:384`、`database/repositories.rs:5415`、`commands/mod.rs:4020` 等）。

### 1.2 发生点清单（occurrence = 一次逻辑读/写；跨多行的链式调用合并为一行表项）

| file:line | 内容 | 分类 |
|---|---|---|
| `runtime_host/mod.rs:747` | `struct WorkerHandle {` | 其他（类型定义） |
| `runtime_host/mod.rs:795` | `worker: Option<WorkerHandle>,` | 其他（状态字段声明） |
| `runtime_host/mod.rs:1449-1456` | `current_conversation_id: state.worker.as_ref()...` / `active_run_id: state.worker...active_run_id.clone()`（`status()` 只读上报） | 其他 |
| `runtime_host/mod.rs:1497-1504` | 同上，`diagnostics()` 只读上报 | 其他 |
| `runtime_host/mod.rs:1541-1547` | `.worker.as_ref().and_then(\|worker\| worker.active_run_id.clone())`，`reload_configuration` 用它拒绝改模型配置 | 其他 |
| `runtime_host/mod.rs:1625-1629` | `if let Some(worker) = state.worker.as_mut() { ... worker.active_run_id = None; }` | 启动停止状态机 |
| `runtime_host/mod.rs:1630-1632` | `state.worker.as_ref().and_then(\|worker\| worker.active_run_id.as_ref()).is_none()` → 置 `ready` | 启动停止状态机 |
| `runtime_host/mod.rs:1967-1973` | `let legacy_available = ... worker.active_run_id.as_ref()).is_none() ...`；`if item.kernel { true } else { legacy_available }` | 启动停止状态机（派发闸门） |
| `runtime_host/mod.rs:1998-2001` | `CANCELLED_BEFORE_SUBMISSION` 分支里再查 `worker.active_run_id` 后置 `ready` | 取消 |
| `runtime_host/mod.rs:2026-2031` | `cancel_queued_run`：`state.worker.as_ref().and_then(...active_run_id.as_deref()) != Some(run_id)` | 取消 |
| `runtime_host/mod.rs:2581-2584` | `worker.active_run_id = Some(started.run.id.clone()); worker.current_runtime_session_id = ...`（写） | 启动停止状态机 |
| `runtime_host/mod.rs:2742-2748` | `live_runtime_session_id`：`let worker = state.worker.as_ref()?;` + 会话 id 比对 | 其他（会话身份查询） |
| `runtime_host/mod.rs:2785-2789` | `reset_conversation_context` 后写 `worker.current_runtime_session_id` | 其他（会话身份更新） |
| `runtime_host/mod.rs:2805-2816` | `purge_conversation_session_files`：`.worker...current_conversation_id == Some(conversation_id)` → `stop_worker()` | 启动停止状态机 |
| `runtime_host/mod.rs:3303-3318` | `ensure_active_envelope_identity`：取 conversation/run/session 三元组做 envelope 身份校验 | 请求响应 |
| `runtime_host/mod.rs:3687-3707` | `cancel_run`：`worker.active_run_id.as_deref() != Some(run_id)` 才继续 | 取消 |
| `runtime_host/mod.rs:3828-3835` | `remove_conversation`：读 `current_conversation_id` 决定是否 `stop_worker` | 启动停止状态机 |
| `runtime_host/mod.rs:3877-3891` | 应用退出：`.worker...active_run_id.clone()` + 取队列，随后 `stop_worker()` 并 `mark_run_interrupted` | 启动停止状态机 |
| `runtime_host/mod.rs:3911-3925` | `mark_run_ready`：清 `active_run_id`，`state.state` 置 ready/busy，触发 `dispatch_next_queued_run` | 启动停止状态机 |
| `runtime_host/mod.rs:3934-3947` | `ensure_worker`：读 `current_conversation_id` 与 `active_run_id.is_some()`（busy） | 启动停止状态机 |
| `runtime_host/mod.rs:4019-4026` | `state.worker = Some(WorkerHandle { child, stdin, pending, ... active_run_id: None })`（唯一构造） | 启动停止状态机 |
| `runtime_host/mod.rs:4105-4121` | `send_request`：`(worker.stdin.clone(), worker.pending.clone())` 后注册 pending 并写 stdin | 派发 |
| `runtime_host/mod.rs:4627-4657` | stdout 事件循环内身份校验失败时读 `worker.active_run_id` → `mark_run_failed` + `handle_runtime_crash` | 事件路由 |
| `runtime_host/mod.rs:4848-4892` | `handle_runtime_crash`：读 conversation+active_run，`let worker = state.worker.take();`，`child.kill()/wait()` | 退出回调 |
| `runtime_host/mod.rs:4973-4993` | `stop_worker`：`state.worker.is_some()` → 发 `shutdown` → `state.worker.take()` → kill | 启动停止状态机 |
| `runtime_host/mod.rs:5009-5014` | `cleanup_failed_worker`：`.and_then(\|mut state\| state.worker.take())` → kill → `crashed` | 启动停止状态机 |
| `runtime_host/mod.rs:10346-10353` | `record_runtime_crash`：`state.worker.as_ref().and_then(...conversation?, active_run?)` | 退出回调 |
| `runtime_host/kernel_host.rs:1808-1814` | Kernel 恢复结束：`&& state.worker.as_ref()...active_run_id.as_ref()).is_none()` → `ready` | 启动停止状态机 |
| `runtime_host/kernel_host.rs:2189-2195` | Kernel 启动结束：同一判定 → `ready` | 启动停止状态机 |

### 1.3 计数

- 总发生点：**29**（`mod.rs` 27 + `kernel_host.rs` 2）。原始标识符行共 37 行（`state.worker`/`.worker` 33 行 + `WorkerHandle` 自身定义 2 行 + `legacy_available` 2 行），其中 8 处属于同一语句的多行链式调用，合并后为 29 个发生点。
- 按桶：派发 **1**；请求响应 **1**；事件路由 **1**；取消 **3**；退出回调 **2**；启动停止状态机 **14**；其他 **7**。
- 相关但未计入（不读 `state.worker`）：响应解析走 `send_request` 克隆出的 `pending` Arc
  （`mod.rs:4134`、`mod.rs:4143`，在 `spawn_stdout_reader` 线程内），因此「请求响应」桶里
  `state.worker` 只在发起侧出现一次。

### 1.4 Yuxi / codex / deepseek_harness 是否走同一个 worker

| 路径 | 是否走 `state.worker` | 证据 |
|---|---|---|
| Yuxi | **false** | `commands/mod.rs:3944-3955`：`if runtime.runtime_type == "yuxi" { state.yuxi_runtime.start_run_detached(...) } else { state.runtime_host.start_run_detached(...) }`；`yuxi/runtime.rs:45/76/119` 全部是 `tauri::async_runtime::spawn` + `YuxiClient` HTTP，无子进程、无 `WorkerHandle`。`runtime_host/session_lifecycle.rs:194` 也对 `runtime_type == "yuxi"` 单独分支。 |
| codex（Kernel engine_id） | **false** | `services/agent-runtime/src/pi-kernel-worker.mjs:17` `export const RETIRED_KERNEL_ENGINES = ['codex', 'deepseek_harness']`；`pi-kernel-worker.mjs:115-119` 在 initialize 阶段 `throw retiredEngineError(identity.engineId)`；`crates/fox-engine-protocol/src/control.rs:230` 与 `database/repositories/run_control.rs:281` 仍允许该字符串落库（旧记录可读），`kernel_model_config.rs:26` 同样标记为 retired。 |
| deepseek_harness | **false** | 同上（`pi-kernel-worker.mjs:17/115-119`）。若指 External/ACP 后端（`AgentBackend::Deepseek`，`crates/fox-agent-backend/src/port.rs:14`），它由 `external_backend.rs:272 start_run(&self, request, backend: AgentBackend)` 走 ACP 外部进程，同样不经 `state.worker`。 |

补充：Kernel 路径整体不经 `state.worker`。Kernel 进程由 `kernel_model_worker.rs:517-524`
`Worker::spawn` 启动，命令行带 `--kernel-worker`（`command.arg("--kernel-worker")`），对应
`services/agent-runtime/src/pi-runtime.mjs:1129` `process.argv.includes('--kernel-worker') ? createKernelWorker(write) : null`。
`pi-kernel-worker.mjs:36` 注释原文：`A separate process mode, not another command on a shared Legacy worker.`
`kernel_host.rs` 内不存在 `send_request` 调用（`rg -n "send_request" kernel_host.rs` 无结果）。

**结论**：`state.worker` 全部 29 处读写集中在 Legacy（`runtime_type == "pi"`）单进程 sidecar 上，其职责以启动/停止状态机（14/29）为主；Yuxi、codex、deepseek_harness（无论作为 Kernel engine 还是外部后端）都不经过这个 worker。

---

## Q2 — RunControlBinding（`crates/fox-engine-protocol/src/control.rs:212`）

### 2.1 结构与本机校验

- 定义：`crates/fox-engine-protocol/src/control.rs:210-223`，`#[serde(rename_all = "camelCase", deny_unknown_fields)]`（第 211 行）。
- `CONTROL_SCHEMA_VERSION: u32 = 1`：`control.rs:5`。
- `validate()`：`control.rs:226-236`，首行即 `if self.schema_version != CONTROL_SCHEMA_VERSION { return Err("unsupported frozen control schema".into()); }`（第 227 行）。
- 嵌套类型的 `deny_unknown_fields` 实况：
  - `PermissionGrant`：**有**（`control.rs:56`，`rename_all = "camelCase", deny_unknown_fields`）
  - `FrozenPermission`：**有**（`control.rs:67`）
  - `TimeBudgets`：**有**（`control.rs:84`）；且其自定义 `Deserialize` 里的内部结构 `struct Stored` 也带 `deny_unknown_fields`（`control.rs:120`）
  - `RunControlBinding`：**有**（`control.rs:211`）
  - `ExecutionAuthority`：无（不是 struct，是 enum，`control.rs:7-9`）。枚举语义上不存在「未知字段」，只有未知变体被拒。

### 2.2 构造点

| file:line | 说明 |
|---|---|
| `database/repositories/run_control.rs:335-341` | **唯一生产构造点**（`freeze_selected_run_control` 内），`schema_version: fox_engine_protocol::CONTROL_SCHEMA_VERSION` |
| `runtime_host/mod.rs:2904-2917` | 子 Run 从父 binding 派生（`let mut inherited = ...` 后 `freeze_run_control(&inherited)`），非从零构造 |
| `database/repositories/kernel_continuation.rs:762` | 测试模块内构造 |
| `runtime_host/kernel_gateway.rs:1533 / 1573 / 1630`、`mcp_catalog.rs:87` | 测试内构造 |
| `closeout_fixture.rs:75`、`runtime_host/delivery.rs:8697`、`kernel_executor`/`kernel_coordinator(tests.rs:280/3771/4013/4163)`、`resource_gateway.rs:885`、`p2e_router_tests.rs:112`、`p2b_decision_tests.rs:132`、`p2a_host_decode_tests.rs:440`、`delivery_*_tests`、`kernel_artifact_gate_tests.rs:43/237`、`rev_repro_tests.rs:1275`、`real_eval_tests.rs:2613`、`waiting_jobs_boundary_tests.rs:343`、`kernel_authorization.rs:504` | 测试/fixture 构造 |

### 2.3 持久化点

| file:line | 表 / 列 | 是否 JSON 文本 |
|---|---|---|
| `database/migrations.rs:6811-6819` | `CREATE TABLE run_control_bindings(run_id, conversation_id, authority, engine_id, binding_json, binding_hash, created_at)` | `binding_json TEXT NOT NULL CHECK(json_valid(binding_json))` → **是 JSON 文本**；`binding_hash` 是 `binding_json` 的 `sha256:` 内容哈希（`run_control.rs:8`、`345`） |
| `database/repositories/run_control.rs:343-345` | `INSERT INTO run_control_bindings(... binding_json, binding_hash ...)`（首次冻结） | 是 |
| `database/repositories/run_control.rs:371-374` | `INSERT INTO run_control_bindings(...)`（`freeze_run_control`，显式传入 binding） | 是 |
| `database/repositories/run_control.rs:10-17` | `encode()`：`binding.validate()?` → `canonical_json::to_string(binding)` | 序列化点（写与读都经过它） |

`run_control_bindings.binding_json` 没有任何生产 UPDATE 路径（唯一 UPDATE 是测试 `mod.rs:7412`
`UPDATE run_control_bindings SET binding_hash='corrupt'`）。

### 2.4 读取 / 反序列化点

| file:line | 说明 |
|---|---|
| `database/repositories/run_control.rs:380-396` | **中心读入口** `run_control_binding()`：`:388` 先校验 `binding_hash`，`:389` `serde_json::from_str::<RunControlBinding>(&json).map_err(\|e\| format!("corrupt control binding: {e}"))`，`:390` 再 `encode(&binding)?`（即重跑 `validate()`），`:392` 比对列与 JSON 身份 |
| `database/repositories/kernel_execution_admission.rs:145-155` | 直接 `SELECT binding_json` 后 `serde_json::from_str`（`:154`），**不校验 schema_version** |
| `database/repositories/kernel_execution_admission.rs:226-236` | 同上，`.and_then(\|json\| serde_json::from_str::<...>(&json).ok())`，**失败静默变 None** |
| `database/repositories/kernel_execution_admission.rs:1895-1904` | `SELECT` 后反序列化 + `binding_hash` 校验（`:1897-1903`）+ `binding.validate()?`（`:1904`） |
| `database/repositories/kernel_execution_admission.rs:2145-2156` | `serde_json::from_str(...).ok()` → 失败静默变 `None` → 后续视为 `budget_binding_unavailable` |
| `database/repositories/kernel_authorization.rs:192-195` | `serde_json::from_str(...).map_err(\|e\| format!("invalid frozen binding: {e}"))?` |
| `database/repositories/kernel_model_config.rs:65-67` | `serde_json::from_str(...).map_err(\|_\| invalid("invalid frozen control binding"))?` 后 `binding.validate()` |
| `services/agent-runtime/src/control-binding.mjs:24-29` | JS 侧不反序列化到 struct，而是 `validateWireValue('RunControlBinding', binding)`（`:24`）+ 硬编码 `binding.schemaVersion !== 1`（`:26`）。调用方：`pi-kernel-batch-resume.mjs:132/204`、`pi-kernel-compaction.mjs:39`、`pi-runtime.mjs:1057`（Legacy） |

Rust 侧不存在「从 wire 反序列化 binding」的路径：binding 只从 SQLite 读出，再 `Serialize`
进帧（`kernel_model_worker.rs:778/844/1011`、`kernel_coordinator/live.rs:1678`、`mod.rs:2604`）。

### 2.5 `validate()` 调用点

`RunControlBinding::validate()` 本身：`control.rs:226`（定义）；`control.rs:235` 内调用 `self.budgets.validate()`。

外部调用点：

| file:line | 场景 |
|---|---|
| `database/repositories/run_control.rs:11` | `encode()` 内 —— 写(`:342`)与读(`:390`)都经此，最关键 |
| `database/repositories/run_control.rs:275` | `freeze_kernel_run_control` 先 `budgets.validate()` |
| `database/repositories/run_control.rs:280` | `freeze_kernel_run_control_for_engine` 同上 |
| `database/repositories/kernel_continuation.rs:488` | `binding.validate().map_err(\|_\| invalid("续做绑定无效"))?` |
| `database/repositories/kernel_execution_admission.rs:1904` | 旧凭证签发路径 |
| `database/repositories/kernel_model_config.rs:67` | 读取冻结模型配置时 |
| `database/repositories/kernel_reconciliation.rs:293` | 对账/恢复时 |
| `runtime_host/kernel_gateway.rs:418` | `self.binding.validate()?`（每个 effect 执行前） |
| `runtime_host/kernel_model_worker.rs:881`、`:1063` | 下发 initial/batch 帧前 |
| `resource_gateway.rs:39`、`:462`、`:491` | 只读/写资源网关每次执行前 |
| `runtime_host/attachment_compute.rs:222`、`:774` | 计算/项目绑定前的绑定校验 |

### 2.6 问题 A：把 `CONTROL_SCHEMA_VERSION` +1，旧库记录在哪些路径开始失败

机制：`serde_json::from_str` 不做版本校验，旧记录（`schemaVersion: 1`）**能反序列化成功**；
失败发生在随后的 `validate()`（`control.rs:227`），错误串固定为 `unsupported frozen control schema`。

具体路径（按启动到执行）：

1. **启动恢复**：`lib.rs:274` → `kernel_host.rs:1753 recover_kernel_runs_detached()`
   → `kernel_host.rs:1783-1786 .run_control_binding(&run_id)?.ok_or("missing Kernel recovery binding")?`
   → 读路径 `run_control.rs:389` 反序列化成功 → `:390 encode()` → `:11 validate()` → `Err("unsupported frozen control schema")`。
   该 Err 走 `kernel_host.rs:1801-1804`（`host.record_kernel_start_failure(&run_id)`），并把
   `state.last_error` 置为 `kernel_host.rs:1817-1821` 的字面量
   `"A Kernel Run could not be recovered; no fallback execution was used."`。
   结果：所有旧 Run 在重启后都无法恢复，且按设计**不回落 Legacy 执行**。
2. **正常派发**：`runtime_host/mod.rs:2379`（`start_run_inner` 读冻结绑定）→ `:2455-2484`
   比对 authority/engine/profile 之前就已失败；错误冒泡到 `start_run` 的失败分支
   `mod.rs:1611-1636`，写 `state.last_error` 并（若 worker 空闲）把状态复位 `ready`。
3. **Kernel 起跑/续跑**：`mod.rs:2467` 的 `freeze_kernel_run_control_for_engine` 会先命中
   `run_control.rs:288-292` 的「重启必须复用既有 binding」分支 → 再次走读路径 → 同上失败。
4. **每个 effect 的网关再校验**：`kernel_gateway.rs:826 / 944 / 1043 / 1102 / 1184 / 1203`
   （`database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)`）
   → 直接 `Err` → 工具 effect 被拒（fail-closed，不会静默放行）。
5. **只读/写资源网关**：`resource_gateway.rs:39 / 462 / 491` 各自 `binding.validate()?` → 拒绝。
6. **Rust reader / 命令 job / 后台 job**：`mod.rs:9722`、`:9810`、`:10032`、`:1186`、
   `background_jobs.rs:43/72`、`command_jobs.rs:254/319` → `Err` → 该操作返回错误
   （例如 `command_jobs.rs:254` 的 `"command run binding missing"` 变体或下层错误串）。
7. **老 sidecar（未升级的 JS）**：即使 Host 升级并把 `schemaVersion: 2` 写进帧，
   `services/agent-runtime/src/control-binding.mjs:26` 硬编码 `binding.schemaVersion !== 1`
   → 抛 `Unsupported frozen Run control version`。Legacy 侧同样中招（`pi-runtime.mjs:1057`
   → `control-binding.mjs:9` 的 `validatePromptControl`）。
8. **不失败的路径（静默/半静默）**：`kernel_execution_admission.rs:234` 与 `:2154` 用
   `.ok()` 吞掉错误。版本升级本身不触发它们（它们从不看 `schemaVersion`），只会在
   下游以 `budget_binding_unavailable`（`:2157-2158`）或 budget=0（`:236`）显现。
   这是本次审计里唯一「不做版本判定」的读点。

失败模式汇总：**命令/恢复被拒绝并返回错误串**，不是静默忽略；错误串主要是
`unsupported frozen control schema`（Rust）与 `Unsupported frozen Run control version`（JS），
读入口包装后是 `corrupt control binding: ...`。

### 2.7 问题 B：给结构体新增一个路由字段，旧二进制会在哪些路径拒绝

前提：`RunControlBinding` 带 `#[serde(deny_unknown_fields)]`（`control.rs:211`），
所以**旧二进制读新记录**会得到 serde 的 `unknown field <name>, expected one of ...`。

| 读点 | 旧二进制的表现 |
|---|---|
| `run_control.rs:389` | `format!("corrupt control binding: {e}")`，`e` 为 unknown field → 所有经中心读入口的路径全部失败 |
| `kernel_model_config.rs:65` | `invalid("invalid frozen control binding")` → Kernel 冻结配置读取失败 |
| `kernel_execution_admission.rs:154` | `map_err(\|e\| e.to_string())` → 直接冒 serde 原文，凭证签发失败、dispatch 被拒 |
| `kernel_execution_admission.rs:234` | `.ok()` → `None` → `budget_ceiling_ms = 0`（`:236`） |
| `kernel_execution_admission.rs:2154` | `.ok()` → `None` → `budget_binding_unavailable`（`:2157`） |
| `kernel_authorization.rs:193` | `format!("invalid frozen binding: {e}")` → 授权登记被拒（`GrantRegistration::Skipped` 之外的硬错误） |
| `control-binding.mjs:24-25` | `validateWireValue` 返回 errors → `Invalid frozen Run control: ...`。JSON Schema 里 `RunControlBinding` 有 `additionalProperties: false`（`packages/fox-engine-protocol/schema.json:1831`；嵌套 `FrozenPermission` `:1698`、`PermissionGrant` `:1748`、`TimeBudgets` `:1784` 也各自为 false） |

嵌套类型补充：新字段若加在 `TimeBudgets` / `FrozenPermission` / `PermissionGrant` 内，同样被
`deny_unknown_fields`（`control.rs:84 / 67 / 56`）挡住；`TimeBudgets` 还有内部 `Stored`
（`control.rs:120`）双重把关。若加在 `ExecutionAuthority` 上不成立（它是 enum）。

反向兼容（新二进制读旧记录）：若新字段没有 `#[serde(default)]`，新二进制读旧记录会得到
`missing field ...`；现有代码里同类字段（`approval_epoch`、`model_first_response_ms`、
`model_idle_ms`、`run_execution_limited`）都显式加了 `#[serde(default)]`
（`control.rs:79/92/97/103`），这是既有惯例。

`binding_hash` 不构成保护也不构成障碍：它是 `binding_json` 的内容哈希
（`run_control.rs:8/345`、`:388`），记录一旦写入，新旧二进制算出的值一致。

**结论**：`RunControlBinding` 的唯一生产构造/落库点是 `run_control.rs:335-345`，唯一中心读点是
`run_control.rs:380-396`（另有 5 处绕过中心读点的直读 SQL）；`CONTROL_SCHEMA_VERSION` +1 会让所有
旧记录在 `control.rs:227` 处被拒（`unsupported frozen control schema`），启动恢复、正常派发、
网关再校验、JS sidecar 全线 fail-closed，只有 `kernel_execution_admission.rs:234/2154` 两处
用 `.ok()` 静默吞掉；新增字段则靠 `deny_unknown_fields`（binding 与其三个嵌套 struct 都有，
enum 无）让旧二进制在反序列化处以 `unknown field` / `Invalid frozen Run control` 拒绝。

---

## Q3 — skill_load 激活对同一 Run 内的影响

### 3.1 被质疑的断言与它实际描述的代码

`runtime_host/kernel_coordinator/live.rs:133-134` 原文：

```
///  * `skill_load` returns read-only skill text; its only persistence is an
///    idempotent `INSERT OR IGNORE` activation row.
```

该断言服务于 `parallel_read_only_dispatch`（`live.rs:138-149`）：它把 `read` / `ls` / `find`
/ `grep` / `skill_load` 判定为可并行只读，其余为串行屏障（`:147`）。

`skill_load` 的实际执行链：

| file:line | 内容 |
|---|---|
| `runtime_host/skill_load.rs:282-291` | `fn execute_load(...)`：`let loaded = skills::on_demand_skill(skills_dir, enabled_skill_ids, frozen_tools, skill_id)?` —— 纯读取 |
| `runtime_host/skill_load.rs:293-308` | `database.record_skill_activation(run_id, Some(conversation_id), &SkillActivationRecord{...})` |
| `database/repositories/skill_activations.rs:68-88` | `"INSERT OR IGNORE INTO skill_activations(...)"` —— 幂等插入，列含 run_id/skill_id/version/content_sha256/… |
| `runtime_host/skill_load.rs:309-316` | 工具不可用时只加一条警告文本，**不授权** |
| `runtime_host/skill_load.rs:317-330` | 返回值只有 `content: [{type:"text", text: loaded.block}]` + details |
| `runtime_host/skill_load.rs:20-23`（模块文档） | `Neither discovery nor loading grants tools. ... the frozen scope is what denies the later call.` |

`rg -n "skill_activations\("` 全仓库只命中两处：定义 `skill_activations.rs:35` 和 INSERT
`skill_activations.rs:69`。也就是说 **`skill_activations(run_id)` 这个读取函数在生产代码里没有调用方**，
这些行目前是纯审计数据。

### 3.2 工具目录（catalog）与 system prompt 是否在同一 Run 内可变

Kernel 路径（`--kernel-worker` 进程）：

| file:line | 事实 |
|---|---|
| `services/agent-runtime/src/pi-kernel-worker.mjs:102-104` | `const systemPrompt = request.payload?.systemPrompt`、`const definitions = request.payload?.proposalTools` —— 只从 initialize 请求取一次 |
| `services/agent-runtime/src/pi-kernel-worker.mjs:142-149` | `SettingsManager.inMemory({ compaction: {enabled:false}, retry: {...} })` + `DefaultResourceLoader({ ... systemPrompt })` + `createFoxAgentSession({ ... tools: [], customTools: [] ... })` |
| `services/agent-runtime/src/pi-kernel-worker.mjs:150-152` | 注释原文 `// The Host freezes this worker's prompt and tools.`，随后 `session.agent.prepareNextTurnWithContext = undefined`（阻止 Pi 每轮重建 SDK loadout） |
| `services/agent-runtime/src/pi-kernel-worker.mjs:155-167` | live loop：`installKernelHostTools(session, definitions, ...)`；single-round：`installKernelProposalSchemas(session, definitions)` —— 都在 initialize 阶段一次性安装 |
| `services/agent-runtime/src/pi-kernel-loop.mjs:144`、`:207` | 两处唯一的 `session.agent.state.tools = tools` 赋值（install 时刻） |
| `services/agent-runtime/src/pi-kernel-loop.mjs:224-233` | **每个 provider 请求**都会经过的 `installHostSchemaProjection`：`session.agent.streamFunction = (model, context, options) => { ... projectTools(context.tools) ... }` —— 它只把参数 schema 投影成严格版，**不增删工具** |
| `services/agent-runtime/src/pi-kernel-loop.mjs:251-256` | 起跑前断言 `session.agent.state.tools` 与安装时**逐个 `!==` 全等**，否则 `fail('replacement session must not retain executable local tools')` |

Legacy 路径（`state.worker`）：

| file:line | 事实 |
|---|---|
| `runtime_host/mod.rs:2588-2605` | 一次 Run 只构造一次 `RuntimeRequest::new("prompt")`，payload 内含 `"systemPrompt": context.system_prompt`、`"skillPrompt": context.skill_prompt`、`"controlBinding": control_binding` |
| `runtime_host/mod.rs:2450-2454` | `skill_activations` 来自 Run 起点的 `skill_plan`，在起跑时写库（`record_skill_activation`） |
| `services/agent-runtime/src/pi-runtime.mjs:736-754` | `composeRuntimePrompt({ systemPrompt: request.payload?.systemPrompt, ... })` —— 每个 `prompt` 请求（= 每个 Run）组合一次 |
| `services/agent-runtime/src/pi-runtime.mjs:786-797` | `DefaultResourceLoader({ ..., systemPrompt: promptComposition.prompt })` |
| `services/agent-runtime/src/pi-runtime.mjs:1057` | `const frozenProject = validatePromptControl(request, executionProfile.id)` —— 每个 prompt 校验一次绑定 |

### 3.3 结论

- 断言成立：`skill_load` 的唯一写操作是 `INSERT OR IGNORE` 审计行（`skill_activations.rs:69`），
  且该行没有任何生产读取方。
- 同一 Run 内 catalog 与 system prompt **不能**改变：Kernel 侧在 worker initialize 时冻结
  （`pi-kernel-worker.mjs:146-152`），每请求只做 schema 投影（`pi-kernel-loop.mjs:224-233`），
  并在起跑前断言工具数组全等（`:251-256`）；Legacy 侧 system prompt 与 skill prompt 在 Run 起点
  生成（`mod.rs:2588-2605` → `pi-runtime.mjs:736-797`）。
- 因此不存在「生效时刻」需要改动，也不存在被 skill 激活失效的缓存：全仓库没有
  「激活后重新读取 skill 清单」或「按 activation 失效 prompt/catalog 缓存」的代码路径
  （`rg -n "skill_activations\("` 只有定义与 INSERT）。
- 唯一在同一 Run 内可见的效果是**内容通道**：`skill_load` 的工具结果文本进入 transcript
  （`skill_load.rs:317-330`），模型下一轮能看到这段文本；这不是 catalog/prompt 变化，
  也不新增工具授权（`skill_load.rs:20-23`）。

**结论**：`live.rs:133` 的断言与代码一致——`skill_load` 只做幂等激活行插入，Kernel 的
tool catalog 与 system prompt 在 worker initialize 时即冻结、每请求只做严格 schema 投影，
同 Run 内既不会重建也不会因激活而失效任何缓存。

---

## Q4 — 删除成本行数统计

### 4.1 路径确认

| 需求中的名字 | 实际路径 |
|---|---|
| `services/agent-runtime/pi-kernel-worker.mjs` | **`services/agent-runtime/src/pi-kernel-worker.mjs`**（该目录下无顶层 `pi-kernel-worker.mjs`） |
| `services/agent-runtime/pi-kernel-compaction.mjs` | **`services/agent-runtime/src/pi-kernel-compaction.mjs`** |
| `services/agent-runtime/pi-kernel-loop.mjs` | **`services/agent-runtime/src/pi-kernel-loop.mjs`** |

### 4.2 行数（总行 / 非空非注释行）

注释行判定规则（下文所有「code」列都用它）：把行 `Trim()` 后，
① 为空 → 不算；② 以 `//`、`/*`、`*/`、`*` 开头 → 不算；其余都算。
局限：块注释内不以 `*` 开头的续行会被算作代码行，因此 code 列是**上界**（偏大）。

| 模块 | 总行 | code 行 | 备注 |
|---|---|---|---|
| `apps/desktop/src-tauri/src/runtime_host/kernel_model_worker.rs` | **1629** | 1415 | 含内联测试 |
| `services/agent-runtime/src/pi-kernel-worker.mjs` | **443** | 357 | |
| `services/agent-runtime/src/pi-kernel-loop.mjs` | **602** | 437 | |
| `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/compaction.rs` | **362** | 323 | |
| `services/agent-runtime/src/pi-kernel-compaction.mjs` | **81** | 69 | |
| `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/live.rs` | **2039** | 1493 | 见 4.3 的 loop-control 拆分 |

合计：**5156 行 / 4094 code 行**。若把 `live.rs` 只按 loop-control 计（见 4.3），
合计为 **4163 行 / 3383 code 行**。

### 4.3 live.rs 的 loop-control 部分（含精确区间与规则）

规则（本次审计采用的定义）：loop-control = 直接拥有「轮次边界 → 派发 → 决策提交」的
三个方法体，即 `service_round_output`、`commit_round_decision`、`dispatch_live`。
区间由 `rg`/`Select-String` 得到的函数起始行界定：

- `service_round_output`：`live.rs:632`–`1364`
- `commit_round_decision`：`live.rs:1365`–`1449`
- `dispatch_live`：`live.rs:1490`–`1716`（`impl` 块在 `:1717` 关闭）

精确计数（同一 code 规则）：

| 区间 | 总行 | code 行 |
|---|---|---|
| `632`–`1449` | 818 | 599 |
| `1490`–`1717` | 228 | 183 |
| **loop-control 合计** | **1046** | **782** |

上界参考：若把整个 `impl KernelCoordinator`（`live.rs:229`–`1717`，含全部只读辅助函数）
都算作 loop-control，则是 **1489 行 / 1123 code 行**。本报告以 1046 为首选估计值，
1489 作为上界。`live.rs` 顶部 `enum Stage`（`:46`）、`StopFollowup`（`:61`）、三个自由函数
（`:138`/`:157`/`:165`）以及 `:1719` 之后的自由函数、`:1864` 之后的测试辅助**不计入**。

### 4.4 专用测试文件与行数

| 模块 | 专用测试 | 行数 |
|---|---|---|
| `kernel_model_worker.rs` | **无同名外部测试文件**；仅内联：`#[cfg(test)] mod tests`（`:1307`–`1499`，193 行）+ `#[cfg(test)] mod model_failure_diagnostic_tests`（`:1503`–`1629`，127 行） | 320（内联） |
| `pi-kernel-worker.mjs` | `services/agent-runtime/test/pi-kernel-worker.test.mjs`；`services/agent-runtime/test/kernel-engine-retirement.test.mjs`（`import { ... } from '../src/pi-kernel-worker.mjs'`，`:9`） | 899 + 82 |
| `pi-kernel-loop.mjs` | `services/agent-runtime/test/kernel-tool-catalog.test.mjs`（`:4`）；`test/pi-kernel-host-tools.test.mjs`（`:8`）；`test/pi-kernel-work-tool-args.test.mjs`（`:11`）。**不存在** `pi-kernel-loop.test.mjs` | 23 + 73 + 88 |
| `kernel_coordinator/compaction.rs` | `apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/compaction_tests.rs`；另有 `f1_model_delivery_tests.rs:943-945`、`host_job_notice_tests.rs:196-197`、`steering_tests.rs:212` 直接调用其方法 | 713（专用）|
| `pi-kernel-compaction.mjs` | `services/agent-runtime/test/pi-kernel-compaction.test.mjs`（`:4`）；`test/host-runtime-capabilities.test.mjs:7` 只导入 `compactionFailureDiagnostic` | 56（+ 部分引用） |
| `kernel_coordinator/live.rs` | **无** `live_tests.rs`；相关行为测试分散在 `concurrency_tests.rs`、`delivery_live_tests.rs`、`steering_live_tests.rs`、`f3_approval_tests.rs`、`waiting_jobs_storage_tests.rs` 等（通过 `super::live::...` 引用） | 未单独成文件 |

补充：`kernel_model_worker.rs:52` 有 `#[path = "kernel_model_worker_job.rs"]` 引入
`kernel_model_worker_job.rs`（3238 字节，生产代码，不是测试文件）。

### 4.5 外部调用点（排除模块自身与其专用测试）

| 模块 | 生产调用点 | 数量 | 分布 |
|---|---|---|---|
| `kernel_model_worker` | 37 行 | **37** | `kernel_coordinator.rs` 11、`kernel_host.rs` 11、`mod.rs` 3（`:13` mod 声明 + `:2241` + `:2253`）、`kernel_coordinator/compaction.rs` 6、`kernel_coordinator/live.rs` 6 |
| `pi-kernel-worker.mjs` | 1 行 | **1** | `services/agent-runtime/src/pi-runtime.mjs:19` `import { createKernelWorker } from './pi-kernel-worker.mjs'` |
| `pi-kernel-loop.mjs` | 1 行（+2 处注释提及） | **1** | `pi-kernel-worker.mjs:9`（import）；注释提及：`pi-kernel-batch-resume.mjs:245`、`pi-runtime.mjs:962` |
| `kernel_coordinator/compaction.rs` | 14 行 | **14** | 方法调用 `ensure_context_with_worker` / `model_retry_context` / `context_view` / `context_fits` / `resume_context_compaction`：`kernel_coordinator.rs` 5、`kernel_coordinator/live.rs` 7、`kernel_host.rs` 1；另含 `kernel_coordinator.rs:18` `mod compaction;` 声明 |
| `pi-kernel-compaction.mjs` | 1 行 | **1** | `pi-kernel-worker.mjs:13`（import 4 个符号） |
| `kernel_coordinator/live.rs` | 9 行 | **9** | `kernel_host.rs:1408`（`live::dispatch_waits_for_approval`）、`:1556`（`live::LIVE_DETACHED`）；`kernel_coordinator.rs:19`（`pub(super) mod live;`）+ `:3085`/`:3172`/`:3188`/`:3427`/`:3513`/`:3529` |

含测试在内的总引用数（`rg -n` 计数）：`live::` 39 行、`kernel_model_worker` 62 行、
compaction 方法名 52 行。测试侧引用主要集中在
`compaction_tests.rs`、`f1_engine_path_tests.rs`(9)、`f3_approval_tests.rs`(3+8)、
`real_eval_tests.rs`(10)、`tests.rs`(2+3)、`concurrency_tests.rs`、`delivery_live_tests.rs`、
`steering_tests.rs`、`steering_live_tests.rs`、`waiting_jobs_storage_tests.rs`、
`host_job_notice_tests.rs`、`f1_model_delivery_tests.rs`、`tool_result_read_tests.rs`、
`p2b_consumer_h_repair_tests.rs`。

**结论**：六个模块合计 5156 行（4094 code 行），`live.rs` 若只算 loop-control（`632`–`1449` 与
`1490`–`1717`，规则见 4.3）则为 4163 行 / 3383 code 行；`kernel_model_worker.rs` 与
`live.rs` 都没有同名专用测试文件（前者内联 320 行测试，后者测试散落在多个 `*_tests.rs`），
JS 侧三个模块各有 1–3 个 `*.test.mjs`；生产外部调用点为 37 / 1 / 1 / 14 / 1 / 9。

---

## 未验证 / 不确定项

| 项 | 原因 |
|---|---|
| 上述行数与调用点是否随未提交改动而变化 | 审计基于当前工作树快照；未执行 `git status/diff`，未验证工作树是否 dirty |
| `kernel_model_worker.rs` 内联测试 320 行的「专用」边界 | 两个 `mod` 区间来自 `#[cfg(test)]` 行号与闭合花括号位置（`:1499`、`:1629`），未逐函数确认每个测试都只测该模块 |
| JS `additionalProperties: false` 是否与 Rust `deny_unknown_fields` 完全等价 | 只核对 `schema.json` 的 `RunControlBinding`(`:1831`) 与三个嵌套 def，未核对 `validateSchema` 实现对 `additionalProperties` 的具体报错文案（只确认 `validateWireValue` 返回 errors 数组，`index.mjs:2537-2540`） |
| 旧版本已安装二进制与新版共享同一 SQLite 的真实升级路径 | 需要真实升级场景，本次审计未运行构建或迁移，仅按代码推断 |
| `services/agent-runtime` 下是否存在未纳入 `rg` 的构建产物副本（`dist/`） | 未检查 `services/agent-runtime/dist`，其内容不影响源码行数结论 |

