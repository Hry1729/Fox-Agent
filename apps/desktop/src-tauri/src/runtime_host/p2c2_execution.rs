//! Owns one controlled verification. Executable receipts never leave this module.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::super::p2b_decision::{self as decision, DecisionFact, FactContext, FactOrigin};
use super::*;
use crate::runtime_host::attachment_compute::{
    authorized_delivery_root_typed, read_authorized_delivery_bytes_typed, DeliveryReadFailure,
};
use crate::runtime_host::delivery_checks_v2::{
    self as leaf, CandidateBinding, CheckLimits, CheckOutcome, CheckState, RuleEnvelope,
    TableCheckRequest, TextConcatRequest, TextSegment,
};
use leaf::text_csv::ParseInput;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Obligation {
    id: String,
    check: String,
    discharged: bool,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Coverage {
    obligations: Option<Vec<Obligation>>,
}

impl Coverage {
    /// Test fixture: a contract whose completeness was never stated.
    #[cfg(test)]
    #[allow(non_upper_case_globals)]
    pub(crate) const Unstated: Self = Self { obligations: None };

    pub(crate) fn fully_discharged(&self) -> bool {
        self.obligations.as_ref().is_some_and(|entries| {
            !entries.is_empty() && entries.iter().all(|entry| entry.discharged)
        })
    }

    pub(crate) fn detail(&self) -> String {
        match &self.obligations {
            None => "尚未提供可证明覆盖完整性的合同义务集合，本入口不判定为已确认".into(),
            Some(entries) if entries.is_empty() => "合同义务集合为空，不能视为覆盖完整".into(),
            Some(entries) => format!(
                "合同存在未履行的核验义务：{}",
                entries
                    .iter()
                    .filter(|entry| !entry.discharged)
                    .map(|entry| format!("{} [{}]: {}", entry.id, entry.check, entry.reason))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn single_check(check: &str, discharged: bool) -> Self {
        Self::test_declared(vec![(check.into(), discharged)])
    }

    #[cfg(test)]
    pub(crate) fn test_declared(entries: Vec<(String, bool)>) -> Self {
        Self {
            obligations: Some(
                entries
                    .into_iter()
                    .map(|(check, discharged)| Obligation {
                        id: check.clone(),
                        check,
                        discharged,
                        reason: "synthetic decision fixture".into(),
                    })
                    .collect(),
            ),
        }
    }
}
