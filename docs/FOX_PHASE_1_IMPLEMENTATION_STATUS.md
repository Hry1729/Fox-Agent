# Fox 第一阶段实施状态

更新时间：2026-07-15

## 已实现

- Fox 原生 Agent 与 Yuxi 解耦；不连接 Yuxi 也可通过独立模型服务运行。
- Yuxi 仅保存一个服务配置，由 SQLite `singleton_id = 1` 强制保证。
- Yuxi 地址支持本机 HTTP、私有局域网 HTTP、远程 HTTPS；远程 HTTP 会被拒绝。
- Yuxi Token 按规范化 Base URL 保存到 Windows 凭证库，不写入 SQLite。
- Yuxi 连接测试先检查 `/api/system/health`；提供或已保存 Token 时，再通过 `/api/agent` 验证 Bearer 凭证。
- 测试未保存的新地址时可直接使用表单中尚未落盘的 Token；更换单例服务地址后会清理旧地址的凭证。
- 模型服务为单例 OpenAI-compatible 配置，支持本机、局域网和远程 HTTPS API。
- 模型 API Key 保存到 Windows 凭证库。
- `fox-runtime-jsonl v1` 已覆盖握手、Session 创建/恢复、Prompt、取消、事件和 Host Request。
- `fox-pi-runtime` 已实现，依赖固定为 `@earendil-works/pi-agent-core` 与 `@earendil-works/pi-ai` `0.79.9`。
- Fake Runtime 仅保留为显式开发/测试入口：`FOX_RUNTIME_MODE=fake`。
- Pi 事件会转换为 Fox 稳定事件，不向 UI 泄漏 Pi 原始类型。
- SQLite 是会话与消息事实来源；Runtime Session 只承担恢复职责。
- `reasoning.*` 与 `tool.*` 事件随会话从 SQLite 恢复，并使用 AI Elements 的 `ChainOfThought` 与 `Tool` 组件实时和历史渲染。
- `read`、`grep`、`find`、`ls` 由 Fox 自己定义，不引入 `pi-coding-agent`。
- 所有只读工具执行前必须等待 Rust `tool.preflight`，且路径必须位于会话授权项目目录内。
- 对话输入区可以授权一个现有本机文件夹并创建绑定该目录的新会话；Rust 会规范化路径并拒绝不存在或非目录路径。
- 取消只允许活动 Run 进入 `cancelling`；Runtime 未接受取消时会投影为 `interrupted`，避免会话永久卡住。
- 自动审批、写入、编辑和 Bash 明确不属于第一阶段。
- 生产分发入口采用 Bun compile 独立 Sidecar；`pnpm tauri:build` 会先运行 `pnpm runtime:build`。
- Runtime 构建采用临时文件成功后原子替换正式 EXE，构建失败不会破坏上一次可用产物。
- `pnpm runtime:smoke` 会直接验证编译后的 Sidecar EXE，而不是源码入口。

## 当前验证

- Rust：21 项测试通过，其中包含 Yuxi `/api/agent` Bearer 鉴权、思考/工具事件历史恢复及取消状态约束测试。
- Runtime 协议、Fake Sidecar、真实 Pi Faux Provider Agent Loop、事件映射、Session、工具执行器和兼容模型规划文本分流：30 项测试通过。
- 前端 TypeScript 检查通过。
- Vite 生产构建通过。
- Rust `clippy --all-targets -- -D warnings` 与 Release 编译通过。
- `git diff --check` 通过。
- Bun `1.3.14` 已用于生成独立 Windows x64 Runtime EXE。
- 独立 Runtime EXE 冒烟测试通过，覆盖握手、Session 创建、Pi Faux 流式回复、Run 完成和正常关闭。
- Tauri Windows Release 构建通过，并生成 MSI 与 NSIS 两种安装包。
- 本机 Yuxi `http://127.0.0.1:5050` 健康检查已返回版本 `0.7.0`。
- 本机 Yuxi `/api/agent` 未携带凭证时返回 `401` 和 `WWW-Authenticate: Bearer`，与 Fox 的鉴权方式一致。

## 剩余验收

第一阶段人工发布验收已于 2026-07-16 完成：

1. 已使用真实 OpenAI-compatible 模型完成多轮流式对话、取消和只读工具端到端测试。
2. 已安装发布包并验证首次启动、重启恢复和卸载流程。
3. 已验证包含空格或中文的授权项目目录下 Sidecar、SQLite 和只读工具行为。

第一阶段验收通过，可以进入第二阶段。后续针对消息即时滚动和兼容模型规划文本泄漏的修复属于第一阶段稳定性维护，不改变阶段范围。

Vite 的大于 500 kB Chunk 提示属于性能优化项，不影响当前桌面程序或安装包运行，后续可通过页面和 AI Elements 动态导入降低首屏体积。

## Windows 构建产物

- Runtime：`services/agent-runtime/dist/fox-agent-runtime-x86_64-pc-windows-msvc.exe`
- 桌面程序：`apps/desktop/src-tauri/target/release/fox-desktop.exe`
- MSI：`apps/desktop/src-tauri/target/release/bundle/msi/Fox_0.1.0_x64_en-US.msi`
- NSIS：`apps/desktop/src-tauri/target/release/bundle/nsis/Fox_0.1.0_x64-setup.exe`

## 依赖恢复后执行

```powershell
pnpm.cmd install
pnpm.cmd runtime:test
pnpm.cmd runtime:build
pnpm.cmd runtime:smoke
pnpm.cmd --dir apps/desktop build
pnpm.cmd tauri:build
```
