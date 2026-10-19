# 外部 Agent 后端的权限模式与 srt 沙箱层

- 代码：`src/sandbox.rs`（模式、策略、路径规范化、预检、启动包装、能力声明、交付文件校验），`src/winacl.rs`（Windows 所有权 / `WRITE_DAC` 预检、暴露面审计、精确 ACE 增删、所有者修改、私有目录创建），`src/winruntime.rs`（Windows 运行时位置与检查、ProgramData 私有目录、srt 注册表状态、SID 台账、安装记录、卸载边界），`src/winancestor.rs`（祖先元数据 ACE 与引用台账），`src/winowner.rs`（所有权修复），`src/statefile.rs`（加锁的 JSON 状态文件），`scripts/windows/enable-sandbox.ps1` / `disable-sandbox.ps1`（一次提权的启用 / 停用脚本），`src/acp.rs` 的 `AcpConnection::spawn_prepared`、停止状态与 Windows Job Object，`src/proctree.rs`（进程树停止确认），`src/port.rs`（撤权停止与重建的状态机）。
- 实测：`tests/sandbox_srt.rs`。真实测试标 `#[ignore]`，普通 `cargo test` 只把它计入 “ignored”（即跳过数），**cargo 全绿不等于实测通过**；统计口径见第 6.3 节。
- 决策来源：用户 2026-10-19 拍板；方案文档 `docs/02-架构/四Agent改造实施方案-v0.3.1.md` 末尾“v0.3.2 增补”。
- srt：`@anthropic-ai/sandbox-runtime`（Apache-2.0），本文按 0.0.80 及其 README 编写。

## 1 两种模式

| 模式 | 何时 | 隔离 | 不变的 Host 底线 |
|---|---|---|---|
| `sandboxed`（默认） | 除明确授予 full_access 外的一切情况；旧会话（未记录模式）一律按此处理；未知取值也按此处理 | 整个外部后端进程（codex-acp / pi-acp / dsh）在 srt 里运行，一个会话一个沙箱。Linux/macOS：读写限于授权目录 + 本项目本后端的私有 HOME + 本会话临时目录，另只读放行后端/Node/srt 安装目录与第 3 节列出的最小系统路径；Windows：沙箱账户只能读写授权目录、私有 HOME、会话临时目录、Fox 运行时，以及 DACL 对所有本地账户开放的位置（见第 5、7 节）。出站网络只放行该后端模型 API 域名白名单 | 环境变量白名单 / 凭据不下发；取消与进程树清理；记录；原生审批如实转发 |
| `full_access` | 用户按会话或项目明确授予（`HostPolicy.full_access_grants` 中的 `conversation:<id>` / `project:<root>`），且企业策略未禁止（`full_access_forbidden`）、`locked_keys.permission_mode` 未锁成 `sandboxed` | 不套沙箱，不承诺目录边界；能力声明中沙箱两维固定为 `absent`，即使后端原生有沙箱也不替它宣称 | 同上 |

判定入口：`effective_permission_mode(policy, snapshot, conversation_id)`。项目配置、用户设置都无法单独产生 full_access：只有“会话快照请求 full_access”＋“Host 持有的有效授予”同时成立才行。

收紧立即生效（沿用 6.5）：`recheck_permission_mode(...)` 与 `config::recheck_before_continue` 一起在每次继续 / 恢复前调用。授予被撤销而进程仍在沙箱外运行 → `RestartSandboxed`，Host 按第 1.1 节的“撤权停止”契约处理。放宽（运行中的沙箱会话新获授予）不静默带入，需 Host 有意重启。

### 1.1 撤权停止：Host 接线契约

crate 提供语义，Host 负责在对的时机调用（Desktop Host 接线是下一步）。

| 步骤 | 入口 | 语义 |
|---|---|---|
| 0 触发 | `recheck_permission_mode(policy, snapshot, conversation_id, running_mode)` 返回 `RestartSandboxed(reason)` | Host 在撤销授予（UI 撤权、企业策略切换、`locked_keys.permission_mode=sandboxed`）**当下**对该会话调用一次，并在每次继续 / 恢复前再调用；不得等到下一轮 turn 才处理正在跑的进程 |
| 1 进入 Stopping | 活连接：`AcpConnection::stop_for_revocation(self, session_id, &mut pending, &mut arbiter, reason).await` 的第一步即 `begin_stopping()`；其他形态：`port::stop_for_revocation(&mut dyn RevocationTarget, ..)` 先调 `RevocationTarget::begin_stopping()` | 连接进入“正在停止”：`prompt` / `new_session` 返回 `STOPPING_REFUSAL` 错误；当时挂起的原生审批（所有会话）全部答 `cancelled`；**之后才到达的** `session/request_permission` 在到达时即由 Host 答 `cancelled`，不进入待审批表、不发 `PermissionRequested`，只发观测事件 `BackendEvent::PermissionAutoCancelled { session_id, reason, request }`（UI 可显示“已自动取消”，不得提供允许按钮）。“停止标志 + 待审批表”在同一把锁下，检查与登记、置位与清空都是原子的，不存在漏进去的窗口（见 1.3） |
| 2 取消与结算 | 同上 | ① `session/cancel`；② 本 run 的 `PendingRegistry` 全部结算——未发出 → `cancelled`，已发出 → `outcome_unknown`（复用 6.4 规则 1，保留关联 ID，晚到响应进审计不翻转） |
| 3 停进程并确认 | 同上（内部 `terminate()`） | Unix：先快照后代（pid + 启动时间）→ 进程组 SIGTERM → 3 s 宽限 → 再快照 → 进程组 SIGKILL → 回收子进程 → 按 pid SIGKILL 已离开进程组的被跟踪后代 → 轮询确认（≤ 3 s）。Windows：关闭命令通道 → 3 s 宽限 → `TerminateJobObject`（无 Job 才兜底 `taskkill /T /F`）→ 轮询 `ActiveProcesses == 0`。结果是 `ProcessTreeStop { force_killed, confirmation: StopConfirmation, confirmation_scope_note, process_group, tracked }`；确认标准见 1.2 |
| 4 状态 | 返回 `RevocationStop { state, process_tree, settled, cancelled_permissions, terminal, replay_last_operation: false }` | `TerminalArbiter` 提交 `cancelled`（已先提交 `completed` 的保持 `completed`）；被强杀、有 `outcome_unknown` **或停止未确认**时标 `possible_unverified_side_effects`。`state`：确认 → `NeedsSandboxedRebuild`；未确认 → `StoppedUnconfirmed { reasons, remedy }`（阻塞，`remedy` = `STOP_UNCONFIRMED_REMEDY`，提示用户结束残留进程或重启 Fox）。`needs_review()` 列出结果未知的操作 |
| 5 重建 | `port::rebuild_after_revocation(&mut stop, \|mode\| { preflight + prepare_launch(mode, ..) })` → `Result<RebuiltSession, RebuildError>` | **停止未确认时直接返回 `RebuildError::StopUnconfirmed { reasons, remedy }`，不调用闭包，状态保持 `StoppedUnconfirmed`，任何模式都不启动。** 已确认时：闭包只会收到 `Sandboxed`；返回的启动若不是 sandboxed（或没有 srt 会话）一律拒绝。成功 → `RebuiltSession { launch, replay_last_operation: false, review }`，Host 用 `spawn_prepared` 启动并 `bump_generation()`；**不重发上一条提示、不重跑 `review` 中的操作**，交给用户核对。沙箱失败 → `RebuildError::Sandbox { error }`，`stop.state = StoppedSandboxUnavailable { error }`，会话保持停止，UI 展示 `error.remedy`，绝不回退 full_access |
| 6 复查（可选） | Unix：`proctree::confirm_tree_gone(stop.process_tree.process_group?, &stop.process_tree.tracked, timeout)`；任意来源的结论：`stop.record_confirmation(confirmation)` | 用户按提示处理后 Host 可复查；`Confirmed` 把状态改回 `NeedsSandboxedRebuild`，否则刷新 `StoppedUnconfirmed` 的原因。Windows 上 Job 句柄随连接关闭（kill-on-close 已生效），无法事后复查：引导用户重启 Fox 后由 Host 记录确认 |

Host 侧还需要：撤权与停止写审计；停止期间到达的原生事件按旧 generation 进审计；`PermissionAutoCancelled` 只做展示；用户在 `review` 核对完之前，UI 不提供“一键重试上一步”。`AcpConnection::close()` / `close_with_report()` 同样先进入 Stopping，`TerminateReport` 带同样的 `confirmation`。

### 1.2 停止确认标准（`StopConfirmation`）

确认的对象是**旧进程树全部结束**，不是父进程退出：

| 平台 / 模式 | `Confirmed` 的条件 | 说明 |
|---|---|---|
| Linux / macOS | 后端进程组已不存在：`kill(-pgid, 0)` 返回 `ESRCH`，或组内只剩僵尸（已退出、只等新父进程回收，不再执行代码）；**并且**停止前快照到的每个后代（pid + 启动时间相同）都已结束；在 `STOP_CONFIRM_TIMEOUT`（3 s）内轮询 | 后端以 `process_group(0)` 启动。离开进程组的后代（pi 的 bash 以 `detached` 启动，自成进程组；srt 的 bwrap 用 `--new-session`）靠快照找到、按 pid 杀掉后再确认。进程表：Linux 读 `/proc`，macOS 用 `ps -A -o pid=,ppid=,pgid=,stat=,lstart=`；读不到进程表、`kill` 返回 `EPERM` 等无法判定的情况一律不确认。已知盲区：最后一次快照之后才出生并立刻离开进程组的后代看不到 |
| Windows 完全访问（撤权场景） | 我们的 Job Object `QueryInformationJobObject(JobObjectBasicAccountingInformation).ActiveProcesses == 0` | 完全访问模式下后端直接在我们的 Job 里（挂起 → 入 Job → 恢复），可确认。无 Job、只能走 `taskkill /T /F` 兜底时一律 `FailedOrUnconfirmed`（`taskkill` 的退出码不能证明每个后代都已结束） |
| Windows 沙箱模式 | 同上：我们的 Job 已空 | **范围更小**：Job 里只有 srt 的 node 与 srt-win broker；runner 与后端由 seclogon 创建，不在我们的 Job（two-hop，7.4），其结束依赖 srt-win 的 Job 连锁关闭，**需实测**。此时 `confirmation_scope_note` = `WINDOWS_SANDBOXED_SCOPE_NOTE` 写明这一点。撤权场景停止的是完全访问进程，不受此影响 |

