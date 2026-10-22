//! r16 P0 delivery-check contract layer (V2).
//!
//! ## What this layer freezes (C0)
//!
//! The minimum public interface P0 needs, and nothing more:
//!
//! * **Rule versioning** — [`RuleEnvelope`] lives *beside* the existing frozen
//!   `rule` payload inside the persisted `ContentContract` spec, so a record
//!   written before this layer existed is still read as v1 (see
//!   [`RuleEnvelope::from_spec`]). A running Run is never silently upgraded.
//! * **The existing three states** — [`CheckState`] is exactly
//!   `passed` / `failed` / `unverified`, and [`CheckState::from`] /
//!   [`CheckState::to_verdict`] are the only bridge to
//!   [`RequirementVerdict`]. No second state machine is introduced.
//! * **Stable reason codes** — [`reason_code`]; the strings are the contract.
//! * **Candidate binding** — [`CandidateBinding`] (artifact id / version /
//!   hash / bytes) for the bytes a check actually read.
//! * **Location** — [`RecordLocator`] (record, physical line, field, column,
//!   byte offset).
//! * **Input type** — [`InputKind`].
//! * **Resource ceilings and over-limit semantics** — [`CheckLimits`],
//!   [`LimitKind`] and [`budget_exceeded`]: exceeding a ceiling yields an
//!   explicit `unverified` observation naming the limit. It never loops the
//!   model and never degrades into `passed`.
//!
//! ## What this layer deliberately does not do
//!
//! It implements **no check logic** (P0 owns that in `text_csv.rs`), it does not
//! touch `delivery.rs` / `delivery_content_requirements.rs` expectations, it
//! reads no database, and it decides no approval, source-freeze or version
//! credential. Those are the security vetoes in [`SecurityVeto`], which are
//! separate and **never downgradable**.
//!
//! ## Ownership
//!
//! `mod.rs`, the public DTOs and the freeze tests are owned by A (contract and
//! integration). After the C0 base commit is published the leaf modules move to
//! their wave owners: `text_csv.rs` → B, `typed_metrics.rs` → C,
//! `lineage_roles.rs` → D, `fact_pdf.rs` → E. A leaf owner never edits the DTOs
//! here; a missing field is requested from A, who publishes a new base.
//!
// C0 freeze window: the interface lands here before it is wired, so the DTOs and
// leaf entry points are intentionally unused until P0 wires them. A must remove
// this allow in the same commit that wires the first checker (P0-1/P0-2).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::delivery::RequirementVerdict;

pub(crate) mod budget;
pub(crate) mod fact_pdf;
pub(crate) mod lineage_roles;
pub(crate) mod merge_rule;
pub(crate) mod text_csv;
pub(crate) mod typed_metrics;

use merge_rule::MergeRule;

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// Envelope generation of a frozen content-contract spec. An absent field means
/// v1: records written before this layer existed are read unchanged.
pub(crate) const ENVELOPE_VERSION_V1: u32 = 1;
/// Envelope generation that may carry an explicit [`RulePolicy`].
pub(crate) const ENVELOPE_VERSION_V2: u32 = 2;
/// Checker generation for v1 records (the behaviour that exists today).
pub(crate) const CHECKER_VERSION_V1: u32 = 1;
/// Checker generation for rules frozen by P0 and later.
pub(crate) const CHECKER_VERSION_V2: u32 = 2;
/// Version of the `rule` payload (the `Contract` enum in
/// `delivery_content_requirements.rs`).
///
/// C0 does **not** change the shape of `rule`. That shape is matched with
/// `deny_unknown_fields`, so adding a key inside a `rule` variant would make
/// every existing reader reject the whole contract. Therefore all new fields
/// live beside `rule`, and this stays `1`.
pub(crate) const RULE_PAYLOAD_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Stable reason codes
// ---------------------------------------------------------------------------

/// Stable reason codes. **The strings are the contract**: once published they
/// may be added to but never renamed, because they are persisted in
/// `finding_json` and shown to the user.
pub(crate) mod reason_code {
    // Text merge / EOF (F01).
    pub(crate) const TEXT_SOURCE_MISSING: &str = "delivery.check.text.source_missing";
    pub(crate) const TEXT_ORDER_MISMATCH: &str = "delivery.check.text.order_mismatch";
    pub(crate) const TEXT_SEPARATOR_MISMATCH: &str = "delivery.check.text.separator_mismatch";
    pub(crate) const TEXT_CONTENT_MISMATCH: &str = "delivery.check.text.content_mismatch";
    /// The two spellings of "one optional final LF" no longer agree.
    pub(crate) const TEXT_EOF_MISMATCH: &str = "delivery.check.text.eof_mismatch";
    /// The bytes contradict a line-ending declaration the frozen contract
    /// actually made. A declared policy is enforced, so this is `failed`.
    pub(crate) const TEXT_LINE_ENDING_VIOLATION: &str =
        "delivery.check.text.line_ending_violation";
    /// Nothing in the declaration decides these line endings, so no faithful
    /// comparison exists. Reported as `unverified`: "we cannot compare this" is
    /// not evidence that the content is wrong, and converting silently is
    /// forbidden (R05).
    pub(crate) const TEXT_LINE_ENDING_UNSUPPORTED: &str =
        "delivery.check.text.line_ending_unsupported";

    // Delimited tables (F02).
    pub(crate) const TABLE_ENCODING_UNSUPPORTED: &str =
        "delivery.check.table.encoding_unsupported";
    pub(crate) const TABLE_UNTERMINATED_QUOTE: &str = "delivery.check.table.unterminated_quote";
    pub(crate) const TABLE_CHARACTER_AFTER_QUOTE: &str =
        "delivery.check.table.character_after_quote";
    pub(crate) const TABLE_UNESCAPED_QUOTE: &str = "delivery.check.table.unescaped_quote";
    pub(crate) const TABLE_HEADER_EMPTY: &str = "delivery.check.table.header_empty";
    pub(crate) const TABLE_HEADER_DUPLICATE: &str = "delivery.check.table.header_duplicate";
    /// A real record whose field count disagrees with the header.
    pub(crate) const TABLE_WIDTH_MISMATCH: &str = "delivery.check.table.width_mismatch";
    pub(crate) const TABLE_ZERO_RECORDS: &str = "delivery.check.table.zero_records";
    pub(crate) const TABLE_DIALECT_UNSUPPORTED: &str = "delivery.check.table.dialect_unsupported";

    // Shared.
    /// A ceiling in [`super::CheckLimits`] was reached. Over-limit is reported,
    /// never retried through the model.
    pub(crate) const BUDGET_EXCEEDED: &str = "delivery.check.budget_exceeded";
    /// The frozen contract (or its envelope) cannot be read at all.
    pub(crate) const CONTRACT_UNREADABLE: &str = "delivery.check.contract_unreadable";
    pub(crate) const CONTRACT_VERSION_UNSUPPORTED: &str =
        "delivery.check.contract_version_unsupported";
    /// The evidence this check needs is not available to the Host.
    pub(crate) const EVIDENCE_UNAVAILABLE: &str = "delivery.check.evidence_unavailable";
    /// The candidate bytes no longer match the receipt/version hash.
    pub(crate) const CANDIDATE_HASH_DRIFT: &str = "delivery.check.candidate_hash_drift";
    /// C0 placeholder only: the leaf module has no implementation yet. Must
    /// disappear from every code path once its wave lands.
    pub(crate) const CHECKER_NOT_IMPLEMENTED: &str = "delivery.check.checker_not_implemented";

