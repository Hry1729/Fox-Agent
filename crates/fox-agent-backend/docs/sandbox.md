# 外部 Agent 后端的权限模式与 srt 沙箱层

- 代码：`src/sandbox.rs`（模式、策略、预检、启动包装、能力声明），`src/acp.rs` 的 `AcpConnection::spawn_prepared`。
- 实测：`tests/sandbox_srt.rs`（`FOX_TEST_REAL_SRT=1` 才运行；否则打印 SKIPPED 并返回，跳过不算通过）。
- 决策来源：用户 2026-10-19 拍板；方案文档 `docs/02-架构/四Agent改造实施方案-v0.3.1.md` 末尾“v0.3.2 增补”。
- srt：`@anthropic-ai/sandbox-runtime`（Apache-2.0），本文按 0.0.80 及其 README 编写。

## 1 两种模式

| 模式 | 何时 | 隔离 | 不变的 Host 底线 |
|---|---|---|---|
| `sandboxed`（默认） | 除明确授予 full_access 外的一切情况；旧会话（未记录模式）一律按此处理；未知取值也按此处理 | 整个外部后端进程（codex-acp / pi-acp / dsh）在 srt 里运行，一个会话一个沙箱：只读写授权目录 + 私有 HOME + 运行所需系统目录；出站网络只放行该后端模型 API 域名白名单 | 环境变量白名单 / 凭据不下发；取消与进程树清理；记录；原生审批如实转发 |
| `full_access` | 用户按会话或项目明确授予（`HostPolicy.full_access_grants` 中的 `conversation:<id>` / `project:<root>`），且企业策略未禁止（`full_access_forbidden`）、`locked_keys.permission_mode` 未锁成 `sandboxed` | 不套沙箱，不承诺目录边界；能力声明中沙箱两维固定为 `absent`，即使后端原生有沙箱也不替它宣称 | 同上 |

判定入口：`effective_permission_mode(policy, snapshot, conversation_id)`。项目配置、用户设置都无法单独产生 full_access：只有“会话快照请求 full_access”＋“Host 持有的有效授予”同时成立才行。

收紧立即生效（沿用 6.5）：`recheck_permission_mode(...)` 与 `config::recheck_before_continue` 一起在每次继续 / 恢复前调用。授予被撤销而进程仍在沙箱外运行 → `RestartSandboxed`：Host 先停进程，再按 sandboxed 重新预检和启动（失败即拒绝）。放宽（运行中的沙箱会话新获授予）不静默带入，需 Host 有意重启。

## 2 fail-closed 规则

sandboxed 模式下，以下任何一项失败都**拒绝启动该后端**并返回结构化错误 `SandboxError { code, message, missing, remedy }`（serde 输出 snake_case 的 `code`；`PortError::Sandbox` 携带它）。**绝不自动退回 full_access。**

| code | 触发 |
|---|---|
| `sandbox_dependency_missing` | srt 包不在 Fox 管理目录、srt 版本 < 0.0.80、Node 缺失或 < 22.12、Linux 缺 bwrap / socat / rg、macOS 缺 rg |
| `sandbox_not_installed` | Windows 未执行 `windows-install`（查 `HKLM\SOFTWARE\sandbox-runtime`） |
| `sandbox_unavailable` | 不支持的 OS；Linux 上 bwrap 无法建立 user/net namespace（预检实跑 `bwrap --ro-bind / / --unshare-user --unshare-net -- /bin/true`）；macOS 无 `/usr/bin/sandbox-exec`；准备会话目录失败 |
| `sandbox_policy_invalid` | 无授权目录；无 denyRead 根；路径非绝对；授权目录 / 私有 HOME / 额外只读路径等于或包含某个 denyRead 根（会把它重新打开）；域名格式错误；会话 ID 非 `[A-Za-z0-9_-]{1,128}`；settings 目录落在沙箱可写范围内 |
| `sandbox_backend_not_installed` | 后端经 `npx` 启动（需要 npm 源与用户的 npx 缓存）；程序不是绝对路径；程序在沙箱内不可读 |

