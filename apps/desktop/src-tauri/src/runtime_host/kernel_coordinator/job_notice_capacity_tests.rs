//! Capacity boundary of the typed Host notice lane (C2 / C2-R1).
//!
//! Evidence tier: **component/coordinator** for the packing, binding,
//! acknowledgement and response-planning cases. The last test additionally
//! drives the **real RuntimeHost** drive loop with the real Node/Pi worker on
//! both transports and a bounded local mock OpenAI-compatible Provider. There is
//! no real Provider, and the notices of that test are manufactured through
//! database interfaces rather than produced by a compute execution, so none of
//! this is a real Host/Node/Pi end-to-end acceptance.
//!
//! Every capacity boundary is measured in the serialized bytes the production
//! packer itself counts — never in characters and never from a slack guess.
use super::*;
use super::host_job_notice_tests::{enable_notices, failed_compute, notice_host_fixture, stop_response};
use crate::database::JobStartRequest;

/// The batch budget the production planner uses
/// (`(16 * 1024).saturating_sub(historical)` in `job_notice_followup_input`).
const NOTICE_BUDGET: usize = 16 * 1024;
/// The per-notice bound enforced by `HostJobNotice::validate`.
const NOTICE_MAX: usize = 2_048;

/// Serialized bytes of one pending notice, taken from the production packer's
/// own returned fact.
fn packed_bytes(db: &Database, conversation: &str, run: &str, job: &str) -> usize {
    let facts = db.pending_host_job_notices(conversation, run, NOTICE_BUDGET).unwrap();
    let fact = facts.iter().find(|fact| fact.job_id == job)
        .unwrap_or_else(|| panic!("notice {job} is not inside the packed prefix"));
    serde_json::to_vec(fact).unwrap().len()
}

