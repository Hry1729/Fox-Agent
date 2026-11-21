use super::*;
use crate::database::companion_coordination::Assignment;
use fox_engine_protocol::{PermissionMode, TimeBudgets};
use rusqlite::OptionalExtension;
use std::{
    io::{BufRead, BufReader},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tauri::Manager;

const CHILD: &str = "FOX_TEST_COMPANION_COORDINATION_CHILD";
const TEST: &str =
    "runtime_host::companion_coordination_tests::coordination_native_pi_delivers_and_returns_with_user_approval";
const TARGET: &str = "验收接收者";
const CANARY: &str = "FOX_ORIGIN_PRIVATE_CANARY";

struct Provider(Child);
impl Drop for Provider {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct HostCleanup(RuntimeHost);
impl Drop for HostCleanup {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn provider(root: &Path, recipient: &str) -> (Provider, String) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../../services/agent-runtime/test/fixtures/companion-coordination-model-server.mjs",
    );
    let mut provider = Provider(
        Command::new("node")
            .arg(script)
            .arg(root.join("provider-requests.jsonl"))
            .arg(recipient)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start isolated loopback provider"),
    );
    let mut address = String::new();
    BufReader::new(provider.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    assert!(address.starts_with("http://127.0.0.1:"), "{address}");
    let address = address.trim().to_owned();
    let socket = address
        .strip_prefix("http://")
        .and_then(|endpoint| endpoint.strip_suffix("/v1"))
        .unwrap();
    TcpStream::connect(socket).unwrap_or_else(|error| {
        panic!("isolated model requires loopback TCP at {socket}: {error}")
    });
    (provider, address)
}

fn exercise_child() {
    let fixture = std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("fox-coordination-native-{}", uuid::Uuid::new_v4()));
    let sessions = fixture.join("runtime-sessions");
    let attachments = fixture.join("attachments");
    let skills = fixture.join("skills");
    for directory in [&sessions, &attachments, &skills] {
        std::fs::create_dir_all(directory).unwrap();
    }
    println!("coordination native Pi evidence: {}", fixture.display());
    if let Some(executable) = std::env::var_os("FOX_TEST_NATIVE_SIDECAR_BIN") {
        std::env::set_var("FOX_RUNTIME_EXECUTABLE", executable);
    } else {
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../services/agent-runtime/src/pi-runtime.mjs");
        assert!(runtime.is_file());
        std::env::set_var("FOX_RUNTIME_SCRIPT", runtime);
    }

    let db = Database::open(fixture.join("fox.db")).unwrap();
    let chief = crate::commands::companions::ensure_default(&db, &fixture).unwrap();
    let sender = db.create_blank_companion(&fixture).unwrap();
    crate::companion_workspace::repair(&db, &fixture, &sender.profile.id).unwrap();
    let peer = db.create_blank_companion(&fixture).unwrap();
    let peer = db
        .edit_companion(
            &peer.profile.id,
            &crate::database::companions::CompanionProfileEdit {
                display_name: Some(TARGET.into()),
                avatar_ref: None,
                duty: None,
                persona_override: None,
            },
        )
        .unwrap();
    let peer_workspace =
        crate::companion_workspace::repair(&db, &fixture, &peer.profile.id).unwrap();
    std::fs::write(peer_workspace.join("proof.txt"), "FOX_TARGET_FILE_PROOF").unwrap();
    let (_provider, endpoint) = provider(&fixture, &peer.profile.id);
    db.save_model_service(&crate::database::ModelServiceRecord {
        name: "companion-coordination-fixture".into(),
        base_url: endpoint,
        model_id: "fox-coordination-fixture".into(),
        api_type: "openai-completions".into(),
        context_window: 131072,
        max_output_tokens: 1024,
        supports_image_input: false,
        enabled: true,
        credential_configured: false,
        connection_type: "local".into(),
        last_status: "unknown".into(),
        last_latency_ms: None,
        last_checked_at: None,
        created_at: 1,
        updated_at: 1,
    })
    .unwrap();
    let work_title = format!("协作验收 {CANARY}");
    let work = db
        .create_conversation(db.default_agent_id(), Some(&work_title), None, Some("ask"))
        .unwrap();

    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let host = RuntimeHost::new(
        app.handle().clone(),
        db.clone(),
        sessions,
        attachments,
        skills.clone(),
        client.clone(),
    );
    let cleanup = HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(
        db.clone(),
        host.clone(),
        client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(), db.clone(), client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(
            &crate::local_knowledge_storage::resolve_startup_root(&fixture, None),
            None,
        ),
        fixture.clone(),
        skills,
    ));
    host.start_digital_colleague_scheduler();

    let ordinary_run = send(&host, &db, &work.id, "FOX_COORD_ORDINARY_SCOPE 检查工具");
    wait_for(&host, &fixture, "ordinary chat tool catalog", || {
        (run_completed(&db, &ordinary_run)
            && assistant_includes(&db, &ordinary_run, "FOX_COORD_ORDINARY_SCOPE_OK")).then_some(())
    });
    assert!(assignment(&db, &work.id, &ordinary_run).is_none());

    let root_text = format!("FOX_COORD_ROOT_WORK 请 @{TARGET} 检查文件。{CANARY}");
    let root_run = send(&host, &db, &sender.conversation_id, &root_text);
    let assigned = wait_for(&host, &fixture, "assigned companion handoff", || {
        assignment(&db, &sender.conversation_id, &root_run)
    });
    assert_eq!(assigned.source_companion_id.as_deref(), Some(sender.profile.id.as_str()));
    assert_eq!(assigned.source_name, sender.profile.display_name);
    assert_eq!(assigned.recipient_companion_id, peer.profile.id);
    let (pending, approval_id) = wait_for(&host, &fixture, "recipient approval", || {
        let item = assignment(&db, &sender.conversation_id, &root_run)?;
        if item.status != "needs_input" {
            return None;
        }
        let run = item.recipient_run_id.as_deref()?;
        let approval = pending_approval(&db, run)?;
        Some((item, approval))
    });
    assert!(pending.resume_run_id.is_none());
    assert!(!peer_workspace.join("confirmed.txt").exists());
    let projected = db
        .load_conversation(&pending.recipient_conversation_id)
        .expect("load the actual recipient conversation for the UI");
    let recipient_run = pending.recipient_run_id.as_deref().unwrap();
    assert!(projected.approvals.iter().any(|item| item.id == approval_id));
    assert!(projected.tool_calls.iter().any(|item| item.run_id == recipient_run));
    assert!(projected.runtime_events.iter().any(|item| item.run_id == recipient_run));
    let notification = db
        .list_app_notifications(false, 100)
        .unwrap()
        .into_iter()
        .find(|item| {
            item.source_type == "companion"
                && item.action["coordinationConversationId"] == pending.recipient_conversation_id
                && item.kind == "approval"
        })
        .expect("the recipient approval is in notifications");
    assert_eq!(notification.source_id, peer.profile.id);
    assert!(host
        .resolve_approval(&approval_id, ApprovalDecision::AllowOnce)
        .unwrap());
    let returned = wait_for(
        &host,
        &fixture,
        "automatic wake with actual work result",
        || returned_assignment(&db, &sender.conversation_id, &root_run, "已汇总 FOX_COORD_WORK_RESULT"),
    );
    assert_eq!(returned.status, "completed");
    assert_eq!(
        returned.result.as_deref(),
        Some("FOX_COORD_WORK_RESULT FOX_TARGET_FILE_PROOF")
    );
    assert_eq!(
        std::fs::read_to_string(peer_workspace.join("confirmed.txt")).unwrap(),
        "FOX_COORD_APPROVED_WRITE"
    );

    let chief_run = send(
        &host,
        &db,
        &chief.conversation_id,
        "FOX_COORD_CHIEF_CREATE 请创建新伙伴",
    );
    wait_for(&host, &fixture, "default companion P0 tool from Pi", || {
        let actions = crate::companion_team::list(&db, &chief.profile.id).unwrap();
        (run_completed(&db, &chief_run)
            && assistant_includes(&db, &chief_run, "FOX_COORD_CHIEF_CREATED")
            && !actions.is_empty())
        .then_some(())
    });
    assert!(db
        .list_companions()
        .unwrap()
        .companions
        .iter()
        .any(|item| item.profile.display_name == "验收新伙伴"));

    let creations = crate::companion_team::list(&db, &chief.profile.id).unwrap();
    let created = serde_json::to_value(&creations[0]).unwrap();
    crate::companion_team::resolve(
        app.state::<crate::app_state::AppState>().inner(),
        created["id"].as_str().unwrap(),
        crate::companion_team::Decision::Undo,
    ).unwrap();
    let recreated_run = send(&host, &db, &chief.conversation_id, "FOX_COORD_CHIEF_RECREATE 重新新建同名伙伴");
    wait_for(&host, &fixture, "recreate after undo in the same native session", || {
        (run_completed(&db, &recreated_run)
            && assistant_includes(&db, &recreated_run, "FOX_COORD_CHIEF_CREATED"))
            .then_some(())
    });
    let new_peer = db.list_companions().unwrap().companions.into_iter()
        .find(|item| item.profile.display_name == "验收新伙伴").unwrap();
    assert_ne!(created["targetId"], new_peer.profile.id);
    for (index, text) in ["FOX_COORD_CHIEF_ARCHIVE 提议归档", "FOX_COORD_CHIEF_ARCHIVE_AGAIN 再次提议归档"].iter().enumerate() {
        let run = send(&host, &db, &chief.conversation_id, text);
        wait_for(&host, &fixture, "new archive card after cancellation", || {
            (run_completed(&db, &run) && assistant_includes(&db, &run, "FOX_COORD_CHIEF_ARCHIVE_PENDING")).then_some(())
        });
        let cards = crate::companion_team::list(&db, &chief.profile.id).unwrap();
        let archive = cards.iter().map(|item| serde_json::to_value(item).unwrap())
            .filter(|item| item["kind"] == "archive").collect::<Vec<_>>();
        assert_eq!(archive.len(), index + 1);
        if index == 0 {
            crate::companion_team::resolve(
                app.state::<crate::app_state::AppState>().inner(),
                archive[0]["id"].as_str().unwrap(),
                crate::companion_team::Decision::Dismiss,
            ).unwrap();
        }
    }

    let consult_run = send(
        &host,
        &db,
        &chief.conversation_id,
        &format!("FOX_COORD_ROOT_CONSULT 请向伙伴咨询。{CANARY}"),
    );
    let blocked = wait_for(&host, &fixture, "recipient routes missing information back to the source", || {
        let item = assignment(&db, &chief.conversation_id, &consult_run)?;
        (item.status == "failed" && item.resume_run_id.is_some()).then_some(item)
    });
    assert_eq!(blocked.source_name, chief.profile.display_name);
    assert!(blocked.question.is_none());
    let consult_recipient_run = blocked.recipient_run_id.as_deref().unwrap();
    assert_eq!(
        db.run_control_binding(consult_recipient_run)
            .unwrap()
            .unwrap()
            .permission
            .mode,
        PermissionMode::ReadOnly
    );
    assert!(db
        .ensure_companion_tool_granted(
            consult_recipient_run,
            "write_file",
            Some(&json!({"path":"forbidden.txt","content":"never"})),
        )
        .is_err());
    assert!(!peer_workspace.join("forbidden.txt").exists());
    let follow_up_run = blocked.resume_run_id.as_deref().unwrap();
    let followed_up = wait_for(&host, &fixture, "source asks the same peer to resend before escalating", || {
        returned_assignment(
            &db,
            &chief.conversation_id,
            follow_up_run,
            "现有资料仍然只有汇总",
        )
    });
    assert!(followed_up.consult);
    assert!(followed_up.result.as_deref().unwrap().contains("FOX_COORD_CONSULT_STILL_MISSING"));
    assert_eq!(followed_up.recipient_conversation_id, blocked.recipient_conversation_id);
    assert!(!peer_workspace.join("forbidden.txt").exists());
    let question_run = followed_up.resume_run_id.as_deref().unwrap();
    let detail = db.load_conversation(&chief.conversation_id).unwrap();
    let question = detail.runtime_events.iter().find(|event|
        event.run_id == question_run && event.event_type == "user.question.requested").unwrap();
    assert_eq!(question.event["questions"][0]["options"].as_array().unwrap().len(), 2);
    assert!(detail.bubbles.iter().any(|bubble| {
        bubble.run_id.as_deref() == Some(question_run)
            && bubble.delivered_at <= question.created_at
            && bubble.content.contains("我请核对员补发明细")
    }));
    db.sync_app_notifications().unwrap();
    assert!(db.list_app_notifications(false, 100).unwrap().iter().all(|item|
        !item.merge_key.starts_with("companion-assignment:")
            && item.merge_key != format!("run:{consult_run}")
            && item.merge_key != format!("run:{follow_up_run}")));
    let started = crate::commands::run_resume(
        app.handle().clone(),
        app.state::<crate::app_state::AppState>(),
        crate::database::ResumeRunRequest {
            conversation_id: chief.conversation_id.clone(),
            parent_run_id: question_run.to_owned(),
            text: "接下来怎么交付？：FOX_COORD_USER_ANSWER 先交汇总".to_owned(),
            answers: json!({"q-1":"FOX_COORD_USER_ANSWER 先交汇总"}),
            agent_backend: Some("fox".to_owned()),
        },
    ).data.expect("the desktop can submit the contact's selected option");
    wait_for(&host, &fixture, "source delivers the chosen result in the same chat", || {
        (run_completed(&db, &started.run.id)
            && assistant_includes(&db, &started.run.id, "已按你的选择交付 FOX_COORD_CONSULT_RESULT")).then_some(())
    });

    let model_log = std::fs::read_to_string(fixture.join("provider-requests.jsonl")).unwrap();
    for raw in model_log.lines() {
        let item: Value = serde_json::from_str(raw).unwrap();
        assert!(item.get("error").is_none(), "{item}");
    }
    drop(cleanup);
}

