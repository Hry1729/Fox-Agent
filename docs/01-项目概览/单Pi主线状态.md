# 单 Pi 主线状态

> 状态：维护中；默认 Authoritative + PiNative，已安装 v6 核心链路及两项补测通过，尚未发布<br>
> 适用范围：S5 合并交付；v6 构建源码 `a952125d`，补测交接 HEAD `2e302344`<br>
> 最后更新：2026-10-30（补入实际安装和补测证据；保留此前历史正文）

## 当前结论（2026-10-30 S5 候选）

新本地 Run 和子 Run 一律使用 Authoritative 执行契约和 PiNative 路由，没有路由开关（A0 删除了 `FOX_EXECUTOR_ROUTE`，不再有 Kernel 回退）。已有冻结绑定不改写；未结束的历史 Kernel Run 在启动时标为中断（「执行路径已退役」），不再续跑。`FOX_KERNEL_MODE` 不再决定新 Run 路径。

能力说明 JSON 缺口已修复。既有完整本地 CI 及 Zvec/ONNX 独立真实 smoke 已通过，保留各自源码时点和忽略项。实际安装的 v6 desktop/sidecar 哈希已与候选核对，runtime 0.1.6 下的审批、命令、写入、交付卡打开、会话隔离、重置和重启续聊已通过。

两项补测已通过：自动压缩、原生落盘、严格重启与摘要续接；知识/记忆隔离及重置后保留。知识仅验证 `sqlite_lexical` 词法回退，记忆仅验证 project/global/`fox-general` 作用域。详见 [已安装 v6 验收与合并依据](../../migration-work/s5-installed-v6-acceptance-20261030.md)。

当前运行时实现和指定补测已确认，可合并交付 main；联系人 M1/M2 暂不推进。真实外部模型、完整向量检索、原生文件夹选择、升级/卸载/还原和发布周期观察仍待验收。远端 CI 以交付 SHA 的实际结果为准。S5 不宣称完整关闭，未发布、未执行 S6。

## 历史增量（2026-10-30 安装前的能力说明修复）

以下保留安装前的原始记录。其中安装阻塞、runtime 0.1.5 和未合并等状态不代表上面的当前结论。

确认了一个实际产品缺陷：`composeFoxPrompt` 生成了能力事实和完整提示，但返回对象缺少 `stablePrefix`/`dynamicTail`；Pi 规划扩展因此收到空分区，模型实际使用 SDK 默认提示，能力 JSON 从未进入 HTTP。`2ccd2a97` 只补回这两个分区，并增加分区级回归断言；manifest/profile 校验、Host 权限和工具筛选不变。

最终 v5 payload 的真实 HTTP 已观察到 `30094` 字符系统提示、能力标题和 `<fox_context>`，冻结 profile 为 `legacy`，`run_command.availability=available`。修复前同路径 HTTP 是 `1875` 字符的 SDK 默认提示。新增真实 runtime + SDK 扩展 + HTTP 序列化回归，持续保护完整交付链。证据见 [S5 完整 CI 记录](../../migration-work/s5-full-ci-20261030.md) 和 [能力 HTTP 证据](../../migration-work/s5-capability-http-20261030-v5.json)。

Node 22 下 Desktop 全量 **872 通过 / 2 跳过 / 0 失败**（工作流加 `--isolate`，不跳过断言），Rust Host **2271 通过 / 0 失败 / 31 忽略**，Agent Runtime **626 通过 + 1 个预期 todo / 0 失败**；协议、ACP、Kernel worker 和性能基线通过。Zvec/ONNX 已通过资源准备和真实 smoke（旧时点曾因资源缺失跳过），远端 CI 未运行。

系统安装仍阻塞：NSIS 清单要求 `requireAdministrator`，直接启动返回“请求的操作需要提升”（HRESULT `0x800702E4`），当前没有隔离提升环境；本机仍是 runtime 0.1.5，随包 0.1.6 的安装、重启、命令、升级和卸载未验证。解包 payload 启动与能力 Run 不能算安装通过。S5 仍未关闭，未合并、未推送、未发布，未执行 S6。

