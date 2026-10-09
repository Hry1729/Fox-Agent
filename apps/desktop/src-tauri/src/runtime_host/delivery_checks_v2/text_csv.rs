//! Text merge / EOF equivalence and the one bounded delimited-table parser.
//!
//! ## What this implements (P0-2, plan §7)
//!
//! * [`check_text_concat`] — source set / order / headers / body / inter-segment
//!   blank line / final newline are checked *separately*, and the failure is
//!   located. A v1 envelope keeps the byte-exact v1 comparison **in full**: a
//!   mismatch stays a definite blocking `failed` and is never downgraded to
//!   `unverified`. Only a v2 envelope that declares
//!   `ComparisonMode::TextSegments` may treat "zero or one terminating LF" as
//!   equivalent, and when it does the equivalence is **recorded** on the passing
//!   observation rather than implied. Declared EOF / line-ending constraints are
//!   enforced on every candidate, including one that equals the construction byte
//!   for byte. No global `trim`, no dropping trailing whitespace, no implicit
//!   CRLF→LF conversion, and no normalisation that could ever produce a pass.
//! * [`parse_table`] — the single bounded CSV/TSV parser, i.e. the one definition
//!   of "record" and "field" for the basic check, the content check and row
//!   counting (F02). It returns each field's value, its position and whether it
//!   was quoted; it never converts a type and never drops a field.
//! * [`check_table_columns`] — real record-width disagreement, empty headers and
//!   duplicate headers only when the frozen rule *declares* uniqueness, explicit
//!   zero-record handling, and an explicit over-limit report.
//!
//! ## What this deliberately does not do
//!
//! No file writes, no model calls, no repair decision, no approval or version
//! gate. It never reports a parser refusal as wrong data: "this input cannot be
//! read under the declared dialect" and "this content disagrees with the
//! contract" are different findings with different codes.
//!
//! ## Wiring still owed to the integration layer
//!
//! `delivery.rs::csv_consistency` (currently `lines()` + `split(',')`, F02) and
//! `delivery_content_requirements.rs::delimited` must both call [`parse_table`]
//! so the basic check, the content check and row counting share this dialect.
//! That wiring is the integration step, not part of this module.

use super::*;

// ---------------------------------------------------------------------------
// Text merge / EOF equivalence
// ---------------------------------------------------------------------------

/// Number of trailing LF bytes. A lone trailing CR is not an LF.
fn trailing_lf_count(bytes: &[u8]) -> usize {
    bytes.iter().rev().take_while(|byte| **byte == b'\n').count()
}

/// The same slice with at most one trailing LF removed.
fn strip_one_trailing_lf(bytes: &[u8]) -> &[u8] {
    match bytes.last() {
        Some(b'\n') => &bytes[..bytes.len() - 1],
        _ => bytes,
    }
}

/// The same slice with every trailing LF removed. Used to tell "the terminating
/// newline count differs" from "some other byte differs".
fn trim_all_trailing_lf(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len() - trailing_lf_count(bytes)]
}

