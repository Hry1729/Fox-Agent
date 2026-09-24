//! Default-set coverage for the artifact-reference flow through the REAL
//! Kernel frozen-resource policy (`validate`/`decide`), not a direct call to
//! `office::prepare_with_context`. A legitimate `artifactId` produced by this
//! run's own conversation passes the gate; cross-session, tampered, malformed
//! and dual-payload references are rejected before dispatch.
use super::*;
use sha2::{Digest, Sha256};

/// Set up a run whose frozen scope authorizes the fox-office connector and
/// `office_import_data`, returning the database, project root, run/conversation
/// ids and an isolated sessions store.
fn artifact_gate_fixture() -> (Database, PathBuf, String, String, std::path::PathBuf) {
    // Mirror the shared coordinator fixture (prepared=true) but freeze an Allow
    // permission once, so a mutating import reaches an Allow decision.
    let clock = TestClock::new(crate::database::now_ms());
    let root = std::env::temp_dir().join(format!("fox-kernel-artifact-gate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    db.ensure_office_connector("kernel-artifact-gate-fixture", None)
        .unwrap();

    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("allow"))
        .unwrap();
    let run_id = db.create_run(&conversation.id, "test", None).unwrap().run.id;

    let mut model = worker_configuration();
    model.proposal_tools = vec![json!({
        "name": "call_mcp_tool",
        "description": "Managed Office",
        "parameters": { "type": "object" }
    })];
    let prompt_hash = model.hash().unwrap();

    let permission = FrozenPermission {
        mode: PermissionMode::Allow,
        project_root: Some(root.to_string_lossy().into_owned()),
        grants: vec![],
        approval_epoch: None,
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id.clone(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets: TimeBudgets::default(),
    };
    db.freeze_run_control(&binding).unwrap();
    let config = kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "coordinator-test-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: prompt_hash.clone(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0,
        turn_max_retries: 0,
        experimental_compute_job_notice: false,
    };
    db.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        &prompt_hash,
        &serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    db.freeze_kernel_model_config(&run_id, &model).unwrap();
    let mut initial = initial_input(&run_id, &prompt_hash);
    initial.messages = vec![json!({"role":"user","content":"import by artifact"})];
    db.freeze_kernel_initial_input(&initial).unwrap();
    let _ = clock;

    let sessions_dir = root.join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let server = db.get_mcp_server(crate::office::SERVER_ID).unwrap().unwrap();
    let digest = format!(
        "sha256:{}",
        hex::encode(
            Sha256::digest(
                json!({"id":server.id,"transport":server.transport,"command":server.command,
                    "args":server.args,"endpoint":server.endpoint_url,"definition":server.definition})
                    .to_string()
                    .as_bytes()
            )
        )
    );
    let scope = crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["call_mcp_tool".to_owned()].into_iter().collect(),
        mcp_server_hashes: [(crate::office::SERVER_ID.to_owned(), digest)].into_iter().collect(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: ["office_import_data".to_owned()].into_iter().collect(),
        lifecycle_hooks: vec![],
    };
    db.freeze_kernel_host_scope(&run_id, &scope).unwrap();

    (db, root, run_id, conversation.id, sessions_dir)
}

/// Register a saved compute artifact for one conversation and return its stable
/// compute-artifact id and the file path.
fn seed_artifact(
    db: &Database,
    sessions_dir: &std::path::Path,
    conversation: &str,
    run: &str,
    file_name: &str,
    bytes: &[u8],
) -> (String, std::path::PathBuf) {
    let outputs = crate::runtime_host::attachment_compute::safe_workspace(
        sessions_dir,
        conversation,
        run,
    )
    .unwrap()
    .join("outputs");
    std::fs::create_dir_all(&outputs).unwrap();
    let path = outputs.join(file_name);
    std::fs::write(&path, bytes).unwrap();
    let path_string = path.to_string_lossy().into_owned();
    let id = Database::computed_artifact_id(&path_string);
    let media_type = if file_name.ends_with(".csv") {
        "text/csv"
    } else {
        "text/plain"
    };
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
             VALUES(?1,?2,?3,?4,'created_file',?5,?6,?7,?8,'ready',?9,?9)",
            rusqlite::params![
                id,
                conversation,
                run,
                file_name,
                path_string,
                media_type,
                bytes.len() as i64,
                hex::encode(Sha256::digest(bytes)),
                crate::database::now_ms(),
            ],
        )
    })
    .unwrap();
    (id, path)
}

