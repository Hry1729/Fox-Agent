//! Package 2-B1: pure, internal classification and aggregation of a v2
//! delivery-verification decision.
//!
//! ## What this module is
//!
//! 1. [`classify_v2_facts`] assembles an explicit, ordered set of
//!    [`DecisionFact`]s out of **already-produced evidence**: a decoded spec, an
//!    already-classified typed read, an identity-drift observation, a leaf
//!    [`CheckOutcome`]. A refusal is a refusal because the *producer's own
//!    variant* said so — never because a message string matched a pattern.
//! 2. [`decide`] folds that fact set into one [`DeliveryClass`] in a fixed,
//!    documented priority order. It is a pure function: no I/O, no database, no
//!    clock, no global state.
//!
//! The classifiers that turn a real typed result into facts
//! ([`classify_read_result`], [`drift_fact`]) are called at the producing site:
//! `delivery_content_requirements::classify_v2_requirement` owns the project-root
//! lookup, the typed read and the `checkedHash` comparison, and hands this module
//! the resulting facts.
//!
//! ## What this module deliberately does not do
//!
//! It produces **no** `ItemVerdict`, touches **no** `DeliveryStop`, changes
//! **no** `evaluate_stop`, writes **nothing** to the database and reaches **no**
//! Kernel type. It does have a production caller as of package 2-E:
//! `delivery.rs`'s `route_typed_run` folds every row's facts through [`decide`],
//! and translates a legacy v1 verdict into this vocabulary with
//! `legacy_verdict_decision` so one order decides a mixed Run. `apply` keeps its
//! `schemaVersion != 1` gate, so a v2 spec still cannot reach the v1 lane.
//!
//! There is intentionally no `passed` / `acceptable` boolean on the output. A
//! [`DeliveryClass::Confirmed`] states only that *this verification function's*
//! evidence was sufficient and every observation agreed. It is not a grant of
//! delivery acceptance, which additionally needs current authorization, current
//! identity, the frozen contract and a durable acceptance record — none of which
//! exist yet.
//!
//! ## Fact preservation
//!
//! The aggregate is a *view* over a retained fact set, never a replacement for
//! it. Choosing one [`DeliveryClass`] discards no refusal, no `Failed` and no
//! `Unverified` fact: [`DecisionResult::facts`] carries every one of them.
//!
//! A [`DecisionFact`] keeps the producing check's identity (`rule_id`, `check`,
//! `checker_version`), its structured `expected` / `actual` / `equivalence`, and
//! **both** ceiling dimensions separately: `limit: Option<u64>` for a typed byte
//! ceiling and `limit_kind: Option<LimitKind>` for the leaf's ceiling *kind*.
//! Nothing downstream has to re-derive a number from prose.
//!
//! ## Local aggregation vs. contract confirmation
//!
//! [`decide`] is a **local** fold: given a fact set, it says what that set
//! supports. "Every fact I was handed is `Passed`" is *not* the same claim as
//! "the frozen contract was completely verified", and this module never lets the
//! two be confused:
//!
//! * [`decide`] answers the local question only.
//! * [`classify_and_decide`] — the entry point — additionally demands an
//!   sealed [`Coverage`] value naming the contract's verification
//!   obligations. [`Coverage::Unstated`] (no trustworthy completeness
//!   information), a partial set, an empty obligation set, or any undischarged
//!   obligation makes `Confirmed` unreachable at that entry, and records the gap
//!   as its own retained [`FactClass::Unconfirmed`] fact.
//!
//! This is release-mode behaviour, not a `debug_assert!`: coverage is enforced
//! by the facts the entry returns. The obligation set is always *declared* by
//! the caller — never inferred from observation counts, and never defaulted to
//! "complete".

#![allow(dead_code)]

use serde_json::Value;

use super::super::attachment_compute::DeliveryReadFailure;
use super::super::delivery_checks_v2::{
    CandidateBinding, CheckObservation, CheckOutcome, CheckState, LimitKind, RecordLocator,
    reason_code,
};
use super::content_requirements::{DecodedSpec, decode_spec};

/// One project-authorization refusal, spelled once. The root-lookup adapter and
/// the artifact-read adapter both funnel through [`authorization_refused`], so
/// the same project fact cannot acquire two different names depending on which
/// path observed it.
const AUTHORIZATION_REFUSED: &str = "authorization_refused";

/// The post-verification re-check found the project authorization different from
/// the one the read started under. One spelling, one construction site.
const AUTHORIZATION_CHANGED: &str = "authorization_changed_during_verification";

/// The five classes one fact can carry. This mirrors the leaf's three states
/// plus the two refusal classes the typed reader distinguishes. It is
/// deliberately *not* the aggregate vocabulary ([`DeliveryClass`]), so a single
/// fact can never be misread as "the whole delivery passed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactClass {
    Passed,
    Failed,
    Unconfirmed,
    Invalid,
    Refused,
}

