use super::{projection_violation, run_control};
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriterRoute { Legacy, Kernel, PiNative }

pub(super) fn writer_route(connection: &Connection, run_id: &str) -> rusqlite::Result<WriterRoute> {
    let Some(binding) = run_control::binding_in_connection(connection, run_id)? else { return Ok(WriterRoute::Legacy); };
    match (binding.authority, binding.executor_route) {
        (ExecutionAuthority::Legacy, None) => Ok(WriterRoute::Legacy),
        (ExecutionAuthority::Authoritative, None) => Ok(WriterRoute::Kernel),
        (ExecutionAuthority::Authoritative, Some(ExecutorRoute::PiNative)) if binding.engine_id == "pi" => Ok(WriterRoute::PiNative),
        _ => Err(projection_violation("unsupported frozen Run executor route".into())),
    }
}

pub(super) fn require_host_writer(connection: &Connection, run_id: &str) -> rusqlite::Result<()> {
    if writer_route(connection, run_id)? == WriterRoute::Kernel {
        return Err(projection_violation("Kernel-owned read models cannot be changed by a Legacy writer".into()));
    }
    Ok(())
}

fn parse(body: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(body).map_err(|error| projection_violation(format!("invalid native tool fact: {error}")))
}

fn waiting_approval(connection: &Connection, run_id: &str) -> rusqlite::Result<bool> {
    let mut query = connection.prepare(
        "SELECT n.tool,n.canonical_input_json,t.tool_name,t.input_json
         FROM kernel_tool_calls n JOIN tool_calls t ON t.run_id=n.run_id AND t.runtime_tool_call_id=n.tool_call_id
         JOIN approvals a ON a.tool_call_id=t.id
         WHERE n.run_id=?1 AND n.result_json IS NULL AND n.state IN ('pending','waiting_approval','running')
           AND t.execution_location='host' AND t.status='pending' AND t.requires_approval=1
           AND a.category IN ('tool_execution','task_repair_budget_override') AND a.status IN ('pending','approved','denied') AND a.claimed_at IS NULL",
    )?;
    let rows = query.query_map([run_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let waiting = !rows.is_empty();
    for (tool,input,host_tool,host_input) in rows {
        if tool != host_tool || parse(&input)? != parse(&host_input)? {
            return Err(projection_violation("native approval disagrees with the registered effective input".into()));
        }
    }
    Ok(waiting)
}

impl super::Database {
    /// Whether startup retired this historical Kernel Run (A0). A retired Run
    /// is never driven again, not even by a late background-job wake.
    pub(crate) fn run_executor_route_retired(&self, run_id: &str) -> Result<bool,String> {
        self.with_connection(|connection| connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status='interrupted' AND error_code=?2)",
            params![run_id, run_control::EXECUTOR_ROUTE_RETIRED], |row| row.get(0)))
    }

    pub(crate) fn initialize_native_model_budget(&self) -> Result<(),String> {
        self.with_connection(|connection|connection.execute_batch(super::super::migrations::MIGRATION_102_NATIVE_MODEL_BUDGET))
    }

    pub(crate) fn native_run_waiting_approval(&self, run_id: &str) -> Result<bool,String> {
        self.with_connection(|connection| {
            if writer_route(connection,run_id)? != WriterRoute::PiNative { return Ok(false); }
            waiting_approval(connection,run_id)
        })
    }
}

