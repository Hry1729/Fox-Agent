//! Process-local signals for the durable WaitingJobs wake CAS. The database is
//! the source of truth; this slot only avoids lost in-process notifications.
use super::RuntimeHost;
use std::{sync::{atomic::{AtomicUsize, Ordering}, mpsc, Arc, Weak}, time::Duration};
use tokio::sync::Notify;

fn safe_wake_error_kind(error: &str) -> &'static str {
    if error.contains("cancelled wait reconciliation") { "cancel_scope_mismatch" }
    else if error.contains("frozen") || error.contains("identity") { "frozen_identity_mismatch" }
    else if error.contains("job notice") { "notice_unavailable" }
    else if error.contains("shutting down") { "host_stopping" }
    else { "durable_check_failed" }
}

pub(super) struct WakeSlot {
    pub park_seq: u64,
    pub notify: Weak<Notify>,
    pub finished: mpsc::Receiver<()>,
    pub lock_conflicts: Arc<AtomicUsize>,
}

impl RuntimeHost {
    pub(super) fn signal_waiting_run(&self, run_id: &str) -> Result<(), String> {
        if self.database.kernel_host_run_state(run_id)?.as_deref() != Some("waiting_jobs") {
            return Ok(());
        }
        let (park_seq, parked) = self.database.kernel_waiting_park(run_id)?;
        let deadline = parked["waitDeadlineWallMs"].as_i64()
            .filter(|value| *value > 0).ok_or("waiting Run has no original deadline")?;
        let notify = Arc::new(Notify::new());
        let lock_conflicts = Arc::new(AtomicUsize::new(0));
        let (finished_tx, finished_rx) = mpsc::channel();
        let force_per_round;
        {
            let mut state = self.state.lock().map_err(|_| "runtime state lock poisoned")?;
            if state.shutting_down { return Ok(()); }
            #[cfg(test)]
            if state.auto_wake_disabled.contains(run_id) { return Ok(()); }
            force_per_round = {
                #[cfg(test)] { state.auto_wake_force_round.contains(run_id) }
                #[cfg(not(test))] { false }
            };
            if let Some(slot) = state.waiting_wakes.get(run_id) {
                if slot.park_seq == park_seq {
                    if let Some(existing) = slot.notify.upgrade() {
                        existing.notify_one();
                        return Ok(());
                    }
                }
            }
            if let Some(old) = state.waiting_wakes.remove(run_id) {
                if let Some(existing) = old.notify.upgrade() { existing.notify_one(); }
            }
            state.waiting_wakes.insert(run_id.to_owned(), WakeSlot {
                park_seq, notify: Arc::downgrade(&notify), finished: finished_rx,
                lock_conflicts: lock_conflicts.clone(),
            });
        }
        let host = self.clone();
        let run_id = run_id.to_owned();
        tauri::async_runtime::spawn(async move {
            host.watch_waiting_run(&run_id, park_seq, deadline, notify,
                lock_conflicts, force_per_round).await;
            let _ = finished_tx.send(());
        });
        Ok(())
    }

    fn waiting_slot_is_current(&self, run_id: &str, park_seq: u64) -> bool {
        self.state.lock().ok().is_some_and(|state| !state.shutting_down
            && state.waiting_wakes.get(run_id).is_some_and(|slot| slot.park_seq == park_seq))
    }

    async fn watch_waiting_run(&self, run_id: &str, park_seq: u64,
        deadline_wall_ms: i64, notify: Arc<Notify>, lock_conflicts: Arc<AtomicUsize>,
        force_per_round: bool) {
        let mut lock_retry = Duration::from_millis(100);
        loop {
            if !self.waiting_slot_is_current(run_id, park_seq) { break; }
            // A wake can drive the real Pi worker. Never run it on the async
            // timer thread; the existing Host owns its lock, active marker and
            // cancellation scope throughout this blocking call.
            let host = self.clone();
            let run = run_id.to_owned();
            let checked = tauri::async_runtime::spawn_blocking(move ||
                host.wake_kernel_waiting_run_for_park_transport(&run, park_seq, force_per_round)).await;
            if !self.waiting_slot_is_current(run_id, park_seq) { break; }
            let now = crate::database::now_ms();
            let due = deadline_wall_ms.saturating_sub(now).max(0) as u64;
            let pause = match checked {
                Ok(Ok(true)) => break,
                Ok(Ok(false)) => {
                    if self.database.kernel_host_run_state(run_id).ok().flatten().as_deref()
                        != Some("waiting_jobs") { break; }
                    if due == 0 { break; }
                    Duration::from_millis(due)
                }
                Ok(Err(error)) if error == super::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED
                    || error.contains("job_wake_policy_changed")
                    || error.contains("job_wake_cancel_pending") => {
                    if error == super::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED {
                        lock_conflicts.fetch_add(1, Ordering::SeqCst);
                    }
                    // Another process may own the lock and cannot notify this
                    // Host. Retry finitely without extending the original wait.
                    if now >= deadline_wall_ms.saturating_add(5_000) { break; }
                    let pause = lock_retry;
                    lock_retry = (lock_retry * 2).min(Duration::from_secs(1));
                    pause
                }
                Ok(Err(error)) => {
                    eprintln!("Kernel WaitingJobs check for run {run_id} failed: {}",
                        safe_wake_error_kind(&error));
                    // A malformed or uncertain durable fact stays fail-closed.
                    // A later committed notice may signal another check; the
                    // deadline still terminates this in-process watcher.
                    if due == 0 { break; }
                    Duration::from_millis(due)
                }
                Err(_) => {
                    eprintln!("Kernel WaitingJobs check task for run {run_id} failed");
                    if due == 0 { break; }
                    Duration::from_millis(due)
                }
            };
            if tokio::time::timeout(pause, notify.notified()).await.is_ok() {
                lock_retry = Duration::from_millis(100);
            }
        }
        if let Ok(mut state) = self.state.lock() {
            if state.waiting_wakes.get(run_id).is_some_and(|slot| slot.park_seq == park_seq) {
                state.waiting_wakes.remove(run_id);
            }
        }
    }

    pub(super) fn stop_waiting_watches(&self) -> Result<(), String> {
        let slots = {
            let mut state = self.state.lock().map_err(|_| "runtime state lock poisoned")?;
            std::mem::take(&mut state.waiting_wakes)
        };
        for slot in slots.values() {
            if let Some(notify) = slot.notify.upgrade() { notify.notify_one(); }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        for (_, slot) in slots {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() || slot.finished.recv_timeout(remaining).is_err() {
                return Err("Kernel WaitingJobs watcher did not stop before shutdown".into());
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn disable_auto_wake_for_test(&self, run_id: &str) {
        self.state.lock().unwrap().auto_wake_disabled.insert(run_id.to_owned());
    }

    #[cfg(test)]
    pub(crate) fn force_auto_wake_round_for_test(&self, run_id: &str) {
        self.state.lock().unwrap().auto_wake_force_round.insert(run_id.to_owned());
    }

    #[cfg(test)]
    pub(crate) fn auto_wake_lock_conflicts_for_test(&self, run_id: &str) -> usize {
        self.state.lock().unwrap().waiting_wakes.get(run_id)
            .map_or(0, |slot| slot.lock_conflicts.load(Ordering::SeqCst))
    }
}
