//! Compaction uses the existing event transaction and a single Run-owned intent.
//! No mutable history table, credential snapshot or separate commit window.
use super::Database;
use crate::kernel::{CompactionState, KernelPersistCommand, RunState};
use crate::kernel_compaction::{CompactionPlan, CompactionResult};
use rusqlite::{params, Transaction};
use serde_json::Value;

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

    pub(crate) fn kernel_compaction_results(
        &self,
        run_id: &str,
        target: &str,
    ) -> Result<Vec<(CompactionPlan, CompactionResult)>, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
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
                let plan = read_plan(&tx, run_id, payload["id"].as_str().ok_or_else(||invalid("missing compaction id"))?)?;
                let result: CompactionResult = serde_json::from_value(payload["result"].clone()).map_err(|_|invalid("invalid compaction result"))?;
                result.view(&plan).map_err(invalid)?;
                results.push((plan,result));
            }
            tx.commit()?;
            Ok(results)
        })
    }
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
