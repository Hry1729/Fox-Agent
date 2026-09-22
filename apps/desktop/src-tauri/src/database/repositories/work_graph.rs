use super::{now_ms, Database};
use crate::database::{
    AcceptanceRecord, AppendContinuationDecisionInput, ContinuationDecisionKind,
    ContinuationDecisionRecord, ContinuationIngestDiagnosticRecord, ContinuationReasonCode,
    ContinuationRetryClass, ContinuationValidationOutcome, EvidenceReferenceKind, EvidenceType,
    EvidenceValidityStatus, GoalRecord, GoalStatus, PlanRevisionRecord,
    RecordContinuationIngestDiagnosticInput, ReviewFindingRecord, TaskAttemptFinishResult,
    TaskAttemptKind, TaskAttemptRecord, TaskAttemptStartResult, TaskAttemptStatus,
    TaskEvidenceRecord, TaskLedgerAcceptanceCheck, TaskLedgerCompletedOutcome,
    TaskLedgerCompletedSummary, TaskLedgerEventRange, TaskLedgerEvidenceProjection,
    TaskLedgerExecutionCursor, TaskLedgerGoalProjection, TaskLedgerPendingAction,
    TaskLedgerPendingActionKind, TaskLedgerPhase, TaskLedgerProjection, TaskLedgerProjectionMeta,
    TaskLedgerRunCursor, TaskLedgerSourceHighWatermark, TaskLedgerTaskProjection,
    TaskRepairOverrideEventRecord, TaskRepairOverrideStartResult, TaskValidationPolicyRecord,
    UnprojectedContinuationProposal, ValidationPolicySnapshot, WorkTaskRecord, WorkTaskStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
    thread,
    time::Duration,
};
use uuid::Uuid;

const MAX_BUSY_ATTEMPTS: usize = 5;
const INITIAL_BUSY_DELAY_MS: u64 = 10;
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;
const WORK_SNAPSHOT_TASK_LIMIT: usize = 64;
const WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT: usize = 4;
const WORK_SNAPSHOT_PLAN_LIMIT: usize = 8;
const WORK_SNAPSHOT_FINDING_LIMIT: usize = 32;
const WORK_SNAPSHOT_ACCEPTANCE_LIMIT: usize = 8;
const WORK_SNAPSHOT_LEDGER_ACTIVE_LIMIT: usize = 64;
const WORK_SNAPSHOT_LEDGER_PENDING_LIMIT: usize = 64;
const WORK_SNAPSHOT_LEDGER_COMPLETED_LIMIT: usize = 32;
const WORK_SNAPSHOT_DERIVED_GOAL_LIMIT: usize = 32;
const WORK_SNAPSHOT_DERIVED_TASK_LIMIT: usize = 128;
const WORK_SNAPSHOT_RUN_LIMIT: usize = 32;
const WORK_SNAPSHOT_ATTEMPTS_PER_TASK_LIMIT: usize = 4;
const LEGACY_VALIDATION_POLICY_ID: &str = "legacy_v1";
const LEGACY_POLICY_HASH: &str = "4129f5db32d88d7070c59ed2351aab9c6ad59c8cc380451901bb10962c5bee92";
const STANDARD_POLICY_HASH: &str =
    "bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d";
const HIGH_RISK_POLICY_HASH: &str =
    "bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea";

pub fn validation_policy_snapshot(id: &str) -> Result<ValidationPolicySnapshot, RepositoryError> {
    let snapshot = match id {
        "legacy_v1" => ValidationPolicySnapshot {
            schema_version: 1,
            id: "legacy_v1".to_owned(),
            risk_level: "legacy".to_owned(),
            required_checks: Vec::new(),
            allowed_check_types: vec![
                "test".to_owned(),
                "inspection".to_owned(),
                "review".to_owned(),
                "manual".to_owned(),
                "other".to_owned(),
            ],
            reviewer_policy: "legacy".to_owned(),
            max_repair_attempts: 0,
            completion_requires_acceptance: false,
            hash: LEGACY_POLICY_HASH.to_owned(),
        },
        "standard_v1" => ValidationPolicySnapshot {
            schema_version: 1,
            id: "standard_v1".to_owned(),
            risk_level: "standard".to_owned(),
            required_checks: vec!["inspection".to_owned()],
            allowed_check_types: vec![
                "test".to_owned(),
                "inspection".to_owned(),
                "review".to_owned(),
                "manual".to_owned(),
            ],
            reviewer_policy: "host_validated".to_owned(),
            max_repair_attempts: 2,
            completion_requires_acceptance: true,
            hash: STANDARD_POLICY_HASH.to_owned(),
        },
        "high_risk_v1" => ValidationPolicySnapshot {
            schema_version: 1,
            id: "high_risk_v1".to_owned(),
            risk_level: "high".to_owned(),
            required_checks: vec!["inspection".to_owned(), "review".to_owned()],
            allowed_check_types: vec![
                "test".to_owned(),
                "inspection".to_owned(),
                "review".to_owned(),
                "manual".to_owned(),
            ],
            reviewer_policy: "independent".to_owned(),
            max_repair_attempts: 1,
            completion_requires_acceptance: true,
            hash: HIGH_RISK_POLICY_HASH.to_owned(),
        },
        other => {
            return Err(RepositoryError::InvalidValidationPolicy(format!(
                "unsupported ValidationPolicy id '{other}'"
            )))
        }
    };
    Ok(snapshot)
}

fn canonical_run_execution_profile(profile_id: &str) -> Result<(String, String), RepositoryError> {
    if ![
        "legacy",
        "durable_v2_shadow",
        "durable_v2",
        "graph_readonly_preview",
        "graph_reviewer_v1",
    ]
    .contains(&profile_id)
    {
        return Err(RepositoryError::InvalidValidationPolicy(format!(
            "unsupported Host execution profile '{profile_id}'"
        )));
    }
    let snapshot = serde_json::json!({ "schemaVersion": 1, "id": profile_id });
    let encoded = serde_json::to_string(&snapshot)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let hash = format!("{:x}", Sha256::digest(encoded.as_bytes()));
    Ok((encoded, hash))
}

pub(super) fn freeze_run_execution_profile_in_transaction(
    connection: &Connection,
    run_id: &str,
    profile_id: &str,
    frozen_at: &str,
) -> Result<(), RepositoryError> {
    validate_bounded_text("run id", run_id, 200)?;
    let (snapshot_json, profile_hash) = canonical_run_execution_profile(profile_id)?;
    require_exists(connection, "runs", run_id)?;
    let existing = connection
        .query_row(
            "SELECT schema_version, profile_id, snapshot_json, profile_hash
             FROM run_execution_profiles WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(RepositoryError::database)?;
    if let Some(existing) = existing {
        if existing
            == (
                1,
                profile_id.to_owned(),
                snapshot_json.clone(),
                profile_hash.clone(),
            )
        {
            return Ok(());
        }
        return Err(RepositoryError::InvalidValidationPolicy(format!(
            "Run '{run_id}' execution profile conflicts with its Host-frozen snapshot"
        )));
    }
    connection
        .execute(
            "INSERT INTO run_execution_profiles(
                run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5)",
            params![run_id, profile_id, snapshot_json, profile_hash, frozen_at],
        )
        .map_err(RepositoryError::database)?;
    Ok(())
}

impl Database {
    pub fn task_has_attempt_run(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<bool, RepositoryError> {
        validate_bounded_text("task id", task_id, 200)?;
        validate_bounded_text("run id", run_id, 200)?;
        with_read_connection(self, |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM task_attempts WHERE task_id = ?1 AND run_id = ?2
                     )",
                    params![task_id, run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn accept_goal_with_review(
        &self,
        conversation_id: &str,
        goal_id: &str,
        plan_revision_id: &str,
        expected_goal_version: i64,
        summary: &str,
        checks: &Value,
        reviewer_run_id: &str,
    ) -> Result<(AcceptanceRecord, GoalRecord), RepositoryError> {
        validate_bounded_text("conversation id", conversation_id, 200)?;
        validate_bounded_text("goal id", goal_id, 200)?;
        validate_bounded_text("plan revision id", plan_revision_id, 200)?;
        validate_bounded_text("acceptance summary", summary, 8_000)?;
        validate_bounded_text("reviewer run id", reviewer_run_id, 200)?;
        if expected_goal_version < 1 {
            return Err(RepositoryError::InvalidInput(
                "expected Goal version must be at least 1".to_owned(),
            ));
        }
        let acceptance_id = Uuid::new_v4().to_string();
        let now = timestamp();
        with_write_transaction(self, |transaction| {
            let goal =
                load_goal(transaction, goal_id)?.ok_or_else(|| not_found("goal", goal_id))?;
            if goal.conversation_id != conversation_id {
                return Err(RepositoryError::CrossConversationReference);
            }
            let graph_goal = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM work_graph_specs WHERE goal_id = ?1)",
                    [goal_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if graph_goal {
                return Err(RepositoryError::ConstraintViolation(
                    "[graph.readonly_dedicated_accept_required] activated Graph Goals require graph_readonly_accept with immutable Graph provenance"
                        .to_owned(),
                ));
            }

            let existing = load_accepted_acceptance(transaction, goal_id)?;
            if let Some(existing) = existing.as_ref() {
                let same_content = existing.plan_revision_id.as_deref() == Some(plan_revision_id)
                    && existing.summary == summary
                    && existing.checks == *checks
                    && existing.reviewer == reviewer_run_id;
                if !same_content {
                    return Err(RepositoryError::ConstraintViolation(
                        "Goal already has an accepted Acceptance with conflicting content"
                            .to_owned(),
                    ));
                }
                if goal.status == GoalStatus::Completed {
                    return Ok((existing.clone(), goal));
                }
            }
            if goal.status != GoalStatus::Active {
                return Err(RepositoryError::InvalidTransition {
                    entity: "goal acceptance",
                    from: goal_status_text(&goal.status).to_owned(),
                    to: "completed".to_owned(),
                });
            }
            if goal.version != expected_goal_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: goal_id.to_owned(),
                    expected_version: expected_goal_version,
                });
            }

            let approved_revision = transaction
                .query_row(
                    "SELECT revision FROM plan_revisions
                     WHERE id = ?1 AND goal_id = ?2 AND conversation_id = ?3
                       AND status = 'approved'",
                    params![plan_revision_id, goal_id, conversation_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| {
                    RepositoryError::ConstraintViolation(
                        "final acceptance requires the approved PlanRevision".to_owned(),
                    )
                })?;
            let newer_plan_pending = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM plan_revisions
                        WHERE goal_id = ?1 AND status = 'proposed' AND revision > ?2
                     )",
                    params![goal_id, approved_revision],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if newer_plan_pending {
                return Err(RepositoryError::ConstraintViolation(
                    "a newer PlanRevision is still awaiting approval".to_owned(),
                ));
            }
            let reviewer_run_matches = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM runs WHERE id = ?1 AND conversation_id = ?2
                     )",
                    params![reviewer_run_id, conversation_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if !reviewer_run_matches {
                return Err(RepositoryError::CrossConversationReference);
            }
            let reviewer_finding_exists = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM review_findings
                        WHERE goal_id = ?1 AND plan_revision_id = ?2 AND created_by = ?3
                     )",
                    params![goal_id, plan_revision_id, reviewer_run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if !reviewer_finding_exists {
                return Err(RepositoryError::ConstraintViolation(
                    "Acceptance reviewer must be the Run that reviewed the approved PlanRevision"
                        .to_owned(),
                ));
            }
            let blockers = transaction
                .query_row(
                    "SELECT COUNT(*) FROM review_findings
                     WHERE goal_id = ?1 AND status = 'open'
                       AND severity IN ('critical', 'high', 'medium')",
                    [goal_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(RepositoryError::database)?;
            if blockers > 0 {
                return Err(RepositoryError::ReviewBlocked {
                    task_id: format!("goal:{goal_id}"),
                });
            }

            let mut task_ids = Vec::new();
            let mut completed_task_ids = Vec::new();
            let mut statement = transaction
                .prepare("SELECT id FROM work_tasks WHERE goal_id = ?1 ORDER BY ordinal, id")
                .map_err(RepositoryError::database)?;
            let ids = statement
                .query_map([goal_id], |row| row.get::<_, String>(0))
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)?;
            drop(statement);
            if ids.is_empty() {
                return Err(RepositoryError::ConstraintViolation(
                    "a goal cannot be accepted before at least one Task is recorded".to_owned(),
                ));
            }
            for task_id in ids {
                let task = load_task(transaction, &task_id)?
                    .ok_or_else(|| not_found("work task", &task_id))?;
                let policy = load_task_validation_policy(transaction, &task_id)?;
                match task.status {
                    WorkTaskStatus::Completed => {
                        let valid_evidence = transaction
                            .query_row(
                                "SELECT COUNT(*) FROM task_evidence
                                 WHERE task_id = ?1 AND validity_status = 'valid'",
                                [task_id.as_str()],
                                |row| row.get::<_, i64>(0),
                            )
                            .map_err(RepositoryError::database)?;
                        if valid_evidence == 0 {
                            return Err(RepositoryError::EvidenceRequired {
                                task_id: task_id.clone(),
                            });
                        }
                        if policy.snapshot.id != LEGACY_VALIDATION_POLICY_ID {
                            let attempt_id = transaction
                                .query_row(
                                    "SELECT id FROM task_attempts
                                     WHERE task_id = ?1 AND status = 'succeeded'
                                     ORDER BY attempt_number DESC LIMIT 1",
                                    [task_id.as_str()],
                                    |row| row.get::<_, String>(0),
                                )
                                .optional()
                                .map_err(RepositoryError::database)?
                                .ok_or_else(|| {
                                    RepositoryError::ConstraintViolation(format!(
                                        "durable Task '{task_id}' has no succeeded Attempt"
                                    ))
                                })?;
                            let attempt = load_task_attempt(transaction, &attempt_id)?
                                .ok_or_else(|| not_found("task attempt", &attempt_id))?;
                            validate_task_completion_policy(
                                transaction,
                                &task,
                                &attempt,
                                &policy.snapshot,
                            )?;
                        }
                        completed_task_ids.push(task_id.clone());
                    }
                    WorkTaskStatus::Skipped
                        if policy.snapshot.id == LEGACY_VALIDATION_POLICY_ID => {}
                    WorkTaskStatus::Skipped => {
                        return Err(RepositoryError::ConstraintViolation(format!(
                            "durable Task '{task_id}' cannot be accepted as skipped without explicit Host/user authorization"
                        )));
                    }
                    _ => {
                        return Err(RepositoryError::ConstraintViolation(format!(
                            "Task '{task_id}' is not complete"
                        )));
                    }
                }
                task_ids.push(task_id);
            }

            let reviewer_implemented = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM task_attempts attempt
                        JOIN work_tasks task ON task.id = attempt.task_id
                        WHERE task.goal_id = ?1 AND attempt.run_id = ?2
                     )",
                    params![goal_id, reviewer_run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if reviewer_implemented {
                return Err(RepositoryError::ConstraintViolation(
                    "Acceptance reviewer Run must be independent from implementation Evidence"
                        .to_owned(),
                ));
            }
            validate_atomic_acceptance_checks(
                transaction,
                goal_id,
                reviewer_run_id,
                checks,
                &task_ids,
                &completed_task_ids,
            )?;

            let acceptance = if let Some(existing) = existing {
                existing
            } else {
                transaction
                    .execute(
                        "INSERT INTO acceptances(
                            id, goal_id, plan_revision_id, conversation_id, status, summary,
                            checks_json, reviewer, created_at, resolved_at
                         ) VALUES (?1, ?2, ?3, ?4, 'accepted', ?5, ?6, ?7, ?8, ?8)",
                        params![
                            acceptance_id,
                            goal_id,
                            plan_revision_id,
                            conversation_id,
                            summary,
                            checks.to_string(),
                            reviewer_run_id,
                            now,
                        ],
                    )
                    .map_err(RepositoryError::database)?;
                load_acceptance(transaction, &acceptance_id)?
                    .ok_or_else(|| not_found("acceptance", &acceptance_id))?
            };
            let changed = transaction
                .execute(
                    "UPDATE goals
                     SET status = 'completed', version = version + 1, updated_at = ?2,
                         completed_at = ?2, blocked_reason = NULL
                     WHERE id = ?1 AND status = 'active' AND version = ?3",
                    params![goal_id, now, expected_goal_version],
                )
                .map_err(RepositoryError::database)?;
            if changed != 1 {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: goal_id.to_owned(),
                    expected_version: expected_goal_version,
                });
            }
            let completed_goal =
                load_goal(transaction, goal_id)?.ok_or_else(|| not_found("goal", goal_id))?;
            Ok((acceptance, completed_goal))
        })
    }

    pub fn freeze_run_execution_profile(
        &self,
        run_id: &str,
        profile_id: &str,
    ) -> Result<(), RepositoryError> {
        validate_bounded_text("run id", run_id, 200)?;
        let now = timestamp();
        with_write_transaction(self, |transaction| {
            freeze_run_execution_profile_in_transaction(transaction, run_id, profile_id, &now)
        })
    }

    pub fn frozen_run_execution_profile_id(
        &self,
        run_id: &str,
    ) -> Result<Option<String>, RepositoryError> {
        validate_bounded_text("run id", run_id, 200)?;
        with_read_connection(self, |connection| {
            let exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
                    [run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if !exists {
                return Err(not_found("run", run_id));
            }
            let frozen = connection
                .query_row(
                    "SELECT schema_version, profile_id, snapshot_json, profile_hash
                     FROM run_execution_profiles WHERE run_id = ?1",
                    [run_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let Some((schema_version, profile_id, snapshot_json, profile_hash)) = frozen else {
                return Ok(None);
            };
            let (canonical_json, canonical_hash) = canonical_run_execution_profile(&profile_id)?;
            if schema_version != 1
                || snapshot_json != canonical_json
                || profile_hash != canonical_hash
            {
                return Err(RepositoryError::InvalidValidationPolicy(format!(
                    "Run '{run_id}' has an invalid Host-frozen execution profile"
                )));
            }
            Ok(Some(profile_id))
        })
    }

    pub fn validation_policy_id_for_run(
        &self,
        run_id: &str,
        risk_hint: Option<&str>,
    ) -> Result<&'static str, RepositoryError> {
        // Pre-v34 Runs have no Host-frozen profile and retain Legacy behavior.
        // Runtime event/request payloads are deliberately not consulted here.
        let profile = self
            .frozen_run_execution_profile_id(run_id)?
            .unwrap_or_else(|| "legacy".to_owned());
        match profile.as_str() {
            "legacy" => Ok("legacy_v1"),
            "durable_v2" => match risk_hint.map(str::trim) {
                Some("high" | "critical") => Ok("high_risk_v1"),
                None | Some("" | "low" | "standard") => Ok("standard_v1"),
                Some(other) => Err(RepositoryError::InvalidInput(format!(
                    "unsupported task risk level '{other}'"
                ))),
            },
            "durable_v2_shadow" | "graph_readonly_preview" | "graph_reviewer_v1" => {
                Err(RepositoryError::ConstraintViolation(format!(
                    "execution profile '{profile}' cannot create mutable Task facts"
                )))
            }
            other => Err(RepositoryError::InvalidValidationPolicy(format!(
                "unsupported authoritative execution profile '{other}'"
            ))),
        }
    }

    pub fn task_validation_policy(
        &self,
        task_id: &str,
    ) -> Result<TaskValidationPolicyRecord, RepositoryError> {
        validate_bounded_text("task id", task_id, 200)?;
        with_read_connection(self, |connection| {
            load_task_validation_policy(connection, task_id)
        })
    }

    pub fn list_task_attempts(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskAttemptRecord>, RepositoryError> {
        validate_bounded_text("task id", task_id, 200)?;
        if !(1..=100).contains(&limit) {
            return Err(RepositoryError::InvalidInput(
                "task attempt limit must be between 1 and 100".to_owned(),
            ));
        }
        with_read_connection(self, |connection| {
            require_exists(connection, "work_tasks", task_id)?;
            connection
                .prepare(
                    "SELECT id, task_id, attempt_number, kind, status, run_id, policy_hash,
                            root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                            version, evidence_rowid_watermark, finding_rowid_watermark,
                            started_at, finished_at
                     FROM task_attempts WHERE task_id = ?1
                     ORDER BY attempt_number DESC, id DESC LIMIT ?2",
                )
                .map_err(RepositoryError::database)?
                .query_map(params![task_id, limit as i64], task_attempt_from_row)
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)
        })
    }

    pub fn start_task_attempt(
        &self,
        input: StartTaskAttemptInput,
    ) -> Result<TaskAttemptStartResult, RepositoryError> {
        validate_start_task_attempt(&input)?;
        let now = timestamp();
        with_write_transaction(self, |transaction| {
            let graph_task = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM work_graph_nodes WHERE task_id = ?1)",
                    [input.task_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if graph_task {
                return Err(RepositoryError::ConstraintViolation(
                    "[graph.readonly_dedicated_start_required] Graph node execution requires graph_readonly_node_start so Attempt ownership and Child delegation are atomic"
                        .to_owned(),
                ));
            }
            start_task_attempt_in_transaction(transaction, &input, &now)
        })
    }
}

pub(super) fn start_task_attempt_in_transaction(
    transaction: &Transaction<'_>,
    input: &StartTaskAttemptInput,
    now: &str,
) -> Result<TaskAttemptStartResult, RepositoryError> {
    if let Some(existing) = load_task_attempt(transaction, &input.id)? {
        if existing.task_id == input.task_id
            && existing.run_id == input.run_id
            && existing.kind == input.kind
            && existing.root_cause == input.root_cause
            && existing.finding_ids == input.finding_ids
        {
            let task = load_task(transaction, &input.task_id)?
                .ok_or_else(|| not_found("work task", &input.task_id))?;
            return Ok(TaskAttemptStartResult {
                task,
                attempt: Some(existing),
                budget_exhausted: false,
            });
        }
        return Err(RepositoryError::ConstraintViolation(format!(
            "task attempt '{}' conflicts with existing content",
            input.id
        )));
    }

    let current = load_task(transaction, &input.task_id)?
        .ok_or_else(|| not_found("work task", &input.task_id))?;
    if current.version != input.expected_task_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.task_id.clone(),
            expected_version: input.expected_task_version,
        });
    }
    ensure_run_matches_task_conversation(transaction, &input.run_id, &input.task_id)?;
    let policy = load_task_validation_policy(transaction, &input.task_id)?;
    let running = load_running_task_attempt(transaction, &input.task_id)?;
    if running.is_some() {
        return Err(RepositoryError::ConstraintViolation(
            "a task can have only one running Attempt".to_owned(),
        ));
    }

    if input.kind == TaskAttemptKind::Execution {
        let execution_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM task_attempts
                         WHERE task_id = ?1 AND kind = 'execution'",
                [input.task_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(RepositoryError::database)?;
        if policy.snapshot.id != LEGACY_VALIDATION_POLICY_ID && execution_count > 0 {
            return Err(RepositoryError::ConstraintViolation(
                        "A durable Task permits exactly one Execution Attempt; subsequent work must use a bounded Repair Attempt with root-cause and Finding references"
                            .to_owned(),
                    ));
        }
        let repair_required = transaction
            .query_row(
                "SELECT EXISTS(
                            SELECT 1 FROM task_attempts
                            WHERE task_id = ?1 AND kind = 'repair'
                         ) OR EXISTS(
                            SELECT 1 FROM review_findings
                            WHERE goal_id = ?2 AND (task_id = ?1 OR task_id IS NULL)
                              AND status = 'open'
                              AND severity IN ('critical', 'high', 'medium')
                         )",
                params![input.task_id, current.goal_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(RepositoryError::database)?
            || current
                .blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("repair_budget_exhausted:"));
        if repair_required {
            return Err(RepositoryError::ConstraintViolation(
                        "Task requires a bounded Repair Attempt; Execution cannot bypass repair history or blocking Findings"
                            .to_owned(),
                    ));
        }
    }

    if input.kind == TaskAttemptKind::Repair {
        validate_repair_findings(transaction, &current, &input.finding_ids)?;
        let repair_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM task_attempts
                         WHERE task_id = ?1 AND kind = 'repair'",
                [input.task_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(RepositoryError::database)?;
        if repair_count >= i64::from(policy.snapshot.max_repair_attempts) {
            let reason = format!(
                "repair_budget_exhausted: policy={} repairs={}/{}; human escalation required",
                policy.snapshot.id, repair_count, policy.snapshot.max_repair_attempts
            );
            transaction
                .execute(
                    "UPDATE work_tasks
                             SET status = 'blocked', blocked_reason = ?2, owner_run_id = NULL,
                                 version = version + 1, updated_at = ?3, finished_at = ?3
                             WHERE id = ?1 AND version = ?4",
                    params![input.task_id, reason, now, input.expected_task_version],
                )
                .map_err(RepositoryError::database)?;
            let task = load_task(transaction, &input.task_id)?
                .ok_or_else(|| not_found("work task", &input.task_id))?;
            return Ok(TaskAttemptStartResult {
                task,
                attempt: None,
                budget_exhausted: true,
            });
        }
    }

    let allowed = match input.kind {
        TaskAttemptKind::Execution => matches!(
            current.status,
            WorkTaskStatus::Queued | WorkTaskStatus::Blocked | WorkTaskStatus::Interrupted
        ),
        TaskAttemptKind::Repair => matches!(
            current.status,
            WorkTaskStatus::Completed | WorkTaskStatus::Blocked | WorkTaskStatus::Interrupted
        ),
    };
    if !allowed {
        return Err(RepositoryError::InvalidTransition {
            entity: "work task Attempt",
            from: task_status_text(&current.status).to_owned(),
            to: "in_progress".to_owned(),
        });
    }

    let attempt_number = transaction
        .query_row(
            "SELECT COALESCE(MAX(attempt_number), 0) + 1
                     FROM task_attempts WHERE task_id = ?1",
            [input.task_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    let evidence_rowid_watermark = transaction
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM task_evidence WHERE task_id = ?1",
            [input.task_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    let finding_rowid_watermark = transaction
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM review_findings
                     WHERE goal_id = ?1 AND task_id = ?2",
            params![current.goal_id, input.task_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    let finding_ids_json = serde_json::to_string(&input.finding_ids)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO task_attempts(
                        id, task_id, attempt_number, kind, status, run_id, policy_hash,
                        root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                        version, evidence_rowid_watermark, finding_rowid_watermark,
                        started_at, finished_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7, ?8, '[]', NULL, 1,
                        ?9, ?10, ?11, NULL
                     )",
            params![
                input.id,
                input.task_id,
                attempt_number,
                task_attempt_kind_text(&input.kind),
                input.run_id,
                policy.snapshot.hash,
                input.root_cause,
                finding_ids_json,
                evidence_rowid_watermark,
                finding_rowid_watermark,
                now,
            ],
        )
        .map_err(RepositoryError::database)?;
    let changed = transaction
        .execute(
            "UPDATE work_tasks
                     SET status = 'in_progress', owner_run_id = ?2, attempt = ?3,
                         version = version + 1, blocked_reason = NULL,
                         started_at = COALESCE(started_at, ?4), finished_at = NULL, updated_at = ?4
                     WHERE id = ?1 AND version = ?5",
            params![
                input.task_id,
                input.run_id,
                attempt_number,
                now,
                input.expected_task_version
            ],
        )
        .map_err(RepositoryError::database)?;
    if changed != 1 {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.task_id.clone(),
            expected_version: input.expected_task_version,
        });
    }
    Ok(TaskAttemptStartResult {
        task: load_task(transaction, &input.task_id)?
            .ok_or_else(|| not_found("work task", &input.task_id))?,
        attempt: Some(
            load_task_attempt(transaction, &input.id)?
                .ok_or_else(|| not_found("task attempt", &input.id))?,
        ),
        budget_exhausted: false,
    })
}

impl Database {
    pub fn start_task_repair_override(
        &self,
        input: StartTaskRepairOverrideInput,
    ) -> Result<TaskRepairOverrideStartResult, RepositoryError> {
        enum TransactionOutcome {
            Started(TaskRepairOverrideStartResult),
            ManagedBudgetRejected(String),
        }

        validate_start_task_repair_override(&input)?;
        let canonical_input = task_repair_override_input_value(&input);
        let encoded_input = serde_json::to_string(&canonical_input)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let input_hash = format!("{:x}", Sha256::digest(encoded_input.as_bytes()));
        let now = timestamp();
        let outcome = with_write_transaction(self, |transaction| {
            if let Some(existing) = load_task_repair_override_collision(
                transaction,
                &input.task_id,
                &input.attempt_id,
                &input.approval_id,
                &input.tool_call_id,
            )? {
                let attempt = load_task_attempt(transaction, &existing.attempt_id)?
                    .ok_or_else(|| not_found("task attempt", &existing.attempt_id))?;
                let policy = load_task_validation_policy(transaction, &input.task_id)?;
                if existing.task_id == input.task_id
                    && existing.attempt_id == input.attempt_id
                    && existing.approval_id == input.approval_id
                    && existing.tool_call_id == input.tool_call_id
                    && existing.run_id == input.run_id
                    && existing.conversation_id == input.conversation_id
                    && existing.policy_id == policy.snapshot.id
                    && existing.policy_hash == policy.snapshot.hash
                    && existing.input_hash == input_hash
                    && existing.escalation_reason == input.escalation_reason
                    && attempt.task_id == input.task_id
                    && attempt.run_id == input.run_id
                    && attempt.kind == TaskAttemptKind::Repair
                    && attempt.root_cause.as_deref() == Some(input.root_cause.as_str())
                    && attempt.finding_ids == input.finding_ids
                {
                    let task = load_task(transaction, &input.task_id)?
                        .ok_or_else(|| not_found("work task", &input.task_id))?;
                    return Ok(TransactionOutcome::Started(TaskRepairOverrideStartResult {
                        task,
                        attempt,
                        override_event: existing,
                    }));
                }
                return Err(RepositoryError::ConstraintViolation(
                    "task repair override conflicts with an existing immutable authorization event"
                        .to_owned(),
                ));
            }

            let eligibility = validate_task_repair_override_eligibility(
                transaction,
                RepairOverrideEligibilityInput {
                    task_id: &input.task_id,
                    attempt_id: &input.attempt_id,
                    run_id: &input.run_id,
                    conversation_id: &input.conversation_id,
                    expected_task_version: input.expected_task_version,
                    finding_ids: &input.finding_ids,
                },
            )?;
            let current = eligibility.task;
            let policy = eligibility.policy;
            let normal_repair_used = eligibility.normal_repair_used;
            let normal_repair_budget = eligibility.normal_repair_budget;
            let kernel_owned: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
                [&input.run_id],|row|row.get(0)).map_err(RepositoryError::database)?;
            if kernel_owned {
                // The desktop approval row is only a projection. Require the
                // exact Kernel decision and a currently leased dispatch too.
                let authorized: bool = transaction.query_row("SELECT EXISTS(
                    SELECT 1 FROM kernel_tool_calls t JOIN kernel_approvals a
                      ON a.run_id=t.run_id AND a.tool_call_id=t.tool_call_id
                    JOIN kernel_effect_outbox o ON o.run_id=t.run_id AND o.tool_call_id=t.tool_call_id
                    JOIN kernel_runs k ON k.run_id=t.run_id
                    WHERE t.run_id=?1 AND 'kernel-tool:'||t.run_id||':'||t.tool_call_id=?2
                      AND 'kernel-approval:'||t.run_id||':'||t.tool_call_id||':v'||a.policy_version=?3
                      AND a.policy_version=(SELECT p.version FROM kernel_execution_policies p
                        JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=t.run_id)
                      AND t.tool='task_repair_escalate_start' AND t.state='running'
                      AND a.state='allow_once' AND o.effect_type='dispatch_tool' AND o.status='leased'
                      AND k.state='running' AND NOT EXISTS(SELECT 1 FROM kernel_host_commands c
                        WHERE c.run_id=t.run_id AND c.kind='cancel'))",
                    params![input.run_id,input.tool_call_id,input.approval_id],|row|row.get(0))
                    .map_err(RepositoryError::database)?;
                if !authorized { return Err(RepositoryError::ConstraintViolation(
                    "Kernel repair override requires its one-time decision and active dispatch lease".into())); }
            }

            let approval = transaction
                .query_row(
                    "SELECT a.status, a.category, a.decision_json, a.claimed_at,
                            a.claimed_by_run_id, t.run_id, t.conversation_id, t.tool_name,
                            t.input_json, t.status, t.requires_approval
                     FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
                     WHERE a.id = ?1 AND a.tool_call_id = ?2",
                    params![input.approval_id, input.tool_call_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, String>(9)?,
                            row.get::<_, i64>(10)?,
                        ))
                    },
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| {
                    RepositoryError::MissingReference(
                        "repair override approval is not bound to the current ToolCall".to_owned(),
                    )
                })?;
            let decision = approval
                .2
                .as_deref()
                .and_then(|value| serde_json::from_str::<Value>(value).ok());
            let approved_once = decision.as_ref().is_some_and(|value| {
                value.get("approved").and_then(Value::as_bool) == Some(true)
                    && value.get("scope").and_then(Value::as_str) == Some("once")
                    && value.get("category").and_then(Value::as_str)
                        == Some(super::TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY)
            });
            let tool_input = serde_json::from_str::<Value>(&approval.8).map_err(|error| {
                RepositoryError::InvalidInput(format!("invalid Host ToolCall input JSON: {error}"))
            })?;
            if approval.0 != "approved"
                || approval.1 != super::TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY
                || !approved_once
                || approval.3.is_some()
                || approval.4.is_some()
                || approval.5 != input.run_id
                || approval.6 != input.conversation_id
                || approval.7 != "task_repair_escalate_start"
                || tool_input != canonical_input
                || approval.9 != if kernel_owned { "running" } else { "pending" }
                || approval.10 != 1
            {
                return Err(RepositoryError::ConstraintViolation(
                    "repair override requires the current pending ToolCall's unclaimed allow_once approval"
                        .to_owned(),
                ));
            }

            if let Err(error) = super::validate_managed_tool_acquisition_with_clock(
                transaction,
                &input.run_id,
                super::now_ms(),
                false,
                !kernel_owned,
            ) {
                // Kernel alone settles the tool and Run; do not publish a
                // Legacy failed ToolCall or consume a projection-only grant.
                if kernel_owned { return Err(RepositoryError::database(error)); }
                let error_message = error.to_string();
                let claimed = transaction
                    .execute(
                        "UPDATE approvals
                         SET claimed_at = ?2, claimed_by_run_id = ?3
                         WHERE id = ?1 AND status = 'approved' AND claimed_at IS NULL
                           AND category = 'task_repair_budget_override'",
                        params![input.approval_id, now, input.run_id],
                    )
                    .map_err(RepositoryError::database)?;
                let tool_failed = transaction
                    .execute(
                        "UPDATE tool_calls
                         SET status = 'failed', error_message = ?3,
                             completed_at = ?4, updated_at = ?4
                         WHERE id = ?1 AND run_id = ?2 AND status = 'pending'
                           AND execution_location = 'host' AND requires_approval = 1",
                        params![input.tool_call_id, input.run_id, error_message, now],
                    )
                    .map_err(RepositoryError::database)?;
                if claimed != 1 || tool_failed != 1 {
                    return Err(RepositoryError::ConstraintViolation(
                        "repair override managed-budget rejection lost its terminal CAS".to_owned(),
                    ));
                }
                return Ok(TransactionOutcome::ManagedBudgetRejected(error_message));
            }

            let attempt_number = transaction
                .query_row(
                    "SELECT COALESCE(MAX(attempt_number), 0) + 1
                     FROM task_attempts WHERE task_id = ?1",
                    [input.task_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(RepositoryError::database)?;
            let evidence_rowid_watermark = transaction
                .query_row(
                    "SELECT COALESCE(MAX(rowid), 0) FROM task_evidence WHERE task_id = ?1",
                    [input.task_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(RepositoryError::database)?;
            let finding_rowid_watermark = transaction
                .query_row(
                    "SELECT COALESCE(MAX(rowid), 0) FROM review_findings
                     WHERE goal_id = ?1 AND task_id = ?2",
                    params![current.goal_id, input.task_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(RepositoryError::database)?;
            let finding_ids_json = serde_json::to_string(&input.finding_ids)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO task_attempts(
                        id, task_id, attempt_number, kind, status, run_id, policy_hash,
                        root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                        version, evidence_rowid_watermark, finding_rowid_watermark,
                        started_at, finished_at
                     ) VALUES (
                        ?1, ?2, ?3, 'repair', 'running', ?4, ?5, ?6, ?7, '[]', NULL, 1,
                        ?8, ?9, ?10, NULL
                     )",
                    params![
                        input.attempt_id,
                        input.task_id,
                        attempt_number,
                        input.run_id,
                        policy.snapshot.hash,
                        input.root_cause,
                        finding_ids_json,
                        evidence_rowid_watermark,
                        finding_rowid_watermark,
                        now,
                    ],
                )
                .map_err(RepositoryError::database)?;
            let event_id = Uuid::new_v4().to_string();
            transaction
                .execute(
                    "INSERT INTO task_repair_override_events(
                        id, task_id, attempt_id, approval_id, tool_call_id, run_id,
                        conversation_id, policy_id, policy_hash, normal_repair_budget,
                        normal_repair_used, override_count, input_hash, escalation_reason,
                        created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1,
                               ?12, ?13, ?14)",
                    params![
                        event_id,
                        input.task_id,
                        input.attempt_id,
                        input.approval_id,
                        input.tool_call_id,
                        input.run_id,
                        input.conversation_id,
                        policy.snapshot.id,
                        policy.snapshot.hash,
                        normal_repair_budget,
                        normal_repair_used,
                        input_hash,
                        input.escalation_reason,
                        now,
                    ],
                )
                .map_err(RepositoryError::database)?;
            let changed = transaction
                .execute(
                    "UPDATE work_tasks
                     SET status = 'in_progress', owner_run_id = ?2, attempt = ?3,
                         version = version + 1, blocked_reason = NULL,
                         started_at = COALESCE(started_at, ?4), finished_at = NULL,
                         updated_at = ?4
                     WHERE id = ?1 AND version = ?5",
                    params![
                        input.task_id,
                        input.run_id,
                        attempt_number,
                        now,
                        input.expected_task_version,
                    ],
                )
                .map_err(RepositoryError::database)?;
            if changed != 1 {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.task_id.clone(),
                    expected_version: input.expected_task_version,
                });
            }
            let claimed = transaction
                .execute(
                    "UPDATE approvals
                     SET claimed_at = ?2, claimed_by_run_id = ?3
                     WHERE id = ?1 AND status = 'approved' AND claimed_at IS NULL
                       AND category = 'task_repair_budget_override'",
                    params![input.approval_id, now, input.run_id],
                )
                .map_err(RepositoryError::database)?;
            let tool_claimed = if kernel_owned { 1 } else { transaction
                .execute(
                    "UPDATE tool_calls SET status = 'running', updated_at = ?2
                     WHERE id = ?1 AND status = 'pending' AND requires_approval = 1",
                    params![input.tool_call_id, now],
                )
                .map_err(RepositoryError::database)? };
            if claimed != 1 || tool_claimed != 1 {
                return Err(RepositoryError::ConstraintViolation(
                    "repair override approval claim lost its one-time CAS".to_owned(),
                ));
            }
            let override_event = load_task_repair_override_event(transaction, &event_id)?
                .ok_or_else(|| not_found("task repair override event", &event_id))?;
            let attempt = load_task_attempt(transaction, &input.attempt_id)?
                .ok_or_else(|| not_found("task attempt", &input.attempt_id))?;
            let task = load_task(transaction, &input.task_id)?
                .ok_or_else(|| not_found("work task", &input.task_id))?;
            Ok(TransactionOutcome::Started(TaskRepairOverrideStartResult {
                task,
                attempt,
                override_event,
            }))
        })?;
        match outcome {
            TransactionOutcome::Started(result) => Ok(result),
            TransactionOutcome::ManagedBudgetRejected(message) => {
                Err(RepositoryError::ConstraintViolation(message))
            }
        }
    }

    pub fn preflight_task_repair_override(
        &self,
        input: PreflightTaskRepairOverrideInput,
    ) -> Result<(), RepositoryError> {
        validate_preflight_task_repair_override(&input)?;
        with_read_transaction(self, |transaction| {
            let kernel_owned: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
                [&input.run_id],|row|row.get(0)).map_err(RepositoryError::database)?;
            super::validate_managed_tool_acquisition_with_clock(
                transaction,
                &input.run_id,
                super::now_ms(),
                false,
                !kernel_owned,
            )
            .map_err(RepositoryError::database)?;
            validate_task_repair_override_eligibility(
                transaction,
                RepairOverrideEligibilityInput {
                    task_id: &input.task_id,
                    attempt_id: &input.attempt_id,
                    run_id: &input.run_id,
                    conversation_id: &input.conversation_id,
                    expected_task_version: input.expected_task_version,
                    finding_ids: &input.finding_ids,
                },
            )?;
            Ok(())
        })
    }

    pub fn finish_task_attempt(
        &self,
        input: FinishTaskAttemptInput,
    ) -> Result<TaskAttemptFinishResult, RepositoryError> {
        let now = timestamp();
        with_write_transaction(self, |transaction| {
            if matches!(
                input.status,
                TaskAttemptStatus::Succeeded
                    | TaskAttemptStatus::Failed
                    | TaskAttemptStatus::Cancelled
                    | TaskAttemptStatus::Blocked
            ) {
                let graph_attempt = transaction
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM child_run_delegations
                            WHERE graph_task_attempt_id = ?1
                         )",
                        [input.id.as_str()],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(RepositoryError::database)?;
                if graph_attempt {
                    let message = match input.status {
                        TaskAttemptStatus::Succeeded => "[graph.readonly_dedicated_finish_required] Graph node acceptance requires graph_readonly_node_finish with complete criterion-to-Evidence bindings",
                        TaskAttemptStatus::Blocked => "[graph.readonly_dedicated_review_required] Graph node blocking requires an immutable independent Reviewer revise decision",
                        _ => "[graph.readonly_dedicated_terminal_required] Graph node failure and cancellation are Host-owned Child terminal reconciliations; generic task_attempt_finish cannot set them",
                    };
                    return Err(RepositoryError::ConstraintViolation(message.to_owned()));
                }
            }
            finish_task_attempt_in_transaction(transaction, &input, &now)
        })
    }

    pub(crate) fn load_work_graph_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<
        (
            Vec<GoalRecord>,
            Vec<WorkTaskRecord>,
            Vec<TaskEvidenceRecord>,
        ),
        RepositoryError,
    > {
        with_read_connection(self, |connection| {
            load_work_graph_facts(connection, conversation_id)
        })
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn load_work_snapshot_v2(
        &self,
        conversation_id: &str,
    ) -> Result<
        (
            Option<GoalRecord>,
            Vec<WorkTaskRecord>,
            Vec<TaskEvidenceRecord>,
            Vec<PlanRevisionRecord>,
            Vec<ReviewFindingRecord>,
            Vec<AcceptanceRecord>,
            Vec<TaskValidationPolicyRecord>,
            Vec<TaskAttemptRecord>,
            TaskLedgerProjection,
            Value,
        ),
        RepositoryError,
    > {
        validate_non_empty("conversation id", conversation_id)?;
        with_read_transaction(self, |transaction| {
            ensure_conversation_exists(transaction, conversation_id)?;
            let generated_at = now_ms();
            let goals = load_goals(transaction, conversation_id)?;
            let selected_goal = select_work_snapshot_goal(&goals).cloned();
            let Some(goal) = selected_goal else {
                let ledger = project_task_ledger_from_facts(
                    transaction,
                    conversation_id,
                    generated_at,
                    &goals,
                    &[],
                    &[],
                    None,
                )?;
                return Ok((
                    None,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    bounded_task_ledger_for_snapshot(ledger),
                    empty_work_snapshot_history_summary(),
                ));
            };

            let tasks = load_tasks_for_goal(transaction, &goal.id)?;
            let evidence = load_bounded_evidence_for_goal(transaction, &goal.id)?;
            let ledger = project_task_ledger_from_facts(
                transaction,
                conversation_id,
                generated_at,
                &goals,
                &tasks,
                &evidence,
                Some(&goal),
            )?;
            let bounded_ledger = bounded_task_ledger_for_snapshot(ledger);
            let snapshot_tasks = bounded_snapshot_tasks(&tasks, &bounded_ledger);
            let snapshot_evidence = bounded_snapshot_evidence(&snapshot_tasks, &evidence);
            let validation_policies = snapshot_tasks
                .iter()
                .map(|task| load_task_validation_policy(transaction, &task.id))
                .collect::<Result<Vec<_>, _>>()?;
            let snapshot_task_ids = snapshot_tasks
                .iter()
                .map(|task| task.id.clone())
                .collect::<Vec<_>>();
            let task_attempts = load_bounded_task_attempts(
                transaction,
                &snapshot_task_ids,
                WORK_SNAPSHOT_ATTEMPTS_PER_TASK_LIMIT,
            )?;
            let (plans, findings, acceptances) =
                super::a1_workflow::load_bounded_a1_snapshot_for_goal(
                    transaction,
                    conversation_id,
                    &goal.id,
                    WORK_SNAPSHOT_PLAN_LIMIT,
                    WORK_SNAPSHOT_FINDING_LIMIT,
                    WORK_SNAPSHOT_ACCEPTANCE_LIMIT,
                )
                .map_err(RepositoryError::database)?;
            let history_summary = load_work_snapshot_history_summary(
                transaction,
                &goal.id,
                snapshot_tasks.len(),
                snapshot_evidence.len(),
                plans.len(),
                findings.len(),
                acceptances.len(),
                bounded_ledger.projection_meta.derived_run_ids.len(),
            )?;
            Ok((
                Some(goal),
                snapshot_tasks,
                snapshot_evidence,
                plans,
                findings,
                acceptances,
                validation_policies,
                task_attempts,
                bounded_ledger,
                history_summary,
            ))
        })
    }

    pub fn append_continuation_decision(
        &self,
        input: AppendContinuationDecisionInput,
    ) -> Result<ContinuationDecisionRecord, RepositoryError> {
        validate_continuation_input(&input)?;
        let created_at = now_ms();
        with_write_transaction(self, |transaction| {
            if let Some(existing) =
                load_continuation_decision_by_id(transaction, &input.decision_id)?
            {
                if continuation_decision_matches_input(&existing, &input) {
                    return Ok(existing);
                }
                return Err(RepositoryError::ConstraintViolation(format!(
                    "continuation decision '{}' conflicts with existing content",
                    input.decision_id
                )));
            }
            let conversation_id = validate_continuation_associations(transaction, &input)?;
            if let Some(expected_hash) = input.expected_projection_hash.as_deref() {
                let current = project_task_ledger(transaction, &conversation_id, created_at)?;
                let actual_hash = current.projection_meta.projection_hash;
                if actual_hash != expected_hash {
                    return Err(RepositoryError::ProjectionHashMismatch {
                        expected: expected_hash.to_owned(),
                        actual: actual_hash,
                    });
                }
            }
            let record = ContinuationDecisionRecord {
                schema_version: input.schema_version,
                decision_id: input.decision_id.clone(),
                run_id: input.run_id.clone(),
                event_cursor: input.event_cursor,
                decision: input.decision.clone(),
                reason_code: input.reason_code.clone(),
                active_task_ids: input.active_task_ids.clone(),
                evidence_ids: input.evidence_ids.clone(),
                missing_acceptance: input.missing_acceptance.clone(),
                next_action: input.next_action.clone(),
                retry_class: input.retry_class.clone(),
                blocked_dependency_refs: input.blocked_dependency_refs.clone(),
                host_validation_outcome: input.host_validation_outcome.clone(),
                host_validation_error: input.host_validation_error.clone(),
                created_at,
            };
            transaction
                .execute(
                    "INSERT INTO run_continuation_decisions(
                        decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                        active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                        next_action, retry_class, blocked_dependency_refs_json,
                        host_validation_outcome, host_validation_error, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        record.decision_id,
                        i64::from(record.schema_version),
                        record.run_id,
                        record.event_cursor,
                        continuation_decision_text(&record.decision),
                        continuation_reason_text(&record.reason_code),
                        serde_json::to_string(&record.active_task_ids)
                            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?,
                        serde_json::to_string(&record.evidence_ids)
                            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?,
                        serde_json::to_string(&record.missing_acceptance)
                            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?,
                        record.next_action,
                        record.retry_class.as_ref().map(continuation_retry_text),
                        serde_json::to_string(&record.blocked_dependency_refs)
                            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?,
                        continuation_validation_text(&record.host_validation_outcome),
                        record.host_validation_error,
                        record.created_at,
                    ],
                )
                .map_err(RepositoryError::database)?;
            Ok(record)
        })
    }

    pub fn authoritative_run_conversation_id(
        &self,
        run_id: &str,
    ) -> Result<String, RepositoryError> {
        validate_non_empty("run id", run_id)?;
        with_read_connection(self, |connection| {
            connection
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| not_found("run", run_id))
        })
    }

    pub fn record_continuation_ingest_diagnostic(
        &self,
        input: &RecordContinuationIngestDiagnosticInput,
    ) -> Result<ContinuationIngestDiagnosticRecord, RepositoryError> {
        validate_continuation_ingest_diagnostic(input)?;
        let created_at = now_ms();
        with_write_transaction(self, |transaction| {
            if let Some(existing) =
                load_continuation_ingest_diagnostic(transaction, &input.run_id, input.event_seq)?
            {
                if continuation_ingest_diagnostic_matches_input(&existing, input) {
                    return Ok(existing);
                }
                return Err(RepositoryError::ConstraintViolation(format!(
                    "continuation ingest diagnostic for run '{}' event {} conflicts with existing content",
                    input.run_id, input.event_seq
                )));
            }

            let event_type = transaction
                .query_row(
                    "SELECT event_type FROM run_events WHERE run_id = ?1 AND seq = ?2",
                    params![input.run_id, input.event_seq],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| {
                    not_found(
                        "run event",
                        &format!("{}:{}", input.run_id, input.event_seq),
                    )
                })?;
            if event_type != "run.continuation_proposed" {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "run event '{}:{}' is not a continuation proposal",
                    input.run_id, input.event_seq
                )));
            }

            let record = ContinuationIngestDiagnosticRecord {
                run_id: input.run_id.clone(),
                event_seq: input.event_seq,
                execution_profile_id: input.execution_profile_id.clone(),
                shadow_mode: input.shadow_mode,
                error_code: input.error_code.clone(),
                error_message: input.error_message.clone(),
                created_at,
            };
            transaction
                .execute(
                    "INSERT INTO run_continuation_ingest_diagnostics(
                        run_id, event_seq, execution_profile_id, shadow_mode,
                        error_code, error_message, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        record.run_id,
                        record.event_seq,
                        record.execution_profile_id,
                        record.shadow_mode,
                        record.error_code,
                        record.error_message,
                        record.created_at,
                    ],
                )
                .map_err(RepositoryError::database)?;
            Ok(record)
        })
    }

    pub fn unprojected_continuation_proposals(
        &self,
        limit: usize,
    ) -> Result<Vec<UnprojectedContinuationProposal>, RepositoryError> {
        if !(1..=500).contains(&limit) {
            return Err(RepositoryError::InvalidInput(
                "unprojected continuation proposal limit must be between 1 and 500".to_owned(),
            ));
        }
        let limit = i64::try_from(limit).map_err(|error| {
            RepositoryError::InvalidInput(format!("invalid continuation proposal limit: {error}"))
        })?;
        with_read_connection(self, |connection| {
            connection
                .prepare(
                    "SELECT runs.conversation_id, events.run_id, events.seq, events.event_json,
                            COALESCE(
                                (
                                    SELECT json_extract(started.event_json, '$.executionProfileId')
                                    FROM run_events started
                                    WHERE started.run_id = events.run_id
                                      AND started.event_type = 'run.started'
                                      AND started.seq < events.seq
                                      AND json_type(
                                          started.event_json, '$.executionProfileId'
                                      ) = 'text'
                                      AND NULLIF(trim(json_extract(
                                          started.event_json, '$.executionProfileId'
                                      )), '') IS NOT NULL
                                    ORDER BY started.seq
                                    LIMIT 1
                                ),
                                (
                                    SELECT json_extract(
                                        snapshot.event_json, '$.executionProfile.id'
                                    )
                                    FROM run_events snapshot
                                    WHERE snapshot.run_id = events.run_id
                                      AND snapshot.event_type = 'run.request_snapshot'
                                      AND snapshot.seq < events.seq
                                      AND json_type(
                                          snapshot.event_json, '$.executionProfile.id'
                                      ) = 'text'
                                      AND NULLIF(trim(json_extract(
                                          snapshot.event_json, '$.executionProfile.id'
                                      )), '') IS NOT NULL
                                    ORDER BY snapshot.seq DESC
                                    LIMIT 1
                                )
                            ) AS execution_profile_id
                     FROM run_events events
                     JOIN runs ON runs.id = events.run_id
                     WHERE events.event_type = 'run.continuation_proposed'
                       AND NOT EXISTS (
                           SELECT 1 FROM run_continuation_decisions decisions
                           WHERE decisions.run_id = events.run_id
                             AND decisions.decision_id =
                                 json_extract(events.event_json, '$.proposal.decisionId')
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM run_continuation_ingest_diagnostics diagnostics
                           WHERE diagnostics.run_id = events.run_id
                             AND diagnostics.event_seq = events.seq
                       )
                     ORDER BY events.created_at, events.run_id, events.seq
                     LIMIT ?1",
                )
                .map_err(RepositoryError::database)?
                .query_map([limit], |row| {
                    let encoded = row.get::<_, String>(3)?;
                    let payload = serde_json::from_str(&encoded).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(UnprojectedContinuationProposal {
                        conversation_id: row.get(0)?,
                        run_id: row.get(1)?,
                        seq: row.get(2)?,
                        payload,
                        execution_profile_id: row.get(4)?,
                    })
                })
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)
        })
    }

    pub fn continuation_decisions_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<ContinuationDecisionRecord>, RepositoryError> {
        validate_non_empty("run id", run_id)?;
        with_read_connection(self, |connection| {
            let exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
                    [run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(RepositoryError::database)?;
            if !exists {
                return Err(not_found("run", run_id));
            }
            load_continuation_decisions(connection, run_id)
        })
    }

    pub fn continuation_decision_by_id(
        &self,
        run_id: &str,
        decision_id: &str,
    ) -> Result<Option<ContinuationDecisionRecord>, RepositoryError> {
        validate_non_empty("run id", run_id)?;
        validate_non_empty("decision id", decision_id)?;
        with_read_connection(self, |connection| {
            let decision = load_continuation_decision_by_id(connection, decision_id)?;
            if decision
                .as_ref()
                .is_some_and(|record| record.run_id != run_id)
            {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "continuation decision '{decision_id}' belongs to a different run"
                )));
            }
            Ok(decision)
        })
    }

    pub fn task_ledger_projection(
        &self,
        conversation_id: &str,
    ) -> Result<TaskLedgerProjection, RepositoryError> {
        validate_non_empty("conversation id", conversation_id)?;
        with_read_connection(self, |connection| {
            project_task_ledger(connection, conversation_id, now_ms())
        })
    }
}

