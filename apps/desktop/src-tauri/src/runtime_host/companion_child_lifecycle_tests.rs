use super::*;
use crate::database::{CreateChildRunInput, CreateDigitalColleagueInput};

fn delegate(database: &Database, parent: &str, call: &str) -> StartRunResult {
    database
        .create_host_tool_call(
            parent,
            call,
            "child_run_start",
            &json!({}),
            "running",
            false,
        )
        .unwrap();
    database
        .create_child_run(CreateChildRunInput {
            parent_run_id: parent,
            tool_call_id: call,
            worker_agent_id: "fox-general",
            objective: "Review the evidence",
            context: "",
            budget: &ChildRunBudget {
                max_duration_ms: 30_000,
                max_total_tokens: 4_000,
                max_output_tokens: 1_000,
                max_tool_calls: 4,
            },
            team_run_id: None,
            team_member_id: None,
            allowed_tools: None,
        })
        .unwrap()
        .0
}

#[test]
fn companion_child_lifecycle_stops_descendants_before_releasing_workspace() {
    let root = std::env::temp_dir().join(format!("fox-child-lifecycle-{}", Uuid::new_v4()));
    for directory in ["sessions", "attachments", "skills"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let database = Database::open(root.join("facts.db")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let host = RuntimeHost::new(
        app.handle().clone(),
        database.clone(),
        root.join("sessions"),
        root.join("attachments"),
        root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap(),
    );

    let ordinary = database
        .create_conversation("fox-general", Some("Ordinary"), None, None)
        .unwrap();
    let companion = database.create_blank_companion(&root).unwrap();
    let task = database
        .create_digital_colleague(&CreateDigitalColleagueInput {
            name: "Scheduled review".into(),
            expert_id: String::new(),
            objective: "Review the evidence".into(),
            project_id: None,
            project_root: None,
            knowledge_references: Vec::new(),
            max_runs_per_day: 8,
            max_tokens_per_day: 10_000,
            max_duration_ms: 60_000,
            max_output_tokens: 2_000,
            max_tool_calls: 20,
            companion_id: Some(companion.profile.id.clone()),
        })
        .unwrap();
    let ordinary_run = database.create_run(&ordinary.id, "Review", None).unwrap();
    let private_run = database
        .create_run(&companion.conversation_id, "Review", None)
        .unwrap();
    let background_run = database
        .accept_digital_colleague_trigger(
            &task.id,
            "manual",
            None,
            "manual:review",
            &json!({}),
            None,
            "user",
        )
        .unwrap()
        .prepared
        .expect("dispatch background task")
        .started;

    for parent in [ordinary_run, private_run, background_run] {
        let parent_id = parent.run.id.clone();
        database
            .apply_runtime_event(&parent_id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let child = delegate(&database, &parent_id, "stop-delegation");
        let companion_slot = database.run_companion_slot(&parent_id).unwrap();
        host.state.lock().unwrap().queued_runs.push_back(QueuedRun {
            started: parent,
            text: "Review".into(),
            attachments: Vec::new(),
            companion: companion_slot,
        });
        assert!(host.cancel_managed_run_by_user(&parent_id).unwrap());
        assert_eq!(
            database.child_run_status(&child.run.id).unwrap().as_deref(),
            Some("cancelled"),
        );
    }

    let parent = database
        .create_run(&companion.conversation_id, "Finish review", None)
        .unwrap();
    database
        .apply_runtime_event(&parent.run.id, 1, &json!({"type":"run.started"}))
        .unwrap();
    let live_child = delegate(&database, &parent.run.id, "pending-delegation");
    let released_child = delegate(&database, &parent.run.id, "completed-delegation");
    database
        .apply_runtime_event(&released_child.run.id, 1, &json!({"type":"run.started"}))
        .unwrap();
    database
        .apply_runtime_event(&released_child.run.id, 2, &json!({"type":"run.completed"}))
        .unwrap();

    let mut sessions = sidecar_sessions::Sessions::default();
    sessions
        .start(
            &parent.run.conversation_id,
            "parent-session",
            1,
            &parent.run.id,
            Some((&companion.profile.id, false)),
        )
        .unwrap();
    let child_slot = database.run_companion_slot(&released_child.run.id).unwrap();
    sessions
        .start(
            &released_child.run.conversation_id,
            "child-session",
            1,
            &released_child.run.id,
            child_slot
                .as_ref()
                .map(|(id, background)| (id.as_str(), *background)),
        )
        .unwrap();

    let mut command = Command::new("node");
    command
        .args(["-e", "process.stdin.resume()"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut process = command.spawn().expect("start inert sidecar");
    let stdin = Arc::new(Mutex::new(process.stdin.take().unwrap()));
    let process = Arc::new(Mutex::new(process));
    host.state.lock().unwrap().worker = Some(WorkerHandle {
        child: process.clone(),
        stdin,
        pending: Arc::new(Mutex::new(HashMap::new())),
        sessions,
        session_leases: HashMap::new(),
        _job: process_job::ProcessJob::none(),
    });

    database
        .apply_runtime_event(&parent.run.id, 2, &json!({"type":"run.completed"}))
        .unwrap();
    host.mark_run_ready(&parent.run.id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while database
        .child_run_status(&live_child.run.id)
        .unwrap()
        .as_deref()
        != Some("cancelled")
    {
        assert!(
            Instant::now() < deadline,
            "parent completion did not cancel its child"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let next_can_enter = |sessions: &sidecar_sessions::Sessions| {
        sessions.can_admit(&task.conversation_id, Some(&companion.profile.id))
    };
    {
        let mut state = host.state.lock().unwrap();
        let sessions = &mut state.worker.as_mut().unwrap().sessions;
        assert!(
            !next_can_enter(sessions),
            "the parent must keep its slot while its child still has a session owner"
        );
        assert!(sessions.finish(&released_child.run.id));
        state.queue_changed.1.notify_all();
    }
    while !next_can_enter(&host.state.lock().unwrap().worker.as_ref().unwrap().sessions) {
        assert!(
            Instant::now() < deadline,
            "the next top-level Run could not enter"
        );
        thread::sleep(Duration::from_millis(10));
    }

    drop(host);
    let mut process = process.lock().unwrap();
    let _ = process.kill();
    let _ = process.wait();
    drop(database);
    drop(app);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn companion_child_budget_failure_stops_root_before_releasing_slot() {
    let root = std::env::temp_dir().join(format!("fox-child-budget-stop-{}", Uuid::new_v4()));
    for directory in ["sessions", "attachments", "skills"] { std::fs::create_dir_all(root.join(directory)).unwrap(); }
    let database = Database::open(root.join("facts.db")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = RuntimeHost::new(app.handle().clone(), database.clone(), root.join("sessions"),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    let companion = database.create_blank_companion(&root).unwrap();
    let task = database.create_owned_companion_task(&CreateDigitalColleagueInput {
        name: "Scheduled review".into(), expert_id: String::new(), objective: "Review".into(),
        project_id: None, project_root: None, knowledge_references: Vec::new(), max_runs_per_day: 8,
        max_tokens_per_day: 128_000, max_duration_ms: 300_000, max_output_tokens: 4_096,
        max_tool_calls: 16, companion_id: Some(companion.profile.id.clone()),
    }, crate::database::DigitalColleagueScheduleSpec::Interval { interval_seconds: 3_600, timezone: "UTC".into() }, true).unwrap().task;
    let parent = database.accept_digital_colleague_trigger(&task.id, "manual", None, "budget:stop", &json!({}), None, "user")
        .unwrap().prepared.unwrap().started;
    let parent_id = &parent.run.id;
    database.apply_runtime_event(parent_id, 1, &json!({"type":"run.started"})).unwrap();
    let child = delegate(&database, parent_id, "delegate");
    for index in 0..15 {
        database.create_host_tool_call(parent_id, &format!("read-{index}"), "read", &json!({}), "running", false).unwrap();
    }
    let error = database.create_host_tool_call(parent_id, "read-16", "read", &json!({}), "running", false).unwrap_err();
    assert!(error.contains("digital_colleague.tool_budget_exceeded"), "{error}");
    let mut sessions = sidecar_sessions::Sessions::default();
    sessions.start(&parent.run.conversation_id, "parent-session", 1, parent_id,
        Some((&companion.profile.id, true))).unwrap();
    let mut command = Command::new("node");
    command.args(["-e", "process.stdin.resume()"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut process = command.spawn().unwrap();
    let stdin = Arc::new(Mutex::new(process.stdin.take().unwrap()));
    let process = Arc::new(Mutex::new(process));
    host.state.lock().unwrap().worker = Some(WorkerHandle {
        child: process.clone(), stdin, pending: Arc::new(Mutex::new(HashMap::new())), sessions,
        session_leases: HashMap::new(), _job: process_job::ProcessJob::none(),
    });
    host.reject_owned_task_budget(&child.run.id, "ordinary failure");
    let parent_status: String = database.with_connection(|connection|
        connection.query_row("SELECT status FROM runs WHERE id = ?1", [parent_id], |row| row.get(0))).unwrap();
    assert_eq!(parent_status, "running");
    let transition = host.run_transition.lock().unwrap();
    host.reject_owned_task_budget(&child.run.id, &error);
    let (status, code, message): (String, Option<String>, Option<String>) = database.with_connection(|connection| connection.query_row(
        "SELECT status, error_code, error_message FROM runs WHERE id = ?1", [parent_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )).unwrap();
    assert_eq!(status, "failed");
    assert_eq!(code.as_deref(), Some("digital_colleague.budget_exceeded"));
    assert!(message.unwrap().contains("digital_colleague.tool_budget_exceeded"));
    assert!(!host.state.lock().unwrap().worker.as_ref().unwrap().sessions.can_admit(&task.conversation_id, Some(&companion.profile.id)));
    drop(transition);
    let deadline = Instant::now() + Duration::from_secs(10);
    while database.child_run_status(&child.run.id).unwrap().as_deref() != Some("cancelled") {
        assert!(Instant::now() < deadline, "budget failure did not cancel child");
        thread::sleep(Duration::from_millis(10));
    }
    drop(host);
    let mut process = process.lock().unwrap();
    let _ = process.kill();
    let _ = process.wait();
    drop(database);
    drop(app);
    let _ = std::fs::remove_dir_all(root);
}
