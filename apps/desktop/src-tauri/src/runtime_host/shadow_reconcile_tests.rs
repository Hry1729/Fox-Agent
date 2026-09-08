//! Host-integration Kernel Shadow acceptance tests.
//!
//! These drive `ShadowReconciler` over a real (temporary file) `Database`,
//! bootstrapping a shadow run, feeding preflight/result/cancel/crash events
//! and asserting against durable `kernel_shadow_diffs` rows. They do not
//! require a Tauri `AppHandle` or a running runtime, but they exercise the
//! same DB-backed reconciliation path the Host uses (durable diffs, cursor
//! resume, context lifecycle, DB ToolCall projection lookups).

use serde_json::json;

use crate::database::Database;
use crate::kernel::{RunOutcome, ShadowDiff};
use crate::runtime_host::shadow_reconcile::ShadowReconciler;

fn test_db() -> Database {
    use std::sync::atomic::{AtomicU64, Ordering};
    static DB_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = DB_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "fox-shadow-it-{}-{}-{}.sqlite",
        std::process::id(),
        crate::database::now_ms(),
        seq,
    ));
    let _ = std::fs::remove_file(&path);
    Database::open(path).expect("open test database")
}

fn boot_run(db: &Database) -> (String, String) {
    let conversation = db
        .create_conversation(db.default_agent_id(), None, None, Some("ask"))
        .unwrap();
    let run = db
        .create_run(&conversation.id, "shadow integration", None)
        .unwrap()
        .run;
    db.apply_runtime_event(&run.id, 1, &json!({"type": "run.started"}))
        .unwrap();
    (run.id, conversation.id)
}

fn boot_reconciler(db: &Database, run_id: &str, conversation_id: &str) -> ShadowReconciler {
    let reconciler = ShadowReconciler::new(db.clone());
    reconciler
        .bootstrap(
            run_id,
            conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
        )
        .expect("bootstrap shadow");
    reconciler
}

/// Map a Result<ReconcileOutcome> to the persisted ShadowDiff, asserting the
/// durable write actually succeeded.
fn persisted(
    outcome: Result<crate::runtime_host::shadow_reconcile::ReconcileOutcome, String>,
) -> ShadowDiff {
    let outcome = outcome.expect("reconcile succeeded");
    assert!(
        outcome.persisted(),
        "diff must be persisted; error: {:?}",
        outcome.persist_error
    );
    outcome.diff.expect("a diff was produced")
}

#[test]
fn zero_tool_run_terminal_match_is_persisted_and_context_dropped() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let diff = persisted(reconciler.terminal(&run_id, "completed", RunOutcome::Completed, true));
    assert!(diff.is_match(), "genuine tool-less completed run matches");
    assert!(diff.kernel.tools.is_empty() && diff.legacy.tools.is_empty());
    let counts = reconciler.diff_counts(&run_id).unwrap();
    assert!(*counts.get("match").unwrap_or(&0) >= 1);
    assert!(!reconciler.has_context(&run_id));
    // Read the diff back from durable storage and assert identity/category.
    let terminal = reconciler
        .last_terminal_diff(&run_id)
        .expect("terminal diff");
    assert!(terminal.is_match());
    assert_eq!(terminal.legacy.run_state, "completed");
}

#[test]
fn preflight_allow_reads_back_identity_input_decision_and_category() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "pi-tcid-100";
    let outcome = reconciler
        .feed_preflight(&run_id, "read", tcid, json!({"path":"/a.txt"}), "allow")
        .expect("feed preflight");
    assert!(outcome.persisted());
    let diff = outcome.diff.unwrap();
    assert!(diff.is_match());
    // Read back the durable diff and assert exact toolCallId / count / input.
    let readback = reconciler
        .last_tool_diff(&run_id, tcid)
        .expect("durable tool diff");
    assert_eq!(readback.kernel.tools.len(), 1);
    assert_eq!(readback.legacy.tools.len(), 1);
    let k = &readback.kernel.tools[0];
    let l = &readback.legacy.tools[0];
    assert_eq!(k.tool_call_id, tcid);
    assert_eq!(l.tool_call_id, tcid);
    assert_eq!(k.tool, "read");
    assert!(k.canonical_input_json.contains("/a.txt"));
    assert_eq!(k.decision, "allow");
    assert_eq!(l.decision, "allow");
    reconciler
        .settle_tool(&run_id, tcid, true, r#"{"ok":true}"#)
        .unwrap();
    let terminal =
        persisted(reconciler.terminal(&run_id, "completed", RunOutcome::Completed, true));
    assert!(terminal.is_match());
}

#[test]
fn tool_iserror_maps_to_failed_terminal_and_does_not_fake_completed() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "pi-tcid-200";
    assert!(reconciler
        .feed_preflight(&run_id, "read", tcid, json!({"path":"/missing"}), "allow")
        .unwrap()
        .persisted());
    // isError=true -> ok=false.
    reconciler
        .settle_tool(&run_id, tcid, false, r#"{"isError":true,"error":"ENOENT"}"#)
        .unwrap();
    let terminal = persisted(reconciler.terminal(
        &run_id,
        "failed",
        RunOutcome::Failed {
            code: "tool.execution_failed".into(),
            message: "ENOENT".into(),
        },
        true,
    ));
    assert_eq!(terminal.legacy.run_state, "failed");
    assert_eq!(terminal.kernel.run_state, "failed");
    // The decision fact stays "allow" (result does not overwrite it).
    let readback = reconciler.last_tool_diff(&run_id, tcid).expect("tool diff");
    assert_eq!(readback.kernel.tools[0].decision, "allow");
}

#[test]
fn cancel_sequence_persists_cancelled_match_and_drops_context() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    reconciler.cancel(&run_id).expect("cancel feed");
    let diff = persisted(reconciler.terminal(&run_id, "cancelled", RunOutcome::Cancelled, true));
    assert!(diff.is_match());
    assert_eq!(diff.kernel.run_state, "cancelled");
    assert!(!reconciler.has_context(&run_id));
}

