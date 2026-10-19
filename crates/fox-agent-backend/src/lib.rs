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
//! * [`launch`]: process launch specs for the external backends, all over ACP
//!   (section 6.7).
//! * [`sandbox`]: permission modes (`full_access` / `sandboxed`) and the srt
//!   isolation layer for external backends (v0.3.2 addendum), fail-closed.
//! * [`proctree`]: confirming that a stopped backend's whole process tree
//!   ended (process group + tracked descendants on Unix).
//! * [`winacl`]: Windows ACL queries for that layer (ownership / `WRITE_DAC`
//!   preflight, report-only exposure audit of broadly readable folders).
//! * `acp` (feature `acp`): the Host-side ACP v1 client built on the official
//!   `agent-client-protocol` crate.
//!
//! This crate has no Tauri dependency so it builds and tests on any host.

pub mod config;
pub mod launch;
pub mod legacy;
pub mod limits;
pub mod port;
pub mod proctree;
pub mod reverse;
pub mod sandbox;
pub mod settlement;
pub mod winacl;

#[cfg(feature = "acp")]
pub mod acp;

pub mod backends;
