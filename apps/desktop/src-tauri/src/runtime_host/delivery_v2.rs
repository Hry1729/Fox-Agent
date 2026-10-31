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

use crate::runtime_host::delivery_checks_v2::merge_rule;

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
        assert!(matches!(decode_spec(&v1), DecodedSpec::V1(..)));
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
        let persisted = crate::canonical_json::to_string(&requirement).expect("the requirement serializes");
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

