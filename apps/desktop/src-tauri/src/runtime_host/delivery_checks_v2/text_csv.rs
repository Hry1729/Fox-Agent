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

use super::budget::{validate_utf8_metered, Budget, BudgetStop, Clock, Outcome, SystemClock};
#[cfg(test)]
use super::budget::Phase;
use super::*;

// ---------------------------------------------------------------------------
// Text merge / EOF equivalence
// ---------------------------------------------------------------------------

/// Number of trailing LF bytes. Metered variant: only the bytes of the
/// actual trailing run are read; a lone trailing CR is not an LF.
fn trailing_lf_count_metered(bytes: &[u8], budget: &mut Budget<'_>) -> Result<usize, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::TailLf);
    let mut count = 0usize;
    let len = bytes.len();
    while count < len {
        if budget.byte_at(bytes, len - 1 - count)? != b'\n' {
            break;
        }
        count += 1;
    }
    Ok(count)
}

/// The same slice with at most one trailing LF removed. Pure slicing (no input
/// read); the subsequent comparison is metered by the caller.
fn strip_one_trailing_lf(bytes: &[u8]) -> &[u8] {
    match bytes.last() {
        Some(b'\n') => &bytes[..bytes.len() - 1],
        _ => bytes,
    }
}

/// Metered variant of trimming every trailing LF: reads only the bytes of the
/// trailing run.
fn trim_all_trailing_lf_metered<'b>(
    bytes: &'b [u8],
    budget: &mut Budget<'_>,
) -> Result<&'b [u8], BudgetStop> {
    let run = trailing_lf_count_metered(bytes, budget)?;
    Ok(&bytes[..bytes.len() - run])
}

/// The authoritative expected byte construction.
///
/// This reproduces, byte for byte, what the v1 `Contract::Concat` writer
/// produced (`delivery_content_requirements.rs`), so:
///
/// * a v1 record keeps **exactly** its old semantics, and
/// * a v2 record differs from v1 only where its declared policy says it may.
///
/// Each entry is `name` + LF + the body copied verbatim, and adjacent entries
/// are separated by **exactly one blank line**. The separator therefore adds
/// one LF when the body already ended its own line (it ends with LF, or it is
/// empty and contributed no line at all), and two when a non-empty body left
/// its line unterminated — the first LF closes that line, the second is the
/// blank line. Treating a 0-byte body as "unterminated" would put *two* blank
/// lines between entries (D1).
fn expected_concat(segments: &[TextSegment<'_>], out: &mut Vec<u8>) {
    for (index, segment) in segments.iter().enumerate() {
        out.extend_from_slice(segment.file_name().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(segment.bytes);
        if index + 1 < segments.len() {
            if needs_terminator(segment.bytes) {
                out.push(b'\n');
            }
            out.push(b'\n');
        }
    }
}

/// True when a body must gain one LF before the separator blank line, because
/// it holds a line of its own that did not end with LF.
///
/// A 0-byte body contributes no line, so it never needs one: the entry is just
/// the header line, already terminated, and the separator supplies the blank
/// line on its own.
fn needs_terminator(body: &[u8]) -> bool {
    !body.is_empty() && !body.ends_with(b"\n")
}

/// Length of [`expected_concat`]'s output, computed from frozen metadata with
/// `checked_add` and **before** any allocation. Mirrors the writer byte for
/// byte: `Σ(h_i + 1 + b_i) + Σ_{i<n-1}(2 − e_i)` where `e_i` is 1 iff segment
/// `i` needs no extra terminator LF — that is, iff its body is empty or already
/// ends with LF. A 0-byte body is therefore charged **one** separator byte, not
/// two, exactly as the writer emits it (D1). The last segment adds no separator,
/// so a one-segment rule reads no body tail byte.
///
/// Overflow or an over-`max_text_bytes` result is a `TextBytes` stop.
fn expected_concat_len(
    segments: &[TextSegment<'_>],
    budget: &mut Budget<'_>,
) -> Result<usize, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::ConstructionLength);
    let limits = budget.limits();
    let ceiling = limits.max_text_bytes as u64;
    let mut total: u64 = 0;
    let n = segments.len();
    for (index, segment) in segments.iter().enumerate() {
        budget.check_time()?;
        let part = (segment.file_name().len() as u64)
            .checked_add(1)
            .and_then(|value| value.checked_add(segment.bytes.len() as u64))
            .ok_or_else(|| budget.text_length_overflow())?;
        total = total
            .checked_add(part)
            .ok_or_else(|| budget.text_length_overflow())?;
        if index + 1 < n {
            // An empty body has no tail byte to read and needs no terminator;
            // only a non-empty body obliges the scan.
            let terminated = segment.bytes.is_empty()
                || matches!(budget.last_byte(segment.bytes)?, Some(b'\n'));
            let separator = if terminated { 1 } else { 2 };
            total = total
                .checked_add(separator)
                .ok_or_else(|| budget.text_length_overflow())?;
        }
        if total > ceiling {
            return Err(BudgetStop {
                kind: LimitKind::TextBytes,
                observed: total,
                need: 0,
                next: total,
            });
        }
    }
    usize::try_from(total).map_err(|_| budget.text_length_overflow())
}

/// Build the expected merge under the `TextBytes` and `Millis` budgets. The
/// copy is chunked (≤8 KiB per extend) and time-checked per chunk; construction
/// bytes are not charged to `ScanBytes` (their size is already capped here).
fn build_expected_metered(
    segments: &[TextSegment<'_>],
    budget: &mut Budget<'_>,
) -> Result<Vec<u8>, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Construction);
    let length = expected_concat_len(segments, budget)?;
    budget.check_time()?;
    // One bounded, non-preemptible allocation. OOM is not promised catchable;
    // the honest boundary is the declared `max_text_bytes` ceiling.
    let mut out = Vec::with_capacity(length);
    const CHUNK: usize = 8 * 1024;
    for (index, segment) in segments.iter().enumerate() {
        budget.check_time()?;
        extend_chunked(&mut out, segment.file_name().as_bytes(), CHUNK, budget)?;
        out.push(b'\n');
        extend_chunked(&mut out, segment.bytes, CHUNK, budget)?;
        if index + 1 < segments.len() {
            if needs_terminator(segment.bytes) {
                out.push(b'\n');
            }
            out.push(b'\n');
        }
    }
    budget.check_time()?;
    debug_assert_eq!(out.len(), length);
    Ok(out)
}

/// `extend_from_slice` in ≤`chunk` pieces with a time gate between pieces. The
/// copied bytes are construction output (not charged to ScanBytes).
fn extend_chunked(
    out: &mut Vec<u8>,
    bytes: &[u8],
    chunk: usize,
    budget: &mut Budget<'_>,
) -> Result<(), BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::ConstructionCopy);
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        budget.check_time()?;
        let end = (cursor + chunk).min(bytes.len());
        out.extend_from_slice(&bytes[cursor..end]);
        cursor = end;
    }
    Ok(())
}

/// The byte offset at which two slices first disagree, or `None` when one is a
/// prefix of the other. Pairwise comparison is metered and stops at the first
/// differing pair.
fn first_difference_metered(
    left: &[u8],
    right: &[u8],
    budget: &mut Budget<'_>,
) -> Result<Option<usize>, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::RawCompare);
    if left.len() != right.len() {
        return Ok(None);
    }
    for index in 0..left.len() {
        let (a, b) = read_pair(budget, left, right, index)?;
        if a != b {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

/// One paired read at an index known to exist on both sides (2 logical bytes).
fn read_pair(
    budget: &mut Budget<'_>,
    left: &[u8],
    right: &[u8],
    index: usize,
) -> Result<(u8, u8), BudgetStop> {
    let permit = budget.pre_read_pair()?;
    let pair = (left[index], right[index]);
    budget.commit(permit);
    Ok(pair)
}

/// A bounded, human-readable rendering of the bytes around an offset. Never the
/// whole artifact. Every byte rendered is an actual (and charged) read; on a
/// budget refusal the caller degrades rather than scanning further.
fn window_metered(
    bytes: &[u8],
    offset: usize,
    budget: &mut Budget<'_>,
) -> Result<String, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Window);
    let start = offset.saturating_sub(16);
    let end = (offset + 16).min(bytes.len());
    let mut rendered = String::new();
    for index in start..end {
        let byte = budget.byte_at(bytes, index)?;
        match byte {
            b'\n' => rendered.push_str("\\n"),
            b'\r' => rendered.push_str("\\r"),
            b'\t' => rendered.push_str("\\t"),
            0x20..=0x7e => rendered.push(byte as char),
            _ => rendered.push_str(&format!("\\x{byte:02x}")),
        }
    }
    Ok(format!("字节 {offset} 附近「{rendered}」"))
}

/// Naive substring search with every haystack byte actually compared charged
/// to the budget. Frozen `needle` bytes are metadata (free); a failed match
/// re-reads haystack bytes on the next attempt, and that repeated read is
/// charged again. `start` bounds the search as the original cursor did.
fn find_needle_metered(
    haystack: &[u8],
    start: usize,
    needle: &[u8],
    budget: &mut Budget<'_>,
) -> Result<Option<usize>, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::HeaderSearch);
    if needle.is_empty() || start > haystack.len() {
        return Ok(None);
    }
    let mut position = start;
    while position + needle.len() <= haystack.len() {
        let mut matched = true;
        for (j, wanted) in needle.iter().enumerate() {
            let got = budget.byte_at(haystack, position + j)?;
            if &got != wanted {
                matched = false;
                break;
            }
        }
        if matched {
            return Ok(Some(position));
        }
        position += 1;
    }
    Ok(None)
}

/// Locate every frozen header line (`name\n`) in the candidate, in frozen order.
///
/// Returns `None` when a frozen header cannot be found at or after the previous
/// one — that is "the frozen source set is not represented", which is a
/// different finding from "the bytes differ".
fn header_positions_metered(
    actual: &[u8],
    segments: &[TextSegment<'_>],
    budget: &mut Budget<'_>,
) -> Result<Option<Vec<usize>>, BudgetStop> {
    let mut positions = Vec::with_capacity(segments.len());
    let mut cursor = 0usize;
    for segment in segments {
        let mut needle = segment.file_name().as_bytes().to_vec();
        needle.push(b'\n');
        let Some(relative) = find_needle_metered(actual, cursor, &needle, budget)? else {
            return Ok(None);
        };
        positions.push(relative);
        cursor = relative + needle.len();
    }
    Ok(Some(positions))
}

/// Build the failure that best describes *why* the candidate is not the expected
/// merge. The distinction matters for repair cost: "one terminating LF" must not
/// be reported as a whole-file content error (F01), and vice versa.
///
/// Every step is metered. A budget stop has two different meanings here:
///
/// * while the **fact** (which finding applies, its locator) is still being
///   established, the stop propagates and the caller keeps the generic
///   blocking failure with no fabricated classification/location;
/// * once a specific finding (EOF/ORDER/SOURCE/SEPARATOR/CONTENT) is proven
///   together with its locator, a later stop is allowed only in a **display
///   window** (`window_metered`): the rendering becomes `None` and the
///   already-proven code/locator is returned unchanged. Display reads never
///   add bytes to the proven fact or fabricate a fragment that was not read.
fn diagnose_concat_metered(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
    segments: &[TextSegment<'_>],
    expected: &[u8],
    actual: &[u8],
    budget: &mut Budget<'_>,
) -> Result<CheckObservation, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Diagnosis);
    // Best-effort human-readable rendering for an already-proven finding:
    // a display-only budget stop yields `None`, not a propagated error.
    let display = |bytes: &[u8], offset: usize, budget: &mut Budget<'_>| -> Option<String> {
        window_metered(bytes, offset, budget).ok()
    };

    // 1. Only the final newlines differ: the most likely O19 false rejection.
    let trimmed_actual = trim_all_trailing_lf_metered(actual, budget)?;
    let trimmed_expected = trim_all_trailing_lf_metered(expected, budget)?;
    if budget.slices_equal(trimmed_actual, trimmed_expected)? {
        let actual_lf = trailing_lf_count_metered(actual, budget)?;
        let expected_lf = trailing_lf_count_metered(expected, budget)?;
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
        return Ok(observation);
    }

    // 2. The frozen source set is not present at all, or not in frozen order.
    match header_positions_metered(actual, segments, budget)? {
        None => {
            // Every frozen header may still be present out of order: a bounded
            // reordering search gated by the existing 1 MiB input-length scope.
            if actual.len() <= DIAGNOSIS_SCAN_BYTES
                && headers_all_present_metered(actual, segments, budget)?
            {
                let frozen = segments
                    .iter()
                    .map(|segment| segment.file_name())
                    .collect::<Vec<_>>();
                // ORDER is proven: code and expected are fixed before any
                // display-only window read.
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
                observation.actual = display(actual, 0, budget);
                return Ok(observation);
            }
            let frozen_names = segments
                .iter()
                .map(|segment| segment.file_name())
                .collect::<Vec<_>>();
            // A CRLF-terminated header is the usual reason a frozen `name\n` is
            // not found. The byte comparison remains the authority.
            let mut crlf_header: Option<(String, usize)> = None;
            for segment in segments {
                let mut needle = segment.file_name().as_bytes().to_vec();
                needle.extend_from_slice(b"\r\n");
                if let Some(offset) = find_needle_metered(actual, 0, &needle, budget)? {
                    crlf_header = Some((segment.file_name().to_owned(), offset));
                    break;
                }
            }
            // SOURCE is proven (with an already-held CRLF-header locator):
            // fix code/detail/expected/locator before the display window.
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
            observation.locator =
                crlf_header.map(|(_, offset)| RecordLocator::default().at_byte(offset as u64));
            observation.actual = display(actual, 0, budget);
            return Ok(observation);
        }
        Some(_positions) => {
            // Headers appear in frozen order; content before the first one is a
            // content difference, decided below.
        }
    }

    // 3. Bodies are byte-identical and only the inter-segment blank line
    //    structure differs.
    if only_separators_differ_metered(actual, segments, budget)? {
        let mut observation = failed(
            rule_id,
            check,
            reason_code::TEXT_SEPARATOR_MISMATCH,
            binding,
            "各段正文与冻结来源逐字节一致，但段间空行（恰好一个空行）不符合冻结合并格式".to_owned(),
        );
        observation.expected = Some("段间恰好一个空行".to_owned());
        observation.actual = Some("段间空行数量不符".to_owned());
        return Ok(observation);
    }

    // 4. A real body difference. The first-difference locator is the proven
    //    fact (set before any display read). The three display windows are
    //    then read **independently and strictly in order**, each through the
    //    existing metered window helper:
    //
    //    1. first expected window — only upgrades the window-free detail;
    //    2. second expected window — fills the `expected` field (a real
    //       second read; the first render is never reused as the field);
    //    3. actual window — fills the `actual` field.
    //
    //    A budget stop in any window returns immediately with the fields
    //    already obtained: neither a later window nor its time pre-check is
    //    started after a stop.
    let offset = match first_difference_metered(expected, actual, budget)? {
        Some(offset) => offset,
        None => expected.len().min(actual.len()),
    };
    let mut observation = failed(
        rule_id,
        check,
        reason_code::TEXT_CONTENT_MISMATCH,
        binding,
        format!("候选正文与冻结来源不一致，首个差异在字节 {offset}"),
    );
    observation.locator = Some(RecordLocator::default().at_byte(offset as u64));

    // Window 1: window-version detail only.
    let Ok(first_expected_window) = window_metered(expected, offset, budget) else {
        return Ok(observation);
    };
    observation.detail = Some(format!(
        "候选正文与冻结来源不一致，首个差异在{first_expected_window}"
    ));

    // Window 2: an independent read that fills `expected`.
    let Ok(second_expected_window) = window_metered(expected, offset, budget) else {
        return Ok(observation);
    };
    observation.expected = Some(second_expected_window);

    // Window 3: the actual-side display.
    if let Ok(actual_window) = window_metered(actual, offset, budget) {
        observation.actual = Some(actual_window);
    }
    Ok(observation)
}

/// A diagnosis scan is bounded: locating *which* rule broke must never cost more
/// than the check itself. Beyond this many bytes the result is still located at
/// its first differing byte, but no reordered-header search is attempted.
const DIAGNOSIS_SCAN_BYTES: usize = 1 << 20;

/// True when the candidate contains the byte sequence `name\n` for a segment.
fn header_present_metered(
    actual: &[u8],
    segment: &TextSegment<'_>,
    budget: &mut Budget<'_>,
) -> Result<bool, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::HeaderSearch);
    let mut needle = segment.file_name().as_bytes().to_vec();
    needle.push(b'\n');
    Ok(find_needle_metered(actual, 0, &needle, budget)?.is_some())
}

