//! Reproduction (diagnosis round, 2026-10-20): Kernel work tools fail on the
//! Host side for inputs a real OpenAI-compatible model sends, and the failure
//! is a *hard* `WorkToolError`, not a tool result the model could correct.
//!
//! On the authoritative Kernel path every `Err` from a work tool is propagated
//! as a fatal executor error (`kernel_host.rs`, the work-tool arm of the drive
//! closure): the call stays leased, the drive ends, the Run fails with the
//! generic `kernel.execution_failed`, and `RunController::terminate` marks the
//! still-running call `cancelled`. These tests pin the first half of that chain
//! — which inputs fail and with which code — and that the code is lost by the
//! `to_string()` the Kernel gateway applies. They do not change any behavior.

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

/// Run 1 of the Windows report: `plan_revision_create` with `tasks` sent as a
/// JSON *string*. The legacy Node path repairs this in `prepareArguments`
/// (`preparePlanRevisionArguments`); the Kernel live loop installs permissive
/// replay tools without that preparer, so the Host receives the string as is.
#[test]
fn plan_revision_tasks_sent_as_a_json_string_is_a_hard_work_tool_error() {
    let (database, path, conversation_id, run_id, goal_id) = work_fixture();
    let as_string = json!({
        "goalId": goal_id, "title": "算术计划", "summary": "一步计算",
        "tasks": "[{\"title\":\"计算 17*23+5\",\"ordinal\":0}]"
    });
    let error = execute(&database, &conversation_id, &run_id, "plan_revision_create", &as_string)
        .expect_err("a JSON-string task list is rejected by the Host");
    assert_eq!(error.code, "plan.tasks_required");
    // What the Kernel gateway keeps (`error.to_string()`) carries neither the
    // code nor the field: the persisted attempt code and the run.failed event
    // can therefore only say `kernel.execution_failed`.
    let kept = error.to_string();
    assert!(!kept.contains("plan.tasks_required"), "{kept}");
    assert!(!kept.contains("string"), "{kept}");
    assert_eq!(
        crate::tool_host::ToolErrorCode::classify(&kept),
        None,
        "the kept text is unclassified, so the Run fails with the generic code"
    );
    // The new diagnostic names the offending field and its wire type.
    let shape = super::super::super::kernel_gateway::input_field_types(&as_string);
    assert!(shape.contains("tasks:string(json-array?)"), "{shape}");

    // The very same plan as a real array is accepted: only the field type differs.
    let as_array = json!({
        "goalId": goal_id, "title": "算术计划", "summary": "一步计算",
        "tasks": [{"title":"计算 17*23+5","ordinal":0}]
    });
    execute(&database, &conversation_id, &run_id, "plan_revision_create", &as_array)
        .expect("the array form is accepted");
    drop(database);
    let _ = std::fs::remove_file(path);
}

/// Run 2 of the Windows report: `task_evidence_add` after goal/task/attempt
/// setup. For a pure text task the only things a model can cite are its own
/// tool calls or messages, by the ids it knows. Kernel tool calls are projected
/// into `tool_calls` as `kernel-tool:{run}:{call}` (kernel_projection.rs), so a
/// raw provider call id never resolves; message evidence must name a *user*
/// message id the model never sees. Each of these is a hard error.
#[test]
fn task_evidence_add_inputs_a_text_only_run_can_produce_are_hard_errors() {
    let (database, path, conversation_id, run_id, goal_id) = work_fixture();
    let created = execute(&database, &conversation_id, &run_id, "task_create_many",
        &json!({"goalId": goal_id, "tasks": [{"title": "计算", "ordinal": 0}]}))
        .expect("create task");
    let task_id = created.result["details"]["tasks"][0]["id"].as_str().unwrap().to_owned();
    execute(&database, &conversation_id, &run_id, "task_update",
        &json!({"taskId": task_id, "status": "in_progress", "expectedVersion": 1}))
        .expect("start task");

    let cases = [
        // The model cites its own (provider) tool-call id.
        (json!({"taskId": task_id, "evidenceType": "tool_call", "refKind": "tool_call",
            "refId": "call_function_0_1", "summary": "已调用工具"}), "was not found"),
        // The model cites "the conversation" with an id it made up.
        (json!({"taskId": task_id, "evidenceType": "user_confirmation", "refKind": "message",
            "refId": "msg-1", "summary": "用户确认"}), "was not found"),
        // A pairing outside the Host's evidence matrix.
        (json!({"taskId": task_id, "evidenceType": "test_result", "refKind": "message",
            "refId": "msg-1", "summary": "计算结果 396"}), "cannot reference"),
    ];
    for (input, expected) in cases {
        let error = execute(&database, &conversation_id, &run_id, "task_evidence_add", &input)
            .expect_err("unresolvable evidence is rejected");
        // Repository errors arrive as `String` and become the generic code.
        assert_eq!(error.code, "work_tool.failed", "{input}");
        assert!(error.message.contains(expected), "{input}: {}", error.message);
    }
    drop(database);
    let _ = std::fs::remove_file(path);
}
