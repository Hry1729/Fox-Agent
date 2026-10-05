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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

/// 一条已结算结果的 durable 行：优先取权威 Kernel 行；兼容投影可能让同一
/// Authoritative Run 同时拥有 `tool_calls` 行。Legacy 则只有后一种行。
fn durable_result_json(db: &Database, run_id: &str, call: &str) -> Value {
    let raw: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2
                 UNION ALL
                 SELECT result_json FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2
                 LIMIT 1",
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
    f1_replace_grant_state_for(db, run, "write-1")
}

fn f1_replace_grant_state_for(db: &Database, run: &str, call: &str) -> Option<String> {
    let dispatch = fox_engine_protocol::encode_dispatch_id(run, call).expect("dispatch identity");
    db.with_connection(|c| {
        c.query_row(
            "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1 AND dispatch_id=?2",
            rusqlite::params![run, dispatch],
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
    target_path: String,
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
    let run = build_f1_read_only(label, mode, body, read_args);
    propose_f1_write(&run, candidate);
    run
}

fn settle_f1_host_approval(run: &F1Run, decision: kernel::ApprovalDecision, label: &str) {
    let approval_id = run.db.kernel_approval_identity(&run.run_id, "write-1")
        .expect("projected approval identity");
    assert!(run.db.queue_kernel_host_approval(&approval_id, label)
        .expect("enqueue Host approval command"));
    let command = run.db.pending_kernel_host_commands(&run.run_id)
        .expect("pending commands").into_iter().find(|command| command.kind == "approval")
        .expect("approval command");
    let cancellation = CancellationRegistry::default();
    let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.run_id, &cancellation)
        .expect("reopen for approval");
    coordinator.resolve_approval("write-1", decision)
        .expect("durable Kernel approval transition");
    run.db.complete_kernel_host_command(&run.run_id, command.seq)
        .expect("Host acknowledgement and dedicated-grant settlement");
}

/// 把实际 Host 结算结果送入 Node 的真实 Pi adapter。子进程有独立硬截止，
/// 避免测试 runner 被 worker 挂住；这是 Legacy 模型投影证据，不借 Kernel 助手代替。
fn node_projection_view(db: &Database, run_id: &str, call: &str, result: &Value, path: &str) -> Value {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../services/agent-runtime/test/fixtures/f1-host-projection-cli.mjs");
    let storage = db.tool_result_storage(run_id, call)
        .ok().and_then(|value| serde_json::to_value(value).ok());
    let input = json!({
        "path":path, "runId":run_id, "toolCallId":call,
        "resultRef":format!("fox-result://{run_id}/{call}"),
        "result":result, "storage":storage,
    });
    let output_base = std::env::temp_dir().join(format!(
        "fox-f1-node-{}", uuid::Uuid::new_v4().simple(),
    ));
    let stdout_path = output_base.with_extension("stdout.json");
    let stderr_path = output_base.with_extension("stderr.log");
    let mut child = Command::new("node")
        .arg(&script)
        .arg("--f1-bridge")
        .stdin(Stdio::piped())
        .stdout(std::fs::File::create(&stdout_path).expect("Node stdout file"))
        .stderr(std::fs::File::create(&stderr_path).expect("Node stderr file"))
        .spawn()
        .expect("start the real Legacy adapter bridge");
    child.stdin.take().expect("stdin").write_all(input.to_string().as_bytes())
        .expect("send Host result to Node");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().expect("poll Node adapter").is_some() { break; }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Legacy adapter bridge exceeded 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = child.wait().expect("collect Node adapter status");
    let stdout = std::fs::read(&stdout_path).expect("Node output");
    let stderr = std::fs::read(&stderr_path).expect("Node errors");
    let _ = std::fs::remove_file(stdout_path);
    let _ = std::fs::remove_file(stderr_path);
    assert!(status.success(), "Node projection failed: {}", String::from_utf8_lossy(&stderr));
    serde_json::from_slice(&stdout).expect("Node projection JSON")
}

fn legacy_adapter_view(db: &Database, run_id: &str, call: &str, result: &Value) -> Value {
    node_projection_view(db, run_id, call, result, "legacy")
}

/// 停在真实 read 已结算、模型还没有提出 write 的边界，供恢复用例使用。
fn build_f1_read_only(label: &str, mode: &str, body: &str, read_args: Value) -> F1Run {
    build_f1_read_only_with_file(label, mode, "target.txt", body.as_bytes(), read_args)
}

/// 正常冻结顺序构造带真实历史消息的 Run，供生产压缩入口消费。
fn f1_long_history_fixture(
) -> (Database, PathBuf, String, String, crate::database::KernelHostScope) {
    let root = std::env::temp_dir().join(format!(
        "fox-f1-compaction-{}", uuid::Uuid::new_v4().simple(),
    ));
    std::fs::create_dir_all(&root).expect("project root");
    let db = Database::open(root.join("facts.db")).expect("database");
    let conversation = db.create_conversation(
        db.default_agent_id(), None, Some(root.to_str().expect("root path")), Some("allow"),
    ).expect("conversation");
    let run = db.create_run(&conversation.id, "F1 history compaction", None)
        .expect("run").run;
    let binding = db.freeze_kernel_run_control(
        &run.id, "legacy", fox_engine_protocol::TimeBudgets::continuous(),
    ).expect("frozen binding");
    let mut model = rev_consumer_model_config();
    model.model_service["contextWindow"] = json!(8192);
    model.model_service["maxOutputTokens"] = json!(512);
    let config_hash = model.hash().expect("model config hash");
    let frozen = crate::kernel::RunFrozenConfig {
        engine_id: "pi".into(), kernel_mode: "authoritative".into(),
        capability_manifest_version: 2, capability_manifest_hash: "f1-history-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: config_hash.clone(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0, turn_max_retries: 0,
        experimental_compute_job_notice: false,
    };
    db.kernel_create_run(
        &run.id, "pi", "authoritative", 2,
        &binding.permission_snapshot_id, &binding.execution_profile_id,
        &config_hash, &serde_json::to_string(&frozen).expect("frozen json"),
    ).expect("kernel run");
    db.freeze_kernel_model_config(&run.id, &model).expect("model config");
    let mut messages = vec![json!({"role":"user","content":"F1 fixture task"})];
    messages.extend((0..12).map(|index| json!({
        "role":if index % 2 == 0 {"user"} else {"assistant"},
        "content":format!("old-{index}: {}", "bounded earlier discussion ".repeat(38)),
    })));
    messages.extend((0..8).map(|index| json!({
        "role":"user", "content":format!("recent-{index}: preserve exactly"),
    })));
    db.freeze_kernel_initial_input(&fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1, run_id: run.id.clone(), turn_id: "turn-1".into(),
        prompt_config_hash: config_hash, messages,
    }).expect("initial history");
    let scope = kernel_gateway::freeze_scope(&db, &binding, &json!({}), &model)
        .expect("Host scope");
    db.freeze_kernel_host_scope(&run.id, &scope).expect("persist Host scope");
    db.apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
        .expect("running Run");
    (db, root, conversation.id, run.id, scope)
}

fn build_f1_read_only_with_file(
    label: &str, mode: &str, target_path: &str, body: &[u8], read_args: Value,
) -> F1Run {
    let fixture = kernel_gateway_fixture_with_model(label, mode, &rev_consumer_model_config());
    build_f1_read_only_from_fixture(fixture, target_path, body, read_args)
}

fn build_f1_read_only_from_fixture(
    fixture: (Database, PathBuf, String, String, crate::database::KernelHostScope),
    target_path: &str, body: &[u8], read_args: Value,
) -> F1Run {
    let (db, root, conversation, run, scope) = fixture;
    let clock = crate::kernel::TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&db, &run, scope.clone());
    std::fs::write(root.join(target_path), body).expect("write target");
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
        target_path: target_path.into(),
    }
}