/// The authoritative expected byte construction.
///
/// This reproduces, byte for byte, what the v1 `Contract::Concat` writer
/// produced (`delivery_content_requirements.rs`), so:
///
/// * a v1 record keeps **exactly** its old semantics, and
/// * a v2 record differs from v1 only where its declared policy says it may.
fn expected_concat(segments: &[TextSegment<'_>], out: &mut Vec<u8>) {
    for (index, segment) in segments.iter().enumerate() {
        out.extend_from_slice(segment.file_name().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(segment.bytes);
        if index + 1 < segments.len() {
            if !segment.bytes.ends_with(b"\n") {
                out.push(b'\n');
            }
            out.push(b'\n');
        }
    }
}

/// The byte offset at which two slices first disagree, or `None` when one is a
/// prefix of the other. Bounded by the shorter slice.
fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    let common = left.len().min(right.len());
    (0..common).find(|index| left[*index] != right[*index])
}

/// A bounded, human-readable rendering of the bytes around an offset. Never the
/// whole artifact (plan §7 P0-1: "预期／实际" must be locatable but bounded).
fn window(bytes: &[u8], offset: usize) -> String {
    let start = offset.saturating_sub(16);
    let end = (offset + 16).min(bytes.len());
    let slice = &bytes[start..end];
    let mut rendered = String::new();
    for byte in slice {
        match byte {
            b'\n' => rendered.push_str("\\n"),
            b'\r' => rendered.push_str("\\r"),
            b'\t' => rendered.push_str("\\t"),
            0x20..=0x7e => rendered.push(*byte as char),
            _ => rendered.push_str(&format!("\\x{byte:02x}")),
        }
    }
    format!("字节 {offset} 附近「{rendered}」")
}

/// Locate every frozen header line (`name\n`) in the candidate, in frozen order.
///
/// Returns `None` when a frozen header cannot be found at or after the previous
/// one — that is "the frozen source set is not represented", which is a
/// different finding from "the bytes differ".
fn header_positions(actual: &[u8], segments: &[TextSegment<'_>]) -> Option<Vec<usize>> {
    let mut positions = Vec::with_capacity(segments.len());
    let mut cursor = 0usize;
    for segment in segments {
        let mut needle = segment.file_name().as_bytes().to_vec();
        needle.push(b'\n');
        let haystack = actual.get(cursor..)?;
        let found = haystack
            .windows(needle.len())
            .position(|window| window == needle.as_slice())?;
        let position = cursor + found;
        positions.push(position);
        cursor = position + needle.len();
    }
    Some(positions)
}

/// Build the failure that best describes *why* the candidate is not the expected
/// merge. The distinction matters for repair cost: "one terminating LF" must not
/// be reported as a whole-file content error (F01), and vice versa.
fn diagnose_concat(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
    segments: &[TextSegment<'_>],
    expected: &[u8],
    actual: &[u8],
) -> CheckObservation {
    // 1. Only the final newlines differ: by far the most likely cause of the O19
    //    false rejection, so it is named rather than folded into "格式不符".
    //    Covers 0 vs 1 vs 2+ trailing LFs: an extra blank line at end-of-file is
    //    an EOF finding, not a separator finding, and the repair it provokes must
    //    be correspondingly small.
    if trim_all_trailing_lf(actual) == trim_all_trailing_lf(expected) {
        let (actual_lf, expected_lf) = (trailing_lf_count(actual), trailing_lf_count(expected));
        let mut observation = failed(
            rule_id,
            check,
            reason_code::TEXT_EOF_MISMATCH,
            binding,
            format!(
                "正文与段间结构一致，只有末尾终止 LF 的数量不同：候选 {actual_lf} 个，期望 {expected_lf} 个；当前合同不允许这项等价"
            ),
        );
        observation.expected = Some(format!("末尾 LF {expected_lf} 个"));
        observation.actual = Some(format!("末尾 LF {actual_lf} 个"));
        observation.locator = Some(
            RecordLocator::default()
                .at_byte(strip_one_trailing_lf(expected).len() as u64),
        );
        return observation;
    }

    // 2. The frozen source set is not present at all, or not in the frozen order.
    match header_positions(actual, segments) {
        None => {
            // Every frozen header may still be present, just not in the frozen
            // order: a reordering, not a missing source. The search is bounded so
            // that locating *which* rule broke never costs more than the check.
            if actual.len() <= DIAGNOSIS_SCAN_BYTES
                && segments
                    .iter()
                    .all(|segment| header_present(actual, segment))
            {
                let frozen = segments
                    .iter()
                    .map(|segment| segment.file_name())
                    .collect::<Vec<_>>();
                let mut observation = failed(
                    rule_id,
                    check,
                    reason_code::TEXT_ORDER_MISMATCH,
                    binding,
                    format!(
                        "候选包含全部来源标题行，但顺序与冻结合并顺序不一致；冻结顺序：{}",
                        frozen.join("、")
                    ),
                );
                observation.expected = Some(frozen.join("\n"));
                observation.actual = Some(window(actual, 0));
                return observation;
            }
            let frozen_names = segments
                .iter()
                .map(|segment| segment.file_name())
                .collect::<Vec<_>>();
            // A CRLF-terminated header is the usual reason a frozen `name\n` is not
            // found. The byte comparison remains the authority; this only names the
            // cause and supplies a location instead of a generic "source missing".
            let crlf_header = segments.iter().find_map(|segment| {
                let mut needle = segment.file_name().as_bytes().to_vec();
                needle.extend_from_slice(b"\r\n");
                actual
                    .windows(needle.len())
                    .position(|window| window == needle.as_slice())
                    .map(|offset| (segment.file_name().to_owned(), offset))
            });
            let detail = match &crlf_header {
                Some((name, _)) => format!(
                    "候选没有按冻结顺序包含标题行「{name}」+LF；该标题在候选里以 CRLF 结尾，而冻结合并格式要求标题行用 LF。冻结顺序：{}",
                    frozen_names.join("、")
                ),
                None => format!(
                    "候选没有按冻结顺序包含全部来源标题行；冻结顺序：{}",
                    frozen_names.join("、")
                ),
            };
            let mut observation = failed(
                rule_id,
                check,
                reason_code::TEXT_SOURCE_MISSING,
                binding,
                detail,
            );
            observation.expected = Some(frozen_names.join("\n"));
            observation.actual = Some(window(actual, 0));
            observation.locator =
                crlf_header.map(|(_, offset)| RecordLocator::default().at_byte(offset as u64));
            return observation;
        }
        Some(_positions) => {
            // The headers do appear in the frozen order. Content before the first
            // one is reported below as a content difference, not as an ordering
            // change, so nothing further is decided here.
        }
    }

    // 3. Every segment body is byte-identical and the only extra bytes are
    //    newlines, so the inter-segment blank line structure differs.
    if only_separators_differ(actual, segments) {
        let mut observation = failed(
            rule_id,
            check,
            reason_code::TEXT_SEPARATOR_MISMATCH,
            binding,
            "各段正文与冻结来源逐字节一致，但段间空行（恰好一个空行）不符合冻结合并格式".to_owned(),
        );
        observation.expected = Some("段间恰好一个空行".to_owned());
        observation.actual = Some("段间空行数量不符".to_owned());
        return observation;
    }

    // 4. A real body difference. Locate the first differing byte.
    let offset = first_difference(expected, actual).unwrap_or(expected.len().min(actual.len()));
    let mut observation = failed(
        rule_id,
        check,
        reason_code::TEXT_CONTENT_MISMATCH,
        binding,
        format!(
            "候选正文与冻结来源不一致，首个差异在{}",
            window(expected, offset)
        ),
    );
    observation.expected = Some(window(expected, offset));
    observation.actual = Some(window(actual, offset));
    observation.locator = Some(RecordLocator::default().at_byte(offset as u64));
    observation
}

/// A diagnosis scan is bounded: locating *which* rule broke must never cost more
/// than the check itself. Beyond this many bytes the result is still located at
/// its first differing byte, but no reordered-header search is attempted.
const DIAGNOSIS_SCAN_BYTES: usize = 1 << 20;

/// True when the candidate contains the byte sequence `name\n` for a segment.
fn header_present(actual: &[u8], segment: &TextSegment<'_>) -> bool {
    let mut needle = segment.file_name().as_bytes().to_vec();
    needle.push(b'\n');
    actual
        .windows(needle.len())
        .any(|window| window == needle.as_slice())
}

/// True when every frozen payload appears verbatim at its header position, in
/// order, and the only bytes that differ anywhere are newlines.
///
/// This is exactly the "the segments are right, the blank-line structure is not"
/// case. It never grants a pass on its own — it only decides which of two
/// already-failing findings to report, so that a separator repair is not asked to
/// rewrite a body.
fn only_separators_differ(actual: &[u8], segments: &[TextSegment<'_>]) -> bool {
    // An empty selection has no separator structure at all, and without this guard
    // the loop below would be vacuously true and mislabel the difference as an
    // inter-segment blank-line problem.
    if segments.is_empty() {
        return false;
    }
    let Some(positions) = header_positions(actual, segments) else {
        return false;
    };
    for (index, (segment, position)) in segments.iter().zip(positions.iter()).enumerate() {
        let start = position + segment.file_name().len() + 1;
        let Some(payload) = actual.get(start..start + segment.bytes.len()) else {
            return false;
        };
        if payload != segment.bytes {
            return false;
        }
        // Between the end of this payload and the next header (or the end of the
        // file) only newline bytes may appear.
        let tail_start = start + segment.bytes.len();
        let tail_end = positions.get(index + 1).copied().unwrap_or(actual.len());
        if !actual[tail_start..tail_end]
            .iter()
            .all(|byte| *byte == b'\n')
        {
            return false;
        }
    }
    true
}

/// Check one frozen text-merge contract against the candidate bytes.
///
/// Returns exactly one observation. The caller folds it into the existing
/// `passed` / `failed` / `unverified` vocabulary through
/// [`verdict_from_observation`]; it does not decide repair.
///
/// ## Enforced ordering (every step exists because of a review finding)
///
/// 1. **v1 is the old comparison, in full.** A v1 record gets exactly
///    `bytes == expected`; a mismatch is a definite `failed` and is never
///    downgraded to `unverified` (F-P02-01). `freeze_concat` refuses any source
///    containing CR (`delivery_content_requirements.rs:309`), so a frozen v1
///    merge is CR-free and the comparison is fully decidable.
/// 2. **Declared constraints hold for every candidate**, including one that
///    equals the construction byte for byte (F-P02-03, F-P02-04).
/// 3. **A contradictory contract is reported, not waived.** If the frozen sources
///    themselves contradict the declared line-ending policy then no candidate can
///    satisfy both "sources verbatim" and the policy, so the result is
///    `unverified` with an explicit conflict diagnostic.
/// 4. **A known failure is never demoted.** The line-ending "cannot decide"
///    verdict is reported only when the bytes do not already prove a difference
///    that line endings cannot explain (F-P02-02).
/// 5. **An equivalence is recorded only when the pass relied on it** (F-P02-06).
pub(crate) fn check_text_concat(request: &TextConcatRequest<'_>) -> CheckObservation {
    let check = check_name::TEXT_CONCAT;
    let policy = &request.envelope.policy;
    let declared = policy.table.line_ending;

    // The expected merge, byte for byte from the v1 writer. An empty selection is
    // excluded by the freeze precondition: `delivery_content_requirements.rs:284-285`
    // keeps the selection count inside `1..=64`, and `:313`/`:316` make
    // `selected.len() == count`. The genuine "not bound" outcome is the separate
    // `Contract::Unbound` variant (`:626` → `Unverified`), which must not be
    // conflated with an empty `Concat`. If an empty selection ever did occur, the
    // construction yields an empty expectation exactly as the old code did.
    let mut expected = Vec::new();
    expected_concat(request.segments, &mut expected);

    let diagnose = |expected: &[u8], actual: &[u8]| {
        diagnose_concat(
            request.rule_id,
            check,
            request.binding,
            request.segments,
            expected,
            actual,
        )
    };

    // (1) v1: the old byte comparison, and nothing else.
    if request.envelope.is_v1() {
        return if request.actual == expected.as_slice() {
            passed(request.rule_id, check, request.binding)
        } else {
            diagnose(&expected, request.actual)
        };
    }

    // (3) A contract that contradicts itself cannot be decided for any candidate.
    if let LineEndingVerdict::Violation(detail) = line_ending_verdict(&expected, declared) {
        return unverified(
            request.rule_id,
            check,
            reason_code::TEXT_LINE_ENDING_UNSUPPORTED,
            request.binding,
            format!(
                "冻结合同自相矛盾：合并格式要求来源原样（冻结来源已经{detail}），同时声明行结束符为 {declared:?}；两者无法同时满足。不修改来源、不静默放行，本项按未核验处理"
            ),
        );
    }

    // (2) The declared line-ending policy, enforced on the candidate itself and
    //     therefore *before* any byte-equality shortcut.
    let candidate_endings = line_ending_verdict(request.actual, declared);
    if let LineEndingVerdict::Violation(detail) = &candidate_endings {
        let mut observation = failed(
            request.rule_id,
            check,
            reason_code::TEXT_LINE_ENDING_VIOLATION,
            request.binding,
            format!("候选违反冻结合同声明的行结束符政策：{detail}"),
        );
        observation.expected = Some(format!("行结束符符合声明 {declared:?}"));
        observation.actual = Some(window(request.actual, 0));
        return observation;
    }

    // (2) A declared final-newline *requirement* also holds for every candidate,
    //     so it too runs before the byte-equality shortcut: `expected_concat` never
    //     adds a trailing LF of its own, so a candidate can match the construction
    //     exactly and still miss the LF the contract demands.
    let actual_lf = trailing_lf_count(request.actual);
    let expected_lf = trailing_lf_count(&expected);
    if policy.eof == EofPolicy::RequireTrailingLf && actual_lf != 1 {
        let mut observation = failed(
            request.rule_id,
            check,
            reason_code::TEXT_EOF_MISMATCH,
            request.binding,
            format!("冻结合同要求恰好一个终止 LF，候选有 {actual_lf} 个"),
        );
        observation.expected = Some("末尾 LF 1 个".to_owned());
        observation.actual = Some(format!("末尾 LF {actual_lf} 个"));
        return observation;
    }

    // (4) Byte-identical: nothing was tolerated, so nothing is recorded.
    if request.actual == expected.as_slice() {
        return passed(request.rule_id, check, request.binding);
    }

    // (4) Line endings the declaration does not decide. Reported as undecidable
    //     *only* when nothing else is already wrong: `differs_beyond_line_endings`
    //     is used here to choose between `failed` and `unverified`, and can never
    //     produce a pass.
    if let LineEndingVerdict::Unsupported(detail) = candidate_endings {
        if !differs_beyond_line_endings(&expected, request.actual) {
            return unverified(
                request.rule_id,
                check,
                reason_code::TEXT_LINE_ENDING_UNSUPPORTED,
                request.binding,
                detail,
            );
        }
        // Otherwise fall through: the bytes prove a difference that line endings
        // cannot explain, and that definite failure must keep blocking.
    }

    // A v2 envelope that did not opt into segment comparison keeps the strict
    // comparison; only the *diagnosis* is sharper.
    if policy.comparison != ComparisonMode::TextSegments {
        return diagnose(&expected, request.actual);
    }

    // (5) The one tolerated equivalence: at most one terminating LF on each side.
    //     Two or more is an extra blank line, which is never equivalent.
    let equivalence_available = actual_lf <= 1 && expected_lf <= 1;
    let bodies_equal = strip_one_trailing_lf(request.actual) == strip_one_trailing_lf(&expected);

    match policy.eof {
        EofPolicy::ByteExact | EofPolicy::SourceVerbatim => {}
        EofPolicy::AllowSingleTrailingLf if equivalence_available && bodies_equal => {
            return passed_with_equivalence(
                request.rule_id,
                check,
                request.binding,
                equivalence::OPTIONAL_FINAL_LF,
                format!(
                    "正文与段间结构与冻结合并一致；按冻结合同允许的等价，末尾终止 LF 的有／无视为等价（候选 {actual_lf} 个，期望 {expected_lf} 个）"
                ),
            );
        }
        // The required LF was already enforced above, so this pass rests on the
        // contract's own requirement rather than on a tolerated difference.
        // Recording `optionalFinalLf` here would mislabel a satisfied hard
        // requirement as an optional equivalence (F-P02-06).
        EofPolicy::RequireTrailingLf if equivalence_available && bodies_equal => {
            return passed(request.rule_id, check, request.binding);
        }
        EofPolicy::AllowSingleTrailingLf | EofPolicy::RequireTrailingLf => {}
    }

    diagnose(&expected, request.actual)
}

// ---------------------------------------------------------------------------
// Declared line endings (R05)
// ---------------------------------------------------------------------------

enum LineEndingVerdict {
    Ok,
    /// The bytes contradict a declaration the frozen contract actually made.
    Violation(String),
    /// Nothing in the declaration decides these bytes, so no faithful comparison
    /// exists. Reported as `unverified`, never as a pass or a content error.
    Unsupported(String),
}

/// Classify bytes against the declared line-ending policy.
///
/// * `Violation` — the bytes contradict a policy the frozen contract actually
///   declared. A declared policy is enforced.
/// * `Unsupported` — nothing the contract declared decides these bytes, so no
///   faithful comparison exists without converting them, which the plan forbids.
///   Reported as `unverified`, never as a pass and never as a content error.
///
/// A v1 envelope is handled before this is ever called, so this only sees v2.
/// `LineEnding::Any` permits a byte *kind* to appear; it does not declare that two
/// different kinds are equal, so a CRLF/LF difference stays undecidable.
fn line_ending_verdict(bytes: &[u8], declared: LineEnding) -> LineEndingVerdict {
    let mut has_crlf = false;
    let mut has_bare_lf = false;
    let mut lone_cr: Option<usize> = None;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    has_crlf = true;
                    index += 2;
                    continue;
                }
                lone_cr = Some(index);
                break;
            }
            b'\n' => has_bare_lf = true,
            _ => {}
        }
        index += 1;
    }

    if let Some(offset) = lone_cr {
        // No variant of `LineEnding` declares a bare CR, and guessing that it
        // means a line break would be exactly the silent conversion the plan
        // forbids.
        return LineEndingVerdict::Unsupported(format!(
            "字节 {offset} 存在孤立的 CR（既不与 LF 组成 CRLF，也不被任何行结束声明覆盖）；不做隐式转换，本项按未核验处理"
        ));
    }

    match declared {
        LineEnding::Any => {
            if has_crlf {
                LineEndingVerdict::Unsupported(
                    "冻结合同声明行结束符为任意：CRLF 允许出现，但“允许出现某种字节”不等于“两种字节相等”，在不得转换字节的前提下无法判定相等"
                        .to_owned(),
                )
            } else {
                LineEndingVerdict::Ok
            }
        }
        LineEnding::Lf => {
            if has_crlf {
                LineEndingVerdict::Violation("含 CRLF".to_owned())
            } else {
                LineEndingVerdict::Ok
            }
        }
        LineEnding::Crlf => {
            if has_bare_lf {
                LineEndingVerdict::Violation("含单独的 LF".to_owned())
            } else {
                LineEndingVerdict::Ok
            }
        }
    }
}

