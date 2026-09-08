use super::*;
use crate::kernel::{CancellationRegistry, CancellationPort};
use fox_engine_protocol::ResourceExecutor;

fn fixture(profile: &str) -> (Database, fox_engine_protocol::RunControlBinding, CancellationRegistry, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("fox-rust-reader-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note.txt"), "Rust gateway proof: 中文 😀").unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db.create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("read_only")).unwrap();
    let run = db.create_run(&conversation.id, "read the proof", None).unwrap().run;
    let binding = db.freeze_legacy_run_control_with_executor(&run.id, profile, ResourceExecutor::Rust).unwrap();
    let registry = CancellationRegistry::default();
    registry.register_run(&run.id).unwrap();
    (db, binding, registry, root)
}

fn request(binding: &fox_engine_protocol::RunControlBinding, id: &str) -> RuntimeEnvelope {
    serde_json::from_value(json!({
        "protocol": protocol::PROTOCOL_NAME, "version": protocol::PROTOCOL_VERSION,
        "kind":"request", "type":"tool.readonly_execute", "id":"reader-request", "timestamp":protocol::timestamp(),
        "runId":binding.run_id, "conversationId":binding.conversation_id,
        "payload":{"toolCallId":id,"tool":"read","input":{"path":"note.txt"},"permissionSnapshotId":binding.permission_snapshot_id}
    })).unwrap()
}

#[test]
fn rust_reader_dispatch_persists_exact_result_and_replays_without_reading_again() {
    let (db, binding, registry, root) = fixture("legacy");
    db.apply_runtime_event(&binding.run_id, 1, &json!({"type":"run.started"})).unwrap();
    let mut request = request(&binding, "read-once");
    let token = registry.tool_token(&binding.run_id, "read-once").unwrap();
    let result = execute_rust_reader_request(&db, &request, &token).unwrap();
    assert_eq!(result["result"]["content"][0]["text"], "Rust gateway proof: 中文 😀");
    std::fs::write(root.join("note.txt"), "changed after first dispatch").unwrap();
    assert_eq!(execute_rust_reader_request(&db, &request, &token).unwrap()["result"], result["result"]);
    request.payload.as_mut().unwrap()["input"]["path"] = json!("different.txt");
    assert!(execute_rust_reader_request(&db, &request, &token).is_err());
    registry.request_run_cancel(&binding.run_id);
    assert!(execute_rust_reader_request(&db, &request, &token).unwrap_err().contains("cancelled"));
}

#[test]
fn rust_reader_preparation_cannot_change_the_original_target_or_drop_default_path() {
    let (db, binding, registry, root) = fixture("legacy");
    db.apply_runtime_event(&binding.run_id, 1, &json!({"type":"run.started"})).unwrap();
    std::fs::write(root.join("different.txt"), "not the original target").unwrap();
    let mut request = request(&binding, "changed-preparation");
    request.payload.as_mut().unwrap()["originalInput"] = json!({"path":"note.txt"});
    request.payload.as_mut().unwrap()["input"] = crate::tool_guard::approve_read_only_tool("read", &json!({"path":"different.txt"}), binding.permission.project_root.as_deref()).unwrap().input;
    let token = registry.tool_token(&binding.run_id, "changed-preparation").unwrap();
    assert!(execute_rust_reader_request(&db, &request, &token).unwrap_err().contains("prepared input differs"));
    let failed = db.get_runtime_tool_call(&binding.run_id, "changed-preparation").unwrap().unwrap();
    assert_eq!(failed.status, "failed");
    let payload = request.payload.as_mut().unwrap();
    payload["toolCallId"] = json!("default-path");
    payload["tool"] = json!("ls");
    payload["originalInput"] = json!({});
    payload["input"] = crate::tool_guard::approve_read_only_tool("ls", &json!({"path":"."}), binding.permission.project_root.as_deref()).unwrap().input;
    let token = registry.tool_token(&binding.run_id, "default-path").unwrap();
    assert!(!execute_rust_reader_request(&db, &request, &token).unwrap()["isError"].as_bool().unwrap());
    assert_eq!(db.get_runtime_tool_call(&binding.run_id, "default-path").unwrap().unwrap().input, json!({}));
}