fn propose_f1_write(run: &F1Run, candidate: &str) {
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&run.db, &run.run_id, run.scope.clone());
    // 真实恢复/续答后的首次提案也走同一生产策略入口。
    let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.run_id, &cancellation)
        .expect("reopen for write proposal");
    coordinator
        .propose_tools(
            "f1-write",
            vec![kernel::ToolCallRequest {
                tool_call_id: "write-1".into(),
                tool: "write_file".into(),
                canonical_input_json: json!({
                    "path":run.target_path,"content":candidate,"expectedVersion":run.version
                })
                .to_string(),
                source_order: 0,
            }],
            &policy,
        )
        .expect("the write proposal is recorded through the real policy");
}

fn propose_f1_continued_read(run: &F1Run, call: &str, input: Value) -> Value {
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&run.db, &run.run_id, run.scope.clone());
    let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.run_id, &cancellation)
        .expect("reopen for continued read");
    coordinator.propose_tools(
        "f1-continued-read",
        vec![kernel::ToolCallRequest {
            tool_call_id: call.into(), tool: "read".into(),
            canonical_input_json: input.to_string(), source_order: 0,
        }], &policy,
    ).expect("second read proposal");
    let results = execute_pending_dispatches(
        &run.db, &run.root, &run.scope, &run.run_id, &run.clock,
    ).expect("settle continued read");
    assert_eq!(results.len(), 1);
    results.into_iter().next().expect("read result")
}

fn target_disk_bytes(run: &F1Run) -> Vec<u8> {
    std::fs::read(run.root.join(&run.target_path)).expect("target bytes")
}

fn version_row_count(run: &F1Run) -> usize {
    version_rows(
        &run.db,
        &run.conversation_id,
        &stored_path(&run.root, &run.target_path),
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
    let run = build_f1_read_only("r1", "allow", &body, json!({"path":"target.txt"}));
    assert_ne!(f1_run_state(&run.db, &run.run_id), "waiting_approval",
        "重开前不得已有写提案或待确认请求");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id), None);
    let before = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    let before_rows = run
        .db
        .host_observations_for_target(
            &run.run_id,
            &tool_host::canonical_file_identity(&run.root, "target.txt").expect("target"),
        )
        .expect("observations");

    // 首次 write 提案之前关库重开：只剩 durable 读取和源观察事实。
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
        target_path,
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
        target_path,
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
    assert_eq!(f1_replace_grant_state(&reopened.db, &reopened.run_id), None,
        "恢复时仍不得有预先提出的确认请求");
    propose_f1_write(&reopened, &body);
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

