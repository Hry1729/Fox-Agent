# FoxOps 开发设计文档

> ⚠️ **前端工程师注意（2026-07-08 标记）**：本文档是 FoxOps 早期「自建全栈」设计稿，其中前端部分（§3 前端设计 / §18 目录结构 / §19 技术选型）描述的 React + Tauri + shadcn/ui 方案**已废弃**。当前 FoxOps 改为以 Yuxi 为底座二次开发，前端实际技术栈为 Vue 3 + Art Design Pro + Element Plus，以 `docs/vibe/foxops基于yuxi开发的设计文档.md` 与 `foxops-web/前端开发指引.md` 为准。本文档仅用于了解领域逻辑（故障码建模 / 安全规程 / 诊断流程 / HITL 等），**不要据此搭建前端**。

> 版本：V0.6
> 定位：内部开发设计文档（可拆任务开工的 MVP 设计）
> 范围：产品结构、前端工作台、后端主线架构、知识库、检索、知识图谱、智能体、本地智能层、设备数据联动、安全权限、可观测性、质量评测
> 原则：工作台为主界面，小雪狐为宠物系统；知识库服务于智能体；后端统一管理权限与可信数据；官方知识以 PostgreSQL 为唯一可信源；高风险写操作必须人工确认（只读诊断链无 HITL）。

---

## 0. 文档说明与版本变更记录

### 0.1 文档目的

本文档面向 FoxOps 研发团队，用于统一产品结构、架构认知、技术主线和阶段边界，作为各阶段开发的设计基线。

V0.4 收敛高风险架构问题：客户端形态、数据出域红线、本地向量层可替换、安全知识在线校验、阶段一 Spike、故障码关系前置、切片策略和评测冷启动。V0.5 修正落正文时引入的二阶问题：向量列维度与 HNSW 索引矛盾、Zvec 在规范条款中的残留、现场入口与离线交互态的缺口，并补齐入库作业表、安全在线校验归属、现场入口契约等。

**V0.6** 在 V0.5 基础上，按三方联合审阅（架构 / 工程实现 / 逻辑严谨性）建议，做四个动作：

1. **压范围**：补"团队假设"锚定人力（§0.7）；阶段一拆为 M0/M1/M2 三个 2-3 周可闭环里程碑（§17）；阶段三以后的车载 DDL / 查询模式 / 设备协议比较 / 宠物状态机降级为方向占位。
2. **硬化契约**：补 TypeScript 类型定义 Citation/Evidence/DiagnoseEvent/SafetyRuleCheckResult（§5.6）；补业务会话表 conversation/message/agent_checkpoint（§6.3）；补 embedding_model_registry/embedding_index 锚点表（§6.3）；补离线包 Manifest 契约（§11.4）。前后端共享 API 类型通过 FastAPI OpenAPI spec 自动生成（§5.6 / §18）。
3. **修补逻辑冲突**：恢复最低限度 `sensitivity` 字段（public/internal/restricted），解决 RBAC 与"机密禁止导出"矛盾（§6.3 / §13.3）；明确 `required_online_check` 分层语义（§3.4 / §7.4 / §13.4）；阶段一a DoD 加 Schema 前向验证检查点（§17）；PGroonga / 模型降级标注风险并给回退方案（§7.4 / §14.4 / §19）；显式声明离线权限缺口（§11.2 / §13.6）。
4. **修正流程错误**：诊断流程 HITL 移出只读链，拆只读链 / 写操作链（§9.3）；知识状态机补 rollback 路径（§6.5）；统一 token 预算裁剪点（§7.2 / §9.3）；角色变更后 JWT 失效机制（§13.5）；入库 stage 支持 skip/跳步（§6.4）。

> 详见 §0.2 V0.5→V0.6 变更记录。专项细节下沉到附录 A 占位的 5 份专项文档中。

### 0.2 V0.5 → V0.6 变更记录

| 类别 | 变更内容 |
|---|---|
| 新增 | §0.7 团队规模与资源约束声明（≤5 人 / 有限 GPU / 砍一期 LocalVectorStore/宠物系统/MinIO，纯在线模式起跑） |
| 调整 | §17 阶段一 1a+1b 拆为 M0/M1/M2 三段 2-3 周里程碑；阶段三后车载细节降为方向占位 |
| 新增 | §5.6 补 TypeScript 类型 Citation / Evidence / DiagnoseEvent / SafetyRuleCheckResult；SSE 统一用 `event: diagnose` + `data.type` 做分发（discriminated union） |
| 新增 | §6.3 补业务会话表 conversation / message / agent_checkpoint（与 LangGraph checkpoint 解耦职责） |
| 新增 | §6.3 补 embedding_model_registry / embedding_index 锚点表，承接模型版本与索引可用性判断 |
| 修复 | §13.3 恢复最低限度 `sensitivity` 字段（public/internal/restricted）；`restricted` 不进离线包、不允许导出、需管理员查看。不恢复完整 ACL |
| 修复 | §3.4 / §7.4 / §13.4 明确 `required_online_check` 分层语义：控制"在线状态下是否强制回 PG 校验最新版本"；离线时一律走 offline-safety 降级（基线 #10 不变） |
| 修复 | §9.3 拆"只读链（诊断/检索/问答，无 HITL）"与"写操作链（发布/配置/设备，HITL）"，移除只读链中永真为假的 HITL 节点 |
| 修复 | §6.5 知识状态机补 reactivate/rollback：A) 间接回滚（创建新版本走正常流）；B) 直接 reactivate（deprecated→active，需审核确认 + audit_log） |
| 修复 | §7.2 / §9.3 统一 token 预算裁剪点：检索管线输出 Top-K evidence 时做一次裁剪，Agent generate 节点直接使用不再二次裁 |
| 修复 | §6.4 入库 stage 支持跳步：按 mime_type / 文档质量评估动态决定 stage 列表，纯文本 docx 跳过 OCR/extract，一期先 5 步 |
| 新增 | §13.5 角色变更后 JWT 失效机制：将已签发 access_token 加入黑名单或设极短 TTL，迫使 refresh 拿新 claims |
| 新增 | §11.4 / §13.6 显式声明离线权限缺口：LocalVectorStore 为纯本地存储，不嵌入 RBAC；离线场景 safety 由"桌面设备物理安全 + OS 级登录"保护，而非 RBAC |
| 新增 | §11.4 离线包 Manifest 契约（package_id / version / generated_for_user_id / allowed_until / contains_safety_rules / sensitivity_max / embedding 信息 / content_hash） |
| 新增 | §9.4 工具风险分级枚举 ToolRiskLevel：read_only / write_local / write_server / safety_critical / device_control |
| 新增 | §9.5 证据约束生成三步校验流程：引用存在性 → 安全结论蕴含校验 → 降级处理 |
| 新增 | §6.3 DDL 补 CHECK/UNIQUE 约束（fault_code.status/severity、knowledge_chunk (document_id, ordinal) 复合 UNIQUE、document_section (document_id, parent_id, ordinal)） |
| 调整 | §7.4 / §14.4 / §19 PGroonga 标注为风险项，非硬依赖，配置项 `fulltext_backend` 支持 trgm / tsvector+zhparser / pgroonga；§14.4 模型降级矩阵（LLM/Embedding/Reranker/safety 检查/设备数据）|
| 新增 | §6.3 DDL 后置项本版保持后置（safety_rule 版本/有效期、结构化主数据审核流、各表软删字段统一、无维度 vector 列 ORM 映射），与 V0.5 §0.2 一致，不越界提前 |
| 新增 | §18 提倡 Monorepo 根目录治理（apps/client、apps/server、packages/api-contracts、packages/config、infra/docker、docs）；前后端共享 API 类型由 FastAPI OpenAPI spec 通过 `openapi-typescript` 自动生成 |
| 新增 | 附录 A 列专项文档占位清单（API 契约 / 数据库设计 / 前端组件与状态 / RAG 与评测方案 / 安全权限与离线包），本版不生成专项文档内容 |

#### 0.2.1 三方审阅建议 traceability 表（共 30 条，全部已处置）

| # | 建议摘要 | 处置 | 落点 |
|---|---|---|---|
| #01 | 一期范围过大需拆 M0/M1/M2 | 采纳 | §17 M0/M1/M2 |
| #02 | 缺团队规模声明 | 采纳 | §0.8 团队假设 |
| #03 | API/SSE/Citation/Evidence 类型缺失 | 采纳 | §5.6 TypeScript 类型 |
| #04 | 缺 conversation/message 表 | 采纳 | §6.3.4 |
| #05 | RBAC 与"机密禁止导出"冲突 | 采纳 | §13.3 sensitivity |
| #06 | 诊断流程 HITL 永真为假 | 采纳 | §9.3 拆只读链/写链 |
| #07 | 离线权限不可执行需声明 | 采纳 | §11.2 / §13.6 |
| #08 | 阶段一 Schema 与阶段三 Agent 时序风险 | 采纳（M1 DoD 加检查点）| §17 M1 |
| #09 | Embedding 锚点表 | 采纳 | §6.3.5 / §14.4 |
| #10 | Evidence 知识版本锁定 | 采纳 | §5.6.1 Evidence.version / §9.3 |
| #11 | 工具风险分级 | 采纳 | §9.4 ToolRiskLevel |
| #12 | required_online_check 语义模糊 | 采纳 | §3.4 / §7.4 / §13.4 |
| #13 | DDL 缺 CHECK/UNIQUE | 采纳 | §6.3 各表补约束 |
| #14 | 证据约束生成流程未具体化 | 采纳 | §9.5 三步校验 |
| #15 | 知识状态机缺 reactivate/rollback | 采纳 | §6.5 |
| #16 | token 预算双重裁剪 | 采纳 | §7.2 / §9.3.1 |
| #17 | 角色变更后 JWT 未失效 | 采纳 | §13.5 |
| #18 | SearchBackend 抽象降级 | 采纳 | §7.4 |
| #19 | 离线包加密/Manifest 空白 | 采纳 | §11.4 Manifest（加密方案阶段四前定）|
| #20 | 阶段一b "最薄 RAG" 验收口径 | 采纳 | §17 M2 DoD |
| #21 | 入库 stage 可跳步 | 采纳 | §6.4 |
| #22 | PGroonga 部署风险 + 模型降级路径 | 采纳 | §7.4 / §14.4 / §19 |
| #23 | LlamaIndex 不接管业务对象 | 采纳 | §4.2 / §9.4 |
| #24 | 关系表→图谱同步策略 | 方向确认 | §8.5 占位 |
| #25 | "域内模型"精确定义 | 采纳 | §0.7 基线 #9 / §13.7 |
| #26 | HITL 入口仅限桌面 | 采纳 | §9.3.2 / §13.5 |
| #27 | 宠物系统移出主文档 | 采纳 | §3.6 改为方向占位 |
| #28 | 缓存失效触发机制 / 离线包分发 | 方向确认 | §11.5 占位 |
| #29 | 灰度与离线状态优先级 | 方向确认 | §19.2 第 14 项 |
| #30 | Monorepo 根目录治理 | 采纳 | §18 |

### 0.3 V0.4 → V0.5 变更记录（历史）

| 类别 | 变更内容 |
|---|---|
| 修复 | embedding 列改为：模型选型前不写维度、且不建 HNSW 索引；定维后再 ALTER 为 vector(N) 并建索引。删除"单列多版本向量并存"的不实说法，改为停服窗口整体重建（§6.3 / §14.4） |
| 修复 | 清理规范条款中残留的 Zvec 硬编码：统一改为 LocalVectorStore / 本地向量层 |
| 修复 | required_online_check 由 fault_code_safety_rule 关系表移到 safety_rule 本身 |
| 调整 | §3.4 交互态拆出 offline-safety 安全强降级子态；明确离线/本地向量为桌面端专属 |
| 新增 | §3.3 补现场速查入口（/field）的路由、最低权限、能力面与鉴权契约 |
| 新增 | 新增 ingestion_job 入库作业表，支撑分布式 worker 断点续跑 |
| 简化 | 权限模型降级为"动作级 RBAC"；删除 owner_scope / classification 两列（V0.6 恢复 sensitivity 最低字段，见 §0.2） |

### 0.4 V0.3 → V0.4 变更记录（历史）

| 类别 | 变更内容 |
|---|---|
| 调整 | 客户端形态改为 Tauri 桌面工作台优先；Tauri 前端按 Web-first 组织 |
| 调整 | 数据不出域升级为硬约束：RAG / 诊断 / 安全主链路只能使用域内模型 |
| 调整 | Zvec 抽象为 LocalVectorStore，本地向量实现可替换 |
| 调整 | safety_rule 离线策略升级：安全结论必须在线校验 |
| 调整 | 阶段规划新增阶段零 / 阶段一并行 Spike |
| 调整 | 故障码与原因、措施、安全规则的关系建模前置到阶段一/二 |
| 新增 | shadcn 生态 AI 组件候选：AI Elements 优先，assistant-ui / prompt-kit 作为备选参考 |

### 0.5 V0.2 → V0.3 变更记录（历史）

| 类别 | 变更内容 |
|---|---|
| 调整 | 前端主界面明确为左侧常驻导航、中央对话/结果区、底部固定输入框、右侧悬浮辅助卡片 |
| 调整 | 后端主线收敛为 FastAPI + LangGraph + LlamaIndex 工具箱 |
| 调整 | API 契约、数据库 DDL、图谱结构、检索评测指标、模型选型改为专项设计或暂定内容 |
| 新增 | AI Elements 作为候选对话 UI 框架 |
| 新增 | 内网部署拓扑明确为客户端层、后端服务层、模型层三段结构 |
| 新增 | 小雪狐宠物系统按 MVP 收敛 |

### 0.6 V0.1 → V0.2 变更记录（历史）

| 类别 | 变更内容 |
|---|---|
| 新增 | 第 5 章 API 接口契约；第 10/14/15/16/19/20 章 |
| 补全 | 第 6 章补字段级数据库 DDL、索引策略、知识状态机、文档入库幂等与失败重试 |
| 细化 | 第 7 章混合检索管线；第 8 章图谱；第 9 章智能体 |
| 补强 | 第 1/2/3 章 |
| 调整 | 第 17 章每阶段补 DoD |

### 0.7 关键设计基线（不可动摇的约束）

```text
1. PostgreSQL 是官方知识唯一可信源；本地向量层（LocalVectorStore，Zvec / sqlite-vec 等为可替换实现）仅为客户端本地辅助层，不得作为官方知识主库。
2. 权限判断全部在后端完成；前端仅展示权限结果；权限为动作级 RBAC（查/传/改/管理）+ 最低限度 sensitivity 字段（public/internal/restricted）控制离线/导出能力；写操作后端二次校验（一期不做按部门的数据级可见性过滤）。
3. 未审核、已弃用的知识，绝不进入模型上下文（检索只召回 status='active'）。
4. 答案必须基于检索证据生成（证据约束生成），并附带可追溯引用；安全相关结论需通过蕴含校验。
5. 知识发布、知识弃用、文件写入、命令执行、敏感设备接口等高风险写操作必须人工确认（HITL）；诊断/检索/问答只读链无 HITL。
6. 当前阶段智能体不得直接控制设备。
7. 一期优先保证 Windows 桌面工作台；前端按 Web-first 组织，预留轻量 Web / 移动现场速查入口。
8. API、数据库、模型、图谱本体和评测指标先保留设计方向，详细实现下沉专项文档（见附录 A）。
9. RAG / 诊断 / 安全规程主链路不得把内部知识发送到外部模型 API；测试期外部 API 仅允许使用无关、脱敏、非涉密数据。"域内模型"指模型推理在本组织物理或逻辑隔离的服务器/集群上完成，不存在推理数据离开组织信任边界的路径。
10. 安全规程类知识必须在线校验；离线知识包不得直接生成最终安全处置结论。`required_online_check` 控制在线状态下是否强制回 PG 校验最新版本（true=不可用本地缓存）；离线状态下无论该字段值如何，一律走 offline-safety 降级。
```

### 0.8 团队假设与资源约束

V0.6 显式声明一期资源基线，作为范围裁剪与里程碑周期判断的锚点：

```text
团队规模     ≤5 人（含前后端 + 算法/知识工程 + 兼职运维）
GPU 资源     有限（推理资源紧张，需控制并发与 Reranker 规模）
部署环境     内网 Windows 桌面 + 单机 PG；MinIO 后置，文件存储用本地磁盘 / NAS
据此裁剪：
  - 一期不引入 LocalVectorStore / 离线知识包（推到阶段四以后）
  - 一期不做小雪狐宠物系统（推到阶段四及以后）
  - 一期不做 MinIO 对象存储（文件存储用本地磁盘 / NAS）
  - 一期跑纯在线模式：Tauri 工作台 + Web 现场速查入口 + 后端服务 + 单机 PG
阶段四的本地向量层 / 离线包 / 宠物系统规划保留设计方向，等团队与资源到位后再行启动。
```

> 后续若团队扩张或资源升级，再据 §17 重新评估阶段推进节奏。

---

## 1. 产品定位

FoxOps 是一个面向港口设备运维场景的智能体工作台，核心形态是 Windows Tauri 桌面端，同时预留内网 Web / 移动现场速查入口。

