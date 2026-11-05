Fox 13.38 Windows 实测结果（1338）
机器型号：Micro-Star International Co., Ltd. MS-7E01（i5-13600KF）
Windows 版本：Windows 11 专业版 10.0.26300 build 26300
提交 sha：32df063d（Fox-Agent，docs/windows-test/Fox-13.38-测试说明.md）
安装包 SHA256：A9585F7C5678BD64238BF9AF795584E2140A8BE3089AA4481A654B9BAA9DDF9D（Fox_0.1.0_x64-setup.exe，481,543,027 字节）
运行时 SHA256：0f9c199da3aed28b3ccdc52885c04a49548c79f65e98a89106836e50e35d397f（fox-sandbox-runtime-0.1.8-windows-x64.zip，已校验一致）
批次 1：停止与取消
R1-1 失败 证据：R1-1-before-20261102-172618.json, R1-1-20261102-173032.json, R1-1-cancelled.png 备注：Pi 会话（绑定测试项目、询问档）ping -n 60 批准后运行；点停止后约 0.7 秒界面即结束，未捕捉到「正在取消」中间态，最终显示「已取消」（无「已停止」字样）；耗时 confirm=669 grace=1506 sweep=8 settled=666，体感 <1 秒到「已取消」；取消后该轮只剩「1 项未完成 · 过程已取消 / 已取消」，执行命令的工具卡与任何 ping 输出都看不到（db 该 run 仅 48 条 tool.updated status=in_progress，无 output 内容），A-2 不满足；摘要 ALIVE 6 条均为 Pi ACP 常驻会话进程（cmd/node/conhost/pi-gate，会话启动时创建，inFoxJob=True），无 ping 残留。

R1-2 失败 证据：R1-2-before-20261102-174407.json, R1-2-20261102-175102.json 备注：dsh 会话档位没有「询问」，只有「仅可查看/工作区完全修改/完全权限」（与说明不符，记录）；A-6 通过：计数循环经 dsh 后台作业分两次取回 1–119 与 120，数字连续；取消耗时 confirm=7 grace=1502 sweep=24 settled=6，killed=2 failed=1（TerminateProcess(32040) 失败 (Win32 5)（pwsh.exe，不在 Job 中）），取消后 1..600 的 pwsh.exe 仍存活（inFoxJob=False），摘要 7 条 ALIVE（含残留 pwsh 与常驻 dsh/relay 进程）；界面只剩「已取消」，命令与已输出数字不可见（A-2 失败）。

R1-3 失败 证据：R1-3-20261102-181437.json, R1-3-run-failed.png 备注：A-8 通过，第一条回复的 output 中文正常（「正在 Ping 127.0.0.1 具有 32 字节的数据:」「来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128」）；但两次运行都以「运行中断/运行失败」结束，界面出现英文内部报错「Pi released its Run before a terminal result was recorded.」，db error=runtime.released_without_terminal 与 provider.request_failed（provider 连接中断），未能做到「查询过程中点停止」，也无法验证停止后显示「已取消」。

R1-4 失败 证据：R1-3-20261102-181437.json 备注：同一会话追问旧后台任务状态，回复称「但这只是抽样结果，不能作为该任务已结束的证据，所以我不会据此下结论」，未说明已取消/已结束；证据 db 里两条 jobs 均为 state=cancelled，界面表述与数据库不一致。

R1-5 失败 证据：R1-5-20261102-181918.json 备注：Codex 会话 ping -n 60 批准后运行，点停止后界面显示「过程已取消 / 已取消 / 运行已取消，已生成内容保留」，无「已停止」；耗时 confirm=16 grace=1515 sweep=31 settled=15，killed=4 outside_job=3 failed=0；但取消后同样只剩「过程已取消」，命令与输出不可见（A-2 失败）；摘要 ALIVE 6 条均为 Codex ACP 常驻后端（会话启动时创建，inFoxJob=True），无 ping 残留。

批次 1 完成（A-3：本轮全程未出现「已停止」；「正在取消」中间态未捕捉到，四次取消的 cancel_settled_ms 分别为 666/6/6/15ms，界面在 1 秒内即已结束。A-2 在 Pi/dsh/Codex 三种后端均失败：取消后工具卡与输出全部消失。）