/// Which identity a drift was observed against.
///
/// The role is an explicit input because the same `expected` / `actual` shape is
/// produced by two different production checks. Ownership is never inferred from
/// the fact that a value happens to be a hash, nor from a [`RecordLocator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HashRole {
    /// The candidate bytes no longer match the recorded write-receipt / version
    /// hash (the `checkedHash` comparison `apply` performs at `:963`).
    CandidateReceipt,
    /// A frozen source's bytes no longer match the identity recorded when the
    /// contract was frozen.
    SourceIdentity,
}

impl HashRole {
    /// An **existing, frozen** leaf reason code, reused rather than re-spelled:
    /// a second literal for the same fact would create two names for one truth.
    pub(crate) fn reason_code(self) -> &'static str {
        match self {
            HashRole::CandidateReceipt => reason_code::CANDIDATE_HASH_DRIFT,
            HashRole::SourceIdentity => reason_code::security::SOURCE_IDENTITY_MISMATCH,
        }
    }
}

/// Where a fact came from, and in which role. Locator/binding ownership travels
/// with the fact instead of being guessed downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FactOrigin {
    /// The spec itself: there is no artifact-side location.
    Spec,
    /// The Run/project authorization binding. A root lookup that fails is
    /// reported against the binding, not against the artifact it could not
    /// reach.
    ProjectBinding,
    /// A fact about a **frozen source** input. Any locator describes the
    /// *reference* position and must never enter a candidate position field.
    Source { path: String, sha256: String },
    /// A fact about the **candidate** bytes under verification.
    Candidate { path: String, sha256: String },
    /// A check-level fact: the producer supplied no binding at all. This exists
    /// so an unbound fact never becomes an empty `candidate("", "")`
    /// pseudo-identity, and never claims to be a project-authorization fact it
    /// cannot prove. The fact's own `check` still names the check.
    CheckUnbound,
}

impl FactOrigin {
    pub(crate) fn candidate(path: &str, sha256: &str) -> Self {
        FactOrigin::Candidate {
            path: path.to_owned(),
            sha256: sha256.to_owned(),
        }
    }

    pub(crate) fn source(path: &str, sha256: &str) -> Self {
        FactOrigin::Source {
            path: path.to_owned(),
            sha256: sha256.to_owned(),
        }
    }
}

/// The context a caller can *prove* for a check-level fact.
///
/// A [`SecurityVeto`] carries no binding of its own, so nothing here is inferred
/// from the fact that some observation happens to be nearby: an explicit context
/// wins, then a candidate that is the only one in the outcome, and otherwise the
/// fact stays explicitly unbound. Falling back to the first accompanying
/// observation would assert an ownership the producer never expressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FactContext {
    /// The caller proved which candidate this check ran against.
    Candidate { path: String, sha256: String },
    /// The caller proved which frozen source this check ran against.
    Source { path: String, sha256: String },
    /// The Run/project authorization binding, as an explicit project fact.
    Project,
    /// Nothing was supplied. The check itself is the attribution.
    Unbound,
}

impl FactContext {
    pub(crate) fn unbound() -> Self {
        FactContext::Unbound
    }

    pub(crate) fn origin(&self) -> FactOrigin {
        match self {
            FactContext::Candidate { path, sha256 } => FactOrigin::candidate(path, sha256),
            FactContext::Source { path, sha256 } => FactOrigin::source(path, sha256),
            FactContext::Project => FactOrigin::ProjectBinding,
            FactContext::Unbound => FactOrigin::CheckUnbound,
        }
    }
}

/// One classified verification fact. Every field is evidence, not a summary:
/// retaining facts is what makes it impossible to lose a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionFact {
    pub(crate) class: FactClass,
    pub(crate) origin: FactOrigin,
    /// The producing rule's frozen id, verbatim. Two different rules must never
    /// classify to equal facts.
    pub(crate) rule_id: Option<String>,
    /// The check that produced this fact (its own or a veto's), verbatim.
    pub(crate) check: Option<String>,
    /// The checker version the producing layer reported.
    pub(crate) checker_version: Option<u32>,
    /// A stable reason code where the producing layer has one.
    pub(crate) reason_code: Option<String>,
    /// Human-readable detail, bounded by the producer. Never the whole artifact.
    pub(crate) detail: Option<String>,
    /// True only for a real, checkable mismatch (`failed()` semantics). A
    /// non-blocking failure is still retained and still blocks acceptance.
    pub(crate) blocking: bool,
    /// Leaf [`CheckOutcome::needs_repair`] semantics: a real mismatch that is
    /// not a security refusal and may therefore arm one repair round.
    pub(crate) needs_repair: bool,
    /// A declared **byte** ceiling, when this fact came from a resource gate.
    /// Kept so nothing downstream has to re-derive a budget from prose.
    pub(crate) limit: Option<u64>,
    /// The leaf's declared ceiling **kind**, kept separately from [`Self::limit`]:
    /// `Records`/`Cells`/`Millis` are not byte counts, and `Bytes` is both a kind
    /// and the only kind whose number is a byte count. Collapsing the two would
    /// either lose the kind or misreport a record count as bytes.
    pub(crate) limit_kind: Option<LimitKind>,
    /// The contract's structured expectation, verbatim. Never re-parsed from
    /// [`Self::detail`].
    pub(crate) expected: Option<String>,
    /// What was actually observed, verbatim, on the same terms as
    /// [`Self::expected`].
    pub(crate) actual: Option<String>,
    /// The allowed equivalence a pass relied on. `None` on a plain byte match, so
    /// "passed on equivalence" and "passed byte-exact" stay distinguishable.
    pub(crate) equivalence: Option<String>,
    /// A candidate-side location, only when the producing check bound one.
    pub(crate) locator: Option<RecordLocator>,
}

