# 沙箱网络审批与项目预览

本次实现延续 srt，运行包版本为 0.1.5，包含 srt 0.0.81、Node 24.15.0 和 npm/npx 11.12.1。0.1.4 引入网络审批和预览；0.1.5 接入上游 Windows 写入受限令牌，并增加安装及启动检查。

## 行为

- 未授权的网络目标进入 Fox 审批卡，目标精确到主机和端口。待审批期间暂停响应超时计时。
- 同一目标同时到达的请求共用一张待审批卡。“允许一次”处理该卡正在等待的连接；不会保存后续授权。
- “本次对话内允许”写入现有会话权限表。用户可以在 Agent 栏撤销，也可以明确保存为当前项目的规则。
- 删除项目规则影响后续连接，不会停止其他会话。撤销会话授权会停止当前会话的 Agent，以关闭既有连接。
- Host 用 Windows Job Object 的进程列表核验监听端口。仅当前会话进程拥有的回环端口可以自动通过代理访问。
- Agent 栏可打开已核验的预览。停止操作明确命名为“停止预览和当前 Agent”，并核验进程树结束；不尝试跨账户单独终止未知进程。
- 关闭、取消、重启和旧回调均拒绝遗留的网络请求。审批通道使用独立的 Host 私有管道，控制凭据不进入 Agent 环境。

## Windows 文件权限修补

srt 0.0.81 使用写入受限令牌和每个授权目录的能力 SID。实测中，项目外文件虽然允许 Users 组写入，Agent 仍无法修改；项目内写入成功。第二个项目也无法写入上一次授权的项目，中文和空格路径正常。

Fox 在启动前核验 helper 的 setup 版本、安装标记、能力项、账户 SID、授权组、凭据和 Secondary Logon 状态。旧版组件、配套不完整或已有用户主目录读取授权时，启动会被拒绝。安装命令显式传入 `--skip-profile-grant`，不会采用上游默认的主目录读取授权。

这些改动没有建立完整读取边界。`directory_isolation=false` 和 `UsersGroupReadable` 保持不变。要求完整目录隔离的 Host 可在启动 Fox 前设置以下进程环境变量：

```powershell
$env:FOX_REQUIRE_DIRECTORY_ISOLATION='1'
```

当前 Windows srt 后端会返回 `sandbox_directory_isolation_unavailable`，不启动 Agent，也不自动转为完全访问。此开关只约束沙箱模式；若还需禁止用户另行授予完全访问，可同时设置 `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1`。默认值保持有限隔离模式。

## 验证口径

本地测试覆盖运行时审批、并发合并、取消、旧连接、超时、项目规则和前端动作。Windows 实测使用生产的 SrtLauncher、NetworkBroker、ACP 和 ExternalRuntime，加一个可控 ACP 测试进程，不调用真实大模型。

真实 Windows 流程覆盖：启动网页服务、Host 核验进程归属、代理 HTTP、WebSocket、拒绝直接回环连接、拒绝其他本机端口、显式审批放行、npm 从 registry.npmjs.org 安装 is-number 7.0.0，以及停止后的端口清理。浏览器另行验证网页按钮、WebSocket 消息和 Agent 网络面板的窄屏布局。

复现入口：

```powershell
$env:FOX_TEST_REAL_SRT='1'
$env:FOX_TEST_NPM_NETWORK='1'
$env:FOX_TEST_NPM_SOURCE='<已暂存运行包>/node/node_modules/npm'
$env:FOX_AGENT_RUNTIME_ROOT='C:/Program Files/Fox/sandbox-runtime/0.1.5'
cargo test -p fox-agent-backend --features acp --offline --test network_preview_srt -- --include-ignored --nocapture
```

测试在独立项目中复制 npm，以免改动当前安装的只读运行包。0.1.5 的测试断言项目外写入被拒绝、项目内写入成功，并验证外部文件内容不变；读取结果仍单独记录。`FOX_TEST_OUTPUT_ROOT` 可指定测试输出目录。`FOX_TEST_PREVIOUS_PROJECT` 可指向上一次测试项目，以验证其持久授权不会让新项目获得写入权限。默认测试不会运行该真实测试。

srt 0.0.81 会保留目录授权记录。测试结束后应先确认测试进程已停止，再从 helper 的 `user status` 取得本次项目、私有 HOME 和临时目录的完整规范路径，用 `acl forget <精确路径...>` 清理。已删除的临时目录也要按状态记录中的原路径清理。不要清理其他会话的授权。此次实测的六个授权根和残留记录已清理；旧版系统组件已恢复，并用旧运行时在沙箱中执行 `node --version` 验证可启动。当前 Fox 桌面应用未替换。

## 已知限制

1. 0.0.80 的旧实测允许读写项目外文件。0.0.81 已阻止本次复现的 Users 组越界写入，但项目外读取仍成功。不能据此宣称“只能访问当前项目”，也不能推定所有特殊 ACL、已有授权或系统可写位置都受到完整限制。Fox 没有修改共享磁盘根目录的权限。
2. 预览核验目前实现 Windows IPv4。停止预览会同时停止当前 Agent。项目规则删除不能断开其他会话已经建立的连接。
3. 代理授权约束使用代理的连接。客户端需要支持 HTTP/SOCKS 代理；任意客户端的网络行为不能仅靠环境变量推定成功。
4. 未完成真实模型生成网页的完整业务验收，也未替换或验收用户正在运行的 Fox 安装。

## MXC 评估结论（2026-10-26）

在本机 Windows 26H2 build 26300.9711 上，官方 `@microsoft/mxc-sdk` 1.0.1 的探测确认 BaseContainer、拒绝路径、Host 回环和无身份回环代理能力可用。`cmd.exe` 的固定输出命令实际执行成功。

进一步使用按 Node 官方 SHA256 校验的 Node 24.22.1 定位了启动条件：默认关闭 Win32k 的配置会触发 `0xC0000142`；设置 `ui.disable=false` 后可以启动。随后按 SDK 1.0.1 的 `containment.config` 配置独立桌面，并仅给运行路径的祖先目录枚举权限，项目内写入和项目外读写拒绝均通过。早期探测使用过无效的顶层 `processContainer` 字段，因此独立桌面结论以修正配置后的测试为准。

MXC 的网络与预览组合仍不满足本任务要求：不开原生代理时，Host 预览可访问，但 Agent 能直接访问其他本机端口；启用精确代理端点后，直接连接被阻止、代理可用，但 Host 访问 Agent 预览超时。补充的精确 WFP 出站规则也未消除前一种配置的直接回环访问。因此本轮保留 srt，未将 MXC 放入产品依赖。上述结论只覆盖本机版本及测试配置，不代表其他 Windows 版本的结果。

官方依据：

- [MXC SDK 与运行方式](https://github.com/microsoft/mxc/blob/main/README.md)
- [ProcessContainer 网络策略及已知入站限制](https://github.com/microsoft/mxc/blob/main/docs/backends/process-container/networking.md)
- [Node 官方 24.22.1 校验清单](https://nodejs.org/dist/v24.22.1/SHASUMS256.txt)
