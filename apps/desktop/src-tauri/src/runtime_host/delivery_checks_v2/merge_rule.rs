//! The `mergeRule` marker: which inter-entry separator a frozen `Concat`
//! contract is verified against.
//!
//! `schemaVersion` selects the **verification chain** (v1 byte comparison vs the
//! typed v2 chain). This marker is deliberately a separate axis: it selects the
//! **separator semantics** used inside whichever chain reads the record. The two
//! must not be conflated — a v1 record frozen after the D1 correction carries the
//! new separator under the old chain, so neither value can be derived from the
//! other.
//!
//! The marker is a top-level key of the persisted `ContentContract` spec, beside
//! `schemaVersion` and `rule`. Two consequences follow, and both are wanted:
//!
//! * No migration: `spec` is JSON text, and the existing decoders read only the
//!   keys they name.
//! * The marker is inside the frozen identity, so tampering with it after the
//!   fact is visible to the acceptance-time contract-drift check rather than
//!   being a silent change of meaning.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `spec` key that carries the marker.
pub(crate) const KEY: &str = "mergeRule";

/// The historical separator: a body that does not end with LF gets a terminator
/// LF before the separator's blank LF. A **0-byte** body counts as
/// unterminated, so adjacent empty entries are separated by two LF bytes (D1).
pub(crate) const SEPARATOR_V1: &str = "separator-v1";

/// The corrected separator: a 0-byte body contributes no line of its own and
/// therefore never earns a terminator LF; the separator supplies exactly one
/// blank line.
pub(crate) const SEPARATOR_V2: &str = "separator-v2";

/// Which separator a record is verified with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MergeRule {
    #[serde(rename = "separator-v1")]
    SeparatorV1,
    #[serde(rename = "separator-v2")]
    SeparatorV2,
}

impl Default for MergeRule {
    /// The meaning of a record that carries no marker: every contract frozen
    /// before the marker existed was written by the old writer.
    fn default() -> Self {
        Self::SeparatorV1
    }
}

impl MergeRule {
    /// The rule a freeze writes today.
    ///
    /// This is the single place the "current" choice is named, so a caller
    /// cannot write one literal and verify with another.
    pub(crate) fn current() -> Self {
        Self::SeparatorV2
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SeparatorV1 => SEPARATOR_V1,
            Self::SeparatorV2 => SEPARATOR_V2,
        }
    }

    /// True when the marker is the historical rule and may be omitted from a
    /// serialized envelope.
    pub(crate) fn is_legacy(self) -> bool {
        self == Self::SeparatorV1
    }

    /// Whether a body must gain one LF before the separator's blank line.
    ///
    /// The two rules differ **only** for a 0-byte body. That is the whole
    /// behavioural delta: no other shape can tell them apart, so nothing but the
    /// empty-body case can be affected by which rule a record carries.
    pub(crate) fn needs_terminator(self, body: &[u8]) -> bool {
        match self {
            // Historical: "does not end with LF" — an empty body does not.
            Self::SeparatorV1 => !body.ends_with(b"\n"),
            // Corrected: an empty body holds no line to terminate.
            Self::SeparatorV2 => !body.is_empty() && !body.ends_with(b"\n"),
        }
    }

    /// Separator LF bytes emitted after this body when it is not the last
    /// entry: two when the body's own line must first be terminated, one
    /// otherwise.
    pub(crate) fn separator_len(self, body: &[u8]) -> u64 {
        if self.needs_terminator(body) {
            2
        } else {
            1
        }
    }
}