fn headers_all_present_metered(
    actual: &[u8],
    segments: &[TextSegment<'_>],
    budget: &mut Budget<'_>,
) -> Result<bool, BudgetStop> {
    for segment in segments {
        if !header_present_metered(actual, segment, budget)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// True when every frozen payload appears verbatim at its header position, in
/// order, and the only bytes that differ anywhere are newlines.
///
/// It never grants a pass on its own — it decides which of two already-failing
/// findings to report, so a separator repair is not asked to rewrite a body.
fn only_separators_differ_metered(
    actual: &[u8],
    segments: &[TextSegment<'_>],
    budget: &mut Budget<'_>,
) -> Result<bool, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Diagnosis);
    // An empty selection has no separator structure.
    if segments.is_empty() {
        return Ok(false);
    }
    let Some(positions) = header_positions_metered(actual, segments, budget)? else {
        return Ok(false);
    };
    for (index, (segment, position)) in segments.iter().zip(positions.iter()).enumerate() {
        let start = position + segment.file_name().len() + 1;
        let Some(payload) = actual.get(start..start + segment.bytes.len()) else {
            return Ok(false);
        };
        if !budget.slices_equal(payload, segment.bytes)? {
            return Ok(false);
        }
        // Between this payload and the next header (or EOF) only newline bytes
        // may appear.
        let tail_start = start + segment.bytes.len();
        let tail_end = positions.get(index + 1).copied().unwrap_or(actual.len());
        for tail_index in tail_start..tail_end {
            if budget.byte_at(actual, tail_index)? != b'\n' {
                return Ok(false);
            }
        }
    }
    Ok(true)
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
/// 1b. **A v2 envelope with an empty frozen selection is a malformed contract.**
///    The freeze precondition excludes it, the finding belongs to the contract
///    rather than the candidate (no expected/actual/locator), and it is decided
///    before every other v2 step. It is never rewritten into `Contract::Unbound`,
///    and v1 history keeps its own byte comparison (charter §21 D-1).
/// 2. **A mandatory final newline is enforced before everything else, and on every
///    candidate** — including one that equals the construction byte for byte,
///    because the construction never adds a trailing LF of its own. Missing or
///    excess trailing LFs are pure byte properties needing no line-ending
///    adjudication, so neither a contract contradiction nor an "undecidable
///    convention" verdict may absorb them (R7, F-P02-03).
/// 3. **The candidate's declared line-ending policy is enforced** next, also
///    before any byte-equality shortcut (F-P02-04).
/// 4. **A strict comparison decides every difference itself.** The line-ending
///    "cannot decide" verdict may only be reached by a policy that grants at
///    least one equivalence; otherwise any byte difference is a blocking failure
///    (R1). It is never allowed to pre-empt the strict branch.
/// 5. **A contradictory contract is reported, not waived, and it never swallows a
///    provable failure.** If the frozen bytes contradict the declared policy the
///    outcome is `unverified` *unless* the bytes also prove a difference that
///    neither the line-ending convention nor the EOF shape the contract itself
///    asked for explains, in which case the failure still blocks and keeps its
///    location and reason code, with the contradiction appended (R2).
/// 6. **The contradiction names the side that is at fault**: a `Crlf` declaration
///    is contradicted by the constructor's own LF bytes (no candidate can satisfy
///    it), while an `Lf` declaration can only be contradicted by a source body
///    (R3). The policy still governs the whole artifact — the constructor's bytes
///    are never exempted.
/// 7. **The authorized EOF difference is decided on the original bytes**, before
///    the undecidable verdict. If both sides are byte-identical once the declared
///    trailing LF is discounted, the pass follows from the frozen bytes plus the
///    contract's own EOF rule — never from normalising. A body that both sides
///    hold with the same CRLF bytes therefore passes (R8), while a genuine
///    LF↔CRLF replacement is not raw-byte equal and still reaches the undecidable
///    branch below.
/// 8. **A known failure is never demoted.** The line-ending "cannot decide"
///    verdict is reported only when the residual difference is fully explained by
///    the undecidable convention and is not already an authorized EOF difference
///    (F-P02-02).
/// 9. **An equivalence is recorded only when the pass relied on it** (F-P02-06).
pub(crate) fn check_text_concat(request: &TextConcatRequest<'_>) -> CheckObservation {
    check_text_concat_budgeted(request, &CheckLimits::P0, &SystemClock)
}

/// Budgeted entry point. `limits` is the caller's frozen budget and `clock`
/// the injectable time source. The judgment ordering (R1–R8/A7) is unchanged;
/// budget stops are either K1 (conclusion incomplete) or, after a failure is
/// proven under the complete policy, a degraded-but-blocking Failed.
pub(crate) fn check_text_concat_budgeted(
    request: &TextConcatRequest<'_>,
    limits: &CheckLimits,
    clock: &dyn Clock,
) -> CheckObservation {
    let check = check_name::TEXT_CONCAT;
    let rule_id = request.rule_id;
    let binding = request.binding;
    let policy = &request.envelope.policy;
    let declared = policy.table.line_ending;
    // Whether this policy can legitimately end in "these bytes differ but the
    // contract tolerates it". Only then may an uncovered difference stay
    // undecided; a purely byte-exact contract decides every difference (R1).
    let grants_equivalence = policy_grants_equivalence(policy);

    let mut budget = Budget::new(limits, clock);
    #[cfg(test)]
    budget.phase(Phase::Entry);

    // K1 helper: a stop obtained while the conclusion is still incomplete.
    let stopped =
        |stop: BudgetStop| stop.to_observation(limits, rule_id, check, binding);
    // Unified exit: Stopped -> K1, ProvenFailed is kept (terminal time check
    // cannot demote it), Complete passes the terminal time check.
    let finish =
        |budget: &mut Budget<'_>, outcome: Outcome| budget.finalize(outcome, rule_id, check, binding);

    // S0: the 0 ms gate trips before any allocation or scan.
    if let Err(stop) = budget.check_time() {
        return finish(&mut budget, Outcome::Stopped(stop));
    }
    // S1: candidate byte ceiling (length is metadata).
    if request.actual.len() as u64 > limits.max_bytes {
        return budget_exceeded(
            rule_id,
            check,
            binding,
            LimitKind::Bytes,
            limits.max_bytes,
            request.actual.len() as u64,
        );
    }
    // S2/S3: construction length (TextBytes, pre-allocation) and chunked copy.
    let expected = match build_expected_metered(request.segments, &mut budget) {
        Ok(expected) => expected,
        Err(stop) => return stopped(stop),
    };

    // A proven failure whose detailed diagnosis may still stop. A stop while
    // the *fact* is being established yields the generic content failure with
    // no fabricated location; a stop in a *display window* is handled inside
    // diagnose_concat_metered and keeps the already-proven code/locator.
    let diagnose_failed =
        |budget: &mut Budget<'_>, note: Option<&str>| -> CheckObservation {
            let mut observation = match diagnose_concat_metered(
                rule_id,
                check,
                binding,
                request.segments,
                &expected,
                request.actual,
                budget,
            ) {
                Ok(observation) => observation,
                Err(_) => failed(
                    rule_id,
                    check,
                    reason_code::TEXT_CONTENT_MISMATCH,
                    binding,
                    "候选与冻结构造不一致；详细诊断在资源上限内未完成".to_owned(),
                ),
            };
            if let Some(note) = note {
                let suffix = format!("；另外，{note}");
                match observation.detail.as_mut() {
                    Some(detail) => detail.push_str(&suffix),
                    None => observation.detail = Some(note.to_owned()),
                }
            }
            // The failure is already proven under the full policy; the
            // terminal time check must not demote it (K2).
            budget.finalize(
                Outcome::ProvenFailed(observation),
                rule_id,
                check,
                binding,
            )
        };

    // (1) v1: the old byte comparison, and nothing else.
    if request.envelope.is_v1() {
        #[cfg(test)]
        budget.phase(Phase::RawCompare);
        return match budget.slices_equal(request.actual, &expected) {
            Ok(true) => budget.finalize(
                Outcome::Complete(passed(rule_id, check, binding)),
                rule_id,
                check,
                binding,
            ),
            Ok(false) => diagnose_failed(&mut budget, None),
            Err(stop) => stopped(stop),
        };
    }

    // (1b) A v2 envelope whose frozen selection is empty is a malformed
    //      contract. It is not rewritten into `Contract::Unbound`, and v1
    //      history keeps its own byte comparison above.
    if request.segments.is_empty() {
        return unverified(
            rule_id,
            check,
            reason_code::CONTRACT_UNREADABLE,
            binding,
            "冻结合同的选择集合为空：这是畸形合同（冻结前置条件要求 1..=64 个来源），本项按未核验处理"
                .to_owned(),
        );
    }

    // (2) The declared policy applied to the frozen artifact itself (R3). The
    //     verdict waits until after the mandatory final-newline requirement
    //     (R7), so a contradiction can never absorb a hard requirement.
    let expected_verdict = match line_ending_verdict_metered(&expected, declared, &mut budget) {
        Ok(verdict) => verdict,
        Err(stop) => return stopped(stop),
    };
    let conflict = if matches!(expected_verdict, LineEndingVerdict::Violation(_)) {
        match conflict_attribution_metered(request.segments, declared, &mut budget) {
            Ok(attribution) => Some(attribution),
            Err(stop) => return stopped(stop),
        }
    } else {
        None
    };

    let actual_lf = match trailing_lf_count_metered(request.actual, &mut budget) {
        Ok(count) => count,
        Err(stop) => return stopped(stop),
    };
    let expected_lf = match trailing_lf_count_metered(&expected, &mut budget) {
        Ok(count) => count,
        Err(stop) => return stopped(stop),
    };

    // (3) A declared final-newline requirement is a hard, independently
    //     decidable property (R7). This failure is already proven.
    if policy.eof == EofPolicy::RequireTrailingLf && actual_lf != 1 {
        let mut detail = format!("冻结合同要求恰好一个终止 LF，候选有 {actual_lf} 个");
        if let Some(conflict) = &conflict {
            detail.push_str(&format!("；另外，{conflict}"));
        }
        let mut observation = failed(
            rule_id,
            check,
            reason_code::TEXT_EOF_MISMATCH,
            binding,
            detail,
        );
        observation.expected = Some("末尾 LF 1 个".to_owned());
        observation.actual = Some(format!("末尾 LF {actual_lf} 个"));
        observation.locator =
            Some(RecordLocator::default().at_byte((request.actual.len() - actual_lf) as u64));
        return observation;
    }

    // (4) The contradiction's verdict. A difference the bytes prove
    //     independently of the line-ending question still blocks (R2); a
    //     strict policy decides every difference outright (R1).
    if let Some(conflict) = &conflict {
        #[cfg(test)]
        budget.phase(Phase::RawCompare);
        let unequal = match budget.slices_equal(request.actual, &expected) {
            Ok(equal) => !equal,
            Err(stop) => return stopped(stop),
        };
        let independently_different = !grants_equivalence
            || match proves_independent_difference_metered(
                &expected,
                request.actual,
                discounts_one_trailing_lf(policy),
                &mut budget,
            ) {
                Ok(different) => different,
                Err(stop) => return stopped(stop),
            };
        if unequal && independently_different {
            return diagnose_failed(&mut budget, Some(conflict));
        }
        // The contract is unsatisfiable and no independent difference is
        // proven: normal undecided conclusion (Complete, terminal time check).
        let observation = unverified(
            rule_id,
            check,
            reason_code::TEXT_LINE_ENDING_UNSUPPORTED,
            binding,
            conflict.clone(),
        );
        return budget.finalize(
            Outcome::Complete(observation),
            rule_id,
            check,
            binding,
        );
    }

    // (5) The candidate's own declared line-ending policy. A violation is a
    //     proven failure.
    let candidate_endings =
        match line_ending_verdict_metered(request.actual, declared, &mut budget) {
            Ok(verdict) => verdict,
            Err(stop) => return stopped(stop),
        };
    if let LineEndingVerdict::Violation(detail) = &candidate_endings {
        // The violation is already proven; render the window only if its
        // bounded read still fits the budget (no free scanning for a locator).
        let actual_window = window_metered(request.actual, 0, &mut budget).unwrap_or_default();
        let mut observation = failed(
            rule_id,
            check,
            reason_code::TEXT_LINE_ENDING_VIOLATION,
            binding,
            format!("候选违反冻结合同声明的行结束符政策：{detail}"),
        );
        observation.expected = Some(format!("行结束符符合声明 {declared:?}"));
        observation.actual = Some(actual_window);
        return observation;
    }

    // (6) Byte-identical: decided before the authorized-EOF arm so an ordinary
    //     match is never labelled with an equivalence (R8).
    #[cfg(test)]
    budget.phase(Phase::RawCompare);
    match budget.slices_equal(request.actual, &expected) {
        Ok(true) => {
            return budget.finalize(
                Outcome::Complete(passed(rule_id, check, binding)),
                rule_id,
                check,
                binding,
            );
        }
        Ok(false) => {}
        Err(stop) => return stopped(stop),
    }

    // (7) The EOF difference the contract itself authorized, on the original
    //     bytes only — never normalized (R8).
    if policy.comparison == ComparisonMode::TextSegments {
        let equivalence_available = actual_lf <= 1 && expected_lf <= 1;
        let raw_bodies_equal = if equivalence_available {
            let actual_stripped = strip_one_trailing_lf(request.actual);
            let expected_stripped = strip_one_trailing_lf(&expected);
            #[cfg(test)]
            budget.phase(Phase::RawCompare);
            match budget.slices_equal(actual_stripped, expected_stripped) {
                Ok(equal) => equal,
                Err(stop) => return stopped(stop),
            }
        } else {
            false
        };
        if equivalence_available && raw_bodies_equal {
            let observation = match policy.eof {
                EofPolicy::AllowSingleTrailingLf => passed_with_equivalence(
                    rule_id,
                    check,
                    binding,
                    equivalence::OPTIONAL_FINAL_LF,
                    format!(
                        "正文与段间结构与冻结合并逐字节一致；按冻结合同允许的等价，末尾终止 LF 的有／无视为等价（候选 {actual_lf} 个，期望 {expected_lf} 个）"
                    ),
                ),
                EofPolicy::RequireTrailingLf => passed(rule_id, check, binding),
                EofPolicy::ByteExact | EofPolicy::SourceVerbatim => {
                    // Strict EOF policies do not take this arm.
                    return diagnose_failed(&mut budget, None);
                }
            };
            return budget.finalize(
                Outcome::Complete(observation),
                rule_id,
                check,
                binding,
            );
        }
    }

    // (8) Line endings the declaration does not decide. The independent
    //     difference test chooses between failed and unverified; it can never
    //     produce a pass.
    if let LineEndingVerdict::Unsupported(detail) = &candidate_endings {
        if !grants_equivalence {
            return diagnose_failed(&mut budget, None);
        }
        let still_different = match proves_independent_difference_metered(
            &expected,
            request.actual,
            discounts_one_trailing_lf(policy),
            &mut budget,
        ) {
            Ok(different) => different,
            Err(stop) => return stopped(stop),
        };
        if !still_different {
            // Normal undecided conclusion.
            let observation = unverified(
                rule_id,
                check,
                reason_code::TEXT_LINE_ENDING_UNSUPPORTED,
                binding,
                detail.clone(),
            );
            return budget.finalize(
                Outcome::Complete(observation),
                rule_id,
                check,
                binding,
            );
        }
        // Fall through: an independent difference keeps blocking (R2).
    }

    // A v2 envelope that did not opt into segment comparison keeps the strict
    // comparison; only the diagnosis is sharper.
    diagnose_failed(&mut budget, None)
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
/// Classify bytes against the declared line-ending policy. Every byte actually
/// inspected (including the one-byte CRLF lookahead) is charged to the budget.
///
/// * `Violation` — the bytes contradict a policy the frozen contract declared.
/// * `Unsupported` — nothing the contract declared decides these bytes, so no
///   faithful comparison exists without converting them (which is forbidden).
fn line_ending_verdict_metered(
    bytes: &[u8],
    declared: LineEnding,
    budget: &mut Budget<'_>,
) -> Result<LineEndingVerdict, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::LineEndingVerdict);
    let mut has_crlf = false;
    let mut has_bare_lf = false;
    let mut lone_cr: Option<usize> = None;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = budget.byte_at(bytes, index)?;
        match byte {
            b'\r' => {
                // The lookahead is a real read; if the next byte does not exist
                // there is nothing to charge (natural EOF).
                let followed_by_lf = index + 1 < bytes.len()
                    && budget.byte_at(bytes, index + 1)? == b'\n';
                if followed_by_lf {
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
        return Ok(LineEndingVerdict::Unsupported(format!(
            "字节 {offset} 存在孤立的 CR（既不与 LF 组成 CRLF，也不被任何行结束声明覆盖）；不做隐式转换，本项按未核验处理"
        )));
    }

    Ok(match declared {
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
    })
}

/// Line endings mapped to a single LF so that a difference which is *only* the
/// LF / CRLF / CR convention can be recognised.
///
/// **Invariant**: this exists solely to choose between `failed` and `unverified`.
/// It can never produce a pass — a pass comes either from a byte-equal comparison
/// or from an equivalence the frozen contract explicitly declares. The artifact is
/// never rewritten or "fixed" from this result; the plan forbids normalising it.
/// Next symbol of the line-ending-normalized stream, without building a
/// normalized `Vec`: CRLF and a lone CR both yield one LF, every other byte
/// yields itself. Each raw byte actually consumed is charged to the budget
/// (the LF lookahead after a CR is a real read); repeated passes re-charge.
fn next_norm_symbol(
    bytes: &[u8],
    pos: &mut usize,
    budget: &mut Budget<'_>,
) -> Result<Option<u8>, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Normalization);
    if *pos >= bytes.len() {
        return Ok(None);
    }
    let byte = budget.byte_at(bytes, *pos)?;
    *pos += 1;
    if byte == b'\r' {
        if *pos < bytes.len() && budget.byte_at(bytes, *pos)? == b'\n' {
            *pos += 1;
        }
        return Ok(Some(b'\n'));
    }
    Ok(Some(byte))
}

/// `(normalized length, normalized trailing LF count)` from one streaming pass.
fn norm_stats(bytes: &[u8], budget: &mut Budget<'_>) -> Result<(usize, usize), BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Normalization);
    let mut pos = 0usize;
    let mut length = 0usize;
    let mut trailing = 0usize;
    while let Some(symbol) = next_norm_symbol(bytes, &mut pos, budget)? {
        length += 1;
        if symbol == b'\n' {
            trailing += 1;
        } else {
            trailing = 0;
        }
    }
    Ok((length, trailing))
}

/// Streaming, allocation-free replacement of the old
/// `normalize_line_endings` pair of `Vec`s. Semantics are identical: line
/// endings on both sides map to LF, and when the policy tolerates one
/// terminating LF on each side one trailing LF symbol is discounted.
///
/// **Invariant**: this exists solely to choose between `failed` and
/// `unverified`. It can never produce a pass — a pass comes either from a
/// byte-equal comparison or from an equivalence the frozen contract declares.
fn proves_independent_difference_metered(
    expected: &[u8],
    actual: &[u8],
    tolerate_single_lf: bool,
    budget: &mut Budget<'_>,
) -> Result<bool, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::Normalization);
    let (expected_len, expected_tail) = norm_stats(expected, budget)?;
    let (actual_len, actual_tail) = norm_stats(actual, budget)?;

    let mut compare_expected = expected_len;
    let mut compare_actual = actual_len;
    if tolerate_single_lf && expected_tail <= 1 && actual_tail <= 1 {
        compare_expected -= usize::from(expected_tail >= 1);
        compare_actual -= usize::from(actual_tail >= 1);
    }
    if compare_expected != compare_actual {
        return Ok(true);
    }

    // Second pass over the raw inputs (re-charged, like every repeated scan),
    // comparing only the discounted normalized prefixes. The symbol counts
    // were established by `norm_stats`, so a missing symbol cannot occur on
    // a budget-clean path; if it does, treat the inputs as different rather
    // than panicking.
    let (mut pe, mut pa) = (0usize, 0usize);
    for _ in 0..compare_expected {
        let Some(left) = next_norm_symbol(expected, &mut pe, budget)? else {
            return Ok(true);
        };
        let Some(right) = next_norm_symbol(actual, &mut pa, budget)? else {
            return Ok(true);
        };
        if left != right {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True when the frozen policy grants at least one equivalence, i.e. a difference
/// may legitimately end in "these bytes differ but the contract tolerates it".
///
/// Only such a policy may report a difference it does not cover as *undecidable*.
/// A purely byte-exact contract decides every difference outright, so the
/// line-ending "cannot decide" verdict must never pre-empt the strict comparison
/// (R1).
fn policy_grants_equivalence(policy: &RulePolicy) -> bool {
    policy.comparison == ComparisonMode::TextSegments
        && matches!(
            policy.eof,
            EofPolicy::AllowSingleTrailingLf | EofPolicy::RequireTrailingLf
        )
}

/// The policy's own permission to discount **one** terminating LF on each side
/// while testing whether a *further*, independent difference exists.
///
/// This answers a different question from [`policy_grants_equivalence`] (which
/// decides whether an uncovered difference may stay undecided at all), so the two
/// are named separately even though they currently agree. It is deliberately true
/// for **both** policies that declare an EOF equivalence, and narrowing it to
/// `AllowSingleTrailingLf` would be wrong:
///
/// * `AllowSingleTrailingLf` declares "zero or one on each side".
/// * `RequireTrailingLf` declares "exactly one", but `expected_concat` never adds
///   one, so a candidate that **satisfied** the requirement — enforced before this
///   predicate is consulted — differs from the construction by exactly the LF the
///   contract demanded. Discounting that is what the contract asked for, not an
///   extra tolerance; narrowing the predicate would report a compliant artifact as
///   an independent body difference.
///
/// A *missing* or *excess* LF under `RequireTrailingLf` never reaches this
/// predicate, because the hard requirement is enforced first (R7).
fn discounts_one_trailing_lf(policy: &RulePolicy) -> bool {
    policy_grants_equivalence(policy)
}

/// True when a frozen source body carries a CRLF, found with a metered
/// overlapping pair scan (repeated position reads are charged again).
fn contains_crlf_metered(bytes: &[u8], budget: &mut Budget<'_>) -> Result<bool, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::ConflictAttribution);
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if budget.byte_at(bytes, index)? == b'\r'
            && budget.byte_at(bytes, index + 1)? == b'\n'
        {
            return Ok(true);
        }
        index += 1;
    }
    Ok(false)
}

/// Name the origin of a declared-policy contradiction so the message does not
/// blame the wrong side (R3).
///
/// The frozen construction writes every header as `name` + LF and every separator
/// as LF, and copies each source body verbatim. Consequently:
///
/// * a `Crlf` declaration can never be satisfied by **any** candidate — the
///   format's own LF bytes contradict it — and that must be reported as a
///   constructor-format conflict rather than as a source problem;
/// * an `Lf` declaration can only be contradicted by a source **body** carrying
///   CRLF, so that conflict is attributed to the frozen sources.
///
/// The policy still governs the whole artifact: both sides take part in the
/// detection above, and neither is exempted.
/// Name the origin of a declared-policy contradiction so the message does not
/// blame the wrong side (R3). Every source-body scan is metered.
fn conflict_attribution_metered(
    segments: &[TextSegment<'_>],
    declared: LineEnding,
    budget: &mut Budget<'_>,
) -> Result<String, BudgetStop> {
    #[cfg(test)]
    budget.phase(Phase::ConflictAttribution);
    Ok(match declared {
        LineEnding::Crlf => "冻结合同自相矛盾：合并格式自身用 LF 结束每段标题行与段间空行（来源正文逐字节原样保留），因此不存在满足 Crlf 声明的候选；不修改构造格式、不把标题改成 CRLF、不缩小政策范围，本组合按未核验处理".to_owned(),
        LineEnding::Lf => {
            let mut offenders = Vec::new();
            for segment in segments {
                if contains_crlf_metered(segment.bytes, budget)? {
                    offenders.push(segment.file_name());
                }
            }
            if offenders.is_empty() {
                "冻结合同自相矛盾：声明行结束符为 Lf，但冻结产物与声明不一致".to_owned()
            } else {
                format!(
                    "冻结合同自相矛盾：声明行结束符为 Lf，但冻结**来源正文**已含 CRLF（{}）；合并格式要求来源原样，两者无法同时满足。不修改来源、不静默放行，本项按未核验处理",
                    offenders.join("、")
                )
            }
        }
        LineEnding::Any => {
            "冻结合同的行结束声明与冻结产物不一致，无法判定本次差异".to_owned()
        }
    })
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

/// Map a parser-stage budget stop to a table error with an explicit locator.
fn table_budget_error(
    stop: BudgetStop,
    limits: &CheckLimits,
    locator: RecordLocator,
) -> TableError {
    TableError::budget(
        stop.kind,
        limits.ceiling(stop.kind),
        stop.observed,
        locator,
    )
}

/// Parse a delimited table under an explicit dialect and the frozen ceilings.
///
/// Errors carry a stable [`reason_code`] and a [`RecordLocator`], so "the parser
/// refuses this input" and "the data disagrees with the contract" stay different
/// findings. This is the P0/[`SystemClock`] compatibility wrapper; callers that
/// freeze their own limits (and tests) use [`parse_table_with_clock`], which
/// transparently forwards the same `limits`.
pub(crate) fn parse_table(
    bytes: &[u8],
    dialect: &TableDialect,
    limits: &CheckLimits,
) -> Result<ParsedTable, TableError> {
    parse_table_with_clock(bytes, dialect, limits, &SystemClock)
}

/// Decode one character at `i` (1..4 raw bytes), charging every byte actually
/// read through the budget. P2 already proved the sequence is valid UTF-8; this
/// is a safe bounded decode (no unsafe, no whole-slice re-validation).
fn decode_char_at(
    raw: &[u8],
    i: usize,
    budget: &mut Budget<'_>,
    limits: &CheckLimits,
    locator: RecordLocator,
) -> Result<(char, usize), TableError> {
    let stop = |stop: BudgetStop| table_budget_error(stop, limits, locator.clone());
    let lead = budget.byte_at(raw, i).map_err(&stop)?;
    let width = match lead {
        0x00..=0x7F => 1usize,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        // P2 ruled illegal leads out; defensive only.
        _ => {
            return Err(TableError::unsupported(
                reason_code::TABLE_ENCODING_UNSUPPORTED,
                RecordLocator::default().at_byte(i as u64),
                format!("非法 UTF-8 起始字节 0x{lead:02x}"),
            ));
        }
    };
    let mut cp: u32 = match width {
        1 => u32::from(lead),
        2 => u32::from(lead & 0x1F),
        3 => u32::from(lead & 0x0F),
        _ => u32::from(lead & 0x07),
    };
    for j in 1..width {
        let byte = budget.byte_at(raw, i + j).map_err(&stop)?;
        cp = (cp << 6) | u32::from(byte & 0x3F);
    }
    let ch = char::from_u32(cp).ok_or_else(|| {
        TableError::unsupported(
            reason_code::TABLE_ENCODING_UNSUPPORTED,
            RecordLocator::default().at_byte(i as u64),
            format!("UTF-8 码点 U+{cp:04X} 无法解码"),
        )
    })?;
    Ok((ch, width))
}

/// Same as [`parse_table`] with an injectable clock. Every input byte is
/// actually read through the [`Budget`] (P2 validation, P3 pre-scan and the
/// P4 main loop are separate passes and are charged separately); a refusal to
/// read a still-existing byte is a Resource stop (`ScanBytes`/`Millis`),
/// never an encoding error or a fake EOF.
pub(crate) fn parse_table_with_clock(
    bytes: &[u8],
    dialect: &TableDialect,
    limits: &CheckLimits,
    clock: &dyn Clock,
) -> Result<ParsedTable, TableError> {
    let mut budget = Budget::new(limits, clock);

    // 0 ms trips before any allocation or scan.
    budget
        .check_time()
        .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?;
    if bytes.len() as u64 > limits.max_bytes {
        return Err(TableError::budget(
            LimitKind::Bytes,
            limits.max_bytes,
            bytes.len() as u64,
            RecordLocator::default(),
        ));
    }

    // BOM handling is a declared policy, and a violation of it is an encoding
    // finding. The comparison short-circuits on the first byte, so an input
    // without a BOM costs one logical read.
    let (body, bom_stripped) = match dialect.bom {
        BomPolicy::Auto => {
            // Natural EOF reads nothing; otherwise the first byte is read
            // and a BOM match continues only when it is 0xEF.
            let first = if bytes.is_empty() {
                None
            } else {
                Some(
                    budget
                        .byte_at(bytes, 0)
                        .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?,
                )
            };
            if first == Some(0xEF)
                && bytes.len() >= 3
                && budget
                    .byte_at(bytes, 1)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBB
                && budget
                    .byte_at(bytes, 2)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBF
            {
                (&bytes[3..], true)
            } else {
                (bytes, false)
            }
        }
        BomPolicy::Required => {
            if bytes.len() < 3 {
                return Err(TableError::violation(
                    reason_code::TABLE_ENCODING_UNSUPPORTED,
                    RecordLocator::default(),
                    "冻结合同要求 UTF-8 BOM，候选没有".to_owned(),
                ));
            }
            let has_bom = budget
                .byte_at(bytes, 0)
                .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                == 0xEF
                && budget
                    .byte_at(bytes, 1)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBB
                && budget
                    .byte_at(bytes, 2)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBF;
            if has_bom {
                (&bytes[3..], true)
            } else {
                return Err(TableError::violation(
                    reason_code::TABLE_ENCODING_UNSUPPORTED,
                    RecordLocator::default(),
                    "冻结合同要求 UTF-8 BOM，候选没有".to_owned(),
                ));
            }
        }
        BomPolicy::Forbidden => {
            let first = if bytes.is_empty() {
                None
            } else {
                Some(
                    budget
                        .byte_at(bytes, 0)
                        .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?,
                )
            };
            if first == Some(0xEFu8)
                && bytes.len() >= 3
                && budget
                    .byte_at(bytes, 1)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBB
                && budget
                    .byte_at(bytes, 2)
                    .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                    == 0xBF
            {
                return Err(TableError::violation(
                    reason_code::TABLE_ENCODING_UNSUPPORTED,
                    RecordLocator::default(),
                    "冻结合同禁止 UTF-8 BOM，候选带 BOM".to_owned(),
                ));
            }
            (bytes, false)
        }
    };

    // P2: per-byte UTF-8 validation (no whole-slice from_utf8 re-scan).
    validate_utf8_metered(body, &mut budget)?;

    // P3: a lone CR is not representable in any declared dialect, and a
    // declared LF/CRLF policy must not be silently overridden. Lookahead and
    // lookbehind bytes are read (and charged) only when the branch actually
    // evaluates them, mirroring the original short-circuiting conditions.
    let raw = body;
    let mut index = 0usize;
    while index < raw.len() {
        let byte = budget
            .byte_at(raw, index)
            .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?;
        match byte {
            b'\r' => {
                let mut followed_by_lf = false;
                if index + 1 < raw.len() {
                    followed_by_lf = budget
                        .byte_at(raw, index + 1)
                        .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                        == b'\n';
                }
                if !followed_by_lf {
                    return Err(TableError::unsupported(
                        reason_code::TABLE_DIALECT_UNSUPPORTED,
                        RecordLocator::default().at_byte(index as u64),
                        "候选存在孤立的 CR，声明方言无法安全解释；不做隐式转换".to_owned(),
                    ));
                }
                if dialect.line_ending == LineEnding::Lf {
                    return Err(TableError::violation(
                        reason_code::TABLE_DIALECT_UNSUPPORTED,
                        RecordLocator::default().at_byte(index as u64),
                        "冻结合同声明行结束符为 LF，候选含 CRLF".to_owned(),
                    ));
                }
                index += 2;
                continue;
            }
            b'\n' => {
                if dialect.line_ending == LineEnding::Crlf {
                    // The lookbehind is evaluated only under the Crlf policy.
                    let preceded_by_cr = index >= 1
                        && budget
                            .byte_at(raw, index - 1)
                            .map_err(|stop| table_budget_error(stop, limits, RecordLocator::default()))?
                            == b'\r';
                    if !preceded_by_cr {
                        return Err(TableError::violation(
                            reason_code::TABLE_DIALECT_UNSUPPORTED,
                            RecordLocator::default().at_byte(index as u64),
                            "冻结合同声明行结束符为 CRLF，候选含单独的 LF".to_owned(),
                        ));
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }

    // P4: main parsing loop over raw bytes. Every input byte is read through
    // the budget exactly once in this pass, including while a field is open;
    // the structural limits (line/text/cells/fields/records) stay enforced by
    // the builder itself.
    let mut builder = TableBuilder::new(dialect, limits);
    let mut index = 0usize;
    while index < raw.len() {
        let char_index = index;
        let (ch, width) = decode_char_at(raw, index, &mut budget, limits, builder.locator())?;
        index += width;

        if builder.in_quotes {
            if ch == '"' {
                // Read the next byte once, only to test quote doubling.
                let next = if index < raw.len() {
                    Some(
                        budget
                            .byte_at(raw, index)
                            .map_err(|stop| {
                                table_budget_error(stop, limits, builder.locator())
                            })?,
                    )
                } else {
                    None
                };
                if next == Some(b'"') {
                    builder.count_bytes(2)?;
                    builder.field.push('"');
                    index += 1;
                    continue;
                }
                builder.in_quotes = false;
                builder.after_quote = true;
                continue;
            }
            if ch == '\n' {
                builder.line += 1;
            }
            builder.push_char(ch)?;
            continue;
        }

        if builder.after_quote {
            if ch == builder.delimiter {
                builder.after_quote = false;
                builder.push_field()?;
                continue;
            }
            if ch == '\n' {
                builder.after_quote = false;
                builder.push_record(false)?;
                builder.line += 1;
                continue;
            }
            if ch == '\r' && index < raw.len() {
                // Lookahead for the LF of a CRLF terminator (read + charged).
                if budget
                    .byte_at(raw, index)
                    .map_err(|stop| table_budget_error(stop, limits, builder.locator()))?
                    == b'\n'
                {
                    // The LF branch handles the terminator; skip the CR.
                    continue;
                }
            }
            return Err(TableError::violation(
                reason_code::TABLE_CHARACTER_AFTER_QUOTE,
                builder.locator().at_byte(char_index as u64),
                format!("引号闭合后出现未转义字符「{ch}」"),
            ));
        }

        match ch {
            '"' if builder.quoting && builder.field.is_empty() && !builder.field_quoted => {
                builder.mark_record_start();
                builder.count_bytes(1)?;
                builder.in_quotes = true;
                builder.field_quoted = true;
            }
            '"' if builder.quoting => {
                return Err(TableError::violation(
                    reason_code::TABLE_UNESCAPED_QUOTE,
                    builder.locator().at_byte(char_index as u64),
                    "未加引号的字段中出现引号；若为字面引号须写作双引号并整体加引号".to_owned(),
                ));
            }
            c if c == builder.delimiter => {
                builder.mark_record_start();
                builder.push_field()?;
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
            }
            '\r' => {
                // Part of a CRLF terminator the P3 pre-scan validated; the LF
                // branch handles the terminator.
            }
            c => {
                builder.push_char(c)?;
            }
        }
    }

    if builder.in_quotes {
        // Natural EOF with a quote still open: the complete input was read,
        // so this is a syntax refusal, not a budget stop.
        return Err(TableError::violation(
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

/// Which side of a content comparison was being parsed. A typed enum (not a
/// bool) so a call site cannot silently swap source and candidate: a source
/// parse failure and a candidate parse failure have different repair
/// implications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseInput {
    /// The frozen reference (already hash-checked by the Host). If it cannot
    /// be interpreted, the candidate cannot be judged — never a candidate
    /// content failure the model could repair.
    Source,
    /// The delivered candidate artifact. A definite violation here is a
    /// blocking, repairable content failure.
    Candidate,
}

/// Map a parser error into the observation vocabulary, by explicit input
/// attribution and the construction-site [`ParseFailureKind`].
///
/// * Candidate [`ParseFailureKind::Resource`] → unverified BUDGET_EXCEEDED
///   with the real limit; * Candidate `Unsupported` → unverified with the
///   parser's own code/detail/locator; * Candidate `Violation` → blocking
///   Failed with the parser's own code/detail/locator.
///
/// A **Source** error is always non-blocking unverified (the reference is
/// unreadable under this checker, so no candidate conclusion follows); it
/// carries an attribution detail and the original code, but its locator
/// describes the *reference* parse and must never be presented as a
/// candidate position. Source/candidate authorization or hash failures are
/// not parser errors and never enter this function.
pub(crate) fn table_parse_observation(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
    input: ParseInput,
    error: &TableError,
) -> CheckObservation {
    let attributed_detail = match input {
        ParseInput::Source => format!("冻结参考表格无法由当前检查器解释，候选未判定：{}", error.detail()),
        ParseInput::Candidate => error.detail().to_owned(),
    };
    match (input, error.kind()) {
        (ParseInput::Candidate, ParseFailureKind::Resource) => {
            let mut observation = unverified(
                rule_id,
                check,
                reason_code::BUDGET_EXCEEDED,
                binding,
                attributed_detail,
            );
            observation.limit = error.limit();
            // The parser already located where the ceiling was hit (a byte,
            // or the record/line of the offending builder check). Unverified
            // does not mean "no position": keep it, exactly as for Unsupported.
            observation.locator = Some(error.locator().clone());
            observation
        }
        (ParseInput::Candidate, ParseFailureKind::Unsupported) => {
            let mut observation = unverified(
                rule_id,
                check,
                error.code(),
                binding,
                attributed_detail,
            );
            // Keep the position the parser had reached (e.g. the illegal byte
            // or the lone CR); this is evidence, not a fabricated location.
            observation.locator = Some(error.locator().clone());
            observation
        }
        (ParseInput::Candidate, ParseFailureKind::Violation) => {
            let mut observation = failed(
                rule_id,
                check,
                error.code(),
                binding,
                attributed_detail,
            );
            observation.locator = Some(error.locator().clone());
            observation
        }
        // A reference-table failure, whatever its parser kind, never blocks
        // repair of or a verdict against the candidate. It keeps the parser
        // code/limit and a *reference* locator (callers must not label it a
        // candidate position); Resource keeps its limit for diagnostics.
        (ParseInput::Source, _) => {
            let mut observation = unverified(
                rule_id,
                check,
                error.code(),
                binding,
                attributed_detail,
            );
            observation.locator = Some(error.locator().clone());
            observation.limit = error.limit();
            observation
        }
    }
}

/// Check a parsed table's headers and record widths against a frozen rule.
pub(crate) fn check_table_columns(request: &TableCheckRequest<'_>) -> CheckObservation {
    let check = check_name::TABLE_COLUMNS;
    let parsed = request.parsed;
    let binding = request.binding;

    if parsed.records.is_empty() {
        // A truly empty table (empty file, BOM only, or blank lines the dialect
        // skips) is decided by *two* independent declarations:
        //
        //   * `HeaderPolicy::Required` demands a header, and no other declaration
        //     may waive it: `allow_zero_records` speaks about **data rows**, so it
        //     cannot be silently widened into "a required header may be missing";
        //   * otherwise the contract itself may declare that zero records are legal.
        if parsed.dialect.header == HeaderPolicy::Required {
            let mut observation = failed(
                request.rule_id,
                check,
                reason_code::TABLE_ZERO_RECORDS,
                binding,
                "冻结合同要求表头，但候选没有任何记录（空文件、仅 BOM 或仅被跳过的空行）；缺的是必需表头，声明允许零数据行不能豁免"
                    .to_owned(),
            );
            observation.expected = Some("至少一条表头记录".to_owned());
            observation.actual = Some("0 条记录".to_owned());
            return observation;
        }
        if !request.allow_zero_records {
            let mut observation = failed(
                request.rule_id,
                check,
                reason_code::TABLE_ZERO_RECORDS,
                binding,
                "候选没有任何记录，而冻结合同未声明允许零记录".to_owned(),
            );
            observation.expected = Some("至少一条记录".to_owned());
            observation.actual = Some("0 条记录".to_owned());
            return observation;
        }
        // `HeaderPolicy::Optional` and the contract declares zero records legal:
        // there is no header to require and no data row to demand, so the artifact
        // satisfies exactly what was declared.
        return passed(request.rule_id, check, binding);
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

/// Independent **test oracle** for the frozen G5 conversion-asymmetry
/// fixture: the exact allocating normaliser the v1 comparison used
/// (`95c704b::normalize_line_endings`). Test-only and module-visible so both
/// the built-in tests and the independent versioned regression overlay can
/// call it. Production code keeps the allocation-free, budgeted streaming
/// comparison and never calls this. It maps CRLF and a lone CR to one LF and
/// copies every other byte; no EOF discount is added here.
#[cfg(test)]
pub(crate) fn normalize_line_endings(bytes: &[u8]) -> Vec<u8> {
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
        v2_with(ComparisonMode::TextSegments, eof)
    }

    fn v2_with(comparison: ComparisonMode, eof: EofPolicy) -> RuleEnvelope {
        RuleEnvelope::v2(
            "content:concat",
            RulePolicy {
                comparison,
                eof,
                table: TableDialect::default(),
            },
        )
    }

    fn concat_of(segments: &[TextSegment<'_>]) -> Vec<u8> {
        let mut expected = Vec::new();
        expected_concat(segments, &mut expected);
        expected
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

    // -----------------------------------------------------------------------
    // 空来源边界（R6：v1 与 v2 必须分开）
    //
    // **前置条件证据**：选择数量被限制为 `1..=64`
    // （`delivery_content_requirements.rs:284-285` 的 `.filter(|n| *n > 0 && *n <= 64)`），
    // 且 `:313` 保证 `sources.len() >= count`、`:316` 取前 `count` 个，所以
    // `selected.len() == count >= 1`——**合法冻结的 `Concat` 不可能没有来源**。
    // 真正的"未绑定"是**另一个契约变体** `Contract::Unbound`（`:626` →
    // `Unverified(reason)`），不能与空 `Concat` 混同。
    // -----------------------------------------------------------------------

    /// **v1 空集合 = 旧字节比较的确切行为（明确对照）。**
    ///
    /// 该状态**不可达**（见上方前置条件），因此这里记录的是旧 checker 对空期望的
    /// 语义，而不是业务验收结论：只有空候选满足空期望，其他候选按内容差异失败。
    #[test]
    fn v1_empty_selection_documents_the_legacy_byte_comparison() {
        let v1 = RuleEnvelope::v1();

        assert_eq!(
            check(&v1, &[], b"").state,
            CheckState::Passed,
            "legacy byte comparison: an empty expectation is satisfied only by empty bytes"
        );

        let non_empty = check(&v1, &[], b"anything");
        assert_eq!(non_empty.state, CheckState::Failed);
        assert!(non_empty.blocking);
        assert_eq!(
            non_empty.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH),
            "an empty selection must not be reported as a separator or ordering problem"
        );
    }

    /// **v2 空集合 = 畸形合同（章程 §21 D-1 冻结的期望）。**
    ///
    /// 空集合被冻结前置条件排除（`delivery_content_requirements.rs:284-285` 的
    /// `count∈1..=64`、`:313`、`:316`），所以 v2 遇到它意味着合同已畸形：本项报
    /// `Unverified`/`CONTRACT_UNREADABLE`，**不携带候选侧的 expected/actual/locator**，
    /// 空候选与非空候选**同判**——不得因为"候选也是空"就当作通过。判定发生在
    /// 强制 EOF 与字节相同分支**之前**，因此也不依赖任何 EOF 政策。
    ///
    /// **期望变更记录（章程 §21 D-1）**：本测试原为**行为观测**，只断言
    /// "`state != Failed`"与"非空候选 `!= Passed`"，并在注释中写明"行为未决、
    /// 不主张与 v1 行为一致即验收"。§21 D-1 冻结后，本测试升格为回归断言
    /// （旧两条断言的语义仍成立，现由更强的断言覆盖），旧表述保留于此作为记录。
    /// 旧版观测测试（G 的 round-1 `g_v2_empty_selection_is_recorded_not_prescribed`）
    /// 属历史观察，由 G 的新版独立期望取代，本卡不改 G 测试。
    #[test]
    fn v2_empty_selection_is_a_malformed_contract() {
        for comparison in [ComparisonMode::BytesExact, ComparisonMode::TextSegments] {
            for eof in [
                EofPolicy::ByteExact,
                EofPolicy::SourceVerbatim,
                EofPolicy::AllowSingleTrailingLf,
                EofPolicy::RequireTrailingLf,
            ] {
                for candidate in [&b""[..], &b"anything"[..]] {
                    let envelope = v2_with(comparison, eof);
                    let observation = check(&envelope, &[], candidate);
                    let label = format!("{comparison:?}/{eof:?}/{candidate:?}");
                    assert_eq!(
                        observation.state,
                        CheckState::Unverified,
                        "state must not depend on the comparison mode, the EOF policy \
                         or the candidate being empty: {label}"
                    );
                    assert!(!observation.blocking, "{label}");
                    assert_eq!(
                        observation.reason_code.as_deref(),
                        Some(reason_code::CONTRACT_UNREADABLE),
                        "{label}"
                    );
                    assert_eq!(observation.equivalence, None, "{label}");
                    assert!(
                        observation.expected.is_none() && observation.actual.is_none(),
                        "a contract-side finding carries no candidate expected/actual: {label}"
                    );
                    assert!(
                        observation.locator.is_none(),
                        "a contract-side finding carries no candidate locator: {label}"
                    );
                    assert!(
                        observation
                            .detail
                            .as_deref()
                            .is_some_and(|detail| detail.contains("选择集合为空")),
                        "the malformed selection must be named: {label} / {:?}",
                        observation.detail
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // R1：严格比较不得被行结束"无法判定"抢先降级
    // -----------------------------------------------------------------------

    /// **R1 定点反例（全部组合逐项执行，首条失败不得遮蔽其余）。**
    ///
    /// `BytesExact`、以及 `ByteExact` / `SourceVerbatim` 这两个不授予任何等价的
    /// EOF 政策，都必须自行判定每一个字节差异：字节不等即不等，无需裁定行结束
    /// 语义。因此含 CRLF、孤立 CR、末尾 LF、正文改动的候选一律是**阻断失败**。
    #[test]
    fn r1_strict_modes_decide_every_difference() {
        let segments = [segment("in/a.txt", b"alpha")];
        let strict = [
            (
                "BytesExact+ByteExact",
                ComparisonMode::BytesExact,
                EofPolicy::ByteExact,
            ),
            (
                "BytesExact+SourceVerbatim",
                ComparisonMode::BytesExact,
                EofPolicy::SourceVerbatim,
            ),
            (
                "BytesExact+AllowSingleTrailingLf",
                ComparisonMode::BytesExact,
                EofPolicy::AllowSingleTrailingLf,
            ),
            (
                "TextSegments+ByteExact",
                ComparisonMode::TextSegments,
                EofPolicy::ByteExact,
            ),
            (
                "TextSegments+SourceVerbatim",
                ComparisonMode::TextSegments,
                EofPolicy::SourceVerbatim,
            ),
        ];
        let candidates = [
            ("crlf-only", &b"a.txt\r\nalpha"[..]),
            ("lone-cr-only", &b"a.txt\ralpha"[..]),
            ("trailing-lf", &b"a.txt\nalpha\n"[..]),
            ("body-change", &b"a.txt\nalphX"[..]),
        ];

        let mut unexpected = Vec::new();
        for (label, comparison, eof) in strict {
            let envelope = v2_with(comparison, eof);
            for (case, actual) in candidates {
                let observation = check(&envelope, &segments, actual);
                if observation.state != CheckState::Failed || !observation.blocking {
                    unexpected.push((
                        label,
                        case,
                        observation.state.as_str(),
                        observation.blocking,
                        observation.reason_code.clone(),
                    ));
                }
            }
        }
        assert!(
            unexpected.is_empty(),
            "every strict combination must decide every difference as a blocking failure; unexpected: {unexpected:?}"
        );
    }

    // -----------------------------------------------------------------------
    // R2：合同冲突不得吞掉可独立证明的失败；允许的 EOF 差异不得被误报为正文错
    // -----------------------------------------------------------------------

    /// **R2(a)：冲突 + 允许的末尾 LF 差异 → 未核验（不得被误报为正文错误）。**
    ///
    /// 来源正文中段含 CRLF（故声明 `Lf` 与"来源原样"矛盾），构造本身无末尾 LF，
    /// 候选恰好多出一个末尾 LF——这是冻结合同明确允许的等价，不是正文差异。
    #[test]
    fn r2_conflict_plus_allowed_eof_difference_stays_undecided() {
        let segments = [segment("in/a.txt", b"alpha\r\nbeta")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let expected = concat_of(&segments);
        let mut actual = expected.clone();
        actual.push(b'\n');

        let observation = check(&envelope, &segments, &actual);
        assert_eq!(
            observation.state,
            CheckState::Unverified,
            "an allowed trailing LF must not be reported as a body error: {:?}",
            observation.reason_code
        );
        assert!(!observation.blocking);
        assert!(
            observation
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("矛盾")),
            "the contradiction must still be named: {:?}",
            observation.detail
        );
    }

    /// **R2(b)：冲突 + 可独立证明的正文错误 → 必须阻断，并保留定位与冲突说明。**
    #[test]
    fn r2_conflict_plus_a_provable_body_error_still_blocks() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let observation = check(&envelope, &segments, b"a.txt\nalphX\r\n");
        assert_eq!(
            observation.state,
            CheckState::Failed,
            "a contradiction must not swallow a line-ending-independent body error"
        );
        assert!(observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH),
            "the accurate content finding must be kept"
        );
        assert!(
            observation.locator.is_some(),
            "the location must be kept"
        );
        assert!(
            observation
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("矛盾")),
            "the contradiction must be appended: {:?}",
            observation.detail
        );
    }

    // -----------------------------------------------------------------------
    // R3：准确归因——来源正文冲突 vs 构造器格式冲突
    // -----------------------------------------------------------------------

    /// 声明 `Crlf`：破坏声明的裸 LF 是**构造器自己写的标题/段间 LF**，而来源本身
    /// 是 CRLF。诊断必须指向构造格式，且不得说来源含裸 LF。
    #[test]
    fn r3_a_crlf_declaration_blames_the_constructor_format_not_the_source() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Crlf;

        let expected = concat_of(&segments);
        let observation = check(&envelope, &segments, &expected);
        let detail = observation.detail.clone().unwrap_or_default();
        assert!(
            detail.contains("合并格式自身"),
            "the constructor format must be named: {detail}"
        );
        assert!(
            !detail.contains("已含 CRLF"),
            "the source body must not be blamed for carrying CRLF: {detail}"
        );
        assert_ne!(
            observation.state,
            CheckState::Passed,
            "an unsatisfiable declaration must not pass"
        );
    }

    /// 声明 `Lf`：构造器写的是 LF，符合声明；冲突只可能来自**来源正文**的 CRLF。
    #[test]
    fn r3_an_lf_declaration_blames_the_source_body() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let expected = concat_of(&segments);
        let observation = check(&envelope, &segments, &expected);
        let detail = observation.detail.clone().unwrap_or_default();
        assert!(
            detail.contains("已含 CRLF"),
            "the source body must be named as the side carrying CRLF: {detail}"
        );
        assert!(
            !detail.contains("合并格式自身"),
            "the constructor format must not be blamed: {detail}"
        );
        assert_ne!(observation.state, CheckState::Passed);
    }

    // -----------------------------------------------------------------------
    // R7：强制 EOF（RequireTrailingLf）与可选 EOF（AllowSingleTrailingLf）必须分开
    // -----------------------------------------------------------------------

    /// `construction` with exactly `count` trailing LFs.
    fn eof_counted(concat: &[u8], count: usize) -> Vec<u8> {
        let mut out = concat.to_vec();
        while out.last() == Some(&b'\n') {
            out.pop();
        }
        for _ in 0..count {
            out.push(b'\n');
        }
        out
    }

    /// One row of the mandatory-vs-optional EOF matrix. Every row states the
    /// expected three-state verdict, whether it must block, which equivalence (if
    /// any) may be recorded, and whether the failure must carry the EOF code.
    struct EofCase {
        label: &'static str,
        policy: EofPolicy,
        declared: LineEnding,
        source: &'static [u8],
        candidate_eof: usize,
        state: CheckState,
        blocking: bool,
        equivalence: Option<&'static str>,
        eof_code: bool,
    }

    /// **R7 交叉矩阵：来源末尾 0/1 LF × 候选 0/1/2 LF × 无冲突／来源冲突／构造冲突 ×
    /// Require／Allow。**
    ///
    /// 全部行先执行、再一次性汇总，因此首条不符合不会遮蔽其余行；`executed` 断言防止
    /// 某行被静默跳过。
    ///
    /// 关键区分（本卡语义 3）：
    /// * `RequireTrailingLf` **不是**"0/1 任选"的可选容忍——缺失（0）与多余（≥2）都是
    ///   可独立判定的硬性违规，必须 `Failed` 且阻断；
    /// * 候选**已满足**强制要求（恰 1 个 LF）且其余内容正确时，不得因为构造原本 0 个
    ///   LF 就把那个被要求的 LF 误报为正文差异；
    /// * `AllowSingleTrailingLf` 的原有允许范围（每侧 0 或 1）不变，2 个 LF 仍必须失败。
    #[test]
    fn r7_mandatory_and_optional_eof_matrix() {
        let cases = vec![
            // ---- RequireTrailingLf, no conflict (declared Any) ----
            EofCase { label: "REQ/any/src0lf/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/any/src0lf/cand1", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 1, state: CheckState::Passed, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "REQ/any/src0lf/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/any/src1lf/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/any/src1lf/cand1", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 1, state: CheckState::Passed, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "REQ/any/src1lf/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            // ---- RequireTrailingLf under a source-body conflict (declared Lf) ----
            EofCase { label: "REQ/srcConflict/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/srcConflict/cand1-identical", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            // The "candidate keeps exactly the mandated LF but ends the body with
            // LF only" variant is not expressible as a trailing-LF count from the
            // construction, so it is covered separately by
            // `r7_a_satisfied_mandatory_eof_with_an_lf_ending_stays_undecided`.
            EofCase { label: "REQ/srcConflict/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            // ---- RequireTrailingLf + conflict where the construction has NO trailing LF ----
            // This is the row that proves narrowing the discount to
            // `AllowSingleTrailingLf` alone would be wrong: the candidate satisfied
            // the mandated LF, so the single LF is what the contract asked for and
            // must not be reported as an independent body difference.
            EofCase { label: "REQ/srcConflictMid/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\nbeta", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/srcConflictMid/cand1", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\nbeta", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "REQ/srcConflictMid/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\nbeta", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            // ---- RequireTrailingLf under a constructor-format conflict (declared Crlf) ----
            EofCase { label: "REQ/ctorConflict/src0lf/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/ctorConflict/src0lf/cand1", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "REQ/ctorConflict/src0lf/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/ctorConflict/src1lf/cand0", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha\n", candidate_eof: 0, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "REQ/ctorConflict/src1lf/cand1", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha\n", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "REQ/ctorConflict/src1lf/cand2", policy: EofPolicy::RequireTrailingLf, declared: LineEnding::Crlf, source: b"alpha\n", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            // ---- AllowSingleTrailingLf controls: the existing window is unchanged ----
            EofCase { label: "ALLOW/any/src0lf/cand0", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 0, state: CheckState::Passed, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/any/src0lf/cand1", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 1, state: CheckState::Passed, blocking: false, equivalence: Some(equivalence::OPTIONAL_FINAL_LF), eof_code: false },
            EofCase { label: "ALLOW/any/src0lf/cand2", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "ALLOW/any/src1lf/cand0", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 0, state: CheckState::Passed, blocking: false, equivalence: Some(equivalence::OPTIONAL_FINAL_LF), eof_code: false },
            EofCase { label: "ALLOW/any/src1lf/cand1", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 1, state: CheckState::Passed, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/any/src1lf/cand2", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Any, source: b"alpha\n", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "ALLOW/srcConflict/cand0", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 0, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/srcConflict/cand1", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/srcConflict/cand2", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Lf, source: b"alpha\r\n", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
            EofCase { label: "ALLOW/ctorConflict/src0lf/cand0", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 0, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/ctorConflict/src0lf/cand1", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 1, state: CheckState::Unverified, blocking: false, equivalence: None, eof_code: false },
            EofCase { label: "ALLOW/ctorConflict/src0lf/cand2", policy: EofPolicy::AllowSingleTrailingLf, declared: LineEnding::Crlf, source: b"alpha", candidate_eof: 2, state: CheckState::Failed, blocking: true, equivalence: None, eof_code: true },
        ];

        let mut mismatches: Vec<String> = Vec::new();
        let mut executed = 0usize;
        for case in &cases {
            let segments = [segment("in/a.txt", case.source)];
            let concat = concat_of(&segments);
            let actual = eof_counted(&concat, case.candidate_eof);
            let mut envelope = v2_with(ComparisonMode::TextSegments, case.policy);
            envelope.policy.table.line_ending = case.declared;
            let observation = check(&envelope, &segments, &actual);
            executed += 1;

            let mut problems = Vec::new();
            if observation.state != case.state {
                problems.push(format!("state={:?}", observation.state));
            }
            if observation.blocking != case.blocking {
                problems.push(format!("blocking={}", observation.blocking));
            }
            if observation.equivalence.as_deref() != case.equivalence {
                problems.push(format!("equivalence={:?}", observation.equivalence));
            }
            if case.eof_code
                && observation.reason_code.as_deref() != Some(reason_code::TEXT_EOF_MISMATCH)
            {
                problems.push(format!("code={:?}", observation.reason_code));
            }
            if !problems.is_empty() {
                mismatches.push(format!(
                    "{}: {} (code={:?})",
                    case.label,
                    problems.join(", "),
                    observation.reason_code
                ));
            }
        }

        assert_eq!(executed, cases.len(), "every matrix row must run");
        assert_eq!(executed, 30, "the matrix has 30 rows");
        assert!(
            mismatches.is_empty(),
            "mandatory vs optional EOF matrix mismatches ({} rows executed): {mismatches:#?}",
            executed
        );
    }

    /// **R7 缺失侧**：强制 EOF 缺失必须带 EOF 码、位置与冲突说明，不得被记成来源或正文问题。
    #[test]
    fn r7_a_missing_mandatory_eof_keeps_its_code_location_and_conflict_note() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::RequireTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let observation = check(&envelope, &segments, b"a.txt\nalpha");
        assert_eq!(observation.state, CheckState::Failed);
        assert!(observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH),
            "the missing hard requirement must not be reported as a source or content problem"
        );
        assert_eq!(observation.expected.as_deref(), Some("末尾 LF 1 个"));
        assert_eq!(observation.actual.as_deref(), Some("末尾 LF 0 个"));
        assert_eq!(
            observation.locator.as_ref().and_then(|l| l.byte_offset),
            Some(11),
            "the position where the trailing run would start must be reported"
        );
        assert!(
            observation
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("矛盾")),
            "the contradiction must be retained alongside the hard-requirement failure: {:?}",
            observation.detail
        );
    }

    /// **R7 满足侧（LF 结尾变体）**：候选保留了强制要求的那个 LF，只是正文自身以 LF
    /// 结束（来源是 CRLF）。合同无解，故按现行裁定未核验，且**不得**记录任何等价。
    #[test]
    fn r7_a_satisfied_mandatory_eof_with_an_lf_ending_stays_undecided() {
        let segments = [segment("in/a.txt", b"alpha\r\n")];
        let mut envelope = v2_text(EofPolicy::RequireTrailingLf);
        envelope.policy.table.line_ending = LineEnding::Lf;

        let observation = check(&envelope, &segments, b"a.txt\nalpha\n");
        assert_eq!(
            observation.state,
            CheckState::Unverified,
            "the mandated LF is satisfied; only the unsolvable convention question remains"
        );
        assert!(!observation.blocking);
        assert_eq!(
            observation.equivalence, None,
            "an undecided outcome is not a pass and records no equivalence"
        );
    }

    /// **R7 满足侧**：`RequireTrailingLf` 不是"0/1 任选"的容忍，而"为满足它补上的 LF"
    /// 也不是正文差异——两侧对照同一条规则。
    #[test]
    fn r7_require_trailing_lf_is_not_an_either_or_tolerance() {
        let segments = [segment("in/a.txt", b"alpha")];
        let required = v2_text(EofPolicy::RequireTrailingLf);

        // Unsatisfied (0) and excess (2) are hard violations.
        for count in [0usize, 2] {
            let observation = check(&required, &segments, &eof_counted(&concat_of(&segments), count));
            assert_eq!(observation.state, CheckState::Failed, "{count} trailing LFs");
            assert!(observation.blocking, "{count} trailing LFs");
            assert_eq!(
                observation.reason_code.as_deref(),
                Some(reason_code::TEXT_EOF_MISMATCH),
                "{count} trailing LFs"
            );
        }

        // Exactly one satisfies the contract, and that is a plain pass: the LF was
        // required, not tolerated, so no equivalence may be claimed.
        let satisfied = check(&required, &segments, &eof_counted(&concat_of(&segments), 1));
        assert_eq!(satisfied.state, CheckState::Passed);
        assert!(!satisfied.blocking);
        assert_eq!(
            satisfied.equivalence, None,
            "a satisfied hard requirement is not the optional `0-or-1` equivalence"
        );

        // Control: the optional policy does treat the same shape as a tolerance,
        // and it is the only policy that may record the equivalence.
        let optional = check(
            &v2_text(EofPolicy::AllowSingleTrailingLf),
            &segments,
            &eof_counted(&concat_of(&segments), 1),
        );
        assert_eq!(optional.state, CheckState::Passed);
        assert_eq!(
            optional.equivalence.as_deref(),
            Some(equivalence::OPTIONAL_FINAL_LF),
            "only AllowSingleTrailingLf may claim the optional equivalence"
        );
    }

    // -----------------------------------------------------------------------
    // R8：两侧原样相同的 CRLF 正文 + 合同自己声明/要求的 EOF 差额
    // -----------------------------------------------------------------------

    struct R8Case {
        label: &'static str,
        policy: EofPolicy,
        declared: LineEnding,
        source: &'static [u8],
        candidate: &'static [u8],
        state: CheckState,
        blocking: bool,
        equivalence: Option<&'static str>,
        /// Required reason code when the row is a failure.
        code: Option<&'static str>,
    }

    /// **R8 回归：判定键必须是原始字节，而不是"候选里是否含 CRLF"。**
    ///
    /// 当两侧持有的 CRLF 正文逐字节相同时，两者之差**只有**合同自己声明或要求的那个
    /// 末尾 LF，因此必须按声明通过（Allow 记等价、Require 记普通通过）。同一张表里
    /// 同时放了"归一化后才相等"的真实换行替换行：只要判定回落到
    /// `normalize_line_endings`，这些行就会立刻暴露。
    #[test]
    fn r8_authorized_eof_difference_is_decided_on_original_bytes() {
        let cases = vec![
            // -- 正文中段含 CRLF、两侧逐字节相同（R8 的定点反例）--
            R8Case {
                label: "midCRLF/require/plus1LF",
                policy: EofPolicy::RequireTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta\n",
                state: CheckState::Passed,
                blocking: false,
                equivalence: None,
                code: None,
            },
            R8Case {
                label: "midCRLF/allow/plus1LF",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta\n",
                state: CheckState::Passed,
                blocking: false,
                equivalence: Some(equivalence::OPTIONAL_FINAL_LF),
                code: None,
            },
            R8Case {
                label: "midCRLF/require/noLF",
                policy: EofPolicy::RequireTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta",
                state: CheckState::Failed,
                blocking: true,
                equivalence: None,
                code: Some(reason_code::TEXT_EOF_MISMATCH),
            },
            R8Case {
                label: "midCRLF/allow/byteIdentical",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta",
                state: CheckState::Passed,
                blocking: false,
                equivalence: None,
                code: None,
            },
            R8Case {
                label: "midCRLF/allow/plus2LF",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta\n\n",
                state: CheckState::Failed,
                blocking: true,
                equivalence: None,
                code: None,
            },
            R8Case {
                label: "midCRLF/allow/bodyChange",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\r\nbetX\n",
                state: CheckState::Failed,
                blocking: true,
                equivalence: None,
                code: None,
            },
            // The CRLF -> LF direction of a real conversion: the candidate carries no
            // CRLF at all, so the "undecidable convention" verdict never applies and
            // the strict byte comparison decides. This is **pre-existing** behaviour,
            // not a product of the R8 change: the probe in `A-P02-logs/19-…` (pre-fix)
            // and `20-…` (post-fix) return the same `Failed/content_mismatch` for this
            // fixture. The opposite direction (`lfBody/allow/realConversion`) stays
            // `Unverified`, and that asymmetry belongs to the still-open
            // declared-conversion-equivalence item, not to R8.
            R8Case {
                label: "midCRLF/allow/realConversion",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\r\nbeta",
                candidate: b"a.txt\nalpha\nbeta",
                state: CheckState::Failed,
                blocking: true,
                equivalence: None,
                code: Some(reason_code::TEXT_CONTENT_MISMATCH),
            },
            // -- 纯 LF 正文对照：同样的形态必须得到同样的结论 --
            R8Case {
                label: "lfBody/require/plus1LF",
                policy: EofPolicy::RequireTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\nbeta",
                candidate: b"a.txt\nalpha\nbeta\n",
                state: CheckState::Passed,
                blocking: false,
                equivalence: None,
                code: None,
            },
            R8Case {
                label: "lfBody/allow/plus1LF",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\nbeta",
                candidate: b"a.txt\nalpha\nbeta\n",
                state: CheckState::Passed,
                blocking: false,
                equivalence: Some(equivalence::OPTIONAL_FINAL_LF),
                code: None,
            },
            R8Case {
                label: "lfBody/allow/realConversion",
                policy: EofPolicy::AllowSingleTrailingLf,
                declared: LineEnding::Any,
                source: b"alpha\nbeta",
                candidate: b"a.txt\nalpha\r\nbeta\n",
                state: CheckState::Unverified,
                blocking: false,
                equivalence: None,
                code: None,
            },
        ];

        let mut mismatches: Vec<String> = Vec::new();
        let mut executed = 0usize;
        for case in &cases {
            let segments = [segment("in/a.txt", case.source)];
            let mut envelope = v2_with(ComparisonMode::TextSegments, case.policy);
            envelope.policy.table.line_ending = case.declared;
            let observation = check(&envelope, &segments, case.candidate);
            executed += 1;

            let mut problems = Vec::new();
            if observation.state != case.state {
                problems.push(format!("state={:?}", observation.state));
            }
            if observation.blocking != case.blocking {
                problems.push(format!("blocking={}", observation.blocking));
            }
            if observation.equivalence.as_deref() != case.equivalence {
                problems.push(format!("equivalence={:?}", observation.equivalence));
            }
            if let Some(code) = case.code {
                if observation.reason_code.as_deref() != Some(code) {
                    problems.push(format!("code={:?}", observation.reason_code));
                }
            }
            if !problems.is_empty() {
                mismatches.push(format!(
                    "{}: {} (code={:?})",
                    case.label,
                    problems.join(", "),
                    observation.reason_code
                ));
            }
        }

        assert_eq!(executed, cases.len(), "every R8 row must run");
        assert_eq!(executed, 10, "the R8 table has 10 rows");
        assert!(
            mismatches.is_empty(),
            "R8 rows must be decided on the original bytes ({} rows executed): {mismatches:#?}",
            executed
        );
    }

    // =======================================================================
    // F-P02-07 budget entry-point tests (source + tests delivered without
    // compilation per card; the meter is exercised through the real public
    // leaf entries, not through predicate-only unit tests).
    // =======================================================================

    use super::budget::{Clock, FakeClock};
    use std::cell::Cell as StdCell;
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    fn limits_with(scan: u64, millis: u64, text_bytes: usize, bytes: u64) -> CheckLimits {
        let mut limits = CheckLimits::P0;
        limits.max_scan_bytes = scan;
        limits.max_millis = millis;
        limits.max_text_bytes = text_bytes;
        limits.max_bytes = bytes;
        limits
    }

    fn check_budgeted<'a>(
        envelope: &'a RuleEnvelope,
        segments: &'a [TextSegment<'a>],
        actual: &'a [u8],
        limits: &'a CheckLimits,
        clock: &'a dyn Clock,
    ) -> CheckObservation {
        check_text_concat_budgeted(
            &TextConcatRequest {
                rule_id: "content:concat",
                envelope,
                segments,
                actual,
                binding: &binding(),
            },
            limits,
            clock,
        )
    }

    /// A clock that reports `budget_ms` from the (`after`+1)-th `now()` call
    /// on, and zero before. The production path records its real phase on
    /// this clock instance before each metered operation (`record_phase`),
    /// so a test can assert *where* the deadline was first observed — e.g.
    /// that a Complete outcome became K1 at `Finalize`, not at an earlier
    /// read. Unlike the removed backtrace fixture, this is stable under
    /// `debug=0`: the phase label comes from the actual call site, not a
    /// stack symbol. Instance-bound (no global counter), so parallel tests
    /// never cross-talk.
    struct StepClock {
        start: Instant,
        calls: StdCell<u32>,
        after: u32,
        budget_ms: u64,
        /// Production phases in call order, one per metered operation.
        phases: RefCell<Vec<Phase>>,
        /// Phase recorded just before the first expired `now()`.
        first_expired: RefCell<Option<Phase>>,
    }

    impl StepClock {
        fn expire_after_call(after: u32, budget_ms: u64) -> Self {
            Self {
                start: Instant::now(),
                calls: StdCell::new(0),
                after,
                budget_ms,
                phases: RefCell::new(Vec::new()),
                first_expired: RefCell::new(None),
            }
        }

        /// Phase of the metered operation whose time check first observed
        /// the deadline. Recorded at most once (the second `now()` inside
        /// error construction does not overwrite it).
        fn expired_phase(&self) -> Option<Phase> {
            *self.first_expired.borrow()
        }

        fn call_count(&self) -> u32 {
            self.calls.get()
        }

        fn recorded_phases(&self) -> Vec<Phase> {
            self.phases.borrow().clone()
        }
    }

    impl Clock for StepClock {
        fn now(&self) -> Instant {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n > self.after && self.first_expired.borrow().is_none() {
                *self.first_expired.borrow_mut() = self.phases.borrow().last().copied();
            }
            let ms = if n > self.after { self.budget_ms } else { 0 };
            self.start + Duration::from_millis(ms)
        }

        fn record_phase(&self, phase: Phase) {
            self.phases.borrow_mut().push(phase);
        }
    }

    #[test]
    fn budget_b3_total_46_cap45_stops_with_observed_44() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let actual = b"a.txt\nalpha";
        let limits = limits_with(45, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let observation = check_budgeted(&envelope, &segments, actual, &limits, &clock);
        assert_eq!(observation.state, CheckState::Unverified);
        assert!(!observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::BUDGET_EXCEEDED)
        );
        assert_eq!(observation.limit, Some(LimitKind::ScanBytes));
        // Ten pairs were read after the 24 pre-comparison bytes: observed is
        // the amount actually consumed, never the rejected 46th byte.
        assert_eq!(observation.actual.as_deref(), Some("44"));
    }

    #[test]
    fn budget_b3_cap46_equality_passes_and_cap24_blocks_first_pair() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let actual = b"a.txt\nalpha";
        let clock = FakeClock::new();

        let at_46 = limits_with(46, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        assert_eq!(
            check_budgeted(&envelope, &segments, actual, &at_46, &clock).state,
            CheckState::Passed
        );

        let at_24 = limits_with(24, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let observation = check_budgeted(&envelope, &segments, actual, &at_24, &clock);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::ScanBytes));
        assert_eq!(observation.actual.as_deref(), Some("24"));
    }

    #[test]
    fn budget_k2_scan70_keeps_failed_without_fabricated_locator() {
        // Full 11-byte candidate differing at byte 10. The failure is proven
        // (46) and stays proven through the authorized-EOF retry (70); the
        // diagnosis cannot read its first trim byte at scan 70, so the
        // observation is a generic content failure with no locator.
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let actual = b"a.txt\nalphX";
        let limits = limits_with(70, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let observation = check_budgeted(&envelope, &segments, actual, &limits, &clock);
        assert_eq!(observation.state, CheckState::Failed);
        assert!(observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH)
        );
        assert_eq!(observation.limit, None);
        assert!(observation.locator.is_none(), "no unfunded scan for a locator");
    }

    #[test]
    fn budget_text_bytes_construction_37_vs_36_and_equality_23() {
        // Three 5-byte bodies without trailing LF: 3*(5+1+5) + 2 + 2 = 37.
        let three = [
            segment("a.txt", b"alpha"),
            segment("b.txt", b"alpha"),
            segment("c.txt", b"alpha"),
        ];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let clock = FakeClock::new();
        let too_small = limits_with(64 * 1024 * 1024, 30_000, 36, 64 * 1024 * 1024);
        let observation =
            check_budgeted(&envelope, &three, b"", &too_small, &clock);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::TextBytes));
        assert_eq!(observation.actual.as_deref(), Some("37"));

        let exactly_37 = limits_with(64 * 1024 * 1024, 30_000, 37, 64 * 1024 * 1024);
        let actual = concat_of(&three);
        assert_eq!(
            check_budgeted(&envelope, &three, &actual, &exactly_37, &clock).state,
            CheckState::Passed
        );

        // Two segments, first body already ends with LF (5 bytes):
        // 11 + separator 1 + 11 = 23.
        let two = [segment("a.txt", b"alph\n"), segment("b.txt", b"alpha")];
        let exactly_23 = limits_with(64 * 1024 * 1024, 30_000, 23, 64 * 1024 * 1024);
        let actual = concat_of(&two);
        assert_eq!(
            check_budgeted(&envelope, &two, &actual, &exactly_23, &clock).state,
            CheckState::Passed
        );
    }

    #[test]
    fn budget_bytes_ceiling_equality_and_over() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let clock = FakeClock::new();
        let over = limits_with(64 * 1024 * 1024, 30_000, 32 * 1024 * 1024, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &over, &clock);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Bytes));
        assert_eq!(observation.actual.as_deref(), Some("11"));

        let equal = limits_with(64 * 1024 * 1024, 30_000, 32 * 1024 * 1024, 11);
        assert_eq!(
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &equal, &clock).state,
            CheckState::Passed
        );
    }

    #[test]
    fn budget_zero_millis_stops_before_any_work() {
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let limits = limits_with(64 * 1024 * 1024, 0, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &limits, &clock);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Millis));
    }

    /// Frozen now()→phase trajectory (captured via backtrace, not guessed):
    /// `a.txt`/`alpha`, actual `a.txt\nalpha`. Any reordering of the budgeted
    /// control flow changes this table and fails here.
    #[test]
    fn budget_clock_phase_trajectory() {
        let segments = [segment("in/a.txt", b"alpha")];
        let allow = v2_text(EofPolicy::AllowSingleTrailingLf);
        let require = v2_text(EofPolicy::RequireTrailingLf);
        // Scan bytes generous; the expiring clocks advance only 10 ms, so
        // the ceiling must match.
        let clock_limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);

        let run = |envelope: &RuleEnvelope, actual: &[u8]| {
            let clock = StepClock::expire_after_call(u32::MAX, 10);
            let observation =
                check_budgeted(envelope, &segments, actual, &clock_limits, &clock);
            (observation, clock.call_count())
        };

        // Pass path: 44 time checks; finalize is the last one.
        let (passed, count) = run(&allow, b"a.txt\nalpha");
        assert_eq!((passed.state, count), (CheckState::Passed, 44));
        // Re-run with an expiring clock purely to read the phase at #44.
        let at_finalize = StepClock::expire_after_call(43, 10);
        let observation =
            check_budgeted(&allow, &segments, b"a.txt\nalpha", &clock_limits, &at_finalize);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            at_finalize.expired_phase(),
            Some(Phase::Finalize),
            "the deadline is observed at the terminal check, not at a pair read"
        );
        let at_pair = StepClock::expire_after_call(42, 10);
        let observation =
            check_budgeted(&allow, &segments, b"a.txt\nalpha", &clock_limits, &at_pair);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            at_pair.expired_phase(),
            Some(Phase::RawCompare),
            "one call earlier the same K1 state comes from the last raw pair"
        );

        // Require-missing-LF path: the failure is returned directly after the
        // two tail reads; no finalize time check exists on this branch.
        let (failed, count) = run(&require, b"a.txt\nalpha");
        assert_eq!((failed.state, count), (CheckState::Failed, 21));
        assert_eq!(
            failed.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH)
        );
        assert_eq!(failed.locator.and_then(|loc| loc.byte_offset), Some(11));

        // Normal-undecided path: 80 checks; finalize is #80 after 46
        // normalization reads (#34–79).
        let (undecided, count) = run(&allow, b"a.txt\r\nalpha");
        assert_eq!((undecided.state, count), (CheckState::Unverified, 80));
        assert_eq!(
            undecided.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
        let at_finalize = StepClock::expire_after_call(79, 10);
        let observation =
            check_budgeted(&allow, &segments, b"a.txt\r\nalpha", &clock_limits, &at_finalize);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Millis));
        assert_eq!(at_finalize.expired_phase(), Some(Phase::Finalize));
    }

    #[test]
    fn budget_complete_passed_turns_k1_only_at_terminal_time_check() {
        // Frozen trajectory (see budget_clock_phase_trajectory): the B3 pass
        // path reads 43 times before finalize; the 44th `now()` is the
        // terminal check. Expiring then turns Complete(Passed) into K1;
        // expiring one call later (never observed) keeps Passed.
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);

        let at_terminal = StepClock::expire_after_call(43, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &limits, &at_terminal);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Millis));
        assert_eq!(
            at_terminal.expired_phase(),
            Some(Phase::Finalize),
            "must be the terminal check, not an earlier Unverified stop"
        );

        let before_terminal = StepClock::expire_after_call(44, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &limits, &before_terminal);
        assert_eq!(observation.state, CheckState::Passed);
    }

    #[test]
    fn budget_require_failure_incomplete_before_proven_kept_after() {
        // The Require-missing-LF branch reads its last clock at call #21
        // (the second tail read) and then returns the proven failure
        // directly. So:
        // * expiry observed at #21 stops the tail read itself — the hard
        //   failure is not yet proven: K1 (entry path).
        // * a clock that never expires keeps the proven Failed with its
        //   locator; the branch reads no clock after the failure, which is
        //   why "ProvenFailed kept after a later expiry" for THIS branch is
        //   covered by budget::tests::finalize_... (the Outcome rule) rather
        //   than faked here.
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::RequireTrailingLf);
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);

        let at_last_read = StepClock::expire_after_call(20, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &limits, &at_last_read);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(observation.limit, Some(LimitKind::Millis));
        assert_eq!(
            at_last_read.expired_phase(),
            Some(Phase::TailLf),
            "the deadline stops the last tail read before the failure is proven"
        );

        let never = StepClock::expire_after_call(u32::MAX, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\nalpha", &limits, &never);
        assert_eq!(observation.state, CheckState::Failed);
        assert!(observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_EOF_MISMATCH)
        );
        assert_eq!(observation.locator.and_then(|loc| loc.byte_offset), Some(11));
    }

    #[test]
    fn budget_normal_undecided_turns_k1_only_at_terminal_check() {
        // Frozen trajectory: 79 reads precede finalize (#80) on the CRLF-only
        // undecided path.
        let segments = [segment("in/a.txt", b"alpha")];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);

        let at_terminal = StepClock::expire_after_call(79, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\r\nalpha", &limits, &at_terminal);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::BUDGET_EXCEEDED),
            "the terminal check, not an early read, converts it to K1"
        );
        assert_eq!(observation.limit, Some(LimitKind::Millis));
        assert_eq!(at_terminal.expired_phase(), Some(Phase::Finalize));

        let before_terminal = StepClock::expire_after_call(80, 10);
        let observation =
            check_budgeted(&envelope, &segments, b"a.txt\r\nalpha", &limits, &before_terminal);
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_LINE_ENDING_UNSUPPORTED)
        );
    }

    // ---- Table parser: ScanBytes/Millis through the real three-arg entry ---

    fn parse_with(
        bytes: &[u8],
        limits: &CheckLimits,
        clock: &dyn Clock,
    ) -> Result<ParsedTable, TableError> {
        parse_table_with_clock(bytes, &TableDialect::default(), limits, clock)
    }

    #[test]
    fn budget_table_ab_newline_costs_13_cap12_resource_cap13_ok() {
        let limits = limits_with(12, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_with(b"a,b\n", &limits, &clock).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::ScanBytes));

        let limits = limits_with(13, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let parsed = parse_with(b"a,b\n", &limits, &clock).unwrap();
        assert_eq!(parsed.records.len(), 1);
    }

    #[test]
    fn budget_table_empty_succeeds_even_at_zero_scan() {
        let limits = limits_with(0, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let parsed = parse_with(b"", &limits, &clock).unwrap();
        assert!(parsed.records.is_empty());
        assert!(!parsed.bom_stripped);
    }

    #[test]
    fn budget_table_bom_only_is_three_reads_and_empty_body() {
        let limits = limits_with(3, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let parsed = parse_with(b"\xEF\xBB\xBF", &limits, &clock).unwrap();
        assert!(parsed.records.is_empty());
        assert!(parsed.bom_stripped);

        // One fewer byte: the third BOM byte cannot be read.
        let limits = limits_with(2, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let error = parse_with(b"\xEF\xBB\xBF", &limits, &clock).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::ScanBytes));
    }

    #[test]
    fn budget_table_unterminated_quote_natural_eof_vs_budget_stop() {
        let generous = limits_with(64 * 1024, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_with(b"\"abc", &generous, &clock).unwrap_err();
        assert_eq!(
            error.code,
            reason_code::TABLE_UNTERMINATED_QUOTE,
            "a complete read reaches natural EOF: a syntax refusal, not a budget stop"
        );
        assert_eq!(error.limit, None);

        // One BOM byte + only two P2 bytes: validation cannot finish.
        let tiny = limits_with(3, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let error = parse_with(b"\"abc", &tiny, &clock).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::ScanBytes));
    }

    #[test]
    fn budget_table_non_utf8_is_a_parser_error_without_host_mapping() {
        let limits = limits_with(64 * 1024, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_with(b"\xFF", &limits, &clock).unwrap_err();
        assert_eq!(
            error.code,
            reason_code::TABLE_ENCODING_UNSUPPORTED
        );
        assert_eq!(error.limit, None);
        assert_eq!(error.locator.byte_offset, Some(0));
    }

    #[test]
    fn budget_table_e1_80_natural_eof_offset_zero_or_resource_at_scan_one() {
        let limits = limits_with(64 * 1024, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_with(b"\xE1\x80", &limits, &clock).unwrap_err();
        assert_eq!(error.code, reason_code::TABLE_ENCODING_UNSUPPORTED);
        assert_eq!(error.locator.byte_offset, Some(0));

        let one = limits_with(1, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let error = parse_with(b"\xE1\x80", &one, &clock).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::ScanBytes));
    }

    #[test]
    fn budget_table_decodes_multibyte_fields_with_safe_bounded_decode() {
        let limits = limits_with(64 * 1024, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let three_byte = parse_with(b"\xE4\xBD\xA0\n", &limits, &clock).unwrap();
        assert_eq!(three_byte.records[0].fields[0].value, "你");
        let four_byte = parse_with(b"\xF0\x9F\x98\x80\n", &limits, &clock).unwrap();
        assert_eq!(four_byte.records[0].fields[0].value, "😀");
    }

    #[test]
    fn budget_table_zero_millis_is_resource_at_entry() {
        let limits = limits_with(64 * 1024, 0, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_with(b"a,b\n", &limits, &clock).unwrap_err();
        assert_eq!(error.limit, Some(LimitKind::Millis));
    }
    // =======================================================================
    // K2 diagnostic retention: once ORDER/SOURCE/CONTENT is proven with its
    // locator, a budget stop in a *display window* keeps the code/locator
    // (missing render fields are None), instead of collapsing to the generic
    // CONTENT finding. Scan boundaries were measured per-phase, not copied
    // from the ORDER numbers (A-P0-budget-diagnostic-retention-repair).
    // =======================================================================

    fn k2_segments() -> [TextSegment<'static>; 2] {
        [
            segment("in/a.txt", b"alpha"),
            segment("in/b.txt", b"beta"),
        ]
    }

    fn k2_run_scan(
        segments: &[TextSegment<'_>],
        actual: &[u8],
        scan: u64,
        eof: EofPolicy,
    ) -> CheckObservation {
        let envelope = v2_with(ComparisonMode::TextSegments, eof);
        let limits = limits_with(scan, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        check_budgeted(&envelope, segments, actual, &limits, &clock)
    }

    #[test]
    fn k2_order_proven_then_display_window_stop_keeps_order() {
        let segs = k2_segments();
        let actual = b"b.txt\nbeta\n\na.txt\nalpha"; // 22 bytes, reordered
        // 101: ORDER proven via the reordering search; the bounded display
        // window cannot be rendered.
        let stopped = k2_run_scan(&segs, actual, 101, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(stopped.state, CheckState::Failed);
        assert!(stopped.blocking, "K2 stays blocking");
        assert_eq!(stopped.limit, None, "a display stop is not BUDGET_EXCEEDED");
        assert_eq!(
            stopped.reason_code.as_deref(),
            Some(reason_code::TEXT_ORDER_MISMATCH),
            "the proven ORDER code survives the display-window stop"
        );
        assert!(stopped.expected.is_some(), "the non-window expected list is kept");
        assert!(stopped.actual.is_none(), "the unrendered display field is None");

        // 117: every display window fits — normal path unchanged.
        let rendered = k2_run_scan(&segs, actual, 117, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            rendered.reason_code.as_deref(),
            Some(reason_code::TEXT_ORDER_MISMATCH)
        );
        assert!(rendered.actual.is_some());

        // 100: the fact itself is not proven yet — no fabricated ORDER.
        let incomplete = k2_run_scan(&segs, actual, 100, EofPolicy::AllowSingleTrailingLf);
        assert_ne!(
            incomplete.reason_code.as_deref(),
            Some(reason_code::TEXT_ORDER_MISMATCH)
        );
        assert!(incomplete.locator.is_none(), "no locator before the fact is proven");
    }

    #[test]
    fn k2_source_proven_then_display_window_stop_keeps_source() {
        let segs = k2_segments();
        let missing = b"xxxx\nyyyy\n";
        // 58: SOURCE proven (no CRLF-header locator on this input); window
        // cannot render.
        let stopped = k2_run_scan(&segs, missing, 58, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(stopped.state, CheckState::Failed);
        assert_eq!(
            stopped.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
        assert!(stopped.expected.is_some());
        assert!(stopped.actual.is_none());
        // 68: display fits.
        let rendered = k2_run_scan(&segs, missing, 68, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            rendered.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
        assert!(rendered.actual.is_some());
        // 57: fact incomplete — generic content finding, no SOURCE fabrication.
        let incomplete = k2_run_scan(&segs, missing, 57, EofPolicy::AllowSingleTrailingLf);
        assert_ne!(
            incomplete.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
    }

    #[test]
    fn k2_source_with_crlf_header_locator_kept_through_display_stop() {
        let segs = k2_segments();
        // The candidate begins with a CRLF-terminated header: the CRLF-header
        // locator (byte 0) is an already-held proven fact at scan 122; the
        // display window needs more.
        let candidate = b"a.txt\r\nxxxx yyyy";
        let stopped = k2_run_scan(&segs, candidate, 122, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            stopped.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
        assert_eq!(
            stopped.locator.and_then(|loc| loc.byte_offset),
            Some(0),
            "the CRLF-header locator survives the display stop"
        );
        assert!(stopped.actual.is_none());
        // 138: display window fits.
        let rendered = k2_run_scan(&segs, candidate, 138, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            rendered.reason_code.as_deref(),
            Some(reason_code::TEXT_SOURCE_MISSING)
        );
        assert!(rendered.actual.is_some());
    }

    #[test]
    fn k2_content_three_display_windows_are_read_in_order_and_stop_immediately() {
        // CONTENT display reads are three independent metered windows in a
        // fixed order (charter §42): 1) expected window upgrading only the
        // detail, 2) expected window filling `expected`, 3) actual window.
        // A stop in one returns the observation with only the fields already
        // obtained; no later window is started. Input: a.txt:alpha vs
        // candidate `a.txt\nalphX` (first difference byte 10).
        let segs = [segment("in/a.txt", b"alpha")];
        let candidate = b"a.txt\nalphX";

        // scan 136: locator 10 proven; only the window-free detail exists.
        let at_136 = k2_run_scan(&segs, candidate, 136, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(at_136.state, CheckState::Failed);
        assert!(at_136.blocking);
        assert_eq!(at_136.limit, None);
        assert_eq!(
            at_136.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH)
        );
        assert_eq!(at_136.locator.and_then(|loc| loc.byte_offset), Some(10));
        assert!(
            at_136.detail.as_deref().is_some_and(|detail| detail.contains("字节 10")),
            "window-free detail"
        );
        assert!(at_136.expected.is_none());
        assert!(at_136.actual.is_none());

        // scan 147: window 1 completed (window-version detail); window 2 did
        // not start, so `expected` is still None.
        let at_147 = k2_run_scan(&segs, candidate, 147, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            at_147.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH)
        );
        assert!(
            at_147
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains('「')),
            "window-version detail is set"
        );
        assert!(at_147.expected.is_none(), "window 2 has not filled expected yet");
        assert!(at_147.actual.is_none());

        // scan 158: window 2 completed (expected filled); window 3 not yet.
        let at_158 = k2_run_scan(&segs, candidate, 158, EofPolicy::AllowSingleTrailingLf);
        assert!(at_158.expected.is_some());
        assert!(at_158.actual.is_none(), "window 3 has not filled actual yet");

        // scan 169: window 3 completed — the normal, fully-rendered path
        // (3 × 11 display bytes), code/locator unchanged.
        let at_169 = k2_run_scan(&segs, candidate, 169, EofPolicy::AllowSingleTrailingLf);
        assert_eq!(
            at_169.reason_code.as_deref(),
            Some(reason_code::TEXT_CONTENT_MISMATCH)
        );
        assert_eq!(at_169.locator.and_then(|loc| loc.byte_offset), Some(10));
        assert!(at_169.detail.is_some());
        assert!(at_169.expected.is_some());
        assert!(at_169.actual.is_some());
    }

    /// Deterministic clock for the CONTENT three-window display stops:
    /// production phases are instance-bound; the first expired now() is the
    /// (`after`+1)-th, with its phase captured from the current phase.
    struct ContentMillisClock {
        start: Instant,
        calls: StdCell<u32>,
        after: u32,
        phases: RefCell<Vec<Phase>>,
        first_expired: RefCell<Option<Phase>>,
    }

    impl ContentMillisClock {
        fn at(after: u32) -> Self {
            Self {
                start: Instant::now(),
                calls: StdCell::new(0),
                after,
                phases: RefCell::new(Vec::new()),
                first_expired: RefCell::new(None),
            }
        }

        fn call_count(&self) -> u32 {
            self.calls.get()
        }

        fn first_expired_phase(&self) -> Option<Phase> {
            *self.first_expired.borrow()
        }
    }

    impl Clock for ContentMillisClock {
        fn now(&self) -> Instant {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n > self.after && self.first_expired.borrow().is_none() {
                *self.first_expired.borrow_mut() = self.phases.borrow().last().copied();
            }
            if n > self.after {
                self.start + Duration::from_millis(10)
            } else {
                self.start
            }
        }

        fn record_phase(&self, phase: Phase) {
            self.phases.borrow_mut().push(phase);
        }
    }
    fn k2_run_millis(
        segments: &[TextSegment<'_>],
        actual: &[u8],
        clock: &dyn Clock,
    ) -> CheckObservation {
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        // Generous scan bytes; the only stop is the injected deadline.
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);
        check_budgeted(&envelope, segments, actual, &limits, clock)
    }

    #[test]
    fn k2_content_millis_stop_in_each_display_window_keeps_completed_fields() {
        // Window clock calls measured on this entry (probe): display window
        // bytes are now() #90..#100 (window 1), #101..#111 (window 2),
        // #112..#122 (window 3); the second now() inside error construction
        // is #101/#112/#123. A stop in one window starts no later window.
        let segs = [segment("in/a.txt", b"alpha")];
        let candidate = b"a.txt\nalphX";

        // Deadline first observed inside window 1: only the window-free
        // detail exists; total time checks = 101 (window read + the error
        // construction re-read), first expired phase is Window, and window 2
        // never started.
        let clock = ContentMillisClock::at(99);
        let o = k2_run_millis(&segs, candidate, &clock);
        assert_eq!(o.state, CheckState::Failed);
        assert_eq!(o.reason_code.as_deref(), Some(reason_code::TEXT_CONTENT_MISMATCH));
        assert_eq!(o.locator.and_then(|loc| loc.byte_offset), Some(10));
        assert!(o.detail.as_deref().is_some_and(|d| d.contains("字节 10")));
        assert!(o.detail.as_deref().is_none_or(|d| !d.contains('「')));
        assert!(o.expected.is_none());
        assert!(o.actual.is_none());
        assert_eq!(clock.call_count(), 101);
        assert_eq!(clock.first_expired_phase(), Some(Phase::Window));

        // Inside window 2: window-version detail kept; expected/actual None;
        // total 112 time checks; window 3 not started.
        let clock = ContentMillisClock::at(110);
        let o = k2_run_millis(&segs, candidate, &clock);
        assert_eq!(o.reason_code.as_deref(), Some(reason_code::TEXT_CONTENT_MISMATCH));
        assert!(o.detail.as_deref().is_some_and(|d| d.contains('「')));
        assert!(o.expected.is_none());
        assert!(o.actual.is_none());
        assert_eq!(clock.call_count(), 112);
        assert_eq!(clock.first_expired_phase(), Some(Phase::Window));

        // Inside window 3: detail + expected kept; actual None; total 123.
        let clock = ContentMillisClock::at(121);
        let o = k2_run_millis(&segs, candidate, &clock);
        assert!(o.expected.is_some());
        assert!(o.actual.is_none());
        assert_eq!(clock.call_count(), 123);
        assert_eq!(clock.first_expired_phase(), Some(Phase::Window));

        // Deadline after every window: all three display fields present; no
        // expired phase and the full 128-time-check normal path.
        let clock = ContentMillisClock::at(200);
        let o = k2_run_millis(&segs, candidate, &clock);
        assert!(o.detail.is_some() && o.expected.is_some() && o.actual.is_some());
        assert_eq!(clock.first_expired_phase(), None);
        assert_eq!(clock.call_count(), 128);
    }

    /// Phase collection is instance-bound: two independently driven clocks
    /// never share phase/expired state (parallel-test isolation). This runs
    /// alongside the rest of the suite under the default test scheduler.
    #[test]
    fn clock_phase_recording_is_instance_bound_without_cross_talk() {
        let segs = [segment("in/a.txt", b"alpha")];
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);

        // Clock A expires during the finalize check (B3 pass path, #44).
        let clock_a = StepClock::expire_after_call(43, 10);
        let oa = check_budgeted(&envelope, &segs, b"a.txt\nalpha", &limits, &clock_a);
        // Clock B is driven independently and never expires on this run.
        let clock_b = StepClock::expire_after_call(u32::MAX, 10);
        let ob = check_budgeted(&envelope, &segs, b"a.txt\nalpha", &limits, &clock_b);

        assert_eq!(oa.state, CheckState::Unverified);
        assert_eq!(clock_a.expired_phase(), Some(Phase::Finalize));
        assert_eq!(ob.state, CheckState::Passed);
        assert_eq!(clock_b.expired_phase(), None);

        // A new clock sees a fresh phase sequence starting with Entry.
        let clock_c = StepClock::expire_after_call(u32::MAX, 10);
        let _ = check_budgeted(&envelope, &segs, b"a.txt\nalpha", &limits, &clock_c);
        assert_eq!(clock_c.recorded_phases().first().copied(), Some(Phase::Entry));
        // The earlier expired clock A is unaffected by later runs.
        assert_eq!(clock_a.expired_phase(), Some(Phase::Finalize));
    }

    /// Deterministic clock for the ORDER display-phase stop. Instance-bound
    /// production phases; the first expired check records its phase.
    struct MillisStepClock {
        start: Instant,
        calls: StdCell<u32>,
        after: u32,
        budget_ms: u64,
        phases: RefCell<Vec<Phase>>,
        first_expired: RefCell<Option<Phase>>,
    }

    impl MillisStepClock {
        fn at(after: u32) -> Self {
            Self {
                start: Instant::now(),
                calls: StdCell::new(0),
                after,
                budget_ms: 10,
                phases: RefCell::new(Vec::new()),
                first_expired: RefCell::new(None),
            }
        }
        fn expired_phase(&self) -> Option<Phase> {
            *self.first_expired.borrow()
        }
    }

    impl Clock for MillisStepClock {
        fn now(&self) -> Instant {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n > self.after && self.first_expired.borrow().is_none() {
                *self.first_expired.borrow_mut() = self.phases.borrow().last().copied();
            }
            if n > self.after {
                self.start + Duration::from_millis(self.budget_ms)
            } else {
                self.start
            }
        }

        fn record_phase(&self, phase: Phase) {
            self.phases.borrow_mut().push(phase);
        }
    }
    #[test]
    fn k2_millis_display_phase_keeps_order_code() {
        // The fact scans (verdict/tail/raw/normalization/header search) all
        // observe an unexpired clock; the deadline is first observed by a
        // display window read. Scan bytes are generous, so the stop is Millis
        // (not ScanBytes), and the proven ORDER code with its expected list
        // is returned with actual=None.
        let segs = k2_segments();
        let actual = b"b.txt\nbeta\n\na.txt\nalpha";
        let limits = limits_with(64 * 1024 * 1024, 10, 32 * 1024 * 1024, 64 * 1024 * 1024);
        // 117 time checks on the generous-scan ORDER path; expiring after 116
        // puts the first expired check inside the display window (#117).
        let clock = MillisStepClock::at(116);
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let observation = check_text_concat_budgeted(
            &TextConcatRequest {
                rule_id: "content:concat",
                envelope: &envelope,
                segments: &segs,
                actual,
                binding: &binding(),
            },
            &limits,
            &clock,
        );
        assert_eq!(observation.state, CheckState::Failed);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TEXT_ORDER_MISMATCH),
            "a display-phase time stop keeps the proven ORDER code"
        );
        assert_eq!(observation.limit, None, "K2 is never BUDGET_EXCEEDED");
        assert!(observation.actual.is_none());
    }
    // =======================================================================
    // ParseFailureKind classification + table_parse_observation
    // (A-P1-table-error-classification): the parser decides the category at
    // the construction site, not from code/message. Source and candidate
    // failures have different repair implications.
    // =======================================================================

    fn dialect_with(over: impl FnOnce(&mut TableDialect)) -> TableDialect {
        let mut dialect = TableDialect::default();
        over(&mut dialect);
        dialect
    }

    fn parse_err(bytes: &[u8], dialect: &TableDialect) -> TableError {
        parse_table(bytes, dialect, &CheckLimits::P0).expect_err("must be a parse error")
    }

    fn table_binding() -> CandidateBinding {
        CandidateBinding::new("out/r.csv", "s".repeat(64), 1)
    }

    #[test]
    fn parse_kind_non_utf8_and_lone_cr_are_unsupported() {
        // 0xFF is not a legal UTF-8 lead byte; there is no frozen encoding
        // declaration, so this is capability insufficiency (Unsupported),
        // not a definite contract violation.
        let error = parse_err(b"\xFF,b\n", &TableDialect::default());
        assert_eq!(error.kind(), ParseFailureKind::Unsupported);
        assert_eq!(error.limit(), None);

        // A lone CR under the default Any line-ending policy is a dialect
        // the checker cannot safely explain; still Unsupported.
        let error = parse_err(b"a,b\r1,2\n", &TableDialect::default());
        assert_eq!(error.kind(), ParseFailureKind::Unsupported);
    }

    #[test]
    fn parse_kind_declared_line_ending_violations_are_violations() {
        let lf = dialect_with(|d| d.line_ending = LineEnding::Lf);
        let error = parse_err(b"a,b\r\n1,2\n", &lf);
        assert_eq!(error.kind(), ParseFailureKind::Violation);

        let crlf = dialect_with(|d| d.line_ending = LineEnding::Crlf);
        let error = parse_err(b"a,b\n1,2\n", &crlf);
        assert_eq!(error.kind(), ParseFailureKind::Violation);
    }

    #[test]
    fn parse_kind_bom_required_and_forbidden_are_violations() {
        let required = dialect_with(|d| d.bom = BomPolicy::Required);
        let error = parse_err(b"a,b\n", &required);
        assert_eq!(error.kind(), ParseFailureKind::Violation);

        let forbidden = dialect_with(|d| d.bom = BomPolicy::Forbidden);
        let error = parse_err(b"\xEF\xBB\xBFa,b\n", &forbidden);
        assert_eq!(error.kind(), ParseFailureKind::Violation);
    }

    #[test]
    fn parse_kind_quote_syntax_three_branches_are_violations() {
        // Quote opened at the start and never closed at EOF.
        let unterminated = parse_err(b"\"abc", &TableDialect::default());
        assert_eq!(unterminated.kind(), ParseFailureKind::Violation);
        assert_eq!(unterminated.code(), reason_code::TABLE_UNTERMINATED_QUOTE);

        // A quote inside an unquoted field must be doubled/quoted.
        let unescaped = parse_err(b"a\"b\n", &TableDialect::default());
        assert_eq!(unescaped.kind(), ParseFailureKind::Violation);
        assert_eq!(unescaped.code(), reason_code::TABLE_UNESCAPED_QUOTE);

        // A non-delimiter, non-EOL character after a closed quote.
        let after_quote = parse_err(b"\"a\"x\n", &TableDialect::default());
        assert_eq!(after_quote.kind(), ParseFailureKind::Violation);
        assert_eq!(after_quote.code(), reason_code::TABLE_CHARACTER_AFTER_QUOTE);
    }

    #[test]
    fn parse_kind_budget_stops_are_resources_with_a_real_limit() {
        // scan=12 stops the P2/P3 scan of `a,b\n`.
        let limits = limits_with(12, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_table_with_clock(b"a,b\n", &TableDialect::default(), &limits, &clock)
            .expect_err("scan budget");
        assert_eq!(error.kind(), ParseFailureKind::Resource);
        assert_eq!(error.limit(), Some(LimitKind::ScanBytes));
    }

    #[test]
    fn candidate_resource_maps_to_unverified_budget_with_limit() {        let limits = limits_with(12, 30_000, 32 * 1024 * 1024, 64 * 1024 * 1024);
        let clock = FakeClock::new();
        let error = parse_table_with_clock(b"a,b\n", &TableDialect::default(), &limits, &clock)
            .expect_err("scan budget");
        let binding = table_binding();
        let observation = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &binding,
            ParseInput::Candidate,
            &error,
        );
        assert_eq!(observation.state, CheckState::Unverified);
        assert!(!observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::BUDGET_EXCEEDED)
        );
        assert_eq!(observation.limit, Some(LimitKind::ScanBytes));
    }

    #[test]
    fn candidate_resource_builder_hit_keeps_record_line_locator_and_limit() {
        // `a,b\nc,d\n` with max_records = 1: the builder's records ceiling
        // fires with the parser's record/line locator already in hand. The
        // mapping must keep that locator (record/line, not a byte) and the
        // Records limit, as an unverified non-blocking resource stop.
        let mut limits = CheckLimits::P0;
        limits.max_records = 1;
        let clock = FakeClock::new();
        let error = parse_table_with_clock(b"a,b\nc,d\n", &TableDialect::default(), &limits, &clock)
            .expect_err("records budget");
        assert_eq!(error.kind(), ParseFailureKind::Resource);
        assert_eq!(error.limit(), Some(LimitKind::Records));
        // Assert the parser locator shape (record/line present), whatever the
        // exact numbering rule is; the mapping must preserve it identically.
        let parser_record = error.locator().record;
        let parser_line = error.locator().line;
        assert!(parser_record.is_some(), "parser hit a record-count ceiling");
        let binding = table_binding();
        let observation = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &binding,
            ParseInput::Candidate,
            &error,
        );
        assert_eq!(observation.state, CheckState::Unverified);
        assert!(!observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::BUDGET_EXCEEDED)
        );
        assert_eq!(observation.limit, Some(LimitKind::Records));
        assert_eq!(
            observation.locator.as_ref().map(|loc| (loc.record, loc.line)),
            Some((parser_record, parser_line)),
            "the builder record/line locator must survive the mapping"
        );

        // A resource error with no specific position carries the parser's
        // default locator through unchanged (no fabricated byte/line).
        let default_err = TableError::resource(
            LimitKind::ScanBytes,
            64 * 1024 * 1024,
            64 * 1024 * 1024 + 1,
            RecordLocator::default(),
        );
        let default_obs = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &table_binding(),
            ParseInput::Candidate,
            &default_err,
        );
        assert_eq!(
            default_obs.locator.map(|loc| (loc.record, loc.line, loc.byte_offset)),
            Some((None, None, None))
        );
    }

    #[test]
    fn candidate_unsupported_maps_to_unverified_with_parser_locator() {
        let binding = table_binding();

        // Non-UTF-8 lead byte: the parser reached byte 0. Both assertions are
        // hard so a failure of one never hides the other.
        let non_utf8 = parse_err(b"\xFF,b\n", &TableDialect::default());
        assert_eq!(non_utf8.kind(), ParseFailureKind::Unsupported);
        assert_eq!(
            non_utf8.locator().byte_offset,
            Some(0),
            "parser locates the illegal UTF-8 byte at 0"
        );
        let mapped = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &binding,
            ParseInput::Candidate,
            &non_utf8,
        );
        assert_eq!(mapped.state, CheckState::Unverified);
        assert!(!mapped.blocking);
        assert_eq!(
            mapped.reason_code.as_deref(),
            Some(reason_code::TABLE_ENCODING_UNSUPPORTED)
        );
        assert_eq!(mapped.limit, None);
        assert_eq!(
            mapped.locator.and_then(|loc| loc.byte_offset),
            Some(0),
            "the non-UTF-8 byte locator must survive the mapping"
        );

        // Lone CR at byte 3 under the default Any line-ending policy.
        let lone_cr = parse_err(b"a,b\rc", &TableDialect::default());
        assert_eq!(lone_cr.kind(), ParseFailureKind::Unsupported);
        assert_eq!(lone_cr.locator().byte_offset, Some(3));
        let mapped_cr = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &binding,
            ParseInput::Candidate,
            &lone_cr,
        );
        assert_eq!(mapped_cr.state, CheckState::Unverified);
        assert_eq!(
            mapped_cr.reason_code.as_deref(),
            Some(reason_code::TABLE_DIALECT_UNSUPPORTED)
        );
        assert_eq!(
            mapped_cr.locator.and_then(|loc| loc.byte_offset),
            Some(3),
            "the lone-CR locator must survive the mapping"
        );
    }

    #[test]
    fn candidate_violation_maps_to_blocking_failed_with_locator() {
        let error = parse_err(b"\"abc", &TableDialect::default());
        let binding = table_binding();
        let observation = table_parse_observation(
            "r",
            check_name::TABLE_COLUMNS,
            &binding,
            ParseInput::Candidate,
            &error,
        );
        assert_eq!(observation.state, CheckState::Failed);
        assert!(observation.blocking);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::TABLE_UNTERMINATED_QUOTE)
        );
        assert!(observation.locator.is_some());
        assert_eq!(observation.limit, None);
    }

    #[test]
    fn source_failure_is_never_a_candidate_verdict_even_for_violation() {
        // The same parser error on the frozen *source* side must not arm a
        // candidate repair or report a candidate Failed, regardless of the
        // parser kind.
        let binding = table_binding();
        for make_error in [
            || parse_err(b"\"abc", &TableDialect::default()),
            || parse_err(b"\xFF,b\n", &TableDialect::default()),
        ] {
            let error = make_error();
            let observation = table_parse_observation(
                "r",
                check_name::TABLE_COLUMNS,
                &binding,
                ParseInput::Source,
                &error,
            );
            assert_eq!(observation.state, CheckState::Unverified);
            assert!(!observation.blocking, "source errors never repair the candidate");
            assert!(
                observation
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("冻结参考")),
                "the attribution must say this is the frozen reference: {:?}",
                observation.detail
            );
            // The source locator is carried but remains a *reference*
            // position, never presented as a candidate position.
            assert!(observation.locator.is_some());
        }
    }

    #[test]
    fn table_error_kind_limit_invariant_holds_for_constructors() {
        let resource = TableError::resource(
            LimitKind::Records,
            250_000,
            250_001,
            RecordLocator::default(),
        );
        assert_eq!(resource.kind(), ParseFailureKind::Resource);
        assert_eq!(resource.limit(), Some(LimitKind::Records));

        let unsupported = TableError::unsupported(
            reason_code::TABLE_DIALECT_UNSUPPORTED,
            RecordLocator::default(),
            "x",
        );
        assert_eq!(unsupported.kind(), ParseFailureKind::Unsupported);
        assert_eq!(unsupported.limit(), None);

        let violation = TableError::violation(
            reason_code::TABLE_UNTERMINATED_QUOTE,
            RecordLocator::default(),
            "x",
        );
        assert_eq!(violation.kind(), ParseFailureKind::Violation);
        assert_eq!(violation.limit(), None);
        // budget() is an alias for resource.
        let budget = TableError::budget(LimitKind::Bytes, 64, 65, RecordLocator::default());
        assert_eq!(budget.kind(), ParseFailureKind::Resource);
        assert_eq!(budget.code(), reason_code::BUDGET_EXCEEDED);
    }

    // ---- D1: the inter-segment separator is exactly one blank line ----------
    //
    // The merge rule is: every entry is `filename` + LF + the source body copied
    // verbatim, and entries are joined by **exactly one blank line**. The
    // separator must therefore not depend on whether the body happens to end
    // with LF, and a 0-byte body must not add anything beyond the separator.
    //
    // These expectations are written out here from that wording, as literal
    // bytes. They are deliberately **not** produced by `expected_concat` /
    // `build_expected_metered`, because an expectation generated by the code
    // under test could never disagree with it.

    /// `E1`..`E5` of the D1 matrix: the construction itself, byte for byte.
    ///
    /// Each arm builds the expected merge by hand from the rule text, so a
    /// regression in the writer shows up as a byte difference rather than as
    /// two copies of the same mistake agreeing with each other.
    #[test]
    fn d1_separator_is_exactly_one_blank_line_for_every_body_shape() {
        // E1: a single 0-byte body. Entry is `name` + LF, and there is no
        // separator at all (last segment), so nothing else may be appended.
        let e1 = [segment("in/a.txt", b"")];
        assert_eq!(
            concat_of(&e1),
            b"a.txt\n".to_vec(),
            "E1: an empty body must not gain a terminator LF of its own"
        );

        // E2: non-empty bodies with no trailing LF. The separator must first
        // terminate the body's own line, then supply the blank line — two LFs.
        // A body that already ended its line needs only the blank line.
        let e2 = [segment("in/a.txt", b"alpha"), segment("in/b.txt", b"beta")];
        assert_eq!(
            concat_of(&e2),
            b"a.txt\nalpha\n\nb.txt\nbeta".to_vec(),
            "E2: a body without a terminator needs one LF for its line plus the blank line"
        );

        // E3: non-empty bodies that already end with LF. The body's own LF is
        // what terminates the entry, and the separator LF supplies the blank
        // line. Adding a third LF would create two blank lines.
        let e3 = [
            segment("in/a.txt", b"alpha\n"),
            segment("in/b.txt", b"beta\n"),
        ];
        assert_eq!(
            concat_of(&e3),
            b"a.txt\nalpha\n\nb.txt\nbeta\n".to_vec(),
            "E3: an LF-terminated body still gets exactly one separator LF"
        );

        // E4: an empty body followed by a non-empty one. The empty entry is
        // `a.txt` + LF; the separator adds the one blank line; then `b.txt`.
        let e4 = [segment("in/a.txt", b""), segment("in/b.txt", b"beta")];
        assert_eq!(
            concat_of(&e4),
            b"a.txt\n\nb.txt\nbeta".to_vec(),
            "E4: an empty body gets one blank line, not two"
        );

        // E5: several consecutive empty bodies.
        let e5 = [
            segment("in/a.txt", b""),
            segment("in/b.txt", b""),
            segment("in/c.txt", b""),
        ];
        assert_eq!(
            concat_of(&e5),
            b"a.txt\n\nb.txt\n\nc.txt\n".to_vec(),
            "E5: every adjacent empty pair is separated by exactly one blank line"
        );
    }

    /// The three 0-byte B4 sources, as the real acceptance froze them.
    ///
    /// This is the artifact the delivered business case hands to the checker, so
    /// it is asserted as an exact byte string: 44 bytes, matching the rule
    /// wording rather than the historical 46-byte construction.
    #[test]
    fn d1_b4_three_empty_bodies_is_44_bytes() {
        let segments = [
            segment("in/01_周一.txt", b""),
            segment("in/02_周二.txt", b""),
            segment("in/03_周三.txt", b""),
        ];
        let expected = concat_of(&segments);

        let literal = "01_周一.txt\n\n02_周二.txt\n\n03_周三.txt\n".as_bytes();
        assert_eq!(expected, literal, "D1: B4 must be the rule's 44 bytes");
        assert_eq!(expected.len(), 44, "D1: the corrected expectation is 44 B");
        assert_ne!(expected.len(), 46, "D1: 46 B is the defect, not the rule");
    }

    /// The checker must accept that 44-byte artifact, not reject it.
    ///
    /// This is the business-visible half of D1: before the fix the construction
    /// demanded 46 bytes, so a candidate holding the correct 44 bytes failed
    /// with `separator_mismatch`.
    #[test]
    fn d1_the_correct_44_byte_b4_artifact_passes() {
        let segments = [
            segment("in/01_周一.txt", b""),
            segment("in/02_周二.txt", b""),
            segment("in/03_周三.txt", b""),
        ];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        let correct = "01_周一.txt\n\n02_周二.txt\n\n03_周三.txt\n".as_bytes();

        let observation = check(&envelope, &segments, correct);
        assert_eq!(
            observation.state,
            CheckState::Passed,
            "D1: a correct 44-byte merge must pass, got {:?} / {:?}",
            observation.state,
            observation.reason_code
        );

        // The 46-byte shape the defect used to demand must now be the failure:
        // it carries two blank lines between entries.
        let double_blank = "01_周一.txt\n\n\n02_周二.txt\n\n\n03_周三.txt\n".as_bytes();
        let rejected = check(&envelope, &segments, double_blank);
        assert_eq!(rejected.state, CheckState::Failed);
        assert_eq!(
            rejected.reason_code.as_deref(),
            Some(reason_code::TEXT_SEPARATOR_MISMATCH),
            "D1: two blank lines are a separator difference"
        );
    }

    /// The length twin must agree with the bytes actually written.
    ///
    /// `build_expected_metered` allocates from `expected_concat_len` and then
    /// asserts equality in debug builds. Driving the real check over all five
    /// shapes also exercises that agreement in release builds, where the
    /// `debug_assert_eq!` is compiled out.
    #[test]
    fn d1_length_twin_agrees_with_the_written_bytes() {
        let shapes: [&[TextSegment<'static>]; 5] = [
            &[TextSegment { path: "in/a.txt", bytes: b"" }],
            &[
                TextSegment { path: "in/a.txt", bytes: b"alpha" },
                TextSegment { path: "in/b.txt", bytes: b"beta" },
            ],
            &[
                TextSegment { path: "in/a.txt", bytes: b"alpha\n" },
                TextSegment { path: "in/b.txt", bytes: b"beta\n" },
            ],
            &[
                TextSegment { path: "in/a.txt", bytes: b"" },
                TextSegment { path: "in/b.txt", bytes: b"beta" },
            ],
            &[
                TextSegment { path: "in/a.txt", bytes: b"" },
                TextSegment { path: "in/b.txt", bytes: b"" },
                TextSegment { path: "in/c.txt", bytes: b"" },
            ],
        ];
        let envelope = v2_text(EofPolicy::AllowSingleTrailingLf);
        for (index, segments) in shapes.iter().enumerate() {
            let expected = concat_of(segments);
            // The real check, over the construction itself, must conclude
            // "no difference" — which is only reachable when the length twin
            // and the writer agree.
            let observation = check(&envelope, segments, &expected);
            assert_eq!(
                observation.state,
                CheckState::Passed,
                "D1 shape {index}: construction must satisfy its own contract"
            );
        }
    }
}
