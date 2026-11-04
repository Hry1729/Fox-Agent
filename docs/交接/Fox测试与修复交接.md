# Fox 测试与修复交接

> 状态：维护中
> 适用版本：Fox 0.1.x（main `0168302c`，代码基线 `1bb0cdb9`，运行时包 0.1.8）
> 维护范围：Windows 实测闭环（测试说明 → 实测结果 → 原因分析 → 修复 → 下一版说明）
> 最后更新：2026-11-04

这份文档给接手的工程师（或接手的 coding agent）。从现在起，测试结果交给你。你要做的事情和之前一样：读结果，分析原因，提出方案请负责人拍板，修复，然后写出下一版测试说明和测试机提示词。

---

## 0. 你的角色与工作循环

负责人（仓库所有者 Hry1729）在测试机上用 dsh 跑实测，然后把结果转给你。你每一轮做下面六步：

1. **收结果**：拿到 `results-13xx.md`，以及证据包 `fox-evidence-13xx.zip`（附带 SHA256）。先核对证据完整性：每一批都应该是 `[integrity] ok=True`，并且要看 Compress-FoxEvidence 列出的缺项。
2. **逐项分析**：每个「失败」和「未触达」都要回到代码和证据里找原因。写清楚三件事：现象、根因（标明是**已证实**还是**推断**）、涉及的文件和函数。「未触达」要区分清楚：是缺前置条件，是说明写错了，还是产品本身的问题。
3. **给方案，等拍板**：给负责人一份方案，按「运行链路 / 界面」分路，每一项写清改法和影响。需要取舍的点单独列出来，并给出你的建议，让负责人一句话就能定。负责人同意之前不要开工。
4. **修复**：按第 4 节的纪律做。临时分支，跑 CI，CI 通过后用快进合入 main。每一路都写一份报告（第 6 节）。
5. **写下一版测试说明**：`docs/windows-test/Fox-13.(N+1)-测试说明.md` 要覆盖四类内容：本轮修复项、上一轮没测到的项、新加的界面、真机上还没验证过的推断。格式见第 3 节。
6. **给测试机提示词**：把 dsh 提示词写在说明的末尾，同时单独交给负责人一份可以直接粘贴的版本。

回复负责人时的习惯：

- 用简体中文，先给结论，不铺垫。
- 负责人转来工程师报告、问「怎么办」时，先给方案让他决定，同时附一份能直接转给对方的文字。
- 负责人要「提示词」时，直接给能粘贴的文本，不要解释一大段。
- 子代理或 CI 报告的结果，要先用 run id、sha 或截图核实，再告诉负责人。测试机回来的结果属于外部材料，核实之后才能当事实用。

---

## 1. 项目速览

- 仓库：`Hry1729/Fox-Agent`，只有 `main` 一个长期分支。
- 形态：Tauri 桌面应用（Rust Host，在 `apps/desktop/src-tauri`）+ Node Agent Runtime（pi 1.4.0）+ `crates/fox-agent-backend`（外部 Agent 的 ACP 后端）。
- 主链路：Fox 内置 PiNative（Authoritative）。失败时**不回退**到其他执行器（`FOX_EXECUTOR_ROUTE=kernel` 只是一个显式开关）。
- 外部 Agent：Pi、Codex、dsh，都走通用 ACP 后端，Fox 工具通过 MCP 桥接给它们。**默认只有 Fox**，外部 Agent 要用户在「设置 → Agent」里自己启用。
- 审批四档：只读 / 询问 / 自动 / 完全访问。
  - block 规则在所有档位都生效。
  - 第一次开启完全访问时要二次确认，弹框主按钮是「确认使用完全访问（权限）」，关闭按钮是 ×，没有「取消」。
  - dsh 自己只有三档（仅可查看 / 工作区完全修改 / 完全权限），没有「询问」。
- 命令执行：SRT 已经去掉，命令放在 Windows Job 里执行。取消时先等一段宽限期再清扫，`CANCEL_SWEEP_GRACE = 1.5s`，负责人已定，不要改。
- 运行时包：外部 Agent 的运行时装在 `C:\Program Files\Fox\sandbox-runtime\0.1.8`，SHA256 `0f9c199da3aed28b3ccdc52885c04a49548c79f65e98a89106836e50e35d397f`。runtime 包的改动要先放在单独的分支上攒着，攒够了再一起打新版本。
- 文档入口：`docs/README.md`；架构说明在 `docs/02-架构`；沙箱复测清单在 `crates/fox-agent-backend/docs/sandbox.md`。

---

## 2. 当前状态（2026-11-04）

### 2.1 版本