/// F1-S2：实际 xlsx 容器由 Host 读取并提取文本，提取视图不能授权替换容器。
#[test]
fn f1_office_extract_view_kind_never_authorizes_replacement() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/office-reading.xlsx");
    let bytes = std::fs::read(fixture).expect("real xlsx fixture");
    let run = build_f1_read_only_with_file(
        "f1-office", "allow", "book.xlsx", &bytes, json!({"path":"book.xlsx"}),
    );
    let read = kernel_read_result(&run.db, &run.run_id, &run.read_call);
    assert!(read["content"][0]["text"].as_str().is_some_and(|text| !text.is_empty()),
        "真实 Office 读取必须交付提取文本");
    let observation = observation(&run.db, &run.run_id, &run.root, "book.xlsx", &run.version);
    assert_eq!(observation.view_kind, fox_engine_protocol::ObservationView::OfficeExtract);
    assert!(
        !observation.authorizes_whole_file_replacement(),
        "Office 提取视图永远不构成源全文"
    );
    assert!(run.db.needs_replace_grant(&run.run_id, &observation)
        .expect("F1 delivery admission"));
    propose_f1_write(&run, "malicious container replacement");
    let write = kernel_read_result(&run.db, &run.run_id, "write-1");
    let (input, state): (String, String) = run.db.with_connection(|c| c.query_row(
        "SELECT canonical_input_json,state FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='write-1'",
        [&run.run_id], |row| Ok((row.get(0)?, row.get(1)?)),
    )).expect("write proposal row");
    assert_eq!(serde_json::from_str::<Value>(&input).expect("write input")["path"], "book.xlsx",
        "写提案与 Office 读取必须指向同一目标");
    assert_eq!(state, "failed", "二进制 Office 容器先由文本写入校验拒绝");
    assert_eq!(write["details"]["errorCode"], "tool.invalid_input");
    assert!(write["content"][0]["text"].as_str().is_some_and(|text|
        text.contains("not valid UTF-8 text")));
    assert!(execute_pending_dispatches(&run.db, &run.root, &run.scope, &run.run_id, &run.clock)
        .expect("Host execution seam").is_empty());
    assert_eq!(target_disk_bytes(&run), bytes);
    assert_eq!(version_row_count(&run), 0);
}

#[test]
fn f1_two_pages_of_the_same_snapshot_still_require_a_dedicated_confirmation() {
    let body = format!("{}{}", "A".repeat(64), "B".repeat(64));
    let run = build_f1_read_only(
        "same-snapshot-pages", "allow", &body,
        json!({"path":"target.txt","offset":0,"limit":64}),
    );
    let next = propose_f1_continued_read(
        &run, "read-next", json!({"path":"target.txt","offset":64,"limit":64}),
    );
    assert_eq!(next["details"]["readVersion"], run.version);
    let second = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    assert!(!second.authorizes_whole_file_replacement());
    propose_f1_write(&run, "combined replacement");
    assert_eq!(f1_run_state(&run.db, &run.run_id), "waiting_approval");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("pending"));
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
}

#[test]
fn f1_pages_from_different_versions_cannot_form_a_whole_file_read() {
    let first = format!("{}{}", "A".repeat(64), "B".repeat(64));
    let mut run = build_f1_read_only(
        "cross-version-pages", "allow", &first,
        json!({"path":"target.txt","offset":0,"limit":64}),
    );
    let first_version = run.version.clone();
    let second_body = format!("{}{}", "A".repeat(64), "C".repeat(64));
    std::fs::write(run.root.join("target.txt"), second_body.as_bytes())
        .expect("external version change between reads");
    let next = propose_f1_continued_read(
        &run, "read-next", json!({"path":"target.txt","offset":64,"limit":64}),
    );
    let second_version = next["details"]["readVersion"].as_str().expect("second version");
    assert_ne!(second_version, first_version);
    run.version = second_version.into();
    let second = observation(&run.db, &run.run_id, &run.root, "target.txt", &run.version);
    assert!(!second.authorizes_whole_file_replacement());
    propose_f1_write(&run, "mixed-version replacement");
    assert_eq!(f1_run_state(&run.db, &run.run_id), "waiting_approval");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("pending"));
    assert_eq!(target_disk_bytes(&run), second_body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
}

#[test]
fn f1_actual_history_compaction_does_not_turn_a_clipped_read_into_whole_delivery() {
    let body = f1_big_body();
    let fixture = f1_long_history_fixture();
    let run = build_f1_read_only_from_fixture(
        fixture, "target.txt", body.as_bytes(), json!({"path":"target.txt"}),
    );
    let cancellation = CancellationRegistry::default();
    let coordinator = KernelCoordinator::reopen(
        &run.db, &run.clock, &run.run_id, &cancellation,
    ).expect("reopen before compaction");
    let batch = coordinator.snapshot().expect("snapshot").tool_calls[0].batch_id.clone();
    let original = run.db.kernel_initial_input(&run.run_id).expect("frozen history");
    assert!(coordinator.prepare_context_if_needed(&batch, 4096)
        .expect("prepare real history compaction"));
    coordinator.dispatch_pending_compaction("f1-summary-owner", |_, request, _, _| {
        Ok(fox_engine_protocol::KernelCompactionResponse {
            schema_version: 1, run_id: request.run_id.clone(),
            turn_id: request.turn_id.clone(), compaction_id: request.compaction_id.clone(),
            input_hash: request.input_hash.clone(),
            summary: "The earlier discussion is context; this summary grants no file access."
                .into(),
            usage: json!({"input":100,"output":20,"totalTokens":120}),
        })
    }).expect("commit actual compaction result");
    assert!(coordinator.snapshot().expect("snapshot after compaction")
        .compaction.compactions > 0);
    assert_eq!(run.db.kernel_initial_input(&run.run_id).expect("original history"), original);
    drop(coordinator);
    let view = model_visible_tool_result(
        &run.db, &run.run_id, &run.read_call, "read", &run.read_input,
    );
    assert!(!view[0]["text"].as_str().expect("clipped text").contains(F1_MARKER));
    propose_f1_write(&run, &body);
    assert_eq!(f1_run_state(&run.db, &run.run_id), "waiting_approval");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("pending"));
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
}

