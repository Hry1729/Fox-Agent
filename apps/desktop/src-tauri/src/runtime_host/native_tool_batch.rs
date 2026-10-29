use std::sync::{Arc, Condvar, Mutex};
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use fox_engine_protocol::{
    HostResponse, NativeToolCallRegistration, RuntimeEnvelope, RunControlBinding,
    ToolBatchRegisterRequest, ToolBatchRegisterResponse, ToolCallCancelResponse,
    ToolCallCloseReason, ToolCallCloseRequest, ToolCallCloseResponse, ToolCallCancelRequest,
    ToolCallExecuteRequest, TOOL_BATCH_REGISTER, TOOL_CALL_EXECUTE, TOOL_CALL_CLOSE, TOOL_CALL_CANCEL, TOOL_CALL_FACTS,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest,Sha256};
use crate::database::{Database, ToolCallRecord};

pub(crate) const PARALLEL_READ_ONLY_LIMIT: usize = 4;

pub(crate) fn is_parallel_read_only_tool(tool: &str) -> bool {
    matches!(tool, "read" | "ls" | "find" | "grep" | "skill_load")
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn canonical<T: serde::Serialize>(value: &T) -> Result<String, rusqlite::Error> {
    crate::canonical_json::to_string(value).map_err(|error| invalid(error.to_string()))
}

fn failure(message: impl Into<String>) -> Value {
    json!({"isError": true, "error": message.into()})
}

fn reason_name(reason: ToolCallCloseReason) -> &'static str {
    match reason {
        ToolCallCloseReason::UnknownTool => "unknown_tool",
        ToolCallCloseReason::ValidationFailed => "validation_failed",
        ToolCallCloseReason::Intercepted => "intercepted",
        ToolCallCloseReason::Aborted => "aborted",
        ToolCallCloseReason::NotStarted => "not_started",
    }
}

#[derive(Debug)]
struct Position {
    call_id: String,
    tool: String,
    source_order: i64,
    input_digest: Option<String>,
    input: String,
    state: String,
    result: Option<String>,
    generation: i64,
    claimed_at: Option<i64>,
    close_reason: Option<String>,
}

fn positions(connection: &Connection, run: &str, batch: &str) -> rusqlite::Result<Vec<Position>> {
    let mut query = connection.prepare(
        "SELECT tool_call_id,tool,source_order,input_digest,canonical_input_json,state,
                result_json,context_generation,claimed_at,close_reason
         FROM kernel_tool_calls WHERE run_id=?1 AND batch_id=?2 ORDER BY source_order",
    )?;
    let rows = query.query_map(params![run,batch], |row| Ok(Position {
        call_id: row.get(0)?, tool: row.get(1)?, source_order: row.get(2)?,
        input_digest: row.get(3)?, input: row.get(4)?, state: row.get(5)?, result: row.get(6)?,
        generation: row.get(7)?, claimed_at: row.get(8)?, close_reason: row.get(9)?,
    }))?.collect();
    rows
}

fn position<'a>(rows: &'a [Position], call: &str) -> rusqlite::Result<&'a Position> {
    rows.iter().find(|row| row.call_id == call).ok_or_else(|| invalid("native batch/call ownership mismatch"))
}

fn is_open(row: &Position) -> bool {
    matches!(row.state.as_str(), "pending" | "waiting_approval" | "running")
}

fn may_claim(rows: &[Position], candidate: &Position) -> bool {
    if candidate.state != "pending" { return false; }
    let read = is_parallel_read_only_tool(&candidate.tool);
    if rows.iter().any(|row| row.source_order < candidate.source_order && is_open(row)
        && !(read && is_parallel_read_only_tool(&row.tool))) { return false; }
    !read || rows.iter().filter(|row| row.state == "running").count() < PARALLEL_READ_ONLY_LIMIT
}

#[derive(Debug, PartialEq)]
enum Claim {
    Execute,
    Replay(Value),
    Wait,
}

pub(crate) struct SharedBatchRegistry {
    gate: Mutex<()>,
    changed: Condvar,
    recovering_approvals: Mutex<HashSet<String>>,
}

impl SharedBatchRegistry {
    pub(crate) fn new() -> Self {
        Self { gate: Mutex::new(()), changed: Condvar::new(), recovering_approvals:Mutex::new(HashSet::new()) }
    }

    fn register(&self, database: &Database, request: &ToolBatchRegisterRequest) -> Result<ToolBatchRegisterResponse, String> {
        self.register_inner(database, request, None, false)
    }

    fn register_inner(&self, database: &Database, request: &ToolBatchRegisterRequest,
        envelope: Option<&RuntimeEnvelope>, cancelled: bool) -> Result<ToolBatchRegisterResponse, String> {
        let _guard = self.gate.lock().map_err(|_| "native batch lock poisoned")?;
        let mut calls: Vec<&NativeToolCallRegistration> = request.calls.iter().collect();
        calls.sort_by_key(|call| call.source_order);
        if request.run_id.trim().is_empty() || request.context_generation < 1 || calls.is_empty() {
            return Err("native registration needs a run, positive generation and nonempty batch".into());
        }
        let mut ids = std::collections::HashSet::new();
        for (index, call) in calls.iter().enumerate() {
            if call.source_order != index as i64 || call.call_id.trim().is_empty()
                || call.tool.trim().is_empty() || !ids.insert(&call.call_id)
                || !call.input_digest.strip_prefix("sha256:")
                    .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())) {
                return Err("native registration has invalid identity, ordering or input digest".into());
            }
        }
        database.with_connection(|connection| {
            let tx = connection.transaction()?;
            let stopped=tx.query_row("SELECT status FROM runs WHERE id=?1",[&request.run_id],|row|row.get::<_,String>(0))
                .optional()?.is_some_and(|state|matches!(state.as_str(),"cancelling" | "cancelled" | "failed" | "interrupted" | "completed"));
            let cancelled=cancelled || stopped;
            if let Some(envelope) = envelope {
                let scope = super::native_recovery::session_scope(&tx, envelope, Some(request.context_generation))?;
                for call in &calls {
                    let owned_elsewhere: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM kernel_tool_calls k JOIN runs r ON r.id=k.run_id
                         LEFT JOIN run_control_bindings b ON b.run_id=k.run_id
                         WHERE (r.conversation_id=?1 OR b.conversation_id=?1) AND k.context_generation=?2
                           AND k.tool_call_id=?3 AND k.run_id<>?4
                           AND (k.input_digest IS NOT NULL OR json_extract(b.binding_json,'$.executorRoute')='pi_native'))",
                        params![scope.conversation_id,scope.generation,call.call_id,request.run_id], |row| row.get(0),
                    )?;
                    if owned_elsewhere { return Err(invalid("native call id is already owned by an earlier Run in this session generation")); }
                }
            }
            let generation: Option<i64> = tx.query_row(
                "SELECT context_generation FROM kernel_tool_calls WHERE run_id=?1 LIMIT 1",
                [&request.run_id], |row| row.get(0),
            ).optional()?;
            if generation.is_some_and(|value| value != request.context_generation) {
                return Err(invalid("native batch context generation changed"));
            }
            let existing: Option<String> = tx.query_row(
                "SELECT batch_id FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
                params![request.run_id,calls[0].call_id], |row| row.get(0),
            ).optional()?;
            if let Some(batch_id) = existing {
                let rows = positions(&tx, &request.run_id, &batch_id)?;
                if rows.len() != calls.len() || rows.iter().zip(&calls).any(|(row, call)| {
                    row.call_id != call.call_id || row.tool != call.tool || row.source_order != call.source_order
                        || row.input_digest.as_deref() != Some(call.input_digest.as_str())
                        || row.generation != request.context_generation
                }) {
                    return Err(invalid("native batch replay changed its registered calls"));
                }
                if cancelled { close_cancelled_registration(&tx,&request.run_id,&batch_id)?; }
                tx.commit()?;
                return Ok(ToolBatchRegisterResponse { batch_id, registered_calls: calls.len() as u32 });
            }
            let batch_id = format!("native-batch-{}", uuid::Uuid::new_v4());
            let now = crate::database::now_ms();
            let ordered: Vec<&str> = calls.iter().map(|call| call.call_id.as_str()).collect();
            tx.execute("INSERT INTO kernel_tool_batches(batch_id,run_id,ordered_tool_call_ids_json,created_at)
                        VALUES(?1,?2,?3,?4)", params![batch_id,request.run_id,canonical(&ordered)?,now])?;
            for call in &calls {
                tx.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,
                            canonical_input_json,state,created_at,context_generation,input_digest)
                            VALUES(?1,?2,?3,?4,?5,'{}','pending',?6,?7,?8)",
                    params![request.run_id,call.call_id,batch_id,call.tool,call.source_order,
                        now,request.context_generation,call.input_digest])?;
            }
            if cancelled { close_cancelled_registration(&tx,&request.run_id,&batch_id)?; }
            tx.commit()?;
            Ok(ToolBatchRegisterResponse { batch_id, registered_calls: calls.len() as u32 })
        })
    }

    fn try_claim(database: &Database, request: &ToolCallExecuteRequest) -> Result<Claim, String> {
        database.with_connection(|connection| {
            let tx = connection.transaction()?;
            let rows = positions(&tx, &request.run_id, &request.batch_id)?;
            let row = position(&rows, &request.call_id)?;
            let input = canonical(&request.input)?;
            if row.tool != request.tool || (row.claimed_at.is_some() && row.input != input) {
                return Err(invalid("native execution changed the registered tool or effective input"));
            }
            if let Some(result) = &row.result {
                let result = serde_json::from_str(result).map_err(|error| invalid(error.to_string()))?;
                return Ok(Claim::Replay(result));
            }
            if row.state == "running" || row.claimed_at.is_some() {
                return Err(invalid("native call is already claimed; its unknown outcome cannot be replayed"));
            }
            if row.state != "pending" { return Err(invalid("native call is no longer executable")); }
            if !may_claim(&rows, row) { return Ok(Claim::Wait); }
            let changed = tx.execute("UPDATE kernel_tool_calls SET state='running',canonical_input_json=?4,claimed_at=?5
                                      WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='pending'",
                params![request.run_id,request.batch_id,request.call_id,input,crate::database::now_ms()])?;
            if changed != 1 { return Err(invalid("native claim lost its pending position")); }
            tx.commit()?;
            Ok(Claim::Execute)
        })
    }

    fn claim(&self, database: &Database, request: &ToolCallExecuteRequest, timeout: Duration,
        check: impl Fn() -> Result<(), String>) -> Result<Claim, String> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.gate.lock().map_err(|_| "native batch lock poisoned")?;
        loop {
            check()?;
            match Self::try_claim(database, request)? {
                Claim::Wait => {},
                outcome => return Ok(outcome),
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err("native batch admission timed out".into()); }
            // Run cancellation can originate outside this registry, so a wait is bounded even without a notification.
            guard = self.changed.wait_timeout(guard, remaining.min(Duration::from_millis(100)))
                .map_err(|_| "native batch lock poisoned")?.0;
        }
    }

    fn close(&self, database: &Database, request: &ToolCallCloseRequest) -> Result<ToolCallCloseResponse, String> {
        if !request.is_error { return Err("a never-executed native position cannot close successfully".into()); }
        let _guard = self.gate.lock().map_err(|_| "native batch lock poisoned")?;
        let result = database.with_connection(|connection| {
            let tx = connection.transaction()?;
            let rows = positions(&tx, &request.run_id, &request.batch_id)?;
            let row = position(&rows, &request.call_id)?;
            if row.state=="not_started" && row.claimed_at.is_none()
                && row.close_reason.as_deref()==Some(reason_name(request.reason)) && row.result.is_some() {
                return Ok(ToolCallCloseResponse {closed:true,state:row.state.clone()});
            }
            if row.state != "pending" || row.claimed_at.is_some() {
                return Ok(ToolCallCloseResponse { closed: false, state: row.state.clone() });
            }
            let reason = reason_name(request.reason);
            let body = canonical(&failure(format!("tool was not executed: {reason}")))?;
            tx.execute("UPDATE kernel_tool_calls SET state='not_started',close_reason=?4,result_json=?5,settled_at=?6
                        WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='pending' AND claimed_at IS NULL",
                params![request.run_id,request.batch_id,request.call_id,reason,body,crate::database::now_ms()])?;
            tx.commit()?;
            Ok(ToolCallCloseResponse { closed: true, state: "not_started".into() })
        });
        self.changed.notify_all();
        result
    }

    fn refusal_receipt(&self, database: &Database, envelope: &RuntimeEnvelope,
        request: &ToolCallExecuteRequest) -> Result<Value,String> {
        let _guard=self.gate.lock().map_err(|_|"native batch lock poisoned")?;
        let result=database.with_connection(|connection| {
            let tx=connection.transaction()?;
            let scope=super::native_recovery::session_scope(&tx,envelope,None)?;
            if request.run_id!=envelope.run_id.as_deref().unwrap_or_default() {
                return Err(invalid("native refusal does not own its envelope"));
            }
            let rows=positions(&tx,&request.run_id,&request.batch_id)?;let row=position(&rows,&request.call_id)?;
            if row.generation!=scope.generation || row.tool!=request.tool
                || row.claimed_at.is_some() && row.input!=canonical(&request.input)? {
                return Err(invalid("native refusal changed its call identity, generation or claimed input"));
            }
            if let Some(stored)=&row.result {
                let receipt:Value=serde_json::from_str(stored).map_err(|error|invalid(error.to_string()))?;
                if !receipt.is_object() || !matches!(row.state.as_str(),"completed" | "failed" | "cancelled" | "not_started") {
                    return Err(invalid("native refusal found an invalid committed receipt"));
                }
                return Ok(receipt);
            }
            if row.state!="pending" || row.claimed_at.is_some() {
                return Err(invalid("native refusal cannot settle an unknown execution outcome"));
            }
            let response=failure("tool was not executed: intercepted");
            let changed=tx.execute("UPDATE kernel_tool_calls SET state='not_started',close_reason='intercepted',result_json=?4,settled_at=?5
                                   WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='pending' AND claimed_at IS NULL AND result_json IS NULL",
                params![request.run_id,request.batch_id,request.call_id,canonical(&response)?,crate::database::now_ms()])?;
            if changed!=1 { return Err(invalid("native refusal lost its unclaimed position")); }
            tx.commit()?;
            Ok(response)
        });
        self.changed.notify_all();
        result
    }

    fn cancel(&self, database: &Database, request: &ToolCallCancelRequest) -> Result<ToolCallCancelResponse, String> {
        let _guard = self.gate.lock().map_err(|_| "native batch lock poisoned")?;
        let result = database.with_connection(|connection| {
            let tx = connection.transaction()?;
            let rows = positions(&tx, &request.run_id, &request.batch_id)?;
            let row = position(&rows, &request.call_id)?;
            let outcome_unknown = row.claimed_at.is_some() && row.result.is_none();
            let result_recorded = row.result.is_some();
            if row.state == "pending" && row.claimed_at.is_none() {
                tx.execute("UPDATE kernel_tool_calls SET state='cancelled',close_reason='aborted',result_json=?4,settled_at=?5
                            WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='pending' AND claimed_at IS NULL",
                    params![request.run_id,request.batch_id,request.call_id,
                        canonical(&failure("tool cancelled before execution"))?,crate::database::now_ms()])?;
            } else if matches!(row.state.as_str(),"pending" | "waiting_approval" | "running") {
                // An in-flight writer remains a barrier until it actually returns; a cancel acknowledgement is not completion.
                tx.execute("UPDATE kernel_tool_calls SET close_reason='aborted'
                            WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND result_json IS NULL",
                    params![request.run_id,request.batch_id,request.call_id])?;
            }
            tx.commit()?;
            Ok(ToolCallCancelResponse { cancelled: true, result_recorded, outcome_unknown })
        });
        self.changed.notify_all();
        result
    }
}