系统不是单纯的知识库问答工具，而是以智能体为核心，将官方维修知识库、知识图谱、设备数据、文档处理、代码辅助、个人记忆和本地缓存能力统一整合到工作台中。Tauri 本质上承载 Web 前端，因此前端按 Web-first 方式组织：业务页面、组件、状态和 API Client 尽量与 Tauri Shell 解耦，使同一套 React 应用后续可部署为内网 Web 服务。

> V0.6 调整：因团队 ≤5 人（§0.8），一期聚焦"在线工作台 + 现场速查入口 + RAG 诊断"主线；小雪狐宠物系统、LocalVectorStore 本地向量层、MinIO 一期不做，相关章节保留方向占位。

### 1.1 核心定位

- **主界面**：Windows Tauri 维修智能工作台。
- **现场入口**：轻量 Web / 移动窄屏入口，优先只做故障码速查、引用查看、安全提示和只读知识浏览。
- **辅助入口**（后置）：小雪狐宠物系统。
- **核心能力**：智能体通过工具调用完成知识检索、故障诊断、文档生成、代码辅助、设备数据查询等任务。
- **知识定位**：知识库不是独立终点，而是智能体的核心能力底座。
- **部署定位**：公司内部使用，优先内网部署；RAG、诊断和安全主链路数据不出域。

### 1.2 目标用户画像

| 角色 | 典型场景 | 核心诉求 |
|---|---|---|
| 维修骨干 / 班组长 | 复盘疑难案例、沉淀经验、指导一线处理 | 能补充案例、能引用历史、能被复用 |
| 运维 / 设备工程师 | 结合设备数据判断趋势与风险 | 知识与现场数据联动 |
| 知识管理员 | 录入手册、审核发布、维护故障码库 | 入库高效、审核可控、版本可追溯 |
| 一线维修人员 | 现场排故、查故障码、对照手册步骤 | 快、准、有出处、移动/现场可用 |
| 系统管理员 | 用户/角色/模型/系统配置 | 权限清晰、可审计、可运维 |

### 1.3 使用场景示例

```text
场景 A 故障码速查：现场扫到 QC101 报 "E-4312"，通过轻量 Web / 移动入口输入故障码，即得含义、可能原因、标准处置、安全提示和引用来源。
场景 B 自然语言排故："起升下降时有异响且电流偏高"，智能体召回相似案例与排查路径。
场景 C 知识沉淀：维修骨干把一次疑难处理写成案例，提交审核后进入正式知识库。
场景 D 文档生成：基于一次诊断对话生成维修记录/工单草稿。
场景 E 数据联动（后期）："QC101 起升最近是否异常？"结合设备数据趋势 + 知识 + 图谱给出风险提示。
```

### 1.4 非目标（Non-goals）

```text
- 不做通用聊天机器人，不追求开放域闲聊能力。
- 不做面向公网的 SaaS；一期只服务公司内网。
- 不在一期做完整移动 App；移动/平板优先提供轻量 Web 只读速查入口。
- 不做设备实时控制 / 下发指令（安全红线）。
- 不在客户端构建"第二官方知识库"；LocalVectorStore 仅作本地辅助与缓存（一期不做）。
- 一期不引入独立图数据库（Neo4j 等），用 PostgreSQL 关系表承载轻量图谱。
- 一期不做 LocalVectorStore / 离线知识包 / 小雪狐宠物系统 / MinIO（团队资源约束，见 §0.8）。
- 不追求 API 框架极限性能（TurboAPI / Python 3.14t 暂不进入主线，见 §19）。
```

### 1.5 成功指标（建议口径）

| 维度 | 指标 | 一期目标参考 |
|---|---|---|
| 检索质量 | 故障码精确命中率 | ≥ 99%（结构化精确匹配） |
| 检索质量 | 自然语言问题 Top-5 召回命中率 | ≥ 85% |
| 答案质量 | 答案引用可追溯率（每条结论有出处） | 100%（无出处不下结论） |
| 答案质量 | 人工抽检答案有用率 | ≥ 80% |
| 效率 | 现场单次排故平均耗时下降 | 相对基线下降 ≥ 30% |
| 可信 | 未发布/已弃用知识进入上下文事件 | 0 |

> 指标为设计期建议值，需在阶段三/五结合真实数据校准。

---

## 2. 总体架构

系统采用"Web-first 前端工作台 + 后端智能体服务 + PostgreSQL 官方知识底座 + 可替换本地向量层（后置）"的分层架构。

```text
FoxOps（一期在线形态）
├── 前端：共享 React Web App + Tauri Shell + 轻量 Web 入口
│   ├── 维修智能工作台
│   ├── 现场速查轻量入口（/field，online-only）
│   ├── 管理员功能入口
│   ├── [后置] 小雪狐宠物系统
│   └── [后置] LocalVectorStore 本地向量层
│
├── 后端：Python 服务
│   ├── API 服务（FastAPI）
│   ├── Agent Runtime 智能体运行时（LangGraph）
│   ├── 知识处理服务（Document Pipeline）
│   ├── 检索服务（混合检索）
│   ├── 知识图谱服务
│   ├── 设备数据联动服务
│   ├── 模型网关（LLM / Embedding / Reranker）
│   └── 权限 / 审核 / 日志服务
│
└── 数据层：PostgreSQL 官方知识底座
    ├── 结构化知识表
    ├── 全文检索索引（PGroonga 优先，附回退）
    ├── pgvector 向量索引
    ├── 模糊索引（pg_trgm）
    ├── 轻量知识图谱关系表
    ├── 版本 / 权限 / 审核 / 反馈
    ├── 会话与消息（conversation / message）
    └── 文档元数据与切片
```

### 2.1 分层职责

| 层 | 职责 | 不负责 |
|---|---|---|
| 前端（React Web App + Tauri Shell） | 交互、展示、本地能力封装、权限结果渲染；同一套页面可复用为内网 Web | 权限判定、官方知识存储 |
| 后端（FastAPI + Agent） | 任务编排、检索、权限判定、知识治理、模型网关 | UI 呈现细节 |
| 数据层（PostgreSQL） | 官方可信知识、索引、版本、审计、业务会话 | 业务编排逻辑 |
| 本地向量层（LocalVectorStore）[后置] | 个人记忆、本地缓存、离线知识包 | 充当官方知识源；一期不做 |

### 2.2 端到端数据流（一次诊断请求）

```text
[Tauri 工作台 / Web 速查入口] 用户输入
   → HTTPS/JWT → [FastAPI 网关] 鉴权 + 限流 + 请求校验
   → [Agent Runtime] 意图识别 → 规划工具调用（只读链，无 HITL）
   → [检索服务] 发布状态过滤（仅 active）→ 结构化/全文/向量/图谱混合召回 → RRF 融合 → Rerank
   → [Model Gateway] 证据约束 Prompt → LLM 生成（SSE 流式）
   → [Agent Runtime] 组装结构化诊断结果 + 引用 + 安全提示
   → SSE → [Tauri 工作台 / Web 入口] 渐进式渲染
   → [conversation/message] 写入业务会话消息（§6.3）
   → [Audit/Log] 记录调用链、命中知识、权限上下文
```

> V0.5 中"LocalVectorStore 写入本地问答缓存 / 个人记忆"为阶段四以后的能力，一期不在线。

### 2.3 技术栈总览

| 层 | 主线选型 | 备注 / 预研 |
|---|---|---|
| 桌面框架 | Tauri | 见 §19 |
| 前端 | React + TypeScript + Tailwind + shadcn/ui + Zustand | Web-first；AI Elements 候选；Bklit UI 暂缓 |
| 动效 | Lottie / Rive | [后置] 用于小雪狐状态 |
| 本地向量层 | LocalVectorStore 抽象接口 | [后置] Zvec / sqlite-vec / LanceDB / Chroma；阶段四启动 |
| API 框架 | FastAPI（主线） | TurboAPI + Python 3.14t 暂缓 |
| ORM / 迁移 | SQLAlchemy / SQLModel + Alembic | 异步驱动 asyncpg |
| 后台任务 | 一套任务队列 + 一套定时器 | 候选 arq / Dramatiq+APScheduler / Celery+Beat |
| 智能体编排 | LangGraph | DeepAgents / Hermes Agent 仅作参考 |
| 文档处理 | LlamaIndex + 自定义解析 | 仅作 parser/chunk helper/retriever wrapper，不接管业务对象模型（见 §9.4 / §23 重申） |
| 数据库 | PostgreSQL 16+ | PGroonga（非硬依赖，见 §7.4）+ pgvector + pg_trgm + tsvector 回退 |
| 文件存储 | 本地磁盘 / NAS（一期） | MinIO 推到后期 |
| 模型接入 | 统一模型网关 | 内网模型优先 |
| API 类型共享 | FastAPI OpenAPI → openapi-typescript 自动生成 | 见 §5.6 / §18 |

---

## 3. 前端设计

### 3.1 前端技术选型

| 项目 | 选型 |
|---|---|
| 桌面框架 | Tauri |
| 前端语言 | TypeScript + Tailwind CSS + shadcn/ui |
| UI 框架 | React |
| AI 对话组件 | AI Elements 优先；assistant-ui / prompt-kit 备选；PoC 不通过则自建 |
| 状态管理 | Zustand |
| UI 风格 | 商务简约、工作台气质、信息密度适中 |
| 本地向量层 [后置] | LocalVectorStore 接口，本地实现可替换；一期不做 |
| 本地文件能力 | Tauri Commands / Sidecar |
| 动效 [后置] | Lottie / Rive，用于小雪狐宠物状态；一期不做 |
| 数据请求 | TanStack Query + 统一 API Client（基于 OpenAPI 生成 typed client，§5.6） |
| 流式渲染 | SSE 客户端（EventSource / fetch stream） |

前端组织原则：

```text
共享层：React 页面 / 组件 / hooks / Zustand store / API Client / SSE Client
桌面层：Tauri Shell / 本地文件 / [后置] 本地向量层 / [后置] 桌面宠物 / 系统托盘
Web 层：内网 Web 部署 / 移动窄屏速查 / 只读知识入口
```

### 3.2 前端主界面：维修智能工作台

工作台是桌面端用户的主要使用界面，采用"左侧常驻导航 + 中央对话/结果区 + 底部固定输入框 + 右侧悬浮辅助卡片"布局。

```text
维修智能工作台
├── 左侧常驻导航区
│   ├── 工作区
│   ├── 知识区
│   ├── 智能体区
│   ├── 个人区
│   └── 管理区（按权限显示）
│
├── 中央对话与结果区
│   ├── 对话消息流
│   ├── 结构化诊断报告
│   ├── 安全提示卡
│   ├── 候选根因卡
│   ├── 排查步骤卡
│   ├── 相似案例卡
│   └── 来源引用卡
│
├── 底部固定输入区
│   ├── 自然语言输入
│   ├── 故障码速查
│   ├── 设备上下文选择
│   ├── 模型 / 模式状态
│   └── 发送 / 停止 / 附件入口
│
└── 右侧悬浮辅助卡片
    ├── 知识来源
    ├── 相关部件
    ├── 图谱关联
    ├── [后置] 历史缓存命中（LocalVectorStore）
    └── 点击展开为详情面板
```

右侧辅助区参考 Codex / Claude Desktop 的轻量侧边信息形态：默认悬浮小卡只展示摘要/状态；点击"查看详情"展开为侧栏或抽屉，展示完整引用、图谱路径、工具调用、检索命中信息。

### 3.3 页面与路由清单

| 路由 | 页面 | 主要能力 | 最低权限 |
|---|---|---|---|
| `/chat` | 对话页面 | 智能体对话、诊断结果、引用、工具调用 | 只读用户 |
| `/conversations` | 最近对话记录 | 历史会话、收藏、继续对话、导出 | 普通用户 |
| `/knowledge/*` | 知识库 | 服务器知识库、故障代码、故障案例、文档资料 | 只读用户 |
| `/graph` | 知识图谱 | 图谱浏览、节点详情、关系路径、引用跳转 | 只读用户 |
| `/tools` | 工具 | 工具列表、工具配置、工具调用记录 | 普通用户 |
| `/agents` | 智能体 | 智能体配置、角色、工具集、提示词版本 | 知识/系统管理员 |
| `/models` | 模型管理 | LLM / Embedding / Reranker 配置与用量 | 系统管理员 |
| `/profile` | 个人信息 | 活跃情况、Token 消耗、个人设置 | 普通用户 |
| `/admin/dashboard` | 数据总览 | 系统指标、知识入库、模型用量、用户活跃 | 系统管理员 |
| `/admin/users` | 用户管理 | 用户、部门、角色、权限 | 系统管理员 |
| `/admin/system` | 系统设置 | 系统参数、文件目录、日志、备份 | 系统管理员 |
| `/field`（独立轻量入口） | 现场速查 | 故障码速查 + 含义/可能原因/标准处置 + 引用查看 + 安全提示（在线校验）+ 只读知识浏览 | 只读用户 |

> V0.6 删除：`/mcp`、`/skills`、`/pet`、本地知识库/下载到本地、`/field` 中本地向量/离线相关能力。`/mcp`、`/skills` 后续再扩展。

导航原则：左侧导航只放一级主模块。模块内部存在多个子页面时，优先用页面顶部 Tabs / Segmented Control；子功能很多时可在内容区内使用局部二级侧栏。管理员功能按权限显示，前端隐藏入口不构成安全边界。

现场速查入口（`/field`）契约：独立的轻量前端（Web / 移动窄屏，见 §18 `field-entry/`），与桌面工作台共用同一套后端 API 与 JWT 鉴权——必须登录后使用，不提供内网匿名访问；登录后按 §3.5 的 `permissions` 能力清单裁剪为只读能力面（故障码速查、引用查看、安全提示、只读知识浏览），不暴露写操作与管理入口。鉴权与权限判定一律在后端完成，入口裁剪不构成安全边界。该入口为 online-only（见 §3.4 端形态约束）。

### 3.4 关键交互态规范

为避免"只画了正常态"，前端每个数据视图须实现以下状态：

```text
loading        骨架屏 / 进度（区分首屏与流式增量）
streaming      结果分块渐进渲染，可中断（Stop）
empty          无结果引导（给出改写建议 / 缩小范围提示）
error          可重试，展示 trace_id 便于排查
partial        部分工具失败时降级展示已得到的证据
no-perm        访问受限动作/页面时提示"无权限"，不展示该功能；命中未发布/已弃用知识时提示并不展示内容
offline        [后置] 离线/内网中断时切换 LocalVectorStore 本地结果，普通知识明确标注"可能不是最新版"，并在恢复联网后触发与 PG 的版本校验（一期桌面端断网即报错，无离线兜底）
offline-safety [后置] 离线且命中安全规程时的强降级子态：不输出最终安全结论，仅展示"需联网确认 / 请人工复核"
model-down     模型网关不可用时，返回检索结果 + 引用列表，不做总结，标注"模型暂不可用"（见 §14.4 模型降级矩阵）
```

> 端形态约束：`offline` / `offline-safety` 仅适用于桌面端（Tauri）[后置]。一期桌面端断网即不可用（无 LocalVectorStore 兜底）。轻量 Web / 移动现场速查入口运行在浏览器中，本质 online-only，断网态只展示"网络不可用，请重试"。
>
> `required_online_check` 语义明确（V0.6 修订）：控制**在线状态下**是否强制回 PG 校验最新版本（true=不可用本地缓存）；**离线状态下**无论该字段值如何，一律走 offline-safety 降级（基线 #10 不变）。该语义与 §7.4 / §13.4 一致。

### 3.5 前端权限设计

前端根据账号角色显示不同功能，但权限判断必须由后端完成。前端隐藏入口只是体验优化，不构成安全边界（见 §13）。一期权限是**动作级 RBAC + 最低限度 sensitivity 字段**——按角色控制"能做哪些动作"（查 / 传文档 / 改知识库 / 管理），并用 `sensitivity`（public/internal/restricted）控制离线/导出能力（一期不做离线，因此 sensitivity 仅用于导出权限）；不做按部门的数据级可见性区分（所有可读用户看到同一份已发布 active 知识）。

| 角色 | 能做的动作 |
|---|---|
| 只读用户 | 只能查（查询、查看已发布知识与引用）；不可查看 restricted |
| 普通用户 | 查 + 问答、收藏、反馈 |
| 维修骨干（高级工） | 普通用户能力 + 上传文档、案例补充、经验提交 |
| 知识管理员 | 知识录入、编辑、审核、发布、弃用 |
| 系统管理员 | 用户、角色、模型、系统配置 |

前端实现要点：登录后从后端拉取 `permissions` 能力清单，用于渲染；所有受控操作发起时仍由后端二次校验；**UI 不得在本地缓存可绕过的权限判定逻辑**；`/field` 按只读能力面渲染。

> 角色变更后，由后端发起 JWT 失效（§13.5）；前端只需在收到 401 时刷新 token 或重新登录。

### 3.6 小雪狐宠物系统 [后置]

> V0.6：受团队资源约束（§0.8），本期不做小雪狐宠物系统。以下保留设计方向占位，待团队扩张或资源升级后再启动。

设计方向（占位）：桌面常驻形象、状态反馈、轻量提醒、双击打开主界面、由后端 SSE 事件驱动的状态机（idle/listening/thinking/retrieving/generating/done/notify/risk/error）；不承载复杂信息、不替代工作台、不过度卡通化。完整设计独立出一份产品设计稿，不进架构主文档。

---

## 4. 后端设计

### 4.1 后端技术选型

