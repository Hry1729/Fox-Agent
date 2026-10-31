//! Offline S5 measurement of the PiNative route. Run the exact ignored test
//! with --test-threads=1. Runs enter RuntimeHost::start_run; the shared
//! two-slot queue belongs to this fixture, not to Host scheduling. The Kernel
//! comparison cells left with the Kernel loop (B1).

use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, ResourceExecutor, TimeBudgets};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Barrier;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{Listener, Manager};

const TEST_NAME: &str = "runtime_host::native_performance_tests::native_offline_performance";
const CHILD: &str = "FOX_NATIVE_PERF_CHILD";
const MODEL: &str = "fox-offline-paired-performance";
const FIXTURE_CAPACITY: usize = 2;
const POLL_MS: u64 = 2;
const RESOURCE_MS: u64 = 50;

fn repository() -> PathBuf {
    standard_launch_path(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap())
}

fn standard_launch_path(path: &Path) -> PathBuf {
    // Same conversion as native_host_integration_tests' nativePath adapter.
    let value = path.to_str().expect("performance fixture paths must be Unicode");
    if let Some(tail) = value.strip_prefix(r"\\?\UNC\") { PathBuf::from(format!(r"\\{tail}")) }
    else if let Some(tail) = value.strip_prefix(r"\\?\") { PathBuf::from(tail) }
    else { path.to_owned() }
}

fn env_count(name: &str, fallback: usize) -> usize {
    std::env::var(name).map(|value| value.parse::<usize>().expect("performance sample count must be an integer"))
        .unwrap_or(fallback)
}

fn elapsed(clock: Instant) -> f64 { clock.elapsed().as_secs_f64() * 1000.0 }
fn digest(bytes: &[u8]) -> String { format!("sha256:{}", hex::encode(Sha256::digest(bytes))) }
fn write_json(path: &Path, value: &impl Serialize) {
    std::fs::write(path, crate::canonical_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn source_identity() -> Value {
    let root = repository();
    let files = [
        "apps/desktop/src-tauri/src/runtime_host/mod.rs",
        "apps/desktop/src-tauri/src/runtime_host/kernel_host.rs",
        "apps/desktop/src-tauri/src/runtime_host/sidecar_sessions.rs",
        "apps/desktop/src-tauri/src/runtime_host/native_tool_batch.rs",
        "services/agent-runtime/src/pi-runtime.mjs",
        "services/agent-runtime/src/pi-adapter.mjs",
        "services/agent-runtime/src/pi-native-tool-bridge.mjs",
        "services/agent-runtime/src/pi-native-recovery.mjs",
        "services/agent-runtime/src/model-usage-runtime.mjs",
        "services/agent-runtime/package.json", "pnpm-lock.yaml",
    ];
    let hashes: serde_json::Map<String, Value> = files.into_iter().map(|file| {
        (file.into(), json!(digest(&std::fs::read(root.join(file)).unwrap())))
    }).collect();
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(&root).output().ok()
        .filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    json!({"head":git(&["rev-parse", "HEAD"]), "gitStatus":git(&["status", "--porcelain"]),
        "testBinary":std::env::current_exe().unwrap(),
        "testBinarySha256":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),
        "compiledHostSha256":digest(include_bytes!("mod.rs")),
        "compiledFixtureSha256":digest(include_bytes!("native_performance_tests.rs")), "files":hashes})
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    run_id: String,
    conversation_id: String,
    phase: String,
    sample: usize,
    lane: usize,
    submitted_ms: Option<f64>,
    fixture_admitted_ms: Option<f64>,
    host_active_ms: Option<f64>,
    host_call_returned_ms: Option<f64>,
    first_body_ms: Option<f64>,
    terminal_ms: Option<f64>,
    released_ms: Option<f64>,
    cancel_requested_ms: Option<f64>,
    terminal_status: Option<String>,
    start_error: Option<String>,
}
type Observations = Arc<Mutex<HashMap<String, Observation>>>;

impl Observation {
    fn new(started: &StartRunResult, phase: &str, sample: usize, lane: usize) -> Self {
        Self { run_id:started.run.id.clone(), conversation_id:started.run.conversation_id.clone(),
            phase:phase.into(), sample, lane, submitted_ms:None, fixture_admitted_ms:None, host_active_ms:None,
            host_call_returned_ms:None, first_body_ms:None, terminal_ms:None, released_ms:None,
            cancel_requested_ms:None, terminal_status:None, start_error:None }
    }
}

#[derive(Default)]
struct FixtureQueue {
    state: Mutex<(usize, usize)>,
    changed: Condvar,
}
struct FixturePermit(Arc<FixtureQueue>);
impl FixtureQueue {
    fn acquire(self: &Arc<Self>, ticket: usize) -> FixturePermit {
        let mut state = self.state.lock().unwrap();
        while state.0 >= FIXTURE_CAPACITY || state.1 != ticket { state = self.changed.wait(state).unwrap(); }
        state.0 += 1;
        state.1 += 1;
        self.changed.notify_all();
        FixturePermit(self.clone())
    }
}
impl Drop for FixturePermit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.0 -= 1;
        self.0.changed.notify_all();
    }
}

fn terminal(status: &str) -> bool { matches!(status, "completed" | "cancelled" | "failed" | "interrupted") }
fn ownership_released(host: &RuntimeHost, run: &str) -> bool {
    let state = host.state.lock().unwrap();
    !state.native_run_leases.contains_key(run) && !state.dispatching_runs.contains_key(run)
        && state.worker.as_ref().is_none_or(|worker| worker.sessions.by_run(run).is_none())
}

fn start_observer(host: RuntimeHost, observations: Observations, clock: Instant,
    stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let ids: Vec<_> = observations.lock().unwrap().values()
                .filter(|row| row.fixture_admitted_ms.is_some() && row.released_ms.is_none())
                .map(|row| row.run_id.clone()).collect();
            for id in ids {
                let active = {
                    let state = host.state.lock().unwrap();
                    state.worker.as_ref().is_some_and(|worker|worker.sessions.by_run(&id).is_some())
                };
                if active {
                    if let Some(row) = observations.lock().unwrap().get_mut(&id) { row.host_active_ms.get_or_insert(elapsed(clock)); }
                }
                let status = host.database.with_connection(|connection| connection.query_row(
                    "SELECT status FROM runs WHERE id=?1", [&id], |row| row.get::<_,String>(0)));
                if let Ok(status) = status {
                    if terminal(&status) {
                        let observed = elapsed(clock);
                        let released = ownership_released(&host, &id);
                        if let Some(row) = observations.lock().unwrap().get_mut(&id) {
                            row.terminal_ms.get_or_insert(observed);
                            row.terminal_status = Some(status);
                            if released { row.released_ms.get_or_insert(elapsed(clock)); }
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(POLL_MS));
        }
    })
}

fn observe_body(observations: &Observations, run: &str, text: &str, clock: Instant) {
    if !text.is_empty() {
        if let Some(row) = observations.lock().unwrap().get_mut(run) { row.first_body_ms.get_or_insert(elapsed(clock)); }
    }
}

fn sample_resources(clock: Instant, stop: Arc<AtomicBool>, destination: PathBuf) -> thread::JoinHandle<Value> {
    thread::spawn(move || {
        let mut system = System::new();
        let root = Pid::from_u32(std::process::id());
        let mut samples = Vec::new();
        let mut peak_rss = 0u64;
        let mut peak_runtime_rss = 0u64;
        let mut max_nodes = 0usize;
        let mut runtime_pids = HashSet::new();
        let mut child_pids = HashSet::new();
        loop {
            system.refresh_processes_specifics(ProcessesToUpdate::All, true,
                ProcessRefreshKind::nothing().with_memory().with_cmd(sysinfo::UpdateKind::OnlyIfNotSet));
            let mut owned = HashSet::from([root]);
            loop {
                let before = owned.len();
                for (pid, process) in system.processes() {
                    if process.parent().is_some_and(|parent| owned.contains(&parent)) { owned.insert(*pid); }
                }
                if owned.len() == before { break; }
            }
            let mut rss = 0u64;
            let mut runtime_rss = 0u64;
            let mut nodes = 0usize;
            let mut processes = Vec::new();
            for pid in owned {
                let Some(process) = system.process(pid) else { continue };
                let name = process.name().to_string_lossy().to_lowercase();
                let command = process.cmd().iter().map(|value| value.to_string_lossy()).collect::<Vec<_>>().join(" ");
                let node = name == "node" || name == "node.exe" || name.starts_with("fox-agent-runtime");
                let worker = node && (command.contains("pi-runtime.mjs") || name.starts_with("fox-agent-runtime"));
                rss += process.memory();
                if node { nodes += 1; }
                if pid != root { child_pids.insert((pid.as_u32(), process.start_time())); }
                if worker { runtime_rss += process.memory(); runtime_pids.insert((pid.as_u32(), process.start_time())); }
                processes.push(json!({"pid":pid.as_u32(),"parentPid":process.parent().map(|value|value.as_u32()),
                    "startTime":process.start_time(),"name":name,"command":command,
                    "rssBytes":process.memory(),"virtualMemoryBytes":process.virtual_memory(),"runtimeWorker":worker}));
            }
            peak_rss = peak_rss.max(rss);
            peak_runtime_rss = peak_runtime_rss.max(runtime_rss);
            max_nodes = max_nodes.max(nodes);
            samples.push(json!({"atMs":elapsed(clock),"treeRssBytes":rss,"runtimeRssBytes":runtime_rss,
                "nodeProcessCount":nodes,"processes":processes}));
            if stop.load(Ordering::SeqCst) { break; }
            thread::sleep(Duration::from_millis(RESOURCE_MS));
        }
        write_json(&destination, &samples);
        json!({"samplingIntervalMs":RESOURCE_MS,"sampledPeakTreeRssBytes":peak_rss,
            "sampledPeakRuntimeRssBytes":peak_runtime_rss,"maxObservedNodeProcessCount":max_nodes,
            "observedRuntimePids":runtime_pids,"observedChildPids":child_pids,
            "definition":"owned test Host process plus descendants; sampled peaks and counts are lower bounds",
            "rawSamples":destination})
    })
}

fn read_request(stream: &mut TcpStream) -> Result<Value, String> {
    stream.set_nonblocking(false).map_err(|error|error.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|error|error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(15))).map_err(|error|error.to_string())?;
    let mut bytes = Vec::new();
    let mut scratch = [0u8; 8192];
    let header_end = loop {
        let count = stream.read(&mut scratch).map_err(|error|error.to_string())?;
        if count == 0 { return Err("HTTP request ended before headers".into()); }
        bytes.extend_from_slice(&scratch[..count]);
        if let Some(index) = bytes.windows(4).position(|window|window == b"\r\n\r\n") { break index + 4; }
        if bytes.len() > 64 * 1024 { return Err("oversize HTTP headers".into()); }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let first = headers.lines().next().unwrap_or_default();
    if !first.starts_with("POST ") || !first.contains("/chat/completions ") { return Err(format!("unexpected fixture request: {first}")); }
    let length: usize = headers.lines().find_map(|line| line.split_once(':')
        .filter(|(name,_)|name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_,value)|value.trim().parse().ok())).ok_or("missing Content-Length")?;
    if length > 16 * 1024 * 1024 { return Err("oversize fixture model request".into()); }
    while bytes.len() < header_end + length {
        let count = stream.read(&mut scratch).map_err(|error|error.to_string())?;
        if count == 0 { return Err("HTTP request ended inside body".into()); }
        bytes.extend_from_slice(&scratch[..count]);
    }
    serde_json::from_slice(&bytes[header_end..header_end+length]).map_err(|error|error.to_string())
}

