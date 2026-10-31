use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding, TimeBudgets};

fn fixture(route: Option<ExecutorRoute>) -> (Database, String, String) {
    let db = Database::open(":memory:".into()).unwrap();
    let conversation = db.create_conversation("fox-general", None, None, Some("ask")).unwrap();
    let run = db.create_run(&conversation.id, "native route fixture", None).unwrap().run.id;
    let permission = FrozenPermission { mode:PermissionMode::Ask,project_root:None,grants:Vec::new(),approval_epoch:None };
    db.freeze_run_control(&RunControlBinding {
        schema_version:1,run_id:run.clone(),conversation_id:conversation.id.clone(),engine_id:"pi".into(),
        execution_profile_id:"legacy".into(),authority:ExecutionAuthority::Authoritative,read_only_executor:ResourceExecutor::Rust,
        permission_snapshot_id:Database::run_control_permission_hash(&permission).unwrap(),permission,
        budgets:TimeBudgets::default(),executor_route:route,
    }).unwrap();
    db.with_connection(|connection| connection.execute("UPDATE runs SET status='running',started_at=?2 WHERE id=?1",params![run,now_ms()])).unwrap();
    (db,run,conversation.id)
}

fn status(db: &Database, run: &str) -> String {
    db.with_connection(|connection| connection.query_row("SELECT status FROM runs WHERE id=?1",[run],|row|row.get(0))).unwrap()
}

fn input() -> Value { json!({"path":"proof.txt"}) }

fn ledger(db: &Database, run: &str, state: &str, result: Option<&Value>) {
    db.with_connection(|connection| {
        let batch = format!("batch:{run}");
        connection.execute("INSERT INTO kernel_tool_batches(batch_id,run_id,ordered_tool_call_ids_json,created_at)
            VALUES(?1,?2,'[\"call\"]',1)",params![batch,run])?;
        connection.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,
            state,result_json,created_at,settled_at,context_generation,input_digest,claimed_at)
            VALUES(?1,'call',?2,'read',0,?3,?4,?5,1,?6,1,?7,?8)",
            params![run,batch,input().canonical_string(),state,result.map(|value|value.canonical_string()),
                result.map(|_|2),format!("sha256:{}","a".repeat(64)),(state=="running").then_some(1)])?;
        Ok(())
    }).unwrap();
}

#[test]
fn native_host_writers_and_display_events_preserve_failed_and_cancelled_facts() {
    for state in ["failed","cancelled"] {
        let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
        ledger(&db,&run,"running",None);
        assert!(db.apply_runtime_event(&run,1,&json!({"type":"tool.started","toolCallId":"call","tool":"read","input":input()})).unwrap());
        let record = db.create_host_tool_call(&run,"call","read",&input(),"running",false).unwrap();
        assert_eq!(record.execution_location,"host");
        db.complete_host_tool_call(&run,"call",Some(&json!({"proof":"host"})),Some("Host refusal")).unwrap();
        let body = json!({"isError":true,"result":{"proof":"host"},"error":"Host refusal"});
        db.with_connection(|connection| connection.execute("UPDATE kernel_tool_calls SET state=?2,result_json=?3,settled_at=2 WHERE run_id=?1",
            params![run,state,body.canonical_string()])).unwrap();
        for (seq,event) in [(2,"tool.updated"),(3,"tool.completed")] {
            assert!(db.apply_runtime_event(&run,seq,&json!({"type":event,"toolCallId":"call","tool":"read","isError":false,
                "result":{"proof":"sidecar"},"update":{"proof":"sidecar"}})).unwrap());
        }
        let shown = db.get_runtime_tool_call(&run,"call").unwrap().unwrap();
        assert_eq!(shown.status,state);
        assert_eq!(shown.result,Some(json!({"proof":"host"})));
        let stored: (String,String) = db.with_connection(|connection| connection.query_row(
            "SELECT state,result_json FROM kernel_tool_calls WHERE run_id=?1",[&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(stored,(state.into(),body.canonical_string()));
        let trace_status: String = db.with_connection(|connection| connection.query_row(
            "SELECT status FROM trace_spans WHERE run_id=?1 AND category='tool'",[&run],|row|row.get(0))).unwrap();
        assert_eq!(trace_status,"error");
        assert!(db.apply_runtime_event(&run,4,&json!({"type":"tool.completed","toolCallId":"foreign","tool":"read","isError":false})).is_err());
    }
}

#[test]
fn native_completion_cannot_skip_unsettled_or_unknown_tool_outcomes() {
    for state in ["pending","waiting_approval","running","completed"] {
        let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
        ledger(&db,&run,state,None);
        assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err(),"{state}");
        assert_eq!(status(&db,&run),"running");
        assert_eq!(db.next_run_seq(&run).unwrap(),1);
    }
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    ledger(&db,&run,"completed",Some(&json!({"isError":false,"result":{"proof":"committed"}})));
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).unwrap());
    assert_eq!(status(&db,&run),"completed");
    assert_eq!(db.get_runtime_tool_call(&run,"call").unwrap().unwrap().result,Some(json!({"proof":"committed"})));
}

