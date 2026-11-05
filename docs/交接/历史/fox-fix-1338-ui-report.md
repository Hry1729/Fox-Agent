# Fox 13.38 界面与工具修复（fix-1338-ui）收尾报告

- 分支：`feat/fox-fix-1338-ui`（原 WIP，基于 dc4a0331，HEAD 5248ef9f）→ rebase 到 main b9730626（run 那一路已合入）后推到新分支 `feat/fox-fix-1338-ui-r2`（没有强推）。
- main 当前 sha：`1bb0cdb96b027a27367b626f50825ca261067611`（2026-11-03 21:2x CST 快进，详见文末「交付 / CI」）。
- 截图：/workspace/work/out/ux-1338-shots/
- mock：/workspace/work/ux1104/harness/ux-mock/fixtures.ts（新增场景见第 6 节）；拍图脚本 /workspace/work/ux1338/shoot1338.py。

## 1. 编译修复（wincheck E0425）

- 根因：`write_precondition` 的参数在 B5-1 里从 `current`（文本）改名成 `current_version`（磁盘字节的版本），但 `None if current.is_none()` 这一行没跟着改。
- 改法：改成 `current_version.is_none()`。另外 `b5_1338_tests` 里用了未导入的 `Uuid`，改成 `uuid::Uuid`。
- 验证：wincheck（windows-gnu `cargo check --lib --tests`）lib 通过；lib test 阶段在本机两次都在全部诊断输出完、没有任何 error 之后被 OOM（SIGKILL）杀掉（7 GB 内存与另一子代理共用）。真正的编译和测试以 Windows 定向 CI 为准（见第 7 节）。

## 2. B4-5 发送前模型地址预检（风险核实）

- 核实结论：上一轮说「只在用模型服务登录时才预检」**不成立**。外部 Agent 路径里只要 `unified_login(backend)` 有值（模型服务里有一个已验证、协议匹配的地址）就会预检，即使这个 Agent 私有 HOME 里已经有用户自己的登录（Pi 原生 / 复用 ~/.pi、dsh 自己的 .env、Codex 自己的 auth）——这时启动时 `unified_login::apply` 返回 `ExistingLogin`，根本不用模型服务地址，却会因为模型服务地址连不上而被拒发。
- 改法（external_backend.rs）：`model_login_context` 返回的 `native_login_present` 留下来，新增纯函数 `reachability_probe_target(native_login_present, unified_base_url)`：Agent 已有自己的原生登录时返回 None（不预检），否则才预检模型服务地址。Fox 原生路径（commands::run_start）预检的就是 Fox 自己的模型服务，不变。
- 测试：`external_backend::tests::b4_5_reachability_probe_only_for_the_model_service_login`；原有 `model_reachability::tests`（本机端口通/不通、DNS 失败、坏地址、代理下远程地址不探测）。
- 界面：toast 正文原来太长，被截成三行省略号（截图里看出来的）。改为「请检查地址，或确认服务已经启动。草稿已保留。」，标题「连不上模型服务」+ 按钮「去设置模型服务」不变；测试加了正文 ≤27 字的断言。地址和错误码只在开发者模式出现。

## 3. R2-4 第一轮未完成保留托盘 + 「更多 → 更换项目」

- 根因（上一轮已改）：托盘是否收起看的是「模型是否回复过」，第一轮被取消/失败也算回复过，于是托盘消失、改不了项目和 Agent；会话头部菜单也没有入口。
- 改法：`conversationFirstReplyCompleted`（有一轮 completed 才算）；后端 `conversation_change_project`（原对话里换项目，清完全访问，执行策略版本 +1）。本轮把两条校验抽成纯函数 `change_project_refusal(run_active, backend, external_started)`：运行中 → `conversation.run_active`（可重试）；外部 Agent 已有完成的一轮或已有原生会话 → `conversation.project_locked`；Fox 会话空闲时随时可换。
- 本轮界面修正：换项目成功的 toast 用项目名（原来取文件夹名，显示成「thesis」）；菜单里禁用项的原因原来挤在同一行，把「重置上下文」挤成两行、原因被截断，改为原因换到标签下方完整显示（workbench.css `.fox-menu-hint`）。
- 测试：`commands::r2_4_change_project_tests`（3 条）、`database::...::a_conversation_can_change_its_project_in_place`、前端 `composer-new-conversation-layout-dom.test.tsx`（R2-4 三条）、`fix-1338-ui.test.tsx`。

## 4. B5-1 批处理编码 / B5-2 代理变量 / B5-3 web_read

