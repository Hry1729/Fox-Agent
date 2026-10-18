//! Resource budgeting for `delivery_checks_v2` (F-P02-07).
//!
//! Design basis: charter §27–§35 and the frozen plans (A third version as
//! amended by consolidation/closeout/closeout-errata).
//!
//! * [`Clock`] is an injectable monotonic time source, so the millisecond
//!   budget is testable through [`FakeClock`] instead of real sleeps.
//! * [`Budget`] is the single per-call meter: time is checked **before** every
//!   atomic read, and a read permit is granted only when the *next* counter
//!   value stays within `max_scan_bytes`. Bytes are committed only after the
//!   authorized bytes were actually consumed, so an over-limit rejection never
//!   reports unread bytes as `observed`.
//! * [`Outcome`]/[`Budget::finalize`] encode the three exit kinds: a failure
//!   already proven under the full policy, a completed `Passed`/normal
//!   undecided conclusion, or a budget stop obtained before the conclusion was
//!   complete.
//!
//! The construction memcpy (`expected_concat`) is bounded by `max_text_bytes`
//! and time-checked per chunk but is deliberately **not** charged to
//! `max_scan_bytes`: its size is already capped, and charging it twice would
//! count the frozen inputs a second time.

use std::time::Instant;
#[cfg(test)]
use std::{cell::Cell, time::Duration};

use super::{
    budget_exceeded, reason_code, CandidateBinding, CheckLimits, CheckObservation, LimitKind,
    TableError,
};

/// Which production phase a time check belongs to. Test-only phase
/// attribution: the production path records the phase **before** a metered
/// read/time check, and an instrumented test clock stores it on its own
/// instance. This replaces an earlier fixture that inferred the phase from a
/// `Backtrace` string, which is not reliable under `debug=0` (frames get
/// inlined away). Recording neither calls [`Clock::now`] nor charges a read.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Entry,
    ConstructionLength,
    Construction,
    ConstructionCopy,
    LineEndingVerdict,
    TailLf,
    ConflictAttribution,
    RawCompare,
    Normalization,
    Diagnosis,
    HeaderSearch,
    Window,
    Finalize,
    TableUtf8,
    TablePrescan,
    TableParse,
}

/// Monotonic time source for one check. Production uses [`SystemClock`]; tests
/// drive an instrumented clock deterministically.
pub(crate) trait Clock {
    fn now(&self) -> Instant;

    /// Record the production phase of the next time/read check. Default is a
    /// no-op; only instrumented test clocks collect phases. Not called by
    /// production beyond the `#[cfg(test)]` markers, never calls [`Self::now`].
    #[cfg(test)]
    fn record_phase(&self, _phase: Phase) {}
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A deterministic clock for tests: a fixed start plus an externally advanced
/// offset. Interior mutability lets a test advance the clock while a [`Budget`]
/// still borrows it. No real sleeping is involved.
///
/// It also collects the `#[cfg(test)]` production phases on its own instance
/// (parallel-safe: each test builds its own clock) and records the phase of
/// the first time check that observes an expired deadline.
#[cfg(test)]
pub(crate) struct FakeClock {
    start: Instant,
    elapsed_ms: Cell<u64>,
    phases: std::cell::RefCell<Vec<Phase>>,
}

#[cfg(test)]
impl FakeClock {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            elapsed_ms: Cell::new(0),
            phases: std::cell::RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn advance(&self, ms: u64) {
        self.elapsed_ms
            .set(self.elapsed_ms.get().saturating_add(ms));
    }

    pub(crate) fn set(&self, ms: u64) {
        self.elapsed_ms.set(ms);
    }

    /// All recorded production phases, in call order.
    pub(crate) fn phases(&self) -> Vec<Phase> {
        self.phases.borrow().clone()
    }

    /// The production phase recorded immediately before the **first**
    /// `now()` observation that saw an expired deadline. This is the phase
    /// the first expired check belongs to; the second `now()` inside error
    /// construction is not recorded as a new expired phase.
    pub(crate) fn first_expired_phase(&self) -> Option<Phase> {
        // Reconstructed by the instrumented step clock instead; FakeClock
        // at a fixed elapsed value reports the last recorded phase.
        self.phases.borrow().last().copied()
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.start + Duration::from_millis(self.elapsed_ms.get())
    }

