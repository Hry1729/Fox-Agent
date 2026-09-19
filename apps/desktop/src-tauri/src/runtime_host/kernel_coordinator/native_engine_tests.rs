use super::*;
use std::io::BufRead;

struct Provider(std::process::Child);
impl Drop for Provider {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn native_engine_flow(engine: &str, native: Option<crate::kernel_model_config::NativeAdapterConfig>) {
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let scratch = std::env::temp_dir().join(format!("fox-native-provider-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&scratch).unwrap();
    let log = scratch.join("requests.jsonl");
    let mut child = std::process::Command::new("node")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime/test/fixtures/native-model-server.mjs"))
        .arg(engine).arg(&log).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let mut url = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut url).unwrap();
    let _provider = Provider(child);
    let mut config = worker_configuration();
    config.engine_id = engine.into(); config.native_adapter = native;
    config.model_service = json!({"apiType":if engine=="codex" {"openai-responses"} else {"openai-completions"},
        "modelId":if engine=="codex" {"gpt-5.1-codex"} else {"deepseek-v4-flash"},"baseUrl":url.trim(),"maxOutputTokens":512,"reasoning":false});
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let scope = freeze_host_scope(&db, &run_id);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial_with_worker("native-first", &Ask, &real_worker_command(), "isolated").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "waiting_approval");
    for id in ["read-a", "read-b"] { db.queue_kernel_host_command(&run_id, Some((id,"allow_once"))).unwrap(); }
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_model_config(&run_id).unwrap(), config);
    assert_eq!(db.run_control_binding(&run_id).unwrap().unwrap().engine_id, engine);
    let policy = super::super::super::kernel_gateway::GatewayPolicy { binding: db.run_control_binding(&run_id).unwrap().unwrap(), scope , database: None, sessions_dir: None, artifacts_dir: None };
    let executions = AtomicUsize::new(0);
    super::super::super::kernel_host::drive(super::super::super::kernel_host::acquire(&root,&run_id).unwrap(),
        &db,&clock,&cancellation,&run_id,&real_worker_command(),"isolated",&policy,|binding,effect,token| {
            executions.fetch_add(1,Ordering::SeqCst);
            let payload:Value=serde_json::from_str(&effect.payload_json).unwrap();
            Ok((true,crate::resource_gateway::execute(binding,"read",&payload["input"],token)?))
        }).unwrap();
    assert_eq!(executions.load(Ordering::SeqCst),2);
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state,"completed");
    let requests=std::fs::read_to_string(log).unwrap();
    assert_eq!(requests.lines().count(),2);
    let second=requests.lines().nth(1).unwrap();
    assert!(second.contains("read-a") && second.contains("read-b") && second.contains("FOX_EXECUTION_RECEIPT_V1"));
    assert!(second.contains("durable coordinator"));
    assert!(db.pending_kernel_host_commands(&run_id).unwrap().is_empty());
    drop(_provider);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_deepseek_host_approvals_reads_and_restart_continuation() { native_engine_flow("deepseek_harness",None); }

#[test]
#[ignore = "requires FOX_TEST_CODEX_BINARY; run explicitly against the installed native executable"]
fn native_codex_host_approvals_reads_and_restart_continuation() {
    use sha2::{Digest,Sha256};
    let command=std::env::var("FOX_TEST_CODEX_BINARY").expect("explicit native Codex binary");
    let binary_hash=format!("sha256:{}",hex::encode(Sha256::digest(std::fs::read(&command).unwrap())));
    native_engine_flow("codex",Some(crate::kernel_model_config::NativeAdapterConfig {command,binary_hash}));
}

#[test]
fn native_engine_configuration_cannot_cross_a_frozen_identity() {
    let clock=TestClock::new(1000);
    let mut config=worker_configuration(); config.engine_id="deepseek_harness".into();
    config.model_service=json!({"apiType":"openai-completions","modelId":"test","baseUrl":"http://localhost"});
    let (db,root,run)=fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let mut wrong=config.clone(); wrong.engine_id="pi".into();
    assert!(db.freeze_kernel_model_config(&run,&wrong).is_err());
    drop(db);
    let db=Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_model_config(&run).unwrap(),config);
    let connection=rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute("UPDATE kernel_runs SET engine_id='codex' WHERE run_id=?1",[&run]).unwrap();
    assert!(db.kernel_model_config(&run).is_err());
}
