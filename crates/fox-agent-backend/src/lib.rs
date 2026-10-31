//! Fox AgentBackend public port.
//!
//! Implements the rules frozen in the four-agent plan v0.3.1 (docs/02-架构/
//! 四Agent改造实施方案-v0.3.1.md, sections 5 and 6) as plain, testable types:
//!
//! * [`port`]: backend identity, four-dimension capability declaration, session
//!   binding, event envelope with connection generation.
//! * [`settlement`]: pending-request settlement on cancel/disconnect and the
//!   single authoritative terminal per run; late responses are retained for
//!   audit and never flip a terminal.
//! * [`limits`]: concurrency admission (section 6.2).
//! * [`config`]: five-layer configuration precedence; tightening applies to
//!   existing sessions (section 6.5).
//! * [`legacy`]: deterministic mapping of old run records (section 6.6).
//! * [`host_routing`]: Host wiring first batch - conversation-level
//!   `agent_backend` routing of start / cancel / approve / resume, misroute
//!   refusal, the `FOX_EXTERNAL_BACKENDS` switch and the additive migration
//!   SQL (sections 5.4, 6.6).
//! * [`launch`]: process launch specs for the external backends, all over ACP
//!   (section 6.7).
//! * [`external_home`]: the permission mode a session records, the private
//!   HOME per project x backend and the launch preparation. Fox adds no
//!   sandbox around an external backend; its native sandbox is what limits it.
//! * [`proctree`]: confirming that a stopped backend's whole process tree
//!   ended (process group + tracked descendants on Unix).
//! * [`statefile`]: locked, atomically written JSON state files.
//! * [`credential_save`]: provider save order with Key persistence (snapshot,
//!   Keys before the record, undo on failure, deletions after the record).
//! * [`credential_delete`]: provider delete reclaims its Keys (record first,
//!   then Keys; a failed Key delete never restores the record, it is audited).
//! * `acp` (feature `acp`): the Host-side ACP v1 client built on the official
//!   `agent-client-protocol` crate.
//! * [`projection`]: ACP `session/update` / `request_permission` -> the Fox
//!   runtime events and approval card the desktop already renders.
//! * [`native_permission`]: per-agent native permission list (Codex modes,
//!   dsh presets, pi gate modes), the mode a session really runs in and the
//!   Host's fail-closed answer to a permission request.
//! * [`capability_view`]: the four capability dimensions as UI text, from the
//!   backend's measured declaration.
//! * `external` (feature `acp`): the Host's external backend runtime (one
//!   ACP connection per conversation: start, stream, approvals, cancel,
//!   crash, close).
//! * `external_launch` (feature `acp`): the real launcher (Fox-managed
//!   runtime layout, per-backend launch specs, private HOME).
//!
//! This crate has no Tauri dependency so it builds and tests on any host.

pub mod capability_view;
pub mod config;
pub mod console;
pub mod credential_delete;
pub mod credential_save;
pub mod external_home;
pub mod host_routing;
pub mod launch;
pub mod legacy;
pub mod limits;
pub mod model_endpoint;
pub mod login_source;
pub mod runtime_adapters;
pub mod native_permission;
pub mod onboarding;
pub mod port;
pub mod proctree;
pub mod projection;
pub mod settlement;
pub mod statefile;
pub mod unified_login;
pub mod provider_protocol;

#[cfg(feature = "acp")]
pub mod acp;
#[cfg(feature = "acp")]
pub mod external;
#[cfg(feature = "acp")]
pub mod external_launch;
#[cfg(feature = "acp")]
pub mod preview;
#[cfg(feature = "acp")]
pub mod fox_mcp;

pub mod backends;
