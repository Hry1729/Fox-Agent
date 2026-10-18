//! Package 2-C1 §2/§4a: the progress summary and the continuation prompt.
//!
//! `delivery_outstanding` feeds `kernel_run_progress.summary_json`, which the
//! continuation entry point republishes as `recordedProgress`. It does **not**
//! decide the Run's terminal state, and nothing here reads it as a terminal gate:
//! the terminal is still `kernel.model_retry_exhausted`, unchanged.
//!
//! The card's requirement is that the summary counts a pass by the **same** rule the
//! two display consumers use, on the same row. The fixing shape is a real v2 row
//! whose stored `passed` the display surfaces withhold: before the fix the summary
//! counted it as delivered (the SQL asked `status<>'passed'`), so three consumers
//! gave two conclusions about one row.
//!
//! The Run is driven through the real worker exactly as the frozen regression does,
//! so the summary is produced by the production terminal path rather than written by
//! the fixture.

use super::*;
use crate::runtime_host::{kernel_host, RuntimeCommand};
use std::path::Path;

/// The frozen terminal-failure fixture script, with the mode the regression uses.
fn exit_script(root: &Path, name: &str) -> std::path::PathBuf {
    let path = root.join(name);
    std::fs::write(
        &path,
        include_str!("../../../../../../services/agent-runtime/test/fixtures/lost-live-frame.mjs")
            .replace("__MODE__", "exit")
            .replace("__ADAPTER__", crate::kernel_model_config::KERNEL_MODEL_ADAPTER),
    )
    .unwrap();
    path
}

/// Drive a Run to its terminal model failure and return the banked summary.
fn drive_to_terminal_summary(
    db: &Database,
    root: &Path,
    run_id: &str,
    clock: &dyn Clock,
    script: &Path,
) -> Value {
    let runtime = RuntimeCommand {
        program: "node".into(),
        script: Some(script.to_path_buf()),
    };
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    kernel_host::drive_with_actions(
        &kernel_host::acquire(root, run_id).unwrap(),
        db,
        clock,
        &CancellationRegistry::default(),
        run_id,
        &runtime,
        "local-test-only",
        &Allow,
        |_, _, _| panic!("transport failure cannot execute a tool"),
        |_| Ok(()),
        |_| Ok(()),
        &preview,
    )
    .unwrap();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    serde_json::from_str(
        &connection
            .query_row(
                "SELECT summary_json FROM kernel_run_progress WHERE run_id=?1",
                [run_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap()
}

/// One promised item, seeded in v2 scope with a stored `passed` whose own record is
/// gone — the row the display consumers withhold.
fn seed(withheld: bool) -> Vec<crate::database::DeliveryChecklistSeed> {
    let requirements = if withheld {
        serde_json::from_str(&requirements_json(host_v2_spec())).unwrap()
    } else {
        Vec::new()
    };
    vec![crate::database::DeliveryChecklistSeed {
        item_key: "file:proof.csv".into(),
        target_path: Some("proof.csv".into()),
        artifact_id: None,
        display_name: "proof.csv".into(),
        checks: vec!["exists".into()],
        requirements,
    }]
}

/// Bind the promised item to a **real** artifact on disk, so the third surface —
/// the artifact view — can be read for the same row the checklist and the summary
/// are read for. Without a real version row the artifact view reports its own
/// version state and would not be answering the pass question at all.
fn bind_real_artifact(db: &Database, root: &Path, run_id: &str) -> (String, String) {
    let path = root.join("proof.csv");
    std::fs::write(&path, b"item,total\nship,4\n").unwrap();
    let (hash, size) = crate::runtime_host::managed_files::hash_file(&path).unwrap();
    let artifact_id = "artifact-proof".to_owned();
    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET target_path=?1, artifact_id=?2
             WHERE run_id=?3 AND item_key='file:proof.csv'",
            rusqlite::params![path.to_string_lossy(), artifact_id, run_id],
        )?;
        connection.execute(
            "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,
                artifact_class,artifact_origin,storage_path,media_type,byte_size,sha256,status,
                created_at,updated_at)
             VALUES (?1,?2,?3,'proof.csv','created_file','deliverable','project',?4,
                NULL,?5,?6,'ready',1,1)",
            rusqlite::params![
                artifact_id,
                conversation_id,
                run_id,
                path.to_string_lossy(),
                size as i64,
                hash
            ],
        )?;
        connection.execute(
            "INSERT INTO managed_file_versions(id,conversation_id,run_id,tool_call_id,tool,
                storage_path,display_name,version_no,change_kind,after_hash,after_size,created_at)
             VALUES ('mv-1',?1,?2,'tool-1','write_file',?3,'proof.csv',1,'created',?4,?5,1)",
            rusqlite::params![
                conversation_id,
                run_id,
                path.to_string_lossy(),
                hash,
                size as i64
            ],
        )?;
        Ok(())
    })
    .unwrap();
    (conversation_id, artifact_id)
}

