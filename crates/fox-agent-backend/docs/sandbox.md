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
- **完全访问也用这份私有 HOME**（5f2 后续批起）：完全访问只取消隔离（不包 srt、不写 settings、不承诺目录边界），`HOME`、`XDG_*`、`TMPDIR/TMP/TEMP`、Windows 的 `USERPROFILE/APPDATA/LOCALAPPDATA`、`CODEX_HOME` / `DSH_HOME` 与沙箱模式取值完全相同（`sandbox::private_home_env`，单测核对与 `sandbox_env` 一致），只是不设 srt 相关的 `CLAUDE_CODE_TMPDIR`、`NODE_USE_ENV_PROXY`、`NODE_OPTIONS`，`PATH` 照环境白名单继承。私有 HOME 与会话临时目录不存在就创建（Windows 走与沙箱相同的 `prepare_windows_private_root`）。见 13.12。
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

### 6.5 第六轮（2026-10-20）

- broker / 后端环境拆分后（7.11：私有 HOME 等经 `/usr/bin/env K=V` 传给后端，srt 只拿 TMPDIR / CLAUDE_CODE_TMPDIR），Linux 真实 T04（`real_srt_pi_acp_t04_filesystem_and_network_boundary`，aarch64，bubblewrap 0.12.0、socat 1.8.0、srt 0.0.80、pi-acp 0.0.34）重跑通过：后端看到的 HOME / TMPDIR 仍是私有目录与会话临时目录，授权外读写、直连 IP 仍被拒，停止确认 `Confirmed`。
- 桥接（7.12）只在 Windows 使用；Linux 上用 Unix socket 跑了同一套握手与转发（真实 bridge.mjs + mock ACP 后端）。

### 6.6 第七轮（Windows 第六轮修复，2026-10-20）

- 脚本模型移到沙箱外（7.14）后，Linux 真实 T04 重跑通过（`real_srt_pi_acp_t04_filesystem_and_network_boundary ... ok`，95.7 s，同 6.5 的环境）。srt settings 的 `allowedDomains` 恰为两项：允许目标 `127.0.0.1:<端口>` 与模型 `127.0.0.1:<端口>`；新断言“第一次工具 turn 后模型服务至少收到 2 个请求”通过（模型请求确实经 srt 代理到达主机回环）；未列端口（403 / CONNECT 403、目标计数不变）、直连回环（exit 7）、直连 1.1.1.1（exit 7）断言不变且通过。
- 同时重跑 `real_srt_dsh_initialize_inside_sandbox`：通过（171.1 s；dsh 无内置模型服务，只做 initialize / session/new，不受本改动影响）。
- 每条真实测试单独运行（直接执行测试二进制、后台运行，`--include-ignored --nocapture`）。

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

**Windows**（alpha，未实测；本节按 srt 0.0.80 源码核实，2026-10-19 第三轮重写，第五轮补 7.5–7.8，第六轮（2026-10-20）补 7.11–7.13 并修订 7.3、7.5、7.7、7.8）

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
- （第六轮）**读取失败不算无暴露**：实机 3 盘、78 暴露项、7 个审计错误曾被当作成功审计。现在每个路径的结果是“暴露 / 无暴露 / 读取失败”三者之一（`checked[{path, result: exposed|not_exposed|read_failed}]`），读取失败（DACL 读不到、盘根列不出）另列 `audit_errors[{path, reason}]`；`ExposureReport::status()` = `complete` / `incomplete` / `not_supported`，有任何审计错误即 `incomplete`，**不是成功审计**。声明（`with_exposure`）带 `exposure_audit`（`complete` / `incomplete`）与 `audit_errors`（`路径: 原因`）；有审计错误时 `exposure_warning` 追加“部分目录无法检查。”（`WINDOWS_EXPOSURE_INCOMPLETE`）；没发现暴露但有审计错误时警告为“此模式不限制在授权目录内。部分目录无法检查。”。旧字段 `errors` 保留同样内容。

7.4 进程清理：Job Object 与 two-hop 风险（`acp.rs`）
- 实现：`AcpConnection::spawn` 在 Windows 上以 `CREATE_SUSPENDED` 启动子进程 → 新建 Job（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，不允许 breakaway，句柄不可继承）→ `AssignProcessToJobObject` → 用 Toolhelp 快照找到该进程的线程并 `ResumeThread`。进程在入 Job 之前无法运行，消除了“先 spawn 再 attach”的竞态（std/tokio 不返回主线程句柄，故用 Toolhelp 恢复；任何一步失败即杀掉挂起的子进程并报错，不在 Job 外运行）。
- 关闭（`close` / `close_with_report`，先进入 Stopping）：Windows 先丢弃命令通道（ACP 连接结束、后端 stdin 关闭，给 srt 机会自己 `reset()` 撤销 ACE），等 `TERMINATE_GRACE`（3 s），再 `TerminateJobObject`；只有没有 Job 或 `TerminateJobObject` 失败时才兜底 `taskkill /PID <pid> /T /F`，并检查退出码（128 = 进程已不在）。之后在 `STOP_CONFIRM_TIMEOUT` 内轮询 `QueryInformationJobObject(JobObjectBasicAccountingInformation).ActiveProcesses`，为 0 才算 `Confirmed`（无 Job 走 taskkill 时一律未确认）。结果放进 `TerminateReport { confirmation, confirmation_scope_note, force_killed, job_terminated, notes, .. }`，同时以 `fox-agent-backend:` 前缀写 stderr。沙箱模式下“确认”只覆盖我们的 Job（1.2）。连接被直接 drop 时，Job 句柄关闭同样触发 kill-on-close。
- **two-hop 风险（源码核实）**：sandboxed 模式下我们的 Job 里只有 srt 的 node 进程和 srt-win broker（libuv 把 node 的子进程放进它自己的嵌套 Job，`SILENT_BREAKAWAY_OK` 只脱离 libuv 的 Job，不脱离不允许 breakaway 的外层 Job——此点按 Windows 嵌套 Job 文档推断，需实测）。runner 是 seclogon 服务代为创建的（`CreateProcessWithLogonW`），**不在我们的 Job 里**；后端本体由 runner 以 `CREATE_BREAKAWAY_FROM_JOB` 放进 runner 自己的 Job，**也不在我们的 Job 里**。清理链依赖 srt-win 自身：我们 `TerminateJobObject` → broker 死 → broker 持有的 broker→runner Job（kill-on-close）句柄关闭 → runner 死 → runner 的 Job（kill-on-close、不允许 breakaway）句柄关闭 → 后端整棵树死。若 broker→runner 的 assign 因 seclogon Job 不支持嵌套而失败（srt-win 打印 `broker→runner Job assign not supported`），外部杀 broker 后 runner 不会被杀（它的 `SpawnedChild` 守卫只在 broker 进程内正常退出时生效），只能等后端因 stdin 断开自行退出；srt 注释给出的兜底是**提权** `taskkill /F /FI "USERNAME eq srt-sandbox"`。
- （第六轮）Windows 沙箱模式下 ACP 走桥接管道（7.12）：srt 进程的 stdin 为 null，关闭时丢弃命令通道即关闭管道，桥接随即关闭后端 stdin 并在 5 s 宽限后结束后端、以后端退出码退出；Job Object、`TerminateJobObject` 与 two-hop 链对 srt 进程的作用不变。
- ACE 回收：硬杀 node 会跳过 srt 的 `reset()`，ACE 留在授权目录上，直到下一次任何 `srt-win acl` 操作的崩溃恢复按“持有者 PID 已死”清理（`with_init_lock`），或手动 `srt-win acl recover`。持有者 PID 被复用时清理会推迟。故关闭时先走 stdin 关闭的温和路径。

7.5 运行时与私有目录布局（第五轮）
- **受管运行时**：Windows 只接受 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（`winruntime::default_windows_runtime_root`；`windows_runtime_root` 允许测试用 `FOX_RUNTIME_ROOT` 覆盖，覆盖值同样要求管理员拥有、Users 不可写）。由启用脚本（7.7）以管理员复制并设 ACL：去继承，Administrators / SYSTEM 完全控制，Users 读取和执行，所有者 Administrators。沙箱账户属于 Users，可直接读，Fox 不为它生成任何 ACE（用户对管理员拥有的目录也没有 `WRITE_DAC`）。包内布局：`node\node.exe`、`srt\node_modules\@anthropic-ai\sandbox-runtime\…`、`backends\<后端>\…`、`dsh-patches\*.yml`（本 crate `runtime-assets/dsh-patches`，与 `deepseek::patch_variants()` 逐字节一致，有单元测试守护）、（第六轮）`acp-bridge\bridge.mjs`（本 crate `runtime-assets/acp-bridge/bridge.mjs`，Fox 内嵌一份 `launch::BRIDGE_SCRIPT`，启动前逐字节比较，换行归一；缺失或不同 → `sandbox_bridge_unavailable`，规则同 dsh patch）；实测夹具可放 `t04-fixtures\`。后端版本随运行时包固定，升级需重跑启用脚本（一次 UAC）。
- （第六轮）**运行时包构建与路径预算**：`scripts/windows/build-runtime-package.py`（Python，Linux 上也能跑）：
  - `check <目录|zip> --version <v> [--budget 240]`：按 `C:\Program Files\Fox\sandbox-runtime\<v>\` + 包内相对路径（反斜杠）以及启用脚本的暂存路径 `C:\Program Files\Fox\.s<8 位>\c\` 计算每个文件的完整长度，≥ 预算即失败并列出文件；
  - `stage <输出目录> [--node-dir] [--srt-prefix] [--backend 名=前缀 …] [--prune-dts]`：组装目录并复制 `acp-bridge`、`dsh-patches`（不下载；前缀用 `npm install --prefix <p> --os=win32 --cpu=x64 <包>@<版本>` 准备）；
  - `pack <目录> --version <v> --out <zip>`：校验布局（`node\node.exe`、srt `cli.js`、`acp-bridge\bridge.mjs` 与 dsh patch 与本 crate 一致）、跑预算检查，写确定性 zip（固定时间戳、排序）、`<zip>.sha256` 与 `<zip>.manifest.json`（每个文件的大小与 SHA256、最长安装路径）。
  - 对当前包的判断（2026-10-20，用本仓库 Linux 安装的同版本 npm 树按 Windows 布局计算，Windows 平台二进制包名不同，未计入）：77 679 个文件，最终安装路径最长 234（pi 下 `@aws-sdk/core/dist-types/ts3.4/...browser.d.ts`），DeepSeek inspector 最长 230，均 < 240；`--prune-dts` 后最长 230。第五轮启用脚本的暂存路径 `…\sandbox-runtime\.staging-<32 位>\content\` 比最终目录长 44 个字符，inspector 文件在暂存处达 274 > 260，这正是 PS5.1 `Expand-Archive` 的 `DirectoryNotFoundException`；现暂存路径改为比最终目录短（见 7.7）。
  - （第七轮）**Windows 长路径**：工程师实测 Windows 上 `stage` 以相对路径复制深层依赖报 WinError 3、`pack` 用 `\\?\` 源路径时必需文件检查因混用正斜线误报 missing、`pack` 用较长的普通绝对路径读深层文件失败，只能用短路径 junction 绕过。现在脚本内所有读写都经 `fs_path()`：Windows 上为扩展长度路径（`\\?\` + 绝对路径、全部反斜杠、`.` / `..` 已解析；UNC `\\server\share\…` → `\\?\UNC\server\share\…`；已是扩展形式的只修正分隔符），其他平台为普通绝对路径；复制改为脚本自己遍历（`walk_files` + `copyfile`，链接按目标复制，与原 `copytree(symlinks=False)` 一致）。内部一律以“包内键”（`rel_key`：正斜线、无 `./`、拒绝 `..`）标识文件：必需文件检查、dsh patch 比较、预算报告、zip 条目名都用它，不再拼接 OS 路径比较。不再需要 junction。单元测试 `scripts/windows/tests/test_build_runtime_package.py`（`python3 -m unittest discover -s scripts/windows/tests`，15 条）：扩展路径转换（盘符、正斜线与 `..`、相对路径 + cwd、无盘符根路径、UNC 两种写法、已扩展路径修正分隔符、> 400 字符原样保留）作为纯函数在任何系统上覆盖 Windows 分支；在当前系统上用深度 > 300 字符的依赖树、相对路径参数真实跑 `stage → pack → check`（zip 条目与目录键一致、清单 SHA256、`--prune-dts`、不带 `bridge.test.mjs`、超预算拒绝）；必需文件只报真缺失；dsh patch 不一致 / 缺失；链接在 stage 中跟随、在 pack 中拒绝。Windows 上真实长路径的读写需工程师按第 9 节②验证。
  - 超预算时的缩短手段：先 `--prune-dts`（运行时不需要 `.d.ts` / `.map`）；再对超长的包用 esbuild 打成单文件（DeepSeek inspector 的 `lib/devtools/...` 描述文件是静态资源，需改为按需读取或随包重命名，需上游配合，本轮未实现）；或 `npm install --install-strategy=shallow` 扁平安装去掉嵌套 `node_modules` 层级。
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
- （第六轮）**PS5.1 首装与长路径**：解压前先按 zip 条目名做路径检查（`Get-PackageLongPath`，不需要长路径 API；目标目录与暂存目录两种前缀，任一 ≥ `-MaxPathLength`（默认 260）→ `package_path_too_long`，日志列出最长的 20 个，不解压）；解压优先 `%SystemRoot%\System32\tar.exe -xf <zip> -C <目录>`（bsdtar，支持 > 260 的路径），失败再清空后退回 `Expand-Archive`，结果 JSON 记 `extractMethod`；暂存目录改为 `%ProgramFiles%\Fox\.s<8 位>\{p.zip, c}`（比最终目录短），备份目录 `.o<8 位>`。失败处理：先把**原始异常**（步骤、异常类型、消息、内层异常、脚本位置、栈）写日志并放进结果 JSON 的 `primaryError`，再回滚 / 清理；清理用 `Remove-FoxTree`（`Remove-Item` 失败时改用 `cmd /c rd /s /q "\\?\<路径>"`），清理错误只进 `cleanupErrors`，**不覆盖原始错误**（第五轮实机日志最后一行是清理阶段“找不到 dsh-patches”，掩盖了解压时的 `DirectoryNotFoundException`）。包内必须有 `acp-bridge\bridge.mjs`。
- （第七轮）**原始错误保留抽成函数 + 故障注入**：`Resolve-FoxFailure -Result -ErrorRecord -Step -Cleanup <有序字典>` 先把原始错误写入结果（`error`、`primaryError{step,type,message,inner,at}`）与日志，再逐项运行回滚 / 清理脚本块；任一项抛出只记入 `cleanupErrors`（`<步骤名>: <消息>`），绝不覆盖 `primaryError`；暂存目录的清理仍在 `finally` 中由 `Remove-FoxTree` 完成（从不抛出）。结果 JSON 新增 `stagingDir`。测试用注入点：环境变量 `FOX_SCRIPT_TEST_FAULTS`（逗号分隔，忽略大小写与空格）——`extract`：`Expand-FoxPackage` 开头抛出 `FOX_SCRIPT_TEST_FAULTS: injected extract failure`；`cleanup`：`Remove-FoxTree` 不删除并记一条 `<对象> <路径>: injected cleanup failure (FOX_SCRIPT_TEST_FAULTS)`。**未设置或为空时注入点是空操作**，生产路径行为不变；设置时日志写 `TEST FAULTS ACTIVE`。测试脚本 `scripts/windows/tests/test-enable-faults.ps1`：默认模式从语法树载入 enable 脚本的全部顶层函数（不执行主体、不需要管理员），在 `extract,cleanup` 下复现“解压与清理同时失败”，检查 `primaryError` 为解压错误、`cleanupErrors` 含暂存与回滚两条注入的清理错误和一条抛出异常的清理步骤、原始错误先于清理写入日志，以及未设变量时注入点为空操作（21 条检查，Linux 上 pwsh 7.4.6 全过）；`-FullRun -PackagePath … -ExpectedSha256 …`（Windows、提权）以子进程 powershell.exe 运行真实脚本并检查结果 JSON，之后删除注入留下的暂存目录。
- （第六轮）`enable-sandbox.ps1` / `disable-sandbox.ps1` 新增可选 `-RepairOwnershipList <json>`，调用同目录的 `repair-ownership.ps1`（7.8）；enable 在安装成功后执行，disable 在卸载判断之前执行；结果在 `ownershipRepair` 字段，不改变脚本自身的 `ok`。
- `disable-sandbox.ps1 -StateDir <%LOCALAPPDATA%\Fox\state> -InvokingUserProfile <%USERPROFILE%> -Version <v> -ResultPath <file> [-RemoveRuntime] [-ConfirmNoOtherSrtClients]`：按上述判断后才 `windows-uninstall`；`-RemoveRuntime` 在其后删除运行时目录；**不删任何用户的私有 HOME**（`%ProgramData%\Fox\sandbox` 一律不动）。卸载前 Fox 应先跑 7.8 的所有权修复与 `recover_ancestor_aces`。
- srt 安装时在 13 个世界可写系统目录加沙箱 SID 的写拒绝 ACE（实机记录为 11 处），卸载时移除。

7.8 所有权修复（第五轮，`winowner.rs`）
- 沙箱内新建的文件 / 目录所有者是 `srt-sandbox` 的 SID，srt 卸载后成为孤儿（第三轮实机 7 个对象）。`repair_sandbox_ownership(roots, known_srt_sids, probe, writer) -> OwnershipReport`：只遍历传入的根（授权目录与私有 HOME 根），只处理所有者属于“Fox 记录的历代 srt SID”（`srt-sids.json`，`known_srt_sids(state_root)`）的对象，改为当前用户（`SetNamedSecurityInfoW(OWNER_SECURITY_INFORMATION)`，依赖 `WRITE_OWNER`，用户经其文件夹继承来的完全控制即有）；其他 SID（包括别的孤儿 SID）一律不动；不跟随重解析点（Windows 上任何 `FILE_ATTRIBUTE_REPARSE_POINT`，含符号链接、junction 和其他标签；既不改也不进入）；返回逐项报告（`repaired` / `failed` / `skipped_reparse_point` / `owner_unreadable`，另计 `scanned`、`untouched`、`errors`）。
- 触发点只写 API 与文档：卸载 srt 之前、重装 srt 之后、用户手动；**不在每次会话后自动跑**。
- （第六轮）**`needs_elevation`**：实机 `D:\fox-t04-exposed-round3-20261019\mechanism-write.txt` 所在目录给 Authenticated Users 的是 Modify，当前用户没有 `WRITE_OWNER`，改所有者返回 Win32 5。现在普通权限下 Access denied（`is_access_denied`：`Win32 error 5` / access denied）记为 `OwnershipAction::NeedsElevation { previous_owner, reason }`，**不算失败**（`OwnershipReport::needs_elevation()` 单独计数）；其他错误仍是 `failed`。`ownership_repair_list(&report, user_sid, &known_srt_sids)` 把这些项生成清单 `OwnershipRepairList { version: 1, source: "fox-nonelevated-ownership-scan", userSid, srtSids, items[{path, expectedOwner, newOwner}] }`（只收所有者属于已记录 srt SID 的项）。Fox 写成 JSON 交给提权脚本 `-RepairOwnershipList`。
- `scripts/windows/repair-ownership.ps1 -ListPath <json> [-ResultPath] [-LogPath]`（管理员）：校验清单版本、来源、SID 格式，`newOwner` 必须等于 `userSid`；每项要求本机绝对路径（拒 UNC / `\\?\`）、路径本身与每一级上级都不是重解析点（不跟随）、读回当前所有者等于 `expectedOwner` 且属于 `srtSids`；启用 `SeRestorePrivilege` 与 `SeTakeOwnershipPrivilege` 后用 `SetNamedSecurityInfoW(OWNER_SECURITY_INFORMATION)` 只改所有者（不动 DACL），再读回确认；逐项写日志，结果 `{ok, repaired, skipped, failed, items[{path, status: repaired|skipped|failed, previousOwner, detail}], privileges}`。清单之外的对象一概不碰。

7.9 网络
- WFP 在 `ALE_AUTH_CONNECT` 拦截 `srt-sandbox` SID 的一切出站连接，只放行回环上的代理端口段（默认 60080–60089，须与 `windows-install --proxy-port-range` 一致）；直连公网 IP、直连回环其他端口都被拒（curl exit 7）。
- **系统 DNS 解析不受同等限制**（srt README Known limitations）：`getaddrinfo()` 由以 NETWORK SERVICE 运行的 Dnscache 服务完成，名字解析会成功（随后的 connect 仍被拦）；自己发 UDP/53 的工具（nslookup、dig）被拦。DNS 查询本身可作为外泄通道，**不能宣称 Windows 上网络完全封闭**。schannel 的 CRL/OCSP 吊销检查被 WFP 拦截（`CRYPT_E_REVOCATION_OFFLINE`）。
- 代理令牌在 runner 命令行中可见（`HTTP_PROXY=http://srt:<token>@127.0.0.1:...`），同机其他本地主体可读。

