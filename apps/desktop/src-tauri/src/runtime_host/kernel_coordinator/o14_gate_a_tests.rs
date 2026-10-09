use super::*;

#[test]
fn o14_wait_notice_is_bound_to_current_dispatch_and_cancellation_keeps_pending_verdict() {
    let clock=TestClock::new(1_000);
    let cancellation=CancellationRegistry::default();
    let config=worker_configuration();
    let (db,_root,run)=fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    freeze_host_scope(&db,&run);
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    coordinator.dispatch_initial("o14-owner",&Allow,|binding,frame,_| {
        let identity=db.kernel_model_wait_identity(&run).unwrap().unwrap();
        for stale in [(identity.0-1,identity.1),(identity.0,identity.1-1)] {
            db.record_kernel_model_waiting(&run,"stale-attempt",15_000,30_000,"model_response",Some(stale)).unwrap();
        }
        db.record_kernel_model_waiting(&run,"active-attempt",15_000,30_000,"model_response",Some(identity)).unwrap();
        Ok(fox_engine_protocol::KernelInitialModelResponse{schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","content":[{"type":"text","text":"test completed"}],"stopReason":"stop"})})
    }).unwrap();
    let count:i64=db.with_connection(|conn|conn.query_row("SELECT COUNT(*) FROM run_events WHERE run_id=?1 AND event_type='run.model_waiting'",[&run],|r|r.get(0))).unwrap();
    assert_eq!(count,1,"stale attempts never become waiting events");

    let (db,_root,run)=fixture(&clock);
    let seeds=crate::runtime_host::delivery::expectations_from_task("请生成 result.csv 和 analysis_method.md");
    assert!(!seeds.is_empty());
    db.seed_delivery_checklist(&run,&seeds,1_000).unwrap();
    let coordinator=KernelCoordinator::reopen(&db,&clock,&run,&cancellation).unwrap();
    coordinator.cancel().unwrap();
    coordinator.settle_cancellation().unwrap();
    for item in db.delivery_checklist(&run).unwrap() {
        assert_eq!(item.status,"pending");
        assert_eq!(item.checked_at,None);
        let finding:Value=serde_json::from_str(item.finding.as_deref().unwrap()).unwrap();
        assert_eq!(finding["reasonCode"],"delivery.cancelled_unverified");
        assert!(finding["reason"].as_str().unwrap().contains("未完成核验"));
    }
}