/// The checklist row as the real projection reports it.
fn projected_status(db: &Database, conversation_id: &str, item_key: &str) -> String {
    db.load_conversation(conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == item_key)
        .expect("the promised item is projected")
        .status
}

/// The artifact as the real artifact entry reports it.
fn artifact_status(db: &Database, conversation_id: &str, artifact_id: &str) -> String {
    db.artifact_for_conversation(conversation_id, artifact_id)
        .unwrap()
        .expect("the artifact exists")
        .delivery
        .expect("a deliverable project artifact carries a delivery view")
        .verification_status
}

fn unbound_rule() -> Value {
    serde_json::json!({"type": "unbound", "reason": "未绑定"})
}

/// A legal Host v2 contract, serialized through the real stored requirement types.
fn host_v2_spec() -> Value {
    serde_json::json!({
        "schemaVersion": 2,
        "envelopeVersion": 2,
        "checkerVersion": 2,
        "ruleId": "content:concat",
        "policy": {"comparison": "textSegments", "eof": "allowSingleTrailingLf"},
        "rule": unbound_rule(),
    })
}

fn requirements_json(spec: Value) -> String {
    use crate::database::{DeliveryRequirement, RequirementKind};
    serde_json::to_string(&vec![DeliveryRequirement {
        item_key: None,
        id: "c-0".to_owned(),
        kind: RequirementKind::ContentContract { spec },
        source_text: "内容契约".to_owned(),
    }])
    .unwrap()
}

/// The defect itself, on the real terminal path: a row the display consumers
/// withhold must also count as outstanding in the summary.
///
/// Before the fix the summary's SQL counted it as delivered (`status='passed'`), so
/// `deliveryOutstanding` was 0 and the continuation was told the promised work was
/// done — while both display surfaces said `pending` about the same row.
#[test]
fn c1_the_summary_counts_a_withheld_pass_as_outstanding() {
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let config = worker_configuration();
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    db.seed_delivery_checklist(&run_id, &seed(true), crate::database::now_ms())
        .unwrap();
    // A stored pass with no record: exactly the shape the v2 contract obliges this
    // row to carry one for.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET status='passed', finding_json=NULL
             WHERE run_id=?1",
            [&run_id],
        )?;
        Ok(())
    })
    .unwrap();
    // The display consumer must withhold the same row, or this proves nothing.
    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [&run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let projected = db
        .load_conversation(&conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == "file:proof.csv")
        .expect("the promised item is projected")
        .status;
    assert_eq!(
        projected, "pending",
        "the display consumer withholds this row's pass"
    );

    freeze_host_scope(&db, &run_id);
    let script = exit_script(&root, "withheld-pass.mjs");
    let summary = drive_to_terminal_summary(&db, &root, &run_id, &clock, &script);

    assert_eq!(
        summary["deliveryItems"], 1,
        "one promised item: {summary}"
    );
    assert_eq!(
        summary["deliveryOutstanding"], 1,
        "a withheld pass is outstanding, agreeing with the display consumers: {summary}"
    );
    assert_eq!(
        summary["deliverablesSatisfiedReason"], "promised_deliverables_outstanding",
        "{summary}"
    );
    assert_eq!(summary["nextAction"], "continue_remaining_work", "{summary}");
    assert_eq!(summary["deliverablesSatisfied"], false, "{summary}");

    // The terminal itself is unchanged: this is a summary correction, not a new
    // terminal condition.
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let code: String = connection
        .query_row(
            "SELECT json_extract(payload_json,'$.code') FROM kernel_events
             WHERE run_id=?1 AND event_type='run.failed'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        code, "kernel.model_retry_exhausted",
        "the terminal is the existing model-retry exhaustion, not a delivery gate"
    );
}

