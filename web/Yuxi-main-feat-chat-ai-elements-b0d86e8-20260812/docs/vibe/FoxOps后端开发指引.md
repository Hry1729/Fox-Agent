# FoxOps 后端开发指引

> 读者：FoxOps 后端工程师
> 工作目录：`D:\python\projects\Yuxi-main\backend`（Yuxi 后端，复用 + 扩展）
> 目标：在 Yuxi 后端新增 `yuxi/foxops/` 包，实现港口运维垂直业务，不改 Yuxi 核心

---

## 1. 你的任务

FoxOps 基于 Yuxi 二次开发。**后端复用 Yuxi**（FastAPI + LangGraph + ARQ + Milvus + Neo4j + Postgres + Redis），只新增"港口运维垂直业务"部分（故障码/安全规程/诊断/证据约束等）。

你的工作：
1. 在 `backend/package/yuxi/foxops/` 下新增业务包（独立成包，不散进 Yuxi 原模块）
2. 复用 Yuxi 的通用能力（知识库/图谱/智能体/SSE/worker/认证/模型管理），**不改 Yuxi 核心**
3. 新增 FoxOps 垂直业务表 + repository/service/router + 诊断 service + SSE worker

**核心红线（评审定的）**：不改 Yuxi 核心模块（`agents/`/`services/chat_service.py`/`storage/postgres/manager.py` 等）。FoxOps 代码独立成包，复用 Yuxi 暴露的接口。

---

## 2. 先读这些文档（理解项目）

必读（按顺序，都在 `D:\python\projects\Yuxi-main\` 下）：
1. `docs/vibe/foxops基于yuxi开发的设计文档.md` - **总设计**：技术栈/能力映射/扩展方案/SSE 验证/开发顺序
2. `docs/vibe/foxops-yuxi-review-final.md` - 评审决策（3 条调整 + 风险 + 排期）
3. `docs/FoxOps_开发设计文档_V0.6.md` - 领域逻辑（故障码建模/安全规程/证据约束/HITL/状态机）
4. `ARCHITECTURE.md` - Yuxi 架构（后端代码地图 + 不变量）
5. `CLAUDE.md` - 开发规范（pythonic/不过度防御/测试规范/changelog）

重点理解：
- 评审 3 条调整：M0 诊断走 service 不走 LangGraph；权限沿用角色级不做 permission 码；图谱后置
- SSE 管道复用（路 A，已验证可行，设计文档第 10 节）
- 建表机制（设计文档第 12 节第 4 条，已验证）
- 数据流单向：PG（主库）-> Neo4j/Milvus（索引层），不可双边编辑

---

## 3. 工作目录与技术栈

**工作目录**：`backend/`（Yuxi 后端）

**技术栈**（Yuxi 已有，复用）：
- API：FastAPI（`server/main.py` 启动，端口 5050）
- 智能体：LangGraph（M0 诊断先不走，后期再 LangGraph 化）
- 后台任务：ARQ worker（`server/worker_main.py` -> `yuxi.services.run_worker.WorkerSettings`）
- 数据库：PostgreSQL 16（SQLAlchemy + asyncpg）
- 向量库：Milvus（知识库检索）
- 图谱：Neo4j（M0 后置）
- 缓存/队列：Redis（SSE 事件流 + ARQ + 取消信号）
- 对象存储：MinIO
- 模型：多 provider + Ollama/Xinference 本地（域内模型）
- 依赖管理：uv（`backend/pyproject.toml` + `uv.lock`）

**FoxOps 扩展不需要单独装依赖**，用 Yuxi 的即可。

---

## 4. 开发环境

### 推荐：docker 热重载（已配好）
在 `D:\python\projects\Yuxi-main` 目录：
```bash
docker compose up -d
```
- `api-dev`：`uvicorn --reload --reload-dir server --reload-dir package`（改 `yuxi/foxops/` 自动重载）
- `worker-dev`：`watchfiles`（改 .py 自动重启）
- 看日志：`docker logs api-dev --tail 100 -f` / `docker logs worker-dev --tail 100 -f`
- 改 FoxOps 代码立即生效，不用重建容器（只有改 `pyproject.toml` 依赖才重建）

### 本地跑（IDE 断点调试，可选）
```bash
docker compose up -d postgres redis minio etcd milvus graph sandbox-provisioner   # 只起基础设施
cd backend
uv sync
# 环境变量 override（容器名改 localhost）：
set POSTGRES_URL=postgresql+asyncpg://postgres:postgres@localhost:5432/yuxi
set REDIS_URL=redis://localhost:6379/0
set NEO4J_URI=bolt://localhost:7687
set MILVUS_URI=http://localhost:19530
set MINIO_URI=http://localhost:9000
set SANDBOX_PROVISIONER_URL=http://localhost:8002
set RUNNING_IN_DOCKER=false
uv run uvicorn server.main:app --reload --port 5050
# 另开终端跑 worker：
uv run arq server.worker_main.WorkerSettings
```

### 测试
- 测试目录：`backend/test/`，按 `unit`/`integration`/`e2e` 分层
- 遵循 `docs/develop-guides/testing-guidelines.md`
- FoxOps 测试放 `backend/test/unit/foxops/`、`backend/test/integration/foxops/`
- 格式化：`make format`
- 提交前确保测试通过

---

## 5. FoxOps 扩展架构（独立成包）

**工程隔离原则**：FoxOps 代码独立成包，不散进 Yuxi 原有模块。这样内聚、不污染 Yuxi、升级 Yuxi 方便。

```
backend/package/yuxi/foxops/
├── __init__.py          # ★ 显式 import models（确保建表注册，见第 6 节）
├── models.py            # fault_code/fault_cause/maintenance_action/safety_rule/fault_case 等业务表
├── repositories.py      # 业务表查询（SQLAlchemy）
├── schemas.py           # Pydantic：Evidence / Citation / DiagnosisResult / FaultCode 等
├── services/
│   ├── fault_code_service.py    # 故障码 CRUD + 速查
│   ├── diagnosis_service.py     # 诊断编排（M0 普通函数，yield chunk）
│   └── safety_service.py        # 安全规程校验
├── worker.py            # process_foxops_diagnosis（复用 Yuxi SSE 管道，见第 8 节）
├── fusion.py            # FoxOps 专用 RRF 融合器（业务表第三路，不改 Yuxi）
└── eval/                # 领域评测集

