//! Per-backend specifics on top of the shared ACP client: measured capability
//! overrides, launch checks and known gaps. One file per backend so the three
//! integration branches never edit the same file (plan 7.1).

pub mod codex;
pub mod deepseek;
pub mod pi;