/// A0 startup retirement of the Kernel execution path.
///
/// A historical Kernel Run (Authoritative, no frozen route) that a previous
/// process left unfinished is never resumed: the Kernel loop is frozen. It is
/// interrupted with `executor_route_retired` / 「执行路径已退役」 so the desktop
/// shows it as interrupted and the user continues in a new Run. Its Kernel
/// facts are left untouched; only the desktop read model is settled.
pub(super) fn retire_unrouted_kernel_runs(transaction: &Transaction<'_>, now: i64) -> rusqlite::Result<Vec<String>> {
    let mut query = transaction.prepare(
        "SELECT b.run_id FROM run_control_bindings b JOIN runs r ON r.id=b.run_id
         WHERE r.status IN ('queued','running','cancelling') ORDER BY b.run_id")?;
    let runs = query.query_map([], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    let mut retired = Vec::new();
    for run_id in runs {
        if writer_route(transaction, &run_id)? != WriterRoute::Kernel { continue; }
        super::kernel_projection::expire_inactive_approvals(transaction, Some(&run_id), now)?;
        transaction.execute(
            "UPDATE runs SET status='interrupted', finished_at=?2, error_code=?3, error_message=?4
             WHERE id=?1 AND status IN ('queued','running','cancelling')",
            params![run_id, now, run_control::EXECUTOR_ROUTE_RETIRED, run_control::EXECUTOR_ROUTE_RETIRED_MESSAGE])?;
        retired.push(run_id);
    }
    Ok(retired)
}

pub(super) fn protect_restart_owners(transaction: &Transaction<'_>, now: i64, retired: &[String]) -> rusqlite::Result<()> {
    let mut query = transaction.prepare("SELECT run_id FROM run_control_bindings ORDER BY run_id")?;
    let runs = query.query_map([], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS fox_restart_protected_runs(run_id TEXT PRIMARY KEY); DELETE FROM fox_restart_protected_runs;")?;
    for run_id in runs {
        let route = writer_route(transaction, &run_id)?;
        if route == WriterRoute::Kernel {
            super::kernel_projection::expire_inactive_approvals(transaction, Some(&run_id), now)?;
        }
        // A retired Kernel Run has no owner left to protect: its streaming
        // message, open tool calls and pending approvals settle with the
        // generic restart repair below.
        let retired_kernel = route == WriterRoute::Kernel && retired.contains(&run_id);
        if (route == WriterRoute::Kernel && !retired_kernel) || (route == WriterRoute::PiNative && waiting_approval(transaction, &run_id)?) {
            transaction.execute("INSERT INTO fox_restart_protected_runs(run_id) VALUES(?1)", [&run_id])?;
        }
    }
    Ok(())
}

fn project_settled_tools(transaction: &Transaction<'_>, run_id: &str, now: i64) -> rusqlite::Result<()> {
    let mut query = transaction.prepare(
        "SELECT tool_call_id,tool,canonical_input_json,state,result_json,created_at,settled_at
         FROM kernel_tool_calls WHERE run_id=?1 AND result_json IS NOT NULL ORDER BY batch_id,source_order",
    )?;
    let rows = query.query_map([run_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,
        row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,i64>(5)?,row.get::<_,Option<i64>>(6)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    for (call,tool,input,state,body,created,settled) in rows {
        let stored = parse(&body)?;
        if !stored.is_object() { return Err(projection_violation("native result is not a Host response object".into())); }
        parse(&input)?;
        let status = match state.as_str() {
            "completed" if stored["isError"] != true => "completed",
            "failed" | "not_started" if stored["isError"] == true => "failed",
            "cancelled" if stored["isError"] == true => "cancelled",
            "expired" => "interrupted",
            _ => return Err(projection_violation("native result and terminal state disagree".into())),
        };
        let result = stored.get("result").unwrap_or(&stored);
        let (result,blob) = super::split_tool_result(result);
        let result = crate::canonical_json::to_string(&result).map_err(|error| projection_violation(error.to_string()))?;
        if let Some((sha,full,bytes)) = &blob {
            transaction.execute("INSERT INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at) VALUES(?1,?2,?3,?4)
                ON CONFLICT(sha256) DO NOTHING",params![sha,full,bytes,now])?;
        }
        let blob_sha = blob.as_ref().map(|(sha,_,_)|sha.as_str());
        let error = (status != "completed").then(|| stored["error"].as_str().unwrap_or("native tool did not complete successfully").to_owned());
        transaction.execute(
            "INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,result_json,
                 error_message,execution_location,requires_approval,started_at,completed_at,updated_at,result_blob_sha256)
             SELECT ?1,?2,r.id,r.conversation_id,?3,?4,?5,?6,?7,'host',0,?8,?9,?10,?12 FROM runs r WHERE r.id=?11
             ON CONFLICT(run_id,runtime_tool_call_id) DO UPDATE SET status=excluded.status,
                 result_json=excluded.result_json,error_message=excluded.error_message,execution_location='host',
                 completed_at=excluded.completed_at,updated_at=excluded.updated_at,
                 result_blob_sha256=COALESCE(excluded.result_blob_sha256,tool_calls.result_blob_sha256)",
            params![format!("native-tool:{run_id}:{call}"),call,tool,input,status,result,error,created,settled.unwrap_or(now),now,run_id,blob_sha],
        )?;
    }
    Ok(())
}

pub(super) struct ToolProjection {
    pub handled: bool,
    pub is_error: Option<bool>,
}

pub(super) fn project_tool_lifecycle(transaction: &Transaction<'_>, run_id: &str, _event: &str,
    payload: &Value, now: i64) -> rusqlite::Result<ToolProjection> {
    let call = payload["toolCallId"].as_str().filter(|id| !id.is_empty())
        .ok_or_else(|| projection_violation("native tool event omitted its call identity".into()))?;
    let row: Option<(String,String,Option<String>)> = transaction.query_row(
        "SELECT tool,state,result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
        params![run_id,call], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    let (tool,state,result) = row.ok_or_else(|| projection_violation("native tool event has no Host registration".into()))?;
    if payload.get("tool").and_then(Value::as_str).is_some_and(|name| name != tool) {
        return Err(projection_violation("native tool event changed its registered tool".into()));
    }
    if result.is_some() { project_settled_tools(transaction, run_id, now)?; }
    // Sidecar lifecycle events cannot create or claim a Host ToolCall, including
    // the interval between batch registration and the Host execution claim.
    Ok(ToolProjection { handled:true, is_error:result.is_some().then_some(state != "completed") })
}

pub(super) fn require_completed(transaction: &Transaction<'_>, run_id: &str, now: i64) -> rusqlite::Result<()> {
    super::Database::enforce_native_model_completion(transaction,run_id)?;
    project_settled_tools(transaction, run_id, now)?;
    let blocked: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status='cancelling')
             OR EXISTS(SELECT 1 FROM kernel_tool_calls WHERE run_id=?1 AND (result_json IS NULL OR state IN ('pending','waiting_approval','running')))
             OR EXISTS(SELECT 1 FROM tool_calls WHERE run_id=?1 AND status IN ('pending','running'))
             OR EXISTS(SELECT 1 FROM task_attempts WHERE run_id=?1 AND status='running')
             OR EXISTS(SELECT 1 FROM work_tasks WHERE owner_run_id=?1 AND status NOT IN ('completed','skipped'))
             OR EXISTS(SELECT 1 FROM kernel_jobs WHERE run_id=?1 AND state NOT IN ('completed','failed','cancelled'))
             OR EXISTS(SELECT 1 FROM runs WHERE parent_run_id=?1 AND status IN ('queued','running','cancelling','awaiting_confirmation'))",
        [run_id], |row| row.get(0),
    )?;
    if blocked { return Err(projection_violation("native Run completion is blocked by unsettled Host tools or business state".into())); }
    let mut query = transaction.prepare("SELECT item_key,status,finding_json,requirements_json FROM delivery_checklist_items WHERE run_id=?1")?;
    let rows = query.query_map([run_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,
        row.get::<_,Option<String>>(2)?,row.get::<_,Option<String>>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (item,status,finding,requirements) in rows {
        let qualification = crate::runtime_host::delivery::passed_qualification_of_row(&status,finding.as_deref(),requirements.as_deref(),&item);
        if status != "passed" || matches!(qualification,
            crate::runtime_host::delivery::PassQualification::ContradictedByFacts | crate::runtime_host::delivery::PassQualification::RecordUnusable(_)) {
            return Err(projection_violation("native Run completion requires the Host delivery verdict".into()));
        }
    }
    Ok(())
}