- B5-1：script_encoding.rs（CRLF + GetACP 代码页，编码不了回退 UTF-8 并加 chcp 65001），EditFile.content 改 Vec<u8>，版本按磁盘字节。测试：`script_encoding::tests`（7 条，Linux 也跑）、`tool_host::b5_1338_tests`（Windows 上真写「测试.bat」再执行「测试.bat 参数一」）。
- B5-2：本轮把「哪些档位带代理」抽到 `system_proxy::command_environment_for_mode(mode, settings)`，runtime_host 只调用它。测试 `system_proxy::tests::proxy_reaches_commands_only_in_auto_and_full_access`（只读/询问连检测都不做）。
- B5-3：web_read/http_request 有代理时放行 198.18.0.0/15 并写审计 `proxy_fake_ip`；错误改中文、原文只在开发者模式。rebase 时和 run 那一路的 `outputView` 合并：保留 run 的「定格卡片 + 部分输出」，在失败分支里加回开发者模式的「原文」。测试：`capability_tools::tests::proxy_fake_ip_*`、`fix-1338-ui.test.tsx`。

## 5. 本地测试

- bun 全量（仓库根目录，`--isolate`，同 CI，排除 a0-performance）：rebase 前 950 pass / 2 skip / 0 fail（952 个，129 个文件）。rebase 后结果见第 7 节。
- 相关 bun：fix-1338-ui / composer-new-conversation-layout-dom / send-failure-notice 21 pass。
- tsc：只剩 beautiful-mermaid-diagram.ts 的两条（原有）。

## 6. 截图（/workspace/work/out/ux-1338-shots/，全部打开看过）

| 文件 | 内容 | 检查 |
|---|---|---|
| r24-tray-first-cancelled.png / -dark | 第一轮被取消的 Pi 会话：输入框下有项目「Fox-Agent」和 Agent「Pi」托盘 | 无截断/重叠，普通模式无调试信息 |
| r24-tray-completed-locked.png | 第一轮已完成的会话：窄输入框，没有托盘 | 同上 |
| r24-more-change-project.png / -dark | 会话头部「更多 → 更换项目」子菜单：当前项目打勾不可选、另一项目、选择文件夹…、不绑定项目；「重置上下文」的禁用原因在下方完整显示 | 同上 |
| r24-change-project-done.png | 选了「毕业设计论文与答辩材料」后：托盘、侧栏分组都换了，toast「已换到项目「毕业设计论文与答辩材料」」 | 同上（「更多」按钮的 tooltip 是菜单关闭后焦点回到按钮的正常行为） |
| b45-toast-unreachable.png / -dark | run_start 返回 model.connection_unreachable：toast「连不上模型服务」+「去设置模型服务」，草稿「回复 ok」保留，托盘还在 | 正文完整无省略号，无地址/错误码 |
| b45-toast-unreachable-dev.png | 开发者模式：正文带原文（错误码和地址），toast 里被省略号截断——开发者模式原有表现，普通模式不受影响 | 仅作对照 |

mock 新增：`scene=firstcancel`（第一轮被取消的 Pi 会话）、`?unreachable`（conversation_create 成功、run_start 返回 model.connection_unreachable）、`conversation_change_project` handler（`?locked` 时返回 conversation.project_locked）；`conversations_list` 改为每次返回新对象（原来返回同一个数组引用，React 不重渲染，侧栏不更新）。

## 7. CI

见文末「交付 / CI」。

## 8. 13.39 Windows 实测步骤与预期

