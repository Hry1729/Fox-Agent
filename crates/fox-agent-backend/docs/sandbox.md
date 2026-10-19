# 外部 Agent 后端的权限模式与 srt 沙箱层

- 代码：`src/sandbox.rs`（模式、策略、预检、启动包装、能力声明），`src/acp.rs` 的 `AcpConnection::spawn_prepared`。
- 实测：`tests/sandbox_srt.rs`。真实测试标 `#[ignore]`，普通 `cargo test` 只把它计入 “ignored”（即跳过数），**cargo 全绿不等于实测通过**；统计口径见第 6.3 节。
- 决策来源：用户 2026-10-19 拍板；方案文档 `docs/02-架构/四Agent改造实施方案-v0.3.1.md` 末尾“v0.3.2 增补”。
- srt：`@anthropic-ai/sandbox-runtime`（Apache-2.0），本文按 0.0.80 及其 README 编写。

## 1 两种模式

| 模式 | 何时 | 隔离 | 不变的 Host 底线 |
|---|---|---|---|
| `sandboxed`（默认） | 除明确授予 full_access 外的一切情况；旧会话（未记录模式）一律按此处理；未知取值也按此处理 | 整个外部后端进程（codex-acp / pi-acp / dsh）在 srt 里运行，一个会话一个沙箱。Linux/macOS：读写限于授权目录 + 本项目本后端的私有 HOME + 本会话临时目录，另只读放行后端/Node/srt 安装目录与第 3 节列出的最小系统路径；Windows：仅用户目录受读保护（见第 5 节）。出站网络只放行该后端模型 API 域名白名单 | 环境变量白名单 / 凭据不下发；取消与进程树清理；记录；原生审批如实转发 |
| `full_access` | 用户按会话或项目明确授予（`HostPolicy.full_access_grants` 中的 `conversation:<id>` / `project:<root>`），且企业策略未禁止（`full_access_forbidden`）、`locked_keys.permission_mode` 未锁成 `sandboxed` | 不套沙箱，不承诺目录边界；能力声明中沙箱两维固定为 `absent`，即使后端原生有沙箱也不替它宣称 | 同上 |

判定入口：`effective_permission_mode(policy, snapshot, conversation_id)`。项目配置、用户设置都无法单独产生 full_access：只有“会话快照请求 full_access”＋“Host 持有的有效授予”同时成立才行。

收紧立即生效（沿用 6.5）：`recheck_permission_mode(...)` 与 `config::recheck_before_continue` 一起在每次继续 / 恢复前调用。授予被撤销而进程仍在沙箱外运行 → `RestartSandboxed`，Host 按第 1.1 节的“撤权停止”契约处理。放宽（运行中的沙箱会话新获授予）不静默带入，需 Host 有意重启。

### 1.1 撤权停止：Host 接线契约

crate 提供语义，Host 负责在对的时机调用（本轮未接入 Desktop Host）。

| 步骤 | 入口 | 语义 |
|---|---|---|
| 0 触发 | `recheck_permission_mode(policy, snapshot, conversation_id, running_mode)` 返回 `RestartSandboxed(reason)` | Host 在撤销授予（UI 撤权、企业策略切换、`locked_keys.permission_mode=sandboxed`）**当下**对该会话调用一次，并在每次继续 / 恢复前再调用；不得等到下一轮 turn 才处理正在跑的进程 |
| 1 停止 | 活连接：`AcpConnection::stop_for_revocation(self, session_id, &mut pending, &mut arbiter, reason).await`；其他形态：`port::stop_for_revocation(&mut dyn RevocationTarget, ..)` | 依次：① `session/cancel`，未答复的原生审批全部答 `cancelled`；② 本 run 的 `PendingRegistry` 全部结算——未发出 → `cancelled`，已发出 → `outcome_unknown`（复用 6.4 规则 1，保留关联 ID，晚到响应进审计不翻转）；③ 杀进程树（SIGTERM → 3 s 宽限 → 进程组 SIGKILL；Windows 由 Host 的 Job Object 负责）；④ `TerminalArbiter` 提交 `cancelled`（已先提交 `completed` 的保持 `completed`），被强杀或有 `outcome_unknown` 时标 `possible_unverified_side_effects` |
| 2 状态 | 返回 `RevocationStop { state: NeedsSandboxedRebuild, replay_last_operation: false, settled, cancelled_permissions, terminal }` | 会话不再有任何在沙箱外运行的进程；`needs_review()` 列出结果未知的操作 |
| 3 重建 | `port::rebuild_after_revocation(&mut stop, \|mode\| { preflight + prepare_launch(mode, ..) })` | 闭包只会收到 `Sandboxed`；返回的启动若不是 sandboxed（或没有 srt 会话）一律拒绝。成功 → `RebuiltSession { launch, replay_last_operation: false, review }`，Host 用 `spawn_prepared` 启动并 `bump_generation()`；**不重发上一条提示、不重跑 `review` 中的操作**，交给用户核对。失败 → `stop.state = StoppedSandboxUnavailable { error }`，会话保持停止，UI 展示 `error.remedy`，绝不回退 full_access |

