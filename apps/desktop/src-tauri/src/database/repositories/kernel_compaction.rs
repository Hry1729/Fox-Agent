//! Compaction uses the existing event transaction and a single Run-owned intent.
//! No mutable history table, credential snapshot or separate commit window.
use super::Database;
use crate::kernel::{CompactionState, KernelPersistCommand, RunState};
use crate::kernel_compaction::{
    self as context, CompactionPlan, CompactionResult, UsageCalibrationData,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Mirror of `checkpoint_hash` in kernel_coordinator.rs (A-owned): sha256 of
/// the compact JSON form. Reconstructed views are only used for estimation,
/// never fed back into durable state.
fn checkpoint_hash_mirror(value: &Value) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.to_string().as_bytes())))
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

fn read_plan(tx: &Transaction<'_>, run_id: &str, id: &str) -> rusqlite::Result<CompactionPlan> {
    let mut query = tx.prepare("SELECT payload_json FROM kernel_events WHERE run_id=?1
        AND event_type='context.compaction.prepared' AND json_extract(payload_json,'$.id')=?2 ORDER BY seq LIMIT 2")?;
    let rows = query
        .query_map(params![run_id, id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() != 1 {
        return Err(invalid("missing or duplicate compaction input"));
    }
    let payload: Value =
        serde_json::from_str(&rows[0]).map_err(|_| invalid("invalid compaction event"))?;
    let plan: CompactionPlan = serde_json::from_value(payload["plan"].clone())
        .map_err(|_| invalid("invalid compaction plan"))?;
    plan.validate().map_err(invalid)?;
    if plan.request.run_id != run_id || plan.request.compaction_id != id {
        return Err(invalid("compaction input identity mismatch"));
    }
    Ok(plan)
}

impl Database {
    pub(crate) fn kernel_compaction_plan(
        &self,
        run_id: &str,
        id: &str,
    ) -> Result<CompactionPlan, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let plan = read_plan(&tx, run_id, id)?;
            let config = super::kernel_model_config::read_model_config(&tx, run_id)?;
            if config.hash().map_err(invalid)? != plan.config_hash {
                return Err(invalid("compaction model config changed"));
            }
            tx.commit()?;
            Ok(plan)
        })
    }

    /// Reconstruct the exact model view of each completed initial/batch model
    /// round and pair its estimated tokens with the provider usage the round
    /// reported (#11). Cached input counts at full occupancy
    /// (`input + cacheRead + cacheWrite`): a cache-read token still sits in
    /// the context window, so no billing discount is applied. Continuation
    /// rounds are skipped (their stored input is the continuation's own);
    /// rounds whose view can no longer be re-derived exactly are poisoned
    /// and skipped instead of guessed.
    pub(crate) fn kernel_usage_calibration(
        &self,
        run_id: &str,
    ) -> Result<UsageCalibrationData, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let config = super::kernel_model_config::read_model_config(&tx, run_id)?;
            // A batch-only run (stored batch resume without an initial model
            // request) has no initial input row; calibration simply starts
            // from the batch checkpoints instead of failing the dispatch.
            let initial_json: Option<String> = tx
                .query_row(
                    "SELECT input_json FROM kernel_initial_inputs WHERE run_id=?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()?;
            let mut views: HashMap<String, Vec<Value>> = HashMap::new();
            if let Some(body) = initial_json {
                let initial: Value = serde_json::from_str(&body)
                    .map_err(|_| invalid("invalid Kernel initial input"))?;
                views.insert(
                    "initial".to_string(),
                    initial["messages"].as_array().cloned().unwrap_or_default(),
                );
            }
            let mut plans: HashMap<String, CompactionPlan> = HashMap::new();
            let mut poisoned: HashSet<String> = HashSet::new();
            let mut pairs: Vec<(usize, usize)> = Vec::new();
            let mut latest_usage: Option<u64> = None;
            let rows = {
                let mut query = tx.prepare(
                    "SELECT seq,event_type,payload_json FROM kernel_events WHERE run_id=?1
                     AND event_type IN ('engine.initial_response','engine.batch_response',
                       'engine.batch_checkpoint','context.compaction.prepared','context.compaction.result')
                     ORDER BY seq",
                )?;
                let rows = query
                    .query_map(params![run_id], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            for (_seq, kind, body) in rows {
                let payload: Value = match serde_json::from_str(&body) {
                    Ok(payload) => payload,
                    Err(_) => continue,
                };
                match kind.as_str() {
                    "engine.batch_checkpoint" => {
                        let value = &payload["checkpoint"]["value"];
                        if payload["checkpoint"]["hash"].as_str()
                            != Some(checkpoint_hash_mirror(value).as_str())
                        {
                            continue;
                        }
                        if let Ok(checkpoint) =
                            serde_json::from_value::<fox_engine_protocol::KernelEngineBatchCheckpoint>(
                                value.clone(),
                            )
                        {
                            if checkpoint.validate().is_ok() {
                                views.insert(checkpoint.batch_id.clone(), checkpoint.history);
                            }
                        }
                    }
                    "context.compaction.prepared" => {
                        if let Ok(plan) =
                            serde_json::from_value::<CompactionPlan>(payload["plan"].clone())
                        {
                            if plan.validate().is_ok() && plan.request.run_id == run_id {
                                plans.insert(plan.request.compaction_id.clone(), plan);
                            }
                        }
                    }
                    "context.compaction.result" => {
                        let Some(id) = payload["id"].as_str() else { continue };
                        let Some(plan) = plans.get(id).cloned() else { continue };
                        let Ok(result) =
                            serde_json::from_value::<CompactionResult>(payload["result"].clone())
                        else {
                            continue;
                        };
                        let target = plan.target.clone();
                        if poisoned.contains(&target) {
                            continue;
                        }
                        match views.get(&target) {
                            Some(current) if *current == plan.source => {
                                match result.view(&plan) {
                                    Ok(view) => {
                                        views.insert(target, view);
                                    }
                                    Err(_) => {
                                        poisoned.insert(target);
                                    }
                                }
                            }
                            _ => {
                                poisoned.insert(target);
                            }
                        }
                    }
                    "engine.initial_response" | "engine.batch_response" => {
                        let usage = &payload["response"]["assistantMessage"]["usage"];
                        let component = |key: &str| {
                            usage
                                .get(key)
                                .and_then(Value::as_u64)
                                .filter(|value| *value <= super::MAX_RUNTIME_USAGE_COUNTER as u64)
                        };
                        let actual = match (component("input"), component("cacheRead"), component("cacheWrite")) {
                            (input, cache_read, cache_write) => input
                                .unwrap_or(0)
                                .saturating_add(cache_read.unwrap_or(0))
                                .saturating_add(cache_write.unwrap_or(0)),
                        };
                        if actual == 0 {
                            continue;
                        }
                        latest_usage = Some(actual);
                        let target = if kind == "engine.initial_response" {
                            "initial"
                        } else {
                            payload["batchId"].as_str().unwrap_or("")
                        };
                        if poisoned.contains(target) {
                            continue;
                        }
                        if let Some(view) = views.get(target) {
                            if let Ok(estimated) = context::estimate_components_tokens(&config, view) {
                                pairs.push((estimated, actual as usize));
                            }
                        }
                    }
                    _ => {}
                }
            }
            tx.commit()?;
            Ok(UsageCalibrationData {
                calibration: context::calibration_from_pairs(&pairs),
                latest_usage_input_tokens: latest_usage,
            })
        })
    }

    pub(crate) fn kernel_compaction_results(
        &self,
        run_id: &str,
        target: &str,
    ) -> Result<Vec<(CompactionPlan, CompactionResult)>, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let results=read_compaction_results_on(&tx,run_id,target)?;
            tx.commit()?;
            Ok(results)
        })
    }
}

