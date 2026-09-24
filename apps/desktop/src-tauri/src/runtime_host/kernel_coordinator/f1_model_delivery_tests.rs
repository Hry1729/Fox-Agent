//! A-F1：**模型实际交付**与整文件替换资格。
//!
//! 缺陷：`read` 在 Host 侧完整取得源文件后登记 `FullFile/covered_whole_file`
//! （源事实），而模型看到的是这份结果的后续生产投影（头 6 000 / 尾 2 000 /
//! 上限 9 000 字节）。准入过去只看源事实，于是"Host 读了全文、模型没收到中间
//! 部分"的 `write_file` 仍被当成"已全文读过"而放行整文件替换。
//!
//! 本文件的所有用例都走**真实入口**：真实 `Database`、真实项目绑定、真实
//! `managed_files::execute_observed_reader` 读取缝（并真实落 `tool_calls` 行）、
//! 真实 `KernelCoordinator::start_prepared` + 真实策略 + 真实 Host 准入与执行缝。
//! 唯一被脚本化的依赖是模型提案本身，与既有 `build_kernel_run` 的做法一致。
//!
//! 明确的限制（不写"已通过"）：这里不启动 Node worker，因此
//! Live Pi / 逐轮**引擎**分支不在本文件覆盖；本文件覆盖的是 Host 侧的完整交付链
//! （读取 → durable 行 → 生产投影 → 交付事实 → 准入 → 真实文件提交）。

use super::*;

use crate::database::Database;
use crate::kernel::CancellationRegistry;
use crate::runtime_host::{kernel_gateway, managed_files};
use crate::tool_host;
use fox_engine_protocol::HostObservation;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 大文件正文：唯一标记落在头 6 000 / 尾 2 000 之外。
const F1_MARKER: &str = "FOX-F1-MIDDLE-MARKER-7c1f9a2d-NEVER-DELIVERED";

fn f1_big_body() -> String {
    format!("{}{}{}", "H".repeat(15_000), F1_MARKER, "T".repeat(15_000))
}

/// 经**生产 Kernel 读派发缝**读一次：真实 `kernel_tool_calls` 行 + 真实观察登记。
///
/// 与 `host_read` 的区别正是 A-F1 依赖的那一点：durable 结果必须真实存在，
/// 因为交付事实只能从 durable 行重放。这里不手造任何结果——读走
/// `execute_claimed_dispatch` + `GatewayPolicy::execute_reader`，也就是
/// `kernel_host::drive` 的 `DispatchTool` 分支所用的同一条路。
fn kernel_read_result(db: &Database, run_id: &str, call: &str) -> Value {
    let raw: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
                rusqlite::params![run_id, call],
                |r| r.get(0),
            )
        })
        .expect("the settled Kernel read row");
    serde_json::from_str(&raw).expect("durable Kernel result json")
}

fn durable_result_json(db: &Database, run_id: &str, call: &str) -> Value {
    let raw: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
                rusqlite::params![run_id, call],
                |r| r.get(0),
            )
        })
        .expect("the durable result row");
    serde_json::from_str(&raw).expect("durable result json")
}

/// 真实生产投影：Host 交给 worker、最终进入 Provider 请求的那条工具结果消息。
///
/// 走 `KernelCoordinator::tool_result_messages`——`live.rs` 里给模型组装
/// `role:"toolResult"` 的同一个函数——而不是测试自己实现的裁剪。
fn model_visible_tool_result(
    db: &Database,
    run_id: &str,
    call: &str,
    tool: &str,
    input: &Value,
) -> Value {
    let result = durable_result_json(db, run_id, call);
    let storage = db
        .tool_result_storage(run_id, call)
        .ok()
        .and_then(|storage| serde_json::to_value(storage).ok());
    let settled = vec![fox_engine_protocol::KernelSettledToolResult {
        tool_call_id: call.to_owned(),
        tool: tool.to_owned(),
        canonical_input: input.clone(),
        source_order: 0,
        state: fox_engine_protocol::KernelSettledToolState::Completed,
        result,
        storage,
    }];
    let messages = KernelCoordinator::tool_result_messages(run_id, &json!({"timestamp":0}), &settled);
    messages
        .first()
        .expect("one settled tool message")
        .get("content")
        .cloned()
        .expect("content")
}

fn observation(
    db: &Database,
    run_id: &str,
    root: &Path,
    path: &str,
    version: &str,
) -> HostObservation {
    let target = tool_host::canonical_file_identity(root, path).expect("target identity");
    db.host_observation_for_version(run_id, &target, Some(version))
        .expect("observation lookup")
        .expect("the read registered an observation")
}