Host 侧还需要：撤权与停止写审计；停止期间到达的原生事件按旧 generation 进审计；用户在 `review` 核对完之前，UI 不提供“一键重试上一步”。

## 2 fail-closed 规则

sandboxed 模式下，以下任何一项失败都**拒绝启动该后端**并返回结构化错误 `SandboxError { code, message, missing, remedy }`（serde 输出 snake_case 的 `code`；`PortError::Sandbox` 携带它）。**绝不自动退回 full_access。**

| code | 触发 |
|---|---|
| `sandbox_dependency_missing` | srt 包不在 Fox 管理目录、srt 版本 < 0.0.80、Node 缺失或 < 22.12、Linux 缺 bwrap / socat / rg、macOS 缺 rg |
| `sandbox_not_installed` | Windows 未执行 `windows-install`（查 `HKLM\SOFTWARE\sandbox-runtime`） |
| `sandbox_unavailable` | 不支持的 OS；Linux 上 bwrap 无法建立 user/net namespace（预检实跑 `bwrap --ro-bind / / --unshare-user --unshare-net -- /bin/true`）；macOS 无 `/usr/bin/sandbox-exec`；准备会话目录失败 |
| `sandbox_policy_invalid` | 无授权目录；无 denyRead 根；路径非绝对；授权目录 / 私有 HOME / 额外只读路径等于或包含某个 denyRead 根（会把它重新打开，例如授权 `/`）；项目 UUID 为空 / 非法 / 全零；私有 HOME 不在私有 HOME 根下；授权目录与私有 HOME 根或会话临时目录重叠；域名格式错误；会话 ID 非 `[A-Za-z0-9_-]{1,128}`；settings 目录落在沙箱可读或可写范围内；撤权重建拿到的不是 sandboxed 启动 |
| `sandbox_backend_not_installed` | 后端经 `npx` 启动（需要 npm 源与用户的 npx 缓存）；程序不是绝对路径；程序在沙箱内不可读 |

调用顺序：`preflight(install, probe)` → `SrtLauncher::new(ready, state_root)` → `prepare_launch(mode, spec, session_id, SandboxSetup::{Ready|Unavailable}, os)` → `AcpConnection::spawn_prepared(prepared, cwd)`。预检失败时把错误放进 `SandboxSetup::Unavailable(err)`，`prepare_launch` 在 sandboxed 下原样返回、在 full_access 下忽略。

## 3 srt 配置模板

srt 的读权限默认全放行（deny-then-allow，`allowRead` 优先于 `denyRead`），写默认全拒，网络默认全拒。所以每会话 settings 在 Linux/macOS 上先 denyRead 根目录 `/`（srt README：`denyRead: ["/"]` 拒绝 `/` 下每个目录，`/proc` `/dev` `/sys` 除外），再 allowRead 需要的路径；Windows 只能拒读 `C:\Users`（第 5 节）。`SandboxPolicy::to_srt_settings()` 生成（路径一律用 `{"path": ..., "literal": true}`，不按 glob 解释，`[WIP] 项目` 这样的目录名也只表示它本身）：

