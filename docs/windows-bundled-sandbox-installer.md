# Windows 安装包内置沙箱

`pnpm tauri:build:windows` 构建 x64 NSIS 安装包。安装器申请一次管理员授权，先安装沙箱，再复制 Fox 应用文件。沙箱安装失败时停止安装并返回非零退出码。

> 适用范围（2026-10-28）：共享 SRT 仍供 Fox Host 命令使用。当前 `sandbox-release.json` 固定运行包 0.1.6；保留的外部模型适配器与 ACP bridge 校验属于资源兼容，不恢复退休驱动的执行权。本轮未构建或运行安装器，阶段二状态见[单 Pi 主线状态](01-项目概览/单Pi主线状态.md)。

## 构建

先准备 `apps/desktop/src-tauri/windows/sandbox-release.json` 固定的运行时 ZIP，再执行：

```powershell
$env:FOX_SANDBOX_RUNTIME_PACKAGE = 'D:\releases\fox-sandbox-runtime-0.1.6-windows-x64.zip'
pnpm install --frozen-lockfile
pnpm tauri:build:windows
```

构建脚本检查 ZIP 的 SHA256、Host 的运行时版本、模型适配器固定哈希和 ACP bridge 源码。任何一项不一致都会阻止构建。生成目录 `resources/sandbox-installer` 不进入 Git。更新运行时后，必须重新验证 ZIP 并更新固定发布信息。

## 安装和启动

- NSIS 使用 `perMachine`，通过 `NSIS_HOOK_PREINSTALL` 调用 64 位 Windows PowerShell。沙箱 ZIP 随安装器分发，不需要另行下载或手动运行脚本。
- 运行时安装到 `%ProgramFiles%\Fox\sandbox-runtime\0.1.6`。现有 srt 安装按原脚本幂等复用，不使用 `--force` 重建账号或网络规则。
- 安装日志位于 `%ProgramData%\Fox\logs\sandbox-installer-*.log`。失败时安装器显示该位置，不显示安装成功。
- 成功记录写入运行时目录的 `.fox-installer-result.json`。该文件由管理员拥有，普通用户只读。
- Fox 启动时检查记录及目录的权限，并核对版本、路径和当前 srt 注册表中的 SID、创建时间、标记版本。通过后写入当前用户的 `%LOCALAPPDATA%\Fox\state`。UAC 使用另一个管理员账号时，也不会把使用者的状态写进管理员配置目录。
- 用户私有目录仍由非提权的 Fox 按当前用户 SID 创建。安装器不复制或修改模型密钥。
- 卸载 Fox 保留共享的 srt 账号、网络规则和沙箱运行时。清理这些资源仍须满足 `decide_srt_uninstall` 的既有条件。

NSIS 没有事务回滚。沙箱失败会停止后续应用安装；若此前选择卸载旧版，旧应用不会自动恢复。运行时脚本只回滚本次写入的运行时版本，不删除用户数据。

原先按当前用户安装的 Fox 可能仍保留在用户目录。实际测试应从此次安装包生成的快捷方式启动。确认新版可用后再处理旧版。

## 验证

```powershell
node --test scripts/prepare-sandbox-installer.test.mjs
node scripts/test-sandbox-installer-hook.mjs
cargo test -p fox-agent-backend --lib winruntime::tests
```

Hook 测试编译并运行不提权的隔离 NSIS 安装器，以假安装脚本检查非零退出和 `ok=false` 两条失败路径。测试不会安装真实 Fox、srt 账号或网络规则。完整 UAC 安装、安装后的桌面启动和真实模型对话需要另行实测。

Tauri 安装钩子说明：[Windows Installer](https://v2.tauri.app/distribute/windows-installer/)。
