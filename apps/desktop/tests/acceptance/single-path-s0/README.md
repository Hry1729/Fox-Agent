# S0 探针证据归档 · 单路径运行时收敛

日期：2026-10-28
来源：`services/agent-runtime/.probe-s0/`（未跟踪的临时探针目录），S1 第 1 天按方案归档到此。
源码基线：`main @ daa799ffd22dcc1e5fd17815643ef4ba4b1247ff`
方案：[Fox 单路径运行时收敛执行方案](../../../../docs/05-开发指南/Fox单路径运行时收敛执行方案-20261028.md)

这里保存的是 **Pi 1.4.0 SDK 行为探针**的脚本与原始输出，是方案第二节 D1–D10 十项决策的依据。**不是产品验收**：没有连接 Fox Host、没有真实模型、没有用户数据，全部使用 faux provider 在独立进程内运行。

## 文件

| 文件 | 内容 |
| --- | --- |
| `RESULT.md` | 探针结论 P1–P8、Q1–Q4，逐条附原始输出定位 |
| `PART-B-raw.md` | 源码盘点：`state.worker` 的 29 处使用、删除清单、Yuxi/ACP 不经过 worker 的核对 |
| `p1-mixed-batch.mjs` | 混合批次的只读前缀并行与写入屏障时间线 → D2 |
| `p2-batch-registration.mjs` | 首次 `beforeToolCall` 中幂等登记整批调用 → D3 |
| `p2b-settle-deadlock.mjs` | 反证：钩子等待结算会死锁且 abort 解不开 → D3 的硬性禁区 |
| `p3-missing-execute.mjs` | `tool_execution_end` 覆盖未执行位置；`Value.Check` 预校验与 SDK 不一致 → D4 |
| `p4-prep-abort.mjs` | 准备阶段中止后留下孤立调用 → D4、D7 |
| `p5-exec-cancel.mjs` | SDK 不拦截已开始的工具，`Promise.all` 等最慢者 → D5 |
| `p6-persistence.mjs` | 没有公开的落盘事件，`appendFileSync` 不 fsync → D6 |
| `p7-resume.mjs` / `p7-artifact.mjs` | 打开会话时 `appendMessage` 补交、`branch()` 两种形态；SDK 把孤立调用静默补成 `No result provided` → D7 |
| `p8-multi-session.mjs` | 同进程两个会话的事件/abort/工具隔离与内存数据 → D1、D9 |
| `lib/` | 共用夹具：`harness.mjs`、`session.mjs`、`cold-start.mjs` |
| `out-*.txt` | 各脚本的原始标准输出，`RESULT.md` 的定位依据 |
| `SDK-HASHES.json` | 本轮结论所依据的 SDK 构建：14 个已安装 `@earendil-works/pi-*` 包的版本与 `package.json` 哈希，以及 `RESULT.md`/`PART-B-raw.md` 引用的 8 个 dist 文件的 sha256 |

`out-p4.txt`、`out-p5.txt`、`out-p6.txt` 是 `p4/p5/p6` 早期一轮的输出，与改名后的 `out-p4-prep-abort.txt` 等并存，保留以对应 `RESULT.md` 中的引用。原目录下的 `tmp/`、`lib/tmp/` 是探针运行时的一次性会话目录和 scratch 脚本，不属于结论证据，未归档。

## 重跑

脚本从 `services/agent-runtime` 解析 `@earendil-works/pi-*`，需要在仓库根安装依赖后运行：

```powershell
node apps/desktop/tests/acceptance/single-path-s0/p1-mixed-batch.mjs
```

重跑前先核对 `SDK-HASHES.json`：SDK 版本或 dist 内容变化后，原结论不再自动成立，尤其是 D2 的屏障时间线和 D4 的事件覆盖。

## 证据边界

- 全部结论基于锁定版本 `1.4.0`（`services/agent-runtime/package.json` 精确固定，无 caret）。
- 探针在独立 Node 进程内用 faux provider 运行，**没有**经过 Fox Host 的准入、审批、台账或沙箱。
- 未在真实 provider 上验证 aborted assistant 是否被丢弃（方案附录 B 列为 S3 的真实模型候选项）。
- 内存与冷启动数据（常驻约 75 MiB、每会话 2–8 MiB、import 754 ms）来自本机单次测量，不是容量结论；D9 的 2 个活动 Run 仍是候选值，S4 需复测 3、4 个。
- 原 `.probe-s0/` 目录未删除，此归档是副本。确认无其他引用后可由负责人清理。
