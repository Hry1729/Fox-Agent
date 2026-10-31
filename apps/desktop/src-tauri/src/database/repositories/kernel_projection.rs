//! Compatibility read models for the existing desktop. These are committed in
//! the same transaction as Kernel facts and are never an execution authority.
use rusqlite::{params, Transaction};

/// Projection only: hook policy was evaluated from this immutable scope before
/// dispatch. Deterministic audit keys prevent duplicates on any later commit.
/// Compatibility tickets do not grant authority once their owning work settled.
/// Includes replacement tickets whose Kernel approval row has already gone away.
pub(super) fn expire_inactive_approvals(
    tx: &Transaction<'_>,
    run_id: Option<&str>,
    now: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE approvals SET status='expired',
            decision_json=json_object('approved',json('false'),'reason','owner_settled','authority','kernel'),
            resolved_at=COALESCE(resolved_at,?2)
         WHERE status='pending' AND EXISTS (
             SELECT 1 FROM tool_calls t JOIN kernel_runs k ON k.run_id=t.run_id
             JOIN run_control_bindings b ON b.run_id=t.run_id
             WHERE t.id=approvals.tool_call_id AND (?1 IS NULL OR t.run_id=?1)
                 AND k.kernel_mode='authoritative' AND b.authority='authoritative'
                 AND (k.terminal_written=1 OR t.status IN ('completed','failed','cancelled','interrupted','denied','expired')))",
        params![run_id, now],
    )
}
