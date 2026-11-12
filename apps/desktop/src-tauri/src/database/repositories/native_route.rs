use super::{projection_violation, run_control};
use crate::canonical_json::CanonicalJson;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriterRoute { Legacy, Kernel, PiNative }

pub(super) fn writer_route(connection: &Connection, run_id: &str) -> rusqlite::Result<WriterRoute> {
    let Some(binding) = run_control::binding_in_connection(connection, run_id)? else { return Ok(WriterRoute::Legacy); };
    match (binding.authority, binding.executor_route) {
        (ExecutionAuthority::Legacy, None) => Ok(WriterRoute::Legacy),
        (ExecutionAuthority::Authoritative, None) => Ok(WriterRoute::Kernel),
        (ExecutionAuthority::Authoritative, Some(ExecutorRoute::PiNative)) if binding.engine_id == "pi" => Ok(WriterRoute::PiNative),
        _ => Err(projection_violation(format!("{}: unsupported frozen Run executor route", run_control::BINDING_INTEGRITY_FAILED))),
    }
}

/// The writer route for a startup sweep or a recovery scan: `None` when the
/// Run's frozen binding cannot be verified. Such a Run is never driven or
/// recovered (every strict reader keeps refusing it); the sweep in
/// [`interrupt_unverifiable_runs`] interrupts it so one bad historical row
/// cannot fail the whole startup. A database fault still fails.
pub(super) fn sweep_writer_route(connection: &Connection, run_id: &str) -> rusqlite::Result<Option<WriterRoute>> {
    match writer_route(connection, run_id) {
        Ok(route) => Ok(Some(route)),
        Err(error) if run_control::binding_refusal(&error).is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Startup: a Run whose frozen control binding fails verification (corrupt,
/// tampered, from an older development build, or with an unknown route) is
/// interrupted with a stable code and logged; it is not
/// skipped past and never continued. Historical Runs that already reached a
/// terminal state keep their own terminal facts (a graph child's terminal fact
/// is immutable once reconciled) and none of their messages, tool calls or
/// approvals are rewritten; every reader still refuses them.
///
/// A Run this sweep actually moves to `interrupted` settles its own assistant
/// messages, open tool calls and pending approvals in the same transaction.
/// `mark_run_interrupted` cannot do that: its legacy-writer check refuses the
/// unverifiable binding. Bindings are left untouched and no Kernel event is emitted.
pub(super) fn interrupt_unverifiable_runs(transaction: &Transaction<'_>, now: i64) -> rusqlite::Result<Vec<String>> {
    let mut query = transaction.prepare(
        "SELECT b.run_id,r.status FROM run_control_bindings b JOIN runs r ON r.id=b.run_id ORDER BY b.run_id")?;
    let runs = query.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    let mut refused = Vec::new();
    for (run_id, status) in runs {
        let error = match writer_route(transaction, &run_id) { Ok(_) => continue, Err(error) => error };
        let Some((code, message)) = run_control::binding_refusal(&error) else { return Err(error) };
        let interrupted = transaction.execute(
            "UPDATE runs SET status='interrupted', finished_at=?2, error_code=?3, error_message=?4
             WHERE id=?1 AND status IN ('queued','running','cancelling')",
            params![run_id, now, code, message])? > 0;
        if interrupted {
            // messages.status 无 CHECK。生产写入为 streaming/completed/interrupted；
            // 历史失败行还可能是 failed/error/pending。只回写本 Run 的 assistant 行。
            transaction.execute(
                "UPDATE messages SET status='interrupted', updated_at=?2
                 WHERE run_id=?1 AND role='assistant' AND status IN ('streaming','pending','failed','error')",
                params![run_id, now])?;
            transaction.execute(
                "UPDATE tool_calls
                 SET status='interrupted', error_message=?2, completed_at=?3, updated_at=?3
                 WHERE run_id=?1 AND status IN ('pending','running')",
                params![run_id, message, now])?;
            super::end_pending_approvals_for_interrupted_run(transaction, &run_id, Some(code), now)?;
        }
        eprintln!("fox.run_control binding_refused run={run_id} status={status} code={code} interrupted={interrupted} error={error}");
        refused.push(run_id);
    }
    Ok(refused)
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

/// D1-1: a typed, Host-written execution refusal for one unclaimed approval.
/// The code travels in the standard error receipt; the message is what both the
/// user's card and the model read. Only the Host constructs this — never tool
/// input.
pub(crate) struct ApprovalRefusal {
    pub code: String,
    pub message: String,
}

impl ApprovalRefusal {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }
}

/// The single native error receipt every approval refusal settles to. It matches
/// the Host's `failure()` + `refusal_receipt` contract (`isError`/`error`/
/// `errorDetails`) so a live waiter, a detached recovery and the startup repair
/// all answer with byte-identical facts and the immutable-receipt guard cannot
/// see a re-wrapped second result.
pub(crate) fn approval_refusal_receipt(refusal: &ApprovalRefusal) -> Value {
    json!({
        "isError": true,
        "error": refusal.message,
        "errorDetails": { "code": refusal.code, "retryable": false },
    })
}

/// Read a persisted Host invalidation from an approval decision. The field is an
/// irreversible execution barrier: it is validated at this unknown boundary
/// (object with a non-empty code/message and a positive timestamp) so a corrupt
/// or foreign value can never masquerade as one.
pub(crate) fn approval_invalidation(decision: Option<&Value>) -> Option<(String, String)> {
    let invalidation = decision?.get("invalidation")?;
    let code = invalidation.get("code")?.as_str()?.trim();
    let message = invalidation.get("message")?.as_str()?.trim();
    let at = invalidation.get("at")?.as_i64()?;
    if code.is_empty() || message.is_empty() || at <= 0 {
        return None;
    }
    Some((code.to_owned(), message.to_owned()))
}

/// Merge the Host's invalidation into the user's original decision, preserving
/// `approved`/`scope`/`category`/`resolved_at`. The first invalidation wins: a
/// later generic reason can never overwrite a specific revocation, and a
/// regrant never clears it.
fn merge_approval_invalidation(decision: Option<Value>, refusal: &ApprovalRefusal, now: i64) -> Value {
    let mut decision = decision.filter(Value::is_object).unwrap_or_else(|| json!({}));
    if approval_invalidation(Some(&decision)).is_some() {
        return decision;
    }
    decision["invalidation"] = json!({ "code": refusal.code, "message": refusal.message, "at": now });
    decision
}

/// The outcome of one refusal settlement attempt, carrying the persisted receipt
/// so a retry replays the original value instead of fabricating a second one.
#[derive(Debug)]
pub(crate) enum ApprovalRefusalSettlement {
    /// This call consumed the approval, closed the Host intent and (for
    /// PiNative) settled the native receipt in one transaction.
    Settled(Value),
    /// A previous settlement already committed the same barrier; the persisted
    /// receipt is replayed verbatim.
    AlreadySettled(Value),
    /// Not an unclaimed, identity-verified refusal target (claimed, foreign
    /// category/route, terminal, or unverifiable native intent). The caller
    /// must still persist the invalidation barrier; nothing was written here.
    NotEligible,
}

/// D1-2: register the irreversible invalidation on one unclaimed approval
/// (pending or approved) without settling it. Idempotent — the first reason
/// wins. The claim/resolve/startup/waiter gates perform the D1-1 settlement, so
/// the setter never needs an immediate settlement to succeed to record the
/// revocation. Returns whether this call wrote the barrier.
pub(crate) fn register_approval_invalidation_in_tx(
    transaction: &Transaction<'_>,
    approval_id: &str,
    refusal: &ApprovalRefusal,
    now: i64,
) -> rusqlite::Result<bool> {
    // The outer Option is row existence; the inner is decision_json, which is
    // legitimately NULL on a pending approval. A pending row must still receive
    // the barrier (merged into an empty decision), so only a missing row exits.
    let row: Option<Option<String>> = transaction.query_row(
        "SELECT decision_json FROM approvals
         WHERE id = ?1 AND status IN ('pending','approved') AND claimed_at IS NULL AND category = 'tool_execution'",
        [approval_id], |row| row.get::<_, Option<String>>(0),
    ).optional()?;
    let Some(decision_json) = row else { return Ok(false); };
    let decision = decision_json.and_then(|json| serde_json::from_str::<Value>(&json).ok());
    if approval_invalidation(decision.as_ref()).is_some() {
        return Ok(false);
    }
    let merged = merge_approval_invalidation(decision, refusal, now).canonical_string();
    let changed = transaction.execute(
        "UPDATE approvals SET decision_json = ?2
         WHERE id = ?1 AND status IN ('pending','approved') AND claimed_at IS NULL AND category = 'tool_execution'",
        params![approval_id, merged],
    )?;
    Ok(changed == 1)
}

/// D1-1: the unified, transaction-level refusal settlement shared by the
/// approved-execution claim, the resolve barrier, the live waiter, the detached
/// recovery and the startup pre-validation.
///
/// It re-verifies the frozen binding/route, the run/conversation ownership, the
/// exact approval↔Host↔native association (tool, canonical input, category,
/// decision/scope) and — for PiNative — the current session/generation and the
/// native receipt's running/claimed/no-result/no-close-reason state. Only then
/// does it consume the approval, close the Host intent and settle the native
/// receipt with one standard error result, each write guarded by a CAS row
/// count. Any mismatch rolls the whole transaction back; a retry replays the
/// persisted receipt (`AlreadySettled`). A Legacy/Host-only record is settled by
/// its own writer contract — no native record is fabricated for it, and the
/// retired Kernel and the task-repair override keep their separate contracts.
pub(crate) fn settle_unclaimed_approval_refusal_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    approval_id: &str,
    refusal: &ApprovalRefusal,
    now: i64,
) -> rusqlite::Result<ApprovalRefusalSettlement> {
    // An unverifiable binding was already interrupted by the startup sweep; a
    // retired Kernel Run keeps its own retirement. Neither is written here.
    let Some(route) = sweep_writer_route(transaction, run_id)? else {
        return Ok(ApprovalRefusalSettlement::NotEligible);
    };
    if route == WriterRoute::Kernel {
        return Ok(ApprovalRefusalSettlement::NotEligible);
    }

    let row = transaction.query_row(
        "SELECT a.status, a.category, a.decision_json, a.claimed_at,
                t.runtime_tool_call_id, t.tool_name, t.input_json, t.conversation_id,
                t.status, t.execution_location, t.requires_approval, t.result_json,
                r.status, r.conversation_id, r.runtime_session_id
         FROM approvals a
         JOIN tool_calls t ON t.id = a.tool_call_id
         JOIN runs r ON r.id = t.run_id
         WHERE a.id = ?1 AND t.run_id = ?2",
        params![approval_id, run_id],
        |row| Ok((
            row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<i64>>(3)?,
            row.get::<_, String>(4)?, row.get::<_, String>(5)?, row.get::<_, String>(6)?, row.get::<_, String>(7)?,
            row.get::<_, String>(8)?, row.get::<_, String>(9)?, row.get::<_, i64>(10)?, row.get::<_, Option<String>>(11)?,
            row.get::<_, String>(12)?, row.get::<_, String>(13)?, row.get::<_, Option<String>>(14)?,
        )),
    ).optional()?;
    let Some((approval_status, category, decision_json, approval_claimed,
        call_id, tool_name, input_json, host_conversation, host_status, execution_location,
        requires_approval, host_result, run_status, run_conversation, run_session)) = row
    else { return Ok(ApprovalRefusalSettlement::NotEligible); };

    if category != "tool_execution" || execution_location != "host" || requires_approval != 1
        || host_conversation != run_conversation
    {
        return Ok(ApprovalRefusalSettlement::NotEligible);
    }
    let decision = decision_json.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let persisted = approval_invalidation(decision.as_ref());
    // The first persisted invalidation wins over any caller-supplied reason.
    let effective = match &persisted {
        Some((code, message)) => ApprovalRefusal { code: code.clone(), message: message.clone() },
        None => ApprovalRefusal { code: refusal.code.clone(), message: refusal.message.clone() },
    };
    let host_input = serde_json::from_str::<Value>(&input_json).ok();

    // The native receipt is a third execution fact for PiNative. Its committed
    // identity (tool + claimed canonical input) must match the Host intent.
    let native = if route == WriterRoute::PiNative {
        let native = transaction.query_row(
            "SELECT state, claimed_at, result_json, close_reason, tool, canonical_input_json, context_generation
             FROM kernel_tool_calls WHERE run_id = ?1 AND tool_call_id = ?2",
            params![run_id, call_id],
            |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?, row.get::<_, String>(4)?, row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
            )),
        ).optional()?;
        let Some(native) = native else { return Ok(ApprovalRefusalSettlement::NotEligible); };
        let native_input = native.5.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok());
        if native.4 != tool_name || host_input.is_none() || native_input != host_input {
            return Ok(ApprovalRefusalSettlement::NotEligible);
        }
        Some(native)
    } else {
        None
    };

    let host_pending = host_status == "pending";
    let unclaimed = approval_claimed.is_none();

    // Already settled by a previous refusal: replay the exact persisted receipt.
    // The current session generation may have advanced since the commit, so the
    // replay trusts the receipt's own committed identity, not a re-validation.
    if !host_pending && host_status == "failed" && persisted.is_some()
        && (approval_claimed.is_some() || approval_status == "expired")
    {
        let receipt = match route {
            WriterRoute::PiNative => {
                let native = native.as_ref().expect("PiNative native row verified above");
                if native.0 != "failed" { return Ok(ApprovalRefusalSettlement::NotEligible); }
                native.2.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok())
            }
            _ => host_result.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok()),
        };
        return match receipt {
            Some(receipt) => Ok(ApprovalRefusalSettlement::AlreadySettled(receipt)),
            None => Ok(ApprovalRefusalSettlement::NotEligible),
        };
    }

    // A fresh settlement needs a still-pending Host intent, an unclaimed
    // approval, a running Run and (for approved) the original decision intact.
    if !host_pending || !unclaimed || run_status != "running"
        || !matches!(approval_status.as_str(), "pending" | "approved")
    {
        return Ok(ApprovalRefusalSettlement::NotEligible);
    }
    if approval_status == "approved" {
        let approved = decision.as_ref().and_then(|d| d.get("approved"))
            .is_some_and(|value| value == &json!(true) || value == &json!(1));
        let scope = decision.as_ref().and_then(|d| d.get("scope")).and_then(Value::as_str);
        let decision_category = decision.as_ref().and_then(|d| d.get("category")).and_then(Value::as_str);
        if !approved || !matches!(scope, Some("once") | Some("conversation")) || decision_category != Some("tool_execution") {
            return Ok(ApprovalRefusalSettlement::NotEligible);
        }
    }
    if route == WriterRoute::PiNative {
        let native = native.as_ref().expect("PiNative native row verified above");
        // This recovery stage's running/claimed receipt, no result, no close.
        if native.0 != "running" || native.1.is_none() || native.2.is_some() || native.3.is_some() {
            return Ok(ApprovalRefusalSettlement::NotEligible);
        }
        let session = transaction.query_row(
            "SELECT runtime_session_id, context_generation, format_version FROM runtime_sessions
             WHERE conversation_id = ?1 AND runtime_type = 'pi'",
            [&run_conversation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        ).optional()?;
        let Some((session_id, generation, format)) = session else {
            return Ok(ApprovalRefusalSettlement::NotEligible);
        };
        if format < 2 || generation < 1 || native.6 != generation
            || run_session.as_deref().is_some_and(|recorded| recorded != session_id)
        {
            return Ok(ApprovalRefusalSettlement::NotEligible);
        }
    }

    let receipt = approval_refusal_receipt(&effective);
    let receipt_json = receipt.canonical_string();
    let merged_json = merge_approval_invalidation(decision, &effective, now).canonical_string();

    // 1) Consume the approval. An approved decision keeps its status/scope and
    //    resolved_at; a pending one expires (never becomes a user denial).
    #[cfg(test)]
    refusal_fault::checkpoint()?;
    let approval_changed = if approval_status == "approved" {
        transaction.execute(
            "UPDATE approvals SET decision_json = ?2, claimed_at = ?3, claimed_by_run_id = ?4
             WHERE id = ?1 AND status = 'approved' AND claimed_at IS NULL AND category = 'tool_execution'",
            params![approval_id, merged_json, now, run_id],
        )?
    } else {
        transaction.execute(
            "UPDATE approvals SET status = 'expired', decision_json = ?2, resolved_at = ?3,
                    claimed_at = ?3, claimed_by_run_id = ?4
             WHERE id = ?1 AND status = 'pending' AND claimed_at IS NULL AND category = 'tool_execution'",
            params![approval_id, merged_json, now, run_id],
        )?
    };
    if approval_changed != 1 {
        return Err(projection_violation("approval refusal settlement lost its unclaimed approval".into()));
    }

    // 2) Close the Host intent with the standard error result, preserving input.
    #[cfg(test)]
    refusal_fault::checkpoint()?;
    let host_changed = transaction.execute(
        "UPDATE tool_calls SET status = 'failed', error_message = ?3, result_json = ?4,
                completed_at = ?5, updated_at = ?5, result_blob_sha256 = NULL
         WHERE run_id = ?1 AND runtime_tool_call_id = ?2 AND execution_location = 'host'
           AND status = 'pending' AND requires_approval = 1
           AND EXISTS(SELECT 1 FROM runs WHERE runs.id = tool_calls.run_id AND runs.status = 'running')",
        params![run_id, call_id, effective.message, receipt_json, now],
    )?;
    if host_changed != 1 {
        return Err(projection_violation("approval refusal settlement lost its pending Host intent".into()));
    }

    // 3) Settle the native receipt with the SAME standard error result, keeping
    //    the original native claim. Legacy/Host-only records have no native fact.
    if route == WriterRoute::PiNative {
        #[cfg(test)]
        refusal_fault::checkpoint()?;
        let native_changed = transaction.execute(
            "UPDATE kernel_tool_calls SET state = 'failed', result_json = ?3, settled_at = ?4
             WHERE run_id = ?1 AND tool_call_id = ?2 AND state = 'running'
               AND claimed_at IS NOT NULL AND result_json IS NULL AND close_reason IS NULL",
            params![run_id, call_id, receipt_json, now],
        )?;
        if native_changed != 1 {
            return Err(projection_violation("approval refusal settlement lost its native execution claim".into()));
        }
    }

    Ok(ApprovalRefusalSettlement::Settled(receipt))
}