    /// Security vetoes. **Separate and never downgradable**: no API in this
    /// layer maps one of these into [`super::CheckState`], and a shadow pass or
    /// "the data turned out correct" can never cover one (plan §9.3).
    pub(crate) mod security {
        pub(crate) const PATH_OUTSIDE_AUTHORIZED_ROOT: &str =
            "delivery.security.path_outside_authorized_root";
        pub(crate) const READONLY_INPUT_WRITE: &str = "delivery.security.readonly_input_write";
        pub(crate) const SOURCE_IDENTITY_MISMATCH: &str =
            "delivery.security.source_identity_mismatch";
        pub(crate) const VERSION_CREDENTIAL_INVALID: &str =
            "delivery.security.version_credential_invalid";
        pub(crate) const LOCK_RACE_VERSION_CHANGED: &str =
            "delivery.security.lock_race_version_changed";
        pub(crate) const APPROVAL_MISSING: &str = "delivery.security.approval_missing";
    }
}

/// Check names. Stable, `snake_case`, persisted with every observation.
pub(crate) mod check_name {
    pub(crate) const TEXT_CONCAT: &str = "text_concat";
    pub(crate) const TABLE_COLUMNS: &str = "table_columns";
    pub(crate) const TYPED_METRIC: &str = "typed_metric";
    pub(crate) const LINEAGE: &str = "lineage";
    pub(crate) const PDF_STRUCTURE: &str = "pdf_structure";
}

/// The equivalences a frozen policy may rely on. Stable literals: they are
/// persisted on the observation and shown to the user, so a pass that needed one
/// is auditable rather than implied by free text.
pub(crate) mod equivalence {
    /// Zero or one terminating LF are equivalent
    /// ([`EofPolicy::AllowSingleTrailingLf`]). The only tolerated width in the
    /// whole text comparison; never a licence to trim the body (plan §4.1).
    pub(crate) const OPTIONAL_FINAL_LF: &str = "optionalFinalLf";
}

// ---------------------------------------------------------------------------
// The three states (single authority)
// ---------------------------------------------------------------------------

/// The one verdict vocabulary, identical to the existing
/// `passed` / `failed` / `unverified`.
///
/// Only [`CheckState::Failed`] may change a passing item to failing. This
/// mirrors the behaviour that exists today ("only `failed` changes the pass
/// boolean") and is asserted by the freeze tests in this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CheckState {
    Passed,
    Failed,
    Unverified,
}

impl CheckState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CheckState::Passed => "passed",
            CheckState::Failed => "failed",
            CheckState::Unverified => "unverified",
        }
    }

    /// Bridge **into** the existing verdict type. The reverse of
    /// [`CheckState::from`]; together they are the only translation between the
    /// two names for the same three states.
    pub(crate) fn to_verdict(self, reason: Option<String>) -> RequirementVerdict {
        match self {
            CheckState::Passed => RequirementVerdict::Met,
            CheckState::Failed => RequirementVerdict::Unmet(
                reason.unwrap_or_else(|| "真实交付内容与冻结合同不一致".to_owned()),
            ),
            CheckState::Unverified => RequirementVerdict::Unverified(
                reason.unwrap_or_else(|| "该交付要求尚未核验".to_owned()),
            ),
        }
    }
}

impl From<&RequirementVerdict> for CheckState {
    /// Bridge **out of** the existing verdict type.
    fn from(verdict: &RequirementVerdict) -> Self {
        match verdict {
            RequirementVerdict::Met => CheckState::Passed,
            RequirementVerdict::Unmet(_) => CheckState::Failed,
            RequirementVerdict::Unverified(_) => CheckState::Unverified,
        }
    }
}

// ---------------------------------------------------------------------------
// Resource ceilings and over-limit semantics
// ---------------------------------------------------------------------------

/// The ceilings a check runs under. They are frozen with the checker
/// generation: **a rule may not raise them**, and no leaf module defines its
/// own copy. Values are aligned with the ceilings the codebase already applies
/// so a v2 check cannot enlarge a v1 budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckLimits {
    /// Read ceiling for one candidate. Mirrors `delivery.rs::MAX_VERIFY_BYTES`.
    pub max_bytes: u64,
    /// Total decoded text. Mirrors `data_compute.rs::MAX_TOTAL_TEXT_BYTES`.
    pub max_text_bytes: usize,
    /// Records in one table. Mirrors `MAX_SHEET_ROWS`.
    pub max_records: usize,
    /// Fields in one record. Mirrors `MAX_SHEET_COLUMNS`.
    pub max_fields: usize,
    /// Cells in one table. Mirrors `MAX_TOTAL_CELLS`.
    pub max_cells: usize,
    /// Bytes in a single physical line/record.
    pub max_line_bytes: usize,
    /// Bytes examined by a structural scan (PDF and similar).
    pub max_scan_bytes: u64,
    /// Indirect objects in one structured document.
    pub max_objects: usize,
    /// Nesting depth of a structured document.
    pub max_depth: usize,
    /// Nodes in a lineage graph.
    pub max_nodes: usize,
    /// Pages in one document.
    pub max_pages: usize,
    /// Wall-clock budget for one check. Mirrors `MAX_TIMEOUT_MS`.
    pub max_millis: u64,
}

impl CheckLimits {
    /// The ceilings P0 checks run under.
    pub(crate) const P0: Self = Self {
        max_bytes: 64 * 1024 * 1024,
        max_text_bytes: 32 * 1024 * 1024,
        max_records: 250_000,
        max_fields: 512,
        max_cells: 1_000_000,
        max_line_bytes: 8 * 1024 * 1024,
        max_scan_bytes: 64 * 1024 * 1024,
        max_objects: 200_000,
        max_depth: 64,
        max_nodes: 8_192,
        max_pages: 10_000,
        max_millis: 30_000,
    };

    /// The ceiling [`LimitKind`] names, for an explicit over-limit report.
    pub(crate) fn ceiling(self, limit: LimitKind) -> u64 {
        match limit {
            LimitKind::Bytes => self.max_bytes,
            LimitKind::TextBytes => self.max_text_bytes as u64,
            LimitKind::Records => self.max_records as u64,
            LimitKind::Fields => self.max_fields as u64,
            LimitKind::Cells => self.max_cells as u64,
            LimitKind::LineBytes => self.max_line_bytes as u64,
            LimitKind::ScanBytes => self.max_scan_bytes,
            LimitKind::Objects => self.max_objects as u64,
            LimitKind::Depth => self.max_depth as u64,
            LimitKind::Nodes => self.max_nodes as u64,
            LimitKind::Pages => self.max_pages as u64,
            LimitKind::Millis => self.max_millis,
        }
    }
}

impl Default for CheckLimits {
    fn default() -> Self {
        Self::P0
    }
}

/// Which ceiling was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LimitKind {
    Bytes,
    TextBytes,
    Records,
    Fields,
    Cells,
    LineBytes,
    ScanBytes,
    Objects,
    Depth,
    Nodes,
    Pages,
    Millis,
}