未确认时 `reasons` 列出具体原因（仍在运行的 pid、系统调用错误、Job 中剩余进程数），以 `fox-agent-backend:` 前缀写 stderr。

### 1.3 撤权后晚到的审批

ACP 的审批回调是异步的：取消只能清掉“当时已在表里”的审批，后端可能在 `session/cancel` 之后、进程被杀之前又发来新的 `session/request_permission`。处理：连接一旦 `begin_stopping()`，回调在**同一把锁**下看到停止标志，直接以 `cancelled` 答复后端，不登记、不发 `PermissionRequested`，只发 `PermissionAutoCancelled`。测试 `tests/acp_mock_agent.rs::late_permission_after_stopping_is_auto_cancelled_and_new_work_is_refused` 用 mock agent（提示词 `late`：收到 `session/cancel` 后 100 ms 才发审批）断言：agent 收到 `cancelled`（回显 `late-permission:cancelled`）、事件流里没有可答复的 `PermissionRequested`、`pending_permissions() == 0`、新 `prompt` / `new_session` 返回 `STOPPING_REFUSAL`、随后的撤权停止为 `Confirmed`。

## 2 fail-closed 规则

sandboxed 模式下，以下任何一项失败都**拒绝启动该后端**并返回结构化错误 `SandboxError { code, message, missing, remedy }`（serde 输出 snake_case 的 `code`；`PortError::Sandbox` 携带它）。**绝不自动退回 full_access。**

| code | 触发 |
|---|---|
| `sandbox_dependency_missing` | srt 包不在 Fox 管理目录、srt 版本 < 0.0.80、Node 缺失或 < 22.12、Linux 缺 bwrap / socat / rg、macOS 缺 rg |
| `sandbox_not_installed` | Windows 未执行 `windows-install`（查 `HKLM\SOFTWARE\sandbox-runtime`） |
| `sandbox_unavailable` | 不支持的 OS；Linux 上 bwrap 无法建立 user/net namespace（预检实跑 `bwrap --ro-bind / / --unshare-user --unshare-net -- /bin/true`）；macOS 无 `/usr/bin/sandbox-exec`；准备会话目录失败 |
| `sandbox_policy_invalid` | 授权目录不存在或无法解析；路径规范化后为 UNC / 设备路径（Windows）；受管路径在缺失部分含 `..`（第 10 节）；无授权目录；无 denyRead 根（Linux/macOS）；Windows 上授权 / deny 目标是盘根或共享根、运行时目录与可写授权目录重叠、会话变量或参数含 `"` `%` 换行；路径非绝对；授权目录 / 私有 HOME / 额外只读路径等于或包含某个 denyRead 根（会把它重新打开，例如授权 `/`）；项目 UUID 为空 / 非法 / 全零；私有 HOME 不在私有 HOME 根下；授权目录与私有 HOME 根或会话临时目录重叠；域名格式错误；会话 ID 非 `[A-Za-z0-9_-]{1,128}`；settings 目录落在沙箱可读或可写范围内；撤权重建拿到的不是 sandboxed 启动 |
| `sandbox_backend_not_installed` | 后端经 `npx` 启动（需要 npm 源与用户的 npx 缓存）；程序不是绝对路径；程序在沙箱内不可读 |
| `sandbox_path_not_owned` | Windows：srt 需要加 ACE 的路径（授权目录、私有 HOME、会话临时目录、deny 路径及其父目录）或 Fox 要加祖先元数据 ACE 的路径（7.6）当前用户拿不到 `WRITE_DAC`；或 Node / srt / 后端运行时不在 Fox 管理的运行时目录（也不在 Users 可读的系统位置）下。消息与 `missing` 指名路径（7.2） |
| `sandbox_path_exposed` | Windows：Fox 私有目录（state、私有 HOME 根、会话临时根）的 DACL 对所有本地账户开放读或写；运行时目录不是管理员拥有或有其他账户可写；`%ProgramData%\Fox\sandbox\<SID>` 所有者不是本人或 DACL 含多余 ACE（7.2、7.5） |
| `sandbox_runtime_not_installed` | Windows（第五轮）：受管运行时不在 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（如旧的 `%LOCALAPPDATA%` 布局），或 `%ProgramData%\Fox\sandbox` 不存在。remedy：“需在设置中启用沙箱（一次管理员授权）”（7.5） |
| `delivered_file_rejected` | 不是启动拒绝：`verify_delivered_file` 判定 Agent 交付的文件不合格（第 11 节） |

调用顺序：`preflight(install, probe)` → `SrtLauncher::new(ready, state_root)`（Windows 另 `.with_managed_runtime_root(<Fox 运行时目录>)`）→ `prepare_launch(mode, spec, session_id, SandboxSetup::{Ready|Unavailable}, os)` → `AcpConnection::spawn_prepared(prepared, cwd)`。预检失败时把错误放进 `SandboxSetup::Unavailable(err)`，`prepare_launch` 在 sandboxed 下原样返回、在 full_access 下忽略。

## 3 srt 配置模板

srt 的读权限默认全放行（deny-then-allow，`allowRead` 优先于 `denyRead`），写默认全拒，网络默认全拒。所以每会话 settings 在 Linux/macOS 上先 denyRead 根目录 `/`（srt README：`denyRead: ["/"]` 拒绝 `/` 下每个目录，`/proc` `/dev` `/sys` 除外），再 allowRead 需要的路径；Windows 不写 denyRead，allowRead/allowWrite 只含用户拥有的路径（第 7 节）。`SandboxPolicy::to_srt_settings()` 生成（路径一律用 `{"path": ..., "literal": true}`，不按 glob 解释，`[WIP] 项目` 这样的目录名也只表示它本身）：

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

- denyRead 默认根（`default_deny_read_roots`）：Linux / macOS `/`；Windows 无（7.1、7.2）。
- 最小系统路径（`default_system_read_only`，只读；启动时去掉本机不存在的项）：
  - Linux（实测）：`/usr` `/bin` `/sbin` `/lib` `/lib32` `/lib64` `/libx32`；`/etc` **不整体放行**，只放行 `/etc/alternatives` `/etc/ca-certificates` `/etc/crypto-policies` `/etc/gai.conf` `/etc/group` `/etc/host.conf` `/etc/hosts` `/etc/ld.so.cache` `/etc/ld.so.conf` `/etc/ld.so.conf.d` `/etc/localtime` `/etc/nsswitch.conf` `/etc/passwd` `/etc/pki` `/etc/resolv.conf` `/etc/ssl`。`/etc/hostname`、`/etc/shadow`、`/etc/environment` 等其余内容在沙箱内不存在（实测）。
  - macOS（**未实测**的起始集合）：`/usr` `/bin` `/sbin` `/System` `/Library/Apple` `/Library/Preferences/SystemConfiguration` `/private/etc/{hosts,resolv.conf,ssl,passwd,group,localtime}` `/private/var/db/timezone` `/private/var/db/dyld` `/dev`。
  - Windows：无；Users 组可读的系统位置（Program Files、Windows）不生成 allowRead。
- srt 包目录与 Node 运行时若不在上述系统路径内（例如 `/opt/fox/node`、`~/.nvm`），`SrtLauncher::effective_policy` 自动只读放行它们本身（Linux 上 srt 要在沙箱内执行 `vendor/seccomp/<arch>/apply-seccomp`，不放行会直接失败）。
- 被拒读的目录在沙箱内是**空的 tmpfs**：允许路径的上级目录名会以空目录骨架出现（例如 `/home/<用户>/…`），在拒读区域内写入只落进该 tmpfs，主机上不可见、会话结束即丢失（实测：向 Fox state 目录写入在沙箱内“成功”，主机上无文件）。srt 另默认放行 `/tmp/claude` 写入（同为 tmpfs）。
- settings 文件写在 `state_root/sandbox-sessions/<session_id>/srt-settings.json`（Unix 0600），`state_root` 必须既不可写也不可读（`wrap` 校验：不在任何可读 / 可写根之下）；`SandboxSession` 随连接释放（`close()` 或 drop）时删除会话 settings 目录**和本会话临时目录**，私有 HOME 保留。
- 启动命令：Linux/macOS `<node> <srt>/dist/cli.js --settings <file> -- <后端程序绝对路径> <参数...>`；Windows `<node> <srt>/dist/cli.js --settings <file> -c "set \"K=V\" && ... && <程序> <参数>"`（7.10）。
- 环境变量（在 launch spec 自身的 session_env 之后覆盖）：`HOME`、`XDG_CONFIG_HOME/CACHE_HOME/DATA_HOME/STATE_HOME`（私有 HOME 下）；`TMPDIR/TMP/TEMP` 与 `CLAUDE_CODE_TMPDIR` = 本会话临时目录；`NODE_USE_ENV_PROXY=1`；Windows 另设 `USERPROFILE/APPDATA/LOCALAPPDATA`；codex 设 `CODEX_HOME=<私有 HOME>/.codex`，deepseek 设 `DSH_HOME=<私有 HOME>/.dsh`；`PATH` = 后端程序目录 + Node 目录 + 系统目录，不继承用户 PATH。环境白名单（`launch::child_environment`）不变，仍先于这些覆盖生效。
  - 临时目录必须在沙箱内可读：srt 进程本身继承这些变量，把代理桥接 socket 建在自己的 `TMPDIR` 下再 bind 进沙箱；若该目录被 denyRead 覆盖，沙箱内所有经代理的请求都失败（实测报 `Proxy CONNECT aborted`）。
  - srt 会把沙箱内的 `TMPDIR` 改成 `CLAUDE_CODE_TMPDIR`（缺省 `/tmp/claude`，在 `/` 拒读下是内存 tmpfs），所以同时设置它（实测沙箱内 `TMPDIR` = 本会话临时目录）。
  - Node 内置 `fetch` 默认不读 `HTTP(S)_PROXY`，沙箱内唯一出口是 srt 代理，故设 `NODE_USE_ENV_PROXY=1`（实测 `node -e fetch(...)`：不设 `EAI_AGAIN`，设后 200；真实模型 provider 尚未在沙箱内实测）。

