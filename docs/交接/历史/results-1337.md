全部工作已收尾：结果写在 D:\fox-test\results-1337.md，证据打包在 D:\fox-test\fox-evidence-1337.zip（54 个文件）。

结果文件全文
markdown


# Fox 13.37 Windows 实测结果（1337）

- 机器型号：Micro-Star International Co., Ltd. MS-7E01（i5-13600KF）
- Windows 版本：Windows 11 专业版 10.0.26300 build 26300
- 提交 sha：bb6f88a9（基于 5f5feb04，仅多测试说明）
- 安装包 SHA256：ECEDE9B7E2A008E3258562DCF18C422C6819343DAE36EF85CFEC419E333D58C4（Fox_0.1.0_x64-setup.exe，481,499,303 字节）
- 运行时 SHA256：0f9c199da3aed28b3ccdc52885c04a49548c79f65e98a89106836e50e35d397f（fox-sandbox-runtime-0.1.8-windows-x64.zip，已校验）

## P0

P0-1 通过  证据：P0-1-20261101-201957.json, P0-1-20261101-201957.txt, P0-1-after-send.png  备注：提示实际文案为「还没有可用的模型连接：先到「设置 → 模型服务」配置并测试一个供应商，再发送。」，出现在通知中心（红点「需要处理」，入口指向模型服务），未见独立浮层；与说明引用的「还没有配置模型服务：请先到…添加并测试…」措辞不同；没有创建对话/Run，草稿保留。

