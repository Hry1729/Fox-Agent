//! Isolated transport for a claimed Kernel batch; never shares Legacy workers.
use super::RuntimeCommand;
use crate::kernel::CancellationToken;
use fox_engine_protocol::{KernelBatchResumeFrame, KernelModelResponse, RunControlBinding, PROTOCOL_NAME, PROTOCOL_VERSION};
use serde_json::{json, Value};
pub(super) use crate::kernel_model_config::KernelModelConfig;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 1_048_576;

// Drop always terminates/reaps only the child created here, then joins bounded
// readers/writers. A timeout never leaves a pipe writer or model process alive.
struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    messages: Receiver<Result<Value, String>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        self.stdin.take();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        if let Some(reader) = self.reader.take() { let _ = reader.join(); }
    }
}

fn wait<T>(receiver: &Receiver<T>, token: &CancellationToken, deadline: Instant) -> Result<T, String> {
    loop {
        token.check()?;
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or("Kernel worker deadline exceeded; reconcile delivery")?;
        match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(value) => { token.check()?; return Ok(value); }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Kernel worker disconnected; reconcile delivery".into()),
        }
    }
}

impl Worker {
    fn spawn(runtime: &RuntimeCommand) -> Result<Self, String> {
        let mut command = Command::new(&runtime.program);
        if let Some(script) = &runtime.script { command.arg(script); }
        command.arg("--kernel-worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "could not start isolated Kernel worker")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped worker stdout");
        let (sender, messages) = mpsc::sync_channel(2);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let result = reader.by_ref().take((MAX_FRAME + 1) as u64).read_until(b'\n', &mut bytes);
                if matches!(result, Ok(0)) { break; }
                let message = if result.is_err() || bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
                    Err("invalid or oversized Kernel worker response".into())
                } else {
                    serde_json::from_slice(&bytes).map_err(|_| "invalid Kernel worker JSON".into())
                };
                let failed = message.is_err();
                // Unsolicited flooding fails closed, never blocks Drop's join.
                if sender.try_send(message).is_err() || failed { break; }
            }
        });
        Ok(Self { child, stdin, reader: Some(reader), writer: None, messages })
    }

    fn exchange(&mut self, request: Value, expected: &str, token: &CancellationToken, deadline: Instant) -> Result<Value, String> {
        let mut bytes = serde_json::to_vec(&request).map_err(|_| "invalid Kernel request")?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME { return Err("Kernel request exceeds frame limit".into()); }
        let mut stdin = self.stdin.take().ok_or("Kernel worker input unavailable")?;
        let (sender, written) = mpsc::sync_channel(1);
        self.writer = Some(thread::spawn(move || {
            let result = stdin.write_all(&bytes).and_then(|_| stdin.flush()).map_err(|_| "Kernel worker write failed");
            let _ = sender.send((stdin, result));
        }));
        let (stdin, result) = wait(&written, token, deadline)?;
        self.stdin = Some(stdin);
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        result?;
        let response = wait(&self.messages, token, deadline)??;
        if response["protocol"] != PROTOCOL_NAME || response["version"] != PROTOCOL_VERSION
            || response["kind"] != "response" || response["type"] != expected
            || response["requestId"] != request["id"]
            || ["runId", "conversationId", "runtimeSessionId"].iter().any(|key| response[key] != request[key]) {
            // No child message/provider error is copied into durable diagnostics.
            return Err("Kernel worker response identity/type mismatch; reconcile delivery".into());
        }
        Ok(response["payload"].clone())
    }
}

