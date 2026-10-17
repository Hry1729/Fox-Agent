//! Package 2-E: the production enablement boundary for v2 content contracts.
//!
//! This module is the **one** place that decides whether this process may write
//! a v2 content contract, and the **one** place that builds its envelope. It is
//! deliberately small: the classification policy, the checker leaf, the
//! consumer and the version router stay in the modules that already own them,
//! and nothing here re-derives any of them.
//!
//! It lives beside `freeze` rather than at the crate root because the write site
//! it governs is `add` in the parent module, and the rule it inspects is the
//! parent's own `Contract`. Keeping the gate and its single writer in one module
//! is what makes "there is exactly one place that can produce a v2 record" a
//! property of the code layout rather than a convention.
//!
//! # Why a process switch rather than a stored setting
//!
//! The card that authorizes this wiring requires the gate to be **off on a
//! normal start** and reachable only from an explicitly enabled isolated
//! process. A process environment variable gives exactly that and nothing more:
//!
//! * it cannot be turned on by a stored preference, a migration, an imported
//!   project or a continuation, so no user instance can acquire it by accident;
//! * it is read at the moment of the decision and never cached, so a restart
//!   returns to the default with no stale state to reconcile;
//! * it is not inherited into another Run's *decision* — a Run is judged by the
//!   process that judges it.
//!
//! # Fail-closed on an unusable value
//!
//! A missing, empty or clearly-false value is `Off`. A clearly-true value is
//! `On`. **Anything else is `Invalid`, which is treated exactly as `Off`** — a
//! typo must not enable the gate. It is a separate state rather than a bare
//! `false` so the reason a gate stayed closed is reportable instead of being
//! laundered into the same answer as a deliberate "off".

use super::*;

/// The process-scoped enablement switch for v2 content contracts.
pub(super) const ENABLE_VARIABLE: &str = "FOX_DELIVERY_V2";

/// The resolved state of the enablement switch.
///
/// `pub(crate)` rather than `pub(super)` because the switch matrix is asserted
/// from `delivery`'s own test module, which is a sibling of the module that owns
/// this gate — the alternative was a second copy of the vocabulary there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum V2Enablement {
    /// Explicitly enabled for this process.
    On,
    /// Absent or explicitly disabled. The default state of every normal start.
    Off,
    /// Present but not a value this gate understands. Behaves exactly as `Off`;
    /// the value is retained so a closed gate can say why.
    Invalid(String),
}

impl V2Enablement {
    /// Whether this state permits writing a v2 contract. Only `On` does.
    pub(crate) fn permits_v2(&self) -> bool {
        matches!(self, V2Enablement::On)
    }

    /// A bounded description for a finding, a log or an acceptance record.
    pub(super) fn detail(&self) -> String {
        match self {
            V2Enablement::On => "本进程已显式启用 v2 内容契约".to_owned(),
            V2Enablement::Off => "本进程未启用 v2 内容契约（默认关闭）".to_owned(),
            V2Enablement::Invalid(value) => {
                format!("v2 启用变量取值 {value:?} 无法识别，按关闭处理")
            }
        }
    }
}

/// Resolve the switch from the process environment.
///
/// Called at the moment of a decision rather than cached at startup: a value set
/// for an isolated acceptance process is therefore honoured without a restart,
/// and no state outlives the process that read it.
pub(super) fn enablement() -> V2Enablement {
    #[cfg(test)]
    if let Some(state) = test_override() {
        return state;
    }
    enablement_from_environment()
}

fn enablement_from_environment() -> V2Enablement {
    match std::env::var(ENABLE_VARIABLE) {
        Err(std::env::VarError::NotPresent) => V2Enablement::Off,
        Err(std::env::VarError::NotUnicode(_)) => {
            V2Enablement::Invalid("<非 Unicode 取值>".to_owned())
        }
        Ok(value) => classify(&value),
    }
}

