# FoxOps 基于 Yuxi 开发的设计文档

> 版本：V0.2（纳入三方评审最终决策）
> 日期：2026-07-08
> 读者：FoxOps 研发工程师
> 定位：说明 FoxOps 从"自建全栈"改为"基于 Yuxi 扩展"的技术栈替换与扩展方案
> 评审：V0.1 后经 Claude Code × Codex × DeepSeek 三方联审，结论"可行，基于 Yuxi 是两个月内唯一现实路线"。本版纳入评审 3 条必须调整 + 关键风险 + 8 周排期 + 交付定义
> 关系：不替代《FoxOps_开发设计文档_V0.6.md》（领域逻辑仍以 V0.6 为准），只讲技术栈迁移与扩展落地

---

## 1. 背景与决策

FoxOps V0.6 原设计是"从零自建全栈"：FastAPI + LangGraph + LlamaIndex + PostgreSQL(pgvector/PGroonga) + 关系表图谱，前端 React + Tauri + shadcn。该设计因 ≤5 人团队资源约束，做了大量轻量/后置妥协（一期不用 Neo4j、用 pgvector 而非专业向量库、MinIO 后置、LocalVectorStore 后置）。

经评估 Yuxi（语析）项目后，发现 **Yuxi 已覆盖 FoxOps 设计中 70-80% 的通用能力（能力层面），且不少比 FoxOps 自建方案更成熟**。扣除配置/集成调试/融合改造成本后，**真实复用率约 50-60%**。

| FoxOps 自建妥协 | Yuxi 已有（成熟） |
|---|---|
| 一期不用 Neo4j，用关系表图谱 | Neo4j + MilvusGraphService + 实体关系抽取 + 图检索融合 |
| pgvector 向量 | Milvus 专业向量库 |
| MinIO 后置 | MinIO 已有 |
| 自建文档解析 | MinerU / PaddleX / RapidOCR / DeepSeek OCR |
| 自建 LangGraph 智能体 | BaseAgent + 中间件 + 工具 + Skills + MCP + 沙盒 + SubAgent |
| 自建 ARQ worker / SSE 流式 | ARQ worker + Redis 事件流 + SSE 已有 |

**决策**：FoxOps 改为**以 Yuxi 为底座增量扩展**——复用 Yuxi 全部通用能力，只新建"港口运维垂直业务"部分。

**两个月（8 周）交付范围**：港口故障码管理 + 知识库检索 + 带引用诊断问答 + 安全规则提示。
**后置（不在 MVP）**：桌面端、完整 HITL 写链、设备数据联动、复杂权限体系、蕴含校验、宠物系统。

**评审 3 条必须调整（本版已纳入）**：
1. **前端只做 Vue，砍掉 React 桌面端**——两套前端成本过高，Vue 里加 `/foxops/workbench` 页面承载工作台
2. **M0 不依赖图谱/Agent**——诊断先 service 编排不走 LangGraph，跑通后再 LangGraph 化；图谱作增强不作地基
3. **权限沿用 Yuxi 角色级，不做 permission 码**——sensitivity 加业务表控制导出，大后期再扩

---

## 2. Yuxi 简介（让团队快速认识 Yuxi）

Yuxi 是一个基于大模型的智能知识库与知识图谱智能体开发平台，融合 RAG 与知识图谱，架构为 **LangGraph v1 + Vue.js + FastAPI + LightRAG**，完全通过 Docker Compose 管理，支持热重载开发。

### 2.1 架构分层

后端分两个顶层边界：

```
backend/server    Web 应用入口与 HTTP 适配层（路由、认证、日志、生命周期）
backend/package/yuxi    可复用业务包（后端主体）
  ├─ agents/        LangGraph 智能体（BaseAgent/中间件/工具/Skills/MCP/沙盒/SubAgent）
  ├─ services/      用例层（业务流程编排，路由调它）
  ├─ repositories/  数据库访问边界（SQLAlchemy 查询）
  ├─ storage/       持久化基础设施（postgres 连接池 / minio 对象存储）
  ├─ knowledge/     知识库与图谱（KnowledgeBaseManager + Milvus + Neo4j + 分块 + 解析）
  ├─ models/        chat / embedding / rerank 模型适配
  ├─ config/        应用配置
  └─ utils/         业务通用工具
```

**依赖方向（架构不变量）**：`server(路由,薄) → services(用例) → repositories/agents/knowledge/models(领域) → storage(基础设施)`，单向向下。

### 2.2 核心能力地图

