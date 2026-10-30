use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, ResourceExecutor, TimeBudgets};
use rusqlite::params;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

const TEST: &str = "runtime_host::native_business_integration_tests::native_worker_child_collect_inherits_binding_and_preserves_sibling";
const CHILD_ENV: &str = "FOX_TEST_NATIVE_BUSINESS_CHILD";
const PARENT: &str = "[FOX_U09_PARENT]";
const NEXT: &str = "[FOX_U09_PARENT_NEXT]";
const CHILD: &str = "[FOX_U09_CHILD]";
const SIBLING: &str = "[FOX_U09_SIBLING]";
const JOB: &str = "[FOX_U09_COMPUTE_JOB]";
const JOB_TEST: &str = "runtime_host::native_business_integration_tests::native_compute_job_result_and_file_delivery_use_the_real_executor";
const JOB_ENV: &str = "FOX_TEST_NATIVE_COMPUTE_JOB_CHILD";
const CHILD_PROOF: &str = "U09 child proof from the actual project file";
const SIBLING_PROOF: &str = "U09 sibling proof from its own project file";
const PARENT_PROOF: &str = "U09 parent continuation proof";

fn wait_for(label: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn status(database: &Database, run: &str) -> String {
    database.with_connection(|connection| connection.query_row(
        "SELECT status FROM runs WHERE id=?1", [run], |row| row.get(0),
    )).unwrap()
}

fn receipt(database: &Database, run: &str, call: &str) -> Value {
    let raw: String = database.with_connection(|connection| connection.query_row(
        "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
        params![run, call], |row| row.get(0),
    )).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn native_run(database: &Database, conversation: &str, prompt: &str) -> StartRunResult {
    let started = database.create_run(conversation, prompt, None).unwrap();
    let mut budgets = TimeBudgets::continuous();
    budgets.restrict_execution_ms(60_000);
    let binding = database.freeze_kernel_run_control_for_engine(&started.run.id, "legacy", budgets, "pi").unwrap();
    assert_eq!(binding.authority, ExecutionAuthority::Authoritative);
    assert_eq!(binding.executor_route, Some(ExecutorRoute::PiNative));
    assert_eq!(binding.read_only_executor, ResourceExecutor::Rust);
    started
}

fn start(host: &RuntimeHost, started: &StartRunResult, prompt: &str, root: &Path) {
    if let Err(error) = host.start_run(started, prompt, &[]) {
        let diagnostics = host.diagnostics();
        panic!("native business start failed: {error}; root={}; state={}; last_error={:?}; stderr={:?}",
            root.display(), diagnostics.state, diagnostics.last_error, diagnostics.stderr_tail);
    }
}

fn pi_results(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).expect("the real Pi session must exist").lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter_map(|entry| entry.get("message").cloned())
        .filter(|message| message["role"] == "toolResult")
        .collect()
}

fn native_path(path: &Path) -> PathBuf {
    let text = path.to_str().expect("fixture runtime path must be Unicode");
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") { return PathBuf::from(format!(r"\\{unc}")); }
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text))
}

fn configure_runtime() {
    std::env::remove_var("FOX_RUNTIME_EXECUTABLE");
    std::env::remove_var("FOX_RUNTIME_SCRIPT");
    std::env::remove_var("FOX_RUNTIME_MODE");
    if let Some(value) = std::env::var_os("FOX_TEST_NATIVE_SIDECAR_BIN") {
        let path = PathBuf::from(value);
        assert!(path.is_absolute() && path.is_file(), "FOX_TEST_NATIVE_SIDECAR_BIN must name an existing absolute file");
        std::env::set_var("FOX_RUNTIME_EXECUTABLE", native_path(&path.canonicalize().unwrap()));
    } else {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime/src/pi-runtime.mjs");
        assert!(script.is_absolute() && script.is_file());
        std::env::set_var("FOX_RUNTIME_SCRIPT", native_path(&script));
    }
}

fn text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Array(parts) => Ok(parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n")),
        _ => Err("fixture provider received unsupported message content".into()),
    }
}

fn marker(body: &Value) -> Result<&'static str, String> {
    let latest = body["messages"].as_array().ok_or("provider request has no messages")?.iter().rev()
        .find(|message| message["role"] == "user").ok_or("provider request has no user message")?;
    let latest = text(&latest["content"])?;
    [NEXT, CHILD, SIBLING, PARENT, JOB].into_iter().find(|marker| latest.contains(*marker))
        .ok_or_else(|| "provider request has no isolated business marker".into())
}

