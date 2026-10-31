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
//!
//! # Lanes
//!
//! Every row belongs to exactly one lane, recorded at receipt:
//!
//! * [`STEERING_LANE_CURRENT`] — "补充到当前任务". The row is bound to the active
//!   Run's next dispatch boundary and walks the state machine above.
//! * [`STEERING_LANE_NEXT_TURN`] — "排队，下一轮处理". The row is durably accepted
//!   but bound to **no** dispatch of the active Run: it stays `received` while
//!   the Run is live, is excluded from every model-input read and from the
//!   Run's follow-up budget, and is not cancelled by the Run's terminal
//!   decision (the Run never promised to answer it). It waits for the next task
//!   the user starts in the conversation.

use super::*;

/// Lane for input delivered at the active Run's next dispatch boundary.
pub(crate) const STEERING_LANE_CURRENT: &str = "current";
/// Lane for input held for the conversation's next task instead.
pub(crate) const STEERING_LANE_NEXT_TURN: &str = "next-turn";

pub(crate) fn is_steering_lane(value: &str) -> bool {
    value == STEERING_LANE_CURRENT || value == STEERING_LANE_NEXT_TURN
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SteeringMessage {
    pub run_id: String,
    pub seq: i64,
    pub message_id: String,
    pub content: String,
    pub status: String,
    pub lane: String,
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

    /// Durably accept one additional request on an explicit lane.
    ///
    /// Only [`STEERING_LANE_CURRENT`] spends the Run's additional-input budget:
    /// a `next-turn` row is never handed to this Run, so it neither consumes nor
    /// is bounded by that Run's follow-up rounds. The queue byte cap is enforced
    /// per lane — queued `next-turn` text cannot exhaust the frozen context
    /// window of the active dispatch, and vice versa.
    pub(crate) fn enqueue_run_steering_on_lane(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        lane: &str,
        now: i64,
    ) -> Result<i64, String> {
        let content = content.trim();
        if message_id.trim().is_empty() {
            return Err("steering message id is empty".into());
        }
        if !is_steering_lane(lane) {
            return Err(format!("unknown steering lane: {lane}"));
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
            // `next-turn` rows cost this Run nothing and are never refused here.
            if lane == STEERING_LANE_CURRENT {
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
            }
            // The queue cap is enforced at this authoritative acceptance
            // boundary, in the same transaction as the insert, so two concurrent
            // accepts cannot both observe room for the same bytes.
            let outstanding_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(octet_length(content)),0) FROM run_steering_messages
                  WHERE run_id=?1 AND lane=?2 AND status IN ('received','delivered')",
                params![run_id, lane],
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
                    run_id, seq, message_id, content, status, lane, received_at)
                VALUES (?1, ?2, ?3, ?4, 'received', ?5, ?6)",
                params![run_id, seq, message_id, content, lane, now],
            )?;
            let effective = conn.query_row(
                "SELECT seq FROM run_steering_messages WHERE message_id=?1",
                params![message_id],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(effective)
        })
    }

    /// Every still-undelivered queued row of a conversation, whichever
    /// authoritative Run received it.
    ///
    /// The queue outlives the Run that accepted it — including across the start
    /// of the next task, which is a *new* Run. Scoping the read to one run id
    /// would make a queue the user is still waiting on invisible as soon as the
    /// next task begins, which is the silent loss this lane exists to prevent.
    pub(crate) fn queued_next_turn_steering_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&steering_select(
                "WHERE lane=?2 AND status='received' AND run_id IN (
                     SELECT r.run_id FROM kernel_runs r
                     JOIN run_control_bindings b ON b.run_id = r.run_id
                     WHERE b.conversation_id = ?1
                       AND r.kernel_mode = 'authoritative'
                       AND b.authority = 'authoritative')
                 ORDER BY received_at, run_id, seq",
            ))?;
            let rows = stmt
                .query_map(
                    params![conversation_id, STEERING_LANE_NEXT_TURN],
                    row_to_steering,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Remove one queued row of a conversation, whichever authoritative Run
    /// received it. Same narrowness as [`Database::discard_run_steering`].
    pub(crate) fn discard_queued_next_turn_steering_for_conversation(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<bool, String> {
        self.with_connection(|conn| {
            let removed = conn.execute(
                "DELETE FROM run_steering_messages
                  WHERE message_id=?2 AND lane=?3 AND status='received'
                    AND run_id IN (
                      SELECT r.run_id FROM kernel_runs r
                      JOIN run_control_bindings b ON b.run_id = r.run_id
                      WHERE b.conversation_id = ?1
                        AND r.kernel_mode = 'authoritative'
                        AND b.authority = 'authoritative')",
                params![conversation_id, message_id, STEERING_LANE_NEXT_TURN],
            )?;
            Ok(removed == 1)
        })
    }

    pub(crate) fn run_steering_messages(&self, run_id: &str) -> Result<Vec<SteeringMessage>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&steering_select("WHERE run_id=?1 ORDER BY seq"))?;
            let rows = stmt
                .query_map(params![run_id], row_to_steering)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

}

/// One projection of the steering table for every reader in this module, so a
/// new column can never be added to some queries and forgotten in others.
fn steering_select(tail: &str) -> String {
    format!(
        "SELECT run_id, seq, message_id, content, status, lane, received_at,
                applied_at, applied_event_seq, applied_dispatch_key
         FROM run_steering_messages {tail}"
    )
}

fn row_to_steering(row: &rusqlite::Row<'_>) -> rusqlite::Result<SteeringMessage> {
    Ok(SteeringMessage {
        run_id: row.get(0)?,
        seq: row.get(1)?,
        message_id: row.get(2)?,
        content: row.get(3)?,
        status: row.get(4)?,
        lane: row.get(5)?,
        received_at: row.get(6)?,
        applied_at: row.get(7)?,
        applied_event_seq: row.get(8)?,
        applied_dispatch_key: row.get(9)?,
    })
}