```json
{
  "network": {
    "allowedDomains": ["<该后端模型 API 域名>"],
    "deniedDomains": [],
    "allowLocalBinding": false
  },
  "filesystem": {
    "denyRead":  ["/"],
    "allowRead": ["<授权目录...>", "<本项目本后端私有 HOME>", "<本会话临时目录>", "<后端安装目录>", "<Node 运行时>", "<srt 包目录>", "<最小系统路径...>"],
    "allowWrite": ["<可写授权目录...>", "<私有 HOME>", "<本会话临时目录>"],
    "denyWrite": []
  },
  "enableWeakerNestedSandbox": false,
  "allowNestedUserNamespaces": false
}
```

- denyRead 默认根（`default_deny_read_roots`）：Linux / macOS `/`；Windows `C:\Users`。
- 最小系统路径（`default_system_read_only`，只读；启动时去掉本机不存在的项）：
  - Linux（实测）：`/usr` `/bin` `/sbin` `/lib` `/lib32` `/lib64` `/libx32`；`/etc` **不整体放行**，只放行 `/etc/alternatives` `/etc/ca-certificates` `/etc/crypto-policies` `/etc/gai.conf` `/etc/group` `/etc/host.conf` `/etc/hosts` `/etc/ld.so.cache` `/etc/ld.so.conf` `/etc/ld.so.conf.d` `/etc/localtime` `/etc/nsswitch.conf` `/etc/passwd` `/etc/pki` `/etc/resolv.conf` `/etc/ssl`。`/etc/hostname`、`/etc/shadow`、`/etc/environment` 等其余内容在沙箱内不存在（实测）。
  - macOS（**未实测**的起始集合）：`/usr` `/bin` `/sbin` `/System` `/Library/Apple` `/Library/Preferences/SystemConfiguration` `/private/etc/{hosts,resolv.conf,ssl,passwd,group,localtime}` `/private/var/db/timezone` `/private/var/db/dyld` `/dev`。
  - Windows：无（系统目录本就不拒读）。
- srt 包目录与 Node 运行时若不在上述系统路径内（例如 `/opt/fox/node`、`~/.nvm`），`SrtLauncher::effective_policy` 自动只读放行它们本身（Linux 上 srt 要在沙箱内执行 `vendor/seccomp/<arch>/apply-seccomp`，不放行会直接失败）。
- 被拒读的目录在沙箱内是**空的 tmpfs**：允许路径的上级目录名会以空目录骨架出现（例如 `/home/<用户>/…`），在拒读区域内写入只落进该 tmpfs，主机上不可见、会话结束即丢失（实测：向 Fox state 目录写入在沙箱内“成功”，主机上无文件）。srt 另默认放行 `/tmp/claude` 写入（同为 tmpfs）。
- settings 文件写在 `state_root/sandbox-sessions/<session_id>/srt-settings.json`（Unix 0600），`state_root` 必须既不可写也不可读（`wrap` 校验：不在任何可读 / 可写根之下）；`SandboxSession` 随连接释放（`close()` 或 drop）时删除会话 settings 目录**和本会话临时目录**，私有 HOME 保留。
- 启动命令：`<node> <srt>/dist/cli.js --settings <file> -- <后端程序绝对路径> <参数...>`。
- 环境变量（在 launch spec 自身的 session_env 之后覆盖）：`HOME`、`XDG_CONFIG_HOME/CACHE_HOME/DATA_HOME/STATE_HOME`（私有 HOME 下）；`TMPDIR/TMP/TEMP` 与 `CLAUDE_CODE_TMPDIR` = 本会话临时目录；`NODE_USE_ENV_PROXY=1`；Windows 另设 `USERPROFILE/APPDATA/LOCALAPPDATA`；codex 设 `CODEX_HOME=<私有 HOME>/.codex`，deepseek 设 `DSH_HOME=<私有 HOME>/.dsh`；`PATH` = 后端程序目录 + Node 目录 + 系统目录，不继承用户 PATH。环境白名单（`launch::child_environment`）不变，仍先于这些覆盖生效。
  - 临时目录必须在沙箱内可读：srt 进程本身继承这些变量，把代理桥接 socket 建在自己的 `TMPDIR` 下再 bind 进沙箱；若该目录被 denyRead 覆盖，沙箱内所有经代理的请求都失败（实测报 `Proxy CONNECT aborted`）。
  - srt 会把沙箱内的 `TMPDIR` 改成 `CLAUDE_CODE_TMPDIR`（缺省 `/tmp/claude`，在 `/` 拒读下是内存 tmpfs），所以同时设置它（实测沙箱内 `TMPDIR` = 本会话临时目录）。
  - Node 内置 `fetch` 默认不读 `HTTP(S)_PROXY`，沙箱内唯一出口是 srt 代理，故设 `NODE_USE_ENV_PROXY=1`（实测 `node -e fetch(...)`：不设 `EAI_AGAIN`，设后 200；真实模型 provider 尚未在沙箱内实测）。

