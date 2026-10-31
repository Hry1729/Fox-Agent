> **现状（2026-10-31，B4c）**：本页是外部 Agent 的历史接入与实测记录，正文保持原样。现状：
> - 内置主线是 PiNative；外部 Pi、Codex、dsh 目前处于退役状态（旧会话只读，入口返回 `agent_backend_retired`），**计划在下一阶段（阶段三）恢复**为可选后端，现在不可用。
> - Fox 自身已不用沙箱：SRT、Windows ACL、网络代理、ACP stdio 桥已在 A1、B2 删除，运行时打包脚本不再打包 srt；命令在 Fox Job 中执行。下文凡写「在 srt 沙箱里运行」「共享 Host SRT」「网络审批」的内容都只是当时的记录。
> - 外部 Agent 恢复后，Codex、dsh 使用各自自带的原生沙箱和审批（属于它们自己的功能），外部 Pi 与 PiNative 共用 Fox 的四档审批引擎。
> 当前状态见[单 Pi 主线状态](../../../docs/01-项目概览/单Pi主线状态.md)与[安全架构](../../../docs/02-架构/安全架构.md)。
>
> 2026-10-28 的旧说明（保留）： **单 Pi 分支适用范围（2026-10-28）**：旧外部驱动、受管运行时或外层 SRT 的设计与实测快照。当前外部 Pi ACP、DeepSeek、Codex 已退休，历史只读，执行返回 agent_backend_retired；下文的安装、启用、审批和继续运行步骤不能用于当前产品路径。（当时共享 Host SRT 仍保留；现已删除。）

# CodexBackend（codex-acp）阶段 0 证据与能力声明

## 当前状态（2026-10-24）

> 本节是当前事实；下文第 1 节起是 2026-10-19 的阶段 0 证据原文，未改动。

- **已实现，暂时隐藏**。产品内置的 Agent 是 Fox、DeepSeek Harness（dsh）和 pi；Codex 以后与其他 ACP Agent 一起通过「自定义 ACP Agent」通用入口由用户自行安装接入（**待办 / 规划中**）。
- 隐藏开关集中在一处：`AgentBackend::HIDDEN`（`src/port.rs`），前端镜像 `HIDDEN_AGENT_BACKENDS`（`apps/desktop/src/features/conversations/model/agent-backend.ts`）。删掉这两处的 `codex` 即可恢复。
- 隐藏期间：设置 →「新会话使用的 Agent」、新会话 Agent 选择器、各 Agent 默认权限、供应商协议的“适用于”列表都不列出 Codex；新建 `codex` 会话被拒绝（`agent_backend_hidden`）；之前保存的默认 Agent 为 `codex` 时有效值回退 Fox。
- 已有的 Codex 会话照常打开、查看历史，并按原有路由继续运行（仍受 `FOX_EXTERNAL_BACKENDS` 约束），不会被换成其他后端。
- 代码与测试保留：`src/backends/codex.rs`、`tests/codex_backend.rs`；`capabilities_for`、原生权限（请求审批 / 帮我批准 / 完全访问权限）和 Responses 协议探测都在。
- 桌面 Host 从 Fox 管理的运行时 `<runtime>/backends/codex/` 启动 `codex-acp`（锁定 `2.2.2`），不经 `npx`；`npx` 入口只用于开发和真实二进制测试。

完整的路由、隐藏开关、权限和凭据说明见 `docs/02-架构/Agent运行时架构.md` 的「外层 Agent 后端（四 Agent）」一节；人工复测记录见本目录 `sandbox.md`。

分支：Codex（方案 v0.3.1 第 7 节）。代码：`src/backends/codex.rs`；测试：`tests/codex_backend.rs`。
记录日期：2026-10-19。结论只写有证据的部分；没有证据的写“待验证”。

## 1 版本与平台