/// Line endings mapped to a single LF so that a difference which is *only* the
/// LF / CRLF / CR convention can be recognised.
///
/// **Invariant**: this exists solely to choose between `failed` and `unverified`.
/// It can never produce a pass — a pass comes either from a byte-equal comparison
/// or from an equivalence the frozen contract explicitly declares. The artifact is
/// never rewritten or "fixed" from this result; the plan forbids normalising it.
fn normalize_line_endings(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                out.push(b'\n');
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    out
}

/// True when the two sides differ in something a line-ending convention cannot
/// explain, i.e. the difference is a definite content difference.
fn differs_beyond_line_endings(left: &[u8], right: &[u8]) -> bool {
    normalize_line_endings(left) != normalize_line_endings(right)
}

// ---------------------------------------------------------------------------
// The one bounded delimited-table parser (F02)
// ---------------------------------------------------------------------------

/// Parser state. A struct rather than nested closures so every push can enforce
/// the frozen ceilings at the exact point a limit is exceeded (plan §7 P0-2:
/// "解析器超预算返回明确限制，不循环调用模型").
struct TableBuilder<'a> {
    dialect: &'a TableDialect,
    limits: &'a CheckLimits,
    delimiter: char,
    quoting: bool,
    records: Vec<TableRecord>,
    fields: Vec<TableField>,
    field: String,
    field_quoted: bool,
    in_quotes: bool,
    after_quote: bool,
    line: u64,
    record_line: u64,
    at_record_start: bool,
    record_bytes: usize,
    total_text: usize,
    cells: usize,
}

