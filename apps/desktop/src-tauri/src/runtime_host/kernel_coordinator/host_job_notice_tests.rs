use super::*;
use crate::database::JobStartRequest;
use sha2::{Digest, Sha256};

fn notice_host_fixture(_tag: &str) -> (Database, PathBuf, String, TestClock, CancellationRegistry, String) {
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let config=worker_configuration();
    let (db, root, run) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    enable_notices(&db,&run);
    freeze_host_scope(&db, &run);
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    (db, root, run, clock, cancellation, conversation)
}

fn enable_notices(db:&Database,run:&str) {
    db.with_connection(|conn| conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimental_compute_job_notice',json('true')) WHERE run_id=?1", [run],
    )).unwrap();
}

fn finished_compute(db: &Database, run: &str, conversation: &str, key: &str) -> String {
    let job = db.kernel_job_start(&JobStartRequest {
        run_id: run.into(), kind: "attachment_compute".into(), idempotency_key: key.into(),
        params: json!({"input":key}), deadline_ms: Some(crate::database::now_ms()+60_000),
        progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    db.kernel_job_complete_attempt(&job,1,conversation,&json!({"result":key})).unwrap();
    job
}

fn stop_response(binding: &RunControlBinding, frame: &fox_engine_protocol::KernelInitialModelFrame)
    -> fox_engine_protocol::KernelInitialModelResponse {
    fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"The Host job has completed."}]}),
    }
}

#[test]
fn experimental_host_initial_lease_binds_complete_input_and_acks_with_response() {
    let (db, root, run, clock, cancellation, conversation) = notice_host_fixture("b2a-success");
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job = finished_compute(&db,&run,&conversation,"first");
    let durable = db.kernel_job_notice(&conversation,&run,&job).unwrap().unwrap();
    let expected = db.pending_host_job_notices(&conversation,&run,16*1024).unwrap().remove(0);
    coordinator.dispatch_initial("owner",&Allow,|binding,frame,_| {
        assert_eq!(frame.host_job_notices,vec![expected.clone()]);
        let (input_hash,input_json,state,position): (String,String,String,i64) = db.with_connection(|conn| {
            conn.query_row("SELECT m.input_hash,m.input_json,d.state,d.history_position
              FROM kernel_model_notice_inputs m JOIN kernel_job_notice_deliveries d
                ON d.dispatch_key=m.dispatch_key AND d.job_id=?1
              WHERE m.run_id=?2",rusqlite::params![job,run],
              |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))
        }).unwrap();
        assert_eq!(state,"bound");
        assert_eq!(input_hash,format!("sha256:{}",hex::encode(Sha256::digest(input_json.as_bytes()))));
        let input: Value=serde_json::from_str(&input_json).unwrap();
        assert_eq!(input["modelInput"],serde_json::to_value(frame).unwrap());
        assert_eq!(input["checkpointSeq"],frame.checkpoint_seq);
        assert_eq!(position,frame.input.messages.len() as i64);
        Ok(stop_response(binding,frame))
    }).unwrap();
    let (delivery,input_state,assistant_count): (String,String,i64)=db.with_connection(|conn| {
        conn.query_row("SELECT d.state,m.state,
          (SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant')
          FROM kernel_job_notice_deliveries d JOIN kernel_model_notice_inputs m
            ON m.dispatch_key=d.dispatch_key WHERE d.job_id=?2",
          rusqlite::params![run,job],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))
    }).unwrap();
    assert_eq!((delivery.as_str(),input_state.as_str()),("acknowledged","acknowledged"));
    assert_eq!(assistant_count,1);
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state,"completed");
    // A legal compacted view can remove older user text before the marker;
    // the ledger checks its original bound position and stable dispatch order,
    // not the marker's new index in the compacted model view.
    let compacted=vec![expected.history_marker()];
    db.validate_host_job_notice_history(&conversation,&run,&compacted).unwrap();
    assert!(db.validate_host_job_notice_history(&conversation,&run,
        &[expected.history_marker(),expected.history_marker()]).is_err());
    let mut forged=expected.clone(); forged.result_sha256=Some(format!("sha256:{}","f".repeat(64)));
    assert!(db.validate_host_job_notice_history(&conversation,&run,&[forged.history_marker()]).is_err());
    assert!(db.validate_host_job_notice_history(&conversation,"other-run",&compacted).is_err());
    drop(coordinator); drop(db);
    let reopened=Database::open(root.join("facts.db")).unwrap();
    assert_eq!(reopened.kernel_job_notice(&conversation,&run,&job).unwrap().unwrap(),durable);
}