- **智能体**：LangGraph，BaseAgent + 中间件 + 工具 + Skills + MCP + 沙盒 + SubAgent + interrupt/approval（HITL 基础）
- **知识库**：KnowledgeBaseManager + Milvus + 文档解析（MinerU/PaddleX/RapidOCR/DeepSeek OCR）+ 分块（ragflow_like）+ LightRAG
- **知识图谱**：Neo4j + MilvusGraphService + 实体关系抽取 + 图检索融合
- **检索**：Milvus 向量 + RRF 融合（`_fuse_chunk_rankings`，融合 chunk 与 graph 两路）+ Rerank
- **运行链路**：API → agent_run → ARQ 队列 → worker 执行 LangGraph → 进度写 Redis → 前端 SSE（支持 interrupt/resume）
- **认证**：JWT + API Key（`yxkey_`）+ 登录锁定 + 操作日志
- **权限**：角色级（user / admin / superadmin）+ 部门隔离
- **模型管理**：多 provider，支持 Ollama / Xinference 本地模型（满足"域内模型"）
- **多租户**：企业级
- **可观测**：Langfuse + 操作日志
- **评测**：`knowledge/eval`

### 2.3 运行环境

Docker Compose：`web-dev`/`api-dev`/`worker-dev`/`sandbox-provisioner`/`postgres`/`redis`/`minio`/`milvus`/`graph`(Neo4j) + `mineru-*`/`paddlex`（按 profile）。支持 `LITE_MODE`（跳过知识库/图谱/评估等重依赖）。

---

## 3. 技术栈对比（FoxOps V0.6 原设计 → Yuxi 现有）

| 维度 | FoxOps V0.6 原设计 | Yuxi 现有 | 迁移说明 |
|---|---|---|---|
| Web 前端 | React + Tauri + shadcn | Vue 3 + Vite + Pinia + Ant Design Vue | 改用 **Vue + Art Design Pro**，承载管理 + 工作台 |
| 桌面端 | React + Tauri + shadcn | — | **MVP 砍掉**，后置（评审调整 1） |
| 后端框架 | FastAPI（自建） | FastAPI（已有） | 一致，复用 |
| 智能体编排 | LangGraph（自建只读链/写链） | LangGraph（BaseAgent+中间件+工具+Skills+MCP+沙盒） | M0 先 service 编排，跑通后 LangGraph 化（评审调整 2） |
| 文档处理 | LlamaIndex | MinerU+PaddleX+RapidOCR+DeepSeek OCR + ragflow 分块 | 复用，**不引入 LlamaIndex** |
| 向量库 | pgvector + HNSW | Milvus | 用 Milvus，**不用 pgvector** |
| 全文检索 | PGroonga+zhparser+pg_trgm | Yuxi 检索走 Milvus+LightRAG | 用 Yuxi 现有；中文全文 W0 验证 |
| 知识图谱 | 关系表 + 递归 CTE | Neo4j + MilvusGraphService | **M0 不依赖图谱**，后置增强（评审调整 2） |
| 混合检索融合 | RRF + Rerank | RRF（`_fuse_chunk_rankings`，仅 chunk+graph 两路）+ Rerank | 复用；FoxOps 业务表第三路自写融合器（不改 Yuxi） |
| 后台任务 | arq worker | ARQ worker + tasker + Redis 事件流 | 复用 |
| 会话/消息/checkpoint | conversation/message/agent_checkpoint | conversation+message+agent_run+AsyncPostgresSaver | 复用 |
| SSE 流式 | DiagnoseEvent | SSE 事件流 + Redis | 复用（见第 10 节验证） |
| HITL | LangGraph interrupt（写链） | interrupt + approval 已有基础 | **M0 不做完整写链**，后置 |
| 认证 | JWT + refresh + 黑名单 | JWT + APIKey + 登录锁定 + log_operation | 复用；JWT 黑名单后置 |
| 权限 | 动作级 RBAC + sensitivity | 角色级 + 部门 | **沿用角色级**，permission 码后置（评审调整 3） |
| 模型管理 | 模型网关 | 多 provider + Ollama/Xinference 本地 + rerank | 复用；域内模型 = 本地模型 |
| 对象存储 | MinIO（后置） | MinIO（已有） | 复用 |
| 评测 | golden set + Recall/MRR/nDCG | eval(benchmark/evaluator/metrics) | 复用，加领域评测集 |
| 可观测(LLM 追踪) | Langfuse | Langfuse(`langfuse_service.py`，独立服务+完整测试+多处集成) | ✅ 直接复用 |
| 操作日志/审计 | audit_log(结构化) | log_operation(操作流水，4 字段) | 🟡 MVP 先用 log_operation，按需扩展 target/result |
| 请求级 trace_id | X-Trace-Id 贯穿日志 | ❌ 无（Yuxi trace_id 是 Langfuse 的，非 HTTP 请求级） | 后置：MVP 用 Langfuse trace 追踪诊断链路 |
| 数据库 / 缓存 | PostgreSQL / Redis | PostgreSQL / Redis | 一致 |

