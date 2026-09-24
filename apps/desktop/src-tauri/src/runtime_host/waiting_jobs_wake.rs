//! Process-local signals for the durable WaitingJobs wake CAS. The database is
//! the source of truth; this slot only avoids lost in-process notifications.
use super::RuntimeHost;
use std::{sync::{mpsc, Arc, Weak}, time::Duration};
use tokio::sync::Notify;

pub(super) struct WakeSlot {
    pub park_seq: u64,
    pub notify: Weak<Notify>,
    pub finished: mpsc::Receiver<()>,
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
        let (finished_tx, finished_rx) = mpsc::channel();
        {
            let mut state = self.state.lock().map_err(|_| "runtime state lock poisoned")?;
            if state.shutting_down { return Ok(()); }
            #[cfg(test)]
            if state.auto_wake_disabled.contains(run_id) { return Ok(()); }
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
            });
        }
        let host = self.clone();
        let run_id = run_id.to_owned();
        tauri::async_runtime::spawn(async move {
            host.watch_waiting_run(&run_id, park_seq, deadline, notify).await;
            let _ = finished_tx.send(());
        });
        Ok(())
    }

    fn waiting_slot_is_current(&self, run_id: &str, park_seq: u64) -> bool {
        self.state.lock().ok().is_some_and(|state| !state.shutting_down
            && state.waiting_wakes.get(run_id).is_some_and(|slot| slot.park_seq == park_seq))
    }

    async fn watch_waiting_run(&self, run_id: &str, park_seq: u64,
        deadline_wall_ms: i64, notify: Arc<Notify>) {
        let mut lock_retry = Duration::from_millis(100);
        loop {
            if !self.waiting_slot_is_current(run_id, park_seq) { break; }
            // A wake can drive the real Pi worker. Never run it on the async
            // timer thread; the existing Host owns its lock, active marker and
            // cancellation scope throughout this blocking call.
            let host = self.clone();
            let run = run_id.to_owned();
            let checked = tauri::async_runtime::spawn_blocking(move ||
                host.wake_kernel_waiting_run_for_park(&run, park_seq)).await;
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
                    // Another process may own the lock and cannot notify this
                    // Host. Retry finitely without extending the original wait.
                    if now >= deadline_wall_ms.saturating_add(5_000) { break; }
                    let pause = lock_retry;
                    lock_retry = (lock_retry * 2).min(Duration::from_secs(1));
                    pause
                }
                _ => {
                    // A malformed or uncertain durable fact stays fail-closed.
                    // A later committed notice may signal another check; the
                    // deadline still terminates this in-process watcher.
                    if due == 0 { break; }
                    Duration::from_millis(due)
                }
            };
            tokio::select! {
                _ = notify.notified() => { lock_retry = Duration::from_millis(100); }
                _ = tokio::time::sleep(pause) => {}
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
}