### 私有 HOME（已定：每个项目 × 每个后端一份）

- 布局：`PrivateHomes::new(<fox_data>/sandbox-homes).home_for(project_uuid, backend)` → `<fox_data>/sandbox-homes/<项目 UUID>/<后端 ID>`（`PrivateHomeScope` 默认且目前唯一取值 `per_project_per_backend`）。同一项目同一后端的多个会话复用同一份 HOME（原生登录与历史在这些会话间可用），跨项目、跨后端互不可见。
- 目录名只用 Host 数据库中的**稳定项目 UUID**（规范化为小写 8-4-4-4-12）加后端 ID，不再从项目路径或名称派生，因此同名项目、同名文件夹、中文或含特殊字符的路径都不会碰撞。UUID 为空、格式非法或为全零 UUID → `sandbox_policy_invalid`；FoxBackend 没有沙箱私有 HOME。
- `SandboxPolicy` 携带 `private_home_root`；`validate` 拒绝：私有 HOME 不在该根下；任一授权目录等于、包含或位于私有 HOME 根之下（即不可能把其他项目 / 其他后端的私有 HOME 授权进来）；任一授权目录与本会话临时目录重叠。沙箱内只 bind 本项目本后端那一份 HOME，同根下的其他 HOME 不可见。
- 临时目录（`TMPDIR` 等）按会话分开：`<tmp_root>/<session_id>`（默认 `tmp_root = state_root/sandbox-tmp`，可用 `SrtLauncher::with_tmp_root` 改），会话结束随 `SandboxSession` 删除。
- 认证：尊重各后端原生登录方式，不做统一认证，不复制用户真实 HOME，也不复制其中的原生凭据（6.5）。沙箱模式下用户在该项目该后端的私有 HOME 内按后端原生方式登录（如 codex `codex login` 写入私有 `CODEX_HOME`），或使用环境白名单内的凭据变量。私有 HOME 位于沙箱可读写范围内，后端进程（及其执行的命令）能读到其中保存的登录凭据；**不得宣称“Agent 接触不到密钥”，也不得宣称“一次登录到处用”**——换项目或换后端需要再次登录。

## 4 网络白名单

`default_allowed_domains(backend, pi_providers)`：

| 后端 | 默认域名 | 依据 |
|---|---|---|
| codex | `api.openai.com`、`chatgpt.com`、`auth.openai.com` | API key 端点、ChatGPT 登录的 Responses 端点、令牌刷新；源码阅读，未经真实沙箱 turn 实测 |
| deepseek | `api.deepseek.com` | 官方 API |
| pi | 会话所配 provider 的并集（anthropic→`api.anthropic.com`，openai→`api.openai.com`，google→`generativelanguage.googleapis.com`，deepseek→`api.deepseek.com`，openrouter→`openrouter.ai`，xai→`api.x.ai`，groq→`api.groq.com`，mistral→`api.mistral.ai`，cerebras→`api.cerebras.ai`） | provider id 未逐个对照 pi 0.84.2 的 provider 表 |

