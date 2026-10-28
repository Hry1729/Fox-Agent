> **单 Pi 分支适用范围（2026-10-28）**：旧外部驱动、受管运行时或外层 SRT 的设计与实测快照。当前外部 Pi ACP、DeepSeek、Codex 已退休，历史只读，执行返回 agent_backend_retired；下文的安装、启用、审批和继续运行步骤不能用于当前产品路径。共享 Host SRT 保留，但本页旧 ACP 链路结果不能自动作为其验收证明。
> 当前状态与证据边界见[单 Pi 主线状态](../../../docs/01-项目概览/单Pi主线状态.md)。以下历史正文保持原样。

# DeepSeekBackend（实验）— 阶段 0 实测与接入说明

## 当前状态（2026-10-24）

> 本节是当前事实；下文是 2026-10-19 的阶段 0 实测原文，未改动。下文“T04 不满足 → 不得默认开启、不对用户开放”是阶段 0 的结论，之后 Host 在 dsh 进程外加了 srt 沙箱（见 `sandbox.md` 第 13 节），dsh 作为实验后端内置。

- DeepSeek Harness（dsh）是**内置 Agent** 之一，标记为实验（`AgentBackend::experimental`）。另三个内置 Agent 是 Fox、pi 和 Codex。四 Agent 改造已完成并合入 `main`。
- 桌面 Host 从 Fox 管理的运行时 `<runtime>/backends/dsh/` 启动 `dsh --profile acp`（锁定 `0.2.1-rc.2`），并应用 `<runtime>/dsh-patches/` 下逐字节校验过的 Host 补丁；不经 `npx`。见 `src/external_launch.rs`。
- 默认在 srt 沙箱里运行；进程只继承白名单环境变量；登录信息放在 Fox 管理的私有 HOME。
- 权限按 dsh 的原生选项显示（`src/native_permission.rs`）：只读、工作区可写（询问）、工作区可写（不询问，默认）、完全访问。dsh 不支持会话中途切换，下一轮开始时重启 dsh 生效。完全访问是独立开关，只在创建会话时授予，会话中只能撤销。
- 模型默认来自「模型服务」供应商的 Anthropic Messages 协议；API Key 按 base URL 在系统凭据库全局共享。

完整的路由、隐藏开关、权限和凭据说明见 `docs/02-架构/Agent运行时架构.md` 的「外层 Agent 后端（四 Agent）」一节；人工复测记录见本目录 `sandbox.md`。

- 后端：DeepSeek Harness 原生 ACP 入口 `dsh --profile acp`（npm `@deepseek-ai/dsh`）。
- 上游状态：developer preview，未做安全审计，会有破坏性变更（官方 README）。
- 本分支代码：`src/backends/deepseek.rs`；测试：`tests/deepseek_backend.rs`。
- 结论先行：**第 8 节底线中 T04 当前不满足 → DeepSeekBackend 不得默认开启，手动开关也不应对用户开放**，直到 Host 在 dsh 进程外再加一层 OS 隔离（见第 4 节）。T05、凭据保护（Host 侧职责）满足。

## 1 环境与版本

| 项 | 值 |
|---|---|
| 测试日期 | 2026-10-19 |
| 平台 | Linux aarch64，Debian 13 (trixie)，内核 6.12；Node 22.23.3；cargo 1.99.1 |
| dsh 版本 | `dsh --version` → `0.2.1-rc.2`（npm dist-tag latest = next = 0.2.1-rc.2） |
| 锁定 | `PINNED_VERSION = "0.2.1-rc.2"`，`check_version()` 只接受这一版本（容忍前缀 `dsh`/`v`），其余拒绝启动 |
| initialize 中的 agentInfo | `{"name":"deepseek-harness-acp","version":"0.0.1"}`（0.0.1 是 dsh-acp 硬编码的协议面版本，不是包版本，版本校验必须走 `dsh --version`） |
| 安装 | `npm install --prefix /workspace/work/tools/dsh @deepseek-ai/dsh@0.2.1-rc.2 --save-exact`（615 个包，约 850 MB）。注意：直接装到 JuiceFS 工作区超时并产生损坏的 node_modules（zod 缺文件，dsh 启动报 `required plugin did not activate`），实际做法是先装到本地盘 `/tmp/dshinst` 再复制。 |
| 未测平台 | Windows、macOS（T14 未做） |