impl<'a> TableBuilder<'a> {
    fn new(dialect: &'a TableDialect, limits: &'a CheckLimits) -> Self {
        Self {
            dialect,
            limits,
            delimiter: dialect.delimiter.as_char(),
            quoting: dialect.quote == QuotePolicy::DoubleQuote,
            records: Vec::new(),
            fields: Vec::new(),
            field: String::new(),
            field_quoted: false,
            in_quotes: false,
            after_quote: false,
            line: 1,
            record_line: 1,
            at_record_start: true,
            record_bytes: 0,
            total_text: 0,
            cells: 0,
        }
    }

    fn locator(&self) -> RecordLocator {
        RecordLocator::record(self.records.len() as u64 + 1).at_line(self.record_line)
    }

    fn mark_record_start(&mut self) {
        if self.at_record_start {
            self.record_line = self.line;
            self.at_record_start = false;
        }
    }

    fn count_bytes(&mut self, bytes: usize) -> Result<(), TableError> {
        self.record_bytes += bytes;
        if self.record_bytes > self.limits.max_line_bytes {
            return Err(TableError::budget(
                LimitKind::LineBytes,
                self.limits.max_line_bytes as u64,
                self.record_bytes as u64,
                self.locator(),
            ));
        }
        Ok(())
    }

    fn push_char(&mut self, ch: char) -> Result<(), TableError> {
        self.mark_record_start();
        self.count_bytes(ch.len_utf8())?;
        self.field.push(ch);
        Ok(())
    }

    fn push_field(&mut self) -> Result<(), TableError> {
        self.count_bytes(1)?;
        self.total_text += self.field.len();
        if self.total_text > self.limits.max_text_bytes {
            return Err(TableError::budget(
                LimitKind::TextBytes,
                self.limits.max_text_bytes as u64,
                self.total_text as u64,
                self.locator(),
            ));
        }
        self.cells += 1;
        if self.cells > self.limits.max_cells {
            return Err(TableError::budget(
                LimitKind::Cells,
                self.limits.max_cells as u64,
                self.cells as u64,
                self.locator(),
            ));
        }
        if self.fields.len() + 1 > self.limits.max_fields {
            return Err(TableError::budget(
                LimitKind::Fields,
                self.limits.max_fields as u64,
                (self.fields.len() + 1) as u64,
                self.locator(),
            ));
        }
        self.fields.push(TableField {
            field: self.fields.len() as u64 + 1,
            value: std::mem::take(&mut self.field),
            quoted: self.field_quoted,
        });
        self.field_quoted = false;
        Ok(())
    }

    fn push_record(&mut self, blank: bool) -> Result<(), TableError> {
        self.push_field()?;
        self.records.push(TableRecord {
            record: self.records.len() as u64 + 1,
            line: self.record_line,
            fields: std::mem::take(&mut self.fields),
            blank,
        });
        if self.records.len() > self.limits.max_records {
            return Err(TableError::budget(
                LimitKind::Records,
                self.limits.max_records as u64,
                self.records.len() as u64,
                self.locator(),
            ));
        }
        self.at_record_start = true;
        self.record_bytes = 0;
        Ok(())
    }

    /// True when the record accumulated so far is empty, i.e. the physical line
    /// that just ended carried no field content at all.
    fn record_is_empty(&self) -> bool {
        self.fields.is_empty() && self.field.is_empty() && !self.field_quoted
    }
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
    if bytes.len() as u64 > limits.max_bytes {
        return Err(TableError::budget(
            LimitKind::Bytes,
            limits.max_bytes,
            bytes.len() as u64,
            RecordLocator::default(),
        ));
    }

    // BOM handling is a declared policy, and a violation of it is an encoding
    // finding, not a width finding.
    let (body, bom_stripped) = match dialect.bom {
        BomPolicy::Auto => match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (rest, true),
            None => (bytes, false),
        },
        BomPolicy::Required => match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (rest, true),
            None => {
                return Err(TableError::new(
                    reason_code::TABLE_ENCODING_UNSUPPORTED,
                    RecordLocator::default(),
                    "冻结合同要求 UTF-8 BOM，候选没有".to_owned(),
                ))
            }
        },
        BomPolicy::Forbidden => {
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                return Err(TableError::new(
                    reason_code::TABLE_ENCODING_UNSUPPORTED,
                    RecordLocator::default(),
                    "冻结合同禁止 UTF-8 BOM，候选带 BOM".to_owned(),
                ));
            }
            (bytes, false)
        }
    };

    let text = std::str::from_utf8(body).map_err(|error| {
        TableError::new(
            reason_code::TABLE_ENCODING_UNSUPPORTED,
            RecordLocator::default().at_byte(error.valid_up_to() as u64),
            format!("候选不是合法 UTF-8：{error}"),
        )
    })?;

    // A lone CR is not representable in any declared dialect, and a declared
    // LF/CRLF policy must not be silently overridden.
    let mut index = 0usize;
    let raw = text.as_bytes();
    while index < raw.len() {
        match raw[index] {
            b'\r' => {
                if raw.get(index + 1) != Some(&b'\n') {
                    return Err(TableError::new(
                        reason_code::TABLE_DIALECT_UNSUPPORTED,
                        RecordLocator::default().at_byte(index as u64),
                        "候选存在孤立的 CR，声明方言无法安全解释；不做隐式转换".to_owned(),
                    ));
                }
                if dialect.line_ending == LineEnding::Lf {
                    return Err(TableError::new(
                        reason_code::TABLE_DIALECT_UNSUPPORTED,
                        RecordLocator::default().at_byte(index as u64),
                        "冻结合同声明行结束符为 LF，候选含 CRLF".to_owned(),
                    ));
                }
                index += 2;
                continue;
            }
            b'\n' => {
                if dialect.line_ending == LineEnding::Crlf && raw.get(index.wrapping_sub(1)) != Some(&b'\r') {
                    return Err(TableError::new(
                        reason_code::TABLE_DIALECT_UNSUPPORTED,
                        RecordLocator::default().at_byte(index as u64),
                        "冻结合同声明行结束符为 CRLF，候选含单独的 LF".to_owned(),
                    ));
                }
            }
            _ => {}
        }
        index += 1;
    }

    let mut builder = TableBuilder::new(dialect, limits);
    let mut iter = text.char_indices().peekable();
    let mut current = iter.next();

    while let Some((offset, ch)) = current {
        if builder.in_quotes {
            if ch == '"' {
                if let Some((_, '"')) = iter.peek().copied() {
                    iter.next();
                    builder.count_bytes(2)?;
                    builder.field.push('"');
                    current = iter.next();
                    continue;
                }
                builder.in_quotes = false;
                builder.after_quote = true;
                current = iter.next();
                continue;
            }
            if ch == '\n' {
                builder.line += 1;
            }
            builder.push_char(ch)?;
            current = iter.next();
            continue;
        }

        if builder.after_quote {
            if ch == builder.delimiter {
                builder.after_quote = false;
                builder.push_field()?;
                current = iter.next();
                continue;
            }
            if ch == '\n' {
                builder.after_quote = false;
                builder.push_record(false)?;
                builder.line += 1;
                current = iter.next();
                continue;
            }
            if ch == '\r' && raw.get(offset + 1) == Some(&b'\n') {
                // The LF branch handles the terminator; skip the CR.
                current = iter.next();
                continue;
            }
            return Err(TableError::new(
                reason_code::TABLE_CHARACTER_AFTER_QUOTE,
                builder.locator().at_byte(offset as u64),
                format!("引号闭合后出现未转义字符「{ch}」"),
            ));
        }

        match ch {
            '"' if builder.quoting && builder.field.is_empty() && !builder.field_quoted => {
                builder.mark_record_start();
                builder.count_bytes(1)?;
                builder.in_quotes = true;
                builder.field_quoted = true;
                current = iter.next();
            }
            '"' if builder.quoting => {
                return Err(TableError::new(
                    reason_code::TABLE_UNESCAPED_QUOTE,
                    builder.locator().at_byte(offset as u64),
                    "未加引号的字段中出现引号；若为字面引号须写作双引号并整体加引号".to_owned(),
                ));
            }
            c if c == builder.delimiter => {
                builder.mark_record_start();
                builder.push_field()?;
                current = iter.next();
            }
            '\n' => {
                if builder.record_is_empty() {
                    match dialect.blank_line {
                        BlankLinePolicy::Skip => {
                            builder.at_record_start = true;
                            builder.record_bytes = 0;
                        }
                        BlankLinePolicy::Record => {
                            builder.mark_record_start();
                            builder.push_record(true)?;
                        }
                    }
                } else {
                    builder.push_record(false)?;
                }
                builder.line += 1;
                current = iter.next();
            }
            '\r' => {
                // Part of a CRLF terminator the pre-scan already validated.
                current = iter.next();
            }
            c => {
                builder.push_char(c)?;
                current = iter.next();
            }
        }
    }

    if builder.in_quotes {
        return Err(TableError::new(
            reason_code::TABLE_UNTERMINATED_QUOTE,
            builder.locator(),
            "候选在文件结束前仍有未闭合的引号字段".to_owned(),
        ));
    }

    // A final record without a terminating newline is still a record.
    if builder.after_quote || !builder.record_is_empty() {
        builder.after_quote = false;
        builder.push_record(false)?;
    }

    Ok(ParsedTable {
        records: builder.records,
        dialect: dialect.clone(),
        bom_stripped,
    })
}

