//! Isolated transport for a claimed Kernel batch; never shares Legacy workers.
use super::RuntimeCommand;
use crate::kernel::CancellationToken;
pub(super) use crate::kernel_model_config::KernelModelConfig;
use fox_engine_protocol::{
    KernelBatchResumeFrame, KernelModelResponse, RunControlBinding, PROTOCOL_NAME, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 1_048_576;

#[cfg(windows)]
#[path = "kernel_model_worker_job.rs"]
mod worker_job;

const SETTLED_FAILURE_PREFIX: &str = "kernel.settled_model_failure:";
pub(super) fn settled_failure(error: &str) -> Option<fox_engine_protocol::KernelModelFailure> {
    let evidence: fox_engine_protocol::KernelModelFailure = serde_json::from_str(error.strip_prefix(SETTLED_FAILURE_PREFIX)?).ok()?;
    evidence.validate().ok()?;
    Some(evidence)
}

pub(super) fn describe(
    runtime: &RuntimeCommand,
    binding: &RunControlBinding,
    model_service: Value,
    prompt: Value,
    supported_tools: Vec<&str>,
    token: &CancellationToken,
) -> Result<KernelModelConfig, String> {
    token.check()?;
    let request = json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request", "type": "kernel.describe",
        "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(),
        "runId": binding.run_id, "conversationId": binding.conversation_id,
        "runtimeSessionId": format!("kernel-description-{}", uuid::Uuid::new_v4()),
        "payload": {"executionProfileId":binding.execution_profile_id,"modelService":model_service,"prompt":prompt,"supportedTools":supported_tools},
    });
    let mut worker = Worker::spawn(runtime)?;
    let result = worker.exchange(
        request,
        "kernel.description",
        token,
        Instant::now() + Duration::from_secs(30),
    )?;
    if result["schemaVersion"] != 1 || result["executionProfileId"] != binding.execution_profile_id
    {
        return Err("Kernel description identity mismatch".into());
    }
    let config = KernelModelConfig {
        engine_id: binding.engine_id.clone(),
        native_adapter: crate::kernel_model_config::capture_native_adapter(&binding.engine_id)?,
        execution_profile_id: binding.execution_profile_id.clone(),
        model_service,
        system_prompt: result["systemPrompt"]
            .as_str()
            .ok_or("missing Kernel system prompt")?
            .into(),
        proposal_tools: result["proposalTools"]
            .as_array()
            .ok_or("missing Kernel tool descriptions")?
            .clone(),
    };
    config.hash()?;
    if config.proposal_tools.iter().any(|tool| !supported_tools.contains(&tool["name"].as_str().unwrap_or_default())) {
        return Err("Kernel description added unsupported resource tools".into());
    }
    Ok(config)
}

// Drop always terminates/reaps only the child created here, then joins bounded
// readers/writers. A timeout never leaves a pipe writer or model process alive.
struct Worker {
    child: Child,
    #[cfg(windows)]
    job: Option<worker_job::WorkerJob>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    messages: Receiver<Result<Value, String>>,
}

pub(super) type PreviewSink = dyn Fn(&fox_engine_protocol::KernelModelPreview) + Sync;

impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.job.take();
        let _ = self.child.kill();
        self.stdin.take();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        if let Some(reader) = self.reader.take() { let _ = reader.join(); }
    }
}