impl DecisionFact {
    /// A refusal: a security/trust decision. Never repairable, never acceptable.
    pub(crate) fn refused(origin: FactOrigin, reason_code: &str, detail: impl Into<String>) -> Self {
        Self {
            class: FactClass::Refused,
            origin,
            rule_id: None,
            check: None,
            checker_version: None,
            reason_code: Some(reason_code.to_owned()),
            detail: Some(detail.into()),
            blocking: true,
            needs_repair: false,
            limit: None,
            limit_kind: None,
            expected: None,
            actual: None,
            equivalence: None,
            locator: None,
        }
    }

    /// The frozen spec is unusable. The concrete reason is preserved verbatim;
    /// this never collapses into "the content is wrong".
    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Self::invalid_with_code(reason_code::CONTRACT_UNREADABLE, reason)
    }

    /// As [`DecisionFact::invalid`], with an explicit reason code so a version
    /// problem is not reported as a general unreadability problem.
    pub(crate) fn invalid_with_code(code: &str, reason: impl Into<String>) -> Self {
        Self {
            class: FactClass::Invalid,
            origin: FactOrigin::Spec,
            rule_id: None,
            check: None,
            checker_version: None,
            reason_code: Some(code.to_owned()),
            detail: Some(reason.into()),
            blocking: true,
            needs_repair: false,
            limit: None,
            limit_kind: None,
            expected: None,
            actual: None,
            equivalence: None,
            locator: None,
        }
    }

    /// Evidence is missing, unverified, or over a declared ceiling. Explicitly
    /// not a content failure and explicitly not acceptable: `Io` is never
    /// permission to proceed.
    pub(crate) fn unconfirmed(
        origin: FactOrigin,
        reason_code: Option<&str>,
        detail: impl Into<String>,
        limit: Option<u64>,
    ) -> Self {
        Self {
            class: FactClass::Unconfirmed,
            origin,
            rule_id: None,
            check: None,
            checker_version: None,
            reason_code: reason_code.map(str::to_owned),
            detail: Some(detail.into()),
            blocking: false,
            needs_repair: false,
            limit,
            limit_kind: None,
            expected: None,
            actual: None,
            equivalence: None,
            locator: None,
        }
    }

    /// A real, checkable mismatch. `blocking` decides repair eligibility only —
    /// it never makes the item acceptable.
    pub(crate) fn failed(
        origin: FactOrigin,
        reason_code: &str,
        detail: impl Into<String>,
        blocking: bool,
        locator: Option<RecordLocator>,
    ) -> Self {
        Self {
            class: FactClass::Failed,
            origin,
            rule_id: None,
            check: None,
            checker_version: None,
            reason_code: Some(reason_code.to_owned()),
            detail: Some(detail.into()),
            blocking,
            needs_repair: blocking,
            limit: None,
            limit_kind: None,
            expected: None,
            actual: None,
            equivalence: None,
            locator,
        }
    }

    /// An observation that agreed. Individually it proves nothing about the
    /// whole delivery; sufficiency is decided by [`decide`].
    pub(crate) fn passed(origin: FactOrigin) -> Self {
        Self {
            class: FactClass::Passed,
            origin,
            rule_id: None,
            check: None,
            checker_version: None,
            reason_code: None,
            detail: None,
            blocking: false,
            needs_repair: false,
            limit: None,
            limit_kind: None,
            expected: None,
            actual: None,
            equivalence: None,
            locator: None,
        }
    }

    /// Attach a fact that was observed but not decided: the missing-hash case,
    /// where nothing was read so there is no leaf observation to classify.
    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Carry the producing check's identity. Kept verbatim so a fact can be
    /// traced back to the rule/check/version that produced it.
    pub(crate) fn with_check_identity(
        mut self,
        rule_id: Option<&str>,
        check: &str,
        checker_version: u32,
    ) -> Self {
        self.rule_id = rule_id.map(str::to_owned);
        self.check = Some(check.to_owned());
        self.checker_version = Some(checker_version);
        self
    }

    /// Carry the structured expectation/observation and the allowed equivalence
    /// exactly as the leaf produced them.
    pub(crate) fn with_structured(
        mut self,
        expected: Option<&str>,
        actual: Option<&str>,
        equivalence: Option<&str>,
    ) -> Self {
        self.expected = expected.map(str::to_owned);
        self.actual = actual.map(str::to_owned);
        self.equivalence = equivalence.map(str::to_owned);
        self
    }
}

