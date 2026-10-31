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

}

#[cfg(test)]
mod tests {
    use super::*;

}