fn tool_text(body: &Value, call: &str) -> Result<Option<String>, String> {
    let values = body["messages"].as_array().ok_or("provider request has no messages")?.iter()
        .filter(|message| message["role"] == "tool" && message["tool_call_id"] == call).collect::<Vec<_>>();
    if values.len() > 1 { return Err(format!("duplicate provider-visible tool result for {call}")); }
    values.first().map(|message| text(&message["content"])).transpose()
}

fn tool_json(body: &Value, call: &str) -> Result<Option<Value>, String> {
    tool_text(body, call)?.map(|raw| serde_json::from_str(&raw)
        .map_err(|error| format!("Host result for {call} is not intact JSON: {error}"))).transpose()
}

fn read_request(stream: &mut TcpStream) -> Result<Value, String> {
    stream.set_nonblocking(false).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(15))).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut scratch = [0_u8; 8192];
    let header_end = loop {
        let count = stream.read(&mut scratch).map_err(|error| error.to_string())?;
        if count == 0 { return Err("fixture HTTP request ended before headers".into()); }
        bytes.extend_from_slice(&scratch[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") { break index + 4; }
        if bytes.len() > 64 * 1024 { return Err("fixture HTTP headers exceed their limit".into()); }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let first = headers.lines().next().unwrap_or_default();
    if !first.starts_with("POST ") || !first.contains("/chat/completions ") { return Err(format!("unexpected fixture request: {first}")); }
    let length: usize = headers.lines().find_map(|line| line.split_once(':')
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())).ok_or("missing Content-Length")?;
    if length > 16 * 1024 * 1024 { return Err("fixture HTTP body exceeds its limit".into()); }
    while bytes.len() < header_end + length {
        let count = stream.read(&mut scratch).map_err(|error| error.to_string())?;
        if count == 0 { return Err("fixture HTTP body ended early".into()); }
        bytes.extend_from_slice(&scratch[..count]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + length]).map_err(|error| error.to_string())
}

enum Reply {
    Tool(&'static str, &'static str, Value),
    Text(String),
}

#[derive(Default)]
struct ProviderState { events: Vec<Value>, errors: Vec<String>, child_run_id: Option<String> }

struct ProviderShared {
    state: Mutex<ProviderState>,
    gates: Mutex<HashSet<&'static str>>,
    changed: Condvar,
    stop: AtomicBool,
    root: PathBuf,
}

impl ProviderShared {
    fn record(&self, marker: &str, phase: &str, body: &Value) -> Result<(), String> {
        let event = json!({"marker":marker,"phase":phase,"request":body});
        let mut state = self.state.lock().map_err(|_| "business provider state poisoned")?;
        state.events.push(event.clone());
        let encoded = crate::canonical_json::to_string(&event).map_err(|error| error.to_string())?;
        let mut file = OpenOptions::new().create(true).append(true).open(self.root.join("provider-requests.jsonl")).map_err(|error| error.to_string())?;
        writeln!(file, "{encoded}").map_err(|error| error.to_string())
    }

    fn wait_gate(&self, name: &'static str) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut gates = self.gates.lock().map_err(|_| "business provider gate poisoned")?;
        while !gates.contains(name) {
            if self.stop.load(Ordering::SeqCst) { return Err("business provider stopped".into()); }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(format!("fixture gate {name} timed out")); }
            gates = self.changed.wait_timeout(gates, remaining).map_err(|_| "business provider gate poisoned")?.0;
        }
        Ok(())
    }

    fn reply(&self, body: &Value) -> Result<Reply, String> {
        let marker = marker(body)?;
        if marker == JOB {
            let Some(started) = tool_json(body, "u09-job-start")? else {
                self.record(marker, "job-start-requested", body)?;
                return Ok(Reply::Tool("u09-job-start", "compute_job_start", json!({
                    "idempotencyKey":"u09-native-row-count","params":{
                        "projectPaths":["job-input.csv"],"processing":"chunked","profile":"standard",
                        "code":"let rows=0; function onChunk(c){rows+=c.rows.length;} function onFinish(){return {rows:rows};}",
                    },
                })));
            };
            let id = started["jobId"].as_str().filter(|id| !id.is_empty()).ok_or("actual Job start returned no jobId")?;
            let Some(current) = tool_json(body, "u09-job-status")? else {
                self.record(marker, "job-start-result", body)?;
                return Ok(Reply::Tool("u09-job-status", "compute_job_status", json!({"jobId":id,"waitMs":5000})));
            };
            if current["jobId"] != id || current["state"] != "completed" {
                return Err(format!("actual Job did not complete: {current}"));
            }
            let Some(result) = tool_json(body, "u09-job-result")? else {
                self.record(marker, "job-status-result", body)?;
                return Ok(Reply::Tool("u09-job-result", "compute_job_result", json!({"jobId":id,"offset":0,"limit":4096})));
            };
            if result["jobId"] != id || result["complete"] != true { return Err("actual Job result is incomplete or has a different identity".into()); }
            let computed: Value = serde_json::from_str(result["content"].as_str().ok_or("actual Job result omitted content")?)
                .map_err(|error| format!("actual Job content is not intact JSON: {error}"))?;
            if tool_text(body, "u09-job-write")?.is_none() {
                self.record(marker, "job-result-received", body)?;
                return Ok(Reply::Tool("u09-job-write", "write_file", json!({
                    "path":"job-result.txt","content":format!("rows={}\n",computed["result"]["rows"]),
                })));
            }
            let Some(declared) = tool_json(body, "u09-job-present")? else {
                self.record(marker, "job-write-result", body)?;
                return Ok(Reply::Tool("u09-job-present", "present_files", json!({
                    "files":[{"path":"job-result.txt","description":"Actual compute Job row count"}],
                })));
            };
            if declared["recorded"] != true || declared["declared"] != 1 || declared["files"][0]["path"] != "job-result.txt" {
                return Err(format!("actual file declaration did not complete: {declared}"));
            }
            self.record(marker, "job-files-declared", body)?;
            return Ok(Reply::Text(format!("job {id} computed {} rows and declared job-result.txt", computed["result"]["rows"])));
        }
        if marker == PARENT {
            if let Some(collected) = tool_json(body, "u09-collect")? {
                let children = collected["childRuns"].as_array().ok_or("collect did not return actual childRuns")?;
                if collected["allTerminal"] != true || children.len() != 1 || children[0]["status"] != "completed" {
                    return Err(format!("actual child collection is not a single completed result: {collected}"));
                }
                let id = self.state.lock().unwrap().child_run_id.clone().ok_or("collect has no preceding real start result")?;
                if children[0]["childRunId"] != id { return Err("collect returned a different child identity".into()); }
                let result = children[0]["resultText"].as_str().ok_or("collect returned no persisted child result")?;
                if !result.contains(CHILD_PROOF) { return Err("child result does not preserve its actual file proof".into()); }
                self.record(marker, "parent-collected", body)?;
                self.wait_gate("parent-final")?;
                return Ok(Reply::Text(format!("parent collected {id}: {result}")));
            }
            if let Some(started) = tool_json(body, "u09-start")? {
                let id = started["childRun"]["childRunId"].as_str().filter(|id| !id.is_empty())
                    .ok_or("start tool result omitted its actual childRunId")?.to_owned();
                {
                    let mut state = self.state.lock().unwrap();
                    if state.child_run_id.as_ref().is_some_and(|previous| previous != &id) { return Err("start returned a second child identity".into()); }
                    state.child_run_id = Some(id.clone());
                }
                self.record(marker, "parent-start-result", body)?;
                self.wait_gate("dispatch-collect")?;
                return Ok(Reply::Tool("u09-collect", "child_run_collect", json!({"childRunIds":[id],"waitMs":30000})));
            }
            self.record(marker, "parent-start", body)?;
            return Ok(Reply::Tool("u09-start", "child_run_start", json!({
                "mode":"worker","objective":format!("{CHILD} Read child-proof.txt and report only its actual proof."),
                "context":"Bounded isolated acceptance task; use the inherited project.","budget":{"maxDurationMs":30000},
            })));
        }
        let (call, path, proof, phase, gate) = match marker {
            CHILD => ("u09-child-read", "child-proof.txt", CHILD_PROOF, "child-read", Some("child-final")),
            SIBLING => ("u09-sibling-read", "sibling-proof.txt", SIBLING_PROOF, "sibling-read", Some("sibling-final")),
            NEXT => ("u09-parent-next-read", "parent-proof.txt", PARENT_PROOF, "parent-next-read", None),
            _ => return Err("unsupported business marker".into()),
        };
        if let Some(observed) = tool_text(body, call)? {
            let actual = observed.lines().next().ok_or("actual read result is empty")?;
            if actual != proof { return Err(format!("{call} did not return its real project proof: {observed}")); }
            self.record(marker, phase, body)?;
            if let Some(gate) = gate { self.wait_gate(gate)?; }
            Ok(Reply::Text(format!("read result: {actual}")))
        } else {
            self.record(marker, "read-requested", body)?;
            Ok(Reply::Tool(call, "read", json!({"path":path})))
        }
    }
}