/// The card's three-way requirement, on the two damages it names: for **one** row in
/// **one** state, the checklist projection, the artifact view and the progress
/// summary must give one answer, and that answer must not be "passed".
///
/// The pre-fix counterexample for both shapes is in `prefix-probe.log`: the checklist
/// projection read `passed` for a lost envelope and `passed` for a whole deleted
/// finding column. Those are the two rows this test re-reads after the fix.
#[test]
fn c1_all_three_surfaces_agree_when_the_record_is_gone() {
    for (tag, delete_whole_column) in [("lost-envelope", false), ("lost-column", true)] {
        let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
        let config = worker_configuration();
        let budgets = TimeBudgets {
            model_request_ms: 15_000,
            model_first_response_ms: 15_000,
            model_idle_ms: 15_000,
            run_execution_ms: 45_000,
            ..TimeBudgets::default()
        };
        let (db, root, run_id) = fixture_with_budgets_opt(
            &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
        );
        db.seed_delivery_checklist(&run_id, &seed(true), crate::database::now_ms())
            .unwrap();
        let (conversation_id, artifact_id) = bind_real_artifact(&db, &root, &run_id);

        // The two damaged shapes the card names. The envelope case keeps a finding
        // object that names this exact artifact and version but drops the verdict
        // envelope; the column case removes the finding outright.
        let finding = if delete_whole_column {
            "NULL".to_owned()
        } else {
            format!(
                "'{}'",
                serde_json::json!({
                    "boundPath": root.join("proof.csv").to_string_lossy(),
                    "managedVersionId": "mv-1",
                    "reason": null
                })
            )
        };
        db.with_connection(|connection| {
            connection.execute(
                &format!(
                    "UPDATE delivery_checklist_items
                     SET status='passed', finding_json={finding} WHERE run_id=?1"
                ),
                [&run_id],
            )?;
            Ok(())
        })
        .unwrap();

        // Two of the three surfaces, read before the Run reaches its terminal.
        let checklist = projected_status(&db, &conversation_id, "file:proof.csv");
        let artifact = artifact_status(&db, &conversation_id, &artifact_id);
        assert_ne!(
            checklist, "passed",
            "`{tag}`: the checklist projection must withhold the pass"
        );
        assert_ne!(
            artifact, "passed",
            "`{tag}`: the artifact view must withhold the pass (got {artifact})"
        );

        freeze_host_scope(&db, &run_id);
        let script = exit_script(&root, &format!("three-way-{tag}.mjs"));
        let summary = drive_to_terminal_summary(&db, &root, &run_id, &clock, &script);

        // The third surface, produced by the real terminal path.
        assert_eq!(
            summary["deliveryOutstanding"], 1,
            "`{tag}`: the summary must agree with the other two surfaces: {summary}"
        );
        assert_eq!(
            summary["deliverablesSatisfiedReason"], "promised_deliverables_outstanding",
            "`{tag}`: {summary}"
        );
        assert_eq!(summary["deliverablesSatisfied"], false, "`{tag}`: {summary}");
        assert_eq!(
            summary["deliveryItems"], 1,
            "`{tag}`: one promised item, one withheld pass: {summary}"
        );

        // Still a summary correction rather than a new terminal gate.
        let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        let code: String = connection
            .query_row(
                "SELECT json_extract(payload_json,'$.code') FROM kernel_events
                 WHERE run_id=?1 AND event_type='run.failed'",
                [&run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            code, "kernel.model_retry_exhausted",
            "`{tag}`: the terminal stays the existing model-retry exhaustion"
        );
    }
}

/// A row the displays **keep** as passed still counts as delivered, so the fix did
/// not simply make every row outstanding.
#[test]
fn c1_a_kept_pass_still_counts_as_delivered() {
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let config = worker_configuration();
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    db.seed_delivery_checklist(&run_id, &seed(false), crate::database::now_ms())
        .unwrap();
    // A legal legacy pass: no envelope, no content contract. It keeps its status.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET status='passed',
                finding_json='{\"boundPath\":\"proof.csv\",\"managedVersionId\":null,\"reason\":null}'
             WHERE run_id=?1",
            [&run_id],
        )?;
        Ok(())
    })
    .unwrap();
    freeze_host_scope(&db, &run_id);
    let script = exit_script(&root, "kept-pass.mjs");
    let summary = drive_to_terminal_summary(&db, &root, &run_id, &clock, &script);

    assert_eq!(
        summary["deliveryOutstanding"], 0,
        "a kept pass is delivered: {summary}"
    );
    assert_eq!(
        summary["deliverablesSatisfiedReason"], "all_promised_deliverables_passed",
        "{summary}"
    );
    assert_eq!(summary["nextAction"], "continue_for_final_reply", "{summary}");
}

