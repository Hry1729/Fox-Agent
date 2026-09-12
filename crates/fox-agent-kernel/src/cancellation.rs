//! Cooperative cancellation identity. Cancelling a scope is irreversible;
//! registering it again cannot resurrect already-issued execution tokens.
use crate::CancellationPort;
use std::{collections::HashMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};

#[derive(Clone)]
pub struct CancellationToken {
    run: Arc<AtomicBool>,
    tool: Arc<AtomicBool>,
}

impl CancellationToken {
    /// A token that is never cancelled. Used only to bound a short, in-process
    /// drain (e.g. reading the worker's final frame after the Host already
    /// committed the terminal state) by deadline rather than cancellation.
    pub fn uncancellable() -> Self {
        CancellationToken {
            run: Arc::new(AtomicBool::new(false)),
            tool: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.run.load(Ordering::Acquire) || self.tool.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err("[tool.cancelled] execution was cancelled".into()) } else { Ok(()) }
    }
}

#[derive(Default)]
struct RunScope {
    cancelled: Arc<AtomicBool>,
    tools: HashMap<String, Arc<AtomicBool>>,
}

#[derive(Clone, Default)]
pub struct CancellationRegistry {
    runs: Arc<Mutex<HashMap<String, RunScope>>>,
}

impl CancellationRegistry {
    pub fn register_run(&self, run_id: &str) -> Result<(), String> {
        if run_id.trim().is_empty() { return Err("cancellation run identity is empty".into()); }
        let mut runs = self.runs.lock().map_err(|_| "cancellation registry poisoned")?;
        let scope = runs.entry(run_id.into()).or_default();
        if scope.cancelled.load(Ordering::Acquire) { return Err("Run was already cancelled".into()); }
        Ok(())
    }
    pub fn tool_token(&self, run_id: &str, tool_call_id: &str) -> Result<CancellationToken, String> {
        if tool_call_id.trim().is_empty() { return Err("cancellation tool identity is empty".into()); }
        let mut runs = self.runs.lock().map_err(|_| "cancellation registry poisoned")?;
        let scope = runs.get_mut(run_id).ok_or("unknown cancellation Run")?;
        Ok(CancellationToken { run: scope.cancelled.clone(), tool: scope.tools.entry(tool_call_id.into()).or_default().clone() })
    }
    pub fn run_token(&self, run_id: &str) -> Result<CancellationToken, String> {
        let runs = self.runs.lock().map_err(|_| "cancellation registry poisoned")?;
        let scope = runs.get(run_id).ok_or("unknown cancellation Run")?;
        Ok(CancellationToken { run: scope.cancelled.clone(), tool: Arc::new(AtomicBool::new(false)) })
    }
    /// Drop terminal scope bookkeeping only after dispatch is fenced. Old
    /// tokens remain cancelled even if another scope is later registered.
    pub fn retire_run(&self, run_id: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(scope) = runs.remove(run_id) { scope.cancelled.store(true, Ordering::Release); }
        }
    }
}

impl CancellationPort for CancellationRegistry {
    fn request_run_cancel(&self, run_id: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            runs.entry(run_id.into()).or_default().cancelled.store(true, Ordering::Release);
        }
    }
    fn is_run_cancelled(&self, run_id: &str) -> bool {
        self.runs.lock().ok().and_then(|runs| runs.get(run_id).map(|scope| scope.cancelled.load(Ordering::Acquire))).unwrap_or(true)
    }
    fn request_tool_cancel(&self, run_id: &str, tool_call_id: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            // Preserve a cancellation received before Run registration. The
            // later registration may enable other tools, never this identity.
            runs.entry(run_id.into()).or_default().tools.entry(tool_call_id.into()).or_default().store(true, Ordering::Release);
        }
    }
    fn is_tool_cancelled(&self, run_id: &str, tool_call_id: &str) -> bool {
        self.runs.lock().ok().and_then(|runs| runs.get(run_id).map(|scope| {
            scope.cancelled.load(Ordering::Acquire) || scope.tools.get(tool_call_id).map(|token| token.load(Ordering::Acquire)).unwrap_or(true)
        })).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_scoped_and_cannot_be_cleared_by_reregistration() {
        let registry = CancellationRegistry::default();
        registry.register_run("a").unwrap();
        registry.register_run("b").unwrap();
        let a1 = registry.tool_token("a", "same-id").unwrap();
        let a2 = registry.tool_token("a", "other").unwrap();
        let b1 = registry.tool_token("b", "same-id").unwrap();
        registry.request_tool_cancel("a", "same-id");
        assert!(a1.is_cancelled());
        assert!(registry.tool_token("a", "same-id").unwrap().is_cancelled());
        assert!(!a2.is_cancelled());
        assert!(!b1.is_cancelled());
        registry.request_run_cancel("a");
        assert!(a2.check().is_err());
        assert!(registry.register_run("a").is_err());
        assert!(!b1.is_cancelled());
    }
    #[test]
    fn unknown_scopes_fail_closed_and_retirement_cancels_issued_tokens() {
        let registry = CancellationRegistry::default();
        assert!(registry.is_run_cancelled("unknown"));
        assert!(registry.tool_token("unknown", "tool").is_err());
        registry.request_run_cancel("before-start");
        assert!(registry.register_run("before-start").is_err());
        registry.request_tool_cancel("tool-before-start", "cancelled-tool");
        registry.register_run("tool-before-start").unwrap();
        assert!(registry.tool_token("tool-before-start", "cancelled-tool").unwrap().is_cancelled());
        assert!(!registry.tool_token("tool-before-start", "other-tool").unwrap().is_cancelled());
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "t").unwrap();
        registry.retire_run("r");
        assert!(token.is_cancelled());
        assert!(registry.is_tool_cancelled("r", "t"));
    }
}