impl LimitKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LimitKind::Bytes => "bytes",
            LimitKind::TextBytes => "textBytes",
            LimitKind::Records => "records",
            LimitKind::Fields => "fields",
            LimitKind::Cells => "cells",
            LimitKind::LineBytes => "lineBytes",
            LimitKind::ScanBytes => "scanBytes",
            LimitKind::Objects => "objects",
            LimitKind::Depth => "depth",
            LimitKind::Nodes => "nodes",
            LimitKind::Pages => "pages",
            LimitKind::Millis => "millis",
        }
    }
}

// ---------------------------------------------------------------------------
// Rule envelope and policy
// ---------------------------------------------------------------------------

/// Comparision mode of a text artifact. `BytesExact` is the default because it
/// is exactly what the frozen v1 `Concat` contract does today: a v1 record must
/// keep byte-exact semantics. `TextSegments` is opt-in per rule (R01–R05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ComparisonMode {
    #[default]
    BytesExact,
    TextSegments,
}

/// Final-newline policy, always declared by the frozen rule — never left
/// implicit (plan §4.1). Only one *provable* equivalence is allowed: the
/// presence or absence of exactly one terminating LF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum EofPolicy {
    /// Byte-for-byte, including the presence and count of trailing newlines.
    /// This is the v1 behaviour.
    #[default]
    ByteExact,
    /// The last source's own final newline decides; nothing is added or removed.
    SourceVerbatim,
    /// Zero or one trailing LF are equivalent. The only tolerated width in the
    /// whole comparison; never a licence to trim the body.
    AllowSingleTrailingLf,
    RequireTrailingLf,
}

/// Cell delimiter of a delimited table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Delimiter {
    Comma,
    Tab,
    Semicolon,
    Pipe,
}

impl Delimiter {
    pub(crate) fn as_char(self) -> char {
        match self {
            Delimiter::Comma => ',',
            Delimiter::Tab => '\t',
            Delimiter::Semicolon => ';',
            Delimiter::Pipe => '|',
        }
    }
}

/// Quoting rule. `None` means quotes are ordinary data, which is a real
/// declaration and not a missing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum QuotePolicy {
    DoubleQuote,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BomPolicy {
    /// Strip a leading UTF-8 BOM when present, and tolerate its absence.
    Auto,
    Required,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LineEnding {
    /// LF, CRLF and a final unterminated line are all accepted.
    Any,
    Lf,
    Crlf,
}

/// Whether a physically empty line is a record.
///
/// Default `Skip` is the target unified dialect (R07/R09: a legal quoted field
/// may contain newlines, and a blank line must not be counted as a wrong-width
/// record). It differs from what `delivery_content_requirements.rs::delimited`
/// does today, which is a *documented* "old reject / new pass" case for G's
/// shadow comparison — see the C0 handoff, decision D-C0-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BlankLinePolicy {
    Skip,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum HeaderPolicy {
    Required,
    Optional,
}

/// The unified table dialect (P0-2: basic check, content check and row counting
/// must share one definition of "record" and "field").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TableDialect {
    #[serde(default = "default_delimiter")]
    pub delimiter: Delimiter,
    #[serde(default = "default_quote")]
    pub quote: QuotePolicy,
    #[serde(default = "default_bom")]
    pub bom: BomPolicy,
    #[serde(default = "default_line_ending")]
    pub line_ending: LineEnding,
    #[serde(default = "default_blank_line")]
    pub blank_line: BlankLinePolicy,
    #[serde(default = "default_header")]
    pub header: HeaderPolicy,
}

fn default_delimiter() -> Delimiter {
    Delimiter::Comma
}
fn default_quote() -> QuotePolicy {
    QuotePolicy::DoubleQuote
}
fn default_bom() -> BomPolicy {
    BomPolicy::Auto
}
fn default_line_ending() -> LineEnding {
    LineEnding::Any
}
fn default_blank_line() -> BlankLinePolicy {
    BlankLinePolicy::Skip
}
fn default_header() -> HeaderPolicy {
    HeaderPolicy::Required
}

impl Default for TableDialect {
    fn default() -> Self {
        Self {
            delimiter: default_delimiter(),
            quote: default_quote(),
            bom: default_bom(),
            line_ending: default_line_ending(),
            blank_line: default_blank_line(),
            header: default_header(),
        }
    }
}

/// Everything a rule declares beyond its `rule` payload.
///
/// The default is deliberately the *strictest* reading of a v1 artifact
/// (byte-exact compare, byte-exact final newline), so a rule that omits a field
/// can never become more permissive by accident.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RulePolicy {
    #[serde(default)]
    pub comparison: ComparisonMode,
    #[serde(default)]
    pub eof: EofPolicy,
    #[serde(default)]
    pub table: TableDialect,
}

/// The versioned envelope that travels beside the frozen `rule` payload.
///
/// Persisted shape (inside the existing `ContentContract { spec }` value in
/// `delivery_checklist_items.requirements_json`):
///
/// ```json
/// {"schemaVersion":1,"rule":{...},
///  "envelopeVersion":2,"ruleId":"content:concat","checkerVersion":2,
///  "policy":{"comparison":"textSegments","eof":"allowSingleTrailingLf", …}}
/// ```
///
/// No migration and no schema change are needed: v1 readers ignore the extra
/// keys, and v2 readers read v1 records as v1.
///
/// `merge_rule` is the separator semantics of a `Concat` rule, and is a
/// different axis from `envelope_version` / `schemaVersion` (see
/// [`merge_rule`]). It is skipped when it is the historical rule so that a
/// record written by an older build and one written today with that same rule
/// serialize identically — the marker only ever adds information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuleEnvelope {
    #[serde(default = "envelope_v1")]
    pub envelope_version: u32,
    #[serde(default = "checker_v1")]
    pub checker_version: u32,
    /// The frozen requirement id this rule belongs to, when the writer had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub policy: RulePolicy,
    /// The `Concat` separator this record was frozen with.
    #[serde(
        default,
        rename = "mergeRule",
        skip_serializing_if = "is_legacy_merge_rule"
    )]
    pub merge_rule: MergeRule,
}

fn envelope_v1() -> u32 {
    ENVELOPE_VERSION_V1
}
fn checker_v1() -> u32 {
    CHECKER_VERSION_V1
}

/// `skip_serializing_if` takes a reference, and keeping the predicate beside the
/// field is what stops the serialized shape from drifting away from `is_legacy`.
fn is_legacy_merge_rule(rule: &MergeRule) -> bool {
    rule.is_legacy()
}

impl Default for RuleEnvelope {
    fn default() -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION_V1,
            checker_version: CHECKER_VERSION_V1,
            rule_id: None,
            policy: RulePolicy::default(),
            merge_rule: merge_rule::MergeRule::default(),
        }
    }
}

impl RuleEnvelope {
    /// A v1 envelope: what every record written before C0 means.
    pub(crate) fn v1() -> Self {
        Self::default()
    }

