//! Package 2-B2: the typed **consumer** of a package-2-B1 delivery decision.
//!
//! ## What this module is
//!
//! Package 2-B1 produces a [`DecisionResult`]: one [`DeliveryClass`] plus every
//! [`DecisionFact`] it was folded from. This module turns one or more of those
//! results into a *typed delivery verdict* a Host can act on and persist, and it
//! defines the durable encoding that verdict is written in.
//!
//! The aggregate is deliberately **not** a second boolean fold. Every item's
//! facts are concatenated and handed back to 2-B1's own [`decide`], so the
//! priority order (`Refused` > `Invalid` > repairable `Failed` > `DefiniteFailure`
//! > `Unconfirmed` > `Confirmed`) exists in exactly one place and one passing item
//! can never cover another item's refusal, definite failure or unknown fact.
//!
//! ## The six classes and what a consumer does with them
//!
//! | class | consumer behaviour |
//! |---|---|
//! | `Refused` | no repair; not passed; the existing failed outcome carries the terminal |
//! | `Invalid` | the spec is unusable; no repair; not passed; terminal |
//! | `RepairNeeded` | one bounded repair round, never over a higher-priority refusal |
//! | `DefiniteFailure` | definite non-pass (including an exhausted budget); never renamed `Invalid` |
//! | `Unconfirmed` | not passed, not repaired, **not a failed terminal**: [`DeliveryDisposition::RequiresU1`] |
//! | `Confirmed` | only a sufficient verdict maps to acceptance; coverage is never bypassed |
//!
//! ## What this module deliberately does not do
//!
//! It performs no I/O, reaches no Kernel type, writes no database row and
//! terminates nothing. It is the *type mapping*; the Host/Kernel edges and the
//! durable writes live at their own call sites. It adds **no** Kernel state and
//! **no** database status string.
//!
//! `Unconfirmed` deliberately stops at [`DeliveryDisposition::RequiresU1`]: U1
//! (accept / wait / resume) is not implemented, so this package must not call
//! terminate, must not fabricate a waiting or approval event, and must not claim
//! a resumable wait exists. The artifacts and facts are retained so that a later
//! U1 has something real to activate on. Package 2-E carries that deferred
//! aggregate as a stop whose terminal row set is empty, rather than as a bare
//! `Err` at the producer, so the retention path is reached with the same verdict.
//!
//! ## Coverage
//!
//! A [`DeliveryClass::Confirmed`] here means only "the supplied decision said the
//! evidence was sufficient". Constructing one in a test proves the **mapping**;
//! it never proves the frozen contract was completely verified. Production now
//! reaches it only when the typed execution actually discharged the contract's
//! obligations: `classify_v2_requirement` runs the 2-C2 coverage gate, so a
//! `Concat` row's `concat.all_sources` must be discharged by a passing leaf
//! observation. Nothing in this module changes that.

#![allow(dead_code)]

use serde_json::{Map, Value, json};

use super::p2b_decision::{
    DecisionFact, DecisionResult, DeliveryClass, FactClass, FactOrigin, decide,
};
use super::super::delivery_checks_v2::{LimitKind, RecordLocator};

/// The durable verdict-record version. Written by this consumer and required on
/// read, so a v2 verdict record is identified by its **own** version pair rather
/// than by any single upstream finding key that a projection might drop.
pub(crate) const VERDICT_SCHEMA_VERSION: u32 = 2;
pub(crate) const VERDICT_ENVELOPE_VERSION: u32 = 2;

/// The envelope key inside a persisted finding.
const VERDICT_KEY: &str = "deliveryVerdict";

/// Two items of one verdict claim the same `item_key`.
///
/// This is an **input ambiguity**, not a verdict class. The ledger is keyed by
/// `item_key` (`delivery_outcome_stage` has `PRIMARY KEY(run_id,item_key)`), so
/// two items under one key are two decisions competing for one row. Which of them
/// is "the" decision is not something this layer may decide: choosing one silently
/// discards the other item's facts, and merging them would invent a verdict that
/// neither item produced.
///
/// So it is refused **before any write**. The error carries the conflicting key and
/// both original items intact, so the caller can route it as the input problem it
/// is. It is deliberately not one of `Invalid` / `Refused` / `DefiniteFailure`:
/// those are statements about a delivery that was evaluated, and this is a
/// statement that the input could not be evaluated at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbiguousItemKey {
    /// The key both items claim.
    pub(crate) item_key: String,
    /// Every item carrying that key, in verdict order, with its full facts.
    pub(crate) collisions: Vec<ItemDecision>,
}