## 历史结论（2026-10-28 桌面验收后）

以下正文保留当时的事实与缺口；其中“默认 Authoritative 原生会话未完成”、v99 与旧候选状态不代表上面的 S5 分支现状。

本地新会话固定 `agent_backend=fox`，本地引擎为 Pi；旧外部 Pi ACP、Codex、DeepSeek 会话保留历史并拒绝执行。数据库为 **v99**：v98 保存原生映射，v99 增加 `runtime_session_generations` 退休代次台账。重置上下文与永久删除自有代次文件均已实现并提交。

本轮使用真实 Windows 桌面、隔离数据和真实模型执行 A–I 矩阵。各项分别固定实际 Host、sidecar 和前端版本，详见[桌面验收报告](../../apps/desktop/tests/acceptance/single-pi-20261027/RUN-20261028-terra.md)。已取得连续重置、重置后重启首发、切换专家后首发、编辑重跑与代次归属、回收站恢复与永久删除、同一原生会话重启恢复、损坏会话连续两次重试拒绝的证据。新对话静态 Fox 标签也已复核。

引用可以展开来源并打开正确的本机文档，来源 ID 与资料库、文档 ID 可核对。当前来源块覆盖整篇文档，**没有验证 A-7 条款级精确定位**。

这些原生会话结果属于显式 `FOX_KERNEL_MODE=legacy` 的 SDK 路径。默认仍是 Authoritative，不能据此宣布默认路径已具备同样的原生持久会话能力。A–I 的记录分布在修复过程中的不同构建，不是最终同一安装包上的完整矩阵。

剩余事项：Authoritative 原生会话接入；压缩后完整重启、中断/取消、Windows 长路径等未覆盖场景；历史 404 根因与并发 live 测试失败归属；最终统一构建、安装包和完整 CI。详见验收报告与[当前状态与核验基线](当前状态与核验基线.md)。

## 早期文档核验快照（`d0495a3a` 至 `e252bbd2`）

以下正文保留早期文档核验的事实与限制。其中 v98、未提交生命周期改造和未验收描述只适用于该旧基线；当前状态以上节及桌面验收报告为准。

## 结论

本分支已把本地驱动准入收敛为 Pi。新会话仍保存 `agent_backend = 'fox'`，本地 Kernel 仅运行 `engine_id = 'pi'`。历史外部 Pi ACP、Codex、DeepSeek Harness 会话保留原绑定和历史，执行入口拒绝，不自动换成 Pi 继续运行。远程知识库和 `runtime_type = 'yuxi'` 的远程专家保留。

**单驱动准入已实现，不等于 Pi 原生持久会话已完整接入。阶段二与 C gap 仍未完成。** v98 映射已存在，但当前原生会话接入位于 Legacy 路径；默认 Authoritative 路径仍使用 Kernel 的既有流程。已有记录中的 Legacy 进程崩溃、恢复后模型 404，以及恢复边界的验收缺口均未关闭。

本页描述分支实现，不代表已合入远端 `main`、发布安装包或完成业务验收。通用证据口径见[当前状态与核验基线](当前状态与核验基线.md)。

## 驱动、存储与执行权