fn f1_replace_grant_state(db: &Database, run: &str) -> Option<String> {
    db.with_connection(|c| {
        c.query_row(
            "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
            [run],
            |r| r.get::<_, String>(0),
        )
        .optional()
    })
    .expect("replacement grant state")
}

fn f1_run_state(db: &Database, run: &str) -> String {
    db.with_connection(|c| {
        c.query_row("SELECT state FROM kernel_runs WHERE run_id=?1", [run], |r| {
            r.get::<_, String>(0)
        })
    })
    .expect("kernel run state")
}

/// 真实 Host 执行缝：每个待派发工具都经 `execute_claimed_dispatch`（它做准入、
/// 认领并原子消费专门替换授权）与共享的受管文件提交缝执行，与
/// `kernel_host::drive` 的 `DispatchTool` 分支是同一条路径。
///
/// 循环有硬上界：每次只处理执行前快照里的一个待派发 effect，且要求 effect 真的
/// 从 pending 变为已结算，避免测试挂住。
fn execute_pending_dispatches(
    db: &Database,
    root: &Path,
    scope: &crate::database::KernelHostScope,
    run_id: &str,
    clock: &crate::kernel::TestClock,
) -> Result<Vec<Value>, String> {
    let cancellation = CancellationRegistry::default();
    cancellation.register_run(run_id)?;
    let policy = rev_gateway_policy_existing(db, run_id, scope.clone());
    let coordinator = KernelCoordinator::reopen(db, clock, run_id, &cancellation)?;
    let mut results = Vec::new();
    for _ in 0..8 {
        let pending = coordinator
            .snapshot()?
            .pending_effects
            .iter()
            .find(|effect| {
                effect.kind == kernel::OutboxEffectKind::DispatchTool
                    && effect.status == kernel::OutboxStatus::Pending
            })
            .and_then(|effect| effect.tool_call_id.clone());
        let Some(tool_call_id) = pending else {
            break;
        };
        let policy_for_executor = rev_gateway_policy_existing(db, run_id, scope.clone());
        let mut value = None;
        let dispatched = coordinator.dispatch_tool(&tool_call_id, "f1-owner", |binding, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json)
                .map_err(|error| error.to_string())?;
            let tool = payload["tool"].as_str().unwrap_or_default().to_owned();
            let result = crate::runtime_host::kernel_host::execute_claimed_dispatch(
                db,
                binding,
                effect,
                &tool,
                &payload,
                |durable_input, claim| {
                    let input: Value =
                        serde_json::from_str(durable_input).unwrap_or(Value::Null);
                    let (result, evidence) = if matches!(tool.as_str(), "write_file" | "edit_file") {
                        managed_files::execute_admitted_file(
                            managed_files::ManagedExecutionContext {
                                database: db,
                                backups_dir: &root.join("managed-files"),
                                conversation_id: &binding.conversation_id,
                                run_id: &binding.run_id,
                                project_root: binding.permission.project_root.as_deref(),
                                permission_mode: binding.permission.mode.as_str(),
                                scope: &policy_for_executor.scope,
                                sessions_dir: None,
                            },
                            &tool,
                            &input,
                            &tool_call_id,
                            claim.credential(),
                            Some(token),
                            &mut || Ok(()),
                        )
                    } else {
                        (
                            policy_for_executor.execute_reader(db, &tool, &input, &tool_call_id, token),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    };
                    let outcome = crate::runtime_host::kernel_host::call_outcome(&result);
                    (result, evidence, outcome)
                },
            )?;
            value = Some(result);
            Ok((
                value
                    .as_ref()
                    .and_then(|value| value.get("isError").and_then(Value::as_bool))
                    != Some(true),
                value.clone().unwrap_or(Value::Null),
            ))
        })?;
        if let Some(value) = value {
            results.push(value);
        }
        if !dispatched {
            break;
        }
    }
    Ok(results)
}

/// 一个跑在真实 Kernel Run 上的 F1 夹具。
///
/// 两轮都经生产入口：
///  1. 模型提案 `read`（真实 `dispatch_initial`）→ 真实派发与真实 Kernel 读取缝结算，
///     于是 `kernel_tool_calls` 里有一条真实结果，观察也从真实读取登记；
///  2. 模型提案 `write_file`（真实 `propose_tools`）→ 真实 `GatewayPolicy::decide`。
struct F1Run {
    db: Database,
    root: PathBuf,
    run_id: String,
    conversation_id: String,
    scope: crate::database::KernelHostScope,
    clock: crate::kernel::TestClock,
    /// `read` 输入，用于按同一身份重放投影。
    read_input: Value,
    read_call: String,
    version: String,
}

