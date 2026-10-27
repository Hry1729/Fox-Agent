# PiBackend（pi-acp）阶段 0 实测与接入说明

运行时 0.1.3 的模型授权、代理和错误结算以 [受管模型连接](model-connectivity.md) 为准。下文保留早期 stock pi-acp 的测量记录。

## 当前状态（2026-10-27）

> 本节是当前事实；下文是 2026-10-19 的阶段 0 实测原文，未改动。

- pi 是**内置 Agent** 之一（另两个是 Fox 和 DeepSeek Harness；Codex 已实现但暂时隐藏）。四 Agent 改造已完成并合入 `main`。
- 桌面 Host 从 Fox 管理的运行时 `<runtime>/backends/pi/` 启动 `pi-acp`（锁定 `0.0.34`），pi 本体为 `1.4.0`；不经 `npx`（`npx` 入口只用于开发和真实二进制测试）。运行时带 `<runtime>/pi-gate/` 时用 Fox 审批拦截扩展启动 pi（门控），否则按原样启动（没有 Host 审批）。见 `src/external_launch.rs`。
- 默认在 srt 沙箱里运行；进程只继承白名单环境变量；登录信息放在 Fox 管理的私有 HOME。
- 权限按 pi 的原生选项显示（`src/native_permission.rs`）：只读、询问（默认）、自动、完全访问。由 Fox 作答，切换后从下一次工具调用起生效。完全访问是独立开关，只在创建会话时授予，会话中只能撤销。
- 模型默认来自「模型服务」供应商：pi 可用 OpenAI Chat Completions、Anthropic Messages、OpenAI Responses 三种协议；API Key 按 base URL 在系统凭据库全局共享。

完整的路由、隐藏开关、权限和凭据说明见 `docs/02-架构/Agent运行时架构.md` 的「外层 Agent 后端（四 Agent）」一节；人工复测记录见本目录 `sandbox.md`。

### Pi 1.4.0 升级核验（2026-10-27）

- npm `latest` 与上游 `v1.4.0` 发布一致。三个 Pi SDK 包和受管原生 Pi 同步锁定 `1.4.0`，`pi-acp` 保持 `0.0.34`。Windows 运行时包版本为 `0.1.6`。
- Pi 1.4 将系统提示和工具声明放入消息记录，并从 `SessionManager` 重建请求。Fox 通过 Adapter 写入历史、补充要求、任务通知和审查提示；Kernel 保留 Host 冻结的工具集，供应商请求仍使用 Host 的严格参数 schema。
- CLI 改用 `dist/bundle/cli.js`。模型代理补丁同时覆盖 SDK dispatcher 和 CLI bundle chunk，分别校验上游哈希及补丁后哈希。
- 定向源码测试覆盖 Kernel、Legacy、历史恢复、通知跨轮保留、供应商协议、取消和工具审批。编译后 Runtime 的 5 项 HTTP 检查通过，另有侧车冒烟通过。未提供 Codex 外部二进制的检查保持跳过。
- 实际 Windows srt 0.0.81 + Pi 1.4.0 + pi-acp 0.0.34 已验证模型选择、正常输出、401 明确失败、空网络允许列表失败且服务收到零请求。模型为本机脚本服务，不使用真实供应商凭据。
- 原生工具批准、拒绝、取消、取消后续聊在隔离 HOME 下通过。补充的沙箱审批检查因全局 `.acl-init.lock` 拒绝访问而未进入 Pi；该项未计为沙箱审批通过。此升级不代表沙箱整体隔离问题已解决，也不代表桌面或真实供应商验收。
- 旧会话升级在隔离 HOME 下通过：先用真实 Pi 0.84.2 创建对话并结束进程，再用 Pi 1.4.0 的 ACP `session/load` 载入。后续 HTTP 请求保留旧用户消息、旧回答和新问题。

分支：pi（阶段 3）｜实测日期：2026-10-19｜实测平台：Linux x86_64（沙箱），Node v22.23.3｜Windows / macOS：未实测

相关文件：