#[test]
fn nested_graph_read_skipped_for_valid_parent_and_observed_for_fabricated_parent() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // Outer graph_readonly_run as a RUNTIME-location ToolCall.
    db.apply_runtime_event(
        &run_id,
        2,
        &json!({
            "type": "tool.started",
            "toolCallId": "graph-parent-1",
            "tool": "graph_readonly_run",
            "input": {"goalId":"g1"}
        }),
    )
    .expect("project outer graph runtime tool");
    // Valid runtime graph parent -> skip inner independent registration.
    let skipped = reconciler
        .feed_nested_preflight(&run_id, "graph-parent-1")
        .expect("nested feed");
    assert!(skipped);
    assert!(!reconciler.has_tool_diff(&run_id, "inner-1"));
    // Fabricated/missing parent -> NOT skipped; the inner call proceeds down
    // the normal registration path and IS observed.
    let fake_skip = reconciler
        .feed_nested_preflight(&run_id, "fabricated-parent")
        .expect("nested feed fake");
    assert!(!fake_skip);
    // The inner call is then observed via a normal preflight feed.
    let outcome = reconciler
        .feed_preflight(&run_id, "grep", "inner-2", json!({"pattern":"y"}), "allow")
        .expect("inner observed");
    assert!(outcome.persisted());
    assert!(reconciler.has_tool_diff(&run_id, "inner-2"));
}

#[test]
fn unresolved_tool_before_terminal_never_fakes_completed_match() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "pi-tcid-300";
    assert!(reconciler
        .feed_preflight(&run_id, "read", tcid, json!({"path":"/x"}), "allow")
        .unwrap()
        .persisted());
    // Completed WITHOUT settling the mid-flight tool -> kernel rejects.
    let diff = persisted(reconciler.terminal(&run_id, "completed", RunOutcome::Completed, true));
    assert_ne!(
        diff.kernel.run_state, "completed",
        "completed with an unresolved tool must not be a fake Match"
    );
    assert!(diff.is_divergence() || diff.kernel.run_state != "completed");
}

#[test]
fn no_preflight_runtime_projection_is_swept_and_completes_consistently() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    // A RUNTIME-canonical tool (`read`) with NO preflight observed by shadow,
    // already terminal. The sweep must observe and settle it as allow, then
    // complete to a genuine Match (not an empty-tool fake completion).
    db.apply_runtime_event(
        &run_id,
        2,
        &json!({
            "type":"tool.started","toolCallId":"rt-1","tool":"read","input":{"path":"/x"}
        }),
    )
    .unwrap();
    db.apply_runtime_event(
        &run_id,
        3,
        &json!({
            "type":"tool.completed","toolCallId":"rt-1","result":{"content":"ok"}
        }),
    )
    .unwrap();
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, true)
        .expect("terminal");
    assert!(outcome.persisted());
    let diff = outcome.diff.expect("diff");
    // The sweep must observe and settle rt-1 as allow (runtime tool).
    let fact = diff
        .legacy
        .tools
        .iter()
        .find(|t| t.tool_call_id == "rt-1")
        .expect("swept runtime tool present");
    assert_eq!(fact.decision, "allow");
    assert!(
        diff.is_match(),
        "successful sweep of a settled runtime tool produces a consistent Match"
    );
    assert!(
        diff.kernel.tools.iter().any(|t| t.tool_call_id == "rt-1"),
        "swept runtime tool must appear in kernel facts"
    );
}

#[test]
fn host_canonical_tool_never_promoted_from_runtime_is_deny() {
    // web_search is a HOST-canonical tool. If its projection never promotes
    // execution_location to "host" (still "runtime", e.g. an early reject), it
    // never acquired execution authority -> legacy decision deny.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    // tool.started projects a runtime-location ToolCall (NOT promoted to host).
    db.apply_runtime_event(
        &run_id,
        2,
        &json!({
            "type":"tool.started","toolCallId":"web-pending-1","tool":"web_search",
            "input":{"query":"x"}
        }),
    )
    .unwrap();
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let outcome = reconciler
        .terminal(
            &run_id,
            "failed",
            RunOutcome::Failed {
                code: "interrupted".into(),
                message: "host tool never promoted".into(),
            },
            true,
        )
        .expect("terminal");
    assert!(outcome.persisted());
    let diff = outcome.diff.expect("diff");
    // The swept legacy fact for web-pending-1 must be DENY (assert the
    // decision, not just presence).
    let legacy_fact = diff
        .legacy
        .tools
        .iter()
        .find(|t| t.tool_call_id == "web-pending-1")
        .expect("swept tool present");
    assert_eq!(
        legacy_fact.decision, "deny",
        "host-canonical tool still at runtime location must be deny"
    );
}

#[test]
fn promoted_host_tool_running_is_allow_even_while_executing() {
    // web_search promoted to execution_location "host" (Host acquired
    // execution authority) — even while still running, its decision is allow
    // (not inferred from terminal state).
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    db.create_host_tool_call(
        &run_id,
        "web-promoted-1",
        "web_search",
        &json!({"query":"x"}),
        "running",
        false,
    )
    .unwrap();
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // The run completes while the host tool is mid-flight; the sweep must
    // classify it by contract+location (host, no approval -> allow), NOT deny.
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, true)
        .expect("terminal");
    let diff = outcome.diff.expect("diff");
    let fact = diff
        .legacy
        .tools
        .iter()
        .chain(diff.kernel.tools.iter())
        .find(|t| t.tool_call_id == "web-promoted-1")
        .expect("promoted host tool present");
    assert_eq!(
        fact.decision, "allow",
        "promoted host tool must be allow even while executing"
    );
}

#[test]
fn runtime_graph_readonly_tool_classified_by_canonical_contract() {
    // graph_readonly_run is a genuine RUNTIME tool; sweep classifies it by its
    // real contract (runtime inspection -> allow), independent of promotion.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    db.apply_runtime_event(
        &run_id,
        2,
        &json!({
            "type":"tool.started","toolCallId":"graph-1","tool":"graph_readonly_run",
            "input":{"goalId":"g1"}
        }),
    )
    .unwrap();
    db.apply_runtime_event(
        &run_id,
        3,
        &json!({
            "type":"tool.completed","toolCallId":"graph-1","result":{}
        }),
    )
    .unwrap();
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, true)
        .unwrap();
    let diff = outcome.diff.expect("diff");
    let fact = diff
        .legacy
        .tools
        .iter()
        .find(|t| t.tool_call_id == "graph-1")
        .expect("graph runtime tool present");
    assert_eq!(
        fact.decision, "allow",
        "runtime graph_readonly_run classifies as allow by contract"
    );
    assert!(
        diff.is_match(),
        "runtime tool settled and consistent -> Match"
    );
}

