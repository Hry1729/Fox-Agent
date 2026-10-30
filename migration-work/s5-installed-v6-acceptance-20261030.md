# S5 已安装 v6 验收与合并依据

日期：2026-10-30。当前运行时实现和本轮指定补测已确认，可合并交付 `main`。这不代表 S5 全部验收或正式发布完成。联系人 M1/M2 暂不推进，S6 不执行。

## 源码、产物与核对方法

- 交接分支：`s5-default-pinative-20261029`，HEAD `2e30234482aaa8f90b78b864fa3143d53636f4c5`。
- v6 构建源码：`a952125dd53d8a8bc7b01e670bf71068076ad344`；其后至交接 HEAD 只改文档和证据清单。本次也没有修改生产代码。
- 实际安装目录：`D:\ruanjian\Fox`；运行库：`C:\Program Files\Fox\sandbox-runtime\0.1.6`。
- 合并前重新计算安装文件和原生会话 SHA-256，核对已有事件、请求片段、会话映射及隔离矩阵。此轮为只读证据复核，没有重复运行桌面或模型测试。
- [结构化复核记录](s5-installed-v6-evidence-20261030.json)保存源证据哈希、关键 ID、断言和范围。[候选清单](s5-candidate-manifest-20261030-v6.json)保留构建时点的原始状态。

| 产物 | SHA-256 |
| --- | --- |
| v6 安装包 | `8f86b8747cdcaed79825e165faf62be9ac3797e9d5acb395c5621391802e57c6` |
| 已安装 desktop | `457492bec74d63d78794c2ff848d0dbf85886644ff369672d22afaf08069488f` |
| 已安装 sidecar | `f16812055cdd930ec308dab4df680be318de1b51444b8a6474fb834c5b889846` |

## 已取得的验收证据

| 项目 | 结论与范围 |
| --- | --- |
| 安装身份、0.1.6 运行库 | PASS；实际安装程序及 Host 命令已运行 |
| 默认执行路径 | PASS；新 Run 为 `authoritative + pi + pi_native` |
| 普通对话、能力提示交付 | PASS；受控本地 OpenAI-compatible Provider |
| 命令和写入审批 | PASS；真实 Host 执行，命令 `sandboxMechanism=srt`、`exitCode=0`，写入 33 字节交付文件 |
| 文件交付卡与打开 | PASS；`present_files` 后在安装程序 UI 打开实际文件 |
| 会话隔离、上下文重置、重启续聊 | PASS；独立映射、历史保留及新代次已核对 |
| 自动压缩、持久化、严格重启续聊 | PASS；SDK 自己写入摘要，重启后的请求仍带摘要 |
| 知识隔离 | PASS；仅 `sqlite_lexical` 词法回退，不包括完整向量检索 |
| 记忆隔离、重置后保留 | PASS；范围为 project/global/`fox-general` agent，不是联系人私有记忆 |

原安装验收：`D:\python\projects\Fox\output\s5-install-v6-acceptance-20261030\postinstall-report.md`。

两项补测：`D:\python\projects\Fox\output\s5-install-v6-supplement-20261031\supplement-report.md`。该目录名和报告日期沿用原记录；其中执行时间戳为 2026-10-30，本次未重写原始报告。

## 两项补测的关键事实

自动压缩使用明确标注的合成 usage，未修改生产阈值。模型 contextWindow 为 32768，reserveTokens 为 5120，阈值为 27648；压缩前用量为 30002，SDK `prepareCompaction` 可生成摘要计划。Run `effa0768-7b6f-4279-979d-412a2a8b5d76` 产生 seq 11 的 started 和 seq 14 的 completed，`aborted=false`、`willRetry=false`。原生 JSONL 的 compaction 条目 `c02b01ea` 含历史标记 `S5_COMPACTION_EVIDENCE_A1`。

旧 desktop PID 21864、sidecar 6252 退出后，从相同安装路径重启为 PID 41732、sidecar 32636。会话 `ac9920fe-9128-440a-a418-d9460b12c5ee` 沿用 Host runtime session `ddcb6478-df4e-4189-b77f-ea98be65a2ce`，对应 Pi 原生 session ID `01a18cc2-f673-74fc-afd0-6add3c9c7d9f`。重启续聊 Run `00e0f2e3-301c-4fcd-ad53-f2d3292a13dc` 和 `2b102aa2-dde1-47fb-b3c3-1d983da69547` 完成；实际请求仍有摘要。

A/B/普通会话通过真实 Host 工具检索。A 仅有 A 项目记忆和明确共享记忆，B 同理，普通会话没有 A/B 项目记忆。A/B 的 `list_knowledge_bases → search_knowledge → read_knowledge_document` 分别返回各自文档，普通会话列表为空。知识检索明确返回 `sqlite_lexical / vector_index_unavailable`。

A 重置后 contextGeneration 为 2，历史和绑定保留，真实检索仍返回 A 的内容；B 保持 generation 1。三个会话的实际 agent 都是 `fox-general`，`expert_id` 不等于记忆 agent 作用域。

Provider 固定 usage、知识列表解析和导入文件名问题都属于测试夹具问题，已在补测中修正，没有因此修改生产代码、放宽权限或伪造工具结果。

## 合并检查与剩余事项

既有完整本地检查见 [S5 CI 记录](s5-full-ci-20261030.md)：Desktop 872 pass / 2 skip，Rust Host 2271 pass / 31 ignored，Agent Runtime 626 pass / 1 todo；均无失败。之后的 Zvec/ONNX 真实独立 smoke 已通过。结果保留各自源码时点和范围，不把旧 SHA 的测试写成新 SHA 的远端 CI。

本次合并前，tracked 工作区干净。远端 `main` 为 `daa799ff`，本地 `main` 为 `71ffbb7e`，两者均为交接 HEAD 的祖先。待交付包含此前本地 main 的 10 个提交和 S5 分支的 16 个提交；未跟踪的测试数据、日志和安装包保留在本地，不纳入本次提交。用户本轮已授权合并、提交和推送，旧记录的“未授权推送”限制不再适用。

以下项目仍未取得当前已安装 v6 的完整通过证据：真实外部模型、知识库向量检索全链、原生文件夹选择成功路径、升级/卸载/还原及 ACL/SID 清理。远端 CI 以推送后对应 SHA 的实际结果为准。

没有发布安装包或创建标签。S5 完整关闭仍需完成剩余验收，并观察至少一个实际发布周期未触发回退；合并不改变此要求。
