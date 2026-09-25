//! B2b-1 storage tests. These use a real SQLite Run, model lease and Job
//! settlement; they do not claim that the detached Host/wake loop is wired.
use super::*;
use crate::database::{JobStartRequest, ModelNoticeInput};
use crate::kernel::{RunState, WaitingJobFact};
use serde_json::json;
use sha2::Digest;

pub(super) fn parked_job_fixture() -> (Database, PathBuf, String, String, String, RunController, u64, i64) {
    parked_job_fixture_with_mutation(None)
}

fn parked_job_fixture_with_mutation(mutation: Option<&str>)
    -> (Database, PathBuf, String, String, String, RunController, u64, i64) {
    parked_job_fixture_with_inputs(mutation,None)
}

fn parked_job_fixture_with_inputs(mutation: Option<&str>, history_override: Option<serde_json::Value>)
    -> (Database, PathBuf, String, String, String, RunController, u64, i64) {
    let now = crate::database::now_ms();
    let clock = TestClock::new(now);
    let model = worker_configuration();
    let (db, root, run) = fixture_with_start_opt(
        &clock, &model.hash().unwrap(), Some(&model), true, true);
    freeze_host_scope(&db, &run);
    db.with_connection(|conn| conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1", [&run],
    )).unwrap();
    assert!(db.compute_job_notice_enabled(&run).unwrap());
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let cancellation = CancellationRegistry::default();
    drop(KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap());

    let job = db.kernel_job_start(&JobStartRequest {
        run_id: run.clone(), kind: "attachment_compute".into(),
        idempotency_key: "waiting-storage".into(), params: json!({"input":"fixture"}),
        deadline_ms: Some(now+6_000), progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    if let Some(sql)=mutation {
        db.with_connection(|conn|conn.execute(sql,[&job])).unwrap();
    }

    let mut controller = RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let input = db.kernel_initial_input(&run).unwrap();
    let frame = fox_engine_protocol::KernelInitialModelFrame {
        schema_version: 1, input, idempotency_key: kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(),
        continuation_key: None, continuation_lane: None,
        checkpoint_seq: controller.last_event_seq(),
        host_job_notices: Vec::new(),
    };
    frame.validate().unwrap();
    let frame_json = serde_json::to_value(&frame).unwrap();
    let historical_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&frame.input.messages).unwrap();
    let binding = ModelNoticeInput {
        payload: &frame_json, delivered_history: &frame.input.messages,
        live_history: None, checkpoint_seq: frame.checkpoint_seq,
        history_start: frame.input.messages.len(), historical_bytes,
    };
    let dispatched = controller.begin_initial_model_request(now,now).unwrap();
    db.kernel_commit_initial_model(&run,now,&controller.persist_command(&dispatched),
        "owner",false,None,Some(&binding)).unwrap();

    let response = fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: run.clone(), turn_id: frame.input.turn_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"The child Job is still running."}]}),
    };
    response.validate().unwrap();
    let response_json = serde_json::to_string(&response).unwrap();
    let settled = controller.record_initial_model_response(&response_json).unwrap();
    let response_seq = controller.last_event_seq();
    let mut effects = vec![settled];
    let parked_history = history_override.unwrap_or_else(||json!(frame.input.messages));
    effects.extend(controller.park_waiting_jobs(now,now,db.kernel_data_root_id(),
        response_seq,&response_json,&parked_history.to_string(),
        &[WaitingJobFact {job_id: job.clone(),attempt:1,deadline_wall_ms:now+6_000}]).unwrap());
    let committed=db.kernel_commit_initial_model(&run,now,&controller.persist_command(&effects),
        "owner",true,None,None);
    let park_seq = controller.last_event_seq();
    if mutation.is_none() && parked_history == json!(frame.input.messages) {
        committed.unwrap();
        assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::WaitingJobs);
    } else {
        assert!(committed.is_err(),"mutated Job must reject the whole park transaction");
        assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::Running);
    }
    (db,root,run,conversation,job,controller,park_seq,now)
}