---

## 4. 能力映射（FoxOps 能力 → Yuxi 现状）

> 已通过 codebase-memory 核实关键点（RRF / rerank / 权限粒度 / HITL / sensitivity）。已纳入评审调整。

| FoxOps 能力 | Yuxi 现状 | 处理 |
|---|---|---|
| 认证/用户/角色 | ✅ 角色级 RBAC + 部门 + JWT + APIKey + 登录锁定 + log_operation | ✅ 沿用角色级（评审调整 3）；permission 码/JWT 黑名单后置 |
| 知识库(上传/解析/分块/向量/检索) | ✅ KnowledgeBaseManager + Milvus + 多解析器 + ragflow 分块 + LightRAG | ✅ 复用 |
| 混合检索 RRF+Rerank | ✅ `_fuse_chunk_rankings`(RRF，仅 chunk+graph 两路) + `models/rerank.py` | 🟡 复用 + FoxOps 业务表第三路自写融合器 |
| 知识图谱 | ✅ Neo4j + MilvusGraphService | 🟡 M0 不依赖，后置增强；关系表直查为主 |
| 智能体(LangGraph) | ✅ BaseAgent + 中间件 + 工具 + Skills + MCP + 沙盒 | 🟡 M0 先 service 编排，跑通后 LangGraph 化 |
| 会话/消息/checkpoint | ✅ conversation + message + agent_run + AsyncPostgresSaver | ✅ 复用 |
| SSE 流式 | ✅ stream_agent_run_events + Redis 事件流 | ✅ 复用（见第 10 节） |
| HITL(人工确认) | ✅ interrupt + approval 已有基础 | 🟡 完整写链后置；MVP 只做引用存在性 + safety 降级 |
| 后台任务 | ✅ ARQ worker + tasker | ✅ 复用 |
| 模型管理/域内模型 | ✅ 多 provider + Ollama/Xinference 本地 | ✅ 复用 |
| 可观测(LLM 追踪) | ✅ Langfuse(`langfuse_service.py`，成熟，chat/feedback/agent_run/cli 都用) | ✅ 直接复用 |
| 操作日志/审计 | ✅ log_operation(操作流水) | 🟡 MVP 先用 log_operation；audit_log 结构化字段按需扩展 |
| 请求级 trace_id | ❌ 无（仅有 Langfuse trace） | 后置：MVP 用 Langfuse trace 追踪诊断 |
| 评测 | ✅ eval(benchmark/evaluator/metrics) | 🟡 加领域评测集 |
| **故障码/设备/部件/安全规程** | ❌ 无 | ❌ 新建：表 + repository + service + router |
| **证据约束生成** | ❌ 无 | ❌ MVP 只做引用存在性 + 无 evidence 不下结论；蕴含校验后置 |
| **sensitivity 数据分级** | ❌ 无 | ❌ 新建（加业务表，控制导出，不改鉴权链路） |
| 设备数据联动(TDengine) | ❌ 无 | ❌ 后置 |

**汇总**：✅ 直接复用约 50-60%，🟡 复用+扩展/后置约 25%，❌ 新建约 15-20%。

---

## 5. 扩展方案（工程隔离 + 数据流向）

### 5.1 工程隔离原则（评审建议，比散放更好）

FoxOps 代码**独立成包**，不散落进 Yuxi 原有模块：

```
backend/package/yuxi/foxops/
├── models.py              # fault_code/equipment/component/safety_rule/fault_case 等业务表
├── repositories.py        # 业务表查询
├── services/
│   ├── fault_code_service.py
│   ├── diagnosis_service.py    # 诊断编排（M0 普通函数，yield chunk）
│   └── safety_service.py
├── worker.py              # process_foxops_diagnosis（复用 Yuxi SSE 管道，见第 10 节）
├── fusion.py              # FoxOps 专用 RRF 融合器（业务表第三路，不改 Yuxi）
├── schemas.py             # Evidence / Citation / DiagnosisResult
└── eval/                  # 领域评测集

backend/server/routers/foxops.py   # 薄路由，注册到 routers/__init__.py
```