fn send(host: &RuntimeHost, db: &Database, conversation: &str, prompt: &str) -> String {
    let started = db.create_run(conversation, prompt, None).unwrap();
    db.freeze_default_run_control_for_engine(
        &started.run.id,
        "legacy",
        TimeBudgets::continuous(),
        "pi",
    )
    .unwrap();
    host.start_run(&started, prompt, &[]).unwrap();
    started.run.id
}

fn assignment(db: &Database, conversation: &str, source_run: &str) -> Option<Assignment> {
    db.companion_assignments_list(conversation)
        .unwrap()
        .into_iter()
        .find(|item| item.source_run_id == source_run)
}

fn returned_assignment(
    db: &Database,
    conversation: &str,
    source_run: &str,
    text: &str,
) -> Option<Assignment> {
    let item = assignment(db, conversation, source_run)?;
    let resume_run = item.resume_run_id.as_deref()?;
    (run_completed(db, resume_run) && assistant_includes(db, resume_run, text)).then_some(item)
}

fn pending_approval(db: &Database, run: &str) -> Option<String> {
    db.with_connection(|conn| conn.query_row(
        "SELECT a.id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE t.run_id=?1 AND a.status='pending'",
        [run], |row| row.get(0),
    ).optional()).unwrap()
}

fn run_completed(db: &Database, id: &str) -> bool {
    db.with_connection(|conn| {
        conn.query_row(
            "SELECT status='completed' FROM runs WHERE id=?1",
            [id],
            |row| row.get(0),
        )
    })
    .unwrap()
}