| 项 | 值 |
|---|---|
| 适配器 | `@agentclientprotocol/codex-acp@2.2.2`（`launch_spec(AgentBackend::Codex)` = `npx -y @agentclientprotocol/codex-acp@2.2.2`）。注意：2.x 是 TypeScript 实现（`dist/index.js`），不是早期 Rust 版 |
| 实际解析到的 Codex CLI | `@openai/codex` 0.160.1（`codex-cli 0.160.1`，`codex-linux-arm64` musl 二进制）。适配器依赖写的是 `^0.160.1`，**npx 以后可能解析到更新的 CLI**，见缺口 G5 |
| 实测平台 | Linux aarch64，内核 6.12.110，Node 22.23.3，npm 10.9.9（容器环境） |
| 未测平台 | macOS、Windows（全部待验证） |
| Host 侧 ACP 客户端 | `agent-client-protocol` =3.4.0（`acp.rs` 的 `AcpConnection`） |

aarch64 Linux 可用：npm 安装了 `@openai/codex-linux-arm64` 可选依赖，不需要换版本。

证据代号：
- **M1**：真实启动 codex-acp 2.2.2，跑 initialize / authenticate / session/new / session/prompt（Node 原始 JSON-RPC 探针 + Rust `AcpConnection` 冒烟测试）。
- **M2**：同机直接运行内置 `codex sandbox`（codex-acp 每个 turn 选用的同一沙箱机制）。
- **S**：codex-acp 2.2.2 发布包源码 `dist/index.js` 与 README（npm 包内，对应 github.com/agentclientprotocol/codex-acp）。

## 2 实测记录（原样关键输出）

### 2.1 initialize（M1，Node 原始响应，客户端 fs 读写 false、terminal false）

```json
{"protocolVersion":1,"agentInfo":{"name":"@agentclientprotocol/codex-acp","title":"Codex","version":"2.2.2"},
 "agentCapabilities":{"auth":{"logout":{}},"providers":{},"loadSession":true,
  "promptCapabilities":{"embeddedContext":true,"image":true},
  "sessionCapabilities":{"resume":{},"list":{},"close":{},"delete":{},"fork":{},"additionalDirectories":{},"subagents":{}},
  "mcpCapabilities":{"acp":false,"http":true,"sse":false},"_meta":{"authStatus":{}}},
 "authMethods":[{"id":"api-key","name":"API Key","description":"Use an API key to authenticate","_meta":{"api-key":{"provider":"openai"}}},
                {"id":"chat-gpt","name":"ChatGPT","description":"Use ChatGPT to authenticate"}],
 "_meta":{"steering":{"supported":true}}}
```

经共享 Rust 客户端（3.4.0）反序列化再序列化后（冒烟测试 `INITIALIZE` 输出）：`sessionCapabilities` 只剩 `list/delete/additionalDirectories/resume/close`，**`fork`、`subagents` 被丢弃**（crate 不认识）；顶层 `_meta.steering` 保留。对能力判断无影响（Fox 不用 fork/subagents），但 Host 若要读未知扩展须注意。

### 2.2 session/new 与认证（M1）

| 条件 | 结果 |
|---|---|
| 无凭据 | `{"code":-32000,"message":"Authentication required"}` |
| 设 `OPENAI_API_KEY`（无效 key）、全新 `CODEX_HOME`、不调 authenticate | 同上 `Authentication required`（Rust 冒烟测试同样结果） |
| 先 `authenticate {"methodId":"api-key"}` | 收到 `_auth/status_update {"authStatus":{"kind":"api_key","label":"OpenAI API key"}}`，`result:{}`；**`$CODEX_HOME/auth.json` 被写入 `{"auth_mode":"apikey","OPENAI_API_KEY":"sk-…"}`** |
| 设 `DEFAULT_AUTH_REQUEST={"methodId":"api-key"}` | session/new 成功（同样会写 auth.json） |
| 认证后 session/new | 返回 sessionId、models、`modes.availableModes` = read-only / workspace-write / agent / agent-full-access，**`currentModeId":"agent"`**（默认自动审核模式）；`configOptions` 含 mode、collaboration_mode、model、reasoning_effort、fast-mode |
| `INITIAL_AGENT_MODE=read-only` | `currentModeId":"read-only"` |

### 2.3 session/prompt（M1，无效 key）

