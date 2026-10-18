//! Package 2-C1: the durable **contract identity** of a checklist row, and the
//! one pass-qualification rule every delivery consumer consults.
//!
//! # The gap this closes
//!
//! Before this module, a row's "may this be shown as a pass?" answer came from
//! the finding column alone ([`display_pass_of_finding`]): no decodable v2
//! envelope meant `None`, which meant "a legacy row, keep its stored status".
//! That is correct for a genuine legacy row and **wrong for a v2 row whose
//! envelope was lost** — the two are isomorphic in the finding column alone, so a
//! stripped v2 record kept reading as `passed`.
//!
//! The consumers did not agree about that same row either: the checklist
//! projection and the artifact view withheld the pass, while the terminal
//! model-failure progress summary still counted the item as delivered because it
//! asked the shallow question `status<>'passed'` in SQL.
//!
//! # The identity criterion (plan A)
//!
//! A checklist row is **v2 scope** exactly when its *own* frozen contract column
//! `requirements_json` carries a `ContentContract` whose `spec.schemaVersion`
//! selects the Host v2 contract. This deliberately mirrors the production
//! selector (`delivery_content_requirements::decode_spec` routes on
//! `spec["schemaVersion"]`; `apply` only evaluates `schemaVersion == 1`), so the
//! criterion cannot drift from the gate that actually runs.
//!
//! `spec.schemaVersion` is the **only** contract identity marker. A spec that
//! says `schemaVersion: 1` beside `envelopeVersion: 2` stays v1 with the extra
//! keys ignored — that is the frozen `delivery_checks_v2` reading, and it is
//! pinned by that module's own round-trip test. Identity is therefore *not*
//! guessed from a version-shaped substring; it is read through the real
//! `DeliveryRequirement` serde rules and the real [`decode_spec`].
//!
//! The two columns are independently durable, which is exactly what gives the
//! criterion its (limited, honestly bounded) discriminating power: the contract
//! column is written when the item is created or bound, the finding column when
//! the outcome is finalized, so a row that lost only its finding keeps its
//! identity.
//!
//! # What this cannot detect
//!
//! Stated plainly so no caller over-reads the result: a rewrite of *both* columns
//! together, a cross-row wholesale replacement, a deleted row, and arbitrary
//! corruption are all invisible here. Version/hash drift stays the artifact view's
//! own comparison.
//!
//! An envelope that names another item **is** detected (see [`RowBinding`]): the
//! record's own `itemKey` is compared with the ledger key of the row it was read
//! from, and a disagreement withholds the pass. Two limits stay: the comparison is
//! only made for a **decodable** envelope, because an unreadable one has no key to
//! compare and already fails on its own; and it is a check on the *pair* of columns
//! as they are read now, so a replacement that moved a whole record and its row key
//! together, or a rewrite of both columns into a self-consistent pair, is not
//! something a single-row reader can see.

use super::content_requirements::{DecodedSpec, decode_spec};
use super::display_pass_of_finding;
use super::p2b_consumer::envelope_item_key_of_finding;
use crate::database::{DeliveryRequirement, RequirementKind};

/// Which contract a checklist row's own frozen requirements select.
///
/// `Indeterminate` is not a synonym for "legacy": a row whose requirements exist
/// but cannot be read as the current strict shape — or that name a contract
/// version this build does not support — is **not** legal v1 either, and
/// [`qualify_pass`] treats a declared pass on such a row as unconfirmable rather
/// than silently falling back to the most permissive reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContractIdentity {
    /// No content contract at all, or contracts that decode as the frozen v1
    /// shape. A legal historical row: its own status stands.
    LegacyV1,
    /// At least one `ContentContract` on this row selects the Host v2 contract.
    /// Any one of them is enough: a row is in v2 scope if *any* of its
    /// requirements says so, never merely the first.
    HostV2,
    /// The requirements cannot be read, or name an unsupported contract version.
    Indeterminate(String),
}