fn ensure_conversation_exists(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), RepositoryError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            [conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(RepositoryError::database)?;
    if exists {
        Ok(())
    } else {
        Err(not_found("conversation", conversation_id))
    }
}

fn load_task_validation_policy(
    connection: &Connection,
    task_id: &str,
) -> Result<TaskValidationPolicyRecord, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT schema_version, policy_id, snapshot_json, policy_hash, frozen_at
             FROM task_validation_policies WHERE task_id = ?1",
            [task_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(RepositoryError::database)?;
    let Some((schema_version, policy_id, snapshot_json, policy_hash, frozen_at)) = row else {
        require_exists(connection, "work_tasks", task_id)?;
        return Ok(TaskValidationPolicyRecord {
            task_id: task_id.to_owned(),
            snapshot: validation_policy_snapshot("legacy_v1")?,
            frozen_at: "legacy".to_owned(),
            legacy_fallback: true,
        });
    };
    if schema_version != 1 {
        return Err(RepositoryError::InvalidValidationPolicy(format!(
            "Task '{task_id}' uses unsupported schema version {schema_version}"
        )));
    }
    let expected = validation_policy_snapshot(&policy_id)?;
    let raw = serde_json::from_str::<Value>(&snapshot_json).map_err(|error| {
        RepositoryError::InvalidValidationPolicy(format!(
            "Task '{task_id}' policy snapshot is not valid JSON: {error}"
        ))
    })?;
    let expected_value = serde_json::to_value(&expected)
        .map_err(|error| RepositoryError::InvalidValidationPolicy(error.to_string()))?;
    if raw != expected_value || policy_hash != expected.hash {
        return Err(RepositoryError::InvalidValidationPolicy(format!(
            "Task '{task_id}' policy fields or hash do not match frozen policy '{policy_id}'"
        )));
    }
    Ok(TaskValidationPolicyRecord {
        task_id: task_id.to_owned(),
        snapshot: expected,
        frozen_at: frozen_at.to_string(),
        legacy_fallback: false,
    })
}

fn validate_start_task_attempt(input: &StartTaskAttemptInput) -> Result<(), RepositoryError> {
    validate_bounded_text("attempt id", &input.id, 200)?;
    validate_bounded_text("task id", &input.task_id, 200)?;
    validate_bounded_text("run id", &input.run_id, 200)?;
    if input.expected_task_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Task version must be at least 1".to_owned(),
        ));
    }
    validate_identifier_list("repair finding ids", &input.finding_ids)?;
    if input.finding_ids.len() > 32 {
        return Err(RepositoryError::InvalidInput(
            "a repair Attempt cannot cite more than 32 findings".to_owned(),
        ));
    }
    for finding_id in &input.finding_ids {
        validate_bounded_text("repair finding id", finding_id, 200)?;
    }
    match input.kind {
        TaskAttemptKind::Execution
            if input.root_cause.is_some() || !input.finding_ids.is_empty() =>
        {
            Err(RepositoryError::InvalidInput(
                "an execution Attempt cannot include repair root cause or findings".to_owned(),
            ))
        }
        TaskAttemptKind::Repair => {
            let root_cause = input.root_cause.as_deref().ok_or_else(|| {
                RepositoryError::InvalidInput("a repair Attempt requires rootCause".to_owned())
            })?;
            validate_bounded_text("repair root cause", root_cause, 4000)?;
            if input.finding_ids.is_empty() {
                return Err(RepositoryError::InvalidInput(
                    "a repair Attempt requires at least one finding".to_owned(),
                ));
            }
            Ok(())
        }
        TaskAttemptKind::Execution => Ok(()),
    }
}

fn validate_start_task_repair_override(
    input: &StartTaskRepairOverrideInput,
) -> Result<(), RepositoryError> {
    validate_bounded_text("attempt id", &input.attempt_id, 200)?;
    validate_bounded_text("task id", &input.task_id, 200)?;
    validate_bounded_text("run id", &input.run_id, 200)?;
    validate_bounded_text("conversation id", &input.conversation_id, 200)?;
    validate_bounded_text("ToolCall id", &input.tool_call_id, 200)?;
    validate_bounded_text("approval id", &input.approval_id, 200)?;
    validate_bounded_text("repair root cause", &input.root_cause, 4000)?;
    validate_bounded_text("escalation reason", &input.escalation_reason, 2000)?;
    if input.expected_task_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Task version must be at least 1".to_owned(),
        ));
    }
    validate_identifier_list("repair finding ids", &input.finding_ids)?;
    if input.finding_ids.is_empty() || input.finding_ids.len() > 32 {
        return Err(RepositoryError::InvalidInput(
            "a repair override requires between 1 and 32 unique findings".to_owned(),
        ));
    }
    for finding_id in &input.finding_ids {
        validate_bounded_text("repair finding id", finding_id, 200)?;
    }
    Ok(())
}

fn validate_preflight_task_repair_override(
    input: &PreflightTaskRepairOverrideInput,
) -> Result<(), RepositoryError> {
    validate_bounded_text("attempt id", &input.attempt_id, 200)?;
    validate_bounded_text("task id", &input.task_id, 200)?;
    validate_bounded_text("run id", &input.run_id, 200)?;
    validate_bounded_text("conversation id", &input.conversation_id, 200)?;
    validate_bounded_text("repair root cause", &input.root_cause, 4000)?;
    validate_bounded_text("escalation reason", &input.escalation_reason, 2000)?;
    if input.expected_task_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Task version must be at least 1".to_owned(),
        ));
    }
    validate_identifier_list("repair finding ids", &input.finding_ids)?;
    if input.finding_ids.is_empty() || input.finding_ids.len() > 32 {
        return Err(RepositoryError::InvalidInput(
            "a repair override requires between 1 and 32 unique findings".to_owned(),
        ));
    }
    for finding_id in &input.finding_ids {
        validate_bounded_text("repair finding id", finding_id, 200)?;
    }
    Ok(())
}

fn task_repair_override_input_value(input: &StartTaskRepairOverrideInput) -> Value {
    serde_json::json!({
        "taskId": input.task_id,
        "attemptId": input.attempt_id,
        "expectedVersion": input.expected_task_version,
        "rootCause": input.root_cause,
        "findingIds": input.finding_ids,
        "escalationReason": input.escalation_reason,
    })
}

pub(super) fn finish_task_attempt_in_transaction(
    transaction: &Transaction<'_>,
    input: &FinishTaskAttemptInput,
    now: &str,
) -> Result<TaskAttemptFinishResult, RepositoryError> {
    finish_task_attempt_with_authority_in_transaction(transaction, input, now, false)
}

pub(super) fn finish_graph_task_attempt_in_transaction(
    transaction: &Transaction<'_>,
    input: &FinishTaskAttemptInput,
    now: &str,
) -> Result<TaskAttemptFinishResult, RepositoryError> {
    finish_task_attempt_with_authority_in_transaction(transaction, input, now, true)
}

fn finish_task_attempt_with_authority_in_transaction(
    transaction: &Transaction<'_>,
    input: &FinishTaskAttemptInput,
    now: &str,
    graph_authorized: bool,
) -> Result<TaskAttemptFinishResult, RepositoryError> {
    validate_finish_task_attempt(input)?;
    let current_task = load_task(transaction, &input.task_id)?
        .ok_or_else(|| not_found("work task", &input.task_id))?;
    let attempt = load_task_attempt(transaction, &input.id)?
        .ok_or_else(|| not_found("task attempt", &input.id))?;
    if attempt.task_id != input.task_id || attempt.run_id != input.run_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if attempt.status == input.status
        && attempt.failure_reason == input.failure_reason
        && attempt.finished_at.is_some()
    {
        return Ok(TaskAttemptFinishResult {
            task: current_task,
            attempt,
        });
    }
    if current_task.version != input.expected_task_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.task_id.clone(),
            expected_version: input.expected_task_version,
        });
    }
    if attempt.version != input.expected_attempt_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.id.clone(),
            expected_version: input.expected_attempt_version,
        });
    }
    if attempt.status != TaskAttemptStatus::Running
        || current_task.status != WorkTaskStatus::InProgress
        || current_task.owner_run_id.as_deref() != Some(input.run_id.as_str())
    {
        return Err(RepositoryError::InvalidTransition {
            entity: "task attempt",
            from: task_attempt_status_text(&attempt.status).to_owned(),
            to: task_attempt_status_text(&input.status).to_owned(),
        });
    }
    ensure_run_matches_task_conversation(transaction, &input.run_id, &input.task_id)?;
    let policy = load_task_validation_policy(transaction, &input.task_id)?;
    if attempt.policy_hash != policy.snapshot.hash {
        return Err(RepositoryError::InvalidValidationPolicy(
            "Attempt policy hash no longer matches the Task frozen policy".to_owned(),
        ));
    }
    let evidence_ids = load_valid_evidence_ids(
        transaction,
        &input.task_id,
        attempt.evidence_rowid_watermark,
    )?;
    if input.status == TaskAttemptStatus::Succeeded {
        if graph_authorized {
            validate_graph_task_completion_mechanics(
                transaction,
                &current_task,
                &attempt,
                &policy.snapshot,
            )?;
        } else {
            validate_task_completion_policy(
                transaction,
                &current_task,
                &attempt,
                &policy.snapshot,
            )?;
        }
    }
    let evidence_ids_json = serde_json::to_string(&evidence_ids)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let changed = transaction
        .execute(
            "UPDATE task_attempts
             SET status = ?2, evidence_ids_json = ?3, failure_reason = ?4,
                 version = version + 1, finished_at = ?5
             WHERE id = ?1 AND status = 'running' AND version = ?6",
            params![
                input.id,
                task_attempt_status_text(&input.status),
                evidence_ids_json,
                input.failure_reason,
                now,
                input.expected_attempt_version
            ],
        )
        .map_err(RepositoryError::database)?;
    if changed != 1 {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.id.clone(),
            expected_version: input.expected_attempt_version,
        });
    }
    let (task_status, blocked_reason) = match input.status {
        TaskAttemptStatus::Succeeded => ("completed", None),
        TaskAttemptStatus::Blocked => ("blocked", input.failure_reason.as_deref()),
        TaskAttemptStatus::Failed | TaskAttemptStatus::Cancelled => {
            ("interrupted", input.failure_reason.as_deref())
        }
        TaskAttemptStatus::Running => unreachable!("finish validation rejects running"),
    };
    let changed = transaction
        .execute(
            "UPDATE work_tasks
             SET status = ?2, blocked_reason = ?3, owner_run_id = NULL,
                 version = version + 1, updated_at = ?4, finished_at = ?4
             WHERE id = ?1 AND status = 'in_progress' AND version = ?5",
            params![
                input.task_id,
                task_status,
                blocked_reason,
                now,
                input.expected_task_version
            ],
        )
        .map_err(RepositoryError::database)?;
    if changed != 1 {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.task_id.clone(),
            expected_version: input.expected_task_version,
        });
    }
    Ok(TaskAttemptFinishResult {
        task: load_task(transaction, &input.task_id)?
            .ok_or_else(|| not_found("work task", &input.task_id))?,
        attempt: load_task_attempt(transaction, &input.id)?
            .ok_or_else(|| not_found("task attempt", &input.id))?,
    })
}

fn validate_graph_task_completion_mechanics(
    connection: &Connection,
    task: &WorkTaskRecord,
    attempt: &TaskAttemptRecord,
    policy: &ValidationPolicySnapshot,
) -> Result<(), RepositoryError> {
    let evidence = connection
        .prepare(
            "SELECT rowid, source_run_id, metadata_json, evidence_type, ref_kind, ref_id
             FROM task_evidence
             WHERE task_id = ?1 AND validity_status = 'valid' AND rowid > ?2
             ORDER BY rowid DESC",
        )
        .map_err(RepositoryError::database)?
        .query_map(params![task.id, attempt.evidence_rowid_watermark], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)?;
    let mut inspection_seen = false;
    for (_, source_run_id, metadata_json, evidence_type, ref_kind, ref_id) in evidence {
        let metadata = serde_json::from_str::<Value>(&metadata_json).map_err(|error| {
            RepositoryError::InvalidInput(format!("invalid Evidence metadata JSON: {error}"))
        })?;
        let Some(check_type) = metadata.get("validationCheckType").and_then(Value::as_str) else {
            continue;
        };
        if !policy
            .allowed_check_types
            .iter()
            .any(|allowed| allowed == check_type)
        {
            return Err(RepositoryError::InvalidValidationPolicy(format!(
                "Evidence uses check type '{check_type}' outside policy '{}'",
                policy.id
            )));
        }
        validate_evidence_check_semantics(
            connection,
            check_type,
            &evidence_type,
            &ref_kind,
            &ref_id,
            source_run_id.as_deref(),
            &metadata,
        )?;
        inspection_seen |= check_type == "inspection";
    }
    if !inspection_seen {
        return Err(RepositoryError::ValidationChecksMissing {
            task_id: task.id.clone(),
            check_types: vec!["inspection".to_owned()],
        });
    }
    let blockers = connection
        .query_row(
            "SELECT COUNT(*) FROM review_findings
             WHERE goal_id = ?1 AND (task_id = ?2 OR task_id IS NULL)
               AND status = 'open' AND severity IN ('critical', 'high', 'medium')",
            params![task.goal_id, task.id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    if blockers > 0 {
        return Err(RepositoryError::ReviewBlocked {
            task_id: task.id.clone(),
        });
    }
    Ok(())
}

fn validate_finish_task_attempt(input: &FinishTaskAttemptInput) -> Result<(), RepositoryError> {
    validate_bounded_text("attempt id", &input.id, 200)?;
    validate_bounded_text("task id", &input.task_id, 200)?;
    validate_bounded_text("run id", &input.run_id, 200)?;
    if input.expected_task_version < 1 || input.expected_attempt_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Task and Attempt versions must be at least 1".to_owned(),
        ));
    }
    if input.status == TaskAttemptStatus::Running {
        return Err(RepositoryError::InvalidInput(
            "finishing an Attempt requires a terminal status".to_owned(),
        ));
    }
    match (
        input.status == TaskAttemptStatus::Succeeded,
        input.failure_reason.as_deref(),
    ) {
        (true, None) => Ok(()),
        (true, Some(_)) => Err(RepositoryError::InvalidInput(
            "a succeeded Attempt cannot include failureReason".to_owned(),
        )),
        (false, Some(reason)) => validate_bounded_text("Attempt failure reason", reason, 4000),
        (false, None) => Err(RepositoryError::InvalidInput(
            "a non-success Attempt requires failureReason".to_owned(),
        )),
    }
}

fn validate_repair_findings(
    connection: &Connection,
    task: &WorkTaskRecord,
    finding_ids: &[String],
) -> Result<(), RepositoryError> {
    for finding_id in finding_ids {
        let finding = connection
            .query_row(
                "SELECT goal_id, task_id, status FROM review_findings WHERE id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(RepositoryError::database)?
            .ok_or_else(|| not_found("review finding", finding_id))?;
        if finding.0 != task.goal_id || finding.1.as_deref() != Some(task.id.as_str()) {
            return Err(RepositoryError::InvalidReference(format!(
                "review finding '{finding_id}' is not attached to Task '{}'",
                task.id
            )));
        }
        if finding.2 != "open" {
            return Err(RepositoryError::InvalidReference(format!(
                "review finding '{finding_id}' is not open"
            )));
        }
    }
    Ok(())
}

struct RepairOverrideEligibilityInput<'a> {
    task_id: &'a str,
    attempt_id: &'a str,
    run_id: &'a str,
    conversation_id: &'a str,
    expected_task_version: i64,
    finding_ids: &'a [String],
}

struct RepairOverrideEligibility {
    task: WorkTaskRecord,
    policy: TaskValidationPolicyRecord,
    normal_repair_used: i64,
    normal_repair_budget: i64,
}

