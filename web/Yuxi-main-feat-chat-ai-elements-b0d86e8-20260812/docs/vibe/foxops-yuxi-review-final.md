# FoxOps 基于 Yuxi 开发 · 可行性评审意见

> 审阅方：Claude Code × Codex × DeepSeek
> 日期：2026-07-08
> 结论：**可行。基于 Yuxi 是两个月内唯一现实路线。**

---

## 一、核心判断

| 维度 | 结论 |
|------|------|
| 方向 | 正确。Yuxi 覆盖通用底座，FoxOps 只补垂直业务层 |
| 真实复用率 | 约 50-60%（配置/集成调试/融合改造有成本） |
| 两个月可交付 | 港口故障码管理 + 知识库检索 + 带引用诊断问答 + 安全规则提示 |
| 不可交付 | 桌面端、完整 HITL 写链、设备数据联动、复杂权限体系 |

---

## 二、必须调整的设计决策（3 条）

### 1. 前端只做 Vue，砍掉 React 桌面端

两套前端（Vue 管理 + React 桌面）= 两套 SSE Client、两套状态管理、两套组件库、每次 API 变更改两遍。

**改为**：Vue Web 端做全部交付（管理 + 工作台），桌面端后置。如果需要"工作台感"，在 Vue 里加 `/foxops/workbench` 页面。

### 2. M0 不依赖图谱/Agent 跑通主流程

Neo4j + Milvus 部署重、调试复杂。主流程应该用关系表直查（故障码→原因→措施→安全规则），文档检索走 Yuxi Milvus，图谱作增强不作地基。

诊断逻辑先用普通 service 编排，不走 LangGraph：

```python
async def diagnose(query, user):
    fault_hit = await fault_code_service.search(query)
    kb_hits = await yuxi_retriever.search(query)
    evidence = build_evidence(fault_hit, kb_hits)
    safety = await safety_check(evidence)
    answer = await model.generate(build_prompt(query, evidence, safety))
    return DiagnosisResult(answer=answer, evidence=evidence, safety=safety)
```

跑通后再 LangGraph 化。避免改 Yuxi 核心中间件影响原有功能。

### 3. 权限沿用 Yuxi 角色级，不做 permission 码

"加 permission 码"是对 Yuxi 鉴权中间件的结构性手术（数据模型+中间件判据+API Key 映射三层改造）。

**改为**：M0/M1 直接用 Yuxi 现有角色（user/admin/superadmin），admin 管理故障码，user 只读查询。sensitivity 字段加在业务表上控制导出，不改鉴权链路。

---

## 三、关键技术风险（4 条）

| 风险 | 说明 | 应对 |
|------|------|------|
| Yuxi RRF 融合不支持第三路 | `_fuse_chunk_rankings` 只融合 chunk+graph 两路，FoxOps 多了"业务表精确匹配"第三路 | 写 FoxOps 专用融合器，不改 Yuxi 原有融合逻辑 |
| 蕴含校验（entailment）复杂度高 | 判断"evidence 是否真正支撑结论"需要 NLI 模型，延迟和可靠性都难保证 | MVP 只做引用编号存在性校验 + 无 evidence 不下结论 + safety 降级；蕴含校验后置 |
| Milvus 中文检索覆盖度未知 | Yuxi 的 Milvus+LightRAG 对中文维修手册的召回率未验证 | W0 Spike 必须用真实手册数据验证；不达标则补 pg_trgm 全文路 |
| Docker 资源门槛 | PG+Redis+MinIO+Milvus+Neo4j+worker 全起至少 16GB RAM | 定义 LITE_MODE：Neo4j 可选、OCR 容器关闭、用可解析 PDF/Markdown |

---

## 四、工程隔离原则

FoxOps 代码独立成包，不散落进 Yuxi 原有模块：

```
backend/package/yuxi/foxops/
├── models.py
├── repositories.py
├── services/
│   ├── fault_code_service.py
│   ├── diagnosis_service.py
│   └── safety_service.py
├── schemas.py        # Evidence / Citation / DiagnosisResult 类型
└── eval/

backend/server/routers/foxops.py
```

数据流向单向：PostgreSQL（业务主库）→ 派生到 Neo4j/Milvus（索引层）。不允许双边可编辑。

---

## 五、排期建议（8 周）

| 周 | 目标 | 交付物 |
|---|---|---|
| W0 | Spike | Docker Compose 跑通 + 中文检索效果验证 + 新增 router 成功 |
| W1-2 | 垂直数据层 | fault_code/cause/action/safety_rule 表 + CRUD API + Vue 管理页 + 50 条数据 |
| W3 | 知识库引用 | 上传手册 + citation 关联 + 引用展示 |
| W4-5 | 诊断 service + SSE | 故障码/自然语言→检索→证据约束生成→流式输出 |
| W6 | 可信化 | 引用存在性校验 + safety_rule active 校验 + audit_log |
| W7 | 工作台页面 | Vue `/foxops/workbench`：搜索 + 对话 + 引用卡片 + 安全提示 |
| W8 | 打磨 | golden set 20-50 条 + 修 bug + 部署文档 + 演示脚本 |

---

## 六、最终交付定义

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

## 七、W0 Spike 必须回答的问题

1. Docker Compose 能否在团队环境一键跑通？
2. Yuxi 检索对中文维修手册的召回率是否可接受？
3. 新增 router/service/repository 的扩展模式是否顺畅？
4. Yuxi SSE 事件格式能否承载诊断专用事件？
5. ingestion pipeline 失败后能否重试（不需要断点续跑，能重跑即可）？

W0 如果有 2 个以上卡住，立即调整方案。