    /// A v2 envelope with an explicit policy, frozen by the Host for a new Run.
    ///
    /// A new envelope describes a contract frozen **now**, so it carries the
    /// separator this build writes. A reader never makes that choice on its own:
    /// this is the writer declaring the rule, and an envelope for an existing
    /// record is produced by [`Self::from_spec`], which reads what is there.
    pub(crate) fn v2(rule_id: impl Into<String>, policy: RulePolicy) -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION_V2,
            checker_version: CHECKER_VERSION_V2,
            rule_id: Some(rule_id.into()),
            policy,
            merge_rule: merge_rule::MergeRule::current(),
        }
    }

    /// The same envelope frozen with an explicit separator rule.
    pub(crate) fn with_merge_rule(mut self, rule: merge_rule::MergeRule) -> Self {
        self.merge_rule = rule;
        self
    }

    /// True when this contract predates the envelope and must be read with v1
    /// semantics. A running Run is never auto-upgraded.
    pub(crate) fn is_v1(&self) -> bool {
        self.envelope_version < ENVELOPE_VERSION_V2
    }

    /// Read an envelope out of a persisted `spec` value.
    ///
    /// * A missing `envelopeVersion` / `checkerVersion` / `policy` is a **v1
    ///   record** and reads as v1 — never an error, never an upgrade.
    /// * A v1 record that nevertheless carries a `policy` is refused rather than
    ///   silently honoured, because honouring it would change the meaning of an
    ///   already-frozen contract.
    /// * An unreadable or unsupported version is an `Err`: the caller reports the
    ///   rule as `unverified` (the behaviour `delivery_content_requirements.rs`
    ///   already has for an unsupported `schemaVersion`). It never defaults to a
    ///   looser policy.
    /// * A `mergeRule` this build does not understand is an `Err` for the same
    ///   reason: reading it as the legacy rule would verify a record against
    ///   semantics its author did not declare.
    pub(crate) fn from_spec(spec: &Value) -> Result<Self, String> {
        let object = spec
            .as_object()
            .ok_or_else(|| "内容契约规格不是对象，无法按 v1 解释".to_owned())?;
        let envelope_version = match object.get("envelopeVersion") {
            None | Some(Value::Null) => ENVELOPE_VERSION_V1,
            Some(value) => value
                .as_u64()
                .filter(|version| *version >= 1 && *version <= ENVELOPE_VERSION_V2 as u64)
                .ok_or_else(|| "内容契约信封版本不支持".to_owned())?
                as u32,
        };
        let checker_version = match object.get("checkerVersion") {
            None | Some(Value::Null) => CHECKER_VERSION_V1,
            Some(value) => value
                .as_u64()
                .filter(|version| *version >= 1 && *version <= CHECKER_VERSION_V2 as u64)
                .ok_or_else(|| "内容契约 checker 版本不支持".to_owned())?
                as u32,
        };
        let rule_id = match object.get("ruleId") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                    .ok_or_else(|| "内容契约规则标识不可读".to_owned())?
                    .to_owned(),
            ),
        };
        let policy = match object.get("policy") {
            None | Some(Value::Null) => RulePolicy::default(),
            Some(_) if envelope_version < ENVELOPE_VERSION_V2 => {
                return Err("v1 内容契约不允许携带 v2 政策；不能改变已冻结合同的含义".to_owned());
            }
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| "内容契约政策字段不可读".to_owned())?,
        };
        let merge_rule = merge_rule::from_spec(spec)?;
        Ok(Self {
            envelope_version,
            checker_version,
            rule_id,
            policy,
            merge_rule,
        })
    }

    /// The keys to merge beside `schemaVersion` and `rule` when freezing a rule.
    pub(crate) fn to_spec_keys(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Object(Default::default()))
    }
}

// ---------------------------------------------------------------------------
// Location
// ---------------------------------------------------------------------------

/// Where a finding is, in both the logical and the physical space.
///
/// `record` counts CSV records (header = record 1); `line` counts physical lines
/// of the file, so a quoted field that spans lines cannot make the two agree by
/// accident. `field` is 1-based; `column` is the header name when one is known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordLocator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
}

impl RecordLocator {
    pub(crate) fn record(record: u64) -> Self {
        Self {
            record: Some(record),
            ..Self::default()
        }
    }

    pub(crate) fn cell(record: u64, field: u64, column: Option<&str>) -> Self {
        Self {
            record: Some(record),
            field: Some(field),
            column: column.map(str::to_owned),
            ..Self::default()
        }
    }

    pub(crate) fn at_line(mut self, line: u64) -> Self {
        self.line = Some(line);
        self
    }

    pub(crate) fn at_byte(mut self, offset: u64) -> Self {
        self.byte_offset = Some(offset);
        self
    }

    /// A bounded, user-facing rendering; never the whole file.
    pub(crate) fn display(&self) -> String {
        let mut parts = Vec::new();
        if let Some(record) = self.record {
            parts.push(format!("第 {record} 条记录"));
        }
        if let Some(line) = self.line {
            parts.push(format!("第 {line} 行"));
        }
        if let Some(column) = &self.column {
            parts.push(format!("字段「{column}」"));
        } else if let Some(field) = self.field {
            parts.push(format!("第 {field} 个字段"));
        }
        if let Some(offset) = self.byte_offset {
            parts.push(format!("字节 {offset}"));
        }
        parts.join("，")
    }
}

// ---------------------------------------------------------------------------
// Input type
// ---------------------------------------------------------------------------

/// Text encodings this round supports. Anything else is `unknown` and therefore
/// `unverified`: D1 forbids adding an encoding-detection dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TextEncoding {
    Utf8,
}

/// What kind of input a check read, with the declaration it was read under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum InputKind {
    Text {
        encoding: TextEncoding,
    },
    Delimited {
        dialect: TableDialect,
    },
    Workbook {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    Pdf {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pages: Option<u64>,
    },
    Json,
    Docx,
    Unknown {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Candidate binding
// ---------------------------------------------------------------------------

/// The exact candidate bytes a check read, bound to the identity the Host
/// already recorded. A check may only report on the bytes it was handed with
/// this binding; evidence from an earlier round or another Run is never bound by
/// accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidateBinding {
    /// Project-relative path, in the same space as `delivery::path_key`.
    pub path: String,
    /// Hex SHA-256 of the bytes the check actually read.
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_no: Option<i64>,
    /// The write receipt hash recorded when the candidate was committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_run_id: Option<String>,
}

impl CandidateBinding {
    pub(crate) fn new(path: impl Into<String>, sha256: impl Into<String>, bytes: u64) -> Self {
        Self {
            path: path.into(),
            sha256: sha256.into(),
            bytes,
            artifact_id: None,
            version_id: None,
            version_no: None,
            checked_hash: None,
            source_run_id: None,
        }
    }

    /// Bind to the durable fields an existing `ItemVerdict` finding already
    /// carries. `path` / `sha256` / `bytes` are the bytes the caller read; the
    /// rest is copied from the finding so a check cannot invent an identity.
    pub(crate) fn from_finding(finding: &Value, path: &str, sha256: &str, bytes: u64) -> Self {
        let text = |key: &str| finding[key].as_str().map(str::to_owned);
        Self {
            path: path.to_owned(),
            sha256: sha256.to_owned(),
            bytes,
            artifact_id: text("artifactId"),
            version_id: text("managedVersionId"),
            version_no: finding["managedVersionNo"].as_i64(),
            checked_hash: text("checkedHash"),
            source_run_id: text("sourceRunId"),
        }
    }

    /// The prefix rule the existing check applies: `checkedHash` may carry a
    /// `sha256:` prefix. One helper, so no leaf module re-derives it.
    pub(crate) fn matches_checked_hash(&self, hash: &str) -> bool {
        hash.trim_start_matches("sha256:") == self.sha256
    }
}

// ---------------------------------------------------------------------------
// Observations and outcome
// ---------------------------------------------------------------------------

/// One rule's result against one candidate. This is the `CheckObservationV2`
/// shape of the plan, kept deliberately close to the existing `checks` entry so
/// it extends the current finding instead of replacing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckObservation {
    pub rule_id: String,
    pub checker_version: u32,
    pub check: String,
    pub state: CheckState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<RecordLocator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Free text, bounded by the caller. Never the whole artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitKind>,
    /// True **only** for `failed`, exactly like today's behaviour that only
    /// `failed` changes the pass boolean. `unverified` never blocks.
    #[serde(default)]
    pub blocking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<CandidateBinding>,
    /// Set **only** on a `passed` observation, and only when the pass relied on
    /// an equivalence the frozen policy explicitly allows (see [`equivalence`]).
    /// A plain byte match leaves this `None`, so "R01: passed *and* the
    /// equivalence was recorded" is observable instead of assumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equivalence: Option<String>,
}