#[test]
fn diverging_terminal_states_are_classified_not_forced_to_match() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // Legacy reports completed but the kernel reaches failed (e.g. unresolved
    // issue): independent terminal facts must surface a mismatch.
    let diff = persisted(reconciler.terminal(
        &run_id,
        "completed",
        RunOutcome::Failed {
            code: "kernel.disagreement".into(),
            message: "kernel failed".into(),
        },
        false,
    ));
    assert_ne!(
        diff.legacy.run_state, diff.kernel.run_state,
        "independent terminal facts must not be forced to match"
    );
}

#[test]
fn failed_tool_then_run_failed_is_consistent() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "write-tcid";
    // write_file requires approval on the kernel side; legacy allowed it;
    // then it failed -> run failed. This asserts the failure path.
    assert!(reconciler
        .feed_preflight(&run_id, "write_file", tcid, json!({"path": "/z"}), "allow")
        .unwrap()
        .persisted());
    reconciler
        .settle_tool(&run_id, tcid, false, r#"{"isError":true,"error":"boom"}"#)
        .unwrap();
    let diff = persisted(reconciler.terminal(
        &run_id,
        "failed",
        RunOutcome::Failed {
            code: "x".into(),
            message: "boom".into(),
        },
        true,
    ));
    assert_eq!(diff.kernel.run_state, "failed");
    assert_eq!(diff.legacy.run_state, "failed");
}

#[test]
fn bootstrap_keeps_state_registered_tool_survives_into_terminal() {
    // Required order: register an UNSETTLED tool -> same-config bootstrap
    // -> settle the original tool -> terminal. The toolCallId, count and
    // expected category must survive the re-bootstrap; an implementation that
    // clears the context would fail here (the terminal would not complete).
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = ShadowReconciler::new(db.clone());
    let cfg = ShadowReconciler::shadow_config("legacy");
    reconciler
        .bootstrap(&run_id, &conversation_id, "legacy", cfg.clone())
        .unwrap();
    // Register an unsettled tool BEFORE re-bootstrap.
    assert!(reconciler
        .feed_preflight(&run_id, "read", "keep-1", json!({"path": "/k"}), "allow")
        .unwrap()
        .persisted());
    // Re-bootstrap with the SAME frozen config (idempotent, state retained).
    reconciler
        .bootstrap(&run_id, &conversation_id, "legacy", cfg)
        .unwrap();
    // Settle the ORIGINAL tool after the re-bootstrap: the context must still
    // know about keep-1 (in-memory state not reset).
    reconciler
        .settle_tool(&run_id, "keep-1", true, r#"{"ok":true}"#)
        .unwrap();
    let terminal =
        persisted(reconciler.terminal(&run_id, "completed", RunOutcome::Completed, false));
    assert!(
        terminal.is_match(),
        "kept unsettled tool settles and completes -> Match"
    );
    // Assert the exact toolCallId, count and category survive in the terminal.
    let kernel_tools: Vec<_> = terminal
        .kernel
        .tools
        .iter()
        .filter(|t| t.tool_call_id == "keep-1")
        .collect();
    assert_eq!(
        kernel_tools.len(),
        1,
        "terminal retains exactly keep-1 on kernel side"
    );
    let legacy_tools: Vec<_> = terminal
        .legacy
        .tools
        .iter()
        .filter(|t| t.tool_call_id == "keep-1")
        .collect();
    assert_eq!(
        legacy_tools.len(),
        1,
        "terminal retains exactly keep-1 on legacy side"
    );
    assert_eq!(kernel_tools[0].decision, "allow");
}

#[test]
fn failed_context_bootstrap_returns_error_not_ok() {
    // A context already in Failed state cannot be revived by re-bootstrap.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = ShadowReconciler::new(db.clone());
    reconciler
        .bootstrap(
            &run_id,
            &conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
        )
        .unwrap();
    // Force a durable-write failure -> context becomes Failed.
    db.execute_raw_sql(
        "CREATE TRIGGER fail_shadow_bootstrap
         BEFORE INSERT ON kernel_shadow_diffs
         BEGIN SELECT RAISE(FAIL, 'simulated'); END;",
    )
    .unwrap();
    let outcome = reconciler.feed_preflight(&run_id, "read", "x", json!({"path": "/p"}), "allow");
    assert!(outcome.is_ok());
    assert!(!outcome.unwrap().persisted());
    assert_eq!(reconciler.context_state(&run_id), "failed");
    // Re-bootstrap must ERROR, not return Ok over an unusable state.
    let result = reconciler.bootstrap(
        &run_id,
        &conversation_id,
        "legacy",
        ShadowReconciler::shadow_config("legacy"),
    );
    assert!(
        result.is_err(),
        "re-bootstrap of a failed context must fail"
    );
    assert_eq!(reconciler.context_state(&run_id), "failed");
}

#[test]
fn bootstrap_rejects_conflicting_frozen_config() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = ShadowReconciler::new(db.clone());
    reconciler
        .bootstrap(
            &run_id,
            &conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
        )
        .unwrap();
    // Re-bootstrap with a DIFFERENT profile/frozen config must fail closed.
    let mut conflicting = ShadowReconciler::shadow_config("legacy");
    conflicting.prompt_config_hash = "sha256:different".into();
    let result = reconciler.bootstrap(&run_id, &conversation_id, "legacy", conflicting);
    assert!(
        result.is_err(),
        "conflicting frozen config must be rejected"
    );
}

#[test]
fn diff_insert_failure_surfaces_persist_error_and_failed_state() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = ShadowReconciler::new(db.clone());
    reconciler
        .bootstrap(
            &run_id,
            &conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
        )
        .unwrap();
    // Install a trigger that rejects inserts into kernel_shadow_diffs,
    // simulating a durable-write failure on a real temp file database.
    db.execute_raw_sql(
        "CREATE TRIGGER fail_shadow_diff_insert
         BEFORE INSERT ON kernel_shadow_diffs
         BEGIN SELECT RAISE(FAIL, 'simulated write failure'); END;",
    )
    .expect("install reject trigger");
    // A tool-batch diff cannot be persisted: persist_error is surfaced.
    let outcome = reconciler
        .feed_preflight(&run_id, "read", "fail-1", json!({"path":"/x"}), "allow")
        .expect("reconcile step ran");
    assert!(
        !outcome.persisted(),
        "write failure must NOT report persisted"
    );
    assert!(outcome.persist_error.is_some());
    // The run context is in a queryable failed state.
    assert_eq!(reconciler.context_state(&run_id), "failed");
    // A subsequent terminal attempt surfaces an error (failed context is not
    // silently reconciled as success).
    let terminal = reconciler.terminal(&run_id, "completed", RunOutcome::Completed, false);
    assert!(
        terminal.is_err(),
        "a failed context must not be silently reconciled as a terminal success"
    );
}