pub(crate) fn deliver(
    runtime: &RuntimeCommand, config: &KernelModelConfig, api_key: &str,
    binding: &RunControlBinding, frame: &KernelBatchResumeFrame, token: &CancellationToken,
    remaining_budget_ms: i64,
) -> Result<KernelModelResponse, String> {
    token.check()?;
    config.hash()?;
    binding.validate()?;
    frame.validate()?;
    if binding.engine_id != "pi" || binding.authority != fox_engine_protocol::ExecutionAuthority::Authoritative
        || config.execution_profile_id != binding.execution_profile_id {
        return Err("isolated Kernel model identity mismatch".into());
    }
    let budget = binding.budgets.model_request_ms.min(binding.budgets.run_execution_ms).min(remaining_budget_ms);
    if budget <= 0 { return Err("Kernel worker has no remaining execution budget".into()); }
    let deadline = Instant::now() + Duration::from_millis(budget.try_into().map_err(|_| "invalid model budget")?);
    let session_id = format!("kernel-once-{}", uuid::Uuid::new_v4());
    let request = |kind: &str, payload: Value| json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request",
        "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(), "type": kind,
        "runId": binding.run_id, "conversationId": binding.conversation_id, "runtimeSessionId": session_id, "payload": payload,
    });
    let mut initialization = serde_json::to_value(config).map_err(|_| "invalid Kernel configuration")?;
    initialization["modelService"]["apiKey"] = Value::String(api_key.into());
    let mut worker = Worker::spawn(runtime)?;
    let ready = worker.exchange(request("kernel.initialize", initialization), "kernel.ready", token, deadline)?;
    if ready["singleUse"] != true || ready["resourceExecution"] != false || ready["automaticReplay"] != false
        || ready["adapterVersion"] != crate::kernel_model_config::KERNEL_MODEL_ADAPTER {
        return Err("Kernel worker lacks isolated single-use capability".into());
    }
    let payload = worker.exchange(request("kernel.resume_batch", json!({"controlBinding":binding,"batchResume":frame})),
        "kernel.model_response", token, deadline)?;
    if payload["idempotencyKey"] != frame.idempotency_key || payload["checkpointSeq"] != frame.checkpoint_seq {
        return Err("Kernel worker returned a different delivery cursor".into());
    }
    let response: KernelModelResponse = serde_json::from_value(payload["response"].clone()).map_err(|_| "invalid Kernel model response")?;
    response.validate()?;
    if response.run_id != binding.run_id || response.turn_id != frame.turn_id || response.batch_id != frame.batch_id
        || response.checkpoint_seq != frame.checkpoint_seq { return Err("Kernel model response identity mismatch".into()); }
    token.check()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{CancellationPort, CancellationRegistry};

    fn script(source: &str) -> (std::path::PathBuf, RuntimeCommand) {
        let root = std::env::temp_dir().join(format!("fox-kernel-pipe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("worker.mjs");
        std::fs::write(&script, source).unwrap();
        (root, RuntimeCommand { program: "node".into(), script: Some(script) })
    }

    #[test]
    fn blocked_pipe_write_is_bounded_and_the_owned_child_is_reaped() {
        let (root, runtime) = script("setInterval(() => {}, 1000)");
        let registry = CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.run_token("r").unwrap();
        let start = Instant::now();
        {
            let mut worker = Worker::spawn(&runtime).unwrap();
            let result = worker.exchange(json!({"id":"r","payload":"x".repeat(900_000)}), "unused", &token,
                Instant::now() + Duration::from_millis(200));
            assert!(result.unwrap_err().contains("deadline"));
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_stdout_and_foreign_response_are_rejected_without_echoing_content() {
        for source in [
            "process.stdout.write('s'.repeat(1048580)+'\\n'); setInterval(() => {}, 1000)",
            "process.stdin.once('data', () => { process.stdout.write(JSON.stringify({kind:'response',type:'kernel.ready',requestId:'foreign',payload:'secret-provider-data'})+'\\n') }); setInterval(() => {}, 1000)",
        ] {
            let (root, runtime) = script(source);
            let registry = CancellationRegistry::default();
            registry.register_run("r").unwrap();
            let token = registry.run_token("r").unwrap();
            {
                let mut worker = Worker::spawn(&runtime).unwrap();
                let error = worker.exchange(json!({"id":"r"}), "kernel.ready", &token,
                    Instant::now() + Duration::from_secs(10)).unwrap_err();
                assert!(error.contains("oversized") || error.contains("mismatch"), "{error}");
                assert!(!error.contains("secret-provider-data"));
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancellation_during_pipe_wait_cleans_up_without_waiting_for_the_deadline() {
        let (root, runtime) = script("setInterval(() => {}, 1000)");
        let registry = CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.run_token("r").unwrap();
        let start = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                registry.request_run_cancel("r");
            });
            let mut worker = Worker::spawn(&runtime).unwrap();
            assert!(worker.exchange(json!({"id":"r"}), "kernel.ready", &token,
                Instant::now() + Duration::from_secs(30)).unwrap_err().contains("cancelled"));
        });
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(root).unwrap();
    }
}