/// `mode` 为会话权限模式。`body` 是写入磁盘的正文，模型候选内容由调用方给出。
fn build_f1_run(label: &str, mode: &str, body: &str, candidate: &str) -> F1Run {
    build_f1_run_with_read(label, mode, body, candidate, json!({"path":"target.txt"}))
}

/// 同上，但由调用方给出真实读取参数（例如分页读取）。
fn build_f1_run_with_read(
    label: &str,
    mode: &str,
    body: &str,
    candidate: &str,
    read_args: Value,
) -> F1Run {
    let (db, root, conversation, run, scope) =
        kernel_gateway_fixture_with_model(label, mode, &rev_consumer_model_config());
    let clock = crate::kernel::TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&db, &run, scope.clone());
    std::fs::write(root.join("target.txt"), body.as_bytes()).expect("write target");
    let read_input = read_args.clone();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).expect("real Run start");
    // 第 1 轮：模型提案读取。
    coordinator
        .dispatch_initial("owner-f1-read", &policy, move |binding, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({
                    "role":"assistant","stopReason":"toolUse","content":[
                        {"type":"toolCall","id":"read-whole","name":"read",
                         "arguments":read_args}]}),
            })
        })
        .expect("the read proposal is recorded through the real policy");
    drop(coordinator);
    // 真实派发 + 真实 Kernel 读取缝：结算出 durable 结果并登记观察。
    let read_results = execute_pending_dispatches(&db, &root, &scope, &run, &clock)
        .expect("the real dispatch seam settles the read");
    assert_eq!(read_results.len(), 1, "第 1 轮必须恰好结算一条读取");
    let version = read_results[0]["details"]["readVersion"]
        .as_str()
        .expect("the settled read carries readVersion")
        .to_owned();
    // 第 2 轮：模型提案整文件替换。
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run, &cancellation).expect("reopen");
    coordinator
        .propose_tools(
            "f1-write",
            vec![kernel::ToolCallRequest {
                tool_call_id: "write-1".into(),
                tool: "write_file".into(),
                canonical_input_json: json!({
                    "path":"target.txt","content":candidate,"expectedVersion":version
                })
                .to_string(),
                source_order: 0,
            }],
            &policy,
        )
        .expect("the write proposal is recorded through the real policy");
    drop(coordinator);
    F1Run {
        db,
        root,
        run_id: run,
        conversation_id: conversation,
        scope,
        clock,
        read_input,
        read_call: "read-whole".into(),
        version,
    }
}

fn target_disk_bytes(run: &F1Run) -> Vec<u8> {
    std::fs::read(run.root.join("target.txt")).expect("target bytes")
}

fn version_row_count(run: &F1Run) -> usize {
    version_rows(
        &run.db,
        &run.conversation_id,
        &stored_path(&run.root, "target.txt"),
    )
}

// ---------------------------------------------------------------------------
// 负例 1（修前红例）：Host 读了全文、模型没收到中间部分 → 不得整文件替换
// ---------------------------------------------------------------------------