/// Classify one row's `requirements_json`.
///
/// A parse failure is `Indeterminate`, never `LegacyV1`: the column is written by
/// `serde_json::to_string` over these very types, so a body that does not read
/// back is damaged rather than historical, and the conservative direction for an
/// unreadable contract is "cannot confirm", not "assume the permissive version".
///
/// An empty list — the column's own `DEFAULT '[]'`, and the shape of every plain
/// question that seeded no content demand — is a legal legacy row, not damage.
/// So is a column that is absent: `requirements_json` is `NOT NULL`, and the
/// absence of a `ContentContract` is what "no content contract" means.
pub(crate) fn classify_requirements(raw: Option<&str>) -> ContractIdentity {
    let Some(body) = raw else {
        return ContractIdentity::LegacyV1;
    };
    let requirements: Vec<DeliveryRequirement> = match serde_json::from_str(body) {
        Ok(requirements) => requirements,
        Err(_) => {
            return ContractIdentity::Indeterminate(
                "the frozen requirement column does not parse".into(),
            )
        }
    };
    // A readable v2 declaration wins over both v1 siblings and an unreadable one,
    // so one unrelated or damaged requirement cannot demote a row that really is
    // v2 scope.
    let mut indeterminate = None;
    for requirement in &requirements {
        let RequirementKind::ContentContract { spec } = &requirement.kind else {
            continue;
        };
        match decode_spec(spec) {
            DecodedSpec::V2(_) => return ContractIdentity::HostV2,
            DecodedSpec::V1(..) => {}
            DecodedSpec::Invalid(reason) => {
                indeterminate.get_or_insert(reason);
            }
        }
    }
    match indeterminate {
        Some(reason) => ContractIdentity::Indeterminate(reason),
        None => ContractIdentity::LegacyV1,
    }
}

/// What the shared rule decides about one row's declared pass.
///
/// The distinction between the last two variants is the whole point: a pass its
/// own facts contradict is a **definite** non-pass, while a pass whose record is
/// missing or unreadable is merely **unconfirmable**. Degrading the second into
/// the first would invent a verdict no checker produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PassQualification {
    /// Nothing to decide: no pass claim, or a legal legacy row. The status
    /// written by the row's own path stands.
    KeepStoredStatus,
    /// The row's own durable v2 facts support the declared pass.
    PassSupported,
    /// The row declares a pass and its own facts contradict it.
    ContradictedByFacts,
    /// The row declares a pass but the record it needs is absent or unusable, so
    /// the pass cannot stand. Never a definite failure.
    RecordUnusable(String),
}