#[test]
fn unknown_context_is_named_and_errors() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let _ = (run_id, conversation_id);
    let reconciler = ShadowReconciler::new(db.clone());
    // A run that was never bootstrapped must error (distinct name).
    let outcome = reconciler.terminal(
        "never-bootstrapped-run",
        "completed",
        RunOutcome::Completed,
        false,
    );
    assert!(outcome.is_err(), "missing context must be an Err");
    assert_eq!(reconciler.context_state("never-bootstrapped-run"), "absent");
}

#[test]
fn failed_tool_then_run_completed_is_a_consistent_match() {
    // A tool that returns isError=true reaches a terminal FAILED state (not
    // unresolved). The model retries/recovers and the Run ultimately reports
    // completed on BOTH sides: a failed-but-terminal tool does not block
    // run.completed, so this is a genuine completed Match (not a forced failed
    // terminal). Decision facts remain "allow" (a result never overwrites the
    // policy decision).
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "read-recovers";
    assert!(reconciler
        .feed_preflight(&run_id, "read", tcid, json!({"path": "/bad"}), "allow")
        .unwrap()
        .persisted());
    // isError=true -> ok=false. The tool becomes terminal (Failed), which the
    // state machine counts as resolved for run.completed.
    reconciler
        .settle_tool(&run_id, tcid, false, r#"{"isError":true,"error":"ENOENT"}"#)
        .unwrap();
    // Legacy reports a genuine Run completion (the model recovered).
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, false)
        .expect("terminal step");
    assert!(outcome.persisted());
    let diff = outcome.diff.expect("diff");
    // Both sides reach completed: the failed-but-terminal tool does not block.
    assert_eq!(diff.legacy.run_state, "completed");
    assert_eq!(diff.kernel.run_state, "completed");
    assert!(
        diff.is_match(),
        "recovered run after a terminal tool failure matches"
    );
    // Decision fact stays allow; the failure is a result, not a decision change.
    assert_eq!(
        diff.legacy
            .tools
            .iter()
            .find(|t| t.tool_call_id == tcid)
            .unwrap()
            .decision,
        "allow",
        "decision fact stays allow after an isError result"
    );
    assert_eq!(
        diff.kernel
            .tools
            .iter()
            .find(|t| t.tool_call_id == tcid)
            .unwrap()
            .decision,
        "allow"
    );
}

#[test]
fn unresolved_tool_blocks_completed_and_surfaces_divergence() {
    // Control case: a tool that is still RUNNING (no result) makes
    // run.completed illegal on the kernel side, so legacy=completed vs kernel
    // non-completed is a divergence — never a faked Match.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let tcid = "read-hanging";
    assert!(reconciler
        .feed_preflight(&run_id, "read", tcid, json!({"path": "/x"}), "allow")
        .unwrap()
        .persisted());
    // No settle_tool -> the tool is still running/unresolved.
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, false)
        .expect("terminal step");
    assert!(outcome.persisted());
    let diff = outcome.diff.expect("diff");
    assert_ne!(
        diff.kernel.run_state, "completed",
        "an unresolved running tool blocks kernel run.completed"
    );
    assert!(
        !diff.is_match(),
        "must not fake a Match with an unresolved tool"
    );
}

#[test]
fn monotonic_clock_is_non_decreasing_and_separate_from_wall() {
    use crate::kernel::Clock;
    use crate::runtime_host::shadow_reconcile::ReconcilerClock;
    // Monotonic elapsed is Instant-based: repeated reads never go backwards,
    // even if the wall clock were to jump or roll back (the reconciliation
    // elapsed domain does not read SystemTime).
    let clock = ReconcilerClock;
    let m1 = clock.now_monotonic_ms();
    let mut last = m1;
    for _ in 0..8 {
        std::thread::sleep(std::time::Duration::from_millis(1));
        let now = clock.now_monotonic_ms();
        assert!(now >= last, "monotonic elapsed must never decrease");
        last = now;
    }
    // Wall time is a separate, positive reading.
    let wall = clock.now_wall_ms();
    assert!(wall > 0);
    // Both feed into the reconciler independently: feed_batch passes mono for
    // the elapsed domain and wall for persistence (asserted by construction in
    // shadow_reconcile::feed_batch/sweep).
}

#[test]
fn terminal_insert_failure_keeps_failed_state() {
    // A durable failure at the TERMINAL point (not just preflight) must
    // surface persist_error and leave the context queryable (failed), never
    // report a clean closure.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // Reject inserts only after the run starts — install trigger now.
    db.execute_raw_sql(
        "CREATE TRIGGER fail_shadow_terminal
         BEFORE INSERT ON kernel_shadow_diffs
         BEGIN SELECT RAISE(FAIL, 'terminal write failure'); END;",
    )
    .unwrap();
    let outcome = reconciler
        .terminal(&run_id, "completed", RunOutcome::Completed, false)
        .expect("terminal step returns an outcome");
    assert!(
        !outcome.persisted(),
        "terminal write failure must not report persisted"
    );
    assert!(outcome.persist_error.is_some());
    // Context retained in a queryable failed state (not dropped).
    assert_eq!(reconciler.context_state(&run_id), "failed");
    assert!(
        reconciler.last_terminal_diff(&run_id).is_none(),
        "no terminal diff was durably persisted"
    );
}

#[test]
fn unknown_tool_in_sweep_marks_context_failed_not_fabricated_deny() {
    // A DB-projected tool absent from the canonical catalog must fail the
    // sweep (the decision is an error), never be fabricated as a legacy deny.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    // Insert a runtime ToolCall with an unknown tool name directly.
    db.create_host_tool_call(
        &run_id,
        "mystery-1",
        "totally_made_up_tool_xyz",
        &json!({"q": 1}),
        "running",
        false,
    )
    .unwrap();
    // Force it to read as a host-location unknown tool via runtime event is
    // awkward; instead use host location (promoted) — still unknown in catalog.
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    let result = reconciler.terminal(
        &run_id,
        "completed",
        RunOutcome::Completed,
        true, // sweep
    );
    assert!(result.is_err(), "unknown tool in sweep must fail closed");
    assert_eq!(reconciler.context_state(&run_id), "failed");
}