#[test]
fn waiting_jobs_rejects_history_not_bound_to_the_settled_model_lease() {
    let (db,_root,run,_conversation,_job,_controller,_park_seq,_now)=
        parked_job_fixture_with_inputs(None,Some(json!([{"role":"user","content":[{"type":"text","text":"forged"}]}])));
    let count:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(count,0,"a fabricated history must roll back response settlement and park");
    let bound:(String,String,String)=db.with_connection(|conn|conn.query_row(
        "SELECT input_json,input_hash,state FROM kernel_model_notice_inputs WHERE run_id=?1",
        [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
    assert_eq!(bound.2,"bound","failed settlement must retain the original lease");
    assert_eq!(bound.1,format!("sha256:{}",hex::encode(sha2::Sha256::digest(bound.0.as_bytes()))));
}

#[test]
fn waiting_jobs_model_lease_rejects_forged_initial_delivery_history() {
    let now=crate::database::now_ms();
    let clock=TestClock::new(now);
    let model=worker_configuration();
    let (db,_root,run)=fixture_with_start_opt(&clock,&model.hash().unwrap(),Some(&model),true,true);
    db.with_connection(|conn|conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1",[&run],
    )).unwrap();
    let cancellation=CancellationRegistry::default();
    drop(KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap());
    let mut controller=RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let input=db.kernel_initial_input(&run).unwrap();
    let frame=fox_engine_protocol::KernelInitialModelFrame {
        schema_version:1,input,idempotency_key:kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(),
        continuation_key:None,continuation_lane:None,
        checkpoint_seq:controller.last_event_seq(),host_job_notices:vec![],
    };
    frame.validate().unwrap();
    let payload=serde_json::to_value(&frame).unwrap();
    let mut forged=frame.input.messages.clone();
    forged.push(json!({"role":"assistant","stopReason":"stop",
        "content":[{"type":"text","text":"never delivered"}]}));
    let binding=ModelNoticeInput {payload:&payload,delivered_history:&forged,
        live_history:None,checkpoint_seq:frame.checkpoint_seq,
        history_start:forged.len(),historical_bytes:0};
    let effects=controller.begin_initial_model_request(now,now).unwrap();
    let error=db.kernel_commit_initial_model(&run,now,&controller.persist_command(&effects),
        "owner",false,None,Some(&binding)).unwrap_err();
    assert!(error.contains("bound model history"),"unexpected error: {error}");
    let mut forged_frame=frame.clone();
    forged_frame.input.messages[0]["content"]=json!("a different user request");
    forged_frame.validate().unwrap();
    let forged_payload=serde_json::to_value(&forged_frame).unwrap();
    let forged_binding=ModelNoticeInput {payload:&forged_payload,
        delivered_history:&forged_frame.input.messages,live_history:None,
        checkpoint_seq:forged_frame.checkpoint_seq,
        history_start:forged_frame.input.messages.len(),historical_bytes:0};
    let error=db.kernel_commit_initial_model(&run,now,&controller.persist_command(&effects),
        "owner",false,None,Some(&forged_binding)).unwrap_err();
    assert!(error.contains("bound initial frame differs from frozen Host sources"),
        "synchronously forged frame and history were not checked against frozen source: {error}");
    let mut foreign_frame=frame.clone();
    foreign_frame.input.run_id="another-run".into();
    foreign_frame.validate().unwrap();
    let foreign_payload=serde_json::to_value(&foreign_frame).unwrap();
    let foreign_binding=ModelNoticeInput {payload:&foreign_payload,
        delivered_history:&foreign_frame.input.messages,live_history:None,
        checkpoint_seq:foreign_frame.checkpoint_seq,
        history_start:foreign_frame.input.messages.len(),historical_bytes:0};
    let error=db.kernel_commit_initial_model(&run,now,&controller.persist_command(&effects),
        "owner",false,None,Some(&foreign_binding)).unwrap_err();
    assert!(error.contains("bound initial frame identity differs from frozen Host scope"),
        "frame identity forged without changing history: {error}");
    let stored:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1",[&run],|row|row.get(0))).unwrap();
    assert_eq!(stored,0,"the forged frame must not be leased");
}

#[test]
fn waiting_jobs_bound_batch_history_reconstructs_tool_and_steering_middle() {
    let frame=fox_engine_protocol::KernelBatchResumeFrame {
        schema_version:1,turn_id:"turn".into(),batch_id:"batch".into(),
        idempotency_key:"tool-batch-delivery:batch".into(),checkpoint_seq:1,
        history:vec![json!({"role":"user","content":"read"})],
        assistant_message:json!({"role":"assistant","stopReason":"toolUse",
            "content":[{"type":"toolCall","id":"read-1","name":"read","arguments":{"path":"a.txt"}}]}),
        tools:vec![fox_engine_protocol::KernelSettledToolResult {
            tool_call_id:"read-1".into(),tool:"read".into(),canonical_input:json!({"path":"a.txt"}),
            source_order:0,storage:None,state:fox_engine_protocol::KernelSettledToolState::Completed,
            result:json!({"content":[{"type":"text","text":"proof"}]}),
        }],
        steering:vec![fox_engine_protocol::KernelSteeringNotice {
            message_id:"steer-1".into(),content:"继续".into(),received_at:Some(2),
        }],host_job_notices:vec![],
    };
    frame.validate().unwrap();
    let payload=serde_json::to_value(&frame).unwrap();
    let (actual,start)=super::super::bound_model_delivery_history("run",&payload,None).unwrap();
    assert_eq!(start,4);
    assert_eq!(actual[1],frame.assistant_message);
    assert_eq!(actual[2]["role"],"toolResult");
    assert_eq!(actual[3]["role"],"user");
    let mut fake=actual;
    fake[2]["content"]=json!([{"type":"text","text":"forged"}]);
    assert_ne!(fake,super::super::bound_model_delivery_history("run",&payload,None).unwrap().0);
}

#[test]
fn waiting_jobs_park_rejects_missing_expired_or_changed_original_job_facts() {
    for mutation in [
        "UPDATE kernel_jobs SET deadline_ms=NULL WHERE job_id=?1",
        "UPDATE kernel_jobs SET deadline_ms=1 WHERE job_id=?1",
        "UPDATE kernel_jobs SET attempts=2 WHERE job_id=?1",
    ] {
        let (db,_root,run,_conversation,_job,_controller,_park_seq,_now)=
            parked_job_fixture_with_mutation(Some(mutation));
        let count:i64=db.with_connection(|conn|conn.query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
            [&run],|row|row.get(0))).unwrap();
        assert_eq!(count,0);
        let leased:String=db.with_connection(|conn|conn.query_row(
            "SELECT status FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='initial-model'",
            [&run],|row|row.get(0))).unwrap();
        assert_eq!(leased,"leased","failed park must not settle original model lease");
    }
}

#[test]
fn waiting_jobs_per_round_settled_final_parks_instead_of_completing() {
    let now=crate::database::now_ms();
    let clock=TestClock::new(now);
    let model=worker_configuration();
    let (db,_root,run)=fixture_with_start_opt(&clock,&model.hash().unwrap(),Some(&model),true,true);
    db.with_connection(|conn|conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1",[&run],
    )).unwrap();
    let cancellation=CancellationRegistry::default();
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=db.kernel_job_start(&JobStartRequest {
        run_id:run.clone(),kind:"attachment_compute".into(),idempotency_key:"round-park".into(),
        params:json!({"input":"local fixture"}),deadline_ms:Some(now+6_000),progress_total:None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    coordinator.dispatch_initial("park-owner",&Allow,|binding,frame,_| {
        Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"stop",
                "content":[{"type":"text","text":"Job still running"}]}),
        })
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state,"waiting_jobs");
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(),Some("waiting_jobs"));
    let (terminal,status):(i64,String)=db.with_connection(|conn|conn.query_row(
        "SELECT k.terminal_written,r.status FROM kernel_runs k JOIN runs r ON r.id=k.run_id
         WHERE k.run_id=?1",[&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!((terminal,status),(0,"running".into()));
}

#[test]
fn waiting_jobs_park_is_atomic_rooted_and_accounted_once_after_reopen() {
    let (db,root,run,_conversation,_job,_controller,park_seq,now)=parked_job_fixture();
    assert!(db.kernel_update_run_state(&run,"completed").is_err());
    let reopened=Database::open(root.join("facts.db")).unwrap();
    let old=RunController::rehydrate(reopened.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let mut first=old.clone();
    let mut stale=old;
    let debit=first.account_waiting_jobs(park_seq,now+1_000).unwrap();
    reopened.kernel_commit_decision(&run,now+1_000,&first.persist_command(&debit)).unwrap();
    let duplicate=stale.account_waiting_jobs(park_seq,now+1_000).unwrap();
    assert!(db.kernel_commit_decision(&run,now+1_000,&stale.persist_command(&duplicate)).is_err());
    let saved=db.kernel_rehydrate(&run).unwrap().unwrap();
    assert_eq!(saved.running_elapsed_ms,1_000);
    assert_eq!(saved.wait_accounted_until_wall_ms,Some(now+1_000));
    assert_eq!(saved.wait_deadline_wall_ms,Some(now+6_000));
    let count:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_wait_accounted'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(count,1);
}

#[test]
fn waiting_jobs_wake_outbox_failure_rolls_back_and_two_handles_cannot_wake_twice() {
    let (db,root,run,conversation,job,_controller,park_seq,now)=parked_job_fixture();
    let other=Database::open(root.join("facts.db")).unwrap();
    let old=RunController::rehydrate(other.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    db.kernel_job_complete_attempt(&job,1,&conversation,&json!({"answer":"ready"})).unwrap();
    assert!(db.kernel_job_notice(&conversation,&run,&job).unwrap().is_some());
    let mut first=old.clone();
    let mut second=old;
    let mut input=db.kernel_initial_input(&run).unwrap();
    let parked:String=db.with_connection(|conn|conn.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run],|row|row.get(0))).unwrap();
    let parked:serde_json::Value=serde_json::from_str(&parked).unwrap();
    input.messages=serde_json::from_value(parked["history"].clone()).unwrap();
    input.messages.push(parked["response"]["assistantMessage"].clone());
    let notices=db.pending_host_job_notices(&conversation,&run,16*1024).unwrap();
    assert_eq!(notices.len(),1);
    let next_seq=first.last_event_seq()+2;
    let payload=json!({"turnId":input.turn_id,"effectKey":format!("continuation:{next_seq}"),
        "lane":"job_notice","prompt":"","input":input,
        "hostJobNotices":notices});
    for forged in [
        { let mut value=payload.clone(); value["hostJobNotices"]=json!([]); value },
        { let mut value=payload.clone(); value["input"]["messages"][0]["content"]=json!("forged"); value },
    ] {
        let mut rejected=first.clone();
        let effects=rejected.wake_waiting_jobs(park_seq,now+1_000,&forged.to_string()).unwrap();
        let error=db.kernel_commit_decision(&run,now+1_000,&rejected.persist_command(&effects)).unwrap_err();
        assert!(error.contains("job notice"),"unexpected wake refusal: {error}");
        assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::WaitingJobs);
        let pending:i64=db.with_connection(|conn|conn.query_row(
            "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model'",
            [&run],|row|row.get(0))).unwrap();
        assert_eq!(pending,0,"forged wake may not create an outbox");
    }
    let payload=payload.to_string();
    let effects=first.wake_waiting_jobs(park_seq,now+1_000,&payload).unwrap();
    let command=first.persist_command(&effects);
    let old_policy=db.execution_policy(&conversation).unwrap();
    let changed=db.change_execution_policy(&conversation,"waiting-policy-race",
        old_policy.version,if old_policy.mode=="allow" {"ask"} else {"allow"}).unwrap();
    assert_ne!(changed.version,old_policy.version);
    let stale_policy=db.kernel_commit_waiting_wake(&run,now+1_000,&command,
        old_policy.version).unwrap_err();
    assert!(stale_policy.contains("job_wake_policy_changed"),"{stale_policy}");
    assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::WaitingJobs);
    let outbox_before:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(outbox_before,0,"policy CAS failure must roll back the wake outbox");
    db.with_connection(|conn|conn.execute_batch(
        "CREATE TRIGGER reject_job_wake_outbox BEFORE INSERT ON kernel_effect_outbox
          WHEN NEW.effect_type='continuation_model'
          BEGIN SELECT RAISE(ABORT,'injected job wake outbox rollback'); END;"
    )).unwrap();
    assert!(db.kernel_commit_decision(&run,now+1_000,&command).is_err());
    let still=db.kernel_rehydrate(&run).unwrap().unwrap();
    assert_eq!(still.state,RunState::WaitingJobs);
    assert_eq!(still.running_elapsed_ms,0);
    let wake_count:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(wake_count,0);
    db.with_connection(|conn|conn.execute_batch("DROP TRIGGER reject_job_wake_outbox;")) .unwrap();
    db.kernel_commit_decision(&run,now+1_000,&command).unwrap();
    assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::Running);
    let stale=second.wake_waiting_jobs(park_seq,now+1_000,&payload).unwrap();
    assert!(other.kernel_commit_decision(&run,now+1_000,&second.persist_command(&stale)).is_err());
    let counts:(i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model')",
        [&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!(counts,(1,1));
}

#[test]
fn automatic_waiting_wake_cannot_overtake_a_queued_cancel() {
    let (db,_root,run,conversation,job,_controller,park_seq,now)=parked_job_fixture();
    db.kernel_job_complete_attempt(&job,1,&conversation,&json!({"answer":"ready"})).unwrap();
    let mut controller=RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let mut input=db.kernel_initial_input(&run).unwrap();
    let parked:String=db.with_connection(|conn|conn.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run],|row|row.get(0))).unwrap();
    let parked:serde_json::Value=serde_json::from_str(&parked).unwrap();
    input.messages=serde_json::from_value(parked["history"].clone()).unwrap();
    input.messages.push(parked["response"]["assistantMessage"].clone());
    let notices=db.pending_host_job_notices(&conversation,&run,16*1024).unwrap();
    let next_seq=controller.last_event_seq()+2;
    let payload=json!({"turnId":input.turn_id,"effectKey":format!("continuation:{next_seq}"),
        "lane":"job_notice","prompt":"","input":input,"hostJobNotices":notices});
    let effects=controller.wake_waiting_jobs(park_seq,now+1_000,&payload.to_string()).unwrap();
    let version=db.execution_policy(&conversation).unwrap().version;
    db.queue_kernel_host_command(&run,None).unwrap();
    let rejected=db.kernel_commit_waiting_wake(&run,now+1_000,
        &controller.persist_command(&effects),version).unwrap_err();
    assert!(rejected.contains("job_wake_cancel_pending"),"{rejected}");
    assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().state,RunState::WaitingJobs);
    let (wakes,outbox):(i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model')",
        [&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!((wakes,outbox),(0,0));
}

#[test]
fn waiting_deadline_accounting_cannot_overtake_an_already_queued_cancel() {
    let (db,_root,run,_conversation,_job,_controller,park_seq,now)=parked_job_fixture();
    let mut controller=RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let effects=controller.account_waiting_jobs(park_seq,now+6_000).unwrap();
    assert_eq!(controller.state(),RunState::Failed,
        "without a cancel, the original deadline would terminate the Run");
    db.queue_kernel_host_command(&run,None).unwrap();
    let rejected=db.kernel_commit_waiting_account(&run,now+6_000,
        &controller.persist_command(&effects)).unwrap_err();
    assert!(rejected.contains("job_wake_cancel_pending"),"{rejected}");
    let saved=db.kernel_rehydrate(&run).unwrap().unwrap();
    assert_eq!(saved.state,RunState::WaitingJobs);
    assert_eq!(saved.wait_accounted_until_wall_ms,Some(now));
    let terminal:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type IN
            ('run.failed','run.budget_exhausted','run.jobs_woken')",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(terminal,0,"cancel-first CAS must roll back the deadline terminal");
}

#[test]
fn cancelled_wait_reopen_only_consumes_its_durable_cancel_command() {
    let (db,_root,run,_conversation,_job,_controller,_park_seq,now)=parked_job_fixture();
    let clock=TestClock::new(now+1_000);
    let cancellation=CancellationRegistry::default();
    cancellation.register_run(&run).unwrap();
    cancellation.request_run_cancel(&run);
    assert!(KernelCoordinator::reopen(&db,&clock,&run,&cancellation).is_err(),
        "ordinary reopen must not revive the cancelled execution scope");
    assert!(KernelCoordinator::reopen_cancelled_wait(&db,&clock,&run,&cancellation).is_err(),
        "a cancelled token alone is not authority to settle a user command");
    db.queue_kernel_host_command(&run,None).unwrap();
    let coordinator=KernelCoordinator::reopen_cancelled_wait(&db,&clock,&run,&cancellation).unwrap();
    coordinator.cancel_waiting_jobs_now().unwrap();
    assert!(KernelCoordinator::reopen_cancelled_wait(&db,&clock,&run,&cancellation).is_err(),
        "a pending command cannot reopen an already cancelling Run for execution");
    coordinator.settle_cancellation().unwrap();
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(),Some("cancelled"));
    assert!(cancellation.run_token(&run).unwrap().is_cancelled());
    let requests:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(requests,0,"cancel-only reconciliation may not create a model request");
}