## 2 阶段 0 实测（步骤 / 预期 / 实际 / 证据）

证据标记：**M1** = 真实二进制 ACP 实测（Rust `AcpConnection` 与 Node 原始 JSON-RPC 探针 `/workspace/work/tools/dsh-probe.mjs`，输出存于 `/workspace/work/tools/dsh-probe-output.txt`）；**M2** = `dsh --profile acp [--patch] --dump-config`（输出 `/workspace/work/tools/dsh-acp-dump-config.yml`）；**S** = 0.2.1-rc.2 包内源码与 README。

| # | 步骤 | 预期 | 实际 | 证据 |
|---|---|---|---|---|
| 1 | `dsh --version` | 输出版本号 | `0.2.1-rc.2` | M1 |
| 2 | `initialize`（protocolVersion 1） | ACP v1 + 能力声明 | 见下方原始响应；`sessionCapabilities` = close/list/resume；无 `loadSession`；`mcpCapabilities.http=true`；`authMethods=[]` | M1 |
| 3 | `session/new`（绝对 cwd） | 返回 sessionId | 返回 UUID + configOptions（model 仅 `deepseek-official / deepseek-v4-flash`；reasoning_effort off/low/high/max，默认 high） | M1 |
| 4 | `session/new`（相对 cwd） | 拒绝 | `-32602 Invalid params: cwd must be an absolute path` | M1 |
| 5 | 同一连接第二个 `session/new` | 一个连接多会话 | 成功，得到第二个独立 sessionId | M1 |
| 6 | `session/list` | 列出持久化会话 | 只列出**已不活动**的持久化根会话（上一次进程留下的），当前活动会话不在列表 | M1 |
| 7 | `session/load` | 不支持 | `-32601 Method not found: session/load` | M1 |
| 8 | `session/prompt`（无 DEEPSEEK_API_KEY） | 明确失败，不伪造成功 | 113 ms 内返回 `-32603 Internal error: turn failed: llm-deepseek: no API key for provider route "deepseek-official"; store DEEPSEEK_API_KEY through the credentials service ..., or export DEEPSEEK_API_KEY ...`；Rust 侧为 `BackendEvent::Failed("prompt failed: ...")` | M1 |
| 9 | `session/close` ×2 | 成功 | 都返回 `{}` | M1 |
| 10 | `session/cancel` | 是通知 | 以请求方式发送得到 `Method not found`（符合规范：cancel 是通知）；通知形式的取消效果需要真实 turn，**未测** | M1 |
| 11 | `--patch` 与 `$DSH_HOME/cordis.patch.yml` 冲突 | --patch 优先 | home 级补丁设 `danger-full-access`，`--patch` 设 `read-only`，合成结果为 `read-only` | M2 |
| 12 | 带 Host 补丁启动并跑 2–9 | 正常启动 | 正常（read-only + approval never + 单一 preset + telemetry DISABLED + tool-web disabled） | M1 |
| 13 | 工具调用 / 审批往返 / 取消确认 / 沙箱经 turn 生效 | — | **未测**（无 DeepSeek 凭据，模型不会发起工具调用） | — |

`initialize` 原始响应（线上 JSON-RPC result，原样）：

```json
{"protocolVersion":1,"agentInfo":{"name":"deepseek-harness-acp","version":"0.0.1"},"agentCapabilities":{"mcpCapabilities":{"http":true},"promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},"sessionCapabilities":{"close":{},"list":{},"resume":{}}},"authMethods":[]}
```