fn wait<T>(
    receiver: &Receiver<T>,
    token: &CancellationToken,
    deadline: Instant,
) -> Result<T, String> {
    loop {
        token.check()?;
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or("Kernel worker deadline exceeded; reconcile delivery")?;
        match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(value) => { token.check()?; return Ok(value); }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Kernel worker disconnected; reconcile delivery".into())
            }
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
        #[cfg(windows)]
        let job = match worker_job::WorkerJob::attach(&child) {
            Ok(job) => Some(job),
            Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped worker stdout");
        let (sender, messages) = mpsc::sync_channel(16);
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
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
            stdin,
            reader: Some(reader),
            writer: None,
            messages,
        })
    }

    fn exchange(
        &mut self,
        request: Value,
        expected: &str,
        token: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, String> {
        self.exchange_with_preview(request, expected, token, deadline, None)
    }

    fn exchange_with_preview(
        &mut self,
        request: Value,
        expected: &str,
        token: &CancellationToken,
        deadline: Instant,
        preview: Option<&PreviewSink>,
    ) -> Result<Value, String> {
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
        let mut revision = 0;
        loop {
            let response = wait(&self.messages, token, deadline)??;
            if response["protocol"] != PROTOCOL_NAME
                || response["version"] != PROTOCOL_VERSION
                || response["kind"] != "response"
                || response["requestId"] != request["id"]
                || ["runId", "conversationId", "runtimeSessionId"]
                    .iter()
                    .any(|key| response[key] != request[key])
            {
                // No child message/provider error is copied into durable diagnostics.
                return Err(
                    "Kernel worker response identity/type mismatch; reconcile delivery".into(),
                );
            }
            if response["type"] == "kernel.model_preview" {
                let Some(sink) = preview.filter(|_| expected == "kernel.model_response") else {
                    return Err("unexpected Kernel model preview".into());
                };
                let notice: fox_engine_protocol::KernelModelPreview =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid Kernel model preview")?;
                notice.validate()?;
                let frame = request["payload"]
                    .get("initialModel")
                    .or_else(|| request["payload"].get("batchResume"))
                    .ok_or("missing model preview cursor")?;
                let turn = frame
                    .get("turnId")
                    .or_else(|| frame.get("input").and_then(|input| input.get("turnId")));
                if notice.run_id != request["runId"]
                    || notice.conversation_id != request["conversationId"]
                    || Some(&Value::String(notice.turn_id.clone())) != turn
                    || notice.checkpoint_seq != frame["checkpointSeq"]
                    || notice.revision <= revision
                {
                    return Err("Kernel model preview identity/order mismatch".into());
                }
                revision = notice.revision;
                token.check()?;
                sink(&notice);
                continue;
            }
            if response["type"] == "kernel.model_failure" && expected == "kernel.model_response" {
                let failure: fox_engine_protocol::KernelModelFailure =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid settled Kernel model failure")?;
                failure.validate()?;
                let frame = request["payload"]
                    .get("initialModel")
                    .or_else(|| request["payload"].get("batchResume"))
                    .ok_or("missing failure request identity")?;
                let turn = frame
                    .get("turnId")
                    .or_else(|| frame.get("input").and_then(|input| input.get("turnId")));
                if failure.run_id != request["runId"]
                    || Some(&Value::String(failure.turn_id.clone())) != turn
                    || failure.checkpoint_seq != frame["checkpointSeq"]
                {
                    return Err("Kernel failure belongs to another model request".into());
                }
                return Err(format!(
                    "{SETTLED_FAILURE_PREFIX}{}",
                    serde_json::to_string(&failure).map_err(|_| "invalid failure")?
                ));
            }
            if response["type"] != expected {
                return Err(
                    "Kernel worker response identity/type mismatch; reconcile delivery".into(),
                );
            }
            return Ok(response["payload"].clone());
        }
    }
}

pub(crate) fn deliver(
    runtime: &RuntimeCommand, config: &KernelModelConfig, api_key: &str,
    binding: &RunControlBinding, frame: &KernelBatchResumeFrame, token: &CancellationToken,
    remaining_budget_ms: i64,
) -> Result<KernelModelResponse, String> {
    deliver_with_preview(
        runtime,
        config,
        api_key,
        binding,
        frame,
        token,
        remaining_budget_ms,
        None,
    )
}

pub(super) fn deliver_with_preview(
    runtime: &RuntimeCommand, config: &KernelModelConfig, api_key: &str,
    binding: &RunControlBinding, frame: &KernelBatchResumeFrame, token: &CancellationToken,
    remaining_budget_ms: i64, preview: Option<&PreviewSink>,
) -> Result<KernelModelResponse, String> {
    token.check()?;
    frame.validate()?;
    let payload = call_model(
        runtime,
        config,
        api_key,
        binding,
        "kernel.resume_batch",
        json!({"controlBinding":binding,"batchResume":frame}),
        token,
        remaining_budget_ms,
        preview,
    )?;
    if payload["idempotencyKey"] != frame.idempotency_key
        || payload["checkpointSeq"] != frame.checkpoint_seq
    {
        return Err("Kernel worker returned a different delivery cursor".into());
    }
    let response: KernelModelResponse = serde_json::from_value(payload["response"].clone()).map_err(|_| "invalid Kernel model response")?;
    response.validate()?;
    if response.run_id != binding.run_id || response.turn_id != frame.turn_id || response.batch_id != frame.batch_id
        || response.checkpoint_seq != frame.checkpoint_seq { return Err("Kernel model response identity mismatch".into()); }
    token.check()?;
    Ok(response)
}

pub(crate) fn deliver_initial(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    frame: &fox_engine_protocol::KernelInitialModelFrame,
    token: &CancellationToken,
    remaining_budget_ms: i64,
) -> Result<fox_engine_protocol::KernelInitialModelResponse, String> {
    deliver_initial_with_preview(
        runtime,
        config,
        api_key,
        binding,
        frame,
        token,
        remaining_budget_ms,
        None,
    )
}