backend/server/routers/foxops_router.py   # 薄路由，注册到 server/routers/__init__.py
```

**分层规范（Yuxi 不变量，必须遵守）**：
- `server/routers/` 薄：只接请求、调 service、返响应，不写业务逻辑
- `yuxi/foxops/services/` 用例编排：业务流程在这
- `yuxi/foxops/repositories.py` DB 查询：封装 SQLAlchemy，路由不绕过
- 依赖方向单向向下：路由 -> service -> repository

---

## 6. 建表机制（已验证可行，关键）

Yuxi 的 `pg_manager.create_tables()` 用 SQLAlchemy metadata 自动注册：
```python
# yuxi/storage/postgres/manager.py
async def create_tables(self):
    async with self.async_engine.begin() as conn:
        await conn.run_sync(KnowledgeBase.metadata.create_all)
        await conn.run_sync(BusinessBase.metadata.create_all)   # 业务表
```

**FoxOps 表要被建出来，只需两步**：
1. 模型继承 `BusinessBase`（从 `yuxi.storage.postgres` 导入）
2. 在 `foxops/__init__.py` 显式 `import` models（确保类定义执行、注册到 metadata）

**★ 最容易踩的坑**：忘记 import models，类没注册，表建不出来。

**骨架示例**（`foxops/models.py`）：
```python
from sqlalchemy import String, Text, Integer, TIMESTAMP, func
from yuxi.storage.postgres import BusinessBase  # 继承这个

class FaultCode(BusinessBase):
    __tablename__ = "foxops_fault_code"
    id = Column(Integer, primary_key=True)
    code = Column(String(80), nullable=False, unique=True)
    title = Column(String(200), nullable=False)
    severity = Column(String(20))  # low/medium/high/critical
    equipment_type = Column(String(50))
    description = Column(Text)
    status = Column(String(20), default="active")  # active/deprecated
    sensitivity = Column(String(20), default="internal")  # public/internal/restricted
    created_at = Column(TIMESTAMP, server_default=func.now())