/// The one project-authorization refusal constructor.
///
/// The artifact-read adapter and the project-root adapter both reach project
/// authorization from different places; routing both through here is what stops
/// "the project authorization changed" from acquiring two spellings or drifting
/// apart as one site is edited. The caller supplies the origin it can prove, so
/// the project fact is never replaced by a path.
pub(crate) fn authorization_refused(origin: FactOrigin, detail: &str) -> DecisionFact {
    DecisionFact::refused(origin, AUTHORIZATION_REFUSED, detail.to_owned())
}

/// Classify one typed read failure into a fact.
///
/// This is the whole point of the typed twins: the class comes from the
/// *variant*, so nothing anywhere parses a message to decide whether a
/// readability problem was a project-authorization fact or an OS error.
pub(crate) fn classify_read_failure(
    origin: FactOrigin,
    failure: &DeliveryReadFailure,
) -> DecisionFact {
    match failure {
        // Path escape / non-regular / reparse object: the bytes cannot be
        // authenticated as an in-project deliverable.
        DeliveryReadFailure::TrustRefusal { reason, detail } => {
            DecisionFact::refused(origin, reason, detail.clone())
        }
        // A project-authorization fact: the access frozen for this Run is gone,
        // or no longer matches. Never repairable by the model.
        DeliveryReadFailure::AuthorizationRefused { detail } => {
            authorization_refused(origin, detail)
        }
        // Missing evidence is neither a content verdict nor acceptable.
        DeliveryReadFailure::EvidenceUnavailable { detail } => DecisionFact::unconfirmed(
            origin,
            Some(reason_code::EVIDENCE_UNAVAILABLE),
            detail.clone(),
            None,
        ),
        // A declared ceiling. Retained with its byte limit and kept strictly
        // separate from the model repair budget.
        DeliveryReadFailure::Resource { limit } => DecisionFact::unconfirmed(
            origin,
            Some(reason_code::BUDGET_EXCEEDED),
            format!("交付读取达到资源上限 {limit} 字节"),
            Some(*limit),
        ),
        // An OS/database error proves only that this read did not happen. It is
        // never upgraded to an authorization conclusion and never a pass.
        DeliveryReadFailure::Io(detail) => DecisionFact::unconfirmed(
            origin,
            None,
            format!("交付读取失败：{detail}"),
            None,
        ),
    }
}

/// Classify a typed candidate read at the producing site: the failure variants
/// directly, and an empty-but-readable artifact as missing evidence rather than
/// a silent agreement.
pub(crate) fn classify_read_result(
    relative: &str,
    result: &Result<Vec<u8>, DeliveryReadFailure>,
) -> Vec<DecisionFact> {
    match result {
        Ok(bytes) if bytes.is_empty() => vec![DecisionFact::unconfirmed(
            FactOrigin::candidate(relative, ""),
            Some(reason_code::EVIDENCE_UNAVAILABLE),
            "候选产物为空，没有可核验内容",
            None,
        )],
        Ok(_) => Vec::new(),
        Err(failure) => vec![classify_read_failure(
            FactOrigin::candidate(relative, ""),
            failure,
        )],
    }
}

/// Classify one identity drift. The role decides both the reason code and which
/// identity the fact belongs to.
pub(crate) fn drift_fact(
    role: HashRole,
    expected: &str,
    actual: &str,
    path: &str,
) -> DecisionFact {
    let origin = match role {
        HashRole::CandidateReceipt => FactOrigin::candidate(path, actual),
        HashRole::SourceIdentity => FactOrigin::source(path, actual),
    };
    DecisionFact::refused(
        origin,
        role.reason_code(),
        format!("身份已偏离记录值（期望 {expected}，实际 {actual}）"),
    )
}

/// Classify a typed **project-root / authorization** lookup failure.
///
/// A root lookup has no artifact path yet, so its facts are attributed to the
/// Run binding. `AuthorizationRefused` here is the explicit project-level fact
/// (access removed, root changed) — it is legitimate to classify, and it does
/// **not** invent a path. An OS/DB error stays `Io` and never becomes an
/// authorization conclusion.
pub(crate) fn classify_root_failure(failure: &DeliveryReadFailure) -> DecisionFact {
    match failure {
        DeliveryReadFailure::AuthorizationRefused { detail } => {
            authorization_refused(FactOrigin::ProjectBinding, detail)
        }
        DeliveryReadFailure::TrustRefusal { reason, detail } => DecisionFact::refused(
            FactOrigin::ProjectBinding,
            reason,
            detail.clone(),
        ),
        DeliveryReadFailure::EvidenceUnavailable { detail } => DecisionFact::unconfirmed(
            FactOrigin::ProjectBinding,
            Some(reason_code::EVIDENCE_UNAVAILABLE),
            detail.clone(),
            None,
        ),
        DeliveryReadFailure::Resource { limit } => DecisionFact::unconfirmed(
            FactOrigin::ProjectBinding,
            Some(reason_code::BUDGET_EXCEEDED),
            format!("交付根读取达到资源上限 {limit} 字节"),
            Some(*limit),
        ),
        DeliveryReadFailure::Io(detail) => DecisionFact::unconfirmed(
            FactOrigin::ProjectBinding,
            None,
            format!("交付根查找失败：{detail}"),
            None,
        ),
    }
}

