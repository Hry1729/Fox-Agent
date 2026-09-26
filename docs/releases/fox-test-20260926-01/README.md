# Fox 内测候选 fox-test-20260926-01

这是源码测试候选，不是已发布安装包。最终提交由同名 annotated Git tag 固定；后续修复使用新版本标识，不移动本标签。

## 本轮范围

- F1 模型实际交付与整文件覆盖资格、F2 restore 认领/去重/结算、F3 live/逐轮审批入口修复。
- compute Job 终态与通知原子保存、安全投递、等待/续答、去重、取消与重启对账。
- C 阶段验证已有批量通知能力；C4 通知/轮询均为 3 次模型请求，通知减少两次 status 工具调用，不证明真实费用下降或稳定延迟收益。
- 最后补充通知容量/未决通知冲突的明确终态诊断，保留原有上限、失败保护与审批预算。

## 验证口径

历史固定点 ed104ea670315b3dc4661470d4428c84d3e5101c：协调者独立 Windows Rust lib 全量 1320/0/27，164.37 秒。原始日志与 runner JSON 在 evidence；不冒充本候选补丁后结果。

补丁后固定点与结果见本页后续“最终验证”。Rust 验证使用 --offline --no-default-features --lib、默认测试并行、LIBSQLITE3_FLAGS=SQLITE_DEFAULT_MEMSTATUS=0、ZVEC_AUTO_BUILD=0、真实 zvec DLL、隔离 FOX_DATA_DIR 和 TMP。测试脚本不等于默认安装包构建。

证据层级：SQLite/组件、Authoritative Host、真实 Node/Pi live 与逐轮、本地模拟 Provider 各按测试所属范围解释；全量汇总不把所有用例升级成端到端验收。未运行真实付费 Provider；真实桌面、默认 features Windows 安装包、跨机器安装仍待验收。

## 固定输入与构建

inputs.sha256.json 固定锁文件及配置哈希，源码 tag 同时固定 Runtime 与受管资源清单。资源生成/外部下载的最终产物版本和哈希必须在实际构建时另记，当前没有产物哈希。

计划的标准 Windows 安装包构建命令为 `pnpm tauri:build:windows`（见根 package.json）；尚未执行，不把此命令当作构建成功记录。记录工具链版本、features、依赖安装日志、sidecar/资源哈希、安装包 SHA-256 后才交付安装包。

## 测试配置与开放项

- 常规组：新建隔离数据目录和 Run，通知实验默认关闭。
- 实验组：明确 Authoritative + Pi，`FOX_EXPERIMENTAL_COMPUTE_JOB_NOTICE=1`，新建 Run 并检查冻结配置；Run 创建后切环境变量不改变既有 Run。
- 自动 wake 不受通知实验开关门控，要求 Authoritative；这是本轮有意集成的既有行为，通知 flag off 不是整个后台等待机制关闭。
- Provider、模型、预算与题目版本在真实验收时固定；密钥不写入清单。不得启动任意 Shell 或关闭准入保护。
- 历史 F3 20 秒超时与 P95 写入波动的排他根因仍开放；不能以本次绿色关闭。
- 27 个 ignored 及真实桌面/模型/安装包未验收保持记录；参见历史交付清单。

## 下一阶段烟测

依次验证启动/数据库副本升级重开、大小文件读改及拒绝、审批期间切换模式、恢复双击/中断、一个知识引用任务与一个文档任务、双 compute Job 通知/取消/重启。冻结候选不变，开发补丁另走分支形成下一候选；数据回退使用匹配备份。

历史交回原文在 ../../f3-timeout-20260924，保留原貌，其当时的分支/未提交状态不是本候选当前状态。

## 最后诊断修复的边界

源码提交 `3cedbed42586808ea3d63eaf94a349d182b4ca5c`：仅在真实 Host 终态分类中保留 `kernel.job_notice_competition` 及固定消息，提示容量不足或并发状态变化均可能造成未处理事实；不持久化任意原始错误文本。原本 `kernel.job_notice_capacity_blocked` 的 wake 路径保持原样返回明确错误并维持 parked，未改为强制终结。开发者定向容量组 4/0（含 live/逐轮真实 Host）、失败分类组 7/0；源提交前运行的 JSON 如实保留 WIP 状态，不冒充提交后独立证据。

## 最终验证

协调者在固定源码提交 `3cedbed42586808ea3d63eaf94a349d182b4ca5c` 独立运行默认并行 Rust lib 全量：**1321 通过 / 0 失败 / 27 忽略**，退出码 0，97.71 秒，外层上限 600 秒。完整命令、环境、源码状态和日志见 `evidence/freeze-final-full-3cedbed.{json,log}`。后续候选归档提交仅包含文档/证据，与该测试源码一致，不声称在归档提交上重跑。源码测试 PASS；尚未产出或验收 Windows 安装包。

原两份 schema 的工作区字节经 Git 规范化后均与 HEAD blob `328664596978cc7913c66cc10dbc6b9c0fb14f94` 一致；仅刷新索引，不提交 schema 改动。三份历史交回文档保留原始内容并正式归档，不删除原资料。

archive.sha256.json 按 Git 存储内容计算 SHA-256（使用 git show TAG:path 校验），不按 Windows checkout 的 CRLF 转换后字节计算。