```
< session/update agent_message_chunk "Warning: Falling back from WebSockets to HTTPS transport. unexpected status 401 Unauthorized ... auth error code: invalid_api_key"
< session/update session_info_update _meta.codex.threadStatus {"type":"systemError"}
< {"id":4,"error":{"code":-32000,"message":"Authentication required"}}
```

失败以 JSON-RPC error 结束 prompt（`AcpConnection` 中表现为 `BackendEvent::Failed("prompt failed: ...")`），不是 `stopReason`。**没有任何成功的模型 turn**，以下凡需要真实 turn 的项目均未实测。

### 2.4 沙箱（M2，`codex sandbox`，同机）

| 步骤 | 预期 | 实际 |
|---|---|---|
| 默认（read-only）写 `/tmp/fox-outside.txt` | 拒绝 | `cannot create /tmp/fox-outside.txt: Read-only file system` |
| workspace-write 写 cwd 内文件 | 允许 | `WROTE_INSIDE` |
| workspace-write 写 `/var/tmp/...` | 拒绝 | `Read-only file system` |
| workspace-write 访问 `https://1.1.1.1`（去掉代理变量） | 拒绝 | `Failed to connect to 1.1.1.1 port 443 after 0 ms` |
| 对照：无沙箱 curl | 成功 | `200` |
| 对照：`sandbox_workspace_write.network_access=true` | 成功 | `200` |
| workspace-write 读 `/etc/hostname`（根目录外） | — | **读取成功**（`READ_OK`） |

### 2.5 进程树（M1，Rust 冒烟测试，Linux）

close 前 cwd 为会话目录的进程：`npm exec @agentclientprotocol/codex-acp@2.2.2`、`sh -c codex-acp`、`node .../codex-acp`、`node .../@openai/codex/bin/codex.js app-server`、`.../codex app-server`（5 个，同一进程组）。`AcpConnection::close()` 后：`[]`。

## 3 源码确认（S）

- **审批映射**：Codex app-server 的 `item/commandExecution/requestApproval`、`item/fileChange/requestApproval`、`item/permissions/requestApproval`、`mcpServer/elicitation/request`（含 `codex_approval_kind: mcp_tool_call`）都由 `CodexApprovalHandler` 转成 `session/request_permission`；选项为 `allow_once`（accept）、`allow_always`（acceptForSession，会话级）、`reject_once`，MCP 另有按 `_meta.persist` 出现的“本会话 / 永久允许”。客户端答 `cancelled` 或请求异常时一律回 Codex `cancel`（fail-closed）。**何时**发起审批由 Codex 的 approvalPolicy 决定，不是逐个工具调用。
- **模式 = 审批 + 沙箱预设**（`AgentMode`）：

| mode id | approvalPolicy | 审批由谁答 | sandbox | 网络 |
|---|---|---|---|---|
| `read-only` | on-request | user（→ Host） | readOnly | 关 |
| `workspace-write` | on-request | user（→ Host） | workspaceWrite（cwd + additionalDirectories 可写） | 关 |
| `agent`（**默认**） | on-request | **auto_review（Codex 自动审核，不到 Host）** | workspaceWrite | 关 |
| `agent-full-access` | never | — | dangerFullAccess | 开 |

  模式在每个 turn 的 `runTurn` 里随 `approvalPolicy/approvalsReviewer/sandboxPolicy` 下发，覆盖配置文件里的 sandbox 设置。配置途径：环境变量 `INITIAL_AGENT_MODE`、ACP `session/set_mode`、`session/set_config_option`（id `mode`）、`CODEX_CONFIG`（JSON 合并进 session config）。