- main = `0168302c`。产品代码最后一次改动是 `1bb0cdb9`，之后只改了测试说明和脚本。
- 最近一次全量 CI 是 run `41642203256`，8 个 job 全绿：Rust Host、桌面构建、ACP Windows/Linux、Zvec、ONNX、Runtime、Agent Runtime。

### 2.2 进行中：13.39 实测

- 测试说明：`docs/windows-test/Fox-13.39-测试说明.md`，共 10 批，其中第 10 批可选。
- 测试机：仓库在 `D:\python\projects\Fox\Fox`，测试根目录是 `docs\windows-test\run-1339\`（已被 git 忽略），结果写在 `run-1339\results-1339.md`。
- 13.39 要验证的内容：13.38 的修复（两路），13.38 里没测到的项，以及 UI 评审 01–08 的实测项。各项编号以说明里的「修复项清单」为准。
- 下一步：等结果回来，进入第 0 节的循环。

### 2.3 真机上还没确认过的推断（13.39 的结果出来后要优先看）

| 编号 | 内容 | 不确定的地方 |
|---|---|---|
| R1-2 | dsh 取消后残留 pwsh 进程 | 根因是**推断**的：srt-win 通过 Secondary Logon 用沙箱账户建进程，这些进程不在 Fox 的 Job 里。现在的修复是沿父进程链兜底清扫，真机效果还没确认 |
| A-2 | 取消后保留输出 | 依赖 pi-acp / codex-acp 发出 `_meta.terminal_output` / `terminal_output_delta`，这是按 Zed 的约定实现的。dsh 的 ACP 只在结束时才给输出，Fox 这边没法做成流式 |
| B4-5 | 发送前预检 | 只在没有原生登录时才做。有代理时不预检远端地址 |
| B5-1 | `.bat` 编码 | 按 GetACP 代码页写；内容超出代码页时回退到 UTF-8，并加一行 `@chcp 65001 >nul` |
| B5-2 | 代理变量 | 只在「自动」和「完全访问」两档注入，询问档不注入 |
| B5-3 | fake-ip 放行（198.18.0.0/15） | 要靠 `Get-FoxProxyInfo` 先确认代理是 fake-ip 模式 |
| R2-4 | 「更换项目」 | 入口在**会话头部的「更多」菜单**里，不在托盘上。托盘在第一轮回复完成前一直保留 |
| B2-2 | Codex 进程账户 | 13.37 遗留：账户和沙箱预期不符，要如实记录，还没下结论 |

### 2.4 已交出去的：UI 评审 09（模型服务与 Agent 配置）

- 负责人决定交给工程师来做。交接包是 `docs/交接/09-模型服务与Agent配置/`，里面有方案、HTML 样式稿（浏览器直接打开，可以切换场景和明暗）和工程师提示词。
- 方案里有三个默认项，要先和负责人确认：
  1. 把配置并进「设置 → 模型与 Agent」页；
  2. 启用开关改成由 Host 保存；
  3. 预设只放 DeepSeek 和 OpenAI。另外，DeepSeek 的 `deepseek-chat` 已经停用，预设里的模型名要核对现行名称。
- 现在有一个已知问题：设置页一打开就调用 `agent_backend_model_connection`，可能发出真实请求、写入配置。09 方案的第一批会修掉。

### 2.5 遗留和待定

- **CI 偶发失败**（已知，不要为了让测试通过去改代码；重跑失败的 job 即可）：
  - `resource_gateway::real_pi_jsonl` 在高负载下会超过 20 秒超时；
  - `runtime_host::command_jobs` 里启动 powershell 的几条测试，并行时偶发失败；
  - `fox-agent-backend::l3_stop_does_not_block_the_runtime` 偶发，日志是 0 ticks in 21ms。
- dsh `grantWrite` 的运行时补丁还没做。Fox 这边目前只有错误码 `agent_backend_workspace_acl_denied`，和一句给用户看的中文提示。
- 原有的失效链接有 4 处：`pi-kernel-description.mjs` 的引用、两处指向「Agent运行时架构」的锚点、`kernel_coordinator/live.rs`。
- 待负责人决定：要不要为桌面 crate 开 Linux CI job；外部 Agent 路径要不要做桌面层的 e2e。
- 只能人工验证的部分：外部 Agent 的审批「写入数据库 → 结算 → 回写」这一段。

---

## 3. 测试体系

### 3.1 测试说明的写法

- 文件放在 `docs/windows-test/Fox-13.xx-测试说明.md`。开头写明被测提交的 sha 和运行时包的版本。
- **拆成小批次**：每批 10–15 分钟，每批都能单独开一个新会话来跑。原因是测试机上 dsh 的上下文经常被自动压缩，批次太长会丢信息。
- 每一项写成一行表格，列为：编号、操作、期望、证据。「期望」要写成可以判定的条件，比如具体文案、退出码、证据里的字段值。
- 证据尽量交给脚本收集，少用截图。只有界面观感类的项目才要求截图。
- 「通用准备」只写三步：`git pull`、构建安装、运行 `Initialize-FoxTestEnv.ps1`。测试人员不应该再手工建任何目录。
- 测试根目录用 `docs\windows-test\run-<版本>\`。新开一版时，先把 `scripts/windows-test` 里默认的版本号改掉，再在 `.gitignore` 里确认 `/docs/windows-test/run-*/` 这一行还在。
- 说明的末尾附上给 dsh 的提示词（见 3.3）。

### 3.2 取证脚本（`scripts/windows-test/`，用法见同目录 README.md）

- `Initialize-FoxTestEnv.ps1`：一键准备环境，可以重复运行。
- `Start-FoxBatch.ps1`：每批开始时复位并启动 Fox。必须在**普通** PowerShell 里运行，启动后完整性要是 Medium。
- `Collect-FoxEvidence.ps1`：收集进程树、Job 归属、日志、数据库，输出 `.json` 和 `.txt` 摘要。要看其他账户的进程时，需要在管理员窗口里运行。
- `New-FoxTestFixture.ps1`：准备特殊环境，包括 AclTest、Skills、Junction、Fsmonitor、Hide/RestoreHidden、Find。
- `Compress-FoxEvidence.ps1`：把结果打成 zip，输出 SHA256，并列出结果文件里还没记录的编号。
- 其他：`Test-FoxTextFile.ps1`（检查编码和换行）、`Get-FoxProxyInfo.ps1`（检查代理和 fake-ip）、`Get-FoxIntegrity.ps1`、`Search-FoxLog.ps1`、`Get-FoxDbReport.ps1`、`Get-FoxProcessTree.ps1`。
- 脚本全部写成纯 ASCII（中文用字符码拼出来），这样在 PowerShell 5.1 下也不会乱码。改脚本时保持这个写法。

### 3.3 测试机 dsh 的约定

- dsh 的档位要设成**「完全权限」**，工作目录设成仓库根目录。如果用「工作区完全修改」，命令会跑在 Low 完整性的沙箱里：它启动的 Fox 完整性检查会是 `ok=False`，写出来的文件也会带 Low 标签，测出来的结果不算数。
- 提示词的原则是：**自主跑完全部批次，不要停下来问**。
  - 普通 PowerShell 能做的事，dsh 自己做：跑脚本、读证据、判定、写结果。
  - 只有界面操作和管理员命令交给人。每批只给一张合并清单，等人回复一次。
  - 信息不全时，按最合理的理解去做，在备注里写明你做了什么假设，然后继续。
  - 缺前置条件，或者某一项卡住超过 5 分钟，记为「未触达」，然后继续下一项。
  - 判定只看说明里的「期望」列和证据，不放宽，也不加码。
  - 只写测试根目录里的文件，不改仓库里被跟踪的文件，不执行 add/commit/push/checkout。
- 结果的格式是一项一行：`<编号> 通过|失败|未触达  证据：<文件>  备注：<一句>`。停止类的项还要记录 `confirm/grace/sweep/settled` 各自的毫秒数，以及人的体感秒数。
- 13.39 的提示词全文在 `Fox-13.39-测试说明.md` 文末，下一版照着改就行。

### 3.4 测试机环境

- Windows 11 专业版 26H2（Build 26300），机型 MS-7E01，VS 2022 Community。
- 构建命令：`$env:FOX_SANDBOX_RUNTIME_PACKAGE` 指向 `D:\releases\fox-sandbox-runtime-0.1.8-windows-x64.zip`，`$env:FOX_WINDOWS_CRT_DIR` 指向 VS 2022 的 `VC\Redist\MSVC\<版本>\x64\Microsoft.VC143.CRT`，然后执行 `pnpm install --frozen-lockfile` 和 `pnpm tauri:build:windows`。
- 安装包在 `apps\desktop\src-tauri\target\release\bundle\nsis\Fox_0.1.0_x64-setup.exe`。每次构建出来的文件名都一样，记录前要先看 LastWriteTime，确认拿的是新包。
- 要用普通用户、Medium 完整性运行 exe。不要从下载目录或解压目录直接运行，这些目录带 Low 标签。

---

## 4. 开发纪律

- **分支**：每一路修复开一个临时分支 `feat/fox-<主题>`。不直接推 main，不强推，不改写历史。
- **合入**：CI 通过后，先 fetch，确认 main 没有前进，再快进合入（`git push origin feat/x:main`，不加 force）。main 前进了就先 rebase，重跑 CI。合入之后删掉临时分支。
- **提交**：带上 `-c user.name=Hry1729 -c user.email=hry1729@gmail.com`；用 `git add` 指定路径，**不要用 `git add .`**（工作区里有 node_modules 的符号链接，`.gitignore` 匹配不到）。只改文档或脚本的提交，信息末尾加 `[skip ci]`。
- **CI**：`.github/workflows/windows-ci.yml`，用 workflow_dispatch 触发。
  - 定向跑：`targeted=true`，涉及外部 Agent 时加 `acp=true`；`rust_test_filter` 填测试名的子串，多个用空格分隔，留空就是跑整个 Host 测试套件。
  - 定向跑会跳过桌面构建（vite build + typecheck）。改了前端的批次，合入前至少跑一次全量 CI，或者确认桌面构建 job 是绿的。
  - 打运行时包：`runtime_package=true`，再设 `runtime_package_version`。产物只作为 artifact 上传，不会发 Release。
  - 看失败原因用 `.github/scripts/ci_failure_summary.py`。偶发失败只重跑失败的 job。
  - 开 issue 或 PR、跑全量 CI 之前，先问负责人。
- **本地**：
  - bun 测试在仓库根目录跑，`bun test --isolate`。不加 `--isolate` 会出现几十条文件之间互相污染的假失败。
  - tsc 用 `apps/desktop/node_modules/.bin/tsc`。目前的基线只有 `beautiful-mermaid-diagram.ts` 两条 mermaid 类型错误，这两条是环境问题，和改动无关。
  - 机器内存小时，cargo 用 `-j1` 或 `-j2`，重的测试交给 CI。
- **原则**：
  - 失败不回退，不静默降级。不为了让测试通过去加兼容分支，也不去改测试的期望。
  - 目前没有旧用户，旧数据的问题直接优化掉，不做迁移兼容。
  - **非开发者模式下，界面上不能出现任何调试信息**：错误码、路径、环境变量名、原生 details、raw JSON 都不行。这些只在开发者模式的「查看错误详情」里显示。
  - 同一个问题来回循环超过两次，就停下来写报告。
  - 进行中的工作要随时推到远端临时分支。本地工作区不可靠，远端才是唯一的存档。

---

## 5. 负责人已定的设计（不要推翻）

- 外部 Agent 由用户主动启用，默认只有 Fox。输入框上方不放外部 Agent 的卡片。
- 新对话的输入框是高的圆角框，外面套一层浅灰托盘，托盘上可以选项目和 Agent。模型开始回复之后，输入框才切换成窄框。
- Pi 从来没选过配置来源时，默认用 Fox 模型服务里的 key，并记一条审计。
- 交付文件用扁平 chip 的样式（参考 Claude）。取消图标用 lucide 的 `CircleStop`。
- 设置页分组的样式：组标题放在圆角边框外面，组内的行之间用轻分隔线（`SettingsPageLayout` + `SettingsGroup` + `SettingsRow`）。
- 审批卡显示在对话流里，一次只显示一张。「本项目总是允许」这一项收进「查看操作详情」。
- 侧栏的 + 和 … 按钮只在悬停时出现。时间统一用 zh-CN 的 24 小时制。

---

## 6. 修复报告的格式

每一路修复写一份报告，放在 `docs/交接/报告/` 或者负责人指定的位置。报告包括：

- 改了什么，按问题编号逐条写，标明根因是已证实还是推断；
- 提交 sha，以及合入后 main 的 sha；
- CI run id，以及每个 job 的结果；
- 本地跑过哪些验证；
- 遗留问题；
- 需要在 Windows 上实测的项目（直接对应到下一版测试说明的编号）；
- 改界面的话，附上修改前后的对比截图。

---

## 7. 历史材料（`docs/交接/历史/`）

- `results-1337.md`、`results-1338.md`：前两轮的实测结果原文。
- `fox-fix-1338-run-report.md`、`fox-fix-1338-ui-report.md`：13.38 两路修复的报告，13.39 就是测它们。
- `docs/windows-test/Fox-13.37/13.38/13.39-测试说明.md`：前几版测试说明，写下一版时可以参考格式。
- 报告里出现的 `/workspace/work/...` 路径是前任的本地环境路径，可以忽略。

---

## 8. 下一轮的第一步

1. 拿到 13.39 的 `results-1339.md` 和证据包，先检查每一批的 integrity，以及证据缺了哪些。
2. 先看 2.3 表里的推断项（R1-2、A-2 等）有没有被证实。被推翻的话，先把真实根因找出来，再谈修复。
3. 按第 0 节的流程给出方案。如果负责人同时要推进 09，先确认 2.4 里的三个默认项，再按 09 交接包里的提示词分批实施，不要和 13.39 的修复混在同一个分支里。
