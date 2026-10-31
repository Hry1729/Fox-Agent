# Windows 安装包与受管运行时

> 现状（2026-10-31，B4c）：**Fox 已不用沙箱。** SRT、srt 账户、网络规则和 ACP stdio 桥已在 A1、B2 删除；Fox Host 命令在 Fox Job 中执行（见[安全架构](02-架构/安全架构.md)）。安装器里仍叫 “sandbox” 的脚本、环境变量和目录（`enable-sandbox.ps1`、`FOX_SANDBOX_RUNTIME_PACKAGE`、`%ProgramFiles%\Fox\sandbox-runtime`）只是历史命名，现在安装的是外部 Agent 用的受管运行时（Node 及外部 Agent 程序）。外部 Agent（Pi、Codex、dsh）计划在下一阶段恢复，目前不可用。本轮未构建或运行安装器。

`pnpm tauri:build:windows` 构建 x64 NSIS 安装包。安装器申请一次管理员授权，先安装受管运行时，再复制 Fox 应用文件。运行时安装失败时停止安装并返回非零退出码。

## 构建

先准备 `apps/desktop/src-tauri/windows/sandbox-release.json` 固定的运行时 ZIP，再执行：

```powershell
$env:FOX_SANDBOX_RUNTIME_PACKAGE = 'D:\releases\fox-sandbox-runtime-0.1.6-windows-x64.zip'
pnpm install --frozen-lockfile
pnpm tauri:build:windows
```

构建脚本（`scripts/prepare-sandbox-installer.mjs`）检查 ZIP 的 SHA256、Host 的运行时版本（`launch.rs` 的 `RUNTIME_PACKAGE_VERSION`）和模型适配器固定哈希；任何一项不一致都会阻止构建。ACP bridge 的哈希校验已随 bridge 删除。生成目录 `resources/sandbox-installer` 不进入 Git。

运行时包由 `build-runtime-package.py` 生成：不再打包 `srt\` 和 `acp-bridge\`，`pack` / `check` 遇到这两个目录直接拒绝。当前 pin 的 0.1.6 包是 B2 之前构建的，仍带 `srt\`；安装时第 6 步会把它删掉。在 Windows 上重建运行时包并同时更新 `sandbox-release.json`、`launch.rs` 的 `RUNTIME_PACKAGE_VERSION`、`build-runtime-package.py` 的 `DEFAULT_VERSION`，留给 B5。

## 安装

- NSIS 使用 `perMachine`，通过 `NSIS_HOOK_PREINSTALL` 调用 64 位 Windows PowerShell 运行 `enable-sandbox.ps1`。运行时 ZIP 随安装器分发，不需要另行下载或手动运行脚本。
- `enable-sandbox.ps1` 依次：
  1. 校验 ZIP 的 SHA256（先复制到仅管理员可写的暂存目录再校验）；
  2. 解压并安装到 `%ProgramFiles%\Fox\sandbox-runtime\<版本>`（同哈希时跳过，失败回滚）；
  3. 设置运行时目录的访问权限：Administrators / SYSTEM 完全控制，Users 只读和执行；
  4. 创建 `%ProgramData%\Fox` 与 `%ProgramData%\Fox\sandbox`（外部 Agent 私有目录的根）；
  5. 清理旧版 Fox 自己安装的 SRT：只有旧版安装回执证明这份 SRT 是 Fox 装的（`srt.alreadyInstalled == false`，且 `SandboxUserSid`、`CreatedAt` 与注册表一致）时，才用旧运行时自带的 srt 执行 `windows-uninstall`，删除 `srt-sandbox` 账户、`sandbox-runtime-users` 组、WFP 出站过滤规则和 `HKLM\SOFTWARE\sandbox-runtime`，再删 `%ProgramData%\sandbox-runtime`；别的程序装的 SRT 保留（`legacySrt.decision = keep`）。清理失败只记入结果与日志（`legacySrt.decision = failed`，并把归属证明转存到新回执的 `legacySrtRecord`，下次安装重试），不中止安装；
  6. 从本次安装的版本目录和已安装的旧版本目录删除 `srt\` 和 `acp-bridge\`（`removedRuntimeDirs`；第 5 步失败时旧版本的 `srt\` 保留）；
  7. 写结果 JSON。
- 安装日志位于 `%ProgramData%\Fox\logs\sandbox-installer-*.log`。失败时安装器显示该位置，不显示安装成功。
- 成功记录写入运行时目录的 `.fox-installer-result.json`，由管理员拥有，普通用户只读。
- 用户私有目录由非提权的 Fox 按当前用户创建。安装器不复制或修改模型密钥。
- 第 6 步（B5 起）也删除已安装旧版本目录（如 `sandbox-runtime\0.1.6\`）里的 `srt\`、`acp-bridge\`，旧版本目录的其余内容不动；只有第 5 步清理失败（`legacySrt.decision = failed`）时保留旧版本里的 `srt\`，供下次安装重试卸载。

NSIS 没有事务回滚。运行时安装失败会停止后续应用安装；若此前选择卸载旧版，旧应用不会自动恢复。运行时脚本只回滚本次写入的运行时版本，不删除用户数据。

原先按当前用户安装的 Fox 可能仍保留在用户目录。实际测试应从此次安装包生成的快捷方式启动。确认新版可用后再处理旧版。

## 验证

```powershell
node --test scripts/prepare-sandbox-installer.test.mjs
node scripts/test-sandbox-installer-hook.mjs
pwsh -File crates/fox-agent-backend/scripts/windows/tests/test-enable-faults.ps1
python -m unittest crates/fox-agent-backend/scripts/windows/tests/test_build_runtime_package.py
```

Hook 测试编译并运行不提权的隔离 NSIS 安装器，以假安装脚本检查非零退出和 `ok=false` 两条失败路径。以上脚本测试都不在 windows-ci 里。完整 UAC 安装、旧机器升级时的 SRT 清理、安装后的桌面启动和真实模型对话需要另行实测（清单见 B2 执行报告与 `crates/fox-agent-backend/docs/sandbox.md` 13.31）。

Tauri 安装钩子说明：[Windows Installer](https://v2.tauri.app/distribute/windows-installer/)。