经 agent-client-protocol 3.4.0 反序列化再序列化后（`BackendEvent::Initialized` 携带的形态，默认值被显式化）：

```json
{"protocolVersion":1,"agentCapabilities":{"loadSession":false,"promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},"mcpCapabilities":{"http":true,"sse":false},"sessionCapabilities":{"list":{},"resume":{},"close":{}},"auth":{}},"authMethods":[],"agentInfo":{"name":"deepseek-harness-acp","version":"0.0.1"}}
```

两种形态都作为测试常量，`capabilities()` 对两者给出相同结果。

## 3 源码确认（S）

| 问题 | 结论 | 出处 |
|---|---|---|
| `request_permission` 覆盖哪些工具 | **只覆盖沙箱“提权”**。acp 组合中没有任何 `tools/pre-execute` 监听器返回 `ask`（唯一返回 `ask` 的 experimental auto-review 未挂载）；`ctx.approval.request` 只在 bash / fs 写入工具带 `sandbox_permissions` 重试更宽模式时被调用。普通读、工作区内写、沙箱内 bash、web_fetch/web_search、MCP 工具都**不经审批**直接运行 | dsh-tools `prepareExecution`/`serviceAsk`；dsh-sandbox `approveEscalation`；dsh-tool-bash、dsh-tool-fs；dsh-acp `approval/request` |
| 选项粒度 | 固定两个：`allow-once`（kind allow_once）/ `reject-once`（kind reject_once）；无“总是允许”。dsh 侧审批只对该请求有效；请求被中止后晚到的答复被丢弃 | dsh-acp、dsh-user-approval README |
| 答复映射 | `cancelled` → cancelled；`allow-once` → allowed-once；其余 → rejected | dsh-acp |
| cancel | `session/cancel` / `$/cancel_request`：停止调用时存在的 root 活动并等其输出排空，prompt 以 cancelled 结算；但 README 写明之后子代理消息“可能启动自主工作”，不推迟已取消的响应 | dsh-acp README |
| session list / resume / close | 支持；resume 校验 cwd 物理一致，不回放历史；close 排空并释放，持久化状态保留可再 resume | dsh-acp README、M1 |
| session/load、历史回放 | 不支持 | README、M1 |
| MCP | stdio 与 Streamable HTTP；不支持 SSE / ACP 传输；只用 tools，不用 resources/prompts；客户端被视为可信控制者（stdio 条目即授权其命令与环境） | dsh-acp README |
| 一个连接多会话 | 支持，会话独立，每会话一个 in-flight prompt | README、M1 |
| 工作目录 | 每会话一个绝对 cwd，不可变，不支持额外目录；`sandbox-policy.workspaceRoot` 回退值为 `process.cwd()` | dsh-acp、dsh-sandbox-policy |
| 沙箱模式 | `read-only` / `workspace-write`（默认）/ `danger-full-access`，由 `DSH_PERMISSION_MODE` 或配置决定；会话可在运行中切换（UI / `/permission` 命令）；模型可经提权审批对单次调用放宽 | dsh-sandbox-policy、dsh-permission-presets、M2 |
| 文件系统边界 | 写：bash 走内核 runner（bwrap / Landlock / Seatbelt，不可用时 `SANDBOX_UNAVAILABLE` 失败关闭，可能报告 `partial`）；fs 工具走用户态路径围栏（“a policy fence, not a kernel boundary”）。**读：完全不受限** | dsh-bash-sandbox、dsh-fs-sandbox、dsh-sandbox-local |
| 网络 | **无网络沙箱**：“network stays unrestricted”；bwrap 参数中无 `--unshare-net`；web_fetch/web_search 默认启用 | dsh-bash-sandbox、dsh-sandbox-local |
| 凭据 | `DEEPSEEK_API_KEY` 查找顺序：dsh 凭据文件 > 启动环境 > **项目 `.env`** > `$DSH_HOME/.env`；工具子进程环境会剔除名字含 KEY/PASSWORD/SECRET/TOKEN 的变量；但凭据文件“cannot isolate secrets from the agent”（同用户、读不受限） | dsh-credentials-local、dsh-subprocess `scrubbedParentEnv`、dsh-launch-environment |
| 遥测 | `session-telemetry-otel` 默认 `FEEDBACK_ONLY`，仅在用户显式反馈后上传到 `dsh-otel-collector.deepseeksvc.com`；可设 `DISABLED` | dsh-session-telemetry-otel、M2 |
| 数据落盘 | 会话日志写在 `$HOME/.dsh/sessions/<cwd 编码>/<id>`，首次启动还会在 `$HOME/.dsh/profiles/acp` 生成 profile | M1 |

