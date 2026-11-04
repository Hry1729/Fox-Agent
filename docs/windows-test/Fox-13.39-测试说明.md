# Fox 13.39 Windows 测试说明

**被测版本：main `1bb0cdb9`**（`1bb0cdb96b027a27367b626f50825ca261067611`）。之后 main 上只多了本说明和 `scripts/windows-test/` 的提交（带 `[skip ci]`，不改产品代码），用最新 main 构建，产物和 `1bb0cdb9` 相同。运行时包仍是 **0.1.8**。

13.38 实测（`32df063d`）之后合入了三路修复：

| 合入 | 内容 | 本说明里的编号 |
|---|---|---|
| ui-1104（`dc4a0331`） | UI 评审 01–08：输入框底栏、Agent / 项目菜单、取消图标 CircleStop、交付文件 chip、资料库 chip、审批卡随对话滚动、设置分组和「通用」页结构 | U-1～U-8 |
| fix-1338-run（`b9730626`） | A-2 取消后保留工具卡和输出；R1-2 dsh 取消清扫兜底和 `access_denied` 审计；R1-3 / B4-2 连接中断写中文终态、普通模式不显示英文原文；R1-4 后台任务终态说明、同一会话可只读查询；B3-4 Pi 默认用模型服务时写审计 | R1-1～R1-6、B3-4、B4-2 |
| fix-1338-ui（`1bb0cdb9`） | B4-5 发送前预检模型地址、外部 Agent 原生登录不预检；R2-4 第一轮没完成时保留托盘、「更多 → 更换项目」；B5-1 `.bat` 按系统代码页和 CRLF 写；B5-2 代理变量只给自动档和完全访问档；B5-3 web_read 的 fake-ip 放行、审计和中文提示 | B4-5、R2-4、B5-1～B5-3 |

## 修复项清单

| 编号 | 13.38 结果 | 修了什么 | 这一轮的期望 |
|---|---|---|---|
| A-2 | 失败（Pi / dsh / Codex） | 投影层读取 `_meta.terminal_output[_delta]`，取消宽限期内仍接收已在运行的工具卡输出，取消时把卡定格为「已取消」，Fox 原生 `run_command` 每秒写一次输出 | 取消后这一轮默认展开，「执行命令」卡显示「· 已取消」，展开能看到取消前的输出，末尾有「[已取消，以上是取消前已经产生的输出]」，摘要写「1 项已取消」 |
| R1-2 | 失败（pwsh 残留，Win32 5） | 启动时校验 Job 限制；清扫遇到拒绝访问时，沿父进程链结束 Job 外的祖先；仍结束不了就写审计 `agent_backend.cancel_sweep_access_denied` | 取消后没有残留的 `pwsh.exe`；拒绝访问有审计，取消照常在 1～2 秒内完成 |
| R1-3 / B4-2 | 失败（英文「Pi released its Run…」） | 投影被拒时同步写失败终态；释放时如果这一轮有过重试或 `provider.*` 错误，就记成 `provider.request_failed`；前端按错误码显示中文，成句英文在普通模式下一律换成中文兜底 | 普通模式显示「运行失败 / 模型服务连接中断，可重试。」，界面上没有英文；开发者模式「查看错误详情」里有错误码和英文原文 |
| R1-4 | 失败（「这只是抽样结果」） | 后台任务状态带 `final` / `stateLabel` / `note`；同一会话里后一轮可以只读查询前一轮的任务 | 回复明确说「已取消」或「已结束（退出码 N）」；查询上一轮任务的 `status` 不被拒；对它 `cancel` 仍被拒 |
| B3-4 | 通过（缺审计） | 从没选过配置来源、默认用模型服务时也写审计 | 审计里有 `agent_backend.pi_source_defaulted_to_fox`，`reason=no_source_chosen`，同一项目只写一次 |
| B4-5 | 失败（直接建 Run 失败） | 发送前探测模型服务地址；外部 Agent 已有自己的原生登录时不探测 | 连不上时 1～3 秒内弹「连不上模型服务」，草稿保留，不建 Run；Pi 用本机 `~/.pi` 时照常发送 |
| R2-4 | 失败（没有托盘、没有入口） | 有一轮完成之前托盘一直在；会话头部「更多」菜单加「更换项目」 | 第一轮被取消后托盘还在；「更多 → 更换项目」能换，运行中灰掉并写原因 |
| B5-1 | 失败（UTF-8 + LF） | 脚本文件按系统 ANSI 代码页和 CRLF 写；编码不了时用 UTF-8 并在首行加 `chcp 65001 >nul` | `测试.bat 参数一` 输出正确，文件是 GBK（936）、CRLF、无 BOM |
| B5-2 | 部分通过（代理变量没带进命令） | 代理变量只在自动档和完全访问档带进命令 | 自动、完全访问能看到 `HTTP_PROXY` / `HTTPS_PROXY`；询问档看不到 |
| B5-3 | 未触达（英文拦截说明） | 有代理时放行 198.18.0.0/15 并写审计 `proxy_fake_ip`；拦截说明改成中文 | fake-ip 下能读到内容并有审计；被拦时是中文句子 |
| U-1～U-8 | 新增 | UI 评审 01–08 | 见批次 7 |

**13.38 说明里和实际不符、本轮已改正的两处：**

1. **dsh 只有三档**：「仅可查看」（查看）、「工作区完全修改」（工作）、「完全权限」（完全）。没有「询问」，原来写「询问」的地方，dsh 一律用「**工作区完全修改**」（它就是 dsh 的询问档：工作区内自由修改，越出工作区由 dsh 发起审批）。
2. **B4-8 完全访问确认框**：按 13.38 实测，弹框里的主按钮是「确认使用<档位全名>」（Fox / Pi 是「确认使用完全访问」，Codex 是「确认使用完全访问权限」，dsh 是「确认使用完全权限」），右上角有 ×（读屏名「关闭弹窗」）。不再要求「确认使用「…」？」标题，也不要求按钮是两字缩写；有没有「取消」按钮按实际记录。

---

## 本轮批次一览

每批约 10–15 分钟，互不依赖，可以各开一个新会话、按任意顺序执行（**批次 1～5 优先**，它们是 13.38 的失败项）。