| 项目 | 推荐选型 |
|---|---|
| 语言 | Python |
| API 框架 | FastAPI 为主线 |
| 实验方向 | TurboAPI + Python 3.14t 暂不进入主线 |
| 数据库 | PostgreSQL |
| ORM | SQLAlchemy / SQLModel |
| 数据迁移 | Alembic |
| 异步驱动 | asyncpg |
| 后台任务 | 一套任务队列 + 一套定时器 |
| 文档处理 | LlamaIndex + 自定义解析流程（仅作工具箱，不接管业务对象模型，§9.4） |
| 智能体编排 | LangGraph 为主 |
| 候选/参考 | DeepAgents / Hermes Agent 待确认，不作为一期核心依赖 |
| 模型接入 | 统一模型网关 |
| 文件存储 | 本地文件系统 / NAS（一期）；MinIO 后置 |
| 配置管理 | pydantic-settings（环境分层：dev/staging/prod） |
| 缓存/队列 | Redis（会话、限流、任务队列 broker、检索热点缓存、JWT 黑名单 §13.5） |
| API 类型共享 | FastAPI OpenAPI spec → `openapi-typescript` → 前端 typed client（见 §5.6） |
| 检索后端抽象 | `SearchBackend` Protocol（§7.4），避免硬依赖 PGroonga，支持 trgm/tsvector+zhparser/pgroonga 切换 |

### 4.2 FastAPI 与 TurboAPI 的定位

一期使用 FastAPI 作为主 API 框架。FastAPI 生态成熟、文档丰富、生产实践多，适合快速搭建稳定后端。TurboAPI 与 Python 3.14t 暂不进入主线——项目早期核心风险在知识建模、权限、检索、文档处理、智能体流程和桌面体验，不在 API 框架的极限性能。

```text
主线：Python + FastAPI
暂缓：Python 3.14t + TurboAPI
后期：出现明确性能瓶颈后再评估迁移
```

智能体与知识处理框架定位：

```text
FastAPI      对外 API、鉴权、权限、SSE、后台任务入口
LangGraph    Agent Runtime 主线（只读链 / 写操作链分离，§9.3）
LlamaIndex   文档解析、切片、检索封装的工具箱；不接管业务对象，业务主模型自建（§9.4）
```

### 4.3 后端服务模块

```text
FoxOps Server
├── Auth Service：登录、角色、权限、Token + JWT 失效（§13.5）
├── Conversation Service：会话与消息（§6.3 conversation/message/agent_checkpoint）
├── Knowledge Service：知识录入、编辑、审核、发布、版本、状态机回滚（§6.5）
├── Search Service：结构化查询、全文检索（PGroonga 优先 + 回退）、向量检索、混合召回
├── Graph Service：知识图谱节点、关系、路径查询
├── Agent Runtime：任务理解、工具调用、流程编排（只读链 / 写链）
├── Document Pipeline：上传、解析、跳步 stage（§6.4）、向量化
├── Model Gateway：LLM、Embedding、Reranker、Prompt 模板 + 降级矩阵（§14.4）
├── Embedding Registry：模型注册与索引可用性（§6.3 embedding_model_registry/embedding_index）
├── Device Data Adapter：设备数据采集系统接口（后置）
├── Feedback Service：有用/无用、已解决/未解决、案例沉淀
└── Audit & Log Service：调用日志、知识变更、权限审计
```

模型网关出域规则：

```text
诊断 / RAG / 安全规程主链路
  只能使用域内模型；不得把内部知识片段、未发布知识发送到外部 API。
"域内模型"指模型推理在本组织物理或逻辑隔离的服务器/集群上完成，
  不存在推理数据离开组织信任边界的路径（§0.7 基线 #9）。

测试 / 开发联调
  可临时接网络 API 服务，但只能使用无关、脱敏、非涉密数据；
  测试环境必须清楚标注"非生产知识"。

辅助能力
  代码辅助、通用文案润色、空上下文聊天等不涉密能力，
  可在用户显式知情后使用外部 API；
  Prompt 中禁止注入内部知识库证据和内部机密内容。
```

### 4.4 服务边界与部署形态

一期建议**单体可分进程**：业务模块在同一代码库内按目录解耦（见 §18），但运行时可拆为三类进程，便于横向扩展与故障隔离。

```text
进程类型
├── api          FastAPI 在线服务（无状态，可多副本）
├── worker       任务队列 worker（入库、解析、向量化、抽取、重建索引）
└── scheduler    定时任务（缓存预热、健康巡检）

共享依赖：PostgreSQL（主）、Redis（会话/队列/缓存/JWT 黑名单）、文件存储（本地/NAS）、Model Gateway
```

横切关注点统一以中间件/依赖注入实现：请求级 `trace_id`、鉴权上下文、限流、超时、统一异常处理、审计埋点。

---

## 5. API 接口契约

> V0.6 修订：本章在设计原则基础上补 TypeScript 类型定义（§5.6），作为前后端并行的硬契约。完整端点、请求响应 schema、错误码细节下沉到《FoxOps_API_契约.md》（见附录 A）。

### 5.1 通用约定

```text
Base URL     /api/v1
传输          HTTPS（内网亦启用 TLS）
编码          UTF-8，JSON（multipart 仅用于文件上传）
命名          字段 snake_case；资源名复数（/fault-codes、/documents）
时间          ISO 8601、UTC（如 2026-06-26T08:30:00Z）
追踪          每个请求/响应带 X-Trace-Id；错误体回显 trace_id
幂等          写操作支持 Idempotency-Key 头（入库/上传/发布）
版本          路径版本 /api/v1；破坏性变更升 v2
```

### 5.2 鉴权与会话

```text
POST /api/v1/auth/login         账号密码 → { access_token, refresh_token, expires_in }
POST /api/v1/auth/refresh       refresh_token → 新 access_token（角色变更后被强制刷新，§13.5）
POST /api/v1/auth/logout        失效当前会话
GET  /api/v1/auth/me            当前用户 + 角色 + 能力清单(permissions)

鉴权方式：Authorization: Bearer <access_token>（JWT），短时 access + 可吊销 refresh
令牌内含 user_id、roles；但具体资源授权一律后端二次判定（不信任前端）。
角色变更后服务端将旧 access_token 加入 Redis 黑名单，迫使客户端 refresh 拿新 claims（§13.5）。
```

### 5.3 统一响应包络

成功：

```json
{
  "data": { "...": "业务数据" },
  "meta": { "trace_id": "b7f...", "elapsed_ms": 128 }
}
```

列表（游标分页）：

```json
{
  "data": [ { "id": "..." } ],
  "meta": { "trace_id": "...", "next_cursor": "eyJpZCI6..." , "has_more": true, "total": 137 }
}
```

错误：

```json
{
  "error": {
    "code": "KNOWLEDGE_NOT_PUBLISHED",
    "message": "该知识尚未发布，无法引用",
    "details": { "knowledge_id": "kc_123" },
    "trace_id": "b7f..."
  }
}
```

### 5.4 错误码与 HTTP 语义

| HTTP | error.code（示例） | 含义 |
|---|---|---|
| 400 | `INVALID_ARGUMENT` | 参数校验失败 |
| 401 | `UNAUTHENTICATED` | 未登录 / Token 失效 / Token 被吊销 |
| 403 | `PERMISSION_DENIED` | 无权限访问资源 / 操作 |
| 403 | `KNOWLEDGE_NOT_PUBLISHED` | 命中未发布/已弃用知识，拒绝进入上下文 |
| 403 | `SENSITIVITY_DENIED` | 受限资源被无权限用户访问 / 尝试导出 restricted |
| 404 | `NOT_FOUND` | 资源不存在 |
| 409 | `CONFLICT` / `VERSION_CONFLICT` | 并发写冲突 / 版本不一致 |
| 422 | `UNPROCESSABLE` | 语义校验失败（如审核流转非法、状态机非法迁移） |
| 429 | `RATE_LIMITED` | 触发限流（返回 Retry-After） |
| 499 | `CLIENT_CLOSED` | 客户端中断流式请求 |
| 500 | `INTERNAL` | 服务内部错误 |
| 503 | `MODEL_UNAVAILABLE` | 模型网关不可用 / 降级（见 §14.4 降级矩阵） |

### 5.5 分页、过滤、排序

```text
列表参数      ?limit=20&cursor=<opaque>&sort=-updated_at&status=active&q=关键字
约束          limit 默认 20、上限 100；cursor 不透明、由服务端签发
过滤白名单     每个端点声明可过滤字段，禁止任意字段注入
```

### 5.6 TypeScript 类型契约（V0.6 新增）

> 前后端通过 FastAPI OpenAPI spec 自动生成 typed client，**不再手写两套类型**（§18）。以下类型作为契约锚点，由后端 Pydantic schema 映射，前端用 `openapi-typescript` 生成。具体生成流程见《FoxOps_API_契约.md》。

#### 5.6.1 Citation / Evidence / SafetyRuleCheckResult

```typescript
type Citation = {
  document_id: string;
  document_title: string;
  section_id?: string;
  section_label?: string;       // 如 "6.3"
  page?: number;
  version: string;              // 知识版本号，用于回看历史时回答"基于哪版知识"
  chunk_id?: string;
};

type EvidenceSource = "chunk" | "fault_code" | "case" | "safety_rule" | "graph";

type Evidence = {
  source_type: EvidenceSource;
  source_id: string;
  version: string;                        // 知识版本锁定（§ #10）
  status_at_retrieval: "active";          // 仅 active 可被召回（基线 #3）
  citation: Citation;
  content: string;
  score?: number;
  sensitivity: "public" | "internal" | "restricted";   // §13.3，导出/离线影响
};

type SafetyRuleCheckResult = {
  rule_id: string;
  title: string;
  status: "active" | "deprecated" | "missing";  // deprecated/missing → offline-safety 降级
  required_online_check: boolean;
  checked_at: string;                             // ISO 8601
  can_generate_final_advice: boolean;             // false → 不输出最终安全处置结论
};
```

#### 5.6.2 DiagnoseEvent（SSE discriminated union）

```typescript
type DiagnosePhase = "intent" | "retrieving" | "reranking" | "generating" | "assembling_done" | "done";

type DiagnosisCard =
  | { type: "safety";     payload: SafetyRuleCheckResult }
  | { type: "root_cause"; payload: { causes: Array<{ id: string; title: string; confidence?: number; evidence_ids: string[] }> } }
  | { type: "steps";      payload: { steps: Array<{ ordinal: number; action: string; tools?: string[]; evidence_ids: string[] }> } }
  | { type: "cases";      payload: { cases: Array<{ id: string; title: string; score?: number; citation: Citation }> } };

type DiagnoseEvent =
  | { type: "status";   trace_id: string; phase: DiagnosePhase;              message?: string }
  | { type: "evidence"; trace_id: string; evidence: Evidence[] }
  | { type: "token";    trace_id: string; delta: string }
  | { type: "card";     trace_id: string; card: DiagnosisCard }
  | { type: "error";    trace_id: string; code: string; message: string; recoverable: boolean }
  | { type: "done";     trace_id: string; result_id: string; conversation_id: string; message_id: string };
```

> SSE 统一使用 `event: diagnose` + `data: <DiagnoseEvent>` 做分发，**不再每类型一个 event 名**。客户端按 `data.type` 分支处理（discriminated union）。

### 5.7 流式协议（SSE）

诊断、文档生成等长任务使用 Server-Sent Events，事件固定：

```text
event: diagnose
data: { "type": "status",   "trace_id": "...", "phase": "retrieving" }
data: { "type": "evidence", "trace_id": "...", "evidence": [...] }       # 召回证据，先于生成下发
data: { "type": "token",    "trace_id": "...", "delta": "起升机构..." }
data: { "type": "card",     "trace_id": "...", "card": { "type": "safety", "payload": {...} } }
data: { "type": "done",     "trace_id": "...", "result_id": "res_123" }
data: { "type": "error",    "trace_id": "...", "code": "MODEL_UNAVAILABLE", "recoverable": false }
```

客户端须支持 `Stop`（断开连接即取消），服务端检测断开后停止生成并记审计（`CLIENT_CLOSED`）。诊断 / 问答 / 检索均为**只读链，无 HITL**（§9.3）；只有写操作链（发布/弃用/文件写入/设备敏感接口）走 HITL。

### 5.8 核心端点清单

| 方法 | 路径 | 说明 | 流式 | HITL |
|---|---|---|---|---|
| POST | `/agent/diagnose` | 提交诊断任务（自然语言/故障码/设备上下文） | SSE | 否（只读链） |
| POST | `/agent/tasks/{id}/cancel` | 取消任务 | — | 否 |
| POST | `/agent/messages/{id}/export` | 导出诊断结果 / 工单草稿 | — | 是（导出 restricted 需确认） |
| GET | `/fault-codes` | 故障码检索（结构化 + pg_trgm） | — | 否 |
| GET | `/fault-codes/{code}` | 故障码详情（原因/措施/安全来自关系表） | — | 否 |
| GET | `/search` | 通用混合检索（手册/案例/规程） | — | 否 |
| GET | `/cases` / `POST /cases` | 案例检索 / 提交（骨干+，进审核流） | — | 提交否，发布是 |
| GET | `/documents` / `POST /documents` | 文档列表 / 上传（异步入库，stage 跳步） | — | 上传否 |
| GET | `/documents/{id}/status` | 入库流水线状态（ingestion_job） | — | 否 |
| GET | `/graph/relations` | 图谱关系/路径查询 | — | 否 |
| GET | `/devices/{id}/metrics` | 设备数据查询（后期） | — | 是（敏感接口） |
| POST | `/feedback` | 反馈提交 | — | 否 |
| POST | `/admin/knowledge/{id}/review` | 审核流转（pending→active/deprecated） | — | 是（发布需确认） |
| POST | `/admin/knowledge/{id}/reactivate` | deprecated→active 状态机回滚（§6.5） | — | 是 |
| GET | `/admin/audit` | 审计查询（管理员） | — | 否 |

> 全部写操作端点支持 `Idempotency-Key`。审计写 `audit_log`。

### 5.9 典型请求示例

诊断（请求）：

```http
POST /api/v1/agent/diagnose
Authorization: Bearer <token>
Idempotency-Key: 5f1c...
Content-Type: application/json

{
  "query": "起升下降时有异响且电流偏高",
  "context": { "equipment_id": "QC101", "fault_code": null, "mode": "diagnose", "conversation_id": "conv_xxx" },
  "options": { "stream": true, "max_cases": 5 }
}
```

故障码检索（响应）：

```json
{
  "data": [{
    "code": "E-4312",
    "title": "起升变频器过流",
    "severity": "high",
    "equipment_types": ["QC"],
    "probable_causes": ["编码器反馈异常", "电机绕组短路"],
    "standard_actions": ["断电检查编码器", "测量绕组绝缘"],
    "safety_notes": ["高压作业，先验电挂牌"],
    "safety_rules": [{"rule_id":"sr_7","title":"变频器检修断电规程","required_online_check":true}],
    "citations": [{"document_id":"doc_88","section_label":"6.3","version":"v3"}],
    "status": "active"
  }],
  "meta": { "trace_id": "...", "has_more": false }
}
```

---

## 6. 数据与知识库设计

> V0.6 修订：在 V0.5 DDL 基础上补业务会话表（§6.3）、Embedding 锚点表（§6.3）、`sensitivity` 字段（§13.3）、CHECK/UNIQUE 约束（§6.3）；§6.4 入库 stage 支持跳步；§6.5 状态机补 rollback。完整 DDL、索引参数、迁移脚本下沉到《FoxOps_数据库设计.md》（附录 A）。

### 6.1 数据存储原则

PostgreSQL 是官方知识的唯一可信数据源。本地向量层（LocalVectorStore）是客户端本地辅助层，不作为官方知识主库；一期不引入。

| 数据类型 | 存放位置 | 说明 |
|---|---|---|
| 源 PDF / Word / Excel | 文件存储：本地磁盘 / NAS（一期）/ MinIO（后置） | PostgreSQL 只存路径、hash、版本 |
| 文档元数据 | PostgreSQL `document` | 文件名、来源、版本、上传人、状态、sensitivity |
| 文档目录 | PostgreSQL `document_section` | 手册章节、层级结构 |
| 文档切片 | PostgreSQL `knowledge_chunk` | 检索和引用的最小文本单元 |
| 全文索引 | PostgreSQL 扩展（非硬依赖） | PGroonga 优先 / zhparser+tsvector 回退（§7.4） |
| 模糊索引 | PostgreSQL `pg_trgm` | 故障码、设备编号、部件名模糊匹配 |
| 向量 | PostgreSQL `pgvector` | 存在 `knowledge_chunk.embedding` / `fault_case.embedding` |
| 结构化知识 | PostgreSQL 业务表 | 故障码、设备、部件、案例、措施 |
| 知识图谱 | PostgreSQL 节点表 / 关系表 | 一期轻量图谱 |
| 业务会话与消息 | PostgreSQL `conversation` / `message` / `agent_checkpoint` | §6.3 业务会话表 |
| Embedding 模型注册 | PostgreSQL `embedding_model_registry` / `embedding_index` | §6.3 锚点表 |
| [后置] 用户本地记忆 | 客户端 LocalVectorStore | 阶段四 |
| 操作日志 | PostgreSQL | 审计与追溯 |

文件存储方案说明：