fn marker(body: &Value) -> Option<String> {
    body["messages"].as_array()?.iter().rev().filter(|message|message["role"] == "user").find_map(|message| {
        let text = match &message["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts.iter().filter_map(|part|part["text"].as_str()).collect::<Vec<_>>().join("\n"),
            _ => return None,
        };
        let offset = text.rfind("[FOX_PERF_")?;
        let end = text[offset..].find(']')?;
        Some(text[offset+1..offset+end].to_owned())
    })
}

fn send_chunk(stream: &mut TcpStream, token: &str, delta: Value, finish: Value) -> std::io::Result<()> {
    let frame = json!({"id":format!("perf-{token}"),"object":"chat.completion.chunk","created":1,"model":MODEL,
        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
    write!(stream, "data: {}\n\n", frame)?;
    stream.flush()
}

struct Provider {
    endpoint: String,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    seen: Arc<Mutex<HashSet<String>>>,
    records: Arc<Mutex<Vec<Value>>>,
}
impl Provider {
    fn start(first_ms: u64, chunk_ms: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let server_stop = stop.clone();
        let server_seen = seen.clone();
        let server_records = records.clone();
        let join = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !server_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream,_)) => {
                        let seen = server_seen.clone(); let records = server_records.clone();
                        handlers.push(thread::spawn(move || {
                            let clock = Instant::now();
                            let result = (|| -> Result<(),String> {
                                let body = read_request(&mut stream)?;
                                let token = marker(&body).ok_or("provider received a request without a performance marker")?;
                                let first = seen.lock().unwrap().insert(token.clone());
                                records.lock().unwrap().push(json!({"marker":token,"firstRequestForTurn":first,
                                    "request":body,"requestSha256":digest(&crate::canonical_json::to_vec(&body).unwrap())}));
                                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").map_err(|error|error.to_string())?;
                                thread::sleep(Duration::from_millis(first_ms));
                                if token.starts_with("FOX_PERF_CANCEL:") {
                                    for index in 0..500 {
                                        if send_chunk(&mut stream,&token,json!({"content":format!("等待{index}。")}),Value::Null).is_err() { return Ok(()); }
                                        thread::sleep(Duration::from_millis(chunk_ms.max(10)));
                                    }
                                } else if first {
                                    let calls: Vec<_> = (0..6).map(|index| json!({"index":index,"id":format!("{token}-read-{index}"),
                                        "type":"function","function":{"name":"read","arguments":json!({"path":format!("proof-{index}.txt")}).to_string()}})).collect();
                                    send_chunk(&mut stream,&token,json!({"role":"assistant","tool_calls":calls}),Value::Null).map_err(|error|error.to_string())?;
                                    send_chunk(&mut stream,&token,json!({}),json!("tool_calls")).map_err(|error|error.to_string())?;
                                } else {
                                    for text in ["已", "读取", "六份", "夹具", "。"] {
                                        send_chunk(&mut stream,&token,json!({"content":text}),Value::Null).map_err(|error|error.to_string())?;
                                        thread::sleep(Duration::from_millis(chunk_ms));
                                    }
                                }
                                if !first || token.starts_with("FOX_PERF_CANCEL:") {
                                    send_chunk(&mut stream,&token,json!({}),json!("stop")).map_err(|error|error.to_string())?;
                                }
                                let usage = json!({"id":format!("perf-{token}"),"object":"chat.completion.chunk","created":1,"model":MODEL,
                                    "choices":[],"usage":{"prompt_tokens":128,"completion_tokens":32,"total_tokens":160}});
                                write!(stream,"data: {usage}\n\ndata: [DONE]\n\n").map_err(|error|error.to_string())?;
                                stream.flush().map_err(|error|error.to_string())
                            })();
                            if let Err(error) = result { records.lock().unwrap().push(json!({"transportError":error,"elapsedMs":elapsed(clock)})); }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
                    Err(error) => { server_records.lock().unwrap().push(json!({"listenerError":error.to_string()})); break; }
                }
            }
            for handler in handlers { let _ = handler.join(); }
        });
        Self { endpoint, stop, join:Some(join), seen, records }
    }
    fn reset(&self) { self.seen.lock().unwrap().clear(); self.records.lock().unwrap().clear(); }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::SeqCst);
        if let Some(join) = self.join.take() { let _ = join.join(); }
    }
}

