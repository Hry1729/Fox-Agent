# Fox N1–N7 第二次复审（2026-09-15）

## 0. 结论与证据边界

**暂不通过整体验收。** 本轮有实质进展：N1 的生产 Office wrapper 识别、N3 的 steering 压缩前预算、N4 的历史保留、N6 的外部内容版本、N7 的技能发现均有对应实现。仍有 **5 项需要处理的代码／验收链路问题**，其中 N5 仍可能使任务直接失败；不能全部归因于缺少云模型、GUI 或 OfficeCLI 环境。

审核对象：[本轮修复报告](D:/python/projects/Fox/Fox/output/claude-design-repair-20260915/report.md)、其日志和当前未提交源码。范围依据：[上一轮 N1–N7 复审](D:/python/projects/Fox/Fox/output/claude-design-re-review-20260915/report.md)。

证据级别：**源码与测试断言复审 + 已保存日志核对**。本次未重新编译、未运行产品测试或真实 Office／云模型／GUI。以下触发情形来自当前代码分支，不能转述为本次已经跑过的端到端结果。本次只新增此报告，未改产品代码、旧证据、原始 Excel、记忆或技能，未提交、未推送。

## 1. F1 / P1：终态竞争返回重新规划，但旧模型派发仍处于 leased

涉及原 N5。

位置：

- [live.rs:794](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/live.rs:794)：提交终态遇到 `STEERING_COMPETITION`，直接返回 `LIVE_DETACHED`。
- [kernel_coordinator.rs:764](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:764)、[kernel_coordinator.rs:1005](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:1005)：非 Live 只把错误转为 `STEERING_REPLAN`。
- [repositories/kernel.rs:1041](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/database/repositories/kernel.rs:1041)：模型结果的 lease 完成操作在结果提交事务内。
- [repositories/kernel.rs:2102](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/database/repositories/kernel.rs:2102)：同一事务发现未投递补充要求后报错，整个事务回滚，lease 完成也随之回滚。
- [kernel_coordinator.rs:175](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:175)、[kernel_coordinator.rs:245](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:245)：失败的候选状态不会安装到内存；此前的 dispatch 已单独持久化。
- [kernel_host.rs:187](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_host.rs:187)、[kernel_host.rs:290](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_host.rs:290)：虽然遇到标记会 `continue`，下一轮读取持久化快照后仍把 leased 的 InitialModel／ContinuationModel／DeliverToolBatch 判为 `kernel.uncertain_execution`。

触发：最后一次队列读取为空 → 用户成功追加 → 终态事务遇 guard 回滚 → transport 返回重新规划 → 外层循环发现旧 leased 派发 → Run failed。新标记本身没有创建可执行的 steering 派发，也没有安全结算已收到的模型回复。

现有测试 [steering_tests.rs:337](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/steering_tests.rs:337) 在返回模型结果前入队；之后生产代码还会读取队列，因此通常走普通续答分支。测试允许无错误，并只单独检查 guard 字符串映射，没有构造“最后一次队列读取后、事务提交前”的竞争，也未驱动下一轮外层循环。

修复要求：在明确模型结果已收到且执行已结算的边界，重新规划并原子提交该结果、正确结算原派发、创建带完整历史的 steering 续答；或者设计等价的持久化恢复状态。**不要全局清除 leased 状态、放宽 uncertain-execution 保护或重新执行已完成工具。**

验收：用确定性屏障在最后队列读取之后入队，覆盖 Live 与非 Live，驱动真实外层循环直到补充要求 `applied`、Run completed；断言原模型结果保留、没有 uncertain_execution、没有重放工具。补充恢复过程的重启证据。

## 2. F2 / P1：Office 端到端评测仍绕过受管文件登记

涉及 N1 的正向验收。

位置：

- 生产 [kernel_host.rs:795](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_host.rs:795) 先分类受管写入，[kernel_host.rs:848](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_host.rs:848) 在成功后调用版本登记。
- 评测 [real_eval_tests.rs:1977](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs:1977) 自建 execute 回调，仅调用 `policy.execute_context_resource` 或 `policy.execute`，没有上述登记步骤；[real_eval_tests.rs:2014](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs:2014) 直接调用一次 `dispatch_initial_live`，也没有运行生产外层恢复循环。
- 新断言 [real_eval_tests.rs:2703](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs:2703) 却要求至少 6 条 Office 版本及每个文件有版本。

