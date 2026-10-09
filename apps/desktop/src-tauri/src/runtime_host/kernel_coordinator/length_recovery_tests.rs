//! The one controlled length-truncation recovery.
//!
//! Coverage: the trigger evidence, the three observable body shapes, the
//! parameter negotiation (only verified reasoning levers, never a shrunk output
//! budget, never a silent model switch), the state-derived instruction, the
//! single-use durable accounting, restart / effect-key / new-batch independence,
//! cancellation, no tool replay, and the honest ending when the recovery fails.
use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The 2026-10-07 incident shape: thinking produced, no public body, provider
/// stop reason `length`, and therefore no acceptable body or tool proposal.
fn length_truncation_error(
    turn_id: &str,
    run_id: &str,
    checkpoint_seq: u64,
    diagnostic_extra: Value,
) -> String {
    let mut diagnostic = json!({
        "reason": "empty_final_answer",
        "stopReason": "length",
        "textChars": 0,
        "thinkingChars": 27_000,
        "toolCallCount": 0,
        "markerSeen": false,
        "completionRequired": false,
    });
    for (key, value) in diagnostic_extra.as_object().unwrap() {
        diagnostic[key] = value.clone();
    }
    format!("kernel.settled_model_failure:{}", json!({
        "schemaVersion": 1, "runId": run_id, "turnId": turn_id,
        "checkpointSeq": checkpoint_seq, "category": "incomplete_response",
        "httpStatus": null, "retryAfterMs": null, "diagnostic": diagnostic,
    }))
}

fn accepted_answer(binding: &RunControlBinding, frame: &fox_engine_protocol::KernelBatchResumeFrame) -> fox_engine_protocol::KernelModelResponse {
    fox_engine_protocol::KernelModelResponse {
        schema_version: 1,
        run_id: binding.run_id.clone(),
        turn_id: frame.turn_id.clone(),
        batch_id: frame.batch_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role": "assistant", "stopReason": "stop",
            "content": [{"type": "text", "text": "已核对现有结果并给出简短答复。"}]}),
    }
}

/// One accepted response that proposes a brand-new tool call, which is how a Run
/// opens a new batch and therefore a new model effect key.
fn proposing_answer(binding: &RunControlBinding, frame: &fox_engine_protocol::KernelBatchResumeFrame) -> fox_engine_protocol::KernelModelResponse {
    fox_engine_protocol::KernelModelResponse {
        schema_version: 1,
        run_id: binding.run_id.clone(),
        turn_id: frame.turn_id.clone(),
        batch_id: frame.batch_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role": "assistant", "stopReason": "toolUse", "content": [
            {"type": "toolCall", "id": "recovery-read", "name": "read",
             "arguments": {"path": "proof.txt"}}]}),
    }
}

fn retry_events_for(root: &std::path::Path, run_id: &str) -> Vec<Value> {
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let mut query = connection
        .prepare("SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.retrying' ORDER BY seq")
        .unwrap();
    let rows = query.query_map([run_id], |row| row.get::<_, String>(0)).unwrap();
    rows.map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap()).collect()
}

