//! "Tool errors are data, not errors" (fix round, 2026-10-20).
//!
//! The diagnosis round reproduced two Windows failures: a work tool refused a
//! model's arguments (`plan_revision_create` with `tasks` as a JSON string;
//! `task_evidence_add` citing an id that does not resolve) and the Kernel path
//! turned that refusal into a fatal executor error — the call stayed leased,
//! the Run failed with `kernel.execution_failed`, and the call was later shown
//! as `cancelled`.
//!
//! Now a refusal the work tool proves happened before any write
//! (`WorkToolErrorPhase::Rejected`) is settled as an ordinary failed tool
//! result carrying its code and details, exactly like the legacy path, and the
//! Run continues. Failures after a write began, storage failures and anything
//! unclassified stay fatal. A bounded number of consecutive refusals ends the
//! Run with `kernel.tool_error_budget_exhausted`, read back from durable
//! results so it survives a restart.

use super::*;
use crate::database::{CreateGoalInput, Database, GoalStatus};
use crate::runtime_host::work_tools::execute;
use serde_json::json;

fn work_fixture() -> (Database, std::path::PathBuf, String, String, String) {
    let path = std::env::temp_dir().join(format!("fox-work-diag-{}.db", uuid::Uuid::new_v4()));
    let database = Database::open(path.clone()).expect("open database");
    let conversation = database
        .create_conversation(database.default_agent_id(), Some("work diag"), None, None)
        .expect("create conversation");
    let run = database
        .create_run(&conversation.id, "计算 17*23+5，并给出结果", None)
        .expect("create run")
        .run;
    let goal = database
        .goals()
        .create(CreateGoalInput {
            id: None,
            conversation_id: conversation.id.clone(),
            title: "算术".to_owned(),
            objective: "计算 17*23+5".to_owned(),
            acceptance_summary: None,
            status: GoalStatus::Active,
            created_by: run.id.clone(),
        })
        .expect("create active goal");
    (database, path, conversation.id, run.id, goal.id)
}

/// Run 1 of the Windows report, at the work tool boundary: the refusal keeps
/// its code and is typed as a pre-write rejection; nothing is written.
#[test]
fn plan_revision_tasks_sent_as_a_json_string_is_a_typed_pre_write_rejection() {
    let (database, path, conversation_id, run_id, goal_id) = work_fixture();
    let as_string = json!({
        "goalId": goal_id, "title": "算术计划", "summary": "一步计算",
        "tasks": "[{\"title\":\"计算 17*23+5\",\"ordinal\":0}]"
    });
    let error = execute(&database, &conversation_id, &run_id, "plan_revision_create", &as_string)
        .expect_err("a JSON-string task list is still refused by the Host");
    assert_eq!(error.code, "plan.tasks_required");
    assert!(error.is_rejected_before_execution(), "refused before any write");
    let (plans, _, _) = database.load_a1_snapshot(&conversation_id).unwrap();
    assert!(plans.is_empty(), "no PlanRevision was persisted");
    // The Kernel result keeps what `to_string()` used to drop.
    let result = super::super::super::kernel_gateway::work_tool_rejection_result(
        "plan_revision_create", &error, 1);
    assert_eq!(result["isError"], true);
    assert_eq!(result["details"]["errorCode"], "plan.tasks_required");
    assert_eq!(result["details"]["error"]["code"], "plan.tasks_required");
    assert_eq!(result["details"]["error"]["details"], error.details);
    assert_eq!(result["details"]["executionStarted"], false);
    assert!(result["content"][0]["text"].as_str().unwrap().contains("[plan.tasks_required]"));
    let shape = super::super::super::kernel_gateway::input_field_types(&as_string);
    assert!(shape.contains("tasks:string(json-array?)"), "{shape}");

    let as_array = json!({
        "goalId": goal_id, "title": "算术计划", "summary": "一步计算",
        "tasks": [{"title":"计算 17*23+5","ordinal":0}]
    });
    execute(&database, &conversation_id, &run_id, "plan_revision_create", &as_array)
        .expect("the array form is accepted");
    drop(database);
    let _ = std::fs::remove_file(path);
}