#[test]
fn direct_settlement_error_marks_context_failed_and_blocks_healthy_terminal() {
    // Settle a tool the context never registered: on_tool_settled errors;
    // the reconciler must mark the context Failed, and a later terminal must
    // NOT produce a healthy Match (distinct from the INSERT-failure test).
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // Settle an unknown toolCallId -> kernel settlement error.
    let settle = reconciler.settle_tool(&run_id, "never-registered", true, r#"{"ok":true}"#);
    assert!(settle.is_err(), "settling an unknown tool must error");
    assert_eq!(reconciler.context_state(&run_id), "failed");
    // A subsequent terminal cannot reconcile a healthy completed Match.
    let terminal = reconciler.terminal(&run_id, "completed", RunOutcome::Completed, false);
    assert!(
        terminal.is_err(),
        "a failed context must not produce a healthy terminal success"
    );
}

#[test]
fn shared_policy_classification_table() {
    use crate::kernel::PolicyDecision;
    use crate::runtime_host::shadow_reconcile::{kernel_tool_policy, legacy_tool_decision};
    // Execution-ownership classification (legacy decision fact).
    assert_eq!(
        legacy_tool_decision("read", "runtime", false).unwrap(),
        "allow"
    );
    assert_eq!(
        legacy_tool_decision("write_file", "host", false).unwrap(),
        "allow"
    );
    assert_eq!(
        legacy_tool_decision("write_file", "host", true).unwrap(),
        "approval"
    );
    // Host-canonical tool still at runtime location (never promoted) -> deny.
    assert_eq!(
        legacy_tool_decision("web_search", "runtime", false).unwrap(),
        "deny"
    );
    // Runtime inspection tool -> allow by contract regardless of promotion.
    assert_eq!(
        legacy_tool_decision("graph_readonly_run", "runtime", false).unwrap(),
        "allow"
    );
    assert!(legacy_tool_decision("not_a_real_tool", "host", false).is_err());

    // Kernel approval POLICY (a distinct fact from execution ownership):
    // runtime read tools allowed; host side-effecting tools require approval.
    assert!(matches!(kernel_tool_policy("read"), PolicyDecision::Allow));
    assert!(matches!(kernel_tool_policy("ls"), PolicyDecision::Allow));
    assert!(matches!(
        kernel_tool_policy("graph_readonly_run"),
        PolicyDecision::Allow
    ));
    assert!(matches!(
        kernel_tool_policy("write_file"),
        PolicyDecision::RequireApproval
    ));
    assert!(matches!(
        kernel_tool_policy("run_command"),
        PolicyDecision::RequireApproval
    ));
    assert!(matches!(
        kernel_tool_policy("call_mcp_tool"),
        PolicyDecision::RequireApproval
    ));
}

#[test]
fn kernel_policy_diff_for_web_search_is_observed_as_approval_mismatch() {
    // web_search: legacy promoted host tool = allow, but the kernel approval
    // policy requires approval (host execution vs runtime execution are
    // distinct facts). The shadow diff surfaces an approval divergence — it
    // must not assume the two are equivalent just because they share a
    // catalog entry.
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    // Preflight: kernel sees web_search (host, "always") -> RequireApproval;
    // legacy decision is allow -> divergence is observable via the diff.
    let outcome = reconciler
        .feed_preflight(&run_id, "web_search", "ws-1", json!({"query":"x"}), "allow")
        .expect("feed");
    assert!(outcome.persisted());
    let diff = outcome.diff.expect("diff");
    assert!(
        diff.is_divergence() || !diff.is_match(),
        "kernel approval policy diverges from legacy allow for web_search"
    );
}

#[test]
fn semantic_checkpoint_reopens_pending_approval_and_settles_without_execution() {
    let path =
        std::env::temp_dir().join(format!("fox-shadow-reopen-{}.sqlite", uuid::Uuid::new_v4()));
    let (run_id, conversation_id) = {
        let db = Database::open(path.clone()).unwrap();
        let (run_id, conversation_id) = boot_run(&db);
        let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
        let diff = persisted(reconciler.feed_preflight(
            &run_id,
            "write_file",
            "approved-1",
            json!({"path": "/approved.txt", "content": "x"}),
            "approval",
        ));
        assert_eq!(diff.kernel.run_state, "waiting_approval");
        (run_id, conversation_id)
    };
    let db = Database::open(path.clone()).unwrap();
    let reconciler = boot_reconciler(&db, &run_id, &conversation_id);
    reconciler
        .settle_tool(&run_id, "approved-1", true, "{}")
        .unwrap();
    // Duplicate delivery must not try to approve a settled tool a second time.
    reconciler
        .settle_tool(&run_id, "approved-1", true, "{}")
        .unwrap();
    let terminal =
        persisted(reconciler.terminal(&run_id, "completed", RunOutcome::Completed, false));
    assert!(terminal.is_match());
    assert_eq!(terminal.kernel.tools.len(), 1);
    assert_eq!(terminal.kernel.tools[0].decision, "approval");
    assert_eq!(
        db.kernel_executable_outbox_count(&format!("shadow-{run_id}"))
            .unwrap(),
        0
    );
    drop(reconciler);
    let again = ShadowReconciler::new(db.clone());
    assert!(again
        .bootstrap(
            &run_id,
            &conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy")
        )
        .unwrap_err()
        .contains("closed"));
    drop(again);
    drop(db);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cumulative_batches_remain_comparable_after_checkpoint_restart() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let first = boot_reconciler(&db, &run_id, &conversation_id);
    assert!(persisted(first.feed_preflight(
        &run_id,
        "read",
        "read-1",
        json!({"path":"/a"}),
        "allow"
    ))
    .is_match());
    first.settle_tool(&run_id, "read-1", true, "{}").unwrap();
    drop(first);
    let second = boot_reconciler(&db, &run_id, &conversation_id);
    let diff =
        persisted(second.feed_preflight(&run_id, "read", "read-2", json!({"path":"/b"}), "allow"));
    assert!(diff.is_match(), "{diff:?}");
    assert_eq!(diff.kernel.tools.len(), 2);
    second.settle_tool(&run_id, "read-2", true, "{}").unwrap();
    assert!(
        persisted(second.terminal(&run_id, "completed", RunOutcome::Completed, false)).is_match()
    );
}

#[test]
fn failed_diff_cannot_revive_after_observer_restart() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let first = boot_reconciler(&db, &run_id, &conversation_id);
    db.execute_raw_sql("CREATE TRIGGER fail_shadow_checkpoint_diff BEFORE INSERT ON kernel_shadow_diffs BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    let result = first
        .feed_preflight(&run_id, "read", "lost", json!({"path":"/a"}), "allow")
        .unwrap();
    assert!(!result.persisted());
    drop(first);
    db.execute_raw_sql("DROP TRIGGER fail_shadow_checkpoint_diff;")
        .unwrap();
    let second = ShadowReconciler::new(db.clone());
    assert!(second
        .bootstrap(
            &run_id,
            &conversation_id,
            "legacy",
            ShadowReconciler::shadow_config("legacy")
        )
        .unwrap_err()
        .contains("failed"));
    assert!(db
        .kernel_shadow_diffs_for(&format!("shadow-{run_id}"))
        .unwrap()
        .is_empty());
}

#[test]
fn repeated_live_bootstrap_validates_conversation_identity() {
    let db = test_db();
    let (run_id, conversation_id) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation_id);
    assert!(observer
        .bootstrap(
            &run_id,
            "different-conversation",
            "legacy",
            ShadowReconciler::shadow_config("legacy")
        )
        .is_err());
}

