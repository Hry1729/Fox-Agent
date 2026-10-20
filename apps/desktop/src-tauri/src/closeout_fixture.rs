//! Package-2-D closeout: construct the **injected starting point** for the
//! isolated acceptance run.
//!
//! This is a development/test facility, gated behind the `kernel-acceptance`
//! feature so it is never part of a normal production build. It exists so the
//! real desktop can be driven through the real U1 acceptance surfaces without
//! waiting for a real model to produce a `RequiresU1` verdict.
//!
//! **Everything it builds goes through production APIs.** The Run is created with
//! `Database::kernel_create_run`, driven with `RunController::start` /
//! `record_initial_model_response` / `park_awaiting_delivery_review`, and
//! committed with `Database::kernel_commit_decision` — the same decision-commit
//! path the coordinator uses. The checklist is seeded with the production
//! `seed_delivery_checklist`, derived from a real task text by the production
//! `expectations_from_task`. No state is written with SQL, and no acceptance is
//! recorded here: the user's decisions come later, through the real commands,
//! from the real UI.
//!
//! The only things this fixture decides on the Host's behalf are *which* task
//! text to derive requirements from and the verdict envelope each row carries.
//! Both are part of the injected fixture and are labelled as such in the result
//! report.

use crate::database::{Database, DeliveryChecklistSeed};
use crate::kernel::{Clock, RunController, RunFrozenConfig};
use fox_engine_protocol::{
    ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
    TimeBudgets,
};
use rusqlite::OptionalExtension;

/// The task text the fixture derives its requirements from.
const FIXTURE_TASK: &str = "请生成一份 Excel 成果";

