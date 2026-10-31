//! CX-2: `RuntimeHost::shutdown()` runs every step even when one fails, and
//! reports every failure together instead of returning at the first one.
use super::*;
use fox_engine_protocol::TimeBudgets;

#[test]
fn cx2_shutdown_keeps_cleaning_after_a_failed_step_and_reports_errors_together() {
    let root = std::env::temp_dir().join(format!("fox-cx2-shutdown-{}", uuid::Uuid::new_v4()));
    for directory in ["sessions", "attachments", "skills"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let database = Database::open(root.join("facts.db")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = RuntimeHost::new(
        app.handle().clone(),
        database.clone(),
        root.join("sessions"),
        root.join("attachments"),
        root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap(),
    );
    let conversation = database
        .create_conversation(database.default_agent_id(), Some("cx2 shutdown"), None, Some("allow"))
        .unwrap();
    let text = "queued before exit";
    let started = database.create_run(&conversation.id, text, None).unwrap();
    database
        .freeze_default_run_control_for_engine(&started.run.id, "legacy", TimeBudgets::continuous(), "pi")
        .unwrap();
    // A queued entry whose Run row does not exist: marking it interrupted fails.
    // It is queued FIRST, so a shutdown that stops at the first error would
    // leave the real queued Run untouched.
    let mut ghost = started.clone();
    ghost.run.id = "cx2-ghost-run".to_owned();
    {
        let mut state = host.state.lock().unwrap();
        state.queued_runs.push_back(QueuedRun { started: ghost, text: text.to_owned(), attachments: Vec::new() });
        state.queued_runs.push_back(QueuedRun { started: started.clone(), text: text.to_owned(), attachments: Vec::new() });
    }

    let error = host.shutdown().expect_err("a failed step is reported, not swallowed");
    assert!(error.starts_with("[runtime.shutdown_incomplete]"), "{error}");
    assert!(error.contains("queued cx2-ghost-run mark_interrupted"), "{error}");
    let (status, code): (String, Option<String>) = database
        .with_connection(|connection| {
            connection.query_row(
                "SELECT status, error_code FROM runs WHERE id = ?1",
                [&started.run.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(status, "interrupted", "the step after the failure still ran");
    assert_eq!(code.as_deref(), Some("runtime.application_exit"));
    assert!(host.state.lock().unwrap().queued_runs.is_empty());
    assert!(host.state.lock().unwrap().worker.is_none());
    drop(app);
    let _ = std::fs::remove_dir_all(root);
}
