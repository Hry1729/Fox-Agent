# 小 Fox 独立 Agent 改造：交接文档

## Windows 续做（2026-11-12）

最新修复及商汤复验见 [reacceptance-20261112.md](reacceptance-20261112.md)。偏好、共享记忆、计划和委派已复验通过；此前超时未复现，根因尚未确定。外部查看器内容和定位视觉效果未完成目视确认。上一轮失败记录保留在 [acceptance-20261112.md](acceptance-20261112.md)。

已合并云端 `bbb009c3` 与本地 `4142498d`，合并提交为 `591f20e2`。阶段4和阶段5已实现，当前状态与 Windows 验证见 [windows-result.md](windows-result.md)。下文保留云端交接时的状态；其中“未完成”“未开始”不代表本次续做后的状态。

更新：2026-11-12 15:50（北京时间）
依据：`spec.md`（原始方案，§10 工作包，验收 A01–A24）
基线：`792c0684`。之后的 5 个提交已合进 main。

## 一、已完成（已推 main）

| 提交 | 内容 |
|---|---|
| `1b8ed637` | 阶段1：小 Fox 独立身份、配置快照、来源专家可空、空白创建路径、项目笔记按 companion id 作用域 |
| `8d7ca891` | 阶段2：`companion_profile_update` 对话改配置工具、加号新建空白小 Fox、侧栏职责与沟通偏好编辑（保存后按后端归一化值对账） |
| `d59e63d2` | 阶段3a：v115 迁移；`database/schedule.rs` 带 IANA 时区的调度计算（once / interval / daily / weekly，DST 处理，补跑窗口 3600s，间隔 60..=2592000s） |
| `43ca1263` | 阶段3b：8 个 `companion_task_*` 对话工具（协议 index 69–76，排在 `companion_profile_update` 之后），归属任务服务，更新时保留已存时区和启用状态 |
| `95f44258` | 阶段3c：联系人侧栏「定时任务」子页（列表/添加/编辑/启停/立即执行/两步删除/历史、「排队中」状态） |

每个阶段都经过 gpt-6.1-sol 只读审查，审出的 P0–P2 都已修复，复审没有再发现新问题。

开源复用（详见 `oss-reuse-notes.md`）：
- OpenMausBot `1a4e4da9`（Apache-2.0）：参考了下次执行时间和工具形态。`enterprise/` 是单独的商业许可，没有使用。
- Hermes Agent `45a694b9`（MIT）：参考了时区规范化、DST fold，以及 `cronjob_manage` 的拆分方式。
- 署名写在 `third_party/openmausbot/`、`third_party/hermes-agent/` 的 NOTICE 和 FOX-PORT.md 里。

## 二、测试状态（box / Linux）

- Rust：`digital_colleagues` 28/0、schedule 8/0、owned_task 3/0、`database::migrations` 67/0/1 ignored；`cargo check --lib` 通过。
- Runtime（Node 24，`pnpm --dir services/agent-runtime test`）：545 pass / 0 fail / 1 TODO（原有）。
- Desktop Bun：基线原有 8 个失败，本次无新增。`contact-files.test.tsx` 的「按今天/更早分组」偶发失败，单独跑能通过。
- `tsc --noEmit`、`protocol:check`、`git diff --check` 通过。
- 没做 Windows 测试。需要真实模型和真实桌面的验收（A05、A11、A16、A17、A23）都没做，没有按通过算。逐条状态见 `acceptance.md`。

## 三、未完成：阶段4 子 Agent 归属（没有合进 main）

任务说明：`phase4-task.md`（写清了现状缺口、必须实现的 6 条行为、测试清单）。

要点：
1. `child_runs.rs` 创建子会话时不写 `companion_id`，`companion_ownership.rs::resolve_conversation_on` 也不沿 `parent_conversation_id` / `parent_run_id` 往上找。结果是小 Fox 的子 Run 会走普通对话路径，绕过工作区和权限限制。要改成沿父链解析到根 Run 的小 Fox。链断了就报错停止，不能回退成普通路径。
2. 子 Run 不能占小 Fox 的执行位置，否则父 Run 在 `child_run_collect` 里等子 Run 时会死锁。父 Run 结束或停止时，要先停掉所有子 Run。`drain_companion_runs` 和 `companion_active_run_ids` 也要把子 Run 算进去。
3. 子 Run 继承父 Run 冻结的能力集。撤销权限要立即生效。不能给子 Run 提供 `companion_profile_update` 和写笔记工具。
4. 预算（A17）：父子 Run 的 token 和工具调用次数都累计到根 Run 上，每个 Run 只取最新的累计值，不要重复计算。
5. 最后一步才是把 `child_run_start/collect/cancel` 加进 `companion_workspace::allows_tool` 白名单，而且要等 A23 自动化测试全部通过之后。现在这几个工具不在白名单里，这是对的。

草稿：`phase4-wip.patch`，只改了 `companion_ownership.rs`（+111/−20），**编译不过**，因为 `queue_slot()` 改成 `Option` 后和 `digital_colleagues` 里的断言冲突。它可以当思路参考，直接 apply 不行。之前的模型跑了 2.5 小时也没收敛，建议拆开来做：先 1+2，再 3，再 4，最后加白名单。

## 四、未开始：阶段5 清理旧「数字同事」入口

见 `spec.md` §10 最后一个工作包。

## 五、回家后的操作

```bash
git fetch origin && git checkout main && git pull
pnpm install
# Rust 侧有 v115 迁移和新依赖 chrono-tz 0.10 / iana-time-zone 0.1，第一次编译会稍慢
```

注意：
- 数据库会自动从 v114 升到 v115，现有的间隔计划会原样迁移。建议先备份本地数据库。
- `apps/desktop/src-tauri/gen/schemas/linux-schema.json` 是在 box 上生成的文件，没有提交。
- 之前说的 `4142498d`（聊天与联系人 UI 改造）在 GitHub 上一直没出现，这次是基于 `792c0684` 做的。如果你本地有这个提交，pull 之后要和它合并。冲突大概率在 `ContactSideCard` 相关文件里。