| 方案 | 优点 | 缺点 | 建议 |
|---|---|---|---|
| 本地磁盘 | 简单、低成本 | 多副本共享麻烦 | **一期采用** |
| NAS | 内网共享 | 依赖企业存储环境 | 一期可选用 |
| MinIO | 标准对象存储 | 运维复杂 | **后置**（V0.6 砍掉一期 MinIO） |

### 6.2 核心数据表

第一阶段建议先建设以下核心表（V0.6 在 V0.5 基础上新增 conversation / message / agent_checkpoint / embedding_model_registry / embedding_index）：

```text
document                原始文档元数据（含 sensitivity）
document_section        文档目录与章节
ingestion_job           文档入库作业/阶段状态（stage 支持 skip，§6.4）
knowledge_chunk         文档切片与向量字段
equipment               设备信息
component               部件信息
fault_code              故障码库（含 severity CHECK）
fault_cause             故障原因
fault_code_cause        故障码-原因关系
fault_code_action       故障码-处置措施关系
fault_code_safety_rule  故障码-安全规程关系
fault_case              历史维修案例
maintenance_action      维修措施
safety_rule             安全规程（required_online_check 在此）
knowledge_node          知识图谱节点
knowledge_edge          知识图谱关系
feedback                用户反馈
app_user                用户
role                    角色
permission              权限
user_role               用户-角色
role_permission         角色-权限
audit_log               审计日志
conversation            业务会话（V0.6 新增）
message                 业务消息（V0.6 新增）
agent_checkpoint        LangGraph 图执行检查点（V0.6 新增）
embedding_model_registry  Embedding 模型注册（V0.6 新增）
embedding_index           Embedding 索引可用性（V0.6 新增）
```

### 6.3 字段级 Schema（核心表 DDL）

> 暂定内容：以下 DDL 作为数据库专项设计的初稿参考。完整 DDL、索引、迁移脚本下沉到《FoxOps_数据库设计.md》。约定：所有表含 `created_at/updated_at`（timestamptz）、软删除用 `deleted_at`；状态字段使用知识状态机（§6.5）。
>
> 向量列约定（V0.5）：模型选型前 `embedding` 列不指定维度、不建 HNSW 索引；阶段三确定模型后再 `ALTER` 为 `vector(N)` 并创建 HNSW 索引。同一 `embedding` 列同一时刻只承载一种模型的向量，更换 Embedding 模型按停服窗口整体重建（§14.4）。
>
> V0.6 新增：通过 `embedding_model_registry` / `embedding_index` 承载模型状态与索引可用性（§6.3 末尾），检索时判断 `if not embedding_index_ready: return keyword_search_only(query)`（§7.4 / §14.4）。
>
> V0.6 新增：`document` 与受 sensitivity 影响的知识表追加 `sensitivity` 字段（§13.3）；DDL 补 CHECK / UNIQUE 约束（§ #13）。

#### 6.3.1 文档与切片

```sql
-- 扩展
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS vector;
-- PGroonga 非硬依赖：装不上则用 zhparser + tsvector 回退（§7.4）
-- CREATE EXTENSION IF NOT EXISTS pgroonga;

-- 文档元数据（V0.6：补 sensitivity 和 CHECK 约束）
CREATE TABLE document (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title           text NOT NULL,
  source          text,
  file_path       text NOT NULL,
  file_hash       char(64) NOT NULL,
  mime_type       text,
  version         text NOT NULL DEFAULT 'v1',
  status          text NOT NULL DEFAULT 'draft',   -- draft/pending/active/deprecated/archived
  sensitivity     text NOT NULL DEFAULT 'internal', -- public/internal/restricted（V0.6，§13.3）
  uploaded_by     uuid NOT NULL REFERENCES app_user(id),
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  deleted_at      timestamptz,
  UNIQUE (file_hash, version),
  CONSTRAINT chk_doc_status      CHECK (status IN ('draft','pending','active','deprecated','archived')),
  CONSTRAINT chk_doc_sensitivity CHECK (sensitivity IN ('public','internal','restricted'))
);

-- 文档章节（V0.6：补复合 UNIQUE）
CREATE TABLE document_section (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id     uuid NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  parent_id       uuid REFERENCES document_section(id),
  title           text NOT NULL,
  level           int  NOT NULL DEFAULT 1,
  ordinal         int  NOT NULL DEFAULT 0,
  path_label      text,
  CONSTRAINT uq_section_doc_parent_ord UNIQUE (document_id, parent_id, ordinal)
);
CREATE INDEX idx_section_doc ON document_section(document_id);

-- 入库作业（V0.6：attempts/attempts_max，stage 支持 skipped 状态见 §6.4）
CREATE TABLE ingestion_job (
  job_id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id uuid NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  stage       text NOT NULL,                     -- parse/section/clean/chunk/annotate/index_text/embed/extract/review（按 mime 动态跳步，§6.4）
  status      text NOT NULL DEFAULT 'pending',   -- pending/running/succeeded/failed/skipped
  attempts    int  NOT NULL DEFAULT 0,
  attempts_max int NOT NULL DEFAULT 3,
  last_error  text,
  created_at  timestamptz NOT NULL DEFAULT now(),
  updated_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (document_id, stage),
  CONSTRAINT chk_ing_status CHECK (status IN ('pending','running','succeeded','failed','skipped'))
);
CREATE INDEX idx_ingestion_job_doc ON ingestion_job(document_id, stage);

-- 文档切片（V0.6：补 sensitivity、复合 UNIQUE）
CREATE TABLE knowledge_chunk (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id     uuid NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  section_id      uuid REFERENCES document_section(id),
  ordinal         int  NOT NULL,
  content         text NOT NULL,
  token_count     int,
  embedding       vector,                        -- 选型前不写维度；阶段三 ALTER 为 vector(N)
  embedding_model text,
  embedding_model_version text,
  sensitivity     text NOT NULL DEFAULT 'internal', -- 继承自 document（§13.3）
  status          text NOT NULL DEFAULT 'draft',
  version         text NOT NULL DEFAULT 'v1',
  metadata        jsonb NOT NULL DEFAULT '{}',
  created_at      timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_chunk_doc_ord     UNIQUE (document_id, ordinal),
  CONSTRAINT chk_chunk_status     CHECK (status IN ('draft','pending','active','deprecated','archived')),
  CONSTRAINT chk_chunk_sensitivity CHECK (sensitivity IN ('public','internal','restricted'))
);
-- 全文索引按后端配置动态选择：PGroonga / tsvector+zhparser / pg_trgm（§7.4）
--   PGroonga:    CREATE INDEX idx_chunk_pgroonga ON knowledge_chunk USING pgroonga (content);
--   tsvector:    CREATE INDEX idx_chunk_fts ON knowledge_chunk USING gin (to_tsvector('chinese', content));
CREATE INDEX idx_chunk_status ON knowledge_chunk(status);
-- HNSW 索引：阶段三确定 Embedding 模型、ALTER 定维后才建：
--   ALTER TABLE knowledge_chunk ALTER COLUMN embedding TYPE vector(1024);
--   CREATE INDEX idx_chunk_embedding ON knowledge_chunk USING hnsw (embedding vector_cosine_ops);
```

#### 6.3.2 设备 / 部件 / 故障码（结构化主数据）

```sql
CREATE TABLE equipment (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  code        text UNIQUE NOT NULL,
  name        text NOT NULL,
  type        text,
  metadata    jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_equipment_code_trgm ON equipment USING gin (code gin_trgm_ops);

CREATE TABLE component (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  equipment_id  uuid REFERENCES equipment(id),
  code          text,
  name          text NOT NULL,
  subsystem     text,
  metadata      jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_component_name_trgm ON component USING gin (name gin_trgm_ops);

-- 故障码库（V0.6：补 severity CHECK）
CREATE TABLE fault_code (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  code            text NOT NULL,
  title           text NOT NULL,
  severity        text,                          -- low/medium/high/critical
  equipment_type  text,
  description     text,
  status          text NOT NULL DEFAULT 'active',
  version         text NOT NULL DEFAULT 'v1',
  created_at      timestamptz NOT NULL DEFAULT now(),
  UNIQUE (code, equipment_type, version),
  CONSTRAINT chk_fc_status   CHECK (status IN ('draft','pending','active','deprecated','archived')),
  CONSTRAINT chk_fc_severity CHECK (severity IS NULL OR severity IN ('low','medium','high','critical'))
);
CREATE INDEX idx_fault_code_trgm ON fault_code USING gin (code gin_trgm_ops);

-- 故障原因
CREATE TABLE fault_cause (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title       text NOT NULL,
  description text,
  status      text NOT NULL DEFAULT 'active'
);

CREATE TABLE fault_code_cause (
  fault_code_id uuid NOT NULL REFERENCES fault_code(id),
  cause_id      uuid NOT NULL REFERENCES fault_cause(id),
  confidence    numeric,
  sort_order    int NOT NULL DEFAULT 0,
  PRIMARY KEY (fault_code_id, cause_id)
);

CREATE TABLE fault_case (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  equipment_id  uuid REFERENCES equipment(id),
  fault_code_id uuid REFERENCES fault_code(id),
  symptom       text,
  root_cause    text,
  resolution    text,
  embedding      vector,
  embedding_model text,
  embedding_model_version text,
  sensitivity   text NOT NULL DEFAULT 'internal', -- V0.6
  status        text NOT NULL DEFAULT 'draft',
  submitted_by  uuid REFERENCES app_user(id),
  created_at    timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT chk_case_status     CHECK (status IN ('draft','pending','active','deprecated','archived')),
  CONSTRAINT chk_case_sensitivity CHECK (sensitivity IN ('public','internal','restricted'))
);
-- HNSW 索引同 knowledge_chunk：阶段三 ALTER 定维后再建
CREATE INDEX idx_case_symptom_fts ON fault_case USING gin (to_tsvector('chinese', symptom));

CREATE TABLE maintenance_action (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title       text NOT NULL,
  steps       jsonb NOT NULL DEFAULT '[]',
  tools       jsonb NOT NULL DEFAULT '[]',
  parts       jsonb NOT NULL DEFAULT '[]',
  status      text NOT NULL DEFAULT 'active'
);

-- 安全规程（V0.6：明确 required_online_check 语义，§13.4）
CREATE TABLE safety_rule (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title       text NOT NULL,
  content     text NOT NULL,
  risk_level  text,                                          -- low/medium/high/critical
  required_online_check boolean NOT NULL DEFAULT true,      -- 在线状态下是否强制回 PG 校验最新版本；离线时一律走 offline-safety 降级（§13.4）
  status      text NOT NULL DEFAULT 'active',
  CONSTRAINT chk_sr_status     CHECK (status IN ('active','deprecated','archived')),
  CONSTRAINT chk_sr_risk_level CHECK (risk_level IS NULL OR risk_level IN ('low','medium','high','critical'))
);

CREATE TABLE fault_code_action (
  fault_code_id uuid NOT NULL REFERENCES fault_code(id),
  action_id     uuid NOT NULL REFERENCES maintenance_action(id),
  sort_order    int NOT NULL DEFAULT 0,
  PRIMARY KEY (fault_code_id, action_id)
);

-- 故障码↔安全规程关系表（是否在线校验由 safety_rule.required_online_check 决定）
CREATE TABLE fault_code_safety_rule (
  fault_code_id   uuid NOT NULL REFERENCES fault_code(id),
  safety_rule_id  uuid NOT NULL REFERENCES safety_rule(id),
  PRIMARY KEY (fault_code_id, safety_rule_id)
);
```

#### 6.3.3 反馈与审计

```sql
CREATE TABLE feedback (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id     uuid REFERENCES app_user(id),
  result_id   text,
  rating      text CHECK (rating IS NULL OR rating IN ('useful','useless')),
  resolved    boolean,
  comment     text,
  created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE audit_log (
  id          bigserial PRIMARY KEY,
  trace_id    text,
  user_id     uuid,
  action      text NOT NULL,                  -- knowledge.publish / knowledge.reactivate / file.write / jwt.revoke ...
  target      text,
  perm_context jsonb,
  result      text,                           -- ok/denied/error
  created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_audit_user_time ON audit_log(user_id, created_at);
```

#### 6.3.4 业务会话与消息（V0.6 新增）

> LangGraph checkpoint 只负责恢复图执行状态；业务会话与消息由 `conversation` / `message` 单独承载，通过 `conversation_id` / `trace_id` 关联。

```sql
CREATE TABLE conversation (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id      uuid NOT NULL REFERENCES app_user(id),
  title        text,
  mode         text NOT NULL DEFAULT 'diagnose', -- diagnose / chat / generate
  archived_at  timestamptz,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_conversation_user_time ON conversation(user_id, created_at);

CREATE TABLE message (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  conversation_id uuid NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
  role            text NOT NULL,                    -- user / assistant / tool / system
  content         text NOT NULL,
  metadata        jsonb NOT NULL DEFAULT '{}',      -- 含 evidence_ids / citations / trace_id
  created_at      timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT chk_msg_role CHECK (role IN ('user','assistant','tool','system'))
);
CREATE INDEX idx_message_conversation_time ON message(conversation_id, created_at);

-- LangGraph checkpoint（图执行状态持久化，支持暂停/恢复）
CREATE TABLE agent_checkpoint (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  conversation_id uuid NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
  checkpoint_ns   text NOT NULL DEFAULT 'default',
  checkpoint_id   text NOT NULL,
  state           jsonb NOT NULL,                   -- DiagnoseState（§9.3）
  created_at      timestamptz NOT NULL DEFAULT now(),
  UNIQUE (conversation_id, checkpoint_ns, checkpoint_id)
);
CREATE INDEX idx_checkpoint_conv ON agent_checkpoint(conversation_id, created_at);
```

#### 6.3.5 Embedding 模型注册与索引可用性（V0.6 新增）

> 系统在 stopped-window 整体重建可接受，但需锚点表回答"当前哪个模型是 active、索引是否可用"。检索时据此判断降级（§7.4 / §14.4）。

```sql
CREATE TABLE embedding_model_registry (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  name            text NOT NULL,
  version         text NOT NULL,
  dimension       int  NOT NULL,
  distance_metric text NOT NULL DEFAULT 'cosine',
  status          text NOT NULL DEFAULT 'candidate',   -- candidate / active / deprecated
  created_at      timestamptz NOT NULL DEFAULT now(),
  UNIQUE (name, version),
  CONSTRAINT chk_emr_status CHECK (status IN ('candidate','active','deprecated'))
);

CREATE TABLE embedding_index (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  table_name      text NOT NULL,                       -- knowledge_chunk / fault_case
  column_name     text NOT NULL,                       -- embedding
  model_name      text NOT NULL,
  model_version   text NOT NULL,
  dimension       int  NOT NULL,
  status          text NOT NULL DEFAULT 'building',    -- building / ready / failed / deprecated
  built_at        timestamptz,
  last_error      text,
  created_at      timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT chk_ei_status CHECK (status IN ('building','ready','failed','deprecated'))
);
CREATE INDEX idx_embedding_index_tbl ON embedding_index(table_name, status);
```

> 用户/角色/权限表见 §13.2；知识图谱 `knowledge_node`/`knowledge_edge` 见 §8.3。

### 6.4 文档入库流程（含幂等、失败处理与 stage 跳步）

> V0.6 修订：stage 列表根据 mime_type / 文档质量评估动态决定，纯文本 docx 不需要 OCR/extract。每个 stage 满足：幂等、可重入、失败只影响自己、不依赖内存状态。一期先 5 步（parse → chunk → index_text → embed → review），section/annotate/extract 后置。

```text
源文档上传（带 Idempotency-Key / file_hash 去重）
→ 文件存储落盘（本地磁盘 / NAS）
→ 源文档质量评估（数字版/扫描版/OCR需求/表格/图纸）
→ PostgreSQL 记录 document 元数据（status=draft）
→ [异步 worker] 动态生成 stage 列表（按 mime_type + 质量评估）
     parse → [section?] → [clean?] → chunk → [annotate?] → index_text → [embed（仅当 embedding_index.ready）] → [extract?] → review
     — 纯文本 docx：parse → chunk → index_text → embed → review（5 步）
     — 扫描 PDF：  parse(+OCR) → section → clean → chunk → annotate → index_text → embed → extract → review
→ 每个 stage 写 ingestion_job 一行（pending/running/succeeded/failed/skipped）
→ 各 stage 异步执行：失败仅对该 stage 重试（attempts+1，未达 attempts_max 前 backoff），不重复落盘
→ 全部 stage 完成后 → 人工审核（pending → active）
→ 发布到正式知识库（status=active，方可进入检索/上下文）

幂等与失败：document.status 表示整体发布态；ingestion_job 每个 (document_id, stage) 一行记 status/attempts/last_error；
            file_hash 命中且非 draft 状态的相同 version 跳过重复入库；分布式 worker 据 ingestion_job 判断"哪一步失败、从哪一步续跑"。
```

切片策略暂定（同 V0.5）：

| 内容类型 | 策略 |
|---|---|
| 故障码表 / 参数表 | 优先结构化抽取到故障码、原因、措施、安全规则等关系表；不只纯文本切片 |
| 步骤清单 / 维修流程 | 保持步骤块完整，避免前置安全条件和后续动作切开 |
| 普通段落 | 按章节、语义边界和 token 上限切片，保留页码、章节、标题路径 |
| 接线图 / 爆炸图 / 示意图 | 至少索引图号、图题、图注、所在页和相关部件，保留原图引用跳转 |
| 扫描件 | 先做 OCR 质量评估；低质量 OCR 结果不得进入正式知识库 |