```

**`foxops/__init__.py`**：
```python
from . import models  # ★ 必须，否则表不注册
```

**不改 `pg_manager`**。后续加字段仿 `ensure_business_schema` 加 `ALTER TABLE ADD COLUMN IF NOT EXISTS`，或用 Alembic。

---

## 7. 扩展任务清单（按优先级）

### P0 - W1-2：垂直数据层
- [ ] 建表：`fault_code`/`fault_cause`/`maintenance_action`/`safety_rule`/`fault_case` + 关系表（设计见 FoxOps V0.6 §6.3）
- [ ] `repositories.py`：CRUD 查询
- [ ] `services/fault_code_service.py`：故障码速查（精确 + pg_trgm 模糊）
- [ ] `schemas.py`：请求/响应模型
- [ ] `server/routers/foxops_router.py`：CRUD 接口，注册到 `routers/__init__.py`
- [ ] 灌 50 条测试数据
- [ ] 验证：表建出来 + CRUD 接口通 + 数据能查

### P1 - W3：知识库引用
- [ ] 上传维修手册（走 Yuxi 知识库 `KnowledgeBaseManager`）
- [ ] citation 关联故障码/手册章节
- [ ] 引用回填 document/section/version

### P2 - W4-5：诊断 service + SSE
- [ ] `services/diagnosis_service.py`：普通函数编排，`yield` chunk（不走 LangGraph）
  - 流程：查故障码 -> 查知识库（Yuxi Milvus）-> 安全校验 -> 证据约束生成 -> yield
- [ ] `worker.py`：`process_foxops_diagnosis`（复用 Yuxi SSE 管道，见第 8 节）
- [ ] 注册到 `WorkerSettings.functions`
- [ ] `agent_run.run_type` 加 `foxops_diagnosis`
- [ ] 证据约束生成：MVP 只做引用存在性 + 无 evidence 不下结论（蕴含校验后置）
- [ ] 验证：前端 SSE 能收到诊断流式输出 + 带引用

### P3 - W6：可信化
- [ ] 引用存在性校验（生成的 [n] 编号必须对应 evidence）
- [ ] `safety_rule` active 校验（在线校验 status，已弃用则降级）
- [ ] audit_log（`log_operation` 加 target/result，或塞 details）
- [ ] 验证：错误引用被拦 + 安全校验生效 + 操作留痕

### P4 - W7-8：管理 API + 评测 + 部署
- [ ] 管理后台 API（用户/模型/系统，复用 Yuxi 现有接口）
- [ ] 领域评测集（复用 `knowledge/eval`，20-50 条 golden set）
- [ ] 部署文档 + 演示脚本

---

## 8. SSE 管道复用（路 A，已验证可行）

**结论**（设计文档第 10 节）：Yuxi SSE 管道两端（写 `append_run_stream_event`/读 `list_run_stream_events`）都是纯 Redis，跟 LangGraph 零耦合。FoxOps 自写 worker 任务复用管道，stream 来源换成诊断 service，**不改 Yuxi 核心**。

**做法**：写 `process_foxops_diagnosis` worker 任务，复用 Yuxi 的流式基础设施：

| 复用（不改） | 用途 |
|-------------|------|
| `RunContext` | 取消信号监听 |
| `ChunkedEventWriter` | 事件批量缓冲 |
| `append_run_event` / `append_run_stream_event` | 写 Redis 事件流 |
| `mark_run_running` / `mark_run_terminal` | 更新 agent_run DB 状态 |
| `_consume_stream_with_cancel` | 消费 async generator + 取消 |

**换掉源头**：stream 来自 `foxops_diagnosis_service.diagnose(...)`（普通函数，自 yield chunk），**不调 `stream_agent_chat`**（它深度绑 LangGraph）。

**注册**：`run_worker.py` 的 `WorkerSettings.functions = [process_agent_run, process_foxops_diagnosis]`

**API 端**：`stream_agent_run_events` 直接复用（只读 Redis，不管内容谁产）。

**骨架示例**（`foxops/worker.py`，参考 `run_worker.process_agent_run` 写）：
```python
async def process_foxops_diagnosis(ctx, run_id: str):
    run = await _get_run(run_id)
    # ... 解析 input_payload（query/agent_id/...）
    await mark_run_running(run_id)
    run_ctx = RunContext(run_id=run_id)
    writer = ChunkedEventWriter(run_id=run_id, thread_id=thread_id)
    await run_ctx.start()
    try:
        stream = foxops_diagnosis_service.diagnose(query=..., user=...)  # 自 yield chunk
        async for chunk_bytes in _consume_stream_with_cancel(stream, run_ctx):
            for chunk in _iter_json_chunks(chunk_bytes):
                # ... 缓冲/映射/写事件（仿 process_agent_run 的循环）
                await writer.append(chunk, ...)
        await mark_run_terminal(run_id, "completed")
    except asyncio.CancelledError:
        await mark_run_terminal(run_id, "cancelled")
    finally:
        await run_ctx.close()
        await clear_cancel_signal(run_id)