#[test]
fn native_started_and_updated_before_host_claim_cannot_create_or_modify_tool_calls() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    ledger(&db,&run,"pending",None);
    let sidecar_event = |event| json!({"type":event,"toolCallId":"call","tool":"read",
        "input":{"path":"sidecar-forged.txt"},"update":{"content":"sidecar progress"},
        "result":{"proof":"sidecar"},"isError":false});
    for (seq,event) in [(1,"tool.started"),(2,"tool.updated")] {
        assert!(db.apply_runtime_event(&run,seq,&sidecar_event(event)).unwrap());
        assert!(db.get_runtime_tool_call(&run,"call").unwrap().is_none(),"{event}");
    }
    let facts: (String,String,Option<String>,Option<i64>) = db.with_connection(|connection| connection.query_row(
        "SELECT state,canonical_input_json,result_json,claimed_at FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='call'",
        [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap();
    assert_eq!(facts,("pending".into(),input().canonical_string(),None,None));

    let (host,disposition)=db.create_host_tool_call_once(&run,"call","read",&input(),"pending",true).unwrap();
    assert_eq!(disposition,super::super::HostToolCallDisposition::Created);
    let approval=db.create_approval(&host.id,"read",&json!({"input":input()})).unwrap();
    db.resolve_approval(&approval.id,super::super::ApprovalDecision::AllowOnce).unwrap();
    let before=crate::canonical_json::to_string(&db.get_runtime_tool_call(&run,"call").unwrap().unwrap()).unwrap();
    for (seq,event) in [(3,"tool.updated"),(4,"tool.started")] {
        assert!(db.apply_runtime_event(&run,seq,&sidecar_event(event)).unwrap());
        let after=crate::canonical_json::to_string(&db.get_runtime_tool_call(&run,"call").unwrap().unwrap()).unwrap();
        assert_eq!(after,before,"{event} changed the pending Host ToolCall");
    }
    assert!(db.claim_approved_tool_call(&run,"call").unwrap());
    assert!(!db.claim_approved_tool_call(&run,"call").unwrap());
    let claimed=db.get_runtime_tool_call(&run,"call").unwrap().unwrap();
    assert_eq!(claimed.status,"running");
    assert_eq!(claimed.execution_location,"host");
    assert_eq!(claimed.input,input());
    assert!(claimed.result.is_none());
}

#[test]
fn native_lifecycle_completion_before_host_commit_cannot_settle_the_tool() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    ledger(&db,&run,"pending",None);
    db.apply_runtime_event(&run,1,&json!({"type":"tool.started","toolCallId":"call","tool":"read","input":input()})).unwrap();
    db.apply_runtime_event(&run,2,&json!({"type":"tool.completed","toolCallId":"call","tool":"read","isError":false,"result":"uncommitted"})).unwrap();
    assert!(db.get_runtime_tool_call(&run,"call").unwrap().is_none());
    let ended: Option<i64> = db.with_connection(|connection| connection.query_row(
        "SELECT ended_at FROM trace_spans WHERE run_id=?1 AND category='tool'",[&run],|row|row.get(0))).unwrap();
    assert!(ended.is_none());
    assert!(db.apply_runtime_event(&run,3,&json!({"type":"run.completed"})).is_err());
    let failure = json!({"isError":true,"error":"tool was not started"});
    db.with_connection(|connection| connection.execute("UPDATE kernel_tool_calls SET state='not_started',result_json=?2,settled_at=2 WHERE run_id=?1",
        params![run,failure.canonical_string()])).unwrap();
    db.apply_runtime_event(&run,3,&json!({"type":"run.completed"})).unwrap();
    assert_eq!(db.get_runtime_tool_call(&run,"call").unwrap().unwrap().status,"failed");
}

#[test]
fn native_ledger_projection_retains_the_full_large_host_result() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    let result = json!({"payload":"测试".repeat(80_000)});
    ledger(&db,&run,"completed",Some(&json!({"isError":false,"result":result})));
    db.apply_runtime_event(&run,1,&json!({"type":"tool.completed","toolCallId":"call","tool":"read","isError":false,"result":"preview"})).unwrap();
    let stored: String = db.with_connection(|connection| connection.query_row(
        "SELECT b.body_json FROM tool_calls t JOIN tool_call_result_blobs b ON b.sha256=t.result_blob_sha256
         WHERE t.run_id=?1 AND t.runtime_tool_call_id='call'",[&run],|row|row.get(0))).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(),result);
}

#[test]
fn native_completion_requires_settled_host_business_state() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    // Historical delivery checklist rows have no production writer since the
    // typed delivery lane was removed; completion is gated on live Host state.
    let child_conversation=db.create_conversation("fox-general",Some("isolated child"),None,None).unwrap().id;
    let child = db.create_run(&child_conversation,"active child",None).unwrap().run.id;
    db.with_connection(|connection| connection.execute("UPDATE runs SET parent_run_id=?2 WHERE id=?1",params![child,run])).unwrap();
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err());
    db.with_connection(|connection| connection.execute("UPDATE runs SET status='cancelled' WHERE id=?1",[&child])).unwrap();
    let job=db.kernel_job_start(&super::super::JobStartRequest { run_id:run.clone(),kind:"attachment_compute".into(),
        idempotency_key:"native-job".into(),params:json!({}),deadline_ms:None,progress_total:None }).unwrap().snapshot().clone();
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err());
    db.kernel_job_cancel(&job.job_id).unwrap();
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).unwrap());
}

