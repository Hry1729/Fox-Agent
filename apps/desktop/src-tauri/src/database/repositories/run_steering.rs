//! Persistent mid-run additional user requests ("steering").
//!
//! State machine, durable at every step so reopening the application never
//! loses or duplicates a message:
//!
//! ```text
//! received ──dispatch armed──▶ delivered ──round response committed──▶ applied
//!    │                             │
//!    └───────── run terminal ──────┴──────────────────────────────▶ cancelled
//! ```
//!
//! * `received` — durably accepted, not part of any model dispatch yet.
//! * `delivered` — included in the bytes of an armed model dispatch (frame or
//!   round directive); the dispatch may still be retried after a worker death,
//!   in which case the replacement dispatch includes the same rows again.
//! * `applied` — a model round responding to that dispatch is committed. The
//!   message text now also lives inside the durable engine history/checkpoint.
//! * `cancelled` — the Run ended before a response; terminal history.
//!
//! A frozen, already-sent request is never rewritten: rows are attached when
//! the NEXT model input is assembled, and their status flips inside the same
//! transaction that persists that dispatch decision.

use super::*;

/// Business-rule failures raised inside a rusqlite closure travel as sqlite
/// errors (same convention as `kernel_err`); the outer `String` API surface is
/// unaffected.
fn steering_err(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        message.into(),
    )))
}

/// One additional request stays well below the model frame reserve so the
/// frozen dispatch's context budget accounting stays meaningful.
pub(crate) const MAX_STEERING_CONTENT_CHARS: usize = 8_000;
/// Bound the total outstanding (received + delivered) bytes per Run so queued
/// text cannot exhaust the frozen context window between boundaries.
pub(crate) const MAX_PENDING_STEERING_BYTES: usize = 64 * 1024;
/// Bounded number of follow-up rounds one Run dedicates to additional user
/// input. This is the *acceptance* budget too: once they are spent the Run can
/// no longer plan a `steering` round, so the receipt boundary refuses a new
/// request instead of accepting text that could never be answered (which would
/// otherwise surface later as an unexplainable execution failure).
pub(crate) const MAX_STEERING_FOLLOWUPS: i64 = 4;