/// Test-only fault injection for the refusal settlement's three write points.
/// Arming write index N makes the Nth write fail so the test can prove the whole
/// transaction rolls back (no half-settled approval/Host/native fact survives).
#[cfg(test)]
pub(crate) mod refusal_fault {
    use std::cell::Cell;

    thread_local! {
        static FAIL_AT: Cell<Option<usize>> = const { Cell::new(None) };
        static WRITES: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn arm(write_index: usize) {
        FAIL_AT.with(|fail| fail.set(Some(write_index)));
        WRITES.with(|writes| writes.set(0));
    }

    pub(crate) fn disarm() {
        FAIL_AT.with(|fail| fail.set(None));
        WRITES.with(|writes| writes.set(0));
    }

    pub(crate) fn checkpoint() -> rusqlite::Result<()> {
        let next = WRITES.with(|writes| { let value = writes.get() + 1; writes.set(value); value });
        if FAIL_AT.with(|fail| fail.get()) == Some(next) {
            return Err(super::projection_violation("injected approval refusal settlement fault".into()));
        }
        Ok(())
    }
}

impl super::Database {
    /// B1: settle one historical Kernel Run exactly like the startup
    /// retirement. Used when something tries to start or cancel a frozen Kernel
    /// Run after startup: the Kernel loop is gone, so nothing else may own it.
    /// Returns whether this call changed the Run.
    pub(crate) fn retire_kernel_run(&self, run_id: &str) -> Result<bool,String> {
        let now = super::now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if writer_route(&transaction, run_id)? != WriterRoute::Kernel { return Ok(false); }
            super::kernel_projection::expire_inactive_approvals(&transaction, Some(run_id), now)?;
            let changed = retire_kernel_run_row(&transaction, run_id, now)?;
            transaction.commit()?;
            Ok(changed)
        })
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