1. 编译与安装：用 main 新 sha 打包安装，按 13.38 说明准备测试项目 D:\fox-test\测试项目。
2. B4-5（Fox）：设置 → 模型服务把地址改成 http://127.0.0.1:9/v1，保存不测试，新对话发「回复 ok」。预期：1–3 秒内弹「连不上模型服务 / 请检查地址，或确认服务已经启动。草稿已保留。」带「去设置模型服务」，点按钮直达模型服务；不出现新 Run，草稿还在；普通模式不显示地址和错误码，开发者模式显示 model.connection_unreachable 和地址。
3. B4-5（外部 Agent 原生配置不预检）：Pi 配置来源选本机 ~/.pi（或 dsh 私有 HOME 里放自己的 .env），模型服务地址仍是 127.0.0.1:9。预期：Pi/dsh 照常发送、用它自己的配置回复，不弹「连不上模型服务」。再把 Pi 切回「Fox 模型服务」，预期弹 toast。
4. B4-5（代理）：开系统代理时模型服务填一个公网域名（解析不了也行）。预期：不预检、照常建 Run（由 Run 自己报错）。
5. R2-4 托盘：Pi 会话发 ping -n 60 127.0.0.1，批准后点停止。预期：取消后输入框下方回到项目/Agent 托盘，能换项目和 Agent；换 Agent 会变回草稿并带上原消息。再发一轮让它正常完成，预期托盘收起。
6. R2-4 更换项目：会话头部「更多 → 更换项目」。预期：子菜单列出项目（当前项目打勾不可选）、「选择文件夹…」、「不绑定项目」；Fox 会话选另一项目后 toast「已换到项目「…」」，侧栏分组跟着变，消息不丢；运行中该项灰掉并在下方写「对话正在运行，请先停止」；外部 Agent 已有完成的一轮后再换，预期 toast「没有更换项目 / 这个 Agent 的对话已经开始…请新建一个对话」。
7. B5-1：Fox 会话（询问档）让它写「测试.bat」（内容 `@echo off` / `echo 参数是 %1`）再运行「测试.bat 参数一」。预期：输出「参数是 参数一」，文件是 CRLF、GBK（936）无 BOM；再让它 edit 这个文件，编码和换行不变。写入含 emoji 的 .bat，预期文件 UTF-8 且第一行 `chcp 65001 >nul`。
8. B5-2：开 Clash 等系统代理，自动档运行 `set | findstr /i proxy`。预期：看到 HTTP_PROXY/HTTPS_PROXY（大小写两份）；询问/只读档看不到。
9. B5-3：开代理的 fake-ip 模式，web_read 一个公网网址。预期：正常读到内容，审计里有 proxy_fake_ip 一条；关掉代理后同一网址被拦，提示是中文句子，普通模式没有英文原文，开发者模式「原文」可见。

## 交付 / CI

### 结果
- **main = `1bb0cdb96b027a27367b626f50825ca261067611`**：由 b9730626 用 `git_ref_update force=false` 快进而来，没有强推。
- 合入的分支是 `feat/fox-fix-1338-ui-r2`：8d651680 之上加了一个提交 1bb0cdb9（只改 bun 测试，见下）。
- 没有建 r3。原因是 command_jobs 那条失败查下来是偶发，不需要改代码。
- 已删除远端分支 `feat/fox-fix-1338-ui`、`feat/fox-fix-1338-ui-r2`。
- 远端另外几个 1338/ui 分支（fox-doc-1338、fox-fix-1338-run、fox-ui-1104、fox-ui-1337、fox-ui-1337-r）在 fetch --prune 时已经不在了，不是本轮删的。

### command_jobs 失败的根因结论：偶发，与 ui 分支改动无关
- 失败用例：`runtime_host::command_jobs::tests::production_adapter_start_nonzero_and_utf8_byte_paging_are_durable`。
- 出现在 run 41636193757 第 1 次 attempt，job 125917915207。
- 现象：powershell 子进程启动后 0.87 秒内写过一次 checkpoint，之后 20 秒没有任何输出，也没有退出。job 一直停在 Running，stdout、stderr 都没关闭。
- supervisor 线程一直活着：线程如果出错，会走 stop_failed，状态会写成 Failed，而这里没有。
- 所以是子进程本身慢或卡住。当时同一个测试二进制里有 173 条用例并行，很多都在起子进程。

逐条排查：
1. **command_environment_for_mode**：这条用例的 fixture 建会话时用的是 `Some("ask")`（询问档）。
   - `command_environment_for_mode` 在只读和询问档直接返回空，连 `detect()` 都不调用。已有测试 `system_proxy::tests::proxy_reaches_commands_only_in_auto_and_full_access` 用一个 panic 的 detect 证明了这一点。
   - 所以 command_jobs.rs 里 `proxy_environment.is_empty()` 成立，`spawn_spec.env` 保持 `EnvMode::Inherit`，和 main 上完全一样。
   - 即使在自动或完全访问档，`InheritWith` 也是先 `apply_inherited`（允许列表，含 PATH、SystemRoot、COMSPEC），再追加代理变量，不会清空或覆盖这三个变量。
2. **WinINET 和注册表读取**：询问档根本不走 `detect()`，后台命令启动路径上没有注册表读取。
3. **with_live_output 线程局部接收器**：这是 tool_host 同步 run_command 那一路的。后台 job 走的是 `ProcessJobManager` 加独立的 `fox-command-job` 线程，不经过 live output 接收器。
4. **复跑**：用的是同一 sha 8d651680，job 125923391921。这条用例通过了，失败的只剩 resource_gateway 的 real_pi_jsonl 超时，见下。后面两次完整定向 CI 也都通过了。