### 5.2 数据流向单向（不可破坏）

```
PostgreSQL（业务主库，权威源）→ 派生到 Neo4j/Milvus（索引层）
```

**不允许双边可编辑**：图谱/向量是 PG 的派生索引，只能从 PG 单向生成，不能反向写回。

### 5.3 新建 vs 扩展

**新建**（落 `yuxi/foxops/` 包内）：
- 垂直业务表（fault_code/equipment/component/safety_rule/fault_case）→ `foxops/models.py`
- 业务 repository / service / router
- 诊断 service（M0 普通函数编排，yield chunk）
- FoxOps 专用 RRF 融合器（业务表第三路）
- sensitivity 字段（业务表上，控制导出）
- 证据约束（MVP：引用存在性 + 无证据不下结论 + safety 降级）

**复用**（Yuxi 已有，直接用）：
- Langfuse（LLM 可观测，`langfuse_service.py`，诊断链路追踪直接用）
- log_operation（操作流水，MVP 直接用）

**扩展**（Yuxi 已有，小幅增强）：
- audit_log 结构化（log_operation 加 target/result 字段，或塞 details）
- 领域评测集（复用 `knowledge/eval`）

**后置**（MVP 不做）：permission 码 / JWT 黑名单 / 完整 HITL 写链 / 工具风险分级 / 蕴含校验 / 设备数据联动 / 桌面端 / HTTP 请求级 trace_id

---

## 6. 前端策略（评审调整 1：只做 Vue）

**整个 Web 前端用 Art Design Pro 重建**（新起独立项目，不复用 Yuxi 现有 Ant Design Vue 前端），砍掉 React 桌面端：

- **技术栈**：Vue 3 + Art Design Pro（Element Plus）+ Tailwind，新起独立前端项目
- **范围**：所有页面在新前端里重建——
  - 工作台：`/foxops/workbench`（搜索 + 对话 + 引用卡片 + 安全提示）
  - 故障码管理页（CRUD）
  - 管理后台：复刻 Yuxi 管理（用户/知识库/图谱/模型/系统）
- **后端**：全部调 Yuxi `/api`（含 foxops router）
- **认证**：JWT（浏览器登录）
- **Yuxi 现有 `web/`**：不再使用，仅作参考（照看它调哪些 API、有哪些功能）

**理由**：两套前端（Vue + React）成本过高，≤5 人团队扛不住，故砍 React 桌面端；Web 端统一用 Art Design Pro 重建，UI 风格统一、只维护一套。

**桌面端（React+Tauri）后置**：等 MVP 验证业务后再考虑。

---

## 7. 开发顺序与排期

### 7.1 端到端开发顺序（每阶段含验证标准）

按"先地基后上层、每层可验证"推进，每阶段过了验证才进下一阶段。

**阶段 0 · 环境与扩展验证** ✅ 已完成
- 任务：Docker 跑通、中文检索验证、ingestion 重跑验证、SSE 管道可行性、建表机制可行性
- 验证：Yuxi 能跑 + FoxOps 扩展模式（建表/SSE/检索/重跑）全部可行
- 产出：架构风险清零，可进入开发

**阶段 1 · 垂直数据层（后端地基）**
- 任务：建表（fault_code/fault_cause/maintenance_action/safety_rule，继承 `BusinessBase`，`foxops/__init__.py` 显式 import）+ repository/service/router + 50 条测试数据
- 验证：表建出来 + CRUD 接口通 + 数据能查
- 产出：故障码数据层可用

**阶段 2 · 知识库引用**
- 任务：上传维修手册（走 Yuxi 知识库）+ citation 关联 + 引用展示
- 验证：故障码速查能带出手册引用
- 产出：知识库与故障码关联

**阶段 3 · 诊断 service + SSE**
- 任务：`diagnosis_service`（普通函数编排，yield chunk）+ `process_foxops_diagnosis` worker（复用 Yuxi SSE 管道，路 A，第 10 节）+ 证据约束生成（引用存在性 + 无证据不下结论）
- 验证：SSE 能收到诊断流式输出 + 带引用
- 产出：诊断主链路通

**阶段 4 · 可信化**
- 任务：引用存在性校验 + safety_rule active 校验 + audit_log（log_operation 扩展）
- 验证：错误引用被拦 + 安全规则校验生效 + 操作留痕
- 产出：诊断可信