批次 2：dsh 授权、审批卡与界面修复
R2-1 未触达 证据：无 备注：需要管理员 PowerShell 把 D:\fox-test\acl-test 的所有者改为 Administrators（icacls /setowner 需 SeRestorePrivilege），本次会话是 Medium 完整性、无法提权，造不出「当前用户不是所有者」的目录；dsh 首轮在本机也没有再报 grantWrite，故 A-5 未触达。 R2-2 通过 证据：R2-2-approval-path.png, R2-2-20261102-190657.json 备注：审批卡目标路径显示为 D:\fox-test\测试项目\p.txt，没有 \\?\ 前缀；「只允许这一次」后 p.txt 生成（5 字节 hello），run 7141902c status=completed。 R2-3 通过 证据：R2-3-agent-detect.png 备注：设置 → Agent 里把 dsh 关掉再打开，约 1.7 秒内直接变成「已检测到」，不需要切页；切到「模型」再回「Agent」、最小化再恢复窗口后同样保持「已检测到」并重新检测；未观察到「检测中」中间态。 R2-4 失败 证据：R2-2-20261102-190657.json 备注：已取消的 Pi 会话（有消息、这一轮没有回复）打开后，输入框下方只有「询问」和发送键，没有项目/Agent 托盘；会话头部「更多」菜单只有置顶/重命名/重置上下文/归档，找不到修改项目或 Agent 的入口。 R2-5 失败 证据：R2-5-toast.png 备注：全新数据目录 data-R2-empty 下连发两次「回复 ok」，草稿保留、通知计数 1→2，通知中心出现「还没有配置模型服务 / 模型服务」条目，但没有截到浮层 toast，正文与「去设置模型服务」按钮未验证；dsh 停用后连发两次未做。

批次 2 完成（R2-1 未触达，R2-4、R2-5 失败）

批次 3：模型服务、Pi 复用 Key、dsh 首轮
B3-1 通过 证据：B3-1-toast.png, B3-1-20261102-183114.json 备注：删掉基线里 Magpie 已有的 Anthropic Messages 协议后新增一个地址（http://127.0.0.1:3425/v1）保存且不测试；dsh 会话发「回复 ok」弹 toast，标题「模型连接需要重新测试」，正文「模型服务的配置改过了，当前地址还没有通过连接测试：请到「设置 → 模型服务」重新测试连接，通过后再发送。」，带「去设置模型服务」且点击直达模型服务页；普通模式下无错误码。 B3-2 通过 备注：「验证此协议」通过（1070 ms）后重发正常开始新一轮并 5 秒回 ok；不改内容再保存一次后仍能直接发送。 B3-3 通过 备注：再加一个 Messages 地址并验证通过后发送，提示「模型服务有多个地址」+ 正文原文；给其中一个打开「用作 DeepSeek 默认协议」后重发正常（5 秒回 ok）。 B3-4 通过 证据：B3-4-20261102-185411.json, B3-4-setup.png 备注：全新 data-B3-4，新建 Magpie 并「连接正常」，只启用 Pi；Pi 首轮 2 秒回 ok，无登录卡片；db run status=completed。偏差：未见 agent_backend.pi_source_defaulted_to_fox 审计。 B3-5 通过 证据：B3-5a-20261102-185648.json 备注：删掉唯一供应商后发送，发送前即拒绝（无新 Run），toast「pi 还没配置好 / 先在电脑上装好并配置 pi，或在模型服务里配好供应商，再到设置里检查。」+「去设置」；无旧文案。子项「只清空 Key」未触达（清空输入框保存后仍能发送）。 B3-6 通过 证据：B3-6-20261102-190442.json 备注：无官方 Key，用备选网关 Magpie 的 Anthropic Messages 地址；dsh 首轮 5 秒成功，run completed。

批次 3 完成

批次 4：UI 复测与完全访问二次确认
B4-2 失败 证据：R1-3-run-failed.png, B4-5-send-fail.png 备注：改坏模型地址后的失败卡片是中文「处理失败，可重试或查看问题诊断 / 运行过程中遇到了问题。你可以重试，或在设置中查看运行诊断。」，无错误码；但 R1-3 的失败 Run 在普通模式界面直接出现英文内部报错「Pi released its Run before a terminal result was recorded.」，失败提示并不干净。 B4-4 通过 证据：B4-4-experts-light.png, B4-4-experts-dark.png 备注：浅色/深色专家页排版一致，头像与标题约 8px 间距，说明文字下一行铺满。 B4-5 失败 证据：B4-5-send-fail.png 备注：把 Magpie 服务地址改成 http://127.0.0.1:9/v1 后在已有会话发送，没有停在「等待开始」，但也没有 toast 和「去设置模型服务」，而是直接建 Run 并 1 秒内以「处理失败 / 运行失败」结束，草稿没有保留在输入框。 B4-7 通过 证据：B4-7-disabled-toast.png 备注：停用 Pi 后打开已有 Pi 会话发送，弹「pi 已停用 / 该 Agent 已停用，可在设置中启用。」带「去设置」，页面保持、草稿保留、不建 Run；两个子项未做。 B4-8 通过（有偏差） 证据：B4-8-fullaccess-confirm.png, B4-8-dsh-dialog.png 备注：Fox/Pi/Codex/dsh 四家都弹确认框；取消或关闭后档位不变；同一项目换回普通档再选仍弹框。偏差：无「确认使用「…」？」标题，按钮是「确认使用完全访问（权限）」+「关闭弹窗」，不是两字缩写，Codex/dsh 没有「取消」。 B4-9 通过 备注：完全访问确认后切到 Pi 变「询问」（不沿用）；「自动」切到 Pi 沿用「自动」。 B4-10 通过 证据：B4-10-draft.png, B4-10-replying.png 备注：草稿是高的圆角输入框+浅灰托盘（项目/Agent/权限）；开始回复后变窄输入框、托盘消失；失败时草稿仍在。 B4-11 未触达 证据：B3-1-toast.png 备注：「去设置模型服务」已点并直达模型服务页；「去设置」按钮出现过但点击未观察到跳转。 B4-12 未触达 备注：未切换 Pi 配置来源。 B4-13 未触达 备注：未临时改名 dsh 可执行文件。