| 文件 | 作用 |
|---|---|
| `src/backends/pi.rs` | 能力声明 `capabilities()`（stock）/ `capabilities_with_approval_gate()`（门控原型）、`check_initialize()` 版本校验、`parse_tool_approval()` 解码门控审批、`classify_failure()` |
| `tests/pi_backend.rs` | 单元测试（录制的 initialize）+ 真实二进制测试（`FOX_TEST_REAL_ACP=1` 门控，未设置时为“跳过”，不算通过） |
| `fixtures/pi/initialize-pi-acp-0.0.34.json` | 真实 `npx -y pi-acp@0.0.34` 的 initialize 原始响应 |
| `fixtures/pi/fox-approval-gate.ts` | pi 扩展：每次工具调用经 ACP `request_permission` 审批（原型） |
| `fixtures/pi/pi-gated.sh` | 作为 `PI_ACP_PI_COMMAND`：以 `--no-extensions -e fox-approval-gate.ts` 启动锁定的 pi |
| `fixtures/pi/launch-pi-acp.mjs` | 测试用启动器：隔离 HOME，注入 `PI_ACP_PI_COMMAND` / `FOX_PI_BIN` 后运行真实 `npx -y pi-acp@0.0.34` |
| `fixtures/pi/scripted-provider.ts` | **仅测试**：pi 进程内的 OpenAI 兼容脚本模型，免凭据驱动真实 pi 走工具调用；pi / pi-acp 代码均未打桩 |

运行：

```bash
cargo test -p fox-agent-backend --features acp                       # 单元测试；real_* 打印 SKIPPED
FOX_TEST_REAL_ACP=1 cargo test -p fox-agent-backend --features acp --test pi_backend -- --nocapture
# 可选 FOX_PI_BIN=<pi 路径>，默认 services/agent-runtime/node_modules/.bin/pi（0.84.2）
```

## 1 版本与依赖（实测）

| 项 | 结论 | 证据 |
|---|---|---|
| pi-acp 版本 | 0.0.34（npm latest，2026-09-25 发布），MIT | `npm view pi-acp@0.0.34`；initialize `agentInfo.version` |
| pi-acp 依赖 | `@agentclientprotocol/sdk ^0.26.0`、`cross-spawn`、`zod`；**不依赖 pi 包** | `npm view ... dependencies` |
| 如何找到 pi | `PI_ACP_PI_COMMAND` 环境变量，否则 PATH 上的 `pi`（Windows `pi.cmd`，经 cross-spawn）；启动参数固定为 `pi --mode rpc --no-themes [--session <file>]` | dist/index.js `getPiCommand` / `PiRpcProcess.spawn` |
| pi 最低版本 | README：pi v0.81.0+，Node 22+ | pi-acp README |
| 与 Fox 锁定的 pi 0.84.2 兼容性 | **兼容**：session/new、prompt、工具、扩展 UI、abort、`agent_settled` 全部跑通 | `real_gated_*` 测试 |
| pi 子进程环境 | pi-acp 以 `env: process.env` 启动 pi，即继承 Fox 传给 pi-acp 的（白名单）环境 | `PiRpcProcess.spawn` |
| 版本横幅副作用 | 每次 session/new 运行裸 `pi --version`（忽略 PI_ACP_PI_COMMAND），并可能 `npm view @earendil-works/pi-coding-agent version`（联网，800ms 超时） | `buildStartupInfo` / `buildUpdateNotice` |

## 2 initialize / session/new / prompt 实测

initialize 原始响应（经 `AcpConnection`，`launch_spec(AgentBackend::Pi)`）：

```json
{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true,"audio":false,"embeddedContext":false},"mcpCapabilities":{"http":false,"sse":false},"sessionCapabilities":{"list":{},"delete":{}},"auth":{}},"authMethods":[{"type":"terminal","id":"pi_terminal_login","name":"Launch pi in the terminal","description":"Start pi in an interactive terminal to configure API keys or login","args":["--terminal-login"]}],"agentInfo":{"name":"pi-acp","title":"pi ACP adapter","version":"0.0.34"}}
```

（`auth:{}` 为 Rust 侧序列化补出的默认字段；裸 stdio 录制见 fixtures 文件。）

| 步骤 | 预期 | 实际 |
|---|---|---|
| initialize（无论 pi 是否存在） | 成功 | 成功；pi-acp 在 initialize 时不启动 pi |
| session/new，PATH 上无 pi | 明确失败 | `-32603 Internal error: Could not start pi: executable not found (command: pi)... data:{code:"ENOENT"}` → `PiFailure::PiNotFound` |
| session/new，pi 0.84.2 在 PATH、无模型凭据 | 明确失败 | `-32000 Authentication required: Configure an API key or log in with an OAuth provider.`，附 `authMethods` → `PiFailure::AuthRequired`。**失败发生在 session/new，而非 prompt**（pi-acp 在 session/new 调 `get_available_models`，0 个模型即报 authRequired） |
| prompt（无凭据） | — | 无法到达：没有会话。未伪造成功 |
| 用脚本模型（测试专用）session/new + prompt | 成功 | 成功，stopReason `end_turn` |

注意：stock 启动使用真实 HOME，pi 会创建 `~/.pi/agent/...`，pi-acp 写 `~/.pi/pi-acp/session-map.json`。