> 待专项研究：维修手册图表处理、OCR 方案、多模态索引、表格抽取准确率、图纸引用交互方式。

### 6.5 知识版本与审核（状态机，V0.6 补 rollback）

```text
                  提交            审核通过          弃用            归档
draft     ───────▶ pending ──────▶ active ────────▶ deprecated ──▶ archived
  ▲                     │                            │
  │      退回(打回)      │                            │ 重新编辑新版本
  └─────────────────────┘                            ▼
                                              创建新版本(version+1)
                                              走正常 draft→pending→active 流
                                  (B) 直接 reactivate
                                      deprecated──审核确认──▶active
                                      需写 audit_log.knowledge.reactivate
约束：
- 仅 active 可进入检索结果与模型上下文；
- pending/deprecated/archived 一律对查询不可见（管理员审核视图除外）；
- 状态流转写 audit_log；publish/deprecate/reactivate 属高风险写操作，需人工确认（§13.5）。
- 版本回滚两种策略（V0.6 新增）：
  A) 间接回滚：创建新版本 v(n+1) 使内容等于旧版 → 走正常 draft→pending→active 流
  B) 直接 reactivate：deprecated → active（需审核确认，写 audit_log）
  默认采用 B；若内容需大改则用 A。
```

> 一期后置（不在本版展开，与 V0.5 §0.2 一致）：结构化主数据（fault_code / fault_cause / maintenance_action / safety_rule）一期由知识管理员维护、默认 `status='active'`，不纳入 draft→pending 审核流；`safety_rule` 的 version 与有效期/强制刷新窗口字段一期不建模——安全在线校验先只判 `status`（是否 active / 未弃用）+ `required_online_check`，"未过期/版本"后续随审核流一起补。

---

## 7. 检索设计

FoxOps 采用"结构化查询 + 全文检索 + 向量检索 + 知识图谱"的混合检索路线（V0.6：PGroonga 非硬依赖，附回退）。

| 检索类型 | 技术 | 适用问题 |
|---|---|---|
| 结构化查询 | PostgreSQL SQL | 故障码、设备、部件、标准措施 |
| 模糊匹配 | pg_trgm | 故障码输错、设备编号近似、部件名模糊 |
| 中文全文检索 | PGroonga 优先，zhparser+tsvector / pg_trgm 回退（V0.6） | 中文手册、案例、工单、规程搜索 |
| 向量检索 | pgvector + HNSW（阶段三定维后建索引） | 自然语言故障描述、相似案例召回 |
| 图谱关系查询 | knowledge_node / knowledge_edge | 关联部件、排查路径、安全风险 |
| [后置] 本地记忆检索 | LocalVectorStore | 阶段四 |

### 7.1 检索路由

```text
故障码问题          → 结构化查询 + pg_trgm
明确关键词问题       → 全文检索 + 结构化过滤
自然语言故障描述     → 全文检索 + 向量检索 + Reranker
关联排查问题         → 知识图谱关系查询 + 手册 / 案例补充
安全风险问题         → 安全规程表 + 图谱风险关系（在线校验 §7.4）
设备数据问题         → 设备数据接口 + 知识库 + 图谱关联
```

路由判定建议由 Agent 意图识别节点产出 `retrieval_plan`（命中哪几路、各路参数、是否需要图谱扩展），而非写死规则；规则仅作兜底。

### 7.2 混合检索管线（V0.6 统一 token 裁剪点）

```text
1. 解析与归一      提取故障码/设备/部件/现象；查询改写（同义词、纠错）
2. 发布状态过滤    所有检索路只召回 status='active'（基线 #3）
3. 多路召回（并行）
   - 结构化（fault_code/equipment/component 精确+pg_trgm）
   - 全文（PGroonga / tsvector+zhparser / pg_trgm，按 SearchBackend 配置 §7.4）
   - 向量（pgvector HNSW，仅当 embedding_index.ready；否则跳过向量路，§14.4）
   - 图谱（命中实体的 1-2 跳关联）
4. 融合排序        RRF 合并多路结果（见 §7.3）
5. 重排            Reranker 对融合 Top-K 精排（不可用时直接返回 RRF Top-K，§14.4）
6. 引用装配        为每条候选回填 document/section/version/sensitivity → citations
7. 上下文裁剪      ★ 统一裁剪点：按 token 预算选证据，去重，保留高分 + 多样性（仅在此处裁剪一次）
8. 缓存写入        query 指纹 → 结果（带知识版本号，便于失效）
```

> V0.6 修订（§ #16）：**token 预算只在此处裁剪一次**。检索管线输出 Top-K evidence（已适配 token 预算）后，Agent generate 节点直接使用，不再二次裁剪——避免双重裁剪造成信息丢失。

### 7.3 融合与重排细节

倒数排名融合对每路排名取倒数加权求和，跨异构检索源稳定有效：

```text
score(d) = Σ_route  w_route * 1 / (k + rank_route(d))     # 经验 k≈60
最终按 score 降序 → 取 Top-K 交给 Reranker 精排
```

- 权重 `w_route` 可按问题类型调整（故障码类提高结构化权重，自然语言类提高向量权重）。
- Reranker 输入 (query, chunk) 对，输出相关性分；只对融合后的 Top-K（如 20→8）精排，控制时延。

### 7.4 权限、时效约束与全文后端抽象（V0.6 修订）

硬规则：

- 检索 SQL 一律带 `status='active'` 过滤，**未发布/已弃用不可被召回**；`sensitivity='restricted'` 资源不进入 export/导出场景（§13.3）。
- 命中安全规程 / 高风险处置 / `safety_rule` 时，必须在线校验（§13.4 `required_online_check` 语义）：
  - **在线状态** + `required_online_check=true` → 强制回 PG 校验最新 `status`；若已 `deprecated` → offline-safety 降级处理（不输出最终处置结论）。
  - **在线状态** + `required_online_check=false` → 允许使用缓存证据。
  - **离线状态**（一期不覆盖；desktop 端 [后置]） → 一律走 offline-safety 降级，无论 `required_online_check` 值。
- 引用缺失（无法回填 document/section/version）的候选不得作为"有出处结论"使用。

**`required_online_check` 语义统一**（V0.6）：该字段控制"在线状态下是否强制回 PG 校验最新版本"。离线状态下无论该字段值如何，一律走 offline-safety 降级（基线 #10 不变）。语义定义在 §3.4 / §13.4 复述一致。

**全文后端抽象（V0.6 新增，§ #18 / § #22）**：PGroonga 不是标准 PG 扩展（Windows/Docker/国产化环境安装有坑），一期不硬依赖。代码统一通过 `SearchBackend` Protocol 访问，配置 `fulltext_backend` 切换：

```python
from typing import Protocol

class SearchBackend(Protocol):
    async def search(self, query: str, limit: int) -> list["SearchHit"]: ...

# 配置项
# fulltext_backend: "trgm" | "tsvector_zhparser" | "pgroonga"
# 检索能力分层：
#   Level 0: LIKE/ILIKE + pg_trgm          （保底，必装）
#   Level 1: tsvector + zhparser            （可选）
#   Level 2: PGroonga                       （正式中文全文增强）
#   Level 3: pgvector + Reranker            （阶段三）
```

代码不硬依赖 PGroonga；安装不可用时自动降级到 Level 1/0。详细信息见《FoxOps_数据库设计.md》。

**filtered-ANN 风险**：pgvector HNSW 叠加 `WHERE status='active'` 属于 filtered-ANN 问题，可能出现召回率下降或延迟退化。HNSW 索引在阶段三确定 Embedding 模型、向量列定维后才创建（§6.3）。该 benchmark 在阶段零以候选模型 + 样本数据预跑，阶段二有真实数据量级后复测，重点验证发布状态过滤后的 Top-K 召回与 P95 延迟。一期权限简化后向量层不承担按部门/密级细粒度 ACL（§13.3 sensitivity 仅在导出/离线场景生效，不参与检索 WHERE）。

### 7.5 检索缓存

```text
键    hash(归一化 query + 路由计划)
      权限为动作级 RBAC，已发布(active)集合对所有可读用户一致，缓存键不含权限维度
值    结果集 + 命中知识的 version 列表
失效  对应知识 version 变更 / 状态流转 → 主动失效（应用层发布事件 / Redis pub/sub）
      TTL 兜底
触发  知识发布/弃用/reactivate 时由 Knowledge Service 发出 invalidated_event（§11.4 缓存失效 [后置]）
```

---

## 8. 知识图谱设计

知识图谱用于表达设备维修知识之间的结构化关系，支撑排查路径推荐、关联部件分析和安全风险提示。

### 8.1 图谱节点

> 知识图谱前端入口保留，普通用户可查看图谱。本体结构先不急于定稿，先梳理知识库文档类别再确定节点类型、关系类型和抽取规则。

```text
设备 / 子系统 / 部件 / 故障码 / 故障现象 / 故障原因 / 维修措施
工具 / 备件 / 安全风险 / 历史案例 / 手册章节 / 设备数据点位
```

### 8.2 图谱关系

```text
设备 包含 子系统          子系统 包含 部件
故障码 对应 故障现象       故障现象 可能由 故障原因
故障原因 建议检查 部件     故障原因 推荐处置 维修措施
维修措施 需要 工具         维修措施 需要 备件
维修措施 涉及 安全风险     历史案例 验证 故障原因
手册章节 支撑 维修措施     设备数据点位 反映 部件状态
```

### 8.3 一期实现方式（PostgreSQL 轻量图谱）

一期使用 PostgreSQL 表实现轻量知识图谱，本体以枚举约束。

```sql
CREATE TABLE knowledge_node (
  node_id     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  node_type   text NOT NULL,
  name        text NOT NULL,
  ref_id      uuid,                  -- 指向业务表主键
  description text,
  source_id   uuid,                  -- 来源文档/案例
  version     text NOT NULL DEFAULT 'v1',
  status      text NOT NULL DEFAULT 'active',
  metadata    jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_node_type_name ON knowledge_node(node_type, name);

CREATE TABLE knowledge_edge (
  edge_id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  from_node_id     uuid NOT NULL REFERENCES knowledge_node(node_id),
  to_node_id       uuid NOT NULL REFERENCES knowledge_node(node_id),
  relation_type    text NOT NULL,
  confidence_level numeric(3,2) DEFAULT 1.0,
  source_id        uuid,
  version          text NOT NULL DEFAULT 'v1',
  status           text NOT NULL DEFAULT 'active',
  metadata         jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_edge_from ON knowledge_edge(from_node_id, relation_type);
CREATE INDEX idx_edge_to   ON knowledge_edge(to_node_id, relation_type);
```

### 8.4 查询模式（PG 内多跳）

一期多跳路径用递归 CTE 即可满足（深度限制 2–3 跳，防爆炸）：

```sql
WITH RECURSIVE path AS (
  SELECT e.from_node_id, e.to_node_id, e.relation_type, 1 AS depth
  FROM knowledge_edge e
  WHERE e.from_node_id = :symptom_node AND e.status='active'
  UNION ALL
  SELECT e.from_node_id, e.to_node_id, e.relation_type, p.depth+1
  FROM knowledge_edge e JOIN path p ON e.from_node_id = p.to_node_id
  WHERE p.depth < 3 AND e.status='active'
)
SELECT * FROM path;
```

### 8.5 抽取与人审 / 关系表→图谱同步（V0.6 占位）

- 实体/关系由入库流水线（LLM + 规则）抽取，写入时带 `confidence_level`。
- 低置信度边进入人审队列；高风险关系（安全风险、设备控制相关）必须人审后 `active`。
- 图谱节点尽量回指业务表 `ref_id`，保证"图谱—知识—原文"三层可追溯。

> V0.6（§ #24 占位）："关系表 vs 知识图谱"为**主—从关系**：业务关系表（fault_code_cause / fault_code_action / fault_code_safety_rule）是权威源；knowledge_node / knowledge_edge 在其上做扩展视图与多跳查询。阶段五前确认同步策略（一次性快照迁移 vs 增量 CDC/trigger/事件总线）。后期需复杂多跳图算法时再扩展 Neo4j（Graph Service 接口层封装，迁移不影响上层）。

---

## 9. 智能体设计

### 9.1 智能体定位

智能体是系统的任务编排核心，负责理解用户任务并调用工具：查询知识库、检索文档、召回案例、查询知识图谱、分析设备数据、生成维修建议、编写文档、辅助写代码、总结报告、记录记忆和提交反馈。

### 9.2 Agent Runtime

```text
LangGraph：核心诊断流程编排（状态机 + 检查点）
  V0.6 拆"只读链（诊断/检索/问答，无 HITL）"与"写操作链（发布/配置/设备，HITL）"
Hermes Agent：暂不作为核心依赖
自研 Tool System：封装 FoxOps 业务工具，带风险分级（§9.4）
```

Kun / Codex 仅作前端交互体验参考，不作为主技术依赖。

### 9.3 LangGraph 状态与节点图（V0.6 拆只读链 / 写链）

以 TypedDict 定义图状态（持久化于 `agent_checkpoint`，支持暂停/恢复）：

```python
from typing import TypedDict, Optional, List

class Evidence:
    source_type: str            # chunk/fault_code/case/safety_rule/graph
    source_id: str
    version: str                 # 知识版本锁定（§ #10）
    status_at_retrieval: str = "active"
    citation: dict
    content: str
    score: Optional[float]
    sensitivity: str = "internal"

class Card(TypedDict, total=False):
    type: str                    # safety / root_cause / steps / cases
    payload: dict

class UserCtx(TypedDict):
    user_id: str
    roles: list[str]

class DiagnoseState(TypedDict, total=False):
    trace_id: str
    conversation_id: str         # 业务会话关联（§6.3）
    user: UserCtx                # 动作级 RBAC
    query: str
    context: dict                # equipment_id / fault_code / mode
    intent: Optional[str]        # fault_code / nl_fault / relation / safety / device
    retrieval_plan: Optional[dict]
    evidence: List[Evidence]
    draft: Optional[str]
    cards: List[Card]
    safety_check_results: List[dict]   # SafetyRuleCheckResult[]
    errors: list[dict]
    # V0.6 移除：pending_action（HITL 节点迁到写操作链，§6.5 / §13.5）
```

#### 9.3.1 只读链（诊断 / 检索 / 问答，无 HITL）—— V0.6 修订（§ #06）

```text
[ intent ]   意图识别
   → [ plan ]              生成 retrieval_plan
   → [ retrieve ]          混合检索（§7，发布状态过滤；统一 token 预算裁剪一次）
   → [ graph_expand ]      可选：图谱关联扩展
   → [ safety_check ]      安全规程 / risk_level 关系检查（在线校验，§7.4 / §13.4）
                          → 命中 safety_rule 时校验 status：deprecated/missing → offline-safety 降级
                          → can_generate_final_advice=false 时阻断最终安全结论
   → [ rerank ]            证据重排与裁剪（注意：此处不再做 token 裁剪，§ #16）
   → [ generate ]          证据约束生成（SSE 流式；执行三步校验 §9.5）
   → [ assemble ]          组装结构化卡片 + 引用
   → [ done ]              写 conversation/message，发出 SSE done 事件
```

> **只读链无 HITL**。诊断/检索/问答的输出直接展示给用户；不需要用户确认"是否输出诊断结果"。若诊断中触发写副作用（如用户主动请求"把这个诊断保存为知识案例"），由前端发起独立写操作链请求。

#### 9.3.2 写操作链（发布 / 配置 / 文件 / 设备，有 HITL）

```text
[ prepare ]  解析写操作类型 + 目标资源
   → [ authorize ]         权限二次校验（§13.2 / §13.3 sensitivity）
   → [ validate ]          业务校验（状态机合法性、幂等、版本）
   → [ hitl ]              ★ 人工确认检查点（LangGraph interrupt，§ #06 修复）
                          → 用户在桌面工作台确认 / 取消（HITL 入口仅限桌面端，见 §13.5）
   → [ execute ]           执行写操作
   → [ audit ]             写 audit_log
   → [ done ]
```

LangGraph 的 `interrupt()` 在 `[hitl]` 节点暂停、保存状态、等待用户决定（秒级或更久），收到决定后从断点恢复，不阻塞线程。生产可叠加重试中间件（指数退避）与内容安全中间件。

> **HITL 入口约束**：人工确认入口仅限桌面工作台（§ #26）；现场 Web / 移动 `/field` 入口不暴露任何写操作 / 确认动作，遇到需 HITL 的请求返回 `PERMISSION_DENIED` 并提示"请在桌面工作台完成"。

### 9.4 工具系统（I/O 契约与风险分级）

所有后端能力封装为智能体工具，**每个工具具有明确输入/输出 schema、权限要求、风险分级与超时**。

```text
fault_code_search       故障码查询              read_only
manual_search           手册全文检索            read_only
case_search             历史案例检索            read_only
vector_search           向量相似召回            read_only
graph_relation_search   知识图谱关系查询        read_only
safety_rule_check       安全规程检查            safety_critical（在线校验）
device_data_query       设备数据查询            device_control → 当前版本禁止注册/调用
                          （一期后置，且即便启用只读，仍标记敏感接口）
document_generate       文档生成                write_local  （导出 restricted 时需 HITL）
code_assistant          代码辅助                read_only
feedback_submit         反馈提交                write_server
knowledge_publish       知识发布/弃用/reactivate  write_server + safety_critical（HITL）
knowledge_review        审核流转                write_server + safety_critical（HITL）
file_write              本地文件写入            write_local  + HITL
command_exec            本地命令执行            write_server + safety_critical + HITL
[后置] local_memory_search    本地记忆检索      read_only
```

