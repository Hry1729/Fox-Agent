# 单 Pi 主线 · 独立验收目录（2026-10-27）

对应执行方案第九节（工作包 D2 / D3）。本目录只做验收，不参与产品构建。

## 原则

- 只操作本目录创建的进程和目录：进程通过 `state.json` 记录的 PID 停止，**从不按进程名批量结束**。
- 只使用 `FOX_DATA_DIR` 指向的独立数据目录，**不在用户真实数据库中创建或重置验收专家**。
- 脚本不构建、不安装。缺少二进制时直接失败并说明，不假装通过。
- 任何脚本读不到的事实一律记为「未验证」，不会写成通过。

## 隔离目录

默认 `<系统临时目录>/fox-acceptance-single-pi`，可用 `FOX_ACCEPTANCE_ROOT` 覆盖（覆盖值必须在检出目录之外）。目录内：

| 路径 | 内容 |
| --- | --- |
| `data/` | `FOX_DATA_DIR`：数据库、会话文件、日志 |
| `evidence/` | 应用日志、数据库只读副本、`evidence-*.md` |
| `state.json` | 本次运行启动过的 PID 与停止结果 |

## 使用顺序

```powershell
# 1. 启动（记录 PID，使用隔离数据目录）
node apps/desktop/tests/acceptance/single-pi-20261027/launch.mjs --reset
node apps/desktop/tests/acceptance/single-pi-20261027/launch.mjs --binary <exe>

# 2. 在界面上按 D3 场景操作；每个场景完成一次就对照通过条件

# 3. 正常退出（关闭窗口），或模拟崩溃
node apps/desktop/tests/acceptance/single-pi-20261027/stop.mjs
node apps/desktop/tests/acceptance/single-pi-20261027/stop.mjs --force

# 4. 重新启动并追问前文，验证续聊

# 5. 汇总证据
node apps/desktop/tests/acceptance/single-pi-20261027/evidence.mjs
```

`stop.mjs` 只接受 `state.json` 里记录过的 PID；传入未记录的 PID 会以退出码 4 拒绝。

## 夹具内容（`fixture.json`）

- 专家 A「合同审阅专家 A」→ 资料库「城北项目资料库 A」，独特事实：验收押金 **37,500 元**（条款 A-7）、逾期违约金 **万分之三**（条款 A-9）。
- 专家 B「财务分析专家 B」→ 资料库「南岸项目资料库 B」，独特事实：质保金 **82,000 元**（条款 B-3）。
- 两套资料互相不含对方事实，用于跨专家隔离与伪造 ID 的拒绝验证。
- 资料正文在 `knowledge/`，是可在 UI 中直接上传或导入的中文 Markdown。

资料正文刻意不包含对方的数字，因此"串用"只可能来自权限或检索越界，不会因为文本本身而误判。

## 场景与通过条件

见 `fixture.json` 的 `scenarios` 与 `evidence.mjs` 输出的表格。表格的「结果」列由执行人员填写：

| 场景 | 判定要点 |
| --- | --- |
| 单驱动准入 | UI 新建、直接 IPC、旧偏好、环境变量分别请求 `pi`/`codex`/`deepseek` → 只有内置 Pi 会话；其余返回 `agent_backend_retired` |
| 专家连续对话 | 从专家头像进入、发送、离开、再点同一头像 → 同一聊天、同一绑定 |
| 资料问答 | 无项目私聊里列库、搜索、读文档 → 答案由真实 Host 工具结果支撑，引用可定位到 `knowledge/` 中的真实文件 |
| 完整重启 | 正常退出后重启并追问前文 → 恢复同一 Pi session，而不是把 UI 历史重新灌进模型 |
| 进程中断 | `stop.mjs --force` 后在受控点恢复 → 中断被识别，已执行工具不重放，未知结果不变成功 |
| 上下文重置 | 重置专家 A 的运行上下文 → 新上下文生效，历史、身份、资料绑定仍在 |
| 跨专家隔离 | A/B 各自提问对方的事实，伪造对方 `knowledgeBaseId`/`documentId` → 拒绝，且不出现对方数字 |
| 压缩后重启 | 触发 Pi 原生压缩后退出恢复 → 压缩后的上下文保留，未被 SQLite 展示历史覆盖 |
| 取消与迟到事件 | 生成中与工具等待中分别取消 → 最终状态唯一，取消后不跳回成功 |
| 会话损坏 | 在 `data/` 副本中删除或破坏会话文件 → 明确恢复错误，不静默创建空会话 |
| Windows 路径 | 用中文、空格、较长路径的 `FOX_ACCEPTANCE_ROOT` 重跑 → 启动、保存、恢复、退出正常 |

## 已知限制

- 脚本不驱动界面：界面操作的观察与判定由人完成，脚本只提供隔离启动、精确停止和可核对的事实。
- 在未构建桌面二进制、或未配置可用模型时，`launch.mjs` 会以退出码 2 失败；此时本目录的全部场景保持「未验证」。

## 用夹具程序建好两位专家（D2 快照）

界面里「新建资料库 / 新建专家」的按钮在 WebView 上不可靠，所以这一步由程序完成：
`apps/desktop/src-tauri/src/bin/single-pi-acceptance-fixture.rs`。

```powershell
$env:FOX_DATA_DIR = "$env:TEMP\fox-fixture-check\data"      # 必须是绝对路径
cargo build -p fox-desktop --bin single-pi-acceptance-fixture

# 1. 建好两位专家、两个资料库、两条会话
#    （幂等：重复执行按名称/标题查重后复用，不会新增任何一行）
.\apps\desktop\src-tauri\target\debug\single-pi-acceptance-fixture.exe seed

# 2. 记录证据（同一个 JSON 直接贴进 evidence-*.md）
.\apps\desktop\src-tauri\target\debug\single-pi-acceptance-fixture.exe dump
```

- stdout 只有一个 JSON 对象，进度写 stderr。
- 只接受绝对路径的 `FOX_DATA_DIR`；目录里已经有 `fox.db` 却没有 `.fox-single-pi-acceptance`
  标记时直接拒绝（退出码 2），所以不可能写进 `%APPDATA%\com.fox.agent`。
- 建库、导入文档、建专家、绑定专家与资料库全部走产品 API
  （`LocalKnowledgeStore::create_base` / `start_import`、`Database::save_agent` /
  `bind_conversation_expert` / `set_knowledge_reference_bindings`），没有直接写 SQL。
- 会话用 `create_conversation_with_backend(..., "fox")` 建成，再绑定专家：
  会话只能属于助手，专家是通过 `conversation_expert_bindings` 挂上去的
  （与界面里点专家头像进入时同一条路径）。
- `dump.searchProbes` 用 Host 的 `search_knowledge` / `read_knowledge_document` 对本地库
  调用的同一对方法（`LocalKnowledgeStore::search_lexical` / `read_document`）检索夹具事实，
  并复刻 Host 的绑定校验；**完整的 Host 工具层（工具守卫、审批、Run 记录）不在此程序内验证**。

`fixture.json` 的 `seed` 段与每条 `questions` / `isolationProbes` 的 `searchQuery` 字段
就是本程序使用的稳定标识与短检索词（长中文问题按 Host 的提示需要退化成短关键词才有命中）。