/// Run 2 of the Windows report: unresolvable evidence references are refused
/// inside the evidence write transaction, which rolls back, so they are data.
/// (Storage failures stay uncertain: `work_tools::phase_tests` and
/// `kernel_work_tool_failure_inside_the_write_stays_fatal`.)
#[test]
fn task_evidence_add_unresolvable_references_are_typed_rejections() {
    let (database, path, conversation_id, run_id, goal_id) = work_fixture();
    let created = execute(&database, &conversation_id, &run_id, "task_create_many",
        &json!({"goalId": goal_id, "tasks": [{"title": "计算", "ordinal": 0}]}))
        .expect("create task");
    let task_id = created.result["details"]["tasks"][0]["id"].as_str().unwrap().to_owned();
    execute(&database, &conversation_id, &run_id, "task_update",
        &json!({"taskId": task_id, "status": "in_progress", "expectedVersion": 1}))
        .expect("start task");

    let cases = [
        (json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
            "refId": "call_function_0_1", "summary": "已调用工具"}), "was not found"),
        (json!({"taskId": task_id, "evidenceType": "user_confirmation", "refKind": "message",
            "refId": "msg-1", "summary": "用户确认"}), "was not found"),
        (json!({"taskId": task_id, "evidenceType": "test_result", "refKind": "message",
            "refId": "msg-1", "summary": "计算结果 396"}), "cannot reference"),
        (json!({"taskId": task_id, "evidenceType": "no_such_type", "refKind": "message",
            "refId": "msg-1", "summary": "x"}), "unsupported evidenceType"),
        (json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
            "summary": "missing refId"}), "refId is required"),
    ];
    for (input, expected) in cases {
        let error = execute(&database, &conversation_id, &run_id, "task_evidence_add", &input)
            .expect_err("unresolvable evidence is refused");
        assert_eq!(error.code, "work_tool.failed", "legacy code unchanged: {input}");
        assert!(error.message.contains(expected), "{input}: {}", error.message);
        assert!(error.is_rejected_before_execution(), "{input}");
    }
    let count = |db: &Database| db.with_connection(|c| c.query_row(
        "SELECT COUNT(*) FROM task_evidence", [], |row| row.get::<_, i64>(0))).unwrap();
    assert_eq!(count(&database), 0, "no evidence row was written");

    drop(database);
    let _ = std::fs::remove_file(path);
}

fn plan_args(goal_id: &str, tasks: serde_json::Value) -> serde_json::Value {
    json!({"goalId": goal_id, "title": "算术计划", "summary": "一步计算", "tasks": tasks})
}

/// A Kernel Run with the plan tool in its frozen scope and an active Goal.
fn kernel_work_fixture(clock: &TestClock) -> (Database, PathBuf, String, String, super::super::super::kernel_gateway::GatewayPolicy) {
    kernel_work_fixture_with(clock, &["plan_revision_create"])
}

/// As [`kernel_work_fixture`], with the given work tools in the frozen scope.
fn kernel_work_fixture_with(clock: &TestClock, tools: &[&str]) -> (Database, PathBuf, String, String, super::super::super::kernel_gateway::GatewayPolicy) {
    let mut config = worker_configuration();
    config.proposal_tools = tools.iter().map(|tool| json!({"name":tool,"description":"Host work tool",
        "parameters":{"type":"object","properties":{}}})).collect();
    let (db, root, run_id) = fixture_with_start_opt(clock, &config.hash().unwrap(), Some(&config), true, true);
    let scope = crate::database::KernelHostScope { schema_version: 1,
        tool_names: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks: Vec::new() };
    db.freeze_kernel_host_scope(&run_id, &scope).unwrap();
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    let goal = db.goals().create(CreateGoalInput {
        id: None, conversation_id: binding.conversation_id.clone(), title: "算术".to_owned(),
        objective: "计算 17*23+5".to_owned(), acceptance_summary: None, status: GoalStatus::Active,
        created_by: run_id.clone(),
    }).unwrap();
    let policy = super::super::super::kernel_gateway::GatewayPolicy { binding, scope, database: None, sessions_dir: None, artifacts_dir: None };
    (db, root, run_id, goal.id, policy)
}

fn plan_calls(calls: &[(&str, serde_json::Value)]) -> serde_json::Value {
    json!({"role":"assistant","stopReason":"toolUse","content": calls.iter().map(|(id, args)|
        json!({"type":"toolCall","id":id,"name":"plan_revision_create","arguments":args})).collect::<Vec<_>>()})
}