/// Shared by the Host's model-frame builder and the model-lease verifier.
/// Both sides replay the same persisted compaction chain from its source.
pub(super) fn read_compaction_results_on(
    tx:&Transaction<'_>, run_id:&str, target:&str,
) -> rusqlite::Result<Vec<(CompactionPlan,CompactionResult)>> {
            let rows = {
                let mut query = tx.prepare("SELECT r.payload_json FROM kernel_events r
                    JOIN kernel_events p ON p.run_id=r.run_id AND p.event_type='context.compaction.prepared'
                      AND json_extract(p.payload_json,'$.id')=json_extract(r.payload_json,'$.id')
                    WHERE r.run_id=?1 AND r.event_type='context.compaction.result'
                      AND json_extract(p.payload_json,'$.plan.target')=?2 ORDER BY r.seq LIMIT 5")?;
                let rows = query.query_map(params![run_id,target], |row| row.get::<_,String>(0))?
                    .collect::<Result<Vec<_>,_>>()?;
                rows
            };
            if rows.len() > crate::kernel_compaction::MAX_PASSES { return Err(invalid("too many compaction passes")); }
            let mut results = Vec::new();
            for row in rows {
                let payload: Value = serde_json::from_str(&row).map_err(|_|invalid("invalid compaction result event"))?;
                let plan = read_plan(tx, run_id, payload["id"].as_str().ok_or_else(||invalid("missing compaction id"))?)?;
                let result: CompactionResult = serde_json::from_value(payload["result"].clone()).map_err(|_|invalid("invalid compaction result"))?;
                result.view(&plan).map_err(invalid)?;
                results.push((plan,result));
            }
            Ok(results)
}

/// Called inside the normal decision transaction BEFORE appending new events.
/// Cancellation, owner/cursor checks and output become one atomic decision.
pub(super) fn validate_decision(
    tx: &Transaction<'_>,
    run_id: &str,
    wall_ms: i64,
    previous_seq: i64,
    cmd: &KernelPersistCommand,
) -> rusqlite::Result<()> {
    let events = cmd
        .events
        .iter()
        .filter(|event| {
            event.seq > previous_seq as u64
                && matches!(
                    event.event_type.as_str(),
                    "context.compaction.prepared"
                        | "context.compaction.dispatched"
                        | "context.compaction.result"
                )
        })
        .collect::<Vec<_>>();
    let (state, body, since, timeout, elapsed): (String,Option<String>,Option<i64>,i64,i64) = tx.query_row(
        "SELECT state,compaction_state_json,model_request_since_wall_ms,json_extract(frozen_config_json,'$.model_request_timeout_ms'),running_elapsed_ms FROM kernel_runs WHERE run_id=?1",
        [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
    let old: CompactionState = body
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| invalid("invalid compaction state"))?
        .unwrap_or_default();
    if (old.pending.is_some() || old.compactions > 0) && cmd.running_elapsed_ms < elapsed {
        return Err(invalid(
            "stale compaction decision cannot refund execution time",
        ));
    }
    if events.is_empty() {
        // A rival/stale coordinator may tick without appending events. It must
        // not overwrite an already-claimed intent or restore an older deadline.
        if old != cmd.compaction
            || (old.pending.is_some() || cmd.compaction.pending.is_some())
                && (cmd.run_state == RunState::Compacting
                    && (state != "compacting" || since != cmd.model_request_since_wall_ms)
                    || !cmd.run_state.is_terminal()
                        && cmd.run_state != RunState::Cancelling
                        && cmd.run_state.as_str() != state)
        {
            return Err(invalid(
                "stale decision cannot overwrite compaction ownership",
            ));
        }
        return Ok(());
    }
    if events.len() != 1 {
        return Err(invalid(
            "compaction boundaries require separate atomic decisions",
        ));
    }
    let event = events[0];
    let payload: Value = serde_json::from_str(&event.payload_json)
        .map_err(|_| invalid("invalid compaction event"))?;
    let id = payload["id"]
        .as_str()
        .ok_or_else(|| invalid("missing compaction id"))?;
    let cancelled: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kernel_host_commands WHERE run_id=?1 AND kind='cancel')",
        [run_id],
        |row| row.get(0),
    )?;
    if cancelled {
        return Err(invalid("queued cancellation wins compaction decision"));
    }
    let config = super::kernel_model_config::read_model_config(tx, run_id)?;
    if event.event_type == "context.compaction.prepared" {
        let plan: CompactionPlan = serde_json::from_value(payload["plan"].clone())
            .map_err(|_| invalid("invalid compaction plan"))?;
        plan.validate().map_err(invalid)?;
        if state != "running"
            || since.is_some()
            || old.pending.is_some()
            || plan.request.run_id != run_id
            || plan.request.turn_id != cmd.turn_id
            || plan.request.compaction_id != id
            || plan.config_hash != config.hash().map_err(invalid)?
            || cmd.run_state != RunState::Compacting
            || cmd.model_request_since_wall_ms.is_some()
            || cmd
                .compaction
                .pending
                .as_ref()
                .is_none_or(|pending| pending.id != id || pending.owner.is_some())
        {
            return Err(invalid(
                "compaction input is not bound to an idle frozen Run",
            ));
        }
        let ready: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox WHERE run_id=?1 AND status='pending'
            AND ((?2='initial' AND effect_type='initial_model') OR (batch_id=?2 AND effect_type='deliver_tool_batch')
                OR (effect_key=?2 AND effect_type='continuation_model')))
            AND NOT EXISTS(SELECT 1 FROM kernel_tool_calls WHERE run_id=?1 AND state NOT IN ('completed','failed','cancelled'))
            AND NOT EXISTS(SELECT 1 FROM kernel_effect_outbox WHERE run_id=?1 AND status='leased')",
            params![run_id,plan.target], |row| row.get(0))?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='context.compaction.prepared'
            AND json_extract(payload_json,'$.plan.target')=?2", params![run_id,plan.target],|row|row.get(0))?;
        if !ready || count >= crate::kernel_compaction::MAX_PASSES as i64 {
            return Err(invalid(
                "compaction requires a pending delivery and bounded passes",
            ));
        }
    } else {
        let plan = read_plan(tx, run_id, id)?;
        if plan.config_hash != config.hash().map_err(invalid)?
            || state != "compacting"
            || plan.request.turn_id != cmd.turn_id
            || old.pending.as_ref().is_none_or(|pending| pending.id != id)
        {
            return Err(invalid(
                "compaction lost its immutable input or active state",
            ));
        }
        let owner = payload["owner"]
            .as_str()
            .filter(|owner| !owner.trim().is_empty())
            .ok_or_else(|| invalid("missing compaction owner"))?;
        if event.event_type == "context.compaction.dispatched" {
            if old.pending.as_ref().unwrap().owner.is_some()
                || since.is_some()
                || cmd.run_state != RunState::Compacting
                || cmd.model_request_since_wall_ms != Some(wall_ms)
                || payload["startedAt"] != wall_ms
                || cmd.compaction.pending.as_ref().is_none_or(|pending| {
                    pending.id != id || pending.owner.as_deref() != Some(owner)
                })
            {
                return Err(invalid("compaction dispatch was already claimed"));
            }
            let exhausted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM child_run_delegations d WHERE d.child_run_id=?1
                AND (d.total_tokens>=d.max_total_tokens OR d.output_tokens>=d.max_output_tokens))",
                [run_id],
                |row| row.get(0),
            )?;
            if exhausted {
                return Err(invalid("compaction child cumulative budget exhausted"));
            }
            super::validate_managed_tool_acquisition_with_clock(tx, run_id, wall_ms, false, false)?;
        } else {
            if old.pending.as_ref().unwrap().owner.as_deref() != Some(owner)
                || since.is_none()
                || since.is_some_and(|since| {
                    wall_ms < since || wall_ms.saturating_sub(since) >= timeout
                })
                || cmd.run_state != RunState::Running
                || cmd.compaction.pending.is_some()
                || cmd.model_request_since_wall_ms.is_some()
            {
                return Err(invalid("compaction result lost its owner or deadline"));
            }
            let result: CompactionResult = serde_json::from_value(payload["result"].clone())
                .map_err(|_| invalid("invalid compaction result"))?;
            result.view(&plan).map_err(invalid)?;
            if cmd.compaction.compactions
                != old.compactions.saturating_add(u32::from(result.applied))
            {
                return Err(invalid("compaction count disagrees with applied result"));
            }
        }
    }
    Ok(())
}

