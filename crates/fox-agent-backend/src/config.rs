//! Five-layer configuration precedence (plan 6.5).
//!
//! 1. Host policy (authorization, egress, disabled backends, env allowlist,
//!    enterprise policy) - lower layers can never loosen it.
//! 2. Session snapshot frozen at creation.
//! 3. User backend settings.
//! 4. Project configuration.
//! 5. Backend native defaults.
//!
//! Freezing is not permanent permission: tightening (revocation, enterprise
//! disable, stricter policy) applies to existing sessions before every
//! continue, resume or continuation.

use crate::port::AgentBackend;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostPolicy {
    pub disabled_backends: BTreeSet<AgentBackend>,
    /// Directories currently authorized. Revocation removes entries here.
    pub authorized_roots: BTreeSet<String>,
    pub egress_allowed: bool,
    /// Keys lower layers may never set (e.g. sandbox off, network on).
    pub locked_keys: BTreeMap<String, String>,
    /// Explicit full-access grants (`conversation:<id>` / `project:<root>`,
    /// see [`crate::external_home::FullAccessGrant`]). Revoking removes the key and
    /// applies to existing sessions before their next continue.
    pub full_access_grants: BTreeSet<String>,
    /// Enterprise switch: no session may run in full access.
    pub full_access_forbidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub backend: AgentBackend,
    pub project_root: String,
    pub egress_allowed: bool,
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub settings: BTreeMap<String, String>,
    /// Lower-layer values overridden by a higher layer, for audit.
    pub conflicts: Vec<String>,
}

/// Merge layers 2-5 under layer 1.
pub fn resolve(
    policy: &HostPolicy,
    snapshot: &SessionSnapshot,
    user: &BTreeMap<String, String>,
    project: &BTreeMap<String, String>,
    native_defaults: &BTreeMap<String, String>,
) -> Resolved {
    let mut settings = BTreeMap::new();
    let mut conflicts = Vec::new();
    let layers: [(&str, &BTreeMap<String, String>); 4] = [
        ("native default", native_defaults),
        ("project", project),
        ("user", user),
        ("session snapshot", &snapshot.settings),
    ];
    for (name, layer) in layers {
        for (key, value) in layer {
            if let Some(previous) = settings.insert(key.clone(), value.clone()) {
                if previous != *value {
                    conflicts.push(format!("{key}: {name} overrides {previous:?} with {value:?}"));
                }
            }
        }
    }
    for (key, value) in &policy.locked_keys {
        if let Some(previous) = settings.insert(key.clone(), value.clone()) {
            if previous != *value {
                conflicts.push(format!("{key}: host policy enforces {value:?} over {previous:?}"));
            }
        }
    }
    Resolved { settings, conflicts }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recheck {
    Proceed,
    /// Only read-only display of history is allowed.
    ReadOnly(String),
    Stop(String),
}

/// Run before continuing, resuming or continuing a session. The snapshot only
/// prevents loosening from being carried in silently; it never preserves a
/// permission the Host has since withdrawn.
pub fn recheck_before_continue(policy: &HostPolicy, snapshot: &SessionSnapshot) -> Recheck {
    if policy.disabled_backends.contains(&snapshot.backend) {
        return Recheck::ReadOnly(format!(
            "{} has been disabled; this conversation can be read but not continued",
            snapshot.backend.as_str()
        ));
    }
    if !policy.authorized_roots.contains(&snapshot.project_root) {
        return Recheck::Stop("the project folder is no longer authorized".into());
    }
    if snapshot.egress_allowed && !policy.egress_allowed {
        return Recheck::Stop("sending data to external services is no longer allowed".into());
    }
    Recheck::Proceed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            backend: AgentBackend::Codex,
            project_root: "/p".into(),
            egress_allowed: true,
            settings: BTreeMap::from([("model".into(), "m-snap".into())]),
        }
    }

    fn policy() -> HostPolicy {
        HostPolicy {
            authorized_roots: BTreeSet::from(["/p".into()]),
            egress_allowed: true,
            ..HostPolicy::default()
        }
    }

    #[test]
    fn host_policy_beats_every_lower_layer() {
        let mut policy = policy();
        policy.locked_keys.insert("sandbox".into(), "workspace-write".into());
        let user = BTreeMap::from([("sandbox".into(), "danger-full-access".into()), ("model".into(), "m-user".into())]);
        let resolved = resolve(&policy, &snapshot(), &user, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(resolved.settings["sandbox"], "workspace-write");
        assert_eq!(resolved.settings["model"], "m-snap", "frozen snapshot beats later user settings");
        assert_eq!(resolved.conflicts.len(), 2);
    }

    #[test]
    fn tightening_applies_to_existing_sessions() {
        let mut policy = policy();
        assert_eq!(recheck_before_continue(&policy, &snapshot()), Recheck::Proceed);
        policy.egress_allowed = false;
        assert!(matches!(recheck_before_continue(&policy, &snapshot()), Recheck::Stop(_)));
        policy.egress_allowed = true;
        policy.authorized_roots.clear();
        assert!(matches!(recheck_before_continue(&policy, &snapshot()), Recheck::Stop(_)));
        let mut disabled = self::policy();
        disabled.disabled_backends.insert(AgentBackend::Codex);
        assert!(matches!(recheck_before_continue(&disabled, &snapshot()), Recheck::ReadOnly(_)));
    }
}