#[test]
fn kernel_gate_accepts_own_conversation_artifact_and_rejects_abuse() {
    let (db, root, run, conversation, sessions_dir) = artifact_gate_fixture();
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let scope = db.kernel_host_scope(&run).unwrap();
    let policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding,
        scope,
        database: Some(db.clone()),
        sessions_dir: Some(sessions_dir.clone()), artifacts_dir: None,
    };
    let csv = "name,qty\nalpha,1\nbeta,2\n";
    let (artifact_id, artifact_path) =
        seed_artifact(&db, &sessions_dir, &conversation, &run, "saved.csv", csv.as_bytes());

    let import = |id: &str, extra: serde_json::Value| {
        let mut arguments = json!({
            "output": "out.xlsx",
            "sheet": "Sheet1",
            "createSheet": false,
            "artifactId": id,
        });
        if let Some(object) = extra.as_object() {
            arguments.as_object_mut().unwrap().extend(object.into_iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_import_data",
            "arguments": arguments,
        })
    };

    // 1) The real frozen policy validate accepts the legitimate reference.
    let valid = import(&artifact_id, json!({}));
    policy.validate("call_mcp_tool", &valid).unwrap();

    // 2) The real policy decide does not deny the resolved reference: an MCP
    //    mutation reaches the human-approval gate (RequireApproval), proving
    //    the prepare inside decide resolved the artifact rather than refusing
    //    an unresolvable id.
    let decision = policy.decide(&run, "tc-import", "call_mcp_tool", &valid.to_string());
    assert!(
        !matches!(decision, PolicyDecision::Deny { .. }),
        "a resolved artifact import must reach the approval gate, not be denied: {decision:?}"
    );

    // 3) data + artifactId together are rejected before dispatch.
    let dual = import(
        &artifact_id,
        json!({ "data": "a,b\n1,2\n" }),
    );
    assert!(policy
        .validate("call_mcp_tool", &dual)
        .is_err_and(|error| error.contains("只能使用")));

    // 4) Tampered content (hash mismatch) is rejected.
    std::fs::write(&artifact_path, b"name,qty\nalpha,999\n").unwrap();
    assert!(policy
        .validate("call_mcp_tool", &valid)
        .is_err_and(|error| error.contains("校验失败")));

    // 5) A foreign conversation cannot resolve this run's artifact. Build the
    //    policy in-memory with a different frozen conversation but the same
    //    connector/scope; validate/decide scope every lookup to that
    //    conversation, so the scoped query finds nothing.
    let foreign = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("allow"))
        .unwrap();
    let foreign_binding = RunControlBinding {
        conversation_id: foreign.id.clone(),
        ..policy.binding.clone()
    };
    let foreign_scope = policy.scope.clone();
    let foreign_policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding: foreign_binding,
        scope: foreign_scope,
        database: Some(db.clone()),
        sessions_dir: Some(sessions_dir.clone()), artifacts_dir: None,
    };
    assert!(foreign_policy
        .validate("call_mcp_tool", &valid)
        .is_err_and(|error| error.contains("不存在")));

    // 6) A non-CSV saved file does not match the declared import format.
    let (txt_id, _) = seed_artifact(
        &db,
        &sessions_dir,
        &conversation,
        &run,
        "notes.txt",
        b"not a csv",
    );
    let wrong_format = import(&txt_id, json!({}));
    assert!(foreign_policy
        .validate("call_mcp_tool", &wrong_format)
        .map(|_| ())
        .unwrap_err()
        .contains("不存在"));
    assert!(policy
        .validate("call_mcp_tool", &wrong_format)
        .is_err_and(|error| error.contains("不一致")));

    // Nothing was written by any refused preparation.
    assert!(!root.join("out.xlsx").exists());
}