impl AmbiguousItemKey {
    /// Every duplicated key in a verdict, with the items claiming it.
    ///
    /// Scans the **whole** verdict rather than reporting the first hit, so a
    /// caller sees the complete set of ambiguous keys in one go. Duplicates are
    /// reported even when the items are byte-identical: an identical pair is still
    /// two decisions about one row, and "which one is durable" has no answer that
    /// does not discard something.
    pub(crate) fn all_in(verdict: &DeliveryVerdict) -> Vec<Self> {
        let mut keys: Vec<&str> = verdict
            .items
            .iter()
            .map(|item| item.item_key.as_str())
            .collect();
        keys.sort_unstable();
        let mut duplicates: Vec<String> = Vec::new();
        for pair in keys.windows(2) {
            if pair[0] == pair[1] && duplicates.last().map(String::as_str) != Some(pair[0]) {
                duplicates.push(pair[0].to_owned());
            }
        }
        duplicates
            .into_iter()
            .map(|item_key| Self {
                collisions: verdict
                    .items
                    .iter()
                    .filter(|item| item.item_key == item_key)
                    .cloned()
                    .collect(),
                item_key,
            })
            .collect()
    }

    /// A bounded, single-line message that names the key.
    ///
    /// The full items stay in the structured error; this is only the human-facing
    /// summary, so it quotes the key and the number of competing items rather than
    /// flattening the facts into text.
    pub(crate) fn message(&self) -> String {
        format!(
            "the delivery verdict names item_key `{}` {} times, so its facts cannot be written to \
             one ledger row without losing a decision",
            self.item_key,
            self.collisions.len()
        )
    }
}

/// What a consumer is supposed to do about a delivery, given the class.
///
/// Four behaviours over six classes: `Refused`, `Invalid` and `DefiniteFailure`
/// all end the Run as a definite non-pass, but they keep their own class so the
/// reason is never flattened into "the spec was illegal" or vice versa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeliveryDisposition {
    /// The verdict was sufficient. Reached only with an explicit, proven
    /// coverage; never a bypass of the coverage gate.
    Accept,
    /// A real, repairable mismatch with budget left. Arms one repair round.
    Repair,
    /// A definite non-pass. Ends the Run through the **existing** failed outcome;
    /// no Kernel state is added for it.
    Terminal {
        /// The class that made this terminal, kept so `Invalid` and
        /// `DefiniteFailure` never collapse into each other.
        class: DeliveryClass,
        /// The centrally chosen failure code.
        code: &'static str,
        /// A bounded message quoting the retained reason codes.
        message: String,
    },
    /// Not passed, not failed, not repairable here: the delivery is waiting on U1.
    ///
    /// This is an explicit internal boundary, not a resumable wait. Nothing was
    /// terminated, no completion or failure event was emitted, and no wait
    /// facility is claimed to exist.
    RequiresU1 { reason: String },
}

/// The centrally-defined failed-outcome codes for a definite non-pass.
///
/// Defined once each. The concrete check reason is preserved alongside them in
/// the retained facts **and** quoted into the message, so a caller does not have
/// to invent a code per check.
pub(crate) const DELIVERY_REFUSED: &str = "delivery.refused";
pub(crate) const DELIVERY_INVALID: &str = "delivery.invalid";
pub(crate) const DELIVERY_DEFINITE_FAILURE: &str = "delivery.definite_failure";

/// One checklist item's decision: its class plus every fact behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemDecision {
    pub(crate) item_key: String,
    pub(crate) class: DeliveryClass,
    pub(crate) facts: Vec<DecisionFact>,
    pub(crate) repair_budget_remaining: bool,
}

impl ItemDecision {
    /// Fold one item's requirements into a single item decision.
    ///
    /// Every requirement's facts are concatenated and folded by 2-B1's [`decide`],
    /// so an item with two requirements cannot have one requirement's pass hide
    /// the other's refusal.
    pub(crate) fn from_results(
        item_key: &str,
        results: &[DecisionResult],
        repair_budget_remaining: bool,
    ) -> Self {
        let mut facts = Vec::new();
        for result in results {
            facts.extend(result.facts.iter().cloned());
        }
        let folded = decide(&facts, repair_budget_remaining);
        Self {
            item_key: item_key.to_owned(),
            class: folded.class,
            facts,
            repair_budget_remaining,
        }
    }

