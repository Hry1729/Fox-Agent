# Fox Harness v1 — 固定任务集与独立 checker（角色 E）

版本 `harness-v1.2`（含协调者的证据类型、执行批次与完整验收门禁修正）；对应契约 CONTRACTS v1.2；初始产品基线 `5cb665c50dd267c9bc45bbf8b9af4cedfd4efa40`，报告另记实际产品 commit 和文件哈希。

本目录是**评测基础设施**，不是产品实现。它提供可离线执行的固定任务、独立 checker、运行器和结果清单。

## 1. 它是什么 / 不是什么

- **是**：对冻结契约（EX-v1 / RD-v1）的独立消费者验证。checker 只从 fixture 状态与工具结果独立重算预期值，不引用模型自评，也不从修改后输出反推正确答案。
- **不是**产品能力证明。本批次没有模型参与决策；`real_eval_tests.rs` 的 synthetic-provider / real-cloud-model 三层分层仍然独立保留，本 harness 不替代、不覆盖它们。
- 本目录的自身 fixture 测试（`harness-self.test.mjs`）只检验 checker 本身的正确性（能拒绝坏输出、接受好输出），**不算产品能力通过**。
- 产品 SHA 与 checker SHA 严格分离：运行器接受 `--product-root` 指向被测产品检出，默认为本 worktree（基线 `5cb665c`）。O 集成 A/B 修复后可指向集成 SHA 重跑。

## 2. 证据分层

| tier | 含义 | 本批次任务 |
|---|---|---|
| `tool-contract` | 直接驱动真实 Node 只读工具执行器，输入固定脚本化；无模型轮次 | E-T-03、E-T-03b、E-T-04a、E-T-04b、E-T-04c |
| `rust-host` | 直接调用真实 Host/DB/reader，含实际命令与文件操作；局部 model_receipt，不驱动 Node 模型 worker | E-T-01、E-T-02、E-T-05 |

`tool-contract` 属于 C01 三层中的**契约层**，不是 synthetic-provider 真实 Host 层，更不是 real-cloud-model 层。缺 Rust 编译时 `rust-host` 任务记 `not_run` + `notRunReason`，不伪装通过。

## 3. 运行方法

```bash
# 在 services/agent-runtime 下（需 dot-source Enter-Role.ps1 -Role E 后）
node evals/harness-v1/run-harness.mjs --output "$FOX_EVAL_OUTPUT_ROOT"

# 只跑指定任务
node evals/harness-v1/run-harness.mjs --only E-T-03,E-T-04a

# 指向其它产品检出（例如 O 集成后的 SHA）
node evals/harness-v1/run-harness.mjs --product-root /path/to/product-checkout

# checker 自身正确性测试
node --test evals/harness-v1/harness-self.test.mjs
```

默认 `--mode acceptance`：产品失败退出1；任一选定任务未执行、证据缺失或被拒退出3；只有选定任务全部执行且通过才退出0。`--mode baseline` 如实记录失败并退出0，仅供基线采集。非法参数退出2。可以用 `--only` 明确选定要验收的任务子集。

Rust证据的生成与导入须在同一隔离环境设置一个新的 `FOX_HARNESS_EXECUTION_ID`（例如本轮RunId加UUID），并共用 `FOX_EVAL_OUTPUT_ROOT`。先按tasks.mjs精确Cargo filter执行三项，再运行Node验收；代码/commit/checker变化后应重新生成。错误类型、重复/缺失文件清单、非布尔结果、旧批次ID均拒绝，72小时新鲜度不能代替本次执行身份。无该ID时Rust可以产出诊断，但不能被作为本次验收证据导入。

Rust记录的六个被测产品文件哈希来自编译时嵌入的源码字节，导入时与当前工作区比较；旧测试二进制不能仅靠读取新源码给自己贴上新版本标签。这是所列文件范围内的身份校验，不是整个依赖树或发布二进制的构建证明。

## 4. 任务清单与基线预期

| ID | 标题 | 问题引用 | 要求 | 基线 5cb665c 预期 |
|---|---|---|---|---|
| E-T-03 | 只读分页往返无损 | B02/B03 | offset/limit 分页可精确重建原文件；部分页 truncated=true | **pass**（既有能力，验证 harness 与真实能力） |
| E-T-03b | UTF-16 边界安全 | B03/RD-v1 | 落在代理对内部的 offset 不得返回半截代理对 | **fail**（返回 lone surrogate，已复现） |
| E-T-04a | 行模式参数成对校验 | B03/RD-v1 | 缺 lineCount、负数、小数必须拒绝 | **fail**（默默补默认值） |
| E-T-04b | 行模式 pageComplete 语义 | B02/RD-v1 | 请求文本被上限丢弃时 pageComplete 必须 false 且字段存在 | **fail**（无 pageComplete 字段） |
| E-T-04c | 搜索扫描/匹配区分 | B01/B02/RD-v1 | matchLimitReached/scanComplete/scanTruncated/returnedCount/totalMatches 区分可见 | **fail**（Node 路径只有 count） |
| E-T-01 | stdout-only 非零退出 | A01/EX-v1 | exitCode 保留、诊断到达模型、isError=true | not_run（需 Rust Host 编译槽） |
| E-T-02 | 合法文本删除 | A03/EX-v1 | 删除至空内容允许；空 oldText 拒绝 | not_run（需 Rust Host 编译槽） |
| E-T-05 | 越权拒绝 | S01/RD-v1 | 项目根外读取被拒，不伪装为跳过 | not_run（Rust Host 层作用域校验） |

## 5. 与既有评测入口的关系

- `evals/manifests/phase-0a/0b` 与 `src/offline-evaluator.mjs` 保持不变；本 harness 是新增独立清单，不改它们。
- `real_eval_tests.rs` 的既有完整链路入口保持不变；本目录的三个Rust Host定向用例与之并列，不能冒充完整synthetic-provider或云模型证据。