**阶段 5 · UI（Art Design Pro 重建整个前端）**
- 任务：新起 Art Design Pro 项目，重建所有页面——工作台（/foxops/workbench：搜索+对话+引用卡片+安全提示）+ 故障码管理页（CRUD）+ 管理后台（用户/知识库/图谱/模型/系统，复刻 Yuxi）
- 验证：端到端能用——用户能搜故障码、看诊断、看引用、做管理
- 产出：完整可用的 Web 界面
- 注：重建整个前端工作量大于"加页面"，建议按优先级分批（FoxOps 核心页优先，管理后台渐进复刻），W7 一周可能不够，排期按需调整

**阶段 6 · 测试与打磨**
- 任务：golden set 20-50 条（故障码/自然语言/安全）+ 检索/答案评测（复用 `knowledge/eval`）+ 修 bug
- 验证：评测指标达标 + 关键路径用例全绿
- 产出：质量达标

**阶段 7 · 部署**
- 任务：生产 Docker Compose（LITE_MODE 按需）+ 数据备份（PG + 文件存储）+ 监控告警（Langfuse + 日志）+ 部署文档 + 演示脚本
- 验证：生产环境跑通 + 备份恢复演练 + 演示通过
- 产出：可交付的生产系统

### 7.2 8 周排期（评审）

| 周 | 目标 | 交付物 |
|---|---|---|
| W0 ✅ | Spike | Docker 跑通 + 中文检索验证 + ingestion 重跑验证 + SSE 管道 PoC + 建表机制验证（全过） |
| W1-2 | 垂直数据层 | fault_code/cause/action/safety_rule 表 + CRUD API + Vue 管理页 + 50 条数据 |
| W3 | 知识库引用 | 上传手册 + citation 关联 + 引用展示 |
| W4-5 | 诊断 service + SSE | 故障码/自然语言→检索→证据约束生成→流式输出 |
| W6 | 可信化 | 引用存在性校验 + safety_rule active 校验 + audit_log |
| W7 | 工作台页面 | Vue `/foxops/workbench`：搜索 + 对话 + 引用卡片 + 安全提示 |
| W8 | 打磨 + 部署 | golden set 20-50 条 + 修 bug + 部署文档 + 演示脚本 + 生产部署 |

---

## 8. 关键技术风险（评审 4 条）

| 风险 | 说明 | 应对 |
|------|------|------|
| Yuxi RRF 不支持第三路 | `_fuse_chunk_rankings` 只融合 chunk+graph 两路，FoxOps 多"业务表精确匹配"第三路 | 写 FoxOps 专用融合器（`foxops/fusion.py`），不改 Yuxi 原融合逻辑 |
| 蕴含校验复杂度高 | NLI 模型判断"evidence 是否支撑结论"，延迟可靠性难保证 | MVP 只做引用编号存在性 + 无 evidence 不下结论 + safety 降级；蕴含校验后置 |
| Milvus 中文检索覆盖度未知 | Yuxi Milvus+LightRAG 对中文维修手册召回率未验证 | W0 Spike 用真实手册数据验证；不达标补 pg_trgm 全文路 |
| Docker 资源门槛 | PG+Redis+MinIO+Milvus+Neo4j+worker 全起 ≥16GB RAM | 定义 LITE_MODE：Neo4j 可选、OCR 容器关闭、用可解析 PDF/Markdown |

---

## 9. 最终交付定义（MVP）

```
FoxOps MVP = 基于 Yuxi 的港口设备运维知识助手（Web）

包含：
1. 故障码结构化管理（CRUD + 原因/措施/安全规则关系）
2. 故障码速查（精确 + 模糊 + 引用）
3. 维修手册知识库检索（Yuxi Milvus）
4. 带引用的诊断问答（证据约束生成 + SSE 流式）
5. 安全规则提示（active 校验 + 离线/失败降级）
6. 操作日志

不含（后置）：
- React 桌面端 / Tauri
- 完整 HITL 写操作链
- permission 码 / JWT 黑名单
- 设备数据联动
- 蕴含校验（entailment）
- 宠物系统
```

---

## 10. Yuxi SSE 管道复用可行性验证（已验证可行）

针对评审 W0 问题 4「Yuxi SSE 事件格式能否承载诊断专用事件」，用 codebase-memory 查证 Yuxi 流式管道与 LangGraph 的耦合度。结论：**路 A 可行——复用 Yuxi SSE 管道 + 自写 worker 任务，不改 Yuxi 核心。**

**查证依据**：