/// The candidate's recorded receipt hash, as the input actually presented it.
///
/// The three cases are kept apart because only `Recorded` can ever agree with
/// the bytes: an absent receipt is missing *evidence*, while an empty or
/// malformed one is present-but-unusable and stays on the existing non-match
/// path. Neither may collapse into a silent pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckedHash {
    /// A usable hex hash, `sha256:` prefix already stripped.
    Recorded(String),
    /// Present but not a usable hash: empty, or not 64 hex digits.
    Malformed { value: String },
    /// The key is absent, JSON null, or not a string at all.
    Absent(&'static str),
}

/// Read the candidate's `checkedHash` as a typed input category.
///
/// This never treats a missing or malformed receipt as agreement, and it never
/// fabricates a hash drift from a value that is not a hash at all.
pub(crate) fn classify_checked_hash(finding: &Value) -> CheckedHash {
    match finding.get("checkedHash") {
        None => CheckedHash::Absent("字段缺失"),
        Some(Value::Null) => CheckedHash::Absent("字段为 null"),
        Some(Value::String(value)) => {
            let hex = value.trim_start_matches("sha256:");
            let usable = hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
            if usable {
                CheckedHash::Recorded(hex.to_owned())
            } else {
                CheckedHash::Malformed {
                    value: value.clone(),
                }
            }
        }
        Some(_) => CheckedHash::Absent("字段不是字符串"),
    }
}

/// Classify the candidate's `checkedHash` comparison — the drift check `apply`
/// performs at `:963`. `expected` is the recorded receipt/version hash;
/// `actual` is the hash of the bytes that were really read. A mismatch is a
/// refusal (reusing the frozen `CANDIDATE_HASH_DRIFT` code), never a spec error.
pub(crate) fn candidate_hash_drift_fact(expected: &str, actual: &str, path: &str) -> DecisionFact {
    drift_fact(HashRole::CandidateReceipt, expected, actual, path)
}

/// A present-but-unusable receipt hash. This keeps the existing non-match
/// refusal semantics (an empty or malformed hash is not a match), while
/// recording the input category so the raw value is not lost behind a generic
/// "the hashes differ".
pub(crate) fn candidate_hash_malformed_fact(value: &str, actual: &str, path: &str) -> DecisionFact {
    let mut fact = drift_fact(HashRole::CandidateReceipt, value, actual, path);
    fact.detail = Some(format!(
        "产物回执哈希不可用（原始值 {value:?}），按非匹配处理；实际 {actual}"
    ));
    fact
}

/// Classify a frozen-source identity drift: the source no longer hashes to the
/// value recorded when the contract was frozen. Reuses the frozen
/// `security::SOURCE_IDENTITY_MISMATCH` code.
pub(crate) fn source_hash_drift_fact(expected: &str, actual: &str, path: &str) -> DecisionFact {
    drift_fact(HashRole::SourceIdentity, expected, actual, path)
}

/// The post-read authorization re-check fact (`apply` re-resolves the root after
/// verification and compares it against the one the read started under).
///
/// This is a **project-binding** fact: the Run's authorization changed, which is
/// a property of the project binding, not of the artifact's content. A known
/// candidate path is never a substitute for the project fact.
pub(crate) fn authorization_changed_fact() -> DecisionFact {
    DecisionFact::refused(
        FactOrigin::ProjectBinding,
        AUTHORIZATION_CHANGED,
        "内容核验期间项目授权改变，未核验",
    )
}

/// Everything [`classify_v2_facts`] is allowed to look at.
///
/// `read_facts` and `drift_facts` arrive already classified from the producing
/// site, which keeps every typed-variant decision at the point where the real
/// result exists. `outcome` stays optional when the controlled execution cannot
/// run a leaf. Production coverage can only be created by that execution.
pub(crate) struct FactInput<'a> {
    pub(crate) spec: &'a Value,
    pub(crate) read_facts: Vec<DecisionFact>,
    pub(crate) drift_facts: Vec<DecisionFact>,
    pub(crate) outcome: Option<&'a CheckOutcome>,
    pub(crate) authorization_changed: bool,
    /// What the contract required to be verified, as far as the caller can
    /// prove it. Defaults to [`Coverage::Unstated`], which forbids `Confirmed`:
    /// a caller that says nothing about completeness cannot get a confirmation
    /// by saying nothing.
    pub(crate) coverage: Coverage,
    /// The candidate the check ran against, when the caller can prove it. Used
    /// only to attribute a check-level fact (a security veto carries no binding
    /// of its own); it never overrides the observation's own binding.
    pub(crate) context: Option<FactContext>,
}