fn validate_task_repair_override_eligibility(
    connection: &Connection,
    input: RepairOverrideEligibilityInput<'_>,
) -> Result<RepairOverrideEligibility, RepositoryError> {
    let task = load_task(connection, input.task_id)?
        .ok_or_else(|| not_found("work task", input.task_id))?;
    if task.version != input.expected_task_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: input.task_id.to_owned(),
            expected_version: input.expected_task_version,
        });
    }
    require_durable_override_run_profile(connection, input.run_id, input.conversation_id)?;
    ensure_run_matches_task_conversation(connection, input.run_id, input.task_id)?;
    if task_conversation(connection, input.task_id)? != input.conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }

    let goal = connection
        .query_row(
            "SELECT conversation_id, status FROM goals WHERE id = ?1",
            [task.goal_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(RepositoryError::database)?
        .ok_or_else(|| not_found("goal", &task.goal_id))?;
    if goal.0 != input.conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if goal.1 != "active" {
        return Err(RepositoryError::InvalidTransition {
            entity: "goal for task repair override",
            from: goal.1,
            to: "active".to_owned(),
        });
    }
    let latest_plan_status = connection
        .query_row(
            "SELECT status FROM plan_revisions
             WHERE goal_id = ?1 AND conversation_id = ?2
             ORDER BY revision DESC, id DESC LIMIT 1",
            params![task.goal_id, input.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(RepositoryError::database)?;
    if latest_plan_status.as_deref() != Some("approved") {
        return Err(RepositoryError::ConstraintViolation(
            "repair override requires the latest PlanRevision to be explicitly approved".to_owned(),
        ));
    }

    let policy = load_task_validation_policy(connection, input.task_id)?;
    if !matches!(policy.snapshot.id.as_str(), "standard_v1" | "high_risk_v1") {
        return Err(RepositoryError::InvalidValidationPolicy(
            "repair budget override requires a frozen durable ValidationPolicy".to_owned(),
        ));
    }
    if !matches!(
        task.status,
        WorkTaskStatus::Blocked | WorkTaskStatus::Interrupted
    ) {
        return Err(RepositoryError::InvalidTransition {
            entity: "work task repair override",
            from: task_status_text(&task.status).to_owned(),
            to: "in_progress".to_owned(),
        });
    }
    if load_running_task_attempt(connection, input.task_id)?.is_some() {
        return Err(RepositoryError::ConstraintViolation(
            "a task can have only one running Attempt".to_owned(),
        ));
    }
    validate_repair_findings(connection, &task, input.finding_ids)?;

    let attempt_id_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_attempts WHERE id = ?1)",
            [input.attempt_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(RepositoryError::database)?;
    if attempt_id_exists {
        return Err(RepositoryError::ConstraintViolation(format!(
            "task Attempt '{}' already exists",
            input.attempt_id
        )));
    }
    let override_used = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_repair_override_events WHERE task_id = ?1)",
            [input.task_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(RepositoryError::database)?;
    if override_used {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Task '{}' already consumed its one-time repair budget override",
            input.task_id
        )));
    }
    let normal_repair_used = connection
        .query_row(
            "SELECT COUNT(*)
             FROM task_attempts attempts
             LEFT JOIN task_repair_override_events overrides
               ON overrides.attempt_id = attempts.id
             WHERE attempts.task_id = ?1 AND attempts.kind = 'repair'
               AND overrides.id IS NULL",
            [input.task_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    let normal_repair_budget = i64::from(policy.snapshot.max_repair_attempts);
    if normal_repair_budget < 1 || normal_repair_used < normal_repair_budget {
        return Err(RepositoryError::ConstraintViolation(format!(
            "normal Repair budget is not exhausted for Task '{}': {normal_repair_used}/{normal_repair_budget}",
            input.task_id
        )));
    }
    Ok(RepairOverrideEligibility {
        task,
        policy,
        normal_repair_used,
        normal_repair_budget,
    })
}

fn validate_task_completion_policy(
    connection: &Connection,
    task: &WorkTaskRecord,
    attempt: &TaskAttemptRecord,
    policy: &ValidationPolicySnapshot,
) -> Result<(), RepositoryError> {
    let mut valid_checks = HashSet::new();
    let mut independent_review_runs = HashSet::new();
    let mut valid_evidence = 0usize;
    let mut statement = connection
        .prepare(
            "SELECT rowid, source_run_id, metadata_json, evidence_type, ref_kind, ref_id
             FROM task_evidence
             WHERE task_id = ?1 AND validity_status = 'valid'
             ORDER BY rowid DESC",
        )
        .map_err(RepositoryError::database)?;
    let evidence = statement
        .query_map([task.id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)?;
    for (rowid, source_run_id, metadata_json, evidence_type, ref_kind, ref_id) in evidence {
        if rowid <= attempt.evidence_rowid_watermark {
            continue;
        }
        valid_evidence += 1;
        let metadata = serde_json::from_str::<Value>(&metadata_json).map_err(|error| {
            RepositoryError::InvalidInput(format!("invalid Evidence metadata JSON: {error}"))
        })?;
        let check_type = metadata.get("validationCheckType").and_then(Value::as_str);
        if let Some(check_type) = check_type {
            if !policy
                .allowed_check_types
                .iter()
                .any(|allowed| allowed == check_type)
            {
                return Err(RepositoryError::InvalidValidationPolicy(format!(
                    "Evidence uses check type '{check_type}' outside policy '{}'",
                    policy.id
                )));
            }
            validate_evidence_check_semantics(
                connection,
                check_type,
                &evidence_type,
                &ref_kind,
                &ref_id,
                source_run_id.as_deref(),
                &metadata,
            )?;
            valid_checks.insert(check_type.to_owned());
            if check_type == "review" {
                if let Some(source_run_id) =
                    source_run_id.filter(|run_id| run_id != &attempt.run_id)
                {
                    independent_review_runs.insert(source_run_id);
                }
            }
        }
    }
    if valid_evidence == 0 {
        return Err(RepositoryError::EvidenceRequired {
            task_id: task.id.clone(),
        });
    }
    let missing_checks = policy
        .required_checks
        .iter()
        .filter(|required| !valid_checks.contains(required.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_checks.is_empty() {
        return Err(RepositoryError::ValidationChecksMissing {
            task_id: task.id.clone(),
            check_types: missing_checks,
        });
    }
    let blockers = connection
        .query_row(
            "SELECT COUNT(*) FROM review_findings
             WHERE goal_id = ?1 AND (task_id = ?2 OR task_id IS NULL)
               AND status = 'open' AND severity IN ('critical', 'high', 'medium')",
            params![task.goal_id, task.id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)?;
    if blockers > 0 {
        return Err(RepositoryError::ReviewBlocked {
            task_id: task.id.clone(),
        });
    }
    if policy.reviewer_policy == "independent" {
        let fresh_finding_reviewers = connection
            .prepare(
                "SELECT created_by FROM review_findings
                 WHERE goal_id = ?1 AND task_id = ?2
                   AND rowid > ?3",
            )
            .map_err(RepositoryError::database)?
            .query_map(
                params![task.goal_id, task.id, attempt.finding_rowid_watermark],
                |row| row.get::<_, String>(0),
            )
            .map_err(RepositoryError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::database)?;
        let independent_finding = fresh_finding_reviewers
            .iter()
            .any(|run_id| independent_review_runs.contains(run_id));
        let implementation_self_resolved_blocker = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM review_findings
                    WHERE goal_id = ?1 AND (task_id = ?2 OR task_id IS NULL)
                      AND severity IN ('critical', 'high', 'medium')
                      AND status IN ('resolved', 'waived')
                      AND resolved_by IN (
                          SELECT run_id FROM task_attempts WHERE task_id = ?2
                      )
                 )",
                params![task.goal_id, task.id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(RepositoryError::database)?;
        if !independent_finding
            || independent_review_runs.is_empty()
            || implementation_self_resolved_blocker
        {
            return Err(RepositoryError::IndependentReviewRequired {
                task_id: task.id.clone(),
                implementation_run_id: attempt.run_id.clone(),
            });
        }
    }
    Ok(())
}

fn load_valid_evidence_ids(
    connection: &Connection,
    task_id: &str,
    evidence_rowid_watermark: i64,
) -> Result<Vec<String>, RepositoryError> {
    connection
        .prepare(
            "SELECT id FROM task_evidence
             WHERE task_id = ?1 AND validity_status = 'valid'
               AND rowid > ?2
             ORDER BY rowid DESC LIMIT 100",
        )
        .map_err(RepositoryError::database)?
        .query_map(params![task_id, evidence_rowid_watermark], |row| row.get(0))
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_running_task_attempt(
    connection: &Connection,
    task_id: &str,
) -> Result<Option<TaskAttemptRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                    version, evidence_rowid_watermark, finding_rowid_watermark,
                    started_at, finished_at
             FROM task_attempts WHERE task_id = ?1 AND status = 'running'",
            [task_id],
            task_attempt_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

pub(super) fn load_task_attempt(
    connection: &Connection,
    id: &str,
) -> Result<Option<TaskAttemptRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                    version, evidence_rowid_watermark, finding_rowid_watermark,
                    started_at, finished_at
             FROM task_attempts WHERE id = ?1",
            [id],
            task_attempt_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn load_task_repair_override_collision(
    connection: &Connection,
    task_id: &str,
    attempt_id: &str,
    approval_id: &str,
    tool_call_id: &str,
) -> Result<Option<TaskRepairOverrideEventRecord>, RepositoryError> {
    let mut records = connection
        .prepare(
            "SELECT id, task_id, attempt_id, approval_id, tool_call_id, run_id,
                    conversation_id, policy_id, policy_hash, normal_repair_budget,
                    normal_repair_used, override_count, input_hash, escalation_reason,
                    created_at
             FROM task_repair_override_events
             WHERE task_id = ?1 OR attempt_id = ?2 OR approval_id = ?3 OR tool_call_id = ?4
             ORDER BY created_at, id LIMIT 2",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![task_id, attempt_id, approval_id, tool_call_id],
            task_repair_override_event_from_row,
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)?;
    if records.len() > 1 {
        return Err(RepositoryError::ConstraintViolation(
            "repair override identifiers collide with multiple immutable events".to_owned(),
        ));
    }
    Ok(records.pop())
}

fn load_task_repair_override_event(
    connection: &Connection,
    id: &str,
) -> Result<Option<TaskRepairOverrideEventRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, task_id, attempt_id, approval_id, tool_call_id, run_id,
                    conversation_id, policy_id, policy_hash, normal_repair_budget,
                    normal_repair_used, override_count, input_hash, escalation_reason,
                    created_at
             FROM task_repair_override_events WHERE id = ?1",
            [id],
            task_repair_override_event_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn task_repair_override_event_from_row(
    row: &Row<'_>,
) -> rusqlite::Result<TaskRepairOverrideEventRecord> {
    Ok(TaskRepairOverrideEventRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        attempt_id: row.get(2)?,
        approval_id: row.get(3)?,
        tool_call_id: row.get(4)?,
        run_id: row.get(5)?,
        conversation_id: row.get(6)?,
        policy_id: row.get(7)?,
        policy_hash: row.get(8)?,
        normal_repair_budget: row.get(9)?,
        normal_repair_used: row.get(10)?,
        override_count: row.get(11)?,
        input_hash: row.get(12)?,
        escalation_reason: row.get(13)?,
        created_at: row.get::<_, i64>(14)?.to_string(),
    })
}

fn load_bounded_task_attempts(
    connection: &Connection,
    task_ids: &[String],
    per_task_limit: usize,
) -> Result<Vec<TaskAttemptRecord>, RepositoryError> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let task_ids_json = serde_json::to_string(task_ids)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let global_limit = task_ids.len().saturating_mul(per_task_limit) as i64;
    connection
        .prepare(
            "WITH ranked_attempts AS (
                SELECT attempts.id, attempts.task_id, attempts.attempt_number, attempts.kind,
                       attempts.status, attempts.run_id, attempts.policy_hash,
                       attempts.root_cause, attempts.finding_ids_json,
                       attempts.evidence_ids_json, attempts.failure_reason, attempts.version,
                       attempts.evidence_rowid_watermark, attempts.finding_rowid_watermark,
                       attempts.started_at, attempts.finished_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY attempts.task_id
                           ORDER BY CASE WHEN attempts.status = 'running' THEN 0 ELSE 1 END,
                                    attempts.attempt_number DESC, attempts.id DESC
                       ) AS attempt_rank
                FROM task_attempts attempts
                WHERE attempts.task_id IN (SELECT value FROM json_each(?1))
             )
             SELECT id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                    version, evidence_rowid_watermark, finding_rowid_watermark,
                    started_at, finished_at
             FROM ranked_attempts WHERE attempt_rank <= ?2
             ORDER BY CASE WHEN status = 'running' THEN 0 ELSE 1 END,
                      task_id, attempt_number DESC, id DESC
             LIMIT ?3",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![task_ids_json, per_task_limit as i64, global_limit],
            task_attempt_from_row,
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn task_attempt_from_row(row: &Row<'_>) -> rusqlite::Result<TaskAttemptRecord> {
    Ok(TaskAttemptRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        attempt_number: row.get(2)?,
        kind: parse_task_attempt_kind(row, 3)?,
        status: parse_task_attempt_status(row, 4)?,
        run_id: row.get(5)?,
        policy_hash: row.get(6)?,
        root_cause: row.get(7)?,
        finding_ids: parse_string_array(row, 8)?,
        evidence_ids: parse_string_array(row, 9)?,
        failure_reason: row.get(10)?,
        version: row.get(11)?,
        evidence_rowid_watermark: row.get(12)?,
        finding_rowid_watermark: row.get(13)?,
        started_at: row.get::<_, i64>(14)?.to_string(),
        finished_at: row
            .get::<_, Option<i64>>(15)?
            .map(|value| value.to_string()),
    })
}

fn parse_task_attempt_kind(row: &Row<'_>, index: usize) -> rusqlite::Result<TaskAttemptKind> {
    match row.get::<_, String>(index)?.as_str() {
        "execution" => Ok(TaskAttemptKind::Execution),
        "repair" => Ok(TaskAttemptKind::Repair),
        value => Err(invalid_enum(index, "task attempt kind", value)),
    }
}

fn parse_task_attempt_status(row: &Row<'_>, index: usize) -> rusqlite::Result<TaskAttemptStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "running" => Ok(TaskAttemptStatus::Running),
        "succeeded" => Ok(TaskAttemptStatus::Succeeded),
        "failed" => Ok(TaskAttemptStatus::Failed),
        "blocked" => Ok(TaskAttemptStatus::Blocked),
        "cancelled" => Ok(TaskAttemptStatus::Cancelled),
        value => Err(invalid_enum(index, "task attempt status", value)),
    }
}

fn task_attempt_kind_text(kind: &TaskAttemptKind) -> &'static str {
    match kind {
        TaskAttemptKind::Execution => "execution",
        TaskAttemptKind::Repair => "repair",
    }
}

fn task_attempt_status_text(status: &TaskAttemptStatus) -> &'static str {
    match status {
        TaskAttemptStatus::Running => "running",
        TaskAttemptStatus::Succeeded => "succeeded",
        TaskAttemptStatus::Failed => "failed",
        TaskAttemptStatus::Blocked => "blocked",
        TaskAttemptStatus::Cancelled => "cancelled",
    }
}

fn load_goals(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<GoalRecord>, RepositoryError> {
    connection
        .prepare(
            "SELECT id, conversation_id, title, objective, acceptance_summary, status,
                    version, created_by, created_at, updated_at, completed_at, blocked_reason
             FROM goals WHERE conversation_id = ?1 ORDER BY updated_at DESC, id",
        )
        .map_err(RepositoryError::database)?
        .query_map([conversation_id], goal_from_row)
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn select_work_snapshot_goal(goals: &[GoalRecord]) -> Option<&GoalRecord> {
    goals
        .iter()
        .find(|goal| matches!(goal.status, GoalStatus::Active | GoalStatus::Blocked))
        .or_else(|| {
            goals
                .iter()
                .find(|goal| matches!(goal.status, GoalStatus::Proposed))
        })
}

fn load_tasks_for_goal(
    connection: &Connection,
    goal_id: &str,
) -> Result<Vec<WorkTaskRecord>, RepositoryError> {
    connection
        .prepare(
            "SELECT id, goal_id, parent_task_id, ordinal, title, detail, status, owner_run_id,
                    attempt, version, blocked_reason, created_at, updated_at, started_at, finished_at
             FROM work_tasks
             WHERE goal_id = ?1
             ORDER BY ordinal, id",
        )
        .map_err(RepositoryError::database)?
        .query_map([goal_id], task_from_row)
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_bounded_evidence_for_goal(
    connection: &Connection,
    goal_id: &str,
) -> Result<Vec<TaskEvidenceRecord>, RepositoryError> {
    connection
        .prepare(
            "WITH ranked_evidence AS (
                SELECT evidence.id, evidence.task_id, evidence.source_run_id,
                       evidence.evidence_type, evidence.ref_kind, evidence.ref_id,
                       evidence.summary, evidence.metadata_json, evidence.validity_status,
                       evidence.trace_id, evidence.span_id, evidence.checked_at,
                       evidence.invalid_reason, evidence.created_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY evidence.task_id
                           ORDER BY evidence.created_at DESC, evidence.id DESC
                       ) AS recent_rank,
                       ROW_NUMBER() OVER (
                           PARTITION BY evidence.task_id, evidence.validity_status
                           ORDER BY evidence.created_at DESC, evidence.id DESC
                       ) AS validity_rank
                FROM task_evidence evidence
                JOIN work_tasks tasks ON tasks.id = evidence.task_id
                WHERE tasks.goal_id = ?1
             )
             SELECT id, task_id, source_run_id, evidence_type, ref_kind, ref_id, summary,
                    metadata_json, validity_status, trace_id, span_id, checked_at,
                    invalid_reason, created_at
             FROM ranked_evidence
             WHERE recent_rank <= ?2 OR (validity_status = 'valid' AND validity_rank = 1)
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![goal_id, WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT as i64],
            evidence_from_row,
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_work_graph_facts(
    connection: &Connection,
    conversation_id: &str,
) -> Result<
    (
        Vec<GoalRecord>,
        Vec<WorkTaskRecord>,
        Vec<TaskEvidenceRecord>,
    ),
    RepositoryError,
> {
    let goals = load_goals(connection, conversation_id)?;

    let tasks = connection
        .prepare(
            "SELECT work_tasks.id, work_tasks.goal_id, work_tasks.parent_task_id,
                    work_tasks.ordinal, work_tasks.title, work_tasks.detail,
                    work_tasks.status, work_tasks.owner_run_id, work_tasks.attempt,
                    work_tasks.version, work_tasks.blocked_reason, work_tasks.created_at,
                    work_tasks.updated_at, work_tasks.started_at, work_tasks.finished_at
             FROM work_tasks
             JOIN goals ON goals.id = work_tasks.goal_id
             WHERE goals.conversation_id = ?1
             ORDER BY work_tasks.goal_id, work_tasks.ordinal, work_tasks.id",
        )
        .map_err(RepositoryError::database)?
        .query_map([conversation_id], task_from_row)
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)?;

    // Initial conversation loading follows the Evidence pagination contract:
    // retain only the latest 50 records for each Task in one bounded query.
    let evidence = connection
        .prepare(
            "WITH ranked_evidence AS (
                SELECT task_evidence.id, task_evidence.task_id,
                       task_evidence.source_run_id, task_evidence.evidence_type,
                       task_evidence.ref_kind, task_evidence.ref_id,
                       task_evidence.summary, task_evidence.metadata_json,
                       task_evidence.validity_status, task_evidence.trace_id,
                       task_evidence.span_id, task_evidence.checked_at,
                       task_evidence.invalid_reason, task_evidence.created_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY task_evidence.task_id
                           ORDER BY task_evidence.created_at DESC, task_evidence.id
                       ) AS evidence_rank
                FROM task_evidence
                JOIN work_tasks ON work_tasks.id = task_evidence.task_id
                JOIN goals ON goals.id = work_tasks.goal_id
                WHERE goals.conversation_id = ?1
             )
             SELECT id, task_id, source_run_id, evidence_type, ref_kind, ref_id,
                    summary, metadata_json, validity_status, trace_id, span_id,
                    checked_at, invalid_reason, created_at
             FROM ranked_evidence
             WHERE evidence_rank <= 50
             ORDER BY created_at DESC, id",
        )
        .map_err(RepositoryError::database)?
        .query_map([conversation_id], evidence_from_row)
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)?;

    Ok((goals, tasks, evidence))
}

fn validate_continuation_input(
    input: &AppendContinuationDecisionInput,
) -> Result<(), RepositoryError> {
    if input.schema_version != 1 {
        return Err(RepositoryError::InvalidInput(format!(
            "unsupported continuation decision schema version {}",
            input.schema_version
        )));
    }
    validate_non_empty("decision id", &input.decision_id)?;
    validate_non_empty("run id", &input.run_id)?;
    if input.event_cursor < 0 {
        return Err(RepositoryError::InvalidInput(
            "event cursor cannot be negative".to_owned(),
        ));
    }
    if input
        .next_action
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(RepositoryError::InvalidInput(
            "next action cannot be empty".to_owned(),
        ));
    }
    match (
        &input.host_validation_outcome,
        input.host_validation_error.as_deref(),
    ) {
        (ContinuationValidationOutcome::Accepted, None) => {}
        (ContinuationValidationOutcome::Rejected, Some(error)) if !error.trim().is_empty() => {}
        (ContinuationValidationOutcome::Accepted, Some(_)) => {
            return Err(RepositoryError::InvalidInput(
                "accepted continuation decision cannot include a validation error".to_owned(),
            ));
        }
        (ContinuationValidationOutcome::Rejected, _) => {
            return Err(RepositoryError::InvalidInput(
                "rejected continuation decision requires a validation error".to_owned(),
            ));
        }
    }
    validate_identifier_list("active task ids", &input.active_task_ids)?;
    validate_identifier_list("evidence ids", &input.evidence_ids)?;
    validate_identifier_list("missing acceptance", &input.missing_acceptance)?;
    validate_identifier_list(
        "blocked dependency references",
        &input.blocked_dependency_refs,
    )?;
    if input
        .expected_projection_hash
        .as_deref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(RepositoryError::InvalidInput(
            "expected projection hash must be a 64-character SHA-256 hex digest".to_owned(),
        ));
    }
    Ok(())
}

fn validate_continuation_ingest_diagnostic(
    input: &RecordContinuationIngestDiagnosticInput,
) -> Result<(), RepositoryError> {
    validate_bounded_text("run id", &input.run_id, 200)?;
    if input.event_seq < 0 {
        return Err(RepositoryError::InvalidInput(
            "continuation ingest diagnostic event sequence cannot be negative".to_owned(),
        ));
    }
    if let Some(execution_profile_id) = input.execution_profile_id.as_deref() {
        validate_bounded_text(
            "continuation ingest diagnostic execution profile id",
            execution_profile_id,
            200,
        )?;
    }
    validate_bounded_text(
        "continuation ingest diagnostic error code",
        &input.error_code,
        200,
    )?;
    validate_bounded_text(
        "continuation ingest diagnostic error message",
        &input.error_message,
        4096,
    )?;
    Ok(())
}

fn validate_bounded_text(
    field: &str,
    value: &str,
    max_characters: usize,
) -> Result<(), RepositoryError> {
    validate_non_empty(field, value)?;
    if value.chars().count() > max_characters {
        return Err(RepositoryError::InvalidInput(format!(
            "{field} cannot exceed {max_characters} characters"
        )));
    }
    Ok(())
}

fn validate_identifier_list(field: &str, values: &[String]) -> Result<(), RepositoryError> {
    let mut unique = HashSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(format!(
                "{field} cannot contain an empty value"
            )));
        }
        if !unique.insert(value) {
            return Err(RepositoryError::InvalidInput(format!(
                "{field} cannot contain duplicate value '{value}'"
            )));
        }
    }
    Ok(())
}