/// Check a parsed table's headers and record widths against a frozen rule.
pub(crate) fn check_table_columns(request: &TableCheckRequest<'_>) -> CheckObservation {
    let check = check_name::TABLE_COLUMNS;
    let parsed = request.parsed;
    let binding = request.binding;

    if parsed.records.is_empty() {
        let mut observation = failed(
            request.rule_id,
            check,
            reason_code::TABLE_ZERO_RECORDS,
            binding,
            "候选没有任何记录（连表头都没有）".to_owned(),
        );
        observation.expected = Some("至少一条记录".to_owned());
        observation.actual = Some("0 条记录".to_owned());
        return observation;
    }

    let header = parsed.header();
    if parsed.dialect.header == HeaderPolicy::Required {
        let Some(header) = header else {
            return failed(
                request.rule_id,
                check,
                reason_code::TABLE_ZERO_RECORDS,
                binding,
                "冻结合同要求表头，但候选没有记录".to_owned(),
            );
        };

        // Empty header names are reported only because a header exists at all.
        if let Some(empty) = header.fields.iter().find(|field| field.value.trim().is_empty()) {
            let mut observation = failed(
                request.rule_id,
                check,
                reason_code::TABLE_HEADER_EMPTY,
                binding,
                format!("表头第 {} 个字段为空", empty.field),
            );
            observation.locator = Some(
                RecordLocator::cell(header.record, empty.field, None).at_line(header.line),
            );
            observation.expected = Some("非空列名".to_owned());
            observation.actual = Some("空列名".to_owned());
            return observation;
        }

        // Column-name uniqueness is enforced **only** when the frozen rule
        // declares it (R08: an undeclared uniqueness demand must not be added).
        if request.require_unique_headers {
            let mut seen: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
            for field in &header.fields {
                if let Some(first) = seen.insert(field.value.as_str(), field.field) {
                    let mut observation = failed(
                        request.rule_id,
                        check,
                        reason_code::TABLE_HEADER_DUPLICATE,
                        binding,
                        format!(
                            "冻结合同要求列名唯一，列名「{}」在第 {} 与第 {} 个字段重复",
                            field.value, first, field.field
                        ),
                    );
                    observation.locator = Some(
                        RecordLocator::cell(header.record, field.field, Some(&field.value))
                            .at_line(header.line),
                    );
                    observation.expected = Some("每个列名出现一次".to_owned());
                    observation.actual = Some(format!("列名「{}」出现两次", field.value));
                    return observation;
                }
            }
        }
    }

    let reference = match parsed.dialect.header {
        HeaderPolicy::Required => parsed
            .header()
            .map(|header| (header.record, header.line, header.fields.len())),
        HeaderPolicy::Optional => parsed
            .records
            .first()
            .map(|record| (record.record, record.line, record.fields.len())),
    };
    let Some((ref_record, ref_line, ref_width)) = reference else {
        return failed(
            request.rule_id,
            check,
            reason_code::TABLE_ZERO_RECORDS,
            binding,
            "候选没有可作为宽度基准的记录".to_owned(),
        );
    };

    let data = parsed.data_records();
    if data.is_empty() && !request.allow_zero_records {
        let mut observation = failed(
            request.rule_id,
            check,
            reason_code::TABLE_ZERO_RECORDS,
            binding,
            "候选只有表头没有数据记录，而冻结合同未声明允许零记录".to_owned(),
        );
        observation.expected = Some("至少一条数据记录".to_owned());
        observation.actual = Some("0 条数据记录".to_owned());
        observation.locator = Some(RecordLocator::record(ref_record).at_line(ref_line));
        return observation;
    }

    for record in data {
        if record.fields.len() != ref_width {
            let mut observation = failed(
                request.rule_id,
                check,
                reason_code::TABLE_WIDTH_MISMATCH,
                binding,
                format!(
                    "第 {} 条记录（第 {} 行）有 {} 个字段，基准有 {} 个",
                    record.record,
                    record.line,
                    record.fields.len(),
                    ref_width
                ),
            );
            observation.locator = Some(RecordLocator::record(record.record).at_line(record.line));
            observation.expected = Some(format!("{ref_width} 个字段"));
            observation.actual = Some(format!("{} 个字段", record.fields.len()));
            return observation;
        }
    }

    passed(request.rule_id, check, binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/report.csv", "b".repeat(64), 42)
    }

    fn segment<'a>(path: &'a str, bytes: &'a [u8]) -> TextSegment<'a> {
        TextSegment { path, bytes }
    }

    fn v2_text(eof: EofPolicy) -> RuleEnvelope {
        RuleEnvelope::v2(
            "content:concat",
            RulePolicy {
                comparison: ComparisonMode::TextSegments,
                eof,
                table: TableDialect::default(),
            },
        )
    }

    fn check<'a>(
        envelope: &'a RuleEnvelope,
        segments: &'a [TextSegment<'a>],
        actual: &'a [u8],
    ) -> CheckObservation {
        check_text_concat(&TextConcatRequest {
            rule_id: "content:concat",
            envelope,
            segments,
            actual,
            binding: &binding(),
        })
    }

    // ---- R01: only the terminating LF differs, and the contract allows it ----

    #[test]
    fn r01_single_optional_terminating_lf_passes_and_records_the_equivalence() {
        // The last source has no final LF, so the frozen merge has none either.
        let segments = [segment("in/a.txt", b"alpha"), segment("in/b.txt", b"beta")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        let without = check(&envelope, &segments, b"a.txt\nalpha\n\nb.txt\nbeta");
        let with = check(&envelope, &segments, b"a.txt\nalpha\n\nb.txt\nbeta\n");

        // A byte-identical merge is a plain pass: nothing had to be tolerated, so
        // there is no equivalence to record. The observation must not claim one.
        assert_eq!(without.state, CheckState::Passed);
        assert!(!without.blocking, "a pass never arms a repair");
        assert!(
            without.equivalence.is_none(),
            "a byte-identical match relies on no equivalence"
        );

        // The other spelling differs from the frozen merge only by one terminating
        // LF, so it passes *because* the frozen contract allows that equivalence.
        // That reliance must be recorded rather than implied (R01).
        assert_eq!(with.state, CheckState::Passed);
        assert!(!with.blocking, "a pass never arms a repair");
        assert_eq!(
            with.equivalence.as_deref(),
            Some(equivalence::OPTIONAL_FINAL_LF),
            "the allowed equivalence must be recorded, not implied"
        );
        assert_eq!(with.reason_code, None, "a pass carries no reason code");
    }

    #[test]
    fn r01_a_v1_envelope_keeps_byte_exact_semantics_and_stays_strict() {
        let segments = [segment("in/a.txt", b"alpha")];
        let observation = check(&RuleEnvelope::v1(), &segments, b"a.txt\nalpha\n");
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH),
            "a v1 record must not gain the new equivalence"
        );
        assert!(observation.blocking);
    }

    // ---- R02: byte-exact stays strict, and two extra LFs are never equivalent ----

    #[test]
    fn r02_byte_exact_and_more_than_one_lf_are_refused_with_an_exact_eof_difference() {
        let segments = [segment("in/a.txt", b"alpha")];

        let byte_exact = check(&RuleEnvelope::v1(), &segments, b"a.txt\nalpha\n\n");
        assert_eq!(byte_exact.state, CheckState::Failed);
        assert_eq!(
            byte_exact.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH)
        );
        assert_eq!(byte_exact.expected.as_deref(), Some("末尾 LF 0 个"));
        assert_eq!(byte_exact.actual.as_deref(), Some("末尾 LF 2 个"));

        // The same two extra LFs under the permissive policy: still refused,
        // because the tolerance is exactly one LF.
        let permissive = check(&v2_text(EofPolicy::AllowSingleTrailingLf), &segments, b"a.txt\nalpha\n\n");
        assert_eq!(permissive.state, CheckState::Failed);
        assert_ne!(permissive.equivalence.as_deref(), Some(equivalence::OPTIONAL_FINAL_LF));
    }

    #[test]
    fn r02_require_trailing_lf_demands_exactly_one() {
        let segments = [segment("in/a.txt", b"alpha\n")];
        let envelope = v2_text(EofPolicy::RequireTrailingLf);

        assert_eq!(
            check(&envelope, &segments, b"a.txt\nalpha\n").state,
            CheckState::Passed
        );
        let missing = check(&envelope, &segments, b"a.txt\nalpha");
        assert_eq!(missing.state, CheckState::Failed);
        assert_eq!(
            missing.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH)
        );
    }

    // ---- R03: real body changes are never masked by trimming ----

    #[test]
    fn r03_content_changes_fail_even_when_a_trim_would_hide_them() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        for actual in [
            &b"a.txt\nalph"[..],           // a character deleted
            &b"a.txt\nalpha "[..],         // trailing whitespace added (trim-equal)
            &b"a.txt\n alpha"[..],         // indentation added
            &b"a.txt\nalphA"[..],          // a digit/letter changed
        ] {
            let observation = check(&envelope, &segments, actual);
            assert_eq!(
                observation.state,
                CheckState::Failed,
                "{actual:?} must not pass"
            );
            assert_eq!(
                observation.reason_code.as_deref(),
                Some(reason_code::TEXT_CONTENT_MISMATCH),
                "{actual:?} must be located as a content difference"
            );
            assert!(observation.locator.is_some(), "the difference must be located");
        }
    }

    #[test]
    fn r03_a_deleted_inner_blank_line_is_a_separator_difference() {
        let segments = [segment("in/a.txt", b"alpha"), segment("in/b.txt", b"beta")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        // Correct: "a.txt\nalpha" + "\n" (body lacks LF) + "\n" + "b.txt\nbeta".
        let correct = b"a.txt\nalpha\n\nb.txt\nbeta";
        assert_eq!(check(&envelope, &segments, correct).state, CheckState::Passed);

        // The inter-segment blank line was dropped, but both payloads survive.
        let missing_blank = b"a.txt\nalpha\nb.txt\nbeta";
        let observation = check(&envelope, &segments, missing_blank);
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_SEPARATOR_MISMATCH),
            "a separator change must not be reported as a body change"
        );
    }

    // ---- R04: frozen selection and order, including the legal boundaries ----

    #[test]
    fn r04_a_reordered_selection_is_an_order_finding() {
        let segments = [segment("in/a.txt", b"alpha"), segment("in/b.txt", b"beta")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        // Frozen order is a then b; the candidate wrote b first.
        let swapped = b"b.txt\nbeta\n\na.txt\nalpha";
        let observation = check(&envelope, &segments, swapped);
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_ORDER_MISMATCH)
        );
    }

    #[test]
    fn r04_a_missing_source_is_a_source_finding_not_a_content_finding() {
        let segments = [segment("in/a.txt", b"alpha"), segment("in/b.txt", b"beta")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let only_a = b"a.txt\nalpha";
        let observation = check(&envelope, &segments, only_a);
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
    }

    // 期望变更记录（保留原样本，不静默修改）：
    //
    // 原测试 `r04_a_contract_without_sources_is_unverified_not_failed` 断言
    // `check(&envelope, &[], b"anything")` → `Unverified` / `CONTRACT_UNREADABLE`。
    // 该期望是我此前自行发明的第三种语义，**没有证据支持**，且偏离了旧 checker 对
    // 空期望的行为（旧 `Contract::Concat` 只做 `bytes == expected`）。
    //
    // 证据（本轮核对）：`delivery_content_requirements.rs:284-285` 把选择数量限制在
    // `1..=64`，`:313` 保证 `sources.len() >= count`、`:316` 取前 `count` 个，故
    // `selected.len() == count >= 1`，合法冻结的 `Concat` 不可能没有来源；真正的
    // "未绑定"是**另一个契约变体** `Contract::Unbound`（`:626` → `Unverified`），
    // 不能与空 `Concat` 混同。
    //
    // 该边界现由 `empty_selection_mirrors_the_old_byte_comparison` 唯一权威地固定，
    // 它同时覆盖空候选（`Passed`，与旧字节比较一致）与非空候选（`Failed`，且不得
    // 误报为分隔/顺序问题）。

    #[test]
    fn r04_chinese_names_and_size_ties_are_accepted() {
        // "报告.md" and "附录.md" are different names of equal byte length: a
        // naive byte-length sort would be unstable here, so the frozen order must
        // be honoured as given.
        let segments = [segment("in/报告.md", "正文".as_bytes()), segment("in/附录.md", b"x")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let actual = "报告.md\n正文\n\n附录.md\nx".as_bytes();
        let observation = check(&envelope, &segments, actual);
        assert_eq!(observation.state, CheckState::Passed);
        assert!(observation.equivalence.is_none(), "a byte-exact match needs no equivalence");
    }

    // ---- R05: declared line endings, never a silent conversion ----

    #[test]
    fn r05_crlf_against_an_lf_declaration_is_a_located_violation() {
        let segments = [segment("in/a.txt", b"alpha")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;
        let observation = check(&envelope, &segments, b"a.txt\r\nalpha");
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_VIOLATION)
        );
    }

    #[test]
    fn r05_an_undeclared_convention_is_never_silently_converted_and_never_a_false_reject() {
        let envelope = {
            let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
            envelope.policy.table.line_ending = LineEnding::Any;
            envelope
        };

        // A CRLF merge whose bytes match the frozen construction exactly still
        // passes: a byte-identical artifact needs no conversion at all.
        let crlf_segments = [segment("in/a.txt", b"alpha\r\n")];
        assert_eq!(
            check(&envelope, &crlf_segments, b"a.txt\nalpha\r\n").state,
            CheckState::Passed
        );

        // A CRLF candidate that does not match byte-for-byte is neither converted
        // to force a match nor accused of wrong content.
        let lf_segments = [segment("in/a.txt", b"alpha")];
        let observation = check(&envelope, &lf_segments, b"a.txt\r\nalpha");
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
        assert!(!observation.blocking);
    }

    #[test]
    fn r05_a_lone_cr_is_unverified_and_never_silently_converted() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let observation = check(&envelope, &segments, b"a.txt\ralpha");
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
        assert!(!observation.blocking);
    }

    // ---- R06/R07: one parser, real quoting, encoding and dialects ----

    #[test]
    fn r06_quoted_commas_escapes_and_embedded_newlines_are_single_fields() {
        let csv = b"name,note\nAlice,\"a,b\"\nBob,\"he said \"\"hi\"\"\"\nCara,\"line1\nline2\"\n";
        let parsed = parse_table(csv, &TableDialect::default(), &CheckLimits::P0).expect("parses");

        assert_eq!(parsed.records.len(), 4, "a quoted newline is not a record break");
        let alice = &parsed.records[1];
        assert_eq!(alice.fields.len(), 2, "a quoted comma is not a field break");
        assert_eq!(alice.fields[1].value, "a,b");
        assert!(alice.fields[1].quoted);
        assert_eq!(parsed.records[2].fields[1].value, "he said \"hi\"");
        let cara = &parsed.records[3];
        assert_eq!(cara.fields[1].value, "line1\nline2");
        assert_eq!(cara.line, 4, "the record reports the line it started on");
    }

    #[test]
    fn r07_bom_crlf_and_tsv_all_parse_under_their_own_dialect() {
        let bom_crlf = b"\xEF\xBB\xBFname,qty\r\nwidget,3\r\n";
        let parsed = parse_table(bom_crlf, &TableDialect::default(), &CheckLimits::P0).expect("parses");
        assert!(parsed.bom_stripped);
        assert_eq!(parsed.header().unwrap().fields[0].value, "name");
        assert_eq!(parsed.data_records().len(), 1);

        let tsv_dialect = TableDialect {
            delimiter: Delimiter::Tab,
            ..TableDialect::default()
        };
        let parsed = parse_table(b"a\tb\n1\t2\t3\n", &tsv_dialect, &CheckLimits::P0).expect("parses");
        assert_eq!(parsed.header().unwrap().fields.len(), 2);
        assert_eq!(parsed.data_records()[0].fields.len(), 3, "a real TSV width error");
    }

    #[test]
    fn r07_a_required_bom_that_is_absent_is_an_encoding_finding() {
        let dialect = TableDialect {
            bom: BomPolicy::Required,
            ..TableDialect::default()
        };
        let error = parse_table(b"a,b\n", &dialect, &CheckLimits::P0).expect_err("no BOM");
        assert_eq!(error.code, reason_code::TABLE_ENCODING_UNSUPPORTED);
    }

    // ---- R08: real width errors, unclosed quotes, declared uniqueness only ----

    #[test]
    fn r08_a_real_eight_versus_seven_width_error_is_located_by_record() {
        // This is the O20 shape: 8 header names, a 7-field row.
        let csv = b"name,email,phone,company,title,city,country,industry\n\
                    Ann,a@x,1,Acme,CTO,SH,CN\n";
        let dialect = TableDialect::default();
        let parsed = parse_table(csv, &dialect, &CheckLimits::P0).expect("parses");
        assert_eq!(parsed.header().unwrap().fields.len(), 8);

        let observation = check_table_columns(&TableCheckRequest {
            rule_id: "content:contacts",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: false,
        });
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TABLE_WIDTH_MISMATCH)
        );
        assert_eq!(observation.expected.as_deref(), Some("8 个字段"));
        assert_eq!(observation.actual.as_deref(), Some("7 个字段"));
        assert_eq!(observation.locator.as_ref().unwrap().record, Some(2));
        assert_eq!(observation.locator.as_ref().unwrap().line, Some(2));

        // The invalid input is refused, and a run with the same width passes.
        let ok = b"name,email,phone,company,title,city,country,industry\nAnn,a@x,1,Acme,CTO,SH,CN,X\n";
        let parsed = parse_table(ok, &dialect, &CheckLimits::P0).expect("parses");
        assert_eq!(
            check_table_columns(&TableCheckRequest {
                rule_id: "content:contacts",
                parsed: &parsed,
                binding: &binding(),
                require_unique_headers: false,
                allow_zero_records: false,
            })
            .state,
            CheckState::Passed
        );
    }

    #[test]
    fn r08_an_unterminated_quote_is_a_parse_finding_not_a_width_finding() {
        let error = parse_table(b"a,b\n\"unclosed\n", &TableDialect::default(), &CheckLimits::P0)
            .expect_err("unterminated");
        assert_eq!(error.code, reason_code::TABLE_UNTERMINATED_QUOTE);
    }

    #[test]
    fn r08_an_unescaped_quote_is_reported_with_its_position() {
        let error = parse_table(b"a,b\nab\"cd,e\n", &TableDialect::default(), &CheckLimits::P0)
            .expect_err("unescaped");
        assert_eq!(error.code, reason_code::TABLE_UNESCAPED_QUOTE);
        assert!(error.locator.byte_offset.is_some());
    }

    #[test]
    fn r08_duplicate_headers_are_refused_only_when_the_rule_declares_uniqueness() {
        let parsed =
            parse_table(b"name,name\n1,2\n", &TableDialect::default(), &CheckLimits::P0).expect("parses");

        let undeclared = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: false,
        });
        assert_eq!(
            undeclared.state,
            CheckState::Passed,
            "an undeclared uniqueness demand must not be added"
        );

        let declared = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: true,
            allow_zero_records: false,
        });
        assert_eq!(declared.state, CheckState::Failed);
        assert_eq!(
            declared.reason_code.as_deref(),
            Some(reason_code::TABLE_HEADER_DUPLICATE)
        );
        assert_eq!(declared.locator.as_ref().unwrap().field, Some(2));
    }

    #[test]
    fn r08_an_empty_header_field_is_reported() {
        let parsed =
            parse_table(b"name,\n1,2\n", &TableDialect::default(), &CheckLimits::P0).expect("parses");
        let observation = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: false,
        });
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TABLE_HEADER_EMPTY)
        );
        assert_eq!(observation.locator.as_ref().unwrap().field, Some(2));
    }

    // ---- R09: blank lines, zero records and explicit declarations ----

    #[test]
    fn r09_blank_lines_are_not_counted_as_wrong_width_records() {
        let dialect = TableDialect::default();
        assert_eq!(dialect.blank_line, BlankLinePolicy::Skip);
        let parsed = parse_table(b"a,b\n1,2\n\n3,4\n", &dialect, &CheckLimits::P0).expect("parses");
        assert_eq!(parsed.records.len(), 3, "the blank line is not a record");
        assert_eq!(parsed.records[2].line, 4, "line numbers stay physical");

        let observation = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: false,
        });
        assert_eq!(observation.state, CheckState::Passed);
    }

    #[test]
    fn r09_zero_records_is_decided_by_the_declaration() {
        let parsed =
            parse_table(b"a,b\n", &TableDialect::default(), &CheckLimits::P0).expect("parses");

        let disallowed = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: false,
        });
        assert_eq!(disallowed.state, CheckState::Failed);
        assert_eq!(
            disallowed.reason_code.as_deref(),
            Some(reason_code::TABLE_ZERO_RECORDS)
        );

        let allowed = check_table_columns(&TableCheckRequest {
            rule_id: "r",
            parsed: &parsed,
            binding: &binding(),
            require_unique_headers: false,
            allow_zero_records: true,
        });
        assert_eq!(allowed.state, CheckState::Passed);
    }

    #[test]
    fn r09_whitespace_only_fields_keep_their_value() {
        let parsed =
            parse_table(b"a,b\n\" \",2\n", &TableDialect::default(), &CheckLimits::P0).expect("parses");
        assert_eq!(
            parsed.data_records()[0].fields[0].value, " ",
            "the parser must not drop or trim a value"
        );
    }

    // ---- Resource ceilings: an over-limit parse is never a content error ----

    #[test]
    fn a_ceiling_is_reported_as_an_explicit_budget_stop() {
        let tight = CheckLimits {
            max_records: 2,
            ..CheckLimits::P0
        };
        let error = parse_table(b"a,b\n1,2\n3,4\n5,6\n", &TableDialect::default(), &tight)
            .expect_err("row ceiling");
        assert_eq!(error.code, reason_code::BUDGET_EXCEEDED);
        assert_eq!(error.limit, Some(LimitKind::Records));

        let wide = CheckLimits {
            max_fields: 2,
            ..CheckLimits::P0
        };
        let error = parse_table(b"a,b,c\n", &TableDialect::default(), &wide).expect_err("fields");
        assert_eq!(error.limit, Some(LimitKind::Fields));

        let small = CheckLimits {
            max_bytes: 4,
            ..CheckLimits::P0
        };
        let error = parse_table(b"a,b,c\nd,e,f\n", &TableDialect::default(), &small).expect_err("bytes");
        assert_eq!(error.limit, Some(LimitKind::Bytes));
    }

    // -----------------------------------------------------------------------
    // F-P02 定点反例（先于修正写出，用于在旧 HEAD 暴露控制流顺序缺陷）
    //
    // 编号对应 G-P02-review.md 的发现编号，便于独立复验逐条对照。这些期望不是
    // 从报告照抄的：每条都先自己推导旧 `Contract::Concat` / 冻结前置条件的实际
    // 行为，再写成断言。
    // -----------------------------------------------------------------------

    /// **F-P02-01**：v1 记录 + 含 CR 的字节不等候选，不得由确定失败降级为未核验。
    ///
    /// 依据（冻结前置条件，非推测）：`freeze_concat` 在
    /// `delivery_content_requirements.rs:309` 直接拒绝任何 `body.contains('\r')`
    /// 的来源，所以冻结来源永不含 CR，而**候选**可以。旧 `Concat` 分支只做
    /// `bytes == expected`，不等即 `Unmet` → 确定的阻断失败。
    ///
    /// 同时覆盖 v1/ByteExact 的相同字节与各类不同字节：尾部 LF、CRLF、孤立 CR、
    /// 真正文变化。
    #[test]
    fn f_p02_01_v1_keeps_a_total_byte_comparison() {
        let segments = [segment("in/a.txt", b"alpha")];
        let v1 = RuleEnvelope::v1();

        assert_eq!(
            check(&v1, &segments, b"a.txt\nalpha").state,
            CheckState::Passed,
            "the byte-identical candidate still passes"
        );

        for actual in [
            &b"a.txt\nalpha\n"[..], // one extra trailing LF
            &b"a.txt\r\nalpha"[..], // CRLF
            &b"a.txt\ralpha"[..],   // lone CR
            &b"a.txt\nalphX"[..],   // a real body change
        ] {
            let observation = check(&v1, &segments, actual);
            assert_eq!(
                observation.state,
                CheckState::Failed,
                "v1 must keep the old definite verdict for {actual:?}"
            );
            assert!(
                observation.blocking,
                "a v1 mismatch must block, not merely report: {actual:?}"
            );
        }
    }

    /// **F-P02-02 / F-P02-08**：行结束裁决不得抢先返回而吞掉已知正文错误。
    ///
    /// 反例是**可达**的：冻结来源 CR-free（同上 :309），候选含 CRLF。旧实现在
    /// 行结束裁决处直接 `unverified`，后面的正文诊断根本没跑。
    ///
    /// 修正后：仅当差异**确实只限行结束**时才允许未核验；否则已知正文差异仍然
    /// 阻断并保留定位。
    #[test]
    fn f_p02_02_a_known_body_error_behind_crlf_still_blocks() {
        let segments = [segment("in/a.txt", b"alpha")];
        // Default dialect declares `line_ending: Any`.
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        let body_error = check(&envelope, &segments, b"a.txt\r\nalphX");
        assert_eq!(
            body_error.state,
            CheckState::Failed,
            "a known body difference must not be demoted to unverified"
        );
        assert!(body_error.blocking, "and it must block");
        assert_ne!(
            body_error.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED),
            "the line-ending code must not stand in for a content finding"
        );
        assert!(
            body_error.locator.is_some() || body_error.actual.is_some(),
            "the difference must stay locatable"
        );

        // The genuinely line-ending-only case stays undecidable: `Any` permits the
        // byte kind, it does not grant byte equality.
        let only_endings = check(&envelope, &segments, b"a.txt\r\nalpha");
        assert_eq!(only_endings.state, CheckState::Unverified);
        assert_eq!(
            only_endings.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
        assert!(!only_endings.blocking);
    }

    /// **F-P02-03**：`RequireTrailingLf` 不得被"字节相同"早退绕过。
    ///
    /// 反例可达的原因在 `expected_concat`：最后一段**不补**终止 LF，所以来源无
    /// LF 时构造也没有。候选因此可以与构造逐字节相同，却违反"恰好一个终止 LF"。
    #[test]
    fn f_p02_03_require_trailing_lf_survives_byte_equality() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::RequireTrailingLf);

        let missing = check(&envelope, &segments, b"a.txt\nalpha");
        assert_eq!(
            missing.state,
            CheckState::Failed,
            "the declared requirement must be enforced even when the bytes match the construction"
        );
        assert_eq!(
            missing.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH)
        );
        assert!(missing.blocking);
    }

    /// **F-P02-04**：明确声明的行结束要求不得被"等于构造"豁免。
    ///
    /// 反例：冻结来源本身是 CRLF，于是构造（=来源原样）也含 CRLF；合同又声明
    /// `Lf`。候选与构造逐字节相同 → 旧实现 `Passed`。合同同时要求"来源原样"和
    /// 一个与来源不符的换行约束，属**自相矛盾**，必须明确报告，既不静默放行也
    /// 不丢弃来源。
    #[test]
    fn f_p02_04_a_contradictory_contract_is_reported_not_waived() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let actual = b"a.txt\nalpha\r\n"; // byte-identical to `expected_concat`
        let mut expected = Vec::new();
        expected_concat(&segments, &mut expected);
        assert_eq!(actual.as_slice(), expected.as_slice(), "the counterexample must be byte-identical");

        let observation = check(&envelope, &segments, actual);
        assert_ne!(
            observation.state,
            CheckState::Passed,
            "a declared policy must not be waived by byte equality"
        );
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
        assert!(!observation.blocking, "a contradictory contract is not a candidate error");
        assert!(
            observation
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("矛盾")),
            "the conflict must be named: {:?}",
            observation.detail
        );
    }

    /// **F-P02-06**：`RequireTrailingLf` 的通过不得记成 `optionalFinalLf`。
    ///
    /// G 报告原示例（来源自带 LF）实测是字节相同，走不到所谓"不等路径"；这里用
    /// **可达**反例：来源无终止 LF、候选恰好 1 个。
    #[test]
    fn f_p02_06_require_trailing_lf_never_claims_the_optional_equivalence() {
        let segments = [segment("in/a.txt", b"alpha")]; // no terminating LF in the source
        let envelope = v2_text(EofPolicy::RequireTrailingLf);

        let satisfied = check(&envelope, &segments, b"a.txt\nalpha\n");
        assert_eq!(satisfied.state, CheckState::Passed);
        assert!(
            !satisfied.blocking,
            "satisfying a declared requirement never arms a repair"
        );
        assert_eq!(
            satisfied.equivalence, None,
            "a satisfied hard requirement is not an optional tolerance"
        );
    }

    /// 空来源 / 空候选边界。
    ///
    /// **证据（说明为何这是防御分支而非可达状态）**：选择数量被限制为 `1..=64`
    /// （`delivery_content_requirements.rs:284-285` 的 `.filter(|n| *n > 0 && *n <= 64)`），
    /// 且 `:313` 保证 `sources.len() >= count`、`:316` 取前 `count` 个，所以
    /// `selected.len() == count >= 1`——**合法冻结的 `Concat` 不可能没有来源**。
    /// 真正的"未绑定"是**另一个契约变体** `Contract::Unbound`（`:626` →
    /// `Unverified(reason)`），不能与空 `Concat` 混同。
    ///
    /// 万一出现空选择，行为与旧 checker 一致：期望为空字节串，只有空候选通过。
    #[test]
    fn empty_selection_mirrors_the_old_byte_comparison() {
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        assert_eq!(
            check(&envelope, &[], b"").state,
            CheckState::Passed,
            "an empty expectation is satisfied only by an empty candidate, as before"
        );

        let non_empty = check(&envelope, &[], b"anything");
        assert_eq!(non_empty.state, CheckState::Failed);
        assert!(non_empty.blocking);
        assert_eq!(
            non_empty.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH),
            "an empty selection must not be reported as a separator or ordering problem"
        );
    }
}