#[test]
fn terminal_job_direct_notice_requires_the_settled_input_and_exact_typed_fact() {
    let now=crate::database::now_ms();
    let clock=TestClock::new(now);
    let model=worker_configuration();
    let (db,_root,run)=fixture_with_start_opt(&clock,&model.hash().unwrap(),Some(&model),true,true);
    freeze_host_scope(&db,&run);
    db.with_connection(|conn|conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1",[&run])).unwrap();
    let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let cancellation=CancellationRegistry::default();
    drop(KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap());
    let job=db.kernel_job_start(&JobStartRequest {
        run_id:run.clone(),kind:"attachment_compute".into(),idempotency_key:"direct-notice".into(),
        params:json!({"input":"fixture"}),deadline_ms:Some(now+6_000),progress_total:None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    let mut controller=RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let input=db.kernel_initial_input(&run).unwrap();
    let frame=fox_engine_protocol::KernelInitialModelFrame {
        schema_version:1,input,idempotency_key:kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(),
        continuation_key:None,continuation_lane:None,checkpoint_seq:controller.last_event_seq(),
        host_job_notices:vec![],
    };
    let frame_json=serde_json::to_value(&frame).unwrap();
    let binding=ModelNoticeInput {payload:&frame_json,delivered_history:&frame.input.messages,
        live_history:None,checkpoint_seq:frame.checkpoint_seq,
        history_start:frame.input.messages.len(),historical_bytes:0};
    let effects=controller.begin_initial_model_request(now,now).unwrap();
    db.kernel_commit_initial_model(&run,now,&controller.persist_command(&effects),
        "owner",false,None,Some(&binding)).unwrap();
    db.kernel_job_complete_attempt(&job,1,&conversation,&json!({"answer":"ready"})).unwrap();
    let notices=db.pending_host_job_notices(&conversation,&run,16*1024).unwrap();
    assert_eq!(notices.len(),1);
    let response=fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:run.clone(),turn_id:frame.input.turn_id.clone(),
        checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"The calculation is ready."}]}),
    };
    let response_json=serde_json::to_string(&response).unwrap();
    let mut next_input=frame.input.clone();
    next_input.messages.push(response.assistant_message.clone());
    for forged in [
        { let mut value=next_input.clone(); value.messages[0]["content"]=json!("forged");
          (value,notices.clone()) },
        (next_input.clone(),vec![]),
    ] {
        let mut rejected=controller.clone();
        let mut effects=vec![rejected.record_initial_model_response(&response_json).unwrap()];
        if forged.1.is_empty() {
            // The controller itself may refuse a missing notice before the
            // transaction, which is still a fail-closed result.
            assert!(rejected.request_job_notice_followup(
                &serde_json::to_string(&forged.0).unwrap(),"[]").is_err());
            continue;
        }
        effects.extend(rejected.request_job_notice_followup(
            &serde_json::to_string(&forged.0).unwrap(),
            &serde_json::to_string(&forged.1).unwrap()).unwrap());
        let error=db.kernel_commit_initial_model(&run,now,&rejected.persist_command(&effects),
            "owner",true,None,None).unwrap_err();
        assert!(error.contains("job notice"),"unexpected direct refusal: {error}");
    }
    let mut accepted=controller;
    let mut effects=vec![accepted.record_initial_model_response(&response_json).unwrap()];
    effects.extend(accepted.request_job_notice_followup(
        &serde_json::to_string(&next_input).unwrap(),
        &serde_json::to_string(&notices).unwrap()).unwrap());
    db.kernel_commit_initial_model(&run,now,&accepted.persist_command(&effects),
        "owner",true,None,None).unwrap();
    let outbox:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model' AND status='pending'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(outbox,1);
}