fn validate_continuation_associations(
    connection: &Connection,
    input: &AppendContinuationDecisionInput,
) -> Result<String, RepositoryError> {
    let run = connection
        .query_row(
            "SELECT conversation_id, last_seq FROM runs WHERE id = ?1",
            [input.run_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(RepositoryError::database)?
        .ok_or_else(|| not_found("run", &input.run_id))?;
    let (conversation_id, last_seq) = run;
    if input.event_cursor > last_seq {
        return Err(RepositoryError::InvalidInput(format!(
            "event cursor {} is ahead of run '{}' watermark {last_seq}",
            input.event_cursor, input.run_id
        )));
    }
    if input.event_cursor > 0 {
        let event_exists = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM run_events WHERE run_id = ?1 AND seq = ?2
                 )",
                params![input.run_id, input.event_cursor],
                |row| row.get::<_, bool>(0),
            )
            .map_err(RepositoryError::database)?;
        if !event_exists {
            return Err(RepositoryError::InvalidReference(format!(
                "run '{}' has no event at cursor {}",
                input.run_id, input.event_cursor
            )));
        }
    }
    let previous_cursor = connection
        .query_row(
            "SELECT MAX(event_cursor) FROM run_continuation_decisions WHERE run_id = ?1",
            [input.run_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(RepositoryError::database)?;
    if previous_cursor.is_some_and(|cursor| input.event_cursor < cursor) {
        return Err(RepositoryError::InvalidInput(
            "continuation decision event cursor cannot move backwards".to_owned(),
        ));
    }
    // Accepted decisions may only cite authoritative facts from the same conversation.
    // Rejected proposals keep their model-supplied identifiers for audit; the raw
    // runtime event remains the source for investigating an invalid reference.
    if matches!(
        input.host_validation_outcome,
        ContinuationValidationOutcome::Accepted
    ) {
        for task_id in &input.active_task_ids {
            let task_conversation = connection
                .query_row(
                    "SELECT goals.conversation_id
                     FROM work_tasks JOIN goals ON goals.id = work_tasks.goal_id
                     WHERE work_tasks.id = ?1",
                    [task_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| not_found("work task", task_id))?;
            ensure_same_conversation(&conversation_id, &task_conversation)?;
        }
        for evidence_id in &input.evidence_ids {
            let evidence_conversation = connection
                .query_row(
                    "SELECT goals.conversation_id
                     FROM task_evidence
                     JOIN work_tasks ON work_tasks.id = task_evidence.task_id
                     JOIN goals ON goals.id = work_tasks.goal_id
                     WHERE task_evidence.id = ?1",
                    [evidence_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| not_found("task evidence", evidence_id))?;
            ensure_same_conversation(&conversation_id, &evidence_conversation)?;
        }
    }
    Ok(conversation_id)
}

fn load_continuation_ingest_diagnostic(
    connection: &Connection,
    run_id: &str,
    event_seq: i64,
) -> Result<Option<ContinuationIngestDiagnosticRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT run_id, event_seq, execution_profile_id, shadow_mode,
                    error_code, error_message, created_at
             FROM run_continuation_ingest_diagnostics
             WHERE run_id = ?1 AND event_seq = ?2",
            params![run_id, event_seq],
            |row| {
                Ok(ContinuationIngestDiagnosticRecord {
                    run_id: row.get(0)?,
                    event_seq: row.get(1)?,
                    execution_profile_id: row.get(2)?,
                    shadow_mode: row.get(3)?,
                    error_code: row.get(4)?,
                    error_message: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn continuation_ingest_diagnostic_matches_input(
    record: &ContinuationIngestDiagnosticRecord,
    input: &RecordContinuationIngestDiagnosticInput,
) -> bool {
    record.run_id == input.run_id
        && record.event_seq == input.event_seq
        && record.execution_profile_id == input.execution_profile_id
        && record.shadow_mode == input.shadow_mode
        && record.error_code == input.error_code
        && record.error_message == input.error_message
}

fn load_continuation_decision_by_id(
    connection: &Connection,
    decision_id: &str,
) -> Result<Option<ContinuationDecisionRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT schema_version, decision_id, run_id, event_cursor, decision, reason_code,
                    active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                    next_action, retry_class, blocked_dependency_refs_json,
                    host_validation_outcome, host_validation_error, created_at
             FROM run_continuation_decisions WHERE decision_id = ?1",
            [decision_id],
            continuation_decision_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn continuation_decision_matches_input(
    record: &ContinuationDecisionRecord,
    input: &AppendContinuationDecisionInput,
) -> bool {
    record.schema_version == input.schema_version
        && record.decision_id == input.decision_id
        && record.run_id == input.run_id
        && record.event_cursor == input.event_cursor
        && record.decision == input.decision
        && record.reason_code == input.reason_code
        && record.active_task_ids == input.active_task_ids
        && record.evidence_ids == input.evidence_ids
        && record.missing_acceptance == input.missing_acceptance
        && record.next_action == input.next_action
        && record.retry_class == input.retry_class
        && record.blocked_dependency_refs == input.blocked_dependency_refs
        && record.host_validation_outcome == input.host_validation_outcome
        && record.host_validation_error == input.host_validation_error
}

fn load_continuation_decisions(
    connection: &Connection,
    run_id: &str,
) -> Result<Vec<ContinuationDecisionRecord>, RepositoryError> {
    connection
        .prepare(
            "SELECT schema_version, decision_id, run_id, event_cursor, decision, reason_code,
                    active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                    next_action, retry_class, blocked_dependency_refs_json,
                    host_validation_outcome, host_validation_error, created_at
             FROM run_continuation_decisions
             WHERE run_id = ?1
             ORDER BY event_cursor, created_at, decision_id",
        )
        .map_err(RepositoryError::database)?
        .query_map([run_id], continuation_decision_from_row)
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn continuation_decision_from_row(row: &Row<'_>) -> rusqlite::Result<ContinuationDecisionRecord> {
    let schema_version = row.get::<_, i64>(0)?;
    let schema_version = u32::try_from(schema_version).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(ContinuationDecisionRecord {
        schema_version,
        decision_id: row.get(1)?,
        run_id: row.get(2)?,
        event_cursor: row.get(3)?,
        decision: parse_continuation_decision(row, 4)?,
        reason_code: parse_continuation_reason(row, 5)?,
        active_task_ids: parse_string_array(row, 6)?,
        evidence_ids: parse_string_array(row, 7)?,
        missing_acceptance: parse_string_array(row, 8)?,
        next_action: row.get(9)?,
        retry_class: parse_optional_continuation_retry(row, 10)?,
        blocked_dependency_refs: parse_string_array(row, 11)?,
        host_validation_outcome: parse_continuation_validation(row, 12)?,
        host_validation_error: row.get(13)?,
        created_at: row.get(14)?,
    })
}

fn parse_string_array(row: &Row<'_>, index: usize) -> rusqlite::Result<Vec<String>> {
    let encoded = row.get::<_, String>(index)?;
    serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_continuation_decision(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<ContinuationDecisionKind> {
    match row.get::<_, String>(index)?.as_str() {
        "continue" => Ok(ContinuationDecisionKind::Continue),
        "repair" => Ok(ContinuationDecisionKind::Repair),
        "wait_approval" => Ok(ContinuationDecisionKind::WaitApproval),
        "complete" => Ok(ContinuationDecisionKind::Complete),
        "blocked" => Ok(ContinuationDecisionKind::Blocked),
        value => Err(invalid_enum(index, "continuation decision", value)),
    }
}

fn parse_continuation_reason(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<ContinuationReasonCode> {
    match row.get::<_, String>(index)?.as_str() {
        "work_remaining" => Ok(ContinuationReasonCode::WorkRemaining),
        "validation_failed" => Ok(ContinuationReasonCode::ValidationFailed),
        "approval_pending" => Ok(ContinuationReasonCode::ApprovalPending),
        "external_dependency_unavailable" => {
            Ok(ContinuationReasonCode::ExternalDependencyUnavailable)
        }
        "budget_exhausted" => Ok(ContinuationReasonCode::BudgetExhausted),
        "user_input_required" => Ok(ContinuationReasonCode::UserInputRequired),
        "retry_available" => Ok(ContinuationReasonCode::RetryAvailable),
        "retry_exhausted" => Ok(ContinuationReasonCode::RetryExhausted),
        "acceptance_missing" => Ok(ContinuationReasonCode::AcceptanceMissing),
        "acceptance_candidate" => Ok(ContinuationReasonCode::AcceptanceCandidate),
        "acceptance_passed" => Ok(ContinuationReasonCode::AcceptancePassed),
        "host_audit_required" => Ok(ContinuationReasonCode::HostAuditRequired),
        "tool_failed" => Ok(ContinuationReasonCode::ToolFailed),
        "task_interrupted" => Ok(ContinuationReasonCode::TaskInterrupted),
        value => Err(invalid_enum(index, "continuation reason code", value)),
    }
}

fn parse_optional_continuation_retry(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<ContinuationRetryClass>> {
    match row.get::<_, Option<String>>(index)?.as_deref() {
        None => Ok(None),
        Some("none") => Ok(Some(ContinuationRetryClass::None)),
        Some("recoverable") => Ok(Some(ContinuationRetryClass::Recoverable)),
        Some("retry_limited") => Ok(Some(ContinuationRetryClass::RetryLimited)),
        Some("non_retryable") => Ok(Some(ContinuationRetryClass::NonRetryable)),
        Some("host_decides") => Ok(Some(ContinuationRetryClass::HostDecides)),
        Some(value) => Err(invalid_enum(index, "continuation retry class", value)),
    }
}

fn parse_continuation_validation(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<ContinuationValidationOutcome> {
    match row.get::<_, String>(index)?.as_str() {
        "accepted" => Ok(ContinuationValidationOutcome::Accepted),
        "rejected" => Ok(ContinuationValidationOutcome::Rejected),
        value => Err(invalid_enum(
            index,
            "continuation validation outcome",
            value,
        )),
    }
}

fn continuation_decision_text(decision: &ContinuationDecisionKind) -> &'static str {
    match decision {
        ContinuationDecisionKind::Continue => "continue",
        ContinuationDecisionKind::Repair => "repair",
        ContinuationDecisionKind::WaitApproval => "wait_approval",
        ContinuationDecisionKind::Complete => "complete",
        ContinuationDecisionKind::Blocked => "blocked",
    }
}

fn continuation_reason_text(reason: &ContinuationReasonCode) -> &'static str {
    match reason {
        ContinuationReasonCode::WorkRemaining => "work_remaining",
        ContinuationReasonCode::ValidationFailed => "validation_failed",
        ContinuationReasonCode::ApprovalPending => "approval_pending",
        ContinuationReasonCode::ExternalDependencyUnavailable => "external_dependency_unavailable",
        ContinuationReasonCode::BudgetExhausted => "budget_exhausted",
        ContinuationReasonCode::UserInputRequired => "user_input_required",
        ContinuationReasonCode::RetryAvailable => "retry_available",
        ContinuationReasonCode::RetryExhausted => "retry_exhausted",
        ContinuationReasonCode::AcceptanceMissing => "acceptance_missing",
        ContinuationReasonCode::AcceptanceCandidate => "acceptance_candidate",
        ContinuationReasonCode::AcceptancePassed => "acceptance_passed",
        ContinuationReasonCode::HostAuditRequired => "host_audit_required",
        ContinuationReasonCode::ToolFailed => "tool_failed",
        ContinuationReasonCode::TaskInterrupted => "task_interrupted",
    }
}

fn continuation_retry_text(retry: &ContinuationRetryClass) -> &'static str {
    match retry {
        ContinuationRetryClass::None => "none",
        ContinuationRetryClass::Recoverable => "recoverable",
        ContinuationRetryClass::RetryLimited => "retry_limited",
        ContinuationRetryClass::NonRetryable => "non_retryable",
        ContinuationRetryClass::HostDecides => "host_decides",
    }
}

fn continuation_validation_text(outcome: &ContinuationValidationOutcome) -> &'static str {
    match outcome {
        ContinuationValidationOutcome::Accepted => "accepted",
        ContinuationValidationOutcome::Rejected => "rejected",
    }
}

#[derive(Clone)]
struct ProjectionAcceptanceFact {
    id: String,
    status: String,
    summary: String,
    checks: Value,
    created_at: String,
    resolved_at: Option<String>,
}

#[derive(Clone)]
struct ProjectionFindingFact {
    id: String,
    task_id: Option<String>,
    title: String,
    detail: String,
}

struct ProjectionApprovalFact {
    id: String,
    run_id: String,
    tool_call_id: String,
    requested_action: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionEvidenceValidityFact {
    id: String,
    task_id: String,
    validity_status: String,
    checked_at: Option<String>,
    invalid_reason: Option<String>,
}

struct ProjectionRunFact {
    id: String,
    status: String,
    last_seq: i64,
}

fn project_task_ledger(
    connection: &Connection,
    conversation_id: &str,
    generated_at: i64,
) -> Result<TaskLedgerProjection, RepositoryError> {
    ensure_conversation_exists(connection, conversation_id)?;
    let (goals, tasks, evidence) = load_work_graph_facts(connection, conversation_id)?;
    let selected_goal = select_work_snapshot_goal(&goals);
    project_task_ledger_from_facts(
        connection,
        conversation_id,
        generated_at,
        &goals,
        &tasks,
        &evidence,
        selected_goal,
    )
}

#[allow(clippy::too_many_arguments)]
fn project_task_ledger_from_facts(
    connection: &Connection,
    conversation_id: &str,
    generated_at: i64,
    goals: &[GoalRecord],
    tasks: &[WorkTaskRecord],
    evidence: &[TaskEvidenceRecord],
    selected_goal: Option<&GoalRecord>,
) -> Result<TaskLedgerProjection, RepositoryError> {
    let selected_goal_id = selected_goal.map(|goal| goal.id.as_str());
    let selected_tasks = tasks
        .iter()
        .filter(|task| selected_goal_id == Some(task.goal_id.as_str()))
        .collect::<Vec<_>>();
    let evidence_validity_facts = load_projection_evidence_validity(connection, selected_goal_id)?;

    let acceptances = load_projection_acceptances(connection, conversation_id, selected_goal_id)?;
    let findings = load_projection_findings(connection, conversation_id, selected_goal_id)?;
    let child_run_ids = load_projection_child_run_ids(connection, conversation_id)?;
    let runs = load_projection_runs(connection, conversation_id)?;
    let latest_run = runs.first();
    let approvals = load_projection_approvals(
        connection,
        conversation_id,
        latest_run.map(|run| run.id.as_str()),
    )?;
    let latest_host_accepted_decision = match latest_run {
        Some(run) => load_latest_host_accepted_decision(connection, &run.id)?,
        None => None,
    };

    let mut evidence_by_task: HashMap<&str, Vec<&TaskEvidenceRecord>> = HashMap::new();
    for record in evidence {
        evidence_by_task
            .entry(record.task_id.as_str())
            .or_default()
            .push(record);
    }
    let latest_acceptance = acceptances.first();
    let accepted_at = latest_acceptance
        .filter(|acceptance| acceptance.status == "accepted")
        .and_then(|acceptance| acceptance.resolved_at.clone());

    let active_tasks = selected_tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                WorkTaskStatus::Queued
                    | WorkTaskStatus::InProgress
                    | WorkTaskStatus::Blocked
                    | WorkTaskStatus::Interrupted
            )
        })
        .map(|task| {
            let task_evidence = evidence_by_task
                .get(task.id.as_str())
                .into_iter()
                .flat_map(|records| records.iter())
                .take(10)
                .map(|record| TaskLedgerEvidenceProjection {
                    id: record.id.clone(),
                    evidence_type: record.evidence_type.clone(),
                    validity_status: record.validity_status.clone(),
                    summary: record.summary.clone(),
                    created_at: record.created_at.clone(),
                })
                .collect();
            let mut blockers = task.blocked_reason.iter().cloned().collect::<Vec<_>>();
            blockers.extend(
                findings
                    .iter()
                    .filter(|finding| finding.task_id.as_deref() == Some(task.id.as_str()))
                    .map(|finding| finding.detail.clone()),
            );
            let acceptance_checks = acceptances
                .iter()
                .flat_map(|acceptance| {
                    acceptance_checks_for_task(&acceptance.checks, &task.id)
                        .into_iter()
                        .map(|check| TaskLedgerAcceptanceCheck {
                            acceptance_id: acceptance.id.clone(),
                            status: acceptance.status.clone(),
                            check,
                        })
                })
                .collect();
            let child_run_id = task
                .owner_run_id
                .as_ref()
                .filter(|run_id| child_run_ids.contains(run_id.as_str()))
                .cloned();
            TaskLedgerTaskProjection {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.clone(),
                attempt: task.attempt,
                acceptance_checks,
                latest_evidence: task_evidence,
                blockers,
                assigned_run_id: task.owner_run_id.clone(),
                child_run_id,
            }
        })
        .collect::<Vec<_>>();

    let completed_summary = selected_tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                WorkTaskStatus::Completed | WorkTaskStatus::Skipped
            )
        })
        .map(|task| {
            let evidence_ids = evidence_by_task
                .get(task.id.as_str())
                .into_iter()
                .flat_map(|records| records.iter())
                .filter(|record| matches!(record.validity_status, EvidenceValidityStatus::Valid))
                .map(|record| record.id.clone())
                .collect();
            let outcome = match task.status {
                WorkTaskStatus::Skipped => TaskLedgerCompletedOutcome::Skipped,
                WorkTaskStatus::Completed if accepted_at.is_some() => {
                    TaskLedgerCompletedOutcome::Accepted
                }
                WorkTaskStatus::Completed => TaskLedgerCompletedOutcome::Completed,
                _ => unreachable!("completed summary filters terminal tasks"),
            };
            TaskLedgerCompletedSummary {
                task_id: task.id.clone(),
                outcome,
                evidence_ids,
                accepted_at: accepted_at.clone(),
            }
        })
        .collect::<Vec<_>>();

    let mut pending_actions = approvals
        .into_iter()
        .map(|approval| TaskLedgerPendingAction {
            kind: TaskLedgerPendingActionKind::Approval,
            reference_id: approval.id,
            task_id: None,
            run_id: Some(approval.run_id),
            tool_call_id: Some(approval.tool_call_id),
            summary: approval.requested_action,
        })
        .collect::<Vec<_>>();
    pending_actions.extend(findings.iter().map(|finding| TaskLedgerPendingAction {
        kind: TaskLedgerPendingActionKind::Review,
        reference_id: finding.id.clone(),
        task_id: finding.task_id.clone(),
        run_id: None,
        tool_call_id: None,
        summary: finding.title.clone(),
    }));
    if let Some(acceptance) = latest_acceptance.filter(|record| record.status != "accepted") {
        pending_actions.push(TaskLedgerPendingAction {
            kind: TaskLedgerPendingActionKind::Review,
            reference_id: acceptance.id.clone(),
            task_id: None,
            run_id: None,
            tool_call_id: None,
            summary: acceptance.summary.clone(),
        });
    }
    pending_actions.extend(
        selected_tasks
            .iter()
            .filter(|task| matches!(task.status, WorkTaskStatus::Interrupted))
            .map(|task| TaskLedgerPendingAction {
                kind: TaskLedgerPendingActionKind::Retry,
                reference_id: task.id.clone(),
                task_id: Some(task.id.clone()),
                run_id: task.owner_run_id.clone(),
                tool_call_id: None,
                summary: task
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "task execution was interrupted".to_owned()),
            }),
    );
    let (last_event_id, event_range) = match latest_run {
        Some(run) => load_projection_event_watermark(connection, &run.id)?,
        None => (None, None),
    };
    let phase_goal = selected_goal.or_else(|| {
        goals
            .iter()
            .find(|goal| matches!(goal.status, GoalStatus::Completed | GoalStatus::Cancelled))
    });
    let current_phase = projection_phase(phase_goal, &active_tasks, &pending_actions, latest_run);
    let resumable_from = projection_resumable_from(&current_phase, &active_tasks, &pending_actions);
    let execution_cursor = TaskLedgerExecutionCursor {
        current_phase,
        run_id: latest_run.map(|run| run.id.clone()),
        event_cursor: latest_run.map_or(0, |run| run.last_seq),
        last_event_id,
        resumable_from,
    };

    let work_updated_at = goals
        .iter()
        .map(|goal| goal.updated_at.as_str())
        .chain(tasks.iter().map(|task| task.updated_at.as_str()))
        .max()
        .map(str::to_owned);
    let source_high_watermark = TaskLedgerSourceHighWatermark {
        run_cursors: runs
            .iter()
            .map(|run| TaskLedgerRunCursor {
                run_id: run.id.clone(),
                event_cursor: run.last_seq,
            })
            .collect(),
        work_updated_at,
        acceptance_created_at: acceptances
            .iter()
            .map(|acceptance| acceptance.created_at.as_str())
            .max()
            .map(str::to_owned),
    };
    let projection_meta = TaskLedgerProjectionMeta {
        schema_version: 1,
        projection_hash: String::new(),
        source_high_watermark,
        generated_at,
        derived_goal_ids: goals.iter().map(|goal| goal.id.clone()).collect(),
        derived_task_ids: tasks.iter().map(|task| task.id.clone()).collect(),
        derived_run_ids: runs.iter().map(|run| run.id.clone()).collect(),
        event_range,
    };

    let mut projection = TaskLedgerProjection {
        schema_version: 1,
        conversation_id: conversation_id.to_owned(),
        goal: selected_goal.map(|goal| TaskLedgerGoalProjection {
            id: goal.id.clone(),
            title: goal.title.clone(),
            status: goal.status.clone(),
            completion_policy: "legacy_v1".to_owned(),
        }),
        active_tasks,
        pending_actions,
        completed_summary,
        execution_cursor,
        projection_meta,
        latest_host_accepted_decision,
    };
    projection.projection_meta.projection_hash =
        projection_hash(&projection, &evidence_validity_facts)?;
    Ok(projection)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionHashMaterial<'a> {
    schema_version: u32,
    conversation_id: &'a str,
    goal: &'a Option<TaskLedgerGoalProjection>,
    active_tasks: &'a [TaskLedgerTaskProjection],
    pending_actions: &'a [TaskLedgerPendingAction],
    completed_summary: &'a [TaskLedgerCompletedSummary],
    execution_cursor: &'a TaskLedgerExecutionCursor,
    projection_schema_version: u32,
    source_high_watermark: &'a TaskLedgerSourceHighWatermark,
    derived_goal_ids: &'a [String],
    derived_task_ids: &'a [String],
    derived_run_ids: &'a [String],
    event_range: &'a Option<TaskLedgerEventRange>,
    evidence_validity: &'a [ProjectionEvidenceValidityFact],
}

fn projection_hash(
    projection: &TaskLedgerProjection,
    evidence_validity: &[ProjectionEvidenceValidityFact],
) -> Result<String, RepositoryError> {
    let material = ProjectionHashMaterial {
        schema_version: projection.schema_version,
        conversation_id: &projection.conversation_id,
        goal: &projection.goal,
        active_tasks: &projection.active_tasks,
        pending_actions: &projection.pending_actions,
        completed_summary: &projection.completed_summary,
        execution_cursor: &projection.execution_cursor,
        projection_schema_version: projection.projection_meta.schema_version,
        source_high_watermark: &projection.projection_meta.source_high_watermark,
        derived_goal_ids: &projection.projection_meta.derived_goal_ids,
        derived_task_ids: &projection.projection_meta.derived_task_ids,
        derived_run_ids: &projection.projection_meta.derived_run_ids,
        event_range: &projection.projection_meta.event_range,
        evidence_validity,
    };
    let encoded = serde_json::to_vec(&material)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn load_projection_evidence_validity(
    connection: &Connection,
    goal_id: Option<&str>,
) -> Result<Vec<ProjectionEvidenceValidityFact>, RepositoryError> {
    let Some(goal_id) = goal_id else {
        return Ok(Vec::new());
    };
    connection
        .prepare(
            "WITH ranked_evidence AS (
                SELECT evidence.id, evidence.task_id, evidence.validity_status,
                       evidence.checked_at, evidence.invalid_reason, evidence.created_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY evidence.task_id
                           ORDER BY evidence.created_at DESC, evidence.id DESC
                       ) AS recent_rank,
                       ROW_NUMBER() OVER (
                           PARTITION BY evidence.task_id, evidence.validity_status
                           ORDER BY evidence.created_at DESC, evidence.id DESC
                       ) AS validity_rank
                FROM task_evidence evidence
                JOIN work_tasks tasks ON tasks.id = evidence.task_id
                WHERE tasks.goal_id = ?1
             )
             SELECT id, task_id, validity_status, checked_at, invalid_reason
             FROM ranked_evidence
             WHERE recent_rank <= ?2 OR (validity_status = 'valid' AND validity_rank = 1)
             ORDER BY id",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![goal_id, WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT as i64],
            |row| {
                Ok(ProjectionEvidenceValidityFact {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    validity_status: row.get(2)?,
                    checked_at: row.get(3)?,
                    invalid_reason: row.get(4)?,
                })
            },
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_projection_acceptances(
    connection: &Connection,
    conversation_id: &str,
    goal_id: Option<&str>,
) -> Result<Vec<ProjectionAcceptanceFact>, RepositoryError> {
    let Some(goal_id) = goal_id else {
        return Ok(Vec::new());
    };
    connection
        .prepare(
            "SELECT id, status, summary, checks_json, created_at, resolved_at
             FROM acceptances
             WHERE conversation_id = ?1 AND goal_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![
                conversation_id,
                goal_id,
                WORK_SNAPSHOT_ACCEPTANCE_LIMIT as i64
            ],
            |row| {
                let encoded = row.get::<_, String>(3)?;
                let checks = serde_json::from_str(&encoded).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(ProjectionAcceptanceFact {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    summary: row.get(2)?,
                    checks,
                    created_at: row.get(4)?,
                    resolved_at: row.get(5)?,
                })
            },
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_projection_findings(
    connection: &Connection,
    conversation_id: &str,
    goal_id: Option<&str>,
) -> Result<Vec<ProjectionFindingFact>, RepositoryError> {
    let Some(goal_id) = goal_id else {
        return Ok(Vec::new());
    };
    connection
        .prepare(
            "SELECT id, task_id, title, detail
             FROM review_findings
             WHERE conversation_id = ?1 AND goal_id = ?2 AND status = 'open'
             ORDER BY created_at, id
             LIMIT ?3",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![
                conversation_id,
                goal_id,
                WORK_SNAPSHOT_LEDGER_PENDING_LIMIT as i64
            ],
            |row| {
                Ok(ProjectionFindingFact {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    title: row.get(2)?,
                    detail: row.get(3)?,
                })
            },
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_projection_approvals(
    connection: &Connection,
    conversation_id: &str,
    run_id: Option<&str>,
) -> Result<Vec<ProjectionApprovalFact>, RepositoryError> {
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    connection
        .prepare(
            "SELECT approvals.id, tool_calls.run_id, tool_calls.id,
                    approvals.requested_action
             FROM approvals
             JOIN tool_calls ON tool_calls.id = approvals.tool_call_id
             WHERE tool_calls.conversation_id = ?1 AND tool_calls.run_id = ?2
               AND approvals.status = 'pending'
             ORDER BY approvals.requested_at, approvals.id
             LIMIT ?3",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![
                conversation_id,
                run_id,
                WORK_SNAPSHOT_LEDGER_PENDING_LIMIT as i64
            ],
            |row| {
                Ok(ProjectionApprovalFact {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    tool_call_id: row.get(2)?,
                    requested_action: row.get(3)?,
                })
            },
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_projection_child_run_ids(
    connection: &Connection,
    conversation_id: &str,
) -> Result<HashSet<String>, RepositoryError> {
    connection
        .prepare(
            "SELECT child_run_delegations.child_run_id
             FROM child_run_delegations
             JOIN runs parent_runs ON parent_runs.id = child_run_delegations.parent_run_id
             WHERE parent_runs.conversation_id = ?1
             LIMIT ?2",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![conversation_id, WORK_SNAPSHOT_DERIVED_TASK_LIMIT as i64],
            |row| row.get::<_, String>(0),
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_projection_runs(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ProjectionRunFact>, RepositoryError> {
    connection
        .prepare(
            "SELECT id, status, last_seq
             FROM runs WHERE conversation_id = ?1
             ORDER BY created_at DESC, runs.rowid DESC
             LIMIT ?2",
        )
        .map_err(RepositoryError::database)?
        .query_map(
            params![conversation_id, WORK_SNAPSHOT_RUN_LIMIT as i64],
            |row| {
                Ok(ProjectionRunFact {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    last_seq: row.get(2)?,
                })
            },
        )
        .map_err(RepositoryError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RepositoryError::database)
}

fn load_latest_host_accepted_decision(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<ContinuationDecisionRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT decisions.schema_version, decisions.decision_id, decisions.run_id,
                    decisions.event_cursor, decisions.decision, decisions.reason_code,
                    decisions.active_task_ids_json, decisions.evidence_ids_json,
                    decisions.missing_acceptance_json, decisions.next_action,
                    decisions.retry_class, decisions.blocked_dependency_refs_json,
                    decisions.host_validation_outcome, decisions.host_validation_error,
                    decisions.created_at
             FROM run_continuation_decisions decisions
             WHERE decisions.run_id = ?1
               AND decisions.host_validation_outcome = 'accepted'
             ORDER BY decisions.event_cursor DESC, decisions.created_at DESC,
                      decisions.decision_id DESC
             LIMIT 1",
            [run_id],
            continuation_decision_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn load_projection_event_watermark(
    connection: &Connection,
    run_id: &str,
) -> Result<(Option<String>, Option<TaskLedgerEventRange>), RepositoryError> {
    let last_event_id = connection
        .query_row(
            "SELECT id FROM run_events WHERE run_id = ?1 ORDER BY seq DESC LIMIT 1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(RepositoryError::database)?;
    let (first, last) = connection
        .query_row(
            "SELECT MIN(seq), MAX(seq) FROM run_events WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(RepositoryError::database)?;
    Ok((
        last_event_id,
        first
            .zip(last)
            .map(|(first, last)| TaskLedgerEventRange { first, last }),
    ))
}

fn acceptance_checks_for_task(checks: &Value, task_id: &str) -> Vec<Value> {
    let values = checks
        .as_array()
        .cloned()
        .or_else(|| checks.get("checks").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    values
        .into_iter()
        .filter(|check| {
            check
                .get("taskId")
                .or_else(|| check.get("task_id"))
                .and_then(Value::as_str)
                == Some(task_id)
        })
        .collect()
}

fn projection_phase(
    goal: Option<&GoalRecord>,
    active_tasks: &[TaskLedgerTaskProjection],
    pending_actions: &[TaskLedgerPendingAction],
    latest_run: Option<&ProjectionRunFact>,
) -> TaskLedgerPhase {
    if pending_actions
        .iter()
        .any(|action| matches!(action.kind, TaskLedgerPendingActionKind::Approval))
    {
        return TaskLedgerPhase::WaitApproval;
    }
    if active_tasks
        .iter()
        .any(|task| matches!(task.status, WorkTaskStatus::InProgress))
    {
        return TaskLedgerPhase::Execute;
    }
    if active_tasks
        .iter()
        .any(|task| matches!(task.status, WorkTaskStatus::Interrupted))
    {
        return TaskLedgerPhase::Interrupted;
    }
    if active_tasks
        .iter()
        .any(|task| matches!(task.status, WorkTaskStatus::Blocked))
    {
        return TaskLedgerPhase::Blocked;
    }
    if active_tasks
        .iter()
        .any(|task| matches!(task.status, WorkTaskStatus::Queued))
    {
        return TaskLedgerPhase::Queued;
    }
    match goal.map(|goal| &goal.status) {
        Some(GoalStatus::Completed) => return TaskLedgerPhase::Complete,
        Some(GoalStatus::Blocked) => return TaskLedgerPhase::Blocked,
        Some(GoalStatus::Cancelled) => return TaskLedgerPhase::Cancelled,
        _ => {}
    }
    match latest_run.map(|run| run.status.as_str()) {
        Some("queued" | "awaiting_confirmation") => TaskLedgerPhase::Queued,
        Some("running" | "cancelling") => TaskLedgerPhase::Execute,
        Some("completed") => TaskLedgerPhase::Validate,
        Some("failed") => TaskLedgerPhase::Failed,
        Some("cancelled") => TaskLedgerPhase::Cancelled,
        Some("interrupted") => TaskLedgerPhase::Interrupted,
        _ => TaskLedgerPhase::Idle,
    }
}

fn projection_resumable_from(
    phase: &TaskLedgerPhase,
    active_tasks: &[TaskLedgerTaskProjection],
    pending_actions: &[TaskLedgerPendingAction],
) -> Option<String> {
    let pending_reference = |kind: TaskLedgerPendingActionKind| {
        pending_actions
            .iter()
            .find(|action| action.kind == kind)
            .map(|action| action.reference_id.clone())
    };
    let task_reference = |status: WorkTaskStatus| {
        active_tasks
            .iter()
            .find(|task| task.status == status)
            .map(|task| task.task_id.clone())
    };
    match phase {
        TaskLedgerPhase::WaitApproval => pending_reference(TaskLedgerPendingActionKind::Approval),
        TaskLedgerPhase::Execute => task_reference(WorkTaskStatus::InProgress),
        TaskLedgerPhase::Interrupted => task_reference(WorkTaskStatus::Interrupted),
        TaskLedgerPhase::Blocked => task_reference(WorkTaskStatus::Blocked),
        TaskLedgerPhase::Queued => task_reference(WorkTaskStatus::Queued),
        TaskLedgerPhase::Repair => pending_reference(TaskLedgerPendingActionKind::Repair),
        TaskLedgerPhase::Validate => pending_reference(TaskLedgerPendingActionKind::Review),
        TaskLedgerPhase::Idle
        | TaskLedgerPhase::Complete
        | TaskLedgerPhase::Failed
        | TaskLedgerPhase::Cancelled => None,
    }
}

fn bounded_task_ledger_for_snapshot(mut projection: TaskLedgerProjection) -> TaskLedgerProjection {
    let mut task_priority = Vec::new();
    if let Some(reference) = projection.execution_cursor.resumable_from.as_deref() {
        task_priority.push(reference.to_owned());
    }
    task_priority.extend(
        projection
            .pending_actions
            .iter()
            .filter_map(|action| action.task_id.clone()),
    );
    for status in [
        WorkTaskStatus::InProgress,
        WorkTaskStatus::Interrupted,
        WorkTaskStatus::Blocked,
        WorkTaskStatus::Queued,
    ] {
        task_priority.extend(
            projection
                .active_tasks
                .iter()
                .filter(|task| task.status == status)
                .map(|task| task.task_id.clone()),
        );
    }
    let selected_task_ids = take_unique_ids(task_priority, WORK_SNAPSHOT_LEDGER_ACTIVE_LIMIT);
    projection
        .active_tasks
        .retain(|task| selected_task_ids.iter().any(|id| id == &task.task_id));

    let mut action_priority = projection
        .pending_actions
        .iter()
        .filter(|action| matches!(action.kind, TaskLedgerPendingActionKind::Approval))
        .map(|action| action.reference_id.clone())
        .collect::<Vec<_>>();
    if let Some(reference) = projection.execution_cursor.resumable_from.as_deref() {
        action_priority.push(reference.to_owned());
    }
    for kind in [
        TaskLedgerPendingActionKind::Repair,
        TaskLedgerPendingActionKind::Review,
        TaskLedgerPendingActionKind::Retry,
        TaskLedgerPendingActionKind::ExternalWait,
    ] {
        action_priority.extend(
            projection
                .pending_actions
                .iter()
                .filter(|action| action.kind == kind)
                .map(|action| action.reference_id.clone()),
        );
    }
    let selected_action_ids = take_unique_ids(action_priority, WORK_SNAPSHOT_LEDGER_PENDING_LIMIT);
    projection.pending_actions.retain(|action| {
        selected_action_ids
            .iter()
            .any(|id| id == &action.reference_id)
    });

    if projection.completed_summary.len() > WORK_SNAPSHOT_LEDGER_COMPLETED_LIMIT {
        let keep_from = projection.completed_summary.len() - WORK_SNAPSHOT_LEDGER_COMPLETED_LIMIT;
        projection.completed_summary.drain(..keep_from);
    }
    projection
        .projection_meta
        .derived_goal_ids
        .truncate(WORK_SNAPSHOT_DERIVED_GOAL_LIMIT);
    let mut derived_task_priority = selected_task_ids;
    derived_task_priority.extend(
        projection
            .completed_summary
            .iter()
            .map(|summary| summary.task_id.clone()),
    );
    derived_task_priority.extend(projection.projection_meta.derived_task_ids.iter().cloned());
    projection.projection_meta.derived_task_ids =
        take_unique_ids(derived_task_priority, WORK_SNAPSHOT_DERIVED_TASK_LIMIT)
            .into_iter()
            .collect();
    projection
        .projection_meta
        .derived_run_ids
        .truncate(WORK_SNAPSHOT_RUN_LIMIT);
    projection
}

fn bounded_snapshot_tasks(
    tasks: &[WorkTaskRecord],
    ledger: &TaskLedgerProjection,
) -> Vec<WorkTaskRecord> {
    let mut priority = Vec::new();
    if let Some(reference) = ledger.execution_cursor.resumable_from.as_deref() {
        priority.push(reference.to_owned());
    }
    priority.extend(
        ledger
            .pending_actions
            .iter()
            .filter_map(|action| action.task_id.clone()),
    );
    for status in [
        WorkTaskStatus::InProgress,
        WorkTaskStatus::Interrupted,
        WorkTaskStatus::Blocked,
        WorkTaskStatus::Queued,
    ] {
        priority.extend(
            tasks
                .iter()
                .filter(|task| task.status == status)
                .map(|task| task.id.clone()),
        );
    }
    let mut historical = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                WorkTaskStatus::Completed | WorkTaskStatus::Skipped
            )
        })
        .collect::<Vec<_>>();
    historical.sort_by(|left, right| {
        right
            .finished_at
            .as_deref()
            .unwrap_or(right.updated_at.as_str())
            .cmp(
                left.finished_at
                    .as_deref()
                    .unwrap_or(left.updated_at.as_str()),
            )
            .then_with(|| right.id.cmp(&left.id))
    });
    priority.extend(historical.into_iter().map(|task| task.id.clone()));
    priority.extend(tasks.iter().map(|task| task.id.clone()));
    let selected = take_unique_ids(priority, WORK_SNAPSHOT_TASK_LIMIT);
    selected
        .into_iter()
        .filter_map(|id| tasks.iter().find(|task| task.id == id).cloned())
        .collect()
}

fn bounded_snapshot_evidence(
    tasks: &[WorkTaskRecord],
    evidence: &[TaskEvidenceRecord],
) -> Vec<TaskEvidenceRecord> {
    let mut selected = Vec::new();
    for task in tasks {
        let records = evidence
            .iter()
            .filter(|record| record.task_id == task.id)
            .collect::<Vec<_>>();
        if let Some(valid) = records
            .iter()
            .find(|record| record.validity_status == EvidenceValidityStatus::Valid)
        {
            selected.push((*valid).clone());
        }
        for record in records {
            if selected
                .iter()
                .rev()
                .take(WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT)
                .any(|existing| existing.id == record.id)
            {
                continue;
            }
            let selected_for_task = selected
                .iter()
                .filter(|existing| existing.task_id == task.id)
                .count();
            if selected_for_task >= WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT {
                break;
            }
            selected.push(record.clone());
        }
    }
    selected
}

fn take_unique_ids(ids: Vec<String>, limit: usize) -> Vec<String> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        if selected.len() >= limit {
            break;
        }
        if seen.insert(id.clone()) {
            selected.push(id);
        }
    }
    selected
}

fn empty_work_snapshot_history_summary() -> Value {
    serde_json::json!({
        "tasks": { "total": 0, "returned": 0 },
        "evidence": { "total": 0, "returned": 0 },
        "planRevisions": { "total": 0, "returned": 0 },
        "reviewFindings": { "total": 0, "open": 0, "returned": 0 },
        "acceptances": { "total": 0, "returned": 0 },
        "runs": { "total": 0, "returned": 0 }
    })
}

#[allow(clippy::too_many_arguments)]
fn load_work_snapshot_history_summary(
    connection: &Connection,
    goal_id: &str,
    returned_tasks: usize,
    returned_evidence: usize,
    returned_plans: usize,
    returned_findings: usize,
    returned_acceptances: usize,
    returned_runs: usize,
) -> Result<Value, RepositoryError> {
    let (tasks, evidence, plans, findings, open_findings, acceptances, runs) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM work_tasks WHERE goal_id = ?1),
                (SELECT COUNT(*) FROM task_evidence evidence
                   JOIN work_tasks tasks ON tasks.id = evidence.task_id
                  WHERE tasks.goal_id = ?1),
                (SELECT COUNT(*) FROM plan_revisions WHERE goal_id = ?1),
                (SELECT COUNT(*) FROM review_findings WHERE goal_id = ?1),
                (SELECT COUNT(*) FROM review_findings WHERE goal_id = ?1 AND status = 'open'),
                (SELECT COUNT(*) FROM acceptances WHERE goal_id = ?1),
                (SELECT COUNT(*) FROM runs runs
                   JOIN goals goals ON goals.conversation_id = runs.conversation_id
                  WHERE goals.id = ?1)",
            [goal_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .map_err(RepositoryError::database)?;
    Ok(serde_json::json!({
        "tasks": { "total": tasks, "returned": returned_tasks },
        "evidence": { "total": evidence, "returned": returned_evidence },
        "planRevisions": { "total": plans, "returned": returned_plans },
        "reviewFindings": {
            "total": findings,
            "open": open_findings,
            "returned": returned_findings
        },
        "acceptances": { "total": acceptances, "returned": returned_acceptances },
        "runs": { "total": runs, "returned": returned_runs }
    }))
}

#[derive(Debug)]
pub enum RepositoryError {
    NotFound {
        entity: &'static str,
        id: String,
    },
    InvalidInput(String),
    InvalidValidationPolicy(String),
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },
    OptimisticLockFailed {
        id: String,
        expected_version: i64,
    },
    ProjectionHashMismatch {
        expected: String,
        actual: String,
    },
    ConstraintViolation(String),
    EvidenceRequired {
        task_id: String,
    },
    ValidationChecksMissing {
        task_id: String,
        check_types: Vec<String>,
    },
    ReviewBlocked {
        task_id: String,
    },
    IndependentReviewRequired {
        task_id: String,
        implementation_run_id: String,
    },
    DuplicateEvidence,
    MissingReference(String),
    InvalidReference(String),
    CrossConversationReference,
    SqliteBusyExhausted {
        attempts: usize,
    },
    Database {
        message: String,
        busy: bool,
    },
}

impl RepositoryError {
    fn database(error: rusqlite::Error) -> Self {
        let busy = matches!(
            &error,
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        );
        Self::Database {
            message: error.to_string(),
            busy,
        }
    }

    fn is_busy(&self) -> bool {
        matches!(self, Self::Database { busy: true, .. })
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { entity, id } => write!(formatter, "{entity} '{id}' was not found"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::InvalidValidationPolicy(message) => write!(
                formatter,
                "[runtime.validation_policy.invalid_snapshot] {message}"
            ),
            Self::InvalidTransition { entity, from, to } => {
                write!(
                    formatter,
                    "invalid {entity} transition from '{from}' to '{to}'"
                )
            }
            Self::OptimisticLockFailed {
                id,
                expected_version,
            } => write!(
                formatter,
                "record '{id}' version no longer matches expected version {expected_version}"
            ),
            Self::ProjectionHashMismatch { expected, actual } => write!(
                formatter,
                "[continuation.projection_hash_mismatch] expected projection hash '{expected}', got '{actual}'"
            ),
            Self::ConstraintViolation(message) => formatter.write_str(message),
            Self::EvidenceRequired { task_id } => {
                write!(
                    formatter,
                    "work task '{task_id}' requires evidence before completion"
                )
            }
            Self::ValidationChecksMissing {
                task_id,
                check_types,
            } => write!(
                formatter,
                "Task '{task_id}' is missing required validated checks: {}",
                check_types.join(", ")
            ),
            Self::ReviewBlocked { task_id } => write!(
                formatter,
                "Task '{task_id}' has open critical/high/medium review findings"
            ),
            Self::IndependentReviewRequired {
                task_id,
                implementation_run_id,
            } => write!(
                formatter,
                "Task '{task_id}' requires independent review evidence from a run other than '{implementation_run_id}'"
            ),
            Self::DuplicateEvidence => formatter.write_str("duplicate task evidence"),
            Self::MissingReference(message) => formatter.write_str(message),
            Self::InvalidReference(message) => formatter.write_str(message),
            Self::CrossConversationReference => {
                formatter.write_str("evidence reference belongs to a different conversation")
            }
            Self::SqliteBusyExhausted { attempts } => write!(
                formatter,
                "SQLite remained busy after {attempts} bounded attempts"
            ),
            Self::Database { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<rusqlite::Error> for RepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::database(error)
    }
}

#[derive(Debug, Clone)]
pub struct CreateGoalInput {
    pub id: Option<String>,
    pub conversation_id: String,
    pub title: String,
    pub objective: String,
    pub acceptance_summary: Option<String>,
    pub status: GoalStatus,
    pub created_by: String,
}

#[derive(Debug, Clone)]
pub struct CreateTaskInput {
    pub id: Option<String>,
    pub goal_id: String,
    pub parent_task_id: Option<String>,
    pub ordinal: i64,
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AddEvidenceInput {
    pub id: Option<String>,
    pub task_id: String,
    pub source_run_id: Option<String>,
    pub evidence_type: EvidenceType,
    pub ref_kind: EvidenceReferenceKind,
    pub ref_id: String,
    pub summary: String,
    pub metadata: Value,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StartTaskAttemptInput {
    pub id: String,
    pub task_id: String,
    pub run_id: String,
    pub expected_task_version: i64,
    pub kind: TaskAttemptKind,
    pub root_cause: Option<String>,
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StartTaskRepairOverrideInput {
    pub task_id: String,
    pub attempt_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub tool_call_id: String,
    pub approval_id: String,
    pub expected_task_version: i64,
    pub root_cause: String,
    pub finding_ids: Vec<String>,
    pub escalation_reason: String,
}

#[derive(Debug, Clone)]
pub struct PreflightTaskRepairOverrideInput {
    pub task_id: String,
    pub attempt_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub expected_task_version: i64,
    pub root_cause: String,
    pub finding_ids: Vec<String>,
    pub escalation_reason: String,
}

#[derive(Debug, Clone)]
pub struct FinishTaskAttemptInput {
    pub id: String,
    pub task_id: String,
    pub run_id: String,
    pub expected_task_version: i64,
    pub expected_attempt_version: i64,
    pub status: TaskAttemptStatus,
    pub failure_reason: Option<String>,
}

#[derive(Clone)]
pub struct GoalRepository {
    database: Database,
}

#[allow(dead_code)]
impl GoalRepository {
    pub(super) fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn create(&self, input: CreateGoalInput) -> Result<GoalRecord, RepositoryError> {
        validate_non_empty("conversation_id", &input.conversation_id)?;
        validate_non_empty("title", &input.title)?;
        validate_non_empty("objective", &input.objective)?;
        validate_non_empty("created_by", &input.created_by)?;
        if !matches!(input.status, GoalStatus::Proposed | GoalStatus::Active) {
            return Err(RepositoryError::InvalidInput(
                "a goal must be created as proposed or active".to_owned(),
            ));
        }
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        validate_non_empty("id", &id)?;
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            require_exists(transaction, "conversations", &input.conversation_id)?;
            if input.status == GoalStatus::Active {
                ensure_no_active_goal(transaction, &input.conversation_id, None)?;
            }
            transaction
                .execute(
                    "INSERT INTO goals(
                        id, conversation_id, title, objective, acceptance_summary, status,
                        version, created_by, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?8)",
                    params![
                        id,
                        input.conversation_id,
                        input.title,
                        input.objective,
                        input.acceptance_summary,
                        goal_status_text(&input.status),
                        input.created_by,
                        now,
                    ],
                )
                .map_err(RepositoryError::database)?;
            load_goal(transaction, &id)?.ok_or_else(|| not_found("goal", &id))
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<GoalRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| load_goal(connection, id))
    }

    pub fn get_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<GoalRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, conversation_id, title, objective, acceptance_summary, status,
                            version, created_by, created_at, updated_at, completed_at, blocked_reason
                     FROM goals WHERE conversation_id = ?1 ORDER BY updated_at DESC, id",
                )
                .map_err(RepositoryError::database)?;
            let rows = statement
                .query_map([conversation_id], goal_from_row)
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)?;
            Ok(rows)
        })
    }

    pub fn get_active(&self, conversation_id: &str) -> Result<Option<GoalRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, title, objective, acceptance_summary, status,
                            version, created_by, created_at, updated_at, completed_at, blocked_reason
                     FROM goals
                     WHERE conversation_id = ?1 AND status IN ('active', 'blocked')",
                    [conversation_id],
                    goal_from_row,
                )
                .optional()
                .map_err(RepositoryError::database)
        })
    }

    pub fn delete(&self, id: &str, conversation_id: &str) -> Result<bool, RepositoryError> {
        validate_non_empty("id", id)?;
        validate_non_empty("conversation_id", conversation_id)?;
        with_write_transaction(&self.database, |transaction| {
            let deleted = transaction
                .execute(
                    "DELETE FROM goals WHERE id = ?1 AND conversation_id = ?2",
                    params![id, conversation_id],
                )
                .map_err(RepositoryError::database)?;
            Ok(deleted > 0)
        })
    }

    pub fn refine_host_placeholder(
        &self,
        id: &str,
        title: String,
        objective: String,
        acceptance_summary: Option<String>,
    ) -> Result<Option<GoalRecord>, RepositoryError> {
        validate_non_empty("id", id)?;
        validate_non_empty("title", &title)?;
        validate_non_empty("objective", &objective)?;
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let current = load_goal(transaction, id)?.ok_or_else(|| not_found("goal", id))?;
            if current.created_by != "host:work-mode-gate"
                || !matches!(current.status, GoalStatus::Proposed | GoalStatus::Active)
            {
                return Ok(None);
            }
            let task_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM work_tasks WHERE goal_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .map_err(RepositoryError::database)?;
            if task_count > 0 {
                return Ok(None);
            }
            transaction
                .execute(
                    "UPDATE goals
                     SET title = ?2, objective = ?3, acceptance_summary = ?4,
                         version = version + 1, updated_at = ?5
                     WHERE id = ?1",
                    params![id, title, objective, acceptance_summary, now],
                )
                .map_err(RepositoryError::database)?;
            load_goal(transaction, id)
        })
    }

    pub fn activate(&self, id: &str, expected_version: i64) -> Result<GoalRecord, RepositoryError> {
        self.transition(id, expected_version, GoalStatus::Active, None)
    }

    pub fn block(
        &self,
        id: &str,
        reason: String,
        expected_version: i64,
    ) -> Result<GoalRecord, RepositoryError> {
        validate_non_empty("blocked reason", &reason)?;
        self.transition(id, expected_version, GoalStatus::Blocked, Some(reason))
    }

    pub fn complete(&self, id: &str, expected_version: i64) -> Result<GoalRecord, RepositoryError> {
        self.transition(id, expected_version, GoalStatus::Completed, None)
    }

    pub fn cancel(&self, id: &str, expected_version: i64) -> Result<GoalRecord, RepositoryError> {
        self.transition(id, expected_version, GoalStatus::Cancelled, None)
    }

    fn transition(
        &self,
        id: &str,
        expected_version: i64,
        next_status: GoalStatus,
        blocked_reason: Option<String>,
    ) -> Result<GoalRecord, RepositoryError> {
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let current = load_goal(transaction, id)?.ok_or_else(|| not_found("goal", id))?;
            if current.version != expected_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: id.to_owned(),
                    expected_version,
                });
            }
            if !valid_goal_transition(&current.status, &next_status) {
                return Err(RepositoryError::InvalidTransition {
                    entity: "goal",
                    from: goal_status_text(&current.status).to_owned(),
                    to: goal_status_text(&next_status).to_owned(),
                });
            }
            if next_status == GoalStatus::Active {
                ensure_no_active_goal(transaction, &current.conversation_id, Some(id))?;
            }
            if next_status == GoalStatus::Completed {
                let incomplete: i64 = transaction
                    .query_row(
                        "SELECT COUNT(*) FROM work_tasks
                         WHERE goal_id = ?1 AND status NOT IN ('completed', 'skipped')",
                        [id],
                        |row| row.get(0),
                    )
                    .map_err(RepositoryError::database)?;
                if incomplete > 0 {
                    return Err(RepositoryError::ConstraintViolation(
                        "a goal cannot complete while required tasks remain unfinished".to_owned(),
                    ));
                }
                let completed_without_valid_evidence: i64 = transaction
                    .query_row(
                        "SELECT COUNT(*) FROM work_tasks task
                         WHERE task.goal_id = ?1 AND task.status = 'completed'
                           AND NOT EXISTS(
                               SELECT 1 FROM task_evidence evidence
                               WHERE evidence.task_id = task.id
                                 AND evidence.validity_status = 'valid'
                           )",
                        [id],
                        |row| row.get(0),
                    )
                    .map_err(RepositoryError::database)?;
                if completed_without_valid_evidence > 0 {
                    return Err(RepositoryError::ConstraintViolation(
                        "a goal cannot complete until every completed task has valid evidence"
                            .to_owned(),
                    ));
                }
            }
            let changed = transaction
                .execute(
                    "UPDATE goals
                     SET status = ?3, version = version + 1, updated_at = ?4,
                         completed_at = CASE WHEN ?3 = 'completed' THEN ?4 ELSE NULL END,
                         blocked_reason = CASE WHEN ?3 = 'blocked' THEN ?5 ELSE NULL END
                     WHERE id = ?1 AND version = ?2",
                    params![
                        id,
                        expected_version,
                        goal_status_text(&next_status),
                        now,
                        blocked_reason,
                    ],
                )
                .map_err(RepositoryError::database)?;
            if changed == 0 {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: id.to_owned(),
                    expected_version,
                });
            }
            if next_status == GoalStatus::Cancelled {
                transaction
                    .execute(
                        "UPDATE work_tasks
                         SET status = 'interrupted', updated_at = ?2, finished_at = ?2
                         WHERE goal_id = ?1 AND status = 'in_progress'",
                        params![id, now],
                    )
                    .map_err(RepositoryError::database)?;
            }
            load_goal(transaction, id)?.ok_or_else(|| not_found("goal", id))
        })
    }
}