pub(crate) use super::content_requirements::execution::Coverage;

/// A candidate binding that is the *only* one in the outcome — the one case
/// where a check-level fact's ownership is provable rather than guessed.
fn sole_candidate(outcome: &CheckOutcome) -> Option<&CandidateBinding> {
    let mut found: Option<&CandidateBinding> = None;
    for observation in &outcome.observations {
        let binding = observation.candidate.as_ref()?;
        match found {
            None => found = Some(binding),
            Some(previous) if previous.path == binding.path && previous.sha256 == binding.sha256 => {}
            // Two different candidates: the set proves nothing about which one a
            // veto belongs to.
            Some(_) => return None,
        }
    }
    found
}

/// Resolve the origin of a check-level fact (a security veto, which carries no
/// binding). Explicit caller context wins; a sole candidate is then provable;
/// otherwise the fact stays explicitly unbound. Never the first of several.
fn check_origin(outcome: &CheckOutcome, explicit: Option<&FactContext>) -> FactOrigin {
    if let Some(context) = explicit {
        return context.origin();
    }
    match sole_candidate(outcome) {
        Some(binding) => FactOrigin::candidate(&binding.path, &binding.sha256),
        None => FactOrigin::CheckUnbound,
    }
}

/// The candidate origin of one leaf observation, taken from the binding the
/// check itself attached. An observation without a binding is explicitly
/// check-level rather than an empty identity, so it never passes for a real
/// candidate and never borrows someone else's.
fn observation_origin(observation: &CheckObservation) -> FactOrigin {
    match &observation.candidate {
        Some(binding) => FactOrigin::candidate(&binding.path, &binding.sha256),
        None => FactOrigin::CheckUnbound,
    }
}

/// Classify one leaf outcome into facts.
///
/// Leaf precedence is reused, not re-derived: `needs_repair` and
/// `is_fully_verified` are the leaf's own definitions, so this aggregation
/// cannot drift from the checker layer it reads.
fn classify_outcome(outcome: &CheckOutcome, context: Option<&FactContext>) -> Vec<DecisionFact> {
    let mut facts = Vec::new();

    // A security veto is separate and never downgradable: it is a refusal, and
    // it never arms a repair round.
    if let Some(veto) = outcome.veto() {
        let mut fact = DecisionFact::refused(
            check_origin(outcome, context),
            &veto.reason_code,
            veto.detail.clone(),
        );
        // A veto's own `check` is kept; it has no rule id or version of its own,
        // so none is invented for it.
        fact.check = Some(veto.check.clone());
        facts.push(fact);
    }

    for observation in &outcome.observations {
        let origin = observation_origin(observation);
        let structured = (observation.expected.as_deref(), observation.actual.as_deref());
        match observation.state {
            CheckState::Passed => {
                let mut fact = DecisionFact::passed(origin);
                fact = fact
                    .with_check_identity(Some(&observation.rule_id), &observation.check, observation.checker_version)
                    .with_structured(structured.0, structured.1, observation.equivalence.as_deref());
                fact.locator = observation.locator.clone();
                facts.push(fact);
            }
            CheckState::Failed => {
                let code = observation
                    .reason_code
                    .clone()
                    .unwrap_or_else(|| "delivery.check.failed".to_owned());
                let detail = observation
                    .detail
                    .clone()
                    .unwrap_or_else(|| "真实交付内容与冻结合同不一致".to_owned());
                let mut fact = DecisionFact::failed(
                    origin,
                    &code,
                    detail,
                    observation.blocking,
                    observation.locator.clone(),
                );
                // Take the repair decision from the leaf rather than assuming
                // "failed implies repairable": a non-blocking outcome, or one
                // the leaf refuses to repair, must not become a repair round.
                fact.needs_repair = outcome.needs_repair() && observation.blocking;
                fact = fact
                    .with_check_identity(Some(&observation.rule_id), &observation.check, observation.checker_version)
                    .with_structured(structured.0, structured.1, observation.equivalence.as_deref());
                facts.push(fact);
            }
            CheckState::Unverified => {
                let mut fact = DecisionFact::unconfirmed(
                    origin,
                    observation.reason_code.as_deref(),
                    observation
                        .detail
                        .clone()
                        .unwrap_or_else(|| "该交付要求尚未核验".to_owned()),
                    // `limit_kind` below names the ceiling kind; `limit` stays
                    // reserved for a byte-denominated gate (the typed
                    // `Resource { limit }`), because a record count is not bytes.
                    None,
                );
                fact.limit_kind = observation.limit;
                fact = fact
                    .with_check_identity(Some(&observation.rule_id), &observation.check, observation.checker_version)
                    .with_structured(structured.0, structured.1, observation.equivalence.as_deref());
                fact.locator = observation.locator.clone();
                facts.push(fact);
            }
        }
    }

    debug_assert!(
        !outcome.is_fully_verified()
            || facts.iter().all(|fact| fact.class == FactClass::Passed),
        "a fully verified leaf outcome must classify as passes only"
    );
    // Cross-check against the leaf's own failure predicate: classifying must not
    // drop a failure. Agreement is asserted both ways, so an observation that
    // the leaf calls a failure but this layer forgets fails loudly in tests.
    debug_assert_eq!(
        outcome.has_failure(),
        facts
            .iter()
            .any(|fact| matches!(fact.class, FactClass::Failed | FactClass::Refused)),
        "a leaf failure must survive classification as a Failed/Refused fact"
    );
    facts
}