### 私有 HOME（已定：每个项目 × 每个后端一份）

- 布局：`PrivateHomes::new(<fox_data>/sandbox-homes).home_for(project_uuid, backend)` → `<fox_data>/sandbox-homes/<项目 UUID>/<后端 ID>`（`PrivateHomeScope` 默认且目前唯一取值 `per_project_per_backend`）。同一项目同一后端的多个会话复用同一份 HOME（原生登录与历史在这些会话间可用），跨项目、跨后端互不可见。
- 目录名只用 Host 数据库中的**稳定项目 UUID**（规范化为小写 8-4-4-4-12）加后端 ID，不再从项目路径或名称派生，因此同名项目、同名文件夹、中文或含特殊字符的路径都不会碰撞。UUID 为空、格式非法或为全零 UUID → `sandbox_policy_invalid`；FoxBackend 没有沙箱私有 HOME。
- `SandboxPolicy` 携带 `private_home_root`；`validate` 拒绝：私有 HOME 不在该根下；任一授权目录等于、包含或位于私有 HOME 根之下（即不可能把其他项目 / 其他后端的私有 HOME 授权进来）；任一授权目录与本会话临时目录重叠。沙箱内只 bind 本项目本后端那一份 HOME，同根下的其他 HOME 不可见。
- 临时目录（`TMPDIR` 等）按会话分开：`<tmp_root>/<session_id>`（默认 `tmp_root = state_root/sandbox-tmp`，可用 `SrtLauncher::with_tmp_root` 改），会话结束随 `SandboxSession` 删除。
- **Windows 布局不同**（第五轮，见 7.5）：私有 HOME 与会话临时目录在 `%ProgramData%\Fox\sandbox\<用户 SID>\{homes\<项目 UUID>\<后端>, tmp\<会话>}`（`WindowsPrivateLayout::homes_root()` / `tmp_root()`），Fox state 仍在 `%LOCALAPPDATA%\Fox\state`。Linux / macOS 保持 `<fox_data>/sandbox-homes/...` 不变。
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

`SandboxDeclaration::declare(mode, policy, os)` 给出模式、机制（`srt` 或无）、文件 / 网络强制程度、`directory_isolation`、可写 / 可读 / 拒读根与域名；`apply_to(&mut CapabilityDeclaration)` 并入后端实测能力。

**声明来源 = 最终生效配置**（第四轮）：`prepare_launch` 用 `wrap` 返回的 `SandboxedLaunch.effective_policy`（规范化后的路径、会话临时目录、启动时追加的 Node / srt 目录，即实际写进 srt settings 的那一份）生成声明，并放进 `PreparedLaunch.effective_policy`。`readable_roots` / `writable_roots` / `denied_read_roots` 分别**逐项等于** settings 的 `allowRead` / `allowWrite` / `denyRead`（同一个 `SandboxPolicy::settings_roots()`；Windows 上不含 Users 组本来就能读、不生成 ACE 的系统位置）。测试：`declaration_lists_exactly_what_the_written_settings_contain`（单元）与真实 T04 中对 settings 文件的逐项比对。此前按调用方传入的策略生成，漏掉会话临时目录、私有 HOME 之外的运行时目录。



- full_access：`sandbox_filesystem = sandbox_network = absent`，`read_scope = none`，`directory_isolation = false`，不列任何根。UI 不得出现“限定在这些文件夹”。
- sandboxed：取后端原生实测与 srt 两者较强者（排序 enforced > unverified > advisory > absent），既不隐藏原生沙箱，也不夸大 srt。
- srt 强制程度按平台实测：Linux = `enforced`（第 6 节）；macOS / Windows = `unverified`，直到工程师在该平台跑完同样的 T04。
- **读边界如实表达**：`read_scope` 由策略内容推出（`SandboxPolicy::read_scope(os)`），`read_scope_verified` 按平台实测状态给出（`srt_read_scope_verified`）。UI 文案只依据这两项（`ReadScope::ui_label`）：

| 平台 | denyRead | `read_scope` | `read_scope_verified` | UI 文案 | `directory_isolation` |
|---|---|---|---|---|---|
| Linux | `/` | `authorized_only` | 是（本轮实测） | 读取限定在授权文件夹（另含运行必需的系统路径、私有 HOME 和会话临时目录） | 是 |
| macOS | `/` | `authorized_only` | 否 | 读取限定在授权文件夹（另含运行必需的系统路径、私有 HOME 和会话临时目录；本平台待实测） | 否 |
| Windows | 无（Host 可追加用户拥有的路径） | `users_group_readable` | 否 | 沙箱账户可读取授权文件夹和系统中对所有用户开放的位置；个人文件夹按实际 ACL 保护；其他盘上对所有用户开放的目录可能可读写（本平台待实测） | 否（产品决定 B：只警告） |
| Linux / macOS，策略只拒读部分目录 | 非 `/` | `user_dirs_only` | — | 部分：仅用户目录受保护 | 否 |
| full_access | — | `none` | — | 不限制读取 | 否 |

`directory_isolation` 仅在 srt `enforced` 且 `read_scope = authorized_only` 且已实测时为真。

**Windows 暴露面警告（第五轮，产品决定 B）**：工程师实测 D 盘继承默认 ACL 的目录沙箱可读写，产品定为“只警告、不隔离”：`directory_isolation` 恒为 false。`SandboxDeclaration` 新增 `exposed_dirs`（来自 `winacl::audit_broad_exposure`，可选，由 `declaration.with_exposure(&report)` 填入）与 `exposure_warning`；`exposed_dirs` 非空时 `exposure_warning` = 常量 `WINDOWS_EXPOSURE_WARNING`：“此模式不限制在授权目录内。对所有用户开放的目录仍可能被读取或修改；本机已检测到此类目录。”个人文件夹的表述改为“按实际 ACL 保护”，不做绝对保证（用户自己放开过 ACL 的个人文件夹同样可读）。full_access 与非 Windows 忽略审计结果。

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
  - `tests/pi_backend.rs`（3 个）、`tests/codex_backend.rs`（1 个）、`tests/deepseek_backend.rs`（1 个）的真实测试仍是旧式门控：未设 `FOX_TEST_REAL_ACP=1` 时打印 `SKIPPED` 后返回，cargo 计为 `passed`。报告时必须按名单从 passed 中扣除。
- 报告格式：分列“真正执行并通过”“跳过（ignored + SKIPPED 名单）”“真实环境执行”三项，不得只报 cargo 的 passed 总数。

### 6.4 第四轮（撤权收尾，2026-10-19）

同环境（Debian 13 容器，Linux 6.12 aarch64，uid 1000，Node 22.23.3，bwrap 0.12.0，socat 1.8.0.3，srt 0.0.80，pi 0.84.2 / pi-acp 0.0.34）。

- 真实 T04 重跑通过（`real_srt_pi_acp_t04_filesystem_and_network_boundary ... ok`，19.3 s），新增两项断言：声明三张清单与 settings 文件逐项一致（含会话临时目录）；`close_with_report()` 为 `Confirmed`。**发现**：srt 的 bwrap 以 `--new-session` 运行，bwrap 及其沙箱内子树（pi-acp、pi、bash…）**不在 Fox 的进程组里**；SIGTERM 后 srt 的 node 退出时它们仍在，单靠进程组判断会误报“已结束”。这次由后代跟踪按 pid 杀掉（`killed descendant pid … that had left process group …`）后才确认。此前它们依赖 bwrap 的 `--die-with-parent` 在稍后自行结束（旧测试 2 s 后看进程表因此能通过）。
- pi 真实门控测试（非沙箱，`FOX_TEST_REAL_ACP=1`）：
  - 改动前先在 fd48532 上重测旧第 6 步：`close()`（SIGTERM → 宽限 → KILL）后 bash 已不在（pi 的 SIGTERM 处理会结束它跟踪的 detached 子进程），旧断言“bash 仍存活”失败——缺口在加了 SIGTERM 宽限后已不存在。
  - 新第 6 步（正向）：关闭前记录后端下的整棵树（launch node → npm exec → pi-acp → pi → **自成进程组**的 bash → sleep），关闭后全部结束且 `Confirmed`。通过。
  - 新测试 `real_gated_close_kills_detached_bash_even_when_pi_cannot_clean_up`：关闭前 `SIGSTOP` pi（其 SIGTERM 处理无法运行），进程组 SIGKILL 杀不到自成进程组的 bash 子树。四次运行均 `Confirmed`、无残留；其中一次后代跟踪按 pid 杀掉了仍在运行的 `sleep`（日志 `killed descendant pid … that had left process group …`），另三次该子树在检查时已结束（原因未查明，与时序有关），因此测试只断言结果、不断言一定发生按 pid 击杀；机制本身由 `proctree` 单元测试 `escaped_descendant_blocks_confirmation_until_killed` 确定性覆盖。
  - 据此 `backends/pi.rs` 的 `cancel_process_tree` 改为 `true`（Linux 实测；macOS 的 ps 快照与 Windows Job 未实测）。
