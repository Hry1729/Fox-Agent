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
          '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1", [run],
    )).unwrap();
    assert!(db.compute_job_notice_enabled(run).unwrap(),"test must freeze the enabled wire flag");
}

fn finished_compute(db: &Database, run: &str, conversation: &str, key: &str) -> String {
    let job = db.kernel_job_start(&JobStartRequest {
        run_id: run.into(), kind: "attachment_compute".into(), idempotency_key: key.into(),
        params: json!({"input":key}), deadline_ms: Some(crate::database::now_ms()+60_000),
        progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    db.kernel_job_complete_attempt(&job,1,conversation,&json!({"result":key})).unwrap();
    assert!(db.kernel_job_notice(conversation,run,&job).unwrap().is_some(),
        "real compute settlement must create its atomic Host notice");
    job
}

fn failed_compute(db:&Database,run:&str,key:&str,code:&str)->String {
    let job=db.kernel_job_start(&JobStartRequest {run_id:run.into(),kind:"attachment_compute".into(),
        idempotency_key:key.into(),params:json!({"input":key}),
        deadline_ms:Some(crate::database::now_ms()+60_000),progress_total:None})
        .unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    db.kernel_job_settle_attempt(&job,1,crate::database::JobState::Failed,
        Some((code,"bounded error detail"))).unwrap();
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
    for fault in ["hash", "owner", "ack_trigger", "ack_ignore"] {
        let (db, _root, run, clock, cancellation, conversation)=notice_host_fixture(fault);
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
        let job=finished_compute(&db,&run,&conversation,fault);
        assert!(coordinator.dispatch_initial("owner",&Allow,|binding,frame,_| {
            db.with_connection(|conn| match fault {
                "hash" => conn.execute("UPDATE kernel_job_notice_deliveries SET input_hash=?2 WHERE job_id=?1",
                    rusqlite::params![job,format!("sha256:{}","c".repeat(64))]).map(|_|()),
                "owner" => conn.execute("UPDATE kernel_model_notice_inputs SET lease_owner='foreign' WHERE run_id=?1",
                    [&run]).map(|_|()),
                "ack_trigger" => conn.execute_batch("CREATE TRIGGER reject_b2a_ack BEFORE UPDATE OF state
                    ON kernel_job_notice_deliveries WHEN NEW.state='acknowledged'
                    BEGIN SELECT RAISE(ABORT,'injected ack fault'); END"),
                _ => conn.execute_batch("CREATE TRIGGER ignore_b2a_ack BEFORE UPDATE OF state
                    ON kernel_job_notice_deliveries WHEN NEW.state='acknowledged'
                    BEGIN SELECT RAISE(IGNORE); END"),
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
fn batch_model_lease_rejects_forged_durable_middle_in_both_transports() {
    let (db, _root, run, clock, cancellation, _conversation) = notice_host_fixture("batch-source");
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    coordinator.dispatch_initial("initial-owner",&Allow,|binding,frame,_| Ok(
        fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
                {"type":"toolCall","id":"source-read","name":"read",
                 "arguments":{"path":"proof.txt"}}]}),
        })).unwrap();
    let batch = coordinator.snapshot().unwrap().tool_calls[0].batch_id.clone();
    coordinator.dispatch_tool("source-read","tool-owner",|_,_,_|Ok((true,
        json!({"content":[{"type":"text","text":"durable result"}]})))).unwrap();
    let frame = coordinator.prepare_stored_batch_resume(&batch).unwrap();
    let command = {
        let mut controller = RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
        let effects = controller.begin_batch_model_request(&batch,clock.now_monotonic_ms(),
            clock.now_wall_ms()).unwrap();
        controller.persist_command(&effects)
    };
    let dispatch = |payload:&Value, history:&[Value], live_history:Option<&[Value]>,
                    start:usize| {
        let binding = crate::database::ModelNoticeInput {
            payload,delivered_history:history,live_history,
            checkpoint_seq:frame.checkpoint_seq,history_start:start,historical_bytes:0,
        };
        db.kernel_commit_batch_dispatch(&run,clock.now_wall_ms(),&command,&batch,
            "batch-owner",Some(&binding)).unwrap_err()
    };
    for altered in ["history","tool","steering"] {
        let mut forged = frame.clone();
        match altered {
            "history" => forged.history[0]["content"] = json!("forged earlier user text"),
            "tool" => forged.tools[0].result["content"][0]["text"] = json!("forged tool result"),
            _ => forged.steering.push(fox_engine_protocol::KernelSteeringNotice {
                message_id:"forged-steering".into(),content:"new demand".into(),
                received_at:Some(clock.now_wall_ms()),
            }),
        }
        forged.validate().unwrap();
        let payload = serde_json::to_value(&forged).unwrap();
        let (delivered,start) = bound_model_delivery_history(&run,&payload,None).unwrap();
        let error = dispatch(&payload,&delivered,None,start);
        assert!(error.contains("bound batch frame differs from durable Host sources"),
            "{altered}: {error}");
    }
    let directive = fox_engine_protocol::KernelRoundDirective {
        schema_version:1,kind:fox_engine_protocol::KernelRoundDirectiveKind::Batch,
        batch_id:Some(batch.clone()),checkpoint_seq:Some(frame.checkpoint_seq),
        preview_seq:None,tools:frame.tools.clone(),prompt:None,
        steering:Vec::new(),host_job_notices:Vec::new(),
    };
    let mut forged_history = bound_batch_live_history(&run,&frame.history,
        &frame.assistant_message,&frame.tools,&[]);
    forged_history[0]["content"] = json!("forged live history");
    let live_payload = serde_json::to_value(&directive).unwrap();
    let (delivered,start) = bound_model_delivery_history(
        &run,&live_payload,Some(&forged_history)).unwrap();
    let error = dispatch(&live_payload,&delivered,Some(&forged_history),start);
    assert!(error.contains("bound live batch differs from durable Host sources"),"{error}");
    let (bindings,leased):(i64,i64) = db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_model_notice_inputs
                  WHERE run_id=?1 AND dispatch_key=?2),
                (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1
                  AND effect_type='deliver_tool_batch' AND status='leased')",
        rusqlite::params![run,kernel::batch_delivery_idempotency_key(&batch)],
        |row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!((bindings,leased),(0,0),"a rejected frame must roll back the whole lease");
    let initial_state:String = db.with_connection(|conn|conn.query_row(
        "SELECT state FROM kernel_model_notice_inputs WHERE run_id=?1 AND dispatch_key=?2",
        rusqlite::params![run,kernel::INITIAL_MODEL_IDEMPOTENCY_KEY],|row|row.get(0))).unwrap();
    assert_eq!(initial_state,"acknowledged","the valid initial lease remains durable");
}

#[test]
fn live_continuation_lease_rejects_self_consistent_forged_history() {
    // Use the real Kernel continuation intent/outbox transaction. The review
    // request is a controlled fixture so this test targets the DB lease, not
    // the stop-review policy that decides when to request another round.
    let (db,_root,run,clock,cancellation,_conversation)=notice_host_fixture("live-cont-source");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let mut input=db.kernel_initial_input(&run).unwrap();
    input.messages.push(json!({"role":"assistant","stopReason":"stop",
        "content":[{"type":"text","text":"review me"}]}));
    input.messages.push(json!({"role":"user","content":[{"type":"text",
        "text":super::super::live::CONTINUATION_PROMPT}],"timestamp":0}));
    input.validate().unwrap();
    let encoded=serde_json::to_string(&input).unwrap();
    coordinator.apply(None,|controller,_|controller.request_continuation(
        super::super::live::CONTINUATION_PROMPT,&encoded)).unwrap();
    let cursor=coordinator.snapshot().unwrap().last_event_seq;
    let effect_key=format!("continuation:{cursor}");
    let mut controller=RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let effects=controller.begin_continuation_model_request(&effect_key,
        clock.now_monotonic_ms(),clock.now_wall_ms()).unwrap();
    let command=controller.persist_command(&effects);
    let directive=fox_engine_protocol::KernelRoundDirective {
        schema_version:1,kind:fox_engine_protocol::KernelRoundDirectiveKind::Continuation,
        batch_id:None,checkpoint_seq:None,preview_seq:Some(cursor),tools:vec![],
        prompt:Some(super::super::live::CONTINUATION_PROMPT.into()),
        steering:vec![],host_job_notices:vec![],
    };
    directive.validate().unwrap();
    let payload=serde_json::to_value(&directive).unwrap();
    let mut forged=input.messages.clone();
    forged[0]["content"]=json!("different user demand");
    let binding=crate::database::ModelNoticeInput {
        payload:&payload,delivered_history:&forged,live_history:Some(&forged),
        checkpoint_seq:cursor,history_start:forged.len(),historical_bytes:0,
    };
    let error=db.kernel_commit_continuation_model(&run,clock.now_wall_ms(),&command,
        &effect_key,"continuation-owner",false,None,Some(&binding)).unwrap_err();
    assert!(error.contains("bound live continuation differs from durable Host sources"),"{error}");
    let (bound,leased):(i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1 AND dispatch_key=?2),
                (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key=?3
                  AND status='leased')",
        rusqlite::params![run,format!("continuation-delivery:{effect_key}"),effect_key],
        |row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!((bound,leased),(0,0),"forged continuation must roll back its whole lease");
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
        let sink=|_:&fox_engine_protocol::KernelModelPreview|{};
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap().with_preview(&sink);
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
fn real_node_live_and_single_round_park_then_wake_with_one_typed_notice() {
    for live in [true,false] {
        // The provider is a bounded local HTTP fixture; the production Pi
        // worker/transport and durable model leases run unchanged.
        let (address,server)=start_http_model_fixture(vec![
            json!({"role":"assistant","content":"The compute Job is running."}),
            json!({"role":"assistant","content":"The completed result is acknowledged."}),
        ]);
        let mut config=worker_configuration();
        config.model_service=json!({"apiType":"openai-completions","modelId":"kernel-http-test",
            "baseUrl":format!("http://{address}/v1")});
        let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;
        let cancellation=CancellationRegistry::default();
        let (db,_root,run)=fixture_with_retry_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1));
        enable_notices(&db,&run);
        freeze_host_scope(&db,&run);
        let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
        let job=db.kernel_job_start(&JobStartRequest {run_id:run.clone(),kind:"attachment_compute".into(),
            idempotency_key:format!("park-{live}"),params:json!({"input":"local fixture"}),
            deadline_ms:Some(crate::database::now_ms()+60_000),progress_total:None})
            .unwrap().snapshot().job_id.clone();
        db.kernel_job_claim_attempt(&job,1).unwrap();
        let sink=|_:&fox_engine_protocol::KernelModelPreview|{};
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation)
            .unwrap().with_preview(&sink);
        if live {
            coordinator.dispatch_initial_live("park-live",&Allow,&real_worker_command(),"local-test-only",
                &|_,_,_|panic!("a parked Job must not execute a tool twice"),&|_|Ok(()),&|_|Ok(())).unwrap();
        } else {
            coordinator.dispatch_initial_with_worker("park-round",&Allow,&real_worker_command(),
                "local-test-only").unwrap();
        }
        assert_eq!(coordinator.snapshot().unwrap().state,"waiting_jobs");
        let first_count:i64=db.with_connection(|conn|conn.query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_response'",
            [&run],|row|row.get(0))).unwrap();
        assert_eq!(first_count,1);
        drop(coordinator);
        db.kernel_job_complete_attempt(&job,1,&conversation,&json!({"answer":"ready"})).unwrap();
        let coordinator=KernelCoordinator::reopen(&db,&clock,&run,&cancellation).unwrap()
            .with_preview(&sink);
        assert!(coordinator.wake_waiting_jobs_now().unwrap());
        let snapshot=coordinator.snapshot().unwrap();
        let effect=snapshot.pending_effects.iter().find(|effect|
            effect.kind==kernel::OutboxEffectKind::ContinuationModel
                && effect.status==kernel::OutboxStatus::Pending).unwrap();
        let key=effect.effect_key.clone();
        if live {
            coordinator.dispatch_continuation_live(&key,"wake-live",&Allow,&real_worker_command(),
                "local-test-only",&|_,_,_|panic!("wake must not replay a tool"),
                &|_|Ok(()),&|_|Ok(())).unwrap();
        } else {
            coordinator.dispatch_continuation_with_worker(&key,"wake-round",&Allow,
                &real_worker_command(),"local-test-only").unwrap();
        }
        let requests=server.join().unwrap();
        assert_eq!(requests.len(),2);
        let first_wire=requests[0]["messages"].to_string();
        let second_wire=requests[1]["messages"].to_string();
        assert!(!first_wire.contains("FOX_HOST_JOB_NOTICE_V1"));
        assert_eq!(second_wire.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
        assert!(second_wire.contains(&job));
        let (parks,wakes,continuations):(i64,i64,i64)=db.with_connection(|conn|conn.query_row(
            "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                    (SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='continuation_model')",
            [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!((parks,wakes,continuations),(1,1,1));
        assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state,"completed");
    }
}

#[test]
fn real_runtime_host_job_start_parks_and_wakes_on_both_pi_transports() {
    use std::time::{Duration,Instant};
    use tauri::Manager;
    let selected=std::env::var("FOX_TEST_REAL_HOST_JOB_CHAIN_CHILD").ok();
    let Some(selected)=selected else {
        // The backend's existing at-settle hook is process-wide. Each path
        // gets its own bounded test process so parallel libtests cannot hold an
        // unrelated Job at its terminal write or alter its Runtime settings.
        for mode in ["live","round"] {
            let mut child=std::process::Command::new(std::env::current_exe().unwrap())
                .arg("real_runtime_host_job_start_parks_and_wakes_on_both_pi_transports")
                .arg("--test-threads=1")
                .env("FOX_TEST_REAL_HOST_JOB_CHAIN_CHILD",mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline=Instant::now()+Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now()>=deadline {
                    let _=child.kill();let _=child.wait();
                    panic!("{mode} real Host Job chain exceeded 55 seconds");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let output=child.wait_with_output().unwrap();
            assert!(output.status.success(),"{mode} stdout: {}\n{mode} stderr: {}",
                String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    assert!(selected=="live"||selected=="round");
    let live=selected=="live";
    let arguments=json!({"idempotencyKey":"one-real-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address,server)=start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The compute Job is still running."}),
        json!({"role":"assistant","content":"The Host result is ready."}),
    ]);
    let mut config=worker_configuration();
    config.model_service=json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools=vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db,root,run)=fixture_with_retry_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1));
    enable_notices(&db,&run);
    db.freeze_kernel_host_scope(&run,&crate::database::KernelHostScope {
        schema_version:1,tool_names:["compute_job_start".into()].into_iter().collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
        knowledge_connection_hashes:Default::default(),office_tools:Default::default(),
        lifecycle_hooks:vec![],
    }).unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {window.create=false;}
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    // Construct before the Job is created. Startup reconciliation has no
    // reason to relabel a live Job as an orphan in this scenario.
    let host=crate::runtime_host::RuntimeHost::new(app.handle().clone(),db.clone(),root.clone(),
        root.join("attachments"),root.join("skills"),crate::yuxi::YuxiClient::new().unwrap());
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    let parent_token={
        let state=host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run,"job-start").unwrap()
    };
    let (at_settle_tx,at_settle_rx)=std::sync::mpsc::channel();
    let (release_tx,release_rx)=std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move||{
        at_settle_tx.send(()).unwrap();
        assert!(release_rx.recv_timeout(Duration::from_secs(25)).is_ok(),
            "bounded fixture release never arrived");
    })));
    let ownership=crate::runtime_host::kernel_host::acquire(&root,&run).unwrap();
    if live {
        host.start_kernel_run(ownership,&binding,Value::Null,Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership,&binding,Value::Null,Value::Null)
            .unwrap();
    }
    let after_start=db.kernel_host_run_state(&run).unwrap();
    if after_start.as_deref()!=Some("waiting_jobs") {
        let evidence:(String,String,String)=db.with_connection(|conn|conn.query_row(
            "SELECT COALESCE((SELECT payload_json FROM kernel_events WHERE run_id=?1
                    AND event_type='run.failed' ORDER BY seq DESC LIMIT 1),''),
                    COALESCE((SELECT group_concat(event_type,',') FROM kernel_events WHERE run_id=?1),''),
                    COALESCE((SELECT group_concat(tool_call_id||':'||state,',')
                        FROM kernel_tool_calls WHERE run_id=?1),'')",
            [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        let _=release_tx.send(());
        panic!("real Host did not park: state={after_start:?} failure={} events={} tools={}",
            evidence.0,evidence.1,evidence.2);
    }
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("real QuickJS executor did not reach its settlement point");
    let jobs=db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(),1,"one formal job_start result must create only one Job");
    let job=&jobs[0];
    assert_eq!(job.attempts,1);
    assert_eq!(job.state.as_str(),"running");
    assert!(!parent_token.is_cancelled(),"Parked Run retired the real Job parent token");
    let owned=crate::runtime_host::kernel_host::acquire(&root,&run)
        .expect("Parked Host must release the same Run OS lock");
    drop(owned);
    let formal:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
            AND json_extract(payload_json,'$.toolCallId')='job-start'",
        [&run],|row|row.get(0))).unwrap();
    assert_eq!(formal,1,"job_start has one formal tool result");
    release_tx.send(()).unwrap();
    let deadline=Instant::now()+Duration::from_secs(20);
    while !db.kernel_job_snapshot(&job.job_id).unwrap().state.is_terminal() {
        assert!(Instant::now()<deadline,"real Job did not settle after release");
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    let settled=db.kernel_job_snapshot(&job.job_id).unwrap();
    assert_eq!(settled.state.as_str(),"completed");
    let woke=if live {host.wake_kernel_waiting_run(&run)}
        else {host.wake_kernel_waiting_run_forced_round_for_test(&run)};
    assert_eq!(woke.unwrap(),true);
    let requests=server.join().unwrap();
    assert_eq!(requests.len(),3,"original tool, parked Final and resumed answer only");
    let second=requests[1]["messages"].to_string();
    let third=requests[2]["messages"].to_string();
    assert!(second.contains(&job.job_id),"the single formal result reached the model");
    assert!(!second.contains("FOX_HOST_JOB_NOTICE_V1"));
    assert_eq!(third.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
    assert!(third.contains(&job.job_id));
    let (park,wake,reply,formal):(i64,i64,i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_response'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                    AND json_extract(payload_json,'$.toolCallId')='job-start')",
        [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap();
    assert_eq!((park,wake,reply,formal),(1,1,1,1));
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state,"completed");
    drop(host);drop(app);
}

#[test]
fn real_runtime_host_job_finishes_during_second_model_lease_or_uses_legacy_review() {
    use std::time::{Duration,Instant};
    use tauri::Manager;
    let selected=std::env::var("FOX_TEST_REAL_HOST_JOB_RACE_CHILD").ok();
    let Some(selected)=selected else {
        for mode in ["live-direct","round-direct","live-off","round-off"] {
            let mut child=std::process::Command::new(std::env::current_exe().unwrap())
                .arg("real_runtime_host_job_finishes_during_second_model_lease_or_uses_legacy_review")
                .arg("--test-threads=1")
                .env("FOX_TEST_REAL_HOST_JOB_RACE_CHILD",mode)
                .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline=Instant::now()+Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now()>=deadline {
                    let _=child.kill();let _=child.wait();
                    panic!("{mode} real Host Job race exceeded 55 seconds");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let output=child.wait_with_output().unwrap();
            assert!(output.status.success(),"{mode} stdout: {}\n{mode} stderr: {}",
                String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    let live=selected.starts_with("live");
    let direct=selected.ends_with("direct");
    assert!(live||selected.starts_with("round"));
    let (seen_tx,seen_rx)=std::sync::mpsc::channel();
    let (proceed_tx,proceed_rx)=std::sync::mpsc::channel();
    let arguments=json!({"idempotencyKey":"one-real-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address,server)=start_http_model_fixture_with_response_hook(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The compute Job is still running."}),
        json!({"role":"assistant","content":"The Host result is ready."}),
    ],Some((if direct {1}else{2},Box::new(move||{
        seen_tx.send(()).unwrap();
        proceed_rx.recv_timeout(Duration::from_secs(25))
            .expect("the held provider reply was never released");
    }))));
    let mut config=worker_configuration();
    config.model_service=json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools=vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db,root,run)=fixture_with_retry_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1));
    if direct {enable_notices(&db,&run);}
    db.freeze_kernel_host_scope(&run,&crate::database::KernelHostScope {
        schema_version:1,tool_names:["compute_job_start".into()].into_iter().collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
        knowledge_connection_hashes:Default::default(),office_tools:Default::default(),
        lifecycle_hooks:vec![],
    }).unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {window.create=false;}
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let host=crate::runtime_host::RuntimeHost::new(app.handle().clone(),db.clone(),root.clone(),
        root.join("attachments"),root.join("skills"),crate::yuxi::YuxiClient::new().unwrap());
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    let parent_token={let state=host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run,"job-start").unwrap()};
    let (at_settle_tx,at_settle_rx)=std::sync::mpsc::channel();
    let (release_tx,release_rx)=std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move||{
        at_settle_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(25))
            .expect("real QuickJS Job release never arrived");
    })));
    let observed=db.clone();
    let observed_run=run.clone();
    let job_parent=parent_token.clone();
    let settle=std::thread::spawn(move||{
        seen_rx.recv_timeout(Duration::from_secs(20))
            .expect("the selected model lease did not reach local Provider");
        at_settle_rx.recv_timeout(Duration::from_secs(20))
            .expect("real QuickJS Job did not reach terminal write");
        let jobs=observed.kernel_jobs_for_run(&observed_run).unwrap();
        assert_eq!(jobs.len(),1);
        assert_eq!(jobs[0].state.as_str(),"running");
        assert!(!job_parent.is_cancelled(),"active Job lost its Run parent before settlement");
        if direct {
            let bound:i64=observed.with_connection(|conn|conn.query_row(
                "SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1 AND state='bound'",
                [&observed_run],|row|row.get(0))).unwrap();
            assert_eq!(bound,1,"the second request must be leased before Job settlement");
        }
        release_tx.send(()).unwrap();
        let deadline=Instant::now()+Duration::from_secs(20);
        while !observed.kernel_job_snapshot(&jobs[0].job_id).unwrap().state.is_terminal() {
            assert!(Instant::now()<deadline,"real Job did not settle during the held reply");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(observed.kernel_job_snapshot(&jobs[0].job_id).unwrap().state.as_str(),"completed");
        proceed_tx.send(()).unwrap();
        jobs[0].job_id.clone()
    });
    let ownership=crate::runtime_host::kernel_host::acquire(&root,&run).unwrap();
    if live {host.start_kernel_run(ownership,&binding,Value::Null,Value::Null).unwrap();}
    else {host.start_kernel_run_forced_round_for_test(ownership,&binding,Value::Null,Value::Null)
        .unwrap();}
    let job_id=settle.join().unwrap();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    let requests=server.join().unwrap();
    assert_eq!(requests.len(),3,"one formal tool call and two model replies");
    let second=requests[1]["messages"].to_string();
    let third=requests[2]["messages"].to_string();
    assert!(second.contains(&job_id));
    assert!(!second.contains("FOX_HOST_JOB_NOTICE_V1"));
    if direct {
        assert_eq!(third.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
        assert!(third.contains(&job_id));
    } else {
        assert!(!third.contains("FOX_HOST_JOB_NOTICE_V1"));
        assert!(third.contains(super::super::live::CONTINUATION_PROMPT),
            "flag-off path must preserve the original review request");
    }
    let (parks,wakes,continuations,formal):(i64,i64,i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_response'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                   AND json_extract(payload_json,'$.toolCallId')='job-start')",
        [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap();
    assert_eq!((parks,wakes,continuations,formal),(0,0,1,1));
    assert!(parent_token.is_cancelled(),"terminal Run must retire its old cancellation scope");
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state,"completed");
    drop(host);drop(app);
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

#[test]
fn live_and_single_round_batch_bind_steering_receipt_and_job_notice_in_the_same_model_input() {
    for live in [true,false] { exercise_same_round_steering_and_notice(live); }
}

fn exercise_same_round_steering_and_notice(live:bool) {
    use std::io::{Read,Write};
    use std::net::TcpListener;
    use std::time::{Duration,Instant};
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let address=listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut config=worker_configuration();
    config.model_service=json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let cancellation=CancellationRegistry::default();
    let (db,root,run)=fixture_with_retry_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1));
    enable_notices(&db,&run);freeze_host_scope(&db,&run);
    let conversation=db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=db.kernel_job_start(&JobStartRequest {run_id:run.clone(),kind:"attachment_compute".into(),
        idempotency_key:"while-model-running".into(),params:json!({"x":1}),
        deadline_ms:Some(crate::database::now_ms()+60_000),progress_total:None})
        .unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    let source=root.join("facts.db");
    let server_run=run.clone();let server_conversation=conversation.clone();let server_job=job.clone();
    let server=std::thread::spawn(move || {
        let mut requests=Vec::new();
        let deadline=Instant::now()+Duration::from_secs(30);
        let expected_requests=if live {2} else {3};
        while requests.len()<expected_requests {
            let mut stream=match listener.accept() {
                Ok((stream,_))=>stream,
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                    assert!(Instant::now()<deadline,"expected two local Provider requests");
                    std::thread::sleep(Duration::from_millis(10));continue;
                }
                Err(error)=>panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes=Vec::new();let mut buffer=[0u8;8192];
            let (header_end,length)=loop {
                let read=stream.read(&mut buffer).unwrap();assert!(read>0);
                bytes.extend_from_slice(&buffer[..read]);assert!(bytes.len()<1_048_576);
                if let Some(end)=bytes.windows(4).position(|part|part==b"\r\n\r\n") {
                    let headers=String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length:usize=headers.lines().find_map(|line|line.strip_prefix("content-length:"))
                        .unwrap().trim().parse().unwrap();
                    break (end+4,length);
                }
            };
            while bytes.len()<header_end+length {
                let read=stream.read(&mut buffer).unwrap();assert!(read>0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            requests.push(serde_json::from_slice::<Value>(&bytes[header_end..header_end+length]).unwrap());
            if requests.len()==1 {
                let second=Database::open(source.clone()).unwrap();
                second.kernel_job_complete_attempt(&server_job,1,&server_conversation,&json!({"completed":"once"})).unwrap();
                second.enqueue_run_steering(&server_run,"midrun-steering","补充要求：继续核对已完成结果。",
                    crate::database::now_ms()).unwrap();
            }
            let delta=if requests.len()==1 {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"read-once","type":"function",
                    "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]})
            } else if !live && requests.len()==2 {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"read-after-steering","type":"function",
                    "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]})
            } else {json!({"role":"assistant","content":"处理完成。"})};
            let finish=if requests.len()==1 || !live && requests.len()==2 {"tool_calls"} else {"stop"};
            let chunk=json!({"id":"b2a-steering","object":"chat.completion.chunk","created":1,
                "model":"kernel-http-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
            let done=json!({"id":"b2a-steering","object":"chat.completion.chunk","created":1,
                "model":"kernel-http-test","choices":[{"index":0,"delta":{},"finish_reason":finish}]});
            let body=format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n");
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
        requests
    });
    let sink=|_:&fox_engine_protocol::KernelModelPreview|{};
    let coordinator=coordinator.with_preview(&sink);
    if live {
        coordinator.dispatch_initial_live("steer-notice-live",&Allow,&real_worker_command(),"local-test-only",
            &|_,effect,_| {
                assert_eq!(effect.tool_call_id.as_deref(),Some("read-once"));
                Ok((true,json!({"content":[{"type":"text","text":"Host-settled read"}]})))
            },&|_|Ok(()),&|_|Ok(())).unwrap();
    } else {
        coordinator.dispatch_initial_with_worker("steer-notice-initial",&Allow,
            &real_worker_command(),"local-test-only").unwrap();
        assert_eq!(coordinator.snapshot().unwrap().tool_calls.len(),1);
        assert!(coordinator.dispatch_tool("read-once","steer-notice-read",|_,effect,_| {
            assert_eq!(effect.tool_call_id.as_deref(),Some("read-once"));
            Ok((true,json!({"content":[{"type":"text","text":"Host-settled read"}]})))
        }).unwrap());
        let batch=db.kernel_rehydrate(&run).unwrap().unwrap().tools.iter()
            .find(|tool|tool.tool_call_id=="read-once").unwrap().batch_id.clone();
        assert!(db.kernel_engine_batch_checkpoint(&run,&batch).unwrap().is_some());
        coordinator.dispatch_stored_batch_with_worker(&batch,"steer-notice-batch",&Allow,
            &real_worker_command(),"local-test-only").unwrap();
    }
    let (input_json,input_hash,dispatch_key):(String,String,String)=db.with_connection(|conn|conn.query_row(
        "SELECT input_json,input_hash,dispatch_key FROM kernel_model_notice_inputs WHERE run_id=?1
         AND dispatch_key LIKE 'tool-batch-delivery:%'",[&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
    assert_eq!(input_hash,format!("sha256:{}",hex::encode(Sha256::digest(input_json.as_bytes()))));
    let input:Value=serde_json::from_str(&input_json).unwrap();
    let receipt=input["modelInput"]["steering"][0]["receivedAt"].as_i64().unwrap();
    assert!(receipt>0);
    assert_eq!(input["modelInput"]["hostJobNotices"][0]["jobId"].as_str(),Some(job.as_str()));
    if live {
        let history=input["modelHistory"].as_array().unwrap();
        let projected=history.iter().find(|message|message["content"][0]["text"]
            .as_str().is_some_and(|text|text.contains("用户在运行过程中补充要求"))).unwrap();
        assert_eq!(projected["timestamp"].as_i64(),Some(receipt));
    } else {
        assert!(input["modelHistory"].is_null());
        assert!(input["modelInput"]["history"].is_array());
    }
    let (state,delivery_key,delivery_hash):(String,String,String)=db.with_connection(|conn|conn.query_row(
        "SELECT state,dispatch_key,input_hash FROM kernel_job_notice_deliveries WHERE job_id=?1",
        [&job],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
    assert_eq!(state,"acknowledged");
    assert_eq!(delivery_key,dispatch_key);
    assert_eq!(delivery_hash,input_hash);
    if !live {
        let second_batch=db.kernel_rehydrate(&run).unwrap().unwrap().tools.iter()
            .find(|tool|tool.tool_call_id=="read-after-steering").unwrap().batch_id.clone();
        let checkpoint=db.kernel_engine_batch_checkpoint(&run,&second_batch).unwrap().unwrap();
        let history=checkpoint["checkpoint"]["value"]["history"].as_array().unwrap();
        let durable_steering=history.iter().find(|message|message["content"][0]["text"]
            .as_str().is_some_and(|text|text.contains("用户在运行过程中补充要求"))).unwrap();
        assert_eq!(durable_steering["timestamp"].as_i64(),Some(receipt),
            "the next durable checkpoint must match the Provider's receipt timestamp");
        assert_eq!(history.iter().filter(|message|message["role"]=="hostJobNotice").count(),1);
        assert!(coordinator.dispatch_tool("read-after-steering","second-read",|_,effect,_| {
            assert_eq!(effect.tool_call_id.as_deref(),Some("read-after-steering"));
            Ok((true,json!({"content":[{"type":"text","text":"Second settled read"}]})))
        }).unwrap());
        coordinator.dispatch_stored_batch_with_worker(&second_batch,"after-steering-batch",&Allow,
            &real_worker_command(),"local-test-only").unwrap();
    }
    let requests=server.join().unwrap();
    assert_eq!(requests.len(),if live {2} else {3});
    let second_wire=serde_json::to_string(&requests[1]["messages"]).unwrap();
    assert_eq!(second_wire.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
    assert_eq!(second_wire.matches("用户在运行过程中补充要求").count(),1);
    assert!(second_wire.find("用户在运行过程中补充要求")<second_wire.find("FOX_HOST_JOB_NOTICE_V1"));
    assert_eq!(requests[1]["messages"].as_array().unwrap().iter().filter(|message|
        message["role"]=="tool" && message["tool_call_id"]=="read-once").count(),1);
    if !live {
        let third_wire=serde_json::to_string(&requests[2]["messages"]).unwrap();
        assert_eq!(third_wire.matches("FOX_HOST_JOB_NOTICE_V1").count(),1,
            "an acknowledged fact stays once in a subsequent Provider request");
    }
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
}

#[test]
fn oversized_fact_blocks_dispatch_and_many_facts_deliver_only_a_fitting_prefix() {
    let (db,_root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-large-one");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let oversized=failed_compute(&db,&run,"too-large",&"e".repeat(5_000));
    assert!(db.pending_host_job_notices(&conversation,&run,16*1024).is_err());
    assert!(coordinator.dispatch_initial("oversized",&Allow,|_,_,_|panic!("invalid fact must not reach model")).is_err());
    assert!(db.kernel_job_notice(&conversation,&run,&oversized).unwrap().is_some());
    assert_ne!(coordinator.snapshot().unwrap().state,"completed");

    let (db,_root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-prefix");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let code="e".repeat(1_600);
    for index in 0..12 { failed_compute(&db,&run,&format!("many-{index}"),&code); }
    let prefix=db.pending_host_job_notices(&conversation,&run,16*1024).unwrap();
    assert!(!prefix.is_empty() && prefix.len()<12);
    let prefix_len=prefix.len();
    assert!(coordinator.dispatch_initial("prefix",&Allow,|binding,frame,_| {
        assert_eq!(frame.host_job_notices,prefix);
        Ok(stop_response(binding,frame))
    }).is_err(),"remaining pending facts must prevent Final");
    let (bound,pending):(i64,i64)=db.with_connection(|conn|conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
          JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1 AND d.state='bound'),
          (SELECT COUNT(*) FROM kernel_job_notices n LEFT JOIN kernel_job_notice_deliveries d
            ON d.job_id=n.job_id WHERE n.run_id=?1 AND d.job_id IS NULL)",
        [&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!(bound,prefix_len as i64);
    assert_eq!(pending,12-prefix_len as i64);
    assert_ne!(coordinator.snapshot().unwrap().state,"completed");
}

#[test]
fn second_database_job_and_steering_arrival_before_lease_rechecks_the_final_frame() {
    let (db,root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-lease-race");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=db.kernel_job_start(&JobStartRequest {run_id:run.clone(),kind:"attachment_compute".into(),
        idempotency_key:"racing-compute".into(),params:json!({"x":1}),
        deadline_ms:Some(crate::database::now_ms()+60_000),progress_total:None})
        .unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job,1).unwrap();
    let (path,run_for_hook,conversation_for_hook,job_for_hook)=(root.join("facts.db"),
        run.clone(),conversation.clone(),job.clone());
    super::super::notice_lease_test_barrier::install(&run,Box::new(move || {
        let second=Database::open(path.clone()).unwrap();
        second.kernel_job_complete_attempt(&job_for_hook,1,&conversation_for_hook,
            &json!({"settled":"between frame and lease"})).unwrap();
        second.enqueue_run_steering(&run_for_hook,"late-steering","继续核对刚完成的作业。",
            crate::database::now_ms()).unwrap();
    }));
    assert!(coordinator.dispatch_initial("stale-frame",&Allow,|_,_,_| {
        panic!("stale frame must never reach Node")
    }).is_err());
    assert!(db.kernel_job_notice(&conversation,&run,&job).unwrap().is_some());
    let delivery_count:i64=db.with_connection(|conn|conn.query_row(
        "SELECT COUNT(*) FROM kernel_job_notice_deliveries WHERE job_id=?1",[&job],|row|row.get(0))).unwrap();
    assert_eq!(delivery_count,0,"stale lease must roll back the full binding");
    coordinator.dispatch_initial("fresh-frame",&Allow,|binding,frame,_| {
        assert_eq!(frame.host_job_notices.len(),1);
        assert_eq!(frame.host_job_notices[0].job_id,job);
        assert!(frame.input.messages.iter().any(|message|message["content"][0]["text"]
            .as_str().is_some_and(|text|text.contains("继续核对刚完成的作业"))));
        let (stored,hash):(String,String)=db.with_connection(|conn|conn.query_row(
            "SELECT input_json,input_hash FROM kernel_model_notice_inputs WHERE run_id=?1",
            [&run],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(hash,format!("sha256:{}",hex::encode(Sha256::digest(stored.as_bytes()))));
        let input:Value=serde_json::from_str(&stored).unwrap();
        assert_eq!(input["modelInput"],serde_json::to_value(frame).unwrap());
        Ok(stop_response(binding,frame))
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
}

#[test]
fn notice_delivery_insert_fault_rolls_back_model_lease_and_can_dispatch_once_repaired() {
    let (db,_root,run,clock,cancellation,conversation)=notice_host_fixture("b2a-bind-fault");
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let job=finished_compute(&db,&run,&conversation,"bind-once");
    db.with_connection(|conn|conn.execute_batch("CREATE TRIGGER reject_notice_delivery
        BEFORE INSERT ON kernel_job_notice_deliveries
        BEGIN SELECT RAISE(ABORT,'injected binding failure'); END")).unwrap();
    assert!(coordinator.dispatch_initial("first-owner",&Allow,|_,_,_| {
        panic!("uncommitted model lease cannot invoke Node")
    }).is_err());
    let (input_count,delivery_count,request_since,status,lease_owner):(i64,i64,Option<i64>,String,Option<String>)=
        db.with_connection(|conn|conn.query_row(
            "SELECT (SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1),
                (SELECT COUNT(*) FROM kernel_job_notice_deliveries WHERE job_id=?2),
                k.model_request_since_wall_ms,o.status,o.lease_owner
             FROM kernel_runs k JOIN kernel_effect_outbox o ON o.run_id=k.run_id
             WHERE k.run_id=?1 AND o.effect_key=?3",
            rusqlite::params![run,job,kernel::INITIAL_MODEL_EFFECT_KEY],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))).unwrap();
    assert_eq!((input_count,delivery_count),(0,0));
    assert!(request_since.is_none());
    assert_eq!(status,"pending");
    assert!(lease_owner.is_none());
    db.with_connection(|conn|conn.execute_batch("DROP TRIGGER reject_notice_delivery")).unwrap();
    coordinator.dispatch_initial("repaired-owner",&Allow,|binding,frame,_| {
        assert_eq!(frame.host_job_notices.len(),1);
        assert_eq!(frame.host_job_notices[0].job_id,job);
        Ok(stop_response(binding,frame))
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
}