#[derive(Clone)]
pub struct WorkTaskRepository {
    database: Database,
}

#[allow(dead_code)]
impl WorkTaskRepository {
    pub(super) fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn create(&self, input: CreateTaskInput) -> Result<WorkTaskRecord, RepositoryError> {
        let mut created = self.create_many(vec![input])?;
        Ok(created.remove(0))
    }

    pub fn create_many(
        &self,
        inputs: Vec<CreateTaskInput>,
    ) -> Result<Vec<WorkTaskRecord>, RepositoryError> {
        let policy_ids = vec!["legacy_v1".to_owned(); inputs.len()];
        self.create_many_with_policy_ids(inputs, policy_ids)
    }

    pub fn create_many_with_policy_ids(
        &self,
        inputs: Vec<CreateTaskInput>,
        policy_ids: Vec<String>,
    ) -> Result<Vec<WorkTaskRecord>, RepositoryError> {
        if inputs.len() != policy_ids.len() {
            return Err(RepositoryError::InvalidInput(
                "every created task requires exactly one Host-selected ValidationPolicy".to_owned(),
            ));
        }
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let prepared = inputs
            .into_iter()
            .zip(policy_ids)
            .map(|(input, policy_id)| {
                validate_non_empty("goal_id", &input.goal_id)?;
                validate_non_empty("title", &input.title)?;
                if input.ordinal < 0 {
                    return Err(RepositoryError::InvalidInput(
                        "task ordinal cannot be negative".to_owned(),
                    ));
                }
                let policy = validation_policy_snapshot(&policy_id)?;
                Ok((
                    input
                        .id
                        .clone()
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    input,
                    policy,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let mut created = Vec::with_capacity(prepared.len());
            for (id, input, policy) in &prepared {
                require_exists(transaction, "goals", &input.goal_id)?;
                if let Some(parent_id) = input.parent_task_id.as_deref() {
                    let parent_goal: Option<String> = transaction
                        .query_row(
                            "SELECT goal_id FROM work_tasks WHERE id = ?1",
                            [parent_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(RepositoryError::database)?;
                    match parent_goal {
                        None => return Err(not_found("parent work task", parent_id)),
                        Some(goal_id) if goal_id != input.goal_id => {
                            return Err(RepositoryError::ConstraintViolation(
                                "a parent task must belong to the same goal".to_owned(),
                            ));
                        }
                        Some(_) => {}
                    }
                }
                let ordinal_taken: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM work_tasks WHERE goal_id = ?1 AND ordinal = ?2)",
                        params![input.goal_id, input.ordinal],
                        |row| row.get(0),
                    )
                    .map_err(RepositoryError::database)?;
                if ordinal_taken {
                    return Err(RepositoryError::ConstraintViolation(format!(
                        "task ordinal {} already exists for goal '{}'",
                        input.ordinal, input.goal_id
                    )));
                }
                transaction
                    .execute(
                        "INSERT INTO work_tasks(
                            id, goal_id, parent_task_id, ordinal, title, detail, status,
                            attempt, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', 1, ?7, ?7)",
                        params![
                            id,
                            input.goal_id,
                            input.parent_task_id,
                            input.ordinal,
                            input.title,
                            input.detail,
                            now,
                        ],
                    )
                    .map_err(RepositoryError::database)?;
                let snapshot_json = serde_json::to_string(policy)
                    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO task_validation_policies(
                            task_id, schema_version, policy_id, snapshot_json, policy_hash, frozen_at
                         ) VALUES (?1, 1, ?2, ?3, ?4, ?5)",
                        params![id, policy.id, snapshot_json, policy.hash, now],
                    )
                    .map_err(RepositoryError::database)?;
                created
                    .push(load_task(transaction, id)?.ok_or_else(|| not_found("work task", id))?);
            }
            Ok(created)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<WorkTaskRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| load_task(connection, id))
    }

    pub fn list_by_goal(&self, goal_id: &str) -> Result<Vec<WorkTaskRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, goal_id, parent_task_id, ordinal, title, detail, status,
                            owner_run_id, attempt, version, blocked_reason, created_at, updated_at,
                            started_at, finished_at
                     FROM work_tasks WHERE goal_id = ?1 ORDER BY ordinal, id",
                )
                .map_err(RepositoryError::database)?;
            let records = statement
                .query_map([goal_id], task_from_row)
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)?;
            Ok(records)
        })
    }

    pub fn start(&self, id: &str, owner_run_id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        validate_non_empty("owner_run_id", owner_run_id)?;
        self.transition(
            id,
            WorkTaskStatus::InProgress,
            Some(owner_run_id),
            None,
            false,
            None,
        )
    }

    pub fn complete(&self, id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        self.transition(id, WorkTaskStatus::Completed, None, None, false, None)
    }

    pub fn block(&self, id: &str, reason: String) -> Result<WorkTaskRecord, RepositoryError> {
        validate_non_empty("blocked reason", &reason)?;
        self.transition(id, WorkTaskStatus::Blocked, None, Some(reason), false, None)
    }

    pub fn interrupt(&self, id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        self.transition(id, WorkTaskStatus::Interrupted, None, None, false, None)
    }

    pub fn skip(&self, id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        self.transition(id, WorkTaskStatus::Skipped, None, None, false, None)
    }

    pub fn retry(&self, id: &str, owner_run_id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        validate_non_empty("owner_run_id", owner_run_id)?;
        self.transition(
            id,
            WorkTaskStatus::InProgress,
            Some(owner_run_id),
            None,
            true,
            None,
        )
    }

    pub fn requeue(&self, id: &str) -> Result<WorkTaskRecord, RepositoryError> {
        self.transition(id, WorkTaskStatus::Queued, None, None, false, None)
    }

    pub fn update(
        &self,
        id: &str,
        expected_version: i64,
        next_status: WorkTaskStatus,
        owner_run_id: Option<&str>,
        blocked_reason: Option<String>,
    ) -> Result<WorkTaskRecord, RepositoryError> {
        if expected_version < 1 {
            return Err(RepositoryError::InvalidInput(
                "expected task version must be at least 1".to_owned(),
            ));
        }
        if next_status == WorkTaskStatus::InProgress && owner_run_id.is_none() {
            return Err(RepositoryError::InvalidInput(
                "an in-progress task requires the current run".to_owned(),
            ));
        }
        if next_status == WorkTaskStatus::Blocked
            && blocked_reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(RepositoryError::InvalidInput(
                "blocked reason cannot be empty".to_owned(),
            ));
        }
        let retry = self
            .get(id)?
            .is_some_and(|task| task.status == WorkTaskStatus::Interrupted)
            && next_status == WorkTaskStatus::InProgress;
        self.transition(
            id,
            next_status,
            owner_run_id,
            blocked_reason,
            retry,
            Some(expected_version),
        )
    }

    fn transition(
        &self,
        id: &str,
        next_status: WorkTaskStatus,
        owner_run_id: Option<&str>,
        blocked_reason: Option<String>,
        retry: bool,
        expected_version: Option<i64>,
    ) -> Result<WorkTaskRecord, RepositoryError> {
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let current = load_task(transaction, id)?.ok_or_else(|| not_found("work task", id))?;
            if expected_version.is_some_and(|version| version != current.version) {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: id.to_owned(),
                    expected_version: expected_version.unwrap_or_default(),
                });
            }
            let valid = if retry {
                current.status == WorkTaskStatus::Interrupted
                    && next_status == WorkTaskStatus::InProgress
            } else {
                valid_task_transition(&current.status, &next_status)
            };
            if !valid {
                return Err(RepositoryError::InvalidTransition {
                    entity: "work task",
                    from: task_status_text(&current.status).to_owned(),
                    to: task_status_text(&next_status).to_owned(),
                });
            }
            let policy = load_task_validation_policy(transaction, id)?;
            if policy.snapshot.id != "legacy_v1"
                && matches!(
                    next_status,
                    WorkTaskStatus::InProgress
                        | WorkTaskStatus::Completed
                        | WorkTaskStatus::Blocked
                        | WorkTaskStatus::Interrupted
                        | WorkTaskStatus::Skipped
                )
            {
                return Err(RepositoryError::ConstraintViolation(
                    "durable Task execution state requires the Attempt CAS API".to_owned(),
                ));
            }
            if next_status == WorkTaskStatus::Completed {
                let evidence_count: i64 = transaction
                    .query_row(
                        "SELECT COUNT(*) FROM task_evidence WHERE task_id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .map_err(RepositoryError::database)?;
                if evidence_count == 0 {
                    return Err(RepositoryError::EvidenceRequired {
                        task_id: id.to_owned(),
                    });
                }
            }
            if let Some(run_id) = owner_run_id {
                ensure_run_matches_task_conversation(transaction, run_id, id)?;
            }
            if next_status == WorkTaskStatus::InProgress {
                let another_in_progress: bool = transaction
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM work_tasks
                            WHERE goal_id = ?1 AND status = 'in_progress' AND id <> ?2
                         )",
                        params![current.goal_id, id],
                        |row| row.get(0),
                    )
                    .map_err(RepositoryError::database)?;
                if another_in_progress {
                    return Err(RepositoryError::ConstraintViolation(
                        "a goal can have only one in-progress task".to_owned(),
                    ));
                }
            }
            let changed = transaction
                .execute(
                    "UPDATE work_tasks
                     SET status = ?3,
                         owner_run_id = CASE
                             WHEN ?3 = 'in_progress' THEN COALESCE(?4, owner_run_id)
                             WHEN ?3 = 'queued' THEN NULL
                             ELSE owner_run_id
                         END,
                         attempt = attempt + ?5,
                         version = version + 1,
                         blocked_reason = CASE WHEN ?3 = 'blocked' THEN ?6 ELSE NULL END,
                         started_at = CASE
                             WHEN ?3 = 'in_progress' THEN COALESCE(started_at, ?7)
                             WHEN ?3 = 'queued' THEN NULL
                             ELSE started_at
                         END,
                         finished_at = CASE WHEN ?3 IN ('completed', 'interrupted', 'skipped') THEN ?7 ELSE NULL END,
                         updated_at = ?7
                     WHERE id = ?1 AND status = ?2 AND (?8 IS NULL OR version = ?8)",
                    params![
                        id,
                        task_status_text(&current.status),
                        task_status_text(&next_status),
                        owner_run_id,
                        i64::from(retry),
                        blocked_reason,
                        now,
                        expected_version,
                    ],
                )
                .map_err(RepositoryError::database)?;
            if changed == 0 {
                return Err(RepositoryError::ConstraintViolation(
                    "work task status changed concurrently".to_owned(),
                ));
            }
            load_task(transaction, id)?.ok_or_else(|| not_found("work task", id))
        })
    }
}

#[derive(Clone)]
pub struct TaskEvidenceRepository {
    database: Database,
}

#[allow(dead_code)]
impl TaskEvidenceRepository {
    pub(super) fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn add(&self, input: AddEvidenceInput) -> Result<TaskEvidenceRecord, RepositoryError> {
        validate_non_empty("task_id", &input.task_id)?;
        validate_non_empty("ref_id", &input.ref_id)?;
        validate_non_empty("summary", &input.summary)?;
        let metadata_json = serde_json::to_string(&input.metadata)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            validate_reference(
                transaction,
                &input.task_id,
                &input.evidence_type,
                &input.ref_kind,
                &input.ref_id,
                input.trace_id.as_deref(),
                input.span_id.as_deref(),
            )?;
            if let Some(source_run_id) = input.source_run_id.as_deref() {
                ensure_run_matches_task_conversation(transaction, source_run_id, &input.task_id)?;
            }
            let duplicate: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM task_evidence
                        WHERE task_id = ?1 AND evidence_type = ?2 AND ref_kind = ?3 AND ref_id = ?4
                     )",
                    params![
                        input.task_id,
                        evidence_type_text(&input.evidence_type),
                        reference_kind_text(&input.ref_kind),
                        input.ref_id,
                    ],
                    |row| row.get(0),
                )
                .map_err(RepositoryError::database)?;
            if duplicate {
                return Err(RepositoryError::DuplicateEvidence);
            }
            transaction
                .execute(
                    "INSERT INTO task_evidence(
                        id, task_id, source_run_id, evidence_type, ref_kind, ref_id, summary,
                        metadata_json, validity_status, trace_id, span_id, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'unverified', ?9, ?10, ?11)",
                    params![
                        id,
                        input.task_id,
                        input.source_run_id,
                        evidence_type_text(&input.evidence_type),
                        reference_kind_text(&input.ref_kind),
                        input.ref_id,
                        input.summary,
                        metadata_json,
                        input.trace_id,
                        input.span_id,
                        now,
                    ],
                )
                .map_err(RepositoryError::database)?;
            load_evidence(transaction, &id)?.ok_or_else(|| not_found("task evidence", &id))
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<TaskEvidenceRecord>, RepositoryError> {
        with_read_connection(&self.database, |connection| load_evidence(connection, id))
    }

    pub fn list_by_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskEvidenceRecord>, RepositoryError> {
        let limit = limit.clamp(1, 50) as i64;
        with_read_connection(&self.database, |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, task_id, source_run_id, evidence_type, ref_kind, ref_id,
                            summary, metadata_json, validity_status, trace_id, span_id,
                            checked_at, invalid_reason, created_at
                     FROM task_evidence WHERE task_id = ?1 ORDER BY created_at DESC, id LIMIT ?2",
                )
                .map_err(RepositoryError::database)?;
            let records = statement
                .query_map(params![task_id, limit], evidence_from_row)
                .map_err(RepositoryError::database)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(RepositoryError::database)?;
            Ok(records)
        })
    }

    pub fn validate(&self, id: &str) -> Result<EvidenceValidityStatus, RepositoryError> {
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let evidence =
                load_evidence(transaction, id)?.ok_or_else(|| not_found("task evidence", id))?;
            let validation = validate_reference(
                transaction,
                &evidence.task_id,
                &evidence.evidence_type,
                &evidence.ref_kind,
                &evidence.ref_id,
                evidence.trace_id.as_deref(),
                evidence.span_id.as_deref(),
            )
            .and_then(|()| {
                let Some(check_type) = evidence
                    .metadata
                    .get("validationCheckType")
                    .and_then(Value::as_str)
                else {
                    return Ok(());
                };
                validate_evidence_check_semantics(
                    transaction,
                    check_type,
                    evidence_type_text(&evidence.evidence_type),
                    reference_kind_text(&evidence.ref_kind),
                    &evidence.ref_id,
                    evidence.source_run_id.as_deref(),
                    &evidence.metadata,
                )
            });
            let (status, reason) = match validation {
                Ok(()) => (EvidenceValidityStatus::Valid, None),
                Err(RepositoryError::MissingReference(reason)) => {
                    (EvidenceValidityStatus::Missing, Some(reason))
                }
                Err(error @ RepositoryError::Database { .. }) => return Err(error),
                Err(error) => (EvidenceValidityStatus::Invalid, Some(error.to_string())),
            };
            update_evidence_validity(transaction, id, &status, reason.as_deref(), &now)?;
            Ok(status)
        })
    }

    pub fn mark_stale(&self, id: &str, reason: String) -> Result<(), RepositoryError> {
        self.mark(id, EvidenceValidityStatus::Stale, reason)
    }

    pub fn mark_missing(&self, id: &str, reason: String) -> Result<(), RepositoryError> {
        self.mark(id, EvidenceValidityStatus::Missing, reason)
    }

    pub fn mark_invalid(&self, id: &str, reason: String) -> Result<(), RepositoryError> {
        self.mark(id, EvidenceValidityStatus::Invalid, reason)
    }

    fn mark(
        &self,
        id: &str,
        status: EvidenceValidityStatus,
        reason: String,
    ) -> Result<(), RepositoryError> {
        validate_non_empty("validity reason", &reason)?;
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            update_evidence_validity(transaction, id, &status, Some(&reason), &now)
        })
    }
}

fn with_read_connection<T>(
    database: &Database,
    operation: impl FnOnce(&Connection) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| RepositoryError::Database {
            message: "database lock is poisoned".to_owned(),
            busy: false,
        })?;
    operation(&connection)
}

fn with_read_transaction<T>(
    database: &Database,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| RepositoryError::Database {
            message: "database lock is poisoned".to_owned(),
            busy: false,
        })?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(RepositoryError::database)?;
    let value = operation(&transaction)?;
    transaction.commit().map_err(RepositoryError::database)?;
    Ok(value)
}

pub(super) fn with_write_transaction<T>(
    database: &Database,
    mut operation: impl FnMut(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    for attempt in 0..MAX_BUSY_ATTEMPTS {
        let result = {
            let mut connection =
                database
                    .connection
                    .lock()
                    .map_err(|_| RepositoryError::Database {
                        message: "database lock is poisoned".to_owned(),
                        busy: false,
                    })?;
            connection
                .busy_timeout(Duration::ZERO)
                .map_err(RepositoryError::database)?;
            let transaction_result =
                match connection.transaction_with_behavior(TransactionBehavior::Immediate) {
                    Ok(transaction) => match operation(&transaction) {
                        Ok(value) => transaction
                            .commit()
                            .map(|_| value)
                            .map_err(RepositoryError::database),
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(RepositoryError::database(error)),
                };
            let restore_result = connection
                .busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))
                .map_err(RepositoryError::database);
            match (transaction_result, restore_result) {
                (result @ Err(_), _) => result,
                (Ok(_), Err(error)) => Err(error),
                (Ok(value), Ok(())) => Ok(value),
            }
        };
        match result {
            Err(error) if error.is_busy() && attempt + 1 < MAX_BUSY_ATTEMPTS => {
                thread::sleep(Duration::from_millis(
                    INITIAL_BUSY_DELAY_MS * (1_u64 << attempt),
                ));
            }
            Err(error) if error.is_busy() => {
                return Err(RepositoryError::SqliteBusyExhausted {
                    attempts: MAX_BUSY_ATTEMPTS,
                });
            }
            other => return other,
        }
    }
    unreachable!("bounded retry loop always returns")
}

pub(super) fn validate_reference(
    connection: &Connection,
    task_id: &str,
    evidence_type: &EvidenceType,
    ref_kind: &EvidenceReferenceKind,
    ref_id: &str,
    expected_trace_id: Option<&str>,
    expected_span_id: Option<&str>,
) -> Result<(), RepositoryError> {
    validate_evidence_pair(evidence_type, ref_kind)?;
    let conversation_id = task_conversation(connection, task_id)?;
    match ref_kind {
        EvidenceReferenceKind::ToolCall => {
            let target: Option<(String, String)> = connection
                .query_row(
                    "SELECT conversation_id, status FROM tool_calls WHERE id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, status) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("tool call '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if status != "completed" {
                return Err(RepositoryError::InvalidReference(format!(
                    "tool call '{ref_id}' did not complete successfully (status '{status}')"
                )));
            }
        }
        EvidenceReferenceKind::Artifact => {
            let target: Option<(String, String, String)> = connection
                .query_row(
                    "SELECT conversation_id, status, storage_path FROM artifacts WHERE id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, status, storage_path) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("artifact '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if status != "ready" {
                return Err(RepositoryError::InvalidReference(format!(
                    "artifact '{ref_id}' is not accessible"
                )));
            }
            match std::fs::metadata(Path::new(&storage_path)) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(RepositoryError::InvalidReference(format!(
                        "artifact '{ref_id}' does not reference a file"
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(RepositoryError::MissingReference(format!(
                        "artifact file '{}' was not found",
                        storage_path
                    )));
                }
                Err(error) => {
                    return Err(RepositoryError::InvalidReference(format!(
                        "artifact '{ref_id}' is not accessible: {error}"
                    )));
                }
            }
        }
        EvidenceReferenceKind::RunEvent => {
            let target: Option<(String, Option<String>, Option<String>)> = connection
                .query_row(
                    "SELECT runs.conversation_id, run_events.trace_id, run_events.span_id
                     FROM run_events JOIN runs ON runs.id = run_events.run_id
                     WHERE run_events.id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, trace_id, span_id) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("run event '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if matches!(evidence_type, EvidenceType::TraceSpan) && span_id.is_none() {
                return Err(RepositoryError::InvalidReference(format!(
                    "run event '{ref_id}' has no span"
                )));
            }
            if expected_trace_id.is_some_and(|expected| trace_id.as_deref() != Some(expected)) {
                return Err(RepositoryError::InvalidReference(format!(
                    "run event '{ref_id}' trace does not match the evidence"
                )));
            }
            if expected_span_id.is_some_and(|expected| span_id.as_deref() != Some(expected)) {
                return Err(RepositoryError::InvalidReference(format!(
                    "run event '{ref_id}' span does not match the evidence"
                )));
            }
        }
        EvidenceReferenceKind::Message => {
            let target: Option<(String, String)> = connection
                .query_row(
                    "SELECT conversation_id, role FROM messages WHERE id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, role) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("message '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if role != "user" {
                return Err(RepositoryError::InvalidReference(
                    "user confirmation evidence must reference a user message".to_owned(),
                ));
            }
        }
        EvidenceReferenceKind::Source => {
            let parsed = url::Url::parse(ref_id).map_err(|_| {
                RepositoryError::InvalidReference(
                    "external source evidence must use a stable HTTP(S) URL".to_owned(),
                )
            })?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(RepositoryError::InvalidReference(
                    "external source evidence must use a stable HTTP(S) URL".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_evidence_pair(
    evidence_type: &EvidenceType,
    ref_kind: &EvidenceReferenceKind,
) -> Result<(), RepositoryError> {
    let valid = matches!(
        (evidence_type, ref_kind),
        (EvidenceType::ToolCall, EvidenceReferenceKind::ToolCall)
            | (EvidenceType::TraceSpan, EvidenceReferenceKind::RunEvent)
            | (EvidenceType::TestResult, EvidenceReferenceKind::ToolCall)
            | (EvidenceType::TestResult, EvidenceReferenceKind::Artifact)
            | (EvidenceType::FileDiff, EvidenceReferenceKind::Artifact)
            | (EvidenceType::FileDiff, EvidenceReferenceKind::RunEvent)
            | (EvidenceType::Artifact, EvidenceReferenceKind::Artifact)
            | (
                EvidenceType::UserConfirmation,
                EvidenceReferenceKind::Message
            )
            | (
                EvidenceType::ExternalReference,
                EvidenceReferenceKind::Source
            )
    );
    if valid {
        Ok(())
    } else {
        Err(RepositoryError::InvalidReference(format!(
            "evidence type '{}' cannot reference '{}'",
            evidence_type_text(evidence_type),
            reference_kind_text(ref_kind)
        )))
    }
}

pub(super) fn validate_evidence_check_semantics(
    connection: &Connection,
    check_type: &str,
    evidence_type: &str,
    ref_kind: &str,
    ref_id: &str,
    source_run_id: Option<&str>,
    metadata: &Value,
) -> Result<(), RepositoryError> {
    let valid_shape = match check_type {
        "test" => {
            evidence_type == "test_result"
                && ref_kind == "tool_call"
                && tool_call_reports_pass(connection, ref_id)?
        }
        "inspection" => !matches!(evidence_type, "external_reference" | "user_confirmation"),
        "review" => {
            source_run_id.is_some()
                && !matches!(evidence_type, "external_reference" | "user_confirmation")
        }
        "manual" => matches!(
            evidence_type,
            "user_confirmation" | "external_reference" | "artifact" | "tool_call"
        ),
        "other" => true,
        other => {
            return Err(RepositoryError::InvalidReference(format!(
                "unsupported validationCheckType '{other}'"
            )));
        }
    };
    if !valid_shape {
        return Err(RepositoryError::InvalidReference(format!(
            "validation check '{check_type}' cannot be proven by evidence type '{evidence_type}' referencing '{ref_kind}'"
        )));
    }
    if evidence_type == "external_reference" && !matches!(check_type, "manual" | "other") {
        return Err(RepositoryError::InvalidReference(
            "ordinary external-source Evidence may satisfy only manual or other checks".to_owned(),
        ));
    }
    if check_type == "test"
        && metadata
            .get("passed")
            .and_then(Value::as_bool)
            .is_some_and(|passed| !passed)
    {
        return Err(RepositoryError::InvalidReference(
            "test Evidence metadata reports failure".to_owned(),
        ));
    }
    Ok(())
}

fn tool_call_reports_pass(
    connection: &Connection,
    tool_call_id: &str,
) -> Result<bool, RepositoryError> {
    let record = connection
        .query_row(
            "SELECT status, result_json, error_message FROM tool_calls WHERE id = ?1",
            [tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(RepositoryError::database)?;
    let Some((status, result_json, error_message)) = record else {
        return Ok(false);
    };
    if status != "completed"
        || error_message
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    {
        return Ok(false);
    }
    let Some(result_json) = result_json else {
        return Ok(false);
    };
    let result = serde_json::from_str::<Value>(&result_json).map_err(|error| {
        RepositoryError::InvalidReference(format!(
            "test ToolCall result is not structured JSON: {error}"
        ))
    })?;
    Ok(structured_result_reports_pass(&result))
}

fn structured_result_reports_pass(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("passed").and_then(Value::as_bool) == Some(true)
        || object.get("success").and_then(Value::as_bool) == Some(true)
        || object.get("ok").and_then(Value::as_bool) == Some(true)
        || object.get("exitCode").and_then(Value::as_i64) == Some(0)
        || object.get("exit_code").and_then(Value::as_i64) == Some(0)
        || object
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| {
                matches!(
                    status.to_ascii_lowercase().as_str(),
                    "passed" | "success" | "succeeded" | "ok"
                )
            })
    {
        return true;
    }
    ["result", "details", "summary", "data"]
        .iter()
        .filter_map(|key| object.get(*key))
        .any(structured_result_reports_pass)
}

fn ensure_run_matches_task_conversation(
    connection: &Connection,
    run_id: &str,
    task_id: &str,
) -> Result<(), RepositoryError> {
    let task_conversation = task_conversation(connection, task_id)?;
    let run_conversation: Option<String> = connection
        .query_row(
            "SELECT conversation_id FROM runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(RepositoryError::database)?;
    let run_conversation = run_conversation.ok_or_else(|| {
        RepositoryError::MissingReference(format!("run '{run_id}' was not found"))
    })?;
    ensure_same_conversation(&task_conversation, &run_conversation)
}

fn require_durable_override_run_profile(
    connection: &Connection,
    run_id: &str,
    conversation_id: &str,
) -> Result<(), RepositoryError> {
    let row = connection
        .query_row(
            "SELECT runs.conversation_id, runs.status, profiles.schema_version,
                    profiles.profile_id, profiles.snapshot_json, profiles.profile_hash
             FROM runs
             LEFT JOIN run_execution_profiles profiles ON profiles.run_id = runs.id
             WHERE runs.id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(RepositoryError::database)?
        .ok_or_else(|| not_found("run", run_id))?;
    if row.0 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if row.1 != "running" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "repair override Run '{run_id}' is not running"
        )));
    }
    let (Some(schema_version), Some(profile_id), Some(snapshot_json), Some(profile_hash)) =
        (row.2, row.3, row.4, row.5)
    else {
        return Err(RepositoryError::InvalidValidationPolicy(
            "repair override requires an explicit Host-frozen durable_v2 Run profile".to_owned(),
        ));
    };
    let (canonical_json, canonical_hash) = canonical_run_execution_profile(&profile_id)?;
    if schema_version != 1
        || profile_id != "durable_v2"
        || snapshot_json != canonical_json
        || profile_hash != canonical_hash
    {
        return Err(RepositoryError::InvalidValidationPolicy(
            "repair override is restricted to an untampered Host-frozen durable_v2 Run".to_owned(),
        ));
    }
    Ok(())
}

fn task_conversation(connection: &Connection, task_id: &str) -> Result<String, RepositoryError> {
    connection
        .query_row(
            "SELECT goals.conversation_id
             FROM work_tasks JOIN goals ON goals.id = work_tasks.goal_id
             WHERE work_tasks.id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(RepositoryError::database)?
        .ok_or_else(|| not_found("work task", task_id))
}

fn ensure_same_conversation(expected: &str, actual: &str) -> Result<(), RepositoryError> {
    if expected == actual {
        Ok(())
    } else {
        Err(RepositoryError::CrossConversationReference)
    }
}

fn update_evidence_validity(
    transaction: &Transaction<'_>,
    id: &str,
    status: &EvidenceValidityStatus,
    reason: Option<&str>,
    checked_at: &str,
) -> Result<(), RepositoryError> {
    let changed = transaction
        .execute(
            "UPDATE task_evidence
             SET validity_status = ?2, checked_at = ?3, invalid_reason = ?4
             WHERE id = ?1",
            params![id, validity_status_text(status), checked_at, reason],
        )
        .map_err(RepositoryError::database)?;
    if changed == 0 {
        Err(not_found("task evidence", id))
    } else {
        Ok(())
    }
}

fn ensure_no_active_goal(
    connection: &Connection,
    conversation_id: &str,
    except_id: Option<&str>,
) -> Result<(), RepositoryError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM goals
                WHERE conversation_id = ?1 AND status IN ('active', 'blocked')
                  AND (?2 IS NULL OR id <> ?2)
             )",
            params![conversation_id, except_id],
            |row| row.get(0),
        )
        .map_err(RepositoryError::database)?;
    if exists {
        Err(RepositoryError::ConstraintViolation(
            "a conversation can have only one active or blocked goal".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn require_exists(
    connection: &Connection,
    table: &'static str,
    id: &str,
) -> Result<(), RepositoryError> {
    let sql = match table {
        "conversations" => "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
        "runs" => "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
        "goals" => "SELECT EXISTS(SELECT 1 FROM goals WHERE id = ?1)",
        "work_tasks" => "SELECT EXISTS(SELECT 1 FROM work_tasks WHERE id = ?1)",
        _ => unreachable!("table names are not caller-controlled"),
    };
    let exists: bool = connection
        .query_row(sql, [id], |row| row.get(0))
        .map_err(RepositoryError::database)?;
    if exists {
        Ok(())
    } else {
        Err(not_found(table.trim_end_matches('s'), id))
    }
}

fn valid_goal_transition(current: &GoalStatus, next: &GoalStatus) -> bool {
    matches!(
        (current, next),
        (GoalStatus::Proposed, GoalStatus::Active)
            | (GoalStatus::Proposed, GoalStatus::Cancelled)
            | (GoalStatus::Active, GoalStatus::Blocked)
            | (GoalStatus::Blocked, GoalStatus::Active)
            | (GoalStatus::Active, GoalStatus::Completed)
            | (GoalStatus::Active, GoalStatus::Cancelled)
            | (GoalStatus::Blocked, GoalStatus::Cancelled)
    )
}

fn valid_task_transition(current: &WorkTaskStatus, next: &WorkTaskStatus) -> bool {
    matches!(
        (current, next),
        (WorkTaskStatus::Queued, WorkTaskStatus::InProgress)
            | (WorkTaskStatus::InProgress, WorkTaskStatus::Completed)
            | (WorkTaskStatus::InProgress, WorkTaskStatus::Blocked)
            | (WorkTaskStatus::InProgress, WorkTaskStatus::Interrupted)
            | (WorkTaskStatus::Blocked, WorkTaskStatus::InProgress)
            | (WorkTaskStatus::Queued, WorkTaskStatus::Skipped)
            | (WorkTaskStatus::Blocked, WorkTaskStatus::Skipped)
            | (WorkTaskStatus::Completed, WorkTaskStatus::Queued)
    )
}

fn load_goal(connection: &Connection, id: &str) -> Result<Option<GoalRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, conversation_id, title, objective, acceptance_summary, status,
                    version, created_by, created_at, updated_at, completed_at, blocked_reason
             FROM goals WHERE id = ?1",
            [id],
            goal_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

pub(super) fn load_task(
    connection: &Connection,
    id: &str,
) -> Result<Option<WorkTaskRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, goal_id, parent_task_id, ordinal, title, detail, status,
                    owner_run_id, attempt, version, blocked_reason, created_at, updated_at,
                    started_at, finished_at
             FROM work_tasks WHERE id = ?1",
            [id],
            task_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

pub(super) fn load_evidence(
    connection: &Connection,
    id: &str,
) -> Result<Option<TaskEvidenceRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, task_id, source_run_id, evidence_type, ref_kind, ref_id,
                    summary, metadata_json, validity_status, trace_id, span_id,
                    checked_at, invalid_reason, created_at
             FROM task_evidence WHERE id = ?1",
            [id],
            evidence_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn load_acceptance(
    connection: &Connection,
    id: &str,
) -> Result<Option<AcceptanceRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, goal_id, plan_revision_id, conversation_id, status, summary,
                    checks_json, reviewer, created_at, resolved_at
             FROM acceptances WHERE id = ?1",
            [id],
            acceptance_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn load_accepted_acceptance(
    connection: &Connection,
    goal_id: &str,
) -> Result<Option<AcceptanceRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, goal_id, plan_revision_id, conversation_id, status, summary,
                    checks_json, reviewer, created_at, resolved_at
             FROM acceptances
             WHERE goal_id = ?1 AND status = 'accepted'
             ORDER BY rowid DESC LIMIT 1",
            [goal_id],
            acceptance_from_row,
        )
        .optional()
        .map_err(RepositoryError::database)
}

fn acceptance_from_row(row: &Row<'_>) -> rusqlite::Result<AcceptanceRecord> {
    let checks_json = row.get::<_, String>(6)?;
    Ok(AcceptanceRecord {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        plan_revision_id: row.get(2)?,
        conversation_id: row.get(3)?,
        status: row.get(4)?,
        summary: row.get(5)?,
        checks: serde_json::from_str(&checks_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        reviewer: row.get(7)?,
        created_at: row.get(8)?,
        resolved_at: row.get(9)?,
    })
}

fn validate_atomic_acceptance_checks(
    connection: &Connection,
    goal_id: &str,
    reviewer_run_id: &str,
    checks: &Value,
    task_ids: &[String],
    completed_task_ids: &[String],
) -> Result<(), RepositoryError> {
    let criteria = checks
        .get("criteria")
        .and_then(Value::as_array)
        .filter(|criteria| !criteria.is_empty())
        .ok_or_else(|| {
            RepositoryError::InvalidInput(
                "atomic Acceptance requires non-empty normalized criteria".to_owned(),
            )
        })?;
    if checks
        .get("reviewer")
        .and_then(|reviewer| reviewer.get("runId"))
        .and_then(Value::as_str)
        != Some(reviewer_run_id)
    {
        return Err(RepositoryError::InvalidInput(
            "Acceptance checks reviewer does not match the submitting Run".to_owned(),
        ));
    }
    let task_ids = task_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut covered_task_ids = HashSet::new();
    for criterion in criteria {
        let status = criterion
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RepositoryError::InvalidInput(
                    "Acceptance criterion omitted normalized status".to_owned(),
                )
            })?;
        if !["passed", "not_applicable"].contains(&status) {
            return Err(RepositoryError::InvalidInput(format!(
                "Acceptance criterion has unsupported status '{status}'"
            )));
        }
        let evidence_ids = criterion
            .get("evidenceIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RepositoryError::InvalidInput(
                    "Acceptance criterion omitted normalized evidenceIds".to_owned(),
                )
            })?;
        if status == "passed" && evidence_ids.is_empty() {
            return Err(RepositoryError::EvidenceRequired {
                task_id: format!("goal:{goal_id}"),
            });
        }
        for evidence_id in evidence_ids {
            let evidence_id = evidence_id.as_str().ok_or_else(|| {
                RepositoryError::InvalidInput(
                    "Acceptance evidenceIds must contain strings".to_owned(),
                )
            })?;
            let evidence_task_id = connection
                .query_row(
                    "SELECT task_id FROM task_evidence
                     WHERE id = ?1 AND validity_status = 'valid'",
                    [evidence_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(RepositoryError::database)?
                .ok_or_else(|| {
                    RepositoryError::InvalidReference(format!(
                        "Acceptance Evidence '{evidence_id}' is missing or not Host-valid"
                    ))
                })?;
            if !task_ids.contains(evidence_task_id.as_str()) {
                return Err(RepositoryError::CrossConversationReference);
            }
            covered_task_ids.insert(evidence_task_id);
        }
    }
    let uncovered = completed_task_ids
        .iter()
        .filter(|task_id| !covered_task_ids.contains(task_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !uncovered.is_empty() {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Acceptance Evidence does not cover completed Tasks: {}",
            uncovered.join(", ")
        )));
    }

    let task_summary = checks
        .get("taskSummary")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            RepositoryError::InvalidInput(
                "atomic Acceptance requires normalized taskSummary".to_owned(),
            )
        })?;
    let valid_evidence = connection
        .query_row(
            "SELECT COUNT(*) FROM task_evidence evidence
             JOIN work_tasks task ON task.id = evidence.task_id
             WHERE task.goal_id = ?1 AND evidence.validity_status = 'valid'",
            [goal_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(RepositoryError::database)? as u64;
    let expected = [
        ("taskCount", task_ids.len() as u64),
        ("completed", completed_task_ids.len() as u64),
        (
            "skipped",
            (task_ids.len() - completed_task_ids.len()) as u64,
        ),
        ("validEvidence", valid_evidence),
    ];
    for (field, expected) in expected {
        if task_summary.get(field).and_then(Value::as_u64) != Some(expected) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Acceptance taskSummary.{field} is stale"
            )));
        }
    }
    Ok(())
}