impl Database {
    pub(crate) fn kernel_context_budget_stopped(&self, run_id:&str, reason:&str)->Result<(),String> {
        self.with_connection(|c| {c.execute("UPDATE kernel_context_budget_reports SET report_json=json_set(report_json,'$.triggerReason',?2),updated_at=?3 WHERE run_id=?1",params![run_id,reason,crate::database::now_ms()])?;Ok(())})
    }
    pub(crate) fn kernel_store_context_budget(&self, run_id: &str, report: &context::ContextBudgetReport) -> Result<(),String> {
        let body=serde_json::to_string(report).map_err(|e| e.to_string())?;
        self.with_connection(|c| {c.execute("INSERT INTO kernel_context_budget_reports(run_id,report_json,updated_at) VALUES(?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET report_json=excluded.report_json,updated_at=excluded.updated_at",params![run_id,body,report.updated_at])?;Ok(())})
    }
    pub(crate) fn kernel_context_budget(&self, conversation: &str, run_id: &str) -> Result<Option<Value>,String> {
        self.with_connection(|c| {
            let body:Option<String>=c.query_row("SELECT b.report_json FROM kernel_context_budget_reports b JOIN runs r ON r.id=b.run_id WHERE b.run_id=?1 AND r.conversation_id=?2",params![run_id,conversation],|r|r.get(0)).optional()?;
            body.map(|v|serde_json::from_str(&v).map_err(|_|invalid("invalid budget report"))).transpose()
        })
    }
}
