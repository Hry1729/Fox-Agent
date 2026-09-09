//! Own only this worker's process tree; closing the job also kills native engines.
use std::{os::windows::io::AsRawHandle, process::Child};
use windows::Win32::{Foundation::{CloseHandle, HANDLE}, System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
}};

pub(super) struct WorkerJob(HANDLE);

impl WorkerJob {
    pub(super) fn attach(child: &Child) -> Result<Self, String> {
        unsafe {
            let job = Self(CreateJobObjectW(None, None).map_err(|_| "could not create Kernel worker job")?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(job.0, JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _, std::mem::size_of_val(&limits) as u32)
                .map_err(|_| "could not bound Kernel worker process tree")?;
            AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()))
                .map_err(|_| "could not attach Kernel worker process tree")?;
            Ok(job)
        }
    }
}

impl Drop for WorkerJob {
    fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::{BufRead, BufReader, Write}, process::{Command, Stdio}};
    use windows::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE}};

    #[test]
    fn dropping_worker_job_reaps_its_native_descendant() {
        let mut child = Command::new("node").args(["-e",
            "process.stdin.once('data',()=>{const c=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore',windowsHide:true});console.log(c.pid)});setInterval(()=>{},1000)"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let job = WorkerJob::attach(&child).unwrap();
        child.stdin.as_mut().unwrap().write_all(b"start\n").unwrap();
        let mut pid = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut pid).unwrap();
        unsafe {
            let descendant = OpenProcess(PROCESS_SYNCHRONIZE, false, pid.trim().parse().unwrap()).unwrap();
            drop(job);
            assert_eq!(WaitForSingleObject(descendant, 5000), WAIT_OBJECT_0);
            let _ = CloseHandle(descendant);
        }
        child.wait().unwrap();
    }
}