`resolve_allowed_domains(backend, pi_providers, policy, layers)` 遵循五层配置“只能收紧”：

- 上限 = 后端默认 + `HostPolicy.sandbox_extra_domains[backend]`（只有第 1 层可以扩展，用于自定义 provider 端点）。
- `sandbox.allowed_domains`（逗号分隔）可出现在 Host `locked_keys`、会话快照、用户设置、项目配置；每层与当前结果取交集。上限外的条目、格式错的条目一律丢弃并记入 `rejected` 供审计。`*.x.com` 可收窄成 `a.x.com`，反之不行；带端口的上限不覆盖不带端口的请求。空值 = 无网络。
- 未知 pi provider 记入 `unknown_providers`，不放行任何域名，需 Host 在 `sandbox_extra_domains` 中显式列出。
- 运行中会话的网络收紧：本轮在下次继续前重算并重启进程；srt `--control-fd` 支持运行中替换网络名单，可作为后续优化（见第 7 节）。

注意：npm 源不在任何默认白名单中。沙箱模式下后端必须预先安装在 Fox 管理目录（见 `sandbox_backend_not_installed`），pi-acp 的 `npm view` 更新检查会被拒（800 ms 超时后继续）。

## 5 能力声明（四维中的“沙箱范围”）

`SandboxDeclaration::declare(mode, policy, os)` 给出模式、机制（`srt` 或无）、文件 / 网络强制程度、`directory_isolation`、可写 / 可读根与域名；`apply_to(&mut CapabilityDeclaration)` 并入后端实测能力：

- full_access：`sandbox_filesystem = sandbox_network = absent`，`read_scope = none`，`directory_isolation = false`，不列任何根。UI 不得出现“限定在这些文件夹”。
- sandboxed：取后端原生实测与 srt 两者较强者（排序 enforced > unverified > advisory > absent），既不隐藏原生沙箱，也不夸大 srt。
- srt 强制程度按平台实测：Linux = `enforced`（第 6 节）；macOS / Windows = `unverified`，直到工程师在该平台跑完同样的 T04。
- **读边界如实表达**：`read_scope` 由策略内容推出（`SandboxPolicy::read_scope(os)`），`read_scope_verified` 按平台实测状态给出（`srt_read_scope_verified`）。UI 文案只依据这两项（`ReadScope::ui_label`）：

| 平台 | denyRead | `read_scope` | `read_scope_verified` | UI 文案 | `directory_isolation` |
|---|---|---|---|---|---|
| Linux | `/` | `authorized_only` | 是（本轮实测） | 读取限定在授权文件夹 | 是 |
| macOS | `/` | `authorized_only` | 否 | 读取限定在授权文件夹（本平台待实测） | 否 |
| Windows | `C:\Users`（+ Host 追加） | `user_dirs_only` | 否 | 部分：仅用户目录受保护（本平台待实测） | 否 |
| 任一平台，策略只拒读部分目录 | 非 `/` | `user_dirs_only` | — | 部分：仅用户目录受保护 | 否 |
| full_access | — | `none` | — | 不限制读取 | 否 |

`directory_isolation` 仅在 srt `enforced` 且 `read_scope = authorized_only` 且已实测时为真。

## 6 Linux 实测（T04）

### 6.1 本轮：denyRead `/`（2026-10-19，第二轮）

环境：Debian 13 容器，Linux 6.12.110 aarch64，非 root（uid 1000），Node 22.23.3，bubblewrap 0.12.0，socat 1.8.0.3，ripgrep（系统包），srt 0.0.80，pi-acp 0.0.34 + pi 0.84.2 装在独立前缀（模拟 Fox 管理目录）。模型为测试专用 `fixtures/sandbox/scripted-cmd-provider.ts`（pi 进程内的脚本模型，按提示发出 bash / read 工具调用）；每个工具调用都经 Fox 审批扩展并被允许，拦住它的只有沙箱。settings：denyRead `["/"]`，allowRead = 授权目录、私有 HOME（`<fox-data>/sandbox-homes/<UUID>/pi`）、会话临时目录、fixtures 与 pi 前缀、srt 包、第 3 节 Linux 系统路径（本机存在的子集）；网络白名单只有测试自起的一个本地 HTTP 端点 `127.0.0.1:<端口>`。不再需要 `FOX_T04_EXTRA_DENY`。

