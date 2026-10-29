use super::{native_tool_batch, sidecar_sessions, RuntimeHost, WorkerHandle};
use crate::canonical_json::CanonicalJson;
use crate::database::{CreateMemoryRequest, Database, KnowledgeReference, KnowledgeReferenceBindingInput, SaveAgentRequest};
use crate::local_knowledge::{CreateLocalKnowledgeBaseRequest, LocalKnowledgeDocumentsRequest,
    LocalKnowledgeImportFileRequest, LocalKnowledgeImportRequest, LocalKnowledgeStore};
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode,
    ResourceExecutor, RunControlBinding, RuntimeEnvelope, TimeBudgets, TOOL_BATCH_REGISTER, TOOL_CALL_EXECUTE};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, io::{BufRead, BufReader}, path::PathBuf,
    process::{Command, Stdio}, sync::{mpsc, Arc, Mutex}, time::{Duration, Instant}};
use tauri::Manager;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CHILD: &str = "FOX_TEST_NATIVE_ISOLATION_CHILD";
const TOOLS: &[&str] = &["memory_search", "memory_propose", "list_knowledge_bases", "read_knowledge_document"];

fn isolated(test: &str) -> bool {
    if std::env::var(CHILD).as_deref() == Ok(test) { return false; }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, test).env("FOX_KERNEL_MODE", "authoritative").env("FOX_EXECUTOR_ROUTE", "pi_native")
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill(); let _ = child.wait();
            panic!("native isolation fixture exceeded 60 seconds: {test}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "child stdout: {}\nchild stderr: {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    true
}

struct Run {
    id: String,
    conversation: String,
    session: String,
    generation: i64,
    message: String,
}

struct Fixture {
    host: RuntimeHost,
    root: PathBuf,
    knowledge: LocalKnowledgeStore,
    responses: mpsc::Receiver<Value>,
    _app: tauri::App,
}

impl Fixture {
    fn new() -> Self {
        let parent = std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT")
            .map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let root = parent.join(format!("fox-native-isolation-{}", uuid::Uuid::new_v4()));
        for name in ["sessions", "attachments", "skills", "project-a", "project-b", "knowledge"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        println!("native isolation fixture retained at {}", root.display());
        let database = Database::open(root.join("fox.db")).unwrap();
        let knowledge = LocalKnowledgeStore::open(root.join("knowledge")).unwrap();
        let mut context = tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.create = false; }
        let app = tauri::Builder::default().any_thread().build(context).unwrap();
        let client = crate::yuxi::YuxiClient::new().unwrap();
        let host = RuntimeHost::new(app.handle().clone(), database.clone(), root.join("sessions"),
            root.join("attachments"), root.join("skills"), client.clone());
        app.manage(crate::app_state::AppState::new(database.clone(), host.clone(), client.clone(),
            crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(), database,
                client), crate::model_service::ModelServiceClient::new().unwrap(), knowledge.clone(),
            root.clone(), root.join("skills")));
        // This worker only echoes Host protocol replies. It runs no SDK, tool,
        // provider request, or user-supplied code.
        let mut command = Command::new("node");
        command.args(["-e", "process.stdin.on('data', data => process.stdout.write(data))"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().expect("start the protocol-only echo worker");
        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let stdout = child.stdout.take().unwrap();
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break; };
                let value = serde_json::from_str(&line).expect("Host emitted valid JSON");
                if sender.send(value).is_err() { break; }
            }
        });
        {
            let mut state = host.state.lock().unwrap();
            state.state = "ready".into();
            state.execution_profile_id = "legacy".into();
            let tools: Vec<Value> = TOOLS.iter().map(|name| {
                let (category, execution, approval) = fox_engine_protocol::canonical_runtime_tool_contract(name).unwrap();
                json!({"name":name,"category":category,"execution":execution,"approval":approval})
            }).collect();
            state.capabilities = json!({"manifestVersion":fox_engine_protocol::CAPABILITY_MANIFEST_VERSION,
                "streamingText":true,"cancellation":true,"reasoning":false,"sessionResume":true,
                "toolApproval":true,"imageInput":false,"steering":false,"contextCompaction":true,
                "dynamicModelSwitch":false,"workLoop":true,"tools":tools});
            state.worker = Some(WorkerHandle { child:Arc::new(Mutex::new(child)), stdin,
                pending:Arc::new(Mutex::new(HashMap::new())), sessions:sidecar_sessions::Sessions::default(),
                session_leases:HashMap::new() });
        }
        Self { host, root, knowledge, responses, _app:app }
    }

    fn db(&self) -> &Database { &self.host.database }

    fn conversation(&self, agent: &str, project: Option<&str>) -> crate::database::ConversationSummary {
        let root = project.map(|name| self.root.join(name));
        self.db().create_conversation(agent, Some("U10 isolated owner"),
            root.as_ref().and_then(|root| root.to_str()), Some("ask")).unwrap()
    }

    fn start(&self, conversation: &str) -> Run {
        let started = self.db().create_run(conversation, "U10 memory and knowledge ownership", None).unwrap();
        let project = self.db().conversation_project_access(conversation).unwrap().map(|value| value.0);
        let permission = FrozenPermission { mode:PermissionMode::Ask, project_root:project,
            grants:Vec::new(), approval_epoch:None };
        self.db().freeze_run_execution_profile(&started.run.id, "legacy").unwrap();
        self.db().freeze_run_control(&RunControlBinding {
            schema_version:fox_engine_protocol::CONTROL_SCHEMA_VERSION, run_id:started.run.id.clone(),
            conversation_id:conversation.into(), engine_id:"pi".into(), execution_profile_id:"legacy".into(),
            authority:ExecutionAuthority::Authoritative, executor_route:Some(ExecutorRoute::PiNative),
            read_only_executor:ResourceExecutor::Rust,
            permission_snapshot_id:Database::run_control_permission_hash(&permission).unwrap(),
            permission, budgets:TimeBudgets::default(),
        }).unwrap();
        let (session, generation) = match self.db().get_runtime_session(conversation).unwrap() {
            Some(mapping) => (mapping.runtime_session_id, mapping.context_generation),
            None => {
                let session = format!("native-u10-{}", uuid::Uuid::new_v4());
                let path = self.root.join("sessions").join(format!("{session}.jsonl"));
                std::fs::write(&path, format!("{}\n", json!({"type":"session","version":3,"id":session,
                    "timestamp":"2026-10-28T00:00:00.000Z","cwd":self.root.join("sessions")}))).unwrap();
                self.db().ensure_native_runtime_session(conversation, &session, Some("fixture"), &path.to_string_lossy(),
                    Some(&session), 1, crate::database::RuntimeSessionStatus::Ready).unwrap();
                (session, 1)
            }
        };
        self.db().with_connection(|connection| {
            connection.execute("UPDATE runs SET status='running',started_at=?2,runtime_session_id=?3 WHERE id=?1",
                params![started.run.id,crate::database::now_ms(),session])?;
            Ok(())
        }).unwrap();
        let mut state = self.host.state.lock().unwrap();
        state.worker.as_mut().unwrap().sessions.start(conversation, &session, generation, &started.run.id).unwrap();
        state.cancellation.register_run(&started.run.id).unwrap();
        Run { id:started.run.id, conversation:conversation.into(), session, generation, message:started.user_message.id }
    }

    fn release(&self, run: &Run) {
        self.db().mark_run_interrupted(&run.id, "fixture.released", "U10 fixture finished its tool checks.").unwrap();
        let mut state = self.host.state.lock().unwrap();
        state.worker.as_mut().unwrap().sessions.finish(&run.id);
        state.cancellation.retire_run(&run.id);
    }

    fn envelope(&self, run: &Run, kind: &str, payload: Value) -> RuntimeEnvelope {
        RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(), version:fox_engine_protocol::PROTOCOL_VERSION,
            kind:"request".into(), id:uuid::Uuid::new_v4().to_string(), r#type:kind.into(), request_id:None,
            conversation_id:Some(run.conversation.clone()), runtime_session_id:Some(run.session.clone()),
            run_id:Some(run.id.clone()), seq:None, timestamp:String::new(), payload:Some(payload) }
    }

    fn send(&self, envelope: RuntimeEnvelope) -> Value {
        let request = envelope.id.clone();
        let stdin = self.host.state.lock().unwrap().worker.as_ref().unwrap().stdin.clone();
        native_tool_batch::dispatch(&self.host, &stdin, envelope);
        let response = self.responses.recv_timeout(Duration::from_secs(15)).expect("Host protocol reply");
        assert_eq!(response["requestId"], request, "unexpected Host protocol reply: {response}");
        response["payload"].clone()
    }

    fn execute(&self, run: &Run, tool: &str, input: Value) -> Value {
        let call = format!("u10-call-{}", uuid::Uuid::new_v4());
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(input.canonical_string().as_bytes())));
        let registered = self.send(self.envelope(run, TOOL_BATCH_REGISTER, json!({"runId":run.id,
            "contextGeneration":run.generation,"calls":[{"callId":call,"tool":tool,"sourceOrder":0,"inputDigest":digest}]})));
        assert_ne!(registered["isError"], true, "registration failed: {registered}");
        let batch = registered["batchId"].as_str().expect("Host assigned the batch identity");
        let envelope = self.envelope(run, TOOL_CALL_EXECUTE,
            json!({"runId":run.id,"batchId":batch,"callId":call,"tool":tool,"input":input}));
        let response = self.send(envelope);
        let (owner, host_state, native_state, stored): (String,String,String,String) = self.db().with_connection(|connection|
            connection.query_row("SELECT t.conversation_id,t.status,k.state,k.result_json FROM kernel_tool_calls k
                JOIN tool_calls t ON t.run_id=k.run_id AND t.runtime_tool_call_id=k.tool_call_id
                WHERE k.run_id=?1 AND k.tool_call_id=?2",params![run.id,call],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap();
        assert_eq!(owner, run.conversation);
        let expected = if response["isError"] == true { "failed" } else { "completed" };
        assert_eq!(host_state, expected, "{response}");
        assert_eq!(native_state, expected, "{response}");
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), response);
        response
    }

    fn confirmed_memory(&self, scope: &str, key: Option<&str>, label: &str) -> String {
        self.db().create_user_memory(&CreateMemoryRequest { scope:scope.into(),scope_key:key.map(str::to_owned),
            kind:"identity".into(), canonical_key:format!("u10-{label}"), content:label.into(),
            evidence_excerpt:Some("isolated U10 fixture".into()) }).unwrap().id
    }

    fn knowledge_document(&self, label: &str) -> (String,String) {
        let base = self.knowledge.create_base(CreateLocalKnowledgeBaseRequest { name:format!("U10 {label}"),
            description:None, embedding_model_id:None, chunk_size:None, chunk_overlap:None,
            search_mode:Some("keyword".into()) }).unwrap();
        let source = self.root.join(label).join("note.md");
        std::fs::write(&source, format!("U10 {label} private knowledge")).unwrap();
        self.knowledge.start_import(LocalKnowledgeImportRequest { knowledge_base_id:base.id.clone(),
            files:vec![LocalKnowledgeImportFileRequest { source_path:source.to_string_lossy().into_owned(),relative_path:Some("note.md".into()) }],
            parser_version:None, chunk_config_hash:None }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let jobs = self.knowledge.list_jobs(Some(&base.id)).unwrap();
            assert!(!jobs.is_empty());
            assert!(jobs.iter().all(|job| !matches!(job.status.as_str(), "failed" | "cancelled" | "interrupted")), "{jobs:?}");
            if jobs.iter().all(|job| job.status == "completed") { break; }
            assert!(Instant::now() < deadline, "local fixture import did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
        let documents = self.knowledge.list_documents(LocalKnowledgeDocumentsRequest {
            knowledge_base_id:base.id.clone(), query:None }).unwrap();
        assert_eq!(documents.len(), 1);
        (base.id, documents[0].id.clone())
    }

    fn bind(&self, conversation: &str, base: &str) {
        self.db().set_knowledge_reference_bindings(conversation, &[KnowledgeReferenceBindingInput {
            reference:KnowledgeReference::local(base),name:format!("U10 {base}") }]).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let worker = { let mut state = self.host.state.lock().unwrap(); state.shutting_down = true;
            state.queue_changed.1.notify_all(); state.worker.take() };
        if let Some(worker) = worker { let mut child = worker.child.lock().unwrap(); let _ = child.kill(); let _ = child.wait(); }
    }
}

#[test]
fn native_memory_tools_preserve_project_agent_and_source_ownership() {
    let test = "runtime_host::native_isolation_tests::native_memory_tools_preserve_project_agent_and_source_ownership";
    if isolated(test) { return; }
    let fixture = Fixture::new();
    let other_agent = fixture.db().save_agent(&SaveAgentRequest { id:None,name:"U10 isolated expert".into(),
        description:"temporary test expert".into(),icon:None,category:"general".into(),system_prompt:"Use only Host tools.".into(),
        default_model:String::new(),opening_suggestions:Vec::new(),
        package_manifest:json!({"manifestSchemaVersion":2,"allowedTools":TOOLS,"skills":[],"mcpServers":[]}) }).unwrap();
    let a = fixture.conversation("fox-general", Some("project-a"));
    let b = fixture.conversation("fox-general", Some("project-b"));
    let ordinary = fixture.conversation("fox-general", None);
    let global = fixture.confirmed_memory("global", None, "global-owned");
    let agent = fixture.confirmed_memory("agent", Some("fox-general"), "general-agent-owned");
    let other_memory = fixture.confirmed_memory("agent", Some(&other_agent.id), "other-agent-owned");
    let project_a = fixture.confirmed_memory("project", a.project_id.as_deref(), "project-a-owned");
    let project_b = fixture.confirmed_memory("project", b.project_id.as_deref(), "project-b-owned");
    for (conversation, expected) in [(&a,vec![global.clone(),agent.clone(),project_a]),
        (&b,vec![global.clone(),agent.clone(),project_b]),(&ordinary,vec![global,agent])] {
        let run = fixture.start(&conversation.id);
        let response = fixture.execute(&run, "memory_search", json!({"query":"","limit":20,
            "conversationId":b.id,"projectId":b.project_id,"agentId":other_agent.id}));
        assert_ne!(response["isError"], true, "{response}");
        let recalled: HashSet<String> = response["result"]["items"].as_array().unwrap().iter()
            .map(|item| item["id"].as_str().unwrap().to_owned()).collect();
        assert!(!recalled.contains(&other_memory), "an ordinary conversation must not recall another expert's memory");
        assert_eq!(recalled, expected.into_iter().collect::<HashSet<_>>(), "owner={}", conversation.id);
        let foreign: i64 = fixture.db().with_connection(|connection| connection.query_row(
            "SELECT COUNT(*) FROM memory_recalls WHERE run_id=?1 AND conversation_id<>?2",
            params![run.id,conversation.id],|row| row.get(0))).unwrap();
        assert_eq!(foreign, 0);
        fixture.release(&run);
    }
    let run = fixture.start(&a.id);
    let proposed = fixture.execute(&run, "memory_propose", json!({"scope":"project","scopeKey":b.project_id,
        "projectId":b.project_id,"conversationId":b.id,"kind":"fact","canonicalKey":"u10-project-proposal",
        "content":"owned by project A","evidenceExcerpt":"test proposal","sourceMessageId":run.message}));
    assert_ne!(proposed["isError"], true, "{proposed}");
    let memory = fixture.db().get_memory(proposed["result"]["id"].as_str().unwrap()).unwrap().unwrap();
    assert_eq!(memory.scope_key, a.project_id.as_deref().unwrap());
    assert_eq!(memory.source_conversation_id.as_deref(), Some(a.id.as_str()));
    assert_eq!(memory.source_run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(memory.source_message_id.as_deref(), Some(run.message.as_str()));
    assert_ne!(memory.state, "confirmed");
    fixture.release(&run);
    let run_b = fixture.start(&b.id);
    let refused = fixture.execute(&run_b, "memory_propose", json!({"scope":"project","kind":"fact",
        "canonicalKey":"u10-foreign-source","content":"must not be saved","evidenceExcerpt":"test",
        "sourceMessageId":run.message}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused.to_string().contains("memory.source_message_outside_conversation"));
    fixture.release(&run_b);
    let no_project = fixture.start(&ordinary.id);
    let refused = fixture.execute(&no_project, "memory_propose", json!({"scope":"project","projectId":a.project_id,
        "kind":"fact","canonicalKey":"u10-no-project","content":"must not be saved","evidenceExcerpt":"test"}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused.to_string().contains("memory.project_scope_requires_authorized_project"));
}

#[test]
fn native_knowledge_tools_preserve_conversation_references_and_document_ownership() {
    let test = "runtime_host::native_isolation_tests::native_knowledge_tools_preserve_conversation_references_and_document_ownership";
    if isolated(test) { return; }
    let fixture = Fixture::new();
    let a = fixture.conversation("fox-general", Some("project-a"));
    let b = fixture.conversation("fox-general", Some("project-b"));
    let (base_a, document_a) = fixture.knowledge_document("project-a");
    let (base_b, document_b) = fixture.knowledge_document("project-b");
    fixture.bind(&a.id, &base_a); fixture.bind(&b.id, &base_b);
    let run_a = fixture.start(&a.id); let run_b = fixture.start(&b.id);
    for (run, base) in [(&run_a,&base_a),(&run_b,&base_b)] {
        let listed = fixture.execute(run,"list_knowledge_bases",json!({}));
        assert_ne!(listed["isError"], true, "{listed}");
        let items = listed["result"]["details"]["items"].as_array().expect("Host knowledge list details.items");
        assert_eq!(items.len(), 1); assert_eq!(items[0]["id"], base.as_str());
    }
    let own = fixture.execute(&run_a,"read_knowledge_document",json!({"reference":KnowledgeReference::local(&base_a),"documentId":document_a}));
    assert_ne!(own["isError"], true, "{own}");
    assert_eq!(own["result"]["details"]["result"]["content"], "U10 project-a private knowledge");
    let foreign = fixture.execute(&run_a,"read_knowledge_document",json!({"reference":KnowledgeReference::local(&base_b),
        "documentId":document_b,"conversationId":b.id}));
    assert_eq!(foreign["isError"], true, "{foreign}");
    assert!(foreign.to_string().contains("local_knowledge.binding_invalid"));
    assert!(!foreign.to_string().contains("U10 project-b private knowledge"));
    let wrong_document = fixture.execute(&run_a,"read_knowledge_document",json!({"reference":KnowledgeReference::local(&base_a),"documentId":document_b}));
    assert_eq!(wrong_document["isError"], true, "{wrong_document}");
    assert!(!wrong_document.to_string().contains("U10 project-b private knowledge"));
    let b_still_reads = fixture.execute(&run_b,"read_knowledge_document",json!({"reference":KnowledgeReference::local(&base_b),"documentId":document_b}));
    assert_eq!(b_still_reads["result"]["details"]["result"]["content"], "U10 project-b private knowledge");
    fixture.db().set_knowledge_reference_bindings(&a.id, &[]).unwrap();
    let revoked = fixture.execute(&run_a,"read_knowledge_document",json!({"reference":KnowledgeReference::local(&base_a),"documentId":document_a}));
    assert_eq!(revoked["isError"], true, "{revoked}");
    assert!(revoked.to_string().contains("local_knowledge.binding_invalid"));
}