## 4 Host 策略映射（`policy_patch` / `launch_spec`）

dsh 支持用 `--patch <绝对路径>` 叠加配置（优先于 home 级补丁，M2）。`DSH_PERMISSION_MODE` 环境变量不可用：不在 Host 环境白名单内（`launch.rs`），且补丁更明确。映射：

| Host 策略 | dsh 补丁 |
|---|---|
| 可写 / 只读 | `sandbox-policy.mode = workspace-write / read-only` |
| 不允许任何放宽 | `approval.policy = never`（提权请求在 dsh 内直接拒绝，不到 Host）；preset 表替换为仅含所选模式（approval never）且设为默认，`danger-full-access` 无法被选中 |
| 不外发遥测 | `session-telemetry-otel.mode = DISABLED` |
| 不允许网络 | 禁用 `tool-web`（web_fetch/web_search）——**bash/node 子进程网络仍可用，无法关闭** |
| 工作目录 | Host 以会话授权根目录为 spawn cwd 并作为 `session/new` 的 cwd |
| 防御性兜底 | 若仍收到 `request_permission`（只可能是提权），`host_permission_answer` 只回 `reject-once`，绝不选 allow |

`PolicyMapping.unenforced` 列出补丁做不到的部分：目录外读取、fs 工具写入只是用户态检查、用户级补丁插入的行无法中和、（关网络时）子进程网络。

### 4.1 `acp` 行与 `USER_OWNED_ROWS`（2026-10-20，外部后端接通批）

- **现象**（Windows 实测，dsh 0.2.1-rc.2）：`--profile acp` 的配置里，ACP profile 自带一行 `acp`，内容是 `config: { provider: deepseek-official, model: deepseek-v4-flash }`（`--dump-config` 可见）。这一行优先于 `agent-default-model`，所以用户在那里配的 provider 不生效。官方端点如果不提供这个模型，这一轮会被拒。
- **做法**：在私有 HOME 自己的补丁 `$DSH_HOME/cordis.patch.yml`（即 `<私有 HOME>/.dsh/cordis.patch.yml`）里写一行 `- id: acp`，填上用户自己的 `provider` 和 `model`。`deepseek::acp_model_patch(provider, model)` 生成这一行，值写成单引号 YAML 标量，供安装 / 设置工具和文档使用。Fox 不会自行写入。
- **Host 补丁不碰的行**：`deepseek::USER_OWNED_ROWS = ["acp", "agent-default-model", "llm"]`。原因是 Host 的 `--patch` 叠加会**整行替换**它点名的每一行的 config，点名了就会悄悄重置用户的选择。所以 `policy_patch` 生成的每个变体都不得出现这三行，也不得出现 `provider:` / `model:`。单元测试 `host_patch_never_touches_the_users_model_rows` 钉住这一点。
- **审批**：接通批中，DeepSeek 在 Host 补丁下的审批策略为 `never`。如果仍有 `request_permission` 到达，`external.rs` 经 `host_permission_answer` 直接拒绝，不显示为可答的审批卡。
- **真实测试**：`FOX_TEST_REAL_ACP=1` 下的 dsh 测试现在断言 `stopReason = end_turn` 且回复非空，不再只看“没有报错”。
- **第 8 节第 8 条的更新**：模型不再固定为 `deepseek-v4-flash`。用户经私有 HOME 的 `acp` 行自选模型，Host 补丁不覆盖。