#[test]
fn real_pi_process_events_flow_through_host_database_and_shared_shadow_adapter() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for scenario in ["no_tool", "read_batch", "tool_error"] {
        let db = test_db();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, Some("ask"))
            .unwrap();
        let started = db
            .create_run(&conversation.id, "Pi shadow process acceptance", None)
            .unwrap();
        let run_id = &started.run.id;
        let output = std::process::Command::new("node")
            .arg(repository.join("services/agent-runtime/test/fixtures/shadow-process-harness.mjs"))
            .arg(&conversation.id)
            .arg(run_id)
            .arg(scenario)
            .current_dir(&repository)
            .output()
            .expect("Node is required for real Pi acceptance");
        assert!(
            output.status.success(),
            "Pi process harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let trace: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("Pi trace JSON");
        let observer = ShadowReconciler::new(db.clone());
        let mut began = 0;
        let mut settled = 0;
        for message in trace["messages"].as_array().unwrap() {
            if message["kind"] == "shadow_preflight" {
                let result = observer
                    .feed_preflight(
                        run_id,
                        message["tool"].as_str().unwrap(),
                        message["toolCallId"].as_str().unwrap(),
                        message["approved"]["input"].clone(),
                        "allow",
                    )
                    .unwrap();
                assert!(result.persisted());
            } else if message["type"] == "runtime_event" && message["runId"] == *run_id {
                let payload = &message["payload"];
                let seq = message["seq"]
                    .as_i64()
                    .expect("real Runtime event sequence");
                assert!(db.apply_runtime_event(run_id, seq, payload).unwrap());
                if payload["type"] == "run.request_snapshot" {
                    let mut config = ShadowReconciler::shadow_config("legacy");
                    config.prompt_config_hash = super::shadow_prompt_config_hash(payload).unwrap();
                    observer
                        .bootstrap(run_id, &conversation.id, "legacy", config)
                        .unwrap();
                }
                began += usize::from(payload["shadowModelRequest"] == "begin");
                settled += usize::from(payload["shadowModelRequest"] == "settle");
                observer.observe_applied_event(run_id, payload).unwrap();
            }
        }
        assert!(
            began > 0 && settled > 0,
            "actual model lifecycle must be observed"
        );
        let terminal = observer
            .last_terminal_diff(run_id)
            .expect("durable terminal");
        assert!(terminal.is_match(), "{scenario}: {terminal:?}");
        if scenario != "no_tool" {
            assert_eq!(terminal.kernel.tools.len(), 3);
            assert_eq!(
                terminal.kernel.tools[0].batch_id,
                terminal.kernel.tools[1].batch_id
            );
            assert_ne!(
                terminal.kernel.tools[0].batch_id,
                terminal.kernel.tools[2].batch_id
            );
            assert_eq!(terminal.kernel.tools[1].source_order, 1);
        }
        assert_eq!(
            db.kernel_executable_outbox_count(&format!("shadow-{run_id}"))
                .unwrap(),
            0
        );
        assert_eq!(db.kernel_executable_outbox_count(run_id).unwrap(), 0);
        let gate = db.kernel_shadow_exit_gate(&[run_id.clone()]).unwrap();
        assert_eq!(gate["passed"], true, "{scenario}: {gate}");
        assert_eq!(gate["authoritativeEnabled"], false);
        assert_eq!(gate["productionRolloutApproved"], false);
    }
}

#[test]
fn strict_shadow_gate_rejects_empty_incomplete_and_non_transport_samples() {
    let db = test_db();
    assert_eq!(db.kernel_shadow_exit_gate(&[]).unwrap()["passed"], false);
    let (run_id, conversation) = boot_run(&db);
    let missing = db.kernel_shadow_recent_exit_gate().unwrap();
    assert_eq!(
        missing["sampleCount"], 1,
        "unobserved Pi runs must not disappear from the cohort"
    );
    assert_eq!(missing["passed"], false);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    assert_eq!(
        db.kernel_shadow_exit_gate(&[run_id.clone()]).unwrap()["passed"],
        false
    );
    let _ = persisted(observer.terminal(&run_id, "completed", RunOutcome::Completed, false));
    // Synthetic local direct terminal lacks actual model-lifecycle events.
    assert_eq!(
        db.kernel_shadow_exit_gate(&[run_id]).unwrap()["passed"],
        false
    );
}

#[test]
fn gate_rejects_malformed_model_lifecycle_pairing() {
    // The model-request gate must validate the begin/settle PAIRING (no
    // duplicate begin, no settle without begin, no dangling begin), not just
    // that at least one begin and one settle occurred. Two begins + one settle
    // (the previously-accepted shape) must block the gate.
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);

    // Inject a malformed run.phase lifecycle: begin, begin, settle.
    db.apply_runtime_event(
        &run_id,
        10,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"begin"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"begin"}))
        .unwrap();
    db.apply_runtime_event(
        &run_id,
        11,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"begin"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"begin"}))
        .unwrap();
    db.apply_runtime_event(
        &run_id,
        12,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"settle"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"settle"}))
        .unwrap();

    // Close the run (zero tools, completes cleanly apart from the lifecycle).
    let _ = persisted(observer.terminal(&run_id, "completed", RunOutcome::Completed, false));

    let gate = db.kernel_shadow_exit_gate(&[run_id.clone()]).unwrap();
    assert_eq!(gate["passed"], false, "duplicate begin must block the gate: {gate}");
    let reasons = gate["blockingReasons"].as_array().cloned().unwrap_or_default();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.as_str().unwrap_or_default().contains("duplicate begin")),
        "expected a duplicate-begin blocking reason, got {reasons:?}"
    );
}