/// Newly armed steering rows attached to a live round directive. Response
/// commits themselves always settle previously `delivered` rows (applied), so
/// only the directive side is carried here.
#[derive(Debug, Clone, Default)]
pub(crate) struct SteeringDecision {
    /// These still-`received` rows are handed to the next dispatch armed by
    /// this write-set (a live round directive): flip them to `delivered`.
    pub deliver_seqs: Vec<i64>,
    /// Durable identity of that next dispatch, for audit/rebuild.
    pub dispatch_key: String,
    /// Bind **every** still-`received` row to that dispatch, not only the listed
    /// ones.
    ///
    /// Set whenever the decision arms a directive that carries the notices: a
    /// request accepted between the caller's queue read and this write-set is
    /// then still delivered with the round that answers it. Without this, such a
    /// request makes the whole decision fail closed — and for a *terminal*
    /// decision that would mean losing an accepted request or failing the Run
    /// for a race the user cannot see. Never set for a decision that arms no
    /// directive: binding a row to a round that never asks the model about it
    /// would silently drop it.
    pub adopt_all_received: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SteeringMessage {
    pub run_id: String,
    pub seq: i64,
    pub message_id: String,
    pub content: String,
    pub status: String,
    pub received_at: i64,
    pub applied_at: Option<i64>,
    pub applied_event_seq: Option<i64>,
    pub applied_dispatch_key: Option<String>,
}

impl Database {
    /// Newest authoritative active Run bound to a conversation, if any.
    /// Steering commands resolve the Run through this ownership join; the
    /// caller never supplies a run id from the client directly.
    pub(crate) fn active_kernel_run_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT r.run_id FROM kernel_runs r
                 JOIN run_control_bindings b ON b.run_id = r.run_id
                 WHERE b.conversation_id = ?1
                   AND r.state IN ('created','running','retry_scheduled')
                   AND r.kernel_mode = 'authoritative'
                   AND b.authority = 'authoritative'
                 ORDER BY r.created_at DESC, r.run_id DESC
                 LIMIT 1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })
    }

    /// Newest authoritative Run bound to a conversation, whether it is still
    /// active or already terminal. Read-only history uses this: after a task
    /// ends (or the app is reopened) the conversation still owns the rows that
    /// record what happened to each additional request, and a lookup failure
    /// must not make them look like they never existed.
    ///
    /// Enqueueing uses [`Database::active_kernel_run_for_conversation`] instead,
    /// so history can never widen the set of Runs that accept input.
    pub(crate) fn latest_kernel_run_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT r.run_id, r.state FROM kernel_runs r
                 JOIN run_control_bindings b ON b.run_id = r.run_id
                 WHERE b.conversation_id = ?1
                   AND r.kernel_mode = 'authoritative'
                   AND b.authority = 'authoritative'
                 ORDER BY r.created_at DESC, r.run_id DESC
                 LIMIT 1",
                params![conversation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
        })
    }

    /// Durably accept one additional request for an active Run. A repeated
    /// `message_id` is an idempotent no-op returning the existing seq. Fails
    /// once the Run is terminal or when the bounded queue is full.
    pub(crate) fn enqueue_run_steering(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        now: i64,
    ) -> Result<i64, String> {
        let content = content.trim();
        if message_id.trim().is_empty() {
            return Err("steering message id is empty".into());
        }
        if content.is_empty() {
            return Err("steering content is empty".into());
        }
        if content.chars().count() > MAX_STEERING_CONTENT_CHARS {
            return Err(format!(
                "steering content exceeds {MAX_STEERING_CONTENT_CHARS} characters"
            ));
        }
        // Byte accounting is UTF-8 bytes in BOTH places: the value added here and
        // the value summed from durable rows. SQLite's `length()` on TEXT counts
        // characters, so the authoritative sum below uses its own byte length
        // function; comparing characters in SQL against bytes in Rust let a queue
        // of Chinese text exceed the byte cap threefold.
        if content.len() > MAX_PENDING_STEERING_BYTES {
            return Err(format!(
                "steering content exceeds {MAX_PENDING_STEERING_BYTES} bytes"
            ));
        }
        self.with_connection(|conn| {
            let active: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM kernel_runs
                  WHERE run_id=?1 AND state IN ('created','running','retry_scheduled'))",
                params![run_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(steering_err(
                    "steering is only accepted while the Run is active",
                ));
            }
            // The additional-input budget is part of the acceptance decision: a
            // Run that has already spent every steering round cannot answer new
            // text, so accepting it would produce a request the user believes was
            // received and that later turns into an unexplained failure. Refuse
            // it here, at the receipt boundary, with an explainable reason.
            let rounds_used: i64 = conn.query_row(
                "SELECT COUNT(*) FROM kernel_events e JOIN kernel_runs r ON r.run_id=e.run_id
                  WHERE e.run_id=?1 AND e.event_type='engine.continuation_requested'
                    AND r.kernel_mode='authoritative'
                    AND json_extract(e.payload_json,'$.lane')='steering'",
                params![run_id],
                |row| row.get(0),
            )?;
            if rounds_used >= MAX_STEERING_FOLLOWUPS {
                return Err(steering_err(format!(
                    "this Run has used all {MAX_STEERING_FOLLOWUPS} additional-request rounds; \
                     submit the request as a new task instead"
                )));
            }
            // The queue cap is enforced at this authoritative acceptance
            // boundary, in the same transaction as the insert, so two concurrent
            // accepts cannot both observe room for the same bytes.
            let outstanding_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(octet_length(content)),0) FROM run_steering_messages
                  WHERE run_id=?1 AND status IN ('received','delivered')",
                params![run_id],
                |row| row.get(0),
            )?;
            let incoming = content.len() as i64;
            if outstanding_bytes.saturating_add(incoming) > MAX_PENDING_STEERING_BYTES as i64 {
                return Err(steering_err(
                    "pending steering queue is full; wait for the Run to apply earlier requests",
                ));
            }
            let seq = conn.query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM run_steering_messages WHERE run_id=?1",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO run_steering_messages(
                    run_id, seq, message_id, content, status, received_at)
                VALUES (?1, ?2, ?3, ?4, 'received', ?5)",
                params![run_id, seq, message_id, content, now],
            )?;
            let effective = conn.query_row(
                "SELECT seq FROM run_steering_messages WHERE message_id=?1",
                params![message_id],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(effective)
        })
    }

    pub(crate) fn pending_run_steering(&self, run_id: &str) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, seq, message_id, content, status, received_at,
                        applied_at, applied_event_seq, applied_dispatch_key
                 FROM run_steering_messages
                 WHERE run_id=?1 AND status='received' ORDER BY seq",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_steering)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Every message that must be present in a rebuilt dispatch: not-yet-sent
    /// rows plus rows already delivered to an earlier attempt of this same
    /// dispatch whose response never committed.
    pub(crate) fn active_run_steering(&self, run_id: &str) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, seq, message_id, content, status, received_at,
                        applied_at, applied_event_seq, applied_dispatch_key
                 FROM run_steering_messages
                 WHERE run_id=?1 AND status IN ('received','delivered') ORDER BY seq",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_steering)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Rows already carried by the in-flight dispatch whose response is being
    /// committed. They are appended to the durable history reconstruction in
    /// seq order, exactly as the live engine saw them.
    pub(crate) fn delivered_run_steering(&self, run_id: &str) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, seq, message_id, content, status, received_at,
                        applied_at, applied_event_seq, applied_dispatch_key
                 FROM run_steering_messages
                 WHERE run_id=?1 AND status='delivered' ORDER BY seq",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_steering)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Rows durably bound to one dispatch, in input order.
    ///
    /// Read after a decision commits: the directive and the next round's history
    /// must describe exactly what was delivered, including a request that
    /// arrived inside the decision window and was adopted by that write-set.
    pub(crate) fn bound_run_steering(
        &self,
        run_id: &str,
        dispatch_key: &str,
    ) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| Ok(bound_steering_rows(conn, run_id, dispatch_key)?))
    }

    pub(crate) fn run_steering_messages(&self, run_id: &str) -> Result<Vec<SteeringMessage>, String> {        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, seq, message_id, content, status, received_at,
                        applied_at, applied_event_seq, applied_dispatch_key
                 FROM run_steering_messages WHERE run_id=?1 ORDER BY seq",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_steering)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Atomically collect the rows for a dispatch rebuild and bind every
    /// still-`received` row to that dispatch as `delivered`. Rows already
    /// delivered by a failed attempt are returned unchanged, so retrying the
    /// same dispatch rebuilds byte-identical steering content exactly once.
    pub(crate) fn deliver_steering_for_dispatch(
        &self,
        run_id: &str,
        dispatch_key: &str,
    ) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            let mut rows = steering_rows(&tx, run_id)?;
            let received: Vec<i64> = rows
                .iter()
                .filter(|row| row.status == "received")
                .map(|row| row.seq)
                .collect();
            for seq in &received {
                let changed = tx.execute(
                    "UPDATE run_steering_messages
                        SET status='delivered', applied_at=NULL,
                            applied_dispatch_key=?3
                      WHERE run_id=?1 AND seq=?2 AND status='received'",
                    params![run_id, seq, dispatch_key],
                )?;
                if changed != 1 {
                    return Err(steering_err(
                        "steering delivery raced a concurrent decision",
                    ));
                }
            }
            tx.commit()?;
            // The rows were read before the flips; project the committed state
            // onto the returned view so callers never observe a row the
            // transaction has already bound as still `received`. Rows already
            // delivered by an earlier attempt keep their original dispatch key.
            for row in rows.iter_mut() {
                if received.contains(&row.seq) {
                    row.status = "delivered".to_owned();
                    row.applied_dispatch_key = Some(dispatch_key.to_owned());
                }
            }
            Ok(rows)
        })
    }
}