struct HostCleanup(RuntimeHost);
impl Drop for HostCleanup { fn drop(&mut self) { let _ = self.0.shutdown(); } }

fn run_batch(host: &RuntimeHost, conversations: &[String], phase: &str, sample: usize,
    observations: &Observations, clock: Instant, cancel: bool) -> Result<(), String> {
    let queue = Arc::new(FixtureQueue::default());
    let barrier = Arc::new(Barrier::new(conversations.len()+1));
    let mut prepared = Vec::new();
    for (lane, conversation) in conversations.iter().enumerate() {
        let text = if cancel { format!("[FOX_PERF_CANCEL:{sample}:{lane}] 等待取消。") }
            else { format!("[FOX_PERF_BATCH:{sample}:{lane}] 读取 proof-0.txt 至 proof-5.txt，然后回复已读取六份夹具。") };
        let started = host.database.create_run(conversation,&text,None)
            .map_err(|error|format!("create_run phase={phase} sample={sample} lane={lane}: {error}"))?;
        observations.lock().unwrap().insert(started.run.id.clone(), Observation::new(&started,phase,sample,lane));
        let binding = host.database.freeze_default_run_control_for_engine(&started.run.id,"legacy",TimeBudgets::continuous(),"pi")
            .map_err(|error|format!("freeze_run phase={phase} sample={sample} lane={lane}: {error}"))?;
        assert_eq!(binding.authority,ExecutionAuthority::Authoritative);
        assert_eq!(binding.read_only_executor,ResourceExecutor::Rust);
        assert_eq!(binding.permission.mode,fox_engine_protocol::PermissionMode::Allow);
        prepared.push((lane,started,text));
    }
    let mut tasks = Vec::new();
    for (lane,started,text) in prepared {
        let host = host.clone(); let observations = observations.clone(); let queue = queue.clone(); let barrier = barrier.clone();
        tasks.push(thread::spawn(move || {
            let id = started.run.id.clone();
            barrier.wait();
            observations.lock().unwrap().get_mut(&id).unwrap().submitted_ms = Some(elapsed(clock));
            let _permit = queue.acquire(lane);
            observations.lock().unwrap().get_mut(&id).unwrap().fixture_admitted_ms = Some(elapsed(clock));
            let cancellation = cancel.then(|| {
                let host = host.clone(); let observations = observations.clone(); let id = id.clone();
                thread::spawn(move || {
                    let deadline = Instant::now()+Duration::from_secs(60);
                    loop {
                        let row = observations.lock().unwrap().get(&id).unwrap().clone();
                        if row.start_error.is_some() || row.terminal_ms.is_some() { return; }
                        if row.first_body_ms.is_some() { break; }
                        assert!(Instant::now()<deadline,"cancellation fixture did not receive first body");
                        thread::sleep(Duration::from_millis(POLL_MS));
                    }
                    observations.lock().unwrap().get_mut(&id).unwrap().cancel_requested_ms = Some(elapsed(clock));
                    assert!(host.cancel_run(&id).unwrap(),"Host refused cancellation of its own fixture Run");
                })
            });
            let result = host.start_run(&started,&text,&[]);
            {
                let mut rows = observations.lock().unwrap(); let row = rows.get_mut(&id).unwrap();
                row.host_call_returned_ms = Some(elapsed(clock));
                if let Err(error) = result { row.start_error = Some(error); }
            }
            let deadline = Instant::now()+Duration::from_secs(120);
            loop {
                let row = observations.lock().unwrap().get(&id).unwrap().clone();
                if row.released_ms.is_some() || row.start_error.is_some() && !cancel { break; }
                if Instant::now()>=deadline {
                    let _ = host.cancel_run(&id);
                    observations.lock().unwrap().get_mut(&id).unwrap().start_error = Some("120s fixture completion deadline".into());
                    break;
                }
                thread::sleep(Duration::from_millis(POLL_MS));
            }
            if let Some(cancellation) = cancellation { cancellation.join().unwrap(); }
        }));
    }
    barrier.wait();
    let joined = tasks.into_iter().map(|task|task.join().is_ok()).fold(true,|all,success|all && success);
    let rows = observations.lock().unwrap();
    let valid = joined && rows.values().filter(|row|row.phase == phase && row.sample == sample).all(|row|
        row.released_ms.is_some() && row.terminal_status.as_deref() == Some(if cancel {"cancelled"} else {"completed"})
        && (row.start_error.is_none() || cancel && row.cancel_requested_ms.is_some()));
    if valid { Ok(()) } else { Err(format!("batch failed phase={phase} sample={sample}; preserve observations and Host diagnostics")) }
}