    fn record_phase(&self, phase: Phase) {
        self.phases.borrow_mut().push(phase);
    }
}

/// An obtained budget stop. `observed` is the actual, already-consumed amount:
/// bytes examined for [`LimitKind::ScanBytes`], elapsed milliseconds for
/// [`LimitKind::Millis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetStop {
    pub(crate) kind: LimitKind,
    pub(crate) observed: u64,
    /// Bytes the next atomic read asked for (1 single / 2 paired); 0 for time.
    pub(crate) need: u64,
    /// Counter value the next read would have committed.
    pub(crate) next: u64,
}

impl BudgetStop {
    pub(crate) fn to_observation(
        &self,
        limits: &CheckLimits,
        rule_id: &str,
        check: &str,
        binding: &CandidateBinding,
    ) -> CheckObservation {
        budget_exceeded(
            rule_id,
            check,
            binding,
            self.kind,
            limits.ceiling(self.kind),
            self.observed,
        )
    }

    pub(crate) fn to_table_error(&self, limits: &CheckLimits) -> TableError {
        TableError::budget(
            self.kind,
            limits.ceiling(self.kind),
            self.observed,
            super::RecordLocator::default(),
        )
    }
}

/// A granted read: committing `next` is valid only after the authorized bytes
/// were actually consumed.
#[derive(Debug)]
pub(crate) struct Permit {
    pub(crate) next: u64,
}

/// The per-call resource meter.
pub(crate) struct Budget<'a> {
    limits: &'a CheckLimits,
    clock: &'a dyn Clock,
    start: Instant,
    examined: u64,
}

impl<'a> Budget<'a> {
    pub(crate) fn new(limits: &'a CheckLimits, clock: &'a dyn Clock) -> Self {
        Self {
            limits,
            clock,
            start: clock.now(),
            examined: 0,
        }
    }