fn assistant_includes(db: &Database, id: &str, needle: &str) -> bool {
    db.with_connection(|conn| conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE run_id=?1 AND role='assistant' AND instr(content,?2)>0)",
        [id, needle], |row| row.get(0),
    )).unwrap()
}

fn wait_for<T>(
    host: &RuntimeHost,
    fixture: &Path,
    label: &str,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(fact) = check() {
            return fact;
        }
        if Instant::now() >= deadline {
            let diagnostics = host.diagnostics();
            let log = std::fs::read_to_string(fixture.join("provider-requests.jsonl"))
                .unwrap_or_default();
            let errors = log
                .lines()
                .filter(|line| line.starts_with(r#"{"error":"#))
                .collect::<Vec<_>>();
            panic!("timed out: {label}; fixture={}; state={}; error={:?}; stderr={:?}; providerErrors={errors:?}", fixture.display(), diagnostics.state, diagnostics.last_error, diagnostics.stderr_tail);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn coordination_native_pi_delivers_and_returns_with_user_approval() {
    if std::env::var_os(CHILD).is_some() {
        exercise_child();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("FOX_KERNEL_MODE", "authoritative")
        .env_remove("FOX_RUNTIME_EXECUTABLE")
        .env_remove("FOX_RUNTIME_SCRIPT")
        .env_remove("FOX_RUNTIME_MODE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(115);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated native coordination test timed out");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "native coordination test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