fn terminal_failure_for(root: &std::path::Path, run_id: &str) -> (String, String) {
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection
        .query_row(
            "SELECT json_extract(payload_json,'$.code'), json_extract(payload_json,'$.message')
             FROM kernel_events WHERE run_id=?1 AND event_type='run.failed'",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn durable_record(db: &Database, run_id: &str) -> Option<Value> {
    db.kernel_rehydrate(run_id)
        .unwrap()
        .unwrap()
        .retry
        .length_recovery_json
        .as_deref()
        .map(|record| serde_json::from_str(record).unwrap())
}

/// The model service a reasoning-capable provider is frozen with.
fn reasoning_service() -> Value {
    json!({"apiType":"openai-completions","modelId":"lens-reasoner","baseUrl":"http://127.0.0.1:9/v1",
        "reasoning": true, "thinkingLevel": "high", "maxOutputTokens": 8192})
}

fn config_with_service(service: Value) -> crate::kernel_model_config::KernelModelConfig {
    let mut config = worker_configuration();
    config.model_service = service;
    config
}

/// The adapter's own request-boundary record for one completed attempt, exactly
/// as the runtime writes it (`usage.request` with a `config.sent` layer). This is
/// the *only* accepted effectiveness signal in these tests.
fn record_attempt_wire(
    db: &Database,
    run_id: &str,
    attempt: &str,
    sent: Value,
) {
    let record = json!({
        "schemaVersion": "usage-v1",
        "eventId": format!("event-{attempt}"),
        "requestId": format!("request-{attempt}"),
        "attemptId": attempt,
        "runId": run_id,
        "provider": "fixture",
        "modelId": "fixture",
        "stage": "agent",
        "outcome": "failure",
        "usage": {"input": null, "output": null, "cacheRead": null, "cacheWrite": null, "completeness": "unavailable"},
        "cost": {"knownCost": null, "costComplete": false},
        "config": {"raw": null, "resolved": null, "sent": sent},
    });
    db.record_model_usage(run_id, &record).expect("the wire record is durable");
}

/// A wire record that proves the transport serializes the reasoning effort.
fn wire_with_effort(effort: &str) -> Value {
    json!({"model": "lens-reasoner", "max_completion_tokens": 8192, "reasoning_effort": effort, "stream": true})
}

const EVIDENCE_SOURCE: &str = "durable usage.request config.sent layer of this Run's previous model attempt";

// ---------------------------------------------------------------------------
// Parameter negotiation (requirement 5/6)
// ---------------------------------------------------------------------------

#[test]
fn a_parameter_is_effective_only_when_the_adapters_own_wire_record_shows_it() {
    let service = reasoning_service();
    // 1. The adapter really serialized reasoning_effort for this Run, and the
    //    lowered level changes that value: the parameter is a proven lever.
    let effective = plan_length_recovery_lever(
        &service,
        AdapterReasoningEvidence::from_sent(Some(wire_with_effort("high"))),
    );
    assert!(effective.effective);
    assert_eq!(effective.intended, json!({"thinkingLevel": "medium"}));
    assert_eq!(effective.applied, json!({"thinkingLevel": "medium"}));
    assert_eq!(effective.expected_sent, json!({"reasoning_effort": "medium"}));
    assert_eq!(effective.evidence.serialized_effort.as_deref(), Some("high"));
    assert_eq!(effective.lever(), "parameters");

    // 2. The adapter's sent layer carried no reasoning field at all: lowering the
    //    local level cannot be shown to change the request, so it is NOT applied.
    let ineffective = plan_length_recovery_lever(
        &service,
        AdapterReasoningEvidence::from_sent(Some(json!({"model": "lens-reasoner", "max_tokens": 8192}))),
    );
    assert!(!ineffective.effective);
    assert_eq!(ineffective.intended, json!({"thinkingLevel": "medium"}), "the Host did intend it");
    assert_eq!(ineffective.applied, json!({}), "but nothing is applied to the request");
    assert_eq!(ineffective.expected_sent, json!({}));
    assert_eq!(ineffective.lever(), "instruction_only");

    // 3. No wire record at all: effectiveness is unproven, never assumed.
    let unproven = plan_length_recovery_lever(&service, AdapterReasoningEvidence::from_sent(None));
    assert!(!unproven.effective);
    assert_eq!(unproven.applied, json!({}));
    assert!(unproven.evidence.observed_sent.is_none());

    // 4. The value already serialized equals the target level: no observable change.
    let same = plan_length_recovery_lever(
        &service,
        AdapterReasoningEvidence::from_sent(Some(wire_with_effort("medium"))),
    );
    assert!(!same.effective, "an unchanged wire value is not an effective change");
    assert_eq!(same.applied, json!({}));

    // 5. The resolved layer is not evidence, by construction: the signal only ever
    //    comes from `sent`, and a record whose `sent` has no reasoning field stays
    //    ineffective no matter what a resolved layer would have said.
    assert_eq!(AdapterReasoningEvidence::SOURCE, EVIDENCE_SOURCE);

    // Never the output budget, never the model identity, never a timeout field.
    let (derived, changed) = apply_recovery_parameters(&service, &effective.applied).unwrap();
    assert_eq!(changed, vec!["thinkingLevel".to_owned()]);
    assert!(effective.applied.get("maxOutputTokens").is_none());
    assert!(effective.applied.get("modelId").is_none());
    assert!(effective.applied.get("baseUrl").is_none());
    assert!(effective.applied.get("apiType").is_none());
    assert_eq!(derived["modelId"], service["modelId"], "no silent model switch");
    assert_eq!(derived["maxOutputTokens"], service["maxOutputTokens"], "budget untouched");
    assert_eq!(derived["apiType"], service["apiType"]);
    assert_eq!(derived["thinkingLevel"], json!("medium"));

    // The level list is only ever descended, and never below the portable floor.
    assert_eq!(lower_thinking_level(Some("max")), Some("xhigh"));
    assert_eq!(lower_thinking_level(Some("high")), Some("medium"));
    assert_eq!(lower_thinking_level(Some("medium")), Some("low"));
    assert_eq!(lower_thinking_level(Some("low")), None, "low is the portable floor");
    assert_eq!(lower_thinking_level(Some("off")), None, "nothing below off");
    assert_eq!(lower_thinking_level(None), Some("low"), "heuristic default is medium");
    assert_eq!(lower_thinking_level(Some("ultra")), None, "unknown level is not invented");
}

#[test]
fn a_model_without_supported_reasoning_parameters_falls_back_to_the_instruction() {
    // Even with a wire record that shows an effort, a frozen configuration that
    // disables reasoning (or is already at the portable floor) has no local
    // lowering to offer.
    for service in [
        json!({"apiType":"openai-completions","modelId":"plain","baseUrl":"http://127.0.0.1:9/v1","reasoning": false}),
        json!({"apiType":"openai-completions","modelId":"plain","baseUrl":"http://127.0.0.1:9/v1","thinkingLevel": "off"}),
        json!({"apiType":"openai-completions","modelId":"plain","baseUrl":"http://127.0.0.1:9/v1","reasoning": true, "thinkingLevel": "low"}),
        json!({"apiType":"openai-completions","modelId":"plain","baseUrl":"http://127.0.0.1:9/v1","modelProfile": {"reasoning": false}}),
    ] {
        let lever = plan_length_recovery_lever(
            &service,
            AdapterReasoningEvidence::from_sent(Some(wire_with_effort("high"))),
        );
        assert!(!lever.effective, "{service}");
        assert_eq!(lever.intended, json!({}), "{service}");
        assert_eq!(lever.applied, json!({}), "no parameter may be invented for this model");
        let (derived, changed) = apply_recovery_parameters(&service, &lever.applied).unwrap();
        assert_eq!(derived, service, "the request keeps its frozen parameters");
        assert!(changed.is_empty());
    }
}

#[test]
fn the_derived_configuration_changes_only_the_recorded_parameters() {
    let frozen = config_with_service(reasoning_service());
    let record = json!({
        "recoveryParameters": {"thinkingLevel": "medium"},
        "parameterEffect": {
            "effective": true,
            "lever": "parameters",
            "locallyIntendedParameters": {"thinkingLevel": "medium"},
        },
        "originalParameters": {"reasoning": true, "thinkingLevel": "high", "maxOutputTokens": 8192},
    });
    let derived = derive_length_recovery_config(&frozen, &record).unwrap();
    assert_eq!(derived.model_service["thinkingLevel"], json!("medium"));
    assert_eq!(derived.model_service["modelId"], frozen.model_service["modelId"]);
    assert_eq!(derived.system_prompt, frozen.system_prompt);
    assert_eq!(derived.proposal_tools, frozen.proposal_tools);
    assert_ne!(derived.hash().unwrap(), frozen.hash().unwrap(), "the request really differs");
    assert!(derived.hash().unwrap().starts_with("sha256:"));

    // A record that does not match what the frozen configuration authorizes is
    // refused instead of being trusted.
    let tampered = json!({"recoveryParameters": {"thinkingLevel": "off"}});
    assert!(derive_length_recovery_config(&frozen, &tampered).is_err());
    let smuggled = json!({"recoveryParameters": {"modelId": "another-model"}});
    assert!(derive_length_recovery_config(&frozen, &smuggled).is_err());
    let missing = json!({});
    assert!(derive_length_recovery_config(&frozen, &missing).is_err());
}

#[test]
fn the_record_keeps_observed_evidence_identity_and_all_three_parameter_layers() {
    let clock = TestClock::new(5_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    // The adapter's own record of the truncated attempt: it really serialized
    // reasoning_effort, which is what makes the parameter lever effective.
    record_attempt_wire(&db, &run_id, "0", wire_with_effort("high"));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    // A round whose worker observed the stop reason but no text counter at all.
    coordinator
        .dispatch_batch("worker-batch", "truncated", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                json!({})))
        })
        .unwrap();
    let record = durable_record(&db, &run_id).expect("the recovery is recorded durably");
    assert_eq!(record["schemaVersion"], 1);
    assert_eq!(record["trigger"], "length_truncated_no_body");
    assert_eq!(record["evidence"]["form"], "stop_reason_length");
    assert_eq!(record["evidence"]["stopReason"], "length");
    assert_eq!(record["evidence"]["reason"], "empty_final_answer");
    assert_eq!(record["evidence"]["textChars"], 0);
    assert_eq!(record["evidence"]["thinkingChars"], 27_000);
    assert_eq!(record["evidence"]["truncatedShape"], "length_truncated_no_body");
    // Layer 1 — what the Host intended locally.
    assert_eq!(
        record["parameterEffect"]["locallyIntendedParameters"],
        json!({"thinkingLevel": "medium"})
    );
    // Layer 2 — the wire fact the decision rests on: the adapter's sent layer.
    assert_eq!(record["parameterEffect"]["effective"], true);
    assert_eq!(record["parameterEffect"]["lever"], "parameters");
    assert_eq!(record["parameterEffect"]["signalSource"], EVIDENCE_SOURCE);
    assert_eq!(record["parameterEffect"]["observedSentParameters"]["reasoning_effort"], "high");
    assert_eq!(record["parameterEffect"]["observedSentParameters"]["max_completion_tokens"], 8192);
    assert_eq!(record["parameterEffect"]["observedReasoningEffort"], "high");
    assert_eq!(record["parameterEffect"]["expectedSentParameters"], json!({"reasoning_effort": "medium"}));
    assert_eq!(record["recoveryParameters"], json!({"thinkingLevel": "medium"}));
    assert_eq!(record["changedServiceFields"], json!(["thinkingLevel"]));
    assert_eq!(record["parametersMode"], "parameters");
    // Layer 3 — the instruction was really delivered to the model view.
    assert_eq!(record["instructionDelivered"], true);
    assert_eq!(record["instructionDelivery"], "model_view_message");
    // Original parameters kept for cross-checking, unchanged.
    assert_eq!(record["originalParameters"]["thinkingLevel"], "high");
    assert_eq!(record["originalParameters"]["maxOutputTokens"], 8192);
    // The request identity of the dispatch that will carry the recovery.
    assert_eq!(record["effectKey"], "deliver-batch:worker-batch");
    assert_eq!(record["owner"], "truncated");
    assert_eq!(record["runId"], run_id);
    assert!(record["turnId"].as_str().is_some());
    assert!(record["checkpointSeq"].as_u64().is_some());
    assert!(record["recoveryId"].as_str().is_some());
    assert_eq!(record["appliedAtWallMs"], Value::Null, "not applied before the request is armed");
    assert_eq!(record["instructionIsSoftConstraint"], true);
    assert!(record["instruction"].as_str().unwrap().contains("Fox 长度恢复提示"));
    // Schedule and due time are durable and consistent.
    let scheduled = record["scheduledAtWallMs"].as_i64().unwrap();
    let due = record["dueWallMs"].as_i64().unwrap();
    assert_eq!(due - scheduled, MODEL_LENGTH_RECOVERY_INTERVAL_MS);
    let retry = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert_eq!(retry.length_recovery_attempts, 1);
    assert_eq!(retry.model_attempts, 1, "the recovery spends the unified budget too");
}