    /// Whether the terminal stage may carry this item's row.
    ///
    /// Everything except `Unconfirmed` may. That is not a statement that the
    /// class is a final outcome — `RepairNeeded` is not — but that the stage is
    /// the right write for it: a repair round is itself a committed decision that
    /// carries the round's verdicts and finalizes them, exactly as the legacy
    /// `Repair` arm does. `Unconfirmed` is the one class that has no such
    /// decision: the stage's `passed = 0` is finalized as `failed`, which would
    /// turn "the content was never verified" into "the content failed".
    pub(crate) fn may_enter_terminal_stage(&self) -> bool {
        !matches!(self.class, DeliveryClass::Unconfirmed)
    }

    /// Whether this item's class is a final outcome, as opposed to a class that
    /// still has a live repair round in front of it.
    pub(crate) fn is_final_class(&self) -> bool {
        matches!(
            self.class,
            DeliveryClass::Confirmed
                | DeliveryClass::Refused
                | DeliveryClass::Invalid
                | DeliveryClass::DefiniteFailure
        )
    }

    /// Whether this item's facts may be read as "passed".
    pub(crate) fn is_passed(&self) -> bool {
        self.class == DeliveryClass::Confirmed
    }
}

/// The whole delivery's verdict, folding every item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryVerdict {
    /// The aggregate class, folded from **all** items' facts by 2-B1's order.
    pub(crate) class: DeliveryClass,
    pub(crate) items: Vec<ItemDecision>,
    pub(crate) repair_budget_remaining: bool,
}

impl DeliveryVerdict {
    /// Fold item decisions into one verdict.
    ///
    /// The aggregate is computed by handing every item's facts back to 2-B1's
    /// [`decide`]. A second boolean aggregation would be a second place for the
    /// priority order to drift, and would let a passing item mask a refusal.
    pub(crate) fn from_items(items: Vec<ItemDecision>, repair_budget_remaining: bool) -> Self {
        let facts: Vec<DecisionFact> = items
            .iter()
            .flat_map(|item| item.facts.iter().cloned())
            .collect();
        let folded = decide(&facts, repair_budget_remaining);
        Self {
            class: folded.class,
            items,
            repair_budget_remaining,
        }
    }

    /// Every retained fact across every item, in item order.
    pub(crate) fn facts(&self) -> impl Iterator<Item = &DecisionFact> {
        self.items.iter().flat_map(|item| item.facts.iter())
    }

    /// The items whose class is a final outcome, i.e. the classes that are not
    /// still waiting on a repair round or on U1.
    pub(crate) fn final_items(&self) -> impl Iterator<Item = &ItemDecision> {
        self.items.iter().filter(|item| item.is_final_class())
    }

    /// The items that must be retained **without** a terminal write.
    pub(crate) fn pending_items(&self) -> impl Iterator<Item = &ItemDecision> {
        self.items.iter().filter(|item| !item.may_enter_terminal_stage())
    }

    /// What a consumer does with this verdict.
    pub(crate) fn disposition(&self) -> DeliveryDisposition {
        match self.class {
            DeliveryClass::Confirmed => DeliveryDisposition::Accept,
            DeliveryClass::RepairNeeded => DeliveryDisposition::Repair,
            DeliveryClass::Refused => DeliveryDisposition::Terminal {
                class: DeliveryClass::Refused,
                code: DELIVERY_REFUSED,
                message: self.terminal_message("交付被安全/信任拒绝"),
            },
            DeliveryClass::Invalid => DeliveryDisposition::Terminal {
                class: DeliveryClass::Invalid,
                code: DELIVERY_INVALID,
                message: self.terminal_message("内容契约不可用"),
            },
            DeliveryClass::DefiniteFailure => DeliveryDisposition::Terminal {
                class: DeliveryClass::DefiniteFailure,
                code: DELIVERY_DEFINITE_FAILURE,
                message: self.terminal_message("交付内容确定不通过"),
            },
            DeliveryClass::Unconfirmed => DeliveryDisposition::RequiresU1 {
                reason: self.terminal_message("交付证据不足，等待 U1 接受/恢复"),
            },
        }
    }