7.10 启动命令与环境变量（源码核实，影响很大）
- srt CLI 在 Windows 上把 `--` 后的 argv 用 POSIX 单引号规则拼成字符串，再交给 `cmd.exe /d /s /c` 执行：含反斜杠的路径会被包成 `'C:\...'`，cmd 不认单引号，必然失败。srt-win 给子进程的是 `srt-sandbox` 账户的全新 profile 环境，只叠加 broker 的 `PATH`、`PATHEXT`、代理变量与 git 配置变量：Fox 设置的 `HOME/USERPROFILE/CODEX_HOME/DSH_HOME/TEMP/NODE_USE_ENV_PROXY` 和 `PI_ACP_PI_COMMAND` 等原本都传不进去。
- 修正：Windows 上 `wrap` 改用 `srt -c "<cmd 行>"`，`windows_command_line()` 生成 `set "K=V" && ... && "<程序>" <参数>`（srt-win 对 `/c` 之后原样透传）。含 `"`、`%`、换行的值或参数无法安全经过 cmd → `sandbox_policy_invalid`。名字含 `KEY/TOKEN/SECRET/PASSWORD` 的变量**绝不**写进命令行（进程列表可见）；Host 环境中的凭据变量在 Windows 沙箱内本来也拿不到，所以 **Windows 沙箱模式下后端只能用私有 HOME 内的原生登录**。以 shell API 取已知文件夹的程序仍会拿到 `srt-sandbox` 自己的 profile。
- （第五轮）Windows 沙箱模式下每个后端（pi-acp / pi、codex-acp、dsh 都经 `.cmd` 启动 Node）的会话变量里加 `NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main`（`launch::WINDOWS_SANDBOX_NODE_FLAGS`，由 `SrtLauncher::sandbox_env` 写入 `set` 前缀，Node 允许这两个旗标出现在 `NODE_OPTIONS` 中并传给其子 Node 进程）。**只是辅助**：模块解析不再 realpath，可少一部分祖先 `lstat`；真正让 profile 链可 `lstat` 的是 7.6 的祖先 ACE，后端自己调用的 `fs.realpath` 不受影响。

7.11 srt broker 与后端的环境拆分、锁路径一致性（第六轮）
- 问题（工程师实测 3ba5d23）：`AcpConnection::spawn` 把 `spec.session_env` 全部给了 srt 进程，而 Windows 上 `session_env` 含私有 HOME 版的 `LOCALAPPDATA` / `USERPROFILE` / `TEMP`。srt CLI 与 srt-win broker 以真实用户运行，于是把 `state.db` 与 `.acl-init.lock` 建在了 `<私有 HOME>\AppData\Local\sandbox-runtime\`，而 Fox 的 `winancestor::srt_acl_lock_path()` 读 Host 的 `LOCALAPPDATA`：两把“同一个锁”不是同一个文件，Fox 改 DACL 时不再与 srt 的收敛互斥，srt 的崩溃恢复也看不到彼此。
- 修正（`SrtLauncher::wrap`）：后端自己的变量（私有 HOME、XDG、TMP/TEMP、CODEX_HOME/DSH_HOME、PATH、NODE_OPTIONS、`spec.session_env`）放进 `SandboxedLaunch::backend_env` / `PreparedLaunch::backend_env`，**只经命令传入**：Windows 沿用 `-c` 的 `set "K=V"` 前缀；Linux / macOS 改为 `-- /usr/bin/env K=V … <程序> <参数>`（`UNIX_ENV_PROGRAM`，`/usr` 在系统只读集合内）。srt 进程（wrapped spec 的 `session_env`）只拿 `broker_session_env(os, …)`：Windows 为空——`LOCALAPPDATA`、`USERPROFILE`、`APPDATA`、`TEMP`、`TMP`、`HOMEDRIVE`、`HOMEPATH`、`SystemRoot` 等都是 Host 白名单里的真实值，且 Windows 上 wrapped spec 的 `credential_env` 为空（Host 凭据本来进不了沙箱）；Linux / macOS 只给 `TMPDIR` 与 `CLAUDE_CODE_TMPDIR`（srt 自己在 TMPDIR 放代理桥 socket，并把 CLAUDE_CODE_TMPDIR 作为子进程的 TMPDIR，实测需要），不再给私有 HOME（srt 用真实 HOME；在 denyRead `/` 下它的 HOME 便利写目录 `~/.npm/_logs`、`~/.claude/debug` 被 srt 自己剔除）。Linux 真实 T04（pi-acp + srt + bwrap）在此改动后重跑通过（第 6.5 节）。
- `AcpConnection` 实际给进程的环境 = `launch::process_environment(spec, 父环境)`（白名单 + `session_env`）；`process_environment_for(windows, …)` 让 Windows 规则在任意主机可测。`winancestor::srt_acl_lock_path_in(env)` / `srt_state_dir_in(env)` 按给定环境求锁路径（大小写不敏感、后者覆盖前者），`srt_acl_lock_path()` = 本进程环境。单元测试 `windows_broker_gets_the_real_localappdata_so_the_acl_lock_paths_agree` 构造父环境，断言 srt 进程环境求出的锁路径 == Fox 求出的锁路径，且 8 个真实目录变量原样、无 HOME、无凭据；并断言旧做法（私有 `LOCALAPPDATA`）求出的锁路径不同。
- 启动报告：`find_stray_srt_state(private_home)` 在 Windows 每次 `wrap` 时检查 `<私有 HOME>\AppData\Local\sandbox-runtime`，结果放 `SandboxedLaunch::stray_srt_state` / `PreparedLaunch::stray_srt_state`，**只报告不删除**（state.db 可能仍被存活的 broker 使用，且无法可靠判断）。显式清理 `remove_stray_srt_state(private_home, path)`：只接受恰为该私有 HOME 下 `AppData\Local\sandbox-runtime` 的目录、不跟随链接；调用方须先确认没有存活 broker（如 Fox 启动时、无 srt 会话）。

7.12 ACP 桥接：srt 0.0.80 Windows 下 stdin 不可用（第六轮，方案 A）
- **现象**（工程师 2026-10-20 实测，`diag-srt-stdio.ps1` 的前身）：经 srt 启动的进程在父进程 stdin 仍打开时立即读到 EOF（READY 之后约 1 ms），收不到任何输入；stdout / stderr 实时转发正常；同一 node 直接运行（对照）正常。原始 pi T04 与 dsh 都在 `initialize` 阶段 transport closed；srt debug 显示 WFP 自检通过、3 个目录授权成功、代理启动，然后以 0 静默退出，stdout 没有 `initialize` 响应——ACP 后端读到 stdin EOF 正常退出。
- **源码结论（srt 626a5967，确定：不转发）**：srt CLI（`src/cli.ts` 544–548）以 `stdio: 'inherit'` 启动 srt-win，这一跳 stdin 是继承的；断点在 srt-win broker → runner：`vendor/srt-win-src/src/logon.rs` `spawn_runner` 为 runner 新建三条匿名管道（197–199），把 runner 端经 `STARTF_USESTDHANDLES` 交给 `CreateProcessWithLogonW`（245–248），随后向 stdin 管道写入长度前缀的 `RunnerCmd`（380–395）并 `drop(stdin.broker)` 关闭写端（396，注释“close stdin so the runner's read_exact doesn't block”）；broker 只为 stdout / stderr 起泵线程（398–420），**没有任何代码读 broker 自己的 stdin**。runner（`runner.rs` 76–96）`read_exact` 恰好读完规格；`launch.rs` `run_lockdown` 用 `collect_inheritable_std_handles`（688–712）把 runner 的 std 句柄（即那条已关闭写端的管道）作为子进程的 `hStdInput`（242–245）。所以后端的 stdin 是一条写端已关闭、内容已被 runner 读完的管道 → 立即 EOF。`sandbox-manager.ts` 2515–2516 的注释“exec's stdin belongs to the child”与实现不符。
- **设计**：Host 先建一次性端点，再启动 srt；srt 的 `-c` 行为 `set … && set "FOX_ACP_PIPE=\\.\pipe\fox-acp-<128 位 hex>" && set "FOX_ACP_TOKEN=<128 位 hex>" && "<node.exe>" "<运行时>\acp-bridge\bridge.mjs" -- "<后端程序>" <参数>`。桥接连上管道后发一行 `FOX-ACP-BRIDGE/1 <token>\n`，Host 常量时间比较（`launch::constant_time_eq`）后回 `OK\n`（错 token 回 `DENIED\n` 并断开 → `bridge_handshake_rejected`），桥接收到 OK 后才启动后端（`.cmd` 经 `cmd.exe /d /s /c`），管道 ↔ 后端 stdin/stdout 双向转发（OK 之后紧跟的字节先写入后端），后端 stderr 继承到 srt 的 stderr；管道断开 → 关闭后端 stdin，5 s 后仍未退出则结束后端；后端退出 → 关闭管道、以后端退出码退出。桥接自身退出码：64 用法、65 缺 `FOX_ACP_PIPE`/`FOX_ACP_TOKEN`、66 连不上、67 握手被拒或超时、68 后端起不来。启动后端前从环境里删除两个变量；token 不写文件、不写日志（日志只有 `fox-acp-bridge:` 前缀的状态行）。
- **Host 侧**（`bridge.rs`，cfg(windows) 管道；Unix socket 仅供测试）：`CreateNamedPipeW` 经 tokio `ServerOptions`：`FILE_FLAG_FIRST_PIPE_INSTANCE`（名字已存在即失败，防抢注）、`max_instances(1)`、`PIPE_REJECT_REMOTE_CLIENTS`、字节模式；受保护 DACL `D:P(A;;FA;;;<当前用户>)(A;;FRFW;;;<srt 沙箱 SID>)`（无 SYSTEM / Administrators / Everyone），沙箱 SID 取 `HKLM\SOFTWARE\sandbox-runtime` 的 `SandboxUserSid`（`read_srt_sandbox_sid`，也可 `SrtLauncher::with_srt_sandbox_sid`）；SID 缺失 → `sandbox_bridge_unavailable`。只接受一次连接：`BridgeListener::accept` 消耗监听器，之后不再建实例（第二个客户端 `ERROR_PIPE_BUSY` / 找不到）。受限令牌（`token.rs`：RestrictingSids 为空、Medium IL）下用户 SID 照常参与访问检查，故 DACL 授予沙箱 SID 即可。
- **AcpConnection**（`spawn_detailed(spec, cwd, Some(endpoint))` / `spawn_prepared` 取 `PreparedLaunch::acp_bridge`）：先 bind 再 spawn；srt 进程 stdin = null，stdout / stderr 只作诊断尾部；在 `connect_timeout`（`BRIDGE_CONNECT_TIMEOUT` 30 s）内等“连接 + 握手”，同时赛跑进程退出：srt 先退出 → `backend_exited_before_initialize`（`stage = bridge_not_connected`，带退出码与 stderr 尾部）；超时 → `bridge_connect_timeout`；握手问题 → `bridge_handshake_failed` / `bridge_handshake_rejected`；建管道失败 → `bridge_listen_failed`。之后 ACP 与 stdio 模式完全相同。Linux / macOS 不变（stdio）。
- **安全边界（如实）**：DACL 只允许当前用户与 srt 沙箱 SID，挡住其他用户；但它允许**当前用户的任何进程**（它们本来就能以该用户身份做任何事）和**所有以 `srt-sandbox` 运行的进程**（srt 所有会话共用一个沙箱账户，包括其他 Fox 会话的沙箱后端）。token 防的是这些进程在桥接之前抢先连接并冒充。token 出现在 srt 的 node 进程与沙箱内 `cmd.exe` 的命令行里：本人、管理员可读；沙箱内 `cmd.exe` 以 `srt-sandbox` 运行、进程 DACL 取该账户的默认 DACL，**同为 `srt-sandbox` 的其他会话进程也可能读到**——Windows 上无法按会话区分。因此 token 只在“首次连接前”的短窗口内有意义；一次连接后端点即关闭（不再建实例），token 作废；握手失败的会话直接失败、不重试。`windows_command_line` 仍拒绝把其他含 KEY/TOKEN/SECRET/PASSWORD 的变量写进命令行，唯一例外是 `FOX_ACP_TOKEN`。
- （第七轮）**Windows 命名管道没有半关闭**：Host 关闭（或进程消失）后，桥接一侧看到的可能只是 `close`（或先 `error` 再 `close`），没有 `end`，而且此后桥接写回的任何字节都到不了 Host。工程师在 Windows 上跑旧测试 7/8：ENV:none、ECHO:hello、退出码都对，只有“Host 关管道后还要收到后端的 END”这一条失败——那是测试假设了半关闭，不是桥接错误。现在 `bridge.mjs` 把管道的 `end` / `close` / `error` 视为同一个“对端已走”事件，**哪个先到都处理、只处理一次**（`peerHandled`）：关闭后端 stdin（唯一的关闭点；`socket.pipe(child.stdin, { end: false })`），5 s（`CLOSE_GRACE_MS`）后仍未退出则结束后端；非 `end` 的情形同时解除 stdout → 管道的转发并继续排空后端 stdout，避免后端在退出途中因写满管道而阻塞。不假定先收到 `end`。**Host 约定**：关闭管道后不再等待后端经管道回传任何内容（ACP 也不需要）；需要“最后一句话”时必须在关闭之前以应用层消息完成。Linux（Unix socket 支持半关闭）与 Windows 的预期行为相同。
- 测试：桥接脚本 Node 单测 `runtime-assets/acp-bridge/bridge.test.mjs`（`node --test`，Unix socket / Windows 管道，11 条：握手、错 token 拒绝且不启动后端；**(a)** 双向转发 + token 不进后端环境 + 退出码，用应用层 `QUIT` / `END`（END 在后端退出前写出，不依赖半关闭）；**(b)** Host 关闭管道后后端确实收到 EOF 并退出——后端把 `EOF` 写进标记文件，**不经管道回传**，且断言后端 stdin 只被关闭一次；**(c)** 管道 `close` 而无 `end`（销毁桥接自己的一端模拟）也能结束后端；**(d)** 管道 `error` 后 `close` 只处理一次；OK 后紧跟的字节；后端忽略 EOF 时宽限后结束；后端先退出返回其退出码；连不上 66；缺变量 65 / 缺 `--` 64）。`bridge.mjs` 只有一份，Rust 侧以 `include_str!` 内嵌（`launch::BRIDGE_SCRIPT`），运行时包中的副本由 `bridge_script_matches` 逐字节（换行归一）比较，不存在需要同步的第二份源码；Rust：`bridge.rs` 握手单测（对 / 错 token、垃圾行、过长、提前关闭）、Unix 监听只收一次；`tests/acp_mock_agent.rs` 经真实 bridge.mjs + mock ACP 端到端、错 token、从不连接超时、未连上即退出；`#[cfg(windows)]` 真实命名管道测试（FIRST_PIPE_INSTANCE、单实例、无 SID 拒绝）。DACL 对其他账户的效果需真机（第 9 节 17）。

7.13 初始化诊断（第六轮）
- `AcpConnection::spawn_detailed` 返回结构化 `InitializeFailure { code, message, stage, exit_code, stderr_tail, srt_stderr_tail, backend_stderr_tail, stdout_tail }`；`spawn` / `spawn_prepared` 返回其文本（以 code 开头，如 `backend_exited_before_initialize: …`）。code：`spawn_failed`、`backend_exited_before_initialize`、`initialize_timeout`、`initialize_failed`、`bridge_listen_failed`、`bridge_connect_timeout`、`bridge_handshake_failed`、`bridge_handshake_rejected`。
- 只有收到 `initialize` 响应才算初始化成功；进程先退出（**退出码 0 也一样**）→ `backend_exited_before_initialize`；传输报错后 1.5 s 内进程退出也归为此码；超时（180 s）附 stderr 尾部。失败时先结束进程树，再等 stderr 最多 1 s 排空。
- 尾部：每行脱敏（`sk-…`、`KEY=`/`TOKEN=`/`SECRET=`/`PASSWORD=`、≥32 位不透明串（含桥接 token）、URL 中的用户信息 `http://user:pass@host` → `http://[redacted]@host`（srt 代理凭据）、`Proxy-Authorization`），**保留含用户名的路径**；每段最多 4 KiB（`FAILURE_TAIL_BYTES`）。srt / srt-win / 桥接的行（前缀 `srt-win:`、`srt:`、`[SandboxDebug]`、`fox-acp-bridge:` 等，`is_sandbox_stderr_line`）进 `srt_stderr_tail`，其余进 `backend_stderr_tail`——两者共用一条 stderr 管道，只能按前缀启发式区分。