#[test]
fn unobserved_facts_stay_absent_instead_of_becoming_zero_or_false() {
    let clock = TestClock::new(5_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, _root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    // The worker reported only the stop reason: no text/tool/marker counters.
    coordinator
        .dispatch_batch("worker-batch", "sparse", &Allow, |binding, frame, _| {
            Err(format!("kernel.settled_model_failure:{}", json!({
                "schemaVersion": 1, "runId": binding.run_id, "turnId": frame.turn_id,
                "checkpointSeq": frame.checkpoint_seq, "category": "incomplete_response",
                "httpStatus": null, "retryAfterMs": null,
                "diagnostic": {"stopReason": "length"},
            })))
        })
        .unwrap();
    let record = durable_record(&db, &run_id).expect("the recovery is recorded");
    assert_eq!(record["evidence"]["stopReason"], "length");
    for absent in ["textChars", "thinkingChars", "toolCallCount", "completionRequired", "reason"] {
        assert!(
            record["evidence"].get(absent).is_none(),
            "{absent} was never observed and must stay absent, not become 0/false"
        );
    }
    assert_eq!(record["evidence"]["truncatedShape"], "length_truncated_unknown_body");
}

// ---------------------------------------------------------------------------
// The instruction is generated from the Run's own delivery state
// ---------------------------------------------------------------------------

fn delivery(kind: LengthRecoveryDeliveryKind, outstanding: Vec<&str>) -> LengthRecoveryDelivery {
    let verified = matches!(kind, LengthRecoveryDeliveryKind::VerifiedDeliverables);
    let incomplete = matches!(kind, LengthRecoveryDeliveryKind::IncompleteDeliverables);
    LengthRecoveryDelivery {
        kind,
        passed: if verified { 2 } else { 1 },
        failed: if incomplete { 1 } else { 0 },
        pending: 1,
        outstanding: outstanding.into_iter().map(str::to_owned).collect(),
    }
}

#[test]
fn the_instruction_follows_the_real_delivery_state_and_never_invents_completion() {
    // Verified deliverables: ask only for the short closing note.
    let verified = delivery(LengthRecoveryDeliveryKind::VerifiedDeliverables, vec![]).instruction();
    assert!(verified.contains("全部通过"));
    assert!(verified.contains("不要重复读取、计算或写入"));
    assert!(!verified.contains("任务还未完成"));
    assert!(!verified.contains("尚未通过核验"));

    // Artifacts exist but their content is unverified: never say the task is done.
    let unverified =
        delivery(LengthRecoveryDeliveryKind::UnverifiedArtifacts, vec!["汇总表.xlsx"]).instruction();
    assert!(unverified.contains("任务还未完成"));
    assert!(unverified.contains("尚未通过核验"));
    assert!(unverified.contains("汇总表.xlsx"), "the remaining verification item is named");
    assert!(!unverified.contains("全部通过"));

    // A promise is missing or failed: keep the remaining work and the blocker.
    let incomplete =
        delivery(LengthRecoveryDeliveryKind::IncompleteDeliverables, vec!["汇总表.xlsx"]).instruction();
    assert!(incomplete.contains("任务尚未完成"));
    assert!(incomplete.contains("未通过"));
    assert!(incomplete.contains("汇总表.xlsx"));
    assert!(!incomplete.contains("全部通过"));

    // No checklist at all: the ledger cannot decide, so completion is unknown.
    let none = delivery(LengthRecoveryDeliveryKind::NoChecklist, vec![]).instruction();
    assert!(none.contains("无法确认任务是否完成"));
    assert!(!none.contains("全部通过"));

    for text in [verified, unverified, incomplete, none] {
        assert!(text.contains("Fox 长度恢复提示"), "the instruction is a Host retry notice");
        assert!(text.contains("系统提示中的最终答复格式"), "it defers the answer format to the prompt");
        assert!(text.len() <= LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES);
    }
}

#[test]
fn the_instruction_is_bounded_and_sanitized_from_host_item_names() {
    let long = "「".to_owned() + &"很长的交付物名称".repeat(40) + "」";
    let bounded = bound_item_name(&long).expect("a printable name is kept");
    assert!(bounded.chars().count() <= LENGTH_RECOVERY_MAX_ITEM_CHARS + 1);
    assert!(!bounded.contains('\n'));
    assert!(bound_item_name("  \n\t ").is_none(), "a blank name is not quoted back");

    let huge = LengthRecoveryDelivery {
        kind: LengthRecoveryDeliveryKind::UnverifiedArtifacts,
        passed: 0,
        failed: 0,
        pending: 0,
        outstanding: (0..40).map(|index| format!("交付物{index}")).collect(),
    }
    .instruction();
    assert!(huge.len() <= LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES);
    assert!(huge.is_char_boundary(huge.len()), "no multi-byte character is cut in half");
}

#[test]
fn a_real_checklist_decides_the_instruction_kind_and_a_failed_artifact_is_not_completion() {
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "length-delivery", None, false, false, (5, 5));
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms()).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);

    // Nothing was produced: the Host may not call this verified, and the
    // instruction must name the outstanding item instead of claiming success.
    let facts = coordinator.length_recovery_delivery().unwrap();
    assert_eq!(facts.kind, LengthRecoveryDeliveryKind::UnverifiedArtifacts);
    assert_eq!(facts.passed, 0);
    assert!(facts.pending + facts.failed > 0);
    assert!(!facts.outstanding.is_empty(), "the outstanding item is named from the checklist");
    let instruction = facts.instruction();
    assert!(instruction.contains("任务还未完成"));
    assert!(!instruction.contains("全部通过"));

    // A promised artifact that failed its checks is a *failed* verdict, staged and
    // finalized through the ordinary deterministic gate (the same two-phase write
    // a failing decision owns).
    coordinator.fail("test.length_delivery_blocked", "promised artifact not verified").unwrap();
    let staged = db.delivery_checklist(&run_id).unwrap();
    assert!(staged.iter().any(|item| item.status == "failed"), "{staged:?}");
    let facts = coordinator.length_recovery_delivery().unwrap();
    assert_eq!(facts.kind, LengthRecoveryDeliveryKind::IncompleteDeliverables);
    assert!(facts.failed > 0);
    let instruction = facts.instruction();
    assert!(instruction.contains("任务尚未完成"));
    assert!(instruction.contains("未通过"));

    // The Run's own terminal account must not claim completion either.
    let message = coordinator.length_truncated_terminal_message(Some(&facts), true);
    assert!(message.starts_with(LENGTH_TRUNCATED_TERMINAL));
    assert!(message.contains("缺失或不合格"));
    assert!(!message.contains("任务已完成"));
    assert!(!message.contains("全部通过核验"));
}

