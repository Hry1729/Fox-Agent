# DSH T3 验收摘要（2026-10-29）

## 版本与产物

- 分支：`dsh-fix-20261029`
- 当前 HEAD：`b5311102`
- 候选：`D:\python\projects\Fox\output\native-dsh-fix-20261029-1740-streaming\payload`
- desktop SHA-256：`f24a141ad8508977e31c97bcf278bf5fb7bf79cd594960ba393b7ed387432280`
- sidecar SHA-256：`3d701aa08fc4d980749c6c3c5dee030553713699603568821f1d7e97100c5ec0`
- NSIS SHA-256：`59f855d62fc50cd51c248c671c8b85a74f8772ced816b8d241fcfbf374989853`

## 交付提交说明（更正）

`b5311102 test(t3): add isolated approval and live budget probes` 只提交了两个测试文件、共 56 行：

- `apps/desktop/src-tauri/src/runtime_host/native_approval_scope.rs`
- `apps/desktop/src-tauri/src/runtime_host/native_model_budget.rs`

它不包含验收文档。验收记录原先位于仓库外的 `D:/python/projects/Fox/migration-work/`；本文件是纳入仓库的摘要副本。

## T3 场景

顺序 1、2、3、4、6 的通过证据见 `D:/python/projects/Fox/migration-work/dsh-t3-closeout-20261029.md` 和 `dsh-t3-order4-valid-20261029.json`。本轮只补顺序 5。

### 顺序 5：真实 Provider

成功压缩 Run：`9c38c126-717d-4898-8860-74838364407f`，completed。

- 配置：`contextWindow=24576`、`maxOutputTokens=4096`、`total=null`、`output=null`
- `context.compaction.started`（overflow）→ `context.compaction.completed`（willRetry=true）
- 持久压缩记录：`d5af5adf`，native session 第 59 条
- 压缩后 agent 请求 `21258847...`：input 24021 / output 109
- 最终 assistant：`24576 压缩测试。`，completed

重试 + 并发：

- A `a629926b-fe99-4d91-a552-10fb83260bcb`：代理对 A 第一个真实请求注入一次 HTTP 503，未转发、无 usage；Host failed 请求 `10656141...`，重试 `8ed40c59...` 由真实 Provider completed（11697 / 43）
- B `2e8d7f89-a91a-4ef4-979b-3066c3273633`：真实请求 `929a447e...` completed（11704 / 763）
- 配置：`contextWindow=24576`、`maxOutputTokens=8192`、`total=null`、`output=null`

**边界更正：** 真实场景是 `total=null`、`output=null`，仅单次请求输出有上限。累计预算限制与耗尽保护仍由此前的可控 Provider 证据覆盖，不把本次成功场景写成累计预算耗尽验收。

- 无重复 request_id，无重复结算；本轮三个 Run 无 `status=admitted` 的未结算请求。
- 代理不记录 Authorization 或密钥。

## 验收结论

T3 本轮范围可以关闭；这是工程验收结论，不代表已合并、推送或发布。没有发现新的生产缺陷。

## 合并范围

`git log main..HEAD` 包含 9 个提交（含初始 WIP 与本次修复/测试）。`main..HEAD` 共 121 个文件、18641 insertions / 963 deletions（在加入本摘要前统计）。未提交任何 `migration-work/` 历史夹具或 `.probe-s0/`。