#[test]
fn experimental_host_notice_response_faults_leave_lease_bound_and_no_reply() {
    for fault in ["hash", "owner", "ack_trigger"] {
        let (db, _root, run, clock, cancellation, conversation)=notice_host_fixture(fault);
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
        let job=finished_compute(&db,&run,&conversation,fault);
        assert!(coordinator.dispatch_initial("owner",&Allow,|binding,frame,_| {
            db.with_connection(|conn| match fault {
                "hash" => conn.execute("UPDATE kernel_job_notice_deliveries SET input_hash=?2 WHERE job_id=?1",
                    rusqlite::params![job,format!("sha256:{}","c".repeat(64))]).map(|_|()),
                "owner" => conn.execute("UPDATE kernel_model_notice_inputs SET lease_owner='foreign' WHERE run_id=?1",
                    [&run]).map(|_|()),
                _ => conn.execute_batch("CREATE TRIGGER reject_b2a_ack BEFORE UPDATE OF state
                    ON kernel_job_notice_deliveries WHEN NEW.state='acknowledged'
                    BEGIN SELECT RAISE(ABORT,'injected ack fault'); END"),
            }).unwrap();
            Ok(stop_response(binding,frame))
        }).is_err(),"{fault} must reject response atomically");
        let (delivery,assistant_count): (String,i64)=db.with_connection(|conn| conn.query_row(
            "SELECT d.state,(SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant')
             FROM kernel_job_notice_deliveries d WHERE d.job_id=?2",
            rusqlite::params![run,job],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(delivery,"bound");
        assert_eq!(assistant_count,0);
        assert!(coordinator.dispatch_initial("new-owner",&Allow,|_,_,_|panic!("uncertain bound input must not replay")).is_err());
    }
}

#[test]
fn copied_database_cannot_hide_foreign_root_notice_from_host_final_guard() {
    let (db,root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-copy");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=finished_compute(&db,&run,&conversation,"pending-copy");
    drop(coordinator); drop(db);
    let copied=root.join("other-root");
    std::fs::create_dir_all(&copied).unwrap();
    std::fs::copy(root.join("facts.db"),copied.join("facts.db")).unwrap();
    let opened=Database::open(copied.join("facts.db")).unwrap();
    assert!(opened.pending_host_job_notices(&conversation,&run,16*1024).is_err());
    assert!(opened.kernel_job_notice(&conversation,&run,&job).unwrap().is_none());
    let coordinator=KernelCoordinator::reopen(&opened,&clock,&run,&cancellation).unwrap();
    let error=coordinator.apply(None,|controller,_|Ok(controller.terminate(kernel::RunOutcome::Completed)))
        .unwrap_err();
    assert!(error.contains("kernel.jobs_pending"),"{error}");
    assert_ne!(opened.kernel_build_full_snapshot(&run).unwrap().state,"completed");
}

#[test]
fn acknowledged_notice_survives_real_compaction_then_next_host_batch_notice() {
    // Two model leases share one fixed clock millisecond. Compaction moves the
    // first notice left, so its original absolute position can exceed the
    // second notice's new position; durable dispatch sequence orders them.
    let clock=TestClock::new(1_000);
    let cancellation=CancellationRegistry::default();
    let mut config=worker_configuration();
    config.model_service["contextWindow"]=json!(8192);
    config.model_service["maxOutputTokens"]=json!(512);
    let (db,root,run)=fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),false,true);
    let mut input=initial_input(&run,&config.hash().unwrap());
    input.messages.extend((0..12).map(|index|json!({"role":if index%2==0 {"user"} else {"assistant"},
        "content":format!("old-{index}: {}","bounded earlier discussion ".repeat(38))})));
    input.messages.extend((0..8).map(|index|json!({"role":"user",
        "content":format!("recent-{index}: preserve exactly")})));
    db.freeze_kernel_initial_input(&input).unwrap();
    enable_notices(&db,&run);
    freeze_host_scope(&db,&run);
    let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let first=finished_compute(&db,&run,&conversation,"before-compaction");
    coordinator.dispatch_initial("first-owner",&Allow,|binding,frame,_| {
        assert_eq!(frame.host_job_notices.len(),1);
        assert_eq!(frame.host_job_notices[0].job_id,first);
        Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
                {"type":"toolCall","id":"read-after-notice","name":"read","arguments":{"path":"proof.txt"}}]}),
        })
    }).unwrap();
    let batch=coordinator.snapshot().unwrap().tool_calls[0].batch_id.clone();
    coordinator.dispatch_tool("read-after-notice","read-owner",|_,_,_|Ok((true,
        json!({"content":[{"type":"text","text":std::fs::read_to_string(root.join("proof.txt")).unwrap()}]})))).unwrap();
    let before=coordinator.prepare_stored_batch_resume(&batch).unwrap();
    let old_position=before.history.iter().position(|item|item["role"]=="hostJobNotice").unwrap();
    assert!(coordinator.prepare_context_if_needed(&batch,4096).unwrap());
    coordinator.dispatch_pending_compaction("summary-owner",|_,request,_,_|Ok(
        fox_engine_protocol::KernelCompactionResponse {
            schema_version:1,run_id:request.run_id.clone(),turn_id:request.turn_id.clone(),
            compaction_id:request.compaction_id.clone(),input_hash:request.input_hash.clone(),
            summary:"Earlier discussion is context only; retain Host facts.".into(),
            usage:json!({"input":100,"output":20,"totalTokens":120}),
        })).unwrap();
    let after=coordinator.prepare_stored_batch_resume(&batch).unwrap();
    let shifted=after.history.iter().position(|item|item["role"]=="hostJobNotice").unwrap();
    assert!(shifted<old_position,"real compaction must move the durable typed marker left");
    assert_eq!(after.history.iter().filter(|item|item["role"]=="hostJobNotice").count(),1);
    let second=finished_compute(&db,&run,&conversation,"after-compaction");
    coordinator.dispatch_batch(&batch,"second-owner",&Allow,|binding,frame,_| {
        assert_eq!(frame.history.iter().filter(|item|item["role"]=="hostJobNotice").count(),1);
        assert_eq!(frame.host_job_notices.len(),1);
        assert_eq!(frame.host_job_notices[0].job_id,second);
        Ok(fox_engine_protocol::KernelModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),
            batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"stop",
                "content":[{"type":"text","text":"Both Host jobs completed."}]}),
        })
    }).unwrap();
    let acknowledged:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_job_notice_deliveries WHERE job_id IN (?1,?2)
         AND state='acknowledged'",rusqlite::params![first,second],|row|row.get(0))).unwrap();
    assert_eq!(acknowledged,2);
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
}