fn send_reply(stream: &mut TcpStream, reply: Reply) -> Result<(), String> {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").map_err(|error| error.to_string())?;
    let (delta, reason) = match reply {
        Reply::Tool(id, name, input) => (json!({"role":"assistant","tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":crate::canonical_json::to_string(&input).map_err(|error| error.to_string())?}}]}), "tool_calls"),
        Reply::Text(content) => (json!({"role":"assistant","content":content}), "stop"),
    };
    for (delta, finish) in [(delta, Value::Null), (json!({}), json!(reason))] {
        let frame = json!({"id":"business-acceptance","object":"chat.completion.chunk","created":1,"model":"fox-u09-local",
            "choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        writeln!(stream, "data: {}\n", crate::canonical_json::to_string(&frame).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    }
    let usage = json!({"id":"business-acceptance","object":"chat.completion.chunk","created":1,"model":"fox-u09-local",
        "choices":[],"usage":{"prompt_tokens":128,"completion_tokens":32,"total_tokens":160}});
    write!(stream, "data: {}\n\ndata: [DONE]\n\n", crate::canonical_json::to_string(&usage).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

struct Provider { endpoint: String, shared: Arc<ProviderShared>, join: Option<thread::JoinHandle<()>> }

impl Provider {
    fn start(root: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let shared = Arc::new(ProviderShared { state:Mutex::new(ProviderState::default()), gates:Mutex::new(HashSet::new()),
            changed:Condvar::new(), stop:AtomicBool::new(false), root:root.to_path_buf() });
        let server = shared.clone();
        let join = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !server.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let shared = server.clone();
                        handlers.push(thread::spawn(move || {
                            let result = read_request(&mut stream).and_then(|body| {
                                shared.record("incoming", "http-received", &body)?;
                                shared.reply(&body)
                            }).and_then(|reply| send_reply(&mut stream, reply));
                            if let Err(error) = result {
                                if !shared.stop.load(Ordering::SeqCst) {
                                    shared.state.lock().unwrap().errors.push(error.clone());
                                    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(shared.root.join("provider-errors.jsonl")) {
                                        if let Ok(encoded) = crate::canonical_json::to_string(&json!({"error":error})) { let _ = writeln!(file, "{encoded}"); }
                                    }
                                }
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
                    Err(error) => { server.state.lock().unwrap().errors.push(error.to_string()); break; }
                }
            }
            for handler in handlers { let _ = handler.join(); }
        });
        Self { endpoint, shared, join:Some(join) }
    }

    fn observed(&self, marker: &str, phase: &str) -> bool {
        self.shared.state.lock().unwrap().events.iter().any(|event| event["marker"] == marker && event["phase"] == phase)
    }

    fn release(&self, gate: &'static str) {
        self.shared.gates.lock().unwrap().insert(gate);
        self.shared.changed.notify_all();
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        if let Some(join) = self.join.take() { let _ = join.join(); }
    }
}

struct HostCleanup(RuntimeHost);
impl Drop for HostCleanup { fn drop(&mut self) { let _ = self.0.shutdown(); } }

#[test]
fn native_worker_child_collect_inherits_binding_and_preserves_sibling() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let mut child = Command::new(std::env::current_exe().unwrap()).args([TEST,"--exact","--nocapture","--test-threads=1"])
            .env(CHILD_ENV,"1").env("FOX_KERNEL_MODE","authoritative").env("FOX_EXECUTOR_ROUTE","pi_native")
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(150);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("native business child exceeded 150 seconds"); }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "child stdout: {}\nchild stderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    configure_runtime();
    let parent = std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = parent.join(format!("fox-native-business-{}", uuid::Uuid::new_v4()));
    println!("native business fixture: {}", root.display());
    for name in ["sessions","attachments","skills","parent-project","sibling-project"] { std::fs::create_dir_all(root.join(name)).unwrap(); }
    std::fs::write(root.join("parent-project/child-proof.txt"), CHILD_PROOF).unwrap();
    std::fs::write(root.join("parent-project/parent-proof.txt"), PARENT_PROOF).unwrap();
    std::fs::write(root.join("sibling-project/sibling-proof.txt"), SIBLING_PROOF).unwrap();
    let provider = Provider::start(&root);
    let database = Database::open(root.join("facts.db")).unwrap();
    database.save_model_service(&crate::database::ModelServiceRecord {
        name:"fox-u09-local".into(),base_url:provider.endpoint.clone(),model_id:"fox-u09-local".into(),api_type:"openai-completions".into(),
        context_window:131072,max_output_tokens:512,supports_image_input:false,enabled:true,credential_configured:false,
        connection_type:"local".into(),last_status:"unknown".into(),last_latency_ms:None,last_checked_at:None,created_at:1,updated_at:1,
    }).unwrap();
    let conversation = database.create_conversation(database.default_agent_id(),Some("U09 parent"),root.join("parent-project").to_str(),Some("allow")).unwrap();
    let sibling_conversation = database.create_conversation(database.default_agent_id(),Some("U09 sibling"),root.join("sibling-project").to_str(),Some("allow")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create=false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let host = RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    let _cleanup = HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));

    let sibling = native_run(&database,&sibling_conversation.id,SIBLING);
    start(&host,&sibling,SIBLING,&root);
    wait_for("sibling's real read while its final response remains held",|| {
        if status(&database,&sibling.run.id)=="failed" {
            let failure:(Option<String>,Option<String>) = database.with_connection(|connection| connection.query_row(
                "SELECT error_code,error_message FROM runs WHERE id=?1",[&sibling.run.id],|row| Ok((row.get(0)?,row.get(1)?)),
            )).unwrap();
            let diagnostics=host.diagnostics();
            panic!("sibling failed before its real read: {failure:?}; root={}; provider={:?}; runtime_error={:?}; stderr={:?}",
                root.display(),provider.shared.state.lock().unwrap().errors,diagnostics.last_error,diagnostics.stderr_tail);
        }
        provider.observed(SIBLING,"sibling-read")
    });
    let sibling_session = database.get_runtime_session(&sibling_conversation.id).unwrap().unwrap();
    let pid = host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id();
    let parent_run = native_run(&database,&conversation.id,PARENT);
    start(&host,&parent_run,PARENT,&root);
    wait_for("provider receives the actual Host child start result",|| provider.observed(PARENT,"parent-start-result"));
    let child_id = provider.shared.state.lock().unwrap().child_run_id.clone().unwrap();
    let child = database.child_run(&child_id).unwrap().expect("Host created the exact returned child");
    assert_ne!(child_id,parent_run.run.id);
    assert_ne!(child_id,sibling.run.id);
    assert_ne!(child.child_conversation_id,conversation.id);
    assert_ne!(child.child_conversation_id,sibling_conversation.id);
    assert_eq!(child.parent_run_id,parent_run.run.id);
    assert_eq!(child.status,"queued");
    assert_eq!(status(&database,&child_id),"queued");
    assert!(!provider.observed(CHILD,"read-requested"),"the child cannot take a third execution slot before collect");
    assert_eq!(receipt(&database,&parent_run.run.id,"u09-start")["result"]["childRun"]["childRunId"],child_id);
    {
        let state = host.state.lock().unwrap();
        let worker = state.worker.as_ref().unwrap();
        assert_eq!(worker.child.lock().unwrap().id(),pid);
        assert_eq!(worker.sessions.occupied(),2);
        assert!(worker.sessions.by_run(&parent_run.run.id).is_some_and(|run| !run.waiting));
        assert!(worker.sessions.by_run(&sibling.run.id).is_some_and(|run| !run.waiting));
        assert!(worker.sessions.by_run(&child_id).is_none());
    }
    provider.release("dispatch-collect");
    wait_for("collect releases its parent slot and the queued child performs a real Host read",|| provider.observed(CHILD,"child-read"));
    assert_eq!(status(&database,&child_id),"running");
    assert_eq!(status(&database,&sibling.run.id),"running");
    let parent_session = database.get_runtime_session(&conversation.id).unwrap().unwrap();
    let child_session = database.get_runtime_session(&child.child_conversation_id).unwrap().unwrap();
    assert_eq!(HashSet::from([parent_session.runtime_session_id.clone(),child_session.runtime_session_id.clone(),sibling_session.runtime_session_id.clone()]).len(),3);
    assert_eq!(HashSet::from([parent_session.native_session_path.clone(),child_session.native_session_path.clone(),sibling_session.native_session_path.clone()]).len(),3);
    {
        let state = host.state.lock().unwrap();
        let worker = state.worker.as_ref().unwrap();
        assert_eq!(worker.child.lock().unwrap().id(),pid);
        assert_eq!(worker.sessions.occupied(),2);
        assert_eq!(worker.sessions.active().len(),3);
        assert!(worker.sessions.by_run(&parent_run.run.id).is_some_and(|run| run.waiting));
        assert!(worker.sessions.by_run(&sibling.run.id).is_some_and(|run| !run.waiting));
        let active_child = worker.sessions.by_run(&child_id).unwrap();
        assert!(!active_child.waiting);
        assert_eq!(active_child.conversation_id,child.child_conversation_id);
        assert_eq!(active_child.runtime_session_id,child_session.runtime_session_id);
    }
    let parent_binding = database.run_control_binding(&parent_run.run.id).unwrap().unwrap();
    let child_binding = database.run_control_binding(&child_id).unwrap().unwrap();
    let mut expected = parent_binding.clone();
    expected.run_id = child_id.clone();
    expected.conversation_id = child.child_conversation_id.clone();
    expected.budgets.restrict_execution_ms(30_000);
    assert_eq!(crate::canonical_json::to_string(&child_binding).unwrap(),crate::canonical_json::to_string(&expected).unwrap());
    assert_eq!(child.budget.max_duration_ms,30_000);
    assert_eq!(child.budget.max_total_tokens,32_000);
    assert_eq!(child.budget.max_output_tokens,512);
    assert_eq!(child.budget.max_tool_calls,16);
    let (budget_raw,limits_raw):(String,String) = database.with_connection(|connection| connection.query_row(
        "SELECT r.budget_json,l.limits_json FROM runs r JOIN native_model_budget_limits l ON l.run_id=r.id WHERE r.id=?1",[&child_id],
        |row| Ok((row.get(0)?,row.get(1)?)),
    )).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&budget_raw).unwrap(),serde_json::to_value(&child.budget).unwrap());
    let limits: Value = serde_json::from_str(&limits_raw).unwrap();
    assert_eq!(limits["total"],32_000);
    assert_eq!(limits["output"],512);
    assert_eq!(limits["durationMs"],30_000);
    assert_eq!(limits["runtimeSessionId"],child_session.runtime_session_id);
    assert_eq!(limits["contextGeneration"],child_session.context_generation);
    assert_eq!(receipt(&database,&child_id,"u09-child-read")["result"]["content"][0]["text"],CHILD_PROOF);
    assert_eq!(receipt(&database,&sibling.run.id,"u09-sibling-read")["result"]["content"][0]["text"],SIBLING_PROOF);
    assert_eq!(database.get_runtime_tool_call(&parent_run.run.id,"u09-collect").unwrap().unwrap().status,"running");
    provider.release("child-final");
    wait_for("the parent's model receives the actual collected child result",|| provider.observed(PARENT,"parent-collected"));
    wait_for("child cleanup retires only the child",|| host.child_hosts.lock().unwrap().get(&child_id).is_none()
        && host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&child_id).is_none());
    assert_eq!(status(&database,&child_id),"completed");
    assert_eq!(status(&database,&parent_run.run.id),"running");
    assert_eq!(status(&database,&sibling.run.id),"running");
    {
        let state = host.state.lock().unwrap();
        let worker = state.worker.as_ref().unwrap();
        assert_eq!(worker.child.lock().unwrap().id(),pid);
        assert_eq!(worker.sessions.occupied(),2);
        assert!(worker.sessions.by_run(&parent_run.run.id).is_some_and(|run| !run.waiting));
        assert!(worker.sessions.by_run(&sibling.run.id).is_some_and(|run| !run.waiting));
    }
    let collected = receipt(&database,&parent_run.run.id,"u09-collect");
    assert_eq!(collected["isError"],false);
    assert_eq!(collected["result"]["allTerminal"],true);
    let records = collected["result"]["childRuns"].as_array().unwrap();
    assert_eq!(records.len(),1);
    assert_eq!(records[0]["childRunId"],child_id);
    assert_eq!(records[0]["status"],"completed");
    let terminal_child = database.child_run(&child_id).unwrap().unwrap();
    assert_eq!(records[0]["resultText"],terminal_child.result_text.as_deref().unwrap());
    assert!(terminal_child.result_text.as_deref().unwrap().contains(CHILD_PROOF));
    provider.release("sibling-final");
    provider.release("parent-final");
    wait_for("parent and sibling finish without replacing the shared process",|| [&parent_run.run.id,&sibling.run.id].iter().all(|run|
        status(&database,run)=="completed" && host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(run).is_none()));
    let parent_results = pi_results(&parent_session.native_path().unwrap());
    assert_eq!(parent_results.iter().filter_map(|message| message["toolCallId"].as_str()).collect::<Vec<_>>(),vec!["u09-start","u09-collect"]);
    assert_eq!(pi_results(&child_session.native_path().unwrap()).iter().filter_map(|message| message["toolCallId"].as_str()).collect::<Vec<_>>(),vec!["u09-child-read"]);
    assert_eq!(pi_results(&sibling_session.native_path().unwrap()).iter().filter_map(|message| message["toolCallId"].as_str()).collect::<Vec<_>>(),vec!["u09-sibling-read"]);
    let next = native_run(&database,&conversation.id,NEXT);
    start(&host,&next,NEXT,&root);
    wait_for("the original parent session continues after collecting its child",|| status(&database,&next.run.id)=="completed"
        && host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&next.run.id).is_none());
    assert_eq!(receipt(&database,&next.run.id,"u09-parent-next-read")["result"]["content"][0]["text"],PARENT_PROOF);
    let continued = database.get_runtime_session(&conversation.id).unwrap().unwrap();
    assert_eq!(continued.runtime_session_id,parent_session.runtime_session_id);
    assert_eq!(continued.context_generation,parent_session.context_generation);
    assert_eq!(continued.native_session_path,parent_session.native_session_path);
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    let counts:(i64,i64,i64,i64) = database.with_connection(|connection| connection.query_row(
        "SELECT (SELECT COUNT(*) FROM child_run_delegations WHERE parent_run_id=?1),
                (SELECT COUNT(*) FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id='u09-start'),
                (SELECT COUNT(*) FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id='u09-collect'),
                (SELECT COUNT(*) FROM tool_calls WHERE run_id=?2 AND runtime_tool_call_id='u09-child-read')",
        params![parent_run.run.id,child_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    )).unwrap();
    assert_eq!(counts,(1,1,1,1));
    let provider_state = provider.shared.state.lock().unwrap();
    std::fs::write(root.join("business-result.json"),crate::canonical_json::to_string(&json!({
        "parentRunId":parent_run.run.id,"childRunId":child_id,"siblingRunId":sibling.run.id,"continuationRunId":next.run.id,
        "runtimePid":pid,"collected":collected,"childBinding":child_binding,"nativeModelBudget":limits,
        "delegationAndCallCounts":[counts.0,counts.1,counts.2,counts.3],"providerErrors":&provider_state.errors,
        "scope":"real Host + disk SQLite + Pi + local scripted HTTP model; no external model, UI, Job or notification acceptance",
    })).unwrap()).unwrap();
    assert!(provider_state.errors.is_empty(),"provider failures: {:?}",provider_state.errors);
    drop(provider_state);
    host.shutdown().unwrap();
    println!("native business evidence retained at {}",root.display());
}

