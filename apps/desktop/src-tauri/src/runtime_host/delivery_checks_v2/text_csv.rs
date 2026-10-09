//! Text merge / EOF equivalence and the one bounded delimited-table parser.
//!
//! ## Ownership
//!
//! **Owner from the C0 base commit: B (text and CSV).** A owns `mod.rs` and the
//! public DTOs; this file and its tests belong to B. B never edits `mod.rs`,
//! `delivery.rs`, `delivery_content_requirements.rs`, `data_compute.rs` or any
//! `Cargo.*`; a missing interface field is requested from A, who publishes a new
//! base commit.
//!
//! ## What B implements here (P0-2, plan §7)
//!
//! * [`check_text_concat`] — source set / order / headers / body / inter-segment
//!   blank line / final newline checked separately. A v1 envelope keeps the
//!   byte-exact v1 comparison; only a v2 envelope that declares
//!   `ComparisonMode::TextSegments` may treat "zero or one terminating LF" as
//!   equivalent. No global `trim`, no dropping trailing whitespace, no implicit
//!   CRLF→LF conversion.
//! * [`parse_table`] — the single bounded CSV/TSV parser. It is the definition
//!   of "record" and "field" for the basic check, the content check and row
//!   counting (F02). It returns the value, the position and whether the field was
//!   quoted; it never converts a type and never drops a field.
//! * [`check_table_columns`] — real record-width disagreement, duplicate or
//!   empty headers only when the frozen rule declares them, and an explicit
//!   over-limit report.
//!
//! ## What B does not do
//!
//! No file writes, no model calls, no repair decision, no approval or version
//! gate. B does not modify `data_compute.rs::parse_delimited` (A wires the shared
//! parser); B reports the wiring requirement in the hand-off instead.
//!
//! ## C0 state
//!
//! Every entry point below is a **placeholder** that reports
//! `delivery.check.checker_not_implemented` as `unverified`. Nothing in this file
//! is wired into a production code path yet, so nothing can pass by accident.

use super::*;

/// Check one frozen text-merge contract against the candidate bytes.
///
/// Returns exactly one observation. The caller folds it into the existing
/// `passed` / `failed` / `unverified` vocabulary through
/// [`verdict_from_observation`]; it does not decide repair.
pub(crate) fn check_text_concat(request: &TextConcatRequest<'_>) -> CheckObservation {
    // C0 placeholder — B replaces the body in P0. Kept honest on purpose: an
    // unimplemented checker never reports a pass.
    not_implemented(request.rule_id, check_name::TEXT_CONCAT, request.binding)
}

/// Parse a delimited table under an explicit dialect and the frozen ceilings.
///
/// Errors carry a stable [`reason_code`] and a [`RecordLocator`], so "the parser
/// refuses this input" and "the data disagrees with the contract" stay different
/// findings.
pub(crate) fn parse_table(
    bytes: &[u8],
    dialect: &TableDialect,
    limits: &CheckLimits,
) -> Result<ParsedTable, TableError> {
    let _ = (bytes, limits);
    // C0 placeholder — B replaces the body in P0.
    Err(TableError::new(
        reason_code::CHECKER_NOT_IMPLEMENTED,
        RecordLocator::default(),
        format!(
            "有界表格解析器尚未实现（delimiter={:?}）",
            dialect.delimiter
        ),
    ))
}

/// Check a parsed table's headers and record widths against a frozen rule.
pub(crate) fn check_table_columns(request: &TableCheckRequest<'_>) -> CheckObservation {
    // C0 placeholder — B replaces the body in P0.
    not_implemented(request.rule_id, check_name::TABLE_COLUMNS, request.binding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/report.csv", "b".repeat(64), 42)
    }

    /// This test documents the C0 state only. B replaces it with real coverage
    /// of terminating-LF equivalence, genuine whitespace/content changes, source
    /// ordering, a legal quoted CSV, a genuinely wrong-width TSV and an
    /// unterminated quote. B must keep one assertion per reason code it uses.
    #[test]
    fn c0_placeholders_report_unverified_and_never_a_pass() {
        let envelope = RuleEnvelope::v1();
        let segments = [TextSegment {
            path: "in/a.txt",
            bytes: b"a\n",
        }];
        let observation = check_text_concat(&TextConcatRequest {
            rule_id: "content:concat",
            envelope: &envelope,
            segments: &segments,
            actual: b"a.txt\na\n",
            binding: &binding(),
        });
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::CHECKER_NOT_IMPLEMENTED)
        );
        assert!(!observation.blocking);

        let error = parse_table(b"a,b\n1\n", &TableDialect::default(), &CheckLimits::P0)
            .expect_err("the C0 parser is a placeholder");
        assert_eq!(error.code, reason_code::CHECKER_NOT_IMPLEMENTED);
    }

    #[test]
    fn shared_dto_positions_and_segment_helpers_are_usable() {
        // The DTOs A froze are usable as-is, so B does not need a new base just
        // to build a locator or read a segment's frozen header name.
        let segment = TextSegment {
            path: "in/report.md",
            bytes: b"x",
        };
        assert_eq!(segment.file_name(), "report.md");
        assert_eq!(RecordLocator::cell(2, 1, Some("name")).record, Some(2));
        let parsed = ParsedTable {
            records: Vec::new(),
            dialect: TableDialect::default(),
            bom_stripped: false,
        };
        assert!(parsed.header().is_none());
        assert_eq!(
            serde_json::to_value(TableDialect::default()).unwrap()["delimiter"],
            json!("comma")
        );
    }
}