#[test]
fn real_host_to_local_provider_delivers_notice_once_in_live_and_single_round() {
    for live in [false,true] {
        let (address,server)=start_http_model_fixture(vec![json!({"role":"assistant",
            "content":"Observed the Host terminal fact."})]);
        let mut config=worker_configuration();
        config.model_service=json!({"apiType":"openai-completions","modelId":"kernel-http-test",
            "baseUrl":format!("http://{address}/v1")});
        let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;
        let cancellation=CancellationRegistry::default();
        let (db,root,run)=fixture_with_retry_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1));
        enable_notices(&db,&run);
        freeze_host_scope(&db,&run);
        let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
        let job=finished_compute(&db,&run,&conversation,if live {"live"} else {"single"});
        if live {
            coordinator.dispatch_initial_live("real-live",&Allow,&real_worker_command(),"local-test-only",
                &|_,_,_|panic!("Host job completion must not replay a tool"),&|_|Ok(()),&|_|Ok(())).unwrap();
        } else {
            coordinator.dispatch_initial_with_worker("real-single",&Allow,&real_worker_command(),"local-test-only").unwrap();
        }
        let sent=server.join().unwrap();
        assert_eq!(sent.len(),1);
        let wire=serde_json::to_string(&sent[0]["messages"]).unwrap();
        assert_eq!(wire.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
        assert!(wire.contains(&job));
        let (state,user_notices):(String,i64)=db.with_connection(|conn|conn.query_row(
            "SELECT d.state,(SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='user'
             AND content LIKE '%FOX_HOST_JOB_NOTICE_V1%')
             FROM kernel_job_notice_deliveries d WHERE d.job_id=?2",
            rusqlite::params![run,job],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(state,"acknowledged");
        assert_eq!(user_notices,0,"typed Host fact must not become a stored user intent");
        assert_eq!(coordinator.snapshot().unwrap().state,"completed");
        drop(coordinator);drop(db);
        let reopened=Database::open(root.join("facts.db")).unwrap();
        assert_eq!(reopened.kernel_build_full_snapshot(&run).unwrap().state,"completed");
    }
}

#[test]
fn unknown_model_response_reopens_without_replaying_bound_notice_or_job() {
    let (db,root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-unknown");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=finished_compute(&db,&run,&conversation,"one-effect");
    let original=db.kernel_job_notice(&conversation,&run,&job).unwrap().unwrap();
    let calls=AtomicUsize::new(0);
    assert!(coordinator.dispatch_initial("first-owner",&Allow,|_,frame,_| {
        calls.fetch_add(1,Ordering::SeqCst);
        assert_eq!(frame.host_job_notices.len(),1);
        Err("model response disappeared after dispatch".into())
    }).is_err());
    assert_eq!(calls.load(Ordering::SeqCst),1);
    let state:String=db.with_connection(|conn|conn.query_row(
        "SELECT state FROM kernel_job_notice_deliveries WHERE job_id=?1",[&job],|row|row.get(0))).unwrap();
    assert_eq!(state,"bound");
    drop(coordinator);drop(db);
    for _ in 0..2 {
        let reopened=Database::open(root.join("facts.db")).unwrap();
        let coordinator=KernelCoordinator::reopen(&reopened,&clock,&run,&cancellation).unwrap();
        assert!(coordinator.dispatch_initial("replacement-owner",&Allow,|_,_,_| {
            panic!("uncertain provider request must never be called twice")
        }).is_err());
        assert_eq!(reopened.kernel_job_notice(&conversation,&run,&job).unwrap().unwrap(),original);
    }
}