- **`_session/steering`**：参数 `{sessionId, prompt: ContentBlock[]}`；每会话串行队列；结果 `outcome` = `injected`（并入当前 turn）/ `startedNewTurn`（当前无 turn，新开一个）/ `failed`。能力由 initialize 顶层 `_meta.steering.supported` 声明。
- **session/load / resume / close / list / delete / fork**：均已实现并在 initialize 声明；load 会回放历史，resume 不回放；close 先中断当前 turn。
- **MCP 透传**：支持 stdio（command）与 http；`session/new` 的 `mcpServers` 写入 session config 的 `mcp_servers`。**与用户 Codex 配置同名的服务器会被跳过**（`skippedMcpServers`，可用 `DISABLE_MCP_CONFIG_FILTERING=true` 关闭过滤）。
- **取消**：`session/cancel` → 标记客户端取消 → app-server `turn/interrupt`；Codex 报告 turn `interrupted` 后 prompt 返回 `stopReason:"cancelled"`。但 turn 已完成而客户端已 abort 时也返回 `cancelled`，二者不可区分。
- **进程**：codex-acp 以普通子进程启动 `codex app-server`（继承 codex-acp 的环境，即 Host 白名单环境），stdin 关闭时 supervisor 关闭 app-server。
- **项目信任**：codex-acp 给 cwd 及附加目录设 `projects.<root>.trust_level = "trusted"`，项目内 `.codex/config.toml` 会被加载（第 6.5 节第 4 层）。

## 4 能力声明（`capabilities_for`，Host 只用 read-only / workspace-write）

### 4.1 四维

| 维度 | read-only（Linux） | workspace-write（Linux） | macOS / Windows | 证据 |
|---|---|---|---|---|
| 审批-文件读 | 无审批 | 无审批 | 同左 | M2 读根外成功；S 无读审批路径 |
| 审批-文件写 | **待验证**（预期越界补丁逐次问，未跑真实 turn） | 无审批（根内直接写） | 同左 | S |
| 审批-shell | 无审批（on-request：沙箱内命令不问，仅模型主动申请提权时问） | 同左 | 同左 | S |
| 审批-网络 | 无审批（本地网络由沙箱关；模型侧 web search 不经 Host） | 同左 | 同左 | S |
| 审批-MCP 业务工具 | 待验证（Codex 会以 elicitation 问，何时问未测；Fox 业务工具由 Host 自己的 MCP 服务器逐次把关） | 同左 | 同左 | S |
| 沙箱-文件 | 强制（**仅限写**；读不限于授权根） | 强制（cwd 可写，其余只读；读不限） | **待验证**（macOS Seatbelt 未跑；Windows 沙箱较弱/实验，未跑） | M2 |
| 沙箱-网络 | 强制（本地进程出站被拒） | 强制 | 待验证 | M2 |
| 取消-turn | 仅请求（`TurnRequested`） | 同左 | 同左 | S；未跑真实取消 |
| 取消-进程树 | 是（进程组 kill，5→0） | 同左 | Windows 由 Host Job Object，待验证 | M1 |
| 审批答复确认 | 无（ACP 无回执，停在 `response_written`） | 同左 | 同左 | 协议 |
| 可观测性 | 工具开始/结束（shell、文件改动、MCP、web search 以 tool_call / tool_call_update 上报）；命令内部子进程不可见 | 同左 | 同左 | S（README + reporters），未见真实事件 |

`agent` / `agent-full-access` 两个模式所有审批类别都声明为“无审批”（自动审核或不审批），full-access 沙箱为 Absent。`mode_for_policy` 永不选这两个模式。

### 4.2 会话能力

| 能力 | 声明 | 来源 |
|---|---|---|
| load（回放） | 支持 | initialize `loadSession:true`（M1） |
| resume（不回放） | 支持 | `sessionCapabilities.resume`（M1） |
| close | 支持 | M1 |
| list | 支持 | M1 |
| steer | 支持（`_session/steering`） | `_meta.steering.supported:true`（M1）+ S |
| fork / delete / subagents | 声明了，但 Fox 隐藏（7.3“fork 等能力先隐藏”） | M1 |

上述会话能力都只来自声明，**均未实际调用**（需要已认证会话）。T08 前须实测。

### 4.3 Host 策略映射（`codex.rs`）