impl PassQualification {
    /// Whether a declared pass must be withheld from display and from the
    /// delivered-count.
    pub(crate) fn withdraws_pass(&self) -> bool {
        matches!(
            self,
            PassQualification::ContradictedByFacts | PassQualification::RecordUnusable(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RowBinding {
    Matches,
    Unusable(String),
}

fn row_binding(row_item_key: &str, raw_finding: Option<&str>) -> RowBinding {
    let legal = |key: &str| !key.trim().is_empty() && key.len() <= 4096 && !key.chars().any(char::is_control);
    if !legal(row_item_key) {
        return RowBinding::Unusable("当前行 item_key 缺失或不合法，不能证明信封属于此行".into());
    }
    match raw_finding.and_then(envelope_item_key_of_finding) {
        Some(named) if legal(&named) && named == row_item_key => RowBinding::Matches,
        Some(named) => RowBinding::Unusable(format!("信封 itemKey {named:?} 不属于当前行 {row_item_key:?}")),
        None => RowBinding::Unusable("信封缺少可核验的 itemKey".into()),
    }
}

/// The **one** rule deciding whether a row's declared pass may stand, given the
/// row's own durable finding bytes and its independently read contract identity.
///
/// It only ever decides about a pass. A row written `pending` or `failed` always
/// short-circuits to [`PassQualification::KeepStoredStatus`], so a legitimate
/// `Refused` / `Invalid` / `DefiniteFailure` — and `RepairNeeded`, stored
/// `failed` because this round's check did not pass while the Run stays
/// repairable — can never be re-derived here. Because a row must already say
/// `passed` to reach the identity at all, the criterion can only ever *withhold*
/// a pass and can never promote one: it is a downgrade guard, not a
/// re-derivation.
///
/// Four answers, in the order they are decided:
///
/// 1. the row does not claim a pass → keep its status;
/// 2. the record's own bytes are readable and contradict the pass → withhold;
/// 3. the finding column carries a **decodable envelope** that names **another
///    item** → withhold: an envelope is evidence for the item it names, and this
///    row is not that item;
/// 4. a decodable envelope naming this row and supporting the pass → keep it;
/// 5. no decodable envelope → **the identity decides**: a v2-scoped row has lost
///    the record it is obliged to carry and its pass cannot stand, while a
///    non-v2 row is legal history and keeps the status it was written with.
///
/// Step 4 (the present, matching envelope) is deliberately ordered ahead of the
/// identity, and the reason is a production fact rather than a convenience: the
/// v2 contract path is unreachable from a delivery outcome until package 2-D, so
/// *every* v2 verdict record that exists today sits on a row whose own frozen
/// contract still reads v1 (or carries no `ContentContract` at all). A decodable
/// envelope identifies itself through its own version pair, which is exactly how
/// `decode_verdict` admits it. Had the identity been allowed to override a
/// present record, the criterion would have withdrawn every legitimate live v2
/// pass — a mass downgrade in the opposite direction from the one this module
/// exists to prevent.
///
/// The remaining disagreement — an envelope that claims v2 on a row whose contract
/// is legal v1 or absent — therefore has one explicit, conservative outcome, and
/// it is **not** a silent switch of identity: the present record governs, and the
/// identity is consulted only to detect an *absent* one. A row cannot escape a
/// contradicted envelope by carrying a v1 contract, and a row cannot be granted a
/// pass by its contract either. The case is pinned by
/// `c1_an_envelope_still_governs_a_row_whose_contract_reads_v1`.
pub(crate) fn qualify_pass(
    written: &str,
    raw_finding: Option<&str>,
    identity: &ContractIdentity,
    row_item_key: &str,
) -> PassQualification {
    if written != "passed" {
        return PassQualification::KeepStoredStatus;
    }
    let display = raw_finding.and_then(display_pass_of_finding);
    if display.is_some() {
        if let RowBinding::Unusable(reason) = row_binding(row_item_key, raw_finding) {
            return PassQualification::RecordUnusable(reason);
        }
    }
    match display {
        Some(true) => PassQualification::PassSupported,
        Some(false) => PassQualification::ContradictedByFacts,
        None => match identity {
            ContractIdentity::HostV2 => PassQualification::RecordUnusable(
                "the v2 record's verdict envelope is absent from its own row".into(),
            ),
            // A row whose contract column cannot be read is not legal v1 either,
            // so a declared pass on it cannot be confirmed. This is the card's
            // stated boundary: damage must not fall back to the most permissive
            // reading, and it must not be reported as a definite failure.
            ContractIdentity::Indeterminate(reason) => {
                PassQualification::RecordUnusable(reason.clone())
            }
            ContractIdentity::LegacyV1 => PassQualification::KeepStoredStatus,
        },
    }
}

/// The entry point for every path that holds both raw durable columns and only
/// asks the pass/no-pass question. Callers use this rather than composing
/// [`classify_requirements`] and [`qualify_pass`] themselves, so no consumer can
/// pair a contract identity with a different row's finding.
///
/// `row_item_key` is the ledger key of the row these two columns were read from.
/// It is what makes the record's own `itemKey` checkable, so one item's verdict
/// can never be spent on another item's row.
pub(crate) fn passed_qualification_of_row(
    written: &str,
    raw_finding: Option<&str>,
    raw_requirements: Option<&str>,
    row_item_key: &str,
) -> PassQualification {
    qualify_pass(
        written,
        raw_finding,
        &classify_requirements(raw_requirements),
        row_item_key,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The frozen `Contract` payload tag is `type` (see `Contract`'s serde
    /// attribute), which is a different key from `RequirementKind`'s `kind`.
    fn unbound_rule() -> serde_json::Value {
        json!({"type": "unbound", "reason": "未绑定"})
    }

    fn content_contract(spec: serde_json::Value) -> String {
        serde_json::to_string(&vec![DeliveryRequirement {
            item_key: Some("file:out.json".into()),
            id: "content:concat".into(),
            source_text: "冻结内容契约".into(),
            kind: RequirementKind::ContentContract { spec },
        }])
        .unwrap()
    }

    #[test]
    fn an_empty_or_absent_contract_column_is_a_legal_legacy_row() {
        assert_eq!(classify_requirements(Some("[]")), ContractIdentity::LegacyV1);
        assert_eq!(classify_requirements(None), ContractIdentity::LegacyV1);
    }

    #[test]
    fn the_selector_is_spec_schema_version_and_nothing_else() {
        // The frozen two-marker disagreement: schemaVersion 1 beside
        // envelopeVersion 2 stays v1, exactly as `decode_spec` routes it.
        let v1_with_v2_markers = json!({
            "schemaVersion": 1,
            "rule": unbound_rule(),
            "envelopeVersion": 2,
            "checkerVersion": 2,
            "ruleId": "content:concat",
        });
        assert_eq!(
            classify_requirements(Some(&content_contract(v1_with_v2_markers))),
            ContractIdentity::LegacyV1
        );
        // Declaring v2 while the rest of the envelope is illegal is not a v2
        // contract: the strict decode refuses it, so the row is not legal v1
        // either and a declared pass on it cannot be confirmed.
        assert!(matches!(
            classify_requirements(Some(&content_contract(json!({"schemaVersion": 2})))),
            ContractIdentity::Indeterminate(_)
        ));
    }

    #[test]
    fn any_one_v2_requirement_puts_the_whole_row_in_v2_scope() {
        let legacy = DeliveryRequirement {
            item_key: Some("file:out.json".into()),
            id: "content:old".into(),
            source_text: "旧契约".into(),
            kind: RequirementKind::ContentContract {
                spec: json!({"schemaVersion": 1, "rule": unbound_rule()}),
            },
        };
        let v2 = DeliveryRequirement {
            item_key: Some("file:out.json".into()),
            id: "content:new".into(),
            source_text: "新契约".into(),
            kind: RequirementKind::ContentContract {
                spec: json!({
                    "schemaVersion": 2,
                    "envelopeVersion": 2,
                    "checkerVersion": 2,
                    "rule": unbound_rule(),
                    "policy": {},
                }),
            },
        };
        // v2 last among several: not merely the first requirement is consulted.
        let body = serde_json::to_string(&vec![legacy.clone(), v2.clone()]).unwrap();
        assert_eq!(classify_requirements(Some(&body)), ContractIdentity::HostV2);
        let body = serde_json::to_string(&vec![v2, legacy]).unwrap();
        assert_eq!(classify_requirements(Some(&body)), ContractIdentity::HostV2);
    }

    #[test]
    fn a_damaged_requirement_column_is_never_treated_as_legal_v1() {
        assert!(matches!(
            classify_requirements(Some("{not json")),
            ContractIdentity::Indeterminate(_)
        ));
        let qualification = passed_qualification_of_row(
            "passed",
            Some(r#"{"boundPath":"out.json"}"#),
            Some("{not json"),
            "file:out.json",
        );
        assert_eq!(
            qualification,
            PassQualification::RecordUnusable(
                "the frozen requirement column does not parse".into()
            )
        );
        assert!(qualification.withdraws_pass());
    }

    #[test]
    fn a_stripped_v2_row_cannot_keep_its_pass_while_a_legacy_row_keeps_its_status() {
        let identity = ContractIdentity::HostV2;
        assert!(qualify_pass("passed", None, &identity, "file:out.json").withdraws_pass());
        assert!(qualify_pass("passed", Some("{}"), &identity, "file:out.json").withdraws_pass());
        assert!(qualify_pass("passed", Some("null"), &identity, "file:out.json").withdraws_pass());
        assert_eq!(
            qualify_pass("passed", None, &ContractIdentity::LegacyV1, "file:out.json"),
            PassQualification::KeepStoredStatus
        );
    }

    #[test]
    fn only_a_pass_is_ever_decided() {
        for written in ["pending", "failed", "verified", "unverified"] {
            assert_eq!(
                qualify_pass(written, None, &ContractIdentity::HostV2, "file:out.json"),
                PassQualification::KeepStoredStatus
            );
        }
    }

    /// The explicit conservative outcome for the one disagreement the card asks to
    /// be defined rather than left implicit: a finding that carries a v2 envelope
    /// on a row whose own contract reads v1 (or is absent). This is the **live
    /// production shape** until package 2-D, so the present record governs and the
    /// identity is not allowed to override it.
    #[test]
    fn c1_an_envelope_still_governs_a_row_whose_contract_reads_v1() {
        let contradicted = json!({
            "boundPath": "out/one.json",
            "managedVersionId": "v1",
            "deliveryVerdict": {
                "verdictSchemaVersion": 2,
                "verdictEnvelopeVersion": 2,
                "itemKey": "file:out.json",
                "class": "confirmed",
                "aggregateClass": "confirmed",
                "itemCount": 1,
                "terminal": true,
                "repairBudgetRemaining": true,
                "facts": [{
                    "class": "failed", "origin": "candidate", "path": "out/one.json",
                    "sha256": "a".repeat(64), "ruleId": "content",
                    "reasonCode": "sheet_names_mismatch", "blocking": true, "needsRepair": true
                }]
            }
        })
        .to_string();
        let v1_contract = content_contract(json!({"schemaVersion": 1, "rule": unbound_rule()}));
        for contract in [Some("[]"), None, Some(v1_contract.as_str())] {
            assert_eq!(
                passed_qualification_of_row(
                    "passed",
                    Some(&contradicted),
                    contract,
                    "file:out.json"
                ),
                PassQualification::ContradictedByFacts,
                "a present record must govern regardless of the contract column"
            );
        }
    }

    /// The row-level binding: an envelope is evidence for the item it names.
    #[test]
    fn b_an_envelope_naming_another_item_cannot_support_this_rows_pass() {
        let sound = || {
            json!({
                "boundPath": "out/one.json",
                "deliveryVerdict": {
                    "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                    "itemKey": "file:other.json",
                    "class": "confirmed", "aggregateClass": "confirmed",
                    "itemCount": 1, "terminal": true, "repairBudgetRemaining": true,
                    "facts": [{"class": "passed", "origin": "checkUnbound",
                               "ruleId": "content:0", "blocking": false, "needsRepair": false}]
                }
            })
            .to_string()
        };
        let qualification = passed_qualification_of_row(
            "passed",
            Some(&sound()),
            Some("[]"),
            "file:out.json",
        );
        assert!(
            qualification.withdraws_pass(),
            "another item's envelope must not support this row's pass: {qualification:?}"
        );
        assert!(
            matches!(qualification, PassQualification::RecordUnusable(_)),
            "a foreign record is unconfirmable, never a definite failure: {qualification:?}"
        );

        // The same record under its own key is a supported pass, so the test
        // cannot be satisfied by refusing every envelope.
        assert_eq!(
            passed_qualification_of_row("passed", Some(&sound()), Some("[]"), "file:other.json"),
            PassQualification::PassSupported
        );
    }

    /// A row key the caller could not supply is a missing input, not a mismatch.
    #[test]
    fn b_an_unavailable_row_key_is_unconfirmable() {
        let sound = json!({
            "deliveryVerdict": {
                "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                "itemKey": "file:out.json",
                "class": "confirmed", "aggregateClass": "confirmed",
                "itemCount": 1, "terminal": true, "repairBudgetRemaining": true,
                "facts": [{"class": "passed", "origin": "checkUnbound",
                           "ruleId": "content:0", "blocking": false, "needsRepair": false}]
            }
        })
        .to_string();
        for key in ["", " ", "file:out\n.json"] {
            assert!(matches!(passed_qualification_of_row("passed", Some(&sound), Some("[]"), key),
                PassQualification::RecordUnusable(_)));
        }
        for value in [serde_json::Value::Null, json!(""), json!(7), json!("file:foreign.json")] {
            let mut damaged: serde_json::Value = serde_json::from_str(&sound).unwrap();
            damaged["deliveryVerdict"]["itemKey"] = value;
            assert!(matches!(passed_qualification_of_row("passed", Some(&damaged.to_string()), Some("[]"), "file:out.json"),
                PassQualification::RecordUnusable(_)));
        }
    }
}