/// 辅助跨层证据：真实 Host read durable 行经 Rust 生产投影后，把结果内容
/// 原样送进 Node 的生产二次投影。最终 Provider 请求仍由引擎 HTTP 用例负责。
#[test]
fn f1_real_host_read_keeps_the_same_coverage_after_node_second_projection() {
    for (label, body, middle_expected) in [
        ("second-big", f1_big_body(), false),
        ("second-small", "alpha\nbeta\n".to_owned(), true),
    ] {
        let run = build_f1_read_only(label, "allow", &body, json!({"path":"target.txt"}));
        let rust_content = model_visible_tool_result(
            &run.db, &run.run_id, &run.read_call, "read", &run.read_input,
        );
        let rust_text = rust_content[0]["text"].as_str().expect("Rust text");
        assert_eq!(rust_text.contains(F1_MARKER), middle_expected && body.contains(F1_MARKER));
        let node = node_projection_view(
            &run.db, &run.run_id, &run.read_call,
            &json!({"content":rust_content}), "kernel",
        );
        let node_text = node["content"][0]["text"].as_str().expect("Node text");
        assert_eq!(node_text, rust_text,
            "Node 二次投影不得扩大或继续丢失 Rust 交付的正文");
        assert_eq!(node_text.contains(F1_MARKER), middle_expected && body.contains(F1_MARKER));
        assert_eq!(target_disk_bytes(&run), body.as_bytes());
        assert_eq!(version_row_count(&run), 0);
    }
}

/// 当前裁剪读取引出的专门批准只对本次候选有效；实际 Host 写入消耗一次。
#[test]
fn f1_cropped_read_confirmation_is_bound_to_one_candidate_and_consumed_once() {
    let body = f1_big_body();
    let candidate = "approved replacement";
    let run = build_f1_run("confirm-once", "allow", &body, candidate);
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("pending"));
    settle_f1_host_approval(&run, kernel::ApprovalDecision::AllowOnce, "allow_once");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("approved"));

    // 同一份读，另一候选即使使用当前版本也必须重新取得专门确认。
    let changed = json!({
        "path":"target.txt", "content":"different candidate", "expectedVersion":run.version,
    });
    let changed_credential = run.db.issue_dispatch_credential(
        &run.run_id, "write-changed", "write_file", &changed.to_string(),
    ).expect("issue changed candidate").expect("file credential");
    assert!(changed_credential.requires_replace_grant);
    let refused = run.db.claim_execution_attempt(
        &run.run_id, &run.conversation_id, &changed_credential,
        &changed_credential.intent_digest, "changed-owner",
    ).expect("claim changed candidate");
    assert!(matches!(refused, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
        "另一候选不能借用 write-1 的专门确认：{refused:?}");
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);

    let executed = execute_pending_dispatches(
        &run.db, &run.root, &run.scope, &run.run_id, &run.clock,
    ).expect("admitted Host dispatch");
    assert_eq!(executed.len(), 1);
    assert_eq!(target_disk_bytes(&run), candidate.as_bytes());
    assert_eq!(version_row_count(&run), 1);
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("consumed"));
    assert!(execute_pending_dispatches(
        &run.db, &run.root, &run.scope, &run.run_id, &run.clock,
    ).expect("repeated Host drive").is_empty());
    assert_eq!(version_row_count(&run), 1);
}

#[test]
fn f1_cropped_read_denial_cannot_write_or_revive_the_request() {
    let body = f1_big_body();
    let run = build_f1_run("deny-cropped", "allow", &body, "denied candidate");
    settle_f1_host_approval(&run, kernel::ApprovalDecision::Deny, "denied");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("expired"));
    assert!(execute_pending_dispatches(
        &run.db, &run.root, &run.scope, &run.run_id, &run.clock,
    ).expect("denied Host drive").is_empty());
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
    assert!(run.db.approve_whole_file_replacement(
        &run.run_id, "write-1", &run.conversation_id,
        &tool_host::canonical_file_identity(&run.root, "target.txt").expect("target"),
        &run.version, "wrong candidate", "wrong request",
    ).is_err(), "terminal denial cannot be revived");
}