- `mode_for_policy`：只读 → `read-only`；可写 → `workspace-write`；要求网络 → 拒绝（codex-acp 只有 full-access 才开网络，会同时去掉沙箱和审批，不能放宽）。
- `session_env`：`INITIAL_AGENT_MODE=<mode>`、`NO_BROWSER=1`、`CODEX_CONFIG={"web_search":"disabled"}`（后者只是请求，未验证生效）。
- `verify_session_mode`：读 session/new|load|resume 响应的 `modes.currentModeId`（或 `configOptions.mode`），不等于预期即拒绝（fail-closed）；没有报告模式也拒绝。
- `set_mode_params`：策略收紧时对已有会话下发 `session/set_mode`（6.5“收紧立即生效”）。
- `check_initialize`：要求 protocolVersion 1、agentInfo 为 codex-acp 2.2.2，否则拒绝启动（7.3）。
- `steering_params`：构造 `_session/steering` 参数；结果须按 injected / startedNewTurn / failed 如实展示。

## 5 已知缺口

| # | 缺口 | 影响 | 处理 |
|---|---|---|---|
| G1 | codex-acp 默认模式是 `agent`（自动审核），审批不到 Host | 不注入就等于放弃 Host 审批 | Host 必须注入 `INITIAL_AGENT_MODE` 并在建会话后 `verify_session_mode`；**需公共层改动**（CR-1、CR-2） |
| G2 | 文件**读**不受沙箱限制（整盘可读，含 `~/.ssh` 等），也无读审批 | 授权根只约束写，不约束读 | 如实声明；UI 不得显示“只能读授权目录”；Codex 有 readable-root 相关选项（`--sandbox-state-readable-root`），是否能经 codex-acp 配置待查 |
| G3 | 没有“沙箱内开网络”的模式 | 需要网络的会话无法安全开启 | 策略要求网络时拒绝；如需开放，评审 `CODEX_CONFIG` 的 `sandbox_workspace_write.network_access`（是否被每 turn 的 sandboxPolicy 覆盖待验证） |
| G4 | 模型侧 web search 在 OpenAI 服务端执行，本地网络沙箱管不到，属数据外发 | 网络维度不能只看沙箱 | 已请求 `web_search:"disabled"`，生效与否待真实 turn 验证；网络审批声明为“无审批” |
| G5 | `@openai/codex` 依赖为 `^0.160.1`，未锁死 | 同一适配器版本可能带不同 CLI，能力漂移 | Host 用 `CODEX_PATH` 指向锁定版本的 codex，或启动后核对 CLI 版本（当前 `check_initialize` 只核适配器版本） |
| G6 | `authenticate(api-key)` 把 key 写入 `$CODEX_HOME/auth.json`；仅设 `OPENAI_API_KEY` 不够 | 若 Host 代为认证，就是复制凭据到原生存储并可能覆盖用户的 ChatGPT 登录（违反 6.5） | Host 不调用 authenticate、不设 `DEFAULT_AUTH_REQUEST`；依赖用户自己 `codex login`，只读 `_meta.authStatus` / `_auth/status_update` 显示状态。`launch.rs` 中 `credential_env` 透传的 `OPENAI_API_KEY` 实际无效，建议评审是否移除（CR-4） |
| G7 | 与用户 Codex 配置同名的 MCP 服务器被静默跳过 | Host 业务工具可能被用户配置“顶替” | 用唯一名 `HOST_MCP_SERVER_NAME`，并检查响应里 `skippedMcpServers` 为空（字段位置待实测） |
| G8 | 取消确认不可与“已完成后 abort”区分 | 只能声明 `TurnRequested` | T06 实测后再评估是否升为 `TurnConfirmed` |
| G9 | 项目信任设为 trusted，项目 `.codex/config.toml`、hooks 会生效 | 第 4 层可能尝试放宽（如加 MCP、hooks） | 待验证每 turn 的 mode 是否压过项目配置；hooks 需单独评审 |
| G10 | codex-acp 在 `CODEX_HOME`（默认 `~/.codex`）写会话、日志、sqlite 状态 | 原生会话数据由 Codex 管理 | 符合“原生会话不写成 Kernel checkpoint”；Host 不读写这些文件 |
| G11 | Windows / macOS 完全未测 | T14 无法过 | 见第 6 节 |
| G12 | “ultra” 推理档会自动派生子代理；子代理审批 root 路由 | 审批与取消范围扩大 | Fox 先禁用 ultra 或隐藏，待验证 |