### CI run
| run | sha | 内容 | 结果 |
|---|---|---|---|
| 41630366516 | rebase 前 | 定向 | command_jobs 通过（用户提供） |
| 41636193757 attempt 1（job 125917915207） | 8d651680 | 定向 + ACP | Rust Host 1 条失败：command_jobs 偶发（上面说的那条） |
| 41636193757 attempt 2（job 125923391921） | 8d651680 | 只重跑 Rust Host | command_jobs 通过；`resource_gateway_tests::real_pi_jsonl_reads_through_rust_and_returns_the_persisted_tool_result` 报 `Pi JSONL response deadline: Timeout`（老问题，20 秒偶发超时） |
| 41639491422 | 1bb0cdb9 | 定向 `real_pi_jsonl resource_gateway` | ✅ 全绿 |
| 41638894029 | 1bb0cdb9 | 定向 + ACP，过滤条件同上次 | ✅ 全绿（第 1 次） |
| 41640666683 attempt 1 | 1bb0cdb9 | 定向 + ACP，同上 | Rust Host ✅、ACP Windows ✅、Runtime ✅；ACP Linux 1 条失败：`external_runtime::l3_stop_does_not_block_the_runtime`，报「runtime starved during stop: 0 ticks in 21.3ms」 |
| 41640666683 attempt 2 | 1bb0cdb9 | 重跑 | ✅ 全绿（Rust Host 连续第 2 次绿，command_jobs 稳定） |
| 41642203256 | 1bb0cdb9 | main push 全量 CI | 快进后自动触发。写报告时：「Test desktop」（bun 全量）✅、ACP Linux ✅、Runtime package ✅；Rust Host 全量、ACP Windows、Zvec、ONNX、Desktop build/typecheck 还在跑 |

两次 Rust Host 定向 CI 用的过滤条件都是：`model_reachability external_backend:: r2_4_change_project a_conversation_can_change_its_project_in_place script_encoding system_proxy b5_1338 proxy_fake_ip e2e1101_send_refusal tool_host:: runtime_host::capability_tools resource_gateway command_jobs`。

### bun：turn-process-header-dom.test.mjs 的 2 条失败
- 结论：**测试断言过时，实现没有回归**。
- run 那一路在 `fox-runtime-assistant-message.tsx` 加了 `turnDefaultOpen`：取消的这一轮默认展开过程，工具卡定格为「已取消」，并保留取消前的输出。这是 A-2 的设计。
  - 状态词仍是「过程已取消」。
  - 失败和中断仍默认折叠。
- 两条测试还按「默认折叠」去找「展开过程」按钮，所以找不到。
- 改法（提交 1bb0cdb9，只改 `apps/desktop/tests/turn-process-header-dom.test.mjs`）：
  - 取消的这一轮断言开关一开始就是「收起过程 · … · 过程已取消」、aria-expanded=true、过程没有隐藏，点一下以后能折叠；
  - 失败和中断的用例不变。
- 本地（仓库根目录 `bun test --isolate`）：
  - 这个文件 15 pass / 0 fail；
  - 全量 apps/desktop/tests（排除 a0-performance，同 CI；130 个文件分 5 批跑，因为单条命令有 120 秒限制）：961 pass / 2 skip / **0 fail**。日志在 /workspace/work/ux1338/bun-c0{0..4}.log。

### 遗留问题
1. `resource_gateway_tests::real_pi_jsonl_reads_through_rust_and_returns_the_persisted_tool_result`：20 秒的 JSONL 响应期限在 CI 负载高时偶尔超时（41636193757 attempt 2）。单独跑是绿的（41639491422）。没有改。
2. `fox-agent-backend` 的 `external_runtime::l3_stop_does_not_block_the_runtime`：停止只用了 21 毫秒时，期望 tick 数是 21/20=1，但 ticker 可能一次都还没跑，所以这个断言在边界上本身就会偶发失败。和本分支无关，本分支没有改 crates/；同一 sha 的另外两次都是绿的。建议以后给断言加一个最短时长门槛（比如 elapsed < 100ms 时跳过比较）。
3. command_jobs 里起 powershell 的几条用例在并行负载下可能超过 20 秒（这次出现过一次）。如果再出现，可以考虑把 `wait_terminal` 的期限放宽，或者让这组用例串行。本轮按「不为了让测试通过而改」的原则没有动。
4. main 的全量 CI 41642203256 在写报告时还没跑完。上一个 main 全量 41634725138（b9730626）只有「Desktop build + profile isolation」这个 job 失败（就是那 2 条 bun）。这次同一个 job 的「Test desktop」步骤已经通过；Rust Host 全量等其余 job 的最终结果还要再看一眼。
5. 第 8 节的 13.39 Windows 实测项全部还没做。