/// The continuation prompt republishes the same summary as `recordedProgress`, so the
/// corrected count reaches the entry point the user is actually offered — and it
/// survives a real reopen of the database.
#[test]
fn c1_the_continuation_prompt_carries_the_corrected_progress_and_survives_a_reopen() {
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let config = worker_configuration();
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    db.seed_delivery_checklist(&run_id, &seed(true), crate::database::now_ms())
        .unwrap();
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET status='passed', finding_json=NULL
             WHERE run_id=?1",
            [&run_id],
        )?;
        Ok(())
    })
    .unwrap();
    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [&run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    freeze_host_scope(&db, &run_id);
    let script = exit_script(&root, "continuation-prompt.mjs");
    let summary = drive_to_terminal_summary(&db, &root, &run_id, &clock, &script);
    assert_eq!(summary["deliveryOutstanding"], 1, "{summary}");

    // A real reopen, so the prompt is built from durable state rather than from the
    // in-process handle that just wrote it.
    drop(db);
    let reopened = Database::open(root.join("facts.db")).unwrap();
    let prompt = reopened
        .kernel_continuation_prompt(&conversation_id, &run_id)
        .unwrap();
    assert!(
        prompt.contains("\"deliveryOutstanding\":1"),
        "the continuation prompt must carry the corrected outstanding count: {prompt}"
    );
    assert!(
        prompt.contains("\"deliverablesSatisfiedReason\":\"promised_deliverables_outstanding\""),
        "the continuation prompt must carry the corrected reason: {prompt}"
    );
    assert!(
        prompt.contains("\"recordedProgress\""),
        "the progress is republished as recordedProgress: {prompt}"
    );

    // The banked row itself, read back after reopen, is the same record.
    let banked = reopened
        .kernel_continuable_run(&conversation_id, &run_id)
        .unwrap();
    assert_eq!(banked.summary["deliveryOutstanding"], 1);
    assert_eq!(banked.pause_reason, "model_failure");
}