/// F1-N1：约 30 KB ASCII、唯一标记在头尾之外。
///
/// 必须观察到：
///  * 真实读取缝交付了全文（源事实成立）；
///  * 最终模型可见的工具结果里**没有**该标记（生产投影真的裁掉了中间部分）；
///  * durable 结果与源观察都没有被改写（源快照事实保留）；
///  * 未经专门替换确认时，`write_file` 停在该确认上，目标文件与受管版本记录不变。
#[test]
fn f1_whole_read_that_the_model_never_received_cannot_replace_the_file() {
    let body = f1_big_body();
    let run = build_f1_run("n1", "allow", &body, &body);
    let observation = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    assert!(
        observation.authorizes_whole_file_replacement(),
        "源事实必须成立：真实 Host 读取确实交付了全文，否则本用例没有测到缺陷"
    );

    // 1) 最终模型可见内容：生产投影真的裁掉了中间部分。
    let view = model_visible_tool_result(
        &run.db,
        &run.run_id,
        &run.read_call,
        "read",
        &run.read_input,
    );
    let view_text = view[0]["text"].as_str().expect("a text block");
    assert!(
        !view_text.contains(F1_MARKER),
        "最终模型可见的工具结果不得包含中间标记；否则投影没有裁剪，本用例前提不成立"
    );
    assert!(
        view_text.len() < body.len(),
        "模型视图必须比源正文小（被头尾裁剪）"
    );

    // 2) 源快照与 durable 结果都没有被改写。
    assert!(
        durable_result_json(&run.db, &run.run_id, &run.read_call)
            .to_string()
            .contains(F1_MARKER),
        "durable 结果必须保留全部原始字节：源快照不因模型视图而被篡改"
    );

    // 2b) 裁剪之所以可能发生，是因为 Host 为这条结果留下了受信存储事实。
    // 两个 durable 记录形状（权威 Kernel / Legacy）都要能提供它，否则交付
    // 事实无法重放。这里把该事实显式记录下来，供审查核对。
    let storage = run
        .db
        .tool_result_storage(&run.run_id, &run.read_call)
        .expect("the Host's trusted storage fact for the settled read");
    let durable_shapes: (i64, i64) = run
        .db
        .with_connection(|c| {
            Ok((
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
                    rusqlite::params![run.run_id, run.read_call],
                    |r| r.get(0),
                )?,
                c.query_row(
                    "SELECT COUNT(*) FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",
                    rusqlite::params![run.run_id, run.read_call],
                    |r| r.get(0),
                )?,
            ))
        })
        .expect("the durable row shapes");
    assert!(
        storage.stored,
        "the delivery replay needs a verified storage fact; durable rows (kernel, legacy) = {durable_shapes:?}"
    );

    // 3) 准入：必须停在专门替换确认上，且不落盘。
    assert_eq!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "未交付全文的整文件替换必须停在专门替换确认上（{}）",
        match f1_replace_grant_state(&run.db, &run.run_id) {
            Some(state) => format!("当前专门替换授权状态={state}"),
            None => "当前没有任何专门替换授权记录".to_owned(),
        }
    );
    assert_eq!(
        f1_replace_grant_state(&run.db, &run.run_id).as_deref(),
        Some("pending"),
        "必须存在一条 pending 的专门替换请求"
    );

    // 4) 真实执行缝：没有任何可派发工作，目标文件与版本记录不变。
    let executed = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("the execution seam runs");
    assert!(
        executed.is_empty(),
        "未确认的整文件替换不得被派发执行，实际执行了 {} 条",
        executed.len()
    );
    assert_eq!(target_disk_bytes(&run), body.as_bytes(), "目标文件不得被改写");
    assert_eq!(version_row_count(&run), 0, "不得登记任何受管文件版本");
}

// ---------------------------------------------------------------------------
// 正例：小文件完整交付仍可正常替换（不得一律禁写掩盖缺口）
// ---------------------------------------------------------------------------

/// F1-P1：小文件、无分页读取 → 模型确实拿到全文 → 具备版本条件时正常替换。
#[test]
fn f1_small_file_delivered_whole_still_replaces_normally() {
    let body = "alpha\nbeta\ngamma\n".to_owned();
    let candidate = "ALPHA\nBETA\nGAMMA\n";
    let run = build_f1_run("p1", "allow", &body, candidate);
    let observation = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    assert!(observation.authorizes_whole_file_replacement());

    let view = model_visible_tool_result(
        &run.db,
        &run.run_id,
        &run.read_call,
        "read",
        &run.read_input,
    );
    assert_eq!(
        view[0]["text"].as_str().expect("text"),
        body,
        "小文件必须整段交付给模型"
    );

    assert_ne!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "完整交付的小文件替换不得被一律禁写"
    );
    let executed = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("the execution seam runs");
    assert_eq!(executed.len(), 1, "完整交付的小文件替换必须真实执行一次");
    assert_eq!(
        std::fs::read_to_string(run.root.join("target.txt")).expect("target"),
        candidate,
        "候选内容必须落盘"
    );
    assert_eq!(version_row_count(&run), 1, "恰好登记一个受管文件版本");
}

// ---------------------------------------------------------------------------
// 恢复负例：关库重开、恢复会话后仍不得免专门确认
// ---------------------------------------------------------------------------