| 批次 | 内容 | 编号 | 需要提前准备 |
|---|---|---|---|
| 1 | 取消后保留过程：Pi、Codex、Fox 原生 | R1-1、R1-5、R1-6 | 无 |
| 2 | dsh 取消与 pwsh 残留；dsh 授权被拒 | R1-2、R2-1 | R2-1：管理员 PowerShell 建 `acl-test` 并改所有者 |
| 3 | 后台任务终态；模型服务连接中断的中文提示 | R1-4、R1-3、B4-2 | R1-3：模型服务能中途断开（本机网关可以关掉，或能断网） |
| 4 | 发送前预检；第一轮未完成的托盘和「更换项目」；「去设置」入口 | B4-5、B4-5b、R2-4、B4-11 | 第二个项目 `run-1339\project2`；B4-5b 需要本机原生 pi |
| 5 | 命令环境：`.bat` 编码、代理变量、web_read | B5-1、B5-2、B5-3 | 系统代理（Clash 等）打开；先跑 `Get-FoxProxyInfo.ps1` 确认是不是 fake-ip |
| 6 | Agent 设置：Pi 默认来源审计、配置来源切换、「未安装」 | B3-4、B4-12、B4-13 | 全新数据目录；B4-12 需要本机原生 pi；B4-13 用管理员临时改名 dsh 启动文件 |
| 7 | UI 评审 01–08 与完全访问确认框 | U-1～U-8、B4-8 | 无（以截图为主） |
| 8 | 13.31 剩余：坏技能、非推理模型、junction 恢复、大结果续读、恶意 fsmonitor | B5-4～B5-8 | 脚本建三个技能、junction、fsmonitor；B5-5 需要没有 reasoning 字段的模型 |
| 9 | 向量检索失败；外部 Agent 失败提示 | B6-2、B6-3 | 已就绪的向量索引；管理员临时改名模型文件和 `codex-acp` |
| 10（可选） | 13.32 F3–F5、旧库 Kernel Run、默认构建与冷启动 | B6-5、B6-6、B6-7 | exFAT 盘、GBK 文件、旧数据目录 |

13.38 已通过的 R2-2、R2-3、R2-5、B3-1～B3-3、B3-5、B3-6、B4-4、B4-6、B4-7、B4-9、B4-10、B6-1、B6-4 不再重复。

---

## 通用准备（只做一次）