/// A per-thread override of the resolved switch, for tests only.
///
/// The production switch is process-scoped, so a test that had to drive it through
/// `std::env` would be mutating state every other concurrently running test shares
/// — and a test that happened to freeze an authorized rule during that window would
/// quietly produce a v2 record. A thread-local override cannot leak: cargo runs one
/// test per thread, so a fixture that sets it is the only reader that sees it, and
/// no environment variable is touched at all.
///
/// Production never sets it, so `enablement` falls through to the real environment
/// in every non-test build.
#[cfg(test)]
thread_local! {
    static TEST_ENABLEMENT: std::cell::RefCell<Option<V2Enablement>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn test_override() -> Option<V2Enablement> {
    TEST_ENABLEMENT.with(|slot| slot.borrow().clone())
}

/// Run `body` with this thread's switch resolved to `state`, then restore it.
///
/// Restores rather than clears, so a nested scope in one test cannot silently
/// re-enable the gate for the outer one.
#[cfg(test)]
pub(crate) fn with_test_enablement<T>(state: V2Enablement, body: impl FnOnce() -> T) -> T {
    let previous = TEST_ENABLEMENT.with(|slot| slot.replace(Some(state)));
    let outcome = body();
    TEST_ENABLEMENT.with(|slot| *slot.borrow_mut() = previous);
    outcome
}

/// Classify one raw switch value.
///
/// Split out so the accepted vocabulary is testable without mutating the process
/// environment, which is global state shared with every other test in the crate.
pub(crate) fn classify(value: &str) -> V2Enablement {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => V2Enablement::On,
        "" | "0" | "false" | "off" | "no" => V2Enablement::Off,
        other => V2Enablement::Invalid(other.chars().take(64).collect()),
    }
}

/// Whether this wiring may produce a **v2** contract for one rule.
///
/// The first release is deliberately narrower than "v2 is on". Only the `concat`
/// rule has the complete, real, end-to-end verification the card requires: a
/// frozen source identity, the bounded typed reads, the real `text_concat` leaf,
/// a proven coverage obligation, and an accepted consumer/terminal path. Every
/// other contract keeps writing the v1 record it writes today, so enabling the
/// gate cannot claim a capability that has not been accepted.
///
/// The allowlist is keyed off the **decoded rule variant**, not off a
/// name-shaped match on persisted text, so a newly added variant is excluded
/// until it is deliberately listed.
pub(super) fn allows_v2(contract: &Contract) -> bool {
    matches!(contract, Contract::Concat { .. })
}

/// The envelope and checker generation this wiring writes.
///
/// Named here rather than inlined so the write site and its tests cannot
/// disagree about them. They are deliberately **not** the rule-payload version,
/// which stays 1: `schemaVersion` is the only contract identity marker, and the
/// rule payload remains the same `Contract` family v1 uses.
pub(super) const ENVELOPE_VERSION: u32 = 2;
pub(super) const CHECKER_VERSION: u32 = 2;
pub(super) const CONTRACT_SCHEMA_VERSION: u32 = 2;

/// Build the v2 spec for one already-frozen rule.
///
/// The `rule` payload is the **same** value the v1 writer would persist, moved
/// under the envelope unchanged. That is what lets a v2 row be decoded by the
/// existing strict `Contract` reader and executed by the existing leaf: v2 adds
/// the envelope that identifies the generation, it does not fork the rule.
///
/// `rule_id` names the requirement this contract came from, so the durable audit
/// a later reader produces can be tied back to the row it belongs to.
///
/// The rule is passed in already serialized rather than serialized here: the v1
/// branch of the same write site needs the identical value, and serializing it
/// twice is how the two generations could come to hold different rules.
///
/// # The `policy` key is the producer's write obligation (package 2-E R-2)
///
/// `RuleEnvelope::policy` is `#[serde(default)]`, so an omitted `policy` is
/// *readable* as `RulePolicy::default()`. That is the reader's tolerance, not the
/// writer's licence: `A-P1-host-plan-revision-result.md` §3.4 stage B requires a
/// newly frozen v2 contract to persist the full policy explicitly. Omitting it
/// made the equivalence branch in `text_csv::policy_grants_equivalence`
/// unreachable "by omission" — a coincidence — instead of unreachable because
/// `BytesExact` is *declared*, which is assertable.
///
/// The value written is the real `RulePolicy::default()` serialized through
/// `serde`, never a hand-written literal: a hand-written copy would drift from
/// the type the moment a field is added, and this key is exactly where drift
/// would silently change what a frozen contract means. The comparison semantics
/// are unchanged — `ComparisonMode::BytesExact` is already what a frozen v1
/// `Concat` means, so this writes down the current default rather than choosing
/// a new one.
pub(crate) fn spec_for(rule: Value, rule_id: &str) -> Value {
    let policy = serde_json::to_value(crate::runtime_host::delivery_checks_v2::RulePolicy::default())
        .unwrap_or_else(|_| json!({}));
    json!({
        "schemaVersion": CONTRACT_SCHEMA_VERSION,
        "envelopeVersion": ENVELOPE_VERSION,
        "checkerVersion": CHECKER_VERSION,
        "ruleId": rule_id,
        "policy": policy,
        "rule": rule,
    })
}