## 3 源码确认（pi-acp 0.0.34 dist/index.js）

| 问题 | 结论 |
|---|---|
| 工具执行是否映射到 `request_permission` | **否。** `tool_execution_start/update/end` 只映射为 `tool_call` / `tool_call_update`。`conn.requestPermission` 仅在扩展 UI `confirm`/`select` 时调用（`requestExtensionPermission`）。stock pi-acp 下 read/write/edit/bash 不经 Host 审批直接执行 |
| 扩展 UI 映射 | `confirm` → 选项 `yes`(allow_once) / `no`(reject_once)；`select` → `choice-<i>`；`input`/`editor`/未知 → 直接回 cancelled；客户端答 `cancelled` 或请求异常 → 回 pi `cancelled` |
| MCP | **未接到 pi**：`mcpServers` 只存入会话状态；`mcpCapabilities {http:false, sse:false}`。Host 的 MCP 业务工具对 pi 不可达 |
| session/load | 声明 `loadSession:true`；按 session-map 找 pi 会话文件，重启 pi `--session <file>`，再用 `get_messages` 回放 user/assistant 消息。**仅源码确认**，未用真实模型跑回放（`AcpConnection` 暂无 load 方法） |
| session/list | 声明 `list:{}`（pi-acp 注释：UNSTABLE），读 `~/.pi/agent/sessions`；未实测 |
| resume / close | 不声明。另声明 `delete`（端口未建模） |
| 单进程多会话 | session/new、load 后调用 `closeAllExcept`：**一个 pi-acp 进程只保留一个活动会话**，与 6.1“一会话一进程”一致，不可共享进程 |
| 取消 | `session/cancel` → 清空 pi-acp 客户端排队的 prompt（各自返回 cancelled）→ pi RPC `abort`；当前 prompt 在 pi 发出 `agent_settled` 后以 `cancelled` 结束 |
| 完成判定 | pi-acp 以 pi 的 `agent_settled`（非 `agent_end`）结算 turn，符合 7.3 要求 |
| steer | ACP 无 steer；pi-acp 未暴露 pi `steer`。turn 运行中再发 prompt → pi-acp **客户端排队为新 turn**（“Queued message (position n)”），不是 steer。`/steering`、`/follow-up` 只设置 pi 的模式 |
| 进程退出 | stdin 关闭 / SIGTERM → `dispose` → SIGTERM pi |

## 4 审批方案结论

**结论：pi-acp 本身没有工具审批映射；用 pi 扩展可以做到逐次审批，且无需 fork pi-acp。已做出可工作原型并用真实 pi-acp 0.0.34 + pi 0.84.2 端到端验证。**

链路：

```
pi tool_call 钩子（工具执行前，可阻断）
  → fox-approval-gate.ts: ctx.ui.confirm("Fox approval: <tool>", {"fox":"tool-approval",toolCallId,toolName,input,cwd}, {signal})
  → pi RPC extension_ui_request{method:"confirm"}
  → pi-acp → ACP session/request_permission（选项仅 yes/no）
  → Fox 答复 → extension_ui_response{confirmed} → 放行或 {block:true}
```

- fail-closed：只有显式 `confirmed:true` 放行；拒绝、ACP `cancelled`、turn abort（`ctx.signal` 使对话框解析为 false）、pi-acp 异常、扩展异常一律阻断。
- 加载方式：`PI_ACP_PI_COMMAND=fixtures/pi/pi-gated.sh`，后者 `exec $FOX_PI_BIN --no-extensions -e fox-approval-gate.ts "$@"`。理由：RPC 模式下项目 `.pi/extensions` 在未信任时不加载；Fox 不得写用户全局 `~/.pi/agent/extensions`（6.5）；`--no-extensions` 防止其他扩展干扰。同时解决“用哪个 pi”：`FOX_PI_BIN` 指向锁定的 0.84.2。
- Host 识别：`parse_tool_approval()` 只接受 title 前缀 + JSON 标记 + 原生 yes/no 选项三者齐全的请求；其他扩展的对话框不当作工具审批。`dialog_tool_call_id`（`pi-ui-<uuid>`）≠ pi 的 `toolCallId`，关联靠 JSON 里的 `toolCallId`。

实测（`real_gated_allow_reject_cancel_and_process_tree`）：