结论：当前入口能执行真实 Office 工具，但在新建评测数据库中不会经过生产版本登记。即使解决启动阻塞，新增登记断言仍不能通过该执行回调获得所需记录。增加断言不等于接通被测路径。

修复要求：让评测复用生产的受管写入执行流程；必要时把该流程抽成生产与评测共用的无 UI 依赖函数，同时保留冻结授权、执行 claim、写前保护和写后校验。不要在检查器里补造版本行。对会 detach／重试的场景，评测应驱动外层协调流程。

验收：新建隔离数据目录，真实 Office create/edit 经过共用执行函数；输出真实工具执行记录、登记行、内容哈希，选择版本实际恢复并逐字节比较。失败写入、普通 MCP 和未授权 wrapper 不登记。合成供应商模式与真实云模型模式继续分开报告。

报告所称 32 分钟无进展只能证明本次尝试未完成；仅凭 CPU 低、输出目录未出现，尚不能定位是构建锁、测试初始化、数据库等待还是子进程握手。应补阶段标记和超时定位，不继续无观测地长时间等待。

## 3. F3 / P1：源数据独立统计核验仍未接入生产，原要求被遗漏

涉及原 N2／R6，**不是新增范围**。上一轮复审已明确要求期望值从指定源数据独立计算。

位置：

- [delivery.rs:1238](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:1238)、[delivery.rs:1274](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:1274)：比例要求来自任务文本中的数字。
- [delivery.rs:1436](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:1436)：仍是文本启发式，按同一语句内数字大小推导分母，而非绑定源表和指标。
- [delivery.rs:2078](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:2078)：核验对象是交付产物；传入要求为空时直接返回，没有从源附件补建统计要求。
- [修复报告:95](D:/python/projects/Fox/Fox/output/claude-design-repair-20260915/report.md:95) 明确承认源工作簿重算只在评测入口，并将其列为本轮范围外。

当前改进能拒绝“40/100 却写 70%”，但不能证明 40 与 100 符合原始 Excel。真实 AGV 指令要求根据附件统计，本来就不会提前给出所有正确分子和分母；因此仍可能没有对应统计核验项。没有建立的要求，也不会因为存在 `Unverified` 枚举而自动产生未核验记录。

修复要求：对已明确支持、能确定统计口径的指标，绑定源附件版本／哈希、工作表、范围、去重／分类口径及指标 ID，独立计算期望值后比较产物；无法确定口径的要求显式留为未核验。不要硬编码 AGV 数字，不要使用模型自己输出的结果充当独立期望值，也无需扩展成任意自然语言自动证明系统。

验收：源记录改变而产物仍用旧数时必须失败；两个产物同时写出相同错误数字仍失败；正确重算并按约定舍入时通过；含糊统计口径明确未核验。验收必须经过生产交付检查入口。

## 4. F4 / P2：每工作表图表要求仍被总数代替

涉及原 N2。

[delivery.rs:1739](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:1739) 的实际判定是 `总图表数 >= 每表要求 × 工作表数`。它没有记录或检查每张图表属于哪个工作表。

触发：两个工作表，Sheet1 有两张图，Sheet2 没有图。总数 2 满足 `1 × 2`，仍会标为通过。包内未被工作表引用的图表部件也不能单独证明用户能看到图表。

现有 [delivery.rs:2914](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/delivery.rs:2914) 测试手工构造的 `ArtifactFacts` 只有总数与工作表名称，无法表达“2 张都属于 Sheet1”；它还明确把两表两图视为通过。

修复要求：通过实际 OOXML 工作表→drawing→chart 关系记录每个目标工作表的有效图表数量，并逐表核验；绑定关系无法可靠解析时保持未核验。

验收：使用有效 xlsx 文件覆盖两表各一图通过、两图都在同表失败、孤立 chart 部件不计数；不要只增加总数字段测试。

