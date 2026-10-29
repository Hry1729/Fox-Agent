use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, RunControlBinding};
use std::env::VarError;

pub(super) fn select(
    frozen: Option<ExecutionAuthority>,
    configured: Result<String, VarError>,
) -> Result<ExecutionAuthority, String> {
    if let Some(authority) = frozen {
        return Ok(authority);
    }
    match configured {
        // All launch paths (including plain `pnpm tauri.cmd dev`) use Kernel.
        // Legacy is an explicit process-level opt-in, never a build fallback.
        Err(VarError::NotPresent) => Ok(ExecutionAuthority::Authoritative),
        Ok(value) if value == "legacy" => Ok(ExecutionAuthority::Legacy),
        Ok(value) if value == "authoritative" => Ok(ExecutionAuthority::Authoritative),
        _ => Err("FOX_KERNEL_MODE must be legacy or authoritative".into()),
    }
}

/// Which executor owns a frozen Run.
///
/// `Kernel` does not assert "drive this on the Kernel". It means *no route was
/// frozen*, so the authority-derived selection above still decides between the
/// Kernel and the legacy worker — an unrouted binding keeps the meaning it was
/// frozen with, which is why the route is an optional field rather than a
/// `CONTROL_SCHEMA_VERSION` bump.
pub(super) enum RunRoute {
    Kernel,
    PiNative,
    Reject,
}

/// The single routing decision shared by startup recovery, normal dispatch and
/// the legacy startup repair, so that no two of them can claim the same Run.
///
/// A route only ever describes the Authoritative contract driven by the Pi
/// engine: the single path reuses that authority instead of adding a third. Any
/// other combination is one this build cannot honour, and is refused rather
/// than guessed at.
pub(super) fn resolve_run_route(binding: Option<&RunControlBinding>) -> RunRoute {
    let Some(binding) = binding else { return RunRoute::Kernel };
    match (binding.authority, binding.executor_route) {
        (_, None) => RunRoute::Kernel,
        (ExecutionAuthority::Authoritative, Some(ExecutorRoute::PiNative))
            if binding.engine_id == "pi" => RunRoute::PiNative,
        _ => RunRoute::Reject,
    }
}

impl RunRoute {
    /// The stable refusal code for a route this build cannot execute.
    pub(super) fn refusal_reason(self) -> Option<&'static str> {
        match self {
            Self::Reject => Some("run_route_unrecognized"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_environment_uses_the_built_in_default() {
        assert_eq!(select(None, Err(VarError::NotPresent)).unwrap(), ExecutionAuthority::Authoritative);
    }

    #[test]
    fn explicit_override_is_validated() {
        assert_eq!(select(None, Ok("legacy".into())).unwrap(), ExecutionAuthority::Legacy);
        assert_eq!(select(None, Ok("authoritative".into())).unwrap(), ExecutionAuthority::Authoritative);
        assert!(select(None, Ok("typo".into())).is_err());
    }

    #[test]
    fn existing_binding_is_not_reinterpreted_by_a_new_default() {
        for authority in [ExecutionAuthority::Legacy, ExecutionAuthority::Authoritative] {
            assert_eq!(select(Some(authority), Err(VarError::NotPresent)).unwrap(), authority);
            assert_eq!(select(Some(authority), Ok("typo".into())).unwrap(), authority);
        }
    }

    fn binding(
        authority: ExecutionAuthority,
        route: Option<ExecutorRoute>,
        engine: &str,
    ) -> RunControlBinding {
        RunControlBinding {
            schema_version: fox_engine_protocol::CONTROL_SCHEMA_VERSION,
            run_id: "run".into(),
            conversation_id: "conversation".into(),
            engine_id: engine.into(),
            execution_profile_id: "profile".into(),
            authority,
            read_only_executor: fox_engine_protocol::ResourceExecutor::Rust,
            permission_snapshot_id: "snapshot".into(),
            permission: fox_engine_protocol::FrozenPermission {
                mode: fox_engine_protocol::PermissionMode::Ask,
                project_root: None,
                grants: Vec::new(),
                approval_epoch: None,
            },
            budgets: fox_engine_protocol::TimeBudgets::default(),
            executor_route: route,
        }
    }

    #[test]
    fn an_unrouted_binding_keeps_its_authority_derived_meaning() {
        assert!(matches!(resolve_run_route(None), RunRoute::Kernel));
        for authority in [ExecutionAuthority::Legacy, ExecutionAuthority::Authoritative] {
            for engine in ["pi", "codex", "deepseek_harness"] {
                assert!(
                    matches!(resolve_run_route(Some(&binding(authority, None, engine))), RunRoute::Kernel),
                    "an unrouted {engine} binding under {authority:?} must not change executor"
                );
            }
        }
    }

    #[test]
    fn only_the_authoritative_pi_combination_routes_to_the_single_path() {
        let routed = binding(ExecutionAuthority::Authoritative, Some(ExecutorRoute::PiNative), "pi");
        assert!(matches!(resolve_run_route(Some(&routed)), RunRoute::PiNative));
        assert_eq!(RunRoute::PiNative.refusal_reason(), None);
    }

    #[test]
    fn every_other_route_combination_is_refused_rather_than_guessed() {
        // A route never describes the legacy contract.
        let legacy = binding(ExecutionAuthority::Legacy, Some(ExecutorRoute::PiNative), "pi");
        assert!(matches!(resolve_run_route(Some(&legacy)), RunRoute::Reject));
        // It never describes a retired or unknown engine either.
        for engine in ["codex", "deepseek_harness", "unknown_engine"] {
            let foreign = binding(ExecutionAuthority::Authoritative, Some(ExecutorRoute::PiNative), engine);
            assert!(
                matches!(resolve_run_route(Some(&foreign)), RunRoute::Reject),
                "{engine} must not reach the single path"
            );
        }
        assert_eq!(RunRoute::Reject.refusal_reason(), Some("run_route_unrecognized"));
        assert_eq!(RunRoute::Kernel.refusal_reason(), None);
        assert_eq!(RunRoute::PiNative.refusal_reason(), None);
    }
}
