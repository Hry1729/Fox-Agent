//! CX-1 / KB-1: a child process Fox starts is owned by a KILL_ON_JOB_CLOSE Job
//! on Windows, so when Fox exits or crashes the OS reaps it and every process
//! it started. This reuses the Kernel model worker's Job implementation.
//! On other platforms there is no Job; the child's own stdin-close handling
//! (sidecar) or the caller's kill (workers) is what ends it.
use std::process::Child;

#[cfg(windows)]
#[path = "process_tree_job.rs"]
mod worker_job;

pub(crate) struct ProcessJob {
    #[cfg(windows)]
    _job: Option<worker_job::WorkerJob>,
}

impl ProcessJob {
    /// Attach `child` to a fresh KILL_ON_JOB_CLOSE Job. On Windows a failure is
    /// returned to the caller, which must not keep running an unowned child.
    pub(crate) fn attach(child: &Child, what: &str) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let job = worker_job::WorkerJob::attach(child)
                .map_err(|error| format!("[process.job_attach_failed] {what}: {error}"))?;
            Ok(Self { _job: Some(job) })
        }
        #[cfg(not(windows))]
        {
            let _ = (child, what);
            Ok(Self {})
        }
    }

    /// No Job (test doubles that construct a worker by hand).
    #[allow(dead_code)]
    pub(crate) fn none() -> Self {
        Self {
            #[cfg(windows)]
            _job: None,
        }
    }

    /// Attach, or kill the child and return the error: an unowned child must
    /// never outlive a failed start.
    pub(crate) fn attach_or_kill(child: &mut Child, what: &str) -> Result<Self, String> {
        match Self::attach(child, what) {
            Ok(job) => Ok(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
}

// A Job handle is a kernel object handle: owning it from another thread is
// sound, and nothing but Drop (CloseHandle) ever touches it.
#[cfg(windows)]
unsafe impl Send for ProcessJob {}
#[cfg(windows)]
unsafe impl Sync for ProcessJob {}