#[test]
fn f1_cropped_read_expired_confirmation_cannot_write() {
    let body = f1_big_body();
    let run = build_f1_run("expire-cropped", "allow", &body, "late candidate");
    let approval_id = run.db.kernel_approval_identity(&run.run_id, "write-1")
        .expect("approval identity");
    assert!(run.db.queue_kernel_host_approval(&approval_id, "allow_once")
        .expect("queue before expiration"));
    let command = run.db.pending_kernel_host_commands(&run.run_id)
        .expect("pending commands").into_iter().find(|command| command.kind == "approval")
        .expect("approval command");
    run.clock.advance(86_000_000);
    let cancellation = CancellationRegistry::default();
    let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.run_id, &cancellation)
        .expect("reopen after deadline");
    coordinator.tick().expect("expire the durable approval");
    run.db.complete_kernel_host_command(&run.run_id, command.seq)
        .expect("acknowledge the expired command without granting");
    assert_eq!(f1_replace_grant_state(&run.db, &run.run_id).as_deref(), Some("expired"));
    assert!(execute_pending_dispatches(
        &run.db, &run.root, &run.scope, &run.run_id, &run.clock,
    ).expect("expired Host drive").is_empty());
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
}

// ---------------------------------------------------------------------------
// Legacy 可达入口：同一交付规则必须拦住 Legacy 发证路径
// ---------------------------------------------------------------------------

/// 经**Legacy 生产入口**读一次：真实 `tool_calls` 行 + 真实观察登记。
fn legacy_durable_read(
    db: &Database,
    binding: &fox_engine_protocol::RunControlBinding,
    input: &Value,
    call: &str,
) -> Value {
    let registry = CancellationRegistry::default();
    registry.register_run(&binding.run_id).expect("register run");
    let token = registry
        .tool_token(&binding.run_id, call)
        .expect("tool token");
    crate::runtime_host::create_fresh_host_tool_call(
        db,
        &binding.run_id,
        call,
        "read",
        input,
        "running",
        false,
    )
    .expect("a fresh Host read ToolCall");
    let outcome = managed_files::execute_observed_reader(
        db,
        binding,
        "read",
        input,
        call,
        &token,
        std::time::Duration::from_millis(600_000),
    );
    let envelope = crate::runtime_host::finalize_host_tool_execution(
        db,
        &binding.run_id,
        call,
        "read",
        input,
        Vec::new(),
        outcome,
    )
    .expect("finalize the Host read");
    envelope.get("result").cloned().unwrap_or(envelope)
}

/// Legacy（非权威 Kernel）入口仍然可达，因此必须有独立用例证明同一交付规则
/// 在它的发证路径上生效——不能拿 Authoritative 的结果代替。
///
/// 这里走 `Database::issue_legacy_file_credential`，也就是桌面 Host 写路径
/// （`runtime_host::execute_host_tool_call`）用的那条发证入口：
///  * 30 KB 全文读取：Legacy Pi adapter 原样交付，但当前统一保守资格规则
///    仍要求专门确认（当前重算按 Kernel 投影上界）；
///    并留下 pending 的专门替换请求；
///  * 小文件全文交付 → 不额外要求专门确认，避免"一律禁写"。
#[test]
fn f1_legacy_entry_conservatively_requires_replacement_grant_for_large_read() {
    // --- 大文件：真实 Host 读到全文，Legacy adapter 实际保留全文；准入保守。
    let (db, root, conversation, run) = rev_fixture("f1-legacy-big", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    let body = f1_big_body();
    std::fs::write(root.join("big.txt"), body.as_bytes()).expect("write");
    let read_input = json!({"path":"big.txt"});
    let read = legacy_durable_read(&db, &binding, &read_input, "read-big");
    let version = read_version(&read);
    let observation = observation(&db, &run, &root, "big.txt", &version);
    assert!(
        observation.authorizes_whole_file_replacement(),
        "源事实必须成立：Legacy 读取确实交付了全文"
    );
    let view = legacy_adapter_view(&db, &run, "read-big", &read);
    assert!(
        view["content"][0]["text"].as_str().expect("text").contains(F1_MARKER),
        "真实 Legacy adapter 的 Pi 结果分支保留完整 30 KB 正文"
    );

    let policy_version = db
        .execution_policy(&conversation)
        .expect("execution policy")
        .version;
    let write_input = json!({"path":"big.txt","content":body,"expectedVersion":version});
    crate::runtime_host::create_fresh_host_tool_call(
        &db,
        &run,
        "write-1",
        "write_file",
        &write_input,
        "running",
        false,
    )
    .expect("a fresh Host write ToolCall");
    let credential = db
        .issue_legacy_file_credential(&run, "write-1", policy_version)
        .expect("the Legacy issuance entry");
    assert!(
        credential.requires_replace_grant,
        "当前统一重算规则对 Legacy 30 KB 正文保守要求专门确认"
    );
    assert!(credential.replace_candidate_digest.is_some());
    assert!(credential.replace_request_digest.is_some());
    assert_eq!(
        f1_replace_grant_state(&db, &run).as_deref(),
        Some("pending"),
        "Legacy 发证只能提出请求，不能自行授权"
    );
    assert_eq!(
        std::fs::read(root.join("big.txt")).expect("target"),
        body.as_bytes(),
        "未确认的整文件替换不得改写目标"
    );

    // --- 小文件：完整交付，不得被一律禁写 ---
    let (db, root, conversation, run) = rev_fixture("f1-legacy-small", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    let small = "alpha\nbeta\ngamma\n";
    std::fs::write(root.join("small.txt"), small.as_bytes()).expect("write");
    let read_input = json!({"path":"small.txt"});
    let read = legacy_durable_read(&db, &binding, &read_input, "read-small");
    let view = legacy_adapter_view(&db, &run, "read-small", &read);
    assert_eq!(view["content"][0]["text"], small);
    let version = read_version(&read);
    let policy_version = db
        .execution_policy(&conversation)
        .expect("execution policy")
        .version;
    let write_input = json!({"path":"small.txt","content":"ALPHA\nBETA\nGAMMA\n","expectedVersion":version});
    crate::runtime_host::create_fresh_host_tool_call(
        &db,
        &run,
        "write-1",
        "write_file",
        &write_input,
        "running",
        false,
    )
    .expect("a fresh Host write ToolCall");
    let credential = db
        .issue_legacy_file_credential(&run, "write-1", policy_version)
        .expect("the Legacy issuance entry");
    assert!(
        !credential.requires_replace_grant,
        "完整交付的小文件不得被一律禁写"
    );
    assert_eq!(
        f1_replace_grant_state(&db, &run),
        None,
        "不需要专门替换确认时不得留下请求记录"
    );
}

// ---------------------------------------------------------------------------
// 项5：自动模式与写文件授权
//
// 被锁定的规则（全部走真实入口：真实 Database、真实冻结绑定、真实
// `KernelCoordinator` 提案、真实 GatewayPolicy 决策、真实 Host 准入与提交缝）：
//  * 新建（Host 自己证明目标不存在）不是整文件替换：自动模式不逐文件打断，
//    需要审批时给出"仅本次 / 本次对话 / 拒绝"，且不绑定一次性替换凭据；
//  * 已有内容的整份覆盖仍然保留一次性专门替换授权（REV-05 不放宽）；
//  * 会话级授权只覆盖它被批准的那一个确切目标，且撤销后立即失效；
//  * 写入与执行分别判定：不可命名的执行范围永远拿不到会话级授权。
// ---------------------------------------------------------------------------

/// 冻结并真实启动一个 Run，`first_calls` 作为模型首轮提案。
fn build_item5_with_first_calls(
    label: &str,
    mode: &str,
    first_calls: Vec<(&str, &str, Value)>,
) -> F1Run {
    let (db, root, conversation, run, scope) =
        kernel_gateway_fixture_with_model(label, mode, &rev_consumer_model_config());
    let clock = crate::kernel::TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&db, &run, scope.clone());
    let content: Vec<Value> = first_calls
        .iter()
        .map(|(call, tool, arguments)| {
            json!({"type":"toolCall","id":call,"name":tool,"arguments":arguments})
        })
        .collect();
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation)
        .expect("real Run start");
    coordinator
        .dispatch_initial("owner-item5", &policy, move |binding, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({
                    "role":"assistant","stopReason":"toolUse","content":content}),
            })
        })
        .expect("the first proposal is recorded through the real policy");
    drop(coordinator);
    F1Run {
        db,
        root,
        run_id: run,
        conversation_id: conversation,
        scope,
        clock,
        read_input: Value::Null,
        read_call: String::new(),
        version: "missing".into(),
        target_path: String::new(),
    }
}