/// F1-R1：交付缺口是 durable 事实，重开数据库后判定不变；源观察行保持原样。
#[test]
fn f1_recovery_does_not_restore_whole_file_eligibility() {
    let body = f1_big_body();
    let run = build_f1_run("r1", "allow", &body, &body);
    let before = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    let before_rows = run
        .db
        .host_observations_for_target(
            &run.run_id,
            &tool_host::canonical_file_identity(&run.root, "target.txt").expect("target"),
        )
        .expect("observations");

    // 关库重开：只剩 durable 事实。
    let F1Run {
        db,
        root,
        run_id,
        conversation_id,
        scope,
        clock,
        read_input,
        read_call,
        version,
    } = run;
    drop(db);
    let db = Database::open(root.join("facts.db")).expect("reopen");
    let reopened = F1Run {
        db,
        root,
        run_id,
        conversation_id,
        scope,
        clock,
        read_input,
        read_call,
        version,
    };

    let after = observation(
        &reopened.db,
        &reopened.run_id,
        &reopened.root,
        "target.txt",
        &reopened.version,
    );
    assert_eq!(
        before, after,
        "历史源快照事实必须逐字段不变，恢复不得改写它"
    );
    let after_rows = reopened
        .db
        .host_observations_for_target(
            &reopened.run_id,
            &tool_host::canonical_file_identity(&reopened.root, "target.txt").expect("target"),
        )
        .expect("observations");
    assert_eq!(
        before_rows.len(),
        after_rows.len(),
        "观察登记是 append-only：恢复不得新增或删除源观察行"
    );

    let view = model_visible_tool_result(
        &reopened.db,
        &reopened.run_id,
        &reopened.read_call,
        "read",
        &reopened.read_input,
    );
    assert!(
        !view[0]["text"].as_str().expect("text").contains(F1_MARKER),
        "恢复重放不得把未交付的中间内容变回模型可见"
    );
    assert_eq!(
        f1_run_state(&reopened.db, &reopened.run_id),
        "waiting_approval",
        "恢复后仍必须停在专门替换确认上"
    );
    assert_eq!(
        f1_replace_grant_state(&reopened.db, &reopened.run_id).as_deref(),
        Some("pending"),
        "恢复不得把 pending 的专门替换请求变成已授权"
    );
    assert_eq!(
        target_disk_bytes(&reopened),
        body.as_bytes(),
        "恢复不得产生任何写入"
    );
    assert_eq!(version_row_count(&reopened), 0, "恢复不得登记版本");
}

// ---------------------------------------------------------------------------
// 部分读 / Office 提取 / 引用读取：源事实本身就不够，绝不因此获得资格
// ---------------------------------------------------------------------------

/// F1-S1：分页读取只交付一段 `UnitWindow`，源事实就不是全文。
#[test]
fn f1_partial_read_observation_is_not_whole_file_source() {
    let body = f1_big_body();
    let run = build_f1_run_with_read(
        "f1-partial",
        "allow",
        &body,
        &body,
        json!({"path":"target.txt","offset":0,"limit":64}),
    );
    let read = kernel_read_result(&run.db, &run.run_id, &run.read_call);
    let delivered = read["content"][0]["text"].as_str().expect("text");
    assert_eq!(
        delivered,
        &body[..64],
        "分页读取只交付请求的那一段"
    );
    let observation = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    assert!(
        !observation.authorizes_whole_file_replacement(),
        "分页读取不得产生全文源事实（view_kind={:?}）",
        observation.view_kind
    );
    assert_eq!(observation.range_start, Some(0));
    assert_eq!(
        observation.range_end,
        Some(delivered.encode_utf16().count() as u64),
        "观察必须记录真实交付的 UTF-16 范围"
    );
    assert_eq!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "整文件替换仍必须停在专门替换确认上"
    );
    assert_eq!(
        target_disk_bytes(&run),
        body.as_bytes(),
        "未确认的整文件替换不得改写目标"
    );
}

/// F1-S2：Office 提取视图是容器的派生文本，永远不是文档源文本。
///
/// 这是**契约级**用例：观察本身由真实入口产生，这里只把同一结构中的
/// `view_kind` 换成生产枚举里的 `OfficeExtract`，确认源事实闸门不因
/// "看起来像全文"而松动；不冒充 Host 真的读过 Office 容器。
#[test]
fn f1_office_extract_view_kind_never_authorizes_replacement() {
    let (db, root, conversation, run, _scope) = kernel_gateway_fixture("f1-office", "allow");
    let target = tool_host::canonical_file_identity(&root, "book.docx").expect("target");
    let observation = HostObservation {
        target_identity: target,
        version: "v1".into(),
        observed_by_tool_call_id: "read-office".into(),
        view_kind: fox_engine_protocol::ObservationView::OfficeExtract,
        covered_whole_file: true,
        range_start: Some(0),
        range_end: Some(10),
        total_units: Some(10),
        truncated: false,
    };
    db.record_host_observation(&run, &conversation, &observation)
        .expect("record the extract observation");
    assert!(
        !observation.authorizes_whole_file_replacement(),
        "Office 提取视图永远不构成源全文"
    );
}
