//! Fox AgentBackend public port.
//!
//! Implements the rules frozen in the four-agent plan v0.3.1 (docs/02-架构/
//! 四Agent改造实施方案-v0.3.1.md, sections 5 and 6) as plain, testable types:
//!
//! * [`port`]: backend identity, four-dimension capability declaration, session
//!   binding, event envelope with connection generation.
//! * [`reverse`]: Agent-initiated requests (approval, tool, UI) and their state
//!   machine, including `outcome_unknown` and the rule that a native
//!   acknowledgement is never fabricated.
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
//! * [`sandbox`]: permission modes (`full_access` / `sandboxed`) and the srt
//!   isolation layer for external backends (v0.3.2 addendum), fail-closed.
//! * [`proctree`]: confirming that a stopped backend's whole process tree
//!   ended (process group + tracked descendants on Unix).
//! * [`winacl`]: Windows ACL queries and edits for that layer (ownership /
//!   `WRITE_DAC` preflight, report-only exposure audit of broadly readable
//!   folders, exact ACE add / remove, owner change, private directories).
//! * [`winruntime`]: Windows layout and lifecycle (machine-wide runtime in
//!   Program Files, per-user private root in ProgramData, srt registry state,
//!   SID ledger, install record, uninstall boundary).
//! * [`winancestor`]: Windows ancestor metadata ACEs for the sandbox account
//!   and their reference ledger / reconcile / crash recovery.
//! * [`winowner`]: Windows ownership repair of objects the sandbox created.
//! * [`statefile`]: locked, atomically written JSON state files.
//! * `acp` (feature `acp`): the Host-side ACP v1 client built on the official
//!   `agent-client-protocol` crate.
//! * `bridge` (feature `acp`): Host side of the ACP bridge used on Windows in
//!   sandboxed mode (srt 0.0.80 does not deliver stdin there): one-shot named
//!   pipe, token handshake.
//! * [`projection`]: ACP `session/update` / `request_permission` -> the Fox
//!   runtime events and approval card the desktop already renders.
//! * [`capability_view`]: the four capability dimensions as UI text, from the
//!   backend's measured declaration and the session's sandbox declaration.
//! * `external` (feature `acp`): the Host's external backend runtime (one
//!   ACP connection per conversation: start, stream, approvals, cancel,
//!   crash, close).
//! * `external_launch` (feature `acp`): the real launcher (Fox-managed
//!   runtime layout, per-backend launch specs, `prepare_launch` in the
//!   session's permission mode, Windows bridge / private layout / ancestor
//!   ACEs).
//!
//! This crate has no Tauri dependency so it builds and tests on any host.

pub mod capability_view;
pub mod config;
pub mod host_routing;
pub mod launch;
pub mod legacy;
pub mod limits;
pub mod port;
pub mod proctree;
pub mod projection;
pub mod reverse;
pub mod sandbox;
pub mod settlement;
pub mod statefile;
pub mod winacl;
pub mod winancestor;
pub mod winowner;
pub mod winruntime;

#[cfg(feature = "acp")]
pub mod acp;
#[cfg(feature = "acp")]
pub mod bridge;
#[cfg(feature = "acp")]
pub mod external;
#[cfg(feature = "acp")]
pub mod external_launch;

pub mod backends;