#[test]
fn native_compute_job_result_and_file_delivery_use_the_real_executor() {
    use sha2::{Digest, Sha256};

    if std::env::var_os(JOB_ENV).is_none() {
        let mut child = Command::new(std::env::current_exe().unwrap()).args([JOB_TEST,"--exact","--nocapture","--test-threads=1"])
            .env(JOB_ENV,"1").env("FOX_KERNEL_MODE","authoritative").env("FOX_EXECUTOR_ROUTE","pi_native")
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(150);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("native compute Job child exceeded 150 seconds"); }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return;
    }
    configure_runtime();
    let parent = std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = parent.join(format!("fox-native-compute-job-{}",uuid::Uuid::new_v4()));
    println!("native compute Job fixture: {}",root.display());
    for name in ["sessions","attachments","skills","project"] { std::fs::create_dir_all(root.join(name)).unwrap(); }
    let csv: &[u8] = b"value\n10\n20\n30\n";
    std::fs::write(root.join("project/job-input.csv"),csv).unwrap();
    let provider = Provider::start(&root);
    let database = Database::open(root.join("facts.db")).unwrap();
    database.save_model_service(&crate::database::ModelServiceRecord {
        name:"fox-u09-local".into(),base_url:provider.endpoint.clone(),model_id:"fox-u09-local".into(),api_type:"openai-completions".into(),
        context_window:131072,max_output_tokens:512,supports_image_input:false,enabled:true,credential_configured:false,
        connection_type:"local".into(),last_status:"unknown".into(),last_latency_ms:None,last_checked_at:None,created_at:1,updated_at:1,
    }).unwrap();
    let conversation = database.create_conversation(database.default_agent_id(),Some("U09 compute Job"),root.join("project").to_str(),Some("allow")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create=false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let host = RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    let _cleanup = HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));
    let started = native_run(&database,&conversation.id,JOB);
    start(&host,&started,JOB,&root);
    wait_for("the actual compute Job result completes the native Run",|| {
        if status(&database,&started.run.id)=="failed" {
            let failure:(Option<String>,Option<String>) = database.with_connection(|connection| connection.query_row(
                "SELECT error_code,error_message FROM runs WHERE id=?1",[&started.run.id],|row| Ok((row.get(0)?,row.get(1)?)),
            )).unwrap();
            let diagnostics = host.diagnostics();
            panic!("native Job failed: {failure:?}; root={}; provider={:?}; runtime_error={:?}; stderr={:?}",
                root.display(),provider.shared.state.lock().unwrap().errors,diagnostics.last_error,diagnostics.stderr_tail);
        }
        status(&database,&started.run.id)=="completed"
            && host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&started.run.id).is_none()
    });
    let jobs = database.kernel_jobs_for_run(&started.run.id).unwrap();
    assert_eq!(jobs.len(),1);
    let job = &jobs[0];
    assert_eq!(job.state,crate::database::JobState::Completed);
    assert_eq!(job.attempts,1);
    assert_eq!(job.run_id,started.run.id);
    assert_eq!(job.conversation_id,conversation.id);
    assert_eq!(job.kind,"attachment_compute");
    let computed = database.kernel_job_result_value(&conversation.id,&job.job_id).unwrap();
    assert_eq!(computed["computedBy"],"fox-quickjs");
    assert_eq!(computed["rowsProcessed"],4);
    assert_eq!(computed["result"]["rows"],4);
    let bytes = crate::canonical_json::to_vec(&computed).unwrap();
    assert_eq!(job.result_bytes,Some(bytes.len() as u64));
    assert_eq!(job.result_sha256,Some(format!("sha256:{}",hex::encode(Sha256::digest(&bytes)))));
    assert_eq!(std::fs::read(root.join("project/job-input.csv")).unwrap().as_slice(),csv);
    let session = database.get_runtime_session(&conversation.id).unwrap().unwrap();
    let results = pi_results(&session.native_path().unwrap());
    let calls = ["u09-job-start","u09-job-status","u09-job-result","u09-job-write","u09-job-present"];
    assert_eq!(results.iter().filter_map(|message| message["toolCallId"].as_str()).collect::<Vec<_>>(),calls);
    let state = provider.shared.state.lock().unwrap();
    for ((call,phase),message) in calls[..3].iter().copied().zip(["job-start-result","job-status-result","job-result-received"]).zip(&results) {
        let stored = receipt(&database,&started.run.id,call);
        assert_eq!(stored["isError"],false,"{call}: {stored}");
        assert_eq!(message["isError"],false);
        assert_eq!(message["content"],stored["result"]["content"]);
        let events = state.events.iter().filter(|event| event["marker"]==JOB && event["phase"]==phase).collect::<Vec<_>>();
        assert_eq!(events.len(),1,"the model consumes {call} exactly once");
        let observed = tool_json(&events[0]["request"],call).unwrap().unwrap();
        let committed: Value = serde_json::from_str(stored["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(observed,committed);
        assert_eq!(observed["jobId"],job.job_id);
    }
    let result: Value = serde_json::from_str(receipt(&database,&started.run.id,"u09-job-result")["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["reference"],serde_json::to_value(&job.result_ref).unwrap());
    assert_eq!(result["sha256"],serde_json::to_value(&job.result_sha256).unwrap());
    assert_eq!(result["originalBytes"],bytes.len());
    assert_eq!(result["complete"],true);
    assert!(result["nextOffset"].is_null());
    assert_eq!(result["content"].as_str().unwrap().as_bytes(),bytes);
    for (call,phase) in [("u09-job-write","job-write-result"),("u09-job-present","job-files-declared")] {
        let stored = receipt(&database,&started.run.id,call);
        assert_eq!(stored["isError"],false,"{call}: {stored}");
        let message = results.iter().find(|message| message["toolCallId"]==call).unwrap();
        assert_eq!(message["isError"],false);
        assert_eq!(message["content"],stored["result"]["content"]);
        assert_eq!(state.events.iter().filter(|event| event["marker"]==JOB && event["phase"]==phase).count(),1);
    }
    assert_eq!(std::fs::read(root.join("project/job-result.txt")).unwrap(),b"rows=4\n");
    let declarations: Vec<(String,String,String,String,i64)> = database.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT conversation_id,run_id,turn_call_id,display_name,byte_size FROM artifacts WHERE declared_at IS NOT NULL",
        )?;
        let rows = statement.query_map([],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?
            .collect();
        rows
    }).unwrap();
    assert_eq!(declarations,vec![(conversation.id.clone(),started.run.id.clone(),"u09-job-present".into(),"job-result.txt".into(),7)]);
    let counts:(i64,i64) = database.with_connection(|connection| connection.query_row(
        "SELECT (SELECT COUNT(*) FROM tool_calls WHERE run_id=?1),(SELECT COUNT(*) FROM kernel_tool_calls WHERE run_id=?1)",
        [&started.run.id],|row| Ok((row.get(0)?,row.get(1)?)),
    )).unwrap();
    assert_eq!(counts,(5,5));
    assert!(state.errors.is_empty(),"provider failures: {:?}",state.errors);
    std::fs::write(root.join("job-result.json"),crate::canonical_json::to_string(&json!({
        "runId":started.run.id,"conversationId":conversation.id,"job":job,"result":result,"toolCallCounts":[counts.0,counts.1],
        "fileDeclarations":declarations,
        "scope":"real Host + disk SQLite + Pi + QuickJS CSV executor + local scripted HTTP model; no external model or UI",
    })).unwrap()).unwrap();
    drop(state);
    host.shutdown().unwrap();
    println!("native compute Job evidence retained at {}",root.display());
}
