use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, RunControlBinding};

/// Resolve the authority for a Run.
///
/// A frozen binding always wins: a process-level setting must never reinterpret
/// an existing Run. New Runs always use the Authoritative contract. The former
/// `FOX_KERNEL_MODE` process switch no longer selects a new Run's execution
/// path; S5 keeps only the frozen binding and `FOX_EXECUTOR_ROUTE` (see
/// `run_control::candidate_executor_route`).
pub(super) fn select(frozen: Option<ExecutionAuthority>) -> Result<ExecutionAuthority, String> {
    Ok(frozen.unwrap_or(ExecutionAuthority::Authoritative))
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
    fn new_runs_use_the_authoritative_contract_without_an_environment_switch() {
        assert_eq!(select(None).unwrap(), ExecutionAuthority::Authoritative);
    }

    #[test]
    fn existing_authority_is_not_reinterpreted_by_a_new_default() {
        for authority in [ExecutionAuthority::Legacy, ExecutionAuthority::Authoritative] {
            assert_eq!(select(Some(authority)).unwrap(), authority);
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