fn close_cancelled_registration(connection: &Connection, run: &str, batch: &str) -> rusqlite::Result<()> {
    connection.execute("UPDATE kernel_tool_calls SET state='not_started',close_reason='aborted',result_json=?3,settled_at=?4
                        WHERE run_id=?1 AND batch_id=?2 AND state='pending' AND claimed_at IS NULL AND result_json IS NULL",
        params![run,batch,canonical(&failure("tool was not executed: aborted"))?,crate::database::now_ms()])?;
    Ok(())
}

#[derive(Clone)]
struct ApprovalRecoveryContext {
    envelope: RuntimeEnvelope,
    request: ToolCallExecuteRequest,
    approval_id: String,
    category: String,
}

thread_local! {
    static APPROVAL_RECOVERY: RefCell<Option<ApprovalRecoveryContext>> = const { RefCell::new(None) };
}

struct ApprovalRecoveryGuard;

impl ApprovalRecoveryGuard {
    fn enter(context: ApprovalRecoveryContext) -> Result<Self, String> {
        APPROVAL_RECOVERY.with(|active| {
            let mut active = active.borrow_mut();
            if active.is_some() { return Err("native approval recovery cannot be nested".into()); }
            *active = Some(context);
            Ok(Self)
        })
    }
}

impl Drop for ApprovalRecoveryGuard {
    fn drop(&mut self) { APPROVAL_RECOVERY.with(|active| { active.borrow_mut().take(); }); }
}

struct ApprovalRecoveryLease<'a> { registry: &'a SharedBatchRegistry, approval_id: String }

impl Drop for ApprovalRecoveryLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.recovering_approvals.lock() { active.remove(&self.approval_id); }
    }
}

fn validate_approval_recovery(database: &Database, context: &ApprovalRecoveryContext) -> Result<ToolCallRecord, String> {
    let repair=context.category=="task_repair_budget_override";
    if !matches!(context.category.as_str(),"tool_execution" | "task_repair_budget_override")
        || repair && context.request.tool!="task_repair_escalate_start" {
        return Err("native approval recovery has an unsupported tool/category binding".into());
    }
    database.with_connection(|connection| {
        let scope = super::native_recovery::session_scope(connection, &context.envelope, None)?;
        let rows = positions(connection, &context.request.run_id, &context.request.batch_id)?;
        let row = position(&rows, &context.request.call_id)?;
        if row.generation != scope.generation || row.tool != context.request.tool
            || row.input != canonical(&context.request.input)? || row.result.is_some()
            || row.state != "running" || row.claimed_at.is_none() || row.close_reason.is_some() {
            return Err(invalid("native approval recovery has no unchanged pending execution intent"));
        }
        let eligible: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
             JOIN runs r ON r.id=t.run_id WHERE a.id=?1 AND t.run_id=?2 AND t.runtime_tool_call_id=?3
               AND a.status='approved' AND a.claimed_at IS NULL AND a.category=?7
               AND json_extract(a.decision_json,'$.approved')=1
               AND json_extract(a.decision_json,'$.category')=?7
               AND (json_extract(a.decision_json,'$.scope')='once'
                    OR (?7='tool_execution' AND json_extract(a.decision_json,'$.scope')='conversation'))
               AND t.status='pending' AND t.requires_approval=1 AND t.execution_location='host'
               AND t.tool_name=?4 AND t.input_json=?5 AND t.conversation_id=?6 AND r.status='running')",
            params![context.approval_id,context.request.run_id,context.request.call_id,context.request.tool,
                row.input,scope.conversation_id,context.category], |query| query.get(0),
        )?;
        if !eligible { return Err(invalid("native approval recovery cannot reuse a claimed or invalid approval")); }
        Ok(())
    })?;
    let mode = database.conversation_permission_mode(context.envelope.conversation_id.as_deref().unwrap_or_default())?;
    if mode == "read_only" && !is_parallel_read_only_tool(&context.request.tool) {
        return Err("native approval recovery is blocked by the current read-only permission".into());
    }
    database.get_runtime_tool_call(&context.request.run_id, &context.request.call_id)?
        .ok_or_else(|| "native approval recovery lost its Host ToolCall".into())
}

pub(super) fn recovering_host_tool_call(database: &Database, run_id: &str, call_id: &str,
    tool: &str, input: &Value, claim_if_ready: bool) -> Result<Option<ToolCallRecord>, String> {
    let context = APPROVAL_RECOVERY.with(|active| active.borrow().clone());
    let Some(context) = context else { return Ok(None); };
    if context.request.run_id != run_id || context.request.call_id != call_id
        || context.request.tool != tool || context.request.input != *input {
        return Err("native approval recovery cannot change its call identity or effective input".into());
    }
    let mut record = validate_approval_recovery(database, &context)?;
    if claim_if_ready {
        if context.category!="tool_execution" {
            return Err("native repair recovery must use its dedicated budget override claim".into());
        }
        if !database.claim_approved_tool_call(run_id, call_id)? {
            return Err("native approval recovery lost its approved execution claim".into());
        }
        record = database.get_runtime_tool_call(run_id, call_id)?.ok_or("native approval recovery lost its ToolCall after claim")?;
    }
    Ok(Some(record))
}

pub(super) fn recovering_approval_decision(database: &Database, run_id: &str,
    approval_id: &str) -> Result<Option<bool>, String> {
    let context = APPROVAL_RECOVERY.with(|active| active.borrow().clone());
    let Some(context) = context else { return Ok(None); };
    if context.request.run_id != run_id || context.approval_id != approval_id {
        return Err("native approval recovery cannot consume another approval".into());
    }
    validate_approval_recovery(database, &context)?;
    Ok(Some(true))
}

pub(super) fn cancelled_native_approval(database: &Database, run_id: &str,
    approval_id: &str) -> Result<bool,String> {
    let Some(binding)=database.run_control_binding(run_id)? else { return Ok(false); };
    if !matches!(super::kernel_authority::resolve_run_route(Some(&binding)),super::kernel_authority::RunRoute::PiNative) { return Ok(false); }
    database.with_connection(|connection| {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
             JOIN runs r ON r.id=t.run_id JOIN kernel_tool_calls k ON k.run_id=t.run_id AND k.tool_call_id=t.runtime_tool_call_id
             WHERE a.id=?1 AND t.run_id=?2 AND (k.close_reason='aborted' OR r.status IN ('cancelling','cancelled','interrupted','failed')))",
            params![approval_id,run_id],|row|row.get(0),
        )
    })
}