/// The classification entry point, in its fixed order: spec -> read ->
/// identity drift -> authorization re-check -> leaf. The order is stable so the
/// fact vector is deterministic and a spec rejection is always visible before
/// any content fact.
pub(crate) fn classify_v2_facts(input: &FactInput<'_>) -> Vec<DecisionFact> {
    let mut facts = Vec::new();

    // 1. The spec. `decode_spec` performs no I/O and never falls back to v1 on
    //    missing/invalid data, so an unusable spec is a fact in its own right.
    //    The reason code is chosen *structurally* from the envelope's own
    //    `schemaVersion` shape — never by matching the decoder's message text.
    let is_v2 = match decode_spec(input.spec) {
        DecodedSpec::Invalid(reason) => {
            let version_shaped = matches!(
                input.spec.get("schemaVersion").and_then(Value::as_u64),
                Some(1 | 2)
            );
            let code = if version_shaped {
                reason_code::CONTRACT_UNREADABLE
            } else {
                reason_code::CONTRACT_VERSION_UNSUPPORTED
            };
            facts.push(DecisionFact::invalid_with_code(code, reason));
            false
        }
        DecodedSpec::V2(_) => true,
        // A v1 record is not this function's to classify: the v1 path owns it
        // and must stay byte-identical.
        DecodedSpec::V1(..) => false,
    };

    // 2. Candidate read facts, classified by the typed producer.
    facts.extend(input.read_facts.iter().cloned());

    // 3. Identity drift, by role. A drift is a refusal, not a spec problem.
    facts.extend(input.drift_facts.iter().cloned());

    // 4. The post-verification authorization re-check.
    if input.authorization_changed {
        facts.push(authorization_changed_fact());
    }

    // 5. The leaf result, when a checker actually ran. Only meaningful for a
    //    decodable v2 spec: a spec failure keeps its own fact rather than being
    //    diluted by content facts, but every fact is retained either way.
    if is_v2 {
        if let Some(outcome) = input.outcome {
            facts.extend(classify_outcome(outcome, input.context.as_ref()));
        }
    }

    // 6. Coverage. An unproven contract completeness is its own retained fact,
    //    so `Confirmed` is unreachable without an explicit discharge of every
    //    obligation. This is release-mode, not a `debug_assert!`: the fact is
    //    what blocks the class, and it survives even when every leaf fact passed.
    if !input.coverage.fully_discharged() {
        let detail = input.coverage.detail();
        let mut fact = DecisionFact::unconfirmed(
            FactOrigin::Spec,
            Some(reason_code::EVIDENCE_UNAVAILABLE),
            detail,
            None,
        );
        fact.check = Some(COVERAGE_CHECK.to_owned());
        facts.push(fact);
    }

    facts
}

/// The synthetic check name carried by the coverage fact. No leaf module
/// implements it: the fact exists because no single leaf can prove the whole
/// contract was covered.
const COVERAGE_CHECK: &str = "delivery.coverage";

/// The aggregate outcome of one delivery-verification decision.
///
/// `DefiniteFailure` is deliberately distinct from `Invalid`: an exhausted
/// repair budget on real content failure must never be renamed "the spec is
/// illegal". Two facts, two remedies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryClass {
    /// Evidence was sufficient and every observation agreed. Not a grant of
    /// acceptance — see the module docs.
    Confirmed,
    /// A security/trust refusal. Forbids repair and acceptance.
    Refused,
    /// The frozen spec is unusable. Forbids repair and acceptance; the legacy
    /// missing-schema production behaviour is untouched by this.
    Invalid,
    /// A real blocking mismatch with repair budget left. Not acceptable.
    RepairNeeded,
    /// Evidence is missing, over budget, or explicitly unverified. Not
    /// acceptable, and not a content failure.
    Unconfirmed,
    /// A real failure that cannot be repaired within budget — including a
    /// non-blocking failure, which is a definite non-pass. Not acceptable, and
    /// not a spec problem.
    DefiniteFailure,
}