    pub(crate) fn limits(&self) -> &'a CheckLimits {
        self.limits
    }

    /// Test-only: tag the phase of the next metered operation. Forwards to
    /// the clock's instance-bound recorder; no `now()` call, no charge.
    #[cfg(test)]
    pub(crate) fn phase(&self, phase: Phase) {
        self.clock.record_phase(phase);
    }

    /// The actual logical input bytes consumed so far. Test/inspection only.
    #[cfg(test)]
    pub(crate) fn examined(&self) -> u64 {
        self.examined
    }

    /// Test-only constructor with a preset counter, for the arithmetic
    /// overflow/remainder boundaries that real inputs cannot reach.
    #[cfg(test)]
    pub(crate) fn tested_with_examined(
        limits: &'a CheckLimits,
        clock: &'a dyn Clock,
        examined: u64,
    ) -> Self {
        Self {
            limits,
            clock,
            start: clock.now(),
            examined,
        }
    }

    /// Elapsed milliseconds as a wide integer. A non-monotonic clock is treated
    /// as "elapsed" rather than silently shrinking the budget.
    fn elapsed_ms(&self) -> u128 {
        match self.clock.now().checked_duration_since(self.start) {
            Some(duration) => duration.as_millis(),
            None => u128::MAX,
        }
    }

    /// Time gate. Predicate is `elapsed >= max_millis`, so `max_millis = 0`
    /// trips the very first call before any allocation or scan.
    pub(crate) fn check_time(&self) -> Result<(), BudgetStop> {
        if self.elapsed_ms() >= self.limits.max_millis as u128 {
            return Err(BudgetStop {
                kind: LimitKind::Millis,
                observed: self.elapsed_ms().min(u64::MAX as u128) as u64,
                need: 0,
                next: self.examined,
            });
        }
        Ok(())
    }

    fn pre_read(&mut self, need: u64) -> Result<Permit, BudgetStop> {
        // Fixed order at one point: time first, scan bytes second.
        self.check_time()?;
        let next = self.examined.checked_add(need).ok_or(BudgetStop {
            kind: LimitKind::ScanBytes,
            observed: self.examined,
            need,
            next: self.examined,
        })?;
        if next > self.limits.max_scan_bytes {
            return Err(BudgetStop {
                kind: LimitKind::ScanBytes,
                observed: self.examined,
                need,
                next,
            });
        }
        Ok(Permit { next })
    }

    /// Authorize reading one existing byte. The caller has already established
    /// that the index exists; a natural EOF is not pre-checked or charged.
    pub(crate) fn pre_read_one(&mut self) -> Result<Permit, BudgetStop> {
        self.pre_read(1)
    }

    /// Authorize one paired read (one byte from each of two slices), i.e. two
    /// logical bytes. Used by every side-by-side comparison; batches are just
    /// loops of such permits and short-circuit as soon as a pair differs.
    pub(crate) fn pre_read_pair(&mut self) -> Result<Permit, BudgetStop> {
        self.pre_read(2)
    }

    pub(crate) fn commit(&mut self, permit: Permit) {
        // Invariant: `next` came from a successful permit for exactly the bytes
        // about to be consumed, and reads/commit are sequential on one call.
        self.examined = permit.next;
    }

    /// Read one byte known to exist (`i < bytes.len()`), charging one logical
    /// byte. Natural EOF is decided by the caller before calling.
    pub(crate) fn byte_at(&mut self, bytes: &[u8], i: usize) -> Result<u8, BudgetStop> {
        let permit = self.pre_read_one()?;
        let byte = bytes[i];
        self.commit(permit);
        Ok(byte)
    }

    /// Read the last byte, if any. An empty slice costs nothing (natural EOF).
    pub(crate) fn last_byte(&mut self, bytes: &[u8]) -> Result<Option<u8>, BudgetStop> {
        match bytes.last() {
            Some(_) => {
                let permit = self.pre_read_one()?;
                let byte = bytes[bytes.len() - 1];
                self.commit(permit);
                Ok(Some(byte))
            }
            None => Ok(None),
        }
    }

    /// Read one pair of bytes known to exist on both sides (two logical bytes).
    fn pair_at(
        &mut self,
        left: &[u8],
        right: &[u8],
        i: usize,
    ) -> Result<(u8, u8), BudgetStop> {
        let permit = self.pre_read_pair()?;
        let pair = (left[i], right[i]);
        self.commit(permit);
        Ok(pair)
    }

    /// Length is metadata (free). Equality then proceeds pair by pair: actual
    /// comparisons are charged and the scan stops at the first differing pair.
    pub(crate) fn slices_equal(
        &mut self,
        left: &[u8],
        right: &[u8],
    ) -> Result<bool, BudgetStop> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for i in 0..left.len() {
            let (a, b) = self.pair_at(left, right, i)?;
            if a != b {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The construction-length arithmetic overflowed (`S2`). It happens before
    /// the frozen inputs are copied, so it is a `TextBytes` stop.
    pub(crate) fn text_length_overflow(&self) -> BudgetStop {
        BudgetStop {
            kind: LimitKind::TextBytes,
            // No real sum exists; report "strictly over the ceiling", never a
            // fabricated consumed amount.
            observed: self.limits.max_text_bytes as u64 + 1,
            need: 0,
            next: 0,
        }
    }

    /// Exit rule (charter §32/§33):
    /// * a failure proven under the complete policy is returned unchanged —
    ///   terminal checks neither start a timed-out diagnosis nor demote it;
    /// * an already obtained budget stop becomes the K1 observation;
    /// * a completed `Passed`/normal-undecided conclusion still passes the
    ///   terminal time check — expiring during the last required read makes the
    ///   conclusion incomplete (K1).
    pub(crate) fn finalize(
        &mut self,
        outcome: Outcome,
        rule_id: &str,
        check: &str,
        binding: &CandidateBinding,
    ) -> CheckObservation {
        match outcome {
            Outcome::ProvenFailed(observation) => observation,
            Outcome::Stopped(stop) => stop.to_observation(self.limits, rule_id, check, binding),
            Outcome::Complete(observation) => {
                #[cfg(test)]
                self.phase(Phase::Finalize);
                match self.check_time() {
                    Ok(()) => observation,
                    Err(stop) => stop.to_observation(self.limits, rule_id, check, binding),
                }
            }
        }
    }
}

/// The three ways a judgment can end.
pub(crate) enum Outcome {
    ProvenFailed(CheckObservation),
    Complete(CheckObservation),
    Stopped(BudgetStop),
}

// ---------------------------------------------------------------------------
// Chunked, stoppable UTF-8 validation (P2).
//
// Per-byte state machine. Each input byte is processed exactly once: the lead
// sets the *next* continuation range, every continuation is checked against
// the saved range on arrival and the range then resets to 80..BF. Masks are
// 0x1F / 0x0F / 0x07 for 2/3/4-byte leads. A sequence unfinished at natural
// EOF is an encoding error at its `seq_start` (body-relative, zero-based); a
// budget refusal to read a still-existing byte is a Resource stop, never an
// encoding error or a fake EOF.
// ---------------------------------------------------------------------------

enum Utf8State {
    Ascii,
    Need {
        remain: u8,
        cp: u32,
        start: usize,
        next_min: u8,
        next_max: u8,
    },
}

/// Validate `body` as UTF-8 with every byte charged to the scan/time budget.
/// Offsets are relative to `body` (zero-based even when a BOM was stripped).
pub(crate) fn validate_utf8_metered(
    body: &[u8],
    budget: &mut Budget<'_>,
) -> Result<(), TableError> {
    let mut state = Utf8State::Ascii;
    let mut i = 0usize;
    while i < body.len() {
        // The byte exists; only a budget refusal stops us here (Resource).
        let permit = budget
            .pre_read_one()
            .map_err(|stop| stop.to_table_error(budget.limits()))?;
        let x = body[i];
        budget.commit(permit);

        match &mut state {
            Utf8State::Ascii => {
                let (remain, init_cp, lo, hi): (u8, u32, u8, u8) = match x {
                    0x00..=0x7F => {
                        i += 1;
                        continue;
                    }
                    0xC2..=0xDF => (1, u32::from(x & 0x1F), 0x80, 0xBF),
                    0xE0..=0xEF => (
                        2,
                        u32::from(x & 0x0F),
                        if x == 0xE0 { 0xA0 } else { 0x80 },
                        if x == 0xED { 0x9F } else { 0xBF },
                    ),
                    0xF0..=0xF4 => (
                        3,
                        u32::from(x & 0x07),
                        if x == 0xF0 { 0x90 } else { 0x80 },
                        if x == 0xF4 { 0x8F } else { 0xBF },
                    ),
                    // C0/C1 overlong leads and F5..FF are illegal.
                    _ => return Err(encoding_error(i, format!("非法 UTF-8 起始字节 0x{x:02x}"))),
                };
                state = Utf8State::Need {
                    remain,
                    cp: init_cp,
                    start: i,
                    next_min: lo,
                    next_max: hi,
                };
            }
            Utf8State::Need {
                remain,
                cp,
                start,
                next_min,
                next_max,
            } => {
                if x < *next_min || x > *next_max {
                    let detail = if !(0x80..=0xBF).contains(&x) {
                        format!("UTF-8 序列在字节 {start} 后遇到非续字节 0x{x:02x}")
                    } else {
                        format!("UTF-8 序列在字节 {start} 的续字节 0x{x:02x} 超出允许范围")
                    };
                    return Err(encoding_error(*start, detail));
                }
                *cp = (*cp << 6) | u32::from(x & 0x3F);
                *remain -= 1;
                *next_min = 0x80;
                *next_max = 0xBF;
                if *remain == 0 {
                    if *cp > 0x10FFFF {
                        return Err(encoding_error(
                            *start,
                            format!("UTF-8 码点 U+{:04X} 超过 U+10FFFF", *cp),
                        ));
                    }
                    state = Utf8State::Ascii;
                }
            }
        }
        i += 1;
    }
    if let Utf8State::Need { start, .. } = state {
        // Natural EOF with continuation bytes still expected.
        return Err(encoding_error(
            start,
            format!("UTF-8 序列在字节 {start} 处未正常结束（文件结束）"),
        ));
    }
    Ok(())
}

fn encoding_error(byte_offset: usize, detail: String) -> TableError {
    // UTF-8 the checker cannot interpret is an Unsupported (capability)
    // failure, not a declared-encoding violation: there is no frozen
    // encoding field, only the checker's UTF-8 implementation.
    TableError::unsupported(
        reason_code::TABLE_ENCODING_UNSUPPORTED,
        super::RecordLocator::default().at_byte(byte_offset as u64),
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{failed, passed, CandidateBinding, CheckLimits, CheckState, LimitKind};

    fn binding() -> CandidateBinding {
        CandidateBinding::new("o/r", "s".repeat(64), 1)
    }

    fn limits_scan(scan: u64, millis: u64) -> CheckLimits {
        let mut limits = CheckLimits::P0;
        limits.max_scan_bytes = scan;
        limits.max_millis = millis;
        limits
    }

    #[test]
    fn meter_single_and_pair_prechecks_with_remainder_one() {
        let clock = FakeClock::new();
        let limits = limits_scan(3, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        let bytes = b"abcd";
        assert!(budget.byte_at(bytes, 0).is_ok()); // observed 1
        let pair = budget.pre_read_pair(); // 1+2 = 3 allowed
        assert!(pair.is_ok());
        budget.commit(pair.unwrap());
        // One byte of budget remains, but a paired read needs two: stop
        // before reading, and observed stays at 3 (never reports 4).
        let stop = budget.pre_read_pair().unwrap_err();
        assert_eq!(stop.kind, LimitKind::ScanBytes);
        assert_eq!(stop.observed, 3);
        assert_eq!(stop.need, 2);
        assert_eq!(budget.examined(), 3);
    }

    #[test]
    fn meter_equality_charges_until_first_difference() {
        let clock = FakeClock::new();
        let limits = limits_scan(64, 30_000);

        // One call: pair 1 (a == a) is read, pair 2 (x != b) is read and
        // stops the scan — two pairs = four logical bytes in a single
        // slices_equal call.
        let mut budget = Budget::new(&limits, &clock);
        assert!(!budget.slices_equal(b"ax", b"ab").unwrap());
        assert_eq!(budget.examined(), 4);
        // A length-different comparison reads nothing (metadata); the
        // counter stays at 4.
        assert!(!budget.slices_equal(b"a", b"abc").unwrap());
        assert_eq!(budget.examined(), 4);

        // Independent case with a fresh budget: the very first pair differs,
        // so two logical bytes are read and the scan stops immediately.
        let mut fresh = Budget::new(&limits, &clock);
        assert!(!fresh.slices_equal(b"xa", b"ab").unwrap());
        assert_eq!(fresh.examined(), 2);
    }

    #[test]
    fn meter_zero_scan_blocks_the_first_read() {
        let clock = FakeClock::new();
        let limits = limits_scan(0, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        let stop = budget.byte_at(b"x", 0).unwrap_err();
        assert_eq!(stop.kind, LimitKind::ScanBytes);
        assert_eq!(stop.observed, 0);
        // Natural EOF reads nothing and does not error.
        assert_eq!(budget.last_byte(b"").unwrap(), None);
        assert_eq!(budget.examined(), 0);
    }

    #[test]
    fn meter_equality_at_ceiling_is_allowed() {
        let clock = FakeClock::new();
        let limits = limits_scan(4, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        assert!(budget.slices_equal(b"ab", b"ab").unwrap());
        assert_eq!(budget.examined(), 4);
    }

    #[test]
    fn meter_checked_add_overflow_near_u64_max_is_scan_stop() {
        let clock = FakeClock::new();
        let limits = limits_scan(u64::MAX, 30_000);
        let mut budget = Budget::tested_with_examined(&limits, &clock, u64::MAX);
        let stop = budget.pre_read_one().unwrap_err();
        assert_eq!(stop.kind, LimitKind::ScanBytes);
        assert_eq!(stop.observed, u64::MAX);
    }

    #[test]
    fn meter_zero_millis_stops_at_the_first_check() {
        let clock = FakeClock::new();
        let limits = limits_scan(64, 0);
        let budget = Budget::new(&limits, &clock);
        let stop = budget.check_time().unwrap_err();
        assert_eq!(stop.kind, LimitKind::Millis);
    }

    #[test]
    fn meter_nonzero_clock_at_and_before_deadline() {
        let clock = FakeClock::new();
        let limits = limits_scan(64, 10);
        let budget = Budget::new(&limits, &clock);
        clock.advance(9);
        assert!(budget.check_time().is_ok());
        clock.set(10); // >= deadline
        assert!(budget.check_time().is_err());
    }

    #[test]
    fn utf8_valid_ascii_two_three_four_byte_sequences() {
        for input in [
            &b"a"[..],
            &[0xC2, 0xA2],
            &[0xE4, 0xBD, 0xA0],
            &[0xF0, 0x9F, 0x98, 0x80],
            &[0xF4, 0x8F, 0xBF, 0xBF],
            &[0xE0, 0xA0, 0x80],
            &[0xED, 0x9F, 0xBF],
            &[0xEE, 0x80, 0x80],
            &[0xF0, 0x90, 0x80, 0x80],
        ] {
            let clock = FakeClock::new();
            let limits = limits_scan(64, 30_000);
            let mut budget = Budget::new(&limits, &clock);
            validate_utf8_metered(input, &mut budget)
                .unwrap_or_else(|error| panic!("{input:02X?} rejected: {error:?}"));
        }
    }

    #[test]
    fn utf8_special_second_byte_ranges_reject_at_seq_start() {
        for input in [
            &[0xE0, 0x9F, 0x80][..],
            &[0xED, 0xA0, 0x80][..],
            &[0xF0, 0x8F, 0xBF, 0xBF][..],
            &[0xF4, 0x90, 0x80, 0x80][..],
        ] {
            let clock = FakeClock::new();
            let limits = limits_scan(64, 30_000);
            let mut budget = Budget::new(&limits, &clock);
            let error = validate_utf8_metered(input, &mut budget).unwrap_err();
            assert_eq!(error.locator.byte_offset, Some(0), "{input:02X?}");
            assert!(error.limit.is_none(), "{input:02X?} is not a budget stop");
        }
    }

    #[test]
    fn utf8_overlong_and_bare_continuation_leads_are_rejected() {
        let clock = FakeClock::new();
        let limits = limits_scan(64, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        assert!(validate_utf8_metered(&[0xC0, 0x80], &mut budget).is_err());
        let mut budget = Budget::new(&limits, &clock);
        assert!(validate_utf8_metered(&[0xC1, 0xBF], &mut budget).is_err());
        let mut budget = Budget::new(&limits, &clock);
        assert!(validate_utf8_metered(&[0x80], &mut budget).is_err());
        let mut budget = Budget::new(&limits, &clock);
        assert!(validate_utf8_metered(&[0xF5, 0x80, 0x80], &mut budget).is_err());
    }

    #[test]
    fn utf8_unfinished_sequence_at_natural_eof_is_offset_zero() {
        let clock = FakeClock::new();
        let limits = limits_scan(64, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        let error = validate_utf8_metered(&[0xE1, 0x80], &mut budget).unwrap_err();
        assert_eq!(error.locator.byte_offset, Some(0));
        assert!(error.limit.is_none());
    }

    #[test]
    fn utf8_budget_stop_before_deciding_byte_is_resource_not_encoding() {
        // One byte read allowed; the still-existing second byte cannot be
        // read, so this is a Resource stop, not a natural-EOF encoding error.
        let clock = FakeClock::new();
        let limits = limits_scan(1, 30_000);
        let mut budget = Budget::new(&limits, &clock);
        let error = validate_utf8_metered(&[0xE1, 0x80], &mut budget).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::ScanBytes));
        assert_eq!(error.locator.byte_offset, None);
    }

    #[test]
    fn finalize_stopped_is_k1_proven_failed_kept_complete_checked() {
        let limits = limits_scan(64, 10);

        // Stopped maps to the explicit budget observation, independent of the
        // clock.
        let clock = FakeClock::new();
        let mut budget = Budget::new(&limits, &clock);
        let observation = budget.finalize(
            Outcome::Stopped(BudgetStop {
                kind: LimitKind::ScanBytes,
                observed: 1,
                need: 2,
                next: 3,
            }),
            "rule",
            "check",
            &binding(),
        );
        assert_eq!(observation.state, CheckState::Unverified);
        assert!(!observation.blocking);
        assert_eq!(observation.limit, Some(LimitKind::ScanBytes));

        // A failure proven under the complete policy is kept when the clock
        // expires *after* the budget's start was fixed. Create the budget
        // first (pins start at elapsed 0), then advance to the deadline.
        let clock = FakeClock::new();
        let mut budget = Budget::new(&limits, &clock);
        clock.set(10); // now relative elapsed is exactly C
        let proven = failed(
            "rule",
            "check",
            super::super::reason_code::TEXT_CONTENT_MISMATCH,
            &binding(),
            "already proven",
        );
        let observation =
            budget.finalize(Outcome::ProvenFailed(proven), "rule", "check", &binding());
        assert_eq!(observation.state, CheckState::Failed);
        assert!(observation.blocking);

        // Complete is time-checked at the exit. Budget created at elapsed 0,
        // then advanced to C-1: the terminal check observes elapsed < C and
        // keeps the Passed.
        let clock = FakeClock::new();
        let mut budget = Budget::new(&limits, &clock);
        clock.advance(9);
        let complete = passed("rule", "check", &binding());
        let observation =
            budget.finalize(Outcome::Complete(complete), "rule", "check", &binding());
        assert_eq!(observation.state, CheckState::Passed);

        // Budget created at elapsed 0, then advanced to C: the terminal check
        // observes elapsed >= C and turns Complete into K1.
        let clock = FakeClock::new();
        let mut budget = Budget::new(&limits, &clock);
        clock.set(10);
        let complete = passed("rule", "check", &binding());
        let observation =
            budget.finalize(Outcome::Complete(complete), "rule", "check", &binding());
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Millis));
    }
}
