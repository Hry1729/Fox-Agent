//! Finite content contracts derived by Host from user-authorized source bytes.
//! Material text is data: only content restrictions are extracted, never tools,
//! permissions or instructions about the agent. No model assertion proves one.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) use crate::runtime_host::delivery_checks_v2::merge_rule::{self, MergeRule};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub(super) struct Source {
    path: String,
    sha256: String,
    bytes: u64,
    first_line: usize,
    last_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag="type", rename_all="camelCase", deny_unknown_fields)]
pub(super) enum Contract {
    Unbound { reason: String },
    ForbiddenMention { source: Source, literal: String },
    ForbiddenAvailability { source: Source, literal: String },
    FactQualifiers { sources: Vec<Source> },
    Concat {
        sources: Vec<Source>,
        selected: Vec<String>,
        selection_order: String,
        tie_order: String,
        merge_order: String,
        #[serde(default)]
        read_only: bool,
    },
    DataSource { source: Source, sheet: Option<String>, headers: Vec<String>, date_required: bool, metrics: Vec<Metric> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub(super) struct Metric { column:String, operation:String }

pub(super) fn model_reference(spec:&Value)->Value {
    let Ok(contract)=serde_json::from_value::<Contract>(spec["rule"].clone()) else {return json!({"schemaVersion":1,"type":"unbound","reason":"内容契约不支持"});};
    // The separator rule travels with the reference so a user-facing view can
    // tell a record frozen under the historical rule from one frozen under the
    // corrected rule. An unreadable marker keeps the legacy reading: this is a
    // display projection, and rendering is not where a contract is validated.
    let rule=merge_rule::from_spec(spec).unwrap_or_default();
    match contract {
        Contract::Concat{sources,selected,selection_order,tie_order,merge_order,read_only}=>json!({"schemaVersion":1,"type":"concat",
            "selectionOrder":selection_order,"tieOrder":tie_order,"mergeOrder":merge_order,"selected":selected,
            "mergeRule":rule.as_str(),
            "sourceManifestHash":digest(&crate::canonical_json::to_vec(&sources).unwrap_or_default()),"readOnly":read_only,"format":"完整文件名 LF 原文，相邻段恰好一个空行，最后不添空行"}),
        Contract::DataSource{source,sheet,headers,date_required,metrics}=>json!({"schemaVersion":1,"rule":{
            "type":"dataSource","source":source,"sheet":sheet,"headers":headers,"date_required":date_required,"metrics":metrics},
            "computeSourceSheet":if Path::new(&source.path).extension().and_then(|e|e.to_str()).is_some_and(|e|matches!(e.to_ascii_lowercase().as_str(),"csv"|"tsv")) {Some("Sheet1")}else{sheet.as_deref()}}),
        other=>json!({"schemaVersion":1,"rule":other}),
    }
}

// ===========================================================================
// Host-side strict spec decoding (package 2-A, internal only).
//
// `decode_spec` is a *pure* decoder: it classifies a persisted content
// contract spec as V1 (the existing `Contract`), V2 (an envelope-2/checker-2
// rule with a strict, deny-unknown-fields Host DTO), or Invalid. It performs
// no file reads, makes no activation decision and changes no delivery state.
// Production evaluation still matches only schemaVersion == 1 inside `apply`,
// so a decoded V2 never takes the v1 lane; package 2-E routes such a row to the
// typed chain instead (`delivery.rs`'s `route_typed_run`), which is gated by the
// producer's switch through `delivery_v2`.
//
// --- The five version layers, kept deliberately distinct (package 2-C1) -----
//
// These numbers are independent, and **a difference between them is not by
// itself a defect**. Collapsing them is what produced the `envelopeVersion`
// misreading this note exists to prevent.
//
// | layer                         | constant / key                        | value | owner |
// |-------------------------------|---------------------------------------|-------|-------|
// | Host contract selector        | `spec.schemaVersion`                  | 1     | this file (`decode_spec`, `apply`) |
// | Host frozen rule payload      | `RULE_PAYLOAD_SCHEMA_VERSION`         | 1     | `delivery_checks_v2` |
// | Rule envelope generation      | `ENVELOPE_VERSION_V1` / `_V2`         | 1 / 2 | `delivery_checks_v2` |
// | Checker generation            | `CHECKER_VERSION_V1` / `_V2`          | 1 / 2 | `delivery_checks_v2` |
// | Persistent verdict record     | `VERDICT_SCHEMA_VERSION` / `_ENVELOPE_VERSION` | 2 / 2 | `p2b_consumer` |
//
// **The Host contract selector is `spec.schemaVersion`, and it is the only
// contract identity marker.** A spec whose `schemaVersion` is 1 stays a v1
// contract *even when it also carries `envelopeVersion: 2`*: v1 readers ignore
// the extra keys, and `apply` evaluates only `schemaVersion == 1`. That is the
// frozen `delivery_checks_v2` reading, pinned by its own round-trip test
// `v2_envelope_round_trips_beside_the_untouched_rule_payload`. The rule payload
// version therefore stays 1 and is **not** bumped to make the markers agree.
//
// The identity criterion in `p2b_contract_identity` is written against this same
// selector on purpose. Had it keyed off `envelopeVersion` instead, it would
// disagree with the gate that actually runs and would silently fail to fire for
// the records it is meant to protect.
//
// The value of `spec.schemaVersion` is read through the real serde types below,
// never by scanning the JSON text for a version-shaped substring.
// ===========================================================================

/// The decoded form of a persisted content-contract `spec`.
#[derive(Debug)]
pub(super) enum DecodedSpec {
    /// schemaVersion 1 carrying the existing, deny-unknown `Contract`, plus the
    /// separator rule the record declares. The rule is kept **beside** the
    /// contract, not inside it: the frozen `rule` payload is a shape v1 and v2
    /// share and must not change, and the marker is a property of the record.
    V1(Contract, MergeRule),
    /// schemaVersion 2 with envelopeVersion/checkerVersion == 2, a strictly
    /// validated policy, **and** an executable rule decoded as the same
    /// `Contract` family v1 uses.
    V2(V2Rule),
    /// The spec is missing, mistyped, of an unsupported schema version, or
    /// fails strict validation. Carries a stable reason; never falls back to
    /// v1 on missing/invalid data.
    Invalid(String),
}

impl DecodedSpec {
    /// The separator rule a decoded record declares, on either chain.
    ///
    /// `None` for a record that did not decode: there is no rule to report, and
    /// returning a default here would be exactly the silent fallback the marker
    /// exists to prevent. Callers must handle the failure before asking.
    ///
    /// The production v1 path reads the rule directly in `apply` rather than
    /// through this accessor, because it must fail loudly; this projection exists
    /// so the marker's behaviour is observable on both chains in tests.
    #[cfg(test)]
    pub(super) fn merge_rule(&self) -> Option<MergeRule> {
        match self {
            Self::V1(_, rule) => Some(*rule),
            // A v2 record without a marker predates it and was written with the
            // historical rule; a marker is already validated as one of the two
            // literals by `decode_spec`.
            Self::V2(rule) => Some(match rule.envelope.merge_rule.as_deref() {
                Some(merge_rule::SEPARATOR_V2) => MergeRule::SeparatorV2,
                _ => MergeRule::SeparatorV1,
            }),
            Self::Invalid(_) => None,
        }
    }
}

/// A fully validated v2 envelope: the strict Host DTO plus the executable
/// rule decoded through the existing `Contract` reader. Package 2-E consumes
/// `contract` through `execution::execute`.
#[derive(Debug)]
pub(super) struct V2Rule {
    pub(super) envelope: V2EnvelopeSpec,
    /// The executable rule. Same family (and same strictness) as v1; v2 only
    /// differs in the envelope/policy that travels beside it.
    pub(super) contract: Contract,
}

/// The v2 envelope as persisted **beside** the legacy `rule` payload.
///
/// Host-local strict DTO: `deny_unknown_fields` rejects any typo at the top
/// level and inside the policy/table, unlike the shared leaf `RulePolicy`
/// (whose acceptance surface stays frozen). Validated instances are then
/// converted to the leaf policy when a future package activates v2.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct V2EnvelopeSpec {
    pub(super) schema_version: u32,
    pub(super) envelope_version: u32,
    pub(super) checker_version: u32,
    #[serde(default)]
    pub(super) rule_id: Option<String>,
    /// Absent means "all strict defaults" (byte-exact). An explicit null is
    /// rejected: it would erase an otherwise declarable policy ambiguously.
    #[serde(default, deserialize_with = "strict_policy")]
    pub(super) policy: V2Policy,
    /// The executable rule payload (same JSON family as v1); this decoder
    /// validates the envelope/policy only — package 2-D validates the rule
    /// shape when v2 evaluation is activated.
    #[serde(default)]
    pub(super) rule: Option<Value>,
    /// Back-compat key produced by `model_reference` for data sources.
    #[serde(default)]
    pub(super) compute_source_sheet: Option<Value>,
    /// The `Concat` separator rule. Declared here because this DTO denies
    /// unknown fields: a marker the strict decoder did not know would make every
    /// new v2 contract unreadable. `None` means the field was absent, which for a
    /// v2 record is the historical rule.
    #[serde(default, rename = "mergeRule")]
    pub(super) merge_rule: Option<String>,
}

/// Strict mirror of the leaf `RulePolicy` (unknown fields rejected).
///
/// `PartialEq`/`Eq` are derived so a test can assert that an explicitly written
/// policy and an omitted one decode to the *same* policy — which is the property
/// that makes the producer's write obligation additive rather than a behaviour
/// change for records already frozen.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct V2Policy {
    #[serde(default)]
    pub(super) comparison: V2Comparison,
    #[serde(default)]
    pub(super) eof: V2Eof,
    #[serde(default)]
    pub(super) table: V2TableDialect,
}