| 步骤 | 预期 | 实际 |
|---|---|---|
| bash 调用 → 审批前 | 未执行 | 收到 request_permission；文件不存在 ✅ |
| 允许 | 执行 | `fox-proof-allow1.txt` = allow1；状态 in_progress→completed；stopReason end_turn ✅ |
| 重复答复 | 拒绝 | 第二次 `answer_permission` 返回错误 ✅（答复结果为 `Written`，从不标为已确认） |
| 拒绝 | 阻断无副作用 | 状态 failed，文件不存在，turn 正常 end_turn ✅ |
| write 工具 | 逐次审批 | 审批前文件不存在，允许后写入 ✅ |
| read 工具被拒 | 不返回内容 | 状态 failed，内容为 “Rejected by Fox approval…” ✅ |
| 审批挂起时 session/cancel | 挂起请求结算为 cancelled，工具不执行 | `cancel()` 结算 1 个；stopReason cancelled；文件不存在 ✅ |
| 已批准的 bash 运行中 cancel | turn 停止、命令被杀 | stopReason cancelled；`sleep` 进程消失 ✅ |
| Host `close()` 时已批准 bash 运行中 | 进程树清理 | 第四轮（2026-10-19）：SIGTERM → 宽限 → KILL 下 pi 自己结束 detached bash；即使 pi 被 SIGSTOP、无法清理，Fox 的后代跟踪也按 pid 杀掉 bash 子树；整棵树结束且 `StopConfirmation::Confirmed` ✅（G3 已关闭，Linux 实测） |

审批前状态（实测）：pi 先发 `tool_execution_start` 再跑 `tool_call` 钩子，所以 pi-acp 在**等待审批时已报 `in_progress`**。Host 映射层必须把有挂起门控请求的 toolCallId 显示为“等待审批”。

上游/替代：若要去掉 wrapper，可向 pi-acp 上游提议“工具执行前 request_permission”选项（其扩展 UI 映射已具备，改动小）。不建议 fork。

退路评估（只评估不实现）：pi 原生 `pi --mode rpc` 直连。优点：有原生 `steer`/`follow_up`/`abort`/`abort_bash`、`agent_settled`、`extension_ui_request`，可直接实现 steer，且不必经 pi-acp 的排队语义；同一扩展门控可原样复用（实测直连 RPC 下门控也工作，见 §5 手工测量）。缺点：需自研 RPC 协议对接（违背 6.7 复用优先），会话持久化/回放要自己做。建议：保持 ACP 路线；仅当 steer 成为发布要求或 pi-acp 停更时启用该退路。

## 5 阶段 0 能力表

版本：pi-acp 0.0.34 + pi 0.84.2；平台：Linux x86_64；Windows/macOS 全部待验证。