// ---------------------------------------------------------------------------
// The durable one-shot lane
// ---------------------------------------------------------------------------

#[test]
fn one_controlled_recovery_is_scheduled_persisted_and_delivered() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;

    coordinator
        .dispatch_batch("worker-batch", "first", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                json!({})))
        })
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "retry_scheduled", "not terminal: one recovery is planned");
    assert_eq!(snapshot.retry.length_recovery_attempts, 1);
    assert_eq!(snapshot.retry.model_attempts, 1, "it counts toward the unified budget");
    let events = retry_events_for(&root, &run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["class"], "length_truncated_no_body");
    assert_eq!(events[0]["lengthRecoveryAttempt"], 1);
    assert_eq!(events[0]["lengthRecoveryMax"], 1);
    assert_eq!(events[0]["delayMs"].as_i64(), Some(MODEL_LENGTH_RECOVERY_INTERVAL_MS));

    // The recovery request is only built after its own durable due time.
    assert!(coordinator
        .dispatch_batch("worker-batch", "early", &Allow, |_, _, _| panic!("not due yet"))
        .is_err());
    let due = snapshot.retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();

    // The armed recovery request carries the recorded instruction, and answering
    // it accepts the response and marks the allowance used.
    let instruction = std::cell::RefCell::new(None);
    coordinator
        .dispatch_batch("worker-batch", "recovery", &Allow, |binding, frame, _| {
            let last = frame.history.last().cloned().unwrap();
            *instruction.borrow_mut() =
                last["content"][0]["text"].as_str().map(str::to_owned);
            Ok(accepted_answer(binding, frame))
        })
        .unwrap();
    let instruction = instruction.borrow().clone().unwrap();
    let record = durable_record(&db, &run_id).unwrap();
    assert_eq!(
        instruction,
        record["instruction"].as_str().unwrap(),
        "the model received exactly the recorded instruction"
    );
    assert!(instruction.contains("Fox 长度恢复提示"));
    // This fixture records no wire layer, so the Host must not claim the parameter
    // was effective: the recovery is the instruction alone, applied to the frozen
    // request.
    assert_eq!(record["parameterEffect"]["effective"], false);
    assert_eq!(record["parameterEffect"]["lever"], "instruction_only");
    assert_eq!(record["recoveryParameters"], json!({}));
    assert_eq!(record["changedServiceFields"], json!([]));
    assert_eq!(record["instructionDelivered"], true);
    let retry = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert!(
        retry.length_recovery_json.as_deref().unwrap().contains("\"appliedAtWallMs\":"),
        "the arming commit marks the recovery applied"
    );
    assert!(!retry
        .length_recovery_json
        .as_deref()
        .unwrap()
        .contains("\"appliedAtWallMs\":null"));
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools, "no tool was replayed");
}