    /// A bounded message that keeps the concrete check reasons visible instead of
    /// replacing them with the aggregate class.
    fn terminal_message(&self, prefix: &str) -> String {
        let reasons: Vec<String> = self
            .facts()
            .filter(|fact| {
                matches!(
                    fact.class,
                    FactClass::Refused | FactClass::Invalid | FactClass::Failed
                )
            })
            .filter_map(|fact| fact.reason_code.clone())
            .collect();
        let mut unique: Vec<String> = Vec::new();
        for reason in reasons {
            if !unique.contains(&reason) {
                unique.push(reason);
            }
        }
        let quoted = if unique.is_empty() {
            "无具体检查原因码".to_owned()
        } else {
            unique.join(", ")
        };
        let bounded: String = format!("{prefix}：{quoted}").chars().take(400).collect();
        bounded
    }

    /// True when this verdict asks for U1 and therefore must not terminate, must
    /// not repair and must not emit a completion or failure event.
    pub(crate) fn requires_u1(&self) -> bool {
        matches!(self.disposition(), DeliveryDisposition::RequiresU1 { .. })
    }

    // -----------------------------------------------------------------------
    // Durable encoding
    // -----------------------------------------------------------------------

    /// The finding a persisted row carries for this item.
    ///
    /// `existing` is the item's own finding as the gate produced it; its keys are
    /// preserved so no legacy key is lost by adding the verdict. The verdict
    /// envelope is self-contained per row, so a single row is decodable on its
    /// own — which is what the non-terminal (pending) retention path needs.
    pub(crate) fn item_finding(&self, item: &ItemDecision, existing: &Value) -> Value {
        stamp_envelope(self, item, existing)
    }

    fn encode_envelope(&self, item: &ItemDecision) -> Value {
        let mut envelope = Map::new();
        envelope.insert("verdictSchemaVersion".into(), json!(VERDICT_SCHEMA_VERSION));
        envelope.insert(
            "verdictEnvelopeVersion".into(),
            json!(VERDICT_ENVELOPE_VERSION),
        );
        envelope.insert("itemKey".into(), json!(item.item_key));
        envelope.insert("class".into(), json!(delivery_class_str(item.class)));
        envelope.insert("aggregateClass".into(), json!(delivery_class_str(self.class)));
        envelope.insert("itemCount".into(), json!(self.items.len()));
        // `terminal` means "this item's class is a final outcome", which is what
        // guards `may_be_passed`. It is deliberately a different predicate from
        // "may enter the terminal stage": a `RepairNeeded` item is staged and
        // finalized exactly like the legacy repair round, yet it is not final.
        envelope.insert("terminal".into(), json!(item.is_final_class()));
        envelope.insert(
            "repairBudgetRemaining".into(),
            json!(self.repair_budget_remaining),
        );
        envelope.insert(
            "facts".into(),
            Value::Array(item.facts.iter().map(encode_fact).collect()),
        );
        Value::Object(envelope)
    }
}

/// Add the verdict envelope for `item` to `existing`, preserving its keys.
///
/// The free form exists so a caller building the bytes for **one** row — rather than
/// encoding a whole verdict at once — uses the same stamp the whole-verdict encoder
/// uses. Two stamping sites is how a row could acquire an envelope whose
/// `aggregateClass` or `itemCount` described a different fold.
pub(crate) fn stamp_envelope(
    verdict: &DeliveryVerdict,
    item: &ItemDecision,
    existing: &Value,
) -> Value {
    let mut object = existing.as_object().cloned().unwrap_or_default();
    object.insert(VERDICT_KEY.to_owned(), verdict.encode_envelope(item));
    Value::Object(object)
}

/// The durable form of a persisted verdict record.
///
/// The three cases exist so a version decision is never inferred from a single
/// upstream key: a record with no envelope keeps the legacy mapping, an explicit
/// and well-formed v2 envelope is read as v2, and anything else is `Corrupt` —
/// which can never be read as passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoredVerdict {
    V2(Box<StoredV2Verdict>),
    /// No verdict envelope: a v1 / historical record. Its own mapping applies.
    Legacy,
    /// An envelope is present but unusable: unknown version, wrong shape, bad
    /// class, truncated fact list. Explicitly not passed.
    Corrupt { detail: String },
}

/// A decoded, well-formed v2 verdict record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredV2Verdict {
    pub(crate) item_key: String,
    pub(crate) class: DeliveryClass,
    pub(crate) aggregate_class: DeliveryClass,
    pub(crate) item_count: usize,
    pub(crate) terminal: bool,
    pub(crate) repair_budget_remaining: bool,
    pub(crate) facts: Vec<DecisionFact>,
}