fn no_leased_effect(snapshot: &crate::kernel::KernelSnapshot) -> bool {
    snapshot.pending_effects.iter().all(|effect| effect.status != kernel::OutboxStatus::Leased)
}

/// Correctable refusal → data → the model corrects itself → success; a success
/// resets the streak; a reopen never re-dispatches a settled call.
#[test]
fn kernel_work_tool_refusal_is_data_the_run_continues_and_the_corrected_call_succeeds() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id, goal_id, policy) = kernel_work_fixture(&clock);
    let conversation_id = policy.binding.conversation_id.clone();
    let bad = plan_args(&goal_id, json!("[{"));
    let good = plan_args(&goal_id, json!([{"title":"计算 17*23+5","ordinal":0}]));
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("plan-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: plan_calls(&[("plan-bad", bad.clone())]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("plan-bad", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "plan-bad", "plan_revision_create", &bad, token)?;
        assert_eq!(result["isError"], true);
        assert_eq!(result["details"]["error"]["code"], "plan.tasks_required");
        assert_eq!(result["details"]["toolErrorPhase"], "rejected_before_execution");
        assert_eq!(result["details"]["executionStarted"], false);
        assert_eq!(result["details"]["consecutiveRejections"], 1);
        Ok((result["isError"] != true, result))
    }).unwrap());
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "running", "the refusal does not end the Run");
    assert_eq!(snapshot.tool_calls[0].state, "failed", "the call is settled, not left leased");
    assert!(no_leased_effect(&snapshot));
    let stored: serde_json::Value = serde_json::from_str(snapshot.tool_calls[0].result_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored["details"]["errorCode"], "plan.tasks_required", "the code is durable");
    assert!(db.load_a1_snapshot(&conversation_id).unwrap().0.is_empty());

    let batch_id = snapshot.tool_calls[0].batch_id.clone();
    coordinator.dispatch_batch(&batch_id, "plan-model-2", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: plan_calls(&[("plan-good", good.clone())]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("plan-good", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "plan-good", "plan_revision_create", &good, token)?;
        assert_ne!(result["isError"], true);
        Ok((true, result))
    }).unwrap());
    let snapshot = coordinator.snapshot().unwrap();
    let good_call = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "plan-good").unwrap();
    assert_eq!(good_call.state, "completed");
    assert_eq!(db.load_a1_snapshot(&conversation_id).unwrap().0.len(), 1, "the corrected plan exists");

    // A success resets the consecutive count.
    let batch_id = good_call.batch_id.clone();
    coordinator.dispatch_batch(&batch_id, "plan-model-3", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: plan_calls(&[("plan-bad-2", bad.clone())]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("plan-bad-2", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "plan-bad-2", "plan_revision_create", &bad, token)?;
        assert_eq!(result["details"]["consecutiveRejections"], 1, "reset by the success");
        Ok((false, result))
    }).unwrap());

    // Recovery: settled calls are never re-executed after a reopen.
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    for id in ["plan-bad", "plan-good", "plan-bad-2"] {
        assert!(!reopened.dispatch_tool(id, "recovered-owner", |_, _, _| panic!("{id} must not replay")).unwrap());
    }
    assert_eq!(reopened.snapshot().unwrap().state, "running");
    assert_eq!(db.load_a1_snapshot(&conversation_id).unwrap().0.len(), 1);
}