/// Cumulative serialized byte offsets of the first `count` pending notices,
/// measured through the production packer itself:
/// `pending_host_job_notices(max)` returns the longest prefix whose byte sum is
/// `<= max`, so the smallest `max` that first contains notice `i` is the exact
/// byte sum through notice `i`. This is what keeps the boundaries byte-exact.
fn packed_offsets(db: &Database, conversation: &str, run: &str, count: usize) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(count);
    let mut floor = 0usize;
    for index in 0..count {
        let (mut low, mut high) = (floor, NOTICE_BUDGET);
        while low < high {
            let mid = low + (high - low) / 2;
            if db.pending_host_job_notices(conversation, run, mid).unwrap().len() > index {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        assert!(low <= NOTICE_BUDGET, "pending notice {index} never fits the notice budget");
        offsets.push(low);
        floor = low;
    }
    offsets
}

/// A failed compute Job whose notice serializes to `bytes`: `errorCode` is the
/// only variable-length field, so its length is derived from the measured
/// header. The size is then verified against the packer's own bytes.
fn sized_notice(db: &Database, conversation: &str, run: &str, key: &str, header: usize,
    bytes: usize) -> String {
    let job = unverified_notice(db, run, key, header, bytes);
    assert_eq!(packed_bytes(db, conversation, run, &job), bytes,
        "notice {key} must serialize to exactly {bytes} bytes");
    job
}

/// Same shape without the measurement. Used only for notices that are already
/// known not to fit (the packer cannot return them, so they cannot be measured
/// through it); their non-fit is asserted from the packer's own prefix length.
fn unverified_notice(db: &Database, run: &str, key: &str, header: usize, bytes: usize) -> String {
    assert!(bytes > header + 1, "notice {key} cannot reach {bytes} bytes");
    assert!(bytes <= NOTICE_MAX, "notice {key} would break the per-notice bound");
    failed_compute(db, run, key, &"c".repeat(bytes - header))
}

/// Fill one Run's pending set to exactly `total` serialized bytes: `probe` is
/// already `probe_bytes`, the middle notices are the per-notice maximum and the
/// last one absorbs the remainder.
fn fill_to_total(db: &Database, conversation: &str, run: &str, tag: &str, header: usize,
    probe: &str, probe_bytes: usize, total: usize) -> Vec<String> {
    let mut jobs = vec![probe.to_owned()];
    let mut remaining = total - probe_bytes;
    let mut index = 0usize;
    while remaining > NOTICE_MAX {
        jobs.push(sized_notice(db, conversation, run, &format!("{tag}-{index}"), header, NOTICE_MAX));
        remaining -= NOTICE_MAX;
        index += 1;
    }
    assert!(remaining > header + 1, "the remainder {remaining} cannot form a notice");
    jobs.push(sized_notice(db, conversation, run, &format!("{tag}-last"), header, remaining));
    jobs
}

/// The durable `(finished_at, job_id)` order the packer must follow.
fn durable_notice_order(db: &Database, run: &str) -> Vec<String> {
    db.with_connection(|conn| {
        let mut statement = conn.prepare(
            "SELECT job_id FROM kernel_job_notices WHERE run_id=?1 ORDER BY finished_at,job_id")?;
        let rows = statement.query_map([run], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).unwrap()
}

/// `(delivery rows, bound, acknowledged, notices with no row at all)`.
fn delivery_counts(db: &Database, run: &str) -> (i64, i64, i64, i64) {
    db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                   JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1),
                (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                   JOIN kernel_job_notices n ON n.job_id=d.job_id
                  WHERE n.run_id=?1 AND d.state='bound'),
                (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                   JOIN kernel_job_notices n ON n.job_id=d.job_id
                  WHERE n.run_id=?1 AND d.state='acknowledged'),
                (SELECT COUNT(*) FROM kernel_job_notices n
                   LEFT JOIN kernel_job_notice_deliveries d ON d.job_id=n.job_id
                  WHERE n.run_id=?1 AND d.job_id IS NULL)",
        [run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap()
}

/// Notices that carry more than one delivery row: a notice must never be
/// delivered twice.
fn duplicate_deliveries(db: &Database, run: &str) -> i64 {
    db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM (SELECT d.job_id FROM kernel_job_notice_deliveries d
             JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1
             GROUP BY d.job_id HAVING COUNT(*)>1)",
        [run], |row| row.get(0))).unwrap()
}

fn notice_events(db: &Database, run: &str) -> (i64, i64) {
    db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1
                   AND event_type='engine.continuation_requested'
                   AND json_extract(payload_json,'$.lane')='job_notice')",
        [run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap()
}

/// Case 1: the available capacity holds the batch **exactly**. Every byte of
/// the budget is used, the whole batch reaches the model in durable order, all
/// of it is acknowledged, and nothing is lost or duplicated.
#[test]
fn capacity_exact_fit_delivers_acknowledges_and_completes_without_loss_or_duplicate() {
    let (db, _root, run, clock, cancellation, conversation) = notice_host_fixture("c2-exact");
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();

    let probe = failed_compute(&db, &run, "c2-exact-probe", &"p".repeat(32));
    let probe_bytes = packed_bytes(&db, &conversation, &run, &probe);
    let header = probe_bytes - 32;
    let jobs = fill_to_total(&db, &conversation, &run, "c2-exact", header, &probe, probe_bytes,
        NOTICE_BUDGET);

    // Byte-exact: the measured byte sum through the last notice is exactly the
    // whole budget, and one byte less drops that notice.
    let offsets = packed_offsets(&db, &conversation, &run, jobs.len());
    assert_eq!(offsets.last().copied(), Some(NOTICE_BUDGET),
        "the batch must use the notice budget exactly, byte for byte");
    let order = durable_notice_order(&db, &run);
    assert_eq!(order.len(), jobs.len());
    assert_eq!(db.pending_host_job_notices(&conversation, &run, NOTICE_BUDGET).unwrap().len(),
        jobs.len(), "an exactly fitting batch must be delivered whole");
    assert_eq!(db.pending_host_job_notices(&conversation, &run, NOTICE_BUDGET - 1).unwrap().len(),
        jobs.len() - 1, "one byte less must drop exactly the last notice");

    // Real response-planning entry: everything fits, so the Final is legal.
    coordinator.dispatch_initial("c2-exact-owner", &Allow, |binding, frame, _| {
        let delivered: Vec<String> =
            frame.host_job_notices.iter().map(|notice| notice.job_id.clone()).collect();
        assert_eq!(delivered, order, "the frame must carry the durable byte order");
        let bytes: usize = frame.host_job_notices.iter()
            .map(|notice| serde_json::to_vec(notice).unwrap().len()).sum();
        assert_eq!(bytes, NOTICE_BUDGET, "the delivered batch must be exactly the whole budget");
        Ok(stop_response(binding, frame))
    }).expect("a batch that fits exactly must be deliverable and completable");

    let (rows, bound, acknowledged, undelivered) = delivery_counts(&db, &run);
    assert_eq!((rows, bound, acknowledged, undelivered),
        (jobs.len() as i64, 0, jobs.len() as i64, 0),
        "every notice must have exactly one acknowledged delivery row");
    assert_eq!(duplicate_deliveries(&db, &run), 0, "no notice may be delivered twice");
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let (wakes, intents) = notice_events(&db, &run);
    assert_eq!((wakes, intents), (0, 0), "a delivered batch must not arm a second notice lane");
    println!("c2 exact fit: {} notices, {} bytes, deliveries={rows} acknowledged={acknowledged} \
state=completed", jobs.len(), NOTICE_BUDGET);
}

/// Case 2: the current batch only holds a **prefix**; the remainder stays
/// undelivered. The prefix is the maximal byte prefix, bound and never
/// acknowledged or re-delivered, and the response-planning entry refuses to
/// complete the Run.
#[test]
fn capacity_prefix_is_maximal_and_the_remainder_is_not_bound_acked_or_deleted() {
    let (db, _root, run, clock, cancellation, conversation) = notice_host_fixture("c2-prefix");
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();

    let probe = failed_compute(&db, &run, "c2-prefix-probe", &"p".repeat(32));
    let probe_bytes = packed_bytes(&db, &conversation, &run, &probe);
    let header = probe_bytes - 32;
    // The prefix fills the whole budget exactly ...
    let jobs = fill_to_total(&db, &conversation, &run, "c2-prefix", header, &probe, probe_bytes,
        NOTICE_BUDGET);
    let offsets = packed_offsets(&db, &conversation, &run, jobs.len());
    assert_eq!(offsets.last().copied(), Some(NOTICE_BUDGET));
    // ... so any further notice cannot fit, whatever its size.
    let extra: Vec<String> = (0..4)
        .map(|index| unverified_notice(&db, &run, &format!("c2-prefix-extra-{index}"), header,
            NOTICE_MAX))
        .collect();
    let order = durable_notice_order(&db, &run);
    assert_eq!(order.len(), jobs.len() + extra.len());
    assert_eq!(db.pending_host_job_notices(&conversation, &run, NOTICE_BUDGET).unwrap().len(),
        jobs.len(), "the prefix must stay the maximal fitting prefix");
    let remainder: Vec<String> = order.iter().filter(|job| !jobs.contains(*job)).cloned().collect();
    assert_eq!(remainder.len(), extra.len());

    let state_before = db.kernel_host_run_state(&run).unwrap();
    let outcome = coordinator.dispatch_initial("c2-prefix-owner", &Allow, |binding, frame, _| {
        let delivered: Vec<String> =
            frame.host_job_notices.iter().map(|notice| notice.job_id.clone()).collect();
        assert_eq!(delivered, order[..jobs.len()].to_vec(),
            "the frame must carry exactly the maximal byte prefix in durable order");
        Ok(stop_response(binding, frame))
    });
    let error = outcome.expect_err("a Final with an undeliverable remainder must be refused");
    assert!(error.contains("kernel.job_notice_competition"),
        "unexpected planning outcome: {error}");
    assert!(error.contains("pending Host notices arrived at Final"),
        "unexpected planning outcome: {error}");
    println!("c2 prefix remainder planning outcome: {error}");

    // The prefix is bound but not acknowledged (the refused Final rolled back),
    // and the remainder has no delivery row at all: not bound, not acknowledged,
    // not deleted.
    let (rows, bound, acknowledged, undelivered) = delivery_counts(&db, &run);
    assert_eq!((rows, bound, acknowledged, undelivered),
        (jobs.len() as i64, jobs.len() as i64, 0, remainder.len() as i64),
        "only the packed prefix may be bound; the remainder must stay untouched");
    let pending: Vec<String> = db.with_connection(|conn| {
        let mut statement = conn.prepare(
            "SELECT n.job_id FROM kernel_job_notices n
             LEFT JOIN kernel_job_notice_deliveries d ON d.job_id=n.job_id
             WHERE n.run_id=?1 AND d.job_id IS NULL ORDER BY n.finished_at,n.job_id")?;
        let rows = statement.query_map([run.as_str()], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).unwrap();
    assert_eq!(pending, remainder, "every durable notice must still exist exactly once");
    assert_eq!(duplicate_deliveries(&db, &run), 0, "no notice may be delivered twice");
    assert_eq!(db.kernel_host_run_state(&run).unwrap(), state_before,
        "a refused Final must leave the Run exactly where it was");
    let (wakes, intents) = notice_events(&db, &run);
    assert_eq!((wakes, intents), (0, 0), "no park existed, so no wake may be recorded");
    println!("c2 prefix remainder: prefix={} bound, remainder={} undelivered, state={}",
        jobs.len(), remainder.len(), coordinator.snapshot().unwrap().state);
}

/// Case 3: historical markers consume the capacity, so the next notice cannot
/// be packed at all. The park/wake entry reports the capacity error, keeps the
/// park, and neither drops nor re-delivers anything.
#[test]
fn capacity_exhausted_by_historical_markers_blocks_the_wake_and_keeps_every_fact() {
    let (db, _root, run, clock, cancellation, conversation) = notice_host_fixture("c2-historical");
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    // One run-scoped Job stays unfinished, so the settled response parks.
    let parked_job = db.kernel_job_start(&JobStartRequest {
        run_id: run.clone(), kind: "attachment_compute".into(),
        idempotency_key: "c2-historical-parked".into(),
        params: json!({"input":"capacity fixture"}),
        deadline_ms: Some(crate::database::now_ms() + 60_000), progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&parked_job, 1).unwrap();

    let probe = failed_compute(&db, &run, "c2-historical-probe", &"p".repeat(32));
    let probe_bytes = packed_bytes(&db, &conversation, &run, &probe);
    let header = probe_bytes - 32;
    // Leave exactly one byte less than the next notice needs. The next notice's
    // size is the measured header plus a fixed code length, so it is known in
    // bytes before it exists.
    let next_notice_bytes = header + 64;
    let marker_total = NOTICE_BUDGET - (next_notice_bytes - 1);
    let jobs = fill_to_total(&db, &conversation, &run, "c2-historical", header, &probe, probe_bytes,
        marker_total);
    let offsets = packed_offsets(&db, &conversation, &run, jobs.len());
    assert_eq!(offsets.last().copied(), Some(marker_total),
        "the delivered markers must occupy exactly the planned byte total");

    // The first round delivers those markers, acknowledges them and parks.
    coordinator.dispatch_initial("c2-historical-owner", &Allow, |binding, frame, _| {
        assert_eq!(frame.host_job_notices.len(), jobs.len());
        Ok(stop_response(binding, frame))
    }).expect("a fitting batch plus an unfinished Job must park");
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    let (rows, bound, acknowledged, undelivered) = delivery_counts(&db, &run);
    assert_eq!((rows, bound, acknowledged, undelivered), (jobs.len() as i64, 0, jobs.len() as i64, 0),
        "the park decision must acknowledge every delivered marker");

    // The parked history is what the wake path repacks, so its marker bytes are
    // the available-capacity input.
    let parked_json: String = db.with_connection(|conn| conn.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [run.as_str()], |row| row.get::<_, String>(0))).unwrap();
    let parked: serde_json::Value = serde_json::from_str(&parked_json).unwrap();
    let mut messages: Vec<serde_json::Value> =
        serde_json::from_value(parked["history"].clone()).unwrap();
    messages.push(parked["response"]["assistantMessage"].clone());
    let historical = fox_engine_protocol::historical_host_job_notice_bytes(&messages).unwrap();
    assert_eq!(historical, marker_total, "historical markers must occupy the packed bytes");

    // The parked Job now settles: the next notice is durable and is exactly one
    // byte too large for what is left of the budget.
    db.kernel_job_settle_attempt(&parked_job, 1, crate::database::JobState::Failed,
        Some((&"c".repeat(next_notice_bytes - header), "bounded failure detail"))).unwrap();
    let next = parked_job.clone();
    assert!(db.kernel_job_notice(&conversation, &run, &next).unwrap().is_some(),
        "the settling Job must create its scoped notice");
    assert_eq!(packed_bytes(&db, &conversation, &run, &next), next_notice_bytes,
        "the new notice must serialize to exactly {next_notice_bytes} bytes");
    let available = NOTICE_BUDGET - historical;
    assert_eq!(available, next_notice_bytes - 1);
    assert!(db.pending_host_job_notices(&conversation, &run, available).unwrap().is_empty(),
        "one byte short must pack nothing");
    let only = db.pending_host_job_notices(&conversation, &run, available + 1).unwrap();
    assert_eq!(only.iter().map(|notice| notice.job_id.as_str()).collect::<Vec<_>>(),
        vec![next.as_str()], "exactly enough capacity must pack exactly the new notice");
    assert_eq!(delivery_counts(&db, &run), (jobs.len() as i64, 0, jobs.len() as i64, 1));

    // Real response-planning entry: the park/wake path.
    let error = coordinator.wake_waiting_jobs_now()
        .expect_err("a parked Run whose notices no longer fit must not wake");
    assert!(error.contains("kernel.job_notice_capacity_blocked"),
        "unexpected wake outcome: {error}");
    println!("c2 historical capacity planning outcome: {error}");

    // Nothing moved: still parked, no wake, no second notice lane, every durable
    // fact unchanged and the un-packed one still without any delivery row.
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    let (wakes, intents) = notice_events(&db, &run);
    assert_eq!((wakes, intents), (0, 0), "a capacity error must not arm a continuation");
    assert_eq!(delivery_counts(&db, &run), (jobs.len() as i64, 0, jobs.len() as i64, 1));
    assert_eq!(duplicate_deliveries(&db, &run), 0, "no notice may be delivered twice");
    assert!(db.kernel_job_notice(&conversation, &run, &next).unwrap().is_some(),
        "the un-packed notice must stay durable");
    assert!(db.kernel_job_notice(&conversation, &run, &parked_job).unwrap().is_some());
    println!("c2 historical markers: {historical} bytes occupy the budget, available={available}, \
next={next_notice_bytes} bytes, state={}",
        db.kernel_host_run_state(&run).unwrap().unwrap());
}

/// Durable facts the C2-R1 lifecycle test reads back, and compares across its
/// bounded re-entry. Everything comes from the database; the coordinator's
/// in-memory snapshot is never used as evidence.
#[derive(Debug, PartialEq, Eq, Clone)]
struct CapacityLifecycleFacts {
    state: String,
    failed_code: String,
    failed_message: String,
    model_request_since_wall_ms: Option<i64>,
    wait_deadline_wall_ms: Option<i64>,
    elapsed_to_terminal_ms: Option<i64>,
    run_execution_budget_ms: i64,
    unfinished_jobs: i64,
    jobs: Vec<(String, String, i64)>,
    /// `(history_position, job_id, state)` of every notice delivery.
    deliveries: Vec<(i64, String, String)>,
    delivery_dispatch_keys: Vec<String>,
    /// `(dispatch_key, state, history_start, historical_bytes)`.
    notice_inputs: Vec<(String, String, i64, i64)>,
    notices_without_row: i64,
    duplicate_deliveries: i64,
    /// `(effect_key, effect_type, status, lease_owner, attempts)`.
    outbox: Vec<(String, String, String, String, i64)>,
    events: Vec<String>,
}

fn capacity_lifecycle_facts(db: &Database, run: &str) -> CapacityLifecycleFacts {
    let duplicates = duplicate_deliveries(db, run);
    let (_, _, _, without_row) = delivery_counts(db, run);
    let mut facts = db.with_connection(|conn| {
        let (state, since, wait_deadline, created, terminal_at, frozen): (
            String, Option<i64>, Option<i64>, i64, Option<i64>, String,
        ) = conn.query_row(
            "SELECT state,model_request_since_wall_ms,wait_deadline_wall_ms,created_at,terminal_at,
                    frozen_config_json FROM kernel_runs WHERE run_id=?1",
            [run], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))?;
        let (failed_code, failed_message): (String, String) = conn.query_row(
            "SELECT COALESCE(MAX(json_extract(payload_json,'$.code')),''),
                    COALESCE(MAX(json_extract(payload_json,'$.message')),'')
             FROM kernel_events WHERE run_id=?1 AND event_type='run.failed'",
            [run], |row| Ok((row.get(0)?,row.get(1)?)))?;
        let unfinished_jobs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1 AND state IN ('queued','running','paused')",
            [run], |row| row.get(0))?;
        let jobs = {
            let mut statement = conn.prepare(
                "SELECT job_id,state,attempts FROM kernel_jobs WHERE run_id=?1 ORDER BY job_id")?;
            let rows = statement.query_map([run], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(String, String, i64)>>>()?;
            rows
        };
        let deliveries = {
            let mut statement = conn.prepare(
                "SELECT d.history_position,d.job_id,d.state FROM kernel_job_notice_deliveries d
                 JOIN kernel_job_notices n ON n.job_id=d.job_id
                 WHERE n.run_id=?1 ORDER BY d.history_position")?;
            let rows = statement.query_map([run], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(i64, String, String)>>>()?;
            rows
        };
        let delivery_dispatch_keys = {
            let mut statement = conn.prepare(
                "SELECT DISTINCT d.dispatch_key FROM kernel_job_notice_deliveries d
                 JOIN kernel_job_notices n ON n.job_id=d.job_id
                 WHERE n.run_id=?1 ORDER BY d.dispatch_key")?;
            let rows = statement.query_map([run], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            rows
        };
        let notice_inputs = {
            let mut statement = conn.prepare(
                "SELECT dispatch_key,state,history_start,historical_bytes
                 FROM kernel_model_notice_inputs WHERE run_id=?1 ORDER BY dispatch_key")?;
            let rows = statement.query_map([run],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?
                .collect::<rusqlite::Result<Vec<(String, String, i64, i64)>>>()?;
            rows
        };
        let outbox = {
            let mut statement = conn.prepare(
                "SELECT effect_key,effect_type,status,COALESCE(lease_owner,''),attempts
                 FROM kernel_effect_outbox WHERE run_id=?1 ORDER BY effect_key")?;
            let rows = statement.query_map([run], |row| {
                Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))
            })?.collect::<rusqlite::Result<Vec<(String, String, String, String, i64)>>>()?;
            rows
        };
        let events: String = conn.query_row(
            "SELECT COALESCE(group_concat(event_type,','),'') FROM
                (SELECT event_type FROM kernel_events WHERE run_id=?1 ORDER BY seq)",
            [run], |row| row.get(0))?;
        let run_execution_budget_ms = serde_json::from_str::<serde_json::Value>(&frozen)
            .ok().and_then(|value| value["run_execution_budget_ms"].as_i64()).unwrap_or(-1);
        Ok(CapacityLifecycleFacts {
            state, failed_code, failed_message, model_request_since_wall_ms: since,
            wait_deadline_wall_ms: wait_deadline,
            elapsed_to_terminal_ms: terminal_at.map(|at| at - created),
            run_execution_budget_ms, unfinished_jobs, jobs, deliveries, delivery_dispatch_keys,
            notice_inputs, notices_without_row: without_row, duplicate_deliveries: duplicates,
            outbox,
            events: events.split(',').map(str::to_owned).filter(|event| !event.is_empty()).collect(),
        })
    }).unwrap();
    facts.notices_without_row = without_row;
    facts.duplicate_deliveries = duplicates;
    facts
}

/// C2-R1: what actually happens to the "maximal prefix plus undelivered
/// remainder" state in the real RuntimeHost.
///
/// C2 proved the refusal at the coordinator entry. This test follows the same
/// durable state through the real Host drive loop with the real Node/Pi worker
/// on both transports and a local mock OpenAI-compatible Provider that answers
/// once with a plain stop, then performs one bounded re-entry through the same
/// Host entry and proves no durable fact moved.
///
/// The 13 notices are prepared through database interfaces: one settled
/// `attachment_compute` Job per notice, the long `errorCode` of a failed Job
/// setting its exact serialized size. They are therefore **not** the natural
/// output of a compute execution; the fixture manufactures the byte boundary on
/// purpose and this test claims no more than that.
#[test]
fn real_runtime_host_capacity_prefix_remainder_lifecycle_on_both_pi_transports() {
    use std::time::{Duration, Instant};
    use tauri::Manager;
    let selected = std::env::var("FOX_TEST_C2_R1_TRANSPORT").ok();
    let Some(mode) = selected else {
        // One bounded process per transport: the mock Provider, the OS Run lock
        // and the RuntimeHost singleton are all process-wide.
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("real_runtime_host_capacity_prefix_remainder_lifecycle_on_both_pi_transports")
                .arg("--test-threads=1")
                .arg("--nocapture")
                .env("FOX_TEST_C2_R1_TRANSPORT", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(90);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} capacity lifecycle exceeded 90 seconds");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let output = child.wait_with_output().unwrap();
            // The child's own evidence is part of the run log, not only of a
            // failure message.
            println!("=== c2r1 {mode} child stdout ===\n{}\n=== c2r1 {mode} child stderr ===\n{}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success(), "{mode} stdout: {}\n{mode} stderr: {}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    assert!(mode == "live" || mode == "round", "unknown transport {mode}");
    let live = mode == "live";

    // Exactly one provider reply: the fixture fails loudly if the Host asks for
    // a second model request.
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","content":"The requested work is complete."})]);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, run) = fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config),
        true, true, (0, 1));
    enable_notices(&db, &run);
    freeze_host_scope(&db, &run);
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    // Constructed before any Job exists: startup reconciliation has no reason to
    // relabel these Jobs as orphans.
    let host = crate::runtime_host::RuntimeHost::new(app.handle().clone(), db.clone(), root.clone(),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    host.disable_auto_wake_for_test(&run);

    // Capacity fixture: a prefix that fills the 16 KiB budget to the byte plus a
    // remainder that cannot fit under it.
    let probe = failed_compute(&db, &run, "c2r1-probe", &"p".repeat(32));
    let probe_bytes = packed_bytes(&db, &conversation, &run, &probe);
    let header = probe_bytes - 32;
    let prefix = fill_to_total(&db, &conversation, &run, "c2r1", header, &probe, probe_bytes,
        NOTICE_BUDGET);
    let extra: Vec<String> = (0..4)
        .map(|index| unverified_notice(&db, &run, &format!("c2r1-extra-{index}"), header, NOTICE_MAX))
        .collect();
    let offsets = packed_offsets(&db, &conversation, &run, prefix.len());
    assert_eq!(offsets.last().copied(), Some(NOTICE_BUDGET),
        "the prepared prefix must fill the notice budget exactly");
    let order = durable_notice_order(&db, &run);
    let remainder: Vec<String> = order.iter().filter(|job| !prefix.contains(*job)).cloned().collect();
    assert_eq!(remainder.len(), extra.len());
    assert_eq!(prefix.len() + remainder.len(), 13);

    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    let started = if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null)
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
    };
    started.unwrap_or_else(|error| panic!("{mode} Host drive failed: {error}"));

    let facts = capacity_lifecycle_facts(&db, &run);
    println!("c2r1 {mode} durable lifecycle facts: {facts:#?}");
    println!("c2r1 {mode} prefix={} jobs/{} bytes, remainder={} jobs",
        prefix.len(), NOTICE_BUDGET, remainder.len());

    // The competition error is a refusal, not a retryable model failure: the Run
    // reaches a durable terminal state without the execution budget expiring and
    // without a second model request.
    assert!(!matches!(facts.state.as_str(), "running" | "waiting_jobs"),
        "the Run must reach a durable terminal state: {facts:?}");
    assert_ne!(facts.state, "completed",
        "a Final with an undelivered remainder must never be confirmed as complete");
    assert_eq!(facts.state, "failed");
    assert_eq!(facts.failed_code, "kernel.job_notice_competition",
        "the durable code must preserve the notice refusal without exposing raw adapter text");
    assert!(facts.failed_message.contains("可能是通知容量不足")
        && facts.failed_message.contains("任务未确认完成"),
        "unexpected durable failure message: {}", facts.failed_message);
    assert_eq!(facts.model_request_since_wall_ms, None,
        "a terminal Run must not keep an in-flight model-request anchor");
    assert_eq!(facts.wait_deadline_wall_ms, None, "the Run never parked");
    assert!(facts.elapsed_to_terminal_ms.is_some_and(|elapsed| elapsed < facts.run_execution_budget_ms),
        "the terminal state must not be a budget expiry: {facts:?}");
    assert_eq!(facts.unfinished_jobs, 0, "no Job may be left unfinished or replayed");
    assert!(facts.jobs.iter().all(|(_, state, attempts)| state == "failed" && *attempts == 1),
        "every Job must stay settled exactly once: {:?}", facts.jobs);

    // Exactly the packed prefix is bound, in one dispatch, in durable order at
    // consecutive history positions anchored at the bound input's history start,
    // and nothing is acknowledged: the refused Final rolled its response back.
    assert_eq!(facts.notice_inputs.len(), 1);
    assert_eq!(facts.notice_inputs[0].1, "bound");
    assert_eq!(facts.notice_inputs[0].3, 0, "this round had no historical markers");
    assert_eq!(facts.delivery_dispatch_keys, vec![facts.notice_inputs[0].0.clone()],
        "every delivery must belong to the one bound notice input");
    let history_start = facts.notice_inputs[0].2;
    assert_eq!(facts.deliveries.iter().map(|(position, job, _)| (*position, job.clone()))
        .collect::<Vec<_>>(),
        order[..prefix.len()].iter().enumerate()
            .map(|(index, job)| (history_start + index as i64, job.clone())).collect::<Vec<_>>(),
        "the bound deliveries must be exactly the packed prefix in durable order");
    assert!(facts.deliveries.iter().all(|(_, _, state)| state == "bound"),
        "no delivery may be acknowledged while the Final was refused: {:?}", facts.deliveries);
    assert_eq!(facts.notices_without_row, remainder.len() as i64,
        "every undelivered notice must keep its durable row and no delivery row");
    assert_eq!(facts.duplicate_deliveries, 0, "no notice may be delivered twice");
    assert_eq!(facts.outbox.iter().filter(|(_, kind, _, _, _)| kind == "initial_model").count(), 1);
    assert_eq!(facts.outbox.iter().filter(|(_, kind, _, _, _)| kind == "continuation_model").count(),
        0, "no second notice lane may be armed: {:?}", facts.outbox);
    assert_eq!(facts.outbox.iter().filter(|(_, kind, _, _, _)| kind == "dispatch_tool").count(), 0);

    // The Provider was asked exactly once, and only the prefix reached the wire.
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1, "the Host must ask the Provider exactly once per transport");
    let wire = serde_json::to_string(&requests[0]["messages"]).unwrap();
    assert_eq!(wire.matches("FOX_HOST_JOB_NOTICE_V1").count(), prefix.len(),
        "the request must carry exactly the packed prefix");
    for job in &prefix { assert!(wire.contains(job.as_str()), "packed notice {job} is missing"); }
    for job in &remainder {
        assert!(!wire.contains(job.as_str()), "an undelivered notice reached the model: {job}");
    }
    assert_eq!(requests[0]["model"].as_str(), Some("kernel-http-test"),
        "the round must have used the frozen local model service");

    // Bounded re-entry through the same Host entry a restart uses. The OS Run
    // lock must be free and the drive loop must return without dispatching.
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run)
        .expect("the previous owner must have released the OS Run lock");
    let reentry = if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null)
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
    };
    println!("c2r1 {mode} bounded re-entry result: {reentry:?}");
    assert!(reentry.is_ok(), "bounded re-entry failed: {reentry:?}");
    let after = capacity_lifecycle_facts(&db, &run);
    assert_eq!(after, facts, "a bounded re-entry must not change one durable fact");
    assert!(!host.state.lock().unwrap().kernel_active_runs.contains(&run),
        "the Host must not keep an active Run slot after the terminal state");
    println!("c2r1 {mode} done: state={} code={} elapsed={:?}ms budget={}ms prefix={} bound={} \
remainder={} undelivered re-entry=clean", facts.state, facts.failed_code,
        facts.elapsed_to_terminal_ms, facts.run_execution_budget_ms, prefix.len(), prefix.len(),
        remainder.len());
}