```
FOX_TEST_REAL_SRT=1 FOX_SRT_PREFIX=<srt 前缀> FOX_PI_ACP_PREFIX=<pi 前缀> \
  cargo test -p fox-agent-backend --features acp --test sandbox_srt -- --include-ignored --nocapture
```

结果：`real_srt_pi_acp_t04_filesystem_and_network_boundary ... ok`（9.4 s），pi-acp 正常启动、`session/new` 成功、全部 14 个 turn 以 `end_turn` 完成。

| 检查 | 结果（沙箱内工具输出） |
|---|---|
| 授权目录读写 | `cat readme.txt` 得到内容；`inside.txt` 写入成功，主机可见 |
| 读授权目录旁的家目录文件（bash） | `No such file or directory` |
| 读主机 `/tmp` 下文件 | `No such file or directory` |
| 读家目录之外的数据目录（`/workspace/.../target/tmp/...`，上一轮的拒读根之外） | `No such file or directory`；pi read 工具 `ENOENT` |
| 读本会话 srt settings | `No such file or directory` |
| `/etc` 部分放行 | `/etc/hosts` 可读；`/etc/hostname`、`/etc/shadow` → `No such file or directory`；`ls /etc` 只列出本机存在的 13 个放行项 |
| 写授权目录外 | 家目录、主机 `/tmp`：`No such file or directory`；Fox state 路径：沙箱内写进 tmpfs 骨架，主机上无文件（断言通过） |
| 白名单内本地端点（经 srt 代理） | 返回端点令牌，端点计数增加 |
| 白名单外本地端点，普通 HTTP | `HTTP/1.1 403 Forbidden`、`X-Proxy-Error: blocked-by-allowlist`、正文 `Connection blocked by network allowlist`；端点计数不变 |
| 白名单外本地端点，CONNECT | `curl: (56) CONNECT tunnel failed, response 403`；端点计数不变 |
| 绕过代理直连主机回环 | `curl: (7) Failed to connect to 127.0.0.1 port … after 0 ms`，`code=000`，端点计数不变 |
| 绕过代理直连公网 IP | `curl: (7) Failed to connect to 1.1.1.1 port 80 after 0 ms`（连接被拒，不是超时） |
| 私有 HOME / 会话临时目录 | `HOME=<fox-data>/sandbox-homes/<UUID>/pi`、`TMPDIR=<state>/sandbox-tmp/t04`，均可写 |
| 关闭 | 无残留进程；会话 settings 与会话临时目录已删除；私有 HOME 保留 |

两个目标在进沙箱前都先从主机侧直连做了对照（返回 200 与各自令牌），所以沙箱内的拒绝不可能是目标不可达。

本轮调试中的发现（已修进代码）：
1. srt 把代理桥接 socket 建在**自身进程的 `TMPDIR`** 下；手工以默认 `TMPDIR=/tmp` 运行且拒读 `/` 或 `/tmp` 时，沙箱内请求全部 `Proxy CONNECT aborted`。Fox 启动时 srt 继承会话临时目录（可读），所以正常。
2. srt 把沙箱内 `TMPDIR` 改成 `/tmp/claude`；用 `CLAUDE_CODE_TMPDIR` 改回会话临时目录。
3. Node 内置 fetch 需要 `NODE_USE_ENV_PROXY=1` 才走代理。

### 6.2 上一轮：拒读 `/home /root /tmp /workspace`（2026-10-19，第一轮）

同环境；结论（授权目录读写、拒读目录内的读写、白名单外域名 `CONNECT 403`、直连失败、关闭清理）仍成立，但当时的断言对网络较宽（“非 200 即通过”），且拒读范围之外的目录（`/data`、`/mnt` 等）仍可读，本轮已替换。日志：上一轮 `t04-run-final.log`。