/// The streak bound: five refusals are data, the sixth settles the typed
/// budget result (also after a reopen), and the drive loop then fails the Run
/// with the specific code before any model round or resource execution.
#[test]
fn kernel_work_tool_refusal_streak_is_bounded_and_survives_reopen() {
    let limit = super::super::super::kernel_gateway::WORK_TOOL_REJECTION_STREAK_LIMIT;
    assert_eq!(limit, 5);
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id, goal_id, policy) = kernel_work_fixture(&clock);
    let conversation_id = policy.binding.conversation_id.clone();
    let bad = plan_args(&goal_id, json!("[{\"title\":\"x\""));
    let ids = (0..=limit).map(|i| format!("plan-{i}")).collect::<Vec<_>>();
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("plan-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: plan_calls(&ids.iter().map(|id| (id.as_str(), bad.clone())).collect::<Vec<_>>()),
    })).unwrap();
    for (position, id) in ids.iter().take(limit).enumerate() {
        coordinator.dispatch_tool(id, "work-owner", |_, _, token| {
            let result = policy.execute_work(&db, id, "plan_revision_create", &bad, token)?;
            assert_eq!(result["details"]["error"]["code"], "plan.tasks_required");
            assert_eq!(result["details"]["consecutiveRejections"], position + 1);
            Ok((false, result))
        }).unwrap();
        assert!(!super::super::super::kernel_gateway::tool_error_budget_exhausted(&coordinator.snapshot().unwrap()));
    }
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let last = ids.last().unwrap();
    assert!(reopened.dispatch_tool(last, "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, last, "plan_revision_create", &bad, token)?;
        assert_eq!(result["details"]["error"]["code"], "kernel.tool_error_budget_exhausted");
        assert_eq!(result["details"]["error"]["lastRefusalCode"], "plan.tasks_required");
        assert_eq!(result["details"]["retryable"], false);
        assert_eq!(result["details"]["executionStarted"], false);
        Ok((false, result))
    }).unwrap());
    let snapshot = reopened.snapshot().unwrap();
    assert!(super::super::super::kernel_gateway::tool_error_budget_exhausted(&snapshot));
    assert!(no_leased_effect(&snapshot));
    drop(reopened);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    super::super::super::kernel_host::drive(super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &cancellation, &run_id, &real_worker_command(), "test-key", &Allow,
        |_, _, _| panic!("an exhausted refusal budget must not execute another resource")).unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "failed");
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let error: String = conn.query_row("SELECT error_code FROM runs WHERE id=?1", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(error, "kernel.tool_error_budget_exhausted");
    assert!(db.load_a1_snapshot(&conversation_id).unwrap().0.is_empty(), "no refused call wrote anything");
}

/// A failure inside the write (here: storage) is NOT a refusal: the executor
/// error stays fatal and the call stays leased (uncertain), never retried.
#[test]
fn kernel_work_tool_failure_inside_the_write_stays_fatal() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id, goal_id, policy) = kernel_work_fixture(&clock);
    let good = plan_args(&goal_id, json!([{"title":"计算 17*23+5","ordinal":0}]));
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("plan-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: plan_calls(&[("plan-write", good.clone())]),
    })).unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_plan_write BEFORE INSERT ON plan_revisions BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
    let result = coordinator.dispatch_tool("plan-write", "work-owner", |_, _, token| {
        policy.execute_work(&db, "plan-write", "plan_revision_create", &good, token).map(|result| (true, result))
    });
    assert!(result.is_err(), "a failure inside the write must not become a correctable result");
    let snapshot = coordinator.snapshot().unwrap();
    assert!(!no_leased_effect(&snapshot), "the uncertain call stays leased and is not replayed");
    assert!(!matches!(snapshot.tool_calls[0].state.as_str(), "completed" | "failed"));
    assert_eq!(snapshot.state, "running");
}

// ---------------------------------------------------------------------------
// Evidence scheme 3 (2026-10-20): the model can obtain a citable evidence id.
//
// A successful Kernel work result carries `details.evidenceRef` (the call's
// projected `kernel-tool:{run}:{call}` id) plus one trailing text line, and
// `task_evidence_add` also accepts the provider call id, resolving it inside
// the Task's conversation and storing the canonical id.
// ---------------------------------------------------------------------------

fn work_calls(calls: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
    json!({"role":"assistant","stopReason":"toolUse","content": calls.iter().map(|(id, name, args)|
        json!({"type":"toolCall","id":id,"name":name,"arguments":args})).collect::<Vec<_>>()})
}

fn stored_result(snapshot: &crate::kernel::KernelSnapshot, id: &str) -> serde_json::Value {
    let call = snapshot.tool_calls.iter().find(|call| call.tool_call_id == id).unwrap();
    serde_json::from_str(call.result_json.as_deref().unwrap()).unwrap()
}

fn evidence_rows(db: &Database, task_id: &str) -> Vec<(String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT ref_kind, ref_id FROM task_evidence WHERE task_id = ?1 ORDER BY rowid")?;
        let rows = statement.query_map([task_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }).unwrap()
}

