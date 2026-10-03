//! Process-local duplicate guard for independently progressing Kernel runs.
//! This guard has no run-count limit. Per-run ownership and cancellation remain
//! separate from the number of other conversations in progress.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub(super) struct Gate {
    active: Mutex<HashSet<String>>,
}

pub(super) struct Permit {
    gate: Arc<Gate>,
    run_id: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            active.remove(&self.run_id);
        }
    }
}

impl Gate {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self { active: Mutex::new(HashSet::new()) })
    }

    pub(super) fn acquire(
        self: &Arc<Self>, run_id: &str, mut cancelled: impl FnMut() -> bool,
        stopped_error: &str,
    ) -> Result<Permit, String> {
        // Read Host state outside the guard lock to preserve lock ordering.
        if cancelled() {
            return Err(stopped_error.into());
        }
        let mut active = self.active.lock().map_err(|_| "run admission lock poisoned")?;
        if !active.insert(run_id.to_owned()) {
            return Err("duplicate Kernel run admission".into());
        }
        Ok(Permit { gate: self.clone(), run_id: run_id.to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_runs_have_no_count_gate_but_duplicate_ownership_is_rejected() {
        let gate = Gate::new();
        let permits: Vec<_> = (0..64)
            .map(|index| gate.acquire(&format!("run-{index}"), || false, "stopped").unwrap())
            .collect();
        assert_eq!(gate.acquire("run-0", || false, "stopped").err().as_deref(),
            Some("duplicate Kernel run admission"));
        drop(permits);
        let _again = gate.acquire("run-0", || false, "stopped").unwrap();
    }

    #[test]
    fn stopped_run_never_registers() {
        let gate = Gate::new();
        assert_eq!(gate.acquire("run", || true, "stopped").err().as_deref(),
            Some("stopped"));
        let _permit = gate.acquire("run", || false, "stopped").unwrap();
    }
}