**风险分级枚举（V0.6 新增，§ #11）**：

```python
from enum import Enum

class ToolRiskLevel(str, Enum):
    READ_ONLY        = "read_only"          # 可自动执行（只读链）
    WRITE_LOCAL      = "write_local"        # 需用户确认（导出 restricted / 写本地文件）
    WRITE_SERVER     = "write_server"       # 需权限 + HITL
    SAFETY_CRITICAL  = "safety_critical"    # 需在线校验 + HITL（知识发布、命令执行）
    DEVICE_CONTROL   = "device_control"     # 当前版本禁止注册/调用
```

工具契约示例：

```python
# tool: case_search
Input  = {"query": str, "equipment_id": str | None, "top_k": int = 5}
Output = {"cases": [{"id","symptom","root_cause","resolution","score","citations"}],
          "truncated": bool}
Meta   = {"requires_perm": "case.read", "risk_level": ToolRiskLevel.READ_ONLY,
          "timeout_ms": 3000, "side_effect": False}
```

约定：所有 `risk_level >= WRITE_LOCAL` 的工具须走 HITL（§9.3.2 写操作链）；工具失败返回结构化错误，由 Agent 决定降级（partial 结果）而非整体失败。

**LlamaIndex 不接管业务对象**（§ #23）：系统核心数据模型是自建的 `document` / `chunk` / `citation` / `evidence` / `conversation` / `message`，不是 LlamaIndex 的 `Document` / `Node`。LlamaIndex 仅作 parser / chunk helper / retriever wrapper 使用；否则审计、权限、引用、版本将被框架绑架。

### 9.5 证据约束生成（防幻觉，V0.6 三步校验）

```text
原则：
- 仅允许引用 evidence 中的内容下结论；无证据则明确说"知识库未覆盖"，不臆造。
- 生成 Prompt 注入：证据片段（带编号）+ 引用要求（每条结论标注 [n] 对应 citation）。
- 安全结论强制附 safety_rule 出处，并在线确认该 safety_rule 当前 status='active'（已弃用 → can_generate_final_advice=false）。

三步校验流程（V0.6 新增，§ #14，流程结构先固定，模型选型待定）：
1. 引用存在性校验
   - 正则 / 结构化解析生成文本中的 [n] 编号
   - 校验每个 n 是否对应 evidence 列表中的 id
   - 缺失引用 → 标注"需人工核实"，不输出最终结论
2. 安全结论蕴含校验
   - 识别生成文本中的"安全相关结论"（含处置建议、断电、验电挂牌、风险评估等关键词）
   - 对 (evidence_chunk, safety_claim) 用 cross-encoder / NLI 模型判断 evidence 是否真的支撑结论
   - 不支撑 → 降级处理
3. 降级处理
   - 校验失败的安全结论：标注"需人工核实"，不输出最终处置建议
   - 仍返回 evidence + citations 供人工复核，但不作为系统结论展示给用户
   - 写 audit_log 并通过 SSE card.safety 事件下发 can_generate_final_advice=false
```

### 9.6 典型 Agent 流程

诊断流程（只读链，§9.3.1）：

```text
用户描述故障 → 意图识别 → 提取设备/部件/现象/故障码 → 结构化查询 → 全文检索
→ 向量检索（如可用）→ 知识图谱关联 → 安全规则检查 → Reranker 重排
→ LLM 证据约束生成（三步校验）→ 输出结构化维修建议 → 写 conversation/message
→ 用户反馈 → 知识闭环（反馈升级为案例走写操作链审核）
```

文档生成流程（混合：只读检索 + 写操作确认）：

```text
用户提出文档任务 → [只读链]选择模板 + 读取对话/案例/知识库 + 生成文档草稿
→ 用户修改 → [写操作链] 可选提交知识库待审核（HITL）
```

代码辅助流程：

```text
用户提出代码任务 → [只读链] 读取项目上下文 + 生成代码方案 + 生成代码片段 + 解释修改点
→ [写操作链] 可选写入本地文件（HITL 确认）
```

### 9.7 错误、超时与并发

```text
工具超时     单工具超时 → 标记该路失败 → 走 partial 降级；不拖垮整体
模型降级     按降级矩阵（§14.4）：LLM 不可用 → 返回 evidence + citations 不做总结
重试         幂等读类工具自动重试（退避）；写类工具不自动重试，交 HITL
取消         客户端断流 → interrupt 图执行 → 释放资源 → 审计 CLIENT_CLOSED
并发         单请求内多路检索并行；会话隔离（conversation_id + trace_id 贯穿日志）
```

---

## 10. 会话与记忆管理

### 10.1 记忆分层

```text
工作记忆（短期）  当前会话上下文：最近 N 轮 + 当前证据，受 token 预算约束
会话状态（持久）  conversation / message / agent_checkpoint（§6.3）：
                  - conversation/message 负责业务会话与消息回看
                  - agent_checkpoint 负责 LangGraph 图执行状态恢复
                  - 两者通过 conversation_id / trace_id 关联，职责分开
长期记忆          官方侧：知识库本身（PostgreSQL）
                  个人侧 [后置]：LocalVectorStore 本地个人记忆/偏好（阶段四）
```

### 10.2 上下文窗口管理

- 多轮对话保留摘要 + 关键实体（设备/故障码/结论），原始长文不全量回灌。
- 证据按 token 预算选取（高分优先 + 去重 + 多样性）——**统一裁剪点只在检索管线（§7.2 第 7 步）做一次**，生成节点不再二次裁剪（§ #16）。
- 设备上下文（equipment_id 等）作为结构化槽位贯穿会话，避免反复追问。

### 10.3 个人记忆边界 [后置]

- 个人记忆/偏好默认存本地 LocalVectorStore，不上行官方库；涉及内部资料需本地加密（§13.3）。
- 个人记忆可被用户查看与清理；不得污染官方检索结果（仅作个性化补充并标注来源）。
- 一期不实现，方向保留。

---

## 11. LocalVectorStore 本地向量层 [后置]

> V0.6：受团队资源约束（§0.8），本期不引入 LocalVectorStore / 离线知识包。本章保留方向占位与离线权限缺口声明，待阶段四启动。

### 11.1 主要用途

```text
LocalVectorStore 本地层
├── 历史问答缓存
├── 个人记忆
├── 本地个人知识库
├── 离线知识包
├── 高频问题快速召回
├── 相似问题推荐
└── 工作上下文记忆
```

### 11.2 数据边界与离线权限缺口（V0.6 显式声明，§ #07）

- 官方知识以 PostgreSQL 为准；本地仅作缓存。
- **已知缺口——离线权限不可执行**：`LocalVectorStore` 为纯本地存储，**不嵌入权限校验**。离线状态下，所有可读取本地知识包的用户均可查看其中全部内容；拿到机器即读到全部本地知识。
  - 一期离线场景假设"**桌面设备物理安全 + OS 级登录认证**"保护，而非 RBAC 保护。
  - 若后续需要离线角色级可见性差异，须将 LocalVectorStore 升级为支持**加密分区 + 用户级解密**的方案（§11.4）。
- 本地缓存需要版本校验；本地知识必须标注来源；普通离线结果提示"可能不是最新版"。
- **安全规程类结果**：离线状态下不输出最终安全结论，仅展示"需联网确认 / 请人工复核"（基线 #10；`required_online_check` 语义见 §13.4）。
- 涉及内部资料的本地数据需加密；restricted 资料不得进入离线包（§13.3）。

> 该缺口同时写入 §13.6，集中声明。

### 11.3 调用方式 [后置]

```text
Tauri Commands / Sidecar / Native Binding → LocalVectorStore → 返回本地召回结果
```

实现候选（同 V0.5）：Zvec / sqlite-vec / LanceDB / Chroma。阶段四前完成 Windows + Tauri 打包 PoC，确认运行时、依赖打包、索引构建性能与稳定性。PoC 失败时，优先评估 sqlite-vec 作为轻量默认兜底。

### 11.4 离线包 Manifest 契约 [后置]（V0.6 新增，§ #19）

```json
{
  "package_id": "pkg_xxx",
  "version": "2026.07.01",
  "generated_for_user_id": "user_xxx",
  "allowed_until": "2026-08-01T00:00:00Z",
  "contains_safety_rules": true,
  "sensitivity_max": "internal",
  "embedding_model": "bge-m3",
  "embedding_dimension": 1024,
  "content_hash": "sha256:..."
}
```

- `sensitivity_max`：限制包内最高 sensitivity（`restricted` 永不进离线包，§13.3）。
- `contains_safety_rules` 含 safety_rule 时，前端进入离线态时强制走 offline-safety 降级（§3.4 / §11.2）。
- `allowed_until` 到期后强制重新同步；过期离线包不可用。
- 加密密钥来源：使用 **Windows 系统凭据管理器**或**用户密码派生密钥**；用户离职后由服务端吊销 `package_id`、客户端在下次联网时强制清包。
- 阶段四前完成完整加密方案设计，写入《FoxOps_安全权限与离线包.md》。

### 11.5 缓存失效触发机制 [后置]（§ #28）

知识 version 变更 / 状态流转时，由 Knowledge Service 发布失效事件（Redis pub/sub）；客户端在线时订阅并触发本地缓存失效。事件结构举例如下，详见《FoxOps_安全权限与离线包.md》：

```json
{ "event": "knowledge.invalidated", "knowledge_id": "kc_123", "new_version": "v4", "old_status": "active", "new_status": "deprecated" }
```

---

## 12. 设备数据联动 [后置]

> 设备数据联动整体后置到知识库、RAG 与本地知识库稳定之后。当前只保留只读接入、安全边界与未来方向。用户已有设备数据采集平台，并将设备数据汇聚到 TDengine 时序数据库。

### 12.1 接入对象

```text
设备数据采集平台 / TDengine
├── 实时状态 / 历史趋势 / 报警记录 / 故障记录 / PLC 点位 / 运行事件 / 维保记录
```

### 12.2 与知识库结合

```text
用户问：QC101 起升最近是否异常？
→ 查询设备数据 → 分析趋势和报警 → 检索相关故障知识
→ 查询图谱关联部件 → 给出候选风险和排查建议
```

### 12.3 接口契约与安全边界

```text
适配层      Device Data Adapter 优先封装采集平台 / TDengine 查询接口
只读优先    一期仅"读"：状态、趋势、报警、点位；严禁任何控制 / 下发
工具形态    device_data_query(equipment_id, metric, time_range) → 时序点 + 报警摘要
风险分级    ToolRiskLevel.DEVICE_CONTROL 标记；当前版本禁止注册/调用（§9.4）
敏感接口    需权限 + 必要时 HITL；调用全程审计
时效标注    返回数据带采样时间与数据源，供 Agent 判断时效
```

### 12.4 图谱中的设备数据节点

```text
点位 → 反映 → 部件状态
点位异常 → 关联 → 故障现象
故障现象 → 可能由 → 故障原因
```

---

## 13. 安全与权限

### 13.1 权限原则

一期权限为**动作级 RBAC + 最低限度 `sensitivity` 字段**（V0.6）：按角色控制"能不能做某个动作"（查 / 传文档 / 改知识库 / 管理），并用 `sensitivity`（public/internal/restricted）控制离线/导出能力，不做按部门的数据级可见性过滤。

- 权限判断全部在后端完成；
- 前端只展示权限结果（隐藏入口只是体验优化，不构成安全边界）；
- 管理员入口隐藏 ≠ 权限控制；
- 写/管理类动作（上传、编辑、审核、发布、弃用、reactivate、用户/模型配置）在后端按角色二次校验；
- 检索只召回已发布知识（`status='active'`）；`sensitivity='restricted'` 资源不进入 export/导出 / 离线包；
- 未审核、已弃用知识不能进入模型上下文。

### 13.2 RBAC 模型

```sql
CREATE TABLE app_user (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  username      text UNIQUE NOT NULL,
  display_name  text,
  password_hash text NOT NULL,                -- argon2/bcrypt
  is_active     boolean NOT NULL DEFAULT true,
  created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE role (
  id    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  code  text UNIQUE NOT NULL,                  -- readonly/normal/expert/knowledge_admin/sys_admin
  name  text NOT NULL
);

CREATE TABLE permission (
  id    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  code  text UNIQUE NOT NULL                   -- knowledge.read / case.write / knowledge.review / knowledge.publish / knowledge.deprecate / knowledge.reactivate / device.read_sensitive / admin.user_manage / model.config / restricted.read / export.write
);

CREATE TABLE user_role (
  user_id uuid REFERENCES app_user(id),
  role_id uuid REFERENCES role(id),
  PRIMARY KEY (user_id, role_id)
);

CREATE TABLE role_permission (
  role_id       uuid REFERENCES role(id),
  permission_id uuid REFERENCES permission(id),
  PRIMARY KEY (role_id, permission_id)
);
```

角色—能力对照见 §3.5。权限码示例：`knowledge.read` / `case.read` / `case.write` / `knowledge.review` / `knowledge.publish` / `knowledge.deprecate` / `knowledge.reactivate` / `device.read` / `device.read_sensitive` / `restricted.read` / `export.write` / `admin.user_manage` / `model.config`。

身份集成预留：开发期先自建账号密码；企业内网正式部署时预留 OIDC / LDAP / AD 接入位，将企业目录用户组映射到 FoxOps 角色。

### 13.3 `sensitivity` 字段（V0.6 新增，§ #05）

```sql
sensitivity text NOT NULL DEFAULT 'internal'
CONSTRAINT chk_sensitivity CHECK (sensitivity IN ('public','internal','restricted'))
```

**一期用途**（不恢复完整 ACL，不做按部门可见性过滤）：

- **public**：所有可读用户可见；可导出可进离线包（阶段四）。
- **internal**：所有可读用户可见；可导出可进离线包。
- **restricted**：仅持有 `restricted.read` 权限的角色可见；**不进入 export / 离线包**；导出/写入须 `export.write` 权限 + HITL。

> 检索期**不做 sensitivity WHERE 过滤**（避免按部门/密级检索期过滤）；sensitivity 仅在 export/导出/离线包构建/查看受限资源四个动作上生效。

### 13.4 `required_online_check` 分层语义（V0.6 新增，§ #12）

```text
safety_rule.required_online_check 控制在线状态下是否强制回 PG 校验最新版本（true=不可用本地缓存）。
在线状态：
  required_online_check=true   → 必须回 PG 校验 safety_rule.status='active'；deprecated/missing → offline-safety 降级
  required_online_check=false  → 允许使用缓存证据（仍须满足基线 #3：仅 status='active' 的来源可入上下文）
离线状态 [后置]：
  无论 required_online_check 值如何，一律走 offline-safety 降级，不输出最终安全结论（基线 #10 不变）。
```

### 13.5 高风险操作、HITL 与 JWT 失效（V0.6 修订，§ #06 / #17 / #26）

**高风险写操作（必须人工确认）**：

```text
知识发布 / 知识弃用 / 知识 reactivate / 文件写入 / 本地命令执行 / 设备数据敏感接口 / 未来任何设备控制类操作 / 导出 restricted 资源
```

诊断/检索/问答只读链无 HITL（§9.3.1）；写操作链见 §9.3.2。

**HITL 入口约束**：

```text
- 人工确认入口仅限桌面工作台（§ #26）；
- 现场 Web / 移动 /field 入口不暴露任何写操作 / 确认动作，须返回 PERMISSION_DENIED
- 所有 HITL 事件写 audit_log.knowledge.publish / knowledge.deprecate / knowledge.reactivate / file.write / command.exec ...
```

**角色变更后 JWT 失效（V0.6 新增，§ #17）**：

```text
- 角色变更（如 readonly→expert、knowledge_admin→normal）后，
  服务端将该用户已签发的全部 access_token 加入 Redis 黑名单（key: jwt:blacklist:{jti}，TTL=剩余有效期）；
- 受到黑名单影响的请求在 Auth 中间件校验时返回 401 UNAUTHENTICATED，客户端被迫 refresh 拿新 claims；
- 同时设极短 access_token TTL（如 15 分钟）作为兜底；
- 该机制也用于强制下线（admin 主动禁用账号时）。
```

当前阶段不允许智能体直接控制设备；所有高风险操作经 HITL 确认并写 `audit_log`。

### 13.6 离线权限缺口显式声明（V0.6 新增，§ #07）

> 已知缺口——离线权限不可执行：
>
> `LocalVectorStore` 为纯本地存储，**不嵌入权限校验**。离线状态下，所有可读取本地知识包的用户均可查看其中全部内容；拿到机器即读到全部本地知识。一期离线场景假设"桌面设备物理安全 + OS 级登录认证"保护，而非 RBAC 保护。
>
> 若后续需要离线角色级可见性差异，须将 `LocalVectorStore` 升级为支持 **加密分区 + 用户级解密** 的方案（§11.4 Manifest 与密钥方案）。
>
> 该缺口写入 §11.2 / §13.6 两处互相引用，集中声明。

### 13.7 传输与存储安全

