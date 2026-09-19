//! `read_tool_result` production wiring, exercised end to end.
//!
//! These tests deliberately avoid the shortcut the fourth review called out:
//! nothing here calls `Database::tool_result_range` directly to prove the tool
//! works. Every read goes through the code path a model actually reaches —
//!
//! - the **Kernel** path: `GatewayPolicy::execute_context_resource`, which checks
//!   the frozen Run binding, the frozen tool catalogue and the execution profile
//!   before it answers; and
//! - the **Legacy protocol** path: a real `pi-runtime` child process that emits a
//!   real `tool.execute` envelope, which Host admits through
//!   `ensure_runtime_tool_allowed` and serves with `execute_tool_result_read_request`.
//!
//! The result being read is produced by a real Kernel dispatch and settle, not by
//! hand-written rows, so "the model can reach it" also means "Host really stored
//! it that way".

use super::*;
use crate::database::KernelHostScope;
use crate::runtime_host::{
    continuation, execute_tool_result_read_request, tool_result_read,
    validate_runtime_tool_authority, RuntimeToolIngress,
};
use crate::runtime_host::protocol::{
    canonical_runtime_tool_contract, HostResponse, RuntimeCapabilityManifest, RuntimeEnvelope,
    RuntimeRequest, RuntimeToolCapability, CAPABILITY_MANIFEST_VERSION, PROTOCOL_NAME,
    PROTOCOL_VERSION, timestamp,
};
use crate::runtime_host::kernel_gateway::GatewayPolicy;
use fox_engine_protocol::ResourceExecutor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TOOL: &str = "read_tool_result";