#[test]
fn job_finishing_between_pre_read_and_response_commit_replans_without_model_replay() {
    let now=crate::database::now_ms();
    let clock=TestClock::new(now);
    let model=worker_configuration();
    let (db,_root,run)=fixture_with_start_opt(&clock,&model.hash().unwrap(),Some(&model),true,true);
    freeze_host_scope(&db,&run);
    db.with_connection(|conn|conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1",[&run])).unwrap();
    let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let cancellation=CancellationRegistry::default();
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=db.kernel_job_start(&JobStartRequest {
        run_id:run.clone(),kind:"attachment_compute".into(),idempotency_key:"finish-in-window".into(),
        params:json!({"input":"fixture"}),deadline_ms:Some(now+6_000),progress_total:None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    let db_barrier=db.clone();
    super::live::test_barrier::install(&run,Box::new(move || {
        db_barrier.kernel_job_complete_attempt(&job,1,&conversation,&json!({"answer":"ready"}))
            .expect("the child finishes after Host pre-read but before response commit");
    }));
    let model_calls=std::sync::atomic::AtomicUsize::new(0);
    coordinator.dispatch_initial("owner",&Allow,|binding,frame,_| {
        model_calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"stop",
                "content":[{"type":"text","text":"The calculation is running."}]}),
        })
    }).unwrap();
    assert_eq!(model_calls.load(std::sync::atomic::Ordering::SeqCst),1,
        "the held response must be replanned without another model request");
    assert_eq!(coordinator.snapshot().unwrap().state,"running");
    let (responses,continuations,parks):(i64,i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_response'),
                (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs')",
        [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
    assert_eq!((responses,continuations,parks),(1,1,0));
}

#[test]
fn waiting_jobs_copied_database_cannot_reopen_old_root_wait() {
    let (db,root,run,_conversation,_job,_controller,_park_seq,_now)=parked_job_fixture();
    let original_root=db.kernel_data_root_id().to_string();
    assert!(db.kernel_rehydrate(&run).unwrap().is_some());
    drop(db); // close/checkpoint the source file before making a byte copy
    let copied=std::env::temp_dir().join(format!("fox-wait-copy-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&copied).unwrap();
    std::fs::copy(root.join("facts.db"),copied.join("facts.db")).unwrap();
    let foreign=Database::open(copied.join("facts.db")).unwrap();
    assert_ne!(foreign.kernel_data_root_id(),original_root);
    assert!(foreign.kernel_rehydrate(&run).is_err());
    assert!(foreign.kernel_update_run_state(&run,"completed").is_err());
}

#[test]
fn waiting_jobs_reopen_rejects_changed_frozen_attempt() {
    let (db,_root,run,_conversation,job,_controller,_park_seq,_now)=parked_job_fixture();
    db.with_connection(|conn|conn.execute(
        "UPDATE kernel_jobs SET attempts=2 WHERE job_id=?1",[&job])).unwrap();
    assert!(db.kernel_rehydrate(&run).is_err());
}

#[test]
fn waiting_jobs_owned_host_loop_returns_parked_without_error_or_forced_child_settlement() {
    let (db,root,run,_conversation,_job,_controller,_park_seq,_now)=parked_job_fixture();
    let cancellation=CancellationRegistry::default();
    cancellation.register_run(&run).unwrap();
    let token=cancellation.run_token(&run).unwrap();
    let ownership=super::super::super::kernel_host::acquire(&root,&run).unwrap();
    let outcome=super::super::super::kernel_host::drive_with_actions_transport(
        &ownership,&db,&super::super::super::shadow_reconcile::ReconcilerClock,&cancellation,&run,
        &super::super::super::RuntimeCommand {program:"must-not-start-model".into(),script:None},
        "",&Allow,|_,_,_|panic!("parked Run cannot dispatch a tool"),
        |_|panic!("parked Run cannot dispatch a Host action"),
        |force|{assert!(!force,"parked Run cannot force-settle children");Ok(())},
        &|_|{},false).unwrap();
    assert_eq!(outcome,super::super::super::kernel_host::KernelDriveOutcome::Parked);
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(),Some("waiting_jobs"));
    assert!(!token.is_cancelled(),"parent token must remain live across park");
    drop(ownership);
    let reacquired=super::super::super::kernel_host::acquire(&root,&run).unwrap();
    drop(reacquired);
}

#[test]
fn waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token() {
    real_host_lock_race_body(true).unwrap();
}

/// Zero-argument wrapper so the bounded repeat probe can share the exact body
/// and receive the lock trace as a value rather than as a swallowed panic.
pub(super) fn real_host_lock_race_probe() -> Result<(), String> {
    real_host_lock_race_body(true)
}

/// The same body with auto-wake disabled for the Run, used as the CONTROL that
/// attributes the conflict to the watch task. Test-only switch; no production
/// lock semantics change.
pub(super) fn real_host_lock_race_probe_auto_wake_disabled() -> Result<(), String> {
    real_host_lock_race_body_with_auto_wake(true, true)
}

/// The body above, shared verbatim with the bounded repeat probe so the probe
/// exercises exactly the production/test scenario instead of a copy. The
/// assertions stay in place; only the final reacquire reports its failure as a
/// value so the probe can print the captured lock trace. `report_phases` adds
/// opt-in lock-trace markers (no-ops unless `FOX_RUN_LOCK_TRACE` is set).
pub(super) fn real_host_lock_race_body(report_phases: bool) -> Result<(), String> {
    real_host_lock_race_body_with_auto_wake(report_phases, false)
}

/// The shared body. `disable_auto_wake` turns off the Host's WaitingJobs watch
/// task for this Run before the start; that is the control showing which lock
/// holder the immediate reacquire is racing.
fn real_host_lock_race_body_with_auto_wake(
    report_phases: bool,
    disable_auto_wake: bool,
) -> Result<(), String> {
    use tauri::Manager;
    let (db,root,run,_conversation,_job,_controller,_park_seq,_now)=parked_job_fixture();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create=false;
    }
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let host=super::super::super::RuntimeHost::new(
        app.handle().clone(),db.clone(),root.clone(),root.join("attachments"),
        root.join("skills"),crate::yuxi::YuxiClient::new().unwrap());
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    let token={
        let state=host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        // This is the parent token already issued to a compute Job before the
        // Host returns. Re-registering the Run must reuse, not replace, it.
        state.cancellation.tool_token(&run,"job-start").unwrap()
    };
    let ownership=super::super::super::kernel_host::acquire(&root,&run).unwrap();
    if report_phases {
        crate::runtime_host::kernel_run_lock::lock_trace::record(format!("== T611 own run={run}"));
    }
    // Force the exact interleaving: the ordinary start owns the OS lock but
    // has not yet inserted kernel_active_runs. Recovery must not insert a
    // shadow owner whose later cleanup could remove the real owner's marker.
    host.recover_kernel_runs_detached().unwrap();
    if report_phases {
        crate::runtime_host::kernel_run_lock::lock_trace::record(format!("== T615 recovered run={run}"));
    }
    assert!(!host.state.lock().unwrap().kernel_active_runs.contains(&run));
    assert!(!token.is_cancelled());
    if disable_auto_wake {
        // Control only: keep the Host's WaitingJobs watch task from competing
        // for the OS lease, so the start/recovery paths are observed alone.
        host.disable_auto_wake_for_test(&run);
    }
    host.start_kernel_run(ownership,&binding,serde_json::Value::Null,
        serde_json::Value::Null).unwrap();
    if report_phases {
        crate::runtime_host::kernel_run_lock::lock_trace::record(format!("== T619 start-returned run={run}"));
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(),Some("waiting_jobs"));
    assert!(host.state.lock().unwrap().kernel_active_runs.is_empty());
    assert!(!token.is_cancelled(),"Parked must preserve the issued Job parent token");
    if report_phases {
        crate::runtime_host::kernel_run_lock::lock_trace::record(format!("== T623 about-to-reacquire run={run}"));
    }
    let ownership=super::super::super::kernel_host::acquire(&root,&run).map_err(|error| {
        // Mark the failing observation so the captured trace shows exactly which
        // conflict blocked the reacquire, not merely the first conflict in the
        // window (the recovery probe's self-conflict).
        crate::runtime_host::kernel_run_lock::lock_trace::record(format!(
            "== T623 CONFLICT-HERE run={run} error={error} by={}",
            crate::runtime_host::kernel_run_lock::lock_trace::who()
        ));
        format!(
            "T623 immediate reacquire after park failed with {error}; lock trace:\n{}",
            crate::runtime_host::kernel_run_lock::lock_trace::take().join("\n")
        )
    })?;
    // A conflict must never have cancelled the issued parent token.
    assert!(!token.is_cancelled(),"Parked must preserve the issued Job parent token");
    drop(ownership);
    drop(host);
    drop(app);
    Ok(())
}

#[test]
fn waiting_jobs_created_start_error_still_retires_and_records_kernel_failure() {
    if std::env::var_os("FOX_TEST_CREATED_START_FAILURE_CHILD").is_none() {
        // runtime_command reads a process-wide override. Isolate the negative
        // from parallel tests so no other Run observes the missing executable.
        let missing=std::env::temp_dir().join(format!("fox-missing-runtime-{}",uuid::Uuid::new_v4()));
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .arg("waiting_jobs_created_start_error_still_retires_and_records_kernel_failure")
            .arg("--test-threads=1")
            .env("FOX_TEST_CREATED_START_FAILURE_CHILD","1")
            .env("FOX_RUNTIME_EXECUTABLE",&missing)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let started=std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed()>std::time::Duration::from_secs(15) {
                let _=child.kill();
                let _=child.wait();
                panic!("isolated created-start failure test exceeded 15 seconds");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output=child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",
            String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return;
    }
    use tauri::Manager;
    let now=crate::database::now_ms();
    let clock=TestClock::new(now);
    let model=worker_configuration();
    let (db,root,run)=fixture_with_start_opt(&clock,&model.hash().unwrap(),Some(&model),true,true);
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {window.create=false;}
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let host=super::super::super::RuntimeHost::new(
        app.handle().clone(),db.clone(),root.clone(),root.join("attachments"),
        root.join("skills"),crate::yuxi::YuxiClient::new().unwrap());
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    let token={let state=host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.run_token(&run).unwrap()};
    assert!(super::super::super::kernel_host::kernel_scope_should_retire(&db,&run));
    let ownership=super::super::super::kernel_host::acquire(&root,&run).unwrap();
    let error=host.start_kernel_run(ownership,&binding,serde_json::Value::Null,
        serde_json::Value::Null).unwrap_err();
    assert!(error.contains("runtime executable was not found"),"unexpected error: {error}");
    assert!(token.is_cancelled(),"created Run must retire its old scope on failed preparation");
    host.record_kernel_start_failure(&run).unwrap();
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(),Some("failed"));
    let status:String=db.with_connection(|conn|conn.query_row(
        "SELECT status FROM runs WHERE id=?1",[&run],|row|row.get(0))).unwrap();
    assert_eq!(status,"failed");
    drop(host);
    drop(app);
}