pub(super) fn recover_native_approval(host: &super::RuntimeHost,
    stdin: &Arc<Mutex<std::process::ChildStdin>>, approval_id: &str) -> Result<bool, String> {
    let pending = host.state.lock().map_err(|_| "runtime state poisoned")?.pending_approvals.clone();
    if pending.lock().map_err(|_| "approval map poisoned")?.contains_key(approval_id) {
        return Err("native approval recovery refuses an approval with a live waiter".into());
    }
    {
        let mut active = host.native_batches.recovering_approvals.lock().map_err(|_| "native approval recovery lock poisoned")?;
        if !active.insert(approval_id.to_owned()) { return Err("native approval recovery is already in flight".into()); }
    }
    let _lease = ApprovalRecoveryLease { registry:&host.native_batches,approval_id:approval_id.into() };
    let intent: Option<(String,String,String,String,String,i64,String,String,Option<i64>,String)> = host.database.with_connection(|connection| {
        connection.query_row(
            "SELECT t.run_id,t.runtime_tool_call_id,k.batch_id,k.tool,k.canonical_input_json,
                    k.context_generation,t.conversation_id,a.status,a.claimed_at,a.category
             FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
             JOIN kernel_tool_calls k ON k.run_id=t.run_id AND k.tool_call_id=t.runtime_tool_call_id
             JOIN run_control_bindings b ON b.run_id=t.run_id
             WHERE a.id=?1 AND json_extract(b.binding_json,'$.executorRoute')='pi_native'",
            [approval_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,
                row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
        ).optional()
    })?;
    let Some((run_id,call_id,batch_id,tool,input,generation,conversation_id,status,claimed,category)) = intent else { return Ok(false); };
    if claimed.is_some() || !matches!(category.as_str(),"tool_execution" | "task_repair_budget_override")
        || category=="task_repair_budget_override" && tool!="task_repair_escalate_start"
        || !matches!(status.as_str(), "approved" | "denied") {
        return Err("native approval recovery requires an unclaimed explicit approval decision".into());
    }
    let session = host.database.get_runtime_session(&conversation_id)?.ok_or("native approval recovery has no current session")?;
    let request = ToolCallExecuteRequest { run_id:run_id.clone(),batch_id,call_id,tool,
        input:serde_json::from_str(&input).map_err(|error| format!("native approval recovery has corrupt input: {error}"))? };
    let mut envelope = RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
        kind:"request".into(),id:format!("native-approval-recovery-{}",uuid::Uuid::new_v4()),r#type:TOOL_CALL_EXECUTE.into(),request_id:None,
        conversation_id:Some(conversation_id),runtime_session_id:Some(session.runtime_session_id),run_id:Some(run_id),
        seq:None,timestamp:String::new(),payload:Some(serde_json::to_value(&request).map_err(|error| error.to_string())?) };
    let _recovery_scope=host.prepare_native_approval_recovery(&request.run_id)?;
    let host=_recovery_scope.host();
    host.database.with_connection(|connection| {
        let scope = super::native_recovery::session_scope(connection, &envelope, Some(generation))?;
        let rows = positions(connection, &request.run_id, &request.batch_id)?;
        let row = position(&rows, &request.call_id)?;
        if row.generation != scope.generation || row.tool != request.tool || row.input != canonical(&request.input)?
            || row.result.is_some() || row.state != "running" || row.claimed_at.is_none() || row.close_reason.is_some() {
            return Err(invalid("native approval recovery cannot reopen a settled, cancelled or unknown actual execution"));
        }
        let pending: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2
             AND status='pending' AND requires_approval=1 AND execution_location='host' AND tool_name=?3 AND input_json=?4)",
            params![request.run_id,request.call_id,request.tool,row.input], |query| query.get(0),
        )?;
        if !pending { return Err(invalid("native approval recovery cannot reopen a claimed Host ToolCall")); }
        Ok(())
    })?;
    if status == "denied" {
        let error = "The user denied this operation after runtime restart.";
        host.database.complete_host_tool_call(&request.run_id, &request.call_id, None, Some(error))?;
        record_result(&host.database, &request, &failure(error))?;
    } else {
        validate_tool(host, &envelope, &request)?;
        let _permit = host.try_acquire_tool_handler().ok_or("runtime.tool_handler_capacity_exceeded")?;
        let context = ApprovalRecoveryContext { envelope:envelope.clone(),request:request.clone(),approval_id:approval_id.into(),category };
        validate_approval_recovery(&host.database, &context)?;
        let _recovery = ApprovalRecoveryGuard::enter(context)?;
        envelope.payload.as_mut().unwrap()["toolCallId"] = json!(request.call_id);
        super::dispatch_tool_request_envelope(host, &host.app, &host.database, &host.attachments_dir,
            &host.sessions_dir, &host.state, stdin, &host.yuxi_client, envelope.clone());
        host.database.with_connection(|connection| {
            let rows = positions(connection, &request.run_id, &request.batch_id)?;
            if position(&rows, &request.call_id)?.result.is_none() {
                return Err(invalid("native approval recovery returned without a committed execution result"));
            }
            Ok(())
        })?;
    }
    let run_state:String=host.database.with_connection(|connection|connection.query_row(
        "SELECT status FROM runs WHERE id=?1",[&request.run_id],|row|row.get(0)))?;
    if run_state=="running" && !host.database.native_run_waiting_approval(&request.run_id)? {
        host.database.mark_run_interrupted(&request.run_id,"runtime.native_approval_recovered",
            "The original approval was settled without resuming its model loop; reopen the native session to reconcile its result.")?;
    }
    host.native_batches.changed.notify_all();
    Ok(true)
}

pub(super) fn record_result(database: &Database, request: &ToolCallExecuteRequest, response: &Value) -> Result<Value, String> {
    database.with_connection(|connection| {
        let tx = connection.transaction()?;
        let rows = positions(&tx, &request.run_id, &request.batch_id)?;
        let row = position(&rows, &request.call_id)?;
        if row.tool != request.tool || row.input != canonical(&request.input)? {
            return Err(invalid("native result does not match the claimed effective input"));
        }
        let run_cancelled = if row.result.is_none() {
            tx.query_row("SELECT status FROM runs WHERE id=?1",[&request.run_id],|query|query.get::<_,String>(0))
                .optional()?.is_some_and(|state| matches!(state.as_str(),"cancelled" | "cancelling"))
        } else { false };
        let cancelled = row.close_reason.as_deref() == Some("aborted") || run_cancelled;
        let mut stored = response.clone();
        if !stored.is_object() || (stored["isError"] != true && stored.get("result").is_none()) {
            return Err(invalid("native result must be an execute response object"));
        }
        if cancelled {
            stored["isError"] = json!(true);
            stored["error"] = json!("tool cancelled; retained output is evidence, not a successful result");
        }
        if let Some(existing) = &row.result {
            let existing: Value = serde_json::from_str(existing).map_err(|error| invalid(error.to_string()))?;
            if existing != stored { return Err(invalid("native result is immutable")); }
            return Ok(existing);
        }
        if row.state != "running" || row.claimed_at.is_none() { return Err(invalid("native result has no execution claim")); }
        let state = if cancelled { "cancelled" } else if stored["isError"] == true { "failed" } else { "completed" };
        let changed = tx.execute("UPDATE kernel_tool_calls SET state=?4,result_json=?5,settled_at=?6,
                                  close_reason=CASE WHEN ?7 THEN 'aborted' ELSE close_reason END
                    WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='running'",
            params![request.run_id,request.batch_id,request.call_id,state,canonical(&stored)?,crate::database::now_ms(),cancelled])?;
        if changed != 1 { return Err(invalid("native result lost its execution claim")); }
        tx.commit()?;
        Ok(stored)
    })
}

pub(super) fn retain_late_native_result(database: &Database, run: &str, call: &str,
    tool: &str, input: &Value, result: &Value, persist_error: &str) -> Result<Option<Value>,String> {
    let Some(binding)=database.run_control_binding(run)? else { return Ok(None); };
    if !matches!(super::kernel_authority::resolve_run_route(Some(&binding)),super::kernel_authority::RunRoute::PiNative) { return Ok(None); }
    let full=canonical(result).map_err(|error|error.to_string())?;
    let hash=format!("sha256:{}",hex::encode(Sha256::digest(full.as_bytes())));
    let inline=if full.len()<=crate::database::MAX_STORED_TOOL_RESULT_BYTES {result.clone()} else {
        json!({"truncated":true,"originalBytes":full.len(),"content":[{"type":"text","text":full.chars().take(8_000).collect::<String>()}],
            "message":"Host retained the full late output as evidence. It has no verified read_tool_result mapping."})
    };
    database.with_connection(|connection| {
        let tx=connection.transaction()?;
        let (frozen,_)=super::native_recovery::native_binding(&tx,run)?;
        if frozen!=binding { return Err(invalid("native late evidence binding changed")); }
        let (status,conversation):(String,String)=tx.query_row("SELECT status,conversation_id FROM runs WHERE id=?1",[run],|row|Ok((row.get(0)?,row.get(1)?)))?;
        if !matches!(status.as_str(),"completed" | "failed" | "cancelled" | "interrupted") { return Ok(None); }
        if conversation!=binding.conversation_id { return Err(invalid("native late evidence belongs to another conversation")); }
        let batch:Option<String>=tx.query_row("SELECT batch_id FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",params![run,call],|row|row.get(0)).optional()?;
        let Some(batch)=batch else { return Err(invalid("native late evidence has no registered call")); };
        let rows=positions(&tx,run,&batch)?;let row=position(&rows,call)?;
        if row.tool!=tool || row.input!=canonical(input)? || row.claimed_at.is_none() {
            return Err(invalid("native late evidence does not match its original execution claim"));
        }
        let host:Option<(String,String,String,String)>=tx.query_row(
            "SELECT tool_name,input_json,execution_location,conversation_id FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",
            params![run,call],|query|Ok((query.get(0)?,query.get(1)?,query.get(2)?,query.get(3)?)),
        ).optional()?;
        if !host.is_some_and(|(name,arguments,location,owner)|name==tool && arguments==row.input && location=="host" && owner==conversation) {
            return Err(invalid("native late evidence has no matching Host execution intent"));
        }
        if let Some(existing)=&row.result {
            let stored:Value=serde_json::from_str(existing).map_err(|error|invalid(error.to_string()))?;
            let same=stored.pointer("/nativeLateEvidence/sha256").and_then(Value::as_str)==Some(hash.as_str())
                || stored.pointer("/toolResultStorage/blobSha256").and_then(Value::as_str)==Some(hash.as_str())
                || stored.get("result").is_some_and(|value|value==result);
            if !same { return Err(invalid("native late result is immutable; retained output changed")); }
            return Ok(Some(stored));
        }
        if row.state!="running" { return Err(invalid("native late evidence has no live execution claim")); }
        tx.execute("INSERT INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at) VALUES(?1,?2,?3,?4) ON CONFLICT(sha256) DO NOTHING",
            params![hash,full,full.len() as i64,crate::database::now_ms()])?;
        let stored_blob:String=tx.query_row("SELECT body_json FROM tool_call_result_blobs WHERE sha256=?1",[&hash],|query|query.get(0))?;
        if stored_blob!=full { return Err(invalid("native late evidence blob hash disagrees with its retained body")); }
        let cancelled=row.close_reason.as_deref()==Some("aborted") || status=="cancelled";
        let response=json!({"isError":true,"error":if cancelled {
                "tool cancelled; retained output is evidence, not a successful result"
            } else {"Tool output arrived after its Run stopped. Host retained it as evidence; do not repeat the operation automatically."},
            "errorDetails":{"code":"runtime.native_late_result","retryable":false},"result":inline,
            "toolResultStorage":{"stored":false,"storedBytes":full.len(),"retrievableBytes":0,"blobSha256":hash,
                "failureReason":"native_late_evidence_has_no_read_tool_result_mapping"},
            "nativeLateEvidence":{"sha256":hash,"byteSize":full.len(),"retrievable":false,
                "persistFailure":persist_error.chars().take(2_000).collect::<String>()}});
        let changed=tx.execute("UPDATE kernel_tool_calls SET state=?3,result_json=?4,settled_at=?5,
                                close_reason=CASE WHEN ?6 THEN 'aborted' ELSE close_reason END
                                WHERE run_id=?1 AND tool_call_id=?2 AND state='running' AND result_json IS NULL AND claimed_at IS NOT NULL",
            params![run,call,if cancelled {"cancelled"} else {"failed"},canonical(&response)?,crate::database::now_ms(),cancelled])?;
        if changed!=1 { return Err(invalid("native late evidence lost its execution claim")); }
        tx.commit()?;
        Ok(Some(response))
    })
}

pub(super) fn prepare_response(database: &Database, envelope: &RuntimeEnvelope, response_type: &str,
    response: Value) -> Result<HostResponse, String> {
    if envelope.r#type != TOOL_CALL_EXECUTE {
        return Ok(HostResponse::for_request(envelope, response_type, response));
    }
    let mut payload = envelope.payload.clone().ok_or("native execution omitted its payload")?;
    payload.as_object_mut().ok_or("native execution payload is not an object")?.remove("toolCallId");
    let request: ToolCallExecuteRequest = serde_json::from_value(payload).map_err(|error| error.to_string())?;
    let body = record_result(database, &request, &response)?;
    Ok(HostResponse::for_request(envelope,
        if body["isError"] == true { "tool.execute_failed" } else { "tool.execute_completed" }, body))
}

pub(super) fn has_committed_response(database: &Database, envelope: &RuntimeEnvelope) -> Result<bool,String> {
    if envelope.r#type != TOOL_CALL_EXECUTE { return Ok(false); }
    let mut payload=envelope.payload.clone().ok_or("native response omitted its execution identity")?;
    payload.as_object_mut().ok_or("native response execution identity is not an object")?.remove("toolCallId");
    let request:ToolCallExecuteRequest=serde_json::from_value(payload).map_err(|error|error.to_string())?;
    let binding=database.run_control_binding(&request.run_id)?.ok_or("native response lost its frozen binding")?;
    validate_binding(envelope,&binding)?;
    database.with_connection(|connection| {
        let rows=positions(connection,&request.run_id,&request.batch_id)?;
        let row=position(&rows,&request.call_id)?;
        if row.tool != request.tool || row.input != canonical(&request.input)? {
            return Err(invalid("native response does not match its committed execution identity"));
        }
        Ok(row.result.is_some() && matches!(row.state.as_str(),"completed" | "failed" | "cancelled"))
    })
}