调用顺序：`preflight(install, probe)` → `SrtLauncher::new(ready, state_root)` → `prepare_launch(mode, spec, session_id, SandboxSetup::{Ready|Unavailable}, os)` → `AcpConnection::spawn_prepared(prepared, cwd)`。预检失败时把错误放进 `SandboxSetup::Unavailable(err)`，`prepare_launch` 在 sandboxed 下原样返回、在 full_access 下忽略。

## 3 srt 配置模板

srt 的读权限默认全放行（deny-then-allow，`allowRead` 优先于 `denyRead`），写默认全拒，网络默认全拒。所以每会话 settings 必须先 denyRead 用户主目录根，再 allowRead 需要的路径。`SandboxPolicy::to_srt_settings()` 生成（路径一律用 `{"path": ..., "literal": true}`，不按 glob 解释，`[WIP] 项目` 这样的目录名也只表示它本身）：

```json
{
  "network": {
    "allowedDomains": ["<该后端模型 API 域名>"],
    "deniedDomains": [],
    "allowLocalBinding": false
  },
  "filesystem": {
    "denyRead":  ["/home", "/root", "/tmp", "<Host 追加的数据卷>"],
    "allowRead": ["<授权目录...>", "<私有 HOME>", "<后端安装目录>", "<Node 运行时>", "<srt 包目录>"],
    "allowWrite": ["<可写授权目录...>", "<私有 HOME>"],
    "denyWrite": []
  },
  "enableWeakerNestedSandbox": false,
  "allowNestedUserNamespaces": false
}
```

- denyRead 默认根：Linux `/home` `/root` `/tmp`；macOS `/Users` `/Volumes`；Windows `C:\Users`。Host 应追加真实用户目录不在这些根下时的位置（其他盘符、网络 home）及其他数据卷。
- srt 包目录与 Node 运行时若落在 denyRead 根下（例如 nvm 装在 `~/.nvm`），`SrtLauncher::effective_policy` 自动只读放行它们本身（Linux 上 srt 要在沙箱内执行 `vendor/seccomp/<arch>/apply-seccomp`，实测不放行会直接失败）。
- settings 文件写在 `state_root/sandbox-sessions/<session_id>/srt-settings.json`（Unix 0600），`state_root` 必须在沙箱不可写处；`SandboxSession` 随连接释放（`close()` 或 drop）时删除整个会话目录。
- 启动命令：`<node> <srt>/dist/cli.js --settings <file> -- <后端程序绝对路径> <参数...>`。
- 环境变量（在 launch spec 自身的 session_env 之后覆盖）：`HOME`、`XDG_CONFIG_HOME/CACHE_HOME/DATA_HOME/STATE_HOME`、`TMPDIR/TMP/TEMP`（私有 HOME 下 `tmp/`）、Windows 另设 `USERPROFILE/APPDATA/LOCALAPPDATA`；codex 设 `CODEX_HOME=<私有 HOME>/.codex`，deepseek 设 `DSH_HOME=<私有 HOME>/.dsh`；`PATH` = 后端程序目录 + Node 目录 + 系统目录，不继承用户 PATH。环境白名单（`launch::child_environment`）不变，仍先于这些覆盖生效。

### 私有 HOME

`private_home_for(fox_data, backend, scope, key)` → `<fox_data>/backends/<backend>/home[/projects|sessions/<key>]`。`PerBackend` 保留后端自己的登录、跨会话可用，但同后端的不同会话能互读原生历史；`PerSession` 完全隔离但每会话需重新登录；`PerProject` 折中。**默认取哪种需产品定**（见第 7 节）。Host 不复制用户真实 home 中的原生凭据（6.5）：沙箱模式下用户需在私有 HOME 内各自登录（codex `codex login` 写入私有 `CODEX_HOME`），或使用白名单内的凭据环境变量。

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