/// A payload big enough that the model view has to omit records, and shaped like
/// the reviewer's sample: a record collection with no `next` to navigate.
fn stored_payload() -> String {
    let records = (0..220)
        .map(|index| {
            json!({
                "id": index,
                "name": format!("row-{index:03}"),
                "note": "中".repeat(60),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({ "records": records, "total": 220 })).unwrap()
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// One Kernel tool call Host has already settled and stored, together with the
/// Run it belongs to. Built by a real dispatch, not by inserting rows, so the
/// reference a model receives is the same reference production stores.
struct Settled {
    db: Database,
    root: PathBuf,
    run_id: String,
    conversation: String,
    text: String,
    token: crate::kernel::CancellationToken,
}

/// Settle one Kernel tool call for real: propose it, dispatch it once through the
/// coordinator, and let Host persist the result.
fn settle_kernel_result(executions: &AtomicUsize) -> Settled {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let mut config = super::worker_configuration();
    // The frozen model catalogue is what a Kernel host scope is allowed to name,
    // so `read_tool_result` has to be proposed before Host may freeze it.
    config.proposal_tools.push(json!({
        "name": TOOL,
        "description": "Read a byte range of a tool result Fox already stored",
        "parameters": { "type": "object", "properties": { "reference": { "type": "string" } },
                        "required": ["reference"] },
    }));
    // `prepared` keeps the Run in `created`, which is the only point at which
    // Host may still freeze the resource catalogue it will later be held to.
    let (db, root, run_id) =
        super::fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    db.freeze_kernel_host_scope(&run_id, &scope_with(&["read", TOOL]))
        .unwrap();
    cancellation.register_run(&run_id).unwrap();
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    let text = stored_payload();
    let result = json!({ "content": [{ "type": "text", "text": text }], "details": {} });
    assert!(
        coordinator
            .dispatch_tool("read-a", "reader-owner", |_, _, _| {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok((true, result.clone()))
            })
            .unwrap(),
        "the Kernel dispatch must settle"
    );
    let conversation = db
        .run_control_binding(&run_id)
        .unwrap()
        .unwrap()
        .conversation_id;
    let token = cancellation.run_token(&run_id).unwrap();
    Settled {
        db,
        root,
        run_id,
        conversation,
        text,
        token,
    }
}

/// The capability manifest a Runtime really hands Host, built from the canonical
/// contract table rather than from a literal: if `read_tool_result` were not
/// registered in `TOOL_CONTRACTS`, building this manifest would fail.
fn authority_manifest(tool_names: &[&str]) -> RuntimeCapabilityManifest {
    RuntimeCapabilityManifest {
        manifest_version: CAPABILITY_MANIFEST_VERSION,
        streaming_text: true,
        cancellation: true,
        reasoning: true,
        session_resume: true,
        tool_approval: true,
        image_input: false,
        steering: false,
        context_compaction: true,
        dynamic_model_switch: false,
        work_loop: true,
        tools: tool_names
            .iter()
            .map(|name| {
                let (category, execution, approval) =
                    canonical_runtime_tool_contract(name).expect("tool is in the canonical catalog");
                RuntimeToolCapability {
                    name: (*name).to_owned(),
                    category: category.to_owned(),
                    execution: execution.to_owned(),
                    approval: approval.to_owned(),
                }
            })
            .collect(),
    }
}

fn scope_with(tool_names: &[&str]) -> KernelHostScope {
    KernelHostScope {
        schema_version: 1,
        tool_names: tool_names.iter().map(|name| (*name).to_owned()).collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    }
}

#[test]
fn kernel_context_resource_serves_a_persisted_result_and_rebuilds_it_exactly() {
    let executions = AtomicUsize::new(0);
    let Settled { db, root, run_id, conversation: _, text, token } =
        settle_kernel_result(&executions);
    let reference = format!("fox-result://{run_id}/read-a");

    // The model view Host would publish for this call: bounded, and carrying the
    // reference precisely because the omitted records have to stay reachable.
    // Retrievability now comes from the Host's trusted storage fact — this
    // fixture persisted the result and rebuilds it byte-exactly below, so the
    // fact is `whole`. (live.rs must pass the same fact once R4-A1 is wired;
    // without it the projection stays conservative and publishes every byte.)
    let stored_whole = crate::kernel_compaction::ToolResultStorage::whole(text.len());
    let bounded = crate::kernel_compaction::bound_tool_result_content_with_storage(
        "read",
        false,
        &json!([{ "type": "text", "text": text }]),
        Some(&reference),
        &stored_whole,
    )
    .expect("a large stored result must get a bounded view");
    let view: Value = serde_json::from_str(bounded[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(view["foxModelView"]["retrievable"], json!(true));
    assert_eq!(view["foxModelView"]["resultRef"], json!(reference));
    assert!(view["foxModelView"]["omittedItems"].as_u64().unwrap() > 0);

    // Kernel admission: the frozen catalogue is what decides availability.
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    let policy = GatewayPolicy {
        binding: binding.clone(),
        scope: db.kernel_host_scope(&run_id).unwrap(),

        database: None,
        sessions_dir: None,
        artifacts_dir: None,
    };
    let dir = std::env::temp_dir();

    // Walk the stored result with the production executor and rebuild it.
    let mut rebuilt = String::new();
    let mut offset = 0usize;
    let mut ranges = 0usize;
    loop {
        let input = json!({ "reference": reference, "offset": offset, "limit": 7_000 });
        let response = policy
            .execute_context_resource(&db, &dir, &dir, &dir, TOOL, &input, &token)
            .unwrap_or_else(|error| panic!("range {ranges} at {offset} failed: {error}"));
        assert_eq!(response["content"][0]["type"], json!("text"));
        let chunk = response["content"][0]["text"].as_str().unwrap().to_owned();
        assert!(!chunk.contains('\u{FFFD}'), "a range must not split a code point");
        assert_eq!(
            response["details"]["toolName"],
            json!("read"),
            "details={}",
            response["details"]
        );
        assert_eq!(response["details"]["retrievable"], json!(true));
        rebuilt.push_str(&chunk);
        ranges += 1;
        assert!(ranges < 1_000, "a legal walk must terminate");
        match response["details"]["nextOffset"].as_u64() {
            Some(next) => {
                let next = next as usize;
                assert!(next > offset, "a non-terminal cursor must advance");
                offset = next;
            }
            None => {
                assert_eq!(response["details"]["complete"], json!(true));
                break;
            }
        }
    }
    assert!(ranges > 1, "a 40 KiB result needs more than one range");
    assert_eq!(sha256(&rebuilt), sha256(&text), "every stored byte must come back");
    assert_eq!(rebuilt, text);

    // Reading never re-executes anything: the original dispatch ran exactly once
    // and the stored row is untouched.
    assert_eq!(executions.load(Ordering::SeqCst), 1, "ranges must not re-run the tool");
    let stored = db.get_runtime_tool_call(&run_id, "read-a").unwrap().unwrap();
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.result.unwrap()["content"][0]["text"], json!(text));

    // A tool outside the frozen catalogue is refused even though it exists.
    let narrowed = GatewayPolicy {
        binding: binding.clone(),
        scope: scope_with(&["read"]),

        database: None,
        sessions_dir: None,
        artifacts_dir: None,
    };
    let error = narrowed
        .execute_context_resource(
            &db,
            &dir,
            &dir,
            &dir,
            TOOL,
            &json!({ "reference": reference }),
            &token,
        )
        .unwrap_err();
    assert!(
        error.contains("frozen Kernel resource catalog"),
        "unexpected refusal: {error}"
    );

    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn kernel_refuses_scoped_reads_the_model_tried_to_authorize_itself() {
    let executions = AtomicUsize::new(0);
    let Settled { db, root, run_id, conversation, text: _, token: _ } =
        settle_kernel_result(&executions);
    let reference = format!("fox-result://{run_id}/read-a");

    // The scope is the Run binding's; a caller may not supply its own.
    for field in ["conversationId", "conversation", "runId", "toolCallId"] {
        let mut input = json!({ "reference": reference });
        input[field] = json!(conversation);
        let error = tool_result_read::prepare(&input).unwrap_err();
        assert!(
            error.contains("is not an argument of read_tool_result"),
            "{field}: {error}"
        );
    }
    // A reference that is not a Fox result reference never resolves.
    let error = tool_result_read::prepare(&json!({ "reference": "fox-result://nope" }))
        .unwrap_err();
    assert!(error.contains("is not a Fox tool result reference"), "{error}");

    // Cross-conversation: the same reference read under a *different* Run's
    // binding is refused by Host, not by the caller's honesty.
    let other = db
        .create_conversation(db.default_agent_id(), Some("other"), None, None)
        .unwrap();
    let started = db.create_run(&other.id, "other", None).unwrap();
    let other_binding = db
        .freeze_legacy_run_control_with_executor(
            &started.run.id,
            "legacy",
            ResourceExecutor::Runtime,
        )
        .unwrap();
    let error = tool_result_read::execute(
        &db,
        &other_binding.conversation_id,
        &tool_result_read::prepare(&json!({ "reference": reference })).unwrap(),
    )
    .unwrap_err();
    assert!(
        error.contains("outside the authorized conversation"),
        "unexpected refusal: {error}"
    );

    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_reopened_database_reads_the_previous_result_without_replaying_it() {
    let executions = AtomicUsize::new(0);
    let Settled { db, root, run_id, conversation, text, token } =
        settle_kernel_result(&executions);
    let reference = format!("fox-result://{run_id}/read-a");
    drop(db);

    // Reopen the same database the way a restarted desktop process would: the
    // bytes Host stored must still resolve, with no in-memory state carried over.
    let db = Database::open(root.join("facts.db")).unwrap();
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    assert_eq!(binding.conversation_id, conversation);
    let policy = GatewayPolicy {
        binding,
        scope: db.kernel_host_scope(&run_id).unwrap(),

        database: None,
        sessions_dir: None,
        artifacts_dir: None,
    };
    let dir = std::env::temp_dir();
    let mut rebuilt = String::new();
    let mut offset = 0usize;
    loop {
        let response = policy
            .execute_context_resource(
                &db,
                &dir,
                &dir,
                &dir,
                TOOL,
                &json!({ "reference": reference, "offset": offset, "limit": 7_000 }),
                &token,
            )
            .unwrap();
        rebuilt.push_str(response["content"][0]["text"].as_str().unwrap());
        match response["details"]["nextOffset"].as_u64() {
            Some(next) => {
                assert!(next as usize > offset, "a non-terminal cursor must advance");
                offset = next as usize;
            }
            None => break,
        }
    }
    assert_eq!(sha256(&rebuilt), sha256(&text), "a restart must not lose bytes");
    // Still exactly one execution of the original tool: the read is served from
    // storage, not by running the tool again.
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    // On a Kernel-authorized Run the Legacy protocol path refuses to write the
    // read back at all: the two engines do not share the same tool-call model.
    let envelope: RuntimeEnvelope = serde_json::from_value(json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION,
        "kind": "request", "type": "tool.execute", "id": "reopen-read",
        "timestamp": timestamp(),
        "runId": run_id, "conversationId": conversation,
        "payload": { "toolCallId": "reopen-call", "tool": TOOL,
                     "input": { "reference": reference } },
    }))
    .unwrap();
    let error = execute_tool_result_read_request(&db, &envelope).unwrap_err();
    assert!(error.contains("Kernel-owned"), "unexpected refusal: {error}");

    // An envelope that claims a different conversation than its own Run is bound
    // to is refused: the scope comes from the Run, never from the caller.
    let mut forged = envelope.clone();
    forged.conversation_id = Some("conversation-that-is-not-this-run".to_owned());
    let error = execute_tool_result_read_request(&db, &forged).unwrap_err();
    assert!(error.contains("bound to a different conversation"), "{error}");

    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A Legacy Run in which Host really executed `read` and stored its result.
/// Returns the database, its temp root, the conversation, the Run id, the Run
/// binding and the exact text Host persisted for `read-once`.
#[allow(clippy::type_complexity)]
fn legacy_read_result(
    text: &str,
) -> (Database, PathBuf, String, String, RunControlBinding, String) {
    let root = std::env::temp_dir().join(format!("fox-result-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("records.json"), text).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
            Some("read_only"),
        )
        .unwrap();
    let run = db
        .create_run(&conversation.id, "read the stored records", None)
        .unwrap()
        .run;
    let binding =
        db.freeze_legacy_run_control_with_executor(&run.id, "legacy", ResourceExecutor::Rust)
            .unwrap();
    db.apply_runtime_event(&run.id, 1, &json!({ "type": "run.started" }))
        .unwrap();
    let registry = CancellationRegistry::default();
    registry.register_run(&run.id).unwrap();
    let token = registry.tool_token(&run.id, "read-once").unwrap();
    let envelope: RuntimeEnvelope = serde_json::from_value(json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION,
        "kind": "request", "type": "tool.readonly_execute", "id": "reader-request",
        "timestamp": timestamp(),
        "runId": binding.run_id, "conversationId": binding.conversation_id,
        "payload": { "toolCallId": "read-once", "tool": "read",
                     "input": { "path": "records.json" },
                     "permissionSnapshotId": binding.permission_snapshot_id },
    }))
    .unwrap();
    let dispatched = crate::runtime_host::execute_rust_reader_request(&db, &envelope, &token)
        .expect("the Legacy read must execute and persist");
    let stored = dispatched["result"]["content"][0]["text"]
        .as_str()
        .expect("the dispatched read returns text")
        .to_owned();
    (
        db,
        root,
        conversation.id,
        run.id,
        binding,
        stored,
    )
}

/// The production registration and dispatch path, driven by a real Runtime:
/// the model calls `read_tool_result` inside `pi-runtime.mjs`, Host admits it
/// through the protocol policy and answers from the result Host persisted when
/// the original tool actually ran. Nothing in this test hand-writes a result row.
#[test]
fn real_pi_jsonl_reads_a_persisted_result_through_read_tool_result() {
    // A large single-line JSON payload: big enough that the model view omits
    // records, and shaped like the reviewer's sample.
    let (db, root, _conversation, first_run, _first_binding, stored) =
        legacy_read_result(&stored_payload());
    let reference = format!("fox-result://{first_run}/read-once");

    // The model view Host publishes for that call: bounded, and carrying the
    // reference precisely because the omitted records must stay reachable.
    // The storage fact is trusted (this fixture persisted the result and reads
    // it back below); without it the projection would publish every byte.
    let stored_whole = crate::kernel_compaction::ToolResultStorage::whole(stored.len());
    let bounded = crate::kernel_compaction::bound_tool_result_content_with_storage(
        "read",
        false,
        &json!([{ "type": "text", "text": stored }]),
        Some(&reference),
        &stored_whole,
    )
    .expect("a large stored result must get a bounded view");
    let view: Value = serde_json::from_str(bounded[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(view["foxModelView"]["retrievable"], json!(true));
    assert_eq!(view["foxModelView"]["resultRef"], json!(reference));
    assert!(view["foxModelView"]["omittedItems"].as_u64().unwrap() > 0);

    // Re-enter the conversation: a second Run reads the first Run's result. Host
    // allows one active Run per conversation, so the first one is ended first.
    db.mark_run_failed(&first_run, "test-run-ended", "closed before re-entry")
        .unwrap();
    let started = db
        .create_run(&_conversation, "read it back", None)
        .unwrap()
        .run;
    let binding =
        db.freeze_legacy_run_control_with_executor(&started.id, "legacy", ResourceExecutor::Runtime)
            .unwrap();

    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    // A real local HTTP provider drives subsequent calls from the received
    // content cursor. It never sees this database, stored text, or Host details.
    let mut provider_command = Command::new("node");
    #[cfg(windows)]
    provider_command.creation_flags(0x0800_0000);
    let mut provider = ChildGuard(provider_command
        .arg(repository.join("services/agent-runtime/test/fixtures/tool-result-range-provider.mjs"))
        .arg("--serve").arg(&reference).arg("4096")
        .stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap());
    let (provider_sender, provider_receiver) = mpsc::channel();
    let provider_stdout = provider.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(provider_stdout).lines() {
            if provider_sender.send(line).is_err() { break; }
        }
    });
    let provider_ready: Value = serde_json::from_str(&provider_receiver
        .recv_timeout(Duration::from_secs(15)).unwrap().unwrap()).unwrap();
    let mut command = Command::new("node");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = ChildGuard(
        command
            .arg(repository.join("services/agent-runtime/src/pi-runtime.mjs"))
            .current_dir(&repository)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut stdin = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let send = |stdin: &mut std::process::ChildStdin, value: Value| {
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    };
    send(
        &mut stdin,
        serde_json::to_value(RuntimeRequest::new("initialize").with_payload(json!({
            "modelService": {
                "baseUrl": provider_ready["baseUrl"], "modelId": "range-provider", "apiType": "openai-completions",
                "apiKey": "local-test-only", "contextWindow": 256000, "maxOutputTokens": 512,
            }
        })))
        .unwrap(),
    );

    let mut served = 0usize;
    let mut tool_returned = false;
    let mut refused: Option<String> = None;
    loop {
        let line = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("Pi JSONL response deadline")
            .unwrap();
        let envelope: RuntimeEnvelope = serde_json::from_str(&line).unwrap();
        match envelope.r#type.as_str() {
            "ready" => send(
                &mut stdin,
                serde_json::to_value(
                    RuntimeRequest::new("create_session")
                        .with_conversation(&binding.conversation_id)
                        .with_runtime_session("result-reader")
                        .with_payload(json!({})),
                )
                .unwrap(),
            ),
            "session_created" => send(
                &mut stdin,
                serde_json::to_value(
                    RuntimeRequest::new("prompt")
                        .with_conversation(&binding.conversation_id)
                        .with_runtime_session("result-reader")
                        .with_run(&binding.run_id)
                        .with_payload(json!({"text": "read the stored result back", "controlBinding": binding})),
                )
                .unwrap(),
            ),
            "request_failed" => panic!("Pi rejected the Host binding: {:?}", envelope.payload),
            "tool.execute" => {
                let payload = envelope.payload.as_ref().unwrap();
                let tool = payload["tool"].as_str().unwrap_or_default();
                assert_eq!(tool, TOOL, "unexpected tool request: {tool}");
                println!(
                    "REQUEST tool={tool} runId={} toolCallId={} input={}",
                    envelope.run_id.as_deref().unwrap_or("-"),
                    payload["toolCallId"],
                    payload["input"]
                );
                // The same admission gate the desktop host applies before dispatch:
                // active profile, the Run's frozen profile, and the manifest the
                // Runtime declared must all agree.
                let frozen = db.frozen_run_execution_profile_id(&binding.run_id).unwrap();
                let profile = continuation::ExecutionProfileSelection::resolve(
                    frozen.as_deref().unwrap_or("legacy"),
                )
                .unwrap();
                validate_runtime_tool_authority(
                    &profile,
                    frozen.as_deref(),
                    &authority_manifest(&[TOOL]),
                    &binding.run_id,
                    tool,
                    RuntimeToolIngress::Execute,
                )
                .unwrap_or_else(|(code, error)| panic!("{code}: {error}"));
                // ...and the refusal that proves the gate is doing work: the same
                // request under a manifest that never declared the tool.
                let (code, error) = validate_runtime_tool_authority(
                    &profile,
                    frozen.as_deref(),
                    &authority_manifest(&["read"]),
                    &binding.run_id,
                    tool,
                    RuntimeToolIngress::Execute,
                )
                .expect_err("a manifest without the tool must refuse it");
                assert!(!code.is_empty() && !error.is_empty());
                refused = Some(format!("{code}: {error}"));
                let response = execute_tool_result_read_request(&db, &envelope)
                    .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
                assert_eq!(response["isError"], json!(false), "{response}");
                assert_eq!(
                    response["result"]["details"]["runId"],
                    json!(first_run),
                    "the read must resolve the original Run, not this one"
                );
                assert_eq!(response["result"]["details"]["retrievable"], json!(true));
                println!(
                    "RESPONSE toolName={} runId={} toolCallId={} offset={} returnedBytes={} nextOffset={} complete={} retrievable={} originalBytes={} source={}",
                    response["result"]["details"]["toolName"],
                    response["result"]["details"]["runId"],
                    response["result"]["details"]["toolCallId"],
                    response["result"]["details"]["offset"],
                    response["result"]["details"]["returnedBytes"],
                    response["result"]["details"]["nextOffset"],
                    response["result"]["details"]["complete"],
                    response["result"]["details"]["retrievable"],
                    response["result"]["details"]["originalBytes"],
                    response["result"]["details"]["source"],
                );
                served += 1;
                send(
                    &mut stdin,
                    serde_json::to_value(HostResponse::for_request(
                        &envelope,
                        "tool.execute_completed",
                        response,
                    ))
                    .unwrap(),
                );
            }
            "runtime_event" => {
                let payload = envelope.payload.as_ref().unwrap();
                // A duplicate seq is simply not applied; only a failure matters.
                let _ = db.apply_runtime_event(&binding.run_id, envelope.seq.unwrap(), payload);
                assert_ne!(payload["type"], "run.failed", "{payload}");
                if payload["type"] == "tool.completed" {
                    tool_returned = true;
                }
                if payload["type"] == "run.completed" {
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(served >= 3, "the Runtime must read at least three pages");
    let provider_report: Value = serde_json::from_str(&provider_receiver
        .recv_timeout(Duration::from_secs(15)).unwrap().unwrap()).unwrap();
    let provider_report = &provider_report["report"];
    assert_eq!(provider_report["pages"].as_array().unwrap().len(), served);
    println!("MODEL_INPUT_VERIFY {provider_report}");
    assert_eq!(provider_report["sha256"], json!(sha256(&stored)));
    assert!(tool_returned);
    let refusal = refused.expect("the authority gate must have a refusal sample");
    assert!(
        refusal.contains("read_tool_result"),
        "the refusal must name the tool: {refusal}"
    );
    println!("AUTHORITY refusal sample: {refusal}");

    // The read is persisted as its own ToolCall in the *reading* Run, and the
    // bytes it returned are exactly what Host stored for the original read.
    let mut returned = String::new();
    for page in 1..=served {
        let read_call = db.get_runtime_tool_call(&binding.run_id, &format!("range-read-{page}"))
            .unwrap().unwrap();
        assert_eq!(read_call.tool_name, TOOL);
        assert_eq!(read_call.status, "completed");
        let result = read_call.result.unwrap();
        // The model projection must not contaminate the persisted raw ranges.
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
        returned.push_str(result["content"][0]["text"].as_str().unwrap());
    }
    assert_eq!(sha256(&returned), sha256(&stored));
    // The original read ran once and was not repeated by being read.
    let original = db
        .get_runtime_tool_call(&first_run, "read-once")
        .unwrap()
        .unwrap();
    assert_eq!(original.status, "completed");
    assert_eq!(
        original.result.unwrap()["content"][0]["text"].as_str().unwrap(),
        stored
    );
    println!(
        "VERIFY served={served} sha256(returned)={} sha256(stored)={} readingRun={} sourceRun={}",
        sha256(&returned),
        sha256(&stored),
        binding.run_id,
        first_run
    );

    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// The model-view cursor projection that `tool_result_messages` (live.rs) uses to
// keep the durable history replay reachable. These drive paging *from the
// model-visible `content[1]` block*, never from `details["nextOffset"]`, which
// is exactly what the provider projection drops.
// ---------------------------------------------------------------------------

/// Byte-boundary range slice mirroring the Host `utf8_byte_range` contract.
fn range_slice(text: &str, offset: usize, limit: usize) -> (usize, Option<usize>, String) {
    let length = text.len();
    let mut end = (offset + limit).min(length);
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    (end, (end < length).then_some(end), text[offset..end].to_owned())
}

/// A `read_tool_result` durable result for one range, shaped exactly as
/// `tool_result_read::execute` returns it (private fields included so the test
/// can prove they never leak into the model view).
fn fake_read_result(reference: &str, source: &str, offset: usize, limit: usize) -> Value {
    let (end, next, text) = range_slice(source, offset, limit);
    let complete = next.is_none();
    json!({
        "content": [{ "type": "text", "text": text }],
        "details": {
            "reference": reference,
            "runId": "source-run",
            "toolCallId": "read-once",
            "toolName": "read",
            "status": "completed",
            "offset": offset,
            "returnedBytes": end - offset,
            "nextOffset": next,
            "complete": complete,
            "originalBytes": source.len(),
            "truncated": false,
            "retrievable": true,
            "source": "settled tool result stored by Fox Host",
            "reread": Value::Null,
        }
    })
}

/// Extract the rendered cursor from the trailing model-view text block.
fn parse_cursor(view: &Value) -> Value {
    let text = view[1]["text"].as_str().expect("cursor block is text");
    let json = text
        .strip_prefix(crate::kernel_compaction::READ_RESULT_CURSOR_MARKER)
        .expect("cursor block carries the marker")
        .trim();
    serde_json::from_str(json).expect("cursor block is valid JSON")
}

#[test]
fn read_tool_result_model_view_drives_a_multipage_walk_from_the_model_view() {
    let source: String = (0..80)
        .map(|index| format!("记录{index:02}:中文内容😀测试样本"))
        .collect::<Vec<_>>()
        .join("\n");
    let reference = "fox-result://source-run/read-once".to_owned();
    let limit = 512usize;

    let mut rebuilt = String::new();
    let mut offset = 0usize;
    let mut pages = 0usize;
    loop {
        let result = fake_read_result(&reference, &source, offset, limit);
        let view = crate::kernel_compaction::read_tool_result_model_view(&result)
            .expect("a read_tool_result result with navigation must project a view");
        // The raw fragment is the first block, the cursor the trailing block.
        assert_eq!(
            view[0]["text"].as_str().unwrap(),
            result["content"][0]["text"].as_str().unwrap(),
            "the raw fragment must stay first and unchanged"
        );
        let cursor = parse_cursor(&view);
        // Private Host fields never reach the model view.
        for field in ["runId", "toolCallId", "toolName", "status", "source", "reread"] {
            assert!(cursor.get(field).is_none(), "{field} must not leak: {cursor}");
        }
        assert_eq!(cursor["retrievable"], json!(true));
        assert_eq!(cursor["truncated"], json!(false));

        rebuilt.push_str(view[0]["text"].as_str().unwrap());
        pages += 1;
        assert!(pages < 1_000, "a legal walk must terminate");
        if cursor["complete"] == json!(true) {
            break;
        }
        // The next page's offset comes from the model view, not from details.
        let next = cursor["nextOffset"].as_u64().expect("non-terminal cursor has nextOffset") as usize;
        assert!(next > offset, "a non-terminal cursor must advance");
        offset = next;
    }

    assert!(pages >= 3, "a 3 KiB result at 512 bytes/read needs more than one page");
    assert_eq!(sha256(&rebuilt), sha256(&source));
    assert_eq!(rebuilt, source);
}

#[test]
fn read_tool_result_model_view_keeps_receipts_and_surfaces_preview_flags() {
    let reference = "fox-result://source-run/read-once";
    let source = "abcdefgh中文😀".to_owned();

    // A Kernel-settled result also carries an execution receipt in content.
    let mut result = fake_read_result(reference, &source, 0, 64);
    result["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "text", "text": "FOX_EXECUTION_RECEIPT_V1\n{\"executionState\":\"completed\"}" }));
    let view = crate::kernel_compaction::read_tool_result_model_view(&result)
        .expect("a completed read result projects a view");
    let before = result.clone();
    let mut replayed = result.clone();
    replayed["content"] = view.clone();
    assert_eq!(crate::kernel_compaction::read_tool_result_model_view(&replayed), Some(view.clone()));
    assert_eq!(result, before, "durable result must not be changed");
    let blocks = view.as_array().unwrap();
    assert_eq!(blocks.len(), 3, "fragment + receipt + cursor");
    assert_eq!(blocks[0]["text"], result["content"][0]["text"]);
    assert!(blocks[1]["text"].as_str().unwrap().starts_with("FOX_EXECUTION_RECEIPT_V1"));
    assert!(blocks[2]["text"].as_str().unwrap().starts_with(crate::kernel_compaction::READ_RESULT_CURSOR_MARKER));

    // A preview-only read (retrievable=false, truncated=true) must be
    // distinguishable from "I read back the complete original".
    let preview = json!({
        "content": [{ "type": "text", "text": "bound preview bytes" }],
        "details": {
            "reference": reference, "offset": 0, "returnedBytes": 18,
            "nextOffset": Value::Null, "complete": true, "originalBytes": 200_000,
            "truncated": true, "retrievable": false,
        }
    });
    let view = crate::kernel_compaction::read_tool_result_model_view(&preview)
        .expect("a preview read projects a view");
    let cursor = parse_cursor(&view);
    assert_eq!(cursor["complete"], json!(true));
    assert_eq!(cursor["retrievable"], json!(false));
    assert_eq!(cursor["truncated"], json!(true));

    // A result without navigation fields projects nothing.
    let empty = json!({ "content": [{ "type": "text", "text": "x" }], "details": {} });
    assert!(crate::kernel_compaction::read_tool_result_model_view(&empty).is_none());
}