impl StoredVerdict {
    /// Whether this record may be read as a pass.
    ///
    /// Only an explicit, well-formed v2 envelope that says `confirmed` **and**
    /// terminal **and** whose own facts really support that may. A legacy record is
    /// not read here at all — it keeps its own mapping — and a corrupt record never
    /// passes.
    ///
    /// The declared class is not on its own evidence. The envelope is durable
    /// bytes, so a record can carry `class: "confirmed"` beside a `failed` fact,
    /// or beside no fact at all, without any writer having produced it: a
    /// truncated, mis-merged or hand-edited row would then read as a pass while
    /// its own facts say otherwise — the exact "Confirmed 配失败事实" shape. So the
    /// facts are re-folded and must reach `Confirmed` too. The fold is 2-B1's
    /// [`decide`], not a second aggregation, so the priority order cannot drift
    /// here; and an empty fact list folds to `Unconfirmed`, which is also the
    /// truth for a record that observed nothing.
    ///
    /// `class` is deliberately **not** compared with `aggregateClass`: they answer
    /// different questions. The aggregate folds every item of the delivery, so a
    /// `Confirmed` item legitimately sits inside a `Refused` or `RepairNeeded`
    /// aggregate, and this row is still a truthful pass — the item's own class
    /// governs the item's row. Requiring them to agree would turn a real pass into
    /// a non-pass and contradict the staged-round behaviour this consumer already
    /// guarantees.
    pub(crate) fn may_be_passed(&self) -> bool {
        match self {
            StoredVerdict::V2(stored) => {
                stored.class == DeliveryClass::Confirmed
                    && stored.terminal
                    && decide(&stored.facts, stored.repair_budget_remaining).class
                        == DeliveryClass::Confirmed
            }
            StoredVerdict::Legacy => false,
            StoredVerdict::Corrupt { .. } => false,
        }
    }

    /// The category a reload should report, for a caller that must show why a
    /// record is not a pass.
    pub(crate) fn category(&self) -> &'static str {
        match self {
            StoredVerdict::V2(_) => "v2",
            StoredVerdict::Legacy => "legacy",
            StoredVerdict::Corrupt { .. } => "corrupt",
        }
    }

    /// How this record governs whether it may be shown as a pass.
    ///
    /// The three-valued answer is the whole point, and it is the **one** rule both
    /// the writing and the reading path consult instead of each deciding for
    /// itself:
    ///
    /// * `None` — no v2 envelope at all. A legacy / historical record is not this
    ///   check's business: it keeps whatever status its own path gave it, so
    ///   historical v1 rows are never mass-downgraded by introducing v2.
    /// * `Some(true)` — a well-formed v2 record whose own facts really support the
    ///   declared pass.
    /// * `Some(false)` — a v2 record that must not be shown as passed: either a
    ///   well-formed envelope whose facts contradict it, or an envelope present but
    ///   undecodable (unknown version, wrong shape, bad class). The second case is
    ///   deliberately **not** "failed": an unusable record is *unconfirmable*, and
    ///   silently degrading it to a definite failure would invent a verdict no
    ///   checker produced.
    pub(crate) fn display_pass(&self) -> Option<bool> {
        match self {
            StoredVerdict::V2(_) => Some(self.may_be_passed()),
            StoredVerdict::Corrupt { .. } => Some(false),
            StoredVerdict::Legacy => None,
        }
    }
}

/// Whether these durable finding bytes may be shown as a pass.
///
/// `None` means the record carries no v2 envelope and keeps its own status; see
/// [`StoredVerdict::display_pass`]. Takes the **raw** column text rather than a
/// projected value on purpose: the display projection truncates the fact list
/// (`factsTruncated`), so judging the projected form could reject a legitimate
/// record or, worse, accept one whose complete facts were never examined. The
/// durable bytes are the only complete evidence there is.
pub(crate) fn display_pass_of_finding(raw: &str) -> Option<bool> {
    let finding: Value = serde_json::from_str(raw).ok()?;
    decode_verdict(&finding).display_pass()
}