批次 4 完成（B4-2、B4-5 失败，B4-11 未触达，B4-7 部分完成）

批次 5：13.31 剩余项（一）
B5-1 失败 证据：B5-1-bat.png 备注：第 1、2、4 步正常，第 3 步「执行 测试.bat 参数一」失败——Fox 写出的 测试.bat 是 UTF-8 + LF（34 字节，CR=0、LF=2、无 BOM），cmd 在代码页 936 下把中文 UTF-8 字节当 GBK 解码并错行，报「不是内部或外部命令」，%1 没有展开成「参数一」，退出码非 0。 B5-2 通过（部分） 备注：curl -I https://www.baidu.com 返回 HTTP/1.1 200 OK，无 SRT/白名单报错；set HTTPS_PROXY 返回「环境变量 HTTPS_PROXY 没有定义」（退出码 1），代理变量没有带进命令。 B5-3 未触达 备注：web_read 读 https://example.com 被网络保护拦截，界面出现英文内部说明「web_read blocks private, local, reserved, and link-local addresses」，四档位与 permissionAudit 无法测。 B5-4 未触达 备注：需按 13.31 第 8 项手工建 bad-yaml、bad-gbk、num-ver 三个技能目录，未做。 B5-5 未触达 备注：需没有 reasoning 字段的模型，本地只有 opendesign/deepseek-v4.1-flash。 B5-6 / B5-7 / B5-8 未触达 备注：可选，未做（junction 恢复、20000 行续读、恶意 fsmonitor）。

批次 5 完成（B5-1 失败，B5-2 部分通过，B5-3～B5-8 未触达）

批次 6：13.31 剩余项（二）与 13.32 E5 / F / G
B6-1 通过 证据：B6-1-before-20261102-201816.json, B6-1-20261102-202253.json, B6-1b-20261102-202144.json 备注：两次结束 fox-desktop.exe 后 5 秒采集，均为「Fox hosts 0、Fox Job members 0、suspects 0、Fox-related still alive 0」，无任何 ALIVE 行；第二次是在执行 ping -t 127.0.0.1 期间结束进程（PING.EXE 已在跑），ping/cmd/node 全部消失。偏差：第一次场景 run 在结束进程前已完成；本批没有 search_knowledge（无就绪索引），「重启后检索正常」未验证。 B6-2 未触达 备注：需先把向量模型文件改名制造检索失败，未做。 B6-3 未触达 备注：需依次改名 codex-acp、删 Codex 私有 HOME 登录、撤销完全访问、停止运行，未做。 B6-4 通过（部分） 备注：Run 状态从「正在处理/正在调用工具」到「已完成 / 已取消 / 运行失败」刷新正确，时长计数正常累加、结束后不再变；「文件版本」面板未打开验证。 B6-5 / B6-6 / B6-7 未触达 备注：可选，未做（Office/exFAT/GBK/卡顿、旧库 running Run、feature 构建与冷启动耗时）。

批次 6 完成（B6-1 通过、B6-4 部分通过，其余未触达）

总结

本轮补完批次 4 剩余、批次 5、批次 6：新增失败 3 项（B4-2、B4-5、B5-1），累计失败 10 项、未触达 15 项；B6-1（结束进程后无残留）通过。
最严重仍是取消/中断链路：取消后过程与输出全丢（A-2 在 Pi/dsh/Codex 全失败）、dsh 取消残留 pwsh（Win32 5）、Fox 原生 Run 反复以英文内部报错「Pi released its Run before a terminal result was recorded.」中断。
新问题：B5-1 写出的 .bat 为 UTF-8+LF，cmd 中文与 %1 全错；B5-2 代理变量没带进命令；B5-3 的 web_read 拦截说明是英文原文直接显示在界面。
停止四段耗时典型值：confirm 6–27 ms（Pi 669）、grace ≈1500 ms、sweep 8–31 ms、settled 6–15 ms（Pi 666），界面均 1 秒内结束。
交付：results-1338.md、fox-evidence-1338.zip（102 个文件，4.25 MB，SHA256 31B0D6B6…162BE）。