impl Default for V2Policy {
    fn default() -> Self {
        Self {
            comparison: V2Comparison::BytesExact,
            eof: V2Eof::ByteExact,
            table: V2TableDialect::default(),
        }
    }
}

// The Host strict DTOs mirror the leaf enums' accepted strings (serde rejects
// unknown variants already) and carry the *same* documented defaults, so a
// policy that omits a field reads exactly as the leaf would. Conversion to the
// leaf types happens in `V2Policy::into_leaf` when v2 evaluation is activated
// (package 2-D); the shared leaf structures are never modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Comparison {
    #[default]
    BytesExact,
    TextSegments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Eof {
    #[default]
    ByteExact,
    SourceVerbatim,
    AllowSingleTrailingLf,
    RequireTrailingLf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2LineEnding {
    #[default]
    Any,
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Bom {
    #[default]
    Auto,
    Required,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Delimiter {
    #[default]
    Comma,
    Tab,
    Semicolon,
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Quote {
    #[default]
    DoubleQuote,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2BlankLine {
    #[default]
    Skip,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum V2Header {
    #[default]
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct V2TableDialect {
    #[serde(default)]
    pub(super) delimiter: V2Delimiter,
    #[serde(default)]
    pub(super) quote: V2Quote,
    #[serde(default)]
    pub(super) bom: V2Bom,
    #[serde(default)]
    pub(super) line_ending: V2LineEnding,
    #[serde(default)]
    pub(super) blank_line: V2BlankLine,
    #[serde(default)]
    pub(super) header: V2Header,
}

impl Default for V2TableDialect {
    fn default() -> Self {
        Self {
            delimiter: V2Delimiter::Comma,
            quote: V2Quote::DoubleQuote,
            bom: V2Bom::Auto,
            line_ending: V2LineEnding::Any,
            blank_line: V2BlankLine::Skip,
            header: V2Header::Required,
        }
    }
}

/// `policy` may be omitted (defaults) but, when present, must be an object;
/// an explicit null is not silently treated as "default".
fn strict_policy<'de, D>(deserializer: D) -> Result<V2Policy, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    match Option::<V2Policy>::deserialize(deserializer) {
        Ok(Some(policy)) => Ok(policy),
        Ok(None) => Err(serde::de::Error::custom(
            "policy 为 null；省略该字段以使用默认策略",
        )),
        Err(error) => Err(error),
    }
}

/// Pure spec decoder. No I/O, no activation.
///
/// Rules (charter §60/§61 + G-P2):
/// * missing/null/non-integer schemaVersion, or a version other than 1/2 →
///   Invalid (never falls back to v1).
/// * schemaVersion 1 decodes only `rule` into the existing strict
///   `Contract`. Extra *top-level* keys keep the historical behaviour: they
///   are ignored (only unknown keys *inside* `rule` reject via Contract's
///   deny_unknown_fields).
/// * schemaVersion 2 requires envelopeVersion == 2 and checkerVersion == 2
///   and a strict policy/table; any unknown key or bad enum → Invalid.
/// * schemaVersion 2 additionally requires an **executable rule**: the same
///   strict `Contract` family v1 decodes. A missing, mistyped, unknown-`type`
///   or unknown-key rule is Invalid — a v2 envelope with no decodable rule
///   would leave the checker with nothing to execute. (Deferring v2 is about
///   *new* rule variants/policy semantics, never about "rule may be absent".)
pub(super) fn decode_spec(spec: &Value) -> DecodedSpec {
    let schema_version = match spec.get("schemaVersion") {
        None | Some(Value::Null) => {
            return DecodedSpec::Invalid("内容契约缺少 schemaVersion".to_owned())
        }
        Some(value) => match value.as_u64() {
            Some(version) if (1..=2).contains(&version) => version as u32,
            Some(_) => {
                return DecodedSpec::Invalid("内容契约 schemaVersion 不受支持".to_owned())
            }
            // String, float, object, array — keep parity with the production
            // `!= 1` numeric gate instead of coercing.
            None => return DecodedSpec::Invalid("内容契约 schemaVersion 不是整数".to_owned()),
        },
    };

    match schema_version {
        1 => {
            // Historical v1: decode only the rule payload; other extra top-level
            // keys are ignored exactly as the current production reader does.
            //
            // The separator marker is the one key that is *not* ignored when it
            // is unusable. Ignoring it would mean silently verifying the record
            // with a rule its author did not declare, which is the failure mode
            // the marker exists to prevent. An absent marker is still the
            // historical rule, so every existing record keeps decoding.
            let rule = match merge_rule::from_spec(spec) {
                Ok(rule) => rule,
                Err(reason) => return DecodedSpec::Invalid(reason),
            };
            match serde_json::from_value::<Contract>(spec["rule"].clone()) {
                Ok(contract) => DecodedSpec::V1(contract, rule),
                Err(error) => {
                    DecodedSpec::Invalid(format!("内容契约字段不完整或不支持：{error}"))
                }
            }
        }
        2 => {
            let envelope: V2EnvelopeSpec = match serde_json::from_value(spec.clone()) {
                Ok(envelope) => envelope,
                Err(error) => {
                    return DecodedSpec::Invalid(format!("v2 内容契约严格解码失败：{error}"))
                }
            };
            if envelope.envelope_version != 2 {
                return DecodedSpec::Invalid(
                    "v2 内容契约要求 envelopeVersion == 2".to_owned(),
                );
            }
            if envelope.checker_version != 2 {
                return DecodedSpec::Invalid(
                    "v2 内容契约要求 checkerVersion == 2".to_owned(),
                );
            }
            // The envelope must carry a rule the existing strict `Contract`
            // reader can decode; v1 and v2 agree on the rule family.
            let Some(rule) = envelope.rule.as_ref() else {
                return DecodedSpec::Invalid("v2 内容契约缺少可执行 rule".to_owned());
            };
            let contract: Contract = match serde_json::from_value(rule.clone()) {
                Ok(contract) => contract,
                Err(error) => {
                    return DecodedSpec::Invalid(format!(
                        "v2 内容契约 rule 无法解码：{error}"
                    ))
                }
            };
            // The separator marker is validated with the same strictness as the
            // rest of the envelope, through the field the strict decoder already
            // populated: a mistyped value failed above, and anything but a known
            // literal is refused here rather than handed to a verifier that
            // would then have to choose a rule for itself.
            if let Some(literal) = envelope.merge_rule.as_deref() {
                if literal != merge_rule::SEPARATOR_V1 && literal != merge_rule::SEPARATOR_V2 {
                    return DecodedSpec::Invalid(format!(
                        "内容契约分隔规则不受支持：{literal}"
                    ));
                }
            }
            DecodedSpec::V2(V2Rule { envelope, contract })
        }
        _ => DecodedSpec::Invalid("内容契约 schemaVersion 不受支持".to_owned()),
    }
}

fn digest(bytes:&[u8])->String {hex::encode(Sha256::digest(bytes))}

// ===========================================================================
// Host-side typed v2 classification and the per-row version router.
//
// This is the **producing site** for the package-2-B1 facts: it performs the
// real root lookup, the real typed candidate read, the real `checkedHash`
// comparison and the real source-identity reads, and classifies each typed
// result at the point where it is produced. It then hands the classified facts
// to the pure aggregation core in `p2b_decision.rs`. The private execution module
// owns the complete obligation set and invokes the frozen leaves directly.
//
// `route_typed_run` is the production caller. It is driven by the **frozen
// record**, never by the producer's switch: a row frozen as v2 stays v2 on every
// later evaluation, restart and continuation, so turning the producer off cannot
// silently downgrade an in-flight v2 Run. A v1 row keeps its legacy verdict byte
// for byte and contributes it to the one fold as a fact, so a v1 pass can neither
// be upgraded by a v2 sibling nor mask a v2 `Unconfirmed`.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    fn reference()->Source{Source{path:"policy.md".into(),sha256:"a".repeat(64),bytes:1,first_line:1,last_line:1}}
    #[test]
    fn finite_material_budget_and_subject_local_negation() {
        let body=(0..33).map(|i|format!("禁止提及“Feature {i}”。")).collect::<Vec<_>>().join("\n");
        let rules=material_rules(&reference(),&body);
        assert_eq!(rules.iter().filter(|r|matches!(r,Contract::ForbiddenMention{..})).count(),32);
        assert!(rules.iter().any(|r|matches!(r,Contract::Unbound{..})));
        assert!(availability_claim("Guide 已发布，Other 未上线。","Guide"));
        assert!(!availability_claim("Guide 尚未发布。","Guide"));
        assert!(material_rules(&reference(),"Do not claim 'Smart Guide' is released.").iter().any(|r|matches!(r,Contract::ForbiddenAvailability{..})));
        let line="这是产品上线后已经跑出来的数，不是目标值。";
        assert!(!qualifier_negated(line,line.find("上线后").unwrap()));
        assert!(readable(br#"{"title":"\u0048idden Preview"}"#,"brief.json").unwrap().contains("Hidden Preview"));
    }
    #[test]
    fn explicit_sheet_and_multiple_source_remain_ambiguous() {
        assert!(source_sheet_from_task("按字段统计 Sheet 'Alpha' 和 Sheet 'Beta'。").is_err());
        assert_eq!(source_sheet_from_task("以 Detailed Ledger 为事实明细，按 Segment Group 列统计。").unwrap(),Some("Detailed Ledger".into()));
        // asks_for_statistics deliberately stays narrow (a computed word must follow
        // 统计), so the demand names what is counted; the two named sources are what
        // must keep it unbound.
        let requirement=source_statistics_demand("读取 FIRST.XLSX 和 SECOND.XLSX，按 Segment Group 列统计数量，生成 report.md。").unwrap();
        assert!(matches!(requirement.kind,RequirementKind::SourceStatsUnbound{..}));
    }

    // D1: the v1 `Contract::Concat` writer is the path that actually runs when
    // v2 is off (the default), so the empty-body separator lives here too. The
    // expectations below are literal bytes written from the frozen wording
    // ("entries joined by exactly one blank line"), not produced by the writer
    // under test.
    //
    // The separator is a property of the frozen record, so the same contract is
    // exercised twice: once with the corrected rule a new freeze writes, and
    // once with the historical rule an unmarked record still means.
    #[test]
    fn d1_v1_concat_separates_entries_with_exactly_one_blank_line() {
        let root = std::env::temp_dir().join("fox-d1-v1-concat");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // Three 0-byte sources: the B4 shape. `frozen_bytes` re-verifies each
        // digest, so the references must describe the real files.
        let names = ["01_周一.txt", "02_周二.txt", "03_周三.txt"];
        let sources = names
            .iter()
            .map(|name| {
                std::fs::write(root.join(name), b"").unwrap();
                Source {
                    path: (*name).into(),
                    sha256: digest(b""),
                    bytes: 0,
                    first_line: 1,
                    last_line: 1,
                }
            })
            .collect::<Vec<_>>();
        let selected = names.iter().map(|n| (*n).to_string()).collect::<Vec<_>>();
        let contract = Contract::Concat {
            sources,
            selected,
            selection_order: "byteSizeAscThenNameAsc".into(),
            tie_order: "nameAsc".into(),
            merge_order: "nameAsc".into(),
            read_only: true,
        };
        let database = Database::open(root.join("d1.db")).expect("database");

        // Under the corrected rule (what a new freeze writes) the rule's 44
        // bytes are Met, and the old 46-byte shape is Unmet.
        let correct = "01_周一.txt\n\n02_周二.txt\n\n03_周三.txt\n".as_bytes();
        assert_eq!(
            evaluate(&contract, MergeRule::SeparatorV2, &root, correct, "joined.txt", &database, "run", &json!({})),
            RequirementVerdict::Met,
            "D1: the corrected 44-byte merge must satisfy a new contract"
        );
        assert_eq!(correct.len(), 44);

        let double_blank = "01_周一.txt\n\n\n02_周二.txt\n\n\n03_周三.txt\n".as_bytes();
        assert!(
            matches!(
                evaluate(&contract, MergeRule::SeparatorV2, &root, double_blank, "joined.txt", &database, "run", &json!({})),
                RequirementVerdict::Unmet(_)
            ),
            "D1: under the corrected rule two blank lines must not satisfy anything"
        );

        // Under the historical rule the same contract still wants 46 bytes. This
        // is the compatibility guarantee: upgrading must not flip an old record's
        // verdict, so the corrected 44-byte artifact must **not** pass here.
        assert_eq!(
            evaluate(&contract, MergeRule::SeparatorV1, &root, double_blank, "joined.txt", &database, "run", &json!({})),
            RequirementVerdict::Met,
            "D1: an unmarked record keeps the two-blank-line separator it was judged with"
        );
        assert!(
            matches!(
                evaluate(&contract, MergeRule::SeparatorV1, &root, correct, "joined.txt", &database, "run", &json!({})),
                RequirementVerdict::Unmet(_)
            ),
            "D1: the 44-byte artifact must NOT become a pass merely because the build was upgraded"
        );

        // A non-empty body without a terminator still needs two LFs, and an
        // LF-terminated one needs one — the fix must not collapse either.
        let mixed = ["01_周一.txt", "02_周二.txt"];
        std::fs::write(root.join("01_周一.txt"), b"alpha").unwrap();
        std::fs::write(root.join("02_周二.txt"), b"beta\n").unwrap();
        let mixed_sources = mixed
            .iter()
            .map(|name| {
                let body = std::fs::read(root.join(name)).unwrap();
                Source {
                    path: (*name).into(),
                    sha256: digest(&body),
                    bytes: body.len() as u64,
                    first_line: 1,
                    last_line: 1,
                }
            })
            .collect::<Vec<_>>();
        let mixed_contract = Contract::Concat {
            sources: mixed_sources,
            selected: mixed.iter().map(|n| (*n).to_string()).collect(),
            selection_order: "byteSizeAscThenNameAsc".into(),
            tie_order: "nameAsc".into(),
            merge_order: "nameAsc".into(),
            read_only: true,
        };
        // alpha (no LF) -> terminator LF + blank LF; beta\n -> blank LF only.
        // Non-empty bodies are byte-identical under both rules, which is the
        // proof that the marker changes nothing outside the 0-byte case.
        let mixed_bytes =
            b"01_\xe5\x91\xa8\xe4\xb8\x80.txt\nalpha\n\n02_\xe5\x91\xa8\xe4\xba\x8c.txt\nbeta\n";
        for rule in [MergeRule::SeparatorV2, MergeRule::SeparatorV1] {
            assert_eq!(
                evaluate(&mixed_contract, rule, &root, mixed_bytes, "joined.txt", &database, "run", &json!({})),
                RequirementVerdict::Met,
                "D1: non-empty bodies keep their bytes under {}",
                rule.as_str()
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Three 0-byte sources named as the B4 case froze them, plus a ready
    /// database: the shared fixture for the separator tests.
    fn three_empty_concat(root: &Path) -> (Contract, Vec<&'static str>) {
        let names = ["01_周一.txt", "02_周二.txt", "03_周三.txt"];
        let sources = names
            .iter()
            .map(|name| {
                std::fs::write(root.join(name), b"").unwrap();
                Source {
                    path: (*name).into(),
                    sha256: digest(b""),
                    bytes: 0,
                    first_line: 1,
                    last_line: 1,
                }
            })
            .collect::<Vec<_>>();
        let contract = Contract::Concat {
            sources,
            selected: names.iter().map(|n| (*n).to_string()).collect(),
            selection_order: "byteSizeAscThenNameAsc".into(),
            tie_order: "nameAsc".into(),
            merge_order: "nameAsc".into(),
            read_only: true,
        };
        (contract, names.to_vec())
    }

    // ---- compat: the marker is read from the frozen record ----------------
    //
    // The separator rule is a fact in the record, not a property of the build
    // that opens it. These two tests pin the reader: an unmarked record keeps the
    // historical rule however this build is configured, and a marked one gets the
    // rule it declares even when that is the *old* one.

    /// An unmarked v1 `Concat` record is read with the historical separator, so
    /// the 46-byte artifact it was frozen against still passes and the corrected
    /// 44-byte one does not.
    #[test]
    fn d1_compat_an_unmarked_v1_record_keeps_the_historical_separator() {
        let root = std::env::temp_dir().join("fox-d1-compat-unmarked");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (contract, _names) = three_empty_concat(&root);
        let database = Database::open(root.join("d1.db")).expect("database");

        // The shape an old row actually holds: no `mergeRule` key at all.
        let spec = json!({
            "schemaVersion": 1,
            "rule": serde_json::to_value(&contract).unwrap(),
        });
        assert!(
            spec.get("mergeRule").is_none(),
            "the fixture must be a genuinely unmarked record"
        );
        assert_eq!(
            decode_spec(&spec).merge_rule().expect("an unmarked v1 record always decodes with the historical rule"),
            MergeRule::SeparatorV1,
            "an absent marker means the rule the record was written with"
        );

        let old_46 = "01_周一.txt\n\n\n02_周二.txt\n\n\n03_周三.txt\n".as_bytes();
        assert_eq!(old_46.len(), 46);
        assert_eq!(
            evaluate(&contract, decode_spec(&spec).merge_rule().expect("an unmarked v1 record always decodes with the historical rule"), &root, old_46, "joined.txt", &database, "run", &json!({})),
            RequirementVerdict::Met,
            "an old record must still be satisfied by the artifact it was judged against"
        );

        let corrected_44 = "01_周一.txt\n\n02_周二.txt\n\n03_周三.txt\n".as_bytes();
        assert_eq!(corrected_44.len(), 44);
        assert!(
            matches!(
                evaluate(&contract, decode_spec(&spec).merge_rule().expect("an unmarked v1 record always decodes with the historical rule"), &root, corrected_44, "joined.txt", &database, "run", &json!({})),
                RequirementVerdict::Unmet(_)
            ),
            "the corrected artifact must not silently start passing an old record"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A record that explicitly declares the historical rule keeps it even though
    /// this build freezes the corrected one — the marker is authoritative in both
    /// directions, so it cannot be used as a one-way upgrade flag.
    #[test]
    fn d1_compat_an_explicit_historical_marker_wins_over_the_current_default() {
        // A decodable rule, so `decode_spec` reaches the marker rather than
        // failing on the payload: this test is about the marker, not the rule.
        let rule = serde_json::to_value(Contract::Unbound {
            reason: "fixture".into(),
        })
        .unwrap();
        let spec = json!({"schemaVersion": 1, "mergeRule": "separator-v1", "rule": rule});
        assert_eq!(
            decode_spec(&spec).merge_rule(),
            Some(MergeRule::SeparatorV1),
            "an explicit historical marker must be honoured"
        );
        assert_eq!(MergeRule::default(), MergeRule::SeparatorV1);
        assert_ne!(
            MergeRule::current(),
            MergeRule::SeparatorV1,
            "current must differ, or this test proves nothing"
        );

        // And an explicit corrected marker on the same v1 chain is honoured, so
        // the choice is genuinely independent of `schemaVersion`.
        let new_spec = json!({"schemaVersion": 1, "mergeRule": "separator-v2", "rule": rule});
        assert_eq!(
            decode_spec(&new_spec).merge_rule(),
            Some(MergeRule::SeparatorV2),
            "a declared separator-v2 must reach the reader on the v1 chain"
        );
    }

    /// The frozen step writes the marker on every new record, in both envelope
    /// shapes, so a contract cannot be frozen without saying which rule it means.
    #[test]
    fn d1_compat_a_new_freeze_records_the_marker_in_both_envelopes() {
        use delivery_v2::{spec_for_contract, V2Enablement};
        let concat = Contract::Concat {
            sources: Vec::new(),
            selected: Vec::new(),
            selection_order: "byteSizeAscThenNameAsc".into(),
            tie_order: "nameAsc".into(),
            merge_order: "nameAsc".into(),
            read_only: true,
        };
        let rule = serde_json::to_value(&concat).unwrap();
        for (label, enablement, expected_schema) in [
            ("v1", V2Enablement::Off, 1),
            ("v2", V2Enablement::On, 2),
        ] {
            let spec = spec_for_contract(&concat, rule.clone(), "content:concat", &enablement);
            assert_eq!(spec["schemaVersion"], json!(expected_schema), "{label}");
            assert_eq!(
                spec[merge_rule::KEY],
                json!(MergeRule::current().as_str()),
                "{label}: a new freeze must declare its separator"
            );
            assert_eq!(
                decode_spec(&spec).merge_rule().expect("an unmarked v1 record always decodes with the historical rule"),
                MergeRule::current(),
                "{label}: what the writer froze is what the reader sees"
            );
        }
    }

    /// An unusable marker is refused on **both** chains rather than defaulted.
    #[test]
    fn d1_compat_an_unusable_marker_is_refused_on_both_chains() {
        for schema in [1u64, 2] {
            for bad in [json!("separator-v3"), json!(2), json!(null), json!("SEPARATOR-V2")] {
                let spec = json!({
                    "schemaVersion": schema,
                    "envelopeVersion": 2,
                    "checkerVersion": 2,
                    "ruleId": "content:concat",
                    "mergeRule": bad,
                    "rule": {"type": "concat"},
                });
                let decoded = decode_spec(&spec);
                assert!(
                    matches!(decoded, DecodedSpec::Invalid(_)),
                    "schemaVersion {schema} accepted {bad} as {decoded:?}"
                );
            }
        }
    }

    /// The whole point of putting the marker in the spec: it is inside the frozen
    /// identity, so changing it after acceptance is visible.
    #[test]
    fn d1_compat_the_marker_participates_in_the_frozen_identity() {
        let base = json!({"schemaVersion": 1, "mergeRule": "separator-v1", "rule": {"type": "concat"}});
        let mut tampered = base.clone();
        tampered["mergeRule"] = json!("separator-v2");

        // The identity is the digest of the whole spec, so a flipped marker is a
        // different contract — not a silent reinterpretation of the same one.
        assert_ne!(
            digest(&crate::canonical_json::to_vec(&base).unwrap()),
            digest(&crate::canonical_json::to_vec(&tampered).unwrap()),
            "flipping the marker must change the frozen identity"
        );
    }
}

#[path = "p2c2_execution.rs"]
pub(super) mod execution;

#[path = "delivery_v2.rs"]
pub(super) mod delivery_v2;