| 维度 / 能力 | stock pi-acp | 门控原型 | 证据 |
|---|---|---|---|
| 审批：文件读 | None（不经审批） | **PerCall**（实测拒绝） | 源码 §3；`real_gated_*` 3b |
| 审批：文件写 | None | **PerCall**（实测 write；edit 同钩子未单测） | `real_gated_*` 3 |
| 审批：shell | None | **PerCall**（只批准发起命令） | `real_gated_*` 1/2/4 |
| 审批：网络 | None（pi 无网络工具，网络在 bash 内） | None | pi 工具清单 |
| 审批：MCP 业务工具 | Unverified（未接到 pi，不可达） | Unverified | README Limitations；`mcpCapabilities` |
| 沙箱：文件系统 | Absent | Absent | pi docs/security.md“no built-in sandbox”；无 fs/* 委托 |
| 沙箱：网络 | Absent | Absent | 同上 |
| 取消 turn | TurnConfirmed（prompt 在 `agent_settled` 后以 cancelled 返回） | 同左 | `real_gated_*` 4/5（用脚本模型；真实远端模型流式中取消未测） |
| 进程树终止（Fox） | **false**（Unix 上 bash `detached:true` 自成进程组，SIGKILL pi-acp 组不能波及） | false | `real_gated_*` 6；pi dist/core/tools/bash.js |
| 审批答复回执 | 无（停在 response_written） | 无 | ACP v1 / pi extension_ui_response 均无确认 |
| 可观测性 | ToolLifecycle；bash 子进程内部行为**不可观测**（仅 stdout/stderr 文本） | 同左；等待审批时显示 in_progress 需 Host 纠正 | 实测 tool_call / tool_call_update |
| 会话：load | 声明 true；回放仅源码确认 | 同 | initialize；源码 |
| 会话：resume / close | 不支持 | 同 | initialize |
| 会话：list | 声明（UNSTABLE），未实测 | 同 | initialize |
| steer | 不支持（排队为新 turn） | 同 | 源码 §3 |
| 单进程多会话 | 否（closeAllExcept） | 同 | 源码 |

补充手工测量（直连 `pi-gated.sh --mode rpc`，非 pi-acp）：已批准 `sleep` 运行时对 pi 发 SIGTERM → pi 退出（143）且 bash 进程组被清理。说明 TERM 优先、宽限后再 KILL 可关闭 G3。

## 6 已知缺口

| 编号 | 缺口 | 影响 | 处理 |
|---|---|---|---|
| G1 | stock pi-acp 工具不经审批 | 不能以 stock 形态发布 | 用门控原型；需要 G2 |
| G2 | `PI_ACP_PI_COMMAND`、`FOX_PI_BIN` 不在 `launch.rs` 白名单，`LaunchSpec` 也没有“Host 注入的会话变量”字段 | 生产无法接上门控 / 锁定 pi | 向公共层提变更请求：PiBackend 需要 Host 显式注入这两个变量（6.5“白名单 + Host 显式注入”）。本分支未改 launch.rs |
| G3（已关闭，第四轮） | 原：`acp.rs::kill_tree` 直接 SIGKILL 进程组，pi 的 bash 自成进程组成为孤儿 | — | 已实现两者：先 SIGTERM 宽限再 KILL；并在停止前快照后代、按 pid 杀掉离开进程组的后代，确认整棵树结束（`src/proctree.rs`）。Linux 实测通过；macOS（ps 快照）与 Windows（Job Object）待测 |
| G4 | `credential_env` 为空：pi 读取的提供商环境变量（如 `ANTHROPIC_API_KEY`）被白名单丢弃 | 只能用 pi 自己的 `~/.pi/agent/auth.json` 登录 | 符合 7.5“后端自管登录”；若需环境变量登录，按提供商列名单再提变更 |
| G5 | MCP 未接到 pi | Host 业务工具不可用 | 选项：上游 pi-acp 接线；或经 pi 扩展（如 pi-mcp-adapter / 自写扩展）把 Host MCP 窄接口注册为 pi 工具（同样会经门控）。未做 |
| G6 | 等待审批时工具状态为 in_progress | UI 误导 | Host 映射层按 `parse_tool_approval` 覆盖显示 |
| G7 | steer 不支持 | 运行中新输入只能排队或取消重发 | 按 6.7 降级并提示；退路为原生 RPC |
| G8 | session/load 回放、list 未实测 | T08 | `AcpConnection` 增加 load 后补测 |
| G9 | 每次 session/new 运行裸 `pi --version`（可能不是锁定的 pi）及 `npm view` 联网 | 横幅版本可能错；离线环境多 800ms | 可通过 pi `quietStartup` 设置减少；已在启动器把锁定 pi 目录前置到 PATH |
| G10 | `pi-gated.sh` 仅 Unix；Windows 需 `.cmd` | T14 | 未写未测 |
| G11 | 门控扩展运行在 pi 进程内：pi 被攻破或 `--no-extensions` 被绕过时无 Host 侧强制 | 审批是“协作式”而非沙箱 | 已在能力表把沙箱标 Absent；不与沙箱混称 |
| G12 | pi-acp 为 MVP，作者声明“expect breaking changes” | 升级风险 | `check_initialize` 锁定 0.0.34，其他版本拒绝启动 |

## 7 第 8 节发布门槛现状（pi）

| 用例 | 现状 |
|---|---|
| T04 越权阻断 | **未通过**。沙箱 Absent：路径/网络无任何强制；门控只能让用户逐次拒绝，且不约束已批准命令内部行为。需 Host 层沙箱（或按声明只读/受限目录）后才能验证 |
| T05 审批允许/拒绝/重复/失效 | **部分通过（原型、Linux）**：允许、拒绝、重复答复被拒、取消使挂起请求失效均实测通过。未测：超时失效、连接代次变化后旧卡、Host 持久化（属公共层） |
| T06 取消与竞争、结果未知 | **部分**：审批挂起时取消、命令运行中取消均通过；完成/取消竞争、结果未知未测；Host 关闭后进程树清理（G3）第四轮已通过（Linux） |
| T07 断线/崩溃/重启/代次 | **未测** |
| T10 迟到/重复事件、多会话、并发 | **未测**（依赖公共层 EventIntake / limits；pi 侧确认一进程一会话） |
| T12 旧库升级/映射/回滚 | 不涉及 pi 分支代码（公共层 legacy.rs）；pi 侧未测 |
| T14 Windows | **未测**：pi.cmd/cross-spawn、`pi-gated.cmd`、Job Object 均未验证 |

结论：pi 可用 ACP 路线达到逐次审批（原型已证），但当前**不满足发布门槛**；阻塞项为 G2（启动注入）、T04（无沙箱）、T14（Windows）；G3（进程树）第四轮已在 Linux 关闭。