/// The task an in-progress text-only Run works on (in a real Run created by
/// the earlier planning tools).
fn in_progress_task(db: &Database, conversation_id: &str, run_id: &str, goal_id: &str) -> String {
    let created = execute(db, conversation_id, run_id, "task_create_many",
        &json!({"goalId": goal_id, "tasks": [{"title": "计算 17*23+5", "ordinal": 0}]}))
        .expect("create task");
    let task_id = created.result["details"]["tasks"][0]["id"].as_str().unwrap().to_owned();
    execute(db, conversation_id, run_id, "task_update",
        &json!({"taskId": task_id, "status": "in_progress", "expectedVersion": 1}))
        .expect("start task");
    task_id
}

/// End to end on the real coordinator: the evidence id returned by a
/// successful work call is accepted the FIRST time; citing the same call again
/// by its provider id is the same Evidence (DuplicateEvidence, as data with
/// guidance); nothing replays after a reopen.
#[test]
fn kernel_text_only_task_cites_the_returned_evidence_ref_and_succeeds_first_time() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id, goal_id, policy) =
        kernel_work_fixture_with(&clock, &["work_snapshot_get", "task_evidence_add"]);
    let conversation_id = policy.binding.conversation_id.clone();
    let task_id = in_progress_task(&db, &conversation_id, &run_id, &goal_id);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("snap-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: work_calls(&[("snap-1", "work_snapshot_get", json!({}))]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("snap-1", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "snap-1", "work_snapshot_get", &json!({}), token)?;
        Ok((result["isError"] != true, result))
    }).unwrap());

    // The successful result names its own citable id, structured and in text.
    let canonical = format!("kernel-tool:{run_id}:snap-1");
    let snapshot = coordinator.snapshot().unwrap();
    let snap = stored_result(&snapshot, "snap-1");
    assert_eq!(snap["details"]["evidenceRef"],
        json!({"evidenceType": "tool_call", "refKind": "tool_call", "refId": canonical}));
    let blocks = snap["content"].as_array().unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(blocks[0]["text"].as_str().unwrap()).is_ok(),
        "the JSON body block is unchanged");
    assert!(blocks.iter().any(|block| block["text"].as_str()
        .is_some_and(|text| text.contains(&format!("refId={canonical}")))), "{snap}");
    // ...and it is the id of the projected read model row.
    let projected: (String, String) = db.with_connection(|connection| connection.query_row(
        "SELECT runtime_tool_call_id, status FROM tool_calls WHERE id = ?1", [&canonical],
        |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert_eq!(projected, ("snap-1".to_owned(), "completed".to_owned()));

    // First evidence attempt with the returned id succeeds.
    let by_canonical = json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
        "refId": canonical, "summary": "17*23+5 = 396"});
    let batch_id = snapshot.tool_calls[0].batch_id.clone();
    coordinator.dispatch_batch(&batch_id, "evidence-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: work_calls(&[("ev-1", "task_evidence_add", by_canonical.clone())]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("ev-1", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "ev-1", "task_evidence_add", &by_canonical, token)?;
        assert_ne!(result["isError"], true, "{result}");
        assert_eq!(result["details"]["evidence"]["refId"], canonical);
        assert_eq!(result["details"]["evidenceRef"]["refId"], format!("kernel-tool:{run_id}:ev-1"));
        Ok((true, result))
    }).unwrap());
    let snapshot = coordinator.snapshot().unwrap();
    let ev1 = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "ev-1").unwrap();
    assert_eq!(ev1.state, "completed", "no refusal, no wasted correction round");
    assert_eq!(evidence_rows(&db, &task_id), vec![("tool_call".to_owned(), canonical.clone())]);

    // The same call cited by its provider id is the same Evidence.
    let by_provider = json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
        "refId": "snap-1", "summary": "17*23+5 = 396"});
    let batch_id = ev1.batch_id.clone();
    coordinator.dispatch_batch(&batch_id, "evidence-model-2", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: work_calls(&[("ev-2", "task_evidence_add", by_provider.clone())]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("ev-2", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "ev-2", "task_evidence_add", &by_provider, token)?;
        assert_eq!(result["isError"], true);
        assert!(result["details"]["error"]["message"].as_str().unwrap().contains("duplicate task evidence"), "{result}");
        assert_eq!(result["details"]["consecutiveRejections"], 1);
        assert!(result["details"]["evidenceRef"].is_null(), "a refused call is not citable");
        let guidance = &result["details"]["error"]["details"]["evidenceGuidance"];
        assert_eq!(guidance["allowedPairs"]["tool_call"], json!(["tool_call"]));
        assert_eq!(guidance["allowedPairs"]["user_confirmation"], json!(["message"]));
        assert!(result["content"][0]["text"].as_str().unwrap().contains("allowedPairs"));
        Ok((false, result))
    }).unwrap());
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "running");
    assert!(no_leased_effect(&snapshot));
    assert_eq!(evidence_rows(&db, &task_id).len(), 1, "deduplicated by the canonical id");

    // Recovery never re-executes a settled call.
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    for id in ["snap-1", "ev-1", "ev-2"] {
        assert!(!reopened.dispatch_tool(id, "recovered-owner", |_, _, _| panic!("{id} must not replay")).unwrap());
    }
    assert_eq!(evidence_rows(&db, &task_id).len(), 1);
}