fn goal_from_row(row: &Row<'_>) -> rusqlite::Result<GoalRecord> {
    Ok(GoalRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        title: row.get(2)?,
        objective: row.get(3)?,
        acceptance_summary: row.get(4)?,
        status: parse_goal_status(row, 5)?,
        version: row.get(6)?,
        created_by: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        completed_at: row.get(10)?,
        blocked_reason: row.get(11)?,
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<WorkTaskRecord> {
    Ok(WorkTaskRecord {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        parent_task_id: row.get(2)?,
        ordinal: row.get(3)?,
        title: row.get(4)?,
        detail: row.get(5)?,
        status: parse_task_status(row, 6)?,
        owner_run_id: row.get(7)?,
        attempt: row.get(8)?,
        version: row.get(9)?,
        blocked_reason: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        started_at: row.get(13)?,
        finished_at: row.get(14)?,
    })
}

fn evidence_from_row(row: &Row<'_>) -> rusqlite::Result<TaskEvidenceRecord> {
    let metadata_json: String = row.get(7)?;
    let metadata = serde_json::from_str(&metadata_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(TaskEvidenceRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        source_run_id: row.get(2)?,
        evidence_type: parse_evidence_type(row, 3)?,
        ref_kind: parse_reference_kind(row, 4)?,
        ref_id: row.get(5)?,
        summary: row.get(6)?,
        metadata,
        validity_status: parse_validity_status(row, 8)?,
        trace_id: row.get(9)?,
        span_id: row.get(10)?,
        checked_at: row.get(11)?,
        invalid_reason: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn parse_goal_status(row: &Row<'_>, index: usize) -> rusqlite::Result<GoalStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "proposed" => Ok(GoalStatus::Proposed),
        "active" => Ok(GoalStatus::Active),
        "blocked" => Ok(GoalStatus::Blocked),
        "completed" => Ok(GoalStatus::Completed),
        "cancelled" => Ok(GoalStatus::Cancelled),
        value => Err(invalid_enum(index, "goal status", value)),
    }
}

fn parse_task_status(row: &Row<'_>, index: usize) -> rusqlite::Result<WorkTaskStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "queued" => Ok(WorkTaskStatus::Queued),
        "in_progress" => Ok(WorkTaskStatus::InProgress),
        "completed" => Ok(WorkTaskStatus::Completed),
        "blocked" => Ok(WorkTaskStatus::Blocked),
        "interrupted" => Ok(WorkTaskStatus::Interrupted),
        "skipped" => Ok(WorkTaskStatus::Skipped),
        value => Err(invalid_enum(index, "work task status", value)),
    }
}

fn parse_evidence_type(row: &Row<'_>, index: usize) -> rusqlite::Result<EvidenceType> {
    match row.get::<_, String>(index)?.as_str() {
        "tool_call" => Ok(EvidenceType::ToolCall),
        "trace_span" => Ok(EvidenceType::TraceSpan),
        "test_result" => Ok(EvidenceType::TestResult),
        "file_diff" => Ok(EvidenceType::FileDiff),
        "artifact" => Ok(EvidenceType::Artifact),
        "user_confirmation" => Ok(EvidenceType::UserConfirmation),
        "external_reference" => Ok(EvidenceType::ExternalReference),
        value => Err(invalid_enum(index, "evidence type", value)),
    }
}

fn parse_reference_kind(row: &Row<'_>, index: usize) -> rusqlite::Result<EvidenceReferenceKind> {
    match row.get::<_, String>(index)?.as_str() {
        "tool_call" => Ok(EvidenceReferenceKind::ToolCall),
        "artifact" => Ok(EvidenceReferenceKind::Artifact),
        "run_event" => Ok(EvidenceReferenceKind::RunEvent),
        "message" => Ok(EvidenceReferenceKind::Message),
        "source" => Ok(EvidenceReferenceKind::Source),
        value => Err(invalid_enum(index, "evidence reference kind", value)),
    }
}

fn parse_validity_status(row: &Row<'_>, index: usize) -> rusqlite::Result<EvidenceValidityStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "unverified" => Ok(EvidenceValidityStatus::Unverified),
        "valid" => Ok(EvidenceValidityStatus::Valid),
        "stale" => Ok(EvidenceValidityStatus::Stale),
        "missing" => Ok(EvidenceValidityStatus::Missing),
        "invalid" => Ok(EvidenceValidityStatus::Invalid),
        value => Err(invalid_enum(index, "evidence validity status", value)),
    }
}

fn invalid_enum(index: usize, kind: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("unknown {kind} '{value}'").into(),
    )
}

fn goal_status_text(status: &GoalStatus) -> &'static str {
    match status {
        GoalStatus::Proposed => "proposed",
        GoalStatus::Active => "active",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Completed => "completed",
        GoalStatus::Cancelled => "cancelled",
    }
}

fn task_status_text(status: &WorkTaskStatus) -> &'static str {
    match status {
        WorkTaskStatus::Queued => "queued",
        WorkTaskStatus::InProgress => "in_progress",
        WorkTaskStatus::Completed => "completed",
        WorkTaskStatus::Blocked => "blocked",
        WorkTaskStatus::Interrupted => "interrupted",
        WorkTaskStatus::Skipped => "skipped",
    }
}

fn evidence_type_text(evidence_type: &EvidenceType) -> &'static str {
    match evidence_type {
        EvidenceType::ToolCall => "tool_call",
        EvidenceType::TraceSpan => "trace_span",
        EvidenceType::TestResult => "test_result",
        EvidenceType::FileDiff => "file_diff",
        EvidenceType::Artifact => "artifact",
        EvidenceType::UserConfirmation => "user_confirmation",
        EvidenceType::ExternalReference => "external_reference",
    }
}

fn reference_kind_text(ref_kind: &EvidenceReferenceKind) -> &'static str {
    match ref_kind {
        EvidenceReferenceKind::ToolCall => "tool_call",
        EvidenceReferenceKind::Artifact => "artifact",
        EvidenceReferenceKind::RunEvent => "run_event",
        EvidenceReferenceKind::Message => "message",
        EvidenceReferenceKind::Source => "source",
    }
}

fn validity_status_text(status: &EvidenceValidityStatus) -> &'static str {
    match status {
        EvidenceValidityStatus::Unverified => "unverified",
        EvidenceValidityStatus::Valid => "valid",
        EvidenceValidityStatus::Stale => "stale",
        EvidenceValidityStatus::Missing => "missing",
        EvidenceValidityStatus::Invalid => "invalid",
    }
}

fn timestamp() -> String {
    now_ms().to_string()
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), RepositoryError> {
    if value.trim().is_empty() {
        Err(RepositoryError::InvalidInput(format!(
            "{field} cannot be empty"
        )))
    } else {
        Ok(())
    }
}