7.14 T04 的脚本模型移到沙箱外（第七轮）
- **403 的原因**：工程师实测 e41785e 的 Windows T04，第一个 whoami 探测之前 pi 的会话记录里就有 provider `foxcmd` / model `scripted` 的 `errorMessage: "403 Connection blocked by network allowlist"`。旧夹具 `fixtures/sandbox/scripted-cmd-provider.ts` 在 pi 进程里（即沙箱内）起一个随机端口的本地 HTTP 服务作为模型地址，而测试策略只允许 `127.0.0.1:<allowed.port>`。srt 在 Linux / macOS 设 `NO_PROXY=localhost,127.0.0.1,…`，pi 直连沙箱自己的回环，所以 Linux 一直能过；srt 在 Windows 上**删除 NO_PROXY**（`windows-sandbox-utils.ts`：沙箱账户直连回环会被 WFP 拦），pi（undici `EnvHttpProxyAgent`，另有 `NODE_USE_ENV_PROXY=1`）把模型请求交给 srt 代理，代理按允许列表拒绝 → 403。断言 “probe must run as the srt-sandbox account” 是连带失败（没有工具调用就没有 WHO= 输出）。
- **夹具安排**：模型服务改由测试进程在沙箱外提供（`tests/sandbox_srt.rs` 的 `ScriptedModel`：Rust 标准库 TCP + 线程，OpenAI Chat Completions 兼容的流式回复，脚本规则与旧夹具相同：`RUN:` / `READ:` → 一个工具调用，工具结果 → `tool-result: …`，否则 `scripted hello`；支持 Content-Length 与 chunked 请求体、`/v1/models`）。先绑定端口并启动，再把 `127.0.0.1:<该端口>` 加进 srt 允许列表（与允许目标并列，共两项），地址经后端变量 `FOX_SCRIPTED_MODEL_URL=http://127.0.0.1:<端口>/v1` 交给 pi；`scripted-cmd-provider.ts` 只做客户端（读取该变量并 `registerProvider`，格式不对即抛错）。新增总是执行的单测 `scripted_model_serves_tool_calls_over_http`（不需要 srt）。T04 在第一个工具 turn 后断言模型收到 ≥ 2 个请求。
- **网络拒绝断言不变、允许列表不放宽**：另一个未列端口（HTTP 403 与 CONNECT 403，目标计数不变）、绕过代理直连回环、直连 1.1.1.1 仍须被拒。
- **平台差异**：Linux 上 srt 的代理（主机侧）能连主机回环端口（`netallowed` 一直如此实测），但 srt 给子进程的 `NO_PROXY` 含 127.0.0.1，pi 会绕过代理去连 bwrap 网络命名空间里的回环（那里没有服务）。因此 Linux T04 对后端进程额外设 `NO_PROXY=` 与 `no_proxy=`（经 `/usr/bin/env` 传入，只影响该测试的 pi 及其子进程；探针命令本来就显式 `--noproxy`）；Windows 不需要（srt-win 已删除 NO_PROXY）。产品代码不变：真实模型 API 是域名，不在 NO_PROXY 中。Linux 实测见 6.6。
- **dsh / codex 的真实测试**：已检查，`real_srt_dsh_initialize_inside_sandbox` 与 `tests/codex_backend.rs` / `tests/deepseek_backend.rs` 的真实测试都没有内置模型服务（dsh 沙箱测试只做 initialize / session/new；非沙箱真实测试用真实 API），无同类问题。`fixtures/pi/scripted-provider.ts`（进程内服务）只用于非沙箱的 `tests/pi_backend.rs`，不受影响。
- Windows 上的放置不变：`t04-fixtures\sandbox\scripted-cmd-provider.ts` 仍需由管理员放进运行时目录（现在只是客户端代码）。

7.15 跨账户进程快照（第七轮，`scripts/windows/snapshot-sandbox-processes.ps1`）
- 用途：第 9 节⑤每一步都要一个**明确的进程数**（第五轮旧采集方式在零条目时不写计数，无法区分“0 个”与“没采到”）。管理员 PowerShell 5.1 运行：`snapshot-sandbox-processes.ps1 -Label after-host-kill [-IncludeCommandLine] [-ExtraSid <历代 SID>] [-ResultPath …]`。
- 方法：从 `HKLM\SOFTWARE\sandbox-runtime` 读 `SandboxUser` / `SandboxUserSid`；`Get-CimInstance Win32_Process` 枚举全部进程，逐个 `Invoke-CimMethod GetOwnerSid` 与沙箱 SID 比较。输出 JSON：`count`（0 也写；失败时为 null 且 `ok=false`）、`processes[{pid, name, parentPid, startTime(UTC), ownerSid, commandLine?}]`、`ownerUnknownCount` / `ownerUnknown[{pid, name}]`（取不到所有者的进程单列，不猜测归属）、`totalProcesses`、`capturedAt`、`method`、`label`、`elevated`。命令行默认不采集，`-IncludeCommandLine` 时脱敏（`*KEY/TOKEN/SECRET/PASSWORD=`、URL 用户信息、≥ 32 位十六进制串，含桥接 token 与代理令牌）。未提权默认失败（`-AllowNonElevated` 仅供诊断）。

7.16 第二个普通用户打开桥接管道（第七轮，`diag-srt-stdio.ps1 -Variants pipe-other-user`）
- `-OtherUserCredential (Get-Credential)`（另一个**非管理员**本地账户）+ 可选 `-PipeServerTest <tests/bridge_pipe_windows.rs 的测试二进制>`。给出测试二进制时，管道由 Fox 自己的 Rust 实现创建：脚本以 `FOX_TEST_PIPE_SERVE=1` 运行 `#[ignore]` 测试 `bridge_pipe_serve_for_other_user_probe`（`BridgeListener::bind`：受保护 DACL 当前用户完全控制 + srt 沙箱 SID 读写、FIRST_PIPE_INSTANCE、单实例、拒绝远程），读到 `FOX_PIPE_NAME=` 后以 `Start-Process -Credential` 用另一账户启动 `powershell.exe -EncodedCommand`（只含管道名）尝试 `NamedPipeClientStream.Connect`，以 Win32 错误码作为退出码（预期 **5**，拒绝访问）；随后向测试的 stdin 写一行，测试断言此前无人连接，并以所有者身份自连一次作对照（证明尝试期间管道存在且可打开）。未给测试二进制时脚本用相同 SDDL 自建管道（结果 `pipeCreatedBy` 注明）。结果字段：`win32Error`、`serverResult`（`FOX_PIPE_RESULT=no-connection`）、`ownerControl`、`deniedAsExpected`、`otherUserSid`。
- 凭据只存在于 PSCredential 对象中，经 `Start-Process -Credential`（`CreateProcessWithLogonW`）使用：不进命令行、不写日志或结果文件；连账户名也加入脱敏表，结果只记录其 SID。需要 Secondary Logon 服务。

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
- [x] ~~（第五轮）运行时包的构建脚本~~：第六轮 `build-runtime-package.py`（check / stage / pack，路径预算、SHA256、清单）；Fox 安装包仍需内置哈希，Windows 平台的 npm 安装（`--os=win32 --cpu=x64`）与 Node Windows 发行包需在构建机准备。
- [ ] （第六轮）srt 上游修复 stdin 转发后（`diag-srt-stdio.ps1` 的 `srt` 变体与 `direct` 一致），评估是否保留桥接（它同时把 ACP 流从 srt 的 stdout 中隔离出来）。
- [ ] （第六轮）Host 接线：`spawn_prepared` 已自动用桥接；UI 展示 `InitializeFailure` 的 code 与尾部、`stray_srt_state` 启动报告、`exposure_audit` / `audit_errors`；卸载流程把 `needs_elevation` 清单交给 `disable-sandbox.ps1 -RepairOwnershipList`。
- [ ] （第七轮）Windows 实机：9.1 ①–⑥（长路径打包无 junction、双故障、第二用户打开管道、原始 pi T04 完整工具 turn、带明确进程数的强杀链、桌面 Host 与 NSIS）。
- [ ] Desktop Host 接线：交付文件在展示前调用 `verify_delivered_file(prepared.effective_policy, path)`；UI 依据 `PermissionAutoCancelled`、`StoppedUnconfirmed.remedy` 展示。

## 9 Windows 实测清单（Windows 第六轮修复顺序，给工程师）

### 9.1 本轮顺序（按序执行，前一步不过不进入下一步）

以普通用户（非管理员）运行 Fox / cargo，只有启用 / 停用脚本、快照脚本经提权；每步原样保存 `MEASURE` 行、结果 JSON 与日志。细项编号见 9.2。

