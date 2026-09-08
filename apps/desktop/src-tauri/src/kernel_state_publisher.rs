//! Desktop push invalidations of committed Kernel state. No tool payloads and
//! no executable actions travel through this channel.
use crate::database::Database;
use std::{sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Mutex}, thread, time::Duration};
use tauri::{AppHandle, Emitter};

pub(crate) struct KernelStatePublisher {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl KernelStatePublisher {
    pub(crate) fn start(app: AppHandle, database: &Database) -> Result<Self, String> {
        Self::start_with(database.subscribe_kernel_changes(), move || {
            app.emit("fox://kernel-state-invalidated", fox_engine_protocol::KernelStateInvalidation { schema_version: 1 })
                .map_err(|_| ())
        })
    }

    fn start_with(receiver: mpsc::Receiver<()>, mut emit: impl FnMut() -> Result<(), ()> + Send + 'static) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new().name("fox-kernel-state".into()).spawn(move || {
            let mut pending = false;
            while !worker_stop.load(Ordering::SeqCst) {
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok(()) => pending = true,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                }
                if worker_stop.load(Ordering::SeqCst) { break; }
                if pending {
                    pending = emit().is_err();
                    // Rate-limit sustained commits and failed desktop delivery.
                    // The single queued bit retains any newer invalidations.
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }).map_err(|e| format!("cannot start Kernel state publisher: {e}"))?;
        Ok(Self { stop, worker: Mutex::new(Some(worker)) })
    }

    pub(crate) fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() { let _ = worker.join(); }
        }
    }
}

impl Drop for KernelStatePublisher {
    fn drop(&mut self) { self.shutdown(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_delivery_retries_and_shutdown_is_joined_and_idempotent() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (delivered, observed) = mpsc::channel();
        let mut calls = 0;
        let publisher = KernelStatePublisher::start_with(receiver, move || {
            calls += 1;
            if calls == 1 { return Err(()); }
            delivered.send(()).unwrap();
            Ok(())
        }).unwrap();
        sender.send(()).unwrap();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        publisher.shutdown();
        publisher.shutdown();
        assert!(sender.try_send(()).is_err());
    }
}