/// The provider id path: resolved to and stored as the canonical projected id
/// (so citing the canonical id afterwards is a duplicate), and never resolved
/// across conversations.
#[test]
fn kernel_provider_tool_call_id_resolves_to_the_projected_id_within_the_conversation_only() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id, goal_id, policy) =
        kernel_work_fixture_with(&clock, &["work_snapshot_get", "task_evidence_add"]);
    let conversation_id = policy.binding.conversation_id.clone();
    let task_id = in_progress_task(&db, &conversation_id, &run_id, &goal_id);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("snap-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: work_calls(&[("call_function_0_1", "work_snapshot_get", json!({}))]),
    })).unwrap();
    assert!(coordinator.dispatch_tool("call_function_0_1", "work-owner", |_, _, token| {
        let result = policy.execute_work(&db, "call_function_0_1", "work_snapshot_get", &json!({}), token)?;
        Ok((true, result))
    }).unwrap());
    let canonical = format!("kernel-tool:{run_id}:call_function_0_1");

    let cite = |ref_id: &str| json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
        "refId": ref_id, "summary": "17*23+5 = 396"});
    let stored = execute(&db, &conversation_id, &run_id, "task_evidence_add", &cite("call_function_0_1"))
        .expect("the provider id resolves");
    assert_eq!(stored.result["details"]["evidence"]["refId"], canonical, "stored canonically");
    assert_eq!(evidence_rows(&db, &task_id), vec![("tool_call".to_owned(), canonical.clone())]);
    let duplicate = execute(&db, &conversation_id, &run_id, "task_evidence_add", &cite(&canonical))
        .expect_err("the canonical id names the same Evidence");
    assert!(duplicate.message.contains("duplicate task evidence"), "{}", duplicate.message);
    assert!(duplicate.is_rejected_before_execution());

    // Another conversation cannot reach this call by its provider id.
    let other = db.create_conversation(db.default_agent_id(), Some("other"), None, None).unwrap();
    let other_run = db.create_run(&other.id, "另一个对话", None).unwrap().run;
    let other_goal = db.goals().create(CreateGoalInput {
        id: None, conversation_id: other.id.clone(), title: "其他".to_owned(),
        objective: "其他".to_owned(), acceptance_summary: None, status: GoalStatus::Active,
        created_by: other_run.id.clone(),
    }).unwrap();
    let other_task = in_progress_task(&db, &other.id, &other_run.id, &other_goal.id);
    let foreign = execute(&db, &other.id, &other_run.id, "task_evidence_add", &json!({
        "taskId": other_task, "evidenceType": "tool_call", "refKind": "tool_call",
        "refId": "call_function_0_1", "summary": "x"}))
        .expect_err("a provider id never resolves across conversations");
    assert!(foreign.message.contains("was not found"), "{}", foreign.message);
    assert!(foreign.is_rejected_before_execution());
    assert!(evidence_rows(&db, &other_task).is_empty());
    assert_eq!(evidence_rows(&db, &task_id).len(), 1);
    drop(coordinator);
}
