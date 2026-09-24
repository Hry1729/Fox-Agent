//! Persistence adapter for the Fox Agent Kernel.
//!
//! This is an adapter: it owns the rusqlite statements for the Kernel tables
//! but contains no decision logic. The pure [`crate::kernel`] core decides state;
//! these methods durably record and reload Kernel facts after a sidecar or
//! application restart. Pending approval and external-effect facts MUST survive
//! restart — they are never held only in an in-memory promise.

use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

/// The exact Host-to-Node frame or live directive, assembled before the model
/// lease. The transaction re-reads its pending notice set before binding it.
pub(crate) struct ModelNoticeInput<'a> {
    pub payload: &'a serde_json::Value,
    /// Live directives omit transcript history; bind the Host's complete
    /// pre-notice model history alongside the directive in the same lease.
    pub live_history: Option<&'a [serde_json::Value]>,
    pub checkpoint_seq: u64,
    pub history_start: usize,
    pub historical_bytes: usize,
}

fn notice_error(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn bind_model_notices(
    tx: &rusqlite::Transaction<'_>, root: &str, conversation: &str, run: &str,
    dispatch_key: &str, owner: &str, input: &ModelNoticeInput<'_>, now: i64,
) -> rusqlite::Result<()> {
    let supplied: Vec<fox_engine_protocol::HostJobNotice> = match input.payload.get("hostJobNotices") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| notice_error("invalid model Host job notices"))?,
        None => Vec::new(),
    };
    fox_engine_protocol::validate_host_job_notices(&supplied)
        .map_err(|_| notice_error("invalid model Host job notice bounds"))?;
    let embedded_history = input.payload.pointer("/input/messages")
        .or_else(|| input.payload.get("history"))
        .and_then(serde_json::Value::as_array);
    if input.live_history.is_some() == embedded_history.is_some() {
        return Err(notice_error("model notice history must have one Host source"));
    }
    let history = input.live_history.or_else(|| embedded_history.map(Vec::as_slice))
        .ok_or_else(|| notice_error("model notice history is missing"))?;
    Database::validate_host_job_notice_history_on(tx,root,conversation,run,history)?;
    let history_bytes = fox_engine_protocol::historical_host_job_notice_bytes(history)
        .map_err(|_| notice_error("invalid historical Host job notice"))?;
    if input.historical_bytes != history_bytes {
        return Err(notice_error("Host notice historical size changed"));
    }
    let persisted_seq: i64 = tx.query_row("SELECT last_event_seq FROM kernel_runs WHERE run_id=?1",[run],|row|row.get(0))?;
    if u64::try_from(persisted_seq).ok() != Some(input.checkpoint_seq)
        || input.payload.get("checkpointSeq").and_then(serde_json::Value::as_u64)
            .or_else(||input.payload.get("previewSeq").and_then(serde_json::Value::as_u64))
            != Some(input.checkpoint_seq) {
        return Err(notice_error("model notice checkpoint changed before lease"));
    }
    if input.live_history.is_some() && input.history_start != history.len() {
        return Err(notice_error("live Host notice position is not the transcript tail"));
    }
    let remaining = (16 * 1024usize).saturating_sub(input.historical_bytes);
    let expected = super::kernel_job_execution::pending_model_facts(
        tx, root, conversation, run, remaining,
    )?;
    if supplied != expected { return Err(notice_error("Host job notice set changed before model lease")); }
    if supplied.is_empty() { return Ok(()); }
    let bound = serde_json::json!({"modelInput":input.payload,"modelHistory":input.live_history,
        "checkpointSeq":input.checkpoint_seq,"historyStart":input.history_start,
        "historicalHostNoticeBytes":input.historical_bytes});
    let input_json = bound.to_string();
    let input_hash = format!("sha256:{}", hex::encode(Sha256::digest(input_json.as_bytes())));
    tx.execute("INSERT INTO kernel_model_notice_inputs
        (run_id,dispatch_key,lease_owner,input_hash,input_json,history_start,historical_bytes,state,bound_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,'bound',?8)",
        params![run,dispatch_key,owner,input_hash,input_json,
            i64::try_from(input.history_start).map_err(|_|notice_error("Host notice position overflow"))?,
            i64::try_from(input.historical_bytes).map_err(|_|notice_error("Host notice size overflow"))?,now])?;
    for (order, notice) in supplied.iter().enumerate() {
        let position = input.history_start.checked_add(order)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| notice_error("Host notice history position overflow"))?;
        tx.execute("INSERT INTO kernel_job_notice_deliveries
            (job_id,dispatch_key,input_hash,history_position,state,bound_at)
            VALUES (?1,?2,?3,?4,'bound',?5)",
            params![notice.job_id, dispatch_key, input_hash, position, now])?;
    }
    Ok(())
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
    let notices: Vec<fox_engine_protocol::HostJobNotice> = serde_json::from_value(
        bound["modelInput"]["hostJobNotices"].clone(),
    ).map_err(|_|notice_error("invalid stored model notice list"))?;
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

    /// Commit one Kernel decision in a single transaction: events, run/tool
    /// state, pending approvals and the external-effect outbox rows are all
    /// written atomically. The caller performs the outbox side effects only
    /// AFTER this commits, so a crash never loses a decision or performs an
    /// effect whose bookkeeping was not durably recorded. A leased effect still
    /// requires idempotency-key reconciliation after a crash.
    pub fn kernel_commit_decision(
        &self,
        run_id: &str,
        wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand,
    ) -> Result<(), String> {
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, None, None, None, None, None, None, None, None)
    }

    /// Commit a tool result only while this executor still owns its dispatch.
    /// Ownership, result, batch barrier and outbox completion share one transaction.
    pub fn kernel_commit_tool_result(
        &self,
        run_id: &str,
        wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand,
        tool_call_id: &str,
        lease_owner: &str,
    ) -> Result<(), String> {
        if lease_owner.trim().is_empty() { return Err("kernel result lease owner must be non-empty".into()); }
        if cmd.settled_dispatch_tool_call_ids.len() != 1 || cmd.settled_dispatch_tool_call_ids[0] != tool_call_id {
            return Err("kernel result must settle exactly its owned tool dispatch".into());
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, Some((tool_call_id, lease_owner)), None, None, None, None, None, None, None)
    }

    /// Claim exactly one pending result delivery and persist its model deadline
    /// in the same transaction. A crash after this point requires reconciliation.
    pub fn kernel_commit_batch_dispatch(&self, run_id: &str, wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand, batch_id: &str, lease_owner: &str,
        model_input: Option<&ModelNoticeInput<'_>>) -> Result<(), String> {
        if batch_id.trim().is_empty() || lease_owner.trim().is_empty()
            || cmd.run_state != crate::kernel::RunState::Running
            || cmd.model_request_since_wall_ms != Some(wall_now_ms)
            || cmd.events.len() != 1 || cmd.events[0].event_type != "engine.batch_dispatched" {
            return Err("invalid Kernel batch dispatch decision".into());
        }
        let payload: serde_json::Value = serde_json::from_str(&cmd.events[0].payload_json).map_err(|error| error.to_string())?;
        if payload["batchId"].as_str() != Some(batch_id)
            || payload["idempotencyKey"] != crate::kernel::batch_delivery_idempotency_key(batch_id)
            || payload["startedAt"].as_i64() != Some(wall_now_ms) {
            return Err("Kernel batch dispatch identity mismatch".into());
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, None, Some((batch_id, lease_owner)), None, None, None, None, None, model_input)
    }

    /// The response, terminal/next-batch decision, and consumed delivery lease
    /// commit together. An uncertain or foreign worker cannot settle this lease.
    pub fn kernel_commit_batch_response(&self, run_id: &str, wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand, batch_id: &str, lease_owner: &str,
        steering: Option<&super::SteeringDecision>) -> Result<(), String> {
        if batch_id.trim().is_empty() || lease_owner.trim().is_empty() || cmd.model_request_since_wall_ms.is_some() {
            return Err("invalid Kernel batch response decision".into());
        }
        let events = cmd.events.iter().filter(|event| event.event_type == "engine.batch_response").collect::<Vec<_>>();
        if events.len() != 1 { return Err("Kernel batch response must contain exactly one original response".into()); }
        let payload: serde_json::Value = serde_json::from_str(&events[0].payload_json).map_err(|error| error.to_string())?;
        let response: fox_engine_protocol::KernelModelResponse = serde_json::from_value(payload["response"].clone()).map_err(|error| error.to_string())?;
        response.validate()?;
        if payload["batchId"].as_str() != Some(batch_id) || response.batch_id != batch_id
            || response.run_id != run_id || response.turn_id != cmd.turn_id {
            return Err("Kernel batch response identity mismatch".into());
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, None, None, Some((batch_id, lease_owner)), None, None, None, steering, None)
    }

    pub(crate) fn kernel_commit_initial_model(&self, run_id: &str, wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand, owner: &str, response: bool,
        steering: Option<&super::SteeringDecision>, model_input: Option<&ModelNoticeInput<'_>>) -> Result<(), String> {
        if owner.trim().is_empty() { return Err("initial model lease owner is empty".into()); }
        if response {
            let events = cmd.events.iter().filter(|event| event.event_type == "engine.initial_response").collect::<Vec<_>>();
            if events.len() != 1 || cmd.model_request_since_wall_ms.is_some() { return Err("invalid initial model response decision".into()); }
            let payload: serde_json::Value = serde_json::from_str(&events[0].payload_json).map_err(|_| "invalid initial response event")?;
            let value: fox_engine_protocol::KernelInitialModelResponse = serde_json::from_value(payload["response"].clone()).map_err(|_| "invalid initial response")?;
            value.validate()?;
            if value.run_id != run_id || value.turn_id != cmd.turn_id { return Err("initial response identity mismatch".into()); }
        } else {
            if cmd.events.len() != 1 || cmd.events[0].event_type != "engine.initial_dispatched"
                || cmd.run_state != crate::kernel::RunState::Running || cmd.model_request_since_wall_ms != Some(wall_now_ms) {
                return Err("invalid initial model dispatch decision".into());
            }
            let payload: serde_json::Value = serde_json::from_str(&cmd.events[0].payload_json).map_err(|_| "invalid initial dispatch event")?;
            if payload["turnId"] != cmd.turn_id || payload["idempotencyKey"] != crate::kernel::INITIAL_MODEL_IDEMPOTENCY_KEY
                || payload["startedAt"].as_i64() != Some(wall_now_ms) { return Err("initial dispatch identity mismatch".into()); }
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, None, None, None, Some((owner, response)), None, None, steering, model_input)
    }

    pub(crate) fn kernel_continuation_input(&self, run_id: &str, effect_key: &str)
        -> Result<fox_engine_protocol::KernelInitialModelInput, String> {
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
        input.validate()?;
        let original = self.kernel_initial_input(run_id)?;
        if input.run_id != run_id || input.turn_id != original.turn_id
            || input.prompt_config_hash != original.prompt_config_hash || payload["effectKey"] != effect_key {
            return Err("continuation frozen input changed".into());
        }
        Ok(input)
    }

    pub(crate) fn kernel_commit_continuation_model(&self, run_id: &str, wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand, effect_key: &str, owner: &str, response: bool,
        steering: Option<&super::SteeringDecision>, model_input: Option<&ModelNoticeInput<'_>>) -> Result<(), String> {
        let event_type = if response { "engine.continuation_response" } else { "engine.continuation_dispatched" };
        let events = cmd.events.iter().filter(|event| event.event_type == event_type).collect::<Vec<_>>();
        if owner.trim().is_empty() || events.len() != 1 || !effect_key.starts_with("continuation:")
            || (!response && (cmd.events.len() != 1 || cmd.run_state != crate::kernel::RunState::Running
                || cmd.model_request_since_wall_ms != Some(wall_now_ms)))
            || (response && cmd.model_request_since_wall_ms.is_some()) {
            return Err("invalid continuation lease decision".into());
        }
        let payload: serde_json::Value = serde_json::from_str(&events[0].payload_json).map_err(|_| "invalid continuation event")?;
        if payload["turnId"] != cmd.turn_id { return Err("continuation turn changed".into()); }
        if response {
            let value: fox_engine_protocol::KernelInitialModelResponse = serde_json::from_value(payload["response"].clone())
                .map_err(|_| "invalid continuation response")?;
            value.validate()?;
            if value.run_id != run_id || value.turn_id != cmd.turn_id { return Err("continuation response identity changed".into()); }
        } else if payload["effectKey"] != effect_key || payload["startedAt"].as_i64() != Some(wall_now_ms) {
            return Err("continuation dispatch identity changed".into());
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id, wall_now_ms, cmd, None, None, None, None, None,
            Some((effect_key, owner, response)), steering, model_input)
    }

    pub(crate) fn kernel_commit_model_retry(&self, run_id: &str, wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand, effect_key: &str, owner: &str) -> Result<(), String> {
        let terminal = cmd.run_state == crate::kernel::RunState::Failed;
        if owner.trim().is_empty() || (!terminal && cmd.run_state != crate::kernel::RunState::RetryScheduled)
            || cmd.retry.model_dispatch_pending == terminal || cmd.model_request_since_wall_ms.is_some()
            || cmd.events.len() != 2 || cmd.events[0].event_type != "engine.model_rejected"
            || cmd.events[1].event_type != if terminal { "run.failed" } else { "run.retrying" } || !cmd.outbox.is_empty() {
            return Err("invalid settled model retry decision".into());
        }
        self.kernel_commit_decision_with_dispatch_lease(run_id,wall_now_ms,cmd,None,None,None,None,Some((effect_key,owner)), None, None, None)
    }

    fn kernel_commit_decision_with_dispatch_lease(
        &self,
        run_id: &str,
        wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand,
        dispatch_lease: Option<(&str, &str)>,
        batch_lease: Option<(&str, &str)>,
        batch_response_lease: Option<(&str, &str)>,
        initial_lease: Option<(&str, bool)>,
        model_retry_lease: Option<(&str, &str)>,
        continuation_lease: Option<(&str, &str, bool)>,
        steering: Option<&super::SteeringDecision>,
        model_input: Option<&ModelNoticeInput<'_>>,
    ) -> Result<(), String> {
        self.kernel_commit_decision_with_dispatch_lease_and_policy(run_id,wall_now_ms,cmd,dispatch_lease,batch_lease,batch_response_lease,initial_lease,model_retry_lease,continuation_lease,steering,None,model_input)
    }

    /// The UI must send the policy version captured when this approval was
    /// displayed. Re-reading it while answering would revive an obsolete ticket.
    pub fn kernel_commit_decision_with_approval_version(&self,run_id:&str,wall_now_ms:i64,cmd:&crate::kernel::KernelPersistCommand,expected_policy_version:u64)->Result<(),String>{
        self.kernel_commit_decision_with_dispatch_lease_and_policy(run_id,wall_now_ms,cmd,None,None,None,None,None,None,None,Some(expected_policy_version),None)
    }

    fn kernel_commit_decision_with_dispatch_lease_and_policy(
        &self,
        run_id: &str,
        wall_now_ms: i64,
        cmd: &crate::kernel::KernelPersistCommand,
        dispatch_lease: Option<(&str, &str)>,
        batch_lease: Option<(&str, &str)>,
        batch_response_lease: Option<(&str, &str)>,
        initial_lease: Option<(&str, bool)>,
        model_retry_lease: Option<(&str, &str)>,
        continuation_lease: Option<(&str, &str, bool)>,
        steering: Option<&super::SteeringDecision>,
        expected_approval_policy_version: Option<u64>,
        model_input: Option<&ModelNoticeInput<'_>>,
    ) -> Result<(), String> {
        if !valid_run_state(cmd.run_state.as_str()) {
            return Err(format!(
                "unsupported kernel run state: {}",
                cmd.run_state.as_str()
            ));
        }
        if run_id.trim().is_empty() || cmd.turn_id.trim().is_empty() {
            return Err("kernel decision run id and turn id must be non-empty".into());
        }
        if cmd.run_state.is_terminal() != cmd.terminal_written {
            return Err("kernel terminal flag must exactly match the terminal run state".into());
        }
        if cmd.running_elapsed_ms < 0 {
            return Err("kernel running elapsed time must not be negative".into());
        }
        let retry_json = serde_json::to_string(&cmd.retry)
            .map_err(|error| format!("serialize kernel retry state: {error}"))?;
        let compaction_json = serde_json::to_string(&cmd.compaction)
            .map_err(|error| format!("serialize kernel compaction state: {error}"))?;
        let changed = self.with_connection(|connection| {
            // A decision always writes (events, run state, leases), and it reads
            // before it writes. A deferred transaction that starts as a reader and
            // then upgrades fails immediately with SQLITE_BUSY_SNAPSHOT —
            // "database is locked", not retried by any busy timeout — when another
            // connection commits in between (an approval written by the UI, a
            // child Run, an observation reader). Taking the write lock up front
            // makes the same contention wait for its turn instead of failing the
            // Run.
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some((effect_key, owner)) = model_retry_lease {
                let payload: serde_json::Value = serde_json::from_str(&cmd.events[0].payload_json).map_err(|_|kernel_err("invalid retry evidence"))?;
                let failure: fox_engine_protocol::KernelModelFailure = serde_json::from_value(payload["failure"].clone()).map_err(|_|kernel_err("invalid retry evidence"))?;
                failure.validate().map_err(kernel_err)?;
                if failure.run_id != run_id || failure.turn_id != cmd.turn_id || payload["effectKey"] != effect_key {
                    return Err(kernel_err("model retry identity mismatch"));
                }
                let old_retry: String = transaction.query_row("SELECT retry_state_json FROM kernel_runs WHERE run_id=?1",[run_id],|row|row.get(0))?;
                let old_retry: crate::kernel::RetryState = serde_json::from_str(&old_retry).map_err(|_|kernel_err("invalid stored retry counters"))?;
                let provider = failure.category == "provider_unavailable";
                let turn_failure = matches!(failure.category.as_str(), "model_timeout" | "model_transport_failure");
                let terminal = cmd.run_state == crate::kernel::RunState::Failed;
                let mut expected_retry = old_retry.clone();
                if !terminal && !provider {
                    if turn_failure {
                        // Timeout retries spend the turn budget, not the
                        // completion budget.
                        expected_retry.turn_attempts = old_retry.turn_attempts + 1;
                    } else {
                        let used = if old_retry.completion_effect_key.as_deref() == Some(effect_key) {
                            old_retry.completion_attempts
                        } else { 0 };
                        expected_retry.completion_effect_key = Some(effect_key.to_owned());
                        expected_retry.completion_attempts = used + 1;
                    }
                }
                if cmd.retry.provider_attempts != old_retry.provider_attempts + u32::from(provider && !terminal)
                    || cmd.retry.turn_attempts != expected_retry.turn_attempts
                    || cmd.retry.completion_effect_key != expected_retry.completion_effect_key
                    || cmd.retry.completion_attempts != expected_retry.completion_attempts
                    || (!terminal && (cmd.retry.scheduled_at_wall_ms != Some(wall_now_ms)
                    || cmd.retry.due_wall_ms.is_none_or(|due| due < wall_now_ms.saturating_add(failure.retry_after_ms.unwrap_or(0) as i64))))
                    || (terminal && (cmd.retry.scheduled_at_wall_ms.is_some() || cmd.retry.due_wall_ms.is_some())) {
                    return Err(kernel_err("model retry counters or server delay were not honored"));
                }
                if terminal {
                    let outcome: serde_json::Value = serde_json::from_str(&cmd.events[1].payload_json).map_err(|_|kernel_err("invalid model failure outcome"))?;
                    let expected = if failure.category == "incomplete_response" { "kernel.model_incomplete" } else { "kernel.model_retry_exhausted" };
                    if outcome["code"] != expected { return Err(kernel_err("invalid settled model failure code")); }
                }
                let changed = transaction.execute("UPDATE kernel_effect_outbox SET status=?6,completed_at=CASE WHEN ?6='completed' THEN ?4 ELSE NULL END,lease_owner=NULL,leased_at=NULL,updated_at=?4
                    WHERE run_id=?1 AND effect_key=?2 AND lease_owner=?3 AND status='leased'
                      AND effect_type IN ('initial_model','deliver_tool_batch','continuation_model')
                      AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.state='running' AND r.model_request_since_wall_ms IS NOT NULL)
                      AND NOT EXISTS(SELECT 1 FROM kernel_host_commands c WHERE c.run_id=?1 AND c.kind='cancel')
                      AND EXISTS(SELECT 1 FROM kernel_events e WHERE e.run_id=?1 AND e.seq=?5
                        AND ((kernel_effect_outbox.effect_type='initial_model' AND e.event_type='engine.initial_dispatched')
                          OR (kernel_effect_outbox.effect_type='continuation_model' AND e.event_type='engine.continuation_dispatched'
                               AND json_extract(e.payload_json,'$.effectKey')=kernel_effect_outbox.effect_key)
                           OR (kernel_effect_outbox.effect_type='deliver_tool_batch' AND e.event_type='engine.batch_dispatched'
                              AND json_extract(e.payload_json,'$.batchId')=kernel_effect_outbox.batch_id)))",
                    params![run_id,effect_key,owner,wall_now_ms,failure.checkpoint_seq+1,if terminal { "completed" } else { "pending" }])?;
                if changed != 1 { return Err(kernel_err("model retry lost its settled dispatch lease or cancellation won")); }
            }
            if let Some((effect_key, owner, response)) = continuation_lease {
                let changed = if response {
                    let event = cmd.events.iter().find(|event| event.event_type == "engine.continuation_response").unwrap();
                    let payload: serde_json::Value = serde_json::from_str(&event.payload_json).map_err(|_| kernel_err("invalid continuation response"))?;
                    let cursor = payload["response"]["checkpointSeq"].as_u64().ok_or_else(|| kernel_err("missing continuation cursor"))?;
                    transaction.execute("UPDATE kernel_effect_outbox SET status='completed',completed_at=?4,updated_at=?4,lease_owner=NULL
                        WHERE run_id=?1 AND effect_key=?2 AND effect_type='continuation_model' AND status='leased' AND lease_owner=?3
                        AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.state='running' AND r.model_request_since_wall_ms IS NOT NULL)
                        AND NOT EXISTS(SELECT 1 FROM kernel_host_commands c WHERE c.run_id=?1 AND c.kind='cancel')
                        AND EXISTS(SELECT 1 FROM kernel_events e WHERE e.run_id=?1 AND e.seq=?5 AND e.event_type='engine.continuation_dispatched'
                            AND json_extract(e.payload_json,'$.effectKey')=?2)", params![run_id,effect_key,owner,wall_now_ms,cursor+1])?
                } else {
                    transaction.execute("UPDATE kernel_effect_outbox SET status='leased',lease_owner=?3,leased_at=?4,updated_at=?4,attempts=attempts+1
                        WHERE run_id=?1 AND effect_key=?2 AND effect_type='continuation_model' AND status='pending'
                        AND idempotency_key=?5 AND json_extract(payload_json,'$.turnId')=?6
                        AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.state='running' AND r.model_request_since_wall_ms IS NULL)
                        AND NOT EXISTS(SELECT 1 FROM kernel_host_commands c WHERE c.run_id=?1 AND c.kind='cancel')
                        AND NOT EXISTS(SELECT 1 FROM kernel_tool_calls t WHERE t.run_id=?1 AND t.state NOT IN ('completed','failed','cancelled'))",
                        params![run_id,effect_key,owner,wall_now_ms,format!("continuation-delivery:{effect_key}"),cmd.turn_id])?
                };
                if changed != 1 { return Err(kernel_err("continuation delivery lost its lease or cancellation won")); }
            }
            if let Some((owner, response)) = initial_lease {
                let input = super::kernel_initial_input::read_input(&transaction, run_id)?;
                if input.turn_id != cmd.turn_id { return Err(kernel_err("initial input turn changed")); }
                let changed = if response {
                    let event = cmd.events.iter().find(|event| event.event_type == "engine.initial_response").unwrap();
                    let payload: serde_json::Value = serde_json::from_str(&event.payload_json).map_err(|_| kernel_err("invalid initial response"))?;
                    let seq = payload["response"]["checkpointSeq"].as_u64().ok_or_else(|| kernel_err("missing initial cursor"))?;
                    transaction.execute("UPDATE kernel_effect_outbox SET status='completed',completed_at=?3,updated_at=?3,lease_owner=NULL
                        WHERE run_id=?1 AND effect_key='initial-model' AND effect_type='initial_model' AND status='leased' AND lease_owner=?2
                        AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.kernel_mode='authoritative'
                            AND r.state='running' AND r.model_request_since_wall_ms IS NOT NULL)
                        AND EXISTS(SELECT 1 FROM kernel_events e WHERE e.run_id=?1 AND e.event_type='engine.initial_dispatched' AND e.seq=?4)",
                        params![run_id,owner,wall_now_ms,seq+1])?
                } else {
                    transaction.execute("UPDATE kernel_effect_outbox SET status='leased',lease_owner=?2,leased_at=?3,updated_at=?3,attempts=attempts+1
                        WHERE run_id=?1 AND effect_key='initial-model' AND effect_type='initial_model' AND status='pending'
                        AND idempotency_key='initial-model-delivery'
                        AND json_extract(payload_json,'$.inputHash')=(SELECT input_hash FROM kernel_initial_inputs WHERE run_id=?1)
                        AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.kernel_mode='authoritative'
                            AND r.state='running' AND r.model_request_since_wall_ms IS NULL)
                        AND NOT EXISTS(SELECT 1 FROM kernel_tool_batches WHERE run_id=?1)", params![run_id,owner,wall_now_ms])?
                };
                if changed != 1 { return Err(kernel_err("initial model delivery is not owned or already consumed")); }
            }
            if let Some((batch_id, lease_owner)) = batch_response_lease {
                let event = cmd.events.iter().find(|event| event.event_type == "engine.batch_response")
                    .ok_or_else(|| kernel_err("missing model response event"))?;
                let payload: serde_json::Value = serde_json::from_str(&event.payload_json)
                    .map_err(|error| kernel_err(error.to_string()))?;
                let checkpoint_seq = payload["response"]["checkpointSeq"].as_u64()
                    .ok_or_else(|| kernel_err("missing model response cursor"))?;
                let changed = transaction.execute(
                    "UPDATE kernel_effect_outbox SET status='completed', completed_at=?4, updated_at=?4, lease_owner=NULL
                     WHERE run_id=?1 AND batch_id=?2 AND effect_key=?5 AND effect_type='deliver_tool_batch'
                       AND status='leased' AND lease_owner=?3
                       AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1
                           AND r.kernel_mode='authoritative' AND r.state='running'
                           AND r.model_request_since_wall_ms IS NOT NULL)
                       AND EXISTS(SELECT 1 FROM kernel_events e WHERE e.run_id=?1
                           AND e.event_type='engine.batch_dispatched' AND e.seq=?6
                           AND json_extract(e.payload_json,'$.batchId')=?2)",
                    params![run_id, batch_id, lease_owner, wall_now_ms, crate::kernel::batch_delivery_effect_key(batch_id), checkpoint_seq + 1],
                )?;
                if changed != 1 { return Err(kernel_err("Kernel batch response lost lease ownership or active model request")); }
            }
            if let Some((batch_id, lease_owner)) = batch_lease {
                let changed = transaction.execute(
                    "UPDATE kernel_effect_outbox SET status='leased', lease_owner=?3, leased_at=?4,
                         attempts=attempts+1, updated_at=?4
                     WHERE run_id=?1 AND batch_id=?2 AND effect_key=?5
                       AND effect_type='deliver_tool_batch' AND status='pending'
                       AND idempotency_key=?6
                       AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1
                           AND r.kernel_mode='authoritative' AND r.state='running'
                           AND r.model_request_since_wall_ms IS NULL)",
                    params![run_id, batch_id, lease_owner, wall_now_ms,
                        crate::kernel::batch_delivery_effect_key(batch_id),
                        crate::kernel::batch_delivery_idempotency_key(batch_id)],
                )?;
                if changed != 1 { return Err(kernel_err("Kernel batch delivery lost pending lease or model request already active")); }
            }
            let notice_dispatch = if initial_lease.is_some_and(|(_, response)| !response) {
                Some(crate::kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.to_owned())
            } else if let Some((effect_key, _, false)) = continuation_lease {
                Some(format!("continuation-delivery:{effect_key}"))
            } else {
                batch_lease.map(|(batch_id, _)| crate::kernel::batch_delivery_idempotency_key(batch_id))
            };
            let notice_response = if initial_lease.is_some_and(|(_, response)| response) {
                Some(crate::kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.to_owned())
            } else if let Some((effect_key, _, true)) = continuation_lease {
                Some(format!("continuation-delivery:{effect_key}"))
            } else {
                batch_response_lease.map(|(batch_id, _)| crate::kernel::batch_delivery_idempotency_key(batch_id))
            };
            let notice_dispatch_owner = initial_lease.filter(|(_, response)| !*response).map(|(owner,_)|owner)
                .or_else(||continuation_lease.filter(|(_,_,response)| !*response).map(|(_,owner,_)|owner))
                .or_else(||batch_lease.map(|(_,owner)|owner));
            let notice_response_owner = initial_lease.filter(|(_, response)| *response).map(|(owner,_)|owner)
                .or_else(||continuation_lease.filter(|(_,_,response)| *response).map(|(_,owner,_)|owner))
                .or_else(||batch_response_lease.map(|(_,owner)|owner));
            if notice_dispatch.is_some() || notice_response.is_some() || model_retry_lease.is_some() {
                let (conversation, frozen): (String, String) = transaction.query_row(
                    "SELECT r.conversation_id,k.frozen_config_json FROM kernel_runs k
                     JOIN runs r ON r.id=k.run_id WHERE k.run_id=?1 AND k.kernel_mode='authoritative'",
                    [run_id], |row| Ok((row.get(0)?,row.get(1)?)),
                )?;
                let config: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen)
                    .map_err(|_| kernel_err("invalid frozen model notice flag"))?;
                if config.experimental_compute_job_notice {
                    if let Some(key) = notice_dispatch.as_deref() {
                        let input = model_input.ok_or_else(|| kernel_err("model notice input is missing"))?;
                        let owner = notice_dispatch_owner.ok_or_else(||kernel_err("missing model notice dispatch owner"))?;
                        bind_model_notices(&transaction,&self.data_root_id,&conversation,run_id,key,owner,input,wall_now_ms)?;
                    }
                    if let Some(key) = notice_response.as_deref() {
                        if model_input.is_some() { return Err(kernel_err("model response carried a dispatch input")); }
                        let owner = notice_response_owner.ok_or_else(||kernel_err("missing model notice response owner"))?;
                        acknowledge_model_notices(&transaction,&self.data_root_id,&conversation,run_id,key,owner,wall_now_ms)?;
                    }
                    if let Some((effect_key, _)) = model_retry_lease {
                        let bound: i64 = transaction.query_row(
                            "SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                             JOIN kernel_job_notices n ON n.job_id=d.job_id
                             WHERE n.data_root_id=?1 AND n.conversation_id=?2 AND n.run_id=?3
                               AND d.state='bound' AND d.dispatch_key IN
                                 (SELECT idempotency_key FROM kernel_effect_outbox
                                  WHERE run_id=?3 AND effect_key=?4)",
                            params![self.data_root_id,conversation,run_id,effect_key],|row|row.get(0),
                        )?;
                        if bound != 0 { return Err(kernel_err("model notice delivery remains uncertain")); }
                    }
                } else if model_input.is_some() {
                    return Err(kernel_err("model notice input supplied while experiment is disabled"));
                }
            }
            if let Some((tool_call_id, lease_owner)) = dispatch_lease {
                let owns: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox
                     WHERE run_id=?1 AND effect_key=?2 AND tool_call_id=?3
                       AND effect_type='dispatch_tool' AND status='leased' AND lease_owner=?4)",
                    params![run_id, crate::kernel::dispatch_effect_key(tool_call_id), tool_call_id, lease_owner],
                    |row| row.get(0),
                )?;
                if !owns { return Err(kernel_err("kernel result lost dispatch lease ownership")); }
            }
            let (
                current_state,
                persisted_last_seq,
                persisted_turn_id,
                engine_id,
                kernel_mode,
                capability_manifest_version,
                permission_snapshot_id,
                execution_profile_id,
                prompt_config_hash,
                frozen_config_json,
                manifest_hash,
                approval_deadline,
            ): (
                String,
                i64,
                Option<String>,
                String,
                String,
                i64,
                String,
                String,
                String,
                String,
                Option<String>,
                Option<i64>,
            ) = transaction.query_row(
                "SELECT state, last_event_seq, turn_id, engine_id, kernel_mode,
                        capability_manifest_version, permission_snapshot_id,
                        execution_profile_id, prompt_config_hash,
                        frozen_config_json, capability_manifest_hash,
                        approval_deadline_wall_ms
                   FROM kernel_runs WHERE run_id = ?1",
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
                    ))
                },
            )?;
            // HARD ISOLATION: the executable decision commit path must never
            // persist an outbox effect for a shadow run. Shadow decisions go to
            // kernel_shadow_diffs, never here.
            if kernel_mode == "shadow" {
                return Err(kernel_err(format!(
                    "kernel_commit_decision rejects shadow run {run_id}; \
                     shadow decisions are observation-only"
                )));
            }
            if persisted_turn_id
                .as_deref()
                .is_some_and(|turn_id| turn_id != cmd.turn_id)
            {
                return Err(kernel_err(format!(
                    "kernel turn identity conflict for {run_id}"
                )));
            }
            let frozen: crate::kernel::RunFrozenConfig =
                serde_json::from_str(&frozen_config_json).map_err(|error| {
                    kernel_err(format!("invalid frozen config for {run_id}: {error}"))
                })?;
            // Terminal exhaustion commits turn_attempts == turn_max + 1 (the
            // failed attempt that spent the budget); reject only greater values.
            let terminal_retry_exhausted = cmd.run_state.is_terminal()
                && cmd.retry.turn_attempts == cmd.retry.turn_max.saturating_add(1);
            if frozen.engine_id != engine_id
                || frozen.kernel_mode != kernel_mode
                || i64::from(frozen.capability_manifest_version) != capability_manifest_version
                || frozen.permission_snapshot_id != permission_snapshot_id
                || frozen.execution_profile_id != execution_profile_id
                || frozen.prompt_config_hash != prompt_config_hash
                || manifest_hash.as_deref() != Some(frozen.capability_manifest_hash.as_str())
                || cmd.retry.provider_max != frozen.provider_max_retries
                || cmd.retry.turn_max != frozen.turn_max_retries
                || cmd.retry.provider_attempts > cmd.retry.provider_max
                || (cmd.retry.turn_attempts > cmd.retry.turn_max && !terminal_retry_exhausted)
                || cmd.retry.completion_attempts > cmd.retry.turn_max
                || (cmd.retry.completion_attempts > 0 && cmd.retry.completion_effect_key.is_none())
            {
                return Err(kernel_err(format!(
                    "kernel frozen identity or retry policy conflict for {run_id}"
                )));
            }
            let retry_scheduled = cmd.run_state == crate::kernel::RunState::RetryScheduled;
            if retry_scheduled
                != (cmd.retry.scheduled_at_wall_ms.is_some() && cmd.retry.due_wall_ms.is_some())
                || cmd.retry.scheduled_at_wall_ms.is_some() != cmd.retry.due_wall_ms.is_some()
                || matches!(
                    (cmd.retry.scheduled_at_wall_ms, cmd.retry.due_wall_ms),
                    (Some(start), Some(due)) if start <= 0 || due < start
                )
            {
                return Err(kernel_err(format!(
                    "kernel retry schedule is incomplete or inconsistent for {run_id}"
                )));
            }
            // A decision commit may legitimately advance created->running (the run
            // start events and the first decision are committed together); any
            // other transition must follow the declared run state machine.
            let started_here = current_state == "created" && cmd.run_state.as_str() == "running";
            if current_state != cmd.run_state.as_str()
                && !started_here
                && !run_transition_allowed(&current_state, cmd.run_state.as_str())
            {
                return Err(kernel_err(format!(
                    "illegal persisted run transition {} -> {}",
                    current_state,
                    cmd.run_state.as_str()
                )));
            }

            super::kernel_compaction::validate_decision(&transaction, run_id, wall_now_ms, persisted_last_seq, cmd)?;
            // 1. Append events. Exact replays are accepted, but every new event
            // must continue the durable sequence without a gap.
            let actual_last_seq: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM kernel_events WHERE run_id=?1",
                params![run_id],
                |row| row.get(0),
            )?;
            if actual_last_seq != persisted_last_seq {
                return Err(kernel_err(format!(
                    "kernel event cursor disagrees with event stream for {run_id}"
                )));
            }
            let mut seen_event_seqs = std::collections::BTreeSet::new();
            let mut next_new_seq = persisted_last_seq.saturating_add(1);
            let mut previous_cmd_seq = None;
            for event in &cmd.events {
                let seq = i64::try_from(event.seq)
                    .map_err(|_| kernel_err("kernel event sequence exceeds i64"))?;
                if seq <= 0
                    || !seen_event_seqs.insert(seq)
                    || previous_cmd_seq.is_some_and(|previous| seq <= previous)
                {
                    return Err(kernel_err(format!(
                        "kernel decision has duplicate or unordered event seq {seq}"
                    )));
                }
                previous_cmd_seq = Some(seq);
                if event.event_type.trim().is_empty()
                    || serde_json::from_str::<serde_json::Value>(&event.payload_json).is_err()
                {
                    return Err(kernel_err(format!(
                        "kernel event {seq} has an empty type or invalid JSON payload"
                    )));
                }
                let existing: Option<(String, String)> = transaction
                    .query_row(
                        "SELECT event_type, payload_json FROM kernel_events WHERE run_id=?1 AND seq=?2",
                        params![run_id, seq],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((et, ep)) = existing {
                    if et != event.event_type || ep != event.payload_json {
                        return Err(kernel_err(format!(
                            "kernel event conflict for {run_id} seq {}",
                            event.seq
                        )));
                    }
                    continue;
                }
                if seq != next_new_seq {
                    return Err(kernel_err(format!(
                        "kernel event sequence gap for {run_id}: expected {next_new_seq}, got {seq}"
                    )));
                }
                transaction.execute(
                    "INSERT INTO kernel_events (run_id, seq, event_type, payload_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![run_id, seq, event.event_type, event.payload_json, wall_now_ms],
                )?;
                next_new_seq = next_new_seq.saturating_add(1);
            }

            // 2. Persist batch identity before tool rows. Membership is immutable;
            // barrier_emitted is monotonic false->true.
            let mut seen_batch_ids = std::collections::BTreeSet::new();
            for batch in &cmd.batches {
                if batch.batch_id.trim().is_empty()
                    || batch.ordered_tool_call_ids.is_empty()
                    || !seen_batch_ids.insert(batch.batch_id.clone())
                {
                    return Err(kernel_err("empty or duplicate kernel batch identity"));
                }
                let ordered_unique = batch
                    .ordered_tool_call_ids
                    .iter()
                    .all(|id| !id.trim().is_empty())
                    && batch
                        .ordered_tool_call_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == batch.ordered_tool_call_ids.len();
                if !ordered_unique {
                    return Err(kernel_err(format!(
                        "kernel batch {} has duplicate or empty members",
                        batch.batch_id
                    )));
                }
                let existing: Option<(String, String, i64)> = transaction
                    .query_row(
                        "SELECT run_id, ordered_tool_call_ids_json, barrier_emitted
                           FROM kernel_tool_batches WHERE batch_id=?1",
                        params![batch.batch_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                if let Some((owner_run, ordered_json, barrier_emitted)) = existing {
                    let persisted_order: Vec<String> = serde_json::from_str(&ordered_json)
                        .map_err(|error| {
                            kernel_err(format!(
                                "invalid persisted batch ordering for {}: {error}",
                                batch.batch_id
                            ))
                        })?;
                    if owner_run != run_id || persisted_order != batch.ordered_tool_call_ids {
                        return Err(kernel_err(format!(
                            "kernel batch identity conflict for {}",
                            batch.batch_id
                        )));
                    }
                    if batch.barrier_emitted && barrier_emitted == 0 {
                        transaction.execute(
                            "UPDATE kernel_tool_batches SET barrier_emitted=1 WHERE batch_id=?1",
                            params![batch.batch_id],
                        )?;
                    }
                } else {
                    transaction.execute(
                        "INSERT INTO kernel_tool_batches
                            (batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            batch.batch_id,
                            run_id,
                            serde_json::to_string(&batch.ordered_tool_call_ids)
                                .map_err(|error| kernel_err(error.to_string()))?,
                            batch.barrier_emitted as i64,
                            wall_now_ms,
                        ],
                    )?;
                }
            }

            // Explicit re-evaluation is part of this decision's atomic write-set,
            // never a separate pre-commit mutation of the live aggregate.
            for event in cmd.events.iter().filter(|e|e.event_type=="tool.reevaluated") {
                let body:serde_json::Value=serde_json::from_str(&event.payload_json).map_err(|e|kernel_err(e.to_string()))?;
                let call=body.get("toolCallId").and_then(|v|v.as_str()).ok_or_else(||kernel_err("reevaluation is missing tool identity"))?;
                let expected=body.get("expectedPolicyVersion").and_then(|v|v.as_u64()).ok_or_else(||kernel_err("reevaluation is missing policy version"))?;
                let current:Option<u64>=transaction.query_row("SELECT p.version FROM kernel_execution_policies p JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=?1",[run_id],|r|r.get(0)).optional()?;
                if current!=Some(expected) {return Err(kernel_err("policy_version_conflict"));}
                let consumed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox WHERE run_id=?1 AND tool_call_id=?2 AND effect_type='dispatch_tool')",params![run_id,call],|r|r.get(0))?;
                let dispatch=fox_engine_protocol::encode_dispatch_id(run_id,call).map_err(kernel_err)?;
                let attempted:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_execution_attempts WHERE run_id=?1 AND dispatch_id=?2) OR EXISTS(SELECT 1 FROM kernel_execution_credentials WHERE run_id=?1 AND dispatch_id=?2)",params![run_id,dispatch],|r|r.get(0))?;
                if consumed || attempted {return Err(kernel_err("intent_consumed"));}
                let old_version:Option<u64>=transaction.query_row("SELECT policy_version FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",params![run_id,call],|r|r.get(0)).optional()?.flatten();
                if old_version==current {return Err(kernel_err("intent_policy_unchanged"));}
                let reset=transaction.execute("UPDATE kernel_tool_calls SET state='pending' WHERE run_id=?1 AND tool_call_id=?2 AND state IN ('pending','waiting_approval')",params![run_id,call])?;
                if reset!=1 {return Err(kernel_err("intent_not_reevaluable"));}
                transaction.execute(
                    "INSERT INTO kernel_approval_history(
                        run_id,tool_call_id,state,requested_at,resolved_at,created_at)
                     SELECT run_id,tool_call_id,state,created_at,decided_at,?3
                       FROM kernel_approvals
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, call, wall_now_ms],
                )?;
                transaction.execute("DELETE FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",params![run_id,call])?;
                // Keep the old prompt as a terminal audit fact under a generation
                // key, freeing the live prompt key for this same logical intent.
                transaction.execute("UPDATE kernel_effect_outbox SET effect_key='superseded:'||effect_key||':v'||?3,idempotency_key='superseded:'||idempotency_key||':v'||?3,status='completed',completed_at=?4,updated_at=?4,lease_owner=NULL WHERE run_id=?1 AND tool_call_id=?2 AND effect_type='request_approval' AND effect_key NOT LIKE 'superseded:%'",params![run_id,call,old_version.unwrap_or(0),wall_now_ms])?;
            }

            // 3. Apply approval CAS facts in this same decision transaction.
            let mut seen_approval_ids = std::collections::BTreeSet::new();
            for resolution in &cmd.approval_resolutions {
                if resolution.tool_call_id.trim().is_empty()
                    || !seen_approval_ids.insert(resolution.tool_call_id.clone())
                    || !matches!(
                        resolution.state.as_str(),
                        "allow_once" | "allow_conversation" | "denied" | "cancelled" | "expired"
                    )
                {
                    return Err(kernel_err("invalid or duplicate approval resolution"));
                }
                if matches!(resolution.state.as_str(), "allow_once"|"allow_conversation"|"denied") {
                    let reissued:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_approval_history WHERE run_id=?1 AND tool_call_id=?2)",params![run_id,resolution.tool_call_id],|r|r.get(0))?;
                    if reissued && expected_approval_policy_version.is_none() { return Err(kernel_err("stale_approval: reissued intent requires the displayed policy version")); }
                    let ticket_version:Option<u64>=transaction.query_row("SELECT policy_version FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",params![run_id,resolution.tool_call_id],|r|r.get(0))?;
                    let current:Option<u64>=transaction.query_row("SELECT p.version FROM kernel_execution_policies p JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=?1",[run_id],|r|r.get(0)).optional()?;
                    if current.is_some() && current!=ticket_version {return Err(kernel_err("stale_approval: authoritative ticket version is not current"));}
                    if let Some(expected)=expected_approval_policy_version {
                        if current!=Some(expected) || ticket_version!=Some(expected) {return Err(kernel_err("stale_approval: displayed policy version no longer current"));}
                    }
                }
                let current_approval: String = transaction.query_row(
                    "SELECT state FROM kernel_approvals
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, resolution.tool_call_id],
                    |row| row.get(0),
                )?;
                if current_approval == "pending" {
                    let deadline = approval_deadline.ok_or_else(|| {
                        kernel_err(format!(
                            "pending approval has no durable deadline for {run_id}"
                        ))
                    })?;
                    if resolution.state == "expired" && wall_now_ms < deadline {
                        return Err(kernel_err(format!(
                            "approval cannot expire before its deadline for {}/{}",
                            run_id, resolution.tool_call_id
                        )));
                    }
                    if wall_now_ms >= deadline
                        && !matches!(resolution.state.as_str(), "expired" | "cancelled")
                    {
                        return Err(kernel_err(format!(
                            "approval decision arrived after its deadline for {}/{}",
                            run_id, resolution.tool_call_id
                        )));
                    }
                    if wall_now_ms
                        < deadline.saturating_sub(frozen.approval_wait_timeout_ms)
                    {
                        return Err(kernel_err(format!(
                            "wall clock rolled back during approval for {}/{}",
                            run_id, resolution.tool_call_id
                        )));
                    }
                    let affected = transaction.execute(
                        "UPDATE kernel_approvals SET state=?3, decided_at=?4
                          WHERE run_id=?1 AND tool_call_id=?2 AND state='pending'",
                        params![
                            run_id,
                            resolution.tool_call_id,
                            resolution.state,
                            wall_now_ms
                        ],
                    )?;
                    if affected != 1 {
                        return Err(kernel_err("approval CAS did not update exactly one row"));
                    }
                } else if current_approval != resolution.state {
                    return Err(kernel_err(format!(
                        "approval resolution conflict for {}/{}",
                        run_id, resolution.tool_call_id
                    )));
                }
                let prompt: (String, Option<String>, String) = transaction.query_row(
                    "SELECT effect_type, tool_call_id, status
                       FROM kernel_effect_outbox
                      WHERE run_id=?1 AND effect_key=?2",
                    params![
                        run_id,
                        crate::kernel::approval_effect_key(&resolution.tool_call_id)
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                if prompt.0 != "request_approval"
                    || prompt.1.as_deref() != Some(resolution.tool_call_id.as_str())
                    || prompt.2 == "failed"
                {
                    return Err(kernel_err(format!(
                        "approval prompt outbox conflict for {}/{}",
                        run_id, resolution.tool_call_id
                    )));
                }
                if matches!(prompt.2.as_str(), "pending" | "leased") {
                    let affected = transaction.execute(
                        "UPDATE kernel_effect_outbox
                            SET status='completed', completed_at=?3, updated_at=?3,
                                lease_owner=NULL
                          WHERE run_id=?1 AND effect_key=?2
                            AND status IN ('pending','leased')",
                        params![
                            run_id,
                            crate::kernel::approval_effect_key(&resolution.tool_call_id),
                            wall_now_ms
                        ],
                    )?;
                    if affected != 1 {
                        return Err(kernel_err("approval prompt completion lost its CAS"));
                    }
                }
            }

            // 4. Create missing tool calls with their exact canonical input, then
            // apply only legal monotonic transitions.
            let mut seen_tool_ids = std::collections::BTreeSet::new();
            for tool in &cmd.tools {
                if tool.tool_call_id.trim().is_empty()
                    || tool.batch_id.trim().is_empty()
                    || tool.tool.trim().is_empty()
                    || !seen_tool_ids.insert(tool.tool_call_id.clone())
                    || !matches!(
                        serde_json::from_str::<serde_json::Value>(&tool.canonical_input_json),
                        Ok(serde_json::Value::Object(_))
                    )
                {
                    return Err(kernel_err("invalid or duplicate kernel tool identity/input"));
                }
                let state = tool.state.as_str();
                if !valid_tool_state(state)
                    || !tool.state.is_terminal() && tool.result_json.is_some()
                    || tool.state == crate::kernel::ToolCallState::Completed
                        && tool.result_json.is_none()
                {
                    return Err(kernel_err(format!(
                        "tool result/state conflict for {}/{}",
                        run_id, tool.tool_call_id
                    )));
                }
                let source_order = i64::try_from(tool.source_order)
                    .map_err(|_| kernel_err("tool source order exceeds i64"))?;
                let batch_order_json: String = transaction.query_row(
                    "SELECT ordered_tool_call_ids_json FROM kernel_tool_batches
                      WHERE batch_id=?1 AND run_id=?2",
                    params![tool.batch_id, run_id],
                    |row| row.get(0),
                )?;
                let batch_order: Vec<String> = serde_json::from_str(&batch_order_json)
                    .map_err(|error| kernel_err(error.to_string()))?;
                if batch_order.get(tool.source_order) != Some(&tool.tool_call_id) {
                    return Err(kernel_err(format!(
                        "tool/batch source order conflict for {}/{}",
                        run_id, tool.tool_call_id
                    )));
                }
                if let Some(key) = &tool.dispatch_idempotency_key {
                    if key != &fox_engine_protocol::encode_dispatch_id(run_id, &tool.tool_call_id).map_err(kernel_err)? {
                        return Err(kernel_err(format!(
                            "dispatch idempotency key conflict for {}/{}",
                            run_id, tool.tool_call_id
                        )));
                    }
                }
                if tool.state == crate::kernel::ToolCallState::Running
                    && tool.dispatch_idempotency_key.is_none()
                {
                    return Err(kernel_err(format!(
                        "running tool has no dispatch identity for {}/{}",
                        run_id, tool.tool_call_id
                    )));
                }
                let existing: Option<(
                    String,
                    String,
                    i64,
                    String,
                    Option<String>,
                    Option<String>,
                )> = transaction
                    .query_row(
                        "SELECT batch_id, tool, source_order, state, result_json,
                                dispatch_idempotency_key
                           FROM kernel_tool_calls
                          WHERE run_id=?1 AND tool_call_id=?2",
                        params![run_id, tool.tool_call_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        },
                    )
                    .optional()?;
                if existing.is_none() {
                    transaction.execute(
                        "INSERT INTO kernel_tool_calls
                            (run_id, tool_call_id, batch_id, tool, source_order,
                             canonical_input_json, state, result_json, created_at, settled_at,
                             dispatch_idempotency_key)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                                 CASE WHEN ?10 THEN ?9 ELSE NULL END, ?11)",
                        params![
                            run_id,
                            tool.tool_call_id,
                            tool.batch_id,
                            tool.tool,
                            source_order,
                            tool.canonical_input_json,
                            state,
                            tool.result_json,
                            wall_now_ms,
                            tool.state.is_terminal(),
                            tool.dispatch_idempotency_key,
                        ],
                    )?;
                    continue;
                }
                let (batch_id, tool_name, prior_order, prior, prior_result, prior_dispatch) =
                    existing.unwrap();
                let prior_input: String = transaction.query_row(
                    "SELECT canonical_input_json FROM kernel_tool_calls
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, tool.tool_call_id],
                    |row| row.get(0),
                )?;
                if batch_id != tool.batch_id
                    || tool_name != tool.tool
                    || prior_order != source_order
                    || prior_input != tool.canonical_input_json
                    || tool
                        .dispatch_idempotency_key
                        .as_ref()
                        .is_some_and(|key| prior_dispatch.as_ref().is_some_and(|old| old != key))
                {
                    return Err(kernel_err(format!(
                        "kernel tool immutable identity conflict for {}/{}",
                        run_id, tool.tool_call_id
                    )));
                }
                if prior == state {
                    if prior_result != tool.result_json {
                        return Err(kernel_err(format!(
                            "kernel tool result replay conflict for {}/{}",
                            run_id, tool.tool_call_id
                        )));
                    }
                } else {
                    // An approval that already died must not be resurrected by a
                    // late decision: the call was never dispatched, so the only
                    // honest outcome is the one already recorded.
                    let dead_approval = state == "failed" || state == "cancelled";
                    let already_dead = prior == "expired" || prior == "cancelled";
                    if dead_approval && already_dead {
                        continue;
                    }
                    if !tool_transition_allowed(&prior, state) {
                        return Err(kernel_err(format!(
                            "illegal tool transition {prior}->{state} for {}/{}",
                            run_id, tool.tool_call_id
                        )));
                    }
                    let affected = transaction.execute(
                        "UPDATE kernel_tool_calls
                            SET state=?3,
                                result_json=?4,
                                dispatch_idempotency_key=COALESCE(?5, dispatch_idempotency_key),
                                settled_at=CASE WHEN ?6 THEN ?7 ELSE settled_at END
                          WHERE run_id=?1 AND tool_call_id=?2 AND state=?8",
                        params![
                            run_id,
                            tool.tool_call_id,
                            state,
                            tool.result_json,
                            tool.dispatch_idempotency_key,
                            tool.state.is_terminal(),
                            wall_now_ms,
                            prior,
                        ],
                    )?;
                    if affected != 1 {
                        return Err(kernel_err("tool state transition lost its CAS"));
                    }
                }
            }

            // 5. Enqueue outbox effects with exact immutable replay checks.
            let mut seen_effect_keys = std::collections::BTreeSet::new();
            for effect in &cmd.outbox {
                // A dispatch created by THIS effect gets its authoritative
                // credential in the same transaction; a pre-existing row is
                // never backfilled into authorization.
                let mut dispatch_newly_inserted = false;
                if effect.kind == crate::kernel::OutboxEffectKind::ContinuationModel {
                    let payload: serde_json::Value = serde_json::from_str(&effect.payload_json).map_err(|_| kernel_err("invalid continuation input"))?;
                    let input: fox_engine_protocol::KernelInitialModelInput = serde_json::from_value(payload["input"].clone()).map_err(|_| kernel_err("invalid continuation input"))?;
                    input.validate().map_err(kernel_err)?;
                    let prompt_hash: String = transaction.query_row("SELECT prompt_config_hash FROM kernel_runs WHERE run_id=?1", [run_id], |row| row.get(0))?;
                    if input.run_id != run_id || input.turn_id != cmd.turn_id || input.prompt_config_hash != prompt_hash
                        || payload["effectKey"] != effect.effect_key
                        || effect.idempotency_key != format!("continuation-delivery:{}",effect.effect_key)
                        || !cmd.events.iter().any(|event| event.event_type == "engine.continuation_requested" && event.payload_json == effect.payload_json) {
                        return Err(kernel_err("continuation input has no matching durable decision"));
                    }
                }

                if effect.effect_key.trim().is_empty()
                    || effect.idempotency_key.trim().is_empty()
                    || !seen_effect_keys.insert(effect.effect_key.clone())
                    || serde_json::from_str::<serde_json::Value>(&effect.payload_json).is_err()
                {
                    return Err(kernel_err("invalid or duplicate kernel outbox effect"));
                }
                let existing: Option<(
                    String,
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                )> = transaction
                    .query_row(
                        "SELECT effect_type, idempotency_key, tool_call_id, batch_id,
                                payload_json
                           FROM kernel_effect_outbox
                          WHERE run_id=?1 AND effect_key=?2",
                        params![run_id, effect.effect_key],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .optional()?;
                if let Some((kind, idem, tool_call_id, batch_id, payload)) = existing {
                    if kind != effect.kind.as_str()
                        || idem != effect.idempotency_key
                        || tool_call_id != effect.tool_call_id
                        || batch_id != effect.batch_id
                        || payload != effect.payload_json
                    {
                        return Err(kernel_err(format!(
                            "kernel outbox identity conflict for {}/{}",
                            run_id, effect.effect_key
                        )));
                    }
                } else {
                    let owner: Option<String> = transaction
                        .query_row(
                            "SELECT effect_key FROM kernel_effect_outbox
                              WHERE run_id=?1 AND idempotency_key=?2",
                            params![run_id, effect.idempotency_key],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if owner.is_some() {
                        return Err(kernel_err(format!(
                            "idempotency key collision for {} in {run_id}",
                            effect.idempotency_key
                        )));
                    }
                    transaction.execute(
                        "INSERT INTO kernel_effect_outbox
                        (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                         payload_json, status, attempts, lease_owner, leased_at, completed_at,
                         last_error, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0, NULL, NULL, NULL, NULL, ?8, ?8)",
                    params![
                        run_id,
                        effect.effect_key,
                        effect.kind.as_str(),
                        effect.idempotency_key,
                        effect.tool_call_id,
                        effect.batch_id,
                        effect.payload_json,
                        wall_now_ms
                    ],
                    )?;
                    if effect.kind == crate::kernel::OutboxEffectKind::DispatchTool {
                        dispatch_newly_inserted = true;
                    }
                }
                let payload_value: serde_json::Value = serde_json::from_str(&effect.payload_json)
                    .map_err(|error| kernel_err(error.to_string()))?;
                match effect.kind {
                    crate::kernel::OutboxEffectKind::ContinuationModel => {},
                    crate::kernel::OutboxEffectKind::InitialModel => {
                        let input = super::kernel_initial_input::read_input(&transaction, run_id)?;
                        let hash: String = transaction.query_row("SELECT input_hash FROM kernel_initial_inputs WHERE run_id=?1", [run_id], |row| row.get(0))?;
                        if effect.effect_key != crate::kernel::INITIAL_MODEL_EFFECT_KEY
                            || effect.idempotency_key != crate::kernel::INITIAL_MODEL_IDEMPOTENCY_KEY
                            || effect.tool_call_id.is_some() || effect.batch_id.is_some()
                            || payload_value["turnId"] != input.turn_id || payload_value["inputHash"] != hash
                            || !cmd.events.iter().any(|event| event.seq == 2 && event.event_type == "engine.initial_requested") {
                            return Err(kernel_err("initial model effect differs from frozen input"));
                        }
                    }
                    crate::kernel::OutboxEffectKind::DispatchTool
                    | crate::kernel::OutboxEffectKind::RequestApproval => {
                        let tool_call_id = effect.tool_call_id.as_deref().ok_or_else(|| {
                            kernel_err("tool outbox effect is missing tool_call_id")
                        })?;
                        let batch_id = effect.batch_id.as_deref().ok_or_else(|| {
                            kernel_err("tool outbox effect is missing batch_id")
                        })?;
                        let persisted: (String, String, String) = transaction.query_row(
                            "SELECT batch_id, tool, canonical_input_json
                               FROM kernel_tool_calls
                              WHERE run_id=?1 AND tool_call_id=?2",
                            params![run_id, tool_call_id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )?;
                        let persisted_input: serde_json::Value =
                            serde_json::from_str(&persisted.2)
                                .map_err(|error| kernel_err(error.to_string()))?;
                        if persisted.0 != batch_id
                            || payload_value.get("tool").and_then(|value| value.as_str())
                                != Some(persisted.1.as_str())
                            || payload_value.get("input") != Some(&persisted_input)
                        {
                            return Err(kernel_err(format!(
                                "tool outbox payload conflicts with durable identity for {run_id}/{tool_call_id}"
                            )));
                        }
                        // The authoritative credential is frozen in the SAME
                        // transaction as the dispatch fact. Only a dispatch
                        // created by this commit gets one: an older row without
                        // a credential is never backfilled into authorization.
                        if dispatch_newly_inserted {
                            super::kernel_execution_admission::issue_dispatch_credential_in_tx(
                                &transaction,
                                run_id,
                                tool_call_id,
                                &persisted.1,
                                &persisted.2,
                                wall_now_ms,
                            )
                            .map_err(kernel_err)?;
                        }
                    }
                    crate::kernel::OutboxEffectKind::DeliverToolBatch => {
                        if effect.tool_call_id.is_some() {
                            return Err(kernel_err("batch delivery must not own one tool call"));
                        }
                        let batch_id = effect.batch_id.as_deref().ok_or_else(|| {
                            kernel_err("batch delivery is missing batch_id")
                        })?;
                        let ordered_json: String = transaction.query_row(
                            "SELECT ordered_tool_call_ids_json FROM kernel_tool_batches
                              WHERE run_id=?1 AND batch_id=?2",
                            params![run_id, batch_id],
                            |row| row.get(0),
                        )?;
                        let ordered: Vec<String> = serde_json::from_str(&ordered_json)
                            .map_err(|error| kernel_err(error.to_string()))?;
                        if payload_value.get("orderedToolCallIds")
                            != Some(&serde_json::json!(ordered))
                        {
                            return Err(kernel_err(format!(
                                "batch delivery payload conflicts with durable ordering for {run_id}/{batch_id}"
                            )));
                        }
                    }
                    crate::kernel::OutboxEffectKind::CancelEngineTurn => {
                        if effect.tool_call_id.is_some() || effect.batch_id.is_some() {
                            return Err(kernel_err("engine cancellation has invalid ownership"));
                        }
                    }
                    crate::kernel::OutboxEffectKind::CancelToolCall => {
                        if effect.tool_call_id.is_none() || effect.batch_id.is_some() {
                            return Err(kernel_err("tool cancellation has invalid ownership"));
                        }
                    }
                    crate::kernel::OutboxEffectKind::PublishSnapshot => {}
                }
                // A pending approval creates the approval fact in the SAME txn.
                if effect.kind == crate::kernel::OutboxEffectKind::RequestApproval {
                    if let Some(tool_call_id) = &effect.tool_call_id {
                        transaction.execute(
                            "INSERT OR IGNORE INTO kernel_approvals (run_id, tool_call_id, state, created_at, decided_at)
                             VALUES (?1, ?2, 'pending', ?3, NULL)",
                            params![run_id, tool_call_id, wall_now_ms],
                        )?;
                    }
                }
            }

            // Approval resolution, final tool state and dispatch intent are one
            // indivisible decision. This prevents a forged/malformed write-set
            // from recording a denial or expiry while still dispatching the tool.
            for resolution in &cmd.approval_resolutions {
                let final_tool_state: String = transaction.query_row(
                    "SELECT state FROM kernel_tool_calls
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, resolution.tool_call_id],
                    |row| row.get(0),
                )?;
                let dispatch_exists: bool = transaction.query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM kernel_effect_outbox
                         WHERE run_id=?1 AND effect_key=?2
                           AND effect_type='dispatch_tool' AND tool_call_id=?3
                    )",
                    params![
                        run_id,
                        crate::kernel::dispatch_effect_key(&resolution.tool_call_id),
                        resolution.tool_call_id
                    ],
                    |row| row.get(0),
                )?;
                let consistent = match resolution.state.as_str() {
                    "allow_once" | "allow_conversation" => {
                        final_tool_state == "running" && dispatch_exists
                    }
                    // A dead approval may land on `failed` (an explicit denial) or
                    // on `expired` (the wait elapsed with no decision). Both mean
                    // the same thing here: no dispatch, so nothing was executed.
                    "denied" | "expired" => {
                        matches!(final_tool_state.as_str(), "failed" | "expired")
                            && !dispatch_exists
                    }
                    "cancelled" => {
                        !dispatch_exists
                            && (final_tool_state == "cancelled"
                                || cmd.run_state == crate::kernel::RunState::Cancelling
                                    && final_tool_state == "waiting_approval")
                    }
                    _ => false,
                };
                if !consistent {
                    return Err(kernel_err(format!(
                        "approval resolution conflicts with tool state or dispatch for {}/{}",
                        run_id, resolution.tool_call_id
                    )));
                }
            }

            // A true barrier must have a durable delivery effect, and every
            // persisted batch must exactly match its tool membership/state.
            for batch in &cmd.batches {
                for (source_order, tool_call_id) in
                    batch.ordered_tool_call_ids.iter().enumerate()
                {
                    let member: Option<(String, i64)> = transaction
                        .query_row(
                            "SELECT batch_id, source_order FROM kernel_tool_calls
                              WHERE run_id=?1 AND tool_call_id=?2",
                            params![run_id, tool_call_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?;
                    if member
                        != Some((
                            batch.batch_id.clone(),
                            i64::try_from(source_order)
                                .map_err(|_| kernel_err("source order exceeds i64"))?,
                        ))
                    {
                        return Err(kernel_err(format!(
                            "kernel batch membership conflict for {}/{}",
                            run_id, batch.batch_id
                        )));
                    }
                }
                let unsettled: i64 = transaction.query_row(
                    "SELECT COUNT(*) FROM kernel_tool_calls
                      WHERE run_id=?1 AND batch_id=?2
                        AND state NOT IN ('completed','failed','cancelled','expired')",
                    params![run_id, batch.batch_id],
                    |row| row.get(0),
                )?;
                if batch.barrier_emitted && unsettled != 0
                    || !batch.barrier_emitted
                        && unsettled == 0
                        && !cmd.run_state.is_terminal()
                        && cmd.run_state != crate::kernel::RunState::Cancelling
                {
                    return Err(kernel_err(format!(
                        "kernel batch barrier/settlement conflict for {}/{}",
                        run_id, batch.batch_id
                    )));
                }
                if batch.barrier_emitted {
                    let delivery_exists: bool = transaction.query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM kernel_effect_outbox
                             WHERE run_id=?1 AND effect_key=?2
                               AND effect_type='deliver_tool_batch' AND batch_id=?3
                        )",
                        params![
                            run_id,
                            crate::kernel::batch_delivery_effect_key(&batch.batch_id),
                            batch.batch_id
                        ],
                        |row| row.get(0),
                    )?;
                    if !delivery_exists {
                        return Err(kernel_err(format!(
                            "settled batch has no durable delivery effect for {}/{}",
                            run_id, batch.batch_id
                        )));
                    }
                }
            }

            // 6. Tool settlement and dispatch completion are one transaction.
            let mut seen_settled_dispatches = std::collections::BTreeSet::new();
            for tool_call_id in &cmd.settled_dispatch_tool_call_ids {
                if !seen_settled_dispatches.insert(tool_call_id.clone()) {
                    return Err(kernel_err("duplicate settled dispatch identity"));
                }
                let (kind, owner_tool, status): (String, Option<String>, String) = transaction
                    .query_row(
                        "SELECT effect_type, tool_call_id, status
                           FROM kernel_effect_outbox
                          WHERE run_id=?1 AND effect_key=?2",
                        params![run_id, crate::kernel::dispatch_effect_key(tool_call_id)],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                if kind != "dispatch_tool" || owner_tool.as_deref() != Some(tool_call_id.as_str()) {
                    return Err(kernel_err(format!(
                        "dispatch settlement identity conflict for {run_id}/{tool_call_id}"
                    )));
                }
                match status.as_str() {
                    "leased" => {
                        let affected = transaction.execute(
                            "UPDATE kernel_effect_outbox
                                SET status='completed', completed_at=?3, updated_at=?3,
                                    lease_owner=NULL
                              WHERE run_id=?1 AND effect_key=?2 AND status='leased'",
                            params![
                                run_id,
                                crate::kernel::dispatch_effect_key(tool_call_id),
                                wall_now_ms
                            ],
                        )?;
                        if affected != 1 {
                            return Err(kernel_err("dispatch settlement lost its lease CAS"));
                        }
                    }
                    "completed" => {}
                    "pending" => {
                        return Err(kernel_err(format!(
                            "tool result arrived before dispatch lease for {run_id}/{tool_call_id}"
                        )))
                    }
                    _ => {
                        return Err(kernel_err(format!(
                            "cannot settle dispatch in {status} state for {run_id}/{tool_call_id}"
                        )))
                    }
                }
            }

            let unresolved_tools: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM kernel_tool_calls
                  WHERE run_id=?1 AND state NOT IN ('completed','failed','cancelled','expired')",
                params![run_id],
                |row| row.get(0),
            )?;
            if cmd.run_state.is_terminal() && unresolved_tools != 0 {
                return Err(kernel_err(format!(
                    "terminal kernel run {run_id} still has unresolved tools"
                )));
            }
            if cmd.run_state.is_terminal() {
                transaction.execute(
                    "UPDATE kernel_approvals
                        SET state='cancelled', decided_at=?2
                      WHERE run_id=?1 AND state='pending'",
                    params![run_id, wall_now_ms],
                )?;
                transaction.execute(
                    "UPDATE kernel_effect_outbox
                        SET status='failed',
                            last_error='suppressed because run became terminal',
                            updated_at=?2, lease_owner=NULL
                      WHERE run_id=?1 AND status='pending'
                        AND effect_type IN (
                            'dispatch_tool','request_approval','deliver_tool_batch','initial_model','publish_snapshot'
                        )",
                    params![run_id, wall_now_ms],
                )?;
            }
            let pending_approvals: i64 = transaction.query_row(
                // A batch can contain several superseded tickets. Re-evaluating
                // one must not make the other undecided intents disappear from
                // the waiting state. They are not live approval tickets and are
                // never granted here; only the explicit re-evaluation can replace
                // them. Use the same unconsumed-policy-expiry boundary as recovery.
                "SELECT COUNT(*) FROM kernel_approvals a
                  JOIN kernel_tool_calls c ON c.run_id=a.run_id AND c.tool_call_id=a.tool_call_id
                  WHERE a.run_id=?1 AND (a.state='pending' OR (
                    c.state='waiting_approval' AND a.state='expired' AND a.decided_at IS NULL
                    AND a.policy_version < (SELECT p.version FROM kernel_execution_policies p
                      JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=a.run_id)
                    AND EXISTS(SELECT 1 FROM kernel_effect_outbox o WHERE o.run_id=a.run_id
                      AND o.tool_call_id=a.tool_call_id AND o.effect_type='request_approval'
                      AND o.status IN ('pending','leased','completed'))
                    AND NOT EXISTS(SELECT 1 FROM kernel_effect_outbox o WHERE o.run_id=a.run_id
                      AND o.tool_call_id=a.tool_call_id AND o.effect_type='dispatch_tool')
                    AND NOT EXISTS(SELECT 1 FROM kernel_execution_credentials e WHERE e.run_id=a.run_id
                      AND e.dispatch_id='tool-dispatch:'||length(CAST(a.run_id AS BLOB))||':'||a.run_id||':'||a.tool_call_id)
                    AND NOT EXISTS(SELECT 1 FROM kernel_execution_attempts e WHERE e.run_id=a.run_id
                      AND e.dispatch_id='tool-dispatch:'||length(CAST(a.run_id AS BLOB))||':'||a.run_id||':'||a.tool_call_id)
                  ))",
                params![run_id],
                |row| row.get(0),
            )?;
            if (cmd.run_state == crate::kernel::RunState::WaitingApproval)
                != (pending_approvals > 0)
            {
                return Err(kernel_err(format!(
                    "kernel run/approval state conflict for {run_id}"
                )));
            }
            if cmd.run_state == crate::kernel::RunState::WaitingApproval
                && cmd.approval_deadline_wall_ms.is_none()
                || cmd.run_state != crate::kernel::RunState::WaitingApproval
                    && cmd.approval_deadline_wall_ms.is_some()
            {
                return Err(kernel_err(format!(
                    "kernel run/approval deadline conflict for {run_id}"
                )));
            }

            // 7. Update the aggregate only after all decision facts passed their
            // exact replay and transition checks.
            let final_last_seq = next_new_seq.saturating_sub(1);
            let affected = transaction.execute(
                "UPDATE kernel_runs SET
                    state = ?2,
                    turn_id = ?3,
                    running_elapsed_ms = ?4,
                    approval_deadline_wall_ms = ?5,
                    terminal_written = ?6,
                    terminal_at = CASE WHEN ?6 THEN COALESCE(terminal_at, ?7) ELSE NULL END,
                    retry_state_json = ?8,
                    compaction_state_json = ?9,
                    last_event_seq = ?10,
                    model_request_since_wall_ms = ?11,
                    wait_deadline_wall_ms = ?12,
                    wait_accounted_until_wall_ms = ?13,
                    updated_at = ?7
                  WHERE run_id = ?1",
                params![
                    run_id,
                    cmd.run_state.as_str(),
                    cmd.turn_id,
                    cmd.running_elapsed_ms,
                    cmd.approval_deadline_wall_ms,
                    cmd.terminal_written as i64,
                    wall_now_ms,
                    retry_json,
                    compaction_json,
                    final_last_seq,
                    cmd.model_request_since_wall_ms,
                    cmd.wait_deadline_wall_ms,
                    cmd.wait_accounted_until_wall_ms,
                ],
            )?;
            if affected != 1 {
                return Err(kernel_err(format!("kernel run disappeared during commit: {run_id}")));
            }
            if cmd.run_state==crate::kernel::RunState::Completed {
                let pending:i64=transaction.query_row("SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1 AND state IN ('queued','running','paused')",[run_id],|r|r.get(0))?;
                if pending>0 {return Err(kernel_err("kernel.jobs_pending"));}
                let (conversation, frozen): (String,String) = transaction.query_row(
                    "SELECT r.conversation_id,k.frozen_config_json FROM kernel_runs k
                     JOIN runs r ON r.id=k.run_id WHERE k.run_id=?1",[run_id],
                    |row| Ok((row.get(0)?,row.get(1)?)),
                )?;
                let config: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen)
                    .map_err(|_|kernel_err("invalid frozen model notice flag"))?;
                if config.experimental_compute_job_notice {
                    let unhandled: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM kernel_job_notices n
                         LEFT JOIN kernel_job_notice_deliveries d ON d.job_id=n.job_id
                         WHERE n.conversation_id=?2 AND n.run_id=?3
                           AND (n.data_root_id!=?1 OR d.job_id IS NULL OR d.state!='acknowledged')",
                        params![self.data_root_id,conversation,run_id],|row|row.get(0),
                    )?;
                    if unhandled != 0 { return Err(kernel_err("kernel.jobs_pending")); }
                }
            }
            // 7b. A run that stopped because a human had not answered, or because
            //     its execution budget ran out, banks a durable progress snapshot
            //     in the SAME transaction as its terminal state. That is what
            //     makes the continuation entry point honest: the user is offered
            //     the work that actually completed, so the continuation never has
            //     to guess (or replay) what happened. The attempt number keeps
            //     repeated pauses on one run distinct instead of overwriting.
            if cmd.run_state.is_terminal() {
                let pause_reason = match cmd.run_state {
                    crate::kernel::RunState::ApprovalExpired => Some("approval_expired"),
                    crate::kernel::RunState::BudgetExhausted => Some("budget_exhausted"),
                    crate::kernel::RunState::Failed if cmd.events.iter().any(|e| {
                        e.event_type=="run.failed" && serde_json::from_str::<serde_json::Value>(&e.payload_json).ok().is_some_and(|v| {
                            let code=v["code"].as_str().or_else(||v["errorCode"].as_str()).unwrap_or("");
                            ["kernel.jobs_pending",crate::kernel_compaction::NO_CANDIDATES,crate::kernel_compaction::NO_REDUCTION,crate::kernel_compaction::SINGLE_TOO_LARGE,crate::kernel_compaction::INSUFFICIENT,crate::kernel_compaction::FAILED].contains(&code)
                        })
                    }) => Some("context_limit"),
                    _ => None,
                };
                if let Some(pause_reason) = pause_reason {
                    let completed: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM kernel_tool_calls
                          WHERE run_id=?1 AND state='completed'",
                        params![run_id],
                        |row| row.get(0),
                    )?;
                    let pending: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM kernel_tool_calls
                          WHERE run_id=?1 AND state NOT IN ('completed','failed','cancelled','expired')",
                        params![run_id],
                        |row| row.get(0),
                    )?;
                    let attempt: i64 = transaction.query_row(
                        "SELECT attempt FROM kernel_runs WHERE run_id=?1",
                        params![run_id],
                        |row| row.get(0),
                    )?;
                    let summary = serde_json::json!({
                        "runId": run_id,
                        "pauseReason": pause_reason,
                        "completedToolCalls": completed,
                        "pendingToolCalls": pending,
                        "runningElapsedMs": cmd.running_elapsed_ms,
                        "lastEventSeq": final_last_seq,
                        "resumable": true,
                    })
                    .to_string();
                    transaction.execute(
                        "INSERT INTO kernel_run_progress
                         (run_id, attempt, pause_reason, completed_tool_calls, pending_tool_calls,
                          terminal_written, running_elapsed_ms, continuable, summary_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)
                         ON CONFLICT(run_id, attempt, pause_reason) DO UPDATE SET
                             completed_tool_calls=excluded.completed_tool_calls,
                             pending_tool_calls=excluded.pending_tool_calls,
                             terminal_written=excluded.terminal_written,
                             running_elapsed_ms=excluded.running_elapsed_ms,
                             continuable=1,
                             summary_json=excluded.summary_json",
                        params![
                            run_id,
                            attempt,
                            pause_reason,
                            completed,
                            pending,
                            cmd.terminal_written as i64,
                            cmd.running_elapsed_ms,
                            summary,
                            wall_now_ms
                        ],
                    )?;
                }
            }
            // 8. Mid-run steering transitions ride the same decision write-set
            //    so message status can never disagree with the dispatch/response
            //    facts. Every response commit answers the rows delivered to the
            //    in-flight dispatch (live loop or replacement transport); the
            //    directive built with this decision arms the listed rows.
            let is_response_commit = initial_lease.is_some_and(|(_, response)| response)
                || continuation_lease.is_some_and(|(_, _, response)| response)
                || batch_response_lease.is_some();
            if is_response_commit {
                super::apply_delivered_steering_in_tx(
                    &transaction,
                    run_id,
                    wall_now_ms,
                    final_last_seq,
                )?;
            }
            if let Some(decision) = steering {
                if decision.adopt_all_received {
                    // Bind whatever is `received` right now, not only the rows the
                    // caller had already seen: a request accepted inside the
                    // decision window rides this dispatch instead of failing it.
                    super::adopt_received_steering_in_tx(
                        &transaction,
                        run_id,
                        &decision.dispatch_key,
                        final_last_seq,
                    )?;
                } else if !decision.deliver_seqs.is_empty() {
                    super::deliver_steering_in_tx(
                        &transaction,
                        run_id,
                        &decision.deliver_seqs,
                        &decision.dispatch_key,
                        final_last_seq,
                    )
                    .map_err(kernel_err)?;
                }
            }
            // Any open steering dies with a terminal Run, whatever path ended it
            // (completion applies delivered rows first; cancel/fail cancels all).
            if cmd.run_state.is_terminal() {
                // Never drop input the user was told Fox had accepted. A
                // *successful* completion may not abandon rows that were received
                // but never handed to a model; the caller has to answer them
                // (a `steering` lane follow-up) or the Run must end as something
                // other than a clean completion. Explicit cancellation, failure
                // and budget exhaustion keep their own explainable end state, so
                // only `completed` can hit this guard.
                if cmd.run_state == crate::kernel::RunState::Completed {
                    let undelivered: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM run_steering_messages
                          WHERE run_id=?1 AND status='received'",
                        params![run_id],
                        |row| row.get(0),
                    )?;
                    if undelivered > 0 {
                        // A scheduling condition, not an execution failure: this
                        // decision write-set lost a race with a request accepted
                        // moments earlier. The marker lets the transport re-plan
                        // a safe steering round (or detach and re-plan from
                        // durable facts) instead of failing the whole Run.
                        return Err(kernel_err(format!(
                            "{}{undelivered}",
                            crate::kernel::STEERING_COMPETITION
                        )));
                    }
                }
                super::cancel_open_steering_in_tx(&transaction, run_id)?;
            }
            super::kernel_projection::project(&transaction, run_id, wall_now_ms, persisted_last_seq)?;
            transaction.commit()?;
            Ok(kernel_mode == "authoritative" && final_last_seq != persisted_last_seq)
        })?;
        // Publish only AFTER commit and AFTER releasing the connection lock.
        // A failed UI delivery must never make a committed decision look failed.
        if changed { self.kernel_changes.committed(); }
        Ok(())
    }

    /// Compatibility helper for the narrow allow-path: CAS an approval and create
    /// its dispatch intent in one transaction. Production orchestration must use
    /// `RunController::resolve_approval` + `kernel_commit_decision` so events,
    /// batch barriers and the run aggregate are part of the same write-set.
    pub fn kernel_approve_and_enqueue_dispatch(
        &self,
        run_id: &str,
        tool_call_id: &str,
        decision: &str,
        wall_now_ms: i64,
    ) -> Result<Option<crate::kernel::OutboxEffect>, String> {
        if !matches!(decision, "allow_once" | "allow_conversation") {
            return Err(format!(
                "kernel_approve_and_enqueue_dispatch only supports allow decisions: {decision}"
            ));
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (run_state, deadline): (String, Option<i64>) = transaction.query_row(
                "SELECT state, approval_deadline_wall_ms FROM kernel_runs WHERE run_id=?1",
                params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if run_state != "waiting_approval" {
                transaction.commit()?;
                return Ok(None);
            }
            let deadline = deadline.ok_or_else(|| {
                kernel_err(format!("pending approval has no deadline for {run_id}"))
            })?;
            if wall_now_ms > deadline {
                return Err(kernel_err(format!(
                    "approval decision arrived after its deadline for {run_id}/{tool_call_id}"
                )));
            }
            let affected = transaction.execute(
                "UPDATE kernel_approvals SET state=?3, decided_at=?4
                  WHERE run_id=?1 AND tool_call_id=?2 AND state='pending'",
                params![run_id, tool_call_id, decision, wall_now_ms],
            )?;
            if affected == 0 {
                transaction.commit()?;
                return Ok(None);
            }
            let tool_affected = transaction.execute(
                    "UPDATE kernel_tool_calls SET state='running',
                        dispatch_idempotency_key=?3
                      WHERE run_id=?1 AND tool_call_id=?2 AND state='waiting_approval'",
                    params![
                        run_id,
                        tool_call_id,
                        crate::kernel::dispatch_idempotency_key(run_id, tool_call_id)
                    ],
                )?;
            if tool_affected != 1 {
                return Err(kernel_err(format!(
                    "approval tool CAS did not update exactly one row for {run_id}/{tool_call_id}"
                )));
            }
            let (tool, input, batch_id): (String, String, String) = transaction.query_row(
                    "SELECT tool, canonical_input_json, batch_id FROM kernel_tool_calls
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, tool_call_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            let payload_value = serde_json::json!({
                "tool": tool,
                "input": serde_json::from_str::<serde_json::Value>(&input)
                    .map_err(|error| kernel_err(error.to_string()))?
            });
            let payload = payload_value.to_string();
            let effect_key = crate::kernel::dispatch_effect_key(tool_call_id);
            let idempotency_key = crate::kernel::dispatch_idempotency_key(run_id, tool_call_id);
            let existing: Option<(String, String, Option<String>, Option<String>, String)> =
                transaction
                    .query_row(
                        "SELECT effect_type, idempotency_key, tool_call_id, batch_id, payload_json
                           FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key=?2",
                        params![run_id, effect_key],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .optional()?;
            if let Some((kind, idem, owner_tool, owner_batch, existing_payload)) = existing {
                if kind != "dispatch_tool"
                    || idem != idempotency_key
                    || owner_tool.as_deref() != Some(tool_call_id)
                    || owner_batch.as_deref() != Some(batch_id.as_str())
                    || existing_payload != payload
                {
                    return Err(kernel_err(format!(
                        "dispatch outbox identity conflict for {run_id}/{tool_call_id}"
                    )));
                }
            } else {
                transaction.execute(
                    "INSERT INTO kernel_effect_outbox
                        (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                         payload_json, status, attempts, lease_owner, leased_at, completed_at,
                         last_error, created_at, updated_at)
                     VALUES (?1, ?2, 'dispatch_tool', ?3, ?4, ?5, ?6, 'pending', 0, NULL, NULL, NULL, NULL, ?7, ?7)",
                    params![
                        run_id,
                        effect_key,
                        idempotency_key,
                        tool_call_id,
                        batch_id,
                        payload,
                        wall_now_ms
                    ],
                )?;
                // The authoritative credential is frozen in the SAME transaction
                // as the dispatch fact; a pre-existing row is never backfilled.
                super::kernel_execution_admission::issue_dispatch_credential_in_tx(
                    &transaction,
                    run_id,
                    tool_call_id,
                    &tool,
                    &input,
                    wall_now_ms,
                )
                .map_err(kernel_err)?;
            }
            // The approval prompt effect is now satisfied (decided): complete it so
            // recovery does not re-publish an approval that was resolved.
            let prompt_affected = transaction.execute(
                "UPDATE kernel_effect_outbox
                    SET status='completed', completed_at=?3, updated_at=?3, lease_owner=NULL
                  WHERE run_id=?1 AND effect_key=?2 AND status IN ('pending','leased')",
                params![run_id, crate::kernel::approval_effect_key(tool_call_id), wall_now_ms],
            )?;
            if prompt_affected != 1 {
                return Err(kernel_err(format!(
                    "approval prompt outbox was not pending for {run_id}/{tool_call_id}"
                )));
            }
            let pending_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM kernel_approvals WHERE run_id=?1 AND state='pending'",
                params![run_id],
                |row| row.get(0),
            )?;
            if pending_count == 0 {
                transaction.execute(
                    "UPDATE kernel_runs SET state='running', approval_deadline_wall_ms=NULL, updated_at=?2
                      WHERE run_id=?1 AND state='waiting_approval'",
                    params![run_id, wall_now_ms],
                )?;
            }
            let dispatch = transaction
                .query_row(
                        "SELECT effect_key, effect_type, idempotency_key, tool_call_id, batch_id, payload_json, status, attempts
                           FROM kernel_effect_outbox
                          WHERE run_id=?1 AND effect_key=?2",
                        params![run_id, crate::kernel::dispatch_effect_key(tool_call_id)],
                        map_outbox_effect,
                )
                .optional()?;
            transaction.commit()?;
            Ok(dispatch)
        })
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
                    .to_string();
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
        let frozen_json = serde_json::to_string(frozen)
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
        let legacy_json = serde_json::to_string(&diff.legacy)
            .map_err(|error| format!("legacy disposition: {error}"))?;
        let kernel_json = serde_json::to_string(&diff.kernel)
            .map_err(|error| format!("kernel disposition: {error}"))?;
        let detail_json = diff.detail.to_string();
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
        let legacy_json = serde_json::to_string(&diff.legacy)
            .map_err(|error| format!("legacy disposition: {error}"))?;
        let kernel_json = serde_json::to_string(&diff.kernel)
            .map_err(|error| format!("kernel disposition: {error}"))?;
        let detail_json = diff.detail.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> Database {
        // Migrations (including the v48 kernel tables) run on open, so recovery
        // reads only committed durable rows — no in-memory state is relied upon.
        Database::open(std::path::PathBuf::from(":memory:")).unwrap()
    }

    fn temporary_db_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fox-kernel-{label}-{}.db", uuid::Uuid::new_v4()))
    }

    #[test]
    fn pending_approval_persists_and_is_recovered_after_restart() {
        let path = temporary_db_path("pending-restart");
        {
            let db = Database::open(path.clone()).unwrap();
            db.kernel_create_run(
                "run-1",
                "pi",
                "authoritative",
                2,
                "perm-1",
                "legacy",
                "prompt-hash",
                &frozen_config_json("authoritative", "perm-1", "prompt-hash"),
            )
            .unwrap();
            db.kernel_update_run_state("run-1", "running").unwrap();
            db.kernel_update_run_state("run-1", "waiting_approval")
                .unwrap();
            db.kernel_record_tool_batch("batch-1", "run-1", r#"["call-a"]"#)
                .unwrap();
            db.kernel_record_tool_call(
                "run-1",
                "call-a",
                "batch-1",
                "write_file",
                0,
                r#"{"path":"x.txt"}"#,
                "waiting_approval",
            )
            .unwrap();
            db.kernel_record_pending_approval("run-1", "call-a")
                .unwrap();
        }

        // Close and reopen the SQLite connection so this test crosses an actual
        // process-lifetime persistence boundary rather than re-querying one handle.
        let reopened = Database::open(path.clone()).unwrap();
        let recovered = reopened.kernel_recover_nonterminal_runs().unwrap();
        assert_eq!(recovered.len(), 1);
        let run = &recovered[0];
        assert_eq!(run.run_id, "run-1");
        assert_eq!(run.state, "waiting_approval");
        assert_eq!(run.pending_tool_calls.len(), 1);
        let call = &run.pending_tool_calls[0];
        assert_eq!(call.tool_call_id, "call-a");
        assert_eq!(call.tool, "write_file");
        assert_eq!(call.canonical_input_json, r#"{"path":"x.txt"}"#);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn approval_cas_is_once_only_and_terminal_runs_are_not_recovered() {
        let db = fresh_db();
        db.kernel_create_run(
            "run-2",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &frozen_config_json("authoritative", "perm-1", "h"),
        )
        .unwrap();
        db.kernel_update_run_state("run-2", "running").unwrap();
        db.kernel_update_run_state("run-2", "waiting_approval")
            .unwrap();
        db.kernel_record_tool_batch("b", "run-2", r#"["c1"]"#)
            .unwrap();
        db.kernel_record_tool_call("run-2", "c1", "b", "read", 0, "{}", "waiting_approval")
            .unwrap();
        db.kernel_record_pending_approval("run-2", "c1").unwrap();

        // First decision wins.
        assert_eq!(
            db.kernel_resolve_approval("run-2", "c1", "allow_once")
                .unwrap(),
            true
        );
        // A second/late decision must not re-execute the tool.
        assert_eq!(
            db.kernel_resolve_approval("run-2", "c1", "allow_once")
                .unwrap(),
            false
        );
        assert!(db.kernel_recover_nonterminal_runs().unwrap()[0]
            .pending_tool_calls
            .is_empty());

        // A terminal run is not returned for recovery.
        db.kernel_update_tool_call_state("run-2", "c1", "completed", Some(r#"{"ok":true}"#))
            .unwrap();
        db.kernel_update_run_state("run-2", "completed").unwrap();
        assert!(db.kernel_recover_nonterminal_runs().unwrap().is_empty());
    }

    #[test]
    fn decide_persist_reopen_then_resolve_pending_approval_without_regenerating() {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController};

        // A policy that requires approval for writes.
        struct WriteRequiresApproval;
        impl PolicyDecisionPort for WriteRequiresApproval {
            fn decide(
                &self,
                _run_id: &str,
                _permission_snapshot_id: &str,
                tool: &str,
                _input: &str,
            ) -> PolicyDecision {
                if tool == "write_file" {
                    PolicyDecision::RequireApproval
                } else {
                    PolicyDecision::Allow
                }
            }
        }

        let path = temporary_db_path("decision-reopen");
        let db = Database::open(path.clone()).unwrap();
        let config = crate::kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "manifest-hash".into(),
            permission_snapshot_id: "perm-v1".into(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: "h".into(),
            model_request_timeout_ms: 120_000,
            model_first_response_ms: 60_000,
            model_idle_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 600_000,
            run_execution_limited: true,
            approval_wait_timeout_ms: 3_600_000,
            provider_max_retries: 2,
            turn_max_retries: 0,
            experimental_compute_job_notice: false,
        };

        // --- Process lifetime 1: decide and durably park the approval. ---
        let (mut controller, start_effects) = RunController::start(
            "run-e2e",
            "turn-1",
            config.clone(),
            &crate::kernel::TestClock::new(0),
        )
        .unwrap();
        db.kernel_create_run(
            "run-e2e",
            "pi",
            "authoritative",
            2,
            "perm-v1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        db.kernel_update_run_state("run-e2e", "running").unwrap();
        for effect in &start_effects {
            if let crate::kernel::Effect::AppendEvent {
                seq,
                event_type,
                payload_json,
            } = effect
            {
                db.kernel_append_event("run-e2e", *seq as i64, event_type, payload_json)
                    .unwrap();
            }
        }

        let effects = controller
            .propose_tool_batch(
                "batch-1",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-1".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"a.txt"}"#.into(),
                    source_order: 0,
                }],
                &WriteRequiresApproval,
                0,
                0,
            )
            .unwrap();
        // Persist the decision effects as the adapter would.
        db.kernel_record_tool_batch("batch-1", "run-e2e", r#"["call-1"]"#)
            .unwrap();
        for effect in &effects {
            match effect {
                crate::kernel::Effect::AppendEvent {
                    seq,
                    event_type,
                    payload_json,
                } => {
                    db.kernel_append_event("run-e2e", *seq as i64, event_type, payload_json)
                        .unwrap();
                    if event_type == "run.waiting_approval" {
                        db.kernel_update_run_state("run-e2e", "waiting_approval")
                            .unwrap();
                    }
                }
                crate::kernel::Effect::RequestApproval {
                    tool_call_id,
                    tool,
                    input_json,
                } => {
                    db.kernel_record_tool_call(
                        "run-e2e",
                        tool_call_id,
                        "batch-1",
                        tool,
                        0,
                        input_json,
                        "waiting_approval",
                    )
                    .unwrap();
                    db.kernel_record_pending_approval("run-e2e", tool_call_id)
                        .unwrap();
                }
                _ => {}
            }
        }

        // --- Restart: discard both the controller and database handle. ---
        drop(controller);
        drop(db);
        let reopened = Database::open(path.clone()).unwrap();
        let recovered = reopened.kernel_recover_nonterminal_runs().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].pending_tool_calls.len(), 1);
        assert_eq!(recovered[0].pending_tool_calls[0].tool_call_id, "call-1");
        // The model is NOT asked to regenerate: the exact tool + canonical input
        // survive durably.
        assert_eq!(recovered[0].pending_tool_calls[0].tool, "write_file");
        assert_eq!(
            recovered[0].pending_tool_calls[0].canonical_input_json,
            r#"{"path":"a.txt"}"#
        );

        let snapshot = reopened.kernel_build_snapshot("run-e2e").unwrap();
        assert_eq!(snapshot.state, "waiting_approval");
        assert_eq!(
            snapshot.waiting_approval_tool_call_ids,
            vec!["call-1".to_string()]
        );

        // --- Human approves after restart: CAS once, then the tool dispatches. ---
        assert_eq!(
            reopened
                .kernel_resolve_approval("run-e2e", "call-1", "allow_once")
                .unwrap(),
            true
        );
        // The durable transition moves the call to running exactly once. Production
        // dispatch/replay is deliberately not claimed here until the Host adapter is wired.
        let snapshot = reopened.kernel_build_snapshot("run-e2e").unwrap();
        assert_eq!(snapshot.state, "running");
        assert_eq!(snapshot.pending_tool_call_ids, vec!["call-1".to_string()]);
        assert!(reopened.kernel_recover_nonterminal_runs().unwrap()[0]
            .pending_tool_calls
            .is_empty());
        assert_eq!(
            reopened
                .kernel_resolve_approval("run-e2e", "call-1", "allow_once")
                .unwrap(),
            false
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn kernel_events_are_idempotent_on_run_and_seq() {
        let db = fresh_db();
        db.kernel_create_run(
            "run-3",
            "pi",
            "authoritative",
            2,
            "p",
            "legacy",
            "h",
            &frozen_config_json("authoritative", "p", "h"),
        )
        .unwrap();
        assert_eq!(
            db.kernel_append_event("run-3", 1, "run.started", "{}")
                .unwrap(),
            true
        );
        // Replaying the same (run, seq) is ignored, not duplicated.
        assert_eq!(
            db.kernel_append_event("run-3", 1, "run.started", "{}")
                .unwrap(),
            false
        );
        assert_eq!(
            db.kernel_append_event("run-3", 2, "run.waiting_approval", "{}")
                .unwrap(),
            true
        );
    }

    #[test]
    fn conflicting_event_replay_is_rejected_instead_of_silently_ignored() {
        let db = fresh_db();
        db.kernel_create_run(
            "run-events",
            "pi",
            "authoritative",
            2,
            "p",
            "legacy",
            "h",
            &frozen_config_json("authoritative", "p", "h"),
        )
        .unwrap();
        db.kernel_append_event("run-events", 1, "run.started", "{}")
            .unwrap();

        assert!(db
            .kernel_append_event("run-events", 3, "run.completed", "{}")
            .is_err());

        let conflict =
            db.kernel_append_event("run-events", 1, "run.failed", r#"{"code":"different"}"#);
        assert!(conflict.is_err());
    }

    #[test]
    fn persisted_state_machines_reject_terminal_rewrites_and_approval_bypass() {
        let db = fresh_db();
        db.kernel_create_run(
            "run-state",
            "pi",
            "authoritative",
            2,
            "p",
            "legacy",
            "h",
            &frozen_config_json("authoritative", "p", "h"),
        )
        .unwrap();
        db.kernel_update_run_state("run-state", "running").unwrap();
        db.kernel_record_tool_batch("batch-state", "run-state", r#"["call-state"]"#)
            .unwrap();
        db.kernel_record_tool_call(
            "run-state",
            "call-state",
            "batch-state",
            "write_file",
            0,
            "{}",
            "waiting_approval",
        )
        .unwrap();

        assert!(db.kernel_update_run_state("run-state", "failed").is_err());

        // A persisted tool result cannot skip the approval transition.
        assert!(db
            .kernel_update_tool_call_state(
                "run-state",
                "call-state",
                "completed",
                Some(r#"{"ok":true}"#),
            )
            .is_err());

        db.kernel_update_tool_call_state(
            "run-state",
            "call-state",
            "failed",
            Some(r#"{"code":"denied"}"#),
        )
        .unwrap();
        assert!(db
            .kernel_update_tool_call_state(
                "run-state",
                "call-state",
                "completed",
                Some(r#"{"ok":true}"#),
            )
            .is_err());

        db.kernel_update_run_state("run-state", "completed")
            .unwrap();
        assert!(db.kernel_update_run_state("run-state", "running").is_err());
    }

    #[test]
    fn conflicting_batch_and_tool_identities_are_rejected() {
        let db = fresh_db();
        for run_id in ["run-a", "run-b"] {
            db.kernel_create_run(
                run_id,
                "pi",
                "authoritative",
                2,
                "p",
                "legacy",
                "h",
                &frozen_config_json("authoritative", "p", "h"),
            )
            .unwrap();
        }
        db.kernel_record_tool_batch("batch", "run-a", r#"["call"]"#)
            .unwrap();
        assert!(db
            .kernel_record_tool_batch("batch", "run-b", r#"["call"]"#)
            .is_err());

        assert!(db
            .kernel_record_tool_call("run-a", "call", "batch", "read_file", 1, "{}", "running",)
            .is_err());
        db.kernel_record_tool_call("run-a", "call", "batch", "read_file", 0, "{}", "running")
            .unwrap();
        assert!(db
            .kernel_record_tool_call("run-a", "call", "batch", "write_file", 0, "{}", "running",)
            .is_err());
    }

    // ---- Phase 3B crash-window and recovery tests ----

    fn phase3b_config() -> crate::kernel::RunFrozenConfig {
        crate::kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "manifest-hash".into(),
            permission_snapshot_id: "perm-1".into(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: "h".into(),
            model_request_timeout_ms: 120_000,
            model_first_response_ms: 60_000,
            model_idle_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 600_000,
            run_execution_limited: true,
            approval_wait_timeout_ms: 3_600_000,
            provider_max_retries: 2,
            turn_max_retries: 1,
            experimental_compute_job_notice: false,
        }
    }

    #[test]
    fn conversation_snapshot_is_owned_authoritative_and_payload_free() {
        let db = fresh_db();
        let notifications = db.clone().subscribe_kernel_changes();
        notifications.try_recv().unwrap(); // Subscribe/reopen initial invalidation.
        db.with_connection(|connection| {
            connection.execute("INSERT INTO conversations(id, agent_id, title, status, created_at, updated_at)
                SELECT 'snapshot-conversation', id, 'snapshot', 'active', 1, 1 FROM agents LIMIT 1", [])?;
            for run in ["snapshot-auth", "snapshot-legacy"] {
                connection.execute("INSERT INTO runs(id, conversation_id, status, model, created_at)
                    VALUES (?1, 'snapshot-conversation', 'running', 'test', 1)", [run])?;
            }
            Ok(())
        }).unwrap();
        assert!(db.kernel_conversation_snapshot("snapshot-conversation", "snapshot-legacy").unwrap().is_none());
        assert!(db.kernel_conversation_snapshot("other-conversation", "snapshot-legacy").is_err());
        assert!(db.kernel_conversation_snapshot("snapshot-conversation", "missing").is_err());
        let config = phase3b_config();
        db.kernel_create_run("snapshot-auth", "pi", "authoritative", 2, "perm-1", "legacy", "h",
            &serde_json::to_string(&config).unwrap()).unwrap();
        let (mut controller, effects) = crate::kernel::RunController::start(
            "snapshot-auth", "turn-1", config, &crate::kernel::TestClock::new(0)).unwrap();
        db.kernel_commit_decision("snapshot-auth", 0, &controller.persist_command(&effects)).unwrap();
        notifications.try_recv().unwrap();
        // Exact replay causes neither a new revision nor a spurious UI refresh.
        db.kernel_commit_decision("snapshot-auth", 0, &controller.persist_command(&effects)).unwrap();
        assert!(notifications.try_recv().is_err());
        let snapshot = db.kernel_conversation_snapshot("snapshot-conversation", "snapshot-auth").unwrap().unwrap();
        assert_eq!(snapshot.run_id, "snapshot-auth");
        assert_eq!(snapshot.state, "running");
        assert_eq!(snapshot.last_event_seq, controller.last_event_seq().to_string());
        let json = serde_json::to_string(&snapshot).unwrap();
        for forbidden in ["payloadJson", "resultJson", "permissionSnapshot", "pendingEffects"] {
            assert!(!json.contains(forbidden));
        }
        assert!(db.kernel_conversation_snapshot("other-conversation", "snapshot-auth").is_err());
        let cancel = controller.request_cancel();
        db.with_connection(|connection| connection.execute_batch(
            "CREATE TRIGGER reject_kernel_commit BEFORE UPDATE ON kernel_runs BEGIN SELECT RAISE(ABORT, 'injected rollback'); END;"
        )).unwrap();
        assert!(db.kernel_commit_decision("snapshot-auth", 1, &controller.persist_command(&cancel)).is_err());
        assert!(notifications.try_recv().is_err());
        assert_eq!(db.kernel_build_full_snapshot("snapshot-auth").unwrap().state, "running");
        db.with_connection(|connection| connection.execute_batch("DROP TRIGGER reject_kernel_commit;")).unwrap();
        db.kernel_commit_decision("snapshot-auth", 1, &controller.persist_command(&cancel)).unwrap();
        notifications.try_recv().unwrap();
        assert_ne!(db.kernel_build_full_snapshot("snapshot-auth").unwrap().state, "running");
        // Corrupt authority must fail closed, never turn into a Legacy fallback.
        db.with_connection(|connection| {
            connection.execute("UPDATE kernel_runs SET frozen_config_json='{}' WHERE run_id='snapshot-auth'", [])?;
            Ok(())
        }).unwrap();
        assert!(db.kernel_conversation_snapshot("snapshot-conversation", "snapshot-auth").is_err());
    }

    #[test]
    fn single_effect_lease_is_authoritative_scoped_and_does_not_prelease_siblings() {
        let db = Database::open(temporary_db_path("single-effect-lease")).unwrap();
        for mode in ["legacy", "authoritative"] {
            let config = crate::kernel::RunFrozenConfig { kernel_mode: mode.into(), ..phase3b_config() };
            db.kernel_create_run(mode, "pi", mode, 2, "perm-1", "legacy", "h", &serde_json::to_string(&config).unwrap()).unwrap();
            db.kernel_update_run_state(mode, "running").unwrap();
            for key in ["one", "two"] {
                db.with_connection(|connection| connection.execute(
                    "INSERT INTO kernel_effect_outbox(run_id,effect_key,effect_type,idempotency_key,payload_json,status,attempts,created_at,updated_at)
                     VALUES(?1,?2,'publish_snapshot',?2,'{}','pending',0,0,0)", params![mode, key],
                )).unwrap();
            }
        }
        assert!(db.kernel_outbox_lease_effect("legacy", "one", "owner").unwrap().is_none());
        assert!(db.kernel_outbox_lease_pending("legacy", "owner").unwrap().is_empty());
        assert!(db.kernel_outbox_lease_effect("authoritative", "one", "").is_err());
        assert_eq!(db.kernel_outbox_lease_effect("authoritative", "one", "owner-a").unwrap().unwrap().attempts, 1);
        assert!(db.kernel_outbox_lease_effect("authoritative", "one", "owner-b").unwrap().is_none());
        let facts = db.kernel_recovery_facts("authoritative").unwrap().unwrap();
        assert_eq!(facts.open_outbox.iter().find(|effect| effect.effect_key == "two").unwrap().status, crate::kernel::OutboxStatus::Pending);
        assert!(db.kernel_outbox_complete("authoritative", "one", "foreign").is_err());
        assert!(db.kernel_outbox_fail("authoritative", "one", "failed", "foreign").is_err());
        db.kernel_outbox_complete("authoritative", "one", "owner-a").unwrap();
        assert!(db.kernel_outbox_mark_requires_reconcile("authoritative", "one", "owner-a").is_err());
        let global = db.kernel_outbox_lease_all_pending("startup").unwrap();
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].0, "authoritative");
        assert_eq!(global[0].1.effect_key, "two");
    }

    fn frozen_config_json(kernel_mode: &str, permission: &str, prompt_hash: &str) -> String {
        let mut config = phase3b_config();
        config.kernel_mode = kernel_mode.to_string();
        config.permission_snapshot_id = permission.to_string();
        config.prompt_config_hash = prompt_hash.to_string();
        serde_json::to_string(&config).unwrap()
    }

    /// The key acceptance flow: an approval-required run is durably committed, the
    /// process dies (controller + connection dropped), a fresh process opens the
    /// file DB, rehydrates, the human approves, the fake outbox executor dispatches
    /// once, the tool result lands once, and the batch barrier fires once — without
    /// the model regenerating the tool call.
    #[test]
    fn crash_after_approval_before_dispatch_is_recovered_without_regeneration() {
        use crate::kernel::{
            plan_recovery, ApprovalDecision, PolicyDecision, PolicyDecisionPort, RunController,
        };
        struct WriteRequiresApproval;
        impl PolicyDecisionPort for WriteRequiresApproval {
            fn decide(&self, _: &str, _: &str, tool: &str, _: &str) -> PolicyDecision {
                if tool == "write_file" {
                    PolicyDecision::RequireApproval
                } else {
                    PolicyDecision::Allow
                }
            }
        }
        let path = temporary_db_path("crash-approval");
        let run_id = "run-crash";
        let config = phase3b_config();

        // --- Process lifetime 1: decide + transactionally commit, then crash. ---
        {
            let db = Database::open(path.clone()).unwrap();
            let clock = crate::kernel::TestClock::new(0);
            db.kernel_create_run(
                run_id,
                "pi",
                "authoritative",
                2,
                "perm-1",
                "legacy",
                "h",
                &serde_json::to_string(&config).unwrap(),
            )
            .unwrap();
            let (mut controller, start_effects) =
                RunController::start(run_id, "turn-1", config.clone(), &clock).unwrap();
            // Commit the run start (state running + run.started event) first.
            let start_cmd = controller.persist_command(&start_effects);
            db.kernel_commit_decision(run_id, 0, &start_cmd).unwrap();
            let effects = controller
                .propose_tool_batch(
                    "batch-crash",
                    vec![crate::kernel::ToolCallRequest {
                        tool_call_id: "call-x".into(),
                        tool: "write_file".into(),
                        canonical_input_json: r#"{"path":"a.txt"}"#.into(),
                        source_order: 0,
                    }],
                    &WriteRequiresApproval,
                    0,
                    0,
                )
                .unwrap();
            controller.set_approval_deadline(config.approval_wait_timeout_ms);
            let effects2 = {
                // Re-derive so the deadline is reflected in the persisted command.
                let _ = &effects;
                effects
            };
            let cmd = controller.persist_command(&effects2);
            // One atomic decision commit (events + run/tool state + approval + outbox).
            db.kernel_commit_decision(run_id, 0, &cmd).unwrap();
            // The run is parked waiting for approval; no dispatch yet.
        } // <-- "crash": controller and db handle dropped.

        // --- Process lifetime 2: reopen, rehydrate, approve through the decision
        // core, commit approval CAS + dispatch intent, then crash before dispatch. ---
        {
            let db = Database::open(path.clone()).unwrap();
            let facts = db.kernel_recovery_facts(run_id).unwrap().unwrap();
            assert_eq!(facts.pending_approvals.len(), 1);
            assert_eq!(facts.pending_approvals[0].tool_call_id, "call-x");
            let plan = plan_recovery(vec![facts]);
            assert_eq!(plan.republish_approvals.len(), 1);
            assert!(plan.redispatch_pending.is_empty());
            assert!(plan.uncertain_leased.is_empty());

            let rehydrated = db.kernel_rehydrate(run_id).unwrap().unwrap();
            assert_eq!(rehydrated.config.capability_manifest_hash, "manifest-hash");
            assert_eq!(rehydrated.state, crate::kernel::RunState::WaitingApproval);
            let mut controller = RunController::rehydrate(rehydrated).unwrap();
            let approved = controller
                .resolve_approval("call-x", ApprovalDecision::AllowOnce, 1_000)
                .unwrap();
            db.kernel_commit_decision(run_id, 1_000, &controller.persist_command(&approved))
                .unwrap();
            let snapshot = db.kernel_build_full_snapshot(run_id).unwrap();
            let dispatch = snapshot
                .pending_effects
                .iter()
                .find(|effect| effect.kind == crate::kernel::OutboxEffectKind::DispatchTool)
                .expect("approval decision durably enqueued dispatch");
            let payload: serde_json::Value = serde_json::from_str(&dispatch.payload_json).unwrap();
            assert_eq!(payload["tool"], "write_file");
            assert_eq!(payload["input"]["path"], "a.txt");
        } // <-- crash after approval commit, before dispatch lease.

        // --- Process lifetime 3: recover the exact pending dispatch. ---
        let db = Database::open(path.clone()).unwrap();
        let recovered_plan =
            plan_recovery(vec![db.kernel_recovery_facts(run_id).unwrap().unwrap()]);
        assert_eq!(recovered_plan.redispatch_pending.len(), 1);
        let dispatch = &recovered_plan.redispatch_pending[0].1;
        assert_eq!(
            dispatch.idempotency_key,
            crate::kernel::dispatch_idempotency_key(run_id, "call-x")
        );
        let payload: serde_json::Value = serde_json::from_str(&dispatch.payload_json).unwrap();
        assert_eq!(payload["input"]["path"], "a.txt");

        let rehydrated2 = db.kernel_rehydrate(run_id).unwrap().unwrap();
        assert_eq!(
            rehydrated2.tools[0].state,
            crate::kernel::ToolCallState::Running
        );
        let mut controller = RunController::rehydrate(rehydrated2).unwrap();
        assert_eq!(controller.state(), crate::kernel::RunState::Running);

        // The fake outbox executor leases pending effects and dispatches exactly once.
        let leased = db
            .kernel_outbox_lease_pending(run_id, "executor-1")
            .unwrap();
        let dispatch_leased = leased
            .iter()
            .find(|e| e.kind == crate::kernel::OutboxEffectKind::DispatchTool)
            .expect("dispatch leased");
        assert_eq!(dispatch_leased.effect_key, dispatch.effect_key);
        // A second lease must not hand the same effect out again.
        assert!(db
            .kernel_outbox_lease_pending(run_id, "executor-2")
            .unwrap()
            .is_empty());

        // Apply the dispatch outcome through the controller and persist exactly once.
        let settled = controller
            .tool_settled("call-x", true, r#"{"written":true}"#)
            .unwrap();
        let cmd = controller.persist_command(&settled);
        db.kernel_commit_decision(run_id, 1_000, &cmd).unwrap();
        // Tool result + dispatch completion were committed atomically. The batch
        // delivery is now the only pending effect and is itself durable.
        let after_settlement = db.kernel_build_full_snapshot(run_id).unwrap();
        assert!(after_settlement
            .pending_effects
            .iter()
            .all(|effect| { effect.kind == crate::kernel::OutboxEffectKind::DeliverToolBatch }));
        let deliveries = db
            .kernel_outbox_lease_pending(run_id, "batch-delivery-executor")
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].status, crate::kernel::OutboxStatus::Leased);
        db.kernel_outbox_complete(run_id, &deliveries[0].effect_key, "batch-delivery-executor")
            .unwrap();

        // Tool completed exactly once; batch barrier handed off in source order.
        let snap = db.kernel_build_full_snapshot(run_id).unwrap();
        assert_eq!(snap.tool_calls.len(), 1);
        assert_eq!(snap.tool_calls[0].state, "completed");
        assert!(snap.pending_effects.is_empty());

        // No second approval row / no second logical tool call exists.
        let facts2 = db.kernel_recovery_facts(run_id).unwrap().unwrap();
        assert!(facts2.pending_approvals.is_empty());
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    /// A dispatch leased by a process that then crashed is NOT blindly re-run; it
    /// surfaces as an uncertain effect for idempotency-key reconciliation.
    #[test]
    fn leased_effect_after_crash_is_not_blindly_rerun() {
        use crate::kernel::{plan_recovery, PolicyDecision, PolicyDecisionPort, RunController};
        struct RequireApproval;
        impl PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::RequireApproval
            }
        }
        let config = phase3b_config();
        let path = temporary_db_path("crash-lease");
        let run_id = "run-lease";
        {
            let db = Database::open(path.clone()).unwrap();
            db.kernel_create_run(
                run_id,
                "pi",
                "authoritative",
                2,
                "perm-1",
                "legacy",
                "h",
                &serde_json::to_string(&config).unwrap(),
            )
            .unwrap();
            let clock = crate::kernel::TestClock::new(0);
            let (mut controller, start) =
                RunController::start(run_id, "turn-lease", config.clone(), &clock).unwrap();
            db.kernel_commit_decision(run_id, 0, &controller.persist_command(&start))
                .unwrap();
            let proposal = controller
                .propose_tool_batch(
                    "batch-lease",
                    vec![crate::kernel::ToolCallRequest {
                        tool_call_id: "call-l".into(),
                        tool: "read".into(),
                        canonical_input_json: r#"{"path":"x"}"#.into(),
                        source_order: 0,
                    }],
                    &RequireApproval,
                    0,
                    0,
                )
                .unwrap();
            db.kernel_commit_decision(run_id, 0, &controller.persist_command(&proposal))
                .unwrap();
            // Enqueue an approved dispatch directly and lease it, then crash without
            // completing.
            db.kernel_approve_and_enqueue_dispatch(run_id, "call-l", "allow_once", 0)
                .unwrap();
            let leased = db
                .kernel_outbox_lease_pending(run_id, "executor-doomed")
                .unwrap();
            assert_eq!(leased.len(), 1);
        } // crash with the effect still 'leased'
        let db = Database::open(path.clone()).unwrap();
        let facts = db.kernel_recovery_facts(run_id).unwrap().unwrap();
        let plan = plan_recovery(vec![facts]);
        // Nothing is silently re-dispatched from pending...
        assert!(plan.redispatch_pending.is_empty());
        // ...the leased effect is surfaced for reconcile, not rerun blind.
        assert_eq!(plan.uncertain_leased.len(), 1);
        // Marking it requires_reconcile prevents any future duplicate side effect.
        let key = plan.uncertain_leased[0].effect.effect_key.clone();
        assert!(db.kernel_outbox_mark_requires_reconcile(run_id, &key, "").is_err());
        assert!(db.kernel_outbox_mark_requires_reconcile(run_id, &key, "foreign-executor").is_err());
        assert_eq!(db.kernel_recovery_facts(run_id).unwrap().unwrap().open_outbox[0].status, crate::kernel::OutboxStatus::Leased);
        db.kernel_outbox_mark_requires_reconcile(run_id, &key, "executor-doomed")
            .unwrap();
        assert!(db
            .kernel_outbox_lease_pending(run_id, "executor-3")
            .unwrap()
            .is_empty());
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn multi_tool_batch_persists_each_call_and_snapshot_keeps_source_order() {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController};
        struct AllowAll;
        impl PolicyDecisionPort for AllowAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::Allow
            }
        }
        let db = fresh_db();
        let run_id = "run-multi";
        let config = phase3b_config();
        db.kernel_create_run(
            run_id,
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) = RunController::start(run_id, "t", config, &clock).unwrap();
        db.kernel_commit_decision(run_id, 0, &controller.persist_command(&start))
            .unwrap();
        let effects = controller
            .propose_tool_batch(
                "batch-multi",
                vec![
                    crate::kernel::ToolCallRequest {
                        tool_call_id: "a".into(),
                        tool: "read".into(),
                        canonical_input_json: r#"{"path":"1"}"#.into(),
                        source_order: 0,
                    },
                    crate::kernel::ToolCallRequest {
                        tool_call_id: "b".into(),
                        tool: "ls".into(),
                        canonical_input_json: r#"{"path":"2"}"#.into(),
                        source_order: 1,
                    },
                    crate::kernel::ToolCallRequest {
                        tool_call_id: "c".into(),
                        tool: "grep".into(),
                        canonical_input_json: r#"{"path":"3"}"#.into(),
                        source_order: 2,
                    },
                ],
                &AllowAll,
                0,
                0,
            )
            .unwrap();
        db.kernel_commit_decision(run_id, 0, &controller.persist_command(&effects))
            .unwrap();
        let leased = db
            .kernel_outbox_lease_pending(run_id, "multi-executor")
            .unwrap();
        assert_eq!(leased.len(), 3);
        assert!(leased
            .iter()
            .all(|effect| effect.status == crate::kernel::OutboxStatus::Leased));

        // Complete OUT OF ORDER: b, then c, then a. Each result persists once.
        for id in ["b", "c", "a"] {
            let eff = controller.tool_settled(id, true, r#"{"ok":true}"#).unwrap();
            db.kernel_commit_decision(run_id, 0, &controller.persist_command(&eff))
                .unwrap();
        }
        // Snapshot keeps all three with batch source order; barrier was emitted.
        let snap = db.kernel_build_full_snapshot(run_id).unwrap();
        assert_eq!(snap.batches[0].ordered_tool_call_ids, vec!["a", "b", "c"]);
        assert_eq!(snap.tool_calls.len(), 3);
        assert!(snap.tool_calls.iter().all(|t| t.state == "completed"));
        // Every tool call persisted independently (no last-result-wins loss).
        let completed: Vec<String> = snap
            .tool_calls
            .iter()
            .map(|t| t.tool_call_id.clone())
            .collect();
        assert_eq!(completed, vec!["a", "b", "c"]);
    }

    #[test]
    fn decision_commit_rejects_event_gaps_and_conflicting_outbox_replay() {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController};
        struct AllowAll;
        impl PolicyDecisionPort for AllowAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::Allow
            }
        }

        let db = fresh_db();
        let config = phase3b_config();
        db.kernel_create_run(
            "run-conflicts",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start("run-conflicts", "turn-1", config, &clock).unwrap();
        let mut gap = controller.persist_command(&start);
        gap.events[0].seq = 2;
        assert!(db.kernel_commit_decision("run-conflicts", 1, &gap).is_err());
        let event_count: i64 = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM kernel_events WHERE run_id='run-conflicts'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(event_count, 0);

        db.kernel_commit_decision("run-conflicts", 1, &controller.persist_command(&start))
            .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-conflicts",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-1".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"a.txt"}"#.into(),
                    source_order: 0,
                }],
                &AllowAll,
                0,
                1,
            )
            .unwrap();
        let command = controller.persist_command(&proposal);
        db.kernel_commit_decision("run-conflicts", 1, &command)
            .unwrap();
        let mut conflicting = command.clone();
        conflicting.outbox[0].payload_json =
            r#"{"input":{"path":"different.txt"},"tool":"write_file"}"#.into();
        assert!(db
            .kernel_commit_decision("run-conflicts", 1, &conflicting)
            .is_err());
    }

    #[test]
    fn tool_result_and_dispatch_completion_are_one_transaction() {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController};
        struct AllowAll;
        impl PolicyDecisionPort for AllowAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::Allow
            }
        }

        let db = fresh_db();
        let config = phase3b_config();
        db.kernel_create_run(
            "run-atomic-settle",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start("run-atomic-settle", "turn-1", config, &clock).unwrap();
        db.kernel_commit_decision("run-atomic-settle", 1, &controller.persist_command(&start))
            .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-atomic",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-atomic".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"atomic.txt"}"#.into(),
                    source_order: 0,
                }],
                &AllowAll,
                0,
                1,
            )
            .unwrap();
        db.kernel_commit_decision(
            "run-atomic-settle",
            1,
            &controller.persist_command(&proposal),
        )
        .unwrap();
        let settled = controller
            .tool_settled("call-atomic", true, r#"{"ok":true}"#)
            .unwrap();
        let settled_command = controller.persist_command(&settled);

        // A result cannot land before the dispatch intent was leased/executed.
        assert!(db
            .kernel_commit_decision("run-atomic-settle", 2, &settled_command)
            .is_err());
        let leased = db
            .kernel_outbox_lease_pending("run-atomic-settle", "executor-atomic")
            .unwrap();
        assert_eq!(leased.len(), 1);
        db.kernel_commit_decision("run-atomic-settle", 2, &settled_command)
            .unwrap();

        let dispatch_status: String = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM kernel_effect_outbox
                      WHERE run_id='run-atomic-settle' AND effect_key='dispatch:call-atomic'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(dispatch_status, "completed");
        assert_eq!(
            db.kernel_build_full_snapshot("run-atomic-settle")
                .unwrap()
                .tool_calls[0]
                .state,
            "completed"
        );
    }

    #[test]
    fn approval_commit_binds_displayed_policy_version_and_refuses_legacy_reissued_ticket() {
        use crate::kernel::{ApprovalDecision, PolicyDecision, PolicyDecisionPort, RunController};
        struct RequireApproval;
        impl PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::RequireApproval
            }
        }

        let db = fresh_db();
        let mut config = phase3b_config();
        config.approval_wait_timeout_ms = 100;
        let conv=db.create_conversation("fox-general",Some("approval version"),None,None).unwrap();
        db.with_connection(|c|{c.execute("INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES('run-late-approval',?1,'running','test',1)",[&conv.id])?;Ok(())}).unwrap();
        db.kernel_create_run(
            "run-late-approval",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start("run-late-approval", "turn-1", config, &clock).unwrap();
        db.kernel_commit_decision(
            "run-late-approval",
            1_000,
            &controller.persist_command(&start),
        )
        .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-late",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-late".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"late.txt"}"#.into(),
                    source_order: 0,
                }],
                &RequireApproval,
                0,
                1_000,
            )
            .unwrap();
        db.kernel_commit_decision(
            "run-late-approval",
            1_000,
            &controller.persist_command(&proposal),
        )
        .unwrap();
        let approved = controller
            .resolve_approval("call-late", ApprovalDecision::AllowOnce, 0)
            .unwrap();
        let approved_command = controller.persist_command(&approved);

        let version=db.execution_policy(&conv.id).unwrap().version;
        let stale=db.kernel_commit_decision_with_approval_version("run-late-approval",1_001,&approved_command,version+1).unwrap_err();
        assert!(stale.contains("stale_approval"));
        db.with_connection(|c| {c.execute("INSERT INTO kernel_approval_history(run_id,tool_call_id,state,requested_at,created_at) VALUES('run-late-approval','call-late','expired',1,1)",[])?;Ok(())}).unwrap();
        assert!(db.kernel_commit_decision("run-late-approval",1_001,&approved_command).unwrap_err().contains("stale_approval"));
        db.kernel_commit_decision_with_approval_version("run-late-approval",1_001,&approved_command,version).unwrap();
        let facts=db.kernel_recovery_facts("run-late-approval").unwrap().unwrap();
        assert_eq!(facts.pending_approvals.len(),0);
    }

    fn policy_expired_waiting_approval_fixture(
        run_id: &str,
    ) -> (Database, String, u64, u64) {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController};
        struct RequireApproval;
        impl PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::RequireApproval
            }
        }

        let db = fresh_db();
        let config = phase3b_config();
        let conversation = db
            .create_conversation("fox-general", Some("policy expired wait"), None, None)
            .unwrap();
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO runs(id,conversation_id,status,model,created_at)
                 VALUES(?1,?2,'running','test',1)",
                params![run_id, conversation.id],
            )?;
            Ok(())
        })
        .unwrap();
        db.kernel_create_run(
            run_id,
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start(run_id, "turn-1", config, &clock).unwrap();
        db.kernel_commit_decision(run_id, 1_000, &controller.persist_command(&start))
            .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-policy",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-policy".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"policy.txt"}"#.into(),
                    source_order: 0,
                }],
                &RequireApproval,
                0,
                1_000,
            )
            .unwrap();
        db.kernel_commit_decision(run_id, 1_000, &controller.persist_command(&proposal))
            .unwrap();
        let old = db.execution_policy(&conversation.id).unwrap();
        let desired = if old.mode == "allow" { "ask" } else { "allow" };
        let current = db
            .change_execution_policy(&conversation.id, "expire-old-ticket", old.version, desired)
            .unwrap();
        assert_eq!(current.version, old.version + 1);
        let approval: (String, Option<i64>, Option<u64>) = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT state,decided_at,policy_version
                       FROM kernel_approvals
                      WHERE run_id=?1 AND tool_call_id='call-policy'",
                    params![run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(approval, ("expired".into(), None, Some(old.version)));
        (db, run_id.to_owned(), old.version, current.version)
    }

    #[test]
    fn rehydrate_allows_only_unconsumed_policy_expiry_pending_reevaluation() {
        use crate::kernel::plan_recovery;
        let (db, run_id, _, _) =
            policy_expired_waiting_approval_fixture("run-policy-reevaluate");
        let rehydrated = db.kernel_rehydrate(&run_id).unwrap().unwrap();
        assert_eq!(rehydrated.state, crate::kernel::RunState::WaitingApproval);
        assert_eq!(
            rehydrated.tools[0].state,
            crate::kernel::ToolCallState::WaitingApproval
        );
        let recovery = plan_recovery(vec![db.kernel_recovery_facts(&run_id).unwrap().unwrap()]);
        assert!(recovery.republish_approvals.is_empty());
        assert!(recovery.redispatch_pending.is_empty());
    }

    #[test]
    fn rehydrate_rejects_expired_waits_without_authoritative_reevaluation_proof() {
        let (same_version, run_id, _, current_version) =
            policy_expired_waiting_approval_fixture("run-policy-same-version");
        same_version
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE kernel_approvals SET policy_version=?2
                      WHERE run_id=?1 AND tool_call_id='call-policy'",
                    params![run_id, current_version],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(same_version.kernel_rehydrate(&run_id).is_err());

        let (timed_out, run_id, _, _) =
            policy_expired_waiting_approval_fixture("run-policy-timeout");
        timed_out
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE kernel_approvals SET decided_at=2000
                      WHERE run_id=?1 AND tool_call_id='call-policy'",
                    params![run_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(timed_out.kernel_rehydrate(&run_id).is_err());

        let (missing, run_id, _, _) =
            policy_expired_waiting_approval_fixture("run-policy-missing-ticket");
        missing
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM kernel_approvals
                      WHERE run_id=?1 AND tool_call_id='call-policy'",
                    params![run_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(missing.kernel_rehydrate(&run_id).is_err());

        let (consumed, run_id, _, _) =
            policy_expired_waiting_approval_fixture("run-policy-consumed");
        consumed
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO kernel_effect_outbox(
                        run_id,effect_key,effect_type,idempotency_key,tool_call_id,batch_id,
                        payload_json,status,attempts,created_at,updated_at)
                     VALUES(?1,'dispatch:call-policy','dispatch_tool','consumed',
                            'call-policy','batch-policy','{}','pending',0,2,2)",
                    params![run_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(consumed.kernel_rehydrate(&run_id).is_err());
    }

    #[test]
    fn approval_commit_rejects_late_human_decision() {
        use crate::kernel::{ApprovalDecision, PolicyDecision, PolicyDecisionPort, RunController};
        struct RequireApproval;
        impl PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::RequireApproval
            }
        }

        let db = fresh_db();
        let mut config = phase3b_config();
        config.approval_wait_timeout_ms = 100;
        db.kernel_create_run(
            "run-late-approval",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start("run-late-approval", "turn-1", config, &clock).unwrap();
        db.kernel_commit_decision(
            "run-late-approval",
            1_000,
            &controller.persist_command(&start),
        )
        .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-late",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-late".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"late.txt"}"#.into(),
                    source_order: 0,
                }],
                &RequireApproval,
                0,
                1_000,
            )
            .unwrap();
        db.kernel_commit_decision(
            "run-late-approval",
            1_000,
            &controller.persist_command(&proposal),
        )
        .unwrap();
        let approved = controller
            .resolve_approval("call-late", ApprovalDecision::AllowOnce, 0)
            .unwrap();
        let approved_command = controller.persist_command(&approved);

        let mut premature_expiry = approved_command.clone();
        premature_expiry.approval_resolutions[0].state = "expired".into();
        premature_expiry.tools[0].state = crate::kernel::ToolCallState::Failed;
        premature_expiry.tools[0].dispatch_idempotency_key = None;
        premature_expiry.outbox.clear();
        let early_error = db
            .kernel_commit_decision("run-late-approval", 1_099, &premature_expiry)
            .unwrap_err();
        assert!(early_error.contains("cannot expire before its deadline"));

        let mut mismatched_denial = approved_command.clone();
        mismatched_denial.approval_resolutions[0].state = "denied".into();
        let mismatch_error = db
            .kernel_commit_decision("run-late-approval", 1_001, &mismatched_denial)
            .unwrap_err();
        assert!(mismatch_error.contains("approval resolution conflicts"));

        assert!(db
            .kernel_commit_decision("run-late-approval", 1_100, &approved_command,)
            .is_err());
        let facts = db
            .kernel_recovery_facts("run-late-approval")
            .unwrap()
            .unwrap();
        assert_eq!(facts.pending_approvals.len(), 1);
    }

    #[test]
    fn scheduled_retry_due_time_survives_rehydrate() {
        use crate::kernel::{RunController, RunState};
        let path = temporary_db_path("retry-due");
        let mut config = phase3b_config();
        config.turn_max_retries = 1;
        {
            let db = Database::open(path.clone()).unwrap();
            db.kernel_create_run(
                "run-retry-due",
                "pi",
                "authoritative",
                2,
                "perm-1",
                "legacy",
                "h",
                &serde_json::to_string(&config).unwrap(),
            )
            .unwrap();
            let clock = crate::kernel::TestClock::new(0);
            let (mut controller, start) =
                RunController::start("run-retry-due", "turn-1", config, &clock).unwrap();
            db.kernel_commit_decision("run-retry-due", 10_000, &controller.persist_command(&start))
                .unwrap();
            let scheduled = controller
                .schedule_turn_retry(0, 10_000, "provider.5xx", 500)
                .unwrap();
            db.kernel_commit_decision(
                "run-retry-due",
                10_000,
                &controller.persist_command(&scheduled),
            )
            .unwrap();
        }

        let db = Database::open(path.clone()).unwrap();
        let rehydrated = db.kernel_rehydrate("run-retry-due").unwrap().unwrap();
        assert_eq!(rehydrated.state, RunState::RetryScheduled);
        assert_eq!(rehydrated.retry.scheduled_at_wall_ms, Some(10_000));
        assert_eq!(rehydrated.retry.due_wall_ms, Some(10_500));
        let mut controller = RunController::rehydrate(rehydrated).unwrap();
        assert!(controller.tick(100, 10_499).is_empty());
        let resumed = controller.tick(200, 10_500);
        assert!(resumed.iter().any(|effect| matches!(
            effect,
            crate::kernel::Effect::AppendEvent { event_type, .. }
                if event_type == "run.retry.completed"
        )));
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_recovery_drains_only_pending_cancellation_effects() {
        use crate::kernel::{plan_recovery, PolicyDecision, PolicyDecisionPort, RunController};
        struct AllowAll;
        impl PolicyDecisionPort for AllowAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::Allow
            }
        }

        let db = fresh_db();
        let config = phase3b_config();
        db.kernel_create_run(
            "run-terminal-cancel",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start("run-terminal-cancel", "turn-cancel", config, &clock).unwrap();
        db.kernel_commit_decision(
            "run-terminal-cancel",
            1,
            &controller.persist_command(&start),
        )
        .unwrap();
        let proposal = controller
            .propose_tool_batch(
                "batch-cancel",
                vec![crate::kernel::ToolCallRequest {
                    tool_call_id: "call-cancel".into(),
                    tool: "write_file".into(),
                    canonical_input_json: r#"{"path":"cancel.txt"}"#.into(),
                    source_order: 0,
                }],
                &AllowAll,
                0,
                1,
            )
            .unwrap();
        db.kernel_commit_decision(
            "run-terminal-cancel",
            1,
            &controller.persist_command(&proposal),
        )
        .unwrap();
        let cancelling = controller.request_cancel();
        db.kernel_commit_decision(
            "run-terminal-cancel",
            2,
            &controller.persist_command(&cancelling),
        )
        .unwrap();
        let cancelling_plan = plan_recovery(vec![db.kernel_recovery_facts("run-terminal-cancel").unwrap().unwrap()]);
        assert!(cancelling_plan.republish_approvals.is_empty());
        assert!(cancelling_plan.redispatch_pending.iter().all(|(_, effect)| matches!(effect.kind,
            crate::kernel::OutboxEffectKind::CancelEngineTurn | crate::kernel::OutboxEffectKind::CancelToolCall)));
        assert!(db.kernel_outbox_lease_effect("run-terminal-cancel", "dispatch:call-cancel", "must-not-run").unwrap().is_none());
        let terminal = controller.settle_cancellation();
        db.kernel_commit_decision(
            "run-terminal-cancel",
            3,
            &controller.persist_command(&terminal),
        )
        .unwrap();

        let plan = plan_recovery(vec![db
            .kernel_recovery_facts("run-terminal-cancel")
            .unwrap()
            .unwrap()]);
        assert_eq!(plan.terminal_run_ids, vec!["run-terminal-cancel"]);
        assert_eq!(plan.redispatch_pending.len(), 2);
        assert!(plan.redispatch_pending.iter().all(|(_, effect)| matches!(
            effect.kind,
            crate::kernel::OutboxEffectKind::CancelEngineTurn
                | crate::kernel::OutboxEffectKind::CancelToolCall
        )));
        let leased = db
            .kernel_outbox_lease_pending("run-terminal-cancel", "cancel-executor")
            .unwrap();
        assert_eq!(leased.len(), 2);
        assert!(leased.iter().all(|effect| matches!(
            effect.kind,
            crate::kernel::OutboxEffectKind::CancelEngineTurn
                | crate::kernel::OutboxEffectKind::CancelToolCall
        )));
    }

    #[test]
    fn rehydrate_fails_closed_on_corrupt_terminal_and_batch_facts() {
        use crate::kernel::RunController;
        let db = fresh_db();
        let config = phase3b_config();
        db.kernel_create_run(
            "run-corrupt",
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (controller, start) =
            RunController::start("run-corrupt", "turn-1", config, &clock).unwrap();
        db.kernel_commit_decision("run-corrupt", 1, &controller.persist_command(&start))
            .unwrap();
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_runs SET state='completed', terminal_written=0
                  WHERE run_id='run-corrupt'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db.kernel_rehydrate("run-corrupt").is_err());

        db.with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_runs SET state='running', terminal_written=0
                  WHERE run_id='run-corrupt'",
                [],
            )?;
            connection.execute(
                "INSERT INTO kernel_tool_batches
                    (batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
                 VALUES ('bad-batch','run-corrupt','not-json',0,1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db.kernel_rehydrate("run-corrupt").is_err());

        db.with_connection(|connection| {
            connection.execute(
                "DELETE FROM kernel_tool_batches WHERE run_id='run-corrupt'",
                [],
            )?;
            connection.execute(
                "UPDATE kernel_runs SET permission_snapshot_id='tampered'
                  WHERE run_id='run-corrupt'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db.kernel_rehydrate("run-corrupt").is_err());
        assert!(db.kernel_build_full_snapshot("run-corrupt").is_err());
        assert_eq!(db.kernel_recovery_facts("missing-run").unwrap(), None);
    }

    #[test]
    fn rehydrate_restores_full_controller_state() {
        use crate::kernel::{PolicyDecision, PolicyDecisionPort, RunController, RunState};
        // This exercises the executable control surface (controller + commit +
        // rehydrate), so it must use an authoritative (non-shadow) mode.
        let mut config = phase3b_config();
        config.kernel_mode = "authoritative".into();
        let db = fresh_db();
        let run_id = "run-rehydrate";
        db.kernel_create_run(
            run_id,
            "pi",
            "authoritative",
            2,
            "perm-1",
            "legacy",
            "h",
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let clock = crate::kernel::TestClock::new(0);
        let (mut controller, start) =
            RunController::start(run_id, "turn-9", config.clone(), &clock).unwrap();
        db.kernel_commit_decision(run_id, 0, &controller.persist_command(&start))
            .unwrap();
        struct AllowAll;
        impl PolicyDecisionPort for AllowAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
                PolicyDecision::Allow
            }
        }
        let effects = controller
            .propose_tool_batch(
                "batch-r",
                vec![
                    crate::kernel::ToolCallRequest {
                        tool_call_id: "c1".into(),
                        tool: "read".into(),
                        canonical_input_json: r#"{"path":"a"}"#.into(),
                        source_order: 0,
                    },
                    crate::kernel::ToolCallRequest {
                        tool_call_id: "c2".into(),
                        tool: "ls".into(),
                        canonical_input_json: r#"{"path":"b"}"#.into(),
                        source_order: 1,
                    },
                ],
                &AllowAll,
                0,
                0,
            )
            .unwrap();
        let cmd = controller.persist_command(&effects);
        db.kernel_commit_decision(run_id, 0, &cmd).unwrap();
        controller.tool_settled("c1", true, r#"{"ok":1}"#).unwrap();

        let rehydrated = db.kernel_rehydrate(run_id).unwrap().unwrap();
        assert_eq!(rehydrated.turn_id, "turn-9");
        assert_eq!(rehydrated.state, RunState::Running);
        assert_eq!(rehydrated.batches.len(), 1);
        assert_eq!(
            rehydrated.batches[0].ordered,
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert_eq!(rehydrated.tools.len(), 2);
        let restored = RunController::rehydrate(rehydrated).unwrap();
        assert_eq!(restored.state(), RunState::Running);
        assert_eq!(restored.config().capability_manifest_hash, "manifest-hash");
    }

    // ---- Phase 4A: production Shadow observation + zero-side-effect isolation ----

    fn shadow_frozen() -> crate::kernel::RunFrozenConfig {
        crate::kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "shadow".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "manifest-pi-v2".into(),
            permission_snapshot_id: "perm-shadow".into(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: "prompt-shadow".into(),
            model_request_timeout_ms: 120_000,
            model_first_response_ms: 60_000,
            model_idle_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 1_800_000,
            run_execution_limited: true,
            approval_wait_timeout_ms: 3_600_000,
            provider_max_retries: 2,
            turn_max_retries: 0,
            experimental_compute_job_notice: false,
        }
    }

    #[test]
    fn shadow_run_persists_and_is_never_executable_even_after_reopen_and_lease_scan() {
        let path = temporary_db_path("shadow-isolation");
        {
            let db = Database::open(path.clone()).unwrap();
            let created = db
                .kernel_create_shadow_run(
                    "shadow-legacy-1",
                    "legacy-1",
                    "conv-1",
                    "turn-1",
                    &shadow_frozen(),
                )
                .unwrap();
            assert!(created);
            // Idempotent: a second bootstrap for the same legacy run is a no-op.
            let again = db
                .kernel_create_shadow_run(
                    "shadow-legacy-1",
                    "legacy-1",
                    "conv-1",
                    "turn-1",
                    &shadow_frozen(),
                )
                .unwrap();
            assert!(!again);

            // Record a shadow diff (observation only).
            let tool = crate::kernel::ToolDisposition {
                tool_call_id: "a".into(),
                batch_id: "b".into(),
                tool: "read".into(),
                canonical_input_json: r#"{"id":"a"}"#.into(),
                source_order: 0,
                decision: "allow".into(),
            };
            let legacy = crate::kernel::LegacyDisposition {
                run_state: "running".into(),
                tools: vec![tool.clone()],
                terminal: None,
                retry_timeout: Default::default(),
            };
            let kernel_disp = crate::kernel::ShadowDisposition {
                run_state: "running".into(),
                tools: vec![tool],
                terminal: None,
                retry_timeout: Default::default(),
                comparable: true,
            };
            let diff = crate::kernel::ShadowDiff::classify(
                "shadow-legacy-1",
                "legacy-1",
                Some("turn-1".into()),
                1,
                Some("tool_batch".into()),
                legacy,
                kernel_disp,
            );
            assert_eq!(diff.category, crate::kernel::DiffCategory::Match);
            db.kernel_record_shadow_diff(&diff).unwrap();

            // CRITICAL: the shadow run has zero executable outbox rows even right
            // after recording decisions.
            assert_eq!(
                db.kernel_executable_outbox_count("shadow-legacy-1")
                    .unwrap(),
                0
            );
            // The global lease scan (startup executor) returns nothing for the
            // shadow run, because shadow effects are not in kernel_effect_outbox.
            let leased = db
                .kernel_outbox_lease_all_pending("startup-executor")
                .unwrap();
            assert!(
                leased.iter().all(|(run_id, _)| run_id != "shadow-legacy-1"),
                "global lease scan must never pick a shadow run"
            );
        }

        // Reopen across a process-lifetime boundary and re-check.
        let db = Database::open(path.clone()).unwrap();
        assert_eq!(
            db.kernel_executable_outbox_count("shadow-legacy-1")
                .unwrap(),
            0
        );
        let leased = db
            .kernel_outbox_lease_all_pending("startup-executor-2")
            .unwrap();
        assert!(leased.iter().all(|(run_id, _)| run_id != "shadow-legacy-1"));
        // Shadow observation context is recoverable, but only as comparison data.
        let ids = db.kernel_shadow_run_ids().unwrap();
        assert!(ids.contains(&"shadow-legacy-1".to_string()));
        let counts = db.kernel_shadow_diff_counts("shadow-legacy-1").unwrap();
        assert_eq!(counts.get("match"), Some(&1));
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn shadow_divergence_deterministically_classified_and_persisted() {
        let db = fresh_db();
        db.kernel_create_shadow_run(
            "shadow-l2",
            "legacy-2",
            "conv-2",
            "turn-2",
            &shadow_frozen(),
        )
        .unwrap();
        // Legacy auto-runs a tool the Kernel would require approval for.
        let legacy = crate::kernel::LegacyDisposition {
            run_state: "running".into(),
            tools: vec![crate::kernel::ToolDisposition {
                tool_call_id: "write".into(),
                batch_id: "b".into(),
                tool: "write_file".into(),
                canonical_input_json: r#"{"id":"write"}"#.into(),
                source_order: 0,
                decision: "allow".into(),
            }],
            terminal: None,
            retry_timeout: Default::default(),
        };
        let kernel_disp = crate::kernel::ShadowDisposition {
            run_state: "waiting_approval".into(),
            tools: vec![crate::kernel::ToolDisposition {
                tool_call_id: "write".into(),
                batch_id: "b".into(),
                tool: "write_file".into(),
                canonical_input_json: r#"{"id":"write"}"#.into(),
                source_order: 0,
                decision: "approval".into(),
            }],
            terminal: None,
            retry_timeout: Default::default(),
            comparable: true,
        };
        let diff = crate::kernel::ShadowDiff::classify(
            "shadow-l2",
            "legacy-2",
            Some("turn-2".into()),
            1,
            Some("tool_batch".into()),
            legacy,
            kernel_disp,
        );
        assert_eq!(diff.category, crate::kernel::DiffCategory::ApprovalMismatch);
        db.kernel_record_shadow_diff(&diff).unwrap();
        let counts = db.kernel_shadow_diff_counts("shadow-l2").unwrap();
        assert_eq!(counts.get("approval_mismatch"), Some(&1));
        assert_eq!(db.kernel_executable_outbox_count("shadow-l2").unwrap(), 0);
    }

    #[test]
    fn shadow_rejects_unknown_identity_and_wrong_legacy_binding() {
        let db = fresh_db();
        let mut bad = shadow_frozen();
        bad.kernel_mode = "authoritative".into(); // shadow table only accepts shadow
        assert!(db
            .kernel_create_shadow_run("s", "l", "c", "t", &bad)
            .is_err());

        db.kernel_create_shadow_run("shadow-l3", "legacy-3", "c", "t", &shadow_frozen())
            .unwrap();
        // A diff that binds to a different legacy run fails closed.
        let mut diff = crate::kernel::ShadowDiff {
            shadow_run_id: "shadow-l3".into(),
            legacy_run_id: "legacy-OTHER".into(),
            turn_id: None,
            event_cursor: 1,
            event_type: None,
            category: crate::kernel::DiffCategory::Match,
            legacy: Default::default(),
            kernel: crate::kernel::ShadowDisposition {
                run_state: "running".into(),
                tools: vec![],
                terminal: None,
                retry_timeout: Default::default(),
                comparable: true,
            },
            detail: serde_json::json!({}),
        };
        assert!(db.kernel_record_shadow_diff(&diff).is_err());
        diff.legacy_run_id = "legacy-3".into();
        assert!(db.kernel_record_shadow_diff(&diff).is_ok());
    }

    #[test]
    fn shadow_diff_cursor_must_be_monotonic() {
        let db = fresh_db();
        db.kernel_create_shadow_run("shadow-cur", "legacy-cur", "c", "t", &shadow_frozen())
            .unwrap();
        let make_diff =
            |cursor: i64, category: crate::kernel::DiffCategory| crate::kernel::ShadowDiff {
                shadow_run_id: "shadow-cur".into(),
                legacy_run_id: "legacy-cur".into(),
                turn_id: Some("t".into()),
                event_cursor: cursor,
                event_type: Some("tool_batch".into()),
                category,
                legacy: Default::default(),
                kernel: crate::kernel::ShadowDisposition {
                    run_state: "running".into(),
                    tools: vec![],
                    terminal: None,
                    retry_timeout: Default::default(),
                    comparable: !matches!(category, crate::kernel::DiffCategory::NotComparable),
                },
                detail: serde_json::json!({}),
            };
        // cursor 1 accepted; a replay (cursor 1 again) and out-of-order (cursor 0)
        // must fail closed rather than append.
        assert!(db
            .kernel_record_shadow_diff(&make_diff(1, crate::kernel::DiffCategory::NotComparable))
            .is_ok());
        assert!(db
            .kernel_record_shadow_diff(&make_diff(1, crate::kernel::DiffCategory::Match))
            .is_err());
        assert!(db
            .kernel_record_shadow_diff(&make_diff(0, crate::kernel::DiffCategory::Match))
            .is_err());
        assert!(db
            .kernel_record_shadow_diff(&make_diff(2, crate::kernel::DiffCategory::Match))
            .is_ok());
        let counts = db.kernel_shadow_diff_counts("shadow-cur").unwrap();
        assert_eq!(counts.values().sum::<i64>(), 2);
    }

    #[test]
    fn shadow_run_identity_conflict_fails_closed_not_silently_ignored() {
        let db = fresh_db();
        db.kernel_create_shadow_run(
            "shadow-id",
            "legacy-id",
            "conv-1",
            "turn-1",
            &shadow_frozen(),
        )
        .unwrap();
        // Same shadow_run_id but a DIFFERENT frozen config / identity must
        // fail closed (no INSERT OR IGNORE silent success).
        let mut conflicting = shadow_frozen();
        conflicting.execution_profile_id = "some-other-profile".into();
        let err =
            db.kernel_create_shadow_run("shadow-id", "legacy-id", "conv-1", "turn-1", &conflicting);
        assert!(err.is_err(), "identity conflict must fail closed");
        // Exact replay is idempotent.
        let again = db
            .kernel_create_shadow_run(
                "shadow-id",
                "legacy-id",
                "conv-1",
                "turn-1",
                &shadow_frozen(),
            )
            .unwrap();
        assert!(!again);
    }

    #[test]
    fn executable_run_table_rejects_shadow_mode() {
        let db = fresh_db();
        let frozen = shadow_frozen(); // kernel_mode = shadow
                                      // The executable control surface must reject shadow outright.
        assert!(db
            .kernel_create_run(
                "bad-shadow-run",
                "pi",
                "shadow",
                frozen.capability_manifest_version as i64,
                &frozen.permission_snapshot_id,
                &frozen.execution_profile_id,
                &frozen.prompt_config_hash,
                &serde_json::to_string(&frozen).unwrap(),
            )
            .is_err());
    }

    /// Authoritative recovery executor harness: verifies the recovery contract
    /// using the explicit test path (NOT a production default). Pending effects
    /// lease once with a stable idempotency key; leased effects are reconciled
    /// rather than blind-rerun; approvals re-publish without duplication.
    #[test]
    fn authoritative_recovery_executor_harness_pending_lease_and_leased_reconcile() {
        use crate::kernel::{plan_recovery, OutboxEffectKind};
        let path = temporary_db_path("recovery-executor");
        {
            let db = Database::open(path.clone()).unwrap();
            // Set up an AUTHORITATIVE (non-shadow) run with one approved dispatch
            // that crashed after lease, and one never-leased pending dispatch.
            let frozen = crate::kernel::RunFrozenConfig {
                kernel_mode: "authoritative".into(),
                ..shadow_frozen()
            };
            db.kernel_create_run(
                "auth-1",
                "pi",
                "authoritative",
                frozen.capability_manifest_version as i64,
                &frozen.permission_snapshot_id,
                &frozen.execution_profile_id,
                &frozen.prompt_config_hash,
                &serde_json::to_string(&frozen).unwrap(),
            )
            .unwrap();
            db.kernel_update_run_state("auth-1", "running").unwrap();
            db.kernel_update_run_state("auth-1", "waiting_approval")
                .unwrap();
            db.kernel_record_tool_batch("batch-auth", "auth-1", r#"["t1","t2"]"#)
                .unwrap();
            db.kernel_record_tool_call(
                "auth-1",
                "t1",
                "batch-auth",
                "read",
                0,
                r#"{"p":1}"#,
                "waiting_approval",
            )
            .unwrap();
            db.kernel_record_tool_call(
                "auth-1",
                "t2",
                "batch-auth",
                "read",
                1,
                r#"{"p":2}"#,
                "running",
            )
            .unwrap();
            db.kernel_record_pending_approval("auth-1", "t1").unwrap();
            // Set the approval deadline and the approval-prompt outbox row the
            // production commit path creates on entering waiting_approval.
            db.with_connection(|c| {
                c.execute(
                    "UPDATE kernel_runs SET approval_deadline_wall_ms = 10_000 WHERE run_id='auth-1'",
                    [],
                )?;
                c.execute(
                    "INSERT INTO kernel_effect_outbox
                        (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                         payload_json, status, attempts, lease_owner, leased_at, completed_at,
                         last_error, created_at, updated_at)
                     VALUES ('auth-1', ?1, 'request_approval', ?1, 't1', 'batch-auth',
                             '{}', 'pending', 0, NULL, NULL, NULL, NULL, 0, 0)",
                    params![crate::kernel::approval_effect_key("t1")],
                )?;
                Ok(())
            })
            .unwrap();
            // t1 approved -> dispatch enqueued (pending, never leased).
            db.kernel_approve_and_enqueue_dispatch("auth-1", "t1", "allow_once", 1_000)
                .unwrap()
                .expect("dispatch enqueued after approval");
            // Lease all pending once (simulates executor start); t1 dispatch leased.
            let leased = db.kernel_outbox_lease_pending("auth-1", "exec").unwrap();
            let dispatch = leased
                .iter()
                .find(|e| e.kind == OutboxEffectKind::DispatchTool)
                .expect("one dispatch leased");
            assert_eq!(
                dispatch.idempotency_key,
                crate::kernel::dispatch_idempotency_key("auth-1", "t1")
            );
            // Executor crashes WITHOUT completing; mark t2's running state.
        }
        // Reopen: recovery plan must not blind-rerun the leased t1 dispatch.
        let db = Database::open(path.clone()).unwrap();
        let facts = (0..1)
            .map(|_| db.kernel_recovery_facts("auth-1").unwrap().unwrap())
            .collect::<Vec<_>>();
        let plan = plan_recovery(facts);
        // The leased dispatch is surfaced as uncertain (reconcile), not redispatch.
        assert!(
            plan.redispatch_pending.is_empty(),
            "no safe pending redispatch while a leased effect is unresolved"
        );
        assert!(
            plan.uncertain_leased
                .iter()
                .any(|u| u.effect.kind == OutboxEffectKind::DispatchTool),
            "leased dispatch must require reconciliation"
        );
        // Approvals still pending (t1 was approved, so none pending) — verify no
        // duplicate approval is created: republish list is empty for decided tools.
        assert!(plan.republish_approvals.is_empty());
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    /// Harness: a PENDING (never-leased) dispatch is leased exactly once with a
    /// stable idempotency key; a concurrent/second lease gets nothing; after
    /// completion the effect is never re-leased. Tool result and dispatch
    /// settlement stay atomic through the commit path.
    #[test]
    fn recovery_executor_pending_dispatch_leased_once_then_completed() {
        let db = fresh_db();
        let frozen = crate::kernel::RunFrozenConfig {
            kernel_mode: "authoritative".into(),
            ..shadow_frozen()
        };
        db.kernel_create_run(
            "auth-pending",
            "pi",
            "authoritative",
            frozen.capability_manifest_version as i64,
            &frozen.permission_snapshot_id,
            &frozen.execution_profile_id,
            &frozen.prompt_config_hash,
            &serde_json::to_string(&frozen).unwrap(),
        )
        .unwrap();
        db.kernel_update_run_state("auth-pending", "running")
            .unwrap();
        db.kernel_record_tool_batch("batch-p", "auth-pending", r#"["p1"]"#)
            .unwrap();
        // Enqueue a pending dispatch effect directly (as the commit path does).
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_effect_outbox
                    (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                     payload_json, status, attempts, lease_owner, leased_at, completed_at,
                     last_error, created_at, updated_at)
                 VALUES ('auth-pending', 'dispatch:p1', 'dispatch_tool', 'tool-dispatch:p1',
                         'p1', 'batch-p', '{}', 'pending', 0, NULL, NULL, NULL, NULL, 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        // First executor leases the pending dispatch once.
        let first = db
            .kernel_outbox_lease_pending("auth-pending", "exec-a")
            .unwrap();
        assert_eq!(first.len(), 1, "exactly one pending effect leased");
        assert_eq!(first[0].idempotency_key, "tool-dispatch:p1");
        // A second lease attempt (e.g. another worker / restart race) gets none.
        let second = db
            .kernel_outbox_lease_pending("auth-pending", "exec-b")
            .unwrap();
        assert!(
            second.is_empty(),
            "a leased effect must not be leased again"
        );

        // Executor completes the effect; it is never re-leased afterward.
        db.kernel_outbox_complete("auth-pending", "dispatch:p1", "exec-a")
            .unwrap();
        let third = db
            .kernel_outbox_lease_pending("auth-pending", "exec-c")
            .unwrap();
        assert!(third.is_empty(), "a completed effect must never re-lease");
        // The global startup scan also excludes the completed/authoritative run
        // once nothing is pending, and never touches shadow rows.
        let all = db.kernel_outbox_lease_all_pending("startup").unwrap();
        assert!(all.iter().all(|(run_id, _)| run_id != "auth-pending"));
    }
}