pub(super) fn settle_disconnected_native_response(database: &Database,
    envelope: &RuntimeEnvelope) -> Result<bool,String> {
    if !has_committed_response(database,envelope)? { return Ok(false); }
    let run=envelope.run_id.as_deref().ok_or("native response omitted Run identity")?;
    let can_interrupt=database.with_connection(|connection| {
        let (state,unfinished,approvals):(String,i64,i64)=connection.query_row(
            "SELECT status,
                    (SELECT COUNT(*) FROM kernel_tool_calls k WHERE k.run_id=r.id AND k.result_json IS NULL
                       AND (k.claimed_at IS NOT NULL OR k.state IN ('running','waiting_approval'))),
                    (SELECT COUNT(*) FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE t.run_id=r.id
                       AND t.status='pending' AND a.status IN ('pending','approved') AND a.claimed_at IS NULL)
             FROM runs r WHERE id=?1",[run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        Ok(state=="running" && unfinished==0 && approvals==0)
    })?;
    if can_interrupt {
        database.mark_run_interrupted(run,"runtime.native_transport_lost",
            "Native tool results were committed after the runtime disconnected; reopen the session to reconcile them.")?;
    }
    Ok(true)
}

pub(super) fn close_unclaimed_after_transport_loss(database: &Database, run: &str) -> Result<(),String> {
    let binding=database.run_control_binding(run)?.ok_or("native transport loss has no frozen Run")?;
    if !matches!(super::kernel_authority::resolve_run_route(Some(&binding)),super::kernel_authority::RunRoute::PiNative) {
        return Err("native transport loss cannot close another executor's positions".into());
    }
    database.with_connection(|connection| {
        connection.execute("UPDATE kernel_tool_calls SET state='not_started',close_reason='not_started',result_json=?2,settled_at=?3
                            WHERE run_id=?1 AND state='pending' AND claimed_at IS NULL AND result_json IS NULL",
            params![run,canonical(&failure("Tool was not executed before its native transport disconnected."))?,crate::database::now_ms()])?;
        Ok(())
    })
}

fn validate_binding(envelope: &RuntimeEnvelope, binding: &RunControlBinding) -> Result<(), String> {
    let payload = envelope.payload.as_ref().ok_or("native request omitted payload")?;
    if envelope.run_id.as_deref() != Some(binding.run_id.as_str())
        || payload["runId"].as_str() != Some(binding.run_id.as_str())
        || envelope.conversation_id.as_deref() != Some(binding.conversation_id.as_str())
        || !matches!(super::kernel_authority::resolve_run_route(Some(binding)), super::kernel_authority::RunRoute::PiNative) {
        return Err("native request does not match its frozen Run identity and route".into());
    }
    Ok(())
}

fn validate_request(host: &super::RuntimeHost, envelope: &RuntimeEnvelope) -> Result<RunControlBinding, String> {
    host.ensure_active_envelope_identity(envelope).map_err(|(code, error)| format!("{code}: {error}"))?;
    let run = envelope.run_id.as_deref().ok_or("native request omitted runId")?;
    let binding = host.database.run_control_binding(run)?.ok_or("native request has no frozen binding")?;
    validate_binding(envelope, &binding)?;
    let session = host.database.get_runtime_session(&binding.conversation_id)?.ok_or("native session is absent")?;
    if !session.is_native() || envelope.runtime_session_id.as_deref() != Some(session.runtime_session_id.as_str()) {
        return Err("native request belongs to a retired session".into());
    }
    if let Some(batch_id) = envelope.payload.as_ref().and_then(|payload| payload["batchId"].as_str()) {
        host.database.with_connection(|connection| {
            let rows = positions(connection, run, batch_id)?;
            if rows.is_empty() || rows.iter().any(|row| row.generation != session.context_generation) {
                return Err(invalid("native batch belongs to a different context generation"));
            }
            Ok(())
        })?;
    }
    Ok(binding)
}

fn validate_tool(host: &super::RuntimeHost, envelope: &RuntimeEnvelope, request: &ToolCallExecuteRequest) -> Result<(), String> {
    let binding=validate_request(host, envelope)?;
    validate_execution_identity(&host.database,request)?;
    validate_tool_policy(host,envelope,request,&binding)
}

fn validate_execution_identity(database: &Database, request: &ToolCallExecuteRequest) -> Result<(),String> {
    database.with_connection(|connection| {
        let rows=positions(connection,&request.run_id,&request.batch_id)?;
        let row=position(&rows,&request.call_id)?;
        if row.tool!=request.tool || row.claimed_at.is_some() && row.input!=canonical(&request.input)? {
            return Err(invalid("native execution changed its registered identity or input"));
        }
        Ok(())
    })
}

fn validate_tool_policy(host: &super::RuntimeHost, envelope: &RuntimeEnvelope,
    request: &ToolCallExecuteRequest, binding: &RunControlBinding) -> Result<(),String> {
    let elapsed=super::native_model_budget::elapsed_ms(&host.database,&request.run_id)?;
    if binding.budgets.limit_operation_ms(binding.budgets.tool_execution_ms,elapsed)<=0 {
        return Err("runtime.native_tool_budget_exhausted".into());
    }
    let ingress = if crate::resource_gateway::is_reader(&request.tool) {
        super::RuntimeToolIngress::Preflight
    } else { super::RuntimeToolIngress::Execute };
    host.ensure_runtime_tool_allowed(&request.run_id, &request.tool, ingress)
        .map_err(|(code, error)| format!("{code}: {error}"))?;
    host.ensure_delegated_tool_allowed(&request.tool)?;
    super::ensure_expert_tool_allowed(&host.database, envelope.conversation_id.as_deref().unwrap_or_default(), &request.tool)?;
    host.state.lock().map_err(|_| "runtime state poisoned")?
        .cancellation.tool_token(&request.run_id, &request.call_id)?.check()?;
    Ok(())
}

fn execution_refusal(host: &super::RuntimeHost, envelope: &RuntimeEnvelope,
    request: &ToolCallExecuteRequest, diagnostic: &str) -> Option<Value> {
    eprintln!("[native.execution_refused] run={} call={} error={diagnostic}",request.run_id,request.call_id);
    match host.native_batches.refusal_receipt(&host.database,envelope,request) {
        Ok(receipt)=>Some(receipt),
        Err(error)=>{
            host.reject_runtime_run_event(&request.run_id,"runtime.native_refusal_uncommitted",&error);
            None
        }
    }
}

fn execute_native_reader(database: &Database, binding: &RunControlBinding,
    request: &ToolCallExecuteRequest, cancellation: &crate::kernel::CancellationToken) -> Result<Value, String> {
    cancellation.check()?;
    if binding.run_id != request.run_id || !crate::resource_gateway::is_reader(&request.tool)
        || !matches!(super::kernel_authority::resolve_run_route(Some(binding)),super::kernel_authority::RunRoute::PiNative)
        || binding.read_only_executor != fox_engine_protocol::ResourceExecutor::Rust {
        return Err("native reader does not match its frozen Host executor".into());
    }
    let elapsed=super::native_model_budget::elapsed_ms(database,&request.run_id)?;
    let remaining=binding.budgets.limit_operation_ms(binding.budgets.tool_execution_ms,elapsed);
    if remaining<=0 { return Err("runtime.native_tool_budget_exhausted".into()); }
    if let super::HostToolCallExecution::Replay(response) = super::create_fresh_host_tool_call(database,
        &request.run_id,&request.call_id,&request.tool,&request.input,"running",false)? { return Ok(response); }
    let outcome = super::managed_files::execute_observed_reader(database,binding,&request.tool,&request.input,
        &request.call_id,cancellation,Duration::from_millis(remaining as u64));
    super::finalize_host_tool_execution(database,&request.run_id,&request.call_id,&request.tool,&request.input,Vec::new(),outcome)
}

pub(super) fn handles(kind: &str) -> bool {
    matches!(kind, TOOL_BATCH_REGISTER | TOOL_CALL_EXECUTE | TOOL_CALL_CLOSE | TOOL_CALL_CANCEL | TOOL_CALL_FACTS)
}

pub(super) fn dispatch(host: &super::RuntimeHost, stdin: &Arc<Mutex<std::process::ChildStdin>>, envelope: RuntimeEnvelope) {
    if envelope.r#type == TOOL_CALL_FACTS {
        answer(host, stdin, &envelope, Ok(super::native_recovery::response(&host.database, &envelope)));
        host.native_batches.changed.notify_all();
        return;
    }
    let result = (|| -> Result<Option<Value>, String> {
        let binding = validate_request(host, &envelope)?;
        let payload = envelope.payload.clone().ok_or("native request omitted payload")?;
        let registry = &host.native_batches;
        match envelope.r#type.as_str() {
            TOOL_BATCH_REGISTER => {
                let token=host.state.lock().map_err(|_| "runtime state poisoned")?.cancellation.run_token(&binding.run_id);
                let cancelled=match token {
                    Ok(token)=>token.is_cancelled(),
                    Err(error)=>{
                        let stopped:bool=host.database.with_connection(|connection|connection.query_row(
                            "SELECT status IN ('cancelling','cancelled','failed','interrupted','completed') FROM runs WHERE id=?1",[&binding.run_id],|row|row.get(0)))?;
                        if !stopped { return Err(error); }
                        true
                    }
                };
                let request: ToolBatchRegisterRequest = serde_json::from_value(payload).map_err(|e| e.to_string())?;
                let session = host.database.get_runtime_session(&binding.conversation_id)?.ok_or("native session is absent")?;
                if !session.is_native() || session.context_generation != request.context_generation
                    || envelope.runtime_session_id.as_deref() != Some(session.runtime_session_id.as_str()) {
                    return Err("native registration belongs to a different session generation".into());
                }
                let response=registry.register_inner(&host.database,&request,Some(&envelope),cancelled)?;
                registry.changed.notify_all();
                serde_json::to_value(response).map(Some).map_err(|e| e.to_string())
            }
            TOOL_CALL_CLOSE => {
                let request: ToolCallCloseRequest = serde_json::from_value(payload).map_err(|e| e.to_string())?;
                serde_json::to_value(registry.close(&host.database, &request)?).map(Some).map_err(|e| e.to_string())
            }
            TOOL_CALL_CANCEL => {
                let request: ToolCallCancelRequest = serde_json::from_value(payload).map_err(|e| e.to_string())?;
                let response = registry.cancel(&host.database, &request)?;
                use crate::kernel::CancellationPort;
                host.state.lock().map_err(|_| "runtime state poisoned")?.cancellation.request_tool_cancel(&request.run_id, &request.call_id);
                let approval=host.database.with_connection(|connection|connection.query_row(
                    "SELECT a.id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                     WHERE t.run_id=?1 AND t.runtime_tool_call_id=?2 AND a.status IN ('pending','approved') AND a.claimed_at IS NULL",
                    params![request.run_id,request.call_id],|row|row.get::<_,String>(0)).optional())?;
                if let Some(approval)=approval {
                    let pending=host.state.lock().map_err(|_| "runtime state poisoned")?.pending_approvals.clone();
                    if let Some(waiter)=pending.lock().map_err(|_| "approval map poisoned")?.remove(&approval) { let _=waiter.sender.send(false); };
                }
                serde_json::to_value(response).map(Some).map_err(|e| e.to_string())
            }
            TOOL_CALL_EXECUTE => {
                let request: ToolCallExecuteRequest = serde_json::from_value(payload).map_err(|e| e.to_string())?;
                validate_execution_identity(&host.database,&request)?;
                if let Err(error)=validate_tool_policy(host,&envelope,&request,&binding) {
                    return Ok(execution_refusal(host,&envelope,&request,&error));
                }
                let Some(permit) = host.try_acquire_tool_handler() else {
                    return Ok(execution_refusal(host,&envelope,&request,"runtime.tool_handler_capacity_exceeded"));
                };
                let host = host.clone();
                let stdin = stdin.clone();
                let envelope = envelope.clone();
                std::thread::spawn(move || {
                    let _permit = permit;
                    let outcome=(|| {
                        let elapsed=super::native_model_budget::elapsed_ms(&host.database,&request.run_id)?;
                        let remaining=binding.budgets.limit_operation_ms(binding.budgets.tool_execution_ms,elapsed);
                        if remaining<=0 { return Err("runtime.native_tool_budget_exhausted".into()); }
                        host.native_batches.claim(&host.database,&request,Duration::from_millis(remaining as u64),
                            || validate_tool(&host,&envelope,&request))
                    })();
                    match outcome {
                        Ok(Claim::Execute) => {
                            #[cfg(test)]
                            super::native_crash_integration_tests::checkpoint("native_claimed", &request.run_id, &request.call_id);
                            if let Err(error) = validate_tool(&host, &envelope, &request) {
                                match record_result(&host.database, &request, &failure(error)) {
                                    Ok(response) => answer(&host, &stdin, &envelope, Ok(response)),
                                    Err(error) => host.handle_worker_transport_crash(&stdin,error),
                                }
                            } else {
                                if crate::resource_gateway::is_reader(&request.tool) {
                                    let outcome = host.state.lock().map_err(|_| "runtime state poisoned".to_owned())
                                        .and_then(|state|state.cancellation.tool_token(&request.run_id,&request.call_id))
                                        .and_then(|token|execute_native_reader(&host.database,&binding,&request,&token));
                                    match record_result(&host.database,&request,&outcome.unwrap_or_else(failure)) {
                                        Ok(response) => answer(&host,&stdin,&envelope,Ok(response)),
                                        Err(error) => host.handle_worker_transport_crash(&stdin,error),
                                    }
                                    host.native_batches.changed.notify_all();
                                    return;
                                }
                                let mut execute = envelope.clone();
                                execute.payload.as_mut().unwrap()["toolCallId"] = json!(request.call_id);
                                super::dispatch_tool_request_envelope(&host, &host.app, &host.database,
                                    &host.attachments_dir, &host.sessions_dir, &host.state, &stdin,
                                    &host.yuxi_client, execute);
                                let committed = host.database.with_connection(|connection| {
                                    let rows = positions(connection, &request.run_id, &request.batch_id)?;
                                    if position(&rows, &request.call_id)?.result.is_none() {
                                        return Err(invalid("native executor returned without a committed result"));
                                    }
                                    Ok(())
                                });
                                if let Err(error) = committed { host.handle_worker_transport_crash(&stdin,error); }
                            }
                        }
                        Ok(Claim::Replay(response)) => answer(&host, &stdin, &envelope, Ok(response)),
                        Err(error) => {
                            if let Some(receipt)=execution_refusal(&host,&envelope,&request,&error) {
                                answer(&host,&stdin,&envelope,Ok(receipt));
                            }
                        }
                        Ok(Claim::Wait) => unreachable!("claim waits internally"),
                    }
                    host.native_batches.changed.notify_all();
                });
                Ok(None)
            }
            _ => Err("unsupported native request".into()),
        }
    })();
    match result {
        Ok(None) => {},
        Ok(Some(body)) => answer(host, stdin, &envelope, Ok(body)),
        Err(error) => answer(host, stdin, &envelope, Err(error)),
    }
}

