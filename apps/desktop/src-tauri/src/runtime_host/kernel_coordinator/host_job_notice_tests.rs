use super::*;
use crate::database::JobStartRequest;
use sha2::{Digest, Sha256};

fn notice_host_fixture(tag: &str) -> (Database, PathBuf, String, TestClock, CancellationRegistry, String) {
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run) = fixture_with_start_opt(&clock, tag, None, true, true);
    db.with_connection(|conn| conn.execute(
        "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
          '$.experimental_compute_job_notice',json('true')) WHERE run_id=?1", [&run],
    )).unwrap();
    freeze_host_scope(&db, &run);
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    (db, root, run, clock, cancellation, conversation)
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