**测试根目录**（下文简称「运行根」）：`<仓库>\docs\windows-test\run-1339\`，例如 `D:\python\projects\Fox\Fox\docs\windows-test\run-1339`。它在仓库检出目录里（测试机上的 dsh 只能写仓库内），已被 `.gitignore`（`/docs/windows-test/run-*/`）忽略，不会被提交。下文表格里写成 `run-1339\…`；PowerShell 命令里用变量 `$R` 表示它的完整路径；要在 Fox 里输入完整路径的地方写成 `<R>\…`，换成运行根的完整路径。脚本直接在仓库的 `scripts\windows-test\` 里运行，不用复制到别处。

### 1. 拉代码、构建与安装

- 源码：在仓库目录 `git pull`（或 `git fetch && git checkout origin/main`），`git log -1` 是本说明的提交（或其后），`git log --oneline -6` 里能看到 `1bb0cdb`。
- 运行时包：**0.1.8**，SHA256 `0f9c199da3aed28b3ccdc52885c04a49548c79f65e98a89106836e50e35d397f`（来源、校验方法同 13.35）。
- VS 2026 构建机先设运行库目录，再执行 `pnpm tauri:build:windows`：

  ```powershell
  $env:FOX_WINDOWS_CRT_DIR = 'C:\Program Files\Microsoft Visual Studio\18\<Edition>\VC\Redist\MSVC\<版本>\x64\<CRT 目录>'
  ```

- 安装到默认位置 `C:\Program Files\Fox`。记下安装包 SHA256（`Get-FileHash <安装包>`），写进结果文件头。

### 2. 一条命令准备测试环境

在**普通** PowerShell（不要「以管理员身份运行」）里：

```powershell
cd D:\python\projects\Fox\Fox\scripts\windows-test      # 换成你的 <仓库>\scripts\windows-test
.\Initialize-FoxTestEnv.ps1
```

它会在运行根下建好（已有的不覆盖，可以重复运行）：

| 路径 | 用途 |
|---|---|
| `run-1339\测试项目\` | 测试项目（中文路径，含 `readme.txt` 和 `sub\`），备份到 `run-1339\backup\project-1339\`，每批开头由 `Start-FoxBatch.ps1` 还原 |
| `run-1339\project2\` | 第二个项目（R2-4 换项目用），含 `readme.txt` |
| `run-1339\Fox-app\` | 从 `C:\Program Files\Fox` 复制的 exe（不含 `sandbox-runtime`）。不要从下载目录、解压目录直接运行（那些目录带 Low 完整性标签）；仓库目录是普通目录，没有这个问题 |
| `run-1339\shots-1339\` | 截图，文件名用 `<编号>-<内容>.png` |
| `run-1339\logs\` | Fox 日志（`Start-FoxBatch.ps1` 写入） |
| `run-1339\fox-evidence\` | 取证输出（`Collect-FoxEvidence.ps1` 默认写这里，和当前目录无关） |
| `run-1339\outside\` | 项目外目录（B5-6 junction 指向这里） |
| `run-1339\results-1339.md` | 结果文件（没有时写入文件头模板） |

最后逐项打印 `ok` / `created` / `missing`，以及 `fox-desktop.exe` / `Fox.exe` 的修改时间（复制件和安装目录各一个，应该一致）。有 `missing`（例如 Fox 还没安装）就按提示补上后再跑一次。**重新安装 Fox 后再跑一次**，`Fox-app` 会刷新（Fox 必须先关掉）。

运行根下只保留 `测试项目` 这一个中文名文件夹，`Start-FoxBatch.ps1` 靠它找测试项目（找错时加 `-ProjectDir`）。测试项目在仓库检出目录里面，但不是仓库的一部分（被忽略）；如果某一项的现象看起来和 Git 有关（例如 git_read 看到的是 Fox 仓库、文件搜索什么都找不到），记进备注。

### 3. 两个 PowerShell 窗口

- **普通 PowerShell**（不要「以管理员身份运行」）：只用来启动 Fox。`Start-FoxBatch.ps1` 在管理员窗口里会直接拒绝。
- **管理员 PowerShell**：取证（`Collect-FoxEvidence.ps1` 要读其他账户的进程）和少数准备步骤（`New-FoxTestFixture.ps1 -Action AclTest / Hide / RestoreHidden`）。

> **提醒：Fox 必须以普通用户、Medium 完整性运行。** 管理员窗口里启动的 Fox 是 High 完整性，ACL、所有者、Job 清扫的结果都不算数。每次 `Start-FoxBatch.ps1` 启动后会打印 `ok=True  Fox runs at Medium integrity, not elevated`；每份证据的摘要第一段 `[integrity]` 也会记录。看到 `ok=False` 就关掉 Fox，从普通窗口重新启动。

两个窗口都先执行（`Initialize-FoxTestEnv.ps1` 结尾也会打印这两行）：

```powershell
cd D:\python\projects\Fox\Fox\scripts\windows-test      # 换成你的 <仓库>\scripts\windows-test
$R = (Resolve-Path ..\..\docs\windows-test\run-1339).Path
```

### 4. 基线数据目录 `data-1339-base`

在普通窗口里：

```powershell
.\Start-FoxBatch.ps1 -Batch 1339-base -Fresh -KeepProject
```

（基线目录是 `run-1339\data-1339-base`，后面各批从它复制。）然后在 Fox 里：

1. **设置 → 模型服务**：配一个 **OpenAI 兼容** 供应商（例如 Magpie）设为默认，测试连接通过；给它加一个 **Anthropic Messages** 地址并「验证此协议」通过，打开「用作 DeepSeek 默认协议」（dsh 要用）。有官方 DeepSeek Key 时也配上。
2. **设置 → Agent**：打开 Pi、Codex、dsh 的「启用」，三者都是「已检测到」。
3. 侧栏添加两个项目：`<R>\测试项目\`、`<R>\project2\`（文件夹选择框里打开运行根即可）。
4. **设置 → 对话**：开发者模式**关闭**。
5. 关闭 Fox。

### 5. 每批开头的复位

1. 关闭 Fox；管理员窗口跑一次 `.\Get-FoxProcessTree.ps1`，确认没有残留的 `fox-desktop` / `node` / `codex*` / `dsh*` / `pi*` / `pwsh`（有残留先记下来再结束）；
2. 普通窗口：`.\Start-FoxBatch.ps1 -Batch 1339-<批次号>`（自动复制基线、还原测试项目、启动、检查完整性）；
3. 管理员窗口设好本批变量（之后的取证命令都用它们）：

   ```powershell
   $B = '1339-<批次号>'
   $D = "$R\data-$B"
   $L = (Get-ChildItem "$R\logs\fox-$B*.log" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
   ```

   **每次重启 Fox 都会写一个新日志（`fox-<批次>-2.log` …），重启后重新执行一次 `$L` 那一行。**

### 6. 取证

```powershell
.\Collect-FoxEvidence.ps1 -Label <编号>-before -ProcessOnly                       # 动作前的进程基线
.\Collect-FoxEvidence.ps1 -Label <编号> -Baseline "$R\fox-evidence\<编号>-before-*.json" -LogPath $L -DataDir $D
```

输出在 `run-1339\fox-evidence\`：`<编号>-<时间>.json` 是全部数据，`.txt` 是摘要。摘要里本轮新增的几段：

- `[integrity]`：Fox 的完整性级别；
- `tool … status=… resultLen=… streamedOutput=…`：这些 Run 的工具卡（A-2 看这里）；
- `audit …`：`cancel_swept`、`cancel_sweep_access_denied`、`pi_source_defaulted_to_fox` 等审计；
- `permission …`：`permission_audit`（`reason=proxy_fake_ip`、自动放行记录）；
- `job … state=…`：后台任务；
- `SETTLED …`：日志里带 `cancel_settled_ms` 的行。

**停止耗时**：`cancel_swept` 行末尾有 `confirm_ms=… grace_ms=… sweep_ms=…`，`SETTLED` 行有 `cancel_settled_ms=…`，每次停止都抄下这四个数。

数据库副本含对话内容，脚本用完就删。

### 7. 结果文件与打包

结果写在 **`run-1339\results-1339.md`**（`Initialize-FoxTestEnv.ps1` 已写好文件头模板，填上值即可），文件头：

```
Fox 13.39 Windows 实测结果（1339）
机器型号：
Windows 版本：
提交 sha：<git log -1 的完整 sha>
安装包 SHA256：
运行时 SHA256：0f9c199d…（已校验 / 未校验）
Fox 完整性：Medium、未提权（Get-FoxIntegrity ok=True）
```

之后每批一个小标题「批次 N：…」，每项一行：

```
<编号> 通过|失败|未触达  证据：<文件名>[, <文件名>]  备注：<一句话；失败或未触达必填>
```

涉及停止的项，备注里加：`耗时 confirm=<ms> grace=<ms> sweep=<ms> settled=<ms> 体感=<点停止到「已取消」的秒数>`。

全部做完（或今天收工）时，在管理员或普通窗口里：

```powershell
.\Compress-FoxEvidence.ps1
```

生成 **`run-1339\fox-evidence-1339.zip`**（含 `results-1339.md`、`fox-evidence\`、`shots-1339\` 和清单），打印 SHA256，并列出结果文件里还没写的编号。把 zip 和 SHA256 一起发回。日志含对话内容，默认不打包；需要时加 `-IncludeLogs`。

---

## 批次 1：取消后保留过程（约 15 分钟，R1-1、R1-5、R1-6）

**前置**：复位，`-Batch 1339-1`。

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| R1-1 | **Pi**：新建 Pi 会话，绑定测试项目，档位**询问**。采集 `R1-1-before`（`-ProcessOnly`）。发「运行命令 ping -n 60 127.0.0.1，告诉我结果」，批准，**看到输出 3～5 秒后**点停止。3 秒后采集 `R1-1`。然后点开「执行命令」卡 | ① 这一轮结束后过程区**默认展开**；② 「执行命令」卡显示「· 已取消」，展开能看到若干行「来自 127.0.0.1 的回复」，末尾一行「[已取消，以上是取消前已经产生的输出]」；③ 摘要写「1 项已取消」；④ 全程不出现「已停止」（「正在取消」可能一闪而过，没看到不算失败）；⑤ 证据摘要里该卡 `status=cancelled`、`resultLen` 明显大于 0、`streamedOutput=1`；没有 ping 的 `ALIVE` 行（Pi 常驻会话进程 `inFoxJob=True` 不算） | `R1-1` 证据、截图 `R1-1-card.png`（卡片展开）、四个耗时 |
| R1-5 | **Codex**：同 R1-1，新建 Codex 会话（档位「请求审批」），标签用 `R1-5` | 同 R1-1（Codex 的输出来自 `terminal_output_delta`）。没有 ping 的 `ALIVE` 行 | `R1-5` 证据、截图 `R1-5-card.png`、四个耗时 |
| R1-6 | **Fox 原生**：新建 Fox 会话，绑定测试项目，档位**询问**。采集 `R1-6-before`。发「用 run_command 执行 ping -n 60 127.0.0.1」，批准，看到输出几秒后点停止，3 秒后采集 `R1-6` | 卡片「已取消」，展开能看到中文 ping 输出（运行中大约每秒刷新一次），末尾有「[已取消，以上是取消前已经产生的输出]」；证据里该卡 `status=cancelled`、`resultLen>0`；没有 `ALIVE` 行 | `R1-6` 证据、截图 `R1-6-card.png`、四个耗时 |

回传：R1-1、R1-5、R1-6，每项一行，备注带耗时。

---

## 批次 2：dsh 取消与授权被拒（约 15 分钟，R1-2、R2-1）

**前置**：复位，`-Batch 1339-2`。

**R2-1 的准备（必须在管理员 PowerShell 里做，先做）**：

```powershell
.\New-FoxTestFixture.ps1 -Action AclTest -User <当前登录的普通用户名> -OutPath "$R\fox-evidence\R2-1-fixture.json"
```

它会建 `run-1339\acl-test`，把所有者改成 `BUILTIN\Administrators`，并给你的账户「修改」权限（可继承）。输出里 `owner` 应是 `BUILTIN\Administrators`、`ok=True`。（13.38 里 R2-1 没触达，就是因为普通窗口改不了所有者。）然后在 Fox 侧栏把 `<R>\acl-test` 添加为项目。

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| R1-2 | **dsh**：新建 dsh 会话，绑定测试项目，档位「**工作区完全修改**」（dsh 没有「询问」）。采集 `R1-2-before`。让它执行 `powershell -NoProfile -Command "1..600 \| ForEach-Object { Start-Sleep -Seconds 1; $_ }"`（每秒一个数字；不要用 ping，dsh 沙箱里 ping 不可用），出审批卡就批准，看到数字输出约 10 秒后点停止。3 秒后采集 `R1-2`，**30 秒后**再采集一次 `R1-2b`（`-Baseline` 仍用 `R1-2-before`） | ① 卡片「已取消」，展开能看到已经输出的数字（数字连续）；② 取消在 1～2 秒内完成，界面不卡；③ **`R1-2` 和 `R1-2b` 里都没有 `pwsh.exe` / `powershell.exe` 的 `ALIVE` 行**；④ 摘要 `audit` 段有 `agent_backend.cancel_swept`（详情里有 `accessDenied`）；如果清扫时遇到拒绝访问，另有一条 `agent_backend.cancel_sweep_access_denied`（含 pid、`parentChain`、`parentChainKilled`）。只要还有 pwsh 残留就记**失败**，备注抄下残留进程的 pid / `inFoxJob` 和这条审计的内容 | `R1-2`、`R1-2b` 证据，四个耗时 |
| R2-1 | **dsh 授权被拒**：新建 dsh 会话，绑定 `<R>\acl-test`，档位「工作区完全修改」，发「在项目里新建 a.txt，内容写 hi」。采集 `R2-1a`。如果失败：打开开发者模式（设置 → 对话）看「查看错误详情」，再关掉。然后在管理员窗口执行 `.\New-FoxTestFixture.ps1 -Action AclTestOwnerFix -User <普通用户名>`，回到会话重发，采集 `R2-1b` | 第一次失败时提示是一句中文：dsh 没能给项目目录加上写入授权，请把目录所有者改为当前用户或给完全控制后重发；开发者模式详情里错误码是 `agent_backend_workspace_acl_denied`（不是笼统的 `agent_backend_exited`）。改完所有者重发后 `a.txt` 生成。**如果第一次就成功了**（dsh 没报 grantWrite），记「未触达」，备注写明 `AclTest` 输出的所有者 | `R2-1a`、`R2-1b` 证据，`R2-1-fixture.json`，截图 `R2-1-hint.png` |

做完把 `acl-test` 从侧栏移除即可，目录留着不影响别的批次。

回传：R1-2、R2-1。

---

## 批次 3：后台任务终态与连接中断提示（约 15 分钟，R1-4、R1-3、B4-2）

**前置**：复位，`-Batch 1339-3`。开发者模式关闭。R1-3 需要能在回复中途断开模型服务：用本机网关（例如 Magpie）时，准备好随时关掉它的窗口 / 进程；用官方 DeepSeek 时，准备好在任务栏断开网络。

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| R1-4 | 新建 **Fox** 会话，绑定测试项目，档位**询问**。发「用 run_command 的 start（后台）执行 `ping -n 120 127.0.0.1`，然后每隔几秒用 status 查一次，一共查 10 次」，批准，在查询过程中点停止。等 10 秒，在同一会话发「上一轮那个后台 ping 任务现在是什么状态？请用 run_command 的 status 查一下」。回复后再发「把上一轮那个后台任务 cancel 掉」。采集 `R1-4` | ① 回复明确说「已取消」（或「已结束（退出码 N）」），**不再**说「抽样」「仍在运行」「不能下结论」；② 这一轮里 `status` 查询成功（工具卡完成），不出现「outside this Run」之类的拒绝；③ 对上一轮任务的 `cancel` 被拒（这是设计）；④ 证据 `job` 段这条任务 `state=cancelled`，和回复一致 | `R1-4` 证据、截图 `R1-4-reply.png` |
| R1-3 | **连接中断**：新建 Fox 会话，发「写一篇约 3000 字的文章，介绍 Windows 的进程和作业对象」，等正文开始输出后**关掉模型服务**（关掉 Magpie，或断网）。等这一轮结束（可能先重试一会儿，最多等 1 分钟）。采集 `R1-3a`。然后**恢复模型服务**，新建 Pi 会话重复一次，采集 `R1-3b`。最后打开开发者模式，回到这两个失败的会话点「查看错误详情」，再关掉开发者模式 | ① 普通模式下失败卡片显示「运行失败」和「模型服务连接中断，可重试。」；界面上（卡片、toast、过程区）**没有任何英文句子**，没有「Pi released its Run…」；② 证据 `run` 段这一轮 `error=provider.request_failed`，**不再**出现 `runtime.released_without_terminal`；③ 开发者模式的「查看错误详情」里能看到错误码和英文原文。用回环网关时，如果 Pi 因 `agent_backend_loopback_requires_full_access` 起不来，那是既定规则，换 Fox 会话的结果为准并在备注写明 | `R1-3a`、`R1-3b` 证据，截图 `R1-3-normal.png`、`R1-3-dev.png` |
| B4-2 | **普通模式不显示调试信息**：在开发者模式关闭的状态下，看一眼 R1-3 的失败卡片、R1-4 里一次正常 Run 的详情，以及一张审批卡 | 都没有错误码、原始异常、英文原文、内部 id；需要展开的内容默认收起 | 截图 `B4-2-approval.png` |

回传：R1-4、R1-3、B4-2。

---

## 批次 4：发送前预检、托盘和「更换项目」（约 15 分钟，B4-5、B4-5b、R2-4、B4-11）

**前置**：复位，`-Batch 1339-4`。基线里已有 `project2`。开发者模式关闭。

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| B4-5 | 先在 Fox 里发一轮「回复 ok」确认正常。采集 `B4-5-before`（**不加** `-ProcessOnly`，要记下当前的 runs）。到「设置 → 模型服务」把默认供应商的地址改成 `http://127.0.0.1:9/v1`，**保存，不测试**。① 在**已有**的 Fox 会话里发「回复 ok」；② 新建 Fox 对话再发一次；③ 点 toast 上的「去设置模型服务」。采集 `B4-5`。打开开发者模式再发一次看 toast，然后关掉开发者模式，把地址改回原值并测试通过 | ① ② 都在 1～3 秒内弹 toast：标题「连不上模型服务」，正文「请检查地址，或确认服务已经启动。草稿已保留。」，带「去设置模型服务」；不会停在「等待开始」；草稿留在输入框；普通模式不显示地址和错误码；③ 直接打开「设置 → 模型服务」；④ 证据 `run` 段和 `B4-5-before` 相比**没有新 Run**；⑤ 开发者模式下 toast 里能看到 `model.connection_unreachable` 和地址 | `B4-5-before`、`B4-5` 证据，截图 `B4-5-toast.png` |
| B4-5b | **外部 Agent 原生登录不预检**（需要本机装过原生 pi，`~/.pi` 里有可用配置，在终端里 `pi` 能正常回复；没有就记「未触达」）。保持模型服务地址为 `http://127.0.0.1:9/v1`：到「设置 → Agent」把 Pi 的「配置来源」切到「本机 ~/.pi」，新建 Pi 会话发「回复 ok」；再切回「Fox 模型服务」，同一会话或新会话再发一次。采集 `B4-5b`。做完把地址改回 | 用本机 `~/.pi` 时照常发送并回复 ok，**不弹**「连不上模型服务」；切回 Fox 模型服务后弹「连不上模型服务」toast，不建 Run。（可选：开着系统代理，把模型服务地址填一个公网域名，预期不预检、照常建 Run，由这一轮自己报错） | `B4-5b` 证据 |
| R2-4 | **第一轮没完成时保留托盘**：新建 Pi 会话，绑定测试项目，档位询问，发「运行命令 ping -n 60 127.0.0.1」，批准后点停止。等显示「已取消」后看输入框下方；点托盘里的 Agent 改成 Fox。**更换项目**：新建 Fox 会话发「回复 ok」让它完成一轮，打开会话头部「更多」菜单 →「更换项目」；先选 `project2`；再发一个长任务（例如「写一篇 2000 字的文章」），运行中再打开「更多 → 更换项目」；最后对一个已经完成过一轮的 Pi 会话试一次「更换项目」 | ① 取消后输入框下方**仍有**项目 / Agent 托盘；改 Agent 后变成新对话草稿并带上原消息；② 「更换项目」子菜单列出项目（当前项目打勾、不可选）、「选择文件夹…」、「不绑定项目」；③ Fox 会话选 `project2` 后弹「已换到项目「project2」」（显示项目名，不是文件夹路径），侧栏分组跟着变，消息还在；④ 运行中该项灰掉，下方写「对话正在运行，请先停止」；⑤ 外部 Agent 已完成过一轮时，toast「没有更换项目」并说明需要新建对话；⑥ 「重置上下文」等禁用项的原因完整显示在标签下方，没有被截断 | 截图 `R2-4-tray.png`、`R2-4-menu.png`、`R2-4-done.png` |
| B4-11 | **「去设置」入口**：到「设置 → Agent」停用 dsh，打开一个 dsh 会话（没有就先新建一个发一轮再停用）发送，点 toast 上的「去设置」；做完重新启用 dsh | 弹「dsh 已停用 / 该 Agent 已停用，可在设置中启用。」，点「去设置」直接打开「设置 → Agent」。（「去设置模型服务」直达模型服务页已在 B4-5 ③ 验证） | 截图 `B4-11.png` |

回传：B4-5、B4-5b、R2-4、B4-11。

---

## 批次 5：命令环境（约 15 分钟，B5-1、B5-2、B5-3）

**前置**：

1. 打开系统代理（Clash / mihomo / v2rayN 等的「系统代理」开关）。**启动 Fox 的普通 PowerShell 里不要设 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量**，否则会干扰 B5-2 对档位的判断。
2. 普通窗口先跑一次（在复位启动之前或之后都行）：

   ```powershell
   .\Get-FoxProxyInfo.ps1 -OutPath "$R\fox-evidence\B5-proxy.json"
   ```

   看最后一行 `verdict`：
   - `fake-ip mode with a proxy Fox can see`：B5-3 的 fake-ip 步骤可以测；
   - `proxy in use, DNS returns real addresses (not fake-ip)`：代理不是 fake-ip 模式（或没开 TUN / 增强模式），fake-ip 步骤记「未触达」；想测的话把代理软件切到 fake-ip（Clash：DNS 模式 fake-ip + TUN / 增强模式）后再跑一次；
   - `fake-ip DNS but no proxy Fox can see`：DNS 是 fake-ip，但系统代理没开（或只有 PAC），预期 web_read 被拦并显示中文提示；
   - `processEnvironment` 一段应为空（见第 1 条）。
3. 复位，`-Batch 1339-5`。新建 Fox 会话，绑定测试项目。

自动档对「高风险命令」（用 PowerShell 写 `.bat`、删除文件等）仍会出审批卡，这是设计如此，出卡就点「只允许这一次」。

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| B5-1 | 档位**询问**。依次让 Fox：① 执行 `echo 你好，世界 && dir`；② 写文件 `测试.bat`，内容两行 `@echo off`、`echo 参数是 %1`；③ 执行 `测试.bat 参数一`；④ 用 edit 把第二行改成 `echo 第二版 %1` 后再执行一次 `测试.bat 参数一`；⑤ 再写一个 `emoji.bat`，内容 `@echo off` 和 `echo 😀 %1`，执行 `emoji.bat x`。每次写完在普通窗口检查文件：`.\Test-FoxTextFile.ps1 -Path "$R\测试项目\测试.bat","$R\测试项目\emoji.bat" -OutPath "$R\fox-evidence\B5-1-files.json"`（第 ② 步后和第 ⑤ 步后各跑一次，第二次覆盖也可以，备注写两次的结论）。采集 `B5-1` | ① 中文无乱码；③ 输出「参数是 参数一」，退出码 0；`测试.bat` 的检查结果 `guess=gbk`、`bom=none`、`loneLF=0`、`CRLF` ≥ 1、`ACP=936`、`batchOk=True`；④ 改完后编码和换行不变，输出「第二版 参数一」；⑤ `emoji.bat` 为 `guess=utf8`、`chcp65001=True`、`batchOk=True`，执行正常 | `B5-1` 证据、`B5-1-files.json` |
| B5-2 | 依次在三个档位下执行 `set \| findstr /i proxy`：**询问**（出卡批准）、**自动**、**完全访问**（第一次会弹确认框）。最后在自动档执行 `curl -I https://www.baidu.com`。采集 `B5-2` | 询问档**没有**任何 PROXY 变量（findstr 没有输出、退出码 1）；自动档和完全访问档能看到 `HTTP_PROXY` / `HTTPS_PROXY`（大小写各一份）；`curl` 返回 `HTTP/1.1 200 OK`，没有沙箱 / 白名单报错 | `B5-2` 证据、`B5-proxy.json` |
| B5-3 | **a. 四档审批**：在只读、询问、自动、完全访问四档下，各让 Fox「用 web_read 读 https://example.com」。**b. fake-ip 放行**（`verdict` 是 fake-ip 时才做）：自动档 web_read 一个公网网址（`https://example.com`）。**c. 中文拦截提示**：自动档让它「用 web_read 读 http://127.0.0.1:9/」；看卡片提示；打开开发者模式再看一次，再关掉。采集 `B5-3` | a. 只读、询问出审批卡；自动、完全访问直接执行，证据 `permission` 段各有一条自动放行记录。b. 正常读到内容，`permission` 段有一条 `reason=proxy_fake_ip`，summary 以「代理 fake-ip 放行：」开头；不是 fake-ip 模式就记「未触达」并附 `verdict`。c. 卡片提示是中文（例如「网络保护拦截了这个地址」+「这个网址指向内网、本机或保留地址…」），普通模式下没有「web_read blocks …」英文原文；开发者模式下能看到原文 | `B5-3` 证据、`B5-proxy.json`，截图 `B5-3c-normal.png` |

回传：B5-1、B5-2、B5-3（b 未触达时写明 verdict）。

---

## 批次 6：Agent 设置（约 15 分钟，B3-4、B4-12、B4-13）

| 编号 | 步骤 | 期望 | 证据 |
|---|---|---|---|
| B3-4 | **全新数据目录**：`.\Start-FoxBatch.ps1 -Batch 1339-6a -Fresh`（不复制基线）。在「设置 → 模型服务」新建一个供应商并测试连接通过；到「设置 → Agent」**只**打开 Pi；新建 Pi 会话（绑定测试项目）发「回复 ok」；回复后再发一次「回复 ok」。管理员窗口：`$D="$R\data-1339-6a"`，`$L` 指向 `fox-1339-6a*.log`，采集 `B3-4` | 两次都回复 ok，没有登录卡片；证据 `audit` 段有**且只有一条** `agent_backend.pi_source_defaulted_to_fox`，详情含 `reason":"no_source_chosen"`、provider 和 model。（本机有原生 `~/.pi` 配置时，`reason` 可能是 `choice_pending`，备注写明） | `B3-4` 证据 |
| B4-12 | （需要本机原生 pi，没有就「未触达」）接着在 data-1339-6a：「设置 → Agent」把 Pi 的「配置来源」切到「本机 ~/.pi」，发「回复 ok，并告诉我你是什么模型」；再切回「Fox 模型服务」，发同样的话。采集 `B4-12` | 两次都正常回复；切到本机时用的是 `~/.pi` 里配置的模型，切回后用模型服务的模型 | `B4-12` 证据 |
| B4-13 | **「未安装」状态**：关闭 Fox。管理员窗口：`.\New-FoxTestFixture.ps1 -Action Find`，在 `launchers` 里找到 dsh 的启动文件（`…\sandbox-runtime\0.1.8\…\backends\dsh\…\dsh.cmd` 一类），执行 `.\New-FoxTestFixture.ps1 -Action Hide -Path '<那个路径>'`。普通窗口 `.\Start-FoxBatch.ps1 -Batch 1339-6b`，打开「设置 → Agent」，再打开 dsh 的启用开关，新建 dsh 会话发送。关闭 Fox，管理员窗口 `.\New-FoxTestFixture.ps1 -Action RestoreHidden`，再 `.\Start-FoxBatch.ps1 -Batch 1339-6b -KeepProject` 启动，看「设置 → Agent」。采集 `B4-13` | 改名后 dsh 显示「未安装」；发送只弹「dsh 还没安装」加「去设置」，不建 Run；恢复后显示「已检测到」。**一定要执行 RestoreHidden**，否则后面的批次 dsh 都起不来 | `B4-13` 证据，截图 `B4-13-settings.png` |

回传：B3-4、B4-12、B4-13。

---

## 批次 7：UI 评审 01–08 与完全访问确认框（约 15 分钟，U-1～U-8、B4-8）

**前置**：复位，`-Batch 1339-7`。开发者模式关闭。以截图为主，每项 1–2 张，浅色 / 深色各看一眼（「设置 → 通用」切换）。

| 编号 | 对应评审 | 步骤 | 期望 | 证据 |
|---|---|---|---|---|
| U-1 | 08「通用」页 | 打开「设置 → 通用」：改一次个人资料（名字或头像）并保存；切换明暗和配色；改文字大小；关 / 开通知并点「测试通知」；点「打开向导」再关掉 | 每项都能用、立即生效；页面是圆角分组（标题在框外，组内各行在一个圆角框里，行之间有分隔线）；外观选项组和个人资料在框内有内距 | `U-1-light.png`、`U-1-dark.png` |
| U-2 | 08 设置分组 | 逐页看：通用 / 对话 / 模型 / Agent / 插件 / 资料库 / 数据与关于 | 各页分组样式一致；没有空的组框；没有横向溢出；「数据与关于 → 使用情况」只有框外标题，没有双层边框 | `U-2-data.png` |
| U-3 | 07 审批卡 | Fox 会话（询问档）让它连续做三件要审批的事：「依次新建 a1.txt、a2.txt、a3.txt，每个单独写入」。审批卡出现后上下滚动对话；依次批准 | 审批卡在对话流里，跟着对话滚动，输入框保持独立在底部；同一时间只显示一张审批卡，批准后出现下一张；「本项目总是允许」及其说明收在「查看操作详情」里 | `U-3-scroll.png` |
| U-4 | 05 交付文件 chip | 让 Fox「写一份 report.md（几段文字）并用 present_files 交给我」 | 交付文件是扁平 chip：描边类型徽标（无衬线半粗体）+ 文件名 + 紧凑大小（例如 2KB）；没有分组标题和说明文字；悬停显示完整路径；点击能打开 | `U-4-chip.png` |
| U-5 | 04 取消图标 | 发一个长任务后点停止；看这一轮的取消行；把鼠标移到消息上再移开 | 取消行用 CircleStop 图标（圆圈里一个方块），和思维链图标在同一列对齐；消息操作按钮只在悬停或键盘聚焦时出现 | `U-5-cancel.png` |
| U-6 | 01 输入框底栏 | 看已有会话的输入框底栏，悬停项目 / Agent 胶囊 | 底栏高度紧凑（约 32px），胶囊约 28px，悬停高亮四周留白均匀，不贴边 | `U-6-footer.png` |
| U-7 | 02 / 03 菜单 | 新对话里打开 Agent 菜单和项目菜单 | Agent 菜单：名称和说明在同一行，说明是弱化色；项目菜单：外边距只有一层，图标和文字对齐，分隔线上下留白相等 | `U-7-agent.png`、`U-7-project.png` |
| U-8 | 06 资料库 chip | 在输入框里附一个文件，再选一个资料库 | 资料库 chip 显示资料库名称（取不到时显示「资料库」），不显示 UUID；和文件附件的高度、描边、字号、颜色一致；圆角和输入框同心 | `U-8-chips.png` |
| B4-8 | 完全访问确认框（按实际文案） | 在 Fox、Pi、Codex、dsh 会话里各手动选一次完全访问档：先用 ×（或「取消」，如果有）关掉，再选一次并确认；同一项目里再选一次 | 四家都弹确认框；主按钮分别是「确认使用完全访问」（Fox / Pi）、「确认使用完全访问权限」（Codex）、「确认使用完全权限」（dsh）；关掉后档位不变；同一项目第二次选择仍弹框。备注记下：有没有标题、有没有「取消」按钮 | `B4-8-fox.png`、`B4-8-dsh.png` |

回传：U-1～U-8、B4-8。

---

## 批次 8：13.31 剩余项（约 15 分钟，B5-4～B5-8）

**前置**：

1. 复位前（Fox 关闭时）建三个坏技能（普通窗口即可）：

   ```powershell
   .\Start-FoxBatch.ps1 -Batch 1339-8      # 先复制基线并启动一次，然后关掉 Fox
   .\New-FoxTestFixture.ps1 -Action Skills -DataDir "$R\data-1339-8" -OutPath "$R\fox-evidence\B5-4-fixture.json"
   .\Start-FoxBatch.ps1 -Batch 1339-8 -KeepProject
   ```

   `Skills` 会在 `<数据目录>\skills\` 下建 `bad-yaml`（头部 `description: Use when: X`）、`bad-gbk`（中文内容按 GBK 保存）、`num-ver`（`version: 1.0`）。
2. B5-5 需要一个**没有 reasoning 字段**的模型（例如官方 `deepseek-chat`，或网关里任意普通对话模型）。没有就记「未触达」。
3. 新建 Fox 会话，绑定测试项目，档位**自动**。

| 编号 | 13.31 | 步骤 | 期望 | 证据 |
|---|---|---|---|---|
| B5-4 | 8 | 「设置 → 插件 → 技能」里启用三个技能，然后在会话发一条消息 | Fox 正常启动；`bad-yaml` 显示「校验失败」和「SKILL.md 头部 YAML 无法解析：…」；`bad-gbk` 显示「校验失败」和「SKILL.md 不是 UTF-8 编码（例如 GBK），请另存为 UTF-8」；`num-ver` 显示 v1.0；会话正常回复 | 截图 `B5-4-skills.png` |
| B5-5 | 9 | 切到没有 reasoning 字段的模型，发「我先说结论：请用中文分三点介绍一下你自己。」 | 回答完整显示在正文里，没有被折进「思考过程」 | 截图 `B5-5.png` |
| B5-6 | 12 | 让 Fox 两次写 `sub\a.txt`（内容先 v1 后 v2）。关闭 Fox，普通窗口：`.\New-FoxTestFixture.ps1 -Action Junction -ProjectRoot "$R\测试项目"`（把 `sub` 换成指向 `run-1339\outside` 的 junction）。启动 Fox（`-KeepProject`），在「文件版本」里恢复 `sub\a.txt` 的 v1。关闭 Fox，`.\New-FoxTestFixture.ps1 -Action DeleteSub -ProjectRoot "$R\测试项目" -OutPath "$R\fox-evidence\B5-6-fixture.json"`。再启动 Fox（`-KeepProject`），恢复一次。采集 `B5-6` | junction 时拒绝恢复，`DeleteSub` 的输出 `outsideHasAtxt` 为 False（项目外没有出现 `a.txt`）；删掉 `sub` 后恢复成功，项目内重建 `sub\a.txt` | `B5-6` 证据、`B5-6-fixture.json` |
| B5-7 | 14 | 让 Fox 执行 `powershell -NoProfile -Command "1..20000 \| ForEach-Object { '第' + $_ + '行 中文内容' }"`，然后问「第 10000 行是什么？」 | 结果标明省略了多少字节、提示可续读；Fox 用 `fox-result://` 续读，答出「第10000行 中文内容」 | `B5-7` 证据 |
| B5-8 | 15 | 普通窗口：`.\New-FoxTestFixture.ps1 -Action Fsmonitor -ProjectRoot "$R\测试项目"`（测试项目里没有 `.git` 时会先 `git init`，成为独立的小仓库；脚本确认 git 用的是这个小仓库而不是外面的 Fox 仓库，否则拒绝）。让 Fox「用 git_read 看 status 和 diff」，再让它「用 git_read 看 target 为 `--output=<R>\x.txt` 的 diff」。之后 `.\New-FoxTestFixture.ps1 -Action FsmonitorRestore -ProjectRoot "$R\测试项目" -OutPath "$R\fox-evidence\B5-8-fixture.json"`。采集 `B5-8` | `FsmonitorRestore` 输出 `pwnedExists` 为 False（没有生成 `run-1339\pwned.txt`）；以 `-` 开头的 target 被拒绝，`run-1339\x.txt` 不存在 | `B5-8` 证据、`B5-8-fixture.json` |

做完：Fox 关闭后执行 `.\New-FoxTestFixture.ps1 -Action SkillsRemove -DataDir "$R\data-1339-8"`（只影响这一批的数据目录）。

回传：B5-4～B5-8。

---

## 批次 9：向量检索失败与外部 Agent 失败提示（约 15 分钟，B6-2、B6-3）

**前置**：复位，`-Batch 1339-9`。资料库里要有一个**向量索引已就绪**的知识库（没有就先建一个 3～5 个 md 文件的小库，建索引的时间不算在本批里）。B6-2、B6-3 都要临时改名文件，**每一步做完都执行 `RestoreHidden`**。

| 编号 | 来源 | 步骤 | 期望 | 证据 |
|---|---|---|---|---|
| B6-2 | 13.31 第 13 项 | 在「资料库 → 检索测试」做一次混合检索。关闭 Fox，管理员窗口 `.\New-FoxTestFixture.ps1 -Action Find -DataDir $D`，在 `onnxModels` 里找到所选向量模型的 `.onnx`，`-Action Hide -Path '<路径>'`。启动 Fox（`-KeepProject`），再检索一次。关闭 Fox，`-Action RestoreHidden`。采集 `B6-2` | 第一次正常。第二次报「向量检索失败（索引已就绪，不会改用关键词检索）」，**不**返回关键词结果。`onnxModels` 为空（模型不在数据目录下）时记「未触达」并附 Find 的输出 | `B6-2` 证据，截图 `B6-2-error.png` |
| B6-3 | 13.32 E5 | ① 关闭 Fox，管理员窗口 `-Action Find`，在 `launchers` 里找 `codex-acp`，`-Action Hide`；启动后在 Codex 会话发消息，采集 `B6-3a`；关闭，`RestoreHidden`。② `-Action Find` 的 `privateHomeAuth` 里找 Codex 私有 HOME 的 `…\.codex\auth.json`，`-Action Hide`；启动后在 Codex 会话发消息，采集 `B6-3b`；关闭，`RestoreHidden`。③ Codex 会话选完全访问并确认，发消息后在启动过程中（还没开始回复时）把档位改回「请求审批」，采集 `B6-3c`。④ Codex 会话执行 `ping -n 60 127.0.0.1`，批准后点停止，采集 `B6-3d` | ① 报 `agent_backend_not_installed`（或 `spawn_failed`）对应的中文提示并给出解决办法；② 报未登录，提示指向「设置 → 模型服务」（如果模型服务里有可给 Codex 用的地址，Fox 会在启动时重新写入登录，这一步可能照常成功，那就记「通过（自动重新登录）」并写明）；③ 这一轮以「完全访问已撤销」结束，下一轮按撤销后的档位启动；④ 「已取消」，界面不卡，备注带四个耗时 | `B6-3a`～`B6-3d` 证据 |

回传：B6-2、B6-3（四个子步骤写在备注里）。

---

## 批次 10（可选）：13.32 F3–F5、旧库、默认构建（B6-5、B6-6、B6-7）

没有条件的项记「未触达」并写明原因。

| 编号 | 来源 | 步骤 | 期望 |
|---|---|---|---|
| B6-5 | 13.32 F3–F5 | F3：让 Fox 调用内置 Office 被拒绝，看措辞；F4：在 exFAT 卷（U 盘）上的项目里新建文件，正常写入后看项目目录有没有 `.fox-replace-*` 残留，让它读、改一个 GBK 文件；F5：点审批「允许」、设置里资料库「检索测试」时界面是否卡顿 | F3 显示 Host 诊断措辞；F4 exFAT 上新建成功、不覆盖已有文件、无 `.fox-replace-*` 残留，GBK 文件读取正常并标 `GB18030`，编辑时报明确的编码错误、不转码；F5 不卡 |
| B6-6 | 13.31 第 11 项 | 用一份带「运行中」Kernel Run 的旧数据目录（13.30 之前的轮次留下的）启动：`.\Start-FoxBatch.ps1 -Batch 1339-old -BaseDataDir <旧目录>` | 显示「运行中断 · 执行路径已退役」「已中断，可在新 Run 中继续」，不续跑；同会话发新消息正常 |
| B6-7 | 13.32 G | G1：默认 feature 构建、启动、建索引、检索；G2：远程知识库原生索引不可用时检索；G3：重启 Fox 后第一次检索的耗时（毫秒） | G1 正常；G2 返回 `remote_knowledge.index_unavailable`，不改用关键词扫描；G3 记下数字 |

---

## 补充说明

- dsh 的「工作区完全修改」只限制写入，读取工作区外的文件不弹卡，这是预期行为。dsh 在这一档用自带的 Windows 写入沙箱（受限令牌 + Low 完整性）执行命令，`ping` 在里面固定报 `General failure`，长命令用 PowerShell 计数循环。
- 用本机回环地址（127.0.0.1 / localhost）的模型网关时，外部 Agent（Pi / dsh / Codex）在非完全访问档会被 `agent_backend_loopback_requires_full_access` 拒绝，这是既定规则。
- B3-7（SenseNova Messages）仍是已知未修复，本轮不测。13.36「无项目外部会话」（X-1～X-5）不在本轮批次里。
- 新增脚本说明见 `scripts/windows-test/README.md`。全部脚本是 ASCII 文本，PowerShell 5.1 / 7 都能跑。各脚本的测试根目录默认是运行根（由脚本所在目录推出来），`-TestRoot` 仍可覆盖。

---

## 附：给测试机上 dsh 的提示词

测试人员可以让测试机上的 dsh 帮忙按批推进、读证据、写结果。把下面整段发给 dsh（dsh 的工作目录设为**仓库根目录**，例如 `D:\python\projects\Fox\Fox`，档位用「工作区完全修改」）：

```text
你在一台 Windows 测试机上协助我做 Fox 13.39 的实测。全程用简体中文。

位置（<仓库> 是你的工作目录，例如 D:\python\projects\Fox\Fox）：
- 测试说明：<仓库>\docs\windows-test\Fox-13.39-测试说明.md（先完整读一遍）。
- 取证脚本：<仓库>\scripts\windows-test\（说明见同目录 README.md）。脚本直接在这里运行，不复制。
- 运行根：<仓库>\docs\windows-test\run-1339\（git 已忽略）。测试项目、数据目录、logs、截图 shots-1339、证据 fox-evidence 都在这里。
- 结果文件：<仓库>\docs\windows-test\run-1339\results-1339.md。

规则：
1. 按批执行。每次只做我指定的一个批次（例如「做批次 3」）；没指定时，按 1、2、3、4、5、6、7、8、9 的顺序，从结果文件里还没有记录的第一批开始。一批开始前先把这一批的前置条件、准备步骤、要执行的完整命令（绝对路径，并标明在「普通 PowerShell」还是「管理员 PowerShell」里执行）和每一项的步骤列给我，等我确认后再开始。
2. 不要由你启动或关闭 Fox，也不要在你的环境里运行 Initialize-FoxTestEnv.ps1、Start-FoxBatch.ps1、Collect-FoxEvidence.ps1、New-FoxTestFixture.ps1 或任何需要管理员的脚本。所有脚本和界面操作都由我来执行：Fox 由我在普通 PowerShell 里用 Start-FoxBatch.ps1 启动（Medium 完整性），取证由我在管理员 PowerShell 里跑。你把要执行的完整命令写给我，我执行后告诉你，或者你直接读 run-1339\fox-evidence 里新生成的 .txt / .json。
3. 界面操作（点按钮、看提示、截图）由我来做。每一步告诉我要点什么、要看什么、截图存成什么文件名（run-1339\shots-1339\<编号>-<内容>.png），然后等我回报看到的现象。
4. 判定只看说明里的「期望」列和证据，不要放宽也不要加码。证据摘要里重点看：[integrity] 的 ok 必须是 True；tool 行的 status / resultLen / streamedOutput；audit 行（cancel_swept、cancel_sweep_access_denied、pi_source_defaulted_to_fox）；permission 行（proxy_fake_ip、自动放行）；job 行的 state；run 行的 status 和 error；ALIVE 行（取消后不能有 ping / pwsh 残留）；cancel_swept 和 SETTLED 行里的四个耗时。[integrity] 不是 ok=True 时，这一批作废，提醒我从普通 PowerShell 重新启动 Fox。说明里写的步骤或期望和实际界面对不上（按钮名、菜单位置等）时，按最接近的理解判定，并在备注里写「说明偏差：…」。
5. 每做完一项，立刻把结果追加到结果文件，严格用这个格式，一项一行：
   <编号> 通过|失败|未触达  证据：<文件名>[, <文件名>]  备注：<一句话；失败或未触达必填>
   停止类的项在备注里加：耗时 confirm=<ms> grace=<ms> sweep=<ms> settled=<ms> 体感=<秒>。
   文件头缺的值（机器型号、Windows 版本、提交 sha、安装包和运行时 SHA256、Fox 完整性）问我后补上。
   每批开头写一行「批次 N：<标题>」，结尾写一行「批次 N 完成（<失败和未触达的编号>）」。
6. 你只能写 run-1339\ 下的 results-1339.md 和你自己的草稿。不要修改仓库里任何被 git 跟踪的文件，不要 git add / commit / push / checkout / pull；不要修改 Fox 的数据目录、测试项目和脚本。需要改名文件、建目录时，用说明里给的 New-FoxTestFixture.ps1 命令，交给我执行。临时改名过的文件（Hide），这一批结束前提醒我执行 RestoreHidden。
7. 某一项缺少前置条件（例如本机没有原生 pi、代理不是 fake-ip），或卡住超过 5 分钟，记「未触达」并写明原因，继续下一项，不要自行换一种做法。
8. 全部批次做完或我说收工时，提醒我在 <仓库>\scripts\windows-test 里运行 .\Compress-FoxEvidence.ps1，把它打印的缺项和 zip 的 SHA256 写进结果文件末尾的「总结」，总结里列出：失败项、未触达项、取消耗时的典型值、最严重的问题。
```