/// A real, checkable mismatch. `blocking` is set because a failed item is what
/// the existing repair loop reacts to.
pub(crate) fn failed(
    rule_id: &str,
    check: &str,
    code: &str,
    binding: &CandidateBinding,
    detail: impl Into<String>,
) -> CheckObservation {
    CheckObservation {
        rule_id: rule_id.to_owned(),
        checker_version: CHECKER_VERSION_V2,
        check: check.to_owned(),
        state: CheckState::Failed,
        reason_code: Some(code.to_owned()),
        locator: None,
        expected: None,
        actual: None,
        detail: Some(detail.into()),
        limit: None,
        blocking: true,
        candidate: Some(binding.clone()),
        equivalence: None,
    }
}

/// Nothing to report. A pass carries no reason code.
pub(crate) fn passed(rule_id: &str, check: &str, binding: &CandidateBinding) -> CheckObservation {
    CheckObservation {
        rule_id: rule_id.to_owned(),
        checker_version: CHECKER_VERSION_V2,
        check: check.to_owned(),
        state: CheckState::Passed,
        reason_code: None,
        locator: None,
        expected: None,
        actual: None,
        detail: None,
        limit: None,
        blocking: false,
        candidate: Some(binding.clone()),
        equivalence: None,
    }
}

/// A pass that **relied on** an explicitly allowed equivalence. The equivalence
/// literal is frozen ([`equivalence`]) and is persisted with the observation, so
/// a reviewer can tell "byte-identical" from "allowed to differ by one final LF".
pub(crate) fn passed_with_equivalence(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
    equivalence: &str,
    detail: impl Into<String>,
) -> CheckObservation {
    let mut observation = passed(rule_id, check, binding);
    observation.equivalence = Some(equivalence.to_owned());
    observation.detail = Some(detail.into());
    observation
}

/// A demand this round cannot decide. Never a pass, never blocking.
pub(crate) fn unverified(
    rule_id: &str,
    check: &str,
    code: &str,
    binding: &CandidateBinding,
    detail: impl Into<String>,
) -> CheckObservation {
    CheckObservation {
        rule_id: rule_id.to_owned(),
        checker_version: CHECKER_VERSION_V2,
        check: check.to_owned(),
        state: CheckState::Unverified,
        reason_code: Some(code.to_owned()),
        locator: None,
        expected: None,
        actual: None,
        detail: Some(detail.into()),
        limit: None,
        blocking: false,
        candidate: Some(binding.clone()),
        equivalence: None,
    }
}

/// The single over-limit verdict. Exceeding a ceiling is reported explicitly
/// and is terminal for that check: it must never trigger a model repair loop
/// (plan §7 P0-2: "解析器超预算返回明确限制，不循环调用模型").
pub(crate) fn budget_exceeded(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
    limit: LimitKind,
    ceiling: u64,
    observed: u64,
) -> CheckObservation {
    let mut observation = unverified(
        rule_id,
        check,
        reason_code::BUDGET_EXCEEDED,
        binding,
        format!(
            "该项检查达到资源上限 {}（上限 {ceiling}，实际 {observed}）；本轮不再重试，也不据此判定内容错误",
            limit.as_str()
        ),
    );
    observation.limit = Some(limit);
    observation.expected = Some(format!("{} <= {ceiling}", limit.as_str()));
    observation.actual = Some(observed.to_string());
    observation
}

/// C0 placeholder verdict for a leaf module that has no implementation yet.
/// Honest by construction: an unimplemented checker can never report a pass.
pub(crate) fn not_implemented(
    rule_id: &str,
    check: &str,
    binding: &CandidateBinding,
) -> CheckObservation {
    unverified(
        rule_id,
        check,
        reason_code::CHECKER_NOT_IMPLEMENTED,
        binding,
        "该检查项尚未实现，本轮按未核验处理".to_owned(),
    )
}

/// A security refusal. **Separate and never downgradable** — there is
/// deliberately no conversion from this type into [`CheckState`], so no caller
/// can render a veto as "unverified" or "passed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecurityVeto {
    pub reason_code: String,
    pub check: String,
    pub detail: String,
}

impl SecurityVeto {
    pub(crate) fn new(code: &str, check: &str, detail: impl Into<String>) -> Self {
        Self {
            reason_code: code.to_owned(),
            check: check.to_owned(),
            detail: detail.into(),
        }
    }
}

/// Everything one check produced for one candidate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckOutcome {
    #[serde(default)]
    pub observations: Vec<CheckObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_veto: Option<SecurityVeto>,
}

impl CheckOutcome {
    pub(crate) fn single(observation: CheckObservation) -> Self {
        Self {
            observations: vec![observation],
            security_veto: None,
        }
    }

    pub(crate) fn veto(&self) -> Option<&SecurityVeto> {
        self.security_veto.as_ref()
    }

    /// A veto and a failed observation both stop the item. They are kept
    /// distinguishable so a security refusal is never reported as a content
    /// error, and a content error never hides a veto.
    pub(crate) fn has_failure(&self) -> bool {
        self.security_veto.is_some()
            || self
                .observations
                .iter()
                .any(|observation| observation.state == CheckState::Failed)
    }

    /// Only a real, checkable mismatch may arm a repair round. A security veto
    /// does not: it is a refusal, not something to rewrite.
    pub(crate) fn needs_repair(&self) -> bool {
        self.security_veto.is_none()
            && self
                .observations
                .iter()
                .any(|observation| observation.state == CheckState::Failed && observation.blocking)
    }

    pub(crate) fn has_unverified(&self) -> bool {
        self.observations
            .iter()
            .any(|observation| observation.state == CheckState::Unverified)
    }

    /// "Check finished and everything it could decide agreed". Deliberately not
    /// called `passed`: a coverage statement here must never be read as "the
    /// content is correct".
    pub(crate) fn is_fully_verified(&self) -> bool {
        self.security_veto.is_none()
            && !self.observations.is_empty()
            && self
                .observations
                .iter()
                .all(|observation| observation.state == CheckState::Passed)
    }
}