fn not_found(entity: &'static str, id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity,
        id: id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{
        ApprovalDecision, ApprovalRecord, HostToolCallDisposition, ToolCallRecord,
    };
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    fn test_database() -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-work-graph-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open test database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("work graph"), None, None)
            .expect("create conversation");
        assert!(!conversation.id.is_empty());
        (database, path)
    }

    fn conversation_id(database: &Database) -> String {
        database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT id FROM conversations ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    fn insert_running_reviewer_run(database: &Database, conversation_id: &str) -> String {
        let run_id = Uuid::new_v4().to_string();
        database
            .with_connection(|connection| {
                let now = now_ms();
                connection.execute(
                    "INSERT INTO runs(
                        id, conversation_id, status, model, started_at, last_seq, created_at
                     ) VALUES (?1, ?2, 'running', 'independent-reviewer', ?3, 1, ?3)",
                    params![run_id, conversation_id, now],
                )?;
                Ok(())
            })
            .unwrap();
        run_id
    }

    fn goal_input(conversation_id: &str, status: GoalStatus) -> CreateGoalInput {
        CreateGoalInput {
            id: None,
            conversation_id: conversation_id.to_owned(),
            title: "Deliver A0".to_owned(),
            objective: "Close the work graph".to_owned(),
            acceptance_summary: None,
            status,
            created_by: "user".to_owned(),
        }
    }

    fn task_input(goal_id: &str, ordinal: i64) -> CreateTaskInput {
        CreateTaskInput {
            id: None,
            goal_id: goal_id.to_owned(),
            parent_task_id: None,
            ordinal,
            title: format!("Task {ordinal}"),
            detail: None,
        }
    }

    fn external_evidence(task_id: &str, url: &str) -> AddEvidenceInput {
        AddEvidenceInput {
            id: None,
            task_id: task_id.to_owned(),
            source_run_id: None,
            evidence_type: EvidenceType::ExternalReference,
            ref_kind: EvidenceReferenceKind::Source,
            ref_id: url.to_owned(),
            summary: "proof".to_owned(),
            metadata: serde_json::json!({}),
            trace_id: None,
            span_id: None,
        }
    }

    fn add_host_check_evidence(
        database: &Database,
        task_id: &str,
        run_id: &str,
        check_type: &str,
        suffix: &str,
    ) -> TaskEvidenceRecord {
        let runtime_tool_call_id = format!("check-{check_type}-{suffix}");
        let tool_call = database
            .create_host_tool_call(
                run_id,
                &runtime_tool_call_id,
                "host_validation_check",
                &serde_json::json!({ "checkType": check_type }),
                "running",
                false,
            )
            .unwrap();
        database
            .complete_host_tool_call(
                run_id,
                &runtime_tool_call_id,
                Some(&serde_json::json!({ "passed": true })),
                None,
            )
            .unwrap();
        let evidence = database
            .task_evidence()
            .add(AddEvidenceInput {
                id: None,
                task_id: task_id.to_owned(),
                source_run_id: Some(run_id.to_owned()),
                evidence_type: if check_type == "test" {
                    EvidenceType::TestResult
                } else {
                    EvidenceType::ToolCall
                },
                ref_kind: EvidenceReferenceKind::ToolCall,
                ref_id: tool_call.id,
                summary: format!("Host {check_type} passed"),
                metadata: serde_json::json!({ "validationCheckType": check_type }),
                trace_id: None,
                span_id: None,
            })
            .unwrap();
        assert_eq!(
            database.task_evidence().validate(&evidence.id).unwrap(),
            EvidenceValidityStatus::Valid
        );
        evidence
    }

    struct RepairOverrideFixture {
        conversation_id: String,
        run_id: String,
        task: WorkTaskRecord,
        finding: ReviewFindingRecord,
    }

    fn repair_override_fixture(
        database: &Database,
        profile_id: &str,
        exhaust_normal_budget: bool,
    ) -> RepairOverrideFixture {
        let conversation_id = conversation_id(database);
        let run = database
            .create_run(&conversation_id, "repair budget override", None)
            .unwrap()
            .run;
        assert!(database
            .apply_runtime_event(&run.id, 1, &serde_json::json!({ "type": "run.started" }))
            .unwrap());
        let running_after_start: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    [run.id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(running_after_start, "running");
        database
            .freeze_run_execution_profile(&run.id, profile_id)
            .unwrap();
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let mut task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let plan = database
            .create_plan_revision(
                &conversation_id,
                &goal.id,
                "Repair override plan",
                "Approved bounded repair plan",
                serde_json::json!([{ "taskId": task.id.clone() }]),
                &run.id,
            )
            .unwrap();
        database
            .resolve_plan_revision(&conversation_id, &plan.id, "approved")
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'completed' WHERE id = ?1",
                    [task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        task = database.work_tasks().get(&task.id).unwrap().unwrap();
        let finding = database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "medium",
                "correctness",
                "repair me",
                "root cause",
                "open",
                "review-run",
            )
            .unwrap();
        if exhaust_normal_budget {
            for number in 1..=2 {
                let attempt_id = format!("normal-repair-{number}-{}", task.id);
                let started = database
                    .start_task_attempt(StartTaskAttemptInput {
                        id: attempt_id.clone(),
                        task_id: task.id.clone(),
                        run_id: run.id.clone(),
                        expected_task_version: task.version,
                        kind: TaskAttemptKind::Repair,
                        root_cause: Some("same root cause".to_owned()),
                        finding_ids: vec![finding.id.clone()],
                    })
                    .unwrap();
                let finished = database
                    .finish_task_attempt(FinishTaskAttemptInput {
                        id: attempt_id,
                        task_id: task.id.clone(),
                        run_id: run.id.clone(),
                        expected_task_version: started.task.version,
                        expected_attempt_version: 1,
                        status: TaskAttemptStatus::Failed,
                        failure_reason: Some("repair did not validate".to_owned()),
                    })
                    .unwrap();
                task = finished.task;
            }
            task = database
                .start_task_attempt(StartTaskAttemptInput {
                    id: format!("budget-exhaustion-probe-{}", task.id),
                    task_id: task.id.clone(),
                    run_id: run.id.clone(),
                    expected_task_version: task.version,
                    kind: TaskAttemptKind::Repair,
                    root_cause: Some("same root cause".to_owned()),
                    finding_ids: vec![finding.id.clone()],
                })
                .unwrap()
                .task;
            assert_eq!(task.status, WorkTaskStatus::Blocked);
        }
        let running_after_budget: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    [run.id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(running_after_budget, "running");
        RepairOverrideFixture {
            conversation_id,
            run_id: run.id,
            task,
            finding,
        }
    }

    #[test]
    fn kernel_repair_override_requires_once_decision_and_dispatch_lease() {
        use crate::kernel::{self, Clock, PolicyDecisionPort};
        use crate::runtime_host::kernel_coordinator::KernelCoordinator;
        struct Ask;
        impl PolicyDecisionPort for Ask {
            fn decide(&self,_: &str,_: &str,_: &str,_: &str)->kernel::PolicyDecision {
                kernel::PolicyDecision::RequireApproval
            }
        }
        let (database,path)=test_database();
        let fixture=repair_override_fixture(&database,"durable_v2",true);
        let binding=database.freeze_kernel_run_control(&fixture.run_id,"durable_v2",fox_engine_protocol::TimeBudgets::default()).unwrap();
        let config=kernel::RunFrozenConfig {
            engine_id:"pi".into(),kernel_mode:"authoritative".into(),capability_manifest_version:2,
            capability_manifest_hash:"repair-fixture".into(),permission_snapshot_id:binding.permission_snapshot_id.clone(),
            execution_profile_id:"durable_v2".into(),prompt_config_hash:"repair-fixture".into(),
            model_request_timeout_ms:binding.budgets.model_request_ms,model_first_response_ms:binding.budgets.model_first_response_ms,model_idle_ms:binding.budgets.model_idle_ms,tool_execution_timeout_ms:binding.budgets.tool_execution_ms,
            run_execution_budget_ms:binding.budgets.run_execution_ms,
            run_execution_limited: binding.budgets.run_execution_limited,approval_wait_timeout_ms:binding.budgets.approval_wait_ms,
            provider_max_retries:0,turn_max_retries:0,
        };
        database.kernel_create_run(&fixture.run_id,"pi","authoritative",2,&binding.permission_snapshot_id,
            "durable_v2","repair-fixture",&serde_json::to_string(&config).unwrap()).unwrap();
        let clock=kernel::TestClock::new(super::now_ms());
        let (controller,effects)=kernel::RunController::start(&fixture.run_id,"repair-turn",config,&clock).unwrap();
        database.kernel_commit_decision(&fixture.run_id,clock.now_wall_ms(),&controller.persist_command(&effects)).unwrap();
        let cancellation=kernel::CancellationRegistry::default();
        let coordinator=KernelCoordinator::reopen(&database,&clock,&fixture.run_id,&cancellation).unwrap();
        let input=StartTaskRepairOverrideInput {
            attempt_id:"kernel-repair-attempt".into(),task_id:fixture.task.id.clone(),run_id:fixture.run_id.clone(),
            conversation_id:fixture.conversation_id.clone(),tool_call_id:format!("kernel-tool:{}:repair",fixture.run_id),
            approval_id:format!("kernel-approval:{}:repair:v{}",fixture.run_id,
                database.execution_policy(&fixture.conversation_id).unwrap().version),expected_task_version:fixture.task.version,
            root_cause:"operator confirmed root cause".into(),finding_ids:vec![fixture.finding.id.clone()],
            escalation_reason:"Authorize one bounded repair after the normal budget is exhausted.".into(),
        };
        coordinator.propose_tools("repair-batch",vec![kernel::ToolCallRequest {
            tool_call_id:"repair".into(),tool:"task_repair_escalate_start".into(),
            canonical_input_json:task_repair_override_input_value(&input).to_string(),source_order:0,
        }],&Ask).unwrap();
        let approval=database.load_conversation(&fixture.conversation_id).unwrap().approvals.into_iter()
            .find(|approval|approval.id==input.approval_id).unwrap();
        assert_eq!(approval.category,"task_repair_budget_override");
        assert_eq!(approval.request["availableDecisions"],serde_json::json!(["allow_once","deny"]));
        assert!(database.queue_kernel_host_command(&fixture.run_id,Some(("repair","allow_conversation"))).is_err());
        assert!(database.start_task_repair_override(input.clone()).is_err());
        coordinator.resolve_approval("repair",kernel::ApprovalDecision::AllowOnce).unwrap();
        assert!(database.start_task_repair_override(input.clone()).is_err(),"a decision alone is not a dispatch lease");
        assert!(coordinator.dispatch_tool("repair","repair-owner",|_,_,_| {
            let result=database.start_task_repair_override(input.clone()).map_err(|error|error.to_string())?;
            assert_eq!(result.attempt.id,input.attempt_id);
            let replay=database.start_task_repair_override(input.clone()).map_err(|error|error.to_string())?;
            assert_eq!(replay.override_event.id,result.override_event.id);
            Ok((true,serde_json::json!({"attemptId":result.attempt.id})))
        }).unwrap());
        let detail=database.load_conversation(&fixture.conversation_id).unwrap();
        assert_eq!(detail.tool_calls.iter().find(|tool|tool.id==input.tool_call_id).unwrap().status,"completed");
        let approval=detail.approvals.iter().find(|approval|approval.id==input.approval_id).unwrap();
        assert_eq!(approval.decision.as_ref().unwrap()["scope"],"once");
        assert!(approval.claimed_at.is_some());
        drop(coordinator);
        cleanup(database,path);
    }

    fn approved_repair_override(
        database: &Database,
        fixture: &RepairOverrideFixture,
        attempt_id: &str,
        runtime_tool_call_id: &str,
    ) -> (StartTaskRepairOverrideInput, ToolCallRecord, ApprovalRecord) {
        let mut input = StartTaskRepairOverrideInput {
            attempt_id: attempt_id.to_owned(),
            task_id: fixture.task.id.clone(),
            run_id: fixture.run_id.clone(),
            conversation_id: fixture.conversation_id.clone(),
            tool_call_id: "pending-host-tool-call".to_owned(),
            approval_id: "pending-approval".to_owned(),
            expected_task_version: fixture.task.version,
            root_cause: "root cause confirmed by the operator".to_owned(),
            finding_ids: vec![fixture.finding.id.clone()],
            escalation_reason:
                "The ordinary repair budget is exhausted; authorize one final bounded repair."
                    .to_owned(),
        };
        let status_before_tool: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    [input.run_id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(status_before_tool, "running");
        let tool_call = database
            .create_host_tool_call(
                &input.run_id,
                runtime_tool_call_id,
                "task_repair_escalate_start",
                &task_repair_override_input_value(&input),
                "pending",
                true,
            )
            .unwrap();
        let status_after_tool: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    [input.run_id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(status_after_tool, "running");
        let approval = database
            .create_task_repair_override_approval(
                &tool_call.id,
                "Start one operator-authorized Repair beyond the frozen policy budget",
                &serde_json::json!({ "category": "task_repair_budget_override" }),
            )
            .unwrap();
        let (run_status, tool_status): (String, String) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT status FROM runs WHERE id = ?1",
                        [input.run_id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls WHERE id = ?1",
                        [tool_call.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(run_status, "running");
        assert_eq!(tool_status, "pending");
        let approval = database
            .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
            .unwrap()
            .unwrap();
        input.tool_call_id = tool_call.id.clone();
        input.approval_id = approval.id.clone();
        (input, tool_call, approval)
    }

    fn repair_override_preflight_input(
        fixture: &RepairOverrideFixture,
        attempt_id: &str,
    ) -> PreflightTaskRepairOverrideInput {
        PreflightTaskRepairOverrideInput {
            task_id: fixture.task.id.clone(),
            attempt_id: attempt_id.to_owned(),
            run_id: fixture.run_id.clone(),
            conversation_id: fixture.conversation_id.clone(),
            expected_task_version: fixture.task.version,
            root_cause: "operator-confirmed root cause".to_owned(),
            finding_ids: vec![fixture.finding.id.clone()],
            escalation_reason: "operator reviewed exhausted normal budget".to_owned(),
        }
    }

    fn approval_count(database: &Database) -> i64 {
        database
            .with_connection(|connection| {
                connection.query_row("SELECT COUNT(*) FROM approvals", [], |row| row.get(0))
            })
            .unwrap()
    }

    fn continuation_input(
        decision_id: &str,
        run_id: &str,
        event_cursor: i64,
        decision: ContinuationDecisionKind,
        reason_code: ContinuationReasonCode,
    ) -> AppendContinuationDecisionInput {
        AppendContinuationDecisionInput {
            schema_version: 1,
            decision_id: decision_id.to_owned(),
            run_id: run_id.to_owned(),
            event_cursor,
            decision,
            reason_code,
            active_task_ids: Vec::new(),
            evidence_ids: Vec::new(),
            missing_acceptance: Vec::new(),
            next_action: None,
            retry_class: None,
            blocked_dependency_refs: Vec::new(),
            host_validation_outcome: ContinuationValidationOutcome::Accepted,
            host_validation_error: None,
            expected_projection_hash: None,
        }
    }

    fn cleanup(database: Database, path: PathBuf) {
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn continuation_decisions_append_history_and_validate_watermarks_and_associations() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let started = database
            .create_run(&conversation_id, "continue durable work", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({
                    "type": "run.started",
                    "executionProfileId": "durable_v2"
                }),
            )
            .unwrap();

        let mut accepted = continuation_input(
            "decision-a",
            &started.run.id,
            1,
            ContinuationDecisionKind::Repair,
            ContinuationReasonCode::ValidationFailed,
        );
        accepted.active_task_ids = vec![task.id.clone()];
        accepted.missing_acceptance = vec!["tests_pass".to_owned()];
        accepted.next_action = Some("run targeted tests".to_owned());
        accepted.retry_class = Some(ContinuationRetryClass::Recoverable);
        database
            .append_continuation_decision(accepted.clone())
            .unwrap();

        let mut rejected = continuation_input(
            "decision-b",
            &started.run.id,
            1,
            ContinuationDecisionKind::Complete,
            ContinuationReasonCode::AcceptancePassed,
        );
        rejected.host_validation_outcome = ContinuationValidationOutcome::Rejected;
        rejected.host_validation_error = Some("task is not complete".to_owned());
        database.append_continuation_decision(rejected).unwrap();

        let history = database
            .continuation_decisions_for_run(&started.run.id)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].decision_id, "decision-a");
        assert_eq!(history[1].decision_id, "decision-b");
        assert_eq!(
            history[1].host_validation_outcome,
            ContinuationValidationOutcome::Rejected
        );
        let idempotent = database
            .append_continuation_decision(accepted.clone())
            .unwrap();
        assert_eq!(idempotent.decision_id, "decision-a");
        assert_eq!(idempotent.created_at, history[0].created_at);
        assert_eq!(
            database
                .continuation_decision_by_id(&started.run.id, "decision-a")
                .unwrap()
                .unwrap()
                .created_at,
            history[0].created_at
        );
        assert!(matches!(
            database.continuation_decision_by_id("different-run", "decision-a"),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        let mut conflicting = accepted.clone();
        conflicting.next_action = Some("different content".to_owned());
        assert!(matches!(
            database.append_continuation_decision(conflicting),
            Err(RepositoryError::ConstraintViolation(_))
        ));

        let mut backwards = continuation_input(
            "decision-backwards",
            &started.run.id,
            0,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        backwards.active_task_ids = vec![task.id.clone()];
        assert!(matches!(
            database.append_continuation_decision(backwards),
            Err(RepositoryError::InvalidInput(_))
        ));
        let future = continuation_input(
            "decision-future",
            &started.run.id,
            2,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        assert!(matches!(
            database.append_continuation_decision(future),
            Err(RepositoryError::InvalidInput(_))
        ));

        let other_conversation = database
            .create_conversation(
                database.default_agent_id(),
                Some("other durable work"),
                None,
                None,
            )
            .unwrap();
        let other_goal = database
            .goals()
            .create(goal_input(&other_conversation.id, GoalStatus::Active))
            .unwrap();
        let other_task = database
            .work_tasks()
            .create(task_input(&other_goal.id, 0))
            .unwrap();
        let mut cross_conversation = continuation_input(
            "decision-cross-conversation",
            &started.run.id,
            1,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        cross_conversation.active_task_ids = vec![other_task.id];
        assert!(matches!(
            database.append_continuation_decision(cross_conversation),
            Err(RepositoryError::CrossConversationReference)
        ));
        cleanup(database, path);
    }

    #[test]
    fn continuation_projection_hash_cas_rejects_stale_facts_but_allows_idempotent_retry() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let started = database
            .create_run(&conversation_id, "projection hash CAS", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .unwrap();
        database
            .work_tasks()
            .start(&task.id, &started.run.id)
            .unwrap();
        let evidence = database
            .task_evidence()
            .add(external_evidence(
                &task.id,
                "https://example.com/cas-evidence",
            ))
            .unwrap();
        database.task_evidence().validate(&evidence.id).unwrap();
        database.work_tasks().complete(&task.id).unwrap();
        let original_hash = database
            .task_ledger_projection(&conversation_id)
            .unwrap()
            .projection_meta
            .projection_hash;

        let mut committed = continuation_input(
            "decision-cas-committed",
            &started.run.id,
            1,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        committed.expected_projection_hash = Some(original_hash.clone());
        let first = database
            .append_continuation_decision(committed.clone())
            .unwrap();
        assert_eq!(
            database
                .task_ledger_projection(&conversation_id)
                .unwrap()
                .projection_meta
                .projection_hash,
            original_hash
        );

        database
            .task_evidence()
            .mark_stale(&evidence.id, "source expired".to_owned())
            .unwrap();
        let changed_projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert!(changed_projection.completed_summary[0]
            .evidence_ids
            .is_empty());
        let changed_hash = changed_projection.projection_meta.projection_hash;
        assert_ne!(changed_hash, original_hash);

        let retried = database
            .append_continuation_decision(committed)
            .expect("an already-committed identical decision is idempotent before CAS");
        assert_eq!(retried.created_at, first.created_at);

        let mut stale = continuation_input(
            "decision-cas-stale",
            &started.run.id,
            1,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        stale.expected_projection_hash = Some(original_hash);
        let error = database.append_continuation_decision(stale).unwrap_err();
        assert!(matches!(
            error,
            RepositoryError::ProjectionHashMismatch { .. }
        ));
        assert!(error
            .to_string()
            .starts_with("[continuation.projection_hash_mismatch]"));
        cleanup(database, path);
    }

    #[test]
    fn unprojected_proposals_and_run_conversation_use_authoritative_database_identity() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let started = database
            .create_run(&conversation_id, "reconcile continuation proposal", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({
                    "type": "run.started",
                    "executionProfileId": "durable_v2"
                }),
            )
            .unwrap();
        let proposal = serde_json::json!({
            "type": "run.continuation_proposed",
            "proposal": {
                "schemaVersion": 1,
                "decisionId": "decision-needs-reconciliation",
                "runId": started.run.id,
                "eventCursor": 1,
                "decision": "continue",
                "reasonCode": "work_remaining",
                "activeTaskIds": [],
                "evidenceIds": [],
                "missingAcceptance": [],
                "nextAction": "continue",
                "retryClass": "recoverable",
                "blockedDependencyRefs": []
            }
        });
        let malformed = serde_json::json!({
            "type": "run.continuation_proposed",
            "proposal": {}
        });
        database
            .apply_runtime_event(&started.run.id, 2, &malformed)
            .unwrap();
        database
            .apply_runtime_event(&started.run.id, 3, &proposal)
            .unwrap();

        assert_eq!(
            database
                .authoritative_run_conversation_id(&started.run.id)
                .unwrap(),
            conversation_id
        );
        assert!(matches!(
            database.authoritative_run_conversation_id("missing-run"),
            Err(RepositoryError::NotFound { .. })
        ));
        let unprojected = database.unprojected_continuation_proposals(1).unwrap();
        assert_eq!(unprojected.len(), 1);
        assert_eq!(unprojected[0].conversation_id, conversation_id);
        assert_eq!(unprojected[0].run_id, started.run.id);
        assert_eq!(unprojected[0].seq, 2);
        assert_eq!(unprojected[0].payload, malformed);
        assert_eq!(
            unprojected[0].execution_profile_id.as_deref(),
            Some("durable_v2")
        );

        let diagnostic_input = RecordContinuationIngestDiagnosticInput {
            run_id: started.run.id.clone(),
            event_seq: 2,
            execution_profile_id: Some("durable_v2".to_owned()),
            shadow_mode: false,
            error_code: "malformed_proposal".to_owned(),
            error_message: "proposal.decisionId is missing".to_owned(),
        };
        let diagnostic = database
            .record_continuation_ingest_diagnostic(&diagnostic_input)
            .unwrap();
        let idempotent_diagnostic = database
            .record_continuation_ingest_diagnostic(&diagnostic_input)
            .unwrap();
        assert_eq!(idempotent_diagnostic.created_at, diagnostic.created_at);
        let mut conflicting_diagnostic = diagnostic_input.clone();
        conflicting_diagnostic.error_message = "different failure".to_owned();
        assert!(matches!(
            database.record_continuation_ingest_diagnostic(&conflicting_diagnostic),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        let mut oversized_diagnostic = diagnostic_input.clone();
        oversized_diagnostic.error_message = "x".repeat(4097);
        assert!(matches!(
            database.record_continuation_ingest_diagnostic(&oversized_diagnostic),
            Err(RepositoryError::InvalidInput(_))
        ));
        let non_proposal = RecordContinuationIngestDiagnosticInput {
            run_id: started.run.id.clone(),
            event_seq: 1,
            execution_profile_id: Some("durable_v2".to_owned()),
            shadow_mode: false,
            error_code: "unexpected".to_owned(),
            error_message: "not a proposal".to_owned(),
        };
        assert!(matches!(
            database.record_continuation_ingest_diagnostic(&non_proposal),
            Err(RepositoryError::ConstraintViolation(_))
        ));

        // The malformed head event is now suppressed, so a bounded scan reaches
        // the valid proposal behind it instead of starving reconciliation.
        let unprojected = database.unprojected_continuation_proposals(1).unwrap();
        assert_eq!(unprojected.len(), 1);
        assert_eq!(unprojected[0].seq, 3);
        assert_eq!(unprojected[0].payload, proposal);

        let decision = continuation_input(
            "decision-needs-reconciliation",
            &started.run.id,
            1,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        database.append_continuation_decision(decision).unwrap();
        let remaining = database.unprojected_continuation_proposals(10).unwrap();
        assert!(remaining.is_empty());
        cleanup(database, path);
    }

    #[test]
    fn task_ledger_latest_run_uses_insertion_order_when_timestamps_match() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO runs(
                        id, conversation_id, status, model, last_seq, created_at
                     ) VALUES ('zz-old-run', ?1, 'completed', 'model', 0, 42)",
                    [conversation_id.as_str()],
                )?;
                connection.execute(
                    "INSERT INTO runs(
                        id, conversation_id, status, model, last_seq, created_at
                     ) VALUES ('aa-new-run', ?1, 'queued', 'model', 0, 42)",
                    [conversation_id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            projection.execution_cursor.run_id.as_deref(),
            Some("aa-new-run")
        );
        assert_eq!(
            projection.execution_cursor.current_phase,
            TaskLedgerPhase::Queued
        );
        cleanup(database, path);
    }

    #[test]
    fn continuation_v1_reason_and_retry_contract_round_trips() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let started = database
            .create_run(&conversation_id, "round trip durable contract", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .unwrap();
        let reasons = [
            ContinuationReasonCode::WorkRemaining,
            ContinuationReasonCode::ValidationFailed,
            ContinuationReasonCode::ApprovalPending,
            ContinuationReasonCode::ExternalDependencyUnavailable,
            ContinuationReasonCode::BudgetExhausted,
            ContinuationReasonCode::UserInputRequired,
            ContinuationReasonCode::RetryAvailable,
            ContinuationReasonCode::RetryExhausted,
            ContinuationReasonCode::AcceptanceMissing,
            ContinuationReasonCode::AcceptanceCandidate,
            ContinuationReasonCode::AcceptancePassed,
            ContinuationReasonCode::HostAuditRequired,
            ContinuationReasonCode::ToolFailed,
            ContinuationReasonCode::TaskInterrupted,
        ];
        let retry_classes = [
            ContinuationRetryClass::None,
            ContinuationRetryClass::Recoverable,
            ContinuationRetryClass::RetryLimited,
            ContinuationRetryClass::NonRetryable,
            ContinuationRetryClass::HostDecides,
        ];
        for (index, reason) in reasons.iter().enumerate() {
            let mut input = continuation_input(
                &format!("decision-contract-{index}"),
                &started.run.id,
                1,
                ContinuationDecisionKind::Continue,
                reason.clone(),
            );
            input.retry_class = Some(retry_classes[index % retry_classes.len()].clone());
            database.append_continuation_decision(input).unwrap();
        }

        let history = database
            .continuation_decisions_for_run(&started.run.id)
            .unwrap();
        assert_eq!(history.len(), reasons.len());
        for reason in reasons {
            assert!(history.iter().any(|record| record.reason_code == reason));
        }
        for retry_class in retry_classes {
            assert!(history
                .iter()
                .any(|record| record.retry_class.as_ref() == Some(&retry_class)));
        }
        cleanup(database, path);
    }

    #[test]
    fn task_ledger_projection_rebuilds_from_legacy_facts_and_host_accepted_decisions() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let completed_task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let queued_task = database
            .work_tasks()
            .create(task_input(&goal.id, 1))
            .unwrap();
        let started = database
            .create_run(&conversation_id, "project durable work", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .unwrap();
        database
            .work_tasks()
            .start(&completed_task.id, &started.run.id)
            .unwrap();
        let evidence = database
            .task_evidence()
            .add(external_evidence(
                &completed_task.id,
                "https://example.com/durable-proof",
            ))
            .unwrap();
        database.task_evidence().validate(&evidence.id).unwrap();
        database.work_tasks().complete(&completed_task.id).unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO acceptances(
                        id, goal_id, conversation_id, status, summary, checks_json,
                        reviewer, created_at, resolved_at
                     ) VALUES (?1, ?2, ?3, 'accepted', 'legacy checks passed', ?4,
                               'host', '10', '10')",
                    params![
                        "acceptance-ledger",
                        goal.id,
                        conversation_id,
                        serde_json::json!([
                            {"taskId": queued_task.id, "name": "tests_pass", "passed": true}
                        ])
                        .to_string(),
                    ],
                )?;
                Ok(())
            })
            .unwrap();

        let legacy_projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(legacy_projection.schema_version, 1);
        assert_eq!(
            legacy_projection.goal.as_ref().unwrap().completion_policy,
            "legacy_v1"
        );
        assert!(legacy_projection.latest_host_accepted_decision.is_none());
        assert_eq!(legacy_projection.active_tasks.len(), 1);
        assert_eq!(legacy_projection.active_tasks[0].task_id, queued_task.id);
        assert_eq!(legacy_projection.active_tasks[0].acceptance_checks.len(), 1);
        assert_eq!(legacy_projection.completed_summary.len(), 1);
        assert_eq!(
            legacy_projection.completed_summary[0].outcome,
            TaskLedgerCompletedOutcome::Accepted
        );
        assert_eq!(legacy_projection.execution_cursor.event_cursor, 1);
        assert_eq!(
            legacy_projection
                .projection_meta
                .source_high_watermark
                .run_cursors[0]
                .event_cursor,
            1
        );

        let mut decision = continuation_input(
            "decision-projection",
            &started.run.id,
            1,
            ContinuationDecisionKind::Continue,
            ContinuationReasonCode::WorkRemaining,
        );
        decision.active_task_ids = vec![queued_task.id.clone()];
        decision.evidence_ids = vec![evidence.id.clone()];
        decision.next_action = Some("start the queued task".to_owned());
        database.append_continuation_decision(decision).unwrap();

        let rebuilt = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            rebuilt
                .latest_host_accepted_decision
                .as_ref()
                .map(|decision| decision.decision_id.as_str()),
            Some("decision-projection")
        );
        assert_eq!(
            rebuilt.execution_cursor.resumable_from.as_deref(),
            Some(queued_task.id.as_str())
        );
        assert_eq!(
            rebuilt.execution_cursor.current_phase,
            TaskLedgerPhase::Queued
        );
        assert_eq!(
            legacy_projection.projection_meta.derived_task_ids,
            rebuilt.projection_meta.derived_task_ids
        );
        assert_eq!(
            legacy_projection.projection_meta.projection_hash,
            rebuilt.projection_meta.projection_hash,
            "Decision audit metadata must not affect the formal-fact projection hash"
        );

        database
            .apply_runtime_event(
                &started.run.id,
                2,
                &serde_json::json!({"type": "run.completed"}),
            )
            .unwrap();

        let next_run = database
            .create_run(
                &conversation_id,
                "new run must not inherit old decision",
                None,
            )
            .unwrap();
        let next_projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            next_projection.execution_cursor.run_id.as_deref(),
            Some(next_run.run.id.as_str())
        );
        assert!(next_projection.latest_host_accepted_decision.is_none());
        assert_eq!(
            next_projection.execution_cursor.current_phase,
            TaskLedgerPhase::Queued
        );
        assert_eq!(
            next_projection.execution_cursor.resumable_from.as_deref(),
            Some(queued_task.id.as_str())
        );
        cleanup(database, path);
    }

    #[test]
    fn accepted_decision_audit_does_not_create_pending_actions_or_override_completed_facts() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let started = database
            .create_run(&conversation_id, "repair then finish", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .unwrap();

        let mut repair = continuation_input(
            "decision-repair-audit-only",
            &started.run.id,
            1,
            ContinuationDecisionKind::Repair,
            ContinuationReasonCode::ValidationFailed,
        );
        repair.active_task_ids = vec![task.id.clone()];
        repair.missing_acceptance = vec!["tests_pass".to_owned()];
        repair.next_action = Some("repair the task".to_owned());
        repair.retry_class = Some(ContinuationRetryClass::Recoverable);
        database.append_continuation_decision(repair).unwrap();

        database
            .work_tasks()
            .start(&task.id, &started.run.id)
            .unwrap();
        let evidence = database
            .task_evidence()
            .add(external_evidence(
                &task.id,
                "https://example.com/repaired-proof",
            ))
            .unwrap();
        database.task_evidence().validate(&evidence.id).unwrap();
        database.work_tasks().complete(&task.id).unwrap();
        database.goals().complete(&goal.id, goal.version).unwrap();

        let projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            projection.execution_cursor.current_phase,
            TaskLedgerPhase::Complete
        );
        assert!(projection.pending_actions.is_empty());
        assert!(projection.execution_cursor.resumable_from.is_none());
        assert_eq!(
            projection
                .latest_host_accepted_decision
                .as_ref()
                .map(|decision| decision.decision_id.as_str()),
            Some("decision-repair-audit-only")
        );
        cleanup(database, path);
    }

    #[test]
    fn task_ledger_pending_approval_keeps_run_and_tool_call_identity() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let started = database
            .create_run(&conversation_id, "approval identity", None)
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name,
                        input_json, status, execution_location, requires_approval,
                        started_at, updated_at
                     ) VALUES (
                        'tool-call-ledger', 'runtime-tool-call-ledger', ?1, ?2, 'write_file',
                        '{}', 'awaiting_approval', 'host', 1, 10, 10
                     )",
                    params![started.run.id, conversation_id],
                )?;
                connection.execute(
                    "INSERT INTO approvals(
                        id, tool_call_id, status, requested_action, request_json, requested_at
                     ) VALUES (
                        'approval-ledger', 'tool-call-ledger', 'pending',
                        'write an authorized file', '{}', 10
                     )",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let projection = database.task_ledger_projection(&conversation_id).unwrap();
        let approval = projection
            .pending_actions
            .iter()
            .find(|action| matches!(action.kind, TaskLedgerPendingActionKind::Approval))
            .unwrap();
        assert_eq!(approval.reference_id, "approval-ledger");
        assert_eq!(approval.run_id.as_deref(), Some(started.run.id.as_str()));
        assert_eq!(approval.tool_call_id.as_deref(), Some("tool-call-ledger"));
        assert_eq!(
            projection.execution_cursor.current_phase,
            TaskLedgerPhase::WaitApproval
        );
        assert_eq!(
            projection.execution_cursor.resumable_from.as_deref(),
            Some("approval-ledger")
        );

        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs SET status = 'completed', finished_at = 10 WHERE id = ?1",
                    [started.run.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let next_run = database
            .create_run(
                &conversation_id,
                "approval must stay with previous run",
                None,
            )
            .unwrap();
        let next_projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            next_projection.execution_cursor.run_id.as_deref(),
            Some(next_run.run.id.as_str())
        );
        assert!(next_projection.pending_actions.is_empty());
        assert_eq!(
            next_projection.execution_cursor.current_phase,
            TaskLedgerPhase::Queued
        );
        cleanup(database, path);
    }

    #[test]
    fn goal_supports_every_legal_transition_and_rejects_illegal_jumps() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goals = database.goals();

        let proposed = goals
            .create(goal_input(&conversation_id, GoalStatus::Proposed))
            .unwrap();
        let active = goals.activate(&proposed.id, proposed.version).unwrap();
        let blocked = goals
            .block(&active.id, "waiting".to_owned(), active.version)
            .unwrap();
        let resumed = goals.activate(&blocked.id, blocked.version).unwrap();
        let completed = goals.complete(&resumed.id, resumed.version).unwrap();
        assert_eq!(completed.status, GoalStatus::Completed);
        assert!(matches!(
            goals.activate(&completed.id, completed.version),
            Err(RepositoryError::InvalidTransition { .. })
        ));

        let proposed = goals
            .create(goal_input(&conversation_id, GoalStatus::Proposed))
            .unwrap();
        let cancelled = goals.cancel(&proposed.id, proposed.version).unwrap();
        assert_eq!(cancelled.status, GoalStatus::Cancelled);

        let active = goals
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let blocked = goals
            .block(&active.id, "waiting".to_owned(), active.version)
            .unwrap();
        let cancelled = goals.cancel(&blocked.id, blocked.version).unwrap();
        assert_eq!(cancelled.status, GoalStatus::Cancelled);

        let active = goals
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let cancelled = goals.cancel(&active.id, active.version).unwrap();
        assert_eq!(cancelled.status, GoalStatus::Cancelled);

        cleanup(database, path);
    }

    #[test]
    fn goal_optimistic_lock_and_single_active_constraint_are_explicit() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goals = database.goals();
        let active = goals
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        assert!(matches!(
            goals.block(&active.id, "wait".to_owned(), active.version + 1),
            Err(RepositoryError::OptimisticLockFailed { .. })
        ));
        assert!(matches!(
            goals.create(goal_input(&conversation_id, GoalStatus::Active)),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        cleanup(database, path);
    }

    #[test]
    fn task_state_machine_increments_retry_attempt_without_overwriting_evidence() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "execute", None)
            .unwrap()
            .run;
        let tasks = database.work_tasks();
        let first = tasks.create(task_input(&goal.id, 0)).unwrap();
        let second = tasks.create(task_input(&goal.id, 1)).unwrap();
        let first = tasks.start(&first.id, &run.id).unwrap();
        assert_eq!(first.status, WorkTaskStatus::InProgress);
        assert!(matches!(
            tasks.start(&second.id, &run.id),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        assert!(matches!(
            tasks.complete(&first.id),
            Err(RepositoryError::EvidenceRequired { .. })
        ));
        let original_evidence = database
            .task_evidence()
            .add(external_evidence(&first.id, "https://example.com/proof/1"))
            .unwrap();
        let completed = tasks.complete(&first.id).unwrap();
        assert_eq!(completed.status, WorkTaskStatus::Completed);
        let requeued = tasks.requeue(&first.id).unwrap();
        assert_eq!(requeued.status, WorkTaskStatus::Queued);
        let active = tasks.start(&requeued.id, &run.id).unwrap();
        let interrupted = tasks.interrupt(&active.id).unwrap();
        let retried = tasks.retry(&interrupted.id, &run.id).unwrap();
        assert_eq!(retried.attempt, 2);
        let retained = database
            .task_evidence()
            .list_by_task(&retried.id, 50)
            .unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].id, original_evidence.id);
        database
            .task_evidence()
            .add(external_evidence(
                &retried.id,
                "https://example.com/proof/attempt-2",
            ))
            .unwrap();
        assert_eq!(
            database
                .task_evidence()
                .list_by_task(&retried.id, 50)
                .unwrap()
                .len(),
            2
        );
        assert!(matches!(
            tasks.skip(&retried.id),
            Err(RepositoryError::InvalidTransition { .. })
        ));
        cleanup(database, path);
    }

    #[test]
    fn startup_audit_interrupts_tasks_owned_by_terminal_runs() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "execute", None)
            .unwrap()
            .run;
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let task = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "startup-running-attempt".to_owned(),
                task_id: task.id,
                run_id: run.id.clone(),
                expected_task_version: 1,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .unwrap()
            .task;
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs SET status = 'failed', finished_at = 1 WHERE id = ?1",
                    [&run.id],
                )?;
                Ok(())
            })
            .unwrap();

        database.audit_interrupted_tasks().unwrap();

        let audited = database.work_tasks().get(&task.id).unwrap().unwrap();
        assert_eq!(audited.status, WorkTaskStatus::Interrupted);
        assert!(audited.finished_at.is_some());
        assert!(audited.version > task.version);
        let attempt = database
            .list_task_attempts(&audited.id, 10)
            .unwrap()
            .remove(0);
        assert_eq!(attempt.status, TaskAttemptStatus::Failed);
        assert!(attempt.finished_at.is_some());
        cleanup(database, path);
    }

    #[test]
    fn failed_test_tool_call_cannot_be_valid_or_complete_a_durable_attempt() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let run = database
            .create_run(&conversation_id, "failed tests", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &serde_json::json!({ "type": "run.started" }))
            .unwrap();
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let started = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "failed-test-attempt".to_owned(),
                task_id: task.id.clone(),
                run_id: run.id.clone(),
                expected_task_version: task.version,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .unwrap();
        let tool_call = database
            .create_host_tool_call(
                &run.id,
                "failed-test-call",
                "run_tests",
                &serde_json::json!({}),
                "running",
                false,
            )
            .unwrap();
        database
            .complete_host_tool_call(
                &run.id,
                "failed-test-call",
                Some(&serde_json::json!({ "passed": false, "exitCode": 1 })),
                Some("tests failed"),
            )
            .unwrap();
        let evidence = database.task_evidence().add(AddEvidenceInput {
            id: None,
            task_id: task.id.clone(),
            source_run_id: Some(run.id.clone()),
            evidence_type: EvidenceType::TestResult,
            ref_kind: EvidenceReferenceKind::ToolCall,
            ref_id: tool_call.id,
            summary: "failed tests are not proof".to_owned(),
            metadata: serde_json::json!({ "validationCheckType": "test" }),
            trace_id: None,
            span_id: None,
        });
        assert!(matches!(
            evidence,
            Err(RepositoryError::InvalidReference(_))
        ));
        assert!(matches!(
            database.finish_task_attempt(FinishTaskAttemptInput {
                id: "failed-test-attempt".to_owned(),
                task_id: task.id,
                run_id: run.id,
                expected_task_version: started.task.version,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Succeeded,
                failure_reason: None,
            }),
            Err(RepositoryError::EvidenceRequired { .. })
        ));
        cleanup(database, path);
    }

    #[test]
    fn mark_run_interrupted_cascades_without_completing_the_task() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "execute", None)
            .unwrap()
            .run;
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let task = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "interrupted-running-attempt".to_owned(),
                task_id: task.id,
                run_id: run.id.clone(),
                expected_task_version: 1,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .unwrap()
            .task;

        database
            .mark_run_interrupted(&run.id, "runtime.timeout", "runtime timed out")
            .unwrap();

        let interrupted = database.work_tasks().get(&task.id).unwrap().unwrap();
        assert_eq!(interrupted.status, WorkTaskStatus::Interrupted);
        assert_eq!(
            database.list_task_attempts(&task.id, 10).unwrap()[0].status,
            TaskAttemptStatus::Failed
        );
        assert!(matches!(
            database.work_tasks().complete(&interrupted.id),
            Err(RepositoryError::InvalidTransition { .. })
        ));
        cleanup(database, path);
    }

    #[test]
    fn failed_and_cancelled_run_events_interrupt_owned_tasks() {
        for terminal_event in [
            serde_json::json!({
                "type": "run.failed",
                "code": "runtime.process_crashed",
                "message": "sidecar crashed"
            }),
            serde_json::json!({ "type": "run.cancelled" }),
        ] {
            let (database, path) = test_database();
            let conversation_id = conversation_id(&database);
            let goal = database
                .goals()
                .create(goal_input(&conversation_id, GoalStatus::Active))
                .unwrap();
            let run = database
                .create_run(&conversation_id, "execute", None)
                .unwrap()
                .run;
            let task = database
                .work_tasks()
                .create_many_with_policy_ids(
                    vec![task_input(&goal.id, 0)],
                    vec!["standard_v1".to_owned()],
                )
                .unwrap()
                .remove(0);
            let task = database
                .start_task_attempt(StartTaskAttemptInput {
                    id: "terminal-event-running-attempt".to_owned(),
                    task_id: task.id,
                    run_id: run.id.clone(),
                    expected_task_version: 1,
                    kind: TaskAttemptKind::Execution,
                    root_cause: None,
                    finding_ids: Vec::new(),
                })
                .unwrap()
                .task;

            database
                .apply_runtime_event(&run.id, 1, &terminal_event)
                .unwrap();

            assert_eq!(
                database.work_tasks().get(&task.id).unwrap().unwrap().status,
                WorkTaskStatus::Interrupted
            );
            assert_eq!(
                database.list_task_attempts(&task.id, 10).unwrap()[0].status,
                if terminal_event["type"] == "run.cancelled" {
                    TaskAttemptStatus::Cancelled
                } else {
                    TaskAttemptStatus::Failed
                }
            );
            cleanup(database, path);
        }
    }

    #[test]
    fn task_block_resume_and_skip_transitions_are_supported() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "execute", None)
            .unwrap()
            .run;
        let tasks = database.work_tasks();
        let task = tasks.create(task_input(&goal.id, 0)).unwrap();
        let task = tasks.start(&task.id, &run.id).unwrap();
        let task = tasks.block(&task.id, "input needed".to_owned()).unwrap();
        let task = tasks.start(&task.id, &run.id).unwrap();
        let task = tasks.block(&task.id, "obsolete".to_owned()).unwrap();
        let task = tasks.skip(&task.id).unwrap();
        assert_eq!(task.status, WorkTaskStatus::Skipped);

        let queued = tasks.create(task_input(&goal.id, 1)).unwrap();
        assert_eq!(
            tasks.skip(&queued.id).unwrap().status,
            WorkTaskStatus::Skipped
        );
        cleanup(database, path);
    }

    #[test]
    fn evidence_rejects_duplicates_bad_pairs_and_cross_conversation_references() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let evidence = database.task_evidence();
        evidence
            .add(external_evidence(&task.id, "https://example.com/proof/1"))
            .unwrap();
        assert!(matches!(
            evidence.add(external_evidence(&task.id, "https://example.com/proof/1")),
            Err(RepositoryError::DuplicateEvidence)
        ));

        let mut invalid = external_evidence(&task.id, "https://example.com/proof/2");
        invalid.evidence_type = EvidenceType::Artifact;
        assert!(matches!(
            evidence.add(invalid),
            Err(RepositoryError::InvalidReference(_))
        ));

        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO messages(
                        id, conversation_id, role, kind, content, status, ordinal, created_at, updated_at
                     ) VALUES ('other-message', ?1, 'user', 'text', 'yes', 'completed', 1, 1, 1)",
                    [&other.id],
                )?;
                Ok(())
            })
            .unwrap();
        let cross = AddEvidenceInput {
            id: None,
            task_id: task.id,
            source_run_id: None,
            evidence_type: EvidenceType::UserConfirmation,
            ref_kind: EvidenceReferenceKind::Message,
            ref_id: "other-message".to_owned(),
            summary: "yes".to_owned(),
            metadata: serde_json::json!({}),
            trace_id: None,
            span_id: None,
        };
        assert!(matches!(
            evidence.add(cross),
            Err(RepositoryError::CrossConversationReference)
        ));
        cleanup(database, path);
    }

    #[test]
    fn evidence_validity_updates_without_deleting_history() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let evidence_repository = database.task_evidence();
        let evidence = evidence_repository
            .add(external_evidence(&task.id, "https://example.com/proof/1"))
            .unwrap();
        assert_eq!(
            evidence_repository.validate(&evidence.id).unwrap(),
            EvidenceValidityStatus::Valid
        );
        evidence_repository
            .mark_stale(&evidence.id, "workspace changed".to_owned())
            .unwrap();
        let stored = evidence_repository.get(&evidence.id).unwrap().unwrap();
        assert_eq!(stored.validity_status, EvidenceValidityStatus::Stale);
        assert_eq!(stored.invalid_reason.as_deref(), Some("workspace changed"));
        assert!(stored.checked_at.is_some());
        cleanup(database, path);
    }

    #[test]
    fn deleted_tool_call_evidence_becomes_missing_and_keeps_history() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "collect evidence", None)
            .unwrap()
            .run;
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name,
                        status, result_json, started_at, completed_at, updated_at
                     ) VALUES ('deleted-tool-proof', 'runtime-deleted-tool-proof', ?1, ?2,
                               'test', 'completed', '{}', 1, 2, 2)",
                    params![run.id, conversation_id],
                )?;
                Ok(())
            })
            .unwrap();
        let repository = database.task_evidence();
        let evidence = repository
            .add(AddEvidenceInput {
                id: None,
                task_id: task.id,
                source_run_id: Some(run.id),
                evidence_type: EvidenceType::ToolCall,
                ref_kind: EvidenceReferenceKind::ToolCall,
                ref_id: "deleted-tool-proof".to_owned(),
                summary: "tool output".to_owned(),
                metadata: serde_json::json!({}),
                trace_id: None,
                span_id: None,
            })
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute("DELETE FROM tool_calls WHERE id = 'deleted-tool-proof'", [])?;
                Ok(())
            })
            .unwrap();

        assert_eq!(
            repository.validate(&evidence.id).unwrap(),
            EvidenceValidityStatus::Missing
        );
        let retained = repository.get(&evidence.id).unwrap().unwrap();
        assert_eq!(retained.validity_status, EvidenceValidityStatus::Missing);
        assert!(retained.checked_at.is_some());
        assert!(retained
            .invalid_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("was not found")));
        cleanup(database, path);
    }

    #[test]
    fn deleted_artifact_file_becomes_missing_and_keeps_history() {
        let (database, path) = test_database();
        let artifact_path = path.with_extension("evidence-artifact");
        std::fs::write(&artifact_path, b"proof").unwrap();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "collect evidence", None)
            .unwrap()
            .run;
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO artifacts(
                        id, conversation_id, run_id, display_name, artifact_type,
                        storage_path, status, created_at, updated_at
                     ) VALUES ('deleted-artifact-proof', ?1, ?2, 'report', 'test_result',
                               ?3, 'ready', 1, 1)",
                    params![
                        conversation_id,
                        run.id,
                        artifact_path.to_string_lossy().as_ref()
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let repository = database.task_evidence();
        let evidence = repository
            .add(AddEvidenceInput {
                id: None,
                task_id: task.id,
                source_run_id: Some(run.id),
                evidence_type: EvidenceType::TestResult,
                ref_kind: EvidenceReferenceKind::Artifact,
                ref_id: "deleted-artifact-proof".to_owned(),
                summary: "test report".to_owned(),
                metadata: serde_json::json!({}),
                trace_id: None,
                span_id: None,
            })
            .unwrap();
        std::fs::remove_file(&artifact_path).unwrap();

        assert_eq!(
            repository.validate(&evidence.id).unwrap(),
            EvidenceValidityStatus::Missing
        );
        let retained = repository.get(&evidence.id).unwrap().unwrap();
        assert_eq!(retained.validity_status, EvidenceValidityStatus::Missing);
        assert!(retained.checked_at.is_some());
        assert!(retained
            .invalid_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("was not found")));
        cleanup(database, path);
    }

    #[test]
    fn evidence_accepts_every_supported_local_reference_kind() {
        let (database, path) = test_database();
        let artifact_path = path.with_extension("artifact-proof");
        std::fs::write(&artifact_path, b"proof").unwrap();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let tasks = database.work_tasks();
        let inputs = (0..4)
            .map(|ordinal| task_input(&goal.id, ordinal))
            .collect();
        let created = tasks.create_many(inputs).unwrap();
        let run = database
            .create_run(&conversation_id, "collect evidence", None)
            .unwrap()
            .run;
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name,
                        status, result_json, started_at, completed_at, updated_at
                     ) VALUES ('tool-proof', 'runtime-tool-proof', ?1, ?2, 'test',
                               'completed', '{}', 1, 2, 2)",
                    params![run.id, conversation_id],
                )?;
                connection.execute(
                    "INSERT INTO artifacts(
                        id, conversation_id, run_id, display_name, artifact_type,
                        storage_path, status, created_at, updated_at
                     ) VALUES ('artifact-proof', ?1, ?2, 'report', 'test_result',
                               ?3, 'ready', 1, 1)",
                    params![
                        conversation_id,
                        run.id,
                        artifact_path.to_string_lossy().as_ref()
                    ],
                )?;
                connection.execute(
                    "INSERT INTO run_events(
                        id, run_id, seq, event_type, event_json, created_at, trace_id, span_id
                     ) VALUES ('event-proof', ?1, 1, 'file.changed', '{}', 1,
                               'trace-proof', 'span-proof')",
                    [&run.id],
                )?;
                connection.execute(
                    "INSERT INTO messages(
                        id, conversation_id, run_id, role, kind, content, status,
                        ordinal, created_at, updated_at
                     ) VALUES ('message-proof', ?1, ?2, 'user', 'text', 'confirmed',
                               'completed', 2, 1, 1)",
                    params![conversation_id, run.id],
                )?;
                Ok(())
            })
            .unwrap();

        let repository = database.task_evidence();
        for (task, evidence_type, ref_kind, ref_id) in [
            (
                &created[0],
                EvidenceType::ToolCall,
                EvidenceReferenceKind::ToolCall,
                "tool-proof",
            ),
            (
                &created[1],
                EvidenceType::TestResult,
                EvidenceReferenceKind::Artifact,
                "artifact-proof",
            ),
            (
                &created[2],
                EvidenceType::TraceSpan,
                EvidenceReferenceKind::RunEvent,
                "event-proof",
            ),
            (
                &created[3],
                EvidenceType::UserConfirmation,
                EvidenceReferenceKind::Message,
                "message-proof",
            ),
        ] {
            let evidence = repository
                .add(AddEvidenceInput {
                    id: None,
                    task_id: task.id.clone(),
                    source_run_id: Some(run.id.clone()),
                    evidence_type,
                    ref_kind,
                    ref_id: ref_id.to_owned(),
                    summary: "verified reference".to_owned(),
                    metadata: serde_json::json!({}),
                    trace_id: None,
                    span_id: None,
                })
                .unwrap();
            assert_eq!(
                repository.validate(&evidence.id).unwrap(),
                EvidenceValidityStatus::Valid
            );
        }
        std::fs::remove_file(artifact_path).unwrap();
        cleanup(database, path);
    }

    #[test]
    fn sqlite_busy_retry_is_bounded() {
        let (database, path) = test_database();
        let blocker = Connection::open(&path).unwrap();
        blocker
            .execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .unwrap();
        let conversation_id = conversation_id(&database);
        let started = std::time::Instant::now();
        let result = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Proposed));
        assert!(matches!(
            result,
            Err(RepositoryError::SqliteBusyExhausted {
                attempts: MAX_BUSY_ATTEMPTS
            })
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        blocker.execute_batch("ROLLBACK").unwrap();
        cleanup(database, path);
    }

    #[test]
    fn create_many_rolls_back_every_task_on_validation_failure() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let result = database
            .work_tasks()
            .create_many(vec![task_input(&goal.id, 0), task_input(&goal.id, 0)]);
        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
        assert!(database
            .work_tasks()
            .list_by_goal(&goal.id)
            .unwrap()
            .is_empty());
        cleanup(database, path);
    }

    #[test]
    fn work_snapshot_v2_uses_one_selected_goal_for_root_and_ledger() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let active = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let proposed = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Proposed))
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE goals SET updated_at = '9999999999999' WHERE id = ?1",
                    [proposed.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let (goal, _, _, _, _, _, _, _, ledger, _) =
            database.load_work_snapshot_v2(&conversation_id).unwrap();
        assert_eq!(
            goal.as_ref().map(|goal| goal.id.as_str()),
            Some(active.id.as_str())
        );
        assert_eq!(
            ledger.goal.as_ref().map(|goal| goal.id.as_str()),
            Some(active.id.as_str())
        );
        cleanup(database, path);
    }

    #[test]
    fn execution_cursor_selects_the_task_matching_the_projected_phase() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let blocked = database
            .work_tasks()
            .create(task_input(&goal.id, 0))
            .unwrap();
        let running = database
            .work_tasks()
            .create(task_input(&goal.id, 1))
            .unwrap();
        let run = database
            .create_run(&conversation_id, "cursor phase", None)
            .unwrap();
        let started_blocked = database
            .work_tasks()
            .start(&blocked.id, &run.run.id)
            .unwrap();
        database
            .work_tasks()
            .update(
                &blocked.id,
                started_blocked.version,
                WorkTaskStatus::Blocked,
                None,
                Some("waiting on dependency".to_owned()),
            )
            .unwrap();
        database
            .work_tasks()
            .start(&running.id, &run.run.id)
            .unwrap();

        let projection = database.task_ledger_projection(&conversation_id).unwrap();
        assert_eq!(
            projection.execution_cursor.current_phase,
            TaskLedgerPhase::Execute
        );
        assert_eq!(
            projection.execution_cursor.resumable_from.as_deref(),
            Some(running.id.as_str())
        );
        cleanup(database, path);
    }

    #[test]
    fn bounded_work_snapshot_keeps_high_ordinal_critical_task_and_latest_valid_evidence() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let tasks = database
            .work_tasks()
            .create_many(
                (0..100)
                    .map(|ordinal| task_input(&goal.id, ordinal))
                    .collect(),
            )
            .unwrap();
        let critical = tasks.last().unwrap();
        let run = database
            .create_run(&conversation_id, "bounded snapshot", None)
            .unwrap();
        database
            .work_tasks()
            .update(
                &critical.id,
                critical.version,
                WorkTaskStatus::InProgress,
                Some(&run.run.id),
                None,
            )
            .unwrap();
        database
            .with_connection(|connection| {
                let transaction = connection.transaction()?;
                let mut statement = transaction.prepare(
                    "INSERT INTO task_evidence(
                        id, task_id, evidence_type, ref_kind, ref_id, summary, metadata_json,
                        validity_status, created_at, checked_at
                     ) VALUES (?1, ?2, 'external_reference', 'source', ?3, 'proof', '{}', ?4, ?5, ?6)",
                )?;
                for index in 0..1_000 {
                    let id = format!("bounded-evidence-{index:04}");
                    let timestamp = (index + 1).to_string();
                    let status = if index == 0 { "valid" } else { "unverified" };
                    statement.execute(params![
                        id,
                        critical.id,
                        format!("source-{index}"),
                        status,
                        timestamp,
                        if index == 0 { Some("1") } else { None },
                    ])?;
                }
                drop(statement);
                let mut attempt_statement = transaction.prepare(
                    "INSERT INTO task_attempts(
                        id, task_id, attempt_number, kind, status, run_id, policy_hash,
                        root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                        version, evidence_rowid_watermark, finding_rowid_watermark,
                        started_at, finished_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, '[]', '[]', NULL,
                               1, 0, 0, ?8, ?9)",
                )?;
                for (task_index, task) in tasks.iter().enumerate() {
                    for attempt_number in 1..=10_i64 {
                        let running = task.id == critical.id && attempt_number == 10;
                        attempt_statement.execute(params![
                            format!("bounded-attempt-{task_index:03}-{attempt_number:02}"),
                            task.id,
                            attempt_number,
                            "execution",
                            if running { "running" } else { "succeeded" },
                            run.run.id,
                            LEGACY_POLICY_HASH,
                            attempt_number.to_string(),
                            if running { None::<String> } else { Some(attempt_number.to_string()) },
                        ])?;
                    }
                }
                drop(attempt_statement);
                transaction.commit()
            })
            .unwrap();

        let started = std::time::Instant::now();
        let (_, snapshot_tasks, evidence, _, _, _, _, attempts, ledger, history) =
            database.load_work_snapshot_v2(&conversation_id).unwrap();
        assert!(started.elapsed() < Duration::from_millis(750));
        assert_eq!(snapshot_tasks.len(), WORK_SNAPSHOT_TASK_LIMIT);
        assert!(snapshot_tasks.iter().any(|task| task.id == critical.id));
        assert!(ledger
            .active_tasks
            .iter()
            .any(|task| task.task_id == critical.id));
        assert_eq!(
            ledger.execution_cursor.resumable_from.as_deref(),
            Some(critical.id.as_str())
        );
        assert!(evidence.iter().any(|record| {
            record.id == "bounded-evidence-0000"
                && record.validity_status == EvidenceValidityStatus::Valid
        }));
        assert_eq!(history["tasks"]["total"], 100);
        assert_eq!(history["evidence"]["total"], 1_000);
        assert!(evidence.len() <= WORK_SNAPSHOT_TASK_LIMIT * WORK_SNAPSHOT_EVIDENCE_PER_TASK_LIMIT);
        let selected_task_ids = snapshot_tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<HashSet<_>>();
        assert!(attempts
            .iter()
            .all(|attempt| selected_task_ids.contains(attempt.task_id.as_str())));
        assert!(attempts.len() <= WORK_SNAPSHOT_TASK_LIMIT * WORK_SNAPSHOT_ATTEMPTS_PER_TASK_LIMIT);
        assert!(attempts.iter().any(|attempt| {
            attempt.task_id == critical.id && attempt.status == TaskAttemptStatus::Running
        }));
        cleanup(database, path);
    }

    #[test]
    fn read_transaction_does_not_mix_work_graph_and_a1_commit_points() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        database
            .with_connection(|connection| {
                connection.pragma_update(None, "journal_mode", "WAL")?;
                Ok(())
            })
            .unwrap();
        let writer = Database::open(path.clone()).unwrap();
        let (start_tx, receive_start) = std::sync::mpsc::channel();
        let (committed_tx, receive_committed) = std::sync::mpsc::channel();
        let writer_goal_id = goal.id.clone();
        let writer_conversation_id = conversation_id.clone();
        let writer_handle = std::thread::spawn(move || {
            receive_start.recv().unwrap();
            writer
                .with_connection(|connection| {
                    let transaction = connection.transaction()?;
                    transaction.execute(
                        "UPDATE goals SET title = 'after', updated_at = '9999999999999' WHERE id = ?1",
                        [writer_goal_id.as_str()],
                    )?;
                    transaction.execute(
                        "INSERT INTO plan_revisions(
                            id, goal_id, conversation_id, revision, title, summary, tasks_json,
                            status, created_by, created_at
                         ) VALUES ('atomic-plan', ?1, ?2, 1, 'after', 'after', '[]',
                                   'proposed', 'test', '9999999999999')",
                        params![writer_goal_id, writer_conversation_id],
                    )?;
                    transaction.commit()
                })
                .unwrap();
            committed_tx.send(()).unwrap();
        });

        with_read_transaction(&database, |transaction| {
            let goals = load_goals(transaction, &conversation_id)?;
            assert_eq!(goals[0].title, "Deliver A0");
            start_tx.send(()).unwrap();
            receive_committed.recv().unwrap();
            let (plans, _, _) = super::super::a1_workflow::load_bounded_a1_snapshot_for_goal(
                transaction,
                &conversation_id,
                &goal.id,
                WORK_SNAPSHOT_PLAN_LIMIT,
                WORK_SNAPSHOT_FINDING_LIMIT,
                WORK_SNAPSHOT_ACCEPTANCE_LIMIT,
            )
            .map_err(RepositoryError::database)?;
            assert!(plans.is_empty());
            Ok(())
        })
        .unwrap();
        writer_handle.join().unwrap();

        let (selected, _, _, plans, _, _, _, _, _, _) =
            database.load_work_snapshot_v2(&conversation_id).unwrap();
        assert_eq!(selected.unwrap().title, "after");
        assert_eq!(plans[0].id, "atomic-plan");
        cleanup(database, path);
    }

    #[test]
    fn host_frozen_run_profile_ignores_runtime_legacy_spoof_and_only_elevates_risk() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let run = database
            .create_run(&conversation_id, "Host profile authority", None)
            .unwrap()
            .run;
        assert_eq!(
            database
                .validation_policy_id_for_run(&run.id, None)
                .unwrap(),
            "legacy_v1"
        );
        database
            .apply_runtime_event(
                &run.id,
                1,
                &serde_json::json!({
                    "type": "run.started",
                    "executionProfileId": "legacy",
                    "requestSnapshot": { "executionProfile": "legacy" }
                }),
            )
            .unwrap();
        database
            .freeze_run_execution_profile(&run.id, "durable_v2")
            .unwrap();
        assert_eq!(
            database
                .validation_policy_id_for_run(&run.id, None)
                .unwrap(),
            "standard_v1"
        );
        assert_eq!(
            database
                .validation_policy_id_for_run(&run.id, Some("critical"))
                .unwrap(),
            "high_risk_v1"
        );
        assert_eq!(
            database
                .frozen_run_execution_profile_id(&run.id)
                .unwrap()
                .as_deref(),
            Some("durable_v2")
        );
        assert!(matches!(
            database.freeze_run_execution_profile(&run.id, "legacy"),
            Err(RepositoryError::InvalidValidationPolicy(_))
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE run_execution_profiles SET profile_hash = ?2 WHERE run_id = ?1",
                params![run.id, "0".repeat(64)],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            database.frozen_run_execution_profile_id(&run.id),
            Err(RepositoryError::InvalidValidationPolicy(_))
        ));
        cleanup(database, path);
    }

    #[test]
    fn validation_policy_contract_matches_runtime_hashes_and_fails_closed_on_tamper() {
        let legacy = validation_policy_snapshot("legacy_v1").unwrap();
        let standard = validation_policy_snapshot("standard_v1").unwrap();
        let high = validation_policy_snapshot("high_risk_v1").unwrap();
        assert_eq!(legacy.hash, LEGACY_POLICY_HASH);
        assert_eq!(standard.hash, STANDARD_POLICY_HASH);
        assert_eq!(high.hash, HIGH_RISK_POLICY_HASH);
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "id": "legacy_v1",
                "riskLevel": "legacy",
                "requiredChecks": [],
                "allowedCheckTypes": ["test", "inspection", "review", "manual", "other"],
                "reviewerPolicy": "legacy",
                "maxRepairAttempts": 0,
                "completionRequiresAcceptance": false,
                "hash": "4129f5db32d88d7070c59ed2351aab9c6ad59c8cc380451901bb10962c5bee92"
            })
        );
        assert_eq!(
            serde_json::to_value(&standard).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "id": "standard_v1",
                "riskLevel": "standard",
                "requiredChecks": ["inspection"],
                "allowedCheckTypes": ["test", "inspection", "review", "manual"],
                "reviewerPolicy": "host_validated",
                "maxRepairAttempts": 2,
                "completionRequiresAcceptance": true,
                "hash": "bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d"
            })
        );
        assert_eq!(
            serde_json::to_value(&high).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "id": "high_risk_v1",
                "riskLevel": "high",
                "requiredChecks": ["inspection", "review"],
                "allowedCheckTypes": ["test", "inspection", "review", "manual"],
                "reviewerPolicy": "independent",
                "maxRepairAttempts": 1,
                "completionRequiresAcceptance": true,
                "hash": "bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea"
            })
        );
        assert!(matches!(
            validation_policy_snapshot("custom"),
            Err(RepositoryError::InvalidValidationPolicy(_))
        ));

        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE task_validation_policies
                     SET snapshot_json = json_set(snapshot_json, '$.riskLevel', 'high')
                     WHERE task_id = ?1",
                    [task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            database.task_validation_policy(&task.id),
            Err(RepositoryError::InvalidValidationPolicy(_))
        ));
        cleanup(database, path);
    }

    #[test]
    fn standard_attempt_is_atomic_idempotent_and_survives_restart() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let run = database
            .create_run(&conversation_id, "durable execution", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &serde_json::json!({ "type": "run.started" }))
            .unwrap();
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let start = StartTaskAttemptInput {
            id: "attempt-standard-1".to_owned(),
            task_id: task.id.clone(),
            run_id: run.id.clone(),
            expected_task_version: 1,
            kind: TaskAttemptKind::Execution,
            root_cause: None,
            finding_ids: Vec::new(),
        };
        let started = database.start_task_attempt(start.clone()).unwrap();
        assert_eq!(started.task.status, WorkTaskStatus::InProgress);
        assert_eq!(started.attempt.as_ref().unwrap().attempt_number, 1);
        assert_eq!(
            database
                .start_task_attempt(start)
                .unwrap()
                .attempt
                .unwrap()
                .id,
            "attempt-standard-1"
        );
        let evidence =
            add_host_check_evidence(&database, &task.id, &run.id, "inspection", "standard");
        let finish = FinishTaskAttemptInput {
            id: "attempt-standard-1".to_owned(),
            task_id: task.id.clone(),
            run_id: run.id.clone(),
            expected_task_version: 2,
            expected_attempt_version: 1,
            status: TaskAttemptStatus::Succeeded,
            failure_reason: None,
        };
        let finished = database.finish_task_attempt(finish.clone()).unwrap();
        assert_eq!(finished.task.status, WorkTaskStatus::Completed);
        assert_eq!(finished.attempt.status, TaskAttemptStatus::Succeeded);
        assert_eq!(finished.attempt.evidence_ids, vec![evidence.id]);
        assert_eq!(
            database
                .finish_task_attempt(finish)
                .unwrap()
                .attempt
                .version,
            2
        );
        drop(database);

        let reopened = Database::open(path.clone()).unwrap();
        let history = reopened.list_task_attempts(&task.id, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, TaskAttemptStatus::Succeeded);
        let (_, _, _, _, _, _, policies, attempts, _, _) =
            reopened.load_work_snapshot_v2(&conversation_id).unwrap();
        assert_eq!(policies[0].snapshot.id, "standard_v1");
        assert_eq!(attempts[0].id, "attempt-standard-1");
        cleanup(reopened, path);
    }

    #[test]
    fn concurrent_attempt_starts_allow_exactly_one_cas_winner() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let run = database
            .create_run(&conversation_id, "concurrent attempt", None)
            .unwrap()
            .run;
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let contenders = [
            Database::open(path.clone()).unwrap(),
            Database::open(path.clone()).unwrap(),
        ];
        let barrier = Arc::new(Barrier::new(contenders.len()));
        let handles = contenders
            .into_iter()
            .enumerate()
            .map(|(index, contender)| {
                let barrier = Arc::clone(&barrier);
                let task_id = task.id.clone();
                let run_id = run.id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    contender.start_task_attempt(StartTaskAttemptInput {
                        id: format!("attempt-race-{index}"),
                        task_id,
                        run_id,
                        expected_task_version: 1,
                        kind: TaskAttemptKind::Execution,
                        root_cause: None,
                        finding_ids: Vec::new(),
                    })
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert_eq!(database.list_task_attempts(&task.id, 10).unwrap().len(), 1);
        cleanup(database, path);
    }

    #[test]
    fn durable_execution_is_single_use_and_new_run_cannot_reset_task_budget() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let first_run = database
            .create_run(&conversation_id, "first execution", None)
            .unwrap()
            .run;
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let started = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "execution-once".to_owned(),
                task_id: task.id.clone(),
                run_id: first_run.id.clone(),
                expected_task_version: task.version,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .unwrap();
        let failed = database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: "execution-once".to_owned(),
                task_id: task.id.clone(),
                run_id: first_run.id.clone(),
                expected_task_version: started.task.version,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Failed,
                failure_reason: Some("validation failed".to_owned()),
            })
            .unwrap();
        assert!(matches!(
            database.start_task_attempt(StartTaskAttemptInput {
                id: "execution-twice-same-run".to_owned(),
                task_id: task.id.clone(),
                run_id: first_run.id.clone(),
                expected_task_version: failed.task.version,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            }),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        database
            .apply_runtime_event(
                &first_run.id,
                1,
                &serde_json::json!({ "type": "run.failed" }),
            )
            .unwrap();
        let second_run = database
            .create_run(&conversation_id, "new user Run", None)
            .unwrap()
            .run;
        assert!(matches!(
            database.start_task_attempt(StartTaskAttemptInput {
                id: "execution-twice-new-run".to_owned(),
                task_id: task.id.clone(),
                run_id: second_run.id,
                expected_task_version: failed.task.version,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            }),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        assert_eq!(database.list_task_attempts(&task.id, 10).unwrap().len(), 1);
        cleanup(database, path);
    }

    #[test]
    fn repair_budget_exhaustion_persists_real_blockage_without_extra_attempt() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let run = database
            .create_run(&conversation_id, "bounded repairs", None)
            .unwrap()
            .run;
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'completed' WHERE id = ?1",
                    [task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        let finding = database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "medium",
                "correctness",
                "repair me",
                "root cause",
                "open",
                "review-run",
            )
            .unwrap();

        let mut task_version = 1;
        for number in 1..=2 {
            let attempt_id = format!("repair-{number}");
            let started = database
                .start_task_attempt(StartTaskAttemptInput {
                    id: attempt_id.clone(),
                    task_id: task.id.clone(),
                    run_id: run.id.clone(),
                    expected_task_version: task_version,
                    kind: TaskAttemptKind::Repair,
                    root_cause: Some("same root cause".to_owned()),
                    finding_ids: vec![finding.id.clone()],
                })
                .unwrap();
            task_version = started.task.version;
            let finished = database
                .finish_task_attempt(FinishTaskAttemptInput {
                    id: attempt_id,
                    task_id: task.id.clone(),
                    run_id: run.id.clone(),
                    expected_task_version: task_version,
                    expected_attempt_version: 1,
                    status: TaskAttemptStatus::Failed,
                    failure_reason: Some("repair did not validate".to_owned()),
                })
                .unwrap();
            task_version = finished.task.version;
        }
        let exhausted = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "repair-3".to_owned(),
                task_id: task.id.clone(),
                run_id: run.id.clone(),
                expected_task_version: task_version,
                kind: TaskAttemptKind::Repair,
                root_cause: Some("same root cause".to_owned()),
                finding_ids: vec![finding.id.clone()],
            })
            .unwrap();
        assert!(exhausted.budget_exhausted);
        assert!(exhausted.attempt.is_none());
        assert_eq!(exhausted.task.status, WorkTaskStatus::Blocked);
        assert!(exhausted
            .task
            .blocked_reason
            .as_deref()
            .unwrap()
            .contains("human escalation required"));
        assert_eq!(database.list_task_attempts(&task.id, 10).unwrap().len(), 2);
        database
            .apply_runtime_event(&run.id, 1, &serde_json::json!({ "type": "run.completed" }))
            .unwrap();
        let new_run = database
            .create_run(&conversation_id, "new Run cannot reset repair budget", None)
            .unwrap()
            .run;
        let still_exhausted = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "repair-new-run".to_owned(),
                task_id: task.id.clone(),
                run_id: new_run.id.clone(),
                expected_task_version: exhausted.task.version,
                kind: TaskAttemptKind::Repair,
                root_cause: Some("same root cause".to_owned()),
                finding_ids: vec![finding.id],
            })
            .unwrap();
        assert!(still_exhausted.budget_exhausted);
        assert_eq!(database.list_task_attempts(&task.id, 10).unwrap().len(), 2);
        assert!(matches!(
            database.work_tasks().update(
                &task.id,
                still_exhausted.task.version,
                WorkTaskStatus::Skipped,
                None,
                None,
            ),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        assert!(matches!(
            database.start_task_attempt(StartTaskAttemptInput {
                id: "execution-after-budget".to_owned(),
                task_id: task.id.clone(),
                run_id: new_run.id,
                expected_task_version: still_exhausted.task.version,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            }),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        cleanup(database, path);
    }

    #[test]
    fn human_repair_override_is_atomic_idempotent_and_task_lifetime_single_use() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        database
            .save_lifecycle_hook(
                "override-replay-hook",
                "Audit repair override",
                "before_tool",
                "task_repair_escalate_start",
                "annotate",
                "operator escalation audited",
                true,
                10,
            )
            .unwrap();
        let (input, _, approval) = approved_repair_override(
            &database,
            &fixture,
            "operator-repair-attempt",
            "operator-repair-tool-call",
        );
        let canonical_tool_input = task_repair_override_input_value(&input);
        crate::lifecycle_hooks::before_tool(
            &database,
            &fixture.run_id,
            "operator-repair-tool-call",
            "task_repair_escalate_start",
            &canonical_tool_input,
        )
        .unwrap();

        let started = database.start_task_repair_override(input.clone()).unwrap();
        assert_eq!(started.task.status, WorkTaskStatus::InProgress);
        assert_eq!(started.task.version, fixture.task.version + 1);
        assert_eq!(started.attempt.kind, TaskAttemptKind::Repair);
        assert_eq!(started.attempt.status, TaskAttemptStatus::Running);
        assert_eq!(started.attempt.attempt_number, 3);
        assert_eq!(started.override_event.normal_repair_budget, 2);
        assert_eq!(started.override_event.normal_repair_used, 2);
        assert_eq!(started.override_event.override_count, 1);
        assert_eq!(started.override_event.approval_id, approval.id);

        let replayed = database.start_task_repair_override(input.clone()).unwrap();
        assert_eq!(replayed.override_event.id, started.override_event.id);
        assert_eq!(replayed.attempt.id, started.attempt.id);
        let (event_count, attempt_count, approval_claim, tool_status): (
            i64,
            i64,
            (Option<i64>, Option<String>),
            String,
        ) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at, claimed_by_run_id FROM approvals WHERE id = ?1",
                        [input.approval_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls WHERE id = ?1",
                        [input.tool_call_id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(event_count, 1);
        assert_eq!(attempt_count, 3);
        assert!(approval_claim.0.is_some());
        assert_eq!(approval_claim.1.as_deref(), Some(fixture.run_id.as_str()));
        assert_eq!(tool_status, "running");
        database
            .complete_host_tool_call(
                &fixture.run_id,
                "operator-repair-tool-call",
                Some(&serde_json::json!({
                    "attemptId": started.attempt.id.clone(),
                    "overrideEventId": started.override_event.id.clone(),
                })),
                None,
            )
            .unwrap();
        assert!(database
            .preflight_task_repair_override(PreflightTaskRepairOverrideInput {
                task_id: input.task_id.clone(),
                attempt_id: input.attempt_id.clone(),
                run_id: input.run_id.clone(),
                conversation_id: input.conversation_id.clone(),
                expected_task_version: input.expected_task_version,
                root_cause: input.root_cause.clone(),
                finding_ids: input.finding_ids.clone(),
                escalation_reason: input.escalation_reason.clone(),
            })
            .is_err());
        let (replayed_call, disposition) = database
            .inspect_host_tool_call_replay(
                &fixture.run_id,
                "operator-repair-tool-call",
                "task_repair_escalate_start",
                &canonical_tool_input,
            )
            .unwrap()
            .expect("terminal override ToolCall must be replayable before stale preflight");
        assert_eq!(disposition, HostToolCallDisposition::ReplayTerminal);
        assert_eq!(
            replayed_call.result.as_ref().unwrap()["attemptId"],
            started.attempt.id
        );
        assert_eq!(
            database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*) FROM lifecycle_hook_executions
                         WHERE hook_id = 'override-replay-hook'
                           AND run_id = ?1 AND tool_call_id = 'operator-repair-tool-call'",
                        [fixture.run_id.as_str()],
                        |row| row.get::<_, i64>(0),
                    )
                })
                .unwrap(),
            1,
            "terminal exact replay must bypass before_tool and its audit side effect"
        );

        drop(database);
        let reopened = Database::open(path.clone()).unwrap();
        assert_eq!(
            reopened
                .start_task_repair_override(input.clone())
                .unwrap()
                .override_event
                .id,
            started.override_event.id
        );
        let failed = reopened
            .finish_task_attempt(FinishTaskAttemptInput {
                id: started.attempt.id,
                task_id: fixture.task.id.clone(),
                run_id: fixture.run_id.clone(),
                expected_task_version: started.task.version,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Failed,
                failure_reason: Some("operator-authorized repair still failed".to_owned()),
            })
            .unwrap();
        reopened
            .apply_runtime_event(
                &fixture.run_id,
                2,
                &serde_json::json!({ "type": "run.completed" }),
            )
            .unwrap();
        let next_run = reopened
            .create_run(
                &fixture.conversation_id,
                "a new Run cannot reset the override",
                None,
            )
            .unwrap()
            .run;
        assert!(reopened
            .apply_runtime_event(
                &next_run.id,
                1,
                &serde_json::json!({ "type": "run.started" }),
            )
            .unwrap());
        reopened
            .freeze_run_execution_profile(&next_run.id, "durable_v2")
            .unwrap();
        let second_fixture = RepairOverrideFixture {
            conversation_id: fixture.conversation_id,
            run_id: next_run.id,
            task: failed.task,
            finding: fixture.finding,
        };
        let (second_input, _, second_approval) = approved_repair_override(
            &reopened,
            &second_fixture,
            "second-operator-repair-attempt",
            "second-operator-repair-tool-call",
        );
        assert!(matches!(
            reopened.start_task_repair_override(second_input),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        let (events, attempts, second_claimed): (i64, i64, Option<i64>) = reopened
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [second_fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                        [second_fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at FROM approvals WHERE id = ?1",
                        [second_approval.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(events, 1);
        assert_eq!(attempts, 3);
        assert!(second_claimed.is_none());
        cleanup(reopened, path);
    }

    #[test]
    fn completed_run_terminalizes_inflight_override_without_erasing_audit_history() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, tool, approval) = approved_repair_override(
            &database,
            &fixture,
            "run-completed-override-attempt",
            "run-completed-override-tool",
        );
        let started = database.start_task_repair_override(input).unwrap();

        database
            .apply_runtime_event(
                &fixture.run_id,
                2,
                &serde_json::json!({ "type": "run.completed" }),
            )
            .unwrap();

        let (event_count, attempt_status, task_status, tool_status, approval_status, claimed_at): (
            i64,
            String,
            String,
            String,
            String,
            Option<i64>,
        ) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM task_attempts WHERE id = ?1",
                        [started.attempt.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM work_tasks WHERE id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls WHERE id = ?1",
                        [tool.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM approvals WHERE id = ?1",
                        [approval.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at FROM approvals WHERE id = ?1",
                        [approval.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(event_count, 1);
        assert_eq!(attempt_status, "failed");
        assert_eq!(task_status, "interrupted");
        assert_eq!(tool_status, "interrupted");
        assert_eq!(approval_status, "approved");
        assert!(claimed_at.is_some());
        assert!(database
            .complete_host_tool_call(
                &fixture.run_id,
                "run-completed-override-tool",
                Some(&serde_json::json!({"late":true})),
                None,
            )
            .is_err());

        cleanup(database, path);
    }

    #[test]
    fn repair_override_rechecks_managed_duration_after_approval_without_partial_work_writes() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, tool, approval) = approved_repair_override(
            &database,
            &fixture,
            "managed-budget-override-attempt",
            "managed-budget-override-tool",
        );
        let created_at = timestamp().parse::<i64>().unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO child_run_delegations(
                        id, parent_run_id, child_run_id, child_conversation_id, tool_call_id,
                        worker_agent_id, objective, context, status, max_duration_ms,
                        max_total_tokens, max_output_tokens, max_tool_calls,
                        total_tokens, output_tokens, created_at, started_at
                     ) VALUES (?1, ?2, ?3, ?4, 'managed-override-fixture', 'fox-general',
                               'managed override', '', 'running', 30000, 4000, 1000, 4,
                               4000, 0, ?5, ?5)",
                    params![
                        Uuid::new_v4().to_string(),
                        fixture.run_id,
                        fixture.run_id,
                        fixture.conversation_id,
                        created_at,
                    ],
                )?;
                // Child token/tool counters are observational for ordinary Child
                // Runs; the Host-enforced duration deadline remains an admission
                // boundary and must be rechecked after human approval.
                connection.execute(
                    "UPDATE runs SET started_at = ?2 WHERE id = ?1",
                    params![fixture.run_id, created_at.saturating_sub(30_001)],
                )?;
                Ok(())
            })
            .unwrap();
        let task_before = database
            .work_tasks()
            .get(&fixture.task.id)
            .unwrap()
            .unwrap();
        let attempt_count_before = database
            .list_task_attempts(&fixture.task.id, 100)
            .unwrap()
            .len();

        let error = database
            .start_task_repair_override(input.clone())
            .expect_err("managed budget exhaustion after approval must block override execution");
        assert!(error.to_string().contains("child_run.budget_exceeded"));
        let (event_count, attempt_count, tool_state, approval_claim): (
            i64,
            i64,
            (String, Option<String>),
            (Option<i64>, Option<String>),
        ) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [&fixture.task.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                        [&fixture.task.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status, error_message FROM tool_calls WHERE id = ?1",
                        [&tool.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at, claimed_by_run_id FROM approvals WHERE id = ?1",
                        [&approval.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?,
                ))
            })
            .unwrap();
        let task_after = database
            .work_tasks()
            .get(&fixture.task.id)
            .unwrap()
            .unwrap();
        assert_eq!(event_count, 0);
        assert_eq!(attempt_count as usize, attempt_count_before);
        assert_eq!(task_after.status, task_before.status);
        assert_eq!(task_after.version, task_before.version);
        assert_eq!(tool_state.0, "failed");
        assert!(tool_state
            .1
            .as_deref()
            .is_some_and(|message| message.contains("child_run.budget_exceeded")));
        assert!(approval_claim.0.is_some());
        assert_eq!(approval_claim.1.as_deref(), Some(fixture.run_id.as_str()));
        assert!(matches!(
            database.start_task_repair_override(input),
            Err(RepositoryError::ConstraintViolation(_))
        ));

        cleanup(database, path);
    }

    #[test]
    fn rewind_cannot_cross_append_only_repair_override_boundary() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, _, _) = approved_repair_override(
            &database,
            &fixture,
            "rewind-boundary-attempt",
            "rewind-boundary-tool",
        );
        let started = database.start_task_repair_override(input).unwrap();
        database
            .complete_host_tool_call(
                &fixture.run_id,
                "rewind-boundary-tool",
                Some(&serde_json::json!({"attemptId":started.attempt.id.clone()})),
                None,
            )
            .unwrap();
        let finished = database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: started.attempt.id,
                task_id: fixture.task.id.clone(),
                run_id: fixture.run_id.clone(),
                expected_task_version: started.task.version,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Failed,
                failure_reason: Some("operator override did not validate".to_owned()),
            })
            .unwrap();
        database
            .apply_runtime_event(
                &fixture.run_id,
                2,
                &serde_json::json!({ "type": "run.completed" }),
            )
            .unwrap();
        let user_message_id: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT id FROM messages WHERE run_id = ?1 AND role = 'user'",
                    [fixture.run_id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        let rewind_error = database
            .rewind_run(
                &fixture.conversation_id,
                &user_message_id,
                "try to erase the operator override",
                None,
            )
            .expect_err("rewind must preserve the append-only repair override boundary");
        assert!(rewind_error.contains("rewind.repair_override_boundary"));
        assert_eq!(
            database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get::<_, i64>(0),
                    )
                })
                .unwrap(),
            1
        );

        let new_run = database
            .create_run(
                &fixture.conversation_id,
                "new Run keeps lifetime budget",
                None,
            )
            .unwrap()
            .run;
        database
            .apply_runtime_event(
                &new_run.id,
                1,
                &serde_json::json!({ "type": "run.started" }),
            )
            .unwrap();
        database
            .freeze_run_execution_profile(&new_run.id, "durable_v2")
            .unwrap();
        assert!(database
            .preflight_task_repair_override(PreflightTaskRepairOverrideInput {
                task_id: fixture.task.id.clone(),
                attempt_id: "second-override-after-rewind".to_owned(),
                run_id: new_run.id,
                conversation_id: fixture.conversation_id.clone(),
                expected_task_version: finished.task.version,
                root_cause: "same unresolved root cause".to_owned(),
                finding_ids: vec![fixture.finding.id.clone()],
                escalation_reason: "attempt to reuse a consumed human override".to_owned(),
            })
            .is_err());

        cleanup(database, path);
    }

    #[test]
    fn repair_override_rejects_stale_or_unexhausted_state_without_partial_writes() {
        for case in ["stale", "unexhausted"] {
            let (database, path) = test_database();
            let fixture = repair_override_fixture(&database, "durable_v2", case == "stale");
            let (mut input, tool_call, approval) = approved_repair_override(
                &database,
                &fixture,
                &format!("{case}-attempt"),
                &format!("{case}-tool-call"),
            );
            if case == "stale" {
                input.expected_task_version -= 1;
            }
            assert!(database.start_task_repair_override(input).is_err());
            let (events, attempts, claim, status): (i64, i64, Option<i64>, String) = database
                .with_connection(|connection| {
                    Ok((
                        connection.query_row(
                            "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                            [fixture.task.id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                            [fixture.task.id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT claimed_at FROM approvals WHERE id = ?1",
                            [approval.id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT status FROM tool_calls WHERE id = ?1",
                            [tool_call.id.as_str()],
                            |row| row.get(0),
                        )?,
                    ))
                })
                .unwrap();
            let task = database
                .work_tasks()
                .get(&fixture.task.id)
                .unwrap()
                .unwrap();
            assert_eq!(events, 0);
            assert_eq!(attempts, if case == "stale" { 2 } else { 0 });
            assert_eq!(task.version, fixture.task.version);
            assert_eq!(task.status, fixture.task.status);
            assert!(claim.is_none());
            assert_eq!(status, "pending");
            cleanup(database, path);
        }
    }

    #[test]
    fn repair_override_rejects_missing_denied_and_cross_bound_approval() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (approved_input, _, _) = approved_repair_override(
            &database,
            &fixture,
            "approved-attempt",
            "approved-tool-call",
        );

        let mut missing = approved_input.clone();
        missing.approval_id = "missing-approval".to_owned();
        assert!(matches!(
            database.start_task_repair_override(missing),
            Err(RepositoryError::MissingReference(_))
        ));

        let mut cross_bound = approved_input.clone();
        let (_, _, other_approval) =
            approved_repair_override(&database, &fixture, "other-attempt", "other-tool-call");
        cross_bound.approval_id = other_approval.id;
        assert!(matches!(
            database.start_task_repair_override(cross_bound),
            Err(RepositoryError::MissingReference(_))
        ));

        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                     VALUES ('cross-run', ?1, 'running', 'test', 0, 1)",
                    [fixture.conversation_id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        database
            .freeze_run_execution_profile("cross-run", "durable_v2")
            .unwrap();
        let mut cross_run = approved_input.clone();
        cross_run.run_id = "cross-run".to_owned();
        assert!(matches!(
            database.start_task_repair_override(cross_run),
            Err(RepositoryError::ConstraintViolation(_))
        ));

        let mut cross_conversation = approved_input.clone();
        cross_conversation.conversation_id = "runtime-spoofed-conversation".to_owned();
        assert!(matches!(
            database.start_task_repair_override(cross_conversation),
            Err(RepositoryError::CrossConversationReference)
        ));

        let cross_task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&fixture.task.goal_id, 1)],
                vec!["standard_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let cross_finding = database
            .add_review_finding(
                &fixture.conversation_id,
                &fixture.task.goal_id,
                Some(&cross_task.id),
                None,
                "medium",
                "correctness",
                "cross task finding",
                "cross task root",
                "open",
                "review-run",
            )
            .unwrap();
        let cross_finding_ids_json =
            serde_json::to_string(&vec![cross_finding.id.clone()]).unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'blocked',
                         blocked_reason = 'repair_budget_exhausted: test'
                     WHERE id = ?1",
                    [cross_task.id.as_str()],
                )?;
                for number in 1..=2_i64 {
                    connection.execute(
                        "INSERT INTO task_attempts(
                            id, task_id, attempt_number, kind, status, run_id, policy_hash,
                            root_cause, finding_ids_json, evidence_ids_json, failure_reason,
                            version, evidence_rowid_watermark, finding_rowid_watermark,
                            started_at, finished_at
                         ) VALUES (?1, ?2, ?3, 'repair', 'failed', ?4, ?5, 'root', ?6,
                                   '[]', 'failed', 2, 0, 0, ?3, ?3)",
                        params![
                            format!("cross-task-repair-{number}"),
                            cross_task.id,
                            number,
                            fixture.run_id,
                            STANDARD_POLICY_HASH,
                            cross_finding_ids_json,
                        ],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let mut cross_task_input = approved_input.clone();
        cross_task_input.task_id = cross_task.id;
        cross_task_input.finding_ids = vec![cross_finding.id];
        cross_task_input.expected_task_version = cross_task.version;
        assert!(matches!(
            database.start_task_repair_override(cross_task_input),
            Err(RepositoryError::ConstraintViolation(_))
        ));

        let denied_tool = database
            .create_host_tool_call(
                &fixture.run_id,
                "denied-override-tool-call",
                "task_repair_escalate_start",
                &task_repair_override_input_value(&approved_input),
                "pending",
                true,
            )
            .unwrap();
        let denied = database
            .create_task_repair_override_approval(
                &denied_tool.id,
                "operator override",
                &serde_json::json!({ "category": "task_repair_budget_override" }),
            )
            .unwrap();
        database
            .resolve_approval(&denied.id, ApprovalDecision::Deny)
            .unwrap()
            .unwrap();
        let mut denied_input = approved_input;
        denied_input.tool_call_id = denied_tool.id;
        denied_input.approval_id = denied.id;
        assert!(matches!(
            database.start_task_repair_override(denied_input),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        let event_count: i64 = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                    [fixture.task.id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(event_count, 0);
        cleanup(database, path);
    }

    #[test]
    fn repair_override_accepts_only_untampered_durable_profile() {
        for profile in ["legacy", "durable_v2_shadow", "graph_readonly_preview"] {
            let (database, path) = test_database();
            let fixture = repair_override_fixture(&database, profile, true);
            let (input, _, _) = approved_repair_override(
                &database,
                &fixture,
                &format!("{profile}-attempt"),
                &format!("{profile}-tool-call"),
            );
            assert!(matches!(
                database.start_task_repair_override(input),
                Err(RepositoryError::InvalidValidationPolicy(_))
            ));
            let event_count: i64 = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )
                })
                .unwrap();
            assert_eq!(event_count, 0);
            cleanup(database, path);
        }
    }

    #[test]
    fn repair_override_preflight_blocks_approval_fatigue_and_rechecks_goal_plan_and_task() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let valid = repair_override_preflight_input(&fixture, "preflight-valid");
        database
            .preflight_task_repair_override(valid.clone())
            .expect("eligible override reaches the approval boundary");
        assert_eq!(approval_count(&database), 0);
        let mut stale = valid.clone();
        stale.expected_task_version += 1;
        assert!(database.preflight_task_repair_override(stale).is_err());
        let mut bad_finding = valid;
        bad_finding.finding_ids = vec!["missing-finding".to_owned()];
        assert!(database
            .preflight_task_repair_override(bad_finding)
            .is_err());
        assert_eq!(approval_count(&database), 0);
        cleanup(database, path);

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", false);
        assert!(database
            .preflight_task_repair_override(repair_override_preflight_input(
                &fixture,
                "unexhausted"
            ))
            .is_err());
        assert_eq!(approval_count(&database), 0);
        cleanup(database, path);

        for terminal_goal_status in ["completed", "cancelled"] {
            let (database, path) = test_database();
            let fixture = repair_override_fixture(&database, "durable_v2", true);
            database
                .with_connection(|connection| {
                    connection.execute(
                        "UPDATE goals SET status = ?2 WHERE id = ?1",
                        params![fixture.task.goal_id, terminal_goal_status],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(database
                .preflight_task_repair_override(repair_override_preflight_input(
                    &fixture,
                    &format!("goal-{terminal_goal_status}")
                ))
                .is_err());
            assert_eq!(approval_count(&database), 0);
            cleanup(database, path);
        }

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        database
            .create_plan_revision(
                &fixture.conversation_id,
                &fixture.task.goal_id,
                "Unapproved replacement",
                "Must be approved before an override prompt",
                serde_json::json!([]),
                &fixture.run_id,
            )
            .unwrap();
        assert!(database
            .preflight_task_repair_override(repair_override_preflight_input(
                &fixture,
                "pending-plan"
            ))
            .is_err());
        assert_eq!(approval_count(&database), 0);
        cleanup(database, path);

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE task_attempts
                     SET status = 'running', finished_at = NULL, failure_reason = NULL
                     WHERE id = (
                         SELECT id FROM task_attempts WHERE task_id = ?1
                         ORDER BY attempt_number DESC LIMIT 1
                     )",
                    [fixture.task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database
            .preflight_task_repair_override(repair_override_preflight_input(
                &fixture,
                "running-attempt"
            ))
            .is_err());
        assert_eq!(approval_count(&database), 0);
        cleanup(database, path);

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'skipped' WHERE id = ?1",
                    [fixture.task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database
            .preflight_task_repair_override(repair_override_preflight_input(
                &fixture,
                "terminal-task"
            ))
            .is_err());
        assert_eq!(approval_count(&database), 0);
        cleanup(database, path);

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (approved, _, _) = approved_repair_override(
            &database,
            &fixture,
            "consumed-override",
            "consumed-override-tool",
        );
        let started = database.start_task_repair_override(approved).unwrap();
        let finished = database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: started.attempt.id,
                task_id: started.task.id.clone(),
                run_id: fixture.run_id.clone(),
                expected_task_version: started.task.version,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Failed,
                failure_reason: Some("override repair failed validation".to_owned()),
            })
            .unwrap();
        let approvals_before = approval_count(&database);
        let mut consumed = repair_override_preflight_input(&fixture, "second-override");
        consumed.expected_task_version = finished.task.version;
        assert!(database.preflight_task_repair_override(consumed).is_err());
        assert_eq!(approval_count(&database), approvals_before);
        cleanup(database, path);
    }

    #[test]
    fn repair_override_atomic_gate_leaves_no_state_when_goal_plan_or_task_turns_invalid() {
        for goal_status in ["completed", "cancelled"] {
            let (database, path) = test_database();
            let fixture = repair_override_fixture(&database, "durable_v2", true);
            let (input, tool, approval) = approved_repair_override(
                &database,
                &fixture,
                &format!("atomic-goal-{goal_status}"),
                &format!("atomic-goal-tool-{goal_status}"),
            );
            database
                .with_connection(|connection| {
                    connection.execute(
                        "UPDATE goals SET status = ?2 WHERE id = ?1",
                        params![fixture.task.goal_id, goal_status],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(database.start_task_repair_override(input.clone()).is_err());
            let residual: (i64, i64, Option<i64>, String) = database
                .with_connection(|connection| {
                    Ok((
                        connection.query_row(
                            "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                            [fixture.task.id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT COUNT(*) FROM task_attempts WHERE id = ?1",
                            [input.attempt_id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT claimed_at FROM approvals WHERE id = ?1",
                            [approval.id.as_str()],
                            |row| row.get(0),
                        )?,
                        connection.query_row(
                            "SELECT status FROM tool_calls WHERE id = ?1",
                            [tool.id.as_str()],
                            |row| row.get(0),
                        )?,
                    ))
                })
                .unwrap();
            assert_eq!(residual, (0, 0, None, "pending".to_owned()));
            cleanup(database, path);
        }

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, tool, approval) =
            approved_repair_override(&database, &fixture, "atomic-plan", "atomic-plan-tool");
        database
            .create_plan_revision(
                &fixture.conversation_id,
                &fixture.task.goal_id,
                "Pending plan",
                "Must block the atomic override",
                serde_json::json!([]),
                &fixture.run_id,
            )
            .unwrap();
        assert!(database.start_task_repair_override(input.clone()).is_err());
        let residual: (i64, i64, Option<i64>, String) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE id = ?1",
                        [input.attempt_id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at FROM approvals WHERE id = ?1",
                        [approval.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls WHERE id = ?1",
                        [tool.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(residual, (0, 0, None, "pending".to_owned()));
        cleanup(database, path);

        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, tool, approval) =
            approved_repair_override(&database, &fixture, "atomic-task", "atomic-task-tool");
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'skipped' WHERE id = ?1",
                    [fixture.task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database.start_task_repair_override(input.clone()).is_err());
        let residual: (i64, i64, Option<i64>, String) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE id = ?1",
                        [input.attempt_id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT claimed_at FROM approvals WHERE id = ?1",
                        [approval.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls WHERE id = ?1",
                        [tool.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(residual, (0, 0, None, "pending".to_owned()));
        cleanup(database, path);
    }

    #[test]
    fn concurrent_exact_repair_override_replay_creates_one_event_and_attempt() {
        let (database, path) = test_database();
        let fixture = repair_override_fixture(&database, "durable_v2", true);
        let (input, _, _) = approved_repair_override(
            &database,
            &fixture,
            "concurrent-override-attempt",
            "concurrent-override-tool-call",
        );
        let contenders = [
            Database::open(path.clone()).unwrap(),
            Database::open(path.clone()).unwrap(),
        ];
        let barrier = Arc::new(Barrier::new(contenders.len()));
        let handles = contenders
            .into_iter()
            .map(|contender| {
                let input = input.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    contender.start_task_repair_override(input)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results[0].override_event.id, results[1].override_event.id);
        let (events, attempts): (i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_repair_override_events WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                        [fixture.task.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(events, 1);
        assert_eq!(attempts, 3);
        cleanup(database, path);
    }

    #[test]
    fn high_risk_completion_requires_independent_review_run() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let implementation = database
            .create_run(&conversation_id, "high risk implementation", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(
                &implementation.id,
                1,
                &serde_json::json!({ "type": "run.started" }),
            )
            .unwrap();
        let reviewer_id = insert_running_reviewer_run(&database, &conversation_id);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["high_risk_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        let started = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "high-attempt".to_owned(),
                task_id: task.id.clone(),
                run_id: implementation.id.clone(),
                expected_task_version: 1,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .unwrap();
        add_host_check_evidence(
            &database,
            &task.id,
            &implementation.id,
            "inspection",
            "high-inspection",
        );
        add_host_check_evidence(
            &database,
            &task.id,
            &implementation.id,
            "review",
            "high-review-same-run",
        );
        database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "info",
                "independent_review",
                "review passed",
                "no blocking issue",
                "resolved",
                &reviewer_id,
            )
            .unwrap();
        let finish = FinishTaskAttemptInput {
            id: "high-attempt".to_owned(),
            task_id: task.id.clone(),
            run_id: implementation.id.clone(),
            expected_task_version: started.task.version,
            expected_attempt_version: 1,
            status: TaskAttemptStatus::Succeeded,
            failure_reason: None,
        };
        assert!(matches!(
            database.finish_task_attempt(finish.clone()),
            Err(RepositoryError::IndependentReviewRequired { .. })
        ));
        add_host_check_evidence(
            &database,
            &task.id,
            &reviewer_id,
            "review",
            "high-review-independent",
        );
        assert_eq!(
            database.finish_task_attempt(finish).unwrap().task.status,
            WorkTaskStatus::Completed
        );
        cleanup(database, path);
    }

    #[test]
    fn high_risk_repair_rejects_stale_review_and_implementation_self_resolution() {
        let (database, path) = test_database();
        let conversation_id = conversation_id(&database);
        let implementation = database
            .create_run(&conversation_id, "high risk repair", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(
                &implementation.id,
                1,
                &serde_json::json!({ "type": "run.started" }),
            )
            .unwrap();
        let reviewer_id = insert_running_reviewer_run(&database, &conversation_id);
        let goal = database
            .goals()
            .create(goal_input(&conversation_id, GoalStatus::Active))
            .unwrap();
        let task = database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![task_input(&goal.id, 0)],
                vec!["high_risk_v1".to_owned()],
            )
            .unwrap()
            .remove(0);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'completed' WHERE id = ?1",
                    [task.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let stale_review =
            add_host_check_evidence(&database, &task.id, &reviewer_id, "review", "stale-review");
        let stale_finding = database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "info",
                "independent_review",
                "old review",
                "predates repair",
                "resolved",
                &reviewer_id,
            )
            .unwrap();
        let blocker = database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "medium",
                "correctness",
                "repair required",
                "must be independently rechecked",
                "open",
                &reviewer_id,
            )
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE task_evidence SET created_at = '1', checked_at = '1' WHERE id = ?1",
                    [stale_review.id.as_str()],
                )?;
                connection.execute(
                    "UPDATE review_findings SET created_at = '1', resolved_at = '1' WHERE id = ?1",
                    [stale_finding.id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let started = database
            .start_task_attempt(StartTaskAttemptInput {
                id: "high-repair".to_owned(),
                task_id: task.id.clone(),
                run_id: implementation.id.clone(),
                expected_task_version: 1,
                kind: TaskAttemptKind::Repair,
                root_cause: Some("fix reviewed defect".to_owned()),
                finding_ids: vec![blocker.id.clone()],
            })
            .unwrap();
        add_host_check_evidence(
            &database,
            &task.id,
            &implementation.id,
            "inspection",
            "fresh-inspection",
        );
        database
            .resolve_review_finding(&blocker.id, "resolved", &implementation.id)
            .unwrap();
        let finish = FinishTaskAttemptInput {
            id: "high-repair".to_owned(),
            task_id: task.id.clone(),
            run_id: implementation.id.clone(),
            expected_task_version: started.task.version,
            expected_attempt_version: 1,
            status: TaskAttemptStatus::Succeeded,
            failure_reason: None,
        };
        assert!(matches!(
            database.finish_task_attempt(finish.clone()),
            Err(RepositoryError::ValidationChecksMissing { check_types, .. })
                if check_types == vec!["review".to_owned()]
        ));

        let fresh_review =
            add_host_check_evidence(&database, &task.id, &reviewer_id, "review", "fresh-review");
        database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task.id),
                None,
                "info",
                "independent_review",
                "repair reviewed",
                "fresh review is bound by run and attempt watermark",
                "resolved",
                &reviewer_id,
            )
            .unwrap();
        assert!(matches!(
            database.finish_task_attempt(finish.clone()),
            Err(RepositoryError::IndependentReviewRequired { .. })
        ));

        database
            .resolve_review_finding(&blocker.id, "resolved", &reviewer_id)
            .unwrap();
        let finished = database.finish_task_attempt(finish).unwrap();
        assert_eq!(finished.task.status, WorkTaskStatus::Completed);
        assert!(!finished.attempt.evidence_ids.contains(&stale_review.id));
        assert!(finished.attempt.evidence_ids.contains(&fresh_review.id));
        cleanup(database, path);
    }
}