/// 在已启动的 Run 上再走一轮真实提案。
fn item5_propose(run: &F1Run, call: &str, tool: &str, input: Value) -> Result<(), String> {
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&run.db, &run.run_id, run.scope.clone());
    let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.run_id, &cancellation)
        .map_err(|error| error.to_string())?;
    coordinator
        .propose_tools(
            // A batch identity is one durable turn: reusing it across proposals is
            // refused as a conflict, so it is keyed by the call it carries.
            &format!("item5-propose-{call}"),
            vec![kernel::ToolCallRequest {
                tool_call_id: call.into(),
                tool: tool.into(),
                canonical_input_json: input.to_string(),
                source_order: 0,
            }],
            &policy,
        )
        .map_err(|error| error.to_string())
}

/// 该调用当前悬而未决的审批卡（真实投影行）。
fn item5_approval_request(run: &F1Run, call: &str) -> Value {
    let raw: String = run
        .db
        .with_connection(|c| {
            c.query_row(
                "SELECT a.request_json FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                  WHERE t.run_id=?1 AND t.runtime_tool_call_id=?2 AND a.status='pending'",
                rusqlite::params![run.run_id, call],
                |r| r.get(0),
            )
        })
        .expect("a pending approval card for this call");
    serde_json::from_str(&raw).expect("approval request json")
}

fn item5_decisions(request: &Value) -> Vec<String> {
    request
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn item5_replace_grant_count(run: &F1Run) -> i64 {
    run.db
        .with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run.run_id],
                |r| r.get(0),
            )
        })
        .expect("replacement grant count")
}