| 概念 | 当前行为 | 事实源 |
| --- | --- | --- |
| 新会话 `conversations.agent_backend` | 只准入 `fox`；请求 `pi`、`codex`、`deepseek` 返回 `agent_backend_retired` | `crates/fox-agent-backend/src/host_routing.rs` |
| 历史外部会话 | 原值保留、只读；start / cancel / approve / resume 拒绝；不重绑定 | 同上；`commands/agent_backend_routing.rs` |
| 本地 Kernel `engine_id` | 未设置或 `pi` 时使用 Pi；其他引擎值拒绝，错误含 `kernel_engine_retired` | `apps/desktop/src-tauri/src/kernel_model_config.rs`、`services/agent-runtime/src/pi-kernel-worker.mjs` |
| `FOX_EXTERNAL_BACKENDS` | 保留解析、未知值报告与诊断；任何取值都不能重新启用退休后端 | `host_routing.rs::BackendSwitch` |
| `FOX_KERNEL_MODE` | 默认 `authoritative`；显式 `legacy` 仅用于调试和旧执行路径，已冻结 Run 沿用绑定 | `runtime_host/kernel_authority.rs` |
| `agents.runtime_type` | 本地 `pi` 与远程 `yuxi` 仍区分；远程服务不属于退休的会话级驱动 | `runtime_host/mod.rs`、`yuxi/runtime.rs` |
| 设置与聊天 | 设置 → Agent →「Agent 驱动」显示固定说明；新会话不显示驱动选择器，不显示退休驱动的默认原生权限选项 | `agent-backend-settings.tsx`、`composer-agent-picker.tsx` |

`agent_backend = 'pi'` 是旧外部 Pi ACP 路由，不能把数据库中的 `fox` 批量改成 `pi`。界面上的 Pi 名称、存储后端标识、Runtime 类型与执行权是不同层次。

`AgentBackend` 的旧枚举、数据库 CHECK、不可变触发器、部分 ACP/权限/凭据命令与受管运行时资源仍保留兼容用途。源码、测试或配置项仍存在，不代表对应驱动可运行。SRT 仍服务内置 Fox Host 命令，不能因外部驱动退休而删除共享沙箱资源。

## 阶段完成度

| 项目 | 状态 | 可支持的结论与边界 |
| --- | --- | --- |
| 阶段一：驱动、引擎与界面收敛 | 已实现，有既有定向检查记录 | 10 月 27 日记录包含单驱动界面、新会话落 `fox`、Pi 引擎真实模型输出；本轮未复跑 |
| B1/B2：Pi SDK 原生会话 | 已实现，有既有运行时测试记录 | 支持创建、打开精确文件、原生上下文持久化和分类错误；不等于默认桌面路径已接入 |
| C：Host 与数据库映射 | 部分实现 | `d0495a3a` 加入 v98、映射字段和 Legacy 入口；已有记录证明该入口生成文件并在重启后复用映射 |
| C gap：端到端执行与恢复 | **未完成** | 下节的两个运行错误和源码边界未关闭 |
| 阶段二整体 | **未完成，不具备完成验收或合并依据** | 不能把一次历史回灌续聊、数据库升级、文件增长或单元测试通过当作阶段二成功 |

## C gap 与未验证项

### 已记录的运行失败

来源为[阶段二进展记录](../02-架构/Fox-单Pi主线-阶段二进展-20261027.md)，不是本轮复现：

1. 在 `FOX_KERNEL_MODE=legacy` 的真实桌面试验中，首轮出现 `runtime.process_crashed`，消息为 `runtime process closed stdout`。原因与修复效果待定位、复测。
2. 恢复后出现 `provider.request_failed: 404 model route not found`，记录中的模型为 `sensenova-6.8-flash-lite`。现有证据只能证明端点返回该错误，尚不能确定供应商配置、模型路由或适配器中的具体根因。

### 已提交基线 d0495a3a 的源码边界

以下结论来自本轮开始时的已提交源码，不是对后续未提交实现的判定；本轮不修改代码，也未做桌面复现：

- `runtime_host/mod.rs` 在 Authoritative 分支提前进入 `start_kernel_run`；`open_runtime_session` 的新原生映射位于 Legacy 分支。默认路径不能宣称已获得这套持久会话能力。
- `resume_runtime_session` 的原生恢复失败会标记 `unavailable` 并在当次返回错误；但 `open_runtime_session` 遇到 `unavailable` 行会走新建。因而跨请求的“必须由用户显式重置”仍未闭合。
- `resume_plan` 在原生路径缺失时使用 Host 传入的 fallback 路径；缺少会话 ID 时传 `None`。不能宣称所有原生恢复都已强制具备完整、精确的文件与身份映射。
- `create_native_runtime_session` 校验返回路径的父目录后提交映射，允许文件尚未写出；映射失败尝试删除文件。因此“先确认文件已持久化，再提交数据库”的冻结要求尚无完整实现保证。
- 新建请求中的 `contextGeneration` 为 1。字段已加入，不代表显式重置、代次递增和跨专家隔离已完成验收。

