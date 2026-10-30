//! Persistence adapter for the Fox Agent Kernel.
//!
//! This is an adapter: it owns the rusqlite statements for the Kernel tables
//! but contains no decision logic. The pure [`crate::kernel`] core decides state;
//! these methods durably record and reload Kernel facts after a sidecar or
//! application restart. Pending approval and external-effect facts MUST survive
//! restart — they are never held only in an in-memory promise.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The exact Host-to-Node frame or live directive, assembled before the model
/// lease. The transaction re-reads its pending notice set before binding it.
pub(crate) struct ModelNoticeInput<'a> {
    pub payload: &'a serde_json::Value,
    /// Complete Host-assembled model history for this exact lease. It is
    /// frozen even when there are no new notices, so a later Job park cannot
    /// substitute arbitrary history for a response the model already gave.
    pub delivered_history: &'a [serde_json::Value],
    /// Live directives omit transcript history; bind the Host's complete
    /// pre-notice model history alongside the directive in the same lease.
    pub live_history: Option<&'a [serde_json::Value]>,
    pub checkpoint_seq: u64,
    pub history_start: usize,
    pub historical_bytes: usize,
}

impl Database {
    pub(crate) fn kernel_model_wait_identity(&self, run_id: &str) -> Result<Option<(i64,i64)>,String> {
        self.with_connection(|connection| connection.query_row("SELECT model_request_since_wall_ms,
            (SELECT MAX(seq) FROM kernel_events WHERE run_id=?1 AND event_type IN
            ('engine.initial_dispatched','engine.batch_dispatched','engine.continuation_dispatched','context.compaction.dispatched'))
            FROM kernel_runs WHERE run_id=?1 AND state IN ('running','compacting') AND model_request_since_wall_ms IS NOT NULL",
            [run_id], |row| Ok((row.get(0)?, row.get(1)?))).optional())
    }
    /// Presentation telemetry, independent of the authoritative Kernel event
    /// sequence. Only an open model request can produce a waiting notice.
    pub(crate) fn record_kernel_model_waiting(&self, run_id: &str, request_id: &str,
        elapsed_ms: i64, remaining_ms: i64, phase: &str, identity: Option<(i64,i64)>) -> Result<(), String> {
        if !matches!(phase, "model_response" | "context_compaction") || elapsed_ms < 0 || remaining_ms < 0
            || request_id.len() > 128 || request_id.is_empty() { return Err("invalid model waiting notice".into()); }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let Some((since,dispatch_seq)) = identity else { return Ok(()); };
            let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_runs WHERE run_id=?1
                AND state IN ('running','compacting') AND model_request_since_wall_ms=?2 AND ?3=(SELECT MAX(seq)
                FROM kernel_events WHERE run_id=?1 AND event_type IN ('engine.initial_dispatched','engine.batch_dispatched',
                'engine.continuation_dispatched','context.compaction.dispatched')))", params![run_id,since,dispatch_seq], |row| row.get(0))?;
            if active {
                super::kernel_display::event(&transaction, run_id, &format!("model-waiting:{request_id}:{dispatch_seq}:{}", elapsed_ms/15_000),
                    &serde_json::json!({"type":"run.model_waiting","phase":phase,"elapsedMs":elapsed_ms,"remainingMs":remaining_ms,
                        "requestId":request_id,"dispatchSeq":dispatch_seq,"startedAt":since,
                        "message":"仍在等待模型响应","outcomeKnown":false}), now_ms())?;
            }
            transaction.commit()?;
            Ok(())
        })
    }
    /// Owned transport diagnostics use the presentation sequence, never the
    /// Kernel decision sequence. The exact active dispatch must still own them.
    pub(crate) fn record_kernel_response_diagnostic(&self, run_id: &str, request_id: &str,
        turn_id: &str, checkpoint_seq: u64, worker: &Value, host_category: &str,
        identity: Option<(i64,i64)>) -> Result<bool,String> {
        if request_id.is_empty() || request_id.len() > 128 || turn_id.len() > 512
            || worker.canonical_string().len() > 2048 || !matches!(host_category,
                "worker_rejected"|"settled_model_failure"|"protocol_invalid"|"transport_outcome_unknown") {
            return Err("invalid response diagnostic".into());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let Some((since, dispatch_seq)) = identity else { return Ok(false); };
            let active:bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_runs r
                WHERE r.run_id=?1 AND r.state='running' AND r.model_request_since_wall_ms=?2
                AND ?3=(SELECT MAX(seq) FROM kernel_events WHERE run_id=?1 AND event_type IN
                ('engine.initial_dispatched','engine.batch_dispatched','engine.continuation_dispatched','context.compaction.dispatched'))
                AND NOT EXISTS(SELECT 1 FROM kernel_host_commands WHERE run_id=?1 AND kind='cancel'))",
                params![run_id,since,dispatch_seq],|row|row.get(0))?;
            if !active { return Ok(false); }
            let event:Value = transaction.query_row("SELECT payload_json FROM kernel_events WHERE run_id=?1 AND seq=?2",
                params![run_id,dispatch_seq],|row|row.get::<_,String>(0))?
                .parse().map_err(|_|notice_error("invalid dispatch diagnostic identity"))?;
            if event["turnId"].as_str() != Some(turn_id) { return Ok(false); }
            let batch_id=event["batchId"].as_str();
            let effect:Option<(String,String,String,i64)> = transaction.query_row(
                "SELECT effect_key,effect_type,status,attempts FROM kernel_effect_outbox WHERE run_id=?1
                 AND status='leased' AND effect_type IN ('initial_model','continuation_model','deliver_tool_batch')
                 AND ((?2 IS NOT NULL AND batch_id=?2) OR (?2 IS NULL AND batch_id IS NULL)) ORDER BY leased_at DESC LIMIT 1",
                params![run_id,batch_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
            let Some((effect_key,effect_type,status,attempts))=effect else { return Ok(false); };
            super::kernel_display::event(&transaction,run_id,&format!("response-diagnostic:{request_id}:{dispatch_seq}"),
                &serde_json::json!({"type":"run.model_response_diagnostic","schemaVersion":1,"source":"host_response_admission",
                    "runId":run_id,"requestId":request_id,"turnId":turn_id,"checkpointSeq":checkpoint_seq,
                    "dispatchSeq":dispatch_seq,"startedAt":since,"batchId":batch_id,
                    "effectKey":effect_key,"effectType":effect_type,"outboxStatus":status,"attempts":attempts,
                    "hostCategory":host_category,"worker":worker,"resultAccepted":false,
                    "externalEffectStatus":"unknown","replayDecision":"not_authorized_by_diagnostic"}),now_ms())?;
            transaction.commit()?;
            Ok(true)
        })
    }
}

fn notice_error(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn bound_context_on(
    tx: &rusqlite::Transaction<'_>, run: &str, target: &str,
    effect_key: &str, mut messages: Vec<Value>,
) -> rusqlite::Result<Vec<Value>> {
    let model = super::kernel_model_config::read_model_config(tx, run)?;
    let config_hash = model.hash().map_err(|_| notice_error("invalid model config hash"))?;
    for (plan, result) in super::kernel_compaction::read_compaction_results_on(tx, run, target)? {
        if plan.target != target || plan.source != messages || plan.config_hash != config_hash {
            return Err(notice_error("bound model compaction source changed"));
        }
        messages = result.view(&plan).map_err(|_| notice_error("invalid bound model compaction"))?;
    }
    let retry: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
          WHERE e.run_id=?1 AND e.event_type='engine.model_rejected'
            AND json_extract(e.payload_json,'$.effectKey')=?2
            AND json_extract(e.payload_json,'$.failure.category')='incomplete_response'
            AND r.kernel_mode='authoritative')",
        params![run, effect_key], |row| row.get(0),
    )?;
    if retry {
        messages.push(serde_json::json!({"role":"user","content":[{"type":"text",
            "text":crate::kernel_compaction::INCOMPLETE_MODEL_RETRY_PROMPT}],"timestamp":0}));
    }
    Ok(messages)
}

struct BoundBatchSource {
    history: Vec<Value>,
    assistant: Value,
    tools: Vec<fox_engine_protocol::KernelSettledToolResult>,
    turn_id: String,
    response_seq: i64,
}

fn acknowledge_model_notices(
    tx: &rusqlite::Transaction<'_>, root: &str, conversation: &str, run: &str,
    dispatch_key: &str, owner: &str, now: i64,
) -> rusqlite::Result<()> {
    let bound: Option<(String,String,i64,String,String)> = tx.query_row(
        "SELECT input_hash,input_json,history_start,lease_owner,state
         FROM kernel_model_notice_inputs WHERE run_id=?1 AND dispatch_key=?2",
        params![run,dispatch_key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).optional()?;
    let Some((input_hash,input_json,start,stored_owner,state)) = bound else {
        let orphan: i64 = tx.query_row("SELECT COUNT(*) FROM kernel_job_notice_deliveries d
            JOIN kernel_job_notices n ON n.job_id=d.job_id
            WHERE n.data_root_id=?1 AND n.conversation_id=?2 AND n.run_id=?3
              AND d.dispatch_key=?4",params![root,conversation,run,dispatch_key],|row|row.get(0))?;
        if orphan != 0 { return Err(notice_error("orphan model notice binding")); }
        return Ok(());
    };
    if stored_owner != owner || state != "bound"
        || format!("sha256:{}",hex::encode(Sha256::digest(input_json.as_bytes()))) != input_hash {
        return Err(notice_error("model notice dispatch input or owner changed"));
    }
    let bound: serde_json::Value = serde_json::from_str(&input_json)
        .map_err(|_|notice_error("invalid stored model notice input"))?;
    let notices: Vec<fox_engine_protocol::HostJobNotice> = match bound["modelInput"].get("hostJobNotices") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_|notice_error("invalid stored model notice list"))?,
        None => Vec::new(), // DTO omits an empty vec on the wire.
    };
    fox_engine_protocol::validate_host_job_notices(&notices)
        .map_err(|_|notice_error("invalid stored model notice bounds"))?;
    if bound["historyStart"].as_i64() != Some(start) {
        return Err(notice_error("model notice history position changed"));
    }
    let mut statement = tx.prepare("SELECT d.job_id,d.input_hash,d.history_position
        FROM kernel_job_notice_deliveries d JOIN kernel_job_notices n ON n.job_id=d.job_id
        WHERE n.data_root_id=?1 AND n.conversation_id=?2 AND n.run_id=?3
          AND d.dispatch_key=?4 AND d.state='bound' ORDER BY d.history_position")?;
    let rows = statement.query_map(params![root,conversation,run,dispatch_key],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if rows.len() != notices.len() { return Err(notice_error("model notice binding count changed")); }
    for (index, (id, hash, position)) in rows.iter().enumerate() {
        if id != &notices[index].job_id || hash != &input_hash
            || *position != start + index as i64 {
            return Err(notice_error("model notice binding identity, hash or position changed"));
        }
        let fact = super::kernel_job_execution::read_notice(tx,root,conversation,run,id)?
            .ok_or_else(||notice_error("model notice no longer matches terminal job"))?;
        if fact.model_fact() != notices[index] {
            return Err(notice_error("model notice terminal fact changed"));
        }
    }
    let delivered = tx.execute("UPDATE kernel_job_notice_deliveries SET state='acknowledged',acknowledged_at=?5
        WHERE dispatch_key=?4 AND state='bound' AND job_id IN
          (SELECT job_id FROM kernel_job_notices WHERE data_root_id=?1
            AND conversation_id=?2 AND run_id=?3)",
        params![root,conversation,run,dispatch_key,now])?;
    if delivered != notices.len() { return Err(notice_error("model notice acknowledgement count changed")); }
    let changed = tx.execute("UPDATE kernel_model_notice_inputs SET state='acknowledged',acknowledged_at=?4
        WHERE run_id=?1 AND dispatch_key=?2 AND lease_owner=?3 AND state='bound'",
        params![run,dispatch_key,owner,now])?;
    if changed != 1 { return Err(notice_error("model notice input acknowledgement lost")); }
    Ok(())
}

fn valid_run_state(state: &str) -> bool {
    matches!(
        state,
        "created"
            | "running"
            | "waiting_approval"
            | "waiting_jobs"
            | "retry_scheduled"
            // Package 2-D U1: a delivery the user must still accept. Non-terminal;
            // the artifacts are retained but their content is not verified.
            | "awaiting_delivery_review"
            | "compacting"
            | "cancelling"
            | "completed"
            | "failed"
            | "cancelled"
            | "budget_exhausted"
            | "approval_expired"
    )
}

fn run_transition_allowed(from: &str, to: &str) -> bool {
    from == to
        || matches!(
            (from, to),
            ("created", "running")
                | ("created", "failed")
                | ("created", "cancelling")
                | ("running", "waiting_approval")
                | ("running", "waiting_jobs")
                | ("running", "retry_scheduled")
                | ("running", "compacting")
                | ("running", "cancelling")
                | ("running", "completed")
                | ("running", "failed")
                | ("running", "budget_exhausted")
                // Package 2-D U1: the ONLY entry edge. A `RequiresU1` verdict is
                // produced by the stop gate, which runs while the Run is
                // `running`, so this is the only state it can be reached from.
                // `waiting_approval` / `waiting_jobs` / `retry_scheduled` /
                // `compacting` are deliberately NOT sources: each carries an
                // unfinished side effect (an undecided approval plus its
                // deadline, durable child Jobs, a scheduled whole-turn retry, an
                // in-progress compaction). Parking from one of those would
                // abandon it — and from `waiting_approval` would additionally
                // violate the `WaitingApproval ⟺ approval_deadline_wall_ms`
                // invariant the durable commit enforces below.
                | ("running", "awaiting_delivery_review")
                // Exits. `completed` is the user having accepted every item that
                // needed accepting; `failed` is an explicit user rejection
                // (`delivery.rejected_by_user`); `cancelling` is cancellation.
                //
                // `("awaiting_delivery_review", "running")` is deliberately
                // RETAINED BUT UNREACHABLE, and the comment that used to sit here
                // ("补证续写，经新 Run") was wrong. Supplying more evidence starts
                // a NEW Run through the normal continuation entry; the parent Run
                // stays parked on `awaiting_delivery_review` for ever and never
                // returns to `running`. Nothing in this crate triggers the
                // transition — it exists so that a future, separately authorized
                // mechanism has a legal edge to use, not as a description of
                // today's behaviour. Deleting it was considered and rejected:
                // removing a legal edge is a policy change, whereas keeping it
                // with an honest comment is not.
                | ("awaiting_delivery_review", "running")
                | ("awaiting_delivery_review", "completed")
                | ("awaiting_delivery_review", "failed")
                | ("awaiting_delivery_review", "cancelling")
                | ("waiting_approval", "running")
                | ("waiting_approval", "cancelling")
                | ("waiting_approval", "failed")
                | ("waiting_approval", "budget_exhausted")
                // The human never decided: the attempt ends as a continuable
                // expiry, and the expired approval stays non-executable.
                | ("waiting_approval", "approval_expired")
                | ("waiting_jobs", "running")
                | ("waiting_jobs", "cancelling")
                | ("waiting_jobs", "failed")
                | ("waiting_jobs", "budget_exhausted")
                | ("retry_scheduled", "running")
                | ("retry_scheduled", "cancelling")
                | ("retry_scheduled", "failed")
                | ("retry_scheduled", "budget_exhausted")
                | ("compacting", "running")
                | ("compacting", "cancelling")
                | ("compacting", "failed")
                | ("compacting", "budget_exhausted")
                | ("cancelling", "cancelled")
        )
}

/// Recheck a Job-notice continuation against the exact scoped facts visible
/// in this write transaction. A caller's payload alone is never authority to
/// wake a Run or ask the model another question.
fn checked_job_notice_input(
    tx: &rusqlite::Transaction<'_>, root: &str, conversation: &str, run: &str,
    turn: &str, frozen: &crate::kernel::RunFrozenConfig,
    payload: &serde_json::Value, expected_history: &[serde_json::Value],
) -> rusqlite::Result<()> {
    let input: fox_engine_protocol::KernelInitialModelInput =
        serde_json::from_value(payload["input"].clone())
            .map_err(|_| kernel_err("job notice has no valid frozen model input"))?;
    input.validate_job_notice().map_err(kernel_err)?;
    let supplied: Vec<fox_engine_protocol::HostJobNotice> =
        serde_json::from_value(payload["hostJobNotices"].clone())
            .map_err(|_| kernel_err("job notice continuation omitted typed facts"))?;
    fox_engine_protocol::validate_host_job_notices(&supplied).map_err(kernel_err)?;
    if payload["lane"] != "job_notice" || payload["prompt"] != ""
        || input.run_id != run || input.turn_id != turn
        || input.prompt_config_hash != frozen.prompt_config_hash
        || input.messages != expected_history || supplied.is_empty() {
        return Err(kernel_err("job notice continuation changed history, identity or prompt"));
    }
    let historical = fox_engine_protocol::historical_host_job_notice_bytes(&input.messages)
        .map_err(kernel_err)?;
    let expected = super::kernel_job_execution::pending_model_facts(
        tx, root, conversation, run, (16 * 1024usize).saturating_sub(historical),
    )?;
    if supplied != expected {
        return Err(kernel_err(format!("{}typed facts changed",
            crate::kernel::JOB_NOTICE_COMPETITION)));
    }
    let unfinished: i64 = tx.query_row(
        "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1 AND conversation_id=?2
          AND state IN ('queued','running','paused')", params![run,conversation], |row| row.get(0))?;
    if unfinished != 0 { return Err(kernel_err(format!("{}unfinished Jobs changed",
        crate::kernel::JOB_NOTICE_COMPETITION))); }
    let received: i64 = tx.query_row(
        "SELECT COUNT(*) FROM run_steering_messages
          WHERE run_id=?1 AND lane='current' AND status='received'",
        [run], |row| row.get(0))?;
    if received != 0 {
        return Err(kernel_err(format!("{}{received}", crate::kernel::STEERING_COMPETITION)));
    }
    Ok(())
}

/// The replacement frame carries an idempotency key; an in-session Pi
/// directive does not. In the latter case derive its dispatch from the typed
/// directive, model cursor and the durable leased outbox instead of expecting
/// a field the protocol never sent.
fn bound_response_dispatch_matches(
    tx: &rusqlite::Transaction<'_>, run: &str, event_type: &str,
    response: &serde_json::Value, dispatch_key: &str,
    model_input: &serde_json::Value, checkpoint: i64,
) -> rusqlite::Result<bool> {
    let ordinary_key = model_input["idempotencyKey"].as_str() == Some(dispatch_key);
    match event_type {
        "engine.initial_response" => Ok(dispatch_key == crate::kernel::INITIAL_MODEL_IDEMPOTENCY_KEY
            && ordinary_key && model_input["continuationKey"].is_null()),
        "engine.batch_response" => {
            let Some(batch_id) = response["batchId"].as_str() else { return Ok(false) };
            if dispatch_key != crate::kernel::batch_delivery_idempotency_key(batch_id)
                || model_input["batchId"] != batch_id { return Ok(false); }
            if model_input["kind"] == "batch" {
                return Ok(model_input["checkpointSeq"].as_i64() == Some(checkpoint)
                    && tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox
                        WHERE run_id=?1 AND batch_id=?2 AND effect_type='deliver_tool_batch'
                          AND idempotency_key=?3 AND status='completed')",
                        params![run,batch_id,dispatch_key],|row|row.get::<_,bool>(0))?);
            }
            Ok(ordinary_key)
        }
        "engine.continuation_response" => {
            if model_input["kind"] == "continuation" {
                let Some(effect_key)=dispatch_key.strip_prefix("continuation-delivery:") else {
                    return Ok(false);
                };
                return Ok(model_input["previewSeq"].as_i64() == Some(checkpoint)
                    && tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox o
                        JOIN kernel_events e ON e.run_id=o.run_id
                          AND e.event_type='engine.continuation_dispatched'
                          AND e.seq=?3+1
                          AND json_extract(e.payload_json,'$.effectKey')=o.effect_key
                        WHERE o.run_id=?1 AND o.effect_key=?2
                          AND o.effect_type='continuation_model'
                          AND o.idempotency_key=?4 AND o.status='completed')",
                        params![run,effect_key,checkpoint,dispatch_key],|row|row.get::<_,bool>(0))?);
            }
            Ok(ordinary_key && model_input["continuationKey"].as_str()
                .is_some_and(|key| dispatch_key == format!("continuation-delivery:{key}")))
        }
        _ => Ok(false),
    }
}