## 5 四维能力声明（`capabilities()`）

| 维度 | 声明 | 证据 / 说明 |
|---|---|---|
| 审批覆盖：FileRead | None | S：读永不审批 |
| 审批覆盖：FileWrite | None | S：工作区内写不问；区外拒绝，只有提权才问，而 Host 补丁设 never |
| 审批覆盖：Shell | None | S：沙箱内直接运行 |
| 审批覆盖：Network | None | S：web 工具与 bash 网络都不审批 |
| 审批覆盖：McpBusinessTool | None | S：MCP 工具无 ask 闸门；业务授权必须留在 Host 的 MCP 服务端（6.3 第 8 条） |
| 沙箱：文件系统 | Unverified（细分见 `filesystem_detail()`：目录外读 = Absent；bash 写 = Unverified（源码称内核强制，未经 turn 实测）；fs 工具写 = Advisory（用户态围栏）） | S |
| 沙箱：网络 | Absent | S |
| 取消：turn | TurnRequested | 需真实 turn 才能确认；README 称子代理可在取消后自主继续 |
| 取消：进程树 | true（Fox 持有 dsh 进程组）；dsh 子进程用 systemd scope / bwrap `--die-with-parent`，是否全部随组退出未实测，强杀后按 6.4 第 4 条标“可能存在未核实副作用” | acp.rs、S |
| 审批答复确认 | false（ACP v1 无回执） | 规范 |
| 可观测性 | Unverified（README 称有通用 tool lifecycle，未观测到真实 turn） | — |
| 会话 | list ✓ / resume ✓（不回放）/ close ✓ / load ✗ / steer ✗ | M1 |

## 6 实验后端底线（第 8 节）逐项判断

| 底线 | 判断 | 理由 |
|---|---|---|
| **T04 权限隔离** | **不满足** | ① 任何 dsh 模式（含 read-only）都**不阻止授权根目录外的读取**：代理可读 `~/.ssh`、其他 Agent 的凭据文件、Fox 自身数据、`~/.dsh` 凭据、同用户进程的 `/proc/<pid>/environ`，并把内容发给模型服务；② **网络无法阻断**：dsh 没有网络沙箱，禁用 web 工具后 bash/node 仍可外连；③ 只读模式只能保证“不写”（bash 写入由内核 runner 强制的说法也尚未经 turn 实测）。业务操作阻断由 Host MCP 服务端负责，与 dsh 无关 |
| **T05 审批不重放** | 满足（源码 + 公共客户端测试；真实往返未测） | dsh 选项只有一次性 allow/reject，审批只对该请求有效，中止后晚到答复丢弃；Host `AcpConnection` 每个请求只消费一次答复、cancel 时以 cancelled 结算所有挂起请求（`acp_mock_agent` 测试）；Host 补丁设 approval never，根本不存在可重放的授权 |
| **凭据保护** | Host 侧满足；有与 T04 绑定的残余风险 | 子进程环境 = 白名单 + 按名传入 `DEEPSEEK_API_KEY`（`child_environment`）；Host 不复制、不解析 dsh 凭据库；dsh 自身剔除工具子进程中的 KEY/TOKEN/SECRET 变量。残余：读取不受限 → 代理可读 dsh 凭据文件；dsh 还会读取项目目录 `.env` 中的 `DEEPSEEK_API_KEY` |