/// The spec one contract is frozen as, given an already-resolved switch state.
///
/// This is the **whole** generation decision, and the only one: `add` calls it
/// with the process switch, and a test calls it with each state in turn. Keeping
/// it pure is what lets the matrix be checked without mutating the process
/// environment, which every other test in this crate shares — and it is why
/// "enabling the gate changes the envelope and not the rule" is a statement about
/// one function rather than about two branches that have to be kept in step.
///
/// Both envelope shapes are built here so the two generations cannot come to hold
/// different rule payloads.
pub(crate) fn spec_for_contract(
    contract: &Contract,
    rule: Value,
    rule_id: &str,
    enablement: &V2Enablement,
) -> Value {
    if enablement.permits_v2() && allows_v2(contract) {
        spec_for(rule, rule_id)
    } else {
        json!({"schemaVersion": 1, "rule": rule})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            path: "a.txt".into(),
            sha256: "a".repeat(64),
            bytes: 1,
            first_line: 1,
            last_line: 1,
        }
    }

    fn concat() -> Contract {
        Contract::Concat {
            sources: vec![source()],
            selected: vec!["a.txt".into()],
            selection_order: "bytes_ascending".into(),
            tie_order: "filename_ascending".into(),
            merge_order: "filename_ascending".into(),
            read_only: false,
        }
    }

    #[test]
    fn only_an_explicit_true_value_permits_v2() {
        for value in ["1", "true", "TRUE", "on", "yes", " on "] {
            assert_eq!(classify(value), V2Enablement::On, "{value:?} must enable");
            assert!(classify(value).permits_v2());
        }
    }

    #[test]
    fn an_absent_or_explicitly_false_value_stays_closed() {
        for value in ["", "0", "false", "off", "no", "OFF"] {
            assert_eq!(
                classify(value),
                V2Enablement::Off,
                "{value:?} must stay closed"
            );
            assert!(!classify(value).permits_v2());
        }
    }

    /// A typo must not enable the gate, and it must not be laundered into the
    /// same state as a deliberate "off" either.
    #[test]
    fn an_unrecognised_value_is_invalid_and_behaves_as_off() {
        for value in ["2", "enabled", "tru", "y", "ope"] {
            let state = classify(value);
            assert!(
                matches!(state, V2Enablement::Invalid(_)),
                "{value:?} must be Invalid"
            );
            assert!(!state.permits_v2(), "{value:?} must not enable the gate");
        }
        assert_eq!(classify("enabled"), V2Enablement::Invalid("enabled".into()));
    }

    #[test]
    fn the_allowlist_covers_concat_only() {
        assert!(allows_v2(&concat()));
        assert!(!allows_v2(&Contract::Unbound { reason: "x".into() }));
        assert!(!allows_v2(&Contract::FactQualifiers {
            sources: vec![source()]
        }));
        assert!(!allows_v2(&Contract::DataSource {
            source: source(),
            sheet: None,
            headers: vec!["a".into()],
            date_required: false,
            metrics: vec![],
        }));
    }

    /// The envelope's rule payload must be identical to what the v1 writer would
    /// have stored, so the existing strict reader and the frozen leaf accept it.
    #[test]
    fn the_rule_payload_is_the_untouched_v1_rule() {
        let contract = concat();
        let rule = serde_json::to_value(&contract).unwrap();
        let spec = spec_for(rule.clone(), "content:concat");
        assert_eq!(spec["schemaVersion"], json!(2));
        assert_eq!(spec["envelopeVersion"], json!(2));
        assert_eq!(spec["checkerVersion"], json!(2));
        assert_eq!(spec["ruleId"], json!("content:concat"));
        assert_eq!(spec["rule"], rule, "the rule payload must be the untouched v1 rule");
        // The rule payload itself carries no schema version: the envelope's own
        // `schemaVersion` is the only contract identity marker.
        assert!(spec["rule"].get("schemaVersion").is_none());
        assert!(matches!(decode_spec(&spec), DecodedSpec::V2(_)));
    }

    /// The gate is the *only* difference between the two producers: the same
    /// contract must yield a decodable spec of the expected generation in each
    /// state, so enabling the gate cannot change the rule that is verified.
    #[test]
    fn enabling_the_gate_changes_the_envelope_and_not_the_rule() {
        let contract = concat();
        let rule = serde_json::to_value(&contract).unwrap();
        let v2 = spec_for(rule.clone(), "content:concat");
        let v1 = json!({"schemaVersion": 1, "rule": rule});
        assert_eq!(v1["rule"], v2["rule"]);
        assert!(matches!(decode_spec(&v1), DecodedSpec::V1(_)));
        assert!(matches!(decode_spec(&v2), DecodedSpec::V2(_)));
    }

    // -----------------------------------------------------------------------
    // Package 2-E R-2: the producer's `policy` write obligation.
    //
    // Reading a v2 record with no `policy` is legal (`RuleEnvelope::policy` is
    // `#[serde(default)]`), but that is the reader's tolerance. The producer must
    // still *write* the policy explicitly, so what a frozen contract means is a
    // fact in the record rather than a property of whatever reader opens it.
    // -----------------------------------------------------------------------

    /// The producer writes the policy key, and the value it writes is the real
    /// `RulePolicy::default()` — serialized through serde, not a hand-written
    /// literal that could drift from the type.
    #[test]
    fn the_producer_writes_the_current_default_policy_explicitly() {
        let contract = concat();
        let rule = serde_json::to_value(&contract).unwrap();
        let spec = spec_for(rule, "content:concat");

        let written = spec.get("policy").unwrap_or_else(|| {
            panic!("a newly frozen v2 contract must persist its policy: {spec}")
        });
        let expected = serde_json::to_value(
            crate::runtime_host::delivery_checks_v2::RulePolicy::default(),
        )
        .unwrap();
        assert_eq!(
            written, &expected,
            "the written policy must equal the current default, not a hand-written copy"
        );
        // The default is the strict reading of a v1 artifact, so writing it down
        // changes no comparison semantics.
        assert_eq!(expected["comparison"], json!("bytesExact"));
        assert_eq!(expected["eof"], json!("byteExact"));
    }

    /// A v2 record that omits `policy` and one that states the default explicitly
    /// must decode to the *same* policy — which is what makes the R-2 write
    /// additive rather than a behaviour change for records already frozen.
    #[test]
    fn an_omitted_policy_and_an_explicit_default_are_the_same_policy() {
        let contract = concat();
        let rule = serde_json::to_value(&contract).unwrap();
        let explicit = spec_for(rule.clone(), "content:concat");

        let mut omitted = explicit.clone();
        omitted.as_object_mut().unwrap().remove("policy");

        let explicit_policy = match decode_spec(&explicit) {
            DecodedSpec::V2(decoded) => decoded.envelope.policy,
            other => panic!("explicit policy must decode as v2: {other:?}"),
        };
        let omitted_policy = match decode_spec(&omitted) {
            DecodedSpec::V2(decoded) => decoded.envelope.policy,
            other => panic!("omitted policy must still decode as v2: {other:?}"),
        };
        assert_eq!(
            explicit_policy, omitted_policy,
            "an omitted policy and the explicit default must be the same policy"
        );
        assert_eq!(
            explicit_policy,
            super::super::V2Policy::default(),
            "an omitted policy and the explicit default must be the same policy"
        );
    }

    /// The written policy survives the real durable shape: the spec is what lands
    /// in `delivery_checklist_items.requirements_json` inside
    /// `RequirementKind::ContentContract { spec }`, so the producer's obligation
    /// is durable and not just a property of the in-memory value.
    #[test]
    fn the_written_policy_survives_the_durable_round_trip() {
        use crate::database::RequirementKind;

        let contract = concat();
        let rule = serde_json::to_value(&contract).unwrap();
        let spec = spec_for(rule, "content:concat");

        // The real durable shape, through the real requirement type.
        let requirement = RequirementKind::ContentContract { spec: spec.clone() };
        let persisted = serde_json::to_string(&requirement).expect("the requirement serializes");
        let reloaded: RequirementKind =
            serde_json::from_str(&persisted).expect("the durable shape reloads");
        let reloaded_spec = match reloaded {
            RequirementKind::ContentContract { spec } => spec,
            other => panic!("unexpected requirement kind: {other:?}"),
        };

        // The reloaded spec still carries the policy the producer wrote.
        assert_eq!(
            reloaded_spec.get("policy"),
            Some(
                &serde_json::to_value(
                    crate::runtime_host::delivery_checks_v2::RulePolicy::default()
                )
                .unwrap()
            ),
            "the policy must survive the durable round-trip"
        );
        let policy = match decode_spec(&reloaded_spec) {
            DecodedSpec::V2(decoded) => decoded.envelope.policy,
            other => panic!("a reloaded v2 contract must decode as v2: {other:?}"),
        };
        assert_eq!(
            policy,
            super::super::V2Policy::default(),
            "the decoded policy must be the strict default after the round-trip"
        );
    }
}