- 本机前置：`fixtures/pi/pi-gated.sh` 在仓库是 100644（pi-acp 直接执行它会 `EACCES`）；`launch-pi-acp.mjs` 现在与 T04 一样，把它和 `fox-approval-gate.ts` 复制到隔离 HOME 下的 `.fox-pi-launcher/` 并给副本加可执行位，不改动检出目录。

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

**Windows**（alpha，未实测；本节按 srt 0.0.80 源码核实，2026-10-19 第三轮重写，第五轮补 7.5–7.8）

7.1 权限模型（源码核实）
- srt-win 以独立本地账户 `srt-sandbox` 运行后端：broker（调用用户、非提权）解密该账户密码后经 Secondary Logon 的 `CreateProcessWithLogonW` 启动 `srt-win runner`，runner 再以受限令牌（LUA、Administrators 改 deny-only、登录 SID 改 deny-only、Medium IL、只保留 `SeChangeNotifyPrivilege`）在自己的 Job 里启动目标进程（`vendor/srt-win-src/src/logon.rs`、`launch.rs`、`token.rs`）。
- 文件边界 = NTFS DACL。`srt-sandbox` 对调用用户自己的文件没有任何固有权限（`C:\Users\<用户>` 的默认 ACL 只授予本人、SYSTEM、Administrators），srt 靠给该 SID 加显式 ACE 打开路径：allowRead → `(A;OICI;READ|EXECUTE)`，allowWrite → `(A;OICI;MODIFY 去掉 FILE_DELETE_CHILD)`，denyRead/denyWrite → 目标上 `(D;OICI;mask)` **且父目录上** `(D;OICI;FILE_DELETE_CHILD)`（`acl.rs`、`cli.rs` 的 `acl grant|stamp`）。
- 这些 ACE 由 broker 以**调用用户身份、不提权**写入：`SetNamedSecurityInfoW(DACL)`，需要对象上的 `WRITE_DAC`。普通用户只在自己拥有（所有者隐含权限）或被显式授予的对象上有 `WRITE_DAC`；`C:\Users`、`C:\Program Files\nodejs`、`C:\`、`D:\` 根都不是。这就是实机的 `SetNamedSecurityInfoW WIN32_ERROR=0x00000005`：第一次失败在给原 Node 目录加 allowRead，第二次失败在给 `C:\Users` 加 denyRead（3 个目标、4 项 ACE 中 1 项失败），srt 按批次回滚。
- 结论：Windows 上 `denyRead C:\Users` 既多余（沙箱账户本来读不到个人目录）又不可执行；**真正的暴露面是 DACL 对所有本地账户开放的位置**——Users / Authenticated Users / Everyone / INTERACTIVE（沙箱账户属于这些组）：`Program Files`、`Windows`、`ProgramData` 的部分子树、以及其他固定盘上从盘根继承了 `Authenticated Users: Modify` 的目录。注意这对**读和写**都成立：srt 源码明确提到数据盘根的 `Authenticated Users:(M)`，并只对 13 个系统世界可写目录（`%ProgramData%`、`%PUBLIC%`、`Windows\Temp` 等，`ambient.rs`）在安装时加写拒绝，**盘根不在其中**。所以 `D:\` 下继承默认 ACL 的项目目录，沙箱账户可读也可写（比“读暴露”更严重，需实测确认）。

7.2 Fox 的 Windows 策略（`sandbox.rs`）
- `default_deny_read_roots(Windows)` = 空；不再生成 `denyRead C:\Users`。`validate` 在 Windows 上允许 deny 列表为空。
- srt 只对用户拥有的路径生成 ACE：授权目录、私有 HOME、会话临时目录、Host 追加的 deny 路径（第五轮起运行时目录不再生成 ACE，见下一条）。`users_readable_roots`（`windows_users_readable_roots()`：`%ProgramFiles%`、`%ProgramFiles(x86)%`、`%ProgramW6432%`、`%SystemRoot%`）下的路径一律**不生成** allowRead（Users 组本就可读，用户也写不了 ACE）。
- 运行时位置要求（第五轮改，详见 7.5）：Node、srt、各后端运行时及其他只读额外路径必须在 Fox 管理的**机器级**运行时目录 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（管理员拥有、Users 读取和执行）下，或在上述 Users 可读的系统位置下；两者都**不生成 ACE**（`effective_policy` 把运行时根并入 `users_readable_roots`，settings 的 allowRead 里不出现它）。其余位置（如 `D:\tools\node`、nvm、`%APPDATA%\npm`）→ `sandbox_path_not_owned`。旧布局 `%LOCALAPPDATA%\Fox\runtime` → `sandbox_runtime_not_installed`。运行时目录不得与任何可写授权目录重叠 → `sandbox_policy_invalid`。
- 预检（`SrtLauncher::windows_ace_plan`，在 `wrap` 写任何东西之前）：
  1. 对每个 ACE 目标（grant、deny 及其父目录）用 `WindowsAclProbe::check_write_dac` 检查当前用户能否拿到 `WRITE_DAC`（真实实现：`CreateFileW(WRITE_DAC|READ_CONTROL, FILE_FLAG_BACKUP_SEMANTICS)`，与 `SetNamedSecurityInfoW` 打开对象的权限一致；不存在的路径查最近的已存在祖先）。任一失败 → `sandbox_path_not_owned`，消息与 `missing` 指名路径和 Win32 错误码，fail-closed。
  0. （第五轮）运行时根检查 `winruntime::check_windows_runtime_root`：不在用户目录（`%USERPROFILE%`、`%LOCALAPPDATA%`、`%APPDATA%`）内，否则 `sandbox_runtime_not_installed`（remedy“需在设置中启用沙箱（一次管理员授权）”）；所有者为 Administrators / SYSTEM / TrustedInstaller，且作用于该目录本身的允许 ACE 中，除这三者与 CREATOR OWNER 外任何 SID（Users / Authenticated Users / Everyone / INTERACTIVE 在消息中点名）都没有写类权限（`WRITE_CLASS_MASK`：写数据、追加 / 建子目录、写 EA、写属性、删除、删子项、`WRITE_DAC`、`WRITE_OWNER`、GENERIC_WRITE / ALL），否则 `sandbox_path_exposed`；DACL / 所有者读不到也按 exposed 处理。测试环境变量 `FOX_RUNTIME_ROOT` 可覆盖位置，但同样受这些规则约束。
  2. Fox 私有目录（state、私有 HOME 根、会话临时根）的 DACL 若对所有本地账户开放读或写 → `sandbox_path_exposed`。
  3. 盘根 / 共享根作为授权或 deny 目标 → `sandbox_policy_invalid`（srt 自身也不在盘根加 ACE：会改写整卷 DACL）。
  4. （第五轮）祖先元数据 ACE（7.6）：每个要加 ACE 的祖先同样做 `WRITE_DAC` 预检，失败 → `sandbox_path_not_owned`，消息说明“需要能读到授权目录以上直到用户配置文件根的各级目录属性”。计划放在 `WindowsAcePlan.ancestors`，经 `SandboxedLaunch` / `PreparedLaunch.windows_ancestor_aces` 交给 Host。
- 读范围如实声明：Windows `read_scope = users_group_readable`，`read_scope_verified = false`，`directory_isolation = false`（产品决定 B）。UI 文案（`ReadScope::ui_label`，第五轮改）：“沙箱账户可读取授权文件夹和系统中对所有用户开放的位置；个人文件夹按实际 ACL 保护；其他盘上对所有用户开放的目录可能可读写（本平台待实测）”；检测到暴露目录时另显示 `WINDOWS_EXPOSURE_WARNING`（第 5 节）。
- `SandboxPolicy::is_readable` 在 Windows 上表示“策略授予或系统位置可读”；盘上其他 DACL 宽松的目录无法按路径推断，交给暴露面审计。

7.3 暴露面审计（`winacl::audit_broad_exposure`，只报告不修改）
- 枚举固定盘（`GetLogicalDrives` + `GetDriveTypeW == DRIVE_FIXED`）的根及其一级子目录，读 DACL（`GetNamedSecurityInfoW`），列出允许 ACE 授予 Users / Authenticated Users / Everyone / INTERACTIVE 读（`FILE_READ_DATA`/`GENERIC_READ`/`GENERIC_ALL`）或写（第五轮起为整个 `WRITE_CLASS_MASK`，含删除、`WRITE_DAC`、`WRITE_OWNER` 等）的目录；结果经 `SandboxDeclaration::with_exposure` 进入声明的 `exposed_dirs`；跳过 inherit-only ACE；不扣减 deny ACE（只会多报不会漏报）；NULL DACL 记为全开放。返回 `ExposureReport { supported, drives, exposed[{path, access{read, write, groups}}], errors }`，供 UI 警告。不扫描二级以下（更深处可能另设 DACL）。非 Windows 编译通过，`supported = false`。

7.4 进程清理：Job Object 与 two-hop 风险（`acp.rs`）
- 实现：`AcpConnection::spawn` 在 Windows 上以 `CREATE_SUSPENDED` 启动子进程 → 新建 Job（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，不允许 breakaway，句柄不可继承）→ `AssignProcessToJobObject` → 用 Toolhelp 快照找到该进程的线程并 `ResumeThread`。进程在入 Job 之前无法运行，消除了“先 spawn 再 attach”的竞态（std/tokio 不返回主线程句柄，故用 Toolhelp 恢复；任何一步失败即杀掉挂起的子进程并报错，不在 Job 外运行）。
- 关闭（`close` / `close_with_report`，先进入 Stopping）：Windows 先丢弃命令通道（ACP 连接结束、后端 stdin 关闭，给 srt 机会自己 `reset()` 撤销 ACE），等 `TERMINATE_GRACE`（3 s），再 `TerminateJobObject`；只有没有 Job 或 `TerminateJobObject` 失败时才兜底 `taskkill /PID <pid> /T /F`，并检查退出码（128 = 进程已不在）。之后在 `STOP_CONFIRM_TIMEOUT` 内轮询 `QueryInformationJobObject(JobObjectBasicAccountingInformation).ActiveProcesses`，为 0 才算 `Confirmed`（无 Job 走 taskkill 时一律未确认）。结果放进 `TerminateReport { confirmation, confirmation_scope_note, force_killed, job_terminated, notes, .. }`，同时以 `fox-agent-backend:` 前缀写 stderr。沙箱模式下“确认”只覆盖我们的 Job（1.2）。连接被直接 drop 时，Job 句柄关闭同样触发 kill-on-close。
- **two-hop 风险（源码核实）**：sandboxed 模式下我们的 Job 里只有 srt 的 node 进程和 srt-win broker（libuv 把 node 的子进程放进它自己的嵌套 Job，`SILENT_BREAKAWAY_OK` 只脱离 libuv 的 Job，不脱离不允许 breakaway 的外层 Job——此点按 Windows 嵌套 Job 文档推断，需实测）。runner 是 seclogon 服务代为创建的（`CreateProcessWithLogonW`），**不在我们的 Job 里**；后端本体由 runner 以 `CREATE_BREAKAWAY_FROM_JOB` 放进 runner 自己的 Job，**也不在我们的 Job 里**。清理链依赖 srt-win 自身：我们 `TerminateJobObject` → broker 死 → broker 持有的 broker→runner Job（kill-on-close）句柄关闭 → runner 死 → runner 的 Job（kill-on-close、不允许 breakaway）句柄关闭 → 后端整棵树死。若 broker→runner 的 assign 因 seclogon Job 不支持嵌套而失败（srt-win 打印 `broker→runner Job assign not supported`），外部杀 broker 后 runner 不会被杀（它的 `SpawnedChild` 守卫只在 broker 进程内正常退出时生效），只能等后端因 stdin 断开自行退出；srt 注释给出的兜底是**提权** `taskkill /F /FI "USERNAME eq srt-sandbox"`。
- ACE 回收：硬杀 node 会跳过 srt 的 `reset()`，ACE 留在授权目录上，直到下一次任何 `srt-win acl` 操作的崩溃恢复按“持有者 PID 已死”清理（`with_init_lock`），或手动 `srt-win acl recover`。持有者 PID 被复用时清理会推迟。故关闭时先走 stdin 关闭的温和路径。

7.5 运行时与私有目录布局（第五轮）
- **受管运行时**：Windows 只接受 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（`winruntime::default_windows_runtime_root`；`windows_runtime_root` 允许测试用 `FOX_RUNTIME_ROOT` 覆盖，覆盖值同样要求管理员拥有、Users 不可写）。由启用脚本（7.7）以管理员复制并设 ACL：去继承，Administrators / SYSTEM 完全控制，Users 读取和执行，所有者 Administrators。沙箱账户属于 Users，可直接读，Fox 不为它生成任何 ACE（用户对管理员拥有的目录也没有 `WRITE_DAC`）。包内布局：`node\node.exe`、`srt\node_modules\@anthropic-ai\sandbox-runtime\…`、`backends\<后端>\…`、`dsh-patches\*.yml`（本 crate `runtime-assets/dsh-patches`，与 `deepseek::patch_variants()` 逐字节一致，有单元测试守护）；实测夹具可放 `t04-fixtures\`。后端版本随运行时包固定，升级需重跑启用脚本（一次 UAC）。
- 第三轮实机两个问题的处理：私有运行时（`%LOCALAPPDATA%`）下 srt 的 WFP 自检 helper 在 ACL 授权前就以 `srt-sandbox` 启动（`CreateProcessWithLogonW` 0x80070003 = 路径不可达）→ 运行时迁到 Program Files 后沙箱账户天然可达；`%LOCALAPPDATA%` 下的运行时返回结构化错误 `sandbox_runtime_not_installed`（remedy“需在设置中启用沙箱（一次管理员授权）”）。Program Files 下 Node 24 对 `C:/Users/<u>/AppData` 做 realpath/lstat 报 EPERM → 私有 HOME / 临时目录移出用户 profile（下一条），授权目录仍在 profile 内的由 7.6 的祖先 ACE 解决。
- **私有 HOME / 会话临时目录**：`%ProgramData%\Fox\sandbox\<用户 SID>\{homes\<项目 UUID>\<后端>, tmp\<会话>}`（`WindowsPrivateLayout`）。父目录 `%ProgramData%\Fox\sandbox` 由启用脚本以管理员创建：Administrators / SYSTEM 完全控制；Users 只有“建子文件夹 + 读属性 + 遍历 + 同步”且只作用于该目录本身（不能列出、不能读他人的子目录）；CREATOR OWNER `(OI)(CI)(IO)` 完全控制。`prepare_windows_private_root` 在首次使用时：
  1. 目录名必须等于当前令牌用户 SID（`current_user_sid()`，`GetTokenInformation(TokenUser)`），否则 `sandbox_policy_invalid`；
  2. 父目录不存在 → `sandbox_runtime_not_installed`；
  3. 用 `CreateDirectoryW` + 显式安全描述符 `O:<SID>D:P(A;OICI;FA;;;<SID>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` 建 `<SID>` 目录（去继承，只留本人、SYSTEM、Administrators）；已存在则**校验而不信任**（父目录允许任何用户建子目录，别人可能抢先建一个以我 SID 命名的目录）：所有者必须是本人，DACL 不得含继承 ACE，允许 ACE 只能是本人 / SYSTEM / Administrators，外加 Fox 自己为已记录 srt SID 加的祖先 ACE（掩码恰为 `ANCESTOR_ACE_MASK`、无标志）；再跑原有的 `broad_access` 检查。任一不通过 → `sandbox_path_exposed`，拒绝使用；
  4. 建 `homes`、`tmp`（继承 `<SID>` 目录的 ACL）。
  Host 用 `PrivateHomes::new(layout.homes_root())` 与 `SrtLauncher::with_tmp_root(layout.tmp_root())`。Fox state 仍在 `%LOCALAPPDATA%\Fox\state`。Linux / macOS 布局不变。

7.6 祖先元数据 ACE（第五轮，`winancestor.rs`，cfg 无关的逻辑 + `winacl` 的 Windows 实现）
- 原因：srt 0.0.80 的 `acl grant` 只有 read（READ+EXECUTE）与 write，且固定 `(OI)(CI)` 继承，无法表达“只能读这一级目录的属性”；而 Node 解析模块 / realpath 时逐级 `lstat` 祖先，授权目录在 `C:\Users\<u>\Documents\…` 下时，沙箱账户在 `C:\Users\<u>` 等祖先上没有任何权限 → EPERM。
- 计算（`ancestor_chain` / `plan_ancestor_aces`）：对 srt 本会话要授予的每个目标（`grant_read` ∪ `grant_write`），取遍历基点到目标之间的每级祖先（不含目标本身）：在用户 profile 下 = 从 profile 根（含）往下；在 `%ProgramData%\Fox\sandbox` 下 = 基点以下各级（不含基点）；其他位置 = 盘根以下、`broad_access` 显示 broad 组不可读的各级（读不到 DACL 也算，宁可多一条 ACE）。每级加一条 `ACCESS_ALLOWED`，SID = srt 沙箱账户，掩码只有 `FILE_READ_ATTRIBUTES | SYNCHRONIZE`（`ANCESTOR_ACE_MASK` = 0x00100080），**无继承标志**：可 `lstat`，不能列目录、不能读内容。SID 从 `HKLM\SOFTWARE\sandbox-runtime` 的 `SandboxUserSid` 读取（`winruntime::read_srt_registry`，`reg query … /reg:64`；`MarkerVersion` 是 srt 写入的提交点，缺失或 SID 不是本地账户 SID `S-1-5-21-…` → `sandbox_not_installed`，fail-closed）。
- 写法：`GetFileSecurityW` 取 DACL → 新 ACL = 原有 ACE 原样复制，新 ACE 插在开头的显式 deny 之后（规范顺序）→ `SetFileSecurityW`，保留 `SE_DACL_PROTECTED` / `SE_DACL_AUTO_INHERITED`。**不用** `SetNamedSecurityInfoW`：后者会把继承重新传播到整棵子树（在 profile 根上就是整个用户目录）。删除只删 SID、掩码、无标志三者完全一致的 ACE，其他 ACE 一概不碰。写之前同样做 `WRITE_DAC` 预检；用户对 profile 根没有 `WRITE_DAC` → fail-closed 并说明。
- **与 srt 的冲突**（srt 源码 626a5967 `state_db.rs` `recompose_at` → `acl.rs` `apply_sandbox_aces`）：srt 每次收敛某路径时，先删掉该路径上沙箱 SID 的**所有**显式 ACE（allow 与 deny），再写自己那一组；对象是 `working_aces` 里该路径的全部记录，触发点是本用户任何 broker 的 grant / stamp / release，以及每次拿锁时的崩溃恢复（死掉的持有者的路径会被重新收敛）。因此：
  (a) 从祖先集合中剔除 srt 本会话会操作的路径（grant / deny 目标与每个 deny 的父目录，srt 在父目录写 `deny_fdc`）以及位于任一 grant 目标之内的路径——这些地方 srt 已给 READ+EXECUTE（超集），Fox 不加；
  (b) Fox 在启动 srt **之前**加 ACE；
  (c) 全局引用台账 `%LOCALAPPDATA%\Fox\state\win-ancestor-aces.json`（`AncestorLedger`，文件锁 `win-ancestor-aces.json.lock`），每条记录 path、SID、引用它的会话 id；会话关闭减引用，归零才移除 ACE；先写台账再改 ACL（与 srt 的 record-first 一致）；
  (d) 其他并发会话的 srt 可能直接授权了同一路径并在启动 / 关闭时把 Fox 的 ACE 抹掉：Fox 内所有 srt 会话的启动与关闭都在 `AncestorAceCoordinator::transition()` 返回的 `SrtTransition` 内进行（同一把锁，跨 Fox 进程也有效：`File::lock` 在 Windows 上是整段 `LockFileEx`，同进程的两个句柄也互斥），每次以 `finish()` 结束，对台账做一次 reconcile（缺失的补上）；Fox 改 DACL 的那几步还同时持有 srt 自己的 `%LOCALAPPDATA%\sandbox-runtime\.acl-init.lock`（srt 用 `LockFileEx` 锁第 0 字节，Fox 的整文件锁与之重叠），避免与任何 srt 客户端（包括非 Fox 程序）的收敛交错；
  (e) 崩溃恢复：Fox 启动时调用 `recover_ancestor_aces(&coordinator, &live_sessions)`（启动时没有存活会话，即移除台账中全部 ACE）；台账损坏 → 拒绝启动沙箱（不从空台账开始而忘掉要撤销的 ACE）。
- 残余风险：非 Fox 的 srt 客户端在 Fox 两次 transition 之间收敛了同一路径，Fox 的 ACE 会缺失到下一次 transition 才补上（期间该会话的 Node 可能再报 EPERM）；`recover` 会在 Fox 下次启动时补齐 / 清理。
- Host 接线（第 2 步）：
  ```text
  let mut t = coordinator.transition()?;              // 全局锁
  t.begin_session(id, &srt_sid, &prepared.windows_ancestor_aces)?;
  let conn = AcpConnection::spawn_prepared(prepared, cwd).await?;  // 等到 Initialized
  t.finish();                                         // reconcile + 解锁
  … 关闭：let mut t = coordinator.transition()?; conn.close_with_report().await; t.end_session(id)?; t.finish();
  ```
- 单元测试（Linux 可跑，ACL 调用用 trait mock）：祖先计算（中文与空格路径、大小写、profile 根本身、ProgramData 链、其他盘）、剔除 srt 目标、台账引用计数 / 归零 / 持久化、srt 抹掉后 reconcile 补回且不碰 srt 自己的 ACE、崩溃恢复、`WRITE_DAC` / 坏 SID fail-closed 与中途失败回滚、transition 互斥。

7.7 启用脚本、安装记录与卸载边界（第五轮，取代原 7.7）
- 脚本位置：`crates/fox-agent-backend/scripts/windows/enable-sandbox.ps1` 与 `disable-sandbox.ps1`。UTF-8 **带 BOM**、LF 换行（Windows PowerShell 5.1 对无 BOM 的脚本按系统 ANSI 代码页读，中文会乱码；LF 对解析器没有影响）；`.gitattributes` 对 `*.ps1` 显式 `text eol=lf`。不使用 PS 7 专有语法（已用 PowerShell 7.4.6 解析器 + PSScriptAnalyzer 1.25.0 的 `PSUseCompatibleSyntax/Commands/Types`（目标 5.1）检查通过，未在 Windows 上实跑）。提权进程可能是另一个管理员账户（标准用户输入管理员凭据），所以调用用户相关路径全部由参数传入。结果写 JSON 文件（`-ResultPath`，UTF-8 无 BOM），日志写 `%ProgramData%\Fox\logs\*.log`。
- `enable-sandbox.ps1 -PackagePath <zip> -ExpectedSha256 <hex> -Version <v> -ResultPath <file>`，一次提权：先把 zip 复制到仅管理员可写的暂存目录再算 SHA256（避免校验后被替换）→ 解压、检查 `node\node.exe` 与 srt `cli.js` 存在 → 整体移动到 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（已有同哈希版本则跳过复制；不同哈希先挪到备份、成功后删除）→ 设运行时 ACL → 建 `%ProgramData%\Fox`（Users 只读）与 `%ProgramData%\Fox\sandbox`（7.5 的 ACL；若已存在但所有者不是管理员 / SYSTEM，先 `takeown /A` 收回，这是 Fox 自己的命名空间）→ 记录运行前 `HKLM\SOFTWARE\sandbox-runtime` 是否已有完整安装 → `node cli.js windows-install --proxy-port-range 60080-60089`（不加 `--force`，不覆盖别人的不同配置）→ 读回 `SandboxUserSid`、`MarkerVersion`、`CreatedAt` 与 srt 包版本 → 输出 `{ok, version, runtimeRoot, privateBase, srt:{alreadyInstalled, packageVersion, sandboxUserSid, markerVersion, createdAt}, log}`。幂等；失败时删除本次复制的运行时目录、恢复备份、清暂存；不改任何其他目录的 ACL。
- Fox 读结果：`winruntime::apply_enable_result(state_root, &EnableScriptResult)` 写 `srt-sids.json`（`record_srt_sid`，只追加）与 `srt-install.json`（`FoxSrtInstallRecord`）。“由 Fox 安装”只在 srt 原本未安装的那一次判定为真；之后重跑脚本看到“已安装”时，只有记录中的 `CreatedAt` 与 SID 与这次一致才保留真值。Fox 每次检测到 srt 安装（启动、启用后）都应调用 `record_srt_sid`。
- **卸载边界**（`winruntime::decide_srt_uninstall`，脚本内同样规则）：只有同时满足——(1) 记录表明 Fox 安装；(2) 注册表仍是那一次安装（`CreatedAt`、SID、`MarkerVersion` 一致）；(3) 运行时里 srt 包版本与记录一致；(4) 没有活跃 srt 会话；(5) 没有其他使用者证据——才执行 `windows-uninstall` 并删除 srt 留下的 `%ProgramData%\sandbox-runtime` 与**调用用户自己的** `%LOCALAPPDATA%\sandbox-runtime`；否则保留并在结果中列出全部原因。“无法判断”一律按“有证据”处理。
- 判断方法：活跃会话 = 以沙箱账户运行的进程（`Get-Process -IncludeUserName`，需提权，在脚本的提权阶段做）+ 命令行含 `sandbox-runtime` / `srt-win` 的进程（`Win32_Process`，可发现其他程序的 broker）+ Fox 祖先台账仍有会话。其他使用者 = 其他用户 profile 下存在 `AppData\Local\sandbox-runtime\state.db`（`Win32_UserProfile` 枚举）。**state.db 能否判断**（srt 源码 626a5967 `state_db.rs`）：会话库是每用户一份的 SQLite，表 `brokers(pid, process_create_time, started_at)`、`ace_holders(canonical_path, kind, pid, want_mask)`、`working_aces(canonical_path, kind, file_id, mask)`、`placeholders(canonical_path)`；**没有任何字段记录客户端程序**，只有 broker 的 pid 与创建时间，所以无法区分同一 Windows 用户下的 Fox 与其他 srt 客户端（如其他基于 srt 的工具）；其他用户的 state.db 在各自 profile 内。结论：调用用户自己的部分默认“无法排除”→ 保留，除非用户在 Fox 界面明确确认本账户没有其他使用 sandbox-runtime 的程序（脚本参数 `-ConfirmNoOtherSrtClients`）。
- `disable-sandbox.ps1 -StateDir <%LOCALAPPDATA%\Fox\state> -InvokingUserProfile <%USERPROFILE%> -Version <v> -ResultPath <file> [-RemoveRuntime] [-ConfirmNoOtherSrtClients]`：按上述判断后才 `windows-uninstall`；`-RemoveRuntime` 在其后删除运行时目录；**不删任何用户的私有 HOME**（`%ProgramData%\Fox\sandbox` 一律不动）。卸载前 Fox 应先跑 7.8 的所有权修复与 `recover_ancestor_aces`。
- srt 安装时在 13 个世界可写系统目录加沙箱 SID 的写拒绝 ACE（实机记录为 11 处），卸载时移除。

7.8 所有权修复（第五轮，`winowner.rs`）
- 沙箱内新建的文件 / 目录所有者是 `srt-sandbox` 的 SID，srt 卸载后成为孤儿（第三轮实机 7 个对象）。`repair_sandbox_ownership(roots, known_srt_sids, probe, writer) -> OwnershipReport`：只遍历传入的根（授权目录与私有 HOME 根），只处理所有者属于“Fox 记录的历代 srt SID”（`srt-sids.json`，`known_srt_sids(state_root)`）的对象，改为当前用户（`SetNamedSecurityInfoW(OWNER_SECURITY_INFORMATION)`，依赖 `WRITE_OWNER`，用户经其文件夹继承来的完全控制即有）；其他 SID（包括别的孤儿 SID）一律不动；不跟随重解析点（Windows 上任何 `FILE_ATTRIBUTE_REPARSE_POINT`，含符号链接、junction 和其他标签；既不改也不进入）；返回逐项报告（`repaired` / `failed` / `skipped_reparse_point` / `owner_unreadable`，另计 `scanned`、`untouched`、`errors`）。
- 触发点只写 API 与文档：卸载 srt 之前、重装 srt 之后、用户手动；**不在每次会话后自动跑**。

7.9 网络
- WFP 在 `ALE_AUTH_CONNECT` 拦截 `srt-sandbox` SID 的一切出站连接，只放行回环上的代理端口段（默认 60080–60089，须与 `windows-install --proxy-port-range` 一致）；直连公网 IP、直连回环其他端口都被拒（curl exit 7）。
- **系统 DNS 解析不受同等限制**（srt README Known limitations）：`getaddrinfo()` 由以 NETWORK SERVICE 运行的 Dnscache 服务完成，名字解析会成功（随后的 connect 仍被拦）；自己发 UDP/53 的工具（nslookup、dig）被拦。DNS 查询本身可作为外泄通道，**不能宣称 Windows 上网络完全封闭**。schannel 的 CRL/OCSP 吊销检查被 WFP 拦截（`CRYPT_E_REVOCATION_OFFLINE`）。
- 代理令牌在 runner 命令行中可见（`HTTP_PROXY=http://srt:<token>@127.0.0.1:...`），同机其他本地主体可读。

7.10 启动命令与环境变量（源码核实，影响很大）
- srt CLI 在 Windows 上把 `--` 后的 argv 用 POSIX 单引号规则拼成字符串，再交给 `cmd.exe /d /s /c` 执行：含反斜杠的路径会被包成 `'C:\...'`，cmd 不认单引号，必然失败。srt-win 给子进程的是 `srt-sandbox` 账户的全新 profile 环境，只叠加 broker 的 `PATH`、`PATHEXT`、代理变量与 git 配置变量：Fox 设置的 `HOME/USERPROFILE/CODEX_HOME/DSH_HOME/TEMP/NODE_USE_ENV_PROXY` 和 `PI_ACP_PI_COMMAND` 等原本都传不进去。
- 修正：Windows 上 `wrap` 改用 `srt -c "<cmd 行>"`，`windows_command_line()` 生成 `set "K=V" && ... && "<程序>" <参数>`（srt-win 对 `/c` 之后原样透传）。含 `"`、`%`、换行的值或参数无法安全经过 cmd → `sandbox_policy_invalid`。名字含 `KEY/TOKEN/SECRET/PASSWORD` 的变量**绝不**写进命令行（进程列表可见）；Host 环境中的凭据变量在 Windows 沙箱内本来也拿不到，所以 **Windows 沙箱模式下后端只能用私有 HOME 内的原生登录**。以 shell API 取已知文件夹的程序仍会拿到 `srt-sandbox` 自己的 profile。
- （第五轮）Windows 沙箱模式下每个后端（pi-acp / pi、codex-acp、dsh 都经 `.cmd` 启动 Node）的会话变量里加 `NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main`（`launch::WINDOWS_SANDBOX_NODE_FLAGS`，由 `SrtLauncher::sandbox_env` 写入 `set` 前缀，Node 允许这两个旗标出现在 `NODE_OPTIONS` 中并传给其子 Node 进程）。**只是辅助**：模块解析不再 realpath，可少一部分祖先 `lstat`；真正让 profile 链可 `lstat` 的是 7.6 的祖先 ACE，后端自己调用的 `fs.realpath` 不受影响。

**通用**
- `SandboxPolicy::validate` 强制：至少一个 denyRead 根（Windows 除外）；不允许 allowRead 一个 denyRead 根或其祖先（Linux/macOS 上即不允许授权 `/`）；授权目录不得与私有 HOME 根或会话临时目录重叠。
- 网络按域名过滤，不检查流量内容；白名单内域名（如 `chatgpt.com`）本身仍可能被用来外发数据。
- Linux 上 Unix socket 由 seccomp 阻止创建；回环在沙箱自身的网络命名空间内可用，但到不了主机的回环服务（实测直连主机回环端口 `exit 7`）。
- 私有 HOME 中的原生登录凭据对该后端进程可读（见第 3 节“认证”）。
- 一次审批只批准发起调用；命令内部行为由沙箱范围约束（第 2 节原则不变）。

## 8 待办与待验证

- [ ] macOS：用同一测试（`tests/sandbox_srt.rs` 的 Linux 门控需放开或复制一份）跑 T04，验证 denyRead `/` 与系统路径集合，通过后把 `srt_enforcement(MacOs)` 改为 `Enforced`、`srt_read_scope_verified(MacOs)` 改为真。
- [ ] Windows：按第 9 节清单实测（真实 T04 `real_srt_pi_acp_t04_windows`、sandboxed dsh、Job/two-hop 清理、ACE 回收、暴露面审计、DNS）；通过后再评估 `srt_enforcement(Windows)`。盘根加 denyRead 的方案已放弃（普通用户无 `WRITE_DAC`，srt 也不在盘根加 ACE）。
- [ ] codex / deepseek 及真实模型 provider 在沙箱内的真实 turn：域名白名单是否够用（codex ChatGPT 登录、令牌刷新；dsh 的遥测或插件下载）；`NODE_USE_ENV_PROXY` 是否足以让 pi 的 provider 请求走代理。
- [ ] Fox 管理目录内的后端安装并锁定版本（替代 `npx -y`），以及 `launch_spec` 在 sandboxed 下指向已安装的程序。
- [ ] 运行中网络收紧经 `--control-fd` 即时下发（可选）。
- [ ] Desktop Host 接线：会话创建 / 继续路径调用 `effective_permission_mode`、`recheck_permission_mode`、`PrivateHomes::home_for`（项目 UUID 取自 Host 数据库）、`prepare_launch`、`spawn_prepared`；撤权按第 1.1 节契约调用 `stop_for_revocation` / `rebuild_after_revocation`；UI 依据 `read_scope` / `read_scope_verified` 与 `SandboxError.remedy` 展示。
- [x] ~~pi 第 6 步过时断言~~：第四轮已改为正向验证并实测（6.4），`cancel_process_tree = true`。
- [ ] Windows：停止确认（`ActiveProcesses == 0`）、晚到审批、junction 规范化、交付校验在真机上跑（第 9 节 12–14）。
- [ ] macOS：`ps` 快照的后代跟踪与停止确认实测（pi 的 detached bash、srt sandbox-exec 子树）。
- [ ] （第五轮）Desktop Host 接线：启用 / 停用脚本的调用与结果读取（`apply_enable_result`）；每次检测到 srt 安装时 `record_srt_sid`；会话启动 / 关闭包在 `AncestorAceCoordinator::transition()` 内（7.6 的接线示意）；Fox 启动时 `recover_ancestor_aces`；`prepare_windows_private_root` + `PrivateHomes::new(layout.homes_root())` + `with_tmp_root(layout.tmp_root())`；UI 展示 `exposed_dirs` / `exposure_warning`；卸载流程先 `repair_sandbox_ownership`、`recover_ancestor_aces`，再调 `disable-sandbox.ps1`。
- [ ] （第五轮）运行时包的构建脚本（组装 node / srt / backends / dsh-patches 并生成 SHA256）尚未编写；Fox 安装包需内置哈希。
- [ ] Desktop Host 接线：交付文件在展示前调用 `verify_delivered_file(prepared.effective_policy, path)`；UI 依据 `PermissionAutoCancelled`、`StoppedUnconfirmed.remedy` 展示。

## 9 Windows 实测清单（第五轮，给工程师）

以普通用户（非管理员）运行 Fox / cargo，只有启用 / 停用脚本经 UAC 提权；记录 OS 版本、杀软状态（火绒 / Defender）、srt 0.0.80、Node、pi 0.84.2 / pi-acp 0.0.34、dsh 0.2.1-rc.2。第三轮的清单已由第三轮报告覆盖，本轮清单针对第五轮改动；每项原样保存 `MEASURE` 行与脚本日志（`%ProgramData%\Fox\logs`）。

1. 编译与单元测试：`cargo test --locked -p fox-agent-backend --features acp --no-fail-fast`。预期第三轮 4 个夹具失败（`capability_declaration_follows_the_mode`、`authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp`、`chinese_paths_are_literal_and_do_not_affect_the_home_name`、`settings_deny_the_root_and_reallow_only_granted_and_system_paths`）消失：Linux 规则用例在 Windows 上不编译（`cfg(not(windows))`），对应的 Windows 变体（`windows_declaration_is_partial_and_carries_the_exposure_warning`、`windows_authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp`、`windows_chinese_profile_paths_are_literal_and_get_ancestor_aces`、`windows_settings_grant_only_user_paths_and_deny_nothing`）在所有平台都跑。需 Rust ≥ 1.89（`std::fs::File::lock`）。
2. 运行时包：在任意目录组装 zip，顶层 `node\`（含 `node.exe`）、`srt\`（`npm --prefix srt install @anthropic-ai/sandbox-runtime@0.0.80 --ignore-scripts`）、`backends\pi`（`pi-acp@0.0.34 @earendil-works/pi-coding-agent@0.84.2`）、`backends\dsh`（`@deepseek-ai/dsh@0.2.1-rc.2`）、`dsh-patches\`（复制本 crate `runtime-assets/dsh-patches/*.yml`）、`t04-fixtures\`（`fixtures/pi/pi-gated.cmd`、`fixtures/pi/fox-approval-gate.ts`、`fixtures/sandbox/scripted-cmd-provider.ts` 放在 `t04-fixtures\sandbox\`）。`Get-FileHash` 取 SHA256。
3. **一次 UAC 是否成立**：普通用户在 Windows PowerShell 5.1 中执行 `Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','<repo>\crates\fox-agent-backend\scripts\windows\enable-sandbox.ps1','-PackagePath','<zip>','-ExpectedSha256','<hex>','-Version','0.1.0','-ResultPath','%LOCALAPPDATA%\Fox\state\enable-sandbox-result.json'`。记录：共弹出几次 UAC（预期 1 次：srt 的 `windows-install` 自提权在已提权进程中不应再弹）；“标准用户 + 输入管理员凭据”的情形也跑一次（结果文件应写到调用用户的目录，因为路径由参数传入）。核对：`icacls "%ProgramFiles%\Fox\sandbox-runtime\0.1.0"` 为去继承、Administrators/SYSTEM F、Users RX、所有者 Administrators；`icacls "%ProgramData%\Fox\sandbox"` 为 Administrators/SYSTEM F、Users `(RA,X,AD,S)` 仅本目录、CREATOR OWNER `(OI)(CI)(IO)F`；结果 JSON 含 `srt.sandboxUserSid`、`markerVersion`、`createdAt`。再跑一次确认幂等（同哈希跳过复制）；改错 `-ExpectedSha256` 确认失败且不留目录；故意让 `windows-install` 失败（如端口段与已有安装不同）确认运行时目录被回滚。之后用 `apply_enable_result` 写入 `srt-sids.json` / `srt-install.json`（或手工按结果核对）。
4. 原始 pi T04 与 dsh（在第 3 项之后）：`set FOX_TEST_REAL_SRT=1`、`FOX_RUNTIME_ROOT=%ProgramFiles%\Fox\sandbox-runtime\0.1.0`、`FOX_SRT_PREFIX=%FOX_RUNTIME_ROOT%\srt`、`FOX_PI_ACP_PREFIX=%FOX_RUNTIME_ROOT%\backends\pi`、`FOX_SRT_NODE=%FOX_RUNTIME_ROOT%\node\node.exe`、`FOX_DSH_BIN=%FOX_RUNTIME_ROOT%\backends\dsh\node_modules\.bin\dsh.cmd`，运行 `cargo test -p fox-agent-backend --features acp --test sandbox_srt -- --include-ignored --nocapture`（`real_srt_pi_acp_t04_windows`、`real_srt_dsh_initialize_inside_sandbox`）。预期不再出现：WFP 自检 0x80070003、Node EPERM、dsh patch 的 `sandbox_path_not_owned`。私有 HOME 应在 `%ProgramData%\Fox\sandbox\<你的 SID>\homes\…`。
5. **中文空格路径 + 祖先 ACE**：T04 默认把项目建在 `%USERPROFILE%\Documents\中文 空格\proj-<pid>`（可用 `FOX_T04_PROJECT_PARENT` 改）。确认：`ancestor ACEs` 行列出 profile 根、`Documents`、`中文 空格`（以及 `%ProgramData%\Fox\sandbox\<SID>`、`homes`、`homes\<UUID>`、`tmp`）；沙箱内 `Get-ChildItem Documents` 与 profile 根均为 `LIST-DENIED`（只能 stat、不能列）；pi 能正常启动与读写项目；关闭后断言每个祖先上 Fox 的 ACE 已撤销，并用 `icacls` 人工复核这些目录上无 `srt-sandbox` 条目。另试一次 OneDrive 重定向的 Documents（若有）。
6. **两个并发会话交叉验证台账**：会话 A 授权 `Documents\中文 空格\a`，会话 B 授权 `Documents`（B 的 srt 会在 `Documents` 上收敛并抹掉 A 的祖先 ACE）。顺序：起 A → 起 B → 看 B 的 transition `finish()` 报告 `restored` 含 `Documents` → 关 B（srt 撤销 B 在 `Documents` 上的授权时再次抹掉）→ 确认 `finish()` 再次补回、A 仍可正常 stat → 关 A → 全部撤销。期间对照 `%LOCALAPPDATA%\Fox\state\win-ancestor-aces.json` 的引用计数。
7. **强杀 Host 后 reconcile**：T04 运行中强杀 cargo 测试进程（或 Fox Host），之后调用 `recover_ancestor_aces(&coordinator, &BTreeSet::new())`（或再次启动 Fox），确认台账清空、祖先 ACE 全部撤销；再启动一次 sandboxed 会话确认 srt 自己的崩溃恢复撤销了其 ACE。
8. **7 个孤儿对象所有权修复**：在第三轮留下孤儿的目录（或新跑一次让沙箱在授权目录与私有 HOME 写文件）上，先 `record_srt_sid` 记录当前 SID，再调用 `repair_sandbox_ownership(&[授权目录, 私有 HOME 根], &known_srt_sids(state)?, &RealWindowsAcl, &RealWindowsAcl)`；确认报告逐项列出、所有者变为本人（`icacls /verbose` 或 `Get-Acl`）；手工建一个由其他账户 / 不在台账的 SID 拥有的对象，确认不被改；放一个指向目录外的 junction，确认 `skipped_reparse_point` 且不进入。卸载 srt 后已成孤儿的对象：先把旧 SID 写进 `srt-sids.json` 再修复。
9. **跨账户强杀链**：T04 进行中用提权 PowerShell `Get-Process -IncludeUserName | ? UserName -like '*srt-sandbox*'` 记录 runner / node / bash；强杀 Fox 的 node（srt）进程后确认 `srt-sandbox` 名下进程全部结束（two-hop，7.4）；看 stderr 是否出现 `broker→runner Job assign not supported`。
10. **第四轮（b0bed84）的 7 项在 Windows 上复核**：停止确认（`ActiveProcesses == 0` → `Confirmed`）；未确认 → `StoppedUnconfirmed` 且拒绝重建；Stopping 状态下晚到审批自动答为取消（`PermissionAutoCancelled`）、新任务被拒；声明逐项等于写出的 settings；路径规范化（`junctioned_authorized_folder_is_judged_by_its_target`、UNC 被拒）；`verify_delivered_file`（授权可写目录内通过，会话临时目录 / 私有 HOME / 只读授权目录 / 经联接指出去的文件拒绝）；pi `cancel_process_tree = true` 的进程树清理。
11. **1.1.1.1 主机正对照**：在主机（沙箱外）先 `curl.exe -sS -m 15 http://1.1.1.1/` 确认可达（得到 HTTP 响应），再看 T04 中 `directip` 为 curl exit 7，证明拒绝来自 WFP 而不是目标不可达。
12. 暴露面警告（B）：`audit_broad_exposure()` 的结果经 `declaration.with_exposure(&report)` 后 `exposed_dirs` 与 `icacls D:\` 一致，`exposure_warning` 为固定文案；`directory_isolation` 仍为 false。
13. **停用 / 卸载边界**：`disable-sandbox.ps1 -StateDir … -InvokingUserProfile … -Version 0.1.0 -ResultPath …`：默认（未加 `-ConfirmNoOtherSrtClients`）应 `decision = keep` 并列出“无法排除其他使用者”；加上后在无活跃会话时 `decision = uninstall`、`windows-uninstall` 成功、`%ProgramData%\sandbox-runtime` 与本人 `%LOCALAPPDATA%\sandbox-runtime` 被删、`%ProgramData%\Fox\sandbox` 未动；T04 运行中执行应因活跃会话 `keep`；另一用户 profile 下放一个 `AppData\Local\sandbox-runtime\state.db` 应 `keep`；先用 npx 自行装 srt 再跑启用脚本，停用时应因“srt 原本已安装”`keep`。
14. 有余力：Win10 22H2 复跑 4–7；Defender 开启、火绒开启各跑一次 4。

## 10 路径规范化（第四轮）

Host 传入的路径在做重叠检查、生成策略之前先规范化，不再只按文字比较：

- 入口：`SrtLauncher::wrap` 在**沙箱实际启动的主机**上（`ready.os == HostOs::current()`）先规范化 `state_root`、`tmp_root`、`managed_runtime_root`，再对“生效策略”调用 `SandboxPolicy::canonicalized()`，然后才 `validate` / Windows ACE 预检 / 写 settings。为别的平台规划的策略（单元测试里的 Windows 规划）按原文使用。
- 授权目录（`canonical_existing_path`）：**必须存在**。`std::fs::canonicalize` 解析符号链接；Windows 用 `dunce::canonicalize`，解析符号链接与 junction（`GetFinalPathNameByHandle`）并在能表示为盘符路径时去掉 `\\?\` 前缀。不存在 → `sandbox_policy_invalid`（`remedy`：重新选择文件夹）。选择“报错”而不是“规范化父目录”，因为授权一个还不存在的目录没有意义，且父目录规范化会让之后在该位置建立的符号链接逃过检查。
- Fox 受管路径（私有 HOME 及其根、会话临时目录、state、运行时 / 安装目录、额外只读路径、拒读根；`canonical_managed_path`）：可以尚不存在。规范化最深的已存在祖先，再按原样拼上缺失部分；缺失部分含 `..` → 拒绝。
- Windows：规范化结果仍以 `\\` 开头（UNC 共享、`\\.\` 设备路径、无法简化的 verbatim 路径）→ `sandbox_policy_invalid`。
- 系统只读路径（`default_system_read_only`）是 Fox 自己实测过的清单，保持原文（例如 merged-usr 上 `/lib` → `/usr/lib` 的符号链接本身也要在沙箱内可见）。
- 效果：符号链接 / junction 指向私有 HOME 根（或 `/`）的“授权目录”在文字上不重叠，规范化后被拒（`symlinked_authorized_folder_is_judged_by_its_target`，Linux 实测；`junctioned_authorized_folder_is_judged_by_its_target`，Windows 编译通过、待真机运行）；无害的链接以其目标写入 settings。
- 遗留：规范化与 srt 实际 bind 之间仍有时间窗（检查后把目录换成链接）；授权目录内部的符号链接由 srt / bwrap 按挂载语义处理，不在本层。

## 11 交付文件（第四轮，库层；桌面接线是下一步）

- 规则：Agent 交付给用户的文件**必须位于授权的可写持久目录**。会话临时目录随会话删除，私有 HOME 是 Fox 私有区域，沙箱内被拒读区域的写入只落在 tmpfs、主机上根本没有。
- Host 侧校验：`sandbox::verify_delivered_file(policy, path) -> Result<DeliveredFile, SandboxError>`，`policy` 用 `PreparedLaunch.effective_policy`（含会话临时目录）。
  1. `canonical_existing_path(path)`：解析符号链接 / junction，指到外面的链接按目标判定；UNC / 设备路径拒绝；
  2. 策略同样规范化后，文件必须在某个 `writable = true` 的授权目录内（只读授权目录内单独报错）；
  3. 不在会话临时目录、私有 HOME、私有 HOME 根内；Linux 上不在 tmpfs 挂载上（`statfs` 的 `TMPFS_MAGIC`；重启即丢，也正是 srt 沙箱内临时区的形态）；
  4. 在沙箱外：存在、是普通文件、能打开并读首字节（空文件也算合格），不做逐字节比对。
- 结果：`DeliveredFile { path（规范路径）, authorized_root, len }`；拒绝统一为 `delivered_file_rejected`，带用户可读的 `remedy`（让 Agent 把文件保存到授权文件夹后重新交付）。
- 测试：`delivered_files_must_be_real_files_in_a_writable_authorized_folder`、`delivered_symlinks_are_judged_by_their_target_and_unreadable_files_are_refused`（Unix）、`delivered_files_on_tmpfs_are_refused`（Linux，`/dev/shm` 为 tmpfs 时执行，本机已执行）。