- full_access：`sandbox_filesystem = sandbox_network = absent`，`directory_isolation = false`，不列任何根。UI 不得出现“限定在这些文件夹”。
- sandboxed：取后端原生实测与 srt 两者较强者（排序 enforced > unverified > advisory > absent），既不隐藏原生沙箱，也不夸大 srt。
- srt 强制程度按平台实测：Linux = `enforced`（第 6 节）；macOS / Windows = `unverified`，直到工程师在该平台跑完同样的 T04。`directory_isolation` 只有 `enforced` 时为真。

## 6 Linux 实测（T04，2026-10-19）

环境：Debian 13 容器，Linux 6.12.110 aarch64，非 root（uid 1000），Node 22.23.3，bubblewrap 0.12.0，socat 1.8.0.3，ripgrep（系统包），srt 0.0.80（`npm i --prefix <dir>`），pi-acp 0.0.34 + pi 0.84.2 装在独立前缀（模拟 Fox 管理目录）。模型为测试专用 `fixtures/sandbox/scripted-cmd-provider.ts`（pi 进程内的 OpenAI 兼容脚本模型，按提示发出 bash / read 工具调用）；每个工具调用都经 Fox 审批扩展并被允许，因此拦住它的只有沙箱。denyRead = `/home /root /tmp /workspace`，授权目录与秘密文件都在真实 HOME（`/home/sandbox`）下。

```
FOX_TEST_REAL_SRT=1 FOX_SRT_PREFIX=<srt 前缀> FOX_PI_ACP_PREFIX=<pi 前缀> FOX_T04_EXTRA_DENY=/workspace \
  cargo test -p fox-agent-backend --features acp --test sandbox_srt -- --nocapture
```

| 检查 | 结果（沙箱内工具输出） |
|---|---|
| 预检 | 通过：node v22.23.3、srt 0.0.80、bwrap/socat/rg 均找到，namespace 探测成功 |
| 授权目录读写 | `cat readme.txt` 得到内容；`inside.txt` 写入成功，主机可见 |
| 读授权目录外（bash） | `cat .../secret/secret.txt: No such file or directory`（denyRead 目录被 tmpfs 覆盖），秘密未出现在任何事件中 |
| 读授权目录外（pi read 工具） | `ENOENT: no such file or directory` |
| 读本会话 srt settings | `No such file or directory` |
| 写授权目录外 | 被拒读目录：`No such file or directory`；`/tmp` 下目录：`No such file or directory`（/tmp 也被拒读时）或 `Read-only file system`（/tmp 未拒读时）；Fox state 目录：`No such file or directory`；主机上均无文件 |
| 白名单外域名 | `curl https://example.org` → `CONNECT tunnel failed, response 403` |
| 白名单内域名 | `curl https://example.com`（模型 API 替身）→ `code=200` |
| 绕过代理直连 IP | `curl --noproxy '*' http://1.1.1.1` → `Failed to connect`（无网络命名空间出口） |
| 私有 HOME | `HOME=<私有 HOME>`，可写 |
| 关闭 | `close()` 后无残留进程（按测试目录匹配 ps），会话 settings 目录已删除 |
| 缺 srt（真实路径不存在） | 预检 `sandbox_dependency_missing`；sandboxed 拒绝，full_access 照常 |

附带发现：`fixtures/pi/pi-gated.sh` 在仓库里是 100644（contents API 无法设可执行位），沙箱内 pi-acp 报 `EACCES`；测试改为运行一份私有可执行副本。生产中 Host 需保证该脚本可执行（或改为 `sh <脚本>` 调用）。

## 7 各平台注意事项与已知限制

**Linux**
- 依赖：bubblewrap（≥0.5 更佳）、socat、ripgrep；Ubuntu 24.04+ 需关闭 `kernel.apparmor_restrict_unprivileged_userns` 或加 AppArmor 配置，否则预检报 `sandbox_unavailable`。
- root 运行需要 `CAP_SETFCAP`；建议非 root。
- 沙箱内不能再建 user namespace（嵌套 bwrap、Chromium 沙箱、rootless podman 会 EPERM），这是 srt 的安全设计，Fox 不开启 `allowNestedUserNamespaces`。
- srt 包不得装在任何可写授权目录下（README 安全提示），Fox 管理目录满足此条件。

