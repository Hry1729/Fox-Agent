use super::*;

#[test]
fn approved_html_write_and_streamed_answer_remain_available_after_cancel() {
    let config = worker_configuration();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    let conversation = db
        .run_control_binding(&run)
        .unwrap()
        .unwrap()
        .conversation_id;
    let html = "<!doctype html><h1>真实写入的本地页面</h1>";
    coordinator.dispatch_initial("owner",&Ask,|_,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:run.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"write-html","name":"write_file","arguments":{"path":"result.html","content":html}}]})
    })).unwrap();
    assert!(!root.join("result.html").exists());
    assert!(db
        .load_conversation(&conversation)
        .unwrap()
        .artifacts
        .is_empty());
    coordinator
        .resolve_approval("write-html", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator
        .dispatch_tool("write-html", "writer", |_, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let action =
                crate::tool_host::prepare("write_file", &payload["input"], root.to_str().unwrap())?;
            Ok((
                true,
                crate::tool_host::execute_with_cancellation(action, Some(token))?,
            ))
        })
        .unwrap();
    let first = db.load_conversation(&conversation).unwrap();
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(first.artifacts[0].media_type.as_deref(), Some("text/html"));
    let batch = coordinator
        .snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .find(|e| e.kind == kernel::OutboxEffectKind::DeliverToolBatch)
        .unwrap()
        .batch_id
        .clone()
        .unwrap();
    assert!(coordinator
        .dispatch_batch(&batch, "reply", &Allow, |_, frame, _| {
            assert!(db
                .save_kernel_model_display(&fox_engine_protocol::KernelModelPreview {
                    schema_version: 1,
                    run_id: run.clone(),
                    conversation_id: conversation.clone(),
                    turn_id: frame.turn_id.clone(),
                    checkpoint_seq: frame.checkpoint_seq,
                    revision: 1,
                    text: "页面已生成，接下来说明使用方法…".into(),
                    reasoning: String::new()
                })
                .unwrap());
            Err("cancel during streamed explanation".into())
        })
        .is_err());
    coordinator.cancel().unwrap();
    coordinator.settle_cancellation().unwrap();
    drop(coordinator);
    drop(db);
    let reopened = Database::open(root.join("facts.db"))
        .unwrap()
        .load_conversation(&conversation)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("result.html")).unwrap(),
        html
    );
    assert_eq!(reopened.artifacts.len(), 1);
    assert_eq!(reopened.artifacts[0].id, first.artifacts[0].id);
    assert!(reopened
        .messages
        .iter()
        .any(|m| m.content == "页面已生成，接下来说明使用方法…" && m.status == "interrupted"));
}

#[test]
fn received_display_survives_cancel_failure_and_database_reopen_without_settling_execution() {
    for cancel in [true, false] {
        let config = worker_configuration();
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, root, run) =
            fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
        let conversation = db
            .run_control_binding(&run)
            .unwrap()
            .unwrap()
            .conversation_id;
        let mut received = None;
        assert!(coordinator
            .dispatch_initial("display-owner", &Allow, |_, frame, _| {
                let notice = fox_engine_protocol::KernelModelPreview {
                    schema_version: 1,
                    run_id: run.clone(),
                    conversation_id: conversation.clone(),
                    turn_id: frame.input.turn_id.clone(),
                    checkpoint_seq: frame.checkpoint_seq,
                    revision: 1,
                    text: "已经输出的中文内容 😀".into(),
                    reasoning: "供应商返回的思考说明".into(),
                };
                let before = coordinator.snapshot().unwrap();
                assert!(db.save_kernel_model_display(&notice).unwrap());
                assert!(
                    !db.save_kernel_model_display(&notice).unwrap(),
                    "duplicate revision"
                );
                let mut wrong = notice.clone();
                wrong.turn_id = "foreign".into();
                wrong.revision = 2;
                assert!(!db.save_kernel_model_display(&wrong).unwrap());
                assert_eq!(
                    coordinator.snapshot().unwrap().last_event_seq,
                    before.last_event_seq
                );
                let detail = db.load_conversation(&conversation).unwrap();
                assert_eq!(
                    detail
                        .messages
                        .iter()
                        .filter(|m| m.role == "assistant")
                        .count(),
                    1
                );
                assert_eq!(detail.messages.last().unwrap().status, "streaming");
                received = Some(notice);
                Err("injected model delivery interruption".into())
            })
            .is_err());
        if cancel {
            coordinator.cancel().unwrap();
            coordinator.settle_cancellation().unwrap();
        } else {
            coordinator.fail("test.failed", "injected failure").unwrap();
        }
        let mut late = received.unwrap();
        late.revision = 2;
        late.text = "late replacement".into();
        assert!(!db.save_kernel_model_display(&late).unwrap());
        drop(coordinator);
        drop(db);
        let db = Database::open(root.join("facts.db")).unwrap();
        let detail = db.load_conversation(&conversation).unwrap();
        let message = detail
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        assert_eq!(message.content, "已经输出的中文内容 😀");
        assert_eq!(message.status, "interrupted");
        assert!(detail.runtime_events.iter().any(
            |e| e.event_type == "reasoning.delta" && e.event["delta"] == "供应商返回的思考说明"
        ));
    }
}

#[test]
fn final_response_replaces_partial_and_projects_tool_process_without_duplicate_usage() {
    let config = worker_configuration();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    let conversation = db
        .run_control_binding(&run)
        .unwrap()
        .unwrap()
        .conversation_id;
    coordinator.dispatch_initial("owner",&Allow,|_,frame,_| {
        db.save_kernel_model_display(&fox_engine_protocol::KernelModelPreview {schema_version:1,run_id:run.clone(),conversation_id:conversation.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,revision:1,text:"long provisional answer".into(),reasoning:"partial reasoning".into()}).unwrap();
        Ok(fox_engine_protocol::KernelInitialModelResponse {schema_version:1,run_id:run.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking","thinking":"final reasoning"},{"type":"text","text":"short"},{"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}],"usage":{"input":10,"output":2,"totalTokens":12}})})
    }).unwrap();
    coordinator
        .dispatch_tool("read-a", "reader", |_, _, _| {
            Ok((true, json!({"content":"read result"})))
        })
        .unwrap();
    let detail = db.load_conversation(&conversation).unwrap();
    let messages = detail
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "short");
    assert_eq!(messages[0].status, "completed");
    assert_eq!(
        detail
            .runtime_events
            .iter()
            .filter(|e| e.event_type == "reasoning.delta")
            .count(),
        1
    );
    assert!(detail
        .runtime_events
        .iter()
        .any(|e| e.event["delta"] == "final reasoning"));
    assert_eq!(
        detail
            .runtime_events
            .iter()
            .filter(|e| e.event_type == "usage.updated")
            .count(),
        1
    );
    assert!(detail
        .runtime_events
        .iter()
        .any(|e| e.event_type == "tool.started" && e.event["tool"] == "read"));
    assert!(
        detail
            .runtime_events
            .iter()
            .any(|e| e.event_type == "tool.completed"
                && e.event["result"]["content"] == "read result")
    );
}