/// The `item_key` the envelope on these durable bytes names, when one is present
/// and decodable.
///
/// `None` covers both "no envelope at all" (a legacy record) and "an envelope
/// that cannot be decoded" — the latter already never passes, so the row-key
/// comparison has nothing to add there. A `Some` is what lets a caller check that
/// the record it is about to trust is a record about **this** row: an envelope is
/// evidence for the item it names, and a durable row's key is the only thing that
/// says which item that is.
pub(crate) fn envelope_item_key_of_finding(raw: &str) -> Option<String> {
    let finding: Value = serde_json::from_str(raw).ok()?;
    match decode_verdict(&finding) {
        StoredVerdict::V2(stored) => Some(stored.item_key),
        StoredVerdict::Legacy | StoredVerdict::Corrupt { .. } => None,
    }
}

/// Whether a row written with `written` status must have its "passed" display
/// withheld, given its raw durable finding.
///
/// The finding-only special case of the shared rule in `p2b_contract_identity`;
/// the identity-aware entry point is `p2b_contract_identity::qualify_pass`, which
/// is what the checklist projection, the artifact view, the finalize write
/// defence and the progress summary all call. Kept as the answer for a caller
/// that has a finding but no contract column to read, and so the 2-B2 behaviour
/// stays independently assertable.
///
/// It is deliberately narrow:
///
/// * It only ever *withholds a pass*. A row that says `failed` is never touched —
///   the write side already decided that verdict, and rewriting it here would
///   erase a legitimate `Refused`/`Invalid`/`DefiniteFailure` (and `RepairNeeded`,
///   which is stored `failed` because this round's check did not pass while the
///   Run stays repairable). `Some(false)` means "may not be *displayed* as a
///   pass"; it is **not** an instruction to re-derive the row's status.
/// * It returns a plain predicate rather than a status string, because the
///   consumers speak different vocabularies (`pending` for the checklist,
///   `unverified` for the artifact view) and only the pass/no-pass question has
///   to agree. Neither view's DTO gains a second classification policy.
pub(crate) fn passed_display_is_withheld(written: &str, raw: Option<&str>) -> bool {
    written == "passed" && raw.and_then(display_pass_of_finding) == Some(false)
}

/// Decode a persisted finding into a typed verdict record.
pub(crate) fn decode_verdict(finding: &Value) -> StoredVerdict {
    let Some(object) = finding.as_object() else {
        return StoredVerdict::Legacy;
    };
    let Some(envelope) = object.get(VERDICT_KEY) else {
        return StoredVerdict::Legacy;
    };
    let Some(envelope) = envelope.as_object() else {
        return StoredVerdict::Corrupt {
            detail: "verdict envelope is not an object".to_owned(),
        };
    };
    let version = envelope
        .get("verdictSchemaVersion")
        .and_then(Value::as_u64);
    let envelope_version = envelope
        .get("verdictEnvelopeVersion")
        .and_then(Value::as_u64);
    match (version, envelope_version) {
        (Some(schema), Some(env))
            if schema == u64::from(VERDICT_SCHEMA_VERSION)
                && env == u64::from(VERDICT_ENVELOPE_VERSION) => {}
        (Some(schema), Some(env)) => {
            return StoredVerdict::Corrupt {
                detail: format!("unsupported verdict version {schema}/{env}"),
            };
        }
        _ => {
            return StoredVerdict::Corrupt {
                detail: "verdict version is missing or not a number".to_owned(),
            };
        }
    }
    let Some(item_key) = envelope.get("itemKey").and_then(Value::as_str) else {
        return StoredVerdict::Corrupt {
            detail: "verdict itemKey is missing".to_owned(),
        };
    };
    let Some(class) = envelope
        .get("class")
        .and_then(Value::as_str)
        .and_then(parse_delivery_class)
    else {
        return StoredVerdict::Corrupt {
            detail: "verdict class is missing or unknown".to_owned(),
        };
    };
    let Some(aggregate_class) = envelope
        .get("aggregateClass")
        .and_then(Value::as_str)
        .and_then(parse_delivery_class)
    else {
        return StoredVerdict::Corrupt {
            detail: "verdict aggregateClass is missing or unknown".to_owned(),
        };
    };
    let Some(item_count) = envelope.get("itemCount").and_then(Value::as_u64) else {
        return StoredVerdict::Corrupt {
            detail: "verdict itemCount is missing".to_owned(),
        };
    };
    let Some(terminal) = envelope.get("terminal").and_then(Value::as_bool) else {
        return StoredVerdict::Corrupt {
            detail: "verdict terminal flag is missing".to_owned(),
        };
    };
    let repair_budget_remaining = envelope
        .get("repairBudgetRemaining")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(facts) = envelope.get("facts").and_then(Value::as_array) else {
        return StoredVerdict::Corrupt {
            detail: "verdict facts are missing".to_owned(),
        };
    };
    let mut decoded = Vec::with_capacity(facts.len());
    for fact in facts {
        match decode_fact(fact) {
            Ok(fact) => decoded.push(fact),
            Err(detail) => return StoredVerdict::Corrupt { detail },
        }
    }
    StoredVerdict::V2(Box::new(StoredV2Verdict {
        item_key: item_key.to_owned(),
        class,
        aggregate_class,
        item_count: usize::try_from(item_count).unwrap_or(usize::MAX),
        terminal,
        repair_budget_remaining,
        facts: decoded,
    }))
}