fn child_measurement() {
    let root = standard_launch_path(&PathBuf::from(std::env::var_os("FOX_NATIVE_PERF_CELL_ROOT").unwrap()));
    let route = std::env::var("FOX_NATIVE_PERF_ROUTE").unwrap();
    let phase = std::env::var("FOX_NATIVE_PERF_PHASE").unwrap();
    let load = env_count("FOX_NATIVE_PERF_LOAD",1);
    let repetitions = env_count("FOX_NATIVE_PERF_REPETITIONS",1);
    let endpoint = std::env::var("FOX_NATIVE_PERF_ENDPOINT").unwrap();
    assert!(endpoint.starts_with("http://127.0.0.1:"));
    let clock = Instant::now();
    let database = Database::open(root.join("facts.db")).unwrap();
    database.save_model_service(&crate::database::ModelServiceRecord {
        name:MODEL.into(),base_url:endpoint.clone(),model_id:MODEL.into(),api_type:"openai-completions".into(),
        context_window:131072,max_output_tokens:1024,supports_image_input:false,enabled:true,credential_configured:false,
        connection_type:"local".into(),last_status:"unknown".into(),last_latency_ms:None,last_checked_at:None,created_at:1,updated_at:1,
    }).unwrap();
    for directory in ["sessions","attachments","skills","project"] { std::fs::create_dir_all(root.join(directory)).unwrap(); }
    for index in 0..6 { std::fs::write(root.join(format!("project/proof-{index}.txt")),format!("fixture-proof-{index}\n")).unwrap(); }
    let conversations: Vec<_> = (0..load).map(|lane|database.create_conversation(database.default_agent_id(),
        Some(&format!("performance lane {lane}")),root.join("project").to_str(),Some("allow")).unwrap().id).collect();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create=false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let mut host = RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    host.tool_allowlist_override = Some(HashSet::from(["read".to_owned()]));
    host.mcp_server_scope_override = Some(HashSet::new());
    host.execution_profile_override = Some("legacy");
    let _cleanup = HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));
    let observations: Observations = Arc::new(Mutex::new(HashMap::new()));
    let native_rows = observations.clone();
    let native_listener = app.listen("fox://runtime-event",move |event| {
        if let Ok(value) = serde_json::from_str::<Value>(event.payload()) {
            if value["event"]["type"] == "message.delta" {
                if let (Some(run),Some(text)) = (value["runId"].as_str(),value["event"]["delta"].as_str()) { observe_body(&native_rows,run,text,clock); }
            }
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let observer = start_observer(host.clone(),observations.clone(),clock,stop.clone());
    let resources = sample_resources(clock,stop.clone(),root.join("resources.json"));
    let mut failure = if phase == "warm" { run_batch(&host,&conversations,"warmup",0,&observations,clock,false).err() } else { None };
    for sample in 1..=repetitions {
        if failure.is_some() { break; }
        failure = run_batch(&host,&conversations,&phase,sample,&observations,clock,false).err();
        eprintln!("native performance route={route} phase={phase} load={load} sample={sample}/{repetitions}");
    }
    if phase == "warm" && failure.is_none() {
        failure = run_batch(&host,&conversations[..1],"cancellation",repetitions+1,&observations,clock,true).err();
    }
    let diagnostics = host.diagnostics();
    let shutdown_error = host.shutdown().err();
    stop.store(true,Ordering::SeqCst);
    observer.join().unwrap();
    let resources = resources.join().unwrap();
    app.unlisten(native_listener);
    let mut rows: Vec<_> = observations.lock().unwrap().values().cloned().collect();
    rows.sort_by(|left,right|left.phase.cmp(&right.phase).then(left.sample.cmp(&right.sample)).then(left.lane.cmp(&right.lane)));
    assert_eq!(route,"pi_native","only the PiNative route is measured");
    let expected_route = Some(ExecutorRoute::PiNative);
    let bindings: Vec<_> = rows.iter().map(|row|database.run_control_binding(&row.run_id).ok().flatten()).collect();
    assert!(bindings.iter().flatten().all(|binding|binding.executor_route == expected_route));
    let tool_evidence: Vec<_> = rows.iter().filter(|row|row.phase != "cancellation").map(|row| {
        let calls: Vec<_> = (0..6).map(|index| {
            let id = format!("FOX_PERF_BATCH:{}:{}-read-{index}",row.sample,row.lane);
            let result = database.get_runtime_tool_call(&row.run_id,&id).unwrap();
            let verified = result.as_ref().is_some_and(|call|call.tool_name == "read" && call.error_message.is_none()
                && call.result.as_ref().is_some_and(|value|value.to_string().contains(&format!("fixture-proof-{index}"))));
            json!({"callId":id,"verified":verified,"record":result})
        }).collect();
        json!({"runId":row.run_id,"valid":calls.iter().all(|call|call["verified"] == true),"calls":calls})
    }).collect();
    let valid = failure.is_none() && shutdown_error.is_none() && !rows.is_empty() && bindings.iter().all(Option::is_some)
        && tool_evidence.iter().all(|entry|entry["valid"] == true) && rows.iter().all(|row|
        (row.start_error.is_none() || row.phase == "cancellation" && row.cancel_requested_ms.is_some())
        && row.first_body_ms.is_some() && row.released_ms.is_some()
        && row.terminal_status.as_deref() == Some(if row.phase == "cancellation" {"cancelled"} else {"completed"}));
    let report = json!({"route":route,"phase":phase,"load":load,"repetitions":repetitions,"valid":valid,
        "pid":std::process::id(),"clock":"per-cell monotonic milliseconds","fixtureQueueCapacity":FIXTURE_CAPACITY,
        "hostNativeActivityLimit":sidecar_sessions::ACTIVE_RUN_LIMIT,
        "terminalObserverPollMs":POLL_MS,"releaseDefinition":"terminal DB state and absence from Host ownership registries",
        "firstBodyDefinition":"first nonempty Host-published answer delta; excludes reasoning",
        "entry":"RuntimeHost.start_run","sourceIdentity":source_identity(),"bindings":bindings,
        "fixtureFailure":failure,"hostDiagnosticsBeforeShutdown":diagnostics,"shutdownError":shutdown_error,
        "observations":rows,"toolEvidence":tool_evidence,"resources":resources});
    write_json(&root.join("cell.json"),&report);
    assert!(valid,"performance cell failed correctness or observation requirements: {}",root.display());
}

#[test]
fn performance_launch_paths_use_the_proven_windows_prefix_conversion() {
    for (input,expected) in [
        (r"\\?\D:\fixture\pi-runtime.mjs",r"D:\fixture\pi-runtime.mjs"),
        (r"\\?\UNC\server\share\pi-runtime.mjs",r"\\server\share\pi-runtime.mjs"),
        (r"D:\fixture\pi-runtime.mjs",r"D:\fixture\pi-runtime.mjs"),
    ] {
        assert_eq!(standard_launch_path(Path::new(input)),PathBuf::from(expected));
    }
}

fn percentile(values: &mut [f64], fraction: f64) -> Option<f64> {
    if values.is_empty() { return None; }
    values.sort_by(f64::total_cmp);
    Some(values[((values.len() as f64*fraction).ceil() as usize).saturating_sub(1).min(values.len()-1)])
}
fn distribution(values: Vec<f64>) -> Value {
    let mut sorted = values;
    json!({"count":sorted.len(),"p50":percentile(&mut sorted,0.50),"p95":percentile(&mut sorted,0.95),
        "min":sorted.first(),"max":sorted.last()})
}

fn summarize(cells: &[Value], loads: &[usize], warm_samples: usize) -> Value {
    let mut measurements = Vec::new();
    let mut complete = warm_samples >= 30;
    for load in loads {
        let rows: Vec<_> = cells.iter().filter(|cell|cell["route"] == "pi_native" && cell["phase"] == "warm" && cell["load"] == *load)
            .flat_map(|cell|cell["observations"].as_array().into_iter().flatten()).filter(|row|row["phase"] == "warm").collect();
        let metric = |end: &str| rows.iter().filter_map(|row| Some(row[end].as_f64()? - row["submittedMs"].as_f64()?)).collect::<Vec<_>>();
        complete &= rows.len() == warm_samples*load;
        measurements.push(json!({"submittedLoad":load,"simultaneousAdmissionLimit":FIXTURE_CAPACITY,"route":"pi_native",
            "ttftMs":distribution(metric("firstBodyMs")),"wholeTurnMs":distribution(metric("terminalMs")),
            "releasedMs":distribution(metric("releasedMs")),"fixtureQueueMs":distribution(rows.iter().filter_map(|row|
                Some(row["fixtureAdmittedMs"].as_f64()? - row["submittedMs"].as_f64()?)).collect())}));
    }
    json!({"measurements":measurements,"minimumWarmRepetitionsPerCell":30,"sampleCountComplete":complete,
        "decision":if complete {"complete"} else {"incomplete"},
        "scope":"offline Host fixture only; not S4 Host-internal queue, installer, real model, or product acceptance"})
}

fn run_cell(root: &Path, provider: &Provider, route: &str, phase: &str, load: usize, repetitions: usize, suffix: usize) -> Value {
    provider.reset();
    let cell = root.join(format!("{phase}-load-{load}-{suffix}-{route}"));
    std::fs::create_dir(&cell).unwrap();
    let stdout = std::fs::File::create(cell.join("stdout.log")).unwrap();
    let stderr = std::fs::File::create(cell.join("stderr.log")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([TEST_NAME,"--exact","--ignored","--nocapture","--test-threads=1"])
        .env(CHILD,"1").env("FOX_NATIVE_PERF_CELL_ROOT",&cell).env("FOX_NATIVE_PERF_ROUTE",route)
        .env("FOX_NATIVE_PERF_PHASE",phase).env("FOX_NATIVE_PERF_LOAD",load.to_string())
        .env("FOX_NATIVE_PERF_REPETITIONS",repetitions.to_string()).env("FOX_NATIVE_PERF_ENDPOINT",&provider.endpoint)
        .env("FOX_RUNTIME_SCRIPT",repository().join("services/agent-runtime/src/pi-runtime.mjs"))
        .env_remove("FOX_RUNTIME_EXECUTABLE").env_remove("FOX_RUNTIME_MODE").env_remove("FOX_KERNEL_WORKER_BINARY")
        .env_remove("FOX_KERNEL_CODEX_BINARY").env_remove("FOX_EXPERIMENTAL_COMPUTE_JOB_NOTICE")
        .env("FOX_DATA_DIR",&cell).stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now()+Duration::from_secs(env_count("FOX_NATIVE_PERF_CHILD_TIMEOUT_SECS",1800) as u64);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() { break Some(status); }
        if Instant::now()>=deadline { let _=child.kill(); let _=child.wait(); break None; }
        thread::sleep(Duration::from_millis(50));
    };
    let requests = provider.records.lock().unwrap().clone();
    write_json(&cell.join("provider-requests.json"),&requests);
    let path = cell.join("cell.json");
    let mut result: Value = std::fs::read(&path).ok().and_then(|bytes|serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(||json!({"route":route,"phase":phase,"load":load,"valid":false,"observations":[]}));
    result["childSucceeded"] = json!(status.is_some_and(|status|status.success()));
    result["pairIndex"] = json!(suffix);
    let contracts: Vec<_> = requests.iter().filter_map(|record| {
        let body = record.get("request")?;
        let mut tools: Vec<_> = body["tools"].as_array().into_iter().flatten().map(|tool|tool["function"].clone()).collect();
        tools.sort_by_key(|tool|tool["name"].as_str().unwrap_or_default().to_owned());
        Some(json!({"marker":record["marker"],"firstRequestForTurn":record["firstRequestForTurn"],
            "model":body["model"],"tools":tools,"maxTokens":body.get("max_tokens").or_else(||body.get("max_completion_tokens")),
            "temperature":body["temperature"],"topP":body["top_p"],"reasoningEffort":body["reasoning_effort"]}))
    }).collect();
    result["requestContracts"] = json!(contracts);
    result["evidenceDirectory"] = json!(cell);
    result
}

#[test]
#[ignore = "S5 isolated offline PiNative performance; default 30 warm and 5 cold samples, no remote model"]
fn native_offline_performance() {
    if std::env::var_os(CHILD).is_some() { child_measurement(); return; }
    assert!(cfg!(debug_assertions),"this source-runtime fixture requires a debug test build");
    let warm = env_count("FOX_NATIVE_PERF_WARM_SAMPLES",30);
    let cold = env_count("FOX_NATIVE_PERF_COLD_SAMPLES",5);
    assert!(warm>0 && cold>0,"both cold and warm phases must run");
    let loads: Vec<usize> = std::env::var("FOX_NATIVE_PERF_LOADS").unwrap_or_else(|_|"1,2,3,4".into())
        .split(',').map(|value|value.trim().parse::<usize>().unwrap()).collect();
    assert!(!loads.is_empty() && loads.iter().all(|load|(1..=4).contains(load)),"loads must be selected from 1,2,3,4");
    let root = repository().join("migration-work").join(format!("native-performance-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    println!("native performance evidence: {}",root.display());
    let identity = source_identity();
    assert_eq!(identity["compiledHostSha256"],identity["files"]["apps/desktop/src-tauri/src/runtime_host/mod.rs"],"test binary Host source is stale");
    let first_ms = env_count("FOX_NATIVE_PERF_PROVIDER_FIRST_MS",10) as u64;
    let chunk_ms = env_count("FOX_NATIVE_PERF_PROVIDER_CHUNK_MS",2) as u64;
    let provider = Provider::start(first_ms,chunk_ms);
    let mut cells = Vec::new();
    for sample in 0..cold {
        cells.push(run_cell(&root,&provider,"pi_native","cold",1,1,sample));
    }
    for load in &loads {
        cells.push(run_cell(&root,&provider,"pi_native","warm",*load,warm,0));
    }
    let unchanged = identity["files"] == source_identity()["files"];
    let cells_valid = cells.iter().all(|cell|cell["valid"] == true && cell["childSucceeded"] == true);
    let mut summary = summarize(&cells,&loads,warm);
    if !unchanged || !cells_valid { summary["decision"] = json!("incomplete"); }
    let report = json!({"kind":"fox-native-offline-performance","schemaVersion":1,
        "generatedAt":crate::database::now_ms(),"sourceIdentity":identity,"sourceUnchanged":unchanged,
        "provider":{"kind":"shared Rust loopback scripted SSE","endpoint":provider.endpoint,"model":MODEL,
            "firstResponseDelayMs":first_ms,"textChunkDelayMs":chunk_ms,"toolsPerBatch":6,"tool":"read",
            "promptTokensPerRequest":128,"outputTokensPerRequest":32},
        "fixtureQueueCapacity":FIXTURE_CAPACITY,"queueDefinition":"fixture_queue, not Host-internal scheduling",
        "productionAdmission":{"piNativeRunLimit":sidecar_sessions::ACTIVE_RUN_LIMIT,
            "sharedReadOnlyBatchLimit":native_tool_batch::PARALLEL_READ_ONLY_LIMIT},
        "warmSamplesPerCell":warm,"coldSamples":cold,"submittedLoads":loads,"cellsValid":cells_valid,
                "summary":summary,"cells":cells,
        "limitations":["Source runtime under the debug test Host, not an installed candidate",
            "3/4 submitted tasks use the common fixture queue; not 3/4 simultaneously active Native Runs",
            "RSS and process counts are sampled lower bounds; virtual memory is not labeled private bytes",
                        "No real provider, approval wait, compaction, long-lived 20-session capacity, or product acceptance claim"]});
    write_json(&root.join("report.json"),&report);
    println!("native performance report: {}",root.join("report.json").display());
    assert!(unchanged && cells_valid,"performance evidence is incomplete; inspect {}",root.display());
}