/// 自动模式的核心验收：连续新建多份 Markdown + 更新进度 CSV 都不逐文件打断。
#[test]
fn item5_auto_mode_creates_reports_and_updates_the_progress_csv_without_per_file_prompts() {
    let run = build_item5_with_first_calls(
        "item5-auto",
        "allow",
        vec![
            (
                "report-a",
                "write_file",
                json!({"path":"report-a.md","content":"# Report A\n"}),
            ),
            (
                "report-b",
                "write_file",
                json!({"path":"report-b.md","content":"# Report B\n"}),
            ),
            (
                "progress",
                "write_file",
                json!({"path":"progress.csv","content":"step,state\n1,done\n"}),
            ),
        ],
    );
    let results = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("auto-mode dispatch seam");
    assert_eq!(
        results.len(),
        3,
        "自动模式下同轮的三次新建必须全部直接执行，不得逐文件打断：{results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(run.root.join("report-a.md")).expect("report a"),
        "# Report A\n"
    );
    assert_eq!(
        std::fs::read_to_string(run.root.join("report-b.md")).expect("report b"),
        "# Report B\n"
    );
    assert_eq!(
        std::fs::read_to_string(run.root.join("progress.csv")).expect("progress csv"),
        "step,state\n1,done\n"
    );
    assert_eq!(
        item5_replace_grant_count(&run),
        0,
        "新建不是整文件替换：不得签发一次性替换凭据"
    );
    assert_ne!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "自动模式的新建不得停在审批上"
    );

    // 第二轮：读回自己写的 CSV，再整份覆盖为进度更新（源事实与模型交付都覆盖全文）。
    item5_propose(&run, "read-csv", "read", json!({"path":"progress.csv"}))
        .expect("read the report back");
    let read = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("read the report back");
    assert_eq!(read.len(), 1);
    let version = read_version(&read[0]);
    item5_propose(
        &run,
        "update-csv",
        "write_file",
        json!({
            "path":"progress.csv","content":"step,state\n1,done\n2,done\n",
            "expectedVersion":version,
        }),
    )
    .expect("progress update proposal");
    assert_ne!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "已完整读回的进度更新不得再弹窗"
    );
    let updated = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("progress update dispatch");
    assert_eq!(updated.len(), 1);
    assert_eq!(
        std::fs::read_to_string(run.root.join("progress.csv")).expect("updated csv"),
        "step,state\n1,done\n2,done\n"
    );
}

/// 需要审批的新建：给出三个选项、不绑定替换凭据，且越界目标不进入审批面。
#[test]
fn item5_ask_mode_offers_the_conversation_decision_for_a_new_file_and_never_leaves_the_directory() {
    let run = build_item5_with_first_calls(
        "item5-ask",
        "ask",
        vec![(
            "write-1",
            "write_file",
            json!({"path":"notes.md","content":"hello\n"}),
        )],
    );
    assert_eq!(f1_run_state(&run.db, &run.run_id), "waiting_approval");
    assert_eq!(
        item5_replace_grant_count(&run),
        0,
        "新建不得预先提出一次性替换请求"
    );
    let request = item5_approval_request(&run, "write-1");
    assert!(
        request.get("wholeFileReplacement").is_none(),
        "新建不是整文件替换：不得绑定一次性替换凭据（这正是'目标不存在也只给仅本次'的成因）"
    );
    let decisions = item5_decisions(&request);
    assert!(decisions.iter().any(|d| d == "allow_once"), "{decisions:?}");
    assert!(
        decisions.iter().any(|d| d == "allow_conversation"),
        "普通写入必须提供会话级授权，且后端确实接受它：{decisions:?}"
    );
    assert!(decisions.iter().any(|d| d == "deny"), "{decisions:?}");
    assert!(
        std::fs::symlink_metadata(run.root.join("notes.md")).is_err(),
        "未批准的写入不得落盘"
    );

    // 越界路径不是"可被授权的操作范围"：不产生审批卡，也不产生文件。
    let escaped = run
        .root
        .parent()
        .expect("project parent")
        .join("item5-escape.md");
    let _ = item5_propose(
        &run,
        "write-escape",
        "write_file",
        json!({"path":"../item5-escape.md","content":"x"}),
    );
    assert!(
        std::fs::symlink_metadata(&escaped).is_err(),
        "越界写入不得落到授权目录之外"
    );
    let escape_card: Option<String> = run
        .db
        .with_connection(|c| {
            c.query_row(
                "SELECT a.id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                  WHERE t.run_id=?1 AND t.runtime_tool_call_id='write-escape'",
                [&run.run_id],
                |r| r.get(0),
            )
            .optional()
        })
        .expect("escape approval lookup");
    assert!(
        escape_card.is_none(),
        "越界目标不得成为可被授权的操作（审批不能授予目录之外的权限）"
    );
}

/// 自动模式不放宽破坏性覆盖：已有内容的整份覆盖仍保留一次性专门替换授权。
#[test]
fn item5_auto_mode_still_requires_the_one_dispatch_ticket_to_overwrite_existing_content() {
    let body = f1_big_body();
    let run = build_f1_run("item5-overwrite", "allow", &body, &body);
    assert_eq!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "自动模式不得凭权限模式放行对已有内容的整份覆盖"
    );
    assert_eq!(
        f1_replace_grant_state(&run.db, &run.run_id).as_deref(),
        Some("pending")
    );
    let request = item5_approval_request(&run, "write-1");
    assert!(
        request.get("wholeFileReplacement").is_some(),
        "覆盖已有内容必须保留一次性的专门替换授权"
    );
    assert_eq!(
        item5_decisions(&request),
        vec!["allow_once".to_owned(), "deny".to_owned()],
        "一次性替换凭据不得声明会话级授权"
    );
    assert_eq!(target_disk_bytes(&run), body.as_bytes());
    assert_eq!(version_row_count(&run), 0);
}

