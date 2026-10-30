use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, RunControlBinding};

/// Resolve the authority for a Run.
///
/// A frozen binding always wins: a process-level setting must never reinterpret
/// an existing Run. New Runs always use the Authoritative contract. The former
/// `FOX_KERNEL_MODE` process switch no longer selects a new Run's execution
/// path, and since A0 there is no route switch either: a new Run always freezes
/// PiNative (see `run_control::new_run_executor_route`).
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

/// Whether a frozen binding is a historical Kernel Run (B1).
///
/// The Kernel execution loop has been removed. A binding frozen Authoritative
/// with no executor route used to be driven by it; such a Run is never started,
/// resumed, cancelled into a queue or approved again. A Legacy binding, or no
/// binding at all, is not a Kernel Run.
pub(super) fn is_retired_kernel(binding: Option<&RunControlBinding>) -> bool {
    binding.is_some_and(|binding| {
        binding.authority == ExecutionAuthority::Authoritative && binding.executor_route.is_none()
    })
}

/// The refusal a retired Kernel Run gets when anything tries to drive it.
pub(super) fn retired_route_error() -> String {
    format!(
        "{}: {}",
        crate::database::EXECUTOR_ROUTE_RETIRED,
        crate::database::EXECUTOR_ROUTE_RETIRED_MESSAGE,
    )
}

impl RunRoute {
    /// The stable refusal code for a route this build cannot execute.
    pub(super) fn refusal_reason(self) -> Option<&'static str> {
        match self {
            Self::Reject => Some(crate::database::EXECUTOR_ROUTE_UNKNOWN),
            _ => None,
        }
    }
}

/// The binding a child Run freezes (A0).
///
/// A child inherits its Authoritative parent's frozen permission and budgets,
/// never its executor: whatever route the parent was frozen with, including a
/// historical unrouted Kernel parent, the child is frozen PiNative on the pi
/// engine, so no child can ever land on the frozen Kernel loop.
pub(super) fn child_run_binding(
    parent: &RunControlBinding,
    child_run_id: &str,
    child_conversation_id: &str,
    read_only: bool,
    execution_profile: Option<&str>,
    max_duration_ms: i64,
) -> Result<RunControlBinding, String> {
    if parent.authority != ExecutionAuthority::Authoritative {
        return Err("only an Authoritative parent hands its frozen control to a child".into());
    }
    let mut inherited = parent.clone();
    inherited.run_id = child_run_id.into();
    inherited.conversation_id = child_conversation_id.into();
    inherited.engine_id = "pi".into();
    inherited.executor_route = crate::database::Database::new_run_executor_route(ExecutionAuthority::Authoritative);
    if read_only { inherited.permission.mode = fox_engine_protocol::PermissionMode::ReadOnly; }
    if let Some(profile) = execution_profile { inherited.execution_profile_id = profile.into(); }
    inherited.permission_snapshot_id = crate::database::Database::run_control_permission_hash(&inherited.permission)?;
    inherited.budgets.restrict_execution_ms(max_duration_ms);
    inherited.validate()?;
    Ok(inherited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a0_child_runs_always_freeze_pi_native_whatever_the_parent_route() {
        for parent_route in [None, Some(ExecutorRoute::PiNative)] {
            for parent_engine in ["pi", "codex"] {
                let parent = binding(ExecutionAuthority::Authoritative, parent_route, parent_engine);
                let child = child_run_binding(&parent, "child-run", "child-conversation", false, None, 30_000).unwrap();
                assert_eq!(child.executor_route, Some(ExecutorRoute::PiNative), "{parent_route:?} {parent_engine}");
                assert_eq!(child.engine_id, "pi");
                assert_eq!(child.authority, ExecutionAuthority::Authoritative);
                assert_eq!((child.run_id.as_str(), child.conversation_id.as_str()), ("child-run", "child-conversation"));
                // The single resolver routes it to the PiNative path, never Kernel.
                assert!(matches!(resolve_run_route(Some(&child)), RunRoute::PiNative), "{parent_route:?} {parent_engine}");
                assert!(child.budgets.run_execution_limited);
                assert!(child.budgets.run_execution_ms <= 30_000);
            }
        }
    }

    #[test]
    fn a0_child_binding_keeps_the_parent_permission_and_narrows_it_when_asked() {
        let parent = binding(ExecutionAuthority::Authoritative, None, "pi");
        let inherited = child_run_binding(&parent, "child", "child-conversation", false, None, 1_000).unwrap();
        assert_eq!(inherited.permission, parent.permission);
        assert_eq!(inherited.execution_profile_id, parent.execution_profile_id);
        let reviewer = child_run_binding(&parent, "child", "child-conversation", true, Some("graph_reviewer_v1"), 1_000).unwrap();
        assert_eq!(reviewer.permission.mode, fox_engine_protocol::PermissionMode::ReadOnly);
        assert_eq!(reviewer.execution_profile_id, "graph_reviewer_v1");
        assert_eq!(reviewer.permission_snapshot_id,
            crate::database::Database::run_control_permission_hash(&reviewer.permission).unwrap());
        assert_ne!(reviewer.permission_snapshot_id, inherited.permission_snapshot_id);
        // A Legacy parent never hands its control to a child.
        assert!(child_run_binding(&binding(ExecutionAuthority::Legacy, None, "pi"), "child", "c", false, None, 1_000).is_err());
    }

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
        assert_eq!(RunRoute::Reject.refusal_reason(), Some("executor_route_unknown"));
        assert_eq!(RunRoute::Kernel.refusal_reason(), None);
        assert_eq!(RunRoute::PiNative.refusal_reason(), None);
    }
}