## 5. F5 / P2：非 Live 续答将同一 assistant 回复加入历史两次

涉及原 N4，历史丢失的主体修复应保留。

- 初始路径 [kernel_coordinator.rs:752](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:752) 已向 history push 本轮 assistant，然后又把同一回复传给 helper。
- 批次路径 [kernel_coordinator.rs:948](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator.rs:948) 相同。
- helper [steering.rs:81](D:/python/projects/Fox/Fox/apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/steering.rs:81) 再次 push `refused_assistant`。

因此每次非 Live steering 续答都复制一份本轮最终回复，增加上下文与后续压缩压力，也不再与真实轮次一一对应。当前测试只断言字符串存在，没有断言出现次数与完整序列。

修复要求：明确 helper 接收的是“回复前历史”还是“已含回复的完整历史”，只在一处追加。保留已有工具调用／结果历史。

验收：初始与批次两路径比较完整消息序列，assistant 最终回复恰好一次，工具调用／结果配对及顺序不变，追加提示恰好一次，再驱动到 `applied`。

## 6. N1–N7 当前状态

| 原项 | 复审结论 |
| --- | --- |
| N1 Office wrapper | Kernel 生产识别／校验／登记代码已接上，Legacy 也调用共用结果校验；真实正向端到端仍缺，且评测入口存在 F2。不能把当前单元测试称为实际 OfficeCLI 成功。 |
| N2 统计与图表 | 百分比算式与容差检查有进展；源数据核验 F3、按工作表归属核验 F4 未完成。 |
| N3 steering 压缩预算 | 源码已把实际 wrapper 字节纳入压缩前预算，并移除对应重复计量；可保留。真实长历史→摘要→成功投递链路仍未验收，不因此单独判定代码未修。 |
| N4 完整历史 | 原工具历史丢失已修，新增重复回复 F5 待修；完整 applied 流程仍需补证。 |
| N5 终态竞争 | 标记、部分队列重读、接收前预算拒绝已有；F1 表明关键恢复流程仍未完成。 |
| N6 外部 X 内容版本 | 已新增 replaced 行、恢复入口和选择 X 的字节测试；主体修改可保留，GUI 与异常记账路径不在本次运行验收内。 |
| N7 技能发现 | 已有按启用范围搜索／分页与按 ID 加载；主体修改可保留，未把代码检查算成真实模型会用该入口。 |

## 7. 日志、快照及后续顺序

已核对保存日志：Rust 全量 809 passed / 0 failed / 8 ignored；managed-files 14、delivery 17、delivery_live 4、steering 12、skill_load 3、skills 10、migrations 28 均通过；Node 合同 30/0；前端 243/0；协议绑定 current。**这些是另一智能体保存的运行证据，本次没有重跑。** tsc 日志只有 npm notice 及 PowerShell stderr 包装信息，没有 TS 诊断；日志本身未记录退出码，不能仅据该文件独立证实 exit 0。

快照说明需修正文案：[报告:42](D:/python/projects/Fox/Fox/output/claude-design-repair-20260915/report.md:42) 把“上一轮开始时”的 525 文件快照称为本轮 N1–N7 改动前基线，这两者不是同一时点。“文件名都覆盖”不能证明包含上一轮修复后的字节。实查旧快照 `managed_files.rs` 为 21,833 B，连上一轮已新增的 `resolve_restore_source` 都没有；本轮结束快照为 62,376 B，包含该函数。旧快照可以保留为更早恢复点，但不能整树回写来撤销仅本轮修改。缺少确切开工状态的文件如实注明，不伪造新基线。

原始 Excel 本次复核：258,185 B，SHA-256 `b01b6b7ce5dbc8344f2e0866ee9a533973824679601f1d3ebb60abc1ac34b09a`，与此前留存一致。

建议下一批：先修 F1 与 F5，补确定性完整 steering 流程；接着修 F2 并定位评测启动阻塞；再补 F3／F4 的生产核验与有效文件测试。保留已完成部分，无需重做所有 N1–N7。真实云模型 AGV、GUI 和安装仍单独标为未验收。