附带发现（未变）：`fixtures/pi/pi-gated.sh` 在仓库里是 100644（contents API 无法设可执行位），测试改为运行一份私有可执行副本；生产中 Host 需保证该脚本可执行（或改为 `sh <脚本>` 调用）。

### 6.3 测试统计口径

- **cargo 全绿不等于实测通过。** 真实测试有两种门控：
  - `tests/sandbox_srt.rs` 的真实测试标 `#[ignore]`：普通 `cargo test` 把它计入 `ignored`；加 `--include-ignored` 但没设 `FOX_TEST_REAL_SRT=1` 时**失败**而不是通过。`fail_closed_without_srt_refuses_sandboxed_launch` 不需要 srt，总是执行。
  - `tests/pi_backend.rs`（2 个）、`tests/codex_backend.rs`（1 个）、`tests/deepseek_backend.rs`（1 个）的真实测试仍是旧式门控：未设 `FOX_TEST_REAL_ACP=1` 时打印 `SKIPPED` 后返回，cargo 计为 `passed`。报告时必须按名单从 passed 中扣除。
- 报告格式：分列“真正执行并通过”“跳过（ignored + SKIPPED 名单）”“真实环境执行”三项，不得只报 cargo 的 passed 总数。

## 7 各平台注意事项与已知限制

**Linux**
- 依赖：bubblewrap（≥0.5 更佳）、socat、ripgrep；Ubuntu 24.04+ 需关闭 `kernel.apparmor_restrict_unprivileged_userns` 或加 AppArmor 配置，否则预检报 `sandbox_unavailable`。
- root 运行需要 `CAP_SETFCAP`；建议非 root。
- 沙箱内不能再建 user namespace（嵌套 bwrap、Chromium 沙箱、rootless podman 会 EPERM），这是 srt 的安全设计，Fox 不开启 `allowNestedUserNamespaces`。
- srt 包不得装在任何可写授权目录下（README 安全提示），Fox 管理目录满足此条件。
- 非 merged-usr 发行版（`/bin` `/lib` 为实目录）与 RHEL 系（`/etc/pki`、`/etc/crypto-policies`）已列入系统路径但未实测；若有工具缺文件，按实测最小化追加，不整体放行 `/etc`。

**macOS**（未实测）
- 依赖 `/usr/bin/sandbox-exec` 与 ripgrep。
- 默认 denyRead `/` + 第 3 节 macOS 起始集合；seatbelt 下拒读 `/` 后 dyld、系统框架、`/dev`、路径元数据（`stat` 上级目录）是否够用必须实测，按实测增删。在实测通过前 `read_scope_verified = false`，UI 显示“本平台待实测”。
- Go 程序经代理验证 TLS 可能需要 `enableWeakerNetworkIsolation`，Fox 不开启；若 codex 二进制受影响需评审。

**Windows**（alpha，未实测）
- 读边界只覆盖 `C:\Users`：对整盘加 ACE 代价高，`D:\` 等其他固定盘仍可读，能力声明为 `user_dirs_only`（“部分：仅用户目录受保护”）。
- **待工程师验证**：枚举所有非系统固定盘根（`GetLogicalDrives` + `GetDriveTypeW == DRIVE_FIXED`，排除系统盘）加入 denyRead 是否可行——srt-win 为 denyRead 写 deny ACE 的耗时与可回收性、对盘根加 deny ACE 是否影响其他程序、盘符变化（插拔、BitLocker 解锁）时如何重算、网络盘与 `subst` 盘如何处理。可行且实测后，再评估是否把 Windows 的 `read_scope` 提升。
- 首次选沙箱模式时由安装器调用 `windows_install_command()`（`node <srt cli.js> windows-install`，一次 UAC，幂等）；Fox 卸载时必须调用 `windows_uninstall_command()`（`windows-uninstall`，一次 UAC）。本轮只提供检测（`windows_install_state`）、命令与错误码，未集成安装器。
- 沙箱进程以 `srt-sandbox` 本地账户运行，读不到用户目录中的工具（npx 缓存、nvm、`%LOCALAPPDATA%\Programs`）：后端与 Node 必须装在 Fox 管理目录并 allowRead。
- 只支持会话级 allowRead/allowWrite（一会话一沙箱正好满足）。
- srt-win 给子进程的是全新环境，只叠加 broker 的 PATH 与代理变量：Fox 设置的 HOME/USERPROFILE/CODEX_HOME/TMPDIR 及凭据环境变量**很可能不会传入**，子进程用的是 `srt-sandbox` 账户自己的 profile，私有 HOME 与会话临时目录可能不生效。需实测并决定凭据如何提供。
- 代理令牌（`HTTP_PROXY=http://srt:<token>@127.0.0.1:...`）在 runner 进程命令行中可见，同机其他本地主体可读；单用户开发机通常可接受，共享主机上应视为白名单对同会话其他主体可达。
- schannel 吊销检查被 WFP 拦截（`CRYPT_E_REVOCATION_OFFLINE`），系统 DNS 解析不受围栏限制。
- 进程树清理：Fox 的 Job Object 包住的是 node/srt broker；sandbox 账户下的 runner 与子进程是否随之终止需实测（撤权停止依赖它）。