```

---

## 9. 复用 Yuxi 清单（不改核心）

这些 Yuxi 已有能力，**直接调接口复用，不要重写**：

| 能力 | Yuxi 位置 | FoxOps 用法 |
|------|----------|------------|
| 知识库（上传/解析/分块/向量/检索） | `yuxi/knowledge/`（KnowledgeBaseManager + Milvus） | 诊断时调检索 |
| 混合检索 RRF + Rerank | `MilvusKB._fuse_chunk_rankings` + `models/rerank.py` | 复用；业务表第三路自写融合器（`foxops/fusion.py`） |
| 知识图谱 | `yuxi/knowledge/graphs/`（Neo4j） | M0 后置；后期故障码/设备建成图谱节点 |
| 会话/消息/checkpoint | `yuxi/repositories/conversation_repository` + `agent_run` | 诊断会话复用 |
| SSE 流式 | `run_queue_service`（Redis 事件流） + `agent_run_service.stream_agent_run_events` | 路 A 复用（第 8 节） |
| ARQ worker | `run_worker.py`（WorkerSettings） | 加 `process_foxops_diagnosis` 并列 |
| 认证 | `server/utils/auth_middleware.py`（Depends 链） | 路由用 `get_required_user`/`get_admin_user` |
| 权限 | 角色级（user/admin/superadmin） | 沿用，不做 permission 码 |
| 模型管理 | `yuxi/models/`（多 provider + 本地） | 域内模型用本地（Ollama/Xinference） |
| 操作日志 | `log_operation` | MVP 直接用，audit_log 结构化后置 |
| LLM 可观测 | `yuxi/services/langfuse_service.py` | 直接复用 |
| 评测 | `yuxi/knowledge/eval/` | 加领域评测集 |

---

## 10. 新建 vs 扩展 vs 后置

**新建**（FoxOps 没有，落 `yuxi/foxops/`）：
- 垂直业务表（fault_code/equipment/component/safety_rule/fault_case）-> `foxops/models.py`
- 诊断 service（M0 普通函数编排）-> `foxops/services/diagnosis_service.py`
- `process_foxops_diagnosis` worker -> `foxops/worker.py`
- FoxOps RRF 融合器（业务表第三路）-> `foxops/fusion.py`
- sensitivity 字段（业务表上，控制导出）
- 证据约束生成（MVP：引用存在性 + 无证据不下结论）

**扩展**（Yuxi 已有，小幅增强）：
- audit_log 结构化（`log_operation` 加 target/result，或塞 details）
- 领域评测集（复用 `knowledge/eval`）

**后置**（MVP 不做）：
- permission 码 / JWT 黑名单（沿用角色级）
- 完整 HITL 写操作链 / 工具风险分级
- 蕴含校验（NLI 模型，MVP 只做引用存在性）
- 设备数据联动（TDengine）
- 图谱（M0 不依赖，关系表直查为主）
- LangGraph 化（M0 走 service，跑通后再上 LangGraph）
- HTTP 请求级 trace_id（用 Langfuse trace 追踪诊断即可）

---

## 11. 数据流向单向（不可破坏）

```
PostgreSQL（业务主库，权威源）-> 派生到 Neo4j/Milvus（索引层）
```
- 图谱/向量是 PG 的派生索引，只能从 PG 单向生成，**不能反向写回**
- FoxOps 业务表在 PG，是权威源；检索索引从 PG 派生

---

## 12. 开发规范（CLAUDE.md 摘要）

- **pythonic 风格**，用较新语法（Python 3.12+）
- **不过度防御**：不在预设条件外堆 try/except 掩盖问题；错了就报错，别静默回退
- **不把代码写稀碎**：简单线性逻辑别拆一堆 helper；拆函数服务于复用/隔离副作用/降认知负担
- **路由保持薄**：业务逻辑放 service，DB 查询放 repository
- **改完更新** `docs/develop-guides/changelog.md`
- **测试**：`backend/test/` 下，遵循 `testing-guidelines.md`，提交前通过
- **格式化**：`make format`

---

## 13. Yuxi 参考文件清单（开发时对照看）

**启动与路由**：
- `server/main.py` - FastAPI 启动入口
- `server/routers/__init__.py` - 路由集中注册（foxops_router 加这里）
- `server/utils/lifespan.py` - 生命周期（建表在这里触发）
- `server/utils/auth_middleware.py` - 认证 Depends（get_required_user/get_admin_user）

**worker 与 SSE（路 A 关键参考）**：
- `yuxi/services/run_worker.py` - `process_agent_run` + `RunContext`/`ChunkedEventWriter`/`WorkerSettings`（写 `process_foxops_diagnosis` 的模板）
- `yuxi/services/agent_run_service.py` - `create_agent_run`/`enqueue_agent_run`/`stream_agent_run_events`（诊断 run 创建 + SSE 参考）
- `yuxi/services/run_queue_service.py` - `append_run_stream_event`/`list_run_stream_events`/`publish_cancel_signal`（Redis 事件流）
- `yuxi/services/chat_service.py` - `stream_agent_chat`（**LangGraph 耦合，不要调**，仅看 chunk 格式）

**存储与建表**：
- `yuxi/storage/postgres/manager.py` - `pg_manager`，`create_tables`/`ensure_business_schema`（建表机制）
- `yuxi/storage/postgres/models_business.py` - `BusinessBase` + 业务表模型（FoxOps 模型继承 BusinessBase，照格式写）

**知识库（诊断检索用）**：
- `yuxi/knowledge/` - KnowledgeBaseManager + Milvus 实现 + 检索
- `yuxi/knowledge/implementations/milvus.py` - `_fuse_chunk_rankings`（RRF 融合，FoxOps 第三路自写）

**认证**：
- `yuxi/utils/auth_utils.py` - AuthUtils（密码/token）
- `server/utils/auth_middleware.py` - `get_current_user`/`get_required_user`/`get_admin_user`/`get_superadmin_user`/`get_db`/`_verify_api_key`

---

## 14. 里程碑（与前端同步）

| 周 | 后端任务 | 前端同步 |
|----|---------|---------|
| W1-2 | 建表 + repository/service/router + CRUD API + 50 条数据 | 故障码管理页 |
| W3 | 知识库引用（上传手册 + citation） | 知识库管理 |
| W4-5 | 诊断 service + `process_foxops_diagnosis` worker + SSE + 证据约束生成 | 工作台对话联调 |
| W6 | 可信化（引用存在性 + safety 校验 + audit_log） | 引用卡片 + 安全提示 |
| W7 | 管理后台 API（复用 Yuxi） | 管理后台复刻 |
| W8 | 评测 + 打磨 + 部署 | 联调 |

---

> 有问题看设计文档，或问前端同事对齐接口。**红线：不改 Yuxi 核心；FoxOps 独立成包；数据流单向。Yuxi 后端是"参考答案"，复用它的接口，扩展自己的业务。**
