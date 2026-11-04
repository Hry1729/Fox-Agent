# Windows 测试取证脚本

给工程师实测用，Windows PowerShell 5.1 和 PowerShell 7 都能跑。取证脚本不改 Fox 数据，不结束任何进程。全部是 ASCII 文本（中文用字符码拼出来），5.1 下不会出现编码问题。除 `Initialize-FoxTestEnv.ps1`、`Start-FoxBatch.ps1`、`New-FoxTestFixture.ps1`（会建目录、改名文件，见各自说明）外，脚本只读。

脚本直接在仓库里运行，不用复制到别处。**测试根目录**默认是 `<仓库>\docs\windows-test\run-<版本>\`（例如 `run-1339`，由脚本所在目录推出来，和当前目录无关；已被 `.gitignore` 的 `/docs/windows-test/run-*/` 忽略）。数据目录 `data-*`、测试项目、`logs\`、`fox-evidence\`、`shots-*\`、`Fox-app\`、结果文件都在这里。各脚本的 `-TestRoot` / `-OutDir` 等参数仍可覆盖。

一条命令准备环境（`git pull`、安装 Fox 之后，**普通** PowerShell）：

```powershell
cd <仓库>\scripts\windows-test
.\Initialize-FoxTestEnv.ps1
```

| 脚本 | 作用 |
|---|---|
| `Collect-FoxEvidence.ps1` | 一键收集。依次调用 `Get-FoxIntegrity.ps1`、`Get-FoxProcessTree.ps1`、`Search-FoxLog.ps1`、`Get-FoxDbReport.ps1`，输出 `<标签>-<时间>.json`（全部数据）和 `.txt`（摘要，直接贴回即可）。某一部分失败只记进 `errors`，不影响其他部分。 |
| `Get-FoxProcessTree.ps1` | 进程树快照，每个进程记录 pid、ppid、创建时间、账户、受限令牌、是否在任意 Job 中、是否在 Fox Job 中。 |
| `Search-FoxLog.ps1` | 搜索 Fox 日志里的 `cancel_swept`、`outside_job`/`outsideJob` 和错误码。 |
| `Get-FoxDbReport.ps1` | 查询 `fox.db` 的副本，包括最近的 runs、各 Run 的事件、`cancel_swept` 事件、`agent_backend_audit_log`、`permission_audit`、`kernel_jobs`，以及这些 Run 的工具卡 `tool_calls`（状态、输出长度、`streamedOutput` / `outputTruncated`、结果头尾）。 |
| `Get-FoxIntegrity.ps1` | 读 Fox 进程令牌：完整性级别（必须是 Medium）和是否提权。`Collect-FoxEvidence.ps1` 每次都会顺带记录。 |
| `Initialize-FoxTestEnv.ps1` | 在**普通** PowerShell 里一次建好测试根目录：中文名测试项目（`readme.txt`、`sub\`）和它的备份 `backup\project-<版本>\`、`project2\`、`shots-<版本>\`、`logs\`、`outside\`、`fox-evidence\`，把 `C:\Program Files\Fox` 复制到 `Fox-app\`（不含 `sandbox-runtime`），没有结果文件时写文件头模板。可重复运行：已有的项目、备份、结果文件不动，只刷新 `Fox-app`。最后逐项打印 ok / created / missing 和下一步。 |
| `Start-FoxBatch.ps1` | 在**普通** PowerShell 里复位并启动一批：复制基线数据目录、用备份还原测试项目、清掉 `FOX_DEV_UI`、设 `FOX_DATA_DIR`、stderr 写日志，启动后检查完整性。在管理员 PowerShell 里会拒绝运行。 |
| `Test-FoxTextFile.ps1` | 看一个文件的编码和换行：BOM、CRLF / 单独 LF、是否合法 UTF-8、能否按 GBK（936）往返、首行是否 `chcp 65001`。用于 `.bat` 编码检查。 |
| `Get-FoxProxyInfo.ps1` | Fox 能看到的代理来源（进程环境、用户环境、WinINET 系统代理）和 DNS 是否返回 fake-ip（198.18.0.0/15），给出能否复现 fake-ip 放行的结论。代理地址里的用户名密码会打码。 |
| `New-FoxTestFixture.ps1` | 准备 / 撤销特殊测试环境：`AclTest`（管理员，造一个所有者是 Administrators 的目录）、`Skills`（三个坏技能）、`Junction` / `DeleteSub` / `SubRestore`、`Fsmonitor`、`Hide` / `RestoreHidden`（临时改名运行时文件或模型文件）、`Find`（找这些文件的位置）。 |
| `Compress-FoxEvidence.ps1` | 把结果文件、证据文件夹和截图打成一个 zip（UTF-8 文件名），输出 SHA256，并列出结果文件里还没有记录的编号。 |

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
cd <仓库>\scripts\windows-test
$R = (Resolve-Path ..\..\docs\windows-test\run-1339).Path
# 普通 PowerShell：复位并启动一批（stderr 写到 $R\logs\fox-1339-2.log）
.\Start-FoxBatch.ps1 -Batch 1339-2

# 管理员 PowerShell：取消测试，先拍一张基线（输出默认在 $R\fox-evidence\）
.\Collect-FoxEvidence.ps1 -Label b2-before -ProcessOnly
# 开始这一轮，等子进程出现后点停止，再等 3 秒
.\Collect-FoxEvidence.ps1 -Label b2-after -Baseline "$R\fox-evidence\b2-before-*.json" `
    -LogPath "$R\logs\fox-1339-2.log" -DataDir "$R\data-1339-2"
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
