//! Best-effort invalidation, never a decision or an event log. One queued bit
//! represents ALL committed Kernel changes, so a slow UI cannot lose a run's
//! terminal update to queue overflow. Consumers re-read authoritative SQLite.
use std::sync::{mpsc, Mutex};

#[derive(Default)]
pub(crate) struct KernelChanges {
    subscribers: Mutex<Vec<mpsc::SyncSender<()>>>,
}

impl KernelChanges {
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<()> {
        let (sender, receiver) = mpsc::sync_channel(1);
        // Initial invalidation closes the subscribe/reopen race.
        let _ = sender.try_send(());
        self.subscribers.lock().unwrap_or_else(|e| e.into_inner()).push(sender);
        receiver
    }

    pub(crate) fn committed(&self) {
        self.subscribers.lock().unwrap_or_else(|e| e.into_inner()).retain(|sender| {
            !matches!(sender.try_send(()), Err(mpsc::TrySendError::Disconnected(_)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_consumers_coalesce_all_runs_and_independent_subscribers_stay_live() {
        let changes = KernelChanges::default();
        let slow = changes.subscribe();
        let fast = changes.subscribe();
        fast.try_recv().unwrap();
        for _ in 0..10_000 { changes.committed(); }
        slow.try_recv().unwrap();
        assert!(slow.try_recv().is_err());
        fast.try_recv().unwrap();
        drop(slow);
        changes.committed();
        fast.try_recv().unwrap();
        assert_eq!(changes.subscribers.lock().unwrap().len(), 1);
    }
}