/// Fold one observation back into the existing verdict vocabulary.
///
/// A [`SecurityVeto`] is **not** accepted here: the caller must handle
/// `CheckOutcome::veto()` first. This is the only place a check result becomes a
/// [`RequirementVerdict`], so no second state machine can appear.
pub(crate) fn verdict_from_observation(observation: &CheckObservation) -> RequirementVerdict {
    let reason = observation
        .detail
        .clone()
        .or_else(|| observation.reason_code.clone());
    observation.state.to_verdict(reason)
}

// ---------------------------------------------------------------------------
// Shared inputs for the leaf modules
// ---------------------------------------------------------------------------

/// One ordered segment of a text-merge contract. `bytes` are the frozen source
/// bytes; `path` is the project-relative source path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextSegment<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

impl<'a> TextSegment<'a> {
    /// The header name used by the frozen merge format. Frozen here so the v1
    /// rule (`rsplit('/')`) cannot drift between the v1 and v2 writers.
    pub(crate) fn file_name(&self) -> &'a str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }
}

/// Input of the text/EOF check (`text_csv.rs`, owner B).
pub(crate) struct TextConcatRequest<'a> {
    pub rule_id: &'a str,
    /// The frozen envelope. `is_v1()` selects byte-exact v1 semantics.
    pub envelope: &'a RuleEnvelope,
    /// Segments in the frozen selection order (headers are derived from these).
    pub segments: &'a [TextSegment<'a>],
    /// The candidate bytes as they exist now.
    pub actual: &'a [u8],
    pub binding: &'a CandidateBinding,
}

/// One parsed field, with its original value and position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableField {
    pub field: u64,
    pub value: String,
    /// True when the field was written quoted in the source. Kept so a later
    /// type check can tell `136.69` from a quoted `"136.69"` where the rule says
    /// it matters, without re-parsing.
    pub quoted: bool,
}

/// One parsed record. `record` is the logical record number (header = 1) and
/// `line` is the physical line it started on, so a quoted newline cannot make
/// the two agree by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableRecord {
    pub record: u64,
    pub line: u64,
    pub fields: Vec<TableField>,
    /// True when the record came from a blank physical line.
    pub blank: bool,
}

/// A bounded parse result. The parser returns this or a [`TableError`]; it never
/// repairs, never converts types and never drops a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTable {
    pub records: Vec<TableRecord>,
    pub dialect: TableDialect,
    pub bom_stripped: bool,
}

impl ParsedTable {
    pub(crate) fn header(&self) -> Option<&TableRecord> {
        self.records.first()
    }

    /// Data records, i.e. everything after the header when the dialect declares
    /// one.
    pub(crate) fn data_records(&self) -> &[TableRecord] {
        if self.dialect.header == HeaderPolicy::Required && !self.records.is_empty() {
            &self.records[1..]
        } else {
            &self.records
        }
    }
}

/// A parse failure with a stable code and a position. `unterminated_quote` and
/// `width_mismatch` are different findings: a parser bug must never be reported
/// as wrong data.
/// Why a parse failed, independent of the human-readable [`TableError::code`].
///
/// The category is chosen **at the construction site**, where the parser
/// knows whether it hit a resource ceiling, a capability it does not
/// implement, or a definite violation of a declared/syntactic rule. It is
/// never inferred from the code string or the detail message.
///
/// * [`ParseFailureKind::Resource`] — a declared ceiling (bytes/scan/millis/
///   records/fields/cells/line/text) stopped the parse. Always carries a real
///   [`LimitKind`]; maps to an *unverified* budget observation.
/// * [`ParseFailureKind::Unsupported`]] — the checker can read the bytes but
///   cannot interpret them under the frozen contract (today: bytes that are
///   not valid UTF-8, or a lone CR the dialect cannot explain). There is no
///   frozen encoding declaration, so this is evidence insufficiency, not a
///   violation; it maps to *unverified*.
/// * [`ParseFailureKind::Violation`] — a definite, checkable violation under
///   a supported contract: declared BOM required/forbidden, declared LF/CRLF
///   line endings, or the quoted-field syntax. Maps to *failed*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseFailureKind {
    Resource,
    Unsupported,
    Violation,
}

/// A parse failure with a stable code, a position and an explicit,
/// construction-site [`ParseFailureKind`].
///
/// Fields are crate-private and **not** directly writable: every production
/// error goes through [`TableError::resource`] / [`TableError::unsupported`]
/// / [`TableError::violation`] so that the `kind`/`limit` invariant cannot
/// drift (a Resource always carries its limit; Unsupported/Violation never
/// do). Child modules of this crate can technically reach a private field,
/// so the invariant is additionally pinned by the
/// `table_error_kind_limit_invariant` unit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableError {
    code: String,
    locator: RecordLocator,
    detail: String,
    /// Set only for an over-limit stop, so the caller can render the same
    /// explicit over-limit report a direct observation uses. A budget stop is
    /// never a content error (plan §7 P0-2).
    limit: Option<LimitKind>,
    kind: ParseFailureKind,
}

impl TableError {
    /// A parse that stopped at a declared ceiling. The only constructor that
    /// sets `kind = Resource`; it always carries the real [`LimitKind`].
    pub(crate) fn resource(
        limit: LimitKind,
        ceiling: u64,
        observed: u64,
        locator: RecordLocator,
    ) -> Self {
        Self {
            code: reason_code::BUDGET_EXCEEDED.to_owned(),
            locator,
            detail: format!(
                "解析达到资源上限 {}（上限 {ceiling}，实际 {observed}）；本轮不重试，也不据此判定内容错误",
                limit.as_str()
            ),
            limit: Some(limit),
            kind: ParseFailureKind::Resource,
        }
    }

    /// The checker could not interpret the bytes under the frozen contract
    /// (no declared encoding policy is violated; the capability is simply
    /// insufficient). Carries no limit and maps to unverified.
    pub(crate) fn unsupported(
        code: &str,
        locator: RecordLocator,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            locator,
            detail: detail.into(),
            limit: None,
            kind: ParseFailureKind::Unsupported,
        }
    }

    /// A definite violation of a declared/syntactic rule under a supported
    /// contract. Carries no limit and maps to a blocking failed observation.
    pub(crate) fn violation(
        code: &str,
        locator: RecordLocator,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            locator,
            detail: detail.into(),
            limit: None,
            kind: ParseFailureKind::Violation,
        }
    }

    /// Back-compat constructor used by the budget path; identical to
    /// [`Self::resource`].
    pub(crate) fn budget(
        limit: LimitKind,
        ceiling: u64,
        observed: u64,
        locator: RecordLocator,
    ) -> Self {
        Self::resource(limit, ceiling, observed, locator)
    }

    pub(crate) fn code(&self) -> &str {
        &self.code
    }
    pub(crate) fn locator(&self) -> &RecordLocator {
        &self.locator
    }
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
    pub(crate) fn limit(&self) -> Option<LimitKind> {
        self.limit
    }
    pub(crate) fn kind(&self) -> ParseFailureKind {
        self.kind
    }
}