    /// D1-1: settle one approval refusal in its own Immediate transaction. The
    /// live waiter, the resolve barrier and the revocation follow-up call this;
    /// the startup repair uses the tx helper inside its larger transaction.
    pub(crate) fn settle_approval_refusal(
        &self,
        run_id: &str,
        approval_id: &str,
        refusal: &ApprovalRefusal,
    ) -> Result<ApprovalRefusalSettlement, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let outcome = settle_unclaimed_approval_refusal_in_tx(&transaction, run_id, approval_id, refusal, super::now_ms())?;
            transaction.commit()?;
            Ok(outcome)
        })
    }

    /// The persisted invalidation message of one approval, if any. Read-only;
    /// the live waiter uses it to detect the barrier before settling.
    pub(crate) fn approval_invalidation_message(&self, approval_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            let decision_json: Option<String> = connection.query_row(
                "SELECT decision_json FROM approvals WHERE id = ?1", [approval_id], |row| row.get::<_, Option<String>>(0),
            ).optional()?.flatten();
            let decision = decision_json.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok());
            Ok(approval_invalidation(decision.as_ref()).map(|(_, message)| message))
        })
    }

    /// D1-5: the durable, atomic core of safely cancelling an UNOWNED PiNative
    /// approval Run. The Host caller has already proven there is no live owner
    /// (route, queued/reserved, worker session, leases, waiters, recovery
    /// registration) and holds the same Run lease recovery uses; this method
    /// re-reads the Run state inside one Immediate transaction and settles the
    /// unexecuted intent without fabricating an execution result:
    ///   * each unclaimed approved/pending tool-execution approval is settled
    ///     through the same D1-1 refusal settlement (standard isError receipt,
    ///     Host intent failed, native claim settled), with a user-stop reason. A
    ///     more specific persisted revocation reason wins over the generic stop.
    ///   * a pending approval becomes `cancelled` (user reason), never a timeout;
    ///     an approved one keeps its original decision plus the stop/invalidation.
    ///   * the Run is terminated `interrupted` with `runtime.user_cancelled` and
    ///     the full existing projection (messages/trace/child/colleague/task/
    ///     contact bubbles) in the SAME transaction.
    /// Returns false (without rewriting anything) for a non-PiNative or already
    /// terminal Run, so a second cancel is idempotent and a late cancel cannot
    /// replace a real outcome. Any ineligible candidate rolls back the entire cancellation and returns an error.
    pub(crate) fn cancel_unowned_native_approval_run(&self, run_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if sweep_writer_route(&transaction, run_id)? != Some(WriterRoute::PiNative) {
                return Ok(false);
            }
            let status: String = transaction.query_row(
                "SELECT status FROM runs WHERE id = ?1", [run_id], |row| row.get(0),
            )?;
            if matches!(status.as_str(), "completed" | "failed" | "cancelled" | "interrupted") {
                return Ok(false);
            }
            // D1-5 #4: a Host intent already `running` is an actual execution with
            // an unknown outcome, not a provably unexecuted approval. Leave it to
            // the existing reconciliation diagnostics instead of fabricating a
            // failed/interrupted result.
            let host_running: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM tool_calls WHERE run_id = ?1 AND status = 'running' AND execution_location = 'host')",
                [run_id], |row| row.get(0),
            )?;
            if host_running {
                return Ok(false);
            }
            let now = super::now_ms();
            let refusal = ApprovalRefusal::new("runtime.user_cancelled", "用户已停止本次操作，旧的批准不会恢复");
            let approval_ids: Vec<String> = {
                let mut query = transaction.prepare(
                    "SELECT a.id FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
                     WHERE t.run_id = ?1 AND a.category = 'tool_execution'
                       AND a.status IN ('pending','approved') AND a.claimed_at IS NULL
                       AND t.status = 'pending' AND t.requires_approval = 1 AND t.execution_location = 'host'
                     ORDER BY t.runtime_tool_call_id, a.id",
                )?;
                let ids = query.query_map([run_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids
            };
            for approval_id in &approval_ids {
                match settle_unclaimed_approval_refusal_in_tx(&transaction, run_id, approval_id, &refusal, now)? {
                    ApprovalRefusalSettlement::Settled(_) | ApprovalRefusalSettlement::AlreadySettled(_) => {}
                    ApprovalRefusalSettlement::NotEligible => {
                        transaction.rollback()?;
                        return Err(projection_violation(
                            "runtime.cancel_unverifiable_approval: 无法验证待处理批准的结算条件，本次停止未生效，已保留原运行和批准状态".into(),
                        ));
                    }
                }
            }
            // The refusal settlement expires a pending approval; an explicit user
            // stop must read as cancelled, not a timeout. Only the generic stop
            // barrier is converted — a specific revocation reason is untouched.
            transaction.execute(
                "UPDATE approvals SET status = 'cancelled'
                 WHERE status = 'expired'
                   AND json_extract(decision_json, '$.invalidation.code') = 'runtime.user_cancelled'
                   AND tool_call_id IN (SELECT id FROM tool_calls WHERE run_id = ?1)",
                [run_id],
            )?;
            super::mark_run_interrupted_in_tx(
                &transaction, run_id, "runtime.user_cancelled", "用户已停止本次操作", now,
            )?;
            transaction.commit()?;
            Ok(true)
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
        if sweep_writer_route(transaction, &run_id)? != Some(WriterRoute::Kernel) { continue; }
        super::kernel_projection::expire_inactive_approvals(transaction, Some(&run_id), now)?;
        retire_kernel_run_row(transaction, &run_id, now)?;
        retired.push(run_id);
    }
    retire_delivery_review_kernel_runs(transaction, now)?;
    Ok(retired)
}