#[test]
fn nested_rust_reader_requires_running_declared_node_and_namespaced_identity() {
    let (db, binding, _, _) = fixture("graph_readonly_preview");
    db.apply_runtime_event(&binding.run_id, 1, &json!({"type":"run.started"})).unwrap();
    db.apply_runtime_event(&binding.run_id, 2, &json!({"type":"tool.started","toolCallId":"parent","tool":"graph_readonly_run","input":{"nodes":[{"id":"node-a","task":"read"}]}})).unwrap();
    let id = format!("graph-read:{}", hex::encode(Sha256::digest(serde_json::to_vec(&["parent", "node-a", "local-id"]).unwrap())));
    let mut request = request(&binding, &id);
    let payload = request.payload.as_mut().unwrap();
    payload["observationScope"] = json!("nested");
    payload["parentToolCallId"] = json!("parent");
    payload["graphNodeId"] = json!("node-a");
    payload["nodeToolCallId"] = json!("local-id");
    assert!(require_rust_reader_binding(&db, &request).is_ok());
    for (field, value) in [("graphNodeId", "foreign"), ("nodeToolCallId", "different"), ("parentToolCallId", "missing"), ("toolCallId", "unscoped")] {
        let mut changed = request.clone();
        changed.payload.as_mut().unwrap()[field] = json!(value);
        assert!(require_rust_reader_binding(&db, &changed).is_err(), "{field}");
    }
    db.apply_runtime_event(&binding.run_id, 3, &json!({"type":"tool.completed","toolCallId":"parent","tool":"graph_readonly_run","result":{}})).unwrap();
    assert!(require_rust_reader_binding(&db, &request).is_err());
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

#[test]
fn real_pi_jsonl_reads_through_rust_and_returns_the_persisted_tool_result() {
    let (db, binding, registry, _) = fixture("legacy");
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut command = std::process::Command::new("node");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = ChildGuard(command.arg(repository.join("services/agent-runtime/src/pi-runtime.mjs"))
        .current_dir(&repository).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap());
    let mut stdin = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || { for line in BufReader::new(stdout).lines() { if sender.send(line).is_err() { break; } } });
    let send = |stdin: &mut std::process::ChildStdin, value: Value| {
        writeln!(stdin, "{value}").unwrap(); stdin.flush().unwrap();
    };
    send(&mut stdin, serde_json::to_value(RuntimeRequest::new("initialize").with_payload(json!({"modelService":{
        "baseUrl":"faux://fox","modelId":"fox-test","apiType":"faux","contextWindow":4096,"maxOutputTokens":512,
        "fauxResponses":[{"content":[{"type":"toolCall","id":"pi-read","name":"read","arguments":{"path":"note.txt"}}],"stopReason":"toolUse"},"read complete"]
    }}))).unwrap());
    let mut executions = 0;
    let mut tool_returned = false;
    loop {
        let line = receiver.recv_timeout(Duration::from_secs(20)).expect("Pi JSONL response deadline").unwrap();
        let envelope: RuntimeEnvelope = serde_json::from_str(&line).unwrap();
        match envelope.r#type.as_str() {
            "ready" => send(&mut stdin, serde_json::to_value(RuntimeRequest::new("create_session")
                .with_conversation(&binding.conversation_id).with_runtime_session("reader-session").with_payload(json!({}))).unwrap()),
            "session_created" => send(&mut stdin, serde_json::to_value(RuntimeRequest::new("prompt")
                .with_conversation(&binding.conversation_id).with_runtime_session("reader-session").with_run(&binding.run_id)
                .with_payload(json!({"text":"read note.txt","controlBinding":binding}))).unwrap()),
            "request_failed" => panic!("Pi rejected the Host binding: {:?}", envelope.payload),
            "tool.preflight" => {
                let payload = envelope.payload.as_ref().unwrap();
                let (allowed, mut approved) = crate::tool_guard::preflight_payload("read", &payload["input"], binding.permission.project_root.as_deref(), binding.permission.mode.as_str());
                assert!(allowed);
                approved["executionRoute"] = json!("rust");
                approved["permissionSnapshotId"] = json!(binding.permission_snapshot_id);
                send(&mut stdin, serde_json::to_value(HostResponse::for_request(&envelope, "tool.preflight_allowed", approved)).unwrap());
            }
            "tool.readonly_execute" => {
                let token = registry.tool_token(&binding.run_id, "pi-read").unwrap();
                let result = execute_rust_reader_request(&db, &envelope, &token).unwrap();
                executions += 1;
                send(&mut stdin, serde_json::to_value(HostResponse::for_request(&envelope, "tool.execute_completed", result)).unwrap());
            }
            "runtime_event" => {
                let payload = envelope.payload.as_ref().unwrap();
                assert!(db.apply_runtime_event(&binding.run_id, envelope.seq.unwrap(), payload).unwrap());
                if payload["type"] == "tool.completed" {
                    assert_eq!(payload["result"]["content"][0]["text"], "Rust gateway proof: 中文 😀");
                    tool_returned = true;
                }
                assert_ne!(payload["type"], "run.failed", "{payload}");
                if payload["type"] == "run.completed" { break; }
            }
            _ => {}
        }
    }
    assert_eq!(executions, 1);
    assert!(tool_returned);
    let stored = db.get_runtime_tool_call(&binding.run_id, "pi-read").unwrap().unwrap();
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.result.unwrap()["content"][0]["text"], "Rust gateway proof: 中文 😀");
}