/// Input of the table-column check (`text_csv.rs`, owner B).
pub(crate) struct TableCheckRequest<'a> {
    pub rule_id: &'a str,
    pub parsed: &'a ParsedTable,
    pub binding: &'a CandidateBinding,
    /// Only when the frozen rule *declares* unique column names. The Host must
    /// not add an undeclared uniqueness demand (R08).
    pub require_unique_headers: bool,
    /// Only when the frozen rule *declares* that zero data records are legal
    /// (R09). The default is that a header plus no data is not enough.
    pub allow_zero_records: bool,
}

// ---------------------------------------------------------------------------
// Freeze tests (owned by A — leaf owners must not weaken these)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> CandidateBinding {
        CandidateBinding::new("out/report.csv", "a".repeat(64), 12)
    }

    #[test]
    fn v1_spec_without_new_fields_reads_as_v1_and_never_upgrades() {
        let spec = json!({"schemaVersion":1,"rule":{"type":"concat"}});
        let envelope = RuleEnvelope::from_spec(&spec).expect("v1 spec must be readable");
        assert_eq!(envelope.envelope_version, ENVELOPE_VERSION_V1);
        assert_eq!(envelope.checker_version, CHECKER_VERSION_V1);
        assert!(envelope.is_v1());
        assert_eq!(envelope.rule_id, None);
        // The default must be the strict v1 reading, so an omitted field can
        // never become more permissive.
        assert_eq!(envelope.policy.comparison, ComparisonMode::BytesExact);
        assert_eq!(envelope.policy.eof, EofPolicy::ByteExact);
        assert_eq!(envelope.policy, RulePolicy::default());
    }

    #[test]
    fn v1_record_carrying_a_policy_is_refused_rather_than_reinterpreted() {
        let spec = json!({"schemaVersion":1,"rule":{"type":"concat"},
            "policy":{"comparison":"textSegments","eof":"allowSingleTrailingLf"}});
        assert!(RuleEnvelope::from_spec(&spec).is_err());
    }

    #[test]
    fn unreadable_or_unsupported_envelope_never_defaults_to_a_looser_policy() {
        assert!(RuleEnvelope::from_spec(&json!("not-an-object")).is_err());
        assert!(RuleEnvelope::from_spec(&json!({"envelopeVersion":3})).is_err());
        assert!(RuleEnvelope::from_spec(&json!({"envelopeVersion":"two"})).is_err());
        assert!(RuleEnvelope::from_spec(&json!({"envelopeVersion":2,"policy":"nope"})).is_err());
    }

    #[test]
    fn v2_envelope_round_trips_beside_the_untouched_rule_payload() {
        let policy = RulePolicy {
            comparison: ComparisonMode::TextSegments,
            eof: EofPolicy::AllowSingleTrailingLf,
            table: TableDialect {
                delimiter: Delimiter::Tab,
                ..TableDialect::default()
            },
        };
        let envelope = RuleEnvelope::v2("content:concat", policy.clone());
        let spec = json!({
            "schemaVersion": RULE_PAYLOAD_SCHEMA_VERSION,
            "rule": {"type":"concat"},
            "envelopeVersion": ENVELOPE_VERSION_V2,
            "ruleId": "content:concat",
            "checkerVersion": CHECKER_VERSION_V2,
            "policy": serde_json::to_value(&policy).unwrap(),
            // Since 77bb2a0 an unmarked record means the historical SeparatorV1;
            // `RuleEnvelope::v2` builds the current rule, so the spec names it.
            "mergeRule": "separator-v2",
        });
        assert_eq!(merge_rule::KEY, "mergeRule");
        let read = RuleEnvelope::from_spec(&spec).expect("v2 spec must be readable");
        assert_eq!(read, envelope);
        assert!(!read.is_v1());
        assert_eq!(read.policy.table.delimiter, Delimiter::Tab);
        // The envelope keys never touch `rule`.
        assert_eq!(spec["rule"], json!({"type":"concat"}));
        assert_eq!(RULE_PAYLOAD_SCHEMA_VERSION, 1);
    }

    #[test]
    fn state_vocabulary_is_exactly_the_existing_three_states() {
        assert_eq!(CheckState::Passed.as_str(), "passed");
        assert_eq!(CheckState::Failed.as_str(), "failed");
        assert_eq!(CheckState::Unverified.as_str(), "unverified");
        assert_eq!(serde_json::to_value(CheckState::Passed).unwrap(), json!("passed"));
        assert_eq!(serde_json::to_value(CheckState::Failed).unwrap(), json!("failed"));
        assert_eq!(
            serde_json::to_value(CheckState::Unverified).unwrap(),
            json!("unverified")
        );
    }

    #[test]
    fn verdict_bridge_is_lossless_in_both_directions() {
        let verdicts = [
            RequirementVerdict::Met,
            RequirementVerdict::Unmet("x".into()),
            RequirementVerdict::Unverified("y".into()),
        ];
        let states = [CheckState::Passed, CheckState::Failed, CheckState::Unverified];
        for (verdict, state) in verdicts.iter().zip(states) {
            assert_eq!(CheckState::from(verdict), state);
            let round_tripped = state.to_verdict(Some("r".into()));
            assert_eq!(CheckState::from(&round_tripped), state);
        }
        assert_eq!(
            verdict_from_observation(&failed(
                "r",
                check_name::TABLE_COLUMNS,
                reason_code::TABLE_WIDTH_MISMATCH,
                &binding(),
                "d"
            )),
            RequirementVerdict::Unmet("d".to_owned())
        );
    }

    #[test]
    fn only_failed_blocks_and_arms_a_repair() {
        let failed = failed(
            "r",
            check_name::TABLE_COLUMNS,
            reason_code::TABLE_WIDTH_MISMATCH,
            &binding(),
            "第 3 条记录有 3 个字段，表头有 2 个",
        );
        assert!(failed.blocking);
        let outcome = CheckOutcome::single(failed);
        assert!(outcome.has_failure());
        assert!(outcome.needs_repair());
        assert!(!outcome.is_fully_verified());

        let unverified = unverified(
            "r",
            check_name::TABLE_COLUMNS,
            reason_code::EVIDENCE_UNAVAILABLE,
            &binding(),
            "缺少证据",
        );
        assert!(!unverified.blocking);
        let outcome = CheckOutcome::single(unverified);
        assert!(!outcome.has_failure());
        assert!(!outcome.needs_repair());
        assert!(outcome.has_unverified());
        assert!(!outcome.is_fully_verified());

        let outcome = CheckOutcome::single(passed("r", check_name::TABLE_COLUMNS, &binding()));
        assert!(!outcome.has_failure());
        assert!(!outcome.needs_repair());
        assert!(outcome.is_fully_verified());
    }

    #[test]
    fn a_security_veto_is_never_rendered_as_a_check_state() {
        let outcome = CheckOutcome {
            observations: Vec::new(),
            security_veto: Some(SecurityVeto::new(
                reason_code::security::READONLY_INPUT_WRITE,
                check_name::TABLE_COLUMNS,
                "「原始输入.csv」是 Host 冻结的只读输入",
            )),
        };
        assert!(outcome.has_failure());
        // A veto is a refusal, never something the model is asked to rewrite.
        assert!(!outcome.needs_repair());
        assert!(!outcome.is_fully_verified());
        // And it is not reported through the content-error vocabulary.
        assert_eq!(
            outcome.veto().map(|veto| veto.reason_code.as_str()),
            Some("delivery.security.readonly_input_write")
        );
    }

    #[test]
    fn exceeding_a_ceiling_is_an_explicit_limit_not_a_content_error() {
        let observation = budget_exceeded(
            "r",
            check_name::TABLE_COLUMNS,
            &binding(),
            LimitKind::Records,
            CheckLimits::P0.max_records as u64,
            250_001,
        );
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::BUDGET_EXCEEDED)
        );
        assert_eq!(observation.limit, Some(LimitKind::Records));
        assert!(!observation.blocking);
        assert_eq!(observation.expected.as_deref(), Some("records <= 250000"));
        assert_eq!(observation.actual.as_deref(), Some("250001"));
        // The ceilings are frozen with the checker generation, not per rule.
        assert_eq!(CheckLimits::P0.ceiling(LimitKind::Bytes), 64 * 1024 * 1024);
        assert_eq!(CheckLimits::P0.ceiling(LimitKind::Millis), 30_000);
    }

    #[test]
    fn a_c0_placeholder_can_never_report_a_pass() {
        let observation = not_implemented("r", check_name::TYPED_METRIC, &binding());
        assert_eq!(observation.state, CheckState::Unverified);
        assert_eq!(
            observation.reason_code.as_deref(),
            Some(reason_code::CHECKER_NOT_IMPLEMENTED)
        );
        assert!(!observation.blocking);
    }

    #[test]
    fn reason_codes_and_check_names_are_frozen_literals() {
        let expected = [
            (reason_code::TEXT_SOURCE_MISSING, "delivery.check.text.source_missing"),
            (reason_code::TEXT_ORDER_MISMATCH, "delivery.check.text.order_mismatch"),
            (reason_code::TEXT_SEPARATOR_MISMATCH, "delivery.check.text.separator_mismatch"),
            (reason_code::TEXT_CONTENT_MISMATCH, "delivery.check.text.content_mismatch"),
            (reason_code::TEXT_EOF_MISMATCH, "delivery.check.text.eof_mismatch"),
            (reason_code::TABLE_ENCODING_UNSUPPORTED, "delivery.check.table.encoding_unsupported"),
            (reason_code::TABLE_UNTERMINATED_QUOTE, "delivery.check.table.unterminated_quote"),
            (reason_code::TABLE_CHARACTER_AFTER_QUOTE, "delivery.check.table.character_after_quote"),
            (reason_code::TABLE_UNESCAPED_QUOTE, "delivery.check.table.unescaped_quote"),
            (reason_code::TABLE_HEADER_EMPTY, "delivery.check.table.header_empty"),
            (reason_code::TABLE_HEADER_DUPLICATE, "delivery.check.table.header_duplicate"),
            (reason_code::TABLE_WIDTH_MISMATCH, "delivery.check.table.width_mismatch"),
            (reason_code::TABLE_ZERO_RECORDS, "delivery.check.table.zero_records"),
            (reason_code::TABLE_DIALECT_UNSUPPORTED, "delivery.check.table.dialect_unsupported"),
            (reason_code::BUDGET_EXCEEDED, "delivery.check.budget_exceeded"),
            (reason_code::CONTRACT_UNREADABLE, "delivery.check.contract_unreadable"),
            (reason_code::CONTRACT_VERSION_UNSUPPORTED, "delivery.check.contract_version_unsupported"),
            (reason_code::EVIDENCE_UNAVAILABLE, "delivery.check.evidence_unavailable"),
            (reason_code::CANDIDATE_HASH_DRIFT, "delivery.check.candidate_hash_drift"),
            (reason_code::CHECKER_NOT_IMPLEMENTED, "delivery.check.checker_not_implemented"),
            (reason_code::security::PATH_OUTSIDE_AUTHORIZED_ROOT, "delivery.security.path_outside_authorized_root"),
            (reason_code::security::READONLY_INPUT_WRITE, "delivery.security.readonly_input_write"),
            (reason_code::security::SOURCE_IDENTITY_MISMATCH, "delivery.security.source_identity_mismatch"),
            (reason_code::security::VERSION_CREDENTIAL_INVALID, "delivery.security.version_credential_invalid"),
            (reason_code::security::LOCK_RACE_VERSION_CHANGED, "delivery.security.lock_race_version_changed"),
            (reason_code::security::APPROVAL_MISSING, "delivery.security.approval_missing"),
        ];
        for (actual, frozen) in expected {
            assert_eq!(actual, frozen, "reason code text is a contract");
        }
        assert_eq!(check_name::TEXT_CONCAT, "text_concat");
        assert_eq!(check_name::TABLE_COLUMNS, "table_columns");
        assert_eq!(check_name::TYPED_METRIC, "typed_metric");
        assert_eq!(check_name::LINEAGE, "lineage");
        assert_eq!(check_name::PDF_STRUCTURE, "pdf_structure");
    }

    #[test]
    fn candidate_binding_keeps_the_existing_prefixed_hash_rule() {
        let bound = CandidateBinding::from_finding(
            &json!({
                "artifactId": "art-1",
                "managedVersionId": "ver-1",
                "managedVersionNo": 7,
                "checkedHash": "sha256:aaaaaaaa",
                "sourceRunId": "run-1"
            }),
            "out/report.csv",
            "aaaa",
            3,
        );
        assert_eq!(bound.artifact_id.as_deref(), Some("art-1"));
        assert_eq!(bound.version_id.as_deref(), Some("ver-1"));
        assert_eq!(bound.version_no, Some(7));
        assert_eq!(bound.source_run_id.as_deref(), Some("run-1"));
        assert!(bound.matches_checked_hash("sha256:aaaa"));
        assert!(bound.matches_checked_hash("aaaa"));
        assert!(!bound.matches_checked_hash("bbbb"));
    }

    #[test]
    fn locator_reports_both_the_record_and_the_physical_line() {
        let locator = RecordLocator::cell(3, 2, Some("amount")).at_line(5).at_byte(120);
        assert_eq!(locator.record, Some(3));
        assert_eq!(locator.line, Some(5));
        assert_eq!(locator.column.as_deref(), Some("amount"));
        assert_eq!(locator.byte_offset, Some(120));
        let rendered = locator.display();
        assert!(rendered.contains("第 3 条记录"));
        assert!(rendered.contains("第 5 行"));
        assert!(rendered.contains("amount"));
    }

    #[test]
    fn input_kind_and_dialect_defaults_are_declared_not_guessed() {
        let dialect = TableDialect::default();
        assert_eq!(dialect.delimiter, Delimiter::Comma);
        assert_eq!(dialect.quote, QuotePolicy::DoubleQuote);
        assert_eq!(dialect.bom, BomPolicy::Auto);
        assert_eq!(dialect.line_ending, LineEnding::Any);
        assert_eq!(dialect.blank_line, BlankLinePolicy::Skip);
        assert_eq!(dialect.header, HeaderPolicy::Required);
        // A TSV rule is an explicit declaration, never a guess.
        let tsv = InputKind::Delimited {
            dialect: TableDialect {
                delimiter: Delimiter::Tab,
                ..TableDialect::default()
            },
        };
        assert_eq!(
            serde_json::to_value(&tsv).unwrap()["dialect"]["delimiter"],
            json!("tab")
        );
        assert_eq!(Delimiter::Tab.as_char(), '\t');
    }
}