#[test]
fn a_recovery_that_truncates_again_ends_honestly_without_a_second_chance() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;

    coordinator
        .dispatch_batch("worker-batch", "first", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .unwrap();
    let due = coordinator.snapshot().unwrap().retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();

    // The one recovery is truncated as well: the Run must stop, not loop.
    coordinator
        .dispatch_batch("worker-batch", "recovery-again", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.tool_calls, tools, "durable progress is kept");
    assert_eq!(
        retry_events_for(&root, &run_id).len(),
        1,
        "no second recovery and no ladder re-send"
    );
    let (code, message) = terminal_failure_for(&root, &run_id);
    assert_eq!(code, "kernel.model_incomplete", "the established terminal vocabulary is kept");
    assert!(message.starts_with(LENGTH_TRUNCATED_TERMINAL), "{message}");
    assert!(message.contains("已使用过一次"));
    assert!(!message.contains("任务已完成"));
    // The settled delivery lease is released by the same atomic commit.
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let (status, owner): (String, Option<String>) = connection
        .query_row(
            "SELECT status, lease_owner FROM kernel_effect_outbox
              WHERE run_id=?1 AND effect_key='deliver-batch:worker-batch'",
            [run_id.clone()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), owner), ("completed", None));
    // Never flipped to completed.
    assert_ne!(snapshot.state, "completed");
    let completed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.completed'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(completed, 0);
}

#[test]
fn restart_effect_key_and_a_new_batch_never_refresh_the_recovery() {
    // The required combination, in order:
    //  1. a length truncation is hit; 2. the one recovery is spent;
    //  3. a response is actually accepted and a real tool call succeeds (which does
    //     reset the ordinary cumulative counter); 4. a NEW effect key truncates
    //     again; 5. no second recovery exists and no ladder re-send happens.
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);

    // 1. The first truncation, on the settled batch.
    coordinator
        .dispatch_batch("worker-batch", "first-truncation", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().retry.length_recovery_attempts, 1);

    // 2. The recovery is armed (and therefore spent) after its due time.
    let due = coordinator.snapshot().unwrap().retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();

    // 3. An accepted response that proposes a new tool call; the tool succeeds.
    let next_seq = std::cell::Cell::new(0u64);
    coordinator
        .dispatch_batch("worker-batch", "recovery-accepted", &Allow, |binding, frame, _| {
            next_seq.set(frame.checkpoint_seq);
            Ok(proposing_answer(binding, frame))
        })
        .unwrap();
    let after_accept = coordinator.snapshot().unwrap();
    assert_eq!(
        after_accept.retry.model_attempts, 0,
        "an accepted response resets the ordinary cumulative counter"
    );
    assert_eq!(
        after_accept.retry.length_recovery_attempts, 1,
        "but never the length-recovery allowance"
    );
    assert!(after_accept.tool_calls.iter().any(|tool| tool.tool_call_id == "recovery-read"));
    let executions = std::cell::Cell::new(0);
    coordinator
        .dispatch_tool("recovery-read", "recovery-owner", |binding, effect, token| {
            executions.set(executions.get() + 1);
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            Ok((true, crate::resource_gateway::execute(binding, "read", &payload["input"], token)?))
        })
        .unwrap();
    assert_eq!(executions.get(), 1);
    coordinator.tick().unwrap();

    // A restart between the two truncations must not hand out the allowance again.
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().retry.length_recovery_attempts, 1);

    // 4. A truncation on a NEW effect key (the batch the accepted response opened).
    let new_batch = format!("model-batch:{run_id}:{}", next_seq.get());
    coordinator
        .dispatch_batch(&new_batch, "second-truncation", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .expect("the refusal is a terminal decision, not a dispatch error");

    // 5. No second recovery, no ladder re-send, honest ending.
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.retry.length_recovery_attempts, 1, "still exactly one");
    assert_eq!(retry_events_for(&root, &run_id).len(), 1, "no new scheduling event");
    let record = durable_record(&db, &run_id).unwrap();
    assert_eq!(record["effectKey"], "deliver-batch:worker-batch", "the old record is untouched");
    assert_eq!(
        record["parameterEffect"]["effective"], false,
        "this fixture records no wire layer, so no parameter was claimed effective"
    );
    assert_eq!(record["recoveryParameters"], json!({}));
    let (code, message) = terminal_failure_for(&root, &run_id);
    assert_eq!(code, "kernel.model_incomplete");
    assert!(message.starts_with(LENGTH_TRUNCATED_TERMINAL), "{message}");
    assert!(message.contains("不会再重复同一请求"));
    assert!(!message.contains("任务已完成"));
    // The successful tool from step 3 is still durable and was executed once.
    assert_eq!(executions.get(), 1);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let replayed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='dispatch_tool' AND attempts<>1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(replayed, 0, "no successful side effect is replayed");
}

#[test]
fn a_recovery_that_cannot_fit_the_whole_run_budget_is_refused_not_overspent() {
    // The one recovery also answers to the Run's elapsed bound: when its wait does
    // not fit what is left, the Run ends honestly instead of spending the time.
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let budgets = TimeBudgets {
        run_execution_ms: 20_000,
        run_execution_limited: true,
        ..TimeBudgets::default()
    };
    assert!(MODEL_LENGTH_RECOVERY_INTERVAL_MS > budgets.run_execution_ms);
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5), true, budgets,
    );
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);

    coordinator
        .dispatch_batch("worker-batch", "truncated", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "failed");
    assert!(retry_events_for(&root, &run_id).is_empty(), "nothing may be scheduled");
    let retry = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert_eq!(
        retry.length_recovery_attempts, 0,
        "an unaffordable recovery is not spent"
    );
    assert!(retry.length_recovery_json.is_none());
    let (code, message) = terminal_failure_for(&root, &run_id);
    assert_eq!(code, "kernel.model_incomplete");
    assert!(message.starts_with(LENGTH_TRUNCATED_TERMINAL), "{message}");
    assert!(message.contains("剩余运行预算不足"), "{message}");
}