fn retire_kernel_run_row(transaction: &Transaction<'_>, run_id: &str, now: i64) -> rusqlite::Result<bool> {
    Ok(transaction.execute(
        "UPDATE runs SET status='interrupted', finished_at=?2, error_code=?3, error_message=?4
         WHERE id=?1 AND status IN ('queued','running','cancelling')",
        params![run_id, now, run_control::EXECUTOR_ROUTE_RETIRED, run_control::EXECUTOR_ROUTE_RETIRED_MESSAGE])? > 0)
}

/// B1 (A0 follow-up): a historical Kernel Run parked on
/// `awaiting_delivery_review` already reads `interrupted` in the desktop read
/// model, but it used to wait for a delivery decision that resumed the Kernel
/// loop. That loop is gone, so it is retired with the same code and message as
/// every other historical Kernel Run. Its Kernel facts stay untouched.
fn retire_delivery_review_kernel_runs(transaction: &Transaction<'_>, now: i64) -> rusqlite::Result<()> {
    let mut query = transaction.prepare(
        "SELECT r.id FROM runs r JOIN kernel_runs k ON k.run_id=r.id
         WHERE k.state='awaiting_delivery_review' AND r.status='interrupted'
           AND (r.error_code IS NULL OR r.error_code<>?1) ORDER BY r.id")?;
    let runs = query.query_map([run_control::EXECUTOR_ROUTE_RETIRED], |row| row.get::<_,String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    for run_id in runs {
        if sweep_writer_route(transaction, &run_id)? != Some(WriterRoute::Kernel) { continue; }
        super::kernel_projection::expire_inactive_approvals(transaction, Some(&run_id), now)?;
        transaction.execute(
            "UPDATE runs SET finished_at=COALESCE(finished_at, ?2), error_code=?3, error_message=?4
             WHERE id=?1 AND status='interrupted'",
            params![run_id, now, run_control::EXECUTOR_ROUTE_RETIRED, run_control::EXECUTOR_ROUTE_RETIRED_MESSAGE])?;
    }
    Ok(())
}

/// D1-3: startup pre-validation, run BEFORE `protect_restart_owners`. For every
/// unclaimed companion tool-execution approval on a still-running PiNative Run,
/// settle the ones that are provably invalid — a persisted Host invalidation, or
/// a failure of the strict original-snapshot grant gate — through the same D1-1
/// settlement the claim/waiter use. A settled refusal no longer counts as a
/// `waiting_approval` protection reason, so the Run is not kept alive for a dead
/// approval. Valid approved/unclaimed intents keep their original identity and
/// frozen facts and stay protected for native recovery; valid pending approvals
/// stay decidable. This is DB-only: no MCP/sidecar/working-directory gate runs
/// here, and a missing/corrupt companion snapshot is refused with a diagnosable
/// reason rather than falling back to the current grants. Base-whitelist tools
/// and non-companion Runs are untouched, and one invalid call never settles the
/// other legitimate approvals on the same Run.
pub(super) fn settle_invalid_restart_approvals(transaction: &Transaction<'_>, now: i64) -> rusqlite::Result<()> {
    let mut query = transaction.prepare(
        "SELECT a.id, t.run_id, t.tool_name, t.input_json, a.decision_json
         FROM approvals a
         JOIN tool_calls t ON t.id = a.tool_call_id
         JOIN runs r ON r.id = t.run_id
         WHERE a.category = 'tool_execution' AND a.status IN ('pending','approved') AND a.claimed_at IS NULL
           AND t.status = 'pending' AND t.requires_approval = 1 AND t.execution_location = 'host'
           AND r.status = 'running'
           AND EXISTS (
               SELECT 1 FROM conversations c WHERE c.id = t.conversation_id AND c.companion_id IS NOT NULL
               UNION ALL
               SELECT 1 FROM digital_colleagues dc WHERE dc.conversation_id = t.conversation_id AND dc.companion_bound = 1
           )",
    )?;
    let rows = query.query_map([], |row| Ok((
        row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
        row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?,
    )))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    for (approval_id, run_id, tool, input_json, decision_json) in rows {
        if sweep_writer_route(transaction, &run_id)? != Some(WriterRoute::PiNative) { continue; }
        // An archived/deleting companion (or a non-companion Run) is not this
        // gate's job; the generic restart repair and recovery own those.
        if !matches!(super::companion_ownership::resolve_run_on(transaction, &run_id), Ok(Some(_))) { continue; }
        let decision = decision_json.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok());
        let input = serde_json::from_str::<Value>(&input_json).ok();
        let refusal = match approval_invalidation(decision.as_ref()) {
            Some((code, message)) => Some(ApprovalRefusal::new(&code, message)),
            None => match super::companion_capabilities::ensure_granted_strict_on(transaction, &run_id, &tool, input.as_ref()) {
                Ok(()) => None,
                Err(message) => Some(ApprovalRefusal::new("companion.tool_not_available", message)),
            },
        };
        let Some(refusal) = refusal else { continue };
        register_approval_invalidation_in_tx(transaction, &approval_id, &refusal, now)?;
        let _ = settle_unclaimed_approval_refusal_in_tx(transaction, &run_id, &approval_id, &refusal, now)?;
    }
    Ok(())
}

pub(super) fn protect_restart_owners(transaction: &Transaction<'_>, now: i64, retired: &[String]) -> rusqlite::Result<()> {
    let mut query = transaction.prepare("SELECT run_id FROM run_control_bindings ORDER BY run_id")?;
    let runs = query.query_map([], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS fox_restart_protected_runs(run_id TEXT PRIMARY KEY); DELETE FROM fox_restart_protected_runs;")?;
    for run_id in runs {
        // An unverifiable binding is not protected. `interrupt_unverifiable_runs`
        // already interrupted that Run and settled its assistant messages, open
        // tool calls and pending approvals in the same transaction.
        let Some(route) = sweep_writer_route(transaction, &run_id)? else { continue };
        if route == WriterRoute::Kernel {
            super::kernel_projection::expire_inactive_approvals(transaction, Some(&run_id), now)?;
        }
        // A retired Kernel Run has no owner left to protect: its streaming
        // message, open tool calls and pending approvals settle with the
        // generic restart repair below.
        let retired_kernel = route == WriterRoute::Kernel && retired.contains(&run_id);
        // K2: a contact being archived or deleted never resumes execution after
        // a restart. Its waiting approval settles with the generic repair, so
        // the deletion sweep finds a terminal Run instead of a live owner.
        let companion_stopped: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM runs r
                 JOIN companion_profiles p ON p.id = COALESCE(
                     (SELECT c.companion_id FROM conversations c WHERE c.id = r.conversation_id),
                     (SELECT dc.companion_id FROM digital_colleagues dc
                      WHERE dc.conversation_id = r.conversation_id AND dc.companion_bound = 1))
                 WHERE r.id = ?1 AND (p.archived_at IS NOT NULL OR p.deleting_at IS NOT NULL))",
            [&run_id],
            |row| row.get(0),
        )?;
        let native_waiting = !companion_stopped
            && route == WriterRoute::PiNative
            && waiting_approval(transaction, &run_id)?;
        if native_waiting {
            let scope = super::companion_ownership::resolve_run_on(transaction, &run_id);
            if scope.is_err() || scope.is_ok_and(|scope| scope.is_some_and(|scope| scope.is_child)) {
                // A companion child cannot recover an approval without its parent's workspace slot.
                let active: bool = transaction.query_row(
                    "SELECT status IN ('queued', 'running', 'cancelling') FROM runs WHERE id = ?1",
                    [&run_id],
                    |row| row.get(0),
                )?;
                if active {
                    super::mark_run_interrupted_in_tx(
                        transaction,
                        &run_id,
                        "runtime.application_restarted",
                        "Fox closed before the run reached a terminal state.",
                        now,
                    )?;
                }
                continue;
            }
        }
        if !companion_stopped && ((route == WriterRoute::Kernel && !retired_kernel) || native_waiting) {
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
    Ok(())
}