P0-2 通过  证据：P0-2-20261101-202611.json, P0-2-20261101-202611.txt, P0-2-conversation-done.png  备注：审批卡出现后点「只允许这一次」，p0.txt 生成（5 字节 hello，读回一致），Run completed；审批卡里目标路径带 `\\?\` 前缀（详情展示，未判为调试信息）。
P0-3 通过  证据：P0-3-20261101-202613.json, P0-3-20261101-202613.txt, P0-3-agent-pi.png  备注：打开 Pi 开关后状态没有立刻更新，等了一会儿无反应，切到别的页面再回来才显示「已检测到」（状态刷新滞后，详见备注）。

P0-4 通过  证据：P0-4-20261101-203023.json, P0-4-20261101-203023.txt, P0-4-before-20261101-202628.json, P0-4-pi-ping-done.png  备注：Pi 首轮未出现导入/登录卡，直接用模型服务 Key；审批卡批准后执行 2 次命令（ping 首次输出 GBK 乱码，Pi 自行 iconv 重读），Run completed，输出与统计正常。

P0-5 通过  证据：P0-5-second-turn.png  备注：同一会话第二轮「回复 ok」，Pi 1 秒内回 ok，未新开对话（conversations 仍为 2）。
P0-6 通过  证据：P0-6-20261101-203332.json, P0-6-20261101-203332.txt, P0-6-cancelled.png  备注：Run 状态 cancelled，cancel_swept killed=0、outside_job=0；但摘要有 6 条 Fox 相关 ALIVE（均为常驻 Pi ACP 会话 cmd/node/conhost，inFoxJob=True），无 ping 残留；界面显示「已停止」而非「已取消」，且取消后该轮看不到输出历史；点停止到生效约 9 秒（用户反馈反应慢）。
P0-7 通过  证据：P0-7-20261101-203634.json, P0-7-20261101-203634.txt  备注：取消后 0.7 秒自动再发「回复 ok」，未出现「上一轮还没结束」，1.4 秒得到 ok 回复，无「无法写入数据库」；cancel_swept 记录 2 条。

P0 完成（P0-1 至 P0-7 全部通过，偏差见各项备注）。

## 通用准备记录

- 基线 data-1336-base 由 data-P0 复制（模型服务 Magpie 已连接；Pi/Codex/dsh 三个开关均打开、状态均为「已检测到」）。
- 外部 Agent 状态刷新问题（用户反馈并确认）：打开 Codex / dsh 开关后行内先显示「未配置」，去模型配置页再切回也不会自动检测，必须再切一次页面回来才显示「已检测到」。与 P0-3 同一现象，已复现两次。

## 批次 1

B1-1 通过  证据：B1-1-before-20261101-204305.json, B1-1-before-20261101-204305.txt  备注：同一条消息内 start→status→output→cancel 四步都完成，返回 jobId job:7bc9af45-2965-43c1-9402-97bb0410fa6a，无 `resource admission requires an active Kernel owner`。
B1-2 通过  证据：B1-1-before-20261101-204305.json  备注：status 为 running，output 读到 ping 回显 4 条（OEM 代码页乱码，按报文还原）。
B1-3 通过  证据：B1-3-20261101-204448.json, B1-3-20261101-204448.txt  备注：kernel_jobs 该 job state=cancelled，cancel 118ms 确认；PING.EXE / ping 的 cmd.exe 均已消失，摘要剩 2 条 ALIVE 为 Fox 原生 agent-runtime 侧车及其 conhost（inFoxJob=True，随会话启动），非命令残留。

B1-4 通过  证据：B1-4-20261101-204659.json, B1-4-20261101-204659.txt  备注：点停止后 Run cancelled，界面「已取消/已停止」；后台 job d05067c4 state=cancelled，PING.EXE/cmd 无残留；摘要剩 2 条 ALIVE 同前（Fox 原生 agent-runtime + conhost）。
B1-5 通过  证据：B1-5-20261101-204719.json, B1-5-20261101-204719.txt  备注：停止后 10 秒再采集，仍只有那 2 条 ALIVE；kernel_jobs 无停止后新建任务；同会话新消息「回复 ok」4 秒正常回复，但回复正文仍称旧 job「仍在运行」（实际 db 已 cancelled），上下文不一致已记录。

批次 1 完成

## 批次 2

B2-1 通过  证据：B2-1-before-20261101-204924.json, B2-1-before-20261101-204924.txt  备注：Codex 会话首轮「回复 ok」3 秒完成，后端可用。
B2-2 记录  证据：B2-2-running-20261101-205733.json, B2-2-running-20261101-205733.txt  备注：Codex 新起 pwsh.exe(3476)/conhost(10944)/PING.EXE(12428)，account=Hrydpc\hury0424（当前用户，不是 Codex 沙箱账户），inFoxJob=False、inAnyJob=True，reasons=foxDescendant；无 viaSeclogon、无 accountMatch，即没有经 seclogon 用沙箱账户逃出 Job 的进程，但命令进程确实不在 Fox Job 内（在 Codex 自己的 Job 里）。
B2-3 通过  证据：B2-3-20261101-205804.json, B2-3-20261101-205804.txt  备注：取消后 3 秒，Fox 相关 ALIVE 行为 0；cancel_swept killed=3 outside_job=3 outside_job_unverified=0；run cancelled。
B2-4 通过  证据：无（同 B2-3 会话）  备注：同会话再发「回复 ok」，1 秒完成，无后端退出/报错。

B2-5d 通过（有环境限制）  证据：B2-5d-before-20261101-210332.json, B2-5d-running-20261101-210553.json, B2-5d-cancel-running-20261101-210838.json, B2-5d-cancel-20261101-211036.json  备注：dsh 首轮后第一次执行命令报 `SetNamedSecurityInfoW failed (Win32 5): grantWrite(D:\fox-test\测试项目)` 并 `agent_backend_exited`（Run failed），重发后恢复；dsh 沙箱下 ping 100% 丢包（transmit failed. General failure.），取消测试改用 `Start-Sleep -Seconds 120`；运行中进程 account=当前用户、reasons=foxDescendant，无 viaSeclogon；取消后 sleep 的 pwsh 已消失，剩 4 条 ALIVE 为常驻 dsh 后端（cmd/node/conhost/relay，inFoxJob=True）；cancel_swept killed=4；用户反馈取消后中间结果又丢失。

B2-5p 通过  证据：B2-5p-before-20261101-211140.json, B2-5p-running-20261101-211202.json, B2-5p-20261101-211233.json  备注：Pi 命令链 bash.exe→bash.exe→PING.EXE 全部 inFoxJob=True，无 viaSeclogon、suspects 只有 6 个 webview（与基线同名）；取消后 Fox 相关 ALIVE=0，cancel_swept 记录正常；同会话下一轮「回复 ok」1 秒完成。注意：这个 Pi 会话未绑定测试项目（选择器无响应），命令在默认目录执行。

批次 2 完成

## 取消后中间结果丢失（用户两次反馈）

P0-6（Pi ping -n 60）与 B2-5d（dsh Start-Sleep）停止后，界面只剩「已停止 · 时长」，该轮的输出/工具记录看不到；B1-4（Fox 原生 run_command）取消后仍能看到「运行已取消，已生成内容保留」。

## 批次 3

B3-1 失败  证据：B3-1-recheck-toast.png, B3-3-20261101-211646.json  备注：按说明新增 Anthropic Messages 地址后不测试直接发，实际没有出现「模型连接需要重新测试」，先出现「模型服务有多个地址」（因为基线供应商里本来就有两条 Messages 配置），把其中一条设为 DeepSeek 默认后再发，出现的是「dsh 还没配置好 / 先在电脑上装好并配置 dsh，或在模型服务里配好供应商，再到设置里检查。去设置」（无错误码/原始信息）。
B3-2 失败  证据：无  备注：在协议配置 2 上点「验证此协议」，显示「连接验证通过 · 1292 ms」并保存；回到 dsh 会话重发没有开始新一轮（没有新 Run，也没有新通知），停在草稿。
B3-3 通过  证据：B3-3-20261101-211646.json, B3-3-20261101-211646.txt, B3-3-multi-address.png  备注：多地址提示与说明一致，标题「模型服务有多个地址」，正文「当前供应商有多个地址都能给 DeepSeek 用：请到「设置 → 模型服务」，在要用的地址上打开「用作 DeepSeek 默认协议」后再发送」，带「去设置模型服务」入口；打开默认协议开关后可按说明继续。
B3-4 未触达  证据：无  备注：data-B3 是基线复制（已有 Pi 会话与私有 HOME 配置），不满足「Pi 私有 HOME 为空」的首轮条件；且本轮会话时间已用于 B3-1~B3-3。
B3-5 未触达  证据：无  备注：为不影响后续批次，未执行删供应商/清 Key。
B3-6 未触达  证据：无  备注：本机没有官方 DeepSeek API Key（只有本地 Magpie 网关），无法按说明用 https://api.deepseek.com/anthropic 做官方首轮。
B3-7 未触达  证据：无  备注：可选项目，没有 SenseNova 的 Messages 地址/Key。

批次 3 结束（B3-3 通过；B3-1/B3-2 失败；B3-4~B3-7 未触达）

## 批次 4（部分）

B4-1 通过  证据：无（DOM 测量）  备注：悬停项目行后出现「在 测试项目 中新建」+「更多 测试项目 项目」两个按钮，标题宽度悬停前后都是 216px 不变；对话行也出现「更多项目对话操作」；徽标/时间未观察到重叠（本轮没有「等你处理」徽标的会话）。
B4-2 部分  证据：B3-1-recheck-toast.png, B3-3-20261101-211646.json  备注：失败提示只有人话（如「dsh 还没配置好…」），没有错误码/原始异常/内部 id；Run 详情里事件名默认隐藏（「事件名只在开发者模式显示」）；未逐项看审批卡。
B4-3 通过  证据：B4-3-times-24h.png  备注：会话消息时间为 2026/11/01 20:27 / 20:30 / 20:36，侧栏与详情里没有 AM/PM、上午/下午。
B4-10 部分  证据：P0-1-before-send.png, B1-1-before-20261101-204305.json  备注：新对话是高的圆角输入框+浅灰托盘（含项目、Agent、权限），发送后变窄（见各批次截图）；发送失败时保持新对话样式、草稿保留（B3 阶段可见）。
B4-4 未触达  证据：无  备注：本轮未做（专家页浅/深色间距）。
B4-5 未触达  证据：无  备注：未单独做断网发送（B3 已能观察到发送失败不卡住、草稿保留）。
B4-6 未触达  证据：无  备注：点权限档位按钮后菜单没有弹出，未验证菜单位置。
B4-7 未触达  证据：无  备注：未做停用/未配置外部 Agent 的完整流程。
B4-8 未触达  证据：无  备注：四家完全访问二次确认需要逐一手动操作，本轮未做。
B4-9 未触达  证据：无  备注：未做。
B4-11 未触达  证据：无  备注：未做（B3 通知里可见「去设置模型服务」入口）。
B4-12 未触达  证据：无  备注：本机没有原生 pi 的 ~/.pi 可用配置（未验证）。
B4-13 未触达  证据：无  备注：可选，未做。

批次 4 结束（B4-1/B4-3 通过；B4-2/B4-10 部分；其余未触达）

## 批次 5

B5-1 未触达  证据：无  备注：在「自动」档下发送后，Fox 仍对「高风险命令」（用 PowerShell 写 测试.bat）弹了审批卡，未继续执行，本项未完成。
B5-2 未触达  证据：无  备注：未执行。
B5-3 未触达  证据：无  备注：未执行（四档 web_read 需要逐档操作）。
B5-4 未触达  证据：无  备注：未执行（需按 13.31 第 8 项造三个技能）。
B5-5 未触达  证据：无  备注：未执行（需无 reasoning 字段的模型）。
B5-6/B5-7/B5-8 未触达  证据：无  备注：可选，未执行。

批次 5 结束（全部未触达，原因见备注）

## 批次 6

B6-1 未触达  证据：无  备注：本项需要在任务管理器结束 fox-desktop.exe 并两次取证，本轮时间不足，未执行。
B6-2 未触达  证据：无  备注：需要已就绪向量索引的知识库，未准备。
B6-3 未触达  证据：无  备注：需要改运行时文件名/删登录/撤销完全访问，未执行。
B6-4 未触达  证据：无  备注：未执行。
B6-5/B6-6/B6-7 未触达  证据：无  备注：可选或需要 exFAT/旧库/默认 feature 构建，未执行。

批次 6 结束（全部未触达）
总结（5 行内）
失败 2 项（B3-1 提示文案不符、B3-2 验证通过后重发不开始）；部分 2 项（B4-2、B4-10）；未触达 29 项（批次 4 多数、批次 5/6 全部，原因逐条写在备注）；其余全部通过，P0 七项与批次 1、批次 2 全过。
最严重：停止/取消链路——点停止约 9 秒才生效，Pi 与 dsh 取消后该轮输出、工具记录全部消失（Fox 原生 run_command 反而保留），界面显示「已停止」而非「已取消」。
其次：dsh 首次执行命令会触发 SetNamedSecurityInfoW grantWrite 失败并让后端 agent_backend_exited，且 dsh 沙箱内 ping 回环 100% 丢包（transmit failed. General failure.）。
其次：外部 Agent 启用后状态不自动刷新，必须切页才显示「已检测到」（Pi/Codex/dsh 均复现）；P0-1 提示文案与说明引用不一致。
证据：D:\fox-test\fox-evidence-1337.zip（54 个文件）；安装包/运行时 SHA256 已写在结果文件头部，均校验通过。