/// Read the marker out of a persisted spec.
///
/// * **Absent** — a record frozen before the marker existed. Reads as
///   `separator-v1`, which is what it was actually written with.
/// * **A known literal** — that rule, regardless of `schemaVersion`.
/// * **Anything else** — an error. An explicit `null`, a non-string, an unknown
///   literal or a differently-cased spelling is never coerced into the legacy
///   rule: doing so would silently reinterpret a contract whose author declared
///   something this build does not understand.
pub(crate) fn from_spec(spec: &Value) -> Result<MergeRule, String> {
    let object = spec
        .as_object()
        .ok_or_else(|| "内容契约规格不是对象，无法读取分隔规则".to_owned())?;
    let Some(value) = object.get(KEY) else {
        return Ok(MergeRule::default());
    };
    let literal = value
        .as_str()
        .ok_or_else(|| format!("内容契约分隔规则 {KEY} 不是字符串"))?;
    match literal {
        SEPARATOR_V1 => Ok(MergeRule::SeparatorV1),
        SEPARATOR_V2 => Ok(MergeRule::SeparatorV2),
        other => Err(format!("内容契约分隔规则不受支持：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_absent_marker_reads_as_the_legacy_rule() {
        assert_eq!(
            from_spec(&json!({"schemaVersion": 1, "rule": {}})),
            Ok(MergeRule::SeparatorV1)
        );
        assert_eq!(MergeRule::default(), MergeRule::SeparatorV1);
    }

    #[test]
    fn both_literals_round_trip_through_a_spec() {
        for rule in [MergeRule::SeparatorV1, MergeRule::SeparatorV2] {
            let spec = json!({"schemaVersion": 1, "rule": {}, KEY: rule.as_str()});
            assert_eq!(from_spec(&spec), Ok(rule), "{} must round-trip", rule.as_str());
        }
    }

    /// The marker never falls back: an unusable value is refused here so it can
    /// reach the existing abnormal-contract path instead of being read as legacy.
    #[test]
    fn an_unusable_marker_is_refused_rather_than_read_as_legacy() {
        for bad in [
            json!(null),
            json!(""),
            json!("separator-V2"),
            json!(" separator-v2"),
            json!("separator-v3"),
            json!(2),
            json!(true),
            json!(["separator-v2"]),
            json!({"rule": "separator-v2"}),
        ] {
            let spec = json!({"schemaVersion": 1, "rule": {}, KEY: bad});
            assert!(
                from_spec(&spec).is_err(),
                "{} must be refused, not coerced",
                bad
            );
        }
    }

    #[test]
    fn a_non_object_spec_has_no_readable_marker() {
        assert!(from_spec(&json!("v1")).is_err());
        assert!(from_spec(&json!(null)).is_err());
    }

    /// The only behavioural difference between the two rules is the 0-byte body.
    #[test]
    fn the_rules_differ_only_for_an_empty_body() {
        for body in [
            &b""[..],
            b"\n",
            b"alpha",
            b"alpha\n",
            b"alpha\n\n",
            b"alpha\r\n",
            b"a\nb",
            b" ",
            b"\n\n",
        ] {
            let old = MergeRule::SeparatorV1.needs_terminator(body);
            let new = MergeRule::SeparatorV2.needs_terminator(body);
            if body.is_empty() {
                assert!(old, "legacy rule must terminate a 0-byte body");
                assert!(!new, "corrected rule must not terminate a 0-byte body");
            } else {
                assert_eq!(old, new, "{body:?} must be judged identically");
            }
        }
    }

    #[test]
    fn separator_length_counts_the_terminator_plus_the_blank_line() {
        assert_eq!(MergeRule::SeparatorV1.separator_len(b""), 2);
        assert_eq!(MergeRule::SeparatorV2.separator_len(b""), 1);
        assert_eq!(MergeRule::SeparatorV1.separator_len(b"alpha"), 2);
        assert_eq!(MergeRule::SeparatorV2.separator_len(b"alpha"), 2);
        assert_eq!(MergeRule::SeparatorV1.separator_len(b"alpha\n"), 1);
        assert_eq!(MergeRule::SeparatorV2.separator_len(b"alpha\n"), 1);
    }

    #[test]
    fn a_declared_marker_is_visible_even_when_the_value_is_unusable() {
        // The distinction the reader relies on: `Some(null)` is a *declaration*
        // this build cannot honour, while an absent key is a record that predates
        // the marker. Collapsing the two would reinterpret frozen contracts.
        assert!(from_spec(&json!({"schemaVersion": 1, KEY: null})).is_err());
        assert_eq!(from_spec(&json!({"schemaVersion": 1})), Ok(MergeRule::SeparatorV1));
    }

    /// The frozen literal strings are the persisted contract; they may not drift.
    #[test]
    fn the_persisted_literals_are_stable() {
        assert_eq!(KEY, "mergeRule");
        assert_eq!(MergeRule::SeparatorV1.as_str(), "separator-v1");
        assert_eq!(MergeRule::SeparatorV2.as_str(), "separator-v2");
        assert_eq!(MergeRule::current(), MergeRule::SeparatorV2);
    }
}