### 工作区并行实现

本轮文档工作期间，工作区出现了 `runtime_host/session_lifecycle.rs`、生命周期测试、数据库 Repository 和验收夹具的并行改动。`RuntimeHost::open_runtime_session` 已在未提交差异中改为委托生命周期模块，因此上一节描述的旧函数行为只能归属于已提交基线。

收尾时 HEAD 由并行验收夹具提交推进至 `e252bbd2`，包含夹具程序、初始化入口和验收 README，也纳入本轮给 `RUN-20261027.md` 添加的页首说明；Host 会话生命周期改造仍有未提交内容。本轮没有执行提交或推送。

这些改动由其他工作产生，本轮不修改或回滚；只确认存在，不将其视为测试或修复通过。后续应以完成后的代码、候选构建和实际结果更新逐项状态。**阶段二与 C gap 仍未完成。**

仍需验证：专家往返、资料问答、跨专家隔离、压缩后完整重启、损坏或缺失文件恢复、显式上下文重置、中断、取消、Windows 长路径，以及当前候选构建的直接 IPC 拒绝。代码测试记录与真实桌面场景分别登记，未验证既不是通过也不是失败。

## 数据库与原生会话

`fox.db` 当前 Schema 为 **v98**。v98 只向 `runtime_sessions` 增加以下字段；v95 的四种历史后端值及冻结触发器不变：

| 字段 | 类型与默认 | 含义 |
| --- | --- | --- |
| `native_session_ref` | TEXT，可空 | Pi 自己的会话 ID |
| `native_session_path` | TEXT，可空 | Pi 原生 JSONL 文件位置 |
| `format_version` | INTEGER，默认 1 | 1 为旧 Fox JSON 快照；2 为本次 Pi 原生 JSONL，当前恢复代码对 ≥2 走原生分支 |
| `context_generation` | INTEGER，默认 1 | 上下文代次字段；完整重置流程未验收 |

SQLite 保存产品状态和映射；Pi 原生文件保存模型上下文，不能因为 SQLite 展示历史存在就把该文件视为可任意丢弃的缓存。备份应同时覆盖数据库和原生会话目录；旧构建兼容 v98 与原生文件的回退未在本轮验证，不能套用阶段一“没有迁移、无需备份”的旧结论。

## 证据与历史记录

- [单 Pi 执行与交付说明](../02-架构/Fox-单Pi主线-执行与交付说明-20261027.md)：阶段一时点的实现、定向检查和限制。正文中的“阶段二未实现”“未做真实桌面”有时间范围。
- [首次桌面验收](../../apps/desktop/tests/acceptance/single-pi-20261027/RUN-20261027.md)：Authoritative 路径的真实生成与重启续聊；续聊依赖历史回灌，不能证明原生会话恢复。
- [阶段二进展](../02-架构/Fox-单Pi主线-阶段二进展-20261027.md)：B1/B2 及后续 C 部分实现和 Legacy 试验。早期“C 尚未开始”已被后续记录推进。
- [冻结接入契约](../02-架构/Fox-单Pi主线与SDK原型-接入契约-20261027.md)：目标与约束，不能直接作为已实现或已验收的证明。
- [四 Agent 方案](../02-架构/四Agent改造实施方案-v0.3.1.md)及后端人工复测：保留历史正文；可用于解释旧字段和旧记录，不能作为当前驱动准入指南。

本轮只完成文档与源码对照、文档一致性和文件保护检查。没有新增完整 CI、运行时测试、桌面、真实模型或安装包验收结果。