```text
传输   全链路 TLS（内网亦启用）；JWT 短时 access + 可吊销 refresh + 角色变更黑名单
存储   口令 argon2；restricted 资料静态加密；对象存储访问签名 URL
密钥   集中密钥管理；客户端本地敏感数据用系统级安全存储/加密
审计   登录、权限变更、知识流转、高风险操作全量留痕，可追溯 trace_id
不出域 RAG / 诊断 / 安全主链路只用域内模型（§0.7 基线 #9）
```

> 数据级管理建议（§ #25 复述）："域内模型"指模型推理在本组织物理或逻辑隔离的服务器/集群上完成，不存在推理数据离开组织信任边界的路径。

---

## 14. 可观测性与运维

### 14.1 日志、指标、追踪

```text
日志   结构化 JSON 日志，统一含 trace_id / user_id / conversation_id / action；分级（info/warn/error）
指标   QPS、时延 P50/P95/P99、检索各路命中率、模型 token 用量、错误率、队列积压、embedding_index 状态
追踪   一次请求贯穿 Agent→工具→检索→模型 的 span（OpenTelemetry 兼容）
审计   audit_log 独立留存，权限/知识/高风险操作可回溯
```

### 14.2 监控与告警

```text
健康检查   /healthz（存活）/readyz（依赖就绪：PG/Redis/对象存储/模型网关/embedding_index.ready）
告警项     模型网关不可用、检索时延超阈、队列积压、入库失败率、磁盘/向量索引异常、embedding_index.status='failed'
看板       在线服务面板 + 知识入库面板 + 模型用量面板
```

### 14.3 内网部署拓扑

内网部署按三段结构理解：客户端层、后端服务层、模型层。

```text
客户端层
└── Tauri 桌面端 / Web 速查入口 / [后置] LocalVectorStore / 本地文件目录
       │ TLS
       ▼
后端服务层
├── 反向代理 / 网关
├── api 多副本（FastAPI，无状态，可横向扩展）
├── worker 池（入库、解析、向量化、抽取、知识包构建）
├── scheduler（定时巡检、缓存预热、任务调度）
├── PostgreSQL / Redis（含 JWT 黑名单）
└── 文件存储（本地磁盘 / NAS；MinIO 后置）
       │
       ▼
模型层
└── Model Gateway → 域内 LLM / Embedding / Reranker
```

容器化（Docker/Compose 或内网 K8s），配置分环境注入。RAG / 诊断 / 安全主链路必须使用域内模型；开发测试期可临时接网络 API 服务，但只能使用无关、脱敏、非涉密数据，并在环境配置中明确标注为测试模型。

**模型部署档位**（待模型选型专项细化）：

| 档位 | 适用条件 | 设计取向 |
|---|---|---|
| 算力不限制 | 有充足 GPU | 优先质量；支持复杂多轮诊断、强 rerank、faithfulness 校验 |
| 普通配置 | 中等 GPU / 少量推理资源 | 检索质量优先，LLM 做证据约束生成；缓存 + 批量 embedding + 轻量 reranker |
| 入门配置 [一期假定] | 算力有限 / 内网 PoC | 先保证结构化故障码 + 全文检索 + 轻量 Embedding；复杂诊断降级，必要时人工复核 |

### 14.4 备份、升级、迁移与降级矩阵（V0.6 新增）

**备份 / 迁移 / 升级 / 回滚**：

```text
备份     PG 物理+逻辑备份（定时）、文件存储快照、定期恢复演练
迁移     Alembic 版本化迁移；向量列首次定维加索引、更换 Embedding 模型等向量维度/索引变更走停服窗口整体重建
          （一期不做零停机灰度；切换以 embedding_index 状态管理，见 §6.3.5）
升级     api 无状态滚动升级；worker 排空后替换；客户端版本 ↔ [后置]离线知识包版本兼容矩阵
回滚     迁移与发布均可回滚；高风险变更先 staging 验证
```

**模型降级矩阵**（V0.6 新增，§ #22）：

| 失效组件 | 降级行为 | 用户可见效果 |
|---|---|---|
| LLM 不可用 | 返回检索结果 + 引用列表，不做总结 | 标注"模型暂不可用"，仍展示 evidence/citations |
| Embedding 不可用 | 禁用向量检索路；只走结构化 + 全文 | 自然语言问题召回下降，明确提示走故障码速查 |
| Reranker 不可用 | 跳过精排，直接返回 RRF 融合 Top-K | 排序质量下降，仍可下发结果 |
| Safety check 不可用 | 不输出安全处置结论，提示人工复核 | card.safety.can_generate_final_advice=false |
| 设备数据不可用 | 返回知识库诊断，不引用实时数据 | 标注"未带设备数据"，给静态排查建议 |
| `embedding_index.status='failed'` | 自动 `keyword_search_only(query)`（§ #09） | 向量检索路关闭，全文检索兜底 |
| PGroonga 不可用 | SearchBackend 切回 tsvector+zhparser / pg_trgm（§7.4） | 中文检索召回下降，仍可用 |
| Redis 不可用 | 会话与缓存降级，限流降级 | 部分功能不可用，须报警 |

---

## 15. 质量保障与评测

> "可被测试"是本设计的基本要求。检索与答案质量需有量化口径与回归集，避免凭感觉调参。

### 15.1 检索评测

```text
评测集   构造 (问题 → 期望命中知识) 标注集，覆盖故障码/自然语言/关联排查/安全 四类
指标     Recall@k、MRR、nDCG@k；分路由类型分别统计
回归     每次改检索策略/权重/Reranker 跑评测集，对比基线，禁止无依据回退
```

冷启动计划：

```text
初始语料    现有故障码库 + 维修手册 + 历史工单 / 维修记录 / 典型案例
golden set  阶段二前由领域专家审核 100-200 条 (问题 → 期望命中知识 → 标准引用)
标注方式    可用 LLM 草拟候选问法和期望命中，再由专家审核
空库风险    系统在初始知识入库前不可验证真实效果；阶段零需明确首批资料来源、负责人和质量评估口径
```

### 15.2 答案评测

```text
忠实度(faithfulness)   结论是否均由 evidence 支撑（无证据不下结论）
引用准确率              结论标注的 citation 是否真实对应原文
蕴含校验                安全相关结论需校验证据是否真的支撑；不支撑则降级为人工核实（三步校验 §9.5）
有用率                 人工/抽检维度（解决问题/可执行/安全提示到位）
安全合规                安全相关结论是否强制附 safety_rule 出处 + 在线校验
```

### 15.3 幻觉控制清单

```text
- 无证据 → 明确告知"知识库未覆盖"，不臆造；
- 引用编号必须可校验，缺失则标"需人工核实"；
- 安全结论必须通过 evidence 支撑校验（三步校验第二步），不能只检查引用编号存在；
- 未发布/已弃用知识 0 进入上下文（自动化用例守护）；
- 关键数值/型号/参数优先来自结构化表而非自由生成。
```

### 15.4 测试策略

```text
单元    工具 I/O、角色动作权限校验、状态机流转（含 reactivate）、RRF 融合函数、三步校验各步
集成    入库流水线端到端（含 stage 跳步）、检索管线、Agent 只读链/写操作链执行（含 HITL 中断/恢复）
契约    API schema 校验（请求/响应/错误码/SSE DiagnoseEvent），前后端共用 OpenAPI 生成
评测    检索/答案评测集（CI 可跑，输出指标趋势）
安全    动作权限矩阵用例：每角色对每类动作（查/传/改/审核发布/管理/导出 restricted）的允许/拒绝断言
        sensitivity 边界用例（restricted 不进 export/离线包构造）
        required_online_check 语义用例（在线/离线 × true/false）
        JWT 黑名单用例（角色变更后旧 token 失效、refresh 拿新 claims）
```

### 15.5 各阶段完成定义（DoD，见 §17）

每阶段必须满足：功能用例通过 + 对应评测指标达标 + 权限用例全绿 + 关键路径有审计与可观测 + 文档/接口契约更新。

---

## 16. 非功能需求（NFR）

| 维度 | 目标（一期参考） |
|---|---|
| 性能 | 故障码精确查询 P95 < 300ms；自然语言诊断首 token < 2s、完整结果 P95 < 8s（需结合域内模型算力校准） |
| 并发 | 满足班组并发（如 50–100 在线），api 可水平扩展（≤5 人团队一期可单副本起跑，§0.8） |
| 可用性 | 在线服务可用性 ≥ 99%（内网），依赖故障可降级（partial / 缓存 / 模型降级矩阵 §14.4） |
| 可靠性 | 入库失败可重试（stage 级 attempts/attempts_max）、不丢源文件；高风险操作必有确认与留痕 |
| 安全 | 见 §13；未发布/已弃用知识进入上下文事件 = 0；restricted 资源不进 export/离线包 |
| 可维护 | 模块按目录解耦、接口契约稳定、迁移可回滚、SearchBackend/ModelGateway 可替换 |
| 可观测 | 关键路径 100% 带 trace_id；核心指标可看板化；embedding_index 状态可监控 |
| 国际化 | 以中文为主，文案外置，预留多语言 |
| 兼容 | 客户端版本 ↔ [后置]离线知识包版本兼容矩阵 |

---

## 17. 开发阶段规划与验收（V0.6 拆 M0/M1/M2）

> V0.6 把原阶段一 1a+1b 进一步拆成 M0/M1/M2 三个 2-3 周可闭环里程碑，避免"骨架被塞成 MVP"。阶段三后车载细节降为方向占位。

### 阶段零：风险 Spike 与冷启动准备

目标：在正式铺开功能前，回答会影响架构方向的关键问题。

内容：

- AI 对话组件 PoC：AI Elements 优先；同时评估 assistant-ui / prompt-kit；PoC 不通过则自建 shadcn 组件。
- [后置] 本地向量层 Windows + Tauri 打包 PoC（Zvec / sqlite-vec / LanceDB 至少验证两种）。
- Embedding / Reranker / LLM 模型选型初评；域内模型部署档位评估。
- 文档解析、表格抽取、OCR、图纸引用 PoC。
- 向量检索 benchmark（含 `WHERE status='active'` 发布状态过滤的 filtered-ANN）。
- 首批知识资料与 golden set 计划。
- 设备数据平台 / TDengine 接口事实确认。

DoD：关键 PoC 形成结论与回退方案；明确一期是否使用 AI Elements；明确 [后置] 本地向量层首选与兜底；明确测试期外部 API 仅用脱敏数据；形成 100-200 条 golden set 标注计划；设备数据联动标注为后续专项。

### M0 · 单机跑通（2-3 周）

目标：搭最小可运行骨架，验证端到端技术链路。

内容：

- FastAPI + PostgreSQL + 登录（自建账号 + JWT）；
- 故障码 CRUD（基础 `fault_code` 表）；
- React 页面（壳路由 + 故障码列表 + 故障码详情）；
- 结构化日志 + trace_id；
- 单进程运行（api；先不上 worker/scheduler/Redis）。

DoD：用户可登录并查询故障码（精确 + pg_trgm 模糊）；故障码 CRUD 全绿；日志含 trace_id；单机可演示。

### M1 · 可信故障码速查（2-3 周）

目标：在 M0 上建设可信、可审计、可角色控的故障码速查与现场入口。

内容：

- `fault_code` ↔ `fault_cause` / `maintenance_action` / `safety_rule` 关系表；
- 引用来源（citations）显示；
- RBAC 动作校验 + JWT 黑名单（§13.5）；
- `sensitivity` 字段生效（`restricted.read` / `export.write`）；
- `audit_log` 全量留痕；
- `/field` 只读入口（online-only，按能力面裁剪；不暴露写操作）；
- conversation / message / agent_checkpoint 表就位；
- **Schema 前向验证检查点**：把阶段三 Agent 的 Top-5 最复杂诊断查询在 M1 Schema 上模拟走通，确认不产生突破性 Schema 变更（§ #08）。若发现 schema 不够，在 M1 内补字段/索引，不留到阶段三。

DoD：

- 故障码速查返回可能原因、标准措施、安全提示、可追溯引用；
- 角色动作权限矩阵用例全绿；sensitivity 用例全绿；JWT 黑名单用例全绿；
- /field 入口须登录、按只读能力面工作、断网有明确提示；
- 关键操作均有 audit_log 和 trace_id；
- Schema 前向验证检查点通过。

### M2 · 最薄带引用问答（2-3 周）

目标：在 M1 上交付最薄端到端"问答 → 检索 → 带引用回答 → 日志"链路。

内容：

- 文档上传 + 文件存储（本地磁盘/NAS）；
- 文档解析 + chunk（按 mime_type 跳步 stage，§6.4）；
- 全文检索（SearchBackend 抽象，PGroonga/tsvector+zhparser/pg_trgm 可切换，§7.4）；
- ingestion_job 状态机；
- SSE `DiagnoseEvent`（§5.6.2 discriminated union）；
- Agent 只读链 intent → retrieve（无向量）→ safety_check → generate（LLM 域内）→ assemble → done；
- 证据约束生成 + 三步校验（§9.5）；
- 引用回填 document/section/version。

DoD（验收口径修正，§ #20）：

- 验收口径：输入自然语言故障描述，经意图识别 + 结构化 + 全文检索 + LLM 证据约束生成，返回带引用的诊断结果；
- **向量检索和文档级 RAG 不在 M2 范围**（向量索引归阶段三需要 embedding_index.ready；M2 先跑 keyword-only 模式，§ #09）；
- 安全提示在线校验生效，校验失败按强降级处理；
- SSE 流式可用、可中断；
- 未发布/已弃用进入上下文 = 0；
- 关键路径有日志与 trace_id。

### 阶段二：知识库与文档入库完善（M2 之后）

目标：让手册、案例、规程可结构化检索。

内容：服务器知识库；文件存储策略完善；OCR / 表格 / 图纸 / 步骤切片策略落地；中文全文检索调优；来源引用稳定性；知识审核发布（draft→pending→active 状态机 + reactivate 回滚）；golden set 初版。

依赖：M0/M1/M2 数据模型与权限。

DoD：入库流水线端到端可用（含幂等/失败重试，状态落 ingestion_job）；中文全文 + 模糊检索达基线 Recall@k；每条结果可回溯到 document/section/version；审核状态机生效（仅 active 可见）；reactivate 路径可用。

### 阶段三：RAG 检索与智能体应用

目标：形成智能诊断能力。

内容：`embedding_model_registry` / `embedding_index` 锚点表生效；pgvector HNSW 索引（停服窗口定维 + 建索引）；Embedding 服务；`embedding_model_version`；相似案例召回；Reranker；混合检索（RRF+Rerank）；LangGraph 写操作链（含 HITL）；正式 AI 对话 UI；结构化维修建议；faithfulness / entailment 校验；用户反馈。

依赖：阶段二可检索知识 + 模型网关 + 阶段零模型选型结论。

DoD：混合检索（RRF+Rerank）上线并跑通评测集；证据约束生成 + 引用准确率达标；安全结论强制带出处 + 在线校验；SSE 流式 + 可中断 + DiagnoseEvent 全事件类型；未发布/已弃用进入上下文 = 0；写操作链 HITL 工作正常。

### 阶段四 [后置]：本地知识库与 LocalVectorStore 本地向量层

目标：提升桌面端和离线/半离线体验。

内容：本地历史问答缓存；个人记忆；个人知识库；离线知识包（含 Manifest §11.4 + 加密方案）；相似问题推荐；本地文件目录设置；离线权限缺口明确（§11.2 / §13.6）；安全知识离线降级与强制刷新窗口；[后置] 小雪狐宠物系统 MVP 与状态联动。

依赖：阶段三在线检索/诊断；阶段零本地向量层 PoC；团队资源到位（§0.8）。

DoD：本地召回与离线包可用且带版本校验；普通离线结果明确标注时效；安全结论离线状态下不直接输出最终处置建议；个人记忆可清理、不污染官方结果；离线包加密方案落地；宠物状态机联动在线任务事件（error 状态补齐）。

### 阶段五 [后置]：知识图谱与设备数据联动

目标：提升技术亮点与运维价值。

内容：知识图谱前端可视化；`knowledge_node` / `knowledge_edge`；关系表→图谱主-从同步策略（§ #24 决策）；图谱路径查询；设备数据采集平台 / TDengine 查询接口；点位与部件关系映射；数据驱动的故障辅助判断。

依赖：阶段三诊断闭环；设备采集系统接口确认；团队资源到位。

DoD：图谱多跳查询支撑关联排查与安全风险；设备数据只读工具接入并审计（`device_data_query`，但 `risk_level=DEVICE_CONTROL` 在本期仍未启用控制）；点位↔部件映射可用；数据+知识+图谱联动给出风险提示。

```text
依赖关系：阶段零 Spike 并行前置；M0 → M1 → M2 → 阶段二 → 阶段三 → 阶段四[后置] → 阶段五[后置]
（阶段四/五需团队资源到位后启动；阶段零的 PoC 在 ≤5 人团队下也必须先跑完，否则阶段三/四无法定技术选型）
```

---

## 18. 推荐目录结构（V0.6 Monorepo）

> V0.6 引入 Monorepo 根目录治理（§ #30）；前后端共享 API 类型由 FastAPI OpenAPI spec 自动生成，不再手写两套。