#[test]
fn gate_accepts_well_formed_begin_settle_lifecycle() {
    // A clean begin→settle (single pair) plus a closed terminal passes the
    // lifecycle pairing check (the rest of the gate may still block on other
    // facets, but the lifecycle reason must not appear).
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    db.apply_runtime_event(
        &run_id,
        10,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"begin"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"begin"}))
        .unwrap();
    db.apply_runtime_event(
        &run_id,
        11,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"settle"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"settle"}))
        .unwrap();
    let gate = db.kernel_shadow_exit_gate(&[run_id]).unwrap();
    let reasons = gate["blockingReasons"].as_array().cloned().unwrap_or_default();
    assert!(
        !reasons
            .iter()
            .any(|reason| reason.as_str().unwrap_or_default().contains("model request")),
        "a well-formed begin/settle pair must not raise a lifecycle reason: {reasons:?}"
    );
}

/// Lifecycle-sub-condition tests: these inject a specific shadowModelRequest
/// phase sequence into run_events (and the shadow observer) and assert the
/// gate raises the matching model-lifecycle blocking reason. They do NOT
/// assert the overall gate passes (the rest of the run may be incomplete);
/// they assert the lifecycle facet specifically, so a failure caused by a
/// different facet cannot make the test "pass by coincidence".
fn lifecycle_gate_reasons(phases: Vec<&str>) -> Vec<String> {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    for (idx, phase) in phases.iter().enumerate() {
        let seq = (10 + idx) as i64;
        // The authoritative event stream is run_events (via apply_runtime_event);
        // the gate reads lifecycle pairing from there. The shadow observer may
        // reject an unknown phase (that is the shadow's own failure surface),
        // so tolerate it here — the gate is the independent arbiter.
        db.apply_runtime_event(
            &run_id,
            seq,
            &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":phase}),
        )
        .unwrap();
        let _ = observer.observe_applied_event(
            &run_id,
            &json!({"type":"run.phase","shadowModelRequest":phase}),
        );
    }
    // Close the run so the gate reaches the lifecycle check.
    let _ = observer.terminal(&run_id, "completed", RunOutcome::Completed, false);
    let gate = db.kernel_shadow_exit_gate(&[run_id]).unwrap();
    gate["blockingReasons"]
        .as_array()
        .map(|array| {
            array
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn assert_lifecycle_reason(phases: Vec<&str>, expected_substring: &str) {
    let reasons = lifecycle_gate_reasons(phases);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains(expected_substring)),
        "expected a blocking reason containing {expected_substring:?}, got {reasons:?}"
    );
}

#[test]
fn lifecycle_settle_without_begin_blocks() {
    // settle with no open begin -> "settle without an open begin".
    assert_lifecycle_reason(vec!["settle"], "settle without an open begin");
}

#[test]
fn lifecycle_dangling_begin_after_paired_pair_blocks() {
    // A valid pair followed by an extra begin that never settles (dangling).
    assert_lifecycle_reason(
        vec!["begin", "settle", "begin"],
        "dangling model request (begin without settle)",
    );
}

#[test]
fn lifecycle_duplicate_settle_blocks() {
    // begin, settle, settle — the second settle has no open begin.
    assert_lifecycle_reason(vec!["begin", "settle", "settle"], "settle without an open begin");
}

#[test]
fn lifecycle_unknown_event_blocks() {
    // An unrecognised phase value must not be accepted as a lifecycle marker.
    assert_lifecycle_reason(vec!["begin", "settle", "frobnicate"], "unknown model request lifecycle");
}

#[test]
fn lifecycle_multiple_well_formed_pairs_raise_no_lifecycle_reason() {
    // Two complete, correctly-nested pairs (model streaming across two turns)
    // must not raise ANY model-lifecycle reason.
    let reasons = lifecycle_gate_reasons(vec!["begin", "settle", "begin", "settle"]);
    assert!(
        !reasons
            .iter()
            .any(|reason| reason.contains("model request")),
        "two well-formed begin/settle pairs must not raise a lifecycle reason: {reasons:?}"
    );
}

#[test]
fn lifecycle_clean_pair_raises_no_lifecycle_blocking_reason() {
    // LIFECYCLE SUB-CONDITION ONLY — this hand-built sample has a well-formed
    // begin/settle pair but intentionally does NOT construct a real Pi tool
    // batch or a terminal comparison, so it cannot assert the WHOLE gate passes
    // (the whole gate requires actual `pi-batch-*` membership, a closed
    // terminal comparison and a real model lifecycle). It asserts only that a
    // clean pair does not raise a model-lifecycle blocking reason.
    //
    // The authoritative "complete sample passes the whole gate" assertion is
    // `real_pi_process_events_flow_through_host_database_and_shared_shadow_adapter`
    // (a real pi-runtime subprocess feeding real JSONL through the formal
    // SQLite projection and shared adapter), which asserts `gate["passed"] ==
    // true`, `authoritativeEnabled == false` and
    // `productionRolloutApproved == false`.
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    db.apply_runtime_event(
        &run_id,
        10,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"begin"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"begin"}))
        .unwrap();
    db.apply_runtime_event(
        &run_id,
        11,
        &json!({"type":"run.phase","phase":"model_streaming","shadowModelRequest":"settle"}),
    )
    .unwrap();
    observer
        .observe_applied_event(&run_id, &json!({"type":"run.phase","shadowModelRequest":"settle"}))
        .unwrap();
    let gate = db.kernel_shadow_exit_gate(&[run_id]).unwrap();
    // Sub-condition: a clean pair must not be blocked on lifecycle grounds.
    // We deliberately do NOT assert gate["passed"] here (this synthetic run
    // lacks a real pi-batch/terminal); the whole-gate pass is covered by the
    // real Pi integration test referenced above.
    let reasons = gate["blockingReasons"].as_array().cloned().unwrap_or_default();
    assert!(
        !reasons
            .iter()
            .any(|reason| reason.as_str().unwrap_or_default().contains("model request")),
        "a clean begin/settle pair must not raise a lifecycle blocking reason: {reasons:?}"
    );
    let _ = conversation; // bound for readability
}