/// The write defence applies the same row binding as the three readers.
///
/// Driven here rather than beside the matrix cases because the finalize path needs
/// a real Kernel Run: `kernel_decision_marks.run_id` is a foreign key onto
/// `kernel_runs`, and a checklist-only fixture has no such row to commit against.
#[test]
fn b_the_finalize_defence_withholds_a_foreign_record() {
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let config = worker_configuration();
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    db.seed_delivery_checklist(&run_id, &seed(true), crate::database::now_ms())
        .unwrap();
    freeze_host_scope(&db, &run_id);

    // A staged `passed` row whose envelope names a different item: the same
    // file, the same version, the same supporting fact, only the key differs.
    let envelope = serde_json::json!({
        "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
        "itemKey": "file:another-item.csv",
        "class": "confirmed", "aggregateClass": "confirmed",
        "itemCount": 1, "terminal": true, "repairBudgetRemaining": true,
        "facts": [{
            "class": "passed", "origin": "checkUnbound", "ruleId": "content:0",
            "blocking": false, "needsRepair": false
        }]
    });
    let finding = serde_json::json!({
        "boundPath": "proof.csv",
        "reason": null,
        "deliveryVerdict": envelope
    })
    .to_string();
    let now = crate::database::now_ms();
    let mark = format!("test-decision:{}", uuid::Uuid::new_v4());
    db.stage_delivery_outcome(
        &run_id,
        &mark,
        &[crate::database::StagedDeliveryItem {
            item_key: "file:proof.csv".into(),
            passed: true,
            finding_json: finding.clone(),
        }],
        None,
        now,
    )
    .unwrap();
    db.record_decision_mark_for_test(&run_id, &mark, now).unwrap();
    db.finalize_staged_delivery_outcome(&run_id, &mark, now).unwrap();

    let written: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT status FROM delivery_checklist_items
                 WHERE run_id=?1 AND item_key='file:proof.csv'",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        written, "pending",
        "a staged pass supported only by another item's record is not written as passed"
    );

    // And the reading surfaces agree with the write, rather than each deciding.
    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [&run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert_ne!(
        projected_status(&db, &conversation_id, "file:proof.csv"),
        "passed",
        "the checklist projection withholds the same row"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn b_row_binding_reaches_real_summary_continuation_and_reopen() {
    for (tag,key) in [("foreign",serde_json::json!("file:other.csv")),("missing",Value::Null),("illegal",serde_json::json!(7))] {
        let clock=crate::runtime_host::shadow_reconcile::ReconcilerClock;let config=worker_configuration();
        let budgets=TimeBudgets {model_request_ms:15_000,model_first_response_ms:15_000,model_idle_ms:15_000,run_execution_ms:45_000,..TimeBudgets::default()};
        let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1),true,budgets);
        db.seed_delivery_checklist(&run,&seed(true),crate::database::now_ms()).unwrap();
        let (conversation,artifact)=bind_real_artifact(&db,&root,&run);freeze_host_scope(&db,&run);
        let mut finding=serde_json::json!({"boundPath":root.join("proof.csv").to_string_lossy(),"managedVersionId":"mv-1","reason":null,
            "deliveryVerdict":{"verdictSchemaVersion":2,"verdictEnvelopeVersion":2,"itemKey":"file:proof.csv","class":"confirmed","aggregateClass":"confirmed",
                "itemCount":1,"terminal":true,"repairBudgetRemaining":true,"facts":[{"class":"passed","origin":"checkUnbound","blocking":false,"needsRepair":false}]}});
        let now=crate::database::now_ms();let mark=format!("b-binding:{}",uuid::Uuid::new_v4());
        db.stage_delivery_outcome(&run,&mark,&[crate::database::StagedDeliveryItem {item_key:"file:proof.csv".into(),passed:true,finding_json:finding.to_string()}],None,now).unwrap();
        db.record_decision_mark_for_test(&run,&mark,now).unwrap();db.finalize_staged_delivery_outcome(&run,&mark,now).unwrap();
        assert_eq!(projected_status(&db,&conversation,"file:proof.csv"),"passed");
        assert_eq!(artifact_status(&db,&conversation,&artifact),"passed","real matching version positive control");
        finding["deliveryVerdict"]["itemKey"]=key;
        db.with_connection(|connection|{connection.execute("UPDATE delivery_checklist_items SET status='passed',finding_json=?2 WHERE run_id=?1",rusqlite::params![run,finding.to_string()])?;Ok(())}).unwrap();
        assert_eq!(projected_status(&db,&conversation,"file:proof.csv"),"pending");
        assert_eq!(artifact_status(&db,&conversation,&artifact),"unverified");
        let script=exit_script(&root,&format!("b-row-{tag}.mjs"));let summary=drive_to_terminal_summary(&db,&root,&run,&clock,&script);
        assert_eq!(summary["deliveryOutstanding"],1,"{tag}: {summary}");
        drop(db);let reopened=Database::open(root.join("facts.db")).unwrap();
        let prompt=reopened.kernel_continuation_prompt(&conversation,&run).unwrap();
        assert!(prompt.contains("\"deliveryOutstanding\":1"));assert!(prompt.contains("recordedProgress"));
        assert_eq!(projected_status(&reopened,&conversation,"file:proof.csv"),"pending");
        assert_eq!(artifact_status(&reopened,&conversation,&artifact),"unverified");
    }
}
