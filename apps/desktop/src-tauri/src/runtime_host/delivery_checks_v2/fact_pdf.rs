//! Fact invariants (metric identity / value / unit / currency) and real PDF
//! structure checks.
//!
//! ## Ownership
//!
//! **Owner from the P1b base commit: E (facts and PDF).** Until then this file is
//! a C0 placeholder owned by A. E never edits `mod.rs`, the public DTOs,
//! `delivery*.rs`, database code, another role's module or `Cargo.*`.
//!
//! ## C0 inventory facts E starts from (verified at `0df9162`)
//!
//! * There is **no PDF parsing crate** in the workspace. `report_pdf.rs`
//!   (39.7 KB) is a PDF *generator* on top of `krilla` + `ttf-parser`; the only
//!   existing PDF *read* is
//!   `delivery.rs::parse_check` `ArtifactKind::Pdf`, which checks the `%PDF-`
//!   header and byte-scans `/Type/Page` (F07).
//! * `flate2` is already a dependency, so a bounded pure-Rust structural parse
//!   (header, trailer, xref / object walk, `FlateDecode` streams, page tree)
//!   needs **no new dependency** (decision D1).
//! * Rendering "opens correctly" stays **out of scope this round** and must be
//!   reported as a capability limit, never as `passed` (D1, R24).
//! * A v1 byte-scan verdict keeps its original grade. It is never upgraded to
//!   "parsed and openable" (plan §7 P1-3 迁移).
//!
//! ## What E must preserve when the module lands
//!
//! * Separate literal quotation from paraphrase: whole-clause literal matching
//!   only where the rule demands a verbatim quote. A legal Chinese translation or
//!   rewrite is not a failure merely because the sentence does not match.
//! * `$` is not evidence of USD; currency may stay `unknown`. "The admin asked
//!   for it" is not "the admin adopted it". An explicitly forbidden mention stays
//!   a hard failure even in a negated sentence.
//! * Distinguish a corrupt file, a budget stop, an unavailable tool and an
//!   uncovered semantic. A model's semantic opinion is a suggestion, never a
//!   proof.

use super::*;

/// Check the structure of one PDF candidate (and, later, its fact invariants).
///
/// C0 placeholder signature only: P1b replaces it with the real input surface
/// after C and D are wired and A publishes that base.
pub(crate) fn check_pdf_structure(
    rule_id: &str,
    binding: &CandidateBinding,
    limits: &CheckLimits,
) -> CheckObservation {
    let _ = limits;
    // C0 placeholder — E replaces the body in P1b.
    not_implemented(rule_id, check_name::PDF_STRUCTURE, binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/report.pdf", "e".repeat(64), 1024)
    }

    /// C0 state only. E replaces this with real coverage of a legal compressed
    /// object, a fake `%PDF-` header, a broken page tree, a render failure, the
    /// 122-products-minus-Top-5 = 117 case, a legal translation of the same
    /// metric id, a wrong value or borrowed id, a baseless USD claim, the
    /// requirement/adoption confusion and a forbidden feature.
    #[test]
    fn c0_placeholder_reports_unverified_and_never_a_pass() {
        let observation = check_pdf_structure("content:material:0", &binding(), &CheckLimits::P0);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::CHECKER_NOT_IMPLEMENTED)
        );
        assert!(!observation.blocking);
        assert_eq!(observation.check, check_name::PDF_STRUCTURE);
        // The ceilings E must respect for a structural scan are frozen, not
        // chosen per rule.
        assert_eq!(
            CheckLimits::P0.ceiling(LimitKind::ScanBytes),
            64 * 1024 * 1024
        );
        assert_eq!(CheckLimits::P0.ceiling(LimitKind::Objects), 200_000);
        assert_eq!(CheckLimits::P0.ceiling(LimitKind::Depth), 64);
    }
}