/// 会话级授权：只覆盖被批准的那一个目标，别的会话拿不到，撤销后立即失效。
#[test]
fn item5_session_authorization_is_limited_to_its_exact_target_and_dies_on_revocation() {
    let run = build_item5_with_first_calls(
        "item5-scope",
        "ask",
        vec![("read-1", "read", json!({"path":"target.txt"}))],
    );
    std::fs::write(run.root.join("target.txt"), "alpha beta\n").expect("target file");
    let read = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("initial read");
    assert_eq!(read.len(), 1);
    let version = read_version(&read[0]);

    // 一次局部修改 → 用户选择"本次对话内允许"。
    item5_propose(
        &run,
        "write-1",
        "edit_file",
        json!({
            "path":"target.txt","oldText":"alpha","newText":"ALPHA",
            "expectedVersion":version,
        }),
    )
    .expect("first precise edit proposal");
    assert_eq!(f1_run_state(&run.db, &run.run_id), "waiting_approval");
    let decisions = item5_decisions(&item5_approval_request(&run, "write-1"));
    assert!(
        decisions.iter().any(|d| d == "allow_conversation"),
        "局部修改必须能选择会话级授权：{decisions:?}"
    );
    settle_f1_host_approval(
        &run,
        kernel::ApprovalDecision::AllowConversation,
        "allow_conversation",
    );
    // The Host registers the reusable grant inside `consume_approval_step`
    // (`register_approval_grant`), which this fixture's settle helper does not
    // drive; the full drive-loop registration and reuse is covered by
    // `F3Case::ConversationGrantReuse`. Registering here through the same Host API
    // with the same exact scope key keeps this case about the POLICY effect: a
    // live grant covers one named target and nothing else.
    let scope = crate::runtime_host::shadow_reconcile::tool_operation_scope(
        "edit_file",
        &json!({"path":"target.txt"}),
        Some(run.root.to_str().expect("project root")),
    )
    .expect("the edit target must be nameable, or no grant could ever cover it");
    assert!(
        matches!(
            run.db
                .kernel_register_authorization_grant(
                    &run.run_id,
                    "write-1",
                    "allow_conversation",
                    "edit_file",
                    Some(&scope),
                )
                .expect("register the conversation authorization"),
            crate::database::GrantRegistration::Registered { .. }
        ),
        "a real conversation decision on a nameable scope must register a grant"
    );
    assert_eq!(
        run.db
            .kernel_effective_authorization_grants(&run.run_id, crate::database::now_ms())
            .expect("grants")
            .len(),
        1
    );
    assert_eq!(
        execute_pending_dispatches(&run.db, &run.root, &run.scope, &run.run_id, &run.clock)
            .expect("approved edit")
            .len(),
        1
    );

    // 同一目标上的同类修改：授权覆盖，不再弹窗。
    item5_propose(&run, "read-2", "read", json!({"path":"target.txt"})).expect("re-read");
    let read = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("re-read dispatch");
    let version = read_version(&read[0]);
    item5_propose(
        &run,
        "write-2",
        "edit_file",
        json!({
            "path":"target.txt","oldText":"beta","newText":"BETA",
            "expectedVersion":version,
        }),
    )
    .expect("second precise edit proposal");
    assert_ne!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "同一目标上的同类写入已被会话授权覆盖，不得再次弹窗"
    );
    assert_eq!(
        execute_pending_dispatches(&run.db, &run.root, &run.scope, &run.run_id, &run.clock)
            .expect("granted edit")
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(run.root.join("target.txt")).expect("edited"),
        "ALPHA BETA\n"
    );

    // 会话切换不串权：另一个会话即使同一个项目也没有这条授权。
    let other = run
        .db
        .create_conversation(
            run.db.default_agent_id(),
            None,
            Some(run.root.to_str().expect("root")),
            Some("ask"),
        )
        .expect("second conversation");
    let other_run = run
        .db
        .create_run(&other.id, "item5 other conversation", None)
        .expect("run")
        .run
        .id;
    assert!(
        run.db
            .kernel_effective_authorization_grants(&other_run, crate::database::now_ms())
            .expect("grants")
            .is_empty(),
        "另一个会话不得继承本会话的写入授权"
    );

    // 撤销后旧授权不得再生效。
    run.db
        .kernel_revoke_authorization_grants(&run.conversation_id, "user withdrew")
        .expect("revoke the session authorization");
    item5_propose(&run, "read-3", "read", json!({"path":"target.txt"})).expect("third read");
    let read = execute_pending_dispatches(
        &run.db,
        &run.root,
        &run.scope,
        &run.run_id,
        &run.clock,
    )
    .expect("third read dispatch");
    let version = read_version(&read[0]);
    item5_propose(
        &run,
        "write-3",
        "edit_file",
        json!({
            "path":"target.txt","oldText":"ALPHA","newText":"alpha",
            "expectedVersion":version,
        }),
    )
    .expect("post-revocation edit proposal");
    assert_eq!(
        f1_run_state(&run.db, &run.run_id),
        "waiting_approval",
        "撤销后同一范围的写入必须重新审批"
    );
    assert_eq!(
        std::fs::read_to_string(run.root.join("target.txt")).expect("unchanged"),
        "ALPHA BETA\n",
        "撤销后未批准的写入不得落盘"
    );
}