#[test]
fn native_restart_preserves_unclaimed_host_approvals_and_their_native_facts() {
    for decision in ["pending","approved","denied"] {
        let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
        ledger(&db,&run,"running",None);
        let call = db.create_host_tool_call(&run,"call","read",&input(),"pending",true).unwrap();
        let approval = db.create_approval(&call.id,"read",&json!({"input":input()})).unwrap();
        if decision!="pending" {
            db.resolve_approval(&approval.id,if decision=="approved" {super::super::ApprovalDecision::AllowOnce} else {super::super::ApprovalDecision::Deny}).unwrap();
        }
        assert!(db.native_run_waiting_approval(&run).unwrap());
        db.repair_interrupted_runs().unwrap();
        assert_eq!(status(&db,&run),"running");
        assert_eq!(db.get_runtime_tool_call(&run,"call").unwrap().unwrap().status,"pending");
        let approval_state: (String,Option<i64>) = db.with_connection(|connection| connection.query_row(
            "SELECT status,claimed_at FROM approvals WHERE id=?1",[&approval.id],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(approval_state,(decision.into(),None));
        let facts: (String,Option<String>,Option<i64>) = db.with_connection(|connection| connection.query_row(
            "SELECT state,result_json,claimed_at FROM kernel_tool_calls WHERE run_id=?1",[&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!(facts,("running".into(),None,Some(1)));
        assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err());
    }
}

#[test]
fn native_restart_interrupts_an_inflight_run_without_settling_its_unknown_effect() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    ledger(&db,&run,"running",None);
    db.create_host_tool_call(&run,"call","read",&input(),"running",false).unwrap();
    db.repair_interrupted_runs().unwrap();
    assert_eq!(status(&db,&run),"interrupted");
    assert_eq!(db.get_runtime_tool_call(&run,"call").unwrap().unwrap().status,"interrupted");
    let facts: (String,Option<String>) = db.with_connection(|connection| connection.query_row(
        "SELECT state,result_json FROM kernel_tool_calls WHERE run_id=?1",[&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!(facts,("running".into(),None));
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err());
}

fn terminal_fact(db: &Database, run: &str) -> (String,Option<String>,Option<String>,bool) {
    db.with_connection(|connection| connection.query_row(
        "SELECT status,error_code,error_message,finished_at IS NOT NULL FROM runs WHERE id=?1",[run],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap()
}

#[test]
fn a0_startup_unrouted_kernel_runs_refuse_host_writers_and_are_retired_at_startup() {
    let (db,run,_) = fixture(None);
    assert!(db.create_host_tool_call(&run,"call","read",&input(),"running",false).is_err());
    assert!(db.mark_run_failed(&run,"test","failure").is_err());
    assert!(db.mark_run_interrupted(&run,"test","interruption").is_err());
    assert!(!db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).unwrap());
    db.repair_interrupted_runs().unwrap();
    // A0: the frozen Kernel loop never resumes it; it is interrupted with the
    // retired-path reason.
    assert_eq!(terminal_fact(&db,&run),("interrupted".into(),Some("executor_route_retired".into()),Some("执行路径已退役".into()),true));
    // Still a Kernel-owned read model: Host writers stay refused.
    assert!(db.mark_run_failed(&run,"test","failure").is_err());
    // A second startup is a no-op.
    db.repair_interrupted_runs().unwrap();
    assert_eq!(terminal_fact(&db,&run).1.as_deref(),Some("executor_route_retired"));
}

#[test]
fn a0_startup_retirement_settles_the_kernel_read_model_and_leaves_other_routes_alone() {
    let (db,kernel,conversation) = fixture(None);
    let now = now_ms();
    db.with_connection(|connection| {
        connection.execute("INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
            VALUES('a0-message',?1,?2,'assistant','text','partial','streaming',9999,?3,?3)",params![conversation,kernel,now])?;
        connection.execute("INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,
            execution_location,requires_approval,started_at,updated_at)
            VALUES('kernel-tool:a0','a0-call',?1,?2,'write_file','{}','pending','host',1,?3,?3)",params![kernel,conversation,now])?;
        connection.execute("INSERT INTO approvals(id,tool_call_id,status,requested_action,request_json,requested_at)
            VALUES('kernel-approval:a0','kernel-tool:a0','pending','write_file','{}',?1)",[now])?;
        Ok(())
    }).unwrap();
    // A Kernel Run that already finished keeps its own terminal facts.
    let (finished_db,finished,_) = fixture(None);
    finished_db.with_connection(|connection| connection.execute(
        "UPDATE runs SET status='completed',finished_at=1 WHERE id=?1",[&finished])).unwrap();
    finished_db.repair_interrupted_runs().unwrap();
    assert_eq!(terminal_fact(&finished_db,&finished),("completed".into(),None,None,true));
    // A PiNative Run left running is the ordinary restart interruption.
    let (native_db,native,_) = fixture(Some(ExecutorRoute::PiNative));
    native_db.repair_interrupted_runs().unwrap();
    assert_eq!(terminal_fact(&native_db,&native).1.as_deref(),Some("runtime.application_restarted"));

    db.repair_interrupted_runs().unwrap();
    assert_eq!(terminal_fact(&db,&kernel).1.as_deref(),Some("executor_route_retired"));
    let settled: (String,String,String) = db.with_connection(|connection| connection.query_row(
        "SELECT (SELECT status FROM messages WHERE id='a0-message'),(SELECT status FROM tool_calls WHERE id='kernel-tool:a0'),
                (SELECT status FROM approvals WHERE id='kernel-approval:a0')",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
    assert_eq!(settled,("interrupted".into(),"interrupted".into(),"expired".into()));
}

#[test]
fn a0_startup_an_unknown_frozen_route_interrupts_only_that_run_with_executor_route_unknown() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    db.with_connection(|connection| {
        let raw: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1",[&run],|row|row.get(0))?;
        let raw = raw.replace("\"executorRoute\":\"pi_native\"","\"executorRoute\":\"kernel\"");
        let hash = format!("sha256:{}",hex::encode(Sha256::digest(raw.as_bytes())));
        connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1",params![run,raw,hash])?;
        Ok(())
    }).unwrap();
    // dbhash: one unverifiable binding no longer fails startup; its Run is
    // interrupted with the stable code and stays refused.
    db.repair_interrupted_runs().unwrap();
    assert_eq!(terminal_fact(&db,&run).0,"interrupted");
    assert_eq!(terminal_fact(&db,&run).1.as_deref(),Some("executor_route_unknown"));
    assert!(db.run_control_binding(&run).unwrap_err().contains("executor_route_unknown"));
    assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err());
}

#[test]
fn native_failure_and_interruption_use_host_terminal_guards() {
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    db.mark_run_failed(&run,"test.failure","failed").unwrap();
    assert_eq!(status(&db,&run),"failed");
    assert!(db.mark_run_interrupted(&run,"test.interrupt","late").is_err());
    let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
    db.mark_run_interrupted(&run,"test.interrupt","interrupted").unwrap();
    assert_eq!(status(&db,&run),"interrupted");
    assert!(db.mark_run_failed(&run,"test.failure","late").is_err());
}

#[test]
fn corrupt_or_unsupported_native_bindings_cannot_enter_any_host_writer() {
    for corruption in ["hash","unknown_field","permission_hash","authority_column","unsupported_route"] {
        let (db,run,_) = fixture(Some(ExecutorRoute::PiNative));
        db.with_connection(|connection| {
            let raw: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1",[&run],|row|row.get(0))?;
            let mut value: Value = serde_json::from_str(&raw).unwrap();
            match corruption {
                "unknown_field" => value["unexpected"] = json!(true),
                "permission_hash" => value["permission"]["mode"] = json!("allow"),
                "unsupported_route" => value["authority"] = json!("legacy"),
                _ => {},
            }
            let raw = value.canonical_string();
            let hash = if corruption=="hash" {"corrupt".into()} else {format!("sha256:{}",hex::encode(Sha256::digest(raw.as_bytes())))};
            connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3,
                authority=CASE WHEN ?4 IN ('authority_column','unsupported_route') THEN 'legacy' ELSE authority END WHERE run_id=?1",
                params![run,raw,hash,corruption])?;
            Ok(())
        }).unwrap();
        assert!(db.create_host_tool_call(&run,"call","read",&input(),"running",false).is_err(),"{corruption}");
        assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err(),"{corruption}");
        assert!(db.mark_run_failed(&run,"test","failed").is_err(),"{corruption}");
        assert!(db.mark_run_interrupted(&run,"test","interrupted").is_err(),"{corruption}");
        // dbhash: startup succeeds and only this Run is interrupted with the
        // integrity code; every writer keeps refusing it afterwards.
        db.repair_interrupted_runs().unwrap();
        db.audit_interrupted_tasks().unwrap();
        assert_eq!(terminal_fact(&db,&run).0,"interrupted","{corruption}");
        assert_eq!(terminal_fact(&db,&run).1.as_deref(),Some("run_control.binding_integrity_failed"),"{corruption}");
        assert!(db.create_host_tool_call(&run,"call","read",&input(),"running",false).is_err(),"{corruption}");
        assert!(db.apply_runtime_event(&run,1,&json!({"type":"run.completed"})).is_err(),"{corruption}");
        // `unsupported_route` decodes on its own; the route check in every
        // writer is what refuses it.
        if corruption != "unsupported_route" {
            assert!(db.run_control_binding(&run).unwrap_err().contains("run_control.binding_integrity_failed"),"{corruption}");
        }
    }
}

#[test]
fn graph_host_recovery_includes_native_owners_and_refuses_corrupt_bindings() {
    for route in [None,Some(ExecutorRoute::PiNative)] {
        let (db,run,_) = fixture(route);
        let rows=db.with_connection(|connection| {
            super::graph_lead::host_recovery_rows(connection,vec![(run.clone(),"pending")])
                .map_err(|error|projection_violation(error.to_string()))
        }).unwrap();
        assert_eq!(rows,if route.is_some() {vec!["pending"]} else {Vec::new()});
        db.with_connection(|connection|connection.execute("UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1",[&run])).unwrap();
        // dbhash: a refused binding is never recovered, and it no longer fails
        // recovery (and so startup) for every other row.
        assert_eq!(db.with_connection(|connection| {
            super::graph_lead::host_recovery_rows(connection,vec![(run.clone(),"pending")])
                .map_err(|error|projection_violation(error.to_string()))
        }).unwrap(),Vec::<&str>::new());

    }
}

#[test]
fn native_repair_override_keeps_its_specialized_atomic_once_claim() {
    let (db,input,tool,approval)=Database::native_repair_override_fixture_for_test();
    assert!(!db.claim_approved_tool_call(&input.run_id,&tool.runtime_tool_call_id).unwrap());
    let mut drift=input.clone();drift.root_cause="changed after approval".into();
    assert!(db.start_task_repair_override(drift).is_err());
    let claimed:Option<i64>=db.with_connection(|connection|connection.query_row("SELECT claimed_at FROM approvals WHERE id=?1",[&approval.id],|row|row.get(0))).unwrap();
    assert!(claimed.is_none());
    let started=db.start_task_repair_override(input.clone()).unwrap();
    assert_eq!(started.attempt.id,input.attempt_id);
    let replay=db.start_task_repair_override(input.clone()).unwrap();
    assert_eq!(replay.attempt.id,started.attempt.id);
    let claimed:Option<i64>=db.with_connection(|connection|connection.query_row("SELECT claimed_at FROM approvals WHERE id=?1",[&approval.id],|row|row.get(0))).unwrap();
    assert!(claimed.is_some());
}

/// Desktop restart, end to end through the same startup sequence `lib.rs` runs
/// (open with migration backup → repair → task audit) and the conversation
/// reader the desktop loads: a historical Kernel Run is shown interrupted with
/// the retired-path reason, is never recovered, and the next Run is PiNative.
#[test]
fn a0_startup_desktop_restart_shows_the_retired_kernel_run_and_continues_on_pi_native() {
    let root = std::env::temp_dir().join(format!("fox-a0-restart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("fox.db");
    let (conversation, kernel_run) = {
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("A0 restart"), None, Some("ask")).unwrap();
        let run = db.create_run(&conversation.id, "historical Kernel task", None).unwrap().run.id;
        let binding = db.freeze_kernel_run_control(&run, "legacy", TimeBudgets::continuous()).unwrap();
        assert_eq!(binding.executor_route, None);
        db.with_connection(|connection| connection.execute("UPDATE runs SET status='running',started_at=?2 WHERE id=?1",params![run,now_ms()])).unwrap();
        (conversation.id, run)
    };
    let db = Database::open_with_migration_backup(path.clone(), &root.join("backups")).unwrap();
    db.repair_interrupted_runs().unwrap();
    db.audit_interrupted_tasks().unwrap();
    let detail = db.load_conversation(&conversation).unwrap();
    let last = detail.last_run.expect("the retired Run is still the conversation's last Run");
    assert_eq!(last.id, kernel_run);
    assert_eq!(last.status, "interrupted");
    assert_eq!(last.error_code.as_deref(), Some("executor_route_retired"));
    assert_eq!(last.error_message.as_deref(), Some("执行路径已退役"));
    // The user continues in a new Run, which is frozen PiNative.
    let next = db.create_run(&conversation, "continue in a new Run", None).unwrap().run.id;
    let binding = db.freeze_default_run_control_for_engine(&next, "legacy", TimeBudgets::continuous(), "pi").unwrap();
    assert_eq!(binding.executor_route, Some(ExecutorRoute::PiNative));
    drop(db);
    let _ = std::fs::remove_dir_all(&root);
}