/// Build the parked U1 Run in `database_path` under `project_root`.
///
/// Returns the run id and the item keys the checklist carries, so the caller can
/// bind its evidence to them.
pub fn build_u1_parked_fixture(
    database_path: std::path::PathBuf,
    project_root: std::path::PathBuf,
) -> Result<(String, Vec<String>), String> {
    assert!(project_root.is_absolute(), "project root must be absolute");
    std::fs::create_dir_all(&project_root).map_err(|error| error.to_string())?;

    // A real candidate artifact, so the version ledger and the byte re-read in the
    // freshness rule both have something real to resolve.
    std::fs::write(project_root.join("report.csv"), b"segment,value\na,1\nb,2\n")
        .map_err(|error| error.to_string())?;

    let database = Database::open(database_path)?;

    // 1. A real conversation bound to the isolated project root.
    let conversation = database.create_conversation(
        database.default_agent_id(),
        Some("2-D 收口验收（隔离）"),
        Some(
            project_root
                .to_str()
                .ok_or_else(|| "project root is not UTF-8".to_owned())?,
        ),
        Some("allow"),
    )?;
    let conversation_id = conversation.id.clone();

    // 2. A real run, with a real frozen run-control binding.
    let started = database.create_run(&conversation_id, FIXTURE_TASK, None)?;
    let run_id = started.run.id.clone();
    let permission = FrozenPermission {
        mode: PermissionMode::Allow,
        project_root: Some(project_root.to_string_lossy().into_owned()),
        grants: vec![],
        approval_epoch: None,
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation_id.clone(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission)?,
        permission,
        budgets: TimeBudgets::default(),
    };
    database.freeze_run_control(&binding)?;

    // The run's `prompt_config_hash` must be the frozen model configuration's own
    // hash: the schema trigger refuses a model config that does not match the run
    // it is being frozen for. That hash is therefore derived here, before the run
    // row is written.
    let model = crate::kernel_model_config::KernelModelConfig {
        engine_id: "pi".into(),
        native_adapter: None,
        execution_profile_id: "legacy".into(),
        model_service: serde_json::json!({
            "apiType": "faux",
            "modelId": "closeout-acceptance",
            "baseUrl": "http://127.0.0.1",
            "fauxResponses": ["收到，正在整理交付清单。"],
        }),
        system_prompt: "Isolated acceptance fixture; no real model is contacted.".into(),
        proposal_tools: vec![],
    };
    let prompt_config_hash = model.hash()?;
    let config = RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "closeout-acceptance-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: prompt_config_hash.clone(),
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
    database.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        &prompt_config_hash,
        &crate::canonical_json::to_string(&config).map_err(|error| error.to_string())?,
    )?;
    // `start_prepared` needs a frozen model configuration as well as a frozen
    // initial input; both are what the coordinator requires to begin a round.
    database.freeze_kernel_model_config(&run_id, &model)?;

    // 3. The delivery checklist, derived from a real task text by the production
    //    helper — the same one the Host uses.
    let seeds: Vec<DeliveryChecklistSeed> =
        crate::runtime_host::delivery::expectations_from_task(FIXTURE_TASK);
    database.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())?;

    // 4. Drive the Run to the parked state through the real Controller and the
    //    real decision commit. No SQL writes `kernel_runs.state`.
    let clock = FixedClock;
    // The production sequence for one round, each step committed through the
    // matching production entry — the same three the coordinator itself calls:
    //   start_prepared → begin_initial_model_request → record_initial_model_response → park
    // `start_prepared` needs the frozen initial input, which is frozen here.
    let input = fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1,
        run_id: run_id.clone(),
        turn_id: "turn-1".into(),
        prompt_config_hash: prompt_config_hash.clone(),
        messages: vec![serde_json::json!({"role": "user", "content": "初始问题：交付验收"})],
    };
    database.freeze_kernel_initial_input(&input)?;
    let cancellation = crate::kernel::CancellationRegistry::default();
    drop(crate::runtime_host::kernel_coordinator::KernelCoordinator::start_prepared(
        &database, &clock, &run_id, &cancellation,
    )?);

    let mut controller =
        RunController::rehydrate(database.kernel_rehydrate(&run_id)?.ok_or_else(|| {
            "the run must be durable after start_prepared".to_owned()
        })?)
        .map_err(|error| error.to_string())?;
    // The response's checkpoint cursor is the last event seq BEFORE the dispatch,
    // so that the dispatch event it acknowledges is exactly `checkpoint_seq + 1`.
    // That is what the durable lease check compares against.
    let checkpoint_seq = controller.last_event_seq();
    let effects = controller
        .begin_initial_model_request(clock.now_monotonic_ms(), clock.now_wall_ms())
        .map_err(|error| error.to_string())?;
    database.kernel_commit_initial_model(
        &run_id,
        clock.now_wall_ms(),
        &controller.persist_command(&effects),
        "closeout-fixture",
        false,
        None,
        None,
    )?;

    let response = fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1,
        run_id: run_id.clone(),
        turn_id: controller.turn_id().to_owned(),
        checkpoint_seq,
        assistant_message: serde_json::json!({
            "role": "assistant",
            "stopReason": "stop",
            "content": [{"type": "text", "text": "收到，正在整理交付清单。"}],
        }),
    };
    let response_json = crate::canonical_json::to_string(&response).map_err(|error| error.to_string())?;
    let effects = controller
        .record_initial_model_response(&response_json)
        .map_err(|error| error.to_string())?;
    database.kernel_commit_initial_model(
        &run_id,
        clock.now_wall_ms(),
        &controller.persist_command(&[effects]),
        "closeout-fixture",
        true,
        None,
        None,
    )?;

    let items: Vec<String> = seeds.iter().map(|seed| seed.item_key.clone()).collect();
    let effects = controller
        .park_awaiting_delivery_review(
            clock.now_monotonic_ms(),
            clock.now_wall_ms(),
            "内容核验没有绑定真实产物，等待用户逐项决定",
            &items,
        )
        .map_err(|error| error.to_string())?;
    database.kernel_commit_decision(
        &run_id,
        clock.now_wall_ms(),
        &controller.persist_command(&effects),
    )?;
    // 5. Put a durable v2 verdict envelope on each row, with the `unconfirmed`
    //    class — the one class U1 accepts. This is the injected Host verdict.
    let candidate_sha = hex::encode(kernel_sha256(b"segment,value\na,1\nb,2\n"));
    for item in &items {
        let finding = format!(
            r#"{{"checkedHash":"{candidate_sha}","boundPath":"report.csv","deliveryVerdict":{{"verdictSchemaVersion":2,"verdictEnvelopeVersion":2,"itemKey":"{item}","class":"unconfirmed","aggregateClass":"unconfirmed","itemCount":1,"terminal":false,"facts":[{{"class":"unconfirmed","origin":"spec","detail":"内容核验没有绑定真实产物"}}]}}}}"#
        );
        database.with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET status='pending', target_path='report.csv', finding_json=?3 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![run_id, item, finding],
            )?;
            Ok(())
        })?;
    }

    Ok((run_id, items))
}

