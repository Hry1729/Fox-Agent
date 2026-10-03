use super::*;
use std::time::Duration;
use tauri::Manager;

#[test]
fn independent_conversations_reach_three_model_endpoints_before_either_finishes() {
    let (first_arrived_tx, first_arrived_rx) = std::sync::mpsc::channel();
    let (first_release_tx, first_release_rx) = std::sync::mpsc::channel();
    let (first_address, first_server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"First complete"})],
        Some((0, Box::new(move || {
            first_arrived_tx.send(()).unwrap();
            first_release_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        }))),
    );
    let (second_arrived_tx, second_arrived_rx) = std::sync::mpsc::channel();
    let (second_release_tx, second_release_rx) = std::sync::mpsc::channel();
    let (second_address, second_server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"Second complete"})],
        Some((0, Box::new(move || {
            second_arrived_tx.send(()).unwrap();
            second_release_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        }))),
    );
    let (third_arrived_tx, third_arrived_rx) = std::sync::mpsc::channel();
    let (third_address, third_server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"Third complete"})],
        Some((0, Box::new(move || { third_arrived_tx.send(()).unwrap(); }))),
    );
    let mut first_model = worker_configuration();
    first_model.model_service = json!({"apiType":"openai-completions",
        "modelId":"kernel-http-test","baseUrl":format!("http://{first_address}/v1")});
    first_model.proposal_tools.clear();
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, first) = fixture_with_retry_opt(
        &clock, &first_model.hash().unwrap(), Some(&first_model), true, true, (0, 1));
    let conversation = db.create_conversation(db.default_agent_id(), None,
        Some(root.to_str().unwrap()), Some("read_only")).unwrap();
    let second_started = db.create_run(&conversation.id, "Second conversation", None).unwrap();
    let second = second_started.run.id.clone();
    let mut second_binding = db.run_control_binding(&first).unwrap().unwrap();
    second_binding.run_id = second.clone();
    second_binding.conversation_id = conversation.id;
    db.freeze_run_control(&second_binding).unwrap();
    let mut second_model = first_model.clone();
    second_model.model_service["baseUrl"] = json!(format!("http://{second_address}/v1"));
    let second_hash = second_model.hash().unwrap();
    let first_frozen_json: String = db.with_connection(|conn| conn.query_row(
        "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1", [&first], |row| row.get(0))).unwrap();
    let mut second_frozen: kernel::RunFrozenConfig = serde_json::from_str(&first_frozen_json).unwrap();
    second_frozen.prompt_config_hash = second_hash.clone();
    db.kernel_create_run(&second, "pi", "authoritative", 2,
        &second_binding.permission_snapshot_id, &second_binding.execution_profile_id,
        &second_hash, &serde_json::to_string(&second_frozen).unwrap()).unwrap();
    db.freeze_kernel_model_config(&second, &second_model).unwrap();
    db.freeze_kernel_initial_input(&initial_input(&second, &second_hash)).unwrap();
    let third_conversation = db.create_conversation(db.default_agent_id(), None,
        Some(root.to_str().unwrap()), Some("read_only")).unwrap();
    let third_started = db.create_run(&third_conversation.id, "Third conversation", None).unwrap();
    let third = third_started.run.id.clone();
    let mut third_binding = second_binding.clone();
    third_binding.run_id = third.clone();
    third_binding.conversation_id = third_conversation.id;
    db.freeze_run_control(&third_binding).unwrap();
    let mut third_model = second_model.clone();
    third_model.model_service["baseUrl"] = json!(format!("http://{third_address}/v1"));
    let third_hash = third_model.hash().unwrap();
    let mut third_frozen = second_frozen;
    third_frozen.prompt_config_hash = third_hash.clone();
    db.kernel_create_run(&third, "pi", "authoritative", 2,
        &third_binding.permission_snapshot_id, &third_binding.execution_profile_id,
        &third_hash, &serde_json::to_string(&third_frozen).unwrap()).unwrap();
    db.freeze_kernel_model_config(&third, &third_model).unwrap();
    db.freeze_kernel_initial_input(&initial_input(&third, &third_hash)).unwrap();
    for run in [&first, &second, &third] {
        db.freeze_kernel_host_scope(run, &crate::database::KernelHostScope {
            schema_version: 1, tool_names: Default::default(),
            mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(), office_tools: Default::default(),
            lifecycle_hooks: vec![],
        }).unwrap();
    }
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = crate::runtime_host::RuntimeHost::new(app.handle().clone(), db.clone(),
        root.clone(), root.join("attachments"), root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap());
    let first_binding = db.run_control_binding(&first).unwrap().unwrap();
    let host_a = host.clone();
    let root_a = root.clone();
    let first_id = first.clone();
    let first_task = std::thread::spawn(move || {
        let owner = crate::runtime_host::kernel_host::acquire(&root_a, &first_id).unwrap();
        host_a.start_kernel_run(owner, &first_binding, Value::Null, Value::Null)
    });
    first_arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    host.start_run_detached(second_started, "Second conversation".into(), vec![]);
    second_arrived_rx.recv_timeout(Duration::from_secs(20))
        .expect("second model request was blocked by the first conversation");
    host.start_run_detached(third_started, "Third conversation".into(), vec![]);
    third_arrived_rx.recv_timeout(Duration::from_secs(20))
        .expect("third conversation was blocked by a run or model-request count gate");
    second_release_tx.send(()).unwrap();
    first_release_tx.send(()).unwrap();
    first_task.join().unwrap().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while db.kernel_host_run_state(&second).unwrap().as_deref() != Some("completed")
        || db.kernel_host_run_state(&third).unwrap().as_deref() != Some("completed") {
        assert!(std::time::Instant::now() < deadline, "queued runs did not complete");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(db.kernel_host_run_state(&first).unwrap().as_deref(), Some("completed"));
    assert_eq!(db.kernel_host_run_state(&second).unwrap().as_deref(), Some("completed"));
    assert_eq!(db.kernel_host_run_state(&third).unwrap().as_deref(), Some("completed"));
    assert_eq!(first_server.join().unwrap().len(), 1);
    assert_eq!(second_server.join().unwrap().len(), 1);
    assert_eq!(third_server.join().unwrap().len(), 1);
}
