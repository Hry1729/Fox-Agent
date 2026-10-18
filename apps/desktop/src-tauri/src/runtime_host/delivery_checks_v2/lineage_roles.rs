//! Multi-hop lineage and per-artifact-role acceptance.
//!
//! ## Ownership
//!
//! **Owner from the P1a base commit: D (lineage and artifact roles).** Until then
//! this file is a C0 placeholder owned by A. D never edits `mod.rs`, the public
//! DTOs, `attachment_compute.rs`, `delivery*.rs`, database production code or
//! `Cargo.*`.
//!
//! ## Wave note (P1a, plan §7 P1-2)
//!
//! The real input surface (lineage node/edge identity, candidate version, role
//! plan) is frozen in the P1a base commit. D requests it from A; C0 does not
//! pre-build it. A owns tool registration and persistence; D submits a schema /
//! migration proposal instead of touching the database.
//!
//! ## What D must preserve when the module lands
//!
//! * Only edges the **Host** recorded count. A model-written `sourceVerified`
//!   field, a copied parent metadata block or a hash typed into prose is never
//!   evidence.
//! * Every hop is checked for identity, version / hash, generating step,
//!   same-session or legitimate continuation, and authorization scope; cycles,
//!   depth and node count are bounded by [`CheckLimits`].
//! * A missing edge is `unverified`; an identity, permission or version conflict
//!   stops the item. Reachable source bytes prove dependency identity only —
//!   never that a computation or a sentence is correct (F05, R17–R20).

use super::*;

/// Verify the lineage of one bound artifact.
///
/// C0 placeholder signature only: P1a replaces it with the real node/edge input
/// after A publishes that base.
pub(crate) fn verify_lineage(
    rule_id: &str,
    binding: &CandidateBinding,
    limits: &CheckLimits,
) -> CheckObservation {
    let _ = limits;
    // C0 placeholder — D replaces the body in P1a.
    not_implemented(rule_id, check_name::LINEAGE, binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/chart.svg", "d".repeat(64), 256)
    }

    /// C0 state only. D replaces this with real coverage of a legal multi-hop
    /// chain, legitimate continuation, a missing parent, a forged id, a
    /// cross-session hop, a cycle / budget stop, a tampered hash, a stale
    /// version, a method note that cites a real receipt, and "the source is
    /// reachable but the final number is wrong".
    #[test]
    fn c0_placeholder_reports_unverified_and_never_a_pass() {
        let observation = verify_lineage("content:data", &binding(), &CheckLimits::P0);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::CHECKER_NOT_IMPLEMENTED)
        );
        assert!(!observation.blocking);
        assert_eq!(observation.check, check_name::LINEAGE);
    }
}