#[test]
fn permission_mode_is_frozen_and_granted_scopes_are_used_independently() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = ShadowReconciler::new(db.clone());
    let policy = json!({"mode":"allow","projectRoot":null,"grants":[]});
    observer
        .bootstrap_with_policy(
            &run_id,
            &conversation,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
            Some(policy.clone()),
        )
        .unwrap();
    let diff = persisted(observer.feed_preflight(
        &run_id,
        "write_file",
        "allowed-write",
        json!({"path":"/x","content":"x"}),
        "allow",
    ));
    assert!(diff.is_match());
    assert!(observer
        .bootstrap_with_policy(
            &run_id,
            &conversation,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
            Some(json!({"mode":"read_only","projectRoot":null,"grants":[]}))
        )
        .is_err());
    // A scoped allow is an independent frozen input, not copied from Legacy's
    // allow/approval result.
    let (run2, conversation2) = boot_run(&db);
    let input = json!({"query":"scope"});
    let scope = super::capability_permission_scope("web_search", &input).unwrap();
    observer
        .bootstrap_with_policy(
            &run2,
            &conversation2,
            "legacy",
            ShadowReconciler::shadow_config("legacy"),
            Some(json!({"mode":"ask","projectRoot":null,"grants":[["web_search",scope]]})),
        )
        .unwrap();
    let diff = persisted(observer.feed_preflight(&run2, "web_search", "scoped", input, "allow"));
    assert!(diff.is_match());
}

#[test]
fn startup_recovery_observes_legacy_interruption_without_reexecuting_tools() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    let _ = persisted(observer.feed_preflight(
        &run_id,
        "write_file",
        "pending",
        json!({"path":"/x"}),
        "approval",
    ));
    drop(observer);
    db.repair_interrupted_runs().unwrap();
    let recovered = ShadowReconciler::new(db.clone());
    assert_eq!(recovered.recover_all().unwrap(), 1);
    let terminal = recovered
        .last_terminal_diff(&run_id)
        .expect("recovered terminal");
    assert_eq!(terminal.kernel.tools.len(), 1);
    assert_eq!(terminal.kernel.run_state, "failed");
    assert_eq!(
        db.kernel_executable_outbox_count(&format!("shadow-{run_id}"))
            .unwrap(),
        0
    );
}

#[test]
fn real_batch_partial_observations_and_results_survive_restart_in_source_order() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    observer
        .observe_applied_event(
            &run_id,
            &json!({
                "type":"run.phase", "shadowToolBatch":{"batchId":"pi-batch-test-1","calls":[
                    {"toolCallId":"first","tool":"read","input":{"path":"/a"},"sourceOrder":0},
                    {"toolCallId":"second","tool":"read","input":{"path":"/b"},"sourceOrder":1}
                ]}
            }),
        )
        .unwrap();
    let pending = observer
        .feed_preflight(&run_id, "read", "second", json!({"path":"/b"}), "allow")
        .unwrap();
    assert!(pending.persisted() && pending.diff.is_none());
    observer.settle_tool(&run_id, "second", true, "{}").unwrap();
    drop(observer);
    let restored = boot_reconciler(&db, &run_id, &conversation);
    let batch =
        persisted(restored.feed_preflight(&run_id, "read", "first", json!({"path":"/a"}), "allow"));
    assert!(batch.is_match(), "{batch:?}");
    assert_eq!(
        batch
            .kernel
            .tools
            .iter()
            .map(|tool| tool.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    restored.settle_tool(&run_id, "first", true, "{}").unwrap();
    assert!(
        persisted(restored.terminal(&run_id, "completed", RunOutcome::Completed, false)).is_match()
    );
}

#[test]
fn atomic_checkpoint_failure_rolls_back_diff_and_blocks_the_gate() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    db.execute_raw_sql("CREATE TRIGGER fail_active_checkpoint BEFORE UPDATE ON kernel_shadow_checkpoints WHEN NEW.state='active' BEGIN SELECT RAISE(ABORT, 'checkpoint fault'); END;").unwrap();
    let outcome = observer
        .feed_preflight(&run_id, "read", "c", json!({"path":"/a"}), "allow")
        .unwrap();
    assert!(!outcome.persisted());
    assert!(db
        .kernel_shadow_diffs_for(&format!("shadow-{run_id}"))
        .unwrap()
        .is_empty());
    assert_eq!(observer.context_state(&run_id), "failed");
    assert_eq!(
        db.kernel_shadow_exit_gate(&[run_id]).unwrap()["passed"],
        false
    );
}

#[test]
fn orphaned_bootstrap_without_semantic_checkpoint_is_not_a_new_healthy_run() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let config = ShadowReconciler::shadow_config("legacy");
    db.kernel_create_shadow_run(
        &format!("shadow-{run_id}"),
        &run_id,
        &conversation,
        &format!("turn-{run_id}"),
        &config,
    )
    .unwrap();
    let observer = ShadowReconciler::new(db);
    assert!(observer
        .bootstrap(&run_id, &conversation, "legacy", config)
        .unwrap_err()
        .contains("semantic checkpoint"));
}

#[test]
fn conflicting_settled_result_fails_closed_instead_of_reclassifying_history() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    let _ =
        persisted(observer.feed_preflight(&run_id, "read", "once", json!({"path":"/a"}), "allow"));
    observer.settle_tool(&run_id, "once", true, "{}").unwrap();
    assert!(observer.settle_tool(&run_id, "once", false, "{}").is_err());
    assert_eq!(observer.context_state(&run_id), "failed");
}

#[test]
fn large_applied_result_uses_the_canonical_projection_without_a_false_replay_conflict() {
    let db = test_db();
    let (run_id, conversation) = boot_run(&db);
    let observer = boot_reconciler(&db, &run_id, &conversation);
    db.apply_runtime_event(
        &run_id,
        2,
        &json!({
            "type":"tool.started","toolCallId":"large","tool":"read","input":{"path":"/a"}
        }),
    )
    .unwrap();
    let event = json!({"type":"tool.completed","toolCallId":"large","result":{"content":"x".repeat(140_000)}});
    db.apply_runtime_event(&run_id, 3, &event).unwrap();
    let record = db.get_runtime_tool_call(&run_id, "large").unwrap().unwrap();
    assert_eq!(record.result.unwrap()["truncated"], true);
    observer.observe_applied_event(&run_id, &event).unwrap();
    assert!(
        persisted(observer.terminal(&run_id, "completed", RunOutcome::Completed, true)).is_match()
    );
}