| 函数 | 职责 | 与 LangGraph 耦合 |
|------|------|-------------------|
| `stream_agent_chat` | worker 产生 stream 的源头 | ❌ 深度绑死（`_stream_agent_events`→`BaseAgent.stream_messages`、`agent.get_graph()`、`save_messages_from_langgraph_state`、`check_and_handle_interrupts`） |
| `append_run_stream_event` | worker 写事件到 Redis | ✅ 纯 Redis（`xadd`+`expire`，零耦合） |
| `list_run_stream_events` | API SSE 从 Redis 读事件 | ✅ 纯 Redis（`xrange`+解析，零耦合） |
| `process_agent_run` | worker 任务主函数 | ⚠️ 仅在"调 `stream_agent_chat` 拿 stream"一步耦合；其余（取消/缓冲/写 Redis/更新 DB）通用 |

**核心结论**：Yuxi SSE 管道两端都与 LangGraph 零耦合，是纯 Redis 事件流。唯一绑死 LangGraph 的是 `stream_agent_chat`（产生内容那环），FoxOps 诊断走 service 编排不用它。**LangGraph 只在"产生内容"那一环，"传输内容"的整条管道可复用。**

**路 A 的正确做法**（复用管道 + 自写源头）：
- FoxOps 自写 ARQ worker 任务 `process_foxops_diagnosis`
- 复用 Yuxi 基础设施（不改）：`RunContext`（取消）、`ChunkedEventWriter`（缓冲）、`append_run_event`/`append_run_stream_event`（写 Redis）、`mark_run_running`/`mark_run_terminal`（DB 状态）、`_consume_stream_with_cancel`（消费 async generator）
- 换掉源头：stream 来自 `foxops_diagnosis_service.diagnose(...)`，不调 `stream_agent_chat`
- 注册到 `WorkerSettings.functions`（与 `process_agent_run` 并列，ARQ 原生支持多任务）
- API 端 `stream_agent_run_events` 直接复用（只读 Redis）
- `agent_run.run_type` 加 `foxops_diagnosis`（业务字段扩展，非改鉴权/中间件）

**效果**：不改 Yuxi 核心 + 复用整条 SSE 管道 + 前端同一套 SSE 接收。W0 剩余仅"chunk 格式套进管道后前端能渲染"（实现期 W4-5），不再是架构风险。

---

## 11. W0 Spike 必答问题

1. ✅ Docker Compose 一键跑通——已验证（团队环境启动成功，发消息测试通过）
2. ✅ Yuxi 检索对中文维修手册的召回率——已实操验证可接受（栈：Milvus 向量 + rerank，LightRAG 知识库类型不支持新建）
3. ✅ FoxOps 独立成包能否被 pg_manager 建表——已验证可行（见第 12 节第 4 条）
4. ~~Yuxi SSE 事件格式能否承载诊断专用事件~~ → 已验证可行（第 10 节）
5. ✅ ingestion 失败后能否重跑——已实操验证（删除文档 + 重新上传可重新入库，满足"能重跑即可"）

> W0 如有 2 个以上卡住，立即调整方案。

---

## 12. 待确认项

1. ✅ 中文全文检索：已实操验证 Yuxi 检索（Milvus 向量 + rerank）对中文维修手册召回可接受；LightRAG 知识库类型不支持新建。无需补 pg_trgm。
2. ✅ ingestion 重跑：已实操验证（删除文档 + 重新上传可重新入库），满足"能重跑即可"。
3. Yuxi 智能体的 interrupt/approval 与 FoxOps HITL 写操作链契合度——MVP 不做完整写链，推到后期 HITL 启动时再验。
4. ✅ FoxOps 独立成包建表——已验证可行：`pg_manager.create_tables()` 用 `BusinessBase.metadata.create_all`（SQLAlchemy 自动注册机制）；FoxOps 模型继承 `BusinessBase` + 在 `foxops/__init__.py` 显式 import 即被自动建表，**不改 pg_manager**。后续加字段仿 `ensure_business_schema` 加 `ALTER TABLE ADD COLUMN IF NOT EXISTS`，或用 Alembic。

---

> 附：本文档基于 Yuxi 实际代码 + FoxOps V0.6 设计 + 三方评审意见编写，关键能力映射与 SSE 耦合度已用 codebase-memory 核实。领域逻辑（故障码建模/安全规程/证据约束/HITL 流程等）详见《FoxOps_开发设计文档_V0.6.md》，本文档只负责"在 Yuxi 上怎么落地"。