impl DeliveryClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DeliveryClass::Confirmed => "confirmed",
            DeliveryClass::Refused => "refused",
            DeliveryClass::Invalid => "invalid",
            DeliveryClass::RepairNeeded => "repairNeeded",
            DeliveryClass::Unconfirmed => "unconfirmed",
            DeliveryClass::DefiniteFailure => "definiteFailure",
        }
    }

    /// The exact inverse of [`Self::as_str`].
    ///
    /// Added by package 2-B2 so the durable record can be decoded without a
    /// second spelling of the class vocabulary: a string this function does not
    /// recognise is an error, never a default. The classification policy itself
    /// is unchanged.
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "confirmed" => Some(DeliveryClass::Confirmed),
            "refused" => Some(DeliveryClass::Refused),
            "invalid" => Some(DeliveryClass::Invalid),
            "repairNeeded" => Some(DeliveryClass::RepairNeeded),
            "unconfirmed" => Some(DeliveryClass::Unconfirmed),
            "definiteFailure" => Some(DeliveryClass::DefiniteFailure),
            _ => None,
        }
    }
}

/// The decision, plus **every** fact it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionResult {
    pub(crate) class: DeliveryClass,
    pub(crate) facts: Vec<DecisionFact>,
    /// Whether a repair round could still be armed. Separate from any resource
    /// budget: a resource ceiling is a fact, never a repair round.
    pub(crate) repair_budget_remaining: bool,
}

impl DecisionResult {
    pub(crate) fn has_class(&self, class: FactClass) -> bool {
        self.facts.iter().any(|fact| fact.class == class)
    }

    /// Retained refusals, for a caller that must show *why* nothing is
    /// acceptable. Never empty when `class` is [`DeliveryClass::Refused`].
    pub(crate) fn refusals(&self) -> impl Iterator<Item = &DecisionFact> {
        self.facts
            .iter()
            .filter(|fact| fact.class == FactClass::Refused)
    }
}

/// Fold a fact set into one decision. This is the **local** fold: it says what
/// the given facts support, nothing about whether the contract was covered.
/// [`classify_and_decide`] is the entry that additionally requires a proven
/// [`Coverage`] before `Confirmed` can be reached.
///
/// The order below is the contract. It is expressed as typed tests over the fact
/// classes — **never** as "all passed first" — because `Unconfirmed` used to be
/// carriable with `passed == true`, which let an all-passed check swallow an
/// unverified item.
///
/// 1. any `Refused` -> `Refused` (repair and acceptance forbidden)
/// 2. any spec `Invalid` -> `Invalid` (repair and acceptance forbidden)
/// 3. any blocking `Failed` with repair budget left -> `RepairNeeded`, even
///    alongside non-blocking failures or unverified facts (all retained)
/// 4. any `Failed` remaining -> `DefiniteFailure`; a non-blocking failure is a
///    definite non-pass, and an exhausted budget is not a spec problem
/// 5. otherwise any `Unconfirmed`, or no fact at all -> `Unconfirmed`; a
///    non-empty contract with zero observations is exactly this case
/// 6. only a non-empty set of exclusively `Passed` facts -> `Confirmed`
///
/// Step 6 is the local reading only. A caller that has not proven coverage must
/// not treat it as "the contract passed" — see [`classify_and_decide`].
///
/// A legacy `NoChecklist` Run has no checklist rows and never reaches this
/// function; nothing here impersonates that state as confirmed.
pub(crate) fn decide(facts: &[DecisionFact], repair_budget_remaining: bool) -> DecisionResult {
    let has = |class: FactClass| facts.iter().any(|fact| fact.class == class);
    let class = if has(FactClass::Refused) {
        DeliveryClass::Refused
    } else if has(FactClass::Invalid) {
        DeliveryClass::Invalid
    } else if repair_budget_remaining
        && facts
            .iter()
            .any(|fact| fact.class == FactClass::Failed && fact.blocking && fact.needs_repair)
    {
        DeliveryClass::RepairNeeded
    } else if has(FactClass::Failed) {
        // Budget exhausted, or a non-blocking failure that was never
        // repairable. Both are definite non-passes; the retained facts still
        // tell the two apart.
        DeliveryClass::DefiniteFailure
    } else if has(FactClass::Unconfirmed) || facts.is_empty() {
        // An empty fact set means "nothing was observed", which is precisely
        // unconfirmed: it must never read as sufficient verification.
        DeliveryClass::Unconfirmed
    } else {
        DeliveryClass::Confirmed
    };
    DecisionResult {
        class,
        facts: facts.to_vec(),
        repair_budget_remaining,
    }
}

/// Classify then decide, retaining the facts **and** enforcing coverage.
///
/// This is the outward internal entry point, and it is where "every fact I saw
/// passed" is kept separate from "the contract was verified". The coverage gate
/// lives in [`classify_v2_facts`], so an unproven completeness arrives here as a
/// retained `Unconfirmed` fact and `Confirmed` is unreachable — including when
/// the caller supplies an all-`Passed` leaf outcome, and including in a release
/// build with debug assertions off. No flag defaults to "complete".
pub(crate) fn classify_and_decide(
    input: &FactInput<'_>,
    repair_budget_remaining: bool,
) -> DecisionResult {
    decide(&classify_v2_facts(input), repair_budget_remaining)
}

// Test-only registration: nothing here is compiled into a production build.
#[cfg(test)]
#[path = "p2b_decision_tests.rs"]
mod p2b_decision_tests;