```text
foxops/
├── apps/
│   ├── client/                          # 前端（foxops-client）
│   │   ├── src/
│   │   │   ├── app/                     # 应用壳、路由、全局 Provider
│   │   │   ├── pages/                   # 桌面工作台路由页面
│   │   │   ├── field-entry/             # 现场速查轻量入口（Web / 移动窄屏）
│   │   │   ├── components/              # 通用组件（卡片/引用/状态态）
│   │   │   ├── ai-ui/                   # 对话、工具调用、引用、流式状态组件接口
│   │   │   ├── workbench/               # 工作台（输入区/结果区/辅助区）
│   │   │   ├── [后置] pet/              # 小雪狐宠物系统
│   │   │   ├── admin/                   # 管理员功能
│   │   │   ├── api/                     # API Client（typed，由 openapi-typescript 生成）
│   │   │   ├── stores/                  # Zustand 状态
│   │   │   ├── [后置] local-vector/    # LocalVectorStore 封装
│   │   │   └── styles/
│   │   ├── src-tauri/                   # Tauri 原生侧
│   │   ├── public/
│   │   └── package.json
│   └── server/                          # 后端（foxops-server）
│       ├── app/
│       │   ├── api/                     # 路由与依赖（鉴权、限流、分页、错误处理）
│       │   ├── core/                    # 配置、日志、追踪、异常、安全
│       │   ├── auth/                    # 登录、RBAC、令牌 + JWT 黑名单
│       │   ├── conversation/            # 会话与消息（§6.3）
│       │   ├── knowledge/               # 知识录入/审核/发布/版本/状态机回滚
│       │   ├── search/                  # 混合检索 + SearchBackend 抽象（§7.4）
│       │   ├── graph/                   # 知识图谱
│       │   ├── agent/                   # LangGraph 只读链 / 写操作链 + 工具系统 + 风险分级
│       │   ├── models/                  # ORM 模型 + Pydantic schema（OpenAPI 来源）
│       │   ├── documents/               # 文档入库流水线（stage 跳步 §6.4）
│       │   ├── [后置] devices/         # 设备数据适配器
│       │   ├── feedback/                # 反馈与案例沉淀
│       │   ├── gateway/                 # 模型网关（LLM/Embedding/Reranker/Prompt + 降级矩阵）
│       │   ├── embedding/               # embedding_model_registry / embedding_index 管理
│       │   └── common/                  # 工具函数、通用类型
│       ├── migrations/                   # Alembic
│       ├── workers/                      # 任务队列 worker
│       ├── scripts/                      # 运维/数据脚本
│       ├── tests/                       # 单元/集成/契约/评测
│       ├── eval/                         # 检索与答案评测集 + 指标脚本
│       └── pyproject.toml
├── packages/
│   ├── api-contracts/                    # OpenAPI spec → openapi-typescript → 前后端共享 typed client
│   └── config/                           # 共享配置（TS config / Prettier / ESLint / Ruff）
├── infra/
│   └── docker/                           # Docker / Compose / 内网部署
├── docs/                                 # 设计文档 + 专项文档（附录 A）
└── Makefile                              # 常用任务入口
```

API 类型共享流程：`apps/server/app/*` 的 Pydantic schema → FastAPI `/openapi.json` → `openapi-typescript` 生成 `apps/client/src/api/types.ts` → 前端 typed client 直接复用。

---

## 19. 技术选型评审与风险

> 本章对关键选型做成熟度评审，列出需团队拍板的未决项。结论基于公开资料核查（§19.3）。

### 19.1 选型评审表

| 选型 | 成熟度 | 一期建议 | 风险与回退 |
|---|---|---|---|
| FastAPI | 成熟、生态完善 | 直接作主线 | 低 |
| TurboAPI + Python 3.14t | 新技术方向，性能预研 | 暂缓，不进一期主线 | 项目核心风险不在 API 极限性能；回退即留在 FastAPI |
| Tauri | 成熟桌面框架，承载 Web 前端 | 作 Windows 桌面主线，前端按 Web-first 组织 | 注意与共享 Web App 解耦 |
| React + TS + Tailwind + shadcn/ui + Zustand | 成熟 | 作主线 | 低 |
| **AI Elements** | 基于 shadcn 生态 | 优先候选，先 PoC | 需确认流式输出、工具调用展示、引用、停止/重新生成、长消息性能、移动/窄窗口；失败则自建 |
| **assistant-ui / prompt-kit** | React / shadcn 生态 AI 组件参考 | 备选参考 | 可减少自建成本，但需验证与现有 SSE、LangGraph、引用卡片适配 |
| **Bklit UI** | 视觉参考 | 暂缓引入，shadcn/ui 单一设计系统 | 双 UI 库易冲突 |
| **PGroonga** | 成熟但 Windows/Docker/国产化环境安装有坑 | **非硬依赖**，附回退（V0.6 §7.4） | 通过 `SearchBackend` Protocol 抽象，配置 `fulltext_backend: trgm/tsvector_zhparser/pgroonga` 切换；Level 0 pg_trgm 必装作保底 |
| **LocalVectorStore** [后置] | 本地向量层抽象接口 | 阶段四启动 | 避免 Zvec PoC 失败导致重做 |
| **Zvec / sqlite-vec / LanceDB / Chroma** [后置] | 候选实现 | 阶段四 PoC | Zvec 0.3+ 支持 Windows 但需验证打包；sqlite-vec 兜底评估 |
| **embedding_model_registry / embedding_index** | V0.6 新增 | 必做 | 检索期判断索引可用性，避免向量路失效导致整体故障 |
| PostgreSQL + PGroonga(可选) + pgvector + pg_trgm + tsvector+zhparser | 成熟 | 作官方知识底座主线 | 配置化切换；HNSW 在阶段三定维后建 |
| LangGraph | 成熟、生产可用 | 作编排主线（只读链/写操作链分离） | 关注版本演进与依赖 |
| LlamaIndex | 成熟 | 仅作 parser/chunk helper/retriever wrapper（不接管业务对象 §9.4） | 业务主模型自建 |
| DeepAgents | 待确认能力边界 | 暂作参考或局部实验 | 避免与 LangGraph 主线重叠 |
| Hermes Agent | 名称能力边界需澄清 | 暂不作为核心依赖 | 复用前再评估 |
| arq / Dramatiq / Celery / APScheduler | 均可用 | 收敛为一套任务队列 + 一套定时器 | arq 轻量且 asyncio 原生；Dramatiq 中等；Celery 生态成熟但运维重；不要多套混用 |
| Redis | 成熟 | 用于会话/队列/缓存/JWT 黑名单 | 单点风险，需备份/降级 |

### 19.2 未决项清单（需团队拍板）

```text
1. AI 对话组件 PoC：AI Elements 优先，参考 assistant-ui / prompt-kit；PoC 不通过则自建 shadcn 组件。
2. [后置] LocalVectorStore PoC：阶段四前完成，Zvec / sqlite-vec / LanceDB 至少验证两种，确认 Windows + Tauri 打包、运行时、包体积、索引构建、稳定性。
3. Bklit UI 去留：若 shadcn/ui + 图表库足够，则不引入第二套。
4. DeepAgents / Hermes Agent 能力边界：不作为核心依赖，仅在能明显减少工具/技能/智能体搭建成本时再评估。
5. Embedding / Reranker / LLM 具体模型、向量维度、域内部署档位、成本策略，单独做模型选型专项。
6. 后台任务框架收敛为一套：候选 arq / Dramatiq / Celery，定时器候选 APScheduler / beat。
7. [后置] 设备数据采集平台 / TDengine 接入专项：先确认你的平台统一输出方式，再比较 OPC UA / Modbus / REST / TDengine。
8. 数据不出域执行方案：正式 RAG/诊断/安全主链路必须域内模型；测试期外部 API 仅允许无关、脱敏、非涉密数据。
9. 评测集与冷启动：阶段二前完成 100-200 条 golden set 计划，明确首批手册、故障码库、历史工单来源。
10. [后置] 反馈→案例的"知识闭环"审核标准。
11. 现场断网/设备旁离线 [后置]：现场 Web/移动入口 online-only、无离线兜底；桌面端离线能力推到阶段四。一期默认"现场 wifi 可靠（online-only 即够用）"；若现场不可靠，则明确"设备旁离线"一期不覆盖并记为已知缺口。
12. [后置] 关系表→知识图谱同步策略：一次性快照迁移 vs 增量（CDC / trigger / 事件总线），阶段五前拍板；关系表为权威源，图谱为扩展视图（§8.5）。
13. [后置] 离线包加密密钥方案、用户离职后清包策略，阶段四前完成（§11.4 写入《FoxOps_安全权限与离线包.md》）。
14. 灰度策略与离线状态优先级（§ #29）：灰度为 online-only 管理设定，离线时灰度不生效，统一按离线兜底规则执行 [后置]。
```

### 19.3 参考资料（选型核查）

- Zvec（阿里开源，进程内向量库 / 内置 RRF reranker）：GitHub `alibaba/zvec`、PyPI `zvec`、官网 zvec.org
- TurboAPI（FastAPI 兼容、Zig 核心、Python 3.14t）：GitHub `justrach/turboAPI`、PyPI `turboapi`
- PGroonga（多语言全文，中文优于 tsvector/pg_trgm）：pgroonga.github.io、PostgreSQL News
- zhparser（PostgreSQL 中文分词 + tsvector 全文检索）
- pgvector（HNSW、混合检索 + RRF）：GitHub `pgvector/pgvector`
- LangGraph（TypedDict 状态、interrupt/checkpoint HITL、生产中间件）：GitHub `langchain-ai/langgraph`
- AI Elements：Vercel AI SDK Elements 文档
- assistant-ui：assistant-ui 官方文档
- prompt-kit：prompt-kit 官方文档
- openapi-typescript：由 OpenAPI spec 生成 TS typed client

---

## 20. 术语表与数据字典

### 20.1 术语表

```text
工作台        用户主界面，承载完整任务处理与结果展示
小雪狐 [后置]  桌面宠物系统，辅助入口与状态反馈
官方知识      经审核发布、存于 PostgreSQL 的可信知识（status=active）
本地向量层 [后置] LocalVectorStore，承载个人记忆/缓存/离线包，非官方源
混合检索      结构化 + 全文 + 向量 + 图谱多路召回 + RRF 融合 + Rerank
证据约束生成   仅依据检索证据下结论并附引用，无证据不臆造
HITL         Human-in-the-loop，高风险写操作的人工确认检查点（仅桌面工作台）
RRF          倒数排名融合，跨异构检索源的稳定融合方法
动作级RBAC    按角色控制能否执行某动作（查/传/改/管理），不做按部门/密级的数据级可见性过滤
sensitivity   V0.6 新增，public/internal/restricted，控制离线/导出能力
ToolRiskLevel V0.6 新增，工具风险分级：read_only/write_local/write_server/safety_critical/device_control
域内模型      模型推理在本组织物理或逻辑隔离的服务器/集群上完成，不存在推理数据离开组织信任边界的路径（§0.7 基线 #9）
```

### 20.2 状态字典

```text
知识状态   draft / pending / active / deprecated / archived（仅 active 可进入上下文；reactivate: deprecated→active 走 §6.5 B 策略）
ingestion_job.status   pending / running / succeeded / failed / skipped（V0.6 新增 skipped）
embedding_model_registry.status   candidate / active / deprecated
embedding_index.status           building / ready / failed / deprecated
反馈评级   useful / useless；resolved: true/false
```

### 20.3 图谱本体字典 [后置 / 阶段五]

```text
node_type   equipment, subsystem, component, fault_code, symptom, cause,
            action, tool, part, safety_risk, case, manual_section, data_point
relation    contains, corresponds, caused_by, check, treat, needs, involves,
            verified_by, supported_by, reflects
```

### 20.4 权限码字典（示例，V0.6 增 restricted.read / export.write / knowledge.reactivate）

```text
knowledge.read / case.read / case.write /
knowledge.review / knowledge.publish / knowledge.deprecate / knowledge.reactivate /
restricted.read / export.write /
device.read / device.read_sensitive /
admin.user_manage / model.config
```

### 20.5 工具风险分级字典（V0.6 新增）

```text
read_only         可自动执行（只读链）
write_local       需用户确认（导出 restricted / 写本地文件）
write_server      需权限 + HITL
safety_critical  需在线校验 + HITL（知识发布/弃用/reactivate、命令执行）
device_control    当前版本禁止注册/调用
```

---

## 21. 技术路线摘要

FoxOps 采用"Web-first 智能体工作台 + 服务端智能体编排 + PostgreSQL 官方知识底座 + 可替换本地向量层（后置）"的技术路线。

前端使用 React + Tauri 构建 Windows 优先的桌面客户端，按 Web-first 组织共享页面、组件、状态和 API Client，预留内网 Web / 移动窄屏现场速查 `/field` 入口（online-only）。后端采用 Python + FastAPI 构建 API 服务，使用 LangGraph 编排智能体诊断流程（**只读链无 HITL，写操作链带 HITL**），使用 LlamaIndex 仅作文档解析/切片/检索封装的工具箱（不接管业务对象模型）。数据层以 PostgreSQL 为唯一可信知识底座，结合 PGroonga（非硬依赖，附回退 pg_trgm/tsvector+zhparser）、pg_trgm、pgvector 与轻量知识图谱关系表，实现结构化查询、中文全文检索、语义召回、RRF 融合与重排、关系分析与版本管理。业务会话与 LangGraph 图执行状态分离（`conversation`/`message` vs `agent_checkpoint`），通过 `conversation_id`/`trace_id` 关联。Embedding 模型版本与索引可用性由 `embedding_model_registry`/`embedding_index` 承载，检索期判断降级。故障码与原因、措施、安全规则关系前置，支撑 M1 现场速查与旗舰演示。

权限为**动作级 RBAC + 最低 `sensitivity` 字段**（按角色控制查/传/改/管理 + restricted 控制离线/导出），检索只按发布状态过滤（仅 `status='active'`），答案以证据约束生成并附可追溯引用；**安全结论必须在线校验 safety_rule**，并做证据蕴含校验（三步校验：引用存在性 → 蕴含 → 降级）。**角色变更后 JWT 黑名单失效**。诊断/检索/问答只读链无 HITL；写操作链经 HITL 确认并写 `audit_log`，HITL 入口仅限桌面工作台。**离线权限缺口显式承认**：LocalVectorStore 为纯本地存储不嵌入 RBAC，离线场景假设桌面设备物理安全 + OS 级登录认证保护。**PGroonga 非硬依赖**，附 tsvector+zhparser/pg_trgm 回退；模型降级矩阵覆盖 LLM/Embedding/Reranker/safety/设备/embedding_index/PGroonga/Redis 失效场景。

阶段规划上 V0.6 把阶段一拆为 **M0（单机跑通）→ M1（可信故障码速查 + RBAC + /field）→ M2（最薄带引用问答）** 三个 2-3 周里程碑，阶段三以后车载细节降为方向占位。**一期 ≤5 人团队、有限 GPU**，据此砍掉一期 LocalVectorStore / 离线知识包 / 小雪狐宠物 / MinIO，跑纯在线模式；阶段四/五待团队资源到位后启动。前后端共享 API 类型由 FastAPI OpenAPI spec 经 `openapi-typescript` 自动生成，主文档保留方向与边界，细节下沉到附录 A 列出的 5 份专项文档。后期接入设备数据采集平台 / TDengine（只读优先、严禁控制），实现知识、图谱与现场数据联动，并以日志、审计、质量评测与分阶段 DoD 保障工程落地。

---

## 附录 A · 专项文档占位清单（V0.6 新增）

> V0.6 主文档保留方向与边界，可执行细节下沉到以下 5 份专项文档；本版只列出占位清单与"开工前提"，不生成内容。

| 专项文档 | 文件名（占位） | 内容范围 | 开工前提 |
|---|---|---|---|
| API 契约 | `FoxOps_API_契约.md` | 完整端点 + 请求/响应 schema + SSE `DiagnoseEvent` + 错误码 + OpenAPI spec 来源 | M0 启动前后端并行 |
| 数据库设计 | `FoxOps_数据库设计.md` | 完整 DDL + CHECK/UNIQUE + 索引参数 + 迁移策略 + embedding_index 切换流程 + SearchBackend 配置 | M0 启动后端开工 |
| 前端组件与状态 | `FoxOps_前端组件与状态.md` | 组件接口 + Zustand 状态 + FieldEntry 结构 + [后置] LocalVectorStore interface | M0 启动前端开工 |
| RAG 与评测方案 | `FoxOps_RAG与评测方案.md` | 检索管线 + 证据校验三步流程（§9.5）+ golden set 格式 + 评测脚本 + filtered-ANN benchmark | 阶段二/三 |
| 安全权限与离线包 | `FoxOps_安全权限与离线包.md` | RBAC 详细 + JWT 黑名单 + sensitivity 用法矩阵 + 离线包 Manifest + 加密方案 + 离线权限缺口 + Agent 状态机 | 全程参考；阶段四前完成加密方案 |

---

> 附：V0.6 在 V0.5 基础上完成压范围（M0/M1/M2 + 团队假设）、硬化契约（TS 类型 + conversation/message/agent_checkpoint + embedding 锚点 + Manifest + OpenAPI 自动生成）、修补逻辑冲突（sensitivity / required_online_check 语义 / Schema 前向验证 / PGroonga 与模型降级 / 离线权限缺口）、修正流程错误（拆只读链/写链 / 知识状态机 rollback / token 裁剪单点 / JWT 失效 / stage 跳步）四个动作。下一步：按附录 A 依次产出 5 份专项文档，M0 即可拆任务开工。