fn answer(host: &super::RuntimeHost, stdin: &Arc<Mutex<std::process::ChildStdin>>, envelope: &RuntimeEnvelope,
    response: Result<Value, String>) {
    let body = response.unwrap_or_else(failure);
    let kind = if body["isError"] == true { "native_batch.refused" } else {
        match envelope.r#type.as_str() {
            TOOL_BATCH_REGISTER => "tool_batch.registered", TOOL_CALL_CLOSE => "tool_call.closed",
            TOOL_CALL_CANCEL => "tool_call.cancelled", TOOL_CALL_FACTS => "tool_call.facts_completed", _ => "tool.execute_completed",
        }
    };
    if let Err(error) = super::write_protocol_message(stdin, &HostResponse::for_request(envelope, kind, body)) {
        if settle_disconnected_native_response(&host.database,envelope).unwrap_or(false) {
            if let Some(run)=envelope.run_id.as_deref() { super::retire_disconnected_native_run(&host.database,&host.state,run); }
            eprintln!("[native.response_transport_lost] request={} error={error}",envelope.id);
            return;
        }
        host.handle_worker_transport_crash(stdin,error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database { Database::open(":memory:".into()).unwrap() }
    fn call(id: &str, tool: &str, order: i64) -> NativeToolCallRegistration {
        NativeToolCallRegistration { call_id: id.into(), tool: tool.into(), source_order: order,
            input_digest: format!("sha256:{}", "a".repeat(64)) }
    }
    fn batch(calls: Vec<NativeToolCallRegistration>) -> ToolBatchRegisterRequest {
        ToolBatchRegisterRequest { run_id: "run".into(), context_generation: 1, calls }
    }
    fn execute(batch: &str, id: &str, tool: &str) -> ToolCallExecuteRequest {
        ToolCallExecuteRequest { run_id: "run".into(), batch_id: batch.into(), call_id: id.into(), tool: tool.into(), input: json!({"path":"test"}) }
    }
    fn registered(db: &Database, registry: &SharedBatchRegistry, calls: Vec<NativeToolCallRegistration>) -> String {
        registry.register(db, &batch(calls)).unwrap().batch_id
    }
    fn cancel(request: &ToolCallExecuteRequest) -> ToolCallCancelRequest {
        ToolCallCancelRequest { run_id: request.run_id.clone(), batch_id: request.batch_id.clone(), call_id: request.call_id.clone() }
    }
    fn close(request: &ToolCallExecuteRequest) -> ToolCallCloseRequest {
        ToolCallCloseRequest { run_id: request.run_id.clone(), batch_id: request.batch_id.clone(), call_id: request.call_id.clone(),
            reason: ToolCallCloseReason::ValidationFailed, is_error: true }
    }

    #[test]
    fn only_the_frozen_read_whitelist_may_overlap() {
        for tool in ["read","ls","find","grep","skill_load"] { assert!(is_parallel_read_only_tool(tool)); }
        for tool in ["write_file","edit_file","read_attachment","run_command","call_mcp_tool","READ",""] {
            assert!(!is_parallel_read_only_tool(tool));
        }
    }

    #[test]
    fn native_ingress_requires_the_frozen_route_and_matching_envelope_identity() {
        use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, TimeBudgets};
        let binding = RunControlBinding {
            schema_version: fox_engine_protocol::CONTROL_SCHEMA_VERSION, run_id: "run".into(),
            conversation_id: "conversation".into(), engine_id: "pi".into(), execution_profile_id: "legacy".into(),
            authority: ExecutionAuthority::Authoritative, read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: "snapshot".into(), permission: FrozenPermission {
                mode: PermissionMode::Ask, project_root: None, grants: Vec::new(), approval_epoch: None,
            }, budgets: TimeBudgets::default(), executor_route: Some(ExecutorRoute::PiNative),
        };
        let envelope = RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
            kind:"request".into(),id:"request".into(),r#type:TOOL_BATCH_REGISTER.into(),request_id:None,
            conversation_id:Some("conversation".into()),runtime_session_id:Some("session".into()),run_id:Some("run".into()),
            seq:None,timestamp:String::new(),payload:Some(json!({"runId":"run"})) };
        assert!(validate_binding(&envelope,&binding).is_ok());
        let mut foreign = envelope.clone(); foreign.payload.as_mut().unwrap()["runId"] = json!("foreign");
        assert!(validate_binding(&foreign,&binding).is_err());
        foreign = envelope.clone(); foreign.conversation_id = Some("foreign".into());
        assert!(validate_binding(&foreign,&binding).is_err());
        foreign = envelope.clone(); foreign.run_id = Some("foreign".into());
        assert!(validate_binding(&foreign,&binding).is_err());
        let mut invalid = binding.clone(); invalid.executor_route = None;
        assert!(validate_binding(&envelope,&invalid).is_err());
        invalid = binding.clone(); invalid.authority = ExecutionAuthority::Legacy;
        assert!(validate_binding(&envelope,&invalid).is_err());
        invalid = binding.clone(); invalid.engine_id = "codex".into();
        assert!(validate_binding(&envelope,&invalid).is_err());
    }

    #[test]
    fn registration_is_atomic_idempotent_and_survives_registry_restart() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let request = batch(vec![call("a","read",0),call("b","write_file",1)]);
        let first = registry.register(&db, &request).unwrap();
        assert_eq!(SharedBatchRegistry::new().register(&db,&request).unwrap(), first);
        let mut changed = request.clone(); changed.calls[0].input_digest = format!("sha256:{}", "b".repeat(64));
        assert!(registry.register(&db,&changed).is_err());
        changed = request.clone(); changed.calls[1].tool = "read".into();
        assert!(registry.register(&db,&changed).is_err());
        changed = request.clone(); changed.context_generation = 2;
        assert!(registry.register(&db,&changed).is_err());
        let overlap = batch(vec![call("new","read",0),call("a","read",1)]);
        assert!(registry.register(&db,&overlap).is_err());
        db.with_connection(|c| {
            assert_eq!(c.query_row("SELECT COUNT(*) FROM kernel_tool_calls",[],|r|r.get::<_,i64>(0))?,2);
            assert_eq!(c.query_row("SELECT COUNT(*) FROM kernel_tool_batches",[],|r|r.get::<_,i64>(0))?,1);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn registration_refuses_sparse_duplicate_and_bad_digest_without_rows() {
        let db = database(); let registry = SharedBatchRegistry::new();
        for calls in [vec![],vec![call("a","read",1)],vec![call("a","read",0),call("a","grep",1)]] {
            assert!(registry.register(&db,&batch(calls)).is_err());
        }
        let mut invalid = call("a","read",0); invalid.input_digest = "sha256:invalid".into();
        assert!(registry.register(&db,&batch(vec![invalid])).is_err());
        let id = registered(&db,&registry,vec![call("b","read",1),call("a","read",0)]);
        assert!(id.starts_with("native-batch-"));
    }

    #[test]
    fn mixed_batch_waits_for_every_earlier_barrier() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("a","read",0),call("b","grep",1),call("c","write_file",2),call("d","read",3)]);
        let a = execute(&id,"a","read"); let b = execute(&id,"b","grep");
        let c = execute(&id,"c","write_file"); let d = execute(&id,"d","read");
        assert_eq!(SharedBatchRegistry::try_claim(&db,&d).unwrap(),Claim::Wait);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&a).unwrap(),Claim::Execute);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&b).unwrap(),Claim::Execute);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&c).unwrap(),Claim::Wait);
        record_result(&db,&a,&json!({"result":"a"})).unwrap();
        assert_eq!(SharedBatchRegistry::try_claim(&db,&c).unwrap(),Claim::Wait);
        record_result(&db,&b,&failure("read failed")).unwrap();
        assert_eq!(SharedBatchRegistry::try_claim(&db,&c).unwrap(),Claim::Execute);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&d).unwrap(),Claim::Wait);
        record_result(&db,&c,&json!({"result":"c"})).unwrap();
        assert_eq!(SharedBatchRegistry::try_claim(&db,&d).unwrap(),Claim::Execute);
    }

    #[test]
    fn six_readers_claim_at_most_four_slots_atomically() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,(0..6).map(|i|call(&i.to_string(),"read",i)).collect());
        let joins: Vec<_> = (0..6).map(|i| { let db = db.clone(); let request = execute(&id,&i.to_string(),"read");
            std::thread::spawn(move || SharedBatchRegistry::try_claim(&db,&request).unwrap()) }).collect();
        let claims: Vec<_> = joins.into_iter().map(|join|join.join().unwrap()).collect();
        assert_eq!(claims.iter().filter(|claim|**claim == Claim::Execute).count(),4);
        assert_eq!(claims.iter().filter(|claim|**claim == Claim::Wait).count(),2);
    }

    #[test]
    fn unknown_and_foreign_ids_never_claim_or_change_a_position() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("a","read",0)]);
        let mut request = execute(&id,"a","read"); request.run_id = "foreign".into();
        assert!(SharedBatchRegistry::try_claim(&db,&request).is_err());
        assert!(registry.close(&db,&close(&request)).is_err());
        assert!(registry.cancel(&db,&cancel(&request)).is_err());
        request.run_id = "run".into(); request.batch_id = "foreign".into();
        assert!(SharedBatchRegistry::try_claim(&db,&request).is_err());
        request.batch_id = id.clone(); request.tool = "write_file".into();
        assert!(SharedBatchRegistry::try_claim(&db,&request).is_err());
        assert_eq!(SharedBatchRegistry::try_claim(&db,&execute(&id,"a","read")).unwrap(),Claim::Execute);
    }

    #[test]
    fn effective_input_and_committed_result_are_immutable() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("a","read",0)]); let request = execute(&id,"a","read");
        assert_eq!(SharedBatchRegistry::try_claim(&db,&request).unwrap(),Claim::Execute);
        assert!(SharedBatchRegistry::try_claim(&db,&request).is_err());
        let response = json!({"isError":false,"result":{"content":[{"type":"text","text":"exact result"}]}});
        assert_eq!(record_result(&db,&request,&response).unwrap(),response);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&request).unwrap(),Claim::Replay(response.clone()));
        let mut changed = request.clone(); changed.input["path"] = json!("changed");
        assert!(SharedBatchRegistry::try_claim(&db,&changed).is_err());
        assert!(record_result(&db,&request,&failure("changed")).is_err());
        assert!(!registry.close(&db,&close(&request)).unwrap().closed);
    }

    #[test]
    fn failed_registration_rolls_back_all_positions_and_can_retry() {
        let db = database(); let registry = SharedBatchRegistry::new();
        db.execute_raw_sql("CREATE TRIGGER reject_second BEFORE INSERT ON kernel_tool_calls WHEN NEW.source_order=1 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let request = batch(vec![call("a","read",0),call("b","read",1)]);
        assert!(registry.register(&db,&request).is_err());
        db.with_connection(|c| { assert_eq!(c.query_row("SELECT COUNT(*) FROM kernel_tool_calls",[],|r|r.get::<_,i64>(0))?,0); Ok(()) }).unwrap();
        db.execute_raw_sql("DROP TRIGGER reject_second").unwrap();
        assert_eq!(registry.register(&db,&request).unwrap().registered_calls,2);
    }

    #[test]
    fn cancelled_inflight_write_keeps_blocking_and_late_success_is_failure() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("w","write_file",0),call("r","read",1)]);
        let write = execute(&id,"w","write_file"); let read = execute(&id,"r","read");
        assert_eq!(SharedBatchRegistry::try_claim(&db,&write).unwrap(),Claim::Execute);
        assert!(registry.cancel(&db,&cancel(&write)).unwrap().outcome_unknown);
        assert!(registry.cancel(&db,&cancel(&write)).unwrap().outcome_unknown);
        assert!(!registry.close(&db,&close(&write)).unwrap().closed);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&read).unwrap(),Claim::Wait);
        let late = record_result(&db,&write,&json!({"isError":false,"result":"evidence"})).unwrap();
        assert_eq!(late["isError"],true); assert_eq!(late["result"],"evidence");
        assert!(!registry.cancel(&db,&cancel(&write)).unwrap().outcome_unknown);
        assert_eq!(SharedBatchRegistry::try_claim(&db,&read).unwrap(),Claim::Execute);
    }

    #[test]
    fn closed_and_unclaimed_cancelled_positions_release_waiters_without_execution() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("a","unknown",0),call("b","write_file",1),call("c","read",2)]);
        let a = execute(&id,"a","unknown"); let b = execute(&id,"b","write_file"); let c = execute(&id,"c","read");
        assert!(registry.close(&db,&close(&a)).unwrap().closed);
        assert!(!registry.cancel(&db,&cancel(&b)).unwrap().outcome_unknown);
        assert!(matches!(SharedBatchRegistry::try_claim(&db,&b).unwrap(),Claim::Replay(body) if body["isError"] == true));
        assert_eq!(SharedBatchRegistry::try_claim(&db,&c).unwrap(),Claim::Execute);
    }

    #[test]
    fn admission_wait_has_a_deadline_and_observes_external_cancellation() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("w","write_file",0),call("r","read",1)]);
        let request = execute(&id,"r","read");
        assert!(registry.claim(&db,&request,Duration::ZERO,||Ok(())).unwrap_err().contains("timed out"));
        assert_eq!(registry.claim(&db,&request,Duration::from_secs(1),||Err("cancelled".into())).unwrap_err(),"cancelled");
    }

    #[test]
    fn reopening_the_database_never_reexecutes_a_claim_with_unknown_outcome() {
        let path = std::env::temp_dir().join(format!("fox-native-reopen-{}.db",uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap(); let registry = SharedBatchRegistry::new();
        let registration = batch(vec![call("write","write_file",0)]);
        let id = registry.register(&db,&registration).unwrap().batch_id;
        let request = execute(&id,"write","write_file");
        assert_eq!(SharedBatchRegistry::try_claim(&db,&request).unwrap(),Claim::Execute);
        drop(db);
        let reopened = Database::open(path.clone()).unwrap();
        assert_eq!(SharedBatchRegistry::new().register(&reopened,&registration).unwrap().batch_id,id);
        assert!(SharedBatchRegistry::try_claim(&reopened,&request).unwrap_err().contains("unknown outcome"));
        reopened.with_connection(|c| {
            let rows = positions(c,"run",&id)?; let row = position(&rows,"write")?;
            assert!(row.claimed_at.is_some()); assert!(row.result.is_none()); assert_eq!(row.state,"running");
            Ok(())
        }).unwrap();
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn response_is_not_constructed_until_the_result_commit_succeeds() {
        let db = database(); let registry = SharedBatchRegistry::new();
        let id = registered(&db,&registry,vec![call("a","read",0)]); let request = execute(&id,"a","read");
        SharedBatchRegistry::try_claim(&db,&request).unwrap();
        let envelope = RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
            kind:"request".into(),id:"request".into(),r#type:TOOL_CALL_EXECUTE.into(),request_id:None,
            conversation_id:Some("conversation".into()),runtime_session_id:Some("session".into()),run_id:Some("run".into()),
            seq:None,timestamp:String::new(),payload:Some(serde_json::to_value(&request).unwrap()) };
        db.execute_raw_sql("CREATE TRIGGER reject_result BEFORE UPDATE OF result_json ON kernel_tool_calls BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(prepare_response(&db,&envelope,"tool.execute_completed",json!({"result":"full result"})).is_err());
        assert!(SharedBatchRegistry::try_claim(&db,&request).is_err());
        db.execute_raw_sql("DROP TRIGGER reject_result").unwrap();
        let response = prepare_response(&db,&envelope,"tool.execute_completed",json!({"result":"full result"})).unwrap();
        assert_eq!(response.request_id,"request");
        assert_eq!(SharedBatchRegistry::try_claim(&db,&request).unwrap(),Claim::Replay(response.payload));
    }

    #[test]
    fn cancelled_result_commit_retries_keep_the_same_failed_receipt() {
        let db=database(); let registry=SharedBatchRegistry::new();
        let id=registered(&db,&registry,vec![call("a","read",0)]); let request=execute(&id,"a","read");
        SharedBatchRegistry::try_claim(&db,&request).unwrap();
        registry.cancel(&db,&cancel(&request)).unwrap();
        let original=json!({"isError":false,"result":"retained late output"});
        let first=record_result(&db,&request,&original).unwrap();
        assert_eq!(first["isError"],true);
        assert_eq!(record_result(&db,&request,&original).unwrap(),first);
    }

    #[test]
    fn a_new_run_cannot_register_an_old_session_call_or_a_retired_generation() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new(); let registry=SharedBatchRegistry::new();
        fixture.seed(&fixture.old_run,"old-write","running",None,true);
        let request=ToolBatchRegisterRequest {run_id:fixture.new_run.clone(),context_generation:3,calls:vec![call("old-write","read",0)]};
        let mut envelope=fixture.envelope(Vec::new()); envelope.r#type=TOOL_BATCH_REGISTER.into();
        envelope.payload=Some(serde_json::to_value(&request).unwrap());
        assert!(registry.register_inner(&fixture.database,&request,Some(&envelope),false).unwrap_err().contains("earlier Run"));
        let mut fresh=request.clone(); fresh.calls[0].call_id="new-call".into(); fresh.context_generation=2;
        envelope.payload=Some(serde_json::to_value(&fresh).unwrap());
        assert!(registry.register_inner(&fixture.database,&fresh,Some(&envelope),false).is_err());
    }

    fn approval_context() -> (super::super::native_recovery::tests::Fixture,ApprovalRecoveryContext) {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();
        fixture.activate_old();
        let batch=fixture.seed(&fixture.old_run,"approval","running",None,true);
        let host=fixture.host_call("approval","pending",None);
        let approval_id=fixture.approval(&host,"approved");
        let request=ToolCallExecuteRequest {run_id:fixture.old_run.clone(),batch_id:batch,
            call_id:"approval".into(),tool:"read".into(),input:json!({})};
        let mut envelope=fixture.envelope(Vec::new()); envelope.run_id=Some(request.run_id.clone());
        envelope.r#type=TOOL_CALL_EXECUTE.into(); envelope.payload=Some(serde_json::to_value(&request).unwrap());
        (fixture,ApprovalRecoveryContext {envelope,request,approval_id,category:"tool_execution".into()})
    }

    #[test]
    fn approval_recovery_is_scoped_to_one_call_and_cleans_up_on_unwind() {
        let (fixture,context)=approval_context(); let request=context.request.clone();
        {
            let _guard=ApprovalRecoveryGuard::enter(context.clone()).unwrap();
            assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).unwrap().is_some());
            assert!(recovering_host_tool_call(&fixture.database,&request.run_id,"foreign",&request.tool,&request.input,false).is_err());
            assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&json!({"changed":true}),false).is_err());
            assert!(recovering_approval_decision(&fixture.database,&request.run_id,"foreign-approval").is_err());
            assert_eq!(recovering_approval_decision(&fixture.database,&request.run_id,&context.approval_id).unwrap(),Some(true));
        }
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).unwrap().is_none());
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard=ApprovalRecoveryGuard::enter(context).unwrap();
            panic!("injected recovery interruption");
        })).is_err());
        assert!(recovering_approval_decision(&fixture.database,&request.run_id,"outside-recovery").unwrap().is_none());
    }

    #[test]
    fn recovered_approval_uses_the_normal_single_execution_claim() {
        let (fixture,context)=approval_context(); let request=context.request.clone();
        let _guard=ApprovalRecoveryGuard::enter(context.clone()).unwrap();
        let record=recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,true).unwrap().unwrap();
        assert_eq!(record.status,"running");
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).is_err());
        assert!(!fixture.database.claim_approved_tool_call(&request.run_id,&request.call_id).unwrap());
        fixture.database.with_connection(|connection| {
            let claimed:Option<i64>=connection.query_row("SELECT claimed_at FROM approvals WHERE id=?1",[&context.approval_id],|row|row.get(0))?;
            assert!(claimed.is_some());
            let rows=positions(connection,&request.run_id,&request.batch_id)?;
            assert_eq!(position(&rows,&request.call_id)?.claimed_at,Some(10));
            Ok(())
        }).unwrap();
    }

    #[test]
    fn approval_recovery_refuses_current_read_only_permission_and_actual_claims() {
        let (fixture,mut context)=approval_context();
        fixture.database.with_connection(|connection| {
            connection.execute("UPDATE kernel_tool_calls SET tool='write_file' WHERE tool_call_id='approval'",[])?;
            connection.execute("UPDATE tool_calls SET tool_name='write_file' WHERE runtime_tool_call_id='approval'",[])?;
            Ok(())
        }).unwrap();
        context.request.tool="write_file".into(); context.envelope.payload=Some(serde_json::to_value(&context.request).unwrap());
        fixture.database.update_conversation_permission_mode(&fixture.conversation,"read_only").unwrap();
        let request=context.request.clone();
        let _guard=ApprovalRecoveryGuard::enter(context.clone()).unwrap();
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).unwrap_err().contains("read-only"));
        fixture.database.with_connection(|connection| connection.execute("UPDATE approvals SET claimed_at=20 WHERE id=?1",[&context.approval_id])).unwrap();
        assert!(recovering_approval_decision(&fixture.database,&request.run_id,&context.approval_id).is_err());
    }

    #[test]
    fn run_cancellation_before_late_commit_never_returns_success() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new(); let batch=fixture.seed(&fixture.old_run,"late","running",None,true);
        fixture.database.with_connection(|connection| connection.execute("UPDATE runs SET status='cancelled' WHERE id=?1",[&fixture.old_run])).unwrap();
        let request=ToolCallExecuteRequest {run_id:fixture.old_run.clone(),batch_id:batch,call_id:"late".into(),tool:"read".into(),input:json!({})};
        let result=record_result(&fixture.database,&request,&json!({"isError":false,"result":"effect evidence"})).unwrap();
        assert_eq!(result["isError"],true); assert_eq!(result["result"],"effect evidence");
    }

    #[test]
    fn native_reader_executes_through_the_real_rust_gateway_and_replays_the_receipt() {
        use super::super::native_recovery::tests::Fixture;
        use crate::kernel::{CancellationPort,CancellationRegistry};
        let root=std::env::temp_dir().join(format!("fox-native-reader-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path=root.join("note.txt"); std::fs::write(&path,"Native reader proof: 中文").unwrap();
        let fixture=Fixture::with_root(Some(root.to_str().unwrap()));
        fixture.activate_old();
        let binding=fixture.database.run_control_binding(&fixture.old_run).unwrap().unwrap();
        super::super::native_model_budget::freeze(&fixture.database,&binding,4096,None,None).unwrap();
        let batch=fixture.seed(&fixture.old_run,"read-note","pending",None,false);
        let request=ToolCallExecuteRequest {run_id:fixture.old_run.clone(),batch_id:batch,
            call_id:"read-note".into(),tool:"read".into(),input:json!({"path":"note.txt"})};
        let cancellation=CancellationRegistry::default(); cancellation.register_run(&request.run_id).unwrap();
        let token=cancellation.tool_token(&request.run_id,&request.call_id).unwrap();
        assert_eq!(SharedBatchRegistry::try_claim(&fixture.database,&request).unwrap(),Claim::Execute);
        let result=execute_native_reader(&fixture.database,&binding,&request,&token).unwrap();
        assert_eq!(result["result"]["content"][0]["text"],"Native reader proof: 中文");
        let committed=record_result(&fixture.database,&request,&result).unwrap();
        std::fs::write(&path,"changed after committed result").unwrap();
        assert_eq!(SharedBatchRegistry::try_claim(&fixture.database,&request).unwrap(),Claim::Replay(committed));
        let identity=path.canonicalize().unwrap().to_string_lossy().into_owned();
        assert!(fixture.database.host_observation_for(&request.run_id,&identity).unwrap().is_some());
        cancellation.request_run_cancel(&request.run_id);
        assert!(execute_native_reader(&fixture.database,&binding,&request,&token).unwrap_err().contains("cancelled"));
        drop(fixture);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn disconnected_receipts_retire_only_the_original_run_after_all_claims_settle() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();
        fixture.activate_old();
        let first=fixture.seed(&fixture.old_run,"first","running",None,true);
        let second=fixture.seed(&fixture.old_run,"second","running",None,true);
        let request=ToolCallExecuteRequest {run_id:fixture.old_run.clone(),batch_id:first,call_id:"first".into(),tool:"read".into(),input:json!({})};
        let mut envelope=fixture.envelope(Vec::new()); envelope.r#type=TOOL_CALL_EXECUTE.into();
        envelope.run_id=Some(fixture.old_run.clone()); envelope.payload=Some(serde_json::to_value(&request).unwrap());
        let original=json!({"isError":false,"result":"already committed"});
        record_result(&fixture.database,&request,&original).unwrap();
        assert!(settle_disconnected_native_response(&fixture.database,&envelope).unwrap());
        fixture.database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT status FROM runs WHERE id=?1",[&fixture.old_run],|row|row.get::<_,String>(0))?,"running");
            Ok(())
        }).unwrap();
        let second_request=ToolCallExecuteRequest {batch_id:second,call_id:"second".into(),..request.clone()};
        record_result(&fixture.database,&second_request,&json!({"isError":false,"result":"second committed"})).unwrap();
        assert!(settle_disconnected_native_response(&fixture.database,&envelope).unwrap());
        assert!(settle_disconnected_native_response(&fixture.database,&envelope).unwrap());
        fixture.database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT status FROM runs WHERE id=?1",[&fixture.old_run],|row|row.get::<_,String>(0))?,"interrupted");
            assert_eq!(connection.query_row("SELECT status FROM runs WHERE id=?1",[&fixture.new_run],|row|row.get::<_,String>(0))?,"queued");
            let rows=positions(connection,&request.run_id,&request.batch_id)?;
            let stored:Value=serde_json::from_str(position(&rows,&request.call_id)?.result.as_deref().unwrap()).unwrap();
            assert_eq!(stored,original);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn transport_loss_closes_only_unclaimed_positions() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();
        fixture.seed(&fixture.old_run,"not-claimed","pending",None,false);
        fixture.seed(&fixture.old_run,"claimed-unknown","running",None,true);
        fixture.seed(&fixture.new_run,"other-run","pending",None,false);
        close_unclaimed_after_transport_loss(&fixture.database,&fixture.old_run).unwrap();
        fixture.database.with_connection(|connection| {
            for (call,state,has_result) in [("not-claimed","not_started",true),("claimed-unknown","running",false),("other-run","pending",false)] {
                let actual:(String,Option<String>)=connection.query_row("SELECT state,result_json FROM kernel_tool_calls WHERE tool_call_id=?1",[call],|row|Ok((row.get(0)?,row.get(1)?)))?;
                assert_eq!(actual.0,state); assert_eq!(actual.1.is_some(),has_result);
            }
            Ok(())
        }).unwrap();
    }

    #[test]
    fn tool_cancellation_is_visible_to_an_approval_waiter_without_settling_the_claim() {
        let (fixture,context)=approval_context();
        assert!(!cancelled_native_approval(&fixture.database,&context.request.run_id,&context.approval_id).unwrap());
        let registry=SharedBatchRegistry::new();
        let response=registry.cancel(&fixture.database,&cancel(&context.request)).unwrap();
        assert!(response.outcome_unknown);
        assert!(cancelled_native_approval(&fixture.database,&context.request.run_id,&context.approval_id).unwrap());
        assert!(!cancelled_native_approval(&fixture.database,&context.request.run_id,"foreign-approval").unwrap());
        fixture.database.with_connection(|connection| {
            let rows=positions(connection,&context.request.run_id,&context.request.batch_id)?;
            let row=position(&rows,&context.request.call_id)?;
            assert_eq!(row.state,"running");assert!(row.result.is_none());
            assert_eq!(row.close_reason.as_deref(),Some("aborted"));
            Ok(())
        }).unwrap();
    }

    #[test]
    fn an_existing_claim_cannot_be_closed_as_an_unexecuted_pending_position() {
        let db=database();let registry=SharedBatchRegistry::new();
        let batch=registered(&db,&registry,vec![call("a","read",0)]);let request=execute(&batch,"a","read");
        db.with_connection(|connection|connection.execute("UPDATE kernel_tool_calls SET claimed_at=5 WHERE tool_call_id='a'",[])).unwrap();
        assert!(!registry.close(&db,&close(&request)).unwrap().closed);
        assert!(registry.cancel(&db,&cancel(&request)).unwrap().outcome_unknown);
        db.with_connection(|connection| {
            let rows=positions(connection,&request.run_id,&request.batch_id)?;let row=position(&rows,&request.call_id)?;
            assert!(row.result.is_none());assert_eq!(row.claimed_at,Some(5));
            Ok(())
        }).unwrap();
    }

    #[test]
    fn repair_approval_recovery_keeps_the_dedicated_category_and_never_uses_the_ordinary_claim() {
        let (fixture,mut context)=approval_context();
        context.category="task_repair_budget_override".into();
        context.request.tool="task_repair_escalate_start".into();
        context.request.input=json!({"taskId":"task","attemptId":"attempt","expectedVersion":3,
            "rootCause":"confirmed root cause","findingIds":["finding"],"escalationReason":"one bounded repair"});
        context.envelope.payload=Some(serde_json::to_value(&context.request).unwrap());
        fixture.database.with_connection(|connection| {
            let input=canonical(&context.request.input)?;
            connection.execute("UPDATE kernel_tool_calls SET tool=?1,canonical_input_json=?2 WHERE tool_call_id='approval'",params![context.request.tool,input])?;
            connection.execute("UPDATE tool_calls SET tool_name=?1,input_json=?2 WHERE runtime_tool_call_id='approval'",params![context.request.tool,input])?;
            connection.execute("UPDATE approvals SET category=?2,decision_json=?3 WHERE id=?1",params![context.approval_id,context.category,
                canonical(&json!({"approved":true,"scope":"once","category":context.category}))?])?;
            Ok(())
        }).unwrap();
        let _guard=ApprovalRecoveryGuard::enter(context.clone()).unwrap();
        let request=&context.request;
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).unwrap().is_some());
        assert_eq!(recovering_approval_decision(&fixture.database,&request.run_id,&context.approval_id).unwrap(),Some(true));
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,true).unwrap_err().contains("dedicated"));
        let mut changed=request.input.clone();changed["expectedVersion"]=json!(4);
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&changed,false).is_err());
        fixture.database.with_connection(|connection| {
            let claimed:Option<i64>=connection.query_row("SELECT claimed_at FROM approvals WHERE id=?1",[&context.approval_id],|row|row.get(0))?;
            assert!(claimed.is_none(),"the TLS admission must not consume the budget override");
            connection.execute("UPDATE approvals SET decision_json=?2 WHERE id=?1",params![context.approval_id,
                canonical(&json!({"approved":true,"scope":"conversation","category":context.category}))?])?;
            Ok(())
        }).unwrap();
        assert!(recovering_approval_decision(&fixture.database,&request.run_id,&context.approval_id).is_err());
        fixture.database.with_connection(|connection|connection.execute("UPDATE approvals SET claimed_at=42 WHERE id=?1",[&context.approval_id])).unwrap();
        assert!(recovering_host_tool_call(&fixture.database,&request.run_id,&request.call_id,&request.tool,&request.input,false).is_err());
    }

    #[test]
    fn recovered_repair_approval_executes_the_original_dedicated_budget_transaction_once() {
        let (database,input,host_call,approval)=Database::native_repair_override_fixture_for_test();
        let session=format!("native-repair-recovery-{}",uuid::Uuid::new_v4());
        database.with_connection(|connection| {
            connection.execute("INSERT INTO runtime_sessions(id,conversation_id,runtime_type,runtime_session_id,status,created_at,
                                updated_at,format_version,context_generation,native_session_path)
                                VALUES(?1,?2,'pi',?1,'ready',1,1,2,1,'test-only-repair-session.jsonl')",params![session,input.conversation_id])?;
            connection.execute("UPDATE runs SET runtime_session_id=?2 WHERE id=?1",params![input.run_id,session])?;
            Ok(())
        }).unwrap();
        let registry=SharedBatchRegistry::new();
        let batch=registry.register(&database,&ToolBatchRegisterRequest {run_id:input.run_id.clone(),context_generation:1,
            calls:vec![call(&host_call.runtime_tool_call_id,"task_repair_escalate_start",0)]}).unwrap().batch_id;
        let request=ToolCallExecuteRequest {run_id:input.run_id.clone(),batch_id:batch,call_id:host_call.runtime_tool_call_id.clone(),
            tool:host_call.tool_name.clone(),input:host_call.input.clone()};
        assert_eq!(SharedBatchRegistry::try_claim(&database,&request).unwrap(),Claim::Execute);
        let envelope=RuntimeEnvelope {protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
            kind:"request".into(),id:"repair-recovery-test".into(),r#type:TOOL_CALL_EXECUTE.into(),request_id:None,
            conversation_id:Some(input.conversation_id.clone()),runtime_session_id:Some(session),run_id:Some(input.run_id.clone()),
            seq:None,timestamp:String::new(),payload:Some(serde_json::to_value(&request).unwrap())};
        let _guard=ApprovalRecoveryGuard::enter(ApprovalRecoveryContext {envelope,request:request.clone(),approval_id:approval.id.clone(),
            category:"task_repair_budget_override".into()}).unwrap();
        assert!(recovering_host_tool_call(&database,&request.run_id,&request.call_id,&request.tool,&request.input,false).unwrap().is_some());
        assert_eq!(recovering_approval_decision(&database,&request.run_id,&approval.id).unwrap(),Some(true));
        assert!(!database.claim_approved_tool_call(&request.run_id,&request.call_id).unwrap(),"ordinary approval cannot consume a repair budget override");
        let mut changed=request.input.clone();changed["expectedVersion"]=json!(input.expected_task_version+1);
        assert!(recovering_host_tool_call(&database,&request.run_id,&request.call_id,&request.tool,&changed,false).is_err());
        let outcome=super::super::work_tools::execute_approved_task_repair_override(&database,&input.conversation_id,&input.run_id,
            &host_call.id,&approval.id,&request.input).unwrap();
        let response=super::super::finalize_host_tool_execution(&database,&request.run_id,&request.call_id,&request.tool,&request.input,Vec::new(),Ok(outcome.result)).unwrap();
        let committed=record_result(&database,&request,&response).unwrap();
        assert_ne!(committed["isError"],true);
        database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM task_repair_override_events WHERE approval_id=?1",[&approval.id],|row|row.get::<_,i64>(0))?,1);
            let claimed:Option<i64>=connection.query_row("SELECT claimed_at FROM approvals WHERE id=?1",[&approval.id],|row|row.get(0))?;
            assert!(claimed.is_some());
            Ok(())
        }).unwrap();
        assert!(recovering_approval_decision(&database,&request.run_id,&approval.id).is_err());
        assert_eq!(SharedBatchRegistry::try_claim(&database,&request).unwrap(),Claim::Replay(committed));
    }

    #[test]
    fn registration_after_cancellation_commits_only_nonexecution_and_can_reconcile_pi_results() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();let registry=SharedBatchRegistry::new();
        fixture.database.with_connection(|connection|connection.execute("UPDATE runs SET status='cancelled' WHERE id=?1",[&fixture.new_run])).unwrap();
        let request=ToolBatchRegisterRequest {run_id:fixture.new_run.clone(),context_generation:3,
            calls:vec![call("late-unknown","unknown_tool",0)]};
        let mut envelope=fixture.envelope(Vec::new());envelope.r#type=TOOL_BATCH_REGISTER.into();
        envelope.payload=Some(serde_json::to_value(&request).unwrap());
        fixture.database.execute_raw_sql("CREATE TRIGGER reject_cancel_close BEFORE UPDATE OF state ON kernel_tool_calls WHEN NEW.state='not_started' BEGIN SELECT RAISE(ABORT,'injected cancelled registration failure'); END;").unwrap();
        assert!(registry.register_inner(&fixture.database,&request,Some(&envelope),false).is_err());
        fixture.database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM kernel_tool_calls",[],|row|row.get::<_,i64>(0))?,0);
            Ok(())
        }).unwrap();
        fixture.database.execute_raw_sql("DROP TRIGGER reject_cancel_close").unwrap();
        let batch=registry.register_inner(&fixture.database,&request,Some(&envelope),false).unwrap().batch_id;
        let execute=ToolCallExecuteRequest {run_id:request.run_id.clone(),batch_id:batch.clone(),call_id:"late-unknown".into(),tool:"unknown_tool".into(),input:json!({})};
        assert!(matches!(SharedBatchRegistry::try_claim(&fixture.database,&execute).unwrap(),Claim::Replay(body) if body["isError"]==true));
        let close=ToolCallCloseRequest {run_id:request.run_id.clone(),batch_id:batch,call_id:execute.call_id,
            reason:ToolCallCloseReason::Aborted,is_error:true};
        assert!(registry.close(&fixture.database,&close).unwrap().closed);
        assert!(registry.close(&fixture.database,&close).unwrap().closed);
        let mut fact=Fixture::call("late-unknown");fact.tool="unknown_tool".into();fact.has_result=true;
        let facts=super::super::native_recovery::response(&fixture.database,&fixture.envelope(vec![fact]));
        assert_eq!(facts["blocked"],false);assert_eq!(facts["facts"][0]["state"],"not_executed");
        let mut changed=request.clone();changed.calls[0].input_digest=format!("sha256:{}","b".repeat(64));
        assert!(registry.register_inner(&fixture.database,&changed,Some(&envelope),true).is_err());
    }

    #[test]
    fn cancellation_after_registration_never_closes_an_existing_execution_claim() {
        let database=database();let registry=SharedBatchRegistry::new();
        let registration=batch(vec![call("claimed","write_file",0),call("unclaimed","read",1)]);
        let id=registry.register(&database,&registration).unwrap().batch_id;
        assert_eq!(SharedBatchRegistry::try_claim(&database,&execute(&id,"claimed","write_file")).unwrap(),Claim::Execute);
        assert_eq!(registry.register_inner(&database,&registration,None,true).unwrap().batch_id,id);
        database.with_connection(|connection| {
            let rows=positions(connection,&registration.run_id,&id)?;
            let claimed=position(&rows,"claimed")?;assert_eq!(claimed.state,"running");
            assert!(claimed.claimed_at.is_some());assert!(claimed.result.is_none());assert!(claimed.close_reason.is_none());
            let unclaimed=position(&rows,"unclaimed")?;assert_eq!(unclaimed.state,"not_started");
            assert!(unclaimed.claimed_at.is_none());assert_eq!(unclaimed.close_reason.as_deref(),Some("aborted"));
            assert!(unclaimed.result.is_some());
            Ok(())
        }).unwrap();
    }

    #[test]
    fn finalize_retains_late_native_output_without_reopening_terminal_host_business_state() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();fixture.activate_old();
        let batch=fixture.seed(&fixture.old_run,"late-output","running",None,true);
        fixture.host_call("late-output","running",None);
        fixture.database.mark_run_interrupted(&fixture.old_run,"runtime.test_disconnect","original transport failure").unwrap();
        let result=json!({"content":[{"type":"text","text":format!("{}tail-marker-终","数据".repeat(40_000))}],"details":{"source":"late Host executor"}});
        let response=super::super::finalize_host_tool_execution(&fixture.database,&fixture.old_run,"late-output","read",&json!({}),Vec::new(),Ok(result.clone())).unwrap();
        assert_eq!(response["isError"],true);
        assert_eq!(response["errorDetails"]["code"],"runtime.native_late_result");
        assert_eq!(response["result"]["truncated"],true);
        assert_eq!(response["toolResultStorage"]["stored"],false);
        assert_eq!(response["toolResultStorage"]["retrievableBytes"],0);
        assert_eq!(response["nativeLateEvidence"]["retrievable"],false);
        assert!(response.get("resultRef").is_none());
        assert!(canonical(&response).unwrap().len()<crate::database::MAX_STORED_TOOL_RESULT_BYTES);
        let hash=response["nativeLateEvidence"]["sha256"].as_str().unwrap();
        fixture.database.with_connection(|connection| {
            let retained:String=connection.query_row("SELECT body_json FROM tool_call_result_blobs WHERE sha256=?1",[hash],|row|row.get(0))?;
            assert_eq!(serde_json::from_str::<Value>(&retained).unwrap(),result);
            assert!(retained.contains("tail-marker-终"));
            let status:(String,Option<String>)=connection.query_row("SELECT status,error_code FROM runs WHERE id=?1",[&fixture.old_run],|row|Ok((row.get(0)?,row.get(1)?)))?;
            assert_eq!(status,("interrupted".into(),Some("runtime.test_disconnect".into())));
            let host:(String,Option<String>)=connection.query_row("SELECT status,result_json FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id='late-output'",[&fixture.old_run],|row|Ok((row.get(0)?,row.get(1)?)))?;
            assert_eq!(host.0,"interrupted");assert!(host.1.is_none());
            Ok(())
        }).unwrap();
        let request=ToolCallExecuteRequest {run_id:fixture.old_run.clone(),batch_id:batch,call_id:"late-output".into(),tool:"read".into(),input:json!({})};
        assert_eq!(SharedBatchRegistry::try_claim(&fixture.database,&request).unwrap(),Claim::Replay(response.clone()));
        assert_eq!(record_result(&fixture.database,&request,&response).unwrap(),response);
        let repeated=super::super::finalize_host_tool_execution(&fixture.database,&fixture.old_run,"late-output","read",&json!({}),Vec::new(),Ok(result.clone())).unwrap();
        assert_eq!(repeated,response);
        let mut changed=result;changed["content"][0]["text"]=json!("changed late output");
        assert!(retain_late_native_result(&fixture.database,&fixture.old_run,"late-output","read",&json!({}),&changed,"retry").is_err());
        assert!(retain_late_native_result(&fixture.database,&fixture.old_run,"late-output","read",&json!({"different":true}),&changed,"retry").is_err());
    }

    #[test]
    fn late_evidence_requires_a_terminal_run_and_an_original_execution_claim() {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();fixture.activate_old();
        fixture.seed(&fixture.old_run,"unclaimed","pending",None,false);
        fixture.host_call("unclaimed","pending",None);
        assert!(retain_late_native_result(&fixture.database,&fixture.old_run,"unclaimed","read",&json!({}),&json!({"answer":1}),"injected").unwrap().is_none());
        fixture.database.mark_run_interrupted(&fixture.old_run,"runtime.test_disconnect","original failure").unwrap();
        assert!(retain_late_native_result(&fixture.database,&fixture.old_run,"unclaimed","read",&json!({}),&json!({"answer":1}),"injected").is_err());
        fixture.database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM tool_call_result_blobs",[],|row|row.get::<_,i64>(0))?,0);
            Ok(())
        }).unwrap();
    }

    fn refusal_fixture() -> (super::super::native_recovery::tests::Fixture,SharedBatchRegistry,ToolCallExecuteRequest,RuntimeEnvelope) {
        use super::super::native_recovery::tests::Fixture;
        let fixture=Fixture::new();let registry=SharedBatchRegistry::new();
        let batch=registry.register(&fixture.database,&ToolBatchRegisterRequest {run_id:fixture.new_run.clone(),context_generation:3,
            calls:vec![call("barrier","write_file",0),call("refused","read",1)]}).unwrap().batch_id;
        let request=ToolCallExecuteRequest {run_id:fixture.new_run.clone(),batch_id:batch,call_id:"refused".into(),tool:"read".into(),input:json!({})};
        let mut envelope=fixture.envelope(Vec::new());envelope.r#type=TOOL_CALL_EXECUTE.into();
        envelope.payload=Some(serde_json::to_value(&request).unwrap());
        (fixture,registry,request,envelope)
    }

    fn assert_refusal_receipt(fixture: &super::super::native_recovery::tests::Fixture,
        request: &ToolCallExecuteRequest, receipt: &Value) {
        use super::super::native_recovery::tests::Fixture;
        assert_eq!(receipt["isError"],true);
        let stored=fixture.database.with_connection(|connection| {
            let rows=positions(connection,&request.run_id,&request.batch_id)?;let row=position(&rows,&request.call_id)?;
            assert_eq!(row.state,"not_started");assert!(row.claimed_at.is_none());
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM kernel_tool_calls WHERE claimed_at IS NOT NULL",[],|query|query.get::<_,i64>(0))?,0);
            serde_json::from_str::<Value>(row.result.as_deref().unwrap()).map_err(|error|invalid(error.to_string()))
        }).unwrap();
        assert_eq!(&stored,receipt);
        let mut call=Fixture::call(&request.call_id);call.has_result=true;
        let facts=super::super::native_recovery::response(&fixture.database,&fixture.envelope(vec![call]));
        assert_eq!(facts["blocked"],false);
        assert_eq!(&facts["facts"][0]["response"],receipt);
        assert_eq!(Sha256::digest(canonical(receipt).unwrap().as_bytes()),
            Sha256::digest(canonical(&facts["facts"][0]["response"]).unwrap().as_bytes()),"Pi's response digest must equal the recovered Host receipt digest");
    }

    #[test]
    fn capacity_and_policy_refusals_return_the_committed_nonexecution_receipt() {
        let (fixture,registry,request,envelope)=refusal_fixture();
        validate_execution_identity(&fixture.database,&request).unwrap();
        let receipt=registry.refusal_receipt(&fixture.database,&envelope,&request).unwrap();
        assert_refusal_receipt(&fixture,&request,&receipt);
        assert_eq!(registry.refusal_receipt(&fixture.database,&envelope,&request).unwrap(),receipt);
    }

    #[test]
    fn claim_failure_cancellation_and_timeout_return_the_same_committed_receipt() {
        for reason in ["claim refused","cancelled","timeout"] {
            let (fixture,registry,request,envelope)=refusal_fixture();
            let failure=registry.claim(&fixture.database,&request,Duration::ZERO,|| {
                if reason=="timeout" {Ok(())} else {Err(reason.into())}
            }).unwrap_err();
            if reason=="timeout" {assert!(failure.contains("timed out"));} else {assert_eq!(failure,reason);}
            let receipt=registry.refusal_receipt(&fixture.database,&envelope,&request).unwrap();
            assert_refusal_receipt(&fixture,&request,&receipt);
        }
    }

    #[test]
    fn refusal_never_settles_foreign_identity_changed_input_or_an_unknown_claim() {
        let (fixture,registry,request,envelope)=refusal_fixture();
        let mut foreign=envelope.clone();foreign.conversation_id=Some("foreign".into());
        assert!(registry.refusal_receipt(&fixture.database,&foreign,&request).is_err());
        let mut changed=request.clone();changed.tool="write_file".into();
        assert!(registry.refusal_receipt(&fixture.database,&envelope,&changed).is_err());
        let claimed=ToolCallExecuteRequest {call_id:"barrier".into(),tool:"write_file".into(),input:json!({"path":"test"}),..request.clone()};
        assert_eq!(SharedBatchRegistry::try_claim(&fixture.database,&claimed).unwrap(),Claim::Execute);
        let mut claimed_envelope=envelope.clone();claimed_envelope.payload=Some(serde_json::to_value(&claimed).unwrap());
        let mut drifted=claimed.clone();drifted.input["path"]=json!("other");
        assert!(registry.refusal_receipt(&fixture.database,&claimed_envelope,&drifted).is_err());
        assert!(registry.refusal_receipt(&fixture.database,&claimed_envelope,&claimed).unwrap_err().contains("unknown execution outcome"));
        fixture.database.with_connection(|connection| {
            let rows=positions(connection,&request.run_id,&request.batch_id)?;
            assert!(position(&rows,"barrier")?.result.is_none());assert_eq!(position(&rows,"barrier")?.state,"running");
            assert!(position(&rows,"refused")?.result.is_none());assert_eq!(position(&rows,"refused")?.state,"pending");
            Ok(())
        }).unwrap();
    }
}