#[test]
fn cancellation_refuses_the_recovery_before_any_request_is_armed() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);

    // The user cancels while the truncated round is still in flight.
    let refused = coordinator.dispatch_batch("worker-batch", "cancel-race", &Allow, |binding, frame, _| {
        coordinator.cancel().unwrap();
        Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
    });
    assert!(refused.is_err(), "cancellation refuses the recovery");
    assert!(retry_events_for(&root, &run_id).is_empty(), "no recovery may be scheduled");
    let retry = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert_eq!(retry.length_recovery_attempts, 0, "the allowance is not spent");
    assert!(retry.length_recovery_json.is_none());

    // Cancelling after scheduling but before dispatch also stops the request.
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    coordinator
        .dispatch_batch("worker-batch", "first", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    coordinator.cancel().unwrap();
    coordinator.settle_cancellation().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "cancelled");
    let due = 1_000 + MODEL_LENGTH_RECOVERY_INTERVAL_MS;
    clock.advance(due - clock.get() + 1);
    assert!(coordinator
        .dispatch_batch("worker-batch", "after-cancel", &Allow, |_, _, _| panic!("cancelled Run must not dispatch"))
        .is_err());
    assert_eq!(retry_events_for(&root, &run_id).len(), 1, "the schedule was already recorded once");
}
// ---------------------------------------------------------------------------
// Wire level: a real isolated worker against a local provider
// ---------------------------------------------------------------------------

/// A local SSE provider that keeps every request body and truncates the first
/// round with thinking-only output, exactly like the reported incident.
fn truncating_provider() -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        while requests.len() < 2 {
            assert!(Instant::now() < deadline, "the recovery request never reached the provider");
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            let (header_end, length) = loop {
                let read = match stream.read(&mut buffer) {
                    Ok(0) => break (0, 0),
                    Ok(read) => read,
                    Err(_) => break (0, 0),
                };
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            if header_end == 0 {
                continue;
            }
            while bytes.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "model request body ended early");
                bytes.extend_from_slice(&buffer[..read]);
            }
            let body: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let first = requests.is_empty();
            requests.push(body);
            let chunks = if first {
                // Thinking only, then the provider's output limit: no public text and
                // no tool proposal, so the round cannot be accepted.
                vec![
                    json!({"id":"length-recovery","object":"chat.completion.chunk","created":1,
                        "model":"lens-reasoner","choices":[{"index":0,"delta":{"reasoning_content":"still planning the answer in great detail"},
                        "finish_reason":null}]}),
                    json!({"id":"length-recovery","object":"chat.completion.chunk","created":1,
                        "model":"lens-reasoner","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}),
                ]
            } else {
                vec![
                    json!({"id":"length-recovery","object":"chat.completion.chunk","created":1,
                        "model":"lens-reasoner","choices":[{"index":0,"delta":{"role":"assistant","content":"已按核验结果给出简短完成说明。"},
                        "finish_reason":null}]}),
                    json!({"id":"length-recovery","object":"chat.completion.chunk","created":1,
                        "model":"lens-reasoner","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                ]
            };
            let body = format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", chunks[0], chunks[1]);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body).unwrap();
            stream.flush().unwrap();
        }
        requests
    });
    (address, server)
}

/// The durable usage record of the nth provider request of this Run.
fn usage_layers(root: &std::path::Path, run_id: &str, index: usize) -> Value {
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let mut query = connection
        .prepare("SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.request' ORDER BY seq")
        .unwrap();
    let rows: Vec<String> = query
        .query_map([run_id], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    let event: Value = serde_json::from_str(&rows[index]).unwrap();
    event["record"]["config"].clone()
}

#[test]
fn the_recovery_request_reaches_the_provider_with_the_recorded_instruction_and_parameters() {
    let (address, server) = truncating_provider();
    // The `openai` provider family is the one whose verified compat in
    // `services/agent-runtime/src/model-profile.mjs` declares
    // `supportsReasoningEffort`, i.e. the transport that really serializes the
    // reasoning level the recovery lowered. The identity carries that family on
    // purpose: the request itself goes to the local fixture.
    let mut config = config_with_service(json!({
        "apiType": "openai-completions", "modelId": "api.openai.com/gpt-5",
        "baseUrl": format!("http://{address}/v1"),
        "reasoning": true, "thinkingLevel": "high", "maxOutputTokens": 8192,
    }));
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(
        &clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5),
    );
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);

    // Round 1: the real worker hits the provider's output limit. The Host must not
    // re-send on the ladder — it plans the one controlled recovery. The dispatch
    // uses the configuration the coordinator itself resolves for this effect key
    // (identical to the frozen one here), so the identity guard is exercised.
    let (config_for_dispatch, pending) =
        coordinator.dispatch_model_config(&kernel::batch_delivery_effect_key("worker-batch")).unwrap();
    assert!(pending.is_none(), "no recovery is pending before the truncation");
    assert_eq!(config_for_dispatch, config);
    coordinator
        .dispatch_batch_with_worker("worker-batch", "first", &Allow, &real_worker_command(), &config_for_dispatch, "local-test-only")
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "retry_scheduled");
    assert_eq!(snapshot.retry.length_recovery_attempts, 1);
    let events = retry_events_for(&root, &run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["class"], "length_truncated_no_body", "the incident shape");
    assert_eq!(events[0]["delayMs"].as_i64(), Some(MODEL_LENGTH_RECOVERY_INTERVAL_MS));

    // Round 2: the recovery, at its own durable due time, completes the Run. The
    // coordinator must now hand the dispatch the *derived* configuration.
    let due = snapshot.retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();
    let (config_for_recovery, pending) =
        coordinator.dispatch_model_config(&kernel::batch_delivery_effect_key("worker-batch")).unwrap();
    assert!(pending.is_some(), "the recovery config is derived for this dispatch");
    assert_ne!(
        config_for_recovery.hash().unwrap(),
        config.hash().unwrap(),
        "the recovery request is not the frozen one"
    );
    assert_eq!(config_for_recovery.model_service["thinkingLevel"], "medium");
    assert_eq!(config_for_recovery.model_service["maxOutputTokens"], config.model_service["maxOutputTokens"]);
    coordinator
        .dispatch_batch_with_worker("worker-batch", "recovery", &Allow, &real_worker_command(), &config_for_recovery, "local-test-only")
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "the truncated round and its single recovery");

    // The recovery request carried the recorded instruction …
    let instruction = durable_record(&db, &run_id).unwrap()["instruction"]
        .as_str()
        .unwrap()
        .to_owned();
    let messages = requests[1]["messages"].to_string();
    assert!(messages.contains("Fox 长度恢复提示"), "{messages}");
    assert!(messages.contains(&instruction), "the provider saw exactly the recorded text");
    assert!(
        !requests[0]["messages"].to_string().contains("Fox 长度恢复提示"),
        "the first attempt had no recovery instruction"
    );

    // … and the parameter that was proven effective, while nothing else about the
    // request moved. Every field this test relies on is asserted present: a missing
    // field fails the test instead of skipping the check.
    let first = usage_layers(&root, &run_id, 0);
    let second = usage_layers(&root, &run_id, 1);
    println!("[length-recovery] first  resolved/sent = {} / {}", first["resolved"], first["sent"]);
    println!("[length-recovery] second resolved/sent = {} / {}", second["resolved"], second["sent"]);
    assert_eq!(first["resolved"]["thinkingLevel"], "high");
    assert_eq!(
        second["resolved"]["thinkingLevel"], "medium",
        "the one recovery really lowered the reasoning level the worker used"
    );
    let before_effort = first["sent"]["reasoning_effort"]
        .as_str()
        .expect("the first attempt must really have serialized reasoning_effort");
    let after_effort = second["sent"]["reasoning_effort"]
        .as_str()
        .expect("the recovery must really have serialized reasoning_effort");
    assert_eq!(before_effort, "high");
    assert_eq!(after_effort, "medium", "the parameter really decreased on the wire");
    let before_model = first["sent"]["model"].as_str().expect("the sent model is recorded");
    let after_model = second["sent"]["model"].as_str().expect("the sent model is recorded");
    assert_eq!(before_model, after_model, "no silent model switch");
    assert_eq!(before_model, "api.openai.com/gpt-5");
    let before_budget = first["sent"]["max_completion_tokens"]
        .as_i64()
        .expect("the first attempt must really have serialized an output budget");
    let after_budget = second["sent"]["max_completion_tokens"]
        .as_i64()
        .expect("the recovery must really have serialized an output budget");
    assert_eq!(before_budget, after_budget, "the output budget is never changed");
    assert_eq!(before_budget, 8192);
    // The other two budget spellings are absent in both attempts, not silently skipped.
    for layer in [&first, &second] {
        assert!(layer["sent"].get("max_tokens").is_none(), "unexpected max_tokens: {layer}");
        assert!(layer["sent"].get("max_output_tokens").is_none(), "unexpected max_output_tokens: {layer}");
    }
    assert_eq!(first["raw"]["modelId"], second["raw"]["modelId"]);
    // The record distinguishes the three layers and matches what really happened.
    let record = durable_record(&db, &run_id).unwrap();
    assert_eq!(record["parameterEffect"]["effective"], true);
    assert_eq!(record["parameterEffect"]["lever"], "parameters");
    assert_eq!(record["parameterEffect"]["signalSource"], EVIDENCE_SOURCE);
    assert_eq!(record["parameterEffect"]["observedReasoningEffort"], before_effort);
    assert_eq!(record["parameterEffect"]["expectedSentParameters"], json!({"reasoning_effort": after_effort}));
    assert_eq!(record["parameterEffect"]["locallyIntendedParameters"], json!({"thinkingLevel": "medium"}));
    assert_eq!(record["recoveryParameters"], json!({"thinkingLevel": "medium"}));
    assert_eq!(record["instructionDelivered"], true);
    assert_eq!(record["instructionDelivery"], "model_view_message");
    // Every settled tool result is still durable, and no side effect was replayed.
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let replayed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='dispatch_tool' AND attempts<>1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(replayed, 0);
}