/// Round-response commit: all `delivered` rows are now answered by the round
/// in this write-set, so they become `applied` history. Runs inside the
/// decision transaction so the status can never disagree with the committed
/// response.
pub(crate) fn apply_delivered_steering_in_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    now: i64,
    event_seq: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE run_steering_messages
            SET status='applied', applied_at=?3, applied_event_seq=?4
          WHERE run_id=?1 AND status='delivered'",
        params![run_id, now, now, event_seq],
    )?;
    Ok(())
}

/// Bind exactly these `received` rows to the dispatch armed by this write-set
/// (a live round directive). The CAS on status means a concurrent terminal
/// decision cancelling the queue wins and the whole decision fails closed.
pub(crate) fn deliver_steering_in_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    seqs: &[i64],
    dispatch_key: &str,
    event_seq: i64,
) -> rusqlite::Result<(), String> {
    for seq in seqs {
        let changed = tx
            .execute(
                "UPDATE run_steering_messages
                    SET status='delivered', applied_at=NULL,
                        applied_event_seq=?4, applied_dispatch_key=?3
                  WHERE run_id=?1 AND seq=?2 AND status='received'",
                params![run_id, seq, dispatch_key, event_seq],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err(format!("steering seq {seq} is no longer received"));
        }
    }
    Ok(())
}