### 5.1 给公共层的变更请求（7.1：不在本分支改公共文件）

- **CR-1** `LaunchSpec`/`AcpConnection::spawn` 支持 Host 显式注入的会话变量（6.5 允许），用于 `INITIAL_AGENT_MODE`、`NO_BROWSER`、`CODEX_CONFIG`。实例：不注入时 `currentModeId":"agent"`。
- **CR-2** `AcpConnection::new_session` 返回完整响应（modes、configOptions、`_meta`），并支持传 `mcpServers`；另加 `set_mode` / `set_config_option` 与通用扩展请求（`_session/steering`）。否则无法 fail-closed 校验模式，也无法接 Host MCP 与 steer。
- **CR-3** `AcpConnection` 目前把 stderr 丢到 null；codex-acp 的失败原因（如 `[SYSTEM_ERROR] Prompt ... failed: RequestError: Authentication required`）只在 stderr，建议脱敏后进诊断日志。
- **CR-4** 认证状态读取接口（`_meta.authStatus`、`_auth/status_update` 通知目前作为未知通知被丢弃）；并评审 `credential_env` 里的 `OPENAI_API_KEY`（见 G6）。

## 6 第 8 节发布门槛状态（Codex）

| 用例 | 状态 | 已有 | 还差 |
|---|---|---|---|
| T04 越权阻断 | **未通过** | Linux 上 `codex sandbox` 直接实测：根外写、出站网络被拒（M2） | 经真实 ACP turn 复测；读越权无法阻断（G2，须在声明与 UI 中体现）；web search（G4）；macOS、Windows |
| T05 审批 | **未通过** | 共享客户端对 mock agent：答复只记 `Written`、重复答复被拒、取消时挂起审批答 `cancelled`；源码确认 Codex 审批走 `request_permission` 且异常时 fail-closed | 真实 Codex 审批往返（允许、拒绝、重复、失效）；需要凭据或本地假 Responses 服务（见第 7 节） |
| T06 取消/竞争/结果未知 | **未通过** | 源码路径清楚；公共 settlement 单测 | 真实 turn 中途取消、取消与审批/完成竞争 |
| T07 断线/崩溃/代次 | **部分** | Linux 进程树关闭实测 5→0；公共层代次单测 | 杀 app-server / codex-acp 后的事件与审批卡失效、重启后 load/resume |
| T10 迟到/重复/多会话/并发 | **未通过（Codex 专项未做）** | 公共层单测 | 一会话一进程下的多会话隔离取消 |
| T12 旧库/映射/回滚 | **不在本分支实测范围** | 公共 legacy 单测（旧 `engine_id=codex` 映射到 fox） | 随 Host 集成一起测 |
| T14 Windows | **未通过（未开始）** | — | `npx.cmd` 启动、路径与编码、Job Object 清理、Windows 沙箱实际范围 |
| T08 会话恢复 | 声明支持，未调用 | initialize 声明 | 认证后实测 load/resume；不通过则显示“不支持” |

## 7 复现步骤

```bash
source /workspace/work/tools/env.sh
cd /workspace/work/fox
cargo test -p fox-agent-backend --features acp                         # 冒烟测试打印 SKIPPED（跳过不算通过）
FOX_TEST_REAL_ACP=1 cargo test -p fox-agent-backend --features acp \
  --test codex_backend real_ -- --nocapture                           # 真实 codex-acp
```

建议下一步：不依赖真实 OpenAI 凭据跑通 T04/T05/T06——用 `CODEX_CONFIG` 把 model provider 指向本地假的 Responses API（脚本化返回 shell / apply_patch 工具调用），即可在真实 codex-acp + Codex 二进制链路上触发审批、沙箱拒绝与取消。认证仍须走 Codex 自己的登录存储（使用独立临时 `CODEX_HOME`），不得写入用户的 `~/.codex`。