/// A fixed clock for the fixture.
///
/// Implemented here rather than taken from `kernel::TestClock`, because that type
/// is behind the `test-support` feature and this fixture must not change the
/// production dependency graph to build.
struct FixedClock;

impl crate::kernel::Clock for FixedClock {
    fn now_monotonic_ms(&self) -> i64 {
        1_700_000_000_000
    }
    fn now_wall_ms(&self) -> i64 {
        1_700_000_000_000
    }
}

/// Read-only diagnostic dump of the persisted context-budget reports.
///
/// `context_fits` stores one `ContextBudgetReport` per Run in
/// `kernel_context_budget_reports` before deciding whether a compaction is
/// needed, so reading it back establishes the **actual** trigger of
/// `kernel.context_compaction_no_candidates` from data rather than from the error
/// code alone. Also prints each Run's frozen model configuration, because the
/// budget is computed against that.
///
/// Read-only. Feature-gated, so a normal production build does not carry it.
pub fn dump_context_budget_reports(
    database_path: std::path::PathBuf,
    run_id_filter: Option<&str>,
) -> Result<Vec<String>, String> {
    let database = Database::open(database_path)?;
    let mut lines = Vec::new();
    let runs = database.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT id, status, error_code, error_message FROM runs ORDER BY created_at",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    for (id, status, error_code, error_message) in runs {
        if let Some(want) = run_id_filter {
            if id != want {
                continue;
            }
        }
        lines.push(format!(
            "RUN {id} status={status} error={error_code:?} message={error_message:?}"
        ));
        // The authoritative durable Kernel state for this Run, read straight from
        // the table the decision commit writes. This is what proves a parent is
        // still parked after its child ran.
        let kernel_state: Option<String> = database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT state FROM kernel_runs WHERE run_id=?1",
                    [id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        lines.push(match kernel_state {
            Some(state) => format!("KERNEL_STATE {id} {state}"),
            None => format!("KERNEL_STATE {id} <none>"),
        });
        let report: Option<String> = database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT report_json FROM kernel_context_budget_reports WHERE run_id=?1",
                    [id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        lines.push(match report {
            Some(body) => format!("BUDGET {id} {body}"),
            None => format!("BUDGET {id} <none>"),
        });
        let config: Option<String> = database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT config_json FROM kernel_model_configs WHERE run_id=?1",
                    [id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        lines.push(match config {
            Some(body) => format!("MODEL_CONFIG {id} {body}"),
            None => format!("MODEL_CONFIG {id} <none>"),
        });
        // The frozen requirement rows, so the exact persisted contract shape can
        // be read: a pre-R-2 v2 record has no `policy` key, a post-R-2 one does.
        let rows: Vec<(String, String)> = database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT item_key, requirements_json FROM delivery_checklist_items WHERE run_id=?1",
            )?;
            let rows = statement
                .query_map([id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        for (item_key, requirements) in rows {
            let policy = if requirements.contains("\"policy\"") {
                "present"
            } else {
                "absent"
            };
            lines.push(format!(
                "FROZEN_RECORD {id} {item_key} policy={policy} {requirements}"
            ));
        }
    }
    Ok(lines)
}

fn kernel_sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}


