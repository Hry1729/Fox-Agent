# Windows 测试取证脚本

给工程师实测用，Windows PowerShell 5.1 和 PowerShell 7 都能跑。脚本只读：不改 Fox 数据，不结束任何进程。全部是 ASCII 文本，5.1 下不会出现编码问题。

| 脚本 | 作用 |
|---|---|
| `Collect-FoxEvidence.ps1` | 一键收集。依次调用下面三个脚本，输出 `<标签>-<时间>.json`（全部数据）和 `.txt`（摘要，直接贴回即可）。某一部分失败只记进 `errors`，不影响其他部分。 |
| `Get-FoxProcessTree.ps1` | 进程树快照，每个进程记录 pid、ppid、创建时间、账户、受限令牌、是否在任意 Job 中、是否在 Fox Job 中。 |
| `Search-FoxLog.ps1` | 搜索 Fox 日志里的 `cancel_swept`、`outside_job`/`outsideJob` 和错误码。 |
| `Get-FoxDbReport.ps1` | 查询 `fox.db` 的副本，包括最近的 runs、各 Run 的事件、`cancel_swept` 事件、`agent_backend_audit_log`、`permission_audit` 和 `kernel_jobs`。 |

## 判断「是否在 Fox Job 中」

结果是确定的，不靠推测。脚本先读取系统句柄表，找出 Fox 进程（`Fox.exe` / `fox-desktop.exe`）持有的全部 Job 句柄，把它们复制到本进程，再读取每个 Job 的成员 pid 列表（嵌套的子 Job 也会列出）。

满足以下任一条件、并且不在 Fox Job 中的进程，会标为 `suspect`：

- `foxDescendant`：沿父进程链能追到 Fox。追溯时会校验 pid 和创建时间，复用的 pid 不算。
- `viaSeclogon`：父进程是 Secondary Logon 服务。用 `CreateProcessWithLogonW` 以另一个账户启动的进程就是这种情况，例如 Codex 沙箱账户。
- `accountMatch`：账户名匹配 `-SandboxAccountPattern`，默认匹配 `codex|sandbox`。

suspect 的后代也会一并标为 suspect。

查看其他账户进程的账户名通常需要管理员权限。Fox 本身仍按普通用户运行，只有取证脚本需要在管理员 PowerShell 中运行。没有管理员权限时，这些进程的 `accessDenied = true`。

## 用法

```powershell
# Fox 从终端启动，stderr 写到文件（日志就是这个文件）
& 'C:\Program Files\Fox\Fox.exe' 2> D:\fox-test\fox-stderr.log

# 取消测试：先拍一张基线
.\scripts\windows-test\Collect-FoxEvidence.ps1 -Label b2-before -ProcessOnly
# 开始这一轮，等子进程出现后点停止，再等 3 秒
.\scripts\windows-test\Collect-FoxEvidence.ps1 -Label b2-after -Baseline .\fox-evidence\b2-before-*.json `
    -LogPath D:\fox-test\fox-stderr.log -DataDir D:\fox-test\data-1336
```

`.txt` 摘要里：

- `SUSPECT` 行：不在 Fox Job 中的可疑进程。
- `ALIVE` 行：基线之后新建、到现在仍存活、并且和 Fox 相关的进程。
- `[log]` 段：`cancel_swept` 行和错误码计数。
- `[db]` 段：最近的 runs 和清扫事件。

## 数据库查询引擎

`Get-FoxDbReport.ps1` 按以下顺序选择查询引擎：

1. `sqlite3`（需要 3.33 及以上版本，因为要用 `-json`）；
2. `System.Data.SQLite`（用 `-SqliteAssembly` 指定 DLL 路径，或从 GAC 加载）；
3. Python 自带的 `sqlite3` 模块。

三种都不可用时，数据库副本和 `fox-queries.sql` 会保留在输出目录。之后可以运行 `sqlite3 fox.db < fox-queries.sql`，也可以直接回传这个文件夹。副本里有对话内容，默认用完就删除；加 `-KeepSnapshot` 可以保留。