**macOS**（未实测）
- 依赖 `/usr/bin/sandbox-exec` 与 ripgrep。
- 默认 denyRead `/Users`、`/Volumes`；`/private/tmp` 与 `/private/var/folders`（`$TMPDIR`）是否一并拒读需实测后决定。
- Go 程序经代理验证 TLS 可能需要 `enableWeakerNetworkIsolation`，Fox 不开启；若 codex 二进制受影响需评审。

**Windows**（alpha，未实测）
- 首次选沙箱模式时由安装器调用 `windows_install_command()`（`node <srt cli.js> windows-install`，一次 UAC，幂等）；Fox 卸载时必须调用 `windows_uninstall_command()`（`windows-uninstall`，一次 UAC）。本轮只提供检测（`windows_install_state`）、命令与错误码，未集成安装器。
- 沙箱进程以 `srt-sandbox` 本地账户运行，读不到用户目录中的工具（npx 缓存、nvm、`%LOCALAPPDATA%\Programs`）：后端与 Node 必须装在 Fox 管理目录并 allowRead。
- 只支持会话级 allowRead/allowWrite（一会话一沙箱正好满足）。
- srt-win 给子进程的是全新环境，只叠加 broker 的 PATH 与代理变量：Fox 设置的 HOME/USERPROFILE/CODEX_HOME 及凭据环境变量**很可能不会传入**，子进程用的是 `srt-sandbox` 账户自己的 profile。需实测并决定凭据如何提供。
- 代理令牌（`HTTP_PROXY=http://srt:<token>@127.0.0.1:...`）在 runner 进程命令行中可见，同机其他本地主体可读；单用户开发机通常可接受，共享主机上应视为白名单对同会话其他主体可达。
- schannel 吊销检查被 WFP 拦截（`CRYPT_E_REVOCATION_OFFLINE`），系统 DNS 解析不受围栏限制。
- 进程树清理：Fox 的 Job Object 包住的是 node/srt broker；sandbox 账户下的 runner 与子进程是否随之终止需实测。

**通用**
- 读默认放行：配置必须先 denyRead 用户目录根，`SandboxPolicy::validate` 强制要求至少一个 denyRead 根，且不允许 allowRead 一个 denyRead 根或其祖先。denyRead 根之外的路径（系统目录、其他未列出的数据卷）仍可读。
- 网络按域名过滤，不检查流量内容；白名单内域名（如 `chatgpt.com`）本身仍可能被用来外发数据。
- Linux 上 Unix socket 由 seccomp 阻止创建；回环在沙箱自身的网络命名空间内可用（实测 pi 能连本进程内的 127.0.0.1 脚本模型），但到不了主机的回环服务。
- 一次审批只批准发起调用；命令内部行为由沙箱范围约束（第 2 节原则不变）。

## 8 待办与待验证

- [ ] macOS：用同一测试（`tests/sandbox_srt.rs` 的 Linux 门控需放开或复制一份）跑 T04，通过后把 `srt_enforcement(MacOs)` 改为 `Enforced`。
- [ ] Windows：执行 windows-install 后跑 T04；验证环境变量 / 凭据是否传入、私有 HOME 是否生效、Job Object 能否清理 runner 子树、pi-gated 需要 `.cmd` 版本；通过后改 `srt_enforcement(Windows)`。
- [ ] codex / deepseek 在沙箱内的真实 turn：域名白名单是否够用（codex ChatGPT 登录、令牌刷新；dsh 的遥测或插件下载）。
- [ ] 私有 HOME 默认粒度（PerBackend / PerProject / PerSession）产品决定。
- [ ] Fox 管理目录内的后端安装（替代 `npx -y`），以及 `launch_spec` 在 sandboxed 下指向已安装的程序。
- [ ] 运行中网络收紧经 `--control-fd` 即时下发（可选）。
- [ ] Desktop Host 接线：在会话创建 / 继续路径调用 `effective_permission_mode`、`recheck_permission_mode`、`prepare_launch`、`spawn_prepared`，并在 UI 展示 `SandboxDeclaration` 与 `SandboxError.remedy`。
