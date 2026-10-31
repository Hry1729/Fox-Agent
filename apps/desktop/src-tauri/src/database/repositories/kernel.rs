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

impl Database {
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

    // ---- Phase 3B: durable outbox, transactional decisions, rehydration ----

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

}

