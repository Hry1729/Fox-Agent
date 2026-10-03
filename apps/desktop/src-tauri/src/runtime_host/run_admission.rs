//! Process-local admission for independently progressing Kernel runs.
//! Root and child lanes are distinct so a parent waiting for a child cannot
//! consume every permit the child needs to finish.
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

struct GateState {
    active: HashSet<String>,
    waiting: VecDeque<String>,
}

pub(super) struct Gate {
    limit: usize,
    state: Mutex<GateState>,
    changed: Condvar,
}

pub(super) struct Permit {
    gate: Arc<Gate>,
    run_id: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut state) = self.gate.state.lock() {
            state.active.remove(&self.run_id);
            self.gate.changed.notify_all();
        }
    }
}

impl Gate {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: limit.clamp(1, 32),
            state: Mutex::new(GateState {
                active: HashSet::new(),
                waiting: VecDeque::new(),
            }),
            changed: Condvar::new(),
        })
    }

    pub(super) fn limit(&self) -> usize { self.limit }

    pub(super) fn acquire(
        self: &Arc<Self>, run_id: &str, mut cancelled: impl FnMut() -> bool,
        stopped_error: &str,
    ) -> Result<Permit, String> {
        let mut state = self.state.lock().map_err(|_| "run admission lock poisoned")?;
        if state.active.contains(run_id) || state.waiting.iter().any(|id| id == run_id) {
            return Err("duplicate Kernel run admission".into());
        }
        state.waiting.push_back(run_id.to_owned());
        loop {
            // Do not hold the gate lock while reading Host shutdown state.
            drop(state);
            let stopped = cancelled();
            state = self.state.lock().map_err(|_| "run admission lock poisoned")?;
            if stopped {
                state.waiting.retain(|id| id != run_id);
                self.changed.notify_all();
                return Err(stopped_error.into());
            }
            if state.waiting.front().is_some_and(|id| id == run_id)
                && state.active.len() < self.limit {
                state.waiting.pop_front();
                state.active.insert(run_id.to_owned());
                return Ok(Permit { gate: self.clone(), run_id: run_id.to_owned() });
            }
            state = self.changed.wait_timeout(state, Duration::from_millis(50))
                .map_err(|_| "run admission lock poisoned")?.0;
        }
    }
}

pub(super) fn configured_limit(name: &str, fallback: usize) -> usize {
    std::env::var(name).ok().and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=32).contains(value)).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn fifo_capacity_is_bounded_and_released() {
        let gate = Gate::new(2);
        let first = gate.acquire("a", || false, "stopped").unwrap();
        let second = gate.acquire("b", || false, "stopped").unwrap();
        let (sender, receiver) = mpsc::channel();
        let other = gate.clone();
        let worker = std::thread::spawn(move || {
            let _third = other.acquire("c", || false, "stopped").unwrap();
            sender.send(()).unwrap();
        });
        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        drop(first);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(second);
        worker.join().unwrap();
    }

    #[test]
    fn cancelled_waiter_releases_its_place_for_the_next_run() {
        let gate = Gate::new(1);
        let first = gate.acquire("a", || false, "stopped").unwrap();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let waiting = gate.clone();
        let worker = std::thread::spawn(move || waiting.acquire("b",
            || flag.load(std::sync::atomic::Ordering::SeqCst), "cancelled"));
        std::thread::sleep(Duration::from_millis(80));
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(worker.join().unwrap().err().as_deref(), Some("cancelled"));
        drop(first);
        let _next = gate.acquire("c", || false, "stopped").unwrap();
    }
}