pub(super) fn deliver_initial_with_preview(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    frame: &fox_engine_protocol::KernelInitialModelFrame,
    token: &CancellationToken,
    remaining_budget_ms: i64,
    preview: Option<&PreviewSink>,
) -> Result<fox_engine_protocol::KernelInitialModelResponse, String> {
    token.check()?;
    frame.validate()?;
    if frame.input.run_id != binding.run_id || frame.input.prompt_config_hash != config.hash()? {
        return Err("initial input configuration mismatch".into());
    }
    let payload = call_model(
        runtime,
        config,
        api_key,
        binding,
        "kernel.start_initial",
        json!({"controlBinding":binding,"initialModel":frame}),
        token,
        remaining_budget_ms,
        preview,
    )?;
    if payload["idempotencyKey"] != frame.idempotency_key
        || payload["checkpointSeq"] != frame.checkpoint_seq
    {
        return Err("initial delivery cursor mismatch".into());
    }
    let response: fox_engine_protocol::KernelInitialModelResponse =
        serde_json::from_value(payload["response"].clone())
            .map_err(|_| "invalid initial model response")?;
    response.validate()?;
    if response.run_id != binding.run_id || response.turn_id != frame.input.turn_id || response.checkpoint_seq != frame.checkpoint_seq {
        return Err("initial model response identity mismatch".into());
    }
    token.check()?;
    Ok(response)
}

fn call_model(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    kind: &str,
    mut request_payload: Value,
    token: &CancellationToken,
    remaining_budget_ms: i64,
    preview: Option<&PreviewSink>,
) -> Result<Value, String> {
    config.hash()?;
    binding.validate()?;
    if binding.engine_id != config.engine_id || binding.authority != fox_engine_protocol::ExecutionAuthority::Authoritative
        || config.execution_profile_id != binding.execution_profile_id {
        return Err("isolated Kernel model identity mismatch".into());
    }
    let budget = binding.budgets.model_request_ms.min(binding.budgets.run_execution_ms).min(remaining_budget_ms);
    if budget <= 0 { return Err("Kernel worker has no remaining execution budget".into()); }
    let deadline = Instant::now() + Duration::from_millis(budget.try_into().map_err(|_| "invalid model budget")?);
    let session_id = format!("kernel-once-{}", uuid::Uuid::new_v4());
    let request = |kind: &str, payload: Value| {
        json!({
            "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request",
            "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(), "type": kind,
            "runId": binding.run_id, "conversationId": binding.conversation_id, "runtimeSessionId": session_id, "payload": payload,
        })
    };
    let mut initialization =
        serde_json::to_value(config).map_err(|_| "invalid Kernel configuration")?;
    initialization["modelService"]["apiKey"] = Value::String(api_key.into());
    let mut worker = Worker::spawn(runtime)?;
    let ready = worker.exchange(
        request("kernel.initialize", initialization),
        "kernel.ready",
        token,
        deadline,
    )?;
    if ready["singleUse"] != true
        || ready["resourceExecution"] != false
        || ready["automaticReplay"] != false
        || ready["adapterVersion"] != config.adapter_version()?
    {
        return Err("Kernel worker lacks isolated single-use capability".into());
    }
    request_payload["streamPreview"] = json!(preview.is_some());
    let expected = if kind == "kernel.compact_context" {
        if ready["hostCompaction"] != true {
            return Err("Kernel worker lacks Host compaction capability".into());
        }
        "kernel.compaction_result"
    } else {
        "kernel.model_response"
    };
    worker.exchange_with_preview(
        request(kind, request_payload),
        expected,
        token,
        deadline,
        preview,
    )
}

pub(super) fn compact_context(
    runtime: &RuntimeCommand,
    frozen: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    input: &fox_engine_protocol::KernelCompactionRequest,
    token: &CancellationToken,
    remaining_ms: i64,
) -> Result<fox_engine_protocol::KernelCompactionResponse, String> {
    input.validate()?;
    if input.run_id != binding.run_id {
        return Err("compaction Run identity mismatch".into());
    }
    // The caller verifies the original frozen hash. This derived configuration
    // can only remove capabilities; credentials still arrive separately.
    let mut summarizer = frozen.clone();
    summarizer.system_prompt = crate::kernel_compaction::SUMMARY_PROMPT.into();
    summarizer.proposal_tools.clear();
    let output = frozen.model_service["maxOutputTokens"]
        .as_u64()
        .unwrap_or(8192)
        .min(2048);
    summarizer.model_service["maxOutputTokens"] = json!(output);
    let payload = call_model(
        runtime,
        &summarizer,
        api_key,
        binding,
        "kernel.compact_context",
        json!({"controlBinding":binding,"compaction":input}),
        token,
        remaining_ms,
        None,
    )?;
    let response: fox_engine_protocol::KernelCompactionResponse =
        serde_json::from_value(payload).map_err(|_| "invalid compaction response")?;
    response.validate_for(input)?;
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
        (
            root,
            RuntimeCommand {
                program: "node".into(),
                script: Some(script),
            },
        )
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
            let result = worker.exchange(
                json!({"id":"r","payload":"x".repeat(900_000)}),
                "unused",
                &token,
                Instant::now() + Duration::from_millis(200),
            );
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