**结论**：`floor_verdict().may_enable() == false`。按第 8 节“门槛未过不得默认开启、手动开启也一样”，当前 DeepSeekBackend **既不能默认开启，也不应提供手动开关**；即便以后开放，也只能是“受限目录 + 只读 + 外层隔离”。使 T04 可满足的最小前提（需公共层/Host 评审，不在本分支范围）：

1. 由 Host 在 dsh 进程外套一层 OS 隔离（Linux bwrap/容器、macOS sandbox-exec、Windows AppContainer/Job），只挂载授权根目录（其余不可读）、私有 HOME/DSH_HOME；
2. 出站网络只放行 DeepSeek API 端点（模型调用本身需要网络，不能整体断网）；
3. 在该外层隔离内重新跑 T04（越权读、越权写、外连）并把 `sandbox_filesystem` / `sandbox_network` 按实测更新。

## 7 其余门槛现状

| 用例 | 现状 |
|---|---|
| T01 | 依赖公共路由，本分支无专属逻辑；未测 |
| T06 | 未测（需真实 turn）；cancel 只声明 TurnRequested |
| T07 | 可记为已知限制；连接代次由公共层处理；dsh 进程退出会产生 `Closed`，未做崩溃注入 |
| T08 | resume 支持（不回放）、load 不支持；UI 必须提示“恢复不含历史回放” |
| T10 | 一个连接多会话已实测可用；默认拓扑仍为一会话一进程（6.1）；并发上限未测 |
| T12 | 与本后端无关（公共层） |
| T14 | 未测 Windows（`dsh.cmd`、pwsh-sandbox 为 ACL partial 强制） |

## 8 已知缺口与待办

1. **T04 不满足**（见第 6 节），这是开放前的硬阻塞。
2. 无 DeepSeek 凭据，未跑任何真实 turn：工具调用、审批往返、cancel 确认、沙箱经 turn 生效、`session/update` 映射全部未测。拿到 key 后用 `FOX_TEST_REAL_ACP=1 DEEPSEEK_API_KEY=... cargo test -p fox-agent-backend --features acp --test deepseek_backend -- --nocapture` 复测，并补充提权被 never 拒绝、read-only 下写入被拒的用例。
3. ~~`src/launch.rs` 的 `DSH_VERSION` 仍为占位 `"0.1"`~~ 已是 `0.2.1-rc.2`；`deepseek::launch_spec` 用 `PINNED_VERSION`，并加 `--patch`。
4. 公共层建议：`AcpConnection::spawn` 等待 `initialize` 没有超时，且子进程 stderr 被丢弃。实测冷启动耗时差异很大：从本地盘（/tmp）启动到完成 prompt 约 2 秒；从 JuiceFS 工作区路径启动约 76 秒（850 MB 依赖的模块加载慢）；安装损坏时 dsh 报 `required plugin did not activate`，冒烟测试表现为长时间无响应（是否会无限挂起未核实；立即退出的假二进制会快速失败）。建议公共层给 initialize 加可配置超时并在失败时附带 stderr 摘要；生产安装应放在本地盘。
5. 会话可在运行中切换沙箱模式（`/permission` 命令、UI）；ACP 文档称 commands 不在 ACP 面上，但提示词中的斜杠命令是否被解析未经实测。
6. 用户级 dsh 配置（`$HOME/.dsh` 下的 home 补丁、profile 补丁、settings）仍然生效：`--patch` 能覆盖同 id 行，但无法中和用户插入的新插件行。HOME 在白名单中，Host 目前无法给 dsh 私有 DSH_HOME。
7. dsh 把会话日志（含提示词与工具结果）写在 `$HOME/.dsh/sessions`，位于 Fox 数据管理之外。
8. ~~Model 只提供 `deepseek-v4-flash`（acp profile 固定），切换需改补丁。~~ 已解决：用户在私有 HOME 补丁里写 `acp` 行自选模型，见 4.1。
9. 上游为 developer preview，rc 版本之间会破坏性变更；每次升级必须重新实测并更新 `PINNED_VERSION`。