// ---------------------------------------------------------------------------
// Class vocabulary
// ---------------------------------------------------------------------------

pub(crate) fn delivery_class_str(class: DeliveryClass) -> &'static str {
    class.as_str()
}

fn parse_delivery_class(value: &str) -> Option<DeliveryClass> {
    DeliveryClass::from_str(value)
}

fn fact_class_str(class: FactClass) -> &'static str {
    match class {
        FactClass::Passed => "passed",
        FactClass::Failed => "failed",
        FactClass::Unconfirmed => "unconfirmed",
        FactClass::Invalid => "invalid",
        FactClass::Refused => "refused",
    }
}

fn parse_fact_class(value: &str) -> Option<FactClass> {
    match value {
        "passed" => Some(FactClass::Passed),
        "failed" => Some(FactClass::Failed),
        "unconfirmed" => Some(FactClass::Unconfirmed),
        "invalid" => Some(FactClass::Invalid),
        "refused" => Some(FactClass::Refused),
        _ => None,
    }
}

fn limit_kind_str(kind: LimitKind) -> &'static str {
    kind.as_str()
}

fn parse_limit_kind(value: &str) -> Option<LimitKind> {
    match value {
        "bytes" => Some(LimitKind::Bytes),
        "textBytes" => Some(LimitKind::TextBytes),
        "records" => Some(LimitKind::Records),
        "fields" => Some(LimitKind::Fields),
        "cells" => Some(LimitKind::Cells),
        "lineBytes" => Some(LimitKind::LineBytes),
        "scanBytes" => Some(LimitKind::ScanBytes),
        "objects" => Some(LimitKind::Objects),
        "depth" => Some(LimitKind::Depth),
        "nodes" => Some(LimitKind::Nodes),
        "pages" => Some(LimitKind::Pages),
        "millis" => Some(LimitKind::Millis),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Fact encoding
// ---------------------------------------------------------------------------

/// Encode one fact with **every** retained field: class, origin and its binding,
/// rule/check/checker version, reason, limit and limit kind, structured
/// expected/actual/equivalence and the five-field locator.
///
/// Nothing is summarised away and nothing is re-derived from `detail` on read.
fn encode_fact(fact: &DecisionFact) -> Value {
    let mut map = Map::new();
    map.insert("class".into(), json!(fact_class_str(fact.class)));
    match &fact.origin {
        FactOrigin::Spec => {
            map.insert("origin".into(), json!("spec"));
        }
        FactOrigin::ProjectBinding => {
            map.insert("origin".into(), json!("projectBinding"));
        }
        FactOrigin::CheckUnbound => {
            map.insert("origin".into(), json!("checkUnbound"));
        }
        FactOrigin::Source { path, sha256 } => {
            map.insert("origin".into(), json!("source"));
            map.insert("path".into(), json!(path));
            map.insert("sha256".into(), json!(sha256));
        }
        FactOrigin::Candidate { path, sha256 } => {
            map.insert("origin".into(), json!("candidate"));
            map.insert("path".into(), json!(path));
            map.insert("sha256".into(), json!(sha256));
        }
    }
    if let Some(value) = &fact.rule_id {
        map.insert("ruleId".into(), json!(value));
    }
    if let Some(value) = &fact.check {
        map.insert("check".into(), json!(value));
    }
    if let Some(value) = fact.checker_version {
        map.insert("checkerVersion".into(), json!(value));
    }
    if let Some(value) = &fact.reason_code {
        map.insert("reasonCode".into(), json!(value));
    }
    if let Some(value) = &fact.detail {
        map.insert("detail".into(), json!(value));
    }
    map.insert("blocking".into(), json!(fact.blocking));
    map.insert("needsRepair".into(), json!(fact.needs_repair));
    if let Some(value) = fact.limit {
        map.insert("limit".into(), json!(value));
    }
    if let Some(kind) = fact.limit_kind {
        map.insert("limitKind".into(), json!(limit_kind_str(kind)));
    }
    if let Some(value) = &fact.expected {
        map.insert("expected".into(), json!(value));
    }
    if let Some(value) = &fact.actual {
        map.insert("actual".into(), json!(value));
    }
    if let Some(value) = &fact.equivalence {
        map.insert("equivalence".into(), json!(value));
    }
    if let Some(locator) = &fact.locator {
        // `RecordLocator` is already a bounded, serializable five-field shape.
        if let Ok(value) = serde_json::to_value(locator) {
            map.insert("locator".into(), value);
        }
    }
    Value::Object(map)
}

/// Decode one encoded fact. Every unknown or malformed value is an error: a
/// record this consumer cannot fully understand must not be silently read as a
/// weaker — or stronger — fact than it was.
fn decode_fact(value: &Value) -> Result<DecisionFact, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "fact is not an object".to_owned())?;
    let class = object
        .get("class")
        .and_then(Value::as_str)
        .and_then(parse_fact_class)
        .ok_or_else(|| "fact class is missing or unknown".to_owned())?;
    let origin_name = object
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(|| "fact origin is missing".to_owned())?;
    let text = |key: &str| -> Option<String> {
        object.get(key).and_then(Value::as_str).map(str::to_owned)
    };
    let origin = match origin_name {
        "spec" => FactOrigin::Spec,
        "projectBinding" => FactOrigin::ProjectBinding,
        "checkUnbound" => FactOrigin::CheckUnbound,
        "source" => FactOrigin::source(
            &text("path").ok_or_else(|| "source fact has no path".to_owned())?,
            &text("sha256").ok_or_else(|| "source fact has no sha256".to_owned())?,
        ),
        "candidate" => FactOrigin::candidate(
            &text("path").ok_or_else(|| "candidate fact has no path".to_owned())?,
            &text("sha256").ok_or_else(|| "candidate fact has no sha256".to_owned())?,
        ),
        other => return Err(format!("unknown fact origin {other}")),
    };
    let limit_kind = match object.get("limitKind") {
        None => None,
        Some(Value::Null) => None,
        Some(value) => {
            let name = value
                .as_str()
                .ok_or_else(|| "limitKind is not a string".to_owned())?;
            Some(parse_limit_kind(name).ok_or_else(|| format!("unknown limitKind {name}"))?)
        }
    };
    let locator = match object.get("locator") {
        None => None,
        Some(Value::Null) => None,
        Some(value) => Some(
            serde_json::from_value::<RecordLocator>(value.clone())
                .map_err(|error| format!("unreadable locator: {error}"))?,
        ),
    };
    let checker_version = match object.get("checkerVersion") {
        None => None,
        Some(Value::Null) => None,
        Some(value) => Some(
            u32::try_from(
                value
                    .as_u64()
                    .ok_or_else(|| "checkerVersion is not a number".to_owned())?,
            )
            .map_err(|_| "checkerVersion is out of range".to_owned())?,
        ),
    };
    let bool_field = |key: &str| object.get(key).and_then(Value::as_bool).unwrap_or(false);
    Ok(DecisionFact {
        class,
        origin,
        rule_id: text("ruleId"),
        check: text("check"),
        checker_version,
        reason_code: text("reasonCode"),
        detail: text("detail"),
        blocking: bool_field("blocking"),
        needs_repair: bool_field("needsRepair"),
        limit: match object.get("limit") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| "limit is not a number".to_owned())?,
            ),
        },
        limit_kind,
        expected: text("expected"),
        actual: text("actual"),
        equivalence: text("equivalence"),
        locator,
    })
}

/// Class vocabulary for the pending-retention path, which needs the class as a
/// bare token because it does not write a full verdict envelope.
pub(crate) fn parse_delivery_class_token(value: &str) -> Option<DeliveryClass> {
    parse_delivery_class(value)
}

// Test-only registration: nothing here is compiled into a production build.
#[cfg(test)]
#[path = "p2b_consumer_boundary_tests.rs"]
mod p2b_consumer_boundary_tests;