① **Windows 默认测试与桥接 Node 测试全过**：`cargo test --locked -p fox-agent-backend --features acp --no-fail-fast`（分列执行 / 跳过：`#[ignore]` 2 个 + `SKIPPED` 名单 5 个）；`node --test crates\fox-agent-backend\runtime-assets\acp-bridge\bridge.test.mjs` 应 11/11（含拆分后的 (a)(b)(c)(d)，见 7.12；Windows 上不再要求 Host 关管道后收到 END）；`python -m unittest discover -s crates\fox-agent-backend\scripts\windows\tests` 15/15；`powershell -File crates\fox-agent-backend\scripts\windows\tests\test-enable-faults.ps1` 21/21（PS5.1）。
② **直接打包，不用 junction**：在普通（较深的）目录下 `python build-runtime-package.py stage <out> …` → `check` → `pack`，源与输出都用相对路径各跑一次、用 `\\?\` 绝对路径各跑一次；预期不出现 WinError 3、不误报 missing、深层文件可读，zip 条目全为正斜线（7.5）。记录最长安装路径。
③ **双故障错误保留、第二用户打开管道被拒**：(a) 提权 `test-enable-faults.ps1 -FullRun -PackagePath <②的 zip> -ExpectedSha256 <hex>`：结果 JSON `primaryError.message = FOX_SCRIPT_TEST_FAULTS: injected extract failure`、`cleanupErrors` 含 `staging … injected cleanup failure`、测试版本目录不存在、日志含 `TEST FAULTS ACTIVE`（7.7）；(b) `diag-srt-stdio.ps1 -Variants pipe-other-user -OtherUserCredential (Get-Credential) -PipeServerTest <target\debug\deps\bridge_pipe_windows-*.exe>`：`win32Error = 5`、`serverResult = FOX_PIPE_RESULT=no-connection`、`ownerControl = True`（7.16）。
④ **原始 pi T04 完整工具 turn**（9.2 第 4、5、11、18 项的环境变量；运行时包需含新的 `t04-fixtures\sandbox\scripted-cmd-provider.ts`）：`real_srt_pi_acp_t04_windows` 全程通过——身份（whoami 为 `srt-sandbox`，且模型请求计数 ≥ 2，不再 403）、授权目录 read（bash 孙进程与 pi read 工具）、授权内写入、列 Documents / profile 根被拒、网络允许目标可达 / 未列端口 403 / CONNECT 403 / 直连回环与 1.1.1.1 exit 7、关闭后每个祖先 ACE 全撤（断言 + `icacls` 复核）。dsh 沙箱 initialize 同时重跑。
⑤ **双并发 → 强杀 Host 恢复 → 跨账户强杀链**（9.2 第 6、7、9 项），每一步之前与之后各跑一次 `snapshot-sandbox-processes.ps1 -Label <步骤>`（提权），记录明确的 `count` 与 `ownerUnknownCount`：两个并发会话运行中；关闭 B 后；强杀 Host 后、`recover_ancestor_aces` 后；强杀 srt 的 node 后（two-hop，预期最终 `count = 0`，否则列出残留 PID / 名称 / 父 PID）。
⑥ **桌面 Host 与 NSIS**：Host 接线完成后，在安装包（NSIS）安装的 Fox 中走一遍首次启用沙箱（一次 UAC）→ 沙箱会话 → 卸载（停用脚本的保留 / 卸载判断，9.2 第 13 项）。

### 9.2 细项参考（第五轮 1–14、第六轮 15–21，仍有效）

以普通用户（非管理员）运行 Fox / cargo，只有启用 / 停用脚本经 UAC 提权；记录 OS 版本、杀软状态（火绒 / Defender）、srt 0.0.80、Node、pi 0.84.2 / pi-acp 0.0.34、dsh 0.2.1-rc.2。第三轮的清单已由第三轮报告覆盖，本轮清单针对第五轮改动；每项原样保存 `MEASURE` 行与脚本日志（`%ProgramData%\Fox\logs`）。

1. 编译与单元测试：`cargo test --locked -p fox-agent-backend --features acp --no-fail-fast`。预期第三轮 4 个夹具失败（`capability_declaration_follows_the_mode`、`authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp`、`chinese_paths_are_literal_and_do_not_affect_the_home_name`、`settings_deny_the_root_and_reallow_only_granted_and_system_paths`）消失：Linux 规则用例在 Windows 上不编译（`cfg(not(windows))`），对应的 Windows 变体（`windows_declaration_is_partial_and_carries_the_exposure_warning`、`windows_authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp`、`windows_chinese_profile_paths_are_literal_and_get_ancestor_aces`、`windows_settings_grant_only_user_paths_and_deny_nothing`）在所有平台都跑。需 Rust ≥ 1.89（`std::fs::File::lock`）。
2. 运行时包：在任意目录组装 zip，顶层 `node\`（含 `node.exe`）、`srt\`（`npm --prefix srt install @anthropic-ai/sandbox-runtime@0.0.80 --ignore-scripts`）、`backends\pi`（`pi-acp@0.0.34 @earendil-works/pi-coding-agent@0.84.2`）、`backends\dsh`（`@deepseek-ai/dsh@0.2.1-rc.2`）、`dsh-patches\`（复制本 crate `runtime-assets/dsh-patches/*.yml`）、`t04-fixtures\`（`fixtures/pi/pi-gated.cmd`、`fixtures/pi/fox-approval-gate.ts`、`fixtures/sandbox/scripted-cmd-provider.ts` 放在 `t04-fixtures\sandbox\`）。`Get-FileHash` 取 SHA256。
3. **一次 UAC 是否成立**：普通用户在 Windows PowerShell 5.1 中执行 `Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','<repo>\crates\fox-agent-backend\scripts\windows\enable-sandbox.ps1','-PackagePath','<zip>','-ExpectedSha256','<hex>','-Version','0.1.0','-ResultPath','%LOCALAPPDATA%\Fox\state\enable-sandbox-result.json'`。记录：共弹出几次 UAC（预期 1 次：srt 的 `windows-install` 自提权在已提权进程中不应再弹）；“标准用户 + 输入管理员凭据”的情形也跑一次（结果文件应写到调用用户的目录，因为路径由参数传入）。核对：`icacls "%ProgramFiles%\Fox\sandbox-runtime\0.1.0"` 为去继承、Administrators/SYSTEM F、Users RX、所有者 Administrators；`icacls "%ProgramData%\Fox\sandbox"` 为 Administrators/SYSTEM F、Users `(RA,X,AD,S)` 仅本目录、CREATOR OWNER `(OI)(CI)(IO)F`；结果 JSON 含 `srt.sandboxUserSid`、`markerVersion`、`createdAt`。再跑一次确认幂等（同哈希跳过复制）；改错 `-ExpectedSha256` 确认失败且不留目录；故意让 `windows-install` 失败（如端口段与已有安装不同）确认运行时目录被回滚。之后用 `apply_enable_result` 写入 `srt-sids.json` / `srt-install.json`（或手工按结果核对）。
4. 原始 pi T04 与 dsh（在第 3 项之后）：`set FOX_TEST_REAL_SRT=1`、`FOX_RUNTIME_ROOT=%ProgramFiles%\Fox\sandbox-runtime\0.1.0`、`FOX_SRT_PREFIX=%FOX_RUNTIME_ROOT%\srt`、`FOX_PI_ACP_PREFIX=%FOX_RUNTIME_ROOT%\backends\pi`、`FOX_SRT_NODE=%FOX_RUNTIME_ROOT%\node\node.exe`、`FOX_DSH_BIN=%FOX_RUNTIME_ROOT%\backends\dsh\node_modules\.bin\dsh.cmd`，运行 `cargo test -p fox-agent-backend --features acp --test sandbox_srt -- --include-ignored --nocapture`（`real_srt_pi_acp_t04_windows`、`real_srt_dsh_initialize_inside_sandbox`）。预期不再出现：WFP 自检 0x80070003、Node EPERM、dsh patch 的 `sandbox_path_not_owned`。私有 HOME 应在 `%ProgramData%\Fox\sandbox\<你的 SID>\homes\…`。
5. **中文空格路径 + 祖先 ACE**：T04 默认把项目建在 `%USERPROFILE%\Documents\中文 空格\proj-<pid>`（可用 `FOX_T04_PROJECT_PARENT` 改）。确认：`ancestor ACEs` 行列出 profile 根、`Documents`、`中文 空格`（以及 `%ProgramData%\Fox\sandbox\<SID>`、`homes`、`homes\<UUID>`、`tmp`）；沙箱内 `Get-ChildItem Documents` 与 profile 根均为 `LIST-DENIED`（只能 stat、不能列）；pi 能正常启动与读写项目；关闭后断言每个祖先上 Fox 的 ACE 已撤销，并用 `icacls` 人工复核这些目录上无 `srt-sandbox` 条目。另试一次 OneDrive 重定向的 Documents（若有）。
6. **两个并发会话交叉验证台账**：会话 A 授权 `Documents\中文 空格\a`，会话 B 授权 `Documents`（B 的 srt 会在 `Documents` 上收敛并抹掉 A 的祖先 ACE）。顺序：起 A → 起 B → 看 B 的 transition `finish()` 报告 `restored` 含 `Documents` → 关 B（srt 撤销 B 在 `Documents` 上的授权时再次抹掉）→ 确认 `finish()` 再次补回、A 仍可正常 stat → 关 A → 全部撤销。期间对照 `%LOCALAPPDATA%\Fox\state\win-ancestor-aces.json` 的引用计数。
7. **强杀 Host 后 reconcile**：T04 运行中强杀 cargo 测试进程（或 Fox Host），之后调用 `recover_ancestor_aces(&coordinator, &BTreeSet::new())`（或再次启动 Fox），确认台账清空、祖先 ACE 全部撤销；再启动一次 sandboxed 会话确认 srt 自己的崩溃恢复撤销了其 ACE。
8. **7 个孤儿对象所有权修复**：在第三轮留下孤儿的目录（或新跑一次让沙箱在授权目录与私有 HOME 写文件）上，先 `record_srt_sid` 记录当前 SID，再调用 `repair_sandbox_ownership(&[授权目录, 私有 HOME 根], &known_srt_sids(state)?, &RealWindowsAcl, &RealWindowsAcl)`；确认报告逐项列出、所有者变为本人（`icacls /verbose` 或 `Get-Acl`）；手工建一个由其他账户 / 不在台账的 SID 拥有的对象，确认不被改；放一个指向目录外的 junction，确认 `skipped_reparse_point` 且不进入。卸载 srt 后已成孤儿的对象：先把旧 SID 写进 `srt-sids.json` 再修复。
9. **跨账户强杀链**：T04 进行中用提权 `snapshot-sandbox-processes.ps1`（7.15，明确计数）记录 runner / node / bash；强杀 Fox 的 node（srt）进程后确认 `srt-sandbox` 名下进程全部结束（two-hop，7.4）；看 stderr 是否出现 `broker→runner Job assign not supported`。
10. **第四轮（b0bed84）的 7 项在 Windows 上复核**：停止确认（`ActiveProcesses == 0` → `Confirmed`）；未确认 → `StoppedUnconfirmed` 且拒绝重建；Stopping 状态下晚到审批自动答为取消（`PermissionAutoCancelled`）、新任务被拒；声明逐项等于写出的 settings；路径规范化（`junctioned_authorized_folder_is_judged_by_its_target`、UNC 被拒）；`verify_delivered_file`（授权可写目录内通过，会话临时目录 / 私有 HOME / 只读授权目录 / 经联接指出去的文件拒绝）；pi `cancel_process_tree = true` 的进程树清理。
11. **1.1.1.1 主机正对照**：在主机（沙箱外）先 `curl.exe -sS -m 15 http://1.1.1.1/` 确认可达（得到 HTTP 响应），再看 T04 中 `directip` 为 curl exit 7，证明拒绝来自 WFP 而不是目标不可达。
12. 暴露面警告（B）：`audit_broad_exposure()` 的结果经 `declaration.with_exposure(&report)` 后 `exposed_dirs` 与 `icacls D:\` 一致，`exposure_warning` 为固定文案；`directory_isolation` 仍为 false。
13. **停用 / 卸载边界**：`disable-sandbox.ps1 -StateDir … -InvokingUserProfile … -Version 0.1.0 -ResultPath …`：默认（未加 `-ConfirmNoOtherSrtClients`）应 `decision = keep` 并列出“无法排除其他使用者”；加上后在无活跃会话时 `decision = uninstall`、`windows-uninstall` 成功、`%ProgramData%\sandbox-runtime` 与本人 `%LOCALAPPDATA%\sandbox-runtime` 被删、`%ProgramData%\Fox\sandbox` 未动；T04 运行中执行应因活跃会话 `keep`；另一用户 profile 下放一个 `AppData\Local\sandbox-runtime\state.db` 应 `keep`；先用 npx 自行装 srt 再跑启用脚本，停用时应因“srt 原本已安装”`keep`。
14. 有余力：Win10 22H2 复跑 4–7；Defender 开启、火绒开启各跑一次 4。

第六轮新增（2026-10-20，分支 feat/four-agent-phase0-1 第六轮提交之后；第 4 项的原始 pi T04 / dsh 应在 16–17 通过后重跑）：

15. **原始 stdin 探针（回归项）**：普通用户运行 `powershell -NoProfile -ExecutionPolicy Bypass -File crates\fox-agent-backend\scripts\windows\diag-srt-stdio.ps1 -ResultPath %TEMP%\diag.json`（可加 `-RuntimeRoot`、`-SrtPath`、`-NodePath`、`-WorkDir`、`-Variants`）。预期（srt 0.0.80）：`direct` echo=True、END 在关闭 stdin 之后；`srt` 与 `srt-no-node-options` `endBeforeStdinClosed=True`（`stdinEofAfterReadyMs` ≈ 1 ms）、`stdoutRealtime=True`——两者相同说明与 NODE_OPTIONS 无关；`srt-bridge` connected / handshake / echoViaPipe / endViaPipe 均为 True、srt 退出码 0。保存 JSON 原文。srt 升级后再跑：若 `srt` 变体变为与 `direct` 一致，说明上游已修复。
16. **PS5.1 首装**：在 Windows PowerShell 5.1（不是 PS7）里按第 3 项跑 `enable-sandbox.ps1`，包用 `build-runtime-package.py pack` 生成（先 `check`，记录最长路径）。预期：结果 JSON `extractMethod = "tar"`、`packagePaths.maxLength` < 260、`ok = true`；故意给一个含超长路径的包（或 `-MaxPathLength 200`）应在解压前失败，`primaryError.message` 以 `package_path_too_long` 开头，日志列出文件，`cleanupErrors` 单列且不覆盖 `primaryError`；把 `%SystemRoot%\System32\tar.exe` 临时改名（或在没有 tar 的系统上）确认退回 `Expand-Archive`。确认 `%ProgramFiles%\Fox` 下不残留 `.s*` 暂存目录。
17. **桥接安全**：(a) T04 运行中用 `[System.IO.Directory]::GetFiles('\\.\pipe\') | ? { $_ -like '*fox-acp-*' }` 看端点；连接建立后该名字应消失或再连得到 `ERROR_PIPE_BUSY` / 找不到；(b) 以**另一个普通用户**打开管道 → 拒绝访问（第七轮起用 `diag-srt-stdio.ps1 -Variants pipe-other-user`，见 7.16 与 9.1③）；(c) 用当前用户的普通进程抢先连接并发错 token → Fox 报 `bridge_handshake_rejected`，会话不启动（可用 `cargo test … bridge_with_a_wrong_token_is_rejected` 在 Windows 上跑同样逻辑，该测试在 Windows 用真实命名管道）；(d) 让桥接永不连接（例如临时把 `acp-bridge\bridge.mjs` 改坏，Fox 应先报 `sandbox_bridge_unavailable`；或用 `cargo test … bridge_never_connecting_times_out`）→ `bridge_connect_timeout`；(e) 关闭会话后 `srt-sandbox` 名下进程在宽限内全部结束（桥接关闭后端 stdin，5 s 后结束后端），Job `ActiveProcesses == 0`。
18. **原始 pi T04 与 dsh 经桥接**：重跑第 4 项。预期 `initialize` 完成；失败时错误以 code 开头（`backend_exited_before_initialize` 带 `[exit code N]` 与 `srt_stderr_tail` / `backend_stderr_tail`），把整段保存。`cargo test -p fox-agent-backend --features acp --test acp_mock_agent` 在 Windows 上全过（含 4 个桥接测试，走真实命名管道）。
19. **锁路径一致性**：跑完 T04 / dsh 后确认 `%ProgramData%\Fox\sandbox\<SID>\homes\*\*\AppData\Local\sandbox-runtime` **不再出现**，`state.db` 与 `.acl-init.lock` 只在调用用户真实的 `%LOCALAPPDATA%\sandbox-runtime`；第五轮遗留在私有 HOME 下的目录应出现在 `PreparedLaunch::stray_srt_state`（启动报告），确认无 srt 会话后可用 `remove_stray_srt_state` 清理。`cargo test … windows_broker_gets_the_real_localappdata_so_the_acl_lock_paths_agree` 通过。
20. **`needs_elevation` 提权修复**：对 `D:\fox-t04-exposed-round3-20261019\mechanism-write.txt`（或同类：父目录给 Authenticated Users Modify、所有者为已记录 srt SID）先非提权跑 `repair_sandbox_ownership`，确认该项为 `needs_elevation` 而非 `failed`；用 `ownership_repair_list` 写出 JSON，再以管理员 `disable-sandbox.ps1 … -RepairOwnershipList <json>`（或单独 `repair-ownership.ps1 -ListPath <json> -ResultPath …`）；确认 `privileges` 两项为 0、该项 `repaired`、`Get-Acl` 所有者为本人且 DACL 未变；手工把清单中某项的 `expectedOwner` 改成别的 SID、或把路径换成经 junction 的路径，确认 `skipped`。
21. **暴露面审计计数**：同一台机器（3 盘）重跑 `audit_broad_exposure()`，确认 `audit_errors` 恰为此前的 7 项（路径与原因）、`status() == incomplete`、声明 `exposure_audit = "incomplete"`、`exposure_warning` 以“部分目录无法检查。”结尾；`checked` 中三类计数之和 = 审计的路径数。

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

## 12 Host 接线第一批（2026-10-20：P3 四入口路由 + P5 身份字段 + P6 UI 最小部分）

对应方案 v0.3.1 第 5.4、6.6 节。本批只把“会话绑定哪个外层后端”接进桌面 Host，**不接真实 ACP 运行**；外部后端的实际启动（sandbox `prepare_launch` + `bridge`）是下一批。

### 12.1 做了什么

- **身份字段（P5）**：`conversations.agent_backend TEXT NOT NULL DEFAULT 'fox' CHECK(IN fox/pi/codex/deepseek)`，桌面迁移 v95，只增不改（常量默认值的 `ADD COLUMN` 只改元数据，不重写旧行）。旧会话一律读作 `fox`；legacy 路径的标记照旧留在 `kernel_runs.kernel_mode='legacy'`，不改写。新建会话在 INSERT 中写入；触发器 `conversations_agent_backend_immutable`（`BEFORE UPDATE OF agent_backend … WHEN NEW IS NOT OLD → RAISE(ABORT,'agent_backend_immutable')`）拒绝任何修改（选触发器而非仓储层：任何代码路径包括手写 SQL 都绕不过去；同值写入与不涉及该列的更新不受影响）。`engine_id` 含义不变；不复用 `runtime_type`。迁移 SQL 的唯一来源是 `host_routing::AGENT_BACKEND_MIGRATION_SQL`，桌面 `MIGRATION_95` 直接引用它。
- **路由（P3）**：纯逻辑在 `src/host_routing.rs`（`choose_for_new_conversation`、`route`/`route_stored`、`session_access`、`backend_options`、`audit_line`）。桌面胶水 `apps/desktop/src-tauri/src/commands/agent_backend_routing.rs` 只读库、调用、转错误码：
  - 新建：`conversation_create` 接受可选 `agentBackend`（缺省 fox），校验开关后写入；非 fox 与会话专家、知识库（yuxi）智能体组合直接拒绝（`agent_backend_invalid`）。
  - start：`run_start` 在写任何 run / 附件绑定**之前**按冻结值路由；另在 `dispatch_started_run`（rewind、确认后派发等所有启动路径的汇合点）兜底，非 fox 会话的 run 被标为失败并返回同一错误码，绝不进入 Kernel / legacy / yuxi。
  - cancel：`run_cancel` 从 run 所属会话读冻结值；approve：`approval_resolve` 从 approval → tool_call → run → 会话读冻结值；resume：`run_resume` 与 `kernel_run_continue` 按会话读冻结值。请求里带的 `agentBackend` 与冻结值不一致 → `agent_backend_misrouted`（先于开关判断）。找不到的会话 / run / 审批交回原有路径，旧错误码不变。
  - 审计：cancel / approve 每次输出一行，start / resume / create 在拒绝时输出，格式 `fox.host_routing entry=… conversation=… run=… agent_backend=… engine_id=… routed=fox|refused=<code>`（stderr，与现有 Host 日志同路）。`engine_id` 取 `kernel_runs`，没有再取 `run_control_bindings`，都没有时打印 `-`，从不由 `agent_backend` 推断。
- **功能开关**：环境变量 `FOX_EXTERNAL_BACKENDS`（未设 / 空 / `0` / `off` → 全关；`all` / `1` / `on` → 全开；或列表 `pi,codex`；未知项忽略并在状态里列出，绝不等同“全开”）。选环境变量而不是设置项：外部运行时尚未接通，这是工程 / 企业管控而非用户偏好；不会被界面误开；现有安装天然为“关”，无需迁移；属于 6.5 第 1 层 Host 强制策略，只能收紧；与现有 `FOX_KERNEL_MODE` 同类。
  - 关闭：新建非 fox → `agent_backend_disabled`；已绑定外部后端的会话四个入口一律 `agent_backend_disabled`（只读，不启动运行时），读取与展示不受影响。
  - 开启：可以新建外部后端会话（绑定被写入并锁定），但四个入口一律 `agent_backend_not_yet_wired`，在创建 run 行或进程之前返回。
- **前端（P6 最小）**：`agent_backends_status` 命令给出四个后端的 enabled / wired / experimental / reason；输入框上方新会话显示“Agent 后端”选择，开关关闭时禁用并说明需设置 `FOX_EXTERNAL_BACKENDS`；已有非 Fox 会话显示“后端：X（已锁定）”与只读提示；已有 Fox 会话不加任何行。类型：`ConversationSummary.agentBackend`、`model/agent-backend.ts`；`startRun` / `resumeRun` 带冻结值，`cancelRun` / `resolveApproval` 在打开的会话上带冻结值。普通 Fox 新任务的 checklist / 强验收、work_mode_gate 未改。

### 12.2 没做什么

- 外部后端的真实启动、事件投影、审批卡映射（下一批：`sandbox::prepare_launch` + `bridge` + `AcpConnection`）。
- ~~外部会话的输入框不禁用~~（第一批修复已改：开关关闭的只读外部会话禁用输入框，见 12.5）；能力声明 / 审批状态的 UI 展示（P6 其余部分）。
- `conversation_fork` 产生的新会话按列默认值为 `fox`（外部会话当前无法运行，不存在需要继承的情况；接通外部后端时须决定 fork 规则）。子会话等其他 INSERT 同样走默认值 `fox`。
- `work_mode_gate` 对非 Fox 后端是否适用（5.4 第 3 条）未审查；当前非 Fox 会话根本到不了 gate。
- 迁移前自动备份：本批未新增（沿用现有发布 / 备份流程），验收清单要求手工备份副本演练。

### 12.3 测试

- `cargo test -p fox-agent-backend --features acp`：新增 `host_routing` 14 个单元测试（错投矩阵、开关解析、旧会话映射、关闭开关后的读取、审计行、迁移只增）。
- `cargo test -p fox-agent-backend --features sqlite-tests --test host_routing_sqlite`（需 C 编译器，默认不编）：用真实 SQLite 跑 `AGENT_BACKEND_MIGRATION_SQL`：旧行映射 fox 且不改写、新行写入、触发器拒改、CHECK 拒绝 `deepseek_harness` 等、迁移副本不动原库。
- 桌面 crate（未在本地运行）：`database/migrations.rs::agent_backend_migration_tests`（v94 → v95 升级、幂等、不可变）、`database/repositories/agent_backend.rs` 的两个测试、`apps/desktop/tests/agent-backend.test.ts`（bun）。

### 12.4 工程师验收清单

1. **Windows 编译**：`cargo build -p fox-desktop`（默认 features）与 `cargo test -p fox-desktop agent_backend`；确认 `agent_backend_migration_tests`、`repositories::agent_backend::tests`、`schema` 相关既有测试（含 `DATABASE_SCHEMA_VERSION == 95` 断言）全过；`bun test apps/desktop/tests/agent-backend.test.ts`；`tsc`（`pnpm --filter @fox/desktop build` 的 tsc 步骤）。
2. **旧库副本演练**：复制一份真实旧数据目录（含 `fox.db`，先关 Fox）。用新版本指向副本启动 → 旧会话列表、历史、旧 run（含 Codex / DeepSeek 模型适配的旧会话、legacy 会话）照常显示；`SELECT agent_backend, COUNT(*) FROM conversations GROUP BY 1` 只有 `fox`；`SELECT version FROM schema_migrations WHERE version=95` 一行；Fox 旧会话继续对话 / 继续运行正常。再在副本上执行 `UPDATE conversations SET agent_backend='codex' WHERE id=…` 应报 `agent_backend_immutable`。中断重试：迁移中途杀进程后重启，v95 只记一次、数据完整。原库未被改动。
3. **错投测试**（开发者工具或临时脚本调用 invoke）：对 Fox 会话 `run_start` / `run_cancel` / `approval_resolve` / `run_resume` 各带 `agentBackend: "codex"` → 均返回 `agent_backend_misrouted`，且 run_start 不产生新 run 行；带 `"fox"` 或不带 → 行为与改动前一致；带 `"deepseek_harness"` → `agent_backend_invalid`。stderr 中能看到 `fox.host_routing … agent_backend=fox engine_id=… refused=agent_backend_misrouted`。
4. **开关关闭（默认）**：新会话“Agent 后端”下拉禁用，提示需设置 `FOX_EXTERNAL_BACKENDS`；普通 Fox 新任务（含 checklist / 强验收）与改动前一致；invoke `conversation_create` 带 `agentBackend:"pi"` → `agent_backend_disabled`。
5. **开关开启**：设 `FOX_EXTERNAL_BACKENDS=pi` 重启 → 下拉只有 pi 可选（Codex / DeepSeek 显示未启用）；选 pi 发送 → 会话被创建并显示“后端：pi（已锁定）”，发送返回 `agent_backend_not_yet_wired`，无新 run 行、无子进程。
6. **再关开关**：去掉变量重启 → 上一步的 pi 会话仍在列表中可打开查看，显示只读提示；发送 / 取消返回 `agent_backend_disabled`，不启动运行时。

### 12.5 第一批修复（2026-10-20，针对 3a82f5d 的 Windows 验收）

**A. 桌面全量测试 63 项失败**

- 结论：不是本批引入的。工程师在父提交 `a9254d3` 上复跑同一名单，其中 61 项同样失败，属既有问题，本批不修。另外 2 项在 `3a82f5d` 上精确复测通过，父提交也通过，判断为负载 / 时序波动：
  - `delivery_live_tests::a_crash_while_finalizing_the_nth_item_leaves_recoverable_state`
  - `work_tools::tests::a0_snapshot_and_state_update_p95_stay_within_budget`
- 代码证据：
  - `dispatch_started_run` 需要 Tauri `AppHandle`，只有 `commands/` 里的命令会调用它（`run_start`、rewind、`kernel_reconciliation`、确认后派发）。`runtime_host/kernel_coordinator/**` 的测试直接驱动 `Database` + `RuntimeHost`，在 `runtime_host/` 里搜不到对 `dispatch_started_run` 或 `commands::` 的调用。也就是说，本批加的兜底和那次多出的查库，这些测试一次都执行不到。
  - 上面 2 项波动测试的实测路径也都不经过 Host 命令层：
    - 前者是 delivery finalize 的故障注入（`finalize_fault`，按线程设置），加上本地模型桩。
    - 后者计时的是 `load_conversation` 和 `work_tasks().update`。本批在这条路径上只多了 `query_conversation` 的一列 `c.agent_backend`，没有多一次查询。v95 触发器是 `BEFORE UPDATE OF agent_backend`，不更新这一列的 UPDATE 不会触发它；`work_tasks` 本身也不在 `conversations` 表上。
  - 没有找到能说明“多出的一次查库会放大波动”的证据，所以这两项不改。
- 其他嫌疑点的排查（都没发现问题）：
  - 没有对 `conversations` 做无列名的 `INSERT … VALUES`。
  - 没有在 v95 之后重建 `conversations` 表。
  - `ConversationSummary` 只有 Serialize，没有 Deserialize，也没有快照比对。
  - 版本号断言都引用常量 `DATABASE_SCHEMA_VERSION`，写死 94 的只有已改成 95 的那一处。
  - 5 处会话查询都已带出新列，`map_conversation` 的 5 个调用点全部覆盖。
- 兜底改为与其他入口一致（`commands/agent_backend_routing.rs`）。`gate_conversation`（start / resume / `kernel_run_continue` / `dispatch_started_run` 兜底）和 `gate_run_identity`（cancel / approve）统一按以下规则处理：
  - 会话 / run / 审批不存在（`None`）：交回原有路径，和以前一样。
  - 读库出错：不再返回 `storage.operation_failed`。先写一行审计 `fox.host_routing entry=… routed=fox_fallback reason=storage_error|column_missing detail="…"`（列不存在记为 `column_missing`），再放行到原有路径，由原有路径自己按原来的方式报错。
  - 只有确认存储值不是 `fox` 时才拦下：外部后端得到 `disabled` / `not_yet_wired`；存储值非法得到 `agent_backend_invalid`。请求里显式写了不同的后端，照旧返回 `misrouted`。
  - 取舍：读库失败时是“放行”（fail-open）而不是“拦截”。这样兜底永远不会制造原有路径本来没有的失败。本批外部运行时还没接通，即使放行，也只会走 Fox 自己的路径，而这条路径依赖同一个数据库，坏库会由它自己报错。接通外部后端时要重新评估：届时读库失败对非 Fox 会话应改为拦截，并且要在 `prepare_launch` 前再校验一次。
  - 新增单元测试 `commands::agent_backend_routing::tests`，共 3 个。

**B. 后端选择栏位置**

- 原因：选择栏原先作为 `.fox-chat-stage`（flex 列）的普通子元素，渲染在 Composer 之外。`.fox-composer-wrap` 是 `position:absolute; bottom:12px; z-index:16`，所以选择栏掉到舞台底部，被输入框盖住。时间线预留的底部空间（`--fox-composer-occlusion`，等于舞台底到 composer 顶的距离）也没有算上它。
- 修复：
  - Composer 新增 `topSlot`，选择栏作为 `.fox-composer-wrap` 的第一个子元素，紧贴在 `.fox-prompt-shell` 之上，随输入框一起定位（新会话居中态和历史态都一样）。
  - 它的高度计入 `onHeightChange`，因此锁定 / 只读说明不会再被输入框或时间线遮挡。
  - 选择栏改为不透明底色（`--fox-canvas`），宽度 100%，说明文字在 composer 宽度内换行。
  - workbench 用 `useMemo` 缓存这个元素，避免破坏 `MemoizedComposer` 的 memo。
- 1122×770 下的推算：低于 1180px 的紧凑断点，侧栏展开时历史态 composer 宽约 700–750px，最长的锁定说明加上 chip 占 1 行，最多 2 行，选择栏高约 24–42px。

**C. pi 发送被 Host 拒绝后的界面残留**

- 原因：
  - `send` 的 catch 里调用了 `refreshSelectedConversation`，而它用的 `mergeConversationDetail` 会保留当前的乐观消息和 `pending-run-*`，所以“正在处理 / 等待开始”残留下来。
  - catch 还执行了 `setError`，workbench 随之把 `chatState` 设为 `error`，页头显示“处理失败”。
- 修复：
  - Host 返回 `agent_backend_*`（invalid / disabled / misrouted / not_yet_wired）时，`send` 先用 `withoutRefusedSend` 撤掉乐观消息和 pending run，恢复发送前的 lastRun。
  - 不设置 `error` / `errorDetails`，改为写入 `agentBackendRefusal`（按会话区分），并返回 `'agent_backend_refused'`。
  - workbench 收到这个结果后清掉自己的 `pendingUserMessage`，`chatState` 设为 `complete`，并对 composer 返回 `'error'`，这样草稿保留、发送按钮复位。
  - 拒绝原因只在选择栏里显示一条（`role="alert"`），替换掉原来的锁定 / 选择说明，不弹 toast。
  - 其他错误码的处理路径不变。
- 只读禁用：外部会话在开关关闭时（`agentBackendInputLockedReason`，即 access 为 `read_only_disabled`；状态还没读到时同样按只读处理）：
  - 输入框和发送按钮禁用，并显示占位说明；
  - hook 层也会在乐观渲染之前本地拒绝，不调用 Host；
  - 开关开启但运行时未接通（`read_only_not_wired`）时，输入框仍可用，发送会得到 Host 的 `not_yet_wired`，按上面的方式干净地显示拒绝。
  - Fox 会话不受影响。
- 顺带修复（本批引入的前端回归）：`apps/desktop/tests/conversation-draft-selection.test.ts` 把 hook 源码放进沙箱运行，依赖注入表里缺少 `conversationAgentBackend`，所以在 `3a82f5d` 上有 2 项抛 ReferenceError，`a9254d3` 上 16/16 通过。已把本批用到的纯函数补进注入表。
- 新增前端测试：
  - `tests/agent-backend.test.ts` 增加 3 项；
  - 新文件 `tests/agent-backend-send.test.ts` 3 项，沿用沙箱写法，覆盖 not_yet_wired 撤掉乐观状态、只读会话不发送、其他错误路径不变。

**复测清单（工程师，在本次修复的新提交上执行）**

1. `cargo test -p fox-desktop`（全量）：
   - 预期只剩父提交 `a9254d3` 上同样失败的 61 项，名单逐项一致；
   - 另外单独复跑那 2 项波动测试各 3 次，应当通过；
   - 再跑 `cargo test -p fox-desktop agent_backend`，应包含 `commands::agent_backend_routing::tests` 3 项、`repositories::agent_backend::tests` 2 项、`agent_backend_migration_tests` 2 项，全部通过。
2. 跑以下前端测试，都应通过：
   - `bun test apps/desktop/tests/agent-backend.test.ts apps/desktop/tests/agent-backend-send.test.ts apps/desktop/tests/conversation-draft-selection.test.ts`；
   - 相关 DOM 测试 `fox-composer-*-dom`、`fox-thread-hybrid-dom`、`conversation-change-expert-dom`。
   - 另跑前端 `tsc`。
3. `cargo test -p fox-agent-backend --features acp`：163 个，156 执行 / 7 跳过 / 0 失败。
4. **界面复测 B**（窗口 1122×770，另各看一次最大化）：
   - 设置 `FOX_EXTERNAL_BACKENDS=pi` 后，新会话页的“Agent 后端”行紧贴在输入框正上方，不再贴着窗口底部；
   - 选 pi 发送后，历史态的“后端：pi（已锁定）”和说明完整可见，没有被输入框遮挡，滚动时间线时说明下面没有文字透出；
   - Fox 会话的输入框区域与改动前一致。
5. **界面复测 C**：
   - 在 `FOX_EXTERNAL_BACKENDS=pi` 下选 pi 新会话发送 → 不残留临时消息，没有“正在处理”“等待开始”，页头没有“处理失败”；选择栏里只出现一条拒绝提示（not_yet_wired 的文案），草稿保留；数据库里没有对应的 message 或 run。
   - 去掉变量重启 → 打开该 pi 会话，输入框和发送按钮都是禁用状态，占位文字为“此会话为只读，不能发送”。stderr 里没有 `run_start` 的审计行，说明请求没有发到 Host。
   - Fox 会话正常发送不受影响。
6. **兜底回归**：
   - 普通 Fox 会话的发送、rewind、确认后派发、继续运行都与改动前一致；
   - 可选：用只读文件锁让读库失败，stderr 应出现 `routed=fox_fallback reason=storage_error`，并且之后报出的错误码与 `a9254d3` 相同。


### 12.6 `fox-agent-backend` 测试计数：Linux 与 Windows 的口径（2026-10-20）

两边都是 **0 失败**。总数不同是平台条件编译和门控方式不同造成的，不是回退。

| 平台 | 命令 | 总数 | 执行 | 跳过 / 未执行 | 失败 |
|---|---|---|---|---|---|
| Linux（aarch64，本机基线） | `cargo test -p fox-agent-backend --features acp` | 163 | 156 | 7 | 0 |
| Windows（工程师实测） | 同上 | 152 | 146 | 6 | 0 |

- **Linux 的 7 个跳过** = 2 个 `#[ignore]` + 5 个由 `FOX_TEST_REAL_ACP` 门控的 real ACP 测试。后者在函数体内打印 `SKIPPED` 后直接返回，libtest 把它们记为 passed，所以统计里显示为“passed”，实际没有执行。
- **Windows 的 6 个未执行** = 3 个 real ACP 测试（同样在函数体内 `SKIPPED`，计为 passed）+ 3 个 `ignored`。
- **平台独有的单元测试**：Linux 独有 14 个，Windows 独有 4 个（条件编译，只在各自平台编进测试二进制）。real ACP 门控测试的数量两边也不同（Linux 5 个、Windows 3 个）。因此两边的总数和执行数本来就不相等；按执行数判断回退时，用**同一平台**上一轮的结果做基线，不要拿 Linux 和 Windows 互比。
- 说明：按上面的分项粗算，Windows 总数（152）与“Linux 163 − 14 + 4 = 153”差 1 项，差异来源尚未逐项核对（可能在门控/ignore 的分布里）。下一轮 Windows 复测时请导出 `cargo test -p fox-agent-backend --features acp -- --list` 的名单，与 Linux 名单逐项比对后补齐这一行。
- **`host_routing_sqlite` 在只开 `acp` 时是 0 项**：这个集成测试文件要求 feature `sqlite-tests`（rusqlite bundled，需要 C 编译器，默认不编）。只开 `acp` 时这个测试二进制的用例数为 0，这是预期行为，不是测试丢失。要跑它，用 `cargo test -p fox-agent-backend --features sqlite-tests --test host_routing_sqlite`。
- **回退判定**：同一平台上，执行数下降、或任何一项从 ok 变成 FAILED / 从执行变成 SKIPPED，才算回退。

## 13 外部后端接通批（2026-10-20：pi / Codex / DeepSeek 经 ACP 真实运行）

对应方案 v0.3.1 第 6.1–6.4、6.7 节与 v0.3.2 增补。第 12 节的路由不再返回 `agent_backend_not_yet_wired`：开关打开后，外部会话的 start / cancel / approve 交给外部运行时，Fox 会话的路径不变。

### 13.1 做了什么

- **crate（`--features acp`）**
  - `src/external.rs`：每个会话一个 ACP 连接的运行时。
    - 一个会话同时只有一轮。
    - 已知原生会话 id 且后端声明了 `loadSession` 时用 `session/load`（不回放历史），否则 `session/new`。原生 id 记为 `agent_backend.session` 事件。
    - `session/update` 经 `src/projection.rs` 投影成 Fox 运行事件（与 yuxi 同一套，复用现有线程与工具卡）。旧连接代次的事件只审计。
    - 取消后 15 s（`CANCEL_CONFIRM_TIMEOUT`）内后端没有以 `cancelled` 结束，就以 `cancelled` + `outcome: "unknown"` 结束并停进程，不显示为“已确认取消”。
    - 进程在一轮中途退出：run 失败，错误码 `agent_backend_exited`，附脱敏的 stderr 尾部。下一轮重新启动并重开原生会话。
    - 原生审批：显示为审批卡，用户的回答写成原生一次性选项（`allow_once` / `reject_once`）。DeepSeek 在 Host 补丁下审批策略为 `never`，如果仍有请求到达，由 Host 直接拒绝，不显示为可答。
  - `src/external_launch.rs`：真实启动器。按会话的权限模式（默认 `sandboxed`，不降级）走 `prepare_launch` + srt，Windows 走桥接。
    - 后端入口**只**从受管运行时 `<runtime>/backends/<pi|codex|deepseek>/node_modules/.bin/<pi-acp|codex-acp|dsh>[.cmd]` 找，或用开发覆盖 `FOX_*_PROGRAM`。不用 `npx`。都找不到时返回 `agent_backend_not_installed`。
  - `src/capability_view.rs`：四维能力视图（审批 / 沙箱 / 取消 / 可观测）。启动前给预估（`planned`），启动后给实际值（`live`）。
  - `host_routing.rs`：新增 `Route::External`。resume 对外部会话没有意义，返回 `agent_backend_entry_unsupported`。读冻结值失败时：只要有一个外部后端开着，或请求里写了非 Fox 后端，就拦下（`agent_backend_lookup_failed`）；外部后端全关时照旧回落 Fox 路径（第 12.5 节 A 的取舍在此按约定收紧）。
- **桌面**
  - `src-tauri/src/external_backend.rs`：薄胶水。
    - 事件经 `record_external_event` 落库并发 `fox://runtime-event`。
    - 审批卡用 Host 工具调用 `agent_permission`（pending）+ `approvals` 行 + `fox://approval-requested`。
    - 外部审批只接受“允许一次 / 拒绝”。
    - 附件暂不支持（`agent_backend_attachments_unsupported`）。
    - 会话必须绑定一个可访问的项目文件夹（`agent_backend_project_required`）。
    - 启动时把上次残留的外部 run 标为中断（`agent_backend_host_restarted`，结果未知）。
    - 会话归档或移入回收站时在后台停掉它的后端。
  - 新命令 `agent_backend_capabilities`。
  - `Cargo.toml` 给 `fox-agent-backend` 打开 `acp`。
- **前端**：后端栏显示四个能力 chip（`agentBackendCapabilityChips`，顺序固定；预估值的 title 带“（启动前预估）”）。
- **无 `run_control_binding` 的外部 run**（已核实，并有测试 `repositories::agent_backend::tests::external_runs_without_a_binding_resolve_approvals_and_complete`）：
  - `resolve_approval` → `run_approval_window` 在没有 binding 时不报错。（审批无超时批起）审批不再有窗口，多久之后批准都有效，见 13.6；原先“超过 300 000 ms 记为 `expired` 并按拒绝答给后端”已取消。
  - `run.completed` → `enforce_managed_run_completion_budget` 只约束子 run 和数字同事 run，外部 run 直接通过。
  - Fox 路径未改。

### 13.2 环境变量

| 变量 | 作用 |
|---|---|
| `FOX_EXTERNAL_BACKENDS` | 总开关（第 12 节），默认关 |
| `FOX_AGENT_RUNTIME_ROOT` | 覆盖受管运行时目录（所有 OS）。默认：Windows `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（`FOX_RUNTIME_ROOT`，否则 `FOX_RUNTIME_VERSION`，否则取已安装的最新版本）；Linux / macOS `<fox data>/agent-runtime` |
| `FOX_PI_ACP_PROGRAM` / `FOX_CODEX_ACP_PROGRAM` / `FOX_DSH_PROGRAM` | 开发覆盖：指定已安装的入口，优先于受管运行时。不经 `npx` |
| `FOX_EXTERNAL_FULL_ACCESS` | 工程授予 full_access：`1` 表示所有外部会话，或列 `conversation:<id>` / `project:<root>` |
| `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN` | `1` 时禁止任何 full_access，优先级最高 |
| `FOX_EXTERNAL_ENV_PASSTHROUGH` | 透传给后端的 Host 变量名列表（如 `RUSTUP_HOME,CARGO_HOME`），**仅 full_access 生效**，沙箱模式忽略 |
| `FOX_EXTERNAL_EXTRA_DOMAINS` | Host 层追加的沙箱域名，按后端写，如 `pi=api.example.com;codex=127.0.0.1:3425`。本地模型网关写成 `127.0.0.1:<端口>` |
| `FOX_PI_PROVIDERS` | pi 允许的 provider 域名组（如 `anthropic,openai`） |

### 13.3 没做什么（待办）

- ~~真实冒烟：在 srt 内跑 pi-acp / dsh~~：Linux 已做，见 13.8；Codex 对接假的 Responses API 仍未做。
- ~~迁移前自动备份~~：已做，见 13.7。
- 受管运行时目录布局（`backends/<后端>/node_modules/.bin/<入口>`）需要与工程师 0.1.1 运行时包的实际结构核对，不一致时以包为准改 `external_launch.rs`。
- `kernel_gateway_catalogue_size_is_pinned` 仍断言 71；实际是 72 = 35 资源 + 28 工作 + 9 委派（`4b54e68` 新增 `present_files`），等目录负责人确认后再改。
- fork / 子会话的后端继承规则（第 12.2 节）未定。

### 13.4 精简 Windows 复测清单（在本批 HEAD 上；**已被 13.5 取代**，保留作历史）

1. **定向测试**
   - `cargo test -p fox-agent-backend --features acp`：0 失败。Linux 本机为 187 项（180 执行 + 2 ignored + 5 个 `FOX_TEST_REAL_ACP` 门控 SKIPPED），其中 `external_runtime` 9 项。Windows 用上一轮 Windows 的数字做基线（12.6），预期执行数比上一轮多出本批新增的用例。
   - `cargo test -p fox-desktop agent_backend`：应包含 `commands::agent_backend_routing::tests` 3 项、`repositories::agent_backend::tests` 4 项（新增 `external_runs_without_a_binding_resolve_approvals_and_complete`）、`agent_backend_migration_tests` 2 项，全部通过。
   - `cargo test -p fox-desktop kernel_work_tool_missing_field_is_refused_before_execution_as_data kernel_delegation_stages_a_single_child_without_starting_an_executor`：两项通过。`kernel_gateway_catalogue_size_is_pinned` 预期失败（72 ≠ 71，见 13.3）。
   - `bun test apps/desktop/tests/agent-backend.test.ts apps/desktop/tests/agent-backend-send.test.ts apps/desktop/tests/conversation-draft-selection.test.ts`：28 项通过。另跑前端 `tsc`。
2. **每个后端一次真实会话冒烟**（`FOX_EXTERNAL_BACKENDS=pi,codex,deepseek`，受管运行时已装，会话绑定项目文件夹，沙箱模式）
   - pi 和 Codex 各做一次：
     - 新会话发一条会用到工具的消息 → 流式文本 + 工具卡 → 正常结束；
     - 再发一条 → 复用同一原生会话（stderr / 事件里 `agent_backend.session` 的 id 不变）；
     - 触发一次原生审批 → 审批卡只有“允许一次 / 拒绝”，批准后工具继续；
     - 一轮中途点取消 → 以 `cancelled` 结束。15 s 内没有确认的显示“结果未知”。
   - DeepSeek：私有 HOME 的 `.dsh/cordis.patch.yml` 有 `- id: acp` 行（见 `docs/deepseek.md` 第 4 节）→ 一轮正常结束、回复非空，且不出现可答的审批卡。
   - 负向检查：
     - 临时改名入口 → `agent_backend_not_installed`，没有子进程；
     - 一轮进行中杀掉后端进程 → `agent_backend_exited`；
     - 一轮进行中重启 Fox → 那一轮显示 `agent_backend_host_restarted`；
     - 后端栏四个 chip 在启动前带“（启动前预估）”，启动后去掉。
   - Fox 会话的发送、审批、取消与改动前一致。
3. **一次全量** `cargo test -p fox-desktop`：与父基线 `a9254d3` 的 61 项既有失败逐项比对。名单的变化只允许来自本批拆分出的测试：`kernel_gateway_catalogue_size_is_pinned` 预期失败（新名字）；如果 `kernel_delegation_stages_a_single_child_without_starting_an_executor` 或 `kernel_work_snapshot_uses_projected_tool_identity_and_frozen_gateway` 在基线名单里，它们现在应当通过，并从名单中消失。

### 13.5 合并精简 Windows 复测清单（取代 13.4；**已被 13.9 取代**，保留作历史）

在审批无超时批的最终 HEAD 上跑一次即可，13.4 不再单独执行。仍需要的项已并入 13.9。

1. **定向测试**
   - `cargo test -p fox-agent-backend --features acp`：0 失败。按名单区分“真正执行 / ignored / `FOX_TEST_REAL_ACP` 门控 SKIPPED”（12.6）。新增：`launch::tests::dev_entry_prefers_override_then_managed_runtime_then_npx`（普通单测）；`sandbox_srt` 新增两项 `#[ignore]`：`real_srt_external_runtime_pi_session`、`real_srt_external_runtime_dsh_session`（Linux 专用，Windows 上不编译）。
   - `cargo test -p fox-desktop agent_backend`、`approval`、`migration_backup`：全部通过。重点：`repositories::agent_backend::tests::external_runs_without_a_binding_resolve_approvals_and_complete` 现在断言关闭会话 → 审批 `cancelled`（`conversation_closed`）、重启中断 → `cancelled`（`app_restarted`）、其他中断仍为 `expired`（`run_interrupted`）。
   - `bun test`：`apps/desktop/tests/conversation-attention.test.ts`（含新增“关闭/归档/删除会话或重启 Fox 记为 cancelled，显示已取消”）、`agent-backend*.test.ts` 等；另跑前端 `tsc`。
2. **真实 ACP 测试的入口（新）**：Codex / pi / dsh 的真实测试不再写死 `npx -y <包>`。入口顺序：`FOX_CODEX_ACP_PROGRAM` / `FOX_PI_ACP_PROGRAM` / `FOX_DSH_PROGRAM`（dsh 也认旧名 `FOX_DSH_BIN`）→ 受管运行时 `<runtime>/backends/<后端>/node_modules/.bin/<入口>.cmd`（`FOX_AGENT_RUNTIME_ROOT`；Windows 还认 `FOX_RUNTIME_ROOT`、`FOX_RUNTIME_VERSION` 或 `%ProgramFiles%\Fox\sandbox-runtime` 下最新版本）→ 最后才 `npx`（只在测试中用，`launch::dev_launch_spec`）。Windows 上不再需要 `npx.cmd` 转发：装好受管运行时后直接 `set FOX_TEST_REAL_ACP=1` 跑 `pi_backend`、`codex_backend`、`deepseek_backend`。测试会在 stderr 打印实际入口（`pi-acp entry (Managed(..))` 等），请把这一行贴进报告。
3. **dsh 真实 prompt 测试必须有真回复（新）**：`deepseek_backend::real::real_dsh_initialize_session_and_prompt` 断言 `stopReason = end_turn` 且收到非空 agent 文本；模型报错（无 key、key 被拒、provider 路由不对）一律**失败**，不再算通过。跑之前给 dsh 配好模型（`DEEPSEEK_API_KEY` 或私有 HOME 的 `acp` 行）；没有模型时这一项的预期结果是 FAILED，报告里写明原因，不要当回退。
4. **三后端真实会话冒烟**（`FOX_EXTERNAL_BACKENDS=pi,codex,deepseek`，沙箱模式，会话绑定项目文件夹）
   - pi 与 Codex 各一次：流式文本 + 工具卡 → 正常结束；第二轮复用同一原生会话；原生审批只有“允许一次 / 拒绝”，批准后工具继续、拒绝后工具不执行。
   - **审批 > 6 分钟不答**：卡片一直是“等你处理”，本轮不自动结束，之后点“允许一次”工具照常执行（旧的 Node 6 分钟与 5 分钟窗口都已去掉）。
   - **取消本轮**：审批卡显示“已取消”，本轮以 `cancelled` 结束；15 s 内后端没确认的显示“结果未知”。
   - **关闭 / 归档 / 移入回收站 / 删除会话（新语义）**：若有待处理审批，库里记为 `cancelled`（`decision_json.reason = conversation_closed`），界面显示“已取消”，不再是“已过期”或“已结束（本轮已结束）”。
   - **退出 Fox 后重启（新语义）**：上次留下的外部 run 记为中断（`agent_backend_host_restarted`），其待处理审批记为 `cancelled`（`reason = app_restarted`），显示“已取消”；Fox 自己的 legacy run 在退出时（`runtime.application_exit`）同样处理。
   - DeepSeek：一轮正常结束、回复非空，不出现可答的审批卡。
   - 负向：入口改名 → `agent_backend_not_installed`；杀后端 → `agent_backend_exited`；四个能力 chip 的“（启动前预估）”在启动后去掉。
5. **Fox 会话同测**：发送、审批（含 > 6 分钟后再批）、取消（显示“已取消”）与改动前一致；会话列表“等你处理”徽标在审批出现时亮、作答或取消后灭。
6. **旧库启动**：用一份旧版本数据库启动 Fox，`<data>/backups/` 生成一份迁移前备份（最多保留 5 份）；人为让备份失败（例如目录只读）时不迁移、启动报错。
7. **一次全量** `cargo test -p fox-desktop`：与父基线 `a9254d3` 的 61 项既有失败逐项比对；名单变化只允许来自本批改名/拆分的测试（5 个旧测试名改名后消失），其余新增失败都算回退。

### 13.6 审批无超时（审批语义变化）

原生审批和 Fox 自己的审批都**不再有期限**，与 Claude Code / Codex CLI / ACP 的语义一致：一条审批只在下面几种情况结束。

| 结束条件 | 审批记为 | 界面 |
|---|---|---|
| 用户点“允许一次” / “拒绝” | `approved` / `denied` | 已批准 / 用户拒绝 |
| 用户取消本轮（外部：Host 把待答的原生请求答为 `outcome: cancelled`） | `cancelled` | 已取消 |
| 关闭、归档、移入回收站或删除会话（外部：`agent_backend_session_closed`） | `cancelled`（`reason: conversation_closed`） | 已取消 |
| Fox 退出或重启时本轮被中断（`runtime.application_exit`、`agent_backend_host_restarted`） | `cancelled`（`reason: app_restarted`） | 已取消 |
| 本轮因其他原因结束（失败、完成、其他中断、新一轮开始） | `expired`（`reason` 为 `run_failed` / `run_completed` / `run_interrupted` / `new_run_started`） | 已结束（本轮已结束） |
| 旧版本写下的按窗口过期的行 | `expired` | 已过期 |

关闭会话和重启的处理在本批由 `expired + run_interrupted` 改为 `cancelled`（用户决定，2026-10-20）：这两种情况不是“过期”，是本轮被结束了。判定在 `database/repositories.rs` 的 `approval_cancel_reason_for_interruption`（按中断码）。数字同事无人值守时的审批策略（例如自动拒绝）本批不处理。

**超时来源 A–H 的处理摘要**（按本批提交归纳）：

- **A Kernel 控制器**：`WaitingApproval` 不再自动过期，只等用户；等待期间执行预算暂停（`crates/fox-agent-kernel/src/controller.rs`）。
- **B Kernel 数据库**：审批提交不再拒绝“迟到”的决定，时钟回拨也不作废等待（`repositories/kernel.rs`）。
- **C Kernel Host 审批命令**：过了名义窗口也照常接受（`repositories/kernel_host.rs`）。
- **D 桌面数据库**：审批不再按窗口记为 `expired`；`run_approval_window` 只校验冻结绑定；新增审批请求时间与等待时长（`repositories.rs`、`repositories/run_control.rs`）。
- **E legacy 运行时 Host**：审批等待无超时，只在用户作答或取消 / 关闭时结束（`runtime_host/mod.rs`）。
- **F Node agent-runtime**：去掉 `HOST_REQUEST_TIMEOUT_MS`（原 6 分钟固定超时）。代价：Host 卡死时这一轮只能靠取消结束（见风险）。
- **G 协议**：`approval_wait_ms` 只保留为兼容字段，不再是期限（`fox-engine-protocol/src/control.rs`）。
- **H 外部运行时**：`src/external.rs` 对原生审批不设期限；pi 的 Fox 审批门（`fox-approval-gate.ts`）也不设超时，只在 Host 作答、`cancelled` 或本轮中止时放行 / 阻止；dsh 在 Host 补丁下不发审批，到达的由 Host 直接拒绝。

**墙钟扣除审批等待**：子 Run 和数字同事 Run 的墙钟期限只计“非等待审批”的时间，扣除的是本 run 所有审批等待区间的**并集**（并行的两条审批不重复扣）；托管 Run 的完成期限同样扣除。外部 run 没有 `run_control_binding`，不受这些期限约束。

### 13.7 迁移前自动备份

- 桌面启动用 `open_with_migration_backup` 打开数据库：有待执行的迁移时，先用 `VACUUM INTO` 把当前库备份到 `<data>/backups/`，再迁移。
- 只保留最近 5 份。
- 备份失败就**不迁移**，启动报错，原库不动。
- 测试：`migration_backup` 4 项（纯 SQLite），Linux 已跑通。

### 13.8 真实冒烟：Linux srt 内的外部会话（`tests/sandbox_srt.rs`）

两项新测试走生产路径 `ExternalRuntime` + `HostLauncher`（受管运行时布局、`prepare_launch`、srt + bubblewrap、initialize 版本检查），不是 mock 启动器：

- `real_srt_external_runtime_pi_session`：真实 pi-acp 0.0.34 + pi 0.84.2 在 srt 里。依次：启动 → 流式文本 → `run.completed`；权限请求 → 允许 → 工具执行、文件写入；权限请求 → 拒绝 → 工具不执行、本轮正常结束；审批不答等待 `FOX_TEST_APPROVAL_WAIT_SECS`（默认 20 s）期间每秒断言“仍 pending、本轮无终态、进程活着”，之后允许仍有效；审批待答时取消本轮 → 原生请求被答 `cancelled`、run 以 `cancelled` 结束、晚到的回答不写、工具不执行；再来一轮正常完成；关闭会话确认停止且无残留进程。
- `real_srt_external_runtime_dsh_session`：真实 dsh 0.2.1-rc.2 在 srt 里：启动、initialize、session/new、一轮 prompt 必须 `end_turn` 且有非空文本，否则失败；关闭确认。需要 `DEEPSEEK_API_KEY` 和能连到 api.deepseek.com 的网络。
- 模型：沿用 T04 的 `ScriptedModel`，在沙箱外的测试进程里，`127.0.0.1:<端口>` 写进 Host 允许列表（`extra_domains`，同时让 Host 为后端清掉 NO_PROXY）。Host 会清空子进程环境，所以测试往受管运行时写一个 `pi-gate/pi-gated.sh` 包装脚本，由它带上脚本 provider 和模型地址（仅测试）。
- 运行方法（Linux，需 bubblewrap、socat、ripgrep、Node ≥ 22.12）：

```text
FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 \
FOX_SRT_PREFIX=<srt 的 npm prefix> FOX_PI_ACP_PREFIX=<pi-acp 的 npm prefix> [FOX_DSH_BIN=<dsh 绝对路径>] \
[FOX_AGENT_RUNTIME_ROOT=<可写的受管运行时>] [FOX_TEST_APPROVAL_WAIT_SECS=400] \
cargo test -j2 -p fox-agent-backend --features acp --test sandbox_srt real_srt_external_runtime \
  -- --include-ignored --nocapture --test-threads=1
```

  不设 `FOX_AGENT_RUNTIME_ROOT` 时，测试在 cargo 的 target tmp 下用 `cp -al`（失败则 `cp -a`）拼出受管运行时；不能用指向运行时外的符号链接，沙箱里读不到。`FOX_PI_ACP_PROGRAM`、`FOX_DSH_PROGRAM`、`FOX_EXTERNAL_FULL_ACCESS` 必须不设。两个门控变量缺一个就失败，不会静默通过。
- Linux 实测结果见审批无超时批报告：pi 全流程通过（含一次 400 s 不答仍可批）；dsh 在无 key 时启动、会话、结算与关闭都正常，但按新规则 prompt 断言失败。

### 13.9 合并精简 Windows 复测清单（afb 修复批；取代 13.5）

在 afb 修复批的最终 HEAD 上跑一次即可，13.4 / 13.5 不再单独执行。本批内容：规范 JSON（`fox_agent_kernel::canonical_json`，桌面 `crate::canonical_json` 重导出）、bridge 握手带 `{cwd, env}`、verbatim 路径简化、`LoadSettled` / `PromptStarted` 栅栏、`agent_backend_not_installed`、Path 比较、受管目录 `backends\dsh` + 运行时 0.1.2、重启由外部 Host 中断外部 run、能力 chip 随 `agent_backend.session` 刷新、打包脚本打入 pi 审批门。

1. **定向测试**（挑代表，不跑全部 37 项 Graph 用例）
   - 规范 JSON（Graph 37 项回退的根因）：
     - `cargo test -p fox-desktop canonical_json`：`canonical_json::tests::desktop_build_has_preserve_order_and_canonical_json_still_sorts`、`canonical_json::tests::host_source_has_no_raw_serde_json_serializers`（守护测试：Host 源码里不得再出现 `serde_json::to_string(` 等）。
     - `database::repositories::work_graph::tests::canonical_run_execution_profile_matches_trigger_literals_and_hashes`。
     - graph_lead 3 项：`database::repositories::graph_lead::tests::activates_branching_read_only_graph_and_projects_stable_readiness`、`...::a_new_same_conversation_durable_lead_run_can_resume_the_persisted_graph`、`...::active_node_attempt_and_child_snapshot_survive_database_restart`。
     - migrations graph_node 2 项：`database::migrations::tests::graph_node_cancel_migration_enforces_two_phase_negative_reconciliation`、`database::migrations::tests::graph_node_finish_migration_is_idempotent_and_enforces_evidence_contract`。
     - legacy_expert：`database::repositories::tests::legacy_expert_v1_fixture_preserves_snapshot_and_remote_knowledge_contract`。
     - attachment 1 项：`database::repositories::tests::persists_attachments_artifacts_bindings_and_approval_lifecycle`。
     - `cargo test -p fox-agent-kernel`：全部通过（本 crate 的测试以 `preserve_order` 构建，与桌面二进制同条件）。
   - 第 4 项两个测试（只改测试，用户消息改为 `/目标`）：`runtime_host::kernel_coordinator::tests::kernel_work_tool_missing_field_is_refused_before_execution_as_data`、`runtime_host::kernel_coordinator::tests::kernel_goal_propose_in_conversation_is_answered_by_the_gate`（新增）。
   - 重启（第 6 项）：`database::repositories::agent_backend::tests::restart_leaves_external_runs_to_the_external_host`（新增）与 `...::external_runs_without_a_binding_resolve_approvals_and_complete`；另跑 `cargo test -p fox-desktop agent_backend`、`approval`、`migration_backup` 全部通过。
   - `cargo test -p fox-agent-backend --features acp`：0 失败。本批新增：`bridge::tests::listener_answers_with_the_child_env_and_cwd`、`launch::tests::bridge_answer_carries_env_and_cwd_as_one_json_line`、`launch::tests::verbatim_windows_paths_are_simplified_for_backends`、`sandbox::tests::windows_bridge_command_sends_backend_env_through_the_bridge_not_cmd`、`external_runtime::reopening_with_session_load_projects_only_events_after_the_load`（longreplay mock）。Windows 上 `bridge_pipe_windows` 也要跑。按名单区分“真正执行 / ignored / `FOX_TEST_REAL_ACP` 门控 SKIPPED”（12.6）。
   - `node --test crates/fox-agent-backend/runtime-assets/acp-bridge/`：14 项通过（含 env/cwd 握手 3 项）。
   - `bun test apps/desktop/tests/agent-backend-capability-refresh.test.ts apps/desktop/tests/agent-backend.test.ts apps/desktop/tests/agent-backend-send.test.ts apps/desktop/tests/conversation-attention.test.ts`：全部通过；另跑前端 `tsc`。
   - `python -m unittest discover -s crates/fox-agent-backend/scripts/windows/tests`：17 项通过。
2. **运行时包 0.1.2 用打包脚本重打，不再手工拷贝**
   - `python crates\fox-agent-backend\scripts\windows\build-runtime-package.py stage <out> --node-dir <node> --srt-prefix <srt> --backend pi=<prefix> --backend codex=<prefix> --backend dsh=<prefix> --prune-dts`
   - `... pack <out> --out <zip>`（`--version` 默认 0.1.2，与 `RUNTIME_PACKAGE_VERSION` 一致，单测会核对）。pack 会检查 `acp-bridge\bridge.mjs`、`pi-gate\pi-gated.cmd`、`pi-gate\fox-approval-gate.ts` 与仓库副本一致，缺任何一个都拒绝打包。
   - 用 `enable-sandbox.ps1` 装到 `%ProgramFiles%\Fox\sandbox-runtime\0.1.2\`；确认 `backends\dsh`（不是 `deepseek`）与 `pi-gate\` 都在。
3. **三个后端各一次真实 UI 冒烟**（`FOX_EXTERNAL_BACKENDS=pi,codex,deepseek`，沙箱模式，会话绑定项目文件夹）
   - Codex 能启动，一轮正常结束（流式文本 + 工具卡）。
   - 相对路径读取正确（如“读 README.md”）；dsh 不再报 `realpath 'D:'`。
   - 一轮结束后杀后端进程，再打开会话发消息：`session/load` 复用原生会话，历史不重复、不漏（时间线不出现回放的旧事件）。
   - 审批超过 6 分钟不答，之后点“允许一次”，工具照常执行。
   - 取消本轮：本轮以 `cancelled` 结束，审批卡显示“已取消”。
   - 关闭 / 归档 / 删除会话后，原待处理审批显示“已取消”（`reason = conversation_closed`）。
   - 一轮进行中（最好带一条待处理审批）退出 Fox 再启动：那一轮的错误码为 `agent_backend_host_restarted`（不是 `runtime.application_restarted`），待处理审批为 `cancelled`（`reason = app_restarted`），显示“已取消”。
   - 能力 chip：握手完成（`agent_backend.session` 事件）后立即从“（启动前预估）”切到后端实际声明，不必等这一轮结束。
   - 入口缺失（临时改名受管入口）：错误码为 `agent_backend_not_installed`，没有子进程。
   - DeepSeek 一轮正常结束、回复非空（需配置模型），不出现可答的审批卡。
   - Fox 会话的发送、审批、取消与改动前一致。
   - 13.5 中仍需要的：旧库启动生成迁移前备份（`<data>/backups/`，最多 5 份；备份失败不迁移）；真实 ACP 测试按 13.5 第 2 项的入口顺序跑并贴出 stderr 的入口行；dsh 真实 prompt 无模型时预期 FAILED（13.5 第 3 项）。
4. **一次全量** `cargo test -p fox-desktop`：与 `c058c2f` 的 64 项既有失败基线逐项对比。
   - 基线名单里的改名项：`runtime_host::kernel_coordinator::tests::f1_engine_path_tests::f3_approval_tests::f3_fixed_clock_expired_card_is_rejected_after_real_node_proposal`（旧名）在 `097a933` 之后改名为 `...::f3_fixed_clock_card_past_nominal_window_is_still_accepted_after_real_node_proposal`（新名），是同一项基线失败（`requests=0`，环境问题：真实 Node 提案未发出请求），比对时按同一项计，不算新增。**做 diff 时**：基线名单里的旧名（基线 64 项中的第 27 项）与本次结果里的新名互相抵消，既不算“已恢复”，也不算“新增失败”。另：f2bf567 的 Windows 复测中，新名单跑 5/5 通过，全量时可能偶发 `requests=0` 失败，属负载敏感（见 13.10）。
   - Graph 那 37 项（规范 JSON 回退）应全部恢复，从失败名单中消失。
   - 单独列出：`runtime_host::kernel_coordinator::tests::kernel_gateway_catalogue_size_is_pinned`（目录数 72）；第 4 项两个测试（`kernel_work_tool_missing_field_is_refused_before_execution_as_data`、`kernel_goal_propose_in_conversation_is_answered_by_the_gate`），写明通过与否。
   - 其余任何新增失败都算回退。

### 13.10 a5e2c9d 复测后续批：最小 Windows 复测清单（2026-10-21）

> **13.11 更新**：下面写的 DeepSeek 回环端口放行已撤回。dsh 对回环地址直连、不经 srt 代理，放行端口没有作用。现在的做法是：沙箱模式下 DeepSeek 的 base URL 若指向回环地址，启动前直接拒绝（`agent_backend_loopback_requires_full_access`）。第 4 项冒烟以 13.11 为准。

在本批最终 HEAD 上跑。**不需要跑全量** `cargo test -p fox-desktop`：a5e2c9d 的全量已复测（64 项，与基线持平，Graph 37 项已恢复），本批只改下列测试与 DeepSeek 回环端口推导。

本批内容：

- 审批不再过期后，两条测试的预期跟着改（只改测试）：
  - `real_eval_driver_keeps_unanswered_approval_pending_until_cancel`：终态为 `cancelled`（不是 `approval_expired`）；监视器已排过一次取消，测试末尾的重复取消命中幂等分支返回 `Ok(false)`，视为成功（终态 Run 的 `Err` 也接受，只有 `Ok(true)` 即对已取消 Run 新入队才算错）。
  - `f1_cropped_read_expired_confirmation_cannot_write` 改名为 `f1_cropped_read_unanswered_confirmation_stays_pending_and_cancel_cannot_write`：时钟推进 86,000,000 ms 后 Run 仍 `waiting_approval`、审批仍 `pending`、专门替换授权仍 `pending`、不落盘；取消后 Run 与审批为 `cancelled`，重复取消 `Ok(false)`，迟到的“允许一次”入队失败，授权不是 `approved`/`consumed`，目标文件不变。
  - `f3_fixed_clock_card_past_nominal_window_is_still_accepted_after_real_node_proposal` 不改：单跑通过（Windows 5/5）；全量时可能偶发 `requests=0` 失败，属负载敏感。它是基线项改名（13.9 第 4 项；diff 时与旧名 `f3_fixed_clock_expired_card_is_rejected_after_real_node_proposal`，即基线第 27/64 项，互相抵消）。
- `tests/bridge_pipe_windows.rs` 的 owner-control 断言：读到 `\n` 为止，断言以 `OK ` 开头、后面是合法 JSON、含 `cwd`（等于测试设的目录）与 `env`（含测试设的 `FOX_PIPE_PROBE`）。全仓库 grep 同类断言（Rust 与 Node）：`src/bridge.rs` 单元测试里的 `b"OK\n"` 是直接调用 `server_handshake` 时由测试自己传入的 answer，不是过期断言；`windows_pipe_is_first_instance_single_client` 与 Unix 版断言的 `OK {"env":{}}` 与当前实现一致；`bridge.test.mjs` 的 `'OK\n'` 测的是 bridge 对旧式纯 `OK` 的兼容（`parseAnswer('OK')`），保留；`scripts/windows/diag-srt-stdio.ps1` 作为假 Host 回 `OK\n`，bridge 仍接受，保留。`src/bridge.rs` 模块注释里的握手描述已更正为 `OK {"cwd":..,"env":{..}}\n`。
- DeepSeek 回环网关端口（`src/loopback.rs`、`backends::deepseek::base_url_candidates`、`HostLaunchConfig::sandbox_policy`）：
  - 复用第六轮脚本模型的做法：把 `127.0.0.1:<端口>`（IP 字面量）写进 srt `network.allowedDomains`，srt 代理对允许列表里的 IP 字面量不做回环拒绝。只加这一个端口。
  - 端口来源：私有 HOME 里 dsh 自己的配置 `<私有 HOME>/.dsh/cordis.patch.yml` 与 `<私有 HOME>/.dsh/profiles/acp/cordis.patch.yml` 中每个 `baseURL:`（`llm-deepseek` 行，或 `magpie` 这类自定义 provider）。**不读 `.env`**：Linux 实测 dsh 0.2.1-rc.2 在项目 `.env` 或 `$DSH_HOME/.env` 设了 `DEEPSEEK_BASE_URL` 时直接拒绝启动（“only the launching environment may set it”），而沙箱会话的启动环境里不会有这个变量（不在子进程环境白名单里；Windows 上 Host 变量过不了 srt）。
  - 规则：只看主机是回环的 base URL（`127.0.0.0/8`、`localhost`、`*.localhost`、`::1`）；端口取显式端口，否则取协议默认（http 80、https 443）；非回环 base URL 不加；值不是字面 URL（`!!js`、块标量、`${VAR}`/`$VAR`、非 http(s)、端口非法、带 userinfo）视为端口无法确定，**不放行任何回环端口**并给诊断；出现两个不同的回环端点视为不确定，同样不放行并给诊断。
  - 声明：`SandboxPolicy` / `SandboxDeclaration` 新增 `loopback_gateway`（`host`、`port`、`entry`、`source`）与 `loopback_diagnostics`；能力视图沙箱维度的详情写“放行本机回环端口：127.0.0.1:3425（仅此一个，来自 …）”，或“未放行本机回环端口：<诊断>”。
  - **已知不足（Linux 实测）**：只放行端口**不能**让 dsh 的请求到达网关。dsh 的 `@deepseek-ai/dsh-http-proxy` 对回环主机写死直连（`proxyForUrl` 中 `isLoopbackHost` → `undefined`，与 `NO_PROXY` 无关），根本不经过 srt 代理，允许列表管不到它。Linux：bwrap 网络命名空间里的 `127.0.0.1` 是沙箱自己的回环，实测 `real_srt_external_runtime_dsh_loopback_gateway` 端口已推导并写入 `allowedDomains`（`["api.deepseek.com","127.0.0.1:<端口>"]`），但网关收到 0 个请求，本轮以 `DeepSeek Messages transport failed` 失败。Windows：WFP 只放行 srt 代理端口段（默认 60080–60089）的回环连接，srt 0.0.80 没有按端口放行直连回环的配置，结论相同（推断，待 Windows 复测确认）。不给 dsh 打补丁的前提下，要真正打通需另做决定（见本批报告的风险一节）。

最小复测清单（Windows，本批 HEAD）：

1. 三条定向测试（桌面 crate；全路径由模块声明推断，Linux 上桌面测试二进制列不出来，**未经 `cargo test -- --list` 核对**）：
   - `cargo test -p fox-desktop runtime_host::kernel_coordinator::tests::real_eval_tests::real_eval_driver_keeps_unanswered_approval_pending_until_cancel -- --exact`：通过。
   - `cargo test -p fox-desktop runtime_host::kernel_coordinator::tests::rev_repro_tests::f1_model_delivery_tests::f1_cropped_read_unanswered_confirmation_stays_pending_and_cancel_cannot_write -- --exact`：通过；旧名 `...::f1_cropped_read_expired_confirmation_cannot_write` 不再存在。
   - `cargo test -p fox-desktop runtime_host::kernel_coordinator::tests::f1_engine_path_tests::f3_approval_tests::f3_fixed_clock_card_past_nominal_window_is_still_accepted_after_real_node_proposal -- --exact`：单跑通过（Windows 5/5）；全量时可能偶发 `requests=0` 失败，属负载敏感。
   - 若 `--exact` 匹配不到，先 `cargo test -p fox-desktop -- --list | findstr /i "real_eval_driver_keeps f1_cropped_read_unanswered f3_fixed_clock_card"` 取实际全名。
2. bridge 管道（Windows 真机，按 9.1 的方式由 `diag-srt-stdio.ps1 -Variants pipe-other-user` 驱动）：
   - `set FOX_TEST_PIPE_SERVE=1& cargo test -p fox-agent-backend --features acp --test bridge_pipe_windows -- --ignored --nocapture`，测试名 `bridge_pipe_serve_for_other_user_probe`：另一账户打不开（Win32 5），`FOX_PIPE_CONTROL=owner-connected`，owner 握手答复为 `OK {"cwd":…,"env":{"FOX_PIPE_PROBE":"owner-control"}}`。
   - 另跑 `cargo test -p fox-agent-backend --features acp --lib bridge::`（含 `bridge::tests::windows_pipe_is_first_instance_single_client`）。
3. 第 3 项新单测（Linux 已过，Windows 上再跑一次）：`cargo test -p fox-agent-backend --features acp --lib -- loopback`，应恰好执行这 7 项且全部通过：
   - `loopback::tests::loopback_base_urls_give_their_port`
   - `loopback::tests::remote_and_undetermined_base_urls`
   - `loopback::tests::exactly_one_loopback_port_is_allowed`
   - `loopback::tests::ambiguous_or_undetermined_allows_nothing_with_a_diagnostic`
   - `loopback::tests::yaml_base_urls_are_read`
   - `backends::deepseek::tests::loopback_gateway_port_comes_from_the_private_home_dsh_config`
   - `external_launch::tests::deepseek_loopback_gateway_port_is_allowed_from_its_base_url_only`
4. 一轮 DeepSeek 真实 UI 冒烟（`FOX_EXTERNAL_BACKENDS=deepseek`，沙箱模式，会话绑定项目文件夹；私有 HOME 的 `.dsh/cordis.patch.yml` 有 `- id: acp` 行与 Magpie 的 `baseURL`）：
   - 能力 chip 的沙箱详情写出“放行本机回环端口：127.0.0.1:3425（仅此一个，来自 …cordis.patch.yml baseURL）”；若写的是“未放行本机回环端口：…”，贴出诊断。
   - 发一句话能拿到模型回复。**按 Linux 实测，经 Magpie（回环）预计仍失败**（dsh 直连回环被 WFP 拦截），请贴出本轮错误与 dsh stderr；若成功，请注明，这会推翻上面的推断。
   - 再做一次审批：dsh 在 Host 补丁下不发审批（13.9），因此这一步在 DeepSeek 会话里应看不到可答的审批卡；如需验证审批链路，在同一构建的 Fox 会话里做一次“允许一次”，确认正常。
5. 不需要跑全量 `cargo test -p fox-desktop`，也不需要重跑 13.9 的其余条目。

### 13.11 f2bf567 复测后续批：最小 Windows 复测清单（2026-10-21；**已被 13.12 取代**）

在本批最终 HEAD 上跑。**不需要跑全量** `cargo test -p fox-desktop`：f2bf567 在 Windows 上已全部复测通过。本批只改了下面几处。

本批内容：

- **prompt 失败时持久化 stderr 尾部**（`src/acp.rs`、`src/external.rs`）
  - 以下几种失败，`run.failed` 都会带上 `diagnostic`，格式为 `{"kind":"stderr_tail","source":…,"redacted":true,"maxBytes":4096,"stderrTail":"…"}`：
    - prompt 失败（`agent_backend_prompt_failed`，`source=prompt_failed`）
    - 对话中途后端退出（`agent_backend_exited`，`source=backend_exited`）
    - 会话打开失败（`agent_backend_session_failed`，或 initialize 未先到达；`source=session_failed`）
    - 启动失败：原来已有的 `InitializeFailure`，`source=initialize`
  - 尾部取进程 stderr 环形缓冲（8 KiB）的最后 4 KiB，按行切。写入前再脱敏一次（`acp::redact_diagnostic_tail`）。遮掉的内容：
    - `sk-…` 令牌
    - 名字里含 key / token / secret / password / passwd / credential / authorization / cookie（或正好是 `auth`）的赋值的值。名字保留，例如 `DEEPSEEK_API_KEY=[redacted]`、`"api_key":"[redacted]"`
    - `Authorization: Bearer <x>`、`x-api-key: <x>`、`token: <x>` 这类头部写法的值
    - `Bearer` / `Basic` 后面的词
    - 32 位以上的不透明串
    - URL 里的用户名和密码
    - 路径（含用户名的路径）保留。
  - 前端：`RunFailure` 新增 `stderrTail` / `stderrTailSource`，只取自终态事件。即使持久化的 run 行覆盖了 code 和 message，尾部也保留。“查看错误详情”展开后显示 `code: message`，再加“后端 stderr 尾部（已脱敏，最多 4 KiB；…）”。
- **DeepSeek 回环网关只允许完全访问**（`src/external_launch.rs`、`src/loopback.rs`、`src/backends/deepseek.rs`、`src/capability_view.rs`）
  - 拒绝条件：沙箱模式下启动 DeepSeek 会话前，读私有 HOME 里 dsh 的 `cordis.patch.yml`（两份，同 13.10）。只要有一个 `baseURL` 指向回环地址，就拒绝启动，不起任何进程。这里的回环地址包括端口无法确定的写法，例如 `http://localhost:${PORT}`。
  - 错误码 `agent_backend_loopback_requires_full_access`。提示为“本机网关在沙箱模式下不可达：…dsh 对回环地址总是直连、不经沙箱代理（请切换到“完全访问”，或改用官方 api.deepseek.com）”。
  - 不受影响的情况：完全访问模式；非回环的 base URL；其他后端。
  - 能力视图：DeepSeek 在沙箱模式下，沙箱详情会写出这个限制。
  - f2bf567 新增的“只放行这一个回环端口”逻辑不再生效，放行列表里不再有 `127.0.0.1:<端口>`：
    - 原因：dsh 的 `proxyForUrl` 对回环地址写死直连，根本不经 srt 代理，放行没有作用。
    - pi 等其他场景也用不上：pi 的本机网关走的是第六轮的 `FOX_EXTERNAL_EXTRA_DOMAINS` 加清空 `NO_PROXY`，与这套按 base URL 推导的逻辑无关；而按 base URL 推导本来就只给 DeepSeek 用。
    - 函数保留：`derive_loopback_gateway`、`classify_base_url`，以及新增的 `names_loopback`。用途是做上面的拒绝判断、写声明诊断，以及等 dsh 上游加了开关以后重新启用（`deepseek::LOOPBACK_VIA_PROXY_SUPPORTED`，目前为 `false`）。
  - 真实用例 `real_srt_external_runtime_dsh_loopback_gateway`：
    - 现在断言沙箱模式下明确拒绝：`run.failed`、新错误码、10 s 内结束、没有 `agent_backend.session`、网关 0 个请求。Linux 实测约 0.1 s 拒绝。
    - 等 `LOOPBACK_VIA_PROXY_SUPPORTED = true` 以后，同一个用例就是“上游加开关后的验收用例”：放行这一个端口，并且请求能到达网关。
  - 给 dsh 上游的 issue 草稿（未提交），要点：`proxyForUrl` 对回环返回 `undefined`，`withLoopback` 强制把回环并入 `NO_PROXY`。请求增加一个默认关闭的“回环也走代理”选项（环境变量或配置），用于沙箱里唯一出口是宿主机代理的场景。

最小复测清单（Windows，本批 HEAD）：

1. stderr 尾部的新测试（需要 PATH 里有 `node`，mock 后端是 Node 脚本）：
   - `cargo test -p fox-agent-backend --features acp --test external_runtime a_failed_prompt_or_exit_persists_the_redacted_stderr_tail`：通过。prompt 失败和中途退出两种情况，`run.failed.diagnostic.stderrTail` 都含最后一行 `Connection error (last line)` 和 `DEEPSEEK_API_KEY=[redacted]`，整个事件里不出现 mock 写入的任何密钥原文。
   - `cargo test -p fox-agent-backend --features acp --lib -- redaction`：恰好 3 项，全部通过。
   - `cargo test -p fox-agent-backend --features acp --lib -- loopback`：恰好 9 项，全部通过。包括 `external_launch::tests::host_launcher_refuses_a_sandboxed_deepseek_loopback_gateway_before_starting` 和 `external_launch::tests::deepseek_loopback_gateway_is_refused_in_the_sandbox_and_never_allowed`。
   - 前端：`bun test tests/run-failure.test.ts`（在 `apps/desktop` 下）：11 项全部通过，其中新增 3 项在 `run failure stderr diagnostic` 组。
2. DeepSeek 经 Magpie，沙箱模式（`FOX_EXTERNAL_BACKENDS=deepseek`；私有 HOME 的 `.dsh/cordis.patch.yml` 里 `baseURL` 指向 Magpie 的 `127.0.0.1:<端口>`）：
   - 发一句话，应**立即**（几秒内，不等模型）失败，错误码为 `agent_backend_loopback_requires_full_access`，提示含“本机网关在沙箱模式下不可达”“请切换到“完全访问”，或改用官方 api.deepseek.com”。
   - 不应出现 dsh 进程，也不应出现 `Connection error`。
   - 能力 chip 的沙箱详情写有“限制：DeepSeek 的 base URL 若指向本机回环网关…”。
3. 同一会话切到“完全访问”，再跑一轮真实对话：能拿到模型回复，`run.completed`。
   - **注（5f2f2d8 复测后补）**：这一项在 5f2f2d8 上实际走的是工程师**真实 HOME** 里的 dsh 配置和官方 api.deepseek.com（进程连的是 3.173.21.63:443，会话日志也写进了真实 HOME），并没有经过 Magpie，也没有用私有 HOME。完全访问的 HOME 已在 5f2 后续批改为私有 HOME，本项**已由 13.12 第 2 项取代**。
4. 制造一次 prompt 失败，例如完全访问模式下把 Magpie 停掉，或者把 `baseURL` 改成一个没有监听的回环端口，再发一句话：
   - 本轮以 `agent_backend_prompt_failed`（或 `agent_backend_exited`）结束。
   - 点“查看错误详情”，能看到“后端 stderr 尾部（已脱敏，最多 4 KiB；本轮请求失败）”和 dsh 的 stderr。
   - 其中不应出现 API key、`Bearer` 值等原文。有密钥原文漏出来的话，请贴出脱敏前的样式（自己先遮掉）。
5. 不需要跑全量 `cargo test -p fox-desktop`，也不需要重跑 13.9 / 13.10 的其余条目。

### 13.12 5f2f2d8 复测后续批：最小 Windows 复测清单（2026-10-21；取代 13.11）

在本批最终 HEAD 上跑。**不跑全量**：13.11 的四项已在 5f2f2d8 上通过，本批只改了下面几处。

本批内容：

- **完全访问也使用 Fox 私有 HOME**（`src/sandbox.rs` 的 `private_home_env` / `prepare_full_access`，`src/external_launch.rs` 的 `full_access_private`，`src/capability_view.rs`）
  - 问题：5f2f2d8 的完全访问原样启动后端，不设 HOME。dsh 读的是用户真实 HOME 的配置，直连了官方 api.deepseek.com，会话日志也写进真实 HOME；而回环拒绝检查读的是私有 HOME，两边对不上。
  - 现在：完全访问只取消隔离（不包 srt），环境变量与沙箱模式一样指向本项目本后端的私有 HOME：`HOME`、`XDG_CONFIG_HOME/CACHE_HOME/DATA_HOME/STATE_HOME`、`TMPDIR/TMP/TEMP`（本会话临时目录），Windows 另有 `USERPROFILE`、`APPDATA`、`LOCALAPPDATA`；Codex 另有 `CODEX_HOME=<私有 HOME>\.codex`，DeepSeek 另有 `DSH_HOME=<私有 HOME>\.dsh`；pi 没有专用变量，它的配置在 `HOME` 下，同样跟着走。三个外部后端在沙箱模式下本来都改 HOME，没有例外。
  - 不设的：srt 和它的代理才需要的 `CLAUDE_CODE_TMPDIR`、`NODE_USE_ENV_PROXY`、`NODE_OPTIONS`；`PATH` 按环境白名单继承（与 5f2f2d8 的完全访问相同）。
  - 私有 HOME 与会话临时目录不存在就创建；会话结束时删除会话临时目录，私有 HOME 保留。Windows 上与沙箱模式一样走 `prepare_windows_private_root`（创建并校验 `%ProgramData%\Fox\sandbox\<SID>` 的 DACL）。
  - 能力 chip：完全访问的沙箱详情写明“完全访问模式：不套沙箱，不承诺目录边界；使用 Fox 私有 HOME（与沙箱模式相同的登录与配置）”。
- **回环拒绝文案**：remedy 改为“需要完全访问权限（当前构建由 Host 配置 FOX_EXTERNAL_FULL_ACCESS 开启），或改用官方 api.deepseek.com”；能力 chip 里的 DeepSeek 限制说明同步改。
- **脱敏只做一次、幂等**（`src/acp.rs`、`src/external.rs`）
  - 问题：同一段 stderr 在采集、取尾部、写诊断三处各脱敏一次，出现 `DEEPSEEK_API_KEY=[redacted]]]`，`{"authorization":"Bearer [redacted]` 丢了收尾的 `"}`。
  - 现在：采集和取尾部只按行切、截取；只在最终出口脱敏一次：`run.failed.diagnostic`（`external::stderr_diagnostic`）、启动失败报告（`InitializeFailure::attach_tails`，其诊断不再二次脱敏）、调试用的 `AcpConnection::stderr_tail()`。
  - 占位符改为 `<redacted>`（不含 `[` `]`、引号、逗号、大括号），`redact(redact(x)) == redact(x)`。
  - `Bearer` / 头部写法后面的凭据只遮到第一个 JSON 边界（`"` `'` `,` `}` `]` `)` `;`），后面的结构保留并继续按规则处理，例如 `{"authorization":"Bearer <redacted>","x-api-key":"<redacted>"}`。
- **长行显示**：mock 的 `stderr-fail` / `stderr-crash` 场景新增一条约 1960 字节、不含空格的长行和一条中文行。前端“查看错误详情”的 `<pre>` 改为 `white-space: pre-wrap; overflow-wrap: anywhere`，最大高度 20rem，超出纵向滚动（内联样式加 `aui-error-diagnostic` 类，样式表未加载时也生效）。时间线、thread 错误卡片、助手消息三处都经同一个 `ErrorState` 渲染，一处改动三处生效。

**完全访问怎么开**：目前只能由 Host 环境变量 `FOX_EXTERNAL_FULL_ACCESS` 开启（`1` / `all` 表示所有会话，或 `conversation:<会话 id>` 只开指定会话），**界面上没有开关**。

**真实 HOME 里的 dsh 登录此后在 Fox 里不再生效**：完全访问与沙箱模式都只读私有 HOME。需要在私有 HOME 里配置：

- 路径规则（Windows）：`%ProgramData%\Fox\sandbox\<你的用户 SID>\homes\<项目 UUID>\deepseek`，dsh 的配置目录是其下的 `.dsh`（即 `DSH_HOME`）。Codex 是 `...\homes\<项目 UUID>\codex\.codex`，pi 是 `...\homes\<项目 UUID>\pi`（配置在其下 `.pi`）。用户 SID 可用 `whoami /user` 查；项目 UUID 是 Fox 数据库里项目的稳定 UUID（小写 8-4-4-4-12），一个项目一份、一个后端一份，换项目或换后端要重新配置。
- 路径规则（Linux / macOS）：`<Fox 数据目录>/sandbox-homes/<项目 UUID>/<后端>`。
- dsh 需要的文件（**979313c 收尾批按 Windows 实测更正**；原示例 `baseURL: http://127.0.0.1:3425/anthropic` 经 Magpie 会 404）：
  - 两份文件都在私有 HOME 的 `.dsh` 下（即 `DSH_HOME`）：`.dsh\.env` 放密钥，`.dsh\cordis.patch.yml`（或 `.dsh\profiles\acp\cordis.patch.yml`）放 baseURL 和模型。dsh 不接受在 `.env` 里写 `DEEPSEEK_BASE_URL`（只认启动环境），所以 baseURL 必须写在 patch 里。
  - **baseURL 与 `/v1` 的关系**（对照已安装的 dsh 0.2.1-rc.2 源码 `@deepseek-ai/dsh-llm-deepseek/lib/index.js` 的 `messagesApiRoot()`）：去掉末尾 `/` 后，路径以 `/v1` 结尾就原样用，否则补 `/v1`，请求发到 `<根>/messages`。所以写 `http://127.0.0.1:3425/v1` 请求 `/v1/messages`；写 `http://127.0.0.1:3425/anthropic` 会请求 `/anthropic/v1/messages`，Magpie 0.1.1307 对这个路径返回 404。
  - **provider 是 `deepseek-official`**，不是 `deepseek`。这是 dsh 的 API key 路由名（`@deepseek-ai/dsh-llm-deepseek-api-key` 的 `PROVIDER`），它的配置行 id 是 `llm-deepseek`（`dsh-base/cordis.patch.yml`：`- id: llm-deepseek`，`name: '@deepseek-ai/dsh-llm-deepseek-api-key'`）。
  - **model 写全名**，即网关上的完整模型名，例如 Magpie 的 `opendesign/deepseek-v4.1-flash`，不要只写 `deepseek-v4.1-flash`。
  - 模型写在 `- id: acp` 行（见 `docs/deepseek.md` 4.1）：ACP profile 自带的 `acp` 行是 `provider: deepseek-official, model: deepseek-v4-flash`，优先于 `agent-default-model`，不覆盖就会用官方模型名去请求网关。

  经 Magpie（本机网关，只能在完全访问下用）：

  ```yaml
  # <私有 HOME>\.dsh\cordis.patch.yml
  - id: llm-deepseek
    config:
      baseURL: http://127.0.0.1:3425/v1
  - id: acp
    config:
      provider: deepseek-official
      model: opendesign/deepseek-v4.1-flash
  ```

  ```dotenv
  # <私有 HOME>\.dsh\.env
  MAGPIE_GATEWAY_KEY=<Magpie 的 key>
  DEEPSEEK_API_KEY=magpie
  ```

  `DEEPSEEK_API_KEY=magpie` 只是占位：`deepseek-official` 路由没有 `DEEPSEEK_API_KEY` 就直接报 `MISSING_CREDENTIAL`（“no API key for provider route "deepseek-official"”），不发请求，所以经 Magpie 时也必须有一个非空值。

  直连官方 api.deepseek.com（沙箱模式和完全访问都可以用）：

  ```yaml
  # <私有 HOME>\.dsh\cordis.patch.yml（可省略 llm-deepseek 行，dsh 默认就是这个地址）
  - id: llm-deepseek
    config:
      baseURL: https://api.deepseek.com/anthropic
  - id: acp
    config:
      provider: deepseek-official
      model: deepseek-v4-flash
  ```

  ```dotenv
  # <私有 HOME>\.dsh\.env
  DEEPSEEK_API_KEY=<DeepSeek 平台的 key>
  ```

  官方示例保留 `/anthropic`：dsh 源码里的默认值就是 `https://api.deepseek.com/anthropic`（`PUBLIC_BASE_URL`），按 `messagesApiRoot()` 请求的是 `https://api.deepseek.com/anthropic/v1/messages`，即 DeepSeek 官方的 Anthropic 兼容入口。`model` 用官方模型名（ACP profile 的默认值是 `deepseek-v4-flash`，此时 `acp` 行可省略）。
  - 私有 HOME **按“项目 UUID + 后端”各一份**：`homes\<项目 UUID>\deepseek`。换一个项目要在那个项目的私有 HOME 里再放一份 `.env` 和 `cordis.patch.yml`；Codex、pi 的配置互不共用。
  - **会话要先绑定项目才能发消息**（Windows 实测）：私有 HOME 按项目 UUID 定位，未绑定项目的外部会话发不出消息。先把会话绑定到项目（项目文件夹），再发第一句。
- `%ProgramData%\Fox\sandbox` 由“启用沙箱”创建。没启用过沙箱的机器，完全访问会以 `sandbox_runtime_not_installed` 拒绝启动（与沙箱模式相同），需先启用一次沙箱。

**Linux 上已核对的数量**（`--lib` 过滤结果）：

| 命令 | Linux | Windows 预期 |
|---|---|---|
| `cargo test -p fox-agent-backend --features acp --lib -- redaction` | 4 | 4 |
| `cargo test -p fox-agent-backend --features acp --lib -- full_access` | 6 | 5（`external_launch::tests::host_launcher_full_access_runs_unwrapped_with_the_private_home` 只在 Unix 编译） |
| `cargo test -p fox-agent-backend --features acp --lib -- loopback` | 9 | 9 |
| `cargo test -p fox-agent-backend --features acp --lib -- capability_view` | 4 | 4 |
| `cargo test -p fox-agent-backend --features acp --test external_runtime a_failed_prompt_or_exit_persists_the_redacted_stderr_tail` | 1 | 1（需要 PATH 里有 `node`） |
| `bun test tests/run-failure.test.ts`（在 `apps/desktop` 下） | 14 | 14 |

最小复测清单（Windows，本批 HEAD）：

1. 定向测试：跑上表 6 条命令，数量与“Windows 预期”一致且全部通过。重点：
   - 脱敏幂等：`acp::tests::redaction_is_idempotent_and_keeps_json_structure`（同一输入连续脱敏三次与一次相同；JSON 的 `{` `}` `"` 数量不变且仍能解析）。
   - 完全访问 env：`sandbox::tests::full_access_env_points_home_at_the_private_home_without_srt`（`HOME`，Windows 规划下 `USERPROFILE`、`APPDATA`、`LOCALAPPDATA` 等于私有 HOME 下的路径；程序与参数未被 srt 包装；没有 srt 设置文件）、`sandbox::tests::full_access_private_env_is_the_sandbox_env_minus_srt_only_variables`。
   - 拒绝文案：`loopback` 组中的两条 `external_launch` 用例和 `capability_view::tests::deepseek_cancel_is_request_only_and_observability_unverified`。
   - `external_runtime` 用例：诊断尾部含 `request headers {"authorization":"Bearer <redacted>","x-api-key":"<redacted>"}`，没有 `]]` 也没有 `[redacted`，长行与中文行完整，5 个 mock 密钥原文都搜不到。
2. 完全访问 + Magpie（Host 设 `FOX_EXTERNAL_FULL_ACCESS=1`、`FOX_EXTERNAL_BACKENDS=deepseek`；会话已绑定项目；该项目私有 HOME 的 `.dsh\cordis.patch.yml` 按上面“经 Magpie”示例写，baseURL 是 `http://127.0.0.1:3425/v1`（不是 `/anthropic`），`acp` 行 `provider: deepseek-official`、`model: opendesign/deepseek-v4.1-flash`；`.dsh\.env` 里有 `MAGPIE_GATEWAY_KEY` 和占位 `DEEPSEEK_API_KEY=magpie`）：
   - 发一句话，拿到模型回复，`run.completed`；能力 chip 显示“沙箱：无（完全访问）”，详情含“使用 Fox 私有 HOME（与沙箱模式相同的登录与配置）”。
   - dsh 会话日志落在私有 HOME 的 `.dsh\sessions\...` 下；真实 HOME（`%USERPROFILE%\.dsh`）里没有本次新增的会话文件。
   - 进程连的是 `127.0.0.1:3425`，不是 api.deepseek.com（例如用 `Get-NetTCPConnection -OwningProcess <dsh 的 node PID>` 或 Resource Monitor 看远端地址；Magpie 一侧也应看到请求）。
3. 仍在完全访问下，停掉 Magpie，再发一句话，制造一次 prompt 失败：
   - 本轮以 `agent_backend_prompt_failed`（或 `agent_backend_exited`）结束。
   - “查看错误详情”里没有残余括号（没有 `]]`、没有 `[redacted]`），JSON 片段的 `"}` 完整，占位符是 `<redacted>`。
   - 长行自动换行显示，详情块超过约 20rem 高时出现纵向滚动条，不出现横向滚动。
   - 在详情里搜 API key 和 Bearer 值的原文，搜不到。
   - **注（979313c 收尾批补）**：真实 dsh 遇到连接错误时只经 JSON-RPC 报错（`session/prompt` 返回 `-32603`，`message` 为 “Internal error: turn failed: DeepSeek Messages transport failed”，没有 `data`），**不写 stderr**。所以这一项在真实 dsh 上“查看错误详情”没有尾部、甚至没有详情块是正常的，不算失败。尾部的渲染（括号、`<redacted>`、长行换行与滚动、搜不到密钥）用 mock fixture 的副本（`tests/fixtures/mock-acp-session-agent.mjs` 的 `stderr-fail` / `stderr-crash` 场景）验证，见第 1 项的 `external_runtime` 用例与 `bun test tests/run-failure.test.ts`。详见 13.13。
4. 沙箱模式（去掉 `FOX_EXTERNAL_FULL_ACCESS`）下同一配置：应立即以 `agent_backend_loopback_requires_full_access` 失败，提示含“需要完全访问权限（当前构建由 Host 配置 FOX_EXTERNAL_FULL_ACCESS 开启），或改用官方 api.deepseek.com”，不再出现“请切换到“完全访问””；能力 chip 的限制说明也是新文案。
5. 不需要跑全量，也不需要重跑 13.9 – 13.11 的其余条目。

Linux 上的对应实测（供参考）：`tests/sandbox_srt.rs` 新增忽略用例 `real_external_runtime_dsh_full_access_uses_the_private_home`（`FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 FOX_AGENT_RUNTIME_ROOT=<含 backends/dsh 的运行时> -- --include-ignored`），真实 dsh 0.2.1-rc.2、完全访问、dsh 配置只写在私有 HOME 并指向本机假网关：网关收到请求，dsh 的 `sessions`、`profiles` 写在私有 HOME 的 `.dsh` 下，真实 HOME 的 `.dsh`、`.config`、`.cache`、`.local/*` 没有新增文件。假网关不讲 Anthropic 协议，所以该轮以 `agent_backend_prompt_failed`（SSE event type mismatch）结束，这是预期的。

### 13.13 979313c 收尾批（2026-10-21）

只改文档，没有代码改动。**不新增 Windows 复测项**，本节内容随下一大批一起验证。

- **13.12 的配置示例按 Windows 实测更正**：经 Magpie 时 baseURL 写 `http://127.0.0.1:3425/v1`（不是 `/anthropic`）；官方 api.deepseek.com 单独一份示例，保留 `https://api.deepseek.com/anthropic`；补了 provider 是 `deepseek-official`、model 写全名（如 `opendesign/deepseek-v4.1-flash`）、`.env` 里的占位 `DEEPSEEK_API_KEY=magpie`、私有 HOME 按“项目 UUID + 后端”各一份、会话要先绑定项目。依据是已安装 dsh 0.2.1-rc.2 的源码：
  - `@deepseek-ai/dsh-llm-deepseek/lib/index.js`：`messagesApiRoot(baseURL)` 去掉末尾 `/` 后，路径以 `/v1` 结尾就原样用，否则补 `/v1`；请求发到 `${messagesApiRoot(baseURL)}/messages`。默认值 `PUBLIC_BASE_URL = "https://api.deepseek.com/anthropic"`。
  - `@deepseek-ai/dsh-llm-deepseek-api-key/lib/index.js`：`PROVIDER = "deepseek-official"`，`apiKeyEnv` 默认 `DEEPSEEK_API_KEY`，取不到就抛 `MISSING_CREDENTIAL`。
  - `@deepseek-ai/dsh-base/cordis.patch.yml`：`- id: llm-deepseek` 行加载的就是 `dsh-llm-deepseek-api-key`；`@deepseek-ai/dsh-acp-app/cordis.patch.yml` 的 `acp` 行默认 `provider: deepseek-official`、`model: deepseek-v4-flash`。
- **13.12 第 3 项补注**：真实 dsh 的连接错误不写 stderr，尾部为空是正常的；尾部渲染用 fixture 副本验证。
- **JSON-RPC `error.data` 调查（Linux 实测，结论：没有可用的 data，不改代码）**
  - 做法：Linux（aarch64）上直接起真实 dsh 0.2.1-rc.2（`dsh --profile acp --patch <runtime>/dsh-patches/workspace-write-noweb.yml`），不套 srt，环境变量按完全访问的私有 HOME 设（`HOME`、`DSH_HOME`、`XDG_*`、`TMPDIR`）；私有 HOME 的 `.dsh/cordis.patch.yml` 把 `llm-deepseek` 的 baseURL 指向一个没人监听的本机端口（`http://127.0.0.1:<空端口>/v1`），`.env` 里放假 key。用一个原始 ACP 探针逐行记录 stdin/stdout 的 JSON-RPC。
  - 结果：冷启动约 145 s 后 `initialize`、`session/new` 正常；`session/prompt` 约 15 s 后失败（dsh 自己按 `TRANSPORT` 重试 5 次，退避 0.5–7.4 s），stderr **0 字节**。错误对象原文（不含秘密，无需脱敏）：

    ```json
    {"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"Internal error: turn failed: DeepSeek Messages transport failed"}}
    ```

    没有 `data` 字段，`message` 里也没有目标 URL、`ECONNREFUSED` 或 cause。dsh 自己的会话日志（私有 HOME 的 `.dsh/sessions/.../session.v4.jsonl.zstd`）里每次尝试也只有 `{"failure":{"message":"DeepSeek Messages transport failed","code":"TRANSPORT"}}`，同样没有底层原因。
  - 因此“stderr 为空时用 `error.data` 补充诊断”对这个场景没有内容可补，本批不加代码。Fox 现在把 ACP 错误按 `agent_client_protocol::Error` 的 `Display` 写进 `run.failed.message`（`message`，有 `data` 时追加 `: <data 的 pretty JSON>`），所以 dsh 这条错误在界面上就是 “prompt failed: Internal error: turn failed: DeepSeek Messages transport failed”。
  - 附带发现（未改，留作待办）：`Display` 会把 `data` 原样拼进 `run.failed.message`，这条路径**不经脱敏**。dsh 目前不带 `data`，其他后端（如 Codex、pi）若在 `data` 里放请求头或 key，会原样进入 message。以后要在这里接 `redact_diagnostic_tail`，或把 `data` 拆出来作为 `source: "rpc_error_data"` 的诊断。
