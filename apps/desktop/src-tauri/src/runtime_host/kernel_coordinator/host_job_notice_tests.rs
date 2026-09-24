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
fn live_batch_binds_steering_receipt_and_job_notice_in_the_same_model_input() {
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
        while requests.len()<2 {
            let mut stream=match listener.accept() {
                Ok((stream,_))=>stream,
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                    assert!(Instant::now()<deadline,"expected two local Provider requests");
                    std::thread::sleep(Duration::from_millis(10));continue;
                }
                Err(error)=>panic!("{error}"),
            };
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
            } else {json!({"role":"assistant","content":"处理完成。"})};
            let finish=if requests.len()==1 {"tool_calls"} else {"stop"};
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
    coordinator.dispatch_initial_live("steer-notice-live",&Allow,&real_worker_command(),"local-test-only",
        &|_,effect,_| {
            assert_eq!(effect.tool_call_id.as_deref(),Some("read-once"));
            Ok((true,json!({"content":[{"type":"text","text":"Host-settled read"}]})))
        },&|_|Ok(()),&|_|Ok(())).unwrap();
    let requests=server.join().unwrap();
    assert_eq!(requests.len(),2);
    let second_wire=serde_json::to_string(&requests[1]["messages"]).unwrap();
    assert_eq!(second_wire.matches("FOX_HOST_JOB_NOTICE_V1").count(),1);
    assert_eq!(second_wire.matches("用户在运行过程中补充要求").count(),1);
    assert!(second_wire.find("用户在运行过程中补充要求")<second_wire.find("FOX_HOST_JOB_NOTICE_V1"));
    let input_json:String=db.with_connection(|conn|conn.query_row(
        "SELECT input_json FROM kernel_model_notice_inputs WHERE run_id=?1
         AND dispatch_key LIKE 'tool-batch-delivery:%'",[&run],|row|row.get(0))).unwrap();
    let input:Value=serde_json::from_str(&input_json).unwrap();
    let receipt=input["modelInput"]["steering"][0]["receivedAt"].as_i64().unwrap();
    let history=input["modelHistory"].as_array().unwrap();
    let projected=history.iter().find(|message|message["content"][0]["text"]
        .as_str().is_some_and(|text|text.contains("用户在运行过程中补充要求"))).unwrap();
    assert_eq!(projected["timestamp"].as_i64(),Some(receipt));
    let state:String=db.with_connection(|conn|conn.query_row(
        "SELECT state FROM kernel_job_notice_deliveries WHERE job_id=?1",[&job],|row|row.get(0))).unwrap();
    assert_eq!(state,"acknowledged");
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