/// Bind every still-`received` row of the Run to this dispatch.
///
/// This is the race-closing twin of [`deliver_steering_in_tx`]: instead of
/// naming the rows the caller had seen, it binds whatever is `received` at
/// commit time, so a request accepted in the window between the caller's read
/// and this write-set is delivered with the round that answers it rather than
/// making the decision fail. Returns how many rows were bound.
pub(crate) fn adopt_received_steering_in_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    dispatch_key: &str,
    event_seq: i64,
) -> rusqlite::Result<i64> {
    let bound = tx.execute(
        "UPDATE run_steering_messages
            SET status='delivered', applied_at=NULL,
                applied_event_seq=?3, applied_dispatch_key=?2
          WHERE run_id=?1 AND status='received'",
        params![run_id, dispatch_key, event_seq],
    )?;
    Ok(bound as i64)
}

/// Rows bound to one dispatch, in input order. Read after a commit so the
/// directive and the next round's history describe exactly the rows that were
/// durably delivered — including any that arrived inside the decision window.
pub(crate) fn bound_steering_rows(
    connection: &rusqlite::Connection,
    run_id: &str,
    dispatch_key: &str,
) -> rusqlite::Result<Vec<SteeringMessage>> {
    let mut stmt = connection.prepare(
        "SELECT run_id, seq, message_id, content, status, received_at,
                applied_at, applied_event_seq, applied_dispatch_key
         FROM run_steering_messages
         WHERE run_id=?1 AND status='delivered' AND applied_dispatch_key=?2
         ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![run_id, dispatch_key], row_to_steering)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Terminal decisions cancel every open message: queued text never reaches a
/// model and text in flight dies with the aborted dispatch.
pub(crate) fn cancel_open_steering_in_tx(    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE run_steering_messages
            SET status='cancelled'
          WHERE run_id=?1 AND status IN ('received','delivered')",
        params![run_id],
    )?;
    Ok(())
}

fn steering_rows(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
) -> rusqlite::Result<Vec<SteeringMessage>> {
    let mut stmt = tx.prepare(
        "SELECT run_id, seq, message_id, content, status, received_at,
                applied_at, applied_event_seq, applied_dispatch_key
         FROM run_steering_messages
         WHERE run_id=?1 AND status IN ('received','delivered') ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![run_id], row_to_steering)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_to_steering(row: &rusqlite::Row<'_>) -> rusqlite::Result<SteeringMessage> {
    Ok(SteeringMessage {
        run_id: row.get(0)?,
        seq: row.get(1)?,
        message_id: row.get(2)?,
        content: row.get(3)?,
        status: row.get(4)?,
        received_at: row.get(5)?,
        applied_at: row.get(6)?,
        applied_event_seq: row.get(7)?,
        applied_dispatch_key: row.get(8)?,
    })
}