fn settled_model_delivery_history(
    tx: &rusqlite::Transaction<'_>, run: &str,
    response: &crate::kernel::PersistEvent,
) -> rusqlite::Result<Vec<serde_json::Value>> {
    let payload: serde_json::Value = serde_json::from_str(&response.payload_json)
        .map_err(|_| kernel_err("invalid settled model response for job notice"))?;
    let checkpoint = payload["response"]["checkpointSeq"].as_i64()
        .ok_or_else(|| kernel_err("job notice response has no model checkpoint"))?;
    let mut statement = tx.prepare("SELECT dispatch_key,input_hash,input_json FROM kernel_model_notice_inputs
        WHERE run_id=?1 AND state='acknowledged'
          AND json_extract(input_json,'$.checkpointSeq')=?2 LIMIT 2")?;
    let bound = statement.query_map(params![run,checkpoint], |row| {
        Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if bound.len() != 1 { return Err(kernel_err("job notice has no unique settled model input")); }
    let (dispatch_key,input_hash,input_json) = &bound[0];
    if format!("sha256:{}",hex::encode(Sha256::digest(input_json.as_bytes()))) != *input_hash {
        return Err(kernel_err("job notice settled model input hash changed"));
    }
    let bound: serde_json::Value = serde_json::from_str(input_json)
        .map_err(|_| kernel_err("invalid settled model input for job notice"))?;
    let model_input = &bound["modelInput"];
    if !bound_response_dispatch_matches(tx,run,&response.event_type,&payload,
        dispatch_key,model_input,checkpoint)? {
        return Err(kernel_err("job notice response differs from its settled dispatch"));
    }
    let mut history: Vec<serde_json::Value> = serde_json::from_value(bound["deliveredHistory"].clone())
        .map_err(|_| kernel_err("job notice settled delivery history is invalid"))?;
    let assistant = payload["response"]["assistantMessage"].clone();
    if assistant["role"] != "assistant" || assistant["stopReason"] != "stop" {
        return Err(kernel_err("job notice has no settled assistant stop"));
    }
    history.push(assistant);
    Ok(history)
}

/// Guard the wait projection before the generic event/outbox writer runs. The
/// IMMEDIATE transaction makes the old projection, Job rows, event stream and
/// next outbox one serializable decision. An adapter cannot invent a wake by
/// merely setting `run_state` or supplying a `job_notice` lane.
fn validate_job_wait_decision(
    tx: &rusqlite::Transaction<'_>, root: &str, run: &str, now: i64,
    old_state: &str, old_seq: i64, old_elapsed: i64,
    old_deadline: Option<i64>, old_accounted: Option<i64>,
    frozen: &crate::kernel::RunFrozenConfig,
    cmd: &crate::kernel::KernelPersistCommand,
    settling_model_response: bool,
) -> rusqlite::Result<()> {
    let park_events: Vec<_> = cmd.events.iter().filter(|e| e.event_type == "run.waiting_jobs").collect();
    let account_events: Vec<_> = cmd.events.iter().filter(|e| e.event_type == "run.jobs_wait_accounted").collect();
    let wake_events: Vec<_> = cmd.events.iter().filter(|e| e.event_type == "run.jobs_woken").collect();
    let notice_events: Vec<_> = cmd.events.iter().filter(|e| e.event_type == "engine.continuation_requested"
        && serde_json::from_str::<serde_json::Value>(&e.payload_json)
            .ok().is_some_and(|v| v["lane"] == "job_notice")).collect();
    let notice_outbox: Vec<_> = cmd.outbox.iter().filter(|o| {
        o.kind == crate::kernel::OutboxEffectKind::ContinuationModel
            && serde_json::from_str::<serde_json::Value>(&o.payload_json)
                .ok().is_some_and(|v| v["lane"] == "job_notice")
    }).collect();
    if !frozen.experimental_compute_job_notice || frozen.kernel_mode != "authoritative" {
        if old_state == "waiting_jobs" || cmd.run_state == crate::kernel::RunState::WaitingJobs
            || !park_events.is_empty() || !account_events.is_empty() || !wake_events.is_empty()
            || !notice_outbox.is_empty() || !notice_events.is_empty() || cmd.wait_deadline_wall_ms.is_some()
            || cmd.wait_accounted_until_wall_ms.is_some() {
            return Err(kernel_err("job wait requires frozen experimental flag"));
        }
        return Ok(());
    }
    if park_events.len() > 1 || account_events.len() > 1 || wake_events.len() > 1
        || notice_outbox.len() > 1 || notice_events.len() > 1 {
        return Err(kernel_err("duplicate job wait decision fact"));
    }
    if old_state != "waiting_jobs" {
        if old_deadline.is_some() || old_accounted.is_some() {
            return Err(kernel_err("non-waiting Run retains job wait projection"));
        }
        if cmd.run_state != crate::kernel::RunState::WaitingJobs {
            if notice_outbox.len() == 1 && notice_events.len() == 1 {
                let response = cmd.events.iter().find(|event| matches!(event.event_type.as_str(),
                    "engine.initial_response" | "engine.batch_response" | "engine.continuation_response"))
                    .ok_or_else(|| kernel_err("direct job notice has no settled response"))?;
                let event = notice_events[0];
                let outbox = notice_outbox[0];
                if old_state != "running" || cmd.run_state != crate::kernel::RunState::Running
                    || !settling_model_response || cmd.model_request_since_wall_ms.is_some()
                    || cmd.events.len() != 2 || cmd.outbox.len() != 1
                    || response.seq != (old_seq + 1) as u64 || event.seq != response.seq + 1
                    || outbox.effect_key != format!("continuation:{}", event.seq)
                    || event.payload_json != outbox.payload_json {
                    return Err(kernel_err("direct job notice must settle one response and arm one continuation"));
                }
                let conversation: String = tx.query_row(
                    "SELECT conversation_id FROM runs WHERE id=?1", [run], |row| row.get(0))?;
                let history = settled_model_delivery_history(tx,run,response)?;
                let payload: serde_json::Value = serde_json::from_str(&outbox.payload_json)
                    .map_err(|_| kernel_err("invalid direct job notice continuation"))?;
                if payload["turnId"] != cmd.turn_id || payload["effectKey"] != outbox.effect_key {
                    return Err(kernel_err("direct job notice continuation identity changed"));
                }
                checked_job_notice_input(tx,root,&conversation,run,&cmd.turn_id,frozen,&payload,&history)?;
                return Ok(());
            }
            if cmd.wait_deadline_wall_ms.is_some() || cmd.wait_accounted_until_wall_ms.is_some()
                || !park_events.is_empty() || !account_events.is_empty() || !wake_events.is_empty()
                || !notice_outbox.is_empty() || !notice_events.is_empty() {
                return Err(kernel_err("job wait fact outside waiting transition"));
            }
            return Ok(());
        }
        if old_state != "running" || park_events.len() != 1
            || !account_events.is_empty() || !wake_events.is_empty() || !notice_outbox.is_empty()
            || !notice_events.is_empty()
            || !cmd.outbox.is_empty()
            || !settling_model_response || cmd.model_request_since_wall_ms.is_some()
            || cmd.terminal_written || now <= 0 || cmd.running_elapsed_ms < old_elapsed {
            return Err(kernel_err("job wait lacks settled response or legal transition"));
        }
        let event = park_events[0];
        if i64::try_from(event.seq).ok() != Some(old_seq + cmd.events.len() as i64)
            || cmd.events.last().map(|e| e.seq) != Some(event.seq) {
            return Err(kernel_err("invalid job wait event sequence"));
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json)
            .map_err(|_| kernel_err("invalid job wait event"))?;
        let response_seq = payload["settledResponseSeq"].as_u64()
            .ok_or_else(|| kernel_err("job wait has no settled response cursor"))?;
        let response = cmd.events.iter().find(|e| e.seq == response_seq && matches!(
            e.event_type.as_str(), "engine.initial_response" | "engine.batch_response" | "engine.continuation_response"
        )).ok_or_else(|| kernel_err("job wait response is not in the leased decision"))?;
        let response_payload: serde_json::Value = serde_json::from_str(&response.payload_json)
            .map_err(|_| kernel_err("invalid settled response event"))?;
        if response.seq >= event.seq || response_payload["response"] != payload["response"]
            || !payload["history"].is_array() || event.payload_json.len() > 1_048_576
            || payload["dataRootId"] != root || payload["parkedAtWallMs"] != now
            || payload["runningElapsedMs"] != cmd.running_elapsed_ms {
            return Err(kernel_err("job wait response, history, root or clock changed"));
        }
        // This row was written in the exact transaction that leased the Host
        // model frame and acknowledged only while settling its response. It is
        // stored even with zero notices: a caller-supplied history array is
        // never itself proof of what the model actually received.
        let checkpoint = response_payload["response"]["checkpointSeq"].as_i64()
            .ok_or_else(|| kernel_err("job wait has no bound model checkpoint"))?;
        let mut statement = tx.prepare("SELECT dispatch_key,input_hash,input_json FROM kernel_model_notice_inputs
            WHERE run_id=?1 AND state='acknowledged'
              AND json_extract(input_json,'$.checkpointSeq')=?2 LIMIT 2")?;
        let bound_rows = statement.query_map(params![run,checkpoint], |row| {
            Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        if bound_rows.len() != 1 { return Err(kernel_err("job wait has no unique bound model input")); }
        let (dispatch_key,input_hash,input_json) = &bound_rows[0];
        if format!("sha256:{}",hex::encode(Sha256::digest(input_json.as_bytes()))) != *input_hash {
            return Err(kernel_err("job wait bound model input hash changed"));
        }
        let bound: serde_json::Value = serde_json::from_str(input_json)
            .map_err(|_| kernel_err("invalid bound model input for job wait"))?;
        let model_input = &bound["modelInput"];
        if !bound_response_dispatch_matches(tx,run,&response.event_type,&response_payload,
            dispatch_key,model_input,checkpoint)?
            || bound["deliveredHistory"] != payload["history"] {
            return Err(kernel_err("job wait history differs from bound Host model delivery"));
        }
        let mut expected: Vec<crate::kernel::WaitingJobFact> =
            serde_json::from_value(payload["jobs"].clone())
                .map_err(|_| kernel_err("invalid frozen Job wait set"))?;
        if expected.is_empty() || expected.windows(2).any(|pair| pair[0].job_id >= pair[1].job_id) {
            return Err(kernel_err("job wait set is empty or unordered"));
        }
        let conversation: String = tx.query_row(
            "SELECT conversation_id FROM runs WHERE id=?1", [run], |row| row.get(0))?;
        let unresolved_tools: i64 = tx.query_row(
            "SELECT COUNT(*) FROM kernel_tool_calls WHERE run_id=?1
              AND state NOT IN ('completed','failed','cancelled','expired')",
            [run], |row| row.get(0))?;
        let unresolved_models: i64 = tx.query_row(
            "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1
              AND effect_type IN ('initial_model','continuation_model','deliver_tool_batch')
              AND status IN ('pending','leased')",
            [run], |row| row.get(0))?;
        if unresolved_tools != 0 || unresolved_models != 0 {
            return Err(kernel_err("job wait still has executable tool or model effects"));
        }
        let mut statement = tx.prepare(
            "SELECT job_id,attempts,deadline_ms,kind FROM kernel_jobs
              WHERE run_id=?1 AND conversation_id=?2 AND state IN ('queued','running','paused')
              ORDER BY job_id")?;
        let actual = statement.query_map(params![run,conversation], |row| {
            Ok((row.get::<_,String>(0)?, row.get::<_,i64>(1)?,
                row.get::<_,Option<i64>>(2)?, row.get::<_,String>(3)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        if actual.len() != expected.len() { return Err(kernel_err(format!("{}wait set changed before park",
            crate::kernel::JOB_NOTICE_COMPETITION))); }
        let mut max_job_deadline = 0_i64;
        for (fact, (job_id, attempt, deadline, kind)) in expected.drain(..).zip(actual) {
            if fact.job_id != job_id || fact.attempt != attempt
                || deadline != Some(fact.deadline_wall_ms) || fact.deadline_wall_ms <= now
                || kind != "attachment_compute" {
                return Err(kernel_err("job wait identity or original deadline changed"));
            }
            max_job_deadline = max_job_deadline.max(fact.deadline_wall_ms);
        }
        let expected_deadline = if frozen.run_execution_limited {
            let remaining = frozen.run_execution_budget_ms - cmd.running_elapsed_ms;
            if remaining <= 0 { return Err(kernel_err("job wait Run budget already exhausted")); }
            now.checked_add(remaining).ok_or_else(|| kernel_err("job wait deadline overflow"))?
                .min(max_job_deadline)
        } else { max_job_deadline };
        if payload["waitDeadlineWallMs"] != expected_deadline
            || cmd.wait_deadline_wall_ms != Some(expected_deadline)
            || cmd.wait_accounted_until_wall_ms != Some(now) {
            return Err(kernel_err("job wait deadline or accounting cursor changed"));
        }
        let undelivered: i64 = tx.query_row(
            "SELECT COUNT(*) FROM run_steering_messages
              WHERE run_id=?1 AND lane='current' AND status='received'",
            [run], |row| row.get(0))?;
        if undelivered > 0 {
            return Err(kernel_err(format!("{}{undelivered}",crate::kernel::STEERING_COMPETITION)));
        }
        return Ok(());
    }

    // Every waiting decision is fenced by the event sequence of its original
    // park and by the old accounting cursor. A copied DB has a different root.
    let park: (i64, String) = tx.query_row(
        "SELECT seq,payload_json FROM kernel_events
          WHERE run_id=?1 AND event_type='run.waiting_jobs' ORDER BY seq DESC LIMIT 1",
        [run], |row| Ok((row.get(0)?,row.get(1)?)))?;
    let parked: serde_json::Value = serde_json::from_str(&park.1)
        .map_err(|_| kernel_err("invalid stored job wait event"))?;
    if parked["dataRootId"] != root || old_deadline.is_none() || old_accounted.is_none()
        || old_deadline != parked["waitDeadlineWallMs"].as_i64()
        || !park_events.is_empty() {
        return Err(kernel_err("job wait root, deadline or event identity changed"));
    }
    let deadline = old_deadline.unwrap();
    let accounted = old_accounted.unwrap();
    if now < accounted || accounted <= 0 || deadline < accounted {
        return Err(kernel_err("job wait wall clock moved backwards"));
    }
    let through = now.min(deadline);
    let next_elapsed = old_elapsed.checked_add(through - accounted)
        .ok_or_else(|| kernel_err("job wait elapsed overflow"))?;
    if cmd.running_elapsed_ms != next_elapsed {
        return Err(kernel_err("job wait interval was skipped or double charged"));
    }
    if through > accounted {
        if account_events.len() != 1 { return Err(kernel_err("job wait debit has no event")); }
        let debit: serde_json::Value = serde_json::from_str(&account_events[0].payload_json)
            .map_err(|_| kernel_err("invalid job wait debit event"))?;
        if debit["parkSeq"] != park.0 || debit["fromWallMs"] != accounted
            || debit["throughWallMs"] != through || debit["runningElapsedMs"] != next_elapsed {
            return Err(kernel_err("job wait debit identity changed"));
        }
    } else if !account_events.is_empty() {
        return Err(kernel_err("job wait was debited twice"));
    }
    if cmd.run_state == crate::kernel::RunState::WaitingJobs {
        if !wake_events.is_empty() || !notice_outbox.is_empty()
            || !cmd.outbox.is_empty() || through == deadline
            || cmd.wait_deadline_wall_ms != Some(deadline)
            || cmd.wait_accounted_until_wall_ms != Some(through)
            || frozen.run_execution_limited && next_elapsed >= frozen.run_execution_budget_ms {
            return Err(kernel_err("job wait projection or budget changed"));
        }
        return Ok(());
    }
    if cmd.wait_deadline_wall_ms.is_some() || cmd.wait_accounted_until_wall_ms.is_some() {
        return Err(kernel_err("departed job wait retains accounting projection"));
    }
    if cmd.run_state == crate::kernel::RunState::Completed {
        return Err(kernel_err("waiting Jobs cannot complete before a durable wake"));
    }
    if frozen.run_execution_limited && next_elapsed >= frozen.run_execution_budget_ms {
        if !matches!(cmd.run_state,
                crate::kernel::RunState::BudgetExhausted | crate::kernel::RunState::Cancelling)
            || !wake_events.is_empty() || !notice_outbox.is_empty() {
            return Err(kernel_err("exhausted job wait cannot request another model"));
        }
        return Ok(());
    }
    if cmd.run_state != crate::kernel::RunState::Running {
        if !wake_events.is_empty() || !notice_outbox.is_empty() {
            return Err(kernel_err("terminal job wait cannot request another model"));
        }
        return Ok(()); // cancellation/failure is handled by its existing state guard
    }
    if now > deadline || wake_events.len() != 1 || notice_outbox.len() != 1
        || notice_events.len() != 1
        || cmd.outbox.len() != 1 {
        return Err(kernel_err("job wake requires one in-deadline notice outbox"));
    }
    let wake = wake_events[0];
    let wake_payload: serde_json::Value = serde_json::from_str(&wake.payload_json)
        .map_err(|_| kernel_err("invalid job wake event"))?;
    let outbox = notice_outbox[0];
    let outbox_payload: serde_json::Value = serde_json::from_str(&outbox.payload_json)
        .map_err(|_| kernel_err("invalid job wake outbox"))?;
    if wake_payload["parkSeq"] != park.0 || wake_payload["fromWallMs"] != accounted
        || wake_payload["throughWallMs"] != through
        || wake_payload["runningElapsedMs"] != next_elapsed
        || outbox.effect_key != format!("continuation:{}", wake.seq)
        || outbox_payload["effectKey"] != outbox.effect_key
        || notice_events[0].payload_json != outbox.payload_json {
        return Err(kernel_err("job wake/outbox identity mismatch"));
    }
    let conversation: String = tx.query_row(
        "SELECT conversation_id FROM runs WHERE id=?1", [run], |row| row.get(0))?;
    let frozen_jobs: Vec<crate::kernel::WaitingJobFact> =
        serde_json::from_value(parked["jobs"].clone())
            .map_err(|_| kernel_err("invalid frozen Job wait set"))?;
    for job in frozen_jobs {
        let actual: Option<(String,i64,Option<i64>,String)> = tx.query_row(
            "SELECT state,attempts,deadline_ms,conversation_id FROM kernel_jobs WHERE run_id=?1 AND job_id=?2",
            params![run,job.job_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
        ).optional()?;
        if actual.is_none_or(|(state,attempt,job_deadline,owner)| {
            !matches!(state.as_str(), "completed"|"failed"|"cancelled")
                || attempt != job.attempt || job_deadline != Some(job.deadline_wall_ms)
                || owner != conversation
        }) { return Err(kernel_err("frozen Job identity or terminal state changed")); }
        let notice_owned: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM kernel_job_notices
              WHERE data_root_id=?1 AND conversation_id=?2 AND run_id=?3 AND job_id=?4)",
            params![root,conversation,run,job.job_id], |row| row.get(0))?;
        if !notice_owned { return Err(kernel_err("frozen terminal Job has no scoped Host notice")); }
    }
    let unfinished: i64 = tx.query_row(
        "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1 AND conversation_id=?2
          AND state IN ('queued','running','paused')",
        params![run,conversation], |row| row.get(0))?;
    if unfinished != 0 { return Err(kernel_err("job wake still has unfinished Jobs")); }
    let mut expected_history: Vec<serde_json::Value> =
        serde_json::from_value(parked["history"].clone())
            .map_err(|_| kernel_err("job wake has invalid frozen history"))?;
    let response = parked["response"]["assistantMessage"].clone();
    if response["role"] != "assistant" || response["stopReason"] != "stop" {
        return Err(kernel_err("job wake has no frozen settled assistant"));
    }
    expected_history.push(response);
    checked_job_notice_input(tx,root,&conversation,run,&cmd.turn_id,frozen,
        &outbox_payload,&expected_history)?;
    Ok(())
}

fn valid_tool_state(state: &str) -> bool {
    matches!(
        state,
        "pending"
            | "waiting_approval"
            | "running"
            | "completed"
            | "failed"
            | "cancelled"
            | "expired"
    )
}

fn tool_transition_allowed(from: &str, to: &str) -> bool {
    from == to
        || matches!(
            (from, to),
            ("pending", "waiting_approval")
                | ("pending", "running")
                | ("pending", "failed")
                | ("pending", "cancelled")
                | ("waiting_approval", "running")
                | ("waiting_approval", "failed")
                | ("waiting_approval", "cancelled")
                // Never dispatched: the approval expired before a decision.
                | ("waiting_approval", "expired")
                | ("running", "completed")
                | ("running", "failed")
                | ("running", "cancelled")
        )
}

fn parse_json(value: &str, field: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(value).map_err(|error| format!("invalid {field} JSON: {error}"))
}

/// Lift a Kernel fail-closed message into a rusqlite error so transaction
/// closures can abort with a descriptive, non-panic error.
fn kernel_err(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        message.into(),
    )))
}

/// Map an outbox row starting at column 0:
/// effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
/// payload_json, status, attempts.
fn map_outbox_effect(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::kernel::OutboxEffect> {
    map_outbox_effect_row_at(row, 0)
}

fn map_outbox_effect_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<crate::kernel::OutboxEffect> {
    let kind_str: String = row.get(offset + 1)?;
    let status_str: String = row.get(offset + 6)?;
    let kind = crate::kernel::OutboxEffectKind::parse(&kind_str)
        .ok_or_else(|| kernel_err(format!("unknown outbox effect type: {kind_str}")))?;
    let status = crate::kernel::OutboxStatus::parse(&status_str)
        .ok_or_else(|| kernel_err(format!("unknown outbox status: {status_str}")))?;
    Ok(crate::kernel::OutboxEffect {
        effect_key: row.get(offset)?,
        kind,
        idempotency_key: row.get(offset + 2)?,
        tool_call_id: row.get(offset + 3)?,
        batch_id: row.get(offset + 4)?,
        payload_json: row.get(offset + 5)?,
        status,
        attempts: row.get::<_, i64>(offset + 7)? as u32,
    })
}

/// A tool call that was waiting for human approval when the process stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredPendingToolCall {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    pub source_order: i64,
    pub canonical_input_json: String,
}

/// Rehydrated kernel state for a run that is not in a terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelRunRecovery {
    pub run_id: String,
    pub engine_id: String,
    pub kernel_mode: String,
    pub frozen_config_json: String,
    pub state: String,
    pub pending_tool_calls: Vec<RecoveredPendingToolCall>,
}

impl Database {
    pub(crate) fn kernel_waiting_park(&self,run_id:&str)
        -> Result<(u64,serde_json::Value),String> {
        self.with_connection(|conn| {
            let (seq,body):(i64,String)=conn.query_row(
                "SELECT e.seq,e.payload_json FROM kernel_events e JOIN kernel_runs k ON k.run_id=e.run_id
                 WHERE e.run_id=?1 AND k.state='waiting_jobs' AND e.event_type='run.waiting_jobs'
                 ORDER BY e.seq DESC LIMIT 1",[run_id],|row|Ok((row.get(0)?,row.get(1)?)))?;
            let event:serde_json::Value=serde_json::from_str(&body)
                .map_err(|_|kernel_err("invalid frozen job wait event"))?;
            if event["dataRootId"]!=self.data_root_id || !event["history"].is_array()
                || !event["response"].is_object() {
                return Err(kernel_err("frozen job wait is outside this data root"));
            }
            Ok((u64::try_from(seq).map_err(|_|kernel_err("invalid job wait park sequence"))?,event))
        })
    }

    pub(crate) fn kernel_waiting_park_seq(&self, run_id: &str) -> Result<u64, String> {
        self.with_connection(|connection| {
            let seq: i64 = connection.query_row(
                "SELECT seq FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'
                   ORDER BY seq DESC LIMIT 1", [run_id], |row| row.get(0))?;
            u64::try_from(seq).map_err(|_| kernel_err("invalid job wait park sequence"))
        })
    }

    /// Host-only identity for freezing a wait event. The decision transaction
    /// independently compares it with this handle's canonical data-root ID.
    pub(crate) fn kernel_data_root_id(&self) -> &str {
        &self.data_root_id
    }

    /// Read only durable rejection evidence for this exact model effect. A
    /// settled model retry never replays a completed tool or another batch.
    pub(crate) fn kernel_model_retry_needs_completion(&self, run_id: &str, effect_key: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                 WHERE e.run_id=?1 AND e.event_type='engine.model_rejected'
                 AND json_extract(e.payload_json,'$.effectKey')=?2
                 AND json_extract(e.payload_json,'$.failure.category')='incomplete_response'
                 AND r.kernel_mode='authoritative')",
                params![run_id, effect_key], |row| row.get(0),
            )
        })
    }

    /// The checkpoint is an append-only event committed with the original tool
    /// proposal, so no new table or second-transaction crash window is needed.
    pub fn kernel_engine_batch_checkpoint(&self, run_id: &str, batch_id: &str) -> Result<Option<serde_json::Value>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT e.payload_json FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                 WHERE e.run_id=?1 AND e.event_type='engine.batch_checkpoint'
                   AND json_extract(e.payload_json,'$.batchId')=?2
                   AND r.kernel_mode='authoritative' ORDER BY e.seq LIMIT 2",
            )?;
            let rows = statement.query_map(params![run_id, batch_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            if rows.len() > 1 { return Err(kernel_err("multiple engine checkpoints for one immutable batch")); }
            rows.first().map(|row| serde_json::from_str(row).map_err(|error| kernel_err(error.to_string()))).transpose()
        })
    }

    /// Count durable Host continuation decisions for the bounded stop-review.
    /// Only committed decisions count; a crash before the next model round
    /// fails closed on recovery instead of silently re-injecting. Business
    /// delivery repairs ride the same transport but carry
    /// `lane='delivery_repair'` and are counted separately by
    /// [`Database::kernel_count_delivery_repairs`]; they never consume the
    /// stop-review budget.
    pub fn kernel_count_continuations(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                 WHERE e.run_id=?1 AND e.event_type='engine.continuation_requested'
                   AND r.kernel_mode='authoritative'
                   AND COALESCE(json_extract(e.payload_json,'$.lane'),'stop_review')='stop_review'",
                params![run_id], |row| row.get(0),
            )
        })
    }

    /// Count durable bounded business-delivery repair decisions. Separate from
    /// both the stop-review continuation counter and model-failure retries.
    pub fn kernel_count_delivery_repairs(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                 WHERE e.run_id=?1 AND e.event_type='engine.continuation_requested'
                   AND r.kernel_mode='authoritative'
                   AND json_extract(e.payload_json,'$.lane')='delivery_repair'",
                params![run_id], |row| row.get(0),
            )
        })
    }

    /// Count durable additional-user-input (`steering`) follow-ups. The Host
    /// answers mid-run user additions on their own lane and their own bounded
    /// counter: they are new user work and must never consume the stop-review
    /// budget that exists to review the model's own stop. The bound exists so a
    /// user cannot extend a Run without limit through the queue.
    pub fn kernel_count_steering_followups(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                 WHERE e.run_id=?1 AND e.event_type='engine.continuation_requested'
                   AND r.kernel_mode='authoritative'
                   AND json_extract(e.payload_json,'$.lane')='steering'",
                params![run_id], |row| row.get(0),
            )
        })
    }

    /// Latest durable kernel event type, used to distinguish an interrupted
    /// bounded continuation from an ordinary no-progress stop.
    pub fn kernel_last_event_type(&self, run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT event_type FROM kernel_events WHERE run_id=?1 ORDER BY seq DESC LIMIT 1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    /// Create the durable kernel run row with its frozen configuration.
    /// Runs the Kernel compatibility projection for one run. Production calls it
    /// inside the same transaction as the Kernel facts; tests use it to drive the
    /// projection without a worker.
    #[cfg(test)]
    pub(crate) fn kernel_project_for_test(&self, run_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            super::kernel_projection::project(&transaction, run_id, now_ms(), 0)?;
            transaction.commit()
        })
    }

    pub fn kernel_create_run(
        &self,
        run_id: &str,
        engine_id: &str,
        kernel_mode: &str,
        capability_manifest_version: i64,
        permission_snapshot_id: &str,
        execution_profile_id: &str,
        prompt_config_hash: &str,
        frozen_config_json: &str,
    ) -> Result<(), String> {
        if run_id.trim().is_empty()
            || engine_id.trim().is_empty()
            || permission_snapshot_id.trim().is_empty()
            || execution_profile_id.trim().is_empty()
            || prompt_config_hash.trim().is_empty()
        {
            return Err("kernel run identity fields must be non-empty".into());
        }
        // HARD ISOLATION: shadow runs are OBSERVATION-ONLY and must never own a
        // row in the executable kernel control surface (`kernel_runs` +
        // `kernel_effect_outbox`). They live exclusively in `kernel_shadow_runs`.
        // Rejecting at the write boundary (not just at lease time) means a
        // shadow decision can never create a leaseable effect, even if a future
        // caller mistakes the entry point.
        if kernel_mode == "shadow" {
            return Err(
                "shadow kernel runs must be created via kernel_create_shadow_run; \
                 the executable kernel_runs table rejects kernel_mode='shadow'"
                    .into(),
            );
        }
        if !matches!(kernel_mode, "legacy" | "authoritative") {
            return Err(format!("unsupported kernel mode: {kernel_mode}"));
        }
        if capability_manifest_version <= 0 {
            return Err("capability manifest version must be positive".into());
        }
        let frozen: crate::kernel::RunFrozenConfig = serde_json::from_str(frozen_config_json)
            .map_err(|error| format!("invalid frozen kernel config: {error}"))?;
        crate::kernel::RunController::validate_config(&frozen)
            .map_err(|error| error.to_string())?;
        if frozen.engine_id != engine_id
            || frozen.kernel_mode != kernel_mode
            || i64::from(frozen.capability_manifest_version) != capability_manifest_version
            || frozen.permission_snapshot_id != permission_snapshot_id
            || frozen.execution_profile_id != execution_profile_id
            || frozen.prompt_config_hash != prompt_config_hash
        {
            return Err("kernel run columns disagree with the frozen config".into());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO kernel_runs (
                    run_id, engine_id, kernel_mode, capability_manifest_version,
                    permission_snapshot_id, execution_profile_id, prompt_config_hash,
                    frozen_config_json, state, last_event_seq, created_at, updated_at, terminal_at,
                    capability_manifest_hash
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'created', 0, ?9, ?9, NULL, ?10)",
                params![
                    run_id,
                    engine_id,
                    kernel_mode,
                    capability_manifest_version,
                    permission_snapshot_id,
                    execution_profile_id,
                    prompt_config_hash,
                    frozen_config_json,
                    now,
                    frozen.capability_manifest_hash,
                ],
            )?;
            Ok(())
        })
    }

    pub fn kernel_update_run_state(&self, run_id: &str, state: &str) -> Result<(), String> {
        if !valid_run_state(state) {
            return Err(format!("unsupported kernel run state: {state}"));
        }
        if state == "waiting_jobs" {
            return Err("waiting_jobs requires the atomic Kernel decision transaction".into());
        }
        let now = now_ms();
        let terminal = matches!(
            state,
            "completed" | "failed" | "cancelled" | "budget_exhausted"
        );
        let conflict = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let current = transaction.query_row(
                "SELECT state FROM kernel_runs WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )?;
            if current == "waiting_jobs" {
                return Ok(Some("waiting_jobs cannot use the legacy state writer".into()));
            }
            if current == state {
                transaction.commit()?;
                return Ok(None);
            }
            if !run_transition_allowed(&current, state) {
                return Ok(Some(format!(
                    "illegal persisted run transition {current} -> {state} for {run_id}"
                )));
            }
            if terminal {
                let unresolved = transaction.query_row(
                    "SELECT COUNT(*) FROM kernel_tool_calls
                      WHERE run_id = ?1 AND state NOT IN ('completed','failed','cancelled','expired')",
                    params![run_id],
                    |row| row.get::<_, i64>(0),
                )?;
                if unresolved != 0 {
                    return Ok(Some(format!(
                        "cannot persist terminal run {run_id} while {unresolved} tool calls are unresolved"
                    )));
                }
            }
            let affected = transaction.execute(
                "UPDATE kernel_runs
                    SET state = ?2,
                        updated_at = ?3,
                        terminal_at = CASE WHEN ?4 THEN ?3 ELSE terminal_at END
                  WHERE run_id = ?1",
                params![run_id, state, now, terminal],
            )?;
            if affected == 0 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(None)
        })?;
        conflict.map_or(Ok(()), Err)
    }

    /// Append a kernel event. Idempotent on `(run_id, seq)`: a replayed event is
    /// ignored rather than duplicated. Also advances the run's high-water mark.
    pub fn kernel_append_event(
        &self,
        run_id: &str,
        seq: i64,
        event_type: &str,
        payload_json: &str,
    ) -> Result<bool, String> {
        if seq <= 0 || event_type.trim().is_empty() {
            return Err("kernel event sequence must be positive and type must be non-empty".into());
        }
        parse_json(payload_json, "kernel event payload")?;
        let now = now_ms();
        let outcome = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let last_event_seq = transaction.query_row(
                "SELECT last_event_seq FROM kernel_runs WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )?;
            let existing = transaction
                .query_row(
                    "SELECT event_type, payload_json FROM kernel_events WHERE run_id = ?1 AND seq = ?2",
                    params![run_id, seq],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing.0 != event_type || existing.1 != payload_json {
                    return Ok(Err(format!(
                        "kernel event identity conflict for {run_id} sequence {seq}"
                    )));
                }
                transaction.commit()?;
                return Ok(Ok(false));
            }
            if seq != last_event_seq.saturating_add(1) {
                return Ok(Err(format!(
                    "kernel event sequence gap for {run_id}: expected {}, received {seq}",
                    last_event_seq.saturating_add(1)
                )));
            }
            transaction.execute(
                "INSERT INTO kernel_events (run_id, seq, event_type, payload_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, seq, event_type, payload_json, now],
            )?;
            let run_affected = transaction.execute(
                "UPDATE kernel_runs SET last_event_seq = ?2, updated_at = ?3
                 WHERE run_id = ?1",
                params![run_id, seq, now],
            )?;
            if run_affected != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(Ok(true))
        })?;
        outcome
    }

    pub fn kernel_record_tool_batch(
        &self,
        batch_id: &str,
        run_id: &str,
        ordered_tool_call_ids_json: &str,
    ) -> Result<(), String> {
        if batch_id.trim().is_empty() || run_id.trim().is_empty() {
            return Err("kernel batch and run ids must be non-empty".into());
        }
        let ordered_ids = parse_json(ordered_tool_call_ids_json, "ordered tool call ids")?;
        let Some(ordered_ids) = ordered_ids.as_array() else {
            return Err("ordered tool call ids JSON must be an array".into());
        };
        if ordered_ids.is_empty() {
            return Err("kernel tool batch must contain at least one call".into());
        }
        let mut unique_ids = std::collections::BTreeSet::new();
        for value in ordered_ids {
            let Some(id) = value.as_str() else {
                return Err("ordered tool call ids must all be strings".into());
            };
            if id.trim().is_empty() || !unique_ids.insert(id) {
                return Err("ordered tool call ids must be non-empty and unique".into());
            }
        }
        let now = now_ms();
        let conflict = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.query_row(
                "SELECT 1 FROM kernel_runs WHERE run_id = ?1",
                params![run_id],
                |_| Ok(()),
            )?;
            // batch_id is GLOBALLY unique (v48 ruling): a reused batch id in
            // another run is rejected; an exact replay within the same run is ok.
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO kernel_tool_batches
                    (batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![batch_id, run_id, ordered_tool_call_ids_json, now],
            )?;
            if inserted == 0 {
                let existing = transaction.query_row(
                    "SELECT run_id, ordered_tool_call_ids_json FROM kernel_tool_batches
                      WHERE batch_id = ?1",
                    params![batch_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                if existing.0 != run_id || existing.1 != ordered_tool_call_ids_json {
                    return Ok(Some(format!(
                        "kernel tool batch identity conflict (batch id globally unique): {batch_id}"
                    )));
                }
            }
            transaction.commit()?;
            Ok(None)
        })?;
        conflict.map_or(Ok(()), Err)
    }

    /// Persist a single tool call independently (never a batch-wide "last result").
    pub fn kernel_record_tool_call(
        &self,
        run_id: &str,
        tool_call_id: &str,
        batch_id: &str,
        tool: &str,
        source_order: i64,
        canonical_input_json: &str,
        state: &str,
    ) -> Result<(), String> {
        if run_id.trim().is_empty()
            || tool_call_id.trim().is_empty()
            || batch_id.trim().is_empty()
            || tool.trim().is_empty()
            || source_order < 0
        {
            return Err("kernel tool identity fields must be valid and non-empty".into());
        }
        if !valid_tool_state(state) {
            return Err(format!("unsupported kernel tool state: {state}"));
        }
        if !parse_json(canonical_input_json, "canonical tool input")?.is_object() {
            return Err("canonical tool input JSON must be an object".into());
        }
        let now = now_ms();
        let conflict = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (batch_run, ordered_ids_json) = transaction.query_row(
                "SELECT run_id, ordered_tool_call_ids_json
                   FROM kernel_tool_batches WHERE batch_id = ?1",
                params![batch_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            if batch_run != run_id {
                return Ok(Some(format!(
                    "kernel tool batch {batch_id} belongs to a different run"
                )));
            }
            let ordered_ids = match serde_json::from_str::<Vec<String>>(&ordered_ids_json) {
                Ok(ids) => ids,
                Err(_) => {
                    return Ok(Some(format!(
                        "kernel tool batch {batch_id} has invalid persisted ordering"
                    )))
                }
            };
            if ordered_ids.get(source_order as usize).map(String::as_str) != Some(tool_call_id) {
                return Ok(Some(format!(
                    "kernel tool call {tool_call_id} does not match batch {batch_id} source order {source_order}"
                )));
            }
            let inserted = transaction.execute(
                "INSERT INTO kernel_tool_calls
                    (run_id, tool_call_id, batch_id, tool, source_order,
                     canonical_input_json, state, result_json, created_at, settled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL)
                 ON CONFLICT(run_id, tool_call_id) DO NOTHING",
                params![
                    run_id,
                    tool_call_id,
                    batch_id,
                    tool,
                    source_order,
                    canonical_input_json,
                    state,
                    now,
                ],
            )?;
            if inserted == 0 {
                let existing = transaction.query_row(
                    "SELECT batch_id, tool, source_order, canonical_input_json
                       FROM kernel_tool_calls WHERE run_id = ?1 AND tool_call_id = ?2",
                    params![run_id, tool_call_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?;
                if existing.0 != batch_id
                    || existing.1 != tool
                    || existing.2 != source_order
                    || existing.3 != canonical_input_json
                {
                    return Ok(Some(format!(
                        "kernel tool call identity conflict: {run_id}/{tool_call_id}"
                    )));
                }
            }
            transaction.commit()?;
            Ok(None)
        })?;
        conflict.map_or(Ok(()), Err)
    }

    pub fn kernel_update_tool_call_state(
        &self,
        run_id: &str,
        tool_call_id: &str,
        state: &str,
        result_json: Option<&str>,
    ) -> Result<(), String> {
        if !valid_tool_state(state) {
            return Err(format!("unsupported kernel tool state: {state}"));
        }
        if let Some(result) = result_json {
            parse_json(result, "tool result")?;
        }
        let now = now_ms();
        let terminal = matches!(state, "completed" | "failed" | "cancelled");
        let conflict = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (current, current_result) = transaction.query_row(
                "SELECT state, result_json FROM kernel_tool_calls WHERE run_id = ?1 AND tool_call_id = ?2",
                params![run_id, tool_call_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )?;
            if current == state {
                if current_result.as_deref().is_some_and(|existing| {
                    result_json.is_some_and(|incoming| incoming != existing)
                }) {
                    return Ok(Some(format!(
                        "conflicting duplicate tool result for {run_id}/{tool_call_id}"
                    )));
                }
                transaction.commit()?;
                return Ok(None);
            }
            if !tool_transition_allowed(&current, state) {
                return Ok(Some(format!(
                    "illegal persisted tool transition {current} -> {state} for {run_id}/{tool_call_id}"
                )));
            }
            let affected = transaction.execute(
                "UPDATE kernel_tool_calls
                    SET state = ?3,
                        result_json = COALESCE(?4, result_json),
                        settled_at = CASE WHEN ?5 THEN ?6 ELSE settled_at END
                  WHERE run_id = ?1 AND tool_call_id = ?2",
                params![run_id, tool_call_id, state, result_json, terminal, now],
            )?;
            if affected != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(None)
        })?;
        conflict.map_or(Ok(()), Err)
    }

    /// Record a pending approval. Idempotent on (run_id, tool_call_id).
    pub fn kernel_record_pending_approval(
        &self,
        run_id: &str,
        tool_call_id: &str,
    ) -> Result<(), String> {
        let now = now_ms();
        let conflict = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (tool_state, run_state) = transaction.query_row(
                "SELECT calls.state, runs.state
                   FROM kernel_tool_calls AS calls
                   JOIN kernel_runs AS runs ON runs.run_id = calls.run_id
                  WHERE calls.run_id = ?1 AND calls.tool_call_id = ?2",
                params![run_id, tool_call_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            if tool_state != "waiting_approval" || run_state != "waiting_approval" {
                return Ok(Some(format!(
                    "cannot record approval outside waiting_approval for {run_id}/{tool_call_id}"
                )));
            }
            let inserted = transaction.execute(
                "INSERT INTO kernel_approvals (run_id, tool_call_id, state, created_at, decided_at)
                 VALUES (?1, ?2, 'pending', ?3, NULL)
                 ON CONFLICT(run_id, tool_call_id) DO NOTHING",
                params![run_id, tool_call_id, now],
            )?;
            if inserted == 0 {
                let existing = transaction.query_row(
                    "SELECT state FROM kernel_approvals WHERE run_id = ?1 AND tool_call_id = ?2",
                    params![run_id, tool_call_id],
                    |row| row.get::<_, String>(0),
                )?;
                if existing != "pending" {
                    return Ok(Some(format!(
                        "cannot resurrect resolved approval for {run_id}/{tool_call_id}"
                    )));
                }
            }
            transaction.commit()?;
            Ok(None)
        })?;
        conflict.map_or(Ok(()), Err)
    }

    /// Resolve a pending approval with compare-and-set on `state = 'pending'`.
    /// Returns false when there was no pending approval (already decided / expired),
    /// so a late or replayed decision cannot re-execute an approved tool.
    pub fn kernel_resolve_approval(
        &self,
        run_id: &str,
        tool_call_id: &str,
        decision: &str,
    ) -> Result<bool, String> {
        if !matches!(
            decision,
            "allow_once" | "allow_conversation" | "denied" | "cancelled" | "expired"
        ) {
            return Err(format!("unsupported kernel approval decision: {decision}"));
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let run_state = transaction.query_row(
                "SELECT state FROM kernel_runs WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )?;
            if run_state != "waiting_approval" {
                transaction.commit()?;
                return Ok(false);
            }
            let affected = transaction.execute(
                "UPDATE kernel_approvals
                    SET state = ?3, decided_at = ?4
                  WHERE run_id = ?1 AND tool_call_id = ?2 AND state = 'pending'",
                params![run_id, tool_call_id, decision, now],
            )?;
            if affected == 0 {
                transaction.commit()?;
                return Ok(false);
            }

            let (tool_state, terminal) = match decision {
                "allow_once" | "allow_conversation" => ("running", false),
                "cancelled" => ("cancelled", true),
                "denied" | "expired" => ("failed", true),
                _ => unreachable!("decision was validated above"),
            };
            let tool_affected = transaction.execute(
                "UPDATE kernel_tool_calls
                    SET state = ?3,
                        settled_at = CASE WHEN ?4 THEN ?5 ELSE settled_at END
                  WHERE run_id = ?1 AND tool_call_id = ?2 AND state = 'waiting_approval'",
                params![run_id, tool_call_id, tool_state, terminal, now],
            )?;
            if tool_affected != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }

            let pending_count = transaction.query_row(
                "SELECT COUNT(*) FROM kernel_approvals WHERE run_id = ?1 AND state = 'pending'",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )?;
            if pending_count == 0 {
                transaction.execute(
                    "UPDATE kernel_runs
                        SET state = 'running', updated_at = ?2
                      WHERE run_id = ?1 AND state = 'waiting_approval'",
                    params![run_id, now],
                )?;
            }
            transaction.commit()?;
            Ok(true)
        })
    }

    // ---- Phase 3B: durable outbox, transactional decisions, rehydration ----

    pub(crate) fn kernel_continuation_input(&self, run_id: &str, effect_key: &str)
        -> Result<fox_engine_protocol::KernelInitialModelInput, String> {
        self.kernel_continuation_input_with_lane(run_id,effect_key).map(|(input,_,_)|input)
    }

    pub(crate) fn kernel_continuation_input_with_lane(&self, run_id: &str, effect_key: &str)
        -> Result<(fox_engine_protocol::KernelInitialModelInput,Option<String>,
            Vec<fox_engine_protocol::HostJobNotice>), String> {
        let body = self.with_connection(|connection| {
            connection.query_row("SELECT o.payload_json FROM kernel_effect_outbox o
                JOIN kernel_events e ON e.run_id=o.run_id AND e.event_type='engine.continuation_requested'
                    AND e.payload_json=o.payload_json
                WHERE o.run_id=?1 AND o.effect_key=?2 AND o.effect_type='continuation_model' AND o.status='pending'",
                params![run_id,effect_key],|row|row.get::<_,String>(0))
        })?;
        let payload: serde_json::Value = serde_json::from_str(&body).map_err(|_| "invalid continuation payload")?;
        let input: fox_engine_protocol::KernelInitialModelInput = serde_json::from_value(payload["input"].clone())
            .map_err(|_| "invalid stored continuation input")?;
        let lane = payload["lane"].as_str().map(str::to_owned);
        let mut frozen_notices=Vec::new();
        if lane.as_deref() == Some("job_notice") {
            input.validate_job_notice()?;
            frozen_notices = serde_json::from_value(
                payload["hostJobNotices"].clone()).map_err(|_|"job_notice has no typed Host facts")?;
            fox_engine_protocol::validate_host_job_notices(&frozen_notices)?;
            if frozen_notices.is_empty() || payload["prompt"] != "" {
                return Err("job_notice has no bounded typed Host fact or used a synthetic prompt".into());
            }
        } else {
            input.validate()?;
        }
        let original = self.kernel_initial_input(run_id)?;
        if input.run_id != run_id || input.turn_id != original.turn_id
            || input.prompt_config_hash != original.prompt_config_hash || payload["effectKey"] != effect_key {
            return Err("continuation frozen input changed".into());
        }
        Ok((input,lane,frozen_notices))
    }

    /// Lease pending outbox effects for one run, atomically marking them leased.
    /// Each lease uses the same stable idempotency key, so re-leasing after a
    /// crash never implies a second logical side effect.
    pub fn kernel_outbox_lease_pending(
        &self,
        run_id: &str,
        lease_owner: &str,
    ) -> Result<Vec<crate::kernel::OutboxEffect>, String> {
        if lease_owner.trim().is_empty() {
            return Err("kernel outbox lease owner must be non-empty".into());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut statement = transaction.prepare(
                "SELECT o.effect_key, o.effect_type, o.idempotency_key, o.tool_call_id,
                        o.batch_id, o.payload_json, o.status, o.attempts
                   FROM kernel_effect_outbox o
                   JOIN kernel_runs r ON r.run_id=o.run_id
                  WHERE o.run_id=?1 AND o.status='pending'
                    AND o.effect_type<>'initial_model'
                    AND r.kernel_mode = 'authoritative'
                    AND (
                        r.state NOT IN ('cancelling','completed','failed','cancelled','budget_exhausted','approval_expired')
                        OR o.effect_type IN ('cancel_engine_turn','cancel_tool_call')
                    )
                  ORDER BY o.created_at ASC, o.effect_key ASC",
            )?;
            let mut effects = statement
                .query_map(params![run_id], map_outbox_effect)?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let now = now_ms();
            for effect in &mut effects {
                let affected = transaction.execute(
                    "UPDATE kernel_effect_outbox
                        SET status='leased', lease_owner=?2, leased_at=?3,
                            attempts=attempts+1, updated_at=?3
                      WHERE run_id=?1 AND effect_key=?4 AND status='pending'",
                    params![run_id, lease_owner, now, effect.effect_key],
                )?;
                if affected != 1 {
                    return Err(kernel_err("kernel outbox lease lost its pending CAS"));
                }
                effect.status = crate::kernel::OutboxStatus::Leased;
                effect.attempts = effect.attempts.saturating_add(1);
            }
            transaction.commit()?;
            Ok(effects)
        })
    }

    /// Claim one observed effect only when its executor is ready. Unrelated
    /// pending work remains recoverable if this process stops before dispatch.
    pub fn kernel_outbox_lease_effect(
        &self,
        run_id: &str,
        effect_key: &str,
        lease_owner: &str,
    ) -> Result<Option<crate::kernel::OutboxEffect>, String> {
        if lease_owner.trim().is_empty() { return Err("kernel outbox lease owner must be non-empty".into()); }
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let effect = transaction.query_row(
                "SELECT o.effect_key, o.effect_type, o.idempotency_key, o.tool_call_id,
                        o.batch_id, o.payload_json, o.status, o.attempts
                 FROM kernel_effect_outbox o JOIN kernel_runs r ON r.run_id=o.run_id
                 WHERE o.run_id=?1 AND o.effect_key=?2 AND o.status='pending'
                   AND o.effect_type<>'initial_model'
                   AND r.kernel_mode='authoritative'
                   AND (r.state NOT IN ('cancelling','completed','failed','cancelled','budget_exhausted','approval_expired')
                        OR o.effect_type IN ('cancel_engine_turn','cancel_tool_call'))",
                params![run_id, effect_key], map_outbox_effect,
            ).optional()?;
            let Some(mut effect) = effect else { return Ok(None); };
            let changed = transaction.execute(
                "UPDATE kernel_effect_outbox SET status='leased',lease_owner=?3,leased_at=?4,
                     attempts=attempts+1,updated_at=?4
                 WHERE run_id=?1 AND effect_key=?2 AND status='pending'",
                params![run_id, effect_key, lease_owner, now_ms()],
            )?;
            if changed != 1 { return Err(kernel_err("kernel single-effect lease lost pending CAS")); }
            effect.status = crate::kernel::OutboxStatus::Leased;
            effect.attempts = effect.attempts.saturating_add(1);
            transaction.commit()?;
            Ok(Some(effect))
        })
    }

    /// Lease pending outbox effects across authoritative runs (startup).
    pub fn kernel_outbox_lease_all_pending(
        &self,
        lease_owner: &str,
    ) -> Result<Vec<(String, crate::kernel::OutboxEffect)>, String> {
        if lease_owner.trim().is_empty() {
            return Err("kernel outbox lease owner must be non-empty".into());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut statement = transaction.prepare(
                "SELECT o.run_id, o.effect_key, o.effect_type, o.idempotency_key, o.tool_call_id,
                        o.batch_id, o.payload_json, o.status, o.attempts
                   FROM kernel_effect_outbox o
                   JOIN kernel_runs r ON r.run_id = o.run_id
                  WHERE o.status='pending'
                    AND o.effect_type<>'initial_model'
                    AND r.kernel_mode = 'authoritative'
                    AND (
                        r.state NOT IN ('cancelling','completed','failed','cancelled','budget_exhausted','approval_expired')
                        OR o.effect_type IN ('cancel_engine_turn','cancel_tool_call')
                    )
                  ORDER BY o.created_at ASC, o.effect_key ASC",
            )?;
            let mut rows = statement
                .query_map([], |row| {
                    let run_id: String = row.get(0)?;
                    Ok((run_id, map_outbox_effect_row_at(row, 1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let now = now_ms();
            for (run_id, effect) in &mut rows {
                let affected = transaction.execute(
                    "UPDATE kernel_effect_outbox
                        SET status='leased', lease_owner=?3, leased_at=?4,
                            attempts=attempts+1, updated_at=?4
                      WHERE run_id=?1 AND effect_key=?2 AND status='pending'",
                    params![run_id.as_str(), effect.effect_key, lease_owner, now],
                )?;
                if affected != 1 {
                    return Err(kernel_err("kernel outbox lease lost its pending CAS"));
                }
                effect.status = crate::kernel::OutboxStatus::Leased;
                effect.attempts = effect.attempts.saturating_add(1);
            }
            transaction.commit()?;
            Ok(rows)
        })
    }

    /// Effects left in `leased` by a process that crashed mid-execution. These
    /// must NOT be blindly re-run; the executor reconciles by idempotency key or
    /// the run is moved to explicit reconciliation / failure.
    pub fn kernel_outbox_stuck_leased(
        &self,
    ) -> Result<Vec<(String, crate::kernel::OutboxEffect)>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                        payload_json, status, attempts
                   FROM kernel_effect_outbox
                  WHERE status='leased'
                  ORDER BY leased_at ASC",
            )?;
            let rows = statement
                .query_map([], |row| {
                    let run_id: String = row.get(0)?;
                    Ok((run_id, map_outbox_effect_row_at(row, 1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn kernel_outbox_complete(
        &self,
        run_id: &str,
        effect_key: &str,
        lease_owner: &str,
    ) -> Result<(), String> {
        if lease_owner.trim().is_empty() {
            return Err("kernel outbox lease owner must be non-empty".into());
        }
        self.with_connection(|connection| {
            let affected = connection.execute(
                "UPDATE kernel_effect_outbox
                    SET status='completed', completed_at=?3, updated_at=?3, lease_owner=NULL
                  WHERE run_id=?1 AND effect_key=?2 AND status='leased' AND lease_owner=?4",
                params![run_id, effect_key, now_ms(), lease_owner],
            )?;
            if affected != 1 {
                return Err(kernel_err(format!(
                    "kernel outbox completion lost lease ownership for {run_id}/{effect_key}"
                )));
            }
            Ok(())
        })
    }

    pub fn kernel_outbox_fail(
        &self,
        run_id: &str,
        effect_key: &str,
        error: &str,
        lease_owner: &str,
    ) -> Result<(), String> {
        if lease_owner.trim().is_empty() {
            return Err("kernel outbox lease owner must be non-empty".into());
        }
        self.with_connection(|connection| {
            let affected = connection.execute(
                "UPDATE kernel_effect_outbox
                    SET status='failed', last_error=?3, updated_at=?4, lease_owner=NULL
                  WHERE run_id=?1 AND effect_key=?2 AND status='leased' AND lease_owner=?5",
                params![run_id, effect_key, error, now_ms(), lease_owner],
            )?;
            if affected != 1 {
                return Err(kernel_err(format!(
                    "kernel outbox failure lost lease ownership for {run_id}/{effect_key}"
                )));
            }
            Ok(())
        })
    }

    /// Mark an effect that cannot be safely reconciled as failed, so it is never
    /// silently retried into a duplicate side effect.
    pub fn kernel_outbox_mark_requires_reconcile(
        &self,
        run_id: &str,
        effect_key: &str,
        expected_lease_owner: &str,
    ) -> Result<(), String> {
        if expected_lease_owner.trim().is_empty() {
            return Err("kernel reconcile lease owner must be non-empty".into());
        }
        self.with_connection(|connection| {
            let affected = connection.execute(
                "UPDATE kernel_effect_outbox
                    SET status='failed',
                        last_error='requires_reconcile: side effect uncertain after crash',
                        updated_at=?3, lease_owner=NULL
                  WHERE run_id=?1 AND effect_key=?2 AND status='leased' AND lease_owner=?4",
                params![run_id, effect_key, now_ms(), expected_lease_owner],
            )?;
            if affected != 1 {
                return Err(kernel_err(format!(
                    "kernel reconcile marker lost lease ownership for {run_id}/{effect_key}"
                )));
            }
            Ok(())
        })
    }

    /// Rehydrate a full RunController from durable facts.
    pub fn kernel_rehydrate(
        &self,
        run_id: &str,
    ) -> Result<Option<crate::kernel::RehydratedRun>, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let row: Option<(
                Option<String>,
                String,
                String,
                i64,
                String,
                String,
                String,
                String,
                String,
                i64,
                i64,
                Option<i64>,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
            )> = transaction
                .query_row(
                    "SELECT turn_id, engine_id, kernel_mode, capability_manifest_version,
                            permission_snapshot_id, execution_profile_id, prompt_config_hash,
                            frozen_config_json, state,
                            last_event_seq, running_elapsed_ms, approval_deadline_wall_ms,
                            terminal_written, retry_state_json, compaction_state_json,
                            capability_manifest_hash, model_request_since_wall_ms,
                            wait_deadline_wall_ms, wait_accounted_until_wall_ms
                       FROM kernel_runs WHERE run_id=?1",
                    params![run_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                            row.get(10)?,
                            row.get(11)?,
                            row.get(12)?,
                            row.get(13)?,
                            row.get(14)?,
                            row.get(15)?,
                            row.get(16)?,
                            row.get(17)?,
                            row.get(18)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                turn_id,
                engine_id,
                kernel_mode,
                capability_manifest_version,
                permission_snapshot_id,
                execution_profile_id,
                prompt_config_hash,
                frozen_config_json,
                state_str,
                last_seq,
                elapsed,
                deadline,
                terminal_written,
                retry_json,
                compaction_json,
                manifest_hash,
                model_request_since_wall_ms,
                wait_deadline_wall_ms,
                wait_accounted_until_wall_ms,
            )) = row
            else {
                return Ok(None);
            };
            let config: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen_config_json)
                .map_err(|error| {
                    kernel_err(format!("invalid frozen config for {run_id}: {error}"))
                })?;
            if config.engine_id != engine_id
                || config.kernel_mode != kernel_mode
                || i64::from(config.capability_manifest_version) != capability_manifest_version
                || config.permission_snapshot_id != permission_snapshot_id
                || config.execution_profile_id != execution_profile_id
                || config.prompt_config_hash != prompt_config_hash
                || manifest_hash.as_deref() != Some(config.capability_manifest_hash.as_str())
            {
                return Err(kernel_err(format!(
                    "persisted run columns disagree with frozen config for {run_id}"
                )));
            }
            let state = crate::kernel::RunState::parse(&state_str)
                .ok_or_else(|| kernel_err(format!("unknown persisted run state: {state_str}")))?;
            if state == crate::kernel::RunState::WaitingJobs {
                let (park_seq, park_json): (i64,String) = transaction.query_row(
                    "SELECT seq,payload_json FROM kernel_events
                      WHERE run_id=?1 AND event_type='run.waiting_jobs' ORDER BY seq DESC LIMIT 1",
                    [run_id], |row| Ok((row.get(0)?,row.get(1)?)))?;
                let park: serde_json::Value = serde_json::from_str(&park_json)
                    .map_err(|_| kernel_err("invalid durable job wait event"))?;
                let woke_after_park: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM kernel_events WHERE run_id=?1
                      AND seq>?2 AND event_type='run.jobs_woken')",
                    params![run_id,park_seq], |row| row.get(0))?;
                if park["dataRootId"] != self.data_root_id
                    || wait_deadline_wall_ms != park["waitDeadlineWallMs"].as_i64()
                    || wait_accounted_until_wall_ms.is_none_or(|accounted|
                        accounted < park["parkedAtWallMs"].as_i64().unwrap_or(i64::MAX)
                        || accounted > wait_deadline_wall_ms.unwrap_or(0))
                    || elapsed < park["runningElapsedMs"].as_i64().unwrap_or(i64::MAX)
                    || woke_after_park {
                    return Err(kernel_err("job wait root, cursor or wake history changed"));
                }
                let jobs: Vec<crate::kernel::WaitingJobFact> =
                    serde_json::from_value(park["jobs"].clone())
                        .map_err(|_| kernel_err("invalid frozen waiting Job identities"))?;
                for job in jobs {
                    let current: Option<(i64,Option<i64>)> = transaction.query_row(
                        "SELECT attempts,deadline_ms FROM kernel_jobs WHERE run_id=?1 AND job_id=?2",
                        params![run_id,job.job_id], |row| Ok((row.get(0)?,row.get(1)?))
                    ).optional()?;
                    if current != Some((job.attempt,Some(job.deadline_wall_ms))) {
                        return Err(kernel_err("frozen Job attempt or deadline changed on reopen"));
                    }
                }
            }

            let mut batch_statement = transaction.prepare(
                "SELECT batch_id, ordered_tool_call_ids_json, barrier_emitted
                   FROM kernel_tool_batches WHERE run_id=?1
                  ORDER BY created_at ASC, batch_id ASC",
            )?;
            let batches = batch_statement
                .query_map(params![run_id], |row| {
                    let ordered: String = row.get(1)?;
                    let ordered: Vec<String> = serde_json::from_str(&ordered).map_err(|error| {
                        kernel_err(format!("invalid persisted batch ordering JSON: {error}"))
                    })?;
                    Ok(crate::kernel::RehydratedBatch {
                        batch_id: row.get(0)?,
                        ordered,
                        barrier_emitted: row.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(batch_statement);

            let mut tool_statement = transaction.prepare(
                "SELECT tool_call_id, batch_id, tool, source_order, canonical_input_json,
                        state, result_json
                   FROM kernel_tool_calls WHERE run_id=?1
                  ORDER BY batch_id ASC, source_order ASC",
            )?;
            let tools = tool_statement
                .query_map(params![run_id], |row| {
                    let state_str: String = row.get(5)?;
                    let tool_state =
                        crate::kernel::ToolCallState::parse(&state_str).ok_or_else(|| {
                            kernel_err(format!("unknown persisted tool state: {state_str}"))
                        })?;
                    let source_order: i64 = row.get(3)?;
                    if source_order < 0 {
                        return Err(kernel_err("negative persisted tool source order"));
                    }
                    Ok(crate::kernel::RehydratedToolCall {
                        tool_call_id: row.get(0)?,
                        batch_id: row.get(1)?,
                        tool: row.get(2)?,
                        source_order: usize::try_from(source_order)
                            .map_err(|_| kernel_err("tool source order exceeds usize"))?,
                        input_json: row.get(4)?,
                        state: tool_state,
                        result_json: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            drop(tool_statement);

            let approval_mismatches = {
                let mut statement = transaction.prepare(
                    "SELECT c.tool_call_id,c.state,a.state,a.decided_at,a.policy_version
                       FROM kernel_tool_calls c
                       LEFT JOIN kernel_approvals a
                         ON a.run_id=c.run_id AND a.tool_call_id=c.tool_call_id
                      WHERE c.run_id=?1 AND (
                        (c.state='waiting_approval' AND COALESCE(a.state,'')<>'pending')
                        OR (c.state<>'waiting_approval' AND a.state='pending')
                      )",
                )?;
                let values = statement
                    .query_map(params![run_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<u64>>(4)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                values
            };
            for (tool_call_id, tool_state, approval_state, decided_at, ticket_version) in
                approval_mismatches
            {
                // A live policy change expires an unused ticket without deciding
                // it. Keep that exact, unconsumed intent rehydratable so the
                // explicit re-evaluation command can archive the old ticket and
                // install its replacement atomically. This is deliberately much
                // narrower than accepting any expired approval: a human/timeout
                // decision has decided_at, an unchanged/future ticket version is
                // invalid, and any dispatch/credential/attempt consumes the intent.
                let stale_policy_wait = if tool_state == "waiting_approval"
                    && approval_state.as_deref() == Some("expired")
                    && decided_at.is_none()
                    && ticket_version.is_some()
                {
                    let current_version: Option<u64> = transaction
                        .query_row(
                            "SELECT p.version
                               FROM runs r
                               JOIN kernel_execution_policies p
                                 ON p.conversation_id=r.conversation_id
                              WHERE r.id=?1",
                            params![run_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let dispatch_id = fox_engine_protocol::encode_dispatch_id(
                        run_id,
                        &tool_call_id,
                    )
                    .map_err(kernel_err)?;
                    let (has_original_prompt, consumed): (bool, bool) = transaction.query_row(
                        "SELECT
                            EXISTS(
                                SELECT 1 FROM kernel_effect_outbox
                                 WHERE run_id=?1 AND tool_call_id=?2
                                   AND effect_type='request_approval'
                                   AND status IN ('pending','leased','completed')
                            ),
                            EXISTS(
                                SELECT 1 FROM kernel_effect_outbox
                                 WHERE run_id=?1 AND tool_call_id=?2
                                   AND effect_type='dispatch_tool'
                            )
                            OR EXISTS(
                                SELECT 1 FROM kernel_execution_credentials
                                 WHERE run_id=?1 AND dispatch_id=?3
                            )
                            OR EXISTS(
                                SELECT 1 FROM kernel_execution_attempts
                                 WHERE run_id=?1 AND dispatch_id=?3
                            )",
                        params![run_id, tool_call_id, dispatch_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    current_version.is_some_and(|current| {
                        current > ticket_version.expect("checked above")
                    }) && has_original_prompt
                        && !consumed
                } else {
                    false
                };
                if !stale_policy_wait {
                    return Err(kernel_err(format!(
                        "persisted approval facts disagree with tool states for {run_id}"
                    )));
                }
            }
            let running_without_dispatch: i64 = transaction.query_row(
                "SELECT COUNT(*)
                   FROM kernel_tool_calls c
                   LEFT JOIN kernel_effect_outbox o
                     ON o.run_id=c.run_id
                    AND o.effect_key=('dispatch:' || c.tool_call_id)
                    AND o.effect_type='dispatch_tool'
                    AND o.tool_call_id=c.tool_call_id
                  WHERE c.run_id=?1 AND c.state='running' AND o.effect_key IS NULL",
                params![run_id],
                |row| row.get(0),
            )?;
            if running_without_dispatch != 0 {
                return Err(kernel_err(format!(
                    "running tool is missing its durable dispatch for {run_id}"
                )));
            }
            for batch in &batches {
                if batch.barrier_emitted {
                    let delivery: Option<(String, String)> = transaction
                        .query_row(
                            "SELECT effect_type, payload_json FROM kernel_effect_outbox
                              WHERE run_id=?1 AND effect_key=?2 AND batch_id=?3",
                            params![
                                run_id,
                                crate::kernel::batch_delivery_effect_key(&batch.batch_id),
                                batch.batch_id
                            ],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?;
                    let expected = serde_json::json!({
                        "orderedToolCallIds": batch.ordered
                    })
                    .canonical_string();
                    if delivery != Some(("deliver_tool_batch".to_string(), expected)) {
                        return Err(kernel_err(format!(
                            "settled batch is missing exact durable delivery for {run_id}/{}",
                            batch.batch_id
                        )));
                    }
                }
            }

            let retry: crate::kernel::RetryState = serde_json::from_str(
                retry_json
                    .as_deref()
                    .ok_or_else(|| kernel_err(format!("missing retry state for {run_id}")))?,
            )
            .map_err(|error| kernel_err(format!("invalid retry state for {run_id}: {error}")))?;
            let compaction: crate::kernel::CompactionState = serde_json::from_str(
                compaction_json
                    .as_deref()
                    .ok_or_else(|| kernel_err(format!("missing compaction state for {run_id}")))?,
            )
            .map_err(|error| {
                kernel_err(format!("invalid compaction state for {run_id}: {error}"))
            })?;

            if (terminal_written != 0) != state.is_terminal() {
                return Err(kernel_err(format!(
                    "rehydrate inconsistency for {run_id}: terminal flag disagrees with {state_str}"
                )));
            }
            if last_seq < 0 || elapsed < 0 {
                return Err(kernel_err(format!(
                    "negative persisted cursor or elapsed time for {run_id}"
                )));
            }
            let actual_last_seq: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM kernel_events WHERE run_id=?1",
                params![run_id],
                |row| row.get(0),
            )?;
            if actual_last_seq != last_seq {
                return Err(kernel_err(format!(
                    "persisted event cursor disagrees with stream for {run_id}"
                )));
            }
            if deadline.is_some_and(|value| value <= 0) {
                return Err(kernel_err(format!(
                    "invalid persisted approval deadline for {run_id}"
                )));
            }
            let turn_id = turn_id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| kernel_err(format!("missing persisted turn id for {run_id}")))?;
            let approval_deadline_wall_ms = deadline;
            let rehydrated = crate::kernel::RehydratedRun {
                run_id: run_id.to_string(),
                turn_id,
                config,
                state,
                seq: last_seq.max(0) as u64,
                running_elapsed_ms: elapsed,
                approval_deadline_wall_ms,
                wait_deadline_wall_ms,
                wait_accounted_until_wall_ms,
                terminal_written: terminal_written != 0,
                batches,
                tools,
                retry,
                compaction,
                model_request_since_wall_ms,
            };
            crate::kernel::RunController::rehydrate(rehydrated.clone()).map_err(|error| {
                kernel_err(format!(
                    "invalid rehydrated controller for {run_id}: {error}"
                ))
            })?;
            transaction.commit()?;
            Ok(Some(rehydrated))
        })
    }

    /// Rebuild the authoritative read model for a run from durable facts. React
    /// reads this snapshot and never infers run state from scattered events.
    pub fn kernel_build_snapshot(
        &self,
        run_id: &str,
    ) -> Result<crate::kernel::RunSnapshot, String> {
        self.with_connection(|connection| {
            let (state, engine_id, kernel_mode, last_seq): (String, String, String, i64) =
                connection.query_row(
                    "SELECT state, engine_id, kernel_mode, last_event_seq
                       FROM kernel_runs WHERE run_id = ?1",
                    params![run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;

            let mut waiting = Vec::new();
            let mut pending = Vec::new();
            let mut completed = Vec::new();
            {
                let mut statement = connection.prepare(
                    "SELECT calls.tool_call_id, calls.state
                       FROM kernel_tool_calls AS calls
                       JOIN kernel_tool_batches AS batches
                         ON batches.batch_id = calls.batch_id AND batches.run_id = calls.run_id
                      WHERE calls.run_id = ?1
                      ORDER BY batches.created_at ASC, batches.batch_id ASC, calls.source_order ASC",
                )?;
                let rows = statement.query_map(params![run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (id, state) = row?;
                    match state.as_str() {
                        "waiting_approval" => waiting.push(id),
                        "completed" => completed.push(id),
                        "pending" | "running" => pending.push(id),
                        _ => {}
                    }
                }
            }
            Ok(crate::kernel::RunSnapshot {
                run_id: run_id.to_string(),
                state,
                engine_id,
                kernel_mode,
                waiting_approval_tool_call_ids: waiting,
                pending_tool_call_ids: pending,
                completed_tool_call_ids: completed,
                last_event_seq: last_seq.max(0) as u64,
            })
        })
    }

    /// Build the rich read model from a SINGLE consistent read transaction: no
    /// row is read from a different commit point than another. Contains batch
    /// ordering, every tool call's state/result/error/approval, open outbox
    /// effects, and retry/compaction accounting.
    pub fn kernel_build_full_snapshot(
        &self,
        run_id: &str,
    ) -> Result<crate::kernel::KernelSnapshot, String> {
        self.kernel_build_scoped_snapshot(run_id, None)?
            .ok_or_else(|| "kernel run not found".to_string())
    }

    /// Conversation ownership and all authoritative facts share one read transaction.
    /// Missing executable Kernel rows mean Legacy, never an inferred authority switch.
    pub fn kernel_conversation_snapshot(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<fox_engine_protocol::KernelRunSnapshot>, String> {
        Ok(self.kernel_build_scoped_snapshot(run_id, Some(conversation_id))?.map(|s| {
            fox_engine_protocol::KernelRunSnapshot {
                schema_version: 1,
                run_id: s.run_id,
                turn_id: s.turn_id.expect("snapshot validates turn identity"),
                engine_id: s.engine_id,
                state: s.state,
                last_event_seq: s.last_event_seq.to_string(),
                running_elapsed_ms: s.running_elapsed_ms,
                approval_deadline_wall_ms: s.approval_deadline_wall_ms,
                terminal_written: s.terminal_written,
                tools: s.tool_calls.into_iter().map(|t| fox_engine_protocol::KernelToolSnapshot {
                    tool_call_id: t.tool_call_id,
                    batch_id: t.batch_id,
                    tool: t.tool,
                    source_order: t.source_order,
                    state: t.state,
                    approval_state: t.approval_state,
                }).collect(),
                provider_attempts: s.retry.provider_attempts,
                turn_attempts: s.retry.turn_attempts,
                retry_due_wall_ms: s.retry.due_wall_ms,
                compactions: s.compaction.compactions,
            }
        }))
    }

    fn kernel_build_scoped_snapshot(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<Option<crate::kernel::KernelSnapshot>, String> {
        self.with_connection(|connection| {
            let txn = connection.transaction()?;
            if let Some(conversation_id) = conversation_id {
                let owned: bool = txn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND conversation_id=?2)",
                    params![run_id, conversation_id], |row| row.get(0),
                )?;
                if !owned { return Err(kernel_err("kernel snapshot run does not belong to conversation")); }
                let authoritative: bool = txn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM kernel_runs WHERE run_id=?1 AND kernel_mode='authoritative')",
                    params![run_id], |row| row.get(0),
                )?;
                if !authoritative { return Ok(None); }
            }
            let (
                state,
                engine_id,
                kernel_mode,
                last_seq,
                turn_id,
                elapsed,
                deadline,
                terminal_written,
                manifest_hash,
                retry_json,
                compaction_json,
                capability_manifest_version,
                permission_snapshot_id,
                execution_profile_id,
                prompt_config_hash,
                frozen_config_json,
            ): (
                String,
                String,
                String,
                i64,
                Option<String>,
                i64,
                i64,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                i64,
                String,
                String,
                String,
                String,
            ) = txn.query_row(
                "SELECT state, engine_id, kernel_mode, last_event_seq, turn_id,
                        running_elapsed_ms, COALESCE(approval_deadline_wall_ms,0),
                        terminal_written, capability_manifest_hash,
                        retry_state_json, compaction_state_json,
                        capability_manifest_version, permission_snapshot_id,
                        execution_profile_id, prompt_config_hash, frozen_config_json
                   FROM kernel_runs WHERE run_id=?1",
                params![run_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                        row.get(14)?,
                        row.get(15)?,
                    ))
                },
            )?;

            let frozen: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen_config_json)
                .map_err(|error| {
                    kernel_err(format!(
                        "invalid snapshot frozen config for {run_id}: {error}"
                    ))
                })?;
            crate::kernel::RunController::validate_config(&frozen)
                .map_err(|error| kernel_err(error.to_string()))?;
            if frozen.engine_id != engine_id
                || frozen.kernel_mode != kernel_mode
                || i64::from(frozen.capability_manifest_version) != capability_manifest_version
                || frozen.permission_snapshot_id != permission_snapshot_id
                || frozen.execution_profile_id != execution_profile_id
                || frozen.prompt_config_hash != prompt_config_hash
                || manifest_hash.as_deref() != Some(frozen.capability_manifest_hash.as_str())
            {
                return Err(kernel_err(format!(
                    "snapshot columns disagree with frozen config for {run_id}"
                )));
            }

            let mut batches = Vec::new();
            {
                let mut st = txn.prepare(
                    "SELECT batch_id, ordered_tool_call_ids_json, barrier_emitted
                       FROM kernel_tool_batches WHERE run_id=?1
                      ORDER BY created_at ASC, batch_id ASC",
                )?;
                for row in st.query_map(params![run_id], |row| {
                    let ordered: String = row.get(1)?;
                    let ordered_tool_call_ids =
                        serde_json::from_str(&ordered).map_err(|error| {
                            kernel_err(format!("invalid snapshot batch ordering JSON: {error}"))
                        })?;
                    Ok(crate::kernel::SnapshotToolBatch {
                        batch_id: row.get(0)?,
                        ordered_tool_call_ids,
                        barrier_emitted: row.get::<_, i64>(2)? != 0,
                    })
                })? {
                    batches.push(row?);
                }
            }

            let mut tool_calls = Vec::new();
            {
                let mut st = txn.prepare(
                    "SELECT c.tool_call_id, c.batch_id, c.tool, c.source_order, c.state,
                            c.result_json, c.dispatch_idempotency_key, a.state
                       FROM kernel_tool_calls c
                       LEFT JOIN kernel_approvals a
                         ON a.run_id = c.run_id AND a.tool_call_id = c.tool_call_id
                      WHERE c.run_id=?1
                      ORDER BY c.batch_id ASC, c.source_order ASC",
                )?;
                for row in st.query_map(params![run_id], |row| {
                    let source_order: i64 = row.get(3)?;
                    if source_order < 0 {
                        return Err(kernel_err("negative snapshot tool source order"));
                    }
                    Ok(crate::kernel::SnapshotToolCall {
                        tool_call_id: row.get(0)?,
                        batch_id: row.get(1)?,
                        tool: row.get(2)?,
                        source_order: usize::try_from(source_order)
                            .map_err(|_| kernel_err("snapshot source order exceeds usize"))?,
                        state: row.get(4)?,
                        result_json: row.get(5)?,
                        dispatch_idempotency_key: row.get(6)?,
                        approval_state: row.get(7)?,
                    })
                })? {
                    tool_calls.push(row?);
                }
            }

            let mut pending_effects = Vec::new();
            {
                let mut st = txn.prepare(
                    "SELECT effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                            payload_json, status, attempts
                       FROM kernel_effect_outbox
                      WHERE run_id=?1 AND status IN ('pending','leased')
                      ORDER BY created_at ASC, effect_key ASC",
                )?;
                for row in st.query_map(params![run_id], map_outbox_effect)? {
                    pending_effects.push(row?);
                }
            }
            let retry: crate::kernel::RetryState = serde_json::from_str(
                retry_json
                    .as_deref()
                    .ok_or_else(|| kernel_err(format!("missing retry state for {run_id}")))?,
            )
            .map_err(|error| kernel_err(format!("invalid retry state for {run_id}: {error}")))?;
            let compaction: crate::kernel::CompactionState = serde_json::from_str(
                compaction_json
                    .as_deref()
                    .ok_or_else(|| kernel_err(format!("missing compaction state for {run_id}")))?,
            )
            .map_err(|error| {
                kernel_err(format!("invalid compaction state for {run_id}: {error}"))
            })?;
            let parsed_state = crate::kernel::RunState::parse(&state)
                .ok_or_else(|| kernel_err(format!("unknown snapshot run state: {state}")))?;
            if parsed_state == crate::kernel::RunState::WaitingJobs {
                let (deadline,accounted): (Option<i64>,Option<i64>) = txn.query_row(
                    "SELECT wait_deadline_wall_ms,wait_accounted_until_wall_ms
                       FROM kernel_runs WHERE run_id=?1", [run_id],
                    |row| Ok((row.get(0)?,row.get(1)?)))?;
                let root: Option<String> = txn.query_row(
                    "SELECT json_extract(payload_json,'$.dataRootId') FROM kernel_events
                      WHERE run_id=?1 AND event_type='run.waiting_jobs' ORDER BY seq DESC LIMIT 1",
                    [run_id], |row| row.get(0)).optional()?.flatten();
                if root.as_deref() != Some(self.data_root_id.as_str())
                    || deadline.is_none_or(|value| value <= 0)
                    || accounted.is_none_or(|value| value <= 0 || value > deadline.unwrap_or(0)) {
                    return Err(kernel_err("snapshot job wait root or accounting projection changed"));
                }
            }
            let actual_last_seq: i64 = txn.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM kernel_events WHERE run_id=?1",
                params![run_id],
                |row| row.get(0),
            )?;
            if last_seq < 0
                || elapsed < 0
                || actual_last_seq != last_seq
                || manifest_hash
                    .as_deref()
                    .is_none_or(|hash| hash.trim().is_empty())
                || turn_id.as_deref().is_none_or(|turn| turn.trim().is_empty())
                || parsed_state.is_terminal() != (terminal_written != 0)
                || (parsed_state == crate::kernel::RunState::WaitingApproval) != (deadline > 0)
            {
                return Err(kernel_err(format!(
                    "inconsistent full snapshot facts for {run_id}"
                )));
            }
            txn.commit()?;
            Ok(Some(crate::kernel::KernelSnapshot {
                run_id: run_id.to_string(),
                turn_id,
                state,
                engine_id,
                kernel_mode,
                capability_manifest_hash: manifest_hash,
                last_event_seq: last_seq as u64,
                running_elapsed_ms: elapsed,
                approval_deadline_wall_ms: if deadline > 0 { Some(deadline) } else { None },
                terminal_written: terminal_written != 0,
                batches,
                tool_calls,
                pending_effects,
                retry,
                compaction,
            }))
        })
    }

    /// Durable inputs for the recovery orchestrator for one run: pending
    /// approvals (to re-publish) and open outbox effects (to redispatch or
    /// reconcile). Read in one consistent transaction.
    pub fn kernel_recovery_facts(
        &self,
        run_id: &str,
    ) -> Result<Option<crate::kernel::RecoveryFacts>, String> {
        self.with_connection(|connection| {
            let txn = connection.transaction()?;
            let state: Option<String> = txn
                .query_row(
                    "SELECT state FROM kernel_runs WHERE run_id=?1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(state) = state else {
                txn.commit()?;
                return Ok(None);
            };
            let mut pending_approvals = Vec::new();
            {
                let mut st = txn.prepare(
                    "SELECT c.tool_call_id, c.tool, c.canonical_input_json
                       FROM kernel_approvals a
                       JOIN kernel_tool_calls c
                         ON c.run_id = a.run_id AND c.tool_call_id = a.tool_call_id
                      WHERE a.run_id=?1 AND a.state='pending'
                      ORDER BY c.source_order ASC",
                )?;
                for row in st.query_map(params![run_id], |row| {
                    Ok(crate::kernel::RecoveredApproval {
                        run_id: run_id.to_string(),
                        tool_call_id: row.get(0)?,
                        tool: row.get(1)?,
                        input_json: row.get(2)?,
                    })
                })? {
                    pending_approvals.push(row?);
                }
            }
            let mut open_outbox = Vec::new();
            {
                let mut st = txn.prepare(
                    "SELECT effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                            payload_json, status, attempts
                       FROM kernel_effect_outbox
                      WHERE run_id=?1 AND status IN ('pending','leased')
                      ORDER BY created_at ASC, effect_key ASC",
                )?;
                for row in st.query_map(params![run_id], map_outbox_effect)? {
                    open_outbox.push(row?);
                }
            }
            txn.commit()?;
            Ok(Some(crate::kernel::RecoveryFacts {
                run_id: run_id.to_string(),
                state,
                pending_approvals,
                open_outbox,
            }))
        })
    }

    /// Rehydrate all kernel runs that are not terminal, with the tool calls that
    /// were waiting for approval. Used at startup to resume pending approvals
    /// without asking the model to regenerate the same tool calls.
    pub fn kernel_recover_nonterminal_runs(&self) -> Result<Vec<KernelRunRecovery>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT run_id, engine_id, kernel_mode, frozen_config_json, state
                   FROM kernel_runs
                  WHERE state NOT IN ('completed','failed','cancelled','budget_exhausted','approval_expired')",
            )?;
            let runs = statement
                .query_map([], |row| {
                    Ok(KernelRunRecovery {
                        run_id: row.get(0)?,
                        engine_id: row.get(1)?,
                        kernel_mode: row.get(2)?,
                        frozen_config_json: row.get(3)?,
                        state: row.get(4)?,
                        pending_tool_calls: Vec::new(),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);

            let mut recovered = Vec::new();
            for mut run in runs {
                let mut tool_statement = connection.prepare(
                    "SELECT calls.tool_call_id, calls.batch_id, calls.tool,
                            calls.source_order, calls.canonical_input_json
                       FROM kernel_tool_calls AS calls
                       JOIN kernel_approvals AS approvals
                         ON approvals.run_id = calls.run_id
                        AND approvals.tool_call_id = calls.tool_call_id
                        AND approvals.state = 'pending'
                      WHERE calls.run_id = ?1 AND calls.state = 'waiting_approval'
                      ORDER BY calls.source_order ASC",
                )?;
                run.pending_tool_calls = tool_statement
                    .query_map(params![run.run_id], |row| {
                        Ok(RecoveredPendingToolCall {
                            tool_call_id: row.get(0)?,
                            batch_id: row.get(1)?,
                            tool: row.get(2)?,
                            source_order: row.get(3)?,
                            canonical_input_json: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                recovered.push(run);
            }
            Ok(recovered)
        })
    }

    // ---- Phase 4A: production Shadow (observation-only, no executable outbox) ----

    /// Create the durable shadow context for a legacy run. Shadow runs are stored
    /// in `kernel_shadow_runs`, NOT in the executable outbox path. Returns false
    /// if a shadow already exists for the legacy run (idempotent).
    pub fn kernel_create_shadow_run(
        &self,
        shadow_run_id: &str,
        legacy_run_id: &str,
        conversation_id: &str,
        turn_id: &str,
        frozen: &crate::kernel::RunFrozenConfig,
    ) -> Result<bool, String> {
        if shadow_run_id.trim().is_empty() || legacy_run_id.trim().is_empty() {
            return Err("shadow run ids must be non-empty".into());
        }
        if crate::kernel::KernelMode::parse(&frozen.kernel_mode)
            != Some(crate::kernel::KernelMode::Shadow)
        {
            return Err(format!(
                "shadow run must use kernel_mode 'shadow', got {}",
                frozen.kernel_mode
            ));
        }
        if frozen.capability_manifest_hash.trim().is_empty() {
            return Err("shadow frozen config requires a capability manifest hash".into());
        }
        let frozen_json = crate::canonical_json::to_string(frozen)
            .map_err(|error| format!("failed to serialize frozen shadow config: {error}"))?;
        let now = now_ms();
        self.with_connection(|connection| {
            // If a shadow context already exists, it MUST be the same identity
            // and frozen config. An identity/config collision is fail-closed, not
            // silently treated as an idempotent success (no INSERT OR IGNORE).
            let existing: Option<(String, String, String, String)> = connection
                .query_row(
                    "SELECT legacy_run_id, conversation_id, turn_id, frozen_config_json
                       FROM kernel_shadow_runs WHERE shadow_run_id = ?1",
                    params![shadow_run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            if let Some((existing_legacy, existing_conversation, existing_turn, existing_frozen)) =
                existing
            {
                if existing_legacy != legacy_run_id
                    || existing_conversation != conversation_id
                    || existing_turn != turn_id
                    || existing_frozen != frozen_json
                {
                    return Err(kernel_err(format!(
                        "shadow run {shadow_run_id} already exists with a different \
                         frozen identity/config; fail-closed"
                    )));
                }
                return Ok(false); // exact replay: idempotent
            }
            connection.execute(
                "INSERT INTO kernel_shadow_runs
                    (shadow_run_id, legacy_run_id, conversation_id, turn_id, engine_id,
                     kernel_mode, capability_manifest_version, capability_manifest_hash,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash,
                     frozen_config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'shadow', ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    shadow_run_id,
                    legacy_run_id,
                    conversation_id,
                    turn_id,
                    frozen.engine_id,
                    frozen.capability_manifest_version as i64,
                    frozen.capability_manifest_hash,
                    frozen.permission_snapshot_id,
                    frozen.execution_profile_id,
                    frozen.prompt_config_hash,
                    frozen_json,
                    now,
                ],
            )?;
            Ok(true)
        })
    }

    /// Append a shadow comparison record. NEVER writes to kernel_effect_outbox,
    /// so a shadow diff can never be leased or executed by the recovery scanner.
    pub fn kernel_record_shadow_diff(
        &self,
        diff: &crate::kernel::ShadowDiff,
    ) -> Result<(), String> {
        let category = diff.category.as_str();
        let legacy_json = crate::canonical_json::to_string(&diff.legacy)
            .map_err(|error| format!("legacy disposition: {error}"))?;
        let kernel_json = crate::canonical_json::to_string(&diff.kernel)
            .map_err(|error| format!("kernel disposition: {error}"))?;
        let detail_json = diff.detail.canonical_string();
        let now = now_ms();
        self.with_connection(|connection| {
            // The shadow run must exist and reference this legacy run.
            let owner: String = connection.query_row(
                "SELECT legacy_run_id FROM kernel_shadow_runs WHERE shadow_run_id = ?1",
                params![diff.shadow_run_id],
                |row| row.get(0),
            )?;
            if owner != diff.legacy_run_id {
                return Err(kernel_err("shadow diff legacy run mismatch"));
            }
            // Diff cursors must be strictly increasing within a shadow run; a
            // replay or out-of-order feed is fail-closed, not silently appended.
            if diff.event_cursor <= 0 {
                return Err(kernel_err("shadow diff event_cursor must be positive"));
            }
            let max_cursor: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(event_cursor), 0) FROM kernel_shadow_diffs
                      WHERE shadow_run_id = ?1",
                    params![diff.shadow_run_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if diff.event_cursor <= max_cursor {
                return Err(kernel_err(format!(
                    "shadow diff event_cursor {} must advance beyond the last cursor {}",
                    diff.event_cursor, max_cursor
                )));
            }
            connection.execute(
                "INSERT INTO kernel_shadow_diffs
                    (shadow_run_id, legacy_run_id, turn_id, event_cursor, event_type,
                     category, legacy_disposition_json, kernel_disposition_json,
                     detail_json, schema_version, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    diff.shadow_run_id,
                    diff.legacy_run_id,
                    diff.turn_id,
                    diff.event_cursor,
                    diff.event_type,
                    category,
                    legacy_json,
                    kernel_json,
                    detail_json,
                    crate::kernel::SHADOW_DIFF_SCHEMA_VERSION,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    /// Atomically commit observer state and its comparison. Never touches executable tables.
    pub fn kernel_commit_shadow_checkpoint(
        &self,
        diff: &crate::kernel::ShadowDiff,
        checkpoint: &str,
        state: &str,
    ) -> Result<(), String> {
        let category = diff.category.as_str();
        let legacy_json = crate::canonical_json::to_string(&diff.legacy)
            .map_err(|error| format!("legacy disposition: {error}"))?;
        let kernel_json = crate::canonical_json::to_string(&diff.kernel)
            .map_err(|error| format!("kernel disposition: {error}"))?;
        let detail_json = diff.detail.canonical_string();
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let connection = &transaction;
            // The shadow run must exist and reference this legacy run.
            let owner: String = connection.query_row(
                "SELECT legacy_run_id FROM kernel_shadow_runs WHERE shadow_run_id = ?1",
                params![diff.shadow_run_id],
                |row| row.get(0),
            )?;
            if owner != diff.legacy_run_id {
                return Err(kernel_err("shadow diff legacy run mismatch"));
            }
            // Diff cursors must be strictly increasing within a shadow run; a
            // replay or out-of-order feed is fail-closed, not silently appended.
            if diff.event_cursor <= 0 {
                return Err(kernel_err("shadow diff event_cursor must be positive"));
            }
            let max_cursor: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(event_cursor), 0) FROM kernel_shadow_diffs
                      WHERE shadow_run_id = ?1",
                    params![diff.shadow_run_id],
                    |row| row.get(0),
                )
                ?;
            if diff.event_cursor <= max_cursor {
                return Err(kernel_err(format!(
                    "shadow diff event_cursor {} must advance beyond the last cursor {}",
                    diff.event_cursor, max_cursor
                )));
            }
            connection.execute(
                "INSERT INTO kernel_shadow_diffs
                    (shadow_run_id, legacy_run_id, turn_id, event_cursor, event_type,
                     category, legacy_disposition_json, kernel_disposition_json,
                     detail_json, schema_version, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    diff.shadow_run_id,
                    diff.legacy_run_id,
                    diff.turn_id,
                    diff.event_cursor,
                    diff.event_type,
                    category,
                    legacy_json,
                    kernel_json,
                    detail_json,
                    crate::kernel::SHADOW_DIFF_SCHEMA_VERSION,
                    now,
                ],
            )?;
            connection.execute(
                "INSERT INTO kernel_shadow_checkpoints(shadow_run_id, state, checkpoint_json, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(shadow_run_id) DO UPDATE SET state=excluded.state, checkpoint_json=excluded.checkpoint_json, updated_at=excluded.updated_at",
                params![diff.shadow_run_id, state, checkpoint, now],
            )?;
            transaction.commit()
        })
    }

    pub fn kernel_save_shadow_checkpoint(
        &self,
        id: &str,
        checkpoint: &str,
        state: &str,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO kernel_shadow_checkpoints(shadow_run_id, state, checkpoint_json, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(shadow_run_id) DO UPDATE SET state=excluded.state, checkpoint_json=excluded.checkpoint_json, updated_at=excluded.updated_at",
                params![id, state, checkpoint, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn kernel_load_shadow_checkpoint(
        &self,
        id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|connection| {
            connection.query_row(
            "SELECT state, checkpoint_json FROM kernel_shadow_checkpoints WHERE shadow_run_id=?1",
            params![id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()
        })
    }

    pub fn kernel_shadow_permission_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        let mode = self.conversation_permission_mode(conversation_id)?;
        let project = self.conversation_project_access(conversation_id)?;
        let grants = self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT tool_name, scope_key FROM conversation_tool_permissions
                 WHERE conversation_id=?1 AND revoked_at IS NULL
                 ORDER BY tool_name, scope_key")?;
            let rows = query.query_map(params![conversation_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        Ok(serde_json::json!({
            "mode": mode, "projectRoot": project.map(|(root, _)| root), "grants": grants,
        }))
    }

    pub fn kernel_shadow_recovery_rows(
        &self,
    ) -> Result<Vec<(String, String, crate::kernel::RunFrozenConfig, String)>, String> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT s.legacy_run_id, s.conversation_id, s.frozen_config_json, r.status
                 FROM kernel_shadow_runs s JOIN runs r ON r.id=s.legacy_run_id
                 LEFT JOIN kernel_shadow_checkpoints c ON c.shadow_run_id=s.shadow_run_id
                 WHERE c.state='active' OR c.state IS NULL ORDER BY s.created_at",
            )?;
            let rows = query.query_map([], |row| {
                let config: String = row.get(2)?;
                let config = serde_json::from_str(&config)
                    .map_err(|e| kernel_err(format!("invalid frozen shadow config: {e}")))?;
                Ok((row.get(0)?, row.get(1)?, config, row.get(3)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Strict read-only cohort gate. Missing observations, failed persistence,
    /// unknown data and non-comparable points NEVER count as agreement.
    /// A passed cohort is evidence for review, not authorization to switch modes.
    pub fn kernel_shadow_exit_gate(
        &self,
        legacy_run_ids: &[String],
    ) -> Result<serde_json::Value, String> {
        let ids: std::collections::BTreeSet<_> = legacy_run_ids.iter().collect();
        let mut reasons = Vec::new();
        let mut counts = std::collections::BTreeMap::<String, i64>::new();
        if ids.is_empty() {
            reasons.push("empty observation cohort".to_string());
        }
        for run_id in &ids {
            let shadow_id = format!("shadow-{run_id}");
            let mut restored = None;
            match self.kernel_load_shadow_checkpoint(&shadow_id)? {
                Some((state, checkpoint)) if state == "closed" => {
                    restored = Some(crate::kernel::ShadowContext::restore(&checkpoint)?);
                }
                Some((state, _)) => reasons.push(format!("{run_id}: observer is {state}")),
                None => reasons.push(format!("{run_id}: missing semantic checkpoint")),
            }
            let diffs = self.kernel_shadow_diffs_for(&shadow_id)?;
            let terminal = diffs
                .iter()
                .rev()
                .find(|diff| diff.event_type.as_deref() == Some("terminal"));
            if let Some(context) = &restored {
                if context.shadow_run_id() != shadow_id
                    || context.legacy_run_id() != run_id.as_str()
                    || context.persisted_cursor()
                        != diffs.last().map(|diff| diff.event_cursor).unwrap_or(0)
                    || terminal
                        .is_some_and(|diff| context.tool_facts() != diff.kernel.tools.as_slice())
                {
                    reasons.push(format!("{run_id}: checkpoint/diff consistency failed"));
                }
            }
            if terminal.is_none() {
                reasons.push(format!("{run_id}: missing terminal comparison"));
            }
            for diff in &diffs {
                *counts
                    .entry(diff.category.as_str().to_string())
                    .or_default() += 1;
                if !diff.is_match() {
                    reasons.push(format!(
                        "{run_id}: {} at cursor {}",
                        diff.category.as_str(),
                        diff.event_cursor
                    ));
                }
            }
            if terminal.is_some_and(|diff| {
                diff.kernel
                    .tools
                    .iter()
                    .any(|tool| !tool.batch_id.starts_with("pi-batch-"))
            }) {
                reasons.push(format!(
                    "{run_id}: actual Pi batch membership is not established"
                ));
            }
            // Model request lifecycle must be a strict begin/settle pairing,
            // not merely "at least one of each". Read the run.phase events in
            // seq order and require every begin to be followed by exactly one
            // settle before another begin, with no duplicate begin, no settle
            // without a begin, and no dangling (un-settled) begin. Two begins
            // and one settle therefore blocks the gate.
            let lifecycle: Result<Vec<String>, String> = self.with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT json_extract(event_json, '$.shadowModelRequest') AS phase
                       FROM run_events
                      WHERE run_id = ?1 AND event_type = 'run.phase'
                        AND json_extract(event_json, '$.shadowModelRequest') IS NOT NULL
                      ORDER BY seq ASC",
                )?;
                let rows = statement.query_map(params![run_id], |row| row.get::<_, String>(0))?;
                let mut phases = Vec::new();
                for row in rows {
                    phases.push(row?);
                }
                Ok(phases)
            });
            match lifecycle {
                Ok(phases) => {
                    let mut open = 0i64;
                    let mut pairs = 0i64;
                    let mut malformed = None;
                    for phase in &phases {
                        match phase.as_str() {
                            "begin" => {
                                if open != 0 {
                                    malformed = Some(
                                        "model request begin while one is already open (duplicate begin)",
                                    );
                                    break;
                                }
                                open += 1;
                            }
                            "settle" => {
                                if open != 1 {
                                    malformed = Some(
                                        "model request settle without an open begin",
                                    );
                                    break;
                                }
                                open = 0;
                                pairs += 1;
                            }
                            other => {
                                malformed = Some("unknown model request lifecycle event");
                                eprintln!("unknown shadowModelRequest phase: {other}");
                                break;
                            }
                        }
                    }
                    if let Some(reason) = malformed {
                        reasons.push(format!("{run_id}: {reason}"));
                    } else if pairs == 0 {
                        reasons.push(format!("{run_id}: missing real model request lifecycle"));
                    } else if open != 0 {
                        reasons.push(format!(
                            "{run_id}: dangling model request (begin without settle)"
                        ));
                    }
                }
                Err(error) => {
                    reasons.push(format!("{run_id}: model request lifecycle unreadable: {error}"));
                }
            }
            if self.kernel_executable_outbox_count(&shadow_id)? != 0
                || self.kernel_executable_outbox_count(run_id)? != 0
            {
                reasons.push(format!("{run_id}: executable outbox is not empty"));
            }
        }
        Ok(serde_json::json!({
            "schemaVersion": 1, "scope": "selected_observed_runs", "threshold": "strict_zero_non_match",
            "sampleCount": ids.len(), "counts": counts, "passed": reasons.is_empty(),
            "blockingReasons": reasons, "authoritativeEnabled": false,
            "productionRolloutApproved": false,
        }))
    }

    pub fn kernel_shadow_recent_exit_gate(&self) -> Result<serde_json::Value, String> {
        let ids = self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT r.id FROM runs r
                 JOIN conversations c ON c.id=r.conversation_id
                 JOIN agents a ON a.id=c.agent_id
                 WHERE a.runtime_type='pi'
                 ORDER BY r.created_at DESC, r.id LIMIT 100",
            )?;
            let rows = query.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        let mut gate = self.kernel_shadow_exit_gate(&ids)?;
        gate["scope"] = serde_json::json!("latest_100_pi_runs_including_missing_observations");
        Ok(gate)
    }

    /// Count of executable outbox rows for a shadow run id. MUST be zero: shadow
    /// effects are never placed in the executable outbox. Used by tests and by
    /// the startup guard.
    pub fn kernel_executable_outbox_count(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
        })
    }

    /// Divergence summary for a shadow run, for the diagnostics read model.
    pub fn kernel_shadow_diff_counts(
        &self,
        shadow_run_id: &str,
    ) -> Result<std::collections::BTreeMap<String, i64>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT category, COUNT(*) FROM kernel_shadow_diffs
                  WHERE shadow_run_id = ?1 GROUP BY category",
            )?;
            let rows = statement.query_map(params![shadow_run_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut map = std::collections::BTreeMap::new();
            for row in rows {
                let (category, count) = row?;
                map.insert(category, count);
            }
            Ok(map)
        })
    }

    /// Shadow run ids present across the observation tables, used by recovery to
    /// rebuild ONLY comparison context (never side effects).
    pub fn kernel_shadow_run_ids(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT shadow_run_id FROM kernel_shadow_runs")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    /// Read all persisted comparison records for a shadow run, ordered by
    /// cursor (oldest first). Used by Host integration tests and recovery to
    /// read back durable diffs and tool facts.
    pub fn kernel_shadow_diffs_for(
        &self,
        shadow_run_id: &str,
    ) -> Result<Vec<crate::kernel::ShadowDiff>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT legacy_run_id, turn_id, event_cursor, event_type, category,
                        legacy_disposition_json, kernel_disposition_json, detail_json
                   FROM kernel_shadow_diffs
                  WHERE shadow_run_id = ?1
                  ORDER BY event_cursor ASC",
            )?;
            let rows = statement.query_map(params![shadow_run_id], |row| {
                let legacy_run_id: String = row.get(0)?;
                let turn_id: Option<String> = row.get(1)?;
                let event_cursor: i64 = row.get(2)?;
                let event_type: Option<String> = row.get(3)?;
                let category: String = row.get(4)?;
                let legacy_json: String = row.get(5)?;
                let kernel_json: String = row.get(6)?;
                let detail_json: String = row.get(7)?;
                let category = crate::kernel::DiffCategory::parse(&category)
                    .ok_or_else(|| kernel_err("unknown shadow diff category"))?;
                let legacy: crate::kernel::LegacyDisposition =
                    serde_json::from_str(&legacy_json)
                        .map_err(|e| kernel_err(format!("invalid legacy disposition: {e}")))?;
                let kernel: crate::kernel::ShadowDisposition =
                    serde_json::from_str(&kernel_json)
                        .map_err(|e| kernel_err(format!("invalid kernel disposition: {e}")))?;
                let detail = serde_json::from_str(&detail_json)
                    .map_err(|e| kernel_err(format!("invalid shadow detail: {e}")))?;
                Ok(crate::kernel::ShadowDiff {
                    shadow_run_id: shadow_run_id.to_string(),
                    legacy_run_id,
                    turn_id,
                    event_cursor,
                    event_type,
                    category,
                    legacy,
                    kernel,
                    detail,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// Highest persisted diff cursor for a shadow run, used to resume a
    /// re-bootstrapped context so new diffs continue past the last persisted
    /// cursor instead of re-inserting cursor 1 (which the store rejects).
    pub fn kernel_shadow_max_cursor(&self, shadow_run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(MAX(event_cursor), 0) FROM kernel_shadow_diffs
                  WHERE shadow_run_id = ?1",
                params![shadow_run_id],
                |row| row.get(0),
            )
        })
    }
}