#[test]
fn a_transport_that_never_serializes_the_parameter_gets_the_instruction_only() {
    // Same real worker, but this model resolves to the plain `openai-compatible`
    // provider family, whose verified compat does not serialize `reasoning_effort`.
    // The local level is still lowerable — that must NOT be bought as a parameter
    // recovery: the recovery is the instruction alone, recorded as such.
    let (address, server) = truncating_provider();
    let mut config = config_with_service(json!({
        "apiType": "openai-completions", "modelId": "lens-reasoner",
        "baseUrl": format!("http://{address}/v1"),
        "reasoning": true, "thinkingLevel": "high", "maxOutputTokens": 8192,
    }));
    config.model_service["modelId"] = json!("lens-reasoner");
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(
        &clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5),
    );
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let (config_for_dispatch, pending) =
        coordinator.dispatch_model_config(&kernel::batch_delivery_effect_key("worker-batch")).unwrap();
    assert!(pending.is_none());
    coordinator
        .dispatch_batch_with_worker("worker-batch", "first", &Allow, &real_worker_command(), &config_for_dispatch, "local-test-only")
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    let due = coordinator.snapshot().unwrap().retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();

    // The derived configuration for this dispatch must be the frozen one: no local
    // parameter is applied because it cannot be shown to reach the request.
    let (config_for_recovery, pending) =
        coordinator.dispatch_model_config(&kernel::batch_delivery_effect_key("worker-batch")).unwrap();
    assert!(pending.is_some(), "the recovery is still pending");
    assert_eq!(
        config_for_recovery.hash().unwrap(),
        config.hash().unwrap(),
        "an ineffective parameter must not change the request at all"
    );
    assert_eq!(config_for_recovery.model_service, config.model_service);
    coordinator
        .dispatch_batch_with_worker("worker-batch", "recovery", &Allow, &real_worker_command(), &config_for_recovery, "local-test-only")
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "the recovery request really went out");

    // The instruction is the effective change, and it is in the real request.
    let record = durable_record(&db, &run_id).unwrap();
    let instruction = record["instruction"].as_str().expect("the instruction is recorded").to_owned();
    assert!(requests[1]["messages"].to_string().contains(&instruction));
    assert!(!requests[0]["messages"].to_string().contains("Fox 长度恢复提示"));

    // The wire evidence: the transport serialized no reasoning field, before or
    // after, and the parameters are identical — asserted, not skipped.
    let first = usage_layers(&root, &run_id, 0);
    let second = usage_layers(&root, &run_id, 1);
    println!("[length-recovery/instruction-only] first/second sent = {} / {}",
        first["sent"], second["sent"]);
    assert!(
        first["sent"].get("reasoning_effort").is_none(),
        "the fixture transport must not serialize reasoning_effort: {}",
        first["sent"]
    );
    assert!(
        second["sent"].get("reasoning_effort").is_none(),
        "the recovery must not invent a reasoning field: {}",
        second["sent"]
    );
    assert_eq!(first["sent"]["max_tokens"], second["sent"]["max_tokens"]);
    assert_eq!(first["sent"]["model"], second["sent"]["model"]);
    assert_eq!(first["resolved"]["thinkingLevel"], "high");
    assert_eq!(second["resolved"]["thinkingLevel"], "high", "the level is left alone");

    // The record says exactly that: not effective, instruction only, no parameter.
    assert_eq!(record["parameterEffect"]["effective"], false);
    assert_eq!(record["parameterEffect"]["lever"], "instruction_only");
    assert_eq!(record["parametersMode"], "instruction_only");
    assert_eq!(record["parameterEffect"]["locallyIntendedParameters"], json!({"thinkingLevel": "medium"}));
    assert_eq!(
        record["parameterEffect"]["observedReasoningEffort"],
        Value::Null,
        "no effort was observed, and that absence is recorded as absence"
    );
    assert_eq!(record["parameterEffect"]["expectedSentParameters"], json!({}));
    assert_eq!(record["recoveryParameters"], json!({}));
    assert_eq!(record["changedServiceFields"], json!([]));
    assert_eq!(record["instructionDelivered"], true);
    assert_eq!(record["instructionDelivery"], "model_view_message");
    assert_eq!(record["parameterEffect"]["signalSource"], EVIDENCE_SOURCE);
}

