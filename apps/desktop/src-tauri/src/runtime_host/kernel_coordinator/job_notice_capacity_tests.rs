//! Capacity boundary of the typed Host notice lane (C2).
//!
//! Evidence tier: **component/coordinator**. These tests drive the real SQLite
//! kernel state, the real production packer (`pending_model_facts` through
//! `pending_host_job_notices`), the real binding/acknowledgement path and the
//! real response-planning entries (`dispatch_initial` and
//! `wake_waiting_jobs_now`). Model responses are closures and no Pi transport,
//! Node worker or Provider is involved, so this is **not** a real Host/Node/Pi
//! end-to-end acceptance.
//!
//! Every capacity boundary is measured in the serialized bytes the production
//! packer itself counts — never in characters and never from a slack guess.
use super::*;
use super::host_job_notice_tests::{failed_compute, notice_host_fixture, stop_response};
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
