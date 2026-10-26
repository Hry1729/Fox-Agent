# 沙箱网络审批与项目预览

本次实现延续 srt，运行包版本为 0.1.4。此版本增加 npm/npx；srt 仍为 0.0.80，Node 仍为 24.15.0。

## 行为

- 未授权的网络目标进入 Fox 审批卡，目标精确到主机和端口。待审批期间暂停响应超时计时。
- 同一目标同时到达的请求共用一张待审批卡。“允许一次”处理该卡正在等待的连接；不会保存后续授权。
- “本次对话内允许”写入现有会话权限表。用户可以在 Agent 栏撤销，也可以明确保存为当前项目的规则。
- 删除项目规则影响后续连接，不会停止其他会话。撤销会话授权会停止当前会话的 Agent，以关闭既有连接。
- Host 用 Windows Job Object 的进程列表核验监听端口。仅当前会话进程拥有的回环端口可以自动通过代理访问。
- Agent 栏可打开已核验的预览。停止操作明确命名为“停止预览和当前 Agent”，并核验进程树结束；不尝试跨账户单独终止未知进程。
- 关闭、取消、重启和旧回调均拒绝遗留的网络请求。审批通道使用独立的 Host 私有管道，控制凭据不进入 Agent 环境。

## 验证口径

本地测试覆盖运行时审批、并发合并、取消、旧连接、超时、项目规则和前端动作。Windows 实测使用生产的 SrtLauncher、NetworkBroker、ACP 和 ExternalRuntime，加一个可控 ACP 测试进程，不调用真实大模型。

真实 Windows 流程覆盖：启动网页服务、Host 核验进程归属、代理 HTTP、WebSocket、拒绝直接回环连接、拒绝其他本机端口、显式审批放行、npm 从 registry.npmjs.org 安装 is-number 7.0.0，以及停止后的端口清理。浏览器另行验证网页按钮、WebSocket 消息和 Agent 网络面板的窄屏布局。

复现入口：

```powershell
$env:FOX_TEST_REAL_SRT='1'
$env:FOX_TEST_NPM_NETWORK='1'
$env:FOX_TEST_NPM_SOURCE='<已暂存运行包>/node/node_modules/npm'
cargo test -p fox-agent-backend --features acp --offline --test network_preview_srt -- --include-ignored --nocapture
```

测试在独立项目中复制 npm，以免改动当前安装的只读运行包。`filesystemObservation` 是测量记录，不是隔离通过断言。默认测试不会运行该真实测试。

## 已知限制

1. Windows srt 的专用账户仍受磁盘原有 ACL 影响。此次 D 盘 canary 实测可读写项目外、对普通用户开放的文件。`directory_isolation=false` 仍然成立。本次网络功能不修复此文件边界，也不能据此宣称“只能访问当前项目”。Fox 没有修改共享磁盘根目录的权限。
2. 预览核验目前实现 Windows IPv4。停止预览会同时停止当前 Agent。项目规则删除不能断开其他会话已经建立的连接。
3. 代理授权约束使用代理的连接。客户端需要支持 HTTP/SOCKS 代理；任意客户端的网络行为不能仅靠环境变量推定成功。
4. 未完成真实模型生成网页的完整业务验收，也未替换或验收用户正在运行的 Fox 安装。

## MXC 评估结论（2026-10-26）

在本机 Windows 26H2 build 26300.9711 上，官方 `@microsoft/mxc-sdk` 1.0.1 的探测确认 BaseContainer、拒绝路径、Host 回环和无身份回环代理能力可用。`cmd.exe` 的固定输出命令实际执行成功。

Node 24.15.0 不满足 SDK 的原生流接口要求。另行下载并按 Node 官方 SHA256 校验 24.22.1 后，Node 工作负载在默认、无界面和增加 Windows 系统目录只读权限三个配置下均以 `0xC0000142` 退出。SDK 未返回 stdout/stderr；该退出码表示 DLL 初始化失败，具体 DLL 原因尚未确认。

因此，本轮没有采用 MXC。文件边界、网页、代理和生命周期的同类验证未能越过 Node 启动阶段，不能把能力探测当作这些场景已经通过。评估仅使用独立目录和测试进程，没有切换系统功能、安装驱动或改动 Fox 的生产依赖。

官方依据：

- [MXC SDK 与运行方式](https://github.com/microsoft/mxc/blob/main/README.md)
- [ProcessContainer 网络策略及已知入站限制](https://github.com/microsoft/mxc/blob/main/docs/backends/process-container/networking.md)
- [Node 官方 24.22.1 校验清单](https://nodejs.org/dist/v24.22.1/SHASUMS256.txt)