#[test]
fn a_proven_parameter_recovers_even_when_the_instruction_cannot_be_delivered() {
    // The mirror of the refusal case: the wire record proves the transport
    // serializes the reasoning effort, so the parameter alone is an effective
    // change and the recovery is allowed — with the record stating plainly that the
    // instruction was NOT delivered.
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(reasoning_service());
    let (db, root, run_id) = fixture_with_budgets_notice_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (5, 5), true,
        TimeBudgets::default(), true,
    );
    record_attempt_wire(&db, &run_id, "0", wire_with_effort("high"));
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();

    coordinator
        .dispatch_initial("truncated", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.input.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .expect("a proven parameter is an effective lever");

    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    let record = durable_record(&db, &run_id).expect("the recovery is recorded");
    assert_eq!(record["parameterEffect"]["effective"], true);
    assert_eq!(record["parameterEffect"]["lever"], "parameters");
    assert_eq!(record["parameterEffect"]["observedReasoningEffort"], "high");
    assert_eq!(record["parameterEffect"]["expectedSentParameters"], json!({"reasoning_effort": "medium"}));
    assert_eq!(record["recoveryParameters"], json!({"thinkingLevel": "medium"}));
    assert_eq!(record["changedServiceFields"], json!(["thinkingLevel"]));
    assert_eq!(
        record["instructionDelivered"], false,
        "the notice lane forbids the model-view instruction, and the record says so"
    );
    assert_eq!(record["instructionDelivery"], "unavailable_notice_lane");
    // The dispatch that follows really uses the derived configuration.
    let due = coordinator.snapshot().unwrap().retry.due_wall_ms.unwrap();
    clock.advance(due - clock.get());
    coordinator.tick().unwrap();
    let (derived, pending) =
        coordinator.dispatch_model_config(&kernel::INITIAL_MODEL_EFFECT_KEY).unwrap();
    assert!(pending.is_some());
    assert_ne!(derived.hash().unwrap(), config.hash().unwrap());
    assert_eq!(derived.model_service["thinkingLevel"], "medium");
    assert_eq!(derived.model_service["maxOutputTokens"], config.model_service["maxOutputTokens"]);
    let _ = root;
}

#[test]
fn a_recovery_with_no_effective_lever_is_refused_instead_of_dispatched() {
    // The adapter is not shown to serialize the parameter AND this Run's job-notice
    // lane forbids adding the instruction, so the one recovery would be a request
    // with no real change. Nothing may be dispatched.
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let config = config_with_service(json!({
        "apiType": "openai-completions", "modelId": "lens-reasoner",
        "baseUrl": "http://127.0.0.1:9/v1",
        "reasoning": true, "thinkingLevel": "high", "maxOutputTokens": 8192,
    }));
    // A durable wire record that proves the transport serializes no effort.
    let (db, root, run_id) = fixture_with_budgets_notice_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (5, 5), true,
        TimeBudgets::default(), true,
    );
    record_attempt_wire(&db, &run_id, "0", json!({"model": "lens-reasoner", "max_tokens": 8192}));
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();

    coordinator
        .dispatch_initial("truncated", &Allow, |binding, frame, _| {
            Err(length_truncation_error(&frame.input.turn_id, &binding.run_id, frame.checkpoint_seq, json!({})))
        })
        .expect("the refusal is a terminal decision, not a dispatch error");

    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "failed");
    let retry = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert_eq!(retry.length_recovery_attempts, 0, "the allowance is not spent");
    assert!(retry.length_recovery_json.is_none(), "no recovery record is written");
    assert!(
        retry_events_for(&root, &run_id).is_empty(),
        "no recovery may be scheduled at all"
    );
    // No second model dispatch happened: the durable Host fact, not just the state.
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let dispatches: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_dispatched'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dispatches, 1, "only the truncated attempt was ever dispatched");
    let used: i64 = connection
        .query_row(
            "SELECT attempts FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='initial-model'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(used, 1, "the model delivery was attempted exactly once");
    let (code, message) = terminal_failure_for(&root, &run_id);
    assert_eq!(code, "kernel.model_incomplete");
    assert!(message.starts_with(LENGTH_TRUNCATED_TERMINAL), "{message}");
}

