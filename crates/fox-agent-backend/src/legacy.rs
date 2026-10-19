//! Deterministic mapping of existing run records to an outer backend (plan
//! 6.6). `engine_id` keeps its historical meaning; it is read, never rewritten.

use crate::port::AgentBackend;

/// What an old record carries. Values come straight from the existing
/// `kernel_mode` / `engine_id` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRecord<'a> {
    pub kernel_mode: Option<&'a str>,
    pub engine_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyMapping {
    /// Resume through FoxBackend with this historical model adapter.
    Fox { model_adapter: String },
    /// Produced by the legacy path: read-only unless the legacy switch is on.
    FoxLegacy,
    /// Source cannot be determined: read-only, no runtime is started.
    Unmapped(String),
}

impl LegacyMapping {
    pub fn agent_backend(&self) -> Option<AgentBackend> {
        match self {
            Self::Fox { .. } | Self::FoxLegacy => Some(AgentBackend::Fox),
            Self::Unmapped(_) => None,
        }
    }
}

pub fn map_legacy(record: &LegacyRecord<'_>) -> LegacyMapping {
    match (record.kernel_mode, record.engine_id) {
        (Some("authoritative"), Some(engine @ ("pi" | "codex" | "deepseek_harness"))) => {
            // An old `codex` / `deepseek_harness` record is FoxBackend driving
            // that model adapter; it is never resumed as the new full
            // CodexBackend / DeepSeekBackend.
            LegacyMapping::Fox {
                model_adapter: engine.to_owned(),
            }
        }
        (Some("legacy"), _) => LegacyMapping::FoxLegacy,
        (mode, engine) => LegacyMapping::Unmapped(format!(
            "unrecognized kernel_mode={mode:?} engine_id={engine:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_codex_and_deepseek_runs_stay_on_fox() {
        for engine in ["pi", "codex", "deepseek_harness"] {
            let mapped = map_legacy(&LegacyRecord { kernel_mode: Some("authoritative"), engine_id: Some(engine) });
            assert_eq!(mapped, LegacyMapping::Fox { model_adapter: engine.into() });
            assert_eq!(mapped.agent_backend(), Some(AgentBackend::Fox));
        }
    }

    #[test]
    fn missing_fields_are_not_guessed() {
        assert!(matches!(map_legacy(&LegacyRecord { kernel_mode: None, engine_id: None }), LegacyMapping::Unmapped(_)));
        assert!(matches!(
            map_legacy(&LegacyRecord { kernel_mode: Some("authoritative"), engine_id: Some("gpt") }),
            LegacyMapping::Unmapped(_)
        ));
        assert_eq!(map_legacy(&LegacyRecord { kernel_mode: Some("legacy"), engine_id: Some("pi") }), LegacyMapping::FoxLegacy);
    }
}