**通用**
- `SandboxPolicy::validate` 强制：至少一个 denyRead 根；不允许 allowRead 一个 denyRead 根或其祖先（Linux/macOS 上即不允许授权 `/`）；授权目录不得与私有 HOME 根或会话临时目录重叠。
- 网络按域名过滤，不检查流量内容；白名单内域名（如 `chatgpt.com`）本身仍可能被用来外发数据。
- Linux 上 Unix socket 由 seccomp 阻止创建；回环在沙箱自身的网络命名空间内可用，但到不了主机的回环服务（实测直连主机回环端口 `exit 7`）。
- 私有 HOME 中的原生登录凭据对该后端进程可读（见第 3 节“认证”）。
- 一次审批只批准发起调用；命令内部行为由沙箱范围约束（第 2 节原则不变）。

## 8 待办与待验证

- [ ] macOS：用同一测试（`tests/sandbox_srt.rs` 的 Linux 门控需放开或复制一份）跑 T04，验证 denyRead `/` 与系统路径集合，通过后把 `srt_enforcement(MacOs)` 改为 `Enforced`、`srt_read_scope_verified(MacOs)` 改为真。
- [ ] Windows：执行 windows-install 后跑 T04；验证环境变量 / 凭据 / 私有 HOME / 会话临时目录是否传入、Job Object 能否清理 runner 子树、pi-gated 需要 `.cmd` 版本；评估非系统固定盘根加入 denyRead 的可行性（第 7 节）；通过后改 `srt_enforcement(Windows)`。
- [ ] codex / deepseek 及真实模型 provider 在沙箱内的真实 turn：域名白名单是否够用（codex ChatGPT 登录、令牌刷新；dsh 的遥测或插件下载）；`NODE_USE_ENV_PROXY` 是否足以让 pi 的 provider 请求走代理。
- [ ] Fox 管理目录内的后端安装并锁定版本（替代 `npx -y`），以及 `launch_spec` 在 sandboxed 下指向已安装的程序。
- [ ] 运行中网络收紧经 `--control-fd` 即时下发（可选）。
- [ ] Desktop Host 接线：会话创建 / 继续路径调用 `effective_permission_mode`、`recheck_permission_mode`、`PrivateHomes::home_for`（项目 UUID 取自 Host 数据库）、`prepare_launch`、`spawn_prepared`；撤权按第 1.1 节契约调用 `stop_for_revocation` / `rebuild_after_revocation`；UI 依据 `read_scope` / `read_scope_verified` 与 `SandboxError.remedy` 展示。
- [ ] `tests/pi_backend.rs::real_gated_allow_reject_cancel_and_process_tree` 第 6 步断言“Host 杀进程树后 bash 仍存活”的已知缺口，在当前 `close()`（SIGTERM + 宽限）下已不成立，测试失败；需重新测量并更新 `backends/pi.rs` 的 `cancel_process_tree` 声明（本轮未改，见报告）。
