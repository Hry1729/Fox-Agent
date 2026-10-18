//! Typed cell evidence and finite per-metric recomputation.
//!
//! ## Ownership
//!
//! **Owner from the P1a base commit: C (cell types and metrics).** Until then
//! this file is a C0 placeholder owned by A. C never edits `mod.rs`, the public
//! DTOs, `data_compute.rs`, `delivery*.rs` or `Cargo.*`.
//!
//! ## Wave note (P1a, plan §7 P1-1)
//!
//! P1a freezes the real input surface for this module (typed cell evidence and
//! `MetricSpec`): C requests it from A, who publishes the P1a base commit. C0
//! deliberately does **not** pre-build those DTOs, so the placeholder below takes
//! only what every check already has.
//!
//! ## What C must preserve when the module lands
//!
//! * `data_compute.rs::cell_to_value_capped` already keeps `Int` / `Float` /
//!   `Bool` / `DateTime`; the type evidence is lost later, in the string matrix.
//!   The fix is to carry it, never to re-infer it from display text.
//! * A numeric-cell demand is met by a real numeric cell; the text `"136.69"`
//!   must not be accepted in its place. Phone numbers, ids and dates keep their
//!   text/date identity.
//! * An unknown metric stays `unverified` **and the known metrics after it are
//!   still checked** (F04: today an unknown operation returns early).
//! * No undeclared global uniqueness, date-continuity or float-tolerance rule.

use super::*;

/// Evaluate one frozen metric demand against the bound candidate.
///
/// C0 placeholder signature only: P1a replaces it with the real
/// typed-table / `MetricSpec` input after A publishes that base.
pub(crate) fn evaluate_typed_metric(
    rule_id: &str,
    binding: &CandidateBinding,
    limits: &CheckLimits,
) -> CheckObservation {
    let _ = limits;
    // C0 placeholder — C replaces the body in P1a.
    not_implemented(rule_id, check_name::TYPED_METRIC, binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/report.xlsx", "c".repeat(64), 128)
    }

    /// C0 state only. C replaces this with real coverage of numeric vs numeric
    /// text, leading-zero ids, dates, every formula-cache state, a wrong or zero
    /// denominator, unknown metrics with later real errors, declared date/key
    /// constraints, and blank/null/zero and rounding boundaries.
    #[test]
    fn c0_placeholder_reports_unverified_and_never_a_pass() {
        let observation = evaluate_typed_metric("content:data", &binding(), &CheckLimits::P0);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::CHECKER_NOT_IMPLEMENTED)
        );
        assert!(!observation.blocking);
        assert_eq!(observation.check, check_name::TYPED_METRIC);
    }